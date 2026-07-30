//! Research-only, memory-only state for retractable selection evidence.
//!
//! A selected candidate first remains pending with its document span. An
//! overlapping edit retracts that pending evidence, while an edit entirely
//! before it shifts the span. Only an explicit confirmation boundary moves
//! pending selections into bounded positive counts.
//!
//! This module performs no I/O, exposes no ranking hook, and is not connected
//! to the TSF host. It deliberately models no negative or error labels.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

pub const DEFAULT_MAX_PENDING_SELECTIONS: usize = 128;
pub const DEFAULT_MAX_CONFIRMED_SELECTIONS: usize = 8_192;
pub const DEFAULT_MAX_RECENT_CONFIRMED_SELECTIONS: usize = 128;
pub const DEFAULT_MAX_MEDIUM_CONFIRMED_SELECTIONS: usize = 1_024;
pub const DEFAULT_MAX_LONG_CONFIRMED_SELECTIONS: usize = DEFAULT_MAX_CONFIRMED_SELECTIONS
    - DEFAULT_MAX_RECENT_CONFIRMED_SELECTIONS
    - DEFAULT_MAX_MEDIUM_CONFIRMED_SELECTIONS;
pub const MAX_PENDING_SELECTION_LIMIT: usize = 4_096;
pub const MAX_CONFIRMED_SELECTION_LIMIT: usize = 65_536;

const MAX_SELECTION_CODE_BYTES: usize = 64;
const MAX_SELECTION_TEXT_CHARACTERS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingSelectionLimits {
    max_pending: usize,
    max_recent_confirmed: usize,
    max_medium_confirmed: usize,
    max_long_confirmed: usize,
}

impl PendingSelectionLimits {
    pub fn new(max_pending: usize, max_confirmed: usize) -> Result<Self, PendingSelectionError> {
        Self::tiered(max_pending, max_confirmed, 0, 0)
    }

    pub fn tiered(
        max_pending: usize,
        max_recent_confirmed: usize,
        max_medium_confirmed: usize,
        max_long_confirmed: usize,
    ) -> Result<Self, PendingSelectionError> {
        let max_confirmed = max_recent_confirmed
            .checked_add(max_medium_confirmed)
            .and_then(|total| total.checked_add(max_long_confirmed))
            .ok_or(PendingSelectionError::InvalidLimits)?;
        if max_pending == 0
            || max_pending > MAX_PENDING_SELECTION_LIMIT
            || max_confirmed == 0
            || max_confirmed > MAX_CONFIRMED_SELECTION_LIMIT
        {
            return Err(PendingSelectionError::InvalidLimits);
        }
        Ok(Self {
            max_pending,
            max_recent_confirmed,
            max_medium_confirmed,
            max_long_confirmed,
        })
    }

    pub fn max_pending(self) -> usize {
        self.max_pending
    }

    pub fn max_confirmed(self) -> usize {
        self.max_recent_confirmed + self.max_medium_confirmed + self.max_long_confirmed
    }

    pub fn max_recent_confirmed(self) -> usize {
        self.max_recent_confirmed
    }

    pub fn max_medium_confirmed(self) -> usize {
        self.max_medium_confirmed
    }

    pub fn max_long_confirmed(self) -> usize {
        self.max_long_confirmed
    }
}

impl Default for PendingSelectionLimits {
    fn default() -> Self {
        Self {
            max_pending: DEFAULT_MAX_PENDING_SELECTIONS,
            max_recent_confirmed: DEFAULT_MAX_RECENT_CONFIRMED_SELECTIONS,
            max_medium_confirmed: DEFAULT_MAX_MEDIUM_CONFIRMED_SELECTIONS,
            max_long_confirmed: DEFAULT_MAX_LONG_CONFIRMED_SELECTIONS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingSelectionError {
    InvalidLimits,
    InvalidCode,
    InvalidText,
    PositionOverflow,
}

impl fmt::Display for PendingSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidLimits => "pending-selection limits are outside the supported range",
            Self::InvalidCode => "selection code must be 1-64 lowercase ASCII letters",
            Self::InvalidText => "selection text must contain 1-128 characters",
            Self::PositionOverflow => "document position exceeds the supported character range",
        };
        formatter.write_str(message)
    }
}

impl Error for PendingSelectionError {}

/// A document edit expressed in Unicode scalar-value positions and lengths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingSelectionEdit {
    pub start: usize,
    pub deleted_chars: usize,
    pub inserted_chars: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingObservationOutcome {
    pub evicted_pending: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingEditOutcome {
    pub shifted_pending: usize,
    pub retracted_pending: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConfirmedSelectionTierCounts {
    pub recent: usize,
    pub medium: usize,
    pub long: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmedSelectionTier {
    Recent,
    Medium,
    Long,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfirmedSelectionEvidence {
    pub confirmations: u64,
    pub tier: ConfirmedSelectionTier,
    pub last_generation: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingConfirmationOutcome {
    pub confirmed_pending: usize,
    pub new_confirmed_entries: usize,
    pub updated_confirmed_entries: usize,
    pub moved_to_medium: usize,
    pub moved_to_long: usize,
    pub evicted_confirmed_entries: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingForgetOutcome {
    pub removed_pending: usize,
    pub removed_confirmed: usize,
}

/// Bounded positive selection evidence with retractable document spans.
///
/// The contained codes and text have no iterator, serializer, or
/// content-bearing `Debug` implementation.
pub struct PendingSelectionMemory {
    limits: PendingSelectionLimits,
    pending: VecDeque<PendingSelection>,
    recent_confirmed: VecDeque<ConfirmedSelection>,
    medium_confirmed: VecDeque<ConfirmedSelection>,
    long_confirmed: VecDeque<ConfirmedSelection>,
    generation: u64,
}

struct PendingSelection {
    code: String,
    text: String,
    start: usize,
    char_len: usize,
}

struct ConfirmedSelection {
    code: String,
    text: String,
    confirmations: u64,
    last_generation: u64,
}

impl PendingSelectionMemory {
    pub fn new() -> Self {
        Self::with_limits(PendingSelectionLimits::default())
            .expect("default pending-selection limits must remain valid")
    }

    pub fn with_limits(limits: PendingSelectionLimits) -> Result<Self, PendingSelectionError> {
        let limits = PendingSelectionLimits::tiered(
            limits.max_pending,
            limits.max_recent_confirmed,
            limits.max_medium_confirmed,
            limits.max_long_confirmed,
        )?;
        Ok(Self {
            limits,
            pending: VecDeque::new(),
            recent_confirmed: VecDeque::new(),
            medium_confirmed: VecDeque::new(),
            long_confirmed: VecDeque::new(),
            generation: 0,
        })
    }

    pub fn limits(&self) -> PendingSelectionLimits {
        self.limits
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn confirmed_len(&self) -> usize {
        let counts = self.confirmed_tier_counts();
        counts.recent + counts.medium + counts.long
    }

    pub fn confirmed_tier_counts(&self) -> ConfirmedSelectionTierCounts {
        ConfirmedSelectionTierCounts {
            recent: self.recent_confirmed.len(),
            medium: self.medium_confirmed.len(),
            long: self.long_confirmed.len(),
        }
    }

    pub fn observe_commit(
        &mut self,
        code: &str,
        text: &str,
        start: usize,
    ) -> Result<PendingObservationOutcome, PendingSelectionError> {
        validate_identity(code, text)?;
        let char_len = text.chars().count();
        start
            .checked_add(char_len)
            .ok_or(PendingSelectionError::PositionOverflow)?;

        let evicted_pending = usize::from(self.pending.len() == self.limits.max_pending);
        if evicted_pending != 0 {
            self.pending.pop_front();
        }
        self.pending.push_back(PendingSelection {
            code: code.to_owned(),
            text: text.to_owned(),
            start,
            char_len,
        });
        Ok(PendingObservationOutcome { evicted_pending })
    }

    pub fn pending_count(&self, code: &str, text: &str) -> Result<usize, PendingSelectionError> {
        validate_identity(code, text)?;
        Ok(self
            .pending
            .iter()
            .filter(|selection| selection.code == code && selection.text == text)
            .count())
    }

    pub fn confirmed_count(&self, code: &str, text: &str) -> Result<u64, PendingSelectionError> {
        Ok(self
            .confirmed_evidence(code, text)?
            .map_or(0, |evidence| evidence.confirmations))
    }

    pub fn confirmed_tier(
        &self,
        code: &str,
        text: &str,
    ) -> Result<Option<ConfirmedSelectionTier>, PendingSelectionError> {
        Ok(self
            .confirmed_evidence(code, text)?
            .map(|evidence| evidence.tier))
    }

    pub fn confirmed_evidence(
        &self,
        code: &str,
        text: &str,
    ) -> Result<Option<ConfirmedSelectionEvidence>, PendingSelectionError> {
        validate_identity(code, text)?;
        if let Some(selection) = find_selection(&self.recent_confirmed, code, text) {
            Ok(Some(ConfirmedSelectionEvidence {
                confirmations: selection.confirmations,
                tier: ConfirmedSelectionTier::Recent,
                last_generation: selection.last_generation,
            }))
        } else if let Some(selection) = find_selection(&self.medium_confirmed, code, text) {
            Ok(Some(ConfirmedSelectionEvidence {
                confirmations: selection.confirmations,
                tier: ConfirmedSelectionTier::Medium,
                last_generation: selection.last_generation,
            }))
        } else if let Some(selection) = find_selection(&self.long_confirmed, code, text) {
            Ok(Some(ConfirmedSelectionEvidence {
                confirmations: selection.confirmations,
                tier: ConfirmedSelectionTier::Long,
                last_generation: selection.last_generation,
            }))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn visit_confirmed_for_code<'a>(
        &'a self,
        code: &str,
        mut visitor: impl FnMut(&'a str, ConfirmedSelectionEvidence),
    ) -> Result<(), PendingSelectionError> {
        validate_code(code)?;
        for (tier, selections) in [
            (ConfirmedSelectionTier::Recent, &self.recent_confirmed),
            (ConfirmedSelectionTier::Medium, &self.medium_confirmed),
            (ConfirmedSelectionTier::Long, &self.long_confirmed),
        ] {
            for selection in selections {
                if selection.code == code {
                    visitor(
                        &selection.text,
                        ConfirmedSelectionEvidence {
                            confirmations: selection.confirmations,
                            tier,
                            last_generation: selection.last_generation,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    pub fn apply_edit(
        &mut self,
        edit: PendingSelectionEdit,
    ) -> Result<PendingEditOutcome, PendingSelectionError> {
        let edit_end = edit
            .start
            .checked_add(edit.deleted_chars)
            .ok_or(PendingSelectionError::PositionOverflow)?;

        for selection in &self.pending {
            if edit_precedes_selection(edit, edit_end, selection) {
                shifted_start(selection.start, edit)?
                    .checked_add(selection.char_len)
                    .ok_or(PendingSelectionError::PositionOverflow)?;
            }
        }

        let mut outcome = PendingEditOutcome::default();
        let mut retained = VecDeque::with_capacity(self.pending.len());
        while let Some(mut selection) = self.pending.pop_front() {
            if edit_overlaps_selection(edit, edit_end, &selection) {
                outcome.retracted_pending = outcome.retracted_pending.saturating_add(1);
                continue;
            }
            if edit_precedes_selection(edit, edit_end, &selection) {
                selection.start = shifted_start(selection.start, edit)?;
                outcome.shifted_pending = outcome.shifted_pending.saturating_add(1);
            }
            retained.push_back(selection);
        }
        self.pending = retained;
        Ok(outcome)
    }

    pub fn confirm_pending(&mut self) -> PendingConfirmationOutcome {
        let mut outcome = PendingConfirmationOutcome::default();
        while let Some(selection) = self.pending.pop_front() {
            outcome.confirmed_pending = outcome.confirmed_pending.saturating_add(1);
            self.generation = self.generation.saturating_add(1);

            let mut confirmed = self.take_confirmed_selection(&selection.code, &selection.text);
            if let Some(confirmed) = confirmed.as_mut() {
                confirmed.confirmations = confirmed.confirmations.saturating_add(1);
                confirmed.last_generation = self.generation;
                outcome.updated_confirmed_entries =
                    outcome.updated_confirmed_entries.saturating_add(1);
            } else {
                confirmed = Some(ConfirmedSelection {
                    code: selection.code,
                    text: selection.text,
                    confirmations: 1,
                    last_generation: self.generation,
                });
                outcome.new_confirmed_entries = outcome.new_confirmed_entries.saturating_add(1);
            }

            self.recent_confirmed
                .push_back(confirmed.expect("confirmed selection was created or recovered"));
            self.rebalance_confirmed_tiers(&mut outcome);
        }
        outcome
    }

    pub fn forget(
        &mut self,
        code: &str,
        text: &str,
    ) -> Result<PendingForgetOutcome, PendingSelectionError> {
        validate_identity(code, text)?;

        let pending_before = self.pending.len();
        self.pending
            .retain(|selection| selection.code != code || selection.text != text);
        let confirmed_before = self.confirmed_len();
        self.recent_confirmed
            .retain(|selection| selection.code != code || selection.text != text);
        self.medium_confirmed
            .retain(|selection| selection.code != code || selection.text != text);
        self.long_confirmed
            .retain(|selection| selection.code != code || selection.text != text);

        Ok(PendingForgetOutcome {
            removed_pending: pending_before.saturating_sub(self.pending.len()),
            removed_confirmed: confirmed_before.saturating_sub(self.confirmed_len()),
        })
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.recent_confirmed.clear();
        self.medium_confirmed.clear();
        self.long_confirmed.clear();
        self.generation = 0;
    }

    fn take_confirmed_selection(&mut self, code: &str, text: &str) -> Option<ConfirmedSelection> {
        take_selection(&mut self.recent_confirmed, code, text)
            .or_else(|| take_selection(&mut self.medium_confirmed, code, text))
            .or_else(|| take_selection(&mut self.long_confirmed, code, text))
    }

    fn rebalance_confirmed_tiers(&mut self, outcome: &mut PendingConfirmationOutcome) {
        while self.recent_confirmed.len() > self.limits.max_recent_confirmed {
            let selection = self
                .recent_confirmed
                .pop_front()
                .expect("an overflowing recent tier cannot be empty");
            if self.limits.max_medium_confirmed != 0 {
                self.medium_confirmed.push_back(selection);
                outcome.moved_to_medium = outcome.moved_to_medium.saturating_add(1);
            } else if self.limits.max_long_confirmed != 0 {
                self.long_confirmed.push_back(selection);
                outcome.moved_to_long = outcome.moved_to_long.saturating_add(1);
            } else {
                outcome.evicted_confirmed_entries =
                    outcome.evicted_confirmed_entries.saturating_add(1);
            }
        }

        while self.medium_confirmed.len() > self.limits.max_medium_confirmed {
            let selection = self
                .medium_confirmed
                .pop_front()
                .expect("an overflowing medium tier cannot be empty");
            if self.limits.max_long_confirmed != 0 {
                self.long_confirmed.push_back(selection);
                outcome.moved_to_long = outcome.moved_to_long.saturating_add(1);
            } else {
                outcome.evicted_confirmed_entries =
                    outcome.evicted_confirmed_entries.saturating_add(1);
            }
        }

        while self.long_confirmed.len() > self.limits.max_long_confirmed {
            self.long_confirmed.pop_front();
            outcome.evicted_confirmed_entries = outcome.evicted_confirmed_entries.saturating_add(1);
        }
    }
}

impl Default for PendingSelectionMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PendingSelectionMemory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingSelectionMemory")
            .field("debug_contains_text", &false)
            .field("limits", &self.limits)
            .field("pending", &self.pending.len())
            .field("confirmed", &self.confirmed_len())
            .field("confirmed_tiers", &self.confirmed_tier_counts())
            .field("generation", &self.generation)
            .finish()
    }
}

fn find_selection<'a>(
    selections: &'a VecDeque<ConfirmedSelection>,
    code: &str,
    text: &str,
) -> Option<&'a ConfirmedSelection> {
    selections
        .iter()
        .find(|selection| selection.code == code && selection.text == text)
}

fn take_selection(
    selections: &mut VecDeque<ConfirmedSelection>,
    code: &str,
    text: &str,
) -> Option<ConfirmedSelection> {
    selections
        .iter()
        .position(|selection| selection.code == code && selection.text == text)
        .and_then(|index| selections.remove(index))
}

fn validate_identity(code: &str, text: &str) -> Result<(), PendingSelectionError> {
    validate_code(code)?;
    validate_selection_text(text)
}

fn validate_code(code: &str) -> Result<(), PendingSelectionError> {
    if code.is_empty()
        || code.len() > MAX_SELECTION_CODE_BYTES
        || !code.as_bytes().iter().all(u8::is_ascii_lowercase)
    {
        return Err(PendingSelectionError::InvalidCode);
    }
    Ok(())
}

pub(crate) fn validate_selection_text(text: &str) -> Result<(), PendingSelectionError> {
    let text_chars = text.chars().count();
    if text_chars == 0 || text_chars > MAX_SELECTION_TEXT_CHARACTERS {
        return Err(PendingSelectionError::InvalidText);
    }
    Ok(())
}

fn edit_overlaps_selection(
    edit: PendingSelectionEdit,
    edit_end: usize,
    selection: &PendingSelection,
) -> bool {
    let selection_end = selection.start + selection.char_len;
    if edit.deleted_chars != 0 {
        edit.start < selection_end && edit_end > selection.start
    } else {
        edit.start > selection.start && edit.start < selection_end
    }
}

fn edit_precedes_selection(
    edit: PendingSelectionEdit,
    edit_end: usize,
    selection: &PendingSelection,
) -> bool {
    if edit.deleted_chars == 0 {
        edit.start <= selection.start
    } else {
        edit_end <= selection.start
    }
}

fn shifted_start(
    selection_start: usize,
    edit: PendingSelectionEdit,
) -> Result<usize, PendingSelectionError> {
    selection_start
        .checked_sub(edit.deleted_chars)
        .and_then(|start| start.checked_add(edit.inserted_chars))
        .ok_or(PendingSelectionError::PositionOverflow)
}

#[cfg(test)]
mod tests {
    use super::{
        ConfirmedSelectionTier, MAX_CONFIRMED_SELECTION_LIMIT, PendingSelectionEdit,
        PendingSelectionError, PendingSelectionLimits, PendingSelectionMemory,
    };

    fn limited(max_pending: usize, max_confirmed: usize) -> PendingSelectionMemory {
        PendingSelectionMemory::with_limits(
            PendingSelectionLimits::new(max_pending, max_confirmed).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn commit_stays_pending_until_an_explicit_confirmation_boundary() {
        let mut memory = PendingSelectionMemory::new();

        memory.observe_commit("aa", "甲", 4).unwrap();

        assert_eq!(memory.pending_count("aa", "甲"), Ok(1));
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(0));
        assert_eq!(
            memory.confirm_pending(),
            super::PendingConfirmationOutcome {
                confirmed_pending: 1,
                new_confirmed_entries: 1,
                updated_confirmed_entries: 0,
                moved_to_medium: 0,
                moved_to_long: 0,
                evicted_confirmed_entries: 0,
            }
        );
        assert_eq!(memory.pending_count("aa", "甲"), Ok(0));
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(1));
    }

    #[test]
    fn overlapping_deletion_and_replacement_retract_pending_evidence() {
        let mut deletion = PendingSelectionMemory::new();
        deletion.observe_commit("aa", "甲乙", 4).unwrap();
        assert_eq!(
            deletion
                .apply_edit(PendingSelectionEdit {
                    start: 5,
                    deleted_chars: 1,
                    inserted_chars: 0,
                })
                .unwrap(),
            super::PendingEditOutcome {
                shifted_pending: 0,
                retracted_pending: 1,
            }
        );

        let mut replacement = PendingSelectionMemory::new();
        replacement.observe_commit("aa", "甲乙", 4).unwrap();
        assert_eq!(
            replacement
                .apply_edit(PendingSelectionEdit {
                    start: 4,
                    deleted_chars: 1,
                    inserted_chars: 1,
                })
                .unwrap()
                .retracted_pending,
            1
        );
    }

    #[test]
    fn insertion_before_pending_span_shifts_it_before_later_retraction() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲乙", 4).unwrap();

        assert_eq!(
            memory
                .apply_edit(PendingSelectionEdit {
                    start: 2,
                    deleted_chars: 0,
                    inserted_chars: 3,
                })
                .unwrap()
                .shifted_pending,
            1
        );
        assert_eq!(memory.pending[0].start, 7);
        assert_eq!(
            memory
                .apply_edit(PendingSelectionEdit {
                    start: 7,
                    deleted_chars: 1,
                    inserted_chars: 0,
                })
                .unwrap()
                .retracted_pending,
            1
        );
    }

    #[test]
    fn deletion_before_pending_span_shifts_it_left() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 6).unwrap();

        let outcome = memory
            .apply_edit(PendingSelectionEdit {
                start: 1,
                deleted_chars: 3,
                inserted_chars: 0,
            })
            .unwrap();

        assert_eq!(outcome.shifted_pending, 1);
        assert_eq!(memory.pending[0].start, 3);
    }

    #[test]
    fn edits_after_pending_span_leave_it_unchanged() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 2).unwrap();

        assert_eq!(
            memory
                .apply_edit(PendingSelectionEdit {
                    start: 3,
                    deleted_chars: 0,
                    inserted_chars: 4,
                })
                .unwrap(),
            super::PendingEditOutcome::default()
        );
        assert_eq!(memory.pending_len(), 1);
        assert_eq!(memory.pending[0].start, 2);
    }

    #[test]
    fn insertion_boundaries_do_not_retract_but_interior_insertion_does() {
        let mut at_start = PendingSelectionMemory::new();
        at_start.observe_commit("aa", "甲乙", 4).unwrap();
        assert_eq!(
            at_start
                .apply_edit(PendingSelectionEdit {
                    start: 4,
                    deleted_chars: 0,
                    inserted_chars: 1,
                })
                .unwrap()
                .shifted_pending,
            1
        );

        let mut at_end = PendingSelectionMemory::new();
        at_end.observe_commit("aa", "甲乙", 4).unwrap();
        assert_eq!(
            at_end
                .apply_edit(PendingSelectionEdit {
                    start: 6,
                    deleted_chars: 0,
                    inserted_chars: 1,
                })
                .unwrap(),
            super::PendingEditOutcome::default()
        );

        let mut inside = PendingSelectionMemory::new();
        inside.observe_commit("aa", "甲乙", 4).unwrap();
        assert_eq!(
            inside
                .apply_edit(PendingSelectionEdit {
                    start: 5,
                    deleted_chars: 0,
                    inserted_chars: 1,
                })
                .unwrap()
                .retracted_pending,
            1
        );
    }

    #[test]
    fn deletion_then_same_text_commit_creates_only_fresh_positive_evidence() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 0).unwrap();
        memory
            .apply_edit(PendingSelectionEdit {
                start: 0,
                deleted_chars: 1,
                inserted_chars: 0,
            })
            .unwrap();
        memory.observe_commit("aa", "甲", 0).unwrap();

        assert_eq!(memory.pending_count("aa", "甲"), Ok(1));
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(0));
        memory.confirm_pending();
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(1));
    }

    #[test]
    fn aliases_are_counted_separately_and_repeated_confirmations_accumulate() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 0).unwrap();
        memory.observe_commit("ab", "甲", 1).unwrap();
        memory.confirm_pending();
        memory.observe_commit("aa", "甲", 2).unwrap();
        memory.confirm_pending();

        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(2));
        assert_eq!(memory.confirmed_count("ab", "甲"), Ok(1));
        assert_eq!(memory.confirmed_len(), 2);
    }

    #[test]
    fn confirmed_capacity_evicts_the_least_recent_entry_deterministically() {
        let mut memory = limited(4, 2);
        memory.observe_commit("aa", "甲", 0).unwrap();
        memory.observe_commit("ab", "乙", 1).unwrap();
        memory.confirm_pending();
        memory.observe_commit("aa", "甲", 2).unwrap();
        memory.confirm_pending();
        memory.observe_commit("ac", "丙", 3).unwrap();

        let outcome = memory.confirm_pending();

        assert_eq!(outcome.evicted_confirmed_entries, 1);
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(2));
        assert_eq!(memory.confirmed_count("ab", "乙"), Ok(0));
        assert_eq!(memory.confirmed_count("ac", "丙"), Ok(1));
    }

    #[test]
    fn tiered_history_moves_oldest_entries_before_final_eviction() {
        let mut memory = PendingSelectionMemory::with_limits(
            PendingSelectionLimits::tiered(8, 2, 2, 2).unwrap(),
        )
        .unwrap();
        for (code, text, start) in [
            ("aa", "甲", 0),
            ("ab", "乙", 1),
            ("ac", "丙", 2),
            ("ad", "丁", 3),
            ("ae", "戊", 4),
            ("af", "己", 5),
            ("ag", "庚", 6),
        ] {
            memory.observe_commit(code, text, start).unwrap();
        }

        let outcome = memory.confirm_pending();

        assert_eq!(outcome.moved_to_medium, 5);
        assert_eq!(outcome.moved_to_long, 3);
        assert_eq!(outcome.evicted_confirmed_entries, 1);
        assert_eq!(
            memory.confirmed_tier_counts(),
            super::ConfirmedSelectionTierCounts {
                recent: 2,
                medium: 2,
                long: 2,
            }
        );
        assert_eq!(memory.confirmed_tier("aa", "甲"), Ok(None));
        assert_eq!(
            memory.confirmed_tier("ab", "乙"),
            Ok(Some(ConfirmedSelectionTier::Long))
        );
        assert_eq!(
            memory.confirmed_tier("ad", "丁"),
            Ok(Some(ConfirmedSelectionTier::Medium))
        );
        assert_eq!(
            memory.confirmed_tier("ag", "庚"),
            Ok(Some(ConfirmedSelectionTier::Recent))
        );
    }

    #[test]
    fn reconfirming_old_evidence_returns_it_to_recent_without_duplication() {
        let mut memory = PendingSelectionMemory::with_limits(
            PendingSelectionLimits::tiered(4, 1, 1, 1).unwrap(),
        )
        .unwrap();
        for (code, text, start) in [("aa", "甲", 0), ("ab", "乙", 1), ("ac", "丙", 2)] {
            memory.observe_commit(code, text, start).unwrap();
            memory.confirm_pending();
        }
        assert_eq!(
            memory.confirmed_tier("aa", "甲"),
            Ok(Some(ConfirmedSelectionTier::Long))
        );

        memory.observe_commit("aa", "甲", 3).unwrap();
        let outcome = memory.confirm_pending();

        assert_eq!(outcome.updated_confirmed_entries, 1);
        assert_eq!(outcome.new_confirmed_entries, 0);
        assert_eq!(outcome.moved_to_medium, 1);
        assert_eq!(outcome.moved_to_long, 1);
        assert_eq!(outcome.evicted_confirmed_entries, 0);
        assert_eq!(memory.confirmed_len(), 3);
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(2));
        assert_eq!(
            memory.confirmed_tier("aa", "甲"),
            Ok(Some(ConfirmedSelectionTier::Recent))
        );
        assert_eq!(
            memory.confirmed_tier("ac", "丙"),
            Ok(Some(ConfirmedSelectionTier::Medium))
        );
        assert_eq!(
            memory.confirmed_tier("ab", "乙"),
            Ok(Some(ConfirmedSelectionTier::Long))
        );
    }

    #[test]
    fn pending_capacity_discards_unconfirmed_oldest_evidence() {
        let mut memory = limited(2, 4);
        memory.observe_commit("aa", "甲", 0).unwrap();
        memory.observe_commit("ab", "乙", 1).unwrap();

        let outcome = memory.observe_commit("ac", "丙", 2).unwrap();

        assert_eq!(outcome.evicted_pending, 1);
        assert_eq!(memory.pending_count("aa", "甲"), Ok(0));
        assert_eq!(memory.pending_count("ab", "乙"), Ok(1));
        assert_eq!(memory.pending_count("ac", "丙"), Ok(1));
    }

    #[test]
    fn forget_removes_matching_pending_and_confirmed_evidence() {
        let mut memory = PendingSelectionMemory::new();
        memory.observe_commit("aa", "甲", 0).unwrap();
        memory.confirm_pending();
        memory.observe_commit("aa", "甲", 1).unwrap();
        memory.observe_commit("ab", "甲", 2).unwrap();

        assert_eq!(
            memory.forget("aa", "甲").unwrap(),
            super::PendingForgetOutcome {
                removed_pending: 1,
                removed_confirmed: 1,
            }
        );
        assert_eq!(memory.confirmed_count("aa", "甲"), Ok(0));
        assert_eq!(memory.pending_count("ab", "甲"), Ok(1));
    }

    #[test]
    fn forget_removes_evidence_from_an_older_tier() {
        let mut memory = PendingSelectionMemory::with_limits(
            PendingSelectionLimits::tiered(4, 1, 1, 1).unwrap(),
        )
        .unwrap();
        for (code, text, start) in [("aa", "甲", 0), ("ab", "乙", 1), ("ac", "丙", 2)] {
            memory.observe_commit(code, text, start).unwrap();
            memory.confirm_pending();
        }
        memory.observe_commit("ab", "乙", 3).unwrap();

        let outcome = memory.forget("ab", "乙").unwrap();

        assert_eq!(outcome.removed_pending, 1);
        assert_eq!(outcome.removed_confirmed, 1);
        assert_eq!(memory.confirmed_tier("ab", "乙"), Ok(None));
        assert_eq!(memory.confirmed_len(), 2);
    }

    #[test]
    fn invalid_inputs_and_overflow_are_rejected_without_mutation() {
        assert_eq!(
            PendingSelectionLimits::new(0, 1),
            Err(PendingSelectionError::InvalidLimits)
        );
        assert_eq!(
            PendingSelectionLimits::new(1, MAX_CONFIRMED_SELECTION_LIMIT + 1),
            Err(PendingSelectionError::InvalidLimits)
        );
        assert_eq!(
            PendingSelectionLimits::tiered(1, MAX_CONFIRMED_SELECTION_LIMIT, 1, 0),
            Err(PendingSelectionError::InvalidLimits)
        );

        let mut memory = PendingSelectionMemory::new();
        assert_eq!(
            memory.observe_commit("A", "甲", 0),
            Err(PendingSelectionError::InvalidCode)
        );
        assert_eq!(
            memory.observe_commit("aa", "", 0),
            Err(PendingSelectionError::InvalidText)
        );
        assert_eq!(
            memory.observe_commit("aa", "甲", usize::MAX),
            Err(PendingSelectionError::PositionOverflow)
        );
        memory.observe_commit("aa", "甲", 1).unwrap();
        assert_eq!(
            memory.apply_edit(PendingSelectionEdit {
                start: usize::MAX,
                deleted_chars: 1,
                inserted_chars: 0,
            }),
            Err(PendingSelectionError::PositionOverflow)
        );
        assert_eq!(memory.pending_len(), 1);
    }

    #[test]
    fn clear_removes_all_evidence_and_debug_never_contains_selection_text() {
        let mut memory = PendingSelectionMemory::new();
        memory
            .observe_commit("secretcode", "私密测试文字", 0)
            .unwrap();
        memory.confirm_pending();
        let debug = format!("{memory:?}");

        assert!(!debug.contains("secretcode"));
        assert!(!debug.contains("私密测试文字"));
        assert!(debug.contains("debug_contains_text: false"));

        memory.observe_commit("aa", "甲", 0).unwrap();
        memory.clear();
        assert_eq!(memory.pending_len(), 0);
        assert_eq!(memory.confirmed_len(), 0);
    }
}
