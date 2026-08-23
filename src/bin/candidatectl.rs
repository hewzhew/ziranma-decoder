//! Explicit construction, inspection, and local slotting of candidate packages.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::hint::black_box;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

use ziranma_core::{
    CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE, CANDIDATE_EXACT_SHORT_STATE_FILE,
    CANDIDATE_PACKAGE_MANIFEST_FILE, CANDIDATE_PACKAGE_PAYLOAD_FILE,
    CANDIDATE_PACKAGE_PROVENANCE_FILE, CANDIDATE_PACKAGES_DIRECTORY,
    CANDIDATE_PREFLIGHTS_DIRECTORY, CANDIDATE_SLOT_STATE_FILE, CANDIDATE_SNAPSHOT_SCHEMA_V1,
    CANDIDATE_SUPPLEMENTAL_STATE_FILE, CandidateExactShortPreflightReceipt,
    CandidateExactShortState, CandidatePackageManifest, CandidatePackageProvenance,
    CandidateReleaseSignature, CandidateSlotState, CandidateSnapshot, CandidateSnapshotDescriptor,
    CandidateSourceMaterial, CandidateSupplementalState, CharacterBigramLanguageModel,
    ContinuousCompositionProbe, Decoder, DecoderIndexStats, ExactShortPageSession,
    ExactShortWordCatalog, FourCharacterCorrectionDecision, FourCharacterCorrectionKeepReason,
    LexiconEntry, MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES,
    MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES, MAX_CANDIDATE_PROVENANCE_BYTES,
    MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES, MAX_CANDIDATE_SLOT_STATE_BYTES,
    MAX_CANDIDATE_SNAPSHOT_BYTES, MAX_CANDIDATE_SNAPSHOT_ENTRIES, MAX_CANDIDATE_SNAPSHOT_RANK,
    MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES, MAX_EXACT_SHORT_WORDS_PER_CODE,
    MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES, MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES,
    MAX_PUBLIC_RIME_SLICE_ENTRIES, MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS, MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH,
    MAX_PUBLIC_SHORT_WORD_CONFIRMATION_BYTES, MAX_PUBLIC_SHORT_WORD_CONSENSUS_ENTRIES,
    PublicLexiconRankProbe, PublicLexiconTokenCoverageAudit, PublicRimeSliceConfig,
    PublicRimeSliceImportStats, PublicSupplementalCompositionProbe,
    SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH, SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH,
    SentenceCandidate, ShortWordExtraKeyCorrectionDecision, ShortWordExtraKeyCorrectionKeepReason,
    SupplementalCandidateLayerConfig, SupplementalCompositionCandidate,
    SupplementalCompositionOrder, UdCorpusImportStats, are_qwerty_neighbors,
    audit_public_lexicon_token_coverage, audit_public_rime_target, audit_public_supplemental_layer,
    candidate_package_authentication_sha256, candidate_package_storage_id,
    candidate_payload_fingerprint, candidate_preflight_receipt_body, candidate_sha256_hex,
    compare_public_lexicons, encode_pinyin_phrase, layered_candidate_texts,
    layered_candidate_texts_with_consensus, layered_four_character_correction_decision,
    layered_short_word_extra_key_correction_decision, load_candidate_runtime_snapshots_with_layers,
    load_current_candidate_snapshot, normalize_pinyin_tone_marks, parse_lexicon_tsv,
    parse_public_rime_phrase_allowlist, parse_public_rime_slice, parse_public_short_word_consensus,
    parse_rime_lexicon, parse_simplified_rime_lexicon, parse_ud_conllu,
    select_public_bigram_training_sequences, select_public_character_training_texts,
    select_public_continuous_composition_cases, select_public_han_span_rank_probes,
    select_public_lexicon_rank_probes, select_public_single_character_context_cases,
    select_public_static_context_cases, select_public_supplemental_composition_cases,
    supplemental_complete_composition_texts, supplemental_complete_composition_texts_with_order,
    supplemental_complete_compositions_with_order,
};
#[cfg(windows)]
use ziranma_core::{
    CandidatePopupRenderPreflightReport, CandidatePopupRenderSample, CandidatePopupRenderScenario,
    ExactPhrasePopupRenderPreflightReport, TSF_ALPHA_CANDIDATE_PAGE_SIZE,
    TsfCandidatePreflightError, preflight_candidate_popup_rendering, preflight_candidate_snapshot,
    preflight_exact_phrase_candidate_layers, preflight_exact_phrase_candidate_popup_rendering,
    preflight_exact_short_candidate_layers,
};

const PINNED_RIME_PINYIN_SIMP_SHA256: &str =
    "e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b";
const MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES: usize = 16 * 1024 * 1024;
const MAX_STATIC_CONTEXT_ARPA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_STATIC_CONTEXT_ARPA_LINE_BYTES: usize = 1024 * 1024;
// Keep the portable diagnostic aligned with the Windows host's fixed page
// width. The CLI is also built on non-Windows CI, where the TSF constant is
// intentionally unavailable.
const RUNTIME_QUERY_PAGE_SIZE: usize = 6;

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Inspect {
        manifest: PathBuf,
        payload: PathBuf,
        provenance: PathBuf,
    },
    Build {
        source: PathBuf,
        output: PathBuf,
        revision: String,
        declaration: PublicSourceDeclaration,
    },
    BuildRime {
        source: PathBuf,
        output: PathBuf,
        revision: String,
        declaration: PublicSourceDeclaration,
    },
    BuildRimeSlice {
        source: PathBuf,
        output: PathBuf,
        revision: String,
        declaration: PublicSourceDeclaration,
        config: PublicRimeSliceConfig,
    },
    BuildShortConsensusLayer(Box<ShortConsensusLayerBuildOptions>),
    BuildPhraseLayer(Box<PhraseLayerBuildOptions>),
    BuildExactPhraseLayer(Box<ExactPhraseLayerBuildOptions>),
    MergePublicPackages {
        base: PathBuf,
        overlay: PathBuf,
        output: PathBuf,
        revision: String,
    },
    DiagnosePublicMiss {
        source: PathBuf,
        core_package: PathBuf,
        supplemental_package: PathBuf,
        code: String,
        text: String,
    },
    Compare {
        base_payload: PathBuf,
        challenger_payload: PathBuf,
    },
    ConsensusAudit {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
    },
    ShortRankAudit {
        core_payload: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
    },
    SegmentPenaltyAudit {
        core_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
        sample_limit: usize,
    },
    LengthCoverageAudit {
        base_payload: PathBuf,
        challenger_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
    },
    ShortConsensusAudit {
        source: PathBuf,
        confirmation: PathBuf,
        base_payload: PathBuf,
        per_code_depth: usize,
        entry_limit: usize,
    },
    ExactShortLayerAudit {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        exact_package: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
        supplemental_promotions: usize,
    },
    ExactShortLayerBenchmark {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        exact_package: PathBuf,
        frontier_limit: usize,
        supplemental_promotions: usize,
        exact_promotions: usize,
        candidate_limit: usize,
        sample_limit: usize,
        repetitions: usize,
    },
    ExactShortTsfPreflight {
        core_package: PathBuf,
        supplemental_package: Option<PathBuf>,
        exact_package: PathBuf,
        supplemental_promotions: Option<usize>,
        exact_promotions: usize,
        sample_limit: usize,
        repetitions: usize,
    },
    PopupRenderPreflight {
        repetitions: usize,
    },
    PhraseCoverageAudit {
        source: PathBuf,
        allowlist: PathBuf,
        base_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        entry_limit: usize,
    },
    PhraseLayerAudit {
        source: PathBuf,
        allowlist: PathBuf,
        base_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        small_limit: usize,
        large_limit: usize,
        repetitions: usize,
    },
    ExactPhraseLayerAudit {
        source: PathBuf,
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        entry_limit: usize,
        repetitions: usize,
    },
    ExactPhraseLayerPreflight {
        core_package: PathBuf,
        supplemental_package: PathBuf,
        phrase_package: PathBuf,
        sample_limit: usize,
        repetitions: usize,
    },
    ExactPhraseTsfPreflight {
        core_package: PathBuf,
        supplemental_package: PathBuf,
        phrase_package: PathBuf,
        sample_limit: usize,
        repetitions: usize,
    },
    ExactPhrasePopupPreflight {
        core_package: PathBuf,
        supplemental_package: PathBuf,
        phrase_package: PathBuf,
        sample_limit: usize,
        repetitions: usize,
    },
    LayerAudit {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        frontier_limit: usize,
        exact_promotions: usize,
    },
    LayerBenchmark {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        repetitions: usize,
        exact_promotions: usize,
    },
    LayerCompositionAudit {
        core_payload: PathBuf,
        supplemental_payload: PathBuf,
        corpus: PathBuf,
        fit_corpus: Option<PathBuf>,
        frontier_limit: usize,
        sample_limit: usize,
    },
    StaticContextAudit {
        model: PathBuf,
        core_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
        sample_limit: usize,
        max_order: usize,
    },
    SingleCharacterContextAudit {
        model: PathBuf,
        core_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
        sample_limit: usize,
        max_order: usize,
    },
    SingleCharacterContextValidationAudit {
        model: PathBuf,
        core_payload: PathBuf,
        development_corpus: PathBuf,
        held_out_corpus: PathBuf,
        frontier_limit: usize,
        sample_limit: usize,
        max_order: usize,
    },
    SupplementStatus {
        root: PathBuf,
    },
    SupplementEnable {
        root: PathBuf,
        exact_promotions: usize,
    },
    SupplementDisable {
        root: PathBuf,
    },
    ExactShortStatus {
        root: PathBuf,
    },
    ExactShortReadiness {
        root: PathBuf,
        core_root: PathBuf,
        supplemental_root: Option<PathBuf>,
        package: PathBuf,
        expected_sha256: String,
        exact_promotions: usize,
    },
    ExactShortPrepare {
        root: PathBuf,
        core_root: PathBuf,
        supplemental_root: Option<PathBuf>,
        package: PathBuf,
        expected_sha256: String,
        exact_promotions: usize,
        sample_limit: usize,
        repetitions: usize,
    },
    ExactShortEnable {
        root: PathBuf,
        core_root: PathBuf,
        supplemental_root: Option<PathBuf>,
        package: PathBuf,
        expected_sha256: String,
        exact_promotions: usize,
    },
    ExactShortDisable {
        root: PathBuf,
    },
    Preflight {
        package: PathBuf,
    },
    PackageQuery {
        package: PathBuf,
        code: String,
        limit: usize,
    },
    ExactShortQuery {
        package: PathBuf,
        code: String,
        limit: usize,
    },
    ExactShortBenchmark {
        package: PathBuf,
        code: String,
        repetitions: usize,
    },
    Verify {
        package: PathBuf,
        expected_sha256: String,
    },
    VerifySignature {
        package: PathBuf,
        signature: PathBuf,
        trusted_public_key: String,
    },
    Status {
        root: PathBuf,
    },
    RuntimeCheck {
        root: PathBuf,
    },
    RuntimeQuery {
        root: PathBuf,
        supplemental_root: Option<PathBuf>,
        exact_short_root: Option<PathBuf>,
        code: String,
        limit: usize,
    },
    Adopt {
        root: PathBuf,
        package: PathBuf,
        expected_sha256: String,
    },
    Stage {
        root: PathBuf,
        package: PathBuf,
        expected_sha256: String,
    },
    AdoptSigned {
        root: PathBuf,
        package: PathBuf,
        signature: PathBuf,
        trusted_public_key: String,
    },
    StageSigned {
        root: PathBuf,
        package: PathBuf,
        signature: PathBuf,
        trusted_public_key: String,
    },
    Promote {
        root: PathBuf,
    },
    Rollback {
        root: PathBuf,
    },
}

struct LoadedPackage {
    manifest_text: String,
    payload_text: String,
    provenance_text: String,
    manifest: CandidatePackageManifest,
    provenance: CandidatePackageProvenance,
    snapshot: Arc<CandidateSnapshot>,
    authentication_sha256: String,
}

struct LoadedExactShortPackage {
    provenance: CandidatePackageProvenance,
    catalog: ExactShortWordCatalog,
    authentication_sha256: String,
}

type ExactShortPackageMaterials = (LoadedExactShortPackage, Option<Vec<LexiconEntry>>);

#[derive(Clone, Debug, Eq, PartialEq)]
struct PublicSourceDeclaration {
    id: String,
    license: String,
    url: String,
    sha256: String,
}

#[derive(Debug, Eq, PartialEq)]
struct PhraseLayerBuildOptions {
    source: PathBuf,
    allowlist: PathBuf,
    base_payload: PathBuf,
    output: PathBuf,
    revision: String,
    entry_limit: usize,
    source_declaration: PublicSourceDeclaration,
    allowlist_declaration: PublicSourceDeclaration,
    base_declaration: PublicSourceDeclaration,
}

#[derive(Debug, Eq, PartialEq)]
struct ShortConsensusLayerBuildOptions {
    source: PathBuf,
    confirmation: PathBuf,
    base_payload: PathBuf,
    output: PathBuf,
    revision: String,
    per_code_depth: usize,
    entry_limit: usize,
    source_declaration: PublicSourceDeclaration,
    confirmation_declaration: PublicSourceDeclaration,
    base_declaration: PublicSourceDeclaration,
}

#[derive(Debug, Eq, PartialEq)]
struct ExactPhraseLayerBuildOptions {
    source: PathBuf,
    core_payload: PathBuf,
    supplemental_payload: PathBuf,
    fit_corpus: PathBuf,
    output: PathBuf,
    revision: String,
    entry_limit: usize,
    source_declaration: PublicSourceDeclaration,
    core_declaration: PublicSourceDeclaration,
    supplemental_declaration: PublicSourceDeclaration,
    fit_declaration: PublicSourceDeclaration,
}

struct PreflightSummary {
    revision: String,
    input_keys: usize,
    committed_characters: usize,
}

type PackagePreflight = fn(&LoadedPackage) -> Result<PreflightSummary, Box<dyn std::error::Error>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = match parse_options(std::env::args().skip(1))? {
        Options::Help => {
            print_usage();
            return Ok(());
        }
        Options::Inspect {
            manifest,
            payload,
            provenance,
        } => inspect(&manifest, &payload, &provenance)?,
        Options::Build {
            source,
            output,
            revision,
            declaration,
        } => build_public_package(&source, &output, &revision, &declaration)?,
        Options::BuildRime {
            source,
            output,
            revision,
            declaration,
        } => build_rime_public_package(&source, &output, &revision, &declaration)?,
        Options::BuildRimeSlice {
            source,
            output,
            revision,
            declaration,
            config,
        } => build_rime_slice_public_package(&source, &output, &revision, &declaration, config)?,
        Options::BuildShortConsensusLayer(options) => {
            build_short_consensus_layer_public_package(*options)?
        }
        Options::BuildPhraseLayer(options) => {
            let PhraseLayerBuildOptions {
                source,
                allowlist,
                base_payload,
                output,
                revision,
                entry_limit,
                source_declaration,
                allowlist_declaration,
                base_declaration,
            } = *options;
            build_phrase_layer_public_package(PhraseLayerBuildRequest {
                source: &source,
                allowlist: &allowlist,
                base_payload: &base_payload,
                output: &output,
                revision: &revision,
                entry_limit,
                source_declaration: &source_declaration,
                allowlist_declaration: &allowlist_declaration,
                base_declaration: &base_declaration,
            })?
        }
        Options::BuildExactPhraseLayer(options) => {
            build_exact_phrase_layer_public_package(*options)?
        }
        Options::MergePublicPackages {
            base,
            overlay,
            output,
            revision,
        } => merge_public_packages(&base, &overlay, &output, &revision)?,
        Options::DiagnosePublicMiss {
            source,
            core_package,
            supplemental_package,
            code,
            text,
        } => diagnose_public_miss(&source, &core_package, &supplemental_package, &code, &text)?,
        Options::Compare {
            base_payload,
            challenger_payload,
        } => compare_payloads(&base_payload, &challenger_payload)?,
        Options::ConsensusAudit {
            core_payload,
            supplemental_payload,
            held_out_corpus,
            frontier_limit,
        } => audit_public_consensus(
            &core_payload,
            &supplemental_payload,
            &held_out_corpus,
            frontier_limit,
        )?,
        Options::ShortRankAudit {
            core_payload,
            held_out_corpus,
            frontier_limit,
        } => audit_public_short_ranks(&core_payload, &held_out_corpus, frontier_limit)?,
        Options::SegmentPenaltyAudit {
            core_payload,
            fit_corpus,
            held_out_corpus,
            frontier_limit,
            sample_limit,
        } => audit_public_segment_penalty(
            &core_payload,
            &fit_corpus,
            &held_out_corpus,
            frontier_limit,
            sample_limit,
        )?,
        Options::LengthCoverageAudit {
            base_payload,
            challenger_payload,
            fit_corpus,
            held_out_corpus,
        } => audit_length_coverage(
            &base_payload,
            &challenger_payload,
            &fit_corpus,
            &held_out_corpus,
        )?,
        Options::ShortConsensusAudit {
            source,
            confirmation,
            base_payload,
            per_code_depth,
            entry_limit,
        } => audit_short_word_consensus(
            &source,
            &confirmation,
            &base_payload,
            per_code_depth,
            entry_limit,
        )?,
        Options::ExactShortLayerAudit {
            core_payload,
            supplemental_payload,
            exact_package,
            held_out_corpus,
            frontier_limit,
            supplemental_promotions,
        } => audit_exact_short_layer(ExactShortLayerAuditRequest {
            core_payload: &core_payload,
            supplemental_payload: &supplemental_payload,
            exact_package: &exact_package,
            held_out_corpus: &held_out_corpus,
            frontier_limit,
            supplemental_promotions,
        })?,
        Options::ExactShortLayerBenchmark {
            core_payload,
            supplemental_payload,
            exact_package,
            frontier_limit,
            supplemental_promotions,
            exact_promotions,
            candidate_limit,
            sample_limit,
            repetitions,
        } => benchmark_exact_short_layer(ExactShortLayerBenchmarkRequest {
            core_payload: &core_payload,
            supplemental_payload: &supplemental_payload,
            exact_package: &exact_package,
            frontier_limit,
            supplemental_promotions,
            exact_promotions,
            candidate_limit,
            sample_limit,
            repetitions,
        })?,
        Options::ExactShortTsfPreflight {
            core_package,
            supplemental_package,
            exact_package,
            supplemental_promotions,
            exact_promotions,
            sample_limit,
            repetitions,
        } => preflight_exact_short_tsf(ExactShortTsfPreflightRequest {
            core_package: &core_package,
            supplemental_package: supplemental_package.as_deref(),
            exact_package: &exact_package,
            supplemental_promotions,
            exact_promotions,
            sample_limit,
            repetitions,
        })?,
        Options::PopupRenderPreflight { repetitions } => preflight_popup_rendering(repetitions)?,
        Options::PhraseCoverageAudit {
            source,
            allowlist,
            base_payload,
            fit_corpus,
            held_out_corpus,
            entry_limit,
        } => audit_phrase_coverage(
            &source,
            &allowlist,
            &base_payload,
            &fit_corpus,
            &held_out_corpus,
            entry_limit,
        )?,
        Options::PhraseLayerAudit {
            source,
            allowlist,
            base_payload,
            fit_corpus,
            held_out_corpus,
            small_limit,
            large_limit,
            repetitions,
        } => audit_phrase_layers(PhraseLayerAuditRequest {
            source: &source,
            allowlist: &allowlist,
            base_payload: &base_payload,
            fit_corpus: &fit_corpus,
            held_out_corpus: &held_out_corpus,
            small_limit,
            large_limit,
            repetitions,
        })?,
        Options::ExactPhraseLayerAudit {
            source,
            core_payload,
            supplemental_payload,
            fit_corpus,
            held_out_corpus,
            entry_limit,
            repetitions,
        } => audit_exact_phrase_layer(ExactPhraseLayerAuditRequest {
            source: &source,
            core_payload: &core_payload,
            supplemental_payload: &supplemental_payload,
            fit_corpus: &fit_corpus,
            held_out_corpus: &held_out_corpus,
            entry_limit,
            repetitions,
        })?,
        Options::ExactPhraseLayerPreflight {
            core_package,
            supplemental_package,
            phrase_package,
            sample_limit,
            repetitions,
        } => preflight_exact_phrase_layer(ExactPhraseLayerPreflightRequest {
            core_package: &core_package,
            supplemental_package: &supplemental_package,
            phrase_package: &phrase_package,
            sample_limit,
            repetitions,
        })?,
        Options::ExactPhraseTsfPreflight {
            core_package,
            supplemental_package,
            phrase_package,
            sample_limit,
            repetitions,
        } => preflight_exact_phrase_tsf(ExactPhraseTsfPreflightRequest {
            core_package: &core_package,
            supplemental_package: &supplemental_package,
            phrase_package: &phrase_package,
            sample_limit,
            repetitions,
        })?,
        Options::ExactPhrasePopupPreflight {
            core_package,
            supplemental_package,
            phrase_package,
            sample_limit,
            repetitions,
        } => preflight_exact_phrase_popup(ExactPhrasePopupPreflightRequest {
            core_package: &core_package,
            supplemental_package: &supplemental_package,
            phrase_package: &phrase_package,
            sample_limit,
            repetitions,
        })?,
        Options::LayerAudit {
            core_payload,
            supplemental_payload,
            frontier_limit,
            exact_promotions,
        } => audit_candidate_layers(
            &core_payload,
            &supplemental_payload,
            frontier_limit,
            exact_promotions,
        )?,
        Options::LayerBenchmark {
            core_payload,
            supplemental_payload,
            repetitions,
            exact_promotions,
        } => benchmark_candidate_layers(
            &core_payload,
            &supplemental_payload,
            repetitions,
            exact_promotions,
        )?,
        Options::LayerCompositionAudit {
            core_payload,
            supplemental_payload,
            corpus,
            fit_corpus,
            frontier_limit,
            sample_limit,
        } => audit_candidate_layer_compositions(
            &core_payload,
            &supplemental_payload,
            &corpus,
            fit_corpus.as_deref(),
            frontier_limit,
            sample_limit,
        )?,
        Options::StaticContextAudit {
            model,
            core_payload,
            fit_corpus,
            held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        } => audit_static_context(
            &model,
            &core_payload,
            &fit_corpus,
            &held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        )?,
        Options::SingleCharacterContextAudit {
            model,
            core_payload,
            fit_corpus,
            held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        } => audit_single_character_context(
            &model,
            &core_payload,
            &fit_corpus,
            &held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        )?,
        Options::SingleCharacterContextValidationAudit {
            model,
            core_payload,
            development_corpus,
            held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        } => audit_single_character_context_validation(
            &model,
            &core_payload,
            &development_corpus,
            &held_out_corpus,
            frontier_limit,
            sample_limit,
            max_order,
        )?,
        Options::SupplementStatus { root } => supplement_status(&root)?,
        Options::SupplementEnable {
            root,
            exact_promotions,
        } => supplement_enable(&root, exact_promotions)?,
        Options::SupplementDisable { root } => supplement_disable(&root)?,
        Options::ExactShortStatus { root } => exact_short_status(&root)?,
        Options::ExactShortReadiness {
            root,
            core_root,
            supplemental_root,
            package,
            expected_sha256,
            exact_promotions,
        } => exact_short_readiness(ExactShortReadinessRequest {
            root: &root,
            core_root: &core_root,
            supplemental_root: supplemental_root.as_deref(),
            package: &package,
            expected_sha256: &expected_sha256,
            exact_promotions,
        })?,
        Options::ExactShortPrepare {
            root,
            core_root,
            supplemental_root,
            package,
            expected_sha256,
            exact_promotions,
            sample_limit,
            repetitions,
        } => exact_short_prepare(ExactShortPrepareRequest {
            root: &root,
            core_root: &core_root,
            supplemental_root: supplemental_root.as_deref(),
            package: &package,
            expected_sha256: &expected_sha256,
            exact_promotions,
            sample_limit,
            repetitions,
        })?,
        Options::ExactShortEnable {
            root,
            core_root,
            supplemental_root,
            package,
            expected_sha256,
            exact_promotions,
        } => exact_short_enable(ExactShortEnableRequest {
            root: &root,
            core_root: &core_root,
            supplemental_root: supplemental_root.as_deref(),
            package: &package,
            expected_sha256: &expected_sha256,
            exact_promotions,
        })?,
        Options::ExactShortDisable { root } => exact_short_disable(&root)?,
        Options::Preflight { package } => preflight(&package)?,
        Options::PackageQuery {
            package,
            code,
            limit,
        } => public_package_query(&package, &code, limit)?,
        Options::ExactShortQuery {
            package,
            code,
            limit,
        } => exact_short_package_query(&package, &code, limit)?,
        Options::ExactShortBenchmark {
            package,
            code,
            repetitions,
        } => benchmark_exact_short_package(&package, &code, repetitions)?,
        Options::Verify {
            package,
            expected_sha256,
        } => verify(&package, &expected_sha256)?,
        Options::VerifySignature {
            package,
            signature,
            trusted_public_key,
        } => verify_signature(&package, &signature, &trusted_public_key)?,
        Options::Status { root } => status(&root)?,
        Options::RuntimeCheck { root } => runtime_check(&root)?,
        Options::RuntimeQuery {
            root,
            supplemental_root,
            exact_short_root,
            code,
            limit,
        } => runtime_query(
            &root,
            supplemental_root.as_deref(),
            exact_short_root.as_deref(),
            &code,
            limit,
        )?,
        Options::Adopt {
            root,
            package,
            expected_sha256,
        } => adopt(&root, &package, &expected_sha256)?,
        Options::Stage {
            root,
            package,
            expected_sha256,
        } => stage(&root, &package, &expected_sha256)?,
        Options::AdoptSigned {
            root,
            package,
            signature,
            trusted_public_key,
        } => adopt_signed(&root, &package, &signature, &trusted_public_key)?,
        Options::StageSigned {
            root,
            package,
            signature,
            trusted_public_key,
        } => stage_signed(&root, &package, &signature, &trusted_public_key)?,
        Options::Promote { root } => promote(&root)?,
        Options::Rollback { root } => rollback(&root)?,
    };
    print!("{output}");
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(Options::Help);
    };
    if command == "--help" || command == "-h" || command == "help" {
        reject_extra(arguments)?;
        return Ok(Options::Help);
    }

    match command.as_str() {
        "inspect" => parse_inspect(arguments),
        "build" => parse_build(arguments, false),
        "build-rime" => parse_build(arguments, true),
        "build-rime-slice" => parse_build_rime_slice(arguments),
        "build-short-consensus-layer" => parse_build_short_consensus_layer(arguments),
        "build-phrase-layer" => parse_build_phrase_layer(arguments),
        "build-exact-phrase-layer" => parse_build_exact_phrase_layer(arguments),
        "merge-public-packages" => parse_merge_public_packages(arguments),
        "diagnose-public-miss" => parse_diagnose_public_miss(arguments),
        "compare" => parse_compare(arguments),
        "consensus-audit" => parse_consensus_audit(arguments),
        "short-rank-audit" => parse_short_rank_audit(arguments),
        "segment-penalty-audit" => parse_segment_penalty_audit(arguments),
        "length-coverage-audit" => parse_length_coverage_audit(arguments),
        "short-consensus-audit" => parse_short_consensus_audit(arguments),
        "exact-short-layer-audit" => parse_exact_short_layer_audit(arguments),
        "exact-short-layer-benchmark" => parse_exact_short_layer_benchmark(arguments),
        "exact-short-tsf-preflight" => parse_exact_short_tsf_preflight(arguments),
        "popup-render-preflight" => parse_popup_render_preflight(arguments),
        "phrase-coverage-audit" => parse_phrase_coverage_audit(arguments),
        "phrase-layer-audit" => parse_phrase_layer_audit(arguments),
        "exact-phrase-layer-audit" => parse_exact_phrase_layer_audit(arguments),
        "exact-phrase-layer-preflight" => parse_exact_phrase_layer_preflight(arguments),
        "exact-phrase-tsf-preflight" => parse_exact_phrase_tsf_preflight(arguments),
        "exact-phrase-popup-preflight" => parse_exact_phrase_popup_preflight(arguments),
        "layer-audit" => parse_layer_audit(arguments),
        "layer-benchmark" => parse_layer_benchmark(arguments),
        "layer-composition-audit" => parse_layer_composition_audit(arguments),
        "static-context-audit" => parse_static_context_audit(arguments),
        "single-character-context-audit" => parse_single_character_context_audit(arguments),
        "single-character-context-validation-audit" => {
            parse_single_character_context_validation_audit(arguments)
        }
        "supplement-status" => Ok(Options::SupplementStatus {
            root: parse_root_only(arguments, "supplement-status")?,
        }),
        "supplement-enable" => parse_supplement_enable(arguments),
        "supplement-disable" => Ok(Options::SupplementDisable {
            root: parse_root_only(arguments, "supplement-disable")?,
        }),
        "exact-short-status" => Ok(Options::ExactShortStatus {
            root: parse_root_only(arguments, "exact-short-status")?,
        }),
        "exact-short-readiness" => parse_exact_short_readiness(arguments),
        "exact-short-prepare" => parse_exact_short_prepare(arguments),
        "exact-short-enable" => parse_exact_short_enable(arguments),
        "exact-short-disable" => Ok(Options::ExactShortDisable {
            root: parse_root_only(arguments, "exact-short-disable")?,
        }),
        "preflight" => Ok(Options::Preflight {
            package: parse_package_only(arguments, "preflight")?,
        }),
        "package-query" => parse_package_query(arguments),
        "exact-short-query" => parse_exact_short_query(arguments),
        "exact-short-benchmark" => parse_exact_short_benchmark(arguments),
        "verify" => {
            let (package, expected_sha256) =
                parse_package_and_expected_sha256(arguments, "verify")?;
            Ok(Options::Verify {
                package,
                expected_sha256,
            })
        }
        "verify-signature" => parse_verify_signature(arguments),
        "status" => Ok(Options::Status {
            root: parse_root_only(arguments, "status")?,
        }),
        "runtime-check" => Ok(Options::RuntimeCheck {
            root: parse_root_only(arguments, "runtime-check")?,
        }),
        "runtime-query" => parse_runtime_query(arguments),
        "adopt" => {
            let (root, package, expected_sha256) =
                parse_root_package_and_expected_sha256(arguments, "adopt")?;
            Ok(Options::Adopt {
                root,
                package,
                expected_sha256,
            })
        }
        "stage" => {
            let (root, package, expected_sha256) =
                parse_root_package_and_expected_sha256(arguments, "stage")?;
            Ok(Options::Stage {
                root,
                package,
                expected_sha256,
            })
        }
        "adopt-signed" => {
            let (root, package, signature, trusted_public_key) =
                parse_root_package_and_signature(arguments, "adopt-signed")?;
            Ok(Options::AdoptSigned {
                root,
                package,
                signature,
                trusted_public_key,
            })
        }
        "stage-signed" => {
            let (root, package, signature, trusted_public_key) =
                parse_root_package_and_signature(arguments, "stage-signed")?;
            Ok(Options::StageSigned {
                root,
                package,
                signature,
                trusted_public_key,
            })
        }
        "promote" => Ok(Options::Promote {
            root: parse_root_only(arguments, "promote")?,
        }),
        "rollback" => Ok(Options::Rollback {
            root: parse_root_only(arguments, "rollback")?,
        }),
        _ => Err("unknown candidatectl command; value was suppressed".into()),
    }
}

fn parse_inspect(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut manifest = None;
    let mut payload = None;
    let mut provenance = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--manifest" => set_path(&mut manifest, &mut arguments, "--manifest")?,
            "--payload" => set_path(&mut payload, &mut arguments, "--payload")?,
            "--provenance" => set_path(&mut provenance, &mut arguments, "--provenance")?,
            _ => return Err("unknown inspect argument; value was suppressed".into()),
        }
    }
    Ok(Options::Inspect {
        manifest: manifest.ok_or("inspect requires exactly one --manifest path")?,
        payload: payload.ok_or("inspect requires exactly one --payload path")?,
        provenance: provenance.ok_or("inspect requires exactly one --provenance path")?,
    })
}

fn parse_build(
    mut arguments: impl Iterator<Item = String>,
    rime: bool,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut output = None;
    let mut revision = None;
    let mut source_id = None;
    let mut source_license = None;
    let mut source_url = None;
    let mut source_sha256 = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => {
                if revision.is_some() {
                    return Err("--revision can be given only once".into());
                }
                revision = Some(arguments.next().ok_or("--revision requires a value")?);
            }
            "--source-id" => set_value(&mut source_id, &mut arguments, "--source-id")?,
            "--source-license" => {
                set_value(&mut source_license, &mut arguments, "--source-license")?
            }
            "--source-url" => set_value(&mut source_url, &mut arguments, "--source-url")?,
            "--source-sha256" => set_value(&mut source_sha256, &mut arguments, "--source-sha256")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => return Err("unknown build argument; value was suppressed".into()),
        }
    }
    if !public {
        return Err(
            "build requires explicit --public; private package building is unavailable".into(),
        );
    }
    let source = source.ok_or("build requires exactly one --source path")?;
    let output = output.ok_or("build requires exactly one --output path")?;
    let revision = revision.ok_or("build requires exactly one --revision value")?;
    let declaration = PublicSourceDeclaration {
        id: source_id.ok_or("build requires exactly one --source-id value")?,
        license: source_license.ok_or("build requires exactly one --source-license value")?,
        url: source_url.ok_or("build requires exactly one --source-url value")?,
        sha256: source_sha256.ok_or("build requires exactly one --source-sha256 value")?,
    };
    Ok(if rime {
        Options::BuildRime {
            source,
            output,
            revision,
            declaration,
        }
    } else {
        Options::Build {
            source,
            output,
            revision,
            declaration,
        }
    })
}

fn parse_build_rime_slice(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut output = None;
    let mut revision = None;
    let mut source_id = None;
    let mut source_license = None;
    let mut source_url = None;
    let mut source_sha256 = None;
    let mut max_entries = None;
    let mut frequency_frontier_entries = None;
    let mut two_character_coverage_depth = None;
    let mut three_character_coverage_entries = None;
    let mut four_character_coverage_entries = None;
    let mut max_text_characters = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => set_value(&mut revision, &mut arguments, "--revision")?,
            "--source-id" => set_value(&mut source_id, &mut arguments, "--source-id")?,
            "--source-license" => {
                set_value(&mut source_license, &mut arguments, "--source-license")?
            }
            "--source-url" => set_value(&mut source_url, &mut arguments, "--source-url")?,
            "--source-sha256" => set_value(&mut source_sha256, &mut arguments, "--source-sha256")?,
            "--max-entries" => set_usize(&mut max_entries, &mut arguments, "--max-entries")?,
            "--frequency-frontier-entries" => set_usize(
                &mut frequency_frontier_entries,
                &mut arguments,
                "--frequency-frontier-entries",
            )?,
            "--two-character-coverage-depth" => set_usize(
                &mut two_character_coverage_depth,
                &mut arguments,
                "--two-character-coverage-depth",
            )?,
            "--three-character-coverage-entries" => set_usize(
                &mut three_character_coverage_entries,
                &mut arguments,
                "--three-character-coverage-entries",
            )?,
            "--four-character-coverage-entries" => set_usize(
                &mut four_character_coverage_entries,
                &mut arguments,
                "--four-character-coverage-entries",
            )?,
            "--max-text-characters" => set_usize(
                &mut max_text_characters,
                &mut arguments,
                "--max-text-characters",
            )?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => return Err("unknown build-rime-slice argument; value was suppressed".into()),
        }
    }
    if !public {
        return Err("build-rime-slice requires explicit --public".into());
    }
    let max_entries = max_entries.ok_or("build-rime-slice requires --max-entries")?;
    let config = PublicRimeSliceConfig {
        max_entries,
        frequency_frontier_entries: frequency_frontier_entries.unwrap_or(max_entries),
        two_character_coverage_depth: two_character_coverage_depth.unwrap_or(1),
        three_character_coverage_entries: three_character_coverage_entries.unwrap_or(0),
        four_character_coverage_entries: four_character_coverage_entries.unwrap_or(0),
        max_text_characters: max_text_characters
            .ok_or("build-rime-slice requires --max-text-characters")?,
    };
    if config.max_entries == 0 || config.max_entries > MAX_PUBLIC_RIME_SLICE_ENTRIES {
        return Err("build-rime-slice --max-entries is outside the fixed bound".into());
    }
    if config.max_text_characters == 0
        || config.max_text_characters > MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS
    {
        return Err("build-rime-slice --max-text-characters is outside the fixed bound".into());
    }
    if config.frequency_frontier_entries == 0
        || config.frequency_frontier_entries > config.max_entries
    {
        return Err(
            "build-rime-slice --frequency-frontier-entries must be within --max-entries".into(),
        );
    }
    if config.two_character_coverage_depth == 0
        || config.two_character_coverage_depth > MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH
    {
        return Err(
            "build-rime-slice --two-character-coverage-depth is outside the fixed bound".into(),
        );
    }
    if config
        .three_character_coverage_entries
        .saturating_add(config.four_character_coverage_entries)
        > config
            .max_entries
            .saturating_sub(config.frequency_frontier_entries)
    {
        return Err(
            "build-rime-slice three/four-character coverage exceeds post-frontier capacity".into(),
        );
    }
    Ok(Options::BuildRimeSlice {
        source: source.ok_or("build-rime-slice requires --source")?,
        output: output.ok_or("build-rime-slice requires --output")?,
        revision: revision.ok_or("build-rime-slice requires --revision")?,
        declaration: PublicSourceDeclaration {
            id: source_id.ok_or("build-rime-slice requires --source-id")?,
            license: source_license.ok_or("build-rime-slice requires --source-license")?,
            url: source_url.ok_or("build-rime-slice requires --source-url")?,
            sha256: source_sha256.ok_or("build-rime-slice requires --source-sha256")?,
        },
        config,
    })
}

fn parse_build_short_consensus_layer(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut confirmation = None;
    let mut base_payload = None;
    let mut output = None;
    let mut revision = None;
    let mut per_code_depth = None;
    let mut entry_limit = None;
    let mut source_id = None;
    let mut source_license = None;
    let mut source_url = None;
    let mut source_sha256 = None;
    let mut confirmation_id = None;
    let mut confirmation_license = None;
    let mut confirmation_url = None;
    let mut confirmation_sha256 = None;
    let mut base_id = None;
    let mut base_license = None;
    let mut base_url = None;
    let mut base_sha256 = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--confirmation" => set_path(&mut confirmation, &mut arguments, "--confirmation")?,
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => set_value(&mut revision, &mut arguments, "--revision")?,
            "--per-code-depth" => {
                set_usize(&mut per_code_depth, &mut arguments, "--per-code-depth")?
            }
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            "--source-id" => set_value(&mut source_id, &mut arguments, "--source-id")?,
            "--source-license" => {
                set_value(&mut source_license, &mut arguments, "--source-license")?
            }
            "--source-url" => set_value(&mut source_url, &mut arguments, "--source-url")?,
            "--source-sha256" => set_value(&mut source_sha256, &mut arguments, "--source-sha256")?,
            "--confirmation-id" => {
                set_value(&mut confirmation_id, &mut arguments, "--confirmation-id")?
            }
            "--confirmation-license" => set_value(
                &mut confirmation_license,
                &mut arguments,
                "--confirmation-license",
            )?,
            "--confirmation-url" => {
                set_value(&mut confirmation_url, &mut arguments, "--confirmation-url")?
            }
            "--confirmation-sha256" => set_value(
                &mut confirmation_sha256,
                &mut arguments,
                "--confirmation-sha256",
            )?,
            "--base-id" => set_value(&mut base_id, &mut arguments, "--base-id")?,
            "--base-license" => set_value(&mut base_license, &mut arguments, "--base-license")?,
            "--base-url" => set_value(&mut base_url, &mut arguments, "--base-url")?,
            "--base-sha256" => set_value(&mut base_sha256, &mut arguments, "--base-sha256")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => {
                return Err(
                    "unknown build-short-consensus-layer argument; value was suppressed".into(),
                );
            }
        }
    }
    if !public {
        return Err("build-short-consensus-layer requires explicit --public".into());
    }
    let per_code_depth =
        per_code_depth.ok_or("build-short-consensus-layer requires --per-code-depth")?;
    if !(1..=MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH).contains(&per_code_depth) {
        return Err(
            "build-short-consensus-layer --per-code-depth is outside the fixed bound".into(),
        );
    }
    let entry_limit = entry_limit.ok_or("build-short-consensus-layer requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_SHORT_WORD_CONSENSUS_ENTRIES).contains(&entry_limit) {
        return Err("build-short-consensus-layer --entry-limit is outside the fixed bound".into());
    }
    Ok(Options::BuildShortConsensusLayer(Box::new(
        ShortConsensusLayerBuildOptions {
            source: source.ok_or("build-short-consensus-layer requires --source")?,
            confirmation: confirmation
                .ok_or("build-short-consensus-layer requires --confirmation")?,
            base_payload: base_payload
                .ok_or("build-short-consensus-layer requires --base-payload")?,
            output: output.ok_or("build-short-consensus-layer requires --output")?,
            revision: revision.ok_or("build-short-consensus-layer requires --revision")?,
            per_code_depth,
            entry_limit,
            source_declaration: PublicSourceDeclaration {
                id: source_id.ok_or("build-short-consensus-layer requires --source-id")?,
                license: source_license
                    .ok_or("build-short-consensus-layer requires --source-license")?,
                url: source_url.ok_or("build-short-consensus-layer requires --source-url")?,
                sha256: source_sha256
                    .ok_or("build-short-consensus-layer requires --source-sha256")?,
            },
            confirmation_declaration: PublicSourceDeclaration {
                id: confirmation_id
                    .ok_or("build-short-consensus-layer requires --confirmation-id")?,
                license: confirmation_license
                    .ok_or("build-short-consensus-layer requires --confirmation-license")?,
                url: confirmation_url
                    .ok_or("build-short-consensus-layer requires --confirmation-url")?,
                sha256: confirmation_sha256
                    .ok_or("build-short-consensus-layer requires --confirmation-sha256")?,
            },
            base_declaration: PublicSourceDeclaration {
                id: base_id.ok_or("build-short-consensus-layer requires --base-id")?,
                license: base_license
                    .ok_or("build-short-consensus-layer requires --base-license")?,
                url: base_url.ok_or("build-short-consensus-layer requires --base-url")?,
                sha256: base_sha256.ok_or("build-short-consensus-layer requires --base-sha256")?,
            },
        },
    )))
}

fn parse_build_phrase_layer(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut allowlist = None;
    let mut base_payload = None;
    let mut output = None;
    let mut revision = None;
    let mut entry_limit = None;
    let mut source_id = None;
    let mut source_license = None;
    let mut source_url = None;
    let mut source_sha256 = None;
    let mut allowlist_id = None;
    let mut allowlist_license = None;
    let mut allowlist_url = None;
    let mut allowlist_sha256 = None;
    let mut base_id = None;
    let mut base_license = None;
    let mut base_url = None;
    let mut base_sha256 = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--allowlist" => set_path(&mut allowlist, &mut arguments, "--allowlist")?,
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => set_value(&mut revision, &mut arguments, "--revision")?,
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            "--source-id" => set_value(&mut source_id, &mut arguments, "--source-id")?,
            "--source-license" => {
                set_value(&mut source_license, &mut arguments, "--source-license")?
            }
            "--source-url" => set_value(&mut source_url, &mut arguments, "--source-url")?,
            "--source-sha256" => set_value(&mut source_sha256, &mut arguments, "--source-sha256")?,
            "--allowlist-id" => set_value(&mut allowlist_id, &mut arguments, "--allowlist-id")?,
            "--allowlist-license" => set_value(
                &mut allowlist_license,
                &mut arguments,
                "--allowlist-license",
            )?,
            "--allowlist-url" => set_value(&mut allowlist_url, &mut arguments, "--allowlist-url")?,
            "--allowlist-sha256" => {
                set_value(&mut allowlist_sha256, &mut arguments, "--allowlist-sha256")?
            }
            "--base-id" => set_value(&mut base_id, &mut arguments, "--base-id")?,
            "--base-license" => set_value(&mut base_license, &mut arguments, "--base-license")?,
            "--base-url" => set_value(&mut base_url, &mut arguments, "--base-url")?,
            "--base-sha256" => set_value(&mut base_sha256, &mut arguments, "--base-sha256")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => {
                return Err("unknown build-phrase-layer argument; value was suppressed".into());
            }
        }
    }
    if !public {
        return Err("build-phrase-layer requires explicit --public".into());
    }
    let entry_limit = entry_limit.ok_or("build-phrase-layer requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES).contains(&entry_limit) {
        return Err("build-phrase-layer --entry-limit is outside the fixed bound".into());
    }
    Ok(Options::BuildPhraseLayer(Box::new(
        PhraseLayerBuildOptions {
            source: source.ok_or("build-phrase-layer requires --source")?,
            allowlist: allowlist.ok_or("build-phrase-layer requires --allowlist")?,
            base_payload: base_payload.ok_or("build-phrase-layer requires --base-payload")?,
            output: output.ok_or("build-phrase-layer requires --output")?,
            revision: revision.ok_or("build-phrase-layer requires --revision")?,
            entry_limit,
            source_declaration: PublicSourceDeclaration {
                id: source_id.ok_or("build-phrase-layer requires --source-id")?,
                license: source_license.ok_or("build-phrase-layer requires --source-license")?,
                url: source_url.ok_or("build-phrase-layer requires --source-url")?,
                sha256: source_sha256.ok_or("build-phrase-layer requires --source-sha256")?,
            },
            allowlist_declaration: PublicSourceDeclaration {
                id: allowlist_id.ok_or("build-phrase-layer requires --allowlist-id")?,
                license: allowlist_license
                    .ok_or("build-phrase-layer requires --allowlist-license")?,
                url: allowlist_url.ok_or("build-phrase-layer requires --allowlist-url")?,
                sha256: allowlist_sha256.ok_or("build-phrase-layer requires --allowlist-sha256")?,
            },
            base_declaration: PublicSourceDeclaration {
                id: base_id.ok_or("build-phrase-layer requires --base-id")?,
                license: base_license.ok_or("build-phrase-layer requires --base-license")?,
                url: base_url.ok_or("build-phrase-layer requires --base-url")?,
                sha256: base_sha256.ok_or("build-phrase-layer requires --base-sha256")?,
            },
        },
    )))
}

fn parse_build_exact_phrase_layer(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut fit_corpus = None;
    let mut output = None;
    let mut revision = None;
    let mut entry_limit = None;
    let mut source_id = None;
    let mut source_license = None;
    let mut source_url = None;
    let mut source_sha256 = None;
    let mut core_id = None;
    let mut core_license = None;
    let mut core_url = None;
    let mut core_sha256 = None;
    let mut supplemental_id = None;
    let mut supplemental_license = None;
    let mut supplemental_url = None;
    let mut supplemental_sha256 = None;
    let mut fit_id = None;
    let mut fit_license = None;
    let mut fit_url = None;
    let mut fit_sha256 = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => set_value(&mut revision, &mut arguments, "--revision")?,
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            "--source-id" => set_value(&mut source_id, &mut arguments, "--source-id")?,
            "--source-license" => {
                set_value(&mut source_license, &mut arguments, "--source-license")?
            }
            "--source-url" => set_value(&mut source_url, &mut arguments, "--source-url")?,
            "--source-sha256" => set_value(&mut source_sha256, &mut arguments, "--source-sha256")?,
            "--core-id" => set_value(&mut core_id, &mut arguments, "--core-id")?,
            "--core-license" => set_value(&mut core_license, &mut arguments, "--core-license")?,
            "--core-url" => set_value(&mut core_url, &mut arguments, "--core-url")?,
            "--core-sha256" => set_value(&mut core_sha256, &mut arguments, "--core-sha256")?,
            "--supplemental-id" => {
                set_value(&mut supplemental_id, &mut arguments, "--supplemental-id")?
            }
            "--supplemental-license" => set_value(
                &mut supplemental_license,
                &mut arguments,
                "--supplemental-license",
            )?,
            "--supplemental-url" => {
                set_value(&mut supplemental_url, &mut arguments, "--supplemental-url")?
            }
            "--supplemental-sha256" => set_value(
                &mut supplemental_sha256,
                &mut arguments,
                "--supplemental-sha256",
            )?,
            "--fit-id" => set_value(&mut fit_id, &mut arguments, "--fit-id")?,
            "--fit-license" => set_value(&mut fit_license, &mut arguments, "--fit-license")?,
            "--fit-url" => set_value(&mut fit_url, &mut arguments, "--fit-url")?,
            "--fit-sha256" => set_value(&mut fit_sha256, &mut arguments, "--fit-sha256")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => {
                return Err(
                    "unknown build-exact-phrase-layer argument; value was suppressed".into(),
                );
            }
        }
    }
    if !public {
        return Err("build-exact-phrase-layer requires explicit --public".into());
    }
    let entry_limit = entry_limit.ok_or("build-exact-phrase-layer requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES).contains(&entry_limit) {
        return Err("build-exact-phrase-layer --entry-limit is outside the fixed bound".into());
    }
    Ok(Options::BuildExactPhraseLayer(Box::new(
        ExactPhraseLayerBuildOptions {
            source: source.ok_or("build-exact-phrase-layer requires --source")?,
            core_payload: core_payload.ok_or("build-exact-phrase-layer requires --core-payload")?,
            supplemental_payload: supplemental_payload
                .ok_or("build-exact-phrase-layer requires --supplemental-payload")?,
            fit_corpus: fit_corpus.ok_or("build-exact-phrase-layer requires --fit-corpus")?,
            output: output.ok_or("build-exact-phrase-layer requires --output")?,
            revision: revision.ok_or("build-exact-phrase-layer requires --revision")?,
            entry_limit,
            source_declaration: PublicSourceDeclaration {
                id: source_id.ok_or("build-exact-phrase-layer requires --source-id")?,
                license: source_license
                    .ok_or("build-exact-phrase-layer requires --source-license")?,
                url: source_url.ok_or("build-exact-phrase-layer requires --source-url")?,
                sha256: source_sha256.ok_or("build-exact-phrase-layer requires --source-sha256")?,
            },
            core_declaration: PublicSourceDeclaration {
                id: core_id.ok_or("build-exact-phrase-layer requires --core-id")?,
                license: core_license.ok_or("build-exact-phrase-layer requires --core-license")?,
                url: core_url.ok_or("build-exact-phrase-layer requires --core-url")?,
                sha256: core_sha256.ok_or("build-exact-phrase-layer requires --core-sha256")?,
            },
            supplemental_declaration: PublicSourceDeclaration {
                id: supplemental_id.ok_or("build-exact-phrase-layer requires --supplemental-id")?,
                license: supplemental_license
                    .ok_or("build-exact-phrase-layer requires --supplemental-license")?,
                url: supplemental_url
                    .ok_or("build-exact-phrase-layer requires --supplemental-url")?,
                sha256: supplemental_sha256
                    .ok_or("build-exact-phrase-layer requires --supplemental-sha256")?,
            },
            fit_declaration: PublicSourceDeclaration {
                id: fit_id.ok_or("build-exact-phrase-layer requires --fit-id")?,
                license: fit_license.ok_or("build-exact-phrase-layer requires --fit-license")?,
                url: fit_url.ok_or("build-exact-phrase-layer requires --fit-url")?,
                sha256: fit_sha256.ok_or("build-exact-phrase-layer requires --fit-sha256")?,
            },
        },
    )))
}

fn parse_merge_public_packages(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut base = None;
    let mut overlay = None;
    let mut output = None;
    let mut revision = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--base" => set_path(&mut base, &mut arguments, "--base")?,
            "--overlay" => set_path(&mut overlay, &mut arguments, "--overlay")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => set_value(&mut revision, &mut arguments, "--revision")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => {
                return Err("unknown merge-public-packages argument; value was suppressed".into());
            }
        }
    }
    if !public {
        return Err("merge-public-packages requires explicit --public".into());
    }
    Ok(Options::MergePublicPackages {
        base: base.ok_or("merge-public-packages requires --base")?,
        overlay: overlay.ok_or("merge-public-packages requires --overlay")?,
        output: output.ok_or("merge-public-packages requires --output")?,
        revision: revision.ok_or("merge-public-packages requires --revision")?,
    })
}

fn parse_diagnose_public_miss(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut core_package = None;
    let mut supplemental_package = None;
    let mut code = None;
    let mut text = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--core-package" => set_path(&mut core_package, &mut arguments, "--core-package")?,
            "--supplemental-package" => set_path(
                &mut supplemental_package,
                &mut arguments,
                "--supplemental-package",
            )?,
            "--code" => set_value(&mut code, &mut arguments, "--code")?,
            "--text" => set_value(&mut text, &mut arguments, "--text")?,
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => return Err("unknown diagnose-public-miss argument; value was suppressed".into()),
        }
    }
    if !public {
        return Err("diagnose-public-miss requires explicit --public".into());
    }
    let code = code.ok_or("diagnose-public-miss requires --code")?;
    if code.is_empty() || code.len() > 24 || !code.as_bytes().iter().all(u8::is_ascii_lowercase) {
        return Err("diagnose-public-miss --code must be 1..24 lowercase ASCII letters".into());
    }
    let text = text.ok_or("diagnose-public-miss requires --text")?;
    if text.is_empty() || text.len() > 48 {
        return Err("diagnose-public-miss --text is outside the fixed UTF-8 byte bound".into());
    }
    Ok(Options::DiagnosePublicMiss {
        source: source.ok_or("diagnose-public-miss requires --source")?,
        core_package: core_package.ok_or("diagnose-public-miss requires --core-package")?,
        supplemental_package: supplemental_package
            .ok_or("diagnose-public-miss requires --supplemental-package")?,
        code,
        text,
    })
}

fn parse_compare(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut base_payload = None;
    let mut challenger_payload = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--challenger-payload" => set_path(
                &mut challenger_payload,
                &mut arguments,
                "--challenger-payload",
            )?,
            _ => return Err("unknown compare argument; value was suppressed".into()),
        }
    }
    Ok(Options::Compare {
        base_payload: base_payload.ok_or("compare requires --base-payload")?,
        challenger_payload: challenger_payload.ok_or("compare requires --challenger-payload")?,
    })
}

fn parse_consensus_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            _ => return Err("unknown consensus-audit argument; value was suppressed".into()),
        }
    }
    let frontier_limit = frontier_limit.ok_or("consensus-audit requires --frontier-limit")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("consensus-audit --frontier-limit is outside the fixed bound".into());
    }
    Ok(Options::ConsensusAudit {
        core_payload: core_payload.ok_or("consensus-audit requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("consensus-audit requires --supplemental-payload")?,
        held_out_corpus: held_out_corpus.ok_or("consensus-audit requires --held-out-corpus")?,
        frontier_limit,
    })
}

fn parse_short_rank_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            _ => return Err("unknown short-rank-audit argument; value was suppressed".into()),
        }
    }
    let frontier_limit = frontier_limit.ok_or("short-rank-audit requires --frontier-limit")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("short-rank-audit --frontier-limit is outside the fixed bound".into());
    }
    Ok(Options::ShortRankAudit {
        core_payload: core_payload.ok_or("short-rank-audit requires --core-payload")?,
        held_out_corpus: held_out_corpus.ok_or("short-rank-audit requires --held-out-corpus")?,
        frontier_limit,
    })
}

fn parse_segment_penalty_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    let mut sample_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            _ => {
                return Err("unknown segment-penalty-audit argument; value was suppressed".into());
            }
        }
    }
    let frontier_limit = frontier_limit.ok_or("segment-penalty-audit requires --frontier-limit")?;
    if !(1..=10).contains(&frontier_limit) {
        return Err("segment-penalty-audit --frontier-limit must be 1..10".into());
    }
    let sample_limit = sample_limit.ok_or("segment-penalty-audit requires --sample-limit")?;
    if !(1..=512).contains(&sample_limit) {
        return Err("segment-penalty-audit --sample-limit must be 1..512".into());
    }
    Ok(Options::SegmentPenaltyAudit {
        core_payload: core_payload.ok_or("segment-penalty-audit requires --core-payload")?,
        fit_corpus: fit_corpus.ok_or("segment-penalty-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("segment-penalty-audit requires --held-out-corpus")?,
        frontier_limit,
        sample_limit,
    })
}

fn parse_length_coverage_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut base_payload = None;
    let mut challenger_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--challenger-payload" => set_path(
                &mut challenger_payload,
                &mut arguments,
                "--challenger-payload",
            )?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            _ => {
                return Err("unknown length-coverage-audit argument; value was suppressed".into());
            }
        }
    }
    Ok(Options::LengthCoverageAudit {
        base_payload: base_payload.ok_or("length-coverage-audit requires --base-payload")?,
        challenger_payload: challenger_payload
            .ok_or("length-coverage-audit requires --challenger-payload")?,
        fit_corpus: fit_corpus.ok_or("length-coverage-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("length-coverage-audit requires --held-out-corpus")?,
    })
}

fn parse_short_consensus_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut confirmation = None;
    let mut base_payload = None;
    let mut per_code_depth = None;
    let mut entry_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--confirmation" => set_path(&mut confirmation, &mut arguments, "--confirmation")?,
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--per-code-depth" => {
                set_usize(&mut per_code_depth, &mut arguments, "--per-code-depth")?
            }
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            _ => {
                return Err("unknown short-consensus-audit argument; value was suppressed".into());
            }
        }
    }
    let per_code_depth = per_code_depth.ok_or("short-consensus-audit requires --per-code-depth")?;
    if !(1..=MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH).contains(&per_code_depth) {
        return Err("short-consensus-audit --per-code-depth is outside the fixed bound".into());
    }
    let entry_limit = entry_limit.ok_or("short-consensus-audit requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_SHORT_WORD_CONSENSUS_ENTRIES).contains(&entry_limit) {
        return Err("short-consensus-audit --entry-limit is outside the fixed bound".into());
    }
    Ok(Options::ShortConsensusAudit {
        source: source.ok_or("short-consensus-audit requires --source")?,
        confirmation: confirmation.ok_or("short-consensus-audit requires --confirmation")?,
        base_payload: base_payload.ok_or("short-consensus-audit requires --base-payload")?,
        per_code_depth,
        entry_limit,
    })
}

fn parse_exact_short_layer_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut exact_package = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    let mut supplemental_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--exact-package" => set_path(&mut exact_package, &mut arguments, "--exact-package")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--supplemental-promotions" => set_usize(
                &mut supplemental_promotions,
                &mut arguments,
                "--supplemental-promotions",
            )?,
            _ => {
                return Err(
                    "unknown exact-short-layer-audit argument; value was suppressed".into(),
                );
            }
        }
    }
    let frontier_limit =
        frontier_limit.ok_or("exact-short-layer-audit requires --frontier-limit")?;
    if !(2..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("exact-short-layer-audit --frontier-limit must be within 2..50".into());
    }
    let supplemental_promotions = supplemental_promotions
        .ok_or("exact-short-layer-audit requires --supplemental-promotions")?;
    if supplemental_promotions > frontier_limit {
        return Err(
            "exact-short-layer-audit --supplemental-promotions exceeds the frontier".into(),
        );
    }
    Ok(Options::ExactShortLayerAudit {
        core_payload: core_payload.ok_or("exact-short-layer-audit requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("exact-short-layer-audit requires --supplemental-payload")?,
        exact_package: exact_package.ok_or("exact-short-layer-audit requires --exact-package")?,
        held_out_corpus: held_out_corpus
            .ok_or("exact-short-layer-audit requires --held-out-corpus")?,
        frontier_limit,
        supplemental_promotions,
    })
}

fn parse_exact_short_layer_benchmark(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut exact_package = None;
    let mut frontier_limit = None;
    let mut supplemental_promotions = None;
    let mut exact_promotions = None;
    let mut candidate_limit = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--exact-package" => set_path(&mut exact_package, &mut arguments, "--exact-package")?,
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--supplemental-promotions" => set_usize(
                &mut supplemental_promotions,
                &mut arguments,
                "--supplemental-promotions",
            )?,
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            "--candidate-limit" => {
                set_usize(&mut candidate_limit, &mut arguments, "--candidate-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-short-layer-benchmark argument; value was suppressed".into(),
                );
            }
        }
    }
    let frontier_limit =
        frontier_limit.ok_or("exact-short-layer-benchmark requires --frontier-limit")?;
    if !(2..=MAX_CANDIDATE_SNAPSHOT_RANK / 2).contains(&frontier_limit) {
        return Err("exact-short-layer-benchmark --frontier-limit must be within 2..25".into());
    }
    let supplemental_promotions = supplemental_promotions
        .ok_or("exact-short-layer-benchmark requires --supplemental-promotions")?;
    if supplemental_promotions > frontier_limit {
        return Err(
            "exact-short-layer-benchmark --supplemental-promotions exceeds the frontier".into(),
        );
    }
    let exact_promotions =
        exact_promotions.ok_or("exact-short-layer-benchmark requires --exact-promotions")?;
    if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
        return Err(
            "exact-short-layer-benchmark --exact-promotions is outside the fixed bound".into(),
        );
    }
    let candidate_limit =
        candidate_limit.ok_or("exact-short-layer-benchmark requires --candidate-limit")?;
    if !(frontier_limit.saturating_mul(2)..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&candidate_limit)
    {
        return Err(
            "exact-short-layer-benchmark --candidate-limit must contain at least two complete pages and stay within 50"
                .into(),
        );
    }
    let sample_limit = sample_limit.ok_or("exact-short-layer-benchmark requires --sample-limit")?;
    if !(8..=2048).contains(&sample_limit) {
        return Err("exact-short-layer-benchmark --sample-limit must be within 8..2048".into());
    }
    let repetitions = repetitions.ok_or("exact-short-layer-benchmark requires --repetitions")?;
    if !(1..=100).contains(&repetitions) {
        return Err("exact-short-layer-benchmark --repetitions must be within 1..100".into());
    }
    Ok(Options::ExactShortLayerBenchmark {
        core_payload: core_payload.ok_or("exact-short-layer-benchmark requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("exact-short-layer-benchmark requires --supplemental-payload")?,
        exact_package: exact_package
            .ok_or("exact-short-layer-benchmark requires --exact-package")?,
        frontier_limit,
        supplemental_promotions,
        exact_promotions,
        candidate_limit,
        sample_limit,
        repetitions,
    })
}

fn parse_exact_short_tsf_preflight(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_package = None;
    let mut supplemental_package = None;
    let mut exact_package = None;
    let mut supplemental_promotions = None;
    let mut exact_promotions = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-package" => set_path(&mut core_package, &mut arguments, "--core-package")?,
            "--supplemental-package" => set_path(
                &mut supplemental_package,
                &mut arguments,
                "--supplemental-package",
            )?,
            "--exact-package" => set_path(&mut exact_package, &mut arguments, "--exact-package")?,
            "--supplemental-promotions" => set_usize(
                &mut supplemental_promotions,
                &mut arguments,
                "--supplemental-promotions",
            )?,
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-short-tsf-preflight argument; value was suppressed".into(),
                );
            }
        }
    }
    if supplemental_package.is_some() != supplemental_promotions.is_some() {
        return Err(
            "exact-short-tsf-preflight requires supplemental package and promotion bound together"
                .into(),
        );
    }
    if supplemental_promotions
        .is_some_and(|promotions| !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&promotions))
    {
        return Err(
            "exact-short-tsf-preflight --supplemental-promotions is outside the fixed bound".into(),
        );
    }
    let exact_promotions =
        exact_promotions.ok_or("exact-short-tsf-preflight requires --exact-promotions")?;
    if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
        return Err(
            "exact-short-tsf-preflight --exact-promotions is outside the fixed bound".into(),
        );
    }
    let sample_limit = sample_limit.ok_or("exact-short-tsf-preflight requires --sample-limit")?;
    if !(1..=32).contains(&sample_limit) {
        return Err("exact-short-tsf-preflight --sample-limit must be within 1..32".into());
    }
    let repetitions = repetitions.ok_or("exact-short-tsf-preflight requires --repetitions")?;
    if !(1..=20).contains(&repetitions) || sample_limit.saturating_mul(repetitions) > 640 {
        return Err("exact-short-tsf-preflight workload exceeds the fixed 640-probe bound".into());
    }
    Ok(Options::ExactShortTsfPreflight {
        core_package: core_package.ok_or("exact-short-tsf-preflight requires --core-package")?,
        supplemental_package,
        exact_package: exact_package.ok_or("exact-short-tsf-preflight requires --exact-package")?,
        supplemental_promotions,
        exact_promotions,
        sample_limit,
        repetitions,
    })
}

fn parse_popup_render_preflight(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err("unknown popup-render-preflight argument; value was suppressed".into());
            }
        }
    }
    let repetitions = repetitions.ok_or("popup-render-preflight requires --repetitions")?;
    if !(1..=20).contains(&repetitions) {
        return Err("popup-render-preflight --repetitions must be within 1..20".into());
    }
    Ok(Options::PopupRenderPreflight { repetitions })
}

fn parse_phrase_coverage_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut allowlist = None;
    let mut base_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut entry_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--allowlist" => set_path(&mut allowlist, &mut arguments, "--allowlist")?,
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            _ => return Err("unknown phrase-coverage-audit argument; value was suppressed".into()),
        }
    }
    let entry_limit = entry_limit.ok_or("phrase-coverage-audit requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES).contains(&entry_limit) {
        return Err("phrase-coverage-audit --entry-limit is outside the fixed bound".into());
    }
    Ok(Options::PhraseCoverageAudit {
        source: source.ok_or("phrase-coverage-audit requires --source")?,
        allowlist: allowlist.ok_or("phrase-coverage-audit requires --allowlist")?,
        base_payload: base_payload.ok_or("phrase-coverage-audit requires --base-payload")?,
        fit_corpus: fit_corpus.ok_or("phrase-coverage-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("phrase-coverage-audit requires --held-out-corpus")?,
        entry_limit,
    })
}

fn parse_phrase_layer_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut allowlist = None;
    let mut base_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut small_limit = None;
    let mut large_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--allowlist" => set_path(&mut allowlist, &mut arguments, "--allowlist")?,
            "--base-payload" => set_path(&mut base_payload, &mut arguments, "--base-payload")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--small-limit" => set_usize(&mut small_limit, &mut arguments, "--small-limit")?,
            "--large-limit" => set_usize(&mut large_limit, &mut arguments, "--large-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => return Err("unknown phrase-layer-audit argument; value was suppressed".into()),
        }
    }
    let small_limit = small_limit.ok_or("phrase-layer-audit requires --small-limit")?;
    let large_limit = large_limit.ok_or("phrase-layer-audit requires --large-limit")?;
    if small_limit == 0
        || small_limit >= large_limit
        || large_limit > MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES
    {
        return Err("phrase-layer-audit limits must satisfy 1 <= small < large <= 50000".into());
    }
    let repetitions = repetitions.ok_or("phrase-layer-audit requires --repetitions")?;
    if !(1..=100).contains(&repetitions) {
        return Err("phrase-layer-audit --repetitions is outside the fixed bound".into());
    }
    Ok(Options::PhraseLayerAudit {
        source: source.ok_or("phrase-layer-audit requires --source")?,
        allowlist: allowlist.ok_or("phrase-layer-audit requires --allowlist")?,
        base_payload: base_payload.ok_or("phrase-layer-audit requires --base-payload")?,
        fit_corpus: fit_corpus.ok_or("phrase-layer-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus.ok_or("phrase-layer-audit requires --held-out-corpus")?,
        small_limit,
        large_limit,
        repetitions,
    })
}

fn parse_exact_phrase_layer_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut entry_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--entry-limit" => set_usize(&mut entry_limit, &mut arguments, "--entry-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-phrase-layer-audit argument; value was suppressed".into(),
                );
            }
        }
    }
    let entry_limit = entry_limit.ok_or("exact-phrase-layer-audit requires --entry-limit")?;
    if !(1..=MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES).contains(&entry_limit) {
        return Err("exact-phrase-layer-audit --entry-limit is outside the fixed bound".into());
    }
    let repetitions = repetitions.ok_or("exact-phrase-layer-audit requires --repetitions")?;
    if !(1..=100).contains(&repetitions) {
        return Err("exact-phrase-layer-audit --repetitions is outside the fixed bound".into());
    }
    Ok(Options::ExactPhraseLayerAudit {
        source: source.ok_or("exact-phrase-layer-audit requires --source")?,
        core_payload: core_payload.ok_or("exact-phrase-layer-audit requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("exact-phrase-layer-audit requires --supplemental-payload")?,
        fit_corpus: fit_corpus.ok_or("exact-phrase-layer-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("exact-phrase-layer-audit requires --held-out-corpus")?,
        entry_limit,
        repetitions,
    })
}

fn parse_exact_phrase_layer_preflight(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_package = None;
    let mut supplemental_package = None;
    let mut phrase_package = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-package" => set_path(&mut core_package, &mut arguments, "--core-package")?,
            "--supplemental-package" => set_path(
                &mut supplemental_package,
                &mut arguments,
                "--supplemental-package",
            )?,
            "--phrase-package" => {
                set_path(&mut phrase_package, &mut arguments, "--phrase-package")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-phrase-layer-preflight argument; value was suppressed".into(),
                );
            }
        }
    }
    let sample_limit =
        sample_limit.ok_or("exact-phrase-layer-preflight requires --sample-limit")?;
    if !(1..=32).contains(&sample_limit) {
        return Err(
            "exact-phrase-layer-preflight --sample-limit is outside the fixed bound".into(),
        );
    }
    let repetitions = repetitions.ok_or("exact-phrase-layer-preflight requires --repetitions")?;
    if !(1..=20).contains(&repetitions) {
        return Err("exact-phrase-layer-preflight --repetitions is outside the fixed bound".into());
    }
    Ok(Options::ExactPhraseLayerPreflight {
        core_package: core_package.ok_or("exact-phrase-layer-preflight requires --core-package")?,
        supplemental_package: supplemental_package
            .ok_or("exact-phrase-layer-preflight requires --supplemental-package")?,
        phrase_package: phrase_package
            .ok_or("exact-phrase-layer-preflight requires --phrase-package")?,
        sample_limit,
        repetitions,
    })
}

fn parse_exact_phrase_tsf_preflight(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_package = None;
    let mut supplemental_package = None;
    let mut phrase_package = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-package" => set_path(&mut core_package, &mut arguments, "--core-package")?,
            "--supplemental-package" => set_path(
                &mut supplemental_package,
                &mut arguments,
                "--supplemental-package",
            )?,
            "--phrase-package" => {
                set_path(&mut phrase_package, &mut arguments, "--phrase-package")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-phrase-tsf-preflight argument; value was suppressed".into(),
                );
            }
        }
    }
    let sample_limit = sample_limit.ok_or("exact-phrase-tsf-preflight requires --sample-limit")?;
    if !(1..=32).contains(&sample_limit) {
        return Err("exact-phrase-tsf-preflight --sample-limit is outside the fixed bound".into());
    }
    let repetitions = repetitions.ok_or("exact-phrase-tsf-preflight requires --repetitions")?;
    if !(1..=20).contains(&repetitions) || sample_limit.saturating_mul(repetitions) > 640 {
        return Err("exact-phrase-tsf-preflight workload exceeds the fixed 640-probe bound".into());
    }
    Ok(Options::ExactPhraseTsfPreflight {
        core_package: core_package.ok_or("exact-phrase-tsf-preflight requires --core-package")?,
        supplemental_package: supplemental_package
            .ok_or("exact-phrase-tsf-preflight requires --supplemental-package")?,
        phrase_package: phrase_package
            .ok_or("exact-phrase-tsf-preflight requires --phrase-package")?,
        sample_limit,
        repetitions,
    })
}

fn parse_exact_phrase_popup_preflight(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_package = None;
    let mut supplemental_package = None;
    let mut phrase_package = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-package" => set_path(&mut core_package, &mut arguments, "--core-package")?,
            "--supplemental-package" => set_path(
                &mut supplemental_package,
                &mut arguments,
                "--supplemental-package",
            )?,
            "--phrase-package" => {
                set_path(&mut phrase_package, &mut arguments, "--phrase-package")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err(
                    "unknown exact-phrase-popup-preflight argument; value was suppressed".into(),
                );
            }
        }
    }
    let sample_limit =
        sample_limit.ok_or("exact-phrase-popup-preflight requires --sample-limit")?;
    if !(1..=4).contains(&sample_limit) {
        return Err(
            "exact-phrase-popup-preflight --sample-limit is outside the fixed bound".into(),
        );
    }
    let repetitions = repetitions.ok_or("exact-phrase-popup-preflight requires --repetitions")?;
    if !(1..=5).contains(&repetitions)
        || sample_limit.saturating_mul(repetitions).saturating_mul(4) > 80
    {
        return Err(
            "exact-phrase-popup-preflight workload exceeds the fixed 80-window bound".into(),
        );
    }
    Ok(Options::ExactPhrasePopupPreflight {
        core_package: core_package.ok_or("exact-phrase-popup-preflight requires --core-package")?,
        supplemental_package: supplemental_package
            .ok_or("exact-phrase-popup-preflight requires --supplemental-package")?,
        phrase_package: phrase_package
            .ok_or("exact-phrase-popup-preflight requires --phrase-package")?,
        sample_limit,
        repetitions,
    })
}

fn parse_runtime_query(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut supplemental_root = None;
    let mut exact_short_root = None;
    let mut code = None;
    let mut limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--supplemental-root" => set_path(
                &mut supplemental_root,
                &mut arguments,
                "--supplemental-root",
            )?,
            "--exact-short-root" => {
                set_path(&mut exact_short_root, &mut arguments, "--exact-short-root")?
            }
            "--code" => set_value(&mut code, &mut arguments, "--code")?,
            "--limit" => set_usize(&mut limit, &mut arguments, "--limit")?,
            _ => return Err("unknown runtime-query argument; value was suppressed".into()),
        }
    }
    let code = code.ok_or("runtime-query requires --code")?;
    if code.is_empty() || code.len() > 64 || !code.as_bytes().iter().all(u8::is_ascii_lowercase) {
        return Err("runtime-query --code must be 1..64 lowercase ASCII letters".into());
    }
    let limit = limit.ok_or("runtime-query requires --limit")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&limit) {
        return Err("runtime-query --limit is outside the fixed bound".into());
    }
    Ok(Options::RuntimeQuery {
        root: root.ok_or("runtime-query requires --root")?,
        supplemental_root,
        exact_short_root,
        code,
        limit,
    })
}

fn parse_package_query(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut package = None;
    let mut code = None;
    let mut limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--code" => set_value(&mut code, &mut arguments, "--code")?,
            "--limit" => set_usize(&mut limit, &mut arguments, "--limit")?,
            _ => return Err("unknown package-query argument; value was suppressed".into()),
        }
    }
    let code = code.ok_or("package-query requires --code")?;
    if code.is_empty() || code.len() > 64 || !code.as_bytes().iter().all(u8::is_ascii_lowercase) {
        return Err("package-query --code must be 1..64 lowercase ASCII letters".into());
    }
    let limit = limit.ok_or("package-query requires --limit")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&limit) {
        return Err("package-query --limit is outside the fixed bound".into());
    }
    Ok(Options::PackageQuery {
        package: package.ok_or("package-query requires --package")?,
        code,
        limit,
    })
}

fn parse_exact_short_query(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut package = None;
    let mut code = None;
    let mut limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--code" => set_value(&mut code, &mut arguments, "--code")?,
            "--limit" => set_usize(&mut limit, &mut arguments, "--limit")?,
            _ => return Err("unknown exact-short-query argument; value was suppressed".into()),
        }
    }
    let code = code.ok_or("exact-short-query requires --code")?;
    if code.len() != 4 || !code.as_bytes().iter().all(u8::is_ascii_lowercase) {
        return Err("exact-short-query --code must be four lowercase ASCII letters".into());
    }
    let limit = limit.ok_or("exact-short-query requires --limit")?;
    if !(1..=MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH).contains(&limit) {
        return Err("exact-short-query --limit is outside the fixed bound".into());
    }
    Ok(Options::ExactShortQuery {
        package: package.ok_or("exact-short-query requires --package")?,
        code,
        limit,
    })
}

fn parse_exact_short_benchmark(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut package = None;
    let mut code = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--code" => set_value(&mut code, &mut arguments, "--code")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => {
                return Err("unknown exact-short-benchmark argument; value was suppressed".into());
            }
        }
    }
    let code = code.ok_or("exact-short-benchmark requires --code")?;
    if code.len() != 4 || !code.as_bytes().iter().all(u8::is_ascii_lowercase) {
        return Err("exact-short-benchmark --code must be four lowercase ASCII letters".into());
    }
    let repetitions = repetitions.ok_or("exact-short-benchmark requires --repetitions")?;
    if !(1..=1_000_000).contains(&repetitions) {
        return Err("exact-short-benchmark --repetitions is outside the fixed bound".into());
    }
    Ok(Options::ExactShortBenchmark {
        package: package.ok_or("exact-short-benchmark requires --package")?,
        code,
        repetitions,
    })
}

fn parse_layer_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut frontier_limit = None;
    let mut exact_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            _ => return Err("unknown layer-audit argument; value was suppressed".into()),
        }
    }
    let frontier_limit = frontier_limit.ok_or("layer-audit requires --frontier-limit")?;
    let exact_promotions = exact_promotions.ok_or("layer-audit requires --exact-promotions")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("layer-audit --frontier-limit is outside the fixed bound".into());
    }
    if exact_promotions > MAX_CANDIDATE_SNAPSHOT_RANK {
        return Err("layer-audit --exact-promotions is outside the fixed bound".into());
    }
    Ok(Options::LayerAudit {
        core_payload: core_payload.ok_or("layer-audit requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("layer-audit requires --supplemental-payload")?,
        frontier_limit,
        exact_promotions,
    })
}

fn parse_layer_benchmark(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut repetitions = None;
    let mut exact_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            _ => return Err("unknown layer-benchmark argument; value was suppressed".into()),
        }
    }
    let repetitions = repetitions.ok_or("layer-benchmark requires --repetitions")?;
    let exact_promotions = exact_promotions.ok_or("layer-benchmark requires --exact-promotions")?;
    if !(1..=100).contains(&repetitions) {
        return Err("layer-benchmark --repetitions is outside the fixed bound".into());
    }
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&exact_promotions) {
        return Err("layer-benchmark --exact-promotions is outside the fixed bound".into());
    }
    Ok(Options::LayerBenchmark {
        core_payload: core_payload.ok_or("layer-benchmark requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("layer-benchmark requires --supplemental-payload")?,
        repetitions,
        exact_promotions,
    })
}

fn parse_layer_composition_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut core_payload = None;
    let mut supplemental_payload = None;
    let mut corpus = None;
    let mut fit_corpus = None;
    let mut frontier_limit = None;
    let mut sample_limit = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--supplemental-payload" => set_path(
                &mut supplemental_payload,
                &mut arguments,
                "--supplemental-payload",
            )?,
            "--corpus" => set_path(&mut corpus, &mut arguments, "--corpus")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            _ => {
                return Err(
                    "unknown layer-composition-audit argument; value was suppressed".into(),
                );
            }
        }
    }
    let frontier_limit =
        frontier_limit.ok_or("layer-composition-audit requires --frontier-limit")?;
    let sample_limit = sample_limit.ok_or("layer-composition-audit requires --sample-limit")?;
    if !(5..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("layer-composition-audit --frontier-limit is outside the fixed bound".into());
    }
    if !(1..=512).contains(&sample_limit) {
        return Err("layer-composition-audit --sample-limit is outside the fixed bound".into());
    }
    Ok(Options::LayerCompositionAudit {
        core_payload: core_payload.ok_or("layer-composition-audit requires --core-payload")?,
        supplemental_payload: supplemental_payload
            .ok_or("layer-composition-audit requires --supplemental-payload")?,
        corpus: corpus.ok_or("layer-composition-audit requires --corpus")?,
        fit_corpus,
        frontier_limit,
        sample_limit,
    })
}

fn parse_static_context_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut model = None;
    let mut core_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    let mut sample_limit = None;
    let mut max_order = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--model" => set_path(&mut model, &mut arguments, "--model")?,
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--max-order" => set_usize(&mut max_order, &mut arguments, "--max-order")?,
            _ => {
                return Err("unknown static-context-audit argument; value was suppressed".into());
            }
        }
    }
    let frontier_limit = frontier_limit.ok_or("static-context-audit requires --frontier-limit")?;
    let sample_limit = sample_limit.ok_or("static-context-audit requires --sample-limit")?;
    let max_order = max_order.ok_or("static-context-audit requires --max-order")?;
    if !(5..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err("static-context-audit --frontier-limit is outside the fixed bound".into());
    }
    if !(1..=512).contains(&sample_limit) {
        return Err("static-context-audit --sample-limit is outside the fixed bound".into());
    }
    if !(1..=5).contains(&max_order) {
        return Err("static-context-audit --max-order is outside the fixed bound".into());
    }
    Ok(Options::StaticContextAudit {
        model: model.ok_or("static-context-audit requires --model")?,
        core_payload: core_payload.ok_or("static-context-audit requires --core-payload")?,
        fit_corpus: fit_corpus.ok_or("static-context-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("static-context-audit requires --held-out-corpus")?,
        frontier_limit,
        sample_limit,
        max_order,
    })
}

fn parse_single_character_context_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut model = None;
    let mut core_payload = None;
    let mut fit_corpus = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    let mut sample_limit = None;
    let mut max_order = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--model" => set_path(&mut model, &mut arguments, "--model")?,
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--fit-corpus" => set_path(&mut fit_corpus, &mut arguments, "--fit-corpus")?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--max-order" => set_usize(&mut max_order, &mut arguments, "--max-order")?,
            _ => {
                return Err(
                    "unknown single-character-context-audit argument; value was suppressed".into(),
                );
            }
        }
    }
    let frontier_limit =
        frontier_limit.ok_or("single-character-context-audit requires --frontier-limit")?;
    let sample_limit =
        sample_limit.ok_or("single-character-context-audit requires --sample-limit")?;
    let max_order = max_order.ok_or("single-character-context-audit requires --max-order")?;
    if !(5..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err(
            "single-character-context-audit --frontier-limit is outside the fixed bound".into(),
        );
    }
    if !(1..=512).contains(&sample_limit) {
        return Err(
            "single-character-context-audit --sample-limit is outside the fixed bound".into(),
        );
    }
    if !(1..=5).contains(&max_order) {
        return Err("single-character-context-audit --max-order is outside the fixed bound".into());
    }
    Ok(Options::SingleCharacterContextAudit {
        model: model.ok_or("single-character-context-audit requires --model")?,
        core_payload: core_payload
            .ok_or("single-character-context-audit requires --core-payload")?,
        fit_corpus: fit_corpus.ok_or("single-character-context-audit requires --fit-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("single-character-context-audit requires --held-out-corpus")?,
        frontier_limit,
        sample_limit,
        max_order,
    })
}

fn parse_single_character_context_validation_audit(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut model = None;
    let mut core_payload = None;
    let mut development_corpus = None;
    let mut held_out_corpus = None;
    let mut frontier_limit = None;
    let mut sample_limit = None;
    let mut max_order = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--model" => set_path(&mut model, &mut arguments, "--model")?,
            "--core-payload" => set_path(&mut core_payload, &mut arguments, "--core-payload")?,
            "--development-corpus" => set_path(
                &mut development_corpus,
                &mut arguments,
                "--development-corpus",
            )?,
            "--held-out-corpus" => {
                set_path(&mut held_out_corpus, &mut arguments, "--held-out-corpus")?
            }
            "--frontier-limit" => {
                set_usize(&mut frontier_limit, &mut arguments, "--frontier-limit")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--max-order" => set_usize(&mut max_order, &mut arguments, "--max-order")?,
            _ => {
                return Err(
                    "unknown single-character-context-validation-audit argument; value was suppressed"
                        .into(),
                );
            }
        }
    }
    let frontier_limit = frontier_limit
        .ok_or("single-character-context-validation-audit requires --frontier-limit")?;
    let sample_limit =
        sample_limit.ok_or("single-character-context-validation-audit requires --sample-limit")?;
    let max_order =
        max_order.ok_or("single-character-context-validation-audit requires --max-order")?;
    if !(5..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err(
            "single-character-context-validation-audit --frontier-limit is outside the fixed bound"
                .into(),
        );
    }
    if !(1..=512).contains(&sample_limit) {
        return Err(
            "single-character-context-validation-audit --sample-limit is outside the fixed bound"
                .into(),
        );
    }
    if !(1..=5).contains(&max_order) {
        return Err(
            "single-character-context-validation-audit --max-order is outside the fixed bound"
                .into(),
        );
    }
    Ok(Options::SingleCharacterContextValidationAudit {
        model: model.ok_or("single-character-context-validation-audit requires --model")?,
        core_payload: core_payload
            .ok_or("single-character-context-validation-audit requires --core-payload")?,
        development_corpus: development_corpus
            .ok_or("single-character-context-validation-audit requires --development-corpus")?,
        held_out_corpus: held_out_corpus
            .ok_or("single-character-context-validation-audit requires --held-out-corpus")?,
        frontier_limit,
        sample_limit,
        max_order,
    })
}

fn parse_supplement_enable(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut exact_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            _ => return Err("unknown supplement-enable argument; value was suppressed".into()),
        }
    }
    let exact_promotions =
        exact_promotions.ok_or("supplement-enable requires --exact-promotions")?;
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&exact_promotions) {
        return Err("supplement-enable --exact-promotions is outside the fixed bound".into());
    }
    Ok(Options::SupplementEnable {
        root: root.ok_or("supplement-enable requires --root")?,
        exact_promotions,
    })
}

fn parse_exact_short_enable(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut core_root = None;
    let mut supplemental_root = None;
    let mut without_supplement = false;
    let mut package = None;
    let mut expected_sha256 = None;
    let mut exact_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--core-root" => set_path(&mut core_root, &mut arguments, "--core-root")?,
            "--supplemental-root" => set_path(
                &mut supplemental_root,
                &mut arguments,
                "--supplemental-root",
            )?,
            "--without-supplement" => {
                if without_supplement {
                    return Err("--without-supplement can be given only once".into());
                }
                without_supplement = true;
            }
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--expected-sha256" => {
                set_value(&mut expected_sha256, &mut arguments, "--expected-sha256")?
            }
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            _ => return Err("unknown exact-short-enable argument; value was suppressed".into()),
        }
    }
    let exact_promotions =
        exact_promotions.ok_or("exact-short-enable requires --exact-promotions")?;
    if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
        return Err("exact-short-enable --exact-promotions is outside the fixed bound".into());
    }
    if supplemental_root.is_some() == without_supplement {
        return Err(
            "exact-short-enable requires exactly one of --supplemental-root or --without-supplement"
                .into(),
        );
    }
    Ok(Options::ExactShortEnable {
        root: root.ok_or("exact-short-enable requires --root")?,
        core_root: core_root.ok_or("exact-short-enable requires --core-root")?,
        supplemental_root,
        package: package.ok_or("exact-short-enable requires --package")?,
        expected_sha256: canonical_expected_sha256(
            &expected_sha256.ok_or("exact-short-enable requires --expected-sha256")?,
        )?,
        exact_promotions,
    })
}

fn parse_exact_short_readiness(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut core_root = None;
    let mut supplemental_root = None;
    let mut without_supplement = false;
    let mut package = None;
    let mut expected_sha256 = None;
    let mut exact_promotions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--core-root" => set_path(&mut core_root, &mut arguments, "--core-root")?,
            "--supplemental-root" => set_path(
                &mut supplemental_root,
                &mut arguments,
                "--supplemental-root",
            )?,
            "--without-supplement" => {
                if without_supplement {
                    return Err("--without-supplement can be given only once".into());
                }
                without_supplement = true;
            }
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--expected-sha256" => {
                set_value(&mut expected_sha256, &mut arguments, "--expected-sha256")?
            }
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            _ => return Err("unknown exact-short-readiness argument; value was suppressed".into()),
        }
    }
    let exact_promotions =
        exact_promotions.ok_or("exact-short-readiness requires --exact-promotions")?;
    if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
        return Err("exact-short-readiness --exact-promotions is outside the fixed bound".into());
    }
    if supplemental_root.is_some() == without_supplement {
        return Err(
            "exact-short-readiness requires exactly one of --supplemental-root or --without-supplement"
                .into(),
        );
    }
    Ok(Options::ExactShortReadiness {
        root: root.ok_or("exact-short-readiness requires --root")?,
        core_root: core_root.ok_or("exact-short-readiness requires --core-root")?,
        supplemental_root,
        package: package.ok_or("exact-short-readiness requires --package")?,
        expected_sha256: canonical_expected_sha256(
            &expected_sha256.ok_or("exact-short-readiness requires --expected-sha256")?,
        )?,
        exact_promotions,
    })
}

fn parse_exact_short_prepare(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut core_root = None;
    let mut supplemental_root = None;
    let mut without_supplement = false;
    let mut package = None;
    let mut expected_sha256 = None;
    let mut exact_promotions = None;
    let mut sample_limit = None;
    let mut repetitions = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--core-root" => set_path(&mut core_root, &mut arguments, "--core-root")?,
            "--supplemental-root" => set_path(
                &mut supplemental_root,
                &mut arguments,
                "--supplemental-root",
            )?,
            "--without-supplement" => {
                if without_supplement {
                    return Err("--without-supplement can be given only once".into());
                }
                without_supplement = true;
            }
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--expected-sha256" => {
                set_value(&mut expected_sha256, &mut arguments, "--expected-sha256")?
            }
            "--exact-promotions" => {
                set_usize(&mut exact_promotions, &mut arguments, "--exact-promotions")?
            }
            "--sample-limit" => set_usize(&mut sample_limit, &mut arguments, "--sample-limit")?,
            "--repetitions" => set_usize(&mut repetitions, &mut arguments, "--repetitions")?,
            _ => return Err("unknown exact-short-prepare argument; value was suppressed".into()),
        }
    }
    let exact_promotions =
        exact_promotions.ok_or("exact-short-prepare requires --exact-promotions")?;
    if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
        return Err("exact-short-prepare --exact-promotions is outside the fixed bound".into());
    }
    let sample_limit = sample_limit.ok_or("exact-short-prepare requires --sample-limit")?;
    let repetitions = repetitions.ok_or("exact-short-prepare requires --repetitions")?;
    if !(1..=32).contains(&sample_limit)
        || !(1..=20).contains(&repetitions)
        || sample_limit.saturating_mul(repetitions) > 640
    {
        return Err("exact-short-prepare workload exceeds the fixed 640-probe bound".into());
    }
    if supplemental_root.is_some() == without_supplement {
        return Err(
            "exact-short-prepare requires exactly one of --supplemental-root or --without-supplement"
                .into(),
        );
    }
    Ok(Options::ExactShortPrepare {
        root: root.ok_or("exact-short-prepare requires --root")?,
        core_root: core_root.ok_or("exact-short-prepare requires --core-root")?,
        supplemental_root,
        package: package.ok_or("exact-short-prepare requires --package")?,
        expected_sha256: canonical_expected_sha256(
            &expected_sha256.ok_or("exact-short-prepare requires --expected-sha256")?,
        )?,
        exact_promotions,
        sample_limit,
        repetitions,
    })
}

fn parse_root_only(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut root = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    root.ok_or_else(|| format!("{command} requires exactly one --root path").into())
}

fn parse_root_package_and_expected_sha256(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<(PathBuf, PathBuf, String), Box<dyn std::error::Error>> {
    let mut root = None;
    let mut package = None;
    let mut expected_sha256 = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--expected-sha256" => {
                set_value(&mut expected_sha256, &mut arguments, "--expected-sha256")?
            }
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    Ok((
        root.ok_or_else(|| format!("{command} requires exactly one --root path"))?,
        package.ok_or_else(|| format!("{command} requires exactly one --package path"))?,
        canonical_expected_sha256(
            &expected_sha256
                .ok_or_else(|| format!("{command} requires exactly one --expected-sha256 value"))?,
        )?,
    ))
}

fn parse_root_package_and_signature(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<(PathBuf, PathBuf, PathBuf, String), Box<dyn std::error::Error>> {
    let mut root = None;
    let mut package = None;
    let mut signature = None;
    let mut trusted_public_key = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--signature" => set_path(&mut signature, &mut arguments, "--signature")?,
            "--trusted-public-key" => set_value(
                &mut trusted_public_key,
                &mut arguments,
                "--trusted-public-key",
            )?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    Ok((
        root.ok_or_else(|| format!("{command} requires exactly one --root path"))?,
        package.ok_or_else(|| format!("{command} requires exactly one --package path"))?,
        signature.ok_or_else(|| format!("{command} requires exactly one --signature path"))?,
        canonical_trusted_public_key(&trusted_public_key.ok_or_else(|| {
            format!("{command} requires exactly one --trusted-public-key value")
        })?)?,
    ))
}

fn parse_package_and_expected_sha256(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<(PathBuf, String), Box<dyn std::error::Error>> {
    let mut package = None;
    let mut expected_sha256 = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--expected-sha256" => {
                set_value(&mut expected_sha256, &mut arguments, "--expected-sha256")?
            }
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    Ok((
        package.ok_or_else(|| format!("{command} requires exactly one --package path"))?,
        canonical_expected_sha256(
            &expected_sha256
                .ok_or_else(|| format!("{command} requires exactly one --expected-sha256 value"))?,
        )?,
    ))
}

fn parse_verify_signature(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut package = None;
    let mut signature = None;
    let mut trusted_public_key = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            "--signature" => set_path(&mut signature, &mut arguments, "--signature")?,
            "--trusted-public-key" => set_value(
                &mut trusted_public_key,
                &mut arguments,
                "--trusted-public-key",
            )?,
            _ => return Err("unknown verify-signature argument; value was suppressed".into()),
        }
    }
    Ok(Options::VerifySignature {
        package: package.ok_or("verify-signature requires exactly one --package path")?,
        signature: signature.ok_or("verify-signature requires exactly one --signature path")?,
        trusted_public_key: canonical_trusted_public_key(
            &trusted_public_key
                .ok_or("verify-signature requires exactly one --trusted-public-key value")?,
        )?,
    })
}

fn parse_package_only(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut package = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    package.ok_or_else(|| format!("{command} requires exactly one --package path").into())
}

fn set_path(
    slot: &mut Option<PathBuf>,
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if slot.is_some() {
        return Err(format!("{option} can be given only once").into());
    }
    *slot = Some(PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| format!("{option} requires a path"))?,
    ));
    Ok(())
}

fn set_value(
    slot: &mut Option<String>,
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if slot.is_some() {
        return Err(format!("{option} can be given only once").into());
    }
    *slot = Some(
        arguments
            .next()
            .ok_or_else(|| format!("{option} requires a value"))?,
    );
    Ok(())
}

fn set_usize(
    slot: &mut Option<usize>,
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if slot.is_some() {
        return Err(format!("{option} can be given only once").into());
    }
    let value = arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))?;
    *slot = Some(
        value
            .parse::<usize>()
            .map_err(|_| format!("{option} requires a positive integer"))?,
    );
    Ok(())
}

fn canonical_expected_sha256(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    canonical_lowercase_hex(
        value,
        32,
        "--expected-sha256 must be one lowercase SHA-256 value",
    )
}

fn canonical_trusted_public_key(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    canonical_lowercase_hex(
        value,
        32,
        "--trusted-public-key must be one lowercase Ed25519 public key",
    )
}

fn canonical_lowercase_hex(
    value: &str,
    bytes: usize,
    error: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    if value.len() == bytes.saturating_mul(2)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value.to_owned())
    } else {
        Err(error.into())
    }
}

fn reject_extra(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.next().is_some() {
        return Err("unexpected argument; value was suppressed".into());
    }
    Ok(())
}

fn print_usage() {
    eprintln!("candidatectl commands:");
    eprintln!(
        "  inspect --manifest <PACKAGE.zcm> --payload <LEXICON.tsv> --provenance <SOURCE.zcp>"
    );
    eprintln!(
        "  build --source <LEXICON.tsv> --output <NEW_PACKAGE_DIR> --revision <REV> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --public"
    );
    eprintln!(
        "  build-rime --source <RIME.dict.yaml> --output <NEW_PACKAGE_DIR> --revision <REV> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --public"
    );
    eprintln!(
        "  build-rime-slice --source <TONED_RIME.dict.yaml> --output <NEW_PACKAGE_DIR> --revision <REV> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --max-entries <1..120000> [--frequency-frontier-entries <1..MAX>] [--two-character-coverage-depth <1..8>] [--three-character-coverage-entries <N>] [--four-character-coverage-entries <N>] --max-text-characters <1..12> --public"
    );
    eprintln!(
        "  build-short-consensus-layer --source <TONED_RIME.dict.yaml> --confirmation <PUBLIC_WORDS.txt> --base-payload <LEXICON.tsv> --output <NEW_PACKAGE_DIR> --revision <REV> --per-code-depth <1..8> --entry-limit <1..50000> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --confirmation-id <ID> --confirmation-license <SPDX> --confirmation-url <HTTPS_URL> --confirmation-sha256 <SHA256> --base-id <ID> --base-license <SPDX> --base-url <HTTPS_URL> --base-sha256 <SHA256> --public"
    );
    eprintln!(
        "  build-phrase-layer --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --output <NEW_PACKAGE_DIR> --revision <REV> --entry-limit <1..50000> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --allowlist-id <ID> --allowlist-license <SPDX> --allowlist-url <HTTPS_URL> --allowlist-sha256 <SHA256> --base-id <ID> --base-license <SPDX> --base-url <HTTPS_URL> --base-sha256 <SHA256> --public"
    );
    eprintln!(
        "  build-exact-phrase-layer --source <TONED_RIME.dict.yaml> --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --output <NEW_PACKAGE_DIR> --revision <REV> --entry-limit <1..50000> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --core-id <ID> --core-license <SPDX> --core-url <HTTPS_URL> --core-sha256 <SHA256> --supplemental-id <ID> --supplemental-license <SPDX> --supplemental-url <HTTPS_URL> --supplemental-sha256 <SHA256> --fit-id <ID> --fit-license <SPDX> --fit-url <HTTPS_URL> --fit-sha256 <SHA256> --public"
    );
    eprintln!(
        "  merge-public-packages --base <PACKAGE_DIR> --overlay <PACKAGE_DIR> --output <NEW_PACKAGE_DIR> --revision <REV> --public"
    );
    eprintln!(
        "  diagnose-public-miss --source <TONED_RIME.dict.yaml> --core-package <PUBLIC_PACKAGE_DIR> --supplemental-package <PUBLIC_PACKAGE_DIR> --code <FULL_KEYS> --text <PUBLIC_TARGET> --public"
    );
    eprintln!("  compare --base-payload <LEXICON.tsv> --challenger-payload <LEXICON.tsv>");
    eprintln!(
        "  consensus-audit --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <1..50>"
    );
    eprintln!(
        "  short-rank-audit --core-payload <LEXICON.tsv> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <1..50>"
    );
    eprintln!(
        "  segment-penalty-audit --core-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <1..10> --sample-limit <1..512>"
    );
    eprintln!(
        "  length-coverage-audit --base-payload <LEXICON.tsv> --challenger-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu>"
    );
    eprintln!(
        "  short-consensus-audit --source <TONED_RIME.dict.yaml> --confirmation <PUBLIC_WORDS.txt> --base-payload <LEXICON.tsv> --per-code-depth <1..8> --entry-limit <1..50000>"
    );
    eprintln!(
        "  exact-short-layer-audit --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --exact-package <PUBLIC_PACKAGE_DIR> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <2..50> --supplemental-promotions <0..FRONTIER>"
    );
    eprintln!(
        "  exact-short-layer-benchmark --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --exact-package <PUBLIC_PACKAGE_DIR> --frontier-limit <2..25> --supplemental-promotions <0..FRONTIER> --exact-promotions <1..8> --candidate-limit <TWO-PAGES..50> --sample-limit <8..2048> --repetitions <1..100>"
    );
    eprintln!(
        "  exact-short-tsf-preflight --core-package <PUBLIC_PACKAGE_DIR> [--supplemental-package <PUBLIC_PACKAGE_DIR> --supplemental-promotions <1..50>] --exact-package <PUBLIC_PACKAGE_DIR> --exact-promotions <1..8> --sample-limit <1..32> --repetitions <1..20>"
    );
    eprintln!("  popup-render-preflight --repetitions <1..20>");
    eprintln!(
        "  phrase-coverage-audit --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --entry-limit <1..50000>"
    );
    eprintln!(
        "  phrase-layer-audit --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --small-limit <N> --large-limit <N> --repetitions <1..100>"
    );
    eprintln!(
        "  exact-phrase-layer-audit --source <TONED_RIME.dict.yaml> --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --entry-limit <1..50000> --repetitions <1..100>"
    );
    eprintln!(
        "  exact-phrase-layer-preflight --core-package <PUBLIC_PACKAGE_DIR> --supplemental-package <PUBLIC_PACKAGE_DIR> --phrase-package <FOUR_SOURCE_PUBLIC_PACKAGE_DIR> --sample-limit <1..32> --repetitions <1..20>"
    );
    eprintln!(
        "  exact-phrase-tsf-preflight --core-package <PUBLIC_PACKAGE_DIR> --supplemental-package <PUBLIC_PACKAGE_DIR> --phrase-package <FOUR_SOURCE_PUBLIC_PACKAGE_DIR> --sample-limit <1..32> --repetitions <1..20>"
    );
    eprintln!(
        "  exact-phrase-popup-preflight --core-package <PUBLIC_PACKAGE_DIR> --supplemental-package <PUBLIC_PACKAGE_DIR> --phrase-package <FOUR_SOURCE_PUBLIC_PACKAGE_DIR> --sample-limit <1..4> --repetitions <1..5>  (briefly shows isolated popup windows)"
    );
    eprintln!(
        "  layer-audit --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --frontier-limit <1..50> --exact-promotions <0..50>"
    );
    eprintln!(
        "  layer-benchmark --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --repetitions <1..100> --exact-promotions <1..50>"
    );
    eprintln!(
        "  layer-composition-audit --core-payload <LEXICON.tsv> --supplemental-payload <LEXICON.tsv> --corpus <PUBLIC.conllu> [--fit-corpus <PUBLIC-TRAIN.conllu>] --frontier-limit <1..50> --sample-limit <1..512>"
    );
    eprintln!(
        "  static-context-audit --model <PUBLIC.arpa> --core-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <5..50> --sample-limit <1..512> --max-order <1..5>"
    );
    eprintln!(
        "  single-character-context-audit --model <PUBLIC.arpa> --core-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --frontier-limit <5..50> --sample-limit <1..512> --max-order <1..5>"
    );
    eprintln!(
        "  single-character-context-validation-audit --model <PUBLIC.arpa> --core-payload <LEXICON.tsv> --development-corpus <PUBLIC-DEV.conllu> --held-out-corpus <PUBLIC-HOLDOUT.conllu> --frontier-limit <5..50> --sample-limit <1..512> --max-order <1..5>"
    );
    eprintln!("  supplement-status --root <SUPPLEMENTAL_SLOT_DIR>");
    eprintln!("  supplement-enable --root <SUPPLEMENTAL_SLOT_DIR> --exact-promotions <1..50>");
    eprintln!("  supplement-disable --root <SUPPLEMENTAL_SLOT_DIR>");
    eprintln!("  exact-short-status --root <EXACT_SHORT_SLOT_DIR>");
    eprintln!(
        "  exact-short-readiness --root <EXACT_SHORT_SLOT_DIR> --core-root <CORE_SLOT_DIR> (--supplemental-root <ENABLED_SUPPLEMENTAL_SLOT_DIR> | --without-supplement) --package <PUBLIC_EXACT_PACKAGE_DIR> --expected-sha256 <SHA256> --exact-promotions <1..8>"
    );
    eprintln!(
        "  exact-short-prepare --root <EXACT_SHORT_SLOT_DIR> --core-root <CORE_SLOT_DIR> (--supplemental-root <ENABLED_SUPPLEMENTAL_SLOT_DIR> | --without-supplement) --package <PUBLIC_EXACT_PACKAGE_DIR> --expected-sha256 <SHA256> --exact-promotions <1..8> --sample-limit <1..32> --repetitions <1..20>"
    );
    eprintln!(
        "  exact-short-enable --root <EXACT_SHORT_SLOT_DIR> --core-root <CORE_SLOT_DIR> (--supplemental-root <ENABLED_SUPPLEMENTAL_SLOT_DIR> | --without-supplement) --package <PUBLIC_EXACT_PACKAGE_DIR> --expected-sha256 <SHA256> --exact-promotions <1..8>"
    );
    eprintln!("  exact-short-disable --root <EXACT_SHORT_SLOT_DIR>");
    eprintln!("  preflight --package <PACKAGE_DIR>");
    eprintln!(
        "  package-query --package <PUBLIC_PACKAGE_DIR> --code <LOWERCASE_KEYS> --limit <1..50>"
    );
    eprintln!(
        "  exact-short-query --package <PUBLIC_PACKAGE_DIR> --code <FOUR_KEYS> --limit <1..8>"
    );
    eprintln!(
        "  exact-short-benchmark --package <PUBLIC_PACKAGE_DIR> --code <FOUR_KEYS> --repetitions <1..1000000>"
    );
    eprintln!("  verify --package <PACKAGE_DIR> --expected-sha256 <SHA256>");
    eprintln!(
        "  verify-signature --package <PACKAGE_DIR> --signature <STATEMENT> --trusted-public-key <ED25519_HEX>"
    );
    eprintln!("  status --root <SLOT_DIR>");
    eprintln!("  runtime-check --root <SLOT_DIR>");
    eprintln!(
        "  runtime-query --root <SLOT_DIR> [--supplemental-root <SUPPLEMENTAL_SLOT_DIR>] [--exact-short-root <EXACT_SHORT_SLOT_DIR>] --code <LOWERCASE_KEYS> --limit <1..50>"
    );
    eprintln!("  adopt|stage --root <SLOT_DIR> --package <PACKAGE_DIR> --expected-sha256 <SHA256>");
    eprintln!(
        "  adopt-signed|stage-signed --root <SLOT_DIR> --package <PACKAGE_DIR> --signature <STATEMENT> --trusted-public-key <ED25519_HEX>"
    );
    eprintln!("  promote|rollback --root <SLOT_DIR>");
}

fn inspect(
    manifest_path: &Path,
    payload_path: &Path,
    provenance_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest_text = read_explicit_text(
        manifest_path,
        "candidate manifest",
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    )?;
    let manifest = CandidatePackageManifest::parse(&manifest_text)?;
    let provenance_text = read_explicit_text(
        provenance_path,
        "candidate provenance",
        MAX_CANDIDATE_PROVENANCE_BYTES,
    )?;
    let provenance = CandidatePackageProvenance::parse(&provenance_text)?;
    let payload_text = read_explicit_text(
        payload_path,
        "candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    provenance.validate_materials(&manifest_text, &payload_text)?;
    let snapshot = manifest.load_snapshot(&payload_text)?;
    Ok(render_inspect_report(&snapshot, &provenance))
}

fn public_package_query(
    package: &Path,
    code: &str,
    limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    let candidates = loaded.snapshot.candidate_texts(code, limit)?;
    let mut output = String::new();
    writeln!(output, "公开候选包查询")?;
    writeln!(output, "版本：{}", loaded.snapshot.revision())?;
    writeln!(output, "输入：{code}")?;
    for (index, candidate) in candidates.iter().enumerate() {
        writeln!(output, "{}. {}", index + 1, candidate)?;
    }
    if candidates.is_empty() {
        writeln!(output, "（没有候选）")?;
    }
    writeln!(
        output,
        "口径：仅查询已验证的公开包；不加载个人数据或运行时重排。"
    )?;
    writeln!(output, "本次操作：只读")?;
    Ok(output)
}

fn exact_short_package_query(
    package: &Path,
    code: &str,
    limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_exact_short_package_directory(package)?;
    let candidates = loaded.catalog.candidate_texts(code, limit)?;
    let mut output = String::new();
    writeln!(output, "公开精确短词查询")?;
    writeln!(output, "版本：{}", loaded.catalog.revision())?;
    writeln!(output, "输入：{code}")?;
    for (index, candidate) in candidates.iter().enumerate() {
        writeln!(output, "{}. {}", index + 1, candidate)?;
    }
    if candidates.is_empty() {
        writeln!(output, "（没有精确短词）")?;
    }
    writeln!(
        output,
        "口径：认证包后只查询四键码的 TSV 字节范围；未构造通用 Decoder。"
    )?;
    writeln!(output, "本次操作：只读")?;
    Ok(output)
}

fn benchmark_exact_short_package(
    package: &Path,
    code: &str,
    repetitions: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let load_started = Instant::now();
    let loaded = load_exact_short_package_directory(package)?;
    let load_elapsed = load_started.elapsed();
    let expected = loaded.catalog.candidate_texts(code, 8)?;
    let expected_count = expected.len();
    drop(expected);

    let query_started = Instant::now();
    for _ in 0..repetitions {
        let candidates = loaded.catalog.candidate_texts(black_box(code), 8)?;
        black_box(candidates);
    }
    let query_elapsed = query_started.elapsed();
    let average_query_microseconds = query_elapsed.as_secs_f64() * 1_000_000.0 / repetitions as f64;
    Ok(format!(
        "公开精确短词层本机观测\n版本：{}\n词条：{}；完整码：{}；载荷：{} 字节；索引：{} 字节\n首次认证与建索引：{:.3} ms\n热查询：{} 次；每次平均 {:.3} µs；目标码返回 {} 项\n口径：release 模式、当前机器、文件系统当前缓存状态；不是跨设备延迟承诺。\n本次操作：只读\n",
        loaded.catalog.revision(),
        loaded.catalog.entry_count(),
        loaded.catalog.code_count(),
        loaded.catalog.payload_bytes(),
        loaded.catalog.index_bytes(),
        load_elapsed.as_secs_f64() * 1_000.0,
        repetitions,
        average_query_microseconds,
        expected_count,
    ))
}

fn build_public_package(
    source: &Path,
    output: &Path,
    revision: &str,
    declaration: &PublicSourceDeclaration,
) -> Result<String, Box<dyn std::error::Error>> {
    let payload = read_explicit_text(
        source,
        "public lexicon source",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    if candidate_sha256_hex(payload.as_bytes()) != declaration.sha256 {
        return Err("public lexicon source SHA-256 does not match the explicit pin".into());
    }
    write_public_package(output, revision, declaration, &payload)
}

fn build_rime_public_package(
    source: &Path,
    output: &Path,
    revision: &str,
    declaration: &PublicSourceDeclaration,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        source,
        "public Rime lexicon source",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    if candidate_sha256_hex(source_text.as_bytes()) != declaration.sha256 {
        return Err("public Rime source SHA-256 does not match the explicit pin".into());
    }
    let imported = if uses_pinned_simplified_rime_import(declaration) {
        parse_simplified_rime_lexicon(&source_text)?
    } else {
        parse_rime_lexicon(&source_text)?
    };
    let stats = imported.stats;
    let mut payload = String::from("text\tpinyin\tfrequency\n");
    for entry in imported.entries {
        writeln!(
            payload,
            "{}\t{}\t{}",
            entry.text, entry.pinyin, entry.frequency
        )?;
    }
    let mut report = write_public_package(output, revision, declaration, &payload)?;
    if stats.shadowed_traditional_single_character_rows > 0 {
        writeln!(
            report,
            "简体清理：省略 {} 条被同音高频简体字遮蔽的繁体单字读音",
            stats.shadowed_traditional_single_character_rows
        )?;
    }
    Ok(report)
}

fn uses_pinned_simplified_rime_import(declaration: &PublicSourceDeclaration) -> bool {
    declaration.id == "rime-pinyin-simp" && declaration.sha256 == PINNED_RIME_PINYIN_SIMP_SHA256
}

fn build_rime_slice_public_package(
    source: &Path,
    output: &Path,
    revision: &str,
    declaration: &PublicSourceDeclaration,
    config: PublicRimeSliceConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        source,
        "large public Rime lexicon source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    if candidate_sha256_hex(source_text.as_bytes()) != declaration.sha256 {
        return Err("large public Rime source SHA-256 does not match the explicit pin".into());
    }
    let imported = parse_public_rime_slice(&source_text, config)?;
    let mut payload = String::from("text\tpinyin\tfrequency\n");
    for entry in imported.entries {
        writeln!(
            payload,
            "{}\t{}\t{}",
            entry.text, entry.pinyin, entry.frequency
        )?;
    }
    let mut report = write_public_package(output, revision, declaration, &payload)?;
    write_slice_stats(&mut report, imported.stats);
    Ok(report)
}

fn build_short_consensus_layer_public_package(
    options: ShortConsensusLayerBuildOptions,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        &options.source,
        "public toned Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let confirmation_text = read_explicit_text(
        &options.confirmation,
        "independent public short-word confirmation",
        MAX_PUBLIC_SHORT_WORD_CONFIRMATION_BYTES,
    )?;
    let base_text = read_explicit_text(
        &options.base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;

    let materials = vec![
        verified_source_material(
            &options.source_declaration,
            source_text.as_bytes(),
            "public Rime source",
        )?,
        verified_source_material(
            &options.confirmation_declaration,
            confirmation_text.as_bytes(),
            "independent public short-word confirmation",
        )?,
        verified_source_material(
            &options.base_declaration,
            base_text.as_bytes(),
            "base public candidate payload",
        )?,
    ];
    if materials
        .iter()
        .map(CandidateSourceMaterial::sha256)
        .collect::<HashSet<_>>()
        .len()
        != materials.len()
    {
        return Err("build-short-consensus-layer requires three distinct public materials".into());
    }

    let base_entries = parse_lexicon_tsv(&base_text)?;
    let imported = parse_public_short_word_consensus(
        &source_text,
        &confirmation_text,
        &base_entries,
        options.per_code_depth,
        options.entry_limit,
    )?;
    let stats = imported.stats;
    if imported.entries.is_empty() {
        return Err("build-short-consensus-layer found no new confirmed identities".into());
    }
    let mut entries = imported.entries;
    // Stable sorting preserves Rime source order for equal-weight identities.
    entries.sort_by(|left, right| {
        left.code
            .as_str()
            .cmp(right.code.as_str())
            .then_with(|| right.frequency.cmp(&left.frequency))
    });
    let payload = serialize_lexicon_payload(&entries);
    let mut report =
        write_exact_short_public_package(&options.output, &options.revision, materials, &payload)?;
    writeln!(
        report,
        "双来源短词：基础 {} 条；可用新增身份 {}；规范码 {}；每码最多 {}；写入 {} 条；上限外 {}",
        base_entries.len(),
        stats.available_new_identities,
        stats.available_new_codes,
        stats.per_code_depth,
        stats.imported_entries,
        stats.dropped_by_entry_cap,
    )?;
    writeln!(
        report,
        "运行时状态：未接入、未安装、未启用；当前 TSF 候选保持不变"
    )?;
    Ok(report)
}

struct PhraseLayerBuildRequest<'a> {
    source: &'a Path,
    allowlist: &'a Path,
    base_payload: &'a Path,
    output: &'a Path,
    revision: &'a str,
    entry_limit: usize,
    source_declaration: &'a PublicSourceDeclaration,
    allowlist_declaration: &'a PublicSourceDeclaration,
    base_declaration: &'a PublicSourceDeclaration,
}

fn build_phrase_layer_public_package(
    request: PhraseLayerBuildRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let PhraseLayerBuildRequest {
        source,
        allowlist,
        base_payload,
        output,
        revision,
        entry_limit,
        source_declaration,
        allowlist_declaration,
        base_declaration,
    } = request;
    let source_text = read_explicit_text(
        source,
        "public toned Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let allowlist_text = read_explicit_text(
        allowlist,
        "public fixed-phrase allowlist",
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES,
    )?;
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;

    // All three exact inputs are authenticated before an output directory can
    // be created. The base payload affects de-duplication even though it is not
    // copied into the supplemental phrase-layer payload.
    let materials = vec![
        verified_source_material(
            source_declaration,
            source_text.as_bytes(),
            "public Rime source",
        )?,
        verified_source_material(
            allowlist_declaration,
            allowlist_text.as_bytes(),
            "public fixed-phrase allowlist",
        )?,
        verified_source_material(
            base_declaration,
            base_text.as_bytes(),
            "base public candidate payload",
        )?,
    ];

    let base_entries = parse_lexicon_tsv(&base_text)?;
    let imported = parse_public_rime_phrase_allowlist(
        &source_text,
        &allowlist_text,
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES,
    )?;
    let base_surfaces = base_entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let mut available = imported
        .entries
        .into_iter()
        .filter(|entry| !base_surfaces.contains(entry.text.as_str()))
        .collect::<Vec<_>>();
    let available_entries = available.len();
    if available_entries < entry_limit {
        return Err("build-phrase-layer entry limit exceeds available new public entries".into());
    }
    available.truncate(entry_limit);
    let payload = serialize_lexicon_payload(&available);
    let mut report = write_multi_source_public_package(output, revision, materials, &payload)?;
    writeln!(
        report,
        "固定短语层：基础 {} 条；可用新增 {} 条；确定性选取 {} 条",
        base_entries.len(),
        available_entries,
        entry_limit,
    )?;
    Ok(report)
}

fn build_exact_phrase_layer_public_package(
    options: ExactPhraseLayerBuildOptions,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        &options.source,
        "public exact-phrase Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let core_text = read_explicit_text(
        &options.core_payload,
        "core public exact-phrase payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        &options.supplemental_payload,
        "supplemental public exact-phrase payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        &options.fit_corpus,
        "public exact-phrase fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;

    // Selection depends on all four exact byte streams. Authenticate every
    // material before parsing or creating the output directory so a late pin
    // failure cannot leave a plausible partial package behind.
    let materials = vec![
        verified_source_material(
            &options.source_declaration,
            source_text.as_bytes(),
            "public exact-phrase Rime source",
        )?,
        verified_source_material(
            &options.core_declaration,
            core_text.as_bytes(),
            "core public exact-phrase payload",
        )?,
        verified_source_material(
            &options.supplemental_declaration,
            supplemental_text.as_bytes(),
            "supplemental public exact-phrase payload",
        )?,
        verified_source_material(
            &options.fit_declaration,
            fit_text.as_bytes(),
            "public exact-phrase fit corpus",
        )?,
    ];
    if materials
        .iter()
        .map(CandidateSourceMaterial::sha256)
        .collect::<HashSet<_>>()
        .len()
        != materials.len()
    {
        return Err("build-exact-phrase-layer requires four distinct public materials".into());
    }

    let core_entries = parse_lexicon_tsv(&core_text)?;
    let supplemental_entries = parse_lexicon_tsv(&supplemental_text)?;
    let fit = parse_ud_conllu(&fit_text)?;
    let fit_spans = select_public_han_span_rank_probes(
        &fit,
        &core_entries,
        EXACT_PHRASE_CHARACTERS,
        EXACT_PHRASE_MAX_TOKENS,
    );
    let mut existing_entries = core_entries.clone();
    existing_entries.extend(supplemental_entries.iter().cloned());
    let selected = select_exact_phrase_source_entries(
        &source_text,
        &fit_spans.probes,
        &existing_entries,
        options.entry_limit,
    )?;
    if selected.entries.is_empty() {
        return Err("build-exact-phrase-layer selected no new public identities".into());
    }
    let stats = selected.stats;
    let mut entries = selected.entries;
    entries.sort_by(|left, right| {
        left.code
            .as_str()
            .cmp(right.code.as_str())
            .then_with(|| left.text.cmp(&right.text))
            .then_with(|| right.frequency.cmp(&left.frequency))
    });
    let payload = serialize_lexicon_payload(&entries);
    let mut report =
        write_multi_source_public_package(&options.output, &options.revision, materials, &payload)?;
    writeln!(
        report,
        "三字精确层：训练可编码 span {}（身份 {}）；来源匹配身份 {}；多音词面 {}；来源同码歧义 {}；既有身份 {}；上限外 {}；写入 {} 条",
        fit_spans.code_coverable_instances,
        fit_spans.code_coverable_identities,
        stats.matched_identities,
        stats.ambiguous_surfaces,
        stats.ambiguous_codes,
        stats.existing_identities,
        stats.dropped_by_entry_cap,
        entries.len(),
    )?;
    writeln!(
        report,
        "运行时状态：独立实验包；未接入、未安装、未启用；当前 TSF 候选保持不变"
    )?;
    Ok(report)
}

struct MergedPublicLexicon {
    payload: String,
    base_entries: usize,
    overlay_entries: usize,
    appended_entries: usize,
    duplicate_entries: usize,
}

const PUBLIC_MISS_VISIBLE_LIMIT: usize = MAX_CANDIDATE_SNAPSHOT_RANK;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicMissDiagnosis {
    WholeWordVisible {
        rank: usize,
    },
    WholeWordOutsideVisible,
    SourceWholeWordExcluded {
        minimum_segments: Option<usize>,
        visible_rank: Option<usize>,
    },
    CompositionVisible {
        segments: usize,
        rank: usize,
    },
    CompositionCrowded {
        segments: usize,
    },
    Unexplained,
}

fn diagnose_public_miss(
    source: &Path,
    core_package: &Path,
    supplemental_package: &Path,
    code: &str,
    target_text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        source,
        "public Rime target-audit source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let source_sha256 = candidate_sha256_hex(source_text.as_bytes());
    let source_audit = audit_public_rime_target(&source_text, target_text, code)?;
    let core = load_public_package_directory(core_package)?;
    let supplemental = load_public_package_directory(supplemental_package)?;
    let mut entries = parse_lexicon_tsv(&core.payload_text)?;
    let supplemental_entries = parse_lexicon_tsv(&supplemental.payload_text)?;
    let core_whole_word = entries
        .iter()
        .any(|entry| entry.text == target_text && entry.code.as_str() == code);
    let supplemental_whole_word = supplemental_entries
        .iter()
        .any(|entry| entry.text == target_text && entry.code.as_str() == code);
    entries.extend(supplemental_entries);
    let package_whole_word = core_whole_word || supplemental_whole_word;
    let minimum_segments = minimum_exact_public_segments(&entries, target_text, code);
    // Match the conservative runtime path. The broader consensus reorder is
    // available only through `consensus-audit` after failing its holdout gate.
    let candidates = layered_candidate_texts(
        &core.snapshot,
        &supplemental.snapshot,
        code,
        PUBLIC_MISS_VISIBLE_LIMIT,
        SupplementalCandidateLayerConfig {
            exact_promotions: 1,
        },
    )?;
    let visible_rank = candidates
        .iter()
        .position(|candidate| candidate == target_text)
        .map(|index| index + 1);
    let diagnosis = classify_public_miss(
        source_audit.exact_code_rows != 0,
        package_whole_word,
        minimum_segments,
        visible_rank,
    );

    let mut output = String::new();
    writeln!(output, "公开漏词分诊")?;
    writeln!(output, "输入：{code} → {target_text}")?;
    writeln!(output, "来源 SHA-256：{source_sha256}")?;
    writeln!(output, "核心版本：{}", core.snapshot.revision())?;
    writeln!(output, "补充版本：{}", supplemental.snapshot.revision())?;
    match source_audit.highest_exact_frequency {
        Some(frequency) => writeln!(
            output,
            "公开来源完整同码词：有（{} 行；最高权重 {}）",
            source_audit.exact_code_rows, frequency
        )?,
        None if source_audit.surface_rows != 0 => writeln!(
            output,
            "公开来源完整同码词：无（同文字 {} 行，但读音或规范码不符）",
            source_audit.surface_rows
        )?,
        None => writeln!(output, "公开来源完整同码词：无")?,
    }
    writeln!(
        output,
        "核心完整同码词：{}",
        if core_whole_word { "有" } else { "无" }
    )?;
    writeln!(
        output,
        "补充完整同码词：{}",
        if supplemental_whole_word {
            "有"
        } else {
            "无"
        }
    )?;
    match minimum_segments {
        Some(segments) => writeln!(output, "包内最少完整分段：{segments}")?,
        None => writeln!(output, "包内最少完整分段：无")?,
    }
    match visible_rank {
        Some(rank) => writeln!(output, "前 {PUBLIC_MISS_VISIBLE_LIMIT}：第 {rank} 名")?,
        None => writeln!(output, "前 {PUBLIC_MISS_VISIBLE_LIMIT}：未出现")?,
    }
    writeln!(output, "判断：{}", public_miss_diagnosis_text(diagnosis))?;
    writeln!(
        output,
        "边界：只核对显式公开目标、一个固定来源与两个公开候选层；不读取个人数据，不推断用户意图。"
    )?;
    writeln!(output, "本次操作：只读")?;
    Ok(output)
}

fn classify_public_miss(
    source_whole_word: bool,
    package_whole_word: bool,
    minimum_segments: Option<usize>,
    visible_rank: Option<usize>,
) -> PublicMissDiagnosis {
    if package_whole_word {
        return visible_rank.map_or(PublicMissDiagnosis::WholeWordOutsideVisible, |rank| {
            PublicMissDiagnosis::WholeWordVisible { rank }
        });
    }
    if source_whole_word {
        return PublicMissDiagnosis::SourceWholeWordExcluded {
            minimum_segments,
            visible_rank,
        };
    }
    match (minimum_segments, visible_rank) {
        (Some(segments), Some(rank)) => PublicMissDiagnosis::CompositionVisible { segments, rank },
        (Some(segments), None) => PublicMissDiagnosis::CompositionCrowded { segments },
        (None, _) => PublicMissDiagnosis::Unexplained,
    }
}

fn public_miss_diagnosis_text(diagnosis: PublicMissDiagnosis) -> String {
    match diagnosis {
        PublicMissDiagnosis::WholeWordVisible { rank } => {
            format!("完整词已经收录，并在可见范围第 {rank} 名")
        }
        PublicMissDiagnosis::WholeWordOutsideVisible => {
            "完整词已经收录，但没有进入前 50；属于候选生成或排序问题".to_owned()
        }
        PublicMissDiagnosis::SourceWholeWordExcluded {
            minimum_segments: Some(segments),
            visible_rank: Some(rank),
        } => format!(
            "公开来源存在完整同码词，但候选层没有收录；当前 {segments} 段组合位于第 {rank} 名，属于构建选择缺口"
        ),
        PublicMissDiagnosis::SourceWholeWordExcluded {
            minimum_segments: Some(segments),
            visible_rank: None,
        } => format!(
            "公开来源存在完整同码词，但候选层没有收录；当前 {segments} 段组合未进入前 50，属于构建选择缺口"
        ),
        PublicMissDiagnosis::SourceWholeWordExcluded {
            minimum_segments: None,
            visible_rank: Some(rank),
        } => format!(
            "公开来源存在完整同码词，但候选层没有收录；目标通过非完整分段路径位于第 {rank} 名，属于构建选择缺口"
        ),
        PublicMissDiagnosis::SourceWholeWordExcluded {
            minimum_segments: None,
            visible_rank: None,
        } => "公开来源存在完整同码词，但候选层没有收录，也没有完整分段路径；属于构建选择缺口"
            .to_owned(),
        PublicMissDiagnosis::CompositionVisible { segments, rank } => {
            format!("没有完整词条，但可由 {segments} 个包内完整段组成，并在可见范围第 {rank} 名")
        }
        PublicMissDiagnosis::CompositionCrowded { segments } => format!(
            "没有完整词条；可由 {segments} 个包内完整段组成，但没有进入前 50，属于组合召回或排序拥挤"
        ),
        PublicMissDiagnosis::Unexplained => {
            "当前来源无完整同码词，候选包也无法按完整段解释；需要补充来源或检查编码".to_owned()
        }
    }
}

fn minimum_exact_public_segments(
    entries: &[LexiconEntry],
    target_text: &str,
    code: &str,
) -> Option<usize> {
    let mut text_boundaries = target_text
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    text_boundaries.push(target_text.len());
    let character_count = text_boundaries.len().checked_sub(1)?;
    if character_count == 0 || code.len() != character_count * 2 {
        return None;
    }
    let identities = entries
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let mut best = vec![None::<usize>; character_count + 1];
    best[0] = Some(0);
    for start in 0..character_count {
        let Some(prefix_segments) = best[start] else {
            continue;
        };
        for end in start + 1..=character_count {
            let text = &target_text[text_boundaries[start]..text_boundaries[end]];
            let code = &code[start * 2..end * 2];
            if !identities.contains(&(text, code)) {
                continue;
            }
            let segments = prefix_segments + 1;
            if best[end].is_none_or(|current| segments < current) {
                best[end] = Some(segments);
            }
        }
    }
    best[character_count]
}

fn merge_public_packages(
    base: &Path,
    overlay: &Path,
    output: &Path,
    revision: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    // Refuse an existing destination before doing the comparatively expensive
    // package loads. The writer repeats this check immediately before creation.
    ensure_path_absent(output, "package output")?;
    let base = load_public_package_directory(base)?;
    let overlay = load_public_package_directory(overlay)?;
    let materials = merge_public_source_materials(&base.provenance, &overlay.provenance)?;
    let merged = merge_public_lexicon_payloads(
        &base.payload_text,
        &overlay.payload_text,
        MAX_CANDIDATE_SNAPSHOT_ENTRIES,
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let mut report =
        write_multi_source_public_package(output, revision, materials, &merged.payload)?;
    writeln!(
        report,
        "公开包合并：基础 {} 条；叠加 {} 条；新增 {} 条；重复 {} 条",
        merged.base_entries,
        merged.overlay_entries,
        merged.appended_entries,
        merged.duplicate_entries,
    )?;
    writeln!(report, "顺序：基础保持不变；新增项按叠加包原顺序追加")?;
    Ok(report)
}

fn merge_public_source_materials(
    base: &CandidatePackageProvenance,
    overlay: &CandidatePackageProvenance,
) -> Result<Vec<CandidateSourceMaterial>, Box<dyn std::error::Error>> {
    let mut by_id = HashMap::<String, CandidateSourceMaterial>::new();
    for material in base
        .source_materials()
        .iter()
        .chain(overlay.source_materials())
    {
        if let Some(existing) = by_id.get(material.id()) {
            if existing != material {
                return Err("public package source declarations conflict for one source id".into());
            }
            continue;
        }
        by_id.insert(material.id().to_owned(), material.clone());
    }
    let materials = by_id.into_values().collect::<Vec<_>>();
    if materials.len() < 2 {
        return Err("merged public package requires at least two distinct source materials".into());
    }
    Ok(materials)
}

fn merge_public_lexicon_payloads(
    base_payload: &str,
    overlay_payload: &str,
    max_entries: usize,
    max_bytes: usize,
) -> Result<MergedPublicLexicon, Box<dyn std::error::Error>> {
    let mut base_entries = parse_lexicon_tsv(base_payload)?;
    let overlay_entries = parse_lexicon_tsv(overlay_payload)?;
    let base_entry_count = base_entries.len();
    let overlay_entry_count = overlay_entries.len();
    let mut identities = base_entries
        .iter()
        .map(|entry| (entry.text.clone(), entry.code.as_str().to_owned()))
        .collect::<HashSet<_>>();
    let mut appended_entries = 0;
    let mut duplicate_entries = 0;
    for entry in overlay_entries {
        let identity = (entry.text.clone(), entry.code.as_str().to_owned());
        if identities.insert(identity) {
            base_entries.push(entry);
            appended_entries += 1;
        } else {
            duplicate_entries += 1;
        }
    }
    if base_entries.len() > max_entries {
        return Err("merged public package exceeds the fixed entry limit".into());
    }
    let payload = serialize_lexicon_payload(&base_entries);
    if payload.len() > max_bytes {
        return Err("merged public package exceeds the fixed payload byte limit".into());
    }
    Ok(MergedPublicLexicon {
        payload,
        base_entries: base_entry_count,
        overlay_entries: overlay_entry_count,
        appended_entries,
        duplicate_entries,
    })
}

fn verified_source_material(
    declaration: &PublicSourceDeclaration,
    bytes: &[u8],
    label: &str,
) -> Result<CandidateSourceMaterial, Box<dyn std::error::Error>> {
    let material = CandidateSourceMaterial::new(
        &declaration.id,
        &declaration.license,
        &declaration.url,
        &declaration.sha256,
    )?;
    material
        .validate_bytes(bytes)
        .map_err(|_| format!("{label} SHA-256 does not match the explicit pin"))?;
    Ok(material)
}

fn compare_payloads(
    base_payload: &Path,
    challenger_payload: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let challenger_text = read_explicit_text(
        challenger_payload,
        "challenger public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let base = parse_lexicon_tsv(&base_text)?;
    let challenger = parse_lexicon_tsv(&challenger_text)?;
    let report = compare_public_lexicons(&base, &challenger);
    Ok(format!(
        "公开词表对照\n基线词条：{}\n对照词条：{}\n共同词形：{}\n仅基线词形：{}\n仅对照词形：{}\n共同文字与规范码：{}\n共同规范码：{}\n同码首选相同：{}\n同码首选不同：{}\n其中对照首选也被基线同码确认：{}\n  原本第 2 名：{}\n  原本第 3–6 名：{}\n  原本第 7 名以后：{}\n本次操作：只读\n",
        report.base_entries,
        report.challenger_entries,
        report.shared_surface_texts,
        report.base_only_surface_texts,
        report.challenger_only_surface_texts,
        report.shared_text_code_identities,
        report.shared_codes,
        report.same_top_text_codes,
        report.changed_top_text_codes,
        report.consensus_top_reorder_eligible_codes,
        report.consensus_top_original_rank_two_codes,
        report.consensus_top_original_rank_three_to_six_codes,
        report.consensus_top_original_rank_seven_or_later_codes,
    ))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicConsensusRankAudit {
    probes: usize,
    instances: usize,
    before_correct_top: usize,
    before_correct_top_instances: usize,
    after_correct_top: usize,
    after_correct_top_instances: usize,
    order_changes: usize,
    order_change_instances: usize,
    top_changes: usize,
    top_change_instances: usize,
    correct_top_gained: usize,
    correct_top_gained_instances: usize,
    correct_top_lost: usize,
    correct_top_lost_instances: usize,
    non_target_top_changes: usize,
    non_target_top_change_instances: usize,
    target_movement: WeightedRankMovement,
}

impl PublicConsensusRankAudit {
    fn observe(&mut self, before: &[String], after: &[String], expected: &str, instances: usize) {
        let before_rank = candidate_rank(before, expected);
        let after_rank = candidate_rank(after, expected);
        let before_correct = before
            .first()
            .is_some_and(|candidate| candidate == expected);
        let after_correct = after.first().is_some_and(|candidate| candidate == expected);
        let order_changed = before != after;
        let top_changed = before.first() != after.first();

        self.probes += 1;
        self.instances += instances;
        self.before_correct_top += usize::from(before_correct);
        self.before_correct_top_instances += instances * usize::from(before_correct);
        self.after_correct_top += usize::from(after_correct);
        self.after_correct_top_instances += instances * usize::from(after_correct);
        self.order_changes += usize::from(order_changed);
        self.order_change_instances += instances * usize::from(order_changed);
        self.top_changes += usize::from(top_changed);
        self.top_change_instances += instances * usize::from(top_changed);
        if top_changed && !before_correct && after_correct {
            self.correct_top_gained += 1;
            self.correct_top_gained_instances += instances;
        } else if top_changed && before_correct && !after_correct {
            self.correct_top_lost += 1;
            self.correct_top_lost_instances += instances;
        } else if top_changed {
            self.non_target_top_changes += 1;
            self.non_target_top_change_instances += instances;
        }
        self.target_movement
            .observe(before_rank, after_rank, instances);
    }

    fn absorb(&mut self, other: Self) {
        self.probes += other.probes;
        self.instances += other.instances;
        self.before_correct_top += other.before_correct_top;
        self.before_correct_top_instances += other.before_correct_top_instances;
        self.after_correct_top += other.after_correct_top;
        self.after_correct_top_instances += other.after_correct_top_instances;
        self.order_changes += other.order_changes;
        self.order_change_instances += other.order_change_instances;
        self.top_changes += other.top_changes;
        self.top_change_instances += other.top_change_instances;
        self.correct_top_gained += other.correct_top_gained;
        self.correct_top_gained_instances += other.correct_top_gained_instances;
        self.correct_top_lost += other.correct_top_lost;
        self.correct_top_lost_instances += other.correct_top_lost_instances;
        self.non_target_top_changes += other.non_target_top_changes;
        self.non_target_top_change_instances += other.non_target_top_change_instances;
        self.target_movement.absorb(other.target_movement);
    }

    fn gate_passed(self) -> bool {
        self.correct_top_gained != 0
            && self.correct_top_lost == 0
            && self.non_target_top_changes == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicConsensusComparison {
    overall: PublicConsensusRankAudit,
    by_core_exact_width: [PublicConsensusRankAudit; 3],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicConsensusLengthAudit {
    characters: usize,
    source_unique_tokens: usize,
    source_token_instances: usize,
    matched_unique_tokens: usize,
    matched_token_instances: usize,
    ambiguous_unique_tokens: usize,
    ambiguous_token_instances: usize,
    ranking: PublicConsensusRankAudit,
}

fn audit_public_consensus(
    core_payload: &Path,
    supplemental_payload: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public consensus payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        supplemental_payload,
        "supplemental public consensus payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public consensus held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let core_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let supplemental_sha256 = candidate_sha256_hex(supplemental_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if core_sha256 == supplemental_sha256 {
        return Err("consensus-audit requires two distinct candidate payloads".into());
    }
    if held_out_sha256 == core_sha256 || held_out_sha256 == supplemental_sha256 {
        return Err("consensus-audit requires an independent held-out corpus".into());
    }

    let core_entries = parse_lexicon_tsv(&core_text)?;
    let core = snapshot_from_payload("consensus-audit-core-v1", &core_text)?;
    let supplemental =
        snapshot_from_payload("consensus-audit-supplemental-v1", &supplemental_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let ambiguous_surfaces = ambiguous_lexicon_surfaces(&core_entries);
    let mut lengths = Vec::new();
    let mut overall = PublicConsensusRankAudit::default();
    let mut by_core_exact_width = [PublicConsensusRankAudit::default(); 3];
    for characters in 1..=4 {
        let selection = select_public_lexicon_rank_probes(&held_out, &core_entries, characters);
        let matched_unique_tokens = selection.probes.len();
        let mut ambiguous_unique_tokens = 0;
        let mut ambiguous_token_instances = 0;
        let probes = selection
            .probes
            .into_iter()
            .filter(|probe| {
                let ambiguous = ambiguous_surfaces.contains(probe.expected_text.as_str());
                ambiguous_unique_tokens += usize::from(ambiguous);
                ambiguous_token_instances += probe.instances * usize::from(ambiguous);
                !ambiguous
            })
            .collect::<Vec<_>>();
        let comparison =
            compare_public_consensus_ranks(&core, &supplemental, &probes, frontier_limit)?;
        overall.absorb(comparison.overall);
        for (total, row) in by_core_exact_width
            .iter_mut()
            .zip(comparison.by_core_exact_width)
        {
            total.absorb(row);
        }
        lengths.push(PublicConsensusLengthAudit {
            characters,
            source_unique_tokens: selection.source_unique_tokens,
            source_token_instances: selection.source_token_instances,
            matched_unique_tokens,
            matched_token_instances: selection.matched_token_instances,
            ambiguous_unique_tokens,
            ambiguous_token_instances,
            ranking: comparison.overall,
        });
    }

    let mut output = String::new();
    writeln!(output, "公开共识排序独立留出审计")?;
    writeln!(
        output,
        "核心词条：{} · SHA-256 {core_sha256}",
        core_entries.len()
    )?;
    writeln!(
        output,
        "补充词条：{} · SHA-256 {supplemental_sha256}",
        supplemental.entry_count()
    )?;
    writeln!(
        output,
        "独立留出：{} 句，{} 个句法 token · SHA-256 {held_out_sha256}",
        held_out.stats.sentences, held_out.stats.syntactic_tokens
    )?;
    writeln!(
        output,
        "候选范围：前 {frontier_limit}；每码最多补 1 个完整词"
    )?;
    for length in lengths {
        writeln!(
            output,
            "{} 字：语料词面 {}（实例 {}）；核心匹配 {}（实例 {}）；多音排除 {}（实例 {}）；评测 {}（实例 {}）；正确首选新增 {}、丢失 {}、非目标首选变化 {}",
            length.characters,
            length.source_unique_tokens,
            length.source_token_instances,
            length.matched_unique_tokens,
            length.matched_token_instances,
            length.ambiguous_unique_tokens,
            length.ambiguous_token_instances,
            length.ranking.probes,
            length.ranking.instances,
            length.ranking.correct_top_gained,
            length.ranking.correct_top_lost,
            length.ranking.non_target_top_changes,
        )?;
    }
    writeln!(
        output,
        "合计：评测 {}（实例 {}）；校准前正确首选 {}（实例 {}），校准后 {}（实例 {}）",
        overall.probes,
        overall.instances,
        overall.before_correct_top,
        overall.before_correct_top_instances,
        overall.after_correct_top,
        overall.after_correct_top_instances,
    )?;
    writeln!(
        output,
        "候选顺序变化 {}（实例 {}）；首选变化 {}（实例 {}）；正确首选新增 {}（实例 {}），丢失 {}（实例 {}），非目标首选变化 {}（实例 {}）",
        overall.order_changes,
        overall.order_change_instances,
        overall.top_changes,
        overall.top_change_instances,
        overall.correct_top_gained,
        overall.correct_top_gained_instances,
        overall.correct_top_lost,
        overall.correct_top_lost_instances,
        overall.non_target_top_changes,
        overall.non_target_top_change_instances,
    )?;
    writeln!(
        output,
        "目标名次：改善 {}（实例 {}），不变 {}（实例 {}），变差 {}（实例 {}）；新进入范围 {}（实例 {}），掉出范围 {}（实例 {}）",
        overall.target_movement.improved,
        overall.target_movement.improved_instances,
        overall.target_movement.unchanged,
        overall.target_movement.unchanged_instances,
        overall.target_movement.worsened,
        overall.target_movement.worsened_instances,
        overall.target_movement.newly_visible,
        overall.target_movement.newly_visible_instances,
        overall.target_movement.lost_visible,
        overall.target_movement.lost_visible_instances,
    )?;
    writeln!(output, "按核心精确同码宽度：")?;
    for (label, row) in ["1", "2～6", "≥7"].into_iter().zip(by_core_exact_width) {
        writeln!(
            output,
            "  {label}：评测 {}（实例 {}）；首选变化 {}（实例 {}）；正确首选新增 {}（实例 {}）、丢失 {}（实例 {}）、非目标首选变化 {}（实例 {}）",
            row.probes,
            row.instances,
            row.top_changes,
            row.top_change_instances,
            row.correct_top_gained,
            row.correct_top_gained_instances,
            row.correct_top_lost,
            row.correct_top_lost_instances,
            row.non_target_top_changes,
            row.non_target_top_change_instances,
        )?;
    }
    writeln!(
        output,
        "安全门：{}",
        if overall.gate_passed() {
            "通过（至少新增一个正确首选；正确首选零损失；非目标首选零变化）"
        } else {
            "未通过（需要至少一个新增正确首选、正确首选零损失且非目标首选零变化）"
        }
    )?;
    output.push_str(
        "口径：只评测独立公开语料中、核心词典可确认且只有一个规范码的 1～4 字 token；语料不参与规则选择。报告不显示词面，不比较跨词典原始权重，不写文件、不改候选槽位。\n本次操作：只读\n",
    );
    Ok(output)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicShortExactRankAudit {
    probes: usize,
    instances: usize,
    top: usize,
    top_instances: usize,
    within_frontier: usize,
    within_frontier_instances: usize,
    rank_two: usize,
    rank_three_to_six: usize,
    rank_seven_to_fifty: usize,
    beyond_fifty: usize,
    sole_exact_candidate: usize,
    two_to_six_exact_candidates: usize,
    seven_or_more_exact_candidates: usize,
}

impl PublicShortExactRankAudit {
    fn observe(
        &mut self,
        exact_candidates: &[String],
        expected: &str,
        instances: usize,
        frontier_limit: usize,
    ) {
        let rank = candidate_rank(exact_candidates, expected);
        self.probes += 1;
        self.instances += instances;
        self.top += usize::from(rank == Some(1));
        self.top_instances += instances * usize::from(rank == Some(1));
        self.within_frontier += usize::from(rank.is_some_and(|rank| rank <= frontier_limit));
        self.within_frontier_instances +=
            instances * usize::from(rank.is_some_and(|rank| rank <= frontier_limit));
        match rank {
            Some(1) => {}
            Some(2) => self.rank_two += 1,
            Some(3..=6) => self.rank_three_to_six += 1,
            Some(7..=MAX_CANDIDATE_SNAPSHOT_RANK) => self.rank_seven_to_fifty += 1,
            Some(_) => unreachable!("candidate snapshot rank is bounded"),
            None => self.beyond_fifty += 1,
        }
        match exact_candidates.len() {
            1 => self.sole_exact_candidate += 1,
            2..=6 => self.two_to_six_exact_candidates += 1,
            7.. => self.seven_or_more_exact_candidates += 1,
            0 => unreachable!("a selected core lexicon target has an exact candidate"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicShortPrefixAudit {
    completed_probes: usize,
    completed_instances: usize,
    completed_top: usize,
    completed_visible: usize,
    continuing_probes: usize,
    continuing_instances: usize,
    continuing_top: usize,
    continuing_visible: usize,
}

impl PublicShortPrefixAudit {
    fn observe(&mut self, completed: bool, rank: Option<usize>, instances: usize) {
        if completed {
            self.completed_probes += 1;
            self.completed_instances += instances;
            self.completed_top += usize::from(rank == Some(1));
            self.completed_visible += usize::from(rank.is_some());
        } else {
            self.continuing_probes += 1;
            self.continuing_instances += instances;
            self.continuing_top += usize::from(rank == Some(1));
            self.continuing_visible += usize::from(rank.is_some());
        }
    }
}

fn audit_public_short_ranks(
    core_payload: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public short-rank payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public short-rank held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let core_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if core_sha256 == held_out_sha256 {
        return Err("short-rank-audit requires an independent held-out corpus".into());
    }

    let core_entries = parse_lexicon_tsv(&core_text)?;
    let core = snapshot_from_payload("short-rank-audit-core-v1", &core_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let ambiguous_surfaces = ambiguous_lexicon_surfaces(&core_entries);

    let mut source_unique_tokens = 0;
    let mut source_token_instances = 0;
    let mut matched_unique_tokens = 0;
    let mut matched_token_instances = 0;
    let mut ambiguous_unique_tokens = 0;
    let mut ambiguous_token_instances = 0;
    let mut long_code_unique_tokens = 0;
    let mut long_code_token_instances = 0;
    let mut probes = Vec::new();
    for characters in 1..=2 {
        let selection = select_public_lexicon_rank_probes(&held_out, &core_entries, characters);
        source_unique_tokens += selection.source_unique_tokens;
        source_token_instances += selection.source_token_instances;
        matched_unique_tokens += selection.probes.len();
        matched_token_instances += selection.matched_token_instances;
        for probe in selection.probes {
            if ambiguous_surfaces.contains(probe.expected_text.as_str()) {
                ambiguous_unique_tokens += 1;
                ambiguous_token_instances += probe.instances;
            } else if probe.observed.as_str().len() > 4 {
                long_code_unique_tokens += 1;
                long_code_token_instances += probe.instances;
            } else {
                probes.push(probe);
            }
        }
    }

    let mut exact_rows = BTreeMap::<(usize, usize), PublicShortExactRankAudit>::new();
    let mut prefix_rows = [PublicShortPrefixAudit::default(); 4];
    for probe in &probes {
        let code = probe.observed.as_str();
        let exact_candidates = core.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
        exact_rows
            .entry((code.len(), probe.expected_text.chars().count()))
            .or_default()
            .observe(
                &exact_candidates,
                &probe.expected_text,
                probe.instances,
                frontier_limit,
            );

        for typed_keys in 1..=code.len() {
            let prefix = &code[..typed_keys];
            let candidates = core.candidate_texts(prefix, frontier_limit)?;
            prefix_rows[typed_keys - 1].observe(
                typed_keys == code.len(),
                candidate_rank(&candidates, &probe.expected_text),
                probe.instances,
            );
        }
    }

    let mut output = String::new();
    writeln!(output, "公开短输入核心排序审计")?;
    writeln!(
        output,
        "核心词条：{} · SHA-256 {core_sha256}",
        core_entries.len()
    )?;
    writeln!(
        output,
        "独立留出：{} 句，{} 个句法 token · SHA-256 {held_out_sha256}",
        held_out.stats.sentences, held_out.stats.syntactic_tokens
    )?;
    writeln!(
        output,
        "选样：1～2 字公开词面 {}（实例 {}）；核心匹配 {}（实例 {}）；多音排除 {}（实例 {}）；超过四键排除 {}（实例 {}）；评测 {}（实例 {}）",
        source_unique_tokens,
        source_token_instances,
        matched_unique_tokens,
        matched_token_instances,
        ambiguous_unique_tokens,
        ambiguous_token_instances,
        long_code_unique_tokens,
        long_code_token_instances,
        probes.len(),
        probes.iter().map(|probe| probe.instances).sum::<usize>(),
    )?;
    writeln!(output, "逐键前沿（前 {frontier_limit}）：")?;
    for (index, row) in prefix_rows.into_iter().enumerate() {
        let typed_keys = index + 1;
        writeln!(
            output,
            "  {typed_keys} 键：已完成 {}（实例 {}），目标首选 {}、可见 {}；仍在输入 {}（实例 {}），目标预览首选 {}、可见 {}",
            row.completed_probes,
            row.completed_instances,
            row.completed_top,
            row.completed_visible,
            row.continuing_probes,
            row.continuing_instances,
            row.continuing_top,
            row.continuing_visible,
        )?;
    }
    writeln!(output, "完整码核心同码次序：")?;
    for ((keys, characters), row) in exact_rows {
        writeln!(
            output,
            "  {keys} 键 / {characters} 字：评测 {}（实例 {}）；首选 {}（实例 {}），前 {frontier_limit} 可见 {}（实例 {}）；第 2 名 {}、第 3～6 名 {}、第 7～50 名 {}、50 名外 {}；同码宽度 1 / 2～6 / ≥7：{} / {} / {}",
            row.probes,
            row.instances,
            row.top,
            row.top_instances,
            row.within_frontier,
            row.within_frontier_instances,
            row.rank_two,
            row.rank_three_to_six,
            row.rank_seven_to_fifty,
            row.beyond_fifty,
            row.sole_exact_candidate,
            row.two_to_six_exact_candidates,
            row.seven_or_more_exact_candidates,
        )?;
    }
    output.push_str(
        "口径：目标只来自独立公开 UD 留出中、核心词典可确认且只有一个规范码的 1～2 字 token；逐键前沿把未完成规范码称为“预览”，不把它冒充已经表达完的用户意图。完整码次序只读取核心精确词候选，不混入补充层、个人记忆或纠错。报告不显示词面，不写文件、不改候选槽位。\n本次操作：只读\n",
    );
    Ok(output)
}

const SEGMENT_PENALTY_PROFILES_MILLI: [u32; 7] = [0, 10, 25, 50, 100, 250, 500];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PublicSegmentPenaltyAudit {
    probes: usize,
    baseline_at_one: usize,
    reranked_at_one: usize,
    baseline_visible: usize,
    reranked_visible: usize,
    correct_top_gained: usize,
    correct_top_lost: usize,
    newly_visible: usize,
    lost_visible: usize,
    non_target_top_changes: usize,
    target_rank_improved: usize,
    target_rank_unchanged: usize,
    target_rank_worsened: usize,
    target_outside_pool: usize,
}

impl PublicSegmentPenaltyAudit {
    fn observe(
        &mut self,
        baseline: &[SentenceCandidate],
        reranked: &[usize],
        expected: &str,
        frontier_limit: usize,
    ) {
        let baseline_rank = baseline
            .iter()
            .position(|candidate| candidate.text == expected)
            .map(|index| index + 1);
        let reranked_rank = reranked
            .iter()
            .position(|index| baseline[*index].text == expected)
            .map(|index| index + 1);
        let baseline_top = baseline.first().map(|candidate| candidate.text.as_str());
        let reranked_top = reranked.first().map(|index| baseline[*index].text.as_str());
        self.probes += 1;
        self.baseline_at_one += usize::from(baseline_rank == Some(1));
        self.reranked_at_one += usize::from(reranked_rank == Some(1));
        self.baseline_visible +=
            usize::from(baseline_rank.is_some_and(|rank| rank <= frontier_limit));
        self.reranked_visible +=
            usize::from(reranked_rank.is_some_and(|rank| rank <= frontier_limit));
        self.correct_top_gained +=
            usize::from(baseline_rank != Some(1) && reranked_rank == Some(1));
        self.correct_top_lost += usize::from(baseline_rank == Some(1) && reranked_rank != Some(1));
        self.newly_visible += usize::from(
            baseline_rank.is_none_or(|rank| rank > frontier_limit)
                && reranked_rank.is_some_and(|rank| rank <= frontier_limit),
        );
        self.lost_visible += usize::from(
            baseline_rank.is_some_and(|rank| rank <= frontier_limit)
                && reranked_rank.is_none_or(|rank| rank > frontier_limit),
        );
        self.non_target_top_changes += usize::from(
            baseline_top != reranked_top && baseline_rank != Some(1) && reranked_rank != Some(1),
        );
        match (baseline_rank, reranked_rank) {
            (Some(before), Some(after)) if after < before => self.target_rank_improved += 1,
            (Some(before), Some(after)) if after > before => self.target_rank_worsened += 1,
            (Some(_), Some(_)) => self.target_rank_unchanged += 1,
            (None, None) => self.target_outside_pool += 1,
            (None, Some(_)) => self.target_rank_improved += 1,
            (Some(_), None) => self.target_rank_worsened += 1,
        }
    }

    fn safe(self) -> bool {
        self.correct_top_lost == 0 && self.lost_visible == 0 && self.non_target_top_changes == 0
    }
}

fn audit_public_segment_penalty(
    core_payload: &Path,
    fit_corpus: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
    sample_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public segment-penalty payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public segment-penalty fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public segment-penalty held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let core_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let fit_sha256 = candidate_sha256_hex(fit_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if fit_sha256 == held_out_sha256 {
        return Err("segment-penalty-audit requires a distinct held-out corpus".into());
    }
    if core_sha256 == fit_sha256 || core_sha256 == held_out_sha256 {
        return Err("segment-penalty-audit requires separate lexicon and corpus materials".into());
    }

    let core_entries = parse_lexicon_tsv(&core_text)?;
    let decoder = Decoder::new(core_entries.clone());
    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let fit_selection =
        select_public_continuous_composition_cases(&fit, &core_entries, sample_limit);
    let held_out_selection =
        select_public_continuous_composition_cases(&held_out, &core_entries, sample_limit);
    if fit_selection.probes.is_empty() || held_out_selection.probes.is_empty() {
        return Err("segment-penalty-audit public corpora produced no eligible probes".into());
    }

    let fit_reports = evaluate_public_segment_penalty_profiles(
        &decoder,
        &fit_selection.probes,
        frontier_limit,
        &SEGMENT_PENALTY_PROFILES_MILLI,
    )?;
    let mut selected_index = 0;
    for index in 1..fit_reports.len() {
        if public_segment_penalty_profile_precedes(
            fit_reports[index],
            SEGMENT_PENALTY_PROFILES_MILLI[index],
            fit_reports[selected_index],
            SEGMENT_PENALTY_PROFILES_MILLI[selected_index],
        ) {
            selected_index = index;
        }
    }
    let selected_penalty = SEGMENT_PENALTY_PROFILES_MILLI[selected_index];
    let held_out_reports = if selected_penalty == 0 {
        evaluate_public_segment_penalty_profiles(
            &decoder,
            &held_out_selection.probes,
            frontier_limit,
            &[0],
        )?
    } else {
        evaluate_public_segment_penalty_profiles(
            &decoder,
            &held_out_selection.probes,
            frontier_limit,
            &[0, selected_penalty],
        )?
    };
    let held_out_baseline = held_out_reports[0];
    let held_out_selected = held_out_reports[held_out_reports.len() - 1];
    let held_out_gate = held_out_selected.safe()
        && held_out_selected.correct_top_gained != 0
        && held_out_selected.reranked_at_one > held_out_baseline.baseline_at_one;

    let mut output = String::new();
    writeln!(output, "公开连续短语少分段惩罚审计")?;
    writeln!(
        output,
        "核心词条：{} · SHA-256 {core_sha256}；冻结候选池：前 {MAX_CANDIDATE_SNAPSHOT_RANK}；可见前沿：前 {frontier_limit}",
        core_entries.len(),
    )?;
    writeln!(
        output,
        "拟合：{} 句 · SHA-256 {fit_sha256}；候选窗口 {}，可覆盖 {}，句代表 {}，选取 {}",
        fit.stats.sentences,
        fit_selection.stats.source_windows,
        fit_selection.stats.exact_word_coverable,
        fit_selection.stats.sentence_representatives,
        fit_selection.stats.selected,
    )?;
    writeln!(
        output,
        "保留：{} 句 · SHA-256 {held_out_sha256}；候选窗口 {}，可覆盖 {}，句代表 {}，选取 {}",
        held_out.stats.sentences,
        held_out_selection.stats.source_windows,
        held_out_selection.stats.exact_word_coverable,
        held_out_selection.stats.sentence_representatives,
        held_out_selection.stats.selected,
    )?;
    writeln!(output, "拟合档位（每多一个词界扣分）：")?;
    for (penalty, report) in SEGMENT_PENALTY_PROFILES_MILLI.iter().zip(&fit_reports) {
        write_public_segment_penalty_report(&mut output, *penalty, *report, frontier_limit)?;
    }
    writeln!(
        output,
        "拟合选择：{}；先要求正确首选零损失、可见零损失、非目标首选零变化，再比较新增正确首选和可见召回；平局保留更小惩罚。",
        public_segment_penalty_label(selected_penalty),
    )?;
    writeln!(output, "保留评测：")?;
    write_public_segment_penalty_report(&mut output, 0, held_out_baseline, frontier_limit)?;
    if selected_penalty != 0 {
        write_public_segment_penalty_report(
            &mut output,
            selected_penalty,
            held_out_selected,
            frontier_limit,
        )?;
    }
    writeln!(
        output,
        "安全门：{}",
        if held_out_gate {
            "通过（保留集新增正确首选，且正确首选、可见目标和非目标首选均零损失）"
        } else {
            "未通过（不得把该少分段惩罚接入运行时）"
        }
    )?;
    output.push_str(
        "口径：只从公开 UD 相邻双词构造完整双拼短语；所有档位重排同一份现行 Top-50，不创建候选，不读取私人记录，也不按目标文字逐例调参。分段数只作为离线反事实，当前运行时排序未改变。\n本次操作：只读\n",
    );
    Ok(output)
}

fn evaluate_public_segment_penalty_profiles(
    decoder: &Decoder,
    probes: &[ContinuousCompositionProbe],
    frontier_limit: usize,
    penalty_profiles_milli: &[u32],
) -> Result<Vec<PublicSegmentPenaltyAudit>, Box<dyn std::error::Error>> {
    let mut reports = vec![PublicSegmentPenaltyAudit::default(); penalty_profiles_milli.len()];
    for probe in probes {
        let candidates =
            decoder.decode_sentence(probe.full_observed.as_str(), MAX_CANDIDATE_SNAPSHOT_RANK)?;
        for (report, penalty_milli) in reports.iter_mut().zip(penalty_profiles_milli) {
            let penalty = f64::from(*penalty_milli) / 1_000.0;
            let mut reranked = (0..candidates.len()).collect::<Vec<_>>();
            reranked.sort_by(|left, right| {
                let left_boundaries = candidates[*left].segments.len().saturating_sub(1) as f64;
                let right_boundaries = candidates[*right].segments.len().saturating_sub(1) as f64;
                let left_score = candidates[*left].total_score - penalty * left_boundaries;
                let right_score = candidates[*right].total_score - penalty * right_boundaries;
                right_score
                    .total_cmp(&left_score)
                    .then_with(|| left.cmp(right))
            });
            report.observe(&candidates, &reranked, &probe.expected_text, frontier_limit);
        }
    }
    Ok(reports)
}

fn public_segment_penalty_profile_precedes(
    challenger: PublicSegmentPenaltyAudit,
    challenger_penalty: u32,
    current: PublicSegmentPenaltyAudit,
    current_penalty: u32,
) -> bool {
    challenger
        .safe()
        .cmp(&current.safe())
        .then_with(|| {
            challenger
                .correct_top_gained
                .cmp(&current.correct_top_gained)
        })
        .then_with(|| challenger.reranked_at_one.cmp(&current.reranked_at_one))
        .then_with(|| challenger.reranked_visible.cmp(&current.reranked_visible))
        .then_with(|| {
            current
                .target_rank_worsened
                .cmp(&challenger.target_rank_worsened)
        })
        .then_with(|| current_penalty.cmp(&challenger_penalty))
        .is_gt()
}

fn public_segment_penalty_label(penalty_milli: u32) -> String {
    format!("每词界 {:.3}", f64::from(penalty_milli) / 1_000.0)
}

fn write_public_segment_penalty_report(
    output: &mut String,
    penalty_milli: u32,
    report: PublicSegmentPenaltyAudit,
    frontier_limit: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(
        output,
        "  {}：首选 {}/{}（基线 {}），新增正确首选 {}、丢失 {}、非目标首选变化 {}；Top-{frontier_limit} {}/{}（基线 {}），新进 {}、掉出 {}；目标名次改善 / 不变 / 变差 / 池外：{} / {} / {} / {}",
        public_segment_penalty_label(penalty_milli),
        report.reranked_at_one,
        report.probes,
        report.baseline_at_one,
        report.correct_top_gained,
        report.correct_top_lost,
        report.non_target_top_changes,
        report.reranked_visible,
        report.probes,
        report.baseline_visible,
        report.newly_visible,
        report.lost_visible,
        report.target_rank_improved,
        report.target_rank_unchanged,
        report.target_rank_worsened,
        report.target_outside_pool,
    )
}

fn ambiguous_lexicon_surfaces(entries: &[LexiconEntry]) -> HashSet<&str> {
    let mut first_code = HashMap::<&str, &str>::new();
    let mut ambiguous = HashSet::new();
    for entry in entries {
        match first_code.entry(entry.text.as_str()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(entry.code.as_str());
            }
            std::collections::hash_map::Entry::Occupied(slot)
                if *slot.get() != entry.code.as_str() =>
            {
                ambiguous.insert(entry.text.as_str());
            }
            _ => {}
        }
    }
    ambiguous
}

fn compare_public_consensus_ranks(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    probes: &[PublicLexiconRankProbe],
    frontier_limit: usize,
) -> Result<PublicConsensusComparison, Box<dyn std::error::Error>> {
    let config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let mut comparison = PublicConsensusComparison::default();
    for probe in probes {
        let before = layered_candidate_texts(
            core,
            supplemental,
            probe.observed.as_str(),
            frontier_limit,
            config,
        )?;
        let after = layered_candidate_texts_with_consensus(
            core,
            supplemental,
            probe.observed.as_str(),
            frontier_limit,
            config,
        )?;
        comparison
            .overall
            .observe(&before, &after, &probe.expected_text, probe.instances);
        let exact_width = core
            .exact_full_code_texts(probe.observed.as_str(), MAX_CANDIDATE_SNAPSHOT_RANK)?
            .len();
        let width_index = match exact_width {
            1 => 0,
            2..=6 => 1,
            7.. => 2,
            0 => unreachable!("a selected core lexicon target has an exact candidate"),
        };
        comparison.by_core_exact_width[width_index].observe(
            &before,
            &after,
            &probe.expected_text,
            probe.instances,
        );
    }
    Ok(comparison)
}

fn audit_length_coverage(
    base_payload: &Path,
    challenger_payload: &Path,
    fit_corpus: &Path,
    held_out_corpus: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let challenger_text = read_explicit_text(
        challenger_payload,
        "challenger public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let base_sha256 = candidate_sha256_hex(base_text.as_bytes());
    let challenger_sha256 = candidate_sha256_hex(challenger_text.as_bytes());
    let fit_sha256 = candidate_sha256_hex(fit_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if base_sha256 == challenger_sha256 {
        return Err("length-coverage-audit requires two distinct candidate payloads".into());
    }
    if fit_sha256 == held_out_sha256 {
        return Err("length-coverage-audit requires a distinct held-out corpus".into());
    }

    let base = parse_lexicon_tsv(&base_text)?;
    let challenger = parse_lexicon_tsv(&challenger_text)?;
    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let fit_audit = audit_public_lexicon_token_coverage(&fit, &base, &challenger);
    let held_out_audit = audit_public_lexicon_token_coverage(&held_out, &base, &challenger);

    let mut output = format!(
        "公开词面长度覆盖留出审计\n候选载荷：基线 {} 条 · SHA-256 {base_sha256}；对照 {} 条 · SHA-256 {challenger_sha256}\n",
        base.len(),
        challenger.len(),
    );
    write_length_coverage_section(&mut output, "训练侧参考", &fit_sha256, fit.stats, fit_audit);
    write_length_coverage_section(
        &mut output,
        "留出评测",
        &held_out_sha256,
        held_out.stats,
        held_out_audit,
    );
    output.push_str(
        "口径：只比较二至四字 Han token 的公开词面覆盖；不推断读音，不比较跨来源权重，不显示词条文字。\n本次操作：只读\n",
    );
    Ok(output)
}

fn write_length_coverage_section(
    output: &mut String,
    label: &str,
    corpus_sha256: &str,
    corpus_stats: UdCorpusImportStats,
    audit: PublicLexiconTokenCoverageAudit,
) {
    writeln!(
        output,
        "{label}：{} 句，{} 个句法 token · SHA-256 {corpus_sha256}",
        corpus_stats.sentences, corpus_stats.syntactic_tokens,
    )
    .unwrap();
    for length in audit.lengths {
        writeln!(
            output,
            "  {}字：词面 {}（实例 {}）；基线覆盖 {}（实例 {}），对照覆盖 {}（实例 {}）；新增 {}（实例 {}），丢失 {}（实例 {}）",
            length.characters,
            length.source_unique_tokens,
            length.source_token_instances,
            length.base_covered_unique_tokens,
            length.base_covered_token_instances,
            length.challenger_covered_unique_tokens,
            length.challenger_covered_token_instances,
            length.challenger_gained_unique_tokens,
            length.challenger_gained_token_instances,
            length.challenger_lost_unique_tokens,
            length.challenger_lost_token_instances,
        )
        .unwrap();
    }
}

fn audit_short_word_consensus(
    source: &Path,
    confirmation: &Path,
    base_payload: &Path,
    per_code_depth: usize,
    entry_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        source,
        "public toned Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let confirmation_text = read_explicit_text(
        confirmation,
        "independent public short-word confirmation",
        MAX_PUBLIC_SHORT_WORD_CONFIRMATION_BYTES,
    )?;
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let source_sha256 = candidate_sha256_hex(source_text.as_bytes());
    let confirmation_sha256 = candidate_sha256_hex(confirmation_text.as_bytes());
    let base_sha256 = candidate_sha256_hex(base_text.as_bytes());
    if source_sha256 == confirmation_sha256 || source_sha256 == base_sha256 {
        return Err("short-consensus-audit requires distinct public materials".into());
    }

    let base = parse_lexicon_tsv(&base_text)?;
    let imported = parse_public_short_word_consensus(
        &source_text,
        &confirmation_text,
        &base,
        per_code_depth,
        entry_limit,
    )?;
    let stats = imported.stats;
    let mut output = String::new();
    writeln!(output, "公开双来源短词共识审计")?;
    writeln!(output, "Rime 来源：SHA-256 {source_sha256}")?;
    writeln!(output, "独立确认：SHA-256 {confirmation_sha256}")?;
    writeln!(
        output,
        "基础载荷：{} 条 · SHA-256 {base_sha256}",
        base.len()
    )?;
    writeln!(
        output,
        "确认来源：{} 行；合格双字词面 {}；重复 {}",
        stats.confirmation_rows,
        stats.eligible_confirmation_surfaces,
        stats.duplicate_confirmation_surfaces,
    )?;
    writeln!(
        output,
        "Rime 交集：扫描 {} 行；命中 {}；有效 {}；唯一文字/码身份 {}",
        stats.source_rows,
        stats.confirmed_source_rows,
        stats.valid_confirmed_source_rows,
        stats.confirmed_identities,
    )?;
    writeln!(
        output,
        "排除基础：已有 {}；新增身份 {}；新增规范码 {}",
        stats.base_identities, stats.available_new_identities, stats.available_new_codes,
    )?;
    writeln!(
        output,
        "分层结果：每码最多 {}；内存选中 {}；上限外 {}",
        stats.per_code_depth, stats.imported_entries, stats.dropped_by_entry_cap,
    )?;
    writeln!(
        output,
        "口径：第二来源只确认词面存在；读音、规范码和层内顺序只来自 Rime；不比较跨来源频率。"
    )?;
    writeln!(output, "本次操作：只读；没有生成、安装或启用候选包")?;
    Ok(output)
}

struct ExactShortLayerAuditRequest<'a> {
    core_payload: &'a Path,
    supplemental_payload: &'a Path,
    exact_package: &'a Path,
    held_out_corpus: &'a Path,
    frontier_limit: usize,
    supplemental_promotions: usize,
}

struct ExactShortLayerBenchmarkRequest<'a> {
    core_payload: &'a Path,
    supplemental_payload: &'a Path,
    exact_package: &'a Path,
    frontier_limit: usize,
    supplemental_promotions: usize,
    exact_promotions: usize,
    candidate_limit: usize,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactShortTsfPreflightRequest<'a> {
    core_package: &'a Path,
    supplemental_package: Option<&'a Path>,
    exact_package: &'a Path,
    supplemental_promotions: Option<usize>,
    exact_promotions: usize,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactShortPrepareRequest<'a> {
    root: &'a Path,
    core_root: &'a Path,
    supplemental_root: Option<&'a Path>,
    package: &'a Path,
    expected_sha256: &'a str,
    exact_promotions: usize,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactShortTsfLayerIdentity {
    revision: String,
    authentication_sha256: String,
    load_duration: Duration,
}

struct ExactShortTsfPreflightSummary {
    core: ExactShortTsfLayerIdentity,
    supplemental: Option<ExactShortTsfLayerIdentity>,
    exact: ExactShortTsfLayerIdentity,
    exact_promotions: usize,
    requested_probes: usize,
    inspected_codes: usize,
    repetitions: usize,
    first_page: DurationSummary,
    second_page: DurationSummary,
    commit: DurationSummary,
    to_second_page: DurationSummary,
    complete_path: DurationSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactShortTargetDepthAudit {
    first: usize,
    first_instances: usize,
    second: usize,
    second_instances: usize,
    deeper: usize,
    deeper_instances: usize,
}

impl ExactShortTargetDepthAudit {
    fn observe(&mut self, rank: usize, instances: usize) {
        match rank {
            1 => {
                self.first += 1;
                self.first_instances += instances;
            }
            2 => {
                self.second += 1;
                self.second_instances += instances;
            }
            _ => {
                self.deeper += 1;
                self.deeper_instances += instances;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ExactShortInsertionPlacement {
    #[default]
    TopOne,
    ExactLane,
    FirstPage,
    GuardedFirstPage,
}

impl ExactShortInsertionPlacement {
    fn label(self) -> &'static str {
        match self {
            Self::TopOne => "首选后立即插入",
            Self::ExactLane => "现有完整词通道后插入",
            Self::FirstPage => "固定第一页、第二页开头插入",
            Self::GuardedFirstPage => "分页保护、第二页开头插入",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactShortPagingAudit {
    placement: ExactShortInsertionPlacement,
    stable_prefix: usize,
    promotions: usize,
    probes: usize,
    instances: usize,
    baseline_visible: usize,
    baseline_visible_instances: usize,
    preview_visible: usize,
    preview_visible_instances: usize,
    changed_pages: usize,
    changed_page_instances: usize,
    first_page_changes: usize,
    first_page_change_instances: usize,
    top_changes: usize,
    top_change_instances: usize,
    baseline_two_page_visible: usize,
    baseline_two_page_visible_instances: usize,
    preview_two_page_visible: usize,
    preview_two_page_visible_instances: usize,
    newly_two_page_visible: usize,
    newly_two_page_visible_instances: usize,
    lost_two_page_visible: usize,
    lost_two_page_visible_instances: usize,
    cross_page_degradations: usize,
    cross_page_degradation_instances: usize,
    useful_changes: usize,
    useful_change_instances: usize,
    neutral_changes: usize,
    neutral_change_instances: usize,
    harmful_changes: usize,
    harmful_change_instances: usize,
    inserted_slots: usize,
    displaced_slots: usize,
    movement: WeightedRankMovement,
}

impl ExactShortPagingAudit {
    fn with_profile(
        placement: ExactShortInsertionPlacement,
        promotions: usize,
        first_page_size: usize,
    ) -> Self {
        Self {
            placement,
            stable_prefix: match placement {
                ExactShortInsertionPlacement::TopOne => 1,
                ExactShortInsertionPlacement::ExactLane => 0,
                ExactShortInsertionPlacement::FirstPage
                | ExactShortInsertionPlacement::GuardedFirstPage => first_page_size,
            },
            promotions,
            ..Self::default()
        }
    }

    fn observe(
        &mut self,
        before: &[String],
        after: &[String],
        expected: &str,
        instances: usize,
        first_page_size: usize,
    ) {
        let before_rank = candidate_rank(before, expected);
        let after_rank = candidate_rank(after, expected);
        let changed = before != after;
        let first_page_changed = before
            .iter()
            .take(first_page_size)
            .ne(after.iter().take(first_page_size));
        let top_changed = before.first() != after.first();
        let two_page_limit = first_page_size
            .saturating_mul(2)
            .min(MAX_CANDIDATE_SNAPSHOT_RANK);
        let before_two_page = before_rank.is_some_and(|rank| rank <= two_page_limit);
        let after_two_page = after_rank.is_some_and(|rank| rank <= two_page_limit);
        let cross_page_degradation = match (before_rank, after_rank) {
            (Some(before), Some(after)) => {
                (after - 1) / first_page_size > (before - 1) / first_page_size
            }
            _ => false,
        };
        let useful = match (before_rank, after_rank) {
            (None, Some(_)) => true,
            (Some(before), Some(after)) => after < before,
            _ => false,
        };
        let harmful = match (before_rank, after_rank) {
            (Some(_), None) => true,
            (Some(before), Some(after)) => after > before,
            _ => false,
        };

        self.probes += 1;
        self.instances += instances;
        self.baseline_visible += usize::from(before_rank.is_some());
        self.baseline_visible_instances += instances * usize::from(before_rank.is_some());
        self.preview_visible += usize::from(after_rank.is_some());
        self.preview_visible_instances += instances * usize::from(after_rank.is_some());
        self.changed_pages += usize::from(changed);
        self.changed_page_instances += instances * usize::from(changed);
        self.first_page_changes += usize::from(first_page_changed);
        self.first_page_change_instances += instances * usize::from(first_page_changed);
        self.top_changes += usize::from(top_changed);
        self.top_change_instances += instances * usize::from(top_changed);
        self.baseline_two_page_visible += usize::from(before_two_page);
        self.baseline_two_page_visible_instances += instances * usize::from(before_two_page);
        self.preview_two_page_visible += usize::from(after_two_page);
        self.preview_two_page_visible_instances += instances * usize::from(after_two_page);
        self.newly_two_page_visible += usize::from(!before_two_page && after_two_page);
        self.newly_two_page_visible_instances +=
            instances * usize::from(!before_two_page && after_two_page);
        self.lost_two_page_visible += usize::from(before_two_page && !after_two_page);
        self.lost_two_page_visible_instances +=
            instances * usize::from(before_two_page && !after_two_page);
        self.cross_page_degradations += usize::from(cross_page_degradation);
        self.cross_page_degradation_instances += instances * usize::from(cross_page_degradation);
        self.useful_changes += usize::from(useful);
        self.useful_change_instances += instances * usize::from(useful);
        self.harmful_changes += usize::from(harmful);
        self.harmful_change_instances += instances * usize::from(harmful);
        self.neutral_changes += usize::from(changed && !useful && !harmful);
        self.neutral_change_instances += instances * usize::from(changed && !useful && !harmful);
        self.inserted_slots += after
            .iter()
            .filter(|candidate| !before.contains(candidate))
            .count();
        self.displaced_slots += before
            .iter()
            .filter(|candidate| !after.contains(candidate))
            .count();
        self.movement.observe(before_rank, after_rank, instances);
    }

    fn safety_gate_passed(self) -> bool {
        match self.placement {
            ExactShortInsertionPlacement::TopOne | ExactShortInsertionPlacement::ExactLane => {
                self.useful_changes != 0
                    && self.top_changes == 0
                    && self.harmful_changes == 0
                    && self.movement.lost_visible == 0
            }
            ExactShortInsertionPlacement::FirstPage
            | ExactShortInsertionPlacement::GuardedFirstPage => {
                self.useful_changes != 0
                    && self.first_page_changes == 0
                    && self.cross_page_degradations == 0
                    && self.movement.lost_visible == 0
            }
        }
    }
}

fn audit_exact_short_layer(
    request: ExactShortLayerAuditRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        request.core_payload,
        "core public exact-short audit payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        request.supplemental_payload,
        "supplemental public exact-short audit payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        request.held_out_corpus,
        "public exact-short held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let core_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let supplemental_sha256 = candidate_sha256_hex(supplemental_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if core_sha256 == supplemental_sha256 {
        return Err("exact-short-layer-audit requires distinct core and supplemental data".into());
    }
    if held_out_sha256 == core_sha256 || held_out_sha256 == supplemental_sha256 {
        return Err("exact-short-layer-audit requires an independent held-out corpus".into());
    }

    let core = snapshot_from_payload("exact-short-audit-core-v1", &core_text)?;
    let supplemental =
        snapshot_from_payload("exact-short-audit-supplemental-v1", &supplemental_text)?;
    let (exact, exact_entries) =
        load_exact_short_package_directory_with_entries(request.exact_package)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let selection = select_public_lexicon_rank_probes(&held_out, &exact_entries, 2);
    let ambiguous_surfaces = ambiguous_lexicon_surfaces(&exact_entries);
    let matched_unique_tokens = selection.probes.len();
    let mut ambiguous_unique_tokens = 0_usize;
    let mut ambiguous_token_instances = 0_usize;
    let probes = selection
        .probes
        .into_iter()
        .filter(|probe| {
            let ambiguous = ambiguous_surfaces.contains(probe.expected_text.as_str());
            ambiguous_unique_tokens += usize::from(ambiguous);
            ambiguous_token_instances += probe.instances * usize::from(ambiguous);
            !ambiguous
        })
        .collect::<Vec<_>>();

    let supplemental_config = SupplementalCandidateLayerConfig {
        exact_promotions: request.supplemental_promotions,
    };
    let mut target_depth = ExactShortTargetDepthAudit::default();
    let mut profiles = [
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::TopOne,
            1,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::TopOne,
            2,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::ExactLane,
            1,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::ExactLane,
            2,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::FirstPage,
            1,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::FirstPage,
            2,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::GuardedFirstPage,
            1,
            request.frontier_limit,
        ),
        ExactShortPagingAudit::with_profile(
            ExactShortInsertionPlacement::GuardedFirstPage,
            2,
            request.frontier_limit,
        ),
    ];
    for probe in &probes {
        let code = probe.observed.as_str();
        let exact_candidates = exact.catalog.candidate_texts(code, 8)?;
        let exact_rank = exact_candidates
            .iter()
            .position(|candidate| *candidate == probe.expected_text)
            .map(|index| index + 1)
            .ok_or("selected exact-short target is absent from its authenticated code")?;
        target_depth.observe(exact_rank, probe.instances);
        let baseline = layered_candidate_texts(
            &core,
            &supplemental,
            code,
            MAX_CANDIDATE_SNAPSHOT_RANK,
            supplemental_config,
        )?;
        let core_exact = core.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
        let supplemental_exact =
            supplemental.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
        let existing_exact = core_exact
            .iter()
            .chain(&supplemental_exact)
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let exact_lane_prefix = baseline
            .iter()
            .take_while(|candidate| existing_exact.contains(candidate.as_str()))
            .count()
            .max(1);
        for profile in &mut profiles {
            let preview = match profile.placement {
                ExactShortInsertionPlacement::GuardedFirstPage => {
                    exact.catalog.preview_candidate_texts_after_page_guarded(
                        &baseline,
                        code,
                        MAX_CANDIDATE_SNAPSHOT_RANK,
                        profile.promotions,
                        request.frontier_limit,
                    )?
                }
                ExactShortInsertionPlacement::ExactLane => {
                    exact.catalog.preview_candidate_texts_after_prefix(
                        &baseline,
                        code,
                        MAX_CANDIDATE_SNAPSHOT_RANK,
                        profile.promotions,
                        exact_lane_prefix,
                    )?
                }
                ExactShortInsertionPlacement::TopOne | ExactShortInsertionPlacement::FirstPage => {
                    exact.catalog.preview_candidate_texts_after_prefix(
                        &baseline,
                        code,
                        MAX_CANDIDATE_SNAPSHOT_RANK,
                        profile.promotions,
                        profile.stable_prefix,
                    )?
                }
            };
            profile.observe(
                &baseline,
                &preview,
                &probe.expected_text,
                probe.instances,
                request.frontier_limit,
            );
        }
    }

    let mut output = String::new();
    writeln!(output, "公开精确短词分页位移审计")?;
    writeln!(
        output,
        "核心：{} 条 · SHA-256 {core_sha256}",
        core.entry_count()
    )?;
    writeln!(
        output,
        "补充：{} 条 · SHA-256 {supplemental_sha256}；每码补 {} 项",
        supplemental.entry_count(),
        request.supplemental_promotions,
    )?;
    writeln!(
        output,
        "精确层：{} 条，{} 个码 · 认证 SHA-256 {}",
        exact.catalog.entry_count(),
        exact.catalog.code_count(),
        exact.authentication_sha256,
    )?;
    writeln!(
        output,
        "独立留出：{} 句，{} 个句法 token · SHA-256 {held_out_sha256}",
        held_out.stats.sentences, held_out.stats.syntactic_tokens,
    )?;
    writeln!(
        output,
        "选样：双字词面 {}（实例 {}）；精确层匹配 {}（实例 {}）；多音排除 {}（实例 {}）；评测 {}（实例 {}）",
        selection.source_unique_tokens,
        selection.source_token_instances,
        matched_unique_tokens,
        selection.matched_token_instances,
        ambiguous_unique_tokens,
        ambiguous_token_instances,
        probes.len(),
        probes.iter().map(|probe| probe.instances).sum::<usize>(),
    )?;
    writeln!(
        output,
        "目标在精确层内：第 1 项 {}（实例 {}），第 2 项 {}（实例 {}），更深 {}（实例 {}）",
        target_depth.first,
        target_depth.first_instances,
        target_depth.second,
        target_depth.second_instances,
        target_depth.deeper,
        target_depth.deeper_instances,
    )?;
    writeln!(
        output,
        "候选范围：固定总候选前 {MAX_CANDIDATE_SNAPSHOT_RANK}；第一页前 {}；前两页前 {}",
        request.frontier_limit,
        request
            .frontier_limit
            .saturating_mul(2)
            .min(MAX_CANDIDATE_SNAPSHOT_RANK),
    )?;
    for profile in profiles {
        writeln!(
            output,
            "  {} · 精确补 {}：总范围基线可见 {}（实例 {}），预览可见 {}（实例 {}）；前两页基线可见 {}（实例 {}），预览可见 {}（实例 {}），新进入 {}（实例 {}），掉出 {}（实例 {}）；候选变化 {}（实例 {}），有益 {}（实例 {}），中性 {}（实例 {}），名次后移 {}（实例 {}）；第一页变化 {}（实例 {}），首选变化 {}，跨页退化 {}（实例 {}）；新插槽 {}，总范围尾部挤出 {}；新进入总范围 {}，掉出总范围 {}；安全门 {}",
            profile.placement.label(),
            profile.promotions,
            profile.baseline_visible,
            profile.baseline_visible_instances,
            profile.preview_visible,
            profile.preview_visible_instances,
            profile.baseline_two_page_visible,
            profile.baseline_two_page_visible_instances,
            profile.preview_two_page_visible,
            profile.preview_two_page_visible_instances,
            profile.newly_two_page_visible,
            profile.newly_two_page_visible_instances,
            profile.lost_two_page_visible,
            profile.lost_two_page_visible_instances,
            profile.changed_pages,
            profile.changed_page_instances,
            profile.useful_changes,
            profile.useful_change_instances,
            profile.neutral_changes,
            profile.neutral_change_instances,
            profile.harmful_changes,
            profile.harmful_change_instances,
            profile.first_page_changes,
            profile.first_page_change_instances,
            profile.top_changes,
            profile.cross_page_degradations,
            profile.cross_page_degradation_instances,
            profile.inserted_slots,
            profile.displaced_slots,
            profile.movement.newly_visible,
            profile.movement.lost_visible,
            if profile.safety_gate_passed() {
                "通过"
            } else {
                "未通过"
            },
        )?;
    }
    writeln!(
        output,
        "安全门口径：首选后或现有完整词通道后插入均要求至少一个目标受益、首选零变化、公开目标零名次后移且零掉出；普通与受保护的第二页通道均要求至少一个目标受益、第一页逐项零变化、目标零跨页退化且零掉出总范围。总范围尾部位移单独报告，不被伪装成正确性结论。"
    )?;
    writeln!(
        output,
        "本次操作：只读预览；不写候选包、不改槽位、不接入 TSF"
    )?;
    Ok(output)
}

fn benchmark_exact_short_layer(
    request: ExactShortLayerBenchmarkRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-short-layer-benchmark must run from a release build".into());
    }
    let core_text = read_explicit_text(
        request.core_payload,
        "core public exact-short benchmark payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        request.supplemental_payload,
        "supplemental public exact-short benchmark payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    if candidate_sha256_hex(core_text.as_bytes())
        == candidate_sha256_hex(supplemental_text.as_bytes())
    {
        return Err(
            "exact-short-layer-benchmark requires distinct core and supplemental data".into(),
        );
    }

    let core_started = Instant::now();
    let core = snapshot_from_payload("exact-short-benchmark-core-v1", &core_text)?;
    let core_build = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental =
        snapshot_from_payload("exact-short-benchmark-supplemental-v1", &supplemental_text)?;
    let supplemental_build = supplemental_started.elapsed();
    let exact_started = Instant::now();
    let exact = load_exact_short_package_directory(request.exact_package)?;
    let exact_load = exact_started.elapsed();

    let exact_payload = read_explicit_text(
        &request.exact_package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        "exact-short benchmark sampling payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let exact_entries = parse_lexicon_tsv(&exact_payload)?;
    if exact_entries.len() != exact.catalog.entry_count() {
        return Err("exact-short benchmark sampling payload changed after authentication".into());
    }
    let codes = evenly_spaced_exact_short_codes(&exact_entries, request.sample_limit);
    if codes.len() < 8 {
        return Err("exact-short benchmark package exposes too few distinct codes".into());
    }
    let supplemental_config = SupplementalCandidateLayerConfig {
        exact_promotions: request.supplemental_promotions,
    };
    let baselines = codes
        .iter()
        .map(|code| {
            layered_candidate_texts(
                &core,
                &supplemental,
                code,
                request.candidate_limit,
                supplemental_config,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut full_first_pages = 0_usize;
    let mut raw_changed_codes = 0_usize;
    let mut guarded_changed_codes = 0_usize;
    let mut guarded_reductions = 0_usize;
    let mut guarded_inserted_slots = 0_usize;
    for (code, baseline) in codes.iter().zip(&baselines) {
        full_first_pages += usize::from(baseline.len() >= request.frontier_limit);
        let raw = exact.catalog.preview_candidate_texts_after_prefix(
            baseline,
            code,
            request.candidate_limit,
            request.exact_promotions,
            request.frontier_limit,
        )?;
        let guarded = exact.catalog.preview_candidate_texts_after_page_guarded(
            baseline,
            code,
            request.candidate_limit,
            request.exact_promotions,
            request.frontier_limit,
        )?;
        if baseline
            .iter()
            .take(request.frontier_limit)
            .ne(guarded.iter().take(request.frontier_limit))
        {
            return Err("exact-short guarded benchmark changed the first page".into());
        }
        if !existing_exact_short_pages_are_stable(
            &exact.catalog,
            code,
            baseline,
            &guarded,
            request.frontier_limit,
        )? {
            return Err(
                "exact-short guarded benchmark moved an existing exact word across pages".into(),
            );
        }
        raw_changed_codes += usize::from(raw != *baseline);
        guarded_changed_codes += usize::from(guarded != *baseline);
        guarded_reductions += usize::from(raw != guarded);
        guarded_inserted_slots += guarded
            .iter()
            .filter(|candidate| !baseline.contains(candidate))
            .count();
    }

    for (code, baseline) in codes.iter().zip(&baselines) {
        black_box(exact.catalog.preview_candidate_texts_after_page_guarded(
            black_box(baseline),
            black_box(code),
            request.candidate_limit,
            request.exact_promotions,
            request.frontier_limit,
        )?);
    }

    let sample_count = request.repetitions.saturating_mul(codes.len());
    let mut baseline_durations = Vec::with_capacity(sample_count);
    let mut merge_durations = Vec::with_capacity(sample_count);
    let mut combined_durations = Vec::with_capacity(sample_count);
    let mut checksum = 0_usize;
    for _ in 0..request.repetitions {
        for (code, precomputed) in codes.iter().zip(&baselines) {
            let started = Instant::now();
            let baseline = layered_candidate_texts(
                &core,
                &supplemental,
                black_box(code),
                request.candidate_limit,
                supplemental_config,
            )?;
            baseline_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &baseline);
            black_box(baseline);

            let started = Instant::now();
            let merged = exact.catalog.preview_candidate_texts_after_page_guarded(
                black_box(precomputed),
                black_box(code),
                request.candidate_limit,
                request.exact_promotions,
                request.frontier_limit,
            )?;
            merge_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &merged);
            black_box(merged);

            let started = Instant::now();
            let baseline = layered_candidate_texts(
                &core,
                &supplemental,
                black_box(code),
                request.candidate_limit,
                supplemental_config,
            )?;
            let merged = exact.catalog.preview_candidate_texts_after_page_guarded(
                &baseline,
                black_box(code),
                request.candidate_limit,
                request.exact_promotions,
                request.frontier_limit,
            )?;
            combined_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &merged);
            black_box(merged);
        }
    }
    let baseline_latency = summarize_durations(&mut baseline_durations)
        .ok_or("exact-short benchmark produced no baseline samples")?;
    let merge_latency = summarize_durations(&mut merge_durations)
        .ok_or("exact-short benchmark produced no merge samples")?;
    let combined_latency = summarize_durations(&mut combined_durations)
        .ok_or("exact-short benchmark produced no combined samples")?;
    let median_delta_ms =
        duration_ms(combined_latency.median) - duration_ms(baseline_latency.median);
    let local_gate_passed = merge_latency.p95 <= Duration::from_micros(100);

    Ok(format!(
        "公开精确短词分页保护 release 热路径\n精确层：{} 条，{} 个码；认证 SHA-256 {}\n工作负载：全目录等距取码 {}；请求前 {} 项；完整第一页 {}；重复 {repetitions}；计时样本 {}\n冷准备：核心索引 {:.3} ms；补充索引 {:.3} ms；精确包认证与索引 {:.3} ms\n核心＋补充基线：median {:.3} ms；p95 {:.3} ms；p99 {:.3} ms；max {:.3} ms\n仅精确查询与分页保护合并：median {:.3} ms；p95 {:.3} ms；p99 {:.3} ms；max {:.3} ms\n完整三层路径：median {:.3} ms；p95 {:.3} ms；p99 {:.3} ms；max {:.3} ms\n完整路径 median 相对基线：{median_delta_ms:+.3} ms（两个深解码分布之差，仅报告，不作增量门）\n结构：无保护会变化 {} 个码；保护后变化 {} 个码；保护缩减 {} 个码；实际新增槽 {}；第一页变化 0；已有精确词跨页 0\n本机增量门：独立 merge p95 ≤ 0.100 ms：{}\n结果校验和：{checksum}\n口径：release、同机、预热；取样只来自已认证公开精确层并覆盖整个码序。请求范围只验证当前已加载页，不能替代 Top-50 离线全局分页审计；冷准备不是 TSF 绘制首帧，单机阈值也不是跨设备承诺。\n本次操作：只读；不写槽位、不接入 TSF\n",
        exact.catalog.entry_count(),
        exact.catalog.code_count(),
        exact.authentication_sha256,
        codes.len(),
        request.candidate_limit,
        full_first_pages,
        baseline_latency.samples,
        duration_ms(core_build),
        duration_ms(supplemental_build),
        duration_ms(exact_load),
        duration_ms(baseline_latency.median),
        duration_ms(baseline_latency.p95),
        duration_ms(baseline_latency.p99),
        duration_ms(baseline_latency.maximum),
        duration_ms(merge_latency.median),
        duration_ms(merge_latency.p95),
        duration_ms(merge_latency.p99),
        duration_ms(merge_latency.maximum),
        duration_ms(combined_latency.median),
        duration_ms(combined_latency.p95),
        duration_ms(combined_latency.p99),
        duration_ms(combined_latency.maximum),
        raw_changed_codes,
        guarded_changed_codes,
        guarded_reductions,
        guarded_inserted_slots,
        if local_gate_passed {
            "通过"
        } else {
            "未通过"
        },
        repetitions = request.repetitions,
    ))
}

#[cfg(windows)]
fn preflight_exact_short_tsf(
    request: ExactShortTsfPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    run_exact_short_tsf_preflight(request)
        .map(|summary| render_exact_short_tsf_preflight_report(&summary))
}

#[cfg(windows)]
fn run_exact_short_tsf_preflight(
    request: ExactShortTsfPreflightRequest<'_>,
) -> Result<ExactShortTsfPreflightSummary, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-short-tsf-preflight must run from a release build".into());
    }

    let core_started = Instant::now();
    let core = load_public_package_directory(request.core_package)?;
    let core_load_duration = core_started.elapsed();

    let supplemental = match request.supplemental_package {
        Some(path) => {
            let started = Instant::now();
            let loaded = load_public_package_directory(path)?;
            Some((loaded, started.elapsed()))
        }
        None => None,
    };

    let exact_started = Instant::now();
    let (exact, exact_entries) =
        load_exact_short_package_directory_with_entries(request.exact_package)?;
    let exact_load_duration = exact_started.elapsed();
    if exact_entries.len() != exact.catalog.entry_count() {
        return Err("exact-short TSF sampling payload changed after authentication".into());
    }
    if supplemental
        .as_ref()
        .is_some_and(|(loaded, _)| loaded.authentication_sha256 == core.authentication_sha256)
    {
        return Err(
            "exact-short TSF preflight requires distinct core and supplemental data".into(),
        );
    }

    let all_codes = evenly_spaced_exact_short_codes(&exact_entries, exact_entries.len());
    if request.sample_limit > all_codes.len() {
        return Err("exact-short TSF preflight package exposes too few distinct codes".into());
    }

    struct Probe {
        code: String,
        expected_text: String,
    }

    let exact_revision = exact.catalog.revision().to_owned();
    let exact_catalog = Arc::new(exact.catalog);
    let expected_supplemental_revision = supplemental
        .as_ref()
        .map(|(loaded, _)| loaded.snapshot.revision());
    let supplemental_layer = || {
        supplemental.as_ref().map(|(loaded, _)| {
            (
                Arc::clone(&loaded.snapshot),
                SupplementalCandidateLayerConfig {
                    exact_promotions: request
                        .supplemental_promotions
                        .expect("the parser binds a supplemental package and cap together"),
                },
            )
        })
    };
    let mut probes = Vec::<Probe>::with_capacity(request.sample_limit);
    let mut inspected = BTreeSet::<usize>::new();
    let per_anchor_scan = all_codes.len().min(64);
    for sample_index in 0..request.sample_limit {
        let anchor = sample_index * all_codes.len() / request.sample_limit;
        let mut selected = None;
        for offset in 0..per_anchor_scan {
            let code_index = (anchor + offset) % all_codes.len();
            if !inspected.insert(code_index) {
                continue;
            }
            let code = &all_codes[code_index];
            let targets = exact_catalog
                .candidate_texts(code, MAX_EXACT_SHORT_WORDS_PER_CODE)?
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for expected_text in targets {
                match preflight_exact_short_candidate_layers(
                    Arc::clone(&core.snapshot),
                    supplemental_layer(),
                    Arc::clone(&exact_catalog),
                    request.exact_promotions,
                    code,
                    &expected_text,
                ) {
                    Ok(report) => {
                        if report.core_revision() != core.snapshot.revision()
                            || report.supplemental_revision() != expected_supplemental_revision
                            || report.exact_short_revision() != exact_revision
                        {
                            return Err("exact-short TSF preflight runtime identity changed".into());
                        }
                        selected = Some(Probe {
                            code: code.clone(),
                            expected_text,
                        });
                        break;
                    }
                    Err(TsfCandidatePreflightError::ExactShortPageMismatch) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if selected.is_some() {
                break;
            }
        }
        probes.push(selected.ok_or(
            "exact-short TSF preflight found too few page-stable probes within the bounded scan",
        )?);
    }

    let sample_count = probes.len().saturating_mul(request.repetitions);
    let mut first_page_samples = Vec::with_capacity(sample_count);
    let mut second_page_samples = Vec::with_capacity(sample_count);
    let mut commit_samples = Vec::with_capacity(sample_count);
    let mut to_second_page_samples = Vec::with_capacity(sample_count);
    let mut complete_path_samples = Vec::with_capacity(sample_count);
    for _ in 0..request.repetitions {
        for probe in &probes {
            let report = preflight_exact_short_candidate_layers(
                Arc::clone(&core.snapshot),
                supplemental_layer(),
                Arc::clone(&exact_catalog),
                request.exact_promotions,
                &probe.code,
                &probe.expected_text,
            )?;
            if report.core_revision() != core.snapshot.revision()
                || report.supplemental_revision() != expected_supplemental_revision
                || report.exact_short_revision() != exact_revision
            {
                return Err("exact-short TSF preflight runtime identity changed".into());
            }
            first_page_samples.push(report.first_page_duration());
            second_page_samples.push(report.second_page_duration());
            commit_samples.push(report.commit_duration());
            to_second_page_samples
                .push(report.first_page_duration() + report.second_page_duration());
            complete_path_samples.push(
                report.first_page_duration()
                    + report.second_page_duration()
                    + report.commit_duration(),
            );
        }
    }

    let summary = ExactShortTsfPreflightSummary {
        core: ExactShortTsfLayerIdentity {
            revision: core.snapshot.revision().to_owned(),
            authentication_sha256: core.authentication_sha256,
            load_duration: core_load_duration,
        },
        supplemental: supplemental.map(|(loaded, load_duration)| ExactShortTsfLayerIdentity {
            revision: loaded.snapshot.revision().to_owned(),
            authentication_sha256: loaded.authentication_sha256,
            load_duration,
        }),
        exact: ExactShortTsfLayerIdentity {
            revision: exact_revision,
            authentication_sha256: exact.authentication_sha256,
            load_duration: exact_load_duration,
        },
        exact_promotions: request.exact_promotions,
        requested_probes: probes.len(),
        inspected_codes: inspected.len(),
        repetitions: request.repetitions,
        first_page: summarize_durations(&mut first_page_samples)
            .ok_or("exact-short TSF preflight produced no first-page samples")?,
        second_page: summarize_durations(&mut second_page_samples)
            .ok_or("exact-short TSF preflight produced no second-page samples")?,
        commit: summarize_durations(&mut commit_samples)
            .ok_or("exact-short TSF preflight produced no commit samples")?,
        to_second_page: summarize_durations(&mut to_second_page_samples)
            .ok_or("exact-short TSF preflight produced no combined page samples")?,
        complete_path: summarize_durations(&mut complete_path_samples)
            .ok_or("exact-short TSF preflight produced no complete-path samples")?,
    };
    Ok(summary)
}

#[cfg(not(windows))]
fn preflight_exact_short_tsf(
    _request: ExactShortTsfPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    Err("exact-short TSF preflight requires Windows".into())
}

#[cfg(not(windows))]
fn run_exact_short_tsf_preflight(
    _request: ExactShortTsfPreflightRequest<'_>,
) -> Result<ExactShortTsfPreflightSummary, Box<dyn std::error::Error>> {
    Err("exact-short TSF preflight requires Windows".into())
}

fn render_exact_short_tsf_preflight_report(summary: &ExactShortTsfPreflightSummary) -> String {
    let mut output = String::new();
    writeln!(output, "TSF 精确短词第二页 release 预检").unwrap();
    writeln!(
        output,
        "核心：{} · 认证 SHA-256 {} · 载入 {:.3} ms",
        summary.core.revision,
        summary.core.authentication_sha256,
        duration_ms(summary.core.load_duration),
    )
    .unwrap();
    match summary.supplemental.as_ref() {
        Some(supplemental) => writeln!(
            output,
            "补充：{} · 认证 SHA-256 {} · 载入 {:.3} ms",
            supplemental.revision,
            supplemental.authentication_sha256,
            duration_ms(supplemental.load_duration),
        )
        .unwrap(),
        None => writeln!(output, "补充：未使用").unwrap(),
    }
    writeln!(
        output,
        "精确层：{} · 认证 SHA-256 {} · 载入 {:.3} ms · 每码最多补 {}",
        summary.exact.revision,
        summary.exact.authentication_sha256,
        duration_ms(summary.exact.load_duration),
        summary.exact_promotions,
    )
    .unwrap();
    writeln!(
        output,
        "工作负载：等距锚点 {} 个；有界检查 {} 个码；每码预热 1 次；重复 {}；计时样本 {}；页宽 {}",
        summary.requested_probes,
        summary.inspected_codes,
        summary.repetitions,
        summary.first_page.samples,
        TSF_ALPHA_CANDIDATE_PAGE_SIZE,
    )
    .unwrap();
    write_tsf_preflight_duration(&mut output, "输入至第一页状态就绪", summary.first_page);
    write_tsf_preflight_duration(
        &mut output,
        "PageDown 至第二页状态就绪",
        summary.second_page,
    );
    write_tsf_preflight_duration(&mut output, "空格提交第二页首项", summary.commit);
    write_tsf_preflight_duration(&mut output, "首键至第二页状态就绪", summary.to_second_page);
    write_tsf_preflight_duration(&mut output, "首键至提交完成", summary.complete_path);
    writeln!(
        output,
        "功能门：全部样本均保持第一页并提交带 PublicConsensusExact 来源的第二页首项"
    )
    .unwrap();
    writeln!(
        output,
        "性能口径：同机、release、每次新建真实系统 TSF 合成 Context；计时截止同步候选状态，不包含候选窗绘制、桌面合成或实际宿主呈现。只记录分布，尚未授权启用阈值。"
    )
    .unwrap();
    writeln!(
        output,
        "隐私：报告不含探针码、候选正文或个人数据；本次操作只读，不写槽位、不安装、不启用。"
    )
    .unwrap();
    output
}

fn write_tsf_preflight_duration(output: &mut String, label: &str, summary: DurationSummary) {
    writeln!(
        output,
        "{label}：median {:.3} ms；p95 {:.3} ms；p99 {:.3} ms；max {:.3} ms",
        duration_ms(summary.median),
        duration_ms(summary.p95),
        duration_ms(summary.p99),
        duration_ms(summary.maximum),
    )
    .unwrap();
}

#[cfg(windows)]
fn preflight_popup_rendering(repetitions: usize) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("popup render preflight must run from a release build".into());
    }
    let report = preflight_candidate_popup_rendering(repetitions)?;
    render_popup_render_preflight_report(&report)
}

#[cfg(not(windows))]
fn preflight_popup_rendering(_repetitions: usize) -> Result<String, Box<dyn std::error::Error>> {
    Err("popup render preflight requires Windows".into())
}

#[cfg(windows)]
fn render_popup_render_preflight_report(
    report: &CandidatePopupRenderPreflightReport,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut grouped = BTreeMap::<(CandidatePopupRenderScenario, u32), Vec<_>>::new();
    for sample in report.samples() {
        if sample.effective_dpi() != sample.requested_dpi() {
            return Err("popup render preflight observed an unexpected DPI".into());
        }
        grouped
            .entry((sample.scenario(), sample.requested_dpi()))
            .or_default()
            .push(sample);
    }
    let expected_groups = CandidatePopupRenderScenario::ALL.len().saturating_mul(4);
    let expected_samples = report.repetitions().saturating_mul(expected_groups);
    if grouped.len() != expected_groups
        || report.samples().len() != expected_samples
        || grouped
            .values()
            .any(|samples| samples.len() != report.repetitions())
        || report.hide_durations().len() != report.repetitions().saturating_mul(4)
        || report.destroy_durations().len() != report.repetitions().saturating_mul(4)
    {
        return Err("popup render preflight returned an incomplete fixed workload".into());
    }

    let mut output = String::new();
    writeln!(output, "候选窗生产 GDI release 预检")?;
    writeln!(
        output,
        "工作负载：4 条固定视觉路径 × 4 档 DPI × 重复 {}；绘制样本 {}；候选正文不进入报告",
        report.repetitions(),
        report.samples().len(),
    )?;
    for ((scenario, dpi), samples) in grouped {
        let window_ready =
            summarize_popup_render_stage(&samples, |sample| sample.window_ready_duration())?;
        let paint_entered =
            summarize_popup_render_stage(&samples, |sample| sample.paint_entered_duration())?;
        let drawing =
            summarize_popup_render_stage(&samples, |sample| sample.drawing_completed_duration())?;
        let published =
            summarize_popup_render_stage(&samples, |sample| sample.frame_published_duration())?;
        let completed =
            summarize_popup_render_stage(&samples, |sample| sample.paint_completed_duration())?;
        let buffered = samples.iter().filter(|sample| sample.buffered()).count();
        writeln!(
            output,
            "{} · {} DPI · {} 样本 · 双缓冲 {}/{}",
            popup_render_scenario_label(scenario),
            dpi,
            samples.len(),
            buffered,
            samples.len(),
        )?;
        write_popup_render_stage(&mut output, "  窗口就绪", window_ready)?;
        write_popup_render_stage(&mut output, "  进入 WM_PAINT", paint_entered)?;
        write_popup_render_stage(&mut output, "  GDI 绘制完成", drawing)?;
        write_popup_render_stage(&mut output, "  完整帧发布", published)?;
        write_popup_render_stage(&mut output, "  EndPaint 完成", completed)?;

        let mut flush_calls = samples
            .iter()
            .filter_map(|sample| sample.compositor_flush_duration())
            .collect::<Vec<_>>();
        let mut request_to_flush = samples
            .iter()
            .filter_map(|sample| sample.request_to_compositor_flush_duration())
            .collect::<Vec<_>>();
        match (
            summarize_durations(&mut flush_calls),
            summarize_durations(&mut request_to_flush),
        ) {
            (Some(flush), Some(total)) => writeln!(
                output,
                "  DwmFlush：{}/{} 成功；调用 p95 {:.3} ms；请求至返回 p95 {:.3} ms",
                flush.samples,
                samples.len(),
                duration_ms(flush.p95),
                duration_ms(total.p95),
            )?,
            _ => writeln!(output, "  DwmFlush：当前桌面合成环境不可观测")?,
        }
    }

    let mut hide = report.hide_durations().to_vec();
    let mut destroy = report.destroy_durations().to_vec();
    write_popup_render_stage(
        &mut output,
        "隐藏并确认不可见",
        summarize_durations(&mut hide).ok_or("popup render preflight has no hide samples")?,
    )?;
    write_popup_render_stage(
        &mut output,
        "销毁并确认 HWND 失效",
        summarize_durations(&mut destroy).ok_or("popup render preflight has no destroy samples")?,
    )?;
    writeln!(
        output,
        "功能门：每次绘制均依次到达窗口就绪、WM_PAINT、GDI 绘制、完整帧发布和 EndPaint；隐藏与销毁均已由系统窗口状态确认。"
    )?;
    writeln!(
        output,
        "边界：DwmFlush 只表示桌面合成队列的同步边界，不证明屏幕已扫描显示；本预检也不覆盖真实编辑器宿主的调度、显示器刷新或肉眼感知。"
    )?;
    writeln!(
        output,
        "本次操作：release-only、显式运行、进程内临时窗口；不注册 TSF、不安装、不换代、不读写候选槽、反馈记录或个人数据。"
    )?;
    Ok(output)
}

#[cfg(windows)]
fn summarize_popup_render_stage(
    samples: &[&ziranma_core::CandidatePopupRenderSample],
    stage: impl Fn(&ziranma_core::CandidatePopupRenderSample) -> Duration,
) -> Result<DurationSummary, Box<dyn std::error::Error>> {
    let mut durations = samples
        .iter()
        .map(|sample| stage(sample))
        .collect::<Vec<_>>();
    summarize_durations(&mut durations).ok_or_else(|| "popup render stage has no samples".into())
}

#[cfg(windows)]
fn popup_render_scenario_label(scenario: CandidatePopupRenderScenario) -> &'static str {
    match scenario {
        CandidatePopupRenderScenario::InitialShow => "首次创建",
        CandidatePopupRenderScenario::ContentUpdate => "原位更新",
        CandidatePopupRenderScenario::PageRedraw => "第二页重绘",
        CandidatePopupRenderScenario::LongCandidateRedraw => "长候选竖排",
    }
}

#[cfg(windows)]
fn write_popup_render_stage(
    output: &mut String,
    label: &str,
    summary: DurationSummary,
) -> Result<(), std::fmt::Error> {
    writeln!(
        output,
        "{label}：median {:.3} ms；p95 {:.3} ms；max {:.3} ms",
        duration_ms(summary.median),
        duration_ms(summary.p95),
        duration_ms(summary.maximum),
    )
}

fn evenly_spaced_exact_short_codes(entries: &[LexiconEntry], limit: usize) -> Vec<String> {
    let unique = entries
        .iter()
        .map(|entry| entry.code.as_str().to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if unique.len() <= limit {
        return unique;
    }
    (0..limit)
        .map(|sample| unique[sample * unique.len() / limit].clone())
        .collect()
}

fn existing_exact_short_pages_are_stable(
    catalog: &ExactShortWordCatalog,
    code: &str,
    before: &[String],
    after: &[String],
    page_size: usize,
) -> Result<bool, Box<dyn std::error::Error>> {
    for candidate in catalog.candidate_texts(code, MAX_EXACT_SHORT_WORDS_PER_CODE)? {
        let Some(before_rank) = candidate_rank(before, candidate) else {
            continue;
        };
        let Some(after_rank) = candidate_rank(after, candidate) else {
            return Ok(false);
        };
        if (before_rank - 1) / page_size != (after_rank - 1) / page_size {
            return Ok(false);
        }
    }
    Ok(true)
}

fn audit_phrase_coverage(
    source: &Path,
    allowlist: &Path,
    base_payload: &Path,
    fit_corpus: &Path,
    held_out_corpus: &Path,
    entry_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let source_text = read_explicit_text(
        source,
        "public toned Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let allowlist_text = read_explicit_text(
        allowlist,
        "public fixed-phrase allowlist",
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES,
    )?;
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let source_sha256 = candidate_sha256_hex(source_text.as_bytes());
    let allowlist_sha256 = candidate_sha256_hex(allowlist_text.as_bytes());
    let base_sha256 = candidate_sha256_hex(base_text.as_bytes());
    let fit_sha256 = candidate_sha256_hex(fit_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if fit_sha256 == held_out_sha256 {
        return Err("phrase-coverage-audit requires a distinct held-out corpus".into());
    }

    let base = parse_lexicon_tsv(&base_text)?;
    let imported = parse_public_rime_phrase_allowlist(
        &source_text,
        &allowlist_text,
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES,
    )?;
    let base_surfaces = base
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let mut available = imported
        .entries
        .iter()
        .filter(|entry| !base_surfaces.contains(entry.text.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let available_new_entries = available.len();
    available.truncate(entry_limit);
    let selected_entries = available.len();
    let minimum_selected_frequency = available.last().map_or(0, |entry| entry.frequency);
    let mut challenger = base.clone();
    challenger.extend(available);

    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let fit_audit = audit_public_lexicon_token_coverage(&fit, &base, &challenger);
    let held_out_audit = audit_public_lexicon_token_coverage(&held_out, &base, &challenger);
    let stats = imported.stats;
    let mut output = format!(
        "公开固定短语覆盖留出审计\n基础载荷：{} 条 · SHA-256 {base_sha256}\n来源词典：SHA-256 {source_sha256}\n固定短语表：SHA-256 {allowlist_sha256}\n",
        base.len(),
    );
    writeln!(
        output,
        "短语表：{} 行；合格四字词面 {}，重复 {}，非四字 {}，格式异常 {}",
        stats.allowlist_rows,
        stats.eligible_terms,
        stats.duplicate_terms,
        stats.non_four_character_terms,
        stats.malformed_allowlist_rows,
    )
    .unwrap();
    writeln!(
        output,
        "来源交集：可用词面 {}，相关源行 {}，无效相关源行 {}；基础已覆盖 {}，新增候选 {}",
        stats.matched_terms,
        stats.allowlisted_source_rows,
        stats.invalid_allowlisted_source_rows,
        stats.matched_terms.saturating_sub(available_new_entries),
        available_new_entries,
    )
    .unwrap();
    writeln!(
        output,
        "实验配额：{}；实际加入 {}；配额外 {}；最低入选来源权重 {}",
        entry_limit,
        selected_entries,
        available_new_entries.saturating_sub(selected_entries),
        minimum_selected_frequency,
    )
    .unwrap();
    write_phrase_coverage_section(&mut output, "训练侧参考", &fit_sha256, fit.stats, fit_audit);
    write_phrase_coverage_section(
        &mut output,
        "留出评测",
        &held_out_sha256,
        held_out.stats,
        held_out_audit,
    );
    output.push_str(
        "口径：短语表只决定允许研究哪些四字词面；拼音、规范码和权重仍来自同修订 Rime 词典。结果只比较公开 UD token 词面覆盖，不运行候选排序，不构建或安装候选包。真正发布前仍须让两个输入文件同时进入可认证来源声明。\n本次操作：只读\n",
    );
    Ok(output)
}

fn write_phrase_coverage_section(
    output: &mut String,
    label: &str,
    corpus_sha256: &str,
    corpus_stats: UdCorpusImportStats,
    audit: PublicLexiconTokenCoverageAudit,
) {
    let four = audit.lengths[2];
    writeln!(
        output,
        "{label}：{} 句，{} 个句法 token · SHA-256 {corpus_sha256}",
        corpus_stats.sentences, corpus_stats.syntactic_tokens,
    )
    .unwrap();
    writeln!(
        output,
        "  四字词面 {}（实例 {}）；基础覆盖 {}（实例 {}），实验覆盖 {}（实例 {}）；新增 {}（实例 {}），丢失 {}（实例 {}）",
        four.source_unique_tokens,
        four.source_token_instances,
        four.base_covered_unique_tokens,
        four.base_covered_token_instances,
        four.challenger_covered_unique_tokens,
        four.challenger_covered_token_instances,
        four.challenger_gained_unique_tokens,
        four.challenger_gained_token_instances,
        four.challenger_lost_unique_tokens,
        four.challenger_lost_token_instances,
    )
    .unwrap();
}

const PHRASE_LAYER_RANK_DEPTH: usize = 10;
const PHRASE_LAYER_BENCHMARK_CODE_LIMIT: usize = 48;
const PHRASE_LAYER_CONTROL_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WeightedRankCounts {
    at_one: usize,
    at_one_instances: usize,
    at_five: usize,
    at_five_instances: usize,
    at_ten: usize,
    at_ten_instances: usize,
}

impl WeightedRankCounts {
    fn observe(&mut self, rank: Option<usize>, instances: usize) {
        let Some(rank) = rank else {
            return;
        };
        if rank <= 10 {
            self.at_ten += 1;
            self.at_ten_instances += instances;
        }
        if rank <= 5 {
            self.at_five += 1;
            self.at_five_instances += instances;
        }
        if rank == 1 {
            self.at_one += 1;
            self.at_one_instances += instances;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WeightedRankMovement {
    improved: usize,
    improved_instances: usize,
    unchanged: usize,
    unchanged_instances: usize,
    worsened: usize,
    worsened_instances: usize,
    newly_visible: usize,
    newly_visible_instances: usize,
    lost_visible: usize,
    lost_visible_instances: usize,
}

impl WeightedRankMovement {
    fn observe(&mut self, before: Option<usize>, after: Option<usize>, instances: usize) {
        match (before, after) {
            (None, Some(_)) => {
                self.improved += 1;
                self.improved_instances += instances;
                self.newly_visible += 1;
                self.newly_visible_instances += instances;
            }
            (Some(_), None) => {
                self.worsened += 1;
                self.worsened_instances += instances;
                self.lost_visible += 1;
                self.lost_visible_instances += instances;
            }
            (Some(before), Some(after)) if after < before => {
                self.improved += 1;
                self.improved_instances += instances;
            }
            (Some(before), Some(after)) if after > before => {
                self.worsened += 1;
                self.worsened_instances += instances;
            }
            _ => {
                self.unchanged += 1;
                self.unchanged_instances += instances;
            }
        }
    }

    fn absorb(&mut self, other: Self) {
        self.improved += other.improved;
        self.improved_instances += other.improved_instances;
        self.unchanged += other.unchanged;
        self.unchanged_instances += other.unchanged_instances;
        self.worsened += other.worsened;
        self.worsened_instances += other.worsened_instances;
        self.newly_visible += other.newly_visible;
        self.newly_visible_instances += other.newly_visible_instances;
        self.lost_visible += other.lost_visible;
        self.lost_visible_instances += other.lost_visible_instances;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PhraseRankComparison {
    probes: usize,
    instances: usize,
    core: WeightedRankCounts,
    small: WeightedRankCounts,
    large: WeightedRankCounts,
    small_vs_core: WeightedRankMovement,
    large_vs_core: WeightedRankMovement,
    small_order_changes: usize,
    large_order_changes: usize,
    small_top_changes: usize,
    large_top_changes: usize,
}

struct PhraseLayerAuditRequest<'a> {
    source: &'a Path,
    allowlist: &'a Path,
    base_payload: &'a Path,
    fit_corpus: &'a Path,
    held_out_corpus: &'a Path,
    small_limit: usize,
    large_limit: usize,
    repetitions: usize,
}

fn audit_phrase_layers(
    request: PhraseLayerAuditRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let PhraseLayerAuditRequest {
        source,
        allowlist,
        base_payload,
        fit_corpus,
        held_out_corpus,
        small_limit,
        large_limit,
        repetitions,
    } = request;
    if cfg!(debug_assertions) {
        return Err("phrase-layer-audit must run from a release build".into());
    }
    let source_text = read_explicit_text(
        source,
        "public toned Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let allowlist_text = read_explicit_text(
        allowlist,
        "public fixed-phrase allowlist",
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES,
    )?;
    let base_text = read_explicit_text(
        base_payload,
        "base public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let fit_sha256 = candidate_sha256_hex(fit_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if fit_sha256 == held_out_sha256 {
        return Err("phrase-layer-audit requires a distinct held-out corpus".into());
    }

    let base_entries = parse_lexicon_tsv(&base_text)?;
    let imported = parse_public_rime_phrase_allowlist(
        &source_text,
        &allowlist_text,
        MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES,
    )?;
    let base_surfaces = base_entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let available = imported
        .entries
        .into_iter()
        .filter(|entry| !base_surfaces.contains(entry.text.as_str()))
        .collect::<Vec<_>>();
    if available.len() < large_limit {
        return Err("phrase-layer-audit large limit exceeds available new public entries".into());
    }
    let small_entries = available[..small_limit].to_vec();
    let large_entries = available[..large_limit].to_vec();
    let small_payload = serialize_lexicon_payload(&small_entries);
    let large_payload = serialize_lexicon_payload(&large_entries);

    let core_started = Instant::now();
    let core = snapshot_from_payload("phrase-layer-core-v1", &base_text)?;
    let core_build = core_started.elapsed();
    let small_started = Instant::now();
    let small = snapshot_from_payload("phrase-layer-small-v1", &small_payload)?;
    let small_build = small_started.elapsed();
    let large_started = Instant::now();
    let large = snapshot_from_payload("phrase-layer-large-v1", &large_payload)?;
    let large_build = large_started.elapsed();

    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let fit_targets = select_public_lexicon_rank_probes(&fit, &large_entries, 4);
    let held_out_targets = select_public_lexicon_rank_probes(&held_out, &large_entries, 4);
    let fit_controls = select_public_lexicon_rank_probes(&fit, &base_entries, 4);
    let held_out_controls = select_public_lexicon_rank_probes(&held_out, &base_entries, 4);
    let fit_control_unique = fit_controls.probes.len();
    let fit_control_instances = fit_controls.matched_token_instances;
    let held_out_control_unique = held_out_controls.probes.len();
    let held_out_control_instances = held_out_controls.matched_token_instances;
    let fit_control_probes = bounded_public_rank_probes(fit_controls.probes);
    let held_out_control_probes = bounded_public_rank_probes(held_out_controls.probes);
    let fit_target_report = compare_phrase_layer_ranks(&core, &small, &large, &fit_targets.probes)?;
    let held_out_target_report =
        compare_phrase_layer_ranks(&core, &small, &large, &held_out_targets.probes)?;
    let fit_control_report =
        compare_phrase_layer_ranks(&core, &small, &large, &fit_control_probes)?;
    let held_out_control_report =
        compare_phrase_layer_ranks(&core, &small, &large, &held_out_control_probes)?;
    if fit_control_report.small_top_changes != 0
        || fit_control_report.large_top_changes != 0
        || held_out_control_report.small_top_changes != 0
        || held_out_control_report.large_top_changes != 0
    {
        return Err("phrase layer changed a public base-control Top-1".into());
    }

    let mut benchmark_codes = BTreeSet::new();
    for probe in fit_targets
        .probes
        .iter()
        .chain(&held_out_targets.probes)
        .chain(&fit_control_probes)
        .chain(&held_out_control_probes)
    {
        benchmark_codes.insert(probe.observed.as_str().to_owned());
    }
    let benchmark_codes = benchmark_codes
        .into_iter()
        .take(PHRASE_LAYER_BENCHMARK_CODE_LIMIT)
        .collect::<Vec<_>>();
    if benchmark_codes.is_empty() {
        return Err("phrase-layer-audit produced no public benchmark codes".into());
    }
    let config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    for code in &benchmark_codes {
        black_box(core.candidate_texts(black_box(code), PHRASE_LAYER_RANK_DEPTH)?);
        black_box(layered_candidate_texts(
            &core,
            &small,
            black_box(code),
            PHRASE_LAYER_RANK_DEPTH,
            config,
        )?);
        black_box(layered_candidate_texts(
            &core,
            &large,
            black_box(code),
            PHRASE_LAYER_RANK_DEPTH,
            config,
        )?);
    }
    let mut core_durations = Vec::with_capacity(repetitions * benchmark_codes.len());
    let mut small_durations = Vec::with_capacity(repetitions * benchmark_codes.len());
    let mut large_durations = Vec::with_capacity(repetitions * benchmark_codes.len());
    let mut checksum = 0usize;
    for _ in 0..repetitions {
        for code in &benchmark_codes {
            let started = Instant::now();
            let candidates = core.candidate_texts(black_box(code), PHRASE_LAYER_RANK_DEPTH)?;
            core_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);

            let started = Instant::now();
            let candidates = layered_candidate_texts(
                &core,
                &small,
                black_box(code),
                PHRASE_LAYER_RANK_DEPTH,
                config,
            )?;
            small_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);

            let started = Instant::now();
            let candidates = layered_candidate_texts(
                &core,
                &large,
                black_box(code),
                PHRASE_LAYER_RANK_DEPTH,
                config,
            )?;
            large_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);
        }
    }
    let core_latency = summarize_durations(&mut core_durations)
        .ok_or("phrase-layer-audit produced no core timings")?;
    let small_latency = summarize_durations(&mut small_durations)
        .ok_or("phrase-layer-audit produced no small-layer timings")?;
    let large_latency = summarize_durations(&mut large_durations)
        .ok_or("phrase-layer-audit produced no large-layer timings")?;

    let mut output = format!(
        "公开固定短语层排序与成本审计\n基础载荷：{} 条 · SHA-256 {}\n来源词典：SHA-256 {}\n固定短语表：SHA-256 {}\n配额：小层 {small_limit}；大层 {large_limit}；每码最多补 1 个完整词；候选深度 {PHRASE_LAYER_RANK_DEPTH}\n",
        base_entries.len(),
        candidate_sha256_hex(base_text.as_bytes()),
        candidate_sha256_hex(source_text.as_bytes()),
        candidate_sha256_hex(allowlist_text.as_bytes()),
    );
    write_phrase_rank_report(
        &mut output,
        "训练侧新增目标",
        &fit_sha256,
        fit_target_report,
    );
    write_phrase_rank_report(
        &mut output,
        "留出新增目标",
        &held_out_sha256,
        held_out_target_report,
    );
    write_phrase_control_report(
        &mut output,
        "训练侧基础对照",
        fit_control_unique,
        fit_control_instances,
        fit_control_report,
    );
    write_phrase_control_report(
        &mut output,
        "留出基础对照",
        held_out_control_unique,
        held_out_control_instances,
        held_out_control_report,
    );
    write_phrase_index_report(&mut output, "基础", &core, core.index_stats(), core_build);
    write_phrase_index_report(
        &mut output,
        "小层",
        &small,
        small.index_stats(),
        small_build,
    );
    write_phrase_index_report(
        &mut output,
        "大层",
        &large,
        large.index_stats(),
        large_build,
    );
    writeln!(
        output,
        "预热查询：{} 个公开完整码 × {repetitions} 次；样本 {}",
        benchmark_codes.len(),
        core_latency.samples,
    )?;
    write_phrase_latency_report(&mut output, "基础", core_latency);
    write_phrase_latency_report(&mut output, "小层", small_latency);
    write_phrase_latency_report(&mut output, "大层", large_latency);
    writeln!(output, "结果校验和：{checksum}")?;
    output.push_str(
        "口径：新增目标与基础稳定性对照分开统计；短语层通过真实 CandidateSnapshot 和交互合并路径查询。索引结构不是堆内存估算，预热查询也不是 TSF 绘制首帧；耗时只作本机同次诊断。命令不显示词面、不构建候选包、不写槽位。双文件来源认证与宿主首帧仍是发布前独立门槛。\n本次操作：只读\n",
    );
    Ok(output)
}

const EXACT_PHRASE_CHARACTERS: usize = 3;
const EXACT_PHRASE_MAX_TOKENS: usize = 3;
const EXACT_PHRASE_RANK_DEPTH: usize = 10;
const EXACT_PHRASE_FIRST_PAGE: usize = 6;
const EXACT_PHRASE_TSF_MAX_SCAN_PER_ANCHOR: usize = 64;
const EXACT_PHRASE_BENCHMARK_CODE_LIMIT: usize = 48;

struct ExactPhraseLayerAuditRequest<'a> {
    source: &'a Path,
    core_payload: &'a Path,
    supplemental_payload: &'a Path,
    fit_corpus: &'a Path,
    held_out_corpus: &'a Path,
    entry_limit: usize,
    repetitions: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactPhraseSourceSelectionStats {
    source_rows: usize,
    requested_surfaces: usize,
    requested_instances: usize,
    requested_source_rows: usize,
    valid_requested_source_rows: usize,
    matched_identities: usize,
    ambiguous_surfaces: usize,
    ambiguous_codes: usize,
    existing_identities: usize,
    code_collisions: usize,
    dropped_by_entry_cap: usize,
    selected_entries: usize,
}

struct ExactPhraseSourceSelection {
    entries: Vec<LexiconEntry>,
    stats: ExactPhraseSourceSelectionStats,
}

#[derive(Clone)]
struct RankedExactPhraseEntry {
    entry: LexiconEntry,
    source_line: usize,
    fit_instances: usize,
}

impl RankedExactPhraseEntry {
    fn precedes(&self, other: &Self) -> bool {
        self.fit_instances > other.fit_instances
            || (self.fit_instances == other.fit_instances
                && (self.entry.frequency > other.entry.frequency
                    || (self.entry.frequency == other.entry.frequency
                        && (self.source_line, self.entry.text.as_str())
                            < (other.source_line, other.entry.text.as_str()))))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactPhraseRankCounts {
    at_one: usize,
    at_one_instances: usize,
    at_six: usize,
    at_six_instances: usize,
    at_ten: usize,
    at_ten_instances: usize,
}

impl ExactPhraseRankCounts {
    fn observe(&mut self, rank: Option<usize>, instances: usize) {
        let Some(rank) = rank else {
            return;
        };
        if rank <= 10 {
            self.at_ten += 1;
            self.at_ten_instances += instances;
        }
        if rank <= EXACT_PHRASE_FIRST_PAGE {
            self.at_six += 1;
            self.at_six_instances += instances;
        }
        if rank == 1 {
            self.at_one += 1;
            self.at_one_instances += instances;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactPhraseRankComparison {
    probes: usize,
    instances: usize,
    baseline: ExactPhraseRankCounts,
    preview: ExactPhraseRankCounts,
    movement: WeightedRankMovement,
    order_changes: usize,
    order_change_instances: usize,
    top_changes: usize,
    top_change_instances: usize,
    correct_top_gains: usize,
    correct_top_gain_instances: usize,
    correct_top_losses: usize,
    correct_top_loss_instances: usize,
    non_target_top_changes: usize,
    non_target_top_change_instances: usize,
    first_page_degradations: usize,
    first_page_degradation_instances: usize,
    cross_page_degradations: usize,
    cross_page_degradation_instances: usize,
}

impl ExactPhraseRankComparison {
    fn observe(&mut self, before: &[String], after: &[String], expected: &str, instances: usize) {
        let before_rank = candidate_rank(before, expected);
        let after_rank = candidate_rank(after, expected);
        let order_changed = before != after;
        let top_changed = before.first() != after.first();
        let before_correct = before
            .first()
            .is_some_and(|candidate| candidate == expected);
        let after_correct = after.first().is_some_and(|candidate| candidate == expected);
        let first_page_degradation = before_rank
            .is_some_and(|rank| rank <= EXACT_PHRASE_FIRST_PAGE)
            && after_rank.is_none_or(|rank| rank > EXACT_PHRASE_FIRST_PAGE);
        let cross_page_degradation = match (before_rank, after_rank) {
            (Some(before), Some(after)) => {
                (after - 1) / EXACT_PHRASE_FIRST_PAGE > (before - 1) / EXACT_PHRASE_FIRST_PAGE
            }
            (Some(_), None) => true,
            _ => false,
        };

        self.probes += 1;
        self.instances += instances;
        self.baseline.observe(before_rank, instances);
        self.preview.observe(after_rank, instances);
        self.movement.observe(before_rank, after_rank, instances);
        self.order_changes += usize::from(order_changed);
        self.order_change_instances += instances * usize::from(order_changed);
        self.top_changes += usize::from(top_changed);
        self.top_change_instances += instances * usize::from(top_changed);
        self.correct_top_gains += usize::from(!before_correct && after_correct);
        self.correct_top_gain_instances +=
            instances * usize::from(!before_correct && after_correct);
        self.correct_top_losses += usize::from(before_correct && !after_correct);
        self.correct_top_loss_instances +=
            instances * usize::from(before_correct && !after_correct);
        self.non_target_top_changes +=
            usize::from(top_changed && !before_correct && !after_correct);
        self.non_target_top_change_instances +=
            instances * usize::from(top_changed && !before_correct && !after_correct);
        self.first_page_degradations += usize::from(first_page_degradation);
        self.first_page_degradation_instances += instances * usize::from(first_page_degradation);
        self.cross_page_degradations += usize::from(cross_page_degradation);
        self.cross_page_degradation_instances += instances * usize::from(cross_page_degradation);
    }
}

fn audit_exact_phrase_layer(
    request: ExactPhraseLayerAuditRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-phrase-layer-audit must run from a release build".into());
    }
    let source_text = read_explicit_text(
        request.source,
        "public exact-phrase Rime source",
        MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    )?;
    let core_text = read_explicit_text(
        request.core_payload,
        "core public exact-phrase payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        request.supplemental_payload,
        "supplemental public exact-phrase payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        request.fit_corpus,
        "public exact-phrase fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        request.held_out_corpus,
        "public exact-phrase held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let fit_sha256 = candidate_sha256_hex(fit_text.as_bytes());
    let held_out_sha256 = candidate_sha256_hex(held_out_text.as_bytes());
    if fit_sha256 == held_out_sha256 {
        return Err("exact-phrase-layer-audit requires a distinct held-out corpus".into());
    }
    let core_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let supplemental_sha256 = candidate_sha256_hex(supplemental_text.as_bytes());
    if core_sha256 == supplemental_sha256 {
        return Err("exact-phrase-layer-audit requires distinct core and supplemental data".into());
    }

    let core_entries = parse_lexicon_tsv(&core_text)?;
    let supplemental_entries = parse_lexicon_tsv(&supplemental_text)?;
    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let fit_spans = select_public_han_span_rank_probes(
        &fit,
        &core_entries,
        EXACT_PHRASE_CHARACTERS,
        EXACT_PHRASE_MAX_TOKENS,
    );
    let held_out_spans = select_public_han_span_rank_probes(
        &held_out,
        &core_entries,
        EXACT_PHRASE_CHARACTERS,
        EXACT_PHRASE_MAX_TOKENS,
    );
    let mut existing_entries = core_entries.clone();
    existing_entries.extend(supplemental_entries.iter().cloned());
    let selected = select_exact_phrase_source_entries(
        &source_text,
        &fit_spans.probes,
        &existing_entries,
        request.entry_limit,
    )?;
    if selected.entries.is_empty() {
        return Err("exact-phrase-layer-audit selected no new public identities".into());
    }

    let core_started = Instant::now();
    let core = snapshot_from_payload("exact-phrase-core-v1", &core_text)?;
    let core_build = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental = snapshot_from_payload("exact-phrase-supplemental-v1", &supplemental_text)?;
    let supplemental_build = supplemental_started.elapsed();
    let phrase_payload = serialize_lexicon_payload(&selected.entries);
    let phrase_started = Instant::now();
    let phrase = snapshot_from_payload("exact-phrase-fit-v1", &phrase_payload)?;
    let phrase_build = phrase_started.elapsed();

    let selected_identities = selected
        .entries
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let selected_codes = selected
        .entries
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let fit_targets = fit_spans
        .probes
        .iter()
        .filter(|probe| {
            selected_identities.contains(&(probe.expected_text.as_str(), probe.observed.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let held_out_targets = held_out_spans
        .probes
        .iter()
        .filter(|probe| {
            selected_identities.contains(&(probe.expected_text.as_str(), probe.observed.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let held_out_controls = held_out_spans
        .probes
        .iter()
        .filter(|probe| {
            selected_codes.contains(probe.observed.as_str())
                && !selected_identities
                    .contains(&(probe.expected_text.as_str(), probe.observed.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut held_out_exact_controls = Vec::new();
    let mut held_out_composition_controls = Vec::new();
    for probe in &held_out_controls {
        if existing_exact_identity(&core, &supplemental, probe)? {
            held_out_exact_controls.push(probe.clone());
        } else {
            held_out_composition_controls.push(probe.clone());
        }
    }

    let fit_report = compare_exact_phrase_ranks(
        &core,
        &supplemental,
        &phrase,
        &fit_targets,
        EXACT_PHRASE_RANK_DEPTH,
    )?;
    let held_out_target_report = compare_exact_phrase_ranks(
        &core,
        &supplemental,
        &phrase,
        &held_out_targets,
        EXACT_PHRASE_RANK_DEPTH,
    )?;
    let held_out_exact_control_report = compare_exact_phrase_ranks(
        &core,
        &supplemental,
        &phrase,
        &held_out_exact_controls,
        EXACT_PHRASE_RANK_DEPTH,
    )?;
    let held_out_composition_control_report = compare_exact_phrase_ranks(
        &core,
        &supplemental,
        &phrase,
        &held_out_composition_controls,
        EXACT_PHRASE_RANK_DEPTH,
    )?;

    let benchmark_codes =
        evenly_spaced_exact_short_codes(&selected.entries, EXACT_PHRASE_BENCHMARK_CODE_LIMIT);
    for code in &benchmark_codes {
        black_box(layered_candidate_texts(
            &core,
            &supplemental,
            black_box(code),
            EXACT_PHRASE_RANK_DEPTH,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )?);
        black_box(preview_exact_phrase_candidates(
            &core,
            &supplemental,
            &phrase,
            black_box(code),
            EXACT_PHRASE_RANK_DEPTH,
        )?);
    }
    let mut baseline_durations = Vec::with_capacity(request.repetitions * benchmark_codes.len());
    let mut preview_durations = Vec::with_capacity(request.repetitions * benchmark_codes.len());
    let mut checksum = 0_usize;
    for _ in 0..request.repetitions {
        for code in &benchmark_codes {
            let started = Instant::now();
            let candidates = layered_candidate_texts(
                &core,
                &supplemental,
                black_box(code),
                EXACT_PHRASE_RANK_DEPTH,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )?;
            baseline_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);

            let started = Instant::now();
            let candidates = preview_exact_phrase_candidates(
                &core,
                &supplemental,
                &phrase,
                black_box(code),
                EXACT_PHRASE_RANK_DEPTH,
            )?;
            preview_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);
        }
    }
    let baseline_latency = summarize_durations(&mut baseline_durations)
        .ok_or("exact-phrase-layer-audit produced no baseline timings")?;
    let preview_latency = summarize_durations(&mut preview_durations)
        .ok_or("exact-phrase-layer-audit produced no preview timings")?;

    let gate_has_held_out_benefit = held_out_target_report.correct_top_gains != 0;
    let gate_has_no_target_harm = held_out_target_report.correct_top_losses == 0
        && held_out_target_report.non_target_top_changes == 0
        && held_out_target_report.movement.worsened == 0
        && held_out_target_report.movement.lost_visible == 0;
    let gate_has_no_exact_control_harm = held_out_exact_control_report.top_changes == 0
        && held_out_exact_control_report.movement.worsened == 0
        && held_out_exact_control_report.movement.lost_visible == 0;
    let gate_keeps_compositions_available =
        held_out_composition_control_report.cross_page_degradations == 0
            && held_out_composition_control_report.movement.lost_visible == 0;
    let safety_gate_passed = gate_has_held_out_benefit
        && gate_has_no_target_harm
        && gate_has_no_exact_control_harm
        && gate_keeps_compositions_available;

    let mut output = String::new();
    writeln!(output, "公开三字精确短语层留出审计")?;
    writeln!(
        output,
        "核心：{} 条 · SHA-256 {core_sha256}",
        core_entries.len()
    )?;
    writeln!(
        output,
        "补充：{} 条 · SHA-256 {supplemental_sha256}；基线每码补 1 项",
        supplemental_entries.len()
    )?;
    writeln!(
        output,
        "来源词典：SHA-256 {}；训练侧三字 span 选层；最多 {} 个相邻 token",
        candidate_sha256_hex(source_text.as_bytes()),
        EXACT_PHRASE_MAX_TOKENS,
    )?;
    write_exact_phrase_span_selection(&mut output, "训练选择", &fit_sha256, &fit_spans);
    write_exact_phrase_span_selection(&mut output, "独立留出", &held_out_sha256, &held_out_spans);
    let stats = selected.stats;
    writeln!(
        output,
        "来源交集：请求词面 {}（实例 {}）；相关源行 {}，有效 {}；匹配身份 {}；多音词面排除 {}；来源同码歧义排除 {}；既有身份排除 {}；训练同码竞争舍弃 {}；配额外 {}；最终 {}",
        stats.requested_surfaces,
        stats.requested_instances,
        stats.requested_source_rows,
        stats.valid_requested_source_rows,
        stats.matched_identities,
        stats.ambiguous_surfaces,
        stats.ambiguous_codes,
        stats.existing_identities,
        stats.code_collisions,
        stats.dropped_by_entry_cap,
        stats.selected_entries,
    )?;
    write_exact_phrase_rank_report(&mut output, "训练侧新增目标", fit_report);
    write_exact_phrase_rank_report(&mut output, "留出侧新增目标", held_out_target_report);
    write_exact_phrase_rank_report(
        &mut output,
        "留出侧同码既有整词对照",
        held_out_exact_control_report,
    );
    write_exact_phrase_rank_report(
        &mut output,
        "留出侧同码自由组合对照",
        held_out_composition_control_report,
    );
    write_exact_phrase_public_counterexamples(
        &mut output,
        &core,
        &supplemental,
        &phrase,
        &held_out_composition_controls,
    )?;
    writeln!(
        output,
        "严格安全门：{}（留出正确首选新增 {}；目标变差 {}；既有整词首选变化 {}；自由组合跨页 {}、掉出 Top-10 {}）",
        if safety_gate_passed {
            "通过"
        } else if held_out_target_report.probes == 0 {
            "证据不足"
        } else {
            "未通过"
        },
        held_out_target_report.correct_top_gains,
        held_out_target_report.movement.worsened,
        held_out_exact_control_report.top_changes,
        held_out_composition_control_report.cross_page_degradations,
        held_out_composition_control_report.movement.lost_visible,
    )?;
    write_phrase_index_report(&mut output, "核心", &core, core.index_stats(), core_build);
    write_phrase_index_report(
        &mut output,
        "补充",
        &supplemental,
        supplemental.index_stats(),
        supplemental_build,
    );
    write_phrase_index_report(
        &mut output,
        "三字层",
        &phrase,
        phrase.index_stats(),
        phrase_build,
    );
    writeln!(
        output,
        "预热查询：{} 个训练侧公开完整码 × {} 次；样本 {}",
        benchmark_codes.len(),
        request.repetitions,
        baseline_latency.samples,
    )?;
    write_phrase_latency_report(&mut output, "既有两层", baseline_latency);
    write_phrase_latency_report(&mut output, "加入三字精确层", preview_latency);
    writeln!(output, "结果校验和：{checksum}")?;
    output.push_str(
        "口径：训练侧只用公开 UD 的一至三 token 三字 span 选择固定万象中同码、单音且既有两层缺失的完整词；独立留出才决定收益与同码伤害。层内每码最多一个词；既有完整词通道存在时保持其首项，无既有完整词时才允许来源确认的整词越过机械组合。该审计不把私人样本用于选阈值，不构建候选包、不写槽位、不安装或换代。\n本次操作：只读\n",
    );
    Ok(output)
}

struct ExactPhraseLayerPreflightRequest<'a> {
    core_package: &'a Path,
    supplemental_package: &'a Path,
    phrase_package: &'a Path,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactPhraseLayerPreflightSummary {
    core_revision: String,
    supplemental_revision: String,
    phrase_revision: String,
    core_authentication_sha256: String,
    supplemental_authentication_sha256: String,
    phrase_authentication_sha256: String,
    core_load: Duration,
    supplemental_load: Duration,
    phrase_load: Duration,
    phrase_entries: usize,
    phrase_codes: usize,
    catalog_audit: ExactPhraseCatalogAudit,
    catalog_codes_by_rank: [Vec<String>; EXACT_PHRASE_FIRST_PAGE],
    catalog_audit_duration: Duration,
    sampled_codes: usize,
    negative_control_codes: usize,
    repetitions: usize,
    baseline_latency: DurationSummary,
    preview_latency: DurationSummary,
    checksum: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExactPhraseTsfProbeSource {
    RankBucket { rank_index: usize, anchor: usize },
    Catalog { anchor: usize },
}

fn plan_exact_phrase_tsf_probe_sources(
    rank_counts: [usize; EXACT_PHRASE_FIRST_PAGE],
    catalog_len: usize,
    sample_limit: usize,
) -> Result<Vec<ExactPhraseTsfProbeSource>, Box<dyn std::error::Error>> {
    if catalog_len == 0
        || sample_limit == 0
        || sample_limit > catalog_len
        || rank_counts.iter().sum::<usize>() != catalog_len
    {
        return Err("exact phrase TSF probe plan has an invalid aggregate".into());
    }
    let populated_ranks = rank_counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .collect::<Vec<_>>();
    if sample_limit < populated_ranks.len() {
        return Err("exact phrase TSF probe count is below the authenticated rank coverage".into());
    }

    let mut plan = Vec::with_capacity(sample_limit);
    for (rank_index, count) in populated_ranks {
        plan.push(ExactPhraseTsfProbeSource::RankBucket {
            rank_index,
            anchor: count / 2,
        });
    }
    let remaining = sample_limit - plan.len();
    for index in 0..remaining {
        plan.push(ExactPhraseTsfProbeSource::Catalog {
            anchor: index * catalog_len / remaining,
        });
    }
    Ok(plan)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactPhrasePreviewObservation {
    existing_exact_prefix: usize,
    target_rank: Option<usize>,
    target_instances: usize,
    guarded_rank_matches: bool,
    existing_prefix_unchanged: bool,
    preview_unique: bool,
    preview_within_bound: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExactPhraseCatalogAudit {
    targets: usize,
    without_existing_exact_prefix: usize,
    after_existing_exact_prefix: usize,
    target_ranks: [usize; EXACT_PHRASE_FIRST_PAGE],
    targets_outside_first_page: usize,
    missing_targets: usize,
    repeated_targets: usize,
    guarded_rank_mismatches: usize,
    existing_prefix_changes: usize,
    duplicate_previews: usize,
    unbounded_previews: usize,
}

impl ExactPhraseCatalogAudit {
    fn observe(&mut self, observation: ExactPhrasePreviewObservation) {
        self.targets += 1;
        if observation.existing_exact_prefix == 0 {
            self.without_existing_exact_prefix += 1;
        } else {
            self.after_existing_exact_prefix += 1;
        }
        match observation.target_rank {
            Some(rank @ 1..=EXACT_PHRASE_FIRST_PAGE) => self.target_ranks[rank - 1] += 1,
            Some(_) => self.targets_outside_first_page += 1,
            None => self.missing_targets += 1,
        }
        self.repeated_targets += usize::from(observation.target_instances > 1);
        self.guarded_rank_mismatches += usize::from(!observation.guarded_rank_matches);
        self.existing_prefix_changes += usize::from(!observation.existing_prefix_unchanged);
        self.duplicate_previews += usize::from(!observation.preview_unique);
        self.unbounded_previews += usize::from(!observation.preview_within_bound);
    }

    fn verify(self) -> Result<Self, Box<dyn std::error::Error>> {
        let classified_targets = self.target_ranks.iter().sum::<usize>()
            + self.targets_outside_first_page
            + self.missing_targets;
        let classified_prefixes =
            self.without_existing_exact_prefix + self.after_existing_exact_prefix;
        if self.targets == 0
            || classified_targets != self.targets
            || classified_prefixes != self.targets
        {
            return Err("exact phrase full-catalog audit produced an incomplete aggregate".into());
        }
        if self.targets_outside_first_page != 0
            || self.missing_targets != 0
            || self.repeated_targets != 0
            || self.guarded_rank_mismatches != 0
            || self.existing_prefix_changes != 0
            || self.duplicate_previews != 0
            || self.unbounded_previews != 0
        {
            return Err(format!(
                "exact phrase full-catalog audit failed without candidate text: targets {}; outside first page {}; missing {}; repeated targets {}; guarded rank mismatches {}; existing prefix changes {}; duplicate previews {}; unbounded previews {}",
                self.targets,
                self.targets_outside_first_page,
                self.missing_targets,
                self.repeated_targets,
                self.guarded_rank_mismatches,
                self.existing_prefix_changes,
                self.duplicate_previews,
                self.unbounded_previews,
            )
            .into());
        }
        Ok(self)
    }
}

struct ExactPhraseTsfPreflightRequest<'a> {
    core_package: &'a Path,
    supplemental_package: &'a Path,
    phrase_package: &'a Path,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactPhrasePopupPreflightRequest<'a> {
    core_package: &'a Path,
    supplemental_package: &'a Path,
    phrase_package: &'a Path,
    sample_limit: usize,
    repetitions: usize,
}

struct ExactPhraseTsfLayerIdentity {
    revision: String,
    authentication_sha256: String,
    load_duration: Duration,
}

struct ExactPhraseTsfPreflightSummary {
    core: ExactPhraseTsfLayerIdentity,
    supplemental: ExactPhraseTsfLayerIdentity,
    phrase: ExactPhraseTsfLayerIdentity,
    phrase_entries: usize,
    phrase_codes: usize,
    requested_probes: usize,
    inspected_codes: usize,
    repetitions: usize,
    target_rank_samples: [usize; EXACT_PHRASE_FIRST_PAGE],
    first_page: DurationSummary,
    commit: DurationSummary,
    complete_path: DurationSummary,
}

#[cfg(windows)]
struct ExactPhrasePopupPreflightSummary {
    core: ExactPhraseTsfLayerIdentity,
    supplemental: ExactPhraseTsfLayerIdentity,
    phrase: ExactPhraseTsfLayerIdentity,
    phrase_entries: usize,
    phrase_codes: usize,
    requested_probes: usize,
    inspected_codes: usize,
    repetitions: usize,
    target_rank_probes: [usize; EXACT_PHRASE_FIRST_PAGE],
    tsf_first_page: DurationSummary,
    tsf_commit: DurationSummary,
    popup_samples: Vec<CandidatePopupRenderSample>,
    hide_durations: Vec<Duration>,
    destroy_durations: Vec<Duration>,
}

#[cfg(windows)]
struct ExactPhraseTsfProbe {
    code: String,
    expected_text: String,
    target_rank: usize,
    discovery_first_page: Duration,
    discovery_commit: Duration,
}

fn preflight_exact_phrase_layer(
    request: ExactPhraseLayerPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-phrase-layer-preflight must run from a release build".into());
    }
    let core_started = Instant::now();
    let core = load_public_package_directory(request.core_package)?;
    let core_load = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental = load_public_package_directory(request.supplemental_package)?;
    let supplemental_load = supplemental_started.elapsed();
    let phrase_started = Instant::now();
    let phrase = load_public_package_directory(request.phrase_package)?;
    let phrase_load = phrase_started.elapsed();
    let summary = preflight_loaded_exact_phrase_layer(
        &core,
        &supplemental,
        &phrase,
        request.sample_limit,
        request.repetitions,
        [core_load, supplemental_load, phrase_load],
    )?;
    let mut output = String::new();
    writeln!(output, "公开三字精确层三包组合预检")?;
    writeln!(
        output,
        "核心：{} · 认证 SHA-256 {} · 加载 {:.3} ms",
        summary.core_revision,
        summary.core_authentication_sha256,
        duration_ms(summary.core_load),
    )?;
    writeln!(
        output,
        "补充：{} · 认证 SHA-256 {} · 加载 {:.3} ms",
        summary.supplemental_revision,
        summary.supplemental_authentication_sha256,
        duration_ms(summary.supplemental_load),
    )?;
    writeln!(
        output,
        "三字层：{} · 认证 SHA-256 {} · 加载 {:.3} ms",
        summary.phrase_revision,
        summary.phrase_authentication_sha256,
        duration_ms(summary.phrase_load),
    )?;
    writeln!(
        output,
        "形状门：{} 条、{} 个唯一六键码；provenance 为四份公开材料并绑定当前核心/补充载荷",
        summary.phrase_entries, summary.phrase_codes,
    )?;
    writeln!(
        output,
        "全目录功能门：{} 个目标码；无既有完整词前缀 {}，跟随既有完整词前缀 {}；离线审计 {:.3} ms",
        summary.catalog_audit.targets,
        summary.catalog_audit.without_existing_exact_prefix,
        summary.catalog_audit.after_existing_exact_prefix,
        duration_ms(summary.catalog_audit_duration),
    )?;
    writeln!(
        output,
        "全目录目标位次：第 1 项 {}；第 2 项 {}；第 3 项 {}；第 4 项 {}；第 5 项 {}；第 6 项 {}",
        summary.catalog_audit.target_ranks[0],
        summary.catalog_audit.target_ranks[1],
        summary.catalog_audit.target_ranks[2],
        summary.catalog_audit.target_ranks[3],
        summary.catalog_audit.target_ranks[4],
        summary.catalog_audit.target_ranks[5],
    )?;
    writeln!(
        output,
        "全目录异常：跨出第一页 {}，目标缺失 {}，目标重复 {}，守护位次不符 {}，既有前缀变化 {}，候选重复 {}，结果越界 {}；负对照抽样 {} 个码保持无层回退",
        summary.catalog_audit.targets_outside_first_page,
        summary.catalog_audit.missing_targets,
        summary.catalog_audit.repeated_targets,
        summary.catalog_audit.guarded_rank_mismatches,
        summary.catalog_audit.existing_prefix_changes,
        summary.catalog_audit.duplicate_previews,
        summary.catalog_audit.unbounded_previews,
        summary.negative_control_codes,
    )?;
    writeln!(
        output,
        "热路径性能抽样：{} 个目标码 × {} 次；样本 {}",
        summary.sampled_codes, summary.repetitions, summary.baseline_latency.samples,
    )?;
    write_phrase_latency_report(&mut output, "既有两层", summary.baseline_latency);
    write_phrase_latency_report(&mut output, "三包预览", summary.preview_latency);
    writeln!(output, "结果校验和：{}", summary.checksum)?;
    output.push_str(
        "口径：全目录离线审计与有界性能抽样分开；前者证明本包全部目标的结构安全，耗时不是日常按键热路径。边界：本预检认证真实三包与纯候选合并路径，但不创建运行时根、不写开关、不注册或调用 TSF，也不测窗口绘制和真实首帧。通过不等于允许安装或换代。\n本次操作：只读\n",
    );
    Ok(output)
}

fn preflight_loaded_exact_phrase_layer(
    core: &LoadedPackage,
    supplemental: &LoadedPackage,
    phrase: &LoadedPackage,
    sample_limit: usize,
    repetitions: usize,
    load_durations: [Duration; 3],
) -> Result<ExactPhraseLayerPreflightSummary, Box<dyn std::error::Error>> {
    if !(1..=32).contains(&sample_limit) || !(1..=20).contains(&repetitions) {
        return Err("exact phrase preflight workload is outside the fixed bound".into());
    }
    if phrase.provenance.source_count() != 4 {
        return Err("exact phrase package must bind exactly four public materials".into());
    }
    let core_payload_sha256 = candidate_sha256_hex(core.payload_text.as_bytes());
    let supplemental_payload_sha256 = candidate_sha256_hex(supplemental.payload_text.as_bytes());
    let phrase_materials = phrase
        .provenance
        .source_materials()
        .iter()
        .map(CandidateSourceMaterial::sha256)
        .collect::<HashSet<_>>();
    if !phrase_materials.contains(core_payload_sha256.as_str())
        || !phrase_materials.contains(supplemental_payload_sha256.as_str())
    {
        return Err(
            "exact phrase provenance does not bind the supplied core and supplemental payloads"
                .into(),
        );
    }

    let phrase_entries = parse_lexicon_tsv(&phrase.payload_text)?;
    if phrase_entries.is_empty() || phrase_entries.len() > MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES
    {
        return Err("exact phrase package entry count is outside the fixed bound".into());
    }
    let mut texts_by_code = HashMap::<&str, &str>::new();
    for entry in &phrase_entries {
        if entry.text.chars().count() != EXACT_PHRASE_CHARACTERS
            || !entry.text.chars().all(is_han_phrase_character)
            || entry.syllable_codes.len() != EXACT_PHRASE_CHARACTERS
            || entry.code.as_str().len() != EXACT_PHRASE_CHARACTERS * 2
        {
            return Err("exact phrase package contains a non-three-character identity".into());
        }
        if texts_by_code
            .insert(entry.code.as_str(), entry.text.as_str())
            .is_some()
        {
            return Err("exact phrase package contains more than one identity for a code".into());
        }
    }

    let catalog_audit_started = Instant::now();
    let mut catalog_audit = ExactPhraseCatalogAudit::default();
    let mut catalog_codes_by_rank: [Vec<String>; EXACT_PHRASE_FIRST_PAGE] =
        std::array::from_fn(|_| Vec::new());
    for entry in &phrase_entries {
        let observation = inspect_exact_phrase_preview(
            entry.code.as_str(),
            entry.text.as_str(),
            &core.snapshot,
            &supplemental.snapshot,
            &phrase.snapshot,
        )?;
        if let Some(rank @ 1..=EXACT_PHRASE_FIRST_PAGE) = observation.target_rank {
            catalog_codes_by_rank[rank - 1].push(entry.code.as_str().to_owned());
        }
        catalog_audit.observe(observation);
    }
    let catalog_audit = catalog_audit.verify()?;
    for codes in &mut catalog_codes_by_rank {
        codes.sort_unstable();
    }
    if catalog_codes_by_rank
        .iter()
        .map(Vec::len)
        .ne(catalog_audit.target_ranks)
    {
        return Err("exact phrase full-catalog rank index disagrees with its aggregate".into());
    }
    let catalog_audit_duration = catalog_audit_started.elapsed();

    let sampled_codes = evenly_spaced_exact_short_codes(&phrase_entries, sample_limit);
    if sampled_codes.is_empty() {
        return Err("exact phrase preflight selected no target codes".into());
    }

    let phrase_codes = texts_by_code.keys().copied().collect::<HashSet<_>>();
    let negative_entries = parse_lexicon_tsv(&core.payload_text)?
        .into_iter()
        .filter(|entry| !phrase_codes.contains(entry.code.as_str()))
        .collect::<Vec<_>>();
    let negative_codes = evenly_spaced_exact_short_codes(&negative_entries, sample_limit);
    if negative_codes.is_empty() {
        return Err("exact phrase preflight selected no negative-control codes".into());
    }
    for code in &negative_codes {
        let baseline = layered_candidate_texts(
            &core.snapshot,
            &supplemental.snapshot,
            code,
            EXACT_PHRASE_RANK_DEPTH,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )?;
        let preview = preview_exact_phrase_candidates(
            &core.snapshot,
            &supplemental.snapshot,
            &phrase.snapshot,
            code,
            EXACT_PHRASE_RANK_DEPTH,
        )?;
        if preview != baseline {
            return Err("exact phrase layer changed a code absent from the layer".into());
        }
    }

    for code in &sampled_codes {
        black_box(layered_candidate_texts(
            &core.snapshot,
            &supplemental.snapshot,
            black_box(code),
            EXACT_PHRASE_RANK_DEPTH,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )?);
        black_box(preview_exact_phrase_candidates(
            &core.snapshot,
            &supplemental.snapshot,
            &phrase.snapshot,
            black_box(code),
            EXACT_PHRASE_RANK_DEPTH,
        )?);
    }
    let mut baseline_durations = Vec::with_capacity(repetitions * sampled_codes.len());
    let mut preview_durations = Vec::with_capacity(repetitions * sampled_codes.len());
    let mut checksum = 0_usize;
    for _ in 0..repetitions {
        for code in &sampled_codes {
            let started = Instant::now();
            let candidates = layered_candidate_texts(
                &core.snapshot,
                &supplemental.snapshot,
                black_box(code),
                EXACT_PHRASE_RANK_DEPTH,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )?;
            baseline_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);

            let started = Instant::now();
            let candidates = preview_exact_phrase_candidates(
                &core.snapshot,
                &supplemental.snapshot,
                &phrase.snapshot,
                black_box(code),
                EXACT_PHRASE_RANK_DEPTH,
            )?;
            preview_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);
        }
    }
    let baseline_latency = summarize_durations(&mut baseline_durations)
        .ok_or("exact phrase preflight produced no baseline timings")?;
    let preview_latency = summarize_durations(&mut preview_durations)
        .ok_or("exact phrase preflight produced no preview timings")?;
    Ok(ExactPhraseLayerPreflightSummary {
        core_revision: core.snapshot.revision().to_owned(),
        supplemental_revision: supplemental.snapshot.revision().to_owned(),
        phrase_revision: phrase.snapshot.revision().to_owned(),
        core_authentication_sha256: core.authentication_sha256.clone(),
        supplemental_authentication_sha256: supplemental.authentication_sha256.clone(),
        phrase_authentication_sha256: phrase.authentication_sha256.clone(),
        core_load: load_durations[0],
        supplemental_load: load_durations[1],
        phrase_load: load_durations[2],
        phrase_entries: phrase_entries.len(),
        phrase_codes: texts_by_code.len(),
        catalog_audit,
        catalog_codes_by_rank,
        catalog_audit_duration,
        sampled_codes: sampled_codes.len(),
        negative_control_codes: negative_codes.len(),
        repetitions,
        baseline_latency,
        preview_latency,
        checksum,
    })
}

fn inspect_exact_phrase_preview(
    code: &str,
    expected: &str,
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    phrase: &CandidateSnapshot,
) -> Result<ExactPhrasePreviewObservation, Box<dyn std::error::Error>> {
    let baseline = layered_candidate_texts(
        core,
        supplemental,
        code,
        EXACT_PHRASE_RANK_DEPTH,
        SupplementalCandidateLayerConfig {
            exact_promotions: 1,
        },
    )?;
    let phrase_exact = phrase.exact_full_code_texts(code, 1)?;
    let existing_exact =
        existing_exact_texts(core, supplemental, code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
    let (preview, stable_prefix) = match phrase_exact.first() {
        Some(candidate) => merge_exact_phrase_candidate_into_baseline(
            &baseline,
            &existing_exact,
            candidate,
            EXACT_PHRASE_RANK_DEPTH,
        ),
        None => {
            let stable_prefix = baseline
                .iter()
                .take_while(|candidate| existing_exact.contains(candidate.as_str()))
                .count();
            (baseline.clone(), stable_prefix)
        }
    };
    let existing_prefix_unchanged = baseline
        .iter()
        .take(stable_prefix)
        .eq(preview.iter().take(stable_prefix));
    let target_rank = candidate_rank(&preview, expected);
    let target_instances = preview
        .iter()
        .filter(|candidate| candidate.as_str() == expected)
        .count();
    Ok(ExactPhrasePreviewObservation {
        existing_exact_prefix: stable_prefix,
        target_rank,
        target_instances,
        guarded_rank_matches: target_rank == Some(stable_prefix + 1),
        existing_prefix_unchanged,
        preview_unique: preview.iter().collect::<HashSet<_>>().len() == preview.len(),
        preview_within_bound: preview.len() <= EXACT_PHRASE_RANK_DEPTH,
    })
}

#[cfg(windows)]
fn discover_exact_phrase_tsf_probes(
    core: &LoadedPackage,
    supplemental: &LoadedPackage,
    phrase: &LoadedPackage,
    catalog_codes_by_rank: &[Vec<String>; EXACT_PHRASE_FIRST_PAGE],
    sample_limit: usize,
) -> Result<(Vec<ExactPhraseTsfProbe>, usize), Box<dyn std::error::Error>> {
    let all_codes = catalog_codes_by_rank
        .iter()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let rank_counts = std::array::from_fn(|index| catalog_codes_by_rank[index].len());
    let plan = plan_exact_phrase_tsf_probe_sources(rank_counts, all_codes.len(), sample_limit)?;

    let expected_core_revision = core.snapshot.revision();
    let expected_supplemental_revision = supplemental.snapshot.revision();
    let expected_phrase_revision = phrase.snapshot.revision();
    let supplemental_config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let mut probes = Vec::<ExactPhraseTsfProbe>::with_capacity(sample_limit);
    let mut inspected = BTreeSet::<String>::new();
    for source in plan {
        let (candidate_codes, anchor, required_rank) = match source {
            ExactPhraseTsfProbeSource::RankBucket { rank_index, anchor } => (
                catalog_codes_by_rank[rank_index].as_slice(),
                anchor,
                Some(rank_index + 1),
            ),
            ExactPhraseTsfProbeSource::Catalog { anchor } => (all_codes.as_slice(), anchor, None),
        };
        let per_anchor_scan = candidate_codes
            .len()
            .min(EXACT_PHRASE_TSF_MAX_SCAN_PER_ANCHOR);
        let mut selected = None;
        for offset in 0..per_anchor_scan {
            let code_index = (anchor + offset) % candidate_codes.len();
            let code = &candidate_codes[code_index];
            if !inspected.insert(code.clone()) {
                continue;
            }
            let expected_text = phrase
                .snapshot
                .exact_full_code_texts(code, 1)?
                .into_iter()
                .next()
                .ok_or("exact phrase TSF sample is missing its authenticated identity")?;
            match preflight_exact_phrase_candidate_layers(
                Arc::clone(&core.snapshot),
                Arc::clone(&supplemental.snapshot),
                supplemental_config,
                Arc::clone(&phrase.snapshot),
                code,
                &expected_text,
            ) {
                Ok(report) => {
                    if report.core_revision() != expected_core_revision
                        || report.supplemental_revision() != expected_supplemental_revision
                        || report.phrase_revision() != expected_phrase_revision
                        || report.input_keys() != EXACT_PHRASE_CHARACTERS * 2
                        || report.committed_characters() != EXACT_PHRASE_CHARACTERS
                        || !(1..=EXACT_PHRASE_FIRST_PAGE).contains(&report.target_rank())
                    {
                        return Err("exact phrase TSF preflight runtime identity changed".into());
                    }
                    if required_rank.is_some_and(|rank| rank != report.target_rank()) {
                        return Err(
                            "exact phrase TSF preflight disagrees with the authenticated rank bucket"
                                .into(),
                        );
                    }
                    selected = Some(ExactPhraseTsfProbe {
                        code: code.clone(),
                        expected_text,
                        target_rank: report.target_rank(),
                        discovery_first_page: report.first_page_duration(),
                        discovery_commit: report.commit_duration(),
                    });
                }
                Err(TsfCandidatePreflightError::ExactPhrasePageMismatch) => {}
                Err(error) => return Err(error.into()),
            }
            if selected.is_some() {
                break;
            }
        }
        probes.push(selected.ok_or(
            "exact phrase TSF preflight found too few rank-stratified page-stable probes within the bounded scan",
        )?);
    }
    Ok((probes, inspected.len()))
}

#[cfg(windows)]
fn preflight_exact_phrase_tsf(
    request: ExactPhraseTsfPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    run_exact_phrase_tsf_preflight(request)
        .map(|summary| render_exact_phrase_tsf_preflight_report(&summary))
}

#[cfg(windows)]
fn run_exact_phrase_tsf_preflight(
    request: ExactPhraseTsfPreflightRequest<'_>,
) -> Result<ExactPhraseTsfPreflightSummary, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-phrase-tsf-preflight must run from a release build".into());
    }
    if !(1..=32).contains(&request.sample_limit)
        || !(1..=20).contains(&request.repetitions)
        || request.sample_limit.saturating_mul(request.repetitions) > 640
    {
        return Err("exact phrase TSF preflight workload is outside the fixed bound".into());
    }

    let core_started = Instant::now();
    let core = load_public_package_directory(request.core_package)?;
    let core_load = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental = load_public_package_directory(request.supplemental_package)?;
    let supplemental_load = supplemental_started.elapsed();
    let phrase_started = Instant::now();
    let phrase = load_public_package_directory(request.phrase_package)?;
    let phrase_load = phrase_started.elapsed();

    // Authenticate the complete four-source provenance, strict three-character
    // shape, unique-code catalog, guarded first-page merge, deduplication, and
    // negative-control fallback before opening any synthetic TSF Context.
    let layer_summary = preflight_loaded_exact_phrase_layer(
        &core,
        &supplemental,
        &phrase,
        request.sample_limit,
        1,
        [core_load, supplemental_load, phrase_load],
    )?;
    let (probes, inspected_codes) = discover_exact_phrase_tsf_probes(
        &core,
        &supplemental,
        &phrase,
        &layer_summary.catalog_codes_by_rank,
        request.sample_limit,
    )?;
    let expected_core_revision = core.snapshot.revision();
    let expected_supplemental_revision = supplemental.snapshot.revision();
    let expected_phrase_revision = phrase.snapshot.revision();
    let supplemental_config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };

    let sample_count = probes.len().saturating_mul(request.repetitions);
    let mut first_page_samples = Vec::with_capacity(sample_count);
    let mut commit_samples = Vec::with_capacity(sample_count);
    let mut complete_path_samples = Vec::with_capacity(sample_count);
    let mut target_rank_samples = [0_usize; EXACT_PHRASE_FIRST_PAGE];
    for _ in 0..request.repetitions {
        for probe in &probes {
            let report = preflight_exact_phrase_candidate_layers(
                Arc::clone(&core.snapshot),
                Arc::clone(&supplemental.snapshot),
                supplemental_config,
                Arc::clone(&phrase.snapshot),
                &probe.code,
                &probe.expected_text,
            )?;
            if report.core_revision() != expected_core_revision
                || report.supplemental_revision() != expected_supplemental_revision
                || report.phrase_revision() != expected_phrase_revision
                || report.target_rank() != probe.target_rank
            {
                return Err("exact phrase TSF preflight probe changed after discovery".into());
            }
            let target_index = report.target_rank() - 1;
            target_rank_samples[target_index] += 1;
            first_page_samples.push(report.first_page_duration());
            commit_samples.push(report.commit_duration());
            complete_path_samples.push(report.first_page_duration() + report.commit_duration());
        }
    }

    Ok(ExactPhraseTsfPreflightSummary {
        core: ExactPhraseTsfLayerIdentity {
            revision: core.snapshot.revision().to_owned(),
            authentication_sha256: core.authentication_sha256,
            load_duration: core_load,
        },
        supplemental: ExactPhraseTsfLayerIdentity {
            revision: supplemental.snapshot.revision().to_owned(),
            authentication_sha256: supplemental.authentication_sha256,
            load_duration: supplemental_load,
        },
        phrase: ExactPhraseTsfLayerIdentity {
            revision: phrase.snapshot.revision().to_owned(),
            authentication_sha256: phrase.authentication_sha256,
            load_duration: phrase_load,
        },
        phrase_entries: layer_summary.phrase_entries,
        phrase_codes: layer_summary.phrase_codes,
        requested_probes: probes.len(),
        inspected_codes,
        repetitions: request.repetitions,
        target_rank_samples,
        first_page: summarize_durations(&mut first_page_samples)
            .ok_or("exact phrase TSF preflight produced no first-page samples")?,
        commit: summarize_durations(&mut commit_samples)
            .ok_or("exact phrase TSF preflight produced no commit samples")?,
        complete_path: summarize_durations(&mut complete_path_samples)
            .ok_or("exact phrase TSF preflight produced no complete-path samples")?,
    })
}

#[cfg(not(windows))]
fn preflight_exact_phrase_tsf(
    _request: ExactPhraseTsfPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    Err("exact phrase TSF preflight requires Windows".into())
}

fn render_exact_phrase_tsf_preflight_report(summary: &ExactPhraseTsfPreflightSummary) -> String {
    let mut output = String::new();
    writeln!(output, "TSF 三字精确词第一页 release 预检").unwrap();
    for (label, identity) in [
        ("核心", &summary.core),
        ("补充", &summary.supplemental),
        ("三字层", &summary.phrase),
    ] {
        writeln!(
            output,
            "{label}：{} · 认证 SHA-256 {} · 载入 {:.3} ms",
            identity.revision,
            identity.authentication_sha256,
            duration_ms(identity.load_duration),
        )
        .unwrap();
    }
    writeln!(
        output,
        "形状门：{} 条、{} 个唯一六键码；四源 provenance、当前核心/补充载荷、既有整词前缀、第一页和负对照均先通过纯三包认证",
        summary.phrase_entries, summary.phrase_codes,
    )
    .unwrap();
    writeln!(
        output,
        "工作负载：位次分层探针 {} 个；每探针最多检查 64 个码；实际检查 {} 个码；每码发现预热 1 次；重复 {}；计时样本 {}；页宽 {}",
        summary.requested_probes,
        summary.inspected_codes,
        summary.repetitions,
        summary.first_page.samples,
        EXACT_PHRASE_FIRST_PAGE,
    )
    .unwrap();
    writeln!(
        output,
        "目标位次样本：第 1 项 {}；第 2 项 {}；第 3 项 {}；第 4 项 {}；第 5 项 {}；第 6 项 {}",
        summary.target_rank_samples[0],
        summary.target_rank_samples[1],
        summary.target_rank_samples[2],
        summary.target_rank_samples[3],
        summary.target_rank_samples[4],
        summary.target_rank_samples[5],
    )
    .unwrap();
    write_tsf_preflight_duration(&mut output, "输入至第一页状态就绪", summary.first_page);
    write_tsf_preflight_duration(&mut output, "空格或数字键提交目标", summary.commit);
    write_tsf_preflight_duration(&mut output, "首键至提交完成", summary.complete_path);
    writeln!(
        output,
        "功能门：全部计时样本均保持既有完整词前缀，在第一页以 PublicConsensusExact 来源命中目标，并由空格或普通数字键实际提交"
    )
    .unwrap();
    writeln!(
        output,
        "性能口径：同机、release、每次新建真实系统 TSF 合成 Context；计时截止同步候选状态与提交，不包含候选窗绘制、桌面合成、屏幕首帧或实际宿主呈现。"
    )
    .unwrap();
    writeln!(
        output,
        "隐私与启用边界：报告不含探针码、候选正文或个人数据；本次操作只读，不写槽位、状态或运行时凭据，不安装、不启用，预检通过也不构成换代许可。"
    )
    .unwrap();
    output
}

#[cfg(windows)]
fn preflight_exact_phrase_popup(
    request: ExactPhrasePopupPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("exact-phrase-popup-preflight must run from a release build".into());
    }
    if !(1..=4).contains(&request.sample_limit)
        || !(1..=5).contains(&request.repetitions)
        || request
            .sample_limit
            .saturating_mul(request.repetitions)
            .saturating_mul(4)
            > 80
    {
        return Err("exact phrase popup preflight workload is outside the fixed bound".into());
    }

    let core_started = Instant::now();
    let core = load_public_package_directory(request.core_package)?;
    let core_load = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental = load_public_package_directory(request.supplemental_package)?;
    let supplemental_load = supplemental_started.elapsed();
    let phrase_started = Instant::now();
    let phrase = load_public_package_directory(request.phrase_package)?;
    let phrase_load = phrase_started.elapsed();
    let layer_summary = preflight_loaded_exact_phrase_layer(
        &core,
        &supplemental,
        &phrase,
        request.sample_limit,
        1,
        [core_load, supplemental_load, phrase_load],
    )?;
    let (probes, inspected_codes) = discover_exact_phrase_tsf_probes(
        &core,
        &supplemental,
        &phrase,
        &layer_summary.catalog_codes_by_rank,
        request.sample_limit,
    )?;

    let expected_core_revision = core.snapshot.revision();
    let expected_supplemental_revision = supplemental.snapshot.revision();
    let expected_phrase_revision = phrase.snapshot.revision();
    let supplemental_config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let expected_popup_samples = probes
        .len()
        .saturating_mul(request.repetitions)
        .saturating_mul(4);
    let mut popup_samples = Vec::with_capacity(expected_popup_samples);
    let mut hide_durations = Vec::with_capacity(expected_popup_samples);
    let mut destroy_durations = Vec::with_capacity(expected_popup_samples);
    let mut target_rank_probes = [0_usize; EXACT_PHRASE_FIRST_PAGE];
    let mut tsf_first_page = Vec::with_capacity(probes.len());
    let mut tsf_commit = Vec::with_capacity(probes.len());
    for probe in &probes {
        target_rank_probes[probe.target_rank - 1] += 1;
        tsf_first_page.push(probe.discovery_first_page);
        tsf_commit.push(probe.discovery_commit);
        let report = preflight_exact_phrase_candidate_popup_rendering(
            Arc::clone(&core.snapshot),
            Arc::clone(&supplemental.snapshot),
            supplemental_config,
            Arc::clone(&phrase.snapshot),
            &probe.code,
            &probe.expected_text,
            request.repetitions,
        )?;
        validate_exact_phrase_popup_report(
            &report,
            expected_core_revision,
            expected_supplemental_revision,
            expected_phrase_revision,
            probe.target_rank,
            request.repetitions,
        )?;
        popup_samples.extend(report.rendering().samples().iter().copied());
        hide_durations.extend(report.rendering().hide_durations().iter().copied());
        destroy_durations.extend(report.rendering().destroy_durations().iter().copied());
    }
    if popup_samples.len() != expected_popup_samples
        || hide_durations.len() != expected_popup_samples
        || destroy_durations.len() != expected_popup_samples
    {
        return Err("exact phrase popup preflight returned an incomplete fixed workload".into());
    }

    render_exact_phrase_popup_preflight_report(&ExactPhrasePopupPreflightSummary {
        core: ExactPhraseTsfLayerIdentity {
            revision: core.snapshot.revision().to_owned(),
            authentication_sha256: core.authentication_sha256,
            load_duration: core_load,
        },
        supplemental: ExactPhraseTsfLayerIdentity {
            revision: supplemental.snapshot.revision().to_owned(),
            authentication_sha256: supplemental.authentication_sha256,
            load_duration: supplemental_load,
        },
        phrase: ExactPhraseTsfLayerIdentity {
            revision: phrase.snapshot.revision().to_owned(),
            authentication_sha256: phrase.authentication_sha256,
            load_duration: phrase_load,
        },
        phrase_entries: layer_summary.phrase_entries,
        phrase_codes: layer_summary.phrase_codes,
        requested_probes: probes.len(),
        inspected_codes,
        repetitions: request.repetitions,
        target_rank_probes,
        tsf_first_page: summarize_durations(&mut tsf_first_page)
            .ok_or("exact phrase popup preflight produced no TSF page samples")?,
        tsf_commit: summarize_durations(&mut tsf_commit)
            .ok_or("exact phrase popup preflight produced no TSF commit samples")?,
        popup_samples,
        hide_durations,
        destroy_durations,
    })
}

#[cfg(windows)]
fn validate_exact_phrase_popup_report(
    report: &ExactPhrasePopupRenderPreflightReport,
    core_revision: &str,
    supplemental_revision: &str,
    phrase_revision: &str,
    target_rank: usize,
    repetitions: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let rendering = report.rendering();
    if report.core_revision() != core_revision
        || report.supplemental_revision() != supplemental_revision
        || report.phrase_revision() != phrase_revision
        || report.input_keys() != EXACT_PHRASE_CHARACTERS * 2
        || report.committed_characters() != EXACT_PHRASE_CHARACTERS
        || report.target_rank() != target_rank
        || rendering.repetitions() != repetitions
        || rendering.samples().len() != repetitions.saturating_mul(4)
        || rendering.hide_durations().len() != repetitions.saturating_mul(4)
        || rendering.destroy_durations().len() != repetitions.saturating_mul(4)
        || rendering.samples().iter().any(|sample| {
            sample.scenario() != CandidatePopupRenderScenario::InitialShow
                || sample.effective_dpi() != sample.requested_dpi()
        })
    {
        return Err("exact phrase popup preflight runtime identity changed".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn preflight_exact_phrase_popup(
    _request: ExactPhrasePopupPreflightRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    Err("exact phrase popup preflight requires Windows".into())
}

#[cfg(windows)]
fn render_exact_phrase_popup_preflight_report(
    summary: &ExactPhrasePopupPreflightSummary,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut grouped = BTreeMap::<u32, Vec<&CandidatePopupRenderSample>>::new();
    for sample in &summary.popup_samples {
        if sample.scenario() != CandidatePopupRenderScenario::InitialShow
            || sample.effective_dpi() != sample.requested_dpi()
        {
            return Err(
                "exact phrase popup preflight observed an unexpected rendering path".into(),
            );
        }
        grouped
            .entry(sample.requested_dpi())
            .or_default()
            .push(sample);
    }
    let expected_per_dpi = summary.requested_probes.saturating_mul(summary.repetitions);
    if grouped.len() != 4
        || grouped
            .values()
            .any(|samples| samples.len() != expected_per_dpi)
        || summary.hide_durations.len() != summary.popup_samples.len()
        || summary.destroy_durations.len() != summary.popup_samples.len()
        || summary.target_rank_probes.iter().sum::<usize>() != summary.requested_probes
    {
        return Err("exact phrase popup preflight report is incomplete".into());
    }

    let mut output = String::new();
    writeln!(output, "TSF 三字精确词生产候选窗 release 可见预检")?;
    for (label, identity) in [
        ("核心", &summary.core),
        ("补充", &summary.supplemental),
        ("三字层", &summary.phrase),
    ] {
        writeln!(
            output,
            "{label}：{} · 认证 SHA-256 {} · 载入 {:.3} ms",
            identity.revision,
            identity.authentication_sha256,
            duration_ms(identity.load_duration),
        )?;
    }
    writeln!(
        output,
        "形状门：{} 条、{} 个唯一六键码；真实 TSF 稳定探针 {} 个；有界检查 {} 个码",
        summary.phrase_entries,
        summary.phrase_codes,
        summary.requested_probes,
        summary.inspected_codes,
    )?;
    writeln!(
        output,
        "目标页位次：第 1 项 {}；第 2 项 {}；第 3 项 {}；第 4 项 {}；第 5 项 {}；第 6 项 {}",
        summary.target_rank_probes[0],
        summary.target_rank_probes[1],
        summary.target_rank_probes[2],
        summary.target_rank_probes[3],
        summary.target_rank_probes[4],
        summary.target_rank_probes[5],
    )?;
    write_tsf_preflight_duration(&mut output, "探针输入至第一页状态", summary.tsf_first_page);
    write_tsf_preflight_duration(&mut output, "探针提交", summary.tsf_commit);
    writeln!(
        output,
        "可见工作负载：{} 个页面 × 4 档 DPI × 重复 {}；首次创建样本 {}；每个样本均会短暂显示一个 ownerless、nonactivating 窗口",
        summary.requested_probes,
        summary.repetitions,
        summary.popup_samples.len(),
    )?;
    for (dpi, samples) in grouped {
        let window_ready =
            summarize_popup_render_stage(&samples, |sample| sample.window_ready_duration())?;
        let paint_entered =
            summarize_popup_render_stage(&samples, |sample| sample.paint_entered_duration())?;
        let drawing =
            summarize_popup_render_stage(&samples, |sample| sample.drawing_completed_duration())?;
        let published =
            summarize_popup_render_stage(&samples, |sample| sample.frame_published_duration())?;
        let completed =
            summarize_popup_render_stage(&samples, |sample| sample.paint_completed_duration())?;
        let buffered = samples.iter().filter(|sample| sample.buffered()).count();
        writeln!(
            output,
            "生产三字第一页 · {dpi} DPI · {} 样本 · 双缓冲 {buffered}/{}",
            samples.len(),
            samples.len(),
        )?;
        write_popup_render_stage(&mut output, "  窗口就绪", window_ready)?;
        write_popup_render_stage(&mut output, "  进入 WM_PAINT", paint_entered)?;
        write_popup_render_stage(&mut output, "  GDI 绘制完成", drawing)?;
        write_popup_render_stage(&mut output, "  完整帧发布", published)?;
        write_popup_render_stage(&mut output, "  EndPaint 完成", completed)?;
        let mut flush_calls = samples
            .iter()
            .filter_map(|sample| sample.compositor_flush_duration())
            .collect::<Vec<_>>();
        let mut request_to_flush = samples
            .iter()
            .filter_map(|sample| sample.request_to_compositor_flush_duration())
            .collect::<Vec<_>>();
        match (
            summarize_durations(&mut flush_calls),
            summarize_durations(&mut request_to_flush),
        ) {
            (Some(flush), Some(total)) => writeln!(
                output,
                "  DwmFlush：{}/{} 成功；调用 p95 {:.3} ms；请求至返回 p95 {:.3} ms",
                flush.samples,
                samples.len(),
                duration_ms(flush.p95),
                duration_ms(total.p95),
            )?,
            _ => writeln!(output, "  DwmFlush：当前桌面合成环境不可观测")?,
        }
    }
    let mut hide = summary.hide_durations.clone();
    let mut destroy = summary.destroy_durations.clone();
    write_popup_render_stage(
        &mut output,
        "隐藏并确认不可见",
        summarize_durations(&mut hide).ok_or("exact phrase popup preflight has no hide samples")?,
    )?;
    write_popup_render_stage(
        &mut output,
        "销毁并确认 HWND 失效",
        summarize_durations(&mut destroy)
            .ok_or("exact phrase popup preflight has no destroy samples")?,
    )?;
    writeln!(
        output,
        "功能门：同一认证三包先经真实 TSF Context 提交，再重建同一探针的第一页并用生产候选窗绘制；目标仍带 PublicConsensusExact 来源，全部帧完成双缓冲发布、隐藏和销毁。"
    )?;
    writeln!(
        output,
        "边界：Context 与 popup 是串行的两条生产组件路径，不是已安装编辑器的一次真实回调；DwmFlush 只同步桌面合成队列，不证明显示器已扫描或真人已看见。"
    )?;
    writeln!(
        output,
        "隐私与启用边界：报告不含探针码、候选正文或个人数据；不写槽位、状态或运行时凭据，不安装、不启用，预检通过也不构成换代许可。"
    )?;
    Ok(output)
}

fn existing_exact_texts(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    Ok(core
        .exact_full_code_texts(code, limit)?
        .into_iter()
        .chain(supplemental.exact_full_code_texts(code, limit)?)
        .collect())
}

fn select_exact_phrase_source_entries(
    source_text: &str,
    fit_probes: &[PublicLexiconRankProbe],
    existing_entries: &[LexiconEntry],
    entry_limit: usize,
) -> Result<ExactPhraseSourceSelection, Box<dyn std::error::Error>> {
    let fit_instances = fit_probes
        .iter()
        .map(|probe| {
            (
                (
                    probe.expected_text.clone(),
                    probe.observed.as_str().to_owned(),
                ),
                probe.instances,
            )
        })
        .collect::<HashMap<_, _>>();
    let requested_surfaces = fit_probes
        .iter()
        .map(|probe| probe.expected_text.as_str())
        .collect::<HashSet<_>>();
    let requested_codes = fit_probes
        .iter()
        .map(|probe| probe.observed.as_str())
        .collect::<HashSet<_>>();
    let mut stats = ExactPhraseSourceSelectionStats {
        requested_surfaces: requested_surfaces.len(),
        requested_instances: fit_probes.iter().map(|probe| probe.instances).sum(),
        ..ExactPhraseSourceSelectionStats::default()
    };
    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut valid_codes_by_surface = HashMap::<String, HashSet<String>>::new();
    let mut valid_surfaces_by_code = HashMap::<String, HashSet<String>>::new();
    let mut best_by_identity = HashMap::<(String, String), RankedExactPhraseEntry>::new();
    for (zero_based_line, raw_line) in source_text.lines().enumerate() {
        let source_line = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if !saw_data_marker {
            match line {
                "---" => saw_document_start = true,
                "..." if saw_document_start => saw_data_marker = true,
                _ => {}
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        stats.source_rows += 1;
        let mut fields = line.split('\t');
        let (Some(text), Some(toned_pinyin), Some(weight), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let requested_surface = requested_surfaces.contains(text);
        stats.requested_source_rows += usize::from(requested_surface);
        if text.chars().count() != EXACT_PHRASE_CHARACTERS
            || !text.chars().all(is_han_phrase_character)
        {
            continue;
        }
        let Some(weight) = weight.parse::<u64>().ok().filter(|weight| *weight != 0) else {
            continue;
        };
        let Some((pinyin, encoded)) = normalize_pinyin_tone_marks(toned_pinyin)
            .ok()
            .and_then(|pinyin| {
                encode_pinyin_phrase(&pinyin)
                    .ok()
                    .map(|encoded| (pinyin, encoded))
            })
            .filter(|(_, encoded)| encoded.syllable_codes.len() == EXACT_PHRASE_CHARACTERS)
        else {
            continue;
        };
        let code = encoded.full_code.as_str().to_owned();
        if requested_codes.contains(code.as_str()) {
            valid_surfaces_by_code
                .entry(code.clone())
                .or_default()
                .insert(text.to_owned());
        }
        if !requested_surface {
            continue;
        }
        stats.valid_requested_source_rows += 1;
        valid_codes_by_surface
            .entry(text.to_owned())
            .or_default()
            .insert(code.clone());
        let identity = (text.to_owned(), code);
        let Some(&fit_instances) = fit_instances.get(&identity) else {
            continue;
        };
        let ranked = RankedExactPhraseEntry {
            entry: LexiconEntry {
                text: text.to_owned(),
                pinyin,
                code: encoded.full_code,
                syllable_codes: encoded.syllable_codes,
                frequency: weight,
            },
            source_line,
            fit_instances,
        };
        match best_by_identity.entry(identity) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ranked);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if ranked.precedes(slot.get()) {
                    slot.insert(ranked);
                }
            }
        }
    }
    if !saw_document_start {
        return Err("exact-phrase Rime source is missing the YAML document start".into());
    }
    if !saw_data_marker {
        return Err("exact-phrase Rime source is missing the YAML data marker".into());
    }
    stats.matched_identities = best_by_identity.len();
    let ambiguous_surfaces = valid_codes_by_surface
        .iter()
        .filter(|(_, codes)| codes.len() > 1)
        .map(|(surface, _)| surface.as_str())
        .collect::<HashSet<_>>();
    stats.ambiguous_surfaces = ambiguous_surfaces.len();
    let ambiguous_codes = valid_surfaces_by_code
        .iter()
        .filter(|(_, surfaces)| surfaces.len() > 1)
        .map(|(code, _)| code.as_str())
        .collect::<HashSet<_>>();
    stats.ambiguous_codes = ambiguous_codes.len();
    let existing_identities = existing_entries
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let mut best_by_code = HashMap::<String, RankedExactPhraseEntry>::new();
    for ((text, code), ranked) in best_by_identity {
        if ambiguous_surfaces.contains(text.as_str()) {
            continue;
        }
        if ambiguous_codes.contains(code.as_str()) {
            continue;
        }
        if existing_identities.contains(&(text.as_str(), code.as_str())) {
            stats.existing_identities += 1;
            continue;
        }
        match best_by_code.entry(code) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ranked);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                stats.code_collisions += 1;
                if ranked.precedes(slot.get()) {
                    slot.insert(ranked);
                }
            }
        }
    }
    let mut ranked = best_by_code.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        if left.precedes(right) {
            std::cmp::Ordering::Less
        } else if right.precedes(left) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    stats.dropped_by_entry_cap = ranked.len().saturating_sub(entry_limit);
    ranked.truncate(entry_limit);
    let entries = ranked
        .into_iter()
        .map(|ranked| ranked.entry)
        .collect::<Vec<_>>();
    stats.selected_entries = entries.len();
    Ok(ExactPhraseSourceSelection { entries, stats })
}

fn is_han_phrase_character(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{3007}'
    )
}

fn preview_exact_phrase_candidates(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    phrase: &CandidateSnapshot,
    code: &str,
    limit: usize,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
    let baseline = layered_candidate_texts(
        core,
        supplemental,
        code,
        limit,
        SupplementalCandidateLayerConfig {
            exact_promotions: 1,
        },
    )?;
    let phrase_exact = phrase.exact_full_code_texts(code, 1)?;
    let Some(candidate) = phrase_exact.first() else {
        return Ok(baseline);
    };
    let core_exact = core.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
    let supplemental_exact =
        supplemental.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
    let existing_exact = core_exact
        .into_iter()
        .chain(supplemental_exact)
        .collect::<HashSet<_>>();
    Ok(merge_exact_phrase_candidate_into_baseline(&baseline, &existing_exact, candidate, limit).0)
}

fn merge_exact_phrase_candidate_into_baseline(
    baseline: &[String],
    existing_exact: &HashSet<String>,
    candidate: &str,
    limit: usize,
) -> (Vec<String>, usize) {
    let stable_prefix = baseline
        .iter()
        .take_while(|item| existing_exact.contains(item.as_str()))
        .count();
    let mut preview = Vec::with_capacity(limit);
    let mut seen = HashSet::<String>::new();
    for item in baseline.iter().take(stable_prefix) {
        push_unique_candidate(&mut preview, &mut seen, item, limit);
    }
    push_unique_candidate(&mut preview, &mut seen, candidate, limit);
    for item in baseline.iter().skip(stable_prefix) {
        push_unique_candidate(&mut preview, &mut seen, item, limit);
    }
    (preview, stable_prefix)
}

fn push_unique_candidate(
    output: &mut Vec<String>,
    seen: &mut HashSet<String>,
    candidate: &str,
    limit: usize,
) -> bool {
    if output.len() == limit || !seen.insert(candidate.to_owned()) {
        return false;
    }
    output.push(candidate.to_owned());
    true
}

fn compare_exact_phrase_ranks(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    phrase: &CandidateSnapshot,
    probes: &[PublicLexiconRankProbe],
    limit: usize,
) -> Result<ExactPhraseRankComparison, Box<dyn std::error::Error>> {
    let mut report = ExactPhraseRankComparison::default();
    for probe in probes {
        let baseline = layered_candidate_texts(
            core,
            supplemental,
            probe.observed.as_str(),
            limit,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )?;
        let preview = preview_exact_phrase_candidates(
            core,
            supplemental,
            phrase,
            probe.observed.as_str(),
            limit,
        )?;
        report.observe(&baseline, &preview, &probe.expected_text, probe.instances);
    }
    Ok(report)
}

fn existing_exact_identity(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    probe: &PublicLexiconRankProbe,
) -> Result<bool, Box<dyn std::error::Error>> {
    let code = probe.observed.as_str();
    let expected = probe.expected_text.as_str();
    Ok(core
        .exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?
        .iter()
        .chain(
            supplemental
                .exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?
                .iter(),
        )
        .any(|candidate| candidate == expected))
}

fn write_exact_phrase_span_selection(
    output: &mut String,
    label: &str,
    corpus_sha256: &str,
    selection: &ziranma_core::PublicHanSpanRankSelection,
) {
    writeln!(
        output,
        "{label}：候选 span {}；三字汉字 span {}；可编码 {}（单 token {}，跨 token {}）；独立文字/码身份 {} · SHA-256 {corpus_sha256}",
        selection.source_spans,
        selection.han_length_eligible,
        selection.code_coverable_instances,
        selection.one_token_instances,
        selection.multi_token_instances,
        selection.code_coverable_identities,
    )
    .unwrap();
}

fn write_exact_phrase_rank_report(
    output: &mut String,
    label: &str,
    report: ExactPhraseRankComparison,
) {
    writeln!(
        output,
        "{label}：{} 个身份（实例 {}）；顺序变化 {}（实例 {}），首选变化 {}（实例 {}）",
        report.probes,
        report.instances,
        report.order_changes,
        report.order_change_instances,
        report.top_changes,
        report.top_change_instances,
    )
    .unwrap();
    write_exact_phrase_rank_counts(output, "  基线", report.baseline);
    write_exact_phrase_rank_counts(output, "  三字层", report.preview);
    writeln!(
        output,
        "  位次：改善 {}（实例 {}），不变 {}（实例 {}），变差 {}（实例 {}），新进 Top-10 {}（实例 {}），掉出 Top-10 {}（实例 {}）",
        report.movement.improved,
        report.movement.improved_instances,
        report.movement.unchanged,
        report.movement.unchanged_instances,
        report.movement.worsened,
        report.movement.worsened_instances,
        report.movement.newly_visible,
        report.movement.newly_visible_instances,
        report.movement.lost_visible,
        report.movement.lost_visible_instances,
    )
    .unwrap();
    writeln!(
        output,
        "  首选正确性：新增 {}（实例 {}），丢失 {}（实例 {}），非目标变化 {}（实例 {}）",
        report.correct_top_gains,
        report.correct_top_gain_instances,
        report.correct_top_losses,
        report.correct_top_loss_instances,
        report.non_target_top_changes,
        report.non_target_top_change_instances,
    )
    .unwrap();
    writeln!(
        output,
        "  分页保护：首屏掉出 {}（实例 {}），跨页变差 {}（实例 {}）",
        report.first_page_degradations,
        report.first_page_degradation_instances,
        report.cross_page_degradations,
        report.cross_page_degradation_instances,
    )
    .unwrap();
}

fn write_exact_phrase_rank_counts(output: &mut String, label: &str, counts: ExactPhraseRankCounts) {
    writeln!(
        output,
        "{label}：Top-1 {}/{}，Top-6 {}/{}，Top-10 {}/{}（身份/实例）",
        counts.at_one,
        counts.at_one_instances,
        counts.at_six,
        counts.at_six_instances,
        counts.at_ten,
        counts.at_ten_instances,
    )
    .unwrap();
}

fn write_exact_phrase_public_counterexamples(
    output: &mut String,
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    phrase: &CandidateSnapshot,
    controls: &[PublicLexiconRankProbe],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut examples = Vec::new();
    for probe in controls {
        let baseline = layered_candidate_texts(
            core,
            supplemental,
            probe.observed.as_str(),
            EXACT_PHRASE_RANK_DEPTH,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )?;
        let preview = preview_exact_phrase_candidates(
            core,
            supplemental,
            phrase,
            probe.observed.as_str(),
            EXACT_PHRASE_RANK_DEPTH,
        )?;
        let before_rank = candidate_rank(&baseline, &probe.expected_text);
        let after_rank = candidate_rank(&preview, &probe.expected_text);
        let worsened = matches!((before_rank, after_rank), (Some(before), Some(after)) if after > before)
            || matches!((before_rank, after_rank), (Some(_), None));
        if baseline.first() != preview.first() || worsened {
            examples.push((
                probe.observed.as_str().to_owned(),
                probe.expected_text.clone(),
                baseline.first().cloned().unwrap_or_else(|| "∅".to_owned()),
                preview.first().cloned().unwrap_or_else(|| "∅".to_owned()),
                before_rank,
                after_rank,
            ));
        }
    }
    examples.sort_unstable();
    if examples.is_empty() {
        writeln!(output, "公开同码反例：无")?;
        return Ok(());
    }
    writeln!(
        output,
        "公开同码反例（最多 8 个；均来自显式留出语料，不含私人数据）："
    )?;
    for (code, expected, before_top, after_top, before_rank, after_rank) in
        examples.into_iter().take(8)
    {
        writeln!(
            output,
            "  {code} → {expected}：首选 {before_top} → {after_top}；目标位次 {} → {}",
            render_optional_rank(before_rank),
            render_optional_rank(after_rank),
        )?;
    }
    Ok(())
}

fn render_optional_rank(rank: Option<usize>) -> String {
    rank.map_or_else(|| ">10".to_owned(), |rank| rank.to_string())
}

fn compare_phrase_layer_ranks(
    core: &CandidateSnapshot,
    small: &CandidateSnapshot,
    large: &CandidateSnapshot,
    probes: &[PublicLexiconRankProbe],
) -> Result<PhraseRankComparison, Box<dyn std::error::Error>> {
    let config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let mut report = PhraseRankComparison::default();
    for probe in probes {
        report.probes += 1;
        report.instances += probe.instances;
        let code = probe.observed.as_str();
        let core_candidates = core.candidate_texts(code, PHRASE_LAYER_RANK_DEPTH)?;
        let small_candidates =
            layered_candidate_texts(core, small, code, PHRASE_LAYER_RANK_DEPTH, config)?;
        let large_candidates =
            layered_candidate_texts(core, large, code, PHRASE_LAYER_RANK_DEPTH, config)?;
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        let small_rank = candidate_rank(&small_candidates, &probe.expected_text);
        let large_rank = candidate_rank(&large_candidates, &probe.expected_text);
        report.core.observe(core_rank, probe.instances);
        report.small.observe(small_rank, probe.instances);
        report.large.observe(large_rank, probe.instances);
        report
            .small_vs_core
            .observe(core_rank, small_rank, probe.instances);
        report
            .large_vs_core
            .observe(core_rank, large_rank, probe.instances);
        report.small_order_changes += usize::from(core_candidates != small_candidates);
        report.large_order_changes += usize::from(core_candidates != large_candidates);
        report.small_top_changes +=
            usize::from(core_candidates.first() != small_candidates.first());
        report.large_top_changes +=
            usize::from(core_candidates.first() != large_candidates.first());
    }
    Ok(report)
}

fn bounded_public_rank_probes(
    mut probes: Vec<PublicLexiconRankProbe>,
) -> Vec<PublicLexiconRankProbe> {
    probes.sort_by(|left, right| {
        candidate_payload_fingerprint(left.expected_text.as_bytes())
            .cmp(&candidate_payload_fingerprint(
                right.expected_text.as_bytes(),
            ))
            .then_with(|| left.expected_text.cmp(&right.expected_text))
    });
    probes.truncate(PHRASE_LAYER_CONTROL_LIMIT);
    probes
}

fn serialize_lexicon_payload(entries: &[LexiconEntry]) -> String {
    let mut payload = String::from("text\tpinyin\tfrequency\n");
    for entry in entries {
        writeln!(
            payload,
            "{}\t{}\t{}",
            entry.text, entry.pinyin, entry.frequency
        )
        .expect("writing to a String cannot fail");
    }
    payload
}

fn write_phrase_rank_report(
    output: &mut String,
    label: &str,
    corpus_sha256: &str,
    report: PhraseRankComparison,
) {
    writeln!(
        output,
        "{label}：{} 个词面（实例 {}）· SHA-256 {corpus_sha256}",
        report.probes, report.instances,
    )
    .unwrap();
    write_weighted_rank_counts(output, "  基础", report.core);
    write_weighted_rank_counts(output, "  小层", report.small);
    write_weighted_rank_counts(output, "  大层", report.large);
    write_weighted_rank_movement(output, "  小层相对基础", report.small_vs_core);
    write_weighted_rank_movement(output, "  大层相对基础", report.large_vs_core);
}

fn write_phrase_control_report(
    output: &mut String,
    label: &str,
    matched_unique: usize,
    matched_instances: usize,
    report: PhraseRankComparison,
) {
    writeln!(
        output,
        "{label}：语料匹配 {} 个词面（实例 {}）；固定抽样 {}（实例 {}，上限 {PHRASE_LAYER_CONTROL_LIMIT}）",
        matched_unique, matched_instances, report.probes, report.instances,
    )
    .unwrap();
    writeln!(
        output,
        "  小层：顺序变化 {}，首选变化 {}，目标变差 {}（实例 {}），掉出 Top-10 {}（实例 {}）",
        report.small_order_changes,
        report.small_top_changes,
        report.small_vs_core.worsened,
        report.small_vs_core.worsened_instances,
        report.small_vs_core.lost_visible,
        report.small_vs_core.lost_visible_instances,
    )
    .unwrap();
    writeln!(
        output,
        "  大层：顺序变化 {}，首选变化 {}，目标变差 {}（实例 {}），掉出 Top-10 {}（实例 {}）",
        report.large_order_changes,
        report.large_top_changes,
        report.large_vs_core.worsened,
        report.large_vs_core.worsened_instances,
        report.large_vs_core.lost_visible,
        report.large_vs_core.lost_visible_instances,
    )
    .unwrap();
}

fn write_weighted_rank_counts(output: &mut String, label: &str, counts: WeightedRankCounts) {
    writeln!(
        output,
        "{label}：Top-1 {}/{}，Top-5 {}/{}，Top-10 {}/{}（词面/实例）",
        counts.at_one,
        counts.at_one_instances,
        counts.at_five,
        counts.at_five_instances,
        counts.at_ten,
        counts.at_ten_instances,
    )
    .unwrap();
}

fn write_weighted_rank_movement(output: &mut String, label: &str, movement: WeightedRankMovement) {
    writeln!(
        output,
        "{label}：改善 {}（实例 {}），不变 {}（实例 {}），变差 {}（实例 {}）；新进 Top-10 {}，掉出 {}",
        movement.improved,
        movement.improved_instances,
        movement.unchanged,
        movement.unchanged_instances,
        movement.worsened,
        movement.worsened_instances,
        movement.newly_visible,
        movement.lost_visible,
    )
    .unwrap();
}

fn write_phrase_index_report(
    output: &mut String,
    label: &str,
    snapshot: &CandidateSnapshot,
    stats: DecoderIndexStats,
    build: Duration,
) {
    writeln!(
        output,
        "索引 {label}：词条 {}，载荷 {} 字节，节点 {}，边 {}，终端 {}，最大同码扇出 {}，隐式拼写 {}；构建 {:.3} ms",
        snapshot.entry_count(),
        snapshot.payload_bytes(),
        stats.node_count,
        stats.edge_count,
        stats.terminal_count,
        stats.maximum_terminal_fanout,
        stats.represented_spelling_count,
        duration_ms(build),
    )
    .unwrap();
}

fn write_phrase_latency_report(output: &mut String, label: &str, summary: DurationSummary) {
    writeln!(
        output,
        "查询 {label}：median {:.3} ms；p95 {:.3} ms；max {:.3} ms",
        duration_ms(summary.median),
        duration_ms(summary.p95),
        duration_ms(summary.maximum),
    )
    .unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StaticContextProfile {
    search_depth: usize,
    minimum_average_gain: f64,
}

fn static_context_profiles() -> Vec<StaticContextProfile> {
    let mut profiles = vec![StaticContextProfile {
        search_depth: 1,
        minimum_average_gain: 0.0,
    }];
    for search_depth in [8, 16, 32, 50] {
        for minimum_average_gain in [0.10, 0.25, 0.50, 0.75, 1.00] {
            profiles.push(StaticContextProfile {
                search_depth,
                minimum_average_gain,
            });
        }
    }
    profiles
}

fn single_character_context_profiles() -> Vec<StaticContextProfile> {
    let mut profiles = static_context_profiles();
    for search_depth in [8, 16, 32, 50] {
        for minimum_average_gain in [1.25, 1.50, 2.00, 3.00, 4.00] {
            profiles.push(StaticContextProfile {
                search_depth,
                minimum_average_gain,
            });
        }
    }
    profiles
}

#[derive(Clone, Debug)]
struct FrozenStaticContextCandidate {
    text: String,
    segments: Vec<String>,
}

#[derive(Clone, Debug)]
struct FrozenStaticContextCase {
    expected_text: String,
    candidates: Vec<FrozenStaticContextCandidate>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StaticContextSelectionStats {
    source_representatives: usize,
    whole_text_collisions: usize,
    whole_code_collisions: usize,
    selected: usize,
    empty_frontiers: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SingleCharacterContextSelectionStats {
    source_windows: usize,
    single_character_targets: usize,
    exact_word_coverable: usize,
    source_representatives: usize,
    ambiguous_target_surfaces: usize,
    non_two_key_targets: usize,
    uncompetitive_exact_pools: usize,
    target_outside_frontier: usize,
    selected: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StaticContextRankCounts {
    at_one: usize,
    at_three: usize,
    at_five: usize,
    visible: usize,
}

impl StaticContextRankCounts {
    fn observe(&mut self, rank: Option<usize>) {
        let Some(rank) = rank else {
            return;
        };
        self.visible += 1;
        self.at_one += usize::from(rank <= 1);
        self.at_three += usize::from(rank <= 3);
        self.at_five += usize::from(rank <= 5);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StaticContextProfileReport {
    total: usize,
    baseline: StaticContextRankCounts,
    candidate: StaticContextRankCounts,
    rank_improved: usize,
    rank_unchanged: usize,
    rank_worsened: usize,
    correct_top_one_gained: usize,
    correct_top_one_lost: usize,
    non_target_top_one_changes: usize,
    any_top_one_changes: usize,
}

#[derive(Clone, Copy, Debug)]
struct SparseArpaRecord {
    probability_log10: f64,
    backoff_log10: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedArpaNgram {
    probability_log10: f64,
    tokens: Vec<String>,
    backoff_log10: Option<f64>,
}

#[derive(Clone, Debug)]
struct SparseArpaLanguageModel {
    declared_order: usize,
    effective_order: usize,
    sentence_boundaries: bool,
    bytes: u64,
    sha256: String,
    known_query_tokens: HashSet<String>,
    unknown_query_tokens: usize,
    required_ngrams: usize,
    records: HashMap<Vec<String>, SparseArpaRecord>,
}

impl SparseArpaLanguageModel {
    fn canonical_token(&self, token: &str) -> String {
        if self.known_query_tokens.contains(token) {
            token.to_owned()
        } else {
            "<unk>".to_owned()
        }
    }

    fn score_candidate(
        &self,
        candidate: &FrozenStaticContextCandidate,
    ) -> Result<f64, Box<dyn std::error::Error>> {
        let tokens = candidate
            .segments
            .iter()
            .map(|token| self.canonical_token(token))
            .collect::<Vec<_>>();
        let mut history = Vec::new();
        if self.sentence_boundaries {
            history.push("<s>".to_owned());
        }
        let mut total_log10 = 0.0;
        for token in &tokens {
            total_log10 += self.score_word(&history, token)?;
            history.push(token.to_owned());
            let maximum_history = self.effective_order.saturating_sub(1);
            if history.len() > maximum_history {
                let remove = history.len() - maximum_history;
                history.drain(..remove);
            }
        }
        if self.sentence_boundaries {
            total_log10 += self.score_word(&history, "</s>")?;
        }
        let characters = candidate.text.chars().count().max(1);
        Ok(total_log10 / characters as f64)
    }

    fn score_word(
        &self,
        history: &[String],
        word: &str,
    ) -> Result<f64, Box<dyn std::error::Error>> {
        let maximum_history = history.len().min(self.effective_order.saturating_sub(1));
        let mut accumulated_backoff = 0.0;
        for history_length in (0..=maximum_history).rev() {
            let history_start = history.len() - history_length;
            let mut ngram = history[history_start..].to_vec();
            ngram.push(word.to_owned());
            if let Some(record) = self.records.get(&ngram) {
                return Ok(accumulated_backoff + record.probability_log10);
            }
            if history_length != 0 {
                let history_key = history[history_start..].to_vec();
                if let Some(backoff) = self
                    .records
                    .get(&history_key)
                    .and_then(|record| record.backoff_log10)
                {
                    accumulated_backoff += backoff;
                }
            }
        }
        Err("public ARPA model lacks a required unigram after <unk> mapping".into())
    }
}

#[derive(Debug)]
struct ArpaVocabularyScan {
    declared_order: usize,
    bytes: u64,
    sha256: String,
    known_query_tokens: HashSet<String>,
    has_unknown_token: bool,
}

fn audit_static_context(
    model_path: &Path,
    core_payload: &Path,
    fit_corpus: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
    sample_limit: usize,
    max_order: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public static-context fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public static-context held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    if candidate_sha256_hex(fit_text.as_bytes()) == candidate_sha256_hex(held_out_text.as_bytes()) {
        return Err("static-context-audit requires a distinct held-out corpus".into());
    }
    let core_entries = parse_lexicon_tsv(&core_text)?;
    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let decoder = Decoder::new(core_entries.clone());
    let (fit_cases, fit_selection) =
        freeze_static_context_cases(&fit, &core_entries, &decoder, frontier_limit, sample_limit)?;
    let (held_out_cases, held_out_selection) = freeze_static_context_cases(
        &held_out,
        &core_entries,
        &decoder,
        frontier_limit,
        sample_limit,
    )?;
    if fit_cases.is_empty() || held_out_cases.is_empty() {
        return Err("public corpora produced no eligible static-context cases".into());
    }

    let language_model = load_sparse_arpa_language_model(
        model_path,
        fit_cases.iter().chain(&held_out_cases),
        max_order,
    )?;
    let profiles = static_context_profiles();
    let mut fit_reports = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        fit_reports.push(evaluate_static_context_profile(
            &fit_cases,
            &language_model,
            *profile,
        )?);
    }
    let mut selected_index = 0;
    for index in 1..fit_reports.len() {
        if static_context_profile_precedes(&fit_reports[index], &fit_reports[selected_index]) {
            selected_index = index;
        }
    }
    let selected_profile = profiles[selected_index];
    let held_out_baseline =
        evaluate_static_context_profile(&held_out_cases, &language_model, profiles[0])?;
    let held_out_selected =
        evaluate_static_context_profile(&held_out_cases, &language_model, selected_profile)?;
    let held_out_gate_passed = held_out_selected.correct_top_one_gained
        > held_out_selected.correct_top_one_lost
        && held_out_selected.correct_top_one_lost == 0
        && held_out_selected.non_target_top_one_changes == 0;

    let mut output = String::new();
    writeln!(output, "公开静态上下文离线审计")?;
    writeln!(
        output,
        "模型：{} 字节 · SHA-256 {} · 声明 {}-gram · 本次使用 {}-gram",
        language_model.bytes,
        language_model.sha256,
        language_model.declared_order,
        language_model.effective_order,
    )?;
    writeln!(
        output,
        "句界：{}",
        if language_model.sentence_boundaries {
            "模型提供 <s>/</s>"
        } else {
            "模型未提供；按空上下文起始且不添加句末分"
        }
    )?;
    writeln!(
        output,
        "稀疏装载：需要 {} 条 N-gram，实际命中 {}；候选所需词型中 {} 个映射为 <unk>",
        language_model.required_ngrams,
        language_model.records.len(),
        language_model.unknown_query_tokens,
    )?;
    writeln!(
        output,
        "核心词典：{} 条 · SHA-256 {}",
        core_entries.len(),
        candidate_sha256_hex(core_text.as_bytes()),
    )?;
    write_static_context_selection(
        &mut output,
        "拟合",
        &fit,
        &fit_text,
        fit_selection,
        frontier_limit,
    )?;
    write_static_context_selection(
        &mut output,
        "保留评测",
        &held_out,
        &held_out_text,
        held_out_selection,
        frontier_limit,
    )?;
    writeln!(output, "\n拟合档位")?;
    for (profile, report) in profiles.iter().zip(&fit_reports) {
        write_static_context_profile_report(&mut output, *profile, report, frontier_limit)?;
    }
    writeln!(
        output,
        "拟合选择：{}；选择过程未读取保留评测答案。",
        static_context_profile_label(selected_profile),
    )?;
    writeln!(output, "\n保留评测")?;
    write_static_context_profile_report(
        &mut output,
        profiles[0],
        &held_out_baseline,
        frontier_limit,
    )?;
    write_static_context_profile_report(
        &mut output,
        selected_profile,
        &held_out_selected,
        frontier_limit,
    )?;
    if held_out_gate_passed {
        output.push_str(
            "结论：保留集净改善且未损失正确首选；可以继续研究离线蒸馏的小型 sidecar，但本次没有生成或接入运行时资料。\n",
        );
    } else {
        output.push_str(
            "结论：未通过保留集安全门；不得把该静态配置或由它推导的排序资料接入运行时。\n",
        );
    }
    output.push_str(
        "边界：候选与分词均冻结自现有完整双拼前沿；模型最多提升一个已有挑战者，不创建候选。\n本次操作：只读\n",
    );
    Ok(output)
}

fn audit_single_character_context(
    model_path: &Path,
    core_payload: &Path,
    fit_corpus: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
    sample_limit: usize,
    max_order: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public single-character context payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let fit_text = read_explicit_text(
        fit_corpus,
        "public single-character context fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public single-character context held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    if candidate_sha256_hex(fit_text.as_bytes()) == candidate_sha256_hex(held_out_text.as_bytes()) {
        return Err("single-character-context-audit requires a distinct held-out corpus".into());
    }
    let core_entries = parse_lexicon_tsv(&core_text)?;
    let core = snapshot_from_payload("single-character-context-audit-core-v1", &core_text)?;
    let fit = parse_ud_conllu(&fit_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let (fit_cases, fit_selection) = freeze_single_character_context_cases(
        &fit,
        &core_entries,
        &core,
        frontier_limit,
        sample_limit,
    )?;
    let (held_out_cases, held_out_selection) = freeze_single_character_context_cases(
        &held_out,
        &core_entries,
        &core,
        frontier_limit,
        sample_limit,
    )?;
    if fit_cases.is_empty() || held_out_cases.is_empty() {
        return Err("public corpora produced no eligible single-character context cases".into());
    }

    let language_model = load_sparse_arpa_language_model(
        model_path,
        fit_cases.iter().chain(&held_out_cases),
        max_order,
    )?;
    let profiles = single_character_context_profiles();
    let mut fit_reports = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        fit_reports.push(evaluate_static_context_profile(
            &fit_cases,
            &language_model,
            *profile,
        )?);
    }
    let mut selected_index = 0;
    for index in 1..fit_reports.len() {
        if static_context_profile_precedes(&fit_reports[index], &fit_reports[selected_index]) {
            selected_index = index;
        }
    }
    let selected_profile = profiles[selected_index];
    let held_out_baseline =
        evaluate_static_context_profile(&held_out_cases, &language_model, profiles[0])?;
    let held_out_selected =
        evaluate_static_context_profile(&held_out_cases, &language_model, selected_profile)?;
    let held_out_gate_passed = held_out_selected.correct_top_one_gained
        > held_out_selected.correct_top_one_lost
        && held_out_selected.correct_top_one_lost == 0
        && held_out_selected.non_target_top_one_changes == 0;

    let mut output = String::new();
    writeln!(output, "公开单字左上下文离线审计")?;
    writeln!(
        output,
        "模型：{} 字节 · SHA-256 {} · 声明 {}-gram · 本次使用 {}-gram",
        language_model.bytes,
        language_model.sha256,
        language_model.declared_order,
        language_model.effective_order,
    )?;
    writeln!(
        output,
        "句界：{}",
        if language_model.sentence_boundaries {
            "模型提供 <s>/</s>"
        } else {
            "模型未提供；按空上下文起始且不添加句末分"
        }
    )?;
    writeln!(
        output,
        "稀疏装载：需要 {} 条 N-gram，实际命中 {}；候选所需词型中 {} 个映射为 <unk>",
        language_model.required_ngrams,
        language_model.records.len(),
        language_model.unknown_query_tokens,
    )?;
    writeln!(
        output,
        "核心词典：{} 条 · SHA-256 {}",
        core_entries.len(),
        candidate_sha256_hex(core_text.as_bytes()),
    )?;
    write_single_character_context_selection(
        &mut output,
        "拟合",
        &fit,
        &fit_text,
        fit_selection,
        frontier_limit,
    )?;
    write_single_character_context_selection(
        &mut output,
        "保留评测",
        &held_out,
        &held_out_text,
        held_out_selection,
        frontier_limit,
    )?;
    writeln!(output, "\n拟合档位")?;
    for (profile, report) in profiles.iter().zip(&fit_reports) {
        write_static_context_profile_report(&mut output, *profile, report, frontier_limit)?;
    }
    writeln!(
        output,
        "拟合选择：{}；选择过程未读取保留评测答案。",
        static_context_profile_label(selected_profile),
    )?;
    writeln!(output, "\n保留评测")?;
    write_static_context_profile_report(
        &mut output,
        profiles[0],
        &held_out_baseline,
        frontier_limit,
    )?;
    write_static_context_profile_report(
        &mut output,
        selected_profile,
        &held_out_selected,
        frontier_limit,
    )?;
    if held_out_gate_passed {
        output.push_str(
            "结论：保留集净改善且没有正确首选损失或非目标首选变化；可以继续研究有界左上下文 sidecar，但本次没有生成或接入运行时资料。\n",
        );
    } else {
        output.push_str(
            "结论：未通过保留集安全门；不得把该单字左上下文配置或由它推导的排序资料接入运行时。\n",
        );
    }
    output.push_str(
        "边界：左侧身份和当前单字都来自公开语料与核心精确词；只冻结当前两键码已有精确单字候选，最多提升一个挑战者，不创建候选、不读取个人记录。\n本次操作：只读\n",
    );
    Ok(output)
}

fn audit_single_character_context_validation(
    model_path: &Path,
    core_payload: &Path,
    development_corpus: &Path,
    held_out_corpus: &Path,
    frontier_limit: usize,
    sample_limit: usize,
    max_order: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public single-character context validation payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let development_text = read_explicit_text(
        development_corpus,
        "public single-character context development corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let held_out_text = read_explicit_text(
        held_out_corpus,
        "public single-character context final held-out corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    if candidate_sha256_hex(development_text.as_bytes())
        == candidate_sha256_hex(held_out_text.as_bytes())
    {
        return Err(
            "single-character-context-validation-audit requires a distinct final held-out corpus"
                .into(),
        );
    }
    let core_entries = parse_lexicon_tsv(&core_text)?;
    let core = snapshot_from_payload(
        "single-character-context-validation-audit-core-v1",
        &core_text,
    )?;
    let development = parse_ud_conllu(&development_text)?;
    let held_out = parse_ud_conllu(&held_out_text)?;
    let (development_cases, development_selection) = freeze_single_character_context_cases(
        &development,
        &core_entries,
        &core,
        frontier_limit,
        sample_limit,
    )?;
    let (held_out_cases, held_out_selection) = freeze_single_character_context_cases(
        &held_out,
        &core_entries,
        &core,
        frontier_limit,
        sample_limit,
    )?;
    if development_cases.is_empty() || held_out_cases.is_empty() {
        return Err(
            "public corpora produced no eligible single-character context validation cases".into(),
        );
    }

    let language_model = load_sparse_arpa_language_model(
        model_path,
        development_cases.iter().chain(&held_out_cases),
        max_order,
    )?;
    let profiles = single_character_context_profiles();
    let mut development_reports = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        development_reports.push(evaluate_static_context_profile(
            &development_cases,
            &language_model,
            *profile,
        )?);
    }
    let mut selected_index = 0;
    for index in 1..development_reports.len() {
        if static_context_safe_profile_precedes(
            &development_reports[index],
            &development_reports[selected_index],
        ) {
            selected_index = index;
        }
    }
    let selected_profile = profiles[selected_index];
    let development_selected = development_reports[selected_index];
    let development_gate_passed = development_selected.correct_top_one_gained > 0
        && development_selected.correct_top_one_lost == 0
        && development_selected.non_target_top_one_changes == 0;
    let held_out_baseline =
        evaluate_static_context_profile(&held_out_cases, &language_model, profiles[0])?;
    let held_out_selected =
        evaluate_static_context_profile(&held_out_cases, &language_model, selected_profile)?;
    let held_out_gate_passed = held_out_selected.correct_top_one_gained > 0
        && held_out_selected.correct_top_one_lost == 0
        && held_out_selected.non_target_top_one_changes == 0;

    let mut output = String::new();
    writeln!(output, "公开单字左上下文最终验证")?;
    writeln!(
        output,
        "模型：{} 字节 · SHA-256 {} · 声明 {}-gram · 本次使用 {}-gram",
        language_model.bytes,
        language_model.sha256,
        language_model.declared_order,
        language_model.effective_order,
    )?;
    writeln!(
        output,
        "句界：{}",
        if language_model.sentence_boundaries {
            "模型提供 <s>/</s>"
        } else {
            "模型未提供；按空上下文起始且不添加句末分"
        }
    )?;
    writeln!(
        output,
        "稀疏装载：需要 {} 条 N-gram，实际命中 {}；候选所需词型中 {} 个映射为 <unk>",
        language_model.required_ngrams,
        language_model.records.len(),
        language_model.unknown_query_tokens,
    )?;
    writeln!(
        output,
        "核心词典：{} 条 · SHA-256 {}",
        core_entries.len(),
        candidate_sha256_hex(core_text.as_bytes()),
    )?;
    write_single_character_context_selection(
        &mut output,
        "开发",
        &development,
        &development_text,
        development_selection,
        frontier_limit,
    )?;
    write_single_character_context_selection(
        &mut output,
        "最终保留评测",
        &held_out,
        &held_out_text,
        held_out_selection,
        frontier_limit,
    )?;
    writeln!(output, "\n开发集档位")?;
    for (profile, report) in profiles.iter().zip(&development_reports) {
        write_static_context_profile_report(&mut output, *profile, report, frontier_limit)?;
    }
    writeln!(
        output,
        "开发选择：{}；只允许正确首选损失为 0、非目标首选变化为 0 的档位竞争，再最大化新增正确首选。",
        static_context_profile_label(selected_profile),
    )?;
    writeln!(output, "\n最终保留评测")?;
    write_static_context_profile_report(
        &mut output,
        profiles[0],
        &held_out_baseline,
        frontier_limit,
    )?;
    write_static_context_profile_report(
        &mut output,
        selected_profile,
        &held_out_selected,
        frontier_limit,
    )?;
    if development_gate_passed && held_out_gate_passed {
        output.push_str(
            "结论：开发集与最终保留集都净改善，且没有正确首选损失或非目标首选变化；可以继续研究离线蒸馏与运行时成本，但本次没有生成或接入运行时资料。\n",
        );
    } else {
        output.push_str(
            "结论：开发集或最终保留集未通过严格安全门；不得把该配置或由它推导的排序资料接入运行时。\n",
        );
    }
    output.push_str(
        "边界：开发集可以选择预声明档位；最终保留集只运行冻结基线与开发集选中的一个档位。候选来自当前两键码已有精确单字池，最多提升一个挑战者，不创建候选、不读取个人记录。\n本次操作：只读\n",
    );
    Ok(output)
}

fn freeze_single_character_context_cases(
    corpus: &ziranma_core::UdCorpus,
    core_entries: &[LexiconEntry],
    core: &CandidateSnapshot,
    frontier_limit: usize,
    sample_limit: usize,
) -> Result<
    (
        Vec<FrozenStaticContextCase>,
        SingleCharacterContextSelectionStats,
    ),
    Box<dyn std::error::Error>,
> {
    let oversample_limit = sample_limit.saturating_mul(8).max(sample_limit);
    let selection =
        select_public_single_character_context_cases(corpus, core_entries, oversample_limit);
    let ambiguous_surfaces = ambiguous_lexicon_surfaces(core_entries);
    let mut stats = SingleCharacterContextSelectionStats {
        source_windows: selection.stats.source_windows,
        single_character_targets: selection.stats.single_character_targets,
        exact_word_coverable: selection.stats.exact_word_coverable,
        source_representatives: selection.stats.sentence_representatives,
        ..SingleCharacterContextSelectionStats::default()
    };
    let mut cases = Vec::with_capacity(sample_limit);
    for probe in selection.probes {
        if ambiguous_surfaces.contains(probe.expected_text.as_str()) {
            stats.ambiguous_target_surfaces += 1;
            continue;
        }
        if probe.observed.as_str().len() != 2 {
            stats.non_two_key_targets += 1;
            continue;
        }
        let mut candidates =
            core.exact_full_code_texts(probe.observed.as_str(), MAX_CANDIDATE_SNAPSHOT_RANK)?;
        if candidates.len() < 2 {
            stats.uncompetitive_exact_pools += 1;
            continue;
        }
        candidates.truncate(frontier_limit);
        stats.target_outside_frontier +=
            usize::from(candidate_rank(&candidates, &probe.expected_text).is_none());
        cases.push(FrozenStaticContextCase {
            expected_text: probe.expected_text,
            candidates: candidates
                .into_iter()
                .map(|candidate| FrozenStaticContextCandidate {
                    text: candidate.clone(),
                    segments: vec![probe.previous_text.clone(), candidate],
                })
                .collect(),
        });
        if cases.len() == sample_limit {
            break;
        }
    }
    stats.selected = cases.len();
    Ok((cases, stats))
}

fn write_single_character_context_selection(
    output: &mut String,
    label: &str,
    corpus: &ziranma_core::UdCorpus,
    corpus_text: &str,
    stats: SingleCharacterContextSelectionStats,
    frontier_limit: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(
        output,
        "{label}语料：{} 句，{} 个句法 token · SHA-256 {}",
        corpus.stats.sentences,
        corpus.stats.syntactic_tokens,
        candidate_sha256_hex(corpus_text.as_bytes()),
    )?;
    writeln!(
        output,
        "  邻接窗 {}；单字目标 {}；双端核心覆盖 {}；句级代表 {}；多音目标排除 {}；非两键排除 {}；无同码竞争排除 {}；冻结 {}；目标在 Top-{frontier_limit} 外 {}",
        stats.source_windows,
        stats.single_character_targets,
        stats.exact_word_coverable,
        stats.source_representatives,
        stats.ambiguous_target_surfaces,
        stats.non_two_key_targets,
        stats.uncompetitive_exact_pools,
        stats.selected,
        stats.target_outside_frontier,
    )
}

fn freeze_static_context_cases(
    corpus: &ziranma_core::UdCorpus,
    core_entries: &[LexiconEntry],
    decoder: &Decoder,
    frontier_limit: usize,
    sample_limit: usize,
) -> Result<(Vec<FrozenStaticContextCase>, StaticContextSelectionStats), Box<dyn std::error::Error>>
{
    let oversample_limit = sample_limit.saturating_mul(8).max(sample_limit);
    let selection = select_public_static_context_cases(corpus, core_entries, oversample_limit);
    let core_texts = core_entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let core_codes = core_entries
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let mut stats = StaticContextSelectionStats {
        source_representatives: selection.stats.sentence_representatives,
        ..StaticContextSelectionStats::default()
    };
    let mut cases = Vec::with_capacity(sample_limit);
    for probe in selection.probes {
        if core_texts.contains(probe.expected_text.as_str()) {
            stats.whole_text_collisions += 1;
            continue;
        }
        if core_codes.contains(probe.observed.as_str()) {
            stats.whole_code_collisions += 1;
            continue;
        }
        let candidates = decoder
            .decode_complete_sentence(probe.observed.as_str(), frontier_limit)?
            .into_iter()
            .map(|candidate| FrozenStaticContextCandidate {
                text: candidate.text,
                segments: candidate
                    .segments
                    .into_iter()
                    .map(|segment| segment.candidate.text)
                    .collect(),
            })
            .collect::<Vec<_>>();
        stats.empty_frontiers += usize::from(candidates.is_empty());
        cases.push(FrozenStaticContextCase {
            expected_text: probe.expected_text,
            candidates,
        });
        if cases.len() == sample_limit {
            break;
        }
    }
    stats.selected = cases.len();
    Ok((cases, stats))
}

fn load_sparse_arpa_language_model<'a>(
    path: &Path,
    cases: impl Iterator<Item = &'a FrozenStaticContextCase> + Clone,
    requested_order: usize,
) -> Result<SparseArpaLanguageModel, Box<dyn std::error::Error>> {
    let candidate_query_tokens = cases
        .clone()
        .flat_map(|case| &case.candidates)
        .flat_map(|candidate| &candidate.segments)
        .cloned()
        .collect::<HashSet<_>>();
    let mut query_tokens = candidate_query_tokens.clone();
    query_tokens.insert("<s>".to_owned());
    query_tokens.insert("</s>".to_owned());
    let scan = scan_arpa_vocabulary(path, &query_tokens)?;
    let has_start_boundary = scan.known_query_tokens.contains("<s>");
    let has_end_boundary = scan.known_query_tokens.contains("</s>");
    if has_start_boundary != has_end_boundary {
        return Err("public ARPA model must contain both <s> and </s>, or neither".into());
    }
    let sentence_boundaries = has_start_boundary;
    let missing_tokens = candidate_query_tokens
        .iter()
        .filter(|token| !scan.known_query_tokens.contains(*token))
        .count();
    if missing_tokens != 0 && !scan.has_unknown_token {
        return Err("public ARPA model lacks <unk> for candidate vocabulary misses".into());
    }
    let effective_order = requested_order.min(scan.declared_order);
    let canonicalize = |token: &str| {
        if scan.known_query_tokens.contains(token) {
            token.to_owned()
        } else {
            "<unk>".to_owned()
        }
    };
    let mut required = HashSet::<Vec<String>>::new();
    for case in cases {
        for candidate in &case.candidates {
            let mut sequence = Vec::with_capacity(candidate.segments.len() + 2);
            if sentence_boundaries {
                sequence.push("<s>".to_owned());
            }
            sequence.extend(candidate.segments.iter().map(|token| canonicalize(token)));
            if sentence_boundaries {
                sequence.push("</s>".to_owned());
            }
            for start in 0..sequence.len() {
                for order in 1..=effective_order.min(sequence.len() - start) {
                    required.insert(sequence[start..start + order].to_vec());
                }
            }
        }
    }
    if sentence_boundaries {
        required.insert(vec!["<s>".to_owned()]);
        required.insert(vec!["</s>".to_owned()]);
    }
    if missing_tokens != 0 {
        required.insert(vec!["<unk>".to_owned()]);
    }
    let records =
        load_required_arpa_records(path, &required, effective_order, scan.bytes, &scan.sha256)?;
    Ok(SparseArpaLanguageModel {
        declared_order: scan.declared_order,
        effective_order,
        sentence_boundaries,
        bytes: scan.bytes,
        sha256: scan.sha256,
        known_query_tokens: scan.known_query_tokens,
        unknown_query_tokens: missing_tokens,
        required_ngrams: required.len(),
        records,
    })
}

fn scan_arpa_vocabulary(
    path: &Path,
    query_tokens: &HashSet<String>,
) -> Result<ArpaVocabularyScan, Box<dyn std::error::Error>> {
    let mut section = None;
    let mut declared_order = 0;
    let mut saw_data = false;
    let mut saw_end = false;
    let mut known_query_tokens = HashSet::new();
    let mut has_unknown_token = false;
    let mut hasher = Sha256::new();
    let bytes = visit_arpa_lines(path, Some(&mut hasher), |line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            return Ok(());
        }
        if line == "\\data\\" {
            saw_data = true;
            section = None;
            return Ok(());
        }
        if line == "\\end\\" {
            saw_end = true;
            section = None;
            return Ok(());
        }
        if let Some(order) = parse_arpa_section_marker(line)? {
            section = Some(order);
            declared_order = declared_order.max(order);
            return Ok(());
        }
        if let Some(declaration) = line.strip_prefix("ngram ") {
            let (order, _) = declaration
                .split_once('=')
                .ok_or("invalid public ARPA ngram declaration")?;
            let order = order
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid public ARPA ngram order")?;
            declared_order = declared_order.max(order);
            return Ok(());
        }
        if section == Some(1) {
            let ngram = parse_arpa_ngram_line(line, 1)?;
            let token = &ngram.tokens[0];
            if query_tokens.contains(token) {
                known_query_tokens.insert(token.clone());
            }
            has_unknown_token |= token == "<unk>";
        }
        Ok(())
    })?;
    if !saw_data || !saw_end || declared_order == 0 {
        return Err("public ARPA model is missing required structure".into());
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ArpaVocabularyScan {
        declared_order,
        bytes,
        sha256,
        known_query_tokens,
        has_unknown_token,
    })
}

fn load_required_arpa_records(
    path: &Path,
    required: &HashSet<Vec<String>>,
    maximum_order: usize,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<HashMap<Vec<String>, SparseArpaRecord>, Box<dyn std::error::Error>> {
    let mut section = None;
    let mut records = HashMap::with_capacity(required.len());
    let mut hasher = Sha256::new();
    let bytes = visit_arpa_lines(path, Some(&mut hasher), |line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line == "\\data\\" || line == "\\end\\" {
            if line == "\\data\\" || line == "\\end\\" {
                section = None;
            }
            return Ok(());
        }
        if let Some(order) = parse_arpa_section_marker(line)? {
            section = Some(order);
            return Ok(());
        }
        let Some(order) = section else {
            return Ok(());
        };
        if order > maximum_order {
            return Ok(());
        }
        let ngram = parse_arpa_ngram_line(line, order)?;
        if required.contains(&ngram.tokens)
            && records
                .insert(
                    ngram.tokens,
                    SparseArpaRecord {
                        probability_log10: ngram.probability_log10,
                        backoff_log10: ngram.backoff_log10,
                    },
                )
                .is_some()
        {
            return Err("public ARPA model contains a duplicate required N-gram".into());
        }
        Ok(())
    })?;
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if bytes != expected_bytes || sha256 != expected_sha256 {
        return Err("public ARPA model changed between sparse audit passes".into());
    }
    Ok(records)
}

fn visit_arpa_lines(
    path: &Path,
    mut hasher: Option<&mut Sha256>,
    mut visitor: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "cannot inspect explicitly named public ARPA model")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("public ARPA model must be a regular non-symbolic-link file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_STATIC_CONTEXT_ARPA_BYTES {
        return Err("public ARPA model size is outside the fixed byte bound".into());
    }
    let file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file() || opened_metadata.len() != metadata.len() {
        return Err("public ARPA model changed while it was being opened".into());
    }
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut total = 0_u64;
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or("public ARPA model byte count overflowed")?;
        if total > MAX_STATIC_CONTEXT_ARPA_BYTES {
            return Err("public ARPA model exceeds the fixed byte bound".into());
        }
        if buffer.len() > MAX_STATIC_CONTEXT_ARPA_LINE_BYTES {
            return Err("public ARPA model contains an oversized line".into());
        }
        if let Some(hasher) = hasher.as_deref_mut() {
            hasher.update(&buffer);
        }
        let line = std::str::from_utf8(&buffer)?.trim_end_matches(['\r', '\n']);
        visitor(line)?;
    }
    if total != metadata.len() {
        return Err("public ARPA model changed while it was being read".into());
    }
    Ok(total)
}

fn parse_arpa_section_marker(line: &str) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    let Some(section) = line
        .strip_prefix('\\')
        .and_then(|line| line.strip_suffix("-grams:"))
    else {
        return Ok(None);
    };
    let order = section
        .parse::<usize>()
        .map_err(|_| "invalid public ARPA section order")?;
    if order == 0 {
        return Err("public ARPA section order must be positive".into());
    }
    Ok(Some(order))
}

fn parse_arpa_ngram_line(
    line: &str,
    order: usize,
) -> Result<ParsedArpaNgram, Box<dyn std::error::Error>> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() != order + 1 && fields.len() != order + 2 {
        return Err("public ARPA N-gram row has an invalid field count".into());
    }
    let probability_log10 = fields[0]
        .parse::<f64>()
        .map_err(|_| "public ARPA probability is invalid")?;
    if !probability_log10.is_finite() {
        return Err("public ARPA probability must be finite".into());
    }
    let ngram = fields[1..=order]
        .iter()
        .map(|token| (*token).to_owned())
        .collect::<Vec<_>>();
    let backoff_log10 = fields.get(order + 1).map(|value| {
        value
            .parse::<f64>()
            .map_err(|_| "public ARPA backoff is invalid")
    });
    let backoff_log10 = match backoff_log10 {
        Some(Ok(value)) if value.is_finite() => Some(value),
        Some(Ok(_)) => return Err("public ARPA backoff must be finite".into()),
        Some(Err(error)) => return Err(error.into()),
        None => None,
    };
    Ok(ParsedArpaNgram {
        probability_log10,
        tokens: ngram,
        backoff_log10,
    })
}

fn evaluate_static_context_profile(
    cases: &[FrozenStaticContextCase],
    model: &SparseArpaLanguageModel,
    profile: StaticContextProfile,
) -> Result<StaticContextProfileReport, Box<dyn std::error::Error>> {
    let mut report = StaticContextProfileReport {
        total: cases.len(),
        ..StaticContextProfileReport::default()
    };
    for case in cases {
        let baseline_target = case
            .candidates
            .iter()
            .position(|candidate| candidate.text == case.expected_text);
        let baseline_rank = baseline_target.map(|index| index + 1);
        report.baseline.observe(baseline_rank);
        let scores = case
            .candidates
            .iter()
            .map(|candidate| model.score_candidate(candidate))
            .collect::<Result<Vec<_>, _>>()?;
        let promoted = static_context_promoted_index(&scores, profile);
        let candidate_rank = baseline_target.map(|target| {
            if promoted == 0 {
                target + 1
            } else if target == promoted {
                1
            } else if target < promoted {
                target + 2
            } else {
                target + 1
            }
        });
        report.candidate.observe(candidate_rank);
        match (baseline_rank, candidate_rank) {
            (Some(before), Some(after)) if after < before => report.rank_improved += 1,
            (Some(before), Some(after)) if after > before => report.rank_worsened += 1,
            (Some(_), Some(_)) | (None, None) => report.rank_unchanged += 1,
            _ => {}
        }
        let baseline_correct = baseline_target == Some(0);
        let candidate_correct = baseline_target == Some(promoted);
        report.correct_top_one_gained += usize::from(!baseline_correct && candidate_correct);
        report.correct_top_one_lost += usize::from(baseline_correct && !candidate_correct);
        if promoted != 0 {
            report.any_top_one_changes += 1;
            report.non_target_top_one_changes += usize::from(!candidate_correct);
        }
    }
    Ok(report)
}

fn static_context_promoted_index(scores: &[f64], profile: StaticContextProfile) -> usize {
    if scores.len() < 2 || profile.search_depth <= 1 {
        return 0;
    }
    let depth = scores.len().min(profile.search_depth);
    let challenger = scores.iter().take(depth).enumerate().skip(1).max_by(
        |(left_index, left), (right_index, right)| {
            left.total_cmp(right)
                .then_with(|| right_index.cmp(left_index))
        },
    );
    let Some((index, score)) = challenger else {
        return 0;
    };
    if score - scores[0] >= profile.minimum_average_gain {
        index
    } else {
        0
    }
}

fn static_context_profile_precedes(
    challenger: &StaticContextProfileReport,
    current: &StaticContextProfileReport,
) -> bool {
    let challenger_net =
        challenger.correct_top_one_gained as isize - challenger.correct_top_one_lost as isize;
    let current_net =
        current.correct_top_one_gained as isize - current.correct_top_one_lost as isize;
    (challenger.correct_top_one_lost == 0)
        .cmp(&(current.correct_top_one_lost == 0))
        .then_with(|| challenger_net.cmp(&current_net))
        .then_with(|| {
            challenger
                .correct_top_one_gained
                .cmp(&current.correct_top_one_gained)
        })
        .then_with(|| {
            current
                .non_target_top_one_changes
                .cmp(&challenger.non_target_top_one_changes)
        })
        .then_with(|| {
            challenger
                .candidate
                .at_three
                .cmp(&current.candidate.at_three)
        })
        .is_gt()
}

fn static_context_safe_profile_precedes(
    challenger: &StaticContextProfileReport,
    current: &StaticContextProfileReport,
) -> bool {
    let challenger_safe =
        challenger.correct_top_one_lost == 0 && challenger.non_target_top_one_changes == 0;
    let current_safe = current.correct_top_one_lost == 0 && current.non_target_top_one_changes == 0;
    challenger_safe
        .cmp(&current_safe)
        .then_with(|| {
            challenger
                .correct_top_one_gained
                .cmp(&current.correct_top_one_gained)
        })
        .then_with(|| {
            challenger
                .candidate
                .at_three
                .cmp(&current.candidate.at_three)
        })
        .then_with(|| challenger.candidate.at_five.cmp(&current.candidate.at_five))
        .is_gt()
}

fn static_context_profile_label(profile: StaticContextProfile) -> String {
    if profile.search_depth <= 1 {
        "冻结基线".to_owned()
    } else {
        format!(
            "ARPA-d{}-g{:.2}",
            profile.search_depth, profile.minimum_average_gain
        )
    }
}

fn write_static_context_profile_report(
    output: &mut String,
    profile: StaticContextProfile,
    report: &StaticContextProfileReport,
    frontier_limit: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(
        output,
        "{}：目标 Top-1 {}/{}、Top-3 {}、Top-5 {}、Top-{frontier_limit} {}；升/平/降 {}/{}/{}；正确首选 +{} / -{}；非目标首选变化 {}",
        static_context_profile_label(profile),
        report.candidate.at_one,
        report.total,
        report.candidate.at_three,
        report.candidate.at_five,
        report.candidate.visible,
        report.rank_improved,
        report.rank_unchanged,
        report.rank_worsened,
        report.correct_top_one_gained,
        report.correct_top_one_lost,
        report.non_target_top_one_changes,
    )
}

fn write_static_context_selection(
    output: &mut String,
    label: &str,
    corpus: &ziranma_core::UdCorpus,
    corpus_text: &str,
    selection: StaticContextSelectionStats,
    frontier_limit: usize,
) -> Result<(), std::fmt::Error> {
    writeln!(
        output,
        "{label}：{} 句 · SHA-256 {} · 自然相邻词代表 {}，排除整词/整码碰撞 {}/{}，冻结样本 {}，空前沿 {}，每例最多 {frontier_limit} 个候选",
        corpus.stats.sentences,
        candidate_sha256_hex(corpus_text.as_bytes()),
        selection.source_representatives,
        selection.whole_text_collisions,
        selection.whole_code_collisions,
        selection.selected,
        selection.empty_frontiers,
    )
}

fn audit_candidate_layers(
    core_payload: &Path,
    supplemental_payload: &Path,
    frontier_limit: usize,
    exact_promotions: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        supplemental_payload,
        "supplemental public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let core = parse_lexicon_tsv(&core_text)?;
    let supplemental = parse_lexicon_tsv(&supplemental_text)?;
    let report = audit_public_supplemental_layer(
        &core,
        &supplemental,
        frontier_limit,
        SupplementalCandidateLayerConfig { exact_promotions },
    )?;
    if report.core_top_one_changed_codes != 0 {
        return Err("supplemental layer changed a core exact Top-1".into());
    }
    let admitted_percent = if report.available_new_exact_candidates == 0 {
        0.0
    } else {
        report.admitted_new_exact_candidates as f64 * 100.0
            / report.available_new_exact_candidates as f64
    };
    Ok(format!(
        "公开补充词层审计\n候选范围：前 {frontier_limit}；每码最多补 {exact_promotions} 个完整词\n补充规范码：{}（与核心共有 {}，仅补充层有 {}）\n新增完整词：可用 {}，进入候选范围 {}（{admitted_percent:.2}%）\n受益规范码：{}（已有核心词 {}，核心缺词 {}）\n核心完整码首选：保留 {}，变化 {}\n核心缺词码升为首选：{}\n单码最多实际补入：{}\n跨来源原始权重：未比较\n本次操作：只读\n",
        report.supplemental_codes,
        report.shared_exact_codes,
        report.supplemental_only_codes,
        report.available_new_exact_candidates,
        report.admitted_new_exact_candidates,
        report.codes_receiving_new_exact_candidates,
        report.shared_codes_receiving_new_exact_candidates,
        report.supplemental_only_codes_receiving_new_exact_candidates,
        report.core_top_one_preserved_codes,
        report.core_top_one_changed_codes,
        report.supplemental_only_codes_promoted_to_top_one,
        report.maximum_admitted_new_exact_candidates_per_code,
    ))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CompositionRankCounts {
    at_one: usize,
    at_three: usize,
    at_five: usize,
    visible: usize,
}

impl CompositionRankCounts {
    fn observe(&mut self, rank: Option<usize>) {
        let Some(rank) = rank else {
            return;
        };
        self.visible += 1;
        self.at_one += usize::from(rank <= 1);
        self.at_three += usize::from(rank <= 3);
        self.at_five += usize::from(rank <= 5);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CompositionAuditReport {
    total: usize,
    core: CompositionRankCounts,
    layered: CompositionRankCounts,
    preserve_core_top: CompositionRankCounts,
    preserve_core_top_two: CompositionRankCounts,
    newly_visible: usize,
    lost_visible: usize,
    preserve_core_top_newly_visible: usize,
    preserve_core_top_lost_visible: usize,
    preserve_core_top_two_newly_visible: usize,
    preserve_core_top_two_lost_visible: usize,
    rank_improved: usize,
    rank_unchanged: usize,
    rank_worsened: usize,
    candidate_order_changed: usize,
    target_promoted_to_top_one: usize,
    non_target_top_one_changes: usize,
    core_exact_top_preserved: usize,
    core_exact_top_changed: usize,
    missing_supplemental_edge_depth: usize,
    missing_core_edge_depth: usize,
    missing_competing_composition: usize,
    target_composition_rank_one: usize,
    target_composition_rank_two: usize,
    target_composition_rank_three_or_four: usize,
    target_composition_rank_five_to_eight: usize,
    target_composition_outside_eight: usize,
    missing_probe_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CoreCompositionControlSelectionStats {
    source_windows: usize,
    exact_word_coverable: usize,
    sentence_representatives: usize,
    whole_phrase_collisions: usize,
    whole_code_collisions: usize,
    selected: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CompositionControlStrategyReport {
    target: CompositionRankCounts,
    target_newly_visible: usize,
    target_lost_visible: usize,
    target_rank_improved: usize,
    target_rank_unchanged: usize,
    target_rank_worsened: usize,
    top_one_changed: usize,
    samples_evicting_core_candidates: usize,
    core_candidates_evicted: usize,
    maximum_core_candidates_evicted: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CoreCompositionControlReport {
    total: usize,
    core: CompositionRankCounts,
    layered: CompositionControlStrategyReport,
    preserve_core_top_one_slot: CompositionControlStrategyReport,
    preserve_core_top_two_slots: CompositionControlStrategyReport,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CompositionOrderProfileReport {
    supplemental_probes: CompositionControlStrategyReport,
    core_controls: CompositionControlStrategyReport,
    supplemental_composition_choice_changed: usize,
    core_composition_choice_changed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CharacterCompositionProfile {
    label: &'static str,
    search_depth: usize,
    minimum_average_gain: f64,
    minimum_observed_ratio_milli: usize,
}

const CHARACTER_COMPOSITION_PROFILES: [CharacterCompositionProfile; 9] = [
    CharacterCompositionProfile {
        label: "结构基线",
        search_depth: 1,
        minimum_average_gain: 0.0,
        minimum_observed_ratio_milli: 0,
    },
    CharacterCompositionProfile {
        label: "字境-d4-g0.10-r0.50",
        search_depth: 4,
        minimum_average_gain: 0.10,
        minimum_observed_ratio_milli: 500,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.10-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.10,
        minimum_observed_ratio_milli: 500,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.25-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.25,
        minimum_observed_ratio_milli: 500,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.50-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.50,
        minimum_observed_ratio_milli: 500,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.25-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.25,
        minimum_observed_ratio_milli: 750,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.50-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.50,
        minimum_observed_ratio_milli: 750,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g0.75-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.75,
        minimum_observed_ratio_milli: 750,
    },
    CharacterCompositionProfile {
        label: "字境-d8-g1.00-r1.00",
        search_depth: 8,
        minimum_average_gain: 1.00,
        minimum_observed_ratio_milli: 1_000,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct WordBoundaryCompositionProfile {
    label: &'static str,
    search_depth: usize,
    minimum_average_gain: f64,
    minimum_observed_ratio_milli: usize,
}

const WORD_BOUNDARY_COMPOSITION_PROFILES: [WordBoundaryCompositionProfile; 9] = [
    WordBoundaryCompositionProfile {
        label: "结构基线",
        search_depth: 1,
        minimum_average_gain: 0.0,
        minimum_observed_ratio_milli: 0,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d4-g0.05-r0.50",
        search_depth: 4,
        minimum_average_gain: 0.05,
        minimum_observed_ratio_milli: 500,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.05-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.05,
        minimum_observed_ratio_milli: 500,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.10-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.10,
        minimum_observed_ratio_milli: 500,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.25-r0.50",
        search_depth: 8,
        minimum_average_gain: 0.25,
        minimum_observed_ratio_milli: 500,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.10-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.10,
        minimum_observed_ratio_milli: 750,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.25-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.25,
        minimum_observed_ratio_milli: 750,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.50-r0.75",
        search_depth: 8,
        minimum_average_gain: 0.50,
        minimum_observed_ratio_milli: 750,
    },
    WordBoundaryCompositionProfile {
        label: "词界-d8-g0.25-r1.00",
        search_depth: 8,
        minimum_average_gain: 0.25,
        minimum_observed_ratio_milli: 1_000,
    },
];

#[derive(Clone, Debug)]
struct PublicWordBoundaryModel {
    token_counts: HashMap<String, usize>,
    token_instances: usize,
    alpha: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PublicWordBoundaryScore {
    log_probability: f64,
    syllable_count: usize,
    observed_syllables: usize,
}

impl PublicWordBoundaryModel {
    fn from_sequences(sequences: &[Vec<String>]) -> Option<Self> {
        let mut token_counts = HashMap::new();
        let mut token_instances = 0_usize;
        for token in sequences.iter().flatten() {
            *token_counts.entry(token.clone()).or_insert(0) += 1;
            token_instances += 1;
        }
        if token_counts.is_empty() {
            return None;
        }
        Some(Self {
            token_counts,
            token_instances,
            alpha: 0.5,
        })
    }

    fn score(&self, candidate: &SupplementalCompositionCandidate) -> PublicWordBoundaryScore {
        let denominator =
            self.token_instances as f64 + self.alpha * (self.token_counts.len() as f64 + 1.0);
        let mut log_probability = 0.0;
        let mut syllable_count = 0;
        let mut observed_syllables = 0;
        for segment in candidate.segments() {
            let count = self.token_counts.get(segment.text()).copied().unwrap_or(0);
            log_probability += ((count as f64 + self.alpha) / denominator).ln();
            syllable_count += segment.syllable_count();
            if count > 0 {
                observed_syllables += segment.syllable_count();
            }
        }
        PublicWordBoundaryScore {
            log_probability,
            syllable_count,
            observed_syllables,
        }
    }

    fn observed_token_types(&self) -> usize {
        self.token_counts.len()
    }
}

fn audit_candidate_layer_compositions(
    core_payload: &Path,
    supplemental_payload: &Path,
    corpus: &Path,
    fit_corpus: Option<&Path>,
    frontier_limit: usize,
    sample_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let core_text = read_explicit_text(
        core_payload,
        "core public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        supplemental_payload,
        "supplemental public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let corpus_text = read_explicit_text(
        corpus,
        "public composition audit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    let core_entries = parse_lexicon_tsv(&core_text)?;
    let supplemental_entries = parse_lexicon_tsv(&supplemental_text)?;
    let corpus = parse_ud_conllu(&corpus_text)?;
    let core_payload_sha256 = candidate_sha256_hex(core_text.as_bytes());
    let supplemental_payload_sha256 = candidate_sha256_hex(supplemental_text.as_bytes());
    let heldout_corpus_sha256 = candidate_sha256_hex(corpus_text.as_bytes());
    let selection = select_public_supplemental_composition_cases(
        &corpus,
        &core_entries,
        &supplemental_entries,
        sample_limit,
    );
    if selection.probes.is_empty() {
        return Err("public corpus produced no eligible supplemental composition probes".into());
    }
    let (control_probes, control_selection_stats) = select_core_composition_controls(
        &corpus,
        &core_entries,
        &supplemental_entries,
        sample_limit,
    );
    if control_probes.is_empty() {
        return Err("public corpus produced no eligible all-core composition controls".into());
    }

    let core = snapshot_from_payload("layer-composition-core-v1", &core_text)?;
    let supplemental =
        snapshot_from_payload("layer-composition-supplemental-v1", &supplemental_text)?;
    let report = evaluate_candidate_layer_compositions(
        &core,
        &supplemental,
        &selection.probes,
        frontier_limit,
    )?;
    if report.core_exact_top_changed != 0 {
        return Err("composition audit observed a changed core exact Top-1".into());
    }
    let control_report =
        evaluate_core_composition_controls(&core, &supplemental, &control_probes, frontier_limit)?;
    let mut order_reports = Vec::new();
    for (label, order) in [
        ("结构 V1", SupplementalCompositionOrder::StructuralV1),
        (
            "少分段优先",
            SupplementalCompositionOrder::FewerSegmentsFirst,
        ),
        (
            "层内名次优先",
            SupplementalCompositionOrder::LocalRanksFirst,
        ),
    ] {
        order_reports.push((
            label,
            evaluate_composition_order_profile(
                &core,
                &supplemental,
                &selection.probes,
                &control_probes,
                frontier_limit,
                order,
            )?,
        ));
    }
    let structural_profile = order_reports.first().map(|(_, profile)| profile);
    if structural_profile.is_none_or(|profile| {
        profile.core_controls != control_report.layered
            || profile.supplemental_probes.target != report.layered
            || profile.supplemental_probes.target_newly_visible != report.newly_visible
            || profile.supplemental_probes.target_lost_visible != report.lost_visible
            || profile.supplemental_probes.top_one_changed
                != report.target_promoted_to_top_one + report.non_target_top_one_changes
    }) {
        return Err("composition audit simulation diverged from the runtime candidate path".into());
    }
    let character_context_output = match fit_corpus {
        Some(fit_corpus_path) => audit_character_composition_context(
            fit_corpus_path,
            &corpus_text,
            &core_entries,
            &supplemental_entries,
            &core,
            &supplemental,
            &selection.probes,
            &control_probes,
            frontier_limit,
            sample_limit,
        )?,
        None => String::new(),
    };

    let stats = selection.stats;
    let missing = if report.missing_probe_ids.is_empty() {
        "无".to_owned()
    } else {
        report.missing_probe_ids.join("、")
    };
    let mut output = format!(
        "公开补充组合审计\n候选载荷：核心 {} 条 · SHA-256 {}；补充 {} 条 · SHA-256 {}\n保留语料：SHA-256 {}\n语料：{} 句；检查两词与三词相邻窗口\n筛选：窗口 {}；汉字与长度合格 {}；恰有一个补充多字词 {}；排除整词文字 {}、整词同码 {}；句形代表 {}\n样本：{}（两词 {}，三词 {}；补充词在前 {}、居中 {}、在后 {}）\n核心基线：Top-1 {}，Top-3 {}，Top-5 {}，Top-{frontier_limit} {}\n当前组合：Top-1 {}，Top-3 {}，Top-5 {}，Top-{frontier_limit} {}\n保留核心首选模拟：Top-1 {}，Top-3 {}，Top-5 {}，Top-{frontier_limit} {}\n保留首选并给两个组合位：Top-1 {}，Top-3 {}，Top-5 {}，Top-{frontier_limit} {}\n当前召回变化：新增可见 {}，原可见丢失 {}；共同可见中升位 {}、不变 {}、降位 {}\n保留首选模拟：新增可见 {}，原可见丢失 {}\n两个组合位模拟：新增可见 {}，原可见丢失 {}\n目标的结构组合排名：第 1 位 {}，第 2 位 {}，第 3～4 位 {}，第 5～8 位 {}，前 8 外 {}\n候选顺序：发生变化 {}；目标升为首选 {}；非目标首选变化 {}\n核心完整码首选：保留 {}，变化 {}\n当前仍未进入 Top-{frontier_limit}：补充边深度 {}，核心边深度 {}，竞争组合 {}\n样本编号（最多 12）：{missing}\n\n全核心短语负对照\n筛选：窗口 {}；核心整词覆盖 {}；句代表 {}；排除整词文字 {}、整词同码 {}；样本 {}\n核心基线目标：Top-1 {}，Top-3 {}，Top-5 {}，Top-{frontier_limit} {}\n当前组合：目标 Top-1 {}、Top-3 {}、Top-5 {}、Top-{frontier_limit} {}；新增可见 {}，原可见丢失 {}；共同可见中升位 {}、不变 {}、降位 {}；普通首选变化 {}；挤出核心候选 {}（涉及 {} 条，单条最多 {}）\n保留首选 + 一个组合位：目标 Top-1 {}、Top-3 {}、Top-5 {}、Top-{frontier_limit} {}；新增可见 {}，原可见丢失 {}；共同可见中升位 {}、不变 {}、降位 {}；普通首选变化 {}；挤出核心候选 {}（涉及 {} 条，单条最多 {}）\n保留首选 + 两个组合位：目标 Top-1 {}、Top-3 {}、Top-5 {}、Top-{frontier_limit} {}；新增可见 {}，原可见丢失 {}；共同可见中升位 {}、不变 {}、降位 {}；普通首选变化 {}；挤出核心候选 {}（涉及 {} 条，单条最多 {}）\n选择规则不读取解码结果；保留首选结果只是冻结候选上的审计模拟；跨来源原始权重未比较。\n",
        core_entries.len(),
        core_payload_sha256,
        supplemental_entries.len(),
        supplemental_payload_sha256,
        heldout_corpus_sha256,
        corpus.stats.sentences,
        stats.source_windows,
        stats.han_length_eligible,
        stats.one_supplemental_word_eligible,
        stats.whole_phrase_collisions,
        stats.whole_code_collisions,
        stats.sentence_shape_representatives,
        stats.selected,
        stats.selected_two_token,
        stats.selected_three_token,
        stats.selected_supplemental_first,
        stats.selected_supplemental_middle,
        stats.selected_supplemental_last,
        report.core.at_one,
        report.core.at_three,
        report.core.at_five,
        report.core.visible,
        report.layered.at_one,
        report.layered.at_three,
        report.layered.at_five,
        report.layered.visible,
        report.preserve_core_top.at_one,
        report.preserve_core_top.at_three,
        report.preserve_core_top.at_five,
        report.preserve_core_top.visible,
        report.preserve_core_top_two.at_one,
        report.preserve_core_top_two.at_three,
        report.preserve_core_top_two.at_five,
        report.preserve_core_top_two.visible,
        report.newly_visible,
        report.lost_visible,
        report.rank_improved,
        report.rank_unchanged,
        report.rank_worsened,
        report.preserve_core_top_newly_visible,
        report.preserve_core_top_lost_visible,
        report.preserve_core_top_two_newly_visible,
        report.preserve_core_top_two_lost_visible,
        report.target_composition_rank_one,
        report.target_composition_rank_two,
        report.target_composition_rank_three_or_four,
        report.target_composition_rank_five_to_eight,
        report.target_composition_outside_eight,
        report.candidate_order_changed,
        report.target_promoted_to_top_one,
        report.non_target_top_one_changes,
        report.core_exact_top_preserved,
        report.core_exact_top_changed,
        report.missing_supplemental_edge_depth,
        report.missing_core_edge_depth,
        report.missing_competing_composition,
        control_selection_stats.source_windows,
        control_selection_stats.exact_word_coverable,
        control_selection_stats.sentence_representatives,
        control_selection_stats.whole_phrase_collisions,
        control_selection_stats.whole_code_collisions,
        control_selection_stats.selected,
        control_report.core.at_one,
        control_report.core.at_three,
        control_report.core.at_five,
        control_report.core.visible,
        control_report.layered.target.at_one,
        control_report.layered.target.at_three,
        control_report.layered.target.at_five,
        control_report.layered.target.visible,
        control_report.layered.target_newly_visible,
        control_report.layered.target_lost_visible,
        control_report.layered.target_rank_improved,
        control_report.layered.target_rank_unchanged,
        control_report.layered.target_rank_worsened,
        control_report.layered.top_one_changed,
        control_report.layered.core_candidates_evicted,
        control_report.layered.samples_evicting_core_candidates,
        control_report.layered.maximum_core_candidates_evicted,
        control_report.preserve_core_top_one_slot.target.at_one,
        control_report.preserve_core_top_one_slot.target.at_three,
        control_report.preserve_core_top_one_slot.target.at_five,
        control_report.preserve_core_top_one_slot.target.visible,
        control_report
            .preserve_core_top_one_slot
            .target_newly_visible,
        control_report
            .preserve_core_top_one_slot
            .target_lost_visible,
        control_report
            .preserve_core_top_one_slot
            .target_rank_improved,
        control_report
            .preserve_core_top_one_slot
            .target_rank_unchanged,
        control_report
            .preserve_core_top_one_slot
            .target_rank_worsened,
        control_report.preserve_core_top_one_slot.top_one_changed,
        control_report
            .preserve_core_top_one_slot
            .core_candidates_evicted,
        control_report
            .preserve_core_top_one_slot
            .samples_evicting_core_candidates,
        control_report
            .preserve_core_top_one_slot
            .maximum_core_candidates_evicted,
        control_report.preserve_core_top_two_slots.target.at_one,
        control_report.preserve_core_top_two_slots.target.at_three,
        control_report.preserve_core_top_two_slots.target.at_five,
        control_report.preserve_core_top_two_slots.target.visible,
        control_report
            .preserve_core_top_two_slots
            .target_newly_visible,
        control_report
            .preserve_core_top_two_slots
            .target_lost_visible,
        control_report
            .preserve_core_top_two_slots
            .target_rank_improved,
        control_report
            .preserve_core_top_two_slots
            .target_rank_unchanged,
        control_report
            .preserve_core_top_two_slots
            .target_rank_worsened,
        control_report.preserve_core_top_two_slots.top_one_changed,
        control_report
            .preserve_core_top_two_slots
            .core_candidates_evicted,
        control_report
            .preserve_core_top_two_slots
            .samples_evicting_core_candidates,
        control_report
            .preserve_core_top_two_slots
            .maximum_core_candidates_evicted,
    );
    output.push_str("\n单组合位排序冻结对照\n");
    for (label, profile) in order_reports {
        writeln!(
            output,
            "{label}：补充目标 Top-1 {}、Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}；全核心目标 Top-{frontier_limit} {}，原可见丢失 {}、降位 {}，普通首选变化 {}，挤出核心候选 {}",
            profile.supplemental_probes.target.at_one,
            profile.supplemental_probes.target.at_three,
            profile.supplemental_probes.target.at_five,
            profile.supplemental_probes.target.visible,
            profile.supplemental_probes.target_newly_visible,
            profile.supplemental_probes.target_lost_visible,
            profile.core_controls.target.visible,
            profile.core_controls.target_lost_visible,
            profile.core_controls.target_rank_worsened,
            profile.core_controls.top_one_changed,
            profile.core_controls.core_candidates_evicted,
        )?;
    }
    output.push_str("排序对照只交换明确的结构或层内名次字段，不比较跨来源原始权重。\n");
    output.push_str(&character_context_output);
    output.push_str("本次操作：只读\n");
    Ok(output)
}

fn select_core_composition_controls(
    corpus: &ziranma_core::UdCorpus,
    core: &[ziranma_core::LexiconEntry],
    supplemental: &[ziranma_core::LexiconEntry],
    limit: usize,
) -> (
    Vec<ContinuousCompositionProbe>,
    CoreCompositionControlSelectionStats,
) {
    let selection = select_public_continuous_composition_cases(corpus, core, usize::MAX);
    let core_texts = core
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let supplemental_texts = supplemental
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let core_codes = core
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let supplemental_codes = supplemental
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let mut stats = CoreCompositionControlSelectionStats {
        source_windows: selection.stats.source_windows,
        exact_word_coverable: selection.stats.exact_word_coverable,
        sentence_representatives: selection.stats.sentence_representatives,
        ..CoreCompositionControlSelectionStats::default()
    };
    let mut probes = Vec::with_capacity(limit.min(selection.probes.len()));
    for probe in selection.probes {
        if core_texts.contains(probe.expected_text.as_str())
            || supplemental_texts.contains(probe.expected_text.as_str())
        {
            stats.whole_phrase_collisions += 1;
            continue;
        }
        if core_codes.contains(probe.full_observed.as_str())
            || supplemental_codes.contains(probe.full_observed.as_str())
        {
            stats.whole_code_collisions += 1;
            continue;
        }
        probes.push(probe);
        if probes.len() == limit {
            break;
        }
    }
    stats.selected = probes.len();
    (probes, stats)
}

fn evaluate_core_composition_controls(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    probes: &[ContinuousCompositionProbe],
    frontier_limit: usize,
) -> Result<CoreCompositionControlReport, Box<dyn std::error::Error>> {
    let config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let mut report = CoreCompositionControlReport {
        total: probes.len(),
        ..CoreCompositionControlReport::default()
    };
    for probe in probes {
        let code = probe.full_observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let layered = layered_candidate_texts(core, supplemental, code, frontier_limit, config)?;
        let compositions = supplemental_complete_composition_texts(core, supplemental, code, 8)?;
        let preserve_core_top_one_slot = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let preserve_core_top_two_slots = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            2,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        report.core.observe(core_rank);
        observe_composition_control_strategy(
            &mut report.layered,
            &core_candidates,
            &layered,
            core_rank,
            &probe.expected_text,
        );
        observe_composition_control_strategy(
            &mut report.preserve_core_top_one_slot,
            &core_candidates,
            &preserve_core_top_one_slot,
            core_rank,
            &probe.expected_text,
        );
        observe_composition_control_strategy(
            &mut report.preserve_core_top_two_slots,
            &core_candidates,
            &preserve_core_top_two_slots,
            core_rank,
            &probe.expected_text,
        );
    }
    Ok(report)
}

fn evaluate_composition_order_profile(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    supplemental_probes: &[PublicSupplementalCompositionProbe],
    core_controls: &[ContinuousCompositionProbe],
    frontier_limit: usize,
    order: SupplementalCompositionOrder,
) -> Result<CompositionOrderProfileReport, Box<dyn std::error::Error>> {
    let mut report = CompositionOrderProfileReport::default();
    for probe in supplemental_probes {
        let code = probe.observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let compositions =
            supplemental_complete_composition_texts_with_order(core, supplemental, code, 8, order)?;
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.supplemental_probes,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    for probe in core_controls {
        let code = probe.full_observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let compositions =
            supplemental_complete_composition_texts_with_order(core, supplemental, code, 8, order)?;
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.core_controls,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    Ok(report)
}

fn evaluate_character_composition_profile(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    supplemental_probes: &[PublicSupplementalCompositionProbe],
    core_controls: &[ContinuousCompositionProbe],
    frontier_limit: usize,
    language_model: &CharacterBigramLanguageModel,
    profile: CharacterCompositionProfile,
) -> Result<CompositionOrderProfileReport, Box<dyn std::error::Error>> {
    let mut report = CompositionOrderProfileReport::default();
    for probe in supplemental_probes {
        let code = probe.observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let compositions =
            character_context_composition_texts(core, supplemental, code, language_model, profile)?;
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.supplemental_probes,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    for probe in core_controls {
        let code = probe.full_observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let compositions =
            character_context_composition_texts(core, supplemental, code, language_model, profile)?;
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.core_controls,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    Ok(report)
}

fn evaluate_word_boundary_composition_profile(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    supplemental_probes: &[PublicSupplementalCompositionProbe],
    core_controls: &[ContinuousCompositionProbe],
    frontier_limit: usize,
    model: &PublicWordBoundaryModel,
    profile: WordBoundaryCompositionProfile,
) -> Result<CompositionOrderProfileReport, Box<dyn std::error::Error>> {
    let mut report = CompositionOrderProfileReport::default();
    for probe in supplemental_probes {
        let code = probe.observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let structural = supplemental_complete_composition_texts_with_order(
            core,
            supplemental,
            code,
            8,
            SupplementalCompositionOrder::StructuralV1,
        )?;
        let compositions =
            word_boundary_composition_texts(core, supplemental, code, model, profile)?;
        report.supplemental_composition_choice_changed +=
            usize::from(structural.first() != compositions.first());
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.supplemental_probes,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    for probe in core_controls {
        let code = probe.full_observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let structural = supplemental_complete_composition_texts_with_order(
            core,
            supplemental,
            code,
            8,
            SupplementalCompositionOrder::StructuralV1,
        )?;
        let compositions =
            word_boundary_composition_texts(core, supplemental, code, model, profile)?;
        report.core_composition_choice_changed +=
            usize::from(structural.first() != compositions.first());
        let candidate = merge_compositions_after_core_top(
            &core_candidates,
            &compositions,
            code,
            1,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        observe_composition_control_strategy(
            &mut report.core_controls,
            &core_candidates,
            &candidate,
            core_rank,
            &probe.expected_text,
        );
    }
    Ok(report)
}

fn word_boundary_composition_texts(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    model: &PublicWordBoundaryModel,
    profile: WordBoundaryCompositionProfile,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut compositions = supplemental_complete_compositions_with_order(
        core,
        supplemental,
        code,
        8,
        SupplementalCompositionOrder::StructuralV1,
    )?;
    if compositions.len() >= 2 && profile.search_depth > 1 {
        let depth = profile.search_depth.min(compositions.len());
        let scores = compositions
            .iter()
            .take(depth)
            .map(|candidate| model.score(candidate))
            .collect::<Vec<_>>();
        let leader_average = scores[0].log_probability / scores[0].syllable_count as f64;
        let challenger = scores
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, score)| {
                score.observed_syllables * 1_000
                    >= profile.minimum_observed_ratio_milli * score.syllable_count
            })
            .max_by(|(left_index, left), (right_index, right)| {
                let left_average = left.log_probability / left.syllable_count as f64;
                let right_average = right.log_probability / right.syllable_count as f64;
                left_average
                    .total_cmp(&right_average)
                    .then_with(|| right_index.cmp(left_index))
            });
        if let Some((index, score)) = challenger {
            let challenger_average = score.log_probability / score.syllable_count as f64;
            if challenger_average - leader_average >= profile.minimum_average_gain {
                let challenger = compositions.remove(index);
                compositions.insert(0, challenger);
            }
        }
    }
    Ok(compositions
        .into_iter()
        .map(|candidate| candidate.text().to_owned())
        .collect())
}

fn character_context_composition_texts(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    language_model: &CharacterBigramLanguageModel,
    profile: CharacterCompositionProfile,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut compositions = supplemental_complete_composition_texts_with_order(
        core,
        supplemental,
        code,
        8,
        SupplementalCompositionOrder::StructuralV1,
    )?;
    if compositions.len() < 2 || profile.search_depth <= 1 {
        return Ok(compositions);
    }
    let depth = profile.search_depth.min(compositions.len());
    let scores = compositions
        .iter()
        .take(depth)
        .map(|candidate| language_model.score_text(candidate))
        .collect::<Vec<_>>();
    let leader_average = scores[0].log_probability / scores[0].pair_count as f64;
    let challenger = scores
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, score)| {
            score.observed_pairs * 1_000 >= profile.minimum_observed_ratio_milli * score.pair_count
        })
        .max_by(|(left_index, left), (right_index, right)| {
            let left_average = left.log_probability / left.pair_count as f64;
            let right_average = right.log_probability / right.pair_count as f64;
            left_average
                .total_cmp(&right_average)
                .then_with(|| right_index.cmp(left_index))
        });
    if let Some((index, score)) = challenger {
        let challenger_average = score.log_probability / score.pair_count as f64;
        if challenger_average - leader_average >= profile.minimum_average_gain {
            let challenger = compositions.remove(index);
            compositions.insert(0, challenger);
        }
    }
    Ok(compositions)
}

fn composition_profile_is_safe(report: &CompositionOrderProfileReport) -> bool {
    report.supplemental_probes.target_lost_visible == 0
        && report.core_controls.target_lost_visible == 0
        && report.supplemental_probes.top_one_changed == 0
        && report.core_controls.top_one_changed == 0
}

fn composition_profile_precedes(
    challenger: &CompositionOrderProfileReport,
    current: &CompositionOrderProfileReport,
) -> bool {
    composition_profile_is_safe(challenger)
        .cmp(&composition_profile_is_safe(current))
        .then_with(|| {
            challenger
                .supplemental_probes
                .target
                .visible
                .cmp(&current.supplemental_probes.target.visible)
        })
        .then_with(|| {
            challenger
                .supplemental_probes
                .target
                .at_five
                .cmp(&current.supplemental_probes.target.at_five)
        })
        .then_with(|| {
            challenger
                .supplemental_probes
                .target
                .at_three
                .cmp(&current.supplemental_probes.target.at_three)
        })
        .then_with(|| {
            current
                .core_controls
                .target_rank_worsened
                .cmp(&challenger.core_controls.target_rank_worsened)
        })
        .then_with(|| {
            current
                .core_controls
                .core_candidates_evicted
                .cmp(&challenger.core_controls.core_candidates_evicted)
        })
        .is_gt()
}

#[allow(clippy::too_many_arguments)]
fn audit_character_composition_context(
    fit_corpus_path: &Path,
    heldout_corpus_text: &str,
    core_entries: &[ziranma_core::LexiconEntry],
    supplemental_entries: &[ziranma_core::LexiconEntry],
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    heldout_supplemental_probes: &[PublicSupplementalCompositionProbe],
    heldout_core_controls: &[ContinuousCompositionProbe],
    frontier_limit: usize,
    sample_limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let fit_corpus_text = read_explicit_text(
        fit_corpus_path,
        "public composition fit corpus",
        MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES,
    )?;
    if fit_corpus_text == heldout_corpus_text {
        return Err("composition fit corpus must be distinct from held-out corpus".into());
    }
    let fit_corpus = parse_ud_conllu(&fit_corpus_text)?;
    let fit_corpus_sha256 = candidate_sha256_hex(fit_corpus_text.as_bytes());
    let heldout_corpus_sha256 = candidate_sha256_hex(heldout_corpus_text.as_bytes());
    let mut boundary_lexicon = core_entries.to_vec();
    boundary_lexicon.extend_from_slice(supplemental_entries);
    let word_training = select_public_bigram_training_sequences(&fit_corpus, &boundary_lexicon);
    let word_boundary_model = PublicWordBoundaryModel::from_sequences(&word_training.sequences)
        .ok_or("public fit corpus produced no word-boundary training tokens")?;
    let training = select_public_character_training_texts(&fit_corpus);
    let language_model = CharacterBigramLanguageModel::from_text_sequences(&training.sequences)?;
    let fit_supplemental = select_public_supplemental_composition_cases(
        &fit_corpus,
        core_entries,
        supplemental_entries,
        sample_limit,
    );
    let (fit_core_controls, fit_control_stats) = select_core_composition_controls(
        &fit_corpus,
        core_entries,
        supplemental_entries,
        sample_limit,
    );
    if fit_supplemental.probes.is_empty() || fit_core_controls.is_empty() {
        return Err("public fit corpus produced no eligible composition probes".into());
    }

    let mut fit_reports = Vec::with_capacity(CHARACTER_COMPOSITION_PROFILES.len());
    for profile in CHARACTER_COMPOSITION_PROFILES {
        fit_reports.push(evaluate_character_composition_profile(
            core,
            supplemental,
            &fit_supplemental.probes,
            &fit_core_controls,
            frontier_limit,
            &language_model,
            profile,
        )?);
    }
    let mut selected_index = 0;
    for index in 1..fit_reports.len() {
        if composition_profile_precedes(&fit_reports[index], &fit_reports[selected_index]) {
            selected_index = index;
        }
    }
    let baseline_heldout = evaluate_character_composition_profile(
        core,
        supplemental,
        heldout_supplemental_probes,
        heldout_core_controls,
        frontier_limit,
        &language_model,
        CHARACTER_COMPOSITION_PROFILES[0],
    )?;
    let selected_heldout = if selected_index == 0 {
        baseline_heldout
    } else {
        evaluate_character_composition_profile(
            core,
            supplemental,
            heldout_supplemental_probes,
            heldout_core_controls,
            frontier_limit,
            &language_model,
            CHARACTER_COMPOSITION_PROFILES[selected_index],
        )?
    };

    let mut word_fit_reports = Vec::with_capacity(WORD_BOUNDARY_COMPOSITION_PROFILES.len());
    for profile in WORD_BOUNDARY_COMPOSITION_PROFILES {
        word_fit_reports.push(evaluate_word_boundary_composition_profile(
            core,
            supplemental,
            &fit_supplemental.probes,
            &fit_core_controls,
            frontier_limit,
            &word_boundary_model,
            profile,
        )?);
    }
    let mut word_selected_index = 0;
    for index in 1..word_fit_reports.len() {
        if composition_profile_precedes(
            &word_fit_reports[index],
            &word_fit_reports[word_selected_index],
        ) {
            word_selected_index = index;
        }
    }
    let word_baseline_heldout = evaluate_word_boundary_composition_profile(
        core,
        supplemental,
        heldout_supplemental_probes,
        heldout_core_controls,
        frontier_limit,
        &word_boundary_model,
        WORD_BOUNDARY_COMPOSITION_PROFILES[0],
    )?;
    let word_selected_heldout = if word_selected_index == 0 {
        word_baseline_heldout
    } else {
        evaluate_word_boundary_composition_profile(
            core,
            supplemental,
            heldout_supplemental_probes,
            heldout_core_controls,
            frontier_limit,
            &word_boundary_model,
            WORD_BOUNDARY_COMPOSITION_PROFILES[word_selected_index],
        )?
    };

    let training_stats = training.stats;
    let model_stats = language_model.stats();
    let mut output = format!(
        "\n公开字境拟合 / 保留评测\n语料指纹：拟合 SHA-256 {}；保留 SHA-256 {}\n拟合语料：{} 句；保留汉字序列 {}，字符 {}；模型转移类型 {}\n拟合样本：补充组合 {}，全核心负对照 {}（句代表 {}）\n",
        fit_corpus_sha256,
        heldout_corpus_sha256,
        fit_corpus.stats.sentences,
        training_stats.training_sequences,
        training_stats.training_characters,
        model_stats.observed_pair_types,
        fit_supplemental.stats.selected,
        fit_control_stats.selected,
        fit_control_stats.sentence_representatives,
    );
    for (profile, report) in CHARACTER_COMPOSITION_PROFILES.iter().zip(&fit_reports) {
        writeln!(
            output,
            "拟合 {}：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}",
            profile.label,
            report.supplemental_probes.target.at_three,
            report.supplemental_probes.target.at_five,
            report.supplemental_probes.target.visible,
            report.supplemental_probes.target_newly_visible,
            report.supplemental_probes.target_lost_visible,
            report.core_controls.target.visible,
            report.core_controls.target_lost_visible,
            report.core_controls.target_rank_worsened,
            report.core_controls.core_candidates_evicted,
        )?;
    }
    let selected_profile = CHARACTER_COMPOSITION_PROFILES[selected_index];
    writeln!(
        output,
        "拟合选择：{}；只按安全、补充 Top-{frontier_limit}/Top-5/Top-3、负对照降位与挤出依次比较，平局保留更早的保守档。",
        selected_profile.label,
    )?;
    writeln!(
        output,
        "保留评测结构基线：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}",
        baseline_heldout.supplemental_probes.target.at_three,
        baseline_heldout.supplemental_probes.target.at_five,
        baseline_heldout.supplemental_probes.target.visible,
        baseline_heldout.supplemental_probes.target_newly_visible,
        baseline_heldout.supplemental_probes.target_lost_visible,
        baseline_heldout.core_controls.target.visible,
        baseline_heldout.core_controls.target_lost_visible,
        baseline_heldout.core_controls.target_rank_worsened,
        baseline_heldout.core_controls.core_candidates_evicted,
    )?;
    writeln!(
        output,
        "保留评测拟合选择：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}",
        selected_heldout.supplemental_probes.target.at_three,
        selected_heldout.supplemental_probes.target.at_five,
        selected_heldout.supplemental_probes.target.visible,
        selected_heldout.supplemental_probes.target_newly_visible,
        selected_heldout.supplemental_probes.target_lost_visible,
        selected_heldout.core_controls.target.visible,
        selected_heldout.core_controls.target_lost_visible,
        selected_heldout.core_controls.target_rank_worsened,
        selected_heldout.core_controls.core_candidates_evicted,
    )?;
    output.push_str(
        "字境只在结构候选前八中允许一个具有足够公开转移覆盖和平均对数概率增益的挑战者；不创建路径，不读取保留答案来选档。\n",
    );
    let word_training_stats = word_training.stats;
    writeln!(
        output,
        "\n公开词界拟合 / 保留评测\n训练：序列 {}，词次 {}（源词 {}，逐字回退 {}），可见词型 {}",
        word_training_stats.training_sequences,
        word_training_stats.training_words,
        word_training_stats.exact_token_uses,
        word_training_stats.character_fallback_uses,
        word_boundary_model.observed_token_types(),
    )?;
    for (profile, report) in WORD_BOUNDARY_COMPOSITION_PROFILES
        .iter()
        .zip(&word_fit_reports)
    {
        writeln!(
            output,
            "拟合 {}：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}、组合首位变化 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}、组合首位变化 {}",
            profile.label,
            report.supplemental_probes.target.at_three,
            report.supplemental_probes.target.at_five,
            report.supplemental_probes.target.visible,
            report.supplemental_probes.target_newly_visible,
            report.supplemental_probes.target_lost_visible,
            report.supplemental_composition_choice_changed,
            report.core_controls.target.visible,
            report.core_controls.target_lost_visible,
            report.core_controls.target_rank_worsened,
            report.core_controls.core_candidates_evicted,
            report.core_composition_choice_changed,
        )?;
    }
    let selected_word_profile = WORD_BOUNDARY_COMPOSITION_PROFILES[word_selected_index];
    writeln!(
        output,
        "拟合选择：{}；沿用安全、补充 Top-{frontier_limit}/Top-5/Top-3、负对照降位与挤出次序，平局保留更早的保守档。",
        selected_word_profile.label,
    )?;
    writeln!(
        output,
        "保留评测结构基线：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}、组合首位变化 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}、组合首位变化 {}",
        word_baseline_heldout.supplemental_probes.target.at_three,
        word_baseline_heldout.supplemental_probes.target.at_five,
        word_baseline_heldout.supplemental_probes.target.visible,
        word_baseline_heldout
            .supplemental_probes
            .target_newly_visible,
        word_baseline_heldout
            .supplemental_probes
            .target_lost_visible,
        word_baseline_heldout.supplemental_composition_choice_changed,
        word_baseline_heldout.core_controls.target.visible,
        word_baseline_heldout.core_controls.target_lost_visible,
        word_baseline_heldout.core_controls.target_rank_worsened,
        word_baseline_heldout.core_controls.core_candidates_evicted,
        word_baseline_heldout.core_composition_choice_changed,
    )?;
    writeln!(
        output,
        "保留评测拟合选择：补充 Top-3 {}、Top-5 {}、Top-{frontier_limit} {}，新增可见 {}、原可见丢失 {}、组合首位变化 {}；负对照 Top-{frontier_limit} {}，丢失 {}、降位 {}、挤出 {}、组合首位变化 {}",
        word_selected_heldout.supplemental_probes.target.at_three,
        word_selected_heldout.supplemental_probes.target.at_five,
        word_selected_heldout.supplemental_probes.target.visible,
        word_selected_heldout
            .supplemental_probes
            .target_newly_visible,
        word_selected_heldout
            .supplemental_probes
            .target_lost_visible,
        word_selected_heldout.supplemental_composition_choice_changed,
        word_selected_heldout.core_controls.target.visible,
        word_selected_heldout.core_controls.target_lost_visible,
        word_selected_heldout.core_controls.target_rank_worsened,
        word_selected_heldout.core_controls.core_candidates_evicted,
        word_selected_heldout.core_composition_choice_changed,
    )?;
    output.push_str(
        "词界模型只累计拟合语料的公开分词词次，并按每音节平均词概率比较已生成路径；不读取保留答案，不创建路径，不接入运行时。\n",
    );
    Ok(output)
}

fn observe_composition_control_strategy(
    report: &mut CompositionControlStrategyReport,
    core: &[String],
    candidate: &[String],
    core_rank: Option<usize>,
    expected: &str,
) {
    let candidate_rank = candidate_rank(candidate, expected);
    report.target.observe(candidate_rank);
    match (core_rank, candidate_rank) {
        (None, Some(_)) => report.target_newly_visible += 1,
        (Some(_), None) => report.target_lost_visible += 1,
        (Some(core_rank), Some(candidate_rank)) if candidate_rank < core_rank => {
            report.target_rank_improved += 1;
        }
        (Some(core_rank), Some(candidate_rank)) if candidate_rank > core_rank => {
            report.target_rank_worsened += 1;
        }
        (Some(_), Some(_)) => report.target_rank_unchanged += 1,
        (None, None) => {}
    }
    report.top_one_changed += usize::from(core.first() != candidate.first());
    let evicted = core
        .iter()
        .filter(|core_candidate| !candidate.contains(core_candidate))
        .count();
    if evicted != 0 {
        report.samples_evicting_core_candidates += 1;
        report.core_candidates_evicted += evicted;
        report.maximum_core_candidates_evicted =
            report.maximum_core_candidates_evicted.max(evicted);
    }
}

fn evaluate_candidate_layer_compositions(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    probes: &[PublicSupplementalCompositionProbe],
    frontier_limit: usize,
) -> Result<CompositionAuditReport, Box<dyn std::error::Error>> {
    let config = SupplementalCandidateLayerConfig {
        exact_promotions: 1,
    };
    let mut report = CompositionAuditReport {
        total: probes.len(),
        ..CompositionAuditReport::default()
    };
    for probe in probes {
        let code = probe.observed.as_str();
        let core_candidates = core.candidate_texts(code, frontier_limit)?;
        let layered = layered_candidate_texts(core, supplemental, code, frontier_limit, config)?;
        let composition_candidates =
            supplemental_complete_composition_texts(core, supplemental, code, 8)?;
        let preserve_core_top =
            preserve_core_primary_top(&core_candidates, &layered, code, frontier_limit);
        let preserve_core_top_two = merge_compositions_after_core_top(
            &core_candidates,
            &composition_candidates,
            code,
            2,
            frontier_limit,
        );
        let core_rank = candidate_rank(&core_candidates, &probe.expected_text);
        let layered_rank = candidate_rank(&layered, &probe.expected_text);
        let preserve_core_top_rank = candidate_rank(&preserve_core_top, &probe.expected_text);
        let preserve_core_top_two_rank =
            candidate_rank(&preserve_core_top_two, &probe.expected_text);
        let composition_rank = candidate_rank(&composition_candidates, &probe.expected_text);
        report.core.observe(core_rank);
        report.layered.observe(layered_rank);
        report.preserve_core_top.observe(preserve_core_top_rank);
        report
            .preserve_core_top_two
            .observe(preserve_core_top_two_rank);
        match composition_rank {
            Some(1) => report.target_composition_rank_one += 1,
            Some(2) => report.target_composition_rank_two += 1,
            Some(3 | 4) => report.target_composition_rank_three_or_four += 1,
            Some(5..=8) => report.target_composition_rank_five_to_eight += 1,
            Some(_) | None => report.target_composition_outside_eight += 1,
        }
        match (core_rank, layered_rank) {
            (None, Some(_)) => report.newly_visible += 1,
            (Some(_), None) => report.lost_visible += 1,
            (Some(core_rank), Some(layered_rank)) if layered_rank < core_rank => {
                report.rank_improved += 1;
            }
            (Some(core_rank), Some(layered_rank)) if layered_rank > core_rank => {
                report.rank_worsened += 1;
            }
            (Some(_), Some(_)) => report.rank_unchanged += 1,
            (None, None) => {}
        }
        match (core_rank, preserve_core_top_rank) {
            (None, Some(_)) => report.preserve_core_top_newly_visible += 1,
            (Some(_), None) => report.preserve_core_top_lost_visible += 1,
            _ => {}
        }
        match (core_rank, preserve_core_top_two_rank) {
            (None, Some(_)) => report.preserve_core_top_two_newly_visible += 1,
            (Some(_), None) => report.preserve_core_top_two_lost_visible += 1,
            _ => {}
        }
        if core_candidates != layered {
            report.candidate_order_changed += 1;
        }
        if core_candidates.first() != layered.first() {
            if layered.first() == Some(&probe.expected_text) {
                report.target_promoted_to_top_one += 1;
            } else {
                report.non_target_top_one_changes += 1;
            }
        }
        if let Some(core_exact_top) = core.exact_full_code_texts(code, 1)?.first() {
            if layered.first() == Some(core_exact_top) {
                report.core_exact_top_preserved += 1;
            } else {
                report.core_exact_top_changed += 1;
            }
        }
        if layered_rank.is_none() {
            match classify_missing_composition(core, supplemental, probe)? {
                MissingCompositionReason::SupplementalEdgeDepth => {
                    report.missing_supplemental_edge_depth += 1;
                }
                MissingCompositionReason::CoreEdgeDepth => {
                    report.missing_core_edge_depth += 1;
                }
                MissingCompositionReason::CompetingComposition => {
                    report.missing_competing_composition += 1;
                }
            }
            if report.missing_probe_ids.len() < 12 {
                report.missing_probe_ids.push(probe.id.clone());
            }
        }
    }
    Ok(report)
}

fn preserve_core_primary_top(
    core: &[String],
    layered: &[String],
    code: &str,
    frontier_limit: usize,
) -> Vec<String> {
    let Some(core_top) = core.first().filter(|candidate| candidate.as_str() != code) else {
        return layered.to_vec();
    };
    std::iter::once(core_top.clone())
        .chain(
            layered
                .iter()
                .filter(|candidate| *candidate != core_top)
                .cloned(),
        )
        .take(frontier_limit)
        .collect()
}

fn merge_compositions_after_core_top(
    core: &[String],
    compositions: &[String],
    code: &str,
    composition_slots: usize,
    frontier_limit: usize,
) -> Vec<String> {
    let mut merged = Vec::with_capacity(frontier_limit);
    let mut push_unique = |candidate: &String| {
        if merged.len() < frontier_limit && !merged.contains(candidate) {
            merged.push(candidate.clone());
            true
        } else {
            false
        }
    };
    let core_top_is_preserved = core.first().is_some_and(|candidate| candidate != code);
    if core_top_is_preserved {
        let core_top = &core[0];
        push_unique(core_top);
    }
    let mut admitted_compositions = 0;
    for composition in compositions {
        if admitted_compositions == composition_slots {
            break;
        }
        if push_unique(composition) {
            admitted_compositions += 1;
        }
    }
    for candidate in core.iter().skip(usize::from(core_top_is_preserved)) {
        push_unique(candidate);
    }
    merged
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissingCompositionReason {
    SupplementalEdgeDepth,
    CoreEdgeDepth,
    CompetingComposition,
}

fn classify_missing_composition(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    probe: &PublicSupplementalCompositionProbe,
) -> Result<MissingCompositionReason, Box<dyn std::error::Error>> {
    let supplemental_index = probe.supplemental_segment_index;
    let supplemental_text = &probe.expected_segments[supplemental_index];
    let supplemental_code = probe.segment_codes[supplemental_index].as_str();
    let core_exact = core.exact_full_code_texts(supplemental_code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
    let supplemental_rank = supplemental
        .exact_full_code_texts(supplemental_code, MAX_CANDIDATE_SNAPSHOT_RANK)?
        .into_iter()
        .filter(|candidate| !core_exact.contains(candidate))
        .position(|candidate| candidate == supplemental_text.as_str())
        .map(|index| index + 1);
    if supplemental_rank.is_none_or(|rank| rank > SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH) {
        return Ok(MissingCompositionReason::SupplementalEdgeDepth);
    }

    for (index, (segment, code)) in probe
        .expected_segments
        .iter()
        .zip(&probe.segment_codes)
        .enumerate()
    {
        if index == supplemental_index {
            continue;
        }
        let rank = core
            .exact_full_code_texts(code.as_str(), MAX_CANDIDATE_SNAPSHOT_RANK)?
            .iter()
            .position(|candidate| candidate == segment)
            .map(|index| index + 1);
        if rank.is_none_or(|rank| rank > SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH) {
            return Ok(MissingCompositionReason::CoreEdgeDepth);
        }
    }
    Ok(MissingCompositionReason::CompetingComposition)
}

fn candidate_rank(candidates: &[String], expected: &str) -> Option<usize> {
    candidates
        .iter()
        .position(|candidate| candidate == expected)
        .map(|index| index + 1)
}

fn benchmark_candidate_layers(
    core_payload: &Path,
    supplemental_payload: &Path,
    repetitions: usize,
    exact_promotions: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        return Err("layer-benchmark must run from a release build".into());
    }
    let core_text = read_explicit_text(
        core_payload,
        "core public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let supplemental_text = read_explicit_text(
        supplemental_payload,
        "supplemental public candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let core_started = Instant::now();
    let core = snapshot_from_payload("layer-benchmark-core-v1", &core_text)?;
    let core_build = core_started.elapsed();
    let supplemental_started = Instant::now();
    let supplemental =
        snapshot_from_payload("layer-benchmark-supplemental-v1", &supplemental_text)?;
    let supplemental_build = supplemental_started.elapsed();
    let supplemental_entries = parse_lexicon_tsv(&supplemental_text)?;
    let correction_audits =
        audit_four_character_correction_gate(&core, &supplemental, &supplemental_entries, 256)?;
    let short_correction_audits = audit_short_word_extra_key_correction_gate(
        &core,
        &supplemental,
        &supplemental_entries,
        256,
    )?;
    let config = SupplementalCandidateLayerConfig { exact_promotions };
    let codes = layer_benchmark_codes()?;
    let correction_codes = four_character_correction_benchmark_codes()?;
    let short_correction_codes = short_word_extra_key_correction_benchmark_codes()?;

    for code in &codes {
        black_box(core.candidate_texts(black_box(code), 6)?);
        black_box(layered_candidate_texts(
            &core,
            &supplemental,
            black_box(code),
            6,
            config,
        )?);
    }
    for code in &correction_codes {
        black_box(layered_four_character_correction_decision(
            &core,
            Some(&supplemental),
            black_box(code),
            1,
        )?);
    }
    for code in &short_correction_codes {
        black_box(layered_short_word_extra_key_correction_decision(
            &core,
            Some(&supplemental),
            black_box(code),
            1,
        )?);
    }

    let mut core_durations = Vec::with_capacity(repetitions * codes.len());
    let mut layered_durations = Vec::with_capacity(repetitions * codes.len());
    let mut correction_durations =
        Vec::with_capacity(repetitions.saturating_mul(correction_codes.len()));
    let mut short_correction_durations =
        Vec::with_capacity(repetitions.saturating_mul(short_correction_codes.len()));
    let mut checksum = 0usize;
    for _ in 0..repetitions {
        for code in &codes {
            let started = Instant::now();
            let candidates = core.candidate_texts(black_box(code), 6)?;
            core_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);

            let started = Instant::now();
            let candidates =
                layered_candidate_texts(&core, &supplemental, black_box(code), 6, config)?;
            layered_durations.push(started.elapsed());
            checksum = update_candidate_text_checksum(checksum, &candidates);
            black_box(candidates);
        }
        for code in &correction_codes {
            let started = Instant::now();
            let decision = layered_four_character_correction_decision(
                &core,
                Some(&supplemental),
                black_box(code),
                1,
            )?;
            correction_durations.push(started.elapsed());
            if let FourCharacterCorrectionDecision::Offer(offer) = &decision {
                checksum = update_candidate_text_checksum(
                    checksum,
                    &offer
                        .candidates
                        .iter()
                        .map(|candidate| candidate.text.clone())
                        .collect::<Vec<_>>(),
                );
            }
            black_box(decision);
        }
        for code in &short_correction_codes {
            let started = Instant::now();
            let decision = layered_short_word_extra_key_correction_decision(
                &core,
                Some(&supplemental),
                black_box(code),
                1,
            )?;
            short_correction_durations.push(started.elapsed());
            if let ShortWordExtraKeyCorrectionDecision::Offer(offer) = &decision {
                checksum = update_candidate_text_checksum(
                    checksum,
                    &offer
                        .candidates
                        .iter()
                        .map(|candidate| candidate.text.clone())
                        .collect::<Vec<_>>(),
                );
            }
            black_box(decision);
        }
    }

    let mut core_top_changes = 0;
    let mut query_order_changes = 0;
    for code in &codes {
        let core_candidates = core.candidate_texts(code, 6)?;
        let layered = layered_candidate_texts(&core, &supplemental, code, 6, config)?;
        if core_candidates != layered {
            query_order_changes += 1;
        }
        if let Some(core_top) = core.exact_full_code_texts(code, 1)?.first()
            && layered.first() != Some(core_top)
        {
            core_top_changes += 1;
        }
    }
    if core_top_changes != 0 {
        return Err("layer benchmark observed a changed core exact Top-1".into());
    }
    let core_latency = summarize_durations(&mut core_durations)
        .ok_or("layer benchmark produced no core samples")?;
    let layered_latency = summarize_durations(&mut layered_durations)
        .ok_or("layer benchmark produced no layered samples")?;
    let correction_latency = summarize_durations(&mut correction_durations)
        .ok_or("layer benchmark produced no correction samples")?;
    let short_correction_latency = summarize_durations(&mut short_correction_durations)
        .ok_or("layer benchmark produced no short correction samples")?;
    let median_delta_ms = layered_latency.median.as_secs_f64() * 1_000.0
        - core_latency.median.as_secs_f64() * 1_000.0;
    let mut correction_audit_summary = String::new();
    for edit in SyntheticFourCharacterEdit::ALL {
        let audit = correction_audits[edit.index()];
        writeln!(
            correction_audit_summary,
            "四字公开合成（{}）：样本 {}；恢复提示 {}，目标首项 {}，目标可见 {}，原码碰撞 {}，多码歧义 {}，无恢复 {}，错误目标码 {}，原码保护失败 {}",
            edit.label(),
            audit.samples,
            audit.offered,
            audit.target_first,
            audit.target_visible,
            audit.original_code_collision,
            audit.ambiguous_codes,
            audit.no_recovery,
            audit.wrong_intended_code,
            audit.exact_protection_failures,
        )?;
    }
    let mut short_correction_audit_summary = String::new();
    for (index, character_count) in [2, 3].into_iter().enumerate() {
        let audit = short_correction_audits[index];
        writeln!(
            short_correction_audit_summary,
            "短词公开合成（{character_count} 字邻键多按）：样本 {}；恢复提示 {}，目标首项 {}，目标可见 {}，两层冲突 {}，单层缺证据 {}，无恢复 {}，合成目标码偏离 {}，干净码保护失败 {}",
            audit.samples,
            audit.offered,
            audit.target_first,
            audit.target_visible,
            audit.conflicting_codes,
            audit.missing_independent_evidence,
            audit.no_recovery,
            audit.synthetic_target_code_misses,
            audit.clean_code_protection_failures,
        )?;
    }
    Ok(format!(
        "公开补充词层 release 热路径\n固定查询：{}；重复：{repetitions}；样本：{}\n索引构建：核心 {:.3} ms；补充 {:.3} ms\n核心基线：median {:.3} ms；p95 {:.3} ms；max {:.3} ms\n启用补充：median {:.3} ms；p95 {:.3} ms；max {:.3} ms\nmedian 差值：{median_delta_ms:+.3} ms\n四字纠错安全门：查询 {}；样本 {}；median {:.3} ms；p95 {:.3} ms；max {:.3} ms\n短词邻键多按门：查询 {}；样本 {}；median {:.3} ms；p95 {:.3} ms；max {:.3} ms\n{correction_audit_summary}{short_correction_audit_summary}候选顺序发生变化的固定查询：{query_order_changes}\n核心完整码首选变化：{core_top_changes}\n结果校验和：{checksum}\n口径：同机、预热、固定公开完整码与音节边界前缀；不是跨设备性能结论。\n本次操作：只读\n",
        codes.len(),
        core_latency.samples,
        duration_ms(core_build),
        duration_ms(supplemental_build),
        duration_ms(core_latency.median),
        duration_ms(core_latency.p95),
        duration_ms(core_latency.maximum),
        duration_ms(layered_latency.median),
        duration_ms(layered_latency.p95),
        duration_ms(layered_latency.maximum),
        correction_codes.len(),
        correction_latency.samples,
        duration_ms(correction_latency.median),
        duration_ms(correction_latency.p95),
        duration_ms(correction_latency.maximum),
        short_correction_codes.len(),
        short_correction_latency.samples,
        duration_ms(short_correction_latency.median),
        duration_ms(short_correction_latency.p95),
        duration_ms(short_correction_latency.maximum),
    ))
}

fn snapshot_from_payload(
    revision: &str,
    payload: &str,
) -> Result<CandidateSnapshot, Box<dyn std::error::Error>> {
    let expected_entry_count = parse_lexicon_tsv(payload)?.len();
    Ok(CandidateSnapshot::load(CandidateSnapshotDescriptor {
        schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
        revision,
        contains_private_text: false,
        lexicon_tsv: payload,
        expected_payload_bytes: payload.len(),
        expected_payload_fingerprint: candidate_payload_fingerprint(payload.as_bytes()),
        expected_entry_count,
    })?)
}

fn layer_benchmark_codes() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut codes = Vec::new();
    for phrase in [
        "shen me",
        "sheng mu",
        "yun mu",
        "du shu yu",
        "you qi shi",
        "sheng lue hao",
        "yi da chuan",
        "zhi jiao ju xing",
        "ou mu",
        "wai quan",
        "jian ru",
        "guang min xing",
        "bai kai rou sui",
        "zhe shu yu na yi zhong",
    ] {
        let encoded = encode_pinyin_phrase(phrase)?;
        let boundary = encoded.syllable_codes.len().div_ceil(2);
        let prefix = encoded
            .syllable_codes
            .iter()
            .take(boundary)
            .map(|code| code.as_str())
            .collect::<String>();
        for code in [prefix, encoded.full_code.as_str().to_owned()] {
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
    }
    Ok(codes)
}

fn four_character_correction_benchmark_codes() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut codes = Vec::new();
    for (sample_index, phrase) in ["bai kai rou sui", "zi xiang mao dun", "hua she tian zu"]
        .into_iter()
        .enumerate()
    {
        let encoded = encode_pinyin_phrase(phrase)?;
        for edit in SyntheticFourCharacterEdit::ALL {
            let observed =
                synthesize_four_character_edit(encoded.full_code.as_str(), edit, sample_index)
                    .ok_or("fixed four-character benchmark phrase could not be edited")?;
            if !codes.contains(&observed) {
                codes.push(observed);
            }
        }
    }
    Ok(codes)
}

fn short_word_extra_key_correction_benchmark_codes()
-> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut codes = vec!["kehyi".to_owned(), "xnkui".to_owned()];
    let intended = encode_pinyin_phrase("ma fan mao")?.full_code;
    let last = *intended
        .as_str()
        .as_bytes()
        .last()
        .ok_or("fixed short correction phrase has no code")?;
    let extra = (b'a'..=b'z')
        .find(|&key| are_qwerty_neighbors(last, key))
        .ok_or("fixed short correction phrase has no neighbor key")?;
    let mut observed = intended.as_str().to_owned();
    observed.push(extra as char);
    codes.push(observed);
    Ok(codes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyntheticFourCharacterEdit {
    NeighborSubstitution,
    AdjacentTransposition,
    MissingKey,
    ExtraKey,
}

impl SyntheticFourCharacterEdit {
    const ALL: [Self; 4] = [
        Self::NeighborSubstitution,
        Self::AdjacentTransposition,
        Self::MissingKey,
        Self::ExtraKey,
    ];

    const fn index(self) -> usize {
        match self {
            Self::NeighborSubstitution => 0,
            Self::AdjacentTransposition => 1,
            Self::MissingKey => 2,
            Self::ExtraKey => 3,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::NeighborSubstitution => "邻键替换",
            Self::AdjacentTransposition => "相邻换序",
            Self::MissingKey => "少按一键",
            Self::ExtraKey => "多按一键",
        }
    }
}

fn synthesize_four_character_edit(
    intended_code: &str,
    edit: SyntheticFourCharacterEdit,
    sample_index: usize,
) -> Option<String> {
    let mut observed = intended_code.as_bytes().to_vec();
    if observed.len() != 8 || !observed.iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    match edit {
        SyntheticFourCharacterEdit::NeighborSubstitution => {
            let index = sample_index % observed.len();
            let actual =
                (b'a'..=b'z').find(|&actual| are_qwerty_neighbors(observed[index], actual))?;
            observed[index] = actual;
        }
        SyntheticFourCharacterEdit::AdjacentTransposition => {
            let initial_start = sample_index % (observed.len() - 1);
            let start = (0..observed.len() - 1)
                .map(|offset| (initial_start + offset) % (observed.len() - 1))
                .find(|&start| observed[start] != observed[start + 1])?;
            observed.swap(start, start + 1);
        }
        SyntheticFourCharacterEdit::MissingKey => {
            observed.remove(sample_index % observed.len());
        }
        SyntheticFourCharacterEdit::ExtraKey => {
            let index = sample_index % (observed.len() + 1);
            observed.insert(index, b'a' + (sample_index % 26) as u8);
        }
    }
    String::from_utf8(observed).ok()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FourCharacterCorrectionAudit {
    samples: usize,
    offered: usize,
    target_first: usize,
    target_visible: usize,
    original_code_collision: usize,
    ambiguous_codes: usize,
    no_recovery: usize,
    wrong_intended_code: usize,
    exact_protection_failures: usize,
}

fn audit_four_character_correction_gate(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    supplemental_entries: &[ziranma_core::LexiconEntry],
    sample_limit: usize,
) -> Result<[FourCharacterCorrectionAudit; 4], Box<dyn std::error::Error>> {
    let mut selected_codes = HashSet::new();
    let mut samples = Vec::new();
    for entry in supplemental_entries {
        if entry.text.chars().count() != 4
            || entry.syllable_codes.len() != 4
            || !selected_codes.insert(entry.code.as_str().to_owned())
        {
            continue;
        }
        if !SyntheticFourCharacterEdit::ALL.iter().all(|&edit| {
            synthesize_four_character_edit(entry.code.as_str(), edit, samples.len()).is_some()
        }) {
            continue;
        }
        samples.push((entry.text.clone(), entry.code.as_str().to_owned()));
        if samples.len() == sample_limit {
            break;
        }
    }

    let mut audits = [FourCharacterCorrectionAudit::default(); 4];
    for (sample_index, (target_text, intended_code)) in samples.into_iter().enumerate() {
        for edit in SyntheticFourCharacterEdit::ALL {
            let audit = &mut audits[edit.index()];
            let observed = synthesize_four_character_edit(&intended_code, edit, sample_index)
                .ok_or("four-character audit could not synthesize a selected edit")?;
            audit.samples += 1;
            if !matches!(
                layered_four_character_correction_decision(
                    core,
                    Some(supplemental),
                    &intended_code,
                    1,
                )?,
                FourCharacterCorrectionDecision::KeepOrdinary(
                    FourCharacterCorrectionKeepReason::OriginalHasExactFullCode
                )
            ) {
                audit.exact_protection_failures += 1;
            }
            match layered_four_character_correction_decision(
                core,
                Some(supplemental),
                &observed,
                MAX_CANDIDATE_SNAPSHOT_RANK,
            )? {
                FourCharacterCorrectionDecision::Offer(offer) => {
                    audit.offered += 1;
                    audit.wrong_intended_code += usize::from(offer.intended_code != intended_code);
                    audit.target_first += usize::from(
                        offer
                            .candidates
                            .first()
                            .map(|candidate| candidate.text.as_str())
                            == Some(target_text.as_str()),
                    );
                    audit.target_visible += usize::from(
                        offer
                            .candidates
                            .iter()
                            .any(|candidate| candidate.text == target_text),
                    );
                }
                FourCharacterCorrectionDecision::KeepOrdinary(reason) => match reason {
                    FourCharacterCorrectionKeepReason::OriginalHasExactFullCode => {
                        audit.original_code_collision += 1;
                    }
                    FourCharacterCorrectionKeepReason::AmbiguousIntendedCodes => {
                        audit.ambiguous_codes += 1;
                    }
                    FourCharacterCorrectionKeepReason::NoSingleEditRecovery => {
                        audit.no_recovery += 1;
                    }
                    FourCharacterCorrectionKeepReason::UnsupportedInputShape => {
                        return Err(
                            "four-character audit produced an unsupported input shape".into()
                        );
                    }
                },
            }
        }
    }
    Ok(audits)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ShortWordExtraKeyCorrectionAudit {
    samples: usize,
    offered: usize,
    target_first: usize,
    target_visible: usize,
    conflicting_codes: usize,
    missing_independent_evidence: usize,
    no_recovery: usize,
    synthetic_target_code_misses: usize,
    clean_code_protection_failures: usize,
}

fn synthesize_short_word_neighbor_extra_key(
    intended_code: &str,
    sample_index: usize,
) -> Option<String> {
    if !matches!(intended_code.len(), 4 | 6)
        || !intended_code.as_bytes().iter().all(u8::is_ascii_lowercase)
    {
        return None;
    }
    let insertion_index = sample_index % (intended_code.len() + 1);
    let anchor_index = insertion_index.min(intended_code.len() - 1);
    let anchor = intended_code.as_bytes()[anchor_index];
    let neighbor = (b'a'..=b'z').find(|&key| are_qwerty_neighbors(anchor, key))?;
    let mut observed = intended_code.as_bytes().to_vec();
    observed.insert(insertion_index, neighbor);
    String::from_utf8(observed).ok()
}

fn audit_short_word_extra_key_correction_gate(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    supplemental_entries: &[ziranma_core::LexiconEntry],
    sample_limit_per_length: usize,
) -> Result<[ShortWordExtraKeyCorrectionAudit; 2], Box<dyn std::error::Error>> {
    let mut selected_codes = [HashSet::new(), HashSet::new()];
    let mut samples = [Vec::new(), Vec::new()];
    for entry in supplemental_entries {
        let character_count = entry.text.chars().count();
        let length_index = match character_count {
            2 => 0,
            3 => 1,
            _ => continue,
        };
        if samples[length_index].len() == sample_limit_per_length
            || entry.syllable_codes.len() != character_count
            || selected_codes[length_index].contains(entry.code.as_str())
        {
            continue;
        }
        let core_top = core.exact_full_code_texts(entry.code.as_str(), 1)?;
        let supplemental_top = supplemental.exact_full_code_texts(entry.code.as_str(), 1)?;
        if core_top.first() != Some(&entry.text) || supplemental_top.first() != Some(&entry.text) {
            continue;
        }
        selected_codes[length_index].insert(entry.code.as_str().to_owned());
        samples[length_index].push((entry.text.clone(), entry.code.as_str().to_owned()));
        if samples
            .iter()
            .all(|samples| samples.len() == sample_limit_per_length)
        {
            break;
        }
    }

    let mut audits = [ShortWordExtraKeyCorrectionAudit::default(); 2];
    for (length_index, selected) in samples.into_iter().enumerate() {
        for (sample_index, (target_text, intended_code)) in selected.into_iter().enumerate() {
            let audit = &mut audits[length_index];
            audit.samples += 1;
            if !matches!(
                layered_short_word_extra_key_correction_decision(
                    core,
                    Some(supplemental),
                    &intended_code,
                    1,
                )?,
                ShortWordExtraKeyCorrectionDecision::KeepOrdinary(
                    ShortWordExtraKeyCorrectionKeepReason::UnsupportedInputShape
                )
            ) {
                audit.clean_code_protection_failures += 1;
            }
            let observed = synthesize_short_word_neighbor_extra_key(&intended_code, sample_index)
                .ok_or("short-word audit could not synthesize a selected extra key")?;
            match layered_short_word_extra_key_correction_decision(
                core,
                Some(supplemental),
                &observed,
                MAX_CANDIDATE_SNAPSHOT_RANK,
            )? {
                ShortWordExtraKeyCorrectionDecision::Offer(offer) => {
                    audit.offered += 1;
                    audit.synthetic_target_code_misses +=
                        usize::from(offer.intended_code != intended_code);
                    audit.target_first += usize::from(
                        offer
                            .candidates
                            .first()
                            .map(|candidate| candidate.text.as_str())
                            == Some(target_text.as_str()),
                    );
                    audit.target_visible += usize::from(
                        offer
                            .candidates
                            .iter()
                            .any(|candidate| candidate.text == target_text),
                    );
                }
                ShortWordExtraKeyCorrectionDecision::KeepOrdinary(reason) => match reason {
                    ShortWordExtraKeyCorrectionKeepReason::UnsupportedInputShape => {
                        return Err("short-word audit produced an unsupported error shape".into());
                    }
                    ShortWordExtraKeyCorrectionKeepReason::MissingIndependentPublicEvidence => {
                        audit.missing_independent_evidence += 1;
                    }
                    ShortWordExtraKeyCorrectionKeepReason::NoNeighborExtraKeyRecovery => {
                        audit.no_recovery += 1;
                    }
                    ShortWordExtraKeyCorrectionKeepReason::ConflictingIntendedCodes => {
                        audit.conflicting_codes += 1;
                    }
                },
            }
        }
    }
    Ok(audits)
}

#[derive(Clone, Copy)]
struct DurationSummary {
    samples: usize,
    median: Duration,
    p95: Duration,
    p99: Duration,
    maximum: Duration,
}

fn summarize_durations(samples: &mut [Duration]) -> Option<DurationSummary> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let p95_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
    let p99_index = (samples.len() * 99).div_ceil(100).saturating_sub(1);
    Some(DurationSummary {
        samples: samples.len(),
        median: samples[samples.len() / 2],
        p95: samples[p95_index],
        p99: samples[p99_index],
        maximum: *samples.last()?,
    })
}

fn update_candidate_text_checksum(mut checksum: usize, candidates: &[String]) -> usize {
    for candidate in candidates {
        checksum = checksum.wrapping_mul(31).wrapping_add(candidate.len());
    }
    checksum
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn write_slice_stats(output: &mut String, stats: PublicRimeSliceImportStats) {
    writeln!(output, "源数据：{} 行", stats.source_rows).unwrap();
    writeln!(
        output,
        "跳过：字段 {}，权重格式 {}，非正权重 {}，文字范围 {}，拼音 {}，字音不齐 {}，音节过长 {}",
        stats.malformed_rows,
        stats.invalid_weight_rows,
        stats.nonpositive_weight_rows,
        stats.text_shape_rows,
        stats.unsupported_pinyin_rows,
        stats.text_syllable_mismatch_rows,
        stats.too_many_syllable_rows,
    )
    .unwrap();
    writeln!(
        output,
        "裁剪：合格 {}，上限外 {}，所选重复 {}，最低保留权重 {}",
        stats.eligible_rows,
        stats.dropped_by_entry_cap,
        stats.selected_duplicate_rows,
        stats.minimum_selected_frequency,
    )
    .unwrap();
    writeln!(
        output,
        "双字码覆盖：深度 {}，合格码 {}（分层身份 {}），全局前沿已覆盖 {}（{} 行），补充候选 {}，保留 {}，上限外 {}，高频回填 {}",
        stats.two_character_coverage_depth,
        stats.eligible_two_character_codes,
        stats.eligible_two_character_depth_entries,
        stats.frequency_frontier_two_character_codes,
        stats.frequency_frontier_selected_rows,
        stats.two_character_coverage_candidates,
        stats.two_character_coverage_admitted,
        stats.two_character_coverage_dropped,
        stats.frequency_backfill_admitted,
    )
    .unwrap();
    writeln!(
        output,
        "三字码覆盖：合格码 {}，全局前沿 {}，补充候选 {}，保留 {}，配额外 {}，最终切片 {}，缺口 {}",
        stats.eligible_three_character_codes,
        stats.frequency_frontier_three_character_codes,
        stats.three_character_coverage_candidates,
        stats.three_character_coverage_admitted,
        stats.three_character_coverage_dropped,
        stats.imported_three_character_codes,
        stats
            .eligible_three_character_codes
            .saturating_sub(stats.imported_three_character_codes),
    )
    .unwrap();
    writeln!(
        output,
        "四字码覆盖：合格码 {}，全局前沿 {}，补充候选 {}，保留 {}，配额外 {}，最终切片 {}，缺口 {}",
        stats.eligible_four_character_codes,
        stats.frequency_frontier_four_character_codes,
        stats.four_character_coverage_candidates,
        stats.four_character_coverage_admitted,
        stats.four_character_coverage_dropped,
        stats.imported_four_character_codes,
        stats
            .eligible_four_character_codes
            .saturating_sub(stats.imported_four_character_codes),
    )
    .unwrap();
}

fn write_public_package(
    output: &Path,
    revision: &str,
    declaration: &PublicSourceDeclaration,
    payload: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest = CandidatePackageManifest::from_payload(revision, false, payload)?;
    let manifest_text = manifest.render();
    let provenance_text = CandidatePackageProvenance::from_materials(
        &declaration.id,
        &declaration.license,
        &declaration.url,
        &declaration.sha256,
        &manifest_text,
        payload,
    )?
    .render();

    write_public_package_files(output, &manifest_text, &provenance_text, payload)
}

fn write_multi_source_public_package(
    output: &Path,
    revision: &str,
    materials: Vec<CandidateSourceMaterial>,
    payload: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest = CandidatePackageManifest::from_payload(revision, false, payload)?;
    let manifest_text = manifest.render();
    let provenance_text =
        CandidatePackageProvenance::from_source_materials(materials, &manifest_text, payload)?
            .render();

    write_public_package_files(output, &manifest_text, &provenance_text, payload)
}

fn write_exact_short_public_package(
    output: &Path,
    revision: &str,
    materials: Vec<CandidateSourceMaterial>,
    payload: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest = CandidatePackageManifest::from_payload(revision, false, payload)?;
    let manifest_text = manifest.render();
    let provenance_text =
        CandidatePackageProvenance::from_source_materials(materials, &manifest_text, payload)?
            .render();
    ensure_path_absent(output, "exact short-word package output")?;

    fs::create_dir(output)
        .map_err(|_| "cannot create explicitly named exact short-word package output")?;
    let build_result = (|| -> Result<LoadedExactShortPackage, Box<dyn std::error::Error>> {
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
            payload.as_bytes(),
        )?;
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
            manifest_text.as_bytes(),
        )?;
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance_text.as_bytes(),
        )?;
        load_exact_short_package_directory(output)
    })();
    if build_result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    let loaded = build_result?;
    let mut report = String::new();
    writeln!(report, "公开精确短词包已生成")?;
    writeln!(report, "版本：{}", loaded.catalog.revision())?;
    writeln!(report, "词条：{}", loaded.catalog.entry_count())?;
    writeln!(report, "完整码：{}", loaded.catalog.code_count())?;
    writeln!(
        report,
        "最大同码深度：{}",
        loaded.catalog.maximum_code_depth()
    )?;
    writeln!(report, "载荷：{} 字节", loaded.catalog.payload_bytes())?;
    writeln!(report, "紧凑索引：{} 字节", loaded.catalog.index_bytes())?;
    writeln!(
        report,
        "来源：{}",
        provenance_source_summary(&loaded.provenance)
    )?;
    writeln!(report, "认证 SHA-256：{}", loaded.authentication_sha256)?;
    Ok(report)
}

fn write_public_package_files(
    output: &Path,
    manifest_text: &str,
    provenance_text: &str,
    payload: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    ensure_path_absent(output, "package output")?;

    fs::create_dir(output).map_err(|_| "cannot create explicitly named package output")?;
    let build_result = (|| -> Result<LoadedPackage, Box<dyn std::error::Error>> {
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
            payload.as_bytes(),
        )?;
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
            manifest_text.as_bytes(),
        )?;
        write_new_synced(
            &output.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance_text.as_bytes(),
        )?;
        load_package_directory(output)
    })();
    if build_result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    let loaded = build_result?;
    Ok(render_build_report(
        &loaded.snapshot,
        &loaded.provenance,
        &loaded.authentication_sha256,
    ))
}

fn status(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_slot_state(root)?;
    render_slot_report(root, &state)
}

fn runtime_check(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let snapshot =
        load_current_candidate_snapshot(root)?.ok_or("candidate runtime root is not configured")?;
    Ok(format!(
        "候选运行时检查\n版本：{}\n词条：{}\n结果：通过\n本次操作：只读\n",
        snapshot.revision(),
        snapshot.entry_count()
    ))
}

fn runtime_query(
    root: &Path,
    supplemental_root: Option<&Path>,
    exact_short_root: Option<&Path>,
    code: &str,
    limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let runtime =
        load_candidate_runtime_snapshots_with_layers(root, supplemental_root, exact_short_root)?
            .ok_or("candidate runtime root is not configured")?;
    let primary_candidates = |query_limit| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut candidates = match runtime.supplemental() {
            Some(supplemental) => {
                // Keep this diagnostic aligned with TSF's gated runtime path.
                layered_candidate_texts(
                    runtime.core(),
                    supplemental.snapshot(),
                    code,
                    query_limit,
                    supplemental.config(),
                )?
            }
            None => runtime.core().candidate_texts(code, query_limit)?,
        };
        if query_limit >= 2 && !candidates.is_empty() {
            let supplemental = runtime
                .supplemental()
                .map(|supplemental| supplemental.snapshot().as_ref());
            let recovered_text = match layered_short_word_extra_key_correction_decision(
                runtime.core(),
                supplemental,
                code,
                1,
            )? {
                ShortWordExtraKeyCorrectionDecision::Offer(offer) => offer
                    .candidates
                    .into_iter()
                    .next()
                    .map(|candidate| candidate.text),
                ShortWordExtraKeyCorrectionDecision::KeepOrdinary(_) => None,
            };
            let recovered_text = if recovered_text.is_some() {
                recovered_text
            } else {
                match layered_four_character_correction_decision(
                    runtime.core(),
                    supplemental,
                    code,
                    1,
                )? {
                    FourCharacterCorrectionDecision::Offer(offer) => offer
                        .candidates
                        .into_iter()
                        .next()
                        .map(|candidate| candidate.text),
                    FourCharacterCorrectionDecision::KeepOrdinary(_) => None,
                }
            };
            if let Some(recovered_text) = recovered_text {
                let existing_index = candidates
                    .iter()
                    .position(|candidate| candidate == &recovered_text);
                if existing_index != Some(0) {
                    if let Some(existing_index) = existing_index {
                        candidates.remove(existing_index);
                    }
                    candidates.insert(1.min(candidates.len()), recovered_text);
                    candidates.truncate(query_limit);
                }
            }
        }
        Ok(candidates)
    };

    let mut exact_inserted = Vec::new();
    let candidates = if let Some(exact_short) = runtime.exact_short()
        && code.len() == 4
        && limit > RUNTIME_QUERY_PAGE_SIZE
    {
        // Replay the same 6 -> 12 -> 18 ... lazy-depth sequence used by the
        // host. Computing only the final depth can choose a different guarded
        // insertion after an already presented second page has been frozen.
        let mut session = ExactShortPageSession::default();
        let mut requested_limit = RUNTIME_QUERY_PAGE_SIZE;
        loop {
            let primary = primary_candidates(requested_limit)?;
            session.extend(
                exact_short.catalog(),
                &primary,
                code,
                requested_limit,
                exact_short.exact_promotions(),
                RUNTIME_QUERY_PAGE_SIZE,
            )?;
            if requested_limit == limit {
                break;
            }
            requested_limit = requested_limit
                .saturating_add(RUNTIME_QUERY_PAGE_SIZE)
                .min(limit);
        }
        exact_inserted = session
            .primary_indices()
            .iter()
            .map(Option::is_none)
            .collect();
        session
            .extend(
                exact_short.catalog(),
                &primary_candidates(limit)?,
                code,
                limit,
                exact_short.exact_promotions(),
                RUNTIME_QUERY_PAGE_SIZE,
            )?
            .to_vec()
    } else {
        primary_candidates(limit)?
    };
    exact_inserted.resize(candidates.len(), false);

    let supplemental_revision = runtime
        .supplemental()
        .map(|supplemental| supplemental.snapshot().revision());
    let exact_short_revision = runtime
        .exact_short()
        .map(|exact_short| exact_short.catalog().revision());
    let mut output = String::new();
    writeln!(output, "TSF 公共候选管线审计").unwrap();
    writeln!(output, "核心版本：{}", runtime.core().revision()).unwrap();
    writeln!(
        output,
        "补充版本：{}",
        supplemental_revision.unwrap_or(if runtime.supplemental_fell_back() {
            "加载失败，已回退"
        } else {
            "未启用"
        })
    )
    .unwrap();
    writeln!(
        output,
        "精确短词版本：{}",
        exact_short_revision.unwrap_or(if runtime.exact_short_fell_back() {
            "加载失败，已回退"
        } else {
            "未启用"
        })
    )
    .unwrap();
    writeln!(output, "输入：{code}").unwrap();
    for (index, candidate) in candidates.iter().enumerate() {
        let source = if exact_inserted[index] {
            " 〔公开精确短词〕"
        } else {
            ""
        };
        writeln!(output, "{}. {}{source}", index + 1, candidate).unwrap();
    }
    if candidates.is_empty() {
        writeln!(output, "（没有候选）").unwrap();
    }
    writeln!(
        output,
        "口径：按 TSF 的 6 项分页顺序重放公开核心、补充与精确短词层；不含显式别名、项目覆盖、会话记忆、个人学习或上下文重排。"
    )
    .unwrap();
    writeln!(output, "本次操作：只读").unwrap();
    Ok(output)
}

fn supplement_status(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let slots = read_slot_state(root)?;
    let state = read_supplemental_state(root)?;
    let prepared_revision = slot_revision(root, slots.current())?;
    let status = match state.package() {
        None => format!(
            "公开补充词层\n状态：关闭\n已准备：{}\n本次操作：只读\n",
            prepared_revision.as_deref().unwrap_or("未配置")
        ),
        Some(package) if Some(package) != slots.current() => format!(
            "公开补充词层\n状态：已回退，仅使用核心候选\n已准备：{}\n原因：当前包尚未重新确认\n本次操作：只读\n",
            prepared_revision.as_deref().unwrap_or("未配置")
        ),
        Some(package) => {
            let loaded = load_installed_package(root, package)?;
            validate_preflight_receipt(root, package, &loaded.authentication_sha256)?;
            format!(
                "公开补充词层\n状态：启用\n版本：{}\n每码最多补：{} 个完整词\n冷启动：核心已有完整词首选保持不动；共识重排仅供离线审计\n本次操作：只读\n",
                loaded.snapshot.revision(),
                state.exact_promotions(),
            )
        }
    };
    Ok(status)
}

fn supplement_enable(
    root: &Path,
    exact_promotions: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let slots = read_slot_state(root)?;
    let package = slots
        .current()
        .ok_or("supplemental candidate package is not configured")?;
    let loaded = load_installed_package(root, package)?;
    validate_preflight_receipt(root, package, &loaded.authentication_sha256)?;
    let state = CandidateSupplementalState::enabled(package, exact_promotions)?;
    write_supplemental_state(root, &state)?;
    Ok(format!(
        "公开补充词层已启用\n版本：{}\n每码最多补：{exact_promotions} 个完整词\n冷启动：核心已有完整词首选保持不动；共识重排仅供离线审计\n生效：支持热刷新的宿主从下一个组合开始；旧版宿主需重新打开\n",
        loaded.snapshot.revision(),
    ))
}

fn supplement_disable(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_supplemental_state(root)?;
    if !state.is_enabled() {
        return Ok("公开补充词层已经关闭\n写入：0 个文件\n现有输入法宿主：未改动\n".to_owned());
    }
    write_supplemental_state(root, &CandidateSupplementalState::default())?;
    Ok("公开补充词层已关闭\n候选包：保留，可再次启用\n生效：支持热刷新的宿主从下一个组合开始；旧版宿主需重新打开\n".to_owned())
}

fn exact_short_status(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let slots = read_slot_state(root)?;
    let state = read_exact_short_state(root)?;
    let prepared_revision = exact_short_slot_revision(root, slots.current())?;
    let status = match state.package() {
        None => format!(
            "公开精确短词层\n状态：关闭\n已准备：{}\n本次操作：只读\n",
            prepared_revision.as_deref().unwrap_or("未配置")
        ),
        Some(package) if Some(package) != slots.current() => format!(
            "公开精确短词层\n状态：已回退，不注入第二页\n已准备：{}\n原因：当前包尚未重新确认\n本次操作：只读\n",
            prepared_revision.as_deref().unwrap_or("未配置")
        ),
        Some(package) => {
            let loaded = load_installed_exact_short_package(root, package)?;
            validate_preflight_receipt(root, package, &loaded.authentication_sha256)?;
            validate_exact_short_preflight_identity(
                root,
                package,
                &loaded.authentication_sha256,
                state.exact_promotions(),
            )?;
            format!(
                "公开精确短词层\n状态：启用\n版本：{}\n每码最多补：{} 个双来源确认短词\n分页边界：第一页保持不动；从第二页开头按需注入\n组合凭据：存在；运行时还会复核当前核心与补充层\n本次操作：只读\n",
                loaded.catalog.revision(),
                state.exact_promotions(),
            )
        }
    };
    Ok(status)
}

struct ExactShortReadinessRequest<'a> {
    root: &'a Path,
    core_root: &'a Path,
    supplemental_root: Option<&'a Path>,
    package: &'a Path,
    expected_sha256: &'a str,
    exact_promotions: usize,
}

fn exact_short_readiness(
    request: ExactShortReadinessRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let ExactShortReadinessRequest {
        root,
        core_root,
        supplemental_root,
        package,
        expected_sha256,
        exact_promotions,
    } = request;
    let exact_public = match load_public_package_directory(package) {
        Ok(loaded) if verify_expected_sha256(&loaded, expected_sha256).is_ok() => loaded,
        _ => {
            return Ok(
                "公开精确短词准备度\n公开材料：不可用或可信摘要不匹配\n核心与补充层：未检查\n专项准备：不能确认\n日用状态：本次未改变\n下一步：恢复固定公开材料后重新只读体检\n本次操作：只读\n"
                    .to_owned(),
            );
        }
    };
    let exact_catalog = match load_exact_short_package_directory(package) {
        Ok(loaded) => loaded,
        Err(_) => {
            return Ok(
                "公开精确短词准备度\n公开材料：格式不适用于精确短词层\n核心与补充层：未检查\n专项准备：不能确认\n日用状态：本次未改变\n下一步：恢复固定公开材料后重新只读体检\n本次操作：只读\n"
                    .to_owned(),
            );
        }
    };
    if exact_catalog.authentication_sha256 != exact_public.authentication_sha256 {
        return Ok(
            "公开精确短词准备度\n公开材料：检查期间发生变化\n核心与补充层：未检查\n专项准备：不能确认\n日用状态：本次未改变\n下一步：保持材料静止后重新只读体检\n本次操作：只读\n"
                .to_owned(),
        );
    }
    let expected_package = candidate_package_storage_id(
        &exact_public.provenance_text,
        &exact_public.manifest_text,
        &exact_public.payload_text,
    );
    let public = match load_exact_short_public_context(core_root, supplemental_root) {
        Ok(public) => public,
        Err(_) => {
            return Ok(format!(
                "公开精确短词准备度\n公开材料：通过；版本 {}\n核心与补充层：不可用、未启用或凭据无效\n专项准备：不能确认\n日用状态：本次未改变；运行时会按认证结果失败关闭\n下一步：先恢复当前公开运行时组合，再重新只读体检\n本次操作：只读\n",
                exact_catalog.catalog.revision(),
            ));
        }
    };
    let (slots, state) = match (read_slot_state(root), read_exact_short_state(root)) {
        (Ok(slots), Ok(state)) => (slots, state),
        _ => {
            return Ok(format!(
                "公开精确短词准备度\n公开材料：通过；版本 {}\n核心与补充层：通过\n专项准备：独立根损坏，不能启用\n日用状态：本次未改变；运行时将停止注入\n下一步：保留现场并只读诊断\n本次操作：只读\n",
                exact_catalog.catalog.revision(),
            ));
        }
    };

    let Some(current) = slots.current() else {
        if state.is_enabled() {
            return Ok(render_unusable_exact_short_readiness(
                exact_catalog.catalog.revision(),
                &state,
            ));
        }
        return Ok(format!(
            "公开精确短词准备度\n公开材料：通过；版本 {}\n核心与补充层：通过\n专项准备：尚未进行\n日用状态：关闭；候选未改变\n下一步：可显式准备；会写入独立精确短词根，但不会启用\n本次操作：只读\n",
            exact_catalog.catalog.revision(),
        ));
    };
    if current != expected_package {
        let daily_state = match state.package() {
            None => "关闭；候选未改变",
            Some(package) if Some(package) == slots.current() => {
                "状态指向另一版本；本次未认证或改变"
            }
            Some(_) => "已回退；当前准备包不会注入第二页",
        };
        return Ok(format!(
            "公开精确短词准备度\n公开材料：通过\n核心与补充层：通过\n专项准备：独立根指向另一版本，未按本组合认证\n日用状态：{daily_state}\n下一步：需要独立审计与迁移，固定准备入口不会覆盖\n本次操作：只读\n"
        ));
    }

    let installed = match load_installed_exact_short_package(root, current) {
        Ok(installed) => installed,
        Err(_) => {
            return Ok(render_unusable_exact_short_readiness(
                exact_catalog.catalog.revision(),
                &state,
            ));
        }
    };
    if validate_preflight_receipt(root, current, &installed.authentication_sha256).is_err() {
        return Ok(render_unusable_exact_short_readiness(
            exact_catalog.catalog.revision(),
            &state,
        ));
    }
    if installed.authentication_sha256 != exact_public.authentication_sha256 {
        return Ok(render_unusable_exact_short_readiness(
            exact_catalog.catalog.revision(),
            &state,
        ));
    }
    let receipt = match validate_exact_short_preflight_identity(
        root,
        current,
        &installed.authentication_sha256,
        exact_promotions,
    ) {
        Ok(receipt) => receipt,
        Err(_) => {
            return Ok(render_unusable_exact_short_readiness(
                exact_catalog.catalog.revision(),
                &state,
            ));
        }
    };
    let supplemental = public
        .supplemental_sha256
        .as_deref()
        .zip(public.supplemental_promotions);
    if !receipt.matches_runtime(&public.core_sha256, supplemental) {
        return Ok(format!(
            "公开精确短词准备度\n公开材料：通过；版本 {}\n核心与补充层：通过，但已偏离专项凭据\n专项准备：已失效，不能启用\n日用状态：{}；本次未改变\n下一步：需要为当前公开组合重新预检；固定准备入口不会覆盖旧组合\n本次操作：只读\n",
            installed.catalog.revision(),
            if state.is_enabled() {
                "凭据漂移，运行时将停止注入"
            } else {
                "关闭"
            },
        ));
    }

    let (daily_state, next_step) =
        if state.package() == Some(current) && state.exact_promotions() == exact_promotions {
            (
                "已启用；运行时仍会逐次复核组合凭据",
                "无需写入；如需回退可显式关闭",
            )
        } else if state.is_enabled() {
            (
                "已回退；当前准备包不会注入第二页",
                "需先审计现有状态；不能直接复用启用",
            )
        } else {
            (
                "关闭；候选未改变",
                "已可启用；启用会写一个小状态文件并从下一组合生效",
            )
        };
    Ok(format!(
        "公开精确短词准备度\n公开材料：通过；版本 {}\n核心与补充层：通过\n专项准备：通过；组合凭据匹配\n日用状态：{daily_state}\n下一步：{next_step}\n本次操作：只读\n",
        installed.catalog.revision(),
    ))
}

fn render_unusable_exact_short_readiness(
    expected_revision: &str,
    state: &CandidateExactShortState,
) -> String {
    let daily_state = if state.is_enabled() {
        "凭据无效；运行时将停止注入"
    } else {
        "关闭；候选未改变"
    };
    format!(
        "公开精确短词准备度\n公开材料：通过；版本 {expected_revision}\n核心与补充层：通过\n专项准备：损坏或凭据不完整；不能启用\n日用状态：{daily_state}\n下一步：保留现场并只读诊断；固定准备入口不会覆盖\n本次操作：只读\n"
    )
}

fn exact_short_prepare(
    request: ExactShortPrepareRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let ExactShortPrepareRequest {
        root,
        core_root,
        supplemental_root,
        package,
        expected_sha256,
        exact_promotions,
        sample_limit,
        repetitions,
    } = request;
    if cfg!(debug_assertions) {
        return Err("exact-short-prepare must run from a release build".into());
    }
    let slots = read_slot_state(root)?;
    let state = read_exact_short_state(root)?;
    if state.is_enabled() {
        return Err("exact-short-prepare refuses an enabled exact-short root".into());
    }
    if slots.candidate().is_some() || slots.previous().is_some() {
        return Err("exact-short-prepare requires an unambiguous exact-short slot".into());
    }

    let exact_loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&exact_loaded, expected_sha256)?;
    let exact_catalog = load_exact_short_package_directory(package)?;
    if exact_catalog.authentication_sha256 != exact_loaded.authentication_sha256 {
        return Err("exact-short package authentication changed during preparation".into());
    }
    let exact_package_id = candidate_package_storage_id(
        &exact_loaded.provenance_text,
        &exact_loaded.manifest_text,
        &exact_loaded.payload_text,
    );
    if slots
        .current()
        .is_some_and(|current| current != exact_package_id)
    {
        return Err("exact-short-prepare refuses to replace a different current package".into());
    }

    let public = load_exact_short_public_context(core_root, supplemental_root)?;
    let summary = run_exact_short_tsf_preflight(ExactShortTsfPreflightRequest {
        core_package: &public.core_path,
        supplemental_package: public.supplemental_path.as_deref(),
        exact_package: package,
        supplemental_promotions: public.supplemental_promotions,
        exact_promotions,
        sample_limit,
        repetitions,
    })?;
    if summary.core.authentication_sha256 != public.core_sha256
        || summary.exact.authentication_sha256 != exact_loaded.authentication_sha256
        || summary
            .supplemental
            .as_ref()
            .map(|identity| identity.authentication_sha256.as_str())
            != public.supplemental_sha256.as_deref()
    {
        return Err("exact-short preparation identities changed during TSF preflight".into());
    }

    prepare_slot_root(root)?;
    let installed_id = install_package(root, &exact_loaded)?;
    if installed_id != exact_package_id {
        return Err("exact-short installed identity changed during preparation".into());
    }
    let installed = load_installed_exact_short_package(root, &installed_id)?;
    preflight_loaded_package(&load_installed_package(root, &installed_id)?)?;
    write_preflight_receipt(root, &installed_id, &installed.authentication_sha256)?;
    let receipt = CandidateExactShortPreflightReceipt::new(
        &installed_id,
        &installed.authentication_sha256,
        exact_promotions,
        &public.core_sha256,
        public
            .supplemental_sha256
            .as_deref()
            .zip(public.supplemental_promotions),
    )?;
    write_exact_short_combined_preflight(root, &receipt)?;
    if slots.current().is_none() {
        let mut prepared_slots = slots;
        prepared_slots.adopt(&installed_id)?;
        write_slot_state(root, &prepared_slots)?;
    }
    write_exact_short_state(root, &CandidateExactShortState::default())?;
    Ok(format!(
        "公开精确短词层已准备但未启用\n版本：{}\n每码最多补：{exact_promotions} 个双来源确认短词\nTSF 第二页组合预检：{} 个样本通过\n状态：关闭；日用候选尚未改变\n",
        installed.catalog.revision(),
        summary.first_page.samples,
    ))
}

struct ExactShortEnableRequest<'a> {
    root: &'a Path,
    core_root: &'a Path,
    supplemental_root: Option<&'a Path>,
    package: &'a Path,
    expected_sha256: &'a str,
    exact_promotions: usize,
}

type ExactShortRuntimeVerifier = fn(&Path, Option<&Path>, &Path, &str, &str, usize) -> bool;

fn exact_short_enable(
    request: ExactShortEnableRequest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    exact_short_enable_with_runtime_verifier(request, verify_enabled_exact_short_runtime)
}

fn exact_short_enable_with_runtime_verifier(
    request: ExactShortEnableRequest<'_>,
    runtime_verifier: ExactShortRuntimeVerifier,
) -> Result<String, Box<dyn std::error::Error>> {
    let ExactShortEnableRequest {
        root,
        core_root,
        supplemental_root,
        package: public_package,
        expected_sha256,
        exact_promotions,
    } = request;
    let exact_public = load_public_package_directory(public_package)?;
    verify_expected_sha256(&exact_public, expected_sha256)?;
    let expected_package = candidate_package_storage_id(
        &exact_public.provenance_text,
        &exact_public.manifest_text,
        &exact_public.payload_text,
    );
    let slots = read_slot_state(root)?;
    let package = slots
        .current()
        .ok_or("exact-short candidate package is not configured")?;
    if package != expected_package {
        return Err(
            "prepared exact-short package does not match the explicit public package".into(),
        );
    }
    let loaded = load_installed_exact_short_package(root, package)?;
    if loaded.authentication_sha256 != exact_public.authentication_sha256 {
        return Err("prepared exact-short authentication does not match the public package".into());
    }
    validate_preflight_receipt(root, package, &loaded.authentication_sha256)?;
    let public = load_exact_short_public_context(core_root, supplemental_root)?;
    let receipt = validate_exact_short_preflight_identity(
        root,
        package,
        &loaded.authentication_sha256,
        exact_promotions,
    )?;
    if !receipt.matches_runtime(
        &public.core_sha256,
        public
            .supplemental_sha256
            .as_deref()
            .zip(public.supplemental_promotions),
    ) {
        return Err("exact-short combined preflight no longer matches the public runtime".into());
    }
    let state = CandidateExactShortState::enabled(package, exact_promotions)?;
    write_exact_short_state(root, &state)?;

    if !runtime_verifier(
        core_root,
        supplemental_root,
        root,
        package,
        &loaded.authentication_sha256,
        exact_promotions,
    ) {
        return match write_exact_short_state(root, &CandidateExactShortState::default()) {
            Ok(()) => Err(
                "exact-short runtime verification failed; activation was rolled back to disabled"
                    .into(),
            ),
            Err(_) => Err(
                "exact-short runtime verification failed and rollback could not be confirmed"
                    .into(),
            ),
        };
    }
    Ok(format!(
        "公开精确短词层已启用\n版本：{}\n每码最多补：{exact_promotions} 个双来源确认短词\n运行时复读：通过；同一包、同一上限、无分层回退\n分页边界：第一页保持不动；从第二页开头按需注入\n生效：支持热刷新的宿主从下一个组合开始；旧版宿主需重新打开\n",
        loaded.catalog.revision(),
    ))
}

fn verify_enabled_exact_short_runtime(
    core_root: &Path,
    supplemental_root: Option<&Path>,
    exact_root: &Path,
    package: &str,
    authentication_sha256: &str,
    exact_promotions: usize,
) -> bool {
    load_candidate_runtime_snapshots_with_layers(core_root, supplemental_root, Some(exact_root))
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .is_some_and(|snapshots| {
            !snapshots.supplemental_fell_back()
                && !snapshots.exact_short_fell_back()
                && snapshots.exact_short().is_some_and(|exact| {
                    exact.package_id() == package
                        && exact.authentication_sha256() == authentication_sha256
                        && exact.exact_promotions() == exact_promotions
                })
        })
}

struct ExactShortPublicContext {
    core_path: PathBuf,
    core_sha256: String,
    supplemental_path: Option<PathBuf>,
    supplemental_sha256: Option<String>,
    supplemental_promotions: Option<usize>,
}

fn load_exact_short_public_context(
    core_root: &Path,
    supplemental_root: Option<&Path>,
) -> Result<ExactShortPublicContext, Box<dyn std::error::Error>> {
    let core_slots = read_slot_state(core_root)?;
    let core_package = core_slots
        .current()
        .ok_or("exact-short operation requires a configured core package")?;
    let core = load_installed_package(core_root, core_package)?;
    validate_preflight_receipt(core_root, core_package, &core.authentication_sha256)?;
    let core_path = core_root
        .join(CANDIDATE_PACKAGES_DIRECTORY)
        .join(core_package);
    let (supplemental_path, supplemental_sha256, supplemental_promotions) = match supplemental_root
    {
        None => (None, None, None),
        Some(root) => {
            let slots = read_slot_state(root)?;
            let state = read_supplemental_state(root)?;
            let package = state
                .package()
                .ok_or("the explicit supplemental root is not enabled")?;
            if slots.current() != Some(package) {
                return Err("the explicit supplemental root is not current".into());
            }
            let loaded = load_installed_package(root, package)?;
            validate_preflight_receipt(root, package, &loaded.authentication_sha256)?;
            (
                Some(root.join(CANDIDATE_PACKAGES_DIRECTORY).join(package)),
                Some(loaded.authentication_sha256),
                Some(state.exact_promotions()),
            )
        }
    };
    Ok(ExactShortPublicContext {
        core_path,
        core_sha256: core.authentication_sha256,
        supplemental_path,
        supplemental_sha256,
        supplemental_promotions,
    })
}

fn validate_exact_short_preflight_identity(
    root: &Path,
    package: &str,
    authentication_sha256: &str,
    exact_promotions: usize,
) -> Result<CandidateExactShortPreflightReceipt, Box<dyn std::error::Error>> {
    let text = read_explicit_text(
        &root.join(CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE),
        "exact-short combined preflight receipt",
        MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES,
    )?;
    let receipt = CandidateExactShortPreflightReceipt::parse(&text)?;
    if receipt.exact_package() != package
        || receipt.exact_sha256() != authentication_sha256
        || receipt.exact_promotions() != exact_promotions
    {
        return Err("exact-short combined preflight receipt does not match its package".into());
    }
    Ok(receipt)
}

fn write_exact_short_combined_preflight(
    root: &Path,
    receipt: &CandidateExactShortPreflightReceipt,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_slot_root(root)?;
    let body = receipt.render();
    CandidateExactShortPreflightReceipt::parse(&body)?;
    let destination = root.join(CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let existing = read_explicit_text(
                &destination,
                "exact-short combined preflight receipt",
                MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES,
            )?;
            if existing == body {
                return Ok(());
            }
            return Err("a different exact-short combined preflight receipt already exists".into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect exact-short combined preflight receipt".into()),
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(
        ".exact-short-preflight-{}-{stamp}.tmp",
        std::process::id()
    ));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = fs::rename(&temporary, &destination)
        .map_err(|_| "cannot publish exact-short combined preflight receipt".into());
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn exact_short_disable(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_exact_short_state(root)?;
    if !state.is_enabled() {
        return Ok("公开精确短词层已经关闭\n写入：0 个文件\n现有输入法宿主：未改动\n".to_owned());
    }
    write_exact_short_state(root, &CandidateExactShortState::default())?;
    Ok("公开精确短词层已关闭\n候选包：保留，可再次启用\n生效：支持热刷新的宿主从下一个组合开始；旧版宿主需重新打开\n".to_owned())
}

fn preflight(package: &Path) -> Result<String, Box<dyn std::error::Error>> {
    preflight_with(package, preflight_loaded_package)
}

fn preflight_with(
    package: &Path,
    package_preflight: PackagePreflight,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    let summary = package_preflight(&loaded)?;
    Ok(render_preflight_report(&summary))
}

fn verify(package: &Path, expected_sha256: &str) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&loaded, expected_sha256)?;
    Ok(render_verify_report(&loaded))
}

fn verify_signature(
    package: &Path,
    signature_path: &Path,
    trusted_public_key: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_signature_verified_package(package, signature_path, trusted_public_key)?;
    Ok(render_signature_verify_report(&loaded))
}

fn adopt(
    root: &Path,
    package: &Path,
    expected_sha256: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    adopt_with_preflight(root, package, expected_sha256, preflight_loaded_package)
}

fn adopt_with_preflight(
    root: &Path,
    package: &Path,
    expected_sha256: &str,
    package_preflight: PackagePreflight,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&loaded, expected_sha256)?;
    adopt_loaded_with_preflight(root, loaded, package_preflight)
}

fn stage(
    root: &Path,
    package: &Path,
    expected_sha256: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    stage_with_preflight(root, package, expected_sha256, preflight_loaded_package)
}

fn stage_with_preflight(
    root: &Path,
    package: &Path,
    expected_sha256: &str,
    package_preflight: PackagePreflight,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&loaded, expected_sha256)?;
    stage_loaded_with_preflight(root, loaded, package_preflight)
}

fn adopt_signed(
    root: &Path,
    package: &Path,
    signature_path: &Path,
    trusted_public_key: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_signature_verified_package(package, signature_path, trusted_public_key)?;
    adopt_loaded(root, loaded)
}

fn stage_signed(
    root: &Path,
    package: &Path,
    signature_path: &Path,
    trusted_public_key: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_signature_verified_package(package, signature_path, trusted_public_key)?;
    stage_loaded(root, loaded)
}

fn adopt_loaded(root: &Path, loaded: LoadedPackage) -> Result<String, Box<dyn std::error::Error>> {
    adopt_loaded_with_preflight(root, loaded, preflight_loaded_package)
}

fn adopt_loaded_with_preflight(
    root: &Path,
    loaded: LoadedPackage,
    package_preflight: PackagePreflight,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    if state.current().is_some() {
        return Err("current candidate package is already configured".into());
    }
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    package_preflight(&installed)?;
    write_preflight_receipt(root, &package_id, &installed.authentication_sha256)?;
    state.adopt(&package_id)?;
    write_slot_state(root, &state)?;
    Ok(render_preflight_change_report(
        "当前候选包已建立",
        &revision,
    ))
}

fn stage_loaded(root: &Path, loaded: LoadedPackage) -> Result<String, Box<dyn std::error::Error>> {
    stage_loaded_with_preflight(root, loaded, preflight_loaded_package)
}

fn stage_loaded_with_preflight(
    root: &Path,
    loaded: LoadedPackage,
    package_preflight: PackagePreflight,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    if state.current().is_none() {
        return Err("current candidate package is not configured".into());
    }
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    package_preflight(&installed)?;
    write_preflight_receipt(root, &package_id, &installed.authentication_sha256)?;
    state.stage(&package_id)?;
    write_slot_state(root, &state)?;
    Ok(render_preflight_change_report(
        "待切换候选包已暂存",
        &revision,
    ))
}

fn promote(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    validate_installed_slot(root, state.current())?;
    let next = validate_installed_slot(root, state.candidate())?;
    validate_preflight_receipt(
        root,
        state
            .candidate()
            .ok_or("required candidate slot is empty")?,
        &next.authentication_sha256,
    )?;
    let revision = next.snapshot.revision().to_owned();
    state.promote()?;
    write_slot_state(root, &state)?;
    Ok(render_change_report("候选数据槽已切换", &revision))
}

fn rollback(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    validate_installed_slot(root, state.current())?;
    let previous = validate_installed_slot(root, state.previous())?;
    validate_preflight_receipt(
        root,
        state.previous().ok_or("required previous slot is empty")?,
        &previous.authentication_sha256,
    )?;
    let revision = previous.snapshot.revision().to_owned();
    state.rollback()?;
    write_slot_state(root, &state)?;
    Ok(render_change_report("候选数据槽已回退", &revision))
}

fn render_inspect_report(
    snapshot: &CandidateSnapshot,
    provenance: &CandidatePackageProvenance,
) -> String {
    let mut output = String::new();
    writeln!(output, "候选包检查").unwrap();
    writeln!(output, "版本：{}", snapshot.revision()).unwrap();
    writeln!(
        output,
        "内容：{}",
        if snapshot.contains_private_text() {
            "含私人文字"
        } else {
            "公开"
        }
    )
    .unwrap();
    writeln!(output, "词条：{}", snapshot.entry_count()).unwrap();
    writeln!(output, "载荷：{} 字节", snapshot.payload_bytes()).unwrap();
    writeln!(output, "来源：{}", provenance_source_summary(provenance)).unwrap();
    writeln!(output, "许可：{}", provenance_license_summary(provenance)).unwrap();
    writeln!(output, "SHA-256 与兼容性：通过").unwrap();
    writeln!(output, "本次操作：只读").unwrap();
    output
}

fn render_build_report(
    snapshot: &CandidateSnapshot,
    provenance: &CandidatePackageProvenance,
    authentication_sha256: &str,
) -> String {
    format!(
        "公开候选包已生成\n版本：{}\n来源：{}\n许可：{}\n词条：{}\n载荷：{} 字节\n\
         发布 SHA-256：{}\n写入：3 个新文件\n",
        snapshot.revision(),
        provenance_source_summary(provenance),
        provenance_license_summary(provenance),
        snapshot.entry_count(),
        snapshot.payload_bytes(),
        authentication_sha256
    )
}

fn render_verify_report(loaded: &LoadedPackage) -> String {
    format!(
        "候选包验证\n版本：{}\n来源：{}\n许可：{}\n结果：与可信 SHA-256 一致\n\
         本次操作：只读\n",
        loaded.snapshot.revision(),
        provenance_source_summary(&loaded.provenance),
        provenance_license_summary(&loaded.provenance)
    )
}

fn render_signature_verify_report(loaded: &LoadedPackage) -> String {
    format!(
        "候选包签名验证\n版本：{}\n来源：{}\n许可：{}\n结果：可信 Ed25519 签名有效\n\
         发布 SHA-256：{}\n本次操作：只读\n",
        loaded.snapshot.revision(),
        provenance_source_summary(&loaded.provenance),
        provenance_license_summary(&loaded.provenance),
        loaded.authentication_sha256
    )
}

fn provenance_source_summary(provenance: &CandidatePackageProvenance) -> String {
    if provenance.source_count() == 1 {
        provenance.source_id().to_owned()
    } else {
        format!("{} 份公开材料", provenance.source_count())
    }
}

fn provenance_license_summary(provenance: &CandidatePackageProvenance) -> String {
    provenance
        .source_materials()
        .iter()
        .map(|source| source.license())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("、")
}

fn render_preflight_report(summary: &PreflightSummary) -> String {
    format!(
        "TSF 候选预检\n版本：{}\n输入：{} 键\n上屏：{} 字\n结果：通过\n本次操作：不写文件\n",
        summary.revision, summary.input_keys, summary.committed_characters
    )
}

#[cfg(windows)]
fn preflight_loaded_package(
    loaded: &LoadedPackage,
) -> Result<PreflightSummary, Box<dyn std::error::Error>> {
    let entries = parse_lexicon_tsv(&loaded.payload_text)
        .map_err(|_| "candidate package has no usable TSF preflight probe")?;
    let probe = entries
        .first()
        .ok_or("candidate package has no usable TSF preflight probe")?;
    let code = probe.code.as_str();
    let expected = loaded
        .snapshot
        .candidate_text(code, 1)?
        .ok_or("candidate package produced no TSF preflight candidate")?;
    let report =
        preflight_candidate_snapshot(Arc::clone(&loaded.snapshot), code, expected.as_str())?;
    Ok(PreflightSummary {
        revision: report.revision().to_owned(),
        input_keys: report.input_keys(),
        committed_characters: report.committed_characters(),
    })
}

#[cfg(not(windows))]
fn preflight_loaded_package(
    _loaded: &LoadedPackage,
) -> Result<PreflightSummary, Box<dyn std::error::Error>> {
    Err("TSF candidate preflight requires Windows".into())
}

fn render_slot_report(
    root: &Path,
    state: &CandidateSlotState,
) -> Result<String, Box<dyn std::error::Error>> {
    let current = slot_revision(root, state.current())?;
    let candidate = slot_revision(root, state.candidate())?;
    let previous = slot_revision(root, state.previous())?;
    Ok(format!(
        "候选数据槽\n当前：{}\n待切换：{}\n可回退：{}\n本次操作：只读\n",
        current.as_deref().unwrap_or("未配置"),
        candidate.as_deref().unwrap_or("无"),
        previous.as_deref().unwrap_or("无")
    ))
}

fn render_change_report(action: &str, revision: &str) -> String {
    format!("{action}\n版本：{revision}\n")
}

fn render_preflight_change_report(action: &str, revision: &str) -> String {
    format!("{action}\n版本：{revision}\nTSF 预检：通过\n")
}

fn slot_revision(
    root: &Path,
    package_id: Option<&str>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match package_id {
        Some(package_id) => Ok(Some(
            load_installed_package(root, package_id)?
                .snapshot
                .revision()
                .to_owned(),
        )),
        None => Ok(None),
    }
}

fn exact_short_slot_revision(
    root: &Path,
    package_id: Option<&str>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match package_id {
        Some(package_id) => Ok(Some(
            load_installed_exact_short_package(root, package_id)?
                .catalog
                .revision()
                .to_owned(),
        )),
        None => Ok(None),
    }
}

fn load_public_package_directory(
    package: &Path,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let loaded = load_package_directory(package)?;
    if loaded.snapshot.contains_private_text() {
        return Err(
            "plaintext private candidate packages are not accepted by this slot store".into(),
        );
    }
    Ok(loaded)
}

fn load_signature_verified_package(
    package: &Path,
    signature_path: &Path,
    trusted_public_key: &str,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    let signature_text = read_explicit_text(
        signature_path,
        "candidate release signature",
        MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES,
    )?;
    let signature = CandidateReleaseSignature::parse(&signature_text)?;
    signature.verify(trusted_public_key, &loaded.authentication_sha256)?;
    Ok(loaded)
}

fn verify_expected_sha256(
    loaded: &LoadedPackage,
    expected_sha256: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if loaded.authentication_sha256 != expected_sha256 {
        return Err("candidate package does not match the expected SHA-256".into());
    }
    Ok(())
}

fn load_package_directory(package: &Path) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    ensure_regular_directory(package, "candidate package")?;
    let manifest_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        "candidate manifest",
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    )?;
    let provenance_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
        "candidate provenance",
        MAX_CANDIDATE_PROVENANCE_BYTES,
    )?;
    let payload_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        "candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let manifest = CandidatePackageManifest::parse(&manifest_text)?;
    let provenance = CandidatePackageProvenance::parse(&provenance_text)?;
    provenance.validate_materials(&manifest_text, &payload_text)?;
    let snapshot = Arc::new(manifest.load_snapshot(&payload_text)?);
    let authentication_sha256 =
        candidate_package_authentication_sha256(&provenance_text, &manifest_text, &payload_text);
    Ok(LoadedPackage {
        manifest_text,
        payload_text,
        provenance_text,
        manifest,
        provenance,
        snapshot,
        authentication_sha256,
    })
}

fn load_exact_short_package_directory(
    package: &Path,
) -> Result<LoadedExactShortPackage, Box<dyn std::error::Error>> {
    load_exact_short_package_materials(package, false).map(|(loaded, _)| loaded)
}

fn load_exact_short_package_directory_with_entries(
    package: &Path,
) -> Result<(LoadedExactShortPackage, Vec<LexiconEntry>), Box<dyn std::error::Error>> {
    let (loaded, entries) = load_exact_short_package_materials(package, true)?;
    Ok((
        loaded,
        entries.expect("the exact short-word loader was asked to retain entries"),
    ))
}

fn load_exact_short_package_materials(
    package: &Path,
    retain_entries: bool,
) -> Result<ExactShortPackageMaterials, Box<dyn std::error::Error>> {
    ensure_regular_directory(package, "exact short-word package")?;
    let manifest_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        "exact short-word manifest",
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    )?;
    let provenance_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
        "exact short-word provenance",
        MAX_CANDIDATE_PROVENANCE_BYTES,
    )?;
    let payload_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        "exact short-word payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let manifest = CandidatePackageManifest::parse(&manifest_text)?;
    let provenance = CandidatePackageProvenance::parse(&provenance_text)?;
    provenance.validate_materials(&manifest_text, &payload_text)?;
    let authentication_sha256 =
        candidate_package_authentication_sha256(&provenance_text, &manifest_text, &payload_text);
    let entries = retain_entries
        .then(|| parse_lexicon_tsv(&payload_text))
        .transpose()?;
    let catalog = ExactShortWordCatalog::load_owned(&manifest, payload_text)?;
    Ok((
        LoadedExactShortPackage {
            provenance,
            catalog,
            authentication_sha256,
        },
        entries,
    ))
}

fn validate_installed_slot(
    root: &Path,
    package_id: Option<&str>,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let package_id = package_id.ok_or("required candidate slot is empty")?;
    load_installed_package(root, package_id)
}

fn load_installed_package(
    root: &Path,
    package_id: &str,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let loaded = load_package_directory(&root.join(CANDIDATE_PACKAGES_DIRECTORY).join(package_id))?;
    if loaded.snapshot.contains_private_text() {
        return Err("candidate slot unexpectedly contains plaintext private text".into());
    }
    if candidate_package_storage_id(
        &loaded.provenance_text,
        &loaded.manifest_text,
        &loaded.payload_text,
    ) != package_id
    {
        return Err("installed candidate package no longer matches its storage identifier".into());
    }
    Ok(loaded)
}

fn load_installed_exact_short_package(
    root: &Path,
    package_id: &str,
) -> Result<LoadedExactShortPackage, Box<dyn std::error::Error>> {
    let path = root.join(CANDIDATE_PACKAGES_DIRECTORY).join(package_id);
    let ordinary = load_installed_package(root, package_id)?;
    let exact = load_exact_short_package_directory(&path)?;
    if exact.authentication_sha256 != ordinary.authentication_sha256 {
        return Err("installed exact-short package authentication changed".into());
    }
    Ok(exact)
}

fn install_package(
    root: &Path,
    loaded: &LoadedPackage,
) -> Result<String, Box<dyn std::error::Error>> {
    let package_id = candidate_package_storage_id(
        &loaded.provenance_text,
        &loaded.manifest_text,
        &loaded.payload_text,
    );
    let packages = root.join(CANDIDATE_PACKAGES_DIRECTORY);
    let destination = packages.join(&package_id);

    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let installed = load_public_package_directory(&destination)?;
            if installed.manifest_text != loaded.manifest_text
                || installed.payload_text != loaded.payload_text
                || installed.provenance_text != loaded.provenance_text
            {
                return Err("candidate package storage identifier collision".into());
            }
            return Ok(package_id);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect candidate package destination".into()),
    }

    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = packages.join(format!(".install-{}-{stamp}", std::process::id()));
    fs::create_dir(&temporary).map_err(|_| "cannot create temporary candidate package")?;
    let mut destination_created = false;
    let install_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        write_new_synced(
            &temporary.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
            loaded.payload_text.as_bytes(),
        )?;
        write_new_synced(
            &temporary.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
            loaded.manifest_text.as_bytes(),
        )?;
        write_new_synced(
            &temporary.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            loaded.provenance_text.as_bytes(),
        )?;
        fs::rename(&temporary, &destination).map_err(|_| "cannot install candidate package")?;
        destination_created = true;
        let installed = load_public_package_directory(&destination)?;
        if installed.manifest != loaded.manifest
            || installed.payload_text != loaded.payload_text
            || installed.provenance != loaded.provenance
        {
            return Err("installed candidate package failed exact verification".into());
        }
        Ok(())
    })();
    if install_result.is_err() {
        if temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        if destination_created {
            let _ = fs::remove_dir_all(&destination);
        }
    }
    install_result?;
    Ok(package_id)
}

fn preflight_receipt_path(root: &Path, package_id: &str) -> PathBuf {
    root.join(CANDIDATE_PREFLIGHTS_DIRECTORY)
        .join(format!("{package_id}.zpf"))
}

fn write_preflight_receipt(
    root: &Path,
    package_id: &str,
    authentication_sha256: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = candidate_preflight_receipt_body(package_id, authentication_sha256);
    let path = preflight_receipt_path(root, package_id);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let existing = read_explicit_text(
                &path,
                "candidate preflight receipt",
                MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
            )?;
            if existing != expected {
                return Err("candidate preflight receipt does not match its package".into());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_new_synced(&path, expected.as_bytes())
        }
        Err(_) => Err("cannot inspect candidate preflight receipt".into()),
    }
}

fn validate_preflight_receipt(
    root: &Path,
    package_id: &str,
    authentication_sha256: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = read_explicit_text(
        &preflight_receipt_path(root, package_id),
        "candidate preflight receipt",
        MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
    )?;
    if receipt != candidate_preflight_receipt_body(package_id, authentication_sha256) {
        return Err("candidate preflight receipt does not match its package".into());
    }
    Ok(())
}

fn prepare_slot_root(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| "cannot create candidate slot root")?;
            ensure_regular_directory(root, "candidate slot root")?;
        }
        Err(_) => return Err("cannot inspect candidate slot root".into()),
    }
    let packages = root.join(CANDIDATE_PACKAGES_DIRECTORY);
    match fs::symlink_metadata(&packages) {
        Ok(_) => ensure_regular_directory(&packages, "candidate package store")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&packages).map_err(|_| "cannot create candidate package store")?;
        }
        Err(_) => return Err("cannot inspect candidate package store".into()),
    }
    let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
    match fs::symlink_metadata(&preflights) {
        Ok(_) => ensure_regular_directory(&preflights, "candidate preflight store")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&preflights).map_err(|_| "cannot create candidate preflight store")?;
        }
        Err(_) => return Err("cannot inspect candidate preflight store".into()),
    }
    Ok(())
}

fn read_slot_state(root: &Path) -> Result<CandidateSlotState, Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateSlotState::default());
        }
        Err(_) => return Err("cannot inspect candidate slot root".into()),
    }
    let path = root.join(CANDIDATE_SLOT_STATE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let contents = read_explicit_text(
                &path,
                "candidate slot state",
                MAX_CANDIDATE_SLOT_STATE_BYTES,
            )?;
            Ok(CandidateSlotState::parse(&contents)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CandidateSlotState::default())
        }
        Err(_) => Err("cannot inspect candidate slot state".into()),
    }
}

fn read_supplemental_state(
    root: &Path,
) -> Result<CandidateSupplementalState, Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "supplemental candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateSupplementalState::default());
        }
        Err(_) => return Err("cannot inspect supplemental candidate slot root".into()),
    }
    let path = root.join(CANDIDATE_SUPPLEMENTAL_STATE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let contents = read_explicit_text(
                &path,
                "supplemental candidate state",
                MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES,
            )?;
            Ok(CandidateSupplementalState::parse(&contents)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CandidateSupplementalState::default())
        }
        Err(_) => Err("cannot inspect supplemental candidate state".into()),
    }
}

fn read_exact_short_state(
    root: &Path,
) -> Result<CandidateExactShortState, Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "exact-short candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateExactShortState::default());
        }
        Err(_) => return Err("cannot inspect exact-short candidate slot root".into()),
    }
    let path = root.join(CANDIDATE_EXACT_SHORT_STATE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let contents = read_explicit_text(
                &path,
                "exact-short candidate state",
                MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES,
            )?;
            Ok(CandidateExactShortState::parse(&contents)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CandidateExactShortState::default())
        }
        Err(_) => Err("cannot inspect exact-short candidate state".into()),
    }
}

fn write_slot_state(
    root: &Path,
    state: &CandidateSlotState,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_slot_root(root)?;
    let body = state.render();
    CandidateSlotState::parse(&body)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(".slots-{}-{stamp}.tmp", std::process::id()));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = move_replace(&temporary, &root.join(CANDIDATE_SLOT_STATE_FILE));
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_supplemental_state(
    root: &Path,
    state: &CandidateSupplementalState,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_slot_root(root)?;
    let body = state.render();
    CandidateSupplementalState::parse(&body)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(".supplemental-{}-{stamp}.tmp", std::process::id()));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = move_replace(&temporary, &root.join(CANDIDATE_SUPPLEMENTAL_STATE_FILE));
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_exact_short_state(
    root: &Path,
    state: &CandidateExactShortState,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_slot_root(root)?;
    let body = state.render();
    CandidateExactShortState::parse(&body)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(".exact-short-{}-{stamp}.tmp", std::process::id()));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = move_replace(&temporary, &root.join(CANDIDATE_EXACT_SHORT_STATE_FILE));
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_path_absent(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!("{label} already exists; refusing to overwrite it").into()),
        Err(_) => Err(format!("cannot inspect explicitly named {label}").into()),
    }
}

fn ensure_regular_directory(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("cannot inspect {label}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{label} must be a regular directory").into());
    }
    Ok(())
}

fn read_explicit_text(
    path: &Path,
    label: &str,
    maximum_bytes: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| format!("cannot inspect explicitly named {label}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} cannot be a symbolic link").into());
    }
    if !metadata.is_file() {
        return Err(format!("{label} must be a regular file").into());
    }
    let maximum_u64 = u64::try_from(maximum_bytes).unwrap_or(u64::MAX);
    if metadata.len() == 0 || metadata.len() > maximum_u64 {
        return Err(format!("{label} size is outside its fixed limit").into());
    }

    let mut file = File::open(path).map_err(|_| format!("cannot open explicitly named {label}"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| format!("cannot inspect opened {label}"))?;
    if !opened_metadata.is_file()
        || opened_metadata.len() == 0
        || opened_metadata.len() > maximum_u64
    {
        return Err(format!("{label} changed to an invalid file").into());
    }

    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("cannot read explicitly named {label}"))?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(format!("{label} changed to an invalid size").into());
    }
    String::from_utf8(bytes).map_err(|_| format!("{label} must be UTF-8").into())
}

fn write_new_synced(path: &Path, contents: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source_wide = wide_path(source)?;
    let destination_wide = wide_path(destination)?;
    // SAFETY: Both NUL-terminated buffers live through the synchronous call.
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, Box<dyn std::error::Error>> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err("path contains an embedded NUL".into());
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(not(windows))]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};
    use std::sync::atomic::{AtomicU64, Ordering};
    use ziranma_core::{
        CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519, CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1,
        CandidateSourceMaterial, candidate_release_signing_message,
    };

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    const MANIFEST: &str = include_str!("../../tests/fixtures/public/demo_candidate_manifest.zcm");
    const LEXICON: &str = include_str!("../../tests/fixtures/public/demo_lexicon.tsv");
    const RIME_LEXICON: &str = "---\nname: test\n...\n亲\tqin\t6778\n清\tqing\t6000\n請\tqing\t0\n";
    const PHRASE_SOURCE: &str = "---\nname: public-phrases\n...\n\
公开短语\tgōng kāi duǎn yǔ\t30\n\
更多短语\tgèng duō duǎn yǔ\t20\n\
已有短语\tyǐ yǒu duǎn yǔ\t10\n";
    const PHRASE_ALLOWLIST: &str = "gkdy\t公开短语\ngddy\t更多短语\nyydy\t已有短语\n";
    const PHRASE_BASE: &str =
        "text\tpinyin\tfrequency\n已有短语\tyi you duan yu\t10\n基础词\tji chu ci\t9\n";
    const SHORT_CONSENSUS_SOURCE: &str = "---\nname: public-short-words\n...\n\
收束\tshōu shù\t80\n\
手术\tshǒu shù\t70\n\
首项\tshǒu xiàng\t60\n";
    const SHORT_CONSENSUS_CONFIRMATION: &str = "收束 80\n手术 70\n首项 60\n";
    const SHORT_CONSENSUS_BASE: &str = "text\tpinyin\tfrequency\n基础\tji chu\t9\n";
    const PROVENANCE: &str =
        include_str!("../../tests/fixtures/public/demo_candidate_provenance.zcp");

    fn test_declaration(payload: &str) -> PublicSourceDeclaration {
        PublicSourceDeclaration {
            id: "ziranma-demo-v1".to_owned(),
            license: "MPL-2.0".to_owned(),
            url: "https://github.com/hewzhew/ziranma-decoder".to_owned(),
            sha256: candidate_sha256_hex(payload.as_bytes()),
        }
    }

    fn phrase_material_declaration(id: &str, contents: &str) -> PublicSourceDeclaration {
        PublicSourceDeclaration {
            id: id.to_owned(),
            license: "CC-BY-4.0".to_owned(),
            url: format!("https://example.com/{id}"),
            sha256: candidate_sha256_hex(contents.as_bytes()),
        }
    }

    fn test_provenance(manifest: &str, payload: &str) -> CandidatePackageProvenance {
        let declaration = test_declaration(payload);
        CandidatePackageProvenance::from_materials(
            &declaration.id,
            &declaration.license,
            &declaration.url,
            &declaration.sha256,
            manifest,
            payload,
        )
        .unwrap()
    }

    fn package_sha256(package: &Path) -> String {
        load_public_package_directory(package)
            .unwrap()
            .authentication_sha256
    }

    fn portable_test_preflight(
        loaded: &LoadedPackage,
    ) -> Result<PreflightSummary, Box<dyn std::error::Error>> {
        let entries = parse_lexicon_tsv(&loaded.payload_text)
            .map_err(|_| "candidate package has no usable test preflight probe")?;
        let probe = entries
            .first()
            .ok_or("candidate package has no usable test preflight probe")?;
        let expected = loaded
            .snapshot
            .candidate_text(probe.code.as_str(), 1)?
            .ok_or("candidate package produced no test preflight candidate")?;
        Ok(PreflightSummary {
            revision: loaded.snapshot.revision().to_owned(),
            input_keys: probe.code.as_str().len(),
            committed_characters: expected.chars().count(),
        })
    }

    fn preflight(package: &Path) -> Result<String, Box<dyn std::error::Error>> {
        preflight_with(package, portable_test_preflight)
    }

    fn adopt(
        root: &Path,
        package: &Path,
        expected_sha256: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        adopt_with_preflight(root, package, expected_sha256, portable_test_preflight)
    }

    fn stage(
        root: &Path,
        package: &Path,
        expected_sha256: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        stage_with_preflight(root, package, expected_sha256, portable_test_preflight)
    }

    fn adopt_signed(
        root: &Path,
        package: &Path,
        signature_path: &Path,
        trusted_public_key: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let loaded = load_signature_verified_package(package, signature_path, trusted_public_key)?;
        adopt_loaded_with_preflight(root, loaded, portable_test_preflight)
    }

    fn stage_signed(
        root: &Path,
        package: &Path,
        signature_path: &Path,
        trusted_public_key: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let loaded = load_signature_verified_package(package, signature_path, trusted_public_key)?;
        stage_loaded_with_preflight(root, loaded, portable_test_preflight)
    }

    fn encode_hex(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(encoded, "{byte:02x}").unwrap();
        }
        encoded
    }

    fn test_release_signature(package_sha256: &str, seed: u8) -> (String, String) {
        // Public synthetic test material only; this is not a release key.
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let public_key = signing_key.verifying_key().to_bytes();
        let public_key_hex = encode_hex(&public_key);
        let key_sha256 = candidate_sha256_hex(&public_key);
        let message = candidate_release_signing_message(&key_sha256, package_sha256).unwrap();
        let signature = encode_hex(&signing_key.sign(&message).to_bytes());
        (
            format!(
                "schema={CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1}\n\
                 algorithm={CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519}\n\
                 key_sha256={key_sha256}\n\
                 package_sha256={package_sha256}\n\
                 signature={signature}\n"
            ),
            public_key_hex,
        )
    }

    #[test]
    fn four_character_synthetic_audit_covers_each_single_edit_shape() {
        let intended = "abcdefgh";
        let substitution = synthesize_four_character_edit(
            intended,
            SyntheticFourCharacterEdit::NeighborSubstitution,
            0,
        )
        .unwrap();
        assert_eq!(substitution.len(), intended.len());
        assert!(are_qwerty_neighbors(
            intended.as_bytes()[0],
            substitution.as_bytes()[0]
        ));
        assert_eq!(&substitution[1..], &intended[1..]);

        let transposition = synthesize_four_character_edit(
            intended,
            SyntheticFourCharacterEdit::AdjacentTransposition,
            2,
        )
        .unwrap();
        assert_eq!(transposition, "abdcefgh");

        let missing =
            synthesize_four_character_edit(intended, SyntheticFourCharacterEdit::MissingKey, 3)
                .unwrap();
        assert_eq!(missing, "abcefgh");

        let extra =
            synthesize_four_character_edit(intended, SyntheticFourCharacterEdit::ExtraKey, 4)
                .unwrap();
        assert_eq!(extra, "abcdeefgh");
    }

    #[test]
    fn short_word_synthetic_audit_covers_two_and_three_character_consensus() {
        for intended in ["keyi", "mafmmk"] {
            for sample_index in 0..=intended.len() {
                let observed =
                    synthesize_short_word_neighbor_extra_key(intended, sample_index).unwrap();
                let insertion_index = sample_index % (intended.len() + 1);
                assert_eq!(observed.len(), intended.len() + 1);
                let mut restored = observed.as_bytes().to_vec();
                let extra = restored.remove(insertion_index);
                assert_eq!(std::str::from_utf8(&restored).unwrap(), intended);
                let anchor_index = insertion_index.min(intended.len() - 1);
                assert!(are_qwerty_neighbors(
                    extra,
                    intended.as_bytes()[anchor_index]
                ));
            }
        }

        const PUBLIC: &str = "text\tpinyin\tfrequency\n\
可以\tke yi\t1000\n\
辛苦\txin ku\t900\n\
麻烦猫\tma fan mao\t800\n";
        let core = snapshot_from_payload("short-audit-core-v1", PUBLIC).unwrap();
        let supplemental = snapshot_from_payload("short-audit-supplemental-v1", PUBLIC).unwrap();
        let entries = parse_lexicon_tsv(PUBLIC).unwrap();
        let audits =
            audit_short_word_extra_key_correction_gate(&core, &supplemental, &entries, 16).unwrap();

        assert_eq!(audits[0].samples, 2);
        assert_eq!(audits[0].offered, 2);
        assert_eq!(audits[0].target_first, 2);
        assert_eq!(audits[0].target_visible, 2);
        assert_eq!(audits[1].samples, 1);
        assert_eq!(audits[1].offered, 1);
        assert_eq!(audits[1].target_first, 1);
        assert_eq!(audits[1].target_visible, 1);
        for audit in audits {
            assert_eq!(audit.conflicting_codes, 0);
            assert_eq!(audit.missing_independent_evidence, 0);
            assert_eq!(audit.no_recovery, 0);
            assert_eq!(audit.synthetic_target_code_misses, 0);
            assert_eq!(audit.clean_code_protection_failures, 0);
        }
    }

    #[test]
    fn length_coverage_parser_requires_both_public_corpus_roles() {
        assert_eq!(
            parse_options([
                "length-coverage-audit".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--challenger-payload".to_owned(),
                "challenger.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
            ])
            .unwrap(),
            Options::LengthCoverageAudit {
                base_payload: PathBuf::from("base.tsv"),
                challenger_payload: PathBuf::from("challenger.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
            }
        );
        assert!(
            parse_options([
                "length-coverage-audit".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options([
                "exact-short-prepare".to_owned(),
                "--root".to_owned(),
                "exact-root".to_owned(),
                "--core-root".to_owned(),
                "core-root".to_owned(),
                "--package".to_owned(),
                "exact-package".to_owned(),
                "--expected-sha256".to_owned(),
                "a".repeat(64),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
                "--sample-limit".to_owned(),
                "16".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .is_err(),
            "absence of a supplemental layer must be explicit"
        );
    }

    #[test]
    fn short_consensus_parser_bounds_depth_and_output() {
        assert_eq!(
            parse_options([
                "short-consensus-audit".to_owned(),
                "--source".to_owned(),
                "source.yaml".to_owned(),
                "--confirmation".to_owned(),
                "words.txt".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--per-code-depth".to_owned(),
                "2".to_owned(),
                "--entry-limit".to_owned(),
                "50000".to_owned(),
            ])
            .unwrap(),
            Options::ShortConsensusAudit {
                source: PathBuf::from("source.yaml"),
                confirmation: PathBuf::from("words.txt"),
                base_payload: PathBuf::from("base.tsv"),
                per_code_depth: 2,
                entry_limit: 50_000,
            }
        );
        assert!(
            parse_options([
                "short-consensus-audit".to_owned(),
                "--source".to_owned(),
                "source.yaml".to_owned(),
                "--confirmation".to_owned(),
                "words.txt".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--per-code-depth".to_owned(),
                "9".to_owned(),
                "--entry-limit".to_owned(),
                "1".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn consensus_audit_parser_binds_the_independent_corpus_and_frontier() {
        assert_eq!(
            parse_options([
                "consensus-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "6".to_owned(),
            ])
            .unwrap(),
            Options::ConsensusAudit {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 6,
            }
        );
        assert!(
            parse_options([
                "consensus-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "51".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn short_rank_audit_parser_binds_public_holdout_and_frontier() {
        assert_eq!(
            parse_options([
                "short-rank-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "6".to_owned(),
            ])
            .unwrap(),
            Options::ShortRankAudit {
                core_payload: PathBuf::from("core.tsv"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 6,
            }
        );
        assert!(
            parse_options([
                "short-rank-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "0".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn segment_penalty_audit_parser_binds_fit_holdout_and_bounds() {
        assert_eq!(
            parse_options([
                "segment-penalty-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "6".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
            ])
            .unwrap(),
            Options::SegmentPenaltyAudit {
                core_payload: PathBuf::from("core.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 6,
                sample_limit: 128,
            }
        );
        assert!(
            parse_options([
                "segment-penalty-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "11".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn phrase_coverage_parser_binds_both_public_materials_and_holdout() {
        assert_eq!(
            parse_options([
                "phrase-coverage-audit".to_owned(),
                "--source".to_owned(),
                "jichu.dict.yaml".to_owned(),
                "--allowlist".to_owned(),
                "chengyu.txt".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--entry-limit".to_owned(),
                "5000".to_owned(),
            ])
            .unwrap(),
            Options::PhraseCoverageAudit {
                source: PathBuf::from("jichu.dict.yaml"),
                allowlist: PathBuf::from("chengyu.txt"),
                base_payload: PathBuf::from("base.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                entry_limit: 5000,
            }
        );
        assert!(
            parse_options([
                "phrase-coverage-audit".to_owned(),
                "--entry-limit".to_owned(),
                "50001".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn phrase_layer_parser_requires_ordered_quotas_and_release_repetitions() {
        assert_eq!(
            parse_options([
                "phrase-layer-audit".to_owned(),
                "--source".to_owned(),
                "jichu.dict.yaml".to_owned(),
                "--allowlist".to_owned(),
                "chengyu.txt".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--small-limit".to_owned(),
                "5000".to_owned(),
                "--large-limit".to_owned(),
                "10000".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::PhraseLayerAudit {
                source: PathBuf::from("jichu.dict.yaml"),
                allowlist: PathBuf::from("chengyu.txt"),
                base_payload: PathBuf::from("base.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                small_limit: 5000,
                large_limit: 10000,
                repetitions: 5,
            }
        );
        for (small, large) in [(0, 10), (10, 10), (11, 10), (1, 50_001)] {
            assert!(
                parse_options([
                    "phrase-layer-audit".to_owned(),
                    "--small-limit".to_owned(),
                    small.to_string(),
                    "--large-limit".to_owned(),
                    large.to_string(),
                    "--repetitions".to_owned(),
                    "1".to_owned(),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn exact_phrase_layer_parser_binds_train_holdout_and_fixed_bounds() {
        assert_eq!(
            parse_options([
                "exact-phrase-layer-audit".to_owned(),
                "--source".to_owned(),
                "jichu.dict.yaml".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--entry-limit".to_owned(),
                "5000".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactPhraseLayerAudit {
                source: PathBuf::from("jichu.dict.yaml"),
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                entry_limit: 5000,
                repetitions: 5,
            }
        );
        for (entry_limit, repetitions) in [(0, 1), (50_001, 1), (1, 0), (1, 101)] {
            assert!(
                parse_options([
                    "exact-phrase-layer-audit".to_owned(),
                    "--entry-limit".to_owned(),
                    entry_limit.to_string(),
                    "--repetitions".to_owned(),
                    repetitions.to_string(),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn exact_phrase_layer_preflight_parser_binds_three_packages_and_workload() {
        assert_eq!(
            parse_options([
                "exact-phrase-layer-preflight".to_owned(),
                "--core-package".to_owned(),
                "core".to_owned(),
                "--supplemental-package".to_owned(),
                "supplemental".to_owned(),
                "--phrase-package".to_owned(),
                "phrase".to_owned(),
                "--sample-limit".to_owned(),
                "16".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactPhraseLayerPreflight {
                core_package: PathBuf::from("core"),
                supplemental_package: PathBuf::from("supplemental"),
                phrase_package: PathBuf::from("phrase"),
                sample_limit: 16,
                repetitions: 5,
            }
        );
        for (sample_limit, repetitions) in [(0, 1), (33, 1), (1, 0), (1, 21)] {
            assert!(
                parse_options([
                    "exact-phrase-layer-preflight".to_owned(),
                    "--sample-limit".to_owned(),
                    sample_limit.to_string(),
                    "--repetitions".to_owned(),
                    repetitions.to_string(),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn exact_phrase_tsf_preflight_parser_binds_only_bounded_public_packages() {
        assert_eq!(
            parse_options([
                "exact-phrase-tsf-preflight".to_owned(),
                "--core-package".to_owned(),
                "core".to_owned(),
                "--supplemental-package".to_owned(),
                "supplemental".to_owned(),
                "--phrase-package".to_owned(),
                "phrase".to_owned(),
                "--sample-limit".to_owned(),
                "32".to_owned(),
                "--repetitions".to_owned(),
                "20".to_owned(),
            ])
            .unwrap(),
            Options::ExactPhraseTsfPreflight {
                core_package: PathBuf::from("core"),
                supplemental_package: PathBuf::from("supplemental"),
                phrase_package: PathBuf::from("phrase"),
                sample_limit: 32,
                repetitions: 20,
            }
        );
        for (sample_limit, repetitions) in [(0, 1), (33, 1), (1, 0), (1, 21)] {
            assert!(
                parse_options([
                    "exact-phrase-tsf-preflight".to_owned(),
                    "--sample-limit".to_owned(),
                    sample_limit.to_string(),
                    "--repetitions".to_owned(),
                    repetitions.to_string(),
                ])
                .is_err()
            );
        }
        assert!(
            parse_options([
                "exact-phrase-tsf-preflight".to_owned(),
                "--unknown".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn exact_phrase_popup_preflight_parser_keeps_visible_work_bounded() {
        assert_eq!(
            parse_options([
                "exact-phrase-popup-preflight".to_owned(),
                "--core-package".to_owned(),
                "core".to_owned(),
                "--supplemental-package".to_owned(),
                "supplemental".to_owned(),
                "--phrase-package".to_owned(),
                "phrase".to_owned(),
                "--sample-limit".to_owned(),
                "4".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactPhrasePopupPreflight {
                core_package: PathBuf::from("core"),
                supplemental_package: PathBuf::from("supplemental"),
                phrase_package: PathBuf::from("phrase"),
                sample_limit: 4,
                repetitions: 5,
            }
        );
        for (sample_limit, repetitions) in [(0, 1), (5, 1), (1, 0), (1, 6)] {
            assert!(
                parse_options([
                    "exact-phrase-popup-preflight".to_owned(),
                    "--sample-limit".to_owned(),
                    sample_limit.to_string(),
                    "--repetitions".to_owned(),
                    repetitions.to_string(),
                ])
                .is_err()
            );
        }
        assert!(
            parse_options([
                "exact-phrase-popup-preflight".to_owned(),
                "--unknown".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn phrase_layer_build_parser_requires_all_three_public_materials() {
        let arguments = vec![
            "build-phrase-layer",
            "--source",
            "jichu.dict.yaml",
            "--allowlist",
            "chengyu.txt",
            "--base-payload",
            "base.tsv",
            "--output",
            "package",
            "--revision",
            "phrase-layer-v1",
            "--entry-limit",
            "10000",
            "--source-id",
            "wanxiang-jichu",
            "--source-license",
            "CC-BY-4.0",
            "--source-url",
            "https://example.com/jichu",
            "--source-sha256",
            &"1".repeat(64),
            "--allowlist-id",
            "wanxiang-chengyu",
            "--allowlist-license",
            "CC-BY-4.0",
            "--allowlist-url",
            "https://example.com/chengyu",
            "--allowlist-sha256",
            &"2".repeat(64),
            "--base-id",
            "ziranma-base",
            "--base-license",
            "CC-BY-4.0",
            "--base-url",
            "https://example.com/base",
            "--base-sha256",
            &"3".repeat(64),
            "--public",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let Options::BuildPhraseLayer(parsed) = parse_options(arguments.clone()).unwrap() else {
            panic!("expected build-phrase-layer options");
        };
        assert_eq!(parsed.entry_limit, 10000);

        let without_public = arguments[..arguments.len() - 1].to_vec();
        assert!(parse_options(without_public).is_err());
        let without_base_hash = arguments
            .iter()
            .enumerate()
            .filter(|(index, _)| ![arguments.len() - 3, arguments.len() - 2].contains(index))
            .map(|(_, argument)| argument.clone())
            .collect::<Vec<_>>();
        assert!(parse_options(without_base_hash).is_err());
    }

    #[test]
    fn exact_phrase_layer_build_parser_requires_four_pins_and_public_intent() {
        let arguments = [
            "build-exact-phrase-layer",
            "--source",
            "source.yaml",
            "--core-payload",
            "core.tsv",
            "--supplemental-payload",
            "supplemental.tsv",
            "--fit-corpus",
            "train.conllu",
            "--output",
            "package",
            "--revision",
            "exact-phrase-v1",
            "--entry-limit",
            "5000",
            "--source-id",
            "wanxiang-jichu",
            "--source-license",
            "CC-BY-4.0",
            "--source-url",
            "https://example.com/source",
            "--source-sha256",
            "1111111111111111111111111111111111111111111111111111111111111111",
            "--core-id",
            "rime-core",
            "--core-license",
            "Apache-2.0",
            "--core-url",
            "https://example.com/core",
            "--core-sha256",
            "2222222222222222222222222222222222222222222222222222222222222222",
            "--supplemental-id",
            "wanxiang-top100k",
            "--supplemental-license",
            "CC-BY-4.0",
            "--supplemental-url",
            "https://example.com/supplemental",
            "--supplemental-sha256",
            "3333333333333333333333333333333333333333333333333333333333333333",
            "--fit-id",
            "ud-gsdsimp-train",
            "--fit-license",
            "CC-BY-SA-4.0",
            "--fit-url",
            "https://example.com/train",
            "--fit-sha256",
            "4444444444444444444444444444444444444444444444444444444444444444",
            "--public",
        ]
        .map(str::to_owned)
        .to_vec();

        let Options::BuildExactPhraseLayer(parsed) = parse_options(arguments.clone()).unwrap()
        else {
            panic!("expected build-exact-phrase-layer options");
        };
        assert_eq!(parsed.entry_limit, 5000);
        assert_eq!(parsed.source, PathBuf::from("source.yaml"));
        assert_eq!(parsed.core_declaration.id, "rime-core");
        assert_eq!(parsed.supplemental_declaration.id, "wanxiang-top100k");
        assert_eq!(parsed.fit_declaration.id, "ud-gsdsimp-train");
        assert!(parse_options(arguments[..arguments.len() - 1].to_vec()).is_err());
    }

    #[test]
    fn short_consensus_build_parser_requires_three_pins_and_public_intent() {
        let arguments = vec![
            "build-short-consensus-layer",
            "--source",
            "jichu.dict.yaml",
            "--confirmation",
            "words.txt",
            "--base-payload",
            "base.tsv",
            "--output",
            "package",
            "--revision",
            "short-consensus-v1",
            "--per-code-depth",
            "2",
            "--entry-limit",
            "50000",
            "--source-id",
            "wanxiang-jichu",
            "--source-license",
            "CC-BY-4.0",
            "--source-url",
            "https://example.com/jichu",
            "--source-sha256",
            &"1".repeat(64),
            "--confirmation-id",
            "jieba-words",
            "--confirmation-license",
            "MIT",
            "--confirmation-url",
            "https://example.com/jieba",
            "--confirmation-sha256",
            &"2".repeat(64),
            "--base-id",
            "ziranma-base",
            "--base-license",
            "MPL-2.0",
            "--base-url",
            "https://example.com/base",
            "--base-sha256",
            &"3".repeat(64),
            "--public",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let Options::BuildShortConsensusLayer(parsed) = parse_options(arguments.clone()).unwrap()
        else {
            panic!("expected build-short-consensus-layer options");
        };
        assert_eq!(parsed.per_code_depth, 2);
        assert_eq!(parsed.entry_limit, 50_000);
        assert!(parse_options(arguments[..arguments.len() - 1].to_vec()).is_err());
    }

    #[test]
    fn exact_short_query_parser_requires_one_complete_code() {
        assert_eq!(
            parse_options(
                [
                    "exact-short-query",
                    "--package",
                    "package",
                    "--code",
                    "ubuu",
                    "--limit",
                    "8",
                ]
                .map(str::to_owned),
            )
            .unwrap(),
            Options::ExactShortQuery {
                package: PathBuf::from("package"),
                code: "ubuu".to_owned(),
                limit: 8,
            }
        );
        assert!(
            parse_options(
                [
                    "exact-short-query",
                    "--package",
                    "package",
                    "--code",
                    "ubu",
                    "--limit",
                    "8",
                ]
                .map(str::to_owned),
            )
            .is_err()
        );
        assert_eq!(
            parse_options(
                [
                    "exact-short-benchmark",
                    "--package",
                    "package",
                    "--code",
                    "ubuu",
                    "--repetitions",
                    "1000",
                ]
                .map(str::to_owned),
            )
            .unwrap(),
            Options::ExactShortBenchmark {
                package: PathBuf::from("package"),
                code: "ubuu".to_owned(),
                repetitions: 1000,
            }
        );
    }

    #[test]
    fn exact_short_layer_audit_parser_binds_public_materials_and_page_size() {
        assert_eq!(
            parse_options([
                "exact-short-layer-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--exact-package".to_owned(),
                "exact-package".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "7".to_owned(),
                "--supplemental-promotions".to_owned(),
                "1".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortLayerAudit {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                exact_package: PathBuf::from("exact-package"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 7,
                supplemental_promotions: 1,
            }
        );
        assert!(
            parse_options([
                "exact-short-layer-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "1".to_owned(),
                "--supplemental-promotions".to_owned(),
                "1".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn exact_short_layer_benchmark_parser_binds_the_guarded_release_workload() {
        assert_eq!(
            parse_options([
                "exact-short-layer-benchmark".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--exact-package".to_owned(),
                "exact-package".to_owned(),
                "--frontier-limit".to_owned(),
                "7".to_owned(),
                "--supplemental-promotions".to_owned(),
                "1".to_owned(),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
                "--candidate-limit".to_owned(),
                "14".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortLayerBenchmark {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                exact_package: PathBuf::from("exact-package"),
                frontier_limit: 7,
                supplemental_promotions: 1,
                exact_promotions: 2,
                candidate_limit: 14,
                sample_limit: 128,
                repetitions: 5,
            }
        );
        for (flag, value) in [
            ("--frontier-limit", "1"),
            ("--exact-promotions", "9"),
            ("--candidate-limit", "11"),
            ("--sample-limit", "7"),
            ("--repetitions", "101"),
        ] {
            assert!(
                parse_options([
                    "exact-short-layer-benchmark".to_owned(),
                    flag.to_owned(),
                    value.to_owned(),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn exact_short_tsf_preflight_parser_binds_only_bounded_public_packages() {
        assert_eq!(
            parse_options([
                "exact-short-tsf-preflight".to_owned(),
                "--core-package".to_owned(),
                "core-package".to_owned(),
                "--supplemental-package".to_owned(),
                "supplemental-package".to_owned(),
                "--supplemental-promotions".to_owned(),
                "1".to_owned(),
                "--exact-package".to_owned(),
                "exact-package".to_owned(),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
                "--sample-limit".to_owned(),
                "8".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortTsfPreflight {
                core_package: PathBuf::from("core-package"),
                supplemental_package: Some(PathBuf::from("supplemental-package")),
                exact_package: PathBuf::from("exact-package"),
                supplemental_promotions: Some(1),
                exact_promotions: 2,
                sample_limit: 8,
                repetitions: 5,
            }
        );
        assert_eq!(
            parse_options([
                "exact-short-tsf-preflight".to_owned(),
                "--core-package".to_owned(),
                "core-package".to_owned(),
                "--exact-package".to_owned(),
                "exact-package".to_owned(),
                "--exact-promotions".to_owned(),
                "1".to_owned(),
                "--sample-limit".to_owned(),
                "1".to_owned(),
                "--repetitions".to_owned(),
                "1".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortTsfPreflight {
                core_package: PathBuf::from("core-package"),
                supplemental_package: None,
                exact_package: PathBuf::from("exact-package"),
                supplemental_promotions: None,
                exact_promotions: 1,
                sample_limit: 1,
                repetitions: 1,
            }
        );
        assert!(
            parse_options([
                "exact-short-tsf-preflight".to_owned(),
                "--supplemental-package".to_owned(),
                "supplemental-package".to_owned(),
            ])
            .is_err()
        );
        for (flag, value) in [
            ("--supplemental-promotions", "0"),
            ("--exact-promotions", "9"),
            ("--sample-limit", "33"),
            ("--repetitions", "21"),
        ] {
            assert!(
                parse_options([
                    "exact-short-tsf-preflight".to_owned(),
                    flag.to_owned(),
                    value.to_owned(),
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn exact_short_prepare_parser_keeps_preparation_explicit_and_bounded() {
        assert_eq!(
            parse_options([
                "exact-short-prepare".to_owned(),
                "--root".to_owned(),
                "exact-root".to_owned(),
                "--core-root".to_owned(),
                "core-root".to_owned(),
                "--supplemental-root".to_owned(),
                "supplement-root".to_owned(),
                "--package".to_owned(),
                "exact-package".to_owned(),
                "--expected-sha256".to_owned(),
                "a".repeat(64),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
                "--sample-limit".to_owned(),
                "16".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortPrepare {
                root: PathBuf::from("exact-root"),
                core_root: PathBuf::from("core-root"),
                supplemental_root: Some(PathBuf::from("supplement-root")),
                package: PathBuf::from("exact-package"),
                expected_sha256: "a".repeat(64),
                exact_promotions: 2,
                sample_limit: 16,
                repetitions: 5,
            }
        );
        assert!(
            parse_options([
                "exact-short-prepare".to_owned(),
                "--sample-limit".to_owned(),
                "32".to_owned(),
                "--repetitions".to_owned(),
                "21".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn exact_short_readiness_parser_binds_the_complete_public_context() {
        assert_eq!(
            parse_options([
                "exact-short-readiness".to_owned(),
                "--root".to_owned(),
                "exact-root".to_owned(),
                "--core-root".to_owned(),
                "core-root".to_owned(),
                "--supplemental-root".to_owned(),
                "supplement-root".to_owned(),
                "--package".to_owned(),
                "exact-package".to_owned(),
                "--expected-sha256".to_owned(),
                "a".repeat(64),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortReadiness {
                root: PathBuf::from("exact-root"),
                core_root: PathBuf::from("core-root"),
                supplemental_root: Some(PathBuf::from("supplement-root")),
                package: PathBuf::from("exact-package"),
                expected_sha256: "a".repeat(64),
                exact_promotions: 2,
            }
        );
        assert!(
            parse_options([
                "exact-short-readiness".to_owned(),
                "--without-supplement".to_owned(),
                "--supplemental-root".to_owned(),
                "supplement-root".to_owned(),
            ])
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn exact_short_tsf_preflight_debug_guard_runs_before_opening_packages() {
        let error = preflight_exact_short_tsf(ExactShortTsfPreflightRequest {
            core_package: Path::new("must-not-be-opened-core"),
            supplemental_package: None,
            exact_package: Path::new("must-not-be-opened-exact"),
            supplemental_promotions: None,
            exact_promotions: 1,
            sample_limit: 1,
            repetitions: 1,
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "exact-short-tsf-preflight must run from a release build"
        );
    }

    #[test]
    fn popup_render_preflight_parser_accepts_only_the_fixed_workload_bound() {
        assert_eq!(
            parse_options([
                "popup-render-preflight".to_owned(),
                "--repetitions".to_owned(),
                "5".to_owned(),
            ])
            .unwrap(),
            Options::PopupRenderPreflight { repetitions: 5 }
        );
        for arguments in [
            vec!["popup-render-preflight".to_owned()],
            vec![
                "popup-render-preflight".to_owned(),
                "--repetitions".to_owned(),
                "0".to_owned(),
            ],
            vec![
                "popup-render-preflight".to_owned(),
                "--repetitions".to_owned(),
                "21".to_owned(),
            ],
            vec!["popup-render-preflight".to_owned(), "--unknown".to_owned()],
        ] {
            assert!(parse_options(arguments).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn popup_render_preflight_debug_guard_runs_before_creating_a_window() {
        let error = preflight_popup_rendering(1).unwrap_err();
        assert_eq!(
            error.to_string(),
            "popup render preflight must run from a release build"
        );
    }

    #[test]
    fn exact_short_tsf_preflight_report_contains_only_aggregate_identity_and_timings() {
        let duration = DurationSummary {
            samples: 40,
            median: Duration::from_micros(1_250),
            p95: Duration::from_micros(2_500),
            p99: Duration::from_micros(3_750),
            maximum: Duration::from_micros(5_000),
        };
        let report = render_exact_short_tsf_preflight_report(&ExactShortTsfPreflightSummary {
            core: ExactShortTsfLayerIdentity {
                revision: "public-core-v1".to_owned(),
                authentication_sha256: "a".repeat(64),
                load_duration: Duration::from_millis(10),
            },
            supplemental: None,
            exact: ExactShortTsfLayerIdentity {
                revision: "public-exact-v1".to_owned(),
                authentication_sha256: "b".repeat(64),
                load_duration: Duration::from_millis(5),
            },
            exact_promotions: 2,
            requested_probes: 8,
            inspected_codes: 9,
            repetitions: 5,
            first_page: duration,
            second_page: duration,
            commit: duration,
            to_second_page: duration,
            complete_path: duration,
        });
        assert!(report.contains("计时样本 40"));
        assert!(report.contains("median 1.250 ms"));
        assert!(report.contains("本次操作只读"));
        assert!(!report.contains("ubuu"));
        assert!(!report.contains("收束"));
    }

    #[cfg(windows)]
    #[test]
    fn exact_phrase_tsf_preflight_debug_guard_runs_before_opening_packages() {
        let error = preflight_exact_phrase_tsf(ExactPhraseTsfPreflightRequest {
            core_package: Path::new("must-not-be-opened-core"),
            supplemental_package: Path::new("must-not-be-opened-supplemental"),
            phrase_package: Path::new("must-not-be-opened-phrase"),
            sample_limit: 1,
            repetitions: 1,
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "exact-phrase-tsf-preflight must run from a release build"
        );
    }

    #[cfg(windows)]
    #[test]
    fn exact_phrase_popup_preflight_debug_guard_runs_before_opening_or_showing() {
        let error = preflight_exact_phrase_popup(ExactPhrasePopupPreflightRequest {
            core_package: Path::new("must-not-be-opened-core"),
            supplemental_package: Path::new("must-not-be-opened-supplemental"),
            phrase_package: Path::new("must-not-be-opened-phrase"),
            sample_limit: 1,
            repetitions: 1,
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "exact-phrase-popup-preflight must run from a release build"
        );
    }

    #[test]
    fn exact_phrase_tsf_preflight_report_contains_only_aggregate_evidence() {
        let duration = DurationSummary {
            samples: 40,
            median: Duration::from_micros(1_250),
            p95: Duration::from_micros(2_500),
            p99: Duration::from_micros(3_750),
            maximum: Duration::from_micros(5_000),
        };
        let identity = |label: &str, digest: char| ExactPhraseTsfLayerIdentity {
            revision: format!("public-{label}-v1"),
            authentication_sha256: digest.to_string().repeat(64),
            load_duration: Duration::from_millis(5),
        };
        let report = render_exact_phrase_tsf_preflight_report(&ExactPhraseTsfPreflightSummary {
            core: identity("core", 'a'),
            supplemental: identity("supplemental", 'b'),
            phrase: identity("phrase", 'c'),
            phrase_entries: 2_799,
            phrase_codes: 2_799,
            requested_probes: 8,
            inspected_codes: 11,
            repetitions: 5,
            target_rank_samples: [20, 10, 5, 3, 1, 1],
            first_page: duration,
            commit: duration,
            complete_path: duration,
        });
        assert!(report.contains("计时样本 40"));
        assert!(report.contains("第 1 项 20"));
        assert!(report.contains("median 1.250 ms"));
        assert!(report.contains("不写槽位、状态或运行时凭据"));
        assert!(!report.contains("zljnll"));
        assert!(!report.contains("再进来"));
    }

    #[test]
    fn public_package_merge_parser_requires_explicit_public_inputs() {
        let arguments = [
            "merge-public-packages",
            "--base",
            "base-package",
            "--overlay",
            "overlay-package",
            "--output",
            "merged-package",
            "--revision",
            "merged-v1",
            "--public",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse_options(arguments.clone()).unwrap(),
            Options::MergePublicPackages {
                base: PathBuf::from("base-package"),
                overlay: PathBuf::from("overlay-package"),
                output: PathBuf::from("merged-package"),
                revision: "merged-v1".to_owned(),
            }
        );
        assert!(parse_options(arguments[..arguments.len() - 1].to_vec()).is_err());
    }

    #[test]
    fn public_miss_diagnosis_parser_requires_explicit_public_target() {
        let arguments = [
            "diagnose-public-miss",
            "--source",
            "jichu.dict.yaml",
            "--core-package",
            "core-package",
            "--supplemental-package",
            "supplemental-package",
            "--code",
            "bgdr",
            "--text",
            "绷断",
            "--public",
        ]
        .map(str::to_owned);
        assert_eq!(
            parse_options(arguments.clone()).unwrap(),
            Options::DiagnosePublicMiss {
                source: PathBuf::from("jichu.dict.yaml"),
                core_package: PathBuf::from("core-package"),
                supplemental_package: PathBuf::from("supplemental-package"),
                code: "bgdr".to_owned(),
                text: "绷断".to_owned(),
            }
        );
        assert!(parse_options(arguments[..arguments.len() - 1].to_vec()).is_err());
    }

    #[test]
    fn static_context_parser_binds_model_fit_and_independent_holdout() {
        assert_eq!(
            parse_options([
                "static-context-audit".to_owned(),
                "--model".to_owned(),
                "public.arpa".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "32".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .unwrap(),
            Options::StaticContextAudit {
                model: PathBuf::from("public.arpa"),
                core_payload: PathBuf::from("core.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 32,
                sample_limit: 128,
                max_order: 3,
            }
        );
        assert!(
            parse_options([
                "static-context-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "4".to_owned(),
                "--sample-limit".to_owned(),
                "1".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn single_character_context_parser_binds_model_fit_and_holdout() {
        assert_eq!(
            parse_options([
                "single-character-context-audit".to_owned(),
                "--model".to_owned(),
                "public.arpa".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--fit-corpus".to_owned(),
                "train.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "test.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "32".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .unwrap(),
            Options::SingleCharacterContextAudit {
                model: PathBuf::from("public.arpa"),
                core_payload: PathBuf::from("core.tsv"),
                fit_corpus: PathBuf::from("train.conllu"),
                held_out_corpus: PathBuf::from("test.conllu"),
                frontier_limit: 32,
                sample_limit: 128,
                max_order: 3,
            }
        );
        assert!(
            parse_options([
                "single-character-context-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "4".to_owned(),
                "--sample-limit".to_owned(),
                "1".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn single_character_context_validation_parser_binds_development_and_final_holdout() {
        assert_eq!(
            parse_options([
                "single-character-context-validation-audit".to_owned(),
                "--model".to_owned(),
                "public.arpa".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--development-corpus".to_owned(),
                "dev.conllu".to_owned(),
                "--held-out-corpus".to_owned(),
                "holdout.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "50".to_owned(),
                "--sample-limit".to_owned(),
                "512".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .unwrap(),
            Options::SingleCharacterContextValidationAudit {
                model: PathBuf::from("public.arpa"),
                core_payload: PathBuf::from("core.tsv"),
                development_corpus: PathBuf::from("dev.conllu"),
                held_out_corpus: PathBuf::from("holdout.conllu"),
                frontier_limit: 50,
                sample_limit: 512,
                max_order: 3,
            }
        );
        assert!(
            parse_options([
                "single-character-context-validation-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "51".to_owned(),
                "--sample-limit".to_owned(),
                "1".to_owned(),
                "--max-order".to_owned(),
                "3".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn single_character_context_profiles_extend_only_the_fit_search() {
        let shared = static_context_profiles();
        let single = single_character_context_profiles();
        assert_eq!(&single[..shared.len()], shared.as_slice());
        assert!(
            single.iter().any(|profile| {
                profile.search_depth == 8 && profile.minimum_average_gain == 1.25
            })
        );
        assert!(
            single.iter().any(|profile| {
                profile.search_depth == 50 && profile.minimum_average_gain == 4.0
            })
        );
    }

    #[test]
    fn strict_context_profile_selection_rejects_any_collateral_top_one_change() {
        let baseline = StaticContextProfileReport::default();
        let unsafe_gain = StaticContextProfileReport {
            correct_top_one_gained: 20,
            non_target_top_one_changes: 1,
            ..StaticContextProfileReport::default()
        };
        let safe_gain = StaticContextProfileReport {
            correct_top_one_gained: 1,
            ..StaticContextProfileReport::default()
        };
        let safer_larger_gain = StaticContextProfileReport {
            correct_top_one_gained: 2,
            ..StaticContextProfileReport::default()
        };

        assert!(!static_context_safe_profile_precedes(
            &unsafe_gain,
            &baseline
        ));
        assert!(static_context_safe_profile_precedes(&safe_gain, &baseline));
        assert!(static_context_safe_profile_precedes(
            &safer_larger_gain,
            &safe_gain
        ));
    }

    #[test]
    fn public_package_query_parser_is_bounded() {
        assert_eq!(
            parse_options([
                "package-query".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--code".to_owned(),
                "bgdr".to_owned(),
                "--limit".to_owned(),
                "7".to_owned(),
            ])
            .unwrap(),
            Options::PackageQuery {
                package: PathBuf::from("package"),
                code: "bgdr".to_owned(),
                limit: 7,
            }
        );
        assert!(
            parse_options([
                "package-query".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--code".to_owned(),
                "BGDR".to_owned(),
                "--limit".to_owned(),
                "7".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn length_coverage_report_is_aggregate_only_and_requires_holdout() {
        const BASE: &str = "text\tpinyin\tfrequency\n双字\tshuang zi\t10\n三字词\tsan zi ci\t9\n";
        const CHALLENGER: &str =
            "text\tpinyin\tfrequency\n双字\tshuang zi\t10\n四字词语\tsi zi ci yu\t9\n";
        const FIT: &str = "# sent_id = fit\n1\t双字\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\n";
        const HELD_OUT: &str = "# sent_id = held-out\n1\t三字词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n2\t四字词语\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let base = root.join("base.tsv");
        let challenger = root.join("challenger.tsv");
        let fit = root.join("fit.conllu");
        let held_out = root.join("held-out.conllu");
        fs::write(&base, BASE).unwrap();
        fs::write(&challenger, CHALLENGER).unwrap();
        fs::write(&fit, FIT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();

        let report = audit_length_coverage(&base, &challenger, &fit, &held_out).unwrap();
        assert!(report.contains("训练侧参考：1 句"));
        assert!(report.contains("留出评测：1 句"));
        assert!(report.contains("新增 1（实例 1），丢失 0（实例 0）"));
        assert!(report.contains("新增 0（实例 0），丢失 1（实例 1）"));
        assert!(!report.contains("三字词"));
        assert!(!report.contains("四字词语"));
        assert!(audit_length_coverage(&base, &challenger, &fit, &fit).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_consensus_report_is_aggregate_read_only_and_does_not_echo_words() {
        const SOURCE: &str = "---\n...\n公开词\tgōng kāi cí\t30\n收束\tshōu shù\t20\n";
        const CONFIRMATION: &str = "收束 10 v\n无关 5 n\n";
        const BASE: &str = "text\tpinyin\tfrequency\n根基\tgen ji\t10\n";
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.yaml");
        let confirmation = root.join("confirmation.txt");
        let base = root.join("base.tsv");
        fs::write(&source, SOURCE).unwrap();
        fs::write(&confirmation, CONFIRMATION).unwrap();
        fs::write(&base, BASE).unwrap();

        let report = audit_short_word_consensus(&source, &confirmation, &base, 1, 10).unwrap();
        assert!(report.contains("合格双字词面 2"));
        assert!(report.contains("新增身份 1；新增规范码 1"));
        assert!(report.contains("本次操作：只读；没有生成、安装或启用候选包"));
        assert!(!report.contains("收束"));
        assert!(!report.contains("根基"));
        assert_eq!(root.read_dir().unwrap().count(), 3);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phrase_coverage_report_is_aggregate_only_and_does_not_build_a_package() {
        const SOURCE: &str = "---\n...\n公开短语\tgōng kāi duǎn yǔ\t20\n";
        const ALLOWLIST: &str = "gkdy\t公开短语\n";
        const BASE: &str = "text\tpinyin\tfrequency\n基础词\tji chu ci\t10\n";
        const FIT: &str = "# sent_id = fit\n1\t公开短语\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\n";
        const HELD_OUT: &str =
            "# sent_id = held-out\n1\t公开短语\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\n";
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.yaml");
        let allowlist = root.join("phrases.txt");
        let base = root.join("base.tsv");
        let fit = root.join("fit.conllu");
        let held_out = root.join("held-out.conllu");
        fs::write(&source, SOURCE).unwrap();
        fs::write(&allowlist, ALLOWLIST).unwrap();
        fs::write(&base, BASE).unwrap();
        fs::write(&fit, FIT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();

        let report = audit_phrase_coverage(&source, &allowlist, &base, &fit, &held_out, 1).unwrap();
        assert!(report.contains("合格四字词面 1"));
        assert!(report.contains("新增 1（实例 1），丢失 0（实例 0）"));
        assert!(!report.contains("公开短语"));
        assert!(audit_phrase_coverage(&source, &allowlist, &base, &fit, &fit, 1).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phrase_layer_rank_comparison_separates_new_targets_from_base_controls() {
        const CORE: &str = "text\tpinyin\tfrequency\n基础短语\tji chu duan yu\t100\n";
        const SMALL: &str = "text\tpinyin\tfrequency\n公开短语\tgong kai duan yu\t100\n";
        const LARGE: &str = "text\tpinyin\tfrequency\n\
公开短语\tgong kai duan yu\t100\n\
更多短语\tgeng duo duan yu\t90\n";
        let core = snapshot_from_payload("phrase-test-core-v1", CORE).unwrap();
        let small = snapshot_from_payload("phrase-test-small-v1", SMALL).unwrap();
        let large = snapshot_from_payload("phrase-test-large-v1", LARGE).unwrap();
        let targets = [
            PublicLexiconRankProbe {
                observed: encode_pinyin_phrase("gong kai duan yu").unwrap().full_code,
                expected_text: "公开短语".to_owned(),
                instances: 2,
            },
            PublicLexiconRankProbe {
                observed: encode_pinyin_phrase("geng duo duan yu").unwrap().full_code,
                expected_text: "更多短语".to_owned(),
                instances: 1,
            },
        ];
        let target_report = compare_phrase_layer_ranks(&core, &small, &large, &targets).unwrap();
        assert_eq!(target_report.probes, 2);
        assert_eq!(target_report.instances, 3);
        assert_eq!(target_report.core.at_ten, 0);
        assert_eq!(target_report.small.at_ten, 1);
        assert_eq!(target_report.large.at_ten, 2);
        assert_eq!(target_report.small_vs_core.newly_visible, 1);
        assert_eq!(target_report.large_vs_core.newly_visible, 2);

        let controls = [PublicLexiconRankProbe {
            observed: encode_pinyin_phrase("ji chu duan yu").unwrap().full_code,
            expected_text: "基础短语".to_owned(),
            instances: 3,
        }];
        let control_report = compare_phrase_layer_ranks(&core, &small, &large, &controls).unwrap();
        assert_eq!(control_report.small_top_changes, 0);
        assert_eq!(control_report.large_top_changes, 0);
        assert_eq!(control_report.small_vs_core.worsened, 0);
        assert_eq!(control_report.large_vs_core.worsened, 0);
    }

    #[test]
    fn exact_phrase_source_selection_excludes_ambiguity_existing_and_same_code_rivals() {
        const SOURCE: &str = "---\n...\n\
再进来\tzài jìn lái\t20\n\
载进来\tzǎi jìn lái\t30\n\
新版本\txīn bǎn běn\t40\n\
重来了\tzhòng lái le\t50\n\
重来了\tchóng lái le\t45\n\
好使用\thǎo shǐ yòng\t35\n";
        let code = |pinyin: &str| encode_pinyin_phrase(pinyin).unwrap().full_code;
        let probes = vec![
            PublicLexiconRankProbe {
                observed: code("zai jin lai"),
                expected_text: "再进来".to_owned(),
                instances: 5,
            },
            PublicLexiconRankProbe {
                observed: code("zai jin lai"),
                expected_text: "载进来".to_owned(),
                instances: 2,
            },
            PublicLexiconRankProbe {
                observed: code("xin ban ben"),
                expected_text: "新版本".to_owned(),
                instances: 3,
            },
            PublicLexiconRankProbe {
                observed: code("zhong lai le"),
                expected_text: "重来了".to_owned(),
                instances: 4,
            },
            PublicLexiconRankProbe {
                observed: code("hao shi yong"),
                expected_text: "好使用".to_owned(),
                instances: 1,
            },
        ];
        let existing =
            parse_lexicon_tsv("text\tpinyin\tfrequency\n新版本\txin ban ben\t100\n").unwrap();

        let selected = select_exact_phrase_source_entries(SOURCE, &probes, &existing, 10).unwrap();

        assert_eq!(selected.entries.len(), 1);
        assert_eq!(selected.entries[0].text, "好使用");
        assert_eq!(selected.stats.matched_identities, 5);
        assert_eq!(selected.stats.ambiguous_surfaces, 1);
        assert_eq!(selected.stats.ambiguous_codes, 1);
        assert_eq!(selected.stats.existing_identities, 1);
        assert_eq!(selected.stats.code_collisions, 0);
    }

    #[test]
    fn exact_phrase_preview_only_leads_when_no_existing_whole_word_exists() {
        const CORE_COMPOSED: &str = "text\tpinyin\tfrequency\n\
在\tzai\t100\n进来\tjin lai\t90\n";
        const CORE_EXACT: &str = "text\tpinyin\tfrequency\n\
在进来\tzai jin lai\t100\n在\tzai\t90\n进来\tjin lai\t80\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n其他词\tqi ta ci\t10\n";
        const PHRASE: &str = "text\tpinyin\tfrequency\n再进来\tzai jin lai\t20\n";
        let composed = snapshot_from_payload("exact-phrase-composed", CORE_COMPOSED).unwrap();
        let exact = snapshot_from_payload("exact-phrase-existing", CORE_EXACT).unwrap();
        let supplemental =
            snapshot_from_payload("exact-phrase-supplemental", SUPPLEMENTAL).unwrap();
        let phrase = snapshot_from_payload("exact-phrase-new", PHRASE).unwrap();
        let code = encode_pinyin_phrase("zai jin lai").unwrap().full_code;

        let promoted =
            preview_exact_phrase_candidates(&composed, &supplemental, &phrase, code.as_str(), 10)
                .unwrap();
        assert_eq!(promoted.first().map(String::as_str), Some("再进来"));

        let guarded =
            preview_exact_phrase_candidates(&exact, &supplemental, &phrase, code.as_str(), 10)
                .unwrap();
        assert_eq!(guarded.first().map(String::as_str), Some("在进来"));
        assert_eq!(guarded.get(1).map(String::as_str), Some("再进来"));

        let promoted_observation = inspect_exact_phrase_preview(
            code.as_str(),
            "再进来",
            &composed,
            &supplemental,
            &phrase,
        )
        .unwrap();
        let guarded_observation =
            inspect_exact_phrase_preview(code.as_str(), "再进来", &exact, &supplemental, &phrase)
                .unwrap();
        assert_eq!(promoted_observation.existing_exact_prefix, 0);
        assert_eq!(promoted_observation.target_rank, Some(1));
        assert_eq!(guarded_observation.existing_exact_prefix, 1);
        assert_eq!(guarded_observation.target_rank, Some(2));

        let mut audit = ExactPhraseCatalogAudit::default();
        audit.observe(promoted_observation);
        audit.observe(guarded_observation);
        let audit = audit.verify().unwrap();
        assert_eq!(audit.targets, 2);
        assert_eq!(audit.without_existing_exact_prefix, 1);
        assert_eq!(audit.after_existing_exact_prefix, 1);
        assert_eq!(audit.target_ranks, [1, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn exact_phrase_catalog_audit_fails_closed_with_aggregate_counts_only() {
        let mut audit = ExactPhraseCatalogAudit::default();
        audit.observe(ExactPhrasePreviewObservation {
            existing_exact_prefix: 6,
            target_rank: Some(7),
            target_instances: 2,
            guarded_rank_matches: true,
            existing_prefix_unchanged: false,
            preview_unique: false,
            preview_within_bound: false,
        });

        let error = audit.verify().unwrap_err().to_string();
        assert!(error.contains("targets 1"));
        assert!(error.contains("outside first page 1"));
        assert!(error.contains("repeated targets 1"));
        assert!(error.contains("existing prefix changes 1"));
        assert!(error.contains("duplicate previews 1"));
        assert!(error.contains("unbounded previews 1"));
        assert!(!error.contains("再进来"));
    }

    #[test]
    fn exact_phrase_tsf_probe_plan_covers_each_authenticated_rank_bucket_first() {
        let plan = plan_exact_phrase_tsf_probe_sources([2_797, 2, 0, 0, 0, 0], 2_799, 16).unwrap();
        assert_eq!(plan.len(), 16);
        assert_eq!(
            plan[0],
            ExactPhraseTsfProbeSource::RankBucket {
                rank_index: 0,
                anchor: 1_398,
            }
        );
        assert_eq!(
            plan[1],
            ExactPhraseTsfProbeSource::RankBucket {
                rank_index: 1,
                anchor: 1,
            }
        );
        assert!(
            plan[2..]
                .iter()
                .all(|source| matches!(source, ExactPhraseTsfProbeSource::Catalog { .. }))
        );

        let error = plan_exact_phrase_tsf_probe_sources([2_797, 2, 0, 0, 0, 0], 2_799, 1)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "exact phrase TSF probe count is below the authenticated rank coverage"
        );
    }

    #[test]
    fn exact_phrase_composition_audit_distinguishes_page_local_and_cross_page_movement() {
        let before = (1..=10)
            .map(|index| format!("候选{index}"))
            .collect::<Vec<_>>();
        let mut page_local = ExactPhraseRankComparison::default();
        let mut after = before.clone();
        after.insert(0, "公开整词".to_owned());
        after.truncate(10);
        page_local.observe(&before, &after, "候选1", 2);
        assert_eq!(page_local.correct_top_losses, 1);
        assert_eq!(page_local.first_page_degradations, 0);
        assert_eq!(page_local.cross_page_degradations, 0);

        let mut cross_page = ExactPhraseRankComparison::default();
        cross_page.observe(&before, &after, "候选6", 3);
        assert_eq!(cross_page.first_page_degradations, 1);
        assert_eq!(cross_page.first_page_degradation_instances, 3);
        assert_eq!(cross_page.cross_page_degradations, 1);
        assert_eq!(cross_page.cross_page_degradation_instances, 3);
    }

    #[test]
    fn phrase_layer_control_sampling_is_bounded_and_input_order_independent() {
        let probes = (0..PHRASE_LAYER_CONTROL_LIMIT + 7)
            .map(|index| PublicLexiconRankProbe {
                observed: ziranma_core::KeySequence::new("aa").unwrap(),
                expected_text: format!("公开对照{index:03}"),
                instances: index + 1,
            })
            .collect::<Vec<_>>();
        let expected = bounded_public_rank_probes(probes.clone());
        let mut reversed = probes;
        reversed.reverse();

        assert_eq!(expected.len(), PHRASE_LAYER_CONTROL_LIMIT);
        assert_eq!(bounded_public_rank_probes(reversed), expected);
    }

    #[test]
    fn parser_requires_explicit_write_intent_and_paths() {
        let source_hash = candidate_sha256_hex(LEXICON.as_bytes());
        assert_eq!(
            parse_options([
                "build".to_owned(),
                "--revision".to_owned(),
                "demo-v2".to_owned(),
                "--public".to_owned(),
                "--source".to_owned(),
                "words.tsv".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
                "--source-id".to_owned(),
                "ziranma-demo-v1".to_owned(),
                "--source-license".to_owned(),
                "MPL-2.0".to_owned(),
                "--source-url".to_owned(),
                "https://github.com/hewzhew/ziranma-decoder".to_owned(),
                "--source-sha256".to_owned(),
                source_hash.clone(),
            ])
            .unwrap(),
            Options::Build {
                source: PathBuf::from("words.tsv"),
                output: PathBuf::from("package"),
                revision: "demo-v2".to_owned(),
                declaration: PublicSourceDeclaration {
                    id: "ziranma-demo-v1".to_owned(),
                    license: "MPL-2.0".to_owned(),
                    url: "https://github.com/hewzhew/ziranma-decoder".to_owned(),
                    sha256: source_hash,
                },
            }
        );
        assert!(parse_options(["build".to_owned()]).is_err());
        assert!(matches!(
            parse_options([
                "build-rime".to_owned(),
                "--revision".to_owned(),
                "rime-v1".to_owned(),
                "--public".to_owned(),
                "--source".to_owned(),
                "pinyin.dict.yaml".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
                "--source-id".to_owned(),
                "rime-pinyin-simp".to_owned(),
                "--source-license".to_owned(),
                "Apache-2.0".to_owned(),
                "--source-url".to_owned(),
                "https://github.com/rime/rime-pinyin-simp".to_owned(),
                "--source-sha256".to_owned(),
                candidate_sha256_hex(RIME_LEXICON.as_bytes()),
            ])
            .unwrap(),
            Options::BuildRime { .. }
        ));
        assert!(matches!(
            parse_options([
                "build-rime-slice".to_owned(),
                "--revision".to_owned(),
                "wanxiang-slice-v1".to_owned(),
                "--public".to_owned(),
                "--source".to_owned(),
                "jichu.dict.yaml".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
                "--source-id".to_owned(),
                "wanxiang-jichu".to_owned(),
                "--source-license".to_owned(),
                "CC-BY-4.0".to_owned(),
                "--source-url".to_owned(),
                "https://github.com/amzxyz/rime_wanxiang".to_owned(),
                "--source-sha256".to_owned(),
                "b".repeat(64),
                "--max-entries".to_owned(),
                "100000".to_owned(),
                "--frequency-frontier-entries".to_owned(),
                "80000".to_owned(),
                "--two-character-coverage-depth".to_owned(),
                "4".to_owned(),
                "--three-character-coverage-entries".to_owned(),
                "6000".to_owned(),
                "--four-character-coverage-entries".to_owned(),
                "4000".to_owned(),
                "--max-text-characters".to_owned(),
                "8".to_owned(),
            ])
            .unwrap(),
            Options::BuildRimeSlice {
                config: PublicRimeSliceConfig {
                    max_entries: 100_000,
                    frequency_frontier_entries: 80_000,
                    two_character_coverage_depth: 4,
                    three_character_coverage_entries: 6_000,
                    four_character_coverage_entries: 4_000,
                    max_text_characters: 8,
                },
                ..
            }
        ));
        assert!(
            parse_options([
                "build-rime-slice".to_owned(),
                "--public".to_owned(),
                "--max-entries".to_owned(),
                "120001".to_owned(),
                "--max-text-characters".to_owned(),
                "8".to_owned(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_options([
                "compare".to_owned(),
                "--base-payload".to_owned(),
                "base.tsv".to_owned(),
                "--challenger-payload".to_owned(),
                "challenger.tsv".to_owned(),
            ])
            .unwrap(),
            Options::Compare {
                base_payload: PathBuf::from("base.tsv"),
                challenger_payload: PathBuf::from("challenger.tsv"),
            }
        );
        assert_eq!(
            parse_options([
                "layer-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--frontier-limit".to_owned(),
                "6".to_owned(),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
            ])
            .unwrap(),
            Options::LayerAudit {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                frontier_limit: 6,
                exact_promotions: 2,
            }
        );
        assert!(
            parse_options([
                "layer-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "51".to_owned(),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_options([
                "layer-benchmark".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--repetitions".to_owned(),
                "3".to_owned(),
                "--exact-promotions".to_owned(),
                "1".to_owned(),
            ])
            .unwrap(),
            Options::LayerBenchmark {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                repetitions: 3,
                exact_promotions: 1,
            }
        );
        assert_eq!(
            parse_options([
                "layer-composition-audit".to_owned(),
                "--core-payload".to_owned(),
                "core.tsv".to_owned(),
                "--supplemental-payload".to_owned(),
                "supplemental.tsv".to_owned(),
                "--corpus".to_owned(),
                "public.conllu".to_owned(),
                "--fit-corpus".to_owned(),
                "fit.conllu".to_owned(),
                "--frontier-limit".to_owned(),
                "7".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
            ])
            .unwrap(),
            Options::LayerCompositionAudit {
                core_payload: PathBuf::from("core.tsv"),
                supplemental_payload: PathBuf::from("supplemental.tsv"),
                corpus: PathBuf::from("public.conllu"),
                fit_corpus: Some(PathBuf::from("fit.conllu")),
                frontier_limit: 7,
                sample_limit: 128,
            }
        );
        assert!(
            parse_options([
                "layer-composition-audit".to_owned(),
                "--frontier-limit".to_owned(),
                "4".to_owned(),
                "--sample-limit".to_owned(),
                "128".to_owned(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_options([
                "supplement-enable".to_owned(),
                "--root".to_owned(),
                "supplement".to_owned(),
                "--exact-promotions".to_owned(),
                "1".to_owned(),
            ])
            .unwrap(),
            Options::SupplementEnable {
                root: PathBuf::from("supplement"),
                exact_promotions: 1,
            }
        );
        assert_eq!(
            parse_options([
                "supplement-disable".to_owned(),
                "--root".to_owned(),
                "supplement".to_owned(),
            ])
            .unwrap(),
            Options::SupplementDisable {
                root: PathBuf::from("supplement"),
            }
        );
        assert_eq!(
            parse_options([
                "supplement-status".to_owned(),
                "--root".to_owned(),
                "supplement".to_owned(),
            ])
            .unwrap(),
            Options::SupplementStatus {
                root: PathBuf::from("supplement"),
            }
        );
        assert_eq!(
            parse_options([
                "exact-short-status".to_owned(),
                "--root".to_owned(),
                "exact-short".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortStatus {
                root: PathBuf::from("exact-short"),
            }
        );
        assert_eq!(
            parse_options([
                "exact-short-enable".to_owned(),
                "--root".to_owned(),
                "exact-short".to_owned(),
                "--core-root".to_owned(),
                "core".to_owned(),
                "--without-supplement".to_owned(),
                "--package".to_owned(),
                "exact-package".to_owned(),
                "--expected-sha256".to_owned(),
                "a".repeat(64),
                "--exact-promotions".to_owned(),
                "2".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortEnable {
                root: PathBuf::from("exact-short"),
                core_root: PathBuf::from("core"),
                supplemental_root: None,
                package: PathBuf::from("exact-package"),
                expected_sha256: "a".repeat(64),
                exact_promotions: 2,
            }
        );
        assert_eq!(
            parse_options([
                "exact-short-disable".to_owned(),
                "--root".to_owned(),
                "exact-short".to_owned(),
            ])
            .unwrap(),
            Options::ExactShortDisable {
                root: PathBuf::from("exact-short"),
            }
        );
        assert_eq!(
            parse_options([
                "runtime-check".to_owned(),
                "--root".to_owned(),
                "slots".to_owned(),
            ])
            .unwrap(),
            Options::RuntimeCheck {
                root: PathBuf::from("slots"),
            }
        );
        assert!(parse_options(["runtime-check".to_owned()]).is_err());
        assert_eq!(
            parse_options([
                "runtime-query".to_owned(),
                "--root".to_owned(),
                "slots".to_owned(),
                "--supplemental-root".to_owned(),
                "supplement".to_owned(),
                "--exact-short-root".to_owned(),
                "exact-short".to_owned(),
                "--code".to_owned(),
                "daigle".to_owned(),
                "--limit".to_owned(),
                "6".to_owned(),
            ])
            .unwrap(),
            Options::RuntimeQuery {
                root: PathBuf::from("slots"),
                supplemental_root: Some(PathBuf::from("supplement")),
                exact_short_root: Some(PathBuf::from("exact-short")),
                code: "daigle".to_owned(),
                limit: 6,
            }
        );
        assert!(
            parse_options([
                "runtime-query".to_owned(),
                "--root".to_owned(),
                "slots".to_owned(),
                "--code".to_owned(),
                "Daigle".to_owned(),
                "--limit".to_owned(),
                "6".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options([
                "supplement-enable".to_owned(),
                "--root".to_owned(),
                "supplement".to_owned(),
                "--exact-promotions".to_owned(),
                "0".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options([
                "exact-short-enable".to_owned(),
                "--root".to_owned(),
                "exact-short".to_owned(),
                "--exact-promotions".to_owned(),
                "0".to_owned(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_options([
                "inspect".to_owned(),
                "--manifest".to_owned(),
                "manifest.zcm".to_owned(),
                "--payload".to_owned(),
                "lexicon.tsv".to_owned(),
                "--provenance".to_owned(),
                "provenance.zcp".to_owned(),
            ])
            .unwrap(),
            Options::Inspect {
                manifest: PathBuf::from("manifest.zcm"),
                payload: PathBuf::from("lexicon.tsv"),
                provenance: PathBuf::from("provenance.zcp"),
            }
        );
        assert_eq!(
            parse_options([
                "preflight".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
            ])
            .unwrap(),
            Options::Preflight {
                package: PathBuf::from("package")
            }
        );
        assert!(parse_options(["preflight".to_owned()]).is_err());
        let expected_sha256 = "a".repeat(64);
        assert_eq!(
            parse_options([
                "verify".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--expected-sha256".to_owned(),
                expected_sha256.clone(),
            ])
            .unwrap(),
            Options::Verify {
                package: PathBuf::from("package"),
                expected_sha256: expected_sha256.clone(),
            }
        );
        assert_eq!(
            parse_options([
                "adopt".to_owned(),
                "--root".to_owned(),
                "slots".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--expected-sha256".to_owned(),
                expected_sha256.clone(),
            ])
            .unwrap(),
            Options::Adopt {
                root: PathBuf::from("slots"),
                package: PathBuf::from("package"),
                expected_sha256,
            }
        );
        let trusted_public_key = "b".repeat(64);
        assert_eq!(
            parse_options([
                "verify-signature".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--signature".to_owned(),
                "release.zrs".to_owned(),
                "--trusted-public-key".to_owned(),
                trusted_public_key.clone(),
            ])
            .unwrap(),
            Options::VerifySignature {
                package: PathBuf::from("package"),
                signature: PathBuf::from("release.zrs"),
                trusted_public_key,
            }
        );
        let trusted_public_key = "c".repeat(64);
        assert_eq!(
            parse_options([
                "adopt-signed".to_owned(),
                "--root".to_owned(),
                "slots".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--signature".to_owned(),
                "release.zrs".to_owned(),
                "--trusted-public-key".to_owned(),
                trusted_public_key.clone(),
            ])
            .unwrap(),
            Options::AdoptSigned {
                root: PathBuf::from("slots"),
                package: PathBuf::from("package"),
                signature: PathBuf::from("release.zrs"),
                trusted_public_key: trusted_public_key.clone(),
            }
        );
        assert_eq!(
            parse_options([
                "stage-signed".to_owned(),
                "--signature".to_owned(),
                "release.zrs".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--trusted-public-key".to_owned(),
                trusted_public_key.clone(),
                "--root".to_owned(),
                "slots".to_owned(),
            ])
            .unwrap(),
            Options::StageSigned {
                root: PathBuf::from("slots"),
                package: PathBuf::from("package"),
                signature: PathBuf::from("release.zrs"),
                trusted_public_key,
            }
        );
        assert!(parse_options(["adopt-signed".to_owned()]).is_err());
        assert!(
            parse_options([
                "verify".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
                "--expected-sha256".to_owned(),
                "A".repeat(64),
            ])
            .is_err()
        );
        assert!(
            parse_options([
                "build".to_owned(),
                "--source".to_owned(),
                "private.tsv".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
                "--revision".to_owned(),
                "private-v1".to_owned(),
            ])
            .is_err()
        );
        let error = parse_options(["secret-command".to_owned()]).unwrap_err();
        assert!(!error.to_string().contains("secret-command"));
    }

    #[test]
    fn rime_build_pins_source_and_emits_a_canonical_candidate_payload() {
        let root = temporary_test_root();
        let source = root.join("pinyin.dict.yaml");
        let package = root.join("package");
        fs::create_dir(&root).unwrap();
        fs::write(&source, RIME_LEXICON).unwrap();
        let declaration = PublicSourceDeclaration {
            id: "rime-pinyin-simp".to_owned(),
            license: "Apache-2.0".to_owned(),
            url: "https://github.com/rime/rime-pinyin-simp".to_owned(),
            sha256: candidate_sha256_hex(RIME_LEXICON.as_bytes()),
        };

        build_rime_public_package(&source, &package, "rime-v1", &declaration).unwrap();
        let loaded = load_public_package_directory(&package).unwrap();
        assert_eq!(loaded.provenance.source_sha256(), declaration.sha256);
        assert_eq!(
            loaded
                .snapshot
                .candidate_texts("qn", 10)
                .unwrap()
                .first()
                .map(String::as_str),
            Some("亲")
        );
        assert!(loaded.payload_text.starts_with("text\tpinyin\tfrequency\n"));
        assert!(loaded.payload_text.contains("請\tqing\t1\n"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_the_exact_pinned_simplified_rime_source_enables_the_audit_filter() {
        let pinned = PublicSourceDeclaration {
            id: "rime-pinyin-simp".to_owned(),
            license: "Apache-2.0".to_owned(),
            url: "https://github.com/rime/rime-pinyin-simp".to_owned(),
            sha256: PINNED_RIME_PINYIN_SIMP_SHA256.to_owned(),
        };
        assert!(uses_pinned_simplified_rime_import(&pinned));

        let mut other_revision = pinned.clone();
        other_revision.sha256 = candidate_sha256_hex(RIME_LEXICON.as_bytes());
        assert!(!uses_pinned_simplified_rime_import(&other_revision));

        let mut other_source = pinned;
        other_source.id = "another-rime-source".to_owned();
        assert!(!uses_pinned_simplified_rime_import(&other_source));
    }

    #[test]
    fn toned_rime_slice_build_is_bounded_pinned_and_auditable() {
        const TONED_RIME: &str = "---\nname: public\n...\n\
什么\tshén me\t50\n\
声母\tshēng mǔ\t40\n\
缺权重\tquē quán zhòng\n\
测试\tcè shì\t30\n";
        let root = temporary_test_root();
        let source = root.join("toned.dict.yaml");
        let package = root.join("package");
        fs::create_dir(&root).unwrap();
        fs::write(&source, TONED_RIME).unwrap();
        let declaration = PublicSourceDeclaration {
            id: "public-toned-synthetic".to_owned(),
            license: "CC-BY-4.0".to_owned(),
            url: "https://example.com/public-toned".to_owned(),
            sha256: candidate_sha256_hex(TONED_RIME.as_bytes()),
        };

        let report = build_rime_slice_public_package(
            &source,
            &package,
            "toned-slice-v1",
            &declaration,
            PublicRimeSliceConfig {
                max_entries: 2,
                frequency_frontier_entries: 2,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 4,
            },
        )
        .unwrap();
        let loaded = load_public_package_directory(&package).unwrap();
        assert_eq!(loaded.snapshot.entry_count(), 2);
        assert!(loaded.payload_text.contains("什么\tshen me\t50\n"));
        assert!(loaded.payload_text.contains("声母\tsheng mu\t40\n"));
        assert!(!loaded.payload_text.contains("测试"));
        assert!(report.contains("源数据：4 行"));
        assert!(report.contains("字段 1"));
        assert!(report.contains("上限外 1"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phrase_layer_build_is_deterministic_v2_and_usable_by_existing_checks() {
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let allowlist = root.join("phrases.txt");
        let base = root.join("base.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        fs::create_dir(&root).unwrap();
        fs::write(&source, PHRASE_SOURCE).unwrap();
        fs::write(&allowlist, PHRASE_ALLOWLIST).unwrap();
        fs::write(&base, PHRASE_BASE).unwrap();
        let source_declaration = phrase_material_declaration("dictionary", PHRASE_SOURCE);
        let allowlist_declaration =
            phrase_material_declaration("fixed-phrase-list", PHRASE_ALLOWLIST);
        let base_declaration = phrase_material_declaration("base-payload", PHRASE_BASE);

        let build = |output: &Path| {
            build_phrase_layer_public_package(PhraseLayerBuildRequest {
                source: &source,
                allowlist: &allowlist,
                base_payload: &base,
                output,
                revision: "phrase-layer-v1",
                entry_limit: 2,
                source_declaration: &source_declaration,
                allowlist_declaration: &allowlist_declaration,
                base_declaration: &base_declaration,
            })
        };
        let report = build(&package_a).unwrap();
        build(&package_b).unwrap();

        let loaded = load_public_package_directory(&package_a).unwrap();
        assert_eq!(loaded.provenance.source_count(), 3);
        assert_eq!(loaded.snapshot.entry_count(), 2);
        assert!(report.contains("确定性选取 2 条"));
        for text in ["公开短语", "更多短语", "已有短语"] {
            assert!(!report.contains(text));
        }
        assert_eq!(
            inspect(
                &package_a.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
                &package_a.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
                &package_a.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            )
            .unwrap(),
            render_inspect_report(&loaded.snapshot, &loaded.provenance),
        );
        assert!(preflight(&package_a).unwrap().contains("结果：通过"));
        for filename in [
            CANDIDATE_PACKAGE_MANIFEST_FILE,
            CANDIDATE_PACKAGE_PAYLOAD_FILE,
            CANDIDATE_PACKAGE_PROVENANCE_FILE,
        ] {
            assert_eq!(
                fs::read(package_a.join(filename)).unwrap(),
                fs::read(package_b.join(filename)).unwrap(),
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_phrase_layer_build_pins_four_materials_and_is_deterministic() {
        const SOURCE: &str = "---\n...\n再进来\tzài jìn lái\t20\n";
        const CORE: &str = "text\tpinyin\tfrequency\n\
再\tzai\t100\n进来\tjin lai\t90\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n其他词\tqi ta ci\t80\n";
        const FIT: &str = "# sent_id = fit\n\
1\t再\t_\tADV\t_\t_\t2\tadvmod\t_\t_\n\
2\t进来\t_\tVERB\t_\t_\t0\troot\t_\t_\n\n";
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.yaml");
        let core = root.join("core.tsv");
        let supplemental = root.join("supplemental.tsv");
        let fit = root.join("train.conllu");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let core_package = root.join("core-package");
        let supplemental_package = root.join("supplemental-package");
        fs::write(&source, SOURCE).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&supplemental, SUPPLEMENTAL).unwrap();
        fs::write(&fit, FIT).unwrap();
        let declaration = |id: &str, contents: &str| PublicSourceDeclaration {
            id: id.to_owned(),
            license: "CC-BY-4.0".to_owned(),
            url: format!("https://example.com/{id}"),
            sha256: candidate_sha256_hex(contents.as_bytes()),
        };
        let source_declaration = declaration("source", SOURCE);
        let core_declaration = declaration("core", CORE);
        let supplemental_declaration = declaration("supplemental", SUPPLEMENTAL);
        let fit_declaration = declaration("fit", FIT);
        write_public_package(&core_package, "core-v1", &core_declaration, CORE).unwrap();
        write_public_package(
            &supplemental_package,
            "supplemental-v1",
            &supplemental_declaration,
            SUPPLEMENTAL,
        )
        .unwrap();
        let build = |output: PathBuf| {
            build_exact_phrase_layer_public_package(ExactPhraseLayerBuildOptions {
                source: source.clone(),
                core_payload: core.clone(),
                supplemental_payload: supplemental.clone(),
                fit_corpus: fit.clone(),
                output,
                revision: "exact-phrase-v1".to_owned(),
                entry_limit: 10,
                source_declaration: source_declaration.clone(),
                core_declaration: core_declaration.clone(),
                supplemental_declaration: supplemental_declaration.clone(),
                fit_declaration: fit_declaration.clone(),
            })
        };

        let report = build(package_a.clone()).unwrap();
        build(package_b.clone()).unwrap();
        let loaded = load_public_package_directory(&package_a).unwrap();
        let code = encode_pinyin_phrase("zai jin lai").unwrap().full_code;
        assert_eq!(loaded.provenance.source_count(), 4);
        assert_eq!(loaded.snapshot.entry_count(), 1);
        assert_eq!(
            loaded
                .snapshot
                .exact_full_code_texts(code.as_str(), 10)
                .unwrap(),
            vec!["再进来"]
        );
        assert!(report.contains("写入 1 条"));
        assert!(report.contains("未接入、未安装、未启用"));
        assert!(!report.contains("再进来"));
        let loaded_core = load_public_package_directory(&core_package).unwrap();
        let loaded_supplemental = load_public_package_directory(&supplemental_package).unwrap();
        let summary = preflight_loaded_exact_phrase_layer(
            &loaded_core,
            &loaded_supplemental,
            &loaded,
            2,
            2,
            [Duration::ZERO; 3],
        )
        .unwrap();
        assert_eq!(summary.phrase_entries, 1);
        assert_eq!(summary.sampled_codes, 1);
        assert_eq!(summary.catalog_audit.targets, 1);
        assert_eq!(summary.catalog_audit.without_existing_exact_prefix, 1);
        assert_eq!(summary.catalog_audit.after_existing_exact_prefix, 0);
        assert_eq!(summary.catalog_audit.target_ranks, [1, 0, 0, 0, 0, 0]);
        assert_eq!(
            summary.catalog_codes_by_rank[0],
            vec![code.as_str().to_owned()]
        );
        assert!(summary.catalog_codes_by_rank[1..].iter().all(Vec::is_empty));
        assert_eq!(summary.negative_control_codes, 2);
        for filename in [
            CANDIDATE_PACKAGE_MANIFEST_FILE,
            CANDIDATE_PACKAGE_PAYLOAD_FILE,
            CANDIDATE_PACKAGE_PROVENANCE_FILE,
        ] {
            assert_eq!(
                fs::read(package_a.join(filename)).unwrap(),
                fs::read(package_b.join(filename)).unwrap(),
            );
        }

        let failed_output = root.join("failed-package");
        let mut bad_fit = fit_declaration;
        bad_fit.sha256 = "0".repeat(64);
        assert!(
            build_exact_phrase_layer_public_package(ExactPhraseLayerBuildOptions {
                source,
                core_payload: core,
                supplemental_payload: supplemental,
                fit_corpus: fit,
                output: failed_output.clone(),
                revision: "exact-phrase-v1".to_owned(),
                entry_limit: 10,
                source_declaration,
                core_declaration,
                supplemental_declaration,
                fit_declaration: bad_fit,
            })
            .is_err()
        );
        assert!(!failed_output.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_phrase_layer_preflight_rejects_a_different_core_payload_without_text() {
        const BOUND_CORE: &str = "text\tpinyin\tfrequency\n\
再\tzai\t100\n进来\tjin lai\t90\n";
        const DIFFERENT_CORE: &str = "text\tpinyin\tfrequency\n\
在\tzai\t100\n进来\tjin lai\t90\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n其他词\tqi ta ci\t80\n";
        const PHRASE: &str = "text\tpinyin\tfrequency\n再进来\tzai jin lai\t20\n";
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let different_core_package = root.join("different-core");
        let supplemental_package = root.join("supplemental");
        let phrase_package = root.join("phrase");
        write_public_package(
            &different_core_package,
            "different-core-v1",
            &phrase_material_declaration("different-core", DIFFERENT_CORE),
            DIFFERENT_CORE,
        )
        .unwrap();
        write_public_package(
            &supplemental_package,
            "supplemental-v1",
            &phrase_material_declaration("supplemental", SUPPLEMENTAL),
            SUPPLEMENTAL,
        )
        .unwrap();
        write_multi_source_public_package(
            &phrase_package,
            "exact-phrase-v1",
            vec![
                CandidateSourceMaterial::from_bytes(
                    "source",
                    "CC-BY-4.0",
                    "https://example.com/source",
                    b"source",
                )
                .unwrap(),
                CandidateSourceMaterial::from_bytes(
                    "bound-core",
                    "Apache-2.0",
                    "https://example.com/bound-core",
                    BOUND_CORE.as_bytes(),
                )
                .unwrap(),
                CandidateSourceMaterial::from_bytes(
                    "supplemental",
                    "CC-BY-4.0",
                    "https://example.com/supplemental",
                    SUPPLEMENTAL.as_bytes(),
                )
                .unwrap(),
                CandidateSourceMaterial::from_bytes(
                    "fit",
                    "CC-BY-SA-4.0",
                    "https://example.com/fit",
                    b"fit",
                )
                .unwrap(),
            ],
            PHRASE,
        )
        .unwrap();

        let result = preflight_loaded_exact_phrase_layer(
            &load_public_package_directory(&different_core_package).unwrap(),
            &load_public_package_directory(&supplemental_package).unwrap(),
            &load_public_package_directory(&phrase_package).unwrap(),
            1,
            1,
            [Duration::ZERO; 3],
        );
        let error = match result {
            Ok(_) => panic!("a mismatched core payload must fail before candidate queries"),
            Err(error) => error.to_string(),
        };
        assert_eq!(
            error,
            "exact phrase provenance does not bind the supplied core and supplemental payloads"
        );
        for candidate_text in ["再进来", "进来", "其他词"] {
            assert!(!error.contains(candidate_text));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_consensus_layer_build_is_deterministic_and_lightweight_queryable() {
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let confirmation = root.join("words.txt");
        let base = root.join("base.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        fs::create_dir(&root).unwrap();
        fs::write(&source, SHORT_CONSENSUS_SOURCE).unwrap();
        fs::write(&confirmation, SHORT_CONSENSUS_CONFIRMATION).unwrap();
        fs::write(&base, SHORT_CONSENSUS_BASE).unwrap();
        let make_options = |output: PathBuf| ShortConsensusLayerBuildOptions {
            source: source.clone(),
            confirmation: confirmation.clone(),
            base_payload: base.clone(),
            output,
            revision: "short-consensus-v1".to_owned(),
            per_code_depth: 2,
            entry_limit: 50_000,
            source_declaration: phrase_material_declaration("dictionary", SHORT_CONSENSUS_SOURCE),
            confirmation_declaration: phrase_material_declaration(
                "confirmation",
                SHORT_CONSENSUS_CONFIRMATION,
            ),
            base_declaration: phrase_material_declaration("base-payload", SHORT_CONSENSUS_BASE),
        };

        let report =
            build_short_consensus_layer_public_package(make_options(package_a.clone())).unwrap();
        build_short_consensus_layer_public_package(make_options(package_b.clone())).unwrap();
        let loaded = load_exact_short_package_directory(&package_a).unwrap();
        assert_eq!(loaded.provenance.source_count(), 3);
        assert_eq!(loaded.catalog.entry_count(), 3);
        assert_eq!(loaded.catalog.code_count(), 2);
        assert_eq!(loaded.catalog.maximum_code_depth(), 2);
        assert_eq!(
            loaded.catalog.candidate_texts("ubuu", 8).unwrap(),
            ["收束", "手术"]
        );
        assert!(report.contains("运行时状态：未接入、未安装、未启用"));
        assert!(!report.contains("收束"));
        let query = exact_short_package_query(&package_a, "ubxd", 8).unwrap();
        assert!(query.contains("1. 首项"));
        assert!(query.contains("未构造通用 Decoder"));
        let benchmark = benchmark_exact_short_package(&package_a, "ubuu", 10).unwrap();
        assert!(benchmark.contains("目标码返回 2 项"));
        assert!(benchmark.contains("不是跨设备延迟承诺"));
        for filename in [
            CANDIDATE_PACKAGE_MANIFEST_FILE,
            CANDIDATE_PACKAGE_PAYLOAD_FILE,
            CANDIDATE_PACKAGE_PROVENANCE_FILE,
        ] {
            assert_eq!(
                fs::read(package_a.join(filename)).unwrap(),
                fs::read(package_b.join(filename)).unwrap(),
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_consensus_layer_checks_every_pin_before_creating_output() {
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let confirmation = root.join("words.txt");
        let base = root.join("base.tsv");
        fs::create_dir(&root).unwrap();
        fs::write(&source, SHORT_CONSENSUS_SOURCE).unwrap();
        fs::write(&confirmation, SHORT_CONSENSUS_CONFIRMATION).unwrap();
        fs::write(&base, SHORT_CONSENSUS_BASE).unwrap();

        for changed in 0..3 {
            let mut declarations = [
                phrase_material_declaration("dictionary", SHORT_CONSENSUS_SOURCE),
                phrase_material_declaration("confirmation", SHORT_CONSENSUS_CONFIRMATION),
                phrase_material_declaration("base-payload", SHORT_CONSENSUS_BASE),
            ];
            declarations[changed].sha256 = "0".repeat(64);
            let output = root.join(format!("package-{changed}"));
            assert!(
                build_short_consensus_layer_public_package(ShortConsensusLayerBuildOptions {
                    source: source.clone(),
                    confirmation: confirmation.clone(),
                    base_payload: base.clone(),
                    output: output.clone(),
                    revision: "short-consensus-v1".to_owned(),
                    per_code_depth: 2,
                    entry_limit: 50_000,
                    source_declaration: declarations[0].clone(),
                    confirmation_declaration: declarations[1].clone(),
                    base_declaration: declarations[2].clone(),
                })
                .is_err()
            );
            assert!(!output.exists());
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_short_layer_audit_compares_raw_and_page_guarded_insertions_without_writes() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
手书\tshou shu\t100\n\
受书\tshou shu\t90\n\
售书\tshou shu\t80\n\
授书\tshou shu\t70\n\
兽术\tshou shu\t60\n\
绶书\tshou shu\t50\n\
守书\tshou shu\t40\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n别的\tbie de\t10\n";
        const HELD_OUT: &str = "# sent_id = held-out\n1\t收束\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\n";
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let confirmation = root.join("words.txt");
        let base = root.join("base.tsv");
        let core = root.join("core.tsv");
        let supplemental = root.join("supplemental.tsv");
        let held_out = root.join("held-out.conllu");
        let package = root.join("exact-package");
        fs::create_dir(&root).unwrap();
        fs::write(&source, SHORT_CONSENSUS_SOURCE).unwrap();
        fs::write(&confirmation, SHORT_CONSENSUS_CONFIRMATION).unwrap();
        fs::write(&base, SHORT_CONSENSUS_BASE).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&supplemental, SUPPLEMENTAL).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();
        build_short_consensus_layer_public_package(ShortConsensusLayerBuildOptions {
            source: source.clone(),
            confirmation: confirmation.clone(),
            base_payload: base.clone(),
            output: package.clone(),
            revision: "exact-short-audit-test-v1".to_owned(),
            per_code_depth: 2,
            entry_limit: 50_000,
            source_declaration: phrase_material_declaration("dictionary", SHORT_CONSENSUS_SOURCE),
            confirmation_declaration: phrase_material_declaration(
                "confirmation",
                SHORT_CONSENSUS_CONFIRMATION,
            ),
            base_declaration: phrase_material_declaration("base-payload", SHORT_CONSENSUS_BASE),
        })
        .unwrap();
        let files_before = recursive_regular_file_count(&root);

        let report = audit_exact_short_layer(ExactShortLayerAuditRequest {
            core_payload: &core,
            supplemental_payload: &supplemental,
            exact_package: &package,
            held_out_corpus: &held_out,
            frontier_limit: 7,
            supplemental_promotions: 1,
        })
        .unwrap();
        let guarded = report
            .lines()
            .find(|line| line.contains("分页保护、第二页开头插入 · 精确补 2"))
            .unwrap();
        let exact_lane = report
            .lines()
            .find(|line| line.contains("现有完整词通道后插入 · 精确补 2"))
            .unwrap();
        assert!(report.contains("首选后立即插入 · 精确补 1"));
        assert!(report.contains("固定第一页、第二页开头插入 · 精确补 2"));
        assert!(exact_lane.contains("第一页变化 0（实例 0）"));
        assert!(exact_lane.contains("安全门 通过"));
        assert!(guarded.contains("第一页变化 0（实例 0）"));
        assert!(guarded.contains("安全门 通过"));
        assert!(report.contains("本次操作：只读预览"));
        assert!(!report.contains("收束"));
        assert_eq!(recursive_regular_file_count(&root), files_before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_short_benchmark_sampling_is_bounded_sorted_and_deterministic() {
        const PAYLOAD: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t40\n\
首项\tshou xiang\t30\n\
电赛\tdian sai\t20\n\
揉揉\trou rou\t10\n";
        let entries = parse_lexicon_tsv(PAYLOAD).unwrap();
        let first = evenly_spaced_exact_short_codes(&entries, 3);
        let second = evenly_spaced_exact_short_codes(&entries, 3);
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert!(first.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(evenly_spaced_exact_short_codes(&entries, 8).len(), 4);
    }

    #[test]
    fn duration_summary_uses_nearest_rank_tail_percentiles() {
        let mut samples = (1..=100)
            .rev()
            .map(Duration::from_micros)
            .collect::<Vec<_>>();
        let summary = summarize_durations(&mut samples).unwrap();
        assert_eq!(summary.samples, 100);
        assert_eq!(summary.median, Duration::from_micros(51));
        assert_eq!(summary.p95, Duration::from_micros(95));
        assert_eq!(summary.p99, Duration::from_micros(99));
        assert_eq!(summary.maximum, Duration::from_micros(100));
    }

    fn recursive_regular_file_count(root: &Path) -> usize {
        let mut pending = vec![root.to_owned()];
        let mut files = 0;
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else {
                    files += 1;
                }
            }
        }
        files
    }

    #[cfg(windows)]
    #[test]
    fn windows_tsf_preflight_accepts_a_public_package() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package = root.join("package");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        build_public_package(
            &source,
            &package,
            "windows-tsf-preflight-v1",
            &test_declaration(LEXICON),
        )
        .unwrap();

        assert!(super::preflight(&package).unwrap().contains("结果：通过"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phrase_layer_build_checks_every_pin_before_creating_output() {
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let allowlist = root.join("phrases.txt");
        let base = root.join("base.tsv");
        fs::create_dir(&root).unwrap();
        fs::write(&source, PHRASE_SOURCE).unwrap();
        fs::write(&allowlist, PHRASE_ALLOWLIST).unwrap();
        fs::write(&base, PHRASE_BASE).unwrap();

        for changed in 0..3 {
            let mut declarations = [
                phrase_material_declaration("dictionary", PHRASE_SOURCE),
                phrase_material_declaration("fixed-phrase-list", PHRASE_ALLOWLIST),
                phrase_material_declaration("base-payload", PHRASE_BASE),
            ];
            declarations[changed].sha256 = "0".repeat(64);
            let output = root.join(format!("package-{changed}"));
            assert!(
                build_phrase_layer_public_package(PhraseLayerBuildRequest {
                    source: &source,
                    allowlist: &allowlist,
                    base_payload: &base,
                    output: &output,
                    revision: "phrase-layer-v1",
                    entry_limit: 2,
                    source_declaration: &declarations[0],
                    allowlist_declaration: &declarations[1],
                    base_declaration: &declarations[2],
                })
                .is_err()
            );
            assert!(!output.exists());
        }

        let declarations = [
            phrase_material_declaration("dictionary", PHRASE_SOURCE),
            phrase_material_declaration("fixed-phrase-list", PHRASE_ALLOWLIST),
            phrase_material_declaration("base-payload", PHRASE_BASE),
        ];
        let insufficient_output = root.join("package-insufficient");
        assert!(
            build_phrase_layer_public_package(PhraseLayerBuildRequest {
                source: &source,
                allowlist: &allowlist,
                base_payload: &base,
                output: &insufficient_output,
                revision: "phrase-layer-v1",
                entry_limit: 3,
                source_declaration: &declarations[0],
                allowlist_declaration: &declarations[1],
                base_declaration: &declarations[2],
            })
            .is_err()
        );
        assert!(!insufficient_output.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phrase_layer_identity_binds_base_payload_even_when_selected_text_is_unchanged() {
        const OTHER_BASE: &str = "text\tpinyin\tfrequency\n另一个词\tling yi ge ci\t8\n";
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let allowlist = root.join("phrases.txt");
        let base_a = root.join("base-a.tsv");
        let base_b = root.join("base-b.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        fs::create_dir(&root).unwrap();
        fs::write(&source, PHRASE_SOURCE).unwrap();
        fs::write(&allowlist, PHRASE_ALLOWLIST).unwrap();
        fs::write(&base_a, "text\tpinyin\tfrequency\n基础词\tji chu ci\t9\n").unwrap();
        fs::write(&base_b, OTHER_BASE).unwrap();
        let source_declaration = phrase_material_declaration("dictionary", PHRASE_SOURCE);
        let allowlist_declaration =
            phrase_material_declaration("fixed-phrase-list", PHRASE_ALLOWLIST);
        let base_a_text = fs::read_to_string(&base_a).unwrap();
        let base_a_declaration = phrase_material_declaration("base-payload", &base_a_text);
        let base_b_declaration = phrase_material_declaration("base-payload", OTHER_BASE);

        for (base_payload, output, base_declaration) in [
            (&base_a, &package_a, &base_a_declaration),
            (&base_b, &package_b, &base_b_declaration),
        ] {
            build_phrase_layer_public_package(PhraseLayerBuildRequest {
                source: &source,
                allowlist: &allowlist,
                base_payload,
                output,
                revision: "phrase-layer-v1",
                entry_limit: 2,
                source_declaration: &source_declaration,
                allowlist_declaration: &allowlist_declaration,
                base_declaration,
            })
            .unwrap();
        }

        let loaded_a = load_public_package_directory(&package_a).unwrap();
        let loaded_b = load_public_package_directory(&package_b).unwrap();
        assert_eq!(loaded_a.payload_text, loaded_b.payload_text);
        assert_ne!(loaded_a.provenance_text, loaded_b.provenance_text);
        assert_ne!(
            loaded_a.authentication_sha256,
            loaded_b.authentication_sha256
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_package_merge_preserves_base_order_and_deduplicates_identity() {
        const BASE: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n乙\tyi\t20\n";
        const OVERLAY: &str = "text\tpinyin\tfrequency\n乙\tyi\t999\n丙\tbing\t10\n";
        const EXPECTED: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n乙\tyi\t20\n丙\tbing\t10\n";
        let root = temporary_test_root();
        let base = root.join("base");
        let overlay = root.join("overlay");
        let merged = root.join("merged");
        fs::create_dir(&root).unwrap();
        write_public_package(
            &base,
            "base-v1",
            &phrase_material_declaration("base-source", BASE),
            BASE,
        )
        .unwrap();
        write_public_package(
            &overlay,
            "overlay-v1",
            &phrase_material_declaration("overlay-source", OVERLAY),
            OVERLAY,
        )
        .unwrap();

        let report = merge_public_packages(&base, &overlay, &merged, "merged-v1").unwrap();
        let loaded = load_public_package_directory(&merged).unwrap();
        assert_eq!(loaded.payload_text, EXPECTED);
        assert_eq!(loaded.provenance.source_count(), 2);
        assert_eq!(loaded.snapshot.entry_count(), 3);
        assert!(report.contains("基础 2 条；叠加 2 条；新增 1 条；重复 1 条"));
        assert!(!report.contains('甲'));
        assert!(preflight(&merged).unwrap().contains("结果：通过"));
        let query = public_package_query(&merged, "jx", 3).unwrap();
        assert!(query.contains("1. 甲"));
        assert!(query.contains("本次操作：只读"));
        assert!(merge_public_packages(&base, &overlay, &merged, "again-v1").is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_package_merge_rejects_conflicting_sources_before_creating_output() {
        const BASE: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n";
        const OVERLAY: &str = "text\tpinyin\tfrequency\n乙\tyi\t20\n";
        let root = temporary_test_root();
        let base = root.join("base");
        let overlay = root.join("overlay");
        let merged = root.join("merged");
        fs::create_dir(&root).unwrap();
        write_public_package(&base, "base-v1", &test_declaration(BASE), BASE).unwrap();
        write_public_package(&overlay, "overlay-v1", &test_declaration(OVERLAY), OVERLAY).unwrap();

        assert!(merge_public_packages(&base, &overlay, &merged, "merged-v1").is_err());
        assert!(!merged.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_package_merge_rejects_private_and_corrupted_inputs_fail_closed() {
        const PUBLIC: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n";
        const PRIVATE: &str = "text\tpinyin\tfrequency\n乙\tyi\t20\n";
        let root = temporary_test_root();
        let public = root.join("public");
        let private = root.join("private");
        let corrupted = root.join("corrupted");
        let private_output = root.join("private-output");
        let corrupted_output = root.join("corrupted-output");
        fs::create_dir(&root).unwrap();
        write_public_package(
            &public,
            "public-v1",
            &phrase_material_declaration("public-source", PUBLIC),
            PUBLIC,
        )
        .unwrap();

        fs::create_dir(&private).unwrap();
        let private_manifest = CandidatePackageManifest::from_payload("private-v1", true, PRIVATE)
            .unwrap()
            .render();
        let private_provenance = CandidatePackageProvenance::from_materials(
            "private-source",
            "MIT",
            "https://example.com/private-source",
            &candidate_sha256_hex(PRIVATE.as_bytes()),
            &private_manifest,
            PRIVATE,
        )
        .unwrap()
        .render();
        fs::write(private.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), PRIVATE).unwrap();
        fs::write(
            private.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
            private_manifest,
        )
        .unwrap();
        fs::write(
            private.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            private_provenance,
        )
        .unwrap();
        assert!(merge_public_packages(&public, &private, &private_output, "merged-v1").is_err());
        assert!(public_package_query(&private, "yi", 3).is_err());
        assert!(!private_output.exists());

        write_public_package(
            &corrupted,
            "corrupted-v1",
            &phrase_material_declaration("corrupted-source", PRIVATE),
            PRIVATE,
        )
        .unwrap();
        fs::write(
            corrupted.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
            "text\tpinyin\tfrequency\n丙\tbing\t10\n",
        )
        .unwrap();
        assert!(
            merge_public_packages(&public, &corrupted, &corrupted_output, "merged-v1").is_err()
        );
        assert!(!corrupted_output.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_lexicon_merge_checks_entry_and_byte_limits() {
        const BASE: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n乙\tyi\t20\n";
        const OVERLAY: &str = "text\tpinyin\tfrequency\n丙\tbing\t10\n";
        let merged = merge_public_lexicon_payloads(BASE, OVERLAY, 3, usize::MAX).unwrap();
        assert_eq!(merged.appended_entries, 1);
        assert!(merge_public_lexicon_payloads(BASE, OVERLAY, 2, usize::MAX).is_err());
        assert!(
            merge_public_lexicon_payloads(BASE, OVERLAY, usize::MAX, merged.payload.len() - 1)
                .is_err()
        );
    }

    #[test]
    fn public_miss_diagnosis_distinguishes_selection_and_composition() {
        assert_eq!(
            classify_public_miss(true, true, Some(1), Some(1)),
            PublicMissDiagnosis::WholeWordVisible { rank: 1 }
        );
        assert_eq!(
            classify_public_miss(true, true, Some(1), None),
            PublicMissDiagnosis::WholeWordOutsideVisible
        );
        assert_eq!(
            classify_public_miss(true, false, Some(3), None),
            PublicMissDiagnosis::SourceWholeWordExcluded {
                minimum_segments: Some(3),
                visible_rank: None,
            }
        );
        assert_eq!(
            classify_public_miss(false, false, Some(2), Some(7)),
            PublicMissDiagnosis::CompositionVisible {
                segments: 2,
                rank: 7
            }
        );
        assert_eq!(
            classify_public_miss(false, false, Some(2), None),
            PublicMissDiagnosis::CompositionCrowded { segments: 2 }
        );
        assert_eq!(
            classify_public_miss(false, false, None, None),
            PublicMissDiagnosis::Unexplained
        );
    }

    #[test]
    fn exact_public_segmentation_finds_the_fewest_complete_package_words() {
        const PAYLOAD: &str = "text\tpinyin\tfrequency\n\
误\twu\t50\n\
提交\tti jiao\t40\n\
掰开\tbai kai\t30\n\
揉\trou\t20\n\
碎\tsui\t10\n";
        let entries = parse_lexicon_tsv(PAYLOAD).unwrap();
        assert_eq!(
            minimum_exact_public_segments(&entries, "误提交", "wutijc"),
            Some(2)
        );
        assert_eq!(
            minimum_exact_public_segments(&entries, "掰开揉碎", "blklrbsv"),
            Some(3)
        );
        assert_eq!(
            minimum_exact_public_segments(&entries, "未知", "wwvi"),
            None
        );
    }

    #[test]
    fn public_miss_diagnosis_uses_verified_source_package_and_visible_rank() {
        const SOURCE: &str = "---\n...\n绷断\tbēng duàn\t20\n";
        const CORE: &str = "text\tpinyin\tfrequency\n甲\tjia\t30\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n绷断\tbeng duan\t20\n";
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let core = root.join("core");
        let supplemental = root.join("supplemental");
        fs::create_dir(&root).unwrap();
        fs::write(&source, SOURCE).unwrap();
        write_public_package(
            &core,
            "diagnosis-core-v1",
            &phrase_material_declaration("diagnosis-core", CORE),
            CORE,
        )
        .unwrap();
        write_public_package(
            &supplemental,
            "diagnosis-v1",
            &phrase_material_declaration("diagnosis-source", SOURCE),
            SUPPLEMENTAL,
        )
        .unwrap();

        let report = diagnose_public_miss(&source, &core, &supplemental, "bgdr", "绷断").unwrap();
        assert!(report.contains("公开来源完整同码词：有"));
        assert!(report.contains("核心完整同码词：无"));
        assert!(report.contains("补充完整同码词：有"));
        assert!(report.contains("包内最少完整分段：1"));
        assert!(report.contains("前 50：第 1 名"));
        assert!(report.contains("判断：完整词已经收录"));
        assert!(report.contains("本次操作：只读"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn payload_comparison_is_aggregate_only_and_read_only() {
        let root = temporary_test_root();
        let base = root.join("base.tsv");
        let challenger = root.join("challenger.tsv");
        fs::create_dir(&root).unwrap();
        fs::write(&base, "text\tpinyin\tfrequency\n甲\tjia\t10\n钾\tjia\t5\n").unwrap();
        fs::write(
            &challenger,
            "text\tpinyin\tfrequency\n甲\tjia\t5\n钾\tjia\t20\n",
        )
        .unwrap();

        let report = compare_payloads(&base, &challenger).unwrap();
        assert!(report.contains("共同词形：2"));
        assert!(report.contains("同码首选不同：1"));
        assert!(report.contains("原本第 2 名：1"));
        assert!(report.contains("原本第 3–6 名：0"));
        assert!(report.contains("原本第 7 名以后：0"));
        assert!(report.contains("本次操作：只读"));
        assert!(!report.contains('甲'));
        assert!(!report.contains('钾'));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn consensus_audit_is_held_out_unambiguous_aggregate_only_and_read_only() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
甲\tjia\t100\n\
钾\tjia\t90\n\
吗\tma\t100\n\
马\tma\t90\n\
是\tshi\t100\n\
时\tshi\t90\n\
事\tshi\t80\n\
好\thao\t100\n\
行\txing\t100\n\
行\thang\t90\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
钾\tjia\t200\n\
马\tma\t200\n\
时\tshi\t200\n\
好\thao\t200\n\
行\txing\t200\n";
        const HELD_OUT: &str = "# sent_id = held-out\n\
1\t钾\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t吗\t_\tPART\t_\t_\t1\tdep\t_\t_\n\
3\t事\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
4\t好\t_\tADJ\t_\t_\t1\tdep\t_\t_\n\
5\t行\t_\tVERB\t_\t_\t1\tdep\t_\t_\n\n";

        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let supplemental = root.join("supplemental.tsv");
        let held_out = root.join("held-out.conllu");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&supplemental, SUPPLEMENTAL).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();

        let report = audit_public_consensus(&core, &supplemental, &held_out, 4).unwrap();
        assert!(report.contains("1 字：语料词面 5（实例 5）；核心匹配 5（实例 5）"));
        assert!(report.contains("多音排除 1（实例 1）；评测 4（实例 4）"));
        assert!(report.contains("正确首选新增 1、丢失 1、非目标首选变化 1"));
        assert!(report.contains("校准前正确首选 2（实例 2），校准后 2（实例 2）"));
        assert!(report.contains("候选顺序变化 3（实例 3）；首选变化 3（实例 3）"));
        assert!(report.contains("目标名次：改善 1（实例 1），不变 2（实例 2），变差 1（实例 1）"));
        assert!(report.contains(
            "1：评测 1（实例 1）；首选变化 0（实例 0）；正确首选新增 0（实例 0）、丢失 0（实例 0）、非目标首选变化 0（实例 0）"
        ));
        assert!(report.contains(
            "2～6：评测 3（实例 3）；首选变化 3（实例 3）；正确首选新增 1（实例 1）、丢失 1（实例 1）、非目标首选变化 1（实例 1）"
        ));
        assert!(report.contains(
            "≥7：评测 0（实例 0）；首选变化 0（实例 0）；正确首选新增 0（实例 0）、丢失 0（实例 0）、非目标首选变化 0（实例 0）"
        ));
        assert!(report.contains("安全门：未通过"));
        assert!(report.contains("本次操作：只读"));
        for private_value in ["甲", "钾", "吗", "马", "是", "时", "事", "好", "行"] {
            assert!(!report.contains(private_value));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_rank_audit_separates_prefix_preview_and_exact_code_competition() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
甲\tjia\t100\n\
钾\tjia\t90\n\
吗\tma\t100\n\
马\tma\t90\n\
甲吗\tjia ma\t100\n\
钾吗\tjia ma\t90\n\
行\txing\t100\n\
行\thang\t90\n";
        const HELD_OUT: &str = "# sent_id = held-out\n\
1\t钾\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t吗\t_\tPART\t_\t_\t1\tdep\t_\t_\n\
3\t甲吗\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
4\t行\t_\tVERB\t_\t_\t1\tdep\t_\t_\n\n";

        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let held_out = root.join("held-out.conllu");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();

        let report = audit_public_short_ranks(&core, &held_out, 6).unwrap();
        assert!(report.contains(
            "选样：1～2 字公开词面 4（实例 4）；核心匹配 4（实例 4）；多音排除 1（实例 1）"
        ));
        assert!(report.contains("评测 3（实例 3）"));
        assert!(report.contains("1 键：已完成 0（实例 0）"));
        assert!(report.contains(
            "2 键 / 1 字：评测 2（实例 2）；首选 1（实例 1），前 6 可见 2（实例 2）；第 2 名 1、第 3～6 名 0、第 7～50 名 0、50 名外 0；同码宽度 1 / 2～6 / ≥7：0 / 2 / 0"
        ));
        assert!(report.contains(
            "4 键 / 2 字：评测 1（实例 1）；首选 1（实例 1），前 6 可见 1（实例 1）；第 2 名 0、第 3～6 名 0、第 7～50 名 0、50 名外 0；同码宽度 1 / 2～6 / ≥7：0 / 1 / 0"
        ));
        assert!(report.contains("不把它冒充已经表达完的用户意图"));
        assert!(report.contains("本次操作：只读"));
        for public_value in ["甲", "钾", "吗", "马", "行"] {
            assert!(!report.contains(public_value));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn segment_penalty_audit_reorders_only_the_frozen_pool() {
        let entries = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
             不\tbu\t100\n\
             或\thuo\t20\n\
             到\tdao\t100\n\
             捕获\tbu huo\t8\n",
        )
        .unwrap();
        let decoder = Decoder::new(entries);
        let full = encode_pinyin_phrase("bu huo dao").unwrap().full_code;
        let probes = [ContinuousCompositionProbe {
            id: "synthetic-segment-penalty".to_owned(),
            full_observed: full.clone(),
            tail_abbreviated_observed: full.clone(),
            transposed_observed: full,
            expected_text: "捕获到".to_owned(),
            expected_segments: vec!["捕获".to_owned(), "到".to_owned()],
        }];

        let reports =
            evaluate_public_segment_penalty_profiles(&decoder, &probes, 6, &[0, 100]).unwrap();
        let [baseline, reranked] = reports.as_slice() else {
            panic!("both requested segment-penalty profiles should be returned");
        };
        assert_eq!(baseline.baseline_at_one, 0);
        assert_eq!(baseline.reranked_at_one, 0);
        assert_eq!(reranked.correct_top_gained, 1);
        assert_eq!(reranked.correct_top_lost, 0);
        assert_eq!(reranked.non_target_top_changes, 0);
        assert_eq!(reranked.target_rank_improved, 1);
    }

    #[test]
    fn segment_penalty_audit_is_aggregate_only_and_read_only() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
甲乙\tjia yi\t100\n\
丙丁\tbing ding\t80\n";
        const FIT: &str = "# sent_id = public-fit\n\
1\t甲乙\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t丙丁\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        const HELD_OUT: &str = "# sent_id = public-held-out\n\
1\t甲乙\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t丙丁\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";

        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let fit = root.join("fit.conllu");
        let held_out = root.join("held-out.conllu");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&fit, FIT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();

        let report = audit_public_segment_penalty(&core, &fit, &held_out, 6, 8).unwrap();
        assert!(report.contains("公开连续短语少分段惩罚审计"));
        assert!(report.contains("拟合档位（每多一个词界扣分）"));
        assert!(report.contains("安全门："));
        assert!(report.contains("本次操作：只读"));
        for public_value in ["甲乙", "丙丁", "甲乙丙丁"] {
            assert!(!report.contains(public_value));
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layer_audit_is_aggregate_only_bounded_and_read_only() {
        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let supplemental = root.join("supplemental.tsv");
        fs::create_dir(&root).unwrap();
        fs::write(&core, "text\tpinyin\tfrequency\n核心甲\tjia jia jia\t10\n").unwrap();
        fs::write(
            &supplemental,
            "text\tpinyin\tfrequency\n补充甲\tjia jia jia\t20\n独词\tdu ci\t10\n",
        )
        .unwrap();

        let report = audit_candidate_layers(&core, &supplemental, 6, 2).unwrap();
        assert!(report.contains("核心完整码首选：保留 1，变化 0"));
        assert!(report.contains("核心缺词码升为首选：1"));
        assert!(report.contains("单码最多实际补入：1"));
        assert!(report.contains("跨来源原始权重：未比较"));
        assert!(report.contains("本次操作：只读"));
        assert!(!report.contains("核心甲"));
        assert!(!report.contains("补充甲"));
        assert!(!report.contains("独词"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layer_composition_audit_selects_public_structure_and_reports_rank_changes() {
        const CORE: &str = "text\tpinyin\tfrequency\n掰开\tbai kai\t1000\n正常\tzheng chang\t900\n处理\tchu li\t800\n";
        const SUPPLEMENTAL: &str =
            "text\tpinyin\tfrequency\n揉碎\trou sui\t1000\n整场\tzheng chang\t900\n";
        const CORPUS: &str = "# sent_id = public-one\n\
1\t掰开\t掰开\tVERB\t_\t_\t0\troot\t_\t_\n\
2\t揉碎\t揉碎\tVERB\t_\t_\t1\tdep\t_\t_\n\
\n\
# sent_id = public-two\n\
1\t正常\t正常\tADJ\t_\t_\t0\troot\t_\t_\n\
2\t处理\t处理\tVERB\t_\t_\t1\tdep\t_\t_\n";
        const FIT_CORPUS: &str = "# sent_id = fit-one\n\
1\t掰开\t掰开\tVERB\t_\t_\t0\troot\t_\t_\n\
2\t揉碎\t揉碎\tVERB\t_\t_\t1\tdep\t_\t_\n\
\n\
# sent_id = fit-two\n\
1\t正常\t正常\tADJ\t_\t_\t0\troot\t_\t_\n\
2\t处理\t处理\tVERB\t_\t_\t1\tdep\t_\t_\n";
        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let supplemental = root.join("supplemental.tsv");
        let corpus = root.join("public.conllu");
        let fit_corpus = root.join("fit.conllu");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&supplemental, SUPPLEMENTAL).unwrap();
        fs::write(&corpus, CORPUS).unwrap();
        fs::write(&fit_corpus, FIT_CORPUS).unwrap();

        let report = audit_candidate_layer_compositions(
            &core,
            &supplemental,
            &corpus,
            Some(&fit_corpus),
            7,
            8,
        )
        .unwrap();
        assert!(report.contains(&format!(
            "候选载荷：核心 3 条 · SHA-256 {}；补充 2 条 · SHA-256 {}",
            candidate_sha256_hex(CORE.as_bytes()),
            candidate_sha256_hex(SUPPLEMENTAL.as_bytes()),
        )));
        assert!(report.contains(&format!(
            "保留语料：SHA-256 {}",
            candidate_sha256_hex(CORPUS.as_bytes()),
        )));
        assert!(report.contains(&format!(
            "语料指纹：拟合 SHA-256 {}；保留 SHA-256 {}",
            candidate_sha256_hex(FIT_CORPUS.as_bytes()),
            candidate_sha256_hex(CORPUS.as_bytes()),
        )));
        assert!(report.contains("样本：1（两词 1，三词 0"));
        assert!(report.contains("召回变化：新增可见 1，原可见丢失 0"));
        assert!(report.contains("非目标首选变化 0"));
        assert!(report.contains("核心完整码首选：保留 0，变化 0"));
        assert!(report.contains("全核心短语负对照"));
        assert!(report.contains("排除整词文字 0、整词同码 0；样本 1"));
        assert!(report.contains("保留首选 + 一个组合位"));
        assert!(report.contains("保留首选 + 两个组合位"));
        assert!(report.contains("单组合位排序冻结对照"));
        assert!(report.contains("结构 V1"));
        assert!(report.contains("少分段优先"));
        assert!(report.contains("层内名次优先"));
        assert!(report.contains("公开字境拟合 / 保留评测"));
        assert!(report.contains("拟合选择："));
        assert!(report.contains("保留评测结构基线"));
        assert!(report.contains("保留评测拟合选择"));
        assert!(report.contains("公开词界拟合 / 保留评测"));
        assert!(report.contains("可见词型"));
        assert!(report.contains("词界模型只累计拟合语料的公开分词词次"));
        assert!(report.contains("本次操作：只读"));
        assert!(!report.contains("掰开"));
        assert!(!report.contains("揉碎"));
        assert!(!report.contains("正常"));
        assert!(!report.contains("整场"));
        assert!(
            audit_candidate_layer_compositions(&core, &supplemental, &corpus, Some(&corpus), 7, 8,)
                .is_err(),
            "the held-out corpus must never be accepted as its own fit source"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_static_context_audit_promotes_public_collocations_without_echoing_text() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
无\twu\t1000\n\
误\twu\t100\n\
提交\tti jiao\t1000\n\
正确\tzheng que\t900\n\
检查\tjian cha\t800\n";
        const FIT: &str = "# sent_id = fit-1\n\
1\t误\t_\tVERB\t_\t_\t0\troot\t_\t_\n\
2\t提交\t_\tVERB\t_\t_\t1\tobj\t_\t_\n\n";
        const HELD_OUT: &str = "# sent_id = held-1\n\
1\t误\t_\tVERB\t_\t_\t0\troot\t_\t_\n\
2\t检查\t_\tVERB\t_\t_\t1\tobj\t_\t_\n\n\
# sent_id = held-2\n\
1\t正确\t_\tADJ\t_\t_\t0\troot\t_\t_\n\
2\t检查\t_\tVERB\t_\t_\t1\tobj\t_\t_\n\n";
        const ARPA: &str = "\\data\\\n\
ngram 1=8\n\
ngram 2=10\n\n\
\\1-grams:\n\
-0.1 <s> 0\n\
-0.1 </s> 0\n\
-1.0 <unk> 0\n\
-0.1 无 0\n\
-1.0 误 0\n\
-0.1 提交 0\n\
-0.1 正确 0\n\
-0.1 检查 0\n\n\
\\2-grams:\n\
-3.0 <s> 无\n\
-3.0 无 提交\n\
-3.0 无 检查\n\
-0.1 <s> 误\n\
-0.1 误 提交\n\
-0.1 误 检查\n\
-0.1 提交 </s>\n\
-0.1 <s> 正确\n\
-0.1 正确 检查\n\
-0.1 检查 </s>\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let fit = root.join("fit.conllu");
        let held_out = root.join("held-out.conllu");
        let model = root.join("public.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&fit, FIT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();
        fs::write(&model, ARPA).unwrap();

        let entries = parse_lexicon_tsv(CORE).unwrap();
        let decoder = Decoder::new(entries.clone());
        let fit_corpus = parse_ud_conllu(FIT).unwrap();
        let held_out_corpus = parse_ud_conllu(HELD_OUT).unwrap();
        let (fit_cases, _) =
            freeze_static_context_cases(&fit_corpus, &entries, &decoder, 8, 8).unwrap();
        let (held_out_cases, _) =
            freeze_static_context_cases(&held_out_corpus, &entries, &decoder, 8, 8).unwrap();
        assert_eq!(fit_cases.len(), 1);
        assert_eq!(held_out_cases.len(), 2);
        let language_model =
            load_sparse_arpa_language_model(&model, fit_cases.iter().chain(&held_out_cases), 2)
                .unwrap();
        let baseline = evaluate_static_context_profile(
            &fit_cases,
            &language_model,
            StaticContextProfile {
                search_depth: 1,
                minimum_average_gain: 0.0,
            },
        )
        .unwrap();
        let contextual = evaluate_static_context_profile(
            &fit_cases,
            &language_model,
            StaticContextProfile {
                search_depth: 8,
                minimum_average_gain: 0.25,
            },
        )
        .unwrap();
        assert_eq!(baseline.candidate.at_one, 0);
        assert_eq!(contextual.candidate.at_one, 1);
        assert_eq!(contextual.correct_top_one_lost, 0);
        let held_out_contextual = evaluate_static_context_profile(
            &held_out_cases,
            &language_model,
            StaticContextProfile {
                search_depth: 8,
                minimum_average_gain: 0.25,
            },
        )
        .unwrap();
        assert_eq!(held_out_contextual.correct_top_one_gained, 1);
        assert_eq!(held_out_contextual.correct_top_one_lost, 0);

        let report = audit_static_context(&model, &core, &fit, &held_out, 8, 8, 2).unwrap();
        assert!(report.contains("保留集净改善且未损失正确首选"));
        assert!(report.contains("句界：模型提供 <s>/</s>"));
        assert!(report.contains("本次操作：只读"));
        for text in ["误提交", "无提交", "误检查", "正确检查"] {
            assert!(!report.contains(text));
        }
        assert!(audit_static_context(&model, &core, &fit, &fit, 8, 8, 2).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_character_context_audit_freezes_only_current_exact_pool() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
甲\tjia\t1000\n\
钾\tjia\t100\n\
前词\tqian ci\t1000\n\
另词\tling ci\t900\n\
稳词\twen ci\t800\n";
        const FIT: &str = "# sent_id = fit-1\n\
1\t前词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t钾\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        const HELD_OUT: &str = "# sent_id = held-1\n\
1\t另词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t钾\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n\
# sent_id = held-2\n\
1\t稳词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t甲\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        const ARPA: &str = "\\data\\\n\
ngram 1=6\n\
ngram 2=6\n\n\
\\1-grams:\n\
-2.0 <unk> 0\n\
-0.1 甲 0\n\
-0.1 钾 0\n\
-0.1 前词 0\n\
-0.1 另词 0\n\
-0.1 稳词 0\n\n\
\\2-grams:\n\
-3.0 前词 甲\n\
-0.1 前词 钾\n\
-3.0 另词 甲\n\
-0.1 另词 钾\n\
-0.1 稳词 甲\n\
-3.0 稳词 钾\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let fit = root.join("fit.conllu");
        let held_out = root.join("held-out.conllu");
        let model = root.join("public.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&fit, FIT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();
        fs::write(&model, ARPA).unwrap();

        let report =
            audit_single_character_context(&model, &core, &fit, &held_out, 8, 8, 2).unwrap();
        assert!(report.contains("公开单字左上下文离线审计"));
        assert!(report.contains("无同码竞争排除 0；冻结 1；目标在 Top-8 外 0"));
        assert!(report.contains("无同码竞争排除 0；冻结 2；目标在 Top-8 外 0"));
        assert!(report.contains("保留集净改善且没有正确首选损失或非目标首选变化"));
        assert!(report.contains("只冻结当前两键码已有精确单字候选"));
        assert!(report.contains("本次操作：只读"));
        for text in ["甲", "钾", "前词", "另词", "稳词"] {
            assert!(!report.contains(text));
        }
        assert!(audit_single_character_context(&model, &core, &fit, &fit, 8, 8, 2).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_character_context_validation_uses_strict_development_selection() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
甲\tjia\t1000\n\
钾\tjia\t100\n\
前词\tqian ci\t1000\n\
另词\tling ci\t900\n";
        const DEVELOPMENT: &str = "# sent_id = dev-1\n\
1\t前词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t钾\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        const HELD_OUT: &str = "# sent_id = held-1\n\
1\t另词\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t钾\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        const ARPA: &str = "\\data\\\n\
ngram 1=5\n\
ngram 2=4\n\n\
\\1-grams:\n\
-2.0 <unk> 0\n\
-0.1 甲 0\n\
-0.1 钾 0\n\
-0.1 前词 0\n\
-0.1 另词 0\n\n\
\\2-grams:\n\
-3.0 前词 甲\n\
-0.1 前词 钾\n\
-3.0 另词 甲\n\
-0.1 另词 钾\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let core = root.join("core.tsv");
        let development = root.join("development.conllu");
        let held_out = root.join("held-out.conllu");
        let model = root.join("public.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&core, CORE).unwrap();
        fs::write(&development, DEVELOPMENT).unwrap();
        fs::write(&held_out, HELD_OUT).unwrap();
        fs::write(&model, ARPA).unwrap();

        let report = audit_single_character_context_validation(
            &model,
            &core,
            &development,
            &held_out,
            8,
            8,
            2,
        )
        .unwrap();
        assert!(report.contains("公开单字左上下文最终验证"));
        assert!(report.contains("开发选择：ARPA-d8-g0.10"));
        assert!(report.contains("开发集与最终保留集都净改善"));
        assert!(report.contains("最终保留集只运行冻结基线与开发集选中的一个档位"));
        assert!(report.contains("本次操作：只读"));
        for text in ["甲", "钾", "前词", "另词"] {
            assert!(!report.contains(text));
        }
        assert!(
            audit_single_character_context_validation(
                &model,
                &core,
                &development,
                &development,
                8,
                8,
                2,
            )
            .is_err()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_arpa_scoring_applies_standard_history_backoff() {
        const ARPA: &str = "\\data\\\n\
ngram 1=4\n\
ngram 2=0\n\n\
\\1-grams:\n\
-0.1 <s> -0.2\n\
-0.4 </s> 0\n\
-2.0 <unk> 0\n\
-1.0 误 -0.3\n\n\
\\2-grams:\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let model_path = root.join("backoff.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&model_path, ARPA).unwrap();
        let cases = [FrozenStaticContextCase {
            expected_text: "误".to_owned(),
            candidates: vec![FrozenStaticContextCandidate {
                text: "误".to_owned(),
                segments: vec!["误".to_owned()],
            }],
        }];

        let model = load_sparse_arpa_language_model(&model_path, cases.iter(), 2).unwrap();
        let score = model.score_candidate(&cases[0].candidates[0]).unwrap();
        assert!((score - -1.9).abs() < 1e-12);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_arpa_accepts_fcitx_style_models_without_sentence_markers() {
        const ARPA: &str = "\\data\\\n\
ngram 1=3\n\
ngram 2=1\n\n\
\\1-grams:\n\
-2.0 <unk> 0\n\
-1.0 误 -0.3\n\
-0.8 提交 0\n\n\
\\2-grams:\n\
-0.2 误 提交\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let model_path = root.join("boundaryless.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&model_path, ARPA).unwrap();
        let cases = [FrozenStaticContextCase {
            expected_text: "误提交".to_owned(),
            candidates: vec![FrozenStaticContextCandidate {
                text: "误提交".to_owned(),
                segments: vec!["误".to_owned(), "提交".to_owned()],
            }],
        }];

        let model = load_sparse_arpa_language_model(&model_path, cases.iter(), 2).unwrap();
        assert!(!model.sentence_boundaries);
        let score = model.score_candidate(&cases[0].candidates[0]).unwrap();
        assert!((score - -0.4).abs() < 1e-12);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sparse_arpa_rejects_a_single_sentence_marker() {
        const ARPA: &str = "\\data\\\n\
ngram 1=3\n\n\
\\1-grams:\n\
-0.1 <s> 0\n\
-2.0 <unk> 0\n\
-1.0 误 0\n\n\
\\end\\\n";
        let root = temporary_test_root();
        let model_path = root.join("partial-boundary.arpa");
        fs::create_dir(&root).unwrap();
        fs::write(&model_path, ARPA).unwrap();
        let cases = [FrozenStaticContextCase {
            expected_text: "误".to_owned(),
            candidates: vec![FrozenStaticContextCandidate {
                text: "误".to_owned(),
                segments: vec!["误".to_owned()],
            }],
        }];

        let error = load_sparse_arpa_language_model(&model_path, cases.iter(), 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("both <s> and </s>, or neither"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn report_is_compact_and_never_echoes_candidate_text_or_fingerprint() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        let snapshot = manifest.load_snapshot(LEXICON).unwrap();
        let provenance = CandidatePackageProvenance::parse(PROVENANCE).unwrap();
        let report = render_inspect_report(&snapshot, &provenance);
        assert_eq!(
            report,
            "候选包检查\n版本：tsf-public-demo-v1\n内容：公开\n词条：50\n\
             载荷：1132 字节\n来源：ziranma-demo-v1\n许可：MPL-2.0\n\
             SHA-256 与兼容性：通过\n本次操作：只读\n"
        );
        assert!(!report.contains("你好"));
        assert!(!report.contains("nihk"));
        assert!(!report.contains("592a4dbb4b33efa6"));

        let preflight = render_preflight_report(&PreflightSummary {
            revision: "tsf-public-demo-v1".to_owned(),
            input_keys: 4,
            committed_characters: 2,
        });
        assert_eq!(
            preflight,
            "TSF 候选预检\n版本：tsf-public-demo-v1\n输入：4 键\n上屏：2 字\n\
             结果：通过\n本次操作：不写文件\n"
        );
        assert!(!preflight.contains("你好"));
        assert!(!preflight.contains("nihk"));
    }

    #[test]
    fn multi_source_package_loads_and_reports_all_materials_without_text() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        let provenance = CandidatePackageProvenance::from_source_materials(
            vec![
                CandidateSourceMaterial::from_bytes(
                    "dictionary",
                    "Apache-2.0",
                    "https://example.com/dictionary",
                    b"dictionary",
                )
                .unwrap(),
                CandidateSourceMaterial::from_bytes(
                    "phrase-list",
                    "MIT",
                    "https://example.com/phrases",
                    b"phrases",
                )
                .unwrap(),
            ],
            MANIFEST,
            LEXICON,
        )
        .unwrap();
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        fs::write(root.join(CANDIDATE_PACKAGE_MANIFEST_FILE), MANIFEST).unwrap();
        fs::write(root.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), LEXICON).unwrap();
        fs::write(
            root.join(CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance.render(),
        )
        .unwrap();

        let loaded = load_public_package_directory(&root).unwrap();
        assert_eq!(loaded.provenance.source_count(), 2);
        let report = render_inspect_report(&loaded.snapshot, &loaded.provenance);
        assert!(report.contains("来源：2 份公开材料"));
        assert!(report.contains("许可：Apache-2.0、MIT"));
        assert!(!report.contains("你好"));
        assert_eq!(loaded.manifest, manifest);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_build_and_slot_lifecycle_round_trip_real_files() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();

        let declaration = test_declaration(LEXICON);
        let built_a = build_public_package(&source, &package_a, "public-a", &declaration).unwrap();
        let built_b = build_public_package(&source, &package_b, "public-b", &declaration).unwrap();
        let package_a_sha256 = package_sha256(&package_a);
        let package_b_sha256 = package_sha256(&package_b);
        assert!(built_a.contains("版本：public-a"));
        assert!(built_a.contains(&format!("发布 SHA-256：{package_a_sha256}")));
        assert!(built_b.contains("版本：public-b"));
        assert_eq!(
            fs::read_to_string(package_a.join(CANDIDATE_PACKAGE_PAYLOAD_FILE)).unwrap(),
            LEXICON
        );
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：未配置\n待切换：无\n可回退：无\n本次操作：只读\n"
        );
        assert!(!slots.exists());
        assert!(runtime_check(&slots).is_err());
        assert!(!slots.exists());

        let wrong_sha256 = "0".repeat(64);
        assert!(verify(&package_a, &wrong_sha256).is_err());
        assert!(adopt(&slots, &package_a, &wrong_sha256).is_err());
        assert!(!slots.exists());
        let verified = verify(&package_a, &package_a_sha256).unwrap();
        assert!(verified.contains("结果：与可信 SHA-256 一致"));
        assert!(!verified.contains(&package_a_sha256));
        adopt(&slots, &package_a, &package_a_sha256).unwrap();
        assert_eq!(
            runtime_check(&slots).unwrap(),
            "候选运行时检查\n版本：public-a\n词条：50\n结果：通过\n本次操作：只读\n"
        );
        let adopted_state = read_slot_state(&slots).unwrap();
        let receipt = fs::read_to_string(preflight_receipt_path(
            &slots,
            adopted_state.current().unwrap(),
        ))
        .unwrap();
        assert!(!receipt.contains("你好"));
        assert!(!receipt.contains("nihk"));
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：无\n可回退：无\n本次操作：只读\n"
        );
        let before_failed_stage = read_slot_state(&slots).unwrap();
        assert!(stage(&slots, &package_b, &wrong_sha256).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before_failed_stage);
        stage(&slots, &package_b, &package_b_sha256).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：public-b\n可回退：无\n本次操作：只读\n"
        );
        promote(&slots).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-b\n待切换：无\n可回退：public-a\n本次操作：只读\n"
        );
        rollback(&slots).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：无\n可回退：public-b\n本次操作：只读\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supplemental_activation_is_explicit_bounded_and_reversible() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("supplement");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        assert_eq!(
            supplement_status(&slots).unwrap(),
            "公开补充词层\n状态：关闭\n已准备：未配置\n本次操作：只读\n"
        );

        let declaration = test_declaration(LEXICON);
        build_public_package(&source, &package_a, "supplement-a", &declaration).unwrap();
        build_public_package(&source, &package_b, "supplement-b", &declaration).unwrap();
        let package_a_sha256 = package_sha256(&package_a);
        let package_b_sha256 = package_sha256(&package_b);
        adopt(&slots, &package_a, &package_a_sha256).unwrap();
        assert!(supplement_status(&slots).unwrap().contains("状态：关闭"));
        assert!(
            supplement_status(&slots)
                .unwrap()
                .contains("已准备：supplement-a")
        );

        let enabled = supplement_enable(&slots, 1).unwrap();
        assert!(enabled.contains("版本：supplement-a"));
        assert!(enabled.contains("每码最多补：1 个完整词"));
        assert_eq!(
            CandidateSupplementalState::parse(
                &fs::read_to_string(slots.join(CANDIDATE_SUPPLEMENTAL_STATE_FILE)).unwrap()
            )
            .unwrap()
            .exact_promotions(),
            1
        );
        let active = supplement_status(&slots).unwrap();
        assert!(active.contains("状态：启用"));
        assert!(active.contains("核心已有完整词首选保持不动"));
        assert!(active.contains("共识重排仅供离线审计"));

        stage(&slots, &package_b, &package_b_sha256).unwrap();
        promote(&slots).unwrap();
        let drifted = supplement_status(&slots).unwrap();
        assert!(drifted.contains("状态：已回退，仅使用核心候选"));
        assert!(drifted.contains("已准备：supplement-b"));
        supplement_enable(&slots, 1).unwrap();
        assert!(
            supplement_status(&slots)
                .unwrap()
                .contains("版本：supplement-b")
        );

        let package_count = fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
            .unwrap()
            .count();
        assert!(supplement_disable(&slots).unwrap().contains("候选包：保留"));
        assert!(supplement_status(&slots).unwrap().contains("状态：关闭"));
        assert_eq!(
            fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
                .unwrap()
                .count(),
            package_count
        );
        assert!(
            supplement_disable(&slots)
                .unwrap()
                .contains("写入：0 个文件")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_short_activation_is_strict_page_guarded_and_reversible() {
        let root = temporary_test_root();
        let source = root.join("source.yaml");
        let confirmation = root.join("words.txt");
        let base = root.join("base.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let core_package = root.join("core-package");
        let core_package_b = root.join("core-package-b");
        let core_slots = root.join("core-slots");
        let slots = root.join("exact-short");
        fs::create_dir(&root).unwrap();
        fs::write(&source, SHORT_CONSENSUS_SOURCE).unwrap();
        fs::write(&confirmation, SHORT_CONSENSUS_CONFIRMATION).unwrap();
        fs::write(&base, SHORT_CONSENSUS_BASE).unwrap();

        let build = |output: PathBuf, revision: &str| {
            build_short_consensus_layer_public_package(ShortConsensusLayerBuildOptions {
                source: source.clone(),
                confirmation: confirmation.clone(),
                base_payload: base.clone(),
                output,
                revision: revision.to_owned(),
                per_code_depth: 2,
                entry_limit: 50_000,
                source_declaration: phrase_material_declaration(
                    "dictionary",
                    SHORT_CONSENSUS_SOURCE,
                ),
                confirmation_declaration: phrase_material_declaration(
                    "confirmation",
                    SHORT_CONSENSUS_CONFIRMATION,
                ),
                base_declaration: phrase_material_declaration("base-payload", SHORT_CONSENSUS_BASE),
            })
            .unwrap();
        };
        build(package_a.clone(), "exact-short-a");
        build(package_b.clone(), "exact-short-b");
        fs::write(root.join("core.tsv"), LEXICON).unwrap();
        build_public_package(
            &root.join("core.tsv"),
            &core_package,
            "exact-short-core",
            &test_declaration(LEXICON),
        )
        .unwrap();
        build_public_package(
            &root.join("core.tsv"),
            &core_package_b,
            "exact-short-core-b",
            &test_declaration(LEXICON),
        )
        .unwrap();
        adopt(&core_slots, &core_package, &package_sha256(&core_package)).unwrap();

        let unprepared = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(unprepared.contains("专项准备：尚未进行"));
        assert!(unprepared.contains("日用状态：关闭；候选未改变"));
        assert!(unprepared.contains("本次操作：只读"));
        assert!(!slots.exists());

        assert_eq!(
            exact_short_status(&slots).unwrap(),
            "公开精确短词层\n状态：关闭\n已准备：未配置\n本次操作：只读\n"
        );
        assert!(!slots.exists());
        adopt(&slots, &package_a, &package_sha256(&package_a)).unwrap();
        let incomplete = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(incomplete.contains("专项准备：损坏或凭据不完整；不能启用"));
        assert!(incomplete.contains("固定准备入口不会覆盖"));
        assert!(
            exact_short_status(&slots)
                .unwrap()
                .contains("已准备：exact-short-a")
        );

        let write_combined_receipt = |package: &Path, exact_promotions| {
            let exact_slots = read_slot_state(&slots).unwrap();
            let exact_package = exact_slots.current().unwrap();
            let exact = load_installed_exact_short_package(&slots, exact_package).unwrap();
            let core_state = read_slot_state(&core_slots).unwrap();
            let core = load_installed_package(&core_slots, core_state.current().unwrap()).unwrap();
            assert_eq!(exact.authentication_sha256, package_sha256(package));
            write_exact_short_combined_preflight(
                &slots,
                &CandidateExactShortPreflightReceipt::new(
                    exact_package,
                    &exact.authentication_sha256,
                    exact_promotions,
                    &core.authentication_sha256,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
        };
        write_combined_receipt(&package_a, 2);

        let ready = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(ready.contains("专项准备：通过；组合凭据匹配"));
        assert!(ready.contains("已可启用"));
        assert!(ready.contains("日用状态：关闭；候选未改变"));

        let forced_failure = exact_short_enable_with_runtime_verifier(
            ExactShortEnableRequest {
                root: &slots,
                core_root: &core_slots,
                supplemental_root: None,
                package: &package_a,
                expected_sha256: &package_sha256(&package_a),
                exact_promotions: 2,
            },
            |_, _, _, _, _, _| false,
        )
        .unwrap_err()
        .to_string();
        assert!(forced_failure.contains("activation was rolled back to disabled"));
        assert_eq!(
            CandidateExactShortState::parse(
                &fs::read_to_string(slots.join(CANDIDATE_EXACT_SHORT_STATE_FILE)).unwrap()
            )
            .unwrap(),
            CandidateExactShortState::default(),
        );

        stage(
            &core_slots,
            &core_package_b,
            &package_sha256(&core_package_b),
        )
        .unwrap();
        promote(&core_slots).unwrap();
        let drifted_context = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(drifted_context.contains("已偏离专项凭据"));
        assert!(drifted_context.contains("专项准备：已失效，不能启用"));
        assert!(drifted_context.contains("固定准备入口不会覆盖旧组合"));
        rollback(&core_slots).unwrap();

        let enabled = exact_short_enable(ExactShortEnableRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(enabled.contains("版本：exact-short-a"));
        assert!(enabled.contains("每码最多补：2"));
        assert!(enabled.contains("运行时复读：通过"));
        assert!(enabled.contains("第一页保持不动"));
        let state = CandidateExactShortState::parse(
            &fs::read_to_string(slots.join(CANDIDATE_EXACT_SHORT_STATE_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(state.exact_promotions(), 2);
        let selection = ziranma_core::load_candidate_runtime_exact_short_selection(&slots).unwrap();
        let loaded = ziranma_core::load_candidate_runtime_exact_short(&slots, &selection)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.catalog().revision(), "exact-short-a");
        assert_eq!(loaded.exact_promotions(), 2);
        let active = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(active.contains("日用状态：已启用"));
        assert!(active.contains("下一步：无需写入"));

        stage(&slots, &package_b, &package_sha256(&package_b)).unwrap();
        promote(&slots).unwrap();
        let drifted = exact_short_status(&slots).unwrap();
        assert!(drifted.contains("状态：已回退，不注入第二页"));
        assert!(drifted.contains("已准备：exact-short-b"));
        let other_version = exact_short_readiness(ExactShortReadinessRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_a,
            expected_sha256: &package_sha256(&package_a),
            exact_promotions: 2,
        })
        .unwrap();
        assert!(other_version.contains("指向另一版本，未按本组合认证"));
        assert!(other_version.contains("固定准备入口不会覆盖"));
        assert!(
            exact_short_enable(ExactShortEnableRequest {
                root: &slots,
                core_root: &core_slots,
                supplemental_root: None,
                package: &package_b,
                expected_sha256: &package_sha256(&package_b),
                exact_promotions: 1,
            })
            .is_err()
        );
        fs::remove_file(slots.join(CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE)).unwrap();
        write_combined_receipt(&package_b, 1);
        exact_short_enable(ExactShortEnableRequest {
            root: &slots,
            core_root: &core_slots,
            supplemental_root: None,
            package: &package_b,
            expected_sha256: &package_sha256(&package_b),
            exact_promotions: 1,
        })
        .unwrap();
        assert!(
            exact_short_status(&slots)
                .unwrap()
                .contains("版本：exact-short-b")
        );

        let package_count = fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
            .unwrap()
            .count();
        assert!(
            exact_short_disable(&slots)
                .unwrap()
                .contains("候选包：保留")
        );
        assert!(exact_short_status(&slots).unwrap().contains("状态：关闭"));
        assert_eq!(
            fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
                .unwrap()
                .count(),
            package_count
        );
        assert!(
            exact_short_disable(&slots)
                .unwrap()
                .contains("写入：0 个文件")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_query_keeps_core_top_until_consensus_gate_passes() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
大国\tda guo\t1657\n\
打过\tda guo\t1390\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
打过\tda guo\t9480\n\
大国\tda guo\t8656\n";

        let root = temporary_test_root();
        let core_source = root.join("core.tsv");
        let supplemental_source = root.join("supplemental.tsv");
        let core_package = root.join("core-package");
        let supplemental_package = root.join("supplemental-package");
        let core_slots = root.join("core-slots");
        let supplemental_slots = root.join("supplemental-slots");
        fs::create_dir(&root).unwrap();
        fs::write(&core_source, CORE).unwrap();
        fs::write(&supplemental_source, SUPPLEMENTAL).unwrap();
        build_public_package(
            &core_source,
            &core_package,
            "runtime-conservative-core-v1",
            &test_declaration(CORE),
        )
        .unwrap();
        build_public_package(
            &supplemental_source,
            &supplemental_package,
            "runtime-conservative-supplemental-v1",
            &test_declaration(SUPPLEMENTAL),
        )
        .unwrap();
        adopt(&core_slots, &core_package, &package_sha256(&core_package)).unwrap();
        adopt(
            &supplemental_slots,
            &supplemental_package,
            &package_sha256(&supplemental_package),
        )
        .unwrap();
        supplement_enable(&supplemental_slots, 1).unwrap();

        let report =
            runtime_query(&core_slots, Some(&supplemental_slots), None, "dago", 2).unwrap();
        assert!(report.contains("1. 大国\n2. 打过\n"));
        assert!(!report.contains("1. 打过\n"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_query_replays_enabled_exact_short_pages_and_marks_insertions() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
手术\tshou shu\t1200\n\
手书\tshou shu\t1100\n\
收书\tshou shu\t1000\n\
受书\tshou shu\t900\n\
收数\tshou shu\t800\n\
手数\tshou shu\t700\n\
受数\tshou shu\t600\n\
收树\tshou shu\t500\n\
手树\tshou shu\t400\n\
受树\tshou shu\t300\n\
收输\tshou shu\t200\n\
手输\tshou shu\t100\n";
        const EXACT: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t90\n\
首数\tshou shu\t80\n";

        let root = temporary_test_root();
        let core_source = root.join("core.tsv");
        let exact_source = root.join("exact.tsv");
        let core_package = root.join("core-package");
        let exact_package = root.join("exact-package");
        let core_slots = root.join("core-slots");
        let exact_slots = root.join("exact-slots");
        fs::create_dir(&root).unwrap();
        fs::write(&core_source, CORE).unwrap();
        fs::write(&exact_source, EXACT).unwrap();
        build_public_package(
            &core_source,
            &core_package,
            "runtime-exact-core-v1",
            &test_declaration(CORE),
        )
        .unwrap();
        build_public_package(
            &exact_source,
            &exact_package,
            "runtime-exact-layer-v1",
            &test_declaration(EXACT),
        )
        .unwrap();
        adopt(&core_slots, &core_package, &package_sha256(&core_package)).unwrap();
        adopt(
            &exact_slots,
            &exact_package,
            &package_sha256(&exact_package),
        )
        .unwrap();

        let core_state = read_slot_state(&core_slots).unwrap();
        let core = load_installed_package(&core_slots, core_state.current().unwrap()).unwrap();
        let exact_state = read_slot_state(&exact_slots).unwrap();
        let exact_package_id = exact_state.current().unwrap();
        let exact = load_installed_exact_short_package(&exact_slots, exact_package_id).unwrap();
        write_exact_short_combined_preflight(
            &exact_slots,
            &CandidateExactShortPreflightReceipt::new(
                exact_package_id,
                &exact.authentication_sha256,
                2,
                &core.authentication_sha256,
                None,
            )
            .unwrap(),
        )
        .unwrap();
        write_exact_short_state(
            &exact_slots,
            &CandidateExactShortState::enabled(exact_package_id, 2).unwrap(),
        )
        .unwrap();

        let first_page = runtime_query(&core_slots, None, Some(&exact_slots), "ubuu", 6).unwrap();
        assert!(first_page.contains("精确短词版本：runtime-exact-layer-v1"));
        assert!(first_page.contains("6. 手数\n"));
        assert!(!first_page.contains("收束"));

        let second_page = runtime_query(&core_slots, None, Some(&exact_slots), "ubuu", 12).unwrap();
        assert!(second_page.contains("6. 手数\n7. 收束 〔公开精确短词〕\n"));
        assert!(second_page.contains("8. 首数 〔公开精确短词〕\n"));
        assert!(second_page.contains("9. 受数\n"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detached_signature_verification_binds_trusted_key_and_exact_package() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let signature_path = root.join("release.zrs");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        let declaration = test_declaration(LEXICON);
        build_public_package(&source, &package_a, "signed-a", &declaration).unwrap();
        build_public_package(&source, &package_b, "signed-b", &declaration).unwrap();
        let package_a_sha256 = package_sha256(&package_a);
        let (signature_text, trusted_public_key) = test_release_signature(&package_a_sha256, 11);
        fs::write(&signature_path, &signature_text).unwrap();

        let report = verify_signature(&package_a, &signature_path, &trusted_public_key).unwrap();
        assert!(report.contains("结果：可信 Ed25519 签名有效"));
        assert!(report.contains(&format!("发布 SHA-256：{package_a_sha256}")));
        assert!(!report.contains(&trusted_public_key));
        assert!(!report.contains("signature="));

        let (_, wrong_public_key) = test_release_signature(&package_a_sha256, 12);
        assert!(verify_signature(&package_a, &signature_path, &wrong_public_key).is_err());
        assert!(verify_signature(&package_b, &signature_path, &trusted_public_key).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn signed_slot_writes_verify_before_creating_or_changing_state() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let signature_path = root.join("release.zrs");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        let declaration = test_declaration(LEXICON);
        build_public_package(&source, &package_a, "signed-a", &declaration).unwrap();
        build_public_package(&source, &package_b, "signed-b", &declaration).unwrap();

        let package_a_sha256 = package_sha256(&package_a);
        let (signature_a, trusted_key_a) = test_release_signature(&package_a_sha256, 21);
        let (_, wrong_key) = test_release_signature(&package_a_sha256, 22);
        fs::write(&signature_path, &signature_a).unwrap();
        assert!(adopt_signed(&slots, &package_a, &signature_path, &wrong_key).is_err());
        assert!(!slots.exists());

        adopt_signed(&slots, &package_a, &signature_path, &trusted_key_a).unwrap();
        let before_failed_stage = read_slot_state(&slots).unwrap();
        let installed_before = fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
            .unwrap()
            .count();

        assert!(stage_signed(&slots, &package_b, &signature_path, &trusted_key_a).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before_failed_stage);
        assert_eq!(
            fs::read_dir(slots.join(CANDIDATE_PACKAGES_DIRECTORY))
                .unwrap()
                .count(),
            installed_before
        );

        let package_b_sha256 = package_sha256(&package_b);
        let (signature_b, trusted_key_b) = test_release_signature(&package_b_sha256, 23);
        fs::write(&signature_path, signature_b).unwrap();
        stage_signed(&slots, &package_b, &signature_path, &trusted_key_b).unwrap();
        assert!(read_slot_state(&slots).unwrap().candidate().is_some());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_preflight_receipt_blocks_promotion_without_changing_slots() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        let declaration = test_declaration(LEXICON);
        build_public_package(&source, &package_a, "receipt-a", &declaration).unwrap();
        build_public_package(&source, &package_b, "receipt-b", &declaration).unwrap();
        adopt(&slots, &package_a, &package_sha256(&package_a)).unwrap();
        stage(&slots, &package_b, &package_sha256(&package_b)).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let candidate = before.candidate().unwrap();
        fs::remove_file(preflight_receipt_path(&slots, candidate)).unwrap();
        assert!(promote(&slots).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_installed_provenance_cannot_reuse_an_old_preflight_receipt() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        let declaration = test_declaration(LEXICON);
        build_public_package(&source, &package_a, "immutable-a", &declaration).unwrap();
        build_public_package(&source, &package_b, "immutable-b", &declaration).unwrap();
        adopt(&slots, &package_a, &package_sha256(&package_a)).unwrap();
        stage(&slots, &package_b, &package_sha256(&package_b)).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let candidate = before.candidate().unwrap();
        let provenance_path = slots
            .join(CANDIDATE_PACKAGES_DIRECTORY)
            .join(candidate)
            .join(CANDIDATE_PACKAGE_PROVENANCE_FILE);
        let changed_provenance = fs::read_to_string(&provenance_path)
            .unwrap()
            .replace("source_license=MPL-2.0", "source_license=Apache-2.0");
        fs::write(provenance_path, changed_provenance).unwrap();
        assert!(promote(&slots).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_tsf_preflight_leaves_current_and_candidate_unchanged() {
        let root = temporary_test_root();
        let source_a = root.join("source-a.tsv");
        let source_b = root.join("source-b.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source_a, LEXICON).unwrap();
        fs::write(
            &source_b,
            format!("text\tpinyin\tfrequency\n{}\ta\t1\n", "测".repeat(257)),
        )
        .unwrap();
        let source_b_text = fs::read_to_string(&source_b).unwrap();
        build_public_package(
            &source_a,
            &package_a,
            "preflight-good",
            &test_declaration(LEXICON),
        )
        .unwrap();
        build_public_package(
            &source_b,
            &package_b,
            "preflight-rejected",
            &test_declaration(&source_b_text),
        )
        .unwrap();
        adopt(&slots, &package_a, &package_sha256(&package_a)).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let error = super::stage(&slots, &package_b, &package_sha256(&package_b))
            .unwrap_err()
            .to_string();
        assert!(!error.contains('测'));
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn build_refuses_overwrite_and_explicit_reader_rejects_bad_files() {
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let file = root.join("input");

        fs::write(&file, []).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, b"12345").unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, [0xff]).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, LEXICON).unwrap();

        let package = root.join("package");
        let declaration = test_declaration(LEXICON);
        build_public_package(&file, &package, "public-once", &declaration).unwrap();
        assert!(build_public_package(&file, &package, "public-twice", &declaration).is_err());

        let mut wrong_pin = declaration.clone();
        wrong_pin.sha256 = "0".repeat(64);
        let wrong_output = root.join("wrong-pin");
        assert!(build_public_package(&file, &wrong_output, "wrong-pin", &wrong_pin).is_err());
        assert!(!wrong_output.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_slot_store_rejects_plaintext_private_package_without_state_change() {
        let root = temporary_test_root();
        let package = root.join("private-package");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&package).unwrap();
        let manifest = CandidatePackageManifest::from_payload("private-v1", true, LEXICON)
            .unwrap()
            .render();
        let provenance = test_provenance(&manifest, LEXICON).render();
        fs::write(package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), LEXICON).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_MANIFEST_FILE), manifest).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_PROVENANCE_FILE), provenance).unwrap();

        assert!(adopt(&slots, &package, &"0".repeat(64)).is_err());
        assert!(!slots.exists());
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：未配置\n待切换：无\n可回退：无\n本次操作：只读\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ziranma-candidatectl-test-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
