//! Explainable decoder experiments for Ziranma double-pinyin key sequences.
//!
//! The current research baseline supports full-code and mixed-abbreviation
//! spellings, together with at most one local key error. It deliberately uses
//! a prebuilt spelling trie and inspectable local language scores.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

mod codec;
mod evaluation;
mod language_model;

pub use codec::{EncodedPinyin, PinyinEncodeError, encode_pinyin_phrase, encode_pinyin_syllable};
pub use evaluation::{
    EvaluationReport, RecallMetrics, SentenceCaseParseError, SentenceCaseReport, SyntheticCaseKind,
    evaluate_sentence_cases, evaluate_synthetic,
};
pub use language_model::{BigramLanguageModel, BigramScore, LanguageModelParseError};

const BIGRAM_INTERPOLATION_WEIGHT: f64 = 0.65;

/// A validated, lowercase ASCII key sequence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeySequence(String);

impl KeySequence {
    /// Validates and constructs a key sequence.
    pub fn new(value: impl Into<String>) -> Result<Self, KeySequenceError> {
        let value = value.into();
        if value.is_empty()
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_lowercase())
        {
            return Err(KeySequenceError { value });
        }
        Ok(Self(value))
    }

    /// Returns the underlying key string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Error returned for an empty or non-lowercase-ASCII key sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeySequenceError {
    value: String,
}

impl fmt::Display for KeySequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "按键串必须是非空的小写英文字母，实际收到 {:?}",
            self.value
        )
    }
}

impl Error for KeySequenceError {}

/// One entry in the deliberately small public lexicon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexiconEntry {
    /// Candidate Chinese text.
    pub text: String,
    /// Space-separated full pinyin, kept for auditability.
    pub pinyin: String,
    /// Canonical full Ziranma key sequence.
    pub code: KeySequence,
    /// One canonical two-key code per pinyin syllable.
    pub syllable_codes: Vec<KeySequence>,
    /// Synthetic relative weight, not a measured corpus count.
    pub frequency: u64,
}

/// A supported relationship between observed and intended keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Correction {
    /// The observed keys exactly match the spelling interpretation.
    Exact,
    /// One intended key was observed as a physical QWERTY neighbor.
    NeighborSubstitution {
        /// Zero-based byte index in the ASCII key sequence.
        index: usize,
        /// Key prescribed by the spelling interpretation.
        intended: char,
        /// Key actually observed.
        actual: char,
    },
    /// Two adjacent key presses arrived in reverse order.
    AdjacentTransposition {
        /// Zero-based index of the first intended key.
        start: usize,
        /// First key in the intended order.
        intended_left: char,
        /// Second key in the intended order.
        intended_right: char,
    },
    /// One intended key never arrived.
    MissingKey {
        /// Zero-based position in the intended sequence.
        index: usize,
        /// Key that should have appeared.
        intended: char,
    },
    /// One unintended key arrived in the observed sequence.
    ExtraKey {
        /// Zero-based position in the observed sequence.
        index: usize,
        /// Unintended observed key.
        actual: char,
    },
}

impl Correction {
    /// Returns a concise, human-readable explanation.
    pub fn description(&self) -> String {
        match self {
            Self::Exact => "原样匹配，没有纠错".to_owned(),
            Self::NeighborSubstitution {
                index,
                intended,
                actual,
            } => format!(
                "第 {} 键发生邻键替换：本想按 {intended}，实际按到 {actual}",
                index + 1
            ),
            Self::AdjacentTransposition {
                start,
                intended_left,
                intended_right,
            } => format!(
                "第 {}、{} 键顺序颠倒：原顺序为 {intended_left}{intended_right}",
                start + 1,
                start + 2
            ),
            Self::MissingKey { index, intended } => {
                format!("第 {} 键遗漏：本应按 {intended}", index + 1)
            }
            Self::ExtraKey { index, actual } => {
                format!("第 {} 键多按：多出了 {actual}", index + 1)
            }
        }
    }
}

/// One full-code or mixed-abbreviation interpretation of a lexicon entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spelling {
    /// Key sequence used by this interpretation.
    pub code: KeySequence,
    /// Zero-based syllable indices represented by only their first key.
    pub abbreviated_syllables: Vec<usize>,
}

impl Spelling {
    /// Describes which syllables used one-key abbreviation.
    pub fn description(&self) -> String {
        if self.abbreviated_syllables.is_empty() {
            return "全部音节使用完整双拼".to_owned();
        }
        let positions = self
            .abbreviated_syllables
            .iter()
            .map(|index| (index + 1).to_string())
            .collect::<Vec<_>>()
            .join("、");
        format!("第 {positions} 个音节使用一键简拼")
    }
}

/// Components used to rank a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreBreakdown {
    /// Natural logarithm of the entry's synthetic relative frequency.
    pub frequency: f64,
    /// Cost associated with the detected key correction.
    pub correction_penalty: f64,
    /// Cost associated with one-key syllable abbreviations.
    pub abbreviation_penalty: f64,
    /// `frequency - correction_penalty - abbreviation_penalty`.
    pub total: f64,
}

/// A decoded candidate together with its complete explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// Candidate Chinese text.
    pub text: String,
    /// Full pinyin recorded in the public fixture.
    pub pinyin: String,
    /// Canonical full Ziranma code.
    pub code: KeySequence,
    /// Full-code or mixed-abbreviation spelling matched by the decoder.
    pub spelling: Spelling,
    /// How the observed input relates to the matched spelling.
    pub correction: Correction,
    /// Transparent score components.
    pub score: ScoreBreakdown,
}

/// One word-level decision inside a multi-word decoding path.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceSegment {
    /// Slice of observed keys consumed by this segment.
    pub observed: KeySequence,
    /// Word candidate and its local spelling/error explanation.
    pub candidate: Candidate,
    /// Context-sensitive language evidence for this segment.
    pub language_score: SentenceLanguageScore,
}

/// Explainable unigram/bigram interpolation for one sentence segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SentenceLanguageScore {
    /// `ln P(word)` from normalized synthetic lexicon weights.
    pub unigram_log_probability: f64,
    /// Conditional evidence when a previous word and bigram model exist.
    pub bigram: Option<BigramScore>,
    /// Language log score used by the dynamic program.
    pub interpolated_log_probability: f64,
}

/// A complete segmentation of an unseparated key sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct SentenceCandidate {
    /// Concatenated Chinese output.
    pub text: String,
    /// Ordered word decisions and consumed key slices.
    pub segments: Vec<SentenceSegment>,
    /// Sum of normalized unigram log probabilities and local penalties.
    pub total_score: f64,
    /// Whether the complete path consumed the global error budget.
    pub used_error: bool,
}

/// Tunable penalties for the research decoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeConfig {
    /// Penalty for one QWERTY-neighbor substitution.
    pub neighbor_substitution_penalty: f64,
    /// Penalty for one adjacent transposition.
    pub adjacent_transposition_penalty: f64,
    /// Penalty for one missing intended key.
    pub missing_key_penalty: f64,
    /// Penalty for one extra observed key.
    pub extra_key_penalty: f64,
    /// Penalty for each syllable represented by only its first key.
    pub abbreviation_penalty_per_syllable: f64,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            neighbor_substitution_penalty: 2.75,
            adjacent_transposition_penalty: 2.25,
            missing_key_penalty: 3.00,
            extra_key_penalty: 3.00,
            abbreviation_penalty_per_syllable: 0.85,
        }
    }
}

/// Trie-backed decoder over a local lexicon.
#[derive(Clone, Debug)]
pub struct Decoder {
    lexicon: Vec<LexiconEntry>,
    trie: SpellingTrie,
    language_model: Option<BigramLanguageModel>,
    config: DecodeConfig,
}

impl Decoder {
    /// Creates a decoder with conservative default penalties.
    pub fn new(lexicon: Vec<LexiconEntry>) -> Self {
        let trie = SpellingTrie::new(&lexicon);
        Self {
            lexicon,
            trie,
            language_model: None,
            config: DecodeConfig::default(),
        }
    }

    /// Creates a decoder with explicit penalties.
    ///
    /// # Panics
    ///
    /// Panics if any penalty is negative or non-finite. Configuration is
    /// programmer-owned in this milestone rather than loaded from user input.
    pub fn with_config(lexicon: Vec<LexiconEntry>, config: DecodeConfig) -> Self {
        let penalties = [
            config.neighbor_substitution_penalty,
            config.adjacent_transposition_penalty,
            config.missing_key_penalty,
            config.extra_key_penalty,
            config.abbreviation_penalty_per_syllable,
        ];
        assert!(
            penalties
                .iter()
                .all(|penalty| penalty.is_finite() && *penalty >= 0.0),
            "all penalties must be finite and non-negative"
        );
        let trie = SpellingTrie::new(&lexicon);
        Self {
            lexicon,
            trie,
            language_model: None,
            config,
        }
    }

    /// Attaches a local bigram model for context-sensitive sentence ranking.
    pub fn with_bigram_model(mut self, language_model: BigramLanguageModel) -> Self {
        self.language_model = Some(language_model);
        self
    }

    /// Returns at most `top_k` matching candidates in deterministic score order.
    pub fn decode(&self, observed: &str, top_k: usize) -> Result<Vec<Candidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 {
            return Ok(Vec::new());
        }

        let mut candidates = self.lookup_candidates(observed.as_str(), true);

        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        Ok(candidates)
    }

    /// Jointly infers word boundaries, mixed abbreviations, and at most one
    /// local key error across the complete sequence.
    ///
    /// The search uses dynamic programming by observed-key position and a
    /// global one-error budget. A bounded beam keeps the research baseline
    /// responsive while preserving deterministic ordering.
    pub fn decode_sentence(
        &self,
        observed: &str,
        top_k: usize,
    ) -> Result<Vec<SentenceCandidate>, KeySequenceError> {
        let observed = KeySequence::new(observed)?;
        if top_k == 0 || self.lexicon.is_empty() {
            return Ok(Vec::new());
        }

        let length = observed.as_str().len();
        let beam_width = top_k.saturating_mul(8).clamp(32, 512);
        let log_frequency_total = self
            .lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>()
            .ln();
        let mut without_error = vec![Vec::<PartialSentence>::new(); length + 1];
        let mut with_error = vec![Vec::<PartialSentence>::new(); length + 1];
        without_error[0].push(PartialSentence {
            text: String::new(),
            segments: Vec::new(),
            total_score: 0.0,
        });

        for start in 0..length {
            prune_partial_paths(&mut without_error[start], beam_width);
            prune_partial_paths(&mut with_error[start], beam_width);

            let unused_paths = without_error[start].clone();
            if !unused_paths.is_empty() {
                for transition in self.segment_transitions(observed.as_str(), start, true) {
                    let target_states = if transition.uses_error {
                        &mut with_error
                    } else {
                        &mut without_error
                    };
                    extend_paths(
                        &unused_paths,
                        &transition,
                        log_frequency_total,
                        self.language_model.as_ref(),
                        &mut target_states[transition.end],
                    );
                }
            }

            let used_paths = with_error[start].clone();
            if !used_paths.is_empty() {
                for transition in self.segment_transitions(observed.as_str(), start, false) {
                    extend_paths(
                        &used_paths,
                        &transition,
                        log_frequency_total,
                        self.language_model.as_ref(),
                        &mut with_error[transition.end],
                    );
                }
            }
        }

        let mut complete = without_error[length]
            .drain(..)
            .map(|path| SentenceCandidate {
                text: path.text,
                segments: path.segments,
                total_score: path.total_score,
                used_error: false,
            })
            .collect::<Vec<_>>();
        complete.extend(with_error[length].drain(..).map(|path| SentenceCandidate {
            text: path.text,
            segments: path.segments,
            total_score: path.total_score,
            used_error: true,
        }));
        complete.sort_by(sentence_order);

        let mut seen_text = HashSet::new();
        complete.retain(|candidate| seen_text.insert(candidate.text.clone()));
        complete.truncate(top_k);
        Ok(complete)
    }

    fn segment_transitions(
        &self,
        observed: &str,
        start: usize,
        allow_error: bool,
    ) -> Vec<SegmentTransition> {
        let mut transitions = Vec::new();
        let maximum_length = self.trie.maximum_code_length + usize::from(allow_error);
        let remaining = observed.len() - start;
        for observed_length in 1..=maximum_length.min(remaining) {
            let end = start + observed_length;
            let observed_segment = &observed[start..end];
            for candidate in self.lookup_candidates(observed_segment, allow_error) {
                let transition = SegmentTransition {
                    end,
                    uses_error: candidate.correction != Correction::Exact,
                    observed: KeySequence::new(observed_segment)
                        .expect("a sentence slice is lowercase ASCII"),
                    candidate,
                };
                upsert_transition(&mut transitions, transition);
            }
        }
        transitions
    }

    fn lookup_candidates(&self, observed: &str, allow_error: bool) -> Vec<Candidate> {
        let mut best_by_entry = HashMap::<usize, Candidate>::new();
        for hypothesis in key_hypotheses(observed, allow_error) {
            for terminal in self.trie.lookup(&hypothesis.code) {
                let entry = &self.lexicon[terminal.entry_index];
                let candidate = self.make_candidate(
                    entry,
                    terminal.spelling.clone(),
                    hypothesis.correction.clone(),
                );
                match best_by_entry.entry(terminal.entry_index) {
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(candidate);
                    }
                    std::collections::hash_map::Entry::Occupied(mut slot) => {
                        if candidate_order(&candidate, slot.get()) == Ordering::Less {
                            slot.insert(candidate);
                        }
                    }
                }
            }
        }
        let mut candidates = best_by_entry.into_values().collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates
    }

    fn make_candidate(
        &self,
        entry: &LexiconEntry,
        spelling: Spelling,
        correction: Correction,
    ) -> Candidate {
        let frequency = (entry.frequency as f64).ln();
        let correction_penalty = self.correction_penalty(&correction);
        let abbreviation_penalty = spelling.abbreviated_syllables.len() as f64
            * self.config.abbreviation_penalty_per_syllable;
        Candidate {
            text: entry.text.clone(),
            pinyin: entry.pinyin.clone(),
            code: entry.code.clone(),
            spelling,
            correction,
            score: ScoreBreakdown {
                frequency,
                correction_penalty,
                abbreviation_penalty,
                total: frequency - correction_penalty - abbreviation_penalty,
            },
        }
    }

    fn correction_penalty(&self, correction: &Correction) -> f64 {
        match correction {
            Correction::Exact => 0.0,
            Correction::NeighborSubstitution { .. } => self.config.neighbor_substitution_penalty,
            Correction::AdjacentTransposition { .. } => self.config.adjacent_transposition_penalty,
            Correction::MissingKey { .. } => self.config.missing_key_penalty,
            Correction::ExtraKey { .. } => self.config.extra_key_penalty,
        }
    }
}

#[derive(Clone, Debug)]
struct SpellingTrie {
    nodes: Vec<TrieNode>,
    maximum_code_length: usize,
}

impl SpellingTrie {
    fn new(lexicon: &[LexiconEntry]) -> Self {
        let mut trie = Self {
            nodes: vec![TrieNode::new()],
            maximum_code_length: 0,
        };
        for (entry_index, entry) in lexicon.iter().enumerate() {
            for spelling in spelling_variants(&entry.syllable_codes) {
                trie.insert(entry_index, spelling);
            }
        }
        trie
    }

    fn insert(&mut self, entry_index: usize, spelling: Spelling) {
        self.maximum_code_length = self.maximum_code_length.max(spelling.code.as_str().len());
        let mut node_index = 0;
        for key in spelling.code.as_str().bytes() {
            let child_slot = (key - b'a') as usize;
            node_index = match self.nodes[node_index].children[child_slot] {
                Some(child) => child,
                None => {
                    let child = self.nodes.len();
                    self.nodes.push(TrieNode::new());
                    self.nodes[node_index].children[child_slot] = Some(child);
                    child
                }
            };
        }
        self.nodes[node_index].terminals.push(TrieTerminal {
            entry_index,
            spelling,
        });
    }

    fn lookup(&self, code: &str) -> &[TrieTerminal] {
        let mut node_index = 0;
        for key in code.bytes() {
            let child_slot = (key - b'a') as usize;
            let Some(child) = self.nodes[node_index].children[child_slot] else {
                return &[];
            };
            node_index = child;
        }
        &self.nodes[node_index].terminals
    }
}

#[derive(Clone, Debug)]
struct TrieNode {
    children: [Option<usize>; 26],
    terminals: Vec<TrieTerminal>,
}

impl TrieNode {
    fn new() -> Self {
        Self {
            children: [None; 26],
            terminals: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct TrieTerminal {
    entry_index: usize,
    spelling: Spelling,
}

#[derive(Clone, Debug)]
struct KeyHypothesis {
    code: String,
    correction: Correction,
}

fn key_hypotheses(observed: &str, allow_error: bool) -> Vec<KeyHypothesis> {
    let observed_bytes = observed.as_bytes();
    let mut hypotheses = vec![KeyHypothesis {
        code: observed.to_owned(),
        correction: Correction::Exact,
    }];
    if !allow_error {
        return hypotheses;
    }

    for index in 0..observed_bytes.len() {
        for intended in b'a'..=b'z' {
            if are_qwerty_neighbors(intended, observed_bytes[index]) {
                let mut code = observed_bytes.to_vec();
                code[index] = intended;
                hypotheses.push(KeyHypothesis {
                    code: String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                    correction: Correction::NeighborSubstitution {
                        index,
                        intended: intended as char,
                        actual: observed_bytes[index] as char,
                    },
                });
            }
        }
    }

    for start in 0..observed_bytes.len().saturating_sub(1) {
        if observed_bytes[start] != observed_bytes[start + 1] {
            let mut code = observed_bytes.to_vec();
            code.swap(start, start + 1);
            hypotheses.push(KeyHypothesis {
                correction: Correction::AdjacentTransposition {
                    start,
                    intended_left: code[start] as char,
                    intended_right: code[start + 1] as char,
                },
                code: String::from_utf8(code).expect("generated keys are lowercase ASCII"),
            });
        }
    }

    let mut missing_by_code = BTreeMap::new();
    for index in 0..=observed_bytes.len() {
        for intended in b'a'..=b'z' {
            let mut code = observed_bytes.to_vec();
            code.insert(index, intended);
            missing_by_code.insert(
                String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                Correction::MissingKey {
                    index,
                    intended: intended as char,
                },
            );
        }
    }
    hypotheses.extend(
        missing_by_code
            .into_iter()
            .map(|(code, correction)| KeyHypothesis { code, correction }),
    );

    if observed_bytes.len() > 1 {
        let mut extra_by_code = BTreeMap::new();
        for index in 0..observed_bytes.len() {
            let mut code = observed_bytes.to_vec();
            code.remove(index);
            extra_by_code.insert(
                String::from_utf8(code).expect("generated keys are lowercase ASCII"),
                Correction::ExtraKey {
                    index,
                    actual: observed_bytes[index] as char,
                },
            );
        }
        hypotheses.extend(
            extra_by_code
                .into_iter()
                .map(|(code, correction)| KeyHypothesis { code, correction }),
        );
    }

    hypotheses
}

#[derive(Clone, Debug)]
struct PartialSentence {
    text: String,
    segments: Vec<SentenceSegment>,
    total_score: f64,
}

#[derive(Clone, Debug)]
struct SegmentTransition {
    end: usize,
    uses_error: bool,
    observed: KeySequence,
    candidate: Candidate,
}

fn extend_paths(
    paths: &[PartialSentence],
    transition: &SegmentTransition,
    log_frequency_total: f64,
    language_model: Option<&BigramLanguageModel>,
    destination: &mut Vec<PartialSentence>,
) {
    for path in paths {
        let mut text = path.text.clone();
        text.push_str(&transition.candidate.text);

        let unigram_log_probability = transition.candidate.score.frequency - log_frequency_total;
        let bigram = path.segments.last().and_then(|previous| {
            language_model
                .map(|model| model.score(&previous.candidate.text, &transition.candidate.text))
        });
        let interpolated_log_probability = bigram.map_or(unigram_log_probability, |bigram| {
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram_log_probability
                + BIGRAM_INTERPOLATION_WEIGHT * bigram.log_probability
        });
        let language_score = SentenceLanguageScore {
            unigram_log_probability,
            bigram,
            interpolated_log_probability,
        };

        let mut segments = path.segments.clone();
        segments.push(SentenceSegment {
            observed: transition.observed.clone(),
            candidate: transition.candidate.clone(),
            language_score,
        });
        destination.push(PartialSentence {
            text,
            segments,
            total_score: path.total_score + interpolated_log_probability
                - transition.candidate.score.abbreviation_penalty
                - transition.candidate.score.correction_penalty,
        });
    }
}

fn upsert_transition(transitions: &mut Vec<SegmentTransition>, candidate: SegmentTransition) {
    let duplicate = transitions.iter_mut().find(|existing| {
        existing.end == candidate.end
            && existing.uses_error == candidate.uses_error
            && existing.candidate.text == candidate.candidate.text
            && existing.candidate.code == candidate.candidate.code
    });
    if let Some(existing) = duplicate {
        if candidate_order(&candidate.candidate, &existing.candidate) == Ordering::Less {
            *existing = candidate;
        }
    } else {
        transitions.push(candidate);
    }
}

fn prune_partial_paths(paths: &mut Vec<PartialSentence>, beam_width: usize) {
    paths.sort_by(partial_sentence_order);
    let mut seen_state = HashSet::new();
    paths.retain(|path| {
        let previous_word = path
            .segments
            .last()
            .map(|segment| segment.candidate.text.as_str())
            .unwrap_or("");
        seen_state.insert((path.text.clone(), previous_word.to_owned()))
    });
    paths.truncate(beam_width);
}

fn partial_sentence_order(left: &PartialSentence, right: &PartialSentence) -> Ordering {
    right
        .total_score
        .total_cmp(&left.total_score)
        .then_with(|| left.segments.len().cmp(&right.segments.len()))
        .then_with(|| left.text.cmp(&right.text))
}

fn sentence_order(left: &SentenceCandidate, right: &SentenceCandidate) -> Ordering {
    left.used_error
        .cmp(&right.used_error)
        .then_with(|| right.total_score.total_cmp(&left.total_score))
        .then_with(|| left.segments.len().cmp(&right.segments.len()))
        .then_with(|| left.text.cmp(&right.text))
}

fn candidate_order(left: &Candidate, right: &Candidate) -> Ordering {
    right
        .score
        .total
        .total_cmp(&left.score.total)
        .then_with(|| {
            left.spelling
                .abbreviated_syllables
                .len()
                .cmp(&right.spelling.abbreviated_syllables.len())
        })
        .then_with(|| correction_rank(&left.correction).cmp(&correction_rank(&right.correction)))
        .then_with(|| left.text.cmp(&right.text))
        .then_with(|| {
            left.spelling
                .code
                .as_str()
                .cmp(right.spelling.code.as_str())
        })
}

fn correction_rank(correction: &Correction) -> u8 {
    match correction {
        Correction::Exact => 0,
        Correction::AdjacentTransposition { .. } => 1,
        Correction::NeighborSubstitution { .. } => 2,
        Correction::MissingKey { .. } => 3,
        Correction::ExtraKey { .. } => 4,
    }
}

pub(crate) fn spelling_variants(syllable_codes: &[KeySequence]) -> Vec<Spelling> {
    let mut variants = vec![(String::new(), Vec::new())];
    for (syllable_index, syllable_code) in syllable_codes.iter().enumerate() {
        let mut next = Vec::with_capacity(variants.len() * 2);
        for (raw_code, abbreviated_syllables) in variants {
            let mut full_code = raw_code.clone();
            full_code.push_str(syllable_code.as_str());
            next.push((full_code, abbreviated_syllables.clone()));

            let mut abbreviated_code = raw_code;
            abbreviated_code.push(
                syllable_code
                    .as_str()
                    .chars()
                    .next()
                    .expect("a syllable code is non-empty"),
            );
            let mut abbreviated = abbreviated_syllables;
            abbreviated.push(syllable_index);
            next.push((abbreviated_code, abbreviated));
        }
        variants = next;
    }

    variants
        .into_iter()
        .map(|(code, abbreviated_syllables)| Spelling {
            code: KeySequence::new(code).expect("syllable variants are lowercase ASCII"),
            abbreviated_syllables,
        })
        .collect()
}

/// Parses the repository's auditable tab-separated demo lexicon format.
///
/// The first non-comment row must be:
/// `text<TAB>pinyin<TAB>frequency`.
pub fn parse_lexicon_tsv(contents: &str) -> Result<Vec<LexiconEntry>, LexiconParseError> {
    const EXPECTED_HEADER: [&str; 3] = ["text", "pinyin", "frequency"];
    const MAX_SYLLABLES: usize = 12;

    let mut saw_header = false;
    let mut entries = Vec::new();
    let mut duplicates = HashSet::new();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let line_number = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields = line.split('\t').collect::<Vec<_>>();
        if !saw_header {
            if fields != EXPECTED_HEADER {
                return Err(LexiconParseError::InvalidHeader { line_number });
            }
            saw_header = true;
            continue;
        }

        if fields.len() != EXPECTED_HEADER.len() || fields.iter().any(|field| field.is_empty()) {
            return Err(LexiconParseError::InvalidRow { line_number });
        }

        let frequency =
            fields[2]
                .parse::<u64>()
                .map_err(|_| LexiconParseError::InvalidFrequency {
                    line_number,
                    value: fields[2].to_owned(),
                })?;
        if frequency == 0 {
            return Err(LexiconParseError::InvalidFrequency {
                line_number,
                value: fields[2].to_owned(),
            });
        }

        let encoded =
            encode_pinyin_phrase(fields[1]).map_err(|_| LexiconParseError::InvalidPinyin {
                line_number,
                value: fields[1].to_owned(),
            })?;
        if encoded.syllable_codes.len() > MAX_SYLLABLES {
            return Err(LexiconParseError::TooManySyllables {
                line_number,
                count: encoded.syllable_codes.len(),
                maximum: MAX_SYLLABLES,
            });
        }

        let duplicate_key = (fields[0].to_owned(), encoded.full_code.clone());
        if !duplicates.insert(duplicate_key) {
            return Err(LexiconParseError::DuplicateEntry { line_number });
        }

        entries.push(LexiconEntry {
            text: fields[0].to_owned(),
            pinyin: fields[1].to_owned(),
            code: encoded.full_code,
            syllable_codes: encoded.syllable_codes,
            frequency,
        });
    }

    if !saw_header {
        return Err(LexiconParseError::MissingHeader);
    }
    if entries.is_empty() {
        return Err(LexiconParseError::Empty);
    }

    Ok(entries)
}

/// Error returned while parsing a public lexicon fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LexiconParseError {
    /// No non-comment header row was found.
    MissingHeader,
    /// The header did not match the documented three columns.
    InvalidHeader {
        /// One-based source line number.
        line_number: usize,
    },
    /// A data row had missing, extra, or empty fields.
    InvalidRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A frequency was not a positive integer.
    InvalidFrequency {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// A pinyin phrase could not be encoded.
    InvalidPinyin {
        /// One-based source line number.
        line_number: usize,
        /// Invalid source value.
        value: String,
    },
    /// A row would create too many exhaustive abbreviation variants.
    TooManySyllables {
        /// One-based source line number.
        line_number: usize,
        /// Actual number of syllables.
        count: usize,
        /// Accepted maximum.
        maximum: usize,
    },
    /// The same text and code appeared more than once.
    DuplicateEntry {
        /// One-based source line number.
        line_number: usize,
    },
    /// A valid header was present but no entries followed.
    Empty,
}

impl fmt::Display for LexiconParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(formatter, "词典缺少表头"),
            Self::InvalidHeader { line_number } => {
                write!(formatter, "词典第 {line_number} 行表头无效")
            }
            Self::InvalidRow { line_number } => {
                write!(formatter, "词典第 {line_number} 行字段无效")
            }
            Self::InvalidFrequency { line_number, value } => write!(
                formatter,
                "词典第 {line_number} 行的频率必须是正整数，实际为 {value:?}"
            ),
            Self::InvalidPinyin { line_number, value } => write!(
                formatter,
                "词典第 {line_number} 行的拼音无法编码，实际为 {value:?}"
            ),
            Self::TooManySyllables {
                line_number,
                count,
                maximum,
            } => write!(
                formatter,
                "词典第 {line_number} 行有 {count} 个音节，穷举基线最多接受 {maximum} 个"
            ),
            Self::DuplicateEntry { line_number } => {
                write!(formatter, "词典第 {line_number} 行重复")
            }
            Self::Empty => write!(formatter, "词典没有数据行"),
        }
    }
}

impl Error for LexiconParseError {}

#[cfg(test)]
fn detect_correction(observed: &str, intended: &str) -> Option<Correction> {
    if observed.len() + 1 == intended.len() {
        let index = single_removed_index(observed.as_bytes(), intended.as_bytes())?;
        return Some(Correction::MissingKey {
            index,
            intended: intended.as_bytes()[index] as char,
        });
    }
    if observed.len() == intended.len() + 1 {
        let index = single_removed_index(intended.as_bytes(), observed.as_bytes())?;
        return Some(Correction::ExtraKey {
            index,
            actual: observed.as_bytes()[index] as char,
        });
    }
    if observed.len() != intended.len() {
        return None;
    }

    let observed = observed.as_bytes();
    let intended = intended.as_bytes();
    let differences = observed
        .iter()
        .zip(intended)
        .enumerate()
        .filter_map(|(index, (actual, expected))| (actual != expected).then_some(index))
        .collect::<Vec<_>>();

    match differences.as_slice() {
        [] => Some(Correction::Exact),
        [index] if are_qwerty_neighbors(intended[*index], observed[*index]) => {
            Some(Correction::NeighborSubstitution {
                index: *index,
                intended: intended[*index] as char,
                actual: observed[*index] as char,
            })
        }
        [left, right]
            if *right == *left + 1
                && observed[*left] == intended[*right]
                && observed[*right] == intended[*left] =>
        {
            Some(Correction::AdjacentTransposition {
                start: *left,
                intended_left: intended[*left] as char,
                intended_right: intended[*right] as char,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
fn single_removed_index(shorter: &[u8], longer: &[u8]) -> Option<usize> {
    if shorter.len() + 1 != longer.len() {
        return None;
    }
    let first_difference = shorter
        .iter()
        .zip(longer)
        .position(|(left, right)| left != right)
        .unwrap_or(shorter.len());
    (shorter[first_difference..] == longer[first_difference + 1..]).then_some(first_difference)
}

pub(crate) fn are_qwerty_neighbors(left: u8, right: u8) -> bool {
    match left {
        b'q' => matches!(right, b'w' | b'a'),
        b'w' => matches!(right, b'q' | b'e' | b'a' | b's'),
        b'e' => matches!(right, b'w' | b'r' | b's' | b'd'),
        b'r' => matches!(right, b'e' | b't' | b'd' | b'f'),
        b't' => matches!(right, b'r' | b'y' | b'f' | b'g'),
        b'y' => matches!(right, b't' | b'u' | b'g' | b'h'),
        b'u' => matches!(right, b'y' | b'i' | b'h' | b'j'),
        b'i' => matches!(right, b'u' | b'o' | b'j' | b'k'),
        b'o' => matches!(right, b'i' | b'p' | b'k' | b'l'),
        b'p' => matches!(right, b'o' | b'l'),
        b'a' => matches!(right, b'q' | b'w' | b's' | b'z'),
        b's' => {
            matches!(right, b'w' | b'e' | b'a' | b'd' | b'z' | b'x')
        }
        b'd' => {
            matches!(right, b'e' | b'r' | b's' | b'f' | b'x' | b'c')
        }
        b'f' => {
            matches!(right, b'r' | b't' | b'd' | b'g' | b'c' | b'v')
        }
        b'g' => {
            matches!(right, b't' | b'y' | b'f' | b'h' | b'v' | b'b')
        }
        b'h' => {
            matches!(right, b'y' | b'u' | b'g' | b'j' | b'b' | b'n')
        }
        b'j' => {
            matches!(right, b'u' | b'i' | b'h' | b'k' | b'n' | b'm')
        }
        b'k' => matches!(right, b'i' | b'o' | b'j' | b'l' | b'm'),
        b'l' => matches!(right, b'o' | b'p' | b'k'),
        b'z' => matches!(right, b'a' | b's' | b'x'),
        b'x' => matches!(right, b's' | b'd' | b'z' | b'c'),
        b'c' => matches!(right, b'd' | b'f' | b'x' | b'v'),
        b'v' => matches!(right, b'f' | b'g' | b'c' | b'b'),
        b'b' => matches!(right, b'g' | b'h' | b'v' | b'n'),
        b'n' => matches!(right, b'h' | b'j' | b'b' | b'm'),
        b'm' => matches!(right, b'j' | b'k' | b'n'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Candidate, Correction, Decoder, KeySequence, are_qwerty_neighbors, candidate_order,
        detect_correction, parse_lexicon_tsv, spelling_variants,
    };

    const FIXTURE: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

    #[test]
    fn key_sequence_accepts_only_lowercase_ascii() {
        assert!(KeySequence::new("nihk").is_ok());
        assert!(KeySequence::new("").is_err());
        assert!(KeySequence::new("NiHk").is_err());
        assert!(KeySequence::new("你好").is_err());
    }

    #[test]
    fn neighbor_map_is_symmetric() {
        for left in b'a'..=b'z' {
            for right in b'a'..=b'z' {
                assert_eq!(
                    are_qwerty_neighbors(left, right),
                    are_qwerty_neighbors(right, left),
                    "{} -> {} was not symmetric",
                    left as char,
                    right as char
                );
            }
        }
    }

    #[test]
    fn generates_all_mixed_abbreviation_variants() {
        let syllables = [
            KeySequence::new("ni").unwrap(),
            KeySequence::new("hk").unwrap(),
        ];
        let variants = spelling_variants(&syllables);
        assert_eq!(variants.len(), 4);
        assert_eq!(
            variants
                .iter()
                .map(|variant| variant.code.as_str())
                .collect::<Vec<_>>(),
            ["nihk", "nih", "nhk", "nh"]
        );
    }

    #[test]
    fn detects_all_supported_single_corrections() {
        assert_eq!(detect_correction("nihk", "nihk"), Some(Correction::Exact));
        assert!(matches!(
            detect_correction("nigk", "nihk"),
            Some(Correction::NeighborSubstitution {
                index: 2,
                intended: 'h',
                actual: 'g'
            })
        ));
        assert!(matches!(
            detect_correction("nikh", "nihk"),
            Some(Correction::AdjacentTransposition { start: 2, .. })
        ));
        assert!(matches!(
            detect_correction("nik", "nihk"),
            Some(Correction::MissingKey {
                index: 2,
                intended: 'h'
            })
        ));
        assert!(matches!(
            detect_correction("niihk", "nihk"),
            Some(Correction::ExtraKey {
                index: 2,
                actual: 'i'
            })
        ));
        assert!(detect_correction("niqk", "nihk").is_none());
        assert!(detect_correction("nifj", "nihk").is_none());
        assert!(detect_correction("ni", "nihk").is_none());
    }

    #[test]
    fn trie_lookup_matches_the_exhaustive_reference() {
        let lexicon = parse_lexicon_tsv(FIXTURE).unwrap();
        let decoder = Decoder::new(lexicon);

        for observed in [
            "nihk", "nigk", "nikh", "nik", "niihk", "nh", "ni", "zrm", "urf", "ajjp",
        ] {
            assert_eq!(
                decoder.decode(observed, 10).unwrap(),
                exhaustive_reference(&decoder, observed, 10),
                "{observed}"
            );
        }
    }

    fn exhaustive_reference(decoder: &Decoder, observed: &str, top_k: usize) -> Vec<Candidate> {
        let mut candidates = decoder
            .lexicon
            .iter()
            .filter_map(|entry| {
                spelling_variants(&entry.syllable_codes)
                    .into_iter()
                    .filter_map(|spelling| {
                        let correction = detect_correction(observed, spelling.code.as_str())?;
                        Some(decoder.make_candidate(entry, spelling, correction))
                    })
                    .min_by(candidate_order)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(candidate_order);
        candidates.truncate(top_k);
        candidates
    }
}
