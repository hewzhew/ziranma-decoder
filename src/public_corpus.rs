use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{KeySequence, LabeledSentenceProbe, LexiconEntry, ProbeSpellingMode};

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
    use crate::{
        BigramLanguageModel, CharacterBigramLanguageModel, parse_rime_lexicon, parse_ud_conllu,
        select_public_bigram_training_sequences, select_public_calibration_cases,
    };

    const RIME: &str = include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
    const UD_TRAIN: &str =
        include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu");
    const UD_TEST: &str =
        include_str!("../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-test.conllu");

    #[test]
    fn pinned_ud_train_snapshot_has_stable_bigram_mapping() {
        assert_eq!(UD_TRAIN.len(), 9_321_012);
        let corpus = parse_ud_conllu(UD_TRAIN).unwrap();
        assert_eq!(corpus.stats.source_lines, 118_599);
        assert_eq!(corpus.stats.sentences, 3_997);
        assert_eq!(corpus.stats.syntactic_tokens, 98_614);
        assert_eq!(corpus.stats.punctuation_tokens, 13_627);
        assert_eq!(corpus.stats.special_token_rows, 0);

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
        assert_eq!(stats.vocabulary_size, 64_422);
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
