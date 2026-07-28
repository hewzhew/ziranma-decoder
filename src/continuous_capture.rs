//! Bounded, encrypted-at-rest segments for an explicitly started continuous
//! local capture session.
//!
//! This module deliberately separates three things:
//!
//! - a strict private plaintext payload that exists only in process memory;
//! - an opaque protection provider;
//! - an atomic writer that only receives already-protected bytes.
//!
//! The Windows provider uses current-user DPAPI without the machine-wide flag.
//! No network, startup registration, directory discovery, or target selection
//! lives here.

use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{EventCapsuleError, EventCapsuleRecorder, EventCapsuleV1, TrackerOutput};

pub const CONTINUOUS_SEGMENT_SCHEMA_V1: &str = "ziranma-continuous-segment-v1";
pub const CONTINUOUS_SEGMENT_SCHEMA_V2: &str = "ziranma-continuous-segment-v2";
pub const CAPTURE_INTEGRITY_SCHEMA_V1: &str = "ziranma-codex-uia-integrity-v1";
pub const PROTECTED_SEGMENT_SCHEMA_V1: &[u8] = b"ziranma-dpapi-segment-v1\0";
pub const CONTINUOUS_PRODUCER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+continuous.7");
pub const CODEX_CAPTURE_PROFILE_V1: &str = "codex-uia-v1";
pub const CODEX_CAPTURE_PROFILE_V2: &str = "codex-uia-v2";
const MAX_SESSION_ID_BYTES: usize = 80;
const MAX_VERSION_FIELD_BYTES: usize = 80;
const MAX_PROTECTED_SEGMENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureSessionKind {
    Daily,
    Course,
    Theme,
}

impl CaptureSessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Course => "course",
            Self::Theme => "theme",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ContinuousCaptureError> {
        match value {
            "daily" => Ok(Self::Daily),
            "course" => Ok(Self::Course),
            "theme" => Ok(Self::Theme),
            _ => Err(ContinuousCaptureError::InvalidField("session kind")),
        }
    }
}

/// A deliberately coarse reason for closing an encrypted segment.
///
/// Pause, target loss, and target reconstruction intentionally share one
/// value.  The encrypted integrity block is evidence about recorder
/// continuity, not a high-resolution activity log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentCloseReason {
    Capacity,
    Timer,
    Continuity,
    SessionEnd,
}

impl SegmentCloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::Timer => "timer",
            Self::Continuity => "continuity",
            Self::SessionEnd => "session-end",
        }
    }

    fn parse(value: &str) -> Result<Self, ContinuousCaptureError> {
        match value {
            "capacity" => Ok(Self::Capacity),
            "timer" => Ok(Self::Timer),
            "continuity" => Ok(Self::Continuity),
            "session-end" => Ok(Self::SessionEnd),
            _ => Err(ContinuousCaptureError::InvalidField("segment close reason")),
        }
    }
}

/// Low-resolution counters collected only after the existing Codex target
/// policy has accepted an event.
///
/// These counters describe what this recorder observed. They cannot prove
/// that Windows or a UIA provider did not drop something upstream. All fields
/// are encrypted inside a v2 segment; none belongs in a file name or normal
/// recorder receipt.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaptureIntegrityCountersV1 {
    pub key_actions_observed: u64,
    pub composition_callbacks_observed: u64,
    pub composition_finalized_callbacks_observed: u64,
    pub value_callbacks_observed: u64,
    pub value_read_errors: u64,
    pub composition_read_errors: u64,
    pub selection_read_errors: u64,
    pub value_callbacks_without_output: u64,
    pub tracker_outputs_emitted: u64,
    pub key_actions_not_emitted_at_boundary: u64,
    pub key_buffer_resets: u64,
    pub counter_saturated: bool,
}

impl CaptureIntegrityCountersV1 {
    pub fn accumulate(&mut self, other: &Self) {
        self.merge(other.clone());
    }

    pub fn observe_key_action(&mut self, buffer_reset: bool) {
        increment_counter(&mut self.key_actions_observed, &mut self.counter_saturated);
        if buffer_reset {
            increment_counter(&mut self.key_buffer_resets, &mut self.counter_saturated);
        }
    }

    pub fn observe_composition_callback(&mut self) {
        increment_counter(
            &mut self.composition_callbacks_observed,
            &mut self.counter_saturated,
        );
    }

    pub fn observe_composition_read_error(&mut self) {
        self.observe_composition_callback();
        increment_counter(
            &mut self.composition_read_errors,
            &mut self.counter_saturated,
        );
    }

    pub fn observe_composition_finalized_callback(&mut self) {
        increment_counter(
            &mut self.composition_finalized_callbacks_observed,
            &mut self.counter_saturated,
        );
    }

    pub fn observe_value_callback(&mut self, produced_output: bool) {
        increment_counter(
            &mut self.value_callbacks_observed,
            &mut self.counter_saturated,
        );
        if !produced_output {
            increment_counter(
                &mut self.value_callbacks_without_output,
                &mut self.counter_saturated,
            );
        }
    }

    pub fn observe_value_read_error(&mut self) {
        increment_counter(
            &mut self.value_callbacks_observed,
            &mut self.counter_saturated,
        );
        increment_counter(&mut self.value_read_errors, &mut self.counter_saturated);
    }

    pub fn observe_selection_read_error(&mut self) {
        increment_counter(&mut self.selection_read_errors, &mut self.counter_saturated);
    }

    pub fn observe_boundary_discard(&mut self, actions: usize) {
        let actions = u64::try_from(actions).unwrap_or(u64::MAX);
        add_counter(
            &mut self.key_actions_not_emitted_at_boundary,
            actions,
            &mut self.counter_saturated,
        );
    }

    fn observe_tracker_output(&mut self) {
        increment_counter(
            &mut self.tracker_outputs_emitted,
            &mut self.counter_saturated,
        );
    }

    fn merge(&mut self, other: Self) {
        add_counter(
            &mut self.key_actions_observed,
            other.key_actions_observed,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.composition_callbacks_observed,
            other.composition_callbacks_observed,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.composition_finalized_callbacks_observed,
            other.composition_finalized_callbacks_observed,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.value_callbacks_observed,
            other.value_callbacks_observed,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.value_read_errors,
            other.value_read_errors,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.composition_read_errors,
            other.composition_read_errors,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.selection_read_errors,
            other.selection_read_errors,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.value_callbacks_without_output,
            other.value_callbacks_without_output,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.tracker_outputs_emitted,
            other.tracker_outputs_emitted,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.key_actions_not_emitted_at_boundary,
            other.key_actions_not_emitted_at_boundary,
            &mut self.counter_saturated,
        );
        add_counter(
            &mut self.key_buffer_resets,
            other.key_buffer_resets,
            &mut self.counter_saturated,
        );
        self.counter_saturated |= other.counter_saturated;
    }
}

fn increment_counter(value: &mut u64, saturated: &mut bool) {
    add_counter(value, 1, saturated);
}

fn add_counter(value: &mut u64, amount: u64, saturated: &mut bool) {
    match value.checked_add(amount) {
        Some(sum) => *value = sum,
        None => {
            *value = u64::MAX;
            *saturated = true;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureIntegrityV1 {
    pub baseline_epoch: u64,
    pub close_reason: SegmentCloseReason,
    pub counters: CaptureIntegrityCountersV1,
}

impl CaptureIntegrityV1 {
    pub fn new(
        baseline_epoch: u64,
        close_reason: SegmentCloseReason,
        counters: CaptureIntegrityCountersV1,
        capsule_events: usize,
    ) -> Result<Self, ContinuousCaptureError> {
        let capsule_events = u64::try_from(capsule_events)
            .map_err(|_| ContinuousCaptureError::LimitExceeded("capsule event count"))?;
        if counters.tracker_outputs_emitted != capsule_events {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity tracker output count",
            ));
        }
        if counters.composition_read_errors > counters.composition_callbacks_observed {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity composition callback counts",
            ));
        }
        if counters.selection_read_errors
            > counters
                .value_callbacks_observed
                .saturating_sub(counters.value_read_errors)
        {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity selection read errors",
            ));
        }
        let classified = counters
            .value_read_errors
            .saturating_add(counters.value_callbacks_without_output)
            .saturating_add(counters.tracker_outputs_emitted);
        if classified != counters.value_callbacks_observed {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity value callback counts",
            ));
        }
        if counters.key_buffer_resets > counters.key_actions_observed {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity key buffer resets",
            ));
        }
        if counters.counter_saturated
            && ![
                counters.key_actions_observed,
                counters.composition_callbacks_observed,
                counters.composition_finalized_callbacks_observed,
                counters.value_callbacks_observed,
                counters.value_read_errors,
                counters.composition_read_errors,
                counters.selection_read_errors,
                counters.value_callbacks_without_output,
                counters.tracker_outputs_emitted,
                counters.key_actions_not_emitted_at_boundary,
                counters.key_buffer_resets,
            ]
            .contains(&u64::MAX)
        {
            return Err(ContinuousCaptureError::InvalidField(
                "integrity saturation marker",
            ));
        }
        Ok(Self {
            baseline_epoch,
            close_reason,
            counters,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousSegmentMetadata {
    pub session_id: String,
    pub sequence: u64,
    pub started_unix_ms: u64,
    pub ended_unix_ms: u64,
    pub session_kind: CaptureSessionKind,
    pub producer_version: String,
    pub capture_profile: String,
}

impl ContinuousSegmentMetadata {
    pub fn new(
        session_id: String,
        sequence: u64,
        started_unix_ms: u64,
        ended_unix_ms: u64,
        session_kind: CaptureSessionKind,
        producer_version: String,
        capture_profile: String,
    ) -> Result<Self, ContinuousCaptureError> {
        validate_session_id(&session_id)?;
        validate_version_field(&producer_version, "producer version")?;
        validate_version_field(&capture_profile, "capture profile")?;
        if ended_unix_ms < started_unix_ms {
            return Err(ContinuousCaptureError::InvalidField(
                "segment end precedes segment start",
            ));
        }
        Ok(Self {
            session_id,
            sequence,
            started_unix_ms,
            ended_unix_ms,
            session_kind,
            producer_version,
            capture_profile,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousSegmentV1 {
    session_id: String,
    sequence: u64,
    started_unix_ms: u64,
    ended_unix_ms: u64,
    session_kind: CaptureSessionKind,
    producer_version: String,
    capture_profile: String,
    capsule: EventCapsuleV1,
}

impl ContinuousSegmentV1 {
    pub fn new(
        metadata: ContinuousSegmentMetadata,
        capsule: EventCapsuleV1,
    ) -> Result<Self, ContinuousCaptureError> {
        Ok(Self {
            session_id: metadata.session_id,
            sequence: metadata.sequence,
            started_unix_ms: metadata.started_unix_ms,
            ended_unix_ms: metadata.ended_unix_ms,
            session_kind: metadata.session_kind,
            producer_version: metadata.producer_version,
            capture_profile: metadata.capture_profile,
            capsule,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn started_unix_ms(&self) -> u64 {
        self.started_unix_ms
    }

    pub fn ended_unix_ms(&self) -> u64 {
        self.ended_unix_ms
    }

    pub fn session_kind(&self) -> CaptureSessionKind {
        self.session_kind
    }

    pub fn producer_version(&self) -> &str {
        &self.producer_version
    }

    pub fn capture_profile(&self) -> &str {
        &self.capture_profile
    }

    pub fn capsule(&self) -> &EventCapsuleV1 {
        &self.capsule
    }

    pub fn into_capsule(self) -> EventCapsuleV1 {
        self.capsule
    }

    pub fn into_parts(self) -> (ContinuousSegmentMetadata, EventCapsuleV1) {
        (
            ContinuousSegmentMetadata {
                session_id: self.session_id,
                sequence: self.sequence,
                started_unix_ms: self.started_unix_ms,
                ended_unix_ms: self.ended_unix_ms,
                session_kind: self.session_kind,
                producer_version: self.producer_version,
                capture_profile: self.capture_profile,
            },
            self.capsule,
        )
    }

    pub fn to_plaintext(&self) -> Result<Vec<u8>, ContinuousCaptureError> {
        let capsule = self.capsule.to_text()?;
        let header = format!(
            "{CONTINUOUS_SEGMENT_SCHEMA_V1}\n\
             session_id={}\n\
             sequence={}\n\
             started_unix_ms={}\n\
             ended_unix_ms={}\n\
             session_kind={}\n\
             producer_version={}\n\
             capture_profile={}\n\
             capsule_utf8_bytes={}\n",
            self.session_id,
            self.sequence,
            self.started_unix_ms,
            self.ended_unix_ms,
            self.session_kind.as_str(),
            self.producer_version,
            self.capture_profile,
            capsule.len()
        );
        let mut output = Vec::with_capacity(header.len() + capsule.len());
        output.extend_from_slice(header.as_bytes());
        output.extend_from_slice(capsule.as_bytes());
        Ok(output)
    }

    pub fn from_plaintext(input: &[u8]) -> Result<Self, ContinuousCaptureError> {
        let input = std::str::from_utf8(input)
            .map_err(|_| ContinuousCaptureError::InvalidField("segment UTF-8"))?;
        let (header, capsule) = split_header(input, 9)?;
        let mut lines = header.lines();
        expect_header_line(&mut lines, CONTINUOUS_SEGMENT_SCHEMA_V1)?;
        let session_id = parse_header_value(&mut lines, "session_id")?.to_owned();
        let sequence = parse_u64(parse_header_value(&mut lines, "sequence")?, "sequence")?;
        let started_unix_ms = parse_u64(
            parse_header_value(&mut lines, "started_unix_ms")?,
            "started_unix_ms",
        )?;
        let ended_unix_ms = parse_u64(
            parse_header_value(&mut lines, "ended_unix_ms")?,
            "ended_unix_ms",
        )?;
        let session_kind =
            CaptureSessionKind::parse(parse_header_value(&mut lines, "session_kind")?)?;
        let producer_version = parse_header_value(&mut lines, "producer_version")?.to_owned();
        let capture_profile = parse_header_value(&mut lines, "capture_profile")?.to_owned();
        let capsule_bytes = parse_usize(
            parse_header_value(&mut lines, "capsule_utf8_bytes")?,
            "capsule_utf8_bytes",
        )?;
        if capsule.len() != capsule_bytes {
            return Err(ContinuousCaptureError::InvalidField("capsule byte count"));
        }
        let capsule = EventCapsuleV1::from_text(capsule)?;
        let metadata = ContinuousSegmentMetadata::new(
            session_id,
            sequence,
            started_unix_ms,
            ended_unix_ms,
            session_kind,
            producer_version,
            capture_profile,
        )?;
        Self::new(metadata, capsule)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousSegmentV2 {
    metadata: ContinuousSegmentMetadata,
    integrity: CaptureIntegrityV1,
    capsule: EventCapsuleV1,
}

impl ContinuousSegmentV2 {
    pub fn new(
        metadata: ContinuousSegmentMetadata,
        integrity: CaptureIntegrityV1,
        capsule: EventCapsuleV1,
    ) -> Result<Self, ContinuousCaptureError> {
        let integrity = CaptureIntegrityV1::new(
            integrity.baseline_epoch,
            integrity.close_reason,
            integrity.counters,
            capsule.events().len(),
        )?;
        Ok(Self {
            metadata,
            integrity,
            capsule,
        })
    }

    pub fn metadata(&self) -> &ContinuousSegmentMetadata {
        &self.metadata
    }

    pub fn session_id(&self) -> &str {
        &self.metadata.session_id
    }

    pub fn sequence(&self) -> u64 {
        self.metadata.sequence
    }

    pub fn capture_profile(&self) -> &str {
        &self.metadata.capture_profile
    }

    pub fn integrity(&self) -> &CaptureIntegrityV1 {
        &self.integrity
    }

    pub fn capsule(&self) -> &EventCapsuleV1 {
        &self.capsule
    }

    pub fn into_parts(
        self,
    ) -> (
        ContinuousSegmentMetadata,
        CaptureIntegrityV1,
        EventCapsuleV1,
    ) {
        (self.metadata, self.integrity, self.capsule)
    }

    pub fn to_plaintext(&self) -> Result<Vec<u8>, ContinuousCaptureError> {
        let capsule = self.capsule.to_text()?;
        let counters = &self.integrity.counters;
        let header = format!(
            "{CONTINUOUS_SEGMENT_SCHEMA_V2}\n\
             session_id={}\n\
             sequence={}\n\
             started_unix_ms={}\n\
             ended_unix_ms={}\n\
             session_kind={}\n\
             producer_version={}\n\
             capture_profile={}\n\
             integrity_schema={}\n\
             baseline_epoch={}\n\
             close_reason={}\n\
             key_actions_observed={}\n\
             composition_callbacks_observed={}\n\
             composition_finalized_callbacks_observed={}\n\
             value_callbacks_observed={}\n\
             value_read_errors={}\n\
             composition_read_errors={}\n\
             selection_read_errors={}\n\
             value_callbacks_without_output={}\n\
             tracker_outputs_emitted={}\n\
             key_actions_not_emitted_at_boundary={}\n\
             key_buffer_resets={}\n\
             counter_saturated={}\n\
             capsule_utf8_bytes={}\n",
            self.metadata.session_id,
            self.metadata.sequence,
            self.metadata.started_unix_ms,
            self.metadata.ended_unix_ms,
            self.metadata.session_kind.as_str(),
            self.metadata.producer_version,
            self.metadata.capture_profile,
            CAPTURE_INTEGRITY_SCHEMA_V1,
            self.integrity.baseline_epoch,
            self.integrity.close_reason.as_str(),
            counters.key_actions_observed,
            counters.composition_callbacks_observed,
            counters.composition_finalized_callbacks_observed,
            counters.value_callbacks_observed,
            counters.value_read_errors,
            counters.composition_read_errors,
            counters.selection_read_errors,
            counters.value_callbacks_without_output,
            counters.tracker_outputs_emitted,
            counters.key_actions_not_emitted_at_boundary,
            counters.key_buffer_resets,
            counters.counter_saturated,
            capsule.len()
        );
        let mut output = Vec::with_capacity(header.len() + capsule.len());
        output.extend_from_slice(header.as_bytes());
        output.extend_from_slice(capsule.as_bytes());
        Ok(output)
    }

    pub fn from_plaintext(input: &[u8]) -> Result<Self, ContinuousCaptureError> {
        let input = std::str::from_utf8(input)
            .map_err(|_| ContinuousCaptureError::InvalidField("segment UTF-8"))?;
        let (header, capsule) = split_header(input, 24)?;
        let mut lines = header.lines();
        expect_header_line(&mut lines, CONTINUOUS_SEGMENT_SCHEMA_V2)?;
        let session_id = parse_header_value(&mut lines, "session_id")?.to_owned();
        let sequence = parse_u64(parse_header_value(&mut lines, "sequence")?, "sequence")?;
        let started_unix_ms = parse_u64(
            parse_header_value(&mut lines, "started_unix_ms")?,
            "started_unix_ms",
        )?;
        let ended_unix_ms = parse_u64(
            parse_header_value(&mut lines, "ended_unix_ms")?,
            "ended_unix_ms",
        )?;
        let session_kind =
            CaptureSessionKind::parse(parse_header_value(&mut lines, "session_kind")?)?;
        let producer_version = parse_header_value(&mut lines, "producer_version")?.to_owned();
        let capture_profile = parse_header_value(&mut lines, "capture_profile")?.to_owned();
        if parse_header_value(&mut lines, "integrity_schema")? != CAPTURE_INTEGRITY_SCHEMA_V1 {
            return Err(ContinuousCaptureError::InvalidField("integrity schema"));
        }
        let baseline_epoch = parse_u64(
            parse_header_value(&mut lines, "baseline_epoch")?,
            "baseline_epoch",
        )?;
        let close_reason =
            SegmentCloseReason::parse(parse_header_value(&mut lines, "close_reason")?)?;
        let counters = CaptureIntegrityCountersV1 {
            key_actions_observed: parse_u64(
                parse_header_value(&mut lines, "key_actions_observed")?,
                "key_actions_observed",
            )?,
            composition_callbacks_observed: parse_u64(
                parse_header_value(&mut lines, "composition_callbacks_observed")?,
                "composition_callbacks_observed",
            )?,
            composition_finalized_callbacks_observed: parse_u64(
                parse_header_value(&mut lines, "composition_finalized_callbacks_observed")?,
                "composition_finalized_callbacks_observed",
            )?,
            value_callbacks_observed: parse_u64(
                parse_header_value(&mut lines, "value_callbacks_observed")?,
                "value_callbacks_observed",
            )?,
            value_read_errors: parse_u64(
                parse_header_value(&mut lines, "value_read_errors")?,
                "value_read_errors",
            )?,
            composition_read_errors: parse_u64(
                parse_header_value(&mut lines, "composition_read_errors")?,
                "composition_read_errors",
            )?,
            selection_read_errors: parse_u64(
                parse_header_value(&mut lines, "selection_read_errors")?,
                "selection_read_errors",
            )?,
            value_callbacks_without_output: parse_u64(
                parse_header_value(&mut lines, "value_callbacks_without_output")?,
                "value_callbacks_without_output",
            )?,
            tracker_outputs_emitted: parse_u64(
                parse_header_value(&mut lines, "tracker_outputs_emitted")?,
                "tracker_outputs_emitted",
            )?,
            key_actions_not_emitted_at_boundary: parse_u64(
                parse_header_value(&mut lines, "key_actions_not_emitted_at_boundary")?,
                "key_actions_not_emitted_at_boundary",
            )?,
            key_buffer_resets: parse_u64(
                parse_header_value(&mut lines, "key_buffer_resets")?,
                "key_buffer_resets",
            )?,
            counter_saturated: parse_bool(
                parse_header_value(&mut lines, "counter_saturated")?,
                "counter_saturated",
            )?,
        };
        let capsule_bytes = parse_usize(
            parse_header_value(&mut lines, "capsule_utf8_bytes")?,
            "capsule_utf8_bytes",
        )?;
        if capsule.len() != capsule_bytes {
            return Err(ContinuousCaptureError::InvalidField("capsule byte count"));
        }
        let capsule = EventCapsuleV1::from_text(capsule)?;
        let metadata = ContinuousSegmentMetadata::new(
            session_id,
            sequence,
            started_unix_ms,
            ended_unix_ms,
            session_kind,
            producer_version,
            capture_profile,
        )?;
        let integrity = CaptureIntegrityV1::new(
            baseline_epoch,
            close_reason,
            counters,
            capsule.events().len(),
        )?;
        Self::new(metadata, integrity, capsule)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodedContinuousSegment {
    V1(ContinuousSegmentV1),
    V2(ContinuousSegmentV2),
}

impl DecodedContinuousSegment {
    pub fn from_plaintext(input: &[u8]) -> Result<Self, ContinuousCaptureError> {
        if input.starts_with(CONTINUOUS_SEGMENT_SCHEMA_V1.as_bytes()) {
            ContinuousSegmentV1::from_plaintext(input).map(Self::V1)
        } else if input.starts_with(CONTINUOUS_SEGMENT_SCHEMA_V2.as_bytes()) {
            ContinuousSegmentV2::from_plaintext(input).map(Self::V2)
        } else {
            Err(ContinuousCaptureError::InvalidField("segment schema"))
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        ContinuousSegmentMetadata,
        Option<CaptureIntegrityV1>,
        EventCapsuleV1,
    ) {
        match self {
            Self::V1(segment) => {
                let (metadata, capsule) = segment.into_parts();
                (metadata, None, capsule)
            }
            Self::V2(segment) => {
                let (metadata, integrity, capsule) = segment.into_parts();
                (metadata, Some(integrity), capsule)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedSegmentEnvelopeV1 {
    protected: Vec<u8>,
}

impl ProtectedSegmentEnvelopeV1 {
    pub fn new(protected: Vec<u8>) -> Result<Self, ContinuousCaptureError> {
        if protected.is_empty() {
            return Err(ContinuousCaptureError::InvalidField(
                "protected payload is empty",
            ));
        }
        if protected.len() > MAX_PROTECTED_SEGMENT_BYTES {
            return Err(ContinuousCaptureError::LimitExceeded(
                "protected payload bytes",
            ));
        }
        Ok(Self { protected })
    }

    pub fn protected(&self) -> &[u8] {
        &self.protected
    }

    pub fn into_protected(self) -> Vec<u8> {
        self.protected
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ContinuousCaptureError> {
        let length = u32::try_from(self.protected.len())
            .map_err(|_| ContinuousCaptureError::LimitExceeded("protected payload bytes"))?;
        let mut output =
            Vec::with_capacity(PROTECTED_SEGMENT_SCHEMA_V1.len() + 4 + self.protected.len());
        output.extend_from_slice(PROTECTED_SEGMENT_SCHEMA_V1);
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(&self.protected);
        Ok(output)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self, ContinuousCaptureError> {
        let header_len = PROTECTED_SEGMENT_SCHEMA_V1.len();
        if input.len() < header_len + 4 || &input[..header_len] != PROTECTED_SEGMENT_SCHEMA_V1 {
            return Err(ContinuousCaptureError::InvalidField(
                "protected segment schema",
            ));
        }
        let length = u32::from_le_bytes(
            input[header_len..header_len + 4]
                .try_into()
                .expect("four-byte slice"),
        ) as usize;
        let protected = &input[header_len + 4..];
        if protected.len() != length {
            return Err(ContinuousCaptureError::InvalidField(
                "protected payload byte count",
            ));
        }
        Self::new(protected.to_vec())
    }
}

pub trait DataProtector {
    fn protection_name(&self) -> &'static str;
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError>;
    fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError>;
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsUserDataProtector;

#[cfg(windows)]
impl DataProtector for WindowsUserDataProtector {
    fn protection_name(&self) -> &'static str {
        "windows-dpapi-current-user"
    }

    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
        windows_dpapi(plaintext, true)
    }

    fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
        windows_dpapi(protected, false)
    }
}

#[cfg(windows)]
fn windows_dpapi(input: &[u8], protect: bool) -> Result<Vec<u8>, ContinuousCaptureError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;

    if input.is_empty() {
        return Err(ContinuousCaptureError::InvalidField("DPAPI input is empty"));
    }
    let input_len = u32::try_from(input.len())
        .map_err(|_| ContinuousCaptureError::LimitExceeded("DPAPI input bytes"))?;
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input_len,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output_blob = CRYPT_INTEGER_BLOB::default();
    let operation = if protect {
        // SAFETY: input_blob points to `input` for the duration of the call.
        // DPAPI initializes output_blob on success; no machine-wide flag is
        // supplied, so the result stays bound to the current Windows user.
        unsafe {
            CryptProtectData(
                &input_blob,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output_blob,
            )
        }
    } else {
        // SAFETY: Same ownership rules as CryptProtectData. No description is
        // requested, so only output_blob requires LocalFree.
        unsafe {
            CryptUnprotectData(
                &input_blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output_blob,
            )
        }
    };
    operation.map_err(|error| ContinuousCaptureError::Protection(error.to_string()))?;
    if output_blob.pbData.is_null() || output_blob.cbData == 0 {
        return Err(ContinuousCaptureError::Protection(
            "DPAPI returned an empty output".to_owned(),
        ));
    }
    // SAFETY: DPAPI returned output_blob with exactly cbData initialized bytes.
    let output = unsafe {
        std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec()
    };
    // SAFETY: Microsoft documents that DPAPI's output pbData is released with
    // LocalFree exactly once after the caller finishes copying it.
    let not_freed = unsafe { LocalFree(Some(HLOCAL(output_blob.pbData.cast()))) };
    if !not_freed.0.is_null() {
        return Err(ContinuousCaptureError::Protection(
            "LocalFree could not release the DPAPI output".to_owned(),
        ));
    }
    Ok(output)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentWriteReceipt {
    pub path: PathBuf,
    pub sequence: u64,
    pub events: usize,
    pub protected_bytes: usize,
    pub protection: &'static str,
}

pub struct ProtectedSegmentWriterConfig {
    root: PathBuf,
    session_id: String,
    session_kind: CaptureSessionKind,
    producer_version: String,
    capture_profile: String,
    max_events: usize,
    max_age: Duration,
}

impl ProtectedSegmentWriterConfig {
    pub fn new(
        root: PathBuf,
        session_id: String,
        session_kind: CaptureSessionKind,
        producer_version: String,
        capture_profile: String,
        max_events: usize,
        max_age: Duration,
    ) -> Result<Self, ContinuousCaptureError> {
        validate_session_id(&session_id)?;
        validate_version_field(&producer_version, "producer version")?;
        validate_version_field(&capture_profile, "capture profile")?;
        if max_events == 0 || max_events > crate::MAX_EVENT_CAPSULE_EVENTS {
            return Err(ContinuousCaptureError::InvalidField("segment event limit"));
        }
        if max_age.is_zero() {
            return Err(ContinuousCaptureError::InvalidField("segment age limit"));
        }
        Ok(Self {
            root,
            session_id,
            session_kind,
            producer_version,
            capture_profile,
            max_events,
            max_age,
        })
    }
}

pub struct ProtectedSegmentWriter<P> {
    root: PathBuf,
    session_id: String,
    session_kind: CaptureSessionKind,
    producer_version: String,
    capture_profile: String,
    sequence: u64,
    written_segments: u64,
    written_events: u64,
    recorder: EventCapsuleRecorder,
    integrity_counters: CaptureIntegrityCountersV1,
    baseline_epoch: u64,
    baseline_open: bool,
    session_ended: bool,
    segment_started_unix_ms: u64,
    segment_started: Instant,
    max_events: usize,
    max_age: Duration,
    protector: P,
}

impl<P: DataProtector> ProtectedSegmentWriter<P> {
    pub fn new(
        config: ProtectedSegmentWriterConfig,
        protector: P,
    ) -> Result<Self, ContinuousCaptureError> {
        fs::create_dir_all(&config.root)?;
        let now = unix_time_ms()?;
        Ok(Self {
            root: config.root,
            session_id: config.session_id,
            session_kind: config.session_kind,
            producer_version: config.producer_version,
            capture_profile: config.capture_profile,
            sequence: 0,
            written_segments: 0,
            written_events: 0,
            recorder: EventCapsuleRecorder::default(),
            integrity_counters: CaptureIntegrityCountersV1::default(),
            baseline_epoch: 0,
            baseline_open: false,
            session_ended: false,
            segment_started_unix_ms: now,
            segment_started: Instant::now(),
            max_events: config.max_events,
            max_age: config.max_age,
            protector,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn pending_events(&self) -> usize {
        self.recorder.len()
    }

    pub fn written_segments(&self) -> u64 {
        self.written_segments
    }

    pub fn written_events(&self) -> u64 {
        self.written_events
    }

    pub fn baseline_epoch(&self) -> u64 {
        self.baseline_epoch
    }

    pub fn baseline_open(&self) -> bool {
        self.baseline_open
    }

    pub fn session_ended(&self) -> bool {
        self.session_ended
    }

    /// Starts a new in-memory tracker baseline. A serialized segment is never
    /// allowed to straddle this boundary.
    pub fn start_new_baseline_epoch(&mut self) -> Result<u64, ContinuousCaptureError> {
        if self.session_ended {
            return Err(ContinuousCaptureError::InvalidField(
                "baseline started after session end",
            ));
        }
        if self.baseline_open {
            return Err(ContinuousCaptureError::InvalidField(
                "baseline started before prior baseline closed",
            ));
        }
        if !self.recorder.is_empty() {
            return Err(ContinuousCaptureError::InvalidField(
                "baseline changed with pending segment events",
            ));
        }
        let next_epoch = self
            .baseline_epoch
            .checked_add(1)
            .ok_or(ContinuousCaptureError::LimitExceeded("baseline epoch"))?;
        let started_unix_ms = unix_time_ms()?;
        self.baseline_epoch = next_epoch;
        self.baseline_open = true;
        self.integrity_counters = CaptureIntegrityCountersV1::default();
        self.segment_started_unix_ms = started_unix_ms;
        self.segment_started = Instant::now();
        Ok(self.baseline_epoch)
    }

    pub fn absorb_integrity(
        &mut self,
        counters: CaptureIntegrityCountersV1,
    ) -> Result<(), ContinuousCaptureError> {
        if counters == CaptureIntegrityCountersV1::default() {
            return Ok(());
        }
        self.require_open_baseline()?;
        self.integrity_counters.merge(counters);
        Ok(())
    }

    /// Atomically associates the callback counters accumulated by the adapter
    /// with an optional tracker output. The event-limit rotation happens only
    /// after both pieces have entered the same encrypted segment.
    pub fn observe_batch(
        &mut self,
        counters: CaptureIntegrityCountersV1,
        output: Option<TrackerOutput>,
    ) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        self.require_open_baseline()?;
        if self.recorder.len() >= self.max_events {
            return Err(ContinuousCaptureError::InvalidField(
                "unflushed segment event limit",
            ));
        }
        self.integrity_counters.merge(counters);
        if let Some(output) = output {
            let elapsed_ms =
                u64::try_from(self.segment_started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.recorder.observe(elapsed_ms, output)?;
            self.integrity_counters.observe_tracker_output();
        }
        if self.recorder.len() >= self.max_events {
            self.flush_with_reason(SegmentCloseReason::Capacity)
        } else {
            Ok(None)
        }
    }

    pub fn observe(
        &mut self,
        output: TrackerOutput,
    ) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        let mut counters = CaptureIntegrityCountersV1::default();
        counters.observe_value_callback(true);
        self.observe_batch(counters, Some(output))
    }

    pub fn flush_if_due(&mut self) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        if !self.recorder.is_empty() && self.segment_started.elapsed() >= self.max_age {
            self.flush_with_reason(SegmentCloseReason::Timer)
        } else {
            Ok(None)
        }
    }

    pub fn flush(&mut self) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        self.flush_with_reason(SegmentCloseReason::SessionEnd)
    }

    pub fn flush_with_reason(
        &mut self,
        close_reason: SegmentCloseReason,
    ) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        if self.session_ended {
            return Err(ContinuousCaptureError::InvalidField(
                "segment closed after session end",
            ));
        }
        if close_reason == SegmentCloseReason::Continuity
            && !self.baseline_open
            && self.recorder.is_empty()
            && self.integrity_counters == CaptureIntegrityCountersV1::default()
        {
            return Ok(None);
        }
        if close_reason != SegmentCloseReason::SessionEnd && !self.baseline_open {
            return Err(ContinuousCaptureError::InvalidField(
                "segment closed without open baseline",
            ));
        }
        if self.recorder.is_empty() {
            if matches!(
                close_reason,
                SegmentCloseReason::Capacity | SegmentCloseReason::Timer
            ) {
                return Ok(None);
            }
            self.integrity_counters = CaptureIntegrityCountersV1::default();
            self.apply_successful_close(close_reason);
            return Ok(None);
        }
        self.require_open_baseline()?;
        let events = self.recorder.len();
        let next_sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ContinuousCaptureError::LimitExceeded("segment sequence"))?;
        let next_written_segments =
            self.written_segments
                .checked_add(1)
                .ok_or(ContinuousCaptureError::LimitExceeded(
                    "written segment count",
                ))?;
        let event_count = u64::try_from(events)
            .map_err(|_| ContinuousCaptureError::LimitExceeded("written event count"))?;
        let next_written_events = self
            .written_events
            .checked_add(event_count)
            .ok_or(ContinuousCaptureError::LimitExceeded("written event count"))?;
        let ended_unix_ms = unix_time_ms()?;
        let capsule = self.recorder.finish()?;
        let metadata = ContinuousSegmentMetadata::new(
            self.session_id.clone(),
            self.sequence,
            self.segment_started_unix_ms,
            ended_unix_ms,
            self.session_kind,
            self.producer_version.clone(),
            self.capture_profile.clone(),
        )?;
        let integrity = CaptureIntegrityV1::new(
            self.baseline_epoch,
            close_reason,
            self.integrity_counters.clone(),
            events,
        )?;
        let segment = ContinuousSegmentV2::new(metadata, integrity, capsule)?;
        let mut plaintext = segment.to_plaintext()?;
        let protected_result = self.protector.protect(&plaintext);
        plaintext.fill(0);
        let protected = protected_result?;
        let envelope = ProtectedSegmentEnvelopeV1::new(protected)?;
        let bytes = envelope.to_bytes()?;
        let file_name = format!("segment-{}-{:08}.zcs", self.session_id, self.sequence);
        let path = self.root.join(file_name);
        write_create_new_atomic(&path, &bytes)?;
        let receipt = SegmentWriteReceipt {
            path,
            sequence: self.sequence,
            events,
            protected_bytes: bytes.len(),
            protection: self.protector.protection_name(),
        };
        self.sequence = next_sequence;
        self.written_segments = next_written_segments;
        self.written_events = next_written_events;
        self.recorder.reset();
        self.integrity_counters = CaptureIntegrityCountersV1::default();
        self.segment_started_unix_ms = ended_unix_ms;
        self.segment_started = Instant::now();
        self.apply_successful_close(close_reason);
        Ok(Some(receipt))
    }

    fn require_open_baseline(&self) -> Result<(), ContinuousCaptureError> {
        if self.session_ended {
            return Err(ContinuousCaptureError::InvalidField(
                "event observed after session end",
            ));
        }
        if !self.baseline_open {
            return Err(ContinuousCaptureError::InvalidField(
                "event observed without open baseline",
            ));
        }
        Ok(())
    }

    fn apply_successful_close(&mut self, close_reason: SegmentCloseReason) {
        match close_reason {
            SegmentCloseReason::Capacity | SegmentCloseReason::Timer => {}
            SegmentCloseReason::Continuity => self.baseline_open = false,
            SegmentCloseReason::SessionEnd => {
                self.baseline_open = false;
                self.session_ended = true;
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuousCaptureError {
    InvalidField(&'static str),
    LimitExceeded(&'static str),
    Protection(String),
    Capsule(EventCapsuleError),
    Io(String),
}

impl fmt::Display for ContinuousCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(field) => write!(formatter, "invalid continuous capture {field}"),
            Self::LimitExceeded(field) => {
                write!(formatter, "continuous capture exceeded {field} limit")
            }
            Self::Protection(error) => write!(formatter, "data protection failed: {error}"),
            Self::Capsule(error) => error.fmt(formatter),
            Self::Io(error) => write!(formatter, "continuous capture I/O failed: {error}"),
        }
    }
}

impl Error for ContinuousCaptureError {}

impl From<EventCapsuleError> for ContinuousCaptureError {
    fn from(value: EventCapsuleError) -> Self {
        Self::Capsule(value)
    }
}

impl From<std::io::Error> for ContinuousCaptureError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

fn validate_session_id(value: &str) -> Result<(), ContinuousCaptureError> {
    if value.is_empty()
        || value.len() > MAX_SESSION_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ContinuousCaptureError::InvalidField("session id"));
    }
    Ok(())
}

fn validate_version_field(value: &str, field: &'static str) -> Result<(), ContinuousCaptureError> {
    if value.is_empty()
        || value.len() > MAX_VERSION_FIELD_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
    {
        return Err(ContinuousCaptureError::InvalidField(field));
    }
    Ok(())
}

fn split_header(input: &str, line_count: usize) -> Result<(&str, &str), ContinuousCaptureError> {
    let mut newline_count = 0;
    for (index, byte) in input.bytes().enumerate() {
        if byte == b'\n' {
            newline_count += 1;
            if newline_count == line_count {
                return Ok((&input[..index], &input[index + 1..]));
            }
        }
    }
    Err(ContinuousCaptureError::InvalidField("segment header"))
}

fn expect_header_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    expected: &str,
) -> Result<(), ContinuousCaptureError> {
    if lines.next() == Some(expected) {
        Ok(())
    } else {
        Err(ContinuousCaptureError::InvalidField("segment schema"))
    }
}

fn parse_header_value<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<&'a str, ContinuousCaptureError> {
    lines
        .next()
        .and_then(|line| line.strip_prefix(field))
        .and_then(|value| value.strip_prefix('='))
        .ok_or(ContinuousCaptureError::InvalidField(field))
}

fn parse_u64(value: &str, field: &'static str) -> Result<u64, ContinuousCaptureError> {
    value
        .parse()
        .map_err(|_| ContinuousCaptureError::InvalidField(field))
}

fn parse_usize(value: &str, field: &'static str) -> Result<usize, ContinuousCaptureError> {
    value
        .parse()
        .map_err(|_| ContinuousCaptureError::InvalidField(field))
}

fn parse_bool(value: &str, field: &'static str) -> Result<bool, ContinuousCaptureError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ContinuousCaptureError::InvalidField(field)),
    }
}

fn unix_time_ms() -> Result<u64, ContinuousCaptureError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ContinuousCaptureError::InvalidField("system clock"))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| ContinuousCaptureError::LimitExceeded("Unix timestamp"))
}

fn write_create_new_atomic(target: &Path, contents: &[u8]) -> Result<(), ContinuousCaptureError> {
    let parent = target
        .parent()
        .ok_or(ContinuousCaptureError::InvalidField("segment parent"))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ContinuousCaptureError::InvalidField("segment file name"))?;
    let mut temporary = None;
    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or(ContinuousCaptureError::InvalidField(
        "temporary segment allocation",
    ))?;
    let write_result = (|| -> Result<(), ContinuousCaptureError> {
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    if let Err(error) = fs::hard_link(&temporary_path, target) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.into());
    }
    // The hard link is the publication commit point: target now names the
    // fully written, synchronized bytes. Failure to remove the encrypted
    // temporary alias must not make the caller retry an already-published
    // sequence forever.
    let _ = fs::remove_file(&temporary_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_INTEGRITY_SCHEMA_V1, CONTINUOUS_SEGMENT_SCHEMA_V1, CONTINUOUS_SEGMENT_SCHEMA_V2,
        CaptureIntegrityCountersV1, CaptureIntegrityV1, CaptureSessionKind, ContinuousCaptureError,
        ContinuousSegmentMetadata, ContinuousSegmentV1, ContinuousSegmentV2, DataProtector,
        DecodedContinuousSegment, PROTECTED_SEGMENT_SCHEMA_V1, ProtectedSegmentEnvelopeV1,
        ProtectedSegmentWriter, ProtectedSegmentWriterConfig, SegmentCloseReason,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, EventCapsuleV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput,
    };
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    #[derive(Clone, Copy)]
    struct TestProtector;

    impl DataProtector for TestProtector {
        fn protection_name(&self) -> &'static str {
            "test-reversed"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(protected.iter().rev().copied().collect())
        }
    }

    #[derive(Clone)]
    struct FailOnceProtector {
        fail_next: Arc<AtomicBool>,
    }

    impl FailOnceProtector {
        fn new() -> Self {
            Self {
                fail_next: Arc::new(AtomicBool::new(true)),
            }
        }
    }

    impl DataProtector for FailOnceProtector {
        fn protection_name(&self) -> &'static str {
            "test-fail-once"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            if self.fail_next.swap(false, Ordering::AcqRel) {
                Err(ContinuousCaptureError::Protection(
                    "synthetic failure".to_owned(),
                ))
            } else {
                Ok(plaintext.iter().rev().copied().collect())
            }
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(protected.iter().rev().copied().collect())
        }
    }

    fn private_commit(text: &str) -> TrackerOutput {
        TrackerOutput::Commit(CommitRecord {
            keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space],
            keys_complete: true,
            composition: "mao".to_owned(),
            change: TextDelta {
                start: 0,
                deleted: "mao".to_owned(),
                inserted: text.to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
            document_change: TextDelta {
                start: 0,
                deleted: String::new(),
                inserted: text.to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
        })
    }

    fn segment() -> ContinuousSegmentV1 {
        let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 7,
            output: private_commit("猫"),
        }])
        .unwrap();
        let metadata = ContinuousSegmentMetadata::new(
            "1234-77".to_owned(),
            3,
            10,
            20,
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v1".to_owned(),
        )
        .unwrap();
        ContinuousSegmentV1::new(metadata, capsule).unwrap()
    }

    #[test]
    fn plaintext_segment_round_trips_strictly() {
        let segment = segment();
        let plaintext = segment.to_plaintext().unwrap();
        assert!(plaintext.starts_with(CONTINUOUS_SEGMENT_SCHEMA_V1.as_bytes()));
        assert_eq!(
            ContinuousSegmentV1::from_plaintext(&plaintext).unwrap(),
            segment
        );
        assert_eq!(segment.producer_version(), "0.1.0");
        assert_eq!(segment.capture_profile(), "synthetic-v1");
        let (metadata, capsule) = segment.clone().into_parts();
        assert_eq!(metadata.session_id, segment.session_id());
        assert_eq!(metadata.sequence, segment.sequence());
        assert_eq!(metadata.started_unix_ms, segment.started_unix_ms());
        assert_eq!(metadata.ended_unix_ms, segment.ended_unix_ms());
        assert_eq!(metadata.session_kind, segment.session_kind());
        assert_eq!(metadata.producer_version, segment.producer_version());
        assert_eq!(metadata.capture_profile, segment.capture_profile());
        assert_eq!(capsule, *segment.capsule());

        let mut truncated = plaintext;
        truncated.pop();
        assert!(ContinuousSegmentV1::from_plaintext(&truncated).is_err());
    }

    #[test]
    fn v2_integrity_segment_round_trips_and_dispatches_without_changing_v1() {
        let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 7,
            output: private_commit("猫"),
        }])
        .unwrap();
        let metadata = ContinuousSegmentMetadata::new(
            "1234-88".to_owned(),
            4,
            20,
            30,
            CaptureSessionKind::Daily,
            "0.1.0+continuous.7".to_owned(),
            "codex-uia-v2".to_owned(),
        )
        .unwrap();
        let counters = CaptureIntegrityCountersV1 {
            key_actions_observed: 3,
            composition_callbacks_observed: 2,
            composition_finalized_callbacks_observed: 1,
            value_callbacks_observed: 2,
            value_read_errors: 0,
            composition_read_errors: 0,
            selection_read_errors: 1,
            value_callbacks_without_output: 1,
            tracker_outputs_emitted: 1,
            key_actions_not_emitted_at_boundary: 0,
            key_buffer_resets: 0,
            counter_saturated: false,
        };
        let integrity = CaptureIntegrityV1::new(2, SegmentCloseReason::Timer, counters, 1).unwrap();
        let v2_segment = ContinuousSegmentV2::new(metadata, integrity, capsule).unwrap();
        let plaintext = v2_segment.to_plaintext().unwrap();
        assert!(plaintext.starts_with(CONTINUOUS_SEGMENT_SCHEMA_V2.as_bytes()));
        assert!(String::from_utf8_lossy(&plaintext).contains(CAPTURE_INTEGRITY_SCHEMA_V1));
        assert_eq!(
            ContinuousSegmentV2::from_plaintext(&plaintext).unwrap(),
            v2_segment.clone()
        );
        assert!(matches!(
            DecodedContinuousSegment::from_plaintext(&plaintext).unwrap(),
            DecodedContinuousSegment::V2(_)
        ));
        assert!(matches!(
            DecodedContinuousSegment::from_plaintext(&segment().to_plaintext().unwrap()).unwrap(),
            DecodedContinuousSegment::V1(_)
        ));

        let mut invalid = String::from_utf8(plaintext).unwrap();
        invalid = invalid.replace("tracker_outputs_emitted=1", "tracker_outputs_emitted=0");
        assert!(ContinuousSegmentV2::from_plaintext(invalid.as_bytes()).is_err());

        let saturated_bypass = v2_segment
            .to_plaintext()
            .map(String::from_utf8)
            .unwrap()
            .unwrap()
            .replace("value_callbacks_observed=2", "value_callbacks_observed=0")
            .replace("counter_saturated=false", "counter_saturated=true");
        assert!(ContinuousSegmentV2::from_plaintext(saturated_bypass.as_bytes()).is_err());

        let impossible_saturation_marker = v2_segment
            .to_plaintext()
            .map(String::from_utf8)
            .unwrap()
            .unwrap()
            .replace("counter_saturated=false", "counter_saturated=true");
        assert!(
            ContinuousSegmentV2::from_plaintext(impossible_saturation_marker.as_bytes()).is_err()
        );
    }

    #[test]
    fn opaque_envelope_rejects_wrong_schema_and_length() {
        let envelope = ProtectedSegmentEnvelopeV1::new(vec![1, 2, 3]).unwrap();
        let bytes = envelope.to_bytes().unwrap();
        assert!(bytes.starts_with(PROTECTED_SEGMENT_SCHEMA_V1));
        assert_eq!(
            ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap(),
            envelope
        );

        let mut wrong_length = bytes.clone();
        let index = PROTECTED_SEGMENT_SCHEMA_V1.len();
        wrong_length[index..index + 4].copy_from_slice(&9_u32.to_le_bytes());
        assert!(ProtectedSegmentEnvelopeV1::from_bytes(&wrong_length).is_err());
        assert!(ProtectedSegmentEnvelopeV1::from_bytes(b"wrong").is_err());
    }

    #[test]
    fn protected_writer_rotates_without_writing_plaintext() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "test-1".to_owned(),
            CaptureSessionKind::Theme,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();
        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        assert!(writer.observe(private_commit("私密甲")).unwrap().is_none());
        let receipt = writer
            .observe(private_commit("私密乙"))
            .unwrap()
            .expect("second event rotates");
        assert_eq!(receipt.events, 2);
        assert_eq!(receipt.protection, "test-reversed");
        let bytes = fs::read(&receipt.path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("私密"));

        let envelope = ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap();
        let plaintext = TestProtector.unprotect(envelope.protected()).unwrap();
        let decoded = ContinuousSegmentV2::from_plaintext(&plaintext).unwrap();
        assert_eq!(decoded.sequence(), 0);
        assert_eq!(decoded.capsule().events().len(), 2);
        assert_eq!(
            decoded.integrity().close_reason,
            SegmentCloseReason::Capacity
        );
        assert_eq!(decoded.integrity().counters.tracker_outputs_emitted, 2);
        assert_eq!(writer.written_segments(), 1);
        assert_eq!(writer.written_events(), 2);
        assert!(writer.baseline_open());
        assert!(!writer.session_ended());
        assert!(writer.flush().unwrap().is_none());
        assert!(!writer.baseline_open());
        assert!(writer.session_ended());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

        fs::remove_file(receipt.path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn writer_keeps_segments_within_one_baseline_and_skips_integrity_only_files() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-epoch-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "epoch-test".to_owned(),
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();
        assert!(!writer.baseline_open());
        assert!(!writer.session_ended());
        assert!(writer.observe(private_commit("未开始")).is_err());
        let mut before_baseline = CaptureIntegrityCountersV1::default();
        before_baseline.observe_value_read_error();
        assert!(writer.absorb_integrity(before_baseline).is_err());
        assert!(
            writer
                .absorb_integrity(CaptureIntegrityCountersV1::default())
                .is_ok()
        );
        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        assert!(writer.baseline_open());
        let mut failure_only = CaptureIntegrityCountersV1::default();
        failure_only.observe_value_read_error();
        writer.absorb_integrity(failure_only).unwrap();
        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Continuity)
                .unwrap()
                .is_none()
        );
        assert!(!writer.baseline_open());
        assert!(writer.observe(private_commit("边界外")).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 2);
        assert!(writer.baseline_open());
        assert!(writer.observe(private_commit("猫")).unwrap().is_none());
        assert!(writer.start_new_baseline_epoch().is_err());
        let receipt = writer
            .flush_with_reason(SegmentCloseReason::SessionEnd)
            .unwrap()
            .unwrap();
        let bytes = fs::read(&receipt.path).unwrap();
        let envelope = ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap();
        let plaintext = TestProtector.unprotect(envelope.protected()).unwrap();
        let decoded = ContinuousSegmentV2::from_plaintext(&plaintext).unwrap();
        assert_eq!(decoded.integrity().baseline_epoch, 2);
        assert_eq!(decoded.integrity().counters.value_read_errors, 0);
        assert!(!writer.baseline_open());
        assert!(writer.session_ended());
        assert!(writer.start_new_baseline_epoch().is_err());
        assert!(writer.observe(private_commit("结束后")).is_err());
        let mut after_end = CaptureIntegrityCountersV1::default();
        after_end.observe_value_read_error();
        assert!(writer.absorb_integrity(after_end).is_err());

        fs::remove_file(receipt.path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn empty_timer_and_continuity_keep_the_lifecycle_explicit_without_writing_files() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-empty-close-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "empty-close-test".to_owned(),
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();

        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Timer)
                .unwrap()
                .is_none()
        );
        assert!(writer.baseline_open());
        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Continuity)
                .unwrap()
                .is_none()
        );
        assert!(!writer.baseline_open());
        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Continuity)
                .unwrap()
                .is_none()
        );
        assert!(writer.flush().unwrap().is_none());
        assert!(writer.session_ended());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn empty_timer_does_not_discard_integrity_from_the_open_baseline() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-empty-timer-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "empty-timer-test".to_owned(),
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();
        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        let mut failure_only = CaptureIntegrityCountersV1::default();
        failure_only.observe_value_read_error();
        writer.absorb_integrity(failure_only).unwrap();

        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Timer)
                .unwrap()
                .is_none()
        );
        assert!(writer.observe(private_commit("猫")).unwrap().is_none());
        let receipt = writer.flush().unwrap().unwrap();
        let bytes = fs::read(&receipt.path).unwrap();
        let envelope = ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap();
        let plaintext = TestProtector.unprotect(envelope.protected()).unwrap();
        let decoded = ContinuousSegmentV2::from_plaintext(&plaintext).unwrap();
        assert_eq!(decoded.integrity().counters.value_read_errors, 1);
        assert_eq!(decoded.integrity().counters.tracker_outputs_emitted, 1);

        fs::remove_file(receipt.path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn failed_nonempty_write_does_not_close_or_consume_the_baseline() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-fail-once-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "fail-once-test".to_owned(),
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let protector = FailOnceProtector::new();
        let mut writer = ProtectedSegmentWriter::new(config, protector.clone()).unwrap();
        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        assert!(writer.observe(private_commit("猫")).unwrap().is_none());

        assert!(
            writer
                .flush_with_reason(SegmentCloseReason::Continuity)
                .is_err()
        );
        assert!(writer.baseline_open());
        assert!(!writer.session_ended());
        assert_eq!(writer.pending_events(), 1);
        assert_eq!(writer.written_segments(), 0);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        let receipt = writer
            .flush_with_reason(SegmentCloseReason::Continuity)
            .unwrap()
            .unwrap();
        assert!(!writer.baseline_open());
        assert_eq!(writer.pending_events(), 0);
        assert_eq!(writer.written_segments(), 1);
        let bytes = fs::read(&receipt.path).unwrap();
        let envelope = ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap();
        let plaintext = protector.unprotect(envelope.protected()).unwrap();
        let decoded = ContinuousSegmentV2::from_plaintext(&plaintext).unwrap();
        assert_eq!(decoded.integrity().baseline_epoch, 1);
        assert_eq!(
            decoded.integrity().close_reason,
            SegmentCloseReason::Continuity
        );

        fs::remove_file(receipt.path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn counter_overflow_is_rejected_before_a_segment_is_published() {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-protected-writer-overflow-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let config = ProtectedSegmentWriterConfig::new(
            root.clone(),
            "overflow-test".to_owned(),
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            "synthetic-v2".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();
        assert_eq!(writer.start_new_baseline_epoch().unwrap(), 1);
        assert!(writer.observe(private_commit("猫")).unwrap().is_none());
        writer.sequence = u64::MAX;

        assert!(writer.flush().is_err());
        assert!(writer.baseline_open());
        assert!(!writer.session_ended());
        assert_eq!(writer.pending_events(), 1);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn invalid_session_metadata_is_rejected() {
        assert!(
            ContinuousSegmentMetadata::new(
                "../escape".to_owned(),
                0,
                0,
                0,
                CaptureSessionKind::Daily,
                "0.1.0".to_owned(),
                "synthetic-v1".to_owned(),
            )
            .is_err()
        );
        assert!(CaptureSessionKind::parse("unknown").is_err());

        assert!(
            ContinuousSegmentMetadata::new(
                "valid-1".to_owned(),
                0,
                0,
                0,
                CaptureSessionKind::Daily,
                "bad version with spaces".to_owned(),
                "synthetic-v1".to_owned(),
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_current_user_dpapi_round_trips_synthetic_bytes() {
        use super::WindowsUserDataProtector;

        let plaintext = b"synthetic-only-private-segment";
        let protected = WindowsUserDataProtector.protect(plaintext).unwrap();
        assert_ne!(protected, plaintext);
        assert_eq!(
            WindowsUserDataProtector.unprotect(&protected).unwrap(),
            plaintext
        );
    }
}
