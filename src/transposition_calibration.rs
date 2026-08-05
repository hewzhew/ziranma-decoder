//! Bounded, deterministic calibration for automatic double-pinyin
//! transposition exposure.
//!
//! The model stores only aggregate accepted/rejected counts in a fixed table.
//! It performs no I/O and cannot discover private feedback by itself. Unknown
//! outcomes are counted for auditability but never treated as negative labels.

use std::error::Error;
use std::fmt;

use crate::{
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS, NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS,
    NativeAutomaticTranspositionTier,
};

const LATIN_LETTERS: usize = 26;
const ORDERED_PAIR_COUNT: usize = LATIN_LETTERS * LATIN_LETTERS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranspositionCalibrationLabel {
    Accepted,
    Rejected,
    Unknown,
}

/// One private observation reduced to a fixed pair identity and numeric
/// evidence. It deliberately omits `Debug` so the encoded key pair is not
/// exposed through routine diagnostics.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct TranspositionCalibrationObservation {
    pair_index: usize,
    gap_ms: u32,
    cold_tier: NativeAutomaticTranspositionTier,
    label: TranspositionCalibrationLabel,
}

impl TranspositionCalibrationObservation {
    pub fn from_code(
        code: &str,
        syllable_index: usize,
        gap_ms: u32,
        cold_tier: NativeAutomaticTranspositionTier,
        label: TranspositionCalibrationLabel,
    ) -> Result<Self, TranspositionCalibrationError> {
        let start = syllable_index
            .checked_mul(2)
            .ok_or(TranspositionCalibrationError::InvalidIdentity)?;
        let pair = code
            .as_bytes()
            .get(start..start.saturating_add(2))
            .ok_or(TranspositionCalibrationError::InvalidIdentity)?;
        if code.is_empty()
            || code.len() > 64
            || !code.as_bytes().iter().all(u8::is_ascii_lowercase)
            || pair.len() != 2
        {
            return Err(TranspositionCalibrationError::InvalidIdentity);
        }
        let left = usize::from(pair[0] - b'a');
        let right = usize::from(pair[1] - b'a');
        Ok(Self {
            pair_index: left * LATIN_LETTERS + right,
            gap_ms,
            cold_tier,
            label,
        })
    }

    pub fn gap_ms(self) -> u32 {
        self.gap_ms
    }

    pub fn cold_tier(self) -> NativeAutomaticTranspositionTier {
        self.cold_tier
    }

    pub fn label(self) -> TranspositionCalibrationLabel {
        self.label
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranspositionCalibrationConfig {
    pub minimum_global_labels: u32,
    pub minimum_pair_labels: u32,
    pub global_prior_strength: f64,
    pub pair_shrinkage_strength: f64,
    pub primary_probability: f64,
    pub secondary_probability: f64,
}

impl Default for TranspositionCalibrationConfig {
    fn default() -> Self {
        Self {
            minimum_global_labels: 24,
            minimum_pair_labels: 8,
            global_prior_strength: 16.0,
            pair_shrinkage_strength: 8.0,
            primary_probability: 0.72,
            secondary_probability: 0.40,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LabeledEvidence {
    accepted: u32,
    rejected: u32,
}

impl LabeledEvidence {
    fn observe(&mut self, label: TranspositionCalibrationLabel) {
        match label {
            TranspositionCalibrationLabel::Accepted => {
                self.accepted = self.accepted.saturating_add(1);
            }
            TranspositionCalibrationLabel::Rejected => {
                self.rejected = self.rejected.saturating_add(1);
            }
            TranspositionCalibrationLabel::Unknown => {}
        }
    }

    fn labels(self) -> u32 {
        self.accepted.saturating_add(self.rejected)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TranspositionCalibrationSummary {
    pub observations: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub unknown: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranspositionCalibrationRecommendation {
    pub cold_tier: NativeAutomaticTranspositionTier,
    pub recommended_tier: NativeAutomaticTranspositionTier,
    pub acceptance_probability: f64,
    pub global_labels: u32,
    pub pair_labels: u32,
    pub personalized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranspositionCalibrationError {
    InvalidConfig,
    InvalidIdentity,
}

impl fmt::Display for TranspositionCalibrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfig => "transposition calibration configuration is invalid",
            Self::InvalidIdentity => "transposition calibration identity is invalid",
        })
    }
}

impl Error for TranspositionCalibrationError {}

pub struct TranspositionCalibrator {
    config: TranspositionCalibrationConfig,
    global: [LabeledEvidence; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
    pairs: [[LabeledEvidence; NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS]; ORDERED_PAIR_COUNT],
    summary: TranspositionCalibrationSummary,
}

impl Default for TranspositionCalibrator {
    fn default() -> Self {
        Self::new(TranspositionCalibrationConfig::default())
            .expect("the built-in transposition calibration configuration is valid")
    }
}

impl TranspositionCalibrator {
    pub fn new(
        config: TranspositionCalibrationConfig,
    ) -> Result<Self, TranspositionCalibrationError> {
        if config.minimum_global_labels == 0
            || config.minimum_pair_labels == 0
            || !config.global_prior_strength.is_finite()
            || config.global_prior_strength <= 0.0
            || !config.pair_shrinkage_strength.is_finite()
            || config.pair_shrinkage_strength <= 0.0
            || !config.primary_probability.is_finite()
            || !config.secondary_probability.is_finite()
            || !(0.0..1.0).contains(&config.secondary_probability)
            || !(0.0..1.0).contains(&config.primary_probability)
            || config.primary_probability <= config.secondary_probability
        {
            return Err(TranspositionCalibrationError::InvalidConfig);
        }
        Ok(Self {
            config,
            global: [LabeledEvidence::default(); NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS],
            pairs: [[LabeledEvidence::default(); NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS];
                ORDERED_PAIR_COUNT],
            summary: TranspositionCalibrationSummary::default(),
        })
    }

    pub fn observe(&mut self, observation: TranspositionCalibrationObservation) {
        self.summary.observations = self.summary.observations.saturating_add(1);
        match observation.label {
            TranspositionCalibrationLabel::Accepted => {
                self.summary.accepted = self.summary.accepted.saturating_add(1);
            }
            TranspositionCalibrationLabel::Rejected => {
                self.summary.rejected = self.summary.rejected.saturating_add(1);
            }
            TranspositionCalibrationLabel::Unknown => {
                self.summary.unknown = self.summary.unknown.saturating_add(1);
                return;
            }
        }
        let bucket = gap_bucket(observation.gap_ms);
        self.global[bucket].observe(observation.label);
        self.pairs[observation.pair_index][bucket].observe(observation.label);
    }

    pub fn recommendation(
        &self,
        probe: TranspositionCalibrationObservation,
    ) -> TranspositionCalibrationRecommendation {
        let bucket = gap_bucket(probe.gap_ms);
        let global = self.global[bucket];
        let pair = self.pairs[probe.pair_index][bucket];
        let global_labels = global.labels();
        let pair_labels = pair.labels();
        let personalized = global_labels >= self.config.minimum_global_labels
            || pair_labels >= self.config.minimum_pair_labels;
        let prior = cold_prior_probability(probe.cold_tier, self.config);
        if !personalized {
            return TranspositionCalibrationRecommendation {
                cold_tier: probe.cold_tier,
                recommended_tier: probe.cold_tier,
                acceptance_probability: prior,
                global_labels,
                pair_labels,
                personalized: false,
            };
        }

        let global_probability = (prior * self.config.global_prior_strength
            + f64::from(global.accepted))
            / (self.config.global_prior_strength + f64::from(global_labels));
        let pair_probability = (global_probability * self.config.pair_shrinkage_strength
            + f64::from(pair.accepted))
            / (self.config.pair_shrinkage_strength + f64::from(pair_labels));
        let target = if pair_probability >= self.config.primary_probability {
            NativeAutomaticTranspositionTier::Primary
        } else if pair_probability >= self.config.secondary_probability {
            NativeAutomaticTranspositionTier::Secondary
        } else {
            NativeAutomaticTranspositionTier::Shadow
        };
        TranspositionCalibrationRecommendation {
            cold_tier: probe.cold_tier,
            recommended_tier: one_step_toward(probe.cold_tier, target),
            acceptance_probability: pair_probability,
            global_labels,
            pair_labels,
            personalized: true,
        }
    }

    pub fn summary(&self) -> TranspositionCalibrationSummary {
        self.summary
    }
}

fn gap_bucket(gap_ms: u32) -> usize {
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS
        .iter()
        .position(|upper_bound| u64::from(gap_ms) < *upper_bound)
        .unwrap_or(NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKETS - 1)
}

fn cold_prior_probability(
    tier: NativeAutomaticTranspositionTier,
    config: TranspositionCalibrationConfig,
) -> f64 {
    match tier {
        NativeAutomaticTranspositionTier::Primary => (1.0 + config.primary_probability) / 2.0,
        NativeAutomaticTranspositionTier::Secondary => {
            (config.primary_probability + config.secondary_probability) / 2.0
        }
        NativeAutomaticTranspositionTier::Shadow => config.secondary_probability / 2.0,
    }
}

fn one_step_toward(
    cold: NativeAutomaticTranspositionTier,
    target: NativeAutomaticTranspositionTier,
) -> NativeAutomaticTranspositionTier {
    use NativeAutomaticTranspositionTier::{Primary, Secondary, Shadow};
    match (cold, target) {
        (Primary, Shadow) => Secondary,
        (Shadow, Primary) => Secondary,
        (_, target) => target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        code: &str,
        gap_ms: u32,
        tier: NativeAutomaticTranspositionTier,
        label: TranspositionCalibrationLabel,
    ) -> TranspositionCalibrationObservation {
        TranspositionCalibrationObservation::from_code(code, 0, gap_ms, tier, label).unwrap()
    }

    #[test]
    fn cold_start_and_unknown_outcomes_preserve_the_fixed_tier() {
        let mut calibrator =
            TranspositionCalibrator::new(TranspositionCalibrationConfig::default()).unwrap();
        for _ in 0..100 {
            calibrator.observe(observation(
                "am",
                31,
                NativeAutomaticTranspositionTier::Primary,
                TranspositionCalibrationLabel::Unknown,
            ));
        }
        let report = calibrator.recommendation(observation(
            "am",
            31,
            NativeAutomaticTranspositionTier::Primary,
            TranspositionCalibrationLabel::Unknown,
        ));
        assert!(!report.personalized);
        assert_eq!(
            report.recommended_tier,
            NativeAutomaticTranspositionTier::Primary
        );
        assert_eq!(
            calibrator.summary(),
            TranspositionCalibrationSummary {
                observations: 100,
                unknown: 100,
                ..TranspositionCalibrationSummary::default()
            }
        );
    }

    #[test]
    fn repeated_pair_acceptance_can_promote_only_one_exposure_step() {
        let mut calibrator =
            TranspositionCalibrator::new(TranspositionCalibrationConfig::default()).unwrap();
        for _ in 0..8 {
            calibrator.observe(observation(
                "am",
                55,
                NativeAutomaticTranspositionTier::Secondary,
                TranspositionCalibrationLabel::Accepted,
            ));
        }
        let report = calibrator.recommendation(observation(
            "am",
            55,
            NativeAutomaticTranspositionTier::Secondary,
            TranspositionCalibrationLabel::Unknown,
        ));
        assert!(report.personalized);
        assert_eq!(report.pair_labels, 8);
        assert_eq!(
            report.recommended_tier,
            NativeAutomaticTranspositionTier::Primary
        );
    }

    #[test]
    fn repeated_pair_rejection_demotes_primary_by_at_most_one_step() {
        let mut calibrator =
            TranspositionCalibrator::new(TranspositionCalibrationConfig::default()).unwrap();
        for _ in 0..8 {
            calibrator.observe(observation(
                "am",
                31,
                NativeAutomaticTranspositionTier::Primary,
                TranspositionCalibrationLabel::Rejected,
            ));
        }
        let report = calibrator.recommendation(observation(
            "am",
            31,
            NativeAutomaticTranspositionTier::Primary,
            TranspositionCalibrationLabel::Unknown,
        ));
        assert!(report.personalized);
        assert_eq!(
            report.recommended_tier,
            NativeAutomaticTranspositionTier::Secondary
        );
    }

    #[test]
    fn sparse_evidence_for_another_pair_does_not_cross_the_pair_gate() {
        let mut calibrator =
            TranspositionCalibrator::new(TranspositionCalibrationConfig::default()).unwrap();
        for _ in 0..8 {
            calibrator.observe(observation(
                "am",
                55,
                NativeAutomaticTranspositionTier::Secondary,
                TranspositionCalibrationLabel::Accepted,
            ));
        }
        let report = calibrator.recommendation(observation(
            "ma",
            55,
            NativeAutomaticTranspositionTier::Secondary,
            TranspositionCalibrationLabel::Unknown,
        ));
        assert!(!report.personalized);
        assert_eq!(
            report.recommended_tier,
            NativeAutomaticTranspositionTier::Secondary
        );
    }

    #[test]
    fn identity_and_configuration_validation_fail_closed() {
        assert!(matches!(
            TranspositionCalibrationObservation::from_code(
                "a",
                0,
                1,
                NativeAutomaticTranspositionTier::Primary,
                TranspositionCalibrationLabel::Accepted,
            ),
            Err(TranspositionCalibrationError::InvalidIdentity)
        ));
        let mut config = TranspositionCalibrationConfig::default();
        config.primary_probability = config.secondary_probability;
        assert!(matches!(
            TranspositionCalibrator::new(config),
            Err(TranspositionCalibrationError::InvalidConfig)
        ));
    }
}
