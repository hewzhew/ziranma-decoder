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
pub const PROTECTED_SEGMENT_SCHEMA_V1: &[u8] = b"ziranma-dpapi-segment-v1\0";
pub const CONTINUOUS_PRODUCER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+continuous.6");
pub const CODEX_CAPTURE_PROFILE_V1: &str = "codex-uia-v1";
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

    pub fn observe(
        &mut self,
        output: TrackerOutput,
    ) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        let mut receipt = None;
        if self.recorder.len() >= self.max_events {
            receipt = self.flush()?;
        }
        let elapsed_ms =
            u64::try_from(self.segment_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.recorder.observe(elapsed_ms, output)?;
        if self.recorder.len() >= self.max_events {
            receipt = self.flush()?;
        }
        Ok(receipt)
    }

    pub fn flush_if_due(&mut self) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        if !self.recorder.is_empty() && self.segment_started.elapsed() >= self.max_age {
            self.flush()
        } else {
            Ok(None)
        }
    }

    pub fn flush(&mut self) -> Result<Option<SegmentWriteReceipt>, ContinuousCaptureError> {
        if self.recorder.is_empty() {
            return Ok(None);
        }
        let events = self.recorder.len();
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
        let segment = ContinuousSegmentV1::new(metadata, capsule)?;
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
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ContinuousCaptureError::LimitExceeded("segment sequence"))?;
        self.written_segments =
            self.written_segments
                .checked_add(1)
                .ok_or(ContinuousCaptureError::LimitExceeded(
                    "written segment count",
                ))?;
        self.written_events = self
            .written_events
            .checked_add(
                u64::try_from(events)
                    .map_err(|_| ContinuousCaptureError::LimitExceeded("written event count"))?,
            )
            .ok_or(ContinuousCaptureError::LimitExceeded("written event count"))?;
        self.recorder.reset();
        self.segment_started_unix_ms = ended_unix_ms;
        self.segment_started = Instant::now();
        Ok(Some(receipt))
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
    let result = (|| -> Result<(), ContinuousCaptureError> {
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary_path, target)?;
        fs::remove_file(&temporary_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        CONTINUOUS_SEGMENT_SCHEMA_V1, CaptureSessionKind, ContinuousCaptureError,
        ContinuousSegmentMetadata, ContinuousSegmentV1, DataProtector, PROTECTED_SEGMENT_SCHEMA_V1,
        ProtectedSegmentEnvelopeV1, ProtectedSegmentWriter, ProtectedSegmentWriterConfig,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, EventCapsuleV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput,
    };
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
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

        let mut truncated = plaintext;
        truncated.pop();
        assert!(ContinuousSegmentV1::from_plaintext(&truncated).is_err());
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
            "synthetic-v1".to_owned(),
            2,
            Duration::from_secs(60),
        )
        .unwrap();
        let mut writer = ProtectedSegmentWriter::new(config, TestProtector).unwrap();
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
        let decoded = ContinuousSegmentV1::from_plaintext(&plaintext).unwrap();
        assert_eq!(decoded.sequence(), 0);
        assert_eq!(decoded.capsule().events().len(), 2);
        assert_eq!(writer.written_segments(), 1);
        assert_eq!(writer.written_events(), 2);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

        fs::remove_file(receipt.path).unwrap();
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
