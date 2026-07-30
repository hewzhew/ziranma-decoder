//! Public, manually constructed scenarios for adaptive comparison.
//!
//! Each scenario isolates one behavior and starts from empty memory. The same
//! event sequence is replayed through every fixed comparison profile. Reports
//! expose scenario identities and aggregate numeric results, never event text.
//!
//! This module performs no I/O, persistence, or network access.

use std::error::Error;
use std::fmt;

use crate::{
    AdaptiveComparisonError, AdaptiveComparisonReport, AdaptiveEvaluationEvent,
    AdaptiveRankingCandidate, PendingSelectionEdit, PendingSelectionLimits,
    compare_public_synthetic_adaptive_profiles,
};

pub const ADAPTIVE_SYNTHETIC_SCENARIO_COUNT: usize = 6;

pub const ADAPTIVE_SYNTHETIC_SCENARIOS: [AdaptiveSyntheticScenario;
    ADAPTIVE_SYNTHETIC_SCENARIO_COUNT] = [
    AdaptiveSyntheticScenario::StableRepeatedCoverage,
    AdaptiveSyntheticScenario::RetractedBeforeConfirmation,
    AdaptiveSyntheticScenario::ExactCodeAliasIsolation,
    AdaptiveSyntheticScenario::ExplicitForget,
    AdaptiveSyntheticScenario::PublicReranking,
    AdaptiveSyntheticScenario::NonselectedCoverageOccupancy,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveSyntheticScenario {
    StableRepeatedCoverage,
    RetractedBeforeConfirmation,
    ExactCodeAliasIsolation,
    ExplicitForget,
    PublicReranking,
    NonselectedCoverageOccupancy,
}

impl AdaptiveSyntheticScenario {
    pub fn event_count(self) -> usize {
        scenario_events(self).len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveSyntheticScenarioOutcome {
    pub scenario: AdaptiveSyntheticScenario,
    pub comparison: AdaptiveComparisonReport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveSyntheticSuiteReport {
    pub scenario_count: usize,
    pub total_events: usize,
    pub profile_replays: usize,
    pub outcomes: Vec<AdaptiveSyntheticScenarioOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveSyntheticSuiteError {
    pub scenario: AdaptiveSyntheticScenario,
    pub source: AdaptiveComparisonError,
}

impl fmt::Display for AdaptiveSyntheticSuiteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "adaptive synthetic scenario {:?} failed: {}",
            self.scenario, self.source
        )
    }
}

impl Error for AdaptiveSyntheticSuiteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

pub fn evaluate_public_synthetic_adaptive_scenarios(
    limits: PendingSelectionLimits,
) -> Result<AdaptiveSyntheticSuiteReport, AdaptiveSyntheticSuiteError> {
    let mut total_events = 0usize;
    let mut profile_replays = 0usize;
    let mut outcomes = Vec::with_capacity(ADAPTIVE_SYNTHETIC_SCENARIO_COUNT);

    for scenario in ADAPTIVE_SYNTHETIC_SCENARIOS {
        let events = scenario_events(scenario);
        let comparison = compare_public_synthetic_adaptive_profiles(events, limits)
            .map_err(|source| AdaptiveSyntheticSuiteError { scenario, source })?;
        total_events = total_events.saturating_add(events.len());
        profile_replays = profile_replays.saturating_add(comparison.outcomes.len());
        outcomes.push(AdaptiveSyntheticScenarioOutcome {
            scenario,
            comparison,
        });
    }

    Ok(AdaptiveSyntheticSuiteReport {
        scenario_count: outcomes.len(),
        total_events,
        profile_replays,
        outcomes,
    })
}

static PUBLIC_SINGLE: [AdaptiveRankingCandidate<'static>; 1] = [AdaptiveRankingCandidate {
    text: "公开甲",
    public_weight: 1.0,
}];

static PUBLIC_COMPETITION: [AdaptiveRankingCandidate<'static>; 2] = [
    AdaptiveRankingCandidate {
        text: "公开甲",
        public_weight: 0.6,
    },
    AdaptiveRankingCandidate {
        text: "公开乙",
        public_weight: 0.4,
    },
];

static STABLE_REPEATED_COVERAGE_EVENTS: [AdaptiveEvaluationEvent<'static>; 7] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 1,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 2,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 3,
    },
];

static RETRACTED_BEFORE_CONFIRMATION_EVENTS: [AdaptiveEvaluationEvent<'static>; 7] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::DocumentEdit(PendingSelectionEdit {
        start: 0,
        deleted_chars: 3,
        inserted_chars: 0,
    }),
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::DocumentEdit(PendingSelectionEdit {
        start: 0,
        deleted_chars: 3,
        inserted_chars: 0,
    }),
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
];

static EXACT_CODE_ALIAS_ISOLATION_EVENTS: [AdaptiveEvaluationEvent<'static>; 5] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 3,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "bb",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 6,
    },
];

static EXPLICIT_FORGET_EVENTS: [AdaptiveEvaluationEvent<'static>; 6] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 3,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::ForgetSelection {
        code: "aa",
        text: "合成丙",
    },
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 6,
    },
];

static PUBLIC_RERANKING_EVENTS: [AdaptiveEvaluationEvent<'static>; 5] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_COMPETITION,
        selected_text: "公开乙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_COMPETITION,
        selected_text: "公开乙",
        document_start: 3,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_COMPETITION,
        selected_text: "公开乙",
        document_start: 6,
    },
];

static NONSELECTED_COVERAGE_OCCUPANCY_EVENTS: [AdaptiveEvaluationEvent<'static>; 5] = [
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 0,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "合成丙",
        document_start: 3,
    },
    AdaptiveEvaluationEvent::ConfirmBoundary,
    AdaptiveEvaluationEvent::QueryAndCommit {
        code: "aa",
        public_candidates: &PUBLIC_SINGLE,
        selected_text: "公开甲",
        document_start: 6,
    },
];

fn scenario_events(
    scenario: AdaptiveSyntheticScenario,
) -> &'static [AdaptiveEvaluationEvent<'static>] {
    match scenario {
        AdaptiveSyntheticScenario::StableRepeatedCoverage => &STABLE_REPEATED_COVERAGE_EVENTS,
        AdaptiveSyntheticScenario::RetractedBeforeConfirmation => {
            &RETRACTED_BEFORE_CONFIRMATION_EVENTS
        }
        AdaptiveSyntheticScenario::ExactCodeAliasIsolation => &EXACT_CODE_ALIAS_ISOLATION_EVENTS,
        AdaptiveSyntheticScenario::ExplicitForget => &EXPLICIT_FORGET_EVENTS,
        AdaptiveSyntheticScenario::PublicReranking => &PUBLIC_RERANKING_EVENTS,
        AdaptiveSyntheticScenario::NonselectedCoverageOccupancy => {
            &NONSELECTED_COVERAGE_OCCUPANCY_EVENTS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ADAPTIVE_SYNTHETIC_SCENARIOS, AdaptiveSyntheticScenario, AdaptiveSyntheticScenarioOutcome,
        evaluate_public_synthetic_adaptive_scenarios,
    };
    use crate::{AdaptiveComparisonProfile, PendingSelectionLimits};

    fn outcome(
        report: &super::AdaptiveSyntheticSuiteReport,
        scenario: AdaptiveSyntheticScenario,
    ) -> &AdaptiveSyntheticScenarioOutcome {
        report
            .outcomes
            .iter()
            .find(|outcome| outcome.scenario == scenario)
            .unwrap()
    }

    fn profile(
        outcome: &AdaptiveSyntheticScenarioOutcome,
        profile: AdaptiveComparisonProfile,
    ) -> &crate::AdaptiveComparisonOutcome {
        outcome
            .comparison
            .outcomes
            .iter()
            .find(|outcome| outcome.profile == profile)
            .unwrap()
    }

    #[test]
    fn suite_runs_every_isolated_scenario_through_every_fixed_profile() {
        let report =
            evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default())
                .unwrap();

        assert_eq!(report.scenario_count, ADAPTIVE_SYNTHETIC_SCENARIOS.len());
        assert_eq!(
            report.total_events,
            ADAPTIVE_SYNTHETIC_SCENARIOS
                .iter()
                .map(|scenario| scenario.event_count())
                .sum::<usize>()
        );
        assert_eq!(report.profile_replays, 24);
        assert_eq!(
            report
                .outcomes
                .iter()
                .map(|outcome| outcome.scenario)
                .collect::<Vec<_>>(),
            ADAPTIVE_SYNTHETIC_SCENARIOS
        );
    }

    #[test]
    fn stable_repetition_exposes_confirmation_threshold_sensitivity() {
        let report =
            evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default())
                .unwrap();
        let scenario = outcome(&report, AdaptiveSyntheticScenario::StableRepeatedCoverage);
        let reference = profile(scenario, AdaptiveComparisonProfile::Reference);
        let threshold = profile(
            scenario,
            AdaptiveComparisonProfile::HigherConfirmationThreshold,
        );

        assert_eq!(reference.report.oov_queries, 4);
        assert_eq!(reference.report.oov_recalled, 2);
        assert_eq!(threshold.report.oov_recalled, 1);
        assert_eq!(threshold.delta_from_reference.oov_recalled, -1);
    }

    #[test]
    fn retraction_alias_and_forget_scenarios_do_not_create_coverage_leaks() {
        let report =
            evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default())
                .unwrap();

        for profile_name in [
            AdaptiveComparisonProfile::Reference,
            AdaptiveComparisonProfile::HigherConfirmationThreshold,
            AdaptiveComparisonProfile::LowerInfluence,
            AdaptiveComparisonProfile::HigherInfluence,
        ] {
            let retracted = profile(
                outcome(
                    &report,
                    AdaptiveSyntheticScenario::RetractedBeforeConfirmation,
                ),
                profile_name,
            );
            let alias = profile(
                outcome(&report, AdaptiveSyntheticScenario::ExactCodeAliasIsolation),
                profile_name,
            );
            let forgotten = profile(
                outcome(&report, AdaptiveSyntheticScenario::ExplicitForget),
                profile_name,
            );

            assert_eq!(retracted.report.retracted_pending, 2);
            assert_eq!(retracted.report.oov_recalled, 0);
            assert_eq!(alias.report.oov_recalled, 0);
            assert_eq!(forgotten.report.oov_recalled, 0);
            assert_eq!(forgotten.report.forgotten_confirmed, 1);
        }
    }

    #[test]
    fn reranking_and_nonselected_occupancy_remain_separate_measurements() {
        let report =
            evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default())
                .unwrap();
        let reranking = outcome(&report, AdaptiveSyntheticScenario::PublicReranking);
        let occupancy = outcome(
            &report,
            AdaptiveSyntheticScenario::NonselectedCoverageOccupancy,
        );
        let reranking_reference = profile(reranking, AdaptiveComparisonProfile::Reference);
        let reranking_higher = profile(reranking, AdaptiveComparisonProfile::HigherInfluence);
        let occupancy_reference = profile(occupancy, AdaptiveComparisonProfile::Reference);
        let occupancy_threshold = profile(
            occupancy,
            AdaptiveComparisonProfile::HigherConfirmationThreshold,
        );

        assert_eq!(reranking_reference.report.oov_queries, 0);
        assert_eq!(reranking_reference.report.selected_hits_at_1, 0);
        assert_eq!(reranking_higher.report.selected_hits_at_1, 2);
        assert_eq!(
            occupancy_reference.report.nonselected_personal_candidates,
            1
        );
        assert_eq!(
            occupancy_threshold.report.nonselected_personal_candidates,
            0
        );
    }

    #[test]
    fn suite_debug_output_contains_no_synthetic_event_text_or_codes() {
        let report =
            evaluate_public_synthetic_adaptive_scenarios(PendingSelectionLimits::default())
                .unwrap();
        let debug = format!("{report:?}");

        assert!(!debug.contains("公开甲"));
        assert!(!debug.contains("公开乙"));
        assert!(!debug.contains("合成丙"));
        assert!(!debug.contains("\"aa\""));
        assert!(!debug.contains("\"bb\""));
    }
}
