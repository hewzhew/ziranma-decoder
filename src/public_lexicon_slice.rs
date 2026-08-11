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
/// Largest explicitly supplied public fixed-phrase allowlist accepted by an
/// offline audit.
pub const MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_BYTES: usize = 1024 * 1024;
/// Maximum valid fixed phrases retained by one offline allowlist import.
pub const MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES: usize = 50_000;
/// Conservative entry cap for one experimental public slice.
///
/// This remains below the validated snapshot ceiling while allowing a narrow
/// cutoff-sensitivity experiment beyond the original Top-100k frontier.
pub const MAX_PUBLIC_RIME_SLICE_ENTRIES: usize = 120_000;
/// Longest Han-only entry accepted by the first slice experiment.
pub const MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS: usize = 12;
/// Deepest per-code two-character identity tier admitted by one slice audit.
///
/// The bound keeps source scanning and package effects explicit. A depth of
/// one preserves the original missing-code coverage behavior.
pub const MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH: usize = 8;
/// Largest independent public word list accepted by the short-word consensus audit.
pub const MAX_PUBLIC_SHORT_WORD_CONFIRMATION_BYTES: usize = 16 * 1024 * 1024;
/// Largest bounded short-word consensus overlay produced by one audit.
pub const MAX_PUBLIC_SHORT_WORD_CONSENSUS_ENTRIES: usize = 50_000;

/// Explicit deterministic limits for one public Rime slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicRimeSliceConfig {
    /// Maximum retained entries after ranking and deduplication.
    pub max_entries: usize,
    /// Entries reserved for the ordinary global source-weight frontier.
    ///
    /// Explicit three/four-character quotas are applied next; any remaining
    /// capacity retains the strongest public two-character whole word for
    /// canonical full codes that the global frontier did not cover. Setting
    /// this equal to [`Self::max_entries`] preserves pure Top-N selection.
    pub frequency_frontier_entries: usize,
    /// Number of strongest distinct two-character identities retained per
    /// canonical full code by the post-frontier coverage lane.
    ///
    /// Depth one only represents codes missing from the frequency frontier.
    /// Larger values explicitly audit same-code candidate depth without
    /// enlarging [`Self::max_entries`].
    pub two_character_coverage_depth: usize,
    /// Maximum missing three-character codes represented after the global
    /// frontier and before the two-character coverage reserve.
    ///
    /// The default CLI value is zero, so existing slice commands retain their
    /// original selection. Each admitted row represents one canonical code.
    pub three_character_coverage_entries: usize,
    /// Maximum missing four-character codes represented after the global
    /// frontier and before the two-character coverage reserve.
    ///
    /// The default CLI value is zero. Together with the three-character quota,
    /// this remains inside [`Self::max_entries`] rather than enlarging a
    /// runtime snapshot.
    pub four_character_coverage_entries: usize,
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

/// Bounded two-source short-word intersection for an offline audit.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicShortWordConsensusImport {
    /// New entries ordered by coverage tier and source-internal weight.
    pub entries: Vec<LexiconEntry>,
    /// Aggregate accounting that does not expose unrelated source text.
    pub stats: PublicShortWordConsensusImportStats,
}

/// Aggregate accounting for a two-source short-word intersection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicShortWordConsensusImportStats {
    /// Non-empty, non-comment rows in the independent confirmation list.
    pub confirmation_rows: usize,
    /// Unique two-Han surfaces accepted from the confirmation list.
    pub eligible_confirmation_surfaces: usize,
    /// Repeated eligible confirmation surfaces ignored.
    pub duplicate_confirmation_surfaces: usize,
    /// Non-comment Rime rows scanned after the YAML data marker.
    pub source_rows: usize,
    /// Rime rows whose surface was independently confirmed.
    pub confirmed_source_rows: usize,
    /// Confirmed rows with a usable two-syllable reading and positive weight.
    pub valid_confirmed_source_rows: usize,
    /// Unique valid confirmed text/code identities before base exclusion.
    pub confirmed_identities: usize,
    /// Confirmed identities already present in the supplied base payload.
    pub base_identities: usize,
    /// New confirmed identities available after base exclusion.
    pub available_new_identities: usize,
    /// Canonical codes represented by at least one new confirmed identity.
    pub available_new_codes: usize,
    /// Maximum identities considered per canonical code.
    pub per_code_depth: usize,
    /// New identities retained within the explicit output bound.
    pub imported_entries: usize,
    /// Tiered identities excluded by the explicit output bound.
    pub dropped_by_entry_cap: usize,
}

/// Exact four-character entries selected by one public fixed-phrase list.
///
/// This is an audit input, not an installable candidate package. The caller
/// must still authenticate every source material before constructing a
/// package from more than one upstream file.
#[derive(Clone, Debug, PartialEq)]
pub struct PublicRimePhraseAllowlistImport {
    /// Matched source entries ordered by descending source weight, then line.
    pub entries: Vec<LexiconEntry>,
    /// Deterministic accounting for the allowlist/source intersection.
    pub stats: PublicRimePhraseAllowlistImportStats,
}

/// Aggregate accounting for a public fixed-phrase allowlist import.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicRimePhraseAllowlistImportStats {
    /// Non-empty, non-comment rows in the allowlist.
    pub allowlist_rows: usize,
    /// Allowlist rows without one lowercase ASCII key and phrase field.
    pub malformed_allowlist_rows: usize,
    /// Phrase fields rejected because they are not exactly four Han characters.
    pub non_four_character_terms: usize,
    /// Repeated valid phrase surfaces ignored after their first occurrence.
    pub duplicate_terms: usize,
    /// Unique valid four-character phrase surfaces.
    pub eligible_terms: usize,
    /// Non-comment rows after the Rime YAML data marker.
    pub source_rows: usize,
    /// Source rows whose surface was requested by the allowlist.
    pub allowlisted_source_rows: usize,
    /// Requested source rows that could not be normalized into four syllables.
    pub invalid_allowlisted_source_rows: usize,
    /// Unique requested surfaces with at least one usable source entry.
    pub matched_terms: usize,
    /// Usable matched surfaces outside the explicit import bound.
    pub dropped_by_entry_cap: usize,
    /// Final number of entries returned by the audit import.
    pub imported_entries: usize,
}

/// Aggregate evidence for one explicitly supplied public target identity.
///
/// This lookup scans one exact public source without retaining unrelated rows.
/// It is intended to distinguish source absence from later package selection;
/// it does not claim that source inclusion makes a target linguistically good.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicRimeTargetAudit {
    /// Non-comment data rows observed after the Rime YAML marker.
    pub source_rows: usize,
    /// Rows whose text exactly matches the requested public target.
    pub surface_rows: usize,
    /// Matching surfaces with a positive weight and a usable full reading.
    pub valid_surface_rows: usize,
    /// Valid matching surfaces whose canonical full code is the requested code.
    pub exact_code_rows: usize,
    /// Highest public weight among exact-code rows.
    pub highest_exact_frequency: Option<u64>,
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
    /// Unique canonical full codes represented by eligible two-character rows.
    pub eligible_two_character_codes: usize,
    /// Distinct two-character identities represented by the configured
    /// per-code depth tiers before the package cap is applied.
    pub eligible_two_character_depth_entries: usize,
    /// Unique canonical full codes represented by eligible three-character rows.
    pub eligible_three_character_codes: usize,
    /// Unique canonical full codes represented by eligible four-character rows.
    pub eligible_four_character_codes: usize,
    /// Unique two-character codes already represented by the global frontier.
    pub frequency_frontier_two_character_codes: usize,
    /// Unique three-character codes represented by the global frontier.
    pub frequency_frontier_three_character_codes: usize,
    /// Unique four-character codes represented by the global frontier.
    pub frequency_frontier_four_character_codes: usize,
    /// Global source-weight rows retained before identity deduplication.
    pub frequency_frontier_selected_rows: usize,
    /// Missing two-character codes considered for the coverage reserve.
    pub two_character_coverage_candidates: usize,
    /// Coverage-reserve rows admitted within the overall entry cap.
    pub two_character_coverage_admitted: usize,
    /// Coverage candidates that did not fit within the overall entry cap.
    pub two_character_coverage_dropped: usize,
    /// Configured per-code depth used for two-character coverage.
    pub two_character_coverage_depth: usize,
    /// Missing three-character codes considered for their explicit quota.
    pub three_character_coverage_candidates: usize,
    /// Three-character code representatives admitted by their explicit quota.
    pub three_character_coverage_admitted: usize,
    /// Three-character coverage candidates outside their explicit quota.
    pub three_character_coverage_dropped: usize,
    /// Missing four-character codes considered for their explicit quota.
    pub four_character_coverage_candidates: usize,
    /// Four-character code representatives admitted by their explicit quota.
    pub four_character_coverage_admitted: usize,
    /// Four-character coverage candidates outside their explicit quota.
    pub four_character_coverage_dropped: usize,
    /// Additional global-frequency rows admitted after code coverage.
    pub frequency_backfill_admitted: usize,
    /// Unique three-character codes represented by the final slice.
    pub imported_three_character_codes: usize,
    /// Unique four-character codes represented by the final slice.
    pub imported_four_character_codes: usize,
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
    /// Differing Top-1 codes where the challenger Top-1 is independently
    /// present under the same complete code in the baseline lexicon.
    pub consensus_top_reorder_eligible_codes: usize,
    /// Reorder-eligible codes whose challenger Top-1 was originally second
    /// in the baseline's exact full-code order.
    pub consensus_top_original_rank_two_codes: usize,
    /// Reorder-eligible codes whose challenger Top-1 was originally between
    /// third and sixth in the baseline's exact full-code order.
    pub consensus_top_original_rank_three_to_six_codes: usize,
    /// Reorder-eligible codes whose challenger Top-1 was originally seventh
    /// or later in the baseline's exact full-code order.
    pub consensus_top_original_rank_seven_or_later_codes: usize,
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
    let base_exact_texts = exact_texts_by_code(base);
    let mut shared_codes = 0;
    let mut same_top_text_codes = 0;
    let mut consensus_top_reorder_eligible_codes = 0;
    let mut consensus_top_original_rank_two_codes = 0;
    let mut consensus_top_original_rank_three_to_six_codes = 0;
    let mut consensus_top_original_rank_seven_or_later_codes = 0;
    for (code, base_text) in &base_top {
        let Some(challenger_text) = challenger_top.get(code) else {
            continue;
        };
        shared_codes += 1;
        if base_text == challenger_text {
            same_top_text_codes += 1;
        } else if base_identities.contains(&(*challenger_text, *code))
            && let Some(rank) = base_exact_texts.get(code).and_then(|texts| {
                texts
                    .iter()
                    .position(|text| *text == *challenger_text)
                    .map(|index| index + 1)
            })
        {
            // A different Top-1 cannot occupy baseline rank one because both
            // rankings use the same exact full-code ordering below.
            match rank {
                2 => consensus_top_original_rank_two_codes += 1,
                3..=6 => consensus_top_original_rank_three_to_six_codes += 1,
                7.. => consensus_top_original_rank_seven_or_later_codes += 1,
                _ => continue,
            }
            consensus_top_reorder_eligible_codes += 1;
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
        consensus_top_reorder_eligible_codes,
        consensus_top_original_rank_two_codes,
        consensus_top_original_rank_three_to_six_codes,
        consensus_top_original_rank_seven_or_later_codes,
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
                    .then_with(|| left.1.text.cmp(&right.1.text))
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

#[derive(Clone, Debug)]
struct RankedEntry {
    source_line: usize,
    entry: LexiconEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RankedEntryReference {
    source_line: usize,
    frequency: u64,
}

impl RankedEntryReference {
    fn is_better_than(self, other: Self) -> bool {
        self.frequency > other.frequency
            || (self.frequency == other.frequency && self.source_line < other.source_line)
    }
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

fn retain_best_ranked_reference(
    entries: &mut HashMap<KeySequence, RankedEntryReference>,
    code: KeySequence,
    candidate: RankedEntryReference,
) {
    match entries.entry(code) {
        std::collections::hash_map::Entry::Vacant(slot) => {
            slot.insert(candidate);
        }
        std::collections::hash_map::Entry::Occupied(mut slot) => {
            if candidate.is_better_than(*slot.get()) {
                slot.insert(candidate);
            }
        }
    }
}

fn retain_ranked_entries_up_to_depth(
    entries: &mut HashMap<KeySequence, Vec<RankedEntry>>,
    code: KeySequence,
    candidate: RankedEntry,
    depth: usize,
) {
    let ranked = entries.entry(code).or_default();
    if let Some(existing) = ranked
        .iter_mut()
        .find(|existing| existing.entry.text == candidate.entry.text)
    {
        if candidate.is_better_than(existing) {
            *existing = candidate;
        }
    } else {
        ranked.push(candidate);
    }
    ranked.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    ranked.truncate(depth);
}

fn select_ranked_coverage_references(
    entries: HashMap<KeySequence, RankedEntryReference>,
    frequency_frontier_codes: &HashSet<KeySequence>,
) -> Vec<RankedEntryReference> {
    let mut selected = entries
        .into_iter()
        .filter_map(|(code, reference)| {
            (!frequency_frontier_codes.contains(&code)).then_some(reference)
        })
        .collect::<Vec<_>>();
    selected.sort_unstable_by(|left, right| {
        right
            .frequency
            .cmp(&left.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    selected
}

fn materialize_ranked_entries(contents: &str, source_lines: &HashSet<usize>) -> Vec<RankedEntry> {
    if source_lines.is_empty() {
        return Vec::new();
    }
    let mut entries = Vec::with_capacity(source_lines.len());
    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let source_line = zero_based_line + 1;
        if !source_lines.contains(&source_line) {
            continue;
        }
        let line = raw_line.trim_end_matches('\r');
        let mut fields = line.split('\t');
        let text = fields
            .next()
            .expect("a selected public source row retains its text field");
        let toned_pinyin = fields
            .next()
            .expect("a selected public source row retains its pinyin field");
        let frequency = fields
            .next()
            .expect("a selected public source row retains its weight field")
            .parse::<u64>()
            .expect("a selected public source row retains its validated positive weight");
        debug_assert!(fields.next().is_none());
        let pinyin = normalize_pinyin_tone_marks(toned_pinyin)
            .expect("a selected public source row retains its validated pinyin");
        let encoded = encode_pinyin_phrase(&pinyin)
            .expect("a selected public source row retains its encodable pinyin");
        entries.push(RankedEntry {
            source_line,
            entry: LexiconEntry {
                text: text.to_owned(),
                pinyin,
                code: encoded.full_code,
                syllable_codes: encoded.syllable_codes,
                frequency,
            },
        });
    }
    assert_eq!(
        entries.len(),
        source_lines.len(),
        "every selected public source row remains materializable"
    );
    entries
}

/// Intersects independently supplied public surfaces with one toned Rime source.
///
/// The confirmation source contributes only a binary surface-membership signal;
/// pinyin, canonical codes, and source-internal ranking always come from the
/// Rime source. Existing base identities are excluded before a bounded number
/// of candidates per complete code are retained.
pub fn parse_public_short_word_consensus(
    source_contents: &str,
    confirmation_contents: &str,
    base: &[LexiconEntry],
    per_code_depth: usize,
    max_entries: usize,
) -> Result<PublicShortWordConsensusImport, PublicRimeSliceError> {
    if per_code_depth == 0 || per_code_depth > MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH {
        return Err(PublicRimeSliceError::InvalidTwoCharacterCoverageDepth);
    }
    if max_entries == 0 || max_entries > MAX_PUBLIC_SHORT_WORD_CONSENSUS_ENTRIES {
        return Err(PublicRimeSliceError::InvalidShortWordConsensusEntryLimit);
    }

    let mut stats = PublicShortWordConsensusImportStats {
        per_code_depth,
        ..PublicShortWordConsensusImportStats::default()
    };
    let mut confirmed_surfaces = HashSet::<String>::new();
    for raw_line in confirmation_contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        stats.confirmation_rows += 1;
        let Some(surface) = line.split_whitespace().next() else {
            continue;
        };
        if surface.chars().count() != 2 || !surface.chars().all(is_han_character) {
            continue;
        }
        if !confirmed_surfaces.insert(surface.to_owned()) {
            stats.duplicate_confirmation_surfaces += 1;
        }
    }
    stats.eligible_confirmation_surfaces = confirmed_surfaces.len();
    if confirmed_surfaces.is_empty() {
        return Err(PublicRimeSliceError::EmptyShortWordConfirmation);
    }

    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut best_by_identity = HashMap::<(String, KeySequence), RankedEntry>::new();
    for (zero_based_line, raw_line) in source_contents.lines().enumerate() {
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
            continue;
        };
        if !confirmed_surfaces.contains(text) {
            continue;
        }
        stats.confirmed_source_rows += 1;
        let Some(weight) = weight.parse::<u64>().ok().filter(|weight| *weight != 0) else {
            continue;
        };
        let Some((pinyin, encoded)) = normalize_pinyin_tone_marks(toned_pinyin)
            .ok()
            .and_then(|pinyin| {
                encode_pinyin_phrase(&pinyin)
                    .ok()
                    .map(|encoded| (pinyin, encoded))
            })
            .filter(|(_, encoded)| encoded.syllable_codes.len() == 2)
        else {
            continue;
        };
        stats.valid_confirmed_source_rows += 1;
        let ranked = RankedEntry {
            source_line,
            entry: LexiconEntry {
                text: text.to_owned(),
                pinyin,
                code: encoded.full_code,
                syllable_codes: encoded.syllable_codes,
                frequency: weight,
            },
        };
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        match best_by_identity.entry(identity) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ranked);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if ranked.is_better_than(slot.get()) {
                    slot.insert(ranked);
                }
            }
        }
    }
    if !saw_document_start {
        return Err(PublicRimeSliceError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(PublicRimeSliceError::MissingDataMarker);
    }
    stats.confirmed_identities = best_by_identity.len();

    let base_identities = base
        .iter()
        .map(|entry| (entry.text.as_str(), entry.code.as_str()))
        .collect::<HashSet<_>>();
    let mut by_code = HashMap::<KeySequence, Vec<RankedEntry>>::new();
    for ((text, code), ranked) in best_by_identity {
        if base_identities.contains(&(text.as_str(), code.as_str())) {
            stats.base_identities += 1;
            continue;
        }
        stats.available_new_identities += 1;
        retain_ranked_entries_up_to_depth(
            &mut by_code,
            ranked.entry.code.clone(),
            ranked,
            per_code_depth,
        );
    }
    stats.available_new_codes = by_code.len();

    let mut selected = Vec::new();
    for depth_index in 0..per_code_depth {
        let mut tier = by_code
            .values()
            .filter_map(|entries| entries.get(depth_index).cloned())
            .collect::<Vec<_>>();
        tier.sort_by(|left, right| {
            right
                .entry
                .frequency
                .cmp(&left.entry.frequency)
                .then_with(|| left.source_line.cmp(&right.source_line))
        });
        selected.extend(tier);
    }
    stats.dropped_by_entry_cap = selected.len().saturating_sub(max_entries);
    selected.truncate(max_entries);
    let entries = selected
        .into_iter()
        .map(|ranked| ranked.entry)
        .collect::<Vec<_>>();
    stats.imported_entries = entries.len();
    Ok(PublicShortWordConsensusImport { entries, stats })
}

/// Intersects one public fixed-phrase allowlist with a toned Rime dictionary.
///
/// The allowlist format is deliberately narrow: every data row is
/// `lowercase-key<TAB>phrase`, and additional phrases on the same row are
/// separated by the two literal characters `\t`. Only unique four-Han
/// surfaces are retained. Pinyin and frequency always come from the Rime
/// source, so this helper never invents a reading from the allowlist key.
pub fn parse_public_rime_phrase_allowlist(
    source_contents: &str,
    allowlist_contents: &str,
    max_entries: usize,
) -> Result<PublicRimePhraseAllowlistImport, PublicRimeSliceError> {
    if max_entries == 0 || max_entries > MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES {
        return Err(PublicRimeSliceError::InvalidPhraseAllowlistEntryLimit);
    }

    let mut stats = PublicRimePhraseAllowlistImportStats::default();
    let mut allowed = HashSet::<String>::new();
    for raw_line in allowlist_contents.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        stats.allowlist_rows += 1;
        let Some((key, phrases)) = line.split_once('\t') else {
            stats.malformed_allowlist_rows += 1;
            continue;
        };
        if key.is_empty()
            || phrases.is_empty()
            || !key.as_bytes().iter().all(u8::is_ascii_lowercase)
        {
            stats.malformed_allowlist_rows += 1;
            continue;
        }
        for phrase in phrases.split("\\t") {
            if phrase.chars().count() != 4 || !phrase.chars().all(is_han_character) {
                stats.non_four_character_terms += 1;
            } else if !allowed.insert(phrase.to_owned()) {
                stats.duplicate_terms += 1;
            }
        }
    }
    stats.eligible_terms = allowed.len();
    if allowed.is_empty() {
        return Err(PublicRimeSliceError::EmptyPhraseAllowlist);
    }

    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut best_by_text = HashMap::<String, RankedEntry>::new();
    for (zero_based_line, raw_line) in source_contents.lines().enumerate() {
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
            continue;
        };
        if !allowed.contains(text) {
            continue;
        }
        stats.allowlisted_source_rows += 1;
        let Some(weight) = weight.parse::<u64>().ok().filter(|weight| *weight != 0) else {
            stats.invalid_allowlisted_source_rows += 1;
            continue;
        };
        let Some((normalized_pinyin, encoded)) = normalize_pinyin_tone_marks(toned_pinyin)
            .ok()
            .and_then(|pinyin| {
                encode_pinyin_phrase(&pinyin)
                    .ok()
                    .map(|encoded| (pinyin, encoded))
            })
            .filter(|(_, encoded)| encoded.syllable_codes.len() == 4)
        else {
            stats.invalid_allowlisted_source_rows += 1;
            continue;
        };
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
        match best_by_text.entry(text.to_owned()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ranked);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if ranked.is_better_than(slot.get()) {
                    slot.insert(ranked);
                }
            }
        }
    }
    if !saw_document_start {
        return Err(PublicRimeSliceError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(PublicRimeSliceError::MissingDataMarker);
    }
    stats.matched_terms = best_by_text.len();
    if best_by_text.is_empty() {
        return Err(PublicRimeSliceError::Empty);
    }
    let mut ranked = best_by_text.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    stats.dropped_by_entry_cap = ranked.len().saturating_sub(max_entries);
    ranked.truncate(max_entries);
    let entries = ranked
        .into_iter()
        .map(|ranked| ranked.entry)
        .collect::<Vec<_>>();
    stats.imported_entries = entries.len();
    Ok(PublicRimePhraseAllowlistImport { entries, stats })
}

/// Finds aggregate evidence for one public text/code identity in a toned Rime source.
///
/// The target must contain one to twelve Han characters and the code must be one
/// complete two-key syllable per character. Unrelated malformed source rows are
/// ignored just as in the bounded public-slice importer.
pub fn audit_public_rime_target(
    contents: &str,
    target_text: &str,
    target_code: &str,
) -> Result<PublicRimeTargetAudit, PublicRimeSliceError> {
    let target_characters = target_text.chars().count();
    if target_characters == 0
        || target_characters > MAX_PUBLIC_RIME_SLICE_TEXT_CHARACTERS
        || !target_text.chars().all(is_han_character)
        || target_code.len() != target_characters * 2
        || !target_code.as_bytes().iter().all(u8::is_ascii_lowercase)
    {
        return Err(PublicRimeSliceError::InvalidTarget);
    }

    let mut saw_document_start = false;
    let mut saw_data_marker = false;
    let mut audit = PublicRimeTargetAudit::default();
    for raw_line in contents.lines() {
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
        audit.source_rows += 1;
        let mut fields = line.split('\t');
        let (Some(text), Some(toned_pinyin), Some(weight), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if text != target_text {
            continue;
        }
        audit.surface_rows += 1;
        let Some(weight) = weight.parse::<u64>().ok().filter(|weight| *weight != 0) else {
            continue;
        };
        let Some(encoded) = normalize_pinyin_tone_marks(toned_pinyin)
            .ok()
            .and_then(|pinyin| encode_pinyin_phrase(&pinyin).ok())
            .filter(|encoded| encoded.syllable_codes.len() == target_characters)
        else {
            continue;
        };
        audit.valid_surface_rows += 1;
        if encoded.full_code.as_str() == target_code {
            audit.exact_code_rows += 1;
            audit.highest_exact_frequency = Some(
                audit
                    .highest_exact_frequency
                    .map_or(weight, |current| current.max(weight)),
            );
        }
    }
    if !saw_document_start {
        return Err(PublicRimeSliceError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(PublicRimeSliceError::MissingDataMarker);
    }
    if audit.source_rows == 0 {
        return Err(PublicRimeSliceError::Empty);
    }
    Ok(audit)
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
    let mut global_frontier = BinaryHeap::<RankedEntry>::with_capacity(config.max_entries);
    let mut best_two_character_entries_by_code = HashMap::<KeySequence, Vec<RankedEntry>>::new();
    let mut best_three_character_entry_by_code =
        HashMap::<KeySequence, RankedEntryReference>::new();
    let mut best_four_character_entry_by_code = HashMap::<KeySequence, RankedEntryReference>::new();
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
        if text_characters == 2 {
            retain_ranked_entries_up_to_depth(
                &mut best_two_character_entries_by_code,
                ranked.entry.code.clone(),
                ranked.clone(),
                config.two_character_coverage_depth,
            );
        } else if text_characters == 3 {
            retain_best_ranked_reference(
                &mut best_three_character_entry_by_code,
                ranked.entry.code.clone(),
                RankedEntryReference {
                    source_line,
                    frequency: weight,
                },
            );
        } else if text_characters == 4 {
            retain_best_ranked_reference(
                &mut best_four_character_entry_by_code,
                ranked.entry.code.clone(),
                RankedEntryReference {
                    source_line,
                    frequency: weight,
                },
            );
        }
        if global_frontier.len() < config.max_entries {
            global_frontier.push(ranked);
        } else if global_frontier
            .peek()
            .is_some_and(|worst| ranked.is_better_than(worst))
        {
            global_frontier.pop();
            global_frontier.push(ranked);
        }
    }

    if !saw_document_start {
        return Err(PublicRimeSliceError::MissingDocumentStart);
    }
    if !saw_data_marker {
        return Err(PublicRimeSliceError::MissingDataMarker);
    }
    stats.eligible_two_character_codes = best_two_character_entries_by_code.len();
    stats.eligible_two_character_depth_entries = best_two_character_entries_by_code
        .values()
        .map(Vec::len)
        .sum();
    stats.two_character_coverage_depth = config.two_character_coverage_depth;
    stats.eligible_three_character_codes = best_three_character_entry_by_code.len();
    stats.eligible_four_character_codes = best_four_character_entry_by_code.len();
    let mut global_frontier = global_frontier.into_vec();
    global_frontier.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    let frequency_overflow = if global_frontier.len() > config.frequency_frontier_entries {
        global_frontier.split_off(config.frequency_frontier_entries)
    } else {
        Vec::new()
    };
    stats.frequency_frontier_selected_rows = global_frontier.len();
    let mut selected = global_frontier;
    let mut identities = HashSet::<(String, KeySequence)>::new();
    let mut selected_source_lines = HashSet::new();
    selected.retain(|ranked| {
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        if identities.insert(identity) {
            selected_source_lines.insert(ranked.source_line);
            true
        } else {
            stats.selected_duplicate_rows += 1;
            false
        }
    });
    let frequency_frontier_codes = selected
        .iter()
        .filter(|ranked| ranked.entry.syllable_codes.len() == 2)
        .map(|ranked| ranked.entry.code.clone())
        .collect::<HashSet<_>>();
    stats.frequency_frontier_two_character_codes = frequency_frontier_codes.len();
    let frequency_frontier_three_character_codes = selected
        .iter()
        .filter(|ranked| ranked.entry.syllable_codes.len() == 3)
        .map(|ranked| ranked.entry.code.clone())
        .collect::<HashSet<_>>();
    stats.frequency_frontier_three_character_codes = frequency_frontier_three_character_codes.len();
    let frequency_frontier_four_character_codes = selected
        .iter()
        .filter(|ranked| ranked.entry.syllable_codes.len() == 4)
        .map(|ranked| ranked.entry.code.clone())
        .collect::<HashSet<_>>();
    stats.frequency_frontier_four_character_codes = frequency_frontier_four_character_codes.len();

    let mut three_character_coverage = select_ranked_coverage_references(
        best_three_character_entry_by_code,
        &frequency_frontier_three_character_codes,
    );
    stats.three_character_coverage_candidates = three_character_coverage.len();
    three_character_coverage.truncate(config.three_character_coverage_entries);
    stats.three_character_coverage_admitted = three_character_coverage.len();
    stats.three_character_coverage_dropped = stats
        .three_character_coverage_candidates
        .saturating_sub(stats.three_character_coverage_admitted);

    let mut four_character_coverage = select_ranked_coverage_references(
        best_four_character_entry_by_code,
        &frequency_frontier_four_character_codes,
    );
    stats.four_character_coverage_candidates = four_character_coverage.len();
    four_character_coverage.truncate(config.four_character_coverage_entries);
    stats.four_character_coverage_admitted = four_character_coverage.len();
    stats.four_character_coverage_dropped = stats
        .four_character_coverage_candidates
        .saturating_sub(stats.four_character_coverage_admitted);

    let three_character_source_lines = three_character_coverage
        .iter()
        .map(|reference| reference.source_line)
        .collect::<HashSet<_>>();
    let four_character_source_lines = four_character_coverage
        .iter()
        .map(|reference| reference.source_line)
        .collect::<HashSet<_>>();
    let selected_long_source_lines = three_character_source_lines
        .union(&four_character_source_lines)
        .copied()
        .collect::<HashSet<_>>();
    for ranked in materialize_ranked_entries(contents, &selected_long_source_lines) {
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        if identities.insert(identity) {
            selected_source_lines.insert(ranked.source_line);
            selected.push(ranked);
        }
    }
    let mut coverage = best_two_character_entries_by_code
        .values()
        .filter_map(|ranked| ranked.first().cloned())
        .filter(|ranked| {
            !identities.contains(&(ranked.entry.text.clone(), ranked.entry.code.clone()))
        })
        .collect::<Vec<_>>();
    coverage.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    let mut deeper_coverage = best_two_character_entries_by_code
        .values()
        .flat_map(|ranked| ranked.iter().skip(1).cloned())
        .filter(|ranked| {
            !identities.contains(&(ranked.entry.text.clone(), ranked.entry.code.clone()))
        })
        .collect::<Vec<_>>();
    deeper_coverage.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    coverage.extend(deeper_coverage);
    stats.two_character_coverage_candidates = coverage.len();
    for ranked in coverage {
        if selected.len() == config.max_entries {
            stats.two_character_coverage_dropped += 1;
            continue;
        }
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        if identities.insert(identity) {
            selected_source_lines.insert(ranked.source_line);
            selected.push(ranked);
            stats.two_character_coverage_admitted += 1;
        }
    }
    for ranked in frequency_overflow {
        if selected.len() == config.max_entries {
            break;
        }
        if selected_source_lines.contains(&ranked.source_line) {
            continue;
        }
        let identity = (ranked.entry.text.clone(), ranked.entry.code.clone());
        if identities.insert(identity) {
            selected_source_lines.insert(ranked.source_line);
            selected.push(ranked);
            stats.frequency_backfill_admitted += 1;
        } else {
            stats.selected_duplicate_rows += 1;
        }
    }
    selected.sort_by(|left, right| {
        right
            .entry
            .frequency
            .cmp(&left.entry.frequency)
            .then_with(|| left.source_line.cmp(&right.source_line))
    });
    stats.imported_three_character_codes = selected
        .iter()
        .filter(|ranked| ranked.entry.syllable_codes.len() == 3)
        .map(|ranked| ranked.entry.code.clone())
        .collect::<HashSet<_>>()
        .len();
    stats.imported_four_character_codes = selected
        .iter()
        .filter(|ranked| ranked.entry.syllable_codes.len() == 4)
        .map(|ranked| ranked.entry.code.clone())
        .collect::<HashSet<_>>()
        .len();
    stats.dropped_by_entry_cap = stats
        .eligible_rows
        .saturating_sub(selected.len().saturating_add(stats.selected_duplicate_rows));
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
    if config.frequency_frontier_entries == 0
        || config.frequency_frontier_entries > config.max_entries
    {
        return Err(PublicRimeSliceError::InvalidFrequencyFrontierLimit);
    }
    if config.two_character_coverage_depth == 0
        || config.two_character_coverage_depth > MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH
    {
        return Err(PublicRimeSliceError::InvalidTwoCharacterCoverageDepth);
    }
    if config
        .three_character_coverage_entries
        .saturating_add(config.four_character_coverage_entries)
        > config
            .max_entries
            .saturating_sub(config.frequency_frontier_entries)
    {
        return Err(PublicRimeSliceError::InvalidLongWordCoverageLimit);
    }
    Ok(())
}

pub(crate) fn is_han_character(character: char) -> bool {
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
    /// The fixed-phrase import bound is zero or above its audit ceiling.
    InvalidPhraseAllowlistEntryLimit,
    /// The short-word consensus output bound is zero or above its audit ceiling.
    InvalidShortWordConsensusEntryLimit,
    /// The global frequency frontier is zero or exceeds the total entry cap.
    InvalidFrequencyFrontierLimit,
    /// Per-code two-character coverage depth is zero or above its fixed bound.
    InvalidTwoCharacterCoverageDepth,
    /// Explicit three/four-character coverage exceeds post-frontier capacity.
    InvalidLongWordCoverageLimit,
    /// The text-length cap is zero or above the decoder's fixed bound.
    InvalidTextLimit,
    /// A targeted audit received non-Han text or a non-complete canonical code.
    InvalidTarget,
    /// No YAML document start marker was found.
    MissingDocumentStart,
    /// No data marker followed the YAML document start.
    MissingDataMarker,
    /// No compatible row survived the explicit filters.
    Empty,
    /// The fixed-phrase allowlist contained no valid four-Han surface.
    EmptyPhraseAllowlist,
    /// The independent confirmation source contained no valid two-Han surface.
    EmptyShortWordConfirmation,
}

impl fmt::Display for PublicRimeSliceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEntryLimit => write!(formatter, "公开词表裁剪的词条上限无效"),
            Self::InvalidPhraseAllowlistEntryLimit => {
                write!(formatter, "公开固定短语审计的词条上限无效")
            }
            Self::InvalidShortWordConsensusEntryLimit => {
                write!(formatter, "公开短词共识审计的词条上限无效")
            }
            Self::InvalidFrequencyFrontierLimit => {
                write!(formatter, "公开词表裁剪的全局高频前沿上限无效")
            }
            Self::InvalidTwoCharacterCoverageDepth => {
                write!(formatter, "公开词表裁剪的双字同码覆盖深度无效")
            }
            Self::InvalidLongWordCoverageLimit => {
                write!(formatter, "公开词表裁剪的三字、四字覆盖配额超出剩余容量")
            }
            Self::InvalidTextLimit => write!(formatter, "公开词表裁剪的文字长度上限无效"),
            Self::InvalidTarget => write!(formatter, "公开词表目标或完整双拼码无效"),
            Self::MissingDocumentStart => write!(formatter, "公开 Rime 词表缺少 YAML 起始标记 ---"),
            Self::MissingDataMarker => write!(formatter, "公开 Rime 词表缺少数据起始标记 ..."),
            Self::Empty => write!(formatter, "公开 Rime 词表没有可裁剪的数据行"),
            Self::EmptyPhraseAllowlist => write!(formatter, "公开固定短语表没有合格的四字词面"),
            Self::EmptyShortWordConfirmation => {
                write!(formatter, "公开短词确认来源没有合格的双字词面")
            }
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
                frequency_frontier_entries: 3,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
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
                frequency_frontier_entries: 4,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
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
                frequency_frontier_entries: MAX_PUBLIC_RIME_SLICE_ENTRIES,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 1,
            })
            .is_ok()
        );
        assert_eq!(
            validate_config(PublicRimeSliceConfig {
                max_entries: MAX_PUBLIC_RIME_SLICE_ENTRIES + 1,
                frequency_frontier_entries: MAX_PUBLIC_RIME_SLICE_ENTRIES,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 1,
            }),
            Err(PublicRimeSliceError::InvalidEntryLimit)
        );
        assert_eq!(
            validate_config(PublicRimeSliceConfig {
                max_entries: 10,
                frequency_frontier_entries: 11,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 1,
            }),
            Err(PublicRimeSliceError::InvalidFrequencyFrontierLimit)
        );
        assert_eq!(
            validate_config(PublicRimeSliceConfig {
                max_entries: 10,
                frequency_frontier_entries: 9,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 1,
                four_character_coverage_entries: 1,
                max_text_characters: 1,
            }),
            Err(PublicRimeSliceError::InvalidLongWordCoverageLimit)
        );
        assert_eq!(
            validate_config(PublicRimeSliceConfig {
                max_entries: 10,
                frequency_frontier_entries: 10,
                two_character_coverage_depth: MAX_PUBLIC_RIME_TWO_CHARACTER_COVERAGE_DEPTH + 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 1,
            }),
            Err(PublicRimeSliceError::InvalidTwoCharacterCoverageDepth)
        );
        assert_eq!(
            parse_public_rime_slice(
                source,
                PublicRimeSliceConfig {
                    max_entries: 0,
                    frequency_frontier_entries: 0,
                    two_character_coverage_depth: 1,
                    three_character_coverage_entries: 0,
                    four_character_coverage_entries: 0,
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
                    frequency_frontier_entries: 1,
                    two_character_coverage_depth: 1,
                    three_character_coverage_entries: 0,
                    four_character_coverage_entries: 0,
                    max_text_characters: 1,
                }
            ),
            Err(PublicRimeSliceError::MissingDocumentStart)
        ));
    }

    #[test]
    fn slice_uses_reserved_capacity_to_cover_missing_two_character_codes() {
        const SOURCE: &str = "---\n...\n\
高频\tgao pin\t100\n\
高品\tgao pin\t90\n\
完整\twan zheng\t20\n\
完正\twan zheng\t10\n\
另类\tling wai\t15\n\
四字短语\tsi zi duan yu\t80\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 5,
                frequency_frontier_entries: 2,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 4,
            },
        )
        .unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["高频", "高品", "四字短语", "完整", "另类"]
        );
        assert_eq!(imported.stats.eligible_two_character_codes, 3);
        assert_eq!(imported.stats.frequency_frontier_two_character_codes, 1);
        assert_eq!(imported.stats.frequency_frontier_selected_rows, 2);
        assert_eq!(imported.stats.two_character_coverage_candidates, 2);
        assert_eq!(imported.stats.two_character_coverage_admitted, 2);
        assert_eq!(imported.stats.two_character_coverage_dropped, 0);
        assert_eq!(imported.stats.frequency_backfill_admitted, 1);
        assert_eq!(imported.stats.imported_entries, 5);
        assert_eq!(imported.stats.minimum_selected_frequency, 15);
    }

    #[test]
    fn slice_bounds_two_character_coverage_by_the_total_entry_cap() {
        const SOURCE: &str = "---\n...\n\
高频\tgao pin\t100\n\
完整\twan zheng\t20\n\
另类\tling wai\t15\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 2,
                frequency_frontier_entries: 1,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 2,
            },
        )
        .unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["高频", "完整"]
        );
        assert_eq!(imported.stats.two_character_coverage_candidates, 2);
        assert_eq!(imported.stats.two_character_coverage_admitted, 1);
        assert_eq!(imported.stats.two_character_coverage_dropped, 1);
    }

    #[test]
    fn slice_can_reserve_deeper_distinct_words_under_one_complete_code() {
        const SOURCE: &str = "---\n...\n\
首选\tshou xuan\t100\n\
手选\tshou xuan\t80\n\
受选\tshou xuan\t60\n\
收选\tshou xuan\t40\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 3,
                frequency_frontier_entries: 1,
                two_character_coverage_depth: 3,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 2,
            },
        )
        .unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["首选", "手选", "受选"]
        );
        assert_eq!(imported.stats.eligible_two_character_codes, 1);
        assert_eq!(imported.stats.eligible_two_character_depth_entries, 3);
        assert_eq!(imported.stats.two_character_coverage_depth, 3);
        assert_eq!(imported.stats.two_character_coverage_candidates, 2);
        assert_eq!(imported.stats.two_character_coverage_admitted, 2);
    }

    #[test]
    fn slice_orders_deeper_code_tiers_by_source_weight_after_code_coverage() {
        const SOURCE: &str = "---\n...\n\
甲首\tjia shou\t100\n\
甲次\tjia shou\t90\n\
甲三\tjia shou\t60\n\
乙首\tyi shou\t80\n\
乙次\tyi shou\t1\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 4,
                frequency_frontier_entries: 1,
                two_character_coverage_depth: 3,
                three_character_coverage_entries: 0,
                four_character_coverage_entries: 0,
                max_text_characters: 2,
            },
        )
        .unwrap();

        let texts = imported
            .entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(texts, HashSet::from(["甲首", "甲次", "甲三", "乙首"]));
        assert!(!texts.contains("乙次"));
    }

    #[test]
    fn phrase_allowlist_uses_source_readings_and_bounded_source_weight_order() {
        const ALLOWLIST: &str = "wlrh\t无论如何\\t五颜六色\n\
                                  repeat\t无论如何\n\
                                  short\t三个字\n\
                                  INVALID\t固定短语\n";
        const SOURCE: &str = "---\n...\n\
                              无论如何\twú lùn rú hé\t20\n\
                              五颜六色\twǔ yán liù sè\t30\n\
                              固定短语\tgù dìng duǎn yǔ\t40\n\
                              无论如何\twú lùn rú hé\t10\n";
        let imported = parse_public_rime_phrase_allowlist(SOURCE, ALLOWLIST, 1).unwrap();

        assert_eq!(imported.entries.len(), 1);
        assert_eq!(imported.entries[0].text, "五颜六色");
        assert_eq!(imported.entries[0].pinyin, "wu yan liu se");
        assert_eq!(imported.stats.allowlist_rows, 4);
        assert_eq!(imported.stats.malformed_allowlist_rows, 1);
        assert_eq!(imported.stats.non_four_character_terms, 1);
        assert_eq!(imported.stats.duplicate_terms, 1);
        assert_eq!(imported.stats.eligible_terms, 2);
        assert_eq!(imported.stats.allowlisted_source_rows, 3);
        assert_eq!(imported.stats.matched_terms, 2);
        assert_eq!(imported.stats.dropped_by_entry_cap, 1);
        assert_eq!(imported.stats.imported_entries, 1);
    }

    #[test]
    fn short_word_consensus_uses_binary_confirmation_and_excludes_base_identities() {
        const SOURCE: &str = "---\n...\n\
手相\tshǒu xiàng\t30\n\
首项\tshǒu xiàng\t20\n\
收束\tshōu shù\t10\n\
未确认\twèi què rèn\t100\n";
        const CONFIRMATION: &str = "手相 50 n\n首项 9 n\n收束 31 v\n首项 9 n\n英文 1 x\n";
        let base = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n手相\tshou xiang\t100\n首相\tshou xiang\t90\n",
        )
        .unwrap();
        let imported =
            parse_public_short_word_consensus(SOURCE, CONFIRMATION, &base, 2, 10).unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["首项", "收束"])
        );
        assert_eq!(imported.stats.confirmation_rows, 5);
        assert_eq!(imported.stats.eligible_confirmation_surfaces, 4);
        assert_eq!(imported.stats.duplicate_confirmation_surfaces, 1);
        assert_eq!(imported.stats.confirmed_identities, 3);
        assert_eq!(imported.stats.base_identities, 1);
        assert_eq!(imported.stats.available_new_identities, 2);
        assert_eq!(imported.stats.available_new_codes, 2);
        assert_eq!(imported.stats.imported_entries, 2);
    }

    #[test]
    fn phrase_allowlist_rejects_empty_terms_and_unbounded_imports() {
        const SOURCE: &str = "---\n...\n无论如何\twú lùn rú hé\t20\n";
        assert_eq!(
            parse_public_rime_phrase_allowlist(SOURCE, "bad-row\n", 1).unwrap_err(),
            PublicRimeSliceError::EmptyPhraseAllowlist
        );
        assert_eq!(
            parse_public_rime_phrase_allowlist(
                SOURCE,
                "wlrh\t无论如何\n",
                MAX_PUBLIC_RIME_PHRASE_ALLOWLIST_ENTRIES + 1,
            )
            .unwrap_err(),
            PublicRimeSliceError::InvalidPhraseAllowlistEntryLimit
        );
    }

    #[test]
    fn targeted_public_lookup_separates_surface_code_and_absence() {
        const SOURCE: &str = "---\n...\n\
绷断\tbēng duàn\t20\n\
绷断\tbāng duàn\t30\n\
误提交\tunsupported!\t40\n\
其他\tqí tā\t10\n";
        let exact = audit_public_rime_target(SOURCE, "绷断", "bgdr").unwrap();
        assert_eq!(exact.source_rows, 4);
        assert_eq!(exact.surface_rows, 2);
        assert_eq!(exact.valid_surface_rows, 2);
        assert_eq!(exact.exact_code_rows, 1);
        assert_eq!(exact.highest_exact_frequency, Some(20));

        let missing_code = audit_public_rime_target(SOURCE, "绷断", "zzzz").unwrap();
        assert_eq!(missing_code.surface_rows, 2);
        assert_eq!(missing_code.exact_code_rows, 0);
        assert_eq!(missing_code.highest_exact_frequency, None);

        let invalid_reading = audit_public_rime_target(SOURCE, "误提交", "wutijc").unwrap();
        assert_eq!(invalid_reading.surface_rows, 1);
        assert_eq!(invalid_reading.valid_surface_rows, 0);
        assert_eq!(invalid_reading.exact_code_rows, 0);

        let absent = audit_public_rime_target(SOURCE, "掰开揉碎", "blklrbsv").unwrap();
        assert_eq!(absent.surface_rows, 0);
        assert_eq!(absent.exact_code_rows, 0);
    }

    #[test]
    fn targeted_public_lookup_requires_a_complete_han_identity_and_rime_markers() {
        const SOURCE: &str = "---\n...\n绷断\tbēng duàn\t20\n";
        for (text, code) in [("", ""), ("A词", "ac"), ("绷断", "bgd"), ("绷断", "BGDR")] {
            assert_eq!(
                audit_public_rime_target(SOURCE, text, code).unwrap_err(),
                PublicRimeSliceError::InvalidTarget
            );
        }
        assert_eq!(
            audit_public_rime_target("...\n绷断\tbēng duàn\t20\n", "绷断", "bgdr").unwrap_err(),
            PublicRimeSliceError::MissingDocumentStart
        );
        assert_eq!(
            audit_public_rime_target("---\n绷断\tbēng duàn\t20\n", "绷断", "bgdr").unwrap_err(),
            PublicRimeSliceError::MissingDataMarker
        );
    }

    #[test]
    fn slice_applies_bounded_three_and_four_character_code_coverage() {
        const SOURCE: &str = "---\n...\n\
二字\ter zi\t100\n\
三字甲\tsan zi jia\t90\n\
三字钾\tsan zi jia\t80\n\
四字短语\tsi zi duan yu\t70\n\
另三字\tling san zi\t60\n\
另四字词\tling si zi ci\t50\n";
        let imported = parse_public_rime_slice(
            SOURCE,
            PublicRimeSliceConfig {
                max_entries: 4,
                frequency_frontier_entries: 2,
                two_character_coverage_depth: 1,
                three_character_coverage_entries: 1,
                four_character_coverage_entries: 1,
                max_text_characters: 4,
            },
        )
        .unwrap();

        assert_eq!(
            imported
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["二字", "三字甲", "四字短语", "另三字"]
        );
        assert_eq!(imported.stats.eligible_three_character_codes, 2);
        assert_eq!(imported.stats.frequency_frontier_three_character_codes, 1);
        assert_eq!(imported.stats.three_character_coverage_candidates, 1);
        assert_eq!(imported.stats.three_character_coverage_admitted, 1);
        assert_eq!(imported.stats.three_character_coverage_dropped, 0);
        assert_eq!(imported.stats.imported_three_character_codes, 2);
        assert_eq!(imported.stats.eligible_four_character_codes, 2);
        assert_eq!(imported.stats.frequency_frontier_four_character_codes, 0);
        assert_eq!(imported.stats.four_character_coverage_candidates, 2);
        assert_eq!(imported.stats.four_character_coverage_admitted, 1);
        assert_eq!(imported.stats.four_character_coverage_dropped, 1);
        assert_eq!(imported.stats.imported_four_character_codes, 1);
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
                consensus_top_reorder_eligible_codes: 1,
                consensus_top_original_rank_two_codes: 1,
                consensus_top_original_rank_three_to_six_codes: 0,
                consensus_top_original_rank_seven_or_later_codes: 0,
            }
        );
    }

    #[test]
    fn public_comparison_buckets_consensus_target_by_real_exact_rank() {
        let base = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
甲\tjia\t100\n\
钾\tjia\t90\n\
吗\tma\t100\n\
妈\tma\t90\n\
马\tma\t80\n\
是\tshi\t100\n\
时\tshi\t90\n\
事\tshi\t80\n\
市\tshi\t70\n\
十\tshi\t60\n\
使\tshi\t50\n\
世\tshi\t40\n\
八\tba\t100\n\
吧\tba\t100\n",
        )
        .unwrap();
        let challenger = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
钾\tjia\t100\n\
马\tma\t100\n\
世\tshi\t100\n\
吧\tba\t100\n",
        )
        .unwrap();

        let comparison = compare_public_lexicons(&base, &challenger);
        assert_eq!(comparison.changed_top_text_codes, 4);
        assert_eq!(comparison.consensus_top_reorder_eligible_codes, 4);
        // The equal-frequency `ba` pair verifies the decoder's stable text
        // tie-break rather than retaining source-row order.
        assert_eq!(comparison.consensus_top_original_rank_two_codes, 2);
        assert_eq!(comparison.consensus_top_original_rank_three_to_six_codes, 1);
        assert_eq!(
            comparison.consensus_top_original_rank_seven_or_later_codes,
            1
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
