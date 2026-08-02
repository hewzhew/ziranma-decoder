use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    BigramLanguageModel, Decoder, KeySequenceError, LexiconEntry, MAX_CANDIDATE_SNAPSHOT_RANK,
    PublicProtocolProbe,
};

/// Structural statistics for one fixed-spelling word index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolIndexStats {
    /// Unique Chinese texts indexed after per-code deduplication.
    pub indexed_texts: usize,
    /// Distinct protocol codes.
    pub distinct_codes: usize,
    /// Codes shared by more than one Chinese text.
    pub colliding_codes: usize,
    /// Largest number of Chinese texts sharing one code.
    pub maximum_texts_per_code: usize,
    /// Longest word code in letters.
    pub maximum_code_keys: usize,
}

/// Top-K visibility and input cost for one bounded protocol.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolStrategyReport {
    /// Held-out phrases attempted.
    pub attempts: usize,
    /// Letter keys entered across all attempts.
    pub input_letters: usize,
    /// Explicit mode actions required in addition to letter keys.
    pub activation_actions: usize,
    /// Expected phrase ranked first.
    pub hits_at_1: usize,
    /// Expected phrase ranked in the first five.
    pub hits_at_5: usize,
    /// Expected phrase ranked in the first ten.
    pub hits_at_10: usize,
    /// Expected phrase visible but not first.
    pub visible_nonfirst: usize,
    /// Letter savings contributed only by Top-10-visible attempts.
    pub visible_letter_savings: usize,
}

impl ProtocolStrategyReport {
    /// Top-10 misses whose correction cost is deliberately left unknown.
    pub fn misses_at_10(&self) -> usize {
        self.attempts.saturating_sub(self.hits_at_10)
    }
}

/// Fit-only explicit shortcut lane evaluated with complete-code fallback.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WhitelistProtocolReport {
    /// Held-out phrases attempted.
    pub attempts: usize,
    /// Phrases covered by a repeated, collision-free fit shortcut.
    pub covered: usize,
    /// Phrases that fall back to complete code.
    pub full_code_fallbacks: usize,
    /// Shortcut or fallback letter keys across all attempts.
    pub input_letters: usize,
    /// Explicit lane selections needed for covered shortcuts.
    pub lane_selection_actions: usize,
    /// Letters saved before lane-selection actions.
    pub saved_letters: usize,
}

impl WhitelistProtocolReport {
    /// Net physical actions saved if each shortcut needs one lane selection.
    pub fn net_actions_saved(&self) -> isize {
        self.saved_letters as isize - self.lane_selection_actions as isize
    }
}

/// Read-only comparison of three bounded abbreviation protocols.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicProtocolAuditReport {
    /// Exact complete-code grammar.
    pub full_code: ProtocolStrategyReport,
    /// Per-word complete first syllable followed by one-key suffix syllables.
    pub anchored_tail: ProtocolStrategyReport,
    /// Per-word complete spelling with only the final syllable shortened.
    pub conservative_tail: ProtocolStrategyReport,
    /// Explicit mode in which every key represents one syllable.
    pub explicit_abbreviation: ProtocolStrategyReport,
    /// Fit-only exact shortcut lane with complete-code fallback.
    pub whitelist: WhitelistProtocolReport,
    /// Complete-code index collisions.
    pub full_code_index: ProtocolIndexStats,
    /// Anchored-tail index collisions.
    pub anchored_tail_index: ProtocolIndexStats,
    /// Conservative-tail index collisions.
    pub conservative_tail_index: ProtocolIndexStats,
    /// All-short index collisions after the mode fixes syllable width.
    pub explicit_abbreviation_index: ProtocolIndexStats,
}

/// One fixed gate for shortening only the final double-pinyin syllable of a
/// multi-syllable word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollisionGatedTailProfile {
    /// Stable report label.
    pub label: &'static str,
    /// Largest raw shortened-code fanout allowed; `None` keeps complete code.
    pub maximum_shortened_code_fanout: Option<usize>,
}

/// Fixed structural sweep; thresholds are powers of two rather than values
/// fitted on a held-out answer set.
pub const COLLISION_GATED_TAIL_PROFILES: [CollisionGatedTailProfile; 7] = [
    CollisionGatedTailProfile {
        label: "full",
        maximum_shortened_code_fanout: None,
    },
    CollisionGatedTailProfile {
        label: "fanout-1",
        maximum_shortened_code_fanout: Some(1),
    },
    CollisionGatedTailProfile {
        label: "fanout-2",
        maximum_shortened_code_fanout: Some(2),
    },
    CollisionGatedTailProfile {
        label: "fanout-4",
        maximum_shortened_code_fanout: Some(4),
    },
    CollisionGatedTailProfile {
        label: "fanout-8",
        maximum_shortened_code_fanout: Some(8),
    },
    CollisionGatedTailProfile {
        label: "fanout-16",
        maximum_shortened_code_fanout: Some(16),
    },
    CollisionGatedTailProfile {
        label: "all",
        maximum_shortened_code_fanout: Some(usize::MAX),
    },
];

/// Paired accuracy, action cost, and candidate-load evidence for one gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollisionGatedTailReport {
    /// Fixed gate represented by this row.
    pub profile: CollisionGatedTailProfile,
    /// Final mixed full/short word-index structure.
    pub index: ProtocolIndexStats,
    /// Ordinary Top-K and letter-cost accounting.
    pub strategy: ProtocolStrategyReport,
    /// Multi-syllable expected word instances eligible to save one key.
    pub multisyllable_word_instances: usize,
    /// Expected word instances actually shortened by this gate.
    pub shortened_word_instances: usize,
    /// Phrases containing at least one shortened expected word.
    pub attempts_with_shortening: usize,
    /// Largest raw shortened-code fanout among actually shortened words.
    pub maximum_observed_shortened_fanout: usize,
    /// Final candidate pools containing more than one text.
    pub ambiguous_candidate_pools: usize,
    /// Final candidate pools extending beyond the ten visible positions.
    pub candidate_pools_over_ten: usize,
    /// Expected text newly reaching or leaving first place versus full code.
    pub gained_top_1: usize,
    pub lost_top_1: usize,
    /// Expected text newly entering or leaving the first ten versus full code.
    pub recovered_into_top_10: usize,
    pub dropped_out_of_top_10: usize,
}

/// Target visibility at either the odd initial-key state or the following
/// even complete-syllable state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DoublePinyinTrajectoryLaneReport {
    /// Prefix states observed in this parity lane.
    pub steps: usize,
    /// Target prefix ranked first, within five, or within ten.
    pub hits_at_1: usize,
    pub hits_at_5: usize,
    pub hits_at_10: usize,
    /// Visible candidate texts accumulated across the lane.
    pub visible_candidates: usize,
}

/// Aggregate interaction evidence for each initial-key/rhyme-key pair in
/// complete double-pinyin input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DoublePinyinTrajectoryReport {
    /// Public probes supplied to the audit.
    pub requested_probes: usize,
    /// Probes whose Han-character count matches the complete-code syllables.
    pub aligned_probes: usize,
    /// Probes excluded because a character-level target prefix is undefined.
    pub alignment_mismatches: usize,
    /// Initial-key/rhyme-key resolution pairs observed.
    pub syllable_resolutions: usize,
    /// Odd initial-key states.
    pub odd: DoublePinyinTrajectoryLaneReport,
    /// Even complete-syllable states.
    pub even: DoublePinyinTrajectoryLaneReport,
    /// Target prefix visible at both sides of the rhyme key.
    pub target_visible_both: usize,
    /// Target prefix lost or gained when the rhyme key arrives.
    pub target_lost_on_rhyme: usize,
    pub target_gained_on_rhyme: usize,
    /// Target prefix absent from both visible lists.
    pub target_absent_both: usize,
    /// Rank movement among pairs where the target is visible on both sides.
    pub target_rank_improved: usize,
    pub target_rank_unchanged: usize,
    pub target_rank_worsened: usize,
    /// Whether the visible first candidate text survives the rhyme key.
    pub top_1_unchanged: usize,
    pub top_1_changed: usize,
    /// First-candidate changes that land on or leave the target prefix.
    pub top_1_changed_to_target: usize,
    pub top_1_changed_from_target: usize,
    /// Exact odd-state texts retained anywhere in the following even list.
    pub retained_visible_texts: usize,
    /// Odd-state visible texts forming the retention denominator.
    pub odd_visible_texts: usize,
    /// Candidate decode requests before and after public-prefix memoization.
    pub decode_requests: usize,
    pub unique_prefixes: usize,
    pub prefix_cache_hits: usize,
}

/// One held-out anchored-tail miss with deeper and boundary-hint evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchoredTailFailureCase {
    /// Stable public probe identifier.
    pub id: String,
    /// Public anchored-tail key sequence.
    pub observed: String,
    /// Expected public phrase.
    pub expected_text: String,
    /// Ordinary first candidate under the strict anchored-tail grammar.
    pub baseline_top_text: String,
    /// Word segmentation used by the ordinary first candidate.
    pub baseline_top_segments: Vec<String>,
    /// One-based rank in the deeper unsegmented pool.
    pub deeper_rank: Option<usize>,
    /// One-based rank after supplying the expected public word boundary.
    pub boundary_rank: Option<usize>,
    /// Number of word texts sharing each expected word's anchored-tail code.
    pub expected_word_code_fanouts: Vec<usize>,
}

/// Read-only classification of strict anchored-tail Top-K failures.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnchoredTailFailureAuditReport {
    /// Held-out public phrases examined.
    pub total: usize,
    /// Ordinary visible-list depth.
    pub visible_k: usize,
    /// Deeper diagnostic pool depth.
    pub audit_depth: usize,
    /// Expected phrases already visible without a boundary hint.
    pub baseline_visible: usize,
    /// Baseline misses found later in the unsegmented pool.
    pub deeper_visible: usize,
    /// Baseline misses still absent at `audit_depth`.
    pub outside_audit_depth: usize,
    /// Baseline misses recovered into the visible list by one source boundary.
    pub boundary_recovered_visible: usize,
    /// Baseline misses ranked first after one source boundary.
    pub boundary_recovered_at_1: usize,
    /// Baseline misses found only deeper after one source boundary.
    pub boundary_deeper_visible: usize,
    /// Baseline misses still absent at depth even with the source boundary.
    pub boundary_outside_audit_depth: usize,
    /// Failure Top-1 texts shorter than the expected text.
    pub baseline_top_shorter: usize,
    /// Failure Top-1 texts with the same character length as expectation.
    pub baseline_top_same_length: usize,
    /// Failure Top-1 texts longer than the expected text.
    pub baseline_top_longer: usize,
    /// Failures where at least one expected word code has multiple texts.
    pub failures_with_word_code_collision: usize,
    /// Largest expected-word code fanout among failures.
    pub maximum_expected_word_code_fanout: usize,
    /// Letter savings retained by boundary-recovered failures after one marker.
    pub recovered_net_actions_saved: isize,
    /// Per-case public evidence for every ordinary visible-list miss.
    pub failures: Vec<AnchoredTailFailureCase>,
}

/// Baseline-versus-context ranking for one frozen protocol candidate pool.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolContextLaneReport {
    /// Held-out phrases attempted.
    pub total: usize,
    /// Expected phrases present in the frozen pool.
    pub pool_visible: usize,
    /// Baseline unigram Top-1 matches.
    pub baseline_hits_at_1: usize,
    /// Baseline unigram Top-5 matches.
    pub baseline_hits_at_5: usize,
    /// Baseline unigram Top-10 matches.
    pub baseline_hits_at_10: usize,
    /// Fit-only context Top-1 matches.
    pub context_hits_at_1: usize,
    /// Fit-only context Top-5 matches.
    pub context_hits_at_5: usize,
    /// Fit-only context Top-10 matches.
    pub context_hits_at_10: usize,
    /// Baseline Top-10 misses recovered by context.
    pub repaired_into_top_10: usize,
    /// Baseline Top-10 hits pushed out by context.
    pub dropped_out_of_top_10: usize,
    /// Visible expected paths whose rank improved.
    pub improved_ranks: usize,
    /// Visible expected paths whose rank stayed equal.
    pub unchanged_ranks: usize,
    /// Visible expected paths whose rank worsened.
    pub worsened_ranks: usize,
}

/// Fit-only word-context comparison for full and anchored-tail protocols.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PublicProtocolContextAuditReport {
    /// Frozen-pool result for exact complete code.
    pub full_code: ProtocolContextLaneReport,
    /// Frozen-pool result for strict anchored-tail code.
    pub anchored_tail: ProtocolContextLaneReport,
    /// Number of candidates frozen per held-out input.
    pub pool_depth: usize,
}

/// Compares fixed-spelling protocols without changing the production decoder.
///
/// Each word has exactly one spelling inside each index. Word boundaries remain
/// latent and are ranked by the same normalized unigram form for every lane.
/// The all-short lane pays one explicit mode action per phrase. The whitelist
/// uses only fit-derived availability already frozen into each probe.
pub fn audit_public_protocols(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
) -> PublicProtocolAuditReport {
    let full_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::Full);
    let anchored_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::AnchoredTail);
    let conservative_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::ConservativeTail);
    let explicit_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::AllShort);
    let mut report = PublicProtocolAuditReport {
        full_code_index: full_index.stats,
        anchored_tail_index: anchored_index.stats,
        conservative_tail_index: conservative_index.stats,
        explicit_abbreviation_index: explicit_index.stats,
        ..PublicProtocolAuditReport::default()
    };

    for probe in probes {
        let full_keys = probe.full_observed.as_str().len();
        observe_strategy(
            &mut report.full_code,
            full_index.decode(probe.full_observed.as_str(), 10),
            &probe.expected_text,
            full_keys,
            full_keys,
            0,
        );
        let anchored_keys = probe.anchored_tail_observed.as_str().len();
        observe_strategy(
            &mut report.anchored_tail,
            anchored_index.decode(probe.anchored_tail_observed.as_str(), 10),
            &probe.expected_text,
            full_keys,
            anchored_keys,
            0,
        );
        let conservative_keys = probe.conservative_tail_observed.as_str().len();
        observe_strategy(
            &mut report.conservative_tail,
            conservative_index.decode(probe.conservative_tail_observed.as_str(), 10),
            &probe.expected_text,
            full_keys,
            conservative_keys,
            0,
        );
        let explicit_keys = probe.explicit_abbreviation_observed.as_str().len();
        observe_strategy(
            &mut report.explicit_abbreviation,
            explicit_index.decode(probe.explicit_abbreviation_observed.as_str(), 10),
            &probe.expected_text,
            full_keys,
            explicit_keys,
            1,
        );

        report.whitelist.attempts += 1;
        if probe.whitelist_available {
            report.whitelist.covered += 1;
            report.whitelist.input_letters += explicit_keys;
            report.whitelist.lane_selection_actions += 1;
            report.whitelist.saved_letters += full_keys - explicit_keys;
        } else {
            report.whitelist.full_code_fallbacks += 1;
            report.whitelist.input_letters += full_keys;
        }
    }

    report
}

/// Audits a double-pinyin-specific policy that shortens one final-syllable key
/// only when the raw shortened word code stays below a fixed collision gate.
///
/// The gate reads only public lexicon structure. It does not inspect expected
/// ranks, fit a threshold, or alter the production decoder. Every row is
/// paired against the same complete-code result for each probe.
pub fn audit_collision_gated_tail_protocols(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
    profiles: &[CollisionGatedTailProfile],
) -> Vec<CollisionGatedTailReport> {
    audit_collision_gated_tail_protocols_at(
        lexicon,
        probes,
        profiles,
        CollisionGatePlacement::EveryWord,
    )
}

/// Audits the same collision gates when the omitted rhyme key is allowed only
/// at the end of the complete composition. Full double-pinyin words remain
/// the only legal interior transitions, preserving pair synchronization.
pub fn audit_terminal_collision_gated_tail_protocols(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
    profiles: &[CollisionGatedTailProfile],
) -> Vec<CollisionGatedTailReport> {
    audit_collision_gated_tail_protocols_at(
        lexicon,
        probes,
        profiles,
        CollisionGatePlacement::CompositionFinalWord,
    )
}

/// Audits the real interactive candidate ordering after each initial key and
/// its completing rhyme key in public complete-code probes.
///
/// The expected prefix grows by one Han character per double-pinyin syllable.
/// Probes whose character and syllable counts differ are reported and skipped
/// rather than assigned a guessed alignment. Repeated public prefixes share a
/// bounded in-memory cache for this batch only.
pub fn audit_double_pinyin_key_trajectories(
    decoder: &Decoder,
    probes: &[PublicProtocolProbe],
    visible_limit: usize,
) -> Result<DoublePinyinTrajectoryReport, KeySequenceError> {
    let visible_limit = visible_limit.clamp(1, MAX_CANDIDATE_SNAPSHOT_RANK);
    let mut report = DoublePinyinTrajectoryReport {
        requested_probes: probes.len(),
        ..DoublePinyinTrajectoryReport::default()
    };
    let mut prefix_cache = HashMap::<String, Vec<String>>::new();

    for probe in probes {
        let code = probe.full_observed.as_str();
        let expected_chars = probe.expected_text.chars().collect::<Vec<_>>();
        if code.len() != expected_chars.len().saturating_mul(2) {
            report.alignment_mismatches += 1;
            continue;
        }
        report.aligned_probes += 1;

        for syllable_index in 0..expected_chars.len() {
            let odd_end = syllable_index * 2 + 1;
            let even_end = odd_end + 1;
            let target = expected_chars[..=syllable_index].iter().collect::<String>();
            let odd_candidates = cached_interactive_candidates(
                decoder,
                &mut prefix_cache,
                &code[..odd_end],
                visible_limit,
                &mut report,
            )?;
            let even_candidates = cached_interactive_candidates(
                decoder,
                &mut prefix_cache,
                &code[..even_end],
                visible_limit,
                &mut report,
            )?;
            let odd_rank = observe_trajectory_lane(&mut report.odd, &odd_candidates, &target);
            let even_rank = observe_trajectory_lane(&mut report.even, &even_candidates, &target);

            report.syllable_resolutions += 1;
            match (odd_rank, even_rank) {
                (Some(odd), Some(even)) => {
                    report.target_visible_both += 1;
                    match even.cmp(&odd) {
                        Ordering::Less => report.target_rank_improved += 1,
                        Ordering::Equal => report.target_rank_unchanged += 1,
                        Ordering::Greater => report.target_rank_worsened += 1,
                    }
                }
                (Some(_), None) => report.target_lost_on_rhyme += 1,
                (None, Some(_)) => report.target_gained_on_rhyme += 1,
                (None, None) => report.target_absent_both += 1,
            }

            let odd_top = odd_candidates.first().map(String::as_str);
            let even_top = even_candidates.first().map(String::as_str);
            if odd_top == even_top {
                report.top_1_unchanged += 1;
            } else {
                report.top_1_changed += 1;
                report.top_1_changed_to_target += usize::from(even_top == Some(target.as_str()));
                report.top_1_changed_from_target += usize::from(odd_top == Some(target.as_str()));
            }

            let even_texts = even_candidates
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            report.retained_visible_texts += odd_candidates
                .iter()
                .filter(|text| even_texts.contains(text.as_str()))
                .count();
            report.odd_visible_texts += odd_candidates.len();
        }
    }
    report.unique_prefixes = prefix_cache.len();
    debug_assert_eq!(report.odd.steps, report.syllable_resolutions);
    debug_assert_eq!(report.even.steps, report.syllable_resolutions);
    debug_assert_eq!(
        report.target_visible_both
            + report.target_lost_on_rhyme
            + report.target_gained_on_rhyme
            + report.target_absent_both,
        report.syllable_resolutions
    );
    debug_assert_eq!(
        report.target_rank_improved + report.target_rank_unchanged + report.target_rank_worsened,
        report.target_visible_both
    );
    debug_assert_eq!(
        report.top_1_unchanged + report.top_1_changed,
        report.syllable_resolutions
    );
    debug_assert_eq!(
        report.decode_requests,
        report.unique_prefixes + report.prefix_cache_hits
    );
    Ok(report)
}

fn cached_interactive_candidates(
    decoder: &Decoder,
    cache: &mut HashMap<String, Vec<String>>,
    prefix: &str,
    visible_limit: usize,
    report: &mut DoublePinyinTrajectoryReport,
) -> Result<Vec<String>, KeySequenceError> {
    report.decode_requests += 1;
    if let Some(candidates) = cache.get(prefix) {
        report.prefix_cache_hits += 1;
        return Ok(candidates.clone());
    }
    let candidates = decoder.interactive_candidate_texts(prefix, visible_limit)?;
    cache.insert(prefix.to_owned(), candidates.clone());
    Ok(candidates)
}

fn observe_trajectory_lane(
    lane: &mut DoublePinyinTrajectoryLaneReport,
    candidates: &[String],
    target: &str,
) -> Option<usize> {
    lane.steps += 1;
    lane.visible_candidates += candidates.len();
    let rank = candidates
        .iter()
        .position(|candidate| candidate == target)
        .map(|index| index + 1);
    lane.hits_at_1 += usize::from(rank == Some(1));
    lane.hits_at_5 += usize::from(rank.is_some_and(|rank| rank <= 5));
    lane.hits_at_10 += usize::from(rank.is_some_and(|rank| rank <= 10));
    rank
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CollisionGatePlacement {
    EveryWord,
    CompositionFinalWord,
}

fn audit_collision_gated_tail_protocols_at(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
    profiles: &[CollisionGatedTailProfile],
    placement: CollisionGatePlacement,
) -> Vec<CollisionGatedTailReport> {
    const VISIBLE_K: usize = 10;
    const LOAD_AUDIT_K: usize = VISIBLE_K + 1;

    let full_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::Full);
    let conservative_fanouts = protocol_code_fanouts(lexicon, ProtocolCodeMode::ConservativeTail);
    let entries_by_text = best_entries_by_text(lexicon);

    profiles
        .iter()
        .copied()
        .map(|profile| {
            let index = ProtocolIndex::new_with_code(lexicon, |entry| {
                collision_gated_tail_code(entry, &conservative_fanouts, profile).0
            });
            let mut report = CollisionGatedTailReport {
                profile,
                index: index.stats,
                strategy: ProtocolStrategyReport::default(),
                multisyllable_word_instances: 0,
                shortened_word_instances: 0,
                attempts_with_shortening: 0,
                maximum_observed_shortened_fanout: 0,
                ambiguous_candidate_pools: 0,
                candidate_pools_over_ten: 0,
                gained_top_1: 0,
                lost_top_1: 0,
                recovered_into_top_10: 0,
                dropped_out_of_top_10: 0,
            };

            for probe in probes {
                let full_candidates = full_index.decode(probe.full_observed.as_str(), LOAD_AUDIT_K);
                let full_rank = text_rank(&full_candidates, &probe.expected_text);
                let mut observed = String::new();
                let mut shortened_words = 0;
                let mut multisyllable_words = 0;
                let mut maximum_shortened_fanout = 0;
                let mut complete_probe_mapping = true;
                for (segment_index, segment) in probe.expected_segments.iter().enumerate() {
                    let Some(entry) = entries_by_text.get(segment.as_str()).copied() else {
                        complete_probe_mapping = false;
                        break;
                    };
                    let gate_this_word = placement == CollisionGatePlacement::EveryWord
                        || segment_index + 1 == probe.expected_segments.len();
                    multisyllable_words +=
                        usize::from(gate_this_word && entry.syllable_codes.len() >= 2);
                    let (code, shortened, fanout) = if gate_this_word {
                        collision_gated_tail_code(entry, &conservative_fanouts, profile)
                    } else {
                        (entry.code.as_str().to_owned(), false, 0)
                    };
                    observed.push_str(&code);
                    shortened_words += usize::from(shortened);
                    if shortened {
                        maximum_shortened_fanout = maximum_shortened_fanout.max(fanout);
                    }
                }
                if !complete_probe_mapping {
                    observed = probe.full_observed.as_str().to_owned();
                    shortened_words = 0;
                    multisyllable_words = 0;
                    maximum_shortened_fanout = 0;
                }

                let candidates = match placement {
                    CollisionGatePlacement::EveryWord => index.decode(&observed, LOAD_AUDIT_K),
                    CollisionGatePlacement::CompositionFinalWord => {
                        full_index.decode_with_terminal_shortcuts(&index, &observed, LOAD_AUDIT_K)
                    }
                };
                let rank = text_rank(&candidates, &probe.expected_text);
                let candidate_count = candidates.len();
                observe_strategy(
                    &mut report.strategy,
                    candidates,
                    &probe.expected_text,
                    probe.full_observed.as_str().len(),
                    observed.len(),
                    0,
                );
                report.multisyllable_word_instances += multisyllable_words;
                report.shortened_word_instances += shortened_words;
                report.attempts_with_shortening += usize::from(shortened_words > 0);
                report.maximum_observed_shortened_fanout = report
                    .maximum_observed_shortened_fanout
                    .max(maximum_shortened_fanout);
                report.ambiguous_candidate_pools += usize::from(candidate_count > 1);
                report.candidate_pools_over_ten += usize::from(candidate_count > VISIBLE_K);
                report.gained_top_1 += usize::from(full_rank != Some(1) && rank == Some(1));
                report.lost_top_1 += usize::from(full_rank == Some(1) && rank != Some(1));
                report.recovered_into_top_10 += usize::from(
                    full_rank.is_none_or(|value| value > VISIBLE_K)
                        && rank.is_some_and(|value| value <= VISIBLE_K),
                );
                report.dropped_out_of_top_10 += usize::from(
                    full_rank.is_some_and(|value| value <= VISIBLE_K)
                        && rank.is_none_or(|value| value > VISIBLE_K),
                );
            }
            report
        })
        .collect()
}

/// Classifies strict anchored-tail misses without changing production ranking.
///
/// Every ordinary miss receives a deeper unsegmented pool and a counterfactual
/// pool with the source's one public word boundary supplied. The boundary is a
/// diagnostic interaction cost, not a learned feature or production behavior.
pub fn audit_anchored_tail_failures(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
    visible_k: usize,
    audit_depth: usize,
) -> AnchoredTailFailureAuditReport {
    let visible_k = visible_k.max(1);
    let audit_depth = audit_depth.max(visible_k);
    let index = ProtocolIndex::new(lexicon, ProtocolCodeMode::AnchoredTail);
    let best_entries = best_entries_by_text(lexicon);
    let mut report = AnchoredTailFailureAuditReport {
        total: probes.len(),
        visible_k,
        audit_depth,
        ..AnchoredTailFailureAuditReport::default()
    };

    for probe in probes {
        let baseline = index.decode(probe.anchored_tail_observed.as_str(), visible_k);
        if text_rank(&baseline, &probe.expected_text).is_some() {
            report.baseline_visible += 1;
            continue;
        }

        let top = baseline
            .first()
            .expect("an exact public expected path guarantees a candidate");
        match top
            .text
            .chars()
            .count()
            .cmp(&probe.expected_text.chars().count())
        {
            Ordering::Less => report.baseline_top_shorter += 1,
            Ordering::Equal => report.baseline_top_same_length += 1,
            Ordering::Greater => report.baseline_top_longer += 1,
        }

        let deeper = index.decode(probe.anchored_tail_observed.as_str(), audit_depth);
        let deeper_rank = text_rank(&deeper, &probe.expected_text);
        report.deeper_visible += usize::from(deeper_rank.is_some());
        report.outside_audit_depth += usize::from(deeper_rank.is_none());

        let segment_codes = probe
            .expected_segments
            .iter()
            .map(|text| {
                let entry = best_entries
                    .get(text.as_str())
                    .expect("public protocol probes retain exact Rime words");
                protocol_code(entry, ProtocolCodeMode::AnchoredTail)
            })
            .collect::<Vec<_>>();
        let code_slices = segment_codes.iter().map(String::as_str).collect::<Vec<_>>();
        debug_assert_eq!(
            code_slices.concat(),
            probe.anchored_tail_observed.as_str(),
            "the public source boundary must reconstruct the observed code"
        );
        let boundary = index.decode_fixed_segments(&code_slices, audit_depth);
        let boundary_rank = text_rank(&boundary, &probe.expected_text);
        report.boundary_recovered_visible +=
            usize::from(boundary_rank.is_some_and(|rank| rank <= visible_k));
        report.boundary_recovered_at_1 += usize::from(boundary_rank == Some(1));
        report.boundary_deeper_visible +=
            usize::from(boundary_rank.is_some_and(|rank| rank > visible_k));
        report.boundary_outside_audit_depth += usize::from(boundary_rank.is_none());

        let expected_word_code_fanouts = code_slices
            .iter()
            .map(|code| index.code_fanout(code))
            .collect::<Vec<_>>();
        report.failures_with_word_code_collision +=
            usize::from(expected_word_code_fanouts.iter().any(|fanout| *fanout > 1));
        report.maximum_expected_word_code_fanout = report.maximum_expected_word_code_fanout.max(
            expected_word_code_fanouts
                .iter()
                .copied()
                .max()
                .unwrap_or(0),
        );

        if boundary_rank.is_some_and(|rank| rank <= visible_k) {
            let saved_letters =
                probe.full_observed.as_str().len() - probe.anchored_tail_observed.as_str().len();
            let boundary_markers = probe.expected_segments.len().saturating_sub(1);
            report.recovered_net_actions_saved +=
                saved_letters as isize - boundary_markers as isize;
        }
        report.failures.push(AnchoredTailFailureCase {
            id: probe.id.clone(),
            observed: probe.anchored_tail_observed.as_str().to_owned(),
            expected_text: probe.expected_text.clone(),
            baseline_top_text: top.text.clone(),
            baseline_top_segments: top.segments.clone(),
            deeper_rank,
            boundary_rank,
            expected_word_code_fanouts,
        });
    }

    report
}

/// Reranks frozen strict-protocol pools with a fit-only public word bigram.
///
/// Context cannot create paths: each pool is first frozen under the unchanged
/// unigram order. Complete code is reported beside anchored-tail input so a
/// shortcut gain cannot hide a clean-input regression.
pub fn audit_public_protocol_context(
    lexicon: &[LexiconEntry],
    probes: &[PublicProtocolProbe],
    language_model: &BigramLanguageModel,
    pool_depth: usize,
) -> PublicProtocolContextAuditReport {
    let pool_depth = pool_depth.max(10);
    let full_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::Full);
    let anchored_index = ProtocolIndex::new(lexicon, ProtocolCodeMode::AnchoredTail);
    let mut report = PublicProtocolContextAuditReport {
        pool_depth,
        ..PublicProtocolContextAuditReport::default()
    };
    for probe in probes {
        let full_pool = full_index.decode(probe.full_observed.as_str(), pool_depth);
        observe_context_lane(
            &mut report.full_code,
            &full_pool,
            &probe.expected_text,
            language_model,
        );
        let anchored_pool =
            anchored_index.decode(probe.anchored_tail_observed.as_str(), pool_depth);
        observe_context_lane(
            &mut report.anchored_tail,
            &anchored_pool,
            &probe.expected_text,
            language_model,
        );
    }
    report
}

fn observe_context_lane(
    report: &mut ProtocolContextLaneReport,
    pool: &[ProtocolPath],
    expected_text: &str,
    language_model: &BigramLanguageModel,
) {
    let baseline_rank = text_rank(pool, expected_text);
    let context_rank = context_text_rank(pool, expected_text, language_model);
    report.total += 1;
    report.pool_visible += usize::from(baseline_rank.is_some());
    report.baseline_hits_at_1 += usize::from(baseline_rank == Some(1));
    report.baseline_hits_at_5 += usize::from(baseline_rank.is_some_and(|rank| rank <= 5));
    report.baseline_hits_at_10 += usize::from(baseline_rank.is_some_and(|rank| rank <= 10));
    report.context_hits_at_1 += usize::from(context_rank == Some(1));
    report.context_hits_at_5 += usize::from(context_rank.is_some_and(|rank| rank <= 5));
    report.context_hits_at_10 += usize::from(context_rank.is_some_and(|rank| rank <= 10));
    report.repaired_into_top_10 += usize::from(
        baseline_rank.is_none_or(|rank| rank > 10) && context_rank.is_some_and(|rank| rank <= 10),
    );
    report.dropped_out_of_top_10 += usize::from(
        baseline_rank.is_some_and(|rank| rank <= 10) && context_rank.is_none_or(|rank| rank > 10),
    );
    if let (Some(baseline_rank), Some(context_rank)) = (baseline_rank, context_rank) {
        match context_rank.cmp(&baseline_rank) {
            Ordering::Less => report.improved_ranks += 1,
            Ordering::Equal => report.unchanged_ranks += 1,
            Ordering::Greater => report.worsened_ranks += 1,
        }
    }
}

fn context_text_rank(
    pool: &[ProtocolPath],
    expected_text: &str,
    language_model: &BigramLanguageModel,
) -> Option<usize> {
    let mut reranked = pool
        .iter()
        .enumerate()
        .map(|(baseline_rank, path)| {
            (
                baseline_rank,
                path,
                protocol_context_score(path, language_model),
            )
        })
        .collect::<Vec<_>>();
    reranked.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.0.cmp(&right.0))
    });
    reranked
        .iter()
        .position(|(_, path, _)| path.text == expected_text)
        .map(|rank| rank + 1)
}

fn protocol_context_score(path: &ProtocolPath, language_model: &BigramLanguageModel) -> f64 {
    let mut score = 0.0;
    for (index, (word, unigram)) in path
        .segments
        .iter()
        .zip(&path.segment_log_probabilities)
        .enumerate()
    {
        if index == 0 {
            score += unigram;
        } else {
            let previous = &path.segments[index - 1];
            let bigram = language_model.score(previous, word).log_probability;
            score += (1.0 - crate::BIGRAM_INTERPOLATION_WEIGHT) * unigram
                + crate::BIGRAM_INTERPOLATION_WEIGHT * bigram;
        }
    }
    score
}

fn observe_strategy(
    report: &mut ProtocolStrategyReport,
    candidates: Vec<ProtocolPath>,
    expected_text: &str,
    full_keys: usize,
    input_keys: usize,
    activation_actions: usize,
) {
    let rank = candidates
        .iter()
        .position(|candidate| candidate.text == expected_text);
    report.attempts += 1;
    report.input_letters += input_keys;
    report.activation_actions += activation_actions;
    report.hits_at_1 += usize::from(rank == Some(0));
    report.hits_at_5 += usize::from(rank.is_some_and(|rank| rank < 5));
    report.hits_at_10 += usize::from(rank.is_some_and(|rank| rank < 10));
    report.visible_nonfirst += usize::from(rank.is_some_and(|rank| (1..10).contains(&rank)));
    if rank.is_some_and(|rank| rank < 10) {
        report.visible_letter_savings += full_keys.saturating_sub(input_keys);
    }
}

#[derive(Clone, Copy)]
enum ProtocolCodeMode {
    Full,
    AnchoredTail,
    ConservativeTail,
    AllShort,
}

#[derive(Clone, Debug)]
struct ProtocolWord {
    text: String,
    log_probability: f64,
}

#[derive(Clone, Debug)]
struct ProtocolPath {
    text: String,
    score: f64,
    segments: Vec<String>,
    segment_log_probabilities: Vec<f64>,
}

struct ProtocolIndex {
    words_by_code: BTreeMap<String, Vec<ProtocolWord>>,
    stats: ProtocolIndexStats,
}

impl ProtocolIndex {
    fn new(lexicon: &[LexiconEntry], mode: ProtocolCodeMode) -> Self {
        Self::new_with_code(lexicon, |entry| protocol_code(entry, mode))
    }

    fn new_with_code(
        lexicon: &[LexiconEntry],
        code_for_entry: impl Fn(&LexiconEntry) -> String,
    ) -> Self {
        let frequency_total = lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = frequency_total.ln();
        let mut unique_by_code = BTreeMap::<String, BTreeMap<String, ProtocolWord>>::new();

        for entry in lexicon {
            let code = code_for_entry(entry);
            let word = ProtocolWord {
                text: entry.text.clone(),
                log_probability: (entry.frequency as f64).ln() - log_frequency_total,
            };
            let words = unique_by_code.entry(code).or_default();
            match words.get(&entry.text) {
                Some(current) if current.log_probability >= word.log_probability => {}
                _ => {
                    words.insert(entry.text.clone(), word);
                }
            }
        }

        let mut stats = ProtocolIndexStats {
            distinct_codes: unique_by_code.len(),
            ..ProtocolIndexStats::default()
        };
        let words_by_code = unique_by_code
            .into_iter()
            .map(|(code, words)| {
                let mut words = words.into_values().collect::<Vec<_>>();
                words.sort_by(protocol_word_order);
                stats.indexed_texts += words.len();
                stats.colliding_codes += usize::from(words.len() > 1);
                stats.maximum_texts_per_code = stats.maximum_texts_per_code.max(words.len());
                stats.maximum_code_keys = stats.maximum_code_keys.max(code.len());
                (code, words)
            })
            .collect();
        Self {
            words_by_code,
            stats,
        }
    }

    fn decode(&self, observed: &str, top_k: usize) -> Vec<ProtocolPath> {
        if top_k == 0 || observed.is_empty() {
            return Vec::new();
        }
        let mut memo = vec![None; observed.len() + 1];
        self.decode_suffix(observed, 0, top_k, &mut memo)
    }

    fn decode_with_terminal_shortcuts(
        &self,
        terminal_index: &ProtocolIndex,
        observed: &str,
        top_k: usize,
    ) -> Vec<ProtocolPath> {
        if top_k == 0 || observed.is_empty() {
            return Vec::new();
        }
        let mut memo = vec![None; observed.len() + 1];
        self.decode_suffix_with_terminal_shortcuts(terminal_index, observed, 0, top_k, &mut memo)
    }

    fn decode_suffix_with_terminal_shortcuts(
        &self,
        terminal_index: &ProtocolIndex,
        observed: &str,
        start: usize,
        top_k: usize,
        memo: &mut [Option<Vec<ProtocolPath>>],
    ) -> Vec<ProtocolPath> {
        if start == observed.len() {
            return vec![ProtocolPath {
                text: String::new(),
                score: 0.0,
                segments: Vec::new(),
                segment_log_probabilities: Vec::new(),
            }];
        }
        if let Some(cached) = &memo[start] {
            return cached.clone();
        }

        let mut paths = Vec::new();
        for end in start + 1..=observed.len() {
            let Some(words) = self.words_by_code.get(&observed[start..end]) else {
                continue;
            };
            let suffixes = self.decode_suffix_with_terminal_shortcuts(
                terminal_index,
                observed,
                end,
                top_k,
                memo,
            );
            for word in words.iter().take(top_k) {
                for suffix in &suffixes {
                    paths.push(prepend_protocol_word(word, suffix));
                }
            }
        }

        if let Some(words) = terminal_index.words_by_code.get(&observed[start..]) {
            let empty_suffix = ProtocolPath {
                text: String::new(),
                score: 0.0,
                segments: Vec::new(),
                segment_log_probabilities: Vec::new(),
            };
            paths.extend(
                words
                    .iter()
                    .take(top_k)
                    .map(|word| prepend_protocol_word(word, &empty_suffix)),
            );
        }

        paths.sort_by(protocol_path_order);
        let mut texts = HashSet::new();
        paths.retain(|path| texts.insert(path.text.clone()));
        paths.truncate(top_k);
        memo[start] = Some(paths.clone());
        paths
    }

    fn decode_suffix(
        &self,
        observed: &str,
        start: usize,
        top_k: usize,
        memo: &mut [Option<Vec<ProtocolPath>>],
    ) -> Vec<ProtocolPath> {
        if start == observed.len() {
            return vec![ProtocolPath {
                text: String::new(),
                score: 0.0,
                segments: Vec::new(),
                segment_log_probabilities: Vec::new(),
            }];
        }
        if let Some(cached) = &memo[start] {
            return cached.clone();
        }

        let mut paths = Vec::new();
        for end in start + 1..=observed.len() {
            let Some(words) = self.words_by_code.get(&observed[start..end]) else {
                continue;
            };
            let suffixes = self.decode_suffix(observed, end, top_k, memo);
            for word in words.iter().take(top_k) {
                for suffix in &suffixes {
                    let mut segments = Vec::with_capacity(suffix.segments.len() + 1);
                    segments.push(word.text.clone());
                    segments.extend(suffix.segments.iter().cloned());
                    let mut segment_log_probabilities =
                        Vec::with_capacity(suffix.segment_log_probabilities.len() + 1);
                    segment_log_probabilities.push(word.log_probability);
                    segment_log_probabilities
                        .extend(suffix.segment_log_probabilities.iter().copied());
                    paths.push(ProtocolPath {
                        text: format!("{}{}", word.text, suffix.text),
                        score: word.log_probability + suffix.score,
                        segments,
                        segment_log_probabilities,
                    });
                }
            }
        }
        paths.sort_by(protocol_path_order);
        let mut texts = HashSet::new();
        paths.retain(|path| texts.insert(path.text.clone()));
        paths.truncate(top_k);
        memo[start] = Some(paths.clone());
        paths
    }

    fn decode_fixed_segments(&self, codes: &[&str], top_k: usize) -> Vec<ProtocolPath> {
        if top_k == 0 || codes.is_empty() {
            return Vec::new();
        }
        let mut paths = vec![ProtocolPath {
            text: String::new(),
            score: 0.0,
            segments: Vec::new(),
            segment_log_probabilities: Vec::new(),
        }];
        for code in codes {
            let Some(words) = self.words_by_code.get(*code) else {
                return Vec::new();
            };
            let mut next = Vec::new();
            for prefix in &paths {
                for word in words.iter().take(top_k) {
                    let mut segments = prefix.segments.clone();
                    segments.push(word.text.clone());
                    let mut segment_log_probabilities = prefix.segment_log_probabilities.clone();
                    segment_log_probabilities.push(word.log_probability);
                    next.push(ProtocolPath {
                        text: format!("{}{}", prefix.text, word.text),
                        score: prefix.score + word.log_probability,
                        segments,
                        segment_log_probabilities,
                    });
                }
            }
            next.sort_by(protocol_path_order);
            let mut texts = HashSet::new();
            next.retain(|path| texts.insert(path.text.clone()));
            next.truncate(top_k);
            paths = next;
        }
        paths
    }

    fn code_fanout(&self, code: &str) -> usize {
        self.words_by_code.get(code).map_or(0, Vec::len)
    }
}

fn prepend_protocol_word(word: &ProtocolWord, suffix: &ProtocolPath) -> ProtocolPath {
    let mut segments = Vec::with_capacity(suffix.segments.len() + 1);
    segments.push(word.text.clone());
    segments.extend(suffix.segments.iter().cloned());
    let mut segment_log_probabilities =
        Vec::with_capacity(suffix.segment_log_probabilities.len() + 1);
    segment_log_probabilities.push(word.log_probability);
    segment_log_probabilities.extend(suffix.segment_log_probabilities.iter().copied());
    ProtocolPath {
        text: format!("{}{}", word.text, suffix.text),
        score: word.log_probability + suffix.score,
        segments,
        segment_log_probabilities,
    }
}

fn best_entries_by_text(lexicon: &[LexiconEntry]) -> BTreeMap<&str, &LexiconEntry> {
    let mut entries = BTreeMap::<&str, &LexiconEntry>::new();
    for entry in lexicon {
        match entries.get(entry.text.as_str()) {
            Some(current)
                if current.frequency > entry.frequency
                    || (current.frequency == entry.frequency
                        && (current.pinyin.as_str(), current.code.as_str())
                            <= (entry.pinyin.as_str(), entry.code.as_str())) => {}
            _ => {
                entries.insert(entry.text.as_str(), entry);
            }
        }
    }
    entries
}

fn protocol_code_fanouts(
    lexicon: &[LexiconEntry],
    mode: ProtocolCodeMode,
) -> BTreeMap<String, usize> {
    let mut texts_by_code = BTreeMap::<String, HashSet<&str>>::new();
    for entry in lexicon {
        texts_by_code
            .entry(protocol_code(entry, mode))
            .or_default()
            .insert(entry.text.as_str());
    }
    texts_by_code
        .into_iter()
        .map(|(code, texts)| (code, texts.len()))
        .collect()
}

fn collision_gated_tail_code(
    entry: &LexiconEntry,
    conservative_fanouts: &BTreeMap<String, usize>,
    profile: CollisionGatedTailProfile,
) -> (String, bool, usize) {
    let shortened = protocol_code(entry, ProtocolCodeMode::ConservativeTail);
    let fanout = conservative_fanouts.get(&shortened).copied().unwrap_or(0);
    let allowed = entry.syllable_codes.len() >= 2
        && profile
            .maximum_shortened_code_fanout
            .is_some_and(|maximum| fanout <= maximum);
    if allowed {
        (shortened, true, fanout)
    } else {
        (entry.code.as_str().to_owned(), false, fanout)
    }
}

fn text_rank(candidates: &[ProtocolPath], expected_text: &str) -> Option<usize> {
    candidates
        .iter()
        .position(|candidate| candidate.text == expected_text)
        .map(|rank| rank + 1)
}

fn protocol_code(entry: &LexiconEntry, mode: ProtocolCodeMode) -> String {
    match mode {
        ProtocolCodeMode::Full => entry.code.as_str().to_owned(),
        ProtocolCodeMode::AnchoredTail => {
            let mut syllables = entry.syllable_codes.iter();
            let first = syllables
                .next()
                .expect("a parsed lexicon entry has at least one syllable");
            let mut code = first.as_str().to_owned();
            code.extend(syllables.map(first_key));
            code
        }
        ProtocolCodeMode::ConservativeTail => {
            if entry.syllable_codes.len() < 2 {
                return entry.code.as_str().to_owned();
            }
            let split = entry.syllable_codes.len() - 1;
            let mut code = entry.syllable_codes[..split]
                .iter()
                .map(crate::KeySequence::as_str)
                .collect::<String>();
            code.push(first_key(&entry.syllable_codes[split]));
            code
        }
        ProtocolCodeMode::AllShort => entry.syllable_codes.iter().map(first_key).collect(),
    }
}

fn first_key(code: &crate::KeySequence) -> char {
    code.as_str()
        .chars()
        .next()
        .expect("a parsed syllable code is non-empty")
}

fn protocol_word_order(left: &ProtocolWord, right: &ProtocolWord) -> Ordering {
    right
        .log_probability
        .total_cmp(&left.log_probability)
        .then_with(|| left.text.cmp(&right.text))
}

fn protocol_path_order(left: &ProtocolPath, right: &ProtocolPath) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then_with(|| left.segments.len().cmp(&right.segments.len()))
        .then_with(|| left.text.cmp(&right.text))
}

#[cfg(test)]
mod tests {
    use super::{
        COLLISION_GATED_TAIL_PROFILES, ProtocolCodeMode, ProtocolIndex,
        audit_anchored_tail_failures, audit_collision_gated_tail_protocols,
        audit_double_pinyin_key_trajectories, audit_public_protocol_context,
        audit_public_protocols, audit_terminal_collision_gated_tail_protocols,
    };
    use crate::{
        BigramLanguageModel, Decoder, KeySequence, PublicProtocolProbe, parse_lexicon_tsv,
    };

    const LEXICON: &str = "\
text\tpinyin\tfrequency
你\tni\t100
好\thao\t90
你好\tni hao\t500
拟好\tni hao\t50
号\thao\t80
泥好看\tni hao kan\t10000
";

    #[test]
    fn explicit_mode_keeps_fixed_syllable_width_and_ranks_deterministically() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let index = ProtocolIndex::new(&lexicon, ProtocolCodeMode::AllShort);
        let first = index.decode("nh", 10);
        let second = index.decode("nh", 10);
        assert_eq!(
            first.iter().map(|path| &path.text).collect::<Vec<_>>(),
            second.iter().map(|path| &path.text).collect::<Vec<_>>()
        );
        assert_eq!(first[0].text, "你好");
        assert!(index.stats.colliding_codes > 0);
    }

    #[test]
    fn protocol_report_counts_mode_and_whitelist_costs() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let probes = [PublicProtocolProbe {
            id: "synthetic".to_owned(),
            full_observed: KeySequence::new("nihk").unwrap(),
            anchored_tail_observed: KeySequence::new("nih").unwrap(),
            conservative_tail_observed: KeySequence::new("nih").unwrap(),
            explicit_abbreviation_observed: KeySequence::new("nh").unwrap(),
            expected_text: "你好".to_owned(),
            expected_segments: vec!["你好".to_owned()],
            whitelist_available: true,
        }];
        let report = audit_public_protocols(&lexicon, &probes);
        assert_eq!(report.full_code.hits_at_1, 1);
        assert_eq!(report.explicit_abbreviation.hits_at_1, 1);
        assert_eq!(report.explicit_abbreviation.activation_actions, 1);
        assert_eq!(report.whitelist.covered, 1);
        assert_eq!(report.whitelist.saved_letters, 2);
        assert_eq!(report.whitelist.net_actions_saved(), 1);
    }

    #[test]
    fn collision_gate_shortens_only_word_codes_below_its_structural_limit() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
你好\tni hao\t1000\n\
你很\tni hen\t100\n\
世界\tshi jie\t500\n",
        )
        .unwrap();
        let full_observed = ["你很", "世界"]
            .iter()
            .map(|text| {
                lexicon
                    .iter()
                    .find(|entry| entry.text == *text)
                    .expect("synthetic expected word exists")
                    .code
                    .as_str()
            })
            .collect::<String>();
        let probes = [PublicProtocolProbe {
            id: "synthetic-gate".to_owned(),
            full_observed: KeySequence::new(full_observed.clone()).unwrap(),
            anchored_tail_observed: KeySequence::new(full_observed.clone()).unwrap(),
            conservative_tail_observed: KeySequence::new(full_observed.clone()).unwrap(),
            explicit_abbreviation_observed: KeySequence::new(full_observed).unwrap(),
            expected_text: "你很世界".to_owned(),
            expected_segments: vec!["你很".to_owned(), "世界".to_owned()],
            whitelist_available: false,
        }];

        let reports = audit_collision_gated_tail_protocols(
            &lexicon,
            &probes,
            &COLLISION_GATED_TAIL_PROFILES[..3],
        );

        assert_eq!(reports[0].strategy.input_letters, 8);
        assert_eq!(reports[0].strategy.hits_at_1, 1);
        assert_eq!(reports[0].shortened_word_instances, 0);
        assert_eq!(reports[1].strategy.input_letters, 7);
        assert_eq!(reports[1].strategy.hits_at_1, 1);
        assert_eq!(reports[1].shortened_word_instances, 1);
        assert_eq!(reports[2].strategy.input_letters, 6);
        assert_eq!(reports[2].strategy.hits_at_1, 0);
        assert_eq!(reports[2].shortened_word_instances, 2);
        assert_eq!(reports[2].lost_top_1, 1);
        assert_eq!(reports[2].ambiguous_candidate_pools, 1);
    }

    #[test]
    fn terminal_collision_gate_preserves_interior_pair_synchronization() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
你好\tni hao\t1000\n\
你很\tni hen\t100\n\
世界\tshi jie\t500\n",
        )
        .unwrap();
        let full_observed = ["你很", "世界"]
            .iter()
            .map(|text| {
                lexicon
                    .iter()
                    .find(|entry| entry.text == *text)
                    .expect("synthetic expected word exists")
                    .code
                    .as_str()
            })
            .collect::<String>();
        let probes = [PublicProtocolProbe {
            id: "synthetic-terminal-gate".to_owned(),
            full_observed: KeySequence::new(full_observed.clone()).unwrap(),
            anchored_tail_observed: KeySequence::new(full_observed.clone()).unwrap(),
            conservative_tail_observed: KeySequence::new(full_observed.clone()).unwrap(),
            explicit_abbreviation_observed: KeySequence::new(full_observed).unwrap(),
            expected_text: "你很世界".to_owned(),
            expected_segments: vec!["你很".to_owned(), "世界".to_owned()],
            whitelist_available: false,
        }];

        let reports = audit_terminal_collision_gated_tail_protocols(
            &lexicon,
            &probes,
            &COLLISION_GATED_TAIL_PROFILES[..3],
        );

        assert_eq!(reports[2].strategy.input_letters, 7);
        assert_eq!(reports[2].strategy.hits_at_1, 1);
        assert_eq!(reports[2].multisyllable_word_instances, 1);
        assert_eq!(reports[2].shortened_word_instances, 1);
        assert_eq!(reports[2].lost_top_1, 0);
    }

    #[test]
    fn key_trajectory_pairs_initial_and_rhyme_states_and_reuses_prefixes() {
        let lexicon = parse_lexicon_tsv(
            "text\tpinyin\tfrequency\n\
呜\twu\t1000\n\
无\twu\t100\n\
哇\twa\t900\n\
呜哇\twu wa\t5000\n",
        )
        .unwrap();
        let full_observed = lexicon
            .iter()
            .find(|entry| entry.text == "呜哇")
            .expect("synthetic expected word exists")
            .code
            .clone();
        let probe = PublicProtocolProbe {
            id: "synthetic-trajectory".to_owned(),
            full_observed: full_observed.clone(),
            anchored_tail_observed: full_observed.clone(),
            conservative_tail_observed: full_observed.clone(),
            explicit_abbreviation_observed: full_observed.clone(),
            expected_text: "呜哇".to_owned(),
            expected_segments: vec!["呜哇".to_owned()],
            whitelist_available: false,
        };
        let mut repeated = probe.clone();
        repeated.id = "synthetic-trajectory-repeat".to_owned();
        let mut mismatched = probe.clone();
        mismatched.id = "synthetic-trajectory-mismatch".to_owned();
        mismatched.expected_text = "呜".to_owned();
        let decoder = Decoder::new(lexicon);

        let report =
            audit_double_pinyin_key_trajectories(&decoder, &[probe, repeated, mismatched], 10)
                .unwrap();

        assert_eq!(report.requested_probes, 3);
        assert_eq!(report.aligned_probes, 2);
        assert_eq!(report.alignment_mismatches, 1);
        assert_eq!(report.syllable_resolutions, 4);
        assert_eq!(report.odd.steps, 4);
        assert_eq!(report.even.steps, 4);
        assert_eq!(report.odd.hits_at_1, 4);
        assert_eq!(report.even.hits_at_1, 4);
        assert_eq!(report.target_visible_both, 4);
        assert_eq!(report.top_1_unchanged, 4);
        assert_eq!(report.decode_requests, 8);
        assert_eq!(report.unique_prefixes, 4);
        assert_eq!(report.prefix_cache_hits, 4);
    }

    #[test]
    fn boundary_hint_audit_separates_word_boundaries_from_word_code_collisions() {
        let lexicon = parse_lexicon_tsv(LEXICON).unwrap();
        let probes = [PublicProtocolProbe {
            id: "synthetic".to_owned(),
            full_observed: KeySequence::new("nihk").unwrap(),
            anchored_tail_observed: KeySequence::new("nihk").unwrap(),
            conservative_tail_observed: KeySequence::new("nihk").unwrap(),
            explicit_abbreviation_observed: KeySequence::new("nh").unwrap(),
            expected_text: "你好".to_owned(),
            expected_segments: vec!["你".to_owned(), "好".to_owned()],
            whitelist_available: false,
        }];
        let report = audit_anchored_tail_failures(&lexicon, &probes, 1, 10);
        assert_eq!(report.baseline_visible, 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.boundary_recovered_visible, 1);
        assert_eq!(report.failures[0].boundary_rank, Some(1));
        assert_eq!(report.failures[0].expected_word_code_fanouts, [1, 2]);
        assert_eq!(report.failures_with_word_code_collision, 1);
    }

    #[test]
    fn fit_context_reranks_a_frozen_pool_without_creating_paths() {
        let lexicon = parse_lexicon_tsv(
            "\
text\tpinyin\tfrequency
你\tni\t100
好\thao\t90
号\thao\t120
",
        )
        .unwrap();
        let language_model = BigramLanguageModel::from_token_sequences(
            &[
                vec!["你".to_owned(), "好".to_owned()],
                vec!["你".to_owned(), "好".to_owned()],
            ],
            &lexicon,
        )
        .unwrap();
        let probes = [PublicProtocolProbe {
            id: "synthetic".to_owned(),
            full_observed: KeySequence::new("nihk").unwrap(),
            anchored_tail_observed: KeySequence::new("nihk").unwrap(),
            conservative_tail_observed: KeySequence::new("nihk").unwrap(),
            explicit_abbreviation_observed: KeySequence::new("nh").unwrap(),
            expected_text: "你好".to_owned(),
            expected_segments: vec!["你".to_owned(), "好".to_owned()],
            whitelist_available: false,
        }];
        let report = audit_public_protocol_context(&lexicon, &probes, &language_model, 10);
        assert_eq!(report.anchored_tail.pool_visible, 1);
        assert_eq!(report.anchored_tail.baseline_hits_at_1, 0);
        assert_eq!(report.anchored_tail.context_hits_at_1, 1);
        assert_eq!(report.anchored_tail.improved_ranks, 1);
        assert_eq!(report.anchored_tail.repaired_into_top_10, 0);
    }
}
