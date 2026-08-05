//! Conservative, in-memory grouping of atomic tracker events.
//!
//! A grouped item is deliberately called a correction *candidate*. Deleting
//! text and inserting at the same position soon afterwards is observable
//! evidence, but it does not prove the user's intent. This module performs no
//! I/O and retains no session after the detector is dropped.

use std::error::Error;
use std::fmt;

use crate::{DeltaPositionEvidence, RawKey, TextDelta, TrackerOutput};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorrectionCandidateKind {
    RestoredSameText,
    ReplacedWithDifferentText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorrectionCandidateForm {
    DeleteThenInsert,
    DirectReplacement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectionCandidate {
    pub source_commit_sequence: Option<u64>,
    pub deletion_sequence: u64,
    pub replacement_sequence: u64,
    pub deletion_elapsed_ms: u64,
    pub replacement_elapsed_ms: u64,
    pub start: usize,
    pub deleted: String,
    pub inserted: String,
    pub deletion_keys: Vec<RawKey>,
    pub replacement_keys: Vec<RawKey>,
    pub keys_complete: bool,
    pub deletion_position_evidence: DeltaPositionEvidence,
    pub replacement_position_evidence: DeltaPositionEvidence,
    pub replacement_composition: Option<String>,
    pub kind: CorrectionCandidateKind,
    pub form: CorrectionCandidateForm,
}

/// One structurally observed suffix trim of text inserted by the immediately
/// preceding input-method commit.
///
/// The observation contains private composition and document text. Its custom
/// `Debug` implementation therefore exposes only counts and structural
/// metadata. A trim is evidence about an edit, not proof that the retained
/// prefix is an intended new word.
#[derive(Clone, PartialEq, Eq)]
pub struct CommitTailTrimObservation {
    source_commit_sequence: u64,
    trim_sequence: u64,
    commit_elapsed_ms: u64,
    trim_elapsed_ms: u64,
    start: usize,
    composition: String,
    committed_text: String,
    retained_text: String,
    removed_suffix: String,
    commit_keys: Vec<RawKey>,
    trim_keys: Vec<RawKey>,
    keys_complete: bool,
    position_evidence: DeltaPositionEvidence,
}

impl fmt::Debug for CommitTailTrimObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommitTailTrimObservation")
            .field("source_commit_sequence", &self.source_commit_sequence)
            .field("trim_sequence", &self.trim_sequence)
            .field("gap_ms", &self.gap_ms())
            .field("start", &self.start)
            .field("committed_characters", &self.committed_text.chars().count())
            .field("retained_characters", &self.retained_text.chars().count())
            .field("removed_characters", &self.removed_suffix.chars().count())
            .field("commit_key_actions", &self.commit_keys.len())
            .field("trim_key_actions", &self.trim_keys.len())
            .field("keys_complete", &self.keys_complete)
            .field("position_evidence", &self.position_evidence)
            .field("debug_contains_text", &false)
            .finish()
    }
}

impl CommitTailTrimObservation {
    pub fn source_commit_sequence(&self) -> u64 {
        self.source_commit_sequence
    }

    pub fn trim_sequence(&self) -> u64 {
        self.trim_sequence
    }

    pub fn gap_ms(&self) -> u64 {
        self.trim_elapsed_ms - self.commit_elapsed_ms
    }

    pub fn start(&self) -> usize {
        self.start
    }

    pub fn composition(&self) -> &str {
        &self.composition
    }

    pub fn committed_text(&self) -> &str {
        &self.committed_text
    }

    pub fn retained_text(&self) -> &str {
        &self.retained_text
    }

    pub fn removed_suffix(&self) -> &str {
        &self.removed_suffix
    }

    pub fn commit_keys(&self) -> &[RawKey] {
        &self.commit_keys
    }

    pub fn trim_keys(&self) -> &[RawKey] {
        &self.trim_keys
    }

    pub fn keys_complete(&self) -> bool {
        self.keys_complete
    }

    pub fn position_evidence(&self) -> DeltaPositionEvidence {
        self.position_evidence
    }

    /// Derives one conservative complete-double-pinyin prefix proposal.
    ///
    /// This succeeds only when the recorded key actions reduce to lowercase
    /// letters followed by one selection key, every committed character maps
    /// to exactly two letters, and the trim retains a non-empty proper prefix.
    /// The result is still a proposal for review, never an automatic truth.
    pub fn retained_full_code_identity(&self) -> Option<(String, String)> {
        if !self.keys_complete {
            return None;
        }
        let code = effective_selected_letter_code(&self.commit_keys)?;
        let committed_characters = self.committed_text.chars().count();
        let retained_characters = self.retained_text.chars().count();
        if committed_characters == 0
            || retained_characters == 0
            || retained_characters >= committed_characters
            || code.len() != committed_characters.checked_mul(2)?
        {
            return None;
        }
        let retained_code_bytes = retained_characters.checked_mul(2)?;
        Some((
            code.get(..retained_code_bytes)?.to_owned(),
            self.retained_text.clone(),
        ))
    }
}

/// Groups only immediately adjacent suffix deletions with their source commit.
///
/// No time threshold participates in matching. Elapsed time is retained as
/// descriptive evidence so later analysis can study cadence without turning
/// an arbitrary notion of "fast" into an intent label.
pub struct CommitTailTrimDetector {
    next_sequence: u64,
    last_elapsed_ms: Option<u64>,
    recent_commit: Option<RecentTailCommit>,
}

struct RecentTailCommit {
    source_sequence: u64,
    last_sequence: u64,
    elapsed_ms: u64,
    start: usize,
    composition: String,
    original_text: String,
    current_text: String,
    keys: Vec<RawKey>,
    keys_complete: bool,
}

impl fmt::Debug for CommitTailTrimDetector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommitTailTrimDetector")
            .field("next_sequence", &self.next_sequence)
            .field("last_elapsed_ms", &self.last_elapsed_ms)
            .field("has_recent_commit", &self.recent_commit.is_some())
            .field("debug_contains_text", &false)
            .finish()
    }
}

impl Default for CommitTailTrimDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitTailTrimDetector {
    pub fn new() -> Self {
        Self {
            next_sequence: 0,
            last_elapsed_ms: None,
            recent_commit: None,
        }
    }

    pub fn observe(
        &mut self,
        elapsed_ms: u64,
        output: TrackerOutput,
    ) -> Result<Option<CommitTailTrimObservation>, CorrectionDetectorError> {
        if let Some(previous_ms) = self.last_elapsed_ms
            && elapsed_ms < previous_ms
        {
            return Err(CorrectionDetectorError::TimestampMovedBackward {
                previous_ms,
                current_ms: elapsed_ms,
            });
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.last_elapsed_ms = Some(elapsed_ms);

        match output {
            TrackerOutput::Commit(record) => {
                self.recent_commit = (!record.document_change.inserted.is_empty()
                    && reliable_position(record.document_change.position_evidence))
                .then(|| RecentTailCommit {
                    source_sequence: sequence,
                    last_sequence: sequence,
                    elapsed_ms,
                    start: record.document_change.start,
                    composition: record.composition,
                    original_text: record.document_change.inserted.clone(),
                    current_text: record.document_change.inserted,
                    keys: record.keys,
                    keys_complete: record.keys_complete,
                });
                Ok(None)
            }
            TrackerOutput::Revision(record) => {
                let Some(commit) = self.recent_commit.as_mut() else {
                    return Ok(None);
                };
                let deletion = &record.change;
                let commit_end = commit
                    .start
                    .saturating_add(commit.current_text.chars().count());
                let deletion_end = deletion
                    .start
                    .saturating_add(deletion.deleted.chars().count());
                if commit.last_sequence.saturating_add(1) != sequence
                    || !deletion.inserted.is_empty()
                    || deletion.deleted.is_empty()
                    || !reliable_position(deletion.position_evidence)
                    || deletion.start < commit.start
                    || deletion_end != commit_end
                {
                    self.recent_commit = None;
                    return Ok(None);
                }
                let retained_characters = deletion.start - commit.start;
                let Some((retained, removed_now)) =
                    split_at_character(&commit.current_text, retained_characters)
                else {
                    self.recent_commit = None;
                    return Ok(None);
                };
                if removed_now != deletion.deleted {
                    self.recent_commit = None;
                    return Ok(None);
                }
                let Some((_, removed_suffix)) =
                    split_at_character(&commit.original_text, retained_characters)
                else {
                    self.recent_commit = None;
                    return Ok(None);
                };
                commit.current_text = retained.clone();
                commit.last_sequence = sequence;
                Ok(Some(CommitTailTrimObservation {
                    source_commit_sequence: commit.source_sequence,
                    trim_sequence: sequence,
                    commit_elapsed_ms: commit.elapsed_ms,
                    trim_elapsed_ms: elapsed_ms,
                    start: commit.start,
                    composition: commit.composition.clone(),
                    committed_text: commit.original_text.clone(),
                    retained_text: retained,
                    removed_suffix,
                    commit_keys: commit.keys.clone(),
                    trim_keys: record.keys,
                    keys_complete: commit.keys_complete && record.keys_complete,
                    position_evidence: deletion.position_evidence,
                }))
            }
        }
    }
}

fn split_at_character(text: &str, character_offset: usize) -> Option<(String, String)> {
    let byte_offset = if character_offset == text.chars().count() {
        text.len()
    } else {
        text.char_indices().nth(character_offset)?.0
    };
    Some((
        text.get(..byte_offset)?.to_owned(),
        text.get(byte_offset..)?.to_owned(),
    ))
}

fn effective_selected_letter_code(keys: &[RawKey]) -> Option<String> {
    let mut code = String::new();
    let mut selected = false;
    for key in keys {
        match key {
            RawKey::Letter(letter) if !selected && letter.is_ascii_lowercase() => {
                code.push(*letter);
            }
            RawKey::Backspace if !selected => {
                code.pop()?;
            }
            RawKey::Digit(_) | RawKey::Space if !selected && !code.is_empty() => {
                selected = true;
            }
            _ => return None,
        }
    }
    selected.then_some(code)
}

impl CorrectionCandidate {
    pub fn gap_ms(&self) -> u64 {
        self.replacement_elapsed_ms - self.deletion_elapsed_ms
    }

    pub fn logical_key_actions(&self) -> usize {
        self.deletion_keys.len() + self.replacement_keys.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorrectionDetectorError {
    TimestampMovedBackward { previous_ms: u64, current_ms: u64 },
}

impl fmt::Display for CorrectionDetectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimestampMovedBackward {
                previous_ms,
                current_ms,
            } => write!(
                formatter,
                "event timestamp moved backward from {previous_ms} ms to {current_ms} ms"
            ),
        }
    }
}

impl Error for CorrectionDetectorError {}

#[derive(Debug)]
pub struct CorrectionCandidateDetector {
    max_gap_ms: u64,
    next_sequence: u64,
    last_elapsed_ms: Option<u64>,
    last_commit: Option<RecentCommit>,
    pending_deletion: Option<PendingDeletion>,
}

#[derive(Debug)]
struct RecentCommit {
    sequence: u64,
    elapsed_ms: u64,
    start: usize,
    inserted_chars: usize,
}

#[derive(Debug)]
struct PendingDeletion {
    source_commit_sequence: Option<u64>,
    sequence: u64,
    elapsed_ms: u64,
    start: usize,
    deleted: String,
    keys: Vec<RawKey>,
    keys_complete: bool,
    position_evidence: DeltaPositionEvidence,
}

impl CorrectionCandidateDetector {
    /// Creates a detector with a caller-chosen temporal boundary.
    ///
    /// `max_gap_ms` is experimental configuration, not a measured truth about
    /// typing intent. A candidate is emitted only when deletion and insertion
    /// positions are non-ambiguous and equal.
    pub fn new(max_gap_ms: u64) -> Self {
        Self {
            max_gap_ms,
            next_sequence: 0,
            last_elapsed_ms: None,
            last_commit: None,
            pending_deletion: None,
        }
    }

    pub fn observe(
        &mut self,
        elapsed_ms: u64,
        output: TrackerOutput,
    ) -> Result<Option<CorrectionCandidate>, CorrectionDetectorError> {
        if let Some(previous_ms) = self.last_elapsed_ms
            && elapsed_ms < previous_ms
        {
            return Err(CorrectionDetectorError::TimestampMovedBackward {
                previous_ms,
                current_ms: elapsed_ms,
            });
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.last_elapsed_ms = Some(elapsed_ms);
        if self
            .pending_deletion
            .as_ref()
            .is_some_and(|pending| elapsed_ms - pending.elapsed_ms > self.max_gap_ms)
        {
            self.pending_deletion = None;
        }

        match output {
            TrackerOutput::Revision(record)
                if !record.change.deleted.is_empty() && record.change.inserted.is_empty() =>
            {
                let source_commit_sequence = self
                    .last_commit
                    .take()
                    .filter(|commit| {
                        commit.sequence + 1 == sequence
                            && elapsed_ms - commit.elapsed_ms <= self.max_gap_ms
                            && deletion_is_within_commit(&record.change, commit)
                    })
                    .map(|commit| commit.sequence);

                self.pending_deletion = reliable_position(record.change.position_evidence)
                    .then_some(PendingDeletion {
                        source_commit_sequence,
                        sequence,
                        elapsed_ms,
                        start: record.change.start,
                        deleted: record.change.deleted,
                        keys: record.keys,
                        keys_complete: record.keys_complete,
                        position_evidence: record.change.position_evidence,
                    });
                Ok(None)
            }
            TrackerOutput::Commit(record) => {
                let candidate = self
                    .match_insertion(
                        sequence,
                        elapsed_ms,
                        &record.document_change,
                        &record.keys,
                        record.keys_complete,
                        Some(record.composition.clone()),
                    )
                    .or_else(|| {
                        self.direct_replacement(
                            sequence,
                            elapsed_ms,
                            &record.document_change,
                            &record.keys,
                            record.keys_complete,
                            Some(record.composition.clone()),
                        )
                    });
                self.last_commit =
                    (!record.document_change.inserted.is_empty()).then(|| RecentCommit {
                        sequence,
                        elapsed_ms,
                        start: record.document_change.start,
                        inserted_chars: record.document_change.inserted.chars().count(),
                    });
                Ok(candidate)
            }
            TrackerOutput::Revision(record) if !record.change.inserted.is_empty() => {
                let candidate = self.match_insertion(
                    sequence,
                    elapsed_ms,
                    &record.change,
                    &record.keys,
                    record.keys_complete,
                    None,
                );
                self.last_commit = None;
                Ok(candidate)
            }
            TrackerOutput::Revision(_) => {
                self.pending_deletion = None;
                self.last_commit = None;
                Ok(None)
            }
        }
    }

    fn match_insertion(
        &mut self,
        sequence: u64,
        elapsed_ms: u64,
        change: &TextDelta,
        keys: &[RawKey],
        keys_complete: bool,
        replacement_composition: Option<String>,
    ) -> Option<CorrectionCandidate> {
        let pending = self.pending_deletion.take()?;
        if elapsed_ms - pending.elapsed_ms > self.max_gap_ms
            || pending.start != change.start
            || change.inserted.is_empty()
            || !reliable_position(change.position_evidence)
        {
            return None;
        }

        let kind = if pending.deleted == change.inserted {
            CorrectionCandidateKind::RestoredSameText
        } else {
            CorrectionCandidateKind::ReplacedWithDifferentText
        };
        Some(CorrectionCandidate {
            source_commit_sequence: pending.source_commit_sequence,
            deletion_sequence: pending.sequence,
            replacement_sequence: sequence,
            deletion_elapsed_ms: pending.elapsed_ms,
            replacement_elapsed_ms: elapsed_ms,
            start: pending.start,
            deleted: pending.deleted,
            inserted: change.inserted.clone(),
            deletion_keys: pending.keys,
            replacement_keys: keys.to_vec(),
            keys_complete: pending.keys_complete && keys_complete,
            deletion_position_evidence: pending.position_evidence,
            replacement_position_evidence: change.position_evidence,
            replacement_composition,
            kind,
            form: CorrectionCandidateForm::DeleteThenInsert,
        })
    }

    fn direct_replacement(
        &mut self,
        sequence: u64,
        elapsed_ms: u64,
        change: &TextDelta,
        keys: &[RawKey],
        keys_complete: bool,
        replacement_composition: Option<String>,
    ) -> Option<CorrectionCandidate> {
        if change.deleted.is_empty()
            || change.inserted.is_empty()
            || !reliable_position(change.position_evidence)
        {
            return None;
        }

        let source_commit_sequence = self
            .last_commit
            .take()
            .filter(|commit| {
                commit.sequence + 1 == sequence
                    && elapsed_ms - commit.elapsed_ms <= self.max_gap_ms
                    && deletion_is_within_commit(change, commit)
            })
            .map(|commit| commit.sequence);
        let kind = if change.deleted == change.inserted {
            CorrectionCandidateKind::RestoredSameText
        } else {
            CorrectionCandidateKind::ReplacedWithDifferentText
        };

        Some(CorrectionCandidate {
            source_commit_sequence,
            deletion_sequence: sequence,
            replacement_sequence: sequence,
            deletion_elapsed_ms: elapsed_ms,
            replacement_elapsed_ms: elapsed_ms,
            start: change.start,
            deleted: change.deleted.clone(),
            inserted: change.inserted.clone(),
            deletion_keys: Vec::new(),
            replacement_keys: keys.to_vec(),
            keys_complete,
            deletion_position_evidence: change.position_evidence,
            replacement_position_evidence: change.position_evidence,
            replacement_composition,
            kind,
            form: CorrectionCandidateForm::DirectReplacement,
        })
    }
}

fn reliable_position(evidence: DeltaPositionEvidence) -> bool {
    evidence != DeltaPositionEvidence::Ambiguous
}

fn deletion_is_within_commit(deletion: &TextDelta, commit: &RecentCommit) -> bool {
    let deletion_end = deletion.start + deletion.deleted.chars().count();
    let commit_end = commit.start + commit.inserted_chars;
    deletion.start >= commit.start && deletion_end <= commit_end
}

#[cfg(test)]
mod tests {
    use super::{
        CommitTailTrimDetector, CorrectionCandidateDetector, CorrectionCandidateForm,
        CorrectionCandidateKind, CorrectionDetectorError,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, RawKey, RevisionRecord, TextDelta, TrackerOutput,
    };

    fn commit(
        start: usize,
        deleted: &str,
        inserted: &str,
        keys: Vec<RawKey>,
        keys_complete: bool,
    ) -> TrackerOutput {
        TrackerOutput::Commit(CommitRecord {
            keys,
            keys_complete,
            composition: "mao".to_owned(),
            change: TextDelta {
                start,
                deleted: deleted.to_owned(),
                inserted: inserted.to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
            document_change: TextDelta {
                start,
                deleted: String::new(),
                inserted: inserted.to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
        })
    }

    fn direct_commit(
        start: usize,
        deleted: &str,
        inserted: &str,
        evidence: DeltaPositionEvidence,
    ) -> TrackerOutput {
        TrackerOutput::Commit(CommitRecord {
            keys: vec![RawKey::Letter('z'), RawKey::Space],
            keys_complete: true,
            composition: "zai".to_owned(),
            change: TextDelta {
                start,
                deleted: "zai".to_owned(),
                inserted: inserted.to_owned(),
                position_evidence: DeltaPositionEvidence::UniqueText,
            },
            document_change: TextDelta {
                start,
                deleted: deleted.to_owned(),
                inserted: inserted.to_owned(),
                position_evidence: evidence,
            },
        })
    }

    fn deletion(
        start: usize,
        deleted: &str,
        evidence: DeltaPositionEvidence,
        keys_complete: bool,
    ) -> TrackerOutput {
        TrackerOutput::Revision(RevisionRecord {
            keys: vec![RawKey::Backspace],
            keys_complete,
            change: TextDelta {
                start,
                deleted: deleted.to_owned(),
                inserted: String::new(),
                position_evidence: evidence,
            },
        })
    }

    #[test]
    fn groups_direct_delete_and_same_text_recommit_without_claiming_intent() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        assert_eq!(
            detector.observe(
                100,
                commit(
                    0,
                    "mao'mao",
                    "猫猫",
                    vec![RawKey::Letter('m'), RawKey::Space],
                    true,
                ),
            ),
            Ok(None)
        );
        assert_eq!(
            detector.observe(200, deletion(1, "猫", DeltaPositionEvidence::Caret, true),),
            Ok(None)
        );

        let candidate = detector
            .observe(
                450,
                commit(
                    1,
                    "mao",
                    "猫",
                    vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space],
                    true,
                ),
            )
            .expect("monotonic time")
            .expect("correction candidate");
        assert_eq!(candidate.source_commit_sequence, Some(0));
        assert_eq!(candidate.deletion_sequence, 1);
        assert_eq!(candidate.replacement_sequence, 2);
        assert_eq!(candidate.start, 1);
        assert_eq!(candidate.deleted, "猫");
        assert_eq!(candidate.inserted, "猫");
        assert_eq!(candidate.kind, CorrectionCandidateKind::RestoredSameText);
        assert_eq!(candidate.form, CorrectionCandidateForm::DeleteThenInsert);
        assert_eq!(candidate.gap_ms(), 250);
        assert_eq!(candidate.logical_key_actions(), 4);
        assert!(candidate.keys_complete);
    }

    #[test]
    fn labels_different_replacement_text_as_a_distinct_candidate_kind() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        detector
            .observe(
                0,
                deletion(2, "再", DeltaPositionEvidence::UniqueText, true),
            )
            .unwrap();
        let candidate = detector
            .observe(
                100,
                commit(
                    2,
                    "zai",
                    "在",
                    vec![RawKey::Letter('z'), RawKey::Space],
                    true,
                ),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            candidate.kind,
            CorrectionCandidateKind::ReplacedWithDifferentText
        );
        assert_eq!(candidate.source_commit_sequence, None);
    }

    #[test]
    fn rejects_timeout_different_position_and_ambiguous_deletion() {
        let mut timeout = CorrectionCandidateDetector::new(100);
        timeout
            .observe(0, deletion(1, "猫", DeltaPositionEvidence::Caret, true))
            .unwrap();
        assert_eq!(
            timeout
                .observe(101, commit(1, "mao", "猫", Vec::new(), true))
                .unwrap(),
            None
        );

        let mut different_position = CorrectionCandidateDetector::new(5_000);
        different_position
            .observe(0, deletion(1, "猫", DeltaPositionEvidence::Caret, true))
            .unwrap();
        assert_eq!(
            different_position
                .observe(10, commit(2, "mao", "猫", Vec::new(), true))
                .unwrap(),
            None
        );

        let mut ambiguous = CorrectionCandidateDetector::new(5_000);
        ambiguous
            .observe(0, deletion(1, "猫", DeltaPositionEvidence::Ambiguous, true))
            .unwrap();
        assert_eq!(
            ambiguous
                .observe(10, commit(1, "mao", "猫", Vec::new(), true))
                .unwrap(),
            None
        );
    }

    #[test]
    fn propagates_incomplete_key_evidence_without_dropping_the_candidate() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        detector
            .observe(0, deletion(1, "猫", DeltaPositionEvidence::Caret, false))
            .unwrap();
        let candidate = detector
            .observe(10, commit(1, "mao", "猫", Vec::new(), true))
            .unwrap()
            .unwrap();
        assert!(!candidate.keys_complete);
    }

    #[test]
    fn emits_a_direct_replacement_candidate_from_the_document_delta() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        let candidate = detector
            .observe(
                100,
                direct_commit(1, "再", "在", DeltaPositionEvidence::UniqueText),
            )
            .unwrap()
            .expect("direct replacement candidate");

        assert_eq!(candidate.form, CorrectionCandidateForm::DirectReplacement);
        assert_eq!(
            candidate.kind,
            CorrectionCandidateKind::ReplacedWithDifferentText
        );
        assert_eq!(candidate.deletion_sequence, 0);
        assert_eq!(candidate.replacement_sequence, 0);
        assert_eq!(candidate.gap_ms(), 0);
        assert_eq!(candidate.start, 1);
        assert_eq!(candidate.deleted, "再");
        assert_eq!(candidate.inserted, "在");
        assert!(candidate.deletion_keys.is_empty());
        assert_eq!(
            candidate.replacement_keys,
            vec![RawKey::Letter('z'), RawKey::Space]
        );
    }

    #[test]
    fn rejects_a_direct_replacement_with_ambiguous_position() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        assert_eq!(
            detector
                .observe(
                    100,
                    direct_commit(1, "猫", "猫", DeltaPositionEvidence::Ambiguous),
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn rejects_non_monotonic_timestamps_without_consuming_a_sequence() {
        let mut detector = CorrectionCandidateDetector::new(5_000);
        assert_eq!(
            detector
                .observe(20, deletion(1, "猫", DeltaPositionEvidence::Caret, true),)
                .unwrap(),
            None
        );
        assert_eq!(
            detector.observe(19, commit(1, "mao", "猫", Vec::new(), true)),
            Err(CorrectionDetectorError::TimestampMovedBackward {
                previous_ms: 20,
                current_ms: 19,
            })
        );
        let candidate = detector
            .observe(21, commit(1, "mao", "猫", Vec::new(), true))
            .unwrap()
            .unwrap();
        assert_eq!(candidate.replacement_sequence, 1);
    }

    #[test]
    fn observes_an_adjacent_commit_suffix_trim_without_a_speed_threshold() {
        let mut detector = CommitTailTrimDetector::new();
        let keys = "xmuufu"
            .chars()
            .map(RawKey::Letter)
            .chain(std::iter::once(RawKey::Space))
            .collect();
        assert_eq!(
            detector
                .observe(100, commit(0, "xmuufu", "线束缚", keys, true))
                .unwrap(),
            None
        );

        let observation = detector
            .observe(
                900_000,
                deletion(2, "缚", DeltaPositionEvidence::Caret, true),
            )
            .unwrap()
            .expect("the structural suffix relation does not depend on speed");
        assert_eq!(observation.source_commit_sequence(), 0);
        assert_eq!(observation.trim_sequence(), 1);
        assert_eq!(observation.gap_ms(), 899_900);
        assert_eq!(observation.committed_text(), "线束缚");
        assert_eq!(observation.retained_text(), "线束");
        assert_eq!(observation.removed_suffix(), "缚");
        assert_eq!(
            observation.retained_full_code_identity(),
            Some(("xmuu".to_owned(), "线束".to_owned()))
        );
        let debug = format!("{observation:?}");
        assert!(!debug.contains("线束"));
        assert!(!debug.contains("xmuufu"));
        assert!(debug.contains("debug_contains_text: false"));
    }

    #[test]
    fn consecutive_suffix_trims_stay_attached_to_the_original_commit() {
        let mut detector = CommitTailTrimDetector::new();
        let keys = "xmuufu"
            .chars()
            .map(RawKey::Letter)
            .chain(std::iter::once(RawKey::Digit(2)))
            .collect();
        detector
            .observe(10, commit(4, "xmuufu", "线束缚", keys, true))
            .unwrap();
        let first = detector
            .observe(
                20,
                deletion(6, "缚", DeltaPositionEvidence::UniqueText, true),
            )
            .unwrap()
            .unwrap();
        assert_eq!(first.retained_text(), "线束");

        let second = detector
            .observe(30, deletion(5, "束", DeltaPositionEvidence::Caret, true))
            .unwrap()
            .unwrap();
        assert_eq!(second.source_commit_sequence(), 0);
        assert_eq!(second.retained_text(), "线");
        assert_eq!(second.removed_suffix(), "束缚");
        assert_eq!(
            second.retained_full_code_identity(),
            Some(("xm".to_owned(), "线".to_owned()))
        );
    }

    #[test]
    fn tail_trim_rejects_middle_ambiguous_and_interrupted_edits() {
        let mut middle = CommitTailTrimDetector::new();
        middle
            .observe(0, commit(0, "abc", "甲乙丙", vec![RawKey::Space], true))
            .unwrap();
        assert_eq!(
            middle
                .observe(1, deletion(1, "乙", DeltaPositionEvidence::Caret, true),)
                .unwrap(),
            None
        );

        let mut ambiguous = CommitTailTrimDetector::new();
        ambiguous
            .observe(0, commit(0, "abc", "甲乙丙", vec![RawKey::Space], true))
            .unwrap();
        assert_eq!(
            ambiguous
                .observe(1, deletion(2, "丙", DeltaPositionEvidence::Ambiguous, true),)
                .unwrap(),
            None
        );

        let mut interrupted = CommitTailTrimDetector::new();
        interrupted
            .observe(0, commit(0, "abc", "甲乙丙", vec![RawKey::Space], true))
            .unwrap();
        interrupted
            .observe(
                1,
                TrackerOutput::Revision(RevisionRecord {
                    keys: vec![RawKey::Letter('a')],
                    keys_complete: true,
                    change: TextDelta {
                        start: 3,
                        deleted: String::new(),
                        inserted: "丁".to_owned(),
                        position_evidence: DeltaPositionEvidence::Caret,
                    },
                }),
            )
            .unwrap();
        assert_eq!(
            interrupted
                .observe(2, deletion(2, "丙", DeltaPositionEvidence::Caret, true),)
                .unwrap(),
            None
        );
    }

    #[test]
    fn trim_observation_keeps_incomplete_or_nonuniform_codes_out_of_word_proposals() {
        let mut detector = CommitTailTrimDetector::new();
        detector
            .observe(
                0,
                commit(
                    0,
                    "xmuuf",
                    "线束缚",
                    vec![RawKey::Letter('x'), RawKey::Space],
                    false,
                ),
            )
            .unwrap();
        let observation = detector
            .observe(1, deletion(2, "缚", DeltaPositionEvidence::Caret, true))
            .unwrap()
            .unwrap();
        assert_eq!(observation.retained_full_code_identity(), None);
        assert!(!observation.keys_complete());
    }
}
