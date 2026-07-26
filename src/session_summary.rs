//! Strict, text-free session summaries shared by the live probe and offline reports.
//!
//! The v1 JSON parser intentionally accepts only the deterministic byte layout
//! emitted by [`SessionSummaryV1::to_json`]. These files are machine artifacts,
//! not a general JSON interchange surface. Rejecting reordered, reformatted,
//! unknown, or internally inconsistent data keeps schema drift observable.

use std::error::Error;
use std::fmt;

use crate::{
    CorrectionCandidate, CorrectionCandidateForm, CorrectionCandidateKind, DeltaPositionEvidence,
    RawKey, TrackerOutput,
};

pub const SESSION_SUMMARY_SCHEMA_V1: &str = "ziranma-session-summary-v1";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSummaryCounts {
    pub commits: u64,
    pub revisions: u64,
    pub keys_complete_records: u64,
    pub keys_incomplete_records: u64,
    pub logical_key_actions: u64,
    pub commits_with_internal_edit_keys: u64,
    pub ambiguous_document_positions: u64,
    pub direct_replacements: u64,
    pub delete_then_insertions: u64,
    pub restored_same_text: u64,
    pub replaced_with_different_text: u64,
    pub source_linked_candidates: u64,
    pub delete_then_gap_count: u64,
    pub delete_then_gap_min_ms: Option<u64>,
    pub delete_then_gap_max_ms: Option<u64>,
    pub delete_then_gap_total_ms: u64,
}

impl SessionSummaryCounts {
    pub fn observe_output(&mut self, output: &TrackerOutput) {
        let (keys, keys_complete, position_evidence) = match output {
            TrackerOutput::Commit(record) => {
                self.commits = self.commits.saturating_add(1);
                if record.keys.iter().any(is_internal_edit_key) {
                    self.commits_with_internal_edit_keys =
                        self.commits_with_internal_edit_keys.saturating_add(1);
                }
                (
                    &record.keys,
                    record.keys_complete,
                    record.document_change.position_evidence,
                )
            }
            TrackerOutput::Revision(record) => {
                self.revisions = self.revisions.saturating_add(1);
                (
                    &record.keys,
                    record.keys_complete,
                    record.change.position_evidence,
                )
            }
        };

        if keys_complete {
            self.keys_complete_records = self.keys_complete_records.saturating_add(1);
        } else {
            self.keys_incomplete_records = self.keys_incomplete_records.saturating_add(1);
        }
        self.logical_key_actions = self
            .logical_key_actions
            .saturating_add(u64::try_from(keys.len()).unwrap_or(u64::MAX));
        if position_evidence == DeltaPositionEvidence::Ambiguous {
            self.ambiguous_document_positions = self.ambiguous_document_positions.saturating_add(1);
        }
    }

    pub fn observe_candidate(&mut self, candidate: &CorrectionCandidate) {
        match candidate.form {
            CorrectionCandidateForm::DirectReplacement => {
                self.direct_replacements = self.direct_replacements.saturating_add(1);
            }
            CorrectionCandidateForm::DeleteThenInsert => {
                self.delete_then_insertions = self.delete_then_insertions.saturating_add(1);
                let gap_ms = candidate.gap_ms();
                self.delete_then_gap_count = self.delete_then_gap_count.saturating_add(1);
                self.delete_then_gap_min_ms = Some(
                    self.delete_then_gap_min_ms
                        .map_or(gap_ms, |current| current.min(gap_ms)),
                );
                self.delete_then_gap_max_ms = Some(
                    self.delete_then_gap_max_ms
                        .map_or(gap_ms, |current| current.max(gap_ms)),
                );
                self.delete_then_gap_total_ms =
                    self.delete_then_gap_total_ms.saturating_add(gap_ms);
            }
        }
        match candidate.kind {
            CorrectionCandidateKind::RestoredSameText => {
                self.restored_same_text = self.restored_same_text.saturating_add(1);
            }
            CorrectionCandidateKind::ReplacedWithDifferentText => {
                self.replaced_with_different_text =
                    self.replaced_with_different_text.saturating_add(1);
            }
        }
        if candidate.source_commit_sequence.is_some() {
            self.source_linked_candidates = self.source_linked_candidates.saturating_add(1);
        }
    }

    pub fn delete_then_gap_mean_ms(&self) -> Option<u64> {
        (self.delete_then_gap_count > 0)
            .then(|| self.delete_then_gap_total_ms / self.delete_then_gap_count)
    }

    fn checked_merge(&mut self, other: &Self) -> Result<(), SessionSummaryError> {
        macro_rules! add {
            ($field:ident) => {
                self.$field = self
                    .$field
                    .checked_add(other.$field)
                    .ok_or(SessionSummaryError::CounterOverflow(stringify!($field)))?;
            };
        }
        add!(commits);
        add!(revisions);
        add!(keys_complete_records);
        add!(keys_incomplete_records);
        add!(logical_key_actions);
        add!(commits_with_internal_edit_keys);
        add!(ambiguous_document_positions);
        add!(direct_replacements);
        add!(delete_then_insertions);
        add!(restored_same_text);
        add!(replaced_with_different_text);
        add!(source_linked_candidates);
        add!(delete_then_gap_count);
        add!(delete_then_gap_total_ms);
        self.delete_then_gap_min_ms =
            match (self.delete_then_gap_min_ms, other.delete_then_gap_min_ms) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (Some(value), None) | (None, Some(value)) => Some(value),
                (None, None) => None,
            };
        self.delete_then_gap_max_ms =
            match (self.delete_then_gap_max_ms, other.delete_then_gap_max_ms) {
                (Some(left), Some(right)) => Some(left.max(right)),
                (Some(value), None) | (None, Some(value)) => Some(value),
                (None, None) => None,
            };
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummaryV1 {
    pub elapsed_ms: u64,
    pub candidate_gap_limit_ms: u64,
    pub key_capture_requested: bool,
    pub key_capture_ready: bool,
    pub counts: SessionSummaryCounts,
}

impl SessionSummaryV1 {
    pub fn validate(&self) -> Result<(), SessionSummaryError> {
        if self.candidate_gap_limit_ms == 0 {
            return Err(SessionSummaryError::InvalidInvariant(
                "candidate_gap_limit_ms must be positive",
            ));
        }
        if self.key_capture_ready && !self.key_capture_requested {
            return Err(SessionSummaryError::InvalidInvariant(
                "key_capture_ready requires key_capture_requested",
            ));
        }
        let records = self
            .counts
            .commits
            .checked_add(self.counts.revisions)
            .ok_or(SessionSummaryError::CounterOverflow("records"))?;
        let key_records = self
            .counts
            .keys_complete_records
            .checked_add(self.counts.keys_incomplete_records)
            .ok_or(SessionSummaryError::CounterOverflow("key_records"))?;
        if records != key_records {
            return Err(SessionSummaryError::InvalidInvariant(
                "complete and incomplete key records must equal commits and revisions",
            ));
        }
        if self.counts.commits_with_internal_edit_keys > self.counts.commits {
            return Err(SessionSummaryError::InvalidInvariant(
                "commits with internal edit keys cannot exceed commits",
            ));
        }
        if self.counts.ambiguous_document_positions > records {
            return Err(SessionSummaryError::InvalidInvariant(
                "ambiguous document positions cannot exceed all records",
            ));
        }
        let forms = self
            .counts
            .direct_replacements
            .checked_add(self.counts.delete_then_insertions)
            .ok_or(SessionSummaryError::CounterOverflow("candidate_forms"))?;
        let kinds = self
            .counts
            .restored_same_text
            .checked_add(self.counts.replaced_with_different_text)
            .ok_or(SessionSummaryError::CounterOverflow("candidate_kinds"))?;
        if forms != kinds {
            return Err(SessionSummaryError::InvalidInvariant(
                "candidate form and kind totals must match",
            ));
        }
        if self.counts.source_linked_candidates > forms {
            return Err(SessionSummaryError::InvalidInvariant(
                "source-linked candidates cannot exceed all candidates",
            ));
        }
        if self.counts.delete_then_gap_count != self.counts.delete_then_insertions {
            return Err(SessionSummaryError::InvalidInvariant(
                "delete-then gap count must equal delete-then candidates",
            ));
        }
        match self.counts.delete_then_gap_count {
            0 => {
                if self.counts.delete_then_gap_min_ms.is_some()
                    || self.counts.delete_then_gap_max_ms.is_some()
                    || self.counts.delete_then_gap_total_ms != 0
                {
                    return Err(SessionSummaryError::InvalidInvariant(
                        "zero delete-then gaps require null bounds and a zero total",
                    ));
                }
            }
            _ => {
                let minimum = self.counts.delete_then_gap_min_ms.ok_or(
                    SessionSummaryError::InvalidInvariant(
                        "nonzero delete-then gaps require a minimum",
                    ),
                )?;
                let maximum = self.counts.delete_then_gap_max_ms.ok_or(
                    SessionSummaryError::InvalidInvariant(
                        "nonzero delete-then gaps require a maximum",
                    ),
                )?;
                let mean = self
                    .counts
                    .delete_then_gap_mean_ms()
                    .expect("nonzero gap count");
                if minimum > mean || mean > maximum {
                    return Err(SessionSummaryError::InvalidInvariant(
                        "delete-then gap mean must be within the observed bounds",
                    ));
                }
                let minimum_total = minimum
                    .checked_mul(self.counts.delete_then_gap_count)
                    .ok_or(SessionSummaryError::CounterOverflow(
                        "delete_then_gap_minimum_total",
                    ))?;
                let maximum_total = maximum
                    .checked_mul(self.counts.delete_then_gap_count)
                    .ok_or(SessionSummaryError::CounterOverflow(
                        "delete_then_gap_maximum_total",
                    ))?;
                if self.counts.delete_then_gap_total_ms < minimum_total
                    || self.counts.delete_then_gap_total_ms > maximum_total
                {
                    return Err(SessionSummaryError::InvalidInvariant(
                        "delete-then gap total must agree with count and bounds",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn terminal_line(&self) -> String {
        let counts = &self.counts;
        format!(
            "SESSION_SUMMARY elapsed_ms={} candidate_gap_limit_ms={} \
             key_capture_requested={} key_capture_ready={} commits={} revisions={} \
             keys_complete_records={} keys_incomplete_records={} logical_key_actions={} \
             commits_with_internal_edit_keys={} ambiguous_document_positions={} \
             direct_replacements={} delete_then_insertions={} restored_same_text={} \
             replaced_with_different_text={} source_linked_candidates={} \
             delete_then_gap_count={} delete_then_gap_min_ms={} delete_then_gap_max_ms={} \
             delete_then_gap_mean_ms={} delete_then_gap_total_ms={}",
            self.elapsed_ms,
            self.candidate_gap_limit_ms,
            self.key_capture_requested,
            self.key_capture_ready,
            counts.commits,
            counts.revisions,
            counts.keys_complete_records,
            counts.keys_incomplete_records,
            counts.logical_key_actions,
            counts.commits_with_internal_edit_keys,
            counts.ambiguous_document_positions,
            counts.direct_replacements,
            counts.delete_then_insertions,
            counts.restored_same_text,
            counts.replaced_with_different_text,
            counts.source_linked_candidates,
            counts.delete_then_gap_count,
            display_optional(counts.delete_then_gap_min_ms),
            display_optional(counts.delete_then_gap_max_ms),
            display_optional(counts.delete_then_gap_mean_ms()),
            counts.delete_then_gap_total_ms
        )
    }

    pub fn to_json(&self) -> Result<String, SessionSummaryError> {
        self.validate()?;
        let counts = &self.counts;
        Ok(format!(
            "{{\"schema\":\"{SESSION_SUMMARY_SCHEMA_V1}\",\"contains_text\":false,\
             \"elapsed_ms\":{},\"candidate_gap_limit_ms\":{},\
             \"key_capture_requested\":{},\"key_capture_ready\":{},\
             \"commits\":{},\"revisions\":{},\"keys_complete_records\":{},\
             \"keys_incomplete_records\":{},\"logical_key_actions\":{},\
             \"commits_with_internal_edit_keys\":{},\"ambiguous_document_positions\":{},\
             \"direct_replacements\":{},\"delete_then_insertions\":{},\
             \"restored_same_text\":{},\"replaced_with_different_text\":{},\
             \"source_linked_candidates\":{},\"delete_then_gap_count\":{},\
             \"delete_then_gap_min_ms\":{},\"delete_then_gap_max_ms\":{},\
             \"delete_then_gap_mean_ms\":{},\"delete_then_gap_total_ms\":{}}}",
            self.elapsed_ms,
            self.candidate_gap_limit_ms,
            self.key_capture_requested,
            self.key_capture_ready,
            counts.commits,
            counts.revisions,
            counts.keys_complete_records,
            counts.keys_incomplete_records,
            counts.logical_key_actions,
            counts.commits_with_internal_edit_keys,
            counts.ambiguous_document_positions,
            counts.direct_replacements,
            counts.delete_then_insertions,
            counts.restored_same_text,
            counts.replaced_with_different_text,
            counts.source_linked_candidates,
            counts.delete_then_gap_count,
            json_optional(counts.delete_then_gap_min_ms),
            json_optional(counts.delete_then_gap_max_ms),
            json_optional(counts.delete_then_gap_mean_ms()),
            counts.delete_then_gap_total_ms
        ))
    }

    pub fn from_json(input: &str) -> Result<Self, SessionSummaryError> {
        let input = input
            .strip_suffix("\r\n")
            .or_else(|| input.strip_suffix('\n'))
            .unwrap_or(input);
        let mut cursor = StrictV1Cursor::new(input);
        cursor.expect("{\"schema\":\"ziranma-session-summary-v1\",\"contains_text\":false")?;
        let elapsed_ms = cursor.number_field("elapsed_ms")?;
        let candidate_gap_limit_ms = cursor.number_field("candidate_gap_limit_ms")?;
        let key_capture_requested = cursor.bool_field("key_capture_requested")?;
        let key_capture_ready = cursor.bool_field("key_capture_ready")?;
        let commits = cursor.number_field("commits")?;
        let revisions = cursor.number_field("revisions")?;
        let keys_complete_records = cursor.number_field("keys_complete_records")?;
        let keys_incomplete_records = cursor.number_field("keys_incomplete_records")?;
        let logical_key_actions = cursor.number_field("logical_key_actions")?;
        let commits_with_internal_edit_keys =
            cursor.number_field("commits_with_internal_edit_keys")?;
        let ambiguous_document_positions = cursor.number_field("ambiguous_document_positions")?;
        let direct_replacements = cursor.number_field("direct_replacements")?;
        let delete_then_insertions = cursor.number_field("delete_then_insertions")?;
        let restored_same_text = cursor.number_field("restored_same_text")?;
        let replaced_with_different_text = cursor.number_field("replaced_with_different_text")?;
        let source_linked_candidates = cursor.number_field("source_linked_candidates")?;
        let delete_then_gap_count = cursor.number_field("delete_then_gap_count")?;
        let delete_then_gap_min_ms = cursor.optional_number_field("delete_then_gap_min_ms")?;
        let delete_then_gap_max_ms = cursor.optional_number_field("delete_then_gap_max_ms")?;
        let parsed_gap_mean = cursor.optional_number_field("delete_then_gap_mean_ms")?;
        let delete_then_gap_total_ms = cursor.number_field("delete_then_gap_total_ms")?;
        cursor.expect("}")?;
        cursor.finish()?;

        let counts = SessionSummaryCounts {
            commits,
            revisions,
            keys_complete_records,
            keys_incomplete_records,
            logical_key_actions,
            commits_with_internal_edit_keys,
            ambiguous_document_positions,
            direct_replacements,
            delete_then_insertions,
            restored_same_text,
            replaced_with_different_text,
            source_linked_candidates,
            delete_then_gap_count,
            delete_then_gap_min_ms,
            delete_then_gap_max_ms,
            delete_then_gap_total_ms,
        };
        let report = Self {
            elapsed_ms,
            candidate_gap_limit_ms,
            key_capture_requested,
            key_capture_ready,
            counts,
        };
        report.validate()?;
        if parsed_gap_mean != report.counts.delete_then_gap_mean_ms() {
            return Err(SessionSummaryError::InvalidInvariant(
                "stored delete-then gap mean does not match total/count",
            ));
        }
        Ok(report)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregatedSessionSummary {
    pub files: u64,
    pub total_elapsed_ms: u64,
    pub candidate_gap_limit_ms: u64,
    pub key_capture_requested: bool,
    pub key_capture_ready_sessions: u64,
    pub counts: SessionSummaryCounts,
}

impl AggregatedSessionSummary {
    pub fn from_reports(reports: &[SessionSummaryV1]) -> Result<Self, SessionSummaryError> {
        let first = reports
            .first()
            .ok_or(SessionSummaryError::InvalidInvariant(
                "at least one summary is required",
            ))?;
        first.validate()?;
        let mut aggregate = Self {
            files: 0,
            total_elapsed_ms: 0,
            candidate_gap_limit_ms: first.candidate_gap_limit_ms,
            key_capture_requested: first.key_capture_requested,
            key_capture_ready_sessions: 0,
            counts: SessionSummaryCounts::default(),
        };
        for report in reports {
            report.validate()?;
            if report.candidate_gap_limit_ms != aggregate.candidate_gap_limit_ms {
                return Err(SessionSummaryError::InconsistentConfiguration(
                    "candidate_gap_limit_ms",
                ));
            }
            if report.key_capture_requested != aggregate.key_capture_requested {
                return Err(SessionSummaryError::InconsistentConfiguration(
                    "key_capture_requested",
                ));
            }
            aggregate.files = aggregate
                .files
                .checked_add(1)
                .ok_or(SessionSummaryError::CounterOverflow("files"))?;
            aggregate.total_elapsed_ms = aggregate
                .total_elapsed_ms
                .checked_add(report.elapsed_ms)
                .ok_or(SessionSummaryError::CounterOverflow("total_elapsed_ms"))?;
            if report.key_capture_ready {
                aggregate.key_capture_ready_sessions =
                    aggregate.key_capture_ready_sessions.checked_add(1).ok_or(
                        SessionSummaryError::CounterOverflow("key_capture_ready_sessions"),
                    )?;
            }
            aggregate.counts.checked_merge(&report.counts)?;
        }
        Ok(aggregate)
    }

    pub fn terminal_line(&self) -> String {
        let counts = &self.counts;
        format!(
            "SUMMARY_REPORT contains_text=false files={} total_elapsed_ms={} \
             candidate_gap_limit_ms={} \
             key_capture_requested={} key_capture_ready_sessions={} commits={} revisions={} \
             keys_complete_records={} keys_incomplete_records={} logical_key_actions={} \
             commits_with_internal_edit_keys={} ambiguous_document_positions={} \
             direct_replacements={} delete_then_insertions={} restored_same_text={} \
             replaced_with_different_text={} source_linked_candidates={} \
             delete_then_gap_count={} delete_then_gap_min_ms={} delete_then_gap_max_ms={} \
             delete_then_gap_mean_ms={} delete_then_gap_total_ms={}",
            self.files,
            self.total_elapsed_ms,
            self.candidate_gap_limit_ms,
            self.key_capture_requested,
            self.key_capture_ready_sessions,
            counts.commits,
            counts.revisions,
            counts.keys_complete_records,
            counts.keys_incomplete_records,
            counts.logical_key_actions,
            counts.commits_with_internal_edit_keys,
            counts.ambiguous_document_positions,
            counts.direct_replacements,
            counts.delete_then_insertions,
            counts.restored_same_text,
            counts.replaced_with_different_text,
            counts.source_linked_candidates,
            counts.delete_then_gap_count,
            display_optional(counts.delete_then_gap_min_ms),
            display_optional(counts.delete_then_gap_max_ms),
            display_optional(counts.delete_then_gap_mean_ms()),
            counts.delete_then_gap_total_ms
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSummaryError {
    UnexpectedSyntax {
        offset: usize,
        expected: &'static str,
    },
    InvalidNumber {
        offset: usize,
    },
    InvalidInvariant(&'static str),
    InconsistentConfiguration(&'static str),
    CounterOverflow(&'static str),
}

impl fmt::Display for SessionSummaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedSyntax { offset, expected } => {
                write!(
                    formatter,
                    "invalid v1 summary syntax at byte {offset}; expected {expected}"
                )
            }
            Self::InvalidNumber { offset } => {
                write!(formatter, "invalid v1 summary number at byte {offset}")
            }
            Self::InvalidInvariant(message) => {
                write!(formatter, "invalid v1 summary invariant: {message}")
            }
            Self::InconsistentConfiguration(field) => {
                write!(
                    formatter,
                    "cannot aggregate summaries with different {field}"
                )
            }
            Self::CounterOverflow(field) => {
                write!(
                    formatter,
                    "v1 summary counter overflowed while adding {field}"
                )
            }
        }
    }
}

impl Error for SessionSummaryError {}

struct StrictV1Cursor<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> StrictV1Cursor<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn expect(&mut self, expected: &'static str) -> Result<(), SessionSummaryError> {
        if self.input[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            Ok(())
        } else {
            Err(SessionSummaryError::UnexpectedSyntax {
                offset: self.offset,
                expected,
            })
        }
    }

    fn number_field(&mut self, name: &'static str) -> Result<u64, SessionSummaryError> {
        self.expect_field(name)?;
        self.number()
    }

    fn bool_field(&mut self, name: &'static str) -> Result<bool, SessionSummaryError> {
        self.expect_field(name)?;
        if self.input[self.offset..].starts_with("true") {
            self.offset += 4;
            Ok(true)
        } else if self.input[self.offset..].starts_with("false") {
            self.offset += 5;
            Ok(false)
        } else {
            Err(SessionSummaryError::UnexpectedSyntax {
                offset: self.offset,
                expected: "a boolean",
            })
        }
    }

    fn optional_number_field(
        &mut self,
        name: &'static str,
    ) -> Result<Option<u64>, SessionSummaryError> {
        self.expect_field(name)?;
        if self.input[self.offset..].starts_with("null") {
            self.offset += 4;
            Ok(None)
        } else {
            self.number().map(Some)
        }
    }

    fn expect_field(&mut self, name: &'static str) -> Result<(), SessionSummaryError> {
        self.expect(",\"")?;
        if !self.input[self.offset..].starts_with(name) {
            return Err(SessionSummaryError::UnexpectedSyntax {
                offset: self.offset,
                expected: name,
            });
        }
        self.offset += name.len();
        self.expect("\":")
    }

    fn number(&mut self) -> Result<u64, SessionSummaryError> {
        let start = self.offset;
        while self
            .input
            .as_bytes()
            .get(self.offset)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.offset += 1;
        }
        if self.offset == start || (self.offset - start > 1 && self.input.as_bytes()[start] == b'0')
        {
            return Err(SessionSummaryError::InvalidNumber { offset: start });
        }
        self.input[start..self.offset]
            .parse()
            .map_err(|_| SessionSummaryError::InvalidNumber { offset: start })
    }

    fn finish(&self) -> Result<(), SessionSummaryError> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            Err(SessionSummaryError::UnexpectedSyntax {
                offset: self.offset,
                expected: "end of file",
            })
        }
    }
}

fn is_internal_edit_key(key: &RawKey) -> bool {
    match key {
        RawKey::Backspace | RawKey::Delete => true,
        RawKey::Shift(inner) => is_internal_edit_key(inner),
        _ => false,
    }
}

fn display_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

fn json_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        AggregatedSessionSummary, SessionSummaryCounts, SessionSummaryError, SessionSummaryV1,
    };

    fn report(elapsed_ms: u64, gap_ms: u64) -> SessionSummaryV1 {
        SessionSummaryV1 {
            elapsed_ms,
            candidate_gap_limit_ms: 15_000,
            key_capture_requested: true,
            key_capture_ready: true,
            counts: SessionSummaryCounts {
                commits: 2,
                revisions: 1,
                keys_complete_records: 3,
                logical_key_actions: 7,
                delete_then_insertions: 1,
                restored_same_text: 1,
                source_linked_candidates: 1,
                delete_then_gap_count: 1,
                delete_then_gap_min_ms: Some(gap_ms),
                delete_then_gap_max_ms: Some(gap_ms),
                delete_then_gap_total_ms: gap_ms,
                ..SessionSummaryCounts::default()
            },
        }
    }

    #[test]
    fn strict_v1_json_round_trips_without_private_strings() {
        let expected = report(4_491, 999);
        let json = expected.to_json().unwrap();
        assert!(
            json.starts_with("{\"schema\":\"ziranma-session-summary-v1\",\"contains_text\":false")
        );
        assert_eq!(SessionSummaryV1::from_json(&json), Ok(expected));
        assert_eq!(
            SessionSummaryV1::from_json(&format!("{json}\n")),
            Ok(report(4_491, 999))
        );
    }

    #[test]
    fn parser_rejects_reformatting_unknown_fields_and_invalid_invariants() {
        let valid = report(4_491, 999).to_json().unwrap();
        assert!(SessionSummaryV1::from_json(&format!(" {valid}")).is_err());
        assert!(
            SessionSummaryV1::from_json(
                &valid.replace("\"elapsed_ms\":4491", "\"unknown\":1,\"elapsed_ms\":4491")
            )
            .is_err()
        );
        assert!(
            SessionSummaryV1::from_json(
                &valid.replace("\"keys_complete_records\":3", "\"keys_complete_records\":2")
            )
            .is_err()
        );
        assert!(
            SessionSummaryV1::from_json(&valid.replace(
                "\"delete_then_gap_mean_ms\":999",
                "\"delete_then_gap_mean_ms\":998"
            ))
            .is_err()
        );
    }

    #[test]
    fn aggregate_merges_counts_and_gap_bounds_without_averaging_session_means() {
        let aggregate =
            AggregatedSessionSummary::from_reports(&[report(4_000, 500), report(6_000, 1_500)])
                .unwrap();
        assert_eq!(aggregate.files, 2);
        assert_eq!(aggregate.total_elapsed_ms, 10_000);
        assert_eq!(aggregate.counts.commits, 4);
        assert_eq!(aggregate.counts.delete_then_gap_count, 2);
        assert_eq!(aggregate.counts.delete_then_gap_min_ms, Some(500));
        assert_eq!(aggregate.counts.delete_then_gap_max_ms, Some(1_500));
        assert_eq!(aggregate.counts.delete_then_gap_mean_ms(), Some(1_000));
    }

    #[test]
    fn aggregate_rejects_empty_or_inconsistent_configuration() {
        assert_eq!(
            AggregatedSessionSummary::from_reports(&[]),
            Err(SessionSummaryError::InvalidInvariant(
                "at least one summary is required"
            ))
        );
        let first = report(1, 100);
        let mut different_gap = report(1, 100);
        different_gap.candidate_gap_limit_ms = 5_000;
        assert_eq!(
            AggregatedSessionSummary::from_reports(&[first.clone(), different_gap]),
            Err(SessionSummaryError::InconsistentConfiguration(
                "candidate_gap_limit_ms"
            ))
        );
        let mut different_capture = report(1, 100);
        different_capture.key_capture_requested = false;
        different_capture.key_capture_ready = false;
        assert_eq!(
            AggregatedSessionSummary::from_reports(&[first, different_capture]),
            Err(SessionSummaryError::InconsistentConfiguration(
                "key_capture_requested"
            ))
        );
    }
}
