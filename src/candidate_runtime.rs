//! Read-only loading of one explicitly configured candidate-data slot root.
//!
//! The loader follows only the fixed `current` reference from `slots.zcs`.
//! It never scans directories, writes state, learns from input, or connects to
//! a network. An absent root is distinct from a present but invalid root so a
//! host cannot silently fall back after configuration damage.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::{
    CANDIDATE_DECODER_COMPATIBILITY_V1, CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_FILE,
    CANDIDATE_EXACT_PHRASE_STATE_FILE, CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE,
    CANDIDATE_EXACT_SHORT_STATE_FILE, CANDIDATE_PACKAGE_PROVENANCE_FILE,
    CANDIDATE_SUPPLEMENTAL_STATE_FILE, CandidateExactPhrasePreflightReceipt,
    CandidateExactPhrasePreflightReceiptError, CandidateExactPhraseState,
    CandidateExactPhraseStateError, CandidateExactShortPreflightReceipt,
    CandidateExactShortPreflightReceiptError, CandidateExactShortState,
    CandidateExactShortStateError, CandidatePackageError, CandidatePackageManifest,
    CandidatePackageProvenance, CandidateProvenanceError, CandidateSlotError, CandidateSlotState,
    CandidateSnapshot, CandidateSupplementalState, CandidateSupplementalStateError,
    ExactShortWordCatalog, ExactShortWordCatalogError,
    MAX_CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_BYTES, MAX_CANDIDATE_EXACT_PHRASE_STATE_BYTES,
    MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES, MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES,
    MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES, MAX_CANDIDATE_PROVENANCE_BYTES,
    MAX_CANDIDATE_SLOT_STATE_BYTES, MAX_CANDIDATE_SNAPSHOT_BYTES,
    MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES, SupplementalCandidateLayerConfig,
    candidate_package_authentication_sha256, candidate_sha256_hex, parse_lexicon_tsv,
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
/// Schema for a SHA-256-bound local TSF preflight receipt.
pub const CANDIDATE_PREFLIGHT_RECEIPT_SCHEMA_V2: &str = "ziranma-candidate-preflight-v2";
/// Host exercised by the first preflight receipt schema.
pub const CANDIDATE_PREFLIGHT_HOST_V1: &str = "tsf-synthetic-context-v1";
/// Maximum accepted size of one preflight receipt.
pub const MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES: usize = 512;

/// Immutable core snapshot plus independently validated optional public
/// candidate layers.
#[derive(Clone, Debug)]
pub struct CandidateRuntimeSnapshots {
    core: Arc<CandidateSnapshot>,
    core_authentication_sha256: String,
    supplemental: Option<CandidateRuntimeSupplemental>,
    supplemental_fell_back: bool,
    exact_short: Option<CandidateRuntimeExactShort>,
    exact_short_fell_back: bool,
    exact_phrase: Option<CandidateRuntimeExactPhrase>,
    exact_phrase_fell_back: bool,
}

impl CandidateRuntimeSnapshots {
    /// Returns the required core candidate snapshot.
    pub fn core(&self) -> &Arc<CandidateSnapshot> {
        &self.core
    }

    /// Returns the authentication digest of the immutable core package.
    pub fn core_authentication_sha256(&self) -> &str {
        &self.core_authentication_sha256
    }

    /// Returns the enabled, validated supplement when one is available.
    pub fn supplemental(&self) -> Option<&CandidateRuntimeSupplemental> {
        self.supplemental.as_ref()
    }

    /// Reports that an explicit supplemental configuration failed closed.
    pub fn supplemental_fell_back(&self) -> bool {
        self.supplemental_fell_back
    }

    /// Returns the enabled, validated exact-short catalog when available.
    pub fn exact_short(&self) -> Option<&CandidateRuntimeExactShort> {
        self.exact_short.as_ref()
    }

    /// Reports that an explicit exact-short configuration failed closed.
    pub fn exact_short_fell_back(&self) -> bool {
        self.exact_short_fell_back
    }

    /// Returns the enabled, validated exact three-character phrase snapshot.
    pub fn exact_phrase(&self) -> Option<&CandidateRuntimeExactPhrase> {
        self.exact_phrase.as_ref()
    }

    /// Reports that an explicit exact-phrase configuration failed closed.
    pub fn exact_phrase_fell_back(&self) -> bool {
        self.exact_phrase_fell_back
    }
}

/// Validated supplemental snapshot and its fixed candidate influence cap.
#[derive(Clone, Debug)]
pub struct CandidateRuntimeSupplemental {
    package_id: String,
    snapshot: Arc<CandidateSnapshot>,
    authentication_sha256: String,
    payload_sha256: String,
    config: SupplementalCandidateLayerConfig,
}

impl CandidateRuntimeSupplemental {
    /// Returns the immutable package selected by the supplemental state.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Returns the immutable supplemental public snapshot.
    pub fn snapshot(&self) -> &Arc<CandidateSnapshot> {
        &self.snapshot
    }

    /// Returns the authentication digest of the immutable supplemental package.
    pub fn authentication_sha256(&self) -> &str {
        &self.authentication_sha256
    }

    /// Returns the exact supplemental payload digest bound by phrase provenance.
    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    /// Returns the exact-word merge configuration bound by local state.
    pub fn config(&self) -> SupplementalCandidateLayerConfig {
        self.config
    }
}

/// Validated exact three-character phrase snapshot.
#[derive(Clone, Debug)]
pub struct CandidateRuntimeExactPhrase {
    package_id: String,
    snapshot: Arc<CandidateSnapshot>,
    authentication_sha256: String,
}

impl CandidateRuntimeExactPhrase {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn snapshot(&self) -> &Arc<CandidateSnapshot> {
        &self.snapshot
    }

    pub fn authentication_sha256(&self) -> &str {
        &self.authentication_sha256
    }
}

/// Validated exact-short catalog and its fixed per-code influence cap.
#[derive(Clone, Debug)]
pub struct CandidateRuntimeExactShort {
    package_id: String,
    catalog: Arc<ExactShortWordCatalog>,
    authentication_sha256: String,
    exact_promotions: usize,
}

impl CandidateRuntimeExactShort {
    /// Returns the immutable package selected by the exact-short state.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Returns the strictly validated two-character catalog.
    pub fn catalog(&self) -> &Arc<ExactShortWordCatalog> {
        &self.catalog
    }

    /// Returns the authentication digest of the immutable exact-short package.
    pub fn authentication_sha256(&self) -> &str {
        &self.authentication_sha256
    }

    /// Returns the maximum number of page-guarded insertions per code.
    pub fn exact_promotions(&self) -> usize {
        self.exact_promotions
    }
}

/// Small, validated supplemental selection that can be polled without loading
/// the lexicon payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRuntimeSupplementalSelection {
    /// The supplemental root or activation state is absent or explicitly off.
    Disabled,
    /// One immutable package is enabled with a bounded exact-word influence.
    Enabled {
        /// Package identifier bound by both the activation and slot states.
        package_id: String,
        /// Exact-word merge configuration bound by the activation state.
        config: SupplementalCandidateLayerConfig,
    },
}

/// Small exact-short selection that can be polled without loading its payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRuntimeExactShortSelection {
    /// The exact-short root or activation state is absent or explicitly off.
    Disabled,
    /// One immutable package is enabled with a bounded page insertion cap.
    Enabled {
        /// Package identifier bound by both activation and slot states.
        package_id: String,
        /// Maximum number of page-guarded insertions per code.
        exact_promotions: usize,
    },
}

/// Small exact-phrase selection that can be polled without loading its payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRuntimeExactPhraseSelection {
    Disabled,
    Enabled { package_id: String },
}

struct LoadedRuntimeCandidate {
    package_id: String,
    snapshot: Arc<CandidateSnapshot>,
    authentication_sha256: String,
    payload_sha256: String,
}

struct LoadedRuntimePackage {
    package_id: String,
    manifest: CandidatePackageManifest,
    provenance: CandidatePackageProvenance,
    payload: String,
    authentication_sha256: String,
}

/// Computes the internal immutable-package identifier from all exact package bytes.
pub fn candidate_package_storage_id(
    provenance_text: &str,
    manifest_text: &str,
    payload_text: &str,
) -> String {
    let authentication_sha256 =
        candidate_package_authentication_sha256(provenance_text, manifest_text, payload_text);
    format!(
        "pkg-{}-{}",
        &authentication_sha256[..16],
        &authentication_sha256[16..32]
    )
}

/// Renders the exact receipt expected after a package passes local TSF preflight.
pub fn candidate_preflight_receipt_body(
    package_id: &str,
    package_authentication_sha256: &str,
) -> String {
    format!(
        "schema={CANDIDATE_PREFLIGHT_RECEIPT_SCHEMA_V2}\n\
         package={package_id}\n\
         package_sha256={package_authentication_sha256}\n\
         host={CANDIDATE_PREFLIGHT_HOST_V1}\n\
         decoder_compatibility={CANDIDATE_DECODER_COMPATIBILITY_V1}\n"
    )
}

/// Loads the validated snapshot named by the current slot.
///
/// `Ok(None)` means the explicitly supplied root does not exist. Once the root
/// exists, all required state, package, and preflight evidence must validate.
pub fn load_current_candidate_snapshot(
    root: &Path,
) -> Result<Option<Arc<CandidateSnapshot>>, CandidateRuntimeError> {
    Ok(load_current_candidate_package(root)?.map(|loaded| loaded.snapshot))
}

/// Loads the required core root and an optional independent supplemental root.
///
/// Core configuration remains strict. A missing supplemental root or state is
/// disabled; malformed, mismatched, or unreadable supplemental state falls
/// back to the validated core snapshot without changing it.
pub fn load_candidate_runtime_snapshots(
    core_root: &Path,
    supplemental_root: Option<&Path>,
) -> Result<Option<CandidateRuntimeSnapshots>, CandidateRuntimeError> {
    load_candidate_runtime_snapshots_with_layers(core_root, supplemental_root, None)
}

/// Loads the required core root and both independent optional public layers.
///
/// Optional roots are fail-closed and do not alter the validated core. An
/// absent activation file is a normal disabled state. A present but malformed
/// or drifting activation is reported through the corresponding fallback bit.
pub fn load_candidate_runtime_snapshots_with_layers(
    core_root: &Path,
    supplemental_root: Option<&Path>,
    exact_short_root: Option<&Path>,
) -> Result<Option<CandidateRuntimeSnapshots>, CandidateRuntimeError> {
    load_candidate_runtime_snapshots_with_all_layers(
        core_root,
        supplemental_root,
        exact_short_root,
        None,
    )
}

/// Loads the core plus all independently managed optional public layers.
///
/// The exact-phrase lane additionally requires the current supplemental
/// package, four-source payload binding, and a real-TSF combined receipt. It
/// therefore remains unavailable until every dependency passes together.
pub fn load_candidate_runtime_snapshots_with_all_layers(
    core_root: &Path,
    supplemental_root: Option<&Path>,
    exact_short_root: Option<&Path>,
    exact_phrase_root: Option<&Path>,
) -> Result<Option<CandidateRuntimeSnapshots>, CandidateRuntimeError> {
    let Some(core) = load_current_candidate_package(core_root)? else {
        return Ok(None);
    };
    let (supplemental, supplemental_fell_back) = match supplemental_root {
        Some(root) => match load_candidate_runtime_supplemental_selection(root)
            .and_then(|selection| load_candidate_runtime_supplemental(root, &selection))
        {
            Ok(supplemental) => (supplemental, false),
            Err(_) => (None, true),
        },
        None => (None, false),
    };
    let (exact_short, exact_short_fell_back) = match exact_short_root {
        Some(root) => {
            let loaded = (|| {
                let selection = load_candidate_runtime_exact_short_selection(root)?;
                let exact_short = load_candidate_runtime_exact_short(root, &selection)?;
                if exact_short.is_some() {
                    let receipt =
                        load_candidate_runtime_exact_short_preflight(root, &selection)?
                            .ok_or(CandidateRuntimeError::ExactShortCombinedPreflightUnavailable)?;
                    let supplemental_identity = supplemental.as_ref().map(|layer| {
                        (
                            layer.authentication_sha256(),
                            layer.config().exact_promotions,
                        )
                    });
                    if !receipt.matches_runtime(&core.authentication_sha256, supplemental_identity)
                    {
                        return Err(CandidateRuntimeError::ExactShortCombinedPreflightMismatch);
                    }
                }
                Ok(exact_short)
            })();
            match loaded {
                Ok(exact_short) => (exact_short, false),
                Err(_) => (None, true),
            }
        }
        None => (None, false),
    };
    let (exact_phrase, exact_phrase_fell_back) = match exact_phrase_root {
        Some(root) => {
            let loaded = (|| {
                let selection = load_candidate_runtime_exact_phrase_selection(root)?;
                if selection == CandidateRuntimeExactPhraseSelection::Disabled {
                    return Ok(None);
                }
                let supplemental = supplemental
                    .as_ref()
                    .ok_or(CandidateRuntimeError::ExactPhraseSupplementalUnavailable)?;
                let exact_phrase = load_candidate_runtime_exact_phrase(
                    root,
                    &selection,
                    &core.payload_sha256,
                    supplemental.payload_sha256(),
                )?;
                if exact_phrase.is_some() {
                    let receipt = load_candidate_runtime_exact_phrase_preflight(root, &selection)?
                        .ok_or(CandidateRuntimeError::ExactPhraseCombinedPreflightUnavailable)?;
                    if !receipt.matches_runtime(
                        &core.authentication_sha256,
                        supplemental.authentication_sha256(),
                        supplemental.config().exact_promotions,
                    ) {
                        return Err(CandidateRuntimeError::ExactPhraseCombinedPreflightMismatch);
                    }
                }
                Ok(exact_phrase)
            })();
            match loaded {
                Ok(exact_phrase) => (exact_phrase, false),
                Err(_) => (None, true),
            }
        }
        None => (None, false),
    };
    Ok(Some(CandidateRuntimeSnapshots {
        core: core.snapshot,
        core_authentication_sha256: core.authentication_sha256,
        supplemental,
        supplemental_fell_back,
        exact_short,
        exact_short_fell_back,
        exact_phrase,
        exact_phrase_fell_back,
    }))
}

/// Reads only the supplemental activation and slot pointers.
///
/// This intentionally does not open a manifest, provenance file, preflight
/// receipt, or lexicon payload. Callers can compare the returned value with
/// their last applied selection before deciding whether a full load is needed.
pub fn load_candidate_runtime_supplemental_selection(
    root: &Path,
) -> Result<CandidateRuntimeSupplementalSelection, CandidateRuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeSupplementalSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::SupplementalRootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CandidateRuntimeError::InvalidSupplementalRoot);
        }
        Ok(_) => {}
    }
    let state_path = root.join(CANDIDATE_SUPPLEMENTAL_STATE_FILE);
    match fs::symlink_metadata(&state_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeSupplementalSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::SupplementalStateUnavailable),
        Ok(_) => {}
    }
    let state_text = read_regular_utf8(
        &state_path,
        MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES,
        CandidateRuntimeError::SupplementalStateUnavailable,
    )?;
    let state = CandidateSupplementalState::parse(&state_text)
        .map_err(CandidateRuntimeError::SupplementalState)?;
    let Some(expected_package) = state.package() else {
        return Ok(CandidateRuntimeSupplementalSelection::Disabled);
    };
    let slot_text = read_regular_utf8(
        &root.join(CANDIDATE_SLOT_STATE_FILE),
        MAX_CANDIDATE_SLOT_STATE_BYTES,
        CandidateRuntimeError::SlotStateUnavailable,
    )?;
    let slots = CandidateSlotState::parse(&slot_text).map_err(CandidateRuntimeError::SlotState)?;
    if slots.current() != Some(expected_package) {
        return Err(CandidateRuntimeError::SupplementalPackageMismatch);
    }
    Ok(CandidateRuntimeSupplementalSelection::Enabled {
        package_id: expected_package.to_owned(),
        config: SupplementalCandidateLayerConfig {
            exact_promotions: state.exact_promotions(),
        },
    })
}

/// Loads one supplemental snapshot after its small selection changed.
///
/// The selection is checked again after the immutable package is loaded. If a
/// concurrent slot update changed either pointer, the load fails closed and a
/// caller can retain its last known-good in-memory snapshot.
pub fn load_candidate_runtime_supplemental(
    root: &Path,
    expected: &CandidateRuntimeSupplementalSelection,
) -> Result<Option<CandidateRuntimeSupplemental>, CandidateRuntimeError> {
    let loaded = match expected {
        CandidateRuntimeSupplementalSelection::Disabled => None,
        CandidateRuntimeSupplementalSelection::Enabled { package_id, config } => {
            let loaded = load_current_candidate_package(root)?
                .ok_or(CandidateRuntimeError::SupplementalPackageMismatch)?;
            if loaded.package_id != *package_id {
                return Err(CandidateRuntimeError::SupplementalPackageMismatch);
            }
            Some(CandidateRuntimeSupplemental {
                package_id: loaded.package_id,
                snapshot: loaded.snapshot,
                authentication_sha256: loaded.authentication_sha256,
                payload_sha256: loaded.payload_sha256,
                config: *config,
            })
        }
    };
    if load_candidate_runtime_supplemental_selection(root)? != *expected {
        return Err(CandidateRuntimeError::SupplementalSelectionChanged);
    }
    Ok(loaded)
}

/// Reads only the exact-short activation and slot pointers.
///
/// Package bytes are not opened until a caller observes a changed enabled
/// selection. This keeps the new-composition polling boundary small.
pub fn load_candidate_runtime_exact_short_selection(
    root: &Path,
) -> Result<CandidateRuntimeExactShortSelection, CandidateRuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeExactShortSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::ExactShortRootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CandidateRuntimeError::InvalidExactShortRoot);
        }
        Ok(_) => {}
    }
    let state_path = root.join(CANDIDATE_EXACT_SHORT_STATE_FILE);
    match fs::symlink_metadata(&state_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeExactShortSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::ExactShortStateUnavailable),
        Ok(_) => {}
    }
    let state_text = read_regular_utf8(
        &state_path,
        MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES,
        CandidateRuntimeError::ExactShortStateUnavailable,
    )?;
    let state = CandidateExactShortState::parse(&state_text)
        .map_err(CandidateRuntimeError::ExactShortState)?;
    let Some(expected_package) = state.package() else {
        return Ok(CandidateRuntimeExactShortSelection::Disabled);
    };
    let slot_text = read_regular_utf8(
        &root.join(CANDIDATE_SLOT_STATE_FILE),
        MAX_CANDIDATE_SLOT_STATE_BYTES,
        CandidateRuntimeError::SlotStateUnavailable,
    )?;
    let slots = CandidateSlotState::parse(&slot_text).map_err(CandidateRuntimeError::SlotState)?;
    if slots.current() != Some(expected_package) {
        return Err(CandidateRuntimeError::ExactShortPackageMismatch);
    }
    Ok(CandidateRuntimeExactShortSelection::Enabled {
        package_id: expected_package.to_owned(),
        exact_promotions: state.exact_promotions(),
    })
}

/// Loads one exact-short catalog after its small selection changed.
///
/// The selection is re-read after package authentication. A concurrent slot
/// update therefore cannot publish a catalog assembled from mixed states.
pub fn load_candidate_runtime_exact_short(
    root: &Path,
    expected: &CandidateRuntimeExactShortSelection,
) -> Result<Option<CandidateRuntimeExactShort>, CandidateRuntimeError> {
    let loaded = match expected {
        CandidateRuntimeExactShortSelection::Disabled => None,
        CandidateRuntimeExactShortSelection::Enabled {
            package_id,
            exact_promotions,
        } => {
            let loaded = load_current_runtime_package(root)?
                .ok_or(CandidateRuntimeError::ExactShortPackageMismatch)?;
            if loaded.package_id != *package_id {
                return Err(CandidateRuntimeError::ExactShortPackageMismatch);
            }
            let catalog = ExactShortWordCatalog::load(&loaded.manifest, &loaded.payload)
                .map_err(CandidateRuntimeError::ExactShortCatalog)?;
            Some(CandidateRuntimeExactShort {
                package_id: loaded.package_id,
                catalog: Arc::new(catalog),
                authentication_sha256: loaded.authentication_sha256,
                exact_promotions: *exact_promotions,
            })
        }
    };
    if load_candidate_runtime_exact_short_selection(root)? != *expected {
        return Err(CandidateRuntimeError::ExactShortSelectionChanged);
    }
    Ok(loaded)
}

/// Loads the immutable combined-path receipt for an enabled exact-short lane.
pub fn load_candidate_runtime_exact_short_preflight(
    root: &Path,
    expected: &CandidateRuntimeExactShortSelection,
) -> Result<Option<CandidateExactShortPreflightReceipt>, CandidateRuntimeError> {
    let receipt = match expected {
        CandidateRuntimeExactShortSelection::Disabled => None,
        CandidateRuntimeExactShortSelection::Enabled {
            package_id,
            exact_promotions,
        } => {
            let loaded = load_current_runtime_package(root)?
                .ok_or(CandidateRuntimeError::ExactShortPackageMismatch)?;
            if loaded.package_id != *package_id {
                return Err(CandidateRuntimeError::ExactShortPackageMismatch);
            }
            let text = read_regular_utf8(
                &root.join(CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE),
                MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES,
                CandidateRuntimeError::ExactShortCombinedPreflightUnavailable,
            )?;
            let receipt = CandidateExactShortPreflightReceipt::parse(&text)
                .map_err(CandidateRuntimeError::ExactShortCombinedPreflight)?;
            if receipt.exact_package() != package_id
                || receipt.exact_sha256() != loaded.authentication_sha256
                || receipt.exact_promotions() != *exact_promotions
            {
                return Err(CandidateRuntimeError::ExactShortCombinedPreflightMismatch);
            }
            Some(receipt)
        }
    };
    if load_candidate_runtime_exact_short_selection(root)? != *expected {
        return Err(CandidateRuntimeError::ExactShortSelectionChanged);
    }
    Ok(receipt)
}

/// Reads only the exact-phrase activation and slot pointers.
///
/// Package bytes and the combined receipt remain unopened until a caller sees
/// a changed enabled selection.
pub fn load_candidate_runtime_exact_phrase_selection(
    root: &Path,
) -> Result<CandidateRuntimeExactPhraseSelection, CandidateRuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeExactPhraseSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::ExactPhraseRootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CandidateRuntimeError::InvalidExactPhraseRoot);
        }
        Ok(_) => {}
    }
    let state_path = root.join(CANDIDATE_EXACT_PHRASE_STATE_FILE);
    match fs::symlink_metadata(&state_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateRuntimeExactPhraseSelection::Disabled);
        }
        Err(_) => return Err(CandidateRuntimeError::ExactPhraseStateUnavailable),
        Ok(_) => {}
    }
    let state_text = read_regular_utf8(
        &state_path,
        MAX_CANDIDATE_EXACT_PHRASE_STATE_BYTES,
        CandidateRuntimeError::ExactPhraseStateUnavailable,
    )?;
    let state = CandidateExactPhraseState::parse(&state_text)
        .map_err(CandidateRuntimeError::ExactPhraseState)?;
    let Some(expected_package) = state.package() else {
        return Ok(CandidateRuntimeExactPhraseSelection::Disabled);
    };
    let slot_text = read_regular_utf8(
        &root.join(CANDIDATE_SLOT_STATE_FILE),
        MAX_CANDIDATE_SLOT_STATE_BYTES,
        CandidateRuntimeError::SlotStateUnavailable,
    )?;
    let slots = CandidateSlotState::parse(&slot_text).map_err(CandidateRuntimeError::SlotState)?;
    if slots.current() != Some(expected_package) {
        return Err(CandidateRuntimeError::ExactPhrasePackageMismatch);
    }
    Ok(CandidateRuntimeExactPhraseSelection::Enabled {
        package_id: expected_package.to_owned(),
    })
}

/// Loads and validates one exact three-character phrase package.
///
/// Besides the ordinary immutable-package checks, every identity must be a
/// unique six-key three-syllable Han phrase and provenance must bind the exact
/// current core and supplemental payload bytes.
pub fn load_candidate_runtime_exact_phrase(
    root: &Path,
    expected: &CandidateRuntimeExactPhraseSelection,
    core_payload_sha256: &str,
    supplemental_payload_sha256: &str,
) -> Result<Option<CandidateRuntimeExactPhrase>, CandidateRuntimeError> {
    let loaded = match expected {
        CandidateRuntimeExactPhraseSelection::Disabled => None,
        CandidateRuntimeExactPhraseSelection::Enabled { package_id } => {
            if !runtime_sha256(core_payload_sha256) || !runtime_sha256(supplemental_payload_sha256)
            {
                return Err(CandidateRuntimeError::ExactPhraseProvenanceMismatch);
            }
            let loaded = load_current_runtime_package(root)?
                .ok_or(CandidateRuntimeError::ExactPhrasePackageMismatch)?;
            if loaded.package_id != *package_id {
                return Err(CandidateRuntimeError::ExactPhrasePackageMismatch);
            }
            if loaded.provenance.source_count() != 4 {
                return Err(CandidateRuntimeError::ExactPhraseProvenanceMismatch);
            }
            let material_hashes = loaded
                .provenance
                .source_materials()
                .iter()
                .map(crate::CandidateSourceMaterial::sha256)
                .collect::<HashSet<_>>();
            if !material_hashes.contains(core_payload_sha256)
                || !material_hashes.contains(supplemental_payload_sha256)
            {
                return Err(CandidateRuntimeError::ExactPhraseProvenanceMismatch);
            }
            let entries = parse_lexicon_tsv(&loaded.payload)
                .map_err(|_| CandidateRuntimeError::ExactPhrasePayloadProfile)?;
            if entries.is_empty() {
                return Err(CandidateRuntimeError::ExactPhrasePayloadProfile);
            }
            let mut codes = HashSet::with_capacity(entries.len());
            for entry in entries {
                if entry.text.chars().count() != 3
                    || !entry.text.chars().all(runtime_exact_phrase_character)
                    || entry.syllable_codes.len() != 3
                    || entry.code.as_str().len() != 6
                    || !codes.insert(entry.code.as_str().to_owned())
                {
                    return Err(CandidateRuntimeError::ExactPhrasePayloadProfile);
                }
            }
            let snapshot = loaded
                .manifest
                .load_snapshot(&loaded.payload)
                .map_err(CandidateRuntimeError::Package)?;
            Some(CandidateRuntimeExactPhrase {
                package_id: loaded.package_id,
                snapshot: Arc::new(snapshot),
                authentication_sha256: loaded.authentication_sha256,
            })
        }
    };
    if load_candidate_runtime_exact_phrase_selection(root)? != *expected {
        return Err(CandidateRuntimeError::ExactPhraseSelectionChanged);
    }
    Ok(loaded)
}

/// Loads the future real-TSF combined receipt for an enabled phrase lane.
pub fn load_candidate_runtime_exact_phrase_preflight(
    root: &Path,
    expected: &CandidateRuntimeExactPhraseSelection,
) -> Result<Option<CandidateExactPhrasePreflightReceipt>, CandidateRuntimeError> {
    let receipt = match expected {
        CandidateRuntimeExactPhraseSelection::Disabled => None,
        CandidateRuntimeExactPhraseSelection::Enabled { package_id } => {
            let loaded = load_current_runtime_package(root)?
                .ok_or(CandidateRuntimeError::ExactPhrasePackageMismatch)?;
            if loaded.package_id != *package_id {
                return Err(CandidateRuntimeError::ExactPhrasePackageMismatch);
            }
            let text = read_regular_utf8(
                &root.join(CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_FILE),
                MAX_CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_BYTES,
                CandidateRuntimeError::ExactPhraseCombinedPreflightUnavailable,
            )?;
            let receipt = CandidateExactPhrasePreflightReceipt::parse(&text)
                .map_err(CandidateRuntimeError::ExactPhraseCombinedPreflight)?;
            if receipt.phrase_package() != package_id
                || receipt.phrase_sha256() != loaded.authentication_sha256
            {
                return Err(CandidateRuntimeError::ExactPhraseCombinedPreflightMismatch);
            }
            Some(receipt)
        }
    };
    if load_candidate_runtime_exact_phrase_selection(root)? != *expected {
        return Err(CandidateRuntimeError::ExactPhraseSelectionChanged);
    }
    Ok(receipt)
}

fn runtime_exact_phrase_character(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af
    )
}

fn runtime_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn load_current_candidate_package(
    root: &Path,
) -> Result<Option<LoadedRuntimeCandidate>, CandidateRuntimeError> {
    let Some(loaded) = load_current_runtime_package(root)? else {
        return Ok(None);
    };
    let snapshot = loaded
        .manifest
        .load_snapshot(&loaded.payload)
        .map_err(CandidateRuntimeError::Package)?;
    Ok(Some(LoadedRuntimeCandidate {
        package_id: loaded.package_id,
        snapshot: Arc::new(snapshot),
        authentication_sha256: loaded.authentication_sha256,
        payload_sha256: candidate_sha256_hex(loaded.payload.as_bytes()),
    }))
}

fn load_current_runtime_package(
    root: &Path,
) -> Result<Option<LoadedRuntimePackage>, CandidateRuntimeError> {
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
    let provenance_text = read_regular_utf8(
        &package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
        MAX_CANDIDATE_PROVENANCE_BYTES,
        CandidateRuntimeError::ProvenanceUnavailable,
    )?;
    let provenance = CandidatePackageProvenance::parse(&provenance_text)
        .map_err(CandidateRuntimeError::Provenance)?;
    let payload_text = read_regular_utf8(
        &package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        MAX_CANDIDATE_SNAPSHOT_BYTES,
        CandidateRuntimeError::PayloadUnavailable,
    )?;
    provenance
        .validate_materials(&manifest_text, &payload_text)
        .map_err(CandidateRuntimeError::Provenance)?;
    if candidate_package_storage_id(&provenance_text, &manifest_text, &payload_text) != package_id {
        return Err(CandidateRuntimeError::StorageIdentifierMismatch);
    }
    let package_authentication_sha256 =
        candidate_package_authentication_sha256(&provenance_text, &manifest_text, &payload_text);

    let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
    ensure_regular_directory(&preflights, CandidateRuntimeError::InvalidPreflightStore)?;
    let receipt = read_regular_utf8(
        &preflights.join(format!("{package_id}.zpf")),
        MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
        CandidateRuntimeError::PreflightReceiptUnavailable,
    )?;
    if receipt != candidate_preflight_receipt_body(package_id, &package_authentication_sha256) {
        return Err(CandidateRuntimeError::PreflightReceiptMismatch);
    }

    Ok(Some(LoadedRuntimePackage {
        package_id: package_id.to_owned(),
        manifest,
        provenance,
        payload: payload_text,
        authentication_sha256: package_authentication_sha256,
    }))
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
    /// The exact public-source sidecar is missing, unsafe, or unreadable.
    ProvenanceUnavailable,
    /// The exact lexicon payload is missing, unsafe, or unreadable.
    PayloadUnavailable,
    /// The package metadata or payload did not validate.
    Package(CandidatePackageError),
    /// The source declaration, compatibility, or SHA-256 binding failed.
    Provenance(CandidateProvenanceError),
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
    /// The optional supplemental activation state could not be read safely.
    SupplementalStateUnavailable,
    /// The supplemental root metadata could not be inspected.
    SupplementalRootUnavailable,
    /// The existing supplemental root is not a regular directory.
    InvalidSupplementalRoot,
    /// The supplemental activation state did not satisfy its strict schema.
    SupplementalState(CandidateSupplementalStateError),
    /// The activation state and current package slot do not select the same package.
    SupplementalPackageMismatch,
    /// The supplemental selection changed while its immutable package was loading.
    SupplementalSelectionChanged,
    /// The optional exact-short activation state could not be read safely.
    ExactShortStateUnavailable,
    /// The exact-short root metadata could not be inspected.
    ExactShortRootUnavailable,
    /// The existing exact-short root is not a regular directory.
    InvalidExactShortRoot,
    /// The exact-short activation state did not satisfy its strict schema.
    ExactShortState(CandidateExactShortStateError),
    /// The activation state and current package slot do not select the same package.
    ExactShortPackageMismatch,
    /// The exact-short payload did not satisfy the strict two-character profile.
    ExactShortCatalog(ExactShortWordCatalogError),
    /// The combined exact-short TSF preflight receipt is absent or unreadable.
    ExactShortCombinedPreflightUnavailable,
    /// The combined exact-short TSF preflight receipt is malformed.
    ExactShortCombinedPreflight(CandidateExactShortPreflightReceiptError),
    /// The combined receipt does not bind the enabled package and cap.
    ExactShortCombinedPreflightMismatch,
    /// The exact-short selection changed while its immutable package was loading.
    ExactShortSelectionChanged,
    /// The optional exact-phrase activation state could not be read safely.
    ExactPhraseStateUnavailable,
    /// The exact-phrase root metadata could not be inspected.
    ExactPhraseRootUnavailable,
    /// The existing exact-phrase root is not a regular directory.
    InvalidExactPhraseRoot,
    /// The exact-phrase activation state did not satisfy its strict schema.
    ExactPhraseState(CandidateExactPhraseStateError),
    /// The activation state and current package slot select different packages.
    ExactPhrasePackageMismatch,
    /// The enabled phrase layer requires an enabled, validated supplement.
    ExactPhraseSupplementalUnavailable,
    /// The four-source phrase provenance does not bind current payload bytes.
    ExactPhraseProvenanceMismatch,
    /// The phrase payload is not a unique three-Han, three-syllable, six-key catalog.
    ExactPhrasePayloadProfile,
    /// The combined exact-phrase TSF preflight receipt is absent or unreadable.
    ExactPhraseCombinedPreflightUnavailable,
    /// The combined exact-phrase receipt is malformed.
    ExactPhraseCombinedPreflight(CandidateExactPhrasePreflightReceiptError),
    /// The combined receipt does not bind all enabled package identities.
    ExactPhraseCombinedPreflightMismatch,
    /// The exact-phrase selection changed while its immutable package was loading.
    ExactPhraseSelectionChanged,
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
            Self::ProvenanceUnavailable => "当前候选包来源声明不可用",
            Self::PayloadUnavailable => "当前候选包载荷不可用",
            Self::Package(_) => "当前候选包校验失败",
            Self::Provenance(_) => "当前候选包来源或兼容性校验失败",
            Self::PrivatePlaintext => "TSF alpha 不接受明文私人候选包",
            Self::StorageIdentifierMismatch => "当前候选包与存储标识不符",
            Self::InvalidPreflightStore => "候选包预检存储无效",
            Self::PreflightReceiptUnavailable => "当前候选包缺少预检凭据",
            Self::PreflightReceiptMismatch => "当前候选包预检凭据不符",
            Self::SupplementalStateUnavailable => "补充词层状态不可用",
            Self::SupplementalRootUnavailable => "无法检查补充词层目录",
            Self::InvalidSupplementalRoot => "补充词层目录无效",
            Self::SupplementalState(_) => "补充词层状态无效",
            Self::SupplementalPackageMismatch => "补充词层状态与当前候选包不符",
            Self::SupplementalSelectionChanged => "补充词层在载入期间发生变化",
            Self::ExactShortStateUnavailable => "精确短词层状态不可用",
            Self::ExactShortRootUnavailable => "无法检查精确短词层目录",
            Self::InvalidExactShortRoot => "精确短词层目录无效",
            Self::ExactShortState(_) => "精确短词层状态无效",
            Self::ExactShortPackageMismatch => "精确短词层状态与当前候选包不符",
            Self::ExactShortCatalog(_) => "精确短词层载荷校验失败",
            Self::ExactShortCombinedPreflightUnavailable => "精确短词层缺少专项预检凭据",
            Self::ExactShortCombinedPreflight(_) => "精确短词层专项预检凭据无效",
            Self::ExactShortCombinedPreflightMismatch => "精确短词层专项预检凭据不符",
            Self::ExactShortSelectionChanged => "精确短词层在载入期间发生变化",
            Self::ExactPhraseStateUnavailable => "三字精确层状态不可用",
            Self::ExactPhraseRootUnavailable => "无法检查三字精确层目录",
            Self::InvalidExactPhraseRoot => "三字精确层目录无效",
            Self::ExactPhraseState(_) => "三字精确层状态无效",
            Self::ExactPhrasePackageMismatch => "三字精确层状态与当前候选包不符",
            Self::ExactPhraseSupplementalUnavailable => "三字精确层缺少当前补充词层",
            Self::ExactPhraseProvenanceMismatch => "三字精确层未绑定当前公开载荷",
            Self::ExactPhrasePayloadProfile => "三字精确层载荷形状无效",
            Self::ExactPhraseCombinedPreflightUnavailable => "三字精确层缺少专项预检凭据",
            Self::ExactPhraseCombinedPreflight(_) => "三字精确层专项预检凭据无效",
            Self::ExactPhraseCombinedPreflightMismatch => "三字精确层专项预检凭据不符",
            Self::ExactPhraseSelectionChanged => "三字精确层在载入期间发生变化",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SlotState(error) => Some(error),
            Self::Package(error) => Some(error),
            Self::Provenance(error) => Some(error),
            Self::SupplementalState(error) => Some(error),
            Self::ExactShortState(error) => Some(error),
            Self::ExactShortCatalog(error) => Some(error),
            Self::ExactShortCombinedPreflight(error) => Some(error),
            Self::ExactPhraseState(error) => Some(error),
            Self::ExactPhraseCombinedPreflight(error) => Some(error),
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
        let provenance_text = CandidatePackageProvenance::from_materials(
            "runtime-test-source",
            "MPL-2.0",
            "https://github.com/hewzhew/ziranma-decoder",
            &crate::candidate_sha256_hex(payload.as_bytes()),
            &manifest_text,
            payload,
        )
        .unwrap()
        .render();
        let package_id = candidate_package_storage_id(&provenance_text, &manifest_text, payload);
        let package_authentication_sha256 =
            candidate_package_authentication_sha256(&provenance_text, &manifest_text, payload);
        let package = root.join(CANDIDATE_PACKAGES_DIRECTORY).join(&package_id);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_MANIFEST_FILE), manifest_text).unwrap();
        fs::write(
            package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance_text,
        )
        .unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), payload).unwrap();
        let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
        fs::create_dir_all(&preflights).unwrap();
        fs::write(
            preflights.join(format!("{package_id}.zpf")),
            candidate_preflight_receipt_body(&package_id, &package_authentication_sha256),
        )
        .unwrap();
        package_id
    }

    fn install_test_exact_phrase_package(
        root: &Path,
        revision: &str,
        payload: &str,
        bound_core_payload: &str,
        bound_supplemental_payload: &str,
    ) -> String {
        let manifest = CandidatePackageManifest::from_payload(revision, false, payload).unwrap();
        let manifest_text = manifest.render();
        let provenance_text = CandidatePackageProvenance::from_source_materials(
            vec![
                crate::CandidateSourceMaterial::from_bytes(
                    "phrase-source",
                    "CC-BY-4.0",
                    "https://example.com/phrase-source",
                    b"phrase-source",
                )
                .unwrap(),
                crate::CandidateSourceMaterial::from_bytes(
                    "core-payload",
                    "Apache-2.0",
                    "https://example.com/core-payload",
                    bound_core_payload.as_bytes(),
                )
                .unwrap(),
                crate::CandidateSourceMaterial::from_bytes(
                    "supplemental-payload",
                    "CC-BY-4.0",
                    "https://example.com/supplemental-payload",
                    bound_supplemental_payload.as_bytes(),
                )
                .unwrap(),
                crate::CandidateSourceMaterial::from_bytes(
                    "fit-corpus",
                    "CC-BY-SA-4.0",
                    "https://example.com/fit-corpus",
                    b"fit-corpus",
                )
                .unwrap(),
            ],
            &manifest_text,
            payload,
        )
        .unwrap()
        .render();
        let package_id = candidate_package_storage_id(&provenance_text, &manifest_text, payload);
        let authentication_sha256 =
            candidate_package_authentication_sha256(&provenance_text, &manifest_text, payload);
        let package = root.join(CANDIDATE_PACKAGES_DIRECTORY).join(&package_id);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_MANIFEST_FILE), manifest_text).unwrap();
        fs::write(
            package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance_text,
        )
        .unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), payload).unwrap();
        let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
        fs::create_dir_all(&preflights).unwrap();
        fs::write(
            preflights.join(format!("{package_id}.zpf")),
            candidate_preflight_receipt_body(&package_id, &authentication_sha256),
        )
        .unwrap();
        package_id
    }

    fn write_exact_short_test_preflight(
        exact_root: &Path,
        exact_package: &str,
        exact_promotions: usize,
        core_root: &Path,
        supplemental: Option<(&Path, usize)>,
    ) {
        let core = load_current_runtime_package(core_root).unwrap().unwrap();
        let exact = load_current_runtime_package(exact_root).unwrap().unwrap();
        assert_eq!(exact.package_id, exact_package);
        let supplemental_identity = supplemental.map(|(root, promotions)| {
            let loaded = load_current_runtime_package(root).unwrap().unwrap();
            (loaded.authentication_sha256, promotions)
        });
        let receipt = CandidateExactShortPreflightReceipt::new(
            exact_package,
            &exact.authentication_sha256,
            exact_promotions,
            &core.authentication_sha256,
            supplemental_identity
                .as_ref()
                .map(|(sha256, promotions)| (sha256.as_str(), *promotions)),
        )
        .unwrap();
        fs::write(
            exact_root.join(CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE),
            receipt.render(),
        )
        .unwrap();
    }

    fn write_exact_phrase_test_preflight(
        phrase_root: &Path,
        phrase_package: &str,
        core_root: &Path,
        supplemental_root: &Path,
        supplemental_promotions: usize,
    ) {
        let core = load_current_runtime_package(core_root).unwrap().unwrap();
        let supplemental = load_current_runtime_package(supplemental_root)
            .unwrap()
            .unwrap();
        let phrase = load_current_runtime_package(phrase_root).unwrap().unwrap();
        assert_eq!(phrase.package_id, phrase_package);
        let receipt = CandidateExactPhrasePreflightReceipt::new(
            phrase_package,
            &phrase.authentication_sha256,
            &core.authentication_sha256,
            &supplemental.authentication_sha256,
            supplemental_promotions,
        )
        .unwrap();
        fs::write(
            phrase_root.join(CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_FILE),
            receipt.render(),
        )
        .unwrap();
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
        fs::write(
            &receipt,
            format!(
                "schema=ziranma-candidate-preflight-v1\n\
                 package={package_id}\n\
                 host={CANDIDATE_PREFLIGHT_HOST_V1}\n"
            ),
        )
        .unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::PreflightReceiptMismatch
        );
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
    fn provenance_changes_the_immutable_storage_identifier() {
        let manifest =
            CandidatePackageManifest::from_payload("storage-id", false, PAYLOAD).unwrap();
        let manifest_text = manifest.render();
        let provenance = CandidatePackageProvenance::from_materials(
            "runtime-test-source",
            "MPL-2.0",
            "https://github.com/hewzhew/ziranma-decoder",
            &crate::candidate_sha256_hex(PAYLOAD.as_bytes()),
            &manifest_text,
            PAYLOAD,
        )
        .unwrap()
        .render();
        let changed = provenance.replace("source_license=MPL-2.0", "source_license=Apache-2.0");

        assert_ne!(
            candidate_package_storage_id(&provenance, &manifest_text, PAYLOAD),
            candidate_package_storage_id(&changed, &manifest_text, PAYLOAD)
        );
    }

    #[test]
    fn missing_or_incompatible_provenance_is_rejected() {
        let (root, package_id) = configured_root("runtime-provenance", false);
        let provenance = root
            .path()
            .join(CANDIDATE_PACKAGES_DIRECTORY)
            .join(package_id)
            .join(CANDIDATE_PACKAGE_PROVENANCE_FILE);
        let original = fs::read_to_string(&provenance).unwrap();
        fs::remove_file(&provenance).unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::ProvenanceUnavailable
        );
        fs::write(
            provenance,
            original.replace(CANDIDATE_DECODER_COMPATIBILITY_V1, "future-decoder"),
        )
        .unwrap();
        assert_eq!(
            load_current_candidate_snapshot(root.path()).unwrap_err(),
            CandidateRuntimeError::Provenance(CandidateProvenanceError::IncompatibleDecoder)
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
            CandidateRuntimeError::Package(_) | CandidateRuntimeError::Provenance(_)
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

    #[test]
    fn supplemental_root_is_default_off_and_loads_only_an_exact_bound_package() {
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n属于\tshu yu\t100\n";
        let (core_root, _) = configured_root("runtime-core", false);
        let supplemental_root = TestDirectory::new();
        let supplemental_id = install_test_package(
            supplemental_root.path(),
            "runtime-supplemental",
            false,
            SUPPLEMENTAL,
        );
        let mut supplemental_slots = CandidateSlotState::default();
        supplemental_slots.adopt(&supplemental_id).unwrap();
        fs::write(
            supplemental_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            supplemental_slots.render(),
        )
        .unwrap();

        let disabled =
            load_candidate_runtime_snapshots(core_root.path(), Some(supplemental_root.path()))
                .unwrap()
                .unwrap();
        assert!(disabled.supplemental().is_none());
        assert!(!disabled.supplemental_fell_back());

        let state = CandidateSupplementalState::enabled(&supplemental_id, 1).unwrap();
        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            state.render(),
        )
        .unwrap();
        let enabled =
            load_candidate_runtime_snapshots(core_root.path(), Some(supplemental_root.path()))
                .unwrap()
                .unwrap();
        let supplemental = enabled.supplemental().unwrap();
        assert_eq!(supplemental.snapshot().revision(), "runtime-supplemental");
        assert_eq!(supplemental.config().exact_promotions, 1);
        assert!(!enabled.supplemental_fell_back());
        assert_eq!(enabled.core().revision(), "runtime-core");
    }

    #[test]
    fn supplemental_damage_or_pointer_drift_falls_back_to_unchanged_core() {
        const FIRST: &str = "text\tpinyin\tfrequency\n属于\tshu yu\t100\n";
        const SECOND: &str = "text\tpinyin\tfrequency\n甚么\tshen me\t100\n";
        let (core_root, _) = configured_root("runtime-fallback-core", false);
        let supplemental_root = TestDirectory::new();
        let first_id = install_test_package(
            supplemental_root.path(),
            "runtime-supplemental-first",
            false,
            FIRST,
        );
        let second_id = install_test_package(
            supplemental_root.path(),
            "runtime-supplemental-second",
            false,
            SECOND,
        );
        let mut slots = CandidateSlotState::default();
        slots.adopt(&first_id).unwrap();
        fs::write(
            supplemental_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&first_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();

        slots.stage(&second_id).unwrap();
        slots.promote().unwrap();
        fs::write(
            supplemental_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        let drifted =
            load_candidate_runtime_snapshots(core_root.path(), Some(supplemental_root.path()))
                .unwrap()
                .unwrap();
        assert!(drifted.supplemental().is_none());
        assert!(drifted.supplemental_fell_back());
        assert_eq!(drifted.core().revision(), "runtime-fallback-core");

        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            "schema=damaged\n",
        )
        .unwrap();
        let damaged =
            load_candidate_runtime_snapshots(core_root.path(), Some(supplemental_root.path()))
                .unwrap()
                .unwrap();
        assert!(damaged.supplemental().is_none());
        assert!(damaged.supplemental_fell_back());
        assert_eq!(damaged.core().revision(), "runtime-fallback-core");
    }

    #[test]
    fn exact_short_root_is_default_off_and_loads_only_a_strict_catalog() {
        const EXACT: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t90\n\
手术\tshou shu\t80\n";
        let (core_root, _) = configured_root("runtime-exact-core", false);
        let exact_root = TestDirectory::new();
        let exact_id = install_test_package(exact_root.path(), "runtime-exact-short", false, EXACT);
        let mut slots = CandidateSlotState::default();
        slots.adopt(&exact_id).unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();

        let disabled = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(disabled.exact_short().is_none());
        assert!(!disabled.exact_short_fell_back());

        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            CandidateExactShortState::enabled(&exact_id, 2)
                .unwrap()
                .render(),
        )
        .unwrap();
        let missing_receipt = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(missing_receipt.exact_short().is_none());
        assert!(missing_receipt.exact_short_fell_back());
        write_exact_short_test_preflight(exact_root.path(), &exact_id, 2, core_root.path(), None);
        let enabled = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        let exact = enabled.exact_short().unwrap();
        assert_eq!(exact.package_id(), exact_id);
        assert_eq!(exact.catalog().revision(), "runtime-exact-short");
        assert_eq!(exact.exact_promotions(), 2);
        assert_eq!(
            exact.catalog().candidate_texts("ubuu", 2).unwrap(),
            ["收束", "手术"]
        );
        assert!(!enabled.exact_short_fell_back());
        assert_eq!(enabled.core().revision(), "runtime-exact-core");
    }

    #[test]
    fn exact_short_combined_receipt_rejects_runtime_layer_and_cap_drift() {
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n属于\tshu yu\t100\n";
        const EXACT: &str = "text\tpinyin\tfrequency\n收束\tshou shu\t90\n";
        let (core_root, _) = configured_root("runtime-bound-core", false);
        let (different_core_root, _) = configured_root("runtime-different-core", false);
        let supplemental_root = TestDirectory::new();
        let supplemental_id = install_test_package(
            supplemental_root.path(),
            "runtime-bound-supplemental",
            false,
            SUPPLEMENTAL,
        );
        let mut supplemental_slots = CandidateSlotState::default();
        supplemental_slots.adopt(&supplemental_id).unwrap();
        fs::write(
            supplemental_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            supplemental_slots.render(),
        )
        .unwrap();
        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&supplemental_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();

        let exact_root = TestDirectory::new();
        let exact_id = install_test_package(exact_root.path(), "runtime-bound-exact", false, EXACT);
        let mut exact_slots = CandidateSlotState::default();
        exact_slots.adopt(&exact_id).unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            exact_slots.render(),
        )
        .unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            CandidateExactShortState::enabled(&exact_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();
        write_exact_short_test_preflight(
            exact_root.path(),
            &exact_id,
            1,
            core_root.path(),
            Some((supplemental_root.path(), 1)),
        );

        let valid = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(valid.supplemental().is_some());
        assert!(valid.exact_short().is_some());

        let wrong_core = load_candidate_runtime_snapshots_with_layers(
            different_core_root.path(),
            Some(supplemental_root.path()),
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(wrong_core.supplemental().is_some());
        assert!(wrong_core.exact_short().is_none());
        assert!(wrong_core.exact_short_fell_back());

        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&supplemental_id, 2)
                .unwrap()
                .render(),
        )
        .unwrap();
        let wrong_supplemental_cap = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            wrong_supplemental_cap
                .supplemental()
                .unwrap()
                .config()
                .exact_promotions,
            2
        );
        assert!(wrong_supplemental_cap.exact_short().is_none());
        assert!(wrong_supplemental_cap.exact_short_fell_back());

        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&supplemental_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            CandidateExactShortState::enabled(&exact_id, 2)
                .unwrap()
                .render(),
        )
        .unwrap();
        let wrong_exact_cap = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(wrong_exact_cap.exact_short().is_none());
        assert!(wrong_exact_cap.exact_short_fell_back());
    }

    #[test]
    fn exact_short_damage_pointer_drift_and_wrong_payload_profile_fail_closed() {
        const FIRST: &str = "text\tpinyin\tfrequency\n收束\tshou shu\t90\n";
        const WRONG_PROFILE: &str = "text\tpinyin\tfrequency\n候选词\thou xuan ci\t90\n";
        let (core_root, _) = configured_root("runtime-exact-fallback-core", false);
        let exact_root = TestDirectory::new();
        let first_id = install_test_package(exact_root.path(), "runtime-exact-first", false, FIRST);
        let wrong_id = install_test_package(
            exact_root.path(),
            "runtime-exact-wrong-profile",
            false,
            WRONG_PROFILE,
        );
        let mut slots = CandidateSlotState::default();
        slots.adopt(&first_id).unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            CandidateExactShortState::enabled(&first_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();

        slots.stage(&wrong_id).unwrap();
        slots.promote().unwrap();
        fs::write(
            exact_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        let drifted = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(drifted.exact_short().is_none());
        assert!(drifted.exact_short_fell_back());
        assert_eq!(drifted.core().revision(), "runtime-exact-fallback-core");

        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            CandidateExactShortState::enabled(&wrong_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();
        assert!(matches!(
            load_candidate_runtime_exact_short(
                exact_root.path(),
                &load_candidate_runtime_exact_short_selection(exact_root.path()).unwrap()
            ),
            Err(CandidateRuntimeError::ExactShortCatalog(_))
        ));
        let wrong_profile = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(wrong_profile.exact_short().is_none());
        assert!(wrong_profile.exact_short_fell_back());
        assert_eq!(
            wrong_profile.core().revision(),
            "runtime-exact-fallback-core"
        );

        fs::write(
            exact_root.path().join(CANDIDATE_EXACT_SHORT_STATE_FILE),
            "schema=damaged\n",
        )
        .unwrap();
        let damaged = load_candidate_runtime_snapshots_with_layers(
            core_root.path(),
            None,
            Some(exact_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(damaged.exact_short().is_none());
        assert!(damaged.exact_short_fell_back());
    }

    #[test]
    fn exact_phrase_root_is_default_off_and_requires_the_complete_runtime_receipt() {
        const SUPPLEMENTAL: &str =
            "text\tpinyin\tfrequency\n其他词\tqi ta ci\t100\n属于\tshu yu\t90\n";
        const PHRASE: &str = "text\tpinyin\tfrequency\n再进来\tzai jin lai\t80\n";
        let (core_root, _) = configured_root("runtime-phrase-core", false);
        let supplemental_root = TestDirectory::new();
        let supplemental_id = install_test_package(
            supplemental_root.path(),
            "runtime-phrase-supplemental",
            false,
            SUPPLEMENTAL,
        );
        let mut supplemental_slots = CandidateSlotState::default();
        supplemental_slots.adopt(&supplemental_id).unwrap();
        fs::write(
            supplemental_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            supplemental_slots.render(),
        )
        .unwrap();
        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&supplemental_id, 1)
                .unwrap()
                .render(),
        )
        .unwrap();

        let phrase_root = TestDirectory::new();
        let phrase_id = install_test_exact_phrase_package(
            phrase_root.path(),
            "runtime-exact-phrase",
            PHRASE,
            PAYLOAD,
            SUPPLEMENTAL,
        );
        let mut phrase_slots = CandidateSlotState::default();
        phrase_slots.adopt(&phrase_id).unwrap();
        fs::write(
            phrase_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            phrase_slots.render(),
        )
        .unwrap();

        let disabled = load_candidate_runtime_snapshots_with_all_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            None,
            Some(phrase_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(disabled.exact_phrase().is_none());
        assert!(!disabled.exact_phrase_fell_back());

        fs::write(
            phrase_root.path().join(CANDIDATE_EXACT_PHRASE_STATE_FILE),
            CandidateExactPhraseState::enabled(&phrase_id)
                .unwrap()
                .render(),
        )
        .unwrap();
        let missing_receipt = load_candidate_runtime_snapshots_with_all_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            None,
            Some(phrase_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(missing_receipt.exact_phrase().is_none());
        assert!(missing_receipt.exact_phrase_fell_back());

        write_exact_phrase_test_preflight(
            phrase_root.path(),
            &phrase_id,
            core_root.path(),
            supplemental_root.path(),
            1,
        );
        let enabled = load_candidate_runtime_snapshots_with_all_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            None,
            Some(phrase_root.path()),
        )
        .unwrap()
        .unwrap();
        let phrase = enabled.exact_phrase().unwrap();
        assert_eq!(phrase.package_id(), phrase_id);
        assert_eq!(phrase.snapshot().revision(), "runtime-exact-phrase");
        assert_eq!(
            phrase
                .snapshot()
                .exact_full_code_texts("zljnll", 1)
                .unwrap(),
            ["再进来"]
        );
        assert!(!enabled.exact_phrase_fell_back());

        fs::write(
            supplemental_root
                .path()
                .join(CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            CandidateSupplementalState::enabled(&supplemental_id, 2)
                .unwrap()
                .render(),
        )
        .unwrap();
        let cap_drift = load_candidate_runtime_snapshots_with_all_layers(
            core_root.path(),
            Some(supplemental_root.path()),
            None,
            Some(phrase_root.path()),
        )
        .unwrap()
        .unwrap();
        assert!(cap_drift.supplemental().is_some());
        assert!(cap_drift.exact_phrase().is_none());
        assert!(cap_drift.exact_phrase_fell_back());
    }

    #[test]
    fn exact_phrase_payload_shape_and_four_source_binding_fail_closed() {
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n其他词\tqi ta ci\t100\n";
        const PHRASE: &str = "text\tpinyin\tfrequency\n再进来\tzai jin lai\t80\n";
        const WRONG_PROFILE: &str = "text\tpinyin\tfrequency\n再进来\tzai jin\t80\n";
        const DIFFERENT_CORE: &str = "text\tpinyin\tfrequency\n不同\tbu tong\t1\n";
        let core_sha256 = candidate_sha256_hex(PAYLOAD.as_bytes());
        let supplemental_sha256 = candidate_sha256_hex(SUPPLEMENTAL.as_bytes());

        let wrong_binding_root = TestDirectory::new();
        let wrong_binding_id = install_test_exact_phrase_package(
            wrong_binding_root.path(),
            "runtime-phrase-wrong-binding",
            PHRASE,
            DIFFERENT_CORE,
            SUPPLEMENTAL,
        );
        let mut slots = CandidateSlotState::default();
        slots.adopt(&wrong_binding_id).unwrap();
        fs::write(
            wrong_binding_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        fs::write(
            wrong_binding_root
                .path()
                .join(CANDIDATE_EXACT_PHRASE_STATE_FILE),
            CandidateExactPhraseState::enabled(&wrong_binding_id)
                .unwrap()
                .render(),
        )
        .unwrap();
        let wrong_binding_selection =
            load_candidate_runtime_exact_phrase_selection(wrong_binding_root.path()).unwrap();
        assert_eq!(
            load_candidate_runtime_exact_phrase(
                wrong_binding_root.path(),
                &wrong_binding_selection,
                &core_sha256,
                &supplemental_sha256,
            )
            .unwrap_err(),
            CandidateRuntimeError::ExactPhraseProvenanceMismatch
        );

        let wrong_profile_root = TestDirectory::new();
        let wrong_profile_id = install_test_exact_phrase_package(
            wrong_profile_root.path(),
            "runtime-phrase-wrong-profile",
            WRONG_PROFILE,
            PAYLOAD,
            SUPPLEMENTAL,
        );
        let mut slots = CandidateSlotState::default();
        slots.adopt(&wrong_profile_id).unwrap();
        fs::write(
            wrong_profile_root.path().join(CANDIDATE_SLOT_STATE_FILE),
            slots.render(),
        )
        .unwrap();
        fs::write(
            wrong_profile_root
                .path()
                .join(CANDIDATE_EXACT_PHRASE_STATE_FILE),
            CandidateExactPhraseState::enabled(&wrong_profile_id)
                .unwrap()
                .render(),
        )
        .unwrap();
        let wrong_profile_selection =
            load_candidate_runtime_exact_phrase_selection(wrong_profile_root.path()).unwrap();
        assert_eq!(
            load_candidate_runtime_exact_phrase(
                wrong_profile_root.path(),
                &wrong_profile_selection,
                &core_sha256,
                &supplemental_sha256,
            )
            .unwrap_err(),
            CandidateRuntimeError::ExactPhrasePayloadProfile
        );
    }
}
