use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::LexiconEntry;

#[derive(Clone, Debug)]
pub(crate) struct RankedCharacter {
    pub(crate) character: char,
    frequency: u64,
    pinyin: String,
}

impl RankedCharacter {
    pub(crate) fn pinyin(&self) -> &str {
        &self.pinyin
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SingleCharacterPoolIndex {
    source_entries: usize,
    pools: BTreeMap<String, Vec<RankedCharacter>>,
}

impl SingleCharacterPoolIndex {
    pub(crate) fn new(entries: &[LexiconEntry]) -> Self {
        let mut grouped = BTreeMap::<String, BTreeMap<char, RankedCharacter>>::new();
        let mut source_entries = 0usize;
        for entry in entries {
            let mut characters = entry.text.chars();
            let Some(character) = characters.next() else {
                continue;
            };
            if characters.next().is_some() {
                continue;
            }
            source_entries += 1;
            let candidate = RankedCharacter {
                character,
                frequency: entry.frequency,
                pinyin: entry.pinyin.clone(),
            };
            let by_character = grouped.entry(entry.code.as_str().to_owned()).or_default();
            match by_character.get(&character) {
                Some(existing) if compare_ranked(&candidate, existing) != Ordering::Less => {}
                _ => {
                    by_character.insert(character, candidate);
                }
            }
        }
        let pools = grouped
            .into_iter()
            .map(|(code, candidates)| {
                let mut candidates = candidates.into_values().collect::<Vec<_>>();
                candidates.sort_by(compare_ranked);
                (code, candidates)
            })
            .collect();
        Self {
            source_entries,
            pools,
        }
    }

    pub(crate) fn source_entries(&self) -> usize {
        self.source_entries
    }

    pub(crate) fn distinct_candidates(&self) -> usize {
        self.pools.values().map(Vec::len).sum()
    }

    pub(crate) fn len(&self) -> usize {
        self.pools.len()
    }

    pub(crate) fn pools(&self) -> impl Iterator<Item = &[RankedCharacter]> {
        self.pools.values().map(Vec::as_slice)
    }

    pub(crate) fn pool(&self, code: &str) -> Option<&[RankedCharacter]> {
        self.pools.get(code).map(Vec::as_slice)
    }

    pub(crate) fn rank(&self, code: &str, target: char) -> Option<usize> {
        self.pool(code)?
            .iter()
            .position(|candidate| candidate.character == target)
            .map(|index| index + 1)
    }
}

fn compare_ranked(left: &RankedCharacter, right: &RankedCharacter) -> Ordering {
    right
        .frequency
        .cmp(&left.frequency)
        .then_with(|| left.character.cmp(&right.character))
        .then_with(|| left.pinyin.cmp(&right.pinyin))
}
