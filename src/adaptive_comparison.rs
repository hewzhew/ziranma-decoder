//! Fixed-profile comparison for public, manually constructed event streams.
//!
//! Each profile replays the same causal stream independently. The profiles
//! vary one research dimension at a time: the coverage confirmation threshold
//! or the maximum personal influence. Results contain only aggregate counts,
//! numeric parameters, and deltas from the reference profile.
//!
//! This module performs no I/O, persistence, or network access. It cannot
//! establish that an event stream is public or synthetic; callers must enforce
//! that boundary before constructing [`crate::AdaptiveEvaluationEvent`] values.

use std::error::Error;
use std::fmt;

use crate::{
    AdaptiveEvaluationError, AdaptiveEvaluationEvent, AdaptiveEvaluationReport,
    AdaptiveMergeConfig, PendingSelectionLimits, evaluate_adaptive_closed_loop,
};

pub const ADAPTIVE_COMPARISON_PROFILE_COUNT: usize = 4;

pub const ADAPTIVE_COMPARISON_PROFILES: [AdaptiveComparisonProfile;
    ADAPTIVE_COMPARISON_PROFILE_COUNT] = [
    AdaptiveComparisonProfile::Reference,
    AdaptiveComparisonProfile::HigherConfirmationThreshold,
    AdaptiveComparisonProfile::LowerInfluence,
    AdaptiveComparisonProfile::HigherInfluence,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveComparisonProfile {
    Reference,
    HigherConfirmationThreshold,
    LowerInfluence,
    HigherInfluence,
}

impl AdaptiveComparisonProfile {
    pub fn merge_config(self) -> AdaptiveMergeConfig {
        let mut config = AdaptiveMergeConfig::default();
        match self {
            Self::Reference => {}
            Self::HigherConfirmationThreshold => {
                config.coverage.minimum_confirmations = 3;
            }
            Self::LowerInfluence => {
                config.ranking.max_personal_mix = 0.10;
                config.max_coverage_probability = 0.02;
            }
            Self::HigherInfluence => {
                config.ranking.max_personal_mix = 0.40;
                config.max_coverage_probability = 0.10;
            }
        }
        config
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveComparisonParameters {
    pub minimum_confirmations: u64,
    pub max_personal_mix: f64,
    pub max_coverage_probability: f64,
    pub ranking_evidence_saturation: f64,
    pub coverage_evidence_saturation: f64,
}

impl From<AdaptiveMergeConfig> for AdaptiveComparisonParameters {
    fn from(config: AdaptiveMergeConfig) -> Self {
        Self {
            minimum_confirmations: config.coverage.minimum_confirmations,
            max_personal_mix: config.ranking.max_personal_mix,
            max_coverage_probability: config.max_coverage_probability,
            ranking_evidence_saturation: config.ranking.evidence_saturation,
            coverage_evidence_saturation: config.coverage_evidence_saturation,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdaptiveComparisonDelta {
    pub oov_recalled: i64,
    pub selected_hits_at_1: i64,
    pub selected_hits_at_5: i64,
    pub selected_hits_at_10: i64,
    pub public_top_1_changes: i64,
    pub public_rank_displacement_total: i64,
    pub nonselected_personal_candidates: i64,
    pub nonselected_personal_candidates_in_top_5: i64,
    pub maximum_coverage_probability_mass: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveComparisonOutcome {
    pub profile: AdaptiveComparisonProfile,
    pub parameters: AdaptiveComparisonParameters,
    pub report: AdaptiveEvaluationReport,
    pub delta_from_reference: AdaptiveComparisonDelta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveComparisonReport {
    pub events: usize,
    pub outcomes: Vec<AdaptiveComparisonOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveComparisonError {
    pub profile: AdaptiveComparisonProfile,
    pub source: AdaptiveEvaluationError,
}

impl fmt::Display for AdaptiveComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "adaptive comparison profile {:?} failed: {}",
            self.profile, self.source
        )
    }
}

impl Error for AdaptiveComparisonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

pub fn compare_public_synthetic_adaptive_profiles(
    events: &[AdaptiveEvaluationEvent<'_>],
    limits: PendingSelectionLimits,
) -> Result<AdaptiveComparisonReport, AdaptiveComparisonError> {
    let mut outcomes = Vec::with_capacity(ADAPTIVE_COMPARISON_PROFILE_COUNT);
    let mut reference = None;

    for profile in ADAPTIVE_COMPARISON_PROFILES {
        let config = profile.merge_config();
        let report = evaluate_adaptive_closed_loop(events, limits, config)
            .map_err(|source| AdaptiveComparisonError { profile, source })?;
        let baseline = reference.get_or_insert(report);
        outcomes.push(AdaptiveComparisonOutcome {
            profile,
            parameters: config.into(),
            report,
            delta_from_reference: report_delta(report, *baseline),
        });
    }

    Ok(AdaptiveComparisonReport {
        events: events.len(),
        outcomes,
    })
}

fn report_delta(
    report: AdaptiveEvaluationReport,
    reference: AdaptiveEvaluationReport,
) -> AdaptiveComparisonDelta {
    AdaptiveComparisonDelta {
        oov_recalled: difference(report.oov_recalled, reference.oov_recalled),
        selected_hits_at_1: difference(report.selected_hits_at_1, reference.selected_hits_at_1),
        selected_hits_at_5: difference(report.selected_hits_at_5, reference.selected_hits_at_5),
        selected_hits_at_10: difference(report.selected_hits_at_10, reference.selected_hits_at_10),
        public_top_1_changes: difference(
            report.public_top_1_changes,
            reference.public_top_1_changes,
        ),
        public_rank_displacement_total: difference(
            report.public_rank_displacement_total,
            reference.public_rank_displacement_total,
        ),
        nonselected_personal_candidates: difference(
            report.nonselected_personal_candidates,
            reference.nonselected_personal_candidates,
        ),
        nonselected_personal_candidates_in_top_5: difference(
            report.nonselected_personal_candidates_in_top_5,
            reference.nonselected_personal_candidates_in_top_5,
        ),
        maximum_coverage_probability_mass: report.maximum_coverage_probability_mass
            - reference.maximum_coverage_probability_mass,
    }
}

fn difference(value: usize, reference: usize) -> i64 {
    value as i64 - reference as i64
}

#[cfg(test)]
mod tests {
    use super::{
        ADAPTIVE_COMPARISON_PROFILES, AdaptiveComparisonProfile,
        compare_public_synthetic_adaptive_profiles,
    };
    use crate::{
        AdaptiveEvaluationErrorKind, AdaptiveEvaluationEvent, AdaptiveRankingCandidate,
        PendingSelectionEdit, PendingSelectionLimits,
    };

    fn public<'a>(values: &'a [(&'a str, f64)]) -> Vec<AdaptiveRankingCandidate<'a>> {
        values
            .iter()
            .map(|(text, public_weight)| AdaptiveRankingCandidate {
                text,
                public_weight: *public_weight,
            })
            .collect()
    }

    #[test]
    fn fixed_profiles_change_only_the_documented_research_dimensions() {
        let reference = AdaptiveComparisonProfile::Reference.merge_config();
        let threshold = AdaptiveComparisonProfile::HigherConfirmationThreshold.merge_config();
        let lower = AdaptiveComparisonProfile::LowerInfluence.merge_config();
        let higher = AdaptiveComparisonProfile::HigherInfluence.merge_config();

        assert_eq!(ADAPTIVE_COMPARISON_PROFILES.len(), 4);
        assert_eq!(threshold.coverage.minimum_confirmations, 3);
        assert_eq!(threshold.ranking, reference.ranking);
        assert_eq!(
            threshold.max_coverage_probability,
            reference.max_coverage_probability
        );
        assert_eq!(lower.coverage, reference.coverage);
        assert_eq!(lower.ranking.max_personal_mix, 0.10);
        assert_eq!(lower.max_coverage_probability, 0.02);
        assert_eq!(higher.coverage, reference.coverage);
        assert_eq!(higher.ranking.max_personal_mix, 0.40);
        assert_eq!(higher.max_coverage_probability, 0.10);
    }

    #[test]
    fn same_causal_stream_exposes_threshold_and_influence_tradeoffs() {
        let coverage_public = public(&[("甲", 1.0)]);
        let ranking_public = public(&[("甲", 0.6), ("乙", 0.4)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &coverage_public,
                selected_text: "丙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &coverage_public,
                selected_text: "丙",
                document_start: 1,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &coverage_public,
                selected_text: "丙",
                document_start: 2,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &coverage_public,
                selected_text: "甲",
                document_start: 3,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "bb",
                public_candidates: &ranking_public,
                selected_text: "乙",
                document_start: 4,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "bb",
                public_candidates: &ranking_public,
                selected_text: "乙",
                document_start: 5,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "bb",
                public_candidates: &ranking_public,
                selected_text: "乙",
                document_start: 6,
            },
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "cc",
                public_candidates: &coverage_public,
                selected_text: "丁",
                document_start: 7,
            },
            AdaptiveEvaluationEvent::DocumentEdit(PendingSelectionEdit {
                start: 7,
                deleted_chars: 1,
                inserted_chars: 0,
            }),
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "cc",
                public_candidates: &coverage_public,
                selected_text: "丁",
                document_start: 7,
            },
        ];

        let report = compare_public_synthetic_adaptive_profiles(
            &events,
            PendingSelectionLimits::new(32, 64).unwrap(),
        )
        .unwrap();
        let reference = &report.outcomes[0];
        let threshold = &report.outcomes[1];
        let lower = &report.outcomes[2];
        let higher = &report.outcomes[3];

        assert_eq!(report.events, events.len());
        assert_eq!(reference.profile, AdaptiveComparisonProfile::Reference);
        assert_eq!(reference.delta_from_reference, Default::default());
        assert_eq!(reference.report.oov_recalled, 1);
        assert_eq!(threshold.report.oov_recalled, 0);
        assert_eq!(threshold.delta_from_reference.oov_recalled, -1);
        assert_eq!(reference.report.retracted_pending, 1);
        assert_eq!(threshold.report.retracted_pending, 1);
        assert_eq!(lower.report.retracted_pending, 1);
        assert_eq!(higher.report.retracted_pending, 1);
        assert_eq!(reference.report.nonselected_personal_candidates, 1);
        assert_eq!(threshold.report.nonselected_personal_candidates, 1);
        assert_eq!(lower.report.selected_hits_at_1, 1);
        assert_eq!(higher.report.selected_hits_at_1, 3);
        assert_eq!(higher.delta_from_reference.selected_hits_at_1, 2);
        assert!(
            higher.report.maximum_coverage_probability_mass
                > reference.report.maximum_coverage_probability_mass
        );
        assert_eq!(higher.report.coverage_probability_cap_violations, 0);
    }

    #[test]
    fn comparison_errors_identify_the_profile_without_exposing_event_text() {
        let public = public(&[("公开候选", 1.0)]);
        let events = [AdaptiveEvaluationEvent::QueryAndCommit {
            code: "privatecode",
            public_candidates: &public,
            selected_text: "",
            document_start: 0,
        }];

        let error =
            compare_public_synthetic_adaptive_profiles(&events, PendingSelectionLimits::default())
                .unwrap_err();
        let debug = format!("{error:?}");

        assert_eq!(error.profile, AdaptiveComparisonProfile::Reference);
        assert!(matches!(
            error.source.kind,
            AdaptiveEvaluationErrorKind::Memory(_)
        ));
        assert!(!debug.contains("公开候选"));
        assert!(!debug.contains("privatecode"));
    }
}
