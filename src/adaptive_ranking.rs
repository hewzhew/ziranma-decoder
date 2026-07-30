//! Read-only probability mixing over a caller-supplied candidate pool.
//!
//! The caller retains all candidate text. Results identify candidates only by
//! their original indices and contain numeric evidence, so this module does
//! not create a second text store or a content-bearing debug surface.
//!
//! This research helper cannot add candidates, persist state, or change the
//! TSF path. Pending selections are invisible because it reads only confirmed
//! evidence from [`crate::PendingSelectionMemory`].

use std::error::Error;
use std::fmt;

use crate::{ConfirmedSelectionTier, PendingSelectionError, PendingSelectionMemory};

pub const MAX_ADAPTIVE_RANKING_CANDIDATES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveRankingConfig {
    pub max_personal_mix: f64,
    pub evidence_saturation: f64,
    pub additive_smoothing: f64,
    pub recent_evidence_weight: f64,
    pub medium_evidence_weight: f64,
    pub long_evidence_weight: f64,
}

impl Default for AdaptiveRankingConfig {
    fn default() -> Self {
        Self {
            max_personal_mix: 0.25,
            evidence_saturation: 4.0,
            additive_smoothing: 0.5,
            recent_evidence_weight: 4.0,
            medium_evidence_weight: 2.0,
            long_evidence_weight: 1.0,
        }
    }
}

/// One candidate in the fixed pool supplied by the caller.
///
/// This type intentionally has no `Debug` implementation because it borrows
/// candidate text.
pub struct AdaptiveRankingCandidate<'a> {
    pub text: &'a str,
    pub public_weight: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveCandidateScore {
    pub original_index: usize,
    pub public_probability: f64,
    pub personal_probability: f64,
    pub mixed_probability: f64,
    pub confirmations: u64,
    pub tier: Option<ConfirmedSelectionTier>,
    pub weighted_evidence: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdaptiveRankingReport {
    pub personal_mix: f64,
    pub total_weighted_evidence: f64,
    pub candidates: Vec<AdaptiveCandidateScore>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveRankingError {
    InvalidConfig,
    InvalidCandidateCount,
    InvalidCandidateIdentity,
    DuplicateCandidateText,
    InvalidPublicWeight,
    NumericalOverflow,
}

impl fmt::Display for AdaptiveRankingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidConfig => "adaptive ranking configuration is invalid",
            Self::InvalidCandidateCount => "adaptive ranking requires 1-50 candidates",
            Self::InvalidCandidateIdentity => "adaptive ranking candidate identity is invalid",
            Self::DuplicateCandidateText => "adaptive ranking candidate text must be unique",
            Self::InvalidPublicWeight => {
                "adaptive ranking weights must be finite, nonnegative, and not all zero"
            }
            Self::NumericalOverflow => "adaptive ranking calculation exceeded numeric limits",
        };
        formatter.write_str(message)
    }
}

impl Error for AdaptiveRankingError {}

struct CandidateEvidence {
    original_index: usize,
    public_weight: f64,
    confirmations: u64,
    tier: Option<ConfirmedSelectionTier>,
    weighted_evidence: f64,
}

pub fn rank_visible_candidates(
    memory: &PendingSelectionMemory,
    code: &str,
    candidates: &[AdaptiveRankingCandidate<'_>],
    config: AdaptiveRankingConfig,
) -> Result<AdaptiveRankingReport, AdaptiveRankingError> {
    validate_config(config)?;
    if candidates.is_empty() || candidates.len() > MAX_ADAPTIVE_RANKING_CANDIDATES {
        return Err(AdaptiveRankingError::InvalidCandidateCount);
    }

    let mut public_total = 0.0;
    let mut total_weighted_evidence = 0.0;
    let mut evidence = Vec::with_capacity(candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        if !candidate.public_weight.is_finite() || candidate.public_weight < 0.0 {
            return Err(AdaptiveRankingError::InvalidPublicWeight);
        }
        if candidates[..index]
            .iter()
            .any(|previous| previous.text == candidate.text)
        {
            return Err(AdaptiveRankingError::DuplicateCandidateText);
        }

        let confirmed = memory
            .confirmed_evidence(code, candidate.text)
            .map_err(map_identity_error)?;
        let confirmations = confirmed.map_or(0, |value| value.confirmations);
        let tier = confirmed.map(|value| value.tier);
        let tier_weight = match tier {
            Some(ConfirmedSelectionTier::Recent) => config.recent_evidence_weight,
            Some(ConfirmedSelectionTier::Medium) => config.medium_evidence_weight,
            Some(ConfirmedSelectionTier::Long) => config.long_evidence_weight,
            None => 0.0,
        };
        let weighted_evidence = tier_weight * confirmations as f64;
        if !weighted_evidence.is_finite() {
            return Err(AdaptiveRankingError::NumericalOverflow);
        }

        public_total += candidate.public_weight;
        total_weighted_evidence += weighted_evidence;
        evidence.push(CandidateEvidence {
            original_index: index,
            public_weight: candidate.public_weight,
            confirmations,
            tier,
            weighted_evidence,
        });
    }

    if !public_total.is_finite() || public_total <= 0.0 {
        return Err(AdaptiveRankingError::InvalidPublicWeight);
    }
    if !total_weighted_evidence.is_finite() {
        return Err(AdaptiveRankingError::NumericalOverflow);
    }
    let smoothing_total = config.additive_smoothing * candidates.len() as f64;
    let personal_denominator = total_weighted_evidence + smoothing_total;
    if !smoothing_total.is_finite() || !personal_denominator.is_finite() {
        return Err(AdaptiveRankingError::NumericalOverflow);
    }
    let personal_mix = if total_weighted_evidence == 0.0 {
        0.0
    } else {
        config.max_personal_mix * total_weighted_evidence
            / (total_weighted_evidence + config.evidence_saturation)
    };

    let mut scores = evidence
        .into_iter()
        .map(|candidate| {
            let public_probability = candidate.public_weight / public_total;
            let personal_probability =
                (candidate.weighted_evidence + config.additive_smoothing) / personal_denominator;
            let mixed_probability =
                (1.0 - personal_mix) * public_probability + personal_mix * personal_probability;
            AdaptiveCandidateScore {
                original_index: candidate.original_index,
                public_probability,
                personal_probability,
                mixed_probability,
                confirmations: candidate.confirmations,
                tier: candidate.tier,
                weighted_evidence: candidate.weighted_evidence,
            }
        })
        .collect::<Vec<_>>();
    scores.sort_by(|left, right| {
        right
            .mixed_probability
            .total_cmp(&left.mixed_probability)
            .then_with(|| right.public_probability.total_cmp(&left.public_probability))
            .then_with(|| left.original_index.cmp(&right.original_index))
    });

    Ok(AdaptiveRankingReport {
        personal_mix,
        total_weighted_evidence,
        candidates: scores,
    })
}

fn validate_config(config: AdaptiveRankingConfig) -> Result<(), AdaptiveRankingError> {
    let values = [
        config.max_personal_mix,
        config.evidence_saturation,
        config.additive_smoothing,
        config.recent_evidence_weight,
        config.medium_evidence_weight,
        config.long_evidence_weight,
    ];
    if values.iter().any(|value| !value.is_finite())
        || !(0.0..1.0).contains(&config.max_personal_mix)
        || config.evidence_saturation <= 0.0
        || config.additive_smoothing <= 0.0
        || config.recent_evidence_weight <= 0.0
        || config.medium_evidence_weight < 0.0
        || config.long_evidence_weight < 0.0
        || config.recent_evidence_weight < config.medium_evidence_weight
        || config.medium_evidence_weight < config.long_evidence_weight
    {
        return Err(AdaptiveRankingError::InvalidConfig);
    }
    Ok(())
}

fn map_identity_error(_error: PendingSelectionError) -> AdaptiveRankingError {
    AdaptiveRankingError::InvalidCandidateIdentity
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptiveRankingCandidate, AdaptiveRankingConfig, AdaptiveRankingError,
        rank_visible_candidates,
    };
    use crate::{ConfirmedSelectionTier, PendingSelectionLimits, PendingSelectionMemory};

    fn candidates<'a>(values: &'a [(&'a str, f64)]) -> Vec<AdaptiveRankingCandidate<'a>> {
        values
            .iter()
            .map(|(text, public_weight)| AdaptiveRankingCandidate {
                text,
                public_weight: *public_weight,
            })
            .collect()
    }

    fn assert_close(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-12, "{left} != {right}");
    }

    #[test]
    fn no_confirmed_evidence_preserves_public_order_and_probabilities() {
        let memory = PendingSelectionMemory::new();
        let pool = candidates(&[("甲", 3.0), ("乙", 2.0), ("丙", 1.0)]);

        let report =
            rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
                .unwrap();

        assert_eq!(
            report
                .candidates
                .iter()
                .map(|candidate| candidate.original_index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(report.personal_mix, 0.0);
        assert_close(report.candidates[0].mixed_probability, 0.5);
        assert_close(report.candidates[1].mixed_probability, 1.0 / 3.0);
        assert_close(report.candidates[2].mixed_probability, 1.0 / 6.0);
    }

    #[test]
    fn pending_answer_cannot_affect_its_own_prediction() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "乙", 0).unwrap();
        let pool = candidates(&[("甲", 0.51), ("乙", 0.49)]);

        let before =
            rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
                .unwrap();
        assert_eq!(before.candidates[0].original_index, 0);
        assert_eq!(before.personal_mix, 0.0);

        memory.confirm_pending();
        let after = rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
            .unwrap();
        assert_eq!(after.candidates[0].original_index, 1);
        assert!(after.personal_mix > 0.0);
    }

    #[test]
    fn recent_medium_and_long_evidence_have_distinct_bounded_weights() {
        let mut memory = PendingSelectionMemory::with_limits(
            PendingSelectionLimits::tiered(4, 1, 1, 1).unwrap(),
        )
        .unwrap();
        for (text, start) in [("甲", 0), ("乙", 1), ("丙", 2)] {
            memory.observe_commit("aa", text, start).unwrap();
            memory.confirm_pending();
        }
        let pool = candidates(&[("甲", 1.0), ("乙", 1.0), ("丙", 1.0)]);

        let report =
            rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
                .unwrap();

        assert_eq!(
            report
                .candidates
                .iter()
                .map(|candidate| candidate.original_index)
                .collect::<Vec<_>>(),
            [2, 1, 0]
        );
        assert_eq!(
            report.candidates[0].tier,
            Some(ConfirmedSelectionTier::Recent)
        );
        assert_eq!(
            report.candidates[1].tier,
            Some(ConfirmedSelectionTier::Medium)
        );
        assert_eq!(
            report.candidates[2].tier,
            Some(ConfirmedSelectionTier::Long)
        );
        assert_eq!(
            report
                .candidates
                .iter()
                .map(|candidate| candidate.weighted_evidence)
                .collect::<Vec<_>>(),
            [4.0, 2.0, 1.0]
        );
    }

    #[test]
    fn evidence_is_isolated_by_the_observed_code() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "乙", 0).unwrap();
        memory.confirm_pending();
        let pool = candidates(&[("甲", 0.51), ("乙", 0.49)]);

        let other_code =
            rank_visible_candidates(&memory, "ab", &pool, AdaptiveRankingConfig::default())
                .unwrap();

        assert_eq!(other_code.personal_mix, 0.0);
        assert_eq!(other_code.candidates[0].original_index, 0);
    }

    #[test]
    fn personal_mix_approaches_but_never_reaches_its_configured_cap() {
        let mut memory = PendingSelectionMemory::new();
        for start in 0..100 {
            memory.observe_commit("aa", "乙", start).unwrap();
            memory.confirm_pending();
        }
        let pool = candidates(&[("甲", 0.9), ("乙", 0.1)]);
        let config = AdaptiveRankingConfig {
            max_personal_mix: 0.4,
            ..AdaptiveRankingConfig::default()
        };

        let report = rank_visible_candidates(&memory, "aa", &pool, config).unwrap();

        assert!(report.personal_mix > 0.39);
        assert!(report.personal_mix < config.max_personal_mix);
        assert_close(
            report
                .candidates
                .iter()
                .map(|candidate| candidate.mixed_probability)
                .sum(),
            1.0,
        );
    }

    #[test]
    fn tied_candidates_keep_the_callers_original_order() {
        let memory = PendingSelectionMemory::new();
        let pool = candidates(&[("丙", 1.0), ("甲", 1.0), ("乙", 1.0)]);

        let first = rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
            .unwrap();
        let second =
            rank_visible_candidates(&memory, "aa", &pool, AdaptiveRankingConfig::default())
                .unwrap();

        assert_eq!(first, second);
        assert_eq!(
            first
                .candidates
                .iter()
                .map(|candidate| candidate.original_index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn malformed_configuration_candidates_and_weights_are_rejected() {
        let memory = PendingSelectionMemory::new();
        let one = candidates(&[("甲", 1.0)]);
        let invalid_config = AdaptiveRankingConfig {
            medium_evidence_weight: 5.0,
            ..AdaptiveRankingConfig::default()
        };
        assert_eq!(
            rank_visible_candidates(&memory, "aa", &one, invalid_config),
            Err(AdaptiveRankingError::InvalidConfig)
        );
        assert_eq!(
            rank_visible_candidates(&memory, "aa", &[], AdaptiveRankingConfig::default()),
            Err(AdaptiveRankingError::InvalidCandidateCount)
        );
        let duplicates = candidates(&[("甲", 1.0), ("甲", 2.0)]);
        assert_eq!(
            rank_visible_candidates(&memory, "aa", &duplicates, AdaptiveRankingConfig::default()),
            Err(AdaptiveRankingError::DuplicateCandidateText)
        );
        let zero = candidates(&[("甲", 0.0), ("乙", 0.0)]);
        assert_eq!(
            rank_visible_candidates(&memory, "aa", &zero, AdaptiveRankingConfig::default()),
            Err(AdaptiveRankingError::InvalidPublicWeight)
        );
        let invalid_text = candidates(&[("", 1.0)]);
        assert_eq!(
            rank_visible_candidates(
                &memory,
                "aa",
                &invalid_text,
                AdaptiveRankingConfig::default()
            ),
            Err(AdaptiveRankingError::InvalidCandidateIdentity)
        );

        let mut repeated = PendingSelectionMemory::new();
        repeated.observe_commit("aa", "甲", 0).unwrap();
        repeated.confirm_pending();
        let overflowing_config = AdaptiveRankingConfig {
            recent_evidence_weight: f64::MAX,
            ..AdaptiveRankingConfig::default()
        };
        repeated.observe_commit("aa", "甲", 1).unwrap();
        repeated.confirm_pending();
        assert_eq!(
            rank_visible_candidates(&repeated, "aa", &one, overflowing_config),
            Err(AdaptiveRankingError::NumericalOverflow)
        );
    }

    #[test]
    fn ranking_report_debug_contains_indices_and_numbers_but_no_text() {
        let mut memory = PendingSelectionMemory::new();
        memory
            .observe_commit("secretcode", "私密测试文字", 0)
            .unwrap();
        memory.confirm_pending();
        let pool = candidates(&[("公开甲", 0.5), ("私密测试文字", 0.5)]);

        let report = rank_visible_candidates(
            &memory,
            "secretcode",
            &pool,
            AdaptiveRankingConfig::default(),
        )
        .unwrap();
        let debug = format!("{report:?}");

        assert!(!debug.contains("公开甲"));
        assert!(!debug.contains("私密测试文字"));
        assert!(!debug.contains("secretcode"));
    }
}
