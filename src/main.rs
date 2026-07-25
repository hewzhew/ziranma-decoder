use std::env;
use std::error::Error;
use std::process::ExitCode;

use ziranma_decoder::{Decoder, parse_lexicon_tsv};

const DEMO_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

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
    let mut arguments = env::args().skip(1);
    let Some(input) = arguments.next() else {
        print_usage();
        return Ok(());
    };

    if matches!(input.as_str(), "-h" | "--help") {
        print_usage();
        return Ok(());
    }

    let top_k = match arguments.next() {
        Some(value) => value.parse::<usize>().map_err(|_| "Top-K 必须是非负整数")?,
        None => 10,
    };

    if arguments.next().is_some() {
        return Err("参数过多；请运行 --help 查看用法".into());
    }

    let lexicon = parse_lexicon_tsv(DEMO_LEXICON)?;
    let decoder = Decoder::new(lexicon);
    let candidates = decoder.decode(&input, top_k)?;

    println!("输入按键：{input}");
    if candidates.is_empty() {
        println!("演示词典中没有符合第一阶段规则的候选。");
        return Ok(());
    }

    for (index, candidate) in candidates.iter().enumerate() {
        println!(
            "{}. {}  [{} / {}]",
            index + 1,
            candidate.text,
            candidate.pinyin,
            candidate.code
        );
        println!(
            "   {}；总分 {:.3} = 词频分 {:.3} - 纠错代价 {:.3}",
            candidate.correction.description(),
            candidate.score.total,
            candidate.score.frequency,
            candidate.score.correction_penalty
        );
    }

    Ok(())
}

fn print_usage() {
    println!(
        "\
ziranma-decoder：第一阶段自然码容错解码实验

用法：
  cargo run -- <按键串> [Top-K]

示例：
  cargo run -- nihk
  cargo run -- nigk
  cargo run -- nikh 5

程序只读取仓库内的公开演示词典，不会保存输入。"
    );
}
