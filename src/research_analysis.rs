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
const MAX_WISH_CONTEXT_EPISODES: usize = 6;
const RETYPE_MAX_GAP_MS: u64 = 10_000;
const RECORDED_CANDIDATE_PAGE_SIZE: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchHabitKind {
    AcceptedTransposition,
    RepeatedCodeRevision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchWishEpisodeKind {
    CandidateCommit,
    RawCodeCommit,
    Cancellation,
}

/// Evidence-only classification of one completed input near a wish anchor.
///
/// These labels describe what the journal directly observed. They do not
/// decide whether a candidate was correct, useful, or linguistically odd.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchWishEvidenceKind {
    TopCandidate,
    VisibleNonTopCandidate,
    DeepCandidate,
    RawCodeCommit,
    Cancellation,
}

/// One completed input immediately preceding an explicitly anchored wish.
///
/// This type intentionally omits `Debug` because it contains real input and
/// committed text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchWishEpisode {
    kind: ResearchWishEpisodeKind,
    code: String,
    text: Option<String>,
    rank: Option<usize>,
    post_commit_backspace_routed: bool,
    evidence_kind: ResearchWishEvidenceKind,
    followed_by_retype: bool,
}

impl ResearchWishEpisode {
    pub fn kind(&self) -> ResearchWishEpisodeKind {
        self.kind
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn rank(&self) -> Option<usize> {
        self.rank
    }

    pub fn post_commit_backspace_routed(&self) -> bool {
        self.post_commit_backspace_routed
    }

    pub fn evidence_kind(&self) -> ResearchWishEvidenceKind {
        self.evidence_kind
    }

    /// Whether this commit was followed by the bounded, same-chain
    /// Backspace-then-different-commit sequence used for retyping clues.
    pub fn followed_by_retype(&self) -> bool {
        self.followed_by_retype
    }
}

/// Bounded private context attached to one explicit wish anchor.
///
/// Only completed inputs in the same natural scene and at or before the
/// anchor are retained. This type deliberately omits `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchWishContext {
    category: crate::WishCategory,
    preceding_episodes: usize,
    episodes: Vec<ResearchWishEpisode>,
}

impl ResearchWishContext {
    pub fn category(&self) -> crate::WishCategory {
        self.category
    }

    pub fn preceding_episodes(&self) -> usize {
        self.preceding_episodes
    }

    pub fn episodes(&self) -> &[ResearchWishEpisode] {
        &self.episodes
    }
}

/// One evidence-bounded sequence where a committed candidate was followed by
/// the TSF post-commit Backspace route and then a different candidate commit.
///
/// The host document result is not observable, so this remains a retyping
/// clue rather than a claimed correction. Private strings prevent `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchRetypeClue {
    previous_code: String,
    previous_text: String,
    next_code: String,
    next_text: String,
    observations: usize,
    median_gap_ms: u64,
}

impl ResearchRetypeClue {
    pub fn previous_code(&self) -> &str {
        &self.previous_code
    }

    pub fn previous_text(&self) -> &str {
        &self.previous_text
    }

    pub fn next_code(&self) -> &str {
        &self.next_code
    }

    pub fn next_text(&self) -> &str {
        &self.next_text
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn median_gap_ms(&self) -> u64 {
        self.median_gap_ms
    }
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
    wish_contexts: Vec<ResearchWishContext>,
    retype_clues: Vec<ResearchRetypeClue>,
    input_scenes: Vec<ResearchInputScene>,
    selection_confirmation_sequences: Vec<ResearchSelectionConfirmationSequence>,
}

/// One completed input retained inside a reconstructed natural scene.
///
/// This type intentionally omits `Debug` because it contains real input,
/// committed text, and the directly observed first candidate.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchInputEpisode {
    kind: ResearchWishEpisodeKind,
    code: String,
    text: Option<String>,
    rank: Option<usize>,
    top_candidate: Option<String>,
    post_commit_backspace_routed: bool,
}

impl ResearchInputEpisode {
    pub fn kind(&self) -> ResearchWishEpisodeKind {
        self.kind
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn rank(&self) -> Option<usize> {
        self.rank
    }

    pub fn top_candidate(&self) -> Option<&str> {
        self.top_candidate.as_deref()
    }

    pub fn post_commit_backspace_routed(&self) -> bool {
        self.post_commit_backspace_routed
    }
}

/// One process-local natural input scene. Storage batch boundaries are not
/// represented here. This type intentionally omits `Debug` because its
/// episodes contain private text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchInputScene {
    episodes: Vec<ResearchInputEpisode>,
}

impl ResearchInputScene {
    pub fn episodes(&self) -> &[ResearchInputEpisode] {
        &self.episodes
    }
}

/// How much same-chain evidence can safely connect one V19 personal-selection
/// confirmation to an earlier candidate commit.
///
/// A unique identity match is still an analysis-time correlation, not a
/// durable selection identifier. Repeated equal commits deliberately remain
/// ambiguous instead of being assigned by proximity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchSelectionConfirmationMatch {
    UniquePriorCommit,
    AmbiguousPriorCommits,
    NoPriorCommit,
}

/// One V19 personal-selection confirmation retained in its continuous-chain
/// order. This type intentionally omits `Debug` because it contains private
/// input and committed text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchSelectionConfirmation {
    code: String,
    text: String,
    persistent_preferred: bool,
    session_retained: bool,
    matching: ResearchSelectionConfirmationMatch,
}

impl ResearchSelectionConfirmation {
    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn persistent_preferred(&self) -> bool {
        self.persistent_preferred
    }

    pub fn session_retained(&self) -> bool {
        self.session_retained
    }

    pub fn matching(&self) -> ResearchSelectionConfirmationMatch {
        self.matching
    }
}

/// One uninterrupted process-local confirmation timeline. Separate streams
/// and discontinuous journal chains are never merged or globally ordered.
/// This type omits `Debug` because its confirmations contain private text.
#[derive(Clone, Eq, PartialEq)]
pub struct ResearchSelectionConfirmationSequence {
    confirmations: Vec<ResearchSelectionConfirmation>,
}

impl ResearchSelectionConfirmationSequence {
    pub fn confirmations(&self) -> &[ResearchSelectionConfirmation] {
        &self.confirmations
    }
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

    pub fn wish_contexts(&self) -> &[ResearchWishContext] {
        &self.wish_contexts
    }

    pub fn retype_clues(&self) -> &[ResearchRetypeClue] {
        &self.retype_clues
    }

    pub fn input_scenes(&self) -> &[ResearchInputScene] {
        &self.input_scenes
    }

    pub fn selection_confirmation_sequences(&self) -> &[ResearchSelectionConfirmationSequence] {
        &self.selection_confirmation_sequences
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
    top_candidate_code: Option<String>,
    top_candidate: Option<String>,
}

impl PendingEpisode {
    fn new(at_ms: u64, ordinal: u64) -> Self {
        Self {
            start_ms: at_ms,
            first_ordinal: ordinal,
            previous_code: None,
            revision_from: None,
            recovery: None,
            top_candidate_code: None,
            top_candidate: None,
        }
    }

    fn observe_candidates(&mut self, event: &NativeFeedbackEvent) {
        let (code, page_start, candidates, automatic_transposition) = match event {
            NativeFeedbackEvent::CandidatesPresented {
                code,
                page_start,
                candidates,
                ..
            } => (code, page_start, candidates, None),
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                page_start,
                candidates,
                automatic_transposition,
                ..
            } => (
                code,
                page_start,
                candidates,
                automatic_transposition.as_ref(),
            ),
            _ => return,
        };
        self.observe_code(code);
        if *page_start == 0 {
            self.top_candidate_code = Some(code.clone());
            self.top_candidate = candidates.first().cloned();
        } else if self.top_candidate_code.as_deref() != Some(code) {
            self.top_candidate_code = None;
            self.top_candidate = None;
        }
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
    completed_ordinal: u64,
    last_ordinal: u64,
    hard_boundary_after: bool,
    accepted_recovery: Option<RecoveryEvidence>,
    revision: Option<(String, String, String)>,
    outcome: EpisodeOutcome,
    post_commit_backspace_routed: bool,
    post_commit_backspace_ordinal: Option<u64>,
    top_candidate: Option<String>,
}

struct SelectionConfirmation {
    chain: usize,
    ordinal: u64,
    code: String,
    text: String,
    persistent_preferred: bool,
    session_retained: bool,
}

enum EpisodeOutcome {
    CandidateCommit {
        code: String,
        text: String,
        rank: usize,
    },
    RawCodeCommit {
        code: String,
    },
    Cancellation {
        code: String,
    },
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

#[derive(Hash, Eq, PartialEq)]
struct RetypeKey {
    previous_code: String,
    previous_text: String,
    next_code: String,
    next_text: String,
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
    let mut anchor_requests = Vec::new();
    let mut unanchored_wishes = 0;
    for wish in wishes {
        if let Some(WishJournalContext::WishAnchor(anchor)) = wish.journal_context() {
            anchors
                .entry(anchor.stream_id().to_owned())
                .or_default()
                .push(anchor.event_ordinal());
            anchor_requests.push((
                anchor.stream_id().to_owned(),
                anchor.event_ordinal(),
                wish.category(),
            ));
        } else {
            unanchored_wishes += 1;
        }
    }
    let anchored_wishes = anchors.values().map(Vec::len).sum();

    let mut stream_episodes: HashMap<String, Vec<Episode>> = HashMap::new();
    let mut stream_confirmations: HashMap<String, Vec<SelectionConfirmation>> = HashMap::new();
    let mut chain_breaks = 0;
    for (stream_id, batches) in &mut streams {
        batches.sort_by_key(|batch| batch.sequence);
        if batches
            .windows(2)
            .any(|pair| pair[0].sequence == pair[1].sequence)
        {
            return Err(ResearchSceneError::DuplicateBatchSequence);
        }
        let (episodes, confirmations, breaks) = reconstruct_stream(batches)?;
        chain_breaks += breaks;
        stream_episodes.insert(stream_id.clone(), episodes);
        stream_confirmations.insert(stream_id.clone(), confirmations);
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
    let wish_contexts = collect_wish_contexts(&stream_episodes, &scenes, &anchor_requests);
    let retype_clues = collect_retype_clues(stream_episodes.values());
    let input_scenes = collect_input_scenes(&stream_episodes, &scenes);
    let selection_confirmation_sequences =
        collect_selection_confirmation_sequences(&stream_episodes, &stream_confirmations);
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
        wish_contexts,
        retype_clues,
        input_scenes,
        selection_confirmation_sequences,
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
) -> Result<(Vec<Episode>, Vec<SelectionConfirmation>, usize), ResearchSceneError> {
    let mut episodes = Vec::new();
    let mut confirmations = Vec::new();
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
                &mut confirmations,
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
    Ok((episodes, confirmations, chain_breaks))
}

fn observe_event(
    pending: &mut Option<PendingEpisode>,
    episodes: &mut Vec<Episode>,
    confirmations: &mut Vec<SelectionConfirmation>,
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
        | NativeFeedbackEvent::CandidateSuppressionChanged { .. }
        | NativeFeedbackEvent::PersonalPhraseAdjacencyObserved { .. } => {}
        NativeFeedbackEvent::PersonalSelectionConfirmed {
            code,
            text,
            persistent_preferred,
            session_retained,
        } => confirmations.push(SelectionConfirmation {
            chain,
            ordinal,
            code: code.clone(),
            text: text.clone(),
            persistent_preferred: *persistent_preferred,
            session_retained: *session_retained,
        }),
        NativeFeedbackEvent::PostCommitBackspaceRouted => {
            if pending.is_none()
                && let Some(previous) = episodes.last_mut()
                && matches!(previous.outcome, EpisodeOutcome::CandidateCommit { .. })
            {
                previous.post_commit_backspace_routed = true;
                previous.post_commit_backspace_ordinal = Some(ordinal);
                previous.last_ordinal = ordinal;
                previous.end_ms = at_ms;
            }
        }
        NativeFeedbackEvent::CandidateCommitted {
            code,
            text,
            absolute_rank,
            ..
        } => {
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
                completed_ordinal: ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: false,
                accepted_recovery,
                revision,
                outcome: EpisodeOutcome::CandidateCommit {
                    code: code.clone(),
                    text: text.clone(),
                    rank: *absolute_rank,
                },
                post_commit_backspace_routed: false,
                post_commit_backspace_ordinal: None,
                top_candidate: (episode.top_candidate_code.as_deref() == Some(code.as_str()))
                    .then_some(episode.top_candidate)
                    .flatten(),
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
                completed_ordinal: ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: false,
                accepted_recovery: None,
                revision: None,
                outcome: EpisodeOutcome::RawCodeCommit { code: code.clone() },
                post_commit_backspace_routed: false,
                post_commit_backspace_ordinal: None,
                top_candidate: None,
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
                completed_ordinal: ordinal,
                last_ordinal: ordinal,
                hard_boundary_after: matches!(
                    source,
                    NativeCancellationSource::FocusLoss | NativeCancellationSource::HostTermination
                ),
                accepted_recovery: None,
                revision: None,
                outcome: EpisodeOutcome::Cancellation { code: code.clone() },
                post_commit_backspace_routed: false,
                post_commit_backspace_ordinal: None,
                top_candidate: None,
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

fn collect_wish_contexts(
    stream_episodes: &HashMap<String, Vec<Episode>>,
    scenes: &[Scene],
    anchors: &[(String, u64, crate::WishCategory)],
) -> Vec<ResearchWishContext> {
    let mut contexts = Vec::new();
    for (stream_id, ordinal, category) in anchors {
        let Some(episodes) = stream_episodes.get(stream_id) else {
            continue;
        };
        let mut stream_scenes = scenes
            .iter()
            .filter(|scene| scene.stream_id == *stream_id)
            .collect::<Vec<_>>();
        stream_scenes.sort_by_key(|scene| scene.last_ordinal);
        let Some(scene) = stream_scenes
            .iter()
            .rev()
            .find(|scene| scene.first_ordinal <= *ordinal)
        else {
            continue;
        };
        let matching = episodes
            .iter()
            .filter(|episode| {
                episode.chain == scene.chain
                    && episode.first_ordinal >= scene.first_ordinal
                    && episode.last_ordinal <= *ordinal
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            continue;
        }
        let start = matching.len().saturating_sub(MAX_WISH_CONTEXT_EPISODES);
        contexts.push(ResearchWishContext {
            category: *category,
            preceding_episodes: matching.len(),
            episodes: matching[start..]
                .iter()
                .enumerate()
                .map(|(index, episode)| {
                    let followed_by_retype = matching
                        .get(start + index + 1)
                        .is_some_and(|next| is_bounded_retype_pair(episode, next));
                    episode.as_wish_episode(followed_by_retype)
                })
                .collect(),
        });
    }
    contexts
}

impl Episode {
    fn as_wish_episode(&self, followed_by_retype: bool) -> ResearchWishEpisode {
        let (kind, code, text, rank, evidence_kind) = match &self.outcome {
            EpisodeOutcome::CandidateCommit { code, text, rank } => (
                ResearchWishEpisodeKind::CandidateCommit,
                code.clone(),
                Some(text.clone()),
                Some(*rank),
                match *rank {
                    1 => ResearchWishEvidenceKind::TopCandidate,
                    2..=RECORDED_CANDIDATE_PAGE_SIZE => {
                        ResearchWishEvidenceKind::VisibleNonTopCandidate
                    }
                    _ => ResearchWishEvidenceKind::DeepCandidate,
                },
            ),
            EpisodeOutcome::RawCodeCommit { code } => (
                ResearchWishEpisodeKind::RawCodeCommit,
                code.clone(),
                None,
                None,
                ResearchWishEvidenceKind::RawCodeCommit,
            ),
            EpisodeOutcome::Cancellation { code } => (
                ResearchWishEpisodeKind::Cancellation,
                code.clone(),
                None,
                None,
                ResearchWishEvidenceKind::Cancellation,
            ),
        };
        ResearchWishEpisode {
            kind,
            code,
            text,
            rank,
            post_commit_backspace_routed: self.post_commit_backspace_routed,
            evidence_kind,
            followed_by_retype,
        }
    }

    fn as_input_episode(&self) -> ResearchInputEpisode {
        let (kind, code, text, rank) = match &self.outcome {
            EpisodeOutcome::CandidateCommit { code, text, rank } => (
                ResearchWishEpisodeKind::CandidateCommit,
                code.clone(),
                Some(text.clone()),
                Some(*rank),
            ),
            EpisodeOutcome::RawCodeCommit { code } => (
                ResearchWishEpisodeKind::RawCodeCommit,
                code.clone(),
                None,
                None,
            ),
            EpisodeOutcome::Cancellation { code } => (
                ResearchWishEpisodeKind::Cancellation,
                code.clone(),
                None,
                None,
            ),
        };
        ResearchInputEpisode {
            kind,
            code,
            text,
            rank,
            top_candidate: self.top_candidate.clone(),
            post_commit_backspace_routed: self.post_commit_backspace_routed,
        }
    }
}

fn collect_input_scenes(
    stream_episodes: &HashMap<String, Vec<Episode>>,
    scenes: &[Scene],
) -> Vec<ResearchInputScene> {
    let mut ordered = scenes.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.stream_id
            .cmp(&right.stream_id)
            .then_with(|| left.chain.cmp(&right.chain))
            .then_with(|| left.first_ordinal.cmp(&right.first_ordinal))
    });
    ordered
        .into_iter()
        .filter_map(|scene| {
            let episodes = stream_episodes
                .get(&scene.stream_id)?
                .iter()
                .filter(|episode| {
                    episode.chain == scene.chain
                        && episode.first_ordinal >= scene.first_ordinal
                        && episode.last_ordinal <= scene.last_ordinal
                })
                .map(Episode::as_input_episode)
                .collect::<Vec<_>>();
            (!episodes.is_empty()).then_some(ResearchInputScene { episodes })
        })
        .collect()
}

fn collect_selection_confirmation_sequences(
    stream_episodes: &HashMap<String, Vec<Episode>>,
    stream_confirmations: &HashMap<String, Vec<SelectionConfirmation>>,
) -> Vec<ResearchSelectionConfirmationSequence> {
    let mut stream_ids = stream_confirmations.keys().collect::<Vec<_>>();
    stream_ids.sort_unstable();
    let mut sequences = Vec::new();

    for stream_id in stream_ids {
        let episodes = stream_episodes
            .get(stream_id)
            .map_or(&[][..], Vec::as_slice);
        let confirmations = &stream_confirmations[stream_id];
        let mut chains = confirmations
            .iter()
            .map(|confirmation| confirmation.chain)
            .collect::<Vec<_>>();
        chains.sort_unstable();
        chains.dedup();

        for chain in chains {
            let candidate_commits = episodes
                .iter()
                .filter(|episode| {
                    episode.chain == chain
                        && matches!(&episode.outcome, EpisodeOutcome::CandidateCommit { .. })
                })
                .collect::<Vec<_>>();
            let mut chain_confirmations = confirmations
                .iter()
                .filter(|confirmation| confirmation.chain == chain)
                .collect::<Vec<_>>();
            chain_confirmations.sort_by_key(|confirmation| confirmation.ordinal);

            let mut next_candidate = 0_usize;
            let mut available = Vec::<usize>::new();
            let mut rendered = Vec::with_capacity(chain_confirmations.len());
            for confirmation in chain_confirmations {
                while candidate_commits
                    .get(next_candidate)
                    .is_some_and(|episode| episode.completed_ordinal < confirmation.ordinal)
                {
                    available.push(next_candidate);
                    next_candidate = next_candidate.saturating_add(1);
                }

                let matching_positions = available
                    .iter()
                    .enumerate()
                    .filter_map(|(position, candidate_index)| {
                        let episode = candidate_commits[*candidate_index];
                        if episode
                            .post_commit_backspace_ordinal
                            .is_some_and(|retracted| retracted < confirmation.ordinal)
                        {
                            return None;
                        }
                        let EpisodeOutcome::CandidateCommit { code, text, .. } = &episode.outcome
                        else {
                            return None;
                        };
                        (code == &confirmation.code && text == &confirmation.text)
                            .then_some(position)
                    })
                    .collect::<Vec<_>>();
                let matching = match matching_positions.as_slice() {
                    [position] => {
                        available.remove(*position);
                        ResearchSelectionConfirmationMatch::UniquePriorCommit
                    }
                    [] => ResearchSelectionConfirmationMatch::NoPriorCommit,
                    _ => ResearchSelectionConfirmationMatch::AmbiguousPriorCommits,
                };
                rendered.push(ResearchSelectionConfirmation {
                    code: confirmation.code.clone(),
                    text: confirmation.text.clone(),
                    persistent_preferred: confirmation.persistent_preferred,
                    session_retained: confirmation.session_retained,
                    matching,
                });
            }
            if !rendered.is_empty() {
                sequences.push(ResearchSelectionConfirmationSequence {
                    confirmations: rendered,
                });
            }
        }
    }
    sequences
}

fn is_bounded_retype_pair(previous: &Episode, next: &Episode) -> bool {
    if previous.chain != next.chain || !previous.post_commit_backspace_routed {
        return false;
    }
    if next.start_ms.saturating_sub(previous.end_ms) > RETYPE_MAX_GAP_MS {
        return false;
    }
    match (&previous.outcome, &next.outcome) {
        (
            EpisodeOutcome::CandidateCommit {
                code: previous_code,
                text: previous_text,
                ..
            },
            EpisodeOutcome::CandidateCommit {
                code: next_code,
                text: next_text,
                ..
            },
        ) => previous_code != next_code || previous_text != next_text,
        _ => false,
    }
}

fn collect_retype_clues<'a>(
    streams: impl Iterator<Item = &'a Vec<Episode>>,
) -> Vec<ResearchRetypeClue> {
    let mut evidence: HashMap<RetypeKey, Vec<u64>> = HashMap::new();
    for episodes in streams {
        for pair in episodes.windows(2) {
            let [previous, next] = pair else {
                continue;
            };
            if !is_bounded_retype_pair(previous, next) {
                continue;
            }
            let gap_ms = next.start_ms.saturating_sub(previous.end_ms);
            let (
                EpisodeOutcome::CandidateCommit {
                    code: previous_code,
                    text: previous_text,
                    ..
                },
                EpisodeOutcome::CandidateCommit {
                    code: next_code,
                    text: next_text,
                    ..
                },
            ) = (&previous.outcome, &next.outcome)
            else {
                continue;
            };
            if previous_code == next_code && previous_text == next_text {
                continue;
            }
            evidence
                .entry(RetypeKey {
                    previous_code: previous_code.clone(),
                    previous_text: previous_text.clone(),
                    next_code: next_code.clone(),
                    next_text: next_text.clone(),
                })
                .or_default()
                .push(gap_ms);
        }
    }
    let mut clues = evidence
        .into_iter()
        .map(|(key, mut gaps)| {
            gaps.sort_unstable();
            ResearchRetypeClue {
                previous_code: key.previous_code,
                previous_text: key.previous_text,
                next_code: key.next_code,
                next_text: key.next_text,
                observations: gaps.len(),
                median_gap_ms: nearest_rank(&gaps, 50).unwrap_or(0),
            }
        })
        .collect::<Vec<_>>();
    clues.sort_by(|left, right| {
        right
            .observations
            .cmp(&left.observations)
            .then_with(|| left.previous_code.cmp(&right.previous_code))
            .then_with(|| left.next_code.cmp(&right.next_code))
    });
    clues
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
        committed_at_rank(code, text, 1)
    }

    fn committed_at_rank(code: &str, text: &str, rank: usize) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidateCommitted {
            code: code.to_owned(),
            text: text.to_owned(),
            view: NativeCandidateView::Ordinary,
            source: if rank == 1 {
                NativeSelectionSource::FirstCandidate
            } else {
                NativeSelectionSource::Numeric
            },
            absolute_rank: rank,
            visible_rank: (rank.saturating_sub(1) % RECORDED_CANDIDATE_PAGE_SIZE) + 1,
        }
    }

    fn confirmed(
        code: &str,
        text: &str,
        persistent_preferred: bool,
        session_retained: bool,
    ) -> NativeFeedbackEvent {
        NativeFeedbackEvent::PersonalSelectionConfirmed {
            code: code.to_owned(),
            text: text.to_owned(),
            persistent_preferred,
            session_retained,
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
    fn natural_input_scenes_retain_completed_text_rank_and_observed_top() {
        let stream = "89".repeat(32);
        let events = vec![
            (
                10,
                frame(
                    "aa",
                    &["甲", "乙"],
                    &[
                        NativeCandidateSource::CoreExact,
                        NativeCandidateSource::CoreExact,
                    ],
                ),
            ),
            (20, committed_at_rank("aa", "乙", 2)),
            (30, NativeFeedbackEvent::PostCommitBackspaceRouted),
            (
                40,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "tool".to_owned(),
                },
            ),
            (
                50,
                NativeFeedbackEvent::CompositionCancelled {
                    code: "bb".to_owned(),
                    source: NativeCancellationSource::Escape,
                },
            ),
        ];
        let report =
            analyze_linked_research(&[snapshot(&stream, 0, 0, None, events)], &[]).unwrap();
        assert_eq!(report.input_scenes().len(), 1);
        let episodes = report.input_scenes()[0].episodes();
        assert_eq!(episodes.len(), 3);
        assert_eq!(episodes[0].kind(), ResearchWishEpisodeKind::CandidateCommit);
        assert_eq!(episodes[0].code(), "aa");
        assert_eq!(episodes[0].text(), Some("乙"));
        assert_eq!(episodes[0].rank(), Some(2));
        assert_eq!(episodes[0].top_candidate(), Some("甲"));
        assert!(episodes[0].post_commit_backspace_routed());
        assert_eq!(episodes[1].kind(), ResearchWishEpisodeKind::RawCodeCommit);
        assert_eq!(episodes[1].top_candidate(), None);
        assert_eq!(episodes[2].kind(), ResearchWishEpisodeKind::Cancellation);
    }

    #[test]
    fn personal_selection_confirmation_joins_a_contiguous_batch_boundary() {
        let stream = "8a".repeat(32);
        let first = snapshot(&stream, 0, 0, None, vec![(10, committed("aa", "甲"))]);
        let second = snapshot(
            &stream,
            1,
            1,
            Some(12),
            vec![(20, confirmed("aa", "甲", false, true))],
        );

        let report = analyze_linked_research(&[first, second], &[]).unwrap();
        assert_eq!(report.chain_breaks(), 0);
        assert_eq!(report.selection_confirmation_sequences().len(), 1);
        let confirmation = &report.selection_confirmation_sequences()[0].confirmations()[0];
        assert_eq!(confirmation.code(), "aa");
        assert_eq!(confirmation.text(), "甲");
        assert!(!confirmation.persistent_preferred());
        assert!(confirmation.session_retained());
        assert_eq!(
            confirmation.matching(),
            ResearchSelectionConfirmationMatch::UniquePriorCommit
        );
    }

    #[test]
    fn personal_selection_confirmation_uses_identity_not_event_proximity() {
        let stream = "8b".repeat(32);
        let report = analyze_linked_research(
            &[snapshot(
                &stream,
                0,
                0,
                None,
                vec![
                    (10, committed("aa", "甲")),
                    (11, confirmed("aa", "甲", true, false)),
                    (20, committed("bb", "乙")),
                    (30, committed("cc", "丙")),
                    (31, confirmed("bb", "乙", false, true)),
                ],
            )],
            &[],
        )
        .unwrap();

        let confirmations = report.selection_confirmation_sequences()[0].confirmations();
        assert_eq!(confirmations.len(), 2);
        assert_eq!(confirmations[0].code(), "aa");
        assert_eq!(confirmations[1].code(), "bb");
        assert!(confirmations.iter().all(|confirmation| {
            confirmation.matching() == ResearchSelectionConfirmationMatch::UniquePriorCommit
        }));
        assert!(confirmations[0].persistent_preferred());
        assert!(!confirmations[0].session_retained());
        assert!(!confirmations[1].persistent_preferred());
        assert!(confirmations[1].session_retained());
    }

    #[test]
    fn repeated_equal_commits_keep_personal_confirmation_ambiguous() {
        let stream = "8c".repeat(32);
        let report = analyze_linked_research(
            &[snapshot(
                &stream,
                0,
                0,
                None,
                vec![
                    (10, committed("aa", "甲")),
                    (20, committed("aa", "甲")),
                    (21, confirmed("aa", "甲", true, true)),
                ],
            )],
            &[],
        )
        .unwrap();

        assert_eq!(
            report.selection_confirmation_sequences()[0].confirmations()[0].matching(),
            ResearchSelectionConfirmationMatch::AmbiguousPriorCommits
        );
    }

    #[test]
    fn retracted_commit_does_not_make_a_later_confirmation_ambiguous() {
        let stream = "8e".repeat(32);
        let report = analyze_linked_research(
            &[snapshot(
                &stream,
                0,
                0,
                None,
                vec![
                    (10, committed("aa", "甲")),
                    (11, NativeFeedbackEvent::PostCommitBackspaceRouted),
                    (20, committed("aa", "甲")),
                    (21, confirmed("aa", "甲", true, true)),
                ],
            )],
            &[],
        )
        .unwrap();

        assert_eq!(
            report.selection_confirmation_sequences()[0].confirmations()[0].matching(),
            ResearchSelectionConfirmationMatch::UniquePriorCommit
        );
    }

    #[test]
    fn later_backspace_does_not_retroactively_unmatch_an_earlier_confirmation() {
        let stream = "8f".repeat(32);
        let report = analyze_linked_research(
            &[snapshot(
                &stream,
                0,
                0,
                None,
                vec![
                    (10, committed("aa", "甲")),
                    (11, confirmed("aa", "甲", true, true)),
                    (12, NativeFeedbackEvent::PostCommitBackspaceRouted),
                ],
            )],
            &[],
        )
        .unwrap();

        assert_eq!(
            report.selection_confirmation_sequences()[0].confirmations()[0].matching(),
            ResearchSelectionConfirmationMatch::UniquePriorCommit
        );
    }

    #[test]
    fn personal_confirmation_never_pairs_across_a_chain_gap() {
        let stream = "8d".repeat(32);
        let first = snapshot(&stream, 0, 0, None, vec![(10, committed("aa", "甲"))]);
        let after_gap = snapshot(
            &stream,
            2,
            1,
            Some(12),
            vec![(20, confirmed("aa", "甲", true, true))],
        );

        let report = analyze_linked_research(&[first, after_gap], &[]).unwrap();
        assert_eq!(report.chain_breaks(), 1);
        assert_eq!(
            report.selection_confirmation_sequences()[0].confirmations()[0].matching(),
            ResearchSelectionConfirmationMatch::NoPriorCommit
        );
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
    fn post_commit_backspace_and_next_commit_form_bounded_retype_evidence() {
        let stream = "56".repeat(32);
        let report = analyze_linked_research(
            &[snapshot(
                &stream,
                0,
                0,
                None,
                vec![
                    (10, committed("abdc", "旧词")),
                    (
                        11,
                        NativeFeedbackEvent::CandidatePopupTiming {
                            first_frame_ms: 1,
                            fully_visible_ms: 1,
                            initial_show: false,
                        },
                    ),
                    (12, NativeFeedbackEvent::PostCommitBackspaceRouted),
                    (40, committed("abcd", "新词")),
                    (50, committed("efgh", "后文")),
                ],
            )],
            &[],
        )
        .unwrap();

        assert_eq!(report.retype_clues().len(), 1);
        let clue = &report.retype_clues()[0];
        assert_eq!(clue.previous_code(), "abdc");
        assert_eq!(clue.previous_text(), "旧词");
        assert_eq!(clue.next_code(), "abcd");
        assert_eq!(clue.next_text(), "新词");
        assert_eq!(clue.observations(), 1);
        assert_eq!(clue.median_gap_ms(), 28);
    }

    #[test]
    fn retype_evidence_never_crosses_streams_or_chain_breaks() {
        let first_stream = "57".repeat(32);
        let second_stream = "58".repeat(32);
        let first = snapshot(
            &first_stream,
            0,
            0,
            None,
            vec![
                (10, committed("aaaa", "甲")),
                (11, NativeFeedbackEvent::PostCommitBackspaceRouted),
            ],
        );
        let second = snapshot(
            &second_stream,
            0,
            0,
            None,
            vec![(12, committed("bbbb", "乙"))],
        );
        let report = analyze_linked_research(&[first, second], &[]).unwrap();
        assert!(report.retype_clues().is_empty());
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
        assert_eq!(report.wish_contexts().len(), 1);
        let context = &report.wish_contexts()[0];
        assert_eq!(context.category(), WishCategory::Other);
        assert_eq!(context.preceding_episodes(), 2);
        assert_eq!(context.episodes().len(), 2);
        assert_eq!(context.episodes()[0].code(), "aa");
        assert_eq!(context.episodes()[0].text(), Some("甲"));
        assert_eq!(context.episodes()[1].code(), "bb");
        assert_eq!(context.episodes()[1].text(), Some("乙"));
    }

    #[test]
    fn wish_context_is_bounded_to_six_completed_inputs_before_the_anchor() {
        let stream = "59".repeat(32);
        let research = snapshot(
            &stream,
            0,
            0,
            None,
            ["aa", "bb", "cc", "dd", "ee", "ff", "gg", "hh"]
                .into_iter()
                .enumerate()
                .map(|(index, code)| {
                    (
                        u64::try_from(index).unwrap() + 1,
                        committed(code, &format!("词{index}")),
                    )
                })
                .collect(),
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
            WishCategory::Ranking,
            None,
            Some(WishJournalContext::WishAnchor(
                WishJournalAnchor::new(stream, 6).unwrap(),
            )),
        )
        .unwrap();

        let report = analyze_linked_research(&[research], &[wish]).unwrap();
        let context = &report.wish_contexts()[0];
        assert_eq!(context.category(), WishCategory::Ranking);
        assert_eq!(context.preceding_episodes(), 7);
        assert_eq!(context.episodes().len(), 6);
        assert_eq!(context.episodes()[0].code(), "bb");
        assert_eq!(context.episodes()[5].code(), "gg");
        assert!(
            context
                .episodes()
                .iter()
                .all(|episode| episode.code() != "hh")
        );
    }

    #[test]
    fn wish_evidence_cards_classify_rank_raw_cancel_and_bounded_retype_without_guessing() {
        let stream = "5a".repeat(32);
        let research = snapshot(
            &stream,
            0,
            0,
            None,
            vec![
                (10, committed_at_rank("aa", "甲", 1)),
                (20, committed_at_rank("bb", "乙", 6)),
                (30, committed_at_rank("cc", "丙", 7)),
                (31, NativeFeedbackEvent::PostCommitBackspaceRouted),
                (40, committed("dd", "丁")),
                (
                    50,
                    NativeFeedbackEvent::RawCodeCommitted {
                        code: "raw".to_owned(),
                    },
                ),
                (
                    60,
                    NativeFeedbackEvent::CompositionCancelled {
                        code: "esc".to_owned(),
                        source: NativeCancellationSource::Escape,
                    },
                ),
            ],
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
            WishCategory::Ranking,
            None,
            Some(WishJournalContext::WishAnchor(
                WishJournalAnchor::new(stream, 6).unwrap(),
            )),
        )
        .unwrap();

        let report = analyze_linked_research(&[research], &[wish]).unwrap();
        let episodes = report.wish_contexts()[0].episodes();
        assert_eq!(episodes.len(), 6);
        assert_eq!(
            episodes[0].evidence_kind(),
            ResearchWishEvidenceKind::TopCandidate
        );
        assert_eq!(
            episodes[1].evidence_kind(),
            ResearchWishEvidenceKind::VisibleNonTopCandidate
        );
        assert_eq!(
            episodes[2].evidence_kind(),
            ResearchWishEvidenceKind::DeepCandidate
        );
        assert!(episodes[2].followed_by_retype());
        assert_eq!(
            episodes[4].evidence_kind(),
            ResearchWishEvidenceKind::RawCodeCommit
        );
        assert_eq!(
            episodes[5].evidence_kind(),
            ResearchWishEvidenceKind::Cancellation
        );
        assert!(
            episodes[3..]
                .iter()
                .all(|episode| !episode.followed_by_retype())
        );
    }
}
