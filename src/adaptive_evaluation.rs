//! Causal closed-loop evaluation over manually constructed event streams.
//!
//! Every query is evaluated before its selected text is observed. Pending
//! selections remain invisible until an explicit confirmation boundary, and
//! overlapping edits may retract them first. Reports contain only aggregate
//! counts and probability metadata.
//!
//! Event types intentionally have no `Debug` implementation because they
//! borrow text. This module performs no I/O and is not connected to TSF.

use std::error::Error;
use std::fmt;

use crate::{
    AdaptiveMergeConfig, AdaptiveMergeError, AdaptiveMergedCandidateSource,
    AdaptiveRankingCandidate, AdaptiveRankingError, ConfirmedSelectionTierCounts,
    MAX_ADAPTIVE_COVERAGE_PROBABILITY, PendingSelectionEdit, PendingSelectionError,
    PendingSelectionLimits, PendingSelectionMemory, merge_adaptive_candidates,
    rank_visible_candidates,
};

pub const MAX_ADAPTIVE_EVALUATION_EVENTS: usize = 4_096;

/// One manually constructed event in a causal adaptive evaluation stream.
///
/// This type deliberately has no `Debug` implementation because some variants
/// borrow candidate and selection text.
pub enum AdaptiveEvaluationEvent<'a> {
    QueryAndCommit {
        code: &'a str,
        public_candidates: &'a [AdaptiveRankingCandidate<'a>],
        selected_text: &'a str,
        document_start: usize,
    },
    DocumentEdit(PendingSelectionEdit),
    ConfirmBoundary,
    ForgetSelection {
        code: &'a str,
        text: &'a str,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdaptiveEvaluationReport {
    pub events: usize,
    pub queries: usize,
    pub document_edits: usize,
    pub confirmation_boundaries: usize,
    pub forget_events: usize,
    pub oov_queries: usize,
    pub oov_recalled: usize,
    pub selected_hits_at_1: usize,
    pub selected_hits_at_5: usize,
    pub selected_hits_at_10: usize,
    pub public_selected_queries: usize,
    pub selected_public_rank_improved: usize,
    pub selected_public_rank_unchanged: usize,
    pub selected_public_rank_worsened: usize,
    pub public_top_1_changes: usize,
    pub public_candidates_rank_worsened: usize,
    pub public_rank_displacement_total: usize,
    pub returned_personal_candidates: usize,
    pub personal_candidates_in_top_5: usize,
    pub nonselected_personal_candidates: usize,
    pub nonselected_personal_candidates_in_top_5: usize,
    pub coverage_probability_cap_violations: usize,
    pub maximum_coverage_probability_mass: f64,
    pub shifted_pending: usize,
    pub retracted_pending: usize,
    pub confirmed_pending: usize,
    pub new_confirmed_entries: usize,
    pub updated_confirmed_entries: usize,
    pub moved_to_medium: usize,
    pub moved_to_long: usize,
    pub evicted_confirmed_entries: usize,
    pub forgotten_pending: usize,
    pub forgotten_confirmed: usize,
    pub pending_remaining: usize,
    pub confirmed_remaining: usize,
    pub confirmed_tiers: ConfirmedSelectionTierCounts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveEvaluationErrorKind {
    InvalidEventCount,
    Memory(PendingSelectionError),
    Merge(AdaptiveMergeError),
    BaselineRanking(AdaptiveRankingError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdaptiveEvaluationError {
    pub event_index: Option<usize>,
    pub kind: AdaptiveEvaluationErrorKind,
}

impl fmt::Display for AdaptiveEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.event_index {
            Some(index) => write!(
                formatter,
                "adaptive evaluation failed at event {}: {}",
                index + 1,
                self.kind
            ),
            None => write!(formatter, "adaptive evaluation failed: {}", self.kind),
        }
    }
}

impl Error for AdaptiveEvaluationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            AdaptiveEvaluationErrorKind::Memory(error) => Some(error),
            AdaptiveEvaluationErrorKind::Merge(error) => Some(error),
            AdaptiveEvaluationErrorKind::BaselineRanking(error) => Some(error),
            AdaptiveEvaluationErrorKind::InvalidEventCount => None,
        }
    }
}

impl fmt::Display for AdaptiveEvaluationErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEventCount => formatter.write_str("event count is outside fixed bounds"),
            Self::Memory(error) => write!(formatter, "memory transition failed: {error}"),
            Self::Merge(error) => write!(formatter, "candidate merge failed: {error}"),
            Self::BaselineRanking(error) => {
                write!(formatter, "public baseline ranking failed: {error}")
            }
        }
    }
}

pub fn evaluate_adaptive_closed_loop(
    events: &[AdaptiveEvaluationEvent<'_>],
    limits: PendingSelectionLimits,
    config: AdaptiveMergeConfig,
) -> Result<AdaptiveEvaluationReport, AdaptiveEvaluationError> {
    if events.is_empty() || events.len() > MAX_ADAPTIVE_EVALUATION_EVENTS {
        return Err(AdaptiveEvaluationError {
            event_index: None,
            kind: AdaptiveEvaluationErrorKind::InvalidEventCount,
        });
    }
    let mut memory =
        PendingSelectionMemory::with_limits(limits).map_err(|error| AdaptiveEvaluationError {
            event_index: None,
            kind: AdaptiveEvaluationErrorKind::Memory(error),
        })?;
    let baseline_memory = PendingSelectionMemory::new();
    let mut report = AdaptiveEvaluationReport::default();

    for (event_index, event) in events.iter().enumerate() {
        match event {
            AdaptiveEvaluationEvent::QueryAndCommit {
                code,
                public_candidates,
                selected_text,
                document_start,
            } => {
                let merged = merge_adaptive_candidates(&memory, code, public_candidates, config)
                    .map_err(|error| AdaptiveEvaluationError {
                        event_index: Some(event_index),
                        kind: AdaptiveEvaluationErrorKind::Merge(error),
                    })?;
                let baseline = rank_visible_candidates(
                    &baseline_memory,
                    code,
                    public_candidates,
                    config.ranking,
                )
                .map_err(|error| AdaptiveEvaluationError {
                    event_index: Some(event_index),
                    kind: AdaptiveEvaluationErrorKind::BaselineRanking(error),
                })?;

                record_query(
                    &mut report,
                    public_candidates,
                    selected_text,
                    &baseline,
                    &merged,
                    config,
                );
                memory
                    .observe_commit(code, selected_text, *document_start)
                    .map_err(|error| AdaptiveEvaluationError {
                        event_index: Some(event_index),
                        kind: AdaptiveEvaluationErrorKind::Memory(error),
                    })?;
            }
            AdaptiveEvaluationEvent::DocumentEdit(edit) => {
                let outcome =
                    memory
                        .apply_edit(*edit)
                        .map_err(|error| AdaptiveEvaluationError {
                            event_index: Some(event_index),
                            kind: AdaptiveEvaluationErrorKind::Memory(error),
                        })?;
                report.document_edits = report.document_edits.saturating_add(1);
                report.shifted_pending = report
                    .shifted_pending
                    .saturating_add(outcome.shifted_pending);
                report.retracted_pending = report
                    .retracted_pending
                    .saturating_add(outcome.retracted_pending);
            }
            AdaptiveEvaluationEvent::ConfirmBoundary => {
                let outcome = memory.confirm_pending();
                report.confirmation_boundaries = report.confirmation_boundaries.saturating_add(1);
                report.confirmed_pending = report
                    .confirmed_pending
                    .saturating_add(outcome.confirmed_pending);
                report.new_confirmed_entries = report
                    .new_confirmed_entries
                    .saturating_add(outcome.new_confirmed_entries);
                report.updated_confirmed_entries = report
                    .updated_confirmed_entries
                    .saturating_add(outcome.updated_confirmed_entries);
                report.moved_to_medium = report
                    .moved_to_medium
                    .saturating_add(outcome.moved_to_medium);
                report.moved_to_long = report.moved_to_long.saturating_add(outcome.moved_to_long);
                report.evicted_confirmed_entries = report
                    .evicted_confirmed_entries
                    .saturating_add(outcome.evicted_confirmed_entries);
            }
            AdaptiveEvaluationEvent::ForgetSelection { code, text } => {
                let outcome =
                    memory
                        .forget(code, text)
                        .map_err(|error| AdaptiveEvaluationError {
                            event_index: Some(event_index),
                            kind: AdaptiveEvaluationErrorKind::Memory(error),
                        })?;
                report.forget_events = report.forget_events.saturating_add(1);
                report.forgotten_pending = report
                    .forgotten_pending
                    .saturating_add(outcome.removed_pending);
                report.forgotten_confirmed = report
                    .forgotten_confirmed
                    .saturating_add(outcome.removed_confirmed);
            }
        }
    }

    report.events = events.len();
    report.pending_remaining = memory.pending_len();
    report.confirmed_remaining = memory.confirmed_len();
    report.confirmed_tiers = memory.confirmed_tier_counts();
    Ok(report)
}

fn record_query(
    report: &mut AdaptiveEvaluationReport,
    public_candidates: &[AdaptiveRankingCandidate<'_>],
    selected_text: &str,
    baseline: &crate::AdaptiveRankingReport,
    merged: &crate::AdaptiveMergeReport<'_>,
    config: AdaptiveMergeConfig,
) {
    report.queries = report.queries.saturating_add(1);

    let selected_public_index = public_candidates
        .iter()
        .position(|candidate| candidate.text == selected_text);
    let baseline_selected_rank = selected_public_index.and_then(|selected_index| {
        baseline
            .candidates
            .iter()
            .position(|candidate| candidate.original_index == selected_index)
            .map(|rank| rank + 1)
    });
    let merged_selected_rank = merged
        .candidates()
        .iter()
        .position(|candidate| candidate.text() == selected_text)
        .map(|rank| rank + 1);

    if selected_public_index.is_none() {
        report.oov_queries = report.oov_queries.saturating_add(1);
        if merged_selected_rank.is_some() {
            report.oov_recalled = report.oov_recalled.saturating_add(1);
        }
    } else {
        report.public_selected_queries = report.public_selected_queries.saturating_add(1);
        match (baseline_selected_rank, merged_selected_rank) {
            (Some(before), Some(after)) if after < before => {
                report.selected_public_rank_improved =
                    report.selected_public_rank_improved.saturating_add(1);
            }
            (Some(before), Some(after)) if after == before => {
                report.selected_public_rank_unchanged =
                    report.selected_public_rank_unchanged.saturating_add(1);
            }
            (Some(_), Some(_)) | (Some(_), None) => {
                report.selected_public_rank_worsened =
                    report.selected_public_rank_worsened.saturating_add(1);
            }
            (None, _) => {}
        }
    }

    if let Some(rank) = merged_selected_rank {
        if rank == 1 {
            report.selected_hits_at_1 = report.selected_hits_at_1.saturating_add(1);
        }
        if rank <= 5 {
            report.selected_hits_at_5 = report.selected_hits_at_5.saturating_add(1);
        }
        if rank <= 10 {
            report.selected_hits_at_10 = report.selected_hits_at_10.saturating_add(1);
        }
    }

    let baseline_top = baseline
        .candidates
        .first()
        .map(|candidate| candidate.original_index);
    let merged_top = merged.candidates().first().and_then(|candidate| {
        if let AdaptiveMergedCandidateSource::Public { original_index } = candidate.source {
            Some(original_index)
        } else {
            None
        }
    });
    if baseline_top != merged_top {
        report.public_top_1_changes = report.public_top_1_changes.saturating_add(1);
    }

    for (baseline_rank, baseline_candidate) in baseline.candidates.iter().enumerate() {
        let merged_rank = merged
            .candidates()
            .iter()
            .position(|candidate| {
                matches!(
                    candidate.source,
                    AdaptiveMergedCandidateSource::Public { original_index }
                        if original_index == baseline_candidate.original_index
                )
            })
            .map_or(merged.candidates().len() + 1, |rank| rank + 1);
        let baseline_rank = baseline_rank + 1;
        if merged_rank > baseline_rank {
            report.public_candidates_rank_worsened =
                report.public_candidates_rank_worsened.saturating_add(1);
            report.public_rank_displacement_total = report
                .public_rank_displacement_total
                .saturating_add(merged_rank - baseline_rank);
        }
    }

    let returned_personal = merged
        .candidates()
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.source,
                AdaptiveMergedCandidateSource::PersonalCoverage { .. }
            )
        })
        .count();
    let personal_top_5 = merged
        .candidates()
        .iter()
        .take(5)
        .filter(|candidate| {
            matches!(
                candidate.source,
                AdaptiveMergedCandidateSource::PersonalCoverage { .. }
            )
        })
        .count();
    let nonselected_personal = merged
        .candidates()
        .iter()
        .filter(|candidate| {
            candidate.text() != selected_text
                && matches!(
                    candidate.source,
                    AdaptiveMergedCandidateSource::PersonalCoverage { .. }
                )
        })
        .count();
    let nonselected_personal_top_5 = merged
        .candidates()
        .iter()
        .take(5)
        .filter(|candidate| {
            candidate.text() != selected_text
                && matches!(
                    candidate.source,
                    AdaptiveMergedCandidateSource::PersonalCoverage { .. }
                )
        })
        .count();
    report.returned_personal_candidates = report
        .returned_personal_candidates
        .saturating_add(returned_personal);
    report.personal_candidates_in_top_5 = report
        .personal_candidates_in_top_5
        .saturating_add(personal_top_5);
    report.nonselected_personal_candidates = report
        .nonselected_personal_candidates
        .saturating_add(nonselected_personal);
    report.nonselected_personal_candidates_in_top_5 = report
        .nonselected_personal_candidates_in_top_5
        .saturating_add(nonselected_personal_top_5);
    report.maximum_coverage_probability_mass = report
        .maximum_coverage_probability_mass
        .max(merged.summary.coverage_probability_mass);
    if merged.summary.coverage_probability_mass > config.max_coverage_probability
        || merged.summary.coverage_probability_mass > MAX_ADAPTIVE_COVERAGE_PROBABILITY
    {
        report.coverage_probability_cap_violations =
            report.coverage_probability_cap_violations.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptiveEvaluationErrorKind, AdaptiveEvaluationEvent, evaluate_adaptive_closed_loop,
    };
    use crate::{
        AdaptiveMergeConfig, AdaptiveRankingCandidate, PendingSelectionEdit, PendingSelectionLimits,
    };

    fn public<'a>(values: &'a [(&'a str, f64)]) -> Vec<AdaptiveRankingCandidate<'a>> {
        values
            .iter()
            .map(|(text, weight)| AdaptiveRankingCandidate {
                text,
                public_weight: *weight,
            })
            .collect()
    }

    fn defaults() -> (PendingSelectionLimits, AdaptiveMergeConfig) {
        (
            PendingSelectionLimits::new(32, 64).unwrap(),
            AdaptiveMergeConfig::default(),
        )
    }

    #[test]
    fn oov_is_recalled_only_after_two_prior_confirmed_observations() {
        let public = public(&[("甲", 1.0)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 1,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 2,
            },
        ];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.queries, 3);
        assert_eq!(report.oov_queries, 3);
        assert_eq!(report.oov_recalled, 1);
        assert_eq!(report.selected_hits_at_5, 1);
        assert_eq!(report.confirmed_pending, 2);
        assert_eq!(report.pending_remaining, 1);
        assert_eq!(report.coverage_probability_cap_violations, 0);
    }

    #[test]
    fn overlapping_edits_retract_observations_before_confirmation() {
        let public = public(&[("甲", 1.0)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::DocumentEdit(PendingSelectionEdit {
                start: 0,
                deleted_chars: 1,
                inserted_chars: 0,
            }),
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::DocumentEdit(PendingSelectionEdit {
                start: 0,
                deleted_chars: 1,
                inserted_chars: 0,
            }),
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
        ];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.retracted_pending, 2);
        assert_eq!(report.confirmed_pending, 0);
        assert_eq!(report.oov_recalled, 0);
        assert_eq!(report.confirmed_remaining, 0);
    }

    #[test]
    fn multiple_pending_answers_remain_invisible_until_the_boundary() {
        let public = public(&[("甲", 1.0)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 1,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 2,
            },
        ];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.oov_recalled, 1);
        assert_eq!(report.confirmed_pending, 2);
        assert_eq!(report.new_confirmed_entries, 1);
        assert_eq!(report.updated_confirmed_entries, 1);
    }

    #[test]
    fn forget_removes_both_confirmed_and_current_pending_evidence() {
        let public = public(&[("甲", 1.0)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 1,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 2,
            },
            AdaptiveEvaluationEvent::ForgetSelection {
                code: "aa",
                text: "乙",
            },
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 2,
            },
        ];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.oov_recalled, 1);
        assert_eq!(report.forgotten_pending, 1);
        assert_eq!(report.forgotten_confirmed, 1);
        assert_eq!(report.confirmed_remaining, 0);
    }

    #[test]
    fn report_counts_public_displacement_without_a_probability_cap_violation() {
        let public = public(&[("甲", 1.0), ("乙", 0.000_001)]);
        let mut events = Vec::new();
        for start in 0..2 {
            events.push(AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "丙",
                document_start: start,
            });
            events.push(AdaptiveEvaluationEvent::ConfirmBoundary);
        }
        events.push(AdaptiveEvaluationEvent::QueryAndCommit {
            code: "aa",
            public_candidates: &public,
            selected_text: "丙",
            document_start: 2,
        });
        let limits = PendingSelectionLimits::new(32, 64).unwrap();
        let config = AdaptiveMergeConfig {
            max_coverage_probability: 0.1,
            max_merged_candidates: 2,
            ..AdaptiveMergeConfig::default()
        };

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.oov_recalled, 1);
        assert!(report.public_candidates_rank_worsened >= 1);
        assert!(report.public_rank_displacement_total >= 1);
        assert_eq!(report.coverage_probability_cap_violations, 0);
        assert!(report.maximum_coverage_probability_mass < config.max_coverage_probability);
        assert!(report.returned_personal_candidates >= 1);
        assert_eq!(report.nonselected_personal_candidates, 0);
    }

    #[test]
    fn report_separates_selected_personal_hits_from_other_personal_occupancy() {
        let public = public(&[("甲", 1.0)]);
        let events = [
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 0,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "乙",
                document_start: 1,
            },
            AdaptiveEvaluationEvent::ConfirmBoundary,
            AdaptiveEvaluationEvent::QueryAndCommit {
                code: "aa",
                public_candidates: &public,
                selected_text: "甲",
                document_start: 2,
            },
        ];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();

        assert_eq!(report.returned_personal_candidates, 1);
        assert_eq!(report.personal_candidates_in_top_5, 1);
        assert_eq!(report.nonselected_personal_candidates, 1);
        assert_eq!(report.nonselected_personal_candidates_in_top_5, 1);
    }

    #[test]
    fn malformed_stream_and_event_errors_are_redacted_and_indexed() {
        let (limits, config) = defaults();
        assert_eq!(
            evaluate_adaptive_closed_loop(&[], limits, config)
                .unwrap_err()
                .kind,
            AdaptiveEvaluationErrorKind::InvalidEventCount
        );

        let public = public(&[("公开甲", 1.0)]);
        let events = [AdaptiveEvaluationEvent::QueryAndCommit {
            code: "aa",
            public_candidates: &public,
            selected_text: "",
            document_start: 0,
        }];
        let error = evaluate_adaptive_closed_loop(&events, limits, config).unwrap_err();
        let debug = format!("{error:?}");

        assert_eq!(error.event_index, Some(0));
        assert!(matches!(error.kind, AdaptiveEvaluationErrorKind::Memory(_)));
        assert!(!debug.contains("公开甲"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn report_debug_is_aggregate_only() {
        let public = public(&[("公开甲", 1.0)]);
        let events = [AdaptiveEvaluationEvent::QueryAndCommit {
            code: "secretcode",
            public_candidates: &public,
            selected_text: "私密测试文字",
            document_start: 0,
        }];
        let (limits, config) = defaults();

        let report = evaluate_adaptive_closed_loop(&events, limits, config).unwrap();
        let debug = format!("{report:?}");

        assert!(!debug.contains("公开甲"));
        assert!(!debug.contains("私密测试文字"));
        assert!(!debug.contains("secretcode"));
    }
}
