use std::error::Error;
use std::fmt::Write as _;

use ziranma_core::FrozenContextRerankReport;

pub const CANDIDATE_CONTEXT_LAB_USAGE: &str = "\
公开上下文排序实验

用法：
  cargo run --release -- candidate-context-lab <连续全码> [选项]

选项：
  --expect <文字>   对照目标在冻结候选池中的重排前后名次
  --pool <10～200>  冻结的 unigram 候选数，默认 20
  --limit <1～20>   显示重排后的前几项，默认 6

实验只读取仓库内固定的 Rime 与 UD 公开快照，不读取私人记录，
不写文件，也不会修改正在使用的输入法。";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateContextLabOptions {
    pub observed: String,
    pub expected_text: Option<String>,
    pub pool_depth: usize,
    pub visible_limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateContextLabRuntime {
    pub training_sequences: usize,
    pub training_words: usize,
    pub observed_pair_types: usize,
    pub observed_pair_instances: u128,
    pub preparation_millis: f64,
    pub ranking_millis: f64,
}

pub fn parse_candidate_context_lab_arguments(
    arguments: &[String],
) -> Result<CandidateContextLabOptions, Box<dyn Error>> {
    let mut observed = None;
    let mut expected_text = None;
    let mut pool_depth = 20;
    let mut visible_limit = 6;
    let mut saw_pool = false;
    let mut saw_limit = false;

    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--expect" => {
                if expected_text.is_some() {
                    return Err("candidate-context-lab 的 --expect 只能使用一次".into());
                }
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or("candidate-context-lab 的 --expect 后面需要目标文字")?;
                if value.starts_with('-') || value.trim().is_empty() {
                    return Err("candidate-context-lab 的 --expect 后面需要非空目标文字".into());
                }
                expected_text = Some(value.clone());
            }
            "--pool" => {
                if saw_pool {
                    return Err("candidate-context-lab 的 --pool 只能使用一次".into());
                }
                saw_pool = true;
                index += 1;
                pool_depth = parse_bounded_number(
                    arguments.get(index),
                    "--pool 后面需要 10～200 的整数",
                    10,
                    200,
                )?;
            }
            "--limit" => {
                if saw_limit {
                    return Err("candidate-context-lab 的 --limit 只能使用一次".into());
                }
                saw_limit = true;
                index += 1;
                visible_limit = parse_bounded_number(
                    arguments.get(index),
                    "--limit 后面需要 1～20 的整数",
                    1,
                    20,
                )?;
            }
            value if value.starts_with('-') => {
                return Err(format!("candidate-context-lab 不认识选项 {value:?}").into());
            }
            value if observed.is_none() => observed = Some(value.to_owned()),
            _ => {
                return Err(
                    "candidate-context-lab 参数过多；请运行 candidate-context-lab --help".into(),
                );
            }
        }
        index += 1;
    }

    let observed = observed.ok_or("candidate-context-lab 需要一个连续全码")?;
    if observed.is_empty()
        || !observed
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_lowercase())
    {
        return Err("连续全码必须是非空的小写英文字母".into());
    }
    if visible_limit > pool_depth {
        return Err("--limit 不能大于 --pool".into());
    }

    Ok(CandidateContextLabOptions {
        observed,
        expected_text,
        pool_depth,
        visible_limit,
    })
}

fn parse_bounded_number(
    value: Option<&String>,
    message: &'static str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, Box<dyn Error>> {
    let value = value.ok_or(message)?;
    let parsed = value.parse::<usize>().map_err(|_| message)?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(message.into());
    }
    Ok(parsed)
}

pub fn render_candidate_context_lab(
    report: &FrozenContextRerankReport,
    options: &CandidateContextLabOptions,
    runtime: CandidateContextLabRuntime,
) -> String {
    let mut output = String::new();
    writeln!(output, "公开上下文排序实验").expect("writing to String cannot fail");
    writeln!(output, "输入：{}", options.observed).expect("writing to String cannot fail");
    writeln!(
        output,
        "冻结当前 unigram 前 {} 项；其中 {} 项是完整双拼且零纠错，可相互重排。",
        report.pool_depth, report.eligible_candidates
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "公开模型：{} 条训练序列、{} 个词、{} 种相邻词对（共 {} 次）。",
        runtime.training_sequences,
        runtime.training_words,
        runtime.observed_pair_types,
        runtime.observed_pair_instances
    )
    .expect("writing to String cannot fail");
    writeln!(output).expect("writing to String cannot fail");
    writeln!(output, "基线 → 重排  候选").expect("writing to String cannot fail");
    for row in report.candidates.iter().take(options.visible_limit) {
        let segments = row
            .candidate
            .segments
            .iter()
            .map(|segment| segment.candidate.text.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        writeln!(
            output,
            "{:>3} → {:<3}  {}  [{}]",
            row.baseline_rank, row.context_rank, row.candidate.text, segments
        )
        .expect("writing to String cannot fail");
        if row.eligible {
            let evidence = row
                .pair_evidence
                .iter()
                .map(|pair| {
                    format!(
                        "{}→{} {}/{}",
                        pair.previous,
                        pair.current,
                        pair.score.observed_count,
                        pair.score.predecessor_total
                    )
                })
                .collect::<Vec<_>>();
            if !evidence.is_empty() {
                writeln!(output, "           公开词对：{}", evidence.join("；"))
                    .expect("writing to String cannot fail");
            }
        } else {
            writeln!(output, "           保持原位：含简拼、纠错或未解析输入")
                .expect("writing to String cannot fail");
        }
    }

    if let Some(expected_text) = options.expected_text.as_deref() {
        writeln!(output).expect("writing to String cannot fail");
        match report
            .candidates
            .iter()
            .find(|row| row.candidate.text == expected_text)
        {
            Some(row) => writeln!(
                output,
                "目标：{expected_text}，第 {} 名 → 第 {} 名。",
                row.baseline_rank, row.context_rank
            )
            .expect("writing to String cannot fail"),
            None => writeln!(
                output,
                "目标：{expected_text}，不在冻结候选池中；上下文不会补造候选。"
            )
            .expect("writing to String cannot fail"),
        }
    }

    writeln!(output).expect("writing to String cannot fail");
    writeln!(
        output,
        "本机本次：准备公开模型 {:.1} ms；解码与重排 {:.1} ms。",
        runtime.preparation_millis, runtime.ranking_millis
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "只读实验；不读取私人记录，不写文件，也不改变输入法。"
    )
    .expect("writing to String cannot fail");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ziranma_core::{
        BigramLanguageModel, Decoder, parse_lexicon_tsv, rerank_frozen_sentence_pool,
    };

    #[test]
    fn parses_explicit_target_and_bounds() {
        let options = parse_candidate_context_lab_arguments(&[
            "ybqiui".to_owned(),
            "--expect".to_owned(),
            "尤其是".to_owned(),
            "--pool".to_owned(),
            "80".to_owned(),
            "--limit".to_owned(),
            "5".to_owned(),
        ])
        .unwrap();
        assert_eq!(options.observed, "ybqiui");
        assert_eq!(options.expected_text.as_deref(), Some("尤其是"));
        assert_eq!(options.pool_depth, 80);
        assert_eq!(options.visible_limit, 5);
    }

    #[test]
    fn renders_rank_movement_and_public_pair_counts() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n有\tyou\t100\n其实\tqi shi\t100\n尤其\tyou qi\t80\n是\tshi\t80\n",
        )
        .unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let model =
            BigramLanguageModel::from_tsv("tokens\tcount\n尤其 是\t20\n", &lexicon).unwrap();
        let pool = decoder.decode_sentence("ybqiui", 10).unwrap();
        let report = rerank_frozen_sentence_pool(&pool, &model);
        let output = render_candidate_context_lab(
            &report,
            &CandidateContextLabOptions {
                observed: "ybqiui".to_owned(),
                expected_text: Some("尤其是".to_owned()),
                pool_depth: 10,
                visible_limit: 2,
            },
            CandidateContextLabRuntime {
                training_sequences: 1,
                training_words: 2,
                observed_pair_types: 1,
                observed_pair_instances: 20,
                preparation_millis: 1.0,
                ranking_millis: 2.0,
            },
        );

        assert!(output.contains("2 → 1    尤其是"));
        assert!(output.contains("尤其→是 20/20"));
        assert!(output.contains("目标：尤其是，第 2 名 → 第 1 名。"));
    }
}
