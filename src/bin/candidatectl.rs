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

#[cfg(windows)]
use ziranma_core::preflight_candidate_snapshot;
use ziranma_core::{
    CANDIDATE_PACKAGE_MANIFEST_FILE, CANDIDATE_PACKAGE_PAYLOAD_FILE,
    CANDIDATE_PACKAGE_PROVENANCE_FILE, CANDIDATE_PACKAGES_DIRECTORY,
    CANDIDATE_PREFLIGHTS_DIRECTORY, CANDIDATE_SLOT_STATE_FILE, CANDIDATE_SNAPSHOT_SCHEMA_V1,
    CANDIDATE_SUPPLEMENTAL_STATE_FILE, CandidatePackageManifest, CandidatePackageProvenance,
    CandidateReleaseSignature, CandidateSlotState, CandidateSnapshot, CandidateSnapshotDescriptor,
    CandidateSourceMaterial, CandidateSupplementalState, CharacterBigramLanguageModel,
    ContinuousCompositionProbe, Decoder, DecoderIndexStats, FourCharacterCorrectionDecision,
    FourCharacterCorrectionKeepReason, LexiconEntry, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES, MAX_CANDIDATE_PROVENANCE_BYTES,
    MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES, MAX_CANDIDATE_SLOT_STATE_BYTES,
    MAX_CANDIDATE_SNAPSHOT_BYTES, MAX_CANDIDATE_SNAPSHOT_ENTRIES, MAX_CANDIDATE_SNAPSHOT_RANK,
    MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES, MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES,
    MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES, MAX_PUBLIC_RIME_SLICE_ENTRIES,
    MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES, MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS,
    PublicLexiconRankProbe, PublicLexiconTokenCoverageAudit, PublicRimeSliceConfig,
    PublicRimeSliceImportStats, PublicSupplementalCompositionProbe,
    SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH, SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH,
    SupplementalCandidateLayerConfig, SupplementalCompositionCandidate,
    SupplementalCompositionOrder, UdCorpusImportStats, are_qwerty_neighbors,
    audit_public_lexicon_token_coverage, audit_public_rime_target, audit_public_supplemental_layer,
    candidate_package_authentication_sha256, candidate_package_storage_id,
    candidate_payload_fingerprint, candidate_preflight_receipt_body, candidate_sha256_hex,
    compare_public_lexicons, encode_pinyin_phrase, layered_candidate_texts,
    layered_candidate_texts_with_consensus, layered_four_character_correction_decision,
    load_candidate_runtime_snapshots, load_current_candidate_snapshot, parse_lexicon_tsv,
    parse_public_rime_phrase_allowlist, parse_public_rime_slice, parse_rime_lexicon,
    parse_simplified_rime_lexicon, parse_ud_conllu, select_public_bigram_training_sequences,
    select_public_character_training_texts, select_public_continuous_composition_cases,
    select_public_lexicon_rank_probes, select_public_single_character_context_cases,
    select_public_static_context_cases, select_public_supplemental_composition_cases,
    supplemental_complete_composition_texts, supplemental_complete_composition_texts_with_order,
    supplemental_complete_compositions_with_order,
};

const PINNED_RIME_PINYIN_SIMP_SHA256: &str =
    "e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b";
const MAX_PUBLIC_COMPOSITION_AUDIT_CORPUS_BYTES: usize = 16 * 1024 * 1024;
const MAX_STATIC_CONTEXT_ARPA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_STATIC_CONTEXT_ARPA_LINE_BYTES: usize = 1024 * 1024;

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
    BuildPhraseLayer(Box<PhraseLayerBuildOptions>),
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
    LengthCoverageAudit {
        base_payload: PathBuf,
        challenger_payload: PathBuf,
        fit_corpus: PathBuf,
        held_out_corpus: PathBuf,
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
    Preflight {
        package: PathBuf,
    },
    PackageQuery {
        package: PathBuf,
        code: String,
        limit: usize,
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

struct PreflightSummary {
    revision: String,
    input_keys: usize,
    committed_characters: usize,
}

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
        Options::Preflight { package } => preflight(&package)?,
        Options::PackageQuery {
            package,
            code,
            limit,
        } => public_package_query(&package, &code, limit)?,
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
            code,
            limit,
        } => runtime_query(&root, supplemental_root.as_deref(), &code, limit)?,
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
        "build-phrase-layer" => parse_build_phrase_layer(arguments),
        "merge-public-packages" => parse_merge_public_packages(arguments),
        "diagnose-public-miss" => parse_diagnose_public_miss(arguments),
        "compare" => parse_compare(arguments),
        "consensus-audit" => parse_consensus_audit(arguments),
        "short-rank-audit" => parse_short_rank_audit(arguments),
        "length-coverage-audit" => parse_length_coverage_audit(arguments),
        "phrase-coverage-audit" => parse_phrase_coverage_audit(arguments),
        "phrase-layer-audit" => parse_phrase_layer_audit(arguments),
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
        "preflight" => Ok(Options::Preflight {
            package: parse_package_only(arguments, "preflight")?,
        }),
        "package-query" => parse_package_query(arguments),
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

fn parse_runtime_query(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut root = None;
    let mut supplemental_root = None;
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
        "  build-rime-slice --source <TONED_RIME.dict.yaml> --output <NEW_PACKAGE_DIR> --revision <REV> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --max-entries <1..120000> [--frequency-frontier-entries <1..MAX>] [--three-character-coverage-entries <N>] [--four-character-coverage-entries <N>] --max-text-characters <1..12> --public"
    );
    eprintln!(
        "  build-phrase-layer --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --output <NEW_PACKAGE_DIR> --revision <REV> --entry-limit <1..50000> --source-id <ID> --source-license <SPDX> --source-url <HTTPS_URL> --source-sha256 <SHA256> --allowlist-id <ID> --allowlist-license <SPDX> --allowlist-url <HTTPS_URL> --allowlist-sha256 <SHA256> --base-id <ID> --base-license <SPDX> --base-url <HTTPS_URL> --base-sha256 <SHA256> --public"
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
        "  length-coverage-audit --base-payload <LEXICON.tsv> --challenger-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu>"
    );
    eprintln!(
        "  phrase-coverage-audit --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --entry-limit <1..50000>"
    );
    eprintln!(
        "  phrase-layer-audit --source <TONED_RIME.dict.yaml> --allowlist <PUBLIC_PHRASES.txt> --base-payload <LEXICON.tsv> --fit-corpus <PUBLIC-TRAIN.conllu> --held-out-corpus <PUBLIC-TEST.conllu> --small-limit <N> --large-limit <N> --repetitions <1..100>"
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
    eprintln!("  preflight --package <PACKAGE_DIR>");
    eprintln!(
        "  package-query --package <PUBLIC_PACKAGE_DIR> --code <LOWERCASE_KEYS> --limit <1..50>"
    );
    eprintln!("  verify --package <PACKAGE_DIR> --expected-sha256 <SHA256>");
    eprintln!(
        "  verify-signature --package <PACKAGE_DIR> --signature <STATEMENT> --trusted-public-key <ED25519_HEX>"
    );
    eprintln!("  status --root <SLOT_DIR>");
    eprintln!("  runtime-check --root <SLOT_DIR>");
    eprintln!(
        "  runtime-query --root <SLOT_DIR> [--supplemental-root <SUPPLEMENTAL_SLOT_DIR>] --code <LOWERCASE_KEYS> --limit <1..50>"
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
    let config = SupplementalCandidateLayerConfig { exact_promotions };
    let codes = layer_benchmark_codes()?;
    let correction_codes = four_character_correction_benchmark_codes()?;

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

    let mut core_durations = Vec::with_capacity(repetitions * codes.len());
    let mut layered_durations = Vec::with_capacity(repetitions * codes.len());
    let mut correction_durations =
        Vec::with_capacity(repetitions.saturating_mul(correction_codes.len()));
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
    Ok(format!(
        "公开补充词层 release 热路径\n固定查询：{}；重复：{repetitions}；样本：{}\n索引构建：核心 {:.3} ms；补充 {:.3} ms\n核心基线：median {:.3} ms；p95 {:.3} ms；max {:.3} ms\n启用补充：median {:.3} ms；p95 {:.3} ms；max {:.3} ms\nmedian 差值：{median_delta_ms:+.3} ms\n四字纠错安全门：查询 {}；样本 {}；median {:.3} ms；p95 {:.3} ms；max {:.3} ms\n{correction_audit_summary}候选顺序发生变化的固定查询：{query_order_changes}\n核心完整码首选变化：{core_top_changes}\n结果校验和：{checksum}\n口径：同机、预热、固定公开完整码与音节边界前缀；不是跨设备性能结论。\n本次操作：只读\n",
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

#[derive(Clone, Copy)]
struct DurationSummary {
    samples: usize,
    median: Duration,
    p95: Duration,
    maximum: Duration,
}

fn summarize_durations(samples: &mut [Duration]) -> Option<DurationSummary> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let p95_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
    Some(DurationSummary {
        samples: samples.len(),
        median: samples[samples.len() / 2],
        p95: samples[p95_index],
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
        "双字码覆盖：合格码 {}，全局前沿已覆盖 {}（{} 行），补充候选 {}，保留 {}，上限外 {}，高频回填 {}",
        stats.eligible_two_character_codes,
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
    code: &str,
    limit: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let runtime = load_candidate_runtime_snapshots(root, supplemental_root)?
        .ok_or("candidate runtime root is not configured")?;
    let (mut candidates, supplemental_revision) = match runtime.supplemental() {
        Some(supplemental) => (
            // Keep this diagnostic aligned with TSF's gated runtime path.
            layered_candidate_texts(
                runtime.core(),
                supplemental.snapshot(),
                code,
                limit,
                supplemental.config(),
            )?,
            Some(supplemental.snapshot().revision()),
        ),
        None => (runtime.core().candidate_texts(code, limit)?, None),
    };
    if limit >= 2
        && !candidates.is_empty()
        && let FourCharacterCorrectionDecision::Offer(offer) =
            layered_four_character_correction_decision(
                runtime.core(),
                runtime
                    .supplemental()
                    .map(|supplemental| supplemental.snapshot().as_ref()),
                code,
                1,
            )?
        && let Some(recovered) = offer.candidates.into_iter().next()
    {
        let existing_index = candidates
            .iter()
            .position(|candidate| candidate == &recovered.text);
        if existing_index != Some(0) {
            if let Some(existing_index) = existing_index {
                candidates.remove(existing_index);
            }
            candidates.insert(1.min(candidates.len()), recovered.text);
            candidates.truncate(limit);
        }
    }
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
    writeln!(output, "输入：{code}").unwrap();
    for (index, candidate) in candidates.iter().enumerate() {
        writeln!(output, "{}. {}", index + 1, candidate).unwrap();
    }
    if candidates.is_empty() {
        writeln!(output, "（没有候选）").unwrap();
    }
    writeln!(
        output,
        "口径：与 TSF 相同的公开包分层；不含显式别名、项目覆盖、会话记忆、个人学习或上下文重排。"
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

fn preflight(package: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    let summary = preflight_loaded_package(&loaded)?;
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
    let loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&loaded, expected_sha256)?;
    adopt_loaded(root, loaded)
}

fn stage(
    root: &Path,
    package: &Path,
    expected_sha256: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    verify_expected_sha256(&loaded, expected_sha256)?;
    stage_loaded(root, loaded)
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
    let mut state = read_slot_state(root)?;
    if state.current().is_some() {
        return Err("current candidate package is already configured".into());
    }
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    preflight_loaded_package(&installed)?;
    write_preflight_receipt(root, &package_id, &installed.authentication_sha256)?;
    state.adopt(&package_id)?;
    write_slot_state(root, &state)?;
    Ok(render_preflight_change_report(
        "当前候选包已建立",
        &revision,
    ))
}

fn stage_loaded(root: &Path, loaded: LoadedPackage) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    if state.current().is_none() {
        return Err("current candidate package is not configured".into());
    }
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    preflight_loaded_package(&installed)?;
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
                "--code".to_owned(),
                "daigle".to_owned(),
                "--limit".to_owned(),
                "6".to_owned(),
            ])
            .unwrap(),
            Options::RuntimeQuery {
                root: PathBuf::from("slots"),
                supplemental_root: Some(PathBuf::from("supplement")),
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

        let report = runtime_query(&core_slots, Some(&supplemental_slots), "dago", 2).unwrap();
        assert!(report.contains("1. 大国\n2. 打过\n"));
        assert!(!report.contains("1. 打过\n"));

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
        let error = stage(&slots, &package_b, &package_sha256(&package_b))
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
