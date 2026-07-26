//! Explicit Tab-triggered shape refinement for an already generated candidate pool.
//!
//! This module deliberately does not generate phonetic candidates or change
//! their scores. It models the small, inspectable second-stage filter used
//! only after the user explicitly presses Tab.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::KeySequence;

const STROKE_KEYS: &str = "hspnz";

/// A validated `phonetic<Tab>shape-prefix` refinement request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabShapeQuery {
    phonetic_keys: KeySequence,
    shape_prefix: String,
}

impl TabShapeQuery {
    /// Parses exactly one literal Tab separator.
    ///
    /// The shape prefix may be empty immediately after Tab. This represents
    /// entering refinement mode before the first shape key is typed.
    pub fn parse(value: &str) -> Result<Self, ShapeRefinementError> {
        let Some((phonetic, shape_prefix)) = value.split_once('\t') else {
            return Err(ShapeRefinementError::MissingTab);
        };
        if shape_prefix.contains('\t') {
            return Err(ShapeRefinementError::MultipleTabs);
        }
        let phonetic_keys =
            KeySequence::new(phonetic).map_err(|_| ShapeRefinementError::InvalidPhoneticKeys)?;
        if !shape_prefix
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_lowercase())
        {
            return Err(ShapeRefinementError::InvalidShapePrefix);
        }
        Ok(Self {
            phonetic_keys,
            shape_prefix: shape_prefix.to_owned(),
        })
    }

    /// Returns the ordinary double-pinyin keys before Tab.
    pub fn phonetic_keys(&self) -> &KeySequence {
        &self.phonetic_keys
    }

    /// Returns the possibly empty shape prefix after Tab.
    pub fn shape_prefix(&self) -> &str {
        &self.shape_prefix
    }
}

/// Audited shape metadata for one character.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterShape {
    character: char,
    stroke_codes: Vec<String>,
    component_codes: Vec<String>,
}

impl CharacterShape {
    /// Constructs one character annotation.
    ///
    /// Stroke codes use `h/s/p/n/z`; multiple codes preserve accepted
    /// alternative stroke orders. Component codes contain the lowercase
    /// initials of an ordered decomposition. At least one code is required.
    pub fn new(
        character: char,
        stroke_codes: Vec<String>,
        component_codes: Vec<String>,
    ) -> Result<Self, ShapeRefinementError> {
        if stroke_codes.iter().any(|code| !valid_stroke_code(code)) {
            return Err(ShapeRefinementError::InvalidStrokeCode { character });
        }
        if component_codes
            .iter()
            .any(|code| !valid_component_code(code))
        {
            return Err(ShapeRefinementError::InvalidComponentCode { character });
        }
        if stroke_codes.is_empty() && component_codes.is_empty() {
            return Err(ShapeRefinementError::MissingShapeCode { character });
        }
        let mut unique = HashSet::new();
        if stroke_codes
            .iter()
            .any(|code| !unique.insert(code.as_str()))
        {
            return Err(ShapeRefinementError::DuplicateStrokeCode { character });
        }
        unique.clear();
        if component_codes
            .iter()
            .any(|code| !unique.insert(code.as_str()))
        {
            return Err(ShapeRefinementError::DuplicateComponentCode { character });
        }
        Ok(Self {
            character,
            stroke_codes,
            component_codes,
        })
    }

    /// Returns the annotated character.
    pub fn character(&self) -> char {
        self.character
    }

    /// Returns every accepted full five-stroke code.
    pub fn stroke_codes(&self) -> &[String] {
        &self.stroke_codes
    }

    /// Returns all accepted ordered component-initial codes.
    pub fn component_codes(&self) -> &[String] {
        &self.component_codes
    }
}

/// Why one candidate survived a non-empty shape prefix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShapeMatchEvidence {
    /// The supplied prefix matches the character's five-stroke code.
    StrokePrefix {
        /// Complete audited stroke code.
        code: String,
    },
    /// The supplied prefix matches one accepted component decomposition.
    ComponentPrefix {
        /// Complete audited component-initial code.
        code: String,
    },
}

/// One candidate retained by explicit Tab refinement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefinedCandidate<'a> {
    /// Stable zero-based position in the original candidate pool.
    pub original_index: usize,
    /// Complete original candidate text.
    pub text: &'a str,
    /// Character inspected at the requested text position, if present.
    pub target_character: Option<char>,
    /// Every independent reason the candidate matched.
    ///
    /// This is empty only while Tab mode has an empty shape prefix.
    pub evidence: Vec<ShapeMatchEvidence>,
}

/// Inspectable accounting for one refinement operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabShapeRefinementReport<'a> {
    /// Number of ordinary candidates examined.
    pub candidates_examined: usize,
    /// Candidates that contained a character at the requested position.
    pub candidates_with_target: usize,
    /// Candidates whose target character had audited shape metadata.
    pub candidates_with_shape_data: usize,
    /// Stable-order retained candidates.
    pub matches: Vec<RefinedCandidate<'a>>,
}

/// Fixed, deterministic character-shape lookup.
#[derive(Clone, Debug, Default)]
pub struct CharacterShapeIndex {
    shapes: HashMap<char, CharacterShape>,
}

impl CharacterShapeIndex {
    /// Builds an index and rejects duplicate character annotations.
    pub fn new(
        shapes: impl IntoIterator<Item = CharacterShape>,
    ) -> Result<Self, ShapeRefinementError> {
        let mut indexed = HashMap::new();
        for shape in shapes {
            let character = shape.character();
            if indexed.insert(character, shape).is_some() {
                return Err(ShapeRefinementError::DuplicateCharacter { character });
            }
        }
        Ok(Self { shapes: indexed })
    }

    /// Returns the number of distinct annotated characters.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// Returns whether no character has shape metadata.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Returns whether one character has any accepted shape metadata.
    pub fn contains(&self, character: char) -> bool {
        self.shapes.contains_key(&character)
    }

    /// Refines an existing candidate pool without changing its order or score.
    ///
    /// The unmarked suffix is interpreted against both stroke and component
    /// codes. A candidate survives if either interpretation has the supplied
    /// prefix. This avoids inventing an undocumented precedence rule for
    /// letters such as `h/s/p/n/z`, which are valid in both alphabets.
    pub fn refine<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a str>,
        target_character_index: usize,
        query: &TabShapeQuery,
    ) -> TabShapeRefinementReport<'a> {
        let mut report = TabShapeRefinementReport {
            candidates_examined: 0,
            candidates_with_target: 0,
            candidates_with_shape_data: 0,
            matches: Vec::new(),
        };

        for (original_index, text) in candidates.into_iter().enumerate() {
            report.candidates_examined += 1;
            let target_character = text.chars().nth(target_character_index);
            if target_character.is_some() {
                report.candidates_with_target += 1;
            }
            let shape = target_character.and_then(|character| self.shapes.get(&character));
            if shape.is_some() {
                report.candidates_with_shape_data += 1;
            }

            if query.shape_prefix().is_empty() {
                report.matches.push(RefinedCandidate {
                    original_index,
                    text,
                    target_character,
                    evidence: Vec::new(),
                });
                continue;
            }

            let Some(shape) = shape else {
                continue;
            };
            let mut evidence = Vec::new();
            for code in shape.stroke_codes() {
                if code.starts_with(query.shape_prefix()) {
                    evidence.push(ShapeMatchEvidence::StrokePrefix { code: code.clone() });
                }
            }
            for code in shape.component_codes() {
                if code.starts_with(query.shape_prefix()) {
                    evidence.push(ShapeMatchEvidence::ComponentPrefix { code: code.clone() });
                }
            }
            if !evidence.is_empty() {
                report.matches.push(RefinedCandidate {
                    original_index,
                    text,
                    target_character,
                    evidence,
                });
            }
        }
        report
    }
}

/// Validation error for the explicit Tab refinement protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShapeRefinementError {
    /// The request did not contain the explicit Tab delimiter.
    MissingTab,
    /// More than one Tab delimiter was supplied.
    MultipleTabs,
    /// The phonetic portion was not a non-empty lowercase key sequence.
    InvalidPhoneticKeys,
    /// The suffix contained something other than lowercase ASCII letters.
    InvalidShapePrefix,
    /// A stroke annotation was empty or used a key outside `h/s/p/n/z`.
    InvalidStrokeCode {
        /// Character whose annotation was rejected.
        character: char,
    },
    /// One character repeated the same full stroke code.
    DuplicateStrokeCode {
        /// Character whose annotation was rejected.
        character: char,
    },
    /// A component annotation was empty or not lowercase ASCII.
    InvalidComponentCode {
        /// Character whose annotation was rejected.
        character: char,
    },
    /// A character had neither stroke nor component metadata.
    MissingShapeCode {
        /// Character whose annotation was rejected.
        character: char,
    },
    /// One character repeated the same component decomposition.
    DuplicateComponentCode {
        /// Character whose annotation was rejected.
        character: char,
    },
    /// The index received two records for the same character.
    DuplicateCharacter {
        /// Repeated character.
        character: char,
    },
}

impl fmt::Display for ShapeRefinementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTab => formatter.write_str("音形筛选请求必须包含一个 Tab"),
            Self::MultipleTabs => formatter.write_str("音形筛选请求只能包含一个 Tab"),
            Self::InvalidPhoneticKeys => formatter.write_str("Tab 前必须是非空的小写双拼按键"),
            Self::InvalidShapePrefix => formatter.write_str("Tab 后只能包含小写英文字母"),
            Self::InvalidStrokeCode { character } => {
                write!(formatter, "{character} 的笔画码必须是非空 h/s/p/n/z 串")
            }
            Self::DuplicateStrokeCode { character } => {
                write!(formatter, "{character} 含有重复笔画码")
            }
            Self::InvalidComponentCode { character } => {
                write!(formatter, "{character} 的部件码必须是非空小写字母串")
            }
            Self::MissingShapeCode { character } => {
                write!(formatter, "{character} 没有笔画码或部件码")
            }
            Self::DuplicateComponentCode { character } => {
                write!(formatter, "{character} 含有重复部件码")
            }
            Self::DuplicateCharacter { character } => {
                write!(formatter, "{character} 出现了重复音形记录")
            }
        }
    }
}

impl Error for ShapeRefinementError {}

fn valid_stroke_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .chars()
            .all(|character| STROKE_KEYS.contains(character))
}

fn valid_component_code(code: &str) -> bool {
    !code.is_empty() && code.as_bytes().iter().all(|byte| byte.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{
        CharacterShape, CharacterShapeIndex, ShapeMatchEvidence, ShapeRefinementError,
        TabShapeQuery,
    };

    fn synthetic_index() -> CharacterShapeIndex {
        CharacterShapeIndex::new([
            CharacterShape::new('甲', vec!["hspnz".to_owned()], vec!["ts".to_owned()]).unwrap(),
            CharacterShape::new('乙', vec!["zhpn".to_owned()], vec!["ns".to_owned()]).unwrap(),
            CharacterShape::new(
                '丙',
                vec!["hsp".to_owned()],
                vec!["hp".to_owned(), "bs".to_owned()],
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn query_requires_one_explicit_tab_and_keeps_an_empty_mode_prefix() {
        let query = TabShapeQuery::parse("xm\tnx").unwrap();
        assert_eq!(query.phonetic_keys().as_str(), "xm");
        assert_eq!(query.shape_prefix(), "nx");
        assert_eq!(TabShapeQuery::parse("xm\t").unwrap().shape_prefix(), "");
        assert_eq!(
            TabShapeQuery::parse("xm").unwrap_err(),
            ShapeRefinementError::MissingTab
        );
        assert_eq!(
            TabShapeQuery::parse("xm\tn\tx").unwrap_err(),
            ShapeRefinementError::MultipleTabs
        );
        assert_eq!(
            TabShapeQuery::parse("xm\tN").unwrap_err(),
            ShapeRefinementError::InvalidShapePrefix
        );
    }

    #[test]
    fn refinement_preserves_pool_order_and_reports_its_evidence() {
        let index = synthetic_index();
        let query = TabShapeQuery::parse("aa\ths").unwrap();
        let report = index.refine(["乙词", "甲词", "丙词"], 0, &query);
        assert_eq!(report.candidates_examined, 3);
        assert_eq!(report.candidates_with_target, 3);
        assert_eq!(report.candidates_with_shape_data, 3);
        assert_eq!(
            report
                .matches
                .iter()
                .map(|candidate| candidate.text)
                .collect::<Vec<_>>(),
            vec!["甲词", "丙词"]
        );
        assert_eq!(report.matches[0].original_index, 1);
        assert!(matches!(
            report.matches[0].evidence.as_slice(),
            [ShapeMatchEvidence::StrokePrefix { code }] if code == "hspnz"
        ));
    }

    #[test]
    fn overlapping_alphabets_use_auditable_union_instead_of_hidden_precedence() {
        let index = synthetic_index();
        let query = TabShapeQuery::parse("aa\th").unwrap();
        let report = index.refine(["丙"], 0, &query);
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].evidence.len(), 2);
        assert!(matches!(
            &report.matches[0].evidence[0],
            ShapeMatchEvidence::StrokePrefix { code } if code == "hsp"
        ));
        assert!(matches!(
            &report.matches[0].evidence[1],
            ShapeMatchEvidence::ComponentPrefix { code } if code == "hp"
        ));
    }

    #[test]
    fn empty_prefix_retains_even_unannotated_candidates_without_reranking() {
        let index = synthetic_index();
        let query = TabShapeQuery::parse("aa\t").unwrap();
        let report = index.refine(["未知", "", "甲"], 0, &query);
        assert_eq!(
            report
                .matches
                .iter()
                .map(|candidate| candidate.text)
                .collect::<Vec<_>>(),
            vec!["未知", "", "甲"]
        );
        assert!(
            report
                .matches
                .iter()
                .all(|candidate| candidate.evidence.is_empty())
        );
        assert_eq!(report.candidates_with_target, 2);
        assert_eq!(report.candidates_with_shape_data, 1);
    }

    #[test]
    fn target_position_can_refine_one_character_inside_a_longer_candidate() {
        let index = synthetic_index();
        let query = TabShapeQuery::parse("aaaa\tns").unwrap();
        let report = index.refine(["甲乙", "乙甲", "丙乙"], 1, &query);
        assert_eq!(
            report
                .matches
                .iter()
                .map(|candidate| candidate.text)
                .collect::<Vec<_>>(),
            vec!["甲乙", "丙乙"]
        );
    }

    #[test]
    fn malformed_or_duplicate_shape_metadata_is_rejected() {
        assert_eq!(
            CharacterShape::new('甲', vec!["hx".to_owned()], Vec::new()).unwrap_err(),
            ShapeRefinementError::InvalidStrokeCode { character: '甲' }
        );
        assert_eq!(
            CharacterShape::new('甲', Vec::new(), Vec::new()).unwrap_err(),
            ShapeRefinementError::MissingShapeCode { character: '甲' }
        );
        assert_eq!(
            CharacterShape::new('甲', vec!["h".to_owned(), "h".to_owned()], Vec::new())
                .unwrap_err(),
            ShapeRefinementError::DuplicateStrokeCode { character: '甲' }
        );
        let shape = CharacterShape::new('甲', vec!["h".to_owned()], vec!["j".to_owned()]).unwrap();
        assert_eq!(
            CharacterShapeIndex::new([shape.clone(), shape]).unwrap_err(),
            ShapeRefinementError::DuplicateCharacter { character: '甲' }
        );
    }
}
