use ziranma_decoder::{
    BigramLanguageModel, CandidateSource, Correction, Decoder, LexiconParseError, parse_lexicon_tsv,
};

const PUBLIC_DEMO_LEXICON: &str = include_str!("fixtures/public/demo_lexicon.tsv");
const PUBLIC_BIGRAM_CORPUS: &str = include_str!("fixtures/public/demo_bigram_corpus.tsv");
const HELLO_LEXICON: &str = "text\tpinyin\tfrequency\n你好\tni hao\t100\n";
const PUBLIC_RIME_LEXICON: &str =
    include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");

fn demo_decoder() -> Decoder {
    Decoder::new(
        parse_lexicon_tsv(PUBLIC_DEMO_LEXICON).expect("the checked-in public fixture should parse"),
    )
}

#[test]
fn exact_input_is_returned_without_a_correction() {
    let candidates = demo_decoder().decode("nihk", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be present");

    assert_eq!(candidate.correction, Correction::Exact);
    assert_eq!(candidate.score.correction_penalty, 0.0);
    assert_eq!(candidate.score.abbreviation_penalty, 0.0);
}

#[test]
fn one_neighbor_substitution_recovers_the_intended_entry() {
    let candidates = demo_decoder().decode("nigk", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert!(matches!(
        candidate.correction,
        Correction::NeighborSubstitution {
            index: 2,
            intended: 'h',
            actual: 'g'
        }
    ));
    assert!(candidate.score.correction_penalty > 0.0);
}

#[test]
fn one_adjacent_transposition_recovers_the_intended_entry() {
    let candidates = demo_decoder().decode("nikh", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert!(matches!(
        candidate.correction,
        Correction::AdjacentTransposition { start: 2, .. }
    ));
}

#[test]
fn transposition_can_cross_a_two_key_syllable_boundary() {
    let candidates = demo_decoder().decode("nhik", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert!(matches!(
        candidate.correction,
        Correction::AdjacentTransposition { start: 1, .. }
    ));
}

#[test]
fn one_missing_key_recovers_the_intended_entry() {
    let candidates = demo_decoder().decode("nik", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert!(matches!(
        candidate.correction,
        Correction::MissingKey {
            index: 2,
            intended: 'h'
        }
    ));
}

#[test]
fn one_extra_key_recovers_the_intended_entry() {
    let candidates = demo_decoder().decode("niihk", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert!(matches!(
        candidate.correction,
        Correction::ExtraKey {
            index: 2,
            actual: 'i'
        }
    ));
}

#[test]
fn mixed_one_and_two_key_syllables_are_explained() {
    let candidates = demo_decoder().decode("nhk", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert_eq!(candidate.correction, Correction::Exact);
    assert_eq!(candidate.spelling.code.as_str(), "nhk");
    assert_eq!(candidate.spelling.abbreviated_syllables, [0]);
    assert!(candidate.score.abbreviation_penalty > 0.0);
}

#[test]
fn abbreviation_and_one_key_error_can_be_inferred_together() {
    let candidates = demo_decoder().decode("ni", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "你好")
        .expect("你好 should be recovered");

    assert_eq!(candidate.spelling.code.as_str(), "nih");
    assert_eq!(candidate.spelling.abbreviated_syllables, [1]);
    assert!(matches!(
        candidate.correction,
        Correction::MissingKey {
            index: 2,
            intended: 'h'
        }
    ));
}

#[test]
fn unsupported_edits_do_not_recover_the_target() {
    for observed in ["niqk", "nifj", "nfj"] {
        assert!(
            demo_decoder()
                .decode(observed, 10)
                .unwrap()
                .iter()
                .all(|candidate| candidate.text != "你好"),
            "{observed} should not recover 你好"
        );
    }
}

#[test]
fn top_k_is_respected() {
    assert!(demo_decoder().decode("nihk", 0).unwrap().is_empty());
    assert!(demo_decoder().decode("nihk", 1).unwrap().len() <= 1);
}

#[test]
fn score_breakdown_adds_up() {
    for candidate in demo_decoder().decode("nigk", 10).unwrap() {
        let reconstructed = candidate.score.frequency
            - candidate.score.correction_penalty
            - candidate.score.abbreviation_penalty
            - candidate.score.unresolved_input_penalty;
        assert!((reconstructed - candidate.score.total).abs() < f64::EPSILON);
    }
}

#[test]
fn parser_rejects_zero_frequency() {
    let fixture = "text\tpinyin\tfrequency\n你好\tni hao\t0\n";
    assert!(matches!(
        parse_lexicon_tsv(fixture),
        Err(LexiconParseError::InvalidFrequency { .. })
    ));
}

#[test]
fn parser_generates_canonical_codes_from_pinyin() {
    let entries = parse_lexicon_tsv(PUBLIC_DEMO_LEXICON).unwrap();
    let ni_hao = entries
        .iter()
        .find(|entry| entry.text == "你好")
        .expect("fixture contains 你好");
    let shu_ju = entries
        .iter()
        .find(|entry| entry.text == "数据")
        .expect("fixture contains 数据");

    assert_eq!(ni_hao.code.as_str(), "nihk");
    assert_eq!(shu_ju.code.as_str(), "uujv");
}

#[test]
fn pinned_public_rime_snapshot_has_stable_import_accounting() {
    let imported = ziranma_decoder::parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap();

    assert_eq!(imported.stats.source_rows, 65_125);
    assert_eq!(imported.stats.imported_entries, 65_116);
    assert_eq!(imported.stats.zero_weights_floored, 1_714);
    assert_eq!(imported.stats.unsupported_pinyin_rows, 8);
    assert_eq!(imported.stats.too_many_syllable_rows, 0);
    assert_eq!(imported.stats.duplicate_rows, 1);
    assert_eq!(imported.entries.len(), imported.stats.imported_entries);
}

#[test]
fn sentence_decoder_finds_word_boundaries_with_full_abbreviations() {
    let candidates = demo_decoder().decode_sentence("zrmurf", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "自然码输入法")
        .expect("two-word sentence should be recovered");

    assert_eq!(candidate.segments.len(), 2);
    assert_eq!(candidate.segments[0].candidate.text, "自然码");
    assert_eq!(
        candidate.segments[0].candidate.spelling.code.as_str(),
        "zrm"
    );
    assert_eq!(candidate.segments[1].candidate.text, "输入法");
    assert_eq!(
        candidate.segments[1].candidate.spelling.code.as_str(),
        "urf"
    );
    assert!(
        candidate
            .segments
            .iter()
            .all(|segment| segment.candidate.correction == Correction::Exact)
    );
}

#[test]
fn sentence_decoder_shares_one_error_budget_across_words() {
    let candidates = demo_decoder().decode_sentence("zrnurf", 10).unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.text == "自然码输入法")
        .expect("sentence with one neighbor error should be recovered");

    assert_eq!(
        candidate
            .segments
            .iter()
            .filter(|segment| segment.candidate.correction != Correction::Exact)
            .count(),
        1
    );
    assert!(matches!(
        candidate.segments[0].candidate.correction,
        Correction::NeighborSubstitution {
            intended: 'm',
            actual: 'n',
            ..
        }
    ));
}

#[test]
fn sentence_decoder_prefers_complete_exact_path_over_correction() {
    let candidates = demo_decoder().decode_sentence("ajjp", 10).unwrap();

    assert_eq!(candidates[0].text, "按键简拼");
    assert!(!candidates[0].used_error);
    assert!(
        candidates
            .iter()
            .position(|candidate| candidate.text == "按键")
            .is_some_and(|rank| rank > 0)
    );
}

#[test]
fn sentence_decoder_retains_unknown_keys_explicitly() {
    let decoder = Decoder::new(parse_lexicon_tsv(HELLO_LEXICON).unwrap());
    let (candidates, stats) = decoder.decode_sentence_with_stats("nigkz", 10).unwrap();
    let candidate = &candidates[0];

    assert_eq!(candidate.text, "你好〔z〕");
    assert_eq!(candidate.unresolved_key_count, 1);
    assert!(candidate.used_error);
    assert_eq!(candidate.segments.len(), 2);
    let unresolved = &candidate.segments[1].candidate;
    assert_eq!(unresolved.source, CandidateSource::UnresolvedInput);
    assert_eq!(unresolved.pinyin, "");
    assert_eq!(unresolved.code.as_str(), "z");
    assert_eq!(unresolved.spelling.code.as_str(), "z");
    assert_eq!(unresolved.correction, Correction::Exact);
    assert!(unresolved.score.unresolved_input_penalty > 0.0);
    assert!(stats.unresolved_lattice_transitions > 0);
    assert!(stats.lattice_transitions >= stats.unresolved_lattice_transitions);
}

#[test]
fn fully_covered_correction_ranks_before_exact_fallback() {
    let decoder = Decoder::new(parse_lexicon_tsv(HELLO_LEXICON).unwrap());
    let candidates = decoder.decode_sentence("nigk", 10).unwrap();

    assert_eq!(candidates[0].text, "你好");
    assert_eq!(candidates[0].unresolved_key_count, 0);
    assert!(candidates[0].used_error);
    assert!(
        candidates
            .iter()
            .skip(1)
            .any(|candidate| candidate.unresolved_key_count > 0)
    );
    let first_fallback = candidates
        .iter()
        .position(|candidate| candidate.unresolved_key_count > 0)
        .unwrap();
    assert!(
        candidates[..first_fallback]
            .iter()
            .all(|candidate| candidate.unresolved_key_count == 0)
    );
}

#[test]
fn unresolved_input_does_not_consume_the_global_error_budget() {
    let decoder = Decoder::new(parse_lexicon_tsv(HELLO_LEXICON).unwrap());
    let candidates = decoder.decode_sentence("nigkz", 10).unwrap();
    let candidate = &candidates[0];

    assert_eq!(candidate.text, "你好〔z〕");
    assert_eq!(candidate.unresolved_key_count, 1);
    assert!(candidate.used_error);
    assert_eq!(
        candidate
            .segments
            .iter()
            .filter(|segment| segment.candidate.source == CandidateSource::UnresolvedInput)
            .count(),
        1
    );
    assert_eq!(
        candidate
            .segments
            .iter()
            .filter(|segment| segment.candidate.correction != Correction::Exact)
            .count(),
        1
    );
}

#[test]
fn empty_lexicon_still_returns_literal_unresolved_input() {
    let candidates = Decoder::new(Vec::new()).decode_sentence("xyz", 1).unwrap();

    assert_eq!(candidates[0].text, "〔x〕〔y〕〔z〕");
    assert_eq!(candidates[0].unresolved_key_count, 3);
    assert!(!candidates[0].used_error);
    assert!(
        candidates[0]
            .segments
            .iter()
            .all(|segment| segment.candidate.source == CandidateSource::UnresolvedInput)
    );
}

#[test]
fn unresolved_segment_resets_bigram_context() {
    let lexicon = parse_lexicon_tsv(HELLO_LEXICON).unwrap();
    let model = BigramLanguageModel::from_tsv("tokens\tcount\n你好 你好\t100\n", &lexicon).unwrap();
    let decoder = Decoder::new(lexicon).with_bigram_model(model);
    let candidates = decoder.decode_sentence("nihkzznihk", 10).unwrap();
    let candidate = &candidates[0];
    let unresolved_index = candidate
        .segments
        .iter()
        .position(|segment| segment.candidate.source == CandidateSource::UnresolvedInput)
        .expect("the path should retain one unknown key");
    let next_segment = &candidate.segments[unresolved_index + 1];

    assert_eq!(candidate.unresolved_key_count, 1);
    assert_eq!(next_segment.candidate.text, "你好");
    assert_eq!(next_segment.language_score.bigram, None);
}

#[test]
fn bigram_context_resolves_same_code_word_ambiguity() {
    let lexicon = parse_lexicon_tsv(PUBLIC_DEMO_LEXICON).unwrap();
    let model = BigramLanguageModel::from_tsv(PUBLIC_BIGRAM_CORPUS, &lexicon).unwrap();
    let decoder = Decoder::new(lexicon).with_bigram_model(model);
    let candidates = decoder.decode_sentence("ajjp", 10).unwrap();

    assert_eq!(candidates[0].text, "按键键盘");
    let evidence = candidates[0].segments[1]
        .language_score
        .bigram
        .expect("second word should have bigram evidence");
    assert_eq!(evidence.observed_count, 100);
}

#[test]
fn bigram_lattice_preserves_text_distinct_future_states() {
    let lexicon = parse_lexicon_tsv(PUBLIC_DEMO_LEXICON).unwrap();
    let model = BigramLanguageModel::from_tsv(PUBLIC_BIGRAM_CORPUS, &lexicon).unwrap();
    let decoder = Decoder::new(lexicon).with_bigram_model(model);
    let (_candidates, stats) = decoder.decode_sentence_with_stats("zrmurf", 5).unwrap();

    assert_eq!(
        stats.lattice_transitions_retained,
        stats.lattice_transitions
    );
    assert_eq!(
        stats.lattice_transitions_materialized,
        stats.lattice_transitions
    );
}

#[test]
fn compact_syllable_index_stores_each_entry_once() {
    let lexicon = parse_lexicon_tsv(PUBLIC_DEMO_LEXICON).unwrap();
    let entry_count = lexicon.len();
    let decoder = Decoder::new(lexicon);
    let stats = decoder.index_stats();

    assert_eq!(stats.terminal_count, entry_count);
    assert_eq!(stats.node_count, 96);
    assert_eq!(stats.edge_count, 95);
    assert_eq!(stats.represented_spelling_count, 212);
    assert_eq!(stats.maximum_syllables, 3);
    assert_eq!(stats.edge_count + 1, stats.node_count);
    assert!(stats.node_count < stats.represented_spelling_count);
}

#[test]
fn joint_search_reports_inspectable_work() {
    let (candidates, stats) = demo_decoder().decode_with_stats("nhk", 10).unwrap();

    assert_eq!(candidates[0].text, "你好");
    assert!(stats.trie_path_visits > 0);
    assert!(stats.alignment_states_reused > 0);
    assert!(
        stats.alignment_states_examined + stats.alignment_states_reused >= stats.trie_path_visits
    );
    assert!(stats.terminal_spelling_matches >= candidates.len());
}

#[test]
fn sentence_lattice_reports_streaming_search_work() {
    let (candidates, stats) = demo_decoder()
        .decode_sentence_with_stats("zrmurf", 10)
        .unwrap();

    assert_eq!(candidates[0].text, "自然码输入法");
    assert!(stats.segment_trie_scans > 0);
    assert!(stats.trie_path_visits >= stats.segment_trie_scans);
    assert!(stats.alignment_states_reused > 0);
    assert!(
        stats.alignment_states_examined + stats.alignment_states_reused >= stats.trie_path_visits
    );
    assert!(
        stats.terminal_spelling_matches + stats.unresolved_lattice_transitions
            >= stats.lattice_transitions
    );
    assert!(stats.lattice_transitions > 0);
    assert!(stats.lattice_transitions_materialized > 0);
    assert!(stats.lattice_transitions_materialized <= stats.lattice_transitions);
    assert!(stats.lattice_transitions_retained > 0);
    assert!(stats.lattice_transitions_retained <= stats.lattice_transitions_materialized);
    assert!(stats.unresolved_lattice_transitions > 0);
    assert!(stats.unresolved_lattice_transitions_retained <= stats.unresolved_lattice_transitions);
    assert!(stats.ranking_states_evaluated > 0);
    assert!(stats.ranking_transitions_considered > 0);
    assert!(stats.ranking_transitions_retained <= stats.ranking_transitions_considered);
    assert!(stats.path_combinations_considered > 0);
}

#[test]
fn sentence_lattice_exactly_reduces_same_future_state_transitions() {
    let (_candidates, stats) = demo_decoder()
        .decode_sentence_with_stats("zrmurf", 5)
        .unwrap();

    assert!(stats.lattice_transitions_materialized < stats.lattice_transitions);
    assert!(stats.lattice_transitions_retained < stats.lattice_transitions);
    assert!(stats.ranking_transitions_retained <= stats.ranking_transitions_considered);
}
