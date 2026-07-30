//! Bounded, exact-code retrieval of confirmed personal coverage candidates.
//!
//! Results borrow text from [`crate::PendingSelectionMemory`] and deliberately
//! do not implement `Debug`. The only debuggable report is a redacted numeric
//! summary. This module does not merge results into a decoder or TSF candidate
//! list and performs no I/O.

use std::error::Error;
use std::fmt;

use crate::adaptive_memory::validate_selection_text;
use crate::{ConfirmedSelectionTier, PendingSelectionError, PendingSelectionMemory};

pub const MAX_ADAPTIVE_COVERAGE_CANDIDATES: usize = 16;
pub const MAX_ADAPTIVE_COVERAGE_PUBLIC_TEXTS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveCoverageConfig {
    pub minimum_confirmations: u64,
    pub max_candidates: usize,
    pub recent_evidence_weight: f64,
    pub medium_evidence_weight: f64,
    pub long_evidence_weight: f64,
}

impl Default for AdaptiveCoverageConfig {
    fn default() -> Self {
        Self {
            minimum_confirmations: 2,
            max_candidates: 8,
            recent_evidence_weight: 4.0,
            medium_evidence_weight: 2.0,
            long_evidence_weight: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveCoverageSource {
    ConfirmedSelectionHistory,
}

/// One eligible personal candidate borrowed from confirmed memory.
///
/// This type intentionally has no `Debug` implementation because it exposes
/// the candidate text through [`Self::text`].
pub struct AdaptiveCoverageCandidate<'a> {
    text: &'a str,
    pub source: AdaptiveCoverageSource,
    pub confirmations: u64,
    pub tier: ConfirmedSelectionTier,
    pub weighted_evidence: f64,
    pub last_generation: u64,
}

impl<'a> AdaptiveCoverageCandidate<'a> {
    pub fn text(&self) -> &'a str {
        self.text
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AdaptiveCoverageSummary {
    pub examined_confirmed: usize,
    pub below_confirmation_threshold: usize,
    pub excluded_public: usize,
    pub returned_candidates: usize,
    pub truncated_candidates: usize,
}

/// Exact-code coverage results with a separately debuggable redacted summary.
///
/// This type intentionally has no `Debug` implementation because its
/// candidates borrow text.
pub struct AdaptiveCoverageReport<'a> {
    pub summary: AdaptiveCoverageSummary,
    candidates: Vec<AdaptiveCoverageCandidate<'a>>,
}

impl<'a> AdaptiveCoverageReport<'a> {
    pub fn candidates(&self) -> &[AdaptiveCoverageCandidate<'a>] {
        &self.candidates
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveCoverageError {
    InvalidConfig,
    InvalidCode,
    InvalidPublicPool,
    NumericalOverflow,
}

impl fmt::Display for AdaptiveCoverageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidConfig => "adaptive coverage configuration is invalid",
            Self::InvalidCode => "adaptive coverage code is invalid",
            Self::InvalidPublicPool => "adaptive coverage public candidate pool is invalid",
            Self::NumericalOverflow => "adaptive coverage calculation exceeded numeric limits",
        };
        formatter.write_str(message)
    }
}

impl Error for AdaptiveCoverageError {}

pub fn retrieve_personal_coverage<'a>(
    memory: &'a PendingSelectionMemory,
    code: &str,
    public_texts: &[&str],
    config: AdaptiveCoverageConfig,
) -> Result<AdaptiveCoverageReport<'a>, AdaptiveCoverageError> {
    validate_config(config)?;
    validate_public_pool(public_texts)?;

    let mut summary = AdaptiveCoverageSummary::default();
    let mut candidates = Vec::new();
    let mut numerical_overflow = false;
    memory
        .visit_confirmed_for_code(code, |text, evidence| {
            summary.examined_confirmed = summary.examined_confirmed.saturating_add(1);
            if evidence.confirmations < config.minimum_confirmations {
                summary.below_confirmation_threshold =
                    summary.below_confirmation_threshold.saturating_add(1);
                return;
            }
            if public_texts.contains(&text) {
                summary.excluded_public = summary.excluded_public.saturating_add(1);
                return;
            }

            let weighted_evidence =
                tier_weight(config, evidence.tier) * evidence.confirmations as f64;
            if !weighted_evidence.is_finite() {
                numerical_overflow = true;
                return;
            }
            candidates.push(AdaptiveCoverageCandidate {
                text,
                source: AdaptiveCoverageSource::ConfirmedSelectionHistory,
                confirmations: evidence.confirmations,
                tier: evidence.tier,
                weighted_evidence,
                last_generation: evidence.last_generation,
            });
        })
        .map_err(map_code_error)?;
    if numerical_overflow {
        return Err(AdaptiveCoverageError::NumericalOverflow);
    }

    candidates.sort_by(|left, right| {
        right
            .weighted_evidence
            .total_cmp(&left.weighted_evidence)
            .then_with(|| right.confirmations.cmp(&left.confirmations))
            .then_with(|| right.last_generation.cmp(&left.last_generation))
            .then_with(|| left.text.cmp(right.text))
    });
    let eligible = candidates.len();
    candidates.truncate(config.max_candidates);
    summary.returned_candidates = candidates.len();
    summary.truncated_candidates = eligible.saturating_sub(candidates.len());

    Ok(AdaptiveCoverageReport {
        summary,
        candidates,
    })
}

fn validate_config(config: AdaptiveCoverageConfig) -> Result<(), AdaptiveCoverageError> {
    let weights = [
        config.recent_evidence_weight,
        config.medium_evidence_weight,
        config.long_evidence_weight,
    ];
    if config.minimum_confirmations < 2
        || config.max_candidates == 0
        || config.max_candidates > MAX_ADAPTIVE_COVERAGE_CANDIDATES
        || weights.iter().any(|weight| !weight.is_finite())
        || config.recent_evidence_weight <= 0.0
        || config.medium_evidence_weight <= 0.0
        || config.long_evidence_weight <= 0.0
        || config.recent_evidence_weight < config.medium_evidence_weight
        || config.medium_evidence_weight < config.long_evidence_weight
    {
        return Err(AdaptiveCoverageError::InvalidConfig);
    }
    Ok(())
}

fn validate_public_pool(public_texts: &[&str]) -> Result<(), AdaptiveCoverageError> {
    if public_texts.len() > MAX_ADAPTIVE_COVERAGE_PUBLIC_TEXTS {
        return Err(AdaptiveCoverageError::InvalidPublicPool);
    }
    for (index, text) in public_texts.iter().enumerate() {
        if validate_selection_text(text).is_err()
            || public_texts[..index]
                .iter()
                .any(|previous| previous == text)
        {
            return Err(AdaptiveCoverageError::InvalidPublicPool);
        }
    }
    Ok(())
}

fn tier_weight(config: AdaptiveCoverageConfig, tier: ConfirmedSelectionTier) -> f64 {
    match tier {
        ConfirmedSelectionTier::Recent => config.recent_evidence_weight,
        ConfirmedSelectionTier::Medium => config.medium_evidence_weight,
        ConfirmedSelectionTier::Long => config.long_evidence_weight,
    }
}

fn map_code_error(error: PendingSelectionError) -> AdaptiveCoverageError {
    match error {
        PendingSelectionError::InvalidCode => AdaptiveCoverageError::InvalidCode,
        PendingSelectionError::InvalidLimits
        | PendingSelectionError::InvalidText
        | PendingSelectionError::PositionOverflow => AdaptiveCoverageError::InvalidCode,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptiveCoverageConfig, AdaptiveCoverageError, AdaptiveCoverageSource,
        retrieve_personal_coverage,
    };
    use crate::{ConfirmedSelectionTier, PendingSelectionLimits, PendingSelectionMemory};

    fn confirm(
        memory: &mut PendingSelectionMemory,
        code: &str,
        text: &str,
        start: &mut usize,
        times: usize,
    ) {
        for _ in 0..times {
            memory.observe_commit(code, text, *start).unwrap();
            memory.confirm_pending();
            *start += text.chars().count();
        }
    }

    #[test]
    fn pending_and_one_confirmation_stay_below_the_repeated_evidence_gate() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 0).unwrap();

        let pending =
            retrieve_personal_coverage(&memory, "aa", &[], AdaptiveCoverageConfig::default())
                .unwrap();
        assert!(pending.candidates().is_empty());
        assert_eq!(pending.summary.examined_confirmed, 0);

        memory.confirm_pending();
        let once =
            retrieve_personal_coverage(&memory, "aa", &[], AdaptiveCoverageConfig::default())
                .unwrap();
        assert!(once.candidates().is_empty());
        assert_eq!(once.summary.below_confirmation_threshold, 1);

        memory.observe_commit("aa", "甲", 1).unwrap();
        memory.confirm_pending();
        let twice =
            retrieve_personal_coverage(&memory, "aa", &[], AdaptiveCoverageConfig::default())
                .unwrap();
        assert_eq!(twice.candidates().len(), 1);
        assert_eq!(twice.candidates()[0].text(), "甲");
        assert_eq!(twice.candidates()[0].confirmations, 2);
        assert_eq!(
            twice.candidates()[0].source,
            AdaptiveCoverageSource::ConfirmedSelectionHistory
        );
    }

    #[test]
    fn public_candidates_are_not_duplicated_by_the_personal_coverage_lane() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "甲", &mut start, 2);

        let report = retrieve_personal_coverage(
            &memory,
            "aa",
            &["甲", "乙"],
            AdaptiveCoverageConfig::default(),
        )
        .unwrap();

        assert!(report.candidates().is_empty());
        assert_eq!(report.summary.excluded_public, 1);
    }

    #[test]
    fn exact_code_query_keeps_alias_evidence_isolated() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "甲", &mut start, 2);

        let other_code =
            retrieve_personal_coverage(&memory, "ab", &[], AdaptiveCoverageConfig::default())
                .unwrap();

        assert!(other_code.candidates().is_empty());
        assert_eq!(other_code.summary.examined_confirmed, 0);
    }

    #[test]
    fn tier_weight_recency_and_result_cap_produce_a_deterministic_order() {
        let mut memory = PendingSelectionMemory::with_limits(
            PendingSelectionLimits::tiered(8, 1, 1, 1).unwrap(),
        )
        .unwrap();
        let mut start = 0;
        confirm(&mut memory, "aa", "甲", &mut start, 2);
        confirm(&mut memory, "aa", "乙", &mut start, 2);
        confirm(&mut memory, "aa", "丙", &mut start, 3);
        let config = AdaptiveCoverageConfig {
            max_candidates: 2,
            ..AdaptiveCoverageConfig::default()
        };

        let report = retrieve_personal_coverage(&memory, "aa", &[], config).unwrap();

        assert_eq!(
            report
                .candidates()
                .iter()
                .map(|candidate| candidate.text())
                .collect::<Vec<_>>(),
            ["丙", "乙"]
        );
        assert_eq!(report.candidates()[0].tier, ConfirmedSelectionTier::Recent);
        assert_eq!(report.candidates()[1].tier, ConfirmedSelectionTier::Medium);
        assert_eq!(report.summary.returned_candidates, 2);
        assert_eq!(report.summary.truncated_candidates, 1);
    }

    #[test]
    fn explicit_forget_removes_an_eligible_coverage_candidate() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "甲", &mut start, 2);
        memory.forget("aa", "甲").unwrap();

        let report =
            retrieve_personal_coverage(&memory, "aa", &[], AdaptiveCoverageConfig::default())
                .unwrap();

        assert!(report.candidates().is_empty());
    }

    #[test]
    fn invalid_configuration_code_pool_and_numeric_range_are_rejected() {
        let memory = PendingSelectionMemory::new();
        let permissive = AdaptiveCoverageConfig {
            minimum_confirmations: 1,
            ..AdaptiveCoverageConfig::default()
        };
        assert_eq!(
            retrieve_personal_coverage(&memory, "aa", &[], permissive).map(|_| ()),
            Err(AdaptiveCoverageError::InvalidConfig)
        );
        assert_eq!(
            retrieve_personal_coverage(&memory, "A", &[], AdaptiveCoverageConfig::default())
                .map(|_| ()),
            Err(AdaptiveCoverageError::InvalidCode)
        );
        assert_eq!(
            retrieve_personal_coverage(
                &memory,
                "aa",
                &["甲", "甲"],
                AdaptiveCoverageConfig::default()
            )
            .map(|_| ()),
            Err(AdaptiveCoverageError::InvalidPublicPool)
        );

        let mut repeated = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut repeated, "aa", "甲", &mut start, 2);
        let overflowing = AdaptiveCoverageConfig {
            recent_evidence_weight: f64::MAX,
            ..AdaptiveCoverageConfig::default()
        };
        assert_eq!(
            retrieve_personal_coverage(&repeated, "aa", &[], overflowing).map(|_| ()),
            Err(AdaptiveCoverageError::NumericalOverflow)
        );
    }

    #[test]
    fn debuggable_summary_and_errors_never_contain_candidate_text() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "secretcode", "私密测试文字", &mut start, 2);

        let report = retrieve_personal_coverage(
            &memory,
            "secretcode",
            &[],
            AdaptiveCoverageConfig::default(),
        )
        .unwrap();
        let summary_debug = format!("{:?}", report.summary);
        let error_debug = format!("{:?}", AdaptiveCoverageError::InvalidCode);

        assert!(!summary_debug.contains("私密测试文字"));
        assert!(!summary_debug.contains("secretcode"));
        assert!(!error_debug.contains("私密测试文字"));
        assert!(!error_debug.contains("secretcode"));
    }
}
