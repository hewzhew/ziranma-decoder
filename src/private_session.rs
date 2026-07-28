//! Explicit, bounded loading of one current-user-protected capture session.
//!
//! This module never scans for sessions and never writes decrypted material.
//! Callers must name one validated session id; segments are then opened only
//! through their contiguous, predictable names under the repository-private
//! continuous-capture directory.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;

use crate::{
    CODEX_CAPTURE_PROFILE_V1, CODEX_CAPTURE_PROFILE_V2, CaptureIntegrityV1, CaptureSessionKind,
    ContinuousSegmentMetadata, DataProtector, DecodedContinuousSegment, EventCapsuleV1,
    ProtectedSegmentEnvelopeV1, SegmentCloseReason,
};

const MAX_PROTECTED_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_SEGMENTS: u64 = 1_000_000;

pub struct ProtectedSessionSegment {
    pub metadata: ContinuousSegmentMetadata,
    pub integrity: Option<CaptureIntegrityV1>,
    pub capsule: EventCapsuleV1,
}

impl fmt::Debug for ProtectedSessionSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedSessionSegment")
            .field("sequence", &self.metadata.sequence)
            .field("events", &self.capsule.events().len())
            .field("contains_private_text", &true)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedSessionErrorKind {
    InvalidSessionId,
    PrivateRootUnavailable,
    MissingSession,
    UnsafeSegment,
    ReadFailed,
    TooLarge,
    DecodeOrUnprotectFailed,
    MetadataMismatch,
    ContinuityViolation,
    TooManySegments,
}

impl ProtectedSessionErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSessionId => "invalid-session-id",
            Self::PrivateRootUnavailable => "private-root-unavailable",
            Self::MissingSession => "missing-session",
            Self::UnsafeSegment => "unsafe-segment",
            Self::ReadFailed => "read-failed",
            Self::TooLarge => "too-large",
            Self::DecodeOrUnprotectFailed => "decode-or-unprotect-failed",
            Self::MetadataMismatch => "metadata-mismatch",
            Self::ContinuityViolation => "continuity-violation",
            Self::TooManySegments => "too-many-segments",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedSessionError {
    kind: ProtectedSessionErrorKind,
}

impl ProtectedSessionError {
    fn new(kind: ProtectedSessionErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(self) -> ProtectedSessionErrorKind {
        self.kind
    }
}

impl fmt::Display for ProtectedSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protected private session could not be loaded: {}; paths and content were suppressed",
            self.kind.as_str()
        )
    }
}

impl Error for ProtectedSessionError {}

pub struct ProtectedSessionReader<P> {
    manifest_dir: PathBuf,
    protector: P,
}

impl<P: DataProtector> ProtectedSessionReader<P> {
    pub fn new(manifest_dir: impl Into<PathBuf>, protector: P) -> Self {
        Self {
            manifest_dir: manifest_dir.into(),
            protector,
        }
    }

    pub fn load(
        &self,
        session_id: &str,
    ) -> Result<Vec<ProtectedSessionSegment>, ProtectedSessionError> {
        validate_session_id(session_id)?;
        let root = self
            .validate_private_root()
            .map_err(ProtectedSessionError::new)?;
        let mut progress = None;
        let mut segments = Vec::new();

        for sequence in 0..MAX_SESSION_SEGMENTS {
            let target = self
                .manifest_dir
                .join("data/private/continuous-capture")
                .join(format!("segment-{session_id}-{sequence:08}.zcs"));
            let metadata = match fs::symlink_metadata(&target) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && sequence == 0 => {
                    return Err(ProtectedSessionError::new(
                        ProtectedSessionErrorKind::MissingSession,
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(segments),
                Err(_) => {
                    return Err(ProtectedSessionError::new(
                        ProtectedSessionErrorKind::ReadFailed,
                    ));
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::UnsafeSegment,
                ));
            }

            let canonical_target = fs::canonicalize(&target)
                .map_err(|_| ProtectedSessionError::new(ProtectedSessionErrorKind::ReadFailed))?;
            if canonical_target.parent() != Some(root.as_path()) {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::UnsafeSegment,
                ));
            }

            let mut file = File::open(&canonical_target)
                .map_err(|_| ProtectedSessionError::new(ProtectedSessionErrorKind::ReadFailed))?;
            let opened_metadata = file
                .metadata()
                .map_err(|_| ProtectedSessionError::new(ProtectedSessionErrorKind::ReadFailed))?;
            if !opened_metadata.is_file() {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::UnsafeSegment,
                ));
            }
            if opened_metadata.len() > MAX_PROTECTED_FILE_BYTES {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::TooLarge,
                ));
            }

            let mut encoded = Vec::new();
            file.by_ref()
                .take(MAX_PROTECTED_FILE_BYTES + 1)
                .read_to_end(&mut encoded)
                .map_err(|_| ProtectedSessionError::new(ProtectedSessionErrorKind::ReadFailed))?;
            if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_PROTECTED_FILE_BYTES {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::TooLarge,
                ));
            }

            let segment = self.decode_segment(&encoded)?;
            if segment.metadata.session_id != session_id
                || segment.metadata.sequence != sequence
                || canonical_target.file_name().and_then(|name| name.to_str())
                    != Some(format!("segment-{session_id}-{sequence:08}.zcs").as_str())
            {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::MetadataMismatch,
                ));
            }
            observe_continuity(&mut progress, &segment)?;
            segments.push(segment);
        }

        Err(ProtectedSessionError::new(
            ProtectedSessionErrorKind::TooManySegments,
        ))
    }

    fn validate_private_root(&self) -> Result<PathBuf, ProtectedSessionErrorKind> {
        let manifest_metadata = fs::symlink_metadata(&self.manifest_dir)
            .map_err(|_| ProtectedSessionErrorKind::PrivateRootUnavailable)?;
        if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_dir() {
            return Err(ProtectedSessionErrorKind::PrivateRootUnavailable);
        }

        let mut current = self.manifest_dir.clone();
        for component in ["data", "private", "continuous-capture"] {
            current.push(component);
            let metadata = fs::symlink_metadata(&current)
                .map_err(|_| ProtectedSessionErrorKind::PrivateRootUnavailable)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ProtectedSessionErrorKind::PrivateRootUnavailable);
            }
        }

        let canonical_manifest = fs::canonicalize(&self.manifest_dir)
            .map_err(|_| ProtectedSessionErrorKind::PrivateRootUnavailable)?;
        let canonical_root = fs::canonicalize(&current)
            .map_err(|_| ProtectedSessionErrorKind::PrivateRootUnavailable)?;
        if !canonical_root.starts_with(canonical_manifest) {
            return Err(ProtectedSessionErrorKind::PrivateRootUnavailable);
        }
        Ok(canonical_root)
    }

    fn decode_segment(
        &self,
        encoded: &[u8],
    ) -> Result<ProtectedSessionSegment, ProtectedSessionError> {
        let envelope = ProtectedSegmentEnvelopeV1::from_bytes(encoded).map_err(|_| {
            ProtectedSessionError::new(ProtectedSessionErrorKind::DecodeOrUnprotectFailed)
        })?;
        let mut plaintext = self
            .protector
            .unprotect(envelope.protected())
            .map_err(|_| {
                ProtectedSessionError::new(ProtectedSessionErrorKind::DecodeOrUnprotectFailed)
            })?;
        if plaintext.len() > MAX_PROTECTED_FILE_BYTES as usize {
            plaintext.fill(0);
            return Err(ProtectedSessionError::new(
                ProtectedSessionErrorKind::TooLarge,
            ));
        }
        let decoded = DecodedContinuousSegment::from_plaintext(&plaintext);
        plaintext.fill(0);
        let (metadata, integrity, capsule) = decoded
            .map_err(|_| {
                ProtectedSessionError::new(ProtectedSessionErrorKind::DecodeOrUnprotectFailed)
            })?
            .into_parts();
        let expected_profile = if integrity.is_some() {
            CODEX_CAPTURE_PROFILE_V2
        } else {
            CODEX_CAPTURE_PROFILE_V1
        };
        if metadata.capture_profile != expected_profile {
            return Err(ProtectedSessionError::new(
                ProtectedSessionErrorKind::MetadataMismatch,
            ));
        }
        Ok(ProtectedSessionSegment {
            metadata,
            integrity,
            capsule,
        })
    }
}

fn validate_session_id(value: &str) -> Result<(), ProtectedSessionError> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ProtectedSessionError::new(
            ProtectedSessionErrorKind::InvalidSessionId,
        ));
    }
    Ok(())
}

struct SessionProgress {
    session_kind: CaptureSessionKind,
    producer_version: String,
    capture_profile: String,
    last_sequence: u64,
    last_ended_unix_ms: u64,
    last_baseline_epoch: Option<u64>,
    last_close_reason: Option<SegmentCloseReason>,
}

fn observe_continuity(
    progress: &mut Option<SessionProgress>,
    segment: &ProtectedSessionSegment,
) -> Result<(), ProtectedSessionError> {
    let metadata = &segment.metadata;
    let integrity = segment.integrity.as_ref();
    if let Some(previous) = progress {
        let invalid = previous.session_kind != metadata.session_kind
            || previous.producer_version != metadata.producer_version
            || previous.capture_profile != metadata.capture_profile
            || metadata.sequence != previous.last_sequence.saturating_add(1)
            || metadata.started_unix_ms < previous.last_ended_unix_ms
            || previous.last_baseline_epoch.is_some() != integrity.is_some();
        if invalid {
            return Err(ProtectedSessionError::new(
                ProtectedSessionErrorKind::ContinuityViolation,
            ));
        }
        if let Some(integrity) = integrity {
            let previous_epoch = previous
                .last_baseline_epoch
                .expect("integrity availability checked above");
            if integrity.baseline_epoch < previous_epoch
                || previous.last_close_reason == Some(SegmentCloseReason::SessionEnd)
                || (integrity.baseline_epoch == previous_epoch
                    && previous.last_close_reason == Some(SegmentCloseReason::Continuity))
            {
                return Err(ProtectedSessionError::new(
                    ProtectedSessionErrorKind::ContinuityViolation,
                ));
            }
            previous.last_baseline_epoch = Some(integrity.baseline_epoch);
            previous.last_close_reason = Some(integrity.close_reason);
        }
        previous.last_sequence = metadata.sequence;
        previous.last_ended_unix_ms = metadata.ended_unix_ms;
    } else {
        *progress = Some(SessionProgress {
            session_kind: metadata.session_kind,
            producer_version: metadata.producer_version.clone(),
            capture_profile: metadata.capture_profile.clone(),
            last_sequence: metadata.sequence,
            last_ended_unix_ms: metadata.ended_unix_ms,
            last_baseline_epoch: integrity.map(|value| value.baseline_epoch),
            last_close_reason: integrity.map(|value| value.close_reason),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProtectedSessionErrorKind, ProtectedSessionReader};
    use crate::{
        CODEX_CAPTURE_PROFILE_V1, CaptureSessionKind, CommitRecord, ContinuousCaptureError,
        ContinuousSegmentMetadata, ContinuousSegmentV1, DataProtector, DeltaPositionEvidence,
        EventCapsuleV1, ProtectedSegmentEnvelopeV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Copy)]
    struct IdentityProtector;

    impl DataProtector for IdentityProtector {
        fn protection_name(&self) -> &'static str {
            "test-identity"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(plaintext.to_vec())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(protected.to_vec())
        }
    }

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ziranma-private-session-test-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("data/private/continuous-capture")).unwrap();
            Self { root }
        }

        fn write_segment(&self, session_id: &str, sequence: u64, text: &str) {
            let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
                elapsed_ms: sequence.saturating_mul(10),
                output: TrackerOutput::Commit(CommitRecord {
                    keys: vec![RawKey::Letter('m'), RawKey::Space],
                    keys_complete: true,
                    composition: "mao".to_owned(),
                    change: TextDelta {
                        start: 0,
                        deleted: "mao".to_owned(),
                        inserted: text.to_owned(),
                        position_evidence: DeltaPositionEvidence::UniqueText,
                    },
                    document_change: TextDelta {
                        start: sequence as usize,
                        deleted: String::new(),
                        inserted: text.to_owned(),
                        position_evidence: DeltaPositionEvidence::Caret,
                    },
                }),
            }])
            .unwrap();
            let metadata = ContinuousSegmentMetadata::new(
                session_id.to_owned(),
                sequence,
                100 + sequence.saturating_mul(10),
                109 + sequence.saturating_mul(10),
                CaptureSessionKind::Daily,
                "test-producer".to_owned(),
                CODEX_CAPTURE_PROFILE_V1.to_owned(),
            )
            .unwrap();
            let plaintext = ContinuousSegmentV1::new(metadata, capsule)
                .unwrap()
                .to_plaintext()
                .unwrap();
            let protected = IdentityProtector.protect(&plaintext).unwrap();
            let bytes = ProtectedSegmentEnvelopeV1::new(protected)
                .unwrap()
                .to_bytes()
                .unwrap();
            let path = self
                .root
                .join("data/private/continuous-capture")
                .join(format!("segment-{session_id}-{sequence:08}.zcs"));
            fs::write(path, bytes).unwrap();
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn loads_only_the_named_contiguous_session() {
        let workspace = TestWorkspace::new();
        workspace.write_segment("1000-1", 0, "猫");
        workspace.write_segment("1000-1", 1, "猫");
        workspace.write_segment("2000-2", 0, "狗");

        let segments = ProtectedSessionReader::new(workspace.root(), IdentityProtector)
            .load("1000-1")
            .unwrap();

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].metadata.session_id, "1000-1");
        assert_eq!(segments[1].metadata.sequence, 1);
    }

    #[test]
    fn rejects_invalid_or_missing_session_ids_without_scanning() {
        let workspace = TestWorkspace::new();
        let reader = ProtectedSessionReader::new(workspace.root(), IdentityProtector);

        assert_eq!(
            reader.load("../private").unwrap_err().kind(),
            ProtectedSessionErrorKind::InvalidSessionId
        );
        assert_eq!(
            reader.load("1000-1").unwrap_err().kind(),
            ProtectedSessionErrorKind::MissingSession
        );
    }

    #[test]
    fn rejects_metadata_that_does_not_match_the_predictable_name() {
        let workspace = TestWorkspace::new();
        workspace.write_segment("other-session", 0, "猫");
        let root = workspace.root().join("data/private/continuous-capture");
        fs::rename(
            root.join("segment-other-session-00000000.zcs"),
            root.join("segment-1000-1-00000000.zcs"),
        )
        .unwrap();

        let error = ProtectedSessionReader::new(workspace.root(), IdentityProtector)
            .load("1000-1")
            .unwrap_err();
        assert_eq!(error.kind(), ProtectedSessionErrorKind::MetadataMismatch);
    }
}
