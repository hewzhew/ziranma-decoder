//! Strict import of public five-stroke sequence tables.
//!
//! The parser is deliberately source-neutral: it accepts a small documented
//! TSV shape and does not know about a particular upstream repository. Public
//! snapshots, their licensing, and their exact import statistics live beside
//! the vendored data rather than being hidden in this module.

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{CharacterShape, CharacterShapeIndex, LexiconEntry, ShapeRefinementError};

/// Maximum number of non-comment data rows accepted by one import.
pub const MAX_STROKE_DATA_ROWS: usize = 100_000;
/// Maximum number of Unicode scalar assignments accepted by one import.
pub const MAX_STROKE_DATA_ASSIGNMENTS: usize = 500_000;
/// Maximum length of one numeric five-stroke sequence.
pub const MAX_STROKE_SEQUENCE_LENGTH: usize = 128;
/// Maximum number of alternative sequences retained for one character.
pub const MAX_STROKE_SEQUENCES_PER_CHARACTER: usize = 128;
/// Maximum UTF-8 byte length of one physical input line.
pub const MAX_STROKE_DATA_LINE_BYTES: usize = 16_384;

/// Deterministic accounting for one accepted stroke table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrokeSequenceImportStats {
    /// Accepted non-comment rows.
    pub data_rows: usize,
    /// Character-to-sequence assignments across all rows.
    pub character_assignments: usize,
    /// Distinct characters with at least one sequence.
    pub distinct_characters: usize,
    /// Characters with two or more accepted alternative sequences.
    pub characters_with_alternative_sequences: usize,
    /// Largest alternative-sequence count for one character.
    pub maximum_sequences_per_character: usize,
    /// Longest accepted numeric sequence.
    pub maximum_sequence_length: usize,
    /// Largest number of characters sharing one sequence row.
    pub maximum_characters_per_row: usize,
}

/// Parsed stroke metadata plus its complete import accounting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrokeSequenceImport {
    shapes: Vec<CharacterShape>,
    stats: StrokeSequenceImportStats,
}

/// Coverage of shape metadata over one already imported lexicon.
///
/// Only CJK Unified Ideographs and U+3007 are eligible. Punctuation, Latin
/// text, kana, and compatibility ideographs are outside this audit rather than
/// being mislabeled as missing stroke data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LexiconShapeCoverageStats {
    /// Distinct eligible characters occurring anywhere in accepted entries.
    pub distinct_eligible_characters: usize,
    /// Distinct eligible characters present in the shape index.
    pub covered_distinct_characters: usize,
    /// Distinct eligible characters absent from the shape index.
    pub uncovered_distinct_characters: usize,
    /// Eligible character occurrences across accepted lexicon entries.
    pub eligible_character_occurrences: usize,
    /// Eligible occurrences whose character is present in the shape index.
    pub covered_character_occurrences: usize,
    /// Accepted entries containing exactly one eligible character and nothing else.
    pub single_character_entries: usize,
    /// Single-character entries whose character is present in the shape index.
    pub covered_single_character_entries: usize,
}

impl StrokeSequenceImport {
    /// Returns character records in Unicode scalar order.
    pub fn shapes(&self) -> &[CharacterShape] {
        &self.shapes
    }

    /// Consumes the import and returns character records in scalar order.
    pub fn into_shapes(self) -> Vec<CharacterShape> {
        self.shapes
    }

    /// Returns deterministic import statistics.
    pub fn stats(&self) -> &StrokeSequenceImportStats {
        &self.stats
    }
}

/// Strict validation error for a `sequence<TAB>characters` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StrokeDataParseError {
    /// No non-comment data row was present.
    EmptyData,
    /// A physical line exceeded the defensive byte limit.
    LineTooLong {
        /// One-based physical line number.
        line: usize,
        /// Observed UTF-8 byte length.
        bytes: usize,
        /// Configured limit.
        limit: usize,
    },
    /// A data line did not have exactly two tab-separated fields.
    InvalidFieldCount {
        /// One-based physical line number.
        line: usize,
    },
    /// A sequence was empty or contained something other than `1` through `5`.
    InvalidSequence {
        /// One-based physical line number.
        line: usize,
    },
    /// A sequence exceeded the defensive character limit.
    SequenceTooLong {
        /// One-based physical line number.
        line: usize,
        /// Observed ASCII sequence length.
        length: usize,
        /// Configured limit.
        limit: usize,
    },
    /// The same numeric sequence appeared in more than one row.
    DuplicateSequence {
        /// One-based physical line number of the duplicate.
        line: usize,
    },
    /// The character field was empty.
    EmptyCharacters {
        /// One-based physical line number.
        line: usize,
    },
    /// A character field contained whitespace or a control scalar.
    InvalidCharacter {
        /// One-based physical line number.
        line: usize,
        /// Rejected scalar.
        character: char,
    },
    /// One row repeated the same character.
    DuplicateCharacterInRow {
        /// One-based physical line number.
        line: usize,
        /// Repeated scalar.
        character: char,
    },
    /// The import exceeded the non-comment row limit.
    TooManyRows {
        /// Configured limit.
        limit: usize,
    },
    /// The import exceeded the total assignment limit.
    TooManyAssignments {
        /// Configured limit.
        limit: usize,
    },
    /// One character exceeded the alternative-sequence limit.
    TooManySequencesForCharacter {
        /// Character with too many alternatives.
        character: char,
        /// Configured limit.
        limit: usize,
    },
    /// Accepted rows could not be represented as shape metadata.
    InvalidShapeMetadata(ShapeRefinementError),
}

impl fmt::Display for StrokeDataParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyData => formatter.write_str("笔画表没有数据行"),
            Self::LineTooLong { line, bytes, limit } => {
                write!(formatter, "第 {line} 行有 {bytes} 字节，超过上限 {limit}")
            }
            Self::InvalidFieldCount { line } => {
                write!(formatter, "第 {line} 行必须恰有两个 Tab 分隔字段")
            }
            Self::InvalidSequence { line } => {
                write!(formatter, "第 {line} 行的笔画序列必须是非空 1/2/3/4/5 串")
            }
            Self::SequenceTooLong {
                line,
                length,
                limit,
            } => write!(
                formatter,
                "第 {line} 行的笔画序列长度 {length} 超过上限 {limit}"
            ),
            Self::DuplicateSequence { line } => {
                write!(formatter, "第 {line} 行重复了已有笔画序列")
            }
            Self::EmptyCharacters { line } => write!(formatter, "第 {line} 行没有字符"),
            Self::InvalidCharacter { line, character } => {
                write!(formatter, "第 {line} 行含无效字符 {character:?}")
            }
            Self::DuplicateCharacterInRow { line, character } => {
                write!(formatter, "第 {line} 行重复了字符 {character}")
            }
            Self::TooManyRows { limit } => write!(formatter, "笔画表数据行超过上限 {limit}"),
            Self::TooManyAssignments { limit } => {
                write!(formatter, "笔画表字符分配数超过上限 {limit}")
            }
            Self::TooManySequencesForCharacter { character, limit } => {
                write!(formatter, "{character} 的替代笔顺超过上限 {limit}")
            }
            Self::InvalidShapeMetadata(error) => write!(formatter, "形状记录无效：{error}"),
        }
    }
}

impl Error for StrokeDataParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidShapeMetadata(error) => Some(error),
            _ => None,
        }
    }
}

/// Parses a strict `numeric-sequence<TAB>characters` table.
///
/// Empty lines and lines beginning with `#` are ignored. Numeric stroke
/// categories map deterministically to the Sogou-compatible keys
/// `1→h, 2→s, 3→p, 4→n, 5→z`. A character may occur under multiple distinct
/// sequences; those alternatives are retained and sorted lexicographically.
pub fn parse_stroke_sequence_tsv(
    input: &str,
) -> Result<StrokeSequenceImport, StrokeDataParseError> {
    let mut sequences = HashSet::new();
    let mut by_character = BTreeMap::<char, Vec<String>>::new();
    let mut data_rows = 0usize;
    let mut character_assignments = 0usize;
    let mut maximum_sequence_length = 0usize;
    let mut maximum_characters_per_row = 0usize;

    for (offset, line) in input.lines().enumerate() {
        let line_number = offset + 1;
        if line.len() > MAX_STROKE_DATA_LINE_BYTES {
            return Err(StrokeDataParseError::LineTooLong {
                line: line_number,
                bytes: line.len(),
                limit: MAX_STROKE_DATA_LINE_BYTES,
            });
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        data_rows += 1;
        if data_rows > MAX_STROKE_DATA_ROWS {
            return Err(StrokeDataParseError::TooManyRows {
                limit: MAX_STROKE_DATA_ROWS,
            });
        }

        let mut fields = line.split('\t');
        let sequence = fields.next().expect("split always yields one field");
        let characters = fields
            .next()
            .ok_or(StrokeDataParseError::InvalidFieldCount { line: line_number })?;
        if fields.next().is_some() {
            return Err(StrokeDataParseError::InvalidFieldCount { line: line_number });
        }
        if sequence.is_empty()
            || !sequence
                .as_bytes()
                .iter()
                .all(|byte| matches!(byte, b'1'..=b'5'))
        {
            return Err(StrokeDataParseError::InvalidSequence { line: line_number });
        }
        if sequence.len() > MAX_STROKE_SEQUENCE_LENGTH {
            return Err(StrokeDataParseError::SequenceTooLong {
                line: line_number,
                length: sequence.len(),
                limit: MAX_STROKE_SEQUENCE_LENGTH,
            });
        }
        if !sequences.insert(sequence) {
            return Err(StrokeDataParseError::DuplicateSequence { line: line_number });
        }
        if characters.is_empty() {
            return Err(StrokeDataParseError::EmptyCharacters { line: line_number });
        }

        let stroke_code = numeric_sequence_to_keys(sequence);
        let mut row_characters = HashSet::new();
        let mut row_assignments = 0usize;
        for character in characters.chars() {
            if character.is_whitespace() || character.is_control() {
                return Err(StrokeDataParseError::InvalidCharacter {
                    line: line_number,
                    character,
                });
            }
            if !row_characters.insert(character) {
                return Err(StrokeDataParseError::DuplicateCharacterInRow {
                    line: line_number,
                    character,
                });
            }
            character_assignments += 1;
            row_assignments += 1;
            if character_assignments > MAX_STROKE_DATA_ASSIGNMENTS {
                return Err(StrokeDataParseError::TooManyAssignments {
                    limit: MAX_STROKE_DATA_ASSIGNMENTS,
                });
            }
            by_character
                .entry(character)
                .or_default()
                .push(stroke_code.clone());
        }
        maximum_sequence_length = maximum_sequence_length.max(sequence.len());
        maximum_characters_per_row = maximum_characters_per_row.max(row_assignments);
    }

    if data_rows == 0 {
        return Err(StrokeDataParseError::EmptyData);
    }

    let mut characters_with_alternative_sequences = 0usize;
    let mut maximum_sequences_per_character = 0usize;
    let mut shapes = Vec::with_capacity(by_character.len());
    for (character, mut stroke_codes) in by_character {
        stroke_codes.sort_unstable();
        if stroke_codes.len() > MAX_STROKE_SEQUENCES_PER_CHARACTER {
            return Err(StrokeDataParseError::TooManySequencesForCharacter {
                character,
                limit: MAX_STROKE_SEQUENCES_PER_CHARACTER,
            });
        }
        if stroke_codes.len() > 1 {
            characters_with_alternative_sequences += 1;
        }
        maximum_sequences_per_character = maximum_sequences_per_character.max(stroke_codes.len());
        shapes.push(
            CharacterShape::new(character, stroke_codes, Vec::new())
                .map_err(StrokeDataParseError::InvalidShapeMetadata)?,
        );
    }

    let stats = StrokeSequenceImportStats {
        data_rows,
        character_assignments,
        distinct_characters: shapes.len(),
        characters_with_alternative_sequences,
        maximum_sequences_per_character,
        maximum_sequence_length,
        maximum_characters_per_row,
    };
    Ok(StrokeSequenceImport { shapes, stats })
}

/// Audits a frozen character-shape index against accepted lexicon entries.
///
/// Counts are unweighted structural coverage. Dictionary weights are not
/// treated as real-world usage frequencies.
pub fn audit_lexicon_shape_coverage(
    entries: &[LexiconEntry],
    shapes: &CharacterShapeIndex,
) -> LexiconShapeCoverageStats {
    let mut eligible_characters = HashSet::new();
    let mut covered_characters = HashSet::new();
    let mut stats = LexiconShapeCoverageStats::default();

    for entry in entries {
        let characters = entry.text.chars().collect::<Vec<_>>();
        if characters.len() == 1 && is_shape_target_character(characters[0]) {
            stats.single_character_entries += 1;
            if shapes.contains(characters[0]) {
                stats.covered_single_character_entries += 1;
            }
        }
        for character in characters {
            if !is_shape_target_character(character) {
                continue;
            }
            stats.eligible_character_occurrences += 1;
            eligible_characters.insert(character);
            if shapes.contains(character) {
                stats.covered_character_occurrences += 1;
                covered_characters.insert(character);
            }
        }
    }

    stats.distinct_eligible_characters = eligible_characters.len();
    stats.covered_distinct_characters = covered_characters.len();
    stats.uncovered_distinct_characters =
        eligible_characters.difference(&covered_characters).count();
    stats
}

fn numeric_sequence_to_keys(sequence: &str) -> String {
    sequence
        .bytes()
        .map(|byte| match byte {
            b'1' => 'h',
            b'2' => 's',
            b'3' => 'p',
            b'4' => 'n',
            b'5' => 'z',
            _ => unreachable!("numeric sequence was validated"),
        })
        .collect()
}

fn is_shape_target_character(character: char) -> bool {
    matches!(
        character as u32,
        0x3007 | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x20000..=0x323af
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{StrokeDataParseError, audit_lexicon_shape_coverage, parse_stroke_sequence_tsv};
    use crate::{
        CharacterShapeIndex, ShapeMatchEvidence, TabShapeQuery, parse_lexicon_tsv,
        parse_rime_lexicon,
    };

    #[test]
    fn strict_import_maps_digits_and_retains_alternative_sequences() {
        let imported = parse_stroke_sequence_tsv(
            "# synthetic public fixture\n\
             1\t一\n\
             12\t甲乙\n\
             15\t甲\n",
        )
        .unwrap();

        assert_eq!(imported.stats().data_rows, 3);
        assert_eq!(imported.stats().character_assignments, 4);
        assert_eq!(imported.stats().distinct_characters, 3);
        assert_eq!(imported.stats().characters_with_alternative_sequences, 1);
        assert_eq!(imported.stats().maximum_sequences_per_character, 2);
        assert_eq!(imported.stats().maximum_sequence_length, 2);
        assert_eq!(imported.stats().maximum_characters_per_row, 2);

        let shape = imported
            .shapes()
            .iter()
            .find(|shape| shape.character() == '甲')
            .unwrap();
        assert_eq!(shape.stroke_codes(), &["hs".to_owned(), "hz".to_owned()]);

        let index = CharacterShapeIndex::new(imported.into_shapes()).unwrap();
        let report = index.refine(["乙", "甲"], 0, &TabShapeQuery::parse("jw\thz").unwrap());
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].text, "甲");
        assert!(matches!(
            report.matches[0].evidence.as_slice(),
            [ShapeMatchEvidence::StrokePrefix { code }] if code == "hz"
        ));
    }

    #[test]
    fn malformed_rows_and_duplicates_are_rejected_with_line_numbers() {
        assert_eq!(
            parse_stroke_sequence_tsv("# only comments\n").unwrap_err(),
            StrokeDataParseError::EmptyData
        );
        assert_eq!(
            parse_stroke_sequence_tsv("1\t一\textra\n").unwrap_err(),
            StrokeDataParseError::InvalidFieldCount { line: 1 }
        );
        assert_eq!(
            parse_stroke_sequence_tsv("16\t一\n").unwrap_err(),
            StrokeDataParseError::InvalidSequence { line: 1 }
        );
        assert_eq!(
            parse_stroke_sequence_tsv("1\t一\n1\t乙\n").unwrap_err(),
            StrokeDataParseError::DuplicateSequence { line: 2 }
        );
        assert_eq!(
            parse_stroke_sequence_tsv("1\t一一\n").unwrap_err(),
            StrokeDataParseError::DuplicateCharacterInRow {
                line: 1,
                character: '一'
            }
        );
    }

    #[test]
    fn pinned_conway_snapshot_has_stable_import_stats() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/public/conway-stroke-data/sequence-characters.txt");
        let input = std::fs::read_to_string(path).unwrap();
        assert_eq!(input.len(), 1_188_129);

        let imported = parse_stroke_sequence_tsv(&input).unwrap();
        assert_eq!(imported.stats().data_rows, 60_227);
        assert_eq!(imported.stats().character_assignments, 63_005);
        assert_eq!(imported.stats().distinct_characters, 28_165);
        assert_eq!(
            imported.stats().characters_with_alternative_sequences,
            14_176
        );
        assert_eq!(imported.stats().maximum_sequences_per_character, 90);
        assert_eq!(imported.stats().maximum_sequence_length, 52);
        assert_eq!(imported.stats().maximum_characters_per_row, 9);
    }

    #[test]
    fn coverage_excludes_non_ideographs_and_never_uses_dictionary_weights() {
        let imported = parse_stroke_sequence_tsv("1\t一\n12\t甲\n").unwrap();
        let index = CharacterShapeIndex::new(imported.into_shapes()).unwrap();
        let entries = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
             一\tyi\t1\n\
             甲乙\tjia yi\t999999\n\
             A\ta\t50\n",
        )
        .unwrap();

        let stats = audit_lexicon_shape_coverage(&entries, &index);
        assert_eq!(stats.distinct_eligible_characters, 3);
        assert_eq!(stats.covered_distinct_characters, 2);
        assert_eq!(stats.uncovered_distinct_characters, 1);
        assert_eq!(stats.eligible_character_occurrences, 3);
        assert_eq!(stats.covered_character_occurrences, 2);
        assert_eq!(stats.single_character_entries, 1);
        assert_eq!(stats.covered_single_character_entries, 1);
    }

    #[test]
    fn pinned_public_lexicon_has_stable_shape_coverage() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let stroke_input = std::fs::read_to_string(
            root.join("data/public/conway-stroke-data/sequence-characters.txt"),
        )
        .unwrap();
        let rime_input = std::fs::read_to_string(
            root.join("data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"),
        )
        .unwrap();
        let shapes = CharacterShapeIndex::new(
            parse_stroke_sequence_tsv(&stroke_input)
                .unwrap()
                .into_shapes(),
        )
        .unwrap();
        let lexicon = parse_rime_lexicon(&rime_input).unwrap();

        let stats = audit_lexicon_shape_coverage(&lexicon.entries, &shapes);
        assert_eq!(stats.distinct_eligible_characters, 16_469);
        assert_eq!(stats.covered_distinct_characters, 16_469);
        assert_eq!(stats.uncovered_distinct_characters, 0);
        assert_eq!(stats.eligible_character_occurrences, 133_827);
        assert_eq!(stats.covered_character_occurrences, 133_827);
        assert_eq!(stats.single_character_entries, 17_038);
        assert_eq!(stats.covered_single_character_entries, 17_038);
    }
}
