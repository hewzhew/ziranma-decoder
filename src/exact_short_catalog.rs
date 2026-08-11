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
    candidate_payload_fingerprint, encode_pinyin_phrase, normalize_pinyin_tone_marks,
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
    payload_fingerprint: u64,
    payload: Box<str>,
    ranges: Vec<CodeRange>,
    entry_count: usize,
    maximum_code_depth: usize,
}

/// Memory-only candidate-page state for one exact-short query.
///
/// The first page bypasses the exact catalog completely. Crossing into the
/// second page makes one guarded insertion decision; later depth increases
/// freeze every candidate already returned and append only unseen primary
/// identities. The state has no persistence or debug representation because
/// callers may supply personalized candidate text in `primary`.
#[derive(Default)]
pub struct ExactShortPageSession {
    code: String,
    catalog_revision: String,
    catalog_payload_fingerprint: u64,
    page_size: usize,
    exact_promotions: usize,
    requested_limit: usize,
    primary: Vec<String>,
    candidates: Vec<String>,
    primary_indices: Vec<Option<usize>>,
    primary_exhausted: bool,
    second_page_decided: bool,
}

impl ExactShortPageSession {
    /// Extends one candidate query without rewriting an already returned
    /// prefix.
    ///
    /// `primary` must be the deterministic primary-layer prefix for
    /// `total_limit`. A changed code starts a fresh session. For the same code,
    /// the catalog revision, page size, promotion bound, and previously seen
    /// primary prefix are immutable. Requests at or below the high-water mark
    /// reuse the deepest cached result.
    pub fn extend<'a>(
        &'a mut self,
        catalog: &ExactShortWordCatalog,
        primary: &[String],
        code: &str,
        total_limit: usize,
        exact_promotions: usize,
        page_size: usize,
    ) -> Result<&'a [String], ExactShortWordCatalogError> {
        if pack_code(code).is_none() {
            return Err(ExactShortWordCatalogError::InvalidQueryCode);
        }
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&total_limit) {
            return Err(ExactShortWordCatalogError::InvalidVisibleLimit);
        }
        if page_size == 0 || page_size > total_limit {
            return Err(ExactShortWordCatalogError::InvalidStablePrefix);
        }
        if exact_promotions > MAX_EXACT_SHORT_WORDS_PER_CODE {
            return Err(ExactShortWordCatalogError::InvalidPromotionLimit);
        }

        if self.catalog_revision.is_empty() || self.code != code {
            self.clear();
            self.code.push_str(code);
            self.catalog_revision.push_str(catalog.revision());
            self.catalog_payload_fingerprint = catalog.payload_fingerprint;
            self.page_size = page_size;
            self.exact_promotions = exact_promotions;
        } else if self.catalog_revision != catalog.revision()
            || self.catalog_payload_fingerprint != catalog.payload_fingerprint
            || self.page_size != page_size
            || self.exact_promotions != exact_promotions
        {
            return Err(ExactShortWordCatalogError::ChangedPageSession);
        }

        if self.requested_limit >= total_limit {
            return Ok(&self.candidates);
        }

        let primary = &primary[..primary.len().min(total_limit)];
        if self.primary.len() > primary.len()
            || self
                .primary
                .iter()
                .ne(primary.iter().take(self.primary.len()))
        {
            return Err(ExactShortWordCatalogError::UnstablePrimaryPrefix);
        }

        let (candidates, primary_indices) = if self.second_page_decided {
            let mut extended = self.candidates.clone();
            let mut primary_indices = self.primary_indices.clone();
            for (primary_index, candidate) in primary.iter().enumerate() {
                if extended.len() == total_limit {
                    break;
                }
                if !extended.contains(candidate) {
                    extended.push(candidate.clone());
                    primary_indices.push(Some(primary_index));
                }
            }
            (extended, primary_indices)
        } else if total_limit > page_size && primary.len() >= page_size {
            let merged = catalog.preview_candidate_texts_after_page_guarded(
                primary,
                code,
                total_limit,
                exact_promotions,
                page_size,
            )?;
            if !self.candidates.is_empty()
                && self
                    .candidates
                    .iter()
                    .ne(merged.iter().take(self.candidates.len()))
            {
                return Err(ExactShortWordCatalogError::UnstablePrimaryPrefix);
            }
            self.second_page_decided = true;
            let primary_indices = candidate_primary_indices(&merged, primary);
            (merged, primary_indices)
        } else {
            (primary.to_vec(), (0..primary.len()).map(Some).collect())
        };

        self.primary = primary.to_vec();
        self.candidates = candidates;
        self.primary_indices = primary_indices;
        self.primary_exhausted = primary.len() < total_limit;
        self.requested_limit = total_limit;
        Ok(&self.candidates)
    }

    /// Discards the in-memory high-water mark and all candidate text.
    pub fn clear(&mut self) {
        self.code.clear();
        self.catalog_revision.clear();
        self.catalog_payload_fingerprint = 0;
        self.page_size = 0;
        self.exact_promotions = 0;
        self.requested_limit = 0;
        self.primary.clear();
        self.candidates.clear();
        self.primary_indices.clear();
        self.primary_exhausted = false;
        self.second_page_decided = false;
    }

    /// Returns the deepest logical request already represented by this state.
    pub fn requested_limit(&self) -> usize {
        self.requested_limit
    }

    /// Maps every returned candidate to its primary-layer index.
    ///
    /// `None` marks an identity inserted only by the exact-short catalog.
    /// Indices remain valid for the deepest primary prefix accepted by this
    /// session because any primary-prefix rewrite is rejected.
    pub fn primary_indices(&self) -> &[Option<usize>] {
        &self.primary_indices
    }

    /// Reports whether a deeper page may still exist after exact insertion.
    ///
    /// Exact candidates never turn an exhausted primary result into an
    /// unknown-depth result, even when they fill the requested display bound.
    pub fn may_have_more(&self) -> bool {
        !self.primary_exhausted && self.requested_limit < MAX_CANDIDATE_SNAPSHOT_RANK
    }
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
            payload_fingerprint: candidate_payload_fingerprint(payload.as_bytes()),
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
        self.preview_candidate_texts_after_prefix(primary, code, visible_limit, exact_promotions, 1)
    }

    /// Previews insertion after an immutable leading candidate prefix.
    ///
    /// Setting `stable_prefix` to the UI page size preserves the complete
    /// first page while making exact words available at the start of the next
    /// page. The total result remains bounded independently.
    pub fn preview_candidate_texts_after_prefix(
        &self,
        primary: &[String],
        code: &str,
        total_limit: usize,
        exact_promotions: usize,
        stable_prefix: usize,
    ) -> Result<Vec<String>, ExactShortWordCatalogError> {
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&total_limit) {
            return Err(ExactShortWordCatalogError::InvalidVisibleLimit);
        }
        if stable_prefix == 0 || stable_prefix > total_limit {
            return Err(ExactShortWordCatalogError::InvalidStablePrefix);
        }
        if exact_promotions > MAX_EXACT_SHORT_WORDS_PER_CODE {
            return Err(ExactShortWordCatalogError::InvalidPromotionLimit);
        }
        if exact_promotions == 0 {
            return Ok(primary.iter().take(total_limit).cloned().collect());
        }
        let exact = self.candidate_texts(code, MAX_EXACT_SHORT_WORDS_PER_CODE)?;
        Ok(Self::merge_candidate_texts_after_prefix(
            primary,
            &exact,
            total_limit,
            exact_promotions,
            stable_prefix,
        ))
    }

    fn merge_candidate_texts_after_prefix(
        primary: &[String],
        exact: &[&str],
        total_limit: usize,
        exact_promotions: usize,
        stable_prefix: usize,
    ) -> Vec<String> {
        let mut merged = Vec::with_capacity(total_limit);
        let primary_start = stable_prefix.min(primary.len());
        merged.extend(primary.iter().take(primary_start).cloned());
        for candidate in exact {
            if merged.len() == total_limit
                || merged.len().saturating_sub(primary_start) == exact_promotions
            {
                break;
            }
            if primary.iter().any(|existing| existing == candidate) {
                continue;
            }
            merged.push((*candidate).to_owned());
        }
        for candidate in primary.iter().skip(primary_start) {
            if merged.len() == total_limit {
                break;
            }
            merged.push(candidate.clone());
        }
        merged
    }

    /// Previews exact insertion at the second-page boundary while protecting
    /// every exact identity that is already present from crossing a page.
    ///
    /// A shallow primary list is returned unchanged because it has no second
    /// page yet. For a full first page, the largest safe insertion count up to
    /// `exact_promotions` is selected. This deliberately prefers omission to
    /// moving an existing exact short word into a later page or beyond the
    /// bounded result.
    pub fn preview_candidate_texts_after_page_guarded(
        &self,
        primary: &[String],
        code: &str,
        total_limit: usize,
        exact_promotions: usize,
        page_size: usize,
    ) -> Result<Vec<String>, ExactShortWordCatalogError> {
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&total_limit) {
            return Err(ExactShortWordCatalogError::InvalidVisibleLimit);
        }
        if page_size == 0 || page_size > total_limit {
            return Err(ExactShortWordCatalogError::InvalidStablePrefix);
        }
        if exact_promotions > MAX_EXACT_SHORT_WORDS_PER_CODE {
            return Err(ExactShortWordCatalogError::InvalidPromotionLimit);
        }
        if exact_promotions == 0 || primary.len() < page_size {
            return Ok(primary.iter().take(total_limit).cloned().collect());
        }

        let exact = self.candidate_texts(code, MAX_EXACT_SHORT_WORDS_PER_CODE)?;
        let desired_insertions = exact
            .iter()
            .filter(|candidate| !primary.iter().any(|existing| existing == *candidate))
            .take(exact_promotions)
            .count();
        let safe_insertions = (0..=desired_insertions)
            .rev()
            .find(|insertions| {
                exact.iter().all(|candidate| {
                    let Some(index) = primary
                        .iter()
                        .position(|existing| existing.as_str() == *candidate)
                    else {
                        return true;
                    };
                    let before_rank = index + 1;
                    if before_rank <= page_size || before_rank > total_limit {
                        return true;
                    }
                    let after_rank = before_rank + insertions;
                    after_rank <= total_limit
                        && (before_rank - 1) / page_size == (after_rank - 1) / page_size
                })
            })
            .unwrap_or(0);
        Ok(Self::merge_candidate_texts_after_prefix(
            primary,
            &exact,
            total_limit,
            safe_insertions,
            page_size,
        ))
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

fn candidate_primary_indices(candidates: &[String], primary: &[String]) -> Vec<Option<usize>> {
    let mut used = vec![false; primary.len()];
    candidates
        .iter()
        .map(|candidate| {
            let index = primary
                .iter()
                .enumerate()
                .position(|(index, primary)| !used[index] && primary == candidate);
            if let Some(index) = index {
                used[index] = true;
            }
            index
        })
        .collect()
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
    /// A preview prefix was zero or exceeded the total result boundary.
    InvalidStablePrefix,
    /// One active page session changed its catalog or insertion configuration.
    ChangedPageSession,
    /// A deeper primary request rewrote a prefix that had already been seen.
    UnstablePrimaryPrefix,
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
            Self::InvalidStablePrefix => write!(formatter, "精确短词预览固定前缀无效"),
            Self::ChangedPageSession => write!(formatter, "精确短词分页会话配置发生变化"),
            Self::UnstablePrimaryPrefix => write!(formatter, "精确短词分页基础候选前缀不稳定"),
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
    fn preview_after_page_preserves_the_page_and_does_not_charge_duplicates() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = [
            "候选一".to_owned(),
            "候选二".to_owned(),
            "候选三".to_owned(),
            "候选四".to_owned(),
            "候选五".to_owned(),
            "候选六".to_owned(),
            "候选七".to_owned(),
            "收束".to_owned(),
            "末尾".to_owned(),
        ];
        let merged = catalog
            .preview_candidate_texts_after_prefix(&primary, "ubuu", 9, 1, 7)
            .unwrap();
        assert_eq!(&merged[..7], &primary[..7]);
        assert_eq!(
            merged,
            [
                "候选一",
                "候选二",
                "候选三",
                "候选四",
                "候选五",
                "候选六",
                "候选七",
                "手术",
                "收束",
            ]
        );
        assert_eq!(merged.len(), 9);
        let retained_primary = merged
            .iter()
            .filter(|candidate| primary.contains(candidate))
            .collect::<Vec<_>>();
        let expected_primary = primary
            .iter()
            .filter(|candidate| merged.contains(candidate))
            .collect::<Vec<_>>();
        assert_eq!(retained_primary, expected_primary);
    }

    #[test]
    fn preview_after_page_rejects_an_invalid_stable_prefix() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = ["候选".to_owned()];
        assert_eq!(
            catalog.preview_candidate_texts_after_prefix(&primary, "ubuu", 7, 1, 0),
            Err(ExactShortWordCatalogError::InvalidStablePrefix)
        );
        assert_eq!(
            catalog.preview_candidate_texts_after_prefix(&primary, "ubuu", 7, 1, 8),
            Err(ExactShortWordCatalogError::InvalidStablePrefix)
        );
    }

    #[test]
    fn page_guard_refuses_to_push_an_existing_exact_identity_across_a_boundary() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = (1..=14)
            .map(|rank| {
                if rank == 14 {
                    "收束".to_owned()
                } else {
                    format!("候选{rank}")
                }
            })
            .collect::<Vec<_>>();
        let raw = catalog
            .preview_candidate_texts_after_prefix(&primary, "ubuu", 16, 1, 7)
            .unwrap();
        assert_eq!(candidate_position(&raw, "收束"), Some(15));
        let guarded = catalog
            .preview_candidate_texts_after_page_guarded(&primary, "ubuu", 16, 1, 7)
            .unwrap();
        assert_eq!(guarded, primary);
    }

    #[test]
    fn page_guard_uses_the_largest_insertion_count_that_keeps_exact_pages_stable() {
        const GUARDED_PAYLOAD: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t90\n\
手术\tshou shu\t80\n\
首数\tshou shu\t70\n";
        let catalog =
            ExactShortWordCatalog::load(&manifest(GUARDED_PAYLOAD), GUARDED_PAYLOAD).unwrap();
        let primary = (1..=13)
            .map(|rank| {
                if rank == 13 {
                    "收束".to_owned()
                } else {
                    format!("候选{rank}")
                }
            })
            .collect::<Vec<_>>();
        let guarded = catalog
            .preview_candidate_texts_after_page_guarded(&primary, "ubuu", 16, 2, 7)
            .unwrap();
        assert_eq!(&guarded[..7], &primary[..7]);
        assert_eq!(guarded[7], "手术");
        assert_eq!(candidate_position(&guarded, "首数"), None);
        assert_eq!(candidate_position(&guarded, "收束"), Some(14));
    }

    #[test]
    fn page_guard_does_not_create_a_first_page_for_a_shallow_primary() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = ["已有一".to_owned(), "已有二".to_owned()];
        assert_eq!(
            catalog
                .preview_candidate_texts_after_page_guarded(&primary, "ubuu", 14, 2, 7)
                .unwrap(),
            primary
        );
    }

    #[test]
    fn page_session_keeps_the_first_page_unmodified_until_a_second_page_exists() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = (1..=12)
            .map(|rank| format!("基础{rank}"))
            .collect::<Vec<_>>();
        let mut session = ExactShortPageSession::default();

        assert_eq!(
            session
                .extend(&catalog, &primary[..6], "ubuu", 6, 2, 6)
                .unwrap(),
            &primary[..6]
        );
        assert_eq!(session.requested_limit(), 6);
        assert!(!session.second_page_decided);
        assert_eq!(session.candidates, primary[..6]);
        assert_eq!(
            session.primary_indices(),
            &[Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        assert!(session.may_have_more());
    }

    #[test]
    fn page_session_freezes_every_presented_prefix_across_lazy_depths() {
        const GUARDED_PAYLOAD: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t90\n\
手术\tshou shu\t80\n\
首数\tshou shu\t70\n";
        let catalog =
            ExactShortWordCatalog::load(&manifest(GUARDED_PAYLOAD), GUARDED_PAYLOAD).unwrap();
        let mut primary = (1..=50)
            .map(|rank| format!("基础{rank}"))
            .collect::<Vec<_>>();
        primary[16] = "收束".to_owned();
        let mut session = ExactShortPageSession::default();

        let first = session
            .extend(&catalog, &primary[..6], "ubuu", 6, 2, 6)
            .unwrap()
            .to_vec();
        let second = session
            .extend(&catalog, &primary[..12], "ubuu", 12, 2, 6)
            .unwrap()
            .to_vec();
        let third = session
            .extend(&catalog, &primary[..18], "ubuu", 18, 2, 6)
            .unwrap()
            .to_vec();
        let deepest = session
            .extend(&catalog, &primary, "ubuu", 50, 2, 6)
            .unwrap()
            .to_vec();

        assert_eq!(first, primary[..6]);
        assert_eq!(&second[..first.len()], first.as_slice());
        assert_eq!(&second[6..8], ["收束", "手术"]);
        assert_eq!(
            session.primary_indices(),
            &[
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                None,
                None,
                Some(6),
                Some(7),
                Some(8),
                Some(9),
                Some(10),
                Some(11),
                Some(12),
                Some(13),
                Some(14),
                Some(15),
                Some(17),
                Some(18),
                Some(19),
                Some(20),
                Some(21),
                Some(22),
                Some(23),
                Some(24),
                Some(25),
                Some(26),
                Some(27),
                Some(28),
                Some(29),
                Some(30),
                Some(31),
                Some(32),
                Some(33),
                Some(34),
                Some(35),
                Some(36),
                Some(37),
                Some(38),
                Some(39),
                Some(40),
                Some(41),
                Some(42),
                Some(43),
                Some(44),
                Some(45),
                Some(46),
                Some(47),
                Some(48),
            ]
        );
        assert_eq!(&third[..second.len()], second.as_slice());
        assert_eq!(&deepest[..third.len()], third.as_slice());
        assert_eq!(deepest.len(), 50);
        assert_eq!(deepest.iter().collect::<HashSet<_>>().len(), deepest.len());
        assert_eq!(session.requested_limit(), 50);
        assert!(!session.may_have_more());

        let independently_recomputed = catalog
            .preview_candidate_texts_after_page_guarded(&primary[..18], "ubuu", 18, 2, 6)
            .unwrap();
        assert_eq!(independently_recomputed[6], "手术");
        assert_ne!(
            &independently_recomputed[..second.len()],
            second.as_slice(),
            "a fresh deeper guard would retract the already presented second-page decision"
        );

        assert_eq!(
            session
                .extend(&catalog, &primary[..6], "ubuu", 6, 2, 6)
                .unwrap(),
            deepest,
            "shallower navigation reuses the deepest high-water result"
        );
    }

    #[test]
    fn page_session_keeps_primary_exhaustion_distinct_from_an_exactly_filled_result() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = (1..=6)
            .map(|rank| format!("基础{rank}"))
            .collect::<Vec<_>>();
        let mut session = ExactShortPageSession::default();

        let candidates = session.extend(&catalog, &primary, "ubuu", 8, 2, 6).unwrap();
        assert_eq!(candidates.len(), 8);
        assert_eq!(&candidates[6..], ["收束", "手术"]);
        assert_eq!(
            session.primary_indices(),
            &[
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                None,
                None
            ]
        );
        assert!(!session.may_have_more());
    }

    #[test]
    fn page_session_rejects_primary_reordering_and_midstream_configuration_drift() {
        let catalog = ExactShortWordCatalog::load(&manifest(PAYLOAD), PAYLOAD).unwrap();
        let primary = (1..=12)
            .map(|rank| format!("基础{rank}"))
            .collect::<Vec<_>>();
        let mut session = ExactShortPageSession::default();
        session
            .extend(&catalog, &primary[..6], "ubuu", 6, 2, 6)
            .unwrap();

        assert_eq!(
            session.extend(&catalog, &primary[..6], "ubuu", 6, 1, 6),
            Err(ExactShortWordCatalogError::ChangedPageSession)
        );
        let changed_payload = PAYLOAD.replace("手术", "首数");
        let same_revision_changed_catalog =
            ExactShortWordCatalog::load(&manifest(&changed_payload), &changed_payload).unwrap();
        assert_eq!(
            session.extend(&same_revision_changed_catalog, &primary, "ubuu", 12, 2, 6,),
            Err(ExactShortWordCatalogError::ChangedPageSession)
        );
        let replacement_catalog = ExactShortWordCatalog::load(
            &CandidatePackageManifest::from_payload("exact-short-test-v2", false, PAYLOAD).unwrap(),
            PAYLOAD,
        )
        .unwrap();
        assert_eq!(
            session.extend(&replacement_catalog, &primary, "ubuu", 12, 2, 6),
            Err(ExactShortWordCatalogError::ChangedPageSession)
        );

        let mut reordered = primary.clone();
        reordered.swap(0, 1);
        assert_eq!(
            session.extend(&catalog, &reordered, "ubuu", 12, 2, 6),
            Err(ExactShortWordCatalogError::UnstablePrimaryPrefix)
        );
        assert_eq!(session.requested_limit(), 6);

        assert_eq!(
            session
                .extend(&replacement_catalog, &primary[..6], "ubxd", 6, 2, 6)
                .unwrap(),
            &primary[..6],
            "a changed code starts a fresh bounded session"
        );
        assert_eq!(session.requested_limit(), 6);
    }

    fn candidate_position(candidates: &[String], expected: &str) -> Option<usize> {
        candidates
            .iter()
            .position(|candidate| candidate == expected)
            .map(|index| index + 1)
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
