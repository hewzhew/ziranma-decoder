//! Privacy-bounded state reconstruction for a manually armed input tracker.
//!
//! This module contains no Windows hooks and performs no I/O. A platform
//! adapter may feed it events only after independently checking the target
//! process, focused element, and password state. The state machine retains the
//! current field value in memory, but its outputs contain only the changed span.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawKey {
    Letter(char),
    Digit(u8),
    Backspace,
    Delete,
    Space,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// The Shift modifier held while the nested allowed key was pressed.
    Shift(Box<RawKey>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDelta {
    /// Character offset, not a UTF-8 byte offset.
    pub start: usize,
    pub deleted: String,
    pub inserted: String,
    pub position_evidence: DeltaPositionEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaPositionEvidence {
    UniqueText,
    Caret,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    /// Character offset in the value after the observed edit.
    pub start: usize,
    /// Character offset in the value after the observed edit.
    pub end: usize,
}

impl TextSelection {
    pub fn collapsed(offset: usize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    pub keys: Vec<RawKey>,
    pub keys_complete: bool,
    pub composition: String,
    /// The final transition from visible pinyin preedit to committed text.
    pub change: TextDelta,
    /// The net document edit from before composition to after commit.
    pub document_change: TextDelta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionRecord {
    pub keys: Vec<RawKey>,
    pub keys_complete: bool,
    pub change: TextDelta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackerOutput {
    Commit(CommitRecord),
    Revision(RevisionRecord),
}

#[derive(Debug)]
pub struct LocalInputTracker {
    target_name: String,
    current_value: String,
    active_composition: Option<String>,
    pending_keys: Vec<RawKey>,
    key_capture_enabled: bool,
    pending_keys_complete: bool,
    pending_value_baseline: Option<String>,
    composition_value_baseline: Option<String>,
}

impl LocalInputTracker {
    /// Creates an in-memory tracker for one already validated edit control.
    ///
    /// Chromium can expose the accessible name as the value of an empty
    /// ProseMirror editor. Empty-editor placeholders receive a narrow
    /// normalization; later literal user text equal to the control name is
    /// preserved.
    pub fn new(target_name: impl Into<String>, initial_value: impl Into<String>) -> Self {
        let target_name = target_name.into();
        let initial_value = initial_value.into();
        let current_value = if initial_value.trim() == target_name {
            String::new()
        } else {
            initial_value
        };
        Self {
            target_name,
            current_value,
            active_composition: None,
            pending_keys: Vec::new(),
            key_capture_enabled: true,
            pending_keys_complete: true,
            pending_value_baseline: None,
            composition_value_baseline: None,
        }
    }

    pub fn set_key_capture_enabled(&mut self, enabled: bool) {
        self.key_capture_enabled = enabled;
        self.pending_keys.clear();
        self.pending_keys_complete = enabled;
        self.pending_value_baseline = None;
        self.composition_value_baseline = None;
    }

    pub fn mark_pending_keys_incomplete(&mut self) {
        self.pending_keys_complete = false;
    }

    pub fn pending_keys_is_empty(&self) -> bool {
        self.pending_keys.is_empty()
    }

    pub fn pending_key_count(&self) -> usize {
        self.pending_keys.len()
    }

    pub fn observe_key(&mut self, key: RawKey) {
        let _ = self.observe_key_with_buffer_status(key);
    }

    /// Returns true when the bounded 128-key guard had to reset the prior
    /// pending buffer before accepting this key.
    pub fn observe_key_with_buffer_status(&mut self, key: RawKey) -> bool {
        const MAX_PENDING_KEYS: usize = 128;
        if self.pending_keys.is_empty() {
            self.pending_value_baseline = Some(self.current_value.clone());
        }
        let mut reset = false;
        if self.pending_keys.len() == MAX_PENDING_KEYS {
            self.pending_keys.clear();
            self.pending_keys_complete = false;
            self.pending_value_baseline = Some(self.current_value.clone());
            reset = true;
        }
        self.pending_keys.push(key);
        reset
    }

    pub fn observe_composition(&mut self, composition: impl Into<String>) {
        let composition = composition.into();
        if composition.is_empty() {
            // Chromium may announce an empty composition immediately before
            // the generic value event that distinguishes commit from cancel.
            // Keep the prior composition and keys until that value arrives.
            // If no value follows, the next unrelated composition still
            // bounds and replaces the abandoned session below.
        } else {
            let starts_new_composition = self.active_composition.is_none();
            if let Some(previous) = self.active_composition.as_ref() {
                let partial_hanzi_conversion = self.current_value.contains(previous)
                    && (!composition.is_ascii() || !previous.is_ascii());
                let same_session = composition.starts_with(previous)
                    || previous.starts_with(&composition)
                    || partial_hanzi_conversion;
                if !same_session {
                    let document_baseline = self
                        .composition_value_baseline
                        .clone()
                        .unwrap_or_else(|| self.current_value.clone());
                    let latest = self.pending_keys.pop();
                    self.pending_keys.clear();
                    self.pending_keys.extend(latest);
                    self.pending_keys_complete =
                        self.key_capture_enabled && !self.pending_keys.is_empty();
                    self.pending_value_baseline = Some(document_baseline.clone());
                    self.composition_value_baseline = Some(document_baseline);
                }
            }
            if starts_new_composition {
                self.composition_value_baseline = Some(
                    self.pending_value_baseline
                        .clone()
                        .unwrap_or_else(|| self.current_value.clone()),
                );
            }
            self.active_composition = Some(composition);
        }
    }

    pub fn cancel_composition(&mut self) {
        self.active_composition = None;
        self.pending_keys.clear();
        self.pending_keys_complete = self.key_capture_enabled;
        self.pending_value_baseline = None;
        self.composition_value_baseline = None;
    }

    /// Observes the value exposed by the same whitelisted edit control.
    ///
    /// Duplicate provider events are ignored. While the current composition is
    /// still visible verbatim in the value, no record is emitted. The first
    /// differing value after a composition becomes a commit record.
    pub fn observe_value(&mut self, value: impl Into<String>) -> Option<TrackerOutput> {
        self.observe_value_with_selection(value, None)
    }

    pub fn observe_value_with_selection(
        &mut self,
        value: impl Into<String>,
        selection: Option<TextSelection>,
    ) -> Option<TrackerOutput> {
        let mut value = value.into();
        if self.active_composition.is_none()
            && is_wrapped_empty_placeholder(&value, &self.target_name)
        {
            value.clear();
        }
        if value == self.current_value {
            return None;
        }

        let preedit_value = self.current_value.clone();
        let change = single_span_delta_with_selection(&preedit_value, &value, selection);
        self.current_value = value;

        if let Some(composition) = self.active_composition.clone() {
            if self.current_value.contains(&composition) {
                return None;
            }

            if is_composition_fragment(&change.inserted) {
                // Chromium can deliver the specialized composition stream
                // ahead of the generic value stream. A value that is still
                // catching up with lowercase pinyin must not be mistaken for
                // a commit merely because it does not yet contain the newest
                // complete composition.
                return None;
            }

            if change.inserted.is_empty() {
                let (keys, keys_complete) = self.take_pending_keys();
                let record = RevisionRecord {
                    keys,
                    keys_complete,
                    change,
                };
                self.active_composition = None;
                return Some(TrackerOutput::Revision(record));
            }

            let document_value = self
                .composition_value_baseline
                .take()
                .or_else(|| self.pending_value_baseline.clone())
                .unwrap_or(preedit_value);
            let document_change =
                single_span_delta_with_selection(&document_value, &self.current_value, selection);
            let (keys, keys_complete) = self.take_pending_keys();
            let record = CommitRecord {
                keys,
                keys_complete,
                composition,
                change,
                document_change,
            };
            self.active_composition = None;
            return Some(TrackerOutput::Commit(record));
        }

        let (keys, keys_complete) = self.take_pending_keys();
        Some(TrackerOutput::Revision(RevisionRecord {
            keys,
            keys_complete,
            change,
        }))
    }

    pub fn current_value(&self) -> &str {
        &self.current_value
    }

    pub fn has_active_composition(&self) -> bool {
        self.active_composition.is_some()
    }

    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    fn take_pending_keys(&mut self) -> (Vec<RawKey>, bool) {
        let keys = std::mem::take(&mut self.pending_keys);
        let complete = self.key_capture_enabled && self.pending_keys_complete;
        self.pending_keys_complete = self.key_capture_enabled;
        self.pending_value_baseline = None;
        self.composition_value_baseline = None;
        (keys, complete)
    }
}

fn is_composition_fragment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '\'')
}

fn is_wrapped_empty_placeholder(value: &str, target_name: &str) -> bool {
    value != target_name && value.trim() == target_name
}

pub fn single_span_delta(before: &str, after: &str) -> TextDelta {
    single_span_delta_with_selection(before, after, None)
}

pub fn single_span_delta_with_selection(
    before: &str,
    after: &str,
    selection: Option<TextSelection>,
) -> TextDelta {
    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();

    if before_chars == after_chars {
        return TextDelta {
            start: before_chars.len(),
            deleted: String::new(),
            inserted: String::new(),
            position_evidence: DeltaPositionEvidence::UniqueText,
        };
    }

    let prefix = before_chars
        .iter()
        .zip(&after_chars)
        .take_while(|(left, right)| left == right)
        .count();

    let remaining_before = before_chars.len() - prefix;
    let remaining_after = after_chars.len() - prefix;
    let suffix = before_chars[prefix..]
        .iter()
        .rev()
        .zip(after_chars[prefix..].iter().rev())
        .take(remaining_before.min(remaining_after))
        .take_while(|(left, right)| left == right)
        .count();

    let deleted_len = before_chars.len() - prefix - suffix;
    let inserted_len = after_chars.len() - prefix - suffix;
    let max_start = (before_chars.len() - deleted_len).min(after_chars.len() - inserted_len);
    let candidates: Vec<usize> = (0..=max_start)
        .filter(|start| {
            before_chars[..*start] == after_chars[..*start]
                && before_chars[*start + deleted_len..] == after_chars[*start + inserted_len..]
        })
        .collect();

    let (start, position_evidence) = if candidates.len() == 1 {
        (candidates[0], DeltaPositionEvidence::UniqueText)
    } else if let Some(selection) = selection
        .filter(|selection| selection.start == selection.end && selection.end <= after_chars.len())
    {
        let hinted_start = selection.end.saturating_sub(inserted_len);
        if selection.end >= inserted_len && candidates.contains(&hinted_start) {
            (hinted_start, DeltaPositionEvidence::Caret)
        } else {
            (prefix, DeltaPositionEvidence::Ambiguous)
        }
    } else {
        (prefix, DeltaPositionEvidence::Ambiguous)
    };

    TextDelta {
        start,
        deleted: before_chars[start..start + deleted_len].iter().collect(),
        inserted: after_chars[start..start + inserted_len].iter().collect(),
        position_evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitRecord, DeltaPositionEvidence, LocalInputTracker, RawKey, RevisionRecord, TextDelta,
        TextSelection, TrackerOutput, single_span_delta, single_span_delta_with_selection,
    };

    #[test]
    fn reconstructs_the_synthetic_maomao_trace_and_deduplicates_provider_events() {
        let mut tracker = LocalInputTracker::new("随心输入", "\n随心输入");
        for key in [
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Space,
        ] {
            tracker.observe_key(key);
        }

        tracker.observe_composition("m");
        assert_eq!(tracker.observe_value("m"), None);
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        tracker.observe_composition("mao'm");
        assert_eq!(tracker.observe_value("mao'm"), None);
        tracker.observe_composition("mao'mao");
        assert_eq!(tracker.observe_value("mao'mao"), None);

        let expected = TrackerOutput::Commit(CommitRecord {
            keys: vec![
                RawKey::Letter('m'),
                RawKey::Letter('k'),
                RawKey::Letter('m'),
                RawKey::Letter('k'),
                RawKey::Space,
            ],
            keys_complete: true,
            composition: "mao'mao".to_owned(),
            change: TextDelta {
                start: 0,
                deleted: "mao'mao".to_owned(),
                inserted: "猫猫".to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
            document_change: TextDelta {
                start: 0,
                deleted: String::new(),
                inserted: "猫猫".to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
        });
        assert_eq!(tracker.observe_value("猫猫"), Some(expected));
        assert_eq!(tracker.observe_value("猫猫"), None);
    }

    #[test]
    fn repeated_wrapped_empty_placeholder_is_ignored_but_literal_text_is_preserved() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        assert_eq!(tracker.observe_value("\n随心输入"), None);
        assert_eq!(tracker.current_value(), "");
        assert_eq!(
            tracker.observe_value("随心输入"),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: Vec::new(),
                keys_complete: true,
                change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "随心输入".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn deleting_the_last_character_normalizes_the_wrapped_empty_placeholder() {
        let mut tracker = LocalInputTracker::new("随心输入", "猫");
        tracker.observe_key(RawKey::Backspace);
        assert_eq!(
            tracker.observe_value("\n随心输入"),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: vec![RawKey::Backspace],
                keys_complete: true,
                change: TextDelta {
                    start: 0,
                    deleted: "猫".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
        assert_eq!(tracker.current_value(), "");

        for key in [RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space] {
            tracker.observe_key(key);
        }
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        assert_eq!(
            tracker.observe_value("猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space,],
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn commit_output_contains_only_the_changed_span_not_the_surrounding_text() {
        let mut tracker = LocalInputTracker::new("随心输入", "保留前文");
        tracker.observe_key(RawKey::Letter('m'));
        tracker.observe_key(RawKey::Letter('k'));
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("保留前文mao"), None);

        assert_eq!(
            tracker.observe_value("保留前文猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Letter('k')],
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 4,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 4,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn cancellation_emits_a_bounded_revision_with_the_attempted_keys() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        tracker.observe_key(RawKey::Letter('m'));
        tracker.observe_key(RawKey::Escape);
        tracker.observe_composition("m");
        assert_eq!(tracker.observe_value("m"), None);
        tracker.observe_composition("");
        assert_eq!(
            tracker.observe_value(""),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Escape],
                keys_complete: true,
                change: TextDelta {
                    start: 0,
                    deleted: "m".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );

        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        assert_eq!(
            tracker.observe_value("猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: Vec::new(),
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn ordinary_edits_are_reported_as_bounded_revisions() {
        let mut tracker = LocalInputTracker::new("随心输入", "猫猫");
        tracker.observe_key(RawKey::Backspace);
        assert_eq!(
            tracker.observe_value("猫"),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: vec![RawKey::Backspace],
                keys_complete: true,
                change: TextDelta {
                    start: 1,
                    deleted: "猫".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::Ambiguous,
                },
            }))
        );

        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("猫mao"), None);
        assert_eq!(
            tracker.observe_value("猫猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: Vec::new(),
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 1,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 1,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::Ambiguous,
                },
            }))
        );
    }

    #[test]
    fn delta_offsets_count_characters_instead_of_utf8_bytes() {
        assert_eq!(
            single_span_delta("甲猫乙", "甲麻烦乙"),
            TextDelta {
                start: 1,
                deleted: "猫".to_owned(),
                inserted: "麻烦".to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            }
        );
    }

    #[test]
    fn unrelated_composition_drops_keys_left_by_an_abandoned_session() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        tracker.observe_key(RawKey::Letter('m'));
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        tracker.observe_key(RawKey::Letter('n'));
        tracker.observe_composition("ni");
        assert_eq!(tracker.observe_value("ni"), None);
        assert_eq!(
            tracker.observe_value("你"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('n')],
                keys_complete: true,
                composition: "ni".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "ni".to_owned(),
                    inserted: "你".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "你".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn composition_internal_typo_preserves_backspace_and_retyped_keys() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        for key in [
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Letter('x'),
            RawKey::Backspace,
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Space,
        ] {
            tracker.observe_key(key);
        }

        tracker.observe_composition("mao'x");
        assert_eq!(tracker.observe_value("mao'x"), None);
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        tracker.observe_composition("mao'mao");
        assert_eq!(tracker.observe_value("mao'mao"), None);

        assert_eq!(
            tracker.observe_value("猫猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Letter('x'),
                    RawKey::Backspace,
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Space,
                ],
                keys_complete: true,
                composition: "mao'mao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "mao'mao".to_owned(),
                    inserted: "猫猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "猫猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn committed_text_deletion_and_retyping_remain_separate_ordered_records() {
        let mut tracker = LocalInputTracker::new("随心输入", "猫猫");
        tracker.observe_key(RawKey::Left);
        tracker.observe_key(RawKey::Delete);
        assert_eq!(
            tracker.observe_value("猫"),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: vec![RawKey::Left, RawKey::Delete],
                keys_complete: true,
                change: TextDelta {
                    start: 1,
                    deleted: "猫".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::Ambiguous,
                },
            }))
        );

        tracker.observe_key(RawKey::Letter('m'));
        tracker.observe_key(RawKey::Letter('k'));
        tracker.observe_key(RawKey::Space);
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("猫mao"), None);
        assert_eq!(
            tracker.observe_value("猫猫"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space,],
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 1,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 1,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::Ambiguous,
                },
            }))
        );
    }

    #[test]
    fn caret_disambiguates_a_deletion_inside_repeated_text() {
        assert_eq!(
            single_span_delta_with_selection(
                "猫猫猫猫猫猫猫猫",
                "猫猫猫猫猫猫猫",
                Some(TextSelection::collapsed(3)),
            ),
            TextDelta {
                start: 3,
                deleted: "猫".to_owned(),
                inserted: String::new(),
                position_evidence: DeltaPositionEvidence::Caret,
            }
        );
    }

    #[test]
    fn a_detected_key_prefix_gap_is_carried_to_the_next_record_only() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        tracker.mark_pending_keys_incomplete();
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("mao"), None);
        let Some(TrackerOutput::Commit(first)) = tracker.observe_value("猫") else {
            panic!("expected first commit");
        };
        assert!(!first.keys_complete);

        tracker.observe_key(RawKey::Backspace);
        let Some(TrackerOutput::Revision(second)) = tracker.observe_value("") else {
            panic!("expected second revision");
        };
        assert!(second.keys_complete);
    }

    #[test]
    fn shifted_selection_and_delete_are_preserved_with_caret_disambiguation() {
        let mut tracker = LocalInputTracker::new("随心输入", "猫猫猫猫");
        tracker.observe_key(RawKey::Shift(Box::new(RawKey::Left)));
        tracker.observe_key(RawKey::Shift(Box::new(RawKey::Left)));
        tracker.observe_key(RawKey::Delete);

        assert_eq!(
            tracker.observe_value_with_selection("猫猫", Some(TextSelection::collapsed(2)),),
            Some(TrackerOutput::Revision(RevisionRecord {
                keys: vec![
                    RawKey::Shift(Box::new(RawKey::Left)),
                    RawKey::Shift(Box::new(RawKey::Left)),
                    RawKey::Delete,
                ],
                keys_complete: true,
                change: TextDelta {
                    start: 2,
                    deleted: "猫猫".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::Caret,
                },
            }))
        );
    }

    #[test]
    fn lagging_pinyin_value_fragments_do_not_end_a_middle_composition() {
        let mut tracker = LocalInputTracker::new("随心输入", "猫猫猫猫猫猫");
        for key in [
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Letter('m'),
            RawKey::Letter('k'),
            RawKey::Space,
        ] {
            tracker.observe_key(key);
        }

        tracker.observe_composition("m");
        assert_eq!(tracker.observe_value("猫猫猫m猫猫猫"), None);
        tracker.observe_composition("mao");
        assert_eq!(tracker.observe_value("猫猫猫mao猫猫猫"), None);

        // The specialized event is now two generic value fragments ahead.
        tracker.observe_composition("mao'mao");
        assert_eq!(tracker.observe_value("猫猫猫mao'm猫猫猫"), None);
        assert_eq!(tracker.observe_value("猫猫猫mao'mao猫猫猫"), None);

        assert_eq!(
            tracker.observe_value_with_selection(
                "猫猫猫猫猫猫猫猫",
                Some(TextSelection::collapsed(5)),
            ),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Letter('m'),
                    RawKey::Letter('k'),
                    RawKey::Space,
                ],
                keys_complete: true,
                composition: "mao'mao".to_owned(),
                change: TextDelta {
                    start: 3,
                    deleted: "mao'mao".to_owned(),
                    inserted: "猫猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 3,
                    deleted: String::new(),
                    inserted: "猫猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::Caret,
                },
            }))
        );
    }

    #[test]
    fn commit_preserves_preedit_transition_and_direct_document_replacement() {
        let mut tracker = LocalInputTracker::new("随心输入", "甲错乙");
        for key in [RawKey::Letter('z'), RawKey::Letter('d'), RawKey::Space] {
            tracker.observe_key(key);
        }

        tracker.observe_composition("z");
        assert_eq!(tracker.observe_value("甲z乙"), None);
        tracker.observe_composition("zai");
        assert_eq!(tracker.observe_value("甲zai乙"), None);

        assert_eq!(
            tracker.observe_value_with_selection("甲在乙", Some(TextSelection::collapsed(2))),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('z'), RawKey::Letter('d'), RawKey::Space,],
                keys_complete: true,
                composition: "zai".to_owned(),
                change: TextDelta {
                    start: 1,
                    deleted: "zai".to_owned(),
                    inserted: "在".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 1,
                    deleted: "错".to_owned(),
                    inserted: "在".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn partial_hanzi_conversion_stays_in_the_same_composition_session() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        let stages = [
            (RawKey::Letter('j'), "j"),
            (RawKey::Letter('i'), "jia"),
            (RawKey::Letter('c'), "jia'c"),
            (RawKey::Letter('u'), "jia'cuo"),
            (RawKey::Letter('y'), "jia'cuo'y"),
            (RawKey::Letter('i'), "jia'cuo'yi"),
            (RawKey::Digit(5), "甲cuo'yi"),
            (RawKey::Space, "甲错yi"),
        ];
        for (key, composition) in stages {
            tracker.observe_key(key);
            tracker.observe_composition(composition);
            assert_eq!(tracker.observe_value(composition), None);
        }

        assert_eq!(
            tracker.observe_value("甲错乙"),
            Some(TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('j'),
                    RawKey::Letter('i'),
                    RawKey::Letter('c'),
                    RawKey::Letter('u'),
                    RawKey::Letter('y'),
                    RawKey::Letter('i'),
                    RawKey::Digit(5),
                    RawKey::Space,
                ],
                keys_complete: true,
                composition: "甲错yi".to_owned(),
                change: TextDelta {
                    start: 2,
                    deleted: "yi".to_owned(),
                    inserted: "乙".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "甲错乙".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }))
        );
    }

    #[test]
    fn bounded_key_buffer_reports_only_the_actual_reset() {
        let mut tracker = LocalInputTracker::new("随心输入", "");
        for _ in 0..128 {
            assert!(!tracker.observe_key_with_buffer_status(RawKey::Letter('a')));
        }
        assert_eq!(tracker.pending_key_count(), 128);
        assert!(tracker.observe_key_with_buffer_status(RawKey::Letter('b')));
        assert_eq!(tracker.pending_key_count(), 1);
    }
}
