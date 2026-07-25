use ziranma_decoder::{Correction, Decoder, LexiconParseError, parse_lexicon_tsv};

const PUBLIC_DEMO_LEXICON: &str = include_str!("fixtures/public/demo_lexicon.tsv");

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
fn unsupported_edits_do_not_generate_a_candidate() {
    assert!(demo_decoder().decode("niqk", 10).unwrap().is_empty());
    assert!(demo_decoder().decode("nifj", 10).unwrap().is_empty());
    assert!(demo_decoder().decode("nih", 10).unwrap().is_empty());
}

#[test]
fn top_k_is_respected() {
    assert!(demo_decoder().decode("nihk", 0).unwrap().is_empty());
    assert!(demo_decoder().decode("nihk", 1).unwrap().len() <= 1);
}

#[test]
fn score_breakdown_adds_up() {
    for candidate in demo_decoder().decode("nigk", 10).unwrap() {
        let reconstructed = candidate.score.frequency - candidate.score.correction_penalty;
        assert!((reconstructed - candidate.score.total).abs() < f64::EPSILON);
    }
}

#[test]
fn parser_rejects_zero_frequency() {
    let fixture = "text\tpinyin\tcode\tfrequency\n你好\tni hao\tnihk\t0\n";
    assert!(matches!(
        parse_lexicon_tsv(fixture),
        Err(LexiconParseError::InvalidFrequency { .. })
    ));
}
