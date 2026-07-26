use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    Candidate, Correction, Decoder, KeySequence, LexiconEntry, SentenceCandidate,
    are_qwerty_neighbors, spelling_variants,
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
}

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
    use crate::{BigramLanguageModel, Decoder, KeySequence, parse_lexicon_tsv};

    use super::{
        LabeledSentenceProbe, REJECTION_SHADOW_THRESHOLDS_PER_KEY, SyntheticCaseKind,
        evaluate_labeled_rejection_shadow, evaluate_oov_cases, evaluate_rejection_shadow,
        evaluate_sentence_cases, evaluate_synthetic,
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
    fn held_out_words_report_literal_and_lexicon_coverage() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let held_out = parse_lexicon_tsv(OOV_CASES).unwrap();
        let decoder = Decoder::new(lexicon);
        let report = evaluate_oov_cases(&decoder, &held_out);

        assert_eq!(report.total, 12);
        assert_eq!(report.top_1_with_unresolved, 8);
        assert_eq!(report.top_1_fully_unresolved, 0);
        assert_eq!(report.top_1_without_unresolved, 4);
        assert_eq!(report.unresolved_keys, 15);
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
        assert_eq!(first.oov_with_full_coverage, 4);
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
            },
            LabeledSentenceProbe {
                id: "incorrect".to_owned(),
                observed,
                expected_text: "不相符".to_owned(),
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
