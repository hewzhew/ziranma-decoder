//! Strict metadata for immutable candidate-package payloads.
//!
//! The core accepts an explicitly supplied manifest and payload. It performs
//! no path resolution, file discovery, persistence, decryption, or network
//! access.

use std::error::Error;
use std::fmt;

use crate::{
    CANDIDATE_SNAPSHOT_SCHEMA_V1, CandidateSnapshot, CandidateSnapshotDescriptor,
    CandidateSnapshotError, MAX_CANDIDATE_SNAPSHOT_BYTES, MAX_CANDIDATE_SNAPSHOT_ENTRIES,
    candidate_payload_fingerprint, parse_lexicon_tsv,
};

/// First immutable candidate-package manifest schema.
pub const CANDIDATE_PACKAGE_SCHEMA_V1: &str = "ziranma-candidate-package-v1";
/// Lexicon payload format supported by the first package schema.
pub const CANDIDATE_PACKAGE_LEXICON_TSV_V1: &str = "ziranma-lexicon-tsv-v1";
/// Maximum bytes accepted for one textual package manifest.
pub const MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES: usize = 4 * 1024;

/// Strict metadata for one immutable lexicon payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidatePackageManifest {
    revision: String,
    contains_private_text: bool,
    payload_bytes: usize,
    payload_fingerprint: u64,
    entry_count: usize,
}

impl CandidatePackageManifest {
    /// Builds deterministic metadata for one already-materialized payload.
    ///
    /// This function does not write the payload or infer whether its text is
    /// private. The caller must supply that policy decision explicitly.
    pub fn from_payload(
        revision: &str,
        contains_private_text: bool,
        lexicon_tsv: &str,
    ) -> Result<Self, CandidatePackageError> {
        if !super::candidate_snapshot::valid_candidate_snapshot_revision(revision) {
            return Err(CandidatePackageError::InvalidRevision);
        }
        let payload_bytes = lexicon_tsv.len();
        if payload_bytes == 0 || payload_bytes > MAX_CANDIDATE_SNAPSHOT_BYTES {
            return Err(CandidatePackageError::InvalidPayloadBytes);
        }
        let entry_count = parse_lexicon_tsv(lexicon_tsv)
            .map_err(|_| CandidatePackageError::Snapshot(CandidateSnapshotError::Lexicon))?
            .len();
        if entry_count == 0 || entry_count > MAX_CANDIDATE_SNAPSHOT_ENTRIES {
            return Err(CandidatePackageError::InvalidEntryCount);
        }

        Ok(Self {
            revision: revision.to_owned(),
            contains_private_text,
            payload_bytes,
            payload_fingerprint: candidate_payload_fingerprint(lexicon_tsv.as_bytes()),
            entry_count,
        })
    }

    /// Parses the exact eight-line v1 manifest without accepting unknown data.
    pub fn parse(contents: &str) -> Result<Self, CandidatePackageError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES {
            return Err(CandidatePackageError::InvalidManifestSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidatePackageError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 9 || !lines[8].is_empty() {
            return Err(CandidatePackageError::InvalidStructure);
        }

        if field(lines[0], "schema")? != CANDIDATE_PACKAGE_SCHEMA_V1 {
            return Err(CandidatePackageError::UnsupportedSchema);
        }
        if field(lines[1], "snapshot_schema")? != CANDIDATE_SNAPSHOT_SCHEMA_V1 {
            return Err(CandidatePackageError::UnsupportedSnapshotSchema);
        }
        let revision = field(lines[2], "revision")?;
        if !super::candidate_snapshot::valid_candidate_snapshot_revision(revision) {
            return Err(CandidatePackageError::InvalidRevision);
        }
        let contains_private_text = match field(lines[3], "contains_private_text")? {
            "false" => false,
            "true" => true,
            _ => return Err(CandidatePackageError::InvalidPrivacyFlag),
        };
        if field(lines[4], "payload_format")? != CANDIDATE_PACKAGE_LEXICON_TSV_V1 {
            return Err(CandidatePackageError::UnsupportedPayloadFormat);
        }
        let payload_bytes = parse_decimal(field(lines[5], "payload_bytes")?)
            .ok_or(CandidatePackageError::InvalidPayloadBytes)?;
        if payload_bytes == 0 || payload_bytes > MAX_CANDIDATE_SNAPSHOT_BYTES {
            return Err(CandidatePackageError::InvalidPayloadBytes);
        }
        let payload_fingerprint =
            parse_lower_hex_u64(field(lines[6], "payload_fingerprint_fnv1a64")?)
                .ok_or(CandidatePackageError::InvalidPayloadFingerprint)?;
        let entry_count = parse_decimal(field(lines[7], "entry_count")?)
            .ok_or(CandidatePackageError::InvalidEntryCount)?;
        if entry_count == 0 || entry_count > MAX_CANDIDATE_SNAPSHOT_ENTRIES {
            return Err(CandidatePackageError::InvalidEntryCount);
        }

        Ok(Self {
            revision: revision.to_owned(),
            contains_private_text,
            payload_bytes,
            payload_fingerprint,
            entry_count,
        })
    }

    /// Validates the explicitly supplied payload and constructs its snapshot.
    pub fn load_snapshot(
        &self,
        lexicon_tsv: &str,
    ) -> Result<CandidateSnapshot, CandidatePackageError> {
        CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: &self.revision,
            contains_private_text: self.contains_private_text,
            lexicon_tsv,
            expected_payload_bytes: self.payload_bytes,
            expected_payload_fingerprint: self.payload_fingerprint,
            expected_entry_count: self.entry_count,
        })
        .map_err(CandidatePackageError::Snapshot)
    }

    /// Validates payload identity and an independently counted row total.
    ///
    /// Lightweight consumers can use this after applying their own stricter
    /// payload profile. Unlike [`Self::load_snapshot`], it never parses the
    /// generic lexicon or constructs a decoder index.
    pub fn validate_payload_metadata(
        &self,
        lexicon_tsv: &str,
        actual_entry_count: usize,
    ) -> Result<(), CandidatePackageError> {
        let actual_payload_bytes = lexicon_tsv.len();
        if actual_payload_bytes != self.payload_bytes {
            return Err(CandidatePackageError::Snapshot(
                CandidateSnapshotError::PayloadLengthMismatch {
                    expected: self.payload_bytes,
                    actual: actual_payload_bytes,
                },
            ));
        }
        let actual_fingerprint = candidate_payload_fingerprint(lexicon_tsv.as_bytes());
        if actual_fingerprint != self.payload_fingerprint {
            return Err(CandidatePackageError::Snapshot(
                CandidateSnapshotError::PayloadFingerprintMismatch {
                    expected: self.payload_fingerprint,
                    actual: actual_fingerprint,
                },
            ));
        }
        if actual_entry_count != self.entry_count {
            return Err(CandidatePackageError::Snapshot(
                CandidateSnapshotError::EntryCountMismatch {
                    expected: self.entry_count,
                    actual: actual_entry_count,
                },
            ));
        }
        Ok(())
    }

    /// Renders the canonical eight-line v1 manifest.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_PACKAGE_SCHEMA_V1}\n\
             snapshot_schema={CANDIDATE_SNAPSHOT_SCHEMA_V1}\n\
             revision={}\n\
             contains_private_text={}\n\
             payload_format={CANDIDATE_PACKAGE_LEXICON_TSV_V1}\n\
             payload_bytes={}\n\
             payload_fingerprint_fnv1a64={:016x}\n\
             entry_count={}\n",
            self.revision,
            self.contains_private_text,
            self.payload_bytes,
            self.payload_fingerprint,
            self.entry_count
        )
    }

    /// Returns the validated data revision.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Reports whether the manifest marks the payload as private text.
    pub fn contains_private_text(&self) -> bool {
        self.contains_private_text
    }

    /// Returns the exact declared payload size.
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    /// Returns the exact declared entry count.
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }
}

/// Errors returned while validating a candidate package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidatePackageError {
    /// The manifest is empty or exceeds its fixed byte boundary.
    InvalidManifestSize,
    /// The manifest does not use the exact eight-line LF-terminated shape.
    InvalidStructure,
    /// A required key is missing, duplicated, reordered, or empty.
    InvalidField,
    /// The package schema is not supported.
    UnsupportedSchema,
    /// The snapshot schema is not supported.
    UnsupportedSnapshotSchema,
    /// The revision is outside the bounded ASCII grammar.
    InvalidRevision,
    /// The privacy flag is not exactly `true` or `false`.
    InvalidPrivacyFlag,
    /// The payload format is not the exact supported lexicon TSV format.
    UnsupportedPayloadFormat,
    /// The payload length is malformed, zero, or over the snapshot limit.
    InvalidPayloadBytes,
    /// The payload fingerprint is not exactly 16 lowercase hexadecimal digits.
    InvalidPayloadFingerprint,
    /// The entry count is malformed, zero, or over the snapshot limit.
    InvalidEntryCount,
    /// The payload did not match the validated manifest.
    Snapshot(CandidateSnapshotError),
}

impl fmt::Display for CandidatePackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifestSize => write!(formatter, "候选包清单大小无效"),
            Self::InvalidStructure => write!(formatter, "候选包清单结构无效"),
            Self::InvalidField => write!(formatter, "候选包清单字段无效"),
            Self::UnsupportedSchema => write!(formatter, "不支持的候选包格式"),
            Self::UnsupportedSnapshotSchema => write!(formatter, "不支持的候选快照格式"),
            Self::InvalidRevision => write!(formatter, "候选包版本标识无效"),
            Self::InvalidPrivacyFlag => write!(formatter, "候选包隐私标记无效"),
            Self::UnsupportedPayloadFormat => write!(formatter, "不支持的候选包载荷格式"),
            Self::InvalidPayloadBytes => write!(formatter, "候选包载荷长度无效"),
            Self::InvalidPayloadFingerprint => write!(formatter, "候选包载荷指纹无效"),
            Self::InvalidEntryCount => write!(formatter, "候选包词条数无效"),
            Self::Snapshot(error) => write!(formatter, "候选包载荷校验失败：{error}"),
        }
    }
}

impl Error for CandidatePackageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Snapshot(error) => Some(error),
            _ => None,
        }
    }
}

fn field<'a>(line: &'a str, expected_key: &str) -> Result<&'a str, CandidatePackageError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidatePackageError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidatePackageError::InvalidField);
    }
    Ok(value)
}

fn parse_decimal(value: &str) -> Option<usize> {
    if !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse().ok()
}

fn parse_lower_hex_u64(value: &str) -> Option<u64> {
    if value.len() != 16
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    u64::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = include_str!("../tests/fixtures/public/demo_candidate_manifest.zcm");
    const LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

    #[test]
    fn fixed_public_package_builds_the_same_bounded_snapshot() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        assert_eq!(manifest.revision(), "tsf-public-demo-v1");
        assert!(!manifest.contains_private_text());
        assert_eq!(manifest.payload_bytes(), 1_132);
        assert_eq!(manifest.entry_count(), 50);
        let snapshot = manifest.load_snapshot(LEXICON).unwrap();
        assert_eq!(snapshot.revision(), manifest.revision());
        assert_eq!(
            snapshot.candidate_text("nihk", 1).unwrap().as_deref(),
            Some("你好")
        );
    }

    #[test]
    fn deterministic_builder_reproduces_the_fixed_public_manifest() {
        let built =
            CandidatePackageManifest::from_payload("tsf-public-demo-v1", false, LEXICON).unwrap();
        assert_eq!(built.render(), MANIFEST);
        assert_eq!(
            CandidatePackageManifest::parse(&built.render()).unwrap(),
            built
        );
        assert_eq!(
            built
                .load_snapshot(LEXICON)
                .unwrap()
                .candidate_text("nihk", 1)
                .unwrap()
                .as_deref(),
            Some("你好")
        );
    }

    #[test]
    fn parser_rejects_unknown_reordered_noncanonical_and_extra_fields() {
        assert_eq!(
            CandidatePackageManifest::parse(
                &MANIFEST.replace("schema=ziranma-candidate-package-v1", "schema=future")
            )
            .unwrap_err(),
            CandidatePackageError::UnsupportedSchema
        );
        assert_eq!(
            CandidatePackageManifest::parse(&MANIFEST.replace(
                "revision=tsf-public-demo-v1\ncontains_private_text=false",
                "contains_private_text=false\nrevision=tsf-public-demo-v1"
            ))
            .unwrap_err(),
            CandidatePackageError::InvalidField
        );
        assert_eq!(
            CandidatePackageManifest::parse(&MANIFEST.replace("entry_count=50", "entry_count=050"))
                .unwrap_err(),
            CandidatePackageError::InvalidEntryCount
        );
        assert_eq!(
            CandidatePackageManifest::parse(&format!("{MANIFEST}extra=x\n")).unwrap_err(),
            CandidatePackageError::InvalidStructure
        );
    }

    #[test]
    fn payload_drift_is_rejected_without_echoing_candidate_text() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        let changed = LEXICON.replace("你好", "您好");
        let error = manifest.load_snapshot(&changed).unwrap_err();
        assert!(matches!(
            error,
            CandidatePackageError::Snapshot(
                CandidateSnapshotError::PayloadFingerprintMismatch { .. }
                    | CandidateSnapshotError::PayloadLengthMismatch { .. }
            )
        ));
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("你好"));
        assert!(!rendered.contains("您好"));
    }

    #[test]
    fn lightweight_metadata_validation_does_not_require_a_decoder() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        assert_eq!(manifest.validate_payload_metadata(LEXICON, 50), Ok(()));
        assert!(matches!(
            manifest.validate_payload_metadata(LEXICON, 49),
            Err(CandidatePackageError::Snapshot(
                CandidateSnapshotError::EntryCountMismatch {
                    expected: 50,
                    actual: 49
                }
            ))
        ));
        assert!(matches!(
            manifest.validate_payload_metadata(&LEXICON.replace("你好", "您好"), 50),
            Err(CandidatePackageError::Snapshot(
                CandidateSnapshotError::PayloadFingerprintMismatch { .. }
                    | CandidateSnapshotError::PayloadLengthMismatch { .. }
            ))
        ));
    }
}
