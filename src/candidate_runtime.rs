//! Read-only loading of one explicitly configured candidate-data slot root.
//!
//! The loader follows only the fixed `current` reference from `slots.zcs`.
//! It never scans directories, writes state, learns from input, or connects to
//! a network. An absent root is distinct from a present but invalid root so a
//! host cannot silently fall back after configuration damage.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::{
    CandidatePackageError, CandidatePackageManifest, CandidateSlotError, CandidateSlotState,
    CandidateSnapshot, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES, MAX_CANDIDATE_SLOT_STATE_BYTES,
    MAX_CANDIDATE_SNAPSHOT_BYTES, candidate_payload_fingerprint,
};

/// Fixed directory beside the TSF DLL that opts into managed candidate data.
pub const CANDIDATE_RUNTIME_DIRECTORY: &str = "candidate-data";
/// Manifest filename within one immutable candidate package.
pub const CANDIDATE_PACKAGE_MANIFEST_FILE: &str = "manifest.zcm";
/// Lexicon filename within one immutable candidate package.
pub const CANDIDATE_PACKAGE_PAYLOAD_FILE: &str = "lexicon.tsv";
/// Immutable package-store directory within a candidate-data root.
pub const CANDIDATE_PACKAGES_DIRECTORY: &str = "packages";
/// Preflight-receipt directory within a candidate-data root.
pub const CANDIDATE_PREFLIGHTS_DIRECTORY: &str = "preflights";
/// Slot-state filename within a candidate-data root.
pub const CANDIDATE_SLOT_STATE_FILE: &str = "slots.zcs";
/// Schema for a successful local TSF preflight receipt.
pub const CANDIDATE_PREFLIGHT_RECEIPT_SCHEMA_V1: &str = "ziranma-candidate-preflight-v1";
/// Host exercised by the first preflight receipt schema.
pub const CANDIDATE_PREFLIGHT_HOST_V1: &str = "tsf-synthetic-context-v1";
/// Maximum accepted size of one preflight receipt.
pub const MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES: usize = 256;

/// Computes the internal immutable-package identifier from exact file bytes.
pub fn candidate_package_storage_id(manifest_text: &str, payload_text: &str) -> String {
    let manifest_id = candidate_payload_fingerprint(manifest_text.as_bytes());
    let payload_id = candidate_payload_fingerprint(payload_text.as_bytes());
    format!("pkg-{manifest_id:016x}-{payload_id:016x}")
}

/// Renders the exact receipt expected after a package passes local TSF preflight.
pub fn candidate_preflight_receipt_body(package_id: &str) -> String {
    format!(
        "schema={CANDIDATE_PREFLIGHT_RECEIPT_SCHEMA_V1}\n\
         package={package_id}\n\
         host={CANDIDATE_PREFLIGHT_HOST_V1}\n"
    )
}

/// Loads the validated snapshot named by the current slot.
///
/// `Ok(None)` means the explicitly supplied root does not exist. Once the root
/// exists, all required state, package, and preflight evidence must validate.
pub fn load_current_candidate_snapshot(
    root: &Path,
) -> Result<Option<Arc<CandidateSnapshot>>, CandidateRuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CandidateRuntimeError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CandidateRuntimeError::InvalidRoot);
        }
        Ok(_) => {}
    }

    let slot_text = read_regular_utf8(
        &root.join(CANDIDATE_SLOT_STATE_FILE),
        MAX_CANDIDATE_SLOT_STATE_BYTES,
        CandidateRuntimeError::SlotStateUnavailable,
    )?;
    let state = CandidateSlotState::parse(&slot_text).map_err(CandidateRuntimeError::SlotState)?;
    let package_id = state
        .current()
        .ok_or(CandidateRuntimeError::CurrentNotConfigured)?;

    let packages = root.join(CANDIDATE_PACKAGES_DIRECTORY);
    ensure_regular_directory(&packages, CandidateRuntimeError::InvalidPackageStore)?;
    let package = packages.join(package_id);
    ensure_regular_directory(&package, CandidateRuntimeError::InvalidPackageDirectory)?;

    let manifest_text = read_regular_utf8(
        &package.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
        CandidateRuntimeError::ManifestUnavailable,
    )?;
    let manifest =
        CandidatePackageManifest::parse(&manifest_text).map_err(CandidateRuntimeError::Package)?;
    if manifest.contains_private_text() {
        return Err(CandidateRuntimeError::PrivatePlaintext);
    }
    let payload_text = read_regular_utf8(
        &package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        MAX_CANDIDATE_SNAPSHOT_BYTES,
        CandidateRuntimeError::PayloadUnavailable,
    )?;
    let snapshot = manifest
        .load_snapshot(&payload_text)
        .map_err(CandidateRuntimeError::Package)?;
    if candidate_package_storage_id(&manifest_text, &payload_text) != package_id {
        return Err(CandidateRuntimeError::StorageIdentifierMismatch);
    }

    let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
    ensure_regular_directory(&preflights, CandidateRuntimeError::InvalidPreflightStore)?;
    let receipt = read_regular_utf8(
        &preflights.join(format!("{package_id}.zpf")),
        MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
        CandidateRuntimeError::PreflightReceiptUnavailable,
    )?;
    if receipt != candidate_preflight_receipt_body(package_id) {
        return Err(CandidateRuntimeError::PreflightReceiptMismatch);
    }

    Ok(Some(Arc::new(snapshot)))
}

fn ensure_regular_directory(
    path: &Path,
    error: CandidateRuntimeError,
) -> Result<(), CandidateRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| error.clone())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error);
    }
    Ok(())
}

fn read_regular_utf8(
    path: &Path,
    maximum_bytes: usize,
    error: CandidateRuntimeError,
) -> Result<String, CandidateRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| error.clone())?;
    let maximum_u64 = u64::try_from(maximum_bytes).unwrap_or(u64::MAX);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum_u64
    {
        return Err(error);
    }

    let mut file = File::open(path).map_err(|_| error.clone())?;
    let opened = file.metadata().map_err(|_| error.clone())?;
    if !opened.is_file() || opened.len() == 0 || opened.len() > maximum_u64 {
        return Err(error);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| error.clone())?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(error);
    }
    String::from_utf8(bytes).map_err(|_| error)
}

/// Sanitized failures from reading configured candidate runtime data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRuntimeError {
    /// The root metadata could not be inspected.
    RootUnavailable,
    /// The existing root is not a regular directory.
    InvalidRoot,
    /// The exact slot-state file is missing, unsafe, or unreadable.
    SlotStateUnavailable,
    /// The slot state did not satisfy its strict schema.
    SlotState(CandidateSlotError),
    /// A configured root did not name a current package.
    CurrentNotConfigured,
    /// The fixed package store is missing or unsafe.
    InvalidPackageStore,
    /// The referenced immutable package directory is missing or unsafe.
    InvalidPackageDirectory,
    /// The exact manifest is missing, unsafe, or unreadable.
    ManifestUnavailable,
    /// The exact lexicon payload is missing, unsafe, or unreadable.
    PayloadUnavailable,
    /// The package metadata or payload did not validate.
    Package(CandidatePackageError),
    /// Plaintext private candidate data is outside the TSF alpha boundary.
    PrivatePlaintext,
    /// The package bytes no longer match their immutable storage identifier.
    StorageIdentifierMismatch,
    /// The fixed preflight store is missing or unsafe.
    InvalidPreflightStore,
    /// The exact package preflight receipt is missing, unsafe, or unreadable.
    PreflightReceiptUnavailable,
    /// The preflight receipt does not bind the current package and host.
    PreflightReceiptMismatch,
}

impl fmt::Display for CandidateRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::RootUnavailable => "无法检查候选数据目录",
            Self::InvalidRoot => "候选数据目录无效",
            Self::SlotStateUnavailable => "候选数据槽状态不可用",
            Self::SlotState(_) => "候选数据槽状态无效",
            Self::CurrentNotConfigured => "当前候选数据尚未配置",
            Self::InvalidPackageStore => "候选包存储无效",
            Self::InvalidPackageDirectory => "当前候选包目录无效",
            Self::ManifestUnavailable => "当前候选包清单不可用",
            Self::PayloadUnavailable => "当前候选包载荷不可用",
            Self::Package(_) => "当前候选包校验失败",
            Self::PrivatePlaintext => "TSF alpha 不接受明文私人候选包",
            Self::StorageIdentifierMismatch => "当前候选包与存储标识不符",
            Self::InvalidPreflightStore => "候选包预检存储无效",
            Self::PreflightReceiptUnavailable => "当前候选包缺少预检凭据",
            Self::PreflightReceiptMismatch => "当前候选包预检凭据不符",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SlotState(error) => Some(error),
            Self::Package(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const PAYLOAD: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ziranma-candidate-runtime-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn configured_root(revision: &str, private: bool) -> (TestDirectory, String) {
        let root = TestDirectory::new();
        let package_id = install_test_package(root.path(), revision, private, PAYLOAD);
        let mut state = CandidateSlotState::default();
        state.adopt(&package_id).unwrap();
        fs::write(root.path().join(CANDIDATE_SLOT_STATE_FILE), state.render()).unwrap();
        (root, package_id)
    }

    fn install_test_package(root: &Path, revision: &str, private: bool, payload: &str) -> String {
        let manifest = CandidatePackageManifest::from_payload(revision, private, payload).unwrap();
        let manifest_text = manifest.render();
        let package_id = candidate_package_storage_id(&manifest_text, payload);
        let package = root.join(CANDIDATE_PACKAGES_DIRECTORY).join(&package_id);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_MANIFEST_FILE), manifest_text).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), payload).unwrap();
        let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
        fs::create_dir_all(&preflights).unwrap();
        fs::write(
            preflights.join(format!("{package_id}.zpf")),
            candidate_preflight_receipt_body(&package_id),
        )
        .unwrap();
        package_id
    }

    #[test]
    fn absent_root_is_the_only_fallback_state() {
        let root = TestDirectory::new();
        let missing = root.path().join("absent");
        assert!(load_current_candidate_snapshot(&missing).unwrap().is_none());
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::SlotStateUnavailable
        );
    }

    #[test]
    fn loads_exact_current_package_and_preflight() {
        let (root, _) = configured_root("runtime-a", false);
        let snapshot = load_current_candidate_snapshot(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.revision(), "runtime-a");
        assert_eq!(
            snapshot.candidate_text("nihk", 1).unwrap().as_deref(),
            Some("你好")
        );
    }

    #[test]
    fn missing_or_changed_preflight_is_rejected() {
        let (root, package_id) = configured_root("runtime-b", false);
        let receipt = root
            .path()
            .join(CANDIDATE_PREFLIGHTS_DIRECTORY)
            .join(format!("{package_id}.zpf"));
        fs::write(&receipt, "schema=wrong\n").unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::PreflightReceiptMismatch
        );
        fs::remove_file(receipt).unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::PreflightReceiptUnavailable
        );
    }

    #[test]
    fn changed_installed_payload_is_rejected() {
        let (root, package_id) = configured_root("runtime-c", false);
        fs::write(
            root.path()
                .join(CANDIDATE_PACKAGES_DIRECTORY)
                .join(package_id)
                .join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
            "a\t1\t啊\n",
        )
        .unwrap();
        assert!(matches!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::Package(_)
        ));
    }

    #[test]
    fn plaintext_private_package_is_rejected() {
        let (root, package_id) = configured_root("runtime-private", true);
        fs::remove_file(
            root.path()
                .join(CANDIDATE_PACKAGES_DIRECTORY)
                .join(package_id)
                .join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        )
        .unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::PrivatePlaintext
        );
    }

    #[test]
    fn a_loaded_snapshot_stays_immutable_across_slot_promotion() {
        const FIRST: &str = "text\tpinyin\tfrequency\n你好\tni hao\t100\n";
        const SECOND: &str = "text\tpinyin\tfrequency\n您好\tni hao\t100\n";

        let root = TestDirectory::new();
        let first_id = install_test_package(root.path(), "runtime-first", false, FIRST);
        let second_id = install_test_package(root.path(), "runtime-second", false, SECOND);
        let mut state = CandidateSlotState::default();
        state.adopt(&first_id).unwrap();
        fs::write(root.path().join(CANDIDATE_SLOT_STATE_FILE), state.render()).unwrap();
        let first = load_current_candidate_snapshot(root.path())
            .unwrap()
            .unwrap();

        state.stage(&second_id).unwrap();
        state.promote().unwrap();
        fs::write(root.path().join(CANDIDATE_SLOT_STATE_FILE), state.render()).unwrap();
        let second = load_current_candidate_snapshot(root.path())
            .unwrap()
            .unwrap();

        assert_eq!(
            first.candidate_text("nihk", 1).unwrap().as_deref(),
            Some("你好")
        );
        assert_eq!(
            second.candidate_text("nihk", 1).unwrap().as_deref(),
            Some("您好")
        );
        assert_eq!(first.revision(), "runtime-first");
        assert_eq!(second.revision(), "runtime-second");
    }
}
