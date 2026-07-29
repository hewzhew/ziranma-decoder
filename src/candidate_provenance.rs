//! Strict public-source and compatibility metadata for candidate packages.
//!
//! Provenance is an explicit operator declaration, not a digital signature.
//! SHA-256 binds exact bytes and detects drift; it cannot prove that a claimed
//! URL or license is truthful without a separately trusted release process.

use std::error::Error;
use std::fmt::{self, Write as _};

use sha2::{Digest, Sha256};

use crate::CANDIDATE_PACKAGE_SCHEMA_V1;

/// First strict candidate-provenance sidecar schema.
pub const CANDIDATE_PROVENANCE_SCHEMA_V1: &str = "ziranma-candidate-provenance-v1";
/// Decoder/data compatibility boundary accepted by the current TSF alpha.
pub const CANDIDATE_DECODER_COMPATIBILITY_V1: &str = "ziranma-candidate-decoder-v1";
/// Fixed provenance filename within one immutable candidate package.
pub const CANDIDATE_PACKAGE_PROVENANCE_FILE: &str = "provenance.zcp";
/// Maximum accepted size of one provenance sidecar.
pub const MAX_CANDIDATE_PROVENANCE_BYTES: usize = 2 * 1024;

const MAX_SOURCE_ID_BYTES: usize = 128;
const MAX_SOURCE_LICENSE_BYTES: usize = 64;
const MAX_SOURCE_URL_BYTES: usize = 512;
const SHA256_HEX_BYTES: usize = 64;

/// Strict public-source declaration bound to one manifest and payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidatePackageProvenance {
    source_id: String,
    source_license: String,
    source_url: String,
    source_sha256: String,
    manifest_sha256: String,
    payload_sha256: String,
}

impl CandidatePackageProvenance {
    /// Constructs provenance for exact package material.
    ///
    /// `source_sha256` must be an independently supplied lowercase SHA-256
    /// value. The caller remains responsible for checking it against the
    /// source bytes before constructing a package.
    pub fn from_materials(
        source_id: &str,
        source_license: &str,
        source_url: &str,
        source_sha256: &str,
        manifest_text: &str,
        payload_text: &str,
    ) -> Result<Self, CandidateProvenanceError> {
        validate_source_id(source_id)?;
        validate_source_license(source_license)?;
        validate_source_url(source_url)?;
        validate_sha256(source_sha256).map_err(|_| CandidateProvenanceError::InvalidSourceHash)?;
        Ok(Self {
            source_id: source_id.to_owned(),
            source_license: source_license.to_owned(),
            source_url: source_url.to_owned(),
            source_sha256: source_sha256.to_owned(),
            manifest_sha256: candidate_sha256_hex(manifest_text.as_bytes()),
            payload_sha256: candidate_sha256_hex(payload_text.as_bytes()),
        })
    }

    /// Parses the exact nine-line v1 sidecar.
    pub fn parse(contents: &str) -> Result<Self, CandidateProvenanceError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_PROVENANCE_BYTES {
            return Err(CandidateProvenanceError::InvalidSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateProvenanceError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 10 || !lines[9].is_empty() {
            return Err(CandidateProvenanceError::InvalidStructure);
        }
        if field(lines[0], "schema")? != CANDIDATE_PROVENANCE_SCHEMA_V1 {
            return Err(CandidateProvenanceError::UnsupportedSchema);
        }
        if field(lines[1], "package_schema")? != CANDIDATE_PACKAGE_SCHEMA_V1 {
            return Err(CandidateProvenanceError::UnsupportedPackageSchema);
        }
        if field(lines[2], "decoder_compatibility")? != CANDIDATE_DECODER_COMPATIBILITY_V1 {
            return Err(CandidateProvenanceError::IncompatibleDecoder);
        }
        let source_id = field(lines[3], "source_id")?;
        validate_source_id(source_id)?;
        let source_license = field(lines[4], "source_license")?;
        validate_source_license(source_license)?;
        let source_url = field(lines[5], "source_url")?;
        validate_source_url(source_url)?;
        let source_sha256 = field(lines[6], "source_sha256")?;
        validate_sha256(source_sha256).map_err(|_| CandidateProvenanceError::InvalidSourceHash)?;
        let manifest_sha256 = field(lines[7], "manifest_sha256")?;
        validate_sha256(manifest_sha256)
            .map_err(|_| CandidateProvenanceError::InvalidManifestHash)?;
        let payload_sha256 = field(lines[8], "payload_sha256")?;
        validate_sha256(payload_sha256)
            .map_err(|_| CandidateProvenanceError::InvalidPayloadHash)?;
        Ok(Self {
            source_id: source_id.to_owned(),
            source_license: source_license.to_owned(),
            source_url: source_url.to_owned(),
            source_sha256: source_sha256.to_owned(),
            manifest_sha256: manifest_sha256.to_owned(),
            payload_sha256: payload_sha256.to_owned(),
        })
    }

    /// Renders the canonical nine-line v1 sidecar.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_PROVENANCE_SCHEMA_V1}\n\
             package_schema={CANDIDATE_PACKAGE_SCHEMA_V1}\n\
             decoder_compatibility={CANDIDATE_DECODER_COMPATIBILITY_V1}\n\
             source_id={}\n\
             source_license={}\n\
             source_url={}\n\
             source_sha256={}\n\
             manifest_sha256={}\n\
             payload_sha256={}\n",
            self.source_id,
            self.source_license,
            self.source_url,
            self.source_sha256,
            self.manifest_sha256,
            self.payload_sha256
        )
    }

    /// Verifies that the sidecar binds the exact manifest and payload bytes.
    pub fn validate_materials(
        &self,
        manifest_text: &str,
        payload_text: &str,
    ) -> Result<(), CandidateProvenanceError> {
        if self.manifest_sha256 != candidate_sha256_hex(manifest_text.as_bytes()) {
            return Err(CandidateProvenanceError::ManifestHashMismatch);
        }
        if self.payload_sha256 != candidate_sha256_hex(payload_text.as_bytes()) {
            return Err(CandidateProvenanceError::PayloadHashMismatch);
        }
        Ok(())
    }

    /// Returns the bounded public source identifier.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Returns the declared single SPDX-style license identifier.
    pub fn source_license(&self) -> &str {
        &self.source_license
    }

    /// Returns the declared HTTPS source URL.
    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    /// Returns the independently supplied source checksum.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }
}

/// Computes a lowercase SHA-256 digest for exact bytes.
pub fn candidate_sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(SHA256_HEX_BYTES);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

/// Computes the authenticated package-material digest used by TSF preflight.
pub fn candidate_package_authentication_sha256(
    provenance_text: &str,
    manifest_text: &str,
    payload_text: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ziranma-candidate-auth-v1\0");
    for part in [
        provenance_text.as_bytes(),
        manifest_text.as_bytes(),
        payload_text.as_bytes(),
    ] {
        hasher.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(SHA256_HEX_BYTES);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

/// Errors returned while parsing or validating provenance metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateProvenanceError {
    /// The sidecar is empty or exceeds its fixed byte bound.
    InvalidSize,
    /// The sidecar does not use the exact nine-line LF-terminated shape.
    InvalidStructure,
    /// A required key is missing, duplicated, reordered, or empty.
    InvalidField,
    /// The provenance schema is unsupported.
    UnsupportedSchema,
    /// The referenced package schema is unsupported.
    UnsupportedPackageSchema,
    /// The decoder compatibility boundary does not match this build.
    IncompatibleDecoder,
    /// The public source identifier is outside its bounded ASCII grammar.
    InvalidSourceId,
    /// The license is not one bounded SPDX identifier.
    InvalidSourceLicense,
    /// The source URL is not one bounded HTTPS URL without whitespace.
    InvalidSourceUrl,
    /// The declared source checksum is not canonical lowercase SHA-256.
    InvalidSourceHash,
    /// The manifest checksum is not canonical lowercase SHA-256.
    InvalidManifestHash,
    /// The payload checksum is not canonical lowercase SHA-256.
    InvalidPayloadHash,
    /// The exact manifest bytes differ from the provenance declaration.
    ManifestHashMismatch,
    /// The exact payload bytes differ from the provenance declaration.
    PayloadHashMismatch,
}

impl fmt::Display for CandidateProvenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSize => "候选包来源声明大小无效",
            Self::InvalidStructure => "候选包来源声明结构无效",
            Self::InvalidField => "候选包来源声明字段无效",
            Self::UnsupportedSchema => "不支持的候选包来源声明格式",
            Self::UnsupportedPackageSchema => "来源声明引用了不支持的候选包格式",
            Self::IncompatibleDecoder => "候选包与当前解码器不兼容",
            Self::InvalidSourceId => "候选包来源标识无效",
            Self::InvalidSourceLicense => "候选包来源许可证标识无效",
            Self::InvalidSourceUrl => "候选包来源网址无效",
            Self::InvalidSourceHash => "候选包源文件 SHA-256 无效",
            Self::InvalidManifestHash => "候选包清单 SHA-256 无效",
            Self::InvalidPayloadHash => "候选包载荷 SHA-256 无效",
            Self::ManifestHashMismatch => "候选包清单与来源声明不符",
            Self::PayloadHashMismatch => "候选包载荷与来源声明不符",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateProvenanceError {}

fn field<'a>(line: &'a str, expected_key: &str) -> Result<&'a str, CandidateProvenanceError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateProvenanceError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateProvenanceError::InvalidField);
    }
    Ok(value)
}

fn validate_source_id(value: &str) -> Result<(), CandidateProvenanceError> {
    if valid_bounded_identifier(value, MAX_SOURCE_ID_BYTES) {
        Ok(())
    } else {
        Err(CandidateProvenanceError::InvalidSourceId)
    }
}

fn validate_source_license(value: &str) -> Result<(), CandidateProvenanceError> {
    if valid_bounded_identifier(value, MAX_SOURCE_LICENSE_BYTES) {
        Ok(())
    } else {
        Err(CandidateProvenanceError::InvalidSourceLicense)
    }
}

fn valid_bounded_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn validate_source_url(value: &str) -> Result<(), CandidateProvenanceError> {
    let suffix = value
        .strip_prefix("https://")
        .ok_or(CandidateProvenanceError::InvalidSourceUrl)?;
    if value.len() > MAX_SOURCE_URL_BYTES
        || suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'=' | b'\\' | b'\'' | b'"'))
    {
        return Err(CandidateProvenanceError::InvalidSourceUrl);
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ()> {
    if value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = include_str!("../tests/fixtures/public/demo_candidate_manifest.zcm");
    const PAYLOAD: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
    const PROVENANCE: &str = include_str!("../tests/fixtures/public/demo_candidate_provenance.zcp");
    const SOURCE_ID: &str = "ziranma-demo-v1";
    const SOURCE_LICENSE: &str = "MPL-2.0";
    const SOURCE_URL: &str = "https://github.com/hewzhew/ziranma-decoder";

    fn provenance() -> CandidatePackageProvenance {
        CandidatePackageProvenance::from_materials(
            SOURCE_ID,
            SOURCE_LICENSE,
            SOURCE_URL,
            &candidate_sha256_hex(PAYLOAD.as_bytes()),
            MANIFEST,
            PAYLOAD,
        )
        .unwrap()
    }

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            candidate_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            candidate_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn provenance_round_trips_and_binds_exact_materials() {
        let provenance = provenance();
        let rendered = provenance.render();
        assert_eq!(rendered, PROVENANCE);
        let parsed = CandidatePackageProvenance::parse(&rendered).unwrap();
        assert_eq!(parsed, provenance);
        parsed.validate_materials(MANIFEST, PAYLOAD).unwrap();
        assert_eq!(parsed.source_id(), SOURCE_ID);
        assert_eq!(parsed.source_license(), SOURCE_LICENSE);
        assert_eq!(parsed.source_url(), SOURCE_URL);
        assert_eq!(
            parsed.source_sha256(),
            candidate_sha256_hex(PAYLOAD.as_bytes())
        );
    }

    #[test]
    fn parser_rejects_schema_compatibility_url_license_and_hash_drift() {
        let rendered = provenance().render();
        assert_eq!(
            CandidatePackageProvenance::parse(
                &rendered.replace(CANDIDATE_PROVENANCE_SCHEMA_V1, "future-provenance")
            )
            .unwrap_err(),
            CandidateProvenanceError::UnsupportedSchema
        );
        assert_eq!(
            CandidatePackageProvenance::parse(
                &rendered.replace(CANDIDATE_DECODER_COMPATIBILITY_V1, "future-decoder")
            )
            .unwrap_err(),
            CandidateProvenanceError::IncompatibleDecoder
        );
        assert_eq!(
            CandidatePackageProvenance::parse(&rendered.replace(SOURCE_URL, "http://example.com"))
                .unwrap_err(),
            CandidateProvenanceError::InvalidSourceUrl
        );
        assert_eq!(
            CandidatePackageProvenance::parse(&rendered.replace(SOURCE_LICENSE, "MPL 2.0"))
                .unwrap_err(),
            CandidateProvenanceError::InvalidSourceLicense
        );
        assert_eq!(
            CandidatePackageProvenance::parse(
                &rendered.replace("source_sha256=", "source_sha256=A")
            )
            .unwrap_err(),
            CandidateProvenanceError::InvalidSourceHash
        );
    }

    #[test]
    fn exact_manifest_payload_and_provenance_bytes_affect_authentication_digest() {
        let provenance = provenance().render();
        let baseline = candidate_package_authentication_sha256(&provenance, MANIFEST, PAYLOAD);
        assert_ne!(
            baseline,
            candidate_package_authentication_sha256(
                &(provenance.clone() + "\n"),
                MANIFEST,
                PAYLOAD
            )
        );
        assert_ne!(
            baseline,
            candidate_package_authentication_sha256(
                &provenance,
                &(MANIFEST.to_owned() + "\n"),
                PAYLOAD
            )
        );
        assert_ne!(
            baseline,
            candidate_package_authentication_sha256(
                &provenance,
                MANIFEST,
                &(PAYLOAD.to_owned() + "\n")
            )
        );
    }

    #[test]
    fn material_drift_is_rejected_without_echoing_text() {
        let provenance = provenance();
        let manifest_error = provenance
            .validate_materials(&(MANIFEST.to_owned() + "\n"), PAYLOAD)
            .unwrap_err();
        let payload_error = provenance
            .validate_materials(MANIFEST, &(PAYLOAD.to_owned() + "私人文字\n"))
            .unwrap_err();
        assert_eq!(
            manifest_error,
            CandidateProvenanceError::ManifestHashMismatch
        );
        assert_eq!(payload_error, CandidateProvenanceError::PayloadHashMismatch);
        assert!(!format!("{payload_error}").contains("私人文字"));
    }
}
