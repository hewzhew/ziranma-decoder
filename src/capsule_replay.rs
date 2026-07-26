//! Redacted offline replay metrics over explicitly loaded private capsules.
//!
//! This module receives an already parsed capsule and never performs I/O. It
//! compares bounded commit records with a caller-supplied decoder and returns
//! aggregate counts only.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::{
    BIGRAM_INTERPOLATION_WEIGHT, BigramLanguageModel, CandidateSource,
    CharacterBigramLanguageModel, Correction, Decoder, EventCapsuleV1, KeySequence,
    KeySequenceError, RawKey, SentenceCandidate, TrackerOutput, encode_pinyin_phrase,
};

pub const MAX_REPLAY_CODE_KEYS: usize = 64;
const REPLAY_TOP_K: usize = 10;
const PUBLIC_CONTEXT_POOL_DEPTH: usize = 50;
const PERSONAL_CACHE_POOL_DEPTH: usize = 50;
const PERSONAL_CACHE_MAX_PROMOTION: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersonalCacheKind {
    WordFrequency,
    OrderedWordPairs,
}

impl PersonalCacheKind {
    fn terminal_label(self) -> &'static str {
        match self {
            Self::WordFrequency => "word_frequency",
            Self::OrderedWordPairs => "ordered_word_pairs",
        }
    }

    fn compact_scope(self) -> &'static str {
        match self {
            Self::WordFrequency => "window_personal_word_cache",
            Self::OrderedWordPairs => "window_personal_pair_cache",
        }
    }

    fn compact_context_label(self) -> &'static str {
        match self {
            Self::WordFrequency => "personal_word_cache",
            Self::OrderedWordPairs => "personal_word_pair_cache",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicContextKind {
    WordBigram,
    CharacterBigram,
}

impl PublicContextKind {
    fn terminal_label(self) -> &'static str {
        match self {
            Self::WordBigram => "word_bigram",
            Self::CharacterBigram => "character_bigram",
        }
    }

    fn compact_scope(self) -> &'static str {
        match self {
            Self::WordBigram => "window_public_word_bigram",
            Self::CharacterBigram => "window_public_character_bigram",
        }
    }

    fn compact_context_label(self) -> &'static str {
        match self {
            Self::WordBigram => "public_word_bigram",
            Self::CharacterBigram => "public_character_bigram",
        }
    }
}

#[derive(Clone, Copy)]
enum FrozenPublicContext<'a> {
    Word {
        language_model: &'a BigramLanguageModel,
        log_frequency_total: f64,
    },
    Character {
        language_model: &'a CharacterBigramLanguageModel,
    },
}

impl FrozenPublicContext<'_> {
    fn kind(self) -> PublicContextKind {
        match self {
            Self::Word { .. } => PublicContextKind::WordBigram,
            Self::Character { .. } => PublicContextKind::CharacterBigram,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapsuleReplayConfigError {
    ZeroWindowGap,
}

impl fmt::Display for CapsuleReplayConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroWindowGap => write!(formatter, "continuous window gap must be positive"),
        }
    }
}

impl Error for CapsuleReplayConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonalCacheReplayError {
    MissingWindowGap,
    InvalidKeySequence,
}

impl fmt::Display for PersonalCacheReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingWindowGap => {
                write!(
                    formatter,
                    "personal cache replay requires a continuous window gap"
                )
            }
            Self::InvalidKeySequence => {
                write!(
                    formatter,
                    "personal cache replay encountered an invalid key sequence"
                )
            }
        }
    }
}

impl Error for PersonalCacheReplayError {}

impl From<KeySequenceError> for PersonalCacheReplayError {
    fn from(_error: KeySequenceError) -> Self {
        Self::InvalidKeySequence
    }
}

/// Memory-only word counts used by causal personal-cache replay.
///
/// The contained words are intentionally private and have no accessor,
/// serializer, or content-bearing `Debug` implementation.
#[derive(Default)]
pub struct PersonalCacheReplayState {
    word_counts: HashMap<String, u64>,
    learned_word_tokens: u64,
    active_document_spans: Vec<PersonalLearnedSpan>,
    pair_counts: HashMap<(String, String), u64>,
    learned_word_pairs: u64,
    active_pair_spans: Vec<PersonalLearnedPairSpan>,
}

struct PersonalLearnedSpan {
    start: usize,
    end: usize,
    words: Vec<String>,
}

struct PersonalLearnedPairSpan {
    start: usize,
    end: usize,
    pairs: Vec<(String, String)>,
}

#[derive(Clone, Copy, Default)]
struct PersonalCacheEditOutcome {
    invalidated_commits: u64,
    invalidated_word_tokens: u64,
    invalidated_pair_sequences: u64,
    invalidated_word_pairs: u64,
    ambiguous_position: bool,
}

impl PersonalCacheReplayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn learned_word_types(&self) -> usize {
        self.word_counts.len()
    }

    pub fn learned_word_tokens(&self) -> u64 {
        self.learned_word_tokens
    }

    pub fn learned_word_pair_types(&self) -> usize {
        self.pair_counts.len()
    }

    pub fn learned_word_pairs(&self) -> u64 {
        self.learned_word_pairs
    }

    fn start_document(&mut self) {
        self.active_document_spans.clear();
        self.active_pair_spans.clear();
    }

    fn learn_commit(&mut self, start: usize, inserted_chars: usize, words: &[String]) {
        for word in words {
            let count = self.word_counts.entry(word.clone()).or_insert(0);
            *count = count.saturating_add(1);
            self.learned_word_tokens = self.learned_word_tokens.saturating_add(1);
        }
        self.active_document_spans.push(PersonalLearnedSpan {
            start,
            end: start.saturating_add(inserted_chars),
            words: words.to_vec(),
        });
    }

    fn count(&self, word: &str) -> u64 {
        self.word_counts.get(word).copied().unwrap_or(0)
    }

    fn pair_count(&self, previous: &str, current: &str) -> u64 {
        self.pair_counts
            .get(&(previous.to_owned(), current.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    fn learn_pair_sequence(&mut self, start: usize, end: usize, words: &[String]) -> usize {
        let pairs = words
            .windows(2)
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect::<Vec<_>>();
        for pair in &pairs {
            let count = self.pair_counts.entry(pair.clone()).or_insert(0);
            *count = count.saturating_add(1);
            self.learned_word_pairs = self.learned_word_pairs.saturating_add(1);
        }
        if !pairs.is_empty() {
            self.active_pair_spans
                .push(PersonalLearnedPairSpan { start, end, pairs });
        }
        words.len().saturating_sub(1)
    }

    fn apply_document_delta(
        &mut self,
        start: usize,
        deleted_chars: usize,
        inserted_chars: usize,
        position_evidence: crate::DeltaPositionEvidence,
    ) -> PersonalCacheEditOutcome {
        if position_evidence == crate::DeltaPositionEvidence::Ambiguous {
            return PersonalCacheEditOutcome {
                ambiguous_position: true,
                ..PersonalCacheEditOutcome::default()
            };
        }
        let edit_end = start.saturating_add(deleted_chars);
        let mut outcome = PersonalCacheEditOutcome::default();
        let mut retained = Vec::with_capacity(self.active_document_spans.len());
        for mut span in std::mem::take(&mut self.active_document_spans) {
            let overlaps = if deleted_chars > 0 {
                span.start < edit_end && span.end > start
            } else {
                start > span.start && start < span.end
            };
            if overlaps {
                outcome.invalidated_commits = outcome.invalidated_commits.saturating_add(1);
                outcome.invalidated_word_tokens = outcome
                    .invalidated_word_tokens
                    .saturating_add(u64::try_from(span.words.len()).unwrap_or(u64::MAX));
                self.unlearn_words(&span.words);
                continue;
            }
            if span.start >= edit_end {
                if inserted_chars >= deleted_chars {
                    let shift = inserted_chars - deleted_chars;
                    span.start = span.start.saturating_add(shift);
                    span.end = span.end.saturating_add(shift);
                } else {
                    let shift = deleted_chars - inserted_chars;
                    span.start = span.start.saturating_sub(shift);
                    span.end = span.end.saturating_sub(shift);
                }
            }
            retained.push(span);
        }
        self.active_document_spans = retained;

        let mut retained_pairs = Vec::with_capacity(self.active_pair_spans.len());
        for mut span in std::mem::take(&mut self.active_pair_spans) {
            let overlaps = if deleted_chars > 0 {
                span.start < edit_end && span.end > start
            } else {
                start > span.start && start < span.end
            };
            if overlaps {
                outcome.invalidated_pair_sequences =
                    outcome.invalidated_pair_sequences.saturating_add(1);
                outcome.invalidated_word_pairs = outcome
                    .invalidated_word_pairs
                    .saturating_add(u64::try_from(span.pairs.len()).unwrap_or(u64::MAX));
                self.unlearn_pairs(&span.pairs);
                continue;
            }
            if span.start >= edit_end {
                if inserted_chars >= deleted_chars {
                    let shift = inserted_chars - deleted_chars;
                    span.start = span.start.saturating_add(shift);
                    span.end = span.end.saturating_add(shift);
                } else {
                    let shift = deleted_chars - inserted_chars;
                    span.start = span.start.saturating_sub(shift);
                    span.end = span.end.saturating_sub(shift);
                }
            }
            retained_pairs.push(span);
        }
        self.active_pair_spans = retained_pairs;
        outcome
    }

    fn unlearn_words(&mut self, words: &[String]) {
        for word in words {
            let remove = self.word_counts.get_mut(word).is_some_and(|count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
            if remove {
                self.word_counts.remove(word);
            }
            self.learned_word_tokens = self.learned_word_tokens.saturating_sub(1);
        }
    }

    fn unlearn_pairs(&mut self, pairs: &[(String, String)]) {
        for pair in pairs {
            let remove = self.pair_counts.get_mut(pair).is_some_and(|count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
            if remove {
                self.pair_counts.remove(pair);
            }
            self.learned_word_pairs = self.learned_word_pairs.saturating_sub(1);
        }
    }
}

impl fmt::Debug for PersonalCacheReplayState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonalCacheReplayState")
            .field("debug_contains_text", &false)
            .field("learned_word_types", &self.learned_word_types())
            .field("learned_word_tokens", &self.learned_word_tokens)
            .field("active_document_spans", &self.active_document_spans.len())
            .field("learned_word_pair_types", &self.learned_word_pair_types())
            .field("learned_word_pairs", &self.learned_word_pairs)
            .field("active_pair_spans", &self.active_pair_spans.len())
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayStrategyStats {
    pub attempts: u64,
    pub input_keys: u64,
    pub hits_at_1: u64,
    pub hits_at_5: u64,
    pub hits_at_10: u64,
    pub rank_histogram_at_10: [u64; REPLAY_TOP_K],
}

impl ReplayStrategyStats {
    fn observe(
        &mut self,
        code: &str,
        target: &str,
        candidates: &[SentenceCandidate],
    ) -> Option<usize> {
        self.attempts = self.attempts.saturating_add(1);
        self.input_keys = self
            .input_keys
            .saturating_add(u64::try_from(code.len()).unwrap_or(u64::MAX));
        let rank = candidates
            .iter()
            .position(|candidate| candidate.text == target);
        if let Some(rank) = rank {
            if rank == 0 {
                self.hits_at_1 = self.hits_at_1.saturating_add(1);
            }
            if rank < 5 {
                self.hits_at_5 = self.hits_at_5.saturating_add(1);
            }
            if rank < 10 {
                self.hits_at_10 = self.hits_at_10.saturating_add(1);
                self.rank_histogram_at_10[rank] = self.rank_histogram_at_10[rank].saturating_add(1);
            }
        }
        rank.map(|rank| rank + 1)
    }

    pub fn projected_actions_with_one_selection(&self) -> u64 {
        self.input_keys.saturating_add(self.attempts)
    }

    fn terminal_fields(&self, name: &str) -> String {
        format!(
            "{name}_attempts={} {name}_input_keys={} {name}_hits_at_1={} \
             {name}_hits_at_5={} {name}_hits_at_10={} \
             {name}_rank_histogram_at_10={} \
             {name}_projected_actions_one_selection={}",
            self.attempts,
            self.input_keys,
            self.hits_at_1,
            self.hits_at_5,
            self.hits_at_10,
            display_rank_histogram(&self.rank_histogram_at_10),
            self.projected_actions_with_one_selection()
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairedReplayStrategyStats {
    pub comparisons: u64,
    pub shortened_codes: u64,
    pub unchanged_codes: u64,
    pub lengthened_codes: u64,
    pub baseline_input_keys: u64,
    pub strategy_input_keys: u64,
    pub input_keys_saved: u64,
    pub input_keys_added: u64,
    pub baseline_visible_at_10: u64,
    pub strategy_visible_at_10: u64,
    pub rank_improved: u64,
    pub rank_same: u64,
    pub rank_worsened: u64,
    pub both_outside_top_10: u64,
    pub dropped_from_top_10: u64,
    pub recovered_into_top_10: u64,
}

impl PairedReplayStrategyStats {
    fn observe(
        &mut self,
        baseline_code: &str,
        strategy_code: &str,
        baseline_rank: Option<usize>,
        strategy_rank: Option<usize>,
    ) {
        self.comparisons = self.comparisons.saturating_add(1);
        let baseline_keys = saturating_len(baseline_code.len());
        let strategy_keys = saturating_len(strategy_code.len());
        self.baseline_input_keys = self.baseline_input_keys.saturating_add(baseline_keys);
        self.strategy_input_keys = self.strategy_input_keys.saturating_add(strategy_keys);
        match strategy_keys.cmp(&baseline_keys) {
            std::cmp::Ordering::Less => {
                self.shortened_codes = self.shortened_codes.saturating_add(1);
                self.input_keys_saved = self
                    .input_keys_saved
                    .saturating_add(baseline_keys - strategy_keys);
            }
            std::cmp::Ordering::Equal => {
                self.unchanged_codes = self.unchanged_codes.saturating_add(1);
            }
            std::cmp::Ordering::Greater => {
                self.lengthened_codes = self.lengthened_codes.saturating_add(1);
                self.input_keys_added = self
                    .input_keys_added
                    .saturating_add(strategy_keys - baseline_keys);
            }
        }

        if baseline_rank.is_some() {
            self.baseline_visible_at_10 = self.baseline_visible_at_10.saturating_add(1);
        }
        if strategy_rank.is_some() {
            self.strategy_visible_at_10 = self.strategy_visible_at_10.saturating_add(1);
        }
        match (baseline_rank, strategy_rank) {
            (Some(baseline), Some(strategy)) if strategy < baseline => {
                self.rank_improved = self.rank_improved.saturating_add(1);
            }
            (Some(baseline), Some(strategy)) if strategy == baseline => {
                self.rank_same = self.rank_same.saturating_add(1);
            }
            (Some(_), Some(_)) => {
                self.rank_worsened = self.rank_worsened.saturating_add(1);
            }
            (Some(_), None) => {
                self.rank_worsened = self.rank_worsened.saturating_add(1);
                self.dropped_from_top_10 = self.dropped_from_top_10.saturating_add(1);
            }
            (None, Some(_)) => {
                self.rank_improved = self.rank_improved.saturating_add(1);
                self.recovered_into_top_10 = self.recovered_into_top_10.saturating_add(1);
            }
            (None, None) => {
                self.both_outside_top_10 = self.both_outside_top_10.saturating_add(1);
            }
        }
    }

    fn terminal_fields(&self, name: &str) -> String {
        format!(
            "{name}_comparisons={} {name}_shortened_codes={} {name}_unchanged_codes={} \
             {name}_lengthened_codes={} {name}_baseline_input_keys={} \
             {name}_strategy_input_keys={} {name}_input_keys_saved={} \
             {name}_input_keys_added={} {name}_baseline_visible_at_10={} \
             {name}_strategy_visible_at_10={} {name}_rank_improved={} \
             {name}_rank_same={} {name}_rank_worsened={} \
             {name}_both_outside_top_10={} {name}_dropped_from_top_10={} \
             {name}_recovered_into_top_10={}",
            self.comparisons,
            self.shortened_codes,
            self.unchanged_codes,
            self.lengthened_codes,
            self.baseline_input_keys,
            self.strategy_input_keys,
            self.input_keys_saved,
            self.input_keys_added,
            self.baseline_visible_at_10,
            self.strategy_visible_at_10,
            self.rank_improved,
            self.rank_same,
            self.rank_worsened,
            self.both_outside_top_10,
            self.dropped_from_top_10,
            self.recovered_into_top_10
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextReplayComparisonStats {
    pub windows: u64,
    pub baselines_both_visible_at_10: u64,
    pub unigram_baseline_only_visible_at_10: u64,
    pub context_baseline_only_visible_at_10: u64,
    pub neither_baseline_visible_at_10: u64,
    pub relative_degradation_reduced: u64,
    pub relative_degradation_same: u64,
    pub relative_degradation_increased: u64,
    pub unigram_drops_from_top_10: u64,
    pub context_drops_from_top_10: u64,
    pub drops_rescued_by_context: u64,
    pub new_drops_with_context: u64,
}

impl ContextReplayComparisonStats {
    fn observe(
        &mut self,
        unigram_baseline_rank: Option<usize>,
        unigram_strategy_rank: Option<usize>,
        context_baseline_rank: Option<usize>,
        context_strategy_rank: Option<usize>,
    ) {
        self.windows = self.windows.saturating_add(1);
        match (unigram_baseline_rank, context_baseline_rank) {
            (Some(unigram_baseline), Some(context_baseline)) => {
                self.baselines_both_visible_at_10 =
                    self.baselines_both_visible_at_10.saturating_add(1);
                let unigram_degradation =
                    rank_for_comparison(unigram_strategy_rank) - unigram_baseline as i16;
                let context_degradation =
                    rank_for_comparison(context_strategy_rank) - context_baseline as i16;
                match context_degradation.cmp(&unigram_degradation) {
                    std::cmp::Ordering::Less => {
                        self.relative_degradation_reduced =
                            self.relative_degradation_reduced.saturating_add(1);
                    }
                    std::cmp::Ordering::Equal => {
                        self.relative_degradation_same =
                            self.relative_degradation_same.saturating_add(1);
                    }
                    std::cmp::Ordering::Greater => {
                        self.relative_degradation_increased =
                            self.relative_degradation_increased.saturating_add(1);
                    }
                }

                let unigram_drop = unigram_strategy_rank.is_none();
                let context_drop = context_strategy_rank.is_none();
                if unigram_drop {
                    self.unigram_drops_from_top_10 =
                        self.unigram_drops_from_top_10.saturating_add(1);
                }
                if context_drop {
                    self.context_drops_from_top_10 =
                        self.context_drops_from_top_10.saturating_add(1);
                }
                if unigram_drop && !context_drop {
                    self.drops_rescued_by_context = self.drops_rescued_by_context.saturating_add(1);
                }
                if !unigram_drop && context_drop {
                    self.new_drops_with_context = self.new_drops_with_context.saturating_add(1);
                }
            }
            (Some(_), None) => {
                self.unigram_baseline_only_visible_at_10 =
                    self.unigram_baseline_only_visible_at_10.saturating_add(1);
            }
            (None, Some(_)) => {
                self.context_baseline_only_visible_at_10 =
                    self.context_baseline_only_visible_at_10.saturating_add(1);
            }
            (None, None) => {
                self.neither_baseline_visible_at_10 =
                    self.neither_baseline_visible_at_10.saturating_add(1);
            }
        }
    }

    fn terminal_fields(&self, name: &str) -> String {
        format!(
            "{name}_windows={} {name}_baselines_both_visible_at_10={} \
             {name}_unigram_baseline_only_visible_at_10={} \
             {name}_context_baseline_only_visible_at_10={} \
             {name}_neither_baseline_visible_at_10={} \
             {name}_relative_degradation_reduced={} \
             {name}_relative_degradation_same={} \
             {name}_relative_degradation_increased={} \
             {name}_unigram_drops_from_top_10={} \
             {name}_context_drops_from_top_10={} \
             {name}_drops_rescued_by_context={} {name}_new_drops_with_context={}",
            self.windows,
            self.baselines_both_visible_at_10,
            self.unigram_baseline_only_visible_at_10,
            self.context_baseline_only_visible_at_10,
            self.neither_baseline_visible_at_10,
            self.relative_degradation_reduced,
            self.relative_degradation_same,
            self.relative_degradation_increased,
            self.unigram_drops_from_top_10,
            self.context_drops_from_top_10,
            self.drops_rescued_by_context,
            self.new_drops_with_context
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapsuleReplayReport {
    window_gap_limit_ms: Option<u64>,
    public_context_kind: Option<PublicContextKind>,
    personal_cache_kind: Option<PersonalCacheKind>,
    pub capsules: u64,
    pub events: u64,
    pub commits: u64,
    pub revisions: u64,
    pub recorded_logical_key_actions: u64,
    pub incomplete_key_commits: u64,
    pub key_interpretation_failures: u64,
    pub commits_without_letter_code: u64,
    pub commits_over_key_limit: u64,
    pub composition_encodable_commits: u64,
    pub observed_matches_canonical: u64,
    pub word_boundaries_available_commits: u64,
    pub word_boundaries_unavailable_commits: u64,
    pub raw_existing: ReplayStrategyStats,
    pub canonical_full: ReplayStrategyStats,
    pub tail_one_short: ReplayStrategyStats,
    pub head_anchored: ReplayStrategyStats,
    pub all_short: ReplayStrategyStats,
    pub word_tail_one_short: ReplayStrategyStats,
    pub word_tail_keep_singletons: ReplayStrategyStats,
    pub word_head_anchored: ReplayStrategyStats,
    pub window_eligible_commits: u64,
    pub window_ineligible_commits: u64,
    pub continuous_windows: u64,
    pub continuous_window_commits: u64,
    pub continuous_window_recorded_logical_key_actions: u64,
    pub continuous_windows_over_key_limit: u64,
    pub window_raw_joined: ReplayStrategyStats,
    pub window_canonical_full: ReplayStrategyStats,
    pub window_word_tail_one_short: ReplayStrategyStats,
    pub window_word_tail_keep_singletons: ReplayStrategyStats,
    pub window_word_head_anchored: ReplayStrategyStats,
    pub window_word_tail_one_short_vs_full: PairedReplayStrategyStats,
    pub window_word_tail_keep_singletons_vs_full: PairedReplayStrategyStats,
    pub window_word_head_anchored_vs_full: PairedReplayStrategyStats,
    pub public_context_windows: u64,
    pub public_context_window_canonical_full: ReplayStrategyStats,
    pub public_context_window_word_tail_one_short: ReplayStrategyStats,
    pub public_context_window_word_tail_keep_singletons: ReplayStrategyStats,
    pub public_context_window_word_head_anchored: ReplayStrategyStats,
    pub public_context_window_word_tail_one_short_vs_full: PairedReplayStrategyStats,
    pub public_context_window_word_tail_keep_singletons_vs_full: PairedReplayStrategyStats,
    pub public_context_window_word_head_anchored_vs_full: PairedReplayStrategyStats,
    pub word_tail_one_short_context_effect: ContextReplayComparisonStats,
    pub word_tail_keep_singletons_context_effect: ContextReplayComparisonStats,
    pub word_head_anchored_context_effect: ContextReplayComparisonStats,
    pub personal_cache_windows: u64,
    pub personal_cache_history_capsules: u64,
    pub personal_cache_history_events: u64,
    pub personal_cache_history_learning_commits: u64,
    pub personal_cache_history_word_tokens: u64,
    pub personal_cache_history_word_types: u64,
    pub personal_cache_history_word_pairs: u64,
    pub personal_cache_history_word_pair_types: u64,
    pub personal_cache_learning_commits: u64,
    pub personal_cache_learning_word_tokens: u64,
    pub personal_cache_retained_word_tokens: u64,
    pub personal_cache_learned_word_types: u64,
    pub personal_cache_reversed_commits: u64,
    pub personal_cache_reversed_word_tokens: u64,
    pub personal_cache_learning_pair_sequences: u64,
    pub personal_cache_learning_word_pairs: u64,
    pub personal_cache_retained_word_pairs: u64,
    pub personal_cache_learned_word_pair_types: u64,
    pub personal_cache_reversed_pair_sequences: u64,
    pub personal_cache_reversed_word_pairs: u64,
    pub personal_cache_revision_events_with_reversal: u64,
    pub personal_cache_revisions_not_reversed: u64,
    pub personal_cache_ambiguous_edits_not_applied: u64,
    pub personal_cache_window_canonical_full: ReplayStrategyStats,
    pub personal_cache_window_word_tail_one_short: ReplayStrategyStats,
    pub personal_cache_window_word_tail_keep_singletons: ReplayStrategyStats,
    pub personal_cache_window_word_head_anchored: ReplayStrategyStats,
    pub personal_cache_window_word_tail_one_short_vs_full: PairedReplayStrategyStats,
    pub personal_cache_window_word_tail_keep_singletons_vs_full: PairedReplayStrategyStats,
    pub personal_cache_window_word_head_anchored_vs_full: PairedReplayStrategyStats,
    pub word_tail_one_short_personal_cache_effect: ContextReplayComparisonStats,
    pub word_tail_keep_singletons_personal_cache_effect: ContextReplayComparisonStats,
    pub word_head_anchored_personal_cache_effect: ContextReplayComparisonStats,
}

impl CapsuleReplayReport {
    pub fn with_window_gap_limit(
        window_gap_limit_ms: Option<u64>,
    ) -> Result<Self, CapsuleReplayConfigError> {
        if window_gap_limit_ms == Some(0) {
            return Err(CapsuleReplayConfigError::ZeroWindowGap);
        }
        Ok(Self {
            window_gap_limit_ms,
            ..Self::default()
        })
    }

    pub fn observe_capsule(
        &mut self,
        decoder: &Decoder,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        self.observe_capsule_internal(decoder, None, capsule)
    }

    pub fn observe_capsule_with_public_context(
        &mut self,
        decoder: &Decoder,
        public_language_model: &BigramLanguageModel,
        log_frequency_total: f64,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        self.observe_capsule_internal(
            decoder,
            Some(FrozenPublicContext::Word {
                language_model: public_language_model,
                log_frequency_total,
            }),
            capsule,
        )
    }

    pub fn observe_capsule_with_public_character_context(
        &mut self,
        decoder: &Decoder,
        public_language_model: &CharacterBigramLanguageModel,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        self.observe_capsule_internal(
            decoder,
            Some(FrozenPublicContext::Character {
                language_model: public_language_model,
            }),
            capsule,
        )
    }

    pub fn observe_capsule_with_personal_cache(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        self.observe_capsule_with_personal_cache_kind(
            decoder,
            state,
            PersonalCacheKind::WordFrequency,
            capsule,
        )
    }

    pub fn observe_capsule_with_personal_pair_cache(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        self.observe_capsule_with_personal_cache_kind(
            decoder,
            state,
            PersonalCacheKind::OrderedWordPairs,
            capsule,
        )
    }

    pub fn record_personal_cache_history(
        &mut self,
        history_report: &CapsuleReplayReport,
        state: &PersonalCacheReplayState,
    ) {
        assert_eq!(
            self.capsules, 0,
            "personal history must be recorded before evaluation capsules"
        );
        self.personal_cache_history_capsules = history_report.capsules;
        self.personal_cache_history_events = history_report.events;
        self.personal_cache_history_learning_commits =
            history_report.personal_cache_learning_commits;
        self.personal_cache_history_word_tokens = state.learned_word_tokens();
        self.personal_cache_history_word_types =
            u64::try_from(state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_history_word_pairs = state.learned_word_pairs();
        self.personal_cache_history_word_pair_types =
            u64::try_from(state.learned_word_pair_types()).unwrap_or(u64::MAX);
    }

    fn observe_capsule_with_personal_cache_kind(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        kind: PersonalCacheKind,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        if let Some(previous) = self.personal_cache_kind {
            assert_eq!(
                previous, kind,
                "a replay report cannot mix personal cache models"
            );
        } else {
            self.personal_cache_kind = Some(kind);
        }
        self.observe_capsule_internal(decoder, None, capsule)?;
        self.observe_personal_cache_windows(decoder, state, kind, capsule, max_gap_ms)?;
        self.personal_cache_learned_word_types =
            u64::try_from(state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = state.learned_word_pairs();
        Ok(())
    }

    fn observe_capsule_internal(
        &mut self,
        decoder: &Decoder,
        public_context: Option<FrozenPublicContext<'_>>,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        if let Some(context) = public_context {
            let kind = context.kind();
            if let Some(previous) = self.public_context_kind {
                assert_eq!(
                    previous, kind,
                    "a replay report cannot mix public context models"
                );
            } else {
                self.public_context_kind = Some(kind);
            }
        }
        self.capsules = self.capsules.saturating_add(1);
        self.events = self
            .events
            .saturating_add(u64::try_from(capsule.events().len()).unwrap_or(u64::MAX));
        let mut prepared_windows = HashMap::<usize, WindowCommit>::new();
        for (event_index, event) in capsule.events().iter().enumerate() {
            match &event.output {
                TrackerOutput::Revision(record) => {
                    self.revisions = self.revisions.saturating_add(1);
                    self.recorded_logical_key_actions = self
                        .recorded_logical_key_actions
                        .saturating_add(saturating_len(record.keys.len()));
                }
                TrackerOutput::Commit(record) => {
                    self.commits = self.commits.saturating_add(1);
                    self.recorded_logical_key_actions = self
                        .recorded_logical_key_actions
                        .saturating_add(saturating_len(record.keys.len()));
                    if !record.keys_complete {
                        self.incomplete_key_commits = self.incomplete_key_commits.saturating_add(1);
                        continue;
                    }
                    let observed = match effective_letter_code(&record.keys) {
                        Ok(Some(observed)) => observed,
                        Ok(None) => {
                            self.commits_without_letter_code =
                                self.commits_without_letter_code.saturating_add(1);
                            continue;
                        }
                        Err(_) => {
                            self.key_interpretation_failures =
                                self.key_interpretation_failures.saturating_add(1);
                            continue;
                        }
                    };
                    if observed.len() > MAX_REPLAY_CODE_KEYS {
                        self.commits_over_key_limit = self.commits_over_key_limit.saturating_add(1);
                        continue;
                    }

                    let target = &record.change.inserted;
                    let candidates = decoder.decode_sentence(&observed, REPLAY_TOP_K)?;
                    let _ = self.raw_existing.observe(&observed, target, &candidates);
                    let mut commit_decode_cache = HashMap::<String, Vec<SentenceCandidate>>::new();
                    commit_decode_cache.insert(observed.clone(), candidates);

                    let normalized_pinyin = record.composition.replace('\'', " ");
                    let Ok(encoded) = encode_pinyin_phrase(&normalized_pinyin) else {
                        continue;
                    };
                    self.composition_encodable_commits =
                        self.composition_encodable_commits.saturating_add(1);
                    if observed == encoded.full_code.as_str() {
                        self.observed_matches_canonical =
                            self.observed_matches_canonical.saturating_add(1);
                    }

                    let canonical_candidates = decode_sentence_memoized(
                        decoder,
                        &mut commit_decode_cache,
                        encoded.full_code.as_str(),
                    )?;
                    let _ = self.canonical_full.observe(
                        encoded.full_code.as_str(),
                        target,
                        &canonical_candidates,
                    );
                    if let Some(code) = tail_one_short(&encoded.syllable_codes) {
                        observe_strategy_memoized(
                            decoder,
                            &mut commit_decode_cache,
                            &code,
                            target,
                            &mut self.tail_one_short,
                        )?;
                    }
                    if let Some(code) = head_anchored(&encoded.syllable_codes) {
                        observe_strategy_memoized(
                            decoder,
                            &mut commit_decode_cache,
                            &code,
                            target,
                            &mut self.head_anchored,
                        )?;
                    }
                    if let Some(code) = all_short(&encoded.syllable_codes) {
                        observe_strategy_memoized(
                            decoder,
                            &mut commit_decode_cache,
                            &code,
                            target,
                            &mut self.all_short,
                        )?;
                    }
                    if let Some(segmentation) = exact_target_segmentation(
                        &canonical_candidates,
                        target,
                        encoded.syllable_codes.len(),
                    ) {
                        self.word_boundaries_available_commits =
                            self.word_boundaries_available_commits.saturating_add(1);
                        if let Some(code) =
                            word_tail_one_short(&encoded.syllable_codes, &segmentation.word_lengths)
                        {
                            observe_strategy_memoized(
                                decoder,
                                &mut commit_decode_cache,
                                &code,
                                target,
                                &mut self.word_tail_one_short,
                            )?;
                        }
                        if let Some(code) = word_tail_keep_singletons(
                            &encoded.syllable_codes,
                            &segmentation.word_lengths,
                        ) {
                            observe_strategy_memoized(
                                decoder,
                                &mut commit_decode_cache,
                                &code,
                                target,
                                &mut self.word_tail_keep_singletons,
                            )?;
                        }
                        if let Some(code) =
                            word_head_anchored(&encoded.syllable_codes, &segmentation.word_lengths)
                        {
                            observe_strategy_memoized(
                                decoder,
                                &mut commit_decode_cache,
                                &code,
                                target,
                                &mut self.word_head_anchored,
                            )?;
                        }
                        if self.window_gap_limit_ms.is_some()
                            && window_document_delta_is_eligible(record)
                        {
                            prepared_windows.insert(
                                event_index,
                                WindowCommit {
                                    elapsed_ms: event.elapsed_ms,
                                    document_start: record.document_change.start,
                                    document_inserted_chars: record
                                        .document_change
                                        .inserted
                                        .chars()
                                        .count(),
                                    observed,
                                    canonical_full: encoded.full_code.as_str().to_owned(),
                                    syllable_codes: encoded.syllable_codes,
                                    word_lengths: segmentation.word_lengths,
                                    words: segmentation.words,
                                    target: target.clone(),
                                    recorded_logical_key_actions: saturating_len(record.keys.len()),
                                },
                            );
                        }
                    } else {
                        self.word_boundaries_unavailable_commits =
                            self.word_boundaries_unavailable_commits.saturating_add(1);
                    }
                }
            }
        }
        if let Some(max_gap_ms) = self.window_gap_limit_ms {
            self.observe_continuous_windows(
                decoder,
                public_context,
                capsule,
                max_gap_ms,
                &mut prepared_windows,
            )?;
        }
        Ok(())
    }

    fn observe_continuous_windows(
        &mut self,
        decoder: &Decoder,
        public_context: Option<FrozenPublicContext<'_>>,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
        prepared_windows: &mut HashMap<usize, WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        let mut run = Vec::<WindowCommit>::new();
        for (event_index, event) in capsule.events().iter().enumerate() {
            let TrackerOutput::Commit(_) = &event.output else {
                self.finish_window(decoder, public_context, &mut run)?;
                continue;
            };
            let Some(commit) = prepared_windows.remove(&event_index) else {
                self.window_ineligible_commits = self.window_ineligible_commits.saturating_add(1);
                self.finish_window(decoder, public_context, &mut run)?;
                continue;
            };
            self.window_eligible_commits = self.window_eligible_commits.saturating_add(1);
            let joins_previous = run.last().is_some_and(|previous| {
                commit.elapsed_ms.saturating_sub(previous.elapsed_ms) <= max_gap_ms
                    && commit.document_start == previous.document_end()
            });
            if !run.is_empty() && !joins_previous {
                self.finish_window(decoder, public_context, &mut run)?;
            }
            run.push(commit);
        }
        self.finish_window(decoder, public_context, &mut run)
    }

    fn observe_personal_cache_windows(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        kind: PersonalCacheKind,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
    ) -> Result<(), KeySequenceError> {
        state.start_document();
        let mut run = Vec::<WindowCommit>::new();
        for event in capsule.events() {
            let TrackerOutput::Commit(record) = &event.output else {
                self.finish_personal_cache_window(decoder, state, kind, &mut run)?;
                let TrackerOutput::Revision(revision) = &event.output else {
                    unreachable!("tracker output has only commit and revision variants");
                };
                let outcome = state.apply_document_delta(
                    revision.change.start,
                    revision.change.deleted.chars().count(),
                    revision.change.inserted.chars().count(),
                    revision.change.position_evidence,
                );
                self.observe_personal_cache_edit(outcome, true);
                continue;
            };
            let Some(commit) = prepare_window_commit(decoder, event.elapsed_ms, record)? else {
                self.finish_personal_cache_window(decoder, state, kind, &mut run)?;
                let outcome = state.apply_document_delta(
                    record.document_change.start,
                    record.document_change.deleted.chars().count(),
                    record.document_change.inserted.chars().count(),
                    record.document_change.position_evidence,
                );
                self.observe_personal_cache_edit(outcome, false);
                if let Some(words) = prepare_personal_learning_words(decoder, record)? {
                    self.learn_personal_words(
                        state,
                        record.change.start,
                        record.change.inserted.chars().count(),
                        &words,
                    );
                    self.learn_personal_pairs(
                        state,
                        record.change.start,
                        record
                            .change
                            .start
                            .saturating_add(record.change.inserted.chars().count()),
                        &words,
                    );
                }
                continue;
            };
            let joins_previous = run.last().is_some_and(|previous| {
                commit.elapsed_ms.saturating_sub(previous.elapsed_ms) <= max_gap_ms
                    && commit.document_start == previous.document_end()
            });
            if !run.is_empty() && !joins_previous {
                self.finish_personal_cache_window(decoder, state, kind, &mut run)?;
            }
            run.push(commit);
        }
        self.finish_personal_cache_window(decoder, state, kind, &mut run)
    }

    fn finish_personal_cache_window(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        kind: PersonalCacheKind,
        run: &mut Vec<WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        if run.len() >= 2 {
            self.observe_personal_cache_window(decoder, state, kind, run)?;
        }
        for commit in run.iter() {
            let outcome = state.apply_document_delta(
                commit.document_start,
                0,
                commit.document_inserted_chars,
                crate::DeltaPositionEvidence::UniqueText,
            );
            self.observe_personal_cache_edit(outcome, false);
            self.learn_personal_words(
                state,
                commit.document_start,
                commit.document_inserted_chars,
                &commit.words,
            );
        }
        if let (Some(first), Some(last)) = (run.first(), run.last()) {
            let words = run
                .iter()
                .flat_map(|commit| commit.words.iter().cloned())
                .collect::<Vec<_>>();
            self.learn_personal_pairs(state, first.document_start, last.document_end(), &words);
        }
        run.clear();
        Ok(())
    }

    fn learn_personal_words(
        &mut self,
        state: &mut PersonalCacheReplayState,
        start: usize,
        inserted_chars: usize,
        words: &[String],
    ) {
        state.learn_commit(start, inserted_chars, words);
        self.personal_cache_learning_commits =
            self.personal_cache_learning_commits.saturating_add(1);
        self.personal_cache_learning_word_tokens = self
            .personal_cache_learning_word_tokens
            .saturating_add(u64::try_from(words.len()).unwrap_or(u64::MAX));
    }

    fn learn_personal_pairs(
        &mut self,
        state: &mut PersonalCacheReplayState,
        start: usize,
        end: usize,
        words: &[String],
    ) {
        let learned_pairs = state.learn_pair_sequence(start, end, words);
        if learned_pairs == 0 {
            return;
        }
        self.personal_cache_learning_pair_sequences = self
            .personal_cache_learning_pair_sequences
            .saturating_add(1);
        self.personal_cache_learning_word_pairs = self
            .personal_cache_learning_word_pairs
            .saturating_add(u64::try_from(learned_pairs).unwrap_or(u64::MAX));
    }

    fn observe_personal_cache_edit(
        &mut self,
        outcome: PersonalCacheEditOutcome,
        is_revision: bool,
    ) {
        self.personal_cache_reversed_commits = self
            .personal_cache_reversed_commits
            .saturating_add(outcome.invalidated_commits);
        self.personal_cache_reversed_word_tokens = self
            .personal_cache_reversed_word_tokens
            .saturating_add(outcome.invalidated_word_tokens);
        self.personal_cache_reversed_pair_sequences = self
            .personal_cache_reversed_pair_sequences
            .saturating_add(outcome.invalidated_pair_sequences);
        self.personal_cache_reversed_word_pairs = self
            .personal_cache_reversed_word_pairs
            .saturating_add(outcome.invalidated_word_pairs);
        if outcome.ambiguous_position {
            self.personal_cache_ambiguous_edits_not_applied = self
                .personal_cache_ambiguous_edits_not_applied
                .saturating_add(1);
        }
        if is_revision {
            if outcome.invalidated_commits > 0 || outcome.invalidated_pair_sequences > 0 {
                self.personal_cache_revision_events_with_reversal = self
                    .personal_cache_revision_events_with_reversal
                    .saturating_add(1);
            } else {
                self.personal_cache_revisions_not_reversed =
                    self.personal_cache_revisions_not_reversed.saturating_add(1);
            }
        }
    }

    fn observe_personal_cache_window(
        &mut self,
        decoder: &Decoder,
        state: &PersonalCacheReplayState,
        kind: PersonalCacheKind,
        run: &[WindowCommit],
    ) -> Result<(), KeySequenceError> {
        let canonical = run
            .iter()
            .map(|commit| commit.canonical_full.as_str())
            .collect::<String>();
        if canonical.len() > MAX_REPLAY_CODE_KEYS {
            return Ok(());
        }
        let target = run
            .iter()
            .map(|commit| commit.target.as_str())
            .collect::<String>();
        let syllable_codes = run
            .iter()
            .flat_map(|commit| commit.syllable_codes.iter().cloned())
            .collect::<Vec<_>>();
        let word_lengths = run
            .iter()
            .flat_map(|commit| commit.word_lengths.iter().copied())
            .collect::<Vec<_>>();
        let word_tail_code = word_tail_one_short(&syllable_codes, &word_lengths);
        let word_tail_keep_singletons_code =
            word_tail_keep_singletons(&syllable_codes, &word_lengths);
        let word_head_code = word_head_anchored(&syllable_codes, &word_lengths);

        self.personal_cache_windows = self.personal_cache_windows.saturating_add(1);
        let baseline_rank = strategy_rank(decoder, &canonical, &target)?;
        let personal_baseline_rank = observe_strategy_with_personal_cache(
            decoder,
            state,
            kind,
            &canonical,
            &target,
            &mut self.personal_cache_window_canonical_full,
        )?;
        if let Some(code) = &word_tail_code {
            let unigram_rank = strategy_rank(decoder, code, &target)?;
            let personal_rank = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_one_short,
            )?;
            self.personal_cache_window_word_tail_one_short_vs_full
                .observe(&canonical, code, personal_baseline_rank, personal_rank);
            self.word_tail_one_short_personal_cache_effect.observe(
                baseline_rank,
                unigram_rank,
                personal_baseline_rank,
                personal_rank,
            );
        }
        if let Some(code) = &word_tail_keep_singletons_code {
            let unigram_rank = strategy_rank(decoder, code, &target)?;
            let personal_rank = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_keep_singletons,
            )?;
            self.personal_cache_window_word_tail_keep_singletons_vs_full
                .observe(&canonical, code, personal_baseline_rank, personal_rank);
            self.word_tail_keep_singletons_personal_cache_effect
                .observe(
                    baseline_rank,
                    unigram_rank,
                    personal_baseline_rank,
                    personal_rank,
                );
        }
        if let Some(code) = &word_head_code {
            let unigram_rank = strategy_rank(decoder, code, &target)?;
            let personal_rank = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_head_anchored,
            )?;
            self.personal_cache_window_word_head_anchored_vs_full
                .observe(&canonical, code, personal_baseline_rank, personal_rank);
            self.word_head_anchored_personal_cache_effect.observe(
                baseline_rank,
                unigram_rank,
                personal_baseline_rank,
                personal_rank,
            );
        }
        Ok(())
    }

    fn finish_window(
        &mut self,
        decoder: &Decoder,
        public_context: Option<FrozenPublicContext<'_>>,
        run: &mut Vec<WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        if run.len() < 2 {
            run.clear();
            return Ok(());
        }
        self.continuous_windows = self.continuous_windows.saturating_add(1);
        self.continuous_window_commits = self
            .continuous_window_commits
            .saturating_add(u64::try_from(run.len()).unwrap_or(u64::MAX));
        self.continuous_window_recorded_logical_key_actions = self
            .continuous_window_recorded_logical_key_actions
            .saturating_add(
                run.iter()
                    .map(|commit| commit.recorded_logical_key_actions)
                    .fold(0_u64, u64::saturating_add),
            );

        let raw = run
            .iter()
            .map(|commit| commit.observed.as_str())
            .collect::<String>();
        let canonical = run
            .iter()
            .map(|commit| commit.canonical_full.as_str())
            .collect::<String>();
        let target = run
            .iter()
            .map(|commit| commit.target.as_str())
            .collect::<String>();
        let syllable_codes = run
            .iter()
            .flat_map(|commit| commit.syllable_codes.iter().cloned())
            .collect::<Vec<_>>();
        let word_lengths = run
            .iter()
            .flat_map(|commit| commit.word_lengths.iter().copied())
            .collect::<Vec<_>>();

        if raw.len() > MAX_REPLAY_CODE_KEYS || canonical.len() > MAX_REPLAY_CODE_KEYS {
            self.continuous_windows_over_key_limit =
                self.continuous_windows_over_key_limit.saturating_add(1);
            run.clear();
            return Ok(());
        }
        observe_strategy(decoder, &raw, &target, &mut self.window_raw_joined)?;
        let baseline_rank = observe_strategy_with_rank(
            decoder,
            &canonical,
            &target,
            &mut self.window_canonical_full,
        )?;
        let word_tail_code = word_tail_one_short(&syllable_codes, &word_lengths);
        let word_tail_keep_singletons_code =
            word_tail_keep_singletons(&syllable_codes, &word_lengths);
        let word_head_code = word_head_anchored(&syllable_codes, &word_lengths);

        let mut word_tail_rank = None;
        if let Some(code) = &word_tail_code {
            let strategy_rank = observe_strategy_with_rank(
                decoder,
                code,
                &target,
                &mut self.window_word_tail_one_short,
            )?;
            self.window_word_tail_one_short_vs_full.observe(
                &canonical,
                code,
                baseline_rank,
                strategy_rank,
            );
            word_tail_rank = strategy_rank;
        }
        let mut word_tail_keep_singletons_rank = None;
        if let Some(code) = &word_tail_keep_singletons_code {
            let strategy_rank = observe_strategy_with_rank(
                decoder,
                code,
                &target,
                &mut self.window_word_tail_keep_singletons,
            )?;
            self.window_word_tail_keep_singletons_vs_full.observe(
                &canonical,
                code,
                baseline_rank,
                strategy_rank,
            );
            word_tail_keep_singletons_rank = strategy_rank;
        }
        let mut word_head_rank = None;
        if let Some(code) = &word_head_code {
            let strategy_rank = observe_strategy_with_rank(
                decoder,
                code,
                &target,
                &mut self.window_word_head_anchored,
            )?;
            self.window_word_head_anchored_vs_full.observe(
                &canonical,
                code,
                baseline_rank,
                strategy_rank,
            );
            word_head_rank = strategy_rank;
        }

        if let Some(context) = public_context {
            self.public_context_windows = self.public_context_windows.saturating_add(1);
            let context_baseline_rank = observe_strategy_with_frozen_context(
                decoder,
                context,
                &canonical,
                &target,
                &mut self.public_context_window_canonical_full,
            )?;
            if let Some(code) = &word_tail_code {
                let context_strategy_rank = observe_strategy_with_frozen_context(
                    decoder,
                    context,
                    code,
                    &target,
                    &mut self.public_context_window_word_tail_one_short,
                )?;
                self.public_context_window_word_tail_one_short_vs_full
                    .observe(
                        &canonical,
                        code,
                        context_baseline_rank,
                        context_strategy_rank,
                    );
                self.word_tail_one_short_context_effect.observe(
                    baseline_rank,
                    word_tail_rank,
                    context_baseline_rank,
                    context_strategy_rank,
                );
            }
            if let Some(code) = &word_tail_keep_singletons_code {
                let context_strategy_rank = observe_strategy_with_frozen_context(
                    decoder,
                    context,
                    code,
                    &target,
                    &mut self.public_context_window_word_tail_keep_singletons,
                )?;
                self.public_context_window_word_tail_keep_singletons_vs_full
                    .observe(
                        &canonical,
                        code,
                        context_baseline_rank,
                        context_strategy_rank,
                    );
                self.word_tail_keep_singletons_context_effect.observe(
                    baseline_rank,
                    word_tail_keep_singletons_rank,
                    context_baseline_rank,
                    context_strategy_rank,
                );
            }
            if let Some(code) = &word_head_code {
                let context_strategy_rank = observe_strategy_with_frozen_context(
                    decoder,
                    context,
                    code,
                    &target,
                    &mut self.public_context_window_word_head_anchored,
                )?;
                self.public_context_window_word_head_anchored_vs_full
                    .observe(
                        &canonical,
                        code,
                        context_baseline_rank,
                        context_strategy_rank,
                    );
                self.word_head_anchored_context_effect.observe(
                    baseline_rank,
                    word_head_rank,
                    context_baseline_rank,
                    context_strategy_rank,
                );
            }
        }
        run.clear();
        Ok(())
    }

    pub fn terminal_line(&self) -> String {
        format!(
            "CAPSULE_REPLAY_REPORT contains_text=false window_gap_limit_ms={} \
             public_context_kind={} \
             capsules={} events={} commits={} \
             revisions={} recorded_logical_key_actions={} \
             incomplete_key_commits={} key_interpretation_failures={} \
             commits_without_letter_code={} commits_over_key_limit={} \
             composition_encodable_commits={} observed_matches_canonical={} \
             noncanonical_code_observations={} noncanonical_is_error=false \
             word_boundaries_available_commits={} word_boundaries_unavailable_commits={} \
             {} {} {} {} {} {} {} {} \
             window_eligible_commits={} window_ineligible_commits={} continuous_windows={} \
             continuous_window_commits={} \
             continuous_window_recorded_logical_key_actions={} \
             continuous_windows_over_key_limit={} {} {} {} {} {} {} {} {} \
             public_context_windows={} {} {} {} {} {} {} {} {} {} {} \
             personal_cache_kind={} personal_cache_history_capsules={} \
             personal_cache_history_events={} \
             personal_cache_history_learning_commits={} \
             personal_cache_history_word_tokens={} \
             personal_cache_history_word_types={} \
             personal_cache_history_repeated_word_tokens={} \
             personal_cache_history_word_pairs={} \
             personal_cache_history_word_pair_types={} \
             personal_cache_history_repeated_word_pairs={} \
             personal_cache_windows={} \
             personal_cache_learning_commits={} \
             personal_cache_learning_word_tokens={} personal_cache_retained_word_tokens={} \
             personal_cache_learned_word_types={} personal_cache_repeated_word_tokens={} \
             personal_cache_reversed_commits={} \
             personal_cache_reversed_word_tokens={} \
             personal_cache_learning_pair_sequences={} \
             personal_cache_learning_word_pairs={} \
             personal_cache_retained_word_pairs={} \
             personal_cache_learned_word_pair_types={} \
             personal_cache_repeated_word_pairs={} \
             personal_cache_reversed_pair_sequences={} \
             personal_cache_reversed_word_pairs={} \
             personal_cache_revision_events_with_reversal={} \
             personal_cache_revisions_not_reversed={} \
             personal_cache_ambiguous_edits_not_applied={} \
             {} {} {} {} {} {} {} {} {} {}",
            display_optional(self.window_gap_limit_ms),
            display_public_context_kind(self.public_context_kind),
            self.capsules,
            self.events,
            self.commits,
            self.revisions,
            self.recorded_logical_key_actions,
            self.incomplete_key_commits,
            self.key_interpretation_failures,
            self.commits_without_letter_code,
            self.commits_over_key_limit,
            self.composition_encodable_commits,
            self.observed_matches_canonical,
            self.composition_encodable_commits
                .saturating_sub(self.observed_matches_canonical),
            self.word_boundaries_available_commits,
            self.word_boundaries_unavailable_commits,
            self.raw_existing.terminal_fields("raw_existing"),
            self.canonical_full.terminal_fields("canonical_full"),
            self.tail_one_short.terminal_fields("tail_one_short"),
            self.head_anchored.terminal_fields("head_anchored"),
            self.all_short.terminal_fields("all_short"),
            self.word_tail_one_short
                .terminal_fields("word_tail_one_short"),
            self.word_tail_keep_singletons
                .terminal_fields("word_tail_keep_singletons"),
            self.word_head_anchored
                .terminal_fields("word_head_anchored"),
            self.window_eligible_commits,
            self.window_ineligible_commits,
            self.continuous_windows,
            self.continuous_window_commits,
            self.continuous_window_recorded_logical_key_actions,
            self.continuous_windows_over_key_limit,
            self.window_raw_joined.terminal_fields("window_raw_joined"),
            self.window_canonical_full
                .terminal_fields("window_canonical_full"),
            self.window_word_tail_one_short
                .terminal_fields("window_word_tail_one_short"),
            self.window_word_tail_keep_singletons
                .terminal_fields("window_word_tail_keep_singletons"),
            self.window_word_head_anchored
                .terminal_fields("window_word_head_anchored"),
            self.window_word_tail_one_short_vs_full
                .terminal_fields("window_word_tail_one_short_vs_full"),
            self.window_word_tail_keep_singletons_vs_full
                .terminal_fields("window_word_tail_keep_singletons_vs_full"),
            self.window_word_head_anchored_vs_full
                .terminal_fields("window_word_head_anchored_vs_full"),
            self.public_context_windows,
            self.public_context_window_canonical_full
                .terminal_fields("public_context_window_canonical_full"),
            self.public_context_window_word_tail_one_short
                .terminal_fields("public_context_window_word_tail_one_short"),
            self.public_context_window_word_tail_keep_singletons
                .terminal_fields("public_context_window_word_tail_keep_singletons"),
            self.public_context_window_word_head_anchored
                .terminal_fields("public_context_window_word_head_anchored"),
            self.public_context_window_word_tail_one_short_vs_full
                .terminal_fields("public_context_window_word_tail_one_short_vs_full"),
            self.public_context_window_word_tail_keep_singletons_vs_full
                .terminal_fields("public_context_window_word_tail_keep_singletons_vs_full"),
            self.public_context_window_word_head_anchored_vs_full
                .terminal_fields("public_context_window_word_head_anchored_vs_full"),
            self.word_tail_one_short_context_effect
                .terminal_fields("word_tail_one_short_context_effect"),
            self.word_tail_keep_singletons_context_effect
                .terminal_fields("word_tail_keep_singletons_context_effect"),
            self.word_head_anchored_context_effect
                .terminal_fields("word_head_anchored_context_effect"),
            display_personal_cache_kind(self.personal_cache_kind),
            self.personal_cache_history_capsules,
            self.personal_cache_history_events,
            self.personal_cache_history_learning_commits,
            self.personal_cache_history_word_tokens,
            self.personal_cache_history_word_types,
            self.personal_cache_history_word_tokens
                .saturating_sub(self.personal_cache_history_word_types),
            self.personal_cache_history_word_pairs,
            self.personal_cache_history_word_pair_types,
            self.personal_cache_history_word_pairs
                .saturating_sub(self.personal_cache_history_word_pair_types),
            self.personal_cache_windows,
            self.personal_cache_learning_commits,
            self.personal_cache_learning_word_tokens,
            self.personal_cache_retained_word_tokens,
            self.personal_cache_learned_word_types,
            self.personal_cache_retained_word_tokens
                .saturating_sub(self.personal_cache_learned_word_types),
            self.personal_cache_reversed_commits,
            self.personal_cache_reversed_word_tokens,
            self.personal_cache_learning_pair_sequences,
            self.personal_cache_learning_word_pairs,
            self.personal_cache_retained_word_pairs,
            self.personal_cache_learned_word_pair_types,
            self.personal_cache_retained_word_pairs
                .saturating_sub(self.personal_cache_learned_word_pair_types),
            self.personal_cache_reversed_pair_sequences,
            self.personal_cache_reversed_word_pairs,
            self.personal_cache_revision_events_with_reversal,
            self.personal_cache_revisions_not_reversed,
            self.personal_cache_ambiguous_edits_not_applied,
            self.personal_cache_window_canonical_full
                .terminal_fields("personal_cache_window_canonical_full"),
            self.personal_cache_window_word_tail_one_short
                .terminal_fields("personal_cache_window_word_tail_one_short"),
            self.personal_cache_window_word_tail_keep_singletons
                .terminal_fields("personal_cache_window_word_tail_keep_singletons"),
            self.personal_cache_window_word_head_anchored
                .terminal_fields("personal_cache_window_word_head_anchored"),
            self.personal_cache_window_word_tail_one_short_vs_full
                .terminal_fields("personal_cache_window_word_tail_one_short_vs_full"),
            self.personal_cache_window_word_tail_keep_singletons_vs_full
                .terminal_fields("personal_cache_window_word_tail_keep_singletons_vs_full"),
            self.personal_cache_window_word_head_anchored_vs_full
                .terminal_fields("personal_cache_window_word_head_anchored_vs_full"),
            self.word_tail_one_short_personal_cache_effect
                .terminal_fields("word_tail_one_short_personal_cache_effect"),
            self.word_tail_keep_singletons_personal_cache_effect
                .terminal_fields("word_tail_keep_singletons_personal_cache_effect"),
            self.word_head_anchored_personal_cache_effect
                .terminal_fields("word_head_anchored_personal_cache_effect")
        )
    }

    pub fn compact_terminal_report(&self) -> String {
        let mut lines = vec![
            format!(
                "CAPSULE_REPLAY_COMPACT contains_text=false capsules={} events={} commits={} \
                 revisions={} recorded_actions={}",
                self.capsules,
                self.events,
                self.commits,
                self.revisions,
                self.recorded_logical_key_actions
            ),
            format!(
                "QUALITY incomplete_keys={} key_failures={} missing_letter_code={} \
                 over_key_limit={} composition_encodable={} canonical_matches={} \
                 noncanonical_observations={} noncanonical_is_error=false \
                 word_boundaries_available={} word_boundaries_unavailable={}",
                self.incomplete_key_commits,
                self.key_interpretation_failures,
                self.commits_without_letter_code,
                self.commits_over_key_limit,
                self.composition_encodable_commits,
                self.observed_matches_canonical,
                self.composition_encodable_commits
                    .saturating_sub(self.observed_matches_canonical),
                self.word_boundaries_available_commits,
                self.word_boundaries_unavailable_commits
            ),
        ];
        if self.window_gap_limit_ms.is_some() {
            lines.push(format!(
                "WINDOWS gap_ms={} eligible_commits={} ineligible_commits={} windows={} \
                 window_commits={} recorded_actions={} over_key_limit={} \
                 public_context_windows={} public_context_pool_depth={} \
                 personal_cache_kind={} personal_cache_history_capsules={} \
                 personal_cache_history_events={} \
                 personal_cache_history_learning_commits={} \
                 personal_cache_history_word_tokens={} \
                 personal_cache_history_word_types={} \
                 personal_cache_history_repeated_word_tokens={} \
                 personal_cache_history_word_pairs={} \
                 personal_cache_history_word_pair_types={} \
                 personal_cache_history_repeated_word_pairs={} \
                 personal_cache_windows={} \
                 personal_cache_learning_commits={} \
                 personal_cache_learning_word_tokens={} personal_cache_retained_word_tokens={} \
                 personal_cache_learned_word_types={} \
                 personal_cache_repeated_word_tokens={} personal_cache_reversed_commits={} \
                 personal_cache_reversed_word_tokens={} \
                 personal_cache_learning_pair_sequences={} \
                 personal_cache_learning_word_pairs={} \
                 personal_cache_retained_word_pairs={} \
                 personal_cache_learned_word_pair_types={} \
                 personal_cache_repeated_word_pairs={} \
                 personal_cache_reversed_pair_sequences={} \
                 personal_cache_reversed_word_pairs={} \
                 personal_cache_revision_events_with_reversal={} \
                 personal_cache_revisions_not_reversed={} \
                 personal_cache_ambiguous_edits_not_applied={} personal_cache_pool_depth={} \
                 personal_cache_max_promotion={}",
                display_optional(self.window_gap_limit_ms),
                self.window_eligible_commits,
                self.window_ineligible_commits,
                self.continuous_windows,
                self.continuous_window_commits,
                self.continuous_window_recorded_logical_key_actions,
                self.continuous_windows_over_key_limit,
                self.public_context_windows,
                PUBLIC_CONTEXT_POOL_DEPTH,
                display_personal_cache_kind(self.personal_cache_kind),
                self.personal_cache_history_capsules,
                self.personal_cache_history_events,
                self.personal_cache_history_learning_commits,
                self.personal_cache_history_word_tokens,
                self.personal_cache_history_word_types,
                self.personal_cache_history_word_tokens
                    .saturating_sub(self.personal_cache_history_word_types),
                self.personal_cache_history_word_pairs,
                self.personal_cache_history_word_pair_types,
                self.personal_cache_history_word_pairs
                    .saturating_sub(self.personal_cache_history_word_pair_types),
                self.personal_cache_windows,
                self.personal_cache_learning_commits,
                self.personal_cache_learning_word_tokens,
                self.personal_cache_retained_word_tokens,
                self.personal_cache_learned_word_types,
                self.personal_cache_retained_word_tokens
                    .saturating_sub(self.personal_cache_learned_word_types),
                self.personal_cache_reversed_commits,
                self.personal_cache_reversed_word_tokens,
                self.personal_cache_learning_pair_sequences,
                self.personal_cache_learning_word_pairs,
                self.personal_cache_retained_word_pairs,
                self.personal_cache_learned_word_pair_types,
                self.personal_cache_retained_word_pairs
                    .saturating_sub(self.personal_cache_learned_word_pair_types),
                self.personal_cache_reversed_pair_sequences,
                self.personal_cache_reversed_word_pairs,
                self.personal_cache_revision_events_with_reversal,
                self.personal_cache_revisions_not_reversed,
                self.personal_cache_ambiguous_edits_not_applied,
                PERSONAL_CACHE_POOL_DEPTH,
                PERSONAL_CACHE_MAX_PROMOTION
            ));
            if self.personal_cache_windows > 0 {
                let cache_kind = self
                    .personal_cache_kind
                    .expect("personal cache windows must have one cache kind");
                lines.extend([
                    compact_strategy_line(
                        "window_unigram",
                        "canonical_full",
                        &self.window_canonical_full,
                    ),
                    compact_strategy_line(
                        cache_kind.compact_scope(),
                        "canonical_full",
                        &self.personal_cache_window_canonical_full,
                    ),
                    compact_context_comparison_line(
                        cache_kind.compact_context_label(),
                        "word_tail_one_short",
                        &self.window_word_tail_one_short_vs_full,
                        &self.personal_cache_window_word_tail_one_short_vs_full,
                        &self.word_tail_one_short_personal_cache_effect,
                        &self.personal_cache_window_word_tail_one_short,
                    ),
                    compact_context_comparison_line(
                        cache_kind.compact_context_label(),
                        "word_tail_keep_singletons",
                        &self.window_word_tail_keep_singletons_vs_full,
                        &self.personal_cache_window_word_tail_keep_singletons_vs_full,
                        &self.word_tail_keep_singletons_personal_cache_effect,
                        &self.personal_cache_window_word_tail_keep_singletons,
                    ),
                    compact_context_comparison_line(
                        cache_kind.compact_context_label(),
                        "word_head_anchored",
                        &self.window_word_head_anchored_vs_full,
                        &self.personal_cache_window_word_head_anchored_vs_full,
                        &self.word_head_anchored_personal_cache_effect,
                        &self.personal_cache_window_word_head_anchored,
                    ),
                ]);
            } else if self.public_context_windows > 0 {
                let context_kind = self
                    .public_context_kind
                    .expect("public context windows must have one context kind");
                lines.extend([
                    compact_strategy_line(
                        "window_unigram",
                        "canonical_full",
                        &self.window_canonical_full,
                    ),
                    compact_strategy_line(
                        context_kind.compact_scope(),
                        "canonical_full",
                        &self.public_context_window_canonical_full,
                    ),
                    compact_context_comparison_line(
                        context_kind.compact_context_label(),
                        "word_tail_one_short",
                        &self.window_word_tail_one_short_vs_full,
                        &self.public_context_window_word_tail_one_short_vs_full,
                        &self.word_tail_one_short_context_effect,
                        &self.public_context_window_word_tail_one_short,
                    ),
                    compact_context_comparison_line(
                        context_kind.compact_context_label(),
                        "word_tail_keep_singletons",
                        &self.window_word_tail_keep_singletons_vs_full,
                        &self.public_context_window_word_tail_keep_singletons_vs_full,
                        &self.word_tail_keep_singletons_context_effect,
                        &self.public_context_window_word_tail_keep_singletons,
                    ),
                    compact_context_comparison_line(
                        context_kind.compact_context_label(),
                        "word_head_anchored",
                        &self.window_word_head_anchored_vs_full,
                        &self.public_context_window_word_head_anchored_vs_full,
                        &self.word_head_anchored_context_effect,
                        &self.public_context_window_word_head_anchored,
                    ),
                ]);
            } else {
                lines.extend([
                    compact_strategy_line("window", "canonical_full", &self.window_canonical_full),
                    compact_paired_strategy_line(
                        "word_tail_one_short",
                        &self.window_word_tail_one_short_vs_full,
                        &self.window_word_tail_one_short,
                    ),
                    compact_paired_strategy_line(
                        "word_tail_keep_singletons",
                        &self.window_word_tail_keep_singletons_vs_full,
                        &self.window_word_tail_keep_singletons,
                    ),
                    compact_paired_strategy_line(
                        "word_head_anchored",
                        &self.window_word_head_anchored_vs_full,
                        &self.window_word_head_anchored,
                    ),
                ]);
            }
        } else {
            lines.push("SCOPE mode=individual_commits".to_owned());
            lines.extend([
                compact_strategy_line("commit", "raw_existing", &self.raw_existing),
                compact_strategy_line("commit", "canonical_full", &self.canonical_full),
                compact_strategy_line("commit", "word_tail_one_short", &self.word_tail_one_short),
                compact_strategy_line(
                    "commit",
                    "word_tail_keep_singletons",
                    &self.word_tail_keep_singletons,
                ),
                compact_strategy_line("commit", "word_head_anchored", &self.word_head_anchored),
            ]);
        }
        lines.join("\n")
    }
}

struct WindowCommit {
    elapsed_ms: u64,
    document_start: usize,
    document_inserted_chars: usize,
    observed: String,
    canonical_full: String,
    syllable_codes: Vec<KeySequence>,
    word_lengths: Vec<usize>,
    words: Vec<String>,
    target: String,
    recorded_logical_key_actions: u64,
}

impl WindowCommit {
    fn document_end(&self) -> usize {
        self.document_start
            .saturating_add(self.document_inserted_chars)
    }
}

fn prepare_window_commit(
    decoder: &Decoder,
    elapsed_ms: u64,
    record: &crate::CommitRecord,
) -> Result<Option<WindowCommit>, KeySequenceError> {
    if !record.keys_complete || !window_document_delta_is_eligible(record) {
        return Ok(None);
    }
    let Some(observed) = effective_letter_code(&record.keys).ok().flatten() else {
        return Ok(None);
    };
    if observed.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let normalized_pinyin = record.composition.replace('\'', " ");
    let Ok(encoded) = encode_pinyin_phrase(&normalized_pinyin) else {
        return Ok(None);
    };
    let candidates = decoder.decode_sentence(encoded.full_code.as_str(), REPLAY_TOP_K)?;
    let Some(segmentation) = exact_target_segmentation(
        &candidates,
        &record.change.inserted,
        encoded.syllable_codes.len(),
    ) else {
        return Ok(None);
    };
    Ok(Some(WindowCommit {
        elapsed_ms,
        document_start: record.document_change.start,
        document_inserted_chars: record.document_change.inserted.chars().count(),
        observed,
        canonical_full: encoded.full_code.as_str().to_owned(),
        syllable_codes: encoded.syllable_codes,
        word_lengths: segmentation.word_lengths,
        words: segmentation.words,
        target: record.change.inserted.clone(),
        recorded_logical_key_actions: saturating_len(record.keys.len()),
    }))
}

fn window_document_delta_is_eligible(record: &crate::CommitRecord) -> bool {
    record.change.position_evidence != crate::DeltaPositionEvidence::Ambiguous
        && record.document_change.position_evidence != crate::DeltaPositionEvidence::Ambiguous
        && record.document_change.deleted.is_empty()
        && !record.document_change.inserted.is_empty()
        && record.document_change.inserted == record.change.inserted
}

fn prepare_personal_learning_words(
    decoder: &Decoder,
    record: &crate::CommitRecord,
) -> Result<Option<Vec<String>>, KeySequenceError> {
    if !record.keys_complete
        || record.change.inserted.is_empty()
        || record.change.position_evidence == crate::DeltaPositionEvidence::Ambiguous
        || record.document_change.position_evidence == crate::DeltaPositionEvidence::Ambiguous
    {
        return Ok(None);
    }
    let normalized_pinyin = record.composition.replace('\'', " ");
    let Ok(encoded) = encode_pinyin_phrase(&normalized_pinyin) else {
        return Ok(None);
    };
    let candidates = decoder.decode_sentence(encoded.full_code.as_str(), REPLAY_TOP_K)?;
    Ok(exact_target_segmentation(
        &candidates,
        &record.change.inserted,
        encoded.syllable_codes.len(),
    )
    .map(|segmentation| segmentation.words))
}

fn observe_strategy(
    decoder: &Decoder,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<(), KeySequenceError> {
    let _ = observe_strategy_with_rank(decoder, code, target, stats)?;
    Ok(())
}

fn decode_sentence_memoized(
    decoder: &Decoder,
    cache: &mut HashMap<String, Vec<SentenceCandidate>>,
    code: &str,
) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
    if let Some(candidates) = cache.get(code) {
        return Ok(candidates.clone());
    }
    let candidates = decoder.decode_sentence(code, REPLAY_TOP_K)?;
    cache.insert(code.to_owned(), candidates.clone());
    Ok(candidates)
}

fn observe_strategy_memoized(
    decoder: &Decoder,
    cache: &mut HashMap<String, Vec<SentenceCandidate>>,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<(), KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(());
    }
    let candidates = decode_sentence_memoized(decoder, cache, code)?;
    let _ = stats.observe(code, target, &candidates);
    Ok(())
}

fn observe_strategy_with_rank(
    decoder: &Decoder,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<Option<usize>, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let candidates = decoder.decode_sentence(code, REPLAY_TOP_K)?;
    Ok(stats.observe(code, target, &candidates))
}

fn strategy_rank(
    decoder: &Decoder,
    code: &str,
    target: &str,
) -> Result<Option<usize>, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let candidates = decoder.decode_sentence(code, REPLAY_TOP_K)?;
    Ok(candidates
        .iter()
        .position(|candidate| candidate.text == target)
        .map(|rank| rank + 1))
}

#[derive(Clone, Copy)]
struct PersonalCacheEvidence {
    promotion: usize,
    observed_word_tokens: usize,
    prior_occurrences: u64,
    observed_word_pairs: usize,
    prior_pair_occurrences: u64,
}

fn observe_strategy_with_personal_cache(
    decoder: &Decoder,
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<Option<usize>, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let pool = decoder.decode_sentence(code, PERSONAL_CACHE_POOL_DEPTH)?;
    let mut scored = pool
        .into_iter()
        .enumerate()
        .map(|(baseline_rank, candidate)| {
            let evidence = personal_cache_evidence(&candidate, state, kind);
            (baseline_rank, candidate, evidence)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        let left_adjusted_rank = left.0.saturating_sub(left.2.promotion);
        let right_adjusted_rank = right.0.saturating_sub(right.2.promotion);
        left_adjusted_rank
            .cmp(&right_adjusted_rank)
            .then_with(|| {
                right
                    .2
                    .prior_pair_occurrences
                    .cmp(&left.2.prior_pair_occurrences)
            })
            .then_with(|| right.2.observed_word_pairs.cmp(&left.2.observed_word_pairs))
            .then_with(|| right.2.prior_occurrences.cmp(&left.2.prior_occurrences))
            .then_with(|| {
                right
                    .2
                    .observed_word_tokens
                    .cmp(&left.2.observed_word_tokens)
            })
            .then_with(|| left.0.cmp(&right.0))
    });
    let candidates = scored
        .into_iter()
        .take(REPLAY_TOP_K)
        .map(|(_, candidate, _)| candidate)
        .collect::<Vec<_>>();
    Ok(stats.observe(code, target, &candidates))
}

fn personal_cache_evidence(
    candidate: &SentenceCandidate,
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
) -> PersonalCacheEvidence {
    let mut observed_word_tokens = 0_usize;
    let mut prior_occurrences = 0_u64;
    let mut observed_word_pairs = 0_usize;
    let mut prior_pair_occurrences = 0_u64;
    let mut previous_word = None::<&str>;
    for segment in &candidate.segments {
        if segment.candidate.source != CandidateSource::Lexicon {
            previous_word = None;
            continue;
        }
        if kind == PersonalCacheKind::OrderedWordPairs {
            if let Some(previous) = previous_word {
                let count = state.pair_count(previous, &segment.candidate.text);
                if count > 0 {
                    observed_word_pairs = observed_word_pairs.saturating_add(1);
                    prior_pair_occurrences = prior_pair_occurrences.saturating_add(count);
                }
            }
            previous_word = Some(&segment.candidate.text);
        }
        let count = state.count(&segment.candidate.text);
        if count > 0 {
            observed_word_tokens = observed_word_tokens.saturating_add(1);
            prior_occurrences = prior_occurrences.saturating_add(count);
        }
    }
    let word_promotion = match prior_occurrences {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        _ => PERSONAL_CACHE_MAX_PROMOTION,
    };
    let pair_promotion = match prior_pair_occurrences {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    let promotion = word_promotion
        .saturating_add(pair_promotion)
        .min(PERSONAL_CACHE_MAX_PROMOTION);
    PersonalCacheEvidence {
        promotion,
        observed_word_tokens,
        prior_occurrences,
        observed_word_pairs,
        prior_pair_occurrences,
    }
}

fn observe_strategy_with_frozen_context(
    decoder: &Decoder,
    context: FrozenPublicContext<'_>,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<Option<usize>, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let pool = decoder.decode_sentence(code, PUBLIC_CONTEXT_POOL_DEPTH)?;
    let mut scored = pool
        .into_iter()
        .enumerate()
        .map(|(baseline_rank, candidate)| {
            let score = match context {
                FrozenPublicContext::Word {
                    language_model,
                    log_frequency_total,
                } => score_candidate_with_public_word_context(
                    &candidate,
                    language_model,
                    log_frequency_total,
                ),
                FrozenPublicContext::Character { language_model } => {
                    score_candidate_with_public_character_context(&candidate, language_model)
                }
            };
            (baseline_rank, candidate, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
    });
    let candidates = scored
        .into_iter()
        .take(REPLAY_TOP_K)
        .map(|(_, candidate, _)| candidate)
        .collect::<Vec<_>>();
    Ok(stats.observe(code, target, &candidates))
}

fn score_candidate_with_public_word_context(
    candidate: &SentenceCandidate,
    language_model: &BigramLanguageModel,
    log_frequency_total: f64,
) -> f64 {
    let mut total_score = 0.0;
    let mut previous_word = None::<&str>;
    for segment in &candidate.segments {
        if segment.candidate.source == CandidateSource::UnresolvedInput {
            total_score -= segment.candidate.score.unresolved_input_penalty;
            previous_word = None;
            continue;
        }
        let unigram = segment.candidate.score.frequency - log_frequency_total;
        let language_score = previous_word.map_or(unigram, |previous| {
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram
                + BIGRAM_INTERPOLATION_WEIGHT
                    * language_model
                        .score(previous, &segment.candidate.text)
                        .log_probability
        });
        total_score += language_score
            - segment.candidate.score.abbreviation_penalty
            - segment.candidate.score.correction_penalty;
        previous_word = Some(&segment.candidate.text);
    }
    total_score
}

fn score_candidate_with_public_character_context(
    candidate: &SentenceCandidate,
    language_model: &CharacterBigramLanguageModel,
) -> f64 {
    let evidence = language_model.score_text(&candidate.text);
    if evidence.pair_count == 0 {
        f64::NEG_INFINITY
    } else {
        evidence.log_probability / evidence.pair_count as f64
    }
}

fn display_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

fn display_public_context_kind(kind: Option<PublicContextKind>) -> &'static str {
    kind.map_or("none", PublicContextKind::terminal_label)
}

fn display_personal_cache_kind(kind: Option<PersonalCacheKind>) -> &'static str {
    kind.map_or("none", PersonalCacheKind::terminal_label)
}

fn display_rank_histogram(histogram: &[u64; REPLAY_TOP_K]) -> String {
    let fields = histogram
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(rank, count)| format!("{}:{count}", rank + 1))
        .collect::<Vec<_>>();
    if fields.is_empty() {
        "none".to_owned()
    } else {
        fields.join(",")
    }
}

fn compact_strategy_line(scope: &str, name: &str, stats: &ReplayStrategyStats) -> String {
    format!(
        "STRATEGY scope={scope} name={name} attempts={} input_keys={} \
         projected_actions_one_selection={} hits_at_1={} hits_at_5={} hits_at_10={} \
         misses_at_10={} ranks_at_10={}",
        stats.attempts,
        stats.input_keys,
        stats.projected_actions_with_one_selection(),
        stats.hits_at_1,
        stats.hits_at_5,
        stats.hits_at_10,
        stats.attempts.saturating_sub(stats.hits_at_10),
        display_rank_histogram(&stats.rank_histogram_at_10)
    )
}

fn compact_paired_strategy_line(
    name: &str,
    paired: &PairedReplayStrategyStats,
    strategy: &ReplayStrategyStats,
) -> String {
    format!(
        "PAIRED scope=window name={name} comparisons={} shortened={} unchanged={} \
         baseline_keys={} strategy_keys={} saved_keys={} added_keys={} \
         hits_at_1={} hits_at_5={} hits_at_10={} misses_at_10={} ranks_at_10={} \
         improved={} same={} worsened={} both_outside_top_10={} \
         dropped_from_top_10={} recovered_into_top_10={}",
        paired.comparisons,
        paired.shortened_codes,
        paired.unchanged_codes,
        paired.baseline_input_keys,
        paired.strategy_input_keys,
        paired.input_keys_saved,
        paired.input_keys_added,
        strategy.hits_at_1,
        strategy.hits_at_5,
        strategy.hits_at_10,
        strategy.attempts.saturating_sub(strategy.hits_at_10),
        display_rank_histogram(&strategy.rank_histogram_at_10),
        paired.rank_improved,
        paired.rank_same,
        paired.rank_worsened,
        paired.both_outside_top_10,
        paired.dropped_from_top_10,
        paired.recovered_into_top_10
    )
}

fn compact_context_comparison_line(
    context_label: &str,
    name: &str,
    unigram: &PairedReplayStrategyStats,
    context: &PairedReplayStrategyStats,
    effect: &ContextReplayComparisonStats,
    context_strategy: &ReplayStrategyStats,
) -> String {
    format!(
        "CONTEXT_COMPARE context={context_label} name={name} comparisons={} saved_keys={} \
         unigram_improved={} unigram_same={} unigram_worsened={} unigram_dropped={} \
         context_improved={} context_same={} context_worsened={} context_dropped={} \
         context_hits_at_1={} context_hits_at_5={} context_hits_at_10={} \
         context_ranks_at_10={} comparable_baselines={} relative_reduced={} \
         relative_same={} relative_increased={} drops_rescued={} new_drops={} \
         unigram_baseline_only={} context_baseline_only={} neither_baseline={}",
        unigram.comparisons,
        unigram.input_keys_saved,
        unigram.rank_improved,
        unigram.rank_same,
        unigram.rank_worsened,
        unigram.dropped_from_top_10,
        context.rank_improved,
        context.rank_same,
        context.rank_worsened,
        context.dropped_from_top_10,
        context_strategy.hits_at_1,
        context_strategy.hits_at_5,
        context_strategy.hits_at_10,
        display_rank_histogram(&context_strategy.rank_histogram_at_10),
        effect.baselines_both_visible_at_10,
        effect.relative_degradation_reduced,
        effect.relative_degradation_same,
        effect.relative_degradation_increased,
        effect.drops_rescued_by_context,
        effect.new_drops_with_context,
        effect.unigram_baseline_only_visible_at_10,
        effect.context_baseline_only_visible_at_10,
        effect.neither_baseline_visible_at_10
    )
}

fn rank_for_comparison(rank: Option<usize>) -> i16 {
    rank.map_or((REPLAY_TOP_K + 1) as i16, |rank| rank as i16)
}

fn saturating_len(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyInterpretationError {
    InputAfterCandidateSelection,
    ShiftedCompositionEdit,
    CancelledComposition,
}

impl fmt::Display for KeyInterpretationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputAfterCandidateSelection => {
                write!(formatter, "input continued after candidate selection")
            }
            Self::ShiftedCompositionEdit => {
                write!(
                    formatter,
                    "shifted editing inside a composition is unsupported"
                )
            }
            Self::CancelledComposition => write!(formatter, "composition was cancelled"),
        }
    }
}

impl Error for KeyInterpretationError {}

pub fn effective_letter_code(keys: &[RawKey]) -> Result<Option<String>, KeyInterpretationError> {
    let mut code = Vec::<char>::new();
    let mut cursor = 0_usize;
    let mut started = false;
    let mut selected = false;

    for key in keys {
        match key {
            RawKey::Letter(letter) => {
                if selected {
                    return Err(KeyInterpretationError::InputAfterCandidateSelection);
                }
                started = true;
                code.insert(cursor, *letter);
                cursor += 1;
            }
            RawKey::Backspace if started && !selected => {
                if cursor > 0 {
                    cursor -= 1;
                    code.remove(cursor);
                }
            }
            RawKey::Delete if started && !selected => {
                if cursor < code.len() {
                    code.remove(cursor);
                }
            }
            RawKey::Left if started && !selected => {
                cursor = cursor.saturating_sub(1);
            }
            RawKey::Right if started && !selected => {
                cursor = (cursor + 1).min(code.len());
            }
            RawKey::Home if started && !selected => cursor = 0,
            RawKey::End if started && !selected => cursor = code.len(),
            RawKey::Space | RawKey::Digit(_) | RawKey::Up | RawKey::Down if started => {
                selected = true;
            }
            RawKey::Escape if started => {
                return Err(KeyInterpretationError::CancelledComposition);
            }
            RawKey::Shift(_) if started => {
                return Err(KeyInterpretationError::ShiftedCompositionEdit);
            }
            RawKey::Backspace
            | RawKey::Delete
            | RawKey::Digit(_)
            | RawKey::Space
            | RawKey::Escape
            | RawKey::Left
            | RawKey::Right
            | RawKey::Up
            | RawKey::Down
            | RawKey::Home
            | RawKey::End
            | RawKey::Shift(_) => {}
        }
    }

    if !started || code.is_empty() {
        return Ok(None);
    }
    Ok(Some(code.into_iter().collect()))
}

fn tail_one_short(syllable_codes: &[crate::KeySequence]) -> Option<String> {
    shortened_code(syllable_codes, |index, length| index + 1 == length)
}

fn head_anchored(syllable_codes: &[crate::KeySequence]) -> Option<String> {
    shortened_code(syllable_codes, |index, _| index > 0)
}

fn all_short(syllable_codes: &[crate::KeySequence]) -> Option<String> {
    shortened_code(syllable_codes, |_, _| true)
}

struct ExactTargetSegmentation {
    word_lengths: Vec<usize>,
    words: Vec<String>,
}

fn exact_target_segmentation(
    candidates: &[SentenceCandidate],
    target: &str,
    expected_syllables: usize,
) -> Option<ExactTargetSegmentation> {
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == target)?;
    let mut word_lengths = Vec::with_capacity(candidate.segments.len());
    let mut words = Vec::with_capacity(candidate.segments.len());
    let mut total_syllables = 0_usize;
    for segment in &candidate.segments {
        if segment.candidate.source != CandidateSource::Lexicon
            || !matches!(segment.candidate.correction, Correction::Exact)
            || !segment.candidate.spelling.abbreviated_syllables.is_empty()
            || segment.observed.as_str() != segment.candidate.code.as_str()
        {
            return None;
        }
        let syllables = segment.candidate.pinyin.split_whitespace().count();
        if syllables == 0 || segment.candidate.code.as_str().len() != syllables * 2 {
            return None;
        }
        total_syllables = total_syllables.checked_add(syllables)?;
        word_lengths.push(syllables);
        words.push(segment.candidate.text.clone());
    }
    (total_syllables == expected_syllables).then_some(ExactTargetSegmentation {
        word_lengths,
        words,
    })
}

fn word_tail_one_short(syllable_codes: &[KeySequence], word_lengths: &[usize]) -> Option<String> {
    word_shortened_code(syllable_codes, word_lengths, |index, length| {
        index + 1 == length
    })
}

fn word_tail_keep_singletons(
    syllable_codes: &[KeySequence],
    word_lengths: &[usize],
) -> Option<String> {
    word_code(syllable_codes, word_lengths, |index, length| {
        length > 1 && index + 1 == length
    })
}

fn word_head_anchored(syllable_codes: &[KeySequence], word_lengths: &[usize]) -> Option<String> {
    word_code(syllable_codes, word_lengths, |index, _| index > 0)
}

fn word_shortened_code(
    syllable_codes: &[KeySequence],
    word_lengths: &[usize],
    abbreviate: impl Fn(usize, usize) -> bool,
) -> Option<String> {
    let code = word_code(syllable_codes, word_lengths, abbreviate)?;
    let full_length = syllable_codes
        .iter()
        .map(|syllable| syllable.as_str().len())
        .sum::<usize>();
    (code.len() < full_length).then_some(code)
}

fn word_code(
    syllable_codes: &[KeySequence],
    word_lengths: &[usize],
    abbreviate: impl Fn(usize, usize) -> bool,
) -> Option<String> {
    if word_lengths.iter().sum::<usize>() != syllable_codes.len() {
        return None;
    }
    let mut code = String::new();
    let mut syllable_offset = 0_usize;
    for &word_length in word_lengths {
        if word_length == 0 {
            return None;
        }
        for index in 0..word_length {
            let syllable = &syllable_codes[syllable_offset + index];
            if abbreviate(index, word_length) {
                code.push(
                    syllable
                        .as_str()
                        .chars()
                        .next()
                        .expect("canonical syllable codes are nonempty"),
                );
            } else {
                code.push_str(syllable.as_str());
            }
        }
        syllable_offset += word_length;
    }
    Some(code)
}

fn shortened_code(
    syllable_codes: &[crate::KeySequence],
    abbreviate: impl Fn(usize, usize) -> bool,
) -> Option<String> {
    let mut code = String::new();
    for (index, syllable) in syllable_codes.iter().enumerate() {
        if abbreviate(index, syllable_codes.len()) {
            code.push(
                syllable
                    .as_str()
                    .chars()
                    .next()
                    .expect("canonical syllable codes are nonempty"),
            );
        } else {
            code.push_str(syllable.as_str());
        }
    }
    let full_length = syllable_codes
        .iter()
        .map(|syllable| syllable.as_str().len())
        .sum::<usize>();
    (code.len() < full_length).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::{
        CapsuleReplayReport, ContextReplayComparisonStats, PairedReplayStrategyStats,
        PersonalCacheKind, PersonalCacheReplayError, PersonalCacheReplayState, ReplayStrategyStats,
        effective_letter_code, observe_strategy_with_personal_cache,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, EventCapsuleV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput, parse_lexicon_tsv,
    };

    const LEXICON: &str = "\
text\tpinyin\tfrequency
我\two\t200
猫猫\tmao mao\t100
麻烦\tma fan\t90
再\tzai\t300
在\tzai\t100
";

    fn delta(deleted: &str, inserted: &str) -> TextDelta {
        delta_at(0, deleted, inserted)
    }

    fn delta_at(start: usize, deleted: &str, inserted: &str) -> TextDelta {
        TextDelta {
            start,
            deleted: deleted.to_owned(),
            inserted: inserted.to_owned(),
            position_evidence: DeltaPositionEvidence::UniqueText,
        }
    }

    fn commit_event(
        elapsed_ms: u64,
        start: usize,
        code: &str,
        composition: &str,
        target: &str,
    ) -> TimedTrackerOutput {
        let mut keys = code.chars().map(RawKey::Letter).collect::<Vec<_>>();
        keys.push(RawKey::Space);
        TimedTrackerOutput {
            elapsed_ms,
            output: TrackerOutput::Commit(CommitRecord {
                keys,
                keys_complete: true,
                composition: composition.to_owned(),
                change: delta_at(start, composition, target),
                document_change: delta_at(start, "", target),
            }),
        }
    }

    fn revision_event(
        elapsed_ms: u64,
        start: usize,
        deleted: &str,
        inserted: &str,
    ) -> TimedTrackerOutput {
        TimedTrackerOutput {
            elapsed_ms,
            output: TrackerOutput::Revision(crate::RevisionRecord {
                keys: vec![RawKey::Backspace],
                keys_complete: true,
                change: delta_at(start, deleted, inserted),
            }),
        }
    }

    #[test]
    fn effective_code_applies_internal_edits_and_ignores_document_navigation_prefix() {
        assert_eq!(
            effective_letter_code(&[
                RawKey::Home,
                RawKey::Right,
                RawKey::Letter('m'),
                RawKey::Letter('k'),
                RawKey::Letter('m'),
                RawKey::Letter('j'),
                RawKey::Backspace,
                RawKey::Letter('k'),
                RawKey::Space,
            ]),
            Ok(Some("mkmk".to_owned()))
        );
    }

    #[test]
    fn paired_rank_stats_partition_visible_and_outside_top_ten_changes() {
        let mut stats = PairedReplayStrategyStats::default();
        for (baseline_rank, strategy_rank, strategy_code) in [
            (Some(1), Some(2), "abc"),
            (Some(2), Some(1), "abc"),
            (Some(2), Some(2), "abcd"),
            (Some(3), None, "abc"),
            (None, Some(3), "abc"),
            (None, None, "abcd"),
        ] {
            stats.observe("abcd", strategy_code, baseline_rank, strategy_rank);
        }

        assert_eq!(stats.comparisons, 6);
        assert_eq!(stats.shortened_codes, 4);
        assert_eq!(stats.unchanged_codes, 2);
        assert_eq!(stats.lengthened_codes, 0);
        assert_eq!(stats.baseline_input_keys, 24);
        assert_eq!(stats.strategy_input_keys, 20);
        assert_eq!(stats.input_keys_saved, 4);
        assert_eq!(stats.input_keys_added, 0);
        assert_eq!(stats.baseline_visible_at_10, 4);
        assert_eq!(stats.strategy_visible_at_10, 4);
        assert_eq!(stats.rank_improved, 2);
        assert_eq!(stats.rank_same, 1);
        assert_eq!(stats.rank_worsened, 2);
        assert_eq!(stats.both_outside_top_10, 1);
        assert_eq!(stats.dropped_from_top_10, 1);
        assert_eq!(stats.recovered_into_top_10, 1);
        assert_eq!(
            stats.rank_improved + stats.rank_same + stats.rank_worsened + stats.both_outside_top_10,
            stats.comparisons
        );
    }

    #[test]
    fn context_effect_only_compares_windows_with_both_visible_baselines() {
        let mut stats = ContextReplayComparisonStats::default();
        for ranks in [
            (Some(1), None, Some(1), Some(2)),
            (Some(1), Some(2), Some(1), None),
            (Some(2), Some(3), Some(2), Some(3)),
            (Some(1), Some(1), None, None),
            (None, None, Some(1), Some(1)),
            (None, None, None, None),
        ] {
            stats.observe(ranks.0, ranks.1, ranks.2, ranks.3);
        }

        assert_eq!(stats.windows, 6);
        assert_eq!(stats.baselines_both_visible_at_10, 3);
        assert_eq!(stats.unigram_baseline_only_visible_at_10, 1);
        assert_eq!(stats.context_baseline_only_visible_at_10, 1);
        assert_eq!(stats.neither_baseline_visible_at_10, 1);
        assert_eq!(stats.relative_degradation_reduced, 1);
        assert_eq!(stats.relative_degradation_same, 1);
        assert_eq!(stats.relative_degradation_increased, 1);
        assert_eq!(stats.unigram_drops_from_top_10, 1);
        assert_eq!(stats.context_drops_from_top_10, 1);
        assert_eq!(stats.drops_rescued_by_context, 1);
        assert_eq!(stats.new_drops_with_context, 1);
    }

    #[test]
    fn replay_reports_real_and_counterfactual_lanes_without_text() {
        let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 100,
            output: TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Space,
                ],
                keys_complete: true,
                composition: "mao'mao".to_owned(),
                change: delta("mao'mao", "猫猫"),
                document_change: delta("", "猫猫"),
            }),
        }])
        .unwrap();
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut report = CapsuleReplayReport::default();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.raw_existing.attempts, 1);
        assert_eq!(report.raw_existing.hits_at_1, 1);
        assert_eq!(report.raw_existing.rank_histogram_at_10[0], 1);
        assert_eq!(
            report.raw_existing.projected_actions_with_one_selection(),
            5
        );
        assert_eq!(report.recorded_logical_key_actions, 5);
        assert_eq!(report.canonical_full.input_keys, 4);
        assert_eq!(report.tail_one_short.input_keys, 3);
        assert_eq!(report.head_anchored.input_keys, 3);
        assert_eq!(report.all_short.input_keys, 2);
        assert_eq!(report.word_boundaries_available_commits, 1);
        assert_eq!(report.word_tail_one_short.input_keys, 3);
        assert_eq!(report.word_head_anchored.input_keys, 3);
        let line = report.terminal_line();
        assert!(line.contains("contains_text=false"));
        assert!(line.contains("raw_existing_rank_histogram_at_10=1:1"));
        assert!(line.contains("raw_existing_projected_actions_one_selection=5"));
        assert!(!line.contains("猫"));
        assert!(!line.contains("mao"));
    }

    #[test]
    fn replay_reports_exact_redacted_rank_distribution() {
        const AMBIGUOUS_LEXICON: &str = "\
text\tpinyin\tfrequency
毛毛\tmao mao\t200
猫猫\tmao mao\t100
";
        let capsule =
            EventCapsuleV1::new(vec![commit_event(100, 0, "mkmk", "mao'mao", "猫猫")]).unwrap();
        let decoder = crate::Decoder::new(parse_lexicon_tsv(AMBIGUOUS_LEXICON).unwrap());
        let mut report = CapsuleReplayReport::default();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.raw_existing.hits_at_1, 0);
        assert_eq!(report.raw_existing.hits_at_5, 1);
        assert_eq!(report.raw_existing.rank_histogram_at_10[1], 1);
        assert_eq!(
            report.raw_existing.rank_histogram_at_10.iter().sum::<u64>(),
            report.raw_existing.hits_at_10
        );
        let line = report.terminal_line();
        assert!(line.contains("raw_existing_rank_histogram_at_10=2:1"));
        assert!(!line.contains("猫"));
        assert!(!line.contains("毛"));
        assert!(!line.contains("mao"));
    }

    #[test]
    fn word_anchoring_restarts_at_each_exact_full_code_segment() {
        let mut keys = "mafj mkmk"
            .replace(' ', "")
            .chars()
            .map(RawKey::Letter)
            .collect::<Vec<_>>();
        keys.push(RawKey::Space);
        let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 100,
            output: TrackerOutput::Commit(CommitRecord {
                keys,
                keys_complete: true,
                composition: "ma'fan'mao'mao".to_owned(),
                change: delta("ma'fan'mao'mao", "麻烦猫猫"),
                document_change: delta("", "麻烦猫猫"),
            }),
        }])
        .unwrap();
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut report = CapsuleReplayReport::default();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.canonical_full.input_keys, 8);
        assert_eq!(report.head_anchored.input_keys, 5);
        assert_eq!(report.word_boundaries_available_commits, 1);
        assert_eq!(report.word_boundaries_unavailable_commits, 0);
        assert_eq!(report.word_tail_one_short.input_keys, 6);
        assert_eq!(report.word_head_anchored.input_keys, 6);
        assert_eq!(report.word_head_anchored.hits_at_1, 1);
    }

    #[test]
    fn continuous_window_joins_only_adjacent_append_commits_within_explicit_gap() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "mafj", "ma'fan", "麻烦"),
            commit_event(200, 2, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.window_eligible_commits, 2);
        assert_eq!(report.window_ineligible_commits, 0);
        assert_eq!(report.continuous_windows, 1);
        assert_eq!(report.continuous_window_commits, 2);
        assert_eq!(report.continuous_window_recorded_logical_key_actions, 10);
        assert_eq!(report.window_raw_joined.input_keys, 8);
        assert_eq!(report.window_raw_joined.hits_at_1, 1);
        assert_eq!(
            report
                .window_raw_joined
                .projected_actions_with_one_selection(),
            9
        );
        assert_eq!(report.window_word_head_anchored.input_keys, 6);
        assert_eq!(report.window_word_head_anchored.hits_at_1, 1);

        let too_slow = EventCapsuleV1::new(vec![
            commit_event(100, 0, "mafj", "ma'fan", "麻烦"),
            commit_event(5_101, 2, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut gap_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        gap_report.observe_capsule(&decoder, &too_slow).unwrap();
        assert_eq!(gap_report.continuous_windows, 0);

        let nonadjacent = EventCapsuleV1::new(vec![
            commit_event(100, 0, "mafj", "ma'fan", "麻烦"),
            commit_event(200, 3, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut position_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        position_report
            .observe_capsule(&decoder, &nonadjacent)
            .unwrap();
        assert_eq!(position_report.continuous_windows, 0);
    }

    #[test]
    fn conservative_word_tail_lane_keeps_singletons_and_compact_output_is_redacted() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "wo", "wo", "我"),
            commit_event(200, 1, "mafj", "ma'fan", "麻烦"),
            commit_event(300, 3, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.word_tail_one_short.attempts, 3);
        assert_eq!(report.word_tail_one_short.input_keys, 7);
        assert_eq!(report.word_tail_keep_singletons.attempts, 3);
        assert_eq!(report.word_tail_keep_singletons.input_keys, 8);
        assert_eq!(report.word_tail_keep_singletons.hits_at_1, 3);
        assert_eq!(report.continuous_window_recorded_logical_key_actions, 13);
        assert_eq!(report.window_word_tail_one_short.input_keys, 7);
        assert_eq!(report.window_word_tail_keep_singletons.input_keys, 8);
        assert_eq!(
            report
                .window_word_tail_keep_singletons
                .projected_actions_with_one_selection(),
            9
        );
        assert_eq!(report.window_word_tail_keep_singletons.hits_at_1, 1);
        assert_eq!(
            report.window_word_tail_keep_singletons_vs_full.comparisons,
            1
        );
        assert_eq!(
            report
                .window_word_tail_keep_singletons_vs_full
                .input_keys_saved,
            2
        );
        assert_eq!(report.window_word_tail_keep_singletons_vs_full.rank_same, 1);

        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 7);
        assert!(compact.contains("CAPSULE_REPLAY_COMPACT contains_text=false"));
        assert!(compact.contains("recorded_actions=13"));
        assert!(compact.contains("noncanonical_is_error=false"));
        assert!(compact.contains(
            "name=word_tail_keep_singletons comparisons=1 shortened=1 unchanged=0 \
             baseline_keys=10 strategy_keys=8 saved_keys=2"
        ));
        assert!(!compact.contains("我"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("麻烦"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn all_singleton_window_is_an_unchanged_paired_head_anchor_attempt() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "wo", "wo", "我"),
            commit_event(200, 1, "wo", "wo", "我"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.continuous_windows, 1);
        assert_eq!(report.window_word_head_anchored.attempts, 1);
        assert_eq!(report.window_word_head_anchored.input_keys, 4);
        assert_eq!(report.window_word_head_anchored_vs_full.comparisons, 1);
        assert_eq!(report.window_word_head_anchored_vs_full.unchanged_codes, 1);
        assert_eq!(report.window_word_head_anchored_vs_full.shortened_codes, 0);
        assert_eq!(report.window_word_head_anchored_vs_full.rank_same, 1);
        assert_eq!(report.window_word_head_anchored_vs_full.input_keys_saved, 0);
    }

    #[test]
    fn public_context_path_uses_the_same_windows_and_stays_redacted() {
        let entries = parse_lexicon_tsv(LEXICON).unwrap();
        let language_model = crate::BigramLanguageModel::from_token_sequences(
            &[vec!["麻烦".to_owned(), "猫猫".to_owned()]],
            &entries,
        )
        .unwrap();
        let log_frequency_total = entries
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>()
            .ln();
        let decoder = crate::Decoder::new(entries);
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "mafj", "ma'fan", "麻烦"),
            commit_event(200, 2, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report
            .observe_capsule_with_public_context(
                &decoder,
                &language_model,
                log_frequency_total,
                &capsule,
            )
            .unwrap();

        assert_eq!(report.continuous_windows, 1);
        assert_eq!(report.public_context_windows, 1);
        assert_eq!(report.public_context_window_canonical_full.attempts, 1);
        assert_eq!(
            report
                .word_tail_keep_singletons_context_effect
                .baselines_both_visible_at_10,
            1
        );
        assert_eq!(
            report
                .word_tail_keep_singletons_context_effect
                .relative_degradation_same,
            1
        );

        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 8);
        assert!(compact.contains("scope=window_unigram name=canonical_full"));
        assert!(compact.contains("scope=window_public_word_bigram name=canonical_full"));
        assert!(
            compact.contains(
                "CONTEXT_COMPARE context=public_word_bigram name=word_tail_keep_singletons"
            )
        );
        assert!(!compact.contains("麻烦"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn public_character_context_uses_the_frozen_pool_and_stays_redacted() {
        let entries = parse_lexicon_tsv(LEXICON).unwrap();
        let language_model =
            crate::CharacterBigramLanguageModel::from_text_sequences(&["麻烦猫猫".to_owned()])
                .unwrap();
        let decoder = crate::Decoder::new(entries);
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "mafj", "ma'fan", "麻烦"),
            commit_event(200, 2, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report
            .observe_capsule_with_public_character_context(&decoder, &language_model, &capsule)
            .unwrap();

        assert_eq!(report.continuous_windows, 1);
        assert_eq!(report.public_context_windows, 1);
        assert_eq!(report.public_context_window_canonical_full.attempts, 1);

        let terminal = report.terminal_line();
        assert!(terminal.contains("public_context_kind=character_bigram"));
        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 8);
        assert!(compact.contains("scope=window_unigram name=canonical_full"));
        assert!(compact.contains("scope=window_public_character_bigram name=canonical_full"));
        assert!(compact.contains(
            "CONTEXT_COMPARE context=public_character_bigram \
                 name=word_tail_keep_singletons"
        ));
        assert!(!compact.contains("麻烦"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn personal_cache_predicts_before_learning_and_never_debugs_words() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            commit_event(10_000, 3, "zl", "zai", "在"),
            commit_event(10_100, 4, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut state = PersonalCacheReplayState::new();
        report
            .observe_capsule_with_personal_cache(&decoder, &mut state, &capsule)
            .unwrap();

        assert_eq!(report.continuous_windows, 2);
        assert_eq!(report.personal_cache_windows, 2);
        assert_eq!(report.personal_cache_learning_commits, 4);
        assert_eq!(report.personal_cache_learning_word_tokens, 4);
        assert_eq!(report.personal_cache_retained_word_tokens, 4);
        assert_eq!(report.personal_cache_learned_word_types, 2);
        assert_eq!(report.personal_cache_learning_pair_sequences, 2);
        assert_eq!(report.personal_cache_learning_word_pairs, 2);
        assert_eq!(report.personal_cache_retained_word_pairs, 2);
        assert_eq!(report.personal_cache_learned_word_pair_types, 1);
        assert_eq!(report.personal_cache_reversed_commits, 0);
        assert_eq!(report.window_canonical_full.hits_at_1, 0);
        assert_eq!(report.personal_cache_window_canonical_full.attempts, 2);
        assert_eq!(report.personal_cache_window_canonical_full.hits_at_1, 1);
        assert_eq!(state.learned_word_types(), 2);
        assert_eq!(state.learned_word_tokens(), 4);
        assert_eq!(state.learned_word_pair_types(), 1);
        assert_eq!(state.learned_word_pairs(), 2);

        let debug = format!("{state:?}");
        assert!(debug.contains("debug_contains_text: false"));
        assert!(!debug.contains("在"));
        assert!(!debug.contains("猫"));
        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 8);
        assert!(compact.contains("scope=window_personal_word_cache name=canonical_full"));
        assert!(compact.contains(
            "CONTEXT_COMPARE context=personal_word_cache \
                 name=word_tail_keep_singletons"
        ));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn ordered_word_pair_evidence_adds_signal_without_raising_the_promotion_cap() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut state = PersonalCacheReplayState::new();
        state.learn_commit(0, 1, &["在".to_owned()]);
        state.learn_commit(1, 1, &["再".to_owned()]);
        state.learn_commit(2, 2, &["猫猫".to_owned()]);
        assert_eq!(
            state.learn_pair_sequence(0, 3, &["在".to_owned(), "猫猫".to_owned()]),
            1
        );

        let mut word_stats = ReplayStrategyStats::default();
        let word_rank = observe_strategy_with_personal_cache(
            &decoder,
            &state,
            PersonalCacheKind::WordFrequency,
            "zlmkmk",
            "在猫猫",
            &mut word_stats,
        )
        .unwrap();
        let mut pair_stats = ReplayStrategyStats::default();
        let pair_rank = observe_strategy_with_personal_cache(
            &decoder,
            &state,
            PersonalCacheKind::OrderedWordPairs,
            "zlmkmk",
            "在猫猫",
            &mut pair_stats,
        )
        .unwrap();

        assert_eq!(word_rank, Some(3));
        assert_eq!(pair_rank, Some(2));
        assert!(pair_rank < word_rank);
        assert_eq!(pair_stats.hits_at_5, 1);
    }

    #[test]
    fn personal_pair_cache_is_causal_and_has_distinct_redacted_labels() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            commit_event(10_000, 3, "zl", "zai", "在"),
            commit_event(10_100, 4, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut state = PersonalCacheReplayState::new();
        report
            .observe_capsule_with_personal_pair_cache(&decoder, &mut state, &capsule)
            .unwrap();

        assert_eq!(report.personal_cache_windows, 2);
        assert_eq!(report.window_canonical_full.hits_at_1, 0);
        assert_eq!(report.personal_cache_window_canonical_full.hits_at_1, 1);
        assert_eq!(report.personal_cache_learning_pair_sequences, 2);
        assert_eq!(report.personal_cache_learning_word_pairs, 2);

        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 8);
        assert!(compact.contains("personal_cache_kind=ordered_word_pairs"));
        assert!(compact.contains("scope=window_personal_pair_cache name=canonical_full"));
        assert!(compact.contains(
            "CONTEXT_COMPARE context=personal_word_pair_cache \
                 name=word_tail_keep_singletons"
        ));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn explicit_personal_history_trains_before_a_separate_evaluation_report() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut state = PersonalCacheReplayState::new();
        let mut history_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        history_report
            .observe_capsule_with_personal_pair_cache(&decoder, &mut state, &history)
            .unwrap();
        let mut evaluation_report =
            CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        evaluation_report.record_personal_cache_history(&history_report, &state);
        evaluation_report
            .observe_capsule_with_personal_pair_cache(&decoder, &mut state, &evaluation)
            .unwrap();

        assert_eq!(evaluation_report.capsules, 1);
        assert_eq!(evaluation_report.personal_cache_history_capsules, 1);
        assert_eq!(evaluation_report.personal_cache_history_events, 2);
        assert_eq!(evaluation_report.personal_cache_history_learning_commits, 2);
        assert_eq!(evaluation_report.personal_cache_history_word_tokens, 2);
        assert_eq!(evaluation_report.personal_cache_history_word_types, 2);
        assert_eq!(evaluation_report.personal_cache_history_word_pairs, 1);
        assert_eq!(evaluation_report.personal_cache_history_word_pair_types, 1);
        assert_eq!(evaluation_report.window_canonical_full.hits_at_1, 0);
        assert_eq!(
            evaluation_report
                .personal_cache_window_canonical_full
                .hits_at_1,
            1
        );

        let compact = evaluation_report.compact_terminal_report();
        assert!(compact.contains("personal_cache_history_capsules=1"));
        assert!(compact.contains("personal_cache_history_repeated_word_pairs=0"));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn personal_cache_requires_an_explicit_window_gap() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![commit_event(100, 0, "zl", "zai", "在")]).unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(None).unwrap();
        let mut state = PersonalCacheReplayState::new();

        assert_eq!(
            report.observe_capsule_with_personal_cache(&decoder, &mut state, &capsule),
            Err(PersonalCacheReplayError::MissingWindowGap)
        );
        assert_eq!(report.capsules, 0);
        assert_eq!(state.learned_word_tokens(), 0);
    }

    #[test]
    fn personal_cache_reverses_exact_and_partial_document_edits() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            revision_event(300, 0, "在", ""),
            revision_event(400, 1, "猫", ""),
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut state = PersonalCacheReplayState::new();
        report
            .observe_capsule_with_personal_cache(&decoder, &mut state, &capsule)
            .unwrap();

        assert_eq!(report.personal_cache_learning_commits, 2);
        assert_eq!(report.personal_cache_learning_word_tokens, 2);
        assert_eq!(report.personal_cache_reversed_commits, 2);
        assert_eq!(report.personal_cache_reversed_word_tokens, 2);
        assert_eq!(report.personal_cache_reversed_pair_sequences, 1);
        assert_eq!(report.personal_cache_reversed_word_pairs, 1);
        assert_eq!(report.personal_cache_revision_events_with_reversal, 2);
        assert_eq!(report.personal_cache_revisions_not_reversed, 0);
        assert_eq!(report.personal_cache_retained_word_tokens, 0);
        assert_eq!(report.personal_cache_learned_word_types, 0);
        assert_eq!(state.learned_word_tokens(), 0);
        assert_eq!(state.learned_word_types(), 0);
        assert_eq!(state.learned_word_pairs(), 0);
        assert_eq!(state.learned_word_pair_types(), 0);
    }

    #[test]
    fn personal_cache_keeps_history_but_clears_document_spans_between_capsules() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let first = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let second = EventCapsuleV1::new(vec![revision_event(100, 0, "在", "")]).unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut state = PersonalCacheReplayState::new();
        report
            .observe_capsule_with_personal_cache(&decoder, &mut state, &first)
            .unwrap();
        report
            .observe_capsule_with_personal_cache(&decoder, &mut state, &second)
            .unwrap();

        assert_eq!(report.personal_cache_reversed_commits, 0);
        assert_eq!(report.personal_cache_revision_events_with_reversal, 0);
        assert_eq!(report.personal_cache_revisions_not_reversed, 1);
        assert_eq!(report.personal_cache_retained_word_tokens, 2);
        assert_eq!(report.personal_cache_retained_word_pairs, 1);
        assert_eq!(state.learned_word_tokens(), 2);
        assert_eq!(state.learned_word_types(), 2);
        assert_eq!(state.learned_word_pairs(), 1);
        assert_eq!(state.learned_word_pair_types(), 1);
    }

    #[test]
    fn personal_cache_does_not_guess_at_ambiguous_edit_positions() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut ambiguous_revision = revision_event(200, 0, "在", "");
        let TrackerOutput::Revision(revision) = &mut ambiguous_revision.output else {
            unreachable!("test helper created a revision");
        };
        revision.change.position_evidence = DeltaPositionEvidence::Ambiguous;
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            ambiguous_revision,
        ])
        .unwrap();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut state = PersonalCacheReplayState::new();
        report
            .observe_capsule_with_personal_cache(&decoder, &mut state, &capsule)
            .unwrap();

        assert_eq!(report.personal_cache_reversed_commits, 0);
        assert_eq!(report.personal_cache_revisions_not_reversed, 1);
        assert_eq!(report.personal_cache_ambiguous_edits_not_applied, 1);
        assert_eq!(report.personal_cache_retained_word_tokens, 1);
        assert_eq!(state.learned_word_tokens(), 1);
    }
}
