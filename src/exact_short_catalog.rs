//! Lightweight exact-code catalog for independently authenticated short words.
//!
//! The catalog accepts the existing immutable candidate-package manifest and
//! lexicon payload, but applies a stricter canonical two-character profile.
//! It owns the original TSV bytes and indexes only contiguous byte ranges per
//! four-key code. No decoder, trie, correction graph, or sentence lattice is
//! constructed during loading or querying.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::mem::size_of;

use crate::public_lexicon_slice::is_han_character;
use crate::{
    CandidatePackageError, CandidatePackageManifest, MAX_CANDIDATE_SNAPSHOT_RANK,
    encode_pinyin_phrase, normalize_pinyin_tone_marks,
};

const LEXICON_HEADER: &str = "text\tpinyin\tfrequency\n";

/// Largest number of independently confirmed words retained for one complete
/// two-syllable code.
pub const MAX_EXACT_SHORT_WORDS_PER_CODE: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CodeRange {
    code: u32,
    start: u32,
    end: u32,
    entry_count: u8,
}

/// Manifest-bound, immutable exact short-word data with a compact code index.
#[derive(Clone, Debug)]
pub struct ExactShortWordCatalog {
    revision: String,
    payload: Box<str>,
    ranges: Vec<CodeRange>,
    entry_count: usize,
    maximum_code_depth: usize,
}

impl ExactShortWordCatalog {
    /// Validates and loads one canonical two-character lexicon payload.
    ///
    /// Rows must be sorted by canonical four-key code. Within one code they
    /// must be sorted by non-increasing source weight and contain no duplicate
    /// text identity. These invariants let a query binary-search a small range
    /// without building the general decoder index.
    pub fn load(
        manifest: &CandidatePackageManifest,
        lexicon_tsv: &str,
    ) -> Result<Self, ExactShortWordCatalogError> {
        Self::load_boxed(manifest, lexicon_tsv.into())
    }

    /// Moves an owned payload into the catalog after the same strict checks.
    ///
    /// File-backed loaders can avoid retaining or copying a second payload
    /// allocation by using this entry point after provenance validation.
    pub fn load_owned(
        manifest: &CandidatePackageManifest,
        lexicon_tsv: String,
    ) -> Result<Self, ExactShortWordCatalogError> {
        Self::load_boxed(manifest, lexicon_tsv.into_boxed_str())
    }

    fn load_boxed(
        manifest: &CandidatePackageManifest,
        payload: Box<str>,
    ) -> Result<Self, ExactShortWordCatalogError> {
        let lexicon_tsv = payload.as_ref();
        if manifest.contains_private_text() {
            return Err(ExactShortWordCatalogError::PrivatePayload);
        }
        if lexicon_tsv.len() != manifest.payload_bytes() {
            return Err(manifest
                .validate_payload_metadata(lexicon_tsv, 0)
                .expect_err("a payload-length mismatch must be rejected")
                .into());
        }
        if lexicon_tsv.contains('\r') || !lexicon_tsv.ends_with('\n') {
            return Err(ExactShortWordCatalogError::NonCanonicalPayload);
        }
        let Some(data) = lexicon_tsv.strip_prefix(LEXICON_HEADER) else {
            return Err(ExactShortWordCatalogError::InvalidHeader);
        };
        if data.is_empty() {
            return Err(ExactShortWordCatalogError::Empty);
        }

        let mut ranges = Vec::<CodeRange>::new();
        let mut entry_count = 0_usize;
        let mut offset = LEXICON_HEADER.len();
        let mut previous_code = None;
        let mut previous_frequency = 0_u64;
        let mut texts_for_code = HashSet::<&str>::new();
        let mut maximum_code_depth = 0_usize;

        for (row_index, line_with_newline) in data.split_inclusive('\n').enumerate() {
            let line_number = row_index + 2;
            let line = line_with_newline
                .strip_suffix('\n')
                .expect("a canonical LF-terminated payload splits into complete lines");
            if line.is_empty() {
                return Err(ExactShortWordCatalogError::InvalidRow { line_number });
            }
            let mut fields = line.split('\t');
            let (Some(text), Some(pinyin), Some(frequency), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(ExactShortWordCatalogError::InvalidRow { line_number });
            };
            if text.is_empty() || pinyin.is_empty() || frequency.is_empty() {
                return Err(ExactShortWordCatalogError::InvalidRow { line_number });
            }
            if text.chars().count() != 2 || !text.chars().all(is_han_character) {
                return Err(ExactShortWordCatalogError::InvalidShortWord { line_number });
            }
            if normalize_pinyin_tone_marks(pinyin).ok().as_deref() != Some(pinyin) {
                return Err(ExactShortWordCatalogError::InvalidPinyin { line_number });
            }
            let encoded = encode_pinyin_phrase(pinyin)
                .map_err(|_| ExactShortWordCatalogError::InvalidPinyin { line_number })?;
            if encoded.syllable_codes.len() != 2 || encoded.full_code.as_str().len() != 4 {
                return Err(ExactShortWordCatalogError::InvalidPinyin { line_number });
            }
            let frequency = parse_canonical_frequency(frequency)
                .ok_or(ExactShortWordCatalogError::InvalidFrequency { line_number })?;
            let code = pack_code(encoded.full_code.as_str())
                .expect("the central two-syllable codec emits four lowercase ASCII bytes");
            let row_end = offset
                .checked_add(line_with_newline.len())
                .and_then(|end| u32::try_from(end).ok())
                .ok_or(ExactShortWordCatalogError::NonCanonicalPayload)?;

            match previous_code {
                Some(previous) if code < previous => {
                    return Err(ExactShortWordCatalogError::NonCanonicalOrder { line_number });
                }
                Some(previous) if code == previous => {
                    if frequency > previous_frequency {
                        return Err(ExactShortWordCatalogError::NonCanonicalOrder { line_number });
                    }
                    if !texts_for_code.insert(text) {
                        return Err(ExactShortWordCatalogError::DuplicateIdentity { line_number });
                    }
                    let range = ranges
                        .last_mut()
                        .expect("a repeated code always has a preceding range");
                    if usize::from(range.entry_count) == MAX_EXACT_SHORT_WORDS_PER_CODE {
                        return Err(ExactShortWordCatalogError::CodeDepthExceeded { line_number });
                    }
                    range.end = row_end;
                    range.entry_count += 1;
                }
                _ => {
                    texts_for_code.clear();
                    texts_for_code.insert(text);
                    ranges.push(CodeRange {
                        code,
                        start: u32::try_from(offset)
                            .map_err(|_| ExactShortWordCatalogError::NonCanonicalPayload)?,
                        end: row_end,
                        entry_count: 1,
                    });
                }
            }
            previous_code = Some(code);
            previous_frequency = frequency;
            entry_count += 1;
            maximum_code_depth = maximum_code_depth.max(usize::from(
                ranges
                    .last()
                    .expect("every accepted row belongs to one range")
                    .entry_count,
            ));
            offset = usize::try_from(row_end)
                .expect("a validated payload byte offset fits the current process");
        }

        manifest.validate_payload_metadata(lexicon_tsv, entry_count)?;
        drop(texts_for_code);
        Ok(Self {
            revision: manifest.revision().to_owned(),
            payload,
            ranges,
            entry_count,
            maximum_code_depth,
        })
    }

    /// Returns exact words for one complete two-syllable code in source order.
    pub fn candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<&str>, ExactShortWordCatalogError> {
        let code = pack_code(code).ok_or(ExactShortWordCatalogError::InvalidQueryCode)?;
        if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&limit) {
            return Err(ExactShortWordCatalogError::InvalidQueryLimit);
        }
        let Ok(index) = self.ranges.binary_search_by_key(&code, |range| range.code) else {
            return Ok(Vec::new());
        };
        let range = self.ranges[index];
        let bytes = &self.payload[range.start as usize..range.end as usize];
        Ok(bytes
            .lines()
            .take(limit)
            .map(|line| {
                line.split_once('\t')
                    .expect("validated catalog rows always contain a text field")
                    .0
            })
            .collect())
    }

    /// Previews a bounded exact-layer insertion without changing any runtime.
    ///
    /// An existing primary Top-1 is immutable. New exact identities may be
    /// inserted immediately after it up to `exact_promotions`; the remaining
    /// primary order is preserved. If the primary list is empty, exact words
    /// may start the result. This pure function is the candidate-displacement
    /// safety boundary for later offline audits; TSF does not call it yet.
    pub fn preview_candidate_texts(
        &self,
        primary: &[String],
        code: &str,
        visible_limit: usize,
        exact_promotions: usize,
    ) -> Result<Vec<String>, ExactShortWordCatalogError> {
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&visible_limit) {
            return Err(ExactShortWordCatalogError::InvalidVisibleLimit);
        }
        if exact_promotions > MAX_EXACT_SHORT_WORDS_PER_CODE {
            return Err(ExactShortWordCatalogError::InvalidPromotionLimit);
        }
        if exact_promotions == 0 {
            return Ok(primary.iter().take(visible_limit).cloned().collect());
        }
        let exact = self.candidate_texts(code, MAX_EXACT_SHORT_WORDS_PER_CODE)?;
        let mut merged = Vec::with_capacity(visible_limit);
        let primary_start = if let Some(first) = primary.first() {
            merged.push(first.clone());
            1
        } else {
            0
        };
        for candidate in exact {
            if merged.len() == visible_limit
                || merged.len().saturating_sub(primary_start) == exact_promotions
            {
                break;
            }
            if primary.iter().any(|existing| existing == candidate) {
                continue;
            }
            merged.push(candidate.to_owned());
        }
        for candidate in primary.iter().skip(primary_start) {
            if merged.len() == visible_limit {
                break;
            }
            merged.push(candidate.clone());
        }
        Ok(merged)
    }

    /// Returns the validated data revision.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Returns the number of indexed exact-word rows.
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// Returns the number of distinct complete four-key codes.
    pub fn code_count(&self) -> usize {
        self.ranges.len()
    }

    /// Returns the deepest retained identity count for any one code.
    pub fn maximum_code_depth(&self) -> usize {
        self.maximum_code_depth
    }

    /// Returns the payload bytes retained by this catalog.
    pub fn payload_bytes(&self) -> usize {
        self.payload.len()
    }

    /// Returns the exact heap bytes occupied by the compact range array.
    pub fn index_bytes(&self) -> usize {
        self.ranges.len() * size_of::<CodeRange>()
    }
}

fn pack_code(code: &str) -> Option<u32> {
    let bytes: [u8; 4] = code.as_bytes().try_into().ok()?;
    if !bytes.iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    Some(u32::from_be_bytes(bytes))
}

fn parse_canonical_frequency(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok().filter(|frequency| *frequency != 0)
}

/// Errors returned by the strict exact-short-word payload profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExactShortWordCatalogError {
    /// The underlying immutable package metadata did not match the payload.
    Package(CandidatePackageError),
    /// This public catalog does not accept a private-text payload.
    PrivatePayload,
    /// The payload did not use canonical LF-terminated rows.
    NonCanonicalPayload,
    /// The payload header was not the exact lexicon-v1 header.
    InvalidHeader,
    /// No short-word row followed the header.
    Empty,
    /// A row did not contain exactly three non-empty fields.
    InvalidRow { line_number: usize },
    /// A surface was not exactly two Han characters.
    InvalidShortWord { line_number: usize },
    /// Pinyin did not encode to exactly two canonical syllables.
    InvalidPinyin { line_number: usize },
    /// Frequency was not a canonical positive decimal integer.
    InvalidFrequency { line_number: usize },
    /// Codes or source-internal weights were not in canonical order.
    NonCanonicalOrder { line_number: usize },
    /// One code repeated the same surface identity.
    DuplicateIdentity { line_number: usize },
    /// One code exceeded the bounded exact-word depth.
    CodeDepthExceeded { line_number: usize },
    /// A query was not exactly four lowercase ASCII letters.
    InvalidQueryCode,
    /// A query limit was zero or exceeded the catalog depth boundary.
    InvalidQueryLimit,
    /// A visible preview limit was outside the interactive snapshot boundary.
    InvalidVisibleLimit,
    /// A preview requested more exact insertions than one code can contain.
    InvalidPromotionLimit,
}

impl From<CandidatePackageError> for ExactShortWordCatalogError {
    fn from(error: CandidatePackageError) -> Self {
        Self::Package(error)
    }
}

impl fmt::Display for ExactShortWordCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(error) => write!(formatter, "精确短词包校验失败：{error}"),
            Self::PrivatePayload => write!(formatter, "精确短词层不接受私有明文"),
            Self::NonCanonicalPayload => write!(formatter, "精确短词载荷不是规范 LF 格式"),
            Self::InvalidHeader => write!(formatter, "精确短词载荷表头无效"),
            Self::Empty => write!(formatter, "精确短词载荷为空"),
            Self::InvalidRow { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行结构无效")
            }
            Self::InvalidShortWord { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行不是双汉字词面")
            }
            Self::InvalidPinyin { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行拼音无效")
            }
            Self::InvalidFrequency { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行权重无效")
            }
            Self::NonCanonicalOrder { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行顺序无效")
            }
            Self::DuplicateIdentity { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行身份重复")
            }
            Self::CodeDepthExceeded { line_number } => {
                write!(formatter, "精确短词载荷第 {line_number} 行超出同码深度")
            }
            Self::InvalidQueryCode => write!(formatter, "精确短词查询码必须是四个小写字母"),
            Self::InvalidQueryLimit => write!(formatter, "精确短词查询上限无效"),
            Self::InvalidVisibleLimit => write!(formatter, "精确短词预览候选上限无效"),
            Self::InvalidPromotionLimit => write!(formatter, "精确短词预览提升上限无效"),
        }
    }
}

impl Error for ExactShortWordCatalogError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Package(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "text\tpinyin\tfrequency\n\
收拾\tshou shi\t90\n\
收束\tshou shu\t80\n\
手术\tshou shu\t70\n\
首项\tshou xiang\t60\n";

    fn manifest(payload: &str) -> CandidatePackageManifest {
        CandidatePackageManifest::from_payload("exact-short-test-v1", false, payload).unwrap()
    }

    #[test]
    fn loads_compact_ranges_and_queries_only_one_complete_code() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        assert_eq!(catalog.revision(), "exact-short-test-v1");
        assert_eq!(catalog.entry_count(), 4);
        assert_eq!(catalog.code_count(), 3);
        assert_eq!(catalog.maximum_code_depth(), 2);
        assert_eq!(catalog.payload_bytes(), PAYLOAD.len());
        assert_eq!(catalog.index_bytes(), 3 * size_of::<CodeRange>());
        assert_eq!(
            catalog.candidate_texts("ubuu", 8).unwrap(),
            ["收束", "手术"]
        );
        assert_eq!(catalog.candidate_texts("ubxd", 1).unwrap(), ["首项"]);
        assert!(catalog.candidate_texts("nihk", 8).unwrap().is_empty());
    }

    #[test]
    fn metadata_drift_is_rejected_before_catalog_publication() {
        let manifest = manifest(PAYLOAD);
        let changed = PAYLOAD.replace("收束\tshou shu\t80", "收束\tshou shu\t81");
        assert!(matches!(
            ExactShortWordCatalog::load(&manifest, &changed),
            Err(ExactShortWordCatalogError::Package(
                CandidatePackageError::Snapshot(
                    crate::CandidateSnapshotError::PayloadFingerprintMismatch { .. }
                )
            ))
        ));
    }

    #[test]
    fn rejects_non_short_unsorted_duplicate_and_overdeep_payloads() {
        let non_short = PAYLOAD.replace("首项\tshou xiang\t60", "第一个\tdi yi ge\t60");
        assert!(matches!(
            ExactShortWordCatalog::load(&manifest(&non_short), &non_short),
            Err(ExactShortWordCatalogError::InvalidShortWord { .. })
        ));

        let unsorted = "text\tpinyin\tfrequency\n首项\tshou xiang\t60\n收束\tshou shu\t80\n";
        assert!(matches!(
            ExactShortWordCatalog::load(&manifest(unsorted), unsorted),
            Err(ExactShortWordCatalogError::NonCanonicalOrder { .. })
        ));

        let noncanonical_pinyin = PAYLOAD.replace("shou shi", "Shou shi");
        assert!(matches!(
            ExactShortWordCatalog::load(&manifest(&noncanonical_pinyin), &noncanonical_pinyin),
            Err(ExactShortWordCatalogError::InvalidPinyin { .. })
        ));

        let duplicate = "text\tpinyin\tfrequency\n收束\tshou shu\t80\n收束\tshou shu\t70\n";
        assert!(CandidatePackageManifest::from_payload("duplicate", false, duplicate).is_err());

        let mut overdeep = String::from(LEXICON_HEADER);
        for (index, text) in [
            "收束", "手术", "手数", "收数", "授书", "受书", "售书", "兽术", "绶署",
        ]
        .into_iter()
        .enumerate()
        {
            use std::fmt::Write as _;
            writeln!(overdeep, "{text}\tshou shu\t{}", 100 - index).unwrap();
        }
        assert!(matches!(
            ExactShortWordCatalog::load(&manifest(&overdeep), &overdeep),
            Err(ExactShortWordCatalogError::CodeDepthExceeded { .. })
        ));
    }

    #[test]
    fn query_shape_and_limit_are_bounded() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        assert_eq!(
            catalog.candidate_texts("ubu", 1),
            Err(ExactShortWordCatalogError::InvalidQueryCode)
        );
        assert_eq!(
            catalog.candidate_texts("ubuu", 0),
            Err(ExactShortWordCatalogError::InvalidQueryLimit)
        );
    }

    #[test]
    fn preview_preserves_primary_top_one_and_bounds_displacement() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = ["叔叔".to_owned(), "输出".to_owned(), "手术".to_owned()];
        assert_eq!(
            catalog
                .preview_candidate_texts(&primary, "ubuu", 4, 1)
                .unwrap(),
            ["叔叔", "收束", "输出", "手术"]
        );
        assert_eq!(
            catalog
                .preview_candidate_texts(&primary, "ubuu", 2, 0)
                .unwrap(),
            ["叔叔", "输出"]
        );
        assert_eq!(
            catalog.preview_candidate_texts(&[], "ubxd", 2, 1).unwrap(),
            ["首项"]
        );
    }

    #[test]
    fn preview_never_reinserts_an_identity_already_in_the_primary_frontier() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = ["叔叔".to_owned(), "收束".to_owned(), "输出".to_owned()];
        assert_eq!(
            catalog
                .preview_candidate_texts(&primary, "ubuu", 4, 2)
                .unwrap(),
            ["叔叔", "手术", "收束", "输出"]
        );
    }

    #[test]
    fn every_preview_keeps_bounds_top_one_and_primary_relative_order() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = [
            "叔叔".to_owned(),
            "输出".to_owned(),
            "手术".to_owned(),
            "数数".to_owned(),
        ];
        for visible_limit in 1..=MAX_CANDIDATE_SNAPSHOT_RANK {
            for exact_promotions in 0..=MAX_EXACT_SHORT_WORDS_PER_CODE {
                let merged = catalog
                    .preview_candidate_texts(&primary, "ubuu", visible_limit, exact_promotions)
                    .unwrap();
                assert!(merged.len() <= visible_limit);
                assert_eq!(merged.first(), primary.first());
                let retained_primary = merged
                    .iter()
                    .filter(|candidate| primary.contains(candidate))
                    .collect::<Vec<_>>();
                let expected_primary = primary
                    .iter()
                    .filter(|candidate| merged.contains(candidate))
                    .collect::<Vec<_>>();
                assert_eq!(retained_primary, expected_primary);
                let inserted = merged
                    .iter()
                    .filter(|candidate| !primary.contains(candidate))
                    .count();
                assert!(inserted <= exact_promotions);
            }
        }
    }
}
