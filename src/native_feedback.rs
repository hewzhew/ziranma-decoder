//! Bounded, memory-only feedback emitted by an input method itself.
//!
//! This module deliberately contains no host APIs, serialization, file I/O,
//! networking, or background work. A host must explicitly choose either a
//! stop-at-limit session or a bounded rolling-memory session and feed it
//! semantic events only after the corresponding user-visible action succeeds.

use crate::{
    TranspositionCalibrationLabel, TranspositionCalibrationObservation,
    TranspositionCalibrationRecommendation, TranspositionCalibrationSummary,
    TranspositionCalibrator,
};

const MAX_FEEDBACK_CODE_BYTES: usize = 64;
const MAX_FEEDBACK_CANDIDATES_PER_PAGE: usize = 7;
const MAX_FEEDBACK_TEXT_CHARACTERS: usize = 128;
const MAX_FEEDBACK_POPUP_TIMING_MS: u32 = 60_000;

pub const DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS: usize = 4_096;
pub const DEFAULT_NATIVE_FEEDBACK_MAX_PRIVATE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS: u64 = 30_000;
pub const DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS: usize = 1_024;
pub const DEFAULT_NATIVE_FEEDBACK_WISH_EPISODES: usize = 3;
pub const DEFAULT_NATIVE_FEEDBACK_WISH_EPISODE_MAX_LOOKBACK_MS: u64 = 2 * 60_000;
pub const MAX_NATIVE_FEEDBACK_WISH_LOOKBACK_MS: u64 = 5 * 60_000;
pub const NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS: [u64; 8] =
    [8, 16, 24, 32, 48, 64, 96, 160];
pub const NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS: usize =
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS.len() + 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeFeedbackLimits {
    pub max_events: usize,
    pub max_private_bytes: usize,
}

impl Default for NativeFeedbackLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS,
            max_private_bytes: DEFAULT_NATIVE_FEEDBACK_MAX_PRIVATE_BYTES,
        }
    }
}

/// Deliberate acknowledgement that this session may retain private input in
/// process memory.
///
/// The token has no ambient or default constructor. A caller must spell out
/// the memory-only authorization at the point where it starts a session.
#[derive(Clone, Copy)]
#[must_use]
pub struct NativeFeedbackAuthorization {
    _private: (),
}

impl NativeFeedbackAuthorization {
    pub fn explicit_memory_only() -> Self {
        Self { _private: () }
    }
}

/// Deliberate acknowledgement that private in-memory feedback may be copied
/// into a short-lived snapshot for current-user protected storage.
///
/// This is separate from `NativeFeedbackAuthorization`: starting an in-memory
/// session does not silently grant permission to freeze or persist it.
#[derive(Clone, Copy)]
#[must_use]
pub struct NativeFeedbackFreezeAuthorization {
    _private: (),
}

impl NativeFeedbackFreezeAuthorization {
    pub fn explicit_private_snapshot() -> Self {
        Self { _private: () }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NativeFeedbackContext {
    Eligible,
    Password,
    Private,
    KeyboardDisabled,
    Empty,
    Restricted,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCandidateView {
    Ordinary,
    TranspositionRecovery,
    Shape,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NativeCandidateSource {
    #[default]
    Unknown,
    ExplicitAlias,
    ProjectOverlay,
    CoreExact,
    SupplementalExact,
    CharacterPair,
    Decoder,
    TranspositionRecovery,
    Shape,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeCandidateProvenance {
    source: NativeCandidateSource,
    session_promoted: bool,
}

impl NativeCandidateProvenance {
    pub fn new(source: NativeCandidateSource, session_promoted: bool) -> Self {
        Self {
            source,
            session_promoted,
        }
    }

    pub fn source(self) -> NativeCandidateSource {
        self.source
    }

    pub fn session_promoted(self) -> bool {
        self.session_promoted
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSelectionSource {
    FirstCandidate,
    Numeric,
    Punctuation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCancellationSource {
    Backspace,
    Escape,
    FocusLoss,
    HostTermination,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAutomaticTranspositionTier {
    Shadow,
    Secondary,
    Primary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAutomaticTranspositionOutcome {
    Suppressed,
    NoRecovery,
    RecoveryAvailable,
}

/// One explainable automatic transposition decision attached to the candidate
/// frame produced from the same observed code.
///
/// The optional recovery text is private. This type therefore deliberately
/// omits `Debug`, like the containing feedback event.
#[derive(Clone, Eq, PartialEq)]
pub struct NativeAutomaticTranspositionDecision {
    syllable_index: usize,
    syllable_count: usize,
    pair_gap_ms: u32,
    cold_tier: NativeAutomaticTranspositionTier,
    tier: NativeAutomaticTranspositionTier,
    outcome: NativeAutomaticTranspositionOutcome,
    recovered_text: Option<String>,
    visible_rank: Option<usize>,
}

impl NativeAutomaticTranspositionDecision {
    pub(crate) fn new(
        syllable_index: usize,
        pair_gap_ms: u32,
        cold_tier: NativeAutomaticTranspositionTier,
        tier: NativeAutomaticTranspositionTier,
        outcome: NativeAutomaticTranspositionOutcome,
        recovered_text: Option<String>,
        visible_rank: Option<usize>,
    ) -> Self {
        Self::new_span(
            syllable_index..syllable_index.saturating_add(1),
            pair_gap_ms,
            cold_tier,
            tier,
            outcome,
            recovered_text,
            visible_rank,
        )
    }

    pub(crate) fn new_span(
        syllable_span: std::ops::Range<usize>,
        pair_gap_ms: u32,
        cold_tier: NativeAutomaticTranspositionTier,
        tier: NativeAutomaticTranspositionTier,
        outcome: NativeAutomaticTranspositionOutcome,
        recovered_text: Option<String>,
        visible_rank: Option<usize>,
    ) -> Self {
        let syllable_index = syllable_span.start;
        let syllable_count = syllable_span.end.saturating_sub(syllable_span.start);
        Self {
            syllable_index,
            syllable_count,
            pair_gap_ms,
            cold_tier,
            tier,
            outcome,
            recovered_text,
            visible_rank,
        }
    }

    pub fn syllable_index(&self) -> usize {
        self.syllable_index
    }

    pub fn syllable_count(&self) -> usize {
        self.syllable_count
    }

    pub fn pair_gap_ms(&self) -> u32 {
        self.pair_gap_ms
    }

    pub fn tier(&self) -> NativeAutomaticTranspositionTier {
        self.tier
    }

    pub fn cold_tier(&self) -> NativeAutomaticTranspositionTier {
        self.cold_tier
    }

    pub fn outcome(&self) -> NativeAutomaticTranspositionOutcome {
        self.outcome
    }

    pub fn recovered_text(&self) -> Option<&str> {
        self.recovered_text.as_deref()
    }

    pub fn visible_rank(&self) -> Option<usize> {
        self.visible_rank
    }

    fn validate_and_measure(&self, code: &str) -> Option<usize> {
        if !(1..=2).contains(&self.syllable_count)
            || self
                .syllable_index
                .checked_add(self.syllable_count)
                .is_none_or(|end| end > code.len() / 2)
            || self.pair_gap_ms > MAX_FEEDBACK_POPUP_TIMING_MS
            || !transposition_tier_change_is_bounded(self.cold_tier, self.tier)
            || self
                .visible_rank
                .is_some_and(|rank| !(1..=MAX_FEEDBACK_CANDIDATES_PER_PAGE).contains(&rank))
        {
            return None;
        }
        match self.outcome {
            NativeAutomaticTranspositionOutcome::Suppressed
            | NativeAutomaticTranspositionOutcome::NoRecovery => {
                (self.recovered_text.is_none() && self.visible_rank.is_none()).then_some(0)
            }
            NativeAutomaticTranspositionOutcome::RecoveryAvailable => self
                .recovered_text
                .as_deref()
                .filter(|text| valid_text(text))
                .map(str::len),
        }
    }
}

fn transposition_tier_change_is_bounded(
    cold: NativeAutomaticTranspositionTier,
    applied: NativeAutomaticTranspositionTier,
) -> bool {
    !matches!(
        (cold, applied),
        (
            NativeAutomaticTranspositionTier::Primary,
            NativeAutomaticTranspositionTier::Shadow
        ) | (
            NativeAutomaticTranspositionTier::Shadow,
            NativeAutomaticTranspositionTier::Primary
        )
    )
}

/// One private event owned only by the active in-memory session.
///
/// `Debug` and serialization are intentionally not implemented because the
/// event can contain real input codes and committed text.
#[derive(Clone, Eq, PartialEq)]
pub enum NativeFeedbackEvent {
    CandidatesPresented {
        code: String,
        view: NativeCandidateView,
        page_start: usize,
        candidates: Vec<String>,
        may_have_more: bool,
    },
    CandidatesPresentedWithProvenance {
        code: String,
        view: NativeCandidateView,
        page_start: usize,
        candidates: Vec<String>,
        provenance: Vec<NativeCandidateProvenance>,
        automatic_transposition: Option<NativeAutomaticTranspositionDecision>,
        may_have_more: bool,
    },
    CandidateCommitted {
        code: String,
        text: String,
        view: NativeCandidateView,
        source: NativeSelectionSource,
        absolute_rank: usize,
        visible_rank: usize,
    },
    RawCodeCommitted {
        code: String,
    },
    CompositionCancelled {
        code: String,
        source: NativeCancellationSource,
    },
    /// One completed candidate-popup paint. With immediate visibility the
    /// complete frame and the fully visible frame have the same duration.
    CandidatePopupTiming {
        first_frame_ms: u32,
        fully_visible_ms: u32,
        initial_show: bool,
    },
}

impl NativeFeedbackEvent {
    pub(crate) fn validate_and_measure(&self) -> Option<usize> {
        match self {
            Self::CandidatesPresented {
                code,
                page_start,
                candidates,
                ..
            } => {
                if !valid_code(code)
                    || candidates.is_empty()
                    || candidates.len() > MAX_FEEDBACK_CANDIDATES_PER_PAGE
                    || page_start.checked_add(candidates.len()).is_none()
                {
                    return None;
                }
                measure_private_strings(
                    std::iter::once(code.as_str()).chain(candidates.iter().map(String::as_str)),
                )
            }
            Self::CandidatesPresentedWithProvenance {
                code,
                view,
                page_start,
                candidates,
                provenance,
                automatic_transposition,
                ..
            } => {
                if !valid_code(code)
                    || candidates.is_empty()
                    || candidates.len() > MAX_FEEDBACK_CANDIDATES_PER_PAGE
                    || candidates.len() != provenance.len()
                    || page_start.checked_add(candidates.len()).is_none()
                    || (automatic_transposition.is_some() && *view != NativeCandidateView::Ordinary)
                {
                    return None;
                }
                let strings = measure_private_strings(
                    std::iter::once(code.as_str()).chain(candidates.iter().map(String::as_str)),
                )?;
                let decision = automatic_transposition
                    .as_ref()
                    .map_or(Some(0), |decision| decision.validate_and_measure(code))?;
                strings.checked_add(decision)
            }
            Self::CandidateCommitted {
                code,
                text,
                absolute_rank,
                visible_rank,
                ..
            } => {
                if !valid_code(code)
                    || !valid_text(text)
                    || *absolute_rank == 0
                    || !(1..=MAX_FEEDBACK_CANDIDATES_PER_PAGE).contains(visible_rank)
                    || *absolute_rank < *visible_rank
                {
                    return None;
                }
                measure_private_strings([code.as_str(), text.as_str()].into_iter())
            }
            Self::RawCodeCommitted { code } => valid_code(code).then_some(code.len()),
            Self::CompositionCancelled { code, .. } => valid_code(code).then_some(code.len()),
            Self::CandidatePopupTiming {
                first_frame_ms,
                fully_visible_ms,
                ..
            } => (*first_frame_ms <= *fully_visible_ms
                && *fully_visible_ms <= MAX_FEEDBACK_POPUP_TIMING_MS)
                .then_some(0),
        }
    }

    pub(crate) fn completes_input_episode(&self) -> bool {
        matches!(
            self,
            Self::CandidateCommitted { .. }
                | Self::RawCodeCommitted { .. }
                | Self::CompositionCancelled { .. }
        )
    }
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_FEEDBACK_CODE_BYTES
        && code.as_bytes().iter().all(u8::is_ascii_lowercase)
}

fn valid_text(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MAX_FEEDBACK_TEXT_CHARACTERS
}

fn measure_private_strings<'a>(mut strings: impl Iterator<Item = &'a str>) -> Option<usize> {
    strings.try_fold(0_usize, |total, text| {
        valid_text(text)
            .then(|| total.checked_add(text.len()))
            .flatten()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackStopReason {
    EventLimit,
    PrivateByteLimit,
    InvalidEvent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NativeFeedbackLifecycle {
    #[default]
    Disabled,
    Recording,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackStartResult {
    Started,
    AlreadyRecording,
    PreviousSessionRetained,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackStopResult {
    Stopped,
    AlreadyStopped,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackClearResult {
    Cleared,
    AlreadyDisabled,
    StillRecording,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackRecordResult {
    Recorded,
    Disabled,
    NotAccepting,
    Suppressed(NativeFeedbackContext),
    Stopped(NativeFeedbackStopReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFeedbackFreezeError {
    Disabled,
    NotAccepting,
    InvalidLookback,
    InvalidEventLimit,
    FutureTimestamp,
}

/// One private event frozen relative to the explicit feedback marker.
///
/// Neither this type nor its containing snapshot implements `Debug`, so real
/// codes and candidate text cannot leak through routine diagnostics.
#[derive(Clone, Eq, PartialEq)]
pub struct FrozenNativeFeedbackEvent {
    milliseconds_before_marker: u32,
    event: NativeFeedbackEvent,
}

impl FrozenNativeFeedbackEvent {
    pub fn milliseconds_before_marker(&self) -> u32 {
        self.milliseconds_before_marker
    }

    pub fn event(&self) -> &NativeFeedbackEvent {
        &self.event
    }
}

/// A bounded, read-only copy of recent private feedback.
///
/// The marker's absolute monotonic value is intentionally not retained. Only
/// per-event age is exposed to storage code.
#[derive(Clone, Eq, PartialEq)]
pub struct FrozenNativeFeedbackSnapshot {
    lookback_ms: u32,
    source_complete: bool,
    source_events: usize,
    omitted_before_window: usize,
    omitted_untimed: usize,
    omitted_by_event_limit: usize,
    events: Vec<FrozenNativeFeedbackEvent>,
}

impl FrozenNativeFeedbackSnapshot {
    pub fn lookback_ms(&self) -> u32 {
        self.lookback_ms
    }

    pub fn source_complete(&self) -> bool {
        self.source_complete
    }

    pub fn source_events(&self) -> usize {
        self.source_events
    }

    pub fn omitted_before_window(&self) -> usize {
        self.omitted_before_window
    }

    pub fn omitted_untimed(&self) -> usize {
        self.omitted_untimed
    }

    pub fn omitted_by_event_limit(&self) -> usize {
        self.omitted_by_event_limit
    }

    pub fn events(&self) -> &[FrozenNativeFeedbackEvent] {
        &self.events
    }

    /// Builds one non-overlapping, host-owned journal batch from already
    /// validated semantic events.
    ///
    /// This constructor performs no I/O and retains no absolute monotonic
    /// timestamp. It is restricted to the crate so only a host that already
    /// owns an explicitly enabled private feedback stream can use it.
    pub(crate) fn from_journal_events(
        marker_ms: u64,
        events: &[(u64, NativeFeedbackEvent)],
    ) -> Result<Self, NativeFeedbackFreezeError> {
        if events.is_empty() || events.len() > DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS {
            return Err(NativeFeedbackFreezeError::InvalidEventLimit);
        }
        let mut previous_timestamp = None;
        let mut frozen = Vec::with_capacity(events.len());
        for (timestamp, event) in events {
            if *timestamp > marker_ms {
                return Err(NativeFeedbackFreezeError::FutureTimestamp);
            }
            if previous_timestamp.is_some_and(|previous| *timestamp < previous)
                || event.validate_and_measure().is_none()
            {
                return Err(NativeFeedbackFreezeError::InvalidEventLimit);
            }
            previous_timestamp = Some(*timestamp);
            frozen.push(FrozenNativeFeedbackEvent {
                milliseconds_before_marker: u32::try_from(marker_ms - *timestamp)
                    .map_err(|_| NativeFeedbackFreezeError::InvalidLookback)?,
                event: event.clone(),
            });
        }
        let lookback_ms = frozen
            .first()
            .map(|event| event.milliseconds_before_marker.max(1))
            .ok_or(NativeFeedbackFreezeError::InvalidEventLimit)?;
        Ok(Self {
            lookback_ms,
            source_complete: true,
            source_events: frozen.len(),
            omitted_before_window: 0,
            omitted_untimed: 0,
            omitted_by_event_limit: 0,
            events: frozen,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeFeedbackSummary {
    pub lifecycle: NativeFeedbackLifecycle,
    pub enabled: bool,
    pub accepting: bool,
    pub complete: bool,
    pub events: usize,
    pub candidate_pages: usize,
    pub commits: usize,
    pub cancellations: usize,
    pub popup_timing_samples: usize,
    pub context_suppressions: usize,
    pub private_bytes: usize,
    pub half_pair_gap_samples: usize,
    pub half_pair_gap_histogram: [usize; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
    pub rolling_evictions: usize,
    pub stop_reason: Option<NativeFeedbackStopReason>,
}

struct PendingHalfPairTiming {
    code: String,
    monotonic_ms: u64,
}

struct PendingAutomaticTranspositionFeedback {
    code: String,
    decision: NativeAutomaticTranspositionDecision,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum NativeFeedbackRetention {
    #[default]
    StopAtLimit,
    Rolling,
}

/// An explicitly started, bounded, in-memory feedback session.
///
/// The default is disabled. The session never evicts old events while
/// reporting itself complete: hitting a bound stops recording and marks the
/// summary incomplete.
#[derive(Default)]
pub struct NativeFeedbackSession {
    limits: NativeFeedbackLimits,
    retention: NativeFeedbackRetention,
    enabled: bool,
    accepting: bool,
    complete: bool,
    stop_reason: Option<NativeFeedbackStopReason>,
    context_suppressions: usize,
    private_bytes: usize,
    half_pair_gap_samples: usize,
    half_pair_gap_histogram: [usize; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
    pending_half_pair_timing: Option<PendingHalfPairTiming>,
    transposition_calibrator: TranspositionCalibrator,
    pending_automatic_transposition: Option<PendingAutomaticTranspositionFeedback>,
    rolling_evictions: usize,
    last_evicted_monotonic_ms: Option<u64>,
    evicted_untimed: bool,
    events: Vec<NativeFeedbackEvent>,
    event_monotonic_ms: Vec<Option<u64>>,
}

impl NativeFeedbackSession {
    pub fn start_memory(
        &mut self,
        authorization: NativeFeedbackAuthorization,
        limits: NativeFeedbackLimits,
    ) -> NativeFeedbackStartResult {
        self.start_memory_with_retention(
            authorization,
            limits,
            NativeFeedbackRetention::StopAtLimit,
        )
    }

    /// Starts a bounded rolling-memory session.
    ///
    /// When a configured bound is reached, an amortized batch of the oldest
    /// events is removed instead of stopping the host. No event is persisted
    /// by this operation; an explicit freeze authorization is still required.
    pub fn start_rolling_memory(
        &mut self,
        authorization: NativeFeedbackAuthorization,
        limits: NativeFeedbackLimits,
    ) -> NativeFeedbackStartResult {
        self.start_memory_with_retention(authorization, limits, NativeFeedbackRetention::Rolling)
    }

    fn start_memory_with_retention(
        &mut self,
        _authorization: NativeFeedbackAuthorization,
        limits: NativeFeedbackLimits,
        retention: NativeFeedbackRetention,
    ) -> NativeFeedbackStartResult {
        if self.enabled {
            return if self.accepting {
                NativeFeedbackStartResult::AlreadyRecording
            } else {
                NativeFeedbackStartResult::PreviousSessionRetained
            };
        }
        self.limits = limits;
        self.retention = retention;
        self.enabled = true;
        self.accepting = true;
        self.complete = true;
        self.stop_reason = None;
        self.context_suppressions = 0;
        self.private_bytes = 0;
        self.half_pair_gap_samples = 0;
        self.half_pair_gap_histogram = [0; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS];
        self.pending_half_pair_timing = None;
        self.transposition_calibrator = TranspositionCalibrator::default();
        self.pending_automatic_transposition = None;
        self.rolling_evictions = 0;
        self.last_evicted_monotonic_ms = None;
        self.evicted_untimed = false;
        self.events.clear();
        self.event_monotonic_ms.clear();
        NativeFeedbackStartResult::Started
    }

    pub fn stop(&mut self) -> NativeFeedbackStopResult {
        if !self.enabled {
            return NativeFeedbackStopResult::Disabled;
        }
        if !self.accepting {
            return NativeFeedbackStopResult::AlreadyStopped;
        }
        self.accepting = false;
        self.pending_half_pair_timing = None;
        self.finish_pending_automatic_transposition(TranspositionCalibrationLabel::Unknown);
        NativeFeedbackStopResult::Stopped
    }

    pub fn clear_stopped(&mut self) -> NativeFeedbackClearResult {
        if self.accepting {
            return NativeFeedbackClearResult::StillRecording;
        }
        if !self.enabled {
            return NativeFeedbackClearResult::AlreadyDisabled;
        }
        *self = Self::default();
        NativeFeedbackClearResult::Cleared
    }

    pub fn record(
        &mut self,
        context: NativeFeedbackContext,
        event: NativeFeedbackEvent,
    ) -> NativeFeedbackRecordResult {
        self.record_inner(context, event, None)
    }

    /// Records an event together with a host-supplied monotonic timestamp.
    ///
    /// The timestamp updates the aggregate odd-to-even double-pinyin gap
    /// histogram and is retained only while this explicitly started private
    /// session remains in memory. A frozen snapshot converts it to age relative
    /// to an explicit marker and never exposes the absolute value.
    pub fn record_at(
        &mut self,
        context: NativeFeedbackContext,
        event: NativeFeedbackEvent,
        monotonic_ms: u64,
    ) -> NativeFeedbackRecordResult {
        self.record_inner(context, event, Some(monotonic_ms))
    }

    fn record_inner(
        &mut self,
        context: NativeFeedbackContext,
        event: NativeFeedbackEvent,
        monotonic_ms: Option<u64>,
    ) -> NativeFeedbackRecordResult {
        if !self.enabled {
            return NativeFeedbackRecordResult::Disabled;
        }
        if !self.accepting {
            return self
                .stop_reason
                .map_or(NativeFeedbackRecordResult::NotAccepting, |reason| {
                    NativeFeedbackRecordResult::Stopped(reason)
                });
        }
        if context != NativeFeedbackContext::Eligible {
            self.pending_half_pair_timing = None;
            self.finish_pending_automatic_transposition(TranspositionCalibrationLabel::Unknown);
            self.context_suppressions = self.context_suppressions.saturating_add(1);
            return NativeFeedbackRecordResult::Suppressed(context);
        }
        let Some(event_bytes) = event.validate_and_measure() else {
            return self.stop_incomplete(NativeFeedbackStopReason::InvalidEvent);
        };
        if self.retention == NativeFeedbackRetention::Rolling
            && !self.make_rolling_room(event_bytes)
        {
            return self.stop_incomplete(if event_bytes > self.limits.max_private_bytes {
                NativeFeedbackStopReason::PrivateByteLimit
            } else {
                NativeFeedbackStopReason::EventLimit
            });
        }
        if self.events.len() >= self.limits.max_events {
            return self.stop_incomplete(NativeFeedbackStopReason::EventLimit);
        }
        let Some(next_private_bytes) = self.private_bytes.checked_add(event_bytes) else {
            return self.stop_incomplete(NativeFeedbackStopReason::PrivateByteLimit);
        };
        if next_private_bytes > self.limits.max_private_bytes {
            return self.stop_incomplete(NativeFeedbackStopReason::PrivateByteLimit);
        }
        self.observe_half_pair_gap(&event, monotonic_ms);
        self.observe_automatic_transposition(&event);
        self.events.push(event);
        self.event_monotonic_ms.push(monotonic_ms);
        self.private_bytes = next_private_bytes;
        NativeFeedbackRecordResult::Recorded
    }

    fn make_rolling_room(&mut self, incoming_private_bytes: usize) -> bool {
        if self.limits.max_events == 0 || incoming_private_bytes > self.limits.max_private_bytes {
            return false;
        }
        while self.events.len() >= self.limits.max_events
            || self
                .private_bytes
                .checked_add(incoming_private_bytes)
                .is_none_or(|next| next > self.limits.max_private_bytes)
        {
            if self.events.is_empty() || self.events.len() != self.event_monotonic_ms.len() {
                return false;
            }
            let remove = (self.events.len() / 8).max(1);
            let removed_private_bytes = self
                .events
                .iter()
                .take(remove)
                .filter_map(NativeFeedbackEvent::validate_and_measure)
                .fold(0_usize, usize::saturating_add);
            let removed_times = self.event_monotonic_ms.drain(..remove).collect::<Vec<_>>();
            self.events.drain(..remove);
            self.private_bytes = self.private_bytes.saturating_sub(removed_private_bytes);
            self.rolling_evictions = self.rolling_evictions.saturating_add(remove);
            for timestamp in removed_times {
                match timestamp {
                    Some(timestamp) => {
                        self.last_evicted_monotonic_ms = Some(
                            self.last_evicted_monotonic_ms
                                .map_or(timestamp, |previous| previous.max(timestamp)),
                        );
                    }
                    None => self.evicted_untimed = true,
                }
            }
        }
        true
    }

    /// Copies only events in the bounded interval ending at `marker_ms`.
    ///
    /// Untimed events are deliberately omitted instead of being guessed into
    /// the recent window. If more than `max_events` qualify, the newest events
    /// are retained and the omission is reported. This operation never changes
    /// the session lifecycle or its stored events.
    pub fn freeze_recent(
        &self,
        _authorization: NativeFeedbackFreezeAuthorization,
        marker_ms: u64,
        lookback_ms: u64,
        max_events: usize,
    ) -> Result<FrozenNativeFeedbackSnapshot, NativeFeedbackFreezeError> {
        if !self.enabled {
            return Err(NativeFeedbackFreezeError::Disabled);
        }
        if !self.accepting {
            return Err(NativeFeedbackFreezeError::NotAccepting);
        }
        if !(1..=MAX_NATIVE_FEEDBACK_WISH_LOOKBACK_MS).contains(&lookback_ms) {
            return Err(NativeFeedbackFreezeError::InvalidLookback);
        }
        if max_events == 0 || max_events > DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS {
            return Err(NativeFeedbackFreezeError::InvalidEventLimit);
        }
        if self.events.len() != self.event_monotonic_ms.len() {
            return Err(NativeFeedbackFreezeError::InvalidEventLimit);
        }

        let window_start = marker_ms.saturating_sub(lookback_ms);
        let mut omitted_before_window = 0_usize;
        let mut omitted_untimed = 0_usize;
        let mut qualifying = Vec::new();
        for (event, timestamp) in self.events.iter().zip(&self.event_monotonic_ms) {
            let Some(timestamp) = timestamp else {
                omitted_untimed = omitted_untimed.saturating_add(1);
                continue;
            };
            if *timestamp > marker_ms {
                return Err(NativeFeedbackFreezeError::FutureTimestamp);
            }
            if *timestamp < window_start {
                omitted_before_window = omitted_before_window.saturating_add(1);
                continue;
            }
            let age = marker_ms - *timestamp;
            let milliseconds_before_marker =
                u32::try_from(age).map_err(|_| NativeFeedbackFreezeError::InvalidLookback)?;
            qualifying.push(FrozenNativeFeedbackEvent {
                milliseconds_before_marker,
                event: event.clone(),
            });
        }
        let omitted_by_event_limit = qualifying.len().saturating_sub(max_events);
        if omitted_by_event_limit != 0 {
            qualifying.drain(..omitted_by_event_limit);
        }
        Ok(FrozenNativeFeedbackSnapshot {
            lookback_ms: u32::try_from(lookback_ms)
                .map_err(|_| NativeFeedbackFreezeError::InvalidLookback)?,
            source_complete: self.complete
                && !self.evicted_untimed
                && self
                    .last_evicted_monotonic_ms
                    .is_none_or(|timestamp| timestamp < window_start),
            source_events: self.events.len(),
            omitted_before_window,
            omitted_untimed,
            omitted_by_event_limit,
            events: qualifying,
        })
    }

    /// Freezes a history window aligned to completed input episodes.
    ///
    /// Candidate updates and paint timings are grouped until a commit or
    /// cancellation completes the episode. The returned window starts with at
    /// most `completed_episodes` completed episodes and may include a final
    /// open tail, such as the `xuy` composition used to open the wish prompt.
    /// Consumers can therefore distinguish the latest completed episode from
    /// the control tail without guessing from a fixed number of seconds.
    pub fn freeze_recent_episodes(
        &self,
        authorization: NativeFeedbackFreezeAuthorization,
        marker_ms: u64,
        max_lookback_ms: u64,
        completed_episodes: usize,
        max_events: usize,
    ) -> Result<Option<FrozenNativeFeedbackSnapshot>, NativeFeedbackFreezeError> {
        if !self.enabled {
            return Err(NativeFeedbackFreezeError::Disabled);
        }
        if !self.accepting {
            return Err(NativeFeedbackFreezeError::NotAccepting);
        }
        if !(1..=MAX_NATIVE_FEEDBACK_WISH_LOOKBACK_MS).contains(&max_lookback_ms) {
            return Err(NativeFeedbackFreezeError::InvalidLookback);
        }
        if completed_episodes == 0
            || max_events == 0
            || max_events > DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS
            || self.events.len() != self.event_monotonic_ms.len()
        {
            return Err(NativeFeedbackFreezeError::InvalidEventLimit);
        }

        let earliest_ms = marker_ms.saturating_sub(max_lookback_ms);
        let mut timed_indices = Vec::new();
        let mut terminal_indices = Vec::new();
        for (index, (event, timestamp)) in
            self.events.iter().zip(&self.event_monotonic_ms).enumerate()
        {
            let Some(timestamp) = timestamp else {
                continue;
            };
            if *timestamp > marker_ms {
                return Err(NativeFeedbackFreezeError::FutureTimestamp);
            }
            if *timestamp < earliest_ms {
                continue;
            }
            timed_indices.push(index);
            if event.completes_input_episode() {
                terminal_indices.push(index);
            }
        }
        if terminal_indices.is_empty() {
            return Ok(None);
        }

        let first_selected_terminal = terminal_indices.len().saturating_sub(completed_episodes);
        let start_index = first_selected_terminal
            .checked_sub(1)
            .and_then(|previous| terminal_indices.get(previous).copied())
            .map(|terminal| terminal.saturating_add(1))
            .or_else(|| timed_indices.first().copied())
            .ok_or(NativeFeedbackFreezeError::InvalidEventLimit)?;
        let start_timestamp = self.event_monotonic_ms[start_index..]
            .iter()
            .flatten()
            .copied()
            .next()
            .ok_or(NativeFeedbackFreezeError::InvalidEventLimit)?;
        let lookback_ms = marker_ms.saturating_sub(start_timestamp).max(1);
        self.freeze_recent(authorization, marker_ms, lookback_ms, max_events)
            .map(Some)
    }

    pub fn summary(&self) -> NativeFeedbackSummary {
        let mut summary = NativeFeedbackSummary {
            lifecycle: if !self.enabled {
                NativeFeedbackLifecycle::Disabled
            } else if self.accepting {
                NativeFeedbackLifecycle::Recording
            } else {
                NativeFeedbackLifecycle::Stopped
            },
            enabled: self.enabled,
            accepting: self.accepting,
            complete: self.complete,
            events: self.events.len(),
            context_suppressions: self.context_suppressions,
            private_bytes: self.private_bytes,
            half_pair_gap_samples: self.half_pair_gap_samples,
            half_pair_gap_histogram: self.half_pair_gap_histogram,
            rolling_evictions: self.rolling_evictions,
            stop_reason: self.stop_reason,
            ..NativeFeedbackSummary::default()
        };
        for event in &self.events {
            match event {
                NativeFeedbackEvent::CandidatesPresented { .. }
                | NativeFeedbackEvent::CandidatesPresentedWithProvenance { .. } => {
                    summary.candidate_pages = summary.candidate_pages.saturating_add(1);
                }
                NativeFeedbackEvent::CandidateCommitted { .. }
                | NativeFeedbackEvent::RawCodeCommitted { .. } => {
                    summary.commits = summary.commits.saturating_add(1);
                }
                NativeFeedbackEvent::CompositionCancelled { .. } => {
                    summary.cancellations = summary.cancellations.saturating_add(1);
                }
                NativeFeedbackEvent::CandidatePopupTiming { .. } => {
                    summary.popup_timing_samples = summary.popup_timing_samples.saturating_add(1);
                }
            }
        }
        summary
    }

    pub fn transposition_calibration_summary(&self) -> TranspositionCalibrationSummary {
        self.transposition_calibrator.summary()
    }

    pub fn automatic_transposition_recommendation(
        &self,
        code: &str,
        syllable_index: usize,
        gap_ms: u32,
        cold_tier: NativeAutomaticTranspositionTier,
    ) -> Option<TranspositionCalibrationRecommendation> {
        if !self.is_accepting() {
            return None;
        }
        let probe = TranspositionCalibrationObservation::from_code(
            code,
            syllable_index,
            gap_ms,
            cold_tier,
            TranspositionCalibrationLabel::Unknown,
        )
        .ok()?;
        Some(self.transposition_calibrator.recommendation(probe))
    }

    pub fn events(&self) -> &[NativeFeedbackEvent] {
        &self.events
    }

    pub fn is_accepting(&self) -> bool {
        self.enabled && self.accepting
    }

    fn stop_incomplete(&mut self, reason: NativeFeedbackStopReason) -> NativeFeedbackRecordResult {
        self.accepting = false;
        self.complete = false;
        self.stop_reason = Some(reason);
        self.pending_half_pair_timing = None;
        self.finish_pending_automatic_transposition(TranspositionCalibrationLabel::Unknown);
        NativeFeedbackRecordResult::Stopped(reason)
    }

    fn observe_half_pair_gap(&mut self, event: &NativeFeedbackEvent, monotonic_ms: Option<u64>) {
        // Paint diagnostics are orthogonal to key-pair timing and can arrive
        // between the odd and even double-pinyin frames.
        if matches!(event, NativeFeedbackEvent::CandidatePopupTiming { .. }) {
            return;
        }
        let (
            NativeFeedbackEvent::CandidatesPresented {
                code,
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                ..
            }
            | NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                ..
            },
            Some(monotonic_ms),
        ) = (event, monotonic_ms)
        else {
            self.pending_half_pair_timing = None;
            return;
        };

        if code.len() % 2 == 1 {
            let repeats_same_frame = self
                .pending_half_pair_timing
                .as_ref()
                .is_some_and(|pending| pending.code == *code);
            self.pending_half_pair_timing = (!repeats_same_frame).then(|| PendingHalfPairTiming {
                code: code.clone(),
                monotonic_ms,
            });
            return;
        }

        let Some(pending) = self.pending_half_pair_timing.take() else {
            return;
        };
        if code.len() != pending.code.len().saturating_add(1) || !code.starts_with(&pending.code) {
            return;
        }
        let Some(gap_ms) = monotonic_ms.checked_sub(pending.monotonic_ms) else {
            return;
        };
        let bucket = NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS
            .iter()
            .position(|upper_bound| gap_ms < *upper_bound)
            .unwrap_or(NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS - 1);
        self.half_pair_gap_samples = self.half_pair_gap_samples.saturating_add(1);
        self.half_pair_gap_histogram[bucket] =
            self.half_pair_gap_histogram[bucket].saturating_add(1);
    }

    fn finish_pending_automatic_transposition(&mut self, label: TranspositionCalibrationLabel) {
        let Some(pending) = self.pending_automatic_transposition.take() else {
            return;
        };
        if let Ok(observation) = TranspositionCalibrationObservation::from_code(
            &pending.code,
            pending.decision.syllable_index(),
            pending.decision.pair_gap_ms(),
            pending.decision.cold_tier(),
            label,
        ) {
            self.transposition_calibrator.observe(observation);
        }
    }

    fn observe_automatic_transposition(&mut self, event: &NativeFeedbackEvent) {
        match event {
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                automatic_transposition: Some(decision),
                ..
            } => {
                if decision.syllable_count() != 1 {
                    self.finish_pending_automatic_transposition(
                        TranspositionCalibrationLabel::Unknown,
                    );
                    return;
                }
                if self
                    .pending_automatic_transposition
                    .as_ref()
                    .is_some_and(|pending| pending.code == *code && pending.decision == *decision)
                {
                    return;
                }
                self.finish_pending_automatic_transposition(TranspositionCalibrationLabel::Unknown);
                self.pending_automatic_transposition =
                    Some(PendingAutomaticTranspositionFeedback {
                        code: code.clone(),
                        decision: decision.clone(),
                    });
            }
            NativeFeedbackEvent::CandidatesPresented { code, .. }
            | NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                automatic_transposition: None,
                ..
            } => {
                if self
                    .pending_automatic_transposition
                    .as_ref()
                    .is_some_and(|pending| pending.code != *code)
                {
                    self.finish_pending_automatic_transposition(
                        TranspositionCalibrationLabel::Unknown,
                    );
                }
            }
            NativeFeedbackEvent::CandidateCommitted { code, text, .. } => {
                let label = self.pending_automatic_transposition.as_ref().map_or(
                    TranspositionCalibrationLabel::Unknown,
                    |pending| {
                        if pending.code == *code
                            && pending.decision.outcome()
                                == NativeAutomaticTranspositionOutcome::RecoveryAvailable
                            && pending.decision.visible_rank().is_some()
                        {
                            if pending.decision.recovered_text() == Some(text.as_str()) {
                                TranspositionCalibrationLabel::Accepted
                            } else {
                                TranspositionCalibrationLabel::Rejected
                            }
                        } else {
                            TranspositionCalibrationLabel::Unknown
                        }
                    },
                );
                self.finish_pending_automatic_transposition(label);
            }
            NativeFeedbackEvent::RawCodeCommitted { .. }
            | NativeFeedbackEvent::CompositionCancelled { .. } => {
                self.finish_pending_automatic_transposition(TranspositionCalibrationLabel::Unknown)
            }
            NativeFeedbackEvent::CandidatePopupTiming { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidatesPresented {
            code: "ab".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()],
            may_have_more: false,
        }
    }

    fn timed_page(code: &str, view: NativeCandidateView, page_start: usize) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidatesPresented {
            code: code.to_owned(),
            view,
            page_start,
            candidates: vec!["甲".to_owned(), "乙".to_owned()],
            may_have_more: false,
        }
    }

    fn start(session: &mut NativeFeedbackSession, limits: NativeFeedbackLimits) {
        assert_eq!(
            session.start_memory(NativeFeedbackAuthorization::explicit_memory_only(), limits),
            NativeFeedbackStartResult::Started
        );
    }

    fn record(
        session: &mut NativeFeedbackSession,
        event: NativeFeedbackEvent,
    ) -> NativeFeedbackRecordResult {
        session.record(NativeFeedbackContext::Eligible, event)
    }

    fn record_at(
        session: &mut NativeFeedbackSession,
        event: NativeFeedbackEvent,
        monotonic_ms: u64,
    ) -> NativeFeedbackRecordResult {
        session.record_at(NativeFeedbackContext::Eligible, event, monotonic_ms)
    }

    #[test]
    fn feedback_is_disabled_until_explicitly_started() {
        let mut session = NativeFeedbackSession::default();
        assert_eq!(
            record(&mut session, page()),
            NativeFeedbackRecordResult::Disabled
        );
        assert_eq!(session.summary(), NativeFeedbackSummary::default());
    }

    #[test]
    fn records_private_events_in_order_and_exposes_only_redacted_counts() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        assert_eq!(
            record(&mut session, page()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CandidateCommitted {
                    code: "ab".to_owned(),
                    text: "乙".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::Numeric,
                    absolute_rank: 2,
                    visible_rank: 2,
                }
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "ju".to_owned(),
                }
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert!(matches!(
            session.events(),
            [
                NativeFeedbackEvent::CandidatesPresented { .. },
                NativeFeedbackEvent::CandidateCommitted { .. },
                NativeFeedbackEvent::RawCodeCommitted { .. }
            ]
        ));
        let summary = session.summary();
        assert_eq!(summary.lifecycle, NativeFeedbackLifecycle::Recording);
        assert!(summary.enabled);
        assert!(summary.accepting);
        assert!(summary.complete);
        assert_eq!(summary.events, 3);
        assert_eq!(summary.candidate_pages, 1);
        assert_eq!(summary.commits, 2);
        assert_eq!(summary.cancellations, 0);
        assert!(summary.private_bytes > 0);
        assert_eq!(summary.stop_reason, None);
    }

    #[test]
    fn popup_timing_is_bounded_diagnostic_metadata() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CandidatePopupTiming {
                    first_frame_ms: 7,
                    fully_visible_ms: 7,
                    initial_show: true,
                }
            ),
            NativeFeedbackRecordResult::Recorded
        );
        let summary = session.summary();
        assert_eq!(summary.events, 1);
        assert_eq!(summary.popup_timing_samples, 1);
        assert_eq!(summary.private_bytes, 0);

        let mut invalid = NativeFeedbackSession::default();
        start(&mut invalid, NativeFeedbackLimits::default());
        assert_eq!(
            record(
                &mut invalid,
                NativeFeedbackEvent::CandidatePopupTiming {
                    first_frame_ms: 8,
                    fully_visible_ms: 7,
                    initial_show: false,
                }
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::InvalidEvent)
        );
    }

    #[test]
    fn candidate_provenance_must_align_with_the_visible_page() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code: "ab".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["甲".to_owned(), "乙".to_owned()],
                    provenance: vec![NativeCandidateProvenance::new(
                        NativeCandidateSource::CoreExact,
                        false,
                    )],
                    automatic_transposition: None,
                    may_have_more: false,
                }
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::InvalidEvent)
        );
    }

    #[test]
    fn automatic_transposition_feedback_requires_consistent_private_evidence() {
        let event = |decision| NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "am".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["马".to_owned(), "俺们".to_owned()],
            provenance: vec![
                NativeCandidateProvenance::new(NativeCandidateSource::TranspositionRecovery, false),
                NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false),
            ],
            automatic_transposition: Some(decision),
            may_have_more: false,
        };

        let mut valid = NativeFeedbackSession::default();
        start(&mut valid, NativeFeedbackLimits::default());
        assert_eq!(
            record(
                &mut valid,
                event(NativeAutomaticTranspositionDecision::new(
                    0,
                    24,
                    NativeAutomaticTranspositionTier::Primary,
                    NativeAutomaticTranspositionTier::Primary,
                    NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                    Some("马".to_owned()),
                    Some(1),
                )),
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert!(valid.summary().private_bytes >= "am马俺们马".len());

        for inconsistent in [
            NativeAutomaticTranspositionDecision::new(
                0,
                24,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                None,
                Some(1),
            ),
            NativeAutomaticTranspositionDecision::new(
                0,
                24,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionOutcome::Suppressed,
                Some("马".to_owned()),
                None,
            ),
            NativeAutomaticTranspositionDecision::new(
                0,
                24,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionTier::Shadow,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                Some("马".to_owned()),
                None,
            ),
            NativeAutomaticTranspositionDecision::new(
                0,
                24,
                NativeAutomaticTranspositionTier::Shadow,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                Some("马".to_owned()),
                Some(1),
            ),
        ] {
            let mut invalid = NativeFeedbackSession::default();
            start(&mut invalid, NativeFeedbackLimits::default());
            assert_eq!(
                record(&mut invalid, event(inconsistent)),
                NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::InvalidEvent)
            );
        }
    }

    #[test]
    fn explicit_recovery_choices_calibrate_the_same_pair_after_the_sample_gate() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        for index in 0..8_u64 {
            assert_eq!(
                record_at(
                    &mut session,
                    NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                        code: "am".to_owned(),
                        view: NativeCandidateView::Ordinary,
                        page_start: 0,
                        candidates: vec!["俺们".to_owned(), "马".to_owned()],
                        provenance: vec![
                            NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false,),
                            NativeCandidateProvenance::new(
                                NativeCandidateSource::TranspositionRecovery,
                                false,
                            ),
                        ],
                        automatic_transposition: Some(NativeAutomaticTranspositionDecision::new(
                            0,
                            55,
                            NativeAutomaticTranspositionTier::Secondary,
                            NativeAutomaticTranspositionTier::Secondary,
                            NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                            Some("马".to_owned()),
                            Some(2),
                        ),),
                        may_have_more: false,
                    },
                    index.saturating_mul(10),
                ),
                NativeFeedbackRecordResult::Recorded
            );
            assert_eq!(
                record_at(
                    &mut session,
                    NativeFeedbackEvent::CandidateCommitted {
                        code: "am".to_owned(),
                        text: "马".to_owned(),
                        view: NativeCandidateView::Ordinary,
                        source: NativeSelectionSource::Numeric,
                        absolute_rank: 2,
                        visible_rank: 2,
                    },
                    index.saturating_mul(10).saturating_add(1),
                ),
                NativeFeedbackRecordResult::Recorded
            );
        }

        assert_eq!(
            session.transposition_calibration_summary(),
            TranspositionCalibrationSummary {
                observations: 8,
                accepted: 8,
                ..TranspositionCalibrationSummary::default()
            }
        );
        let recommendation = session
            .automatic_transposition_recommendation(
                "am",
                0,
                55,
                NativeAutomaticTranspositionTier::Secondary,
            )
            .unwrap();
        assert!(recommendation.personalized);
        assert_eq!(
            recommendation.recommended_tier,
            NativeAutomaticTranspositionTier::Primary
        );
    }

    #[test]
    fn pending_transposition_becomes_unknown_at_stop_suppression_and_capacity_boundaries() {
        let frame = || NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "am".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["俺们".to_owned(), "马".to_owned()],
            provenance: vec![
                NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false),
                NativeCandidateProvenance::new(NativeCandidateSource::TranspositionRecovery, false),
            ],
            automatic_transposition: Some(NativeAutomaticTranspositionDecision::new(
                0,
                55,
                NativeAutomaticTranspositionTier::Secondary,
                NativeAutomaticTranspositionTier::Secondary,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                Some("马".to_owned()),
                Some(2),
            )),
            may_have_more: false,
        };
        let unknown_summary = TranspositionCalibrationSummary {
            observations: 1,
            unknown: 1,
            ..TranspositionCalibrationSummary::default()
        };

        let mut stopped = NativeFeedbackSession::default();
        start(&mut stopped, NativeFeedbackLimits::default());
        assert_eq!(
            record(&mut stopped, frame()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(stopped.stop(), NativeFeedbackStopResult::Stopped);
        assert_eq!(stopped.transposition_calibration_summary(), unknown_summary);

        let mut suppressed = NativeFeedbackSession::default();
        start(&mut suppressed, NativeFeedbackLimits::default());
        assert_eq!(
            record(&mut suppressed, frame()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            suppressed.record(
                NativeFeedbackContext::Password,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "am".to_owned(),
                },
            ),
            NativeFeedbackRecordResult::Suppressed(NativeFeedbackContext::Password)
        );
        assert_eq!(
            suppressed.transposition_calibration_summary(),
            unknown_summary
        );

        let mut bounded = NativeFeedbackSession::default();
        start(
            &mut bounded,
            NativeFeedbackLimits {
                max_events: 1,
                max_private_bytes: 1_024,
            },
        );
        assert_eq!(
            record(&mut bounded, frame()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record(
                &mut bounded,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "am".to_owned(),
                },
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::EventLimit)
        );
        assert_eq!(bounded.transposition_calibration_summary(), unknown_summary);
    }

    #[test]
    fn seven_candidate_pages_and_the_seventh_numeric_rank_are_valid() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        let candidates = (1..=7).map(|rank| format!("候选{rank}")).collect();
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "ab".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates,
                    may_have_more: true,
                }
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CandidateCommitted {
                    code: "ab".to_owned(),
                    text: "候选7".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::Numeric,
                    absolute_rank: 7,
                    visible_rank: 7,
                }
            ),
            NativeFeedbackRecordResult::Recorded
        );

        let mut oversized = NativeFeedbackSession::default();
        start(&mut oversized, NativeFeedbackLimits::default());
        let candidates = (1..=8).map(|rank| format!("候选{rank}")).collect();
        assert_eq!(
            record(
                &mut oversized,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "ab".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates,
                    may_have_more: true,
                }
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::InvalidEvent)
        );
    }

    #[test]
    fn reaching_a_bound_stops_instead_of_silently_dropping_events() {
        let mut session = NativeFeedbackSession::default();
        start(
            &mut session,
            NativeFeedbackLimits {
                max_events: 1,
                max_private_bytes: 1024,
            },
        );
        assert_eq!(
            record(&mut session, page()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CompositionCancelled {
                    code: "ab".to_owned(),
                    source: NativeCancellationSource::Escape,
                }
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::EventLimit)
        );
        let summary = session.summary();
        assert_eq!(summary.lifecycle, NativeFeedbackLifecycle::Stopped);
        assert!(!summary.accepting);
        assert!(!summary.complete);
        assert_eq!(summary.events, 1);
        assert_eq!(
            summary.stop_reason,
            Some(NativeFeedbackStopReason::EventLimit)
        );
    }

    #[test]
    fn rolling_memory_evicts_oldest_events_and_keeps_recent_window_completeness_honest() {
        let mut session = NativeFeedbackSession::default();
        assert_eq!(
            session.start_rolling_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits {
                    max_events: 2,
                    max_private_bytes: 1024,
                },
            ),
            NativeFeedbackStartResult::Started
        );
        for (code, timestamp) in [("aa", 100), ("bb", 200), ("cc", 300)] {
            assert_eq!(
                record_at(
                    &mut session,
                    NativeFeedbackEvent::RawCodeCommitted {
                        code: code.to_owned(),
                    },
                    timestamp,
                ),
                NativeFeedbackRecordResult::Recorded
            );
        }

        let summary = session.summary();
        assert_eq!(summary.lifecycle, NativeFeedbackLifecycle::Recording);
        assert_eq!(summary.events, 2);
        assert_eq!(summary.rolling_evictions, 1);
        assert!(summary.complete);
        assert_eq!(summary.stop_reason, None);

        let recent = session
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                300,
                150,
                2,
            )
            .unwrap();
        assert!(recent.source_complete());
        assert_eq!(recent.events().len(), 2);

        let overlapping_eviction = session
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                300,
                250,
                2,
            )
            .unwrap();
        assert!(!overlapping_eviction.source_complete());
    }

    #[test]
    fn invalid_private_payload_stops_without_recording_it() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        assert_eq!(
            record(
                &mut session,
                NativeFeedbackEvent::CompositionCancelled {
                    code: "含中文".to_owned(),
                    source: NativeCancellationSource::Backspace,
                }
            ),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::InvalidEvent)
        );
        assert!(session.events().is_empty());
        assert!(!session.summary().complete);
    }

    #[test]
    fn private_byte_limit_is_hard_and_an_explicit_stop_is_not_an_overflow() {
        let mut bounded = NativeFeedbackSession::default();
        start(
            &mut bounded,
            NativeFeedbackLimits {
                max_events: 10,
                max_private_bytes: 1,
            },
        );
        assert_eq!(
            record(&mut bounded, page()),
            NativeFeedbackRecordResult::Stopped(NativeFeedbackStopReason::PrivateByteLimit)
        );
        assert!(bounded.events().is_empty());
        assert_eq!(
            bounded.summary().stop_reason,
            Some(NativeFeedbackStopReason::PrivateByteLimit)
        );

        let mut stopped = NativeFeedbackSession::default();
        start(&mut stopped, NativeFeedbackLimits::default());
        stopped.stop();
        assert_eq!(
            record(&mut stopped, page()),
            NativeFeedbackRecordResult::NotAccepting
        );
        assert!(stopped.summary().complete);
        assert_eq!(stopped.summary().stop_reason, None);
    }

    #[test]
    fn lifecycle_refuses_to_overwrite_a_live_or_stopped_session() {
        let mut session = NativeFeedbackSession::default();
        assert_eq!(session.stop(), NativeFeedbackStopResult::Disabled);
        assert_eq!(
            session.clear_stopped(),
            NativeFeedbackClearResult::AlreadyDisabled
        );

        start(&mut session, NativeFeedbackLimits::default());
        assert_eq!(
            record(&mut session, page()),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            session.start_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits {
                    max_events: 0,
                    max_private_bytes: 0,
                }
            ),
            NativeFeedbackStartResult::AlreadyRecording
        );
        assert_eq!(session.summary().events, 1);
        assert_eq!(
            session.clear_stopped(),
            NativeFeedbackClearResult::StillRecording
        );

        assert_eq!(session.stop(), NativeFeedbackStopResult::Stopped);
        assert_eq!(session.stop(), NativeFeedbackStopResult::AlreadyStopped);
        assert_eq!(
            session.start_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits::default()
            ),
            NativeFeedbackStartResult::PreviousSessionRetained
        );
        assert_eq!(session.summary().events, 1);

        assert_eq!(session.clear_stopped(), NativeFeedbackClearResult::Cleared);
        assert_eq!(
            session.summary().lifecycle,
            NativeFeedbackLifecycle::Disabled
        );
        start(&mut session, NativeFeedbackLimits::default());
    }

    #[test]
    fn context_must_be_explicitly_eligible_and_suppression_is_redacted() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        for context in [
            NativeFeedbackContext::Unknown,
            NativeFeedbackContext::Password,
            NativeFeedbackContext::Private,
            NativeFeedbackContext::KeyboardDisabled,
            NativeFeedbackContext::Empty,
            NativeFeedbackContext::Restricted,
        ] {
            assert_eq!(
                session.record(context, page()),
                NativeFeedbackRecordResult::Suppressed(context)
            );
        }
        assert!(session.events().is_empty());
        let summary = session.summary();
        assert!(summary.complete);
        assert!(summary.accepting);
        assert_eq!(summary.context_suppressions, 6);
        assert_eq!(summary.private_bytes, 0);
    }

    #[test]
    fn ordinary_odd_to_even_prefix_records_one_aggregate_gap() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());

        assert_eq!(
            record_at(
                &mut session,
                timed_page("a", NativeCandidateView::Ordinary, 0),
                100,
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record_at(
                &mut session,
                NativeFeedbackEvent::CandidatePopupTiming {
                    first_frame_ms: 5,
                    fully_visible_ms: 5,
                    initial_show: true,
                },
                105,
            ),
            NativeFeedbackRecordResult::Recorded
        );
        assert_eq!(
            record_at(
                &mut session,
                timed_page("ab", NativeCandidateView::Ordinary, 0),
                124,
            ),
            NativeFeedbackRecordResult::Recorded
        );

        let summary = session.summary();
        assert_eq!(summary.half_pair_gap_samples, 1);
        assert_eq!(summary.half_pair_gap_histogram, [0, 0, 0, 1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn half_pair_gap_buckets_use_the_documented_exclusive_upper_bounds() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        for (index, gap_ms) in [0, 8, 16, 24, 32, 48, 64, 96, 160].into_iter().enumerate() {
            let base = u64::try_from(index).unwrap() * 1_000;
            assert_eq!(
                record_at(
                    &mut session,
                    timed_page("a", NativeCandidateView::Ordinary, 0),
                    base,
                ),
                NativeFeedbackRecordResult::Recorded
            );
            assert_eq!(
                record_at(
                    &mut session,
                    timed_page("ab", NativeCandidateView::Ordinary, 0),
                    base + gap_ms,
                ),
                NativeFeedbackRecordResult::Recorded
            );
        }

        let summary = session.summary();
        assert_eq!(summary.half_pair_gap_samples, 9);
        assert_eq!(summary.half_pair_gap_histogram, [1; 9]);
    }

    #[test]
    fn even_to_odd_and_page_only_updates_do_not_create_gap_samples() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            timed_page("ab", NativeCandidateView::Ordinary, 0),
            0,
        );
        record_at(
            &mut session,
            timed_page("abc", NativeCandidateView::Ordinary, 0),
            10,
        );
        record_at(
            &mut session,
            timed_page("abc", NativeCandidateView::Ordinary, 6),
            15,
        );
        record_at(
            &mut session,
            timed_page("abcd", NativeCandidateView::Ordinary, 0),
            20,
        );

        let summary = session.summary();
        assert_eq!(summary.half_pair_gap_samples, 0);
        assert_eq!(summary.half_pair_gap_histogram, [0; 9]);
    }

    #[test]
    fn commit_cancel_and_nonordinary_views_break_timing_pairs() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            timed_page("a", NativeCandidateView::Ordinary, 0),
            0,
        );
        record_at(
            &mut session,
            NativeFeedbackEvent::CandidateCommitted {
                code: "a".to_owned(),
                text: "甲".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
            1,
        );
        record_at(
            &mut session,
            timed_page("ab", NativeCandidateView::Ordinary, 0),
            2,
        );
        record_at(
            &mut session,
            timed_page("abc", NativeCandidateView::Ordinary, 0),
            3,
        );
        record_at(
            &mut session,
            NativeFeedbackEvent::CompositionCancelled {
                code: "abc".to_owned(),
                source: NativeCancellationSource::Escape,
            },
            4,
        );
        record_at(
            &mut session,
            timed_page("abcd", NativeCandidateView::Ordinary, 0),
            5,
        );
        record_at(
            &mut session,
            timed_page("abcde", NativeCandidateView::Ordinary, 0),
            6,
        );
        record_at(
            &mut session,
            timed_page("abcde", NativeCandidateView::Shape, 0),
            7,
        );
        record_at(
            &mut session,
            timed_page("abcdef", NativeCandidateView::Ordinary, 0),
            8,
        );

        assert_eq!(session.summary().half_pair_gap_samples, 0);
    }

    #[test]
    fn nonmonotonic_time_and_suppressed_context_fail_closed() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            timed_page("a", NativeCandidateView::Ordinary, 0),
            10,
        );
        record_at(
            &mut session,
            timed_page("ab", NativeCandidateView::Ordinary, 0),
            9,
        );
        record_at(
            &mut session,
            timed_page("abc", NativeCandidateView::Ordinary, 0),
            20,
        );
        assert_eq!(
            session.record_at(
                NativeFeedbackContext::Password,
                timed_page("abcd", NativeCandidateView::Ordinary, 0),
                30,
            ),
            NativeFeedbackRecordResult::Suppressed(NativeFeedbackContext::Password)
        );
        record_at(
            &mut session,
            timed_page("abcd", NativeCandidateView::Ordinary, 0),
            31,
        );

        let summary = session.summary();
        assert_eq!(summary.half_pair_gap_samples, 0);
        assert_eq!(summary.context_suppressions, 1);
    }

    #[test]
    fn redacted_summary_debug_contains_neither_codes_nor_candidate_text() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            NativeFeedbackEvent::CandidatesPresented {
                code: "privatecode".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["仅供合成测试".to_owned()],
                may_have_more: false,
            },
            10,
        );

        let debug = format!("{:?}", session.summary());
        assert!(!debug.contains("privatecode"));
        assert!(!debug.contains("仅供合成测试"));
        assert!(debug.contains("half_pair_gap_histogram"));
    }

    #[test]
    fn recent_freeze_keeps_only_the_requested_timed_window() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            timed_page("a", NativeCandidateView::Ordinary, 0),
            1_000,
        );
        record_at(
            &mut session,
            timed_page("ab", NativeCandidateView::Ordinary, 0),
            40_000,
        );
        record_at(
            &mut session,
            timed_page("abc", NativeCandidateView::Ordinary, 0),
            69_999,
        );

        let before = session.summary();
        let frozen = session
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                70_000,
                DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
                DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
            )
            .unwrap();
        assert_eq!(frozen.lookback_ms(), 30_000);
        assert_eq!(frozen.source_events(), 3);
        assert!(frozen.source_complete());
        assert_eq!(frozen.omitted_before_window(), 1);
        assert_eq!(frozen.omitted_untimed(), 0);
        assert_eq!(frozen.omitted_by_event_limit(), 0);
        assert_eq!(frozen.events().len(), 2);
        assert_eq!(frozen.events()[0].milliseconds_before_marker(), 30_000);
        assert_eq!(frozen.events()[1].milliseconds_before_marker(), 1);
        assert_eq!(
            session.summary(),
            before,
            "freezing must not mutate lifecycle"
        );
    }

    #[test]
    fn episode_freeze_keeps_the_latest_completed_input_and_the_open_wish_tail() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        for (code, text, started, committed) in [("ab", "甲", 100, 110), ("cd", "乙", 200, 210)] {
            record_at(
                &mut session,
                timed_page(code, NativeCandidateView::Ordinary, 0),
                started,
            );
            record_at(
                &mut session,
                NativeFeedbackEvent::CandidateCommitted {
                    code: code.to_owned(),
                    text: text.to_owned(),
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::FirstCandidate,
                    absolute_rank: 1,
                    visible_rank: 1,
                },
                committed,
            );
        }
        record_at(
            &mut session,
            timed_page("xuy", NativeCandidateView::Ordinary, 0),
            220,
        );

        let frozen = session
            .freeze_recent_episodes(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                230,
                1_000,
                1,
                128,
            )
            .unwrap()
            .expect("one completed episode is available");
        assert_eq!(frozen.lookback_ms(), 30);
        assert_eq!(frozen.events().len(), 3);
        assert!(matches!(
            frozen.events()[0].event(),
            NativeFeedbackEvent::CandidatesPresented { code, .. } if code == "cd"
        ));
        assert!(matches!(
            frozen.events()[1].event(),
            NativeFeedbackEvent::CandidateCommitted { code, .. } if code == "cd"
        ));
        assert!(matches!(
            frozen.events()[2].event(),
            NativeFeedbackEvent::CandidatesPresented { code, .. } if code == "xuy"
        ));
    }

    #[test]
    fn episode_freeze_reports_no_selection_before_any_completed_input() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(
            &mut session,
            timed_page("xuy", NativeCandidateView::Ordinary, 0),
            10,
        );
        assert!(
            session
                .freeze_recent_episodes(
                    NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                    20,
                    1_000,
                    3,
                    128,
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn recent_freeze_omits_untimed_events_and_keeps_newest_when_bounded() {
        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record(&mut session, page());
        for (index, code) in ["a", "ab", "abc"].into_iter().enumerate() {
            record_at(
                &mut session,
                timed_page(code, NativeCandidateView::Ordinary, 0),
                100 + u64::try_from(index).unwrap(),
            );
        }

        let frozen = session
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                102,
                30_000,
                2,
            )
            .unwrap();
        assert_eq!(frozen.omitted_untimed(), 1);
        assert_eq!(frozen.omitted_by_event_limit(), 1);
        assert_eq!(frozen.events().len(), 2);
        assert_eq!(frozen.events()[0].milliseconds_before_marker(), 1);
        assert_eq!(frozen.events()[1].milliseconds_before_marker(), 0);
    }

    #[test]
    fn recent_freeze_requires_a_live_session_and_valid_bounds() {
        let session = NativeFeedbackSession::default();
        let authorization = NativeFeedbackFreezeAuthorization::explicit_private_snapshot();
        assert!(matches!(
            session.freeze_recent(authorization, 0, 30_000, 10),
            Err(NativeFeedbackFreezeError::Disabled)
        ));

        let mut session = NativeFeedbackSession::default();
        start(&mut session, NativeFeedbackLimits::default());
        record_at(&mut session, page(), 20);
        assert!(matches!(
            session.freeze_recent(authorization, 19, 30_000, 10),
            Err(NativeFeedbackFreezeError::FutureTimestamp)
        ));
        assert!(matches!(
            session.freeze_recent(authorization, 20, 0, 10),
            Err(NativeFeedbackFreezeError::InvalidLookback)
        ));
        assert!(matches!(
            session.freeze_recent(authorization, 20, 30_000, 0),
            Err(NativeFeedbackFreezeError::InvalidEventLimit)
        ));
        session.stop();
        assert!(matches!(
            session.freeze_recent(authorization, 20, 30_000, 10),
            Err(NativeFeedbackFreezeError::NotAccepting)
        ));
    }
}
