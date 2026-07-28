use std::error::Error;
use std::fmt;

use crate::{
    CandidateSource, Correction, Decoder, KeySequence, KeySequenceError, SentenceCandidate,
};

/// Largest visible candidate list accepted by the first candidate-lab protocol.
pub const MAX_CANDIDATE_LAB_TOP_K: usize = 10;

/// One explicitly separated candidate-lab lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateLabLane {
    /// The decoder's ordinary, conservatively ordered sentence candidates.
    Primary,
    /// Anchored suffix-abbreviation candidates that use one transposition.
    AnchoredTranspositionRecovery,
}

impl CandidateLabLane {
    /// Explicit lane-switch actions assumed by the projection.
    pub fn activation_actions(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::AnchoredTranspositionRecovery => 1,
        }
    }
}

/// One candidate together with bounded, inspectable action accounting.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateLabCandidate {
    /// Lane that exposed this candidate.
    pub lane: CandidateLabLane,
    /// One-based rank inside that lane.
    pub rank: usize,
    /// Complete decoder evidence retained for explanation.
    pub candidate: SentenceCandidate,
    /// Letter keys in the observed input.
    pub observed_letter_keys: usize,
    /// Canonical full-code letters for this candidate, absent if any segment is unresolved.
    pub canonical_full_letter_keys: Option<usize>,
    /// One explicit candidate-selection action.
    pub selection_actions: usize,
    /// Explicit action needed to enter a non-primary lane.
    pub lane_activation_actions: usize,
    /// Observed letters plus one selection and any lane activation.
    pub projected_actions_one_selection: usize,
    /// Actions saved against full code plus one selection.
    ///
    /// This remains absent when unresolved input prevents a full-code baseline.
    pub net_actions_saved_vs_full: Option<isize>,
    /// Number of syllables represented by one-key abbreviation.
    pub abbreviated_syllables: usize,
    /// Number of word segments that consumed the global correction budget.
    pub corrected_segments: usize,
}

/// Read-only comparison of the current continuous-input candidate lanes.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateLabReport {
    /// Validated lowercase input.
    pub observed: KeySequence,
    /// Requested visible depth for each lane.
    pub top_k: usize,
    /// Ordinary decoder candidates.
    pub primary: Vec<CandidateLabCandidate>,
    /// Separately exposed anchored-transposition recovery candidates.
    pub anchored_transposition_recovery: Vec<CandidateLabCandidate>,
}

/// Builds a candidate-lab report without reading private data or learning from input.
pub fn analyze_candidate_lab(
    decoder: &Decoder,
    observed: &str,
    top_k: usize,
) -> Result<CandidateLabReport, CandidateLabError> {
    let observed = KeySequence::new(observed)?;
    if !(1..=MAX_CANDIDATE_LAB_TOP_K).contains(&top_k) {
        return Err(CandidateLabError::InvalidTopK {
            requested: top_k,
            maximum: MAX_CANDIDATE_LAB_TOP_K,
        });
    }

    let lanes = decoder.decode_sentence_lanes(observed.as_str(), top_k)?;
    let observed_letter_keys = observed.as_str().len();
    let primary = summarize_lane(
        CandidateLabLane::Primary,
        observed_letter_keys,
        lanes.primary,
    );
    let anchored_transposition_recovery = summarize_lane(
        CandidateLabLane::AnchoredTranspositionRecovery,
        observed_letter_keys,
        lanes.anchored_transposition_recovery,
    );

    Ok(CandidateLabReport {
        observed,
        top_k,
        primary,
        anchored_transposition_recovery,
    })
}

fn summarize_lane(
    lane: CandidateLabLane,
    observed_letter_keys: usize,
    candidates: Vec<SentenceCandidate>,
) -> Vec<CandidateLabCandidate> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            summarize_candidate(lane, index + 1, observed_letter_keys, candidate)
        })
        .collect()
}

fn summarize_candidate(
    lane: CandidateLabLane,
    rank: usize,
    observed_letter_keys: usize,
    candidate: SentenceCandidate,
) -> CandidateLabCandidate {
    let canonical_full_letter_keys = candidate
        .segments
        .iter()
        .all(|segment| segment.candidate.source == CandidateSource::Lexicon)
        .then(|| {
            candidate
                .segments
                .iter()
                .map(|segment| segment.candidate.code.as_str().len())
                .fold(0_usize, usize::saturating_add)
        });
    let abbreviated_syllables = candidate
        .segments
        .iter()
        .map(|segment| segment.candidate.spelling.abbreviated_syllables.len())
        .fold(0_usize, usize::saturating_add);
    let corrected_segments = candidate
        .segments
        .iter()
        .filter(|segment| !matches!(segment.candidate.correction, Correction::Exact))
        .count();
    let selection_actions = 1;
    let lane_activation_actions = lane.activation_actions();
    let projected_actions_one_selection = observed_letter_keys
        .saturating_add(selection_actions)
        .saturating_add(lane_activation_actions);
    let net_actions_saved_vs_full = canonical_full_letter_keys.map(|full_letters| {
        signed_difference(
            full_letters.saturating_add(selection_actions),
            projected_actions_one_selection,
        )
    });

    CandidateLabCandidate {
        lane,
        rank,
        candidate,
        observed_letter_keys,
        canonical_full_letter_keys,
        selection_actions,
        lane_activation_actions,
        projected_actions_one_selection,
        net_actions_saved_vs_full,
        abbreviated_syllables,
        corrected_segments,
    }
}

fn signed_difference(left: usize, right: usize) -> isize {
    if left >= right {
        isize::try_from(left - right).unwrap_or(isize::MAX)
    } else {
        -isize::try_from(right - left).unwrap_or(isize::MAX)
    }
}

/// Candidate-lab configuration or input error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateLabError {
    /// The observed key sequence is empty or not lowercase ASCII.
    InvalidInput(KeySequenceError),
    /// The visible candidate depth is outside the bounded lab protocol.
    InvalidTopK {
        /// Caller-provided depth.
        requested: usize,
        /// Largest accepted depth.
        maximum: usize,
    },
}

impl fmt::Display for CandidateLabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(error) => error.fmt(formatter),
            Self::InvalidTopK { requested, maximum } => write!(
                formatter,
                "候选实验台 Top-K 必须在 1..={maximum}，实际收到 {requested}"
            ),
        }
    }
}

impl Error for CandidateLabError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidInput(error) => Some(error),
            Self::InvalidTopK { .. } => None,
        }
    }
}

impl From<KeySequenceError> for CandidateLabError {
    fn from(error: KeySequenceError) -> Self {
        Self::InvalidInput(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_lexicon_tsv;

    const LEXICON: &str = "\
text\tpinyin\tfrequency
猫猫\tmao mao\t100
";

    fn decoder() -> Decoder {
        Decoder::new(parse_lexicon_tsv(LEXICON).unwrap())
    }

    fn cat_candidate(report: &CandidateLabReport) -> &CandidateLabCandidate {
        report
            .primary
            .iter()
            .find(|row| row.candidate.text == "猫猫")
            .unwrap()
    }

    #[test]
    fn full_code_projection_matches_one_selection_baseline() {
        let report = analyze_candidate_lab(&decoder(), "mkmk", 10).unwrap();
        let row = cat_candidate(&report);
        assert_eq!(row.canonical_full_letter_keys, Some(4));
        assert_eq!(row.observed_letter_keys, 4);
        assert_eq!(row.projected_actions_one_selection, 5);
        assert_eq!(row.net_actions_saved_vs_full, Some(0));
        assert_eq!(row.abbreviated_syllables, 0);
        assert_eq!(row.corrected_segments, 0);
    }

    #[test]
    fn abbreviation_projection_reports_one_saved_action() {
        let report = analyze_candidate_lab(&decoder(), "mkm", 10).unwrap();
        let row = cat_candidate(&report);
        assert_eq!(row.canonical_full_letter_keys, Some(4));
        assert_eq!(row.observed_letter_keys, 3);
        assert_eq!(row.selection_actions, 1);
        assert_eq!(row.lane_activation_actions, 0);
        assert_eq!(row.projected_actions_one_selection, 4);
        assert_eq!(row.net_actions_saved_vs_full, Some(1));
        assert_eq!(row.abbreviated_syllables, 1);
    }

    #[test]
    fn recovery_lane_pays_one_explicit_activation_action() {
        let report = analyze_candidate_lab(&decoder(), "mkm", 10).unwrap();
        let row = summarize_candidate(
            CandidateLabLane::AnchoredTranspositionRecovery,
            1,
            3,
            cat_candidate(&report).candidate.clone(),
        );
        assert_eq!(row.lane_activation_actions, 1);
        assert_eq!(row.projected_actions_one_selection, 5);
        assert_eq!(row.net_actions_saved_vs_full, Some(0));
    }

    #[test]
    fn unresolved_candidate_has_no_invented_full_code_baseline() {
        let report = analyze_candidate_lab(&decoder(), "q", 10).unwrap();
        let unresolved = report
            .primary
            .iter()
            .find(|row| row.candidate.unresolved_key_count > 0)
            .unwrap();
        assert_eq!(unresolved.canonical_full_letter_keys, None);
        assert_eq!(unresolved.net_actions_saved_vs_full, None);
    }

    #[test]
    fn lab_rejects_empty_excessive_and_malformed_requests() {
        assert!(matches!(
            analyze_candidate_lab(&decoder(), "mk", 0),
            Err(CandidateLabError::InvalidTopK { requested: 0, .. })
        ));
        assert!(matches!(
            analyze_candidate_lab(&decoder(), "mk", 11),
            Err(CandidateLabError::InvalidTopK { requested: 11, .. })
        ));
        assert!(matches!(
            analyze_candidate_lab(&decoder(), "MK", 10),
            Err(CandidateLabError::InvalidInput(_))
        ));
    }
}
