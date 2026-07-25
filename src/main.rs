use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::Instant;

use ziranma_decoder::{
    BigramLanguageModel, Candidate, CandidateSource, Decoder, encode_pinyin_phrase,
    evaluate_oov_cases, evaluate_sentence_cases, evaluate_synthetic, parse_lexicon_tsv,
    parse_rime_lexicon,
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
        "本次评测耗时：{:.3} ms（仅供本机观察，不是稳定基准）",
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
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
        "  {label}：trie 扫描 {}；对齐状态 {} 实查 + {} 复用；lattice 边 {} -> {} -> {}；排名转移 {} -> {}；路径组合 {}",
        stats.segment_trie_scans,
        stats.alignment_states_examined,
        stats.alignment_states_reused,
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
        "  按键对齐状态：{} 实查 + {} 精确复用",
        stats.alignment_states_examined, stats.alignment_states_reused
    );
    println!("  去重前终点片段匹配：{}", stats.terminal_spelling_matches);
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
  cargo run --release -- benchmark [重复次数]
  cargo run -- search-stats <按键串> [Top-K]
  cargo run -- evaluate

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
  cargo run --release -- benchmark 3
  cargo run -- search-stats nhk
  cargo run -- evaluate

程序只读取仓库内的公开演示词典，不会保存输入。"
    );
}
