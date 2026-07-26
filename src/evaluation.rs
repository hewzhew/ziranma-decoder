use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    BIGRAM_INTERPOLATION_WEIGHT, BigramLanguageModel, Candidate, CandidateSource,
    CharacterBigramLanguageModel, ContinuousCompositionProbe, Correction, Decoder, KeySequence,
    LexiconEntry, SentenceCandidate, are_qwerty_neighbors, spelling_variants,
};

/// Candidate per-key margins scanned by the rejection shadow evaluation.
///
/// A threshold accepts a fully lexicon-covered path when its normalized score
/// margin over fully literal fallback is at least this value.
pub const REJECTION_SHADOW_THRESHOLDS_PER_KEY: [f64; 9] =
    [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];

/// Deterministic synthetic case families generated from public lexicon entries.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SyntheticCaseKind {
    /// Unmodified canonical full code.
    Clean,
    /// An exact spelling with at least one one-key syllable abbreviation.
    MixedAbbreviation,
    /// One full-code key replaced by each of its QWERTY neighbors.
    NeighborSubstitution,
    /// One pair of distinct adjacent full-code keys reversed.
    AdjacentTransposition,
    /// One full-code key removed.
    MissingKey,
    /// One deterministic repeated key inserted at each full-code gap.
    ExtraKey,
    /// Two adjacent lexicon entries concatenated without a word boundary.
    TwoWordBoundary,
}

impl SyntheticCaseKind {
    /// Stable display label for reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::Clean => "干净全码",
            Self::MixedAbbreviation => "混合简拼",
            Self::NeighborSubstitution => "邻键替换",
            Self::AdjacentTransposition => "相邻颠倒",
            Self::MissingKey => "漏键",
            Self::ExtraKey => "多按",
            Self::TwoWordBoundary => "双词无界",
        }
    }
}

const CASE_KINDS: [SyntheticCaseKind; 7] = [
    SyntheticCaseKind::Clean,
    SyntheticCaseKind::MixedAbbreviation,
    SyntheticCaseKind::NeighborSubstitution,
    SyntheticCaseKind::AdjacentTransposition,
    SyntheticCaseKind::MissingKey,
    SyntheticCaseKind::ExtraKey,
    SyntheticCaseKind::TwoWordBoundary,
];

/// Recall counts for one synthetic case family.
#[derive(Clone, Debug, PartialEq)]
pub struct RecallMetrics {
    /// Case family.
    pub kind: SyntheticCaseKind,
    /// Number of generated cases.
    pub total: usize,
    /// Cases whose source entry appeared first.
    pub hits_at_1: usize,
    /// Cases whose source entry appeared in the first five.
    pub hits_at_5: usize,
    /// Cases whose source entry appeared in the first ten.
    pub hits_at_10: usize,
}

impl RecallMetrics {
    /// Recall@1 as a value from zero to one.
    pub fn recall_at_1(&self) -> f64 {
        rate(self.hits_at_1, self.total)
    }

    /// Recall@5 as a value from zero to one.
    pub fn recall_at_5(&self) -> f64 {
        rate(self.hits_at_5, self.total)
    }

    /// Recall@10 as a value from zero to one.
    pub fn recall_at_10(&self) -> f64 {
        rate(self.hits_at_10, self.total)
    }
}

/// Complete deterministic evaluation report.
#[derive(Clone, Debug, PartialEq)]
pub struct EvaluationReport {
    /// Metrics in a stable case-family order.
    pub metrics: Vec<RecallMetrics>,
    /// Clean cases whose first candidate required no correction.
    pub clean_top_1_exact: usize,
    /// Total number of clean cases.
    pub clean_total: usize,
}

impl EvaluationReport {
    /// Fraction of clean cases whose first candidate was an exact spelling.
    pub fn clean_top_1_exact_rate(&self) -> f64 {
        rate(self.clean_top_1_exact, self.clean_total)
    }

    /// Total cases across all families.
    pub fn total_cases(&self) -> usize {
        self.metrics.iter().map(|metrics| metrics.total).sum()
    }
}

/// Recall report for separately authored segmented sentence cases.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceCaseReport {
    /// Number of sentence cases.
    pub total: usize,
    /// Cases whose expected text appeared first.
    pub hits_at_1: usize,
    /// Cases whose expected text appeared in the first five.
    pub hits_at_5: usize,
    /// Cases whose expected text appeared in the first ten.
    pub hits_at_10: usize,
}

impl SentenceCaseReport {
    /// Recall@1 as a value from zero to one.
    pub fn recall_at_1(&self) -> f64 {
        rate(self.hits_at_1, self.total)
    }

    /// Recall@5 as a value from zero to one.
    pub fn recall_at_5(&self) -> f64 {
        rate(self.hits_at_5, self.total)
    }

    /// Recall@10 as a value from zero to one.
    pub fn recall_at_10(&self) -> f64 {
        rate(self.hits_at_10, self.total)
    }
}

/// Behavior of the top sentence candidate on words held outside the decoder.
#[derive(Clone, Debug, PartialEq)]
pub struct OovCaseReport {
    /// Number of held-out words.
    pub total: usize,
    /// Top candidates containing at least one explicit unresolved key.
    pub top_1_with_unresolved: usize,
    /// Top candidates retaining every observed key as unresolved.
    pub top_1_fully_unresolved: usize,
    /// Top candidates finding some full lexicon coverage despite the holdout.
    pub top_1_without_unresolved: usize,
    /// Unresolved keys summed across all top candidates.
    pub unresolved_keys: usize,
    /// Canonical observed keys summed across all held-out words.
    pub observed_keys: usize,
}

impl OovCaseReport {
    /// Fraction of cases whose top candidate exposes unresolved input.
    pub fn with_unresolved_rate(&self) -> f64 {
        rate(self.top_1_with_unresolved, self.total)
    }

    /// Fraction of cases represented entirely by literal fallback.
    pub fn fully_unresolved_rate(&self) -> f64 {
        rate(self.top_1_fully_unresolved, self.total)
    }

    /// Fraction of held-out keys retained as explicit unresolved input.
    pub fn unresolved_key_rate(&self) -> f64 {
        rate(self.unresolved_keys, self.observed_keys)
    }
}

/// One threshold row from the read-only rejection calibration probe.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectionThresholdMetrics {
    /// Minimum normalized lexicon-over-literal score margin required to accept.
    pub threshold_per_key: f64,
    /// Number of separately authored known sentences.
    pub known_total: usize,
    /// Known sentences that would retain a fully lexicon-covered result.
    pub known_accepted: usize,
    /// Number of independently authored held-out words.
    pub oov_total: usize,
    /// Held-out words that would use fully literal fallback.
    pub oov_rejected: usize,
}

impl RejectionThresholdMetrics {
    /// Fraction of known sentences that would retain a lexicon result.
    pub fn known_acceptance_rate(&self) -> f64 {
        rate(self.known_accepted, self.known_total)
    }

    /// Fraction of held-out words that would use fully literal fallback.
    pub fn oov_rejection_rate(&self) -> f64 {
        rate(self.oov_rejected, self.oov_total)
    }
}

/// Observed range of normalized margins among fully covered paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RejectionMarginRange {
    /// Lowest observed per-key margin.
    pub minimum_per_key: f64,
    /// Highest observed per-key margin.
    pub maximum_per_key: f64,
}

/// One public probe with independently sourced expected text and observed keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabeledSentenceProbe {
    /// Stable upstream-derived identifier.
    pub id: String,
    /// Deterministically constructed lowercase ASCII input keys.
    pub observed: KeySequence,
    /// Text that the top sentence candidate is expected to reproduce.
    pub expected_text: String,
    /// Rime word sequence used to construct the expected path.
    pub expected_segments: Vec<String>,
    /// Whether expected syllables use full codes or one-key abbreviations.
    pub spelling_mode: ProbeSpellingMode,
}

/// Top-K text recall against independently sourced public expectations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LabeledRecallReport {
    /// Number of public probes.
    pub total: usize,
    /// Expected texts ranked first.
    pub hits_at_1: usize,
    /// Expected texts present in the first five unique candidates.
    pub hits_at_5: usize,
    /// Expected texts present in the first ten unique candidates.
    pub hits_at_10: usize,
}

impl LabeledRecallReport {
    /// Fraction of expected texts ranked first.
    pub fn recall_at_1(&self) -> f64 {
        rate(self.hits_at_1, self.total)
    }

    /// Fraction of expected texts present in the first five candidates.
    pub fn recall_at_5(&self) -> f64 {
        rate(self.hits_at_5, self.total)
    }

    /// Fraction of expected texts present in the first ten candidates.
    pub fn recall_at_10(&self) -> f64 {
        rate(self.hits_at_10, self.total)
    }
}

/// Top-1/3/5/10 text recall for one continuous-composition candidate lane.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompositionRecallReport {
    /// Number of evaluated public phrases.
    pub total: usize,
    /// Expected texts ranked first.
    pub hits_at_1: usize,
    /// Expected texts present in the first three candidates.
    pub hits_at_3: usize,
    /// Expected texts present in the first five candidates.
    pub hits_at_5: usize,
    /// Expected texts present in the first ten candidates.
    pub hits_at_10: usize,
}

impl CompositionRecallReport {
    /// Recall@1 as a value from zero to one.
    pub fn recall_at_1(&self) -> f64 {
        rate(self.hits_at_1, self.total)
    }

    /// Recall@3 as a value from zero to one.
    pub fn recall_at_3(&self) -> f64 {
        rate(self.hits_at_3, self.total)
    }

    /// Recall@5 as a value from zero to one.
    pub fn recall_at_5(&self) -> f64 {
        rate(self.hits_at_5, self.total)
    }

    /// Recall@10 as a value from zero to one.
    pub fn recall_at_10(&self) -> f64 {
        rate(self.hits_at_10, self.total)
    }
}

/// Public short-phrase evaluation for continuous tail abbreviation and typo recovery.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContinuousCompositionReport {
    /// Full-code baseline in the ordinary primary list.
    pub full_code: CompositionRecallReport,
    /// Clean tail-abbreviated input in the ordinary primary list.
    pub tail_abbreviation: CompositionRecallReport,
    /// Clean tail-abbreviated rank after filtering the ordinary Top-10 by length.
    pub tail_abbreviation_same_length: CompositionRecallReport,
    /// Transposed input in the conservative primary list.
    pub transposed_primary: CompositionRecallReport,
    /// Transposed input in the anchored recovery lane.
    pub transposed_recovery: CompositionRecallReport,
    /// Full-code key count across all probes.
    pub full_keys: usize,
    /// Tail-abbreviated key count across all probes.
    pub tail_keys: usize,
}

impl ContinuousCompositionReport {
    /// Keys saved before any candidate-selection action.
    pub fn saved_keys(&self) -> usize {
        self.full_keys.saturating_sub(self.tail_keys)
    }

    /// Fraction of full-code keys removed by tail abbreviation.
    pub fn key_saving_rate(&self) -> f64 {
        rate(self.saved_keys(), self.full_keys)
    }
}

/// One tail-abbreviation case that missed the ordinary visible candidate list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousCompositionAuditCase {
    /// Stable upstream-derived probe identifier.
    pub id: String,
    /// Continuous tail-abbreviated keys.
    pub observed: String,
    /// Natural public text expected from those keys.
    pub expected_text: String,
    /// Ordinary unigram first candidate.
    pub baseline_top_text: String,
    /// Word segmentation used by the ordinary first candidate.
    pub baseline_top_segments: Vec<String>,
    /// One-based ordinary rank inside the deeper audit pool, if visible.
    pub baseline_rank: Option<usize>,
    /// One-based rank after train-only word-bigram rescoring of the same pool.
    pub word_context_rank: Option<usize>,
    /// One-based rank after pure average character-bigram rescoring of the pool.
    pub character_average_rank: Option<usize>,
}

/// Read-only failure audit over a wider, unchanged unigram candidate pool.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContinuousCompositionAuditReport {
    /// Number of public phrases examined.
    pub total: usize,
    /// Size of the ordinary user-visible list being audited.
    pub baseline_k: usize,
    /// Maximum ordinary unigram rank inspected for failures.
    pub audit_depth: usize,
    /// Expected texts already visible in the ordinary first `baseline_k`.
    pub baseline_visible: usize,
    /// Misses from the visible list found later in the audit pool.
    pub deeper_visible: usize,
    /// Misses still absent at `audit_depth`.
    pub outside_audit_depth: usize,
    /// Failure rows whose ordinary first candidate has fewer characters.
    pub baseline_top_shorter: usize,
    /// Failure rows whose ordinary first candidate has the same character count.
    pub baseline_top_same_length: usize,
    /// Failure rows whose ordinary first candidate has more characters.
    pub baseline_top_longer: usize,
    /// Visible failures reranked first by train-only word context.
    pub word_context_reranked_at_1: usize,
    /// Visible failures reranked into the first `baseline_k` by word context.
    pub word_context_reranked_visible: usize,
    /// Visible failures reranked first by average train-only character context.
    pub character_average_reranked_at_1: usize,
    /// Visible failures reranked into the first `baseline_k` by character context.
    pub character_average_reranked_visible: usize,
    /// Per-case evidence for every ordinary visible-list miss.
    pub failures: Vec<ContinuousCompositionAuditCase>,
}

/// Audits tail-abbreviation misses without changing production search or ranking.
///
/// The ordinary first `baseline_k` candidates remain the reference. Only its
/// misses receive a deeper unigram pool. Two train-only models then rescore
/// exactly that frozen pool, so this diagnostic cannot manufacture a path that
/// the decoder did not already expose.
pub fn audit_continuous_composition(
    decoder: &Decoder,
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    lexicon: &[LexiconEntry],
    probes: &[ContinuousCompositionProbe],
    baseline_k: usize,
    audit_depth: usize,
) -> ContinuousCompositionAuditReport {
    let baseline_k = baseline_k.max(1);
    let audit_depth = audit_depth.max(baseline_k);
    let frequency_total = lexicon
        .iter()
        .map(|entry| entry.frequency as f64)
        .sum::<f64>();
    let log_frequency_total = if frequency_total > 0.0 {
        frequency_total.ln()
    } else {
        0.0
    };
    let mut report = ContinuousCompositionAuditReport {
        total: probes.len(),
        baseline_k,
        audit_depth,
        ..ContinuousCompositionAuditReport::default()
    };

    for probe in probes {
        let baseline = decoder
            .decode_sentence(probe.tail_abbreviated_observed.as_str(), baseline_k)
            .expect("public probe keys are validated lowercase ASCII");
        if baseline
            .iter()
            .any(|candidate| candidate.text == probe.expected_text)
        {
            report.baseline_visible += 1;
            continue;
        }

        let top = baseline
            .first()
            .expect("literal fallback guarantees a sentence candidate");
        match top
            .text
            .chars()
            .count()
            .cmp(&probe.expected_text.chars().count())
        {
            std::cmp::Ordering::Less => report.baseline_top_shorter += 1,
            std::cmp::Ordering::Equal => report.baseline_top_same_length += 1,
            std::cmp::Ordering::Greater => report.baseline_top_longer += 1,
        }

        let pool = if audit_depth == baseline_k {
            baseline.clone()
        } else {
            decoder
                .decode_sentence(probe.tail_abbreviated_observed.as_str(), audit_depth)
                .expect("public probe keys are validated lowercase ASCII")
        };
        debug_assert_eq!(
            baseline
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            pool.iter()
                .take(baseline.len())
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            "increasing Top-K must preserve the ordinary candidate prefix"
        );
        let baseline_rank = text_rank(&pool, &probe.expected_text);
        if baseline_rank.is_some() {
            report.deeper_visible += 1;
        } else {
            report.outside_audit_depth += 1;
        }

        let word_context_rank = reranked_text_rank(&pool, &probe.expected_text, |candidate| {
            score_candidate_with_context(candidate, word_language_model, log_frequency_total)
        });
        let character_average_rank = reranked_text_rank(&pool, &probe.expected_text, |candidate| {
            let evidence = character_language_model.score_text(&candidate.text);
            evidence.log_probability / evidence.pair_count as f64
        });
        report.word_context_reranked_at_1 += usize::from(word_context_rank == Some(1));
        report.word_context_reranked_visible +=
            usize::from(word_context_rank.is_some_and(|rank| rank <= baseline_k));
        report.character_average_reranked_at_1 += usize::from(character_average_rank == Some(1));
        report.character_average_reranked_visible +=
            usize::from(character_average_rank.is_some_and(|rank| rank <= baseline_k));
        report.failures.push(ContinuousCompositionAuditCase {
            id: probe.id.clone(),
            observed: probe.tail_abbreviated_observed.as_str().to_owned(),
            expected_text: probe.expected_text.clone(),
            baseline_top_text: top.text.clone(),
            baseline_top_segments: top
                .segments
                .iter()
                .map(|segment| segment.candidate.text.clone())
                .collect(),
            baseline_rank,
            word_context_rank,
            character_average_rank,
        });
    }
    report
}

fn text_rank(candidates: &[SentenceCandidate], expected_text: &str) -> Option<usize> {
    candidates
        .iter()
        .position(|candidate| candidate.text == expected_text)
        .map(|rank| rank + 1)
}

fn reranked_text_rank(
    candidates: &[SentenceCandidate],
    expected_text: &str,
    mut score: impl FnMut(&SentenceCandidate) -> f64,
) -> Option<usize> {
    let mut scored = candidates
        .iter()
        .enumerate()
        .map(|(baseline_rank, candidate)| (baseline_rank, candidate, score(candidate)))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored
        .iter()
        .position(|(_, candidate, _)| candidate.text == expected_text)
        .map(|rank| rank + 1)
}

/// Evaluates natural two-word continuous input and one adjacent transposition.
pub fn evaluate_continuous_composition(
    decoder: &Decoder,
    probes: &[ContinuousCompositionProbe],
) -> ContinuousCompositionReport {
    let mut report = ContinuousCompositionReport::default();
    for probe in probes {
        let full = decoder
            .decode_sentence(probe.full_observed.as_str(), 10)
            .expect("public probe keys are validated lowercase ASCII");
        observe_composition_recall(&mut report.full_code, &probe.expected_text, &full);

        let tail = decoder
            .decode_sentence(probe.tail_abbreviated_observed.as_str(), 10)
            .expect("public probe keys are validated lowercase ASCII");
        observe_composition_recall(&mut report.tail_abbreviation, &probe.expected_text, &tail);
        let expected_length = probe.expected_text.chars().count();
        let same_length = tail
            .iter()
            .filter(|candidate| candidate.text.chars().count() == expected_length)
            .cloned()
            .collect::<Vec<_>>();
        observe_composition_recall(
            &mut report.tail_abbreviation_same_length,
            &probe.expected_text,
            &same_length,
        );

        let lanes = decoder
            .decode_sentence_lanes(probe.transposed_observed.as_str(), 10)
            .expect("public probe keys are validated lowercase ASCII");
        observe_composition_recall(
            &mut report.transposed_primary,
            &probe.expected_text,
            &lanes.primary,
        );
        observe_composition_recall(
            &mut report.transposed_recovery,
            &probe.expected_text,
            &lanes.anchored_transposition_recovery,
        );
        report.full_keys += probe.full_observed.as_str().len();
        report.tail_keys += probe.tail_abbreviated_observed.as_str().len();
    }
    report
}

fn observe_composition_recall(
    report: &mut CompositionRecallReport,
    expected_text: &str,
    candidates: &[SentenceCandidate],
) {
    let rank = candidates
        .iter()
        .position(|candidate| candidate.text == expected_text);
    report.total += 1;
    report.hits_at_1 += usize::from(rank.is_some_and(|rank| rank < 1));
    report.hits_at_3 += usize::from(rank.is_some_and(|rank| rank < 3));
    report.hits_at_5 += usize::from(rank.is_some_and(|rank| rank < 5));
    report.hits_at_10 += usize::from(rank.is_some_and(|rank| rank < 10));
}

/// Measures whether public expected text is recalled before changing ranking.
///
/// The production unigram decoder and its ordering remain unchanged. This
/// separates failures where the expected text is absent from the visible
/// candidate set from failures where it is present but ranked too low.
pub fn evaluate_labeled_recall(
    decoder: &Decoder,
    probes: &[LabeledSentenceProbe],
) -> LabeledRecallReport {
    let mut report = LabeledRecallReport::default();
    for probe in probes {
        let candidates = decoder
            .decode_sentence(probe.observed.as_str(), 10)
            .expect("public probe keys are validated lowercase ASCII");
        let rank = candidates
            .iter()
            .position(|candidate| candidate.text == probe.expected_text);
        report.total += 1;
        if rank.is_some_and(|rank| rank < 1) {
            report.hits_at_1 += 1;
        }
        if rank.is_some_and(|rank| rank < 5) {
            report.hits_at_5 += 1;
        }
        if rank.is_some_and(|rank| rank < 10) {
            report.hits_at_10 += 1;
        }
    }
    report
}

/// Uniform spelling mode used to construct one public probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeSpellingMode {
    /// Every syllable uses its canonical two-key code.
    FullCode,
    /// Every syllable uses only the first key of its canonical code.
    FullyAbbreviated,
}

/// Range of expected-minus-baseline context score margins per observed key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextScoreMarginRange {
    /// Lowest observed normalized margin.
    pub minimum_per_key: f64,
    /// Highest observed normalized margin.
    pub maximum_per_key: f64,
}

/// Two-path diagnostic for a train-only public context model.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextOracleReport {
    /// Number of held-out public probes.
    pub total: usize,
    /// Probes already correct under the unmodified unigram Top-1.
    pub unigram_top_1_matches_expected: usize,
    /// Probes incorrect under the unmodified unigram Top-1.
    pub unigram_top_1_differs: usize,
    /// Incorrect probes whose expected path scores above the unigram Top-1
    /// after applying public bigram evidence to both paths.
    pub incorrect_expected_path_preferred: usize,
    /// Incorrect probes whose two context scores are exactly equal.
    pub incorrect_context_ties: usize,
    /// Incorrect probes whose original Top-1 still scores above expectation.
    pub incorrect_baseline_path_preferred: usize,
    /// Correct results plus incorrect results repaired by the two-path oracle.
    pub oracle_pair_matches_expected: usize,
    /// Margin range among originally incorrect probes.
    pub incorrect_margin_range: Option<ContextScoreMarginRange>,
}

impl ContextOracleReport {
    /// Fraction correct when choosing only between expected and unigram Top-1.
    pub fn oracle_pair_accuracy(&self) -> f64 {
        rate(self.oracle_pair_matches_expected, self.total)
    }
}

/// Pairwise text diagnostic for a train-only character bigram model.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterContextOracleReport {
    /// Number of held-out public probes.
    pub total: usize,
    /// Probes already correct under the unmodified unigram Top-1.
    pub unigram_top_1_matches_expected: usize,
    /// Probes incorrect under the unmodified unigram Top-1.
    pub unigram_top_1_differs: usize,
    /// Incorrect probes whose expected text receives the higher character
    /// language score.
    pub incorrect_expected_text_preferred: usize,
    /// Incorrect probes whose two character scores are exactly equal.
    pub incorrect_context_ties: usize,
    /// Incorrect probes whose original Top-1 receives the higher score.
    pub incorrect_baseline_text_preferred: usize,
    /// Correct results plus incorrect results repaired by the text oracle.
    pub oracle_pair_matches_expected: usize,
    /// Expected-minus-baseline character score range per observed key.
    pub incorrect_margin_range: Option<ContextScoreMarginRange>,
    /// Incorrect probes whose expected text has more characters.
    pub incorrect_expected_text_longer: usize,
    /// Incorrect probes whose two texts have the same character count.
    pub incorrect_equal_text_length: usize,
    /// Incorrect probes whose expected text has fewer characters.
    pub incorrect_expected_text_shorter: usize,
    /// Equal-length incorrect probes whose expected text scores higher.
    pub incorrect_equal_length_expected_preferred: usize,
    /// Equal-length incorrect probes whose character scores tie.
    pub incorrect_equal_length_ties: usize,
    /// Equal-length incorrect probes whose original Top-1 scores higher.
    pub incorrect_equal_length_baseline_preferred: usize,
    /// Incorrect probes whose expected text has the higher average
    /// log-probability per character transition.
    pub incorrect_average_expected_preferred: usize,
    /// Incorrect probes tied after character-length normalization.
    pub incorrect_average_context_ties: usize,
    /// Incorrect probes whose original Top-1 has the higher normalized score.
    pub incorrect_average_baseline_preferred: usize,
    /// Correct results plus normalized-score repairs.
    pub average_oracle_pair_matches_expected: usize,
    /// Expected-minus-baseline average-score range.
    pub incorrect_average_margin_range: Option<CharacterAverageMarginRange>,
}

impl CharacterContextOracleReport {
    /// Fraction correct under the optimistic pairwise text oracle.
    pub fn oracle_pair_accuracy(&self) -> f64 {
        rate(self.oracle_pair_matches_expected, self.total)
    }

    /// Fraction correct under the character-length-normalized pairwise oracle.
    pub fn average_oracle_pair_accuracy(&self) -> f64 {
        rate(self.average_oracle_pair_matches_expected, self.total)
    }
}

/// Range of average character log-score differences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterAverageMarginRange {
    /// Lowest expected-minus-baseline average log score.
    pub minimum: f64,
    /// Highest expected-minus-baseline average log score.
    pub maximum: f64,
}

/// Compares expected public text with unigram Top-1 using character bigrams.
///
/// This optimistic diagnostic ignores decoder and spelling scores. It asks
/// only whether denser train-only character evidence distinguishes the two
/// known texts before any production search integration is attempted.
pub fn evaluate_character_context_oracle(
    decoder: &Decoder,
    language_model: &CharacterBigramLanguageModel,
    probes: &[LabeledSentenceProbe],
) -> CharacterContextOracleReport {
    let mut unigram_top_1_matches_expected = 0;
    let mut incorrect_expected_text_preferred = 0;
    let mut incorrect_context_ties = 0;
    let mut incorrect_baseline_text_preferred = 0;
    let mut incorrect_margins = Vec::new();
    let mut incorrect_expected_text_longer = 0;
    let mut incorrect_equal_text_length = 0;
    let mut incorrect_expected_text_shorter = 0;
    let mut incorrect_equal_length_expected_preferred = 0;
    let mut incorrect_equal_length_ties = 0;
    let mut incorrect_equal_length_baseline_preferred = 0;
    let mut incorrect_average_expected_preferred = 0;
    let mut incorrect_average_context_ties = 0;
    let mut incorrect_average_baseline_preferred = 0;
    let mut incorrect_average_margins = Vec::new();

    for probe in probes {
        let candidate = decoder
            .decode_sentence(probe.observed.as_str(), 1)
            .expect("public probe keys are validated lowercase ASCII")
            .into_iter()
            .next()
            .expect("literal fallback guarantees a sentence candidate");
        if candidate.text == probe.expected_text {
            unigram_top_1_matches_expected += 1;
            continue;
        }

        let expected_evidence = language_model.score_text(&probe.expected_text);
        let baseline_evidence = language_model.score_text(&candidate.text);
        let expected_score = expected_evidence.log_probability;
        let baseline_score = baseline_evidence.log_probability;
        let margin_per_key =
            (expected_score - baseline_score) / probe.observed.as_str().len() as f64;
        incorrect_margins.push(margin_per_key);
        match expected_score.total_cmp(&baseline_score) {
            std::cmp::Ordering::Greater => incorrect_expected_text_preferred += 1,
            std::cmp::Ordering::Equal => incorrect_context_ties += 1,
            std::cmp::Ordering::Less => incorrect_baseline_text_preferred += 1,
        }
        let length_ordering = probe
            .expected_text
            .chars()
            .count()
            .cmp(&candidate.text.chars().count());
        match length_ordering {
            std::cmp::Ordering::Greater => incorrect_expected_text_longer += 1,
            std::cmp::Ordering::Equal => {
                incorrect_equal_text_length += 1;
                match expected_score.total_cmp(&baseline_score) {
                    std::cmp::Ordering::Greater => {
                        incorrect_equal_length_expected_preferred += 1;
                    }
                    std::cmp::Ordering::Equal => incorrect_equal_length_ties += 1,
                    std::cmp::Ordering::Less => {
                        incorrect_equal_length_baseline_preferred += 1;
                    }
                }
            }
            std::cmp::Ordering::Less => incorrect_expected_text_shorter += 1,
        }
        let expected_average =
            expected_evidence.log_probability / expected_evidence.pair_count as f64;
        let baseline_average =
            baseline_evidence.log_probability / baseline_evidence.pair_count as f64;
        incorrect_average_margins.push(expected_average - baseline_average);
        match expected_average.total_cmp(&baseline_average) {
            std::cmp::Ordering::Greater => incorrect_average_expected_preferred += 1,
            std::cmp::Ordering::Equal => incorrect_average_context_ties += 1,
            std::cmp::Ordering::Less => incorrect_average_baseline_preferred += 1,
        }
    }

    let unigram_top_1_differs = probes.len() - unigram_top_1_matches_expected;
    CharacterContextOracleReport {
        total: probes.len(),
        unigram_top_1_matches_expected,
        unigram_top_1_differs,
        incorrect_expected_text_preferred,
        incorrect_context_ties,
        incorrect_baseline_text_preferred,
        oracle_pair_matches_expected: unigram_top_1_matches_expected
            + incorrect_expected_text_preferred,
        incorrect_margin_range: context_margin_range(&incorrect_margins),
        incorrect_expected_text_longer,
        incorrect_equal_text_length,
        incorrect_expected_text_shorter,
        incorrect_equal_length_expected_preferred,
        incorrect_equal_length_ties,
        incorrect_equal_length_baseline_preferred,
        incorrect_average_expected_preferred,
        incorrect_average_context_ties,
        incorrect_average_baseline_preferred,
        average_oracle_pair_matches_expected: unigram_top_1_matches_expected
            + incorrect_average_expected_preferred,
        incorrect_average_margin_range: character_average_margin_range(&incorrect_average_margins),
    }
}

/// Compares the expected public path with the current unigram Top-1 path.
///
/// Both fixed paths are rescored with the supplied train-only bigram model and
/// the decoder's existing 35/65 interpolation. This is an oracle diagnostic:
/// it measures whether context evidence prefers the known expected path, but
/// it does not insert that path into production search or alter any ranking.
pub fn evaluate_context_oracle(
    decoder: &Decoder,
    language_model: &BigramLanguageModel,
    lexicon: &[LexiconEntry],
    probes: &[LabeledSentenceProbe],
) -> Result<ContextOracleReport, ContextOracleError> {
    let frequency_total = lexicon
        .iter()
        .map(|entry| entry.frequency as f64)
        .sum::<f64>();
    let log_frequency_total = if frequency_total > 0.0 {
        frequency_total.ln()
    } else {
        0.0
    };
    let entries_by_text = best_evaluation_entries_by_text(lexicon);
    let mut unigram_top_1_matches_expected = 0;
    let mut incorrect_expected_path_preferred = 0;
    let mut incorrect_context_ties = 0;
    let mut incorrect_baseline_path_preferred = 0;
    let mut incorrect_margins = Vec::new();

    for probe in probes {
        if probe.expected_segments.is_empty() {
            return Err(ContextOracleError::EmptyExpectedPath {
                probe_id: probe.id.clone(),
            });
        }
        let candidate = decoder
            .decode_sentence(probe.observed.as_str(), 1)
            .expect("public probe keys are validated lowercase ASCII")
            .into_iter()
            .next()
            .expect("literal fallback guarantees a sentence candidate");
        if candidate.text == probe.expected_text {
            unigram_top_1_matches_expected += 1;
            continue;
        }

        let expected_score = score_expected_context_path(
            probe,
            language_model,
            &entries_by_text,
            log_frequency_total,
            decoder.config.abbreviation_penalty_per_syllable,
        )?;
        let baseline_score =
            score_candidate_with_context(&candidate, language_model, log_frequency_total);
        let margin_per_key =
            (expected_score - baseline_score) / probe.observed.as_str().len() as f64;
        incorrect_margins.push(margin_per_key);
        match expected_score.total_cmp(&baseline_score) {
            std::cmp::Ordering::Greater => incorrect_expected_path_preferred += 1,
            std::cmp::Ordering::Equal => incorrect_context_ties += 1,
            std::cmp::Ordering::Less => incorrect_baseline_path_preferred += 1,
        }
    }

    let unigram_top_1_differs = probes.len() - unigram_top_1_matches_expected;
    Ok(ContextOracleReport {
        total: probes.len(),
        unigram_top_1_matches_expected,
        unigram_top_1_differs,
        incorrect_expected_path_preferred,
        incorrect_context_ties,
        incorrect_baseline_path_preferred,
        oracle_pair_matches_expected: unigram_top_1_matches_expected
            + incorrect_expected_path_preferred,
        incorrect_margin_range: context_margin_range(&incorrect_margins),
    })
}

fn score_expected_context_path(
    probe: &LabeledSentenceProbe,
    language_model: &BigramLanguageModel,
    entries_by_text: &HashMap<&str, &LexiconEntry>,
    log_frequency_total: f64,
    abbreviation_penalty_per_syllable: f64,
) -> Result<f64, ContextOracleError> {
    let mut total_score = 0.0;
    let mut previous_word = None::<&str>;
    for word in &probe.expected_segments {
        let entry = entries_by_text.get(word.as_str()).ok_or_else(|| {
            ContextOracleError::UnknownExpectedSegment {
                probe_id: probe.id.clone(),
                segment: word.clone(),
            }
        })?;
        let unigram = (entry.frequency as f64).ln() - log_frequency_total;
        let language_score = previous_word.map_or(unigram, |previous| {
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram
                + BIGRAM_INTERPOLATION_WEIGHT
                    * language_model.score(previous, &entry.text).log_probability
        });
        let abbreviation_penalty = match probe.spelling_mode {
            ProbeSpellingMode::FullCode => 0.0,
            ProbeSpellingMode::FullyAbbreviated => {
                entry.syllable_codes.len() as f64 * abbreviation_penalty_per_syllable
            }
        };
        total_score += language_score - abbreviation_penalty;
        previous_word = Some(&entry.text);
    }
    Ok(total_score)
}

fn score_candidate_with_context(
    candidate: &SentenceCandidate,
    language_model: &BigramLanguageModel,
    log_frequency_total: f64,
) -> f64 {
    let mut total_score = 0.0;
    let mut previous_word = None::<&str>;
    for segment in &candidate.segments {
        if segment.candidate.source == CandidateSource::UnresolvedInput {
            total_score -= segment.candidate.score.unresolved_input_penalty;
            previous_word = None;
            continue;
        }
        let unigram = segment.candidate.score.frequency - log_frequency_total;
        let language_score = previous_word.map_or(unigram, |previous| {
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram
                + BIGRAM_INTERPOLATION_WEIGHT
                    * language_model
                        .score(previous, &segment.candidate.text)
                        .log_probability
        });
        total_score += language_score
            - segment.candidate.score.abbreviation_penalty
            - segment.candidate.score.correction_penalty;
        previous_word = Some(&segment.candidate.text);
    }
    total_score
}

fn best_evaluation_entries_by_text(lexicon: &[LexiconEntry]) -> HashMap<&str, &LexiconEntry> {
    let mut entries = HashMap::<&str, &LexiconEntry>::new();
    for entry in lexicon {
        match entries.get(entry.text.as_str()) {
            Some(current)
                if entry.frequency < current.frequency
                    || (entry.frequency == current.frequency
                        && (entry.pinyin.as_str(), entry.code.as_str())
                            >= (current.pinyin.as_str(), current.code.as_str())) => {}
            _ => {
                entries.insert(entry.text.as_str(), entry);
            }
        }
    }
    entries
}

fn context_margin_range(margins: &[f64]) -> Option<ContextScoreMarginRange> {
    let mut margins = margins.iter().copied();
    let first = margins.next()?;
    Some(margins.fold(
        ContextScoreMarginRange {
            minimum_per_key: first,
            maximum_per_key: first,
        },
        |range, margin| ContextScoreMarginRange {
            minimum_per_key: range.minimum_per_key.min(margin),
            maximum_per_key: range.maximum_per_key.max(margin),
        },
    ))
}

fn character_average_margin_range(margins: &[f64]) -> Option<CharacterAverageMarginRange> {
    let mut margins = margins.iter().copied();
    let first = margins.next()?;
    Some(margins.fold(
        CharacterAverageMarginRange {
            minimum: first,
            maximum: first,
        },
        |range, margin| CharacterAverageMarginRange {
            minimum: range.minimum.min(margin),
            maximum: range.maximum.max(margin),
        },
    ))
}

/// Error returned when a public oracle probe cannot be mapped to the lexicon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextOracleError {
    /// A probe did not record any expected Rime word.
    EmptyExpectedPath {
        /// Stable probe identifier.
        probe_id: String,
    },
    /// An expected word was absent from the supplied lexicon.
    UnknownExpectedSegment {
        /// Stable probe identifier.
        probe_id: String,
        /// Missing word.
        segment: String,
    },
}

impl fmt::Display for ContextOracleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyExpectedPath { probe_id } => {
                write!(formatter, "公开探针 {probe_id:?} 没有预期词路径")
            }
            Self::UnknownExpectedSegment { probe_id, segment } => write!(
                formatter,
                "公开探针 {probe_id:?} 的预期词 {segment:?} 不在词典中"
            ),
        }
    }
}

impl Error for ContextOracleError {}

/// One threshold row split by whether the unmodified Top-1 text was correct.
#[derive(Clone, Debug, PartialEq)]
pub struct LabeledRejectionThresholdMetrics {
    /// Minimum normalized lexicon-over-literal score margin required to accept.
    pub threshold_per_key: f64,
    /// Probes whose unmodified Top-1 text matched the public source text.
    pub correct_total: usize,
    /// Correct Top-1 results that the hypothetical threshold would retain.
    pub correct_accepted: usize,
    /// Probes whose unmodified Top-1 text differed from the public source text.
    pub incorrect_total: usize,
    /// Incorrect Top-1 results that the hypothetical threshold would reject.
    pub incorrect_rejected: usize,
}

impl LabeledRejectionThresholdMetrics {
    /// Fraction of correct unmodified results retained by the threshold.
    pub fn correct_acceptance_rate(&self) -> f64 {
        rate(self.correct_accepted, self.correct_total)
    }

    /// Fraction of incorrect unmodified results rejected by the threshold.
    pub fn incorrect_rejection_rate(&self) -> f64 {
        rate(self.incorrect_rejected, self.incorrect_total)
    }
}

/// Read-only threshold scan labeled by actual Top-1 text equality.
#[derive(Clone, Debug, PartialEq)]
pub struct LabeledRejectionShadowReport {
    /// Number of public probes.
    pub total: usize,
    /// Probes whose unmodified Top-1 text matched the expected text.
    pub top_1_matches_expected: usize,
    /// Probes whose unmodified Top-1 text differed from the expected text.
    pub top_1_differs: usize,
    /// Incorrect Top-1 results that nevertheless had full lexicon coverage.
    pub incorrect_with_full_coverage: usize,
    /// Margin range among correct Top-1 results with full coverage.
    pub correct_margin_range: Option<RejectionMarginRange>,
    /// Margin range among incorrect Top-1 results with full coverage.
    pub incorrect_margin_range: Option<RejectionMarginRange>,
    /// Fixed threshold scan in ascending order.
    pub thresholds: Vec<LabeledRejectionThresholdMetrics>,
}

/// Evaluates hypothetical rejection against independently sourced expected text.
///
/// The decoder is run exactly once per probe. Its unmodified Top-1 text labels
/// that probe as correct or incorrect, while the same normalized
/// lexicon-over-literal margin used by [`evaluate_rejection_shadow`] determines
/// whether each fixed threshold would accept or reject it. No candidate or
/// ranking behavior is changed.
pub fn evaluate_labeled_rejection_shadow(
    decoder: &Decoder,
    probes: &[LabeledSentenceProbe],
) -> LabeledRejectionShadowReport {
    let observations = probes
        .iter()
        .map(|probe| {
            let (candidate, margin) = top_sentence_and_margin(decoder, probe.observed.as_str());
            (candidate.text == probe.expected_text, margin)
        })
        .collect::<Vec<_>>();
    let correct_total = observations
        .iter()
        .filter(|(correct, _margin)| *correct)
        .count();
    let incorrect_total = observations.len() - correct_total;
    let correct_margins = observations
        .iter()
        .filter_map(|(correct, margin)| correct.then_some(*margin).flatten())
        .map(Some)
        .collect::<Vec<_>>();
    let incorrect_margins = observations
        .iter()
        .filter_map(|(correct, margin)| (!correct).then_some(*margin).flatten())
        .map(Some)
        .collect::<Vec<_>>();
    let incorrect_with_full_coverage = incorrect_margins.len();
    let thresholds = REJECTION_SHADOW_THRESHOLDS_PER_KEY
        .into_iter()
        .map(|threshold_per_key| LabeledRejectionThresholdMetrics {
            threshold_per_key,
            correct_total,
            correct_accepted: observations
                .iter()
                .filter(|(correct, margin)| {
                    *correct && margin.is_some_and(|margin| margin >= threshold_per_key)
                })
                .count(),
            incorrect_total,
            incorrect_rejected: observations
                .iter()
                .filter(|(correct, margin)| {
                    !*correct && margin.is_none_or(|margin| margin < threshold_per_key)
                })
                .count(),
        })
        .collect();

    LabeledRejectionShadowReport {
        total: observations.len(),
        top_1_matches_expected: correct_total,
        top_1_differs: incorrect_total,
        incorrect_with_full_coverage,
        correct_margin_range: rejection_margin_range(&correct_margins),
        incorrect_margin_range: rejection_margin_range(&incorrect_margins),
        thresholds,
    }
}

/// Read-only calibration report comparing lexicon coverage with literal fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct RejectionShadowReport {
    /// Number of separately authored known sentences.
    pub known_total: usize,
    /// Known sentences for which a fully lexicon-covered path exists.
    pub known_with_full_coverage: usize,
    /// Margin range among known sentences with full coverage.
    pub known_margin_range: Option<RejectionMarginRange>,
    /// Number of independently authored held-out words.
    pub oov_total: usize,
    /// Held-out words for which a fully lexicon-covered path exists.
    pub oov_with_full_coverage: usize,
    /// Margin range among held-out words with full coverage.
    pub oov_margin_range: Option<RejectionMarginRange>,
    /// Fixed threshold scan in ascending order.
    pub thresholds: Vec<RejectionThresholdMetrics>,
}

/// Compares the best fully covered sentence path with fully literal fallback.
///
/// For each non-empty observed key sequence, the shadow signal is:
///
/// `(best fully covered score - fully literal score) / observed key count`
///
/// where fully literal score is the configured unresolved-key penalty applied
/// once per key. A threshold accepts lexicon coverage when the signal is at
/// least the threshold; inputs without a fully covered path are rejected. This
/// function only reports hypothetical decisions and does not change decoding.
pub fn evaluate_rejection_shadow(
    decoder: &Decoder,
    lexicon: &[LexiconEntry],
    known_sentence_sets: &[&str],
    oov_cases: &[LexiconEntry],
) -> Result<RejectionShadowReport, SentenceCaseParseError> {
    let mut known_margins = Vec::new();
    for contents in known_sentence_sets {
        let cases = parse_sentence_cases(lexicon, contents)?;
        known_margins.extend(
            cases
                .iter()
                .map(|case| full_lexicon_margin_per_key(decoder, &case.observed)),
        );
    }
    let oov_margins = oov_cases
        .iter()
        .map(|case| full_lexicon_margin_per_key(decoder, case.code.as_str()))
        .collect::<Vec<_>>();

    let known_total = known_margins.len();
    let known_with_full_coverage = known_margins.iter().flatten().count();
    let known_margin_range = rejection_margin_range(&known_margins);
    let oov_total = oov_margins.len();
    let oov_with_full_coverage = oov_margins.iter().flatten().count();
    let oov_margin_range = rejection_margin_range(&oov_margins);
    let thresholds = REJECTION_SHADOW_THRESHOLDS_PER_KEY
        .into_iter()
        .map(|threshold_per_key| RejectionThresholdMetrics {
            threshold_per_key,
            known_total,
            known_accepted: known_margins
                .iter()
                .flatten()
                .filter(|margin| **margin >= threshold_per_key)
                .count(),
            oov_total,
            oov_rejected: oov_margins
                .iter()
                .filter(|margin| margin.is_none_or(|margin| margin < threshold_per_key))
                .count(),
        })
        .collect();

    Ok(RejectionShadowReport {
        known_total,
        known_with_full_coverage,
        known_margin_range,
        oov_total,
        oov_with_full_coverage,
        oov_margin_range,
        thresholds,
    })
}

/// Evaluates canonical codes for words deliberately absent from the decoder.
///
/// The held-out entries should be authored separately and parsed through the
/// shared pinyin codec. This is a refusal/fallback probe, not a word-recall
/// metric: other lexicon entries may legitimately share or segment the same
/// code.
pub fn evaluate_oov_cases(decoder: &Decoder, cases: &[LexiconEntry]) -> OovCaseReport {
    let mut report = OovCaseReport {
        total: 0,
        top_1_with_unresolved: 0,
        top_1_fully_unresolved: 0,
        top_1_without_unresolved: 0,
        unresolved_keys: 0,
        observed_keys: 0,
    };
    for case in cases {
        let observed_keys = case.code.as_str().len();
        let candidate = decoder
            .decode_sentence(case.code.as_str(), 1)
            .expect("held-out canonical codes are lowercase ASCII")
            .into_iter()
            .next()
            .expect("literal fallback guarantees a sentence candidate");
        report.total += 1;
        report.observed_keys += observed_keys;
        report.unresolved_keys += candidate.unresolved_key_count;
        if candidate.unresolved_key_count == 0 {
            report.top_1_without_unresolved += 1;
        } else {
            report.top_1_with_unresolved += 1;
        }
        if candidate.unresolved_key_count == observed_keys {
            report.top_1_fully_unresolved += 1;
        }
    }
    report
}

/// Evaluates fully abbreviated, segmented sentence cases from a separate TSV.
///
/// The first non-comment row must be `id<TAB>tokens`. Each following row uses
/// space-separated lexicon words; observed keys are derived automatically.
pub fn evaluate_sentence_cases(
    decoder: &Decoder,
    lexicon: &[LexiconEntry],
    contents: &str,
) -> Result<SentenceCaseReport, SentenceCaseParseError> {
    let cases = parse_sentence_cases(lexicon, contents)?;
    let mut report = SentenceCaseReport {
        total: 0,
        hits_at_1: 0,
        hits_at_5: 0,
        hits_at_10: 0,
    };
    for case in cases {
        let candidates = decoder
            .decode_sentence(&case.observed, 10)
            .expect("generated sentence keys are lowercase ASCII");
        let rank = candidates
            .iter()
            .position(|candidate| candidate.text == case.expected);
        report.total += 1;
        if rank.is_some_and(|rank| rank < 1) {
            report.hits_at_1 += 1;
        }
        if rank.is_some_and(|rank| rank < 5) {
            report.hits_at_5 += 1;
        }
        if rank.is_some_and(|rank| rank < 10) {
            report.hits_at_10 += 1;
        }
    }
    Ok(report)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AuthoredSentenceCase {
    observed: String,
    expected: String,
}

fn parse_sentence_cases(
    lexicon: &[LexiconEntry],
    contents: &str,
) -> Result<Vec<AuthoredSentenceCase>, SentenceCaseParseError> {
    let entries_by_text = lexicon
        .iter()
        .map(|entry| (entry.text.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut saw_header = false;
    let mut identifiers = HashSet::new();
    let mut cases = Vec::new();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let line_number = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if !saw_header {
            if fields != ["id", "tokens"] {
                return Err(SentenceCaseParseError::InvalidHeader { line_number });
            }
            saw_header = true;
            continue;
        }
        if fields.len() != 2 || fields.iter().any(|field| field.is_empty()) {
            return Err(SentenceCaseParseError::InvalidRow { line_number });
        }
        if !identifiers.insert(fields[0].to_owned()) {
            return Err(SentenceCaseParseError::DuplicateId {
                line_number,
                id: fields[0].to_owned(),
            });
        }

        let tokens = fields[1].split_ascii_whitespace().collect::<Vec<_>>();
        if tokens.len() < 2 {
            return Err(SentenceCaseParseError::TooFewTokens { line_number });
        }
        let mut observed = String::new();
        let mut expected = String::new();
        for token in tokens {
            let entry =
                entries_by_text
                    .get(token)
                    .ok_or_else(|| SentenceCaseParseError::UnknownToken {
                        line_number,
                        token: token.to_owned(),
                    })?;
            observed.push_str(&fully_abbreviated_code(entry));
            expected.push_str(token);
        }
        cases.push(AuthoredSentenceCase { observed, expected });
    }

    if !saw_header {
        return Err(SentenceCaseParseError::MissingHeader);
    }
    if cases.is_empty() {
        return Err(SentenceCaseParseError::Empty);
    }
    Ok(cases)
}

fn full_lexicon_margin_per_key(decoder: &Decoder, observed: &str) -> Option<f64> {
    top_sentence_and_margin(decoder, observed).1
}

fn top_sentence_and_margin(decoder: &Decoder, observed: &str) -> (SentenceCandidate, Option<f64>) {
    // Sentence ordering places every fully covered path before every path with
    // unresolved input, so Top-1 is the best full path whenever one exists.
    let candidate = decoder
        .decode_sentence(observed, 1)
        .expect("evaluation keys are lowercase ASCII")
        .into_iter()
        .next()
        .expect("literal fallback guarantees a sentence candidate");
    if candidate.unresolved_key_count > 0 {
        return (candidate, None);
    }
    let observed_key_count = observed.len();
    debug_assert!(observed_key_count > 0);
    let fully_literal_score = -(observed_key_count as f64) * decoder.config.unresolved_key_penalty;
    let margin = (candidate.total_score - fully_literal_score) / observed_key_count as f64;
    (candidate, Some(margin))
}

fn rejection_margin_range(margins: &[Option<f64>]) -> Option<RejectionMarginRange> {
    let mut margins = margins.iter().flatten().copied();
    let first = margins.next()?;
    Some(margins.fold(
        RejectionMarginRange {
            minimum_per_key: first,
            maximum_per_key: first,
        },
        |range, margin| RejectionMarginRange {
            minimum_per_key: range.minimum_per_key.min(margin),
            maximum_per_key: range.maximum_per_key.max(margin),
        },
    ))
}

/// Error returned while parsing separate sentence-ranking cases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SentenceCaseParseError {
    /// No header row was found.
    MissingHeader,
    /// The header was not `id<TAB>tokens`.
    InvalidHeader {
        /// One-based source line number.
        line_number: usize,
    },
    /// A row did not contain two non-empty fields.
    InvalidRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A case identifier appeared more than once.
    DuplicateId {
        /// One-based source line number.
        line_number: usize,
        /// Duplicate identifier.
        id: String,
    },
    /// A case had fewer than two segmented words.
    TooFewTokens {
        /// One-based source line number.
        line_number: usize,
    },
    /// A token was not present in the decoder lexicon.
    UnknownToken {
        /// One-based source line number.
        line_number: usize,
        /// Unknown token.
        token: String,
    },
    /// A header was present but no cases followed.
    Empty,
}

impl fmt::Display for SentenceCaseParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(formatter, "句子评测集缺少表头"),
            Self::InvalidHeader { line_number } => {
                write!(formatter, "句子评测集第 {line_number} 行表头无效")
            }
            Self::InvalidRow { line_number } => {
                write!(formatter, "句子评测集第 {line_number} 行字段无效")
            }
            Self::DuplicateId { line_number, id } => {
                write!(formatter, "句子评测集第 {line_number} 行编号 {id:?} 重复")
            }
            Self::TooFewTokens { line_number } => {
                write!(formatter, "句子评测集第 {line_number} 行至少需要两个词")
            }
            Self::UnknownToken { line_number, token } => write!(
                formatter,
                "句子评测集第 {line_number} 行含有词典外词 {token:?}"
            ),
            Self::Empty => write!(formatter, "句子评测集没有数据行"),
        }
    }
}

impl Error for SentenceCaseParseError {}

/// Generates public synthetic cases and evaluates decoder Recall@K.
///
/// All mutations originate from canonical codes in the supplied lexicon. No
/// user text, keystrokes, randomness, network access, or disk logging is used.
pub fn evaluate_synthetic(decoder: &Decoder, lexicon: &[LexiconEntry]) -> EvaluationReport {
    let mut accumulators = CASE_KINDS.map(|kind| RecallMetrics {
        kind,
        total: 0,
        hits_at_1: 0,
        hits_at_5: 0,
        hits_at_10: 0,
    });
    let mut clean_top_1_exact = 0;
    let mut clean_total = 0;

    for entry in lexicon {
        let clean_candidates =
            record_case(decoder, entry, entry.code.as_str(), &mut accumulators[0]);
        clean_total += 1;
        if clean_candidates
            .first()
            .is_some_and(|candidate| candidate.correction == Correction::Exact)
        {
            clean_top_1_exact += 1;
        }

        for spelling in spelling_variants(&entry.syllable_codes)
            .into_iter()
            .filter(|spelling| !spelling.abbreviated_syllables.is_empty())
        {
            record_case(decoder, entry, spelling.code.as_str(), &mut accumulators[1]);
        }

        let full_code = entry.code.as_str().as_bytes();
        for index in 0..full_code.len() {
            for actual in b'a'..=b'z' {
                if are_qwerty_neighbors(full_code[index], actual) {
                    let mut observed = full_code.to_vec();
                    observed[index] = actual;
                    record_ascii_case(decoder, entry, observed, &mut accumulators[2]);
                }
            }
        }

        for start in 0..full_code.len().saturating_sub(1) {
            if full_code[start] != full_code[start + 1] {
                let mut observed = full_code.to_vec();
                observed.swap(start, start + 1);
                record_ascii_case(decoder, entry, observed, &mut accumulators[3]);
            }
        }

        for index in 0..full_code.len() {
            let mut observed = full_code.to_vec();
            observed.remove(index);
            record_ascii_case(decoder, entry, observed, &mut accumulators[4]);
        }

        for gap in 0..=full_code.len() {
            let repeated_key = if gap < full_code.len() {
                full_code[gap]
            } else {
                full_code[full_code.len() - 1]
            };
            let mut observed = full_code.to_vec();
            observed.insert(gap, repeated_key);
            record_ascii_case(decoder, entry, observed, &mut accumulators[5]);
        }
    }

    for pair in lexicon.windows(2) {
        let observed = format!(
            "{}{}",
            fully_abbreviated_code(&pair[0]),
            fully_abbreviated_code(&pair[1])
        );
        let target_text = format!("{}{}", pair[0].text, pair[1].text);
        let candidates = decoder
            .decode_sentence(&observed, 10)
            .expect("synthetic sentence keys are valid");
        let rank = candidates
            .iter()
            .position(|candidate| candidate.text == target_text);
        record_rank(rank, &mut accumulators[6]);
    }

    EvaluationReport {
        metrics: accumulators.into_iter().collect(),
        clean_top_1_exact,
        clean_total,
    }
}

fn record_ascii_case(
    decoder: &Decoder,
    target: &LexiconEntry,
    observed: Vec<u8>,
    metrics: &mut RecallMetrics,
) -> Vec<Candidate> {
    let observed = String::from_utf8(observed).expect("synthetic keys are lowercase ASCII");
    record_case(decoder, target, &observed, metrics)
}

fn record_case(
    decoder: &Decoder,
    target: &LexiconEntry,
    observed: &str,
    metrics: &mut RecallMetrics,
) -> Vec<Candidate> {
    let candidates = decoder
        .decode(observed, 10)
        .expect("synthetic keys are valid");
    let rank = candidates
        .iter()
        .position(|candidate| candidate.text == target.text && candidate.code == target.code);

    record_rank(rank, metrics);
    candidates
}

fn record_rank(rank: Option<usize>, metrics: &mut RecallMetrics) {
    metrics.total += 1;
    if rank.is_some_and(|rank| rank < 1) {
        metrics.hits_at_1 += 1;
    }
    if rank.is_some_and(|rank| rank < 5) {
        metrics.hits_at_5 += 1;
    }
    if rank.is_some_and(|rank| rank < 10) {
        metrics.hits_at_10 += 1;
    }
}

fn fully_abbreviated_code(entry: &LexiconEntry) -> String {
    entry
        .syllable_codes
        .iter()
        .map(|code| {
            code.as_str()
                .chars()
                .next()
                .expect("a syllable code is non-empty")
        })
        .collect()
}

fn rate(hits: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        hits as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        BigramLanguageModel, CharacterBigramLanguageModel, ContinuousCompositionProbe, Decoder,
        KeySequence, parse_lexicon_tsv,
    };

    use super::{
        LabeledSentenceProbe, ProbeSpellingMode, REJECTION_SHADOW_THRESHOLDS_PER_KEY,
        SyntheticCaseKind, audit_continuous_composition, evaluate_character_context_oracle,
        evaluate_context_oracle, evaluate_labeled_recall, evaluate_labeled_rejection_shadow,
        evaluate_oov_cases, evaluate_rejection_shadow, evaluate_sentence_cases, evaluate_synthetic,
    };

    const FIXTURE: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
    const BIGRAM_CORPUS: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");
    const SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/demo_sentence_cases.tsv");
    const LONG_SENTENCE_CASES: &str =
        include_str!("../tests/fixtures/public/long_sentence_cases.tsv");
    const OOV_CASES: &str = include_str!("../tests/fixtures/public/oov_lexicon.tsv");

    #[test]
    fn deterministic_report_covers_every_case_family() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let first = evaluate_synthetic(&decoder, &lexicon);
        let second = evaluate_synthetic(&decoder, &lexicon);

        assert_eq!(first, second);
        assert_eq!(first.metrics.len(), 7);
        assert!(first.total_cases() > lexicon.len());
        assert!(first.metrics.iter().all(|metrics| metrics.total > 0));
        assert_eq!(first.metrics[0].kind, SyntheticCaseKind::Clean);
        assert!((0.0..=1.0).contains(&first.clean_top_1_exact_rate()));
    }

    #[test]
    fn context_oracle_is_read_only_and_prefers_known_demo_bigram() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let model = BigramLanguageModel::from_tsv(BIGRAM_CORPUS, &lexicon).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let before = decoder.decode_sentence("ajjp", 10).unwrap();
        assert_eq!(before[0].text, "按键简拼");

        let report = evaluate_context_oracle(
            &decoder,
            &model,
            &lexicon,
            &[LabeledSentenceProbe {
                id: "key-keyboard".to_owned(),
                observed: KeySequence::new("ajjp").unwrap(),
                expected_text: "按键键盘".to_owned(),
                expected_segments: vec!["按键".to_owned(), "键盘".to_owned()],
                spelling_mode: ProbeSpellingMode::FullyAbbreviated,
            }],
        )
        .unwrap();

        assert_eq!(decoder.decode_sentence("ajjp", 10).unwrap(), before);
        assert_eq!(report.total, 1);
        assert_eq!(report.unigram_top_1_matches_expected, 0);
        assert_eq!(report.unigram_top_1_differs, 1);
        assert_eq!(report.incorrect_expected_path_preferred, 1);
        assert_eq!(report.incorrect_context_ties, 0);
        assert_eq!(report.incorrect_baseline_path_preferred, 0);
        assert_eq!(report.oracle_pair_matches_expected, 1);
        let range = report.incorrect_margin_range.unwrap();
        assert!(range.minimum_per_key > 0.0);
        assert_eq!(range.minimum_per_key, range.maximum_per_key);
    }

    #[test]
    fn continuous_audit_only_reranks_its_frozen_deeper_pool() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let word_model = BigramLanguageModel::from_tsv(BIGRAM_CORPUS, &lexicon).unwrap();
        let character_model = CharacterBigramLanguageModel::from_text_sequences(&[
            "按键键盘".to_owned(),
            "简拼".to_owned(),
        ])
        .unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let before = decoder.decode_sentence("ajjp", 10).unwrap();
        let observed = KeySequence::new("ajjp").unwrap();
        let report = audit_continuous_composition(
            &decoder,
            &word_model,
            &character_model,
            &lexicon,
            &[ContinuousCompositionProbe {
                id: "key-keyboard".to_owned(),
                full_observed: observed.clone(),
                tail_abbreviated_observed: observed.clone(),
                transposed_observed: observed,
                expected_text: "按键键盘".to_owned(),
                expected_segments: vec!["按键".to_owned(), "键盘".to_owned()],
            }],
            1,
            10,
        );

        assert_eq!(decoder.decode_sentence("ajjp", 10).unwrap(), before);
        assert_eq!(report.total, 1);
        assert_eq!(report.baseline_visible, 0);
        assert_eq!(report.deeper_visible, 1);
        assert_eq!(report.outside_audit_depth, 0);
        assert_eq!(report.failures.len(), 1);
        let failure = &report.failures[0];
        assert!(failure.baseline_rank.is_some_and(|rank| rank > 1));
        assert!(
            failure
                .word_context_rank
                .is_some_and(|rank| rank < failure.baseline_rank.unwrap())
        );
    }

    #[test]
    fn character_context_oracle_is_read_only_and_prefers_seen_text() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let model = CharacterBigramLanguageModel::from_text_sequences(&[
            "按键键盘".to_owned(),
            "简拼".to_owned(),
        ])
        .unwrap();
        let decoder = Decoder::new(lexicon);
        let before = decoder.decode_sentence("ajjp", 10).unwrap();
        let report = evaluate_character_context_oracle(
            &decoder,
            &model,
            &[LabeledSentenceProbe {
                id: "key-keyboard".to_owned(),
                observed: KeySequence::new("ajjp").unwrap(),
                expected_text: "按键键盘".to_owned(),
                expected_segments: vec!["按键".to_owned(), "键盘".to_owned()],
                spelling_mode: ProbeSpellingMode::FullyAbbreviated,
            }],
        );

        assert_eq!(decoder.decode_sentence("ajjp", 10).unwrap(), before);
        assert_eq!(report.total, 1);
        assert_eq!(report.unigram_top_1_matches_expected, 0);
        assert_eq!(report.unigram_top_1_differs, 1);
        assert_eq!(report.incorrect_expected_text_preferred, 1);
        assert_eq!(report.incorrect_context_ties, 0);
        assert_eq!(report.incorrect_baseline_text_preferred, 0);
        assert_eq!(report.oracle_pair_matches_expected, 1);
        assert!(report.incorrect_margin_range.unwrap().minimum_per_key > 0.0);
        assert_eq!(report.incorrect_expected_text_longer, 0);
        assert_eq!(report.incorrect_equal_text_length, 1);
        assert_eq!(report.incorrect_expected_text_shorter, 0);
        assert_eq!(report.incorrect_equal_length_expected_preferred, 1);
        assert_eq!(report.incorrect_equal_length_ties, 0);
        assert_eq!(report.incorrect_equal_length_baseline_preferred, 0);
        assert_eq!(report.incorrect_average_expected_preferred, 1);
        assert_eq!(report.incorrect_average_context_ties, 0);
        assert_eq!(report.incorrect_average_baseline_preferred, 0);
        assert_eq!(report.average_oracle_pair_matches_expected, 1);
        assert!(report.incorrect_average_margin_range.unwrap().minimum > 0.0);
    }

    #[test]
    fn labeled_recall_separates_top_one_from_visible_candidate_recall() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);
        let before = decoder.decode_sentence("ajjp", 10).unwrap();
        let report = evaluate_labeled_recall(
            &decoder,
            &[LabeledSentenceProbe {
                id: "key-keyboard".to_owned(),
                observed: KeySequence::new("ajjp").unwrap(),
                expected_text: "按键键盘".to_owned(),
                expected_segments: vec!["按键".to_owned(), "键盘".to_owned()],
                spelling_mode: ProbeSpellingMode::FullyAbbreviated,
            }],
        );

        assert_eq!(decoder.decode_sentence("ajjp", 10).unwrap(), before);
        assert_eq!(report.total, 1);
        assert_eq!(report.hits_at_1, 0);
        assert_eq!(report.hits_at_5, 1);
        assert_eq!(report.hits_at_10, 1);
        assert_eq!(report.recall_at_1(), 0.0);
        assert_eq!(report.recall_at_5(), 1.0);
        assert_eq!(report.recall_at_10(), 1.0);
    }

    #[test]
    fn held_out_words_report_literal_and_lexicon_coverage() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let held_out = parse_lexicon_tsv(OOV_CASES).unwrap();
        let decoder = Decoder::new(lexicon);
        let report = evaluate_oov_cases(&decoder, &held_out);

        assert_eq!(report.total, 12);
        assert_eq!(report.top_1_with_unresolved, 9);
        assert_eq!(report.top_1_fully_unresolved, 0);
        assert_eq!(report.top_1_without_unresolved, 3);
        assert_eq!(report.unresolved_keys, 17);
        assert_eq!(report.observed_keys, 48);
        assert_eq!(
            report.top_1_with_unresolved + report.top_1_without_unresolved,
            report.total
        );
        assert!(report.top_1_fully_unresolved <= report.top_1_with_unresolved);
        assert!(report.unresolved_keys <= report.observed_keys);
        assert!((0.0..=1.0).contains(&report.with_unresolved_rate()));
        assert!((0.0..=1.0).contains(&report.fully_unresolved_rate()));
        assert!((0.0..=1.0).contains(&report.unresolved_key_rate()));
    }

    #[test]
    fn rejection_shadow_is_deterministic_monotonic_and_read_only() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let held_out = parse_lexicon_tsv(OOV_CASES).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let before = decoder.decode_sentence("zrmurf", 10).unwrap();

        let first = evaluate_rejection_shadow(
            &decoder,
            &lexicon,
            &[SENTENCE_CASES, LONG_SENTENCE_CASES],
            &held_out,
        )
        .unwrap();
        let second = evaluate_rejection_shadow(
            &decoder,
            &lexicon,
            &[SENTENCE_CASES, LONG_SENTENCE_CASES],
            &held_out,
        )
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(decoder.decode_sentence("zrmurf", 10).unwrap(), before);
        assert_eq!(first.known_total, 18);
        assert_eq!(first.known_with_full_coverage, 18);
        assert_eq!(first.oov_total, 12);
        assert_eq!(first.oov_with_full_coverage, 3);
        let known_range = first.known_margin_range.unwrap();
        let oov_range = first.oov_margin_range.unwrap();
        assert!(known_range.minimum_per_key.is_finite());
        assert!(known_range.minimum_per_key <= known_range.maximum_per_key);
        assert!(oov_range.minimum_per_key.is_finite());
        assert!(oov_range.minimum_per_key <= oov_range.maximum_per_key);
        assert_eq!(
            first
                .thresholds
                .iter()
                .map(|metrics| metrics.threshold_per_key)
                .collect::<Vec<_>>(),
            REJECTION_SHADOW_THRESHOLDS_PER_KEY
        );
        assert!(first.thresholds.windows(2).all(|pair| {
            pair[0].known_accepted >= pair[1].known_accepted
                && pair[0].oov_rejected <= pair[1].oov_rejected
        }));
        assert!(first.thresholds.iter().all(|metrics| {
            metrics.known_accepted <= metrics.known_total
                && metrics.oov_rejected <= metrics.oov_total
                && (0.0..=1.0).contains(&metrics.known_acceptance_rate())
                && (0.0..=1.0).contains(&metrics.oov_rejection_rate())
        }));
    }

    #[test]
    fn rejection_shadow_treats_absent_full_coverage_as_rejected() {
        let held_out = parse_lexicon_tsv(OOV_CASES).unwrap();
        let decoder = Decoder::new(Vec::new());
        let report = evaluate_rejection_shadow(&decoder, &[], &[], &held_out[..1]).unwrap();

        assert_eq!(report.known_total, 0);
        assert_eq!(report.known_with_full_coverage, 0);
        assert_eq!(report.known_margin_range, None);
        assert_eq!(report.oov_total, 1);
        assert_eq!(report.oov_with_full_coverage, 0);
        assert_eq!(report.oov_margin_range, None);
        assert!(
            report
                .thresholds
                .iter()
                .all(|metrics| { metrics.known_accepted == 0 && metrics.oov_rejected == 1 })
        );
    }

    #[test]
    fn labeled_rejection_shadow_uses_top_text_without_changing_it() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);
        let observed = KeySequence::new("nihk").unwrap();
        let before = decoder.decode_sentence(observed.as_str(), 10).unwrap();
        let expected = before[0].text.clone();
        let probes = vec![
            LabeledSentenceProbe {
                id: "correct".to_owned(),
                observed: observed.clone(),
                expected_text: expected,
                expected_segments: vec!["你好".to_owned()],
                spelling_mode: ProbeSpellingMode::FullCode,
            },
            LabeledSentenceProbe {
                id: "incorrect".to_owned(),
                observed,
                expected_text: "不相符".to_owned(),
                expected_segments: vec!["不相符".to_owned()],
                spelling_mode: ProbeSpellingMode::FullCode,
            },
        ];

        let first = evaluate_labeled_rejection_shadow(&decoder, &probes);
        let second = evaluate_labeled_rejection_shadow(&decoder, &probes);

        assert_eq!(first, second);
        assert_eq!(decoder.decode_sentence("nihk", 10).unwrap(), before);
        assert_eq!(first.total, 2);
        assert_eq!(first.top_1_matches_expected, 1);
        assert_eq!(first.top_1_differs, 1);
        assert_eq!(first.incorrect_with_full_coverage, 1);
        assert!(first.correct_margin_range.is_some());
        assert!(first.incorrect_margin_range.is_some());
        assert!(first.thresholds.windows(2).all(|pair| {
            pair[0].correct_accepted >= pair[1].correct_accepted
                && pair[0].incorrect_rejected <= pair[1].incorrect_rejected
        }));
    }

    #[test]
    fn labeled_rejection_shadow_rejects_uncovered_incorrect_text() {
        let decoder = Decoder::new(Vec::new());
        let probes = vec![LabeledSentenceProbe {
            id: "uncovered".to_owned(),
            observed: KeySequence::new("zz").unwrap(),
            expected_text: "词典外".to_owned(),
            expected_segments: vec!["词典外".to_owned()],
            spelling_mode: ProbeSpellingMode::FullCode,
        }];
        let report = evaluate_labeled_rejection_shadow(&decoder, &probes);

        assert_eq!(report.top_1_matches_expected, 0);
        assert_eq!(report.top_1_differs, 1);
        assert_eq!(report.incorrect_with_full_coverage, 0);
        assert_eq!(report.incorrect_margin_range, None);
        assert!(
            report
                .thresholds
                .iter()
                .all(|metrics| metrics.incorrect_rejected == 1)
        );
    }

    #[test]
    fn longer_cases_remain_separate_from_language_model_rows() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let model = BigramLanguageModel::from_tsv(BIGRAM_CORPUS, &lexicon).unwrap();
        let decoder = Decoder::new(lexicon.clone()).with_bigram_model(model);
        let report = evaluate_sentence_cases(&decoder, &lexicon, LONG_SENTENCE_CASES).unwrap();

        assert_eq!(report.total, 5);
        assert_eq!(report.hits_at_1, 5);
        assert_eq!(report.hits_at_5, 5);
        assert_eq!(report.hits_at_10, 5);
    }

    #[test]
    fn separate_sentence_cases_compare_unigram_and_bigram_ranking() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let unigram_decoder = Decoder::new(lexicon.clone());
        let model = BigramLanguageModel::from_tsv(BIGRAM_CORPUS, &lexicon).unwrap();
        let bigram_decoder = Decoder::new(lexicon.clone()).with_bigram_model(model);

        let unigram = evaluate_sentence_cases(&unigram_decoder, &lexicon, SENTENCE_CASES).unwrap();
        let bigram = evaluate_sentence_cases(&bigram_decoder, &lexicon, SENTENCE_CASES).unwrap();

        assert_eq!(unigram.total, bigram.total);
        assert!(bigram.hits_at_1 >= unigram.hits_at_1);
        assert!(bigram.hits_at_5 >= unigram.hits_at_5);
    }
}
