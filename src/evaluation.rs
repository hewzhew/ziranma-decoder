use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    Candidate, Correction, Decoder, LexiconEntry, are_qwerty_neighbors, spelling_variants,
};

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

/// Evaluates fully abbreviated, segmented sentence cases from a separate TSV.
///
/// The first non-comment row must be `id<TAB>tokens`. Each following row uses
/// space-separated lexicon words; observed keys are derived automatically.
pub fn evaluate_sentence_cases(
    decoder: &Decoder,
    lexicon: &[LexiconEntry],
    contents: &str,
) -> Result<SentenceCaseReport, SentenceCaseParseError> {
    let entries_by_text = lexicon
        .iter()
        .map(|entry| (entry.text.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut saw_header = false;
    let mut identifiers = HashSet::new();
    let mut report = SentenceCaseReport {
        total: 0,
        hits_at_1: 0,
        hits_at_5: 0,
        hits_at_10: 0,
    };

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

        let candidates = decoder
            .decode_sentence(&observed, 10)
            .expect("generated sentence keys are lowercase ASCII");
        let rank = candidates
            .iter()
            .position(|candidate| candidate.text == expected);
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

    if !saw_header {
        return Err(SentenceCaseParseError::MissingHeader);
    }
    if report.total == 0 {
        return Err(SentenceCaseParseError::Empty);
    }
    Ok(report)
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
    use crate::{BigramLanguageModel, Decoder, parse_lexicon_tsv};

    use super::{SyntheticCaseKind, evaluate_sentence_cases, evaluate_synthetic};

    const FIXTURE: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
    const BIGRAM_CORPUS: &str = include_str!("../tests/fixtures/public/demo_bigram_corpus.tsv");
    const SENTENCE_CASES: &str = include_str!("../tests/fixtures/public/demo_sentence_cases.tsv");

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
