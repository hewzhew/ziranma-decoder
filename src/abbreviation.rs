use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::LexiconEntry;

/// Structural audit of free one-key/two-key syllable mixing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbbreviationCodebookAudit {
    /// Distinct tone-free pinyin syllable labels observed in the lexicon.
    pub distinct_pinyin_syllables: usize,
    /// Distinct canonical two-key codes used by those syllables.
    pub distinct_full_codes: usize,
    /// Two-key codes shared by more than one pinyin syllable label.
    pub full_code_collision_groups: usize,
    /// Largest number of pinyin labels sharing one full code.
    pub maximum_pinyin_labels_per_full_code: usize,
    /// Distinct first keys accepted as one-key syllable abbreviations.
    pub abbreviation_keys: usize,
    /// Abbreviation keys that expand to more than one pinyin syllable.
    pub ambiguous_abbreviation_keys: usize,
    /// Largest number of pinyin syllables sharing one abbreviation key.
    pub maximum_pinyin_labels_per_abbreviation_key: usize,
    /// Full codes that can also be segmented as two one-key abbreviations.
    pub full_codes_split_as_two_abbreviations: usize,
    /// Largest number of labeled syllable paths behind one two-key string.
    pub maximum_labeled_paths_for_two_keys: usize,
    /// Two-key string attaining `maximum_labeled_paths_for_two_keys`.
    pub maximum_labeled_paths_code: String,
    /// Constructive proof that a two-key code has two different boundaries.
    pub immediate_ambiguity_witness: Option<ImmediateAmbiguityWitness>,
    /// Key whose repeated string has Fibonacci-many one/two-key boundaries.
    pub fibonacci_witness_key: Option<char>,
}

impl AbbreviationCodebookAudit {
    /// Whether an immediate one-full-versus-two-abbreviation witness exists.
    ///
    /// A true result constructively refutes unique decodability. A false
    /// result would not by itself prove unique decodability through all longer
    /// concatenations.
    pub fn unique_decodability_is_refuted(&self) -> bool {
        self.full_codes_split_as_two_abbreviations > 0
    }

    /// Number of boundary parses for a repeated witness key of this length.
    ///
    /// When both the one-key code `x` and the two-key code `xx` exist, this is
    /// `F(length + 1)`. It counts boundaries only, before pinyin or Chinese
    /// alternatives are considered.
    pub fn fibonacci_boundary_parses(&self, length: usize) -> Option<u128> {
        self.fibonacci_witness_key?;
        let mut ending_with_one = 1_u128;
        let mut ending_with_two = 0_u128;
        for _ in 0..length {
            let next = ending_with_one.checked_add(ending_with_two)?;
            ending_with_two = ending_with_one;
            ending_with_one = next;
        }
        Some(ending_with_one)
    }
}

/// One deterministic immediate ambiguity in the public codebook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImmediateAmbiguityWitness {
    /// The same observed two-key string on both paths.
    pub observed: String,
    /// Example pinyin syllable using the string as one full code.
    pub full_syllable: String,
    /// Example pinyin syllable using only the first observed key.
    pub first_abbreviated_syllable: String,
    /// Example pinyin syllable using only the second observed key.
    pub second_abbreviated_syllable: String,
}

/// Audits the actual syllable codebook represented by a decoder lexicon.
pub fn audit_abbreviation_codebook(
    lexicon: &[LexiconEntry],
) -> Result<AbbreviationCodebookAudit, AbbreviationAuditError> {
    if lexicon.is_empty() {
        return Err(AbbreviationAuditError::EmptyLexicon);
    }

    let mut syllable_to_code = BTreeMap::<String, String>::new();
    for entry in lexicon {
        let syllables = entry.pinyin.split_ascii_whitespace().collect::<Vec<_>>();
        if syllables.len() != entry.syllable_codes.len() {
            return Err(AbbreviationAuditError::SyllableCountMismatch {
                text: entry.text.clone(),
                pinyin_syllables: syllables.len(),
                codes: entry.syllable_codes.len(),
            });
        }
        for (syllable, code) in syllables.into_iter().zip(&entry.syllable_codes) {
            if code.as_str().len() != 2 {
                return Err(AbbreviationAuditError::NonTwoKeyCode {
                    syllable: syllable.to_owned(),
                    code: code.as_str().to_owned(),
                });
            }
            match syllable_to_code.get(syllable) {
                Some(previous) if previous != code.as_str() => {
                    return Err(AbbreviationAuditError::InconsistentSyllableCode {
                        syllable: syllable.to_owned(),
                        first_code: previous.clone(),
                        second_code: code.as_str().to_owned(),
                    });
                }
                Some(_) => {}
                None => {
                    syllable_to_code.insert(syllable.to_owned(), code.as_str().to_owned());
                }
            }
        }
    }

    let mut syllables_by_full_code = BTreeMap::<String, BTreeSet<String>>::new();
    let mut syllables_by_abbreviation_key = BTreeMap::<char, BTreeSet<String>>::new();
    for (syllable, code) in &syllable_to_code {
        syllables_by_full_code
            .entry(code.clone())
            .or_default()
            .insert(syllable.clone());
        let first_key = code
            .chars()
            .next()
            .expect("a validated two-key code has a first key");
        syllables_by_abbreviation_key
            .entry(first_key)
            .or_default()
            .insert(syllable.clone());
    }

    let mut full_codes_split_as_two_abbreviations = 0;
    let mut maximum_labeled_paths_for_two_keys = 0;
    let mut maximum_labeled_paths_code = String::new();
    let mut immediate_ambiguity_witness = None;
    for (code, full_syllables) in &syllables_by_full_code {
        let mut keys = code.chars();
        let first_key = keys
            .next()
            .expect("a validated two-key code has a first key");
        let second_key = keys
            .next()
            .expect("a validated two-key code has a second key");
        let Some(first_syllables) = syllables_by_abbreviation_key.get(&first_key) else {
            continue;
        };
        let Some(second_syllables) = syllables_by_abbreviation_key.get(&second_key) else {
            continue;
        };
        full_codes_split_as_two_abbreviations += 1;
        let labeled_paths = full_syllables.len() + first_syllables.len() * second_syllables.len();
        if labeled_paths > maximum_labeled_paths_for_two_keys {
            maximum_labeled_paths_for_two_keys = labeled_paths;
            maximum_labeled_paths_code = code.clone();
        }
        if immediate_ambiguity_witness.is_none() {
            immediate_ambiguity_witness = Some(ImmediateAmbiguityWitness {
                observed: code.clone(),
                full_syllable: first_set_value(full_syllables),
                first_abbreviated_syllable: first_set_value(first_syllables),
                second_abbreviated_syllable: first_set_value(second_syllables),
            });
        }
    }

    let fibonacci_witness_key = syllables_by_abbreviation_key.keys().copied().find(|key| {
        let repeated = format!("{key}{key}");
        syllables_by_full_code.contains_key(&repeated)
    });

    Ok(AbbreviationCodebookAudit {
        distinct_pinyin_syllables: syllable_to_code.len(),
        distinct_full_codes: syllables_by_full_code.len(),
        full_code_collision_groups: syllables_by_full_code
            .values()
            .filter(|syllables| syllables.len() > 1)
            .count(),
        maximum_pinyin_labels_per_full_code: syllables_by_full_code
            .values()
            .map(BTreeSet::len)
            .max()
            .unwrap_or(0),
        abbreviation_keys: syllables_by_abbreviation_key.len(),
        ambiguous_abbreviation_keys: syllables_by_abbreviation_key
            .values()
            .filter(|syllables| syllables.len() > 1)
            .count(),
        maximum_pinyin_labels_per_abbreviation_key: syllables_by_abbreviation_key
            .values()
            .map(BTreeSet::len)
            .max()
            .unwrap_or(0),
        full_codes_split_as_two_abbreviations,
        maximum_labeled_paths_for_two_keys,
        maximum_labeled_paths_code,
        immediate_ambiguity_witness,
        fibonacci_witness_key,
    })
}

fn first_set_value(values: &BTreeSet<String>) -> String {
    values
        .first()
        .expect("a codebook group is non-empty")
        .clone()
}

/// Structural error in an abbreviation codebook audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AbbreviationAuditError {
    /// The supplied decoder lexicon was empty.
    EmptyLexicon,
    /// A lexicon row did not align pinyin syllables with stored codes.
    SyllableCountMismatch {
        /// Lexicon text identifying the invalid row.
        text: String,
        /// Number of pinyin syllables.
        pinyin_syllables: usize,
        /// Number of stored syllable codes.
        codes: usize,
    },
    /// A stored syllable code was not exactly two ASCII keys.
    NonTwoKeyCode {
        /// Pinyin syllable label.
        syllable: String,
        /// Invalid stored code.
        code: String,
    },
    /// One pinyin syllable appeared with two canonical codes.
    InconsistentSyllableCode {
        /// Repeated pinyin syllable label.
        syllable: String,
        /// First observed code.
        first_code: String,
        /// Conflicting code.
        second_code: String,
    },
}

impl fmt::Display for AbbreviationAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLexicon => write!(formatter, "简拼码本审计需要至少一个词条"),
            Self::SyllableCountMismatch {
                text,
                pinyin_syllables,
                codes,
            } => write!(
                formatter,
                "词条 {text:?} 有 {pinyin_syllables} 个拼音音节，但保存了 {codes} 个音节码"
            ),
            Self::NonTwoKeyCode { syllable, code } => {
                write!(
                    formatter,
                    "拼音音节 {syllable:?} 的标准码 {code:?} 不是两键"
                )
            }
            Self::InconsistentSyllableCode {
                syllable,
                first_code,
                second_code,
            } => write!(
                formatter,
                "拼音音节 {syllable:?} 同时映射到 {first_code:?} 和 {second_code:?}"
            ),
        }
    }
}

impl Error for AbbreviationAuditError {}

#[cfg(test)]
mod tests {
    use crate::{parse_lexicon_tsv, parse_rime_lexicon};

    use super::audit_abbreviation_codebook;

    const PUBLIC_RIME: &str = include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");

    #[test]
    fn repeated_prefix_code_constructively_refutes_unique_decodability() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
             阿\ta\t10\n\
             安\tan\t9\n\
             见\tjian\t8\n",
        )
        .unwrap();
        let audit = audit_abbreviation_codebook(&lexicon).unwrap();

        assert!(audit.unique_decodability_is_refuted());
        assert_eq!(audit.fibonacci_witness_key, Some('a'));
        assert_eq!(audit.fibonacci_boundary_parses(1), Some(1));
        assert_eq!(audit.fibonacci_boundary_parses(2), Some(2));
        assert_eq!(audit.fibonacci_boundary_parses(8), Some(34));
        let witness = audit.immediate_ambiguity_witness.unwrap();
        assert_eq!(witness.observed, "aa");
        assert_eq!(witness.full_syllable, "a");
        assert_eq!(witness.first_abbreviated_syllable, "a");
        assert_eq!(witness.second_abbreviated_syllable, "a");
    }

    #[test]
    fn pinned_public_codebook_has_stable_structural_ambiguity() {
        let lexicon = parse_rime_lexicon(PUBLIC_RIME).unwrap().entries;
        let audit = audit_abbreviation_codebook(&lexicon).unwrap();

        assert_eq!(audit.distinct_pinyin_syllables, 411);
        assert_eq!(audit.distinct_full_codes, 410);
        assert_eq!(audit.full_code_collision_groups, 1);
        assert_eq!(audit.maximum_pinyin_labels_per_full_code, 2);
        assert_eq!(audit.abbreviation_keys, 26);
        assert_eq!(audit.ambiguous_abbreviation_keys, 26);
        assert_eq!(audit.maximum_pinyin_labels_per_abbreviation_key, 26);
        assert_eq!(audit.full_codes_split_as_two_abbreviations, 410);
        assert_eq!(audit.maximum_labeled_paths_for_two_keys, 677);
        assert_eq!(audit.maximum_labeled_paths_code, "ll");
        assert_eq!(audit.fibonacci_witness_key, Some('a'));
        assert_eq!(audit.fibonacci_boundary_parses(16), Some(1_597));
        assert_eq!(audit.fibonacci_boundary_parses(32), Some(3_524_578));
        assert!(audit.unique_decodability_is_refuted());
    }
}
