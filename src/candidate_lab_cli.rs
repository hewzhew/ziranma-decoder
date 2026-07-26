use std::error::Error;
use std::fmt::Write as _;

use ziranma_decoder::{
    CandidateLabCandidate, CandidateLabLane, CandidateLabReport, CandidateSource, Correction,
};

pub const CANDIDATE_LAB_USAGE: &str = "\
候选实验台

用法：
  cargo run --release -- candidate-lab <连续按键串> [每栏显示数，1～10] [选项]

选项：
  --recovery  显示可能的相邻按键颠倒候选
  --verbose   显示算法评分、纠错预算和语言模型细节
  --json      输出使用稳定英文字段名的机器可读 JSON

`--verbose` 与 `--json` 不能同时使用。";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateLabOutputMode {
    Concise,
    Verbose,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateLabCliOptions {
    pub observed: String,
    pub top_k: usize,
    pub show_recovery: bool,
    pub output_mode: CandidateLabOutputMode,
}

pub fn parse_candidate_lab_arguments(
    arguments: &[String],
) -> Result<CandidateLabCliOptions, Box<dyn Error>> {
    let mut observed = None;
    let mut top_k = None;
    let mut show_recovery = false;
    let mut output_mode = CandidateLabOutputMode::Concise;

    for argument in arguments {
        match argument.as_str() {
            "--recovery" => show_recovery = true,
            "--verbose" => {
                if output_mode == CandidateLabOutputMode::Json {
                    return Err("candidate-lab 的 --verbose 与 --json 不能同时使用".into());
                }
                output_mode = CandidateLabOutputMode::Verbose;
            }
            "--json" => {
                if output_mode == CandidateLabOutputMode::Verbose {
                    return Err("candidate-lab 的 --verbose 与 --json 不能同时使用".into());
                }
                output_mode = CandidateLabOutputMode::Json;
            }
            value if value.starts_with('-') => {
                return Err(format!("candidate-lab 不认识选项 {value:?}").into());
            }
            value if observed.is_none() => observed = Some(value.to_owned()),
            value if top_k.is_none() => {
                top_k = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "每栏显示数必须是 1～10 的整数")?,
                );
            }
            _ => return Err("candidate-lab 参数过多；请运行 candidate-lab --help".into()),
        }
    }

    let observed = observed.ok_or("candidate-lab 需要一个连续、没有词界的小写字母按键串")?;

    Ok(CandidateLabCliOptions {
        observed,
        top_k: top_k.unwrap_or(10),
        show_recovery,
        output_mode,
    })
}

pub fn render_candidate_lab(
    report: &CandidateLabReport,
    options: &CandidateLabCliOptions,
) -> String {
    match options.output_mode {
        CandidateLabOutputMode::Concise => {
            render_candidate_lab_concise(report, options.show_recovery)
        }
        CandidateLabOutputMode::Verbose => {
            render_candidate_lab_verbose(report, options.show_recovery)
        }
        CandidateLabOutputMode::Json => render_candidate_lab_json(report, options.show_recovery),
    }
}

fn render_candidate_lab_concise(report: &CandidateLabReport, show_recovery: bool) -> String {
    let mut output = String::new();
    writeln!(output, "候选实验台").expect("writing to String cannot fail");
    writeln!(
        output,
        "输入：{}（{} 个字母）；每栏显示 {} 项",
        report.observed,
        report.observed.as_str().len(),
        report.top_k
    )
    .expect("writing to String cannot fail");
    writeln!(output, "固定公开词典；只读；不会学习本次输入。")
        .expect("writing to String cannot fail");
    writeln!(output).expect("writing to String cannot fail");
    render_concise_lane(&mut output, "普通候选", &report.primary);

    if show_recovery {
        writeln!(output).expect("writing to String cannot fail");
        render_concise_lane(
            &mut output,
            "可能的按键颠倒（查看这一栏计 1 次额外操作）",
            &report.anchored_transposition_recovery,
        );
    }

    writeln!(output).expect("writing to String cannot fail");
    writeln!(output, "这是实验排序，不代表最终输入法效果。")
        .expect("writing to String cannot fail");
    if !show_recovery {
        writeln!(output, "如需查看可能的按键颠倒候选，请加 --recovery。")
            .expect("writing to String cannot fail");
    }
    writeln!(output, "如需算法评分与完整解释，请加 --verbose。")
        .expect("writing to String cannot fail");
    output
}

fn render_concise_lane(output: &mut String, label: &str, rows: &[CandidateLabCandidate]) {
    writeln!(output, "{label}").expect("writing to String cannot fail");
    if rows.is_empty() {
        writeln!(output, "  （没有候选）").expect("writing to String cannot fail");
        return;
    }

    for row in rows {
        writeln!(output, "{}. {}", row.rank, row.candidate.text)
            .expect("writing to String cannot fail");
        writeln!(output, "   {}", concise_action_summary(row))
            .expect("writing to String cannot fail");
        for segment in &row.candidate.segments {
            if segment.candidate.source == CandidateSource::UnresolvedInput {
                writeln!(
                    output,
                    "   {} → {}（尚未解析）",
                    segment.observed, segment.candidate.text
                )
                .expect("writing to String cannot fail");
                continue;
            }

            let mut details = Vec::new();
            if segment.candidate.spelling.abbreviated_syllables.is_empty() {
                details.push("完整双拼".to_owned());
            } else {
                let positions = segment
                    .candidate
                    .spelling
                    .abbreviated_syllables
                    .iter()
                    .map(|index| (index + 1).to_string())
                    .collect::<Vec<_>>()
                    .join("、");
                details.push(format!("第 {positions} 个音节使用简拼"));
            }
            if !matches!(segment.candidate.correction, Correction::Exact) {
                details.push(segment.candidate.correction.description());
            }
            writeln!(
                output,
                "   {} → {}（{}）",
                segment.observed,
                segment.candidate.text,
                details.join("；")
            )
            .expect("writing to String cannot fail");
        }
    }
}

fn concise_action_summary(row: &CandidateLabCandidate) -> String {
    match row.net_actions_saved_vs_full {
        Some(saved) if saved > 0 => format!(
            "预计 {} 次操作，比完整输入少 {saved} 次",
            row.projected_actions_one_selection
        ),
        Some(0) => format!(
            "预计 {} 次操作，与完整输入相同",
            row.projected_actions_one_selection
        ),
        Some(saved) => format!(
            "预计 {} 次操作，比完整输入多 {} 次",
            row.projected_actions_one_selection,
            saved.unsigned_abs()
        ),
        None => format!(
            "预计 {} 次操作；含尚未解析的输入，暂不比较",
            row.projected_actions_one_selection
        ),
    }
}

fn render_candidate_lab_verbose(report: &CandidateLabReport, show_recovery: bool) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "候选实验台（详细模式；固定公开词典；只读；不学习输入）"
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "输入：{}；字母键 {}；每栏显示 {} 个候选",
        report.observed,
        report.observed.as_str().len(),
        report.top_k
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "计数规则：每个候选计一次选择；按键颠倒栏另计一次显式切换；不估算翻页、视觉查找或纠错后的重打。"
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "协议提醒：普通候选沿用研究解码器，仍可能出现自由混合简拼，不代表最终默认输入协议。"
    )
    .expect("writing to String cannot fail");
    render_verbose_lane(&mut output, "普通候选（研究排序）", &report.primary);

    if show_recovery {
        render_verbose_lane(
            &mut output,
            "按键颠倒恢复候选",
            &report.anchored_transposition_recovery,
        );
    } else {
        writeln!(output, "按键颠倒恢复候选默认隐藏；需要时请加 --recovery。")
            .expect("writing to String cannot fail");
    }
    output
}

fn render_verbose_lane(output: &mut String, label: &str, rows: &[CandidateLabCandidate]) {
    writeln!(output, "{label}：").expect("writing to String cannot fail");
    if rows.is_empty() {
        writeln!(output, "  （没有候选）").expect("writing to String cannot fail");
        return;
    }

    for row in rows {
        let candidate = &row.candidate;
        writeln!(
            output,
            "{}. {}  [候选评分 {:.3}；未解析 {} 键]",
            row.rank, candidate.text, candidate.total_score, candidate.unresolved_key_count
        )
        .expect("writing to String cannot fail");
        match (
            row.canonical_full_letter_keys,
            row.net_actions_saved_vs_full,
        ) {
            (Some(full_letters), Some(net_saved)) => {
                let baseline_actions = full_letters.saturating_add(row.selection_actions);
                let comparison = if net_saved >= 0 {
                    format!("少 {net_saved} 次")
                } else {
                    format!("多 {} 次", net_saved.unsigned_abs())
                };
                writeln!(
                    output,
                    "   预计操作：{} 字母 + {} 选择 + {} 切栏 = {}；完整输入基线 {}；{comparison}",
                    row.observed_letter_keys,
                    row.selection_actions,
                    row.lane_activation_actions,
                    row.projected_actions_one_selection,
                    baseline_actions
                )
                .expect("writing to String cannot fail");
            }
            _ => writeln!(
                output,
                "   预计操作：{} 字母 + {} 选择 + {} 切栏 = {}；含未解析输入，不虚构完整输入基线",
                row.observed_letter_keys,
                row.selection_actions,
                row.lane_activation_actions,
                row.projected_actions_one_selection
            )
            .expect("writing to String cannot fail"),
        }
        writeln!(
            output,
            "   结构：{} 个词或片段；{} 个简拼音节；{} 个纠错片段；全局纠错预算{}使用",
            candidate.segments.len(),
            row.abbreviated_syllables,
            row.corrected_segments,
            if candidate.used_error { "已" } else { "未" }
        )
        .expect("writing to String cannot fail");
        render_verbose_segments(output, candidate);
    }
}

fn render_verbose_segments(output: &mut String, candidate: &ziranma_decoder::SentenceCandidate) {
    for (index, segment) in candidate.segments.iter().enumerate() {
        if segment.candidate.source == CandidateSource::UnresolvedInput {
            writeln!(
                output,
                "   片段 {}：{} → {} [原样保留；未解析代价 {:.3}；不消耗纠错预算]",
                index + 1,
                segment.observed,
                segment.candidate.text,
                segment.candidate.score.unresolved_input_penalty
            )
            .expect("writing to String cannot fail");
            continue;
        }
        writeln!(
            output,
            "   词 {}：{} → {} [{}；{}；{}]",
            index + 1,
            segment.observed,
            segment.candidate.text,
            segment.candidate.spelling.code,
            segment.candidate.spelling.description(),
            segment.candidate.correction.description()
        )
        .expect("writing to String cannot fail");
        if let Some(bigram) = segment.language_score.bigram {
            writeln!(
                output,
                "      语言评分 {:.3} = 独立词频（unigram）{:.3} 与词间搭配（bigram）{:.3} 插值；共现 {}/{}，α={:.1}",
                segment.language_score.interpolated_log_probability,
                segment.language_score.unigram_log_probability,
                bigram.log_probability,
                bigram.observed_count,
                bigram.predecessor_total,
                bigram.alpha
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "      语言评分 {:.3}（使用独立词频 unigram；当前没有词间搭配 bigram）",
                segment.language_score.interpolated_log_probability
            )
            .expect("writing to String cannot fail");
        }
    }
}

fn render_candidate_lab_json(report: &CandidateLabReport, show_recovery: bool) -> String {
    let mut output = String::new();
    output.push_str("{\"schema\":\"ziranma-candidate-lab-v1\",\"input\":");
    push_json_string(&mut output, report.observed.as_str());
    write!(
        output,
        ",\"input_letter_keys\":{},\"top_k\":{},\"recovery_included\":{},\"lanes\":{{\"primary\":",
        report.observed.as_str().len(),
        report.top_k,
        show_recovery
    )
    .expect("writing to String cannot fail");
    push_json_lane(&mut output, &report.primary);
    output.push_str(",\"anchored_transposition_recovery\":");
    if show_recovery {
        push_json_lane(&mut output, &report.anchored_transposition_recovery);
    } else {
        output.push_str("[]");
    }
    output.push_str("}}\n");
    output
}

fn push_json_lane(output: &mut String, rows: &[CandidateLabCandidate]) {
    output.push('[');
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        push_json_candidate(output, row);
    }
    output.push(']');
}

fn push_json_candidate(output: &mut String, row: &CandidateLabCandidate) {
    output.push_str("{\"lane\":");
    push_json_string(
        output,
        match row.lane {
            CandidateLabLane::Primary => "primary",
            CandidateLabLane::AnchoredTranspositionRecovery => "anchored_transposition_recovery",
        },
    );
    write!(output, ",\"rank\":{},\"text\":", row.rank).expect("writing to String cannot fail");
    push_json_string(output, &row.candidate.text);
    write!(
        output,
        ",\"total_score\":{:.6},\"unresolved_key_count\":{},\"observed_letter_keys\":{},\"canonical_full_letter_keys\":",
        row.candidate.total_score,
        row.candidate.unresolved_key_count,
        row.observed_letter_keys
    )
    .expect("writing to String cannot fail");
    push_json_optional_usize(output, row.canonical_full_letter_keys);
    write!(
        output,
        ",\"selection_actions\":{},\"lane_activation_actions\":{},\"projected_actions\":{},\"net_actions_saved_vs_full\":",
        row.selection_actions, row.lane_activation_actions, row.projected_actions_one_selection
    )
    .expect("writing to String cannot fail");
    push_json_optional_isize(output, row.net_actions_saved_vs_full);
    write!(
        output,
        ",\"abbreviated_syllables\":{},\"corrected_segments\":{},\"used_error\":{},\"segments\":[",
        row.abbreviated_syllables, row.corrected_segments, row.candidate.used_error
    )
    .expect("writing to String cannot fail");
    for (index, segment) in row.candidate.segments.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"observed\":");
        push_json_string(output, segment.observed.as_str());
        output.push_str(",\"text\":");
        push_json_string(output, &segment.candidate.text);
        output.push_str(",\"source\":");
        push_json_string(
            output,
            match segment.candidate.source {
                CandidateSource::Lexicon => "lexicon",
                CandidateSource::UnresolvedInput => "unresolved_input",
            },
        );
        output.push_str(",\"pinyin\":");
        push_json_string(output, &segment.candidate.pinyin);
        output.push_str(",\"canonical_code\":");
        push_json_string(output, segment.candidate.code.as_str());
        output.push_str(",\"matched_code\":");
        push_json_string(output, segment.candidate.spelling.code.as_str());
        output.push_str(",\"abbreviated_syllable_indexes_zero_based\":[");
        for (position, syllable_index) in segment
            .candidate
            .spelling
            .abbreviated_syllables
            .iter()
            .enumerate()
        {
            if position > 0 {
                output.push(',');
            }
            write!(output, "{syllable_index}").expect("writing to String cannot fail");
        }
        output.push_str("],\"correction\":");
        push_json_correction(output, &segment.candidate.correction);
        output.push('}');
    }
    output.push_str("]}");
}

fn push_json_correction(output: &mut String, correction: &Correction) {
    match correction {
        Correction::Exact => output.push_str("{\"kind\":\"exact\"}"),
        Correction::NeighborSubstitution {
            index,
            intended,
            actual,
        } => {
            write!(
                output,
                "{{\"kind\":\"neighbor_substitution\",\"index\":{index},\"intended\":"
            )
            .expect("writing to String cannot fail");
            push_json_string(output, &intended.to_string());
            output.push_str(",\"actual\":");
            push_json_string(output, &actual.to_string());
            output.push('}');
        }
        Correction::AdjacentTransposition {
            start,
            intended_left,
            intended_right,
        } => {
            write!(
                output,
                "{{\"kind\":\"adjacent_transposition\",\"start\":{start},\"intended_left\":"
            )
            .expect("writing to String cannot fail");
            push_json_string(output, &intended_left.to_string());
            output.push_str(",\"intended_right\":");
            push_json_string(output, &intended_right.to_string());
            output.push('}');
        }
        Correction::MissingKey { index, intended } => {
            write!(
                output,
                "{{\"kind\":\"missing_key\",\"index\":{index},\"intended\":"
            )
            .expect("writing to String cannot fail");
            push_json_string(output, &intended.to_string());
            output.push('}');
        }
        Correction::ExtraKey { index, actual } => {
            write!(
                output,
                "{{\"kind\":\"extra_key\",\"index\":{index},\"actual\":"
            )
            .expect("writing to String cannot fail");
            push_json_string(output, &actual.to_string());
            output.push('}');
        }
    }
}

fn push_json_optional_usize(output: &mut String, value: Option<usize>) {
    match value {
        Some(value) => write!(output, "{value}").expect("writing to String cannot fail"),
        None => output.push_str("null"),
    }
}

fn push_json_optional_isize(output: &mut String, value: Option<isize>) {
    match value {
        Some(value) => write!(output, "{value}").expect("writing to String cannot fail"),
        None => output.push_str("null"),
    }
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use ziranma_decoder::{Decoder, analyze_candidate_lab, parse_lexicon_tsv};

    const LEXICON: &str = "\
text\tpinyin\tfrequency
麻烦\tma fan\t200
猫猫\tmao mao\t100
毛毛\tmao mao\t80
面孔\tmian kong\t60
";

    fn report(observed: &str, top_k: usize) -> CandidateLabReport {
        let decoder = Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        analyze_candidate_lab(&decoder, observed, top_k).unwrap()
    }

    fn options(
        observed: &str,
        top_k: usize,
        show_recovery: bool,
        output_mode: CandidateLabOutputMode,
    ) -> CandidateLabCliOptions {
        CandidateLabCliOptions {
            observed: observed.to_owned(),
            top_k,
            show_recovery,
            output_mode,
        }
    }

    #[test]
    fn argument_parser_accepts_flags_in_any_position() {
        let parsed = parse_candidate_lab_arguments(&[
            "--recovery".to_owned(),
            "mafmkm".to_owned(),
            "--verbose".to_owned(),
            "3".to_owned(),
        ])
        .unwrap();
        assert_eq!(
            parsed,
            options("mafmkm", 3, true, CandidateLabOutputMode::Verbose)
        );
    }

    #[test]
    fn argument_parser_rejects_conflicting_output_modes() {
        let error = parse_candidate_lab_arguments(&[
            "mafmkm".to_owned(),
            "--verbose".to_owned(),
            "--json".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("不能同时使用"));
    }

    #[test]
    fn concise_output_hides_research_jargon_and_recovery_by_default() {
        let report = report("mafmkm", 3);
        let output = render_candidate_lab(
            &report,
            &options("mafmkm", 3, false, CandidateLabOutputMode::Concise),
        );
        assert!(output.contains("1. 麻烦猫猫"));
        assert!(output.contains("预计 7 次操作，比完整输入少 2 次"));
        assert!(output.contains("第 2 个音节使用简拼"));
        assert!(!output.contains("候选评分"));
        assert!(!output.contains("unigram"));
        assert!(!output.contains("可能的按键颠倒（查看"));
    }

    #[test]
    fn recovery_and_verbose_details_are_explicit() {
        let report = report("mafkmm", 3);
        let concise = render_candidate_lab(
            &report,
            &options("mafkmm", 3, true, CandidateLabOutputMode::Concise),
        );
        assert!(concise.contains("可能的按键颠倒（查看这一栏计 1 次额外操作）"));

        let verbose = render_candidate_lab(
            &report,
            &options("mafkmm", 3, false, CandidateLabOutputMode::Verbose),
        );
        assert!(verbose.contains("候选评分"));
        assert!(verbose.contains("独立词频 unigram"));
        assert!(verbose.contains("按键颠倒恢复候选默认隐藏"));
    }

    #[test]
    fn json_output_uses_stable_fields_and_escapes_strings() {
        let report = report("mafmkm", 1);
        let output = render_candidate_lab(
            &report,
            &options("mafmkm", 1, false, CandidateLabOutputMode::Json),
        );
        assert!(output.starts_with("{\"schema\":\"ziranma-candidate-lab-v1\""));
        assert!(output.contains("\"recovery_included\":false"));
        assert!(output.contains("\"anchored_transposition_recovery\":[]"));
        assert!(!output.contains("候选实验台"));

        let mut escaped = String::new();
        push_json_string(&mut escaped, "\"猫\\\n");
        assert_eq!(escaped, "\"\\\"猫\\\\\\n\"");
    }
}
