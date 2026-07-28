use std::env;
use std::error::Error;
use std::io::{self, IsTerminal, Write as _};
use std::process::ExitCode;
use std::time::Instant;

use ziranma_decoder::{
    BigramLanguageModel, Candidate, CandidateSource, CharacterBigramLanguageModel,
    CharacterShapeIndex, Decoder, MAX_SHAPE_LAB_VISIBLE, ProtocolContextLaneReport,
    ProtocolIndexStats, ProtocolStrategyReport, SentenceCandidate, ShapeLab, analyze_candidate_lab,
    audit_abbreviation_codebook, audit_anchored_tail_failures, audit_continuous_composition,
    audit_public_protocol_context, audit_public_protocols, audit_shape_refinement_course,
    encode_pinyin_phrase, evaluate_character_context_oracle, evaluate_context_oracle,
    evaluate_continuous_composition, evaluate_labeled_recall, evaluate_labeled_rejection_shadow,
    evaluate_oov_cases, evaluate_rejection_shadow, evaluate_sentence_cases, evaluate_synthetic,
    parse_lexicon_tsv, parse_rime_lexicon, parse_stroke_sequence_tsv, parse_ud_conllu,
    select_public_bigram_training_sequences, select_public_calibration_cases,
    select_public_continuous_composition_cases, select_public_protocol_audit_cases,
    select_shape_course_tasks,
};

mod benchmark;
mod candidate_lab_cli;
mod shape_course_cli;
mod shape_lab_cli;
mod typing_lab_cli;
#[cfg(windows)]
mod windows_shape_keys;
#[cfg(windows)]
mod windows_typing_keys;

use benchmark::{LatencySummary, run_decoder_benchmark};
use candidate_lab_cli::{CANDIDATE_LAB_USAGE, parse_candidate_lab_arguments, render_candidate_lab};
use shape_course_cli::{
    SHAPE_COURSE_USAGE, ShapeCourseAttempt, ShapeCourseAttemptEffect, ShapeCourseProgress,
    parse_shape_course_arguments, parse_shape_course_input, render_shape_course_screen,
    render_shape_course_summary,
};
use shape_lab_cli::{
    SHAPE_LAB_USAGE, ShapeLabInput, ShapeLabSession, ShapeLabSessionEffect,
    normalize_shape_lab_input, parse_shape_lab_arguments, parse_shape_lab_input,
    render_shape_lab_details, render_shape_lab_screen,
};
use typing_lab_cli::{
    TYPING_LAB_CANDIDATE_POOL_DEPTH, TYPING_LAB_USAGE, TypingLabEffect, TypingLabInput,
    TypingLabSelectionMemory, TypingLabSession, find_single_character_pinyin,
    parse_typing_lab_arguments, parse_typing_lab_input, render_typing_lab_screen,
};
#[cfg(windows)]
use windows_shape_keys::WindowsShapeKeyReader;
#[cfg(windows)]
use windows_typing_keys::WindowsTypingKeyReader;

const DEMO_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
const DEMO_BIGRAM_CORPUS: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");
const DEMO_SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/demo_sentence_cases.tsv");
const LONG_SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/long_sentence_cases.tsv");
const OOV_CASES: &str = include_str!("../tests/fixtures/public/oov_lexicon.tsv");
const PUBLIC_RIME_LEXICON: &str =
    include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
const PUBLIC_STROKE_DATA: &str =
    include_str!("../data/public/conway-stroke-data/sequence-characters.txt");
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
        "public-compose" => run_public_compose(&arguments[1..]),
        "candidate-lab" => run_candidate_lab(&arguments[1..]),
        "typing-lab" => run_typing_lab(&arguments[1..]),
        "shape-lab" => run_shape_lab(&arguments[1..]),
        "shape-course" => run_shape_course(&arguments[1..]),
        "public-compose-evaluate" => run_public_compose_evaluate(&arguments[1..]),
        "public-compose-audit" => run_public_compose_audit(&arguments[1..]),
        "public-protocol-audit" => run_public_protocol_audit(&arguments[1..]),
        "public-protocol-failure-audit" => run_public_protocol_failure_audit(&arguments[1..]),
        "public-protocol-context-audit" => run_public_protocol_context_audit(&arguments[1..]),
        "public-shape-audit" => run_public_shape_audit(&arguments[1..]),
        "sentence-stats" => run_sentence_stats(&arguments[1..]),
        // Preserve the first milestone's convenient `cargo run -- nihk` form.
        observed => run_decode_legacy(observed, &arguments[1..]),
    }
}

fn run_public_shape_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-shape-audit 不接受额外参数".into());
    }

    let started = Instant::now();
    let lexicon = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let strokes = parse_stroke_sequence_tsv(PUBLIC_STROKE_DATA)?;
    let shapes = CharacterShapeIndex::new(strokes.into_shapes())?;
    let report = audit_shape_refinement_course(&lexicon.entries, &shapes);

    println!("公开 Tab 笔画课程评测：固定 Rime + Conway 快照；不读取私人记录");
    println!(
        "单字：源词条 {}，去重候选 {}；完整双拼码池 {}，其中歧义池 {}（候选出现 {}）",
        report.single_character_entries,
        report.distinct_single_character_candidates,
        report.phonetic_pools,
        report.ambiguous_pools,
        report.candidates_in_ambiguous_pools
    );
    println!(
        "首屏外课程：超过 10 个候选的码池 {}，最大池 {}；池内候选出现 {}，有笔画数据 {}（{:.2}%）",
        report.hard_pools,
        report.maximum_pool_size,
        report.candidates_in_hard_pools,
        report.hard_pool_candidates_with_stroke_data,
        percentage(
            report.hard_pool_candidates_with_stroke_data,
            report.candidates_in_hard_pools
        )
    );
    println!(
        "困难目标：原排名 11 以后共 {}；有笔画 {}，缺笔画 {}，含替代笔顺 {}",
        report.hard_targets,
        report.hard_targets_with_stroke_data,
        report.hard_targets_without_stroke_data,
        report.hard_targets_with_alternative_sequences
    );
    println!(
        "笔画  额外模式动作  笔顺试次留存   试次进入Top-10  目标任一/全部笔顺可见  平均池缩小"
    );
    for stats in &report.prefixes {
        let average_before = mean(stats.candidates_before_sum, stats.sequence_attempts);
        let average_after = mean(stats.candidates_after_sum, stats.sequence_attempts);
        let reduction = if stats.candidates_before_sum == 0 {
            0.0
        } else {
            (1.0 - stats.candidates_after_sum as f64 / stats.candidates_before_sum as f64) * 100.0
        };
        println!(
            "{:>2}        Tab+{:>1}       {:>5}/{:<5} {:>6.2}%   {:>5}/{:<5} {:>6.2}%   {:>5}/{:<5} / {:>5}/{:<5}   {:>6.2}→{:<6.2} ({:>6.2}%)",
            stats.prefix_keys,
            stats.prefix_keys,
            stats.target_retained_attempts,
            stats.sequence_attempts,
            percentage(stats.target_retained_attempts, stats.sequence_attempts),
            stats.target_visible_attempts,
            stats.sequence_attempts,
            percentage(stats.target_visible_attempts, stats.sequence_attempts),
            stats.targets_visible_with_any_sequence,
            report.hard_targets_with_stroke_data,
            stats.targets_visible_with_all_sequences,
            report.hard_targets_with_stroke_data,
            average_before,
            average_after,
            reduction
        );
        println!(
            "          唯一定位：笔顺试次 {}/{}（{:.2}%）；目标任一/全部笔顺 {}/{} / {}/{}",
            stats.target_isolated_attempts,
            stats.sequence_attempts,
            percentage(stats.target_isolated_attempts, stats.sequence_attempts),
            stats.targets_isolated_with_any_sequence,
            report.hard_targets_with_stroke_data,
            stats.targets_isolated_with_all_sequences,
            report.hard_targets_with_stroke_data
        );
    }
    println!(
        "口径：同码池按上游权重稳定排序；形码只过滤、不重排。任一/全部分别是替代笔顺的乐观/稳健边界。"
    );
    println!(
        "限制：这不是现实输入频率，也未估算翻页、视觉寻找和最终选择；动作栏只列新增的 Tab 与笔画键。"
    );
    println!(
        "本次公开课程耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn percentage(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64 * 100.0
    }
}

fn mean(total: usize, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64
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

fn run_public_compose(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(observed) = arguments.first() else {
        return Err("public-compose 需要一个连续、没有词界的按键串".into());
    };
    let top_k = parse_top_k(arguments.get(1))?;
    if arguments.len() > 2 {
        return Err("public-compose 参数过多".into());
    }

    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let decoder = Decoder::new(imported.entries);
    let lanes = decoder.decode_sentence_lanes(observed, top_k)?;
    println!("连续组合输入：{observed}");
    println!("主候选（保留稳定的零错误优先顺序）：");
    print_sentence_candidate_list(&lanes.primary);
    println!("完整首音节 + 尾部简写的一次顺序颠倒恢复：");
    print_sentence_candidate_list(&lanes.anchored_transposition_recovery);
    Ok(())
}

fn run_candidate_lab(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "-h" | "--help") {
        println!("{CANDIDATE_LAB_USAGE}");
        return Ok(());
    }

    let options = parse_candidate_lab_arguments(arguments)?;
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let decoder = Decoder::new(imported.entries);
    let report = analyze_candidate_lab(&decoder, &options.observed, options.top_k)?;
    print!("{}", render_candidate_lab(&report, &options));
    Ok(())
}

fn run_typing_lab(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "-h" | "--help") {
        println!("{TYPING_LAB_USAGE}");
        return Ok(());
    }

    let options = parse_typing_lab_arguments(arguments)?;
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let strokes = parse_stroke_sequence_tsv(PUBLIC_STROKE_DATA)?;
    let shapes = CharacterShapeIndex::new(strokes.into_shapes())?;
    let decoder = Decoder::new(imported.entries.clone());
    let shape_lab = ShapeLab::new(&imported.entries, &shapes);
    let mut input_source = TypingLabInputSource::open();
    let mut session = TypingLabSession::default();
    let mut selection_memory = TypingLabSelectionMemory::default();
    let mut phonetic_cache = TypingLabPhoneticCache::default();
    let mut screen = AlternateScreen::enter(input_source.direct_keys())?;

    loop {
        let (candidates, ordinary_candidates) = if let Some(pinyin) = session.shape_pinyin() {
            let candidates = shape_lab
                .snapshot(pinyin, session.stroke_prefix(), None, MAX_SHAPE_LAB_VISIBLE)?
                .candidates
                .into_iter()
                .map(|candidate| candidate.character.to_string())
                .collect::<Vec<_>>();
            (candidates, false)
        } else if session.phonetic().is_empty() {
            (Vec::new(), false)
        } else {
            let depth = if session.candidate_page_start() == 0 {
                options.visible_limit
            } else {
                TYPING_LAB_CANDIDATE_POOL_DEPTH
            };
            if phonetic_cache.code != session.phonetic() || phonetic_cache.depth != depth {
                phonetic_cache.code = session.phonetic().to_owned();
                phonetic_cache.depth = depth;
                phonetic_cache.candidates = decoder.decode_sentence(session.phonetic(), depth)?;
            }
            selection_memory.promote(session.phonetic(), &mut phonetic_cache.candidates);
            let candidates = phonetic_cache
                .candidates
                .iter()
                .map(|candidate| candidate.text.clone())
                .collect::<Vec<_>>();
            (candidates, true)
        };
        session.normalize_candidate_page(candidates.len(), options.visible_limit);
        let visible_range =
            session.visible_candidate_range(candidates.len(), options.visible_limit);
        let visible_candidates = &candidates[visible_range];

        screen.clear();
        print!(
            "{}",
            render_typing_lab_screen(
                &session,
                visible_candidates,
                candidates.len(),
                input_source.direct_keys(),
            )
        );
        if !input_source.direct_keys() {
            print!("> ");
        }
        io::stdout().flush()?;

        let Some(input) = input_source.read()? else {
            break;
        };
        match session.apply(input) {
            TypingLabEffect::Continue => {}
            TypingLabEffect::Confirm => select_typing_candidate(
                &mut session,
                &mut selection_memory,
                &candidates,
                ordinary_candidates.then_some(phonetic_cache.candidates.as_slice()),
                1,
            ),
            TypingLabEffect::Select(rank) => select_typing_candidate(
                &mut session,
                &mut selection_memory,
                &candidates,
                ordinary_candidates.then_some(phonetic_cache.candidates.as_slice()),
                rank,
            ),
            TypingLabEffect::PreviousPage => session.previous_candidate_page(options.visible_limit),
            TypingLabEffect::NextPage => {
                session.next_candidate_page(candidates.len(), options.visible_limit)
            }
            TypingLabEffect::RequestTab => {
                if let Some(pinyin) =
                    find_single_character_pinyin(&imported.entries, session.phonetic())
                {
                    session.enter_tab(pinyin);
                } else {
                    session.set_notice("Tab 只用于完整单字码");
                }
            }
            TypingLabEffect::Quit => break,
        }
    }

    screen.leave()?;
    if !session.committed().is_empty() {
        println!("{}", session.committed());
    }
    Ok(())
}

fn select_typing_candidate(
    session: &mut TypingLabSession,
    selection_memory: &mut TypingLabSelectionMemory,
    candidates: &[String],
    candidate_details: Option<&[SentenceCandidate]>,
    rank: usize,
) {
    let candidate_index = rank
        .checked_sub(1)
        .and_then(|index| session.candidate_page_start().checked_add(index));
    if let Some(candidate) = candidate_index.and_then(|index| candidates.get(index)) {
        if let Some(detail) = candidate_index.and_then(|index| candidate_details?.get(index)) {
            selection_memory.remember(session.phonetic(), detail);
        }
        session.commit(candidate);
    } else {
        session.set_notice("这个位置没有候选");
    }
}

fn run_shape_lab(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "-h" | "--help") {
        println!("{SHAPE_LAB_USAGE}");
        return Ok(());
    }

    let options = parse_shape_lab_arguments(arguments)?;
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let strokes = parse_stroke_sequence_tsv(PUBLIC_STROKE_DATA)?;
    let shapes = CharacterShapeIndex::new(strokes.into_shapes())?;
    let lab = ShapeLab::new(&imported.entries, &shapes);

    if options.details {
        println!("Tab 笔画详细审计（固定公开快照；不学习；不写文件）");
        let ordinary = lab.snapshot(
            &options.pinyin,
            "",
            options.expected_character,
            options.visible_limit,
        )?;
        print!("{}", render_shape_lab_details(&ordinary));
        if let Some(prefix) = options.prefix.as_deref() {
            println!();
            let filtered = lab.snapshot(
                &options.pinyin,
                prefix,
                options.expected_character,
                options.visible_limit,
            )?;
            print!("{}", render_shape_lab_details(&filtered));
        }
        return Ok(());
    }

    if let Some(prefix) = options.prefix.as_deref() {
        let snapshot = lab.snapshot(
            &options.pinyin,
            prefix,
            options.expected_character,
            options.visible_limit,
        )?;
        print!(
            "{}",
            render_shape_lab_screen(&snapshot, true, false, None, false)
        );
        return Ok(());
    }

    let mut input_source = ShapeLabInputSource::open();
    let mut session = ShapeLabSession::default();
    loop {
        let snapshot = lab.snapshot(
            &options.pinyin,
            session.active_prefix(),
            options.expected_character,
            options.visible_limit,
        )?;
        clear_interactive_screen();
        print!(
            "{}",
            render_shape_lab_screen(
                &snapshot,
                session.tab_mode(),
                true,
                session.notice(),
                input_source.direct_keys(),
            )
        );
        if !input_source.direct_keys() {
            print!("> ");
        }
        io::stdout().flush()?;

        let Some(input) = input_source.read()? else {
            break;
        };
        match session.apply(input) {
            ShapeLabSessionEffect::Continue => {}
            ShapeLabSessionEffect::Select(rank) => {
                if let Some(character) = snapshot
                    .candidates
                    .iter()
                    .find(|candidate| candidate.filtered_rank == rank)
                    .map(|candidate| candidate.character)
                {
                    clear_interactive_screen();
                    println!("{character}");
                    break;
                }
                session.set_notice("这个位置没有候选");
            }
            ShapeLabSessionEffect::Skip => {}
            ShapeLabSessionEffect::Quit => break,
        }
    }
    Ok(())
}

fn run_shape_course(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.len() == 1 && matches!(arguments[0].as_str(), "-h" | "--help") {
        println!("{SHAPE_COURSE_USAGE}");
        return Ok(());
    }

    let options = parse_shape_course_arguments(arguments)?;
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let strokes = parse_stroke_sequence_tsv(PUBLIC_STROKE_DATA)?;
    let shapes = CharacterShapeIndex::new(strokes.into_shapes())?;
    let tasks = select_shape_course_tasks(
        &imported.entries,
        &shapes,
        options.difficulty,
        options.count,
    );
    if tasks.is_empty() {
        return Err("固定公开快照中没有符合这个课程级别的题目".into());
    }

    let lab = ShapeLab::new(&imported.entries, &shapes);
    let phonetic_codes = tasks
        .iter()
        .map(|task| {
            encode_pinyin_phrase(&task.pinyin).map(|encoded| encoded.full_code.as_str().to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut input_source = ShapeLabInputSource::open();
    let mut progress = ShapeCourseProgress::default();
    let mut task_index = 0usize;
    let mut attempt = ShapeCourseAttempt::new(phonetic_codes[task_index].clone());
    loop {
        let task = &tasks[task_index];
        let snapshot = lab.snapshot(
            &task.pinyin,
            attempt.active_prefix(),
            Some(task.character),
            10,
        )?;
        clear_interactive_screen();
        print!(
            "{}",
            render_shape_course_screen(
                &snapshot,
                task_index + 1,
                tasks.len(),
                &attempt,
                input_source.direct_keys(),
            )
        );
        if !input_source.direct_keys() {
            print!("> ");
        }
        io::stdout().flush()?;

        let Some(input) = input_source.read_course()? else {
            break;
        };
        progress.observe_input(&input, &attempt);
        let mut advance = false;
        match attempt.apply(input) {
            ShapeCourseAttemptEffect::Continue => {}
            ShapeCourseAttemptEffect::Select(rank) => {
                match snapshot
                    .candidates
                    .iter()
                    .find(|candidate| candidate.filtered_rank == rank)
                {
                    Some(candidate) if candidate.character == task.character => {
                        progress.correct += 1;
                        advance = true;
                    }
                    Some(_) => {
                        progress.wrong_selections += 1;
                        attempt.set_notice("请选目标字");
                    }
                    None => attempt.set_notice("这个位置没有候选"),
                }
            }
            ShapeCourseAttemptEffect::Skip => {
                progress.skipped += 1;
                advance = true;
            }
            ShapeCourseAttemptEffect::Quit => break,
        }

        if advance {
            task_index += 1;
            if task_index == tasks.len() {
                break;
            }
            attempt = ShapeCourseAttempt::new(phonetic_codes[task_index].clone());
        }
    }

    clear_interactive_screen();
    print!("{}", render_shape_course_summary(&progress, tasks.len()));
    Ok(())
}

enum ShapeLabInputSource {
    #[cfg(windows)]
    Direct(WindowsShapeKeyReader),
    Line,
}

impl ShapeLabInputSource {
    fn open() -> Self {
        #[cfg(windows)]
        if io::stdin().is_terminal()
            && io::stdout().is_terminal()
            && let Ok(reader) = WindowsShapeKeyReader::open()
        {
            return Self::Direct(reader);
        }
        Self::Line
    }

    fn direct_keys(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::Direct(_))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn read(&mut self) -> io::Result<Option<ShapeLabInput>> {
        match self {
            #[cfg(windows)]
            Self::Direct(reader) => reader.read().map(normalize_shape_lab_input).map(Some),
            Self::Line => {
                let mut input = String::new();
                if io::stdin().read_line(&mut input)? == 0 {
                    Ok(None)
                } else {
                    Ok(Some(parse_shape_lab_input(&input)))
                }
            }
        }
    }

    fn read_course(&mut self) -> io::Result<Option<ShapeLabInput>> {
        match self {
            #[cfg(windows)]
            Self::Direct(reader) => reader.read().map(Some),
            Self::Line => {
                let mut input = String::new();
                if io::stdin().read_line(&mut input)? == 0 {
                    Ok(None)
                } else {
                    Ok(Some(parse_shape_course_input(&input)))
                }
            }
        }
    }
}

enum TypingLabInputSource {
    #[cfg(windows)]
    Direct(WindowsTypingKeyReader),
    Line,
}

struct AlternateScreen {
    active: bool,
}

#[derive(Default)]
struct TypingLabPhoneticCache {
    code: String,
    depth: usize,
    candidates: Vec<SentenceCandidate>,
}

impl AlternateScreen {
    fn enter(active: bool) -> io::Result<Self> {
        if active {
            print!("\x1b[?1049h\x1b[?25l\x1b[H\x1b[J");
            io::stdout().flush()?;
        }
        Ok(Self { active })
    }

    fn clear(&mut self) {
        if self.active {
            print!("\x1b[H\x1b[J");
        }
    }

    fn leave(&mut self) -> io::Result<()> {
        if self.active {
            print!("\x1b[?25h\x1b[?1049l");
            io::stdout().flush()?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for AlternateScreen {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

impl TypingLabInputSource {
    fn open() -> Self {
        #[cfg(windows)]
        if io::stdin().is_terminal()
            && io::stdout().is_terminal()
            && let Ok(reader) = WindowsTypingKeyReader::open()
        {
            return Self::Direct(reader);
        }
        Self::Line
    }

    fn direct_keys(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::Direct(_))
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn read(&mut self) -> io::Result<Option<TypingLabInput>> {
        match self {
            #[cfg(windows)]
            Self::Direct(reader) => reader.read().map(Some),
            Self::Line => {
                let mut input = String::new();
                if io::stdin().read_line(&mut input)? == 0 {
                    Ok(None)
                } else {
                    Ok(Some(parse_typing_lab_input(&input)))
                }
            }
        }
    }
}

fn clear_interactive_screen() {
    if io::stdout().is_terminal() {
        print!("\x1b[2J\x1b[H");
    }
}

fn run_public_compose_evaluate(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-compose-evaluate 不接受额外参数".into());
    }

    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let corpus = parse_ud_conllu(PUBLIC_UD_TEST)?;
    let selection = select_public_continuous_composition_cases(&corpus, &imported.entries, 64);
    let decoder = Decoder::new(imported.entries);
    let report = evaluate_continuous_composition(&decoder, &selection.probes);
    let stats = selection.stats;

    println!("公开连续组合短语评测：UD test 相邻两词、2～6 个汉字、跨句均匀取 64 条");
    println!(
        "筛选：{} 个双词窗口；{} 个长度合格纯汉字；{} 个整词可覆盖；{} 个可省键；{} 个可构造颠倒；{} 个独立句代表；最终 {} 条",
        stats.source_windows,
        stats.han_length_eligible,
        stats.exact_word_coverable,
        stats.key_saving_eligible,
        stats.transposition_eligible,
        stats.sentence_representatives,
        stats.selected
    );
    println!(
        "按键：完整 {}，尾部简写 {}，节省 {}（{:.1}%）",
        report.full_keys,
        report.tail_keys,
        report.saved_keys(),
        report.key_saving_rate() * 100.0
    );
    println!("轨道                       Top-1       Top-3       Top-5      Top-10");
    print_composition_recall("完整码主榜", report.full_code);
    print_composition_recall("尾部简写主榜", report.tail_abbreviation);
    print_composition_recall("尾部简写同字数", report.tail_abbreviation_same_length);
    print_composition_recall("颠倒输入主榜", report.transposed_primary);
    print_composition_recall("颠倒恢复栏", report.transposed_recovery);
    print_composition_visibility("尾部简写主榜", report.tail_abbreviation);
    print_composition_visibility("颠倒恢复栏", report.transposed_recovery);
    let visible_selection_actions =
        report.tail_abbreviation.hits_at_10 - report.tail_abbreviation.hits_at_1;
    println!(
        "乐观操作账：首屏内非首选若各需 1 次选择，尾部简写为省 {} 键、增加 {} 次选择；另有 {} 条仍在 Top-10 外，不能计作净节省",
        report.saved_keys(),
        visible_selection_actions,
        report.tail_abbreviation.total - report.tail_abbreviation.hits_at_10
    );
    println!(
        "本次连续组合评测耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_public_compose_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-compose-audit 不接受额外参数".into());
    }

    const BASELINE_K: usize = 10;
    const AUDIT_DEPTH: usize = 100;
    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let train_corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
    let test_corpus = parse_ud_conllu(PUBLIC_UD_TEST)?;
    let training = select_public_bigram_training_sequences(&train_corpus, &imported.entries);
    let word_language_model =
        BigramLanguageModel::from_token_sequences(&training.sequences, &imported.entries)?;
    let character_training_texts = training
        .sequences
        .iter()
        .map(|sequence| sequence.concat())
        .collect::<Vec<_>>();
    let character_language_model =
        CharacterBigramLanguageModel::from_text_sequences(&character_training_texts)?;
    let selection = select_public_continuous_composition_cases(&test_corpus, &imported.entries, 64);
    let lexicon = imported.entries;
    let decoder = Decoder::new(lexicon.clone());
    let report = audit_continuous_composition(
        &decoder,
        &word_language_model,
        &character_language_model,
        &lexicon,
        &selection.probes,
        BASELINE_K,
        AUDIT_DEPTH,
    );
    let failures = report.total - report.baseline_visible;

    println!("公开连续组合失败审计：固定 64 条尾部简写，主榜 Top-{BASELINE_K}");
    println!(
        "基线：{} 条首屏可见；{} 条首屏外，其中 {} 条在第 {}～{}、{} 条仍在 Top-{} 外",
        report.baseline_visible,
        failures,
        report.deeper_visible,
        BASELINE_K + 1,
        AUDIT_DEPTH,
        report.outside_audit_depth,
        AUDIT_DEPTH
    );
    println!(
        "首屏失败的错误 Top-1 字数：比答案短 {}，等长 {}，比答案长 {}",
        report.baseline_top_shorter, report.baseline_top_same_length, report.baseline_top_longer
    );
    println!(
        "冻结同一 Top-{AUDIT_DEPTH} 池后，train-only 词级上下文：{} 条升至 Top-1，{} 条回到 Top-{BASELINE_K}",
        report.word_context_reranked_at_1, report.word_context_reranked_visible
    );
    println!(
        "纯字符平均上下文：{} 条升至 Top-1，{} 条回到 Top-{BASELINE_K}（只作诊断，不是生产评分）",
        report.character_average_reranked_at_1, report.character_average_reranked_visible
    );
    println!("逐例账目（基线排名 / 词上下文排名 / 字符平均排名）：");
    for case in &report.failures {
        println!(
            "  {}  {} -> {}；{} / {} / {}；原首选 {} [{}]",
            case.id,
            case.observed,
            case.expected_text,
            format_audit_rank(case.baseline_rank, AUDIT_DEPTH),
            format_audit_rank(case.word_context_rank, AUDIT_DEPTH),
            format_audit_rank(case.character_average_rank, AUDIT_DEPTH),
            case.baseline_top_text,
            case.baseline_top_segments.join(" | ")
        );
    }
    println!(
        "本次失败审计耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_public_protocol_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-protocol-audit 不接受额外参数".into());
    }

    const DEV_LIMIT: usize = 128;
    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
    let selection = select_public_protocol_audit_cases(&corpus, &imported.entries, DEV_LIMIT);
    let report = audit_public_protocols(&imported.entries, &selection.probes);
    let long_word_report = audit_public_protocols(&imported.entries, &selection.long_word_probes);
    let stats = selection.stats;

    println!("公开受限简写协议审计：UD train 固定按源句 4:1 切为 fit/dev");
    println!(
        "切分：fit {} 句 / dev {} 句；fit {} 个窗口中 {} 个合格；dev {} 个窗口中 {} 个合格",
        stats.fit_source_sentences,
        stats.dev_source_sentences,
        stats.fit_source_windows,
        stats.fit_eligible_windows,
        stats.dev_source_windows,
        stats.dev_eligible_windows
    );
    println!(
        "dev：{} 个独立句代表，等距保留 {} 条；完整码共 {} 个字母",
        stats.dev_sentence_representatives, stats.selected, stats.selected_full_keys
    );
    println!(
        "fit 快捷短语：{} 个短语 / {} 个码；{} 个码互撞；重复至少两次且不互撞的白名单 {} 条",
        stats.fit_distinct_phrases,
        stats.fit_shortcut_codes,
        stats.fit_colliding_shortcut_codes,
        stats.fit_repeated_collision_free_shortcuts
    );
    println!("固定语法索引（每个词在每种协议中只有一种拼写）：");
    println!("协议                 不同码     冲突码   单码最多文本   最长码");
    print_protocol_index("完整双拼", report.full_code_index);
    print_protocol_index("每词省一键", report.conservative_tail_index);
    print_protocol_index("锚定尾简", report.anchored_tail_index);
    print_protocol_index("显式全简", report.explicit_abbreviation_index);
    println!("held-out dev 排名与操作账（Top-10 外成本未知，不冒充净节省）：");
    println!(
        "协议                 字母  模式动作  字母差  动作后差    Top-1    Top-5   Top-10  可见非首选  可见省字母"
    );
    print_protocol_strategy("完整双拼", report.full_code, report.full_code.input_letters);
    print_protocol_strategy(
        "每词省一键",
        report.conservative_tail,
        report.full_code.input_letters,
    );
    print_protocol_strategy(
        "锚定尾简",
        report.anchored_tail,
        report.full_code.input_letters,
    );
    print_protocol_strategy(
        "显式全简",
        report.explicit_abbreviation,
        report.full_code.input_letters,
    );
    println!(
        "长词压力层：dev 中含至少一个 3+ 音节词的独立代表 {} 条，固定保留 {} 条",
        stats.dev_long_word_representatives, stats.selected_long_word
    );
    println!(
        "协议                 字母  模式动作  字母差  动作后差    Top-1    Top-5   Top-10  可见非首选  可见省字母"
    );
    print_protocol_strategy(
        "完整双拼",
        long_word_report.full_code,
        long_word_report.full_code.input_letters,
    );
    print_protocol_strategy(
        "每词省一键",
        long_word_report.conservative_tail,
        long_word_report.full_code.input_letters,
    );
    print_protocol_strategy(
        "锚定尾简",
        long_word_report.anchored_tail,
        long_word_report.full_code.input_letters,
    );
    println!(
        "快捷白名单：dev 覆盖 {}/{}，其余 {} 条回退完整码；省 {} 字母，需 {} 次显式快捷栏选择，净省 {} 次物理动作",
        report.whitelist.covered,
        report.whitelist.attempts,
        report.whitelist.full_code_fallbacks,
        report.whitelist.saved_letters,
        report.whitelist.lane_selection_actions,
        report.whitelist.net_actions_saved()
    );
    println!(
        "边界：显式全简的 1 次模式动作已经计入；候选确认是各协议共有成本，未重复计入。白名单只保证快捷栏内部不互撞，不宣称能静默上屏。"
    );
    println!(
        "本次协议审计耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_public_protocol_failure_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let details = match arguments {
        [] => false,
        [argument] if argument == "--details" => true,
        _ => {
            return Err("public-protocol-failure-audit 只接受可选的 --details".into());
        }
    };

    const DEV_LIMIT: usize = 128;
    const VISIBLE_K: usize = 10;
    const AUDIT_DEPTH: usize = 100;
    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
    let selection = select_public_protocol_audit_cases(&corpus, &imported.entries, DEV_LIMIT);
    let report =
        audit_anchored_tail_failures(&imported.entries, &selection.probes, VISIBLE_K, AUDIT_DEPTH);
    let failures = report.total - report.baseline_visible;

    println!("公开锚定尾简失败审计：同一固定 held-out dev，严格固定拼写语法");
    println!(
        "基线：{}/{} 在 Top-{}；失败 {} 条",
        report.baseline_visible, report.total, report.visible_k, failures
    );
    println!(
        "无词界加深：{} 条在第 {}～{}；{} 条仍在 Top-{} 外",
        report.deeper_visible,
        report.visible_k + 1,
        report.audit_depth,
        report.outside_audit_depth,
        report.audit_depth
    );
    println!(
        "只补 1 个真实词界：{} 条救回 Top-{}，其中 {} 条升到第一；另有 {} 条仅在第 {}～{}，{} 条仍在 Top-{} 外",
        report.boundary_recovered_visible,
        report.visible_k,
        report.boundary_recovered_at_1,
        report.boundary_deeper_visible,
        report.visible_k + 1,
        report.audit_depth,
        report.boundary_outside_audit_depth,
        report.audit_depth
    );
    println!(
        "失败首选字数：比答案短 {}，等长 {}，比答案长 {}",
        report.baseline_top_shorter, report.baseline_top_same_length, report.baseline_top_longer
    );
    println!(
        "词内码冲突：{} / {} 条失败至少有一个预期词码对应多个文本；最大单词码扇出 {}",
        report.failures_with_word_code_collision,
        failures,
        report.maximum_expected_word_code_fanout
    );
    println!(
        "若只给被救回的失败补一个边界标记，它们相对完整码合计净省 {} 次物理动作；未救回样本不计收益",
        report.recovered_net_actions_saved
    );
    if details {
        println!("逐例公开账目：深层排名 / 补词界排名 / 预期两词码扇出；原首选 [分词]");
        for case in &report.failures {
            println!(
                "  {}  {} -> {}；{} / {} / {}；{} [{}]",
                case.id,
                case.observed,
                case.expected_text,
                format_audit_rank(case.deeper_rank, report.audit_depth),
                format_audit_rank(case.boundary_rank, report.audit_depth),
                case.expected_word_code_fanouts
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                case.baseline_top_text,
                case.baseline_top_segments.join(" | ")
            );
        }
    } else {
        println!("逐例内容默认不展开；需要复核固定公开样本时追加 --details（不读取私人数据）。");
    }
    println!(
        "本次失败审计耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn run_public_protocol_context_audit(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !arguments.is_empty() {
        return Err("public-protocol-context-audit 不接受额外参数".into());
    }

    const DEV_LIMIT: usize = 128;
    const POOL_DEPTH: usize = 100;
    let started = Instant::now();
    let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
    let corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
    let selection = select_public_protocol_audit_cases(&corpus, &imported.entries, DEV_LIMIT);
    let language_model = BigramLanguageModel::from_token_sequences(
        &selection.fit_context_sequences,
        &imported.entries,
    )?;
    let model_stats = language_model.stats();
    let report = audit_public_protocol_context(
        &imported.entries,
        &selection.probes,
        &language_model,
        POOL_DEPTH,
    );

    println!("公开受限协议上下文审计：fit-only 词 bigram 重排冻结候选池");
    println!(
        "隔离：fit {} 句映射为 {} 条上下文序列、{} 个词；held-out dev {} 条从不参与训练",
        selection.stats.fit_source_sentences,
        selection.stats.fit_context_sequences,
        selection.stats.fit_context_words,
        selection.stats.selected
    );
    println!(
        "模型：{} 个词对类型、{} 个词对实例；每条输入先冻结 unigram Top-{}，上下文不能创造路径",
        model_stats.observed_pair_types, model_stats.observed_pair_instances, report.pool_depth
    );
    println!(
        "协议                 池内   基线 Top1/5/10      上下文 Top1/5/10    救回/掉出 Top10    排名升/平/降"
    );
    print_protocol_context_lane("完整双拼", report.full_code);
    print_protocol_context_lane("锚定尾简", report.anchored_tail);
    println!(
        "判定规则：只有锚定尾简净救回且完整双拼不退化，才值得进入下一轮；否则该 word-bigram 分支停止。"
    );
    println!(
        "本次上下文审计耗时：{:.3} ms（固定本机观察）",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn print_protocol_context_lane(label: &str, report: ProtocolContextLaneReport) {
    println!(
        "{label:<18} {:>3}/{:<3} {:>3}/{:<3}/{:<3}       {:>3}/{:<3}/{:<3}          {:>3}/{:<3}          {:>3}/{:<3}/{:<3}",
        report.pool_visible,
        report.total,
        report.baseline_hits_at_1,
        report.baseline_hits_at_5,
        report.baseline_hits_at_10,
        report.context_hits_at_1,
        report.context_hits_at_5,
        report.context_hits_at_10,
        report.repaired_into_top_10,
        report.dropped_out_of_top_10,
        report.improved_ranks,
        report.unchanged_ranks,
        report.worsened_ranks
    );
}

fn print_protocol_index(label: &str, stats: ProtocolIndexStats) {
    println!(
        "{label:<18} {:>7} {:>10} {:>14} {:>10}",
        stats.distinct_codes,
        stats.colliding_codes,
        stats.maximum_texts_per_code,
        stats.maximum_code_keys
    );
}

fn print_protocol_strategy(label: &str, report: ProtocolStrategyReport, baseline_letters: usize) {
    let letter_difference = baseline_letters as isize - report.input_letters as isize;
    let action_difference = letter_difference - report.activation_actions as isize;
    println!(
        "{label:<18} {:>6} {:>9} {:>8} {:>9} {:>5}/{:<3} {:>5}/{:<3} {:>5}/{:<3} {:>11} {:>12}",
        report.input_letters,
        report.activation_actions,
        letter_difference,
        action_difference,
        report.hits_at_1,
        report.attempts,
        report.hits_at_5,
        report.attempts,
        report.hits_at_10,
        report.attempts,
        report.visible_nonfirst,
        report.visible_letter_savings
    );
}

fn format_audit_rank(rank: Option<usize>, audit_depth: usize) -> String {
    rank.map_or_else(|| format!(">{audit_depth}"), |rank| rank.to_string())
}

fn print_composition_recall(label: &str, report: ziranma_decoder::CompositionRecallReport) {
    println!(
        "{label:<18} {:>3}/{:<3} {:>6.1}%  {:>3}/{:<3} {:>6.1}%  {:>3}/{:<3} {:>6.1}%  {:>3}/{:<3} {:>6.1}%",
        report.hits_at_1,
        report.total,
        report.recall_at_1() * 100.0,
        report.hits_at_3,
        report.total,
        report.recall_at_3() * 100.0,
        report.hits_at_5,
        report.total,
        report.recall_at_5() * 100.0,
        report.hits_at_10,
        report.total,
        report.recall_at_10() * 100.0,
    );
}

fn print_composition_visibility(label: &str, report: ziranma_decoder::CompositionRecallReport) {
    println!(
        "{label}可见性：{} 条直接首选；{} 条在第 2～10；{} 条在 Top-10 外",
        report.hits_at_1,
        report.hits_at_10 - report.hits_at_1,
        report.total - report.hits_at_10
    );
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

    print_sentence_candidate_list(candidates);
}

fn print_sentence_candidate_list(candidates: &[ziranma_decoder::SentenceCandidate]) {
    if candidates.is_empty() {
        println!("  （没有候选）");
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
        print_sentence_candidate_segments(candidate);
    }
}

fn print_sentence_candidate_segments(candidate: &ziranma_decoder::SentenceCandidate) {
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
  cargo run -- public-compose <连续按键串> [每栏 Top-K]
  cargo run -- candidate-lab <连续按键串> [每栏显示数，1～10] [--expect <文字>] [--recovery] [--verbose|--json]
  cargo run --release -- typing-lab [--limit <1～10>]
  cargo run --release -- shape-lab <公开单字拼音> [--expect <单字>] [--prefix <hspnz...>] [--limit <1～10>] [--details]
  cargo run --release -- shape-course [--count <1～50>] [--level <easy|medium|hard|mixed>]
  cargo run --release -- public-compose-evaluate
  cargo run --release -- public-compose-audit
  cargo run --release -- public-protocol-audit
  cargo run --release -- public-protocol-failure-audit [--details]
  cargo run --release -- public-protocol-context-audit
  cargo run --release -- public-shape-audit
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
  cargo run -- public-compose mafkmm 3
  cargo run -- candidate-lab mafmkm 3
  cargo run -- candidate-lab mafmkm 3 --expect 麻烦猫猫
  cargo run -- candidate-lab mafkmm 3 --recovery
  cargo run --release -- typing-lab
  cargo run --release -- shape-lab shi --expect 事
  cargo run --release -- shape-lab da --expect 龘 --prefix n
  cargo run --release -- shape-lab da --expect 龘 --prefix n --details
  cargo run --release -- shape-course --count 10 --level mixed
  cargo run --release -- public-compose-evaluate
  cargo run --release -- public-compose-audit
  cargo run --release -- public-protocol-audit
  cargo run --release -- public-protocol-failure-audit --details
  cargo run --release -- public-protocol-context-audit
  cargo run --release -- public-shape-audit
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
