use crate::{
    BIGRAM_INTERPOLATION_WEIGHT, BigramLanguageModel, BigramScore, CandidateSource,
    CharacterBigramLanguageModel, Correction, Decoder, KeySequence, KeySequenceError,
    SentenceCandidate,
};

/// Fixed diagnostic profile for a bounded public hybrid-context sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HybridContextProfile {
    /// Stable report label; never learned from the focused probes.
    pub label: &'static str,
    /// Maximum same-text word segmentations available to context scoring.
    pub max_segmentations_per_text: usize,
    /// Multiplier for mean `ln(1 + observed pair count)`.
    pub word_pair_bonus_weight: f64,
    /// Multiplier for mean public character-bigram log probability.
    pub character_average_weight: f64,
}

/// Small predeclared profile grid for held-out public comparison.
pub const HYBRID_CONTEXT_PROFILES: [HybridContextProfile; 11] = [
    HybridContextProfile {
        label: "word-0.25",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.25,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-0.50",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "char-0.25",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.0,
        character_average_weight: 0.25,
    },
    HybridContextProfile {
        label: "char-0.50",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.0,
        character_average_weight: 0.50,
    },
    HybridContextProfile {
        label: "char-1.00",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.0,
        character_average_weight: 1.00,
    },
    HybridContextProfile {
        label: "char-2.00",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.0,
        character_average_weight: 2.00,
    },
    HybridContextProfile {
        label: "hybrid-0.25-0.25",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.25,
        character_average_weight: 0.25,
    },
    HybridContextProfile {
        label: "hybrid-0.50-0.25",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.25,
    },
    HybridContextProfile {
        label: "hybrid-0.50-0.50",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.50,
    },
    HybridContextProfile {
        label: "hybrid-0.50-1.00",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 1.00,
    },
];

/// Fixed segmentation-retention sweep derived from the bounded-path designs
/// in Chen and Lee (2000), Jia and Zhao (2014), and Rime's small grammar beam.
/// The context weights stay fixed while only the per-text path cap changes.
pub const SEGMENTATION_CONTEXT_PROFILES: [HybridContextProfile; 10] = [
    HybridContextProfile {
        label: "word-0.50-k1",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-0.50-k2",
        max_segmentations_per_text: 2,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-0.50-k3",
        max_segmentations_per_text: 3,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-0.50-k5",
        max_segmentations_per_text: 5,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-0.50-k7",
        max_segmentations_per_text: 7,
        word_pair_bonus_weight: 0.50,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00-k1",
        max_segmentations_per_text: 1,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00-k2",
        max_segmentations_per_text: 2,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00-k3",
        max_segmentations_per_text: 3,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00-k5",
        max_segmentations_per_text: 5,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
    HybridContextProfile {
        label: "word-1.00-k7",
        max_segmentations_per_text: 7,
        word_pair_bonus_weight: 1.00,
        character_average_weight: 0.0,
    },
];

/// Public word-pair evidence used while rescoring one frozen sentence path.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenContextPairEvidence {
    /// Previous lexicon word in the candidate segmentation.
    pub previous: String,
    /// Current lexicon word in the candidate segmentation.
    pub current: String,
    /// Smoothed public-corpus bigram score and its raw counts.
    pub score: BigramScore,
}

/// One unchanged candidate annotated with its baseline and context ranks.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenContextCandidate {
    /// One-based rank in the frozen unigram pool.
    pub baseline_rank: usize,
    /// One-based rank after bounded context reranking.
    pub context_rank: usize,
    /// Whether this path is eligible for context reranking.
    pub eligible: bool,
    /// Interpolated context score when the candidate is eligible.
    pub context_score: Option<f64>,
    /// Public word-pair evidence in segmentation order.
    pub pair_evidence: Vec<FrozenContextPairEvidence>,
    /// Original sentence candidate; no path is created or rewritten.
    pub candidate: SentenceCandidate,
}

/// Read-only reranking result over one already-frozen unigram pool.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenContextRerankReport {
    /// Number of candidates supplied by the unchanged unigram decoder.
    pub pool_depth: usize,
    /// Candidates eligible for word-context reranking.
    pub eligible_candidates: usize,
    /// Candidates in bounded context order.
    pub candidates: Vec<FrozenContextCandidate>,
}

/// One frozen candidate annotated with hybrid public-context features.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenHybridContextCandidate {
    /// One-based rank in the frozen unigram pool.
    pub baseline_rank: usize,
    /// One-based rank after bounded hybrid reranking.
    pub context_rank: usize,
    /// Whether this path is eligible for context reranking.
    pub eligible: bool,
    /// Mean `ln(1 + count)` over adjacent lexicon-word pairs.
    pub word_pair_bonus: Option<f64>,
    /// Mean public character-bigram log probability for the complete text.
    pub character_average_log_probability: Option<f64>,
    /// Final diagnostic score for this profile.
    pub context_score: Option<f64>,
    /// Original sentence candidate; no path is created or rewritten.
    pub candidate: SentenceCandidate,
}

/// Read-only hybrid reranking result over one frozen unigram pool.
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenHybridContextRerankReport {
    /// Profile used for this diagnostic result.
    pub profile: HybridContextProfile,
    /// Number of candidates supplied by the unchanged unigram decoder.
    pub pool_depth: usize,
    /// Candidates eligible for hybrid reranking.
    pub eligible_candidates: usize,
    /// Candidates in bounded hybrid-context order.
    pub candidates: Vec<FrozenHybridContextCandidate>,
}

/// One public or synthetic full-code probe for a frozen-pool context audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenContextProbe {
    /// Stable source identifier used only by the caller.
    pub id: String,
    /// Validated complete double-pinyin input.
    pub observed: KeySequence,
    /// Exact expected output text.
    pub expected_text: String,
}

/// Aggregate metrics for one fixed hybrid-context profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenHybridContextMetrics {
    /// Fixed profile measured by this row.
    pub profile: HybridContextProfile,
    /// Number of probes presented to both orders.
    pub total: usize,
    /// Expected texts present anywhere in the frozen pool.
    pub pool_visible: usize,
    /// Baseline unigram hits at ranks 1, 5, and 10.
    pub baseline_hits_at_1: usize,
    pub baseline_hits_at_5: usize,
    pub baseline_hits_at_10: usize,
    /// Hybrid-context hits at ranks 1, 5, and 10.
    pub context_hits_at_1: usize,
    pub context_hits_at_5: usize,
    pub context_hits_at_10: usize,
    /// Expected text newly entering or leaving rank one.
    pub gained_top_1: usize,
    pub lost_top_1: usize,
    /// Expected text newly entering or leaving the first ten.
    pub recovered_into_top_10: usize,
    pub dropped_out_of_top_10: usize,
    /// Rank direction when the expected text is present in the frozen pool.
    pub improved_ranks: usize,
    pub unchanged_ranks: usize,
    pub worsened_ranks: usize,
}

/// Compares fixed hybrid profiles on exactly the same frozen candidate pools.
pub fn audit_frozen_hybrid_context(
    decoder: &Decoder,
    probes: &[FrozenContextProbe],
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    profiles: &[HybridContextProfile],
    pool_depth: usize,
) -> Result<Vec<FrozenHybridContextMetrics>, KeySequenceError> {
    let pool_depth = pool_depth.max(10);
    let maximum_segmentations_per_text = profiles
        .iter()
        .map(|profile| profile.max_segmentations_per_text)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut metrics = profiles
        .iter()
        .copied()
        .map(|profile| FrozenHybridContextMetrics {
            profile,
            total: probes.len(),
            pool_visible: 0,
            baseline_hits_at_1: 0,
            baseline_hits_at_5: 0,
            baseline_hits_at_10: 0,
            context_hits_at_1: 0,
            context_hits_at_5: 0,
            context_hits_at_10: 0,
            gained_top_1: 0,
            lost_top_1: 0,
            recovered_into_top_10: 0,
            dropped_out_of_top_10: 0,
            improved_ranks: 0,
            unchanged_ranks: 0,
            worsened_ranks: 0,
        })
        .collect::<Vec<_>>();

    for probe in probes {
        let pool = decoder.decode_sentence(probe.observed.as_str(), pool_depth)?;
        let variants = if maximum_segmentations_per_text == 1 {
            pool.clone()
        } else {
            decoder.decode_sentence_segmentation_variants(
                probe.observed.as_str(),
                pool_depth,
                maximum_segmentations_per_text,
            )?
        };
        let baseline_rank = sentence_text_rank(&pool, &probe.expected_text);
        for row in &mut metrics {
            let report = rerank_frozen_sentence_pool_hybrid_with_variants(
                &pool,
                &variants,
                word_language_model,
                character_language_model,
                row.profile,
            );
            let context_rank = report
                .candidates
                .iter()
                .position(|candidate| candidate.candidate.text == probe.expected_text)
                .map(|rank| rank + 1);
            observe_hybrid_metrics(row, baseline_rank, context_rank);
        }
    }
    Ok(metrics)
}

/// Reranks only safe slots in an unchanged sentence-candidate pool.
///
/// A safe slot must be fully covered by lexicon entries, use complete double
/// pinyin, and consume no correction. Ineligible candidates remain at their
/// exact original positions. Context can therefore reorder eligible paths but
/// cannot create a path or move an abbreviation, correction, or unresolved
/// path across the existing safety boundary.
pub fn rerank_frozen_sentence_pool(
    pool: &[SentenceCandidate],
    language_model: &BigramLanguageModel,
) -> FrozenContextRerankReport {
    let eligible_positions = pool
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| candidate_is_context_eligible(candidate).then_some(index))
        .collect::<Vec<_>>();
    let mut eligible_rows = eligible_positions
        .iter()
        .map(|&index| annotate_candidate(index, &pool[index], language_model))
        .collect::<Vec<_>>();
    eligible_rows.sort_by(|left, right| {
        right
            .context_score
            .expect("eligible candidates always have a context score")
            .total_cmp(
                &left
                    .context_score
                    .expect("eligible candidates always have a context score"),
            )
            .then_with(|| left.baseline_rank.cmp(&right.baseline_rank))
    });

    let mut candidates = pool
        .iter()
        .enumerate()
        .map(|(index, candidate)| FrozenContextCandidate {
            baseline_rank: index + 1,
            context_rank: index + 1,
            eligible: false,
            context_score: None,
            pair_evidence: Vec::new(),
            candidate: candidate.clone(),
        })
        .collect::<Vec<_>>();
    for (position, row) in eligible_positions.into_iter().zip(eligible_rows) {
        candidates[position] = row;
    }
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.context_rank = index + 1;
    }

    FrozenContextRerankReport {
        pool_depth: pool.len(),
        eligible_candidates: candidates.iter().filter(|row| row.eligible).count(),
        candidates,
    }
}

/// Reranks safe frozen slots with additive public word and character features.
///
/// This diagnostic deliberately uses fixed profiles rather than fitting on a
/// caller-supplied target. Word evidence is a bounded observed-pair bonus, not
/// another add-alpha conditional probability over the very large lexicon.
/// Character evidence scores the complete output independently of word
/// boundaries, providing a backoff signal when the corpus tokenization and the
/// candidate segmentation disagree.
pub fn rerank_frozen_sentence_pool_hybrid(
    pool: &[SentenceCandidate],
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    profile: HybridContextProfile,
) -> FrozenHybridContextRerankReport {
    rerank_frozen_sentence_pool_hybrid_with_variants(
        pool,
        pool,
        word_language_model,
        character_language_model,
        profile,
    )
}

/// Reranks a frozen unique-text pool while allowing a bounded diagnostic set
/// of alternative word segmentations to supply context evidence for each
/// already-visible text.
///
/// `variants` cannot introduce a new displayed text or a new safety slot. The
/// best context-scored segmentation merely represents its matching baseline
/// text during reranking; final candidates remain unique by text.
pub fn rerank_frozen_sentence_pool_hybrid_with_variants(
    pool: &[SentenceCandidate],
    variants: &[SentenceCandidate],
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    profile: HybridContextProfile,
) -> FrozenHybridContextRerankReport {
    debug_assert!(profile.max_segmentations_per_text > 0);
    debug_assert!(profile.word_pair_bonus_weight.is_finite());
    debug_assert!(profile.word_pair_bonus_weight >= 0.0);
    debug_assert!(profile.character_average_weight.is_finite());
    debug_assert!(profile.character_average_weight >= 0.0);

    let eligible_positions = pool
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| candidate_is_context_eligible(candidate).then_some(index))
        .collect::<Vec<_>>();
    let mut eligible_rows = eligible_positions
        .iter()
        .map(|&index| {
            annotate_best_hybrid_candidate(
                index,
                &pool[index],
                variants,
                word_language_model,
                character_language_model,
                profile,
            )
        })
        .collect::<Vec<_>>();
    eligible_rows.sort_by(|left, right| {
        right
            .context_score
            .expect("eligible candidates always have a hybrid context score")
            .total_cmp(
                &left
                    .context_score
                    .expect("eligible candidates always have a hybrid context score"),
            )
            .then_with(|| left.baseline_rank.cmp(&right.baseline_rank))
    });

    let mut candidates = pool
        .iter()
        .enumerate()
        .map(|(index, candidate)| FrozenHybridContextCandidate {
            baseline_rank: index + 1,
            context_rank: index + 1,
            eligible: false,
            word_pair_bonus: None,
            character_average_log_probability: None,
            context_score: None,
            candidate: candidate.clone(),
        })
        .collect::<Vec<_>>();
    for (position, row) in eligible_positions.into_iter().zip(eligible_rows) {
        candidates[position] = row;
    }
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.context_rank = index + 1;
    }

    FrozenHybridContextRerankReport {
        profile,
        pool_depth: pool.len(),
        eligible_candidates: candidates.iter().filter(|row| row.eligible).count(),
        candidates,
    }
}

fn annotate_best_hybrid_candidate(
    baseline_index: usize,
    baseline: &SentenceCandidate,
    variants: &[SentenceCandidate],
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    profile: HybridContextProfile,
) -> FrozenHybridContextCandidate {
    let mut best = None::<FrozenHybridContextCandidate>;
    for variant in variants
        .iter()
        .filter(|variant| variant.text == baseline.text && candidate_is_context_eligible(variant))
        .take(profile.max_segmentations_per_text.max(1))
    {
        let annotated = annotate_hybrid_candidate(
            baseline_index,
            variant,
            word_language_model,
            character_language_model,
            profile,
        );
        let replace = best.as_ref().is_none_or(|current| {
            annotated
                .context_score
                .expect("eligible variant has a context score")
                .total_cmp(
                    &current
                        .context_score
                        .expect("eligible variant has a context score"),
                )
                .is_gt()
        });
        if replace {
            best = Some(annotated);
        }
    }
    best.unwrap_or_else(|| {
        annotate_hybrid_candidate(
            baseline_index,
            baseline,
            word_language_model,
            character_language_model,
            profile,
        )
    })
}

fn candidate_is_context_eligible(candidate: &SentenceCandidate) -> bool {
    candidate.unresolved_key_count == 0
        && !candidate.used_error
        && !candidate.segments.is_empty()
        && candidate.segments.iter().all(|segment| {
            segment.candidate.source == CandidateSource::Lexicon
                && segment.candidate.spelling.abbreviated_syllables.is_empty()
                && matches!(segment.candidate.correction, Correction::Exact)
        })
}

fn sentence_text_rank(pool: &[SentenceCandidate], expected_text: &str) -> Option<usize> {
    pool.iter()
        .position(|candidate| candidate.text == expected_text)
        .map(|rank| rank + 1)
}

fn observe_hybrid_metrics(
    metrics: &mut FrozenHybridContextMetrics,
    baseline_rank: Option<usize>,
    context_rank: Option<usize>,
) {
    metrics.pool_visible += usize::from(baseline_rank.is_some());
    metrics.baseline_hits_at_1 += usize::from(baseline_rank == Some(1));
    metrics.baseline_hits_at_5 += usize::from(baseline_rank.is_some_and(|rank| rank <= 5));
    metrics.baseline_hits_at_10 += usize::from(baseline_rank.is_some_and(|rank| rank <= 10));
    metrics.context_hits_at_1 += usize::from(context_rank == Some(1));
    metrics.context_hits_at_5 += usize::from(context_rank.is_some_and(|rank| rank <= 5));
    metrics.context_hits_at_10 += usize::from(context_rank.is_some_and(|rank| rank <= 10));
    metrics.gained_top_1 += usize::from(baseline_rank != Some(1) && context_rank == Some(1));
    metrics.lost_top_1 += usize::from(baseline_rank == Some(1) && context_rank != Some(1));
    metrics.recovered_into_top_10 += usize::from(
        baseline_rank.is_none_or(|rank| rank > 10) && context_rank.is_some_and(|rank| rank <= 10),
    );
    metrics.dropped_out_of_top_10 += usize::from(
        baseline_rank.is_some_and(|rank| rank <= 10) && context_rank.is_none_or(|rank| rank > 10),
    );
    if let (Some(baseline_rank), Some(context_rank)) = (baseline_rank, context_rank) {
        match context_rank.cmp(&baseline_rank) {
            std::cmp::Ordering::Less => metrics.improved_ranks += 1,
            std::cmp::Ordering::Equal => metrics.unchanged_ranks += 1,
            std::cmp::Ordering::Greater => metrics.worsened_ranks += 1,
        }
    }
}

fn annotate_candidate(
    baseline_index: usize,
    candidate: &SentenceCandidate,
    language_model: &BigramLanguageModel,
) -> FrozenContextCandidate {
    let mut total_score = 0.0;
    let mut pair_evidence = Vec::with_capacity(candidate.segments.len().saturating_sub(1));
    for (index, segment) in candidate.segments.iter().enumerate() {
        let unigram = segment.language_score.unigram_log_probability;
        let language_score = if index == 0 {
            unigram
        } else {
            let previous = &candidate.segments[index - 1].candidate.text;
            let current = &segment.candidate.text;
            let score = language_model.score(previous, current);
            pair_evidence.push(FrozenContextPairEvidence {
                previous: previous.clone(),
                current: current.clone(),
                score,
            });
            (1.0 - BIGRAM_INTERPOLATION_WEIGHT) * unigram
                + BIGRAM_INTERPOLATION_WEIGHT * score.log_probability
        };
        total_score += language_score
            - segment.candidate.score.abbreviation_penalty
            - segment.candidate.score.correction_penalty
            - segment.candidate.score.unresolved_input_penalty;
    }

    FrozenContextCandidate {
        baseline_rank: baseline_index + 1,
        context_rank: baseline_index + 1,
        eligible: true,
        context_score: Some(total_score),
        pair_evidence,
        candidate: candidate.clone(),
    }
}

fn annotate_hybrid_candidate(
    baseline_index: usize,
    candidate: &SentenceCandidate,
    word_language_model: &BigramLanguageModel,
    character_language_model: &CharacterBigramLanguageModel,
    profile: HybridContextProfile,
) -> FrozenHybridContextCandidate {
    let pair_count = candidate.segments.len().saturating_sub(1);
    let word_pair_bonus = if pair_count == 0 {
        0.0
    } else {
        candidate
            .segments
            .windows(2)
            .map(|segments| {
                let score = word_language_model
                    .score(&segments[0].candidate.text, &segments[1].candidate.text);
                (score.observed_count as f64).ln_1p()
            })
            .sum::<f64>()
            / pair_count as f64
    };
    let character_score = character_language_model.score_text(&candidate.text);
    let character_average_log_probability = if character_score.pair_count == 0 {
        0.0
    } else {
        character_score.log_probability / character_score.pair_count as f64
    };
    let context_score = candidate.total_score
        + profile.word_pair_bonus_weight * word_pair_bonus
        + profile.character_average_weight * character_average_log_probability;

    FrozenHybridContextCandidate {
        baseline_rank: baseline_index + 1,
        context_rank: baseline_index + 1,
        eligible: true,
        word_pair_bonus: Some(word_pair_bonus),
        character_average_log_probability: Some(character_average_log_probability),
        context_score: Some(context_score),
        candidate: candidate.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decoder, parse_lexicon_tsv};

    const LEXICON: &str = "text\tpinyin\tfrequency\n\
有\tyou\t100\n\
其实\tqi shi\t100\n\
尤其\tyou qi\t80\n\
是\tshi\t80\n";
    const CORPUS: &str = "tokens\tcount\n尤其 是\t20\n";

    #[test]
    fn public_word_context_repairs_a_frozen_full_code_segmentation() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let model = BigramLanguageModel::from_tsv(CORPUS, &lexicon).unwrap();
        let pool = decoder.decode_sentence("ybqiui", 10).unwrap();
        assert_eq!(pool[0].text, "有其实");
        assert_eq!(pool[1].text, "尤其是");

        let report = rerank_frozen_sentence_pool(&pool, &model);

        assert_eq!(report.pool_depth, pool.len());
        assert_eq!(report.candidates[0].candidate.text, "尤其是");
        assert_eq!(report.candidates[0].baseline_rank, 2);
        assert_eq!(report.candidates[0].context_rank, 1);
        assert_eq!(report.candidates[0].pair_evidence.len(), 1);
        assert_eq!(
            report.candidates[0].pair_evidence[0].score.observed_count,
            20
        );
        assert_eq!(report.candidates[1].candidate.text, "有其实");
        assert_eq!(report.candidates[1].baseline_rank, 1);
    }

    #[test]
    fn ineligible_candidate_remains_in_its_exact_frozen_slot() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let model = BigramLanguageModel::from_tsv(CORPUS, &lexicon).unwrap();
        let mut pool = decoder.decode_sentence("ybqiui", 10).unwrap();
        let fixed_text = pool[1].text.clone();
        pool[1].used_error = true;

        let report = rerank_frozen_sentence_pool(&pool, &model);

        assert_eq!(report.candidates[1].candidate.text, fixed_text);
        assert!(!report.candidates[1].eligible);
        assert_eq!(report.candidates[1].baseline_rank, 2);
        assert_eq!(report.candidates[1].context_rank, 2);
    }

    #[test]
    fn hybrid_word_bonus_repairs_the_same_frozen_public_example() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let word_model = BigramLanguageModel::from_tsv(CORPUS, &lexicon).unwrap();
        let character_model = CharacterBigramLanguageModel::from_text_sequences(&[
            "尤其是".to_owned(),
            "有其实".to_owned(),
        ])
        .unwrap();
        let pool = decoder.decode_sentence("ybqiui", 10).unwrap();

        let report = rerank_frozen_sentence_pool_hybrid(
            &pool,
            &word_model,
            &character_model,
            HybridContextProfile {
                label: "test",
                max_segmentations_per_text: 1,
                word_pair_bonus_weight: 1.0,
                character_average_weight: 0.0,
            },
        );

        assert_eq!(report.candidates[0].candidate.text, "尤其是");
        assert_eq!(report.candidates[0].baseline_rank, 2);
        assert!(report.candidates[0].word_pair_bonus.unwrap() > 0.0);
        assert_eq!(report.candidates[1].candidate.text, "有其实");
    }

    #[test]
    fn bounded_variants_delay_same_text_segmentation_deduplication() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
不是\tbu shi\t1000\n\
第\tdi\t900\n\
一个\tyi ge\t900\n\
第一\tdi yi\t700\n\
个\tge\t700\n",
        )
        .unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let word_model =
            BigramLanguageModel::from_tsv("tokens\tcount\n不是 第一 个\t20\n", &lexicon).unwrap();
        let character_model =
            CharacterBigramLanguageModel::from_text_sequences(&["不是第一个".to_owned()]).unwrap();
        let pool = decoder.decode_sentence("buuidiyige", 10).unwrap();
        let variants = decoder
            .decode_sentence_segmentation_variants("buuidiyige", 10, 2)
            .unwrap();

        let report = rerank_frozen_sentence_pool_hybrid_with_variants(
            &pool,
            &variants,
            &word_model,
            &character_model,
            HybridContextProfile {
                label: "test-k2",
                max_segmentations_per_text: 2,
                word_pair_bonus_weight: 1.0,
                character_average_weight: 0.0,
            },
        );
        let selected = report
            .candidates
            .iter()
            .find(|candidate| candidate.candidate.text == "不是第一个")
            .unwrap();
        let segmentation = selected
            .candidate
            .segments
            .iter()
            .map(|segment| segment.candidate.text.as_str())
            .collect::<Vec<_>>()
            .join("|");

        assert_eq!(segmentation, "不是|第一|个");
    }

    #[test]
    fn hybrid_audit_uses_one_frozen_pool_for_every_profile() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let decoder = Decoder::new(lexicon.clone());
        let word_model = BigramLanguageModel::from_tsv(CORPUS, &lexicon).unwrap();
        let character_model = CharacterBigramLanguageModel::from_text_sequences(&[
            "尤其是".to_owned(),
            "有其实".to_owned(),
        ])
        .unwrap();
        let probes = [FrozenContextProbe {
            id: "synthetic".to_owned(),
            observed: KeySequence::new("ybqiui").unwrap(),
            expected_text: "尤其是".to_owned(),
        }];
        let profiles = [
            HybridContextProfile {
                label: "baseline-like",
                max_segmentations_per_text: 1,
                word_pair_bonus_weight: 0.0,
                character_average_weight: 0.0,
            },
            HybridContextProfile {
                label: "word",
                max_segmentations_per_text: 1,
                word_pair_bonus_weight: 1.0,
                character_average_weight: 0.0,
            },
        ];

        let metrics = audit_frozen_hybrid_context(
            &decoder,
            &probes,
            &word_model,
            &character_model,
            &profiles,
            10,
        )
        .unwrap();

        assert_eq!(metrics[0].baseline_hits_at_1, 0);
        assert_eq!(metrics[0].context_hits_at_1, 0);
        assert_eq!(metrics[1].baseline_hits_at_1, 0);
        assert_eq!(metrics[1].context_hits_at_1, 1);
        assert_eq!(metrics[1].gained_top_1, 1);
        assert_eq!(metrics[1].lost_top_1, 0);
    }
}
