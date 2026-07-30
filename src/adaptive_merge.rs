//! Synthetic merge of public candidates and bounded personal coverage.
//!
//! Public candidates first use the read-only adaptive reranker. Eligible
//! exact-code coverage candidates then share a strictly capped probability
//! mass. Returned probabilities remain fractions of the complete merged
//! distribution even when the result limit truncates entries, so truncation
//! cannot silently amplify personal influence.
//!
//! Candidate-bearing results borrow text and deliberately do not implement
//! `Debug`. This module performs no I/O and is not connected to TSF.

use std::error::Error;
use std::fmt;

use crate::{
    AdaptiveCoverageConfig, AdaptiveCoverageError, AdaptiveCoverageSource, AdaptiveCoverageSummary,
    AdaptiveRankingCandidate, AdaptiveRankingConfig, AdaptiveRankingError, ConfirmedSelectionTier,
    PendingSelectionMemory, rank_visible_candidates, retrieve_personal_coverage,
};

pub const MAX_ADAPTIVE_MERGED_CANDIDATES: usize = 50;
pub const MAX_ADAPTIVE_COVERAGE_PROBABILITY: f64 = 0.20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveMergeConfig {
    pub ranking: AdaptiveRankingConfig,
    pub coverage: AdaptiveCoverageConfig,
    pub max_coverage_probability: f64,
    pub coverage_evidence_saturation: f64,
    pub max_merged_candidates: usize,
}

impl Default for AdaptiveMergeConfig {
    fn default() -> Self {
        Self {
            ranking: AdaptiveRankingConfig::default(),
            coverage: AdaptiveCoverageConfig::default(),
            max_coverage_probability: 0.05,
            coverage_evidence_saturation: 4.0,
            max_merged_candidates: MAX_ADAPTIVE_MERGED_CANDIDATES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveMergedCandidateSource {
    Public {
        original_index: usize,
    },
    PersonalCoverage {
        source: AdaptiveCoverageSource,
        confirmations: u64,
        tier: ConfirmedSelectionTier,
    },
}

/// One candidate in the merged distribution.
///
/// This type intentionally has no `Debug` implementation because it exposes
/// borrowed candidate text through [`Self::text`].
pub struct AdaptiveMergedCandidate<'a> {
    text: &'a str,
    pub source: AdaptiveMergedCandidateSource,
    pub merged_probability: f64,
    pub public_probability_before_coverage: Option<f64>,
    pub weighted_personal_evidence: f64,
}

impl<'a> AdaptiveMergedCandidate<'a> {
    pub fn text(&self) -> &'a str {
        self.text
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AdaptiveMergeSummary {
    pub public_candidates: usize,
    pub personal_coverage_candidates: usize,
    pub full_merged_candidates: usize,
    pub returned_candidates: usize,
    pub returned_public_candidates: usize,
    pub returned_personal_candidates: usize,
    pub truncated_candidates: usize,
    pub existing_candidate_personal_mix: f64,
    pub coverage_probability_mass: f64,
    pub returned_probability_mass: f64,
    pub truncated_probability_mass: f64,
}

/// Merged candidates plus content-free audit summaries.
///
/// This type intentionally has no `Debug` implementation because its
/// candidates borrow text.
pub struct AdaptiveMergeReport<'a> {
    pub summary: AdaptiveMergeSummary,
    pub coverage_summary: AdaptiveCoverageSummary,
    candidates: Vec<AdaptiveMergedCandidate<'a>>,
}

impl<'a> AdaptiveMergeReport<'a> {
    pub fn candidates(&self) -> &[AdaptiveMergedCandidate<'a>] {
        &self.candidates
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveMergeError {
    InvalidConfig,
    Ranking(AdaptiveRankingError),
    Coverage(AdaptiveCoverageError),
    NumericalOverflow,
}

impl fmt::Display for AdaptiveMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("adaptive merge configuration is invalid"),
            Self::Ranking(error) => write!(formatter, "adaptive public ranking failed: {error}"),
            Self::Coverage(error) => write!(formatter, "adaptive coverage failed: {error}"),
            Self::NumericalOverflow => {
                formatter.write_str("adaptive merge calculation exceeded numeric limits")
            }
        }
    }
}

impl Error for AdaptiveMergeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ranking(error) => Some(error),
            Self::Coverage(error) => Some(error),
            Self::InvalidConfig | Self::NumericalOverflow => None,
        }
    }
}

struct MergeEntry<'a> {
    candidate: AdaptiveMergedCandidate<'a>,
    source_rank: usize,
}

pub fn merge_adaptive_candidates<'a>(
    memory: &'a PendingSelectionMemory,
    code: &str,
    public_candidates: &'a [AdaptiveRankingCandidate<'a>],
    config: AdaptiveMergeConfig,
) -> Result<AdaptiveMergeReport<'a>, AdaptiveMergeError> {
    validate_config(config)?;

    let public_ranking = rank_visible_candidates(memory, code, public_candidates, config.ranking)
        .map_err(AdaptiveMergeError::Ranking)?;
    let public_texts = public_candidates
        .iter()
        .map(|candidate| candidate.text)
        .collect::<Vec<_>>();
    let coverage = retrieve_personal_coverage(memory, code, &public_texts, config.coverage)
        .map_err(AdaptiveMergeError::Coverage)?;

    let total_coverage_evidence = coverage
        .candidates()
        .iter()
        .try_fold(0.0, |total, candidate| {
            let next = total + candidate.weighted_evidence;
            next.is_finite().then_some(next)
        })
        .ok_or(AdaptiveMergeError::NumericalOverflow)?;
    let coverage_probability_mass = if total_coverage_evidence == 0.0 {
        0.0
    } else {
        let denominator = total_coverage_evidence + config.coverage_evidence_saturation;
        if !denominator.is_finite() {
            return Err(AdaptiveMergeError::NumericalOverflow);
        }
        config.max_coverage_probability * total_coverage_evidence / denominator
    };
    if !coverage_probability_mass.is_finite() {
        return Err(AdaptiveMergeError::NumericalOverflow);
    }

    let mut entries =
        Vec::with_capacity(public_ranking.candidates.len() + coverage.candidates().len());
    for (source_rank, score) in public_ranking.candidates.iter().enumerate() {
        let merged_probability = (1.0 - coverage_probability_mass) * score.mixed_probability;
        entries.push(MergeEntry {
            candidate: AdaptiveMergedCandidate {
                text: public_candidates[score.original_index].text,
                source: AdaptiveMergedCandidateSource::Public {
                    original_index: score.original_index,
                },
                merged_probability,
                public_probability_before_coverage: Some(score.mixed_probability),
                weighted_personal_evidence: score.weighted_evidence,
            },
            source_rank,
        });
    }
    for (source_rank, candidate) in coverage.candidates().iter().enumerate() {
        let merged_probability = if total_coverage_evidence == 0.0 {
            0.0
        } else {
            coverage_probability_mass * candidate.weighted_evidence / total_coverage_evidence
        };
        entries.push(MergeEntry {
            candidate: AdaptiveMergedCandidate {
                text: candidate.text(),
                source: AdaptiveMergedCandidateSource::PersonalCoverage {
                    source: candidate.source,
                    confirmations: candidate.confirmations,
                    tier: candidate.tier,
                },
                merged_probability,
                public_probability_before_coverage: None,
                weighted_personal_evidence: candidate.weighted_evidence,
            },
            source_rank,
        });
    }
    if entries
        .iter()
        .any(|entry| !entry.candidate.merged_probability.is_finite())
    {
        return Err(AdaptiveMergeError::NumericalOverflow);
    }

    entries.sort_by(|left, right| {
        right
            .candidate
            .merged_probability
            .total_cmp(&left.candidate.merged_probability)
            .then_with(|| source_priority(left).cmp(&source_priority(right)))
            .then_with(|| left.source_rank.cmp(&right.source_rank))
            .then_with(|| left.candidate.text.cmp(right.candidate.text))
    });

    let full_merged_candidates = entries.len();
    let full_probability_mass = entries
        .iter()
        .map(|entry| entry.candidate.merged_probability)
        .sum::<f64>();
    entries.truncate(config.max_merged_candidates);
    let returned_probability_mass = entries
        .iter()
        .map(|entry| entry.candidate.merged_probability)
        .sum::<f64>();
    let returned_public_candidates = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.candidate.source,
                AdaptiveMergedCandidateSource::Public { .. }
            )
        })
        .count();
    let returned_personal_candidates = entries.len() - returned_public_candidates;
    let candidates = entries
        .into_iter()
        .map(|entry| entry.candidate)
        .collect::<Vec<_>>();

    Ok(AdaptiveMergeReport {
        summary: AdaptiveMergeSummary {
            public_candidates: public_ranking.candidates.len(),
            personal_coverage_candidates: coverage.candidates().len(),
            full_merged_candidates,
            returned_candidates: candidates.len(),
            returned_public_candidates,
            returned_personal_candidates,
            truncated_candidates: full_merged_candidates.saturating_sub(candidates.len()),
            existing_candidate_personal_mix: public_ranking.personal_mix,
            coverage_probability_mass,
            returned_probability_mass,
            truncated_probability_mass: full_probability_mass - returned_probability_mass,
        },
        coverage_summary: coverage.summary,
        candidates,
    })
}

fn validate_config(config: AdaptiveMergeConfig) -> Result<(), AdaptiveMergeError> {
    if !config.max_coverage_probability.is_finite()
        || config.max_coverage_probability <= 0.0
        || config.max_coverage_probability > MAX_ADAPTIVE_COVERAGE_PROBABILITY
        || !config.coverage_evidence_saturation.is_finite()
        || config.coverage_evidence_saturation <= 0.0
        || config.max_merged_candidates == 0
        || config.max_merged_candidates > MAX_ADAPTIVE_MERGED_CANDIDATES
    {
        return Err(AdaptiveMergeError::InvalidConfig);
    }
    Ok(())
}

fn source_priority(entry: &MergeEntry<'_>) -> u8 {
    match entry.candidate.source {
        AdaptiveMergedCandidateSource::Public { .. } => 0,
        AdaptiveMergedCandidateSource::PersonalCoverage { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdaptiveMergeConfig, AdaptiveMergeError, AdaptiveMergedCandidateSource,
        MAX_ADAPTIVE_COVERAGE_PROBABILITY, merge_adaptive_candidates,
    };
    use crate::{AdaptiveCoverageSource, AdaptiveRankingCandidate, PendingSelectionMemory};

    fn public<'a>(values: &'a [(&'a str, f64)]) -> Vec<AdaptiveRankingCandidate<'a>> {
        values
            .iter()
            .map(|(text, weight)| AdaptiveRankingCandidate {
                text,
                public_weight: *weight,
            })
            .collect()
    }

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

    fn assert_close(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-12, "{left} != {right}");
    }

    #[test]
    fn no_coverage_evidence_preserves_the_public_distribution() {
        let memory = PendingSelectionMemory::new();
        let public = public(&[("甲", 3.0), ("乙", 1.0)]);

        let report =
            merge_adaptive_candidates(&memory, "aa", &public, AdaptiveMergeConfig::default())
                .unwrap();

        assert_eq!(report.summary.personal_coverage_candidates, 0);
        assert_eq!(report.summary.coverage_probability_mass, 0.0);
        assert_eq!(
            report
                .candidates()
                .iter()
                .map(|candidate| candidate.text())
                .collect::<Vec<_>>(),
            ["甲", "乙"]
        );
        assert_close(report.candidates()[0].merged_probability, 0.75);
        assert_close(report.candidates()[1].merged_probability, 0.25);
    }

    #[test]
    fn eligible_oov_text_is_added_with_explicit_personal_source() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "乙", &mut start, 2);
        let public = public(&[("甲", 1.0)]);
        let config = AdaptiveMergeConfig {
            max_merged_candidates: 2,
            ..AdaptiveMergeConfig::default()
        };

        let report = merge_adaptive_candidates(&memory, "aa", &public, config).unwrap();

        assert_eq!(
            report
                .candidates()
                .iter()
                .map(|candidate| candidate.text())
                .collect::<Vec<_>>(),
            ["甲", "乙"]
        );
        assert_eq!(report.summary.returned_public_candidates, 1);
        assert_eq!(report.summary.returned_personal_candidates, 1);
        assert!(matches!(
            report.candidates()[1].source,
            AdaptiveMergedCandidateSource::PersonalCoverage {
                source: AdaptiveCoverageSource::ConfirmedSelectionHistory,
                confirmations: 2,
                ..
            }
        ));
    }

    #[test]
    fn public_duplicate_is_kept_only_in_the_public_lane() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "甲", &mut start, 2);
        let public = public(&[("甲", 1.0)]);

        let report =
            merge_adaptive_candidates(&memory, "aa", &public, AdaptiveMergeConfig::default())
                .unwrap();

        assert_eq!(report.summary.personal_coverage_candidates, 0);
        assert_eq!(report.coverage_summary.excluded_public, 1);
        assert_eq!(report.candidates().len(), 1);
        assert!(matches!(
            report.candidates()[0].source,
            AdaptiveMergedCandidateSource::Public { original_index: 0 }
        ));
    }

    #[test]
    fn personal_coverage_can_displace_a_weak_public_tail_without_exceeding_its_cap() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "丙", &mut start, 10);
        let public = public(&[("甲", 1.0), ("乙", 0.000_001)]);
        let config = AdaptiveMergeConfig {
            max_coverage_probability: 0.1,
            max_merged_candidates: 2,
            ..AdaptiveMergeConfig::default()
        };

        let report = merge_adaptive_candidates(&memory, "aa", &public, config).unwrap();

        assert_eq!(
            report
                .candidates()
                .iter()
                .map(|candidate| candidate.text())
                .collect::<Vec<_>>(),
            ["甲", "丙"]
        );
        assert!(report.summary.coverage_probability_mass < config.max_coverage_probability);
        assert!(report.summary.coverage_probability_mass <= MAX_ADAPTIVE_COVERAGE_PROBABILITY);
        assert_eq!(report.summary.truncated_candidates, 1);
        assert_eq!(report.summary.returned_public_candidates, 1);
        assert_eq!(report.summary.returned_personal_candidates, 1);
        assert_close(
            report.summary.returned_probability_mass + report.summary.truncated_probability_mass,
            1.0,
        );
    }

    #[test]
    fn exact_code_alias_isolation_survives_the_merge() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "乙", &mut start, 2);
        let public = public(&[("甲", 1.0)]);

        let report =
            merge_adaptive_candidates(&memory, "ab", &public, AdaptiveMergeConfig::default())
                .unwrap();

        assert_eq!(report.summary.personal_coverage_candidates, 0);
        assert_eq!(report.candidates().len(), 1);
    }

    #[test]
    fn repeated_merge_is_deterministic_for_ties_and_sources() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "aa", "丙", &mut start, 2);
        confirm(&mut memory, "aa", "丁", &mut start, 2);
        let public = public(&[("甲", 1.0), ("乙", 1.0)]);

        let first =
            merge_adaptive_candidates(&memory, "aa", &public, AdaptiveMergeConfig::default())
                .unwrap();
        let second =
            merge_adaptive_candidates(&memory, "aa", &public, AdaptiveMergeConfig::default())
                .unwrap();

        assert_eq!(first.summary, second.summary);
        assert_eq!(
            first
                .candidates()
                .iter()
                .map(|candidate| (candidate.text(), candidate.merged_probability))
                .collect::<Vec<_>>(),
            second
                .candidates()
                .iter()
                .map(|candidate| (candidate.text(), candidate.merged_probability))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn invalid_and_extreme_merge_configuration_fails_closed() {
        let memory = PendingSelectionMemory::new();
        let public = public(&[("甲", 1.0)]);
        let too_much = AdaptiveMergeConfig {
            max_coverage_probability: MAX_ADAPTIVE_COVERAGE_PROBABILITY + 0.01,
            ..AdaptiveMergeConfig::default()
        };
        assert_eq!(
            merge_adaptive_candidates(&memory, "aa", &public, too_much).map(|_| ()),
            Err(AdaptiveMergeError::InvalidConfig)
        );

        let mut repeated = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut repeated, "aa", "乙", &mut start, 2);
        let overflowing = AdaptiveMergeConfig {
            coverage_evidence_saturation: f64::MAX,
            coverage: crate::AdaptiveCoverageConfig {
                recent_evidence_weight: f64::MAX / 4.0,
                ..crate::AdaptiveCoverageConfig::default()
            },
            ..AdaptiveMergeConfig::default()
        };
        assert_eq!(
            merge_adaptive_candidates(&repeated, "aa", &public, overflowing).map(|_| ()),
            Err(AdaptiveMergeError::NumericalOverflow)
        );
    }

    #[test]
    fn debuggable_summaries_and_errors_never_contain_text() {
        let mut memory = PendingSelectionMemory::new();
        let mut start = 0;
        confirm(&mut memory, "secretcode", "私密测试文字", &mut start, 2);
        let public = public(&[("公开甲", 1.0)]);

        let report = merge_adaptive_candidates(
            &memory,
            "secretcode",
            &public,
            AdaptiveMergeConfig::default(),
        )
        .unwrap();
        let summary_debug = format!("{:?}", report.summary);
        let error_debug = format!("{:?}", AdaptiveMergeError::InvalidConfig);

        assert!(!summary_debug.contains("私密测试文字"));
        assert!(!summary_debug.contains("公开甲"));
        assert!(!summary_debug.contains("secretcode"));
        assert!(!error_debug.contains("私密测试文字"));
    }
}
