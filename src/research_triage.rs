//! Evidence-bounded issue triage for continuous native-feedback snapshots.
//!
//! The classifier deliberately reports overlapping structural signals rather
//! than one invented root cause. Private codes and candidate text are used
//! only for in-memory frame matching and are never retained by the report.

use std::error::Error;
use std::fmt;

use crate::{
    NativeCandidateProvenance, NativeCandidateSuppressionAction, NativeCandidateView,
    NativeFeedbackEvent, WishCaptureScope, WishSnapshot, native_slow_key_remainder_ms,
};

/// One 60 Hz frame, used as the fixed threshold for visible latency signals.
pub const RESEARCH_TRIAGE_VISIBLE_LATENCY_MS: u32 = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchTriageCoverage {
    pub batches: usize,
    pub events: usize,
    pub omitted_events: usize,
    pub candidate_commits: usize,
    pub paired_candidate_commits: usize,
    pub unpaired_candidate_commits: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchCandidateReachabilitySignals {
    pub non_top_commits: usize,
    pub paged_commits: usize,
    pub raw_after_exhausted_frame: usize,
    pub raw_while_more_available: usize,
    pub cancellation_after_exhausted_frame: usize,
    pub cancellation_while_more_available: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchRankingSignals {
    pub precise_personalization_non_top_commits: usize,
    pub personalized_target_non_top_commits: usize,
    pub target_provenance_missing: usize,
    pub precise_ranking_non_top_commits: usize,
    pub reranked_top_bypassed_commits: usize,
    pub nonreranked_top_bypassed_commits: usize,
    pub top_provenance_missing: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchRecoverySignals {
    pub shape_lookup_commits: usize,
    pub tab_assisted_commits: usize,
    pub transposition_recovery_selected: usize,
    pub transposition_recovery_not_selected: usize,
    pub post_commit_backspaces_routed: usize,
    pub candidate_suppressions: usize,
    pub candidate_restores: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchSlowKeyPhaseSignals {
    pub refresh: usize,
    pub planning: usize,
    pub edit_session: usize,
    pub remainder: usize,
    pub tied: usize,
}

impl ResearchSlowKeyPhaseSignals {
    pub fn samples(self) -> usize {
        self.refresh + self.planning + self.edit_session + self.remainder + self.tied
    }

    fn observe(&mut self, refresh: u32, planning: u32, edit_session: u32, remainder: u32) {
        let phases = [refresh, planning, edit_session, remainder];
        let maximum = phases.into_iter().max().unwrap_or(0);
        if phases.iter().filter(|value| **value == maximum).count() != 1 {
            self.tied += 1;
            return;
        }
        match phases.iter().position(|value| *value == maximum) {
            Some(0) => self.refresh += 1,
            Some(1) => self.planning += 1,
            Some(2) => self.edit_session += 1,
            Some(3) => self.remainder += 1,
            _ => unreachable!("one of four phases must contain the unique maximum"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchLatencySignals {
    pub slow_initial_first_frames: usize,
    pub slow_initial_fully_visible_frames: usize,
    pub slow_updated_fully_visible_frames: usize,
    pub slow_key_paths: usize,
    pub dominant_phases: ResearchSlowKeyPhaseSignals,
}

/// Aggregate structural evidence. Every group may overlap with every other
/// group; none of the counts is an automatic correctness judgment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResearchIssueTriage {
    pub coverage: ResearchTriageCoverage,
    pub reachability: ResearchCandidateReachabilitySignals,
    pub ranking: ResearchRankingSignals,
    pub recovery: ResearchRecoverySignals,
    pub latency: ResearchLatencySignals,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchTriageError {
    NonContinuousSnapshot,
}

impl fmt::Display for ResearchTriageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("research triage requires continuous-journal snapshots")
    }
}

impl Error for ResearchTriageError {}

struct PresentedFrame {
    code: String,
    view: NativeCandidateView,
    page_start: usize,
    provenance: Vec<NativeCandidateProvenance>,
    global_top_provenance: Option<NativeCandidateProvenance>,
    recovery_text: Option<String>,
    tab_assembly: bool,
    may_have_more: bool,
}

struct PresentedFrameUpdate<'a> {
    code: &'a str,
    view: NativeCandidateView,
    page_start: usize,
    provenance: Vec<NativeCandidateProvenance>,
    recovery_text: Option<&'a str>,
    tab_assembly: bool,
    may_have_more: bool,
}

impl PresentedFrame {
    fn next(update: PresentedFrameUpdate<'_>, previous: Option<&Self>) -> Self {
        let matching_previous =
            previous.filter(|frame| frame.code == update.code && frame.view == update.view);
        let global_top_provenance = if update.page_start == 0 {
            update.provenance.first().copied()
        } else {
            matching_previous.and_then(|frame| frame.global_top_provenance)
        };
        let recovery_text = if update.page_start == 0 {
            update.recovery_text.map(str::to_owned)
        } else {
            update
                .recovery_text
                .map(str::to_owned)
                .or_else(|| matching_previous.and_then(|frame| frame.recovery_text.clone()))
        };
        Self {
            code: update.code.to_owned(),
            view: update.view,
            page_start: update.page_start,
            provenance: update.provenance,
            global_top_provenance,
            recovery_text,
            tab_assembly: update.tab_assembly,
            may_have_more: update.may_have_more,
        }
    }

    fn provenance_for_rank(&self, absolute_rank: usize) -> Option<NativeCandidateProvenance> {
        let index = absolute_rank.checked_sub(self.page_start.saturating_add(1))?;
        self.provenance.get(index).copied()
    }
}

pub fn analyze_research_issue_signals(
    snapshots: &[WishSnapshot],
) -> Result<ResearchIssueTriage, ResearchTriageError> {
    let mut report = ResearchIssueTriage::default();
    for snapshot in snapshots {
        if snapshot.capture_scope() != WishCaptureScope::ContinuousJournal {
            return Err(ResearchTriageError::NonContinuousSnapshot);
        }
        report.coverage.batches += 1;
        report.coverage.events += snapshot.events().len();
        report.coverage.omitted_events += snapshot
            .omitted_before_window()
            .saturating_add(snapshot.omitted_untimed())
            .saturating_add(snapshot.omitted_by_event_limit());
        observe_snapshot(&mut report, snapshot);
    }
    Ok(report)
}

fn observe_snapshot(report: &mut ResearchIssueTriage, snapshot: &WishSnapshot) {
    let precise_personalization = snapshot.supports_precise_candidate_personalization();
    let precise_ranking = snapshot.supports_precise_candidate_ranking_personalization();
    let mut frame: Option<PresentedFrame> = None;
    for wish_event in snapshot.events() {
        match wish_event.event() {
            NativeFeedbackEvent::CandidatesPresented {
                code,
                view,
                page_start,
                may_have_more,
                ..
            } => {
                frame = Some(PresentedFrame::next(
                    PresentedFrameUpdate {
                        code,
                        view: *view,
                        page_start: *page_start,
                        provenance: Vec::new(),
                        recovery_text: None,
                        tab_assembly: false,
                        may_have_more: *may_have_more,
                    },
                    frame.as_ref(),
                ));
            }
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                view,
                page_start,
                provenance,
                automatic_transposition,
                tab_assembly,
                may_have_more,
                ..
            } => {
                let recovery_text = automatic_transposition
                    .as_ref()
                    .and_then(|decision| decision.recovered_text());
                frame = Some(PresentedFrame::next(
                    PresentedFrameUpdate {
                        code,
                        view: *view,
                        page_start: *page_start,
                        provenance: provenance.clone(),
                        recovery_text,
                        tab_assembly: tab_assembly.is_some(),
                        may_have_more: *may_have_more,
                    },
                    frame.as_ref(),
                ));
            }
            NativeFeedbackEvent::CandidateCommitted {
                code,
                text,
                view,
                absolute_rank,
                ..
            } => {
                report.coverage.candidate_commits += 1;
                report.reachability.non_top_commits += usize::from(*absolute_rank > 1);
                report.recovery.shape_lookup_commits +=
                    usize::from(*view == NativeCandidateView::Shape);
                let matching = frame
                    .as_ref()
                    .filter(|frame| frame.code == *code && frame.view == *view);
                match matching {
                    Some(matching) => {
                        report.coverage.paired_candidate_commits += 1;
                        report.reachability.paged_commits += usize::from(matching.page_start > 0);
                        report.recovery.tab_assisted_commits += usize::from(
                            *view == NativeCandidateView::Shape && matching.tab_assembly,
                        );
                        if let Some(recovery) = matching.recovery_text.as_deref() {
                            if recovery == text {
                                report.recovery.transposition_recovery_selected += 1;
                            } else {
                                report.recovery.transposition_recovery_not_selected += 1;
                            }
                        }
                        if *absolute_rank > 1 {
                            observe_non_top_ranking(
                                report,
                                precise_personalization,
                                precise_ranking,
                                matching,
                                *absolute_rank,
                            );
                        }
                    }
                    None => report.coverage.unpaired_candidate_commits += 1,
                }
                frame = None;
            }
            NativeFeedbackEvent::RawCodeCommitted { code } => {
                if let Some(matching) = frame.as_ref().filter(|frame| frame.code == *code) {
                    if matching.may_have_more {
                        report.reachability.raw_while_more_available += 1;
                    } else {
                        report.reachability.raw_after_exhausted_frame += 1;
                    }
                    report.recovery.transposition_recovery_not_selected +=
                        usize::from(matching.recovery_text.is_some());
                }
                frame = None;
            }
            NativeFeedbackEvent::CompositionCancelled { code, .. } => {
                if let Some(matching) = frame.as_ref().filter(|frame| frame.code == *code) {
                    if matching.may_have_more {
                        report.reachability.cancellation_while_more_available += 1;
                    } else {
                        report.reachability.cancellation_after_exhausted_frame += 1;
                    }
                    report.recovery.transposition_recovery_not_selected +=
                        usize::from(matching.recovery_text.is_some());
                }
                frame = None;
            }
            NativeFeedbackEvent::CandidateSuppressionChanged { action, .. } => {
                match action {
                    NativeCandidateSuppressionAction::Suppress => {
                        report.recovery.candidate_suppressions += 1;
                    }
                    NativeCandidateSuppressionAction::Restore => {
                        report.recovery.candidate_restores += 1;
                    }
                }
                frame = None;
            }
            NativeFeedbackEvent::CandidatePopupTiming {
                first_frame_ms,
                fully_visible_ms,
                initial_show,
            } => {
                if *initial_show {
                    report.latency.slow_initial_first_frames +=
                        usize::from(*first_frame_ms >= RESEARCH_TRIAGE_VISIBLE_LATENCY_MS);
                    report.latency.slow_initial_fully_visible_frames +=
                        usize::from(*fully_visible_ms >= RESEARCH_TRIAGE_VISIBLE_LATENCY_MS);
                } else {
                    report.latency.slow_updated_fully_visible_frames +=
                        usize::from(*fully_visible_ms >= RESEARCH_TRIAGE_VISIBLE_LATENCY_MS);
                }
            }
            NativeFeedbackEvent::SlowKeyPathTiming {
                refresh_ms,
                planning_ms,
                edit_session_ms,
                total_ms,
            } => {
                if let Some(remainder) = native_slow_key_remainder_ms(
                    *refresh_ms,
                    *planning_ms,
                    *edit_session_ms,
                    *total_ms,
                ) {
                    report.latency.slow_key_paths += 1;
                    report.latency.dominant_phases.observe(
                        *refresh_ms,
                        *planning_ms,
                        *edit_session_ms,
                        remainder,
                    );
                }
            }
            NativeFeedbackEvent::PostCommitBackspaceRouted => {
                report.recovery.post_commit_backspaces_routed += 1;
            }
            NativeFeedbackEvent::PersonalPhraseAdjacencyObserved { .. } => {}
        }
    }
}

fn observe_non_top_ranking(
    report: &mut ResearchIssueTriage,
    precise_personalization: bool,
    precise_ranking: bool,
    frame: &PresentedFrame,
    absolute_rank: usize,
) {
    if precise_personalization {
        report.ranking.precise_personalization_non_top_commits += 1;
        match frame.provenance_for_rank(absolute_rank) {
            Some(target) => {
                report.ranking.personalized_target_non_top_commits +=
                    usize::from(!target.personalization().is_empty());
            }
            None => report.ranking.target_provenance_missing += 1,
        }
    }
    if precise_ranking {
        report.ranking.precise_ranking_non_top_commits += 1;
        match frame.global_top_provenance {
            Some(top) if top.ranking_personalization().is_empty() => {
                report.ranking.nonreranked_top_bypassed_commits += 1;
            }
            Some(_) => report.ranking.reranked_top_bypassed_commits += 1,
            None => report.ranking.top_provenance_missing += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FrozenNativeFeedbackSnapshot, NativeAutomaticTranspositionDecision,
        NativeAutomaticTranspositionOutcome, NativeAutomaticTranspositionTier,
        NativeCancellationSource, NativeCandidatePersonalization, NativeCandidateSource,
        NativeSelectionSource, NativeTabAssemblyState, WishCategory, WishJournalContext,
        WishJournalSpan,
    };

    fn snapshot(events: Vec<(u64, NativeFeedbackEvent)>) -> WishSnapshot {
        let marker = events.last().unwrap().0;
        let frozen = FrozenNativeFeedbackSnapshot::from_journal_events(marker, &events).unwrap();
        WishSnapshot::from_frozen_with_context(
            &frozen,
            WishCaptureScope::ContinuousJournal,
            WishCategory::Other,
            None,
            Some(WishJournalContext::ContinuousSpan(
                WishJournalSpan::new("12".repeat(32), 0, 0, None).unwrap(),
            )),
        )
        .unwrap()
    }

    fn provenance(
        source: NativeCandidateSource,
        evidence: NativeCandidatePersonalization,
        ranking: NativeCandidatePersonalization,
    ) -> NativeCandidateProvenance {
        NativeCandidateProvenance::with_personalization_and_ranking(source, evidence, ranking)
            .unwrap()
    }

    fn commit(
        code: &str,
        text: &str,
        view: NativeCandidateView,
        rank: usize,
    ) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidateCommitted {
            code: code.to_owned(),
            text: text.to_owned(),
            view,
            source: if rank == 1 {
                NativeSelectionSource::FirstCandidate
            } else {
                NativeSelectionSource::Numeric
            },
            absolute_rank: rank,
            visible_rank: rank.min(6),
        }
    }

    #[test]
    fn ranking_signals_distinguish_personal_target_evidence_from_a_reranked_blocker() {
        let report = analyze_research_issue_signals(&[snapshot(vec![
            (
                10,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "dago".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["大国".to_owned(), "打过".to_owned()],
                    provenance: vec![
                        provenance(
                            NativeCandidateSource::CoreExact,
                            NativeCandidatePersonalization::SESSION_EXACT,
                            NativeCandidatePersonalization::SESSION_EXACT,
                        ),
                        provenance(
                            NativeCandidateSource::CoreExact,
                            NativeCandidatePersonalization::PERSISTENT_EXACT,
                            NativeCandidatePersonalization::NONE,
                        ),
                    ],
                    automatic_transposition: None,
                    loaded_candidates: 2,
                    tab_assembly: None,
                    may_have_more: false,
                },
            ),
            (20, commit("dago", "打过", NativeCandidateView::Ordinary, 2)),
        ])])
        .unwrap();

        assert_eq!(report.coverage.candidate_commits, 1);
        assert_eq!(report.coverage.paired_candidate_commits, 1);
        assert_eq!(report.reachability.non_top_commits, 1);
        assert_eq!(report.ranking.precise_personalization_non_top_commits, 1);
        assert_eq!(report.ranking.personalized_target_non_top_commits, 1);
        assert_eq!(report.ranking.precise_ranking_non_top_commits, 1);
        assert_eq!(report.ranking.reranked_top_bypassed_commits, 1);
        assert_eq!(report.ranking.nonreranked_top_bypassed_commits, 0);
    }

    #[test]
    fn matching_pagination_carries_only_the_observed_global_top_evidence() {
        let core = NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false);
        let decoder = NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false);
        let report = analyze_research_issue_signals(&[snapshot(vec![
            (
                10,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "abcdef".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec![
                        "甲".to_owned(),
                        "乙".to_owned(),
                        "丙".to_owned(),
                        "丁".to_owned(),
                        "戊".to_owned(),
                        "己".to_owned(),
                    ],
                    provenance: vec![core, decoder, decoder, decoder, decoder, decoder],
                    automatic_transposition: None,
                    loaded_candidates: 7,
                    tab_assembly: None,
                    may_have_more: false,
                },
            ),
            (
                20,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "abcdef".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 6,
                    candidates: vec!["庚".to_owned()],
                    provenance: vec![provenance(
                        NativeCandidateSource::Decoder,
                        NativeCandidatePersonalization::PERSISTENT_EXACT,
                        NativeCandidatePersonalization::NONE,
                    )],
                    automatic_transposition: None,
                    loaded_candidates: 7,
                    tab_assembly: None,
                    may_have_more: false,
                },
            ),
            (
                30,
                NativeFeedbackEvent::CandidateCommitted {
                    code: "abcdef".to_owned(),
                    text: "庚".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::Numeric,
                    absolute_rank: 7,
                    visible_rank: 1,
                },
            ),
        ])])
        .unwrap();

        assert_eq!(report.reachability.paged_commits, 1);
        assert_eq!(report.ranking.personalized_target_non_top_commits, 1);
        assert_eq!(report.ranking.nonreranked_top_bypassed_commits, 1);
        assert_eq!(report.ranking.top_provenance_missing, 0);
    }

    #[test]
    fn lookup_recovery_and_latency_signals_stay_orthogonal() {
        let report = analyze_research_issue_signals(&[snapshot(vec![
            (
                10,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "fuem".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["什么".to_owned()],
                    provenance: vec![NativeCandidateProvenance::new(
                        NativeCandidateSource::TranspositionRecovery,
                        false,
                    )],
                    automatic_transposition: Some(NativeAutomaticTranspositionDecision::new_span(
                        0..2,
                        24,
                        NativeAutomaticTranspositionTier::Primary,
                        NativeAutomaticTranspositionTier::Primary,
                        NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                        Some("什么".to_owned()),
                        Some(1),
                    )),
                    loaded_candidates: 1,
                    tab_assembly: None,
                    may_have_more: false,
                },
            ),
            (20, commit("fuem", "什么", NativeCandidateView::Ordinary, 1)),
            (
                30,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "jdj".to_owned(),
                    view: NativeCandidateView::Shape,
                    page_start: 0,
                    candidates: vec!["讲".to_owned()],
                    provenance: vec![NativeCandidateProvenance::new(
                        NativeCandidateSource::Shape,
                        false,
                    )],
                    automatic_transposition: None,
                    loaded_candidates: 1,
                    tab_assembly: Some(NativeTabAssemblyState::new(1, 2, "h")),
                    may_have_more: false,
                },
            ),
            (40, commit("jdj", "讲", NativeCandidateView::Shape, 1)),
            (
                50,
                NativeFeedbackEvent::CandidatePopupTiming {
                    first_frame_ms: 18,
                    fully_visible_ms: 21,
                    initial_show: true,
                },
            ),
            (
                60,
                NativeFeedbackEvent::SlowKeyPathTiming {
                    refresh_ms: 2,
                    planning_ms: 11,
                    edit_session_ms: 1,
                    total_ms: 18,
                },
            ),
            (70, NativeFeedbackEvent::PostCommitBackspaceRouted),
        ])])
        .unwrap();

        assert_eq!(report.recovery.transposition_recovery_selected, 1);
        assert_eq!(report.recovery.shape_lookup_commits, 1);
        assert_eq!(report.recovery.tab_assisted_commits, 1);
        assert_eq!(report.recovery.post_commit_backspaces_routed, 1);
        assert_eq!(report.latency.slow_initial_first_frames, 1);
        assert_eq!(report.latency.slow_initial_fully_visible_frames, 1);
        assert_eq!(report.latency.slow_key_paths, 1);
        assert_eq!(report.latency.dominant_phases.planning, 1);
        assert_eq!(report.latency.dominant_phases.samples(), 1);
    }

    #[test]
    fn a_new_first_page_does_not_inherit_a_stale_transposition_offer() {
        let recovery = NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "fuem".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["什么".to_owned()],
            provenance: vec![NativeCandidateProvenance::new(
                NativeCandidateSource::TranspositionRecovery,
                false,
            )],
            automatic_transposition: Some(NativeAutomaticTranspositionDecision::new_span(
                0..2,
                24,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                Some("什么".to_owned()),
                Some(1),
            )),
            loaded_candidates: 1,
            tab_assembly: None,
            may_have_more: false,
        };
        let ordinary_update = NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "fuem".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["福恩".to_owned()],
            provenance: vec![NativeCandidateProvenance::new(
                NativeCandidateSource::Decoder,
                false,
            )],
            automatic_transposition: None,
            loaded_candidates: 1,
            tab_assembly: None,
            may_have_more: false,
        };
        let report = analyze_research_issue_signals(&[snapshot(vec![
            (10, recovery),
            (20, ordinary_update),
            (30, commit("fuem", "福恩", NativeCandidateView::Ordinary, 1)),
        ])])
        .unwrap();

        assert_eq!(report.recovery.transposition_recovery_selected, 0);
        assert_eq!(report.recovery.transposition_recovery_not_selected, 0);
    }

    #[test]
    fn exhausted_and_expandable_abandonment_are_not_called_missing_words() {
        let report = analyze_research_issue_signals(&[snapshot(vec![
            (
                10,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "aa".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["甲".to_owned()],
                    may_have_more: false,
                },
            ),
            (
                20,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "aa".to_owned(),
                },
            ),
            (
                30,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "bb".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["乙".to_owned()],
                    may_have_more: true,
                },
            ),
            (
                40,
                NativeFeedbackEvent::CompositionCancelled {
                    code: "bb".to_owned(),
                    source: NativeCancellationSource::Escape,
                },
            ),
            (50, commit("cc", "丙", NativeCandidateView::Ordinary, 1)),
        ])])
        .unwrap();

        assert_eq!(report.reachability.raw_after_exhausted_frame, 1);
        assert_eq!(report.reachability.cancellation_while_more_available, 1);
        assert_eq!(report.coverage.unpaired_candidate_commits, 1);
        assert_eq!(report.reachability.paged_commits, 0);
    }

    #[test]
    fn non_continuous_snapshots_are_rejected_instead_of_silently_mixed() {
        let events = vec![(10, commit("aa", "甲", NativeCandidateView::Ordinary, 1))];
        let frozen = FrozenNativeFeedbackSnapshot::from_journal_events(10, &events).unwrap();
        let wish = WishSnapshot::from_frozen_with_metadata(
            &frozen,
            WishCaptureScope::RecentWindow,
            WishCategory::Other,
        )
        .unwrap();
        assert_eq!(
            analyze_research_issue_signals(&[wish]),
            Err(ResearchTriageError::NonContinuousSnapshot)
        );
    }
}
