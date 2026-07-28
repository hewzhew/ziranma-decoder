//! Public, read-only laboratory for explicit Tab stroke filtering.
//!
//! The laboratory freezes one exact full-code single-character pool, then
//! applies a stroke prefix as a stable filter. It neither learns input nor
//! claims to reproduce the ordering of an installed IME.

use std::error::Error;
use std::fmt;

use crate::{
    CharacterShapeIndex, LexiconEntry, PinyinEncodeError, encode_pinyin_phrase,
    single_character_pool::SingleCharacterPoolIndex,
};

/// Largest candidate slice rendered by the command-line laboratory.
pub const MAX_SHAPE_LAB_VISIBLE: usize = 10;

/// One candidate surviving a shape-laboratory filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShapeLabCandidate {
    /// Candidate character.
    pub character: char,
    /// One-based rank in the frozen ordinary candidate pool.
    pub original_rank: usize,
    /// One-based rank after stable shape filtering.
    pub filtered_rank: usize,
}

/// One immutable laboratory view for a phonetic reading and stroke prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeLabSnapshot {
    /// User-supplied tone-free single-syllable pinyin.
    pub pinyin: String,
    /// Canonical full Ziranma code for the syllable.
    pub phonetic_code: String,
    /// Possibly empty five-stroke prefix using `h/s/p/n/z`.
    pub stroke_prefix: String,
    /// Candidates in the exact-code pool before filtering.
    pub ordinary_pool_size: usize,
    /// Candidates in the ordinary pool with public stroke metadata.
    pub candidates_with_stroke_data: usize,
    /// Candidates surviving the complete prefix, before display truncation.
    pub filtered_pool_size: usize,
    /// Stable-order visible slice of surviving candidates.
    pub candidates: Vec<ShapeLabCandidate>,
    /// Optional target supplied only for an explicit public check.
    pub expected_character: Option<char>,
    /// Target rank in the ordinary pool, if present there.
    pub expected_ordinary_rank: Option<usize>,
    /// Target rank after filtering, if it survives.
    pub expected_filtered_rank: Option<usize>,
    /// Public accepted stroke codes for the target.
    pub expected_stroke_codes: Vec<String>,
    /// Projection using phonetic letters and one final selection. A non-empty
    /// prefix additionally counts Tab and every shape letter.
    pub projected_actions_one_selection: usize,
}

/// Validation failure from the public shape laboratory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShapeLabError {
    /// Pinyin could not be encoded by the baseline mapping.
    Pinyin(PinyinEncodeError),
    /// The laboratory accepts exactly one pinyin syllable.
    RequiresSingleSyllable { received: usize },
    /// The requested exact-code pool does not exist in the public snapshot.
    MissingPublicPool { code: String },
    /// A stroke prefix contained an unsupported letter.
    InvalidStrokePrefix { prefix: String },
    /// Display limit was zero or exceeded the public cap.
    InvalidVisibleLimit { limit: usize },
}

impl fmt::Display for ShapeLabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pinyin(error) => error.fmt(formatter),
            Self::RequiresSingleSyllable { received } => write!(
                formatter,
                "shape-lab 只接受一个拼音音节，实际收到 {received} 个"
            ),
            Self::MissingPublicPool { code } => {
                write!(formatter, "公开词典中没有完整双拼码 {code:?} 的单字候选池")
            }
            Self::InvalidStrokePrefix { prefix } => write!(
                formatter,
                "笔画前缀 {prefix:?} 无效；只能使用 h（横）、s（竖）、p（撇）、n（捺）、z（折）"
            ),
            Self::InvalidVisibleLimit { limit } => write!(
                formatter,
                "显示数必须是 1～{MAX_SHAPE_LAB_VISIBLE}，实际收到 {limit}"
            ),
        }
    }
}

impl Error for ShapeLabError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Pinyin(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PinyinEncodeError> for ShapeLabError {
    fn from(error: PinyinEncodeError) -> Self {
        Self::Pinyin(error)
    }
}

/// Frozen public exact-code candidate pools plus audited stroke metadata.
pub struct ShapeLab<'a> {
    pools: SingleCharacterPoolIndex,
    shapes: &'a CharacterShapeIndex,
}

impl<'a> ShapeLab<'a> {
    /// Builds a read-only laboratory from public lexicon and shape snapshots.
    pub fn new(entries: &[LexiconEntry], shapes: &'a CharacterShapeIndex) -> Self {
        Self {
            pools: SingleCharacterPoolIndex::new(entries),
            shapes,
        }
    }

    /// Freezes one ordinary pool and applies a possibly empty stroke prefix.
    pub fn snapshot(
        &self,
        pinyin: &str,
        stroke_prefix: &str,
        expected_character: Option<char>,
        visible_limit: usize,
    ) -> Result<ShapeLabSnapshot, ShapeLabError> {
        if !(1..=MAX_SHAPE_LAB_VISIBLE).contains(&visible_limit) {
            return Err(ShapeLabError::InvalidVisibleLimit {
                limit: visible_limit,
            });
        }
        if !stroke_prefix
            .as_bytes()
            .iter()
            .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z'))
        {
            return Err(ShapeLabError::InvalidStrokePrefix {
                prefix: stroke_prefix.to_owned(),
            });
        }

        let encoded = encode_pinyin_phrase(pinyin)?;
        if encoded.syllable_codes.len() != 1 {
            return Err(ShapeLabError::RequiresSingleSyllable {
                received: encoded.syllable_codes.len(),
            });
        }
        let phonetic_code = encoded.full_code.as_str();
        let pool =
            self.pools
                .pool(phonetic_code)
                .ok_or_else(|| ShapeLabError::MissingPublicPool {
                    code: phonetic_code.to_owned(),
                })?;

        let candidates_with_stroke_data = pool
            .iter()
            .filter(|candidate| {
                self.shapes
                    .get(candidate.character)
                    .is_some_and(|shape| !shape.stroke_codes().is_empty())
            })
            .count();
        let mut filtered_pool_size = 0usize;
        let mut candidates = Vec::with_capacity(visible_limit.min(pool.len()));
        let mut expected_filtered_rank = None;
        for (original_index, candidate) in pool.iter().enumerate() {
            if !stroke_prefix.is_empty()
                && !self.shapes.get(candidate.character).is_some_and(|shape| {
                    shape
                        .stroke_codes()
                        .iter()
                        .any(|code| code.starts_with(stroke_prefix))
                })
            {
                continue;
            }
            filtered_pool_size += 1;
            if expected_character == Some(candidate.character) {
                expected_filtered_rank = Some(filtered_pool_size);
            }
            if candidates.len() < visible_limit {
                candidates.push(ShapeLabCandidate {
                    character: candidate.character,
                    original_rank: original_index + 1,
                    filtered_rank: filtered_pool_size,
                });
            }
        }

        let expected_ordinary_rank =
            expected_character.and_then(|character| self.pools.rank(phonetic_code, character));
        let expected_stroke_codes = expected_character
            .and_then(|character| self.shapes.get(character))
            .map(|shape| shape.stroke_codes().to_vec())
            .unwrap_or_default();
        let projected_actions_one_selection =
            phonetic_code.len() + usize::from(!stroke_prefix.is_empty()) + stroke_prefix.len() + 1;

        Ok(ShapeLabSnapshot {
            pinyin: pinyin.to_owned(),
            phonetic_code: phonetic_code.to_owned(),
            stroke_prefix: stroke_prefix.to_owned(),
            ordinary_pool_size: pool.len(),
            candidates_with_stroke_data,
            filtered_pool_size,
            candidates,
            expected_character,
            expected_ordinary_rank,
            expected_filtered_rank,
            expected_stroke_codes,
            projected_actions_one_selection,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ShapeLab, ShapeLabError};
    use crate::{CharacterShape, CharacterShapeIndex, KeySequence, LexiconEntry};

    fn entry(character: char, frequency: u64) -> LexiconEntry {
        LexiconEntry {
            text: character.to_string(),
            pinyin: "shi".to_owned(),
            code: KeySequence::new("ui").unwrap(),
            syllable_codes: vec![KeySequence::new("ui").unwrap()],
            frequency,
        }
    }

    fn fixture() -> (Vec<LexiconEntry>, CharacterShapeIndex) {
        let entries = vec![entry('甲', 30), entry('乙', 20), entry('丙', 10)];
        let shapes = CharacterShapeIndex::new([
            CharacterShape::new('甲', vec!["hsp".to_owned()], Vec::new()).unwrap(),
            CharacterShape::new('乙', vec!["nhh".to_owned()], Vec::new()).unwrap(),
            CharacterShape::new('丙', vec!["nsh".to_owned()], Vec::new()).unwrap(),
        ])
        .unwrap();
        (entries, shapes)
    }

    #[test]
    fn stable_filter_preserves_original_order_and_reports_target_rank() {
        let (entries, shapes) = fixture();
        let lab = ShapeLab::new(&entries, &shapes);
        let snapshot = lab.snapshot("shi", "n", Some('丙'), 10).unwrap();

        assert_eq!(snapshot.phonetic_code, "ui");
        assert_eq!(snapshot.ordinary_pool_size, 3);
        assert_eq!(snapshot.filtered_pool_size, 2);
        assert_eq!(snapshot.expected_ordinary_rank, Some(3));
        assert_eq!(snapshot.expected_filtered_rank, Some(2));
        assert_eq!(snapshot.projected_actions_one_selection, 5);
        assert_eq!(
            snapshot
                .candidates
                .iter()
                .map(|candidate| (candidate.character, candidate.original_rank))
                .collect::<Vec<_>>(),
            vec![('乙', 2), ('丙', 3)]
        );
    }

    #[test]
    fn empty_prefix_is_the_ordinary_pool_without_tab() {
        let (entries, shapes) = fixture();
        let lab = ShapeLab::new(&entries, &shapes);
        let snapshot = lab.snapshot("shi", "", Some('甲'), 2).unwrap();

        assert_eq!(snapshot.filtered_pool_size, 3);
        assert_eq!(snapshot.candidates.len(), 2);
        assert_eq!(snapshot.expected_filtered_rank, Some(1));
        assert_eq!(snapshot.projected_actions_one_selection, 3);
    }

    #[test]
    fn rejects_multisyllable_readings_and_invalid_shape_letters() {
        let (entries, shapes) = fixture();
        let lab = ShapeLab::new(&entries, &shapes);

        assert_eq!(
            lab.snapshot("shi shi", "", None, 10).unwrap_err(),
            ShapeLabError::RequiresSingleSyllable { received: 2 }
        );
        assert_eq!(
            lab.snapshot("shi", "x", None, 10).unwrap_err(),
            ShapeLabError::InvalidStrokePrefix {
                prefix: "x".to_owned()
            }
        );
    }
}
