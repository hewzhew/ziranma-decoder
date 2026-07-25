//! Explainable baseline decoder for Ziranma double-pinyin key sequences.
//!
//! The first milestone deliberately supports only exact input, one QWERTY
//! neighbor substitution, or one adjacent transposition. It does not segment
//! sentences, expand abbreviations, or collect user input.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

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
    /// Canonical Ziranma key sequence.
    pub code: KeySequence,
    /// Synthetic relative weight, not a measured corpus count.
    pub frequency: u64,
}

/// A supported relationship between observed and intended keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Correction {
    /// The observed keys exactly match the lexicon entry.
    Exact,
    /// One intended key was observed as a physical QWERTY neighbor.
    NeighborSubstitution {
        /// Zero-based byte index in the ASCII key sequence.
        index: usize,
        /// Key prescribed by the lexicon entry.
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
        }
    }
}

/// Components used to rank a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreBreakdown {
    /// Natural logarithm of the entry's synthetic relative frequency.
    pub frequency: f64,
    /// Cost associated with the detected correction.
    pub correction_penalty: f64,
    /// `frequency - correction_penalty`.
    pub total: f64,
}

/// A decoded candidate together with its complete first-stage explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Candidate Chinese text.
    pub text: String,
    /// Full pinyin recorded in the public fixture.
    pub pinyin: String,
    /// Canonical Ziranma code.
    pub code: KeySequence,
    /// How the observed input relates to this code.
    pub correction: Correction,
    /// Transparent score components.
    pub score: ScoreBreakdown,
}

/// Tunable penalties for the first-stage baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeConfig {
    /// Penalty for one QWERTY-neighbor substitution.
    pub neighbor_substitution_penalty: f64,
    /// Penalty for one adjacent transposition.
    pub adjacent_transposition_penalty: f64,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            neighbor_substitution_penalty: 2.75,
            adjacent_transposition_penalty: 2.25,
        }
    }
}

/// Exhaustive decoder over a small lexicon.
#[derive(Clone, Debug)]
pub struct Decoder {
    lexicon: Vec<LexiconEntry>,
    config: DecodeConfig,
}

impl Decoder {
    /// Creates a decoder with conservative default correction penalties.
    pub fn new(lexicon: Vec<LexiconEntry>) -> Self {
        Self {
            lexicon,
            config: DecodeConfig::default(),
        }
    }

    /// Creates a decoder with explicit correction penalties.
    ///
    /// # Panics
    ///
    /// Panics if either penalty is negative or non-finite. Configuration is
    /// programmer-owned in this milestone rather than loaded from user input.
    pub fn with_config(lexicon: Vec<LexiconEntry>, config: DecodeConfig) -> Self {
        assert!(
            config.neighbor_substitution_penalty.is_finite()
                && config.neighbor_substitution_penalty >= 0.0,
            "neighbor substitution penalty must be finite and non-negative"
        );
        assert!(
            config.adjacent_transposition_penalty.is_finite()
                && config.adjacent_transposition_penalty >= 0.0,
            "adjacent transposition penalty must be finite and non-negative"
        );
        Self { lexicon, config }
    }

    /// Returns at most `top_k` matching candidates in deterministic score order.
    pub fn decode(&self, observed: &str, top_k: usize) -> Result<Vec<Candidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let mut candidates = self
            .lexicon
            .iter()
            .filter_map(|entry| {
                let correction = detect_correction(observed.as_str(), entry.code.as_str())?;
                let correction_penalty = match correction {
                    Correction::Exact => 0.0,
                    Correction::NeighborSubstitution { .. } => {
                        self.config.neighbor_substitution_penalty
                    }
                    Correction::AdjacentTransposition { .. } => {
                        self.config.adjacent_transposition_penalty
                    }
                };
                let frequency = (entry.frequency as f64).ln();
                Some(Candidate {
                    text: entry.text.clone(),
                    pinyin: entry.pinyin.clone(),
                    code: entry.code.clone(),
                    correction,
                    score: ScoreBreakdown {
                        frequency,
                        correction_penalty,
                        total: frequency - correction_penalty,
                    },
                })
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| {
            right
                .score
                .total
                .total_cmp(&left.score.total)
                .then_with(|| left.text.cmp(&right.text))
                .then_with(|| left.code.as_str().cmp(right.code.as_str()))
        });
        candidates.truncate(top_k);
        Ok(candidates)
    }
}

/// Parses the repository's auditable tab-separated demo lexicon format.
///
/// The first non-comment row must be:
/// `text<TAB>pinyin<TAB>code<TAB>frequency`.
pub fn parse_lexicon_tsv(contents: &str) -> Result<Vec<LexiconEntry>, LexiconParseError> {
    const EXPECTED_HEADER: [&str; 4] = ["text", "pinyin", "code", "frequency"];

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
            fields[3]
                .parse::<u64>()
                .map_err(|_| LexiconParseError::InvalidFrequency {
                    line_number,
                    value: fields[3].to_owned(),
                })?;
        if frequency == 0 {
            return Err(LexiconParseError::InvalidFrequency {
                line_number,
                value: fields[3].to_owned(),
            });
        }

        let code = KeySequence::new(fields[2]).map_err(|_| LexiconParseError::InvalidCode {
            line_number,
            value: fields[2].to_owned(),
        })?;
        let duplicate_key = (fields[0].to_owned(), code.clone());
        if !duplicates.insert(duplicate_key) {
            return Err(LexiconParseError::DuplicateEntry { line_number });
        }

        entries.push(LexiconEntry {
            text: fields[0].to_owned(),
            pinyin: fields[1].to_owned(),
            code,
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
    /// The header did not match the documented four columns.
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
    /// A code was not a non-empty lowercase ASCII sequence.
    InvalidCode {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
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
            Self::InvalidCode { line_number, value } => write!(
                formatter,
                "词典第 {line_number} 行的编码必须是小写英文字母，实际为 {value:?}"
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

fn are_qwerty_neighbors(left: u8, right: u8) -> bool {
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
    use super::{Correction, KeySequence, are_qwerty_neighbors, detect_correction};

    #[test]
    fn key_sequence_accepts_only_lowercase_ascii() {
        assert!(KeySequence::new("nihk").is_ok());
        assert!(KeySequence::new("").is_err());
        assert!(KeySequence::new("NiHk").is_err());
        assert!(KeySequence::new("你好").is_err());
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
    fn detects_supported_corrections_only() {
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
        assert!(detect_correction("niqk", "nihk").is_none());
        assert!(detect_correction("nifj", "nihk").is_none());
        assert!(detect_correction("nih", "nihk").is_none());
    }
}
