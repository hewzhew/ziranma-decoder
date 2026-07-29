//! Strict detached Ed25519 verification for public candidate releases.
//!
//! This module never generates, loads, or stores private keys. The caller must
//! obtain the trusted public key through a channel independent of the package
//! and detached signature being verified.

use std::error::Error;
use std::fmt;

use ed25519_dalek::{Signature, VerifyingKey};

use crate::candidate_sha256_hex;

/// First detached candidate-release signature schema.
pub const CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1: &str = "ziranma-candidate-release-signature-v1";
/// Only signature algorithm accepted by the first schema.
pub const CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519: &str = "ed25519";
/// Maximum accepted size of one detached signature statement.
pub const MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES: usize = 512;

const SHA256_BYTES: usize = 32;
const ED25519_PUBLIC_KEY_BYTES: usize = 32;
const ED25519_SIGNATURE_BYTES: usize = 64;
const SIGNING_DOMAIN: &[u8] = b"ziranma-candidate-release-signature-v1\0";

/// Canonical detached signature over one exact candidate-package digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateReleaseSignature {
    key_sha256: String,
    package_sha256: String,
    signature: String,
}

impl CandidateReleaseSignature {
    /// Parses the exact five-line v1 detached signature statement.
    pub fn parse(contents: &str) -> Result<Self, CandidateReleaseSignatureError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES {
            return Err(CandidateReleaseSignatureError::InvalidSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateReleaseSignatureError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 6 || !lines[5].is_empty() {
            return Err(CandidateReleaseSignatureError::InvalidStructure);
        }
        if field(lines[0], "schema")? != CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1 {
            return Err(CandidateReleaseSignatureError::UnsupportedSchema);
        }
        if field(lines[1], "algorithm")? != CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519 {
            return Err(CandidateReleaseSignatureError::UnsupportedAlgorithm);
        }
        let key_sha256 = field(lines[2], "key_sha256")?;
        decode_hex::<SHA256_BYTES>(key_sha256)
            .map_err(|_| CandidateReleaseSignatureError::InvalidKeyHash)?;
        let package_sha256 = field(lines[3], "package_sha256")?;
        decode_hex::<SHA256_BYTES>(package_sha256)
            .map_err(|_| CandidateReleaseSignatureError::InvalidPackageHash)?;
        let signature = field(lines[4], "signature")?;
        decode_hex::<ED25519_SIGNATURE_BYTES>(signature)
            .map_err(|_| CandidateReleaseSignatureError::InvalidSignatureEncoding)?;
        Ok(Self {
            key_sha256: key_sha256.to_owned(),
            package_sha256: package_sha256.to_owned(),
            signature: signature.to_owned(),
        })
    }

    /// Renders the canonical five-line v1 statement.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1}\n\
             algorithm={CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519}\n\
             key_sha256={}\n\
             package_sha256={}\n\
             signature={}\n",
            self.key_sha256, self.package_sha256, self.signature
        )
    }

    /// Verifies the statement against one independently trusted public key and package digest.
    pub fn verify(
        &self,
        trusted_public_key_hex: &str,
        actual_package_sha256: &str,
    ) -> Result<(), CandidateReleaseSignatureError> {
        let public_key_bytes = decode_hex::<ED25519_PUBLIC_KEY_BYTES>(trusted_public_key_hex)
            .map_err(|_| CandidateReleaseSignatureError::InvalidTrustedPublicKey)?;
        let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
            .map_err(|_| CandidateReleaseSignatureError::InvalidTrustedPublicKey)?;
        if self.key_sha256 != candidate_sha256_hex(&public_key_bytes) {
            return Err(CandidateReleaseSignatureError::TrustedKeyMismatch);
        }

        decode_hex::<SHA256_BYTES>(actual_package_sha256)
            .map_err(|_| CandidateReleaseSignatureError::InvalidPackageHash)?;
        if self.package_sha256 != actual_package_sha256 {
            return Err(CandidateReleaseSignatureError::PackageHashMismatch);
        }

        let signature_bytes = decode_hex::<ED25519_SIGNATURE_BYTES>(&self.signature)
            .map_err(|_| CandidateReleaseSignatureError::InvalidSignatureEncoding)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let message = candidate_release_signing_message(&self.key_sha256, &self.package_sha256)?;
        verifying_key
            .verify_strict(&message, &signature)
            .map_err(|_| CandidateReleaseSignatureError::SignatureVerificationFailed)
    }

    /// Returns the declared SHA-256 fingerprint of the trusted public key.
    pub fn key_sha256(&self) -> &str {
        &self.key_sha256
    }

    /// Returns the exact candidate-package material digest that was signed.
    pub fn package_sha256(&self) -> &str {
        &self.package_sha256
    }
}

/// Constructs the fixed domain-separated message signed by release tooling.
pub fn candidate_release_signing_message(
    key_sha256: &str,
    package_sha256: &str,
) -> Result<Vec<u8>, CandidateReleaseSignatureError> {
    let key_hash = decode_hex::<SHA256_BYTES>(key_sha256)
        .map_err(|_| CandidateReleaseSignatureError::InvalidKeyHash)?;
    let package_hash = decode_hex::<SHA256_BYTES>(package_sha256)
        .map_err(|_| CandidateReleaseSignatureError::InvalidPackageHash)?;
    let mut message =
        Vec::with_capacity(SIGNING_DOMAIN.len() + key_hash.len() + package_hash.len());
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(&key_hash);
    message.extend_from_slice(&package_hash);
    Ok(message)
}

/// Errors returned while parsing or verifying a detached release signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateReleaseSignatureError {
    /// The statement is empty or exceeds its fixed byte bound.
    InvalidSize,
    /// The statement does not use the exact five-line LF-terminated shape.
    InvalidStructure,
    /// A required field is missing, duplicated, reordered, or empty.
    InvalidField,
    /// The signature schema is unsupported.
    UnsupportedSchema,
    /// The signature algorithm is unsupported.
    UnsupportedAlgorithm,
    /// The declared public-key fingerprint is not canonical SHA-256.
    InvalidKeyHash,
    /// The declared or supplied package digest is not canonical SHA-256.
    InvalidPackageHash,
    /// The detached signature is not canonical Ed25519 hex.
    InvalidSignatureEncoding,
    /// The independently supplied public key is malformed.
    InvalidTrustedPublicKey,
    /// The independently supplied public key does not match the declared fingerprint.
    TrustedKeyMismatch,
    /// The detached statement names a different candidate package.
    PackageHashMismatch,
    /// Ed25519 strict verification failed.
    SignatureVerificationFailed,
}

impl fmt::Display for CandidateReleaseSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidSize => "候选包签名声明大小无效",
            Self::InvalidStructure => "候选包签名声明结构无效",
            Self::InvalidField => "候选包签名声明字段无效",
            Self::UnsupportedSchema => "不支持的候选包签名声明格式",
            Self::UnsupportedAlgorithm => "不支持的候选包签名算法",
            Self::InvalidKeyHash => "候选包签名密钥指纹无效",
            Self::InvalidPackageHash => "候选包签名材料摘要无效",
            Self::InvalidSignatureEncoding => "候选包签名编码无效",
            Self::InvalidTrustedPublicKey => "可信候选包公钥无效",
            Self::TrustedKeyMismatch => "候选包签名与可信公钥不符",
            Self::PackageHashMismatch => "候选包签名指向其他包",
            Self::SignatureVerificationFailed => "候选包签名验证失败",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateReleaseSignatureError {}

fn field<'a>(line: &'a str, expected_key: &str) -> Result<&'a str, CandidateReleaseSignatureError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateReleaseSignatureError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateReleaseSignatureError::InvalidField);
    }
    Ok(value)
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    if value.len() != N * 2 {
        return Err(());
    }
    let mut decoded = [0_u8; N];
    for (index, output) in decoded.iter_mut().enumerate() {
        let offset = index * 2;
        let high = hex_nibble(value.as_bytes()[offset])?;
        let low = hex_nibble(value.as_bytes()[offset + 1])?;
        *output = (high << 4) | low;
    }
    Ok(decoded)
}

fn hex_nibble(value: u8) -> Result<u8, ()> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;

    const PACKAGE_SHA256: &str = "1f2f3c81280641d9963b0ea0fac1fcdaf749d76bae778034037f015f8b8434c2";

    fn encode_hex(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").unwrap();
        }
        encoded
    }

    fn signed_statement() -> (CandidateReleaseSignature, String) {
        // Public synthetic test material only; this is not a release key.
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let public_key = signing_key.verifying_key().to_bytes();
        let public_key_hex = encode_hex(&public_key);
        let key_sha256 = candidate_sha256_hex(&public_key);
        let message = candidate_release_signing_message(&key_sha256, PACKAGE_SHA256).unwrap();
        let signature = signing_key.sign(&message);
        (
            CandidateReleaseSignature {
                key_sha256,
                package_sha256: PACKAGE_SHA256.to_owned(),
                signature: encode_hex(&signature.to_bytes()),
            },
            public_key_hex,
        )
    }

    #[test]
    fn strict_statement_round_trips_and_verifies() {
        let (statement, public_key) = signed_statement();
        let rendered = statement.render();
        let parsed = CandidateReleaseSignature::parse(&rendered).unwrap();
        assert_eq!(parsed, statement);
        assert_eq!(parsed.package_sha256(), PACKAGE_SHA256);
        assert_eq!(parsed.key_sha256().len(), SHA256_BYTES * 2);
        parsed.verify(&public_key, PACKAGE_SHA256).unwrap();
    }

    #[test]
    fn parser_rejects_schema_algorithm_hash_signature_and_structure_drift() {
        let (statement, _) = signed_statement();
        let rendered = statement.render();
        assert_eq!(
            CandidateReleaseSignature::parse(
                &rendered.replace(CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1, "future-signature")
            )
            .unwrap_err(),
            CandidateReleaseSignatureError::UnsupportedSchema
        );
        assert_eq!(
            CandidateReleaseSignature::parse(
                &rendered.replace(CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519, "future")
            )
            .unwrap_err(),
            CandidateReleaseSignatureError::UnsupportedAlgorithm
        );
        assert_eq!(
            CandidateReleaseSignature::parse(&rendered.replace("key_sha256=", "key_sha256=A"))
                .unwrap_err(),
            CandidateReleaseSignatureError::InvalidKeyHash
        );
        assert_eq!(
            CandidateReleaseSignature::parse(
                &rendered.replace("package_sha256=", "package_sha256=A")
            )
            .unwrap_err(),
            CandidateReleaseSignatureError::InvalidPackageHash
        );
        assert_eq!(
            CandidateReleaseSignature::parse(&rendered.replace("signature=", "signature=A"))
                .unwrap_err(),
            CandidateReleaseSignatureError::InvalidSignatureEncoding
        );
        assert_eq!(
            CandidateReleaseSignature::parse(rendered.trim_end()).unwrap_err(),
            CandidateReleaseSignatureError::InvalidStructure
        );
    }

    #[test]
    fn wrong_key_package_and_signature_are_distinct_failures() {
        let (statement, public_key) = signed_statement();
        let wrong_key = SigningKey::from_bytes(&[8_u8; 32])
            .verifying_key()
            .to_bytes();
        assert_eq!(
            statement
                .verify(&encode_hex(&wrong_key), PACKAGE_SHA256)
                .unwrap_err(),
            CandidateReleaseSignatureError::TrustedKeyMismatch
        );
        assert_eq!(
            statement.verify(&public_key, &"0".repeat(64)).unwrap_err(),
            CandidateReleaseSignatureError::PackageHashMismatch
        );
        let mut changed_signature = statement.clone();
        changed_signature.signature = "0".repeat(ED25519_SIGNATURE_BYTES * 2);
        assert_eq!(
            changed_signature
                .verify(&public_key, PACKAGE_SHA256)
                .unwrap_err(),
            CandidateReleaseSignatureError::SignatureVerificationFailed
        );
    }

    #[test]
    fn signing_message_is_domain_separated_and_binds_both_hashes() {
        let (statement, _) = signed_statement();
        let baseline =
            candidate_release_signing_message(statement.key_sha256(), PACKAGE_SHA256).unwrap();
        assert!(baseline.starts_with(SIGNING_DOMAIN));
        assert_ne!(
            baseline,
            candidate_release_signing_message(&"0".repeat(64), PACKAGE_SHA256).unwrap()
        );
        assert_ne!(
            baseline,
            candidate_release_signing_message(statement.key_sha256(), &"0".repeat(64)).unwrap()
        );
    }
}
