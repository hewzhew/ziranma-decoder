//! Public, deterministic course evaluation for explicit Tab stroke refinement.
//!
//! This module deliberately evaluates exact full-code single-character pools.
//! It does not claim that source dictionary weights reproduce a real IME's
//! candidate order, and it does not touch private capture data.

use crate::{
    CharacterShapeIndex, LexiconEntry,
    single_character_pool::{RankedCharacter, SingleCharacterPoolIndex},
};

/// Candidate rank treated as visible without paging in the public course.
pub const SHAPE_COURSE_VISIBLE_LIMIT: usize = 10;
/// Longest five-stroke prefix evaluated by the public course.
pub const SHAPE_COURSE_MAX_PREFIX_KEYS: usize = 3;

/// Results for one `up to N stroke keys` checkpoint.
///
/// Sequence-attempt metrics give every accepted alternative stroke order one
/// trial. Target-level `any` and `all` metrics make the optimistic/robust
/// boundary visible instead of silently overweighting characters with many
/// alternatives.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShapePrefixCourseStats {
    /// Maximum number of stroke keys used at this checkpoint.
    pub prefix_keys: usize,
    /// Alternative-sequence trials for hard targets with stroke data.
    pub sequence_attempts: usize,
    /// Trials in which the target survived its own accepted prefix.
    pub target_retained_attempts: usize,
    /// Trials in which the target's stable filtered rank was at most ten.
    pub target_visible_attempts: usize,
    /// Trials in which the prefix isolated the target as the sole match.
    pub target_isolated_attempts: usize,
    /// Sum of frozen pool sizes before filtering, one term per trial.
    pub candidates_before_sum: usize,
    /// Sum of filtered pool sizes, one term per trial.
    pub candidates_after_sum: usize,
    /// Targets visible with at least one accepted stroke order.
    pub targets_visible_with_any_sequence: usize,
    /// Targets visible with every accepted stroke order.
    pub targets_visible_with_all_sequences: usize,
    /// Targets isolated with at least one accepted stroke order.
    pub targets_isolated_with_any_sequence: usize,
    /// Targets isolated with every accepted stroke order.
    pub targets_isolated_with_all_sequences: usize,
}

/// Structural results from the fixed public single-character course.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeCourseAuditReport {
    /// Accepted input rows whose text is exactly one Unicode scalar.
    pub single_character_entries: usize,
    /// Distinct `(full code, character)` candidates after defensive deduplication.
    pub distinct_single_character_candidates: usize,
    /// Exact full-code pools containing at least one candidate.
    pub phonetic_pools: usize,
    /// Pools containing two or more distinct candidates.
    pub ambiguous_pools: usize,
    /// Candidate occurrences across ambiguous pools.
    pub candidates_in_ambiguous_pools: usize,
    /// Pools containing more candidates than the visible limit.
    pub hard_pools: usize,
    /// Largest exact full-code pool.
    pub maximum_pool_size: usize,
    /// Candidate occurrences across hard pools.
    pub candidates_in_hard_pools: usize,
    /// Hard-pool candidate occurrences with at least one stroke sequence.
    pub hard_pool_candidates_with_stroke_data: usize,
    /// Targets initially ranked below the first ten candidates.
    pub hard_targets: usize,
    /// Hard targets with at least one accepted stroke sequence.
    pub hard_targets_with_stroke_data: usize,
    /// Hard targets without an accepted stroke sequence.
    pub hard_targets_without_stroke_data: usize,
    /// Hard targets with two or more accepted stroke sequences.
    pub hard_targets_with_alternative_sequences: usize,
    /// Checkpoints for one, two, and three stroke keys.
    pub prefixes: Vec<ShapePrefixCourseStats>,
}

/// Evaluates five-stroke refinement on exact full-code single-character pools.
///
/// Within each pool, source weight descending is only a deterministic ranking
/// proxy. Ties use candidate text and then pinyin. Only targets below rank ten
/// are exercised, because targets already on the first page do not need Tab.
/// For a stroke sequence shorter than a checkpoint, its complete sequence is
/// reused; consequently `prefix_keys = 3` means "up to three keys".
pub fn audit_shape_refinement_course(
    entries: &[LexiconEntry],
    shapes: &CharacterShapeIndex,
) -> ShapeCourseAuditReport {
    let pools = SingleCharacterPoolIndex::new(entries);
    let mut report = ShapeCourseAuditReport {
        single_character_entries: pools.source_entries(),
        distinct_single_character_candidates: pools.distinct_candidates(),
        phonetic_pools: pools.len(),
        ambiguous_pools: 0,
        candidates_in_ambiguous_pools: 0,
        hard_pools: 0,
        maximum_pool_size: 0,
        candidates_in_hard_pools: 0,
        hard_pool_candidates_with_stroke_data: 0,
        hard_targets: 0,
        hard_targets_with_stroke_data: 0,
        hard_targets_without_stroke_data: 0,
        hard_targets_with_alternative_sequences: 0,
        prefixes: (1..=SHAPE_COURSE_MAX_PREFIX_KEYS)
            .map(|prefix_keys| ShapePrefixCourseStats {
                prefix_keys,
                ..ShapePrefixCourseStats::default()
            })
            .collect(),
    };

    for pool in pools.pools() {
        report.maximum_pool_size = report.maximum_pool_size.max(pool.len());
        if pool.len() >= 2 {
            report.ambiguous_pools += 1;
            report.candidates_in_ambiguous_pools += pool.len();
        }
        if pool.len() <= SHAPE_COURSE_VISIBLE_LIMIT {
            continue;
        }

        report.hard_pools += 1;
        report.candidates_in_hard_pools += pool.len();
        report.hard_pool_candidates_with_stroke_data += pool
            .iter()
            .filter(|entry| {
                shapes
                    .get(entry.character)
                    .is_some_and(|shape| !shape.stroke_codes().is_empty())
            })
            .count();

        for target_index in SHAPE_COURSE_VISIBLE_LIMIT..pool.len() {
            report.hard_targets += 1;
            let target = &pool[target_index];
            let Some(target_shape) = shapes.get(target.character) else {
                report.hard_targets_without_stroke_data += 1;
                continue;
            };
            if target_shape.stroke_codes().is_empty() {
                report.hard_targets_without_stroke_data += 1;
                continue;
            }

            report.hard_targets_with_stroke_data += 1;
            if target_shape.stroke_codes().len() >= 2 {
                report.hard_targets_with_alternative_sequences += 1;
            }

            for stats in &mut report.prefixes {
                let mut any_visible = false;
                let mut all_visible = true;
                let mut any_isolated = false;
                let mut all_isolated = true;

                for code in target_shape.stroke_codes() {
                    let prefix_length = stats.prefix_keys.min(code.len());
                    let prefix = &code[..prefix_length];
                    let (filtered_candidates, target_rank) =
                        stable_stroke_filter(pool, target_index, prefix, shapes);

                    stats.sequence_attempts += 1;
                    stats.candidates_before_sum += pool.len();
                    stats.candidates_after_sum += filtered_candidates;
                    let retained = target_rank.is_some();
                    stats.target_retained_attempts += usize::from(retained);
                    let visible =
                        target_rank.is_some_and(|rank| rank <= SHAPE_COURSE_VISIBLE_LIMIT);
                    let isolated = retained && filtered_candidates == 1;
                    stats.target_visible_attempts += usize::from(visible);
                    stats.target_isolated_attempts += usize::from(isolated);
                    any_visible |= visible;
                    all_visible &= visible;
                    any_isolated |= isolated;
                    all_isolated &= isolated;
                }

                stats.targets_visible_with_any_sequence += usize::from(any_visible);
                stats.targets_visible_with_all_sequences += usize::from(all_visible);
                stats.targets_isolated_with_any_sequence += usize::from(any_isolated);
                stats.targets_isolated_with_all_sequences += usize::from(all_isolated);
            }
        }
    }

    report
}

fn stable_stroke_filter(
    pool: &[RankedCharacter],
    target_index: usize,
    prefix: &str,
    shapes: &CharacterShapeIndex,
) -> (usize, Option<usize>) {
    let mut filtered_candidates = 0usize;
    let mut target_rank = None;
    for (candidate_index, candidate) in pool.iter().enumerate() {
        let matches = shapes.get(candidate.character).is_some_and(|shape| {
            shape
                .stroke_codes()
                .iter()
                .any(|code| code.starts_with(prefix))
        });
        if !matches {
            continue;
        }
        filtered_candidates += 1;
        if candidate_index == target_index {
            target_rank = Some(filtered_candidates);
        }
    }
    (filtered_candidates, target_rank)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{SHAPE_COURSE_VISIBLE_LIMIT, audit_shape_refinement_course};
    use crate::{
        CharacterShape, CharacterShapeIndex, KeySequence, LexiconEntry, parse_rime_lexicon,
        parse_stroke_sequence_tsv,
    };

    fn entry(text: char, frequency: u64) -> LexiconEntry {
        LexiconEntry {
            text: text.to_string(),
            pinyin: "shi".to_owned(),
            code: KeySequence::new("ui").unwrap(),
            syllable_codes: vec![KeySequence::new("ui").unwrap()],
            frequency,
        }
    }

    #[test]
    fn course_measures_only_targets_below_the_visible_limit() {
        let characters = [
            '甲', '乙', '丙', '丁', '戊', '己', '庚', '辛', '壬', '癸', '子', '丑',
        ];
        let entries = characters
            .iter()
            .enumerate()
            .map(|(index, character)| entry(*character, 100 - index as u64))
            .collect::<Vec<_>>();
        let shapes =
            CharacterShapeIndex::new(characters.iter().enumerate().map(|(index, character)| {
                let code = if index < SHAPE_COURSE_VISIBLE_LIMIT {
                    "hsp"
                } else if index == SHAPE_COURSE_VISIBLE_LIMIT {
                    "nhh"
                } else {
                    "nsh"
                };
                CharacterShape::new(*character, vec![code.to_owned()], Vec::new()).unwrap()
            }))
            .unwrap();

        let report = audit_shape_refinement_course(&entries, &shapes);
        assert_eq!(report.single_character_entries, 12);
        assert_eq!(report.phonetic_pools, 1);
        assert_eq!(report.hard_pools, 1);
        assert_eq!(report.hard_targets, 2);
        assert_eq!(report.hard_targets_with_stroke_data, 2);
        assert_eq!(report.prefixes.len(), 3);
        assert_eq!(report.prefixes[0].target_retained_attempts, 2);
        assert_eq!(report.prefixes[0].targets_visible_with_all_sequences, 2);
        assert_eq!(report.prefixes[0].candidates_after_sum, 4);
        assert_eq!(report.prefixes[1].targets_isolated_with_all_sequences, 2);
    }

    #[test]
    fn course_exposes_any_all_gap_for_alternative_stroke_orders() {
        let characters = [
            '甲', '乙', '丙', '丁', '戊', '己', '庚', '辛', '壬', '癸', '子',
        ];
        let entries = characters
            .iter()
            .enumerate()
            .map(|(index, character)| entry(*character, 100 - index as u64))
            .collect::<Vec<_>>();
        let shapes =
            CharacterShapeIndex::new(characters.iter().enumerate().map(|(index, character)| {
                let codes = if index == SHAPE_COURSE_VISIBLE_LIMIT {
                    vec!["nh".to_owned(), "hh".to_owned()]
                } else {
                    vec!["hh".to_owned()]
                };
                CharacterShape::new(*character, codes, Vec::new()).unwrap()
            }))
            .unwrap();

        let report = audit_shape_refinement_course(&entries, &shapes);
        assert_eq!(report.hard_targets, 1);
        assert_eq!(report.hard_targets_with_alternative_sequences, 1);
        assert_eq!(report.prefixes[0].sequence_attempts, 2);
        assert_eq!(report.prefixes[0].targets_visible_with_any_sequence, 1);
        assert_eq!(report.prefixes[0].targets_visible_with_all_sequences, 0);
        assert_eq!(report.prefixes[1].targets_isolated_with_any_sequence, 1);
        assert_eq!(report.prefixes[1].targets_isolated_with_all_sequences, 0);
    }

    #[test]
    fn pinned_public_course_has_stable_structural_results() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let rime_input = std::fs::read_to_string(
            root.join("data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"),
        )
        .unwrap();
        let stroke_input = std::fs::read_to_string(
            root.join("data/public/conway-stroke-data/sequence-characters.txt"),
        )
        .unwrap();
        let lexicon = parse_rime_lexicon(&rime_input).unwrap();
        let shapes = CharacterShapeIndex::new(
            parse_stroke_sequence_tsv(&stroke_input)
                .unwrap()
                .into_shapes(),
        )
        .unwrap();

        let report = audit_shape_refinement_course(&lexicon.entries, &shapes);
        assert_eq!(report.single_character_entries, 17_038);
        assert_eq!(report.phonetic_pools, 410);
        assert_eq!(report.hard_pools, 342);
        assert_eq!(report.maximum_pool_size, 312);
        assert_eq!(report.hard_targets, 13_285);
        assert_eq!(report.hard_targets_without_stroke_data, 0);
        assert_eq!(report.prefixes[0].target_visible_attempts, 15_410);
        assert_eq!(report.prefixes[1].target_visible_attempts, 24_796);
        assert_eq!(report.prefixes[2].target_visible_attempts, 28_292);
        assert_eq!(
            report.prefixes[2].targets_visible_with_all_sequences,
            12_589
        );
    }
}
