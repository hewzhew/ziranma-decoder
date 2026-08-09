//! Explicit, local-only wish snapshots protected for the current Windows user.
//!
//! The module accepts only an explicitly frozen native-feedback snapshot. It
//! neither discovers capture data nor starts feedback, and it performs no
//! networking. Private event types intentionally omit `Debug`.

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

use crate::candidate_snapshot::valid_candidate_snapshot_revision;
use crate::{
    DataProtector, FrozenNativeFeedbackEvent, FrozenNativeFeedbackSnapshot,
    NativeAutomaticTranspositionDecision, NativeAutomaticTranspositionOutcome,
    NativeAutomaticTranspositionTier, NativeCancellationSource, NativeCandidateProvenance,
    NativeCandidateSource, NativeCandidateView, NativeFeedbackEvent, NativeSelectionSource,
    NativeTabAssemblyState, TranspositionCalibrationLabel, TranspositionCalibrationObservation,
    candidate_sha256_hex,
};

pub const WISH_SCHEMA_V1: &str = "ziranma-wish-v1";
pub const WISH_SCHEMA_V2: &str = "ziranma-wish-v2";
pub const WISH_SCHEMA_V3: &str = "ziranma-wish-v3";
pub const WISH_SCHEMA_V4: &str = "ziranma-wish-v4";
pub const WISH_SCHEMA_V5: &str = "ziranma-wish-v5";
pub const WISH_SCHEMA_V6: &str = "ziranma-wish-v6";
pub const WISH_SCHEMA_V7: &str = "ziranma-wish-v7";
pub const WISH_PACKAGE_FILE_SUFFIX: &str = ".ziw";
pub const WISH_NOTE_FILE_SUFFIX: &str = ".note.ziw";
pub const MAX_WISH_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_WISH_NOTE_BYTES: usize = 8 * 1024;

const MAX_WISH_EVENTS: usize = 4_096;
const MAX_WISH_PLAINTEXT_BYTES: usize = 1536 * 1024;
const MAX_WISH_STRING_BYTES: usize = 64 * 1024;
const WISH_PLAINTEXT_MAGIC_V1: &[u8] = b"ziranma-wish-v1\0";
const WISH_PLAINTEXT_MAGIC_V2: &[u8] = b"ziranma-wish-v2\0";
const WISH_PLAINTEXT_MAGIC_V3: &[u8] = b"ziranma-wish-v3\0";
const WISH_PLAINTEXT_MAGIC_V4: &[u8] = b"ziranma-wish-v4\0";
const WISH_PLAINTEXT_MAGIC_V5: &[u8] = b"ziranma-wish-v5\0";
const WISH_PLAINTEXT_MAGIC_V6: &[u8] = b"ziranma-wish-v6\0";
const WISH_PLAINTEXT_MAGIC_V7: &[u8] = b"ziranma-wish-v7\0";
const WISH_PROTECTED_MAGIC: &[u8] = b"ziranma-wish-dpapi-v1\0";
const WISH_NOTE_PLAINTEXT_MAGIC: &[u8] = b"ziranma-wish-note-v1\0";
const WISH_NOTE_PROTECTED_MAGIC: &[u8] = b"ziranma-wish-note-dpapi-v1\0";
const WISH_ID_PREFIX: &str = "wish-";
const WISH_ID_HEX_BYTES: usize = 64;
const WISH_TRASH_DIRECTORY: &str = "trash";
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishCaptureScope {
    LegacyWindow,
    RecentEpisodes,
    RecentWindow,
    ContinuousJournal,
}

impl WishCaptureScope {
    pub fn slug(self) -> &'static str {
        match self {
            Self::LegacyWindow => "legacy-window",
            Self::RecentEpisodes => "recent-episodes",
            Self::RecentWindow => "recent-window",
            Self::ContinuousJournal => "continuous-journal",
        }
    }

    fn encoded(self) -> u8 {
        match self {
            Self::LegacyWindow => 1,
            Self::RecentEpisodes => 2,
            Self::RecentWindow => 3,
            Self::ContinuousJournal => 4,
        }
    }

    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::LegacyWindow),
            2 => Some(Self::RecentEpisodes),
            3 => Some(Self::RecentWindow),
            4 => Some(Self::ContinuousJournal),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishEventRole {
    Context,
    Focus,
    Trigger,
}

/// One private event loaded from or prepared for a wish package.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishEvent {
    milliseconds_before_marker: u32,
    event: NativeFeedbackEvent,
}

/// Non-private immutable runtime identity attached to one newly captured batch.
///
/// The DLL digest distinguishes source builds even when the Cargo package
/// version is unchanged. Candidate revisions distinguish independently
/// replaceable public data. Older wish formats intentionally leave this absent.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WishRuntimeIdentity {
    module_sha256: String,
    core_candidate_revision: String,
    supplemental_candidate_revision: Option<String>,
}

impl WishRuntimeIdentity {
    pub fn new(
        module_sha256: String,
        core_candidate_revision: String,
        supplemental_candidate_revision: Option<String>,
    ) -> Result<Self, WishFeedbackError> {
        let value = Self {
            module_sha256,
            core_candidate_revision,
            supplemental_candidate_revision,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn module_sha256(&self) -> &str {
        &self.module_sha256
    }

    pub fn core_candidate_revision(&self) -> &str {
        &self.core_candidate_revision
    }

    pub fn supplemental_candidate_revision(&self) -> Option<&str> {
        self.supplemental_candidate_revision.as_deref()
    }

    fn validate(&self) -> Result<(), WishFeedbackError> {
        if self.module_sha256.len() != 64
            || !self
                .module_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !valid_candidate_snapshot_revision(&self.core_candidate_revision)
            || self
                .supplemental_candidate_revision
                .as_deref()
                .is_some_and(|revision| !valid_candidate_snapshot_revision(revision))
        {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        Ok(())
    }
}

impl WishEvent {
    pub fn milliseconds_before_marker(&self) -> u32 {
        self.milliseconds_before_marker
    }

    pub fn event(&self) -> &NativeFeedbackEvent {
        &self.event
    }
}

/// Canonical private contents of one local wish.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishSnapshot {
    capture_scope: WishCaptureScope,
    category: WishCategory,
    runtime_identity: Option<WishRuntimeIdentity>,
    focus_event_start: usize,
    focus_event_count: usize,
    lookback_ms: u32,
    source_complete: bool,
    source_events: usize,
    omitted_before_window: usize,
    omitted_untimed: usize,
    omitted_by_event_limit: usize,
    events: Vec<WishEvent>,
}

impl WishSnapshot {
    pub fn from_frozen(snapshot: &FrozenNativeFeedbackSnapshot) -> Result<Self, WishFeedbackError> {
        Self::from_frozen_with_metadata(
            snapshot,
            WishCaptureScope::RecentWindow,
            WishCategory::Other,
        )
    }

    pub fn from_frozen_with_metadata(
        snapshot: &FrozenNativeFeedbackSnapshot,
        capture_scope: WishCaptureScope,
        category: WishCategory,
    ) -> Result<Self, WishFeedbackError> {
        Self::from_frozen_with_runtime_identity(snapshot, capture_scope, category, None)
    }

    pub fn from_frozen_with_runtime_identity(
        snapshot: &FrozenNativeFeedbackSnapshot,
        capture_scope: WishCaptureScope,
        category: WishCategory,
        runtime_identity: Option<WishRuntimeIdentity>,
    ) -> Result<Self, WishFeedbackError> {
        let (focus_event_start, focus_event_count) =
            if capture_scope == WishCaptureScope::ContinuousJournal {
                (0, snapshot.events().len())
            } else {
                latest_completed_episode_range(snapshot.events())
            };
        let value = Self {
            capture_scope,
            category,
            runtime_identity,
            focus_event_start,
            focus_event_count,
            lookback_ms: snapshot.lookback_ms(),
            source_complete: snapshot.source_complete(),
            source_events: snapshot.source_events(),
            omitted_before_window: snapshot.omitted_before_window(),
            omitted_untimed: snapshot.omitted_untimed(),
            omitted_by_event_limit: snapshot.omitted_by_event_limit(),
            events: snapshot
                .events()
                .iter()
                .map(|event| WishEvent {
                    milliseconds_before_marker: event.milliseconds_before_marker(),
                    event: event.event().clone(),
                })
                .collect(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn capture_scope(&self) -> WishCaptureScope {
        self.capture_scope
    }

    pub fn category(&self) -> WishCategory {
        self.category
    }

    pub fn runtime_identity(&self) -> Option<&WishRuntimeIdentity> {
        self.runtime_identity.as_ref()
    }

    pub fn focus_event_range(&self) -> std::ops::Range<usize> {
        self.focus_event_start
            ..self
                .focus_event_start
                .saturating_add(self.focus_event_count)
    }

    pub fn event_role(&self, index: usize) -> Option<WishEventRole> {
        if index >= self.events.len() {
            return None;
        }
        let focus = self.focus_event_range();
        Some(if index < focus.start {
            WishEventRole::Context
        } else if index < focus.end {
            WishEventRole::Focus
        } else {
            WishEventRole::Trigger
        })
    }

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

    pub fn events(&self) -> &[WishEvent] {
        &self.events
    }

    /// Reduces automatic-transposition candidate frames and their terminal
    /// selection into bounded calibration observations.
    ///
    /// Only a visible recovery candidate followed by a commit for the same
    /// code can become accepted or rejected. Shadow hits, cancellation,
    /// continued typing and incomplete tails remain unknown.
    pub fn automatic_transposition_observations(
        &self,
    ) -> Result<Vec<TranspositionCalibrationObservation>, WishFeedbackError> {
        struct PendingDecision {
            code: String,
            decision: NativeAutomaticTranspositionDecision,
        }

        fn finish(
            output: &mut Vec<TranspositionCalibrationObservation>,
            pending: PendingDecision,
            label: TranspositionCalibrationLabel,
        ) -> Result<(), WishFeedbackError> {
            output.push(
                TranspositionCalibrationObservation::from_code(
                    &pending.code,
                    pending.decision.syllable_index(),
                    pending.decision.pair_gap_ms(),
                    pending.decision.cold_tier(),
                    label,
                )
                .map_err(|_| WishFeedbackError::InvalidSnapshot)?,
            );
            Ok(())
        }

        let mut observations = Vec::new();
        let mut pending: Option<PendingDecision> = None;
        for event in &self.events {
            match event.event() {
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code,
                    automatic_transposition: Some(decision),
                    ..
                } => {
                    if decision.syllable_count() != 1 {
                        if let Some(previous) = pending.take() {
                            finish(
                                &mut observations,
                                previous,
                                TranspositionCalibrationLabel::Unknown,
                            )?;
                        }
                        continue;
                    }
                    if pending.as_ref().is_some_and(|pending| {
                        pending.code == *code && pending.decision == *decision
                    }) {
                        continue;
                    }
                    if let Some(previous) = pending.take() {
                        finish(
                            &mut observations,
                            previous,
                            TranspositionCalibrationLabel::Unknown,
                        )?;
                    }
                    pending = Some(PendingDecision {
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
                    if pending
                        .as_ref()
                        .is_some_and(|pending| pending.code != *code)
                        && let Some(previous) = pending.take()
                    {
                        finish(
                            &mut observations,
                            previous,
                            TranspositionCalibrationLabel::Unknown,
                        )?;
                    }
                }
                NativeFeedbackEvent::CandidateCommitted { code, text, .. } => {
                    if let Some(previous) = pending.take() {
                        let label = if previous.code == *code
                            && previous.decision.outcome()
                                == NativeAutomaticTranspositionOutcome::RecoveryAvailable
                            && previous.decision.visible_rank().is_some()
                        {
                            if previous.decision.recovered_text() == Some(text.as_str()) {
                                TranspositionCalibrationLabel::Accepted
                            } else {
                                TranspositionCalibrationLabel::Rejected
                            }
                        } else {
                            TranspositionCalibrationLabel::Unknown
                        };
                        finish(&mut observations, previous, label)?;
                    }
                }
                NativeFeedbackEvent::RawCodeCommitted { .. }
                | NativeFeedbackEvent::CompositionCancelled { .. } => {
                    if let Some(previous) = pending.take() {
                        finish(
                            &mut observations,
                            previous,
                            TranspositionCalibrationLabel::Unknown,
                        )?;
                    }
                }
                NativeFeedbackEvent::CandidatePopupTiming { .. } => {}
            }
        }
        if let Some(previous) = pending {
            finish(
                &mut observations,
                previous,
                TranspositionCalibrationLabel::Unknown,
            )?;
        }
        Ok(observations)
    }

    fn validate(&self) -> Result<(), WishFeedbackError> {
        if let Some(identity) = &self.runtime_identity {
            identity.validate()?;
        }
        if self.lookback_ms == 0 || self.events.len() > MAX_WISH_EVENTS {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let focus_end = self
            .focus_event_start
            .checked_add(self.focus_event_count)
            .ok_or(WishFeedbackError::InvalidSnapshot)?;
        if self.capture_scope == WishCaptureScope::LegacyWindow {
            if self.focus_event_start != 0 || self.focus_event_count != self.events.len() {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
        } else if self.events.is_empty()
            || self.focus_event_count == 0
            || focus_end > self.events.len()
        {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let accounted = self
            .events
            .len()
            .checked_add(self.omitted_before_window)
            .and_then(|count| count.checked_add(self.omitted_untimed))
            .and_then(|count| count.checked_add(self.omitted_by_event_limit))
            .ok_or(WishFeedbackError::InvalidSnapshot)?;
        if accounted != self.source_events {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let mut previous_age = u32::MAX;
        for event in &self.events {
            if event.milliseconds_before_marker > self.lookback_ms
                || event.milliseconds_before_marker > previous_age
                || event.event.validate_and_measure().is_none()
            {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            previous_age = event.milliseconds_before_marker;
        }
        Ok(())
    }

    fn render(&self) -> Result<Vec<u8>, WishFeedbackError> {
        self.validate()?;
        let mut output = Vec::new();
        output.extend_from_slice(WISH_PLAINTEXT_MAGIC_V7);
        output.push(self.capture_scope.encoded());
        output.push(self.category.encoded());
        put_usize(&mut output, self.focus_event_start)?;
        put_usize(&mut output, self.focus_event_count)?;
        output.push(u8::from(self.runtime_identity.is_some()));
        if let Some(identity) = &self.runtime_identity {
            put_string(&mut output, identity.module_sha256())?;
            put_string(&mut output, identity.core_candidate_revision())?;
            output.push(u8::from(
                identity.supplemental_candidate_revision().is_some(),
            ));
            if let Some(revision) = identity.supplemental_candidate_revision() {
                put_string(&mut output, revision)?;
            }
        }
        put_u32(&mut output, self.lookback_ms);
        output.push(u8::from(self.source_complete));
        put_usize(&mut output, self.source_events)?;
        put_usize(&mut output, self.omitted_before_window)?;
        put_usize(&mut output, self.omitted_untimed)?;
        put_usize(&mut output, self.omitted_by_event_limit)?;
        put_usize(&mut output, self.events.len())?;
        for event in &self.events {
            put_u32(&mut output, event.milliseconds_before_marker);
            render_event(&mut output, &event.event)?;
        }
        if output.len() > MAX_WISH_PLAINTEXT_BYTES {
            return Err(WishFeedbackError::PlaintextTooLarge);
        }
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, WishFeedbackError> {
        if input.len() <= WISH_PLAINTEXT_MAGIC_V1.len() || input.len() > MAX_WISH_PLAINTEXT_BYTES {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let mut reader = SliceReader::new(input);
        let version = if input.starts_with(WISH_PLAINTEXT_MAGIC_V7) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V7)?;
            7
        } else if input.starts_with(WISH_PLAINTEXT_MAGIC_V6) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V6)?;
            6
        } else if input.starts_with(WISH_PLAINTEXT_MAGIC_V5) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V5)?;
            5
        } else if input.starts_with(WISH_PLAINTEXT_MAGIC_V4) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V4)?;
            4
        } else if input.starts_with(WISH_PLAINTEXT_MAGIC_V3) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V3)?;
            3
        } else if input.starts_with(WISH_PLAINTEXT_MAGIC_V2) {
            reader.expect(WISH_PLAINTEXT_MAGIC_V2)?;
            2
        } else {
            reader.expect(WISH_PLAINTEXT_MAGIC_V1)?;
            1
        };
        let (capture_scope, category, focus_event_start, focus_event_count) = if version >= 2 {
            (
                WishCaptureScope::decode(reader.byte()?)
                    .ok_or(WishFeedbackError::InvalidSnapshot)?,
                WishCategory::decode(reader.byte()?).ok_or(WishFeedbackError::InvalidSnapshot)?,
                reader.usize()?,
                reader.usize()?,
            )
        } else {
            (WishCaptureScope::LegacyWindow, WishCategory::Other, 0, 0)
        };
        let runtime_identity = if version >= 7 && reader.boolean()? {
            Some(WishRuntimeIdentity::new(
                reader.string()?,
                reader.string()?,
                if reader.boolean()? {
                    Some(reader.string()?)
                } else {
                    None
                },
            )?)
        } else {
            None
        };
        let lookback_ms = reader.u32()?;
        let source_complete = reader.boolean()?;
        let source_events = reader.usize()?;
        let omitted_before_window = reader.usize()?;
        let omitted_untimed = reader.usize()?;
        let omitted_by_event_limit = reader.usize()?;
        let event_count = reader.usize()?;
        if event_count > MAX_WISH_EVENTS {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            events.push(WishEvent {
                milliseconds_before_marker: reader.u32()?,
                event: parse_event(&mut reader, version)?,
            });
        }
        if !reader.is_empty() {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let snapshot = Self {
            capture_scope,
            category,
            runtime_identity,
            focus_event_start,
            focus_event_count: if version == 1 {
                events.len()
            } else {
                focus_event_count
            },
            lookback_ms,
            source_complete,
            source_events,
            omitted_before_window,
            omitted_untimed,
            omitted_by_event_limit,
            events,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

fn latest_completed_episode_range(events: &[FrozenNativeFeedbackEvent]) -> (usize, usize) {
    let terminals = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(
                event.event(),
                NativeFeedbackEvent::CandidateCommitted { .. }
                    | NativeFeedbackEvent::RawCodeCommitted { .. }
                    | NativeFeedbackEvent::CompositionCancelled { .. }
            )
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let Some(end) = terminals.last().map(|index| index.saturating_add(1)) else {
        return (0, events.len());
    };
    let start = if terminals.len() >= 2 {
        terminals[terminals.len() - 2].saturating_add(1)
    } else {
        0
    };
    (start, end.saturating_sub(start))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishCategory {
    Candidates,
    Ranking,
    Display,
    Latency,
    InputMode,
    Compatibility,
    Other,
}

impl WishCategory {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Candidates => "candidates",
            Self::Ranking => "ranking",
            Self::Display => "display",
            Self::Latency => "latency",
            Self::InputMode => "input-mode",
            Self::Compatibility => "compatibility",
            Self::Other => "other",
        }
    }

    pub fn parse_slug(value: &str) -> Option<Self> {
        match value {
            "candidates" => Some(Self::Candidates),
            "ranking" => Some(Self::Ranking),
            "display" => Some(Self::Display),
            "latency" => Some(Self::Latency),
            "input-mode" => Some(Self::InputMode),
            "compatibility" => Some(Self::Compatibility),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    fn encoded(self) -> u8 {
        match self {
            Self::Candidates => 1,
            Self::Ranking => 2,
            Self::Display => 3,
            Self::Latency => 4,
            Self::InputMode => 5,
            Self::Compatibility => 6,
            Self::Other => 7,
        }
    }

    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Candidates),
            2 => Some(Self::Ranking),
            3 => Some(Self::Display),
            4 => Some(Self::Latency),
            5 => Some(Self::InputMode),
            6 => Some(Self::Compatibility),
            7 => Some(Self::Other),
            _ => None,
        }
    }
}

/// One explicitly supplied private note bound to an immutable wish ID.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishNote {
    wish_id: String,
    category: WishCategory,
    text: String,
}

impl WishNote {
    pub fn new(
        wish_id: &str,
        category: WishCategory,
        text: &str,
    ) -> Result<Self, WishFeedbackError> {
        validate_wish_id(wish_id)?;
        if text.trim().is_empty() || text.len() > MAX_WISH_NOTE_BYTES || text.contains('\0') {
            return Err(WishFeedbackError::InvalidNote);
        }
        Ok(Self {
            wish_id: wish_id.to_owned(),
            category,
            text: text.to_owned(),
        })
    }

    pub fn wish_id(&self) -> &str {
        &self.wish_id
    }

    pub fn category(&self) -> WishCategory {
        self.category
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn render(&self) -> Result<Vec<u8>, WishFeedbackError> {
        let checked = Self::new(&self.wish_id, self.category, &self.text)?;
        let mut output = Vec::new();
        output.extend_from_slice(WISH_NOTE_PLAINTEXT_MAGIC);
        put_string(&mut output, &checked.wish_id)?;
        put_string(&mut output, checked.category.slug())?;
        put_string(&mut output, &checked.text)?;
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, WishFeedbackError> {
        if input.len() <= WISH_NOTE_PLAINTEXT_MAGIC.len()
            || input.len() > MAX_WISH_NOTE_BYTES.saturating_add(512)
        {
            return Err(WishFeedbackError::InvalidNote);
        }
        let mut reader = SliceReader::new(input);
        reader.expect(WISH_NOTE_PLAINTEXT_MAGIC)?;
        let wish_id = reader.string()?;
        let category =
            WishCategory::parse_slug(&reader.string()?).ok_or(WishFeedbackError::InvalidNote)?;
        let text = reader.string()?;
        if !reader.is_empty() {
            return Err(WishFeedbackError::InvalidNote);
        }
        Self::new(&wish_id, category, &text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishPackageInfo {
    id: String,
    protected_bytes: u64,
    modified: SystemTime,
}

impl WishPackageInfo {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn protected_bytes(&self) -> u64 {
        self.protected_bytes
    }

    pub fn modified(&self) -> SystemTime {
        self.modified
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishSaveReceipt {
    id: String,
    events: usize,
    protected_bytes: usize,
}

impl WishSaveReceipt {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn events(&self) -> usize {
        self.events
    }

    pub fn protected_bytes(&self) -> usize {
        self.protected_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishFeedbackError {
    InvalidSnapshot,
    PlaintextTooLarge,
    InvalidPlaintext,
    Protection,
    InvalidProtectedPackage,
    InvalidRoot,
    RootUnavailable,
    InvalidWishId,
    WishUnavailable,
    WishAlreadyExists,
    InvalidNote,
    NoteUnavailable,
    NoteAlreadyExists,
    InvalidTrash,
    Io,
}

impl fmt::Display for WishFeedbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSnapshot => "wish snapshot is invalid",
            Self::PlaintextTooLarge => "wish plaintext exceeds its limit",
            Self::InvalidPlaintext => "wish plaintext is malformed",
            Self::Protection => "current-user wish protection failed",
            Self::InvalidProtectedPackage => "protected wish package is invalid",
            Self::InvalidRoot => "wish root is not a regular directory",
            Self::RootUnavailable => "wish root is unavailable",
            Self::InvalidWishId => "wish id is invalid",
            Self::WishUnavailable => "wish package is unavailable",
            Self::WishAlreadyExists => "wish package already exists",
            Self::InvalidNote => "wish note is invalid",
            Self::NoteUnavailable => "wish note is unavailable",
            Self::NoteAlreadyExists => "wish note already exists",
            Self::InvalidTrash => "wish trash is not a regular directory",
            Self::Io => "wish storage operation failed",
        })
    }
}

impl Error for WishFeedbackError {}

pub fn save_wish_snapshot(
    root: &Path,
    snapshot: &WishSnapshot,
    protector: &dyn DataProtector,
) -> Result<WishSaveReceipt, WishFeedbackError> {
    let mut plaintext = snapshot.render()?;
    let protected = protect_payload(&plaintext, WISH_PROTECTED_MAGIC, protector);
    plaintext.fill(0);
    let protected = protected?;
    let id = format!("{WISH_ID_PREFIX}{}", candidate_sha256_hex(&protected));
    prepare_root(root)?;
    let destination = root.join(wish_filename(&id)?);
    publish_new(
        root,
        &destination,
        &protected,
        WishFeedbackError::WishAlreadyExists,
    )?;
    Ok(WishSaveReceipt {
        id,
        events: snapshot.events.len(),
        protected_bytes: protected.len(),
    })
}

pub fn load_wish_snapshot(
    root: &Path,
    wish_id: &str,
    protector: &dyn DataProtector,
) -> Result<WishSnapshot, WishFeedbackError> {
    ensure_root(root)?;
    let path = root.join(wish_filename(wish_id)?);
    let protected = read_regular_bytes(
        &path,
        MAX_WISH_PACKAGE_BYTES,
        WishFeedbackError::WishUnavailable,
    )?;
    if format!("{WISH_ID_PREFIX}{}", candidate_sha256_hex(&protected)) != wish_id {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let mut plaintext = unprotect_payload(&protected, WISH_PROTECTED_MAGIC, protector)?;
    let snapshot = WishSnapshot::parse(&plaintext);
    plaintext.fill(0);
    snapshot
}

pub fn list_wish_packages(root: &Path) -> Result<Vec<WishPackageInfo>, WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(WishFeedbackError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(WishFeedbackError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let mut packages = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| WishFeedbackError::RootUnavailable)? {
        let entry = entry.map_err(|_| WishFeedbackError::RootUnavailable)?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| WishFeedbackError::RootUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = name.strip_suffix(WISH_PACKAGE_FILE_SUFFIX) else {
            continue;
        };
        if validate_wish_id(id).is_err()
            || metadata.len() == 0
            || metadata.len() > MAX_WISH_PACKAGE_BYTES as u64
        {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        packages.push((
            modified,
            WishPackageInfo {
                id: id.to_owned(),
                protected_bytes: metadata.len(),
                modified,
            },
        ));
    }
    packages.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.id.cmp(&left.1.id))
    });
    Ok(packages.into_iter().map(|(_, info)| info).collect())
}

pub fn save_wish_note(
    root: &Path,
    note: &WishNote,
    protector: &dyn DataProtector,
) -> Result<(), WishFeedbackError> {
    // Refuse detached notes: the exact encrypted wish must already exist.
    ensure_regular_file(
        &root.join(wish_filename(note.wish_id())?),
        WishFeedbackError::WishUnavailable,
    )?;
    let mut plaintext = note.render()?;
    let protected = protect_payload(&plaintext, WISH_NOTE_PROTECTED_MAGIC, protector);
    plaintext.fill(0);
    let protected = protected?;
    if protected.len() > MAX_WISH_NOTE_BYTES.saturating_add(1024) {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let destination = root.join(note_filename(note.wish_id())?);
    publish_new(
        root,
        &destination,
        &protected,
        WishFeedbackError::NoteAlreadyExists,
    )
}

/// Creates or replaces the editable note bound to one immutable wish.
///
/// Only encrypted bytes are written to the temporary file. On Windows the
/// final publication uses a write-through replace so readers see either the
/// old complete note or the new complete note.
pub fn save_or_replace_wish_note(
    root: &Path,
    note: &WishNote,
    protector: &dyn DataProtector,
) -> Result<(), WishFeedbackError> {
    ensure_regular_file(
        &root.join(wish_filename(note.wish_id())?),
        WishFeedbackError::WishUnavailable,
    )?;
    let mut plaintext = note.render()?;
    let protected = protect_payload(&plaintext, WISH_NOTE_PROTECTED_MAGIC, protector);
    plaintext.fill(0);
    let protected = protected?;
    if protected.len() > MAX_WISH_NOTE_BYTES.saturating_add(1024) {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    publish_replace(root, &root.join(note_filename(note.wish_id())?), &protected)
}

pub fn load_wish_note(
    root: &Path,
    wish_id: &str,
    protector: &dyn DataProtector,
) -> Result<WishNote, WishFeedbackError> {
    ensure_root(root)?;
    let path = root.join(note_filename(wish_id)?);
    let protected = read_regular_bytes(
        &path,
        MAX_WISH_NOTE_BYTES.saturating_add(1024),
        WishFeedbackError::NoteUnavailable,
    )?;
    let mut plaintext = unprotect_payload(&protected, WISH_NOTE_PROTECTED_MAGIC, protector)?;
    let note = WishNote::parse(&plaintext);
    plaintext.fill(0);
    let note = note?;
    if note.wish_id() != wish_id {
        return Err(WishFeedbackError::InvalidNote);
    }
    Ok(note)
}

/// Moves one exact wish and its optional note to a recoverable local trash.
pub fn move_wish_to_trash(root: &Path, wish_id: &str) -> Result<(), WishFeedbackError> {
    ensure_root(root)?;
    let trash = root.join(WISH_TRASH_DIRECTORY);
    ensure_or_create_directory(&trash, WishFeedbackError::InvalidTrash)?;
    let wish_name = wish_filename(wish_id)?;
    let source = root.join(&wish_name);
    ensure_regular_file(&source, WishFeedbackError::WishUnavailable)?;
    let destination = trash.join(&wish_name);
    if destination.exists() {
        return Err(WishFeedbackError::WishAlreadyExists);
    }
    let note_name = note_filename(wish_id)?;
    let note_source = root.join(&note_name);
    let note_destination = trash.join(&note_name);
    let has_note = match fs::symlink_metadata(&note_source) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => true,
        Ok(_) => return Err(WishFeedbackError::NoteUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(WishFeedbackError::NoteUnavailable),
    };
    if has_note && note_destination.exists() {
        return Err(WishFeedbackError::NoteAlreadyExists);
    }

    fs::rename(&source, &destination).map_err(|_| WishFeedbackError::Io)?;
    if has_note && fs::rename(&note_source, &note_destination).is_err() {
        // Best-effort rollback keeps the active wish and its note together
        // when the second recoverable move unexpectedly fails.
        let _ = fs::rename(&destination, &source);
        return Err(WishFeedbackError::Io);
    }
    Ok(())
}

fn protect_payload(
    plaintext: &[u8],
    magic: &[u8],
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, WishFeedbackError> {
    if plaintext.is_empty() || plaintext.len() > MAX_WISH_PLAINTEXT_BYTES {
        return Err(WishFeedbackError::PlaintextTooLarge);
    }
    let protected = protector
        .protect(plaintext)
        .map_err(|_| WishFeedbackError::Protection)?;
    if protected.is_empty() || protected.len() > MAX_WISH_PACKAGE_BYTES {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let protected_len =
        u32::try_from(protected.len()).map_err(|_| WishFeedbackError::InvalidProtectedPackage)?;
    let mut output = Vec::with_capacity(magic.len() + 4 + protected.len());
    output.extend_from_slice(magic);
    put_u32(&mut output, protected_len);
    output.extend_from_slice(&protected);
    if output.len() > MAX_WISH_PACKAGE_BYTES {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    Ok(output)
}

fn unprotect_payload(
    package: &[u8],
    magic: &[u8],
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, WishFeedbackError> {
    if package.len() <= magic.len() + 4
        || package.len() > MAX_WISH_PACKAGE_BYTES
        || !package.starts_with(magic)
    {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let length = u32::from_le_bytes(
        package[magic.len()..magic.len() + 4]
            .try_into()
            .map_err(|_| WishFeedbackError::InvalidProtectedPackage)?,
    ) as usize;
    let protected = &package[magic.len() + 4..];
    if protected.is_empty() || protected.len() != length {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    protector
        .unprotect(protected)
        .map_err(|_| WishFeedbackError::Protection)
}

fn render_event(
    output: &mut Vec<u8>,
    event: &NativeFeedbackEvent,
) -> Result<(), WishFeedbackError> {
    if event.validate_and_measure().is_none() {
        return Err(WishFeedbackError::InvalidSnapshot);
    }
    match event {
        NativeFeedbackEvent::CandidatesPresented {
            code,
            view,
            page_start,
            candidates,
            may_have_more,
        } => {
            output.push(1);
            put_string(output, code)?;
            output.push(view_tag(*view));
            put_usize(output, *page_start)?;
            put_usize(output, candidates.len())?;
            for candidate in candidates {
                put_string(output, candidate)?;
            }
            output.push(u8::from(*may_have_more));
        }
        NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code,
            view,
            page_start,
            candidates,
            provenance,
            automatic_transposition,
            loaded_candidates,
            tab_assembly,
            may_have_more,
        } => {
            output.push(9);
            put_string(output, code)?;
            output.push(view_tag(*view));
            put_usize(output, *page_start)?;
            put_usize(output, candidates.len())?;
            for candidate in candidates {
                put_string(output, candidate)?;
            }
            put_usize(output, provenance.len())?;
            for provenance in provenance {
                output.push(candidate_source_tag(provenance.source()));
                output.push(u8::from(provenance.session_promoted()));
            }
            output.push(u8::from(automatic_transposition.is_some()));
            if let Some(decision) = automatic_transposition {
                put_usize(output, decision.syllable_index())?;
                put_usize(output, decision.syllable_count())?;
                put_u32(output, decision.pair_gap_ms());
                output.push(automatic_transposition_tier_tag(decision.cold_tier()));
                output.push(automatic_transposition_tier_tag(decision.tier()));
                output.push(automatic_transposition_outcome_tag(decision.outcome()));
                output.push(u8::from(decision.recovered_text().is_some()));
                if let Some(text) = decision.recovered_text() {
                    put_string(output, text)?;
                }
                put_usize(output, decision.visible_rank().unwrap_or(0))?;
            }
            put_usize(output, *loaded_candidates)?;
            output.push(u8::from(tab_assembly.is_some()));
            if let Some(tab_assembly) = tab_assembly {
                put_usize(output, tab_assembly.position())?;
                put_usize(output, tab_assembly.total_characters())?;
                put_string(output, tab_assembly.stroke_prefix())?;
            }
            output.push(u8::from(*may_have_more));
        }
        NativeFeedbackEvent::CandidateCommitted {
            code,
            text,
            view,
            source,
            absolute_rank,
            visible_rank,
        } => {
            output.push(2);
            put_string(output, code)?;
            put_string(output, text)?;
            output.push(view_tag(*view));
            output.push(selection_tag(*source));
            put_usize(output, *absolute_rank)?;
            put_usize(output, *visible_rank)?;
        }
        NativeFeedbackEvent::RawCodeCommitted { code } => {
            output.push(3);
            put_string(output, code)?;
        }
        NativeFeedbackEvent::CompositionCancelled { code, source } => {
            output.push(4);
            put_string(output, code)?;
            output.push(cancellation_tag(*source));
        }
        NativeFeedbackEvent::CandidatePopupTiming {
            first_frame_ms,
            fully_visible_ms,
            initial_show,
        } => {
            output.push(5);
            put_u32(output, *first_frame_ms);
            put_u32(output, *fully_visible_ms);
            output.push(u8::from(*initial_show));
        }
    }
    Ok(())
}

fn parse_event(
    reader: &mut SliceReader<'_>,
    version: u8,
) -> Result<NativeFeedbackEvent, WishFeedbackError> {
    let event = match reader.byte()? {
        1 => {
            let code = reader.string()?;
            let view = parse_view(reader.byte()?)?;
            let page_start = reader.usize()?;
            let count = reader.usize()?;
            if count > 7 {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            let mut candidates = Vec::with_capacity(count);
            for _ in 0..count {
                candidates.push(reader.string()?);
            }
            NativeFeedbackEvent::CandidatesPresented {
                code,
                view,
                page_start,
                candidates,
                may_have_more: reader.boolean()?,
            }
        }
        2 => NativeFeedbackEvent::CandidateCommitted {
            code: reader.string()?,
            text: reader.string()?,
            view: parse_view(reader.byte()?)?,
            source: parse_selection(reader.byte()?)?,
            absolute_rank: reader.usize()?,
            visible_rank: reader.usize()?,
        },
        3 => NativeFeedbackEvent::RawCodeCommitted {
            code: reader.string()?,
        },
        4 => NativeFeedbackEvent::CompositionCancelled {
            code: reader.string()?,
            source: parse_cancellation(reader.byte()?)?,
        },
        5 => NativeFeedbackEvent::CandidatePopupTiming {
            first_frame_ms: reader.u32()?,
            fully_visible_ms: reader.u32()?,
            initial_show: reader.boolean()?,
        },
        tag if (tag == 6 && version >= 3)
            || (tag == 7 && version >= 4)
            || (tag == 8 && version >= 5)
            || (tag == 9 && version >= 6) =>
        {
            let code = reader.string()?;
            let view = parse_view(reader.byte()?)?;
            let page_start = reader.usize()?;
            let candidate_count = reader.usize()?;
            if candidate_count > 7 {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            let mut candidates = Vec::with_capacity(candidate_count);
            for _ in 0..candidate_count {
                candidates.push(reader.string()?);
            }
            let provenance_count = reader.usize()?;
            if provenance_count != candidate_count {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            let mut provenance = Vec::with_capacity(provenance_count);
            for _ in 0..provenance_count {
                provenance.push(NativeCandidateProvenance::new(
                    parse_candidate_source(reader.byte()?)?,
                    reader.boolean()?,
                ));
            }
            let automatic_transposition = if tag == 9 {
                if reader.boolean()? {
                    let syllable_index = reader.usize()?;
                    let syllable_count = reader.usize()?;
                    let pair_gap_ms = reader.u32()?;
                    let cold_tier = parse_automatic_transposition_tier(reader.byte()?)?;
                    let tier = parse_automatic_transposition_tier(reader.byte()?)?;
                    let outcome = parse_automatic_transposition_outcome(reader.byte()?)?;
                    let recovered_text = reader.boolean()?.then(|| reader.string()).transpose()?;
                    let visible_rank = match reader.usize()? {
                        0 => None,
                        rank => Some(rank),
                    };
                    Some(NativeAutomaticTranspositionDecision::new_span(
                        syllable_index..syllable_index.saturating_add(syllable_count),
                        pair_gap_ms,
                        cold_tier,
                        tier,
                        outcome,
                        recovered_text,
                        visible_rank,
                    ))
                } else {
                    None
                }
            } else if matches!(tag, 7 | 8) {
                let syllable_index = reader.usize()?;
                let syllable_count = if tag == 8 { reader.usize()? } else { 1 };
                let pair_gap_ms = reader.u32()?;
                let cold_tier = parse_automatic_transposition_tier(reader.byte()?)?;
                let tier = parse_automatic_transposition_tier(reader.byte()?)?;
                let outcome = parse_automatic_transposition_outcome(reader.byte()?)?;
                let recovered_text = reader.boolean()?.then(|| reader.string()).transpose()?;
                let visible_rank = match reader.usize()? {
                    0 => None,
                    rank => Some(rank),
                };
                Some(if syllable_count == 1 {
                    NativeAutomaticTranspositionDecision::new(
                        syllable_index,
                        pair_gap_ms,
                        cold_tier,
                        tier,
                        outcome,
                        recovered_text,
                        visible_rank,
                    )
                } else {
                    NativeAutomaticTranspositionDecision::new_span(
                        syllable_index..syllable_index.saturating_add(syllable_count),
                        pair_gap_ms,
                        cold_tier,
                        tier,
                        outcome,
                        recovered_text,
                        visible_rank,
                    )
                })
            } else {
                None
            };
            let (loaded_candidates, tab_assembly) = if tag == 9 {
                let loaded_candidates = reader.usize()?;
                let tab_assembly = if reader.boolean()? {
                    Some(NativeTabAssemblyState::new(
                        reader.usize()?,
                        reader.usize()?,
                        &reader.string()?,
                    ))
                } else {
                    None
                };
                (loaded_candidates, tab_assembly)
            } else {
                (page_start.saturating_add(candidate_count), None)
            };
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                view,
                page_start,
                candidates,
                provenance,
                automatic_transposition,
                loaded_candidates,
                tab_assembly,
                may_have_more: reader.boolean()?,
            }
        }
        _ => return Err(WishFeedbackError::InvalidSnapshot),
    };
    if event.validate_and_measure().is_none() {
        return Err(WishFeedbackError::InvalidSnapshot);
    }
    Ok(event)
}

fn candidate_source_tag(value: NativeCandidateSource) -> u8 {
    match value {
        NativeCandidateSource::Unknown => 1,
        NativeCandidateSource::ExplicitAlias => 2,
        NativeCandidateSource::ProjectOverlay => 3,
        NativeCandidateSource::CoreExact => 4,
        NativeCandidateSource::SupplementalExact => 5,
        NativeCandidateSource::CharacterPair => 6,
        NativeCandidateSource::Decoder => 7,
        NativeCandidateSource::TranspositionRecovery => 8,
        NativeCandidateSource::Shape => 9,
    }
}

fn parse_candidate_source(value: u8) -> Result<NativeCandidateSource, WishFeedbackError> {
    match value {
        1 => Ok(NativeCandidateSource::Unknown),
        2 => Ok(NativeCandidateSource::ExplicitAlias),
        3 => Ok(NativeCandidateSource::ProjectOverlay),
        4 => Ok(NativeCandidateSource::CoreExact),
        5 => Ok(NativeCandidateSource::SupplementalExact),
        6 => Ok(NativeCandidateSource::CharacterPair),
        7 => Ok(NativeCandidateSource::Decoder),
        8 => Ok(NativeCandidateSource::TranspositionRecovery),
        9 => Ok(NativeCandidateSource::Shape),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn automatic_transposition_tier_tag(value: NativeAutomaticTranspositionTier) -> u8 {
    match value {
        NativeAutomaticTranspositionTier::Shadow => 1,
        NativeAutomaticTranspositionTier::Secondary => 2,
        NativeAutomaticTranspositionTier::Primary => 3,
    }
}

fn parse_automatic_transposition_tier(
    value: u8,
) -> Result<NativeAutomaticTranspositionTier, WishFeedbackError> {
    match value {
        1 => Ok(NativeAutomaticTranspositionTier::Shadow),
        2 => Ok(NativeAutomaticTranspositionTier::Secondary),
        3 => Ok(NativeAutomaticTranspositionTier::Primary),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn automatic_transposition_outcome_tag(value: NativeAutomaticTranspositionOutcome) -> u8 {
    match value {
        NativeAutomaticTranspositionOutcome::Suppressed => 1,
        NativeAutomaticTranspositionOutcome::NoRecovery => 2,
        NativeAutomaticTranspositionOutcome::RecoveryAvailable => 3,
    }
}

fn parse_automatic_transposition_outcome(
    value: u8,
) -> Result<NativeAutomaticTranspositionOutcome, WishFeedbackError> {
    match value {
        1 => Ok(NativeAutomaticTranspositionOutcome::Suppressed),
        2 => Ok(NativeAutomaticTranspositionOutcome::NoRecovery),
        3 => Ok(NativeAutomaticTranspositionOutcome::RecoveryAvailable),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn view_tag(value: NativeCandidateView) -> u8 {
    match value {
        NativeCandidateView::Ordinary => 1,
        NativeCandidateView::TranspositionRecovery => 2,
        NativeCandidateView::Shape => 3,
    }
}

fn parse_view(value: u8) -> Result<NativeCandidateView, WishFeedbackError> {
    match value {
        1 => Ok(NativeCandidateView::Ordinary),
        2 => Ok(NativeCandidateView::TranspositionRecovery),
        3 => Ok(NativeCandidateView::Shape),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn selection_tag(value: NativeSelectionSource) -> u8 {
    match value {
        NativeSelectionSource::FirstCandidate => 1,
        NativeSelectionSource::Numeric => 2,
        NativeSelectionSource::Punctuation => 3,
    }
}

fn parse_selection(value: u8) -> Result<NativeSelectionSource, WishFeedbackError> {
    match value {
        1 => Ok(NativeSelectionSource::FirstCandidate),
        2 => Ok(NativeSelectionSource::Numeric),
        3 => Ok(NativeSelectionSource::Punctuation),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn cancellation_tag(value: NativeCancellationSource) -> u8 {
    match value {
        NativeCancellationSource::Backspace => 1,
        NativeCancellationSource::Escape => 2,
        NativeCancellationSource::FocusLoss => 3,
        NativeCancellationSource::HostTermination => 4,
    }
}

fn parse_cancellation(value: u8) -> Result<NativeCancellationSource, WishFeedbackError> {
    match value {
        1 => Ok(NativeCancellationSource::Backspace),
        2 => Ok(NativeCancellationSource::Escape),
        3 => Ok(NativeCancellationSource::FocusLoss),
        4 => Ok(NativeCancellationSource::HostTermination),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_usize(output: &mut Vec<u8>, value: usize) -> Result<(), WishFeedbackError> {
    put_u32(
        output,
        u32::try_from(value).map_err(|_| WishFeedbackError::PlaintextTooLarge)?,
    );
    Ok(())
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), WishFeedbackError> {
    if value.len() > MAX_WISH_STRING_BYTES || value.contains('\0') {
        return Err(WishFeedbackError::InvalidPlaintext);
    }
    put_usize(output, value.len())?;
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

struct SliceReader<'a> {
    remaining: &'a [u8],
}

impl<'a> SliceReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    fn expect(&mut self, expected: &[u8]) -> Result<(), WishFeedbackError> {
        if !self.remaining.starts_with(expected) {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        self.remaining = &self.remaining[expected.len()..];
        Ok(())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WishFeedbackError> {
        if self.remaining.len() < count {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let (head, tail) = self.remaining.split_at(count);
        self.remaining = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, WishFeedbackError> {
        Ok(self.take(1)?[0])
    }

    fn boolean(&mut self) -> Result<bool, WishFeedbackError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WishFeedbackError::InvalidPlaintext),
        }
    }

    fn u32(&mut self) -> Result<u32, WishFeedbackError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| WishFeedbackError::InvalidPlaintext)?,
        ))
    }

    fn usize(&mut self) -> Result<usize, WishFeedbackError> {
        usize::try_from(self.u32()?).map_err(|_| WishFeedbackError::InvalidPlaintext)
    }

    fn string(&mut self) -> Result<String, WishFeedbackError> {
        let length = self.usize()?;
        if length > MAX_WISH_STRING_BYTES {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let value = std::str::from_utf8(self.take(length)?)
            .map_err(|_| WishFeedbackError::InvalidPlaintext)?;
        if value.contains('\0') {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        Ok(value.to_owned())
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn validate_wish_id(value: &str) -> Result<(), WishFeedbackError> {
    let Some(digest) = value.strip_prefix(WISH_ID_PREFIX) else {
        return Err(WishFeedbackError::InvalidWishId);
    };
    if digest.len() != WISH_ID_HEX_BYTES
        || !digest
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(WishFeedbackError::InvalidWishId);
    }
    Ok(())
}

fn wish_filename(wish_id: &str) -> Result<String, WishFeedbackError> {
    validate_wish_id(wish_id)?;
    Ok(format!("{wish_id}{WISH_PACKAGE_FILE_SUFFIX}"))
}

fn note_filename(wish_id: &str) -> Result<String, WishFeedbackError> {
    validate_wish_id(wish_id)?;
    Ok(format!("{wish_id}{WISH_NOTE_FILE_SUFFIX}"))
}

fn ensure_root(root: &Path) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(WishFeedbackError::InvalidRoot),
        Err(_) => Err(WishFeedbackError::RootUnavailable),
    }
}

fn prepare_root(root: &Path) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(WishFeedbackError::InvalidRoot),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| WishFeedbackError::Io)?;
            ensure_root(root)
        }
        Err(_) => Err(WishFeedbackError::RootUnavailable),
    }
}

fn ensure_or_create_directory(
    path: &Path,
    invalid: WishFeedbackError,
) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| WishFeedbackError::Io)
        }
        Err(_) => Err(WishFeedbackError::Io),
    }
}

fn ensure_regular_file(path: &Path, missing: WishFeedbackError) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(missing),
        Err(_) => Err(missing),
    }
}

fn read_regular_bytes(
    path: &Path,
    maximum: usize,
    unavailable: WishFeedbackError,
) -> Result<Vec<u8>, WishFeedbackError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum as u64
    {
        return Err(unavailable);
    }
    let file = File::open(path).map_err(|_| unavailable)?;
    let mut bytes = Vec::new();
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(unavailable);
    }
    Ok(bytes)
}

fn publish_new(
    root: &Path,
    destination: &Path,
    contents: &[u8],
    exists_error: WishFeedbackError,
) -> Result<(), WishFeedbackError> {
    if destination.exists() {
        return Err(exists_error);
    }
    let counter = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = root.join(format!(".wish-{}-{counter}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| WishFeedbackError::Io)?;
        file.write_all(contents)
            .map_err(|_| WishFeedbackError::Io)?;
        file.sync_all().map_err(|_| WishFeedbackError::Io)?;
        drop(file);
        if destination.exists() {
            return Err(exists_error);
        }
        fs::rename(&temporary, destination).map_err(|_| WishFeedbackError::Io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn publish_replace(
    root: &Path,
    destination: &Path,
    contents: &[u8],
) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(WishFeedbackError::NoteUnavailable);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(WishFeedbackError::NoteUnavailable),
    }
    let counter = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = root.join(format!(".wish-{}-{counter}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| WishFeedbackError::Io)?;
        file.write_all(contents)
            .map_err(|_| WishFeedbackError::Io)?;
        file.sync_all().map_err(|_| WishFeedbackError::Io)?;
        drop(file);
        move_replace(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn move_replace(source: &Path, destination: &Path) -> Result<(), WishFeedbackError> {
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // SAFETY: both NUL-terminated buffers remain alive for this synchronous
    // call, and the temporary file contains only protected bytes.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| WishFeedbackError::Io)?;
    }
    Ok(())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, WishFeedbackError> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(WishFeedbackError::Io);
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(not(windows))]
fn move_replace(source: &Path, destination: &Path) -> Result<(), WishFeedbackError> {
    fs::rename(source, destination).map_err(|_| WishFeedbackError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NativeFeedbackAuthorization, NativeFeedbackContext, NativeFeedbackFreezeAuthorization,
        NativeFeedbackLimits, NativeFeedbackSession,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone, Copy)]
    struct TestProtector;

    impl DataProtector for TestProtector {
        fn protection_name(&self) -> &'static str {
            "test"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, crate::ContinuousCaptureError> {
            Ok(plaintext.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, crate::ContinuousCaptureError> {
            self.protect(protected)
        }
    }

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ziranma-wish-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn private_snapshot() -> WishSnapshot {
        let mut feedback = NativeFeedbackSession::default();
        feedback.start_memory(
            NativeFeedbackAuthorization::explicit_memory_only(),
            NativeFeedbackLimits::default(),
        );
        feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::CandidatesPresented {
                code: "wua".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["呜哇".to_owned(), "无哇".to_owned()],
                may_have_more: false,
            },
            1_000,
        );
        feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::CandidateCommitted {
                code: "wua".to_owned(),
                text: "呜哇".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
            1_010,
        );
        let frozen = feedback
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                1_020,
                30_000,
                128,
            )
            .unwrap();
        WishSnapshot::from_frozen(&frozen).unwrap()
    }

    #[test]
    fn private_snapshot_round_trips_without_debug_surface() {
        let snapshot = private_snapshot();
        let rendered = snapshot.render().unwrap();
        assert!(rendered.starts_with(WISH_PLAINTEXT_MAGIC_V7));
        let parsed = WishSnapshot::parse(&rendered).unwrap();
        assert_eq!(parsed.events().len(), 2);
        assert_eq!(parsed.capture_scope(), WishCaptureScope::RecentWindow);
        assert_eq!(parsed.category(), WishCategory::Other);
        assert_eq!(parsed.focus_event_range(), 0..2);
        assert!(parsed == snapshot);
        assert!(WishSnapshot::parse(&rendered[..rendered.len() - 1]).is_err());
    }

    #[test]
    fn v7_round_trips_candidate_runtime_depth_and_multi_syllable_transposition() {
        let mut snapshot = private_snapshot();
        snapshot.events[0].event = NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "fuem".to_owned(),
            view: NativeCandidateView::Ordinary,
            page_start: 0,
            candidates: vec!["什么".to_owned(), "发射没".to_owned()],
            provenance: vec![
                NativeCandidateProvenance::new(NativeCandidateSource::TranspositionRecovery, false),
                NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false),
            ],
            automatic_transposition: Some(NativeAutomaticTranspositionDecision::new_span(
                0..2,
                31,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionTier::Primary,
                NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                Some("什么".to_owned()),
                Some(1),
            )),
            loaded_candidates: 6,
            tab_assembly: None,
            may_have_more: false,
        };

        let rendered = snapshot.render().unwrap();
        assert!(rendered.starts_with(WISH_PLAINTEXT_MAGIC_V7));
        let parsed = WishSnapshot::parse(&rendered).unwrap();
        assert!(parsed == snapshot);
    }

    #[test]
    fn v7_round_trips_tab_assembly_position_strokes_and_loaded_depth() {
        let mut snapshot = private_snapshot();
        snapshot.events[0].event = NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: "hp".to_owned(),
            view: NativeCandidateView::Shape,
            page_start: 6,
            candidates: vec!["魂".to_owned()],
            provenance: vec![NativeCandidateProvenance::new(
                NativeCandidateSource::Shape,
                false,
            )],
            automatic_transposition: None,
            loaded_candidates: 12,
            tab_assembly: Some(NativeTabAssemblyState::new(2, 4, "hs")),
            may_have_more: true,
        };

        let parsed = WishSnapshot::parse(&snapshot.render().unwrap()).unwrap();
        assert!(parsed == snapshot);
    }

    #[test]
    fn v7_round_trips_exact_runtime_identity() {
        let mut snapshot = private_snapshot();
        snapshot.runtime_identity = Some(
            WishRuntimeIdentity::new(
                "12ab".repeat(16),
                "rime-core-test-v1".to_owned(),
                Some("wanxiang-supplement-test-v2".to_owned()),
            )
            .unwrap(),
        );

        let parsed = WishSnapshot::parse(&snapshot.render().unwrap()).unwrap();
        let identity = parsed.runtime_identity().unwrap();
        assert_eq!(identity.module_sha256(), "12ab".repeat(16));
        assert_eq!(identity.core_candidate_revision(), "rime-core-test-v1");
        assert_eq!(
            identity.supplemental_candidate_revision(),
            Some("wanxiang-supplement-test-v2")
        );
        assert!(parsed == snapshot);
    }

    #[test]
    fn v6_snapshot_remains_readable_without_runtime_identity() {
        let snapshot = private_snapshot();
        let mut legacy = Vec::new();
        legacy.extend_from_slice(WISH_PLAINTEXT_MAGIC_V6);
        legacy.push(snapshot.capture_scope.encoded());
        legacy.push(snapshot.category.encoded());
        put_usize(&mut legacy, snapshot.focus_event_start).unwrap();
        put_usize(&mut legacy, snapshot.focus_event_count).unwrap();
        put_u32(&mut legacy, snapshot.lookback_ms);
        legacy.push(u8::from(snapshot.source_complete));
        put_usize(&mut legacy, snapshot.source_events).unwrap();
        put_usize(&mut legacy, snapshot.omitted_before_window).unwrap();
        put_usize(&mut legacy, snapshot.omitted_untimed).unwrap();
        put_usize(&mut legacy, snapshot.omitted_by_event_limit).unwrap();
        put_usize(&mut legacy, snapshot.events.len()).unwrap();
        for event in &snapshot.events {
            put_u32(&mut legacy, event.milliseconds_before_marker);
            render_event(&mut legacy, &event.event).unwrap();
        }

        let parsed = WishSnapshot::parse(&legacy).unwrap();
        assert!(parsed.runtime_identity().is_none());
        assert!(parsed == snapshot);
    }

    #[test]
    fn v6_reader_keeps_v4_single_syllable_transposition_compatibility() {
        let mut legacy = Vec::new();
        legacy.extend_from_slice(WISH_PLAINTEXT_MAGIC_V4);
        legacy.push(WishCaptureScope::RecentWindow.encoded());
        legacy.push(WishCategory::Other.encoded());
        put_usize(&mut legacy, 0).unwrap();
        put_usize(&mut legacy, 1).unwrap();
        put_u32(&mut legacy, 30_000);
        legacy.push(1);
        put_usize(&mut legacy, 1).unwrap();
        put_usize(&mut legacy, 0).unwrap();
        put_usize(&mut legacy, 0).unwrap();
        put_usize(&mut legacy, 0).unwrap();
        put_usize(&mut legacy, 1).unwrap();
        put_u32(&mut legacy, 0);
        legacy.push(7);
        put_string(&mut legacy, "am").unwrap();
        legacy.push(view_tag(NativeCandidateView::Ordinary));
        put_usize(&mut legacy, 0).unwrap();
        put_usize(&mut legacy, 1).unwrap();
        put_string(&mut legacy, "马").unwrap();
        put_usize(&mut legacy, 1).unwrap();
        legacy.push(candidate_source_tag(
            NativeCandidateSource::TranspositionRecovery,
        ));
        legacy.push(0);
        put_usize(&mut legacy, 0).unwrap();
        put_u32(&mut legacy, 31);
        legacy.push(automatic_transposition_tier_tag(
            NativeAutomaticTranspositionTier::Primary,
        ));
        legacy.push(automatic_transposition_tier_tag(
            NativeAutomaticTranspositionTier::Primary,
        ));
        legacy.push(automatic_transposition_outcome_tag(
            NativeAutomaticTranspositionOutcome::RecoveryAvailable,
        ));
        legacy.push(1);
        put_string(&mut legacy, "马").unwrap();
        put_usize(&mut legacy, 1).unwrap();
        legacy.push(0);

        let parsed = WishSnapshot::parse(&legacy).unwrap();
        assert!(matches!(
            parsed.events()[0].event(),
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                loaded_candidates: 1,
                tab_assembly: None,
                ..
            }
        ));
    }

    #[test]
    fn calibration_observations_use_only_visible_same_code_commits_as_labels() {
        let mut feedback = NativeFeedbackSession::default();
        feedback.start_memory(
            NativeFeedbackAuthorization::explicit_memory_only(),
            NativeFeedbackLimits::default(),
        );
        let decision_frame = |code: &str, tier, recovered: &str, visible_rank| {
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code: code.to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["普通".to_owned(), recovered.to_owned()],
                provenance: vec![
                    NativeCandidateProvenance::new(NativeCandidateSource::Decoder, false),
                    NativeCandidateProvenance::new(
                        NativeCandidateSource::TranspositionRecovery,
                        false,
                    ),
                ],
                automatic_transposition: Some(NativeAutomaticTranspositionDecision::new(
                    0,
                    55,
                    tier,
                    tier,
                    NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                    Some(recovered.to_owned()),
                    visible_rank,
                )),
                loaded_candidates: 2,
                tab_assembly: None,
                may_have_more: false,
            }
        };
        let events = [
            decision_frame(
                "am",
                NativeAutomaticTranspositionTier::Secondary,
                "马",
                Some(2),
            ),
            NativeFeedbackEvent::CandidateCommitted {
                code: "am".to_owned(),
                text: "马".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::Numeric,
                absolute_rank: 2,
                visible_rank: 2,
            },
            decision_frame(
                "ma",
                NativeAutomaticTranspositionTier::Secondary,
                "俺们",
                Some(2),
            ),
            NativeFeedbackEvent::CandidateCommitted {
                code: "ma".to_owned(),
                text: "普通".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
            decision_frame("am", NativeAutomaticTranspositionTier::Shadow, "马", None),
            NativeFeedbackEvent::CandidateCommitted {
                code: "am".to_owned(),
                text: "普通".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
        ];
        for (index, event) in events.into_iter().enumerate() {
            feedback.record_at(
                NativeFeedbackContext::Eligible,
                event,
                u64::try_from(index).unwrap().saturating_add(1),
            );
        }
        let frozen = feedback
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                10,
                100,
                32,
            )
            .unwrap();
        let snapshot = WishSnapshot::from_frozen(&frozen).unwrap();
        let observations = snapshot.automatic_transposition_observations().unwrap();
        assert_eq!(observations.len(), 3);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.label())
                .collect::<Vec<_>>(),
            [
                TranspositionCalibrationLabel::Accepted,
                TranspositionCalibrationLabel::Rejected,
                TranspositionCalibrationLabel::Unknown,
            ]
        );
    }

    #[test]
    fn structured_snapshot_marks_context_focus_and_wish_trigger_separately() {
        let mut feedback = NativeFeedbackSession::default();
        feedback.start_memory(
            NativeFeedbackAuthorization::explicit_memory_only(),
            NativeFeedbackLimits::default(),
        );
        for (event, timestamp) in [
            (
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "ab".to_owned(),
                },
                10,
            ),
            (
                NativeFeedbackEvent::CandidatesPresented {
                    code: "cd".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["乙".to_owned()],
                    may_have_more: false,
                },
                20,
            ),
            (
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "cd".to_owned(),
                },
                30,
            ),
            (
                NativeFeedbackEvent::CandidatesPresented {
                    code: "xuy".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["许愿".to_owned()],
                    may_have_more: false,
                },
                40,
            ),
        ] {
            feedback.record_at(NativeFeedbackContext::Eligible, event, timestamp);
        }
        let frozen = feedback
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                50,
                1_000,
                128,
            )
            .unwrap();
        let snapshot = WishSnapshot::from_frozen_with_metadata(
            &frozen,
            WishCaptureScope::RecentEpisodes,
            WishCategory::Ranking,
        )
        .unwrap();

        assert_eq!(snapshot.focus_event_range(), 1..3);
        assert_eq!(snapshot.event_role(0), Some(WishEventRole::Context));
        assert_eq!(snapshot.event_role(1), Some(WishEventRole::Focus));
        assert_eq!(snapshot.event_role(2), Some(WishEventRole::Focus));
        assert_eq!(snapshot.event_role(3), Some(WishEventRole::Trigger));
        assert_eq!(snapshot.category(), WishCategory::Ranking);
        assert_eq!(
            WishSnapshot::parse(&snapshot.render().unwrap())
                .unwrap()
                .focus_event_range(),
            1..3
        );
    }

    #[test]
    fn legacy_v1_snapshot_remains_readable_as_one_unclassified_window() {
        let snapshot = private_snapshot();
        let mut legacy = Vec::new();
        legacy.extend_from_slice(WISH_PLAINTEXT_MAGIC_V1);
        put_u32(&mut legacy, snapshot.lookback_ms);
        legacy.push(u8::from(snapshot.source_complete));
        put_usize(&mut legacy, snapshot.source_events).unwrap();
        put_usize(&mut legacy, snapshot.omitted_before_window).unwrap();
        put_usize(&mut legacy, snapshot.omitted_untimed).unwrap();
        put_usize(&mut legacy, snapshot.omitted_by_event_limit).unwrap();
        put_usize(&mut legacy, snapshot.events.len()).unwrap();
        for event in &snapshot.events {
            put_u32(&mut legacy, event.milliseconds_before_marker);
            render_event(&mut legacy, &event.event).unwrap();
        }

        let parsed = WishSnapshot::parse(&legacy).unwrap();
        assert_eq!(parsed.capture_scope(), WishCaptureScope::LegacyWindow);
        assert_eq!(parsed.category(), WishCategory::Other);
        assert_eq!(parsed.focus_event_range(), 0..parsed.events().len());
    }

    #[test]
    fn legacy_v2_snapshot_remains_readable() {
        let snapshot = private_snapshot();
        let mut legacy = Vec::new();
        legacy.extend_from_slice(WISH_PLAINTEXT_MAGIC_V2);
        legacy.push(snapshot.capture_scope.encoded());
        legacy.push(snapshot.category.encoded());
        put_usize(&mut legacy, snapshot.focus_event_start).unwrap();
        put_usize(&mut legacy, snapshot.focus_event_count).unwrap();
        put_u32(&mut legacy, snapshot.lookback_ms);
        legacy.push(u8::from(snapshot.source_complete));
        put_usize(&mut legacy, snapshot.source_events).unwrap();
        put_usize(&mut legacy, snapshot.omitted_before_window).unwrap();
        put_usize(&mut legacy, snapshot.omitted_untimed).unwrap();
        put_usize(&mut legacy, snapshot.omitted_by_event_limit).unwrap();
        put_usize(&mut legacy, snapshot.events.len()).unwrap();
        for event in &snapshot.events {
            put_u32(&mut legacy, event.milliseconds_before_marker);
            render_event(&mut legacy, &event.event).unwrap();
        }

        let parsed = WishSnapshot::parse(&legacy).unwrap();
        assert!(parsed == snapshot);
    }

    fn v3_provenance_fixture(provenance_count: usize, source_tag: Option<u8>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(WISH_PLAINTEXT_MAGIC_V3);
        bytes.push(WishCaptureScope::RecentWindow.encoded());
        bytes.push(WishCategory::Other.encoded());
        put_usize(&mut bytes, 0).unwrap();
        put_usize(&mut bytes, 1).unwrap();
        put_u32(&mut bytes, 1_000);
        bytes.push(1);
        put_usize(&mut bytes, 1).unwrap();
        put_usize(&mut bytes, 0).unwrap();
        put_usize(&mut bytes, 0).unwrap();
        put_usize(&mut bytes, 0).unwrap();
        put_usize(&mut bytes, 1).unwrap();
        put_u32(&mut bytes, 0);
        bytes.push(6);
        put_string(&mut bytes, "ab").unwrap();
        bytes.push(view_tag(NativeCandidateView::Ordinary));
        put_usize(&mut bytes, 0).unwrap();
        put_usize(&mut bytes, 1).unwrap();
        put_string(&mut bytes, "甲").unwrap();
        put_usize(&mut bytes, provenance_count).unwrap();
        if let Some(source_tag) = source_tag {
            bytes.push(source_tag);
            bytes.push(0);
        }
        bytes.push(0);
        bytes
    }

    #[test]
    fn v3_rejects_misaligned_or_unknown_candidate_provenance() {
        assert!(WishSnapshot::parse(&v3_provenance_fixture(0, None)).is_err());
        assert!(WishSnapshot::parse(&v3_provenance_fixture(1, Some(255))).is_err());
    }

    #[test]
    fn valid_v3_candidate_provenance_remains_readable_without_a_decision() {
        let parsed = WishSnapshot::parse(&v3_provenance_fixture(
            1,
            Some(candidate_source_tag(NativeCandidateSource::CoreExact)),
        ))
        .unwrap();
        assert!(matches!(
            parsed.events()[0].event(),
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                automatic_transposition: None,
                ..
            }
        ));
    }

    #[test]
    fn candidate_source_tags_round_trip_exactly() {
        for source in [
            NativeCandidateSource::Unknown,
            NativeCandidateSource::ExplicitAlias,
            NativeCandidateSource::ProjectOverlay,
            NativeCandidateSource::CoreExact,
            NativeCandidateSource::SupplementalExact,
            NativeCandidateSource::CharacterPair,
            NativeCandidateSource::Decoder,
            NativeCandidateSource::TranspositionRecovery,
            NativeCandidateSource::Shape,
        ] {
            assert_eq!(
                parse_candidate_source(candidate_source_tag(source)),
                Ok(source)
            );
        }
    }

    #[test]
    fn provenance_event_tag_is_not_accepted_by_legacy_schemas() {
        let mut fixture = v3_provenance_fixture(
            1,
            Some(candidate_source_tag(NativeCandidateSource::CoreExact)),
        );
        fixture.splice(
            ..WISH_PLAINTEXT_MAGIC_V3.len(),
            WISH_PLAINTEXT_MAGIC_V2.iter().copied(),
        );
        assert!(WishSnapshot::parse(&fixture).is_err());
    }

    #[test]
    fn protected_package_is_immutable_and_bound_to_its_id() {
        let root = TemporaryDirectory::new();
        let snapshot = private_snapshot();
        let receipt = save_wish_snapshot(&root.0, &snapshot, &TestProtector).unwrap();
        assert_eq!(receipt.events(), 2);
        assert_eq!(list_wish_packages(&root.0).unwrap().len(), 1);
        assert!(load_wish_snapshot(&root.0, receipt.id(), &TestProtector).unwrap() == snapshot);

        let path = root.0.join(wish_filename(receipt.id()).unwrap());
        let mut changed = fs::read(&path).unwrap();
        *changed.last_mut().unwrap() ^= 1;
        fs::write(&path, changed).unwrap();
        assert!(matches!(
            load_wish_snapshot(&root.0, receipt.id(), &TestProtector),
            Err(WishFeedbackError::InvalidProtectedPackage)
        ));
    }

    #[test]
    fn private_note_is_bound_and_trash_is_recoverable() {
        let root = TemporaryDirectory::new();
        let receipt = save_wish_snapshot(&root.0, &private_snapshot(), &TestProtector).unwrap();
        let note = WishNote::new(receipt.id(), WishCategory::Ranking, "第一项不太对").unwrap();
        save_wish_note(&root.0, &note, &TestProtector).unwrap();
        assert!(load_wish_note(&root.0, receipt.id(), &TestProtector).unwrap() == note);
        assert!(matches!(
            save_wish_note(&root.0, &note, &TestProtector),
            Err(WishFeedbackError::NoteAlreadyExists)
        ));

        move_wish_to_trash(&root.0, receipt.id()).unwrap();
        assert!(list_wish_packages(&root.0).unwrap().is_empty());
        assert!(
            root.0
                .join(WISH_TRASH_DIRECTORY)
                .join(wish_filename(receipt.id()).unwrap())
                .is_file()
        );
        assert!(
            root.0
                .join(WISH_TRASH_DIRECTORY)
                .join(note_filename(receipt.id()).unwrap())
                .is_file()
        );
    }

    #[test]
    fn editable_note_replaces_only_the_encrypted_sidecar() {
        let root = TemporaryDirectory::new();
        let receipt = save_wish_snapshot(&root.0, &private_snapshot(), &TestProtector).unwrap();
        let first = WishNote::new(receipt.id(), WishCategory::Ranking, "第一项不太对").unwrap();
        let revised =
            WishNote::new(receipt.id(), WishCategory::Display, "候选间距想再看看").unwrap();

        save_or_replace_wish_note(&root.0, &first, &TestProtector).unwrap();
        save_or_replace_wish_note(&root.0, &revised, &TestProtector).unwrap();

        assert!(load_wish_snapshot(&root.0, receipt.id(), &TestProtector).is_ok());
        assert!(load_wish_note(&root.0, receipt.id(), &TestProtector).unwrap() == revised);
        assert!(fs::read_dir(&root.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn discovery_ignores_unrelated_and_symlink_like_entries() {
        let root = TemporaryDirectory::new();
        fs::write(root.0.join("not-a-wish.txt"), b"public").unwrap();
        fs::create_dir(root.0.join("wish-not-a-file.ziw")).unwrap();
        assert!(list_wish_packages(&root.0).unwrap().is_empty());
        assert!(validate_wish_id("../private").is_err());
    }
}
