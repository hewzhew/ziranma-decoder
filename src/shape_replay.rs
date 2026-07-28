//! Redacted private-replay metrics for explicit Tab stroke refinement.
//!
//! The analyzer receives already loaded capsules, keeps no private strings,
//! performs no I/O, and exposes only aggregate counters. Public dictionary
//! weights remain a structural ranking proxy rather than a claim about the
//! candidate order shown by the user's installed IME.

use crate::{
    CharacterShapeIndex, DeltaPositionEvidence, EventCapsuleV1, LexiconEntry, RawKey,
    SHAPE_COURSE_MAX_PREFIX_KEYS, SHAPE_COURSE_VISIBLE_LIMIT, TextDelta, TrackerOutput,
    effective_letter_code, encode_pinyin_phrase, single_character_pool::SingleCharacterPoolIndex,
};
use std::cmp::Ordering;

/// Longest clean multi-character commit considered as a phrase-trim candidate.
pub const PHRASE_TRIM_MAX_CHARACTERS: usize = 4;
/// Maximum time from a phrase commit to the deletion that leaves one character.
pub const PHRASE_TRIM_MAX_GAP_MS: u64 = 5_000;

/// Redacted comparison between a recorded action lower bound and one projection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PrivateShapeActionComparisonStats {
    /// Comparable events.
    pub cases: u64,
    /// Sum of captured logical key actions. Uncaptured keys remain unknown.
    pub recorded_action_lower_bound: u64,
    /// Sum of canonical phonetic letters, Tab, shape prefix, and one selection.
    pub projected_shape_actions: u64,
    /// Events whose projection uses fewer actions than the recorded lower bound.
    pub projected_fewer: u64,
    /// Events whose projection equals the recorded lower bound.
    pub projected_equal: u64,
    /// Events whose projection uses more actions than the recorded lower bound.
    pub projected_more: u64,
}

impl PrivateShapeActionComparisonStats {
    fn observe(&mut self, recorded: usize, projected: usize) {
        self.cases = self.cases.saturating_add(1);
        self.recorded_action_lower_bound = self
            .recorded_action_lower_bound
            .saturating_add(saturating_usize(recorded));
        self.projected_shape_actions = self
            .projected_shape_actions
            .saturating_add(saturating_usize(projected));
        match projected.cmp(&recorded) {
            Ordering::Less => self.projected_fewer = self.projected_fewer.saturating_add(1),
            Ordering::Equal => self.projected_equal = self.projected_equal.saturating_add(1),
            Ordering::Greater => self.projected_more = self.projected_more.saturating_add(1),
        }
    }
}

/// Text-free aggregate output from one or more explicitly selected capsules.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrivateShapeReplayReport {
    pub capsules: u64,
    pub events: u64,
    pub commits: u64,
    pub revisions: u64,
    pub keys_complete_commits: u64,
    pub commits_with_digit_selection_signal: u64,
    pub commits_with_space_selection_signal: u64,
    pub commits_with_vertical_navigation_signal: u64,
    pub commits_with_internal_edit_keys: u64,
    pub single_character_insert_commits: u64,
    pub single_character_phonetic_commits: u64,
    pub single_character_noncanonical_observations: u64,
    pub single_character_public_ranked_commits: u64,
    pub single_character_public_top_10_commits: u64,
    pub single_character_public_beyond_top_10_commits: u64,
    pub single_character_hard_with_stroke_data: u64,
    pub single_character_hard_noncanonical_observations: u64,
    pub single_character_hard_visible_with_any_sequence: [u64; SHAPE_COURSE_MAX_PREFIX_KEYS],
    pub single_character_hard_visible_with_all_sequences: [u64; SHAPE_COURSE_MAX_PREFIX_KEYS],
    pub single_character_best_action_comparison: PrivateShapeActionComparisonStats,
    pub single_character_robust_action_comparison: PrivateShapeActionComparisonStats,
    pub phrase_trim_candidates: u64,
    pub phrase_trim_completed: u64,
    pub phrase_trim_phonetic_aligned: u64,
    pub phrase_trim_noncanonical_observations: u64,
    pub phrase_trim_public_ranked: u64,
    pub phrase_trim_public_beyond_top_10: u64,
    pub phrase_trim_hard_with_stroke_data: u64,
    pub phrase_trim_hard_visible_with_any_sequence: [u64; SHAPE_COURSE_MAX_PREFIX_KEYS],
    pub phrase_trim_hard_visible_with_all_sequences: [u64; SHAPE_COURSE_MAX_PREFIX_KEYS],
    pub phrase_trim_best_action_comparison: PrivateShapeActionComparisonStats,
    pub phrase_trim_robust_action_comparison: PrivateShapeActionComparisonStats,
}

/// Memory-only analyzer built from public lexicon and stroke metadata.
pub struct PrivateShapeReplayAudit<'a> {
    pools: SingleCharacterPoolIndex,
    shapes: &'a CharacterShapeIndex,
    report: PrivateShapeReplayReport,
}

#[derive(Clone, Copy, Debug, Default)]
struct ShapeOutcome {
    visible_any: [bool; SHAPE_COURSE_MAX_PREFIX_KEYS],
    visible_all: [bool; SHAPE_COURSE_MAX_PREFIX_KEYS],
    minimum_any_prefix: Option<usize>,
    minimum_all_prefix: Option<usize>,
}

struct PendingPhraseTrim {
    started_ms: u64,
    region_start: usize,
    remaining: Vec<(usize, char)>,
    syllable_codes: Option<Vec<String>>,
    observed_is_canonical: Option<bool>,
    recorded_actions: usize,
}

impl<'a> PrivateShapeReplayAudit<'a> {
    /// Builds frozen exact-full-code pools without retaining private material.
    pub fn new(entries: &[LexiconEntry], shapes: &'a CharacterShapeIndex) -> Self {
        Self {
            pools: SingleCharacterPoolIndex::new(entries),
            shapes,
            report: PrivateShapeReplayReport::default(),
        }
    }

    /// Observes one capsule and discards all private strings before returning.
    ///
    /// Phrase-trim detection intentionally stops at capsule boundaries. The
    /// resulting count is a lower bound, but it cannot accidentally join edits
    /// across a rotation or a separate baseline.
    pub fn observe_capsule(&mut self, capsule: &EventCapsuleV1) {
        self.report.capsules = self.report.capsules.saturating_add(1);
        self.report.events = self
            .report
            .events
            .saturating_add(saturating_usize(capsule.events().len()));
        let mut pending_trim = None;

        for event in capsule.events() {
            match &event.output {
                TrackerOutput::Commit(commit) => {
                    self.report.commits = self.report.commits.saturating_add(1);
                    if commit.keys_complete {
                        self.report.keys_complete_commits =
                            self.report.keys_complete_commits.saturating_add(1);
                    }
                    self.observe_commit_key_signals(&commit.keys);
                    self.observe_single_character_commit(commit);
                    pending_trim = self.start_phrase_trim(event.elapsed_ms, commit);
                }
                TrackerOutput::Revision(revision) => {
                    self.report.revisions = self.report.revisions.saturating_add(1);
                    let Some(mut pending) = pending_trim.take() else {
                        continue;
                    };
                    if event.elapsed_ms.saturating_sub(pending.started_ms) > PHRASE_TRIM_MAX_GAP_MS
                        || !revision.keys_complete
                        || revision.change.position_evidence == DeltaPositionEvidence::Ambiguous
                        || !revision.change.inserted.is_empty()
                        || revision.change.deleted.is_empty()
                        || !apply_delete_to_pending(&mut pending, &revision.change)
                    {
                        continue;
                    }
                    pending.recorded_actions =
                        pending.recorded_actions.saturating_add(revision.keys.len());
                    if pending.remaining.len() == 1 {
                        self.observe_completed_phrase_trim(pending);
                    } else if !pending.remaining.is_empty() {
                        pending_trim = Some(pending);
                    }
                }
            }
        }
    }

    /// Returns the text-free aggregate report.
    pub fn report(&self) -> &PrivateShapeReplayReport {
        &self.report
    }

    /// Consumes the analyzer and returns only aggregate counters.
    pub fn into_report(self) -> PrivateShapeReplayReport {
        self.report
    }

    fn observe_commit_key_signals(&mut self, keys: &[RawKey]) {
        self.report.commits_with_digit_selection_signal = self
            .report
            .commits_with_digit_selection_signal
            .saturating_add(u64::from(keys.iter().any(|key| key_matches(key, is_digit))));
        self.report.commits_with_space_selection_signal = self
            .report
            .commits_with_space_selection_signal
            .saturating_add(u64::from(keys.iter().any(|key| key_matches(key, is_space))));
        self.report.commits_with_vertical_navigation_signal = self
            .report
            .commits_with_vertical_navigation_signal
            .saturating_add(u64::from(
                keys.iter().any(|key| key_matches(key, is_vertical)),
            ));
        self.report.commits_with_internal_edit_keys = self
            .report
            .commits_with_internal_edit_keys
            .saturating_add(u64::from(
                keys.iter().any(|key| key_matches(key, is_internal_edit)),
            ));
    }

    fn observe_single_character_commit(&mut self, commit: &crate::CommitRecord) {
        if !commit.keys_complete
            || !commit.document_change.deleted.is_empty()
            || commit.document_change.position_evidence == DeltaPositionEvidence::Ambiguous
        {
            return;
        }
        let mut characters = commit.document_change.inserted.chars();
        let Some(character) = characters.next() else {
            return;
        };
        if characters.next().is_some() {
            return;
        }
        self.report.single_character_insert_commits = self
            .report
            .single_character_insert_commits
            .saturating_add(1);

        let normalized_pinyin = commit.composition.replace('\'', " ");
        let Ok(encoded) = encode_pinyin_phrase(&normalized_pinyin) else {
            return;
        };
        if encoded.syllable_codes.len() != 1 {
            return;
        }
        self.report.single_character_phonetic_commits = self
            .report
            .single_character_phonetic_commits
            .saturating_add(1);
        let canonical_code = encoded.syllable_codes[0].as_str();
        let observed_is_canonical = effective_letter_code(&commit.keys)
            .ok()
            .flatten()
            .is_some_and(|observed| observed == canonical_code);
        if !observed_is_canonical {
            self.report.single_character_noncanonical_observations = self
                .report
                .single_character_noncanonical_observations
                .saturating_add(1);
        }
        let Some(rank) = self.rank(canonical_code, character) else {
            return;
        };
        self.report.single_character_public_ranked_commits = self
            .report
            .single_character_public_ranked_commits
            .saturating_add(1);
        if rank <= SHAPE_COURSE_VISIBLE_LIMIT {
            self.report.single_character_public_top_10_commits = self
                .report
                .single_character_public_top_10_commits
                .saturating_add(1);
            return;
        }
        self.report.single_character_public_beyond_top_10_commits = self
            .report
            .single_character_public_beyond_top_10_commits
            .saturating_add(1);

        let Some(outcome) = self.shape_outcome(canonical_code, character) else {
            return;
        };
        self.report.single_character_hard_with_stroke_data = self
            .report
            .single_character_hard_with_stroke_data
            .saturating_add(1);
        observe_visibility(
            &mut self.report.single_character_hard_visible_with_any_sequence,
            outcome.visible_any,
        );
        observe_visibility(
            &mut self.report.single_character_hard_visible_with_all_sequences,
            outcome.visible_all,
        );

        if !observed_is_canonical {
            self.report.single_character_hard_noncanonical_observations = self
                .report
                .single_character_hard_noncanonical_observations
                .saturating_add(1);
            return;
        }
        if let Some(prefix) = outcome.minimum_any_prefix {
            self.report
                .single_character_best_action_comparison
                .observe(commit.keys.len(), canonical_code.len() + 1 + prefix + 1);
        }
        if let Some(prefix) = outcome.minimum_all_prefix {
            self.report
                .single_character_robust_action_comparison
                .observe(commit.keys.len(), canonical_code.len() + 1 + prefix + 1);
        }
    }

    fn start_phrase_trim(
        &mut self,
        elapsed_ms: u64,
        commit: &crate::CommitRecord,
    ) -> Option<PendingPhraseTrim> {
        if !commit.keys_complete
            || !commit.document_change.deleted.is_empty()
            || commit.document_change.position_evidence == DeltaPositionEvidence::Ambiguous
        {
            return None;
        }
        let characters = commit
            .document_change
            .inserted
            .chars()
            .enumerate()
            .collect::<Vec<_>>();
        if !(2..=PHRASE_TRIM_MAX_CHARACTERS).contains(&characters.len()) {
            return None;
        }
        self.report.phrase_trim_candidates = self.report.phrase_trim_candidates.saturating_add(1);
        let normalized_pinyin = commit.composition.replace('\'', " ");
        let aligned = encode_pinyin_phrase(&normalized_pinyin)
            .ok()
            .filter(|encoded| encoded.syllable_codes.len() == characters.len())
            .map(|encoded| {
                let observed_is_canonical = effective_letter_code(&commit.keys)
                    .ok()
                    .flatten()
                    .is_some_and(|observed| observed == encoded.full_code.as_str());
                let syllable_codes = encoded
                    .syllable_codes
                    .into_iter()
                    .map(|code| code.as_str().to_owned())
                    .collect();
                (syllable_codes, observed_is_canonical)
            });
        let (syllable_codes, observed_is_canonical) = aligned
            .map(|(codes, is_canonical)| (Some(codes), Some(is_canonical)))
            .unwrap_or((None, None));
        Some(PendingPhraseTrim {
            started_ms: elapsed_ms,
            region_start: commit.document_change.start,
            remaining: characters,
            syllable_codes,
            observed_is_canonical,
            recorded_actions: commit.keys.len(),
        })
    }

    fn observe_completed_phrase_trim(&mut self, pending: PendingPhraseTrim) {
        self.report.phrase_trim_completed = self.report.phrase_trim_completed.saturating_add(1);
        let Some(syllable_codes) = pending.syllable_codes else {
            return;
        };
        self.report.phrase_trim_phonetic_aligned =
            self.report.phrase_trim_phonetic_aligned.saturating_add(1);
        if pending.observed_is_canonical == Some(false) {
            self.report.phrase_trim_noncanonical_observations = self
                .report
                .phrase_trim_noncanonical_observations
                .saturating_add(1);
        }
        let (original_index, character) = pending.remaining[0];
        let canonical_code = &syllable_codes[original_index];
        let Some(rank) = self.rank(canonical_code, character) else {
            return;
        };
        self.report.phrase_trim_public_ranked =
            self.report.phrase_trim_public_ranked.saturating_add(1);
        if rank <= SHAPE_COURSE_VISIBLE_LIMIT {
            return;
        }
        self.report.phrase_trim_public_beyond_top_10 = self
            .report
            .phrase_trim_public_beyond_top_10
            .saturating_add(1);
        let Some(outcome) = self.shape_outcome(canonical_code, character) else {
            return;
        };
        self.report.phrase_trim_hard_with_stroke_data = self
            .report
            .phrase_trim_hard_with_stroke_data
            .saturating_add(1);
        observe_visibility(
            &mut self.report.phrase_trim_hard_visible_with_any_sequence,
            outcome.visible_any,
        );
        observe_visibility(
            &mut self.report.phrase_trim_hard_visible_with_all_sequences,
            outcome.visible_all,
        );
        if let Some(prefix) = outcome.minimum_any_prefix {
            self.report.phrase_trim_best_action_comparison.observe(
                pending.recorded_actions,
                canonical_code.len() + 1 + prefix + 1,
            );
        }
        if let Some(prefix) = outcome.minimum_all_prefix {
            self.report.phrase_trim_robust_action_comparison.observe(
                pending.recorded_actions,
                canonical_code.len() + 1 + prefix + 1,
            );
        }
    }

    fn rank(&self, code: &str, target: char) -> Option<usize> {
        self.pools.rank(code, target)
    }

    fn shape_outcome(&self, code: &str, target: char) -> Option<ShapeOutcome> {
        let pool = self.pools.pool(code)?;
        let target_index = pool
            .iter()
            .position(|candidate| candidate.character == target)?;
        let target_shape = self.shapes.get(target)?;
        if target_shape.stroke_codes().is_empty() {
            return None;
        }
        let mut outcome = ShapeOutcome::default();
        for prefix_index in 0..SHAPE_COURSE_MAX_PREFIX_KEYS {
            let prefix_keys = prefix_index + 1;
            let mut any_visible = false;
            let mut all_visible = true;
            for target_code in target_shape.stroke_codes() {
                let prefix_length = prefix_keys.min(target_code.len());
                let prefix = &target_code[..prefix_length];
                let mut filtered_rank = 0usize;
                let mut target_rank = None;
                for (candidate_index, candidate) in pool.iter().enumerate() {
                    let matches = self.shapes.get(candidate.character).is_some_and(|shape| {
                        shape
                            .stroke_codes()
                            .iter()
                            .any(|candidate_code| candidate_code.starts_with(prefix))
                    });
                    if matches {
                        filtered_rank += 1;
                        if candidate_index == target_index {
                            target_rank = Some(filtered_rank);
                        }
                    }
                }
                let visible = target_rank.is_some_and(|rank| rank <= SHAPE_COURSE_VISIBLE_LIMIT);
                any_visible |= visible;
                all_visible &= visible;
            }
            outcome.visible_any[prefix_index] = any_visible;
            outcome.visible_all[prefix_index] = all_visible;
            if any_visible && outcome.minimum_any_prefix.is_none() {
                outcome.minimum_any_prefix = Some(prefix_keys);
            }
            if all_visible && outcome.minimum_all_prefix.is_none() {
                outcome.minimum_all_prefix = Some(prefix_keys);
            }
        }
        Some(outcome)
    }
}

fn apply_delete_to_pending(pending: &mut PendingPhraseTrim, change: &TextDelta) -> bool {
    let deleted = change.deleted.chars().collect::<Vec<_>>();
    let Some(relative_start) = change.start.checked_sub(pending.region_start) else {
        return false;
    };
    let Some(relative_end) = relative_start.checked_add(deleted.len()) else {
        return false;
    };
    if relative_end > pending.remaining.len()
        || pending.remaining[relative_start..relative_end]
            .iter()
            .map(|(_, character)| *character)
            .ne(deleted)
    {
        return false;
    }
    pending.remaining.drain(relative_start..relative_end);
    true
}

fn observe_visibility(
    totals: &mut [u64; SHAPE_COURSE_MAX_PREFIX_KEYS],
    observation: [bool; SHAPE_COURSE_MAX_PREFIX_KEYS],
) {
    for (total, observed) in totals.iter_mut().zip(observation) {
        *total = total.saturating_add(u64::from(observed));
    }
}

fn key_matches(key: &RawKey, predicate: fn(&RawKey) -> bool) -> bool {
    match key {
        RawKey::Shift(inner) => predicate(key) || key_matches(inner, predicate),
        _ => predicate(key),
    }
}

fn is_digit(key: &RawKey) -> bool {
    matches!(key, RawKey::Digit(_))
}

fn is_space(key: &RawKey) -> bool {
    matches!(key, RawKey::Space)
}

fn is_vertical(key: &RawKey) -> bool {
    matches!(key, RawKey::Up | RawKey::Down)
}

fn is_internal_edit(key: &RawKey) -> bool {
    matches!(key, RawKey::Backspace | RawKey::Delete)
}

fn saturating_usize(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::PrivateShapeReplayAudit;
    use crate::{
        CharacterShape, CharacterShapeIndex, CommitRecord, DeltaPositionEvidence, EventCapsuleV1,
        LexiconEntry, RawKey, RevisionRecord, TextDelta, TimedTrackerOutput, TrackerOutput,
        encode_pinyin_phrase,
    };

    fn entry(character: char, pinyin: &str, frequency: u64) -> LexiconEntry {
        let encoded = encode_pinyin_phrase(pinyin).unwrap();
        LexiconEntry {
            text: character.to_string(),
            pinyin: pinyin.to_owned(),
            code: encoded.full_code,
            syllable_codes: encoded.syllable_codes,
            frequency,
        }
    }

    fn delta(start: usize, deleted: &str, inserted: &str) -> TextDelta {
        TextDelta {
            start,
            deleted: deleted.to_owned(),
            inserted: inserted.to_owned(),
            position_evidence: DeltaPositionEvidence::UniqueText,
        }
    }

    fn hard_fixture() -> (Vec<LexiconEntry>, CharacterShapeIndex) {
        let characters = [
            '乙', '丙', '丁', '戊', '己', '庚', '辛', '壬', '癸', '子', '甲',
        ];
        let entries = characters
            .iter()
            .enumerate()
            .map(|(index, character)| entry(*character, "jia", 100 - index as u64))
            .collect::<Vec<_>>();
        let shapes =
            CharacterShapeIndex::new(characters.iter().enumerate().map(|(index, character)| {
                let code = if index < 10 { "hh" } else { "nh" };
                CharacterShape::new(*character, vec![code.to_owned()], Vec::new()).unwrap()
            }))
            .unwrap();
        (entries, shapes)
    }

    #[test]
    fn single_character_course_is_redacted_and_counts_only_canonical_action_comparisons() {
        let (entries, shapes) = hard_fixture();
        let mut audit = PrivateShapeReplayAudit::new(&entries, &shapes);
        let capsule = EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 100,
            output: TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('j'), RawKey::Letter('w'), RawKey::Space],
                keys_complete: true,
                composition: "jia".to_owned(),
                change: delta(0, "jia", "甲"),
                document_change: delta(0, "", "甲"),
            }),
        }])
        .unwrap();

        audit.observe_capsule(&capsule);
        let report = audit.report();
        assert_eq!(report.single_character_public_beyond_top_10_commits, 1);
        assert_eq!(
            report.single_character_hard_visible_with_all_sequences[0],
            1
        );
        assert_eq!(report.single_character_best_action_comparison.cases, 1);
        assert_eq!(
            report
                .single_character_best_action_comparison
                .projected_more,
            1
        );
    }

    #[test]
    fn phrase_trim_requires_a_bounded_pure_deletion_that_leaves_one_original_character() {
        let (mut entries, shapes) = hard_fixture();
        let zi = encode_pinyin_phrase("zi").unwrap();
        entries.push(LexiconEntry {
            text: "子".to_owned(),
            pinyin: "zi".to_owned(),
            code: zi.full_code,
            syllable_codes: zi.syllable_codes,
            frequency: 1,
        });
        let mut audit = PrivateShapeReplayAudit::new(&entries, &shapes);
        let capsule = EventCapsuleV1::new(vec![
            TimedTrackerOutput {
                elapsed_ms: 100,
                output: TrackerOutput::Commit(CommitRecord {
                    keys: vec![
                        RawKey::Letter('j'),
                        RawKey::Letter('w'),
                        RawKey::Letter('z'),
                        RawKey::Letter('i'),
                        RawKey::Space,
                    ],
                    keys_complete: true,
                    composition: "jia'zi".to_owned(),
                    change: delta(0, "jia'zi", "甲子"),
                    document_change: delta(0, "", "甲子"),
                }),
            },
            TimedTrackerOutput {
                elapsed_ms: 500,
                output: TrackerOutput::Revision(RevisionRecord {
                    keys: vec![RawKey::Backspace],
                    keys_complete: true,
                    change: delta(1, "子", ""),
                }),
            },
        ])
        .unwrap();

        audit.observe_capsule(&capsule);
        let report = audit.report();
        assert_eq!(report.phrase_trim_candidates, 1);
        assert_eq!(report.phrase_trim_completed, 1);
        assert_eq!(report.phrase_trim_public_beyond_top_10, 1);
        assert_eq!(report.phrase_trim_best_action_comparison.cases, 1);
        assert_eq!(report.phrase_trim_best_action_comparison.projected_fewer, 1);
    }
}
