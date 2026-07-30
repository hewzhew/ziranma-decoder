//! Bounded, memory-only feedback emitted by an input method itself.
//!
//! This module deliberately contains no host APIs, serialization, file I/O,
//! networking, or background work. A host must explicitly start a session and
//! feed it semantic events only after the corresponding user-visible action
//! succeeds.

const MAX_FEEDBACK_CODE_BYTES: usize = 64;
const MAX_FEEDBACK_CANDIDATES_PER_PAGE: usize = 7;
const MAX_FEEDBACK_TEXT_CHARACTERS: usize = 128;

pub const DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS: usize = 4_096;
pub const DEFAULT_NATIVE_FEEDBACK_MAX_PRIVATE_BYTES: usize = 1024 * 1024;

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
}

impl NativeFeedbackEvent {
    fn validate_and_measure(&self) -> Option<usize> {
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
        }
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
    pub context_suppressions: usize,
    pub private_bytes: usize,
    pub stop_reason: Option<NativeFeedbackStopReason>,
}

/// An explicitly started, bounded, in-memory feedback session.
///
/// The default is disabled. The session never evicts old events while
/// reporting itself complete: hitting a bound stops recording and marks the
/// summary incomplete.
#[derive(Default)]
pub struct NativeFeedbackSession {
    limits: NativeFeedbackLimits,
    enabled: bool,
    accepting: bool,
    complete: bool,
    stop_reason: Option<NativeFeedbackStopReason>,
    context_suppressions: usize,
    private_bytes: usize,
    events: Vec<NativeFeedbackEvent>,
}

impl NativeFeedbackSession {
    pub fn start_memory(
        &mut self,
        _authorization: NativeFeedbackAuthorization,
        limits: NativeFeedbackLimits,
    ) -> NativeFeedbackStartResult {
        if self.enabled {
            return if self.accepting {
                NativeFeedbackStartResult::AlreadyRecording
            } else {
                NativeFeedbackStartResult::PreviousSessionRetained
            };
        }
        self.limits = limits;
        self.enabled = true;
        self.accepting = true;
        self.complete = true;
        self.stop_reason = None;
        self.context_suppressions = 0;
        self.private_bytes = 0;
        self.events.clear();
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
            self.context_suppressions = self.context_suppressions.saturating_add(1);
            return NativeFeedbackRecordResult::Suppressed(context);
        }
        let Some(event_bytes) = event.validate_and_measure() else {
            return self.stop_incomplete(NativeFeedbackStopReason::InvalidEvent);
        };
        if self.events.len() >= self.limits.max_events {
            return self.stop_incomplete(NativeFeedbackStopReason::EventLimit);
        }
        let Some(next_private_bytes) = self.private_bytes.checked_add(event_bytes) else {
            return self.stop_incomplete(NativeFeedbackStopReason::PrivateByteLimit);
        };
        if next_private_bytes > self.limits.max_private_bytes {
            return self.stop_incomplete(NativeFeedbackStopReason::PrivateByteLimit);
        }
        self.events.push(event);
        self.private_bytes = next_private_bytes;
        NativeFeedbackRecordResult::Recorded
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
            stop_reason: self.stop_reason,
            ..NativeFeedbackSummary::default()
        };
        for event in &self.events {
            match event {
                NativeFeedbackEvent::CandidatesPresented { .. } => {
                    summary.candidate_pages = summary.candidate_pages.saturating_add(1);
                }
                NativeFeedbackEvent::CandidateCommitted { .. }
                | NativeFeedbackEvent::RawCodeCommitted { .. } => {
                    summary.commits = summary.commits.saturating_add(1);
                }
                NativeFeedbackEvent::CompositionCancelled { .. } => {
                    summary.cancellations = summary.cancellations.saturating_add(1);
                }
            }
        }
        summary
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
        NativeFeedbackRecordResult::Stopped(reason)
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
}
