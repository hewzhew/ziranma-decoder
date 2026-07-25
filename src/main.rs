use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::Instant;

use ziranma_decoder::{
    BigramLanguageModel, Candidate, Decoder, encode_pinyin_phrase, evaluate_sentence_cases,
    evaluate_synthetic, parse_lexicon_tsv,
};

const DEMO_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
const DEMO_BIGRAM_CORPUS: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");
const DEMO_SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/demo_sentence_cases.tsv");

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
        "search-stats" => run_search_stats(&arguments[1..]),
        "decode" => run_decode(&arguments[1..]),
        "sentence" => run_sentence(&arguments[1..], true),
        "sentence-unigram" => run_sentence(&arguments[1..], false),
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
    println!("  按键对齐状态检查：{}", stats.alignment_states_examined);
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
    println!("  按键对齐状态检查：{}", stats.alignment_states_examined);
    println!("  去重前终点片段匹配：{}", stats.terminal_spelling_matches);
    println!("  去重后 lattice 词边：{}", stats.lattice_transitions);
    println!("  求解的 k-best 状态：{}", stats.ranking_states_evaluated);
    println!("  状态缓存命中：{}", stats.ranking_state_cache_hits);
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
        println!("演示词典中没有能够覆盖完整按键串的分词路径。");
        return;
    }

    for (rank, candidate) in candidates.iter().enumerate() {
        println!(
            "{}. {}  [句子分 {:.3}]",
            rank + 1,
            candidate.text,
            candidate.total_score
        );
        for (index, segment) in candidate.segments.iter().enumerate() {
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
  cargo run -- sentence-stats <按键串> [Top-K]
  cargo run -- index-stats
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
  cargo run -- sentence-stats zrmurf
  cargo run -- index-stats
  cargo run -- search-stats nhk
  cargo run -- evaluate

程序只读取仓库内的公开演示词典，不会保存输入。"
    );
}
