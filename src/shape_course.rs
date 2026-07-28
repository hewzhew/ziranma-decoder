//! Deterministic public task selection for the interactive stroke course.
//!
//! Course levels describe the smallest prefix length that makes a hard target
//! visible under every accepted public stroke sequence. Selection is read-only,
//! deterministic, and independent from private capture data.

use crate::{
    CharacterShapeIndex, LexiconEntry,
    shape_evaluation::{
        SHAPE_COURSE_MAX_PREFIX_KEYS, SHAPE_COURSE_VISIBLE_LIMIT, stable_stroke_filter,
    },
    single_character_pool::SingleCharacterPoolIndex,
};

/// Maximum number of questions accepted by the interactive command.
pub const MAX_INTERACTIVE_SHAPE_COURSE_TASKS: usize = 50;

/// Structural difficulty of one public stroke-refinement question.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShapeCourseDifficulty {
    /// Every accepted public stroke order is visible after one key.
    Easy,
    /// Every accepted public stroke order first becomes visible after two keys.
    Medium,
    /// Every accepted public stroke order first becomes visible after three keys.
    Hard,
    /// Interleave easy, medium, and hard questions in that order.
    Mixed,
}

/// One immutable public question used by the interactive course.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeCourseTask {
    /// Tone-free public reading used to open the frozen exact-code pool.
    pub pinyin: String,
    /// Public target displayed in the course prompt.
    pub character: char,
    /// One-based position before stroke filtering.
    pub ordinary_rank: usize,
    /// Smallest robust prefix length in the range one through three.
    pub minimum_prefix_keys: usize,
}

/// Selects a stable, spread-out slice of hard public targets.
pub fn select_shape_course_tasks(
    entries: &[LexiconEntry],
    shapes: &CharacterShapeIndex,
    difficulty: ShapeCourseDifficulty,
    count: usize,
) -> Vec<ShapeCourseTask> {
    let pools = SingleCharacterPoolIndex::new(entries);
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];

    for pool in pools.pools() {
        if pool.len() <= SHAPE_COURSE_VISIBLE_LIMIT {
            continue;
        }
        for target_index in SHAPE_COURSE_VISIBLE_LIMIT..pool.len() {
            let target = &pool[target_index];
            let Some(shape) = shapes.get(target.character) else {
                continue;
            };
            let Some(minimum_prefix_keys) = (1..=SHAPE_COURSE_MAX_PREFIX_KEYS).find(|keys| {
                !shape.stroke_codes().is_empty()
                    && shape.stroke_codes().iter().all(|code| {
                        let prefix_length = (*keys).min(code.len());
                        let prefix = &code[..prefix_length];
                        stable_stroke_filter(pool, target_index, prefix, shapes)
                            .1
                            .is_some_and(|rank| rank <= SHAPE_COURSE_VISIBLE_LIMIT)
                    })
            }) else {
                continue;
            };
            buckets[minimum_prefix_keys - 1].push(ShapeCourseTask {
                pinyin: target.pinyin().to_owned(),
                character: target.character,
                ordinary_rank: target_index + 1,
                minimum_prefix_keys,
            });
        }
    }

    for bucket in &mut buckets {
        bucket.sort_by(|left, right| {
            stable_task_hash(left)
                .cmp(&stable_task_hash(right))
                .then_with(|| left.character.cmp(&right.character))
                .then_with(|| left.pinyin.cmp(&right.pinyin))
        });
    }

    let mut selected = match difficulty {
        ShapeCourseDifficulty::Easy => std::mem::take(&mut buckets[0]),
        ShapeCourseDifficulty::Medium => std::mem::take(&mut buckets[1]),
        ShapeCourseDifficulty::Hard => std::mem::take(&mut buckets[2]),
        ShapeCourseDifficulty::Mixed => interleave(buckets),
    };
    selected.truncate(count.min(MAX_INTERACTIVE_SHAPE_COURSE_TASKS));
    selected
}

fn interleave(buckets: [Vec<ShapeCourseTask>; 3]) -> Vec<ShapeCourseTask> {
    let capacity = buckets.iter().map(Vec::len).sum();
    let maximum = buckets.iter().map(Vec::len).max().unwrap_or(0);
    let mut output = Vec::with_capacity(capacity);
    for index in 0..maximum {
        for bucket in &buckets {
            if let Some(task) = bucket.get(index) {
                output.push(task.clone());
            }
        }
    }
    output
}

fn stable_task_hash(task: &ShapeCourseTask) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    task.pinyin
        .as_bytes()
        .iter()
        .copied()
        .chain(u32::from(task.character).to_le_bytes())
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ShapeCourseDifficulty, select_shape_course_tasks};
    use crate::{
        CharacterShape, CharacterShapeIndex, KeySequence, LexiconEntry, parse_rime_lexicon,
        parse_stroke_sequence_tsv,
    };

    fn pool(pinyin: &str, code: &str, base: u32, strokes: [&str; 11]) -> Vec<LexiconEntry> {
        strokes
            .iter()
            .enumerate()
            .map(|(index, _)| LexiconEntry {
                text: char::from_u32(base + index as u32).unwrap().to_string(),
                pinyin: pinyin.to_owned(),
                code: KeySequence::new(code).unwrap(),
                syllable_codes: vec![KeySequence::new(code).unwrap()],
                frequency: 100 - index as u64,
            })
            .collect()
    }

    fn fixture() -> (Vec<LexiconEntry>, CharacterShapeIndex) {
        let easy = ["hhh"; 10].into_iter().chain(["nsp"]).collect::<Vec<_>>();
        let medium = ["nsh"; 5]
            .into_iter()
            .chain(["nhh"; 5])
            .chain(["nhp"])
            .collect::<Vec<_>>();
        let hard = ["nhh"; 10].into_iter().chain(["nhp"]).collect::<Vec<_>>();
        let mut entries = pool("shi", "ui", 0x4e00, easy.clone().try_into().unwrap());
        entries.extend(pool("yi", "yi", 0x4e20, medium.clone().try_into().unwrap()));
        entries.extend(pool("da", "da", 0x4e40, hard.clone().try_into().unwrap()));
        let shapes = CharacterShapeIndex::new(
            easy.into_iter()
                .enumerate()
                .map(|(index, code)| (0x4e00 + index as u32, code))
                .chain(
                    medium
                        .into_iter()
                        .enumerate()
                        .map(|(index, code)| (0x4e20 + index as u32, code)),
                )
                .chain(
                    hard.into_iter()
                        .enumerate()
                        .map(|(index, code)| (0x4e40 + index as u32, code)),
                )
                .map(|(scalar, code)| {
                    CharacterShape::new(
                        char::from_u32(scalar).unwrap(),
                        vec![code.to_owned()],
                        Vec::new(),
                    )
                    .unwrap()
                }),
        )
        .unwrap();
        (entries, shapes)
    }

    #[test]
    fn selects_each_robust_minimum_prefix_level() {
        let (entries, shapes) = fixture();
        for (difficulty, keys) in [
            (ShapeCourseDifficulty::Easy, 1),
            (ShapeCourseDifficulty::Medium, 2),
            (ShapeCourseDifficulty::Hard, 3),
        ] {
            let tasks = select_shape_course_tasks(&entries, &shapes, difficulty, 10);
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].minimum_prefix_keys, keys);
            assert_eq!(tasks[0].ordinary_rank, 11);
        }
    }

    #[test]
    fn mixed_course_interleaves_levels_and_respects_count() {
        let (entries, shapes) = fixture();
        let tasks = select_shape_course_tasks(&entries, &shapes, ShapeCourseDifficulty::Mixed, 2);
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.minimum_prefix_keys)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn pinned_public_mixed_course_has_stable_opening_questions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lexicon = parse_rime_lexicon(
            &std::fs::read_to_string(
                root.join("data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"),
            )
            .unwrap(),
        )
        .unwrap();
        let shapes = CharacterShapeIndex::new(
            parse_stroke_sequence_tsv(
                &std::fs::read_to_string(
                    root.join("data/public/conway-stroke-data/sequence-characters.txt"),
                )
                .unwrap(),
            )
            .unwrap()
            .into_shapes(),
        )
        .unwrap();
        let tasks =
            select_shape_course_tasks(&lexicon.entries, &shapes, ShapeCourseDifficulty::Mixed, 3);
        assert_eq!(
            tasks
                .iter()
                .map(|task| (
                    task.character,
                    task.pinyin.as_str(),
                    task.minimum_prefix_keys
                ))
                .collect::<Vec<_>>(),
            vec![('嘧', "mi", 1), ('閎', "hong", 2), ('慾', "yu", 3)]
        );
    }
}
