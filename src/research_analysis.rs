//! Read-only reconstruction of linked continuous-feedback scenes.
//!
//! Storage batches are deliberately not semantic boundaries. V8 journal spans
//! provide one process-local stream and event ordinal, while explicit wishes
//! may anchor the nearest preceding completed scene. Private strings remain in
//! non-`Debug` report types and are never persisted by this module.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::{
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS, NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS,
    NativeAutomaticTranspositionOutcome, NativeCancellationSource, NativeCandidateSource,
    NativeCandidateView, NativeFeedbackEvent, WishJournalContext, WishRuntimeIdentity,
    WishSnapshot,
};

const DEFAULT_SCENE_GAP_MS: u64 = 45_000;
const MIN_SCENE_GAP_MS: u64 = 20_000;
const MAX_SCENE_GAP_MS: u64 = 90_000;
const MAX_SCENE_EPISODES: usize = 128;
const MAX_SCENE_SPAN_MS: u64 = 10 * 60_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchHabitKind {
    AcceptedTransposition,
    RepeatedCodeRevision,
}

/// One private, evidence-bounded hand-habit clue.
///
/// This type intentionally omits `Debug` because it contains real codes and
/// committed text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchHabitClue {
    kind: ResearchHabitKind,
    observed_code: String,
    resulting_code: String,
    committed_text: String,
    observations: usize,
    median_pair_gap_ms: Option<u32>,
}

impl ResearchHabitClue {
    pub fn kind(&self) -> ResearchHabitKind {
        self.kind
    }

    pub fn observed_code(&self) -> &str {
        &self.observed_code
    }

    pub fn resulting_code(&self) -> &str {
        &self.resulting_code
    }

    pub fn committed_text(&self) -> &str {
        &self.committed_text
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn median_pair_gap_ms(&self) -> Option<u32> {
        self.median_pair_gap_ms
    }
}

/// Aggregate scene reconstruction plus private habit clues.
///
/// This type intentionally omits `Debug` because `habit_clues` contains real
/// input and committed text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchSceneAnalysis {
    linked_batches: usize,
    linked_streams: usize,
    chain_breaks: usize,
    episodes: usize,
    scenes: usize,
    gap_threshold_ms: u64,
    median_episodes_per_scene: usize,
    median_scene_duration_ms: u64,
    anchored_wishes: usize,
    linked_wishes: usize,
    unanchored_wishes: usize,
    habit_clues: Vec<ResearchHabitClue>,
}

/// Aggregate, text-free evidence for an odd double-pinyin frame followed by
/// its completed even frame under one immutable runtime identity.
///
/// Candidate strings are compared only while reconstructing the stream. They
/// are not retained in this report, so callers can safely render the counts
/// without disclosing private input or candidate text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResearchHalfPairAnalysis {
    linked_batches: usize,
    linked_streams: usize,
    chain_breaks: usize,
    paired_frames: usize,
    gap_histogram: [usize; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
    top_candidate_comparisons: usize,
    top_candidate_changes: usize,
    candidate_slots_before: usize,
    retained_candidates: usize,
    provenance_comparisons: usize,
    decoder_top_after_completion: usize,
}

impl ResearchHalfPairAnalysis {
    pub fn linked_batches(&self) -> usize {
        self.linked_batches
    }

    pub fn linked_streams(&self) -> usize {
        self.linked_streams
    }

    pub fn chain_breaks(&self) -> usize {
        self.chain_breaks
    }

    pub fn paired_frames(&self) -> usize {
        self.paired_frames
    }

    pub fn gap_histogram(&self) -> &[usize; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS] {
        &self.gap_histogram
    }

    pub fn top_candidate_comparisons(&self) -> usize {
        self.top_candidate_comparisons
    }

    pub fn top_candidate_changes(&self) -> usize {
        self.top_candidate_changes
    }

    pub fn candidate_slots_before(&self) -> usize {
        self.candidate_slots_before
    }

    pub fn retained_candidates(&self) -> usize {
        self.retained_candidates
    }

    pub fn provenance_comparisons(&self) -> usize {
        self.provenance_comparisons
    }

    pub fn decoder_top_after_completion(&self) -> usize {
        self.decoder_top_after_completion
    }

    /// Number of paired frames whose completed frame arrived before the
    /// supplied exclusive upper bound. The bound must be one of the stable
    /// feedback bucket boundaries.
    pub fn completed_before_ms(&self, exclusive_upper_bound_ms: u64) -> Option<usize> {
        let last_bucket = NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS
            .iter()
            .position(|bound| *bound == exclusive_upper_bound_ms)?;
        Some(self.gap_histogram[..=last_bucket].iter().sum())
    }
}

impl ResearchSceneAnalysis {
    pub fn linked_batches(&self) -> usize {
        self.linked_batches
    }

    pub fn linked_streams(&self) -> usize {
        self.linked_streams
    }

    pub fn chain_breaks(&self) -> usize {
        self.chain_breaks
    }

    pub fn episodes(&self) -> usize {
        self.episodes
    }

    pub fn scenes(&self) -> usize {
        self.scenes
    }

    pub fn gap_threshold_ms(&self) -> u64 {
        self.gap_threshold_ms
    }

    pub fn median_episodes_per_scene(&self) -> usize {
        self.median_episodes_per_scene
    }

    pub fn median_scene_duration_ms(&self) -> u64 {
        self.median_scene_duration_ms
    }

    pub fn anchored_wishes(&self) -> usize {
        self.anchored_wishes
    }

    pub fn linked_wishes(&self) -> usize {
        self.linked_wishes
    }

    pub fn unanchored_wishes(&self) -> usize {
        self.unanchored_wishes
    }

    pub fn habit_clues(&self) -> &[ResearchHabitClue] {
        &self.habit_clues
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchSceneError {
    DuplicateBatchSequence,
    InvalidBatchTimeline,
    EventOrdinalOverflow,
}

impl fmt::Display for ResearchSceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateBatchSequence => "linked research stream repeats a batch sequence",
            Self::InvalidBatchTimeline => "linked research batch timeline is invalid",
            Self::EventOrdinalOverflow => "linked research event ordinal overflowed",
        })
    }
}

impl Error for ResearchSceneError {}

struct LinkedBatch<'a> {
    sequence: u64,
    first_ordinal: u64,
    previous_gap_ms: Option<u64>,
    snapshot: &'a WishSnapshot,
}

#[derive(Clone)]
struct RecoveryEvidence {
    code: String,
    text: String,
    pair_gap_ms: u32,
}

struct PendingEpisode {
    start_ms: u64,
    first_ordinal: u64,
    previous_code: Option<String>,
    revision_from: Option<String>,
    recovery: Option<RecoveryEvidence>,
}

impl PendingEpisode {
    fn new(at_ms: u64, ordinal: u64) -> Self {
        Self {
            start_ms: at_ms,
            first_ordinal: ordinal,
            previous_code: None,
            revision_from: None,
            recovery: None,
        }
    }

    fn observe_candidates(&mut self, event: &NativeFeedbackEvent) {
        let NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code,
            automatic_transposition,
            ..
        } = event
        else {
            if let NativeFeedbackEvent::CandidatesPresented { code, .. } = event {
                self.observe_code(code);
            }
            return;
        };
        self.observe_code(code);
        if let Some(decision) = automatic_transposition
            && decision.outcome() == NativeAutomaticTranspositionOutcome::RecoveryAvailable
            && decision.visible_rank().is_some()
            && let Some(text) = decision.recovered_text()
        {
            self.recovery = Some(RecoveryEvidence {
                code: code.clone(),
                text: text.to_owned(),
                pair_gap_ms: decision.pair_gap_ms(),
            });
        }
    }

    fn observe_code(&mut self, code: &str) {
        if let Some(previous) = self.previous_code.as_deref()
            && !code.starts_with(previous)
            && self.revision_from.is_none()
        {
            self.revision_from = Some(previous.to_owned());
        }
        self.previous_code = Some(code.to_owned());
    }
}

struct Episode {
    chain: usize,
    start_ms: u64,
    end_ms: u64,
    first_ordinal: u64,
    last_ordinal: u64,
    hard_boundary_after: bool,
    accepted_recovery: Option<RecoveryEvidence>,
    revision: Option<(String, String, String)>,
}

struct Scene {
    stream_id: String,
    chain: usize,
    start_ms: u64,
    end_ms: u64,
    first_ordinal: u64,
    last_ordinal: u64,
    episodes: usize,
    wishes: usize,
}

#[derive(Hash, Eq, PartialEq)]
struct RecoveryKey {
    code: String,
    text: String,
}

#[derive(Hash, Eq, PartialEq)]
struct RevisionKey {
    from: String,
    to: String,
    text: String,
}

/// Reconstructs process-local natural scenes without treating encrypted file
/// boundaries as semantic breaks. Old unlinked snapshots remain readable but
/// are intentionally excluded from scene claims.
pub fn analyze_linked_research(
    research: &[WishSnapshot],
    wishes: &[WishSnapshot],
) -> Result<ResearchSceneAnalysis, ResearchSceneError> {
    let mut streams: HashMap<String, Vec<LinkedBatch<'_>>> = HashMap::new();
    let mut linked_batches = 0;
    for snapshot in research {
        let Some(WishJournalContext::ContinuousSpan(span)) = snapshot.journal_context() else {
            continue;
        };
        linked_batches += 1;
        streams
            .entry(span.stream_id().to_owned())
            .or_default()
            .push(LinkedBatch {
                sequence: span.batch_sequence(),
                first_ordinal: span.first_event_ordinal(),
                previous_gap_ms: span.previous_event_gap_ms(),
                snapshot,
            });
    }

    let mut anchors: HashMap<String, Vec<u64>> = HashMap::new();
    let mut unanchored_wishes = 0;
    for wish in wishes {
        if let Some(WishJournalContext::WishAnchor(anchor)) = wish.journal_context() {
            anchors
                .entry(anchor.stream_id().to_owned())
                .or_default()
                .push(anchor.event_ordinal());
        } else {
            unanchored_wishes += 1;
        }
    }
    let anchored_wishes = anchors.values().map(Vec::len).sum();

    let mut stream_episodes: HashMap<String, Vec<Episode>> = HashMap::new();
    let mut chain_breaks = 0;
    for (stream_id, batches) in &mut streams {
        batches.sort_by_key(|batch| batch.sequence);
        if batches
            .windows(2)
            .any(|pair| pair[0].sequence == pair[1].sequence)
        {
            return Err(ResearchSceneError::DuplicateBatchSequence);
        }
        let (episodes, breaks) = reconstruct_stream(batches)?;
        chain_breaks += breaks;
        stream_episodes.insert(stream_id.clone(), episodes);
    }

    let gaps = stream_episodes
        .values()
        .flat_map(|episodes| {
            episodes
                .windows(2)
                .filter(|pair| pair[0].chain == pair[1].chain)
                .map(|pair| pair[1].start_ms.saturating_sub(pair[0].end_ms))
        })
        .collect::<Vec<_>>();
    let gap_threshold_ms = adaptive_scene_gap(&gaps);

    let mut scenes = Vec::new();
    for (stream_id, episodes) in &stream_episodes {
        build_scenes(stream_id, episodes, gap_threshold_ms, &mut scenes);
    }

    let mut linked_wishes = 0;
    for (stream_id, ordinals) in &anchors {
        let mut matching = scenes
            .iter_mut()
            .filter(|scene| scene.stream_id == *stream_id)
            .collect::<Vec<_>>();
        matching.sort_by_key(|scene| scene.last_ordinal);
        for ordinal in ordinals {
            if let Some(scene) = matching
                .iter_mut()
                .rev()
                .find(|scene| scene.first_ordinal <= *ordinal)
            {
                scene.wishes += 1;
                linked_wishes += 1;
            }
        }
    }

    let habit_clues = collect_habit_clues(stream_episodes.values().flatten());
    let mut episode_counts = scenes
        .iter()
        .map(|scene| scene.episodes)
        .collect::<Vec<_>>();
    let mut durations = scenes
        .iter()
        .map(|scene| scene.end_ms.saturating_sub(scene.start_ms))
        .collect::<Vec<_>>();
    episode_counts.sort_unstable();
    durations.sort_unstable();
    let episodes = stream_episodes.values().map(Vec::len).sum();
    Ok(ResearchSceneAnalysis {
        linked_batches,
        linked_streams: streams.len(),
        chain_breaks,
        episodes,
        scenes: scenes.len(),
        gap_threshold_ms,
        median_episodes_per_scene: nearest_rank(&episode_counts, 50).unwrap_or(0),
        median_scene_duration_ms: nearest_rank(&durations, 50).unwrap_or(0),
        anchored_wishes,
        linked_wishes,
        unanchored_wishes,
        habit_clues,
    })
}

struct PendingHalfPairFrame {
    code: String,
    at_ms: u64,
    candidates: Vec<String>,
}

#[derive(Default)]
struct HalfPairStreamAnalysis {
    pending: Option<PendingHalfPairFrame>,
    paired_frames: usize,
    gap_histogram: [usize; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
    top_candidate_comparisons: usize,
    top_candidate_changes: usize,
    candidate_slots_before: usize,
    retained_candidates: usize,
    provenance_comparisons: usize,
    decoder_top_after_completion: usize,
}

impl HalfPairStreamAnalysis {
    fn clear_pending(&mut self) {
        self.pending = None;
    }

    fn observe(&mut self, at_ms: u64, event: &NativeFeedbackEvent) {
        // These diagnostics may be recorded between the semantic candidate
        // frames and therefore do not break an otherwise adjacent pair.
        if matches!(
            event,
            NativeFeedbackEvent::CandidatePopupTiming { .. }
                | NativeFeedbackEvent::SlowKeyPathTiming { .. }
                | NativeFeedbackEvent::PostCommitBackspaceRouted
        ) {
            return;
        }

        let (code, candidates, provenance) = match event {
            NativeFeedbackEvent::CandidatesPresented {
                code,
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates,
                ..
            } => (code, candidates, None),
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates,
                provenance,
                ..
            } => (code, candidates, Some(provenance.as_slice())),
            _ => {
                self.clear_pending();
                return;
            }
        };

        if code.len() % 2 == 1 {
            let repeats_same_frame = self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.code == *code);
            self.pending = (!repeats_same_frame).then(|| PendingHalfPairFrame {
                code: code.clone(),
                at_ms,
                candidates: candidates.clone(),
            });
            return;
        }

        let Some(pending) = self.pending.take() else {
            return;
        };
        if code.len() != pending.code.len().saturating_add(1) || !code.starts_with(&pending.code) {
            return;
        }
        let Some(gap_ms) = at_ms.checked_sub(pending.at_ms) else {
            return;
        };
        let bucket = NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS
            .iter()
            .position(|upper_bound| gap_ms < *upper_bound)
            .unwrap_or(NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS - 1);
        self.paired_frames = self.paired_frames.saturating_add(1);
        self.gap_histogram[bucket] = self.gap_histogram[bucket].saturating_add(1);

        if let (Some(odd_top), Some(even_top)) = (pending.candidates.first(), candidates.first()) {
            self.top_candidate_comparisons = self.top_candidate_comparisons.saturating_add(1);
            self.top_candidate_changes = self
                .top_candidate_changes
                .saturating_add(usize::from(odd_top != even_top));
        }
        self.candidate_slots_before = self
            .candidate_slots_before
            .saturating_add(pending.candidates.len());
        self.retained_candidates = self.retained_candidates.saturating_add(
            pending
                .candidates
                .iter()
                .filter(|candidate| candidates.contains(candidate))
                .count(),
        );
        if let Some(provenance) = provenance
            && let Some(top) = provenance.first()
        {
            self.provenance_comparisons = self.provenance_comparisons.saturating_add(1);
            self.decoder_top_after_completion = self
                .decoder_top_after_completion
                .saturating_add(usize::from(top.source() == NativeCandidateSource::Decoder));
        }
    }
}

/// Reconstructs text-free odd-to-even candidate-frame evidence for one
/// versioned runtime. Unlinked snapshots and discontinuous journal spans are
/// excluded from cross-batch claims rather than guessed together.
pub fn analyze_runtime_half_pairs(
    research: &[WishSnapshot],
    identity: &WishRuntimeIdentity,
) -> Result<ResearchHalfPairAnalysis, ResearchSceneError> {
    let mut streams: HashMap<String, Vec<LinkedBatch<'_>>> = HashMap::new();
    let mut linked_batches = 0;
    for snapshot in research
        .iter()
        .filter(|snapshot| snapshot.runtime_identity() == Some(identity))
    {
        let Some(WishJournalContext::ContinuousSpan(span)) = snapshot.journal_context() else {
            continue;
        };
        linked_batches += 1;
        streams
            .entry(span.stream_id().to_owned())
            .or_default()
            .push(LinkedBatch {
                sequence: span.batch_sequence(),
                first_ordinal: span.first_event_ordinal(),
                previous_gap_ms: span.previous_event_gap_ms(),
                snapshot,
            });
    }

    let linked_streams = streams.len();
    let mut chain_breaks = 0_usize;
    let mut aggregate = HalfPairStreamAnalysis::default();
    for batches in streams.values_mut() {
        batches.sort_by_key(|batch| batch.sequence);
        if batches
            .windows(2)
            .any(|pair| pair[0].sequence == pair[1].sequence)
        {
            return Err(ResearchSceneError::DuplicateBatchSequence);
        }
        let mut stream = HalfPairStreamAnalysis::default();
        let mut previous_sequence = None;
        let mut expected_ordinal = None;
        let mut previous_end_ms = 0_u64;
        for batch in batches {
            let continuous = previous_sequence.is_some_and(|sequence: u64| {
                sequence.checked_add(1) == Some(batch.sequence)
                    && expected_ordinal == Some(batch.first_ordinal)
            });
            if previous_sequence.is_some() && !continuous {
                chain_breaks = chain_breaks.saturating_add(1);
                stream.clear_pending();
            }
            let base_ms = if continuous {
                previous_end_ms
                    .checked_add(
                        batch
                            .previous_gap_ms
                            .ok_or(ResearchSceneError::InvalidBatchTimeline)?,
                    )
                    .ok_or(ResearchSceneError::InvalidBatchTimeline)?
            } else {
                0
            };
            let first_age = batch
                .snapshot
                .events()
                .first()
                .ok_or(ResearchSceneError::InvalidBatchTimeline)?
                .milliseconds_before_marker();
            let mut batch_end_ms = base_ms;
            for wish_event in batch.snapshot.events() {
                let offset = first_age
                    .checked_sub(wish_event.milliseconds_before_marker())
                    .ok_or(ResearchSceneError::InvalidBatchTimeline)?;
                let at_ms = base_ms
                    .checked_add(u64::from(offset))
                    .ok_or(ResearchSceneError::InvalidBatchTimeline)?;
                batch_end_ms = at_ms;
                stream.observe(at_ms, wish_event.event());
            }
            previous_sequence = Some(batch.sequence);
            expected_ordinal = Some(
                batch
                    .first_ordinal
                    .checked_add(
                        u64::try_from(batch.snapshot.events().len())
                            .map_err(|_| ResearchSceneError::EventOrdinalOverflow)?,
                    )
                    .ok_or(ResearchSceneError::EventOrdinalOverflow)?,
            );
            previous_end_ms = batch_end_ms;
        }

        aggregate.paired_frames = aggregate.paired_frames.saturating_add(stream.paired_frames);
        for (target, source) in aggregate.gap_histogram.iter_mut().zip(stream.gap_histogram) {
            *target = target.saturating_add(source);
        }
        aggregate.top_candidate_comparisons = aggregate
            .top_candidate_comparisons
            .saturating_add(stream.top_candidate_comparisons);
        aggregate.top_candidate_changes = aggregate
            .top_candidate_changes
            .saturating_add(stream.top_candidate_changes);
        aggregate.candidate_slots_before = aggregate
            .candidate_slots_before
            .saturating_add(stream.candidate_slots_before);
        aggregate.retained_candidates = aggregate
            .retained_candidates
            .saturating_add(stream.retained_candidates);
        aggregate.provenance_comparisons = aggregate
            .provenance_comparisons
            .saturating_add(stream.provenance_comparisons);
        aggregate.decoder_top_after_completion = aggregate
            .decoder_top_after_completion
            .saturating_add(stream.decoder_top_after_completion);
    }

    Ok(ResearchHalfPairAnalysis {
        linked_batches,
        linked_streams,
        chain_breaks,
        paired_frames: aggregate.paired_frames,
        gap_histogram: aggregate.gap_histogram,
        top_candidate_comparisons: aggregate.top_candidate_comparisons,
        top_candidate_changes: aggregate.top_candidate_changes,
        candidate_slots_before: aggregate.candidate_slots_before,
        retained_candidates: aggregate.retained_candidates,
        provenance_comparisons: aggregate.provenance_comparisons,
        decoder_top_after_completion: aggregate.decoder_top_after_completion,
    })
}

fn reconstruct_stream(
    batches: &[LinkedBatch<'_>],
) -> Result<(Vec<Episode>, usize), ResearchSceneError> {
    let mut episodes = Vec::new();
    let mut pending = None;
    let mut previous_sequence = None;
    let mut expected_ordinal = None;
    let mut previous_end_ms = 0_u64;
    let mut chain = 0_usize;
    let mut chain_breaks = 0_usize;

    for batch in batches {
        let continuous = previous_sequence.is_some_and(|sequence: u64| {
            sequence.checked_add(1) == Some(batch.sequence)
                && expected_ordinal == Some(batch.first_ordinal)
        });
        if previous_sequence.is_some() && !continuous {
            chain = chain.saturating_add(1);
            chain_breaks = chain_breaks.saturating_add(1);
            pending = None;
        }
        let base_ms = if continuous {
            previous_end_ms
                .checked_add(
                    batch
                        .previous_gap_ms
                        .ok_or(ResearchSceneError::InvalidBatchTimeline)?,
                )
                .ok_or(ResearchSceneError::InvalidBatchTimeline)?
        } else {
            0
        };
        let first_age = batch
            .snapshot
            .events()
            .first()
            .ok_or(ResearchSceneError::InvalidBatchTimeline)?
            .milliseconds_before_marker();
        let mut batch_end_ms = base_ms;
        for (index, wish_event) in batch.snapshot.events().iter().enumerate() {
            let offset = first_age
                .checked_sub(wish_event.milliseconds_before_marker())
                .ok_or(ResearchSceneError::InvalidBatchTimeline)?;
            let at_ms = base_ms
                .checked_add(u64::from(offset))
                .ok_or(ResearchSceneError::InvalidBatchTimeline)?;
            batch_end_ms = at_ms;
            let ordinal = batch
                .first_ordinal
                .checked_add(
                    u64::try_from(index).map_err(|_| ResearchSceneError::EventOrdinalOverflow)?,
                )
                .ok_or(ResearchSceneError::EventOrdinalOverflow)?;
            observe_event(
                &mut pending,
                &mut episodes,
                chain,
                at_ms,
                ordinal,
                wish_event.event(),
            );
        }
        previous_sequence = Some(batch.sequence);
        expected_ordinal = Some(
            batch
                .first_ordinal
                .checked_add(
                    u64::try_from(batch.snapshot.events().len())
                        .map_err(|_| ResearchSceneError::EventOrdinalOverflow)?,
                )
                .ok_or(ResearchSceneError::EventOrdinalOverflow)?,
        );
        previous_end_ms = batch_end_ms;
    }
    Ok((episodes, chain_breaks))
}

fn observe_event(
    pending: &mut Option<PendingEpisode>,
    episodes: &mut Vec<Episode>,
    chain: usize,
    at_ms: u64,
    ordinal: u64,
    event: &NativeFeedbackEvent,
) {
    match event {
        NativeFeedbackEvent::CandidatesPresented { .. }
        | NativeFeedbackEvent::CandidatesPresentedWithProvenance { .. } => {
            let episode = pending.get_or_insert_with(|| PendingEpisode::new(at_ms, ordinal));
            episode.observe_candidates(event);
        }
        NativeFeedbackEvent::CandidatePopupTiming { .. }
        | NativeFeedbackEvent::SlowKeyPathTiming { .. }
        | NativeFeedbackEvent::PostCommitBackspaceRouted
        | NativeFeedbackEvent::CandidateSuppressionChanged { .. }
        | NativeFeedbackEvent::PersonalPhraseAdjacencyObserved { .. } => {}
        NativeFeedbackEvent::CandidateCommitted { code, text, .. } => {
            let mut episode = pending
                .take()
                .unwrap_or_else(|| PendingEpisode::new(at_ms, ordinal));
            episode.observe_code(code);
            let accepted_recovery = episode
                .recovery
                .filter(|recovery| recovery.code == *code && recovery.text == *text);
            let revision = episode
                .revision_from
                .filter(|from| from != code)
                .map(|from| (from, code.clone(), text.clone()));
            episodes.push(Episode {
                chain,
                start_ms: episode.start_ms,
                end_ms: at_ms,
                first_ordinal: episode.first_ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: false,
                accepted_recovery,
                revision,
            });
        }
        NativeFeedbackEvent::RawCodeCommitted { code } => {
            let mut episode = pending
                .take()
                .unwrap_or_else(|| PendingEpisode::new(at_ms, ordinal));
            episode.observe_code(code);
            episodes.push(Episode {
                chain,
                start_ms: episode.start_ms,
                end_ms: at_ms,
                first_ordinal: episode.first_ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: false,
                accepted_recovery: None,
                revision: None,
            });
        }
        NativeFeedbackEvent::CompositionCancelled { code, source } => {
            let mut episode = pending
                .take()
                .unwrap_or_else(|| PendingEpisode::new(at_ms, ordinal));
            episode.observe_code(code);
            episodes.push(Episode {
                chain,
                start_ms: episode.start_ms,
                end_ms: at_ms,
                first_ordinal: episode.first_ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: matches!(
                    source,
                    NativeCancellationSource::FocusLoss | NativeCancellationSource::HostTermination
                ),
                accepted_recovery: None,
                revision: None,
            });
        }
    }
}

fn adaptive_scene_gap(gaps: &[u64]) -> u64 {
    if gaps.len() < 8 {
        return DEFAULT_SCENE_GAP_MS;
    }
    let mut sorted = gaps.to_vec();
    sorted.sort_unstable();
    nearest_rank(&sorted, 90)
        .unwrap_or(DEFAULT_SCENE_GAP_MS)
        .saturating_mul(2)
        .clamp(MIN_SCENE_GAP_MS, MAX_SCENE_GAP_MS)
}

fn build_scenes(
    stream_id: &str,
    episodes: &[Episode],
    gap_threshold_ms: u64,
    output: &mut Vec<Scene>,
) {
    let mut current: Option<Scene> = None;
    let mut previous_hard_boundary = false;
    for episode in episodes {
        let must_split = current.as_ref().is_some_and(|scene| {
            scene.chain != episode.chain
                || previous_hard_boundary
                || episode.start_ms.saturating_sub(scene.end_ms) > gap_threshold_ms
                || scene.episodes >= MAX_SCENE_EPISODES
                || episode.start_ms.saturating_sub(scene.start_ms) > MAX_SCENE_SPAN_MS
        });
        if must_split && let Some(scene) = current.take() {
            output.push(scene);
        }
        let scene = current.get_or_insert_with(|| Scene {
            stream_id: stream_id.to_owned(),
            chain: episode.chain,
            start_ms: episode.start_ms,
            end_ms: episode.end_ms,
            first_ordinal: episode.first_ordinal,
            last_ordinal: episode.last_ordinal,
            episodes: 0,
            wishes: 0,
        });
        scene.end_ms = episode.end_ms;
        scene.last_ordinal = episode.last_ordinal;
        scene.episodes = scene.episodes.saturating_add(1);
        previous_hard_boundary = episode.hard_boundary_after;
    }
    if let Some(scene) = current {
        output.push(scene);
    }
}

fn collect_habit_clues<'a>(episodes: impl Iterator<Item = &'a Episode>) -> Vec<ResearchHabitClue> {
    let mut recoveries: HashMap<RecoveryKey, Vec<u32>> = HashMap::new();
    let mut revisions: HashMap<RevisionKey, usize> = HashMap::new();
    for episode in episodes {
        if let Some(recovery) = &episode.accepted_recovery {
            recoveries
                .entry(RecoveryKey {
                    code: recovery.code.clone(),
                    text: recovery.text.clone(),
                })
                .or_default()
                .push(recovery.pair_gap_ms);
        }
        if let Some((from, to, text)) = &episode.revision {
            *revisions
                .entry(RevisionKey {
                    from: from.clone(),
                    to: to.clone(),
                    text: text.clone(),
                })
                .or_insert(0) += 1;
        }
    }
    let mut clues = Vec::new();
    for (key, mut gaps) in recoveries {
        gaps.sort_unstable();
        clues.push(ResearchHabitClue {
            kind: ResearchHabitKind::AcceptedTransposition,
            observed_code: key.code.clone(),
            resulting_code: key.code,
            committed_text: key.text,
            observations: gaps.len(),
            median_pair_gap_ms: nearest_rank(&gaps, 50),
        });
    }
    for (key, observations) in revisions {
        if observations < 2 {
            continue;
        }
        clues.push(ResearchHabitClue {
            kind: ResearchHabitKind::RepeatedCodeRevision,
            observed_code: key.from,
            resulting_code: key.to,
            committed_text: key.text,
            observations,
            median_pair_gap_ms: None,
        });
    }
    clues.sort_by(|left, right| {
        right
            .observations
            .cmp(&left.observations)
            .then_with(|| left.observed_code.cmp(&right.observed_code))
            .then_with(|| left.committed_text.cmp(&right.committed_text))
    });
    clues
}

fn nearest_rank<T: Copy>(values: &[T], percentile: usize) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    let rank = values.len().saturating_mul(percentile).saturating_add(99) / 100;
    values
        .get(rank.saturating_sub(1).min(values.len() - 1))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FrozenNativeFeedbackSnapshot, NativeAutomaticTranspositionDecision,
        NativeAutomaticTranspositionTier, NativeCandidateProvenance, NativeCandidateSource,
        NativeCandidateView, NativeFeedbackFreezeAuthorization, NativeFeedbackSession,
        NativePersonalPhraseAdjacency, NativeSelectionSource, WishCaptureScope, WishCategory,
        WishJournalAnchor, WishJournalSpan,
    };

    fn snapshot(
        stream: &str,
        sequence: u64,
        first_ordinal: u64,
        previous_gap_ms: Option<u64>,
        events: Vec<(u64, NativeFeedbackEvent)>,
    ) -> WishSnapshot {
        snapshot_with_identity(
            stream,
            sequence,
            first_ordinal,
            previous_gap_ms,
            None,
            events,
        )
    }

    fn snapshot_with_identity(
        stream: &str,
        sequence: u64,
        first_ordinal: u64,
        previous_gap_ms: Option<u64>,
        runtime_identity: Option<WishRuntimeIdentity>,
        events: Vec<(u64, NativeFeedbackEvent)>,
    ) -> WishSnapshot {
        let marker = events.last().unwrap().0;
        let frozen = FrozenNativeFeedbackSnapshot::from_journal_events(marker, &events).unwrap();
        WishSnapshot::from_frozen_with_context(
            &frozen,
            WishCaptureScope::ContinuousJournal,
            WishCategory::Other,
            runtime_identity,
            Some(WishJournalContext::ContinuousSpan(
                WishJournalSpan::new(stream.to_owned(), sequence, first_ordinal, previous_gap_ms)
                    .unwrap(),
            )),
        )
        .unwrap()
    }

    fn committed(code: &str, text: &str) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidateCommitted {
            code: code.to_owned(),
            text: text.to_owned(),
            view: NativeCandidateView::Ordinary,
            source: NativeSelectionSource::FirstCandidate,
            absolute_rank: 1,
            visible_rank: 1,
        }
    }

    fn runtime_identity(digit: char) -> WishRuntimeIdentity {
        WishRuntimeIdentity::new(
            digit.to_string().repeat(64),
            "test-core-v1".to_owned(),
            Some("test-supplement-v1".to_owned()),
        )
        .unwrap()
    }

    fn frame(
        code: &str,
        candidates: &[&str],
        sources: &[NativeCandidateSource],
    ) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: code.to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: candidates.iter().map(|text| (*text).to_owned()).collect(),
            provenance: sources
                .iter()
                .map(|source| NativeCandidateProvenance::new(*source, false))
                .collect(),
            automatic_transposition: None,
            loaded_candidates: candidates.len(),
            tab_assembly: None,
            may_have_more: false,
        }
    }

    #[test]
    fn storage_batches_and_followup_metadata_join_before_adaptive_scene_segmentation() {
        let stream = "12".repeat(32);
        let first = snapshot(
            &stream,
            0,
            0,
            None,
            vec![(10, committed("aa", "甲")), (20, committed("bb", "乙"))],
        );
        let second = snapshot(
            &stream,
            1,
            2,
            Some(5_000),
            vec![
                (
                    99,
                    NativeFeedbackEvent::PersonalPhraseAdjacencyObserved {
                        adjacency: NativePersonalPhraseAdjacency::VerifiedAdjacent,
                        previous_components: 1,
                        resulting_components: 2,
                    },
                ),
                (100, committed("cc", "丙")),
                (110, committed("dd", "丁")),
            ],
        );

        let report = analyze_linked_research(&[first, second], &[]).unwrap();
        assert_eq!(report.linked_batches(), 2);
        assert_eq!(report.linked_streams(), 1);
        assert_eq!(report.episodes(), 4);
        assert_eq!(report.scenes(), 1);
        assert_eq!(report.median_episodes_per_scene(), 4);
    }

    #[test]
    fn runtime_half_pair_analysis_compares_candidates_without_retaining_text() {
        let stream = "67".repeat(32);
        let identity = runtime_identity('a');
        let research = snapshot_with_identity(
            &stream,
            0,
            0,
            None,
            Some(identity.clone()),
            vec![
                (
                    10,
                    frame(
                        "abc",
                        &["甲乙", "共同", "暂态"],
                        &[
                            NativeCandidateSource::Decoder,
                            NativeCandidateSource::Decoder,
                            NativeCandidateSource::Decoder,
                        ],
                    ),
                ),
                (
                    11,
                    NativeFeedbackEvent::CandidatePopupTiming {
                        first_frame_ms: 1,
                        fully_visible_ms: 1,
                        initial_show: false,
                    },
                ),
                (
                    30,
                    frame(
                        "abcd",
                        &["完整", "共同", "新项"],
                        &[
                            NativeCandidateSource::Decoder,
                            NativeCandidateSource::CoreExact,
                            NativeCandidateSource::Decoder,
                        ],
                    ),
                ),
            ],
        );

        let report = analyze_runtime_half_pairs(&[research], &identity).unwrap();
        assert_eq!(report.linked_batches(), 1);
        assert_eq!(report.linked_streams(), 1);
        assert_eq!(report.paired_frames(), 1);
        assert_eq!(report.gap_histogram(), &[0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(report.completed_before_ms(24), Some(1));
        assert_eq!(report.top_candidate_comparisons(), 1);
        assert_eq!(report.top_candidate_changes(), 1);
        assert_eq!(report.candidate_slots_before(), 3);
        assert_eq!(report.retained_candidates(), 1);
        assert_eq!(report.provenance_comparisons(), 1);
        assert_eq!(report.decoder_top_after_completion(), 1);
        assert!(!format!("{report:?}").contains("共同"));
    }

    #[test]
    fn runtime_half_pair_analysis_joins_a_contiguous_batch_boundary() {
        let stream = "78".repeat(32);
        let identity = runtime_identity('b');
        let first = snapshot_with_identity(
            &stream,
            0,
            0,
            None,
            Some(identity.clone()),
            vec![(
                10,
                frame("abc", &["暂态"], &[NativeCandidateSource::Decoder]),
            )],
        );
        let second = snapshot_with_identity(
            &stream,
            1,
            1,
            Some(12),
            Some(identity.clone()),
            vec![(
                100,
                frame("abcd", &["完整"], &[NativeCandidateSource::CoreExact]),
            )],
        );

        let report = analyze_runtime_half_pairs(&[first, second], &identity).unwrap();
        assert_eq!(report.linked_batches(), 2);
        assert_eq!(report.chain_breaks(), 0);
        assert_eq!(report.paired_frames(), 1);
        assert_eq!(report.completed_before_ms(16), Some(1));
        assert_eq!(report.decoder_top_after_completion(), 0);
    }

    #[test]
    fn focus_loss_and_long_idle_create_natural_boundaries() {
        let stream = "23".repeat(32);
        let events = vec![
            (10, committed("aa", "甲")),
            (
                20,
                NativeFeedbackEvent::CompositionCancelled {
                    code: "bb".to_owned(),
                    source: NativeCancellationSource::FocusLoss,
                },
            ),
            (30, committed("cc", "丙")),
            (100_000, committed("dd", "丁")),
        ];
        let report =
            analyze_linked_research(&[snapshot(&stream, 0, 0, None, events)], &[]).unwrap();
        assert_eq!(report.scenes(), 3);
    }

    #[test]
    fn accepted_recovery_and_repeated_revision_remain_distinct_clues() {
        let stream = "34".repeat(32);
        let recovery_frame = NativeFeedbackEvent::CandidatesPresentedWithProvenance {
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
                31,
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
        let mut events = vec![(10, recovery_frame), (11, committed("fuem", "什么"))];
        for base in [20, 30] {
            events.push((
                base,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "abdc".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["甲乙".to_owned()],
                    may_have_more: false,
                },
            ));
            events.push((
                base + 1,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "abcd".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["甲乙".to_owned()],
                    may_have_more: false,
                },
            ));
            events.push((base + 2, committed("abcd", "甲乙")));
        }
        let report =
            analyze_linked_research(&[snapshot(&stream, 0, 0, None, events)], &[]).unwrap();
        assert_eq!(report.habit_clues().len(), 2);
        assert_eq!(
            report.habit_clues()[0].kind(),
            ResearchHabitKind::RepeatedCodeRevision
        );
        assert_eq!(report.habit_clues()[0].observations(), 2);
        assert_eq!(
            report.habit_clues()[1].kind(),
            ResearchHabitKind::AcceptedTransposition
        );
        assert_eq!(report.habit_clues()[1].median_pair_gap_ms(), Some(31));
    }

    #[test]
    fn wish_anchor_attaches_to_the_nearest_preceding_scene() {
        let stream = "45".repeat(32);
        let research = snapshot(
            &stream,
            0,
            0,
            None,
            vec![(10, committed("aa", "甲")), (20, committed("bb", "乙"))],
        );
        let mut feedback = NativeFeedbackSession::default();
        feedback.start_memory(
            crate::NativeFeedbackAuthorization::explicit_memory_only(),
            crate::NativeFeedbackLimits::default(),
        );
        feedback.record_at(
            crate::NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "xuy".to_owned(),
            },
            1,
        );
        let frozen = feedback
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                2,
                10,
                8,
            )
            .unwrap();
        let wish = WishSnapshot::from_frozen_with_context(
            &frozen,
            WishCaptureScope::RecentWindow,
            WishCategory::Other,
            None,
            Some(WishJournalContext::WishAnchor(
                WishJournalAnchor::new(stream, 2).unwrap(),
            )),
        )
        .unwrap();

        let report = analyze_linked_research(&[research], &[wish]).unwrap();
        assert_eq!(report.anchored_wishes(), 1);
        assert_eq!(report.linked_wishes(), 1);
        assert_eq!(report.unanchored_wishes(), 0);
    }
}
