use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::LexiconEntry;

const DEFAULT_ALPHA: f64 = 0.5;
const CHARACTER_BEGIN: u32 = 0x11_0000;
const CHARACTER_END: u32 = 0x11_0001;
const CHARACTER_UNKNOWN: u32 = 0x11_0002;

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

    /// Estimates add-alpha bigram counts from unweighted segmented sequences.
    ///
    /// Every sequence must contain at least two words and every word must
    /// exist in the supplied decoder lexicon.
    pub fn from_token_sequences(
        sequences: &[Vec<String>],
        lexicon: &[LexiconEntry],
    ) -> Result<Self, LanguageModelParseError> {
        let vocabulary = lexicon
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<HashSet<_>>();
        if vocabulary.is_empty() {
            return Err(LanguageModelParseError::EmptyVocabulary);
        }
        if sequences.is_empty() {
            return Err(LanguageModelParseError::EmptyCorpus);
        }

        let mut pair_counts = HashMap::<(String, String), u64>::new();
        let mut predecessor_totals = HashMap::<String, u64>::new();
        for (sequence_index, tokens) in sequences.iter().enumerate() {
            let source_number = sequence_index + 1;
            if tokens.len() < 2 {
                return Err(LanguageModelParseError::TooFewTokens {
                    line_number: source_number,
                });
            }
            for token in tokens {
                if !vocabulary.contains(token.as_str()) {
                    return Err(LanguageModelParseError::UnknownToken {
                        line_number: source_number,
                        token: token.clone(),
                    });
                }
            }
            for pair in tokens.windows(2) {
                checked_add(
                    pair_counts
                        .entry((pair[0].clone(), pair[1].clone()))
                        .or_insert(0),
                    1,
                    source_number,
                )?;
                checked_add(
                    predecessor_totals.entry(pair[0].clone()).or_insert(0),
                    1,
                    source_number,
                )?;
            }
        }

        Ok(Self {
            pair_counts,
            predecessor_totals,
            vocabulary_size: vocabulary.len(),
            alpha: DEFAULT_ALPHA,
        })
    }

    /// Returns deterministic structural and count statistics.
    pub fn stats(&self) -> BigramLanguageModelStats {
        BigramLanguageModelStats {
            vocabulary_size: self.vocabulary_size,
            observed_pair_types: self.pair_counts.len(),
            observed_predecessor_types: self.predecessor_totals.len(),
            observed_pair_instances: self
                .pair_counts
                .values()
                .map(|count| u128::from(*count))
                .sum(),
        }
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

/// Auditable structure and count statistics for a bigram model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BigramLanguageModelStats {
    /// Distinct word texts in the decoder lexicon.
    pub vocabulary_size: usize,
    /// Distinct observed `(previous, current)` pairs.
    pub observed_pair_types: usize,
    /// Distinct words observed with at least one successor.
    pub observed_predecessor_types: usize,
    /// Total observed pair instances across all sequences.
    pub observed_pair_instances: u128,
}

/// Add-alpha character bigram model trained only from public text.
#[derive(Clone, Debug)]
pub struct CharacterBigramLanguageModel {
    pair_counts: HashMap<(u32, u32), u64>,
    predecessor_totals: HashMap<u32, u64>,
    vocabulary: HashSet<char>,
    alpha: f64,
    stats: CharacterBigramLanguageModelStats,
}

impl CharacterBigramLanguageModel {
    /// Estimates character bigrams from unweighted non-empty text sequences.
    ///
    /// Each sequence receives explicit begin/end transitions. An additional
    /// unknown-character output bucket keeps held-out scoring finite.
    pub fn from_text_sequences(sequences: &[String]) -> Result<Self, CharacterLanguageModelError> {
        if sequences.is_empty() {
            return Err(CharacterLanguageModelError::EmptyCorpus);
        }
        let mut vocabulary = HashSet::new();
        for (sequence_index, sequence) in sequences.iter().enumerate() {
            if sequence.is_empty() {
                return Err(CharacterLanguageModelError::EmptySequence {
                    sequence_number: sequence_index + 1,
                });
            }
            vocabulary.extend(sequence.chars());
        }

        let mut pair_counts = HashMap::<(u32, u32), u64>::new();
        let mut predecessor_totals = HashMap::<u32, u64>::new();
        let mut character_instances = 0_u128;
        for (sequence_index, sequence) in sequences.iter().enumerate() {
            let sequence_number = sequence_index + 1;
            let mut previous = CHARACTER_BEGIN;
            for character in sequence.chars() {
                let current = u32::from(character);
                checked_add_character(
                    pair_counts.entry((previous, current)).or_insert(0),
                    sequence_number,
                )?;
                checked_add_character(
                    predecessor_totals.entry(previous).or_insert(0),
                    sequence_number,
                )?;
                previous = current;
                character_instances += 1;
            }
            checked_add_character(
                pair_counts.entry((previous, CHARACTER_END)).or_insert(0),
                sequence_number,
            )?;
            checked_add_character(
                predecessor_totals.entry(previous).or_insert(0),
                sequence_number,
            )?;
        }

        let stats = CharacterBigramLanguageModelStats {
            sequences: sequences.len(),
            character_instances,
            vocabulary_size: vocabulary.len() + 2,
            observed_pair_types: pair_counts.len(),
            observed_pair_instances: character_instances + sequences.len() as u128,
        };
        Ok(Self {
            pair_counts,
            predecessor_totals,
            vocabulary,
            alpha: DEFAULT_ALPHA,
            stats,
        })
    }

    /// Scores a complete text including its end-of-sequence transition.
    pub fn score_text(&self, text: &str) -> CharacterSequenceScore {
        let mut previous = CHARACTER_BEGIN;
        let mut log_probability = 0.0;
        let mut observed_pairs = 0;
        let mut pair_count = 0;
        for character in text.chars() {
            let current = if self.vocabulary.contains(&character) {
                u32::from(character)
            } else {
                CHARACTER_UNKNOWN
            };
            let score = self.score_pair(previous, current);
            log_probability += score.log_probability;
            observed_pairs += usize::from(score.observed_count > 0);
            pair_count += 1;
            previous = current;
        }
        let score = self.score_pair(previous, CHARACTER_END);
        log_probability += score.log_probability;
        observed_pairs += usize::from(score.observed_count > 0);
        pair_count += 1;
        CharacterSequenceScore {
            log_probability,
            observed_pairs,
            pair_count,
        }
    }

    /// Returns deterministic training statistics.
    pub fn stats(&self) -> CharacterBigramLanguageModelStats {
        self.stats
    }

    fn score_pair(&self, previous: u32, current: u32) -> BigramScore {
        let observed_count = self
            .pair_counts
            .get(&(previous, current))
            .copied()
            .unwrap_or(0);
        let predecessor_total = self.predecessor_totals.get(&previous).copied().unwrap_or(0);
        let numerator = observed_count as f64 + self.alpha;
        let denominator = predecessor_total as f64 + self.alpha * self.stats.vocabulary_size as f64;
        BigramScore {
            observed_count,
            predecessor_total,
            alpha: self.alpha,
            vocabulary_size: self.stats.vocabulary_size,
            log_probability: (numerator / denominator).ln(),
        }
    }
}

/// Auditable structure and counts for a character bigram model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CharacterBigramLanguageModelStats {
    /// Non-empty public text sequences used for training.
    pub sequences: usize,
    /// Han character instances across all sequences.
    pub character_instances: u128,
    /// Seen characters plus unknown and end output symbols.
    pub vocabulary_size: usize,
    /// Distinct observed character transitions, including boundaries.
    pub observed_pair_types: usize,
    /// Total transitions, including one end transition per sequence.
    pub observed_pair_instances: u128,
}

/// Aggregate evidence for one complete character sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterSequenceScore {
    /// Sum of add-alpha log conditional probabilities.
    pub log_probability: f64,
    /// Transitions observed at least once in training.
    pub observed_pairs: usize,
    /// Character transitions plus the final end transition.
    pub pair_count: usize,
}

/// Error returned while estimating a public character model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CharacterLanguageModelError {
    /// No training sequence was supplied.
    EmptyCorpus,
    /// One supplied sequence had no characters.
    EmptySequence {
        /// One-based sequence number.
        sequence_number: usize,
    },
    /// A transition count exceeded `u64`.
    CountOverflow {
        /// One-based sequence number.
        sequence_number: usize,
    },
}

impl fmt::Display for CharacterLanguageModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCorpus => write!(formatter, "字级语言模型语料不能为空"),
            Self::EmptySequence { sequence_number } => {
                write!(formatter, "字级语言模型第 {sequence_number} 个序列为空")
            }
            Self::CountOverflow { sequence_number } => {
                write!(
                    formatter,
                    "字级语言模型累计到第 {sequence_number} 个序列时计数溢出"
                )
            }
        }
    }
}

impl Error for CharacterLanguageModelError {}

fn checked_add_character(
    destination: &mut u64,
    sequence_number: usize,
) -> Result<(), CharacterLanguageModelError> {
    *destination = destination
        .checked_add(1)
        .ok_or(CharacterLanguageModelError::CountOverflow { sequence_number })?;
    Ok(())
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

    use super::{
        BigramLanguageModel, CharacterBigramLanguageModel, CharacterLanguageModelError,
        LanguageModelParseError,
    };

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

    #[test]
    fn token_sequences_share_the_same_add_alpha_evidence() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let sequences = vec![
            vec!["自然码".to_owned(), "输入法".to_owned()],
            vec!["自然码".to_owned(), "输入法".to_owned()],
            vec!["自然码".to_owned(), "项目".to_owned()],
        ];
        let model = BigramLanguageModel::from_token_sequences(&sequences, &lexicon).unwrap();
        let stats = model.stats();

        assert_eq!(model.score("自然码", "输入法").observed_count, 2);
        assert_eq!(model.score("自然码", "项目").observed_count, 1);
        assert_eq!(stats.observed_pair_types, 2);
        assert_eq!(stats.observed_predecessor_types, 1);
        assert_eq!(stats.observed_pair_instances, 3);
    }

    #[test]
    fn character_bigram_prefers_observed_continuation_and_tracks_boundaries() {
        let sequences = vec!["按键键盘".to_owned(), "简拼".to_owned()];
        let model = CharacterBigramLanguageModel::from_text_sequences(&sequences).unwrap();
        let expected = model.score_text("按键键盘");
        let alternative = model.score_text("按键简拼");
        let stats = model.stats();

        assert!(expected.log_probability > alternative.log_probability);
        assert_eq!(expected.pair_count, 5);
        assert_eq!(expected.observed_pairs, 5);
        assert!(alternative.observed_pairs < alternative.pair_count);
        assert_eq!(stats.sequences, 2);
        assert_eq!(stats.character_instances, 6);
        assert_eq!(stats.vocabulary_size, 7);
        assert_eq!(stats.observed_pair_instances, 8);
        assert!(matches!(
            CharacterBigramLanguageModel::from_text_sequences(&[]),
            Err(CharacterLanguageModelError::EmptyCorpus)
        ));
    }
}
