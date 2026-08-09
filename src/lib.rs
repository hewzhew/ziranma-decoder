//! Explainable decoder experiments for Ziranma double-pinyin key sequences.
//!
//! The current research baseline supports full-code and mixed-abbreviation
//! spellings, together with at most one local key error. It deliberately uses
//! a compact syllable trie with joint key alignment and inspectable local
//! language scores, then streams trie prefixes into a memoized k-best sentence
//! lattice with explicit, penalized literal fallback for unresolved keys.

use std::cmp::Ordering;
#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::sync::OnceLock;

mod abbreviation;
mod adaptive_comparison;
mod adaptive_coverage;
mod adaptive_evaluation;
mod adaptive_memory;
mod adaptive_merge;
mod adaptive_ranking;
mod adaptive_scenarios;
mod candidate_lab;
mod candidate_layers;
mod candidate_package;
mod candidate_provenance;
mod candidate_runtime;
mod candidate_signature;
mod candidate_slots;
mod candidate_snapshot;
mod capsule_replay;
mod codec;
mod composition;
mod context_rerank;
mod continuous_capture;
mod correction_episode;
mod double_pinyin_paint;
mod evaluation;
mod event_capsule;
mod explicit_alias;
mod language_model;
mod native_feedback;
mod personal_context;
mod personal_ranking;
mod private_session;
mod protocol_audit;
mod public_corpus;
mod public_lexicon_slice;
mod research_analysis;
mod research_feedback;
mod session_summary;
mod shape_course;
mod shape_evaluation;
mod shape_lab;
mod shape_refinement;
mod shape_replay;
mod single_character_pool;
mod stroke_data;
mod tracker;
mod transposition_calibration;
#[cfg(windows)]
mod tsf_alpha;
mod user_tool_layout;
mod user_tool_runtime;
mod wish_command;
mod wish_feedback;

#[cfg(windows)]
pub use tsf_alpha::{
    TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID, TsfCandidatePreflightError,
    TsfCandidatePreflightReport, preflight_candidate_snapshot,
};

pub use abbreviation::{
    AbbreviationAuditError, AbbreviationCodebookAudit, ImmediateAmbiguityWitness,
    audit_abbreviation_codebook,
};
pub use adaptive_comparison::{
    ADAPTIVE_COMPARISON_PROFILE_COUNT, ADAPTIVE_COMPARISON_PROFILES, AdaptiveComparisonDelta,
    AdaptiveComparisonError, AdaptiveComparisonOutcome, AdaptiveComparisonParameters,
    AdaptiveComparisonProfile, AdaptiveComparisonReport,
    compare_public_synthetic_adaptive_profiles,
};
pub use adaptive_coverage::{
    AdaptiveCoverageCandidate, AdaptiveCoverageConfig, AdaptiveCoverageError,
    AdaptiveCoverageReport, AdaptiveCoverageSource, AdaptiveCoverageSummary,
    MAX_ADAPTIVE_COVERAGE_CANDIDATES, MAX_ADAPTIVE_COVERAGE_PUBLIC_TEXTS,
    retrieve_personal_coverage,
};
pub use adaptive_evaluation::{
    AdaptiveEvaluationError, AdaptiveEvaluationErrorKind, AdaptiveEvaluationEvent,
    AdaptiveEvaluationReport, MAX_ADAPTIVE_EVALUATION_EVENTS, evaluate_adaptive_closed_loop,
};
pub use adaptive_memory::{
    ConfirmedSelectionEvidence, ConfirmedSelectionTier, ConfirmedSelectionTierCounts,
    DEFAULT_MAX_CONFIRMED_SELECTIONS, DEFAULT_MAX_LONG_CONFIRMED_SELECTIONS,
    DEFAULT_MAX_MEDIUM_CONFIRMED_SELECTIONS, DEFAULT_MAX_PENDING_SELECTIONS,
    DEFAULT_MAX_RECENT_CONFIRMED_SELECTIONS, MAX_CONFIRMED_SELECTION_LIMIT,
    MAX_PENDING_SELECTION_LIMIT, PendingConfirmationOutcome, PendingEditOutcome,
    PendingForgetOutcome, PendingObservationOutcome, PendingSelectionEdit, PendingSelectionError,
    PendingSelectionLimits, PendingSelectionMemory,
};
pub use adaptive_merge::{
    AdaptiveMergeConfig, AdaptiveMergeError, AdaptiveMergeReport, AdaptiveMergeSummary,
    AdaptiveMergedCandidate, AdaptiveMergedCandidateSource, MAX_ADAPTIVE_COVERAGE_PROBABILITY,
    MAX_ADAPTIVE_MERGED_CANDIDATES, merge_adaptive_candidates,
};
pub use adaptive_ranking::{
    AdaptiveCandidateScore, AdaptiveRankingCandidate, AdaptiveRankingConfig, AdaptiveRankingError,
    AdaptiveRankingReport, MAX_ADAPTIVE_RANKING_CANDIDATES, rank_visible_candidates,
};
pub use adaptive_scenarios::{
    ADAPTIVE_SYNTHETIC_SCENARIO_COUNT, ADAPTIVE_SYNTHETIC_SCENARIOS, AdaptiveSyntheticScenario,
    AdaptiveSyntheticScenarioOutcome, AdaptiveSyntheticSuiteError, AdaptiveSyntheticSuiteReport,
    evaluate_public_synthetic_adaptive_scenarios,
};
pub use candidate_lab::{
    CandidateLabCandidate, CandidateLabError, CandidateLabLane, CandidateLabReport,
    MAX_CANDIDATE_LAB_TOP_K, analyze_candidate_lab,
};
pub use candidate_layers::{
    CANDIDATE_SUPPLEMENTAL_STATE_FILE, CANDIDATE_SUPPLEMENTAL_STATE_SCHEMA_V1,
    CandidateSupplementalState, CandidateSupplementalStateError,
    MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES,
};
pub use candidate_package::{
    CANDIDATE_PACKAGE_LEXICON_TSV_V1, CANDIDATE_PACKAGE_SCHEMA_V1, CandidatePackageError,
    CandidatePackageManifest, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
};
pub use candidate_provenance::{
    CANDIDATE_DECODER_COMPATIBILITY_V1, CANDIDATE_PACKAGE_PROVENANCE_FILE,
    CANDIDATE_PROVENANCE_SCHEMA_V1, CANDIDATE_PROVENANCE_SCHEMA_V2, CandidatePackageProvenance,
    CandidateProvenanceError, CandidateSourceMaterial, MAX_CANDIDATE_PROVENANCE_BYTES,
    MAX_CANDIDATE_PROVENANCE_SOURCES, candidate_package_authentication_sha256,
    candidate_sha256_hex,
};
pub use candidate_runtime::{
    CANDIDATE_PACKAGE_MANIFEST_FILE, CANDIDATE_PACKAGE_PAYLOAD_FILE, CANDIDATE_PACKAGES_DIRECTORY,
    CANDIDATE_PREFLIGHT_HOST_V1, CANDIDATE_PREFLIGHT_RECEIPT_SCHEMA_V2,
    CANDIDATE_PREFLIGHTS_DIRECTORY, CANDIDATE_RUNTIME_DIRECTORY, CANDIDATE_SLOT_STATE_FILE,
    CandidateRuntimeError, CandidateRuntimeSnapshots, CandidateRuntimeSupplemental,
    CandidateRuntimeSupplementalSelection, MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
    candidate_package_storage_id, candidate_preflight_receipt_body,
    load_candidate_runtime_snapshots, load_candidate_runtime_supplemental,
    load_candidate_runtime_supplemental_selection, load_current_candidate_snapshot,
};
pub use candidate_signature::{
    CANDIDATE_RELEASE_SIGNATURE_ALGORITHM_ED25519, CANDIDATE_RELEASE_SIGNATURE_SCHEMA_V1,
    CandidateReleaseSignature, CandidateReleaseSignatureError,
    MAX_CANDIDATE_RELEASE_SIGNATURE_BYTES, candidate_release_signing_message,
};
pub use candidate_slots::{
    CANDIDATE_SLOT_STATE_SCHEMA_V1, CandidateSlotError, CandidateSlotState,
    MAX_CANDIDATE_SLOT_STATE_BYTES,
};
pub use candidate_snapshot::{
    AutomaticTranspositionDecision, AutomaticTranspositionKeepReason,
    AutomaticTranspositionPromotion, CANDIDATE_SNAPSHOT_SCHEMA_V1, CandidateSnapshot,
    CandidateSnapshotDescriptor, CandidateSnapshotError, FourCharacterCorrectionCandidate,
    FourCharacterCorrectionDecision, FourCharacterCorrectionKeepReason,
    FourCharacterCorrectionOffer, LayeredCandidateTextsError, MAX_CANDIDATE_SNAPSHOT_BYTES,
    MAX_CANDIDATE_SNAPSHOT_ENTRIES, MAX_CANDIDATE_SNAPSHOT_RANK,
    MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES, SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH,
    SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH, SupplementalCandidateLayerConfig,
    SupplementalCandidateLayerError, SupplementalCompositionCandidate,
    SupplementalCompositionOrder, SupplementalCompositionSegment,
    SupplementalCompositionSegmentSource, candidate_payload_fingerprint, layered_candidate_texts,
    layered_candidate_texts_with_consensus, layered_four_character_correction_decision,
    merge_candidate_text_layers, supplemental_complete_composition_texts,
    supplemental_complete_composition_texts_with_order,
    supplemental_complete_compositions_with_order,
};
pub use capsule_replay::{
    CapsuleReplayConfigError, CapsuleReplayReport, ContextReplayComparisonStats,
    KeyInterpretationError, MAX_REPLAY_CODE_KEYS, PUBLIC_CONTEXT_REPLAY_POOL_DEPTH,
    PairedReplayStrategyStats, PersonalCacheReplayError, PersonalCacheReplayState,
    RankingReplayComparisonStats, ReplayStrategyStats, WindowExclusionStats, effective_letter_code,
};
pub use codec::{
    EncodedPinyin, PinyinEncodeError, encode_pinyin_phrase, encode_pinyin_syllable,
    normalize_pinyin_tone_marks,
};
pub use composition::{
    CompositionEffect, CompositionInput, CompositionPunctuation, CompositionSession,
    SessionSelectionMemory,
};
pub use context_rerank::{
    CONSERVATIVE_TOP1_CONTEXT_PROFILES, ConservativeTop1ContextCandidate,
    ConservativeTop1ContextMetrics, ConservativeTop1ContextProfile,
    ConservativeTop1ContextRerankReport, FrozenContextCandidate, FrozenContextPairEvidence,
    FrozenContextProbe, FrozenContextRerankReport, FrozenHybridContextCandidate,
    FrozenHybridContextMetrics, FrozenHybridContextRerankReport, HYBRID_CONTEXT_PROFILES,
    HybridContextProfile, SEGMENTATION_CONTEXT_PROFILES, audit_conservative_top1_context,
    audit_frozen_hybrid_context, rerank_frozen_sentence_pool,
    rerank_frozen_sentence_pool_conservative_top1, rerank_frozen_sentence_pool_hybrid,
    rerank_frozen_sentence_pool_hybrid_with_variants,
};
#[cfg(windows)]
pub use continuous_capture::WindowsUserDataProtector;
pub use continuous_capture::{
    CAPTURE_INTEGRITY_SCHEMA_V1, CODEX_CAPTURE_PROFILE_V1, CODEX_CAPTURE_PROFILE_V2,
    CONTINUOUS_PRODUCER_VERSION, CONTINUOUS_SEGMENT_SCHEMA_V1, CONTINUOUS_SEGMENT_SCHEMA_V2,
    CaptureIntegrityCountersV1, CaptureIntegrityV1, CaptureSessionKind, ContinuousCaptureError,
    ContinuousSegmentMetadata, ContinuousSegmentV1, ContinuousSegmentV2, DataProtector,
    DecodedContinuousSegment, PROTECTED_SEGMENT_SCHEMA_V1, ProtectedSegmentEnvelopeV1,
    ProtectedSegmentWriter, ProtectedSegmentWriterConfig, SegmentCloseReason, SegmentWriteReceipt,
};
pub use correction_episode::{
    CommitTailTrimDetector, CommitTailTrimObservation, CorrectionCandidate,
    CorrectionCandidateDetector, CorrectionCandidateForm, CorrectionCandidateKind,
    CorrectionDetectorError,
};
pub use double_pinyin_paint::{
    HALF_PAIR_PAINT_PROFILES, HALF_PAIR_SYNTHETIC_CADENCES, HalfPairFrameDisposition,
    HalfPairInputEffect, HalfPairPaint, HalfPairPaintAuditReport, HalfPairPaintCoalescer,
    HalfPairPaintProfile, HalfPairSyntheticCadence, audit_half_pair_paint_profiles,
};
pub use evaluation::{
    CharacterAverageMarginRange, CharacterContextOracleReport, CompositionRecallReport,
    ContextOracleError, ContextOracleReport, ContextScoreMarginRange,
    ContinuousCompositionAuditCase, ContinuousCompositionAuditReport, ContinuousCompositionReport,
    EvaluationReport, LabeledRecallReport, LabeledRejectionShadowReport,
    LabeledRejectionThresholdMetrics, LabeledSentenceProbe, OovCaseReport, ProbeSpellingMode,
    REJECTION_SHADOW_THRESHOLDS_PER_KEY, RecallMetrics, RejectionMarginRange,
    RejectionShadowReport, RejectionThresholdMetrics, SentenceCaseParseError, SentenceCaseReport,
    SyntheticCaseKind, audit_continuous_composition, evaluate_character_context_oracle,
    evaluate_context_oracle, evaluate_continuous_composition, evaluate_labeled_recall,
    evaluate_labeled_rejection_shadow, evaluate_oov_cases, evaluate_rejection_shadow,
    evaluate_sentence_cases, evaluate_synthetic,
};
pub use event_capsule::{
    EVENT_CAPSULE_SCHEMA_V1, EventCapsuleError, EventCapsuleRecorder, EventCapsuleV1,
    MAX_EVENT_CAPSULE_EVENTS, MAX_EVENT_CAPSULE_KEYS_PER_EVENT,
    MAX_EVENT_CAPSULE_TEXT_BYTES_PER_FIELD, MAX_EVENT_CAPSULE_TOTAL_TEXT_BYTES, TimedTrackerOutput,
};
pub use explicit_alias::{
    EXPLICIT_ALIAS_PACKAGE_FILE, EXPLICIT_ALIAS_PACKAGES_DIRECTORY, EXPLICIT_ALIAS_SCHEMA_V1,
    EXPLICIT_ALIAS_SLOT_FILE, EXPLICIT_ALIAS_SLOT_SCHEMA_V1, ExplicitAliasError,
    ExplicitAliasSlotState, ExplicitAliasSnapshot, LoadedExplicitAliasSnapshot,
    MAX_EXPLICIT_ALIAS_ENTRIES, MAX_EXPLICIT_ALIAS_PACKAGE_BYTES,
    MAX_EXPLICIT_ALIAS_PLAINTEXT_BYTES, MAX_EXPLICIT_ALIAS_SLOT_BYTES, explicit_alias_package_id,
    load_current_explicit_alias_snapshot, load_explicit_alias_package,
    load_explicit_alias_slot_state, protect_explicit_alias_snapshot,
    unprotect_explicit_alias_snapshot,
};
pub use language_model::{
    BigramLanguageModel, BigramLanguageModelStats, BigramScore, CharacterBigramLanguageModel,
    CharacterBigramLanguageModelStats, CharacterLanguageModelError, CharacterSequenceScore,
    LanguageModelParseError,
};
pub use native_feedback::{
    DEFAULT_NATIVE_FEEDBACK_MAX_EVENTS, DEFAULT_NATIVE_FEEDBACK_MAX_PRIVATE_BYTES,
    DEFAULT_NATIVE_FEEDBACK_WISH_EPISODE_MAX_LOOKBACK_MS, DEFAULT_NATIVE_FEEDBACK_WISH_EPISODES,
    DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS, DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
    FrozenNativeFeedbackEvent, FrozenNativeFeedbackSnapshot, MAX_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS, NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS,
    NativeAutomaticTranspositionDecision, NativeAutomaticTranspositionOutcome,
    NativeAutomaticTranspositionTier, NativeCancellationSource, NativeCandidateProvenance,
    NativeCandidateSource, NativeCandidateView, NativeFeedbackAuthorization,
    NativeFeedbackClearResult, NativeFeedbackContext, NativeFeedbackEvent,
    NativeFeedbackFreezeAuthorization, NativeFeedbackFreezeError, NativeFeedbackLifecycle,
    NativeFeedbackLimits, NativeFeedbackRecordResult, NativeFeedbackSession,
    NativeFeedbackStartResult, NativeFeedbackStopReason, NativeFeedbackStopResult,
    NativeFeedbackSummary, NativeSelectionSource, NativeTabAssemblyState,
};
pub use personal_context::{
    MAX_PERSONAL_CONTEXT_CODE_KEYS, MAX_PERSONAL_CONTEXT_ENTRIES,
    MAX_PERSONAL_CONTEXT_TEXT_CHARACTERS, PERSONAL_CONTEXT_REJECTION_CAP,
    PERSONAL_CONTEXT_SEARCH_DEPTH, PERSONAL_CONTEXT_SUPPORT_CAP, PersonalContextError,
    PersonalContextRanking,
};
pub use personal_ranking::{
    LoadedPersonalRanking, LoadedPersonalRankingSuppressions, MAX_PERSONAL_RANKING_BATCH_EVENTS,
    MAX_PERSONAL_RANKING_BATCH_FILES, MAX_PERSONAL_RANKING_CHECKPOINT_FILES,
    MAX_PERSONAL_RANKING_ENTRIES, MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_FILES,
    MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES, PERSONAL_RANKING_BATCH_EXTENSION,
    PERSONAL_RANKING_BATCH_SCHEMA_V1, PERSONAL_RANKING_CHECKPOINT_EXTENSION,
    PERSONAL_RANKING_CHECKPOINT_SCHEMA_V1, PERSONAL_RANKING_SUPPRESSION_ACTION_EXTENSION,
    PERSONAL_RANKING_SUPPRESSION_ACTION_SCHEMA_V1, PERSONAL_RANKING_SUPPRESSION_DIRECTORY,
    PersonalRankingBatch, PersonalRankingError, PersonalRankingSelection, PersonalRankingSnapshot,
    PersonalRankingSuppressionAction, PersonalRankingSuppressionActionKind,
    PersonalRankingSuppressionSnapshot, load_personal_ranking, load_personal_ranking_suppressions,
    personal_ranking_package_file, personal_ranking_suppression_package_file,
    protect_personal_ranking_batch, protect_personal_ranking_suppression_action,
    refresh_personal_ranking, refresh_personal_ranking_suppressions, save_personal_ranking_batch,
    save_personal_ranking_checkpoint, save_personal_ranking_suppression_action,
    unprotect_personal_ranking_batch, unprotect_personal_ranking_suppression_action,
};
pub use private_session::{
    ProtectedSessionError, ProtectedSessionErrorKind, ProtectedSessionReader,
    ProtectedSessionSegment,
};
pub use protocol_audit::{
    AnchoredTailFailureAuditReport, AnchoredTailFailureCase, COLLISION_GATED_TAIL_PROFILES,
    CollisionGatedTailProfile, CollisionGatedTailReport, DoublePinyinTrajectoryLaneReport,
    DoublePinyinTrajectoryReport, ProtocolContextLaneReport, ProtocolIndexStats,
    ProtocolStrategyReport, PublicProtocolAuditReport, PublicProtocolContextAuditReport,
    WhitelistProtocolReport, audit_anchored_tail_failures, audit_collision_gated_tail_protocols,
    audit_double_pinyin_key_trajectories, audit_public_protocol_context, audit_public_protocols,
    audit_terminal_collision_gated_tail_protocols,
};
pub use public_corpus::{
    ContinuousCompositionProbe, ContinuousCompositionSelection,
    ContinuousCompositionSelectionStats, PublicBigramTrainingCorpus, PublicBigramTrainingStats,
    PublicCalibrationSelection, PublicCalibrationSelectionStats, PublicCharacterTrainingCorpus,
    PublicCharacterTrainingStats, PublicLexiconRankProbe, PublicLexiconRankSelection,
    PublicLexiconTokenCoverageAudit, PublicLexiconTokenCoverageByLength, PublicProtocolProbe,
    PublicProtocolSelection, PublicProtocolSelectionStats, PublicStaticContextProbe,
    PublicStaticContextSelection, PublicStaticContextSelectionStats,
    PublicSupplementalCompositionProbe, PublicSupplementalCompositionSelection,
    PublicSupplementalCompositionSelectionStats, UdCorpus, UdCorpusImportStats, UdCorpusParseError,
    audit_public_lexicon_token_coverage, parse_ud_conllu, select_public_bigram_training_sequences,
    select_public_calibration_cases, select_public_character_training_texts,
    select_public_continuous_composition_cases, select_public_lexicon_rank_probes,
    select_public_protocol_audit_cases, select_public_static_context_cases,
    select_public_supplemental_composition_cases,
};
pub use public_lexicon_slice::{
    MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES, MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES,
    MAX_PUBLIC_RIME_SLICE_ENTRIES, MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES,
    MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS, PublicLexiconComparison,
    PublicRimePhraseAllowlistImport, PublicRimePhraseAllowlistImportStats, PublicRimeSliceConfig,
    PublicRimeSliceError, PublicRimeSliceImport, PublicRimeSliceImportStats, PublicRimeTargetAudit,
    PublicSupplementalLayerAudit, PublicSupplementalLayerAuditError, audit_public_rime_target,
    audit_public_supplemental_layer, compare_public_lexicons, parse_public_rime_phrase_allowlist,
    parse_public_rime_slice,
};
pub use research_analysis::{
    ResearchHabitClue, ResearchHabitKind, ResearchHalfPairAnalysis, ResearchSceneAnalysis,
    ResearchSceneError, analyze_linked_research, analyze_runtime_half_pairs,
};
pub use research_feedback::{
    RESEARCH_FEEDBACK_CONSENT_FILE, RESEARCH_FEEDBACK_CONSENT_SCHEMA_V1,
    RESEARCH_FEEDBACK_DIRECTORY, ResearchFeedbackError, research_feedback_enabled,
    set_research_feedback_enabled,
};
pub use session_summary::{
    AggregatedSessionSummary, SESSION_SUMMARY_SCHEMA_V1, SessionSummaryCounts, SessionSummaryError,
    SessionSummaryV1,
};
pub use shape_course::{
    MAX_INTERACTIVE_SHAPE_COURSE_TASKS, ShapeCourseDifficulty, ShapeCourseTask,
    select_shape_course_tasks,
};
pub use shape_evaluation::{
    SHAPE_COURSE_MAX_PREFIX_KEYS, SHAPE_COURSE_VISIBLE_LIMIT, ShapeCourseAuditReport,
    ShapePrefixCourseStats, audit_shape_refinement_course,
};
pub use shape_lab::{
    MAX_SHAPE_LAB_VISIBLE, ShapeLab, ShapeLabCandidate, ShapeLabError, ShapeLabSnapshot,
};
pub use shape_refinement::{
    CharacterShape, CharacterShapeIndex, RefinedCandidate, ShapeMatchEvidence,
    ShapeRefinementError, TabShapeQuery, TabShapeRefinementReport,
};
pub use shape_replay::{
    PHRASE_TRIM_MAX_CHARACTERS, PHRASE_TRIM_MAX_GAP_MS, PrivateShapeActionComparisonStats,
    PrivateShapeReplayAudit, PrivateShapeReplayReport,
};
pub use stroke_data::{
    LexiconShapeCoverageStats, MAX_STROKE_DATA_ASSIGNMENTS, MAX_STROKE_DATA_LINE_BYTES,
    MAX_STROKE_DATA_ROWS, MAX_STROKE_SEQUENCE_LENGTH, MAX_STROKE_SEQUENCES_PER_CHARACTER,
    StrokeDataParseError, StrokeSequenceImport, StrokeSequenceImportStats,
    audit_lexicon_shape_coverage, parse_stroke_sequence_tsv,
};
pub use tracker::{
    CommitRecord, DeltaPositionEvidence, LocalInputTracker, RawKey, RevisionRecord, TextDelta,
    TextSelection, TrackerOutput, single_span_delta, single_span_delta_with_selection,
};
pub use transposition_calibration::{
    TranspositionCalibrationConfig, TranspositionCalibrationError, TranspositionCalibrationLabel,
    TranspositionCalibrationObservation, TranspositionCalibrationRecommendation,
    TranspositionCalibrationSummary, TranspositionCalibrator,
};
pub use user_tool_layout::{
    repository_root_for_desktop_launcher_executable, repository_root_for_user_tool_executable,
};
pub use user_tool_runtime::{
    LaunchableUserTool, MANAGED_USER_TOOL_NAMES, UserToolRuntimeError, resolve_current_user_tool,
};
#[cfg(windows)]
pub use wish_command::{WISH_ACK_COMPARTMENT_GUID, WISH_COMMAND_COMPARTMENT_GUID};
pub use wish_command::{
    WishCommand, WishCommandAck, WishCommandAckStatus, WishCommandDispatchError,
    WishCommandDispatchReceipt, dispatch_wish_command,
};
pub use wish_feedback::{
    MAX_WISH_NOTE_BYTES, MAX_WISH_PACKAGE_BYTES, WISH_NOTE_FILE_SUFFIX, WISH_PACKAGE_FILE_SUFFIX,
    WISH_SCHEMA_V1, WISH_SCHEMA_V2, WISH_SCHEMA_V3, WISH_SCHEMA_V4, WISH_SCHEMA_V5, WISH_SCHEMA_V6,
    WISH_SCHEMA_V7, WISH_SCHEMA_V8, WISH_SCHEMA_V9, WISH_SCHEMA_V10, WishCaptureScope,
    WishCategory, WishEventRole, WishFeedbackError, WishImportance, WishJournalAnchor,
    WishJournalContext, WishJournalSpan, WishNote, WishPackageInfo, WishReviewStatus,
    WishRuntimeIdentity, WishSaveReceipt, WishSnapshot, list_wish_packages, load_wish_note,
    load_wish_snapshot, move_wish_to_trash, save_or_replace_wish_note, save_wish_note,
    save_wish_snapshot,
};

const BIGRAM_INTERPOLATION_WEIGHT: f64 = 0.65;
const MAX_LEXICON_SYLLABLES: usize = 12;

/// A validated, lowercase ASCII key sequence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeySequence(String);

impl KeySequence {
    /// Validates and constructs a key sequence.
    pub fn new(value: impl Into<String>) -> Result<Self, KeySequenceError> {
        let value = value.into();
        if value.is_empty()
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_lowercase())
        {
            return Err(KeySequenceError { value });
        }
        Ok(Self(value))
    }

    /// Returns the underlying key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Error returned for an empty or non-lowercase-ASCII key sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeySequenceError {
    value: String,
}

impl fmt::Display for KeySequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "按键串必须是非空的小写英文字母，实际收到 {:?}",
            self.value
        )
    }
}

impl Error for KeySequenceError {}

/// One entry in the deliberately small public lexicon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexiconEntry {
    /// Candidate Chinese text.
    pub text: String,
    /// Space-separated full pinyin, kept for auditability.
    pub pinyin: String,
    /// Canonical full Ziranma key sequence.
    pub code: KeySequence,
    /// One canonical two-key code per pinyin syllable.
    pub syllable_codes: Vec<KeySequence>,
    /// Synthetic relative weight, not a measured corpus count.
    pub frequency: u64,
}

/// A supported relationship between observed and intended keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Correction {
    /// The observed keys exactly match the spelling interpretation.
    Exact,
    /// One intended key was observed as a physical QWERTY neighbor.
    NeighborSubstitution {
        /// Zero-based byte index in the ASCII key sequence.
        index: usize,
        /// Key prescribed by the spelling interpretation.
        intended: char,
        /// Key actually observed.
        actual: char,
    },
    /// Two adjacent key presses arrived in reverse order.
    AdjacentTransposition {
        /// Zero-based index of the first intended key.
        start: usize,
        /// First key in the intended order.
        intended_left: char,
        /// Second key in the intended order.
        intended_right: char,
    },
    /// One intended key never arrived.
    MissingKey {
        /// Zero-based position in the intended sequence.
        index: usize,
        /// Key that should have appeared.
        intended: char,
    },
    /// One unintended key arrived in the observed sequence.
    ExtraKey {
        /// Zero-based position in the observed sequence.
        index: usize,
        /// Unintended observed key.
        actual: char,
    },
}

impl Correction {
    /// Returns a concise, human-readable explanation.
    pub fn description(&self) -> String {
        match self {
            Self::Exact => "原样匹配，没有纠错".to_owned(),
            Self::NeighborSubstitution {
                index,
                intended,
                actual,
            } => format!(
                "第 {} 键发生邻键替换：本想按 {intended}，实际按到 {actual}",
                index + 1
            ),
            Self::AdjacentTransposition {
                start,
                intended_left,
                intended_right,
            } => format!(
                "第 {}、{} 键顺序颠倒：原顺序为 {intended_left}{intended_right}",
                start + 1,
                start + 2
            ),
            Self::MissingKey { index, intended } => {
                format!("第 {} 键遗漏：本应按 {intended}", index + 1)
            }
            Self::ExtraKey { index, actual } => {
                format!("第 {} 键多按：多出了 {actual}", index + 1)
            }
        }
    }
}

/// One full-code or mixed-abbreviation interpretation of a lexicon entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spelling {
    /// Key sequence used by this interpretation.
    pub code: KeySequence,
    /// Zero-based syllable indices represented by only their first key.
    pub abbreviated_syllables: Vec<usize>,
}

impl Spelling {
    /// Describes which syllables used one-key abbreviation.
    pub fn description(&self) -> String {
        if self.abbreviated_syllables.is_empty() {
            return "全部音节使用完整双拼".to_owned();
        }
        let positions = self
            .abbreviated_syllables
            .iter()
            .map(|index| (index + 1).to_string())
            .collect::<Vec<_>>()
            .join("、");
        format!("第 {positions} 个音节使用一键简拼")
    }
}

/// Components used to rank a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreBreakdown {
    /// Natural logarithm of the entry's synthetic relative frequency.
    ///
    /// Unresolved-input candidates use zero because they have no lexicon
    /// frequency evidence.
    pub frequency: f64,
    /// Cost associated with the detected key correction.
    pub correction_penalty: f64,
    /// Cost associated with one-key syllable abbreviations.
    pub abbreviation_penalty: f64,
    /// Cost for retaining keys that the lexicon cannot explain.
    pub unresolved_input_penalty: f64,
    /// `frequency - correction_penalty - abbreviation_penalty -
    /// unresolved_input_penalty`.
    pub total: f64,
}

/// Origin of an explainable candidate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CandidateSource {
    /// A normal entry from the configured lexicon.
    Lexicon,
    /// Literal observed input retained without inventing pinyin or Chinese.
    UnresolvedInput,
}

/// A decoded candidate together with its complete explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Whether this candidate came from the lexicon or retained raw input.
    pub source: CandidateSource,
    /// Candidate Chinese text, or an explicit `〔x〕` unresolved marker.
    pub text: String,
    /// Full pinyin recorded in the public fixture; empty when unresolved.
    pub pinyin: String,
    /// Canonical full Ziranma code, or the literal unresolved key.
    pub code: KeySequence,
    /// Full-code or mixed-abbreviation spelling matched by the decoder.
    pub spelling: Spelling,
    /// How the observed input relates to the matched spelling.
    pub correction: Correction,
    /// Transparent score components.
    pub score: ScoreBreakdown,
}

/// One word-level decision inside a multi-word decoding path.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceSegment {
    /// Slice of observed keys consumed by this segment.
    pub observed: KeySequence,
    /// Word candidate and its local spelling/error explanation.
    pub candidate: Candidate,
    /// Context-sensitive language evidence for this segment.
    pub language_score: SentenceLanguageScore,
}

/// Explainable unigram/bigram interpolation for one sentence segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SentenceLanguageScore {
    /// `ln P(word)` from normalized synthetic lexicon weights.
    pub unigram_log_probability: f64,
    /// Conditional evidence when a previous word and bigram model exist.
    pub bigram: Option<BigramScore>,
    /// Language log score used by the dynamic program.
    pub interpolated_log_probability: f64,
}

/// A complete segmentation of an unseparated key sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceCandidate {
    /// Concatenated Chinese output.
    pub text: String,
    /// Ordered word decisions and consumed key slices.
    pub segments: Vec<SentenceSegment>,
    /// Sum of normalized unigram log probabilities and local penalties.
    pub total_score: f64,
    /// Number of literal input keys not explained by the lexicon.
    pub unresolved_key_count: usize,
    /// Whether the complete path consumed the global error budget.
    pub used_error: bool,
}

/// Candidate lanes for an interactive, continuously composed key sequence.
///
/// The primary lane preserves the conservative exact-before-correction order.
/// The recovery lane separately exposes one adjacent transposition inside the
/// high-signal “full first syllable, abbreviated suffix” word shape, so a
/// plausible typo cannot be hidden behind an arbitrarily large exact
/// abbreviation set.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceCandidateLanes {
    /// Conservatively ranked primary candidates.
    pub primary: Vec<SentenceCandidate>,
    /// Anchored suffix-abbreviation candidates using one transposition.
    pub anchored_transposition_recovery: Vec<SentenceCandidate>,
}

/// Tunable penalties for the research decoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeConfig {
    /// Penalty for one QWERTY-neighbor substitution.
    pub neighbor_substitution_penalty: f64,
    /// Penalty for one adjacent transposition.
    pub adjacent_transposition_penalty: f64,
    /// Penalty for one missing intended key.
    pub missing_key_penalty: f64,
    /// Penalty for one extra observed key.
    pub extra_key_penalty: f64,
    /// Penalty for each syllable represented by only its first key.
    pub abbreviation_penalty_per_syllable: f64,
    /// Penalty for each input key retained as explicitly unresolved.
    pub unresolved_key_penalty: f64,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            neighbor_substitution_penalty: 2.75,
            adjacent_transposition_penalty: 2.25,
            missing_key_penalty: 3.00,
            extra_key_penalty: 3.00,
            abbreviation_penalty_per_syllable: 0.85,
            unresolved_key_penalty: 8.00,
        }
    }
}

/// Inspectable structural statistics for the decoder's compact syllable index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecoderIndexStats {
    /// Number of trie nodes, including the root.
    pub node_count: usize,
    /// Number of syllable-labelled edges.
    pub edge_count: usize,
    /// Number of stored lexicon terminals.
    pub terminal_count: usize,
    /// Trie nodes that hold at least one lexicon terminal.
    pub terminal_node_count: usize,
    /// Largest number of homophonous entries stored at one terminal node.
    pub maximum_terminal_fanout: usize,
    /// Number of full-code/abbreviation spellings represented implicitly.
    pub represented_spelling_count: usize,
    /// Largest syllable count among indexed entries.
    pub maximum_syllables: usize,
}

/// Work performed by one word-level joint trie search.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecodeSearchStats {
    /// Recursive trie path states visited.
    pub trie_path_visits: usize,
    /// Trie subtrees skipped by an exact unigram Top-K upper bound.
    pub trie_subtree_prunes: usize,
    /// Alignment states actually examined while consuming intended keys.
    pub alignment_states_examined: usize,
    /// Alignment-state checks avoided by exact per-scan transition reuse.
    pub alignment_states_reused: usize,
    /// Terminal spelling matches produced before per-entry deduplication.
    pub terminal_spelling_matches: usize,
}

/// Work performed while constructing and ranking one sentence lattice.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SentenceSearchStats {
    /// Trie scans started from active word-boundary states.
    pub segment_trie_scans: usize,
    /// Recursive trie path states visited across all scans.
    pub trie_path_visits: usize,
    /// Path visits spent on exact-only evidence scans for prefix bounds.
    pub exact_prefix_prepass_visits: usize,
    /// Lexicon entries inspected while building exact-only prefix evidence.
    pub exact_prefix_prepass_entry_visits: usize,
    /// Trie subtrees skipped by exact unigram Top-K upper bounds.
    pub trie_subtree_prunes: usize,
    /// Alignment states actually examined across all scans.
    pub alignment_states_examined: usize,
    /// Alignment-state checks avoided by exact per-scan transition reuse.
    pub alignment_states_reused: usize,
    /// Terminal spelling paths found before expanding their lexicon entries.
    pub terminal_path_matches: usize,
    /// Terminal lexicon entries expanded after exact entry-bound stopping.
    pub terminal_spelling_matches: usize,
    /// Sorted terminal entries skipped by exact unigram Top-K cutoffs.
    pub terminal_entry_bound_skips: usize,
    /// Deduplicated lexicon and unresolved edges generated for the lattice.
    pub lattice_transitions: usize,
    /// One-key unresolved-input edges included in `lattice_transitions`.
    pub unresolved_lattice_transitions: usize,
    /// Generated edges whose complete candidate objects were materialized.
    pub lattice_transitions_materialized: usize,
    /// Generated edges retained in the final lattice after exact reduction.
    pub lattice_transitions_retained: usize,
    /// Retained unresolved-input edges.
    pub unresolved_lattice_transitions_retained: usize,
    /// Distinct `(position, error budget, previous word)` ranking states solved.
    pub ranking_states_evaluated: usize,
    /// Reuses of an already solved ranking state.
    pub ranking_state_cache_hits: usize,
    /// Lattice transitions examined across solved ranking states.
    pub ranking_transitions_considered: usize,
    /// Transitions retained after exact same-future-state Top-K reduction.
    pub ranking_transitions_retained: usize,
    /// Edge/suffix combinations considered by the k-best dynamic program.
    pub path_combinations_considered: usize,
}

/// Trie-backed decoder over a local lexicon.
#[derive(Clone, Debug)]
pub struct Decoder {
    lexicon: Vec<LexiconEntry>,
    entry_identity_ids: Vec<usize>,
    entry_identity_max_frequencies: Vec<u64>,
    trie: SyllableTrie,
    language_model: Option<BigramLanguageModel>,
    config: DecodeConfig,
}

impl Decoder {
    /// Creates a decoder with conservative default penalties.
    pub fn new(lexicon: Vec<LexiconEntry>) -> Self {
        Self::with_config(lexicon, DecodeConfig::default())
    }

    /// Creates a decoder with explicit penalties.
    ///
    /// # Panics
    ///
    /// Panics if any penalty is negative or non-finite. Configuration is
    /// programmer-owned in this milestone rather than loaded from user input.
    pub fn with_config(lexicon: Vec<LexiconEntry>, config: DecodeConfig) -> Self {
        let penalties = [
            config.neighbor_substitution_penalty,
            config.adjacent_transposition_penalty,
            config.missing_key_penalty,
            config.extra_key_penalty,
            config.abbreviation_penalty_per_syllable,
            config.unresolved_key_penalty,
        ];
        assert!(
            penalties
                .iter()
                .all(|penalty| penalty.is_finite() && *penalty >= 0.0),
            "all penalties must be finite and non-negative"
        );
        let entry_identity_ids = lexicon_entry_identity_ids(&lexicon);
        let entry_identity_max_frequencies =
            lexicon_entry_identity_max_frequencies(&lexicon, &entry_identity_ids);
        let trie = SyllableTrie::new(&lexicon);
        Self {
            lexicon,
            entry_identity_ids,
            entry_identity_max_frequencies,
            trie,
            language_model: None,
            config,
        }
    }

    /// Attaches a local bigram model for context-sensitive sentence ranking.
    pub fn with_bigram_model(mut self, language_model: BigramLanguageModel) -> Self {
        self.language_model = Some(language_model);
        self
    }

    /// Returns structural statistics for auditing index compactness.
    pub fn index_stats(&self) -> DecoderIndexStats {
        DecoderIndexStats {
            node_count: self.trie.nodes.len(),
            edge_count: self.trie.nodes.iter().map(|node| node.children.len()).sum(),
            terminal_count: self
                .trie
                .nodes
                .iter()
                .map(|node| node.terminals.len())
                .sum(),
            terminal_node_count: self
                .trie
                .nodes
                .iter()
                .filter(|node| !node.terminals.is_empty())
                .count(),
            maximum_terminal_fanout: self
                .trie
                .nodes
                .iter()
                .map(|node| node.terminals.len())
                .max()
                .unwrap_or(0),
            represented_spelling_count: self.trie.represented_spelling_count,
            maximum_syllables: self.trie.maximum_syllables,
        }
    }

    /// Returns at most `top_k` matching candidates in deterministic score order.
    pub fn decode(&self, observed: &str, top_k: usize) -> Result<Vec<Candidate>, KeySequenceError> {
        self.decode_with_stats(observed, top_k)
            .map(|(candidates, _stats)| candidates)
    }

    /// Decodes one word-level input and returns inspectable search work.
    pub fn decode_with_stats(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<(Vec<Candidate>, DecodeSearchStats), KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 {
            return Ok((Vec::new(), DecodeSearchStats::default()));
        }

        let mut stats = DecodeSearchStats::default();
        let mut candidates = self.lookup_candidates_with_stats(observed.as_str(), true, &mut stats);

        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        Ok((candidates, stats))
    }

    /// Returns complete whole-word candidates reached by exactly one key
    /// correction for a fixed syllable count.
    ///
    /// This narrow research view bypasses exact sentence-segmentation
    /// crowding without changing the conservative ordinary decoder order. It
    /// rejects abbreviations and keeps the decoder's existing single global
    /// error budget. Interactive policy must still decide whether the public
    /// whole-word evidence is sufficiently unambiguous to display.
    pub(crate) fn decode_complete_word_single_edit(
        &self,
        observed: &str,
        syllable_count: usize,
        top_k: usize,
    ) -> Result<Vec<Candidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 || syllable_count == 0 || syllable_count > MAX_LEXICON_SYLLABLES {
            return Ok(Vec::new());
        }
        let intended_keys = syllable_count * 2;
        if observed.as_str().len().abs_diff(intended_keys) > 1 {
            return Ok(Vec::new());
        }

        let mut stats = DecodeSearchStats::default();
        let mut best_by_entry = HashMap::<usize, Candidate>::new();
        for terminal in self.trie.lookup_noisy(observed.as_str(), true, &mut stats) {
            let entry = &self.lexicon[terminal.entry_index];
            let candidate = self.make_candidate(entry, terminal.spelling, terminal.correction);
            if candidate.text.chars().count() != syllable_count
                || candidate.code.as_str().len() != intended_keys
                || !candidate.spelling.abbreviated_syllables.is_empty()
                || candidate.correction == Correction::Exact
            {
                continue;
            }
            match best_by_entry.entry(terminal.entry_index) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(candidate);
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if candidate_order(&candidate, slot.get()) == Ordering::Less {
                        slot.insert(candidate);
                    }
                }
            }
        }
        let mut candidates = best_by_entry.into_values().collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates.dedup_by(|left, right| left.text == right.text && left.code == right.code);
        candidates.truncate(top_k);
        Ok(candidates)
    }

    /// Returns exact lexicon entries whose complete code is the observed
    /// input.
    ///
    /// Interactive hosts use this bounded view before the more permissive
    /// sentence decoder so a complete code cannot disappear behind numerous
    /// high-frequency abbreviation paths. For the unambiguous `ju` / `qu` /
    /// `xu` / `yu` pinyin spelling convention, an observed second-key `u`
    /// also follows the canonical Ziranma `v` edge without consuming the
    /// decoder's error budget.
    pub(crate) fn decode_exact_full_code(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<Vec<Candidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 {
            return Ok(Vec::new());
        }
        if observed.as_str().len() % 2 != 0 {
            return Ok(Vec::new());
        }

        let mut nodes = vec![0_usize];
        for chunk in observed.as_str().as_bytes().chunks_exact(2) {
            let exact = [chunk[0], chunk[1]];
            let umlaut_alias = (chunk[1] == b'u' && matches!(chunk[0], b'j' | b'q' | b'x' | b'y'))
                .then_some([chunk[0], b'v']);
            let mut next = Vec::new();
            for &node in &nodes {
                if let Some(edge) = self.trie.nodes[node]
                    .children
                    .iter()
                    .find(|edge| edge.code == exact)
                {
                    next.push(edge.child);
                }
                if let Some(alias) = umlaut_alias.as_ref()
                    && let Some(edge) = self.trie.nodes[node]
                        .children
                        .iter()
                        .find(|edge| edge.code == *alias)
                {
                    next.push(edge.child);
                }
            }
            next.sort_unstable();
            next.dedup();
            if next.is_empty() {
                return Ok(Vec::new());
            }
            nodes = next;
        }

        let mut candidates = nodes
            .into_iter()
            .flat_map(|node| self.trie.nodes[node].terminals.iter().copied())
            .map(|entry_index| {
                self.make_candidate(
                    &self.lexicon[entry_index],
                    Spelling {
                        code: observed.clone(),
                        abbreviated_syllables: Vec::new(),
                    },
                    Correction::Exact,
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        Ok(candidates)
    }

    /// Decodes an unsegmented sequence through complete, exact double-pinyin
    /// pairs only.
    ///
    /// This narrow frontier is intended for interactive candidate protection:
    /// free abbreviations, corrections, and unresolved edges must not consume
    /// the bounded search slots before a fully typed sentence can be seen.
    /// It does not replace or reorder the ordinary research decoder.
    pub fn decode_complete_sentence(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 || !observed.as_str().len().is_multiple_of(2) {
            return Ok(Vec::new());
        }

        let frequency_total = self
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = if frequency_total > 0.0 {
            frequency_total.ln()
        } else {
            0.0
        };
        let mut search_stats = SentenceSearchStats::default();
        let lattice = self.build_complete_sentence_lattice(observed.as_str(), &mut search_stats);
        let initial_state = SentenceRankingState {
            position: 0,
            used_error: false,
            previous_word: None,
        };
        let ranking_config = SentenceRankingConfig {
            top_k,
            segmentations_per_text: 1,
            log_frequency_total,
        };
        let mut memo = HashMap::new();
        let mut candidates = self.k_best_from_state(
            &lattice,
            initial_state,
            ranking_config,
            &mut memo,
            &mut search_stats,
        );
        candidates.sort_by(sentence_order);
        candidates.truncate(top_k);
        Ok(candidates)
    }

    /// Decodes an odd-length interactive prefix whose completed syllables use
    /// exact double-pinyin pairs and whose final syllable has only its initial
    /// key so far.
    ///
    /// This is deliberately narrower than ordinary mixed abbreviation: the
    /// unfinished final syllable is the only allowed abbreviation, and no key
    /// correction or unresolved edge is admitted. It gives the host a useful
    /// low-volatility lane while the user is between the two keys of a pair.
    pub(crate) fn decode_complete_sentence_with_final_initial(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 || observed.as_str().len().is_multiple_of(2) {
            return Ok(Vec::new());
        }

        let frequency_total = self
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = if frequency_total > 0.0 {
            frequency_total.ln()
        } else {
            0.0
        };
        let mut search_stats = SentenceSearchStats::default();
        let lattice = self.build_complete_sentence_lattice_with_final_initial(
            observed.as_str(),
            &mut search_stats,
        );
        let initial_state = SentenceRankingState {
            position: 0,
            used_error: false,
            previous_word: None,
        };
        let ranking_config = SentenceRankingConfig {
            top_k,
            segmentations_per_text: 1,
            log_frequency_total,
        };
        let mut memo = HashMap::new();
        let mut candidates = self.k_best_from_state(
            &lattice,
            initial_state,
            ranking_config,
            &mut memo,
            &mut search_stats,
        );
        candidates.sort_by(sentence_order);
        candidates.truncate(top_k);
        Ok(candidates)
    }

    /// Jointly infers word boundaries, mixed abbreviations, and at most one
    /// local key error across the complete sequence.
    ///
    /// A streaming trie scan first builds a word lattice. A memoized k-best
    /// dynamic program then ranks unique paths by input position, global error
    /// budget, and previous-word language state.
    pub fn decode_sentence(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
        self.decode_sentence_with_stats(observed, top_k)
            .map(|(candidates, _stats)| candidates)
    }

    /// Jointly decodes an unsegmented input and returns lattice search work.
    pub fn decode_sentence_with_stats(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<(Vec<SentenceCandidate>, SentenceSearchStats), KeySequenceError> {
        let (mut candidates, search_stats) =
            self.decode_sentence_frontier_with_stats(observed, top_k)?;
        candidates.sort_by(sentence_order);
        candidates.truncate(top_k);
        Ok((candidates, search_stats))
    }

    /// Returns a bounded diagnostic frontier that may contain several word
    /// segmentations for the same output text.
    ///
    /// Ordinary sentence decoding keeps one best path per text. Context
    /// experiments sometimes need to compare different word boundaries that
    /// produce the same text, so this method retains at most
    /// `segmentations_per_text` paths for each text while still admitting at
    /// most `top_k` distinct texts per conservative safety lane. It does not
    /// enable a language model or change the ordinary decoder.
    pub fn decode_sentence_segmentation_variants(
        &self,
        observed: &str,
        top_k: usize,
        segmentations_per_text: usize,
    ) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
        let (mut candidates, _stats) = self.decode_sentence_variant_frontier_with_stats(
            observed,
            top_k,
            segmentations_per_text.max(1),
        )?;
        candidates.sort_by(sentence_order);
        Ok(candidates)
    }

    /// Decodes continuous input into a stable primary lane plus a small,
    /// explicitly inspectable transposition-recovery lane.
    ///
    /// This is an interaction-oriented view, not a second decoder. It widens
    /// the internal search frontier enough to keep recovery evidence,
    /// while the ordinary `decode_sentence` result remains unchanged.
    pub fn decode_sentence_lanes(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<SentenceCandidateLanes, KeySequenceError> {
        if top_k == 0 {
            // Validate the input even when the caller requests no candidates,
            // matching `decode_sentence`.
            KeySequence::new(observed)?;
            return Ok(SentenceCandidateLanes {
                primary: Vec::new(),
                anchored_transposition_recovery: Vec::new(),
            });
        }

        const MINIMUM_RECOVERY_FRONTIER: usize = 20;
        let search_k = top_k.saturating_mul(2).max(MINIMUM_RECOVERY_FRONTIER);
        let (mut frontier, _stats) =
            self.decode_sentence_frontier_with_stats(observed, search_k)?;
        frontier.sort_by(sentence_order);

        let mut primary = frontier.clone();
        primary.truncate(top_k);
        let primary_text = primary
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<HashSet<_>>();
        let anchored_transposition_recovery = frontier
            .into_iter()
            .filter(sentence_is_anchored_transposition_recovery)
            .filter(|candidate| !primary_text.contains(candidate.text.as_str()))
            .take(top_k)
            .collect();

        Ok(SentenceCandidateLanes {
            primary,
            anchored_transposition_recovery,
        })
    }

    fn decode_sentence_frontier_with_stats(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<(Vec<SentenceCandidate>, SentenceSearchStats), KeySequenceError> {
        self.decode_sentence_variant_frontier_with_stats(observed, top_k, 1)
    }

    fn decode_sentence_variant_frontier_with_stats(
        &self,
        observed: &str,
        top_k: usize,
        segmentations_per_text: usize,
    ) -> Result<(Vec<SentenceCandidate>, SentenceSearchStats), KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        let mut search_stats = SentenceSearchStats::default();
        if top_k == 0 {
            return Ok((Vec::new(), search_stats));
        }

        let frequency_total = self
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = if frequency_total > 0.0 {
            frequency_total.ln()
        } else {
            0.0
        };
        let lattice = self.build_sentence_lattice(
            observed.as_str(),
            Some(top_k),
            log_frequency_total,
            &mut search_stats,
        );
        let initial_state = SentenceRankingState {
            position: 0,
            used_error: false,
            previous_word: None,
        };
        let ranking_config = SentenceRankingConfig {
            top_k,
            segmentations_per_text,
            log_frequency_total,
        };
        let mut memo = HashMap::new();
        let candidates = self.k_best_from_state(
            &lattice,
            initial_state,
            ranking_config,
            &mut memo,
            &mut search_stats,
        );
        Ok((candidates, search_stats))
    }

    fn build_complete_sentence_lattice(
        &self,
        observed: &str,
        search_stats: &mut SentenceSearchStats,
    ) -> SentenceLattice {
        let length = observed.len();
        let mut outgoing = vec![Vec::new(); length + 1];
        let mut reachable = vec![false; length + 1];
        reachable[0] = true;

        for start in (0..length).step_by(2) {
            if !reachable[start] {
                continue;
            }
            let transitions =
                self.complete_segment_transitions(observed, start, false, search_stats);
            for transition in &transitions {
                reachable[transition.end] = true;
            }
            search_stats.lattice_transitions_retained += transitions.len();
            outgoing[start] = transitions;
        }

        SentenceLattice { outgoing, length }
    }

    fn build_complete_sentence_lattice_with_final_initial(
        &self,
        observed: &str,
        search_stats: &mut SentenceSearchStats,
    ) -> SentenceLattice {
        debug_assert!(!observed.len().is_multiple_of(2));
        let length = observed.len();
        let mut outgoing = vec![Vec::new(); length + 1];
        let mut reachable = vec![false; length + 1];
        reachable[0] = true;

        for start in (0..length).step_by(2) {
            if !reachable[start] {
                continue;
            }
            let transitions =
                self.complete_segment_transitions(observed, start, true, search_stats);
            for transition in &transitions {
                reachable[transition.end] = true;
            }
            search_stats.lattice_transitions_retained += transitions.len();
            outgoing[start] = transitions;
        }

        SentenceLattice { outgoing, length }
    }

    fn complete_segment_transitions(
        &self,
        observed: &str,
        start: usize,
        allow_final_initial: bool,
        search_stats: &mut SentenceSearchStats,
    ) -> Vec<SegmentTransition> {
        search_stats.segment_trie_scans += 1;
        let mut nodes = vec![0_usize];
        let mut transitions = SegmentTransitionAccumulator::default();
        let remaining = &observed.as_bytes()[start..];
        let paired_length = remaining.len() / 2 * 2;
        let mut exact_prefix_complete = true;

        for (syllable_offset, chunk) in remaining[..paired_length].chunks_exact(2).enumerate() {
            let exact = [chunk[0], chunk[1]];
            let umlaut_alias = (chunk[1] == b'u' && matches!(chunk[0], b'j' | b'q' | b'x' | b'y'))
                .then_some([chunk[0], b'v']);
            let mut next = Vec::new();
            for &node in &nodes {
                search_stats.trie_path_visits += 1;
                if let Some(edge) = self.trie.nodes[node]
                    .children
                    .iter()
                    .find(|edge| edge.code == exact)
                {
                    next.push(edge.child);
                }
                if let Some(alias) = umlaut_alias.as_ref()
                    && let Some(edge) = self.trie.nodes[node]
                        .children
                        .iter()
                        .find(|edge| edge.code == *alias)
                {
                    next.push(edge.child);
                }
            }
            next.sort_unstable();
            next.dedup();
            if next.is_empty() {
                exact_prefix_complete = false;
                break;
            }
            nodes = next;

            let end = start + (syllable_offset + 1) * 2;
            let observed_segment = &observed[start..end];
            let spelling = Spelling {
                code: KeySequence::new(observed_segment)
                    .expect("a sentence slice is lowercase ASCII"),
                abbreviated_syllables: Vec::new(),
            };
            for &node in &nodes {
                for &entry_index in &self.trie.nodes[node].terminals {
                    search_stats.terminal_spelling_matches += 1;
                    let candidate = self.make_candidate(
                        &self.lexicon[entry_index],
                        spelling.clone(),
                        Correction::Exact,
                    );
                    transitions.upsert(SegmentTransition {
                        end,
                        uses_error: false,
                        observed: spelling.code.clone(),
                        candidate,
                    });
                }
            }
        }

        if allow_final_initial
            && paired_length < remaining.len()
            && exact_prefix_complete
            && !nodes.is_empty()
        {
            let initial = remaining[paired_length];
            let end = observed.len();
            let observed_segment = &observed[start..end];
            let spelling = Spelling {
                code: KeySequence::new(observed_segment)
                    .expect("a sentence slice is lowercase ASCII"),
                abbreviated_syllables: vec![paired_length / 2],
            };
            for &node in &nodes {
                search_stats.trie_path_visits += 1;
                for edge in self.trie.nodes[node]
                    .children
                    .iter()
                    .filter(|edge| edge.code[0] == initial)
                {
                    for &entry_index in &self.trie.nodes[edge.child].terminals {
                        search_stats.terminal_spelling_matches += 1;
                        let candidate = self.make_candidate(
                            &self.lexicon[entry_index],
                            spelling.clone(),
                            Correction::Exact,
                        );
                        transitions.upsert(SegmentTransition {
                            end,
                            uses_error: false,
                            observed: spelling.code.clone(),
                            candidate,
                        });
                    }
                }
            }
        }

        let mut transitions = transitions.into_transitions();
        transitions.sort_by(segment_transition_order);
        search_stats.lattice_transitions += transitions.len();
        search_stats.lattice_transitions_materialized += transitions.len();
        transitions
    }

    fn build_sentence_lattice(
        &self,
        observed: &str,
        top_k: Option<usize>,
        log_frequency_total: f64,
        search_stats: &mut SentenceSearchStats,
    ) -> SentenceLattice {
        let length = observed.len();
        let mut outgoing = vec![Vec::new(); length + 1];
        let mut exact_reachable = vec![false; length + 1];
        let mut error_reachable = vec![false; length + 1];
        exact_reachable[0] = true;

        for start in 0..length {
            if !exact_reachable[start] && !error_reachable[start] {
                continue;
            }
            let mut transitions = self.segment_transitions(
                observed,
                start,
                exact_reachable[start],
                error_reachable[start],
                top_k,
                search_stats,
            );
            transitions.push(self.unresolved_transition(observed, start));
            transitions.sort_by(segment_transition_order);
            search_stats.lattice_transitions += 1;
            search_stats.unresolved_lattice_transitions += 1;
            search_stats.lattice_transitions_materialized += 1;

            if self.language_model.is_none()
                && let Some(top_k) = top_k
            {
                transitions = self.compact_unigram_lattice_transitions(
                    transitions,
                    start,
                    exact_reachable[start],
                    error_reachable[start],
                    top_k,
                    log_frequency_total,
                );
            }
            search_stats.lattice_transitions_retained += transitions.len();
            search_stats.unresolved_lattice_transitions_retained += transitions
                .iter()
                .filter(|transition| {
                    transition.candidate.source == CandidateSource::UnresolvedInput
                })
                .count();

            if exact_reachable[start] {
                for transition in collapse_error_layers(&transitions) {
                    if transition.uses_error {
                        error_reachable[transition.end] = true;
                    } else {
                        exact_reachable[transition.end] = true;
                    }
                }
            }
            if error_reachable[start] {
                for transition in transitions
                    .iter()
                    .filter(|transition| !transition.uses_error)
                {
                    error_reachable[transition.end] = true;
                }
            }
            outgoing[start] = transitions;
        }

        SentenceLattice { outgoing, length }
    }

    fn compact_unigram_lattice_transitions(
        &self,
        transitions: Vec<SegmentTransition>,
        position: usize,
        exact_reachable: bool,
        error_reachable: bool,
        top_k: usize,
        log_frequency_total: f64,
    ) -> Vec<SegmentTransition> {
        debug_assert!(self.language_model.is_none());
        let mut retained = SegmentTransitionAccumulator::default();
        for used_error in [false, true] {
            if (used_error && !error_reachable) || (!used_error && !exact_reachable) {
                continue;
            }
            let state_transitions = if used_error {
                transitions
                    .iter()
                    .filter(|transition| !transition.uses_error)
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                collapse_error_layers(&transitions)
            };
            let state = SentenceRankingState {
                position,
                used_error,
                previous_word: None,
            };
            let mut ignored_stats = SentenceSearchStats::default();
            for group in self.prepare_transition_groups(
                state_transitions,
                &state,
                top_k,
                log_frequency_total,
                &mut ignored_stats,
            ) {
                for prepared in group.transitions {
                    retained.upsert(prepared.transition);
                }
            }
        }
        let mut retained = retained.into_transitions();
        retained.sort_by(segment_transition_order);
        retained
    }

    #[cfg(test)]
    fn build_unpruned_sentence_lattice(
        &self,
        observed: &str,
        search_stats: &mut SentenceSearchStats,
    ) -> SentenceLattice {
        self.build_sentence_lattice(observed, None, 0.0, search_stats)
    }

    fn k_best_from_state(
        &self,
        lattice: &SentenceLattice,
        state: SentenceRankingState,
        config: SentenceRankingConfig,
        memo: &mut HashMap<SentenceRankingState, Vec<SentenceCandidate>>,
        search_stats: &mut SentenceSearchStats,
    ) -> Vec<SentenceCandidate> {
        if let Some(cached) = memo.get(&state) {
            search_stats.ranking_state_cache_hits += 1;
            return cached.clone();
        }
        search_stats.ranking_states_evaluated += 1;

        if state.position == lattice.length {
            let terminal = vec![SentenceCandidate {
                text: String::new(),
                segments: Vec::new(),
                total_score: 0.0,
                unresolved_key_count: 0,
                used_error: state.used_error,
            }];
            memo.insert(state, terminal.clone());
            return terminal;
        }

        let transitions = if state.used_error {
            lattice.outgoing[state.position]
                .iter()
                .filter(|transition| !transition.uses_error)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            collapse_error_layers(&lattice.outgoing[state.position])
        };
        let groups = self.prepare_transition_groups(
            transitions,
            &state,
            config.top_k,
            config.log_frequency_total,
            search_stats,
        );
        let mut paths = Vec::new();
        for group in groups {
            let suffixes =
                self.k_best_from_state(lattice, group.child_state, config, memo, search_stats);
            search_stats.path_combinations_considered += suffixes.len() * group.transitions.len();

            for prepared in group.transitions {
                for suffix in &suffixes {
                    let mut text = prepared.transition.candidate.text.clone();
                    text.push_str(&suffix.text);
                    let mut segments = Vec::with_capacity(suffix.segments.len() + 1);
                    segments.push(SentenceSegment {
                        observed: prepared.transition.observed.clone(),
                        candidate: prepared.transition.candidate.clone(),
                        language_score: prepared.language_score,
                    });
                    segments.extend(suffix.segments.iter().cloned());
                    paths.push(SentenceCandidate {
                        text,
                        segments,
                        total_score: prepared.edge_score + suffix.total_score,
                        unresolved_key_count: prepared.unresolved_key_count
                            + suffix.unresolved_key_count,
                        used_error: suffix.used_error,
                    });
                }
            }
        }

        paths.sort_by(sentence_order);
        let mut seen_segmentations = HashSet::<(String, Vec<(String, KeySequence)>)>::new();
        paths.retain(|candidate| {
            let segmentation = candidate
                .segments
                .iter()
                .map(|segment| (segment.candidate.text.clone(), segment.observed.clone()))
                .collect::<Vec<_>>();
            seen_segmentations.insert((candidate.text.clone(), segmentation))
        });
        let mut variants_by_text = HashMap::<String, usize>::new();
        paths.retain(|candidate| {
            let variants = variants_by_text.entry(candidate.text.clone()).or_default();
            if *variants >= config.segmentations_per_text {
                false
            } else {
                *variants += 1;
                true
            }
        });
        let mut exact_texts = HashSet::new();
        let mut error_texts = HashSet::new();
        let mut anchored_transposition_texts = HashSet::new();
        paths.retain(|candidate| {
            let anchored_transposition = sentence_is_anchored_transposition_recovery(candidate);
            if anchored_transposition {
                retain_bounded_text(
                    &mut anchored_transposition_texts,
                    &candidate.text,
                    config.top_k,
                )
            } else if candidate.used_error {
                retain_bounded_text(&mut error_texts, &candidate.text, config.top_k)
            } else {
                retain_bounded_text(&mut exact_texts, &candidate.text, config.top_k)
            }
        });
        memo.insert(state, paths.clone());
        paths
    }

    /// Transitions with the same child state share every possible suffix.
    /// After duplicate prefix text is removed, an edge below the first K
    /// cannot enter the ordinary Top-K: the K better prefixes can all combine
    /// with that exact suffix. The anchored-transposition lane retains its own
    /// first K as a typed diagnostic superset of that exact bound.
    fn prepare_transition_groups(
        &self,
        transitions: Vec<SegmentTransition>,
        state: &SentenceRankingState,
        top_k: usize,
        log_frequency_total: f64,
        search_stats: &mut SentenceSearchStats,
    ) -> Vec<SentenceTransitionGroup> {
        search_stats.ranking_transitions_considered += transitions.len();
        let mut groups = Vec::<SentenceTransitionGroup>::new();
        let mut group_indices = HashMap::<SentenceRankingState, usize>::new();
        for transition in transitions {
            let child_state = SentenceRankingState {
                position: transition.end,
                used_error: state.used_error || transition.uses_error,
                previous_word: match (self.language_model.as_ref(), transition.candidate.source) {
                    (Some(_), CandidateSource::Lexicon) => Some(transition.candidate.text.clone()),
                    _ => None,
                },
            };
            let language_score = self.sentence_language_score(
                state.previous_word.as_deref(),
                &transition.candidate,
                log_frequency_total,
            );
            let edge_score = language_score.interpolated_log_probability
                - transition.candidate.score.abbreviation_penalty
                - transition.candidate.score.correction_penalty
                - transition.candidate.score.unresolved_input_penalty;
            let unresolved_key_count =
                usize::from(transition.candidate.source == CandidateSource::UnresolvedInput);
            let prepared = PreparedSentenceTransition {
                transition,
                language_score,
                edge_score,
                unresolved_key_count,
            };
            if let Some(&index) = group_indices.get(&child_state) {
                groups[index].transitions.push(prepared);
            } else {
                group_indices.insert(child_state.clone(), groups.len());
                groups.push(SentenceTransitionGroup {
                    child_state,
                    transitions: vec![prepared],
                });
            }
        }

        for group in &mut groups {
            group.transitions.sort_by(prepared_transition_order);
            let mut seen_text = HashSet::new();
            group
                .transitions
                .retain(|prepared| seen_text.insert(prepared.transition.candidate.text.clone()));
            let mut globally_retained = 0;
            let mut anchored_transpositions_retained = 0;
            group.transitions.retain(|prepared| {
                let anchored_transposition =
                    candidate_is_anchored_transposition_recovery(&prepared.transition.candidate);
                let retain_globally = globally_retained < top_k;
                let retain_for_recovery =
                    anchored_transposition && anchored_transpositions_retained < top_k;
                if retain_globally {
                    globally_retained += 1;
                }
                if retain_for_recovery {
                    anchored_transpositions_retained += 1;
                }
                retain_globally || retain_for_recovery
            });
            search_stats.ranking_transitions_retained += group.transitions.len();
        }
        groups
    }

    fn sentence_language_score(
        &self,
        previous_word: Option<&str>,
        candidate: &Candidate,
        log_frequency_total: f64,
    ) -> SentenceLanguageScore {
        if candidate.source == CandidateSource::UnresolvedInput {
            return SentenceLanguageScore {
                unigram_log_probability: 0.0,
                bigram: None,
                interpolated_log_probability: 0.0,
            };
        }
        let unigram_log_probability = candidate.score.frequency - log_frequency_total;
        let bigram = previous_word.and_then(|previous| {
            self.language_model
                .as_ref()
                .map(|model| model.score(previous, &candidate.text))
        });
        let interpolated_log_probability = bigram.map_or(unigram_log_probability, |bigram| {
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram_log_probability
                + BIGRAM_INTERPOLATION_WEIGHT * bigram.log_probability
        });
        SentenceLanguageScore {
            unigram_log_probability,
            bigram,
            interpolated_log_probability,
        }
    }

    fn segment_transitions(
        &self,
        observed: &str,
        start: usize,
        exact_reachable: bool,
        error_reachable: bool,
        top_k: Option<usize>,
        search_stats: &mut SentenceSearchStats,
    ) -> Vec<SegmentTransition> {
        search_stats.segment_trie_scans += 1;
        let mut word_stats = DecodeSearchStats::default();
        let pruning = if self.language_model.is_none()
            && let Some(top_k) = top_k
        {
            let mut exact_stats = DecodeSearchStats::default();
            let exact_matches =
                self.trie
                    .lookup_prefixes(&observed[start..], false, None, &mut exact_stats);
            search_stats.exact_prefix_prepass_visits += exact_stats.trie_path_visits;
            word_stats.trie_path_visits += exact_stats.trie_path_visits;
            word_stats.alignment_states_examined += exact_stats.alignment_states_examined;
            word_stats.alignment_states_reused += exact_stats.alignment_states_reused;
            let exact_evidence = ExactPrefixEvidence::new(
                &exact_matches.paths,
                &self.trie,
                &self.lexicon,
                &self.entry_identity_ids,
                &self.entry_identity_max_frequencies,
                &self.config,
                top_k,
                observed.len() - start,
            );
            search_stats.exact_prefix_prepass_entry_visits += exact_evidence.entry_visits;
            Some(UnigramPrefixPruningConfig {
                lexicon: &self.lexicon,
                entry_identity_ids: &self.entry_identity_ids,
                config: &self.config,
                top_k,
                exact_reachable,
                error_reachable,
                exact_evidence,
            })
        } else {
            None
        };
        let prefix_matches = self.trie.lookup_prefixes(
            &observed[start..],
            exact_reachable,
            pruning,
            &mut word_stats,
        );
        search_stats.trie_path_visits += word_stats.trie_path_visits;
        search_stats.trie_subtree_prunes += word_stats.trie_subtree_prunes;
        search_stats.alignment_states_examined += word_stats.alignment_states_examined;
        search_stats.alignment_states_reused += word_stats.alignment_states_reused;
        search_stats.terminal_path_matches += prefix_matches.terminal_path_matches;
        let terminal_entry_bounds = prefix_matches.terminal_entry_bounds;
        let paths = prefix_matches.paths;

        let mut matches_by_identity = NoisyTerminalAccumulator::default();
        for (path_index, path) in paths.iter().enumerate() {
            let terminals = &self.trie.nodes[path.node_index].terminals;
            for (terminal_offset, &entry_index) in terminals.iter().enumerate() {
                let total_score =
                    noisy_terminal_total_score(&self.lexicon[entry_index], path, &self.config);
                if terminal_entry_bounds
                    .as_ref()
                    .is_some_and(|bounds| bounds.score_is_dominated(path, total_score))
                {
                    search_stats.terminal_entry_bound_skips += terminals.len() - terminal_offset;
                    break;
                }
                word_stats.terminal_spelling_matches += 1;
                matches_by_identity.upsert(
                    IndexedNoisyTerminal {
                        path_index,
                        entry_index,
                        entry_identity: self.entry_identity_ids[entry_index],
                        total_score,
                    },
                    &paths,
                    &self.lexicon,
                );
            }
        }
        search_stats.terminal_spelling_matches += word_stats.terminal_spelling_matches;
        let matches = matches_by_identity.into_matches();
        search_stats.lattice_transitions += matches.len();
        let matches = if self.language_model.is_none()
            && let Some(top_k) = top_k
        {
            self.compact_unigram_terminal_matches(
                matches,
                &paths,
                exact_reachable,
                error_reachable,
                top_k,
            )
        } else {
            matches
        };
        search_stats.lattice_transitions_materialized += matches.len();

        let mut transitions = Vec::with_capacity(matches.len());
        for indexed_terminal in matches {
            let path = &paths[indexed_terminal.path_index];
            let end = start + path.observed_length;
            let observed_segment = &observed[start..end];
            let entry = &self.lexicon[indexed_terminal.entry_index];
            let candidate =
                self.make_candidate(entry, path.spelling.clone(), path.correction.clone());
            let transition = SegmentTransition {
                end,
                uses_error: candidate.correction != Correction::Exact,
                observed: KeySequence::new(observed_segment)
                    .expect("a sentence slice is lowercase ASCII"),
                candidate,
            };
            transitions.push(transition);
        }
        transitions.sort_by(segment_transition_order);
        transitions
    }

    fn compact_unigram_terminal_matches(
        &self,
        matches: Vec<IndexedNoisyTerminal>,
        paths: &[NoisyTrieTerminalPath],
        exact_reachable: bool,
        error_reachable: bool,
        top_k: usize,
    ) -> Vec<IndexedNoisyTerminal> {
        debug_assert!(self.language_model.is_none());
        let mut retained = NoisyTerminalAccumulator::default();
        if exact_reachable {
            for terminal in retain_noisy_top_k_by_child(
                collapse_noisy_error_layers(&matches, paths, &self.lexicon),
                false,
                top_k,
                paths,
                &self.lexicon,
            ) {
                retained.upsert(terminal, paths, &self.lexicon);
            }
        }
        if error_reachable {
            let exact_matches = matches
                .iter()
                .filter(|terminal| !terminal.uses_error(paths))
                .copied()
                .collect::<Vec<_>>();
            for terminal in
                retain_noisy_top_k_by_child(exact_matches, true, top_k, paths, &self.lexicon)
            {
                retained.upsert(terminal, paths, &self.lexicon);
            }
        }
        let mut retained = retained.into_matches();
        retained
            .sort_by(|left, right| noisy_terminal_segment_order(left, right, paths, &self.lexicon));
        retained
    }

    fn unresolved_transition(&self, observed: &str, start: usize) -> SegmentTransition {
        let observed_key = &observed[start..start + 1];
        let key_sequence =
            KeySequence::new(observed_key).expect("sentence input is validated lowercase ASCII");
        let spelling = Spelling {
            code: key_sequence.clone(),
            abbreviated_syllables: Vec::new(),
        };
        SegmentTransition {
            end: start + 1,
            uses_error: false,
            observed: key_sequence.clone(),
            candidate: Candidate {
                source: CandidateSource::UnresolvedInput,
                text: format!("〔{observed_key}〕"),
                pinyin: String::new(),
                code: key_sequence,
                spelling,
                correction: Correction::Exact,
                score: ScoreBreakdown {
                    frequency: 0.0,
                    correction_penalty: 0.0,
                    abbreviation_penalty: 0.0,
                    unresolved_input_penalty: self.config.unresolved_key_penalty,
                    total: -self.config.unresolved_key_penalty,
                },
            },
        }
    }

    #[cfg(test)]
    fn segment_transitions_by_slices(
        &self,
        observed: &str,
        start: usize,
        allow_error: bool,
    ) -> Vec<SegmentTransition> {
        let mut transitions = SegmentTransitionAccumulator::default();
        let maximum_length = self.trie.maximum_code_length + usize::from(allow_error);
        let remaining = observed.len() - start;
        for observed_length in 1..=maximum_length.min(remaining) {
            let end = start + observed_length;
            let observed_segment = &observed[start..end];
            for candidate in self.lookup_candidates(observed_segment, allow_error) {
                let transition = SegmentTransition {
                    end,
                    uses_error: candidate.correction != Correction::Exact,
                    observed: KeySequence::new(observed_segment)
                        .expect("a sentence slice is lowercase ASCII"),
                    candidate,
                };
                transitions.upsert(transition);
            }
        }
        let mut transitions = transitions.into_transitions();
        transitions.sort_by(segment_transition_order);
        transitions
    }

    #[cfg(test)]
    fn lookup_candidates(&self, observed: &str, allow_error: bool) -> Vec<Candidate> {
        self.lookup_candidates_with_stats(observed, allow_error, &mut DecodeSearchStats::default())
    }

    fn lookup_candidates_with_stats(
        &self,
        observed: &str,
        allow_error: bool,
        stats: &mut DecodeSearchStats,
    ) -> Vec<Candidate> {
        let mut best_by_entry = HashMap::<usize, Candidate>::new();
        for terminal in self.trie.lookup_noisy(observed, allow_error, stats) {
            let entry = &self.lexicon[terminal.entry_index];
            let candidate = self.make_candidate(entry, terminal.spelling, terminal.correction);
            match best_by_entry.entry(terminal.entry_index) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(candidate);
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if candidate_order(&candidate, slot.get()) == Ordering::Less {
                        slot.insert(candidate);
                    }
                }
            }
        }
        let mut candidates = best_by_entry.into_values().collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates
    }

    fn make_candidate(
        &self,
        entry: &LexiconEntry,
        spelling: Spelling,
        correction: Correction,
    ) -> Candidate {
        let frequency = (entry.frequency as f64).ln();
        let correction_penalty = self.correction_penalty(&correction);
        let abbreviation_penalty = spelling.abbreviated_syllables.len() as f64
            * self.config.abbreviation_penalty_per_syllable;
        Candidate {
            source: CandidateSource::Lexicon,
            text: entry.text.clone(),
            pinyin: entry.pinyin.clone(),
            code: entry.code.clone(),
            spelling,
            correction,
            score: ScoreBreakdown {
                frequency,
                correction_penalty,
                abbreviation_penalty,
                unresolved_input_penalty: 0.0,
                total: frequency - correction_penalty - abbreviation_penalty,
            },
        }
    }

    fn correction_penalty(&self, correction: &Correction) -> f64 {
        configured_correction_penalty(&self.config, correction)
    }
}

#[derive(Clone, Debug)]
struct SyllableTrie {
    nodes: Vec<SyllableTrieNode>,
    maximum_code_length: usize,
    represented_spelling_count: usize,
    maximum_syllables: usize,
}

impl SyllableTrie {
    fn new(lexicon: &[LexiconEntry]) -> Self {
        let mut trie = Self {
            nodes: vec![SyllableTrieNode::default()],
            maximum_code_length: 0,
            represented_spelling_count: 0,
            maximum_syllables: 0,
        };
        for (entry_index, entry) in lexicon.iter().enumerate() {
            trie.insert(entry_index, &entry.syllable_codes);
        }
        trie.finish_subtree_metadata(lexicon);
        trie
    }

    fn finish_subtree_metadata(&mut self, lexicon: &[LexiconEntry]) {
        for node_index in (0..self.nodes.len()).rev() {
            let mut maximum_frequency = self.nodes[node_index]
                .terminals
                .iter()
                .map(|&entry_index| lexicon[entry_index].frequency)
                .max()
                .unwrap_or(0);
            let mut minimum_terminal_syllables = if self.nodes[node_index].terminals.is_empty() {
                usize::MAX
            } else {
                0
            };
            let mut maximum_terminal_syllables = 0;

            for edge in &self.nodes[node_index].children {
                debug_assert!(
                    edge.child > node_index,
                    "trie insertion gives every child a later topological index"
                );
                let child = &self.nodes[edge.child];
                debug_assert_ne!(child.minimum_terminal_syllables, usize::MAX);
                maximum_frequency = maximum_frequency.max(child.subtree_maximum_frequency);
                minimum_terminal_syllables = minimum_terminal_syllables
                    .min(child.minimum_terminal_syllables.saturating_add(1));
                maximum_terminal_syllables = maximum_terminal_syllables
                    .max(child.maximum_terminal_syllables.saturating_add(1));
            }

            let node = &mut self.nodes[node_index];
            node.subtree_maximum_frequency = maximum_frequency;
            node.minimum_terminal_syllables = minimum_terminal_syllables;
            node.maximum_terminal_syllables = maximum_terminal_syllables;
        }

        let maximum_frequencies = self
            .nodes
            .iter()
            .map(|node| node.subtree_maximum_frequency)
            .collect::<Vec<_>>();
        for node in &mut self.nodes {
            node.terminals.sort_unstable_by(|&left, &right| {
                lexicon[right]
                    .frequency
                    .cmp(&lexicon[left].frequency)
                    .then_with(|| left.cmp(&right))
            });
            node.children.sort_unstable_by(|left, right| {
                maximum_frequencies[right.child]
                    .cmp(&maximum_frequencies[left.child])
                    .then_with(|| left.code.cmp(&right.code))
            });
        }
    }

    fn insert(&mut self, entry_index: usize, syllable_codes: &[KeySequence]) {
        self.maximum_code_length = self
            .maximum_code_length
            .max(syllable_codes.len().saturating_mul(2));
        self.maximum_syllables = self.maximum_syllables.max(syllable_codes.len());
        let represented_spellings = 1usize
            .checked_shl(syllable_codes.len() as u32)
            .unwrap_or(usize::MAX);
        self.represented_spelling_count = self
            .represented_spelling_count
            .saturating_add(represented_spellings);

        let mut node_index = 0;
        for syllable_code in syllable_codes {
            let bytes = syllable_code.as_str().as_bytes();
            debug_assert_eq!(bytes.len(), 2, "canonical syllable codes have two keys");
            let code = [bytes[0], bytes[1]];
            let existing_child = self.nodes[node_index]
                .children
                .iter()
                .find(|edge| edge.code == code)
                .map(|edge| edge.child);
            node_index = match existing_child {
                Some(child) => child,
                None => {
                    let child = self.nodes.len();
                    self.nodes.push(SyllableTrieNode::default());
                    self.nodes[node_index]
                        .children
                        .push(SyllableTrieEdge { code, child });
                    child
                }
            };
        }
        self.nodes[node_index].terminals.push(entry_index);
    }

    fn lookup_noisy(
        &self,
        observed: &str,
        allow_error: bool,
        stats: &mut DecodeSearchStats,
    ) -> Vec<NoisyTrieTerminal> {
        let prefix_matches = self.lookup_prefixes(observed, allow_error, None, stats);
        let mut matches = Vec::new();
        for path in prefix_matches
            .paths
            .into_iter()
            .filter(|path| path.observed_length == observed.len())
        {
            for &entry_index in &self.nodes[path.node_index].terminals {
                matches.push(NoisyTrieTerminal {
                    entry_index,
                    spelling: path.spelling.clone(),
                    correction: path.correction.clone(),
                });
            }
        }
        stats.terminal_spelling_matches += matches.len();
        matches
    }

    fn lookup_prefixes(
        &self,
        observed: &str,
        allow_error: bool,
        pruning: Option<UnigramPrefixPruningConfig<'_>>,
        stats: &mut DecodeSearchStats,
    ) -> TriePrefixMatches {
        let mut search = TrieSearch {
            observed: observed.as_bytes(),
            allow_error,
            intended: String::with_capacity(self.maximum_code_length),
            abbreviated_syllables: Vec::with_capacity(self.maximum_syllables),
            paths: Vec::new(),
            terminal_path_matches: 0,
            alignment_state_ids: HashMap::new(),
            alignment_state_sets: Vec::new(),
            alignment_transitions: Vec::new(),
            pruning: pruning.map(|config| UnigramPrefixPruning::new(config, observed.len())),
            stats,
        };
        let initial_state = AlignmentState {
            observed_position: 0,
            used_error: false,
            transposition_pending: false,
        };
        let initial_states =
            search.intern_alignment_states(AlignmentStates::singleton(initial_state));
        self.collect_noisy_matches(0, 0, initial_states, &mut search);
        let terminal_entry_bounds = search
            .pruning
            .take()
            .map(UnigramPrefixPruning::into_terminal_entry_bounds);
        TriePrefixMatches {
            paths: search.paths,
            terminal_path_matches: search.terminal_path_matches,
            terminal_entry_bounds,
        }
    }

    fn collect_noisy_matches(
        &self,
        node_index: usize,
        syllable_index: usize,
        states: usize,
        search: &mut TrieSearch<'_>,
    ) {
        search.stats.trie_path_visits += 1;
        let node = &self.nodes[node_index];
        if !node.terminals.is_empty() {
            let observed_lengths = search.terminal_observed_lengths(states);
            for &observed_length in observed_lengths.as_slice() {
                let observed_prefix = std::str::from_utf8(&search.observed[..observed_length])
                    .expect("decoder inputs are validated lowercase ASCII");
                if let Some(correction) = detect_correction(observed_prefix, &search.intended)
                    && (search.allow_error || correction == Correction::Exact)
                {
                    search.observe_terminal(node, observed_length, &correction);
                    search.terminal_path_matches += 1;
                    search.paths.push(NoisyTrieTerminalPath {
                        node_index,
                        observed_length,
                        spelling: Spelling {
                            code: KeySequence::new(search.intended.clone())
                                .expect("trie edges contain lowercase ASCII"),
                            abbreviated_syllables: search.abbreviated_syllables.clone(),
                        },
                        correction,
                    });
                }
            }
        }

        for edge in &node.children {
            search.intended.push(edge.code[0] as char);
            search.intended.push(edge.code[1] as char);
            let full_states = search.advance(states, &edge.code);
            if !search.alignment_state_sets[full_states].is_empty() {
                if search.subtree_is_dominated(&self.nodes[edge.child], full_states) {
                    search.stats.trie_subtree_prunes += 1;
                } else {
                    self.collect_noisy_matches(edge.child, syllable_index + 1, full_states, search);
                }
            }
            search.intended.truncate(search.intended.len() - 2);

            search.intended.push(edge.code[0] as char);
            search.abbreviated_syllables.push(syllable_index);
            let abbreviated_states = search.advance(states, &edge.code[..1]);
            if !search.alignment_state_sets[abbreviated_states].is_empty() {
                if search.subtree_is_dominated(&self.nodes[edge.child], abbreviated_states) {
                    search.stats.trie_subtree_prunes += 1;
                } else {
                    self.collect_noisy_matches(
                        edge.child,
                        syllable_index + 1,
                        abbreviated_states,
                        search,
                    );
                }
            }
            search.abbreviated_syllables.pop();
            search.intended.pop();
        }
    }

    #[cfg(test)]
    fn lookup(&self, code: &str) -> Vec<TrieTerminal> {
        let Ok(original_code) = KeySequence::new(code) else {
            return Vec::new();
        };
        let mut matches = Vec::new();
        self.collect_matches(
            0,
            original_code.as_str().as_bytes(),
            0,
            0,
            &mut Vec::new(),
            &mut matches,
        );
        matches
            .into_iter()
            .map(|(entry_index, abbreviated_syllables)| TrieTerminal {
                entry_index,
                spelling: Spelling {
                    code: original_code.clone(),
                    abbreviated_syllables,
                },
            })
            .collect()
    }

    #[cfg(test)]
    fn collect_matches(
        &self,
        node_index: usize,
        input: &[u8],
        position: usize,
        syllable_index: usize,
        abbreviated_syllables: &mut Vec<usize>,
        matches: &mut Vec<(usize, Vec<usize>)>,
    ) {
        let node = &self.nodes[node_index];
        if position == input.len() {
            for &entry_index in &node.terminals {
                matches.push((entry_index, abbreviated_syllables.clone()));
            }
            return;
        }

        for edge in &node.children {
            if position + 1 < input.len()
                && input[position] == edge.code[0]
                && input[position + 1] == edge.code[1]
            {
                self.collect_matches(
                    edge.child,
                    input,
                    position + 2,
                    syllable_index + 1,
                    abbreviated_syllables,
                    matches,
                );
            }
            if input[position] == edge.code[0] {
                abbreviated_syllables.push(syllable_index);
                self.collect_matches(
                    edge.child,
                    input,
                    position + 1,
                    syllable_index + 1,
                    abbreviated_syllables,
                    matches,
                );
                abbreviated_syllables.pop();
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SyllableTrieNode {
    children: Vec<SyllableTrieEdge>,
    terminals: Vec<usize>,
    subtree_maximum_frequency: u64,
    minimum_terminal_syllables: usize,
    maximum_terminal_syllables: usize,
}

#[derive(Clone, Copy, Debug)]
struct SyllableTrieEdge {
    code: [u8; 2],
    child: usize,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct TrieTerminal {
    entry_index: usize,
    spelling: Spelling,
}

#[derive(Clone, Debug)]
struct NoisyTrieTerminal {
    entry_index: usize,
    spelling: Spelling,
    correction: Correction,
}

#[derive(Clone, Debug)]
struct NoisyTrieTerminalPath {
    node_index: usize,
    observed_length: usize,
    spelling: Spelling,
    correction: Correction,
}

struct TriePrefixMatches {
    paths: Vec<NoisyTrieTerminalPath>,
    terminal_path_matches: usize,
    terminal_entry_bounds: Option<TerminalEntryBounds>,
}

struct TerminalEntryBounds {
    exact_reachable: bool,
    error_reachable: bool,
    unused_exact_cutoffs: Vec<Option<f64>>,
    unused_error_cutoffs: Vec<Option<f64>>,
    used_error_exact_cutoffs: Vec<Option<f64>>,
}

impl TerminalEntryBounds {
    fn score_is_dominated(&self, path: &NoisyTrieTerminalPath, score: f64) -> bool {
        let end = path.observed_length;
        // An entry can either enter its own error layer or displace the same
        // canonical identity from the other layer. Requiring strict
        // domination in both unused-error frontiers preserves both effects.
        if self.exact_reachable
            && (!score_is_strictly_below(self.unused_exact_cutoffs[end], score)
                || !score_is_strictly_below(self.unused_error_cutoffs[end], score))
        {
            return false;
        }
        if path.correction == Correction::Exact
            && self.error_reachable
            && !score_is_strictly_below(self.used_error_exact_cutoffs[end], score)
        {
            return false;
        }
        true
    }
}

fn score_is_strictly_below(cutoff: Option<f64>, score: f64) -> bool {
    cutoff.is_some_and(|cutoff| score.total_cmp(&cutoff) == Ordering::Less)
}

struct UnigramPrefixPruningConfig<'a> {
    lexicon: &'a [LexiconEntry],
    entry_identity_ids: &'a [usize],
    config: &'a DecodeConfig,
    top_k: usize,
    exact_reachable: bool,
    error_reachable: bool,
    exact_evidence: ExactPrefixEvidence<'a>,
}

/// Exact cutoffs are kept separately for the three lattice roles that share
/// one trie scan: exact edges from an unused-error prefix, corrected edges
/// from that prefix, and exact edges from an already-used-error prefix.
/// A complete exact-only prepass certifies cross-layer identity winners.
struct UnigramPrefixPruning<'a> {
    lexicon: &'a [LexiconEntry],
    entry_identity_ids: &'a [usize],
    config: &'a DecodeConfig,
    top_k: usize,
    exact_reachable: bool,
    error_reachable: bool,
    exact_evidence: ExactPrefixEvidence<'a>,
    unused_error_frontiers: Vec<StableTextFrontier<'a>>,
}

impl<'a> UnigramPrefixPruning<'a> {
    fn new(config: UnigramPrefixPruningConfig<'a>, observed_length: usize) -> Self {
        Self {
            lexicon: config.lexicon,
            entry_identity_ids: config.entry_identity_ids,
            config: config.config,
            top_k: config.top_k,
            exact_reachable: config.exact_reachable,
            error_reachable: config.error_reachable,
            exact_evidence: config.exact_evidence,
            unused_error_frontiers: (0..=observed_length)
                .map(|_| StableTextFrontier::default())
                .collect(),
        }
    }

    fn observe_terminal(
        &mut self,
        node: &SyllableTrieNode,
        observed_length: usize,
        correction: &Correction,
        abbreviation_count: usize,
    ) {
        if *correction == Correction::Exact {
            return;
        }
        for &entry_index in &node.terminals {
            let entry = &self.lexicon[entry_index];
            let score = (entry.frequency as f64).ln()
                - configured_correction_penalty(self.config, correction)
                - abbreviation_count as f64 * self.config.abbreviation_penalty_per_syllable;
            if self.error_score_is_dominated(observed_length, score) {
                break;
            }
            if self.exact_reachable
                && self.error_candidate_is_stable(
                    entry_index,
                    observed_length,
                    score,
                    abbreviation_count,
                )
            {
                self.unused_error_frontiers[observed_length].insert(
                    entry.text.as_str(),
                    score,
                    self.top_k,
                );
            }
        }
    }

    fn error_score_is_dominated(&self, end: usize, score: f64) -> bool {
        self.exact_reachable
            && score_is_strictly_below(
                self.exact_evidence.unused_exact_frontiers[end].cutoff(self.top_k),
                score,
            )
            && score_is_strictly_below(self.unused_error_frontiers[end].cutoff(self.top_k), score)
    }

    fn error_candidate_is_stable(
        &self,
        entry_index: usize,
        observed_length: usize,
        score: f64,
        abbreviation_count: usize,
    ) -> bool {
        let identity = self.entry_identity_ids[entry_index];
        let Some(exact) = self
            .exact_evidence
            .best_by_identity_and_end
            .get(&(identity, observed_length))
        else {
            return true;
        };
        match score.total_cmp(&exact.score) {
            Ordering::Greater => true,
            Ordering::Equal => abbreviation_count < exact.abbreviation_count,
            Ordering::Less => false,
        }
    }

    fn subtree_is_dominated(
        &self,
        node: &SyllableTrieNode,
        states: &AlignmentStates,
        abbreviation_count: usize,
        observed_length: usize,
    ) -> bool {
        // Every future correction and abbreviation cost is non-negative, so
        // the subtree's largest log frequency minus costs already paid is an
        // optimistic score for every descendant. Equality is never pruned.
        let upper_score = (node.subtree_maximum_frequency as f64).ln()
            - abbreviation_count as f64 * self.config.abbreviation_penalty_per_syllable;
        let minimum_remaining_keys = node.minimum_terminal_syllables;
        let maximum_remaining_keys = node.maximum_terminal_syllables.saturating_mul(2);
        let mut has_potential_terminal = false;

        for state in states.as_slice() {
            for remaining_keys in minimum_remaining_keys..=maximum_remaining_keys {
                if state.transposition_pending {
                    if self.exact_reachable && remaining_keys > 0 {
                        has_potential_terminal = true;
                        if self.error_candidate_can_survive(
                            state
                                .observed_position
                                .saturating_add(remaining_keys)
                                .saturating_add(1),
                            upper_score,
                            observed_length,
                        ) {
                            return false;
                        }
                    }
                    continue;
                }

                if state.used_error {
                    if self.exact_reachable {
                        has_potential_terminal = true;
                        if self.error_candidate_can_survive(
                            state.observed_position.saturating_add(remaining_keys),
                            upper_score,
                            observed_length,
                        ) {
                            return false;
                        }
                    }
                    continue;
                }

                let exact_end = state.observed_position.saturating_add(remaining_keys);
                if self.exact_reachable || self.error_reachable {
                    has_potential_terminal = true;
                    if self.exact_candidate_can_survive(exact_end, upper_score, observed_length) {
                        return false;
                    }
                }
                if self.exact_reachable {
                    if remaining_keys > 0
                        && (self.error_candidate_can_survive(
                            exact_end.saturating_sub(1),
                            upper_score,
                            observed_length,
                        ) || self.error_candidate_can_survive(
                            exact_end,
                            upper_score,
                            observed_length,
                        ))
                    {
                        return false;
                    }
                    has_potential_terminal = true;
                    if self.error_candidate_can_survive(
                        exact_end.saturating_add(1),
                        upper_score,
                        observed_length,
                    ) {
                        return false;
                    }
                }
            }
        }

        has_potential_terminal
    }

    fn exact_candidate_can_survive(
        &self,
        end: usize,
        upper_score: f64,
        observed_length: usize,
    ) -> bool {
        if end == 0 || end > observed_length {
            return false;
        }
        if self.exact_reachable
            && self.exact_evidence.unused_exact_frontiers[end]
                .cutoff(self.top_k)
                .is_none_or(|cutoff| upper_score.total_cmp(&cutoff) != Ordering::Less)
        {
            return true;
        }
        self.error_reachable
            && self.exact_evidence.used_error_exact_frontiers[end]
                .cutoff(self.top_k)
                .is_none_or(|cutoff| upper_score.total_cmp(&cutoff) != Ordering::Less)
    }

    fn error_candidate_can_survive(
        &self,
        end: usize,
        upper_score: f64,
        observed_length: usize,
    ) -> bool {
        if end == 0 || end > observed_length {
            return false;
        }
        self.unused_error_frontiers[end]
            .cutoff(self.top_k)
            .is_none_or(|cutoff| upper_score.total_cmp(&cutoff) != Ordering::Less)
    }

    fn into_terminal_entry_bounds(self) -> TerminalEntryBounds {
        TerminalEntryBounds {
            exact_reachable: self.exact_reachable,
            error_reachable: self.error_reachable,
            unused_exact_cutoffs: stable_frontier_cutoffs(
                self.exact_evidence.unused_exact_frontiers,
                self.top_k,
            ),
            unused_error_cutoffs: stable_frontier_cutoffs(self.unused_error_frontiers, self.top_k),
            used_error_exact_cutoffs: stable_frontier_cutoffs(
                self.exact_evidence.used_error_exact_frontiers,
                self.top_k,
            ),
        }
    }
}

fn stable_frontier_cutoffs(
    frontiers: Vec<StableTextFrontier<'_>>,
    top_k: usize,
) -> Vec<Option<f64>> {
    frontiers
        .into_iter()
        .map(|frontier| frontier.cutoff(top_k))
        .collect()
}

struct ExactPrefixEvidence<'a> {
    best_by_identity_and_end: HashMap<(usize, usize), ExactCandidateRank>,
    unused_exact_frontiers: Vec<StableTextFrontier<'a>>,
    used_error_exact_frontiers: Vec<StableTextFrontier<'a>>,
    entry_visits: usize,
}

impl<'a> ExactPrefixEvidence<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        paths: &[NoisyTrieTerminalPath],
        trie: &SyllableTrie,
        lexicon: &'a [LexiconEntry],
        entry_identity_ids: &[usize],
        entry_identity_max_frequencies: &[u64],
        config: &DecodeConfig,
        top_k: usize,
        observed_length: usize,
    ) -> Self {
        let minimum_correction_penalty = [
            config.neighbor_substitution_penalty,
            config.adjacent_transposition_penalty,
            config.missing_key_penalty,
            config.extra_key_penalty,
        ]
        .into_iter()
        .min_by(f64::total_cmp)
        .expect("the error channel has correction operations");
        let mut evidence = Self {
            best_by_identity_and_end: HashMap::new(),
            unused_exact_frontiers: (0..=observed_length)
                .map(|_| StableTextFrontier::default())
                .collect(),
            used_error_exact_frontiers: (0..=observed_length)
                .map(|_| StableTextFrontier::default())
                .collect(),
            entry_visits: 0,
        };

        for path in paths {
            debug_assert_eq!(path.correction, Correction::Exact);
            let abbreviation_count = path.spelling.abbreviated_syllables.len();
            for &entry_index in &trie.nodes[path.node_index].terminals {
                evidence.entry_visits += 1;
                let entry = &lexicon[entry_index];
                let score = (entry.frequency as f64).ln()
                    - abbreviation_count as f64 * config.abbreviation_penalty_per_syllable;
                let rank = ExactCandidateRank {
                    score,
                    abbreviation_count,
                };
                evidence
                    .best_by_identity_and_end
                    .entry((entry_identity_ids[entry_index], path.observed_length))
                    .and_modify(|current| {
                        if rank.is_better_than(current) {
                            *current = rank;
                        }
                    })
                    .or_insert(rank);
                evidence.used_error_exact_frontiers[path.observed_length].insert(
                    entry.text.as_str(),
                    score,
                    top_k,
                );

                let best_error_score = (entry_identity_max_frequencies[entry_index] as f64).ln()
                    - minimum_correction_penalty;
                if score.total_cmp(&best_error_score) == Ordering::Greater
                    || (score.total_cmp(&best_error_score) == Ordering::Equal
                        && abbreviation_count == 0)
                {
                    evidence.unused_exact_frontiers[path.observed_length].insert(
                        entry.text.as_str(),
                        score,
                        top_k,
                    );
                }
            }
        }
        evidence
    }
}

#[derive(Clone, Copy)]
struct ExactCandidateRank {
    score: f64,
    abbreviation_count: usize,
}

impl ExactCandidateRank {
    fn is_better_than(&self, other: &Self) -> bool {
        match self.score.total_cmp(&other.score) {
            Ordering::Greater => true,
            Ordering::Equal => self.abbreviation_count < other.abbreviation_count,
            Ordering::Less => false,
        }
    }
}

#[derive(Default)]
struct StableTextFrontier<'a> {
    candidates: Vec<StableTextScore<'a>>,
}

impl<'a> StableTextFrontier<'a> {
    fn insert(&mut self, text: &'a str, score: f64, top_k: usize) {
        if let Some(candidate) = self
            .candidates
            .iter_mut()
            .find(|candidate| candidate.text == text)
        {
            if score.total_cmp(&candidate.score) == Ordering::Greater {
                candidate.score = score;
            }
        } else if self.candidates.len() < top_k {
            self.candidates.push(StableTextScore { text, score });
        } else if score.total_cmp(&self.candidates[top_k - 1].score) == Ordering::Greater {
            self.candidates[top_k - 1] = StableTextScore { text, score };
        } else {
            return;
        }
        self.candidates.sort_unstable_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.text.cmp(right.text))
        });
    }

    fn cutoff(&self, top_k: usize) -> Option<f64> {
        (self.candidates.len() == top_k).then(|| self.candidates[top_k - 1].score)
    }
}

struct StableTextScore<'a> {
    text: &'a str,
    score: f64,
}

#[derive(Clone, Copy, Debug)]
struct IndexedNoisyTerminal {
    path_index: usize,
    entry_index: usize,
    entry_identity: usize,
    total_score: f64,
}

impl IndexedNoisyTerminal {
    fn uses_error(&self, paths: &[NoisyTrieTerminalPath]) -> bool {
        paths[self.path_index].correction != Correction::Exact
    }
}

fn noisy_terminal_total_score(
    entry: &LexiconEntry,
    path: &NoisyTrieTerminalPath,
    config: &DecodeConfig,
) -> f64 {
    (entry.frequency as f64).ln()
        - configured_correction_penalty(config, &path.correction)
        - path.spelling.abbreviated_syllables.len() as f64
            * config.abbreviation_penalty_per_syllable
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NoisyTerminalIdentity {
    entry_identity: usize,
    observed_length: usize,
    uses_error: bool,
}

#[derive(Default)]
struct NoisyTerminalAccumulator {
    matches: Vec<IndexedNoisyTerminal>,
    indices: HashMap<NoisyTerminalIdentity, usize>,
}

impl NoisyTerminalAccumulator {
    fn upsert(
        &mut self,
        candidate: IndexedNoisyTerminal,
        paths: &[NoisyTrieTerminalPath],
        lexicon: &[LexiconEntry],
    ) {
        let path = &paths[candidate.path_index];
        let identity = NoisyTerminalIdentity {
            entry_identity: candidate.entry_identity,
            observed_length: path.observed_length,
            uses_error: candidate.uses_error(paths),
        };
        if let Some(&index) = self.indices.get(&identity) {
            if noisy_terminal_order(&candidate, &self.matches[index], paths, lexicon)
                == Ordering::Less
            {
                self.matches[index] = candidate;
            }
        } else {
            self.indices.insert(identity, self.matches.len());
            self.matches.push(candidate);
        }
    }

    fn into_matches(self) -> Vec<IndexedNoisyTerminal> {
        self.matches
    }
}

fn collapse_noisy_error_layers(
    matches: &[IndexedNoisyTerminal],
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> Vec<IndexedNoisyTerminal> {
    let mut collapsed = Vec::<IndexedNoisyTerminal>::new();
    let mut indices = HashMap::<(usize, usize), usize>::new();
    for candidate in matches {
        let identity = (
            paths[candidate.path_index].observed_length,
            candidate.entry_identity,
        );
        if let Some(&index) = indices.get(&identity) {
            if noisy_terminal_order(candidate, &collapsed[index], paths, lexicon) == Ordering::Less
            {
                collapsed[index] = *candidate;
            }
        } else {
            indices.insert(identity, collapsed.len());
            collapsed.push(*candidate);
        }
    }
    collapsed.sort_by(|left, right| noisy_terminal_segment_order(left, right, paths, lexicon));
    collapsed
}

fn retain_noisy_top_k_by_child(
    matches: Vec<IndexedNoisyTerminal>,
    state_used_error: bool,
    top_k: usize,
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> Vec<IndexedNoisyTerminal> {
    let mut groups = Vec::<Vec<IndexedNoisyTerminal>>::new();
    let mut group_indices = HashMap::<(usize, bool), usize>::new();
    for terminal in matches {
        let path = &paths[terminal.path_index];
        let child = (
            path.observed_length,
            state_used_error || terminal.uses_error(paths),
        );
        if let Some(&index) = group_indices.get(&child) {
            groups[index].push(terminal);
        } else {
            group_indices.insert(child, groups.len());
            groups.push(vec![terminal]);
        }
    }

    let mut retained = Vec::new();
    for mut group in groups {
        group.sort_by(|left, right| noisy_terminal_prepared_order(left, right, paths, lexicon));
        let mut seen_text = HashSet::new();
        group.retain(|terminal| seen_text.insert(lexicon[terminal.entry_index].text.as_str()));
        let mut globally_retained = 0;
        let mut anchored_transpositions_retained = 0;
        group.retain(|terminal| {
            let anchored_transposition =
                noisy_terminal_is_anchored_transposition_recovery(terminal, paths, lexicon);
            let retain_globally = globally_retained < top_k;
            let retain_for_recovery =
                anchored_transposition && anchored_transpositions_retained < top_k;
            if retain_globally {
                globally_retained += 1;
            }
            if retain_for_recovery {
                anchored_transpositions_retained += 1;
            }
            retain_globally || retain_for_recovery
        });
        retained.extend(group);
    }
    retained
}

fn noisy_terminal_is_anchored_transposition_recovery(
    terminal: &IndexedNoisyTerminal,
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> bool {
    let path = &paths[terminal.path_index];
    matches!(path.correction, Correction::AdjacentTransposition { .. })
        && spelling_indices_are_anchored_suffix(
            &path.spelling.abbreviated_syllables,
            lexicon[terminal.entry_index].syllable_codes.len(),
        )
}

fn noisy_terminal_segment_order(
    left: &IndexedNoisyTerminal,
    right: &IndexedNoisyTerminal,
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> Ordering {
    paths[left.path_index]
        .observed_length
        .cmp(&paths[right.path_index].observed_length)
        .then_with(|| noisy_terminal_order(left, right, paths, lexicon))
}

fn noisy_terminal_prepared_order(
    left: &IndexedNoisyTerminal,
    right: &IndexedNoisyTerminal,
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> Ordering {
    right
        .total_score
        .total_cmp(&left.total_score)
        .then_with(|| {
            lexicon[left.entry_index]
                .text
                .cmp(&lexicon[right.entry_index].text)
        })
        .then_with(|| noisy_terminal_order(left, right, paths, lexicon))
}

fn noisy_terminal_order(
    left: &IndexedNoisyTerminal,
    right: &IndexedNoisyTerminal,
    paths: &[NoisyTrieTerminalPath],
    lexicon: &[LexiconEntry],
) -> Ordering {
    let left_path = &paths[left.path_index];
    let right_path = &paths[right.path_index];
    right
        .total_score
        .total_cmp(&left.total_score)
        .then_with(|| {
            left_path
                .spelling
                .abbreviated_syllables
                .len()
                .cmp(&right_path.spelling.abbreviated_syllables.len())
        })
        .then_with(|| {
            correction_rank(&left_path.correction).cmp(&correction_rank(&right_path.correction))
        })
        .then_with(|| {
            lexicon[left.entry_index]
                .text
                .cmp(&lexicon[right.entry_index].text)
        })
        .then_with(|| {
            left_path
                .spelling
                .code
                .as_str()
                .cmp(right_path.spelling.code.as_str())
        })
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AlignmentState {
    observed_position: usize,
    used_error: bool,
    transposition_pending: bool,
}

/// With at most one edit, a consumed intended prefix can occupy only the
/// exact position, the three edit-distance offsets, and one pending swap.
const MAX_ALIGNMENT_STATES: usize = 5;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AlignmentStates {
    items: [AlignmentState; MAX_ALIGNMENT_STATES],
    len: usize,
}

impl AlignmentStates {
    fn empty() -> Self {
        Self {
            items: [AlignmentState {
                observed_position: 0,
                used_error: false,
                transposition_pending: false,
            }; MAX_ALIGNMENT_STATES],
            len: 0,
        }
    }

    fn singleton(state: AlignmentState) -> Self {
        let mut states = Self::empty();
        states.push_unique(state);
        states
    }

    fn as_slice(&self) -> &[AlignmentState] {
        &self.items[..self.len]
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn len(&self) -> usize {
        self.len
    }

    fn push_unique(&mut self, candidate: AlignmentState) {
        if self.as_slice().contains(&candidate) {
            return;
        }
        assert!(
            self.len < MAX_ALIGNMENT_STATES,
            "one-edit alignment state bound exceeded"
        );
        self.items[self.len] = candidate;
        self.len += 1;
    }

    fn canonicalize(&mut self) {
        self.items[..self.len].sort_unstable_by_key(|state| {
            (
                state.observed_position,
                state.used_error,
                state.transposition_pending,
            )
        });
    }
}

#[derive(Clone, Copy, Debug)]
struct TerminalObservedLengths {
    items: [usize; MAX_ALIGNMENT_STATES + 1],
    len: usize,
}

impl TerminalObservedLengths {
    fn empty() -> Self {
        Self {
            items: [0; MAX_ALIGNMENT_STATES + 1],
            len: 0,
        }
    }

    fn as_slice(&self) -> &[usize] {
        &self.items[..self.len]
    }

    fn push_unique(&mut self, candidate: usize) {
        if self.as_slice().contains(&candidate) {
            return;
        }
        assert!(
            self.len < self.items.len(),
            "terminal observed-length bound exceeded"
        );
        self.items[self.len] = candidate;
        self.len += 1;
    }

    fn sort_unstable(&mut self) {
        self.items[..self.len].sort_unstable();
    }
}

struct TrieSearch<'a> {
    observed: &'a [u8],
    allow_error: bool,
    intended: String,
    abbreviated_syllables: Vec<usize>,
    paths: Vec<NoisyTrieTerminalPath>,
    terminal_path_matches: usize,
    alignment_state_ids: HashMap<AlignmentStates, usize>,
    alignment_state_sets: Vec<AlignmentStates>,
    alignment_transitions: Vec<[Option<usize>; 26]>,
    pruning: Option<UnigramPrefixPruning<'a>>,
    stats: &'a mut DecodeSearchStats,
}

impl TrieSearch<'_> {
    fn observe_terminal(
        &mut self,
        node: &SyllableTrieNode,
        observed_length: usize,
        correction: &Correction,
    ) {
        if let Some(pruning) = &mut self.pruning {
            pruning.observe_terminal(
                node,
                observed_length,
                correction,
                self.abbreviated_syllables.len(),
            );
        }
    }

    fn subtree_is_dominated(&self, node: &SyllableTrieNode, states: usize) -> bool {
        self.pruning.as_ref().is_some_and(|pruning| {
            pruning.subtree_is_dominated(
                node,
                &self.alignment_state_sets[states],
                self.abbreviated_syllables.len(),
                self.observed.len(),
            )
        })
    }

    fn advance(&mut self, states: usize, intended: &[u8]) -> usize {
        let mut current = states;
        for &intended_key in intended {
            current = self.advance_one(current, intended_key);
            if self.alignment_state_sets[current].is_empty() {
                break;
            }
        }
        current
    }

    fn advance_one(&mut self, states: usize, intended_key: u8) -> usize {
        debug_assert!(intended_key.is_ascii_lowercase());
        let key_index = usize::from(intended_key - b'a');
        if let Some(next_id) = self.alignment_transitions[states][key_index] {
            self.stats.alignment_states_reused += self.alignment_state_sets[states].len();
            return next_id;
        }

        let mut next = AlignmentStates::empty();
        for state in self.alignment_state_sets[states].as_slice().iter().copied() {
            self.stats.alignment_states_examined += 1;
            self.advance_key(state, intended_key, &mut next);
        }
        next.canonicalize();
        let next = self.intern_alignment_states(next);
        self.alignment_transitions[states][key_index] = Some(next);
        next
    }

    fn intern_alignment_states(&mut self, states: AlignmentStates) -> usize {
        if let Some(&id) = self.alignment_state_ids.get(&states) {
            return id;
        }

        let id = self.alignment_state_sets.len();
        self.alignment_state_ids.insert(states, id);
        self.alignment_state_sets.push(states);
        self.alignment_transitions.push([None; 26]);
        id
    }

    fn advance_key(&self, state: AlignmentState, intended_key: u8, next: &mut AlignmentStates) {
        let position = state.observed_position;
        if state.transposition_pending {
            if self.observed.get(position) == Some(&intended_key) {
                next.push_unique(AlignmentState {
                    observed_position: position + 2,
                    used_error: true,
                    transposition_pending: false,
                });
            }
            return;
        }

        if self.observed.get(position) == Some(&intended_key) {
            next.push_unique(AlignmentState {
                observed_position: position + 1,
                ..state
            });
        }
        if !self.allow_error || state.used_error {
            return;
        }

        if self
            .observed
            .get(position)
            .is_some_and(|&actual| are_qwerty_neighbors(intended_key, actual))
        {
            next.push_unique(AlignmentState {
                observed_position: position + 1,
                used_error: true,
                transposition_pending: false,
            });
        }

        next.push_unique(AlignmentState {
            observed_position: position,
            used_error: true,
            transposition_pending: false,
        });

        if self.observed.get(position + 1) == Some(&intended_key) {
            next.push_unique(AlignmentState {
                observed_position: position + 2,
                used_error: true,
                transposition_pending: false,
            });
        }

        if position + 1 < self.observed.len()
            && self.observed[position] != self.observed[position + 1]
            && self.observed[position + 1] == intended_key
        {
            next.push_unique(AlignmentState {
                observed_position: position,
                used_error: true,
                transposition_pending: true,
            });
        }
    }

    fn terminal_observed_lengths(&self, states: usize) -> TerminalObservedLengths {
        let mut lengths = TerminalObservedLengths::empty();
        for state in self.alignment_state_sets[states].as_slice() {
            if state.transposition_pending {
                continue;
            }
            if state.observed_position > 0 {
                lengths.push_unique(state.observed_position);
            }
            let trailing_extra_length = state.observed_position + 1;
            if self.allow_error && !state.used_error && trailing_extra_length <= self.observed.len()
            {
                lengths.push_unique(trailing_extra_length);
            }
        }
        lengths.sort_unstable();
        lengths
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct KeyHypothesis {
    code: String,
    correction: Correction,
}

#[cfg(test)]
fn key_hypotheses(observed: &str, allow_error: bool) -> Vec<KeyHypothesis> {
    let observed_bytes = observed.as_bytes();
    let mut hypotheses = vec![KeyHypothesis {
        code: observed.to_owned(),
        correction: Correction::Exact,
    }];
    if !allow_error {
        return hypotheses;
    }

    for index in 0..observed_bytes.len() {
        for intended in b'a'..=b'z' {
            if are_qwerty_neighbors(intended, observed_bytes[index]) {
                let mut code = observed_bytes.to_vec();
                code[index] = intended;
                hypotheses.push(KeyHypothesis {
                    code: String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                    correction: Correction::NeighborSubstitution {
                        index,
                        intended: intended as char,
                        actual: observed_bytes[index] as char,
                    },
                });
            }
        }
    }

    for start in 0..observed_bytes.len().saturating_sub(1) {
        if observed_bytes[start] != observed_bytes[start + 1] {
            let mut code = observed_bytes.to_vec();
            code.swap(start, start + 1);
            hypotheses.push(KeyHypothesis {
                correction: Correction::AdjacentTransposition {
                    start,
                    intended_left: code[start] as char,
                    intended_right: code[start + 1] as char,
                },
                code: String::from_utf8(code).expect("generated keys are lowercase ASCII"),
            });
        }
    }

    let mut missing_by_code = BTreeMap::new();
    for index in 0..=observed_bytes.len() {
        for intended in b'a'..=b'z' {
            let mut code = observed_bytes.to_vec();
            code.insert(index, intended);
            missing_by_code.insert(
                String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                Correction::MissingKey {
                    index,
                    intended: intended as char,
                },
            );
        }
    }
    hypotheses.extend(
        missing_by_code
            .into_iter()
            .map(|(code, correction)| KeyHypothesis { code, correction }),
    );

    if observed_bytes.len() > 1 {
        let mut extra_by_code = BTreeMap::new();
        for index in 0..observed_bytes.len() {
            let mut code = observed_bytes.to_vec();
            code.remove(index);
            extra_by_code.insert(
                String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                Correction::ExtraKey {
                    index,
                    actual: observed_bytes[index] as char,
                },
            );
        }
        hypotheses.extend(
            extra_by_code
                .into_iter()
                .map(|(code, correction)| KeyHypothesis { code, correction }),
        );
    }

    hypotheses
}

#[derive(Clone, Debug)]
struct SentenceLattice {
    outgoing: Vec<Vec<SegmentTransition>>,
    length: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SentenceRankingState {
    position: usize,
    used_error: bool,
    previous_word: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SentenceRankingConfig {
    top_k: usize,
    segmentations_per_text: usize,
    log_frequency_total: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct SegmentTransition {
    end: usize,
    uses_error: bool,
    observed: KeySequence,
    candidate: Candidate,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SegmentTransitionIdentity {
    end: usize,
    uses_error: bool,
    source: CandidateSource,
    text: String,
    code: KeySequence,
}

impl From<&SegmentTransition> for SegmentTransitionIdentity {
    fn from(transition: &SegmentTransition) -> Self {
        Self {
            end: transition.end,
            uses_error: transition.uses_error,
            source: transition.candidate.source,
            text: transition.candidate.text.clone(),
            code: transition.candidate.code.clone(),
        }
    }
}

#[derive(Default)]
struct SegmentTransitionAccumulator {
    transitions: Vec<SegmentTransition>,
    indices: HashMap<SegmentTransitionIdentity, usize>,
}

impl SegmentTransitionAccumulator {
    fn upsert(&mut self, candidate: SegmentTransition) {
        let identity = SegmentTransitionIdentity::from(&candidate);
        if let Some(&index) = self.indices.get(&identity) {
            if candidate_order(&candidate.candidate, &self.transitions[index].candidate)
                == Ordering::Less
            {
                self.transitions[index] = candidate;
            }
        } else {
            self.indices.insert(identity, self.transitions.len());
            self.transitions.push(candidate);
        }
    }

    fn into_transitions(self) -> Vec<SegmentTransition> {
        self.transitions
    }
}

#[derive(Clone, Debug)]
struct PreparedSentenceTransition {
    transition: SegmentTransition,
    language_score: SentenceLanguageScore,
    edge_score: f64,
    unresolved_key_count: usize,
}

#[derive(Clone, Debug)]
struct SentenceTransitionGroup {
    child_state: SentenceRankingState,
    transitions: Vec<PreparedSentenceTransition>,
}

fn collapse_error_layers(transitions: &[SegmentTransition]) -> Vec<SegmentTransition> {
    let mut collapsed = Vec::<SegmentTransition>::new();
    let mut indices = HashMap::<(usize, CandidateSource, String, KeySequence), usize>::new();
    for candidate in transitions {
        let identity = (
            candidate.end,
            candidate.candidate.source,
            candidate.candidate.text.clone(),
            candidate.candidate.code.clone(),
        );
        if let Some(&index) = indices.get(&identity) {
            if candidate_order(&candidate.candidate, &collapsed[index].candidate) == Ordering::Less
            {
                collapsed[index] = candidate.clone();
            }
        } else {
            indices.insert(identity, collapsed.len());
            collapsed.push(candidate.clone());
        }
    }
    collapsed.sort_by(segment_transition_order);
    collapsed
}

fn segment_transition_order(left: &SegmentTransition, right: &SegmentTransition) -> Ordering {
    left.end
        .cmp(&right.end)
        .then_with(|| candidate_order(&left.candidate, &right.candidate))
}

fn prepared_transition_order(
    left: &PreparedSentenceTransition,
    right: &PreparedSentenceTransition,
) -> Ordering {
    left.unresolved_key_count
        .cmp(&right.unresolved_key_count)
        .then_with(|| right.edge_score.total_cmp(&left.edge_score))
        .then_with(|| {
            left.transition
                .candidate
                .text
                .cmp(&right.transition.candidate.text)
        })
        .then_with(|| candidate_order(&left.transition.candidate, &right.transition.candidate))
}

fn sentence_is_anchored_transposition_recovery(candidate: &SentenceCandidate) -> bool {
    let mut found_anchored_transposition = false;
    for segment in &candidate.segments {
        if segment.candidate.source != CandidateSource::Lexicon {
            return false;
        }
        let abbreviated = &segment.candidate.spelling.abbreviated_syllables;
        if !abbreviated.is_empty() && !spelling_is_anchored_suffix(&segment.candidate) {
            return false;
        }
        if candidate_is_anchored_transposition_recovery(&segment.candidate) {
            found_anchored_transposition = true;
        }
    }
    found_anchored_transposition
}

fn candidate_is_anchored_transposition_recovery(candidate: &Candidate) -> bool {
    matches!(
        candidate.correction,
        Correction::AdjacentTransposition { .. }
    ) && spelling_is_anchored_suffix(candidate)
}

fn spelling_is_anchored_suffix(candidate: &Candidate) -> bool {
    spelling_indices_are_anchored_suffix(
        &candidate.spelling.abbreviated_syllables,
        candidate.code.as_str().len() / 2,
    )
}

pub(crate) fn spelling_is_complete_or_anchored_suffix(candidate: &Candidate) -> bool {
    candidate.spelling.abbreviated_syllables.is_empty() || spelling_is_anchored_suffix(candidate)
}

fn spelling_indices_are_anchored_suffix(abbreviated: &[usize], syllable_count: usize) -> bool {
    let Some(&first_abbreviated) = abbreviated.first() else {
        return false;
    };
    first_abbreviated > 0
        && first_abbreviated < syllable_count
        && abbreviated.len() == syllable_count - first_abbreviated
        && abbreviated
            .iter()
            .copied()
            .eq(first_abbreviated..syllable_count)
}

fn retain_bounded_text(seen: &mut HashSet<String>, text: &str, top_k: usize) -> bool {
    seen.contains(text) || (seen.len() < top_k && seen.insert(text.to_owned()))
}

fn sentence_order(left: &SentenceCandidate, right: &SentenceCandidate) -> Ordering {
    left.unresolved_key_count
        .cmp(&right.unresolved_key_count)
        .then_with(|| left.used_error.cmp(&right.used_error))
        .then_with(|| right.total_score.total_cmp(&left.total_score))
        .then_with(|| left.segments.len().cmp(&right.segments.len()))
        .then_with(|| left.text.cmp(&right.text))
}

fn candidate_order(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .score
        .total
        .total_cmp(&left.score.total)
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| {
            left.spelling
                .abbreviated_syllables
                .len()
                .cmp(&right.spelling.abbreviated_syllables.len())
        })
        .then_with(|| correction_rank(&left.correction).cmp(&correction_rank(&right.correction)))
        .then_with(|| left.text.cmp(&right.text))
        .then_with(|| {
            left.spelling
                .code
                .as_str()
                .cmp(right.spelling.code.as_str())
        })
}

fn correction_rank(correction: &Correction) -> u8 {
    match correction {
        Correction::Exact => 0,
        Correction::AdjacentTransposition { .. } => 1,
        Correction::NeighborSubstitution { .. } => 2,
        Correction::MissingKey { .. } => 3,
        Correction::ExtraKey { .. } => 4,
    }
}

fn configured_correction_penalty(config: &DecodeConfig, correction: &Correction) -> f64 {
    match correction {
        Correction::Exact => 0.0,
        Correction::NeighborSubstitution { .. } => config.neighbor_substitution_penalty,
        Correction::AdjacentTransposition { .. } => config.adjacent_transposition_penalty,
        Correction::MissingKey { .. } => config.missing_key_penalty,
        Correction::ExtraKey { .. } => config.extra_key_penalty,
    }
}

fn lexicon_entry_identity_ids(lexicon: &[LexiconEntry]) -> Vec<usize> {
    let mut identities = HashMap::<(&str, &str), usize>::new();
    let mut next_identity = 0;
    lexicon
        .iter()
        .map(|entry| {
            *identities
                .entry((entry.text.as_str(), entry.code.as_str()))
                .or_insert_with(|| {
                    let identity = next_identity;
                    next_identity += 1;
                    identity
                })
        })
        .collect()
}

fn lexicon_entry_identity_max_frequencies(
    lexicon: &[LexiconEntry],
    identity_ids: &[usize],
) -> Vec<u64> {
    let identity_count = identity_ids
        .iter()
        .copied()
        .max()
        .map_or(0, |maximum| maximum + 1);
    let mut maximum_frequencies = vec![0; identity_count];
    for (entry, &identity) in lexicon.iter().zip(identity_ids) {
        maximum_frequencies[identity] = maximum_frequencies[identity].max(entry.frequency);
    }
    identity_ids
        .iter()
        .map(|&identity| maximum_frequencies[identity])
        .collect()
}

pub(crate) fn spelling_variants(syllable_codes: &[KeySequence]) -> Vec<Spelling> {
    let mut variants = vec![(String::new(), Vec::new())];
    for (syllable_index, syllable_code) in syllable_codes.iter().enumerate() {
        let mut next = Vec::with_capacity(variants.len() * 2);
        for (raw_code, abbreviated_syllables) in variants {
            let mut full_code = raw_code.clone();
            full_code.push_str(syllable_code.as_str());
            next.push((full_code, abbreviated_syllables.clone()));

            let mut abbreviated_code = raw_code;
            abbreviated_code.push(
                syllable_code
                    .as_str()
                    .chars()
                    .next()
                    .expect("a syllable code is non-empty"),
            );
            let mut abbreviated = abbreviated_syllables;
            abbreviated.push(syllable_index);
            next.push((abbreviated_code, abbreviated));
        }
        variants = next;
    }

    variants
        .into_iter()
        .map(|(code, abbreviated_syllables)| Spelling {
            code: KeySequence::new(code).expect("syllable variants are lowercase ASCII"),
            abbreviated_syllables,
        })
        .collect()
}

/// Result of importing a Rime YAML dictionary into the decoder's lexicon.
#[derive(Clone, Debug, PartialEq)]
pub struct RimeLexiconImport {
    /// Valid, deduplicated entries accepted by the Ziranma codec.
    pub entries: Vec<LexiconEntry>,
    /// Auditable source-row accounting.
    pub stats: RimeLexiconImportStats,
}

/// Row accounting for one Rime dictionary import.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RimeLexiconImportStats {
    /// Non-comment rows after the Rime YAML `...` marker.
    pub source_rows: usize,
    /// Rows retained in `RimeLexiconImport::entries`.
    pub imported_entries: usize,
    /// Zero source weights conservatively raised to one.
    pub zero_weights_floored: usize,
    /// Rows skipped because the baseline codec cannot map their pinyin.
    pub unsupported_pinyin_rows: usize,
    /// Rows skipped because they exceed the test baseline's syllable limit.
    pub too_many_syllable_rows: usize,
    /// Rows skipped because the same text and Ziranma code already appeared.
    pub duplicate_rows: usize,
    /// Shadowed traditional single-character readings skipped by the pinned
    /// simplified-Rime importer.
    pub shadowed_traditional_single_character_rows: usize,
}

const SHADOWED_TRADITIONAL_SINGLE_CHARACTER_READINGS: &str = include_str!(
    "../data/public/rime-pinyin-simp/shadowed-traditional-single-character-readings.txt"
);

#[derive(Debug)]
struct ShadowedTraditionalSingleCharacterReadings {
    characters: HashSet<char>,
    retained_readings: HashSet<(&'static str, &'static str)>,
}

fn shadowed_traditional_single_character_readings()
-> &'static ShadowedTraditionalSingleCharacterReadings {
    static READINGS: OnceLock<ShadowedTraditionalSingleCharacterReadings> = OnceLock::new();
    READINGS.get_or_init(|| {
        let mut characters = HashSet::new();
        let mut retained_readings = HashSet::new();
        for line in SHADOWED_TRADITIONAL_SINGLE_CHARACTER_READINGS.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(reading) = line.strip_prefix("!keep\t") {
                let (text, pinyin) = reading
                    .split_once('\t')
                    .expect("pinned retained reading must contain text and pinyin");
                retained_readings.insert((text, pinyin));
                continue;
            }
            characters.extend(line.chars().filter(|character| !character.is_whitespace()));
        }
        ShadowedTraditionalSingleCharacterReadings {
            characters,
            retained_readings,
        }
    })
}

fn is_shadowed_traditional_single_character_reading(text: &str, pinyin: &str) -> bool {
    let mut characters = text.chars();
    let Some(character) = characters.next() else {
        return false;
    };
    if characters.next().is_some() {
        return false;
    }
    let readings = shadowed_traditional_single_character_readings();
    readings.characters.contains(&character)
        && !readings.retained_readings.contains(&(text, pinyin))
}

/// Imports the standard three-column body of a Rime YAML dictionary.
///
/// The upstream file is kept unchanged. Rows use
/// `text<TAB>pinyin<TAB>weight`; zero weights are floored to one, while
/// unsupported pinyin, overlong entries, and duplicate text/code pairs are
/// skipped and counted explicitly.
pub fn parse_rime_lexicon(contents: &str) -> Result<RimeLexiconImport, RimeLexiconParseError> {
    parse_rime_lexicon_with_options(contents, false)
}

/// Imports the pinned `rime-pinyin-simp` snapshot while omitting conservative,
/// auditable traditional single-character duplicates.
///
/// The filter is derived from pinned Rime and OpenCC revisions. It only omits
/// a reading when the same pinyin has a different simplified character with
/// an equal or greater source weight. Multi-character entries are untouched.
pub fn parse_simplified_rime_lexicon(
    contents: &str,
) -> Result<RimeLexiconImport, RimeLexiconParseError> {
    parse_rime_lexicon_with_options(contents, true)
}

fn parse_rime_lexicon_with_options(
    contents: &str,
    omit_shadowed_traditional_single_characters: bool,
) -> Result<RimeLexiconImport, RimeLexiconParseError> {
    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut entries = Vec::new();
    let mut duplicates = HashSet::new();
    let mut stats = RimeLexiconImportStats::default();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let line_number = zero_based_line + 1;
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
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) {
            return Err(RimeLexiconParseError::InvalidRow { line_number });
        }
        let source_weight =
            fields[2]
                .parse::<u64>()
                .map_err(|_| RimeLexiconParseError::InvalidWeight {
                    line_number,
                    value: fields[2].to_owned(),
                })?;
        if source_weight == 0 {
            stats.zero_weights_floored += 1;
        }
        if omit_shadowed_traditional_single_characters
            && is_shadowed_traditional_single_character_reading(fields[0], fields[1])
        {
            stats.shadowed_traditional_single_character_rows += 1;
            continue;
        }

        let encoded = match encode_pinyin_phrase(fields[1]) {
            Ok(encoded) => encoded,
            Err(_) => {
                stats.unsupported_pinyin_rows += 1;
                continue;
            }
        };
        if encoded.syllable_codes.len() > MAX_LEXICON_SYLLABLES {
            stats.too_many_syllable_rows += 1;
            continue;
        }
        let duplicate_key = (fields[0].to_owned(), encoded.full_code.clone());
        if !duplicates.insert(duplicate_key) {
            stats.duplicate_rows += 1;
            continue;
        }

        entries.push(LexiconEntry {
            text: fields[0].to_owned(),
            pinyin: fields[1].to_owned(),
            code: encoded.full_code,
            syllable_codes: encoded.syllable_codes,
            frequency: source_weight.max(1),
        });
    }

    if !saw_document_start {
        return Err(RimeLexiconParseError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(RimeLexiconParseError::MissingDataMarker);
    }
    if entries.is_empty() {
        return Err(RimeLexiconParseError::Empty);
    }
    stats.imported_entries = entries.len();
    Ok(RimeLexiconImport { entries, stats })
}

/// Error returned while importing a Rime YAML dictionary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RimeLexiconParseError {
    /// No YAML document start marker was found.
    MissingDocumentStart,
    /// No Rime data marker was found after the YAML header.
    MissingDataMarker,
    /// A data row did not have three non-empty tab-separated fields.
    InvalidRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A source weight was not an unsigned integer.
    InvalidWeight {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// The header was valid but no compatible entries remained.
    Empty,
}

impl fmt::Display for RimeLexiconParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDocumentStart => write!(formatter, "Rime 词典缺少 YAML 起始标记 ---"),
            Self::MissingDataMarker => write!(formatter, "Rime 词典缺少数据起始标记 ..."),
            Self::InvalidRow { line_number } => {
                write!(formatter, "Rime 词典第 {line_number} 行字段无效")
            }
            Self::InvalidWeight { line_number, value } => write!(
                formatter,
                "Rime 词典第 {line_number} 行权重必须是非负整数，实际为 {value:?}"
            ),
            Self::Empty => write!(formatter, "Rime 词典没有可导入的数据行"),
        }
    }
}

impl Error for RimeLexiconParseError {}

/// Parses the repository's auditable tab-separated demo lexicon format.
///
/// The first non-comment row must be:
/// `text<TAB>pinyin<TAB>frequency`.
pub fn parse_lexicon_tsv(contents: &str) -> Result<Vec<LexiconEntry>, LexiconParseError> {
    const EXPECTED_HEADER: [&str; 3] = ["text", "pinyin", "frequency"];

    let mut saw_header = false;
    let mut entries = Vec::new();
    let mut duplicates = HashSet::new();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let line_number = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields = line.split('\t').collect::<Vec<_>>();
        if !saw_header {
            if fields != EXPECTED_HEADER {
                return Err(LexiconParseError::InvalidHeader { line_number });
            }
            saw_header = true;
            continue;
        }

        if fields.len() != EXPECTED_HEADER.len() || fields.iter().any(|field| field.is_empty()) {
            return Err(LexiconParseError::InvalidRow { line_number });
        }

        let frequency =
            fields[2]
                .parse::<u64>()
                .map_err(|_| LexiconParseError::InvalidFrequency {
                    line_number,
                    value: fields[2].to_owned(),
                })?;
        if frequency == 0 {
            return Err(LexiconParseError::InvalidFrequency {
                line_number,
                value: fields[2].to_owned(),
            });
        }

        let encoded =
            encode_pinyin_phrase(fields[1]).map_err(|_| LexiconParseError::InvalidPinyin {
                line_number,
                value: fields[1].to_owned(),
            })?;
        if encoded.syllable_codes.len() > MAX_LEXICON_SYLLABLES {
            return Err(LexiconParseError::TooManySyllables {
                line_number,
                count: encoded.syllable_codes.len(),
                maximum: MAX_LEXICON_SYLLABLES,
            });
        }

        let duplicate_key = (fields[0].to_owned(), encoded.full_code.clone());
        if !duplicates.insert(duplicate_key) {
            return Err(LexiconParseError::DuplicateEntry { line_number });
        }

        entries.push(LexiconEntry {
            text: fields[0].to_owned(),
            pinyin: fields[1].to_owned(),
            code: encoded.full_code,
            syllable_codes: encoded.syllable_codes,
            frequency,
        });
    }

    if !saw_header {
        return Err(LexiconParseError::MissingHeader);
    }
    if entries.is_empty() {
        return Err(LexiconParseError::Empty);
    }

    Ok(entries)
}

/// Error returned while parsing a public lexicon fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexiconParseError {
    /// No non-comment header row was found.
    MissingHeader,
    /// The header did not match the documented three columns.
    InvalidHeader {
        /// One-based source line number.
        line_number: usize,
    },
    /// A data row had missing, extra, or empty fields.
    InvalidRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A frequency was not a positive integer.
    InvalidFrequency {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// A pinyin phrase could not be encoded.
    InvalidPinyin {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// A row would create too many exhaustive abbreviation variants.
    TooManySyllables {
        /// One-based source line number.
        line_number: usize,
        /// Actual number of syllables.
        count: usize,
        /// Accepted maximum.
        maximum: usize,
    },
    /// The same text and code appeared more than once.
    DuplicateEntry {
        /// One-based source line number.
        line_number: usize,
    },
    /// A valid header was present but no entries followed.
    Empty,
}

impl fmt::Display for LexiconParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(formatter, "词典缺少表头"),
            Self::InvalidHeader { line_number } => {
                write!(formatter, "词典第 {line_number} 行表头无效")
            }
            Self::InvalidRow { line_number } => {
                write!(formatter, "词典第 {line_number} 行字段无效")
            }
            Self::InvalidFrequency { line_number, value } => write!(
                formatter,
                "词典第 {line_number} 行的频率必须是正整数，实际为 {value:?}"
            ),
            Self::InvalidPinyin { line_number, value } => write!(
                formatter,
                "词典第 {line_number} 行的拼音无法编码，实际为 {value:?}"
            ),
            Self::TooManySyllables {
                line_number,
                count,
                maximum,
            } => write!(
                formatter,
                "词典第 {line_number} 行有 {count} 个音节，穷举基线最多接受 {maximum} 个"
            ),
            Self::DuplicateEntry { line_number } => {
                write!(formatter, "词典第 {line_number} 行重复")
            }
            Self::Empty => write!(formatter, "词典没有数据行"),
        }
    }
}

impl Error for LexiconParseError {}

fn detect_correction(observed: &str, intended: &str) -> Option<Correction> {
    if observed.len() + 1 == intended.len() {
        let index = single_removed_index(observed.as_bytes(), intended.as_bytes())?;
        return Some(Correction::MissingKey {
            index,
            intended: intended.as_bytes()[index] as char,
        });
    }
    if observed.len() == intended.len() + 1 {
        let index = single_removed_index(intended.as_bytes(), observed.as_bytes())?;
        return Some(Correction::ExtraKey {
            index,
            actual: observed.as_bytes()[index] as char,
        });
    }
    if observed.len() != intended.len() {
        return None;
    }

    let observed = observed.as_bytes();
    let intended = intended.as_bytes();
    let differences = observed
        .iter()
        .zip(intended)
        .enumerate()
        .filter_map(|(index, (actual, expected))| (actual != expected).then_some(index))
        .collect::<Vec<_>>();

    match differences.as_slice() {
        [] => Some(Correction::Exact),
        [index] if are_qwerty_neighbors(intended[*index], observed[*index]) => {
            Some(Correction::NeighborSubstitution {
                index: *index,
                intended: intended[*index] as char,
                actual: observed[*index] as char,
            })
        }
        [left, right]
            if *right == *left + 1
                && observed[*left] == intended[*right]
                && observed[*right] == intended[*left] =>
        {
            Some(Correction::AdjacentTransposition {
                start: *left,
                intended_left: intended[*left] as char,
                intended_right: intended[*right] as char,
            })
        }
        _ => None,
    }
}

fn single_removed_index(shorter: &[u8], longer: &[u8]) -> Option<usize> {
    if shorter.len() + 1 != longer.len() {
        return None;
    }
    let first_difference = shorter
        .iter()
        .zip(longer)
        .position(|(left, right)| left != right)
        .unwrap_or(shorter.len());
    (shorter[first_difference..] == longer[first_difference + 1..]).then_some(first_difference)
}

/// Returns whether two lowercase ASCII keys are physical QWERTY neighbors.
///
/// This is the shared keyboard geometry used by both the decoder's error
/// channel and public synthetic audits. Non-lowercase-ASCII input is never a
/// neighbor.
pub fn are_qwerty_neighbors(left: u8, right: u8) -> bool {
    match left {
        b'q' => matches!(right, b'w' | b'a'),
        b'w' => matches!(right, b'q' | b'e' | b'a' | b's'),
        b'e' => matches!(right, b'w' | b'r' | b's' | b'd'),
        b'r' => matches!(right, b'e' | b't' | b'd' | b'f'),
        b't' => matches!(right, b'r' | b'y' | b'f' | b'g'),
        b'y' => matches!(right, b't' | b'u' | b'g' | b'h'),
        b'u' => matches!(right, b'y' | b'i' | b'h' | b'j'),
        b'i' => matches!(right, b'u' | b'o' | b'j' | b'k'),
        b'o' => matches!(right, b'i' | b'p' | b'k' | b'l'),
        b'p' => matches!(right, b'o' | b'l'),
        b'a' => matches!(right, b'q' | b'w' | b's' | b'z'),
        b's' => {
            matches!(right, b'w' | b'e' | b'a' | b'd' | b'z' | b'x')
        }
        b'd' => {
            matches!(right, b'e' | b'r' | b's' | b'f' | b'x' | b'c')
        }
        b'f' => {
            matches!(right, b'r' | b't' | b'd' | b'g' | b'c' | b'v')
        }
        b'g' => {
            matches!(right, b't' | b'y' | b'f' | b'h' | b'v' | b'b')
        }
        b'h' => {
            matches!(right, b'y' | b'u' | b'g' | b'j' | b'b' | b'n')
        }
        b'j' => {
            matches!(right, b'u' | b'i' | b'h' | b'k' | b'n' | b'm')
        }
        b'k' => matches!(right, b'i' | b'o' | b'j' | b'l' | b'm'),
        b'l' => matches!(right, b'o' | b'p' | b'k'),
        b'z' => matches!(right, b'a' | b's' | b'x'),
        b'x' => matches!(right, b's' | b'd' | b'z' | b'c'),
        b'c' => matches!(right, b'd' | b'f' | b'x' | b'v'),
        b'v' => matches!(right, b'f' | b'g' | b'c' | b'b'),
        b'b' => matches!(right, b'g' | b'h' | b'v' | b'n'),
        b'n' => matches!(right, b'h' | b'j' | b'b' | b'm'),
        b'm' => matches!(right, b'j' | b'k' | b'n'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::{
        BigramLanguageModel, Candidate, CandidateSource, Correction, DecodeConfig, Decoder,
        KeySequence, LexiconEntry, SentenceCandidate, SentenceLattice, SentenceRankingState,
        SentenceSearchStats, SentenceSegment, are_qwerty_neighbors, candidate_order,
        collapse_error_layers, detect_correction, key_hypotheses, parse_lexicon_tsv,
        parse_rime_lexicon, parse_simplified_rime_lexicon, sentence_order, spelling_variants,
    };

    const FIXTURE: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
    const BIGRAM_FIXTURE: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");

    #[test]
    fn key_sequence_accepts_only_lowercase_ascii() {
        assert!(KeySequence::new("nihk").is_ok());
        assert!(KeySequence::new("").is_err());
        assert!(KeySequence::new("NiHk").is_err());
        assert!(KeySequence::new("你好").is_err());
    }

    #[test]
    fn rime_import_reports_every_compatibility_decision() {
        let fixture = "\
---
name: test
...
你好\tni hao\t10
罕\than\t0
呒\thm\t3
你好\tni hao\t8
";
        let imported = parse_rime_lexicon(fixture).unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| (entry.text.as_str(), entry.frequency))
                .collect::<Vec<_>>(),
            [("你好", 10), ("罕", 1)]
        );
        assert_eq!(imported.stats.source_rows, 4);
        assert_eq!(imported.stats.imported_entries, 2);
        assert_eq!(imported.stats.zero_weights_floored, 1);
        assert_eq!(imported.stats.unsupported_pinyin_rows, 1);
        assert_eq!(imported.stats.too_many_syllable_rows, 0);
        assert_eq!(imported.stats.duplicate_rows, 1);
    }

    #[test]
    fn rime_import_rejects_structural_drift() {
        assert!(parse_rime_lexicon("name: test\n...\n你\tni\t1\n").is_err());
        assert!(parse_rime_lexicon("---\nname: test\n你\tni\t1\n").is_err());
        assert!(parse_rime_lexicon("---\n...\n你\tni\tmany\n").is_err());
    }

    #[test]
    fn simplified_rime_import_omits_only_pinned_shadowed_single_readings() {
        let fixture = "---\nname: test\n...\n\
說\tshuo\t621\n\
说\tshuo\t338803\n\
比如說\tbi ru shuo\t10\n\
比如说\tbi ru shuo\t20\n\
乾\tgan\t10\n\
乾\tqian\t10\n\
乾隆\tqian long\t10\n\
乾坤\tqian kun\t10\n\
哪吒\tna zha\t10\n";

        let generic = parse_rime_lexicon(fixture).unwrap();
        assert!(generic.entries.iter().any(|entry| entry.text == "說"));
        assert!(
            generic
                .entries
                .iter()
                .any(|entry| entry.text == "乾" && entry.pinyin == "gan")
        );

        let simplified = parse_simplified_rime_lexicon(fixture).unwrap();
        assert_eq!(
            simplified
                .entries
                .iter()
                .map(|entry| (entry.text.as_str(), entry.pinyin.as_str()))
                .collect::<Vec<_>>(),
            [
                ("说", "shuo"),
                ("比如說", "bi ru shuo"),
                ("比如说", "bi ru shuo"),
                ("乾", "qian"),
                ("乾隆", "qian long"),
                ("乾坤", "qian kun"),
                ("哪吒", "na zha"),
            ]
        );
        assert_eq!(
            simplified.stats.shadowed_traditional_single_character_rows,
            2
        );
        assert_eq!(generic.stats.shadowed_traditional_single_character_rows, 0);
    }

    #[test]
    fn pinned_shadowed_reading_audit_has_stable_bounds() {
        let readings = super::shadowed_traditional_single_character_readings();
        assert_eq!(readings.characters.len(), 2_359);
        assert_eq!(readings.retained_readings.len(), 7);
        assert!(readings.characters.contains(&'說'));
        assert!(readings.retained_readings.contains(&("乾", "qian")));
    }

    #[test]
    fn neighbor_map_is_symmetric() {
        for left in b'a'..=b'z' {
            for right in b'a'..=b'z' {
                assert_eq!(
                    are_qwerty_neighbors(left, right),
                    are_qwerty_neighbors(right, left),
                    "{} -> {} was not symmetric",
                    left as char,
                    right as char
                );
            }
        }
    }

    #[test]
    fn generates_all_mixed_abbreviation_variants() {
        let syllables = [
            KeySequence::new("ni").unwrap(),
            KeySequence::new("hk").unwrap(),
        ];
        let variants = spelling_variants(&syllables);
        assert_eq!(variants.len(), 4);
        assert_eq!(
            variants
                .iter()
                .map(|variant| variant.code.as_str())
                .collect::<Vec<_>>(),
            ["nihk", "nih", "nhk", "nh"]
        );
    }

    #[test]
    fn detects_all_supported_single_corrections() {
        assert_eq!(detect_correction("nihk", "nihk"), Some(Correction::Exact));
        assert!(matches!(
            detect_correction("nigk", "nihk"),
            Some(Correction::NeighborSubstitution {
                index: 2,
                intended: 'h',
                actual: 'g'
            })
        ));
        assert!(matches!(
            detect_correction("nikh", "nihk"),
            Some(Correction::AdjacentTransposition { start: 2, .. })
        ));
        assert!(matches!(
            detect_correction("nik", "nihk"),
            Some(Correction::MissingKey {
                index: 2,
                intended: 'h'
            })
        ));
        assert!(matches!(
            detect_correction("niihk", "nihk"),
            Some(Correction::ExtraKey {
                index: 2,
                actual: 'i'
            })
        ));
        assert!(detect_correction("niqk", "nihk").is_none());
        assert!(detect_correction("nifj", "nihk").is_none());
        assert!(detect_correction("ni", "nihk").is_none());
    }

    #[test]
    fn joint_trie_search_matches_both_previous_references() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);

        let regression_cases = word_regression_cases(&decoder);
        assert!(regression_cases.len() > 1_000);
        for observed in regression_cases {
            let actual = decoder.decode(&observed, 10).unwrap();
            assert_eq!(
                actual,
                hypothesis_reference(&decoder, &observed, 10, true),
                "hypothesis reference diverged for {observed}"
            );
            assert_eq!(
                actual,
                exhaustive_reference(&decoder, &observed, 10),
                "exhaustive reference diverged for {observed}"
            );
        }

        for observed in decoder
            .lexicon
            .iter()
            .flat_map(|entry| spelling_variants(&entry.syllable_codes))
            .map(|spelling| spelling.code.as_str().to_owned())
            .collect::<BTreeSet<_>>()
        {
            assert_eq!(
                decoder.lookup_candidates(&observed, false),
                hypothesis_reference(&decoder, &observed, usize::MAX, false),
                "exact-only lookup diverged for {observed}"
            );
        }
    }

    #[test]
    fn streaming_sentence_edges_match_slice_reference() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);
        let mut cases = BTreeSet::new();

        for pair in decoder.lexicon.windows(2) {
            let abbreviated = format!(
                "{}{}",
                fully_abbreviated(&pair[0]),
                fully_abbreviated(&pair[1])
            );
            cases.insert(abbreviated.clone());
            cases.insert(format!("{}{}", pair[0].code, pair[1].code));

            let bytes = abbreviated.as_bytes();
            if bytes.len() > 1 && bytes[0] != bytes[1] {
                let mut transposed = bytes.to_vec();
                transposed.swap(0, 1);
                cases.insert(String::from_utf8(transposed).unwrap());
            }
            let mut missing = bytes.to_vec();
            missing.remove(bytes.len() / 2);
            cases.insert(String::from_utf8(missing).unwrap());

            let mut extra = bytes.to_vec();
            extra.insert(bytes.len() / 2, bytes[bytes.len() / 2]);
            cases.insert(String::from_utf8(extra).unwrap());

            if let Some(neighbor) = (b'a'..=b'z').find(|&key| are_qwerty_neighbors(bytes[0], key)) {
                let mut substituted = bytes.to_vec();
                substituted[0] = neighbor;
                cases.insert(String::from_utf8(substituted).unwrap());
            }
        }

        assert!(cases.len() > 150);
        for observed in cases {
            for start in 0..observed.len() {
                for allow_error in [false, true] {
                    let mut stats = SentenceSearchStats::default();
                    let streaming = decoder.segment_transitions(
                        &observed,
                        start,
                        allow_error,
                        !allow_error,
                        None,
                        &mut stats,
                    );
                    let streaming = if allow_error {
                        collapse_error_layers(&streaming)
                    } else {
                        streaming
                    };
                    assert_eq!(
                        streaming,
                        decoder.segment_transitions_by_slices(&observed, start, allow_error),
                        "lattice edges diverged for {observed}, start {start}, allow_error {allow_error}"
                    );
                }
            }
        }
    }

    #[test]
    fn streaming_scan_keeps_exact_edge_for_the_used_error_layer() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let config = DecodeConfig {
            abbreviation_penalty_per_syllable: 5.0,
            ..DecodeConfig::default()
        };
        let decoder = Decoder::with_config(lexicon, config);
        let mut stats = SentenceSearchStats::default();
        let transitions = decoder.segment_transitions("nh", 0, true, false, None, &mut stats);
        let hello = transitions
            .iter()
            .filter(|transition| transition.candidate.text == "你好")
            .collect::<Vec<_>>();

        assert!(hello.iter().any(|transition| !transition.uses_error));
        assert!(hello.iter().any(|transition| transition.uses_error));

        let collapsed = collapse_error_layers(&transitions);
        let selected = collapsed
            .iter()
            .find(|transition| transition.candidate.text == "你好")
            .unwrap();
        assert!(selected.uses_error);
    }

    #[test]
    fn subtree_bound_keeps_equal_score_branches_for_deterministic_ties() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
             zeta\tni\t100\n\
             alpha\tnao\t100\n",
        )
        .unwrap();
        let decoder = Decoder::new(lexicon);
        let (candidates, stats) = decoder.decode_sentence_with_stats("n", 1).unwrap();

        assert_eq!(candidates[0].text, "alpha");
        assert!(stats.terminal_path_matches >= 2);
    }

    #[test]
    fn terminal_entry_bound_keeps_equal_score_text_ties() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
             zeta\tni\t100\n\
             alpha\tni\t100\n",
        )
        .unwrap();
        let decoder = Decoder::new(lexicon);
        let (candidates, stats) = decoder.decode_sentence_with_stats("ni", 1).unwrap();

        assert_eq!(candidates[0].text, "alpha");
        assert!(stats.terminal_spelling_matches >= 2);
    }

    #[test]
    fn lightweight_terminal_compaction_matches_materialized_reduction() {
        let decoder = Decoder::new(parse_lexicon_tsv(FIXTURE).unwrap());
        let log_frequency_total = decoder
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>()
            .ln();
        let mut observed_exact_prune = false;

        for observed in ["zrmurf", "nhk", "nigk", "ajjp", "nihkz"] {
            for start in 0..observed.len() {
                for (exact_reachable, error_reachable) in
                    [(true, false), (false, true), (true, true)]
                {
                    for top_k in [1, 5, 10, 25, 50] {
                        let full = decoder.segment_transitions(
                            observed,
                            start,
                            exact_reachable,
                            error_reachable,
                            None,
                            &mut SentenceSearchStats::default(),
                        );
                        let expected = decoder.compact_unigram_lattice_transitions(
                            full,
                            start,
                            exact_reachable,
                            error_reachable,
                            top_k,
                            log_frequency_total,
                        );
                        let mut compact_stats = SentenceSearchStats::default();
                        let compact = decoder.segment_transitions(
                            observed,
                            start,
                            exact_reachable,
                            error_reachable,
                            Some(top_k),
                            &mut compact_stats,
                        );
                        observed_exact_prune |= compact_stats.trie_subtree_prunes > 0;
                        assert_eq!(
                            compact, expected,
                            "terminal compaction diverged for {observed}, start {start}, \
                             exact {exact_reachable}, error {error_reachable}, K {top_k}"
                        );
                    }
                }
            }
        }
        assert!(
            observed_exact_prune,
            "the focused parity matrix must exercise the exact subtree bound"
        );
    }

    #[test]
    fn lightweight_identity_preserves_cross_path_duplicate_behavior() {
        let mut lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let mut duplicate = lexicon[0].clone();
        duplicate.frequency *= 2;
        duplicate.syllable_codes[1] = KeySequence::new("hj").unwrap();
        lexicon.push(duplicate);
        let decoder = Decoder::new(lexicon);
        let mut stats = SentenceSearchStats::default();
        let streaming = collapse_error_layers(
            &decoder.segment_transitions("nh", 0, true, false, None, &mut stats),
        );

        assert_eq!(
            streaming,
            decoder.segment_transitions_by_slices("nh", 0, true)
        );

        let log_frequency_total = decoder
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>()
            .ln();
        let expected = decoder.compact_unigram_lattice_transitions(
            streaming,
            0,
            true,
            false,
            1,
            log_frequency_total,
        );
        let bounded = decoder.segment_transitions(
            "nh",
            0,
            true,
            false,
            Some(1),
            &mut SentenceSearchStats::default(),
        );
        assert_eq!(bounded, expected);
    }

    #[test]
    fn k_best_sentence_paths_match_full_enumeration() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);
        let mut cases = decoder
            .lexicon
            .windows(2)
            .map(|pair| {
                format!(
                    "{}{}",
                    fully_abbreviated(&pair[0]),
                    fully_abbreviated(&pair[1])
                )
            })
            .collect::<BTreeSet<_>>();
        cases.extend(["ajjp", "zrmurf", "zrnurf"].into_iter().map(str::to_owned));

        for observed in cases {
            for top_k in [1, 5, 10, 25, 50] {
                assert_eq!(
                    decoder.decode_sentence(&observed, top_k).unwrap(),
                    exhaustive_sentence_reference(&decoder, &observed, top_k),
                    "k-best paths diverged for {observed}, K={top_k}"
                );
            }
        }

        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let model = BigramLanguageModel::from_tsv(BIGRAM_FIXTURE, &lexicon).unwrap();
        let decoder = Decoder::new(lexicon).with_bigram_model(model);
        for observed in ["ajjp", "zrmurf", "zrnurf"] {
            for top_k in [1, 5, 10, 25, 50] {
                assert_eq!(
                    decoder.decode_sentence(observed, top_k).unwrap(),
                    exhaustive_sentence_reference(&decoder, observed, top_k),
                    "bigram k-best paths diverged for {observed}, K={top_k}"
                );
            }
        }
    }

    #[test]
    fn diagnostic_frontier_retains_bounded_same_text_segmentations() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
不是\tbu shi\t1000\n\
第\tdi\t900\n\
一个\tyi ge\t900\n\
第一\tdi yi\t700\n\
个\tge\t700\n",
        )
        .unwrap();
        let decoder = Decoder::new(lexicon);

        let ordinary = decoder.decode_sentence("buuidiyige", 10).unwrap();
        assert_eq!(
            ordinary
                .iter()
                .filter(|candidate| candidate.text == "不是第一个")
                .count(),
            1
        );

        let variants = decoder
            .decode_sentence_segmentation_variants("buuidiyige", 10, 2)
            .unwrap();
        let segmentations = variants
            .iter()
            .filter(|candidate| candidate.text == "不是第一个")
            .map(|candidate| {
                candidate
                    .segments
                    .iter()
                    .map(|segment| segment.candidate.text.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect::<HashSet<_>>();

        assert_eq!(segmentations.len(), 2);
        assert!(segmentations.contains("不是|第|一个"));
        assert!(segmentations.contains("不是|第一|个"));
    }

    fn exhaustive_sentence_reference(
        decoder: &Decoder,
        observed: &str,
        top_k: usize,
    ) -> Vec<SentenceCandidate> {
        let mut stats = SentenceSearchStats::default();
        let lattice = decoder.build_unpruned_sentence_lattice(observed, &mut stats);
        let log_frequency_total = decoder
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>()
            .ln();
        let initial_state = SentenceRankingState {
            position: 0,
            used_error: false,
            previous_word: None,
        };
        let mut paths =
            enumerate_all_sentence_paths(decoder, &lattice, initial_state, log_frequency_total);
        paths.sort_by(sentence_order);
        let mut seen_text = HashSet::new();
        paths.retain(|candidate| seen_text.insert(candidate.text.clone()));
        paths.truncate(top_k);
        paths
    }

    fn enumerate_all_sentence_paths(
        decoder: &Decoder,
        lattice: &SentenceLattice,
        state: SentenceRankingState,
        log_frequency_total: f64,
    ) -> Vec<SentenceCandidate> {
        if state.position == lattice.length {
            return vec![SentenceCandidate {
                text: String::new(),
                segments: Vec::new(),
                total_score: 0.0,
                unresolved_key_count: 0,
                used_error: state.used_error,
            }];
        }

        let transitions = if state.used_error {
            lattice.outgoing[state.position]
                .iter()
                .filter(|transition| !transition.uses_error)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            collapse_error_layers(&lattice.outgoing[state.position])
        };
        let mut paths = Vec::new();
        for transition in transitions {
            let child_state = SentenceRankingState {
                position: transition.end,
                used_error: state.used_error || transition.uses_error,
                previous_word: match (decoder.language_model.as_ref(), transition.candidate.source)
                {
                    (Some(_), CandidateSource::Lexicon) => Some(transition.candidate.text.clone()),
                    _ => None,
                },
            };
            let suffixes =
                enumerate_all_sentence_paths(decoder, lattice, child_state, log_frequency_total);
            let language_score = decoder.sentence_language_score(
                state.previous_word.as_deref(),
                &transition.candidate,
                log_frequency_total,
            );
            let edge_score = language_score.interpolated_log_probability
                - transition.candidate.score.abbreviation_penalty
                - transition.candidate.score.correction_penalty
                - transition.candidate.score.unresolved_input_penalty;
            let unresolved_key_count =
                usize::from(transition.candidate.source == CandidateSource::UnresolvedInput);
            for suffix in suffixes {
                let mut text = transition.candidate.text.clone();
                text.push_str(&suffix.text);
                let mut segments = Vec::with_capacity(suffix.segments.len() + 1);
                segments.push(SentenceSegment {
                    observed: transition.observed.clone(),
                    candidate: transition.candidate.clone(),
                    language_score,
                });
                segments.extend(suffix.segments);
                paths.push(SentenceCandidate {
                    text,
                    segments,
                    total_score: edge_score + suffix.total_score,
                    unresolved_key_count: unresolved_key_count + suffix.unresolved_key_count,
                    used_error: suffix.used_error,
                });
            }
        }
        paths
    }

    fn fully_abbreviated(entry: &LexiconEntry) -> String {
        entry
            .syllable_codes
            .iter()
            .map(|code| code.as_str().as_bytes()[0] as char)
            .collect()
    }

    fn word_regression_cases(decoder: &Decoder) -> BTreeSet<String> {
        let mut cases = decoder
            .lexicon
            .iter()
            .flat_map(|entry| spelling_variants(&entry.syllable_codes))
            .map(|spelling| spelling.code.as_str().to_owned())
            .collect::<BTreeSet<_>>();

        for entry in &decoder.lexicon {
            let full_code = entry.code.as_str().as_bytes();
            for index in 0..full_code.len() {
                for actual in b'a'..=b'z' {
                    if are_qwerty_neighbors(full_code[index], actual) {
                        let mut observed = full_code.to_vec();
                        observed[index] = actual;
                        cases.insert(String::from_utf8(observed).unwrap());
                    }
                }
            }
            for start in 0..full_code.len().saturating_sub(1) {
                if full_code[start] != full_code[start + 1] {
                    let mut observed = full_code.to_vec();
                    observed.swap(start, start + 1);
                    cases.insert(String::from_utf8(observed).unwrap());
                }
            }
            for index in 0..full_code.len() {
                let mut observed = full_code.to_vec();
                observed.remove(index);
                cases.insert(String::from_utf8(observed).unwrap());
            }
            for gap in 0..=full_code.len() {
                let repeated_key = if gap < full_code.len() {
                    full_code[gap]
                } else {
                    full_code[full_code.len() - 1]
                };
                let mut observed = full_code.to_vec();
                observed.insert(gap, repeated_key);
                cases.insert(String::from_utf8(observed).unwrap());
            }
        }
        cases
    }

    fn hypothesis_reference(
        decoder: &Decoder,
        observed: &str,
        top_k: usize,
        allow_error: bool,
    ) -> Vec<Candidate> {
        let mut best_by_entry = HashMap::<usize, Candidate>::new();
        for hypothesis in key_hypotheses(observed, allow_error) {
            for terminal in decoder.trie.lookup(&hypothesis.code) {
                let candidate = decoder.make_candidate(
                    &decoder.lexicon[terminal.entry_index],
                    terminal.spelling,
                    hypothesis.correction.clone(),
                );
                match best_by_entry.entry(terminal.entry_index) {
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(candidate);
                    }
                    std::collections::hash_map::Entry::Occupied(mut slot) => {
                        if candidate_order(&candidate, slot.get()) == Ordering::Less {
                            slot.insert(candidate);
                        }
                    }
                }
            }
        }
        let mut candidates = best_by_entry.into_values().collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        candidates
    }

    fn exhaustive_reference(decoder: &Decoder, observed: &str, top_k: usize) -> Vec<Candidate> {
        let mut candidates = decoder
            .lexicon
            .iter()
            .filter_map(|entry| {
                spelling_variants(&entry.syllable_codes)
                    .into_iter()
                    .filter_map(|spelling| {
                        let correction = detect_correction(observed, spelling.code.as_str())?;
                        Some(decoder.make_candidate(entry, spelling, correction))
                    })
                    .min_by(candidate_order)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        candidates
    }
}
