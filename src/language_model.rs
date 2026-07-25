use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::LexiconEntry;

const DEFAULT_ALPHA: f64 = 0.5;

/// Locally trained add-alpha bigram model over lexicon words.
#[derive(Clone, Debug)]
pub struct BigramLanguageModel {
    pair_counts: HashMap<(String, String), u64>,
    predecessor_totals: HashMap<String, u64>,
    vocabulary_size: usize,
    alpha: f64,
}

impl BigramLanguageModel {
    /// Parses a weighted, segmented TSV corpus and estimates bigram counts.
    ///
    /// The first non-comment row must be `tokens<TAB>count`. Tokens are
    /// separated by ASCII spaces and every token must exist in the lexicon.
    pub fn from_tsv(
        contents: &str,
        lexicon: &[LexiconEntry],
    ) -> Result<Self, LanguageModelParseError> {
        let vocabulary = lexicon
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<HashSet<_>>();
        if vocabulary.is_empty() {
            return Err(LanguageModelParseError::EmptyVocabulary);
        }

        let mut saw_header = false;
        let mut saw_sequence = false;
        let mut pair_counts = HashMap::<(String, String), u64>::new();
        let mut predecessor_totals = HashMap::<String, u64>::new();

        for (zero_based_line, raw_line) in contents.lines().enumerate() {
            let line_number = zero_based_line + 1;
            let line = raw_line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let fields = line.split('\t').collect::<Vec<_>>();
            if !saw_header {
                if fields != ["tokens", "count"] {
                    return Err(LanguageModelParseError::InvalidHeader { line_number });
                }
                saw_header = true;
                continue;
            }
            if fields.len() != 2 || fields.iter().any(|field| field.is_empty()) {
                return Err(LanguageModelParseError::InvalidRow { line_number });
            }

            let count =
                fields[1]
                    .parse::<u64>()
                    .map_err(|_| LanguageModelParseError::InvalidCount {
                        line_number,
                        value: fields[1].to_owned(),
                    })?;
            if count == 0 {
                return Err(LanguageModelParseError::InvalidCount {
                    line_number,
                    value: fields[1].to_owned(),
                });
            }

            let tokens = fields[0].split_ascii_whitespace().collect::<Vec<_>>();
            if tokens.len() < 2 {
                return Err(LanguageModelParseError::TooFewTokens { line_number });
            }
            for token in &tokens {
                if !vocabulary.contains(token) {
                    return Err(LanguageModelParseError::UnknownToken {
                        line_number,
                        token: (*token).to_owned(),
                    });
                }
            }

            for pair in tokens.windows(2) {
                let pair_key = (pair[0].to_owned(), pair[1].to_owned());
                checked_add(pair_counts.entry(pair_key).or_insert(0), count, line_number)?;
                checked_add(
                    predecessor_totals.entry(pair[0].to_owned()).or_insert(0),
                    count,
                    line_number,
                )?;
            }
            saw_sequence = true;
        }

        if !saw_header {
            return Err(LanguageModelParseError::MissingHeader);
        }
        if !saw_sequence {
            return Err(LanguageModelParseError::EmptyCorpus);
        }

        Ok(Self {
            pair_counts,
            predecessor_totals,
            vocabulary_size: vocabulary.len(),
            alpha: DEFAULT_ALPHA,
        })
    }

    /// Returns an explainable add-alpha conditional score.
    pub fn score(&self, previous: &str, current: &str) -> BigramScore {
        let observed_count = self
            .pair_counts
            .get(&(previous.to_owned(), current.to_owned()))
            .copied()
            .unwrap_or(0);
        let predecessor_total = self.predecessor_totals.get(previous).copied().unwrap_or(0);
        let numerator = observed_count as f64 + self.alpha;
        let denominator = predecessor_total as f64 + self.alpha * self.vocabulary_size as f64;
        BigramScore {
            observed_count,
            predecessor_total,
            alpha: self.alpha,
            vocabulary_size: self.vocabulary_size,
            log_probability: (numerator / denominator).ln(),
        }
    }
}

/// Full evidence behind one conditional word probability.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BigramScore {
    /// Weighted count for `(previous, current)`.
    pub observed_count: u64,
    /// Sum of all weighted bigrams beginning with `previous`.
    pub predecessor_total: u64,
    /// Additive smoothing value.
    pub alpha: f64,
    /// Number of possible current words.
    pub vocabulary_size: usize,
    /// Smoothed `ln P(current | previous)`.
    pub log_probability: f64,
}

/// Error returned while parsing the local synthetic bigram corpus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanguageModelParseError {
    /// The supplied lexicon had no words.
    EmptyVocabulary,
    /// No header row was found.
    MissingHeader,
    /// The header was not `tokens<TAB>count`.
    InvalidHeader {
        /// One-based source line number.
        line_number: usize,
    },
    /// A row did not contain exactly two non-empty fields.
    InvalidRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A count was not a positive integer.
    InvalidCount {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// A sequence had fewer than two segmented words.
    TooFewTokens {
        /// One-based source line number.
        line_number: usize,
    },
    /// A corpus token was not present in the decoder lexicon.
    UnknownToken {
        /// One-based source line number.
        line_number: usize,
        /// Unknown token.
        token: String,
    },
    /// Weighted count accumulation overflowed `u64`.
    CountOverflow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A header was present but no sequences followed.
    EmptyCorpus,
}

impl fmt::Display for LanguageModelParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyVocabulary => write!(formatter, "语言模型词表不能为空"),
            Self::MissingHeader => write!(formatter, "语言模型语料缺少表头"),
            Self::InvalidHeader { line_number } => {
                write!(formatter, "语言模型语料第 {line_number} 行表头无效")
            }
            Self::InvalidRow { line_number } => {
                write!(formatter, "语言模型语料第 {line_number} 行字段无效")
            }
            Self::InvalidCount { line_number, value } => write!(
                formatter,
                "语言模型语料第 {line_number} 行权重必须是正整数，实际为 {value:?}"
            ),
            Self::TooFewTokens { line_number } => {
                write!(formatter, "语言模型语料第 {line_number} 行至少需要两个分词")
            }
            Self::UnknownToken { line_number, token } => write!(
                formatter,
                "语言模型语料第 {line_number} 行含有词典外词 {token:?}"
            ),
            Self::CountOverflow { line_number } => {
                write!(formatter, "语言模型语料累计到第 {line_number} 行时计数溢出")
            }
            Self::EmptyCorpus => write!(formatter, "语言模型语料没有数据行"),
        }
    }
}

impl Error for LanguageModelParseError {}

fn checked_add(
    destination: &mut u64,
    value: u64,
    line_number: usize,
) -> Result<(), LanguageModelParseError> {
    *destination = destination
        .checked_add(value)
        .ok_or(LanguageModelParseError::CountOverflow { line_number })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::parse_lexicon_tsv;

    use super::{BigramLanguageModel, LanguageModelParseError};

    const LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

    #[test]
    fn seen_bigram_scores_above_unseen_alternative() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let model = BigramLanguageModel::from_tsv(
            "tokens\tcount\n自然码 输入法\t10\n自然码 项目\t1\n",
            &lexicon,
        )
        .unwrap();

        assert!(
            model.score("自然码", "输入法").log_probability
                > model.score("自然码", "测试").log_probability
        );
        assert_eq!(model.score("自然码", "输入法").observed_count, 10);
    }

    #[test]
    fn rejects_unknown_corpus_tokens() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        assert!(matches!(
            BigramLanguageModel::from_tsv("tokens\tcount\n自然码 不存在\t1\n", &lexicon),
            Err(LanguageModelParseError::UnknownToken { .. })
        ));
    }
}
