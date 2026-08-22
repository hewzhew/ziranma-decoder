//! Redacted offline replay metrics over explicitly loaded private capsules.
//!
//! This module receives an already parsed capsule and never performs I/O. It
//! compares bounded commit records with a caller-supplied decoder and returns
//! aggregate counts only.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

use crate::{
    BIGRAM_INTERPOLATION_WEIGHT, BigramLanguageModel, CandidateSource,
    CharacterBigramLanguageModel, Correction, Decoder, EventCapsuleV1, KeySequence,
    KeySequenceError, MAX_PERSONAL_CONTEXT_CODE_KEYS, MAX_PERSONAL_CONTEXT_ENTRIES,
    MAX_PERSONAL_CONTEXT_TEXT_CHARACTERS, PERSONAL_CONTEXT_SEARCH_DEPTH,
    PERSONAL_CONTEXT_SUPPORT_CAP, RawKey, SentenceCandidate, TrackerOutput, encode_pinyin_phrase,
};

pub const MAX_REPLAY_CODE_KEYS: usize = 64;
const REPLAY_TOP_K: usize = 10;
pub const PUBLIC_CONTEXT_REPLAY_POOL_DEPTH: usize = 50;
const PERSONAL_CACHE_MAX_PROMOTION: usize = 3;
const PERSONAL_PAIR_RESERVED_WORD_PROMOTION: usize = PERSONAL_CACHE_MAX_PROMOTION - 1;
const PERSONAL_PAIR_ONCE_MIN_COUNT: u64 = 1;
const PERSONAL_PAIR_REPEATED_MIN_COUNT: u64 = 2;
// A candidate below this frozen prefix cannot enter the visible Top-K:
// even after the maximum promotion, its adjusted rank is at least K, while
// the original first K candidates always have adjusted ranks below K.
const PERSONAL_CACHE_POOL_DEPTH: usize = REPLAY_TOP_K + PERSONAL_CACHE_MAX_PROMOTION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersonalCacheKind {
    WordFrequency,
    OrderedWordPairs,
    ExactCodeText,
}

#[derive(Clone, Copy)]
enum PersonalCacheWindowMode<'a> {
    Causal(PersonalCacheKind),
    FrozenAndCausal {
        kind: PersonalCacheKind,
        frozen_state: &'a PersonalCacheReplayState,
    },
    FrozenCausalAndCode {
        kind: PersonalCacheKind,
        frozen_state: &'a PersonalCacheReplayState,
    },
    FrozenWordPairAndCausalPair {
        frozen_state: &'a PersonalCacheReplayState,
    },
}

impl<'a> PersonalCacheWindowMode<'a> {
    fn kind(self) -> PersonalCacheKind {
        match self {
            Self::Causal(kind)
            | Self::FrozenAndCausal { kind, .. }
            | Self::FrozenCausalAndCode { kind, .. } => kind,
            Self::FrozenWordPairAndCausalPair { .. } => PersonalCacheKind::OrderedWordPairs,
        }
    }

    fn frozen_state(self) -> Option<&'a PersonalCacheReplayState> {
        match self {
            Self::Causal(_) => None,
            Self::FrozenAndCausal { frozen_state, .. }
            | Self::FrozenCausalAndCode { frozen_state, .. }
            | Self::FrozenWordPairAndCausalPair { frozen_state } => Some(frozen_state),
        }
    }

    fn includes_code_comparison(self) -> bool {
        matches!(self, Self::FrozenCausalAndCode { .. })
    }

    fn includes_pair_comparison(self) -> bool {
        matches!(self, Self::FrozenWordPairAndCausalPair { .. })
    }
}

impl PersonalCacheKind {
    fn terminal_label(self) -> &'static str {
        match self {
            Self::WordFrequency => "word_frequency",
            Self::OrderedWordPairs => "ordered_word_pairs",
            Self::ExactCodeText => "exact_code_text",
        }
    }

    fn compact_scope(self) -> &'static str {
        match self {
            Self::WordFrequency => "window_personal_word_cache",
            Self::OrderedWordPairs => "window_personal_pair_cache",
            Self::ExactCodeText => "window_personal_code_cache",
        }
    }

    fn compact_context_label(self) -> &'static str {
        match self {
            Self::WordFrequency => "personal_word_cache",
            Self::OrderedWordPairs => "personal_word_pair_cache",
            Self::ExactCodeText => "personal_code_cache",
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
    code_text_counts: HashMap<(String, String), u64>,
    learned_code_text_tokens: u64,
    active_code_text_spans: Vec<PersonalLearnedCodeTextSpan>,
    left_context_counts: BTreeMap<(String, String, String), ReplayLeftContextEvidence>,
    left_context_generation: u64,
    learned_left_context_tokens: u64,
    active_left_context_spans: Vec<PersonalLearnedLeftContextSpan>,
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

struct PersonalLearnedCodeTextSpan {
    start: usize,
    end: usize,
    code: String,
    text: String,
}

#[derive(Clone, Default)]
struct ReplayLeftContextEvidence {
    selection_generations: Vec<u64>,
}

impl ReplayLeftContextEvidence {
    fn selections(&self) -> u64 {
        u64::try_from(self.selection_generations.len()).unwrap_or(u64::MAX)
    }

    fn effective_support(&self) -> u64 {
        self.selections().min(PERSONAL_CONTEXT_SUPPORT_CAP)
    }

    fn last_selection_generation(&self) -> u64 {
        self.selection_generations
            .last()
            .copied()
            .unwrap_or_default()
    }
}

struct PersonalLearnedLeftContextSpan {
    start: usize,
    end: usize,
    previous_text: String,
    code: String,
    selected_text: String,
    generation: u64,
}

#[derive(Clone, Copy, Default)]
struct PersonalCacheEditOutcome {
    invalidated_commits: u64,
    invalidated_word_tokens: u64,
    invalidated_pair_sequences: u64,
    invalidated_word_pairs: u64,
    invalidated_code_text_tokens: u64,
    invalidated_left_context_tokens: u64,
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

    pub fn learned_code_text_types(&self) -> usize {
        self.code_text_counts.len()
    }

    pub fn learned_code_text_tokens(&self) -> u64 {
        self.learned_code_text_tokens
    }

    pub fn learned_left_context_types(&self) -> usize {
        self.left_context_counts.len()
    }

    pub fn learned_left_context_tokens(&self) -> u64 {
        self.learned_left_context_tokens
    }

    pub fn fork_for_frozen_evaluation(&self) -> Self {
        Self {
            word_counts: self.word_counts.clone(),
            learned_word_tokens: self.learned_word_tokens,
            active_document_spans: Vec::new(),
            pair_counts: self.pair_counts.clone(),
            learned_word_pairs: self.learned_word_pairs,
            active_pair_spans: Vec::new(),
            code_text_counts: self.code_text_counts.clone(),
            learned_code_text_tokens: self.learned_code_text_tokens,
            active_code_text_spans: Vec::new(),
            left_context_counts: self.left_context_counts.clone(),
            left_context_generation: self.left_context_generation,
            learned_left_context_tokens: self.learned_left_context_tokens,
            active_left_context_spans: Vec::new(),
        }
    }

    fn start_document(&mut self) {
        self.active_document_spans.clear();
        self.active_pair_spans.clear();
        self.active_code_text_spans.clear();
        self.active_left_context_spans.clear();
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

    fn code_text_count(&self, code: &str, text: &str) -> u64 {
        self.code_text_counts
            .get(&(code.to_owned(), text.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    fn learn_code_text(&mut self, start: usize, end: usize, code: String, text: String) {
        let count = self
            .code_text_counts
            .entry((code.clone(), text.clone()))
            .or_insert(0);
        *count = count.saturating_add(1);
        self.learned_code_text_tokens = self.learned_code_text_tokens.saturating_add(1);
        self.active_code_text_spans
            .push(PersonalLearnedCodeTextSpan {
                start,
                end,
                code,
                text,
            });
    }

    fn learn_left_context(
        &mut self,
        start: usize,
        end: usize,
        previous_text: String,
        code: String,
        selected_text: String,
    ) -> bool {
        if !valid_replay_context_text(&previous_text)
            || !valid_replay_context_code(&code)
            || !valid_replay_context_text(&selected_text)
        {
            return false;
        }
        self.left_context_generation = self.left_context_generation.saturating_add(1);
        let generation = self.left_context_generation;
        self.left_context_counts
            .entry((previous_text.clone(), code.clone(), selected_text.clone()))
            .or_default()
            .selection_generations
            .push(generation);
        self.learned_left_context_tokens = self.learned_left_context_tokens.saturating_add(1);
        self.active_left_context_spans
            .push(PersonalLearnedLeftContextSpan {
                start,
                end,
                previous_text,
                code,
                selected_text,
                generation,
            });
        while self.left_context_counts.len() > MAX_PERSONAL_CONTEXT_ENTRIES {
            let oldest = self
                .left_context_counts
                .iter()
                .min_by_key(|(identity, evidence)| {
                    (evidence.last_selection_generation(), *identity)
                })
                .map(|(identity, _)| identity.clone())
                .expect("an over-capacity replay context table has one oldest entry");
            if let Some(evicted) = self.left_context_counts.remove(&oldest) {
                self.learned_left_context_tokens = self
                    .learned_left_context_tokens
                    .saturating_sub(evicted.selections());
            }
        }
        true
    }

    fn left_context_count(&self, previous_text: &str, code: &str, text: &str) -> u64 {
        self.left_context_counts
            .get(&(previous_text.to_owned(), code.to_owned(), text.to_owned()))
            .map(ReplayLeftContextEvidence::selections)
            .unwrap_or_default()
    }

    fn preferred_left_context_text<'a>(
        &'a self,
        previous_text: &str,
        code: &str,
        candidates: impl IntoIterator<Item = &'a str>,
    ) -> Option<&'a str> {
        if !valid_replay_context_text(previous_text) || !valid_replay_context_code(code) {
            return None;
        }
        let candidates = candidates.into_iter().collect::<Vec<_>>();
        let start = (previous_text.to_owned(), code.to_owned(), String::new());
        self.left_context_counts
            .range(start..)
            .take_while(|((entry_previous, entry_code, _), _)| {
                entry_previous == previous_text && entry_code == code
            })
            .filter(|((_, _, text), evidence)| {
                evidence.effective_support() > 0
                    && self.code_text_count(code, text) > 0
                    && candidates.iter().any(|candidate| *candidate == text)
            })
            .max_by(|((_, _, left_text), left), ((_, _, right_text), right)| {
                left.effective_support()
                    .cmp(&right.effective_support())
                    .then_with(|| {
                        left.last_selection_generation()
                            .cmp(&right.last_selection_generation())
                    })
                    .then_with(|| left.selections().cmp(&right.selections()))
                    .then_with(|| right_text.cmp(left_text))
            })
            .map(|((_, _, text), _)| text.as_str())
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

        let mut retained_code_text = Vec::with_capacity(self.active_code_text_spans.len());
        for mut span in std::mem::take(&mut self.active_code_text_spans) {
            let overlaps = if deleted_chars > 0 {
                span.start < edit_end && span.end > start
            } else {
                start > span.start && start < span.end
            };
            if overlaps {
                outcome.invalidated_code_text_tokens =
                    outcome.invalidated_code_text_tokens.saturating_add(1);
                self.unlearn_code_text(&span.code, &span.text);
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
            retained_code_text.push(span);
        }
        self.active_code_text_spans = retained_code_text;

        let mut retained_left_context = Vec::with_capacity(self.active_left_context_spans.len());
        for mut span in std::mem::take(&mut self.active_left_context_spans) {
            let overlaps = if deleted_chars > 0 {
                span.start < edit_end && span.end > start
            } else {
                start > span.start && start < span.end
            };
            if overlaps {
                if self.unlearn_left_context(
                    &span.previous_text,
                    &span.code,
                    &span.selected_text,
                    span.generation,
                ) {
                    outcome.invalidated_left_context_tokens =
                        outcome.invalidated_left_context_tokens.saturating_add(1);
                }
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
            retained_left_context.push(span);
        }
        self.active_left_context_spans = retained_left_context;
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

    fn unlearn_code_text(&mut self, code: &str, text: &str) {
        let identity = (code.to_owned(), text.to_owned());
        let remove = self
            .code_text_counts
            .get_mut(&identity)
            .is_some_and(|count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
        if remove {
            self.code_text_counts.remove(&identity);
        }
        self.learned_code_text_tokens = self.learned_code_text_tokens.saturating_sub(1);
    }

    fn unlearn_left_context(
        &mut self,
        previous_text: &str,
        code: &str,
        selected_text: &str,
        generation: u64,
    ) -> bool {
        let identity = (
            previous_text.to_owned(),
            code.to_owned(),
            selected_text.to_owned(),
        );
        let Some(evidence) = self.left_context_counts.get_mut(&identity) else {
            return false;
        };
        let Ok(index) = evidence.selection_generations.binary_search(&generation) else {
            return false;
        };
        evidence.selection_generations.remove(index);
        let remove = evidence.selection_generations.is_empty();
        if remove {
            self.left_context_counts.remove(&identity);
        }
        self.learned_left_context_tokens = self.learned_left_context_tokens.saturating_sub(1);
        true
    }
}

fn valid_replay_context_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_PERSONAL_CONTEXT_CODE_KEYS
        && code.as_bytes().iter().all(u8::is_ascii_lowercase)
}

fn valid_replay_context_text(text: &str) -> bool {
    !text.is_empty()
        && !text.contains('\0')
        && text.chars().count() <= MAX_PERSONAL_CONTEXT_TEXT_CHARACTERS
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
            .field("learned_code_text_types", &self.learned_code_text_types())
            .field("learned_code_text_tokens", &self.learned_code_text_tokens)
            .field("active_code_text_spans", &self.active_code_text_spans.len())
            .field(
                "learned_left_context_types",
                &self.learned_left_context_types(),
            )
            .field(
                "learned_left_context_tokens",
                &self.learned_left_context_tokens,
            )
            .field(
                "active_left_context_spans",
                &self.active_left_context_spans.len(),
            )
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
pub struct RankingReplayComparisonStats {
    pub comparisons: u64,
    pub baseline_visible_at_10: u64,
    pub reranked_visible_at_10: u64,
    pub gained_top_1: u64,
    pub lost_top_1: u64,
    pub rank_improved: u64,
    pub rank_same: u64,
    pub rank_worsened: u64,
    pub both_outside_top_10: u64,
    pub dropped_from_top_10: u64,
    pub recovered_into_top_10: u64,
}

impl RankingReplayComparisonStats {
    fn observe(&mut self, baseline_rank: Option<usize>, reranked_rank: Option<usize>) {
        self.comparisons = self.comparisons.saturating_add(1);
        if baseline_rank.is_some() {
            self.baseline_visible_at_10 = self.baseline_visible_at_10.saturating_add(1);
        }
        if reranked_rank.is_some() {
            self.reranked_visible_at_10 = self.reranked_visible_at_10.saturating_add(1);
        }
        if baseline_rank != Some(1) && reranked_rank == Some(1) {
            self.gained_top_1 = self.gained_top_1.saturating_add(1);
        }
        if baseline_rank == Some(1) && reranked_rank != Some(1) {
            self.lost_top_1 = self.lost_top_1.saturating_add(1);
        }
        match (baseline_rank, reranked_rank) {
            (Some(baseline), Some(reranked)) if reranked < baseline => {
                self.rank_improved = self.rank_improved.saturating_add(1);
            }
            (Some(baseline), Some(reranked)) if reranked == baseline => {
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
            "{name}_comparisons={} {name}_baseline_visible_at_10={} \
             {name}_reranked_visible_at_10={} {name}_gained_top_1={} \
             {name}_lost_top_1={} {name}_rank_improved={} {name}_rank_same={} \
             {name}_rank_worsened={} {name}_both_outside_top_10={} \
             {name}_dropped_from_top_10={} {name}_recovered_into_top_10={}",
            self.comparisons,
            self.baseline_visible_at_10,
            self.reranked_visible_at_10,
            self.gained_top_1,
            self.lost_top_1,
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
pub struct WindowExclusionStats {
    pub incomplete_keys: u64,
    pub key_interpretation_failure: u64,
    pub missing_letter_code: u64,
    pub code_over_limit: u64,
    pub composition_unencodable: u64,
    pub canonical_word_boundaries_unavailable: u64,
    pub ambiguous_position: u64,
    pub non_append_document_change: u64,
}

impl WindowExclusionStats {
    pub fn total(&self) -> u64 {
        self.incomplete_keys
            .saturating_add(self.key_interpretation_failure)
            .saturating_add(self.missing_letter_code)
            .saturating_add(self.code_over_limit)
            .saturating_add(self.composition_unencodable)
            .saturating_add(self.canonical_word_boundaries_unavailable)
            .saturating_add(self.ambiguous_position)
            .saturating_add(self.non_append_document_change)
    }

    fn observe(&mut self, reason: WindowExclusionReason) {
        let field = match reason {
            WindowExclusionReason::IncompleteKeys => &mut self.incomplete_keys,
            WindowExclusionReason::KeyInterpretationFailure => &mut self.key_interpretation_failure,
            WindowExclusionReason::MissingLetterCode => &mut self.missing_letter_code,
            WindowExclusionReason::CodeOverLimit => &mut self.code_over_limit,
            WindowExclusionReason::CompositionUnencodable => &mut self.composition_unencodable,
            WindowExclusionReason::CanonicalWordBoundariesUnavailable => {
                &mut self.canonical_word_boundaries_unavailable
            }
            WindowExclusionReason::AmbiguousPosition => &mut self.ambiguous_position,
            WindowExclusionReason::NonAppendDocumentChange => &mut self.non_append_document_change,
        };
        *field = field.saturating_add(1);
    }

    fn terminal_fields(&self) -> String {
        format!(
            "window_exclusion_incomplete_keys={} \
             window_exclusion_key_interpretation_failure={} \
             window_exclusion_missing_letter_code={} \
             window_exclusion_code_over_limit={} \
             window_exclusion_composition_unencodable={} \
             window_exclusion_canonical_word_boundaries_unavailable={} \
             window_exclusion_ambiguous_position={} \
             window_exclusion_non_append_document_change={} \
             window_exclusion_total={}",
            self.incomplete_keys,
            self.key_interpretation_failure,
            self.missing_letter_code,
            self.code_over_limit,
            self.composition_unencodable,
            self.canonical_word_boundaries_unavailable,
            self.ambiguous_position,
            self.non_append_document_change,
            self.total()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowExclusionReason {
    IncompleteKeys,
    KeyInterpretationFailure,
    MissingLetterCode,
    CodeOverLimit,
    CompositionUnencodable,
    CanonicalWordBoundariesUnavailable,
    AmbiguousPosition,
    NonAppendDocumentChange,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersonalLeftContextMovementStats {
    pub preferences: u64,
    pub already_first: u64,
    pub promotions: u64,
    pub target_promotions: u64,
    pub competing_promotions: u64,
}

impl PersonalLeftContextMovementStats {
    fn observe(&mut self, rerank: PersonalLeftContextRerankObservation, preferred_is_target: bool) {
        let Some(rank) = rerank.preferred_rank_after_exact else {
            return;
        };
        self.preferences = self.preferences.saturating_add(1);
        if rank == 0 {
            self.already_first = self.already_first.saturating_add(1);
            return;
        }
        self.promotions = self.promotions.saturating_add(1);
        if preferred_is_target {
            self.target_promotions = self.target_promotions.saturating_add(1);
        } else {
            self.competing_promotions = self.competing_promotions.saturating_add(1);
        }
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
    pub window_exclusions: WindowExclusionStats,
    pub isolated_eligible_commits: u64,
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
    pub public_context_canonical_full_vs_unigram: RankingReplayComparisonStats,
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
    pub personal_cache_history_code_text_tokens: u64,
    pub personal_cache_history_code_text_types: u64,
    pub personal_cache_history_left_context_tokens: u64,
    pub personal_cache_history_left_context_types: u64,
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
    pub personal_cache_learning_code_text_tokens: u64,
    pub personal_cache_retained_code_text_tokens: u64,
    pub personal_cache_learned_code_text_types: u64,
    pub personal_cache_reversed_code_text_tokens: u64,
    pub personal_cache_learning_left_context_tokens: u64,
    pub personal_cache_retained_left_context_tokens: u64,
    pub personal_cache_learned_left_context_types: u64,
    pub personal_cache_reversed_left_context_tokens: u64,
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
    pub personal_frozen_cache_windows: u64,
    pub personal_frozen_window_canonical_full: ReplayStrategyStats,
    pub personal_frozen_window_word_tail_one_short: ReplayStrategyStats,
    pub personal_frozen_window_word_tail_keep_singletons: ReplayStrategyStats,
    pub personal_frozen_window_word_head_anchored: ReplayStrategyStats,
    pub personal_frozen_window_word_tail_one_short_vs_full: PairedReplayStrategyStats,
    pub personal_frozen_window_word_tail_keep_singletons_vs_full: PairedReplayStrategyStats,
    pub personal_frozen_window_word_head_anchored_vs_full: PairedReplayStrategyStats,
    pub word_tail_one_short_personal_frozen_effect: ContextReplayComparisonStats,
    pub word_tail_keep_singletons_personal_frozen_effect: ContextReplayComparisonStats,
    pub word_head_anchored_personal_frozen_effect: ContextReplayComparisonStats,
    pub personal_code_cache_windows: u64,
    pub personal_code_frozen_window_canonical_full: ReplayStrategyStats,
    pub personal_code_causal_window_canonical_full: ReplayStrategyStats,
    pub personal_hybrid_window_canonical_full: ReplayStrategyStats,
    pub personal_hybrid_vs_frozen_word: RankingReplayComparisonStats,
    pub personal_code_frozen_vs_unigram: RankingReplayComparisonStats,
    pub personal_code_causal_vs_unigram: RankingReplayComparisonStats,
    pub personal_code_causal_vs_frozen: RankingReplayComparisonStats,
    pub personal_code_target_in_pool_windows: u64,
    pub personal_code_frozen_any_evidence_windows: u64,
    pub personal_code_frozen_target_evidence_windows: u64,
    pub personal_code_frozen_competing_evidence_windows: u64,
    pub personal_code_causal_any_evidence_windows: u64,
    pub personal_code_causal_target_evidence_windows: u64,
    pub personal_code_causal_competing_evidence_windows: u64,
    pub personal_hybrid_target_extra_promotion_windows: u64,
    pub personal_hybrid_target_word_cap_saturated_windows: u64,
    pub personal_pair_comparison_windows: u64,
    pub personal_pair_public_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_frozen_word_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_frozen_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_causal_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_frozen_vs_frozen_word: RankingReplayComparisonStats,
    pub personal_pair_causal_vs_frozen_word: RankingReplayComparisonStats,
    pub personal_pair_causal_vs_frozen_pair: RankingReplayComparisonStats,
    pub personal_pair_history_any_evidence_windows: u64,
    pub personal_pair_history_target_in_pool_windows: u64,
    pub personal_pair_history_target_evidence_windows: u64,
    pub personal_pair_history_target_extra_promotion_windows: u64,
    pub personal_pair_history_target_word_cap_saturated_windows: u64,
    pub personal_pair_history_competing_evidence_windows: u64,
    pub personal_pair_history_evidence_candidates: u64,
    pub personal_pair_reserved_once_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_reserved_repeated_window_canonical_full: ReplayStrategyStats,
    pub personal_pair_reserved_once_vs_frozen_word: RankingReplayComparisonStats,
    pub personal_pair_reserved_repeated_vs_frozen_word: RankingReplayComparisonStats,
    pub personal_pair_reserved_once_active_windows: u64,
    pub personal_pair_reserved_once_target_evidence_windows: u64,
    pub personal_pair_reserved_repeated_active_windows: u64,
    pub personal_pair_reserved_repeated_target_evidence_windows: u64,
    pub personal_left_context_comparison_commits: u64,
    pub personal_left_context_public: ReplayStrategyStats,
    pub personal_left_context_frozen_exact: ReplayStrategyStats,
    pub personal_left_context_causal_exact: ReplayStrategyStats,
    pub personal_left_context_frozen_context: ReplayStrategyStats,
    pub personal_left_context_causal_context: ReplayStrategyStats,
    pub personal_left_context_frozen_exact_vs_public: RankingReplayComparisonStats,
    pub personal_left_context_frozen_context_vs_exact: RankingReplayComparisonStats,
    pub personal_left_context_causal_context_vs_causal_exact: RankingReplayComparisonStats,
    pub personal_left_context_causal_context_vs_exact: RankingReplayComparisonStats,
    pub personal_left_context_causal_vs_frozen_context: RankingReplayComparisonStats,
    pub personal_left_context_target_in_pool_commits: u64,
    pub personal_left_context_frozen_any_evidence_commits: u64,
    pub personal_left_context_frozen_target_evidence_commits: u64,
    pub personal_left_context_frozen_competing_evidence_commits: u64,
    pub personal_left_context_causal_any_evidence_commits: u64,
    pub personal_left_context_causal_target_evidence_commits: u64,
    pub personal_left_context_causal_competing_evidence_commits: u64,
    pub personal_left_context_frozen_movement: PersonalLeftContextMovementStats,
    pub personal_left_context_causal_movement: PersonalLeftContextMovementStats,
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
        let _ = self.observe_capsule_internal(decoder, None, capsule, true)?;
        Ok(())
    }

    fn observe_window_exclusion(&mut self, reason: WindowExclusionReason) {
        if self.window_gap_limit_ms.is_some() {
            self.window_exclusions.observe(reason);
        }
    }

    pub fn observe_capsule_with_public_context(
        &mut self,
        decoder: &Decoder,
        public_language_model: &BigramLanguageModel,
        log_frequency_total: f64,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        let _ = self.observe_capsule_internal(
            decoder,
            Some(FrozenPublicContext::Word {
                language_model: public_language_model,
                log_frequency_total,
            }),
            capsule,
            true,
        )?;
        Ok(())
    }

    pub fn observe_capsule_with_public_character_context(
        &mut self,
        decoder: &Decoder,
        public_language_model: &CharacterBigramLanguageModel,
        capsule: &EventCapsuleV1,
    ) -> Result<(), KeySequenceError> {
        let _ = self.observe_capsule_internal(
            decoder,
            Some(FrozenPublicContext::Character {
                language_model: public_language_model,
            }),
            capsule,
            true,
        )?;
        Ok(())
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
            true,
        )
    }

    pub fn observe_capsule_with_compact_personal_cache(
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
            false,
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
            true,
        )
    }

    pub fn observe_capsule_with_compact_personal_pair_cache(
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
            false,
        )
    }

    pub fn observe_capsule_with_personal_word_comparison(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        let kind = PersonalCacheKind::WordFrequency;
        if let Some(previous) = self.personal_cache_kind {
            assert_eq!(
                previous, kind,
                "a replay report cannot mix personal cache models"
            );
        } else {
            self.personal_cache_kind = Some(kind);
        }
        let prepared_windows = self.observe_capsule_internal(decoder, None, capsule, false)?;
        self.observe_personal_cache_windows(
            decoder,
            causal_state,
            PersonalCacheWindowMode::FrozenAndCausal { kind, frozen_state },
            capsule,
            max_gap_ms,
            &prepared_windows,
        )?;
        self.personal_cache_learned_word_types =
            u64::try_from(causal_state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = causal_state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(causal_state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = causal_state.learned_word_pairs();
        self.personal_cache_learned_code_text_types =
            u64::try_from(causal_state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_code_text_tokens = causal_state.learned_code_text_tokens();
        Ok(())
    }

    pub fn observe_capsule_with_personal_code_comparison(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        let kind = PersonalCacheKind::WordFrequency;
        if let Some(previous) = self.personal_cache_kind {
            assert_eq!(
                previous, kind,
                "a replay report cannot mix personal cache models"
            );
        } else {
            self.personal_cache_kind = Some(kind);
        }
        let prepared_windows = self.observe_capsule_internal(decoder, None, capsule, false)?;
        self.observe_personal_cache_windows(
            decoder,
            causal_state,
            PersonalCacheWindowMode::FrozenCausalAndCode { kind, frozen_state },
            capsule,
            max_gap_ms,
            &prepared_windows,
        )?;
        self.personal_cache_learned_word_types =
            u64::try_from(causal_state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = causal_state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(causal_state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = causal_state.learned_word_pairs();
        self.personal_cache_learned_code_text_types =
            u64::try_from(causal_state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_code_text_tokens = causal_state.learned_code_text_tokens();
        Ok(())
    }

    pub fn observe_capsule_with_personal_left_context_comparison(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        let prepared_windows = self.observe_capsule_internal(decoder, None, capsule, false)?;
        self.observe_personal_left_context_capsule(
            decoder,
            frozen_state,
            causal_state,
            capsule,
            max_gap_ms,
            &prepared_windows,
        )?;
        self.update_personal_left_context_state_totals(causal_state);
        Ok(())
    }

    pub fn observe_capsule_with_personal_pair_comparison(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        let kind = PersonalCacheKind::OrderedWordPairs;
        if let Some(previous) = self.personal_cache_kind {
            assert_eq!(
                previous, kind,
                "a replay report cannot mix personal cache models"
            );
        } else {
            self.personal_cache_kind = Some(kind);
        }
        let prepared_windows = self.observe_capsule_internal(decoder, None, capsule, false)?;
        self.observe_personal_cache_windows(
            decoder,
            causal_state,
            PersonalCacheWindowMode::FrozenWordPairAndCausalPair { frozen_state },
            capsule,
            max_gap_ms,
            &prepared_windows,
        )?;
        self.personal_cache_learned_word_types =
            u64::try_from(causal_state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = causal_state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(causal_state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = causal_state.learned_word_pairs();
        Ok(())
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
        self.personal_cache_history_code_text_tokens = state.learned_code_text_tokens();
        self.personal_cache_history_code_text_types =
            u64::try_from(state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_history_left_context_tokens = state.learned_left_context_tokens();
        self.personal_cache_history_left_context_types =
            u64::try_from(state.learned_left_context_types()).unwrap_or(u64::MAX);
    }

    pub fn learn_capsule_for_personal_cache(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        self.capsules = self.capsules.saturating_add(1);
        self.events = self
            .events
            .saturating_add(u64::try_from(capsule.events().len()).unwrap_or(u64::MAX));
        self.learn_personal_cache_capsule(decoder, state, capsule, max_gap_ms, false)?;
        self.personal_cache_learned_word_types =
            u64::try_from(state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = state.learned_word_pairs();
        self.personal_cache_learned_code_text_types =
            u64::try_from(state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_code_text_tokens = state.learned_code_text_tokens();
        Ok(())
    }

    pub fn learn_capsule_for_personal_code_comparison(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        self.capsules = self.capsules.saturating_add(1);
        self.events = self
            .events
            .saturating_add(u64::try_from(capsule.events().len()).unwrap_or(u64::MAX));
        self.learn_personal_cache_capsule(decoder, state, capsule, max_gap_ms, true)?;
        self.personal_cache_learned_word_types =
            u64::try_from(state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = state.learned_word_pairs();
        self.personal_cache_learned_code_text_types =
            u64::try_from(state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_code_text_tokens = state.learned_code_text_tokens();
        Ok(())
    }

    pub fn learn_capsule_for_personal_left_context_comparison(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
    ) -> Result<(), PersonalCacheReplayError> {
        let Some(max_gap_ms) = self.window_gap_limit_ms else {
            return Err(PersonalCacheReplayError::MissingWindowGap);
        };
        self.capsules = self.capsules.saturating_add(1);
        self.events = self
            .events
            .saturating_add(u64::try_from(capsule.events().len()).unwrap_or(u64::MAX));
        self.learn_personal_left_context_capsule(decoder, state, capsule, max_gap_ms)?;
        self.update_personal_left_context_state_totals(state);
        Ok(())
    }

    fn update_personal_left_context_state_totals(&mut self, state: &PersonalCacheReplayState) {
        self.personal_cache_retained_code_text_tokens = state.learned_code_text_tokens();
        self.personal_cache_learned_code_text_types =
            u64::try_from(state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_left_context_tokens = state.learned_left_context_tokens();
        self.personal_cache_learned_left_context_types =
            u64::try_from(state.learned_left_context_types()).unwrap_or(u64::MAX);
    }

    fn observe_capsule_with_personal_cache_kind(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        kind: PersonalCacheKind,
        capsule: &EventCapsuleV1,
        collect_commit_strategies: bool,
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
        let prepared_windows =
            self.observe_capsule_internal(decoder, None, capsule, collect_commit_strategies)?;
        self.observe_personal_cache_windows(
            decoder,
            state,
            PersonalCacheWindowMode::Causal(kind),
            capsule,
            max_gap_ms,
            &prepared_windows,
        )?;
        self.personal_cache_learned_word_types =
            u64::try_from(state.learned_word_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_tokens = state.learned_word_tokens();
        self.personal_cache_learned_word_pair_types =
            u64::try_from(state.learned_word_pair_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_word_pairs = state.learned_word_pairs();
        self.personal_cache_learned_code_text_types =
            u64::try_from(state.learned_code_text_types()).unwrap_or(u64::MAX);
        self.personal_cache_retained_code_text_tokens = state.learned_code_text_tokens();
        Ok(())
    }

    fn observe_capsule_internal(
        &mut self,
        decoder: &Decoder,
        public_context: Option<FrozenPublicContext<'_>>,
        capsule: &EventCapsuleV1,
        collect_commit_strategies: bool,
    ) -> Result<HashMap<usize, WindowCommit>, KeySequenceError> {
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
                        self.observe_window_exclusion(WindowExclusionReason::IncompleteKeys);
                        continue;
                    }
                    let observed = match effective_letter_code(&record.keys) {
                        Ok(Some(observed)) => observed,
                        Ok(None) => {
                            self.commits_without_letter_code =
                                self.commits_without_letter_code.saturating_add(1);
                            self.observe_window_exclusion(WindowExclusionReason::MissingLetterCode);
                            continue;
                        }
                        Err(_) => {
                            self.key_interpretation_failures =
                                self.key_interpretation_failures.saturating_add(1);
                            self.observe_window_exclusion(
                                WindowExclusionReason::KeyInterpretationFailure,
                            );
                            continue;
                        }
                    };
                    if observed.len() > MAX_REPLAY_CODE_KEYS {
                        self.commits_over_key_limit = self.commits_over_key_limit.saturating_add(1);
                        self.observe_window_exclusion(WindowExclusionReason::CodeOverLimit);
                        continue;
                    }

                    let mut commit_decode_cache = HashMap::<String, Vec<SentenceCandidate>>::new();
                    let target = &record.change.inserted;
                    if collect_commit_strategies {
                        let candidates = decoder.decode_sentence(&observed, REPLAY_TOP_K)?;
                        let _ = self.raw_existing.observe(&observed, target, &candidates);
                        commit_decode_cache.insert(observed.clone(), candidates);
                    }

                    let normalized_pinyin = record.composition.replace('\'', " ");
                    let Ok(encoded) = encode_pinyin_phrase(&normalized_pinyin) else {
                        self.observe_window_exclusion(
                            WindowExclusionReason::CompositionUnencodable,
                        );
                        continue;
                    };
                    self.composition_encodable_commits =
                        self.composition_encodable_commits.saturating_add(1);
                    if observed == encoded.full_code.as_str() {
                        self.observed_matches_canonical =
                            self.observed_matches_canonical.saturating_add(1);
                    }

                    let canonical_candidates = if collect_commit_strategies {
                        decode_sentence_memoized(
                            decoder,
                            &mut commit_decode_cache,
                            encoded.full_code.as_str(),
                        )?
                    } else {
                        decoder.decode_sentence(encoded.full_code.as_str(), REPLAY_TOP_K)?
                    };
                    if collect_commit_strategies {
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
                    }
                    if let Some(segmentation) = exact_target_segmentation(
                        &canonical_candidates,
                        target,
                        encoded.syllable_codes.len(),
                    ) {
                        self.word_boundaries_available_commits =
                            self.word_boundaries_available_commits.saturating_add(1);
                        if collect_commit_strategies {
                            if let Some(code) = word_tail_one_short(
                                &encoded.syllable_codes,
                                &segmentation.word_lengths,
                            ) {
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
                            if let Some(code) = word_head_anchored(
                                &encoded.syllable_codes,
                                &segmentation.word_lengths,
                            ) {
                                observe_strategy_memoized(
                                    decoder,
                                    &mut commit_decode_cache,
                                    &code,
                                    target,
                                    &mut self.word_head_anchored,
                                )?;
                            }
                        }
                        if self.window_gap_limit_ms.is_some() {
                            if let Some(reason) = window_document_exclusion(record) {
                                self.observe_window_exclusion(reason);
                            } else {
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
                                        recorded_logical_key_actions: saturating_len(
                                            record.keys.len(),
                                        ),
                                    },
                                );
                            }
                        }
                    } else {
                        self.word_boundaries_unavailable_commits =
                            self.word_boundaries_unavailable_commits.saturating_add(1);
                        self.observe_window_exclusion(
                            WindowExclusionReason::CanonicalWordBoundariesUnavailable,
                        );
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
                &prepared_windows,
            )?;
            debug_assert_eq!(
                self.window_ineligible_commits,
                self.window_exclusions.total()
            );
            debug_assert_eq!(
                self.window_eligible_commits,
                self.continuous_window_commits
                    .saturating_add(self.isolated_eligible_commits)
            );
        }
        Ok(prepared_windows)
    }

    fn observe_continuous_windows(
        &mut self,
        decoder: &Decoder,
        public_context: Option<FrozenPublicContext<'_>>,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
        prepared_windows: &HashMap<usize, WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        let mut run = Vec::<WindowCommit>::new();
        for (event_index, event) in capsule.events().iter().enumerate() {
            let TrackerOutput::Commit(_) = &event.output else {
                self.finish_window(decoder, public_context, &mut run)?;
                continue;
            };
            let Some(commit) = prepared_windows.get(&event_index).cloned() else {
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
        mode: PersonalCacheWindowMode<'_>,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
        prepared_windows: &HashMap<usize, WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        state.start_document();
        let mut run = Vec::<WindowCommit>::new();
        for (event_index, event) in capsule.events().iter().enumerate() {
            let TrackerOutput::Commit(record) = &event.output else {
                self.finish_personal_cache_window(decoder, state, mode, &mut run)?;
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
            let Some(commit) = prepared_windows.get(&event_index).cloned() else {
                self.finish_personal_cache_window(decoder, state, mode, &mut run)?;
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
                self.finish_personal_cache_window(decoder, state, mode, &mut run)?;
            }
            run.push(commit);
        }
        self.finish_personal_cache_window(decoder, state, mode, &mut run)
    }

    fn observe_personal_left_context_capsule(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
        prepared_windows: &HashMap<usize, WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        causal_state.start_document();
        let mut run = Vec::<WindowCommit>::new();
        for (event_index, event) in capsule.events().iter().enumerate() {
            let TrackerOutput::Commit(record) = &event.output else {
                self.finish_personal_left_context_run(
                    decoder,
                    frozen_state,
                    causal_state,
                    &mut run,
                )?;
                let TrackerOutput::Revision(revision) = &event.output else {
                    unreachable!("tracker output has only commit and revision variants");
                };
                let outcome = causal_state.apply_document_delta(
                    revision.change.start,
                    revision.change.deleted.chars().count(),
                    revision.change.inserted.chars().count(),
                    revision.change.position_evidence,
                );
                self.observe_personal_cache_edit(outcome, true);
                continue;
            };
            let Some(commit) = prepared_windows.get(&event_index).cloned() else {
                self.finish_personal_left_context_run(
                    decoder,
                    frozen_state,
                    causal_state,
                    &mut run,
                )?;
                let outcome = causal_state.apply_document_delta(
                    record.document_change.start,
                    record.document_change.deleted.chars().count(),
                    record.document_change.inserted.chars().count(),
                    record.document_change.position_evidence,
                );
                self.observe_personal_cache_edit(outcome, false);
                continue;
            };
            let joins_previous = run.last().is_some_and(|previous| {
                commit.elapsed_ms.saturating_sub(previous.elapsed_ms) <= max_gap_ms
                    && commit.document_start == previous.document_end()
            });
            if !run.is_empty() && !joins_previous {
                self.finish_personal_left_context_run(
                    decoder,
                    frozen_state,
                    causal_state,
                    &mut run,
                )?;
            }
            run.push(commit);
        }
        self.finish_personal_left_context_run(decoder, frozen_state, causal_state, &mut run)
    }

    fn learn_personal_left_context_capsule(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
    ) -> Result<(), KeySequenceError> {
        state.start_document();
        let mut run = Vec::<WindowCommit>::new();
        for event in capsule.events() {
            let TrackerOutput::Commit(record) = &event.output else {
                self.finish_personal_left_context_learning(state, &mut run);
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
                self.finish_personal_left_context_learning(state, &mut run);
                let outcome = state.apply_document_delta(
                    record.document_change.start,
                    record.document_change.deleted.chars().count(),
                    record.document_change.inserted.chars().count(),
                    record.document_change.position_evidence,
                );
                self.observe_personal_cache_edit(outcome, false);
                continue;
            };
            let joins_previous = run.last().is_some_and(|previous| {
                commit.elapsed_ms.saturating_sub(previous.elapsed_ms) <= max_gap_ms
                    && commit.document_start == previous.document_end()
            });
            if !run.is_empty() && !joins_previous {
                self.finish_personal_left_context_learning(state, &mut run);
            }
            run.push(commit);
        }
        self.finish_personal_left_context_learning(state, &mut run);
        Ok(())
    }

    fn finish_personal_left_context_run(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &mut PersonalCacheReplayState,
        run: &mut Vec<WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        let mut previous = None::<WindowCommit>;
        for commit in run.iter() {
            if let Some(previous_commit) = previous.as_ref() {
                self.observe_personal_left_context_commit(
                    decoder,
                    frozen_state,
                    causal_state,
                    previous_commit,
                    commit,
                )?;
            }
            self.learn_personal_left_context_commit(causal_state, previous.as_ref(), commit);
            previous = Some(commit.clone());
        }
        run.clear();
        Ok(())
    }

    fn finish_personal_left_context_learning(
        &mut self,
        state: &mut PersonalCacheReplayState,
        run: &mut Vec<WindowCommit>,
    ) {
        let mut previous = None::<WindowCommit>;
        for commit in run.iter() {
            self.learn_personal_left_context_commit(state, previous.as_ref(), commit);
            previous = Some(commit.clone());
        }
        run.clear();
    }

    fn learn_personal_left_context_commit(
        &mut self,
        state: &mut PersonalCacheReplayState,
        previous: Option<&WindowCommit>,
        commit: &WindowCommit,
    ) {
        let outcome = state.apply_document_delta(
            commit.document_start,
            0,
            commit.document_inserted_chars,
            crate::DeltaPositionEvidence::UniqueText,
        );
        self.observe_personal_cache_edit(outcome, false);
        state.learn_code_text(
            commit.document_start,
            commit.document_end(),
            commit.observed.clone(),
            commit.target.clone(),
        );
        self.personal_cache_learning_code_text_tokens = self
            .personal_cache_learning_code_text_tokens
            .saturating_add(1);
        if let Some(previous) = previous
            && state.learn_left_context(
                previous.document_start,
                commit.document_end(),
                previous.target.clone(),
                commit.observed.clone(),
                commit.target.clone(),
            )
        {
            self.personal_cache_learning_left_context_tokens = self
                .personal_cache_learning_left_context_tokens
                .saturating_add(1);
        }
    }

    fn observe_personal_left_context_commit(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &PersonalCacheReplayState,
        previous: &WindowCommit,
        commit: &WindowCommit,
    ) -> Result<(), KeySequenceError> {
        let pool_depth = PERSONAL_CACHE_POOL_DEPTH.max(PERSONAL_CONTEXT_SEARCH_DEPTH);
        let pool = decoder.decode_sentence(&commit.canonical_full, pool_depth)?;
        let public_rank = self.personal_left_context_public.observe(
            &commit.observed,
            &commit.target,
            &pool[..pool.len().min(REPLAY_TOP_K)],
        );
        let frozen_exact_candidates = personal_ranked_candidates_from_pool(
            &pool,
            frozen_state,
            PersonalCacheKind::ExactCodeText,
            &commit.observed,
        );
        let frozen_exact_rank = self.personal_left_context_frozen_exact.observe(
            &commit.observed,
            &commit.target,
            &frozen_exact_candidates[..frozen_exact_candidates.len().min(REPLAY_TOP_K)],
        );
        let causal_exact_candidates = personal_ranked_candidates_from_pool(
            &pool,
            causal_state,
            PersonalCacheKind::ExactCodeText,
            &commit.observed,
        );
        let causal_exact_rank = self.personal_left_context_causal_exact.observe(
            &commit.observed,
            &commit.target,
            &causal_exact_candidates[..causal_exact_candidates.len().min(REPLAY_TOP_K)],
        );
        let (frozen_context_candidates, frozen_context_rerank) =
            personal_left_context_candidates_from_pool(
                &pool,
                frozen_state,
                frozen_state,
                &previous.target,
                &commit.observed,
            );
        self.personal_left_context_frozen_movement.observe(
            frozen_context_rerank,
            frozen_context_candidates
                .first()
                .is_some_and(|candidate| candidate.text == commit.target),
        );
        let frozen_context_rank = self.personal_left_context_frozen_context.observe(
            &commit.observed,
            &commit.target,
            &frozen_context_candidates[..frozen_context_candidates.len().min(REPLAY_TOP_K)],
        );
        let (causal_context_candidates, causal_context_rerank) =
            personal_left_context_candidates_from_pool(
                &pool,
                causal_state,
                causal_state,
                &previous.target,
                &commit.observed,
            );
        self.personal_left_context_causal_movement.observe(
            causal_context_rerank,
            causal_context_candidates
                .first()
                .is_some_and(|candidate| candidate.text == commit.target),
        );
        let causal_context_rank = self.personal_left_context_causal_context.observe(
            &commit.observed,
            &commit.target,
            &causal_context_candidates[..causal_context_candidates.len().min(REPLAY_TOP_K)],
        );

        self.personal_left_context_comparison_commits = self
            .personal_left_context_comparison_commits
            .saturating_add(1);
        self.personal_left_context_frozen_exact_vs_public
            .observe(public_rank, frozen_exact_rank);
        self.personal_left_context_frozen_context_vs_exact
            .observe(frozen_exact_rank, frozen_context_rank);
        self.personal_left_context_causal_context_vs_causal_exact
            .observe(causal_exact_rank, causal_context_rank);
        self.personal_left_context_causal_context_vs_exact
            .observe(frozen_exact_rank, causal_context_rank);
        self.personal_left_context_causal_vs_frozen_context
            .observe(frozen_context_rank, causal_context_rank);

        let target_index = pool
            .iter()
            .position(|candidate| candidate.text == commit.target);
        if target_index.is_some() {
            self.personal_left_context_target_in_pool_commits = self
                .personal_left_context_target_in_pool_commits
                .saturating_add(1);
        }
        let search_pool = &pool[..pool.len().min(PERSONAL_CONTEXT_SEARCH_DEPTH)];
        let target_in_search_pool = search_pool
            .iter()
            .any(|candidate| candidate.text == commit.target);
        let mut frozen_any = false;
        let mut frozen_competing = false;
        let mut causal_any = false;
        let mut causal_competing = false;
        for candidate in search_pool {
            let is_target = candidate.text == commit.target;
            if frozen_state.left_context_count(&previous.target, &commit.observed, &candidate.text)
                > 0
                && frozen_state.code_text_count(&commit.observed, &candidate.text) > 0
            {
                frozen_any = true;
                frozen_competing |= !is_target;
            }
            if causal_state.left_context_count(&previous.target, &commit.observed, &candidate.text)
                > 0
                && causal_state.code_text_count(&commit.observed, &candidate.text) > 0
            {
                causal_any = true;
                causal_competing |= !is_target;
            }
        }
        self.personal_left_context_frozen_any_evidence_commits = self
            .personal_left_context_frozen_any_evidence_commits
            .saturating_add(u64::from(frozen_any));
        self.personal_left_context_frozen_competing_evidence_commits = self
            .personal_left_context_frozen_competing_evidence_commits
            .saturating_add(u64::from(frozen_competing));
        self.personal_left_context_causal_any_evidence_commits = self
            .personal_left_context_causal_any_evidence_commits
            .saturating_add(u64::from(causal_any));
        self.personal_left_context_causal_competing_evidence_commits = self
            .personal_left_context_causal_competing_evidence_commits
            .saturating_add(u64::from(causal_competing));
        if target_in_search_pool
            && frozen_state.left_context_count(&previous.target, &commit.observed, &commit.target)
                > 0
            && frozen_state.code_text_count(&commit.observed, &commit.target) > 0
        {
            self.personal_left_context_frozen_target_evidence_commits = self
                .personal_left_context_frozen_target_evidence_commits
                .saturating_add(1);
        }
        if target_in_search_pool
            && causal_state.left_context_count(&previous.target, &commit.observed, &commit.target)
                > 0
            && causal_state.code_text_count(&commit.observed, &commit.target) > 0
        {
            self.personal_left_context_causal_target_evidence_commits = self
                .personal_left_context_causal_target_evidence_commits
                .saturating_add(1);
        }
        Ok(())
    }

    fn learn_personal_cache_capsule(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        capsule: &EventCapsuleV1,
        max_gap_ms: u64,
        learn_code_text: bool,
    ) -> Result<(), KeySequenceError> {
        state.start_document();
        let mut run = Vec::<WindowCommit>::new();
        for event in capsule.events() {
            let TrackerOutput::Commit(record) = &event.output else {
                self.finish_personal_cache_learning(state, &mut run, learn_code_text);
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
                self.finish_personal_cache_learning(state, &mut run, learn_code_text);
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
                self.finish_personal_cache_learning(state, &mut run, learn_code_text);
            }
            run.push(commit);
        }
        self.finish_personal_cache_learning(state, &mut run, learn_code_text);
        Ok(())
    }

    fn finish_personal_cache_window(
        &mut self,
        decoder: &Decoder,
        state: &mut PersonalCacheReplayState,
        mode: PersonalCacheWindowMode<'_>,
        run: &mut Vec<WindowCommit>,
    ) -> Result<(), KeySequenceError> {
        if run.len() >= 2 {
            if let Some(frozen_state) = mode.frozen_state() {
                if mode.includes_pair_comparison() {
                    self.observe_personal_pair_comparison_window(
                        decoder,
                        frozen_state,
                        state,
                        run,
                    )?;
                } else {
                    self.observe_personal_cache_comparison_window(
                        decoder,
                        frozen_state,
                        state,
                        mode.kind(),
                        run,
                        mode.includes_code_comparison(),
                    )?;
                }
            } else {
                self.observe_personal_cache_window(decoder, state, mode.kind(), run)?;
            }
        }
        self.finish_personal_cache_learning(state, run, mode.includes_code_comparison());
        Ok(())
    }

    fn observe_personal_pair_comparison_window(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &PersonalCacheReplayState,
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
        let pool = decoder.decode_sentence(&canonical, PERSONAL_CACHE_POOL_DEPTH)?;

        let target_index = pool.iter().position(|candidate| candidate.text == target);
        let reserved_once_active = pool.iter().any(|candidate| {
            candidate_has_pair_with_minimum_count(
                candidate,
                frozen_state,
                PERSONAL_PAIR_ONCE_MIN_COUNT,
            )
        });
        let reserved_repeated_active = pool.iter().any(|candidate| {
            candidate_has_pair_with_minimum_count(
                candidate,
                frozen_state,
                PERSONAL_PAIR_REPEATED_MIN_COUNT,
            )
        });
        if reserved_once_active {
            self.personal_pair_reserved_once_active_windows = self
                .personal_pair_reserved_once_active_windows
                .saturating_add(1);
        }
        if reserved_repeated_active {
            self.personal_pair_reserved_repeated_active_windows = self
                .personal_pair_reserved_repeated_active_windows
                .saturating_add(1);
        }
        let mut any_pair_evidence = false;
        let mut competing_pair_evidence = false;
        for (candidate_index, candidate) in pool.iter().enumerate() {
            let pair_evidence = personal_cache_evidence(
                candidate,
                frozen_state,
                PersonalCacheKind::OrderedWordPairs,
                &canonical,
            );
            if pair_evidence.prior_pair_occurrences == 0 {
                continue;
            }
            any_pair_evidence = true;
            self.personal_pair_history_evidence_candidates = self
                .personal_pair_history_evidence_candidates
                .saturating_add(1);
            if Some(candidate_index) != target_index {
                competing_pair_evidence = true;
            }
        }
        if any_pair_evidence {
            self.personal_pair_history_any_evidence_windows = self
                .personal_pair_history_any_evidence_windows
                .saturating_add(1);
        }
        if competing_pair_evidence {
            self.personal_pair_history_competing_evidence_windows = self
                .personal_pair_history_competing_evidence_windows
                .saturating_add(1);
        }
        if let Some(target_index) = target_index {
            self.personal_pair_history_target_in_pool_windows = self
                .personal_pair_history_target_in_pool_windows
                .saturating_add(1);
            let target_candidate = &pool[target_index];
            if candidate_has_pair_with_minimum_count(
                target_candidate,
                frozen_state,
                PERSONAL_PAIR_ONCE_MIN_COUNT,
            ) {
                self.personal_pair_reserved_once_target_evidence_windows = self
                    .personal_pair_reserved_once_target_evidence_windows
                    .saturating_add(1);
            }
            if candidate_has_pair_with_minimum_count(
                target_candidate,
                frozen_state,
                PERSONAL_PAIR_REPEATED_MIN_COUNT,
            ) {
                self.personal_pair_reserved_repeated_target_evidence_windows = self
                    .personal_pair_reserved_repeated_target_evidence_windows
                    .saturating_add(1);
            }
            let word_evidence = personal_cache_evidence(
                target_candidate,
                frozen_state,
                PersonalCacheKind::WordFrequency,
                &canonical,
            );
            let pair_evidence = personal_cache_evidence(
                target_candidate,
                frozen_state,
                PersonalCacheKind::OrderedWordPairs,
                &canonical,
            );
            if pair_evidence.prior_pair_occurrences > 0 {
                self.personal_pair_history_target_evidence_windows = self
                    .personal_pair_history_target_evidence_windows
                    .saturating_add(1);
                if pair_evidence.promotion > word_evidence.promotion {
                    self.personal_pair_history_target_extra_promotion_windows = self
                        .personal_pair_history_target_extra_promotion_windows
                        .saturating_add(1);
                }
                if word_evidence.promotion == PERSONAL_CACHE_MAX_PROMOTION {
                    self.personal_pair_history_target_word_cap_saturated_windows = self
                        .personal_pair_history_target_word_cap_saturated_windows
                        .saturating_add(1);
                }
            }
        }

        self.personal_pair_comparison_windows =
            self.personal_pair_comparison_windows.saturating_add(1);
        let public_rank = self.personal_pair_public_window_canonical_full.observe(
            &canonical,
            &target,
            &pool[..pool.len().min(REPLAY_TOP_K)],
        );
        let frozen_word = observe_personal_strategy_from_pool(
            &pool,
            frozen_state,
            PersonalCacheKind::WordFrequency,
            &canonical,
            &target,
            &mut self.personal_pair_frozen_word_window_canonical_full,
        );
        let frozen_pair = observe_personal_strategy_from_pool(
            &pool,
            frozen_state,
            PersonalCacheKind::OrderedWordPairs,
            &canonical,
            &target,
            &mut self.personal_pair_frozen_window_canonical_full,
        );
        let causal_pair = observe_personal_strategy_from_pool(
            &pool,
            causal_state,
            PersonalCacheKind::OrderedWordPairs,
            &canonical,
            &target,
            &mut self.personal_pair_causal_window_canonical_full,
        );
        let reserved_once = observe_personal_strategy_from_pool_with_evidence(
            &pool,
            &canonical,
            &target,
            &mut self.personal_pair_reserved_once_window_canonical_full,
            |candidate| {
                personal_reserved_pair_evidence(
                    candidate,
                    frozen_state,
                    &canonical,
                    PERSONAL_PAIR_ONCE_MIN_COUNT,
                    reserved_once_active,
                )
            },
        );
        let reserved_repeated = observe_personal_strategy_from_pool_with_evidence(
            &pool,
            &canonical,
            &target,
            &mut self.personal_pair_reserved_repeated_window_canonical_full,
            |candidate| {
                personal_reserved_pair_evidence(
                    candidate,
                    frozen_state,
                    &canonical,
                    PERSONAL_PAIR_REPEATED_MIN_COUNT,
                    reserved_repeated_active,
                )
            },
        );
        debug_assert_eq!(public_rank, frozen_word.unigram);
        debug_assert_eq!(frozen_word.unigram, frozen_pair.unigram);
        debug_assert_eq!(frozen_word.unigram, causal_pair.unigram);
        debug_assert_eq!(frozen_word.unigram, reserved_once.unigram);
        debug_assert_eq!(frozen_word.unigram, reserved_repeated.unigram);
        self.personal_pair_frozen_vs_frozen_word
            .observe(frozen_word.personal, frozen_pair.personal);
        self.personal_pair_causal_vs_frozen_word
            .observe(frozen_word.personal, causal_pair.personal);
        self.personal_pair_causal_vs_frozen_pair
            .observe(frozen_pair.personal, causal_pair.personal);
        self.personal_pair_reserved_once_vs_frozen_word
            .observe(frozen_word.personal, reserved_once.personal);
        self.personal_pair_reserved_repeated_vs_frozen_word
            .observe(frozen_word.personal, reserved_repeated.personal);
        Ok(())
    }

    fn finish_personal_cache_learning(
        &mut self,
        state: &mut PersonalCacheReplayState,
        run: &mut Vec<WindowCommit>,
        learn_code_text: bool,
    ) {
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
            if learn_code_text && run.len() >= 2 {
                let code = run
                    .iter()
                    .map(|commit| commit.observed.as_str())
                    .collect::<String>();
                if code.len() <= MAX_REPLAY_CODE_KEYS {
                    let text = run
                        .iter()
                        .map(|commit| commit.target.as_str())
                        .collect::<String>();
                    self.learn_personal_code_text(
                        state,
                        first.document_start,
                        last.document_end(),
                        code,
                        text,
                    );
                }
            }
        }
        run.clear();
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

    fn learn_personal_code_text(
        &mut self,
        state: &mut PersonalCacheReplayState,
        start: usize,
        end: usize,
        code: String,
        text: String,
    ) {
        state.learn_code_text(start, end, code, text);
        self.personal_cache_learning_code_text_tokens = self
            .personal_cache_learning_code_text_tokens
            .saturating_add(1);
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
        self.personal_cache_reversed_code_text_tokens = self
            .personal_cache_reversed_code_text_tokens
            .saturating_add(outcome.invalidated_code_text_tokens);
        self.personal_cache_reversed_left_context_tokens = self
            .personal_cache_reversed_left_context_tokens
            .saturating_add(outcome.invalidated_left_context_tokens);
        if outcome.ambiguous_position {
            self.personal_cache_ambiguous_edits_not_applied = self
                .personal_cache_ambiguous_edits_not_applied
                .saturating_add(1);
        }
        if is_revision {
            if outcome.invalidated_commits > 0
                || outcome.invalidated_pair_sequences > 0
                || outcome.invalidated_code_text_tokens > 0
                || outcome.invalidated_left_context_tokens > 0
            {
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
        let baseline_ranks = observe_strategy_with_personal_cache(
            decoder,
            state,
            kind,
            &canonical,
            &target,
            &mut self.personal_cache_window_canonical_full,
        )?;
        let baseline_rank = baseline_ranks.unigram;
        let personal_baseline_rank = baseline_ranks.personal;
        if let Some(code) = &word_tail_code {
            let ranks = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_one_short,
            )?;
            let unigram_rank = ranks.unigram;
            let personal_rank = ranks.personal;
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
            let ranks = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_keep_singletons,
            )?;
            let unigram_rank = ranks.unigram;
            let personal_rank = ranks.personal;
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
            let ranks = observe_strategy_with_personal_cache(
                decoder,
                state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_head_anchored,
            )?;
            let unigram_rank = ranks.unigram;
            let personal_rank = ranks.personal;
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

    fn observe_personal_cache_comparison_window(
        &mut self,
        decoder: &Decoder,
        frozen_state: &PersonalCacheReplayState,
        causal_state: &PersonalCacheReplayState,
        kind: PersonalCacheKind,
        run: &[WindowCommit],
        include_code_comparison: bool,
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
        let mut pool_cache = HashMap::<String, Vec<SentenceCandidate>>::new();

        self.personal_cache_windows = self.personal_cache_windows.saturating_add(1);
        self.personal_frozen_cache_windows = self.personal_frozen_cache_windows.saturating_add(1);
        let baseline_pool = decode_personal_pool_memoized(decoder, &mut pool_cache, &canonical)?;
        let causal_baseline = observe_personal_strategy_from_pool(
            &baseline_pool,
            causal_state,
            kind,
            &canonical,
            &target,
            &mut self.personal_cache_window_canonical_full,
        );
        let frozen_baseline = observe_personal_strategy_from_pool(
            &baseline_pool,
            frozen_state,
            kind,
            &canonical,
            &target,
            &mut self.personal_frozen_window_canonical_full,
        );
        debug_assert_eq!(causal_baseline.unigram, frozen_baseline.unigram);
        let baseline_rank = causal_baseline.unigram;
        if include_code_comparison {
            self.personal_code_cache_windows = self.personal_code_cache_windows.saturating_add(1);
            let target_index = baseline_pool
                .iter()
                .position(|candidate| candidate.text == target);
            let mut frozen_any_evidence = false;
            let mut frozen_competing_evidence = false;
            let mut causal_any_evidence = false;
            let mut causal_competing_evidence = false;
            for (candidate_index, candidate) in baseline_pool.iter().enumerate() {
                if frozen_state.code_text_count(&canonical, &candidate.text) > 0 {
                    frozen_any_evidence = true;
                    if Some(candidate_index) != target_index {
                        frozen_competing_evidence = true;
                    }
                }
                if causal_state.code_text_count(&canonical, &candidate.text) > 0 {
                    causal_any_evidence = true;
                    if Some(candidate_index) != target_index {
                        causal_competing_evidence = true;
                    }
                }
            }
            if frozen_any_evidence {
                self.personal_code_frozen_any_evidence_windows = self
                    .personal_code_frozen_any_evidence_windows
                    .saturating_add(1);
            }
            if frozen_competing_evidence {
                self.personal_code_frozen_competing_evidence_windows = self
                    .personal_code_frozen_competing_evidence_windows
                    .saturating_add(1);
            }
            if causal_any_evidence {
                self.personal_code_causal_any_evidence_windows = self
                    .personal_code_causal_any_evidence_windows
                    .saturating_add(1);
            }
            if causal_competing_evidence {
                self.personal_code_causal_competing_evidence_windows = self
                    .personal_code_causal_competing_evidence_windows
                    .saturating_add(1);
            }
            if let Some(target_index) = target_index {
                self.personal_code_target_in_pool_windows =
                    self.personal_code_target_in_pool_windows.saturating_add(1);
                let target_candidate = &baseline_pool[target_index];
                if frozen_state.code_text_count(&canonical, &target_candidate.text) > 0 {
                    self.personal_code_frozen_target_evidence_windows = self
                        .personal_code_frozen_target_evidence_windows
                        .saturating_add(1);
                }
                if causal_state.code_text_count(&canonical, &target_candidate.text) > 0 {
                    self.personal_code_causal_target_evidence_windows = self
                        .personal_code_causal_target_evidence_windows
                        .saturating_add(1);
                    let word_evidence = personal_cache_evidence(
                        target_candidate,
                        frozen_state,
                        PersonalCacheKind::WordFrequency,
                        &canonical,
                    );
                    let hybrid_evidence = personal_hybrid_evidence(
                        target_candidate,
                        frozen_state,
                        causal_state,
                        &canonical,
                    );
                    if hybrid_evidence.promotion > word_evidence.promotion {
                        self.personal_hybrid_target_extra_promotion_windows = self
                            .personal_hybrid_target_extra_promotion_windows
                            .saturating_add(1);
                    }
                    if word_evidence.promotion == PERSONAL_CACHE_MAX_PROMOTION {
                        self.personal_hybrid_target_word_cap_saturated_windows = self
                            .personal_hybrid_target_word_cap_saturated_windows
                            .saturating_add(1);
                    }
                }
            }
            let code_causal = observe_personal_strategy_from_pool(
                &baseline_pool,
                causal_state,
                PersonalCacheKind::ExactCodeText,
                &canonical,
                &target,
                &mut self.personal_code_causal_window_canonical_full,
            );
            let code_frozen = observe_personal_strategy_from_pool(
                &baseline_pool,
                frozen_state,
                PersonalCacheKind::ExactCodeText,
                &canonical,
                &target,
                &mut self.personal_code_frozen_window_canonical_full,
            );
            debug_assert_eq!(code_causal.unigram, code_frozen.unigram);
            debug_assert_eq!(code_causal.unigram, baseline_rank);
            let hybrid = observe_personal_hybrid_strategy_from_pool(
                &baseline_pool,
                frozen_state,
                causal_state,
                &canonical,
                &target,
                &mut self.personal_hybrid_window_canonical_full,
            );
            debug_assert_eq!(hybrid.unigram, baseline_rank);
            self.personal_code_frozen_vs_unigram
                .observe(baseline_rank, code_frozen.personal);
            self.personal_code_causal_vs_unigram
                .observe(baseline_rank, code_causal.personal);
            self.personal_code_causal_vs_frozen
                .observe(code_frozen.personal, code_causal.personal);
            self.personal_hybrid_vs_frozen_word
                .observe(frozen_baseline.personal, hybrid.personal);
        }

        if let Some(code) = &word_tail_code {
            let pool = decode_personal_pool_memoized(decoder, &mut pool_cache, code)?;
            let causal = observe_personal_strategy_from_pool(
                &pool,
                causal_state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_one_short,
            );
            let frozen = observe_personal_strategy_from_pool(
                &pool,
                frozen_state,
                kind,
                code,
                &target,
                &mut self.personal_frozen_window_word_tail_one_short,
            );
            debug_assert_eq!(causal.unigram, frozen.unigram);
            self.personal_cache_window_word_tail_one_short_vs_full
                .observe(&canonical, code, causal_baseline.personal, causal.personal);
            self.personal_frozen_window_word_tail_one_short_vs_full
                .observe(&canonical, code, frozen_baseline.personal, frozen.personal);
            self.word_tail_one_short_personal_cache_effect.observe(
                baseline_rank,
                causal.unigram,
                causal_baseline.personal,
                causal.personal,
            );
            self.word_tail_one_short_personal_frozen_effect.observe(
                baseline_rank,
                frozen.unigram,
                frozen_baseline.personal,
                frozen.personal,
            );
        }
        if let Some(code) = &word_tail_keep_singletons_code {
            let pool = decode_personal_pool_memoized(decoder, &mut pool_cache, code)?;
            let causal = observe_personal_strategy_from_pool(
                &pool,
                causal_state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_tail_keep_singletons,
            );
            let frozen = observe_personal_strategy_from_pool(
                &pool,
                frozen_state,
                kind,
                code,
                &target,
                &mut self.personal_frozen_window_word_tail_keep_singletons,
            );
            debug_assert_eq!(causal.unigram, frozen.unigram);
            self.personal_cache_window_word_tail_keep_singletons_vs_full
                .observe(&canonical, code, causal_baseline.personal, causal.personal);
            self.personal_frozen_window_word_tail_keep_singletons_vs_full
                .observe(&canonical, code, frozen_baseline.personal, frozen.personal);
            self.word_tail_keep_singletons_personal_cache_effect
                .observe(
                    baseline_rank,
                    causal.unigram,
                    causal_baseline.personal,
                    causal.personal,
                );
            self.word_tail_keep_singletons_personal_frozen_effect
                .observe(
                    baseline_rank,
                    frozen.unigram,
                    frozen_baseline.personal,
                    frozen.personal,
                );
        }
        if let Some(code) = &word_head_code {
            let pool = decode_personal_pool_memoized(decoder, &mut pool_cache, code)?;
            let causal = observe_personal_strategy_from_pool(
                &pool,
                causal_state,
                kind,
                code,
                &target,
                &mut self.personal_cache_window_word_head_anchored,
            );
            let frozen = observe_personal_strategy_from_pool(
                &pool,
                frozen_state,
                kind,
                code,
                &target,
                &mut self.personal_frozen_window_word_head_anchored,
            );
            debug_assert_eq!(causal.unigram, frozen.unigram);
            self.personal_cache_window_word_head_anchored_vs_full
                .observe(&canonical, code, causal_baseline.personal, causal.personal);
            self.personal_frozen_window_word_head_anchored_vs_full
                .observe(&canonical, code, frozen_baseline.personal, frozen.personal);
            self.word_head_anchored_personal_cache_effect.observe(
                baseline_rank,
                causal.unigram,
                causal_baseline.personal,
                causal.personal,
            );
            self.word_head_anchored_personal_frozen_effect.observe(
                baseline_rank,
                frozen.unigram,
                frozen_baseline.personal,
                frozen.personal,
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
            self.isolated_eligible_commits = self
                .isolated_eligible_commits
                .saturating_add(u64::try_from(run.len()).unwrap_or(u64::MAX));
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
        let mut window_decode_cache = HashMap::<String, Vec<SentenceCandidate>>::new();
        let _ = observe_strategy_with_rank_memoized(
            decoder,
            &mut window_decode_cache,
            &raw,
            &target,
            &mut self.window_raw_joined,
        )?;
        let baseline_rank = observe_strategy_with_rank_memoized(
            decoder,
            &mut window_decode_cache,
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
            let strategy_rank = observe_strategy_with_rank_memoized(
                decoder,
                &mut window_decode_cache,
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
            let strategy_rank = observe_strategy_with_rank_memoized(
                decoder,
                &mut window_decode_cache,
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
            let strategy_rank = observe_strategy_with_rank_memoized(
                decoder,
                &mut window_decode_cache,
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
            self.public_context_canonical_full_vs_unigram
                .observe(baseline_rank, context_baseline_rank);
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
             window_eligible_commits={} window_ineligible_commits={} \
             isolated_eligible_commits={} {} continuous_windows={} \
             continuous_window_commits={} \
             continuous_window_recorded_logical_key_actions={} \
             continuous_windows_over_key_limit={} {} {} {} {} {} {} {} {} \
             public_context_windows={} {} {} {} {} {} {} {} {} {} {} {} \
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
            self.isolated_eligible_commits,
            self.window_exclusions.terminal_fields(),
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
            self.public_context_canonical_full_vs_unigram
                .terminal_fields("public_context_canonical_full_vs_unigram"),
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

    pub fn personal_word_comparison_terminal_report(&self) -> String {
        let mut lines = vec![
            format!(
                "PERSONAL_WORD_COMPARISON schema=ziranma-personal-word-comparison-v1 \
                 contains_text=false contains_behavioral_metadata=true writes=false network=false \
                 evaluation_learning=frozen_and_causal_online candidate_pool_depth={} \
                 max_promotion={}",
                PERSONAL_CACHE_POOL_DEPTH, PERSONAL_CACHE_MAX_PROMOTION
            ),
            format!(
                "HISTORY capsules={} events={} learning_commits={} word_tokens={} word_types={} \
                 repeated_word_tokens={}",
                self.personal_cache_history_capsules,
                self.personal_cache_history_events,
                self.personal_cache_history_learning_commits,
                self.personal_cache_history_word_tokens,
                self.personal_cache_history_word_types,
                self.personal_cache_history_word_tokens
                    .saturating_sub(self.personal_cache_history_word_types)
            ),
            format!(
                "EVALUATION capsules={} events={} gap_ms={} eligible_commits={} \
                 ineligible_commits={} windows={} window_commits={} \
                 frozen_windows={} causal_learning_commits={} causal_learning_word_tokens={} \
                 causal_retained_word_tokens={} frozen_evaluation_updates=0",
                self.capsules,
                self.events,
                display_optional(self.window_gap_limit_ms),
                self.window_eligible_commits,
                self.window_ineligible_commits,
                self.continuous_windows,
                self.continuous_window_commits,
                self.personal_frozen_cache_windows,
                self.personal_cache_learning_commits,
                self.personal_cache_learning_word_tokens,
                self.personal_cache_retained_word_tokens
            ),
            compact_strategy_line(
                "window_unigram",
                "canonical_full",
                &self.window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_word_cache_frozen",
                "canonical_full",
                &self.personal_frozen_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_word_cache_causal",
                "canonical_full",
                &self.personal_cache_window_canonical_full,
            ),
        ];
        lines.extend([
            compact_context_comparison_line(
                "personal_word_cache_frozen",
                "word_tail_one_short",
                &self.window_word_tail_one_short_vs_full,
                &self.personal_frozen_window_word_tail_one_short_vs_full,
                &self.word_tail_one_short_personal_frozen_effect,
                &self.personal_frozen_window_word_tail_one_short,
            ),
            compact_context_comparison_line(
                "personal_word_cache_causal",
                "word_tail_one_short",
                &self.window_word_tail_one_short_vs_full,
                &self.personal_cache_window_word_tail_one_short_vs_full,
                &self.word_tail_one_short_personal_cache_effect,
                &self.personal_cache_window_word_tail_one_short,
            ),
            compact_context_comparison_line(
                "personal_word_cache_frozen",
                "word_tail_keep_singletons",
                &self.window_word_tail_keep_singletons_vs_full,
                &self.personal_frozen_window_word_tail_keep_singletons_vs_full,
                &self.word_tail_keep_singletons_personal_frozen_effect,
                &self.personal_frozen_window_word_tail_keep_singletons,
            ),
            compact_context_comparison_line(
                "personal_word_cache_causal",
                "word_tail_keep_singletons",
                &self.window_word_tail_keep_singletons_vs_full,
                &self.personal_cache_window_word_tail_keep_singletons_vs_full,
                &self.word_tail_keep_singletons_personal_cache_effect,
                &self.personal_cache_window_word_tail_keep_singletons,
            ),
            compact_context_comparison_line(
                "personal_word_cache_frozen",
                "word_head_anchored",
                &self.window_word_head_anchored_vs_full,
                &self.personal_frozen_window_word_head_anchored_vs_full,
                &self.word_head_anchored_personal_frozen_effect,
                &self.personal_frozen_window_word_head_anchored,
            ),
            compact_context_comparison_line(
                "personal_word_cache_causal",
                "word_head_anchored",
                &self.window_word_head_anchored_vs_full,
                &self.personal_cache_window_word_head_anchored_vs_full,
                &self.word_head_anchored_personal_cache_effect,
                &self.personal_cache_window_word_head_anchored,
            ),
        ]);
        lines.join("\n")
    }

    pub fn personal_pair_comparison_terminal_report(&self) -> String {
        [
            format!(
                "PERSONAL_PAIR_COMPARISON schema=ziranma-personal-pair-comparison-v2 \
                 contains_text=false contains_behavioral_metadata=true writes=false network=false \
                 frozen_evaluation_updates=0 causal_evaluation_learning=after_scoring \
                 pair_identity=adjacent_lexicon_words decay=none candidate_pool_depth={} \
                 max_promotion={} reserved_pair_slot=one_of_three",
                PERSONAL_CACHE_POOL_DEPTH, PERSONAL_CACHE_MAX_PROMOTION
            ),
            format!(
                "HISTORY capsules={} events={} learning_commits={} word_tokens={} word_types={} \
                 repeated_word_tokens={} pair_tokens={} pair_types={} repeated_pair_tokens={}",
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
                    .saturating_sub(self.personal_cache_history_word_pair_types)
            ),
            format!(
                "EVALUATION capsules={} events={} gap_ms={} eligible_commits={} \
                 ineligible_commits={} windows={} comparison_windows={} window_commits={} \
                 causal_learning_commits={} causal_learning_word_tokens={} \
                 causal_learning_pair_sequences={} causal_learning_pair_tokens={} \
                 causal_retained_pair_tokens={} causal_pair_types={} reversed_commits={} \
                 reversed_word_tokens={} reversed_pair_sequences={} reversed_pair_tokens={} \
                 revisions_with_reversal={} revisions_without_reversal={} ambiguous_edits={}",
                self.capsules,
                self.events,
                display_optional(self.window_gap_limit_ms),
                self.window_eligible_commits,
                self.window_ineligible_commits,
                self.continuous_windows,
                self.personal_pair_comparison_windows,
                self.continuous_window_commits,
                self.personal_cache_learning_commits,
                self.personal_cache_learning_word_tokens,
                self.personal_cache_learning_pair_sequences,
                self.personal_cache_learning_word_pairs,
                self.personal_cache_retained_word_pairs,
                self.personal_cache_learned_word_pair_types,
                self.personal_cache_reversed_commits,
                self.personal_cache_reversed_word_tokens,
                self.personal_cache_reversed_pair_sequences,
                self.personal_cache_reversed_word_pairs,
                self.personal_cache_revision_events_with_reversal,
                self.personal_cache_revisions_not_reversed,
                self.personal_cache_ambiguous_edits_not_applied
            ),
            format!(
                "PAIR_EVIDENCE source=frozen_history any_evidence_windows={} \
                 target_in_pool_windows={} target_evidence_windows={} \
                 target_extra_promotion_windows={} target_word_cap_saturated_windows={} \
                 competing_evidence_windows={} evidence_candidates={}",
                self.personal_pair_history_any_evidence_windows,
                self.personal_pair_history_target_in_pool_windows,
                self.personal_pair_history_target_evidence_windows,
                self.personal_pair_history_target_extra_promotion_windows,
                self.personal_pair_history_target_word_cap_saturated_windows,
                self.personal_pair_history_competing_evidence_windows,
                self.personal_pair_history_evidence_candidates
            ),
            format!(
                "PAIR_RESERVED once_min_same_pair_count={} once_active_windows={} \
                 once_target_evidence_windows={} repeated_min_same_pair_count={} \
                 repeated_active_windows={} repeated_target_evidence_windows={} \
                 activation_scope=shared_candidate_pool",
                PERSONAL_PAIR_ONCE_MIN_COUNT,
                self.personal_pair_reserved_once_active_windows,
                self.personal_pair_reserved_once_target_evidence_windows,
                PERSONAL_PAIR_REPEATED_MIN_COUNT,
                self.personal_pair_reserved_repeated_active_windows,
                self.personal_pair_reserved_repeated_target_evidence_windows
            ),
            compact_strategy_line(
                "window_unigram",
                "canonical_full",
                &self.personal_pair_public_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_word_cache_frozen",
                "canonical_full",
                &self.personal_pair_frozen_word_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_pair_cache_frozen",
                "canonical_full",
                &self.personal_pair_frozen_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_pair_cache_causal",
                "canonical_full",
                &self.personal_pair_causal_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_pair_reserved_once_frozen",
                "canonical_full",
                &self.personal_pair_reserved_once_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_pair_reserved_repeated_frozen",
                "canonical_full",
                &self.personal_pair_reserved_repeated_window_canonical_full,
            ),
            compact_ranking_comparison_line(
                "personal_pair_frozen_vs_frozen_word",
                &self.personal_pair_frozen_vs_frozen_word,
            ),
            compact_ranking_comparison_line(
                "personal_pair_causal_vs_frozen_word",
                &self.personal_pair_causal_vs_frozen_word,
            ),
            compact_ranking_comparison_line(
                "personal_pair_causal_vs_frozen_pair",
                &self.personal_pair_causal_vs_frozen_pair,
            ),
            compact_ranking_comparison_line(
                "personal_pair_reserved_once_vs_frozen_word",
                &self.personal_pair_reserved_once_vs_frozen_word,
            ),
            compact_ranking_comparison_line(
                "personal_pair_reserved_repeated_vs_frozen_word",
                &self.personal_pair_reserved_repeated_vs_frozen_word,
            ),
        ]
        .join("\n")
    }

    pub fn personal_code_comparison_terminal_report(&self) -> String {
        [
            format!(
                "PERSONAL_CODE_COMPARISON schema=ziranma-personal-code-comparison-v3 \
                 contains_text=false contains_behavioral_metadata=true writes=false network=false \
                 evaluation_learning=frozen_and_causal_online \
                 code_identity=exact_observed_code_and_window_text decay=none \
                 hybrid=frozen_word_plus_causal_exact_code \
                 combined_promotion=bounded_sum candidate_pool_depth={} max_promotion={}",
                PERSONAL_CACHE_POOL_DEPTH, PERSONAL_CACHE_MAX_PROMOTION
            ),
            format!(
                "HISTORY capsules={} events={} word_tokens={} word_types={} \
                 code_text_tokens={} code_text_types={} repeated_code_text_tokens={}",
                self.personal_cache_history_capsules,
                self.personal_cache_history_events,
                self.personal_cache_history_word_tokens,
                self.personal_cache_history_word_types,
                self.personal_cache_history_code_text_tokens,
                self.personal_cache_history_code_text_types,
                self.personal_cache_history_code_text_tokens
                    .saturating_sub(self.personal_cache_history_code_text_types)
            ),
            format!(
                "EVALUATION capsules={} events={} gap_ms={} windows={} \
                 code_windows={} causal_code_text_tokens={} retained_code_text_tokens={} \
                 code_text_types={} reversed_code_text_tokens={} frozen_evaluation_updates=0",
                self.capsules,
                self.events,
                display_optional(self.window_gap_limit_ms),
                self.continuous_windows,
                self.personal_code_cache_windows,
                self.personal_cache_learning_code_text_tokens,
                self.personal_cache_retained_code_text_tokens,
                self.personal_cache_learned_code_text_types,
                self.personal_cache_reversed_code_text_tokens
            ),
            format!(
                "CODE_EVIDENCE target_in_pool_windows={} frozen_any_evidence_windows={} \
                 frozen_target_evidence_windows={} frozen_competing_evidence_windows={} \
                 causal_any_evidence_windows={} causal_target_evidence_windows={} \
                 causal_competing_evidence_windows={} hybrid_target_extra_promotion_windows={} \
                 hybrid_target_word_cap_saturated_windows={}",
                self.personal_code_target_in_pool_windows,
                self.personal_code_frozen_any_evidence_windows,
                self.personal_code_frozen_target_evidence_windows,
                self.personal_code_frozen_competing_evidence_windows,
                self.personal_code_causal_any_evidence_windows,
                self.personal_code_causal_target_evidence_windows,
                self.personal_code_causal_competing_evidence_windows,
                self.personal_hybrid_target_extra_promotion_windows,
                self.personal_hybrid_target_word_cap_saturated_windows
            ),
            compact_strategy_line(
                "window_unigram",
                "canonical_full",
                &self.window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_word_cache_frozen",
                "canonical_full",
                &self.personal_frozen_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_word_cache_causal",
                "canonical_full",
                &self.personal_cache_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_code_cache_frozen",
                "canonical_full",
                &self.personal_code_frozen_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_code_cache_causal",
                "canonical_full",
                &self.personal_code_causal_window_canonical_full,
            ),
            compact_strategy_line(
                "window_personal_hybrid_cache",
                "canonical_full",
                &self.personal_hybrid_window_canonical_full,
            ),
            compact_ranking_comparison_line(
                "personal_code_frozen_vs_unigram",
                &self.personal_code_frozen_vs_unigram,
            ),
            compact_ranking_comparison_line(
                "personal_code_causal_vs_unigram",
                &self.personal_code_causal_vs_unigram,
            ),
            compact_ranking_comparison_line(
                "personal_code_causal_vs_frozen",
                &self.personal_code_causal_vs_frozen,
            ),
            compact_ranking_comparison_line(
                "personal_hybrid_vs_frozen_word",
                &self.personal_hybrid_vs_frozen_word,
            ),
        ]
        .join("\n")
    }

    pub fn personal_left_context_comparison_terminal_report(&self) -> String {
        let pool_depth = PERSONAL_CACHE_POOL_DEPTH.max(PERSONAL_CONTEXT_SEARCH_DEPTH);
        [
            format!(
                "PERSONAL_LEFT_CONTEXT_COMPARISON \
                 schema=ziranma-personal-left-context-comparison-v2 contains_text=false \
                 contains_behavioral_metadata=true writes=false network=false \
                 candidate_pool_code=canonical target_identity_code=observed \
                 context_identity=previous_committed_text_and_observed_code_and_selected_text \
                 selection_rejections=unavailable evaluation_learning=after_scoring \
                 frozen_evaluation_updates=0 candidate_pool_depth={} context_search_depth={} \
                 max_context_entries={} context_support_cap={}",
                pool_depth,
                PERSONAL_CONTEXT_SEARCH_DEPTH,
                MAX_PERSONAL_CONTEXT_ENTRIES,
                PERSONAL_CONTEXT_SUPPORT_CAP
            ),
            format!(
                "HISTORY capsules={} events={} exact_code_text_tokens={} \
                 exact_code_text_types={} repeated_exact_code_text_tokens={} \
                 left_context_tokens={} left_context_types={} \
                 repeated_left_context_tokens={}",
                self.personal_cache_history_capsules,
                self.personal_cache_history_events,
                self.personal_cache_history_code_text_tokens,
                self.personal_cache_history_code_text_types,
                self.personal_cache_history_code_text_tokens
                    .saturating_sub(self.personal_cache_history_code_text_types),
                self.personal_cache_history_left_context_tokens,
                self.personal_cache_history_left_context_types,
                self.personal_cache_history_left_context_tokens
                    .saturating_sub(self.personal_cache_history_left_context_types)
            ),
            format!(
                "EVALUATION capsules={} events={} gap_ms={} eligible_commits={} \
                 ineligible_commits={} windows={} comparison_commits={} \
                 causal_exact_code_text_tokens={} retained_exact_code_text_tokens={} \
                 exact_code_text_types={} causal_left_context_tokens={} \
                 retained_left_context_tokens={} left_context_types={} \
                 reversed_exact_code_text_tokens={} reversed_left_context_tokens={} \
                 revisions_with_reversal={} revisions_without_reversal={} ambiguous_edits={}",
                self.capsules,
                self.events,
                display_optional(self.window_gap_limit_ms),
                self.window_eligible_commits,
                self.window_ineligible_commits,
                self.continuous_windows,
                self.personal_left_context_comparison_commits,
                self.personal_cache_learning_code_text_tokens,
                self.personal_cache_retained_code_text_tokens,
                self.personal_cache_learned_code_text_types,
                self.personal_cache_learning_left_context_tokens,
                self.personal_cache_retained_left_context_tokens,
                self.personal_cache_learned_left_context_types,
                self.personal_cache_reversed_code_text_tokens,
                self.personal_cache_reversed_left_context_tokens,
                self.personal_cache_revision_events_with_reversal,
                self.personal_cache_revisions_not_reversed,
                self.personal_cache_ambiguous_edits_not_applied
            ),
            format!(
                "LEFT_CONTEXT_EVIDENCE target_in_pool_commits={} \
                 frozen_any_evidence_commits={} frozen_target_evidence_commits={} \
                 frozen_competing_evidence_commits={} causal_any_evidence_commits={} \
                 causal_target_evidence_commits={} causal_competing_evidence_commits={}",
                self.personal_left_context_target_in_pool_commits,
                self.personal_left_context_frozen_any_evidence_commits,
                self.personal_left_context_frozen_target_evidence_commits,
                self.personal_left_context_frozen_competing_evidence_commits,
                self.personal_left_context_causal_any_evidence_commits,
                self.personal_left_context_causal_target_evidence_commits,
                self.personal_left_context_causal_competing_evidence_commits
            ),
            format!(
                "LEFT_CONTEXT_MOVEMENT frozen_preferences={} frozen_already_first={} \
                 frozen_promotions={} frozen_target_promotions={} \
                 frozen_competing_promotions={} causal_preferences={} \
                 causal_already_first={} causal_promotions={} \
                 causal_target_promotions={} causal_competing_promotions={}",
                self.personal_left_context_frozen_movement.preferences,
                self.personal_left_context_frozen_movement.already_first,
                self.personal_left_context_frozen_movement.promotions,
                self.personal_left_context_frozen_movement.target_promotions,
                self.personal_left_context_frozen_movement
                    .competing_promotions,
                self.personal_left_context_causal_movement.preferences,
                self.personal_left_context_causal_movement.already_first,
                self.personal_left_context_causal_movement.promotions,
                self.personal_left_context_causal_movement.target_promotions,
                self.personal_left_context_causal_movement
                    .competing_promotions
            ),
            compact_strategy_line(
                "commit_public",
                "canonical_pool_observed_identity",
                &self.personal_left_context_public,
            ),
            compact_strategy_line(
                "commit_personal_exact_frozen",
                "canonical_pool_observed_identity",
                &self.personal_left_context_frozen_exact,
            ),
            compact_strategy_line(
                "commit_personal_exact_causal",
                "canonical_pool_observed_identity",
                &self.personal_left_context_causal_exact,
            ),
            compact_strategy_line(
                "commit_personal_left_context_frozen",
                "canonical_pool_observed_identity",
                &self.personal_left_context_frozen_context,
            ),
            compact_strategy_line(
                "commit_personal_left_context_causal",
                "canonical_pool_observed_identity",
                &self.personal_left_context_causal_context,
            ),
            compact_ranking_comparison_line(
                "personal_exact_frozen_vs_public",
                &self.personal_left_context_frozen_exact_vs_public,
            ),
            compact_ranking_comparison_line(
                "personal_left_context_frozen_vs_exact_frozen",
                &self.personal_left_context_frozen_context_vs_exact,
            ),
            compact_ranking_comparison_line(
                "personal_left_context_causal_vs_exact_causal",
                &self.personal_left_context_causal_context_vs_causal_exact,
            ),
            compact_ranking_comparison_line(
                "personal_left_context_causal_vs_exact_frozen",
                &self.personal_left_context_causal_context_vs_exact,
            ),
            compact_ranking_comparison_line(
                "personal_left_context_causal_vs_frozen",
                &self.personal_left_context_causal_vs_frozen_context,
            ),
        ]
        .join("\n")
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
                "WINDOWS gap_ms={} eligible_commits={} ineligible_commits={} \
                 isolated_eligible_commits={} {} windows={} \
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
                self.isolated_eligible_commits,
                self.window_exclusions.terminal_fields(),
                self.continuous_windows,
                self.continuous_window_commits,
                self.continuous_window_recorded_logical_key_actions,
                self.continuous_windows_over_key_limit,
                self.public_context_windows,
                PUBLIC_CONTEXT_REPLAY_POOL_DEPTH,
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
                    compact_ranking_comparison_line(
                        context_kind.compact_context_label(),
                        &self.public_context_canonical_full_vs_unigram,
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

#[derive(Clone)]
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
    if !record.keys_complete || window_document_exclusion(record).is_some() {
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

fn window_document_exclusion(record: &crate::CommitRecord) -> Option<WindowExclusionReason> {
    if record.change.position_evidence == crate::DeltaPositionEvidence::Ambiguous
        || record.document_change.position_evidence == crate::DeltaPositionEvidence::Ambiguous
    {
        Some(WindowExclusionReason::AmbiguousPosition)
    } else if !record.document_change.deleted.is_empty()
        || record.document_change.inserted.is_empty()
        || record.document_change.inserted != record.change.inserted
    {
        Some(WindowExclusionReason::NonAppendDocumentChange)
    } else {
        None
    }
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

fn observe_strategy_with_rank_memoized(
    decoder: &Decoder,
    cache: &mut HashMap<String, Vec<SentenceCandidate>>,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<Option<usize>, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(None);
    }
    let candidates = decode_sentence_memoized(decoder, cache, code)?;
    Ok(stats.observe(code, target, &candidates))
}

#[derive(Clone, Copy)]
struct PersonalCacheEvidence {
    promotion: usize,
    observed_word_tokens: usize,
    prior_occurrences: u64,
    observed_word_pairs: usize,
    prior_pair_occurrences: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PersonalStrategyRanks {
    unigram: Option<usize>,
    personal: Option<usize>,
}

fn observe_strategy_with_personal_cache(
    decoder: &Decoder,
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> Result<PersonalStrategyRanks, KeySequenceError> {
    if code.len() > MAX_REPLAY_CODE_KEYS {
        return Ok(PersonalStrategyRanks {
            unigram: None,
            personal: None,
        });
    }
    let pool = decoder.decode_sentence(code, PERSONAL_CACHE_POOL_DEPTH)?;
    Ok(observe_personal_strategy_from_pool(
        &pool, state, kind, code, target, stats,
    ))
}

fn decode_personal_pool_memoized(
    decoder: &Decoder,
    cache: &mut HashMap<String, Vec<SentenceCandidate>>,
    code: &str,
) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
    if let Some(candidates) = cache.get(code) {
        return Ok(candidates.clone());
    }
    let candidates = decoder.decode_sentence(code, PERSONAL_CACHE_POOL_DEPTH)?;
    cache.insert(code.to_owned(), candidates.clone());
    Ok(candidates)
}

fn observe_personal_strategy_from_pool(
    pool: &[SentenceCandidate],
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> PersonalStrategyRanks {
    observe_personal_strategy_from_pool_with_evidence(pool, code, target, stats, |candidate| {
        personal_cache_evidence(candidate, state, kind, code)
    })
}

fn observe_personal_hybrid_strategy_from_pool(
    pool: &[SentenceCandidate],
    frozen_word_state: &PersonalCacheReplayState,
    causal_code_state: &PersonalCacheReplayState,
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
) -> PersonalStrategyRanks {
    observe_personal_strategy_from_pool_with_evidence(pool, code, target, stats, |candidate| {
        personal_hybrid_evidence(candidate, frozen_word_state, causal_code_state, code)
    })
}

fn observe_personal_strategy_from_pool_with_evidence(
    pool: &[SentenceCandidate],
    code: &str,
    target: &str,
    stats: &mut ReplayStrategyStats,
    mut evidence_for: impl FnMut(&SentenceCandidate) -> PersonalCacheEvidence,
) -> PersonalStrategyRanks {
    let unigram = pool
        .iter()
        .take(REPLAY_TOP_K)
        .position(|candidate| candidate.text == target)
        .map(|rank| rank + 1);
    let candidates = personal_ranked_candidates_from_pool_with_evidence(pool, |candidate| {
        evidence_for(candidate)
    });
    let personal = stats.observe(
        code,
        target,
        &candidates[..candidates.len().min(REPLAY_TOP_K)],
    );
    PersonalStrategyRanks { unigram, personal }
}

fn personal_ranked_candidates_from_pool(
    pool: &[SentenceCandidate],
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
    code: &str,
) -> Vec<SentenceCandidate> {
    personal_ranked_candidates_from_pool_with_evidence(pool, |candidate| {
        personal_cache_evidence(candidate, state, kind, code)
    })
}

fn personal_ranked_candidates_from_pool_with_evidence(
    pool: &[SentenceCandidate],
    mut evidence_for: impl FnMut(&SentenceCandidate) -> PersonalCacheEvidence,
) -> Vec<SentenceCandidate> {
    let mut scored = pool
        .iter()
        .cloned()
        .enumerate()
        .map(|(baseline_rank, candidate)| {
            let evidence = evidence_for(&candidate);
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
    scored
        .into_iter()
        .map(|(_, candidate, _)| candidate)
        .collect()
}

fn personal_left_context_candidates_from_pool(
    pool: &[SentenceCandidate],
    exact_state: &PersonalCacheReplayState,
    context_state: &PersonalCacheReplayState,
    previous_text: &str,
    code: &str,
) -> (Vec<SentenceCandidate>, PersonalLeftContextRerankObservation) {
    let mut candidates = personal_ranked_candidates_from_pool(
        pool,
        exact_state,
        PersonalCacheKind::ExactCodeText,
        code,
    );
    let searchable_texts = pool
        .iter()
        .take(PERSONAL_CONTEXT_SEARCH_DEPTH)
        .map(|candidate| candidate.text.as_str());
    let Some(preferred) =
        context_state.preferred_left_context_text(previous_text, code, searchable_texts)
    else {
        return (candidates, PersonalLeftContextRerankObservation::default());
    };
    let Some(index) = candidates
        .iter()
        .position(|candidate| candidate.text == preferred)
    else {
        return (candidates, PersonalLeftContextRerankObservation::default());
    };
    if index > 0 {
        let candidate = candidates.remove(index);
        candidates.insert(0, candidate);
    }
    (
        candidates,
        PersonalLeftContextRerankObservation {
            preferred_rank_after_exact: Some(index),
        },
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PersonalLeftContextRerankObservation {
    preferred_rank_after_exact: Option<usize>,
}

fn personal_hybrid_evidence(
    candidate: &SentenceCandidate,
    frozen_word_state: &PersonalCacheReplayState,
    causal_code_state: &PersonalCacheReplayState,
    code: &str,
) -> PersonalCacheEvidence {
    let word = personal_cache_evidence(
        candidate,
        frozen_word_state,
        PersonalCacheKind::WordFrequency,
        code,
    );
    let exact_code = personal_cache_evidence(
        candidate,
        causal_code_state,
        PersonalCacheKind::ExactCodeText,
        code,
    );
    PersonalCacheEvidence {
        promotion: word
            .promotion
            .saturating_add(exact_code.promotion)
            .min(PERSONAL_CACHE_MAX_PROMOTION),
        observed_word_tokens: word
            .observed_word_tokens
            .saturating_add(exact_code.observed_word_tokens),
        prior_occurrences: word
            .prior_occurrences
            .saturating_add(exact_code.prior_occurrences),
        observed_word_pairs: 0,
        prior_pair_occurrences: 0,
    }
}

fn personal_reserved_pair_evidence(
    candidate: &SentenceCandidate,
    state: &PersonalCacheReplayState,
    code: &str,
    minimum_pair_count: u64,
    reserve_active: bool,
) -> PersonalCacheEvidence {
    let word = personal_cache_evidence(candidate, state, PersonalCacheKind::WordFrequency, code);
    if !reserve_active {
        return word;
    }
    let pair = personal_cache_evidence(candidate, state, PersonalCacheKind::OrderedWordPairs, code);
    let qualifies = candidate_has_pair_with_minimum_count(candidate, state, minimum_pair_count);
    PersonalCacheEvidence {
        promotion: word
            .promotion
            .min(PERSONAL_PAIR_RESERVED_WORD_PROMOTION)
            .saturating_add(usize::from(qualifies)),
        observed_word_tokens: word.observed_word_tokens,
        prior_occurrences: word.prior_occurrences,
        observed_word_pairs: if qualifies {
            pair.observed_word_pairs
        } else {
            0
        },
        prior_pair_occurrences: if qualifies {
            pair.prior_pair_occurrences
        } else {
            0
        },
    }
}

fn candidate_has_pair_with_minimum_count(
    candidate: &SentenceCandidate,
    state: &PersonalCacheReplayState,
    minimum_pair_count: u64,
) -> bool {
    debug_assert!(minimum_pair_count > 0);
    let mut previous_word = None::<&str>;
    for segment in &candidate.segments {
        if segment.candidate.source != CandidateSource::Lexicon {
            previous_word = None;
            continue;
        }
        if previous_word.is_some_and(|previous| {
            state.pair_count(previous, &segment.candidate.text) >= minimum_pair_count
        }) {
            return true;
        }
        previous_word = Some(&segment.candidate.text);
    }
    false
}

fn personal_cache_evidence(
    candidate: &SentenceCandidate,
    state: &PersonalCacheReplayState,
    kind: PersonalCacheKind,
    code: &str,
) -> PersonalCacheEvidence {
    if kind == PersonalCacheKind::ExactCodeText {
        let prior_occurrences = state.code_text_count(code, &candidate.text);
        return PersonalCacheEvidence {
            promotion: personal_promotion(prior_occurrences),
            observed_word_tokens: usize::from(prior_occurrences > 0),
            prior_occurrences,
            observed_word_pairs: 0,
            prior_pair_occurrences: 0,
        };
    }
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
    let word_promotion = personal_promotion(prior_occurrences);
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

fn personal_promotion(prior_occurrences: u64) -> usize {
    match prior_occurrences {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        _ => PERSONAL_CACHE_MAX_PROMOTION,
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
    let pool = decoder.decode_sentence(code, PUBLIC_CONTEXT_REPLAY_POOL_DEPTH)?;
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

fn compact_ranking_comparison_line(
    context_label: &str,
    comparison: &RankingReplayComparisonStats,
) -> String {
    format!(
        "RANKING_COMPARE context={context_label} comparisons={} gained_top_1={} \
         lost_top_1={} improved={} same={} worsened={} both_outside_top_10={} \
         dropped_from_top_10={} recovered_into_top_10={} \
         baseline_visible_at_10={} reranked_visible_at_10={}",
        comparison.comparisons,
        comparison.gained_top_1,
        comparison.lost_top_1,
        comparison.rank_improved,
        comparison.rank_same,
        comparison.rank_worsened,
        comparison.both_outside_top_10,
        comparison.dropped_from_top_10,
        comparison.recovered_into_top_10,
        comparison.baseline_visible_at_10,
        comparison.reranked_visible_at_10
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
        PersonalCacheKind, PersonalCacheReplayError, PersonalCacheReplayState,
        RankingReplayComparisonStats, ReplayStrategyStats, candidate_has_pair_with_minimum_count,
        decode_personal_pool_memoized, effective_letter_code,
        observe_personal_hybrid_strategy_from_pool, observe_personal_strategy_from_pool,
        observe_strategy_with_personal_cache, personal_cache_evidence, personal_hybrid_evidence,
        personal_reserved_pair_evidence,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, EventCapsuleV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput, parse_lexicon_tsv,
    };
    use std::collections::HashMap;

    const LEXICON: &str = "\
text\tpinyin\tfrequency
我\two\t200
猫猫\tmao mao\t100
麻烦\tma fan\t90
再\tzai\t300
在\tzai\t100
";

    const LEFT_CONTEXT_LEXICON: &str = "\
text\tpinyin\tfrequency
请\tqing\t500
好\thao\t500
吧\tba\t400
八\tba\t300
巴\tba\t200
把\tba\t100
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
    fn ranking_comparison_separates_top_one_changes_and_partitions_all_windows() {
        let mut stats = RankingReplayComparisonStats::default();
        for (baseline, reranked) in [
            (Some(1), Some(2)),
            (Some(2), Some(1)),
            (Some(2), Some(2)),
            (Some(3), None),
            (None, Some(3)),
            (None, None),
        ] {
            stats.observe(baseline, reranked);
        }

        assert_eq!(stats.comparisons, 6);
        assert_eq!(stats.baseline_visible_at_10, 4);
        assert_eq!(stats.reranked_visible_at_10, 4);
        assert_eq!(stats.gained_top_1, 1);
        assert_eq!(stats.lost_top_1, 1);
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
    fn window_exclusions_are_mutually_exclusive_and_eligible_singletons_balance() {
        let mut incomplete = commit_event(100, 0, "mk", "mao", "猫猫");
        let TrackerOutput::Commit(record) = &mut incomplete.output else {
            unreachable!();
        };
        record.keys_complete = false;

        let mut key_failure = commit_event(200, 0, "mk", "mao", "猫猫");
        let TrackerOutput::Commit(record) = &mut key_failure.output else {
            unreachable!();
        };
        record.keys = vec![RawKey::Letter('m'), RawKey::Space, RawKey::Letter('k')];

        let mut missing_code = commit_event(300, 0, "mk", "mao", "猫猫");
        let TrackerOutput::Commit(record) = &mut missing_code.output else {
            unreachable!();
        };
        record.keys = vec![RawKey::Space];

        let over_limit = commit_event(400, 0, &"m".repeat(65), "mao", "猫猫");
        let unencodable = commit_event(500, 0, "mk", "not-a-pinyin", "猫猫");
        let missing_boundaries = commit_event(600, 0, "mkmk", "mao'mao", "不存在");

        let mut ambiguous = commit_event(700, 0, "mkmk", "mao'mao", "猫猫");
        let TrackerOutput::Commit(record) = &mut ambiguous.output else {
            unreachable!();
        };
        record.document_change.position_evidence = DeltaPositionEvidence::Ambiguous;

        let mut non_append = commit_event(800, 0, "mkmk", "mao'mao", "猫猫");
        let TrackerOutput::Commit(record) = &mut non_append.output else {
            unreachable!();
        };
        record.document_change.deleted = "旧".to_owned();

        let isolated = commit_event(900, 0, "mkmk", "mao'mao", "猫猫");
        let capsule = EventCapsuleV1::new(vec![
            incomplete,
            key_failure,
            missing_code,
            over_limit,
            unencodable,
            missing_boundaries,
            ambiguous,
            non_append,
            isolated,
        ])
        .unwrap();
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report.observe_capsule(&decoder, &capsule).unwrap();

        assert_eq!(report.window_ineligible_commits, 8);
        assert_eq!(report.window_exclusions.incomplete_keys, 1);
        assert_eq!(report.window_exclusions.key_interpretation_failure, 1);
        assert_eq!(report.window_exclusions.missing_letter_code, 1);
        assert_eq!(report.window_exclusions.code_over_limit, 1);
        assert_eq!(report.window_exclusions.composition_unencodable, 1);
        assert_eq!(
            report
                .window_exclusions
                .canonical_word_boundaries_unavailable,
            1
        );
        assert_eq!(report.window_exclusions.ambiguous_position, 1);
        assert_eq!(report.window_exclusions.non_append_document_change, 1);
        assert_eq!(
            report.window_exclusions.total(),
            report.window_ineligible_commits
        );
        assert_eq!(report.window_eligible_commits, 1);
        assert_eq!(report.isolated_eligible_commits, 1);
        assert_eq!(report.continuous_window_commits, 0);
        assert_eq!(report.continuous_windows, 0);

        let compact = report.compact_terminal_report();
        assert!(compact.contains("isolated_eligible_commits=1"));
        assert!(compact.contains("window_exclusion_total=8"));
        assert!(!compact.contains("不存在"));
        assert!(!compact.contains("not-a-pinyin"));
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
            report.public_context_canonical_full_vs_unigram.comparisons,
            1
        );
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
        assert_eq!(compact.lines().count(), 9);
        assert!(compact.contains("scope=window_unigram name=canonical_full"));
        assert!(compact.contains("scope=window_public_word_bigram name=canonical_full"));
        assert!(compact.contains("RANKING_COMPARE context=public_word_bigram comparisons=1"));
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
        assert_eq!(
            report.public_context_canonical_full_vs_unigram.comparisons,
            1
        );

        let terminal = report.terminal_line();
        assert!(terminal.contains("public_context_kind=character_bigram"));
        let compact = report.compact_terminal_report();
        assert_eq!(compact.lines().count(), 9);
        assert!(compact.contains("scope=window_unigram name=canonical_full"));
        assert!(compact.contains("scope=window_public_character_bigram name=canonical_full"));
        assert!(compact.contains("RANKING_COMPARE context=public_character_bigram comparisons=1"));
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
    fn compact_personal_path_matches_full_metrics_without_hidden_commit_lanes() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            revision_event(300, 3, "", "，"),
            commit_event(10_000, 4, "zl", "zai", "在"),
            commit_event(10_100, 5, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut full_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut full_state = PersonalCacheReplayState::new();
        full_report
            .observe_capsule_with_personal_cache(&decoder, &mut full_state, &capsule)
            .unwrap();
        let mut compact_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut compact_state = PersonalCacheReplayState::new();
        compact_report
            .observe_capsule_with_compact_personal_cache(&decoder, &mut compact_state, &capsule)
            .unwrap();

        assert_eq!(
            compact_report.compact_terminal_report(),
            full_report.compact_terminal_report()
        );
        assert!(full_report.raw_existing.attempts > 0);
        assert_eq!(compact_report.raw_existing.attempts, 0);
        assert_eq!(format!("{compact_state:?}"), format!("{full_state:?}"));
    }

    #[test]
    fn frozen_and_causal_word_caches_share_history_then_diverge_only_after_learning() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let first_evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let second_evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut causal_state = PersonalCacheReplayState::new();
        let frozen_state = causal_state.fork_for_frozen_evaluation();
        assert_eq!(frozen_state.word_counts, causal_state.word_counts);
        assert_eq!(
            frozen_state.learned_word_tokens,
            causal_state.learned_word_tokens
        );
        let frozen_before = format!("{frozen_state:?}");
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report
            .observe_capsule_with_personal_word_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &first_evaluation,
            )
            .unwrap();
        assert_eq!(
            report.personal_frozen_window_canonical_full,
            report.personal_cache_window_canonical_full
        );
        assert_eq!(causal_state.learned_word_tokens(), 2);
        report
            .observe_capsule_with_personal_word_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &second_evaluation,
            )
            .unwrap();

        assert_eq!(report.continuous_windows, 2);
        assert_eq!(report.personal_cache_windows, 2);
        assert_eq!(report.personal_frozen_cache_windows, 2);
        assert_eq!(report.window_canonical_full.hits_at_1, 0);
        assert_eq!(report.personal_frozen_window_canonical_full.hits_at_1, 0);
        assert_eq!(report.personal_cache_window_canonical_full.hits_at_1, 1);
        assert_eq!(
            report.personal_frozen_window_canonical_full.attempts,
            report.personal_cache_window_canonical_full.attempts
        );
        assert_eq!(format!("{frozen_state:?}"), frozen_before);
        assert_eq!(frozen_state.learned_word_tokens(), 0);
        assert_eq!(causal_state.learned_word_tokens(), 4);

        let compact = report.personal_word_comparison_terminal_report();
        assert!(compact.contains("evaluation_learning=frozen_and_causal_online"));
        assert!(compact.contains("scope=window_unigram name=canonical_full"));
        assert!(compact.contains("scope=window_personal_word_cache_frozen name=canonical_full"));
        assert!(compact.contains("scope=window_personal_word_cache_causal name=canonical_full"));
        assert!(compact.contains("frozen_evaluation_updates=0"));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn pair_comparison_shares_one_pool_and_scores_before_causal_learning() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            revision_event(300, 0, "在", ""),
        ])
        .unwrap();
        let mut causal_state = PersonalCacheReplayState::new();
        causal_state.learn_commit(0, 1, &["在".to_owned()]);
        causal_state.learn_commit(1, 1, &["再".to_owned()]);
        causal_state.learn_commit(2, 2, &["猫猫".to_owned()]);
        assert_eq!(
            causal_state.learn_pair_sequence(0, 3, &["在".to_owned(), "猫猫".to_owned()]),
            1
        );
        let frozen_state = causal_state.fork_for_frozen_evaluation();
        let frozen_before = format!("{frozen_state:?}");
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report
            .observe_capsule_with_personal_pair_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &evaluation,
            )
            .unwrap();

        assert_eq!(report.personal_pair_comparison_windows, 1);
        assert_eq!(
            report.personal_pair_public_window_canonical_full.attempts,
            1
        );
        assert_eq!(
            report
                .personal_pair_frozen_word_window_canonical_full
                .attempts,
            1
        );
        assert_eq!(
            report.personal_pair_frozen_window_canonical_full.attempts,
            1
        );
        assert_eq!(
            report.personal_pair_causal_window_canonical_full.attempts,
            1
        );
        assert_eq!(
            report
                .personal_pair_reserved_once_window_canonical_full
                .attempts,
            1
        );
        assert_eq!(
            report
                .personal_pair_reserved_repeated_window_canonical_full
                .attempts,
            1
        );
        assert_eq!(report.personal_pair_frozen_vs_frozen_word.comparisons, 1);
        assert_eq!(report.personal_pair_frozen_vs_frozen_word.rank_improved, 1);
        assert_eq!(report.personal_pair_causal_vs_frozen_word.rank_improved, 1);
        assert_eq!(report.personal_pair_history_any_evidence_windows, 1);
        assert_eq!(report.personal_pair_history_target_in_pool_windows, 1);
        assert_eq!(report.personal_pair_history_target_evidence_windows, 1);
        assert_eq!(
            report.personal_pair_history_target_extra_promotion_windows,
            1
        );
        assert_eq!(
            report.personal_pair_history_target_word_cap_saturated_windows,
            0
        );
        assert_eq!(report.personal_pair_reserved_once_active_windows, 1);
        assert_eq!(
            report.personal_pair_reserved_once_target_evidence_windows,
            1
        );
        assert_eq!(report.personal_pair_reserved_repeated_active_windows, 0);
        assert_eq!(
            report.personal_pair_reserved_repeated_target_evidence_windows,
            0
        );
        assert_eq!(
            report
                .personal_pair_reserved_once_vs_frozen_word
                .rank_improved,
            1
        );
        assert_eq!(
            report
                .personal_pair_reserved_repeated_vs_frozen_word
                .rank_same,
            1
        );
        assert_eq!(report.personal_cache_learning_word_pairs, 1);
        assert_eq!(report.personal_cache_reversed_word_pairs, 1);
        assert_eq!(causal_state.learned_word_pairs(), 1);
        assert_eq!(format!("{frozen_state:?}"), frozen_before);

        let compact = report.personal_pair_comparison_terminal_report();
        assert!(compact.contains("schema=ziranma-personal-pair-comparison-v2"));
        assert!(compact.contains("causal_evaluation_learning=after_scoring"));
        assert!(compact.contains("scope=window_personal_word_cache_frozen"));
        assert!(compact.contains("scope=window_personal_pair_cache_frozen"));
        assert!(compact.contains("scope=window_personal_pair_cache_causal"));
        assert!(compact.contains("context=personal_pair_frozen_vs_frozen_word"));
        assert!(compact.contains("PAIR_EVIDENCE source=frozen_history"));
        assert!(compact.contains("PAIR_RESERVED once_min_same_pair_count=1"));
        assert!(compact.contains("scope=window_personal_pair_reserved_once_frozen"));
        assert!(compact.contains("scope=window_personal_pair_reserved_repeated_frozen"));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn frozen_word_cache_copies_counts_but_not_history_document_spans() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let mut history_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut causal_state = PersonalCacheReplayState::new();
        history_report
            .learn_capsule_for_personal_cache(&decoder, &mut causal_state, &history)
            .unwrap();
        assert!(!causal_state.active_document_spans.is_empty());
        assert!(!causal_state.active_pair_spans.is_empty());

        let frozen_state = causal_state.fork_for_frozen_evaluation();

        assert_eq!(frozen_state.word_counts, causal_state.word_counts);
        assert_eq!(frozen_state.pair_counts, causal_state.pair_counts);
        assert_eq!(
            frozen_state.learned_word_tokens,
            causal_state.learned_word_tokens
        );
        assert_eq!(
            frozen_state.learned_word_pairs,
            causal_state.learned_word_pairs
        );
        assert!(frozen_state.active_document_spans.is_empty());
        assert!(frozen_state.active_pair_spans.is_empty());
    }

    #[test]
    fn exact_code_text_evidence_cannot_promote_the_same_candidate_under_another_code() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let pool = decoder
            .decode_sentence("zlmkmk", super::PERSONAL_CACHE_POOL_DEPTH)
            .unwrap();
        let mut state = PersonalCacheReplayState::new();
        for _ in 0..4 {
            state.learn_code_text(0, 3, "zlmkmk".to_owned(), "在猫猫".to_owned());
        }
        let mut matching_stats = ReplayStrategyStats::default();
        let matching = observe_personal_strategy_from_pool(
            &pool,
            &state,
            PersonalCacheKind::ExactCodeText,
            "zlmkmk",
            "在猫猫",
            &mut matching_stats,
        );
        let mut other_stats = ReplayStrategyStats::default();
        let other = observe_personal_strategy_from_pool(
            &pool,
            &state,
            PersonalCacheKind::ExactCodeText,
            "unrelated",
            "在猫猫",
            &mut other_stats,
        );

        assert_eq!(matching.personal, Some(1));
        assert_eq!(other.personal, other.unigram);
        assert_ne!(matching.personal, other.personal);
        assert_eq!(state.learned_code_text_types(), 1);
        assert_eq!(state.learned_code_text_tokens(), 4);
    }

    #[test]
    fn exact_code_comparison_scores_each_window_before_learning_its_identity() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let capsule = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            commit_event(10_000, 3, "zl", "zai", "在"),
            commit_event(10_100, 4, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let frozen_state = PersonalCacheReplayState::new().fork_for_frozen_evaluation();
        let mut causal_state = PersonalCacheReplayState::new();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report
            .observe_capsule_with_personal_code_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &capsule,
            )
            .unwrap();

        assert_eq!(report.personal_code_cache_windows, 2);
        assert_eq!(report.personal_code_frozen_any_evidence_windows, 0);
        assert_eq!(report.personal_code_frozen_target_evidence_windows, 0);
        assert_eq!(
            report.personal_code_causal_any_evidence_windows, 1,
            "only the second window may observe the first window's identity"
        );
        assert_eq!(report.personal_code_causal_target_evidence_windows, 1);
        assert_eq!(report.personal_code_causal_competing_evidence_windows, 0);
        assert_eq!(report.personal_cache_learning_code_text_tokens, 2);
        assert_eq!(report.personal_cache_retained_code_text_tokens, 2);
        assert_eq!(report.personal_cache_learned_code_text_types, 1);
        assert_eq!(report.personal_code_causal_vs_frozen.comparisons, 2);
        assert_eq!(report.personal_code_causal_vs_frozen.rank_worsened, 0);
    }

    #[test]
    fn hybrid_personal_evidence_shares_one_promotion_budget() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let pool = decoder
            .decode_sentence("zlmkmk", super::PERSONAL_CACHE_POOL_DEPTH)
            .unwrap();
        let candidate = pool
            .iter()
            .find(|candidate| candidate.text == "在猫猫")
            .unwrap();
        let mut frozen_word_state = PersonalCacheReplayState::new();
        let mut causal_code_state = PersonalCacheReplayState::new();
        for index in 0..4 {
            frozen_word_state.learn_commit(index, 3, &["在".to_owned(), "猫猫".to_owned()]);
            causal_code_state.learn_code_text(
                index,
                index + 3,
                "zlmkmk".to_owned(),
                "在猫猫".to_owned(),
            );
        }

        let word = personal_cache_evidence(
            candidate,
            &frozen_word_state,
            PersonalCacheKind::WordFrequency,
            "zlmkmk",
        );
        let exact = personal_cache_evidence(
            candidate,
            &causal_code_state,
            PersonalCacheKind::ExactCodeText,
            "zlmkmk",
        );
        let hybrid =
            personal_hybrid_evidence(candidate, &frozen_word_state, &causal_code_state, "zlmkmk");

        assert_eq!(word.promotion, super::PERSONAL_CACHE_MAX_PROMOTION);
        assert_eq!(exact.promotion, super::PERSONAL_CACHE_MAX_PROMOTION);
        assert_eq!(hybrid.promotion, super::PERSONAL_CACHE_MAX_PROMOTION);
    }

    #[test]
    fn hybrid_without_exact_code_evidence_matches_frozen_word_ranking() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let pool = decoder
            .decode_sentence("zlmkmk", super::PERSONAL_CACHE_POOL_DEPTH)
            .unwrap();
        let mut frozen_word_state = PersonalCacheReplayState::new();
        frozen_word_state.learn_commit(0, 3, &["在".to_owned(), "猫猫".to_owned()]);
        let causal_code_state = PersonalCacheReplayState::new();
        let mut word_stats = ReplayStrategyStats::default();
        let word = observe_personal_strategy_from_pool(
            &pool,
            &frozen_word_state,
            PersonalCacheKind::WordFrequency,
            "zlmkmk",
            "在猫猫",
            &mut word_stats,
        );
        let mut hybrid_stats = ReplayStrategyStats::default();
        let hybrid = observe_personal_hybrid_strategy_from_pool(
            &pool,
            &frozen_word_state,
            &causal_code_state,
            "zlmkmk",
            "在猫猫",
            &mut hybrid_stats,
        );

        assert_eq!(hybrid, word);
        assert_eq!(hybrid_stats, word_stats);
    }

    #[test]
    fn hybrid_without_word_evidence_matches_exact_code_ranking() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let pool = decoder
            .decode_sentence("zlmkmk", super::PERSONAL_CACHE_POOL_DEPTH)
            .unwrap();
        let frozen_word_state = PersonalCacheReplayState::new();
        let mut causal_code_state = PersonalCacheReplayState::new();
        causal_code_state.learn_code_text(0, 3, "zlmkmk".to_owned(), "在猫猫".to_owned());
        let mut exact_stats = ReplayStrategyStats::default();
        let exact = observe_personal_strategy_from_pool(
            &pool,
            &causal_code_state,
            PersonalCacheKind::ExactCodeText,
            "zlmkmk",
            "在猫猫",
            &mut exact_stats,
        );
        let mut hybrid_stats = ReplayStrategyStats::default();
        let hybrid = observe_personal_hybrid_strategy_from_pool(
            &pool,
            &frozen_word_state,
            &causal_code_state,
            "zlmkmk",
            "在猫猫",
            &mut hybrid_stats,
        );

        assert_eq!(hybrid, exact);
        assert_eq!(hybrid_stats, exact_stats);
    }

    #[test]
    fn code_comparison_shares_history_and_reverses_an_edited_online_window() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
        ])
        .unwrap();
        let evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            revision_event(300, 0, "在", ""),
        ])
        .unwrap();
        let mut causal_state = PersonalCacheReplayState::new();
        let mut history_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        history_report
            .learn_capsule_for_personal_code_comparison(&decoder, &mut causal_state, &history)
            .unwrap();
        assert_eq!(causal_state.learned_code_text_types(), 1);
        assert_eq!(causal_state.learned_code_text_tokens(), 1);
        let frozen_state = causal_state.fork_for_frozen_evaluation();
        let frozen_before = format!("{frozen_state:?}");
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report.record_personal_cache_history(&history_report, &causal_state);

        report
            .observe_capsule_with_personal_code_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &evaluation,
            )
            .unwrap();

        assert_eq!(report.personal_code_cache_windows, 1);
        assert_eq!(
            report.personal_code_frozen_window_canonical_full.attempts,
            1
        );
        assert_eq!(
            report.personal_code_causal_window_canonical_full.attempts,
            1
        );
        assert_eq!(report.personal_hybrid_window_canonical_full.attempts, 1);
        assert_eq!(report.personal_hybrid_vs_frozen_word.comparisons, 1);
        assert_eq!(report.personal_code_frozen_vs_unigram.comparisons, 1);
        assert_eq!(report.personal_code_causal_vs_unigram.comparisons, 1);
        assert_eq!(report.personal_code_causal_vs_frozen.comparisons, 1);
        assert_eq!(report.personal_code_target_in_pool_windows, 1);
        assert_eq!(report.personal_code_frozen_any_evidence_windows, 1);
        assert_eq!(report.personal_code_frozen_target_evidence_windows, 1);
        assert_eq!(report.personal_code_frozen_competing_evidence_windows, 0);
        assert_eq!(report.personal_code_causal_any_evidence_windows, 1);
        assert_eq!(report.personal_code_causal_target_evidence_windows, 1);
        assert_eq!(report.personal_code_causal_competing_evidence_windows, 0);
        assert_eq!(report.personal_hybrid_target_extra_promotion_windows, 1);
        assert_eq!(report.personal_hybrid_target_word_cap_saturated_windows, 0);
        assert_eq!(report.personal_cache_history_code_text_tokens, 1);
        assert_eq!(report.personal_cache_history_code_text_types, 1);
        assert_eq!(report.personal_cache_learning_code_text_tokens, 1);
        assert_eq!(report.personal_cache_reversed_code_text_tokens, 1);
        assert_eq!(causal_state.learned_code_text_tokens(), 1);
        assert_eq!(format!("{frozen_state:?}"), frozen_before);

        let compact = report.personal_code_comparison_terminal_report();
        assert!(compact.contains("schema=ziranma-personal-code-comparison-v3"));
        assert!(compact.contains("code_identity=exact_observed_code_and_window_text"));
        assert!(compact.contains("decay=none"));
        assert!(compact.contains("combined_promotion=bounded_sum"));
        assert!(compact.contains("scope=window_personal_code_cache_frozen"));
        assert!(compact.contains("scope=window_personal_code_cache_causal"));
        assert!(compact.contains("scope=window_personal_hybrid_cache"));
        assert!(compact.contains("CODE_EVIDENCE target_in_pool_windows=1"));
        assert!(compact.contains("context=personal_code_frozen_vs_unigram"));
        assert!(compact.contains("context=personal_code_causal_vs_unigram"));
        assert!(compact.contains("context=personal_code_causal_vs_frozen"));
        assert!(compact.contains("context=personal_hybrid_vs_frozen_word"));
        assert!(!compact.contains("在"));
        assert!(!compact.contains("猫"));
        assert!(!compact.contains("zai"));
        assert!(!compact.contains("mao"));
    }

    #[test]
    fn frozen_exact_left_context_adds_signal_beyond_exact_code_text_history() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEFT_CONTEXT_LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "qy", "qing", "请"),
            commit_event(200, 1, "ab", "ba", "把"),
        ])
        .unwrap();
        let evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "qy", "qing", "请"),
            commit_event(200, 1, "ab", "ba", "把"),
        ])
        .unwrap();
        let mut causal_state = PersonalCacheReplayState::new();
        let mut history_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        history_report
            .learn_capsule_for_personal_left_context_comparison(
                &decoder,
                &mut causal_state,
                &history,
            )
            .unwrap();
        assert_eq!(causal_state.learned_code_text_tokens(), 2);
        assert_eq!(causal_state.learned_left_context_tokens(), 1);
        let frozen_state = causal_state.fork_for_frozen_evaluation();
        let frozen_before = format!("{frozen_state:?}");
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        report.record_personal_cache_history(&history_report, &causal_state);

        report
            .observe_capsule_with_personal_left_context_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &evaluation,
            )
            .unwrap();

        assert_eq!(report.personal_left_context_comparison_commits, 1);
        assert_eq!(report.personal_left_context_public.hits_at_1, 0);
        assert_eq!(report.personal_left_context_frozen_exact.hits_at_1, 0);
        assert_eq!(report.personal_left_context_frozen_context.hits_at_1, 1);
        assert_eq!(report.personal_left_context_causal_context.hits_at_1, 1);
        assert_eq!(
            report
                .personal_left_context_frozen_context_vs_exact
                .gained_top_1,
            1
        );
        assert_eq!(report.personal_left_context_frozen_any_evidence_commits, 1);
        assert_eq!(
            report.personal_left_context_frozen_target_evidence_commits,
            1
        );
        assert_eq!(
            report.personal_left_context_frozen_competing_evidence_commits,
            0
        );
        assert_eq!(report.personal_cache_history_code_text_tokens, 2);
        assert_eq!(report.personal_cache_history_left_context_tokens, 1);
        assert_eq!(report.personal_left_context_frozen_movement.preferences, 1);
        assert_eq!(
            report.personal_left_context_frozen_movement.already_first,
            0
        );
        assert_eq!(report.personal_left_context_frozen_movement.promotions, 1);
        assert_eq!(
            report
                .personal_left_context_frozen_movement
                .target_promotions,
            1
        );
        assert_eq!(
            report
                .personal_left_context_frozen_movement
                .competing_promotions,
            0
        );
        assert_eq!(format!("{frozen_state:?}"), frozen_before);

        let compact = report.personal_left_context_comparison_terminal_report();
        assert!(compact.contains("schema=ziranma-personal-left-context-comparison-v2"));
        assert!(compact.contains("candidate_pool_code=canonical"));
        assert!(compact.contains("target_identity_code=observed"));
        assert!(compact.contains("selection_rejections=unavailable"));
        assert!(compact.contains("frozen_target_evidence_commits=1"));
        assert!(compact.contains("frozen_preferences=1"));
        assert!(compact.contains("frozen_promotions=1"));
        assert!(compact.contains("frozen_target_promotions=1"));
        assert!(compact.contains("frozen_competing_promotions=0"));
        assert!(compact.contains("context=personal_left_context_frozen_vs_exact_frozen"));
        assert!(!compact.contains("请"));
        assert!(!compact.contains("把"));
        assert!(!compact.contains("qing"));
    }

    #[test]
    fn causal_left_context_scores_before_learning_each_current_identity() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEFT_CONTEXT_LEXICON).unwrap());
        let evaluation = EventCapsuleV1::new(vec![
            commit_event(100, 0, "qy", "qing", "请"),
            commit_event(200, 1, "ab", "ba", "把"),
            commit_event(10_000, 2, "qy", "qing", "请"),
            commit_event(10_100, 3, "ab", "ba", "把"),
        ])
        .unwrap();
        let frozen_state = PersonalCacheReplayState::new().fork_for_frozen_evaluation();
        let mut causal_state = PersonalCacheReplayState::new();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report
            .observe_capsule_with_personal_left_context_comparison(
                &decoder,
                &frozen_state,
                &mut causal_state,
                &evaluation,
            )
            .unwrap();

        assert_eq!(report.continuous_windows, 2);
        assert_eq!(report.personal_left_context_comparison_commits, 2);
        assert_eq!(report.personal_left_context_frozen_context.hits_at_1, 0);
        assert_eq!(report.personal_left_context_causal_context.hits_at_1, 1);
        assert_eq!(
            report.personal_left_context_causal_target_evidence_commits,
            1
        );
        assert_eq!(
            report
                .personal_left_context_causal_context_vs_causal_exact
                .gained_top_1,
            1
        );
        assert_eq!(report.personal_cache_learning_code_text_tokens, 4);
        assert_eq!(report.personal_cache_learning_left_context_tokens, 2);
        assert_eq!(report.personal_left_context_causal_movement.preferences, 1);
        assert_eq!(report.personal_left_context_causal_movement.promotions, 1);
        assert_eq!(
            report
                .personal_left_context_causal_movement
                .target_promotions,
            1
        );
    }

    #[test]
    fn overlapping_revision_reverses_exact_and_left_context_replay_evidence() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEFT_CONTEXT_LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "qy", "qing", "请"),
            commit_event(200, 1, "ab", "ba", "把"),
            revision_event(300, 1, "把", ""),
        ])
        .unwrap();
        let mut state = PersonalCacheReplayState::new();
        let mut report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();

        report
            .learn_capsule_for_personal_left_context_comparison(&decoder, &mut state, &history)
            .unwrap();

        assert_eq!(state.learned_code_text_tokens(), 1);
        assert_eq!(state.learned_left_context_tokens(), 0);
        assert_eq!(report.personal_cache_reversed_code_text_tokens, 1);
        assert_eq!(report.personal_cache_reversed_left_context_tokens, 1);
        assert_eq!(report.personal_cache_revision_events_with_reversal, 1);
    }

    #[test]
    fn shared_personal_pool_matches_the_independent_word_cache_path() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let mut state = PersonalCacheReplayState::new();
        state.learn_commit(0, 1, &["在".to_owned()]);
        state.learn_commit(1, 2, &["猫猫".to_owned()]);
        let mut cache = HashMap::new();
        let pool = decode_personal_pool_memoized(&decoder, &mut cache, "zlmkmk").unwrap();
        let mut shared_stats = ReplayStrategyStats::default();
        let shared = observe_personal_strategy_from_pool(
            &pool,
            &state,
            PersonalCacheKind::WordFrequency,
            "zlmkmk",
            "在猫猫",
            &mut shared_stats,
        );
        let mut independent_stats = ReplayStrategyStats::default();
        let independent = observe_strategy_with_personal_cache(
            &decoder,
            &state,
            PersonalCacheKind::WordFrequency,
            "zlmkmk",
            "在猫猫",
            &mut independent_stats,
        )
        .unwrap();

        assert_eq!(shared, independent);
        assert_eq!(shared_stats, independent_stats);
        assert_eq!(cache.len(), 1);
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
        let ordinary_rank = decoder
            .decode_sentence("zlmkmk", super::REPLAY_TOP_K)
            .unwrap()
            .iter()
            .position(|candidate| candidate.text == "在猫猫")
            .map(|rank| rank + 1);

        assert_eq!(word_rank.unigram, ordinary_rank);
        assert_eq!(word_rank.unigram, pair_rank.unigram);
        assert_eq!(word_rank.personal, Some(3));
        assert_eq!(pair_rank.personal, Some(2));
        assert!(pair_rank.personal < word_rank.personal);
        assert_eq!(pair_stats.hits_at_5, 1);
    }

    #[test]
    fn reserved_pair_slot_distinguishes_one_observation_from_same_pair_repetition() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let pool = decoder
            .decode_sentence("zlmkmk", super::PERSONAL_CACHE_POOL_DEPTH)
            .unwrap();
        let candidate = pool
            .iter()
            .find(|candidate| candidate.text == "在猫猫")
            .unwrap();
        let words = ["在".to_owned(), "猫猫".to_owned()];
        let mut state = PersonalCacheReplayState::new();
        state.learn_commit(0, 1, &["在".to_owned()]);
        state.learn_commit(1, 2, &["猫猫".to_owned()]);
        assert_eq!(state.learn_pair_sequence(0, 3, &words), 1);

        assert!(candidate_has_pair_with_minimum_count(candidate, &state, 1));
        assert!(!candidate_has_pair_with_minimum_count(candidate, &state, 2));

        assert_eq!(state.learn_pair_sequence(3, 6, &words), 1);
        assert!(candidate_has_pair_with_minimum_count(candidate, &state, 2));
        let evidence = personal_reserved_pair_evidence(candidate, &state, "zlmkmk", 2, true);
        assert_eq!(evidence.prior_pair_occurrences, 2);
        assert!(evidence.promotion <= super::PERSONAL_CACHE_MAX_PROMOTION);
    }

    #[test]
    fn bounded_personal_promotion_has_an_exact_candidate_pool_depth() {
        assert_eq!(
            super::PERSONAL_CACHE_POOL_DEPTH,
            super::REPLAY_TOP_K + super::PERSONAL_CACHE_MAX_PROMOTION
        );
        let best_omitted_adjusted_rank =
            super::PERSONAL_CACHE_POOL_DEPTH - super::PERSONAL_CACHE_MAX_PROMOTION;
        assert_eq!(best_omitted_adjusted_rank, super::REPLAY_TOP_K);
        for baseline_rank in 0..super::REPLAY_TOP_K {
            assert!(
                baseline_rank < best_omitted_adjusted_rank,
                "the original Top-K must remain ahead of every omitted candidate"
            );
        }
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
    fn training_only_history_preserves_learning_and_retraction_state() {
        let decoder = crate::Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());
        let history = EventCapsuleV1::new(vec![
            commit_event(100, 0, "zl", "zai", "在"),
            commit_event(200, 1, "mkmk", "mao'mao", "猫猫"),
            revision_event(300, 0, "在", ""),
            revision_event(400, 1, "猫", ""),
        ])
        .unwrap();
        let mut full_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut full_state = PersonalCacheReplayState::new();
        full_report
            .observe_capsule_with_personal_pair_cache(&decoder, &mut full_state, &history)
            .unwrap();
        let mut training_report = CapsuleReplayReport::with_window_gap_limit(Some(5_000)).unwrap();
        let mut training_state = PersonalCacheReplayState::new();
        training_report
            .learn_capsule_for_personal_cache(&decoder, &mut training_state, &history)
            .unwrap();

        assert_eq!(training_report.capsules, full_report.capsules);
        assert_eq!(training_report.events, full_report.events);
        assert_eq!(
            training_report.personal_cache_learning_commits,
            full_report.personal_cache_learning_commits
        );
        assert_eq!(
            training_report.personal_cache_learning_word_tokens,
            full_report.personal_cache_learning_word_tokens
        );
        assert_eq!(
            training_report.personal_cache_reversed_commits,
            full_report.personal_cache_reversed_commits
        );
        assert_eq!(
            training_report.personal_cache_reversed_word_tokens,
            full_report.personal_cache_reversed_word_tokens
        );
        assert_eq!(
            training_report.personal_cache_learning_pair_sequences,
            full_report.personal_cache_learning_pair_sequences
        );
        assert_eq!(
            training_report.personal_cache_learning_word_pairs,
            full_report.personal_cache_learning_word_pairs
        );
        assert_eq!(
            training_report.personal_cache_reversed_pair_sequences,
            full_report.personal_cache_reversed_pair_sequences
        );
        assert_eq!(
            training_report.personal_cache_reversed_word_pairs,
            full_report.personal_cache_reversed_word_pairs
        );
        assert_eq!(
            training_report.personal_cache_revision_events_with_reversal,
            full_report.personal_cache_revision_events_with_reversal
        );
        assert_eq!(
            training_report.personal_cache_revisions_not_reversed,
            full_report.personal_cache_revisions_not_reversed
        );
        assert_eq!(format!("{training_state:?}"), format!("{full_state:?}"));
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
