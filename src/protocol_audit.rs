use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use crate::{BigramLanguageModel, LexiconEntry, PublicProtocolProbe};

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
        let frequency_total = lexicon
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = frequency_total.ln();
        let mut unique_by_code = BTreeMap::<String, BTreeMap<String, ProtocolWord>>::new();

        for entry in lexicon {
            let code = protocol_code(entry, mode);
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
        ProtocolCodeMode, ProtocolIndex, audit_anchored_tail_failures,
        audit_public_protocol_context, audit_public_protocols,
    };
    use crate::{BigramLanguageModel, KeySequence, PublicProtocolProbe, parse_lexicon_tsv};

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
