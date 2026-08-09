use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    KeySequence, LabeledSentenceProbe, LexiconEntry, MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES,
    ProbeSpellingMode,
};

/// Parsed public UD corpus and auditable row accounting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UdCorpus {
    sentences: Vec<UdSentence>,
    /// Deterministic source accounting.
    pub stats: UdCorpusImportStats,
}

/// Row accounting for one CoNLL-U import.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UdCorpusImportStats {
    /// Physical source lines.
    pub source_lines: usize,
    /// Parsed sentences.
    pub sentences: usize,
    /// Integer-ID syntactic token rows.
    pub syntactic_tokens: usize,
    /// Syntactic tokens tagged as punctuation.
    pub punctuation_tokens: usize,
    /// Multiword-token or empty-node rows skipped by the probe selector.
    pub special_token_rows: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UdSentence {
    id: String,
    tokens: Vec<UdToken>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UdToken {
    form: String,
    upos: String,
}

#[derive(Clone, Debug)]
struct PendingSentence {
    id: String,
    start_line: usize,
    tokens: Vec<UdToken>,
}

/// Deterministic probes selected from public UD text using Rime pronunciations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicCalibrationSelection {
    /// Natural multi-token sentences entered with complete two-key syllables.
    pub sentence_full_code_probes: Vec<LabeledSentenceProbe>,
    /// The same natural sentences entered with one key per syllable.
    pub sentence_abbreviation_probes: Vec<LabeledSentenceProbe>,
    /// Missing-whole-token probes entered with complete two-key syllables.
    pub held_out_token_full_code_probes: Vec<LabeledSentenceProbe>,
    /// The same missing-whole-token probes entered with one key per syllable.
    pub held_out_token_abbreviation_probes: Vec<LabeledSentenceProbe>,
    /// Auditable filtering and selection counts.
    pub stats: PublicCalibrationSelectionStats,
}

/// Filtering and selection counts for public calibration probes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicCalibrationSelectionStats {
    /// UD sentences whose non-punctuation text contains 8 to 24 characters.
    pub sentence_length_eligible: usize,
    /// Length-eligible sentences containing only Han characters.
    pub sentence_han_only: usize,
    /// Han-only sentences for which Rime supplies every required reading.
    pub sentence_lexicon_coverable: usize,
    /// Sentence probes retained under the configured limit.
    pub selected_sentences: usize,
    /// Selected source tokens read through an exact complete Rime entry.
    pub selected_exact_token_uses: usize,
    /// Individual characters used when a selected source token lacked an entry.
    pub selected_character_fallback_uses: usize,
    /// Unique 2-to-4-character UD tokens absent as complete Rime entries but
    /// coverable character by character.
    pub held_out_token_eligible: usize,
    /// Held-out-token probes retained under the configured limit.
    pub selected_held_out_tokens: usize,
}

/// One natural two-word phrase entered without a word-confirmation key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousCompositionProbe {
    /// Stable upstream-derived identifier.
    pub id: String,
    /// Full two-key syllable spelling with no word separator.
    pub full_observed: KeySequence,
    /// Each word keeps its first syllable full and abbreviates its suffix.
    pub tail_abbreviated_observed: KeySequence,
    /// Tail-abbreviated input with one adjacent pair reversed.
    pub transposed_observed: KeySequence,
    /// Natural public text expected from the input.
    pub expected_text: String,
    /// The two exact Rime words used to construct the input.
    pub expected_segments: Vec<String>,
}

/// Deterministic short-phrase probes and auditable selection counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuousCompositionSelection {
    /// Selected two-word continuous-composition probes.
    pub probes: Vec<ContinuousCompositionProbe>,
    /// Filtering, selection, and key-saving counts.
    pub stats: ContinuousCompositionSelectionStats,
}

/// Selection accounting for continuous short-phrase probes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContinuousCompositionSelectionStats {
    /// Adjacent non-punctuation two-token windows examined.
    pub source_windows: usize,
    /// Windows containing 2 to 6 Han characters.
    pub han_length_eligible: usize,
    /// Eligible windows whose two complete tokens both exist in Rime.
    pub exact_word_coverable: usize,
    /// Coverable windows that save at least one key with tail abbreviation.
    pub key_saving_eligible: usize,
    /// Saving-eligible windows with a reversible distinct first-syllable pair.
    pub transposition_eligible: usize,
    /// Unique one-per-sentence representatives available before spreading.
    pub sentence_representatives: usize,
    /// Unique probes retained under the requested limit.
    pub selected: usize,
    /// Full-code keys across retained probes.
    pub selected_full_keys: usize,
    /// Tail-abbreviated keys across retained probes.
    pub selected_tail_keys: usize,
}

/// One natural adjacent-token context for evaluating a frozen candidate
/// frontier with a public static language model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicStaticContextProbe {
    /// Stable upstream-derived identifier.
    pub id: String,
    /// Complete two-key spelling assembled from both public source tokens.
    pub observed: KeySequence,
    /// Concatenated public source text expected from the input.
    pub expected_text: String,
    /// The two exact source tokens used to construct the spelling.
    pub expected_segments: Vec<String>,
}

/// Deterministic adjacent-token contexts and auditable selection counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicStaticContextSelection {
    /// Selected contexts, chosen without consulting decoder output.
    pub probes: Vec<PublicStaticContextProbe>,
    /// Filtering and selection counts.
    pub stats: PublicStaticContextSelectionStats,
}

/// Filtering counts for public static-context probes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicStaticContextSelectionStats {
    /// Adjacent non-punctuation token windows examined.
    pub source_windows: usize,
    /// Windows containing 2 to 8 Han characters.
    pub han_length_eligible: usize,
    /// Eligible windows whose two complete tokens both exist in the lexicon.
    pub exact_word_coverable: usize,
    /// Unique one-per-sentence representatives available before spreading.
    pub sentence_representatives: usize,
    /// Unique probes retained under the requested limit.
    pub selected: usize,
}

/// One natural public phrase with exactly one supplemental-only complete word.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicSupplementalCompositionProbe {
    /// Stable upstream-derived identifier.
    pub id: String,
    /// Complete two-key spelling assembled from the selected source entries.
    pub observed: KeySequence,
    /// Concatenated public source text expected from the input.
    pub expected_text: String,
    /// Ordered UD tokens used to construct the phrase.
    pub expected_segments: Vec<String>,
    /// Complete source-selected code for each corresponding segment.
    pub segment_codes: Vec<KeySequence>,
    /// Zero-based segment supplied by the supplemental lexicon.
    pub supplemental_segment_index: usize,
}

/// Deterministic public probes for the bounded mixed-layer candidate path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicSupplementalCompositionSelection {
    /// Source-selected probes. Decoder output never influences this list.
    pub probes: Vec<PublicSupplementalCompositionProbe>,
    /// Auditable filtering and selection counts.
    pub stats: PublicSupplementalCompositionSelectionStats,
}

/// Filtering and stratified-selection counts for mixed-layer probes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicSupplementalCompositionSelectionStats {
    /// Adjacent two-token and three-token windows examined.
    pub source_windows: usize,
    /// Windows with 3 to 16 Han characters and at most 16 syllables.
    pub han_length_eligible: usize,
    /// Windows with exactly one supplemental-only multi-character token and
    /// core coverage for every other token.
    pub one_supplemental_word_eligible: usize,
    /// Otherwise eligible windows already present as a whole word in either
    /// source and therefore excluded from the composition audit.
    pub whole_phrase_collisions: usize,
    /// Otherwise eligible windows whose assembled full code already names at
    /// least one whole word in either source.
    pub whole_code_collisions: usize,
    /// One-per-sentence-and-shape representatives before the sample cap.
    pub sentence_shape_representatives: usize,
    /// Representatives retained by round-robin shape sampling.
    pub selected: usize,
    /// Selected two-token phrases.
    pub selected_two_token: usize,
    /// Selected three-token phrases.
    pub selected_three_token: usize,
    /// Selected phrases whose supplemental word is first.
    pub selected_supplemental_first: usize,
    /// Selected phrases whose supplemental word is between two core words.
    pub selected_supplemental_middle: usize,
    /// Selected phrases whose supplemental word is last.
    pub selected_supplemental_last: usize,
}

/// One held-out public phrase used to compare bounded abbreviation protocols.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicProtocolProbe {
    /// Stable upstream-derived identifier.
    pub id: String,
    /// Complete two-key syllable spelling with no word separator.
    pub full_observed: KeySequence,
    /// Each source word keeps its first syllable full and abbreviates its tail.
    pub anchored_tail_observed: KeySequence,
    /// Each multi-syllable word abbreviates only its final syllable.
    pub conservative_tail_observed: KeySequence,
    /// Every syllable contributes one first key inside an explicit mode.
    pub explicit_abbreviation_observed: KeySequence,
    /// Natural public text expected from the input.
    pub expected_text: String,
    /// The two exact Rime words used to construct the input.
    pub expected_segments: Vec<String>,
    /// Whether the fit-only phrase whitelist contains this exact shortcut.
    pub whitelist_available: bool,
}

/// Deterministic fit/dev protocol probes and auditable selection counts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicProtocolSelection {
    /// Fit-only public word sequences available for auditable context models.
    pub fit_context_sequences: Vec<Vec<String>>,
    /// Held-out development phrases selected without consulting decoder output.
    pub probes: Vec<PublicProtocolProbe>,
    /// Held-out phrases containing at least one three-or-more-syllable word.
    pub long_word_probes: Vec<PublicProtocolProbe>,
    /// Fit/dev, whitelist, and key accounting.
    pub stats: PublicProtocolSelectionStats,
}

/// Accounting for the public abbreviation-protocol selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicProtocolSelectionStats {
    /// Source sentences assigned to fit by the fixed positional split.
    pub fit_source_sentences: usize,
    /// Source sentences assigned to development by the fixed positional split.
    pub dev_source_sentences: usize,
    /// Adjacent fit windows examined.
    pub fit_source_windows: usize,
    /// Fit sentences fully mapped into at least two Rime words.
    pub fit_context_sequences: usize,
    /// Rime word instances in fit-only context sequences.
    pub fit_context_words: usize,
    /// Fit windows with two exact Rime words and a key-saving tail spelling.
    pub fit_eligible_windows: usize,
    /// Distinct eligible fit phrases.
    pub fit_distinct_phrases: usize,
    /// Distinct all-short codes observed in fit.
    pub fit_shortcut_codes: usize,
    /// Fit shortcut codes associated with more than one phrase.
    pub fit_colliding_shortcut_codes: usize,
    /// Collision-free fit shortcuts observed at least twice.
    pub fit_repeated_collision_free_shortcuts: usize,
    /// Adjacent development windows examined.
    pub dev_source_windows: usize,
    /// Development windows with two exact Rime words and a key-saving tail spelling.
    pub dev_eligible_windows: usize,
    /// Unique one-per-sentence development representatives.
    pub dev_sentence_representatives: usize,
    /// Unique development representatives containing a 3+ syllable word.
    pub dev_long_word_representatives: usize,
    /// Representatives retained under the requested limit.
    pub selected: usize,
    /// Long-word representatives retained under the requested limit.
    pub selected_long_word: usize,
    /// Selected probes covered by the fit-only phrase whitelist.
    pub selected_whitelist_covered: usize,
    /// Complete-code letters across selected probes.
    pub selected_full_keys: usize,
    /// Anchored-tail letters across selected probes.
    pub selected_anchored_tail_keys: usize,
    /// Conservative one-shortening-per-word letters across selected probes.
    pub selected_conservative_tail_keys: usize,
    /// All-short letters across selected probes.
    pub selected_explicit_abbreviation_keys: usize,
    /// Complete-code letters across selected long-word probes.
    pub selected_long_word_full_keys: usize,
    /// Anchored-tail letters across selected long-word probes.
    pub selected_long_word_anchored_tail_keys: usize,
    /// Conservative-tail letters across selected long-word probes.
    pub selected_long_word_conservative_tail_keys: usize,
}

/// Train-only segmented sequences mapped into the Rime word vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicBigramTrainingCorpus {
    /// Each sequence contains at least two Rime word texts.
    pub sequences: Vec<Vec<String>>,
    /// Auditable filtering and mapping counts.
    pub stats: PublicBigramTrainingStats,
}

/// Filtering and mapping counts for public bigram training.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicBigramTrainingStats {
    /// Source sentences in the pinned train split.
    pub source_sentences: usize,
    /// Sentences whose non-punctuation text contains only Han characters.
    pub han_only_sentences: usize,
    /// Han-only sentences fully expressible with the Rime vocabulary.
    pub lexicon_coverable_sentences: usize,
    /// Coverable sequences containing at least two Rime words.
    pub training_sequences: usize,
    /// Rime word instances across retained sequences.
    pub training_words: usize,
    /// Source tokens mapped through an exact complete Rime entry.
    pub exact_token_uses: usize,
    /// Individual Rime characters used for missing complete source tokens.
    pub character_fallback_uses: usize,
}

/// Train-only Han text sequences for a boundary-independent character model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicCharacterTrainingCorpus {
    /// Non-punctuation sentence texts retained without decoder consultation.
    pub sequences: Vec<String>,
    /// Auditable filtering and size counts.
    pub stats: PublicCharacterTrainingStats,
}

/// Aggregate-only surface coverage of one public UD corpus by two lexicons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicLexiconTokenCoverageAudit {
    /// Fixed two-, three-, and four-character rows, in that order.
    pub lengths: [PublicLexiconTokenCoverageByLength; 3],
}

/// Public UD token coverage for one fixed Han-character length.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicLexiconTokenCoverageByLength {
    /// Number of Han characters in every token represented by this row.
    pub characters: usize,
    /// Distinct public UD token surfaces of this length.
    pub source_unique_tokens: usize,
    /// Public UD token occurrences of this length.
    pub source_token_instances: usize,
    /// Distinct source token surfaces present in the baseline lexicon.
    pub base_covered_unique_tokens: usize,
    /// Distinct source token surfaces present in the challenger lexicon.
    pub challenger_covered_unique_tokens: usize,
    /// Distinct source surfaces gained only by the challenger.
    pub challenger_gained_unique_tokens: usize,
    /// Distinct source surfaces lost from the baseline.
    pub challenger_lost_unique_tokens: usize,
    /// Source token occurrences covered by the baseline lexicon.
    pub base_covered_token_instances: usize,
    /// Source token occurrences covered by the challenger lexicon.
    pub challenger_covered_token_instances: usize,
    /// Source occurrences whose surface is gained only by the challenger.
    pub challenger_gained_token_instances: usize,
    /// Source occurrences whose surface is lost from the baseline.
    pub challenger_lost_token_instances: usize,
}

/// One exact public token surface paired with a source-selected full code.
///
/// The probe text remains in memory for rank comparison; aggregate audit
/// callers should avoid printing it when individual public examples are not
/// part of the report contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicLexiconRankProbe {
    /// Complete code from the highest-ranked matching public lexicon entry.
    pub observed: KeySequence,
    /// Exact public UD token surface expected from the code.
    pub expected_text: String,
    /// Number of occurrences of this surface in the selected public corpus.
    pub instances: usize,
}

/// Deterministic exact-token probes and their source accounting.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PublicLexiconRankSelection {
    /// Unique matched surfaces, ordered by surface text.
    pub probes: Vec<PublicLexiconRankProbe>,
    /// Unique eligible source surfaces before lexicon matching.
    pub source_unique_tokens: usize,
    /// Eligible source token occurrences before lexicon matching.
    pub source_token_instances: usize,
    /// Matched public token occurrences represented by `probes`.
    pub matched_token_instances: usize,
}

/// Filtering and size counts for public character-model training text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicCharacterTrainingStats {
    /// Source sentences examined.
    pub source_sentences: usize,
    /// Non-empty sentences whose non-punctuation text is entirely Han.
    pub han_only_sentences: usize,
    /// Retained sequences containing at least two characters.
    pub training_sequences: usize,
    /// Han character instances across retained sequences.
    pub training_characters: usize,
}

/// Parses the integer-token layer of a CoNLL-U corpus.
pub fn parse_ud_conllu(contents: &str) -> Result<UdCorpus, UdCorpusParseError> {
    let mut stats = UdCorpusImportStats {
        source_lines: contents.lines().count(),
        ..UdCorpusImportStats::default()
    };
    let mut current = None;
    let mut sentences = Vec::new();
    let mut identifiers = HashSet::new();

    for (zero_based_line, raw_line) in contents.lines().enumerate() {
        let line_number = zero_based_line + 1;
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            finish_sentence(&mut current, &mut sentences, &mut identifiers)?;
            continue;
        }
        if let Some(id) = line.strip_prefix("# sent_id = ") {
            if current.is_some() {
                return Err(UdCorpusParseError::MissingSentenceSeparator { line_number });
            }
            if id.is_empty() {
                return Err(UdCorpusParseError::EmptySentenceId { line_number });
            }
            current = Some(PendingSentence {
                id: id.to_owned(),
                start_line: line_number,
                tokens: Vec::new(),
            });
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 10 {
            return Err(UdCorpusParseError::InvalidTokenRow { line_number });
        }
        let Some(sentence) = current.as_mut() else {
            return Err(UdCorpusParseError::TokenOutsideSentence { line_number });
        };
        if fields[0].parse::<usize>().is_ok() {
            if fields[1].is_empty() || fields[3].is_empty() {
                return Err(UdCorpusParseError::InvalidTokenRow { line_number });
            }
            stats.syntactic_tokens += 1;
            if fields[3] == "PUNCT" {
                stats.punctuation_tokens += 1;
            }
            sentence.tokens.push(UdToken {
                form: fields[1].to_owned(),
                upos: fields[3].to_owned(),
            });
        } else if fields[0].contains('-') || fields[0].contains('.') {
            stats.special_token_rows += 1;
        } else {
            return Err(UdCorpusParseError::InvalidTokenId {
                line_number,
                value: fields[0].to_owned(),
            });
        }
    }
    finish_sentence(&mut current, &mut sentences, &mut identifiers)?;
    if sentences.is_empty() {
        return Err(UdCorpusParseError::Empty);
    }
    stats.sentences = sentences.len();
    Ok(UdCorpus { sentences, stats })
}

/// Compares two public lexicons against public UD token surfaces.
///
/// Punctuation, non-Han tokens, and lengths outside two through four are
/// excluded. The result contains only aggregate counts; pronunciation is not
/// inferred from UD, and raw frequency values are never compared across the
/// two lexicons.
pub fn audit_public_lexicon_token_coverage(
    corpus: &UdCorpus,
    base: &[LexiconEntry],
    challenger: &[LexiconEntry],
) -> PublicLexiconTokenCoverageAudit {
    let base_surfaces = base
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let challenger_surfaces = challenger
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<HashSet<_>>();
    let mut tokens_by_length =
        std::array::from_fn::<HashMap<&str, usize>, 3, _>(|_| HashMap::new());

    for token in corpus
        .sentences
        .iter()
        .flat_map(|sentence| &sentence.tokens)
    {
        if token.upos == "PUNCT" || !token.form.chars().all(is_han_character) {
            continue;
        }
        let characters = token.form.chars().count();
        let Some(length_index) = characters.checked_sub(2).filter(|&index| index < 3) else {
            continue;
        };
        *tokens_by_length[length_index]
            .entry(token.form.as_str())
            .or_default() += 1;
    }

    let lengths = std::array::from_fn(|index| {
        let tokens = std::mem::take(&mut tokens_by_length[index]);
        let mut audit = PublicLexiconTokenCoverageByLength {
            characters: index + 2,
            source_unique_tokens: tokens.len(),
            source_token_instances: tokens.values().sum(),
            ..PublicLexiconTokenCoverageByLength::default()
        };
        for (token, instances) in tokens {
            let in_base = base_surfaces.contains(token);
            let in_challenger = challenger_surfaces.contains(token);
            audit.base_covered_unique_tokens += usize::from(in_base);
            audit.challenger_covered_unique_tokens += usize::from(in_challenger);
            audit.challenger_gained_unique_tokens += usize::from(!in_base && in_challenger);
            audit.challenger_lost_unique_tokens += usize::from(in_base && !in_challenger);
            audit.base_covered_token_instances += instances * usize::from(in_base);
            audit.challenger_covered_token_instances += instances * usize::from(in_challenger);
            audit.challenger_gained_token_instances +=
                instances * usize::from(!in_base && in_challenger);
            audit.challenger_lost_token_instances +=
                instances * usize::from(in_base && !in_challenger);
        }
        audit
    });
    PublicLexiconTokenCoverageAudit { lengths }
}

/// Selects exact public token probes for one fixed Han-character length.
///
/// Punctuation and non-Han tokens are excluded. Repeated source surfaces are
/// coalesced with an occurrence count, and pronunciation/code always comes
/// from the highest-ranked matching lexicon entry rather than being inferred
/// from the UD text.
pub fn select_public_lexicon_rank_probes(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
    characters: usize,
) -> PublicLexiconRankSelection {
    let entries_by_text = best_entries_by_text(lexicon);
    let mut instances_by_text = BTreeMap::<String, usize>::new();
    for token in corpus
        .sentences
        .iter()
        .flat_map(|sentence| &sentence.tokens)
    {
        if token.upos == "PUNCT"
            || token.form.chars().count() != characters
            || !token.form.chars().all(is_han_character)
        {
            continue;
        }
        *instances_by_text.entry(token.form.clone()).or_default() += 1;
    }

    let source_unique_tokens = instances_by_text.len();
    let source_token_instances = instances_by_text.values().sum();
    let mut matched_token_instances = 0;
    let probes = instances_by_text
        .into_iter()
        .filter_map(|(text, instances)| {
            let entry = entries_by_text.get(text.as_str())?;
            matched_token_instances += instances;
            Some(PublicLexiconRankProbe {
                observed: entry.code.clone(),
                expected_text: text,
                instances,
            })
        })
        .collect();
    PublicLexiconRankSelection {
        probes,
        source_unique_tokens,
        source_token_instances,
        matched_token_instances,
    }
}

fn finish_sentence(
    current: &mut Option<PendingSentence>,
    sentences: &mut Vec<UdSentence>,
    identifiers: &mut HashSet<String>,
) -> Result<(), UdCorpusParseError> {
    let Some(sentence) = current.take() else {
        return Ok(());
    };
    if sentence.tokens.is_empty() {
        return Err(UdCorpusParseError::EmptySentence {
            line_number: sentence.start_line,
            id: sentence.id,
        });
    }
    if !identifiers.insert(sentence.id.clone()) {
        return Err(UdCorpusParseError::DuplicateSentenceId {
            line_number: sentence.start_line,
            id: sentence.id,
        });
    }
    sentences.push(UdSentence {
        id: sentence.id,
        tokens: sentence.tokens,
    });
    Ok(())
}

/// Selects public natural-sentence and missing-whole-token calibration probes.
///
/// Source order is preserved. Exact source tokens use the highest-frequency
/// matching Rime entry; absent whole tokens fall back to one deterministic
/// Rime entry per Han character. The resulting keys therefore remain
/// codec-derived and guaranteed to have at least one lexicon explanation.
pub fn select_public_calibration_cases(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
    sentence_limit: usize,
    held_out_token_limit: usize,
) -> PublicCalibrationSelection {
    let entries_by_text = best_entries_by_text(lexicon);
    let mut stats = PublicCalibrationSelectionStats::default();
    let mut sentence_full_code_probes = Vec::new();
    let mut sentence_abbreviation_probes = Vec::new();

    for sentence in &corpus.sentences {
        let source_tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        let expected_text = source_tokens
            .iter()
            .map(|token| token.form.as_str())
            .collect::<String>();
        let character_count = expected_text.chars().count();
        if !(8..=24).contains(&character_count) {
            continue;
        }
        stats.sentence_length_eligible += 1;
        if !expected_text.chars().all(is_han_character) {
            continue;
        }
        stats.sentence_han_only += 1;
        let Some((observed, expected_segments, exact_token_uses, character_fallback_uses)) =
            observed_for_tokens(&source_tokens, &entries_by_text)
        else {
            continue;
        };
        stats.sentence_lexicon_coverable += 1;
        if sentence_full_code_probes.len() < sentence_limit {
            stats.selected_exact_token_uses += exact_token_uses;
            stats.selected_character_fallback_uses += character_fallback_uses;
            sentence_full_code_probes.push(LabeledSentenceProbe {
                id: format!("{}:full", sentence.id),
                observed: KeySequence::new(observed.full_code)
                    .expect("Rime-derived full codes are lowercase ASCII"),
                expected_text: expected_text.clone(),
                expected_segments: expected_segments.clone(),
                spelling_mode: ProbeSpellingMode::FullCode,
            });
            sentence_abbreviation_probes.push(LabeledSentenceProbe {
                id: format!("{}:abbreviation", sentence.id),
                observed: KeySequence::new(observed.abbreviated_code)
                    .expect("Rime-derived abbreviations are lowercase ASCII"),
                expected_text,
                expected_segments,
                spelling_mode: ProbeSpellingMode::FullyAbbreviated,
            });
        }
    }
    stats.selected_sentences = sentence_full_code_probes.len();

    let mut held_out_token_full_code_probes = Vec::new();
    let mut held_out_token_abbreviation_probes = Vec::new();
    let mut seen_held_out_text = HashSet::new();
    for sentence in &corpus.sentences {
        for (token_index, token) in sentence.tokens.iter().enumerate() {
            let character_count = token.form.chars().count();
            if token.upos == "PUNCT"
                || !(2..=4).contains(&character_count)
                || !token.form.chars().all(is_han_character)
                || entries_by_text.contains_key(token.form.as_str())
            {
                continue;
            }
            let Some((observed, expected_segments)) =
                observed_for_characters(&token.form, &entries_by_text)
            else {
                continue;
            };
            if !seen_held_out_text.insert(token.form.clone()) {
                continue;
            }
            stats.held_out_token_eligible += 1;
            if held_out_token_full_code_probes.len() < held_out_token_limit {
                let id = format!("{}:token-{}", sentence.id, token_index + 1);
                held_out_token_full_code_probes.push(LabeledSentenceProbe {
                    id: format!("{id}:full"),
                    observed: KeySequence::new(observed.full_code)
                        .expect("Rime-derived full codes are lowercase ASCII"),
                    expected_text: token.form.clone(),
                    expected_segments: expected_segments.clone(),
                    spelling_mode: ProbeSpellingMode::FullCode,
                });
                held_out_token_abbreviation_probes.push(LabeledSentenceProbe {
                    id: format!("{id}:abbreviation"),
                    observed: KeySequence::new(observed.abbreviated_code)
                        .expect("Rime-derived abbreviations are lowercase ASCII"),
                    expected_text: token.form.clone(),
                    expected_segments,
                    spelling_mode: ProbeSpellingMode::FullyAbbreviated,
                });
            }
        }
    }
    stats.selected_held_out_tokens = held_out_token_full_code_probes.len();

    PublicCalibrationSelection {
        sentence_full_code_probes,
        sentence_abbreviation_probes,
        held_out_token_full_code_probes,
        held_out_token_abbreviation_probes,
        stats,
    }
}

/// Selects natural, exact-word, two-token spans for continuous composition.
///
/// Each word keeps its first syllable as a two-key anchor and abbreviates every
/// later syllable to one key. At most one unique span is retained per source
/// sentence, then the requested limit is spread evenly over those sentence
/// representatives. No decoder result influences selection.
pub fn select_public_continuous_composition_cases(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
    limit: usize,
) -> ContinuousCompositionSelection {
    let entries_by_text = best_entries_by_text(lexicon);
    let mut stats = ContinuousCompositionSelectionStats::default();
    let mut sentence_representatives = Vec::new();
    let mut seen_text = HashSet::new();

    for sentence in &corpus.sentences {
        let mut sentence_representative = None;
        let tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        for (window_index, window) in tokens.windows(2).enumerate() {
            stats.source_windows += 1;
            let expected_text = window
                .iter()
                .map(|token| token.form.as_str())
                .collect::<String>();
            let character_count = expected_text.chars().count();
            if !(2..=6).contains(&character_count) || !expected_text.chars().all(is_han_character) {
                continue;
            }
            stats.han_length_eligible += 1;

            let Some(first_entry) = entries_by_text.get(window[0].form.as_str()).copied() else {
                continue;
            };
            let Some(second_entry) = entries_by_text.get(window[1].form.as_str()).copied() else {
                continue;
            };
            let entries = [first_entry, second_entry];
            stats.exact_word_coverable += 1;

            let full_code = entries
                .iter()
                .map(|entry| entry.code.as_str())
                .collect::<String>();
            let tail_codes = entries
                .iter()
                .map(|entry| anchored_tail_code(entry))
                .collect::<Vec<_>>();
            let tail_code = tail_codes.concat();
            if tail_code.len() >= full_code.len() {
                continue;
            }
            stats.key_saving_eligible += 1;

            let offsets = [0, tail_codes[0].len()];
            let transposition = entries.iter().zip(&tail_codes).enumerate().rev().find_map(
                |(index, (entry, code))| {
                    if entry.syllable_codes.len() < 2 {
                        return None;
                    }
                    let bytes = code.as_bytes();
                    (bytes[0] != bytes[1]).then_some(offsets[index])
                },
            );
            let Some(transposition) = transposition else {
                continue;
            };
            stats.transposition_eligible += 1;
            if sentence_representative.is_some() || !seen_text.insert(expected_text.clone()) {
                continue;
            }

            let mut transposed = tail_code.as_bytes().to_vec();
            transposed.swap(transposition, transposition + 1);
            let transposed =
                String::from_utf8(transposed).expect("Rime-derived codes are lowercase ASCII");
            sentence_representative = Some(ContinuousCompositionProbe {
                id: format!("{}:continuous-{}", sentence.id, window_index + 1),
                full_observed: KeySequence::new(full_code)
                    .expect("Rime-derived full codes are lowercase ASCII"),
                tail_abbreviated_observed: KeySequence::new(tail_code)
                    .expect("Rime-derived tail abbreviations are lowercase ASCII"),
                transposed_observed: KeySequence::new(transposed)
                    .expect("Rime-derived transpositions are lowercase ASCII"),
                expected_text,
                expected_segments: entries.iter().map(|entry| entry.text.clone()).collect(),
            });
        }
        if let Some(probe) = sentence_representative {
            sentence_representatives.push(probe);
        }
    }

    stats.sentence_representatives = sentence_representatives.len();
    let selected = limit.min(sentence_representatives.len());
    let probes = (0..selected)
        .map(|index| {
            let spread_index = (index * sentence_representatives.len()
                + sentence_representatives.len() / 2)
                / selected;
            sentence_representatives[spread_index].clone()
        })
        .collect::<Vec<_>>();
    stats.selected = probes.len();
    stats.selected_full_keys = probes
        .iter()
        .map(|probe| probe.full_observed.as_str().len())
        .sum();
    stats.selected_tail_keys = probes
        .iter()
        .map(|probe| probe.tail_abbreviated_observed.as_str().len())
        .sum();
    ContinuousCompositionSelection { probes, stats }
}

/// Selects natural adjacent-token contexts for static language-model audits.
///
/// Both source tokens must have exact complete entries. At most one unique
/// window is retained per sentence, and the requested sample is spread over
/// those representatives. Decoder output never influences selection.
pub fn select_public_static_context_cases(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
    limit: usize,
) -> PublicStaticContextSelection {
    let entries_by_text = best_entries_by_text(lexicon);
    let mut stats = PublicStaticContextSelectionStats::default();
    let mut sentence_representatives = Vec::new();
    let mut seen_text = HashSet::new();

    for sentence in &corpus.sentences {
        let mut sentence_representative = None;
        let tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        for (window_index, window) in tokens.windows(2).enumerate() {
            stats.source_windows += 1;
            let expected_text = window
                .iter()
                .map(|token| token.form.as_str())
                .collect::<String>();
            let character_count = expected_text.chars().count();
            if !(2..=8).contains(&character_count) || !expected_text.chars().all(is_han_character) {
                continue;
            }
            stats.han_length_eligible += 1;
            let Some(first_entry) = entries_by_text.get(window[0].form.as_str()).copied() else {
                continue;
            };
            let Some(second_entry) = entries_by_text.get(window[1].form.as_str()).copied() else {
                continue;
            };
            stats.exact_word_coverable += 1;
            if sentence_representative.is_some() || !seen_text.insert(expected_text.clone()) {
                continue;
            }
            let observed = format!("{}{}", first_entry.code, second_entry.code);
            sentence_representative = Some(PublicStaticContextProbe {
                id: format!("{}:static-context-{}", sentence.id, window_index + 1),
                observed: KeySequence::new(observed)
                    .expect("lexicon-derived codes are lowercase ASCII"),
                expected_text,
                expected_segments: vec![first_entry.text.clone(), second_entry.text.clone()],
            });
        }
        if let Some(probe) = sentence_representative {
            sentence_representatives.push(probe);
        }
    }

    stats.sentence_representatives = sentence_representatives.len();
    let selected = limit.min(sentence_representatives.len());
    let probes = if selected == 0 {
        Vec::new()
    } else {
        (0..selected)
            .map(|index| {
                let spread_index = (index * sentence_representatives.len()
                    + sentence_representatives.len() / 2)
                    / selected;
                sentence_representatives[spread_index].clone()
            })
            .collect::<Vec<_>>()
    };
    stats.selected = probes.len();
    PublicStaticContextSelection { probes, stats }
}

/// Selects natural public phrases that exercise exactly one supplemental word.
///
/// Two-token and three-token windows are selected without consulting either
/// decoder's output. A token already present in the core lexicon always stays
/// a core token. Exactly one remaining token must be a multi-character exact
/// supplemental word, while every other token must have core exact coverage.
/// Whole-phrase entries in either lexicon are excluded so exact-word recall
/// cannot be mistaken for mixed-layer composition. At most one representative
/// per sentence and structural shape is retained, then five shapes are sampled
/// round-robin under `limit`.
pub fn select_public_supplemental_composition_cases(
    corpus: &UdCorpus,
    core: &[LexiconEntry],
    supplemental: &[LexiconEntry],
    limit: usize,
) -> PublicSupplementalCompositionSelection {
    const SHAPE_COUNT: usize = 5;

    let core_by_text = best_entries_by_text(core);
    let supplemental_by_text = best_entries_by_text(supplemental);
    let core_whole_codes = core
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let supplemental_whole_codes = supplemental
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<HashSet<_>>();
    let mut stats = PublicSupplementalCompositionSelectionStats::default();
    let mut representatives: [Vec<PublicSupplementalCompositionProbe>; SHAPE_COUNT] =
        std::array::from_fn(|_| Vec::new());
    let mut seen_text = HashSet::new();

    for sentence in &corpus.sentences {
        let tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        let mut represented_shapes = [false; SHAPE_COUNT];

        for window_len in [2_usize, 3] {
            for (window_index, window) in tokens.windows(window_len).enumerate() {
                stats.source_windows += 1;
                let expected_text = window
                    .iter()
                    .map(|token| token.form.as_str())
                    .collect::<String>();
                let character_count = expected_text.chars().count();
                if !(3..=MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES).contains(&character_count)
                    || !expected_text.chars().all(is_han_character)
                {
                    continue;
                }
                stats.han_length_eligible += 1;

                let mut selected_entries = Vec::with_capacity(window_len);
                let mut supplemental_segment_index = None;
                let mut coverable = true;
                for (segment_index, token) in window.iter().enumerate() {
                    if let Some(entry) = core_by_text.get(token.form.as_str()).copied() {
                        selected_entries.push(entry);
                        continue;
                    }
                    let supplemental_entry = supplemental_by_text
                        .get(token.form.as_str())
                        .copied()
                        .filter(|entry| entry.syllable_codes.len() >= 2);
                    let Some(entry) = supplemental_entry else {
                        coverable = false;
                        break;
                    };
                    if supplemental_segment_index.replace(segment_index).is_some() {
                        coverable = false;
                        break;
                    }
                    selected_entries.push(entry);
                }
                let syllable_count = selected_entries
                    .iter()
                    .map(|entry| entry.syllable_codes.len())
                    .sum::<usize>();
                if !coverable
                    || supplemental_segment_index.is_none()
                    || syllable_count > MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES
                {
                    continue;
                }
                stats.one_supplemental_word_eligible += 1;
                if core_by_text.contains_key(expected_text.as_str())
                    || supplemental_by_text.contains_key(expected_text.as_str())
                {
                    stats.whole_phrase_collisions += 1;
                    continue;
                }
                let observed = selected_entries
                    .iter()
                    .map(|entry| entry.code.as_str())
                    .collect::<String>();
                if core_whole_codes.contains(observed.as_str())
                    || supplemental_whole_codes.contains(observed.as_str())
                {
                    stats.whole_code_collisions += 1;
                    continue;
                }

                let supplemental_segment_index =
                    supplemental_segment_index.expect("one supplemental segment was checked above");
                let shape = supplemental_composition_shape(window_len, supplemental_segment_index);
                if represented_shapes[shape] || !seen_text.insert(expected_text.clone()) {
                    continue;
                }
                represented_shapes[shape] = true;
                representatives[shape].push(PublicSupplementalCompositionProbe {
                    id: format!(
                        "{}:supplemental-{window_len}-{}",
                        sentence.id,
                        window_index + 1
                    ),
                    observed: KeySequence::new(observed)
                        .expect("public lexicon codes are lowercase ASCII"),
                    expected_text,
                    expected_segments: window.iter().map(|token| token.form.clone()).collect(),
                    segment_codes: selected_entries
                        .iter()
                        .map(|entry| entry.code.clone())
                        .collect(),
                    supplemental_segment_index,
                });
            }
        }
    }

    stats.sentence_shape_representatives = representatives.iter().map(Vec::len).sum();
    let mut probes = Vec::with_capacity(limit.min(stats.sentence_shape_representatives));
    let mut positions = [0_usize; SHAPE_COUNT];
    while probes.len() < limit {
        let mut advanced = false;
        for shape in 0..SHAPE_COUNT {
            let Some(probe) = representatives[shape].get(positions[shape]).cloned() else {
                continue;
            };
            positions[shape] += 1;
            probes.push(probe);
            advanced = true;
            if probes.len() == limit {
                break;
            }
        }
        if !advanced {
            break;
        }
    }

    stats.selected = probes.len();
    for probe in &probes {
        match probe.expected_segments.len() {
            2 => stats.selected_two_token += 1,
            3 => stats.selected_three_token += 1,
            _ => unreachable!("the selector only constructs two- and three-token probes"),
        }
        if probe.supplemental_segment_index == 0 {
            stats.selected_supplemental_first += 1;
        } else if probe.supplemental_segment_index + 1 == probe.expected_segments.len() {
            stats.selected_supplemental_last += 1;
        } else {
            stats.selected_supplemental_middle += 1;
        }
    }
    PublicSupplementalCompositionSelection { probes, stats }
}

fn supplemental_composition_shape(window_len: usize, supplemental_index: usize) -> usize {
    match (window_len, supplemental_index) {
        (2, 0) => 0,
        (2, 1) => 1,
        (3, 0) => 2,
        (3, 1) => 3,
        (3, 2) => 4,
        _ => unreachable!("only two- and three-token windows are considered"),
    }
}

/// Builds a deterministic fit/dev comparison for bounded abbreviation protocols.
///
/// Every fifth source sentence is assigned to development; the other four are
/// fit-only. The fit side builds a phrase shortcut whitelist from codes that
/// map to exactly one phrase and occur at least twice. Development retains at
/// most one unique eligible two-token span per sentence and is spread evenly
/// under `limit`. Decoder output never influences splitting or selection.
pub fn select_public_protocol_audit_cases(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
    limit: usize,
) -> PublicProtocolSelection {
    const DEV_STRIDE: usize = 5;
    const MIN_SHORTCUT_OCCURRENCES: usize = 2;

    let entries_by_text = best_entries_by_text(lexicon);
    let mut stats = PublicProtocolSelectionStats::default();
    let mut fit_shortcuts = BTreeMap::<String, BTreeMap<String, usize>>::new();
    let mut fit_phrases = HashSet::new();
    let mut fit_context_sequences = Vec::new();
    let mut dev_representatives = Vec::new();
    let mut dev_long_word_representatives = Vec::new();
    let mut seen_dev_text = HashSet::new();
    let mut seen_dev_long_word_text = HashSet::new();

    for (sentence_index, sentence) in corpus.sentences.iter().enumerate() {
        let is_dev = sentence_index % DEV_STRIDE == DEV_STRIDE - 1;
        if is_dev {
            stats.dev_source_sentences += 1;
        } else {
            stats.fit_source_sentences += 1;
        }
        let tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        if !is_dev
            && let Some((_observed, words, _exact_uses, _character_fallback_uses)) =
                observed_for_tokens(&tokens, &entries_by_text)
            && words.len() >= 2
        {
            stats.fit_context_words += words.len();
            stats.fit_context_sequences += 1;
            fit_context_sequences.push(words);
        }
        let mut dev_representative = None;
        let mut dev_long_word_representative = None;

        for (window_index, window) in tokens.windows(2).enumerate() {
            if is_dev {
                stats.dev_source_windows += 1;
            } else {
                stats.fit_source_windows += 1;
            }
            let Some(window) = protocol_window(window, &entries_by_text) else {
                continue;
            };

            if is_dev {
                stats.dev_eligible_windows += 1;
                if dev_representative.is_none()
                    && seen_dev_text.insert(window.expected_text.clone())
                {
                    dev_representative = Some((
                        format!("{}:protocol-{}", sentence.id, window_index + 1),
                        window.clone(),
                    ));
                }
                if window.has_long_word
                    && dev_long_word_representative.is_none()
                    && seen_dev_long_word_text.insert(window.expected_text.clone())
                {
                    dev_long_word_representative = Some((
                        format!("{}:protocol-long-{}", sentence.id, window_index + 1),
                        window,
                    ));
                }
            } else {
                stats.fit_eligible_windows += 1;
                fit_phrases.insert(window.expected_text.clone());
                *fit_shortcuts
                    .entry(window.explicit_abbreviation)
                    .or_default()
                    .entry(window.expected_text)
                    .or_default() += 1;
            }
        }
        if let Some(representative) = dev_representative {
            dev_representatives.push(representative);
        }
        if let Some(representative) = dev_long_word_representative {
            dev_long_word_representatives.push(representative);
        }
    }

    stats.fit_distinct_phrases = fit_phrases.len();
    stats.fit_shortcut_codes = fit_shortcuts.len();
    stats.fit_colliding_shortcut_codes = fit_shortcuts
        .values()
        .filter(|phrases| phrases.len() > 1)
        .count();
    let whitelist = fit_shortcuts
        .into_iter()
        .filter_map(|(code, phrases)| {
            let [(text, occurrences)] = phrases.into_iter().collect::<Vec<_>>().try_into().ok()?;
            (occurrences >= MIN_SHORTCUT_OCCURRENCES).then_some((code, text))
        })
        .collect::<HashMap<_, _>>();
    stats.fit_repeated_collision_free_shortcuts = whitelist.len();
    stats.dev_sentence_representatives = dev_representatives.len();
    stats.dev_long_word_representatives = dev_long_word_representatives.len();

    let probes = spread_protocol_probes(&dev_representatives, limit, &whitelist);
    let long_word_probes =
        spread_protocol_probes(&dev_long_word_representatives, limit, &whitelist);
    stats.selected = probes.len();
    stats.selected_long_word = long_word_probes.len();
    stats.selected_whitelist_covered = probes
        .iter()
        .filter(|probe| probe.whitelist_available)
        .count();
    stats.selected_full_keys = probes
        .iter()
        .map(|probe| probe.full_observed.as_str().len())
        .sum();
    stats.selected_anchored_tail_keys = probes
        .iter()
        .map(|probe| probe.anchored_tail_observed.as_str().len())
        .sum();
    stats.selected_conservative_tail_keys = probes
        .iter()
        .map(|probe| probe.conservative_tail_observed.as_str().len())
        .sum();
    stats.selected_explicit_abbreviation_keys = probes
        .iter()
        .map(|probe| probe.explicit_abbreviation_observed.as_str().len())
        .sum();
    stats.selected_long_word_full_keys = long_word_probes
        .iter()
        .map(|probe| probe.full_observed.as_str().len())
        .sum();
    stats.selected_long_word_anchored_tail_keys = long_word_probes
        .iter()
        .map(|probe| probe.anchored_tail_observed.as_str().len())
        .sum();
    stats.selected_long_word_conservative_tail_keys = long_word_probes
        .iter()
        .map(|probe| probe.conservative_tail_observed.as_str().len())
        .sum();

    PublicProtocolSelection {
        fit_context_sequences,
        probes,
        long_word_probes,
        stats,
    }
}

fn spread_protocol_probes(
    representatives: &[(String, ProtocolWindow)],
    limit: usize,
    whitelist: &HashMap<String, String>,
) -> Vec<PublicProtocolProbe> {
    let selected = limit.min(representatives.len());
    (0..selected)
        .map(|index| {
            let spread_index =
                (index * representatives.len() + representatives.len() / 2) / selected;
            let (id, window) = &representatives[spread_index];
            let whitelist_available = whitelist
                .get(window.explicit_abbreviation.as_str())
                .is_some_and(|text| text == &window.expected_text);
            PublicProtocolProbe {
                id: id.clone(),
                full_observed: KeySequence::new(window.full_code.clone())
                    .expect("Rime-derived full codes are lowercase ASCII"),
                anchored_tail_observed: KeySequence::new(window.anchored_tail.clone())
                    .expect("Rime-derived tail abbreviations are lowercase ASCII"),
                conservative_tail_observed: KeySequence::new(window.conservative_tail.clone())
                    .expect("Rime-derived conservative abbreviations are lowercase ASCII"),
                explicit_abbreviation_observed: KeySequence::new(
                    window.explicit_abbreviation.clone(),
                )
                .expect("Rime-derived abbreviations are lowercase ASCII"),
                expected_text: window.expected_text.clone(),
                expected_segments: window.expected_segments.clone(),
                whitelist_available,
            }
        })
        .collect()
}

#[derive(Clone)]
struct ProtocolWindow {
    expected_text: String,
    expected_segments: Vec<String>,
    full_code: String,
    anchored_tail: String,
    conservative_tail: String,
    explicit_abbreviation: String,
    has_long_word: bool,
}

fn protocol_window(
    window: &[&UdToken],
    entries_by_text: &HashMap<&str, &LexiconEntry>,
) -> Option<ProtocolWindow> {
    let expected_text = window
        .iter()
        .map(|token| token.form.as_str())
        .collect::<String>();
    if !(2..=6).contains(&expected_text.chars().count())
        || !expected_text.chars().all(is_han_character)
    {
        return None;
    }
    let first = entries_by_text
        .get(window.first()?.form.as_str())
        .copied()?;
    let second = entries_by_text.get(window.get(1)?.form.as_str()).copied()?;
    let entries = [first, second];
    let full_code = entries
        .iter()
        .map(|entry| entry.code.as_str())
        .collect::<String>();
    let anchored_tail = entries
        .iter()
        .map(|entry| anchored_tail_code(entry))
        .collect::<String>();
    if anchored_tail.len() >= full_code.len() {
        return None;
    }
    let explicit_abbreviation = entries
        .iter()
        .flat_map(|entry| entry.syllable_codes.iter())
        .map(|code| {
            code.as_str()
                .chars()
                .next()
                .expect("a Rime syllable code is non-empty")
        })
        .collect::<String>();
    let conservative_tail = entries
        .iter()
        .map(|entry| conservative_tail_code(entry))
        .collect::<String>();
    Some(ProtocolWindow {
        expected_text,
        expected_segments: entries.iter().map(|entry| entry.text.clone()).collect(),
        full_code,
        anchored_tail,
        conservative_tail,
        explicit_abbreviation,
        has_long_word: entries.iter().any(|entry| entry.syllable_codes.len() >= 3),
    })
}

fn conservative_tail_code(entry: &LexiconEntry) -> String {
    if entry.syllable_codes.len() < 2 {
        return entry.code.as_str().to_owned();
    }
    let split = entry.syllable_codes.len() - 1;
    let mut code = entry.syllable_codes[..split]
        .iter()
        .map(KeySequence::as_str)
        .collect::<String>();
    code.push(
        entry.syllable_codes[split]
            .as_str()
            .chars()
            .next()
            .expect("a parsed syllable code is non-empty"),
    );
    code
}

/// Selects public Han sentence text for a character bigram model.
///
/// Punctuation is omitted. A sentence is retained only when its remaining
/// text contains at least two Han characters. No lexicon, decoder result, or
/// held-out answer influences the selection.
pub fn select_public_character_training_texts(corpus: &UdCorpus) -> PublicCharacterTrainingCorpus {
    let mut stats = PublicCharacterTrainingStats {
        source_sentences: corpus.stats.sentences,
        ..PublicCharacterTrainingStats::default()
    };
    let mut sequences = Vec::new();
    for sentence in &corpus.sentences {
        let text = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .map(|token| token.form.as_str())
            .collect::<String>();
        if text.is_empty() || !text.chars().all(is_han_character) {
            continue;
        }
        stats.han_only_sentences += 1;
        let character_count = text.chars().count();
        if character_count < 2 {
            continue;
        }
        stats.training_sequences += 1;
        stats.training_characters += character_count;
        sequences.push(text);
    }
    PublicCharacterTrainingCorpus { sequences, stats }
}

/// Maps the pinned train split into Rime word sequences for bigram training.
///
/// Punctuation is omitted. Sentences containing non-Han text or an
/// unresolvable token are excluded as a whole. This function does not inspect
/// the held-out test probes or any decoder result.
pub fn select_public_bigram_training_sequences(
    corpus: &UdCorpus,
    lexicon: &[LexiconEntry],
) -> PublicBigramTrainingCorpus {
    let entries_by_text = best_entries_by_text(lexicon);
    let mut stats = PublicBigramTrainingStats {
        source_sentences: corpus.stats.sentences,
        ..PublicBigramTrainingStats::default()
    };
    let mut sequences = Vec::new();

    for sentence in &corpus.sentences {
        let source_tokens = sentence
            .tokens
            .iter()
            .filter(|token| token.upos != "PUNCT")
            .collect::<Vec<_>>();
        if source_tokens
            .iter()
            .flat_map(|token| token.form.chars())
            .any(|character| !is_han_character(character))
        {
            continue;
        }
        stats.han_only_sentences += 1;
        let Some((_observed, words, exact_token_uses, character_fallback_uses)) =
            observed_for_tokens(&source_tokens, &entries_by_text)
        else {
            continue;
        };
        stats.lexicon_coverable_sentences += 1;
        if words.len() < 2 {
            continue;
        }
        stats.training_sequences += 1;
        stats.training_words += words.len();
        stats.exact_token_uses += exact_token_uses;
        stats.character_fallback_uses += character_fallback_uses;
        sequences.push(words);
    }

    PublicBigramTrainingCorpus { sequences, stats }
}

struct ObservedSpellings {
    full_code: String,
    abbreviated_code: String,
}

fn best_entries_by_text(lexicon: &[LexiconEntry]) -> HashMap<&str, &LexiconEntry> {
    let mut entries = HashMap::<&str, &LexiconEntry>::new();
    for entry in lexicon {
        match entries.get(entry.text.as_str()) {
            Some(current) if !entry_precedes(entry, current) => {}
            _ => {
                entries.insert(entry.text.as_str(), entry);
            }
        }
    }
    entries
}

fn entry_precedes(left: &LexiconEntry, right: &LexiconEntry) -> bool {
    left.frequency > right.frequency
        || (left.frequency == right.frequency
            && (left.pinyin.as_str(), left.code.as_str())
                < (right.pinyin.as_str(), right.code.as_str()))
}

fn observed_for_tokens(
    tokens: &[&UdToken],
    entries_by_text: &HashMap<&str, &LexiconEntry>,
) -> Option<(ObservedSpellings, Vec<String>, usize, usize)> {
    let mut observed = ObservedSpellings {
        full_code: String::new(),
        abbreviated_code: String::new(),
    };
    let mut words = Vec::new();
    let mut exact_token_uses = 0;
    let mut character_fallback_uses = 0;
    for token in tokens {
        if let Some(entry) = entries_by_text.get(token.form.as_str()) {
            append_entry_codes(entry, &mut observed);
            words.push(entry.text.clone());
            exact_token_uses += 1;
            continue;
        }
        for character in token.form.chars() {
            let character = character.to_string();
            let entry = entries_by_text.get(character.as_str())?;
            append_entry_codes(entry, &mut observed);
            words.push(entry.text.clone());
            character_fallback_uses += 1;
        }
    }
    Some((observed, words, exact_token_uses, character_fallback_uses))
}

fn observed_for_characters(
    text: &str,
    entries_by_text: &HashMap<&str, &LexiconEntry>,
) -> Option<(ObservedSpellings, Vec<String>)> {
    let mut observed = ObservedSpellings {
        full_code: String::new(),
        abbreviated_code: String::new(),
    };
    let mut words = Vec::new();
    for character in text.chars() {
        let character = character.to_string();
        let entry = entries_by_text.get(character.as_str())?;
        append_entry_codes(entry, &mut observed);
        words.push(entry.text.clone());
    }
    Some((observed, words))
}

fn append_entry_codes(entry: &LexiconEntry, observed: &mut ObservedSpellings) {
    observed.full_code.push_str(entry.code.as_str());
    observed
        .abbreviated_code
        .extend(entry.syllable_codes.iter().map(|code| {
            code.as_str()
                .chars()
                .next()
                .expect("a Rime syllable code is non-empty")
        }));
}

fn anchored_tail_code(entry: &LexiconEntry) -> String {
    let mut codes = entry.syllable_codes.iter();
    let first = codes
        .next()
        .expect("a parsed lexicon entry has at least one syllable");
    let mut observed = first.as_str().to_owned();
    observed.extend(codes.map(|code| {
        code.as_str()
            .chars()
            .next()
            .expect("a Rime syllable code is non-empty")
    }));
    observed
}

fn is_han_character(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{3007}'
    )
}

/// Error returned while parsing pinned CoNLL-U public data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UdCorpusParseError {
    /// A new sentence started before a blank separator ended the previous one.
    MissingSentenceSeparator {
        /// One-based source line number.
        line_number: usize,
    },
    /// A sentence identifier was empty.
    EmptySentenceId {
        /// One-based source line number.
        line_number: usize,
    },
    /// A token row did not contain ten valid CoNLL-U fields.
    InvalidTokenRow {
        /// One-based source line number.
        line_number: usize,
    },
    /// A token row appeared before a sentence identifier.
    TokenOutsideSentence {
        /// One-based source line number.
        line_number: usize,
    },
    /// A token ID was neither an integer nor a recognized special row.
    InvalidTokenId {
        /// One-based source line number.
        line_number: usize,
        /// Invalid ID.
        value: String,
    },
    /// A sentence identifier appeared more than once.
    DuplicateSentenceId {
        /// One-based source line number.
        line_number: usize,
        /// Duplicate identifier.
        id: String,
    },
    /// A sentence contained no integer-ID tokens.
    EmptySentence {
        /// One-based source line number.
        line_number: usize,
        /// Sentence identifier.
        id: String,
    },
    /// No sentences were parsed.
    Empty,
}

impl fmt::Display for UdCorpusParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSentenceSeparator { line_number } => {
                write!(formatter, "UD 语料第 {line_number} 行前缺少句子分隔空行")
            }
            Self::EmptySentenceId { line_number } => {
                write!(formatter, "UD 语料第 {line_number} 行句子编号为空")
            }
            Self::InvalidTokenRow { line_number } => {
                write!(formatter, "UD 语料第 {line_number} 行 token 字段无效")
            }
            Self::TokenOutsideSentence { line_number } => {
                write!(formatter, "UD 语料第 {line_number} 行 token 不属于任何句子")
            }
            Self::InvalidTokenId { line_number, value } => write!(
                formatter,
                "UD 语料第 {line_number} 行 token ID {value:?} 无效"
            ),
            Self::DuplicateSentenceId { line_number, id } => {
                write!(formatter, "UD 语料第 {line_number} 行句子编号 {id:?} 重复")
            }
            Self::EmptySentence { line_number, id } => write!(
                formatter,
                "UD 语料第 {line_number} 行开始的句子 {id:?} 没有 token"
            ),
            Self::Empty => write!(formatter, "UD 语料没有句子"),
        }
    }
}

impl Error for UdCorpusParseError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{
        BigramLanguageModel, CharacterBigramLanguageModel, MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES,
        audit_anchored_tail_failures, audit_public_lexicon_token_coverage,
        audit_public_protocol_context, audit_public_protocols,
        parse_simplified_rime_lexicon as parse_rime_lexicon, parse_ud_conllu,
        select_public_bigram_training_sequences, select_public_calibration_cases,
        select_public_character_training_texts, select_public_continuous_composition_cases,
        select_public_lexicon_rank_probes, select_public_protocol_audit_cases,
        select_public_static_context_cases, select_public_supplemental_composition_cases,
    };

    const RIME: &str = include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
    const UD_TRAIN: &str =
        include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu");
    const UD_TEST: &str =
        include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu");

    #[test]
    fn token_coverage_audit_reports_unique_and_repeated_held_out_changes() {
        const CORPUS: &str = "# sent_id = public-one\n\
1\t双字\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t三字词\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
3\t三字词\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
4\t四字词语\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        let corpus = parse_ud_conllu(CORPUS).unwrap();
        let base = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
双字\tshuang zi\t100\n\
三字词\tsan zi ci\t90\n",
        )
        .unwrap();
        let challenger = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
双字\tshuang zi\t100\n\
四字词语\tsi zi ci yu\t90\n",
        )
        .unwrap();

        let audit = audit_public_lexicon_token_coverage(&corpus, &base, &challenger);
        assert_eq!(audit.lengths[0].characters, 2);
        assert_eq!(audit.lengths[0].source_unique_tokens, 1);
        assert_eq!(audit.lengths[0].base_covered_unique_tokens, 1);
        assert_eq!(audit.lengths[0].challenger_covered_unique_tokens, 1);
        assert_eq!(audit.lengths[1].characters, 3);
        assert_eq!(audit.lengths[1].source_unique_tokens, 1);
        assert_eq!(audit.lengths[1].source_token_instances, 2);
        assert_eq!(audit.lengths[1].challenger_lost_unique_tokens, 1);
        assert_eq!(audit.lengths[1].challenger_lost_token_instances, 2);
        assert_eq!(audit.lengths[2].characters, 4);
        assert_eq!(audit.lengths[2].challenger_gained_unique_tokens, 1);
        assert_eq!(audit.lengths[2].challenger_gained_token_instances, 1);
        assert!(!format!("{audit:?}").contains("三字词"));
        assert!(!format!("{audit:?}").contains("四字词语"));
    }

    #[test]
    fn rank_probe_selection_coalesces_public_tokens_and_uses_lexicon_codes() {
        const CORPUS: &str = "# sent_id = public-one\n\
1\t固定短语\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t固定短语\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
3\t另个短语\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\
4\t，\t_\tPUNCT\t_\t_\t1\tpunct\t_\t_\n\n";
        let corpus = parse_ud_conllu(CORPUS).unwrap();
        let lexicon = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
固定短语\tgu ding duan yu\t10\n\
固定短语\tgu ding duan ju\t20\n\
三个字\tsan ge zi\t30\n",
        )
        .unwrap();

        let selection = select_public_lexicon_rank_probes(&corpus, &lexicon, 4);
        assert_eq!(selection.source_unique_tokens, 2);
        assert_eq!(selection.source_token_instances, 3);
        assert_eq!(selection.matched_token_instances, 2);
        assert_eq!(selection.probes.len(), 1);
        assert_eq!(selection.probes[0].expected_text, "固定短语");
        assert_eq!(selection.probes[0].instances, 2);
        assert_eq!(
            selection.probes[0].observed.as_str(),
            lexicon[1].code.as_str()
        );
    }

    #[test]
    fn static_context_selection_includes_two_complete_single_syllable_words() {
        const CORPUS: &str = "# sent_id = public-one\n\
1\t甲\t_\tNOUN\t_\t_\t0\troot\t_\t_\n\
2\t乙\t_\tNOUN\t_\t_\t1\tdep\t_\t_\n\n";
        let corpus = parse_ud_conllu(CORPUS).unwrap();
        let lexicon = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
甲\tjia\t20\n\
乙\tyi\t10\n",
        )
        .unwrap();

        let static_context = select_public_static_context_cases(&corpus, &lexicon, 8);
        assert_eq!(static_context.stats.source_windows, 1);
        assert_eq!(static_context.stats.exact_word_coverable, 1);
        assert_eq!(static_context.probes.len(), 1);
        assert_eq!(static_context.probes[0].expected_segments, ["甲", "乙"]);
        assert_eq!(
            static_context.probes[0].observed.as_str(),
            format!("{}{}", lexicon[0].code, lexicon[1].code)
        );
        assert!(
            select_public_continuous_composition_cases(&corpus, &lexicon, 8)
                .probes
                .is_empty()
        );
    }

    #[test]
    fn pinned_ud_train_snapshot_has_stable_bigram_mapping() {
        assert_eq!(UD_TRAIN.len(), 9_321_012);
        let corpus = parse_ud_conllu(UD_TRAIN).unwrap();
        assert_eq!(corpus.stats.source_lines, 118_599);
        assert_eq!(corpus.stats.sentences, 3_997);
        assert_eq!(corpus.stats.syntactic_tokens, 98_614);
        assert_eq!(corpus.stats.punctuation_tokens, 13_627);
        assert_eq!(corpus.stats.special_token_rows, 0);

        let character_training = select_public_character_training_texts(&corpus);
        assert_eq!(character_training.stats.source_sentences, 3_997);
        assert_eq!(
            character_training.sequences.len(),
            character_training.stats.training_sequences
        );
        assert_eq!(
            character_training
                .sequences
                .iter()
                .map(|sequence| sequence.chars().count())
                .sum::<usize>(),
            character_training.stats.training_characters
        );
        assert!(
            character_training
                .sequences
                .iter()
                .all(|sequence| sequence.chars().count() >= 2)
        );

        let lexicon = parse_rime_lexicon(RIME).unwrap().entries;
        let first = select_public_bigram_training_sequences(&corpus, &lexicon);
        let second = select_public_bigram_training_sequences(&corpus, &lexicon);
        assert_eq!(first, second);
        assert_eq!(first.stats.source_sentences, 3_997);
        assert_eq!(first.stats.han_only_sentences, 2_346);
        assert_eq!(first.stats.lexicon_coverable_sentences, 2_339);
        assert_eq!(first.stats.training_sequences, 2_339);
        assert_eq!(first.stats.training_words, 51_712);
        assert_eq!(first.stats.exact_token_uses, 42_745);
        assert_eq!(first.stats.character_fallback_uses, 8_967);
        assert_eq!(first.sequences.len(), first.stats.training_sequences);

        let model = BigramLanguageModel::from_token_sequences(&first.sequences, &lexicon).unwrap();
        let stats = model.stats();
        assert_eq!(stats.vocabulary_size, 62_071);
        assert_eq!(stats.observed_pair_types, 40_299);
        assert_eq!(stats.observed_predecessor_types, 8_890);
        assert_eq!(stats.observed_pair_instances, 49_373);

        let texts = first
            .sequences
            .iter()
            .map(|sequence| sequence.concat())
            .collect::<Vec<_>>();
        let character_model = CharacterBigramLanguageModel::from_text_sequences(&texts).unwrap();
        let character_stats = character_model.stats();
        assert_eq!(character_stats.sequences, 2_339);
        assert_eq!(character_stats.character_instances, 74_381);
        assert_eq!(character_stats.vocabulary_size, 2_995);
        assert_eq!(character_stats.observed_pair_types, 43_048);
        assert_eq!(character_stats.observed_pair_instances, 76_720);

        let protocol = select_public_protocol_audit_cases(&corpus, &lexicon, 128);
        let protocol_again = select_public_protocol_audit_cases(&corpus, &lexicon, 128);
        assert_eq!(protocol, protocol_again);
        assert_eq!(protocol.stats.fit_source_sentences, 3_198);
        assert_eq!(protocol.stats.dev_source_sentences, 799);
        assert_eq!(protocol.stats.fit_source_windows, 64_820);
        assert_eq!(protocol.stats.fit_context_sequences, 1_889);
        assert_eq!(protocol.stats.fit_context_words, 41_965);
        assert_eq!(protocol.stats.fit_eligible_windows, 39_094);
        assert_eq!(protocol.stats.fit_distinct_phrases, 32_711);
        assert_eq!(protocol.stats.fit_shortcut_codes, 19_184);
        assert_eq!(protocol.stats.fit_colliding_shortcut_codes, 5_015);
        assert_eq!(protocol.stats.fit_repeated_collision_free_shortcuts, 826);
        assert_eq!(protocol.stats.dev_source_windows, 16_170);
        assert_eq!(protocol.stats.dev_eligible_windows, 9_665);
        assert_eq!(protocol.stats.dev_sentence_representatives, 793);
        assert_eq!(protocol.stats.dev_long_word_representatives, 116);
        assert_eq!(protocol.stats.selected, 128);
        assert_eq!(protocol.stats.selected_long_word, 116);
        assert_eq!(protocol.stats.selected_whitelist_covered, 0);
        assert_eq!(protocol.stats.selected_full_keys, 840);
        assert_eq!(protocol.stats.selected_anchored_tail_keys, 676);
        assert_eq!(protocol.stats.selected_conservative_tail_keys, 683);
        assert_eq!(protocol.stats.selected_explicit_abbreviation_keys, 420);
        assert_eq!(protocol.stats.selected_long_word_full_keys, 1_082);
        assert_eq!(protocol.stats.selected_long_word_anchored_tail_keys, 773);
        assert_eq!(
            protocol.stats.selected_long_word_conservative_tail_keys,
            922
        );
        assert!(protocol.probes.iter().all(|probe| {
            probe.expected_segments.len() == 2
                && probe.full_observed.as_str().len() > probe.anchored_tail_observed.as_str().len()
                && probe.conservative_tail_observed.as_str().len()
                    >= probe.anchored_tail_observed.as_str().len()
                && probe.anchored_tail_observed.as_str().len()
                    >= probe.explicit_abbreviation_observed.as_str().len()
        }));
        assert!(protocol.long_word_probes.iter().all(|probe| {
            probe.conservative_tail_observed.as_str().len()
                > probe.anchored_tail_observed.as_str().len()
        }));

        let protocol_report = audit_public_protocols(&lexicon, &protocol.probes);
        assert_eq!(
            (
                protocol_report.full_code.hits_at_1,
                protocol_report.full_code.hits_at_5,
                protocol_report.full_code.hits_at_10,
            ),
            (89, 118, 122)
        );
        assert_eq!(
            (
                protocol_report.conservative_tail.hits_at_1,
                protocol_report.conservative_tail.hits_at_5,
                protocol_report.conservative_tail.hits_at_10,
            ),
            (52, 93, 104)
        );
        assert_eq!(
            (
                protocol_report.anchored_tail.hits_at_1,
                protocol_report.anchored_tail.hits_at_5,
                protocol_report.anchored_tail.hits_at_10,
            ),
            (52, 93, 104)
        );
        assert_eq!(
            (
                protocol_report.explicit_abbreviation.hits_at_1,
                protocol_report.explicit_abbreviation.hits_at_5,
                protocol_report.explicit_abbreviation.hits_at_10,
            ),
            (8, 20, 25)
        );

        let long_report = audit_public_protocols(&lexicon, &protocol.long_word_probes);
        assert_eq!(
            (
                long_report.full_code.hits_at_1,
                long_report.full_code.hits_at_5,
                long_report.full_code.hits_at_10,
            ),
            (87, 115, 115)
        );
        assert_eq!(
            (
                long_report.conservative_tail.hits_at_1,
                long_report.conservative_tail.hits_at_5,
                long_report.conservative_tail.hits_at_10,
            ),
            (76, 111, 113)
        );
        assert_eq!(
            (
                long_report.anchored_tail.hits_at_1,
                long_report.anchored_tail.hits_at_5,
                long_report.anchored_tail.hits_at_10,
            ),
            (72, 110, 113)
        );

        let failure_report = audit_anchored_tail_failures(&lexicon, &protocol.probes, 10, 100);
        assert_eq!(failure_report.baseline_visible, 104);
        assert_eq!(failure_report.deeper_visible, 23);
        assert_eq!(failure_report.outside_audit_depth, 1);
        assert_eq!(failure_report.boundary_recovered_visible, 1);
        assert_eq!(failure_report.boundary_recovered_at_1, 0);
        assert_eq!(failure_report.baseline_top_same_length, 24);
        assert_eq!(failure_report.failures_with_word_code_collision, 24);
        assert_eq!(failure_report.maximum_expected_word_code_fanout, 282);
        assert_eq!(failure_report.recovered_net_actions_saved, 0);

        let fit_model =
            BigramLanguageModel::from_token_sequences(&protocol.fit_context_sequences, &lexicon)
                .unwrap();
        let context_report =
            audit_public_protocol_context(&lexicon, &protocol.probes, &fit_model, 100);
        assert_eq!(
            (
                context_report.full_code.context_hits_at_1,
                context_report.full_code.context_hits_at_5,
                context_report.full_code.context_hits_at_10,
                context_report.full_code.repaired_into_top_10,
                context_report.full_code.dropped_out_of_top_10,
            ),
            (87, 116, 121, 2, 3)
        );
        assert_eq!(
            (
                context_report.anchored_tail.context_hits_at_1,
                context_report.anchored_tail.context_hits_at_5,
                context_report.anchored_tail.context_hits_at_10,
                context_report.anchored_tail.repaired_into_top_10,
                context_report.anchored_tail.dropped_out_of_top_10,
            ),
            (54, 89, 102, 2, 4)
        );
    }

    #[test]
    fn pinned_ud_test_snapshot_has_stable_accounting_and_selection() {
        assert_eq!(UD_TEST.len(), 1_136_613);
        let corpus = parse_ud_conllu(UD_TEST).unwrap();
        assert_eq!(corpus.stats.source_lines, 14_510);
        assert_eq!(corpus.stats.sentences, 500);
        assert_eq!(corpus.stats.syntactic_tokens, 12_010);
        assert_eq!(corpus.stats.punctuation_tokens, 1_691);
        assert_eq!(corpus.stats.special_token_rows, 0);

        let lexicon = parse_rime_lexicon(RIME).unwrap().entries;
        let first = select_public_calibration_cases(&corpus, &lexicon, 64, 128);
        let second = select_public_calibration_cases(&corpus, &lexicon, 64, 128);

        assert_eq!(first, second);
        assert_eq!(first.sentence_full_code_probes.len(), 64);
        assert_eq!(first.sentence_abbreviation_probes.len(), 64);
        assert_eq!(first.held_out_token_full_code_probes.len(), 128);
        assert_eq!(first.held_out_token_abbreviation_probes.len(), 128);
        assert_eq!(first.stats.sentence_length_eligible, 153);
        assert_eq!(first.stats.sentence_han_only, 111);
        assert_eq!(first.stats.sentence_lexicon_coverable, 111);
        assert_eq!(first.stats.selected_sentences, 64);
        assert_eq!(first.stats.selected_exact_token_uses, 678);
        assert_eq!(first.stats.selected_character_fallback_uses, 164);
        assert_eq!(first.stats.held_out_token_eligible, 699);
        assert_eq!(first.stats.selected_held_out_tokens, 128);
        assert!(
            first
                .sentence_full_code_probes
                .iter()
                .zip(&first.sentence_abbreviation_probes)
                .chain(
                    first
                        .held_out_token_full_code_probes
                        .iter()
                        .zip(&first.held_out_token_abbreviation_probes),
                )
                .all(|(full, abbreviated)| {
                    full.expected_text == abbreviated.expected_text
                        && full.observed.as_str().len() == abbreviated.observed.as_str().len() * 2
                })
        );
        assert!(
            first
                .sentence_full_code_probes
                .iter()
                .chain(&first.sentence_abbreviation_probes)
                .chain(&first.held_out_token_full_code_probes)
                .chain(&first.held_out_token_abbreviation_probes)
                .all(|probe| {
                    !probe.id.is_empty()
                        && !probe.expected_text.is_empty()
                        && !probe.observed.as_str().is_empty()
                })
        );

        let continuous = select_public_continuous_composition_cases(&corpus, &lexicon, 64);
        let continuous_again = select_public_continuous_composition_cases(&corpus, &lexicon, 64);
        assert_eq!(continuous, continuous_again);
        assert_eq!(continuous.stats.source_windows, 9_819);
        assert_eq!(continuous.stats.han_length_eligible, 8_891);
        assert_eq!(continuous.stats.exact_word_coverable, 7_465);
        assert_eq!(continuous.stats.key_saving_eligible, 5_919);
        assert_eq!(continuous.stats.transposition_eligible, 5_796);
        assert_eq!(continuous.stats.sentence_representatives, 489);
        assert_eq!(continuous.stats.selected, 64);
        assert_eq!(continuous.stats.selected_full_keys, 418);
        assert_eq!(continuous.stats.selected_tail_keys, 337);
        assert_eq!(
            continuous
                .probes
                .iter()
                .map(|probe| probe.id.split(':').next().unwrap())
                .collect::<HashSet<_>>()
                .len(),
            continuous.probes.len()
        );
        assert!(continuous.probes.iter().all(|probe| {
            probe.expected_segments.len() == 2
                && probe.full_observed.as_str().len()
                    > probe.tail_abbreviated_observed.as_str().len()
                && probe.tail_abbreviated_observed.as_str().len()
                    == probe.transposed_observed.as_str().len()
                && probe.tail_abbreviated_observed != probe.transposed_observed
        }));
    }

    #[test]
    fn supplemental_composition_selection_is_source_only_stratified_and_bounded() {
        const CORPUS: &str = "# sent_id = one\n\
1\t这\t这\tPRON\t_\t_\t0\troot\t_\t_\n\
2\t属于\t属于\tVERB\t_\t_\t1\tdep\t_\t_\n\
3\t一种\t一种\tNOUN\t_\t_\t2\tdep\t_\t_\n\n\
# sent_id = two\n\
1\t揉碎\t揉碎\tVERB\t_\t_\t0\troot\t_\t_\n\
2\t以后\t以后\tNOUN\t_\t_\t1\tdep\t_\t_\n";
        let core = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
这\tzhe\t1000\n\
一种\tyi zhong\t900\n\
以后\tyi hou\t800\n",
        )
        .unwrap();
        let supplemental = crate::parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
属于\tshu yu\t1000\n\
揉碎\trou sui\t900\n\
这属于\tzhe shu yu\t800\n",
        )
        .unwrap();
        let corpus = parse_ud_conllu(CORPUS).unwrap();

        let first = select_public_supplemental_composition_cases(&corpus, &core, &supplemental, 5);
        let second = select_public_supplemental_composition_cases(&corpus, &core, &supplemental, 5);
        assert_eq!(first, second);
        assert_eq!(first.stats.source_windows, 4);
        assert_eq!(first.stats.han_length_eligible, 4);
        assert_eq!(first.stats.one_supplemental_word_eligible, 4);
        assert_eq!(first.stats.whole_phrase_collisions, 1);
        assert_eq!(first.stats.whole_code_collisions, 0);
        assert_eq!(first.stats.sentence_shape_representatives, 3);
        assert_eq!(first.stats.selected, 3);
        assert_eq!(first.stats.selected_two_token, 2);
        assert_eq!(first.stats.selected_three_token, 1);
        assert_eq!(first.stats.selected_supplemental_first, 2);
        assert_eq!(first.stats.selected_supplemental_middle, 1);
        assert_eq!(first.stats.selected_supplemental_last, 0);
        assert!(first.probes.iter().all(|probe| {
            probe.observed.as_str().len().is_multiple_of(2)
                && probe.observed.as_str().len() / 2 <= MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES
                && probe.expected_segments.len() == probe.segment_codes.len()
                && probe.expected_segments[probe.supplemental_segment_index]
                    .chars()
                    .count()
                    >= 2
        }));
    }

    #[test]
    fn conllu_parser_rejects_structural_drift() {
        assert!(parse_ud_conllu("# sent_id = one\n1\t你\n").is_err());
        assert!(parse_ud_conllu("1\t你\t你\tPRON\t_\t_\t0\troot\t_\t_\n").is_err());
        assert!(
            parse_ud_conllu(
                "# sent_id = one\n1\t你\t你\tPRON\t_\t_\t0\troot\t_\t_\n\
                 # sent_id = two\n1\t好\t好\tADJ\t_\t_\t0\troot\t_\t_\n"
            )
            .is_err()
        );
    }
}
