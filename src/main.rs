use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::Instant;

use ziranma_decoder::{
    BigramLanguageModel, Candidate, CandidateSource, CharacterBigramLanguageModel, Decoder,
    audit_abbreviation_codebook, encode_pinyin_phrase, evaluate_character_context_oracle,
    evaluate_context_oracle, evaluate_labeled_recall, evaluate_labeled_rejection_shadow,
    evaluate_oov_cases, evaluate_rejection_shadow, evaluate_sentence_cases, evaluate_synthetic,
    parse_lexicon_tsv, parse_rime_lexicon, parse_ud_conllu,
    select_public_bigram_training_sequences, select_public_calibration_cases,
};

mod benchmark;

use benchmark::{LatencySummary, run_decoder_benchmark};

const DEMO_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
const DEMO_BIGRAM_CORPUS: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");
const DEMO_SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/demo_sentence_cases.tsv");
const LONG_SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/long_sentence_cases.tsv");
const OOV_CASES: &str = include_str!("../tests/fixtures/public/oov_lexicon.tsv");
const PUBLIC_RIME_LEXICON: &str =
    include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
const PUBLIC_UD_TEST: &str =
    include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu");
const PUBLIC_UD_TRAIN: &str =
    include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu");

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };

    if matches!(command, "-h" | "--help" | "help") {
        print_usage();
        return Ok(());
    }

    match command {
        "encode" => run_encode(&arguments[1..]),
        "evaluate" => run_evaluate(&arguments[1..]),
        "public-calibrate" => run_public_calibrate(&arguments[1..]),
        "abbreviation-audit" => run_abbreviation_audit(&arguments[1..]),
        "index-stats" => run_index_stats(&arguments[1..]),
        "public-index-stats" => run_public_index_stats(&arguments[1..]),
        "benchmark" => run_benchmark(&arguments[1..]),
        "search-stats" => run_search_stats(&arguments[1..]),
        "decode" => run_decode(&arguments[1..]),
        "public-decode" => run_public_decode(&arguments[1..]),
        "sentence" => run_sentence(&arguments[1..], true),
        "sentence-unigram" => run_sentence(&arguments[1..], false),
        "public-sentence" => run_public_sentence(&arguments[1..]),
        "sentence-stats" => run_sentence_stats(&arguments[1..]),
        // Preserve the first milestone's convenient `cargo run -- nihk` form.
        observed => run_decode_legacy(observed, &arguments[1..]),
    }
}

fn run_abbreviation_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("abbreviation-audit 不接受额外参数".into());
    }
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let audit = audit_abbreviation_codebook(&imported.entries)?;

    println!("自由混合简拼码本审计（固定公开 Rime 快照）：");
    println!("  不同拼音音节：{}", audit.distinct_pinyin_syllables);
    println!("  不同完整双拼码：{}", audit.distinct_full_codes);
    println!(
        "  完整码碰撞组：{}；单码最多 {} 个拼音标签",
        audit.full_code_collision_groups, audit.maximum_pinyin_labels_per_full_code
    );
    println!(
        "  可作一键简拼的键：{}/26；其中 {} 个键对应多个拼音，单键最多 {} 个",
        audit.abbreviation_keys,
        audit.ambiguous_abbreviation_keys,
        audit.maximum_pinyin_labels_per_abbreviation_key
    );
    println!(
        "  可同时切成两个一键简拼的完整码：{}/{}",
        audit.full_codes_split_as_two_abbreviations, audit.distinct_full_codes
    );
    println!(
        "  两键字符串的最大拼音标签路径数：{}（输入 {}）",
        audit.maximum_labeled_paths_for_two_keys, audit.maximum_labeled_paths_code
    );
    println!(
        "  码字边界唯一可解码：{}",
        if audit.unique_decodability_is_refuted() {
            "否（已有直接反例）"
        } else {
            "本审计未由两键反例判定"
        }
    );
    if let Some(witness) = &audit.immediate_ambiguity_witness {
        println!(
            "  直接反例：{} = 完整音节 {}，也 = 简拼音节 {} + {}",
            witness.observed,
            witness.full_syllable,
            witness.first_abbreviated_syllable,
            witness.second_abbreviated_syllable
        );
    }
    if let Some(key) = audit.fibonacci_witness_key {
        println!("  指数边界反例：重复键 {key:?} 同时允许一键 {key} 与两键 {key}{key}");
        for length in [8, 16, 32] {
            println!(
                "    {length} 键仅边界就有 {} 种解释",
                audit
                    .fibonacci_boundary_parses(length)
                    .expect("the reported witness guarantees a bounded Fibonacci count")
            );
        }
    }
    println!(
        "  信息量下界：区分这些拼音至少需 {:.2} bit；一个字母键最多 {:.2} bit",
        (audit.distinct_pinyin_syllables as f64).log2(),
        26_f64.log2()
    );
    println!("结论：逐音节任意一键/两键混输不是唯一可解码协议；语言模型只能排序歧义，不能消除它。");
    Ok(())
}

fn run_encode(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.is_empty() {
        return Err("encode 需要至少一个无声调拼音音节".into());
    }
    let pinyin = arguments.join(" ");
    let encoded = encode_pinyin_phrase(&pinyin)?;

    println!("拼音：{pinyin}");
    println!("完整自然码：{}", encoded.full_code);
    println!("逐音节：");
    for (syllable, code) in pinyin.split_whitespace().zip(encoded.syllable_codes) {
        println!("  {syllable:<8} {code}");
    }
    Ok(())
}

fn run_evaluate(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("evaluate 不接受额外参数".into());
    }
    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    let unigram_decoder = Decoder::new(lexicon.clone());
    let model = BigramLanguageModel::from_tsv(DEMO_BIGRAM_CORPUS, &lexicon)?;
    let decoder = Decoder::new(lexicon.clone()).with_bigram_model(model);
    let started = Instant::now();
    let report = evaluate_synthetic(&unigram_decoder, &lexicon);
    let unigram_sentences =
        evaluate_sentence_cases(&unigram_decoder, &lexicon, DEMO_SENTENCE_CASES)?;
    let bigram_sentences = evaluate_sentence_cases(&decoder, &lexicon, DEMO_SENTENCE_CASES)?;
    let long_unigram = evaluate_sentence_cases(&unigram_decoder, &lexicon, LONG_SENTENCE_CASES)?;
    let long_bigram = evaluate_sentence_cases(&decoder, &lexicon, LONG_SENTENCE_CASES)?;
    let oov_cases = parse_lexicon_tsv(OOV_CASES)?;
    let oov = evaluate_oov_cases(&unigram_decoder, &oov_cases);
    let rejection_shadow = evaluate_rejection_shadow(
        &unigram_decoder,
        &lexicon,
        &[DEMO_SENTENCE_CASES, LONG_SENTENCE_CASES],
        &oov_cases,
    )?;
    let elapsed = started.elapsed();

    println!(
        "公开合成评测：{} 个词条，{} 个确定性样例",
        lexicon.len(),
        report.total_cases()
    );
    println!("类型             样例   Recall@1  Recall@5  Recall@10");
    for metrics in &report.metrics {
        println!(
            "{:<12} {:>7} {:>9.1}% {:>9.1}% {:>10.1}%",
            metrics.kind.label(),
            metrics.total,
            metrics.recall_at_1() * 100.0,
            metrics.recall_at_5() * 100.0,
            metrics.recall_at_10() * 100.0
        );
    }
    println!(
        "干净输入首选无需纠错：{}/{}（{:.1}%）",
        report.clean_top_1_exact,
        report.clean_total,
        report.clean_top_1_exact_rate() * 100.0
    );
    println!(
        "分离句例 unigram：{}/{} Top-1（{:.1}%），Top-5 {:.1}%",
        unigram_sentences.hits_at_1,
        unigram_sentences.total,
        unigram_sentences.recall_at_1() * 100.0,
        unigram_sentences.recall_at_5() * 100.0
    );
    println!(
        "分离句例 bigram ：{}/{} Top-1（{:.1}%），Top-5 {:.1}%",
        bigram_sentences.hits_at_1,
        bigram_sentences.total,
        bigram_sentences.recall_at_1() * 100.0,
        bigram_sentences.recall_at_5() * 100.0
    );
    println!(
        "长句 unigram：{}/{} Top-1（{:.1}%），Top-5 {:.1}%",
        long_unigram.hits_at_1,
        long_unigram.total,
        long_unigram.recall_at_1() * 100.0,
        long_unigram.recall_at_5() * 100.0
    );
    println!(
        "长句 bigram ：{}/{} Top-1（{:.1}%），Top-5 {:.1}%",
        long_bigram.hits_at_1,
        long_bigram.total,
        long_bigram.recall_at_1() * 100.0,
        long_bigram.recall_at_5() * 100.0
    );
    println!(
        "独立词外探针：{}/{} 首选含未解析键（{:.1}%）；完全原样 {}/{}；按键保留 {}/{}（{:.1}%）",
        oov.top_1_with_unresolved,
        oov.total,
        oov.with_unresolved_rate() * 100.0,
        oov.top_1_fully_unresolved,
        oov.total,
        oov.unresolved_keys,
        oov.observed_keys,
        oov.unresolved_key_rate() * 100.0
    );
    println!(
        "拒识影子完整覆盖：已知句 {}/{}；独立词外 {}/{}（只报告，不改变候选）",
        rejection_shadow.known_with_full_coverage,
        rejection_shadow.known_total,
        rejection_shadow.oov_with_full_coverage,
        rejection_shadow.oov_total
    );
    if let (Some(known), Some(oov)) = (
        rejection_shadow.known_margin_range,
        rejection_shadow.oov_margin_range,
    ) {
        println!(
            "完整路径每键分差：已知句 {:.3}～{:.3}；独立词外 {:.3}～{:.3}",
            known.minimum_per_key, known.maximum_per_key, oov.minimum_per_key, oov.maximum_per_key
        );
    }
    println!("每键最低分差   已知句保留       词外拒识");
    for metrics in &rejection_shadow.thresholds {
        println!(
            "{:>10.1}   {:>3}/{:<3} ({:>5.1}%)   {:>3}/{:<3} ({:>5.1}%)",
            metrics.threshold_per_key,
            metrics.known_accepted,
            metrics.known_total,
            metrics.known_acceptance_rate() * 100.0,
            metrics.oov_rejected,
            metrics.oov_total,
            metrics.oov_rejection_rate() * 100.0
        );
    }
    println!(
        "本次评测耗时：{:.3} ms（仅供本机观察，不是稳定基准）",
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_public_calibrate(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-calibrate 不接受额外参数".into());
    }
    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let train_corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
    let test_corpus = parse_ud_conllu(PUBLIC_UD_TEST)?;
    let training = select_public_bigram_training_sequences(&train_corpus, &imported.entries);
    let language_model =
        BigramLanguageModel::from_token_sequences(&training.sequences, &imported.entries)?;
    let language_model_stats = language_model.stats();
    let character_training_texts = training
        .sequences
        .iter()
        .map(|sequence| sequence.concat())
        .collect::<Vec<_>>();
    let character_language_model =
        CharacterBigramLanguageModel::from_text_sequences(&character_training_texts)?;
    let character_language_model_stats = character_language_model.stats();
    let selection = select_public_calibration_cases(&test_corpus, &imported.entries, 64, 128);
    let imported_stats = imported.stats;
    let lexicon = imported.entries;
    let decoder = Decoder::new(lexicon.clone());
    let sentence_full_recall =
        evaluate_labeled_recall(&decoder, &selection.sentence_full_code_probes);
    let sentence_abbreviation_recall =
        evaluate_labeled_recall(&decoder, &selection.sentence_abbreviation_probes);
    let held_out_token_full_recall =
        evaluate_labeled_recall(&decoder, &selection.held_out_token_full_code_probes);
    let held_out_token_abbreviation_recall =
        evaluate_labeled_recall(&decoder, &selection.held_out_token_abbreviation_probes);
    let sentence_full =
        evaluate_labeled_rejection_shadow(&decoder, &selection.sentence_full_code_probes);
    let sentence_abbreviation =
        evaluate_labeled_rejection_shadow(&decoder, &selection.sentence_abbreviation_probes);
    let held_out_token_full =
        evaluate_labeled_rejection_shadow(&decoder, &selection.held_out_token_full_code_probes);
    let held_out_token_abbreviation =
        evaluate_labeled_rejection_shadow(&decoder, &selection.held_out_token_abbreviation_probes);
    let sentence_full_context = evaluate_context_oracle(
        &decoder,
        &language_model,
        &lexicon,
        &selection.sentence_full_code_probes,
    )?;
    let sentence_abbreviation_context = evaluate_context_oracle(
        &decoder,
        &language_model,
        &lexicon,
        &selection.sentence_abbreviation_probes,
    )?;
    let held_out_token_full_context = evaluate_context_oracle(
        &decoder,
        &language_model,
        &lexicon,
        &selection.held_out_token_full_code_probes,
    )?;
    let held_out_token_abbreviation_context = evaluate_context_oracle(
        &decoder,
        &language_model,
        &lexicon,
        &selection.held_out_token_abbreviation_probes,
    )?;
    let sentence_full_character_context = evaluate_character_context_oracle(
        &decoder,
        &character_language_model,
        &selection.sentence_full_code_probes,
    );
    let sentence_abbreviation_character_context = evaluate_character_context_oracle(
        &decoder,
        &character_language_model,
        &selection.sentence_abbreviation_probes,
    );
    let held_out_token_full_character_context = evaluate_character_context_oracle(
        &decoder,
        &character_language_model,
        &selection.held_out_token_full_code_probes,
    );
    let held_out_token_abbreviation_character_context = evaluate_character_context_oracle(
        &decoder,
        &character_language_model,
        &selection.held_out_token_abbreviation_probes,
    );
    let elapsed = started.elapsed();

    println!(
        "公开独立校准：UD Chinese GSDSimp train {} 句、test {} 句；Rime 词典 {} 项",
        train_corpus.stats.sentences, test_corpus.stats.sentences, imported_stats.imported_entries
    );
    println!(
        "测试集统计：{} 行，{} 个 token，标点 token {}，特殊 token 行 {}",
        test_corpus.stats.source_lines,
        test_corpus.stats.syntactic_tokens,
        test_corpus.stats.punctuation_tokens,
        test_corpus.stats.special_token_rows
    );
    println!(
        "训练集筛选：纯汉字 {}，Rime 可覆盖 {}，保留 {} 序列、{} 词实例",
        training.stats.han_only_sentences,
        training.stats.lexicon_coverable_sentences,
        training.stats.training_sequences,
        training.stats.training_words
    );
    println!(
        "训练集映射：整词 {} 次，逐字回退 {} 次；模型 {} 词、{} 二元组类型、{} 二元组实例",
        training.stats.exact_token_uses,
        training.stats.character_fallback_uses,
        language_model_stats.vocabulary_size,
        language_model_stats.observed_pair_types,
        language_model_stats.observed_pair_instances
    );
    println!(
        "字级模型：{} 字实例、{} 输出符号、{} 二元组类型、{} 二元组实例",
        character_language_model_stats.character_instances,
        character_language_model_stats.vocabulary_size,
        character_language_model_stats.observed_pair_types,
        character_language_model_stats.observed_pair_instances
    );
    println!(
        "自然句筛选：长度合格 {}，纯汉字 {}，Rime 可覆盖 {}，固定取前 {}",
        selection.stats.sentence_length_eligible,
        selection.stats.sentence_han_only,
        selection.stats.sentence_lexicon_coverable,
        selection.stats.selected_sentences
    );
    println!(
        "选中句读音来源：整词 {} 次，逐字回退 {} 次",
        selection.stats.selected_exact_token_uses, selection.stats.selected_character_fallback_uses
    );
    println!(
        "未收整词探针：合格唯一 token {}，固定取前 {}",
        selection.stats.held_out_token_eligible, selection.stats.selected_held_out_tokens
    );
    println!("现行 unigram 候选召回（只读 Top-K）：");
    print_labeled_recall_report("自然句完整码", &sentence_full_recall);
    print_labeled_recall_report("自然句全简拼", &sentence_abbreviation_recall);
    print_labeled_recall_report("未收整词完整码", &held_out_token_full_recall);
    print_labeled_recall_report("未收整词全简拼", &held_out_token_abbreviation_recall);
    print_labeled_rejection_report("自然句完整码", &sentence_full);
    print_labeled_rejection_report("自然句全简拼", &sentence_abbreviation);
    print_labeled_rejection_report("未收整词完整码", &held_out_token_full);
    print_labeled_rejection_report("未收整词全简拼", &held_out_token_abbreviation);
    println!("上下文双路径诊断（仅比较已知预期路径与原始 Top-1，不代表完整搜索）：");
    print_context_oracle_report("自然句完整码", &sentence_full_context);
    print_context_oracle_report("自然句全简拼", &sentence_abbreviation_context);
    print_context_oracle_report("未收整词完整码", &held_out_token_full_context);
    print_context_oracle_report("未收整词全简拼", &held_out_token_abbreviation_context);
    println!("字级上下文双文本诊断（只比较文本语言分，不含 Rime 词频或拼写代价）：");
    print_character_context_oracle_report("自然句完整码", &sentence_full_character_context);
    print_character_context_oracle_report("自然句全简拼", &sentence_abbreviation_character_context);
    print_character_context_oracle_report("未收整词完整码", &held_out_token_full_character_context);
    print_character_context_oracle_report(
        "未收整词全简拼",
        &held_out_token_abbreviation_character_context,
    );
    println!(
        "本次公开校准耗时：{:.3} ms（固定本机观察，不代表真实输入准确率）",
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn print_labeled_recall_report(label: &str, report: &ziranma_decoder::LabeledRecallReport) {
    println!(
        "{label}：Top-1 {}/{}（{:.1}%），Top-5 {}/{}（{:.1}%），Top-10 {}/{}（{:.1}%）",
        report.hits_at_1,
        report.total,
        report.recall_at_1() * 100.0,
        report.hits_at_5,
        report.total,
        report.recall_at_5() * 100.0,
        report.hits_at_10,
        report.total,
        report.recall_at_10() * 100.0
    );
}

fn print_context_oracle_report(label: &str, report: &ziranma_decoder::ContextOracleReport) {
    println!(
        "{label}：原始相符 {}/{}；原始不符中预期路径胜 {}、平 {}、原 Top-1 胜 {}",
        report.unigram_top_1_matches_expected,
        report.total,
        report.incorrect_expected_path_preferred,
        report.incorrect_context_ties,
        report.incorrect_baseline_path_preferred
    );
    if let Some(range) = report.incorrect_margin_range {
        println!(
            "{label}预期减原 Top-1 的上下文每键分差：{:.3}～{:.3}",
            range.minimum_per_key, range.maximum_per_key
        );
    }
    println!(
        "{label}双路径上限：{}/{}（{:.1}%）",
        report.oracle_pair_matches_expected,
        report.total,
        report.oracle_pair_accuracy() * 100.0
    );
}

fn print_character_context_oracle_report(
    label: &str,
    report: &ziranma_decoder::CharacterContextOracleReport,
) {
    println!(
        "{label}：原始相符 {}/{}；原始不符中答案文本胜 {}、平 {}、原 Top-1 文本胜 {}",
        report.unigram_top_1_matches_expected,
        report.total,
        report.incorrect_expected_text_preferred,
        report.incorrect_context_ties,
        report.incorrect_baseline_text_preferred
    );
    if let Some(range) = report.incorrect_margin_range {
        println!(
            "{label}答案减原 Top-1 的字级每键分差：{:.3}～{:.3}",
            range.minimum_per_key, range.maximum_per_key
        );
    }
    println!(
        "{label}字级双文本上限：{}/{}（{:.1}%）",
        report.oracle_pair_matches_expected,
        report.total,
        report.oracle_pair_accuracy() * 100.0
    );
    println!(
        "{label}原始不符文本长度：答案更长 {}、等长 {}、答案更短 {}",
        report.incorrect_expected_text_longer,
        report.incorrect_equal_text_length,
        report.incorrect_expected_text_shorter
    );
    if report.incorrect_equal_text_length > 0 {
        println!(
            "{label}等长样本字级胜负：答案胜 {}、平 {}、原 Top-1 胜 {}",
            report.incorrect_equal_length_expected_preferred,
            report.incorrect_equal_length_ties,
            report.incorrect_equal_length_baseline_preferred
        );
    }
    if let Some(range) = report.incorrect_average_margin_range {
        println!(
            "{label}答案减原 Top-1 的字级平均分差：{:.3}～{:.3}",
            range.minimum, range.maximum
        );
    }
    println!(
        "{label}按字符转移平均后的双文本上限：{}/{}（{:.1}%；答案胜 {}、平 {}、原 Top-1 胜 {}）",
        report.average_oracle_pair_matches_expected,
        report.total,
        report.average_oracle_pair_accuracy() * 100.0,
        report.incorrect_average_expected_preferred,
        report.incorrect_average_context_ties,
        report.incorrect_average_baseline_preferred
    );
}

fn print_labeled_rejection_report(
    label: &str,
    report: &ziranma_decoder::LabeledRejectionShadowReport,
) {
    println!(
        "{label}现行 Top-1：文本相符 {}/{}；不符 {}（其中完整覆盖 {}）",
        report.top_1_matches_expected,
        report.total,
        report.top_1_differs,
        report.incorrect_with_full_coverage
    );
    if let Some(range) = report.correct_margin_range {
        println!(
            "{label}相符结果每键分差：{:.3}～{:.3}",
            range.minimum_per_key, range.maximum_per_key
        );
    }
    if let Some(range) = report.incorrect_margin_range {
        println!(
            "{label}不符完整路径每键分差：{:.3}～{:.3}",
            range.minimum_per_key, range.maximum_per_key
        );
    }
    println!("每键最低分差   相符结果保留       不符结果拒识");
    for metrics in &report.thresholds {
        println!(
            "{:>10.1}   {:>3}/{:<3} ({:>5.1}%)   {:>3}/{:<3} ({:>5.1}%)",
            metrics.threshold_per_key,
            metrics.correct_accepted,
            metrics.correct_total,
            metrics.correct_acceptance_rate() * 100.0,
            metrics.incorrect_rejected,
            metrics.incorrect_total,
            metrics.incorrect_rejection_rate() * 100.0
        );
    }
}

fn run_index_stats(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("index-stats 不接受额外参数".into());
    }
    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    let decoder = Decoder::new(lexicon);
    let stats = decoder.index_stats();

    println!("紧凑逐音节 trie：");
    println!("  节点数：{}", stats.node_count);
    println!("  音节边数：{}", stats.edge_count);
    println!("  词条终点数：{}", stats.terminal_count);
    println!("  非空终点节点数：{}", stats.terminal_node_count);
    println!("  单节点最大词条数：{}", stats.maximum_terminal_fanout);
    println!(
        "  隐式表示的全码/简拼拼写数：{}",
        stats.represented_spelling_count
    );
    println!("  最长词条音节数：{}", stats.maximum_syllables);
    Ok(())
}

fn run_public_index_stats(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-index-stats 不接受额外参数".into());
    }
    let import_started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let import_elapsed = import_started.elapsed();
    let stats = imported.stats;
    let build_started = Instant::now();
    let decoder = Decoder::new(imported.entries);
    let build_elapsed = build_started.elapsed();
    let index = decoder.index_stats();

    println!("Rime pinyin-simp 固定公开快照：");
    println!("  上游数据行：{}", stats.source_rows);
    println!("  导入词条：{}", stats.imported_entries);
    println!("  零权重升至 1：{}", stats.zero_weights_floored);
    println!("  跳过的不支持拼音：{}", stats.unsupported_pinyin_rows);
    println!("  跳过的超长词条：{}", stats.too_many_syllable_rows);
    println!("  跳过的重复词条：{}", stats.duplicate_rows);
    println!("紧凑逐音节 trie：");
    println!("  节点数：{}", index.node_count);
    println!("  音节边数：{}", index.edge_count);
    println!("  词条终点数：{}", index.terminal_count);
    println!("  非空终点节点数：{}", index.terminal_node_count);
    println!("  单节点最大词条数：{}", index.maximum_terminal_fanout);
    println!(
        "  隐式表示的全码/简拼拼写数：{}",
        index.represented_spelling_count
    );
    println!("  最长词条音节数：{}", index.maximum_syllables);
    println!(
        "  导入耗时：{:.3} ms；建索引耗时：{:.3} ms（仅供本机观察）",
        import_elapsed.as_secs_f64() * 1000.0,
        build_elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_benchmark(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if cfg!(debug_assertions) {
        return Err("benchmark 必须使用 cargo run --release -- benchmark [重复次数]".into());
    }
    let repetitions = match arguments.first() {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "benchmark 重复次数必须是 1 到 100 的整数")?,
        None => 3,
    };
    if arguments.len() > 1 || !(1..=100).contains(&repetitions) {
        return Err("benchmark 重复次数必须是 1 到 100 的整数".into());
    }

    let import_started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let import_elapsed = import_started.elapsed();
    let imported_entries = imported.stats.imported_entries;
    let build_started = Instant::now();
    let decoder = Decoder::new(imported.entries);
    let build_elapsed = build_started.elapsed();
    let report = run_decoder_benchmark(&decoder, repetitions)?;

    println!("固定公开词典 release benchmark：");
    println!("  导入词条：{imported_entries}");
    println!("  重复次数：{}", report.repetitions);
    println!(
        "  导入：{:.3} ms；建索引：{:.3} ms",
        import_elapsed.as_secs_f64() * 1000.0,
        build_elapsed.as_secs_f64() * 1000.0
    );
    print_latency(
        &format!("单词（{} 条/轮）", report.word_queries),
        report.word_latency,
    );
    print_latency(
        &format!("短句（{} 条/轮）", report.short_sentence_queries),
        report.short_sentence_latency,
    );
    print_sentence_work("短句合计工作量", report.short_sentence_work);
    print_latency(
        &format!("长句（{} 条/轮）", report.long_sentence_queries),
        report.long_sentence_latency,
    );
    print_sentence_work("长句合计工作量", report.long_sentence_work);
    println!("  结果校验和：{}", report.result_checksum);
    println!("这些是固定工作负载的本机重复采样，不代表其他设备或真实输入分布。");
    Ok(())
}

fn print_latency(label: &str, summary: LatencySummary) {
    println!(
        "  {label}：{} 样本；min {:.3} / median {:.3} / mean {:.3} / p95 {:.3} / max {:.3} ms",
        summary.samples,
        summary.minimum.as_secs_f64() * 1000.0,
        summary.median.as_secs_f64() * 1000.0,
        summary.mean.as_secs_f64() * 1000.0,
        summary.p95.as_secs_f64() * 1000.0,
        summary.maximum.as_secs_f64() * 1000.0
    );
}

fn print_sentence_work(label: &str, stats: ziranma_decoder::SentenceSearchStats) {
    println!(
        "  {label}：trie 扫描 {}；路径访问 {}（精确预扫 {} 路径/{} 词条）/子树剪枝 {}；对齐状态 {} 实查 + {} 复用；终点路径/展开词条 {} -> {}（跳过 {}）；lattice 边 {} -> {} -> {}；排名转移 {} -> {}；路径组合 {}",
        stats.segment_trie_scans,
        stats.trie_path_visits,
        stats.exact_prefix_prepass_visits,
        stats.exact_prefix_prepass_entry_visits,
        stats.trie_subtree_prunes,
        stats.alignment_states_examined,
        stats.alignment_states_reused,
        stats.terminal_path_matches,
        stats.terminal_spelling_matches,
        stats.terminal_entry_bound_skips,
        stats.lattice_transitions,
        stats.lattice_transitions_materialized,
        stats.lattice_transitions_retained,
        stats.ranking_transitions_considered,
        stats.ranking_transitions_retained,
        stats.path_combinations_considered
    );
}

fn run_decode(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("decode 需要一个按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("decode 参数过多".into());
    }
    decode_and_print(observed, top_k)
}

fn run_public_decode(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("public-decode 需要一个按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("public-decode 参数过多".into());
    }

    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let decoder = Decoder::new(imported.entries);
    let candidates = decoder.decode(observed, top_k)?;
    print_decoded_candidates(observed, &candidates);
    Ok(())
}

fn run_search_stats(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("search-stats 需要一个按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("search-stats 参数过多".into());
    }

    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    let decoder = Decoder::new(lexicon);
    let (candidates, stats) = decoder.decode_with_stats(observed, top_k)?;
    print_decoded_candidates(observed, &candidates);
    println!("联合搜索统计：");
    println!("  trie 路径状态访问：{}", stats.trie_path_visits);
    println!("  精确上界子树剪枝：{}", stats.trie_subtree_prunes);
    println!(
        "  按键对齐状态：{} 实查 + {} 精确复用",
        stats.alignment_states_examined, stats.alignment_states_reused
    );
    println!("  去重前终点拼写匹配：{}", stats.terminal_spelling_matches);
    Ok(())
}

fn run_sentence(arguments: &[String], use_bigram: bool) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("sentence 需要一个没有词界的按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("sentence 参数过多".into());
    }

    let decoder = demo_sentence_decoder(use_bigram)?;
    let candidates = decoder.decode_sentence(observed, top_k)?;
    print_sentence_candidates(observed, use_bigram, &candidates);
    Ok(())
}

fn run_public_sentence(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("public-sentence 需要一个没有词界的按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("public-sentence 参数过多".into());
    }

    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let decoder = Decoder::new(imported.entries);
    let candidates = decoder.decode_sentence(observed, top_k)?;
    print_sentence_candidates(observed, false, &candidates);
    Ok(())
}

fn run_sentence_stats(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("sentence-stats 需要一个没有词界的按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("sentence-stats 参数过多".into());
    }

    let decoder = demo_sentence_decoder(true)?;
    let (candidates, stats) = decoder.decode_sentence_with_stats(observed, top_k)?;
    print_sentence_candidates(observed, true, &candidates);
    println!("句子 lattice 统计：");
    println!("  活跃词界 trie 扫描：{}", stats.segment_trie_scans);
    println!("  trie 路径状态访问：{}", stats.trie_path_visits);
    println!(
        "  其中精确证据预扫：{} 路径 / {} 词条",
        stats.exact_prefix_prepass_visits, stats.exact_prefix_prepass_entry_visits
    );
    println!("  精确上界子树剪枝：{}", stats.trie_subtree_prunes);
    println!(
        "  按键对齐状态：{} 实查 + {} 精确复用",
        stats.alignment_states_examined, stats.alignment_states_reused
    );
    println!("  终点拼写路径：{}", stats.terminal_path_matches);
    println!("  去重前终点片段匹配：{}", stats.terminal_spelling_matches);
    println!("  终点词条上界跳过：{}", stats.terminal_entry_bound_skips);
    println!(
        "  lattice 边生成/物化/保留：{} -> {} -> {}",
        stats.lattice_transitions,
        stats.lattice_transitions_materialized,
        stats.lattice_transitions_retained
    );
    println!(
        "  其中逐键未解析边：{} -> {}",
        stats.unresolved_lattice_transitions, stats.unresolved_lattice_transitions_retained
    );
    println!("  求解的 k-best 状态：{}", stats.ranking_states_evaluated);
    println!("  状态缓存命中：{}", stats.ranking_state_cache_hits);
    println!(
        "  排名转移精确缩减：{} -> {}",
        stats.ranking_transitions_considered, stats.ranking_transitions_retained
    );
    println!("  路径组合检查：{}", stats.path_combinations_considered);
    Ok(())
}

fn demo_sentence_decoder(use_bigram: bool) -> Result<Decoder, Box<dyn Error>> {
    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    if use_bigram {
        let model = BigramLanguageModel::from_tsv(DEMO_BIGRAM_CORPUS, &lexicon)?;
        Ok(Decoder::new(lexicon).with_bigram_model(model))
    } else {
        Ok(Decoder::new(lexicon))
    }
}

fn print_sentence_candidates(
    observed: &str,
    use_bigram: bool,
    candidates: &[ziranma_decoder::SentenceCandidate],
) {
    println!(
        "整串输入：{observed}（{}）",
        if use_bigram {
            "unigram + bigram"
        } else {
            "仅 unigram"
        }
    );
    if candidates.is_empty() {
        println!("没有返回候选（Top-K 可能为 0）。");
        return;
    }

    for (rank, candidate) in candidates.iter().enumerate() {
        println!(
            "{}. {}  [句子分 {:.3}；未解析 {} 键]",
            rank + 1,
            candidate.text,
            candidate.total_score,
            candidate.unresolved_key_count
        );
        for (index, segment) in candidate.segments.iter().enumerate() {
            if segment.candidate.source == CandidateSource::UnresolvedInput {
                println!(
                    "   片段 {}：{} -> {} [原样保留；未解析代价 {:.3}；不消耗纠错预算]",
                    index + 1,
                    segment.observed,
                    segment.candidate.text,
                    segment.candidate.score.unresolved_input_penalty
                );
                continue;
            }
            println!(
                "   词 {}：{} -> {} [{}；{}；{}]",
                index + 1,
                segment.observed,
                segment.candidate.text,
                segment.candidate.spelling.code,
                segment.candidate.spelling.description(),
                segment.candidate.correction.description()
            );
            if let Some(bigram) = segment.language_score.bigram {
                println!(
                    "      语言分 {:.3} = unigram {:.3} 与 bigram {:.3} 插值；共现 {}/{}，α={:.1}",
                    segment.language_score.interpolated_log_probability,
                    segment.language_score.unigram_log_probability,
                    bigram.log_probability,
                    bigram.observed_count,
                    bigram.predecessor_total,
                    bigram.alpha
                );
            } else {
                println!(
                    "      语言分 {:.3}（首词或未启用 bigram，使用 unigram）",
                    segment.language_score.interpolated_log_probability
                );
            }
        }
    }
}

fn run_decode_legacy(observed: &str, remaining: &[String]) -> Result<(), Box<dyn Error>> {
    let top_k = parse_top_k(remaining.first())?;
    if remaining.len() > 1 {
        return Err("参数过多；请运行 --help 查看用法".into());
    }
    decode_and_print(observed, top_k)
}

fn parse_top_k(value: Option<&String>) -> Result<usize, Box<dyn Error>> {
    match value {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "Top-K 必须是非负整数".into()),
        None => Ok(10),
    }
}

fn decode_and_print(observed: &str, top_k: usize) -> Result<(), Box<dyn Error>> {
    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    let decoder = Decoder::new(lexicon);
    let candidates = decoder.decode(observed, top_k)?;

    print_decoded_candidates(observed, &candidates);
    Ok(())
}

fn print_decoded_candidates(observed: &str, candidates: &[Candidate]) {
    println!("输入按键：{observed}");
    if candidates.is_empty() {
        println!("演示词典中没有符合当前规则的候选。");
        return;
    }

    for (index, candidate) in candidates.iter().enumerate() {
        print_candidate(index + 1, candidate);
    }
}

fn print_candidate(rank: usize, candidate: &Candidate) {
    println!(
        "{rank}. {}  [{} / 完整码 {}]",
        candidate.text, candidate.pinyin, candidate.code
    );
    println!(
        "   解释码 {}；{}；{}",
        candidate.spelling.code,
        candidate.spelling.description(),
        candidate.correction.description()
    );
    println!(
        "   总分 {:.3} = 词频分 {:.3} - 简拼代价 {:.3} - 纠错代价 {:.3}",
        candidate.score.total,
        candidate.score.frequency,
        candidate.score.abbreviation_penalty,
        candidate.score.correction_penalty
    );
}

fn print_usage() {
    println!(
        "\
ziranma-decoder：自然码可解释容错解码实验

用法：
  cargo run -- encode <无声调拼音...>
  cargo run -- decode <按键串> [Top-K]
  cargo run -- sentence <无词界按键串> [Top-K]
  cargo run -- sentence-unigram <按键串> [Top-K]
  cargo run -- public-decode <按键串> [Top-K]
  cargo run -- public-sentence <按键串> [Top-K]
  cargo run -- sentence-stats <按键串> [Top-K]
  cargo run -- index-stats
  cargo run -- public-index-stats
  cargo run -- abbreviation-audit
  cargo run --release -- benchmark [重复次数]
  cargo run -- search-stats <按键串> [Top-K]
  cargo run -- evaluate
  cargo run --release -- public-calibrate

兼容简写：
  cargo run -- <按键串> [Top-K]

示例：
  cargo run -- encode ni hao
  cargo run -- decode nihk
  cargo run -- decode nhk
  cargo run -- decode nik
  cargo run -- sentence zrmurf
  cargo run -- public-sentence zrmurf
  cargo run -- sentence-stats zrmurf
  cargo run -- index-stats
  cargo run -- public-index-stats
  cargo run -- abbreviation-audit
  cargo run --release -- benchmark 3
  cargo run -- search-stats nhk
  cargo run -- evaluate
  cargo run --release -- public-calibrate

程序只读取仓库内的公开演示词典，不会保存输入。"
    );
}
