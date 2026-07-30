use std::env;
use std::fmt::Write as _;
use std::process::ExitCode;

use ziranma_core::{
    AdaptiveComparisonOutcome, AdaptiveComparisonProfile, AdaptiveSyntheticScenario,
    AdaptiveSyntheticSuiteReport, PendingSelectionLimits,
    evaluate_public_synthetic_adaptive_scenarios,
};

const HELP: &str = "\
公开合成测试

用法：
  cargo run --release --bin adaptive-lab
  cargo run --release --bin adaptive-lab -- --details

选项：
  --details     显示每组配置的完整指标
  -h, --help    显示帮助

使用程序内置的公开合成事件；不读取会话、文件或私人模型。
";

const PROFILES: [AdaptiveComparisonProfile; 4] = [
    AdaptiveComparisonProfile::Reference,
    AdaptiveComparisonProfile::HigherConfirmationThreshold,
    AdaptiveComparisonProfile::LowerInfluence,
    AdaptiveComparisonProfile::HigherInfluence,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Summary,
    Details,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Run(OutputMode),
    Help,
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let command = match parse_command(&arguments) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let mode = match command {
        Command::Run(mode) => mode,
        Command::Help => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
    };
    let report =
        match evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default()) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("公开合成测试失败：{error}");
                return ExitCode::FAILURE;
            }
        };
    match render_report(&report, mode) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_command(arguments: &[String]) -> Result<Command, &'static str> {
    match arguments {
        [] => Ok(Command::Run(OutputMode::Summary)),
        [argument] if argument == "--details" => Ok(Command::Run(OutputMode::Details)),
        [argument] if argument == "-h" || argument == "--help" => Ok(Command::Help),
        _ => Err("用法：cargo run --release --bin adaptive-lab [--details]"),
    }
}

fn render_report(
    report: &AdaptiveSyntheticSuiteReport,
    mode: OutputMode,
) -> Result<String, &'static str> {
    match mode {
        OutputMode::Summary => render_summary(report),
        OutputMode::Details => render_details(report),
    }
}

fn render_summary(report: &AdaptiveSyntheticSuiteReport) -> Result<String, &'static str> {
    let mut output = String::new();
    writeln!(output, "公开合成测试").map_err(|_| "无法生成实验报告")?;
    writeln!(output).map_err(|_| "无法生成实验报告")?;
    write_table_row(
        &mut output,
        &[
            ("场景", 18),
            ("观察项", 14),
            ("参考", 10),
            ("三次确认", 12),
            ("低影响", 10),
            ("高影响", 0),
        ],
    );
    writeln!(output, "{}", "-".repeat(80)).map_err(|_| "无法生成实验报告")?;

    for scenario in &report.outcomes {
        let values = PROFILES
            .map(|profile| profile_outcome(scenario, profile))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|outcome| primary_metric_value(scenario.scenario, outcome))
            .collect::<Vec<_>>();
        write_table_row(
            &mut output,
            &[
                (scenario_label(scenario.scenario), 18),
                (primary_metric_label(scenario.scenario), 14),
                (&values[0], 10),
                (&values[1], 12),
                (&values[2], 10),
                (&values[3], 0),
            ],
        );
    }

    Ok(output)
}

fn render_details(report: &AdaptiveSyntheticSuiteReport) -> Result<String, &'static str> {
    let mut output = String::new();
    writeln!(output, "公开合成测试（详细）").map_err(|_| "无法生成实验报告")?;

    let first_scenario = report.outcomes.first().ok_or("实验报告缺少场景")?;
    writeln!(output).map_err(|_| "无法生成实验报告")?;
    writeln!(output, "配置").map_err(|_| "无法生成实验报告")?;
    for profile in PROFILES {
        let outcome = profile_outcome(first_scenario, profile)?;
        writeln!(
            output,
            "  {}：确认 {} 次；排序上限 {:.0}%；覆盖上限 {:.0}%",
            profile_label(profile),
            outcome.parameters.minimum_confirmations,
            outcome.parameters.max_personal_mix * 100.0,
            outcome.parameters.max_coverage_probability * 100.0,
        )
        .map_err(|_| "无法生成实验报告")?;
    }

    for scenario in &report.outcomes {
        let reference = scenario
            .comparison
            .outcomes
            .first()
            .ok_or("实验报告缺少固定配置")?;
        writeln!(output).map_err(|_| "无法生成实验报告")?;
        writeln!(
            output,
            "{}（{} 个事件；确认 {}；撤回 {}；遗忘 {}）",
            scenario_label(scenario.scenario),
            scenario.comparison.events,
            reference.report.confirmation_boundaries,
            reference.report.retracted_pending,
            reference.report.forgotten_confirmed
        )
        .map_err(|_| "无法生成实验报告")?;

        for outcome in &scenario.comparison.outcomes {
            writeln!(
                output,
                "  {}：生词找回 {}/{}；首选命中 {}/{}；未选个人候选 {}；\
                 公共候选累计下移 {}；个人覆盖峰值 {:.1}%",
                profile_label(outcome.profile),
                outcome.report.oov_recalled,
                outcome.report.oov_queries,
                outcome.report.selected_hits_at_1,
                outcome.report.queries,
                outcome.report.nonselected_personal_candidates,
                outcome.report.public_rank_displacement_total,
                outcome.report.maximum_coverage_probability_mass * 100.0,
            )
            .map_err(|_| "无法生成实验报告")?;
        }
    }

    Ok(output)
}

fn write_table_row(output: &mut String, cells: &[(&str, usize)]) {
    for (index, (text, width)) in cells.iter().enumerate() {
        output.push_str(text);
        if index + 1 < cells.len() {
            let padding = width.saturating_sub(display_width(text));
            for _ in 0..padding {
                output.push(' ');
            }
            output.push_str("  ");
        }
    }
    output.push('\n');
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| if character.is_ascii() { 1 } else { 2 })
        .sum()
}

fn profile_outcome(
    scenario: &ziranma_core::AdaptiveSyntheticScenarioOutcome,
    profile: AdaptiveComparisonProfile,
) -> Result<&AdaptiveComparisonOutcome, &'static str> {
    scenario
        .comparison
        .outcomes
        .iter()
        .find(|outcome| outcome.profile == profile)
        .ok_or("实验报告缺少固定配置")
}

fn primary_metric_label(scenario: AdaptiveSyntheticScenario) -> &'static str {
    match scenario {
        AdaptiveSyntheticScenario::PublicReranking => "首选次数",
        AdaptiveSyntheticScenario::NonselectedCoverageOccupancy => "未选候选数",
        AdaptiveSyntheticScenario::StableRepeatedCoverage
        | AdaptiveSyntheticScenario::RetractedBeforeConfirmation
        | AdaptiveSyntheticScenario::ExactCodeAliasIsolation
        | AdaptiveSyntheticScenario::ExplicitForget => "找回次数",
    }
}

fn primary_metric_value(
    scenario: AdaptiveSyntheticScenario,
    outcome: &AdaptiveComparisonOutcome,
) -> String {
    match scenario {
        AdaptiveSyntheticScenario::PublicReranking => format!(
            "{}/{}",
            outcome.report.selected_hits_at_1, outcome.report.queries
        ),
        AdaptiveSyntheticScenario::NonselectedCoverageOccupancy => {
            outcome.report.nonselected_personal_candidates.to_string()
        }
        AdaptiveSyntheticScenario::StableRepeatedCoverage
        | AdaptiveSyntheticScenario::RetractedBeforeConfirmation
        | AdaptiveSyntheticScenario::ExactCodeAliasIsolation
        | AdaptiveSyntheticScenario::ExplicitForget => format!(
            "{}/{}",
            outcome.report.oov_recalled, outcome.report.oov_queries
        ),
    }
}

fn scenario_label(scenario: AdaptiveSyntheticScenario) -> &'static str {
    match scenario {
        AdaptiveSyntheticScenario::StableRepeatedCoverage => "重复选择生词",
        AdaptiveSyntheticScenario::RetractedBeforeConfirmation => "选择后立即删除",
        AdaptiveSyntheticScenario::ExactCodeAliasIsolation => "同词换码",
        AdaptiveSyntheticScenario::ExplicitForget => "遗忘后再输入",
        AdaptiveSyntheticScenario::PublicReranking => "公共候选重排",
        AdaptiveSyntheticScenario::NonselectedCoverageOccupancy => "个人候选未选",
    }
}

fn profile_label(profile: AdaptiveComparisonProfile) -> &'static str {
    match profile {
        AdaptiveComparisonProfile::Reference => "参考",
        AdaptiveComparisonProfile::HigherConfirmationThreshold => "三次确认",
        AdaptiveComparisonProfile::LowerInfluence => "低影响",
        AdaptiveComparisonProfile::HigherInfluence => "高影响",
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, HELP, OutputMode, parse_command, render_report};
    use ziranma_core::{PendingSelectionLimits, evaluate_public_synthetic_adaptive_scenarios};

    fn report() -> ziranma_core::AdaptiveSyntheticSuiteReport {
        evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default()).unwrap()
    }

    #[test]
    fn parser_keeps_details_explicit_and_accepts_no_external_inputs() {
        assert_eq!(
            parse_command(&[]).unwrap(),
            Command::Run(OutputMode::Summary)
        );
        assert_eq!(
            parse_command(&["--details".to_owned()]).unwrap(),
            Command::Run(OutputMode::Details)
        );
        assert_eq!(
            parse_command(&["--help".to_owned()]).unwrap(),
            Command::Help
        );
        assert!(parse_command(&["private.zcs".to_owned()]).is_err());
        assert!(parse_command(&["--session".to_owned(), "secret".to_owned()]).is_err());
    }

    #[test]
    fn default_report_is_one_aligned_six_row_comparison_table() {
        let output = render_report(&report(), OutputMode::Summary).unwrap();
        let table = output.lines().skip(2).collect::<Vec<_>>();

        assert_eq!(table.len(), 8);
        assert_eq!(table[1].len(), 80);
        assert!(table.iter().all(|line| !line.ends_with(' ')));
        for label in [
            "重复选择生词",
            "选择后立即删除",
            "同词换码",
            "遗忘后再输入",
            "公共候选重排",
            "个人候选未选",
            "参考",
            "三次确认",
            "低影响",
            "高影响",
        ] {
            assert!(output.contains(label), "missing label {label}");
        }
        assert!(!output.contains("个人覆盖峰值"));
    }

    #[test]
    fn details_report_exposes_parameters_and_secondary_metrics() {
        let output = render_report(&report(), OutputMode::Details).unwrap();

        for label in [
            "配置",
            "排序上限",
            "覆盖上限",
            "生词找回",
            "首选命中",
            "未选个人候选",
            "公共候选累计下移",
            "个人覆盖峰值",
        ] {
            assert!(output.contains(label), "missing label {label}");
        }
        assert!(HELP.contains("--details"));
        assert!(HELP.contains("不读取会话、文件或私人模型"));
    }

    #[test]
    fn neither_report_prints_fixture_content_or_product_recommendations() {
        let summary = render_report(&report(), OutputMode::Summary).unwrap();
        let details = render_report(&report(), OutputMode::Details).unwrap();

        for hidden in [
            "公开甲",
            "公开乙",
            "合成丙",
            "\"aa\"",
            "\"bb\"",
            "最佳",
            "建议",
            "下一步",
            "应该采用",
        ] {
            assert!(!summary.contains(hidden), "unexpected summary {hidden}");
            assert!(!details.contains(hidden), "unexpected details {hidden}");
        }
    }
}
