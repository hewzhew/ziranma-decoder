//! Deterministic, bounded slices of large public Rime dictionaries.
//!
//! This module only transforms explicitly supplied public text. It performs
//! no file discovery, download, installation, slot mutation, or private-data
//! access.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    KeySequence, LexiconEntry, MAX_CANDIDATE_SNAPSHOT_RANK, MAX_LEXICON_SYLLABLES,
    SupplementalCandidateLayerConfig, SupplementalCandidateLayerError, encode_pinyin_phrase,
    merge_candidate_text_layers, normalize_pinyin_tone_marks,
};

/// Largest source file accepted by the explicit large-public-dictionary CLI.
pub const MAX_PUBLIC_RIME_SLICE_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// Conservative entry cap for one experimental public slice.
///
/// This remains below the validated snapshot ceiling while allowing a narrow
/// cutoff-sensitivity experiment beyond the original Top-100k frontier.
pub const MAX_PUBLIC_RIME_SLICE_ENTRIES: usize = 120_000;
/// Longest Han-only entry accepted by the first slice experiment.
pub const MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS: usize = 12;

/// Explicit deterministic limits for one public Rime slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicRimeSliceConfig {
    /// Maximum retained entries after ranking and deduplication.
    pub max_entries: usize,
    /// Maximum number of Han characters in one retained entry.
    pub max_text_characters: usize,
}

/// A bounded public lexicon slice and its source-row accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicRimeSliceImport {
    /// Selected entries ordered by descending source weight, then source line.
    pub entries: Vec<LexiconEntry>,
    /// Deterministic accounting for every accepted or skipped source row.
    pub stats: PublicRimeSliceImportStats,
}

/// Auditable row accounting for a large public Rime slice.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicRimeSliceImportStats {
    /// Non-comment rows after the Rime YAML data marker.
    pub source_rows: usize,
    /// Rows without exactly three non-empty tab-separated fields.
    pub malformed_rows: usize,
    /// Rows whose weight was not an integer representable as `u64`.
    pub invalid_weight_rows: usize,
    /// Rows with zero or negative source weight.
    pub nonpositive_weight_rows: usize,
    /// Rows containing non-Han text or exceeding the configured text limit.
    pub text_shape_rows: usize,
    /// Rows whose toned pinyin could not be normalized and encoded.
    pub unsupported_pinyin_rows: usize,
    /// Rows whose text character count differs from their pinyin syllables.
    pub text_syllable_mismatch_rows: usize,
    /// Rows exceeding the decoder's fixed syllable bound.
    pub too_many_syllable_rows: usize,
    /// Rows eligible before the configured entry cap is applied.
    pub eligible_rows: usize,
    /// Eligible rows outside the bounded best-weight frontier.
    pub dropped_by_entry_cap: usize,
    /// Duplicate text/code identities removed from the selected frontier.
    pub selected_duplicate_rows: usize,
    /// Final number of entries in the returned slice.
    pub imported_entries: usize,
    /// Lowest source weight retained, or zero when no entry survived.
    pub minimum_selected_frequency: u64,
}

/// Aggregate-only comparison of two public lexicon payloads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicLexiconComparison {
    /// Number of entries in the baseline lexicon.
    pub base_entries: usize,
    /// Number of entries in the challenger lexicon.
    pub challenger_entries: usize,
    /// Surface texts present in both lexicons, regardless of pronunciation.
    pub shared_surface_texts: usize,
    /// Surface texts present only in the baseline.
    pub base_only_surface_texts: usize,
    /// Surface texts present only in the challenger.
    pub challenger_only_surface_texts: usize,
    /// Exact `(text, canonical code)` identities present in both lexicons.
    pub shared_text_code_identities: usize,
    /// Canonical full codes present in both lexicons.
    pub shared_codes: usize,
    /// Shared codes whose highest-frequency text agrees.
    pub same_top_text_codes: usize,
    /// Shared codes whose highest-frequency text differs.
    pub changed_top_text_codes: usize,
}

/// Aggregate-only result of applying the bounded supplemental exact-word lane.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicSupplementalLayerAudit {
    /// Distinct canonical codes present in the supplemental lexicon.
    pub supplemental_codes: usize,
    /// Supplemental codes that also have at least one core exact word.
    pub shared_exact_codes: usize,
    /// Supplemental codes absent from the core exact-word lane.
    pub supplemental_only_codes: usize,
    /// Unique supplemental `(text, code)` candidates not already in core.
    pub available_new_exact_candidates: usize,
    /// New supplemental exact candidates admitted to the bounded frontier.
    pub admitted_new_exact_candidates: usize,
    /// Codes whose bounded frontier receives at least one new exact candidate.
    pub codes_receiving_new_exact_candidates: usize,
    /// Shared exact codes whose frontier receives a new exact candidate.
    pub shared_codes_receiving_new_exact_candidates: usize,
    /// Supplemental-only codes whose frontier receives a new exact candidate.
    pub supplemental_only_codes_receiving_new_exact_candidates: usize,
    /// Shared codes whose original core exact Top-1 remains first.
    pub core_top_one_preserved_codes: usize,
    /// Shared codes whose original core exact Top-1 changed.
    pub core_top_one_changed_codes: usize,
    /// Supplemental-only codes that gain a new exact Top-1.
    pub supplemental_only_codes_promoted_to_top_one: usize,
    /// Largest number of new supplemental candidates admitted for one code.
    pub maximum_admitted_new_exact_candidates_per_code: usize,
}

/// Invalid bounds for one public supplemental-layer audit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicSupplementalLayerAuditError {
    /// The visible frontier must be between one and the snapshot rank cap.
    FrontierLimit,
    /// The supplemental promotion configuration exceeds its fixed bound.
    LayerConfig(SupplementalCandidateLayerError),
}

impl fmt::Display for PublicSupplementalLayerAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrontierLimit => write!(formatter, "补充词层审计的候选上限无效"),
            Self::LayerConfig(_) => write!(formatter, "补充词层审计的影响上限无效"),
        }
    }
}

impl Error for PublicSupplementalLayerAuditError {}

impl From<SupplementalCandidateLayerError> for PublicSupplementalLayerAuditError {
    fn from(error: SupplementalCandidateLayerError) -> Self {
        Self::LayerConfig(error)
    }
}

/// Compares two already parsed public lexicons without exposing their text.
pub fn compare_public_lexicons(
    base: &[LexiconEntry],
    challenger: &[LexiconEntry],
) -> PublicLexiconComparison {
    let base_surfaces = base
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let challenger_surfaces = challenger
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let base_identities = base
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let challenger_identities = challenger
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let base_top = top_text_by_code(base);
    let challenger_top = top_text_by_code(challenger);
    let mut shared_codes = 0;
    let mut same_top_text_codes = 0;
    for (code, base_text) in &base_top {
        let Some(challenger_text) = challenger_top.get(code) else {
            continue;
        };
        shared_codes += 1;
        if base_text == challenger_text {
            same_top_text_codes += 1;
        }
    }

    PublicLexiconComparison {
        base_entries: base.len(),
        challenger_entries: challenger.len(),
        shared_surface_texts: base_surfaces.intersection(&challenger_surfaces).count(),
        base_only_surface_texts: base_surfaces.difference(&challenger_surfaces).count(),
        challenger_only_surface_texts: challenger_surfaces.difference(&base_surfaces).count(),
        shared_text_code_identities: base_identities.intersection(&challenger_identities).count(),
        shared_codes,
        same_top_text_codes,
        changed_top_text_codes: shared_codes - same_top_text_codes,
    }
}

/// Audits one core and one supplemental public lexicon without exposing text
/// or comparing unrelated raw frequency scales across the two sources.
pub fn audit_public_supplemental_layer(
    core: &[LexiconEntry],
    supplemental: &[LexiconEntry],
    frontier_limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<PublicSupplementalLayerAudit, PublicSupplementalLayerAuditError> {
    if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&frontier_limit) {
        return Err(PublicSupplementalLayerAuditError::FrontierLimit);
    }
    merge_candidate_text_layers(&[], &[], &[], frontier_limit, config)?;

    let core_by_code = exact_texts_by_code(core);
    let supplemental_by_code = exact_texts_by_code(supplemental);
    let mut audit = PublicSupplementalLayerAudit {
        supplemental_codes: supplemental_by_code.len(),
        ..PublicSupplementalLayerAudit::default()
    };

    for (code, supplemental_texts) in supplemental_by_code {
        let core_texts = core_by_code.get(code).cloned().unwrap_or_default();
        let core_set = core_texts.iter().copied().collect::<HashSet<_>>();
        let new_supplemental = supplemental_texts
            .iter()
            .copied()
            .filter(|text| !core_set.contains(text))
            .collect::<HashSet<_>>();
        audit.available_new_exact_candidates += new_supplemental.len();

        if core_texts.is_empty() {
            audit.supplemental_only_codes += 1;
        } else {
            audit.shared_exact_codes += 1;
        }

        let core_owned = core_texts
            .iter()
            .map(|text| (*text).to_owned())
            .collect::<Vec<_>>();
        let supplemental_owned = supplemental_texts
            .iter()
            .map(|text| (*text).to_owned())
            .collect::<Vec<_>>();
        let merged = merge_candidate_text_layers(
            &core_owned,
            &supplemental_owned,
            &[],
            frontier_limit,
            config,
        )?;
        let admitted = merged
            .iter()
            .filter(|text| new_supplemental.contains(text.as_str()))
            .count();
        audit.admitted_new_exact_candidates += admitted;
        audit.maximum_admitted_new_exact_candidates_per_code = audit
            .maximum_admitted_new_exact_candidates_per_code
            .max(admitted);
        if admitted != 0 {
            audit.codes_receiving_new_exact_candidates += 1;
            if core_texts.is_empty() {
                audit.supplemental_only_codes_receiving_new_exact_candidates += 1;
            } else {
                audit.shared_codes_receiving_new_exact_candidates += 1;
            }
        }

        if let Some(core_top) = core_texts.first() {
            if merged.first().map(String::as_str) == Some(*core_top) {
                audit.core_top_one_preserved_codes += 1;
            } else {
                audit.core_top_one_changed_codes += 1;
            }
        } else if merged
            .first()
            .is_some_and(|text| new_supplemental.contains(text.as_str()))
        {
            audit.supplemental_only_codes_promoted_to_top_one += 1;
        }
    }
    Ok(audit)
}

fn exact_texts_by_code(entries: &[LexiconEntry]) -> HashMap<&str, Vec<&str>> {
    let mut grouped = HashMap::<&str, Vec<(usize, &LexiconEntry)>>::new();
    for (index, entry) in entries.iter().enumerate() {
        grouped
            .entry(entry.code.as_str())
            .or_default()
            .push((index, entry));
    }
    grouped
        .into_iter()
        .map(|(code, mut candidates)| {
            candidates.sort_by(|left, right| {
                right
                    .1
                    .frequency
                    .cmp(&left.1.frequency)
                    .then_with(|| left.0.cmp(&right.0))
            });
            let mut seen = HashSet::new();
            let texts = candidates
                .into_iter()
                .map(|(_, entry)| entry.text.as_str())
                .filter(|text| seen.insert(*text))
                .collect();
            (code, texts)
        })
        .collect()
}

fn top_text_by_code(entries: &[LexiconEntry]) -> HashMap<&str, &str> {
    let mut top = HashMap::<&str, &LexiconEntry>::new();
    for entry in entries {
        match top.entry(entry.code.as_str()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let current = slot.get();
                if entry.frequency > current.frequency
                    || (entry.frequency == current.frequency && entry.text < current.text)
                {
                    slot.insert(entry);
                }
            }
        }
    }
    top.into_iter()
        .map(|(code, entry)| (code, entry.text.as_str()))
        .collect()
}

#[derive(Debug)]
struct RankedEntry {
    source_line: usize,
    entry: LexiconEntry,
}

impl RankedEntry {
    fn is_better_than(&self, other: &Self) -> bool {
        self.entry.frequency > other.entry.frequency
            || (self.entry.frequency == other.entry.frequency
                && self.source_line < other.source_line)
    }
}

impl PartialEq for RankedEntry {
    fn eq(&self, other: &Self) -> bool {
        self.source_line == other.source_line
    }
}

impl Eq for RankedEntry {}

impl PartialOrd for RankedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .entry
            .frequency
            .cmp(&self.entry.frequency)
            .then_with(|| self.source_line.cmp(&other.source_line))
    }
}

/// Selects a deterministic, Han-only, exact-syllable slice from a large
/// public Rime dictionary with Unicode tone marks.
pub fn parse_public_rime_slice(
    contents: &str,
    config: PublicRimeSliceConfig,
) -> Result<PublicRimeSliceImport, PublicRimeSliceError> {
    validate_config(config)?;
    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut selected = BinaryHeap::<RankedEntry>::with_capacity(config.max_entries);
    let mut stats = PublicRimeSliceImportStats::default();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let source_line = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if !saw_data_marker {
            match line {
                "---" => saw_document_start = true,
                "..." if saw_document_start => saw_data_marker = true,
                _ => {}
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        stats.source_rows += 1;
        let mut fields = line.split('\t');
        let (Some(text), Some(toned_pinyin), Some(weight), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            stats.malformed_rows += 1;
            continue;
        };
        if text.is_empty() || toned_pinyin.is_empty() || weight.is_empty() {
            stats.malformed_rows += 1;
            continue;
        }
        let Ok(weight) = weight.parse::<i128>() else {
            stats.invalid_weight_rows += 1;
            continue;
        };
        if weight <= 0 {
            stats.nonpositive_weight_rows += 1;
            continue;
        }
        let Ok(weight) = u64::try_from(weight) else {
            stats.invalid_weight_rows += 1;
            continue;
        };
        let text_characters = text.chars().count();
        if text_characters == 0
            || text_characters > config.max_text_characters
            || !text.chars().all(is_han_character)
        {
            stats.text_shape_rows += 1;
            continue;
        }
        let normalized_pinyin = match normalize_pinyin_tone_marks(toned_pinyin) {
            Ok(pinyin) => pinyin,
            Err(_) => {
                stats.unsupported_pinyin_rows += 1;
                continue;
            }
        };
        let encoded = match encode_pinyin_phrase(&normalized_pinyin) {
            Ok(encoded) => encoded,
            Err(_) => {
                stats.unsupported_pinyin_rows += 1;
                continue;
            }
        };
        if encoded.syllable_codes.len() != text_characters {
            stats.text_syllable_mismatch_rows += 1;
            continue;
        }
        if encoded.syllable_codes.len() > MAX_LEXICON_SYLLABLES {
            stats.too_many_syllable_rows += 1;
            continue;
        }
        stats.eligible_rows += 1;
        let ranked = RankedEntry {
            source_line,
            entry: LexiconEntry {
                text: text.to_owned(),
                pinyin: normalized_pinyin,
                code: encoded.full_code,
                syllable_codes: encoded.syllable_codes,
                frequency: weight,
            },
        };
        if selected.len() < config.max_entries {
            selected.push(ranked);
        } else if selected
            .peek()
            .is_some_and(|worst| ranked.is_better_than(worst))
        {
            selected.pop();
            selected.push(ranked);
        }
    }

    if !saw_document_start {
        return Err(PublicRimeSliceError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(PublicRimeSliceError::MissingDataMarker);
    }
    stats.dropped_by_entry_cap = stats.eligible_rows.saturating_sub(selected.len());
    let mut selected = selected.into_vec();
    selected.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    let mut identities = HashSet::<(String, KeySequence)>::new();
    selected.retain(|ranked| {
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        if identities.insert(identity) {
            true
        } else {
            stats.selected_duplicate_rows += 1;
            false
        }
    });
    let entries = selected
        .into_iter()
        .map(|ranked| ranked.entry)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(PublicRimeSliceError::Empty);
    }
    stats.imported_entries = entries.len();
    stats.minimum_selected_frequency = entries.last().map_or(0, |entry| entry.frequency);
    Ok(PublicRimeSliceImport { entries, stats })
}

fn validate_config(config: PublicRimeSliceConfig) -> Result<(), PublicRimeSliceError> {
    if config.max_entries == 0 || config.max_entries > MAX_PUBLIC_RIME_SLICE_ENTRIES {
        return Err(PublicRimeSliceError::InvalidEntryLimit);
    }
    if config.max_text_characters == 0
        || config.max_text_characters > MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS
    {
        return Err(PublicRimeSliceError::InvalidTextLimit);
    }
    Ok(())
}

fn is_han_character(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2fa1f}'
            | '\u{30000}'..='\u{323af}'
    )
}

/// Errors that prevent a deterministic public slice from being constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicRimeSliceError {
    /// The configured entry cap is zero or above the fixed experiment bound.
    InvalidEntryLimit,
    /// The text-length cap is zero or above the decoder's fixed bound.
    InvalidTextLimit,
    /// No YAML document start marker was found.
    MissingDocumentStart,
    /// No data marker followed the YAML document start.
    MissingDataMarker,
    /// No compatible row survived the explicit filters.
    Empty,
}

impl fmt::Display for PublicRimeSliceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntryLimit => write!(formatter, "公开词表裁剪的词条上限无效"),
            Self::InvalidTextLimit => write!(formatter, "公开词表裁剪的文字长度上限无效"),
            Self::MissingDocumentStart => write!(formatter, "公开 Rime 词表缺少 YAML 起始标记 ---"),
            Self::MissingDataMarker => write!(formatter, "公开 Rime 词表缺少数据起始标记 ..."),
            Self::Empty => write!(formatter, "公开 Rime 词表没有可裁剪的数据行"),
        }
    }
}

impl Error for PublicRimeSliceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_keeps_the_highest_weight_eligible_rows_with_stable_ties() {
        const SOURCE: &str = "---\nname: public\n...\n\
什么\tshén me\t50\n\
声母\tshēng mǔ\t40\n\
坏行\t字段不足\n\
忽略\thū lüe\t-1\n\
A词\ta cí\t100\n\
测试\tcè shì\t30\n\
较早\tjiào zǎo\t20\n\
较晚\tjiào wǎn\t20\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 3,
                max_text_characters: 4,
            },
        )
        .unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| (entry.text.as_str(), entry.pinyin.as_str(), entry.frequency))
                .collect::<Vec<_>>(),
            [
                ("什么", "shen me", 50),
                ("声母", "sheng mu", 40),
                ("测试", "ce shi", 30),
            ]
        );
        assert_eq!(imported.stats.source_rows, 8);
        assert_eq!(imported.stats.malformed_rows, 1);
        assert_eq!(imported.stats.nonpositive_weight_rows, 1);
        assert_eq!(imported.stats.text_shape_rows, 1);
        assert_eq!(imported.stats.eligible_rows, 5);
        assert_eq!(imported.stats.dropped_by_entry_cap, 2);
        assert_eq!(imported.stats.imported_entries, 3);
        assert_eq!(imported.stats.minimum_selected_frequency, 30);
    }

    #[test]
    fn slice_counts_unsupported_mismatched_and_duplicate_selected_rows() {
        const SOURCE: &str = "---\n...\n\
清歌\tqīng gē\t30\n\
清歌\tqīng gē\t29\n\
异常\tyì cháng\u{200b}\t28\n\
三个字\tsān gè\t27\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 4,
                max_text_characters: 4,
            },
        )
        .unwrap();

        assert_eq!(imported.entries.len(), 1);
        assert_eq!(imported.stats.unsupported_pinyin_rows, 1);
        assert_eq!(imported.stats.text_syllable_mismatch_rows, 1);
        assert_eq!(imported.stats.selected_duplicate_rows, 1);
    }

    #[test]
    fn slice_rejects_unbounded_configuration_and_missing_markers() {
        let source = "---\n...\n词\tcí\t1\n";
        assert!(
            validate_config(PublicRimeSliceConfig {
                max_entries: MAX_PUBLIC_RIME_SLICE_ENTRIES,
                max_text_characters: 1,
            })
            .is_ok()
        );
        assert_eq!(
            validate_config(PublicRimeSliceConfig {
                max_entries: MAX_PUBLIC_RIME_SLICE_ENTRIES + 1,
                max_text_characters: 1,
            }),
            Err(PublicRimeSliceError::InvalidEntryLimit)
        );
        assert_eq!(
            parse_public_rime_slice(
                source,
                PublicRimeSliceConfig {
                    max_entries: 0,
                    max_text_characters: 1,
                }
            )
            .unwrap_err(),
            PublicRimeSliceError::InvalidEntryLimit
        );
        assert!(matches!(
            parse_public_rime_slice(
                "...\n词\tcí\t1\n",
                PublicRimeSliceConfig {
                    max_entries: 1,
                    max_text_characters: 1,
                }
            ),
            Err(PublicRimeSliceError::MissingDocumentStart)
        ));
    }

    #[test]
    fn public_comparison_separates_surface_identity_and_top_text_changes() {
        let base = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
甲\tjia\t10\n\
钾\tjia\t5\n\
你好\tni hao\t10\n",
        )
        .unwrap();
        let challenger = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
甲\tjia\t4\n\
钾\tjia\t20\n\
你好\tni hao\t10\n\
新词\txin ci\t8\n",
        )
        .unwrap();

        assert_eq!(
            compare_public_lexicons(&base, &challenger),
            PublicLexiconComparison {
                base_entries: 3,
                challenger_entries: 4,
                shared_surface_texts: 3,
                base_only_surface_texts: 0,
                challenger_only_surface_texts: 1,
                shared_text_code_identities: 3,
                shared_codes: 2,
                same_top_text_codes: 1,
                changed_top_text_codes: 1,
            }
        );
    }

    #[test]
    fn supplemental_layer_audit_preserves_core_top_and_bounds_new_words() {
        let core = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
核心甲\tjia jia jia\t100\n\
核心乙\tjia jia jia\t90\n\
共有\tgong you\t100\n",
        )
        .unwrap();
        let supplemental = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
补充甲\tjia jia jia\t1000\n\
核心乙\tjia jia jia\t900\n\
补充乙\tjia jia jia\t800\n\
共有\tgong you\t999\n\
独词\tdu ci\t500\n",
        )
        .unwrap();

        assert_eq!(
            audit_public_supplemental_layer(
                &core,
                &supplemental,
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 2,
                },
            )
            .unwrap(),
            PublicSupplementalLayerAudit {
                supplemental_codes: 3,
                shared_exact_codes: 2,
                supplemental_only_codes: 1,
                available_new_exact_candidates: 3,
                admitted_new_exact_candidates: 3,
                codes_receiving_new_exact_candidates: 2,
                shared_codes_receiving_new_exact_candidates: 1,
                supplemental_only_codes_receiving_new_exact_candidates: 1,
                core_top_one_preserved_codes: 2,
                core_top_one_changed_codes: 0,
                supplemental_only_codes_promoted_to_top_one: 1,
                maximum_admitted_new_exact_candidates_per_code: 2,
            }
        );
    }

    #[test]
    fn supplemental_layer_audit_rejects_invalid_bounds() {
        let entries = crate::parse_lexicon_tsv("text\tpinyin\tfrequency\n甲\tjia\t1\n").unwrap();
        assert_eq!(
            audit_public_supplemental_layer(
                &entries,
                &entries,
                0,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap_err(),
            PublicSupplementalLayerAuditError::FrontierLimit
        );
        assert_eq!(
            audit_public_supplemental_layer(
                &entries,
                &entries,
                6,
                SupplementalCandidateLayerConfig {
                    exact_promotions: MAX_CANDIDATE_SNAPSHOT_RANK + 1,
                },
            )
            .unwrap_err(),
            PublicSupplementalLayerAuditError::LayerConfig(
                SupplementalCandidateLayerError::PromotionLimit
            )
        );
    }
}
