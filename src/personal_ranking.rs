//! Bounded, local-only persistent ranking evidence for the personal TSF host.
//!
//! Explicit non-first selections are grouped into small immutable batches.
//! Batch plaintext is protected through the caller-owned `DataProtector`
//! before any file is created. Multiple host processes append independent
//! packages, so they never overwrite one another's newly observed evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{DataProtector, candidate_sha256_hex};

pub const PERSONAL_RANKING_BATCH_SCHEMA_V1: &str = "ziranma-personal-ranking-batch-v1";
pub const PERSONAL_RANKING_BATCH_EXTENSION: &str = "zpr";
pub const PERSONAL_RANKING_CHECKPOINT_SCHEMA_V1: &str = "ziranma-personal-ranking-checkpoint-v1";
pub const PERSONAL_RANKING_CHECKPOINT_EXTENSION: &str = "zpc";
pub const PERSONAL_RANKING_SUPPRESSION_ACTION_SCHEMA_V1: &str =
    "ziranma-personal-ranking-suppression-action-v1";
pub const PERSONAL_RANKING_SUPPRESSION_ACTION_EXTENSION: &str = "zps";
pub const PERSONAL_RANKING_SUPPRESSION_DIRECTORY: &str = "personal-suppression";
pub const MAX_PERSONAL_RANKING_BATCH_EVENTS: usize = 32;
pub const MAX_PERSONAL_RANKING_BATCH_FILES: usize = 4_096;
pub const MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_FILES: usize = 4_096;
pub const MAX_PERSONAL_RANKING_CHECKPOINT_FILES: usize = 32;
pub const MAX_PERSONAL_RANKING_ENTRIES: usize = 8_192;
pub const MAX_PERSONAL_RANKING_PLAINTEXT_BYTES: usize = 64 * 1024;
pub const MAX_PERSONAL_RANKING_PROTECTED_BYTES: usize = 128 * 1024;
pub const MAX_PERSONAL_RANKING_CHECKPOINT_PLAINTEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES: usize = 16 * 1024 * 1024;
pub const MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES: usize = 64;

const MAX_PERSONAL_RANKING_CODE_BYTES: usize = 64;
const MAX_PERSONAL_RANKING_TEXT_BYTES: usize = 512;
const MAX_PERSONAL_RANKING_TEXT_CHARACTERS: usize = 128;
// Four confirmations are enough to establish maximum ordering support. This
// keeps an old preference resistant to one incidental choice without making a
// later deliberate change require an unbounded number of repetitions.
const PERSONAL_RANKING_SUPPORT_CAP: u64 = 4;
const PERSONAL_ABBREVIATION_DISCOVERY_MIN_SELECTIONS: u64 = 2;
const PROTECTED_PERSONAL_RANKING_MAGIC: &[u8] = b"ziranma-personal-ranking-dpapi-v1\0";
const PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC: &[u8] =
    b"ziranma-personal-ranking-checkpoint-dpapi-v1\0";
const MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PLAINTEXT_BYTES: usize = 4 * 1024;
const MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES: usize = 8 * 1024;
const PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC: &[u8] =
    b"ziranma-personal-ranking-suppression-action-dpapi-v1\0";

/// Returns whether `abbreviated_code` is the full-prefix plus abbreviated-tail
/// spelling of `full_code`.
///
/// A complete double-pinyin code contributes two keys per syllable. This
/// relation keeps at least the first syllable complete and replaces every
/// syllable in one non-empty suffix with its first key. It therefore accepts
/// `jdjd -> jdj`, but not arbitrary string prefixes such as `jdjd -> jd`, nor
/// a leading abbreviation such as `jdjd -> jjd`.
pub(crate) fn is_anchored_suffix_abbreviation(full_code: &str, abbreviated_code: &str) -> bool {
    let full = full_code.as_bytes();
    let abbreviated = abbreviated_code.as_bytes();
    if full.len() < 4
        || !full.len().is_multiple_of(2)
        || !full.iter().all(u8::is_ascii_lowercase)
        || !abbreviated.iter().all(u8::is_ascii_lowercase)
    {
        return false;
    }
    let syllable_count = full.len() / 2;
    let Some(complete_prefix_syllables) = abbreviated.len().checked_sub(syllable_count) else {
        return false;
    };
    if complete_prefix_syllables == 0 || complete_prefix_syllables >= syllable_count {
        return false;
    }
    let expected_len = complete_prefix_syllables
        .saturating_mul(2)
        .saturating_add(syllable_count - complete_prefix_syllables);
    if abbreviated.len() != expected_len {
        return false;
    }
    let complete_prefix_bytes = complete_prefix_syllables * 2;
    if abbreviated[..complete_prefix_bytes] != full[..complete_prefix_bytes] {
        return false;
    }
    abbreviated[complete_prefix_bytes..]
        .iter()
        .zip(full[complete_prefix_bytes..].chunks_exact(2))
        .all(|(observed, complete)| *observed == complete[0])
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalRankingSelection {
    code: String,
    text: String,
}

impl PersonalRankingSelection {
    pub fn new(code: &str, text: &str) -> Result<Self, PersonalRankingError> {
        validate_selection(code, text)?;
        Ok(Self {
            code: code.to_owned(),
            text: text.to_owned(),
        })
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalRankingBatch {
    created_unix_ms: u64,
    process_id: u32,
    sequence: u64,
    selections: Vec<PersonalRankingSelection>,
}

impl PersonalRankingBatch {
    pub fn new(
        created_unix_ms: u64,
        process_id: u32,
        sequence: u64,
        selections: Vec<PersonalRankingSelection>,
    ) -> Result<Self, PersonalRankingError> {
        if selections.is_empty() || selections.len() > MAX_PERSONAL_RANKING_BATCH_EVENTS {
            return Err(PersonalRankingError::InvalidEventCount);
        }
        for selection in &selections {
            validate_selection(selection.code(), selection.text())?;
        }
        Ok(Self {
            created_unix_ms,
            process_id,
            sequence,
            selections,
        })
    }

    pub fn now(
        process_id: u32,
        sequence: u64,
        selections: Vec<PersonalRankingSelection>,
    ) -> Result<Self, PersonalRankingError> {
        let created_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PersonalRankingError::Clock)?
            .as_millis()
            .try_into()
            .map_err(|_| PersonalRankingError::Clock)?;
        Self::new(created_unix_ms, process_id, sequence, selections)
    }

    pub fn selection_count(&self) -> usize {
        self.selections.len()
    }

    fn ordering_key(&self) -> (u64, u32, u64) {
        (self.created_unix_ms, self.process_id, self.sequence)
    }

    fn render(&self) -> Result<Vec<u8>, PersonalRankingError> {
        let mut payload = String::new();
        for selection in &self.selections {
            validate_selection(selection.code(), selection.text())?;
            payload.push_str(selection.code());
            payload.push('\t');
            payload.push_str(selection.text());
            payload.push('\n');
        }
        let output = format!(
            "schema={PERSONAL_RANKING_BATCH_SCHEMA_V1}\ncreated_unix_ms={}\nprocess_id={}\nsequence={}\nevent_count={}\npayload_bytes={}\n\n{payload}",
            self.created_unix_ms,
            self.process_id,
            self.sequence,
            self.selections.len(),
            payload.len()
        )
        .into_bytes();
        if output.len() > MAX_PERSONAL_RANKING_PLAINTEXT_BYTES {
            return Err(PersonalRankingError::InvalidPlaintextSize);
        }
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, PersonalRankingError> {
        if input.is_empty() || input.len() > MAX_PERSONAL_RANKING_PLAINTEXT_BYTES {
            return Err(PersonalRankingError::InvalidPlaintextSize);
        }
        let input = std::str::from_utf8(input).map_err(|_| PersonalRankingError::InvalidUtf8)?;
        if input.contains('\r') || !input.ends_with('\n') {
            return Err(PersonalRankingError::InvalidPlaintextStructure);
        }
        let (header, payload) = input
            .split_once("\n\n")
            .ok_or(PersonalRankingError::InvalidPlaintextStructure)?;
        let lines = header.split('\n').collect::<Vec<_>>();
        if lines.len() != 6 || field(lines[0], "schema")? != PERSONAL_RANKING_BATCH_SCHEMA_V1 {
            return Err(PersonalRankingError::InvalidPlaintextStructure);
        }
        let created_unix_ms = parse_canonical_u64(field(lines[1], "created_unix_ms")?)?;
        let process_id = parse_canonical_u32(field(lines[2], "process_id")?)?;
        let sequence = parse_canonical_u64(field(lines[3], "sequence")?)?;
        let event_count = parse_canonical_usize(field(lines[4], "event_count")?)?;
        let payload_bytes = parse_canonical_usize(field(lines[5], "payload_bytes")?)?;
        if event_count == 0 || event_count > MAX_PERSONAL_RANKING_BATCH_EVENTS {
            return Err(PersonalRankingError::InvalidEventCount);
        }
        if payload.len() != payload_bytes {
            return Err(PersonalRankingError::PayloadLengthMismatch);
        }
        let mut selections = Vec::with_capacity(event_count);
        for line in payload.strip_suffix('\n').unwrap_or(payload).split('\n') {
            let (code, text) = line
                .split_once('\t')
                .ok_or(PersonalRankingError::InvalidEntry)?;
            selections.push(PersonalRankingSelection::new(code, text)?);
        }
        if selections.len() != event_count {
            return Err(PersonalRankingError::EventCountMismatch);
        }
        Self::new(created_unix_ms, process_id, sequence, selections)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PersonalRankingEvidence {
    selections: u64,
    last_generation: u64,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct PersonalRankingSnapshot {
    entries: BTreeMap<(String, String), PersonalRankingEvidence>,
    generation: u64,
}

/// Explicit per-identity masks applied after positive ranking evidence.
///
/// The snapshot deliberately exposes no iterator or serializer. A caller can
/// ask about one already-known `code + text` identity, but cannot accidentally
/// dump all private entries through a diagnostic surface.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct PersonalRankingSuppressionSnapshot {
    entries: BTreeMap<String, BTreeSet<String>>,
    entry_count: usize,
}

impl fmt::Debug for PersonalRankingSuppressionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonalRankingSuppressionSnapshot")
            .field("entries", &self.entry_count)
            .field("debug_contains_text", &false)
            .finish()
    }
}

impl PersonalRankingSuppressionSnapshot {
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub fn suppress(&mut self, code: &str, text: &str) -> Result<bool, PersonalRankingError> {
        validate_selection(code, text)?;
        if self.is_suppressed(code, text) {
            return Ok(false);
        }
        if self.entry_count >= MAX_PERSONAL_RANKING_ENTRIES {
            return Err(PersonalRankingError::TooManySuppressions);
        }
        self.entries
            .entry(code.to_owned())
            .or_default()
            .insert(text.to_owned());
        self.entry_count += 1;
        Ok(true)
    }

    pub fn restore(&mut self, code: &str, text: &str) -> Result<bool, PersonalRankingError> {
        validate_selection(code, text)?;
        let Some(texts) = self.entries.get_mut(code) else {
            return Ok(false);
        };
        if !texts.remove(text) {
            return Ok(false);
        }
        self.entry_count -= 1;
        if texts.is_empty() {
            self.entries.remove(code);
        }
        Ok(true)
    }

    pub fn is_suppressed(&self, code: &str, text: &str) -> bool {
        self.entries
            .get(code)
            .is_some_and(|texts| texts.contains(text))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersonalRankingSuppressionActionKind {
    Suppress,
    Restore,
}

impl PersonalRankingSuppressionActionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Suppress => "suppress",
            Self::Restore => "restore",
        }
    }

    fn parse(value: &str) -> Result<Self, PersonalRankingError> {
        match value {
            "suppress" => Ok(Self::Suppress),
            "restore" => Ok(Self::Restore),
            _ => Err(PersonalRankingError::InvalidSuppressionAction),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PersonalRankingSuppressionAction {
    created_unix_ms: u64,
    process_id: u32,
    sequence: u64,
    kind: PersonalRankingSuppressionActionKind,
    code: String,
    text: String,
}

impl fmt::Debug for PersonalRankingSuppressionAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonalRankingSuppressionAction")
            .field("created_unix_ms", &self.created_unix_ms)
            .field("process_id", &self.process_id)
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("debug_contains_text", &false)
            .finish()
    }
}

impl PersonalRankingSuppressionAction {
    pub fn new(
        created_unix_ms: u64,
        process_id: u32,
        sequence: u64,
        kind: PersonalRankingSuppressionActionKind,
        code: &str,
        text: &str,
    ) -> Result<Self, PersonalRankingError> {
        validate_selection(code, text)?;
        Ok(Self {
            created_unix_ms,
            process_id,
            sequence,
            kind,
            code: code.to_owned(),
            text: text.to_owned(),
        })
    }

    pub fn now(
        process_id: u32,
        sequence: u64,
        kind: PersonalRankingSuppressionActionKind,
        code: &str,
        text: &str,
    ) -> Result<Self, PersonalRankingError> {
        let created_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PersonalRankingError::Clock)?
            .as_millis()
            .try_into()
            .map_err(|_| PersonalRankingError::Clock)?;
        Self::new(created_unix_ms, process_id, sequence, kind, code, text)
    }

    pub fn kind(&self) -> PersonalRankingSuppressionActionKind {
        self.kind
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn ordering_key(&self) -> (u64, u32, u64) {
        (self.created_unix_ms, self.process_id, self.sequence)
    }

    fn render(&self) -> Result<Vec<u8>, PersonalRankingError> {
        validate_selection(&self.code, &self.text)?;
        let payload = format!("{}\t{}\n", self.code, self.text);
        let output = format!(
            "schema={PERSONAL_RANKING_SUPPRESSION_ACTION_SCHEMA_V1}\ncreated_unix_ms={}\nprocess_id={}\nsequence={}\naction={}\npayload_bytes={}\n\n{payload}",
            self.created_unix_ms,
            self.process_id,
            self.sequence,
            self.kind.as_str(),
            payload.len(),
        )
        .into_bytes();
        if output.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PLAINTEXT_BYTES {
            return Err(PersonalRankingError::InvalidSuppressionActionSize);
        }
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, PersonalRankingError> {
        if input.is_empty() || input.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PLAINTEXT_BYTES
        {
            return Err(PersonalRankingError::InvalidSuppressionActionSize);
        }
        let input = std::str::from_utf8(input).map_err(|_| PersonalRankingError::InvalidUtf8)?;
        if input.contains('\r') || !input.ends_with('\n') {
            return Err(PersonalRankingError::InvalidSuppressionAction);
        }
        let (header, payload) = input
            .split_once("\n\n")
            .ok_or(PersonalRankingError::InvalidSuppressionAction)?;
        let lines = header.split('\n').collect::<Vec<_>>();
        if lines.len() != 6
            || field(lines[0], "schema")? != PERSONAL_RANKING_SUPPRESSION_ACTION_SCHEMA_V1
        {
            return Err(PersonalRankingError::InvalidSuppressionAction);
        }
        let created_unix_ms = parse_canonical_u64(field(lines[1], "created_unix_ms")?)?;
        let process_id = parse_canonical_u32(field(lines[2], "process_id")?)?;
        let sequence = parse_canonical_u64(field(lines[3], "sequence")?)?;
        let kind = PersonalRankingSuppressionActionKind::parse(field(lines[4], "action")?)?;
        let payload_bytes = parse_canonical_usize(field(lines[5], "payload_bytes")?)?;
        if payload.len() != payload_bytes {
            return Err(PersonalRankingError::PayloadLengthMismatch);
        }
        let payload = payload
            .strip_suffix('\n')
            .ok_or(PersonalRankingError::InvalidSuppressionAction)?;
        let (code, text) = payload
            .split_once('\t')
            .ok_or(PersonalRankingError::InvalidSuppressionAction)?;
        let action = Self::new(created_unix_ms, process_id, sequence, kind, code, text)?;
        if action.render()?.as_slice() != input.as_bytes() {
            return Err(PersonalRankingError::InvalidSuppressionAction);
        }
        Ok(action)
    }

    fn apply_to(
        &self,
        snapshot: &mut PersonalRankingSuppressionSnapshot,
    ) -> Result<(), PersonalRankingError> {
        match self.kind {
            PersonalRankingSuppressionActionKind::Suppress => {
                snapshot.suppress(&self.code, &self.text)?;
            }
            PersonalRankingSuppressionActionKind::Restore => {
                snapshot.restore(&self.code, &self.text)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct LoadedPersonalRankingSuppressions {
    snapshot: PersonalRankingSuppressionSnapshot,
    action_count: usize,
    package_names: BTreeSet<String>,
    last_ordering_key: Option<(u64, u32, u64, String)>,
}

impl fmt::Debug for LoadedPersonalRankingSuppressions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedPersonalRankingSuppressions")
            .field("snapshot", &self.snapshot)
            .field("action_count", &self.action_count)
            .field("package_count", &self.package_names.len())
            .finish()
    }
}

impl LoadedPersonalRankingSuppressions {
    pub fn snapshot(&self) -> &PersonalRankingSuppressionSnapshot {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> PersonalRankingSuppressionSnapshot {
        self.snapshot
    }

    pub fn action_count(&self) -> usize {
        self.action_count
    }
}

impl fmt::Debug for PersonalRankingSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonalRankingSnapshot")
            .field("entries", &self.entries.len())
            .field("generation", &self.generation)
            .field("debug_contains_text", &false)
            .finish()
    }
}

impl PersonalRankingSnapshot {
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn has_evidence(&self, code: &str, text: &str) -> bool {
        self.entries_for_code(code)
            .any(|((_, entry_text), _)| entry_text == text)
    }

    pub fn record(&mut self, code: &str, text: &str) -> Result<(), PersonalRankingError> {
        validate_selection(code, text)?;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(PersonalRankingError::GenerationOverflow)?;
        let entry = self
            .entries
            .entry((code.to_owned(), text.to_owned()))
            .or_insert(PersonalRankingEvidence {
                selections: 0,
                last_generation: self.generation,
            });
        entry.selections = entry.selections.saturating_add(1);
        entry.last_generation = self.generation;
        if self.entries.len() > MAX_PERSONAL_RANKING_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|((code, text), evidence)| {
                    (evidence.last_generation, code.as_str(), text.as_str())
                })
                .map(|(identity, _)| identity.clone())
                .expect("an over-capacity ranking snapshot has one oldest entry");
            self.entries.remove(&oldest);
        }
        Ok(())
    }

    pub fn apply_batch(
        &mut self,
        batch: &PersonalRankingBatch,
    ) -> Result<(), PersonalRankingError> {
        for selection in &batch.selections {
            self.record(selection.code(), selection.text())?;
        }
        Ok(())
    }

    pub fn preferred_text(&self, code: &str) -> Option<&str> {
        self.preferred_text_where(code, |_| true)
    }

    pub fn preferred_text_with_suppressions(
        &self,
        code: &str,
        suppressions: &PersonalRankingSuppressionSnapshot,
    ) -> Option<&str> {
        self.preferred_text_where(code, |text| !suppressions.is_suppressed(code, text))
    }

    fn preferred_text_where(
        &self,
        code: &str,
        mut allowed: impl FnMut(&str) -> bool,
    ) -> Option<&str> {
        self.entries_for_code(code)
            .filter(|((_, text), _)| allowed(text))
            .max_by(|((_, left_text), left), ((_, right_text), right)| {
                left.selections
                    .min(PERSONAL_RANKING_SUPPORT_CAP)
                    .cmp(&right.selections.min(PERSONAL_RANKING_SUPPORT_CAP))
                    .then_with(|| left.last_generation.cmp(&right.last_generation))
                    .then_with(|| left.selections.cmp(&right.selections))
                    .then_with(|| right_text.cmp(left_text))
            })
            .map(|((_, text), _)| text.as_str())
    }

    fn entries_for_code<'a>(
        &'a self,
        code: &str,
    ) -> impl Iterator<Item = (&'a (String, String), &'a PersonalRankingEvidence)> + 'a {
        // Tuple ordering keeps every text for one code contiguous. Starting at
        // the empty-text lower bound avoids scanning unrelated personal codes
        // while preserving the single canonical storage map.
        let code = code.to_owned();
        let start = (code.clone(), String::new());
        self.entries
            .range(start..)
            .take_while(move |((entry_code, _), _)| entry_code == &code)
    }

    fn entries_for_anchored_code<'a>(
        &'a self,
        abbreviated_code: &str,
    ) -> impl Iterator<Item = (&'a (String, String), &'a PersonalRankingEvidence)> + 'a {
        // Every accepted anchored abbreviation keeps its first complete
        // syllable. Tuple ordering therefore lets us restrict the scan to
        // source codes sharing those first two keys before checking the full
        // structural relation.
        let abbreviated_code = abbreviated_code.to_owned();
        let prefix = (abbreviated_code.len() >= 3
            && abbreviated_code
                .as_bytes()
                .iter()
                .all(u8::is_ascii_lowercase))
        .then(|| abbreviated_code[..2].to_owned());
        let start = (prefix.clone().unwrap_or_default(), String::new());
        self.entries
            .range(start..)
            .take_while(move |((entry_code, _), _)| {
                prefix
                    .as_ref()
                    .is_some_and(|prefix| entry_code.starts_with(prefix))
            })
            .filter(move |((entry_code, _), _)| {
                is_anchored_suffix_abbreviation(entry_code, &abbreviated_code)
            })
    }

    pub fn promote_texts_after(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> bool {
        let Some(preferred) = self.preferred_text(code) else {
            return false;
        };
        promote_preferred_text(preferred, candidates, protected_prefix)
    }

    pub fn promote_texts_after_with_suppressions(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        suppressions: &PersonalRankingSuppressionSnapshot,
    ) -> bool {
        let Some(preferred) = self.preferred_text_with_suppressions(code, suppressions) else {
            return false;
        };
        promote_preferred_text(preferred, candidates, protected_prefix)
    }

    /// Promotes evidence learned under a verified complete code into one
    /// structurally compatible anchored-tail abbreviation.
    ///
    /// The inherited text must already exist in the caller's ordinary
    /// candidate pool. `exact_full_code_candidate` lets the host prove that
    /// the source evidence really names a complete public-dictionary spelling;
    /// arbitrary aliases and freely segmented sentences therefore do not
    /// become code-family evidence merely because their key length is even.
    pub(crate) fn promote_anchored_suffix_texts_after_with_suppressions(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        suppressions: &PersonalRankingSuppressionSnapshot,
        mut exact_full_code_candidate: impl FnMut(&str, &str) -> bool,
    ) -> bool {
        let Some(preferred) = self.preferred_anchored_suffix_text_where(
            code,
            candidates,
            suppressions,
            &mut exact_full_code_candidate,
        ) else {
            return false;
        };
        promote_preferred_text(preferred, candidates, protected_prefix)
    }

    pub(crate) fn has_anchored_suffix_evidence_with_suppressions(
        &self,
        code: &str,
        text: &str,
        suppressions: &PersonalRankingSuppressionSnapshot,
        mut exact_full_code_candidate: impl FnMut(&str, &str) -> bool,
    ) -> bool {
        if suppressions.is_suppressed(code, text) {
            return false;
        }
        self.entries_for_anchored_code(code)
            .any(|((entry_code, entry_text), _)| {
                entry_text == text
                    && !suppressions.is_suppressed(entry_code, entry_text)
                    && exact_full_code_candidate(entry_code, entry_text)
            })
    }

    /// Recalls one repeatedly confirmed personal character composition into
    /// its structurally compatible anchored-tail abbreviation.
    ///
    /// Unlike public exact-code inheritance, this lane may insert an absent
    /// candidate, but only after repeated evidence and caller verification of
    /// every complete-code character. It preserves the first unprotected
    /// ordinary candidate as a discovery guard; one explicit short-code
    /// selection can then establish normal exact-code evidence.
    pub(crate) fn recall_repeated_anchored_suffix_text_after_with_suppressions(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        suppressions: &PersonalRankingSuppressionSnapshot,
        mut eligible_character_composition: impl FnMut(&str, &str) -> bool,
    ) -> Option<usize> {
        let preferred = self.preferred_repeated_anchored_suffix_text_where(
            code,
            suppressions,
            &mut eligible_character_composition,
        )?;
        recall_preferred_text_after_first_ordinary(preferred, candidates, protected_prefix)
    }

    pub(crate) fn has_repeated_anchored_suffix_evidence_with_suppressions(
        &self,
        code: &str,
        text: &str,
        suppressions: &PersonalRankingSuppressionSnapshot,
        mut eligible_character_composition: impl FnMut(&str, &str) -> bool,
    ) -> bool {
        if suppressions.is_suppressed(code, text) {
            return false;
        }
        self.entries_for_anchored_code(code)
            .any(|((entry_code, entry_text), evidence)| {
                entry_text == text
                    && evidence.selections >= PERSONAL_ABBREVIATION_DISCOVERY_MIN_SELECTIONS
                    && !suppressions.is_suppressed(entry_code, entry_text)
                    && eligible_character_composition(entry_code, entry_text)
            })
    }

    fn preferred_anchored_suffix_text_where<'a>(
        &'a self,
        code: &str,
        candidates: &[String],
        suppressions: &PersonalRankingSuppressionSnapshot,
        exact_full_code_candidate: &mut impl FnMut(&str, &str) -> bool,
    ) -> Option<&'a str> {
        self.entries_for_anchored_code(code)
            .filter(|((entry_code, text), _)| {
                candidates.iter().any(|candidate| candidate == text)
                    && !suppressions.is_suppressed(code, text)
                    && !suppressions.is_suppressed(entry_code, text)
                    && exact_full_code_candidate(entry_code, text)
            })
            .max_by(|((_, left_text), left), ((_, right_text), right)| {
                left.selections
                    .min(PERSONAL_RANKING_SUPPORT_CAP)
                    .cmp(&right.selections.min(PERSONAL_RANKING_SUPPORT_CAP))
                    .then_with(|| left.last_generation.cmp(&right.last_generation))
                    .then_with(|| left.selections.cmp(&right.selections))
                    .then_with(|| right_text.cmp(left_text))
            })
            .map(|((_, text), _)| text.as_str())
    }

    fn preferred_repeated_anchored_suffix_text_where<'a>(
        &'a self,
        code: &str,
        suppressions: &PersonalRankingSuppressionSnapshot,
        eligible_character_composition: &mut impl FnMut(&str, &str) -> bool,
    ) -> Option<&'a str> {
        self.entries_for_anchored_code(code)
            .filter(|((entry_code, text), evidence)| {
                evidence.selections >= PERSONAL_ABBREVIATION_DISCOVERY_MIN_SELECTIONS
                    && !suppressions.is_suppressed(code, text)
                    && !suppressions.is_suppressed(entry_code, text)
                    && eligible_character_composition(entry_code, text)
            })
            .max_by(|((_, left_text), left), ((_, right_text), right)| {
                left.selections
                    .min(PERSONAL_RANKING_SUPPORT_CAP)
                    .cmp(&right.selections.min(PERSONAL_RANKING_SUPPORT_CAP))
                    .then_with(|| left.last_generation.cmp(&right.last_generation))
                    .then_with(|| left.selections.cmp(&right.selections))
                    .then_with(|| right_text.cmp(left_text))
            })
            .map(|((_, text), _)| text.as_str())
    }
}

fn promote_preferred_text(
    preferred: &str,
    candidates: &mut Vec<String>,
    protected_prefix: usize,
) -> bool {
    let protected_prefix = protected_prefix.min(candidates.len());
    let Some(index) = candidates
        .iter()
        .position(|candidate| candidate == preferred)
    else {
        if protected_prefix > 0 && protected_prefix == candidates.len() {
            return false;
        }
        let original_len = candidates.len();
        candidates.insert(protected_prefix, preferred.to_owned());
        candidates.truncate(original_len.max(1));
        return true;
    };
    if index <= protected_prefix {
        return true;
    }
    let candidate = candidates.remove(index);
    candidates.insert(protected_prefix, candidate);
    true
}

fn recall_preferred_text_after_first_ordinary(
    preferred: &str,
    candidates: &mut Vec<String>,
    protected_prefix: usize,
) -> Option<usize> {
    let protected_prefix = protected_prefix.min(candidates.len());
    let Some(index) = candidates
        .iter()
        .position(|candidate| candidate == preferred)
    else {
        if candidates.is_empty() {
            candidates.push(preferred.to_owned());
            return Some(0);
        }
        if protected_prefix == candidates.len() || candidates.len() < 2 {
            return None;
        }
        let original_len = candidates.len();
        let discovery_index = protected_prefix.saturating_add(1).min(original_len);
        candidates.insert(discovery_index, preferred.to_owned());
        candidates.truncate(original_len);
        return (discovery_index < candidates.len()).then_some(discovery_index);
    };
    let discovery_index = protected_prefix
        .saturating_add(usize::from(protected_prefix < candidates.len()))
        .min(candidates.len().saturating_sub(1));
    if index <= discovery_index {
        return Some(index);
    }
    let candidate = candidates.remove(index);
    candidates.insert(discovery_index, candidate);
    Some(discovery_index)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadedPersonalRanking {
    snapshot: PersonalRankingSnapshot,
    batch_count: usize,
    selection_count: usize,
    package_names: BTreeSet<String>,
    last_ordering_key: Option<(u64, u32, u64, String)>,
    checkpoint_batch_count: usize,
}

impl LoadedPersonalRanking {
    pub fn snapshot(&self) -> &PersonalRankingSnapshot {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> PersonalRankingSnapshot {
        self.snapshot
    }

    pub fn batch_count(&self) -> usize {
        self.batch_count
    }

    pub fn selection_count(&self) -> usize {
        self.selection_count
    }

    pub fn checkpoint_batch_count(&self) -> usize {
        self.checkpoint_batch_count
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PersonalRankingCheckpoint {
    created_unix_ms: u64,
    covered_package_names: BTreeSet<String>,
    last_ordering_key: (u64, u32, u64, String),
    selection_count: usize,
    snapshot: PersonalRankingSnapshot,
}

impl PersonalRankingCheckpoint {
    fn from_loaded(loaded: &LoadedPersonalRanking) -> Result<Self, PersonalRankingError> {
        if loaded.batch_count < MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES
            || loaded.batch_count != loaded.package_names.len()
            || loaded.snapshot.entry_count() == 0
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let last_ordering_key = loaded
            .last_ordering_key
            .clone()
            .ok_or(PersonalRankingError::InvalidCheckpoint)?;
        let created_unix_ms = last_ordering_key.0;
        Ok(Self {
            created_unix_ms,
            covered_package_names: loaded.package_names.clone(),
            last_ordering_key,
            selection_count: loaded.selection_count,
            snapshot: loaded.snapshot.clone(),
        })
    }

    fn render(&self) -> Result<Vec<u8>, PersonalRankingError> {
        self.validate()?;
        let mut entries = String::new();
        for ((code, text), evidence) in &self.snapshot.entries {
            entries.push_str(code);
            entries.push('\t');
            entries.push_str(text);
            entries.push('\t');
            entries.push_str(&evidence.selections.to_string());
            entries.push('\t');
            entries.push_str(&evidence.last_generation.to_string());
            entries.push('\n');
        }
        let packages = checkpoint_coverage_body(&self.covered_package_names);
        let (last_created_unix_ms, last_process_id, last_sequence, last_package) =
            &self.last_ordering_key;
        let header = format!(
            "schema={PERSONAL_RANKING_CHECKPOINT_SCHEMA_V1}\ncreated_unix_ms={}\nbatch_count={}\nselection_count={}\nentry_count={}\ngeneration={}\nlast_created_unix_ms={last_created_unix_ms}\nlast_process_id={last_process_id}\nlast_sequence={last_sequence}\nlast_package={last_package}\nentries_bytes={}\npackages_bytes={}\n\n",
            self.created_unix_ms,
            self.covered_package_names.len(),
            self.selection_count,
            self.snapshot.entry_count(),
            self.snapshot.generation(),
            entries.len(),
            packages.len(),
        );
        let mut output = Vec::with_capacity(header.len() + entries.len() + packages.len());
        output.extend_from_slice(header.as_bytes());
        output.extend_from_slice(entries.as_bytes());
        output.extend_from_slice(packages.as_bytes());
        if output.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PLAINTEXT_BYTES {
            return Err(PersonalRankingError::InvalidCheckpointSize);
        }
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, PersonalRankingError> {
        if input.is_empty() || input.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PLAINTEXT_BYTES {
            return Err(PersonalRankingError::InvalidCheckpointSize);
        }
        let input = std::str::from_utf8(input).map_err(|_| PersonalRankingError::InvalidUtf8)?;
        if input.contains('\r') || !input.ends_with('\n') {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let (header, payload) = input
            .split_once("\n\n")
            .ok_or(PersonalRankingError::InvalidCheckpoint)?;
        let lines = header.split('\n').collect::<Vec<_>>();
        if lines.len() != 12 || field(lines[0], "schema")? != PERSONAL_RANKING_CHECKPOINT_SCHEMA_V1
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let created_unix_ms = parse_canonical_u64(field(lines[1], "created_unix_ms")?)?;
        let batch_count = parse_canonical_usize(field(lines[2], "batch_count")?)?;
        let selection_count = parse_canonical_usize(field(lines[3], "selection_count")?)?;
        let entry_count = parse_canonical_usize(field(lines[4], "entry_count")?)?;
        let generation = parse_canonical_u64(field(lines[5], "generation")?)?;
        let last_created_unix_ms = parse_canonical_u64(field(lines[6], "last_created_unix_ms")?)?;
        let last_process_id = parse_canonical_u32(field(lines[7], "last_process_id")?)?;
        let last_sequence = parse_canonical_u64(field(lines[8], "last_sequence")?)?;
        let last_package = field(lines[9], "last_package")?.to_owned();
        let entries_bytes = parse_canonical_usize(field(lines[10], "entries_bytes")?)?;
        let packages_bytes = parse_canonical_usize(field(lines[11], "packages_bytes")?)?;
        if u64::try_from(selection_count).ok() != Some(generation) {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        if entries_bytes
            .checked_add(packages_bytes)
            .filter(|total| *total == payload.len())
            .is_none()
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let (entries, packages) = payload.as_bytes().split_at(entries_bytes);
        let entries =
            std::str::from_utf8(entries).map_err(|_| PersonalRankingError::InvalidUtf8)?;
        let packages =
            std::str::from_utf8(packages).map_err(|_| PersonalRankingError::InvalidUtf8)?;
        let mut snapshot = PersonalRankingSnapshot {
            entries: BTreeMap::new(),
            generation,
        };
        if entries.is_empty() || !entries.ends_with('\n') {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        for line in entries.strip_suffix('\n').unwrap_or(entries).split('\n') {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 4 {
                return Err(PersonalRankingError::InvalidCheckpoint);
            }
            validate_selection(fields[0], fields[1])?;
            let selections = parse_canonical_u64(fields[2])?;
            let last_generation = parse_canonical_u64(fields[3])?;
            if selections == 0 || last_generation == 0 || last_generation > generation {
                return Err(PersonalRankingError::InvalidCheckpoint);
            }
            if snapshot
                .entries
                .insert(
                    (fields[0].to_owned(), fields[1].to_owned()),
                    PersonalRankingEvidence {
                        selections,
                        last_generation,
                    },
                )
                .is_some()
            {
                return Err(PersonalRankingError::InvalidCheckpoint);
            }
        }
        let retained_selections = snapshot
            .entries
            .values()
            .try_fold(0_u64, |total, evidence| {
                total.checked_add(evidence.selections)
            })
            .ok_or(PersonalRankingError::InvalidCheckpoint)?;
        if snapshot.entry_count() != entry_count
            || snapshot.entry_count() == 0
            || snapshot.entry_count() > MAX_PERSONAL_RANKING_ENTRIES
            || snapshot
                .entries
                .values()
                .map(|evidence| evidence.last_generation)
                .max()
                != Some(generation)
            || retained_selections > u64::try_from(selection_count).unwrap_or(u64::MAX)
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        if packages.is_empty() || !packages.ends_with('\n') {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let mut covered_package_names = BTreeSet::new();
        for name in packages.strip_suffix('\n').unwrap_or(packages).split('\n') {
            if !valid_package_file_name(name) || !covered_package_names.insert(name.to_owned()) {
                return Err(PersonalRankingError::InvalidCheckpoint);
            }
        }
        if covered_package_names.len() != batch_count
            || !(MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES..=MAX_PERSONAL_RANKING_BATCH_FILES)
                .contains(&batch_count)
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        let checkpoint = Self {
            created_unix_ms,
            covered_package_names,
            last_ordering_key: (
                last_created_unix_ms,
                last_process_id,
                last_sequence,
                last_package,
            ),
            selection_count,
            snapshot,
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    fn validate(&self) -> Result<(), PersonalRankingError> {
        if !(MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES..=MAX_PERSONAL_RANKING_BATCH_FILES)
            .contains(&self.covered_package_names.len())
            || self.selection_count == 0
            || self.snapshot.entry_count() == 0
            || self.snapshot.entry_count() > MAX_PERSONAL_RANKING_ENTRIES
            || !self
                .covered_package_names
                .contains(&self.last_ordering_key.3)
            || self.created_unix_ms != self.last_ordering_key.0
            || u64::try_from(self.selection_count).ok() != Some(self.snapshot.generation())
        {
            return Err(PersonalRankingError::InvalidCheckpoint);
        }
        Ok(())
    }
}

fn checkpoint_coverage_body(package_names: &BTreeSet<String>) -> String {
    let mut body = String::new();
    for name in package_names {
        body.push_str(name);
        body.push('\n');
    }
    body
}

pub fn protect_personal_ranking_batch(
    batch: &PersonalRankingBatch,
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, PersonalRankingError> {
    let mut plaintext = batch.render()?;
    let protected = protector.protect(&plaintext);
    plaintext.fill(0);
    let protected = protected.map_err(|_| PersonalRankingError::Protection)?;
    if protected.is_empty() || protected.len() > MAX_PERSONAL_RANKING_PROTECTED_BYTES {
        return Err(PersonalRankingError::InvalidProtectedPackage);
    }
    let protected_len = u32::try_from(protected.len())
        .map_err(|_| PersonalRankingError::InvalidProtectedPackage)?;
    let mut output = Vec::with_capacity(
        PROTECTED_PERSONAL_RANKING_MAGIC.len() + size_of::<u32>() + protected.len(),
    );
    output.extend_from_slice(PROTECTED_PERSONAL_RANKING_MAGIC);
    output.extend_from_slice(&protected_len.to_le_bytes());
    output.extend_from_slice(&protected);
    if output.len() > MAX_PERSONAL_RANKING_PROTECTED_BYTES {
        return Err(PersonalRankingError::InvalidProtectedPackage);
    }
    Ok(output)
}

pub fn unprotect_personal_ranking_batch(
    package: &[u8],
    protector: &dyn DataProtector,
) -> Result<PersonalRankingBatch, PersonalRankingError> {
    if package.len() <= PROTECTED_PERSONAL_RANKING_MAGIC.len() + size_of::<u32>()
        || package.len() > MAX_PERSONAL_RANKING_PROTECTED_BYTES
        || !package.starts_with(PROTECTED_PERSONAL_RANKING_MAGIC)
    {
        return Err(PersonalRankingError::InvalidProtectedPackage);
    }
    let length_start = PROTECTED_PERSONAL_RANKING_MAGIC.len();
    let protected_len = u32::from_le_bytes(
        package[length_start..length_start + size_of::<u32>()]
            .try_into()
            .expect("four-byte protected ranking length"),
    ) as usize;
    let protected = &package[length_start + size_of::<u32>()..];
    if protected.is_empty() || protected.len() != protected_len {
        return Err(PersonalRankingError::InvalidProtectedPackage);
    }
    let mut plaintext = protector
        .unprotect(protected)
        .map_err(|_| PersonalRankingError::Protection)?;
    let parsed = PersonalRankingBatch::parse(&plaintext);
    plaintext.fill(0);
    parsed
}

pub fn protect_personal_ranking_suppression_action(
    action: &PersonalRankingSuppressionAction,
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, PersonalRankingError> {
    let mut plaintext = action.render()?;
    let protected = protector.protect(&plaintext);
    plaintext.fill(0);
    let protected = protected.map_err(|_| PersonalRankingError::Protection)?;
    if protected.is_empty()
        || protected.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES
    {
        return Err(PersonalRankingError::InvalidProtectedSuppressionAction);
    }
    let protected_len = u32::try_from(protected.len())
        .map_err(|_| PersonalRankingError::InvalidProtectedSuppressionAction)?;
    let mut output = Vec::with_capacity(
        PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC.len()
            + size_of::<u32>()
            + protected.len(),
    );
    output.extend_from_slice(PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC);
    output.extend_from_slice(&protected_len.to_le_bytes());
    output.extend_from_slice(&protected);
    if output.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES {
        return Err(PersonalRankingError::InvalidProtectedSuppressionAction);
    }
    Ok(output)
}

pub fn unprotect_personal_ranking_suppression_action(
    package: &[u8],
    protector: &dyn DataProtector,
) -> Result<PersonalRankingSuppressionAction, PersonalRankingError> {
    if package.len() <= PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC.len() + size_of::<u32>()
        || package.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES
        || !package.starts_with(PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC)
    {
        return Err(PersonalRankingError::InvalidProtectedSuppressionAction);
    }
    let length_start = PROTECTED_PERSONAL_RANKING_SUPPRESSION_ACTION_MAGIC.len();
    let protected_len = u32::from_le_bytes(
        package[length_start..length_start + size_of::<u32>()]
            .try_into()
            .expect("four-byte protected suppression action length"),
    ) as usize;
    let protected = &package[length_start + size_of::<u32>()..];
    if protected.is_empty() || protected.len() != protected_len {
        return Err(PersonalRankingError::InvalidProtectedSuppressionAction);
    }
    let mut plaintext = protector
        .unprotect(protected)
        .map_err(|_| PersonalRankingError::Protection)?;
    let parsed = PersonalRankingSuppressionAction::parse(&plaintext);
    plaintext.fill(0);
    parsed
}

fn protect_personal_ranking_checkpoint(
    checkpoint: &PersonalRankingCheckpoint,
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, PersonalRankingError> {
    let mut plaintext = checkpoint.render()?;
    let protected = protector.protect(&plaintext);
    plaintext.fill(0);
    let protected = protected.map_err(|_| PersonalRankingError::Protection)?;
    if protected.is_empty() || protected.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES {
        return Err(PersonalRankingError::InvalidProtectedCheckpoint);
    }
    let protected_len = u32::try_from(protected.len())
        .map_err(|_| PersonalRankingError::InvalidProtectedCheckpoint)?;
    let mut output = Vec::with_capacity(
        PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC.len() + size_of::<u32>() + protected.len(),
    );
    output.extend_from_slice(PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC);
    output.extend_from_slice(&protected_len.to_le_bytes());
    output.extend_from_slice(&protected);
    if output.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES {
        return Err(PersonalRankingError::InvalidProtectedCheckpoint);
    }
    Ok(output)
}

fn unprotect_personal_ranking_checkpoint(
    package: &[u8],
    protector: &dyn DataProtector,
) -> Result<PersonalRankingCheckpoint, PersonalRankingError> {
    if package.len() <= PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC.len() + size_of::<u32>()
        || package.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES
        || !package.starts_with(PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC)
    {
        return Err(PersonalRankingError::InvalidProtectedCheckpoint);
    }
    let length_start = PROTECTED_PERSONAL_RANKING_CHECKPOINT_MAGIC.len();
    let protected_len = u32::from_le_bytes(
        package[length_start..length_start + size_of::<u32>()]
            .try_into()
            .expect("four-byte protected ranking checkpoint length"),
    ) as usize;
    let protected = &package[length_start + size_of::<u32>()..];
    if protected.is_empty() || protected.len() != protected_len {
        return Err(PersonalRankingError::InvalidProtectedCheckpoint);
    }
    let mut plaintext = protector
        .unprotect(protected)
        .map_err(|_| PersonalRankingError::Protection)?;
    let parsed = PersonalRankingCheckpoint::parse(&plaintext);
    plaintext.fill(0);
    parsed
}

fn personal_ranking_checkpoint_file(checkpoint: &PersonalRankingCheckpoint) -> String {
    let coverage = checkpoint_coverage_body(&checkpoint.covered_package_names);
    format!(
        "checkpoint-{:08x}-{}.{}",
        checkpoint.covered_package_names.len(),
        candidate_sha256_hex(coverage.as_bytes()),
        PERSONAL_RANKING_CHECKPOINT_EXTENSION
    )
}

fn parse_checkpoint_file_name(name: &str) -> Option<usize> {
    let rest = name.strip_prefix("checkpoint-")?.strip_suffix(".zpc")?;
    let (count, digest) = rest.split_once('-')?;
    if count.len() != 8
        || !count
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    usize::from_str_radix(count, 16).ok().filter(|count| {
        (*count >= MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES)
            && (*count <= MAX_PERSONAL_RANKING_BATCH_FILES)
    })
}

pub fn personal_ranking_package_file(package: &[u8]) -> String {
    format!(
        "rank-{}.{}",
        candidate_sha256_hex(package),
        PERSONAL_RANKING_BATCH_EXTENSION
    )
}

pub fn personal_ranking_suppression_package_file(package: &[u8]) -> String {
    format!(
        "suppress-{}.{}",
        candidate_sha256_hex(package),
        PERSONAL_RANKING_SUPPRESSION_ACTION_EXTENSION
    )
}

pub fn save_personal_ranking_checkpoint(
    root: &Path,
    loaded: &LoadedPersonalRanking,
    protector: &dyn DataProtector,
) -> Result<Option<String>, PersonalRankingError> {
    if loaded.batch_count() < MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES {
        return Ok(None);
    }
    prepare_root(root)?;
    let checkpoint = PersonalRankingCheckpoint::from_loaded(loaded)?;
    let file_name = personal_ranking_checkpoint_file(&checkpoint);
    let destination = root.join(&file_name);
    if destination.exists() {
        let package = read_regular_checkpoint_bytes(&destination)?;
        let existing = unprotect_personal_ranking_checkpoint(&package, protector)?;
        if existing == checkpoint {
            return Ok(Some(file_name));
        }
        return Err(PersonalRankingError::CheckpointIdentifierMismatch);
    }
    if personal_ranking_checkpoint_file_count(root)? >= MAX_PERSONAL_RANKING_CHECKPOINT_FILES {
        return Err(PersonalRankingError::TooManyCheckpoints);
    }
    let package = protect_personal_ranking_checkpoint(&checkpoint, protector)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PersonalRankingError::Clock)?
        .as_nanos();
    let mut temporary = None;
    for attempt in 0..16_u32 {
        let path = root.join(format!(
            ".checkpoint-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(PersonalRankingError::Write),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or(PersonalRankingError::Write)?;
    let result = (|| {
        file.write_all(&package)
            .map_err(|_| PersonalRankingError::Write)?;
        file.sync_all().map_err(|_| PersonalRankingError::Write)?;
        drop(file);
        fs::rename(&temporary_path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                PersonalRankingError::PackageAlreadyExists
            } else {
                PersonalRankingError::Write
            }
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        if result == Err(PersonalRankingError::PackageAlreadyExists) {
            let existing_package = read_regular_checkpoint_bytes(&destination)?;
            let existing = unprotect_personal_ranking_checkpoint(&existing_package, protector)?;
            if existing == checkpoint {
                return Ok(Some(file_name));
            }
        }
        result?;
    }
    Ok(Some(file_name))
}

fn load_best_personal_ranking_checkpoint(
    root: &Path,
    package_names: &BTreeSet<String>,
    protector: &dyn DataProtector,
) -> Result<Option<PersonalRankingCheckpoint>, PersonalRankingError> {
    let mut checkpoints = personal_ranking_checkpoint_files(root)?;
    checkpoints.sort_by(|(left_count, left_name), (right_count, right_name)| {
        right_count
            .cmp(left_count)
            .then_with(|| right_name.cmp(left_name))
    });
    for (_, name) in checkpoints {
        let package = read_regular_checkpoint_bytes(&root.join(&name))?;
        let checkpoint = unprotect_personal_ranking_checkpoint(&package, protector)?;
        if personal_ranking_checkpoint_file(&checkpoint) != name {
            return Err(PersonalRankingError::CheckpointIdentifierMismatch);
        }
        if checkpoint.covered_package_names.is_subset(package_names) {
            return Ok(Some(checkpoint));
        }
    }
    Ok(None)
}

pub fn load_personal_ranking(
    root: &Path,
    protector: &dyn DataProtector,
) -> Result<LoadedPersonalRanking, PersonalRankingError> {
    let package_names = personal_ranking_package_names(root)?;
    if package_names.is_empty() {
        return Ok(LoadedPersonalRanking::default());
    }
    if let Some(checkpoint) =
        load_best_personal_ranking_checkpoint(root, &package_names, protector)?
    {
        let uncovered_names = package_names
            .difference(&checkpoint.covered_package_names)
            .cloned()
            .collect::<BTreeSet<_>>();
        let batches = load_named_personal_ranking_batches(root, &uncovered_names, protector)?;
        let checkpoint_key = &checkpoint.last_ordering_key;
        if batches
            .first()
            .map(|(batch, name)| ordering_key_with_name(batch, name) > *checkpoint_key)
            .unwrap_or(true)
        {
            let checkpoint_batch_count = checkpoint.covered_package_names.len();
            let mut loaded = LoadedPersonalRanking {
                snapshot: checkpoint.snapshot,
                batch_count: checkpoint_batch_count,
                selection_count: checkpoint.selection_count,
                package_names: checkpoint.covered_package_names,
                last_ordering_key: Some(checkpoint.last_ordering_key),
                checkpoint_batch_count,
            };
            apply_loaded_batches(&mut loaded, batches)?;
            loaded.package_names = package_names;
            loaded.batch_count = loaded.package_names.len();
            return Ok(loaded);
        }
    }
    load_full_personal_ranking(root, package_names, protector)
}

fn load_full_personal_ranking(
    root: &Path,
    package_names: BTreeSet<String>,
    protector: &dyn DataProtector,
) -> Result<LoadedPersonalRanking, PersonalRankingError> {
    let batches = load_named_personal_ranking_batches(root, &package_names, protector)?;
    let mut loaded = LoadedPersonalRanking::default();
    apply_loaded_batches(&mut loaded, batches)?;
    loaded.batch_count = package_names.len();
    loaded.package_names = package_names;
    Ok(loaded)
}

fn load_named_personal_ranking_batches(
    root: &Path,
    package_names: &BTreeSet<String>,
    protector: &dyn DataProtector,
) -> Result<Vec<(PersonalRankingBatch, String)>, PersonalRankingError> {
    let mut batches = Vec::with_capacity(package_names.len());
    for name in package_names {
        let package = read_regular_bytes(&root.join(name))?;
        if personal_ranking_package_file(&package) != *name {
            return Err(PersonalRankingError::PackageIdentifierMismatch);
        }
        let batch = unprotect_personal_ranking_batch(&package, protector)?;
        batches.push((batch, name.clone()));
    }
    batches.sort_by(|(left_batch, left_name), (right_batch, right_name)| {
        left_batch
            .ordering_key()
            .cmp(&right_batch.ordering_key())
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(batches)
}

fn apply_loaded_batches(
    loaded: &mut LoadedPersonalRanking,
    batches: Vec<(PersonalRankingBatch, String)>,
) -> Result<(), PersonalRankingError> {
    for (batch, name) in batches {
        loaded.snapshot.apply_batch(&batch)?;
        loaded.selection_count = loaded
            .selection_count
            .saturating_add(batch.selection_count());
        loaded.last_ordering_key = Some(ordering_key_with_name(&batch, &name));
    }
    Ok(())
}

pub fn refresh_personal_ranking(
    root: &Path,
    protector: &dyn DataProtector,
    previous: &LoadedPersonalRanking,
) -> Result<LoadedPersonalRanking, PersonalRankingError> {
    let package_names = personal_ranking_package_names(root)?;
    if package_names == previous.package_names {
        return Ok(previous.clone());
    }
    if !previous.package_names.is_subset(&package_names) {
        return load_personal_ranking(root, protector);
    }

    let mut added = Vec::new();
    for name in package_names.difference(&previous.package_names) {
        let package = read_regular_bytes(&root.join(name))?;
        if personal_ranking_package_file(&package) != *name {
            return Err(PersonalRankingError::PackageIdentifierMismatch);
        }
        let batch = unprotect_personal_ranking_batch(&package, protector)?;
        added.push((batch, name.clone()));
    }
    added.sort_by(|(left_batch, left_name), (right_batch, right_name)| {
        left_batch
            .ordering_key()
            .cmp(&right_batch.ordering_key())
            .then_with(|| left_name.cmp(right_name))
    });
    if let (Some(previous_key), Some((first_batch, first_name))) =
        (previous.last_ordering_key.as_ref(), added.first())
        && ordering_key_with_name(first_batch, first_name) <= *previous_key
    {
        return load_personal_ranking(root, protector);
    }

    let mut refreshed = previous.clone();
    for (batch, _) in &added {
        refreshed.snapshot.apply_batch(batch)?;
        refreshed.selection_count = refreshed
            .selection_count
            .saturating_add(batch.selection_count());
    }
    refreshed.batch_count = package_names.len();
    refreshed.package_names = package_names;
    if let Some((batch, name)) = added.last() {
        refreshed.last_ordering_key = Some(ordering_key_with_name(batch, name));
    }
    Ok(refreshed)
}

fn ordering_key_with_name(batch: &PersonalRankingBatch, name: &str) -> (u64, u32, u64, String) {
    let (created_unix_ms, process_id, sequence) = batch.ordering_key();
    (created_unix_ms, process_id, sequence, name.to_owned())
}

pub fn save_personal_ranking_batch(
    root: &Path,
    batch: &PersonalRankingBatch,
    protector: &dyn DataProtector,
) -> Result<String, PersonalRankingError> {
    prepare_root(root)?;
    let package = protect_personal_ranking_batch(batch, protector)?;
    let file_name = personal_ranking_package_file(&package);
    let destination = root.join(&file_name);
    if destination.exists() {
        let existing = read_regular_bytes(&destination)?;
        if existing == package {
            return Ok(file_name);
        }
        return Err(PersonalRankingError::PackageIdentifierMismatch);
    }
    if personal_ranking_batch_file_count(root)? >= MAX_PERSONAL_RANKING_BATCH_FILES {
        return Err(PersonalRankingError::TooManyBatches);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PersonalRankingError::Clock)?
        .as_nanos();
    let mut temporary = None;
    for attempt in 0..16_u32 {
        let path = root.join(format!(
            ".rank-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(PersonalRankingError::Write),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or(PersonalRankingError::Write)?;
    let result = (|| {
        file.write_all(&package)
            .map_err(|_| PersonalRankingError::Write)?;
        file.sync_all().map_err(|_| PersonalRankingError::Write)?;
        drop(file);
        fs::rename(&temporary_path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                PersonalRankingError::PackageAlreadyExists
            } else {
                PersonalRankingError::Write
            }
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        if result == Err(PersonalRankingError::PackageAlreadyExists) {
            let existing = read_regular_bytes(&destination)?;
            if existing == package {
                return Ok(file_name);
            }
        }
        result?;
    }
    Ok(file_name)
}

pub fn save_personal_ranking_suppression_action(
    root: &Path,
    action: &PersonalRankingSuppressionAction,
    protector: &dyn DataProtector,
) -> Result<String, PersonalRankingError> {
    prepare_root(root)?;
    let package = protect_personal_ranking_suppression_action(action, protector)?;
    let file_name = personal_ranking_suppression_package_file(&package);
    let destination = root.join(&file_name);
    if destination.exists() {
        let existing = read_regular_suppression_bytes(&destination)?;
        if existing == package {
            return Ok(file_name);
        }
        return Err(PersonalRankingError::SuppressionPackageIdentifierMismatch);
    }
    if personal_ranking_suppression_package_names(root)?.len()
        >= MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_FILES
    {
        return Err(PersonalRankingError::TooManySuppressionActions);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PersonalRankingError::Clock)?
        .as_nanos();
    let mut temporary = None;
    for attempt in 0..16_u32 {
        let path = root.join(format!(
            ".suppress-{}-{stamp}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(PersonalRankingError::Write),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or(PersonalRankingError::Write)?;
    let result = (|| {
        file.write_all(&package)
            .map_err(|_| PersonalRankingError::Write)?;
        file.sync_all().map_err(|_| PersonalRankingError::Write)?;
        drop(file);
        fs::rename(&temporary_path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                PersonalRankingError::PackageAlreadyExists
            } else {
                PersonalRankingError::Write
            }
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        if result == Err(PersonalRankingError::PackageAlreadyExists) {
            let existing = read_regular_suppression_bytes(&destination)?;
            if existing == package {
                return Ok(file_name);
            }
        }
        result?;
    }
    Ok(file_name)
}

pub fn load_personal_ranking_suppressions(
    root: &Path,
    protector: &dyn DataProtector,
) -> Result<LoadedPersonalRankingSuppressions, PersonalRankingError> {
    let package_names = personal_ranking_suppression_package_names(root)?;
    load_full_personal_ranking_suppressions(root, package_names, protector)
}

fn load_full_personal_ranking_suppressions(
    root: &Path,
    package_names: BTreeSet<String>,
    protector: &dyn DataProtector,
) -> Result<LoadedPersonalRankingSuppressions, PersonalRankingError> {
    let actions = load_named_personal_ranking_suppression_actions(root, &package_names, protector)?;
    let mut loaded = LoadedPersonalRankingSuppressions::default();
    apply_loaded_suppression_actions(&mut loaded, actions)?;
    loaded.action_count = package_names.len();
    loaded.package_names = package_names;
    Ok(loaded)
}

fn load_named_personal_ranking_suppression_actions(
    root: &Path,
    package_names: &BTreeSet<String>,
    protector: &dyn DataProtector,
) -> Result<Vec<(PersonalRankingSuppressionAction, String)>, PersonalRankingError> {
    let mut actions = Vec::with_capacity(package_names.len());
    for name in package_names {
        let package = read_regular_suppression_bytes(&root.join(name))?;
        if personal_ranking_suppression_package_file(&package) != *name {
            return Err(PersonalRankingError::SuppressionPackageIdentifierMismatch);
        }
        let action = unprotect_personal_ranking_suppression_action(&package, protector)?;
        actions.push((action, name.clone()));
    }
    actions.sort_by(|(left_action, left_name), (right_action, right_name)| {
        left_action
            .ordering_key()
            .cmp(&right_action.ordering_key())
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(actions)
}

fn apply_loaded_suppression_actions(
    loaded: &mut LoadedPersonalRankingSuppressions,
    actions: Vec<(PersonalRankingSuppressionAction, String)>,
) -> Result<(), PersonalRankingError> {
    for (action, name) in actions {
        action.apply_to(&mut loaded.snapshot)?;
        loaded.last_ordering_key = Some(suppression_ordering_key_with_name(&action, &name));
    }
    Ok(())
}

pub fn refresh_personal_ranking_suppressions(
    root: &Path,
    protector: &dyn DataProtector,
    previous: &LoadedPersonalRankingSuppressions,
) -> Result<LoadedPersonalRankingSuppressions, PersonalRankingError> {
    let package_names = personal_ranking_suppression_package_names(root)?;
    if package_names == previous.package_names {
        return Ok(previous.clone());
    }
    if !previous.package_names.is_subset(&package_names) {
        return load_full_personal_ranking_suppressions(root, package_names, protector);
    }

    let added_names = package_names
        .difference(&previous.package_names)
        .cloned()
        .collect::<BTreeSet<_>>();
    let added = load_named_personal_ranking_suppression_actions(root, &added_names, protector)?;
    if let (Some(previous_key), Some((first_action, first_name))) =
        (previous.last_ordering_key.as_ref(), added.first())
        && suppression_ordering_key_with_name(first_action, first_name) <= *previous_key
    {
        return load_full_personal_ranking_suppressions(root, package_names, protector);
    }

    let mut refreshed = previous.clone();
    for (action, _) in &added {
        action.apply_to(&mut refreshed.snapshot)?;
    }
    refreshed.action_count = package_names.len();
    refreshed.package_names = package_names;
    if let Some((action, name)) = added.last() {
        refreshed.last_ordering_key = Some(suppression_ordering_key_with_name(action, name));
    }
    Ok(refreshed)
}

fn suppression_ordering_key_with_name(
    action: &PersonalRankingSuppressionAction,
    name: &str,
) -> (u64, u32, u64, String) {
    let (created_unix_ms, process_id, sequence) = action.ordering_key();
    (created_unix_ms, process_id, sequence, name.to_owned())
}

fn personal_ranking_batch_file_count(root: &Path) -> Result<usize, PersonalRankingError> {
    Ok(personal_ranking_package_names(root)?.len())
}

fn personal_ranking_package_names(root: &Path) -> Result<BTreeSet<String>, PersonalRankingError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeSet::new());
        }
        Err(_) => return Err(PersonalRankingError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PersonalRankingError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root).map_err(|_| PersonalRankingError::RootUnavailable)? {
        let entry = entry.map_err(|_| PersonalRankingError::RootUnavailable)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if valid_package_file_name(&name) {
            if names.len() == MAX_PERSONAL_RANKING_BATCH_FILES {
                return Err(PersonalRankingError::TooManyBatches);
            }
            names.insert(name);
        }
    }
    Ok(names)
}

fn personal_ranking_suppression_package_names(
    root: &Path,
) -> Result<BTreeSet<String>, PersonalRankingError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeSet::new());
        }
        Err(_) => return Err(PersonalRankingError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PersonalRankingError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root).map_err(|_| PersonalRankingError::RootUnavailable)? {
        let entry = entry.map_err(|_| PersonalRankingError::RootUnavailable)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if valid_suppression_package_file_name(&name) {
            if names.len() == MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_FILES {
                return Err(PersonalRankingError::TooManySuppressionActions);
            }
            names.insert(name);
        }
    }
    Ok(names)
}

fn personal_ranking_checkpoint_file_count(root: &Path) -> Result<usize, PersonalRankingError> {
    Ok(personal_ranking_checkpoint_files(root)?.len())
}

fn personal_ranking_checkpoint_files(
    root: &Path,
) -> Result<Vec<(usize, String)>, PersonalRankingError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(PersonalRankingError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(PersonalRankingError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let mut checkpoints = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| PersonalRankingError::RootUnavailable)? {
        let entry = entry.map_err(|_| PersonalRankingError::RootUnavailable)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(count) = parse_checkpoint_file_name(&name) else {
            continue;
        };
        if checkpoints.len() == MAX_PERSONAL_RANKING_CHECKPOINT_FILES {
            return Err(PersonalRankingError::TooManyCheckpoints);
        }
        checkpoints.push((count, name));
    }
    Ok(checkpoints)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PersonalRankingError {
    InvalidCode,
    InvalidText,
    InvalidEventCount,
    InvalidPlaintextSize,
    InvalidPlaintextStructure,
    InvalidUtf8,
    InvalidField,
    InvalidNumber,
    InvalidEntry,
    PayloadLengthMismatch,
    EventCountMismatch,
    InvalidProtectedPackage,
    InvalidCheckpointSize,
    InvalidCheckpoint,
    InvalidProtectedCheckpoint,
    InvalidSuppressionActionSize,
    InvalidSuppressionAction,
    InvalidProtectedSuppressionAction,
    Protection,
    GenerationOverflow,
    Clock,
    RootUnavailable,
    InvalidRoot,
    TooManySuppressions,
    TooManySuppressionActions,
    TooManyBatches,
    TooManyCheckpoints,
    PackageUnavailable,
    PackageIdentifierMismatch,
    CheckpointIdentifierMismatch,
    SuppressionPackageIdentifierMismatch,
    PackageAlreadyExists,
    Write,
}

impl fmt::Display for PersonalRankingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCode => "个人排序编码无效",
            Self::InvalidText => "个人排序文字无效",
            Self::InvalidEventCount => "个人排序批次条目数无效",
            Self::InvalidPlaintextSize => "个人排序明文大小无效",
            Self::InvalidPlaintextStructure => "个人排序明文结构无效",
            Self::InvalidUtf8 => "个人排序明文不是有效 UTF-8",
            Self::InvalidField => "个人排序字段无效",
            Self::InvalidNumber => "个人排序数字无效",
            Self::InvalidEntry => "个人排序条目无效",
            Self::PayloadLengthMismatch => "个人排序载荷长度不符",
            Self::EventCountMismatch => "个人排序条目数不符",
            Self::InvalidProtectedPackage => "个人排序加密包无效",
            Self::InvalidCheckpointSize => "个人排序检查点大小无效",
            Self::InvalidCheckpoint => "个人排序检查点无效",
            Self::InvalidProtectedCheckpoint => "个人排序加密检查点无效",
            Self::InvalidSuppressionActionSize => "个人排序忘记动作大小无效",
            Self::InvalidSuppressionAction => "个人排序忘记动作无效",
            Self::InvalidProtectedSuppressionAction => "个人排序忘记动作加密包无效",
            Self::Protection => "当前用户无法加密或解密个人排序",
            Self::GenerationOverflow => "个人排序代数溢出",
            Self::Clock => "无法取得个人排序批次时间",
            Self::RootUnavailable => "个人排序目录不可用",
            Self::InvalidRoot => "个人排序目录无效",
            Self::TooManySuppressions => "个人排序抑制条目数超过上限",
            Self::TooManySuppressionActions => "个人排序忘记动作数超过上限",
            Self::TooManyBatches => "个人排序批次数超过上限",
            Self::TooManyCheckpoints => "个人排序检查点数超过上限",
            Self::PackageUnavailable => "个人排序加密包不可用",
            Self::PackageIdentifierMismatch => "个人排序包与内容标识不符",
            Self::CheckpointIdentifierMismatch => "个人排序检查点与内容标识不符",
            Self::SuppressionPackageIdentifierMismatch => "个人排序忘记动作包与内容标识不符",
            Self::PackageAlreadyExists => "个人排序包已存在",
            Self::Write => "无法安全写入个人排序包",
        })
    }
}

impl Error for PersonalRankingError {}

fn validate_selection(code: &str, text: &str) -> Result<(), PersonalRankingError> {
    if code.is_empty()
        || code.len() > MAX_PERSONAL_RANKING_CODE_BYTES
        || !code.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(PersonalRankingError::InvalidCode);
    }
    if text.is_empty()
        || text.len() > MAX_PERSONAL_RANKING_TEXT_BYTES
        || text.chars().count() > MAX_PERSONAL_RANKING_TEXT_CHARACTERS
        || text.chars().any(char::is_control)
        || text
            .chars()
            .any(|character| matches!(character, '\t' | '\r' | '\n'))
    {
        return Err(PersonalRankingError::InvalidText);
    }
    Ok(())
}

fn field<'a>(line: &'a str, expected: &str) -> Result<&'a str, PersonalRankingError> {
    let (key, value) = line
        .split_once('=')
        .ok_or(PersonalRankingError::InvalidField)?;
    if key != expected || value.is_empty() || value.contains('=') {
        return Err(PersonalRankingError::InvalidField);
    }
    Ok(value)
}

fn parse_canonical_u64(value: &str) -> Result<u64, PersonalRankingError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(PersonalRankingError::InvalidNumber);
    }
    value
        .parse::<u64>()
        .map_err(|_| PersonalRankingError::InvalidNumber)
}

fn parse_canonical_u32(value: &str) -> Result<u32, PersonalRankingError> {
    parse_canonical_u64(value)?
        .try_into()
        .map_err(|_| PersonalRankingError::InvalidNumber)
}

fn parse_canonical_usize(value: &str) -> Result<usize, PersonalRankingError> {
    parse_canonical_u64(value)?
        .try_into()
        .map_err(|_| PersonalRankingError::InvalidNumber)
}

fn valid_package_file_name(name: &str) -> bool {
    let Some(digest) = name
        .strip_prefix("rank-")
        .and_then(|rest| rest.strip_suffix(".zpr"))
    else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_suppression_package_file_name(name: &str) -> bool {
    let Some(digest) = name
        .strip_prefix("suppress-")
        .and_then(|rest| rest.strip_suffix(".zps"))
    else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn prepare_root(root: &Path) -> Result<(), PersonalRankingError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(PersonalRankingError::InvalidRoot)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = root.parent().ok_or(PersonalRankingError::InvalidRoot)?;
            let parent_metadata =
                fs::symlink_metadata(parent).map_err(|_| PersonalRankingError::RootUnavailable)?;
            if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
                return Err(PersonalRankingError::InvalidRoot);
            }
            match fs::create_dir(root) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    prepare_root(root)
                }
                Err(_) => Err(PersonalRankingError::Write),
            }
        }
        Err(_) => Err(PersonalRankingError::RootUnavailable),
    }
}

fn read_regular_bytes(path: &Path) -> Result<Vec<u8>, PersonalRankingError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let maximum = u64::try_from(MAX_PERSONAL_RANKING_PROTECTED_BYTES).unwrap_or(u64::MAX);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    let mut file = File::open(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| PersonalRankingError::PackageUnavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_PERSONAL_RANKING_PROTECTED_BYTES {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    Ok(bytes)
}

fn read_regular_suppression_bytes(path: &Path) -> Result<Vec<u8>, PersonalRankingError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let maximum =
        u64::try_from(MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES).unwrap_or(u64::MAX);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    let mut file = File::open(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| PersonalRankingError::PackageUnavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_PROTECTED_BYTES {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    Ok(bytes)
}

fn read_regular_checkpoint_bytes(path: &Path) -> Result<Vec<u8>, PersonalRankingError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let maximum =
        u64::try_from(MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES).unwrap_or(u64::MAX);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    let mut file = File::open(path).map_err(|_| PersonalRankingError::PackageUnavailable)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| PersonalRankingError::PackageUnavailable)?;
    if bytes.is_empty() || bytes.len() > MAX_PERSONAL_RANKING_CHECKPOINT_PROTECTED_BYTES {
        return Err(PersonalRankingError::PackageUnavailable);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContinuousCaptureError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[derive(Clone, Copy)]
    struct TestProtector;

    impl DataProtector for TestProtector {
        fn protection_name(&self) -> &'static str {
            "test"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(plaintext.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            self.protect(protected)
        }
    }

    struct CountingProtector {
        unprotect_calls: AtomicUsize,
    }

    impl CountingProtector {
        fn new() -> Self {
            Self {
                unprotect_calls: AtomicUsize::new(0),
            }
        }

        fn unprotect_calls(&self) -> usize {
            self.unprotect_calls.load(Ordering::Relaxed)
        }
    }

    impl DataProtector for CountingProtector {
        fn protection_name(&self) -> &'static str {
            "counting-test"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(plaintext.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            self.unprotect_calls.fetch_add(1, Ordering::Relaxed);
            self.protect(protected)
        }
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let parent = std::env::temp_dir().join(format!(
                "ziranma-personal-ranking-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&parent).unwrap();
            Self(parent)
        }

        fn root(&self) -> PathBuf {
            self.0.join("ranking")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn batch(
        time: u64,
        process: u32,
        sequence: u64,
        values: &[(&str, &str)],
    ) -> PersonalRankingBatch {
        PersonalRankingBatch::new(
            time,
            process,
            sequence,
            values
                .iter()
                .map(|(code, text)| PersonalRankingSelection::new(code, text).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn suppression_action(
        time: u64,
        process: u32,
        sequence: u64,
        kind: PersonalRankingSuppressionActionKind,
        code: &str,
        text: &str,
    ) -> PersonalRankingSuppressionAction {
        PersonalRankingSuppressionAction::new(time, process, sequence, kind, code, text).unwrap()
    }

    #[test]
    fn protected_batch_round_trips_without_text_in_debug() {
        let batch = batch(10, 20, 30, &[("qnqn", "亲亲"), ("rbrb", "揉揉")]);
        let package = protect_personal_ranking_batch(&batch, &TestProtector).unwrap();
        assert_eq!(
            unprotect_personal_ranking_batch(&package, &TestProtector).unwrap(),
            batch
        );
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.apply_batch(&batch).unwrap();
        let debug = format!("{snapshot:?}");
        assert!(!debug.contains("亲亲"));
        assert!(!debug.contains("qnqn"));
    }

    #[test]
    fn latest_explicit_choice_promotes_without_crossing_a_protected_prefix() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("ab", "乙").unwrap();
        snapshot.record("ab", "丙").unwrap();
        let mut candidates = vec!["固定".to_owned(), "甲".to_owned(), "乙".to_owned()];
        assert!(snapshot.promote_texts_after("ab", &mut candidates, 1));
        assert_eq!(candidates, ["固定", "丙", "甲"]);
    }

    #[test]
    fn exact_code_lookup_reads_only_its_contiguous_ordered_range() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("aa", "前").unwrap();
        snapshot.record("ab", "甲").unwrap();
        snapshot.record("ab", "乙").unwrap();
        snapshot.record("ac", "后").unwrap();

        let texts = snapshot
            .entries_for_code("ab")
            .map(|((_, text), _)| text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, ["乙", "甲"]);
        assert!(snapshot.has_evidence("ab", "甲"));
        assert!(!snapshot.has_evidence("ab", "前"));
        assert_eq!(snapshot.preferred_text("ab"), Some("乙"));
    }

    #[test]
    fn anchored_lookup_keeps_only_structural_sources_from_the_shared_prefix_range() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("abcdef", "目标").unwrap();
        snapshot.record("abdzff", "同前缀").unwrap();
        snapshot.record("abcdefgh", "长度不同").unwrap();
        snapshot.record("acdefg", "相邻编码").unwrap();

        let matches = snapshot
            .entries_for_anchored_code("abce")
            .map(|((code, text), _)| (code.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(matches, [("abcdef", "目标")]);
        assert_eq!(snapshot.entries_for_anchored_code("ABc").count(), 0);
        assert_eq!(snapshot.entries_for_anchored_code("ab").count(), 0);
    }

    #[test]
    fn anchored_suffix_relation_accepts_only_a_complete_prefix_and_abbreviated_tail() {
        assert!(is_anchored_suffix_abbreviation("jdjd", "jdj"));
        assert!(is_anchored_suffix_abbreviation("abcdef", "abce"));
        assert!(is_anchored_suffix_abbreviation("abcdef", "abcde"));
        assert!(is_anchored_suffix_abbreviation("abcdefgh", "abceg"));
        assert!(is_anchored_suffix_abbreviation("abcdefgh", "abcdeg"));
        assert!(is_anchored_suffix_abbreviation("abcdefgh", "abcdefg"));
        assert!(!is_anchored_suffix_abbreviation("jdjd", "jd"));
        assert!(!is_anchored_suffix_abbreviation("jdjd", "jjd"));
        assert!(!is_anchored_suffix_abbreviation("jdjd", "jdjd"));
        assert!(!is_anchored_suffix_abbreviation("abcdefgh", "aceg"));
        assert!(!is_anchored_suffix_abbreviation("abc", "ab"));
        assert!(!is_anchored_suffix_abbreviation("JD	JD", "jdj"));
    }

    #[test]
    fn verified_complete_code_evidence_promotes_only_an_existing_anchored_tail_candidate() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("jdjd", "讲讲").unwrap();
        snapshot.record("abef", "旁路").unwrap();
        let suppressions = PersonalRankingSuppressionSnapshot::default();
        let mut candidates = vec!["固定".to_owned(), "简单".to_owned(), "讲讲".to_owned()];

        assert!(
            snapshot.promote_anchored_suffix_texts_after_with_suppressions(
                "jdj",
                &mut candidates,
                1,
                &suppressions,
                |code, text| code == "jdjd" && text == "讲讲",
            )
        );
        assert_eq!(candidates, ["固定", "讲讲", "简单"]);

        let mut unrelated = vec!["简单".to_owned(), "讲讲".to_owned()];
        assert!(
            !snapshot.promote_anchored_suffix_texts_after_with_suppressions(
                "jd",
                &mut unrelated,
                0,
                &suppressions,
                |_, _| true,
            )
        );
        assert_eq!(unrelated, ["简单", "讲讲"]);

        let mut absent = vec!["简单".to_owned(), "降价".to_owned()];
        assert!(
            !snapshot.promote_anchored_suffix_texts_after_with_suppressions(
                "jdj",
                &mut absent,
                0,
                &suppressions,
                |_, _| true,
            )
        );
        assert_eq!(absent, ["简单", "降价"]);
    }

    #[test]
    fn abbreviated_code_suppression_blocks_only_the_inherited_view() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("jdjd", "讲讲").unwrap();
        let mut suppressions = PersonalRankingSuppressionSnapshot::default();
        suppressions.suppress("jdj", "讲讲").unwrap();
        let mut abbreviated = vec!["简单".to_owned(), "讲讲".to_owned()];

        assert!(
            !snapshot.promote_anchored_suffix_texts_after_with_suppressions(
                "jdj",
                &mut abbreviated,
                0,
                &suppressions,
                |_, _| true,
            )
        );
        assert_eq!(abbreviated, ["简单", "讲讲"]);
        assert_eq!(
            snapshot.preferred_text_with_suppressions("jdjd", &suppressions),
            Some("讲讲")
        );
        assert!(!snapshot.has_anchored_suffix_evidence_with_suppressions(
            "jdj",
            "讲讲",
            &suppressions,
            |_, _| true,
        ));

        suppressions.restore("jdj", "讲讲").unwrap();
        assert!(snapshot.has_anchored_suffix_evidence_with_suppressions(
            "jdj",
            "讲讲",
            &suppressions,
            |_, _| true,
        ));
    }

    #[test]
    fn repeated_character_composition_enters_only_the_guarded_short_code_discovery_lane() {
        let mut snapshot = PersonalRankingSnapshot::default();
        let suppressions = PersonalRankingSuppressionSnapshot::default();
        snapshot.record("qthp", "雀魂").unwrap();
        let mut once = vec!["其他".to_owned(), "雀跃".to_owned(), "去向".to_owned()];
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "qth",
                &mut once,
                0,
                &suppressions,
                |code, text| code == "qthp" && text == "雀魂",
            ),
            None
        );
        assert_eq!(once, ["其他", "雀跃", "去向"]);

        snapshot.record("qthp", "雀魂").unwrap();
        let mut repeated = vec!["其他".to_owned(), "雀跃".to_owned(), "去向".to_owned()];
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "qth",
                &mut repeated,
                0,
                &suppressions,
                |code, text| code == "qthp" && text == "雀魂",
            ),
            Some(1)
        );
        assert_eq!(repeated, ["其他", "雀魂", "雀跃"]);
        assert!(
            snapshot.has_repeated_anchored_suffix_evidence_with_suppressions(
                "qth",
                "雀魂",
                &suppressions,
                |code, text| code == "qthp" && text == "雀魂",
            )
        );

        let mut suppressed = suppressions.clone();
        suppressed.suppress("qth", "雀魂").unwrap();
        let mut hidden = vec!["其他".to_owned(), "雀跃".to_owned(), "去向".to_owned()];
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "qth",
                &mut hidden,
                0,
                &suppressed,
                |_, _| true,
            ),
            None
        );
        assert_eq!(hidden, ["其他", "雀跃", "去向"]);
    }

    #[test]
    fn three_and_four_character_compositions_use_only_their_anchored_suffix_codes() {
        let mut snapshot = PersonalRankingSnapshot::default();
        let suppressions = PersonalRankingSuppressionSnapshot::default();
        for _ in 0..PERSONAL_ABBREVIATION_DISCOVERY_MIN_SELECTIONS {
            snapshot.record("abcdef", "甲乙丙").unwrap();
            snapshot.record("abcdefgh", "甲乙丙丁").unwrap();
        }

        for short_code in ["abce", "abcde"] {
            let mut candidates = vec!["普通".to_owned(), "其他".to_owned(), "末尾".to_owned()];
            assert_eq!(
                snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                    short_code,
                    &mut candidates,
                    0,
                    &suppressions,
                    |code, text| code == "abcdef" && text == "甲乙丙",
                ),
                Some(1)
            );
            assert_eq!(candidates, ["普通", "甲乙丙", "其他"]);
        }

        for short_code in ["abceg", "abcdeg", "abcdefg"] {
            let mut candidates = vec!["普通".to_owned(), "其他".to_owned(), "末尾".to_owned()];
            assert_eq!(
                snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                    short_code,
                    &mut candidates,
                    0,
                    &suppressions,
                    |code, text| code == "abcdefgh" && text == "甲乙丙丁",
                ),
                Some(1)
            );
            assert_eq!(candidates, ["普通", "甲乙丙丁", "其他"]);
        }

        let mut leading_abbreviation =
            vec!["普通".to_owned(), "其他".to_owned(), "末尾".to_owned()];
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "aceg",
                &mut leading_abbreviation,
                0,
                &suppressions,
                |_, _| true,
            ),
            None
        );
        assert_eq!(leading_abbreviation, ["普通", "其他", "末尾"]);
    }

    #[test]
    fn forgetting_one_multi_character_short_code_keeps_its_other_views_available() {
        let mut snapshot = PersonalRankingSnapshot::default();
        for _ in 0..PERSONAL_ABBREVIATION_DISCOVERY_MIN_SELECTIONS {
            snapshot.record("abcdef", "甲乙丙").unwrap();
        }
        let mut suppressions = PersonalRankingSuppressionSnapshot::default();
        suppressions.suppress("abce", "甲乙丙").unwrap();

        let mut hidden = vec!["普通".to_owned(), "其他".to_owned(), "末尾".to_owned()];
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "abce",
                &mut hidden,
                0,
                &suppressions,
                |_, _| true,
            ),
            None
        );
        assert_eq!(hidden, ["普通", "其他", "末尾"]);

        let mut other_short_view = hidden.clone();
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "abcde",
                &mut other_short_view,
                0,
                &suppressions,
                |code, text| code == "abcdef" && text == "甲乙丙",
            ),
            Some(1)
        );
        assert_eq!(other_short_view, ["普通", "甲乙丙", "其他"]);
        assert_eq!(
            snapshot.preferred_text_with_suppressions("abcdef", &suppressions),
            Some("甲乙丙")
        );

        suppressions.restore("abce", "甲乙丙").unwrap();
        let mut restored = hidden;
        assert_eq!(
            snapshot.recall_repeated_anchored_suffix_text_after_with_suppressions(
                "abce",
                &mut restored,
                0,
                &suppressions,
                |code, text| code == "abcdef" && text == "甲乙丙",
            ),
            Some(1)
        );
        assert_eq!(restored, ["普通", "甲乙丙", "其他"]);
    }

    #[test]
    fn one_incidental_choice_does_not_replace_repeated_support() {
        let mut snapshot = PersonalRankingSnapshot::default();
        snapshot.record("ab", "甲").unwrap();
        snapshot.record("ab", "甲").unwrap();
        snapshot.record("ab", "乙").unwrap();

        assert_eq!(snapshot.preferred_text("ab"), Some("甲"));
    }

    #[test]
    fn bounded_support_allows_a_deliberate_preference_change() {
        let mut snapshot = PersonalRankingSnapshot::default();
        for _ in 0..32 {
            snapshot.record("ab", "甲").unwrap();
        }
        for _ in 0..PERSONAL_RANKING_SUPPORT_CAP.saturating_sub(1) {
            snapshot.record("ab", "乙").unwrap();
        }
        assert_eq!(snapshot.preferred_text("ab"), Some("甲"));

        snapshot.record("ab", "乙").unwrap();
        assert_eq!(snapshot.preferred_text("ab"), Some("乙"));
    }

    #[test]
    fn explicit_suppression_masks_only_one_exact_identity_and_is_reversible() {
        let mut ranking = PersonalRankingSnapshot::default();
        ranking.record("ab", "甲").unwrap();
        ranking.record("ab", "甲").unwrap();
        ranking.record("ab", "乙").unwrap();
        ranking.record("cd", "甲").unwrap();
        let mut suppressions = PersonalRankingSuppressionSnapshot::default();

        assert!(suppressions.suppress("ab", "甲").unwrap());
        assert!(!suppressions.suppress("ab", "甲").unwrap());
        assert_eq!(suppressions.entry_count(), 1);
        assert_eq!(
            ranking.preferred_text_with_suppressions("ab", &suppressions),
            Some("乙")
        );
        assert_eq!(
            ranking.preferred_text_with_suppressions("cd", &suppressions),
            Some("甲")
        );
        let mut candidates = vec!["固定".to_owned(), "甲".to_owned(), "乙".to_owned()];
        assert!(ranking.promote_texts_after_with_suppressions(
            "ab",
            &mut candidates,
            1,
            &suppressions,
        ));
        assert_eq!(candidates, ["固定", "乙", "甲"]);

        assert!(suppressions.restore("ab", "甲").unwrap());
        assert!(!suppressions.restore("ab", "甲").unwrap());
        assert_eq!(suppressions.entry_count(), 0);
        assert_eq!(
            ranking.preferred_text_with_suppressions("ab", &suppressions),
            Some("甲")
        );
    }

    #[test]
    fn suppression_debug_and_validation_do_not_expose_private_identity_text() {
        let mut suppressions = PersonalRankingSuppressionSnapshot::default();
        suppressions.suppress("qnqn", "亲亲").unwrap();
        let debug = format!("{suppressions:?}");
        assert!(!debug.contains("qnqn"));
        assert!(!debug.contains("亲亲"));
        assert_eq!(
            suppressions.suppress("INVALID", "无效"),
            Err(PersonalRankingError::InvalidCode)
        );
        assert_eq!(
            suppressions.restore("ab", "\n"),
            Err(PersonalRankingError::InvalidText)
        );
    }

    #[test]
    fn protected_suppression_action_round_trips_with_an_independent_identity() {
        let action = suppression_action(
            10,
            20,
            30,
            PersonalRankingSuppressionActionKind::Suppress,
            "qnqn",
            "亲亲",
        );
        let package = protect_personal_ranking_suppression_action(&action, &TestProtector).unwrap();
        assert_eq!(
            unprotect_personal_ranking_suppression_action(&package, &TestProtector).unwrap(),
            action
        );
        assert_eq!(
            action.kind(),
            PersonalRankingSuppressionActionKind::Suppress
        );
        assert_eq!(action.code(), "qnqn");
        assert_eq!(action.text(), "亲亲");
        let debug = format!("{action:?}");
        assert!(!debug.contains("qnqn"));
        assert!(!debug.contains("亲亲"));

        let positive =
            protect_personal_ranking_batch(&batch(10, 20, 30, &[("qnqn", "亲亲")]), &TestProtector)
                .unwrap();
        assert_eq!(
            unprotect_personal_ranking_suppression_action(&positive, &TestProtector),
            Err(PersonalRankingError::InvalidProtectedSuppressionAction)
        );
    }

    #[test]
    fn suppression_action_parser_requires_canonical_plaintext() {
        let action = suppression_action(
            10,
            2,
            3,
            PersonalRankingSuppressionActionKind::Restore,
            "ab",
            "甲",
        );
        let plaintext = action.render().unwrap();
        assert_eq!(
            PersonalRankingSuppressionAction::parse(&plaintext).unwrap(),
            action
        );

        let noncanonical = String::from_utf8(plaintext)
            .unwrap()
            .replace("created_unix_ms=10", "created_unix_ms=010");
        assert_eq!(
            PersonalRankingSuppressionAction::parse(noncanonical.as_bytes()),
            Err(PersonalRankingError::InvalidNumber)
        );
    }

    #[test]
    fn immutable_suppression_actions_replay_canonically_and_restore_exactly() {
        let directory = TestDirectory::new();
        let root = directory.0.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        let restore = suppression_action(
            20,
            1,
            1,
            PersonalRankingSuppressionActionKind::Restore,
            "ab",
            "甲",
        );
        let suppress = suppression_action(
            10,
            1,
            0,
            PersonalRankingSuppressionActionKind::Suppress,
            "ab",
            "甲",
        );
        let other_code = suppression_action(
            15,
            2,
            0,
            PersonalRankingSuppressionActionKind::Suppress,
            "cd",
            "甲",
        );
        save_personal_ranking_suppression_action(&root, &restore, &TestProtector).unwrap();
        save_personal_ranking_suppression_action(&root, &other_code, &TestProtector).unwrap();
        save_personal_ranking_suppression_action(&root, &suppress, &TestProtector).unwrap();

        let loaded = load_personal_ranking_suppressions(&root, &TestProtector).unwrap();
        assert_eq!(loaded.action_count(), 3);
        assert!(!loaded.snapshot().is_suppressed("ab", "甲"));
        assert!(loaded.snapshot().is_suppressed("cd", "甲"));
        let debug = format!("{loaded:?}");
        assert!(!debug.contains("甲"));
        assert!(!debug.contains("ab"));
    }

    #[test]
    fn suppression_refresh_decrypts_only_newer_additions() {
        let directory = TestDirectory::new();
        let root = directory.0.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        let protector = CountingProtector::new();
        save_personal_ranking_suppression_action(
            &root,
            &suppression_action(
                10,
                1,
                0,
                PersonalRankingSuppressionActionKind::Suppress,
                "ab",
                "甲",
            ),
            &protector,
        )
        .unwrap();
        let loaded = load_personal_ranking_suppressions(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls(), 1);
        let unchanged = refresh_personal_ranking_suppressions(&root, &protector, &loaded).unwrap();
        assert_eq!(protector.unprotect_calls(), 1);

        save_personal_ranking_suppression_action(
            &root,
            &suppression_action(
                20,
                1,
                1,
                PersonalRankingSuppressionActionKind::Restore,
                "ab",
                "甲",
            ),
            &protector,
        )
        .unwrap();
        let refreshed =
            refresh_personal_ranking_suppressions(&root, &protector, &unchanged).unwrap();
        assert_eq!(protector.unprotect_calls(), 2);
        assert_eq!(refreshed.action_count(), 2);
        assert!(!refreshed.snapshot().is_suppressed("ab", "甲"));
    }

    #[test]
    fn suppression_refresh_replays_fully_for_older_additions_or_removed_packages() {
        let directory = TestDirectory::new();
        let root = directory.0.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        let protector = CountingProtector::new();
        let later_name = save_personal_ranking_suppression_action(
            &root,
            &suppression_action(
                20,
                1,
                1,
                PersonalRankingSuppressionActionKind::Suppress,
                "ab",
                "甲",
            ),
            &protector,
        )
        .unwrap();
        let loaded = load_personal_ranking_suppressions(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls(), 1);

        save_personal_ranking_suppression_action(
            &root,
            &suppression_action(
                10,
                2,
                0,
                PersonalRankingSuppressionActionKind::Restore,
                "ab",
                "甲",
            ),
            &protector,
        )
        .unwrap();
        let replayed = refresh_personal_ranking_suppressions(&root, &protector, &loaded).unwrap();
        assert_eq!(protector.unprotect_calls(), 4);
        assert!(replayed.snapshot().is_suppressed("ab", "甲"));

        fs::remove_file(root.join(later_name)).unwrap();
        let after_removal =
            refresh_personal_ranking_suppressions(&root, &protector, &replayed).unwrap();
        assert_eq!(after_removal.action_count(), 1);
        assert!(!after_removal.snapshot().is_suppressed("ab", "甲"));
    }

    #[test]
    fn suppression_store_fails_closed_for_digest_drift_and_non_files() {
        let directory = TestDirectory::new();
        let root = directory.0.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        let name = save_personal_ranking_suppression_action(
            &root,
            &suppression_action(
                1,
                1,
                1,
                PersonalRankingSuppressionActionKind::Suppress,
                "ab",
                "甲",
            ),
            &TestProtector,
        )
        .unwrap();
        let path = root.join(&name);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            load_personal_ranking_suppressions(&root, &TestProtector),
            Err(PersonalRankingError::SuppressionPackageIdentifierMismatch)
        );

        fs::remove_file(path).unwrap();
        fs::create_dir(root.join(name)).unwrap();
        assert_eq!(
            load_personal_ranking_suppressions(&root, &TestProtector),
            Err(PersonalRankingError::PackageUnavailable)
        );
    }

    #[test]
    fn suppression_store_is_append_only_across_process_identities_and_bounded() {
        let directory = TestDirectory::new();
        let root = directory.0.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        for process in [10, 20] {
            save_personal_ranking_suppression_action(
                &root,
                &suppression_action(
                    1,
                    process,
                    0,
                    PersonalRankingSuppressionActionKind::Suppress,
                    "ab",
                    "甲",
                ),
                &TestProtector,
            )
            .unwrap();
        }
        assert_eq!(
            personal_ranking_suppression_package_names(&root)
                .unwrap()
                .len(),
            2
        );

        for index in 2..MAX_PERSONAL_RANKING_SUPPRESSION_ACTION_FILES {
            File::create(root.join(format!("suppress-{index:064x}.zps"))).unwrap();
        }
        assert_eq!(
            save_personal_ranking_suppression_action(
                &root,
                &suppression_action(
                    2,
                    30,
                    0,
                    PersonalRankingSuppressionActionKind::Restore,
                    "ab",
                    "甲",
                ),
                &TestProtector,
            ),
            Err(PersonalRankingError::TooManySuppressionActions)
        );
    }

    #[test]
    fn immutable_batches_merge_in_canonical_time_order() {
        let directory = TestDirectory::new();
        let root = directory.root();
        let later = batch(20, 1, 0, &[("ab", "乙")]);
        let earlier = batch(10, 1, 0, &[("ab", "甲")]);
        save_personal_ranking_batch(&root, &later, &TestProtector).unwrap();
        save_personal_ranking_batch(&root, &earlier, &TestProtector).unwrap();

        let loaded = load_personal_ranking(&root, &TestProtector).unwrap();
        assert_eq!(loaded.batch_count(), 2);
        assert_eq!(loaded.selection_count(), 2);
        assert_eq!(loaded.snapshot().preferred_text("ab"), Some("乙"));
    }

    #[test]
    fn checkpoint_replaces_covered_batch_decryption_and_replays_only_the_tail() {
        let directory = TestDirectory::new();
        let root = directory.root();
        let protector = CountingProtector::new();
        for sequence in 0..64_u64 {
            save_personal_ranking_batch(
                &root,
                &batch(sequence.saturating_add(1), 1, sequence, &[("ab", "乙")]),
                &protector,
            )
            .unwrap();
        }
        let loaded = load_personal_ranking(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls(), 64);
        let checkpoint = save_personal_ranking_checkpoint(&root, &loaded, &protector)
            .unwrap()
            .unwrap();

        let before_checkpoint_load = protector.unprotect_calls();
        let checkpointed = load_personal_ranking(&root, &protector).unwrap();
        assert_eq!(
            protector.unprotect_calls() - before_checkpoint_load,
            1,
            "one checkpoint should replace all 64 covered batch decryptions"
        );
        assert_eq!(checkpointed.checkpoint_batch_count(), 64);
        assert_eq!(checkpointed.snapshot().preferred_text("ab"), Some("乙"));
        assert_eq!(
            save_personal_ranking_checkpoint(&root, &checkpointed, &protector)
                .unwrap()
                .as_deref(),
            Some(checkpoint.as_str())
        );
        assert_eq!(personal_ranking_checkpoint_file_count(&root).unwrap(), 1);

        save_personal_ranking_batch(&root, &batch(65, 2, 0, &[("ab", "丙")]), &protector).unwrap();
        let before_tail_load = protector.unprotect_calls();
        let with_tail = load_personal_ranking(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls() - before_tail_load, 2);
        assert_eq!(with_tail.batch_count(), 65);
        assert_eq!(with_tail.checkpoint_batch_count(), 64);
        assert_eq!(with_tail.snapshot().preferred_text("ab"), Some("乙"));

        let checkpoint_path = root.join(checkpoint);
        let mut bytes = fs::read(&checkpoint_path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(checkpoint_path, bytes).unwrap();
        assert!(load_personal_ranking(&root, &protector).is_err());
    }

    #[test]
    fn a_delayed_older_batch_forces_checkpoint_fallback_to_canonical_replay() {
        let directory = TestDirectory::new();
        let root = directory.root();
        for sequence in 0..64_u64 {
            save_personal_ranking_batch(
                &root,
                &batch(
                    100_u64.saturating_add(sequence),
                    1,
                    sequence,
                    &[("ab", "乙")],
                ),
                &TestProtector,
            )
            .unwrap();
        }
        let loaded = load_personal_ranking(&root, &TestProtector).unwrap();
        save_personal_ranking_checkpoint(&root, &loaded, &TestProtector).unwrap();
        save_personal_ranking_batch(&root, &batch(1, 2, 0, &[("ab", "更早")]), &TestProtector)
            .unwrap();

        let replayed = load_personal_ranking(&root, &TestProtector).unwrap();
        assert_eq!(replayed.checkpoint_batch_count(), 0);
        assert_eq!(replayed.snapshot().preferred_text("ab"), Some("乙"));
    }

    #[test]
    fn refresh_reuses_verified_batches_and_only_decrypts_newer_additions() {
        let directory = TestDirectory::new();
        let root = directory.root();
        let protector = CountingProtector::new();
        save_personal_ranking_batch(&root, &batch(10, 1, 0, &[("ab", "甲")]), &protector).unwrap();
        save_personal_ranking_batch(&root, &batch(20, 1, 1, &[("ab", "乙")]), &protector).unwrap();

        let loaded = load_personal_ranking(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls(), 2);
        let unchanged = refresh_personal_ranking(&root, &protector, &loaded).unwrap();
        assert_eq!(protector.unprotect_calls(), 2);
        assert_eq!(unchanged, loaded);

        save_personal_ranking_batch(&root, &batch(30, 2, 0, &[("ab", "丙")]), &protector).unwrap();
        let refreshed = refresh_personal_ranking(&root, &protector, &unchanged).unwrap();
        assert_eq!(protector.unprotect_calls(), 3);
        assert_eq!(refreshed.batch_count(), 3);
        assert_eq!(refreshed.snapshot().preferred_text("ab"), Some("丙"));
    }

    #[test]
    fn refresh_rebuilds_when_a_new_batch_precedes_the_cached_order() {
        let directory = TestDirectory::new();
        let root = directory.root();
        let protector = CountingProtector::new();
        save_personal_ranking_batch(&root, &batch(20, 1, 1, &[("ab", "乙")]), &protector).unwrap();
        let loaded = load_personal_ranking(&root, &protector).unwrap();
        assert_eq!(protector.unprotect_calls(), 1);

        save_personal_ranking_batch(&root, &batch(10, 2, 0, &[("ab", "甲")]), &protector).unwrap();
        let refreshed = refresh_personal_ranking(&root, &protector, &loaded).unwrap();
        assert_eq!(protector.unprotect_calls(), 4);
        assert_eq!(refreshed.snapshot().preferred_text("ab"), Some("乙"));
    }

    #[test]
    fn refresh_drops_cached_evidence_after_the_live_store_is_archived() {
        let directory = TestDirectory::new();
        let root = directory.root();
        save_personal_ranking_batch(&root, &batch(10, 1, 0, &[("ab", "甲")]), &TestProtector)
            .unwrap();
        let loaded = load_personal_ranking(&root, &TestProtector).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let refreshed = refresh_personal_ranking(&root, &TestProtector, &loaded).unwrap();
        assert_eq!(refreshed.batch_count(), 0);
        assert_eq!(refreshed.snapshot().preferred_text("ab"), None);
    }

    #[test]
    fn modified_and_symlink_like_packages_fail_closed() {
        let directory = TestDirectory::new();
        let root = directory.root();
        let installed =
            save_personal_ranking_batch(&root, &batch(1, 1, 1, &[("ab", "甲")]), &TestProtector)
                .unwrap();
        let path = root.join(installed);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert_eq!(
            load_personal_ranking(&root, &TestProtector),
            Err(PersonalRankingError::PackageIdentifierMismatch)
        );
    }

    #[test]
    fn save_refuses_to_create_a_batch_beyond_the_file_limit() {
        let directory = TestDirectory::new();
        let root = directory.root();
        fs::create_dir(&root).unwrap();
        for index in 0..MAX_PERSONAL_RANKING_BATCH_FILES {
            let name = format!("rank-{index:064x}.zpr");
            File::create(root.join(name)).unwrap();
        }
        assert_eq!(
            save_personal_ranking_batch(&root, &batch(1, 1, 1, &[("ab", "甲")]), &TestProtector,),
            Err(PersonalRankingError::TooManyBatches)
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "local DPAPI and durable-storage timing probe"]
    fn windows_dpapi_batch_flush_and_reload_probe() {
        use crate::WindowsUserDataProtector;
        use std::time::Instant;

        let directory = TestDirectory::new();
        let root = directory.root();
        let mut flush_us = Vec::new();
        for sequence in 0..64_u64 {
            let selections = (0..8)
                .map(|index| {
                    PersonalRankingSelection::new(
                        "benchmark",
                        &format!("合成候选{sequence}-{index}"),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let batch = PersonalRankingBatch::new(
                sequence.saturating_add(1),
                std::process::id(),
                sequence,
                selections.clone(),
            )
            .unwrap();
            let started = Instant::now();
            save_personal_ranking_batch(&root, &batch, &WindowsUserDataProtector).unwrap();
            flush_us.push(started.elapsed().as_micros());
        }
        flush_us.sort_unstable();
        let load_started = Instant::now();
        let loaded = load_personal_ranking(&root, &WindowsUserDataProtector).unwrap();
        let load_us = load_started.elapsed().as_micros();
        assert_eq!(loaded.batch_count(), 64);
        assert_eq!(loaded.selection_count(), 512);
        save_personal_ranking_checkpoint(&root, &loaded, &WindowsUserDataProtector)
            .unwrap()
            .unwrap();
        let checkpoint_load_started = Instant::now();
        let checkpointed = load_personal_ranking(&root, &WindowsUserDataProtector).unwrap();
        let checkpoint_load_us = checkpoint_load_started.elapsed().as_micros();
        assert_eq!(checkpointed.checkpoint_batch_count(), 64);
        let unchanged_started = Instant::now();
        let unchanged =
            refresh_personal_ranking(&root, &WindowsUserDataProtector, &loaded).unwrap();
        let unchanged_us = unchanged_started.elapsed().as_micros();
        let additional = PersonalRankingBatch::new(
            65,
            std::process::id(),
            64,
            vec![PersonalRankingSelection::new("benchmark", "新增候选").unwrap()],
        )
        .unwrap();
        save_personal_ranking_batch(&root, &additional, &WindowsUserDataProtector).unwrap();
        let incremental_started = Instant::now();
        let refreshed =
            refresh_personal_ranking(&root, &WindowsUserDataProtector, &unchanged).unwrap();
        let incremental_us = incremental_started.elapsed().as_micros();
        assert_eq!(refreshed.batch_count(), 65);
        println!(
            "PERSONAL_RANKING_IO flush_median_us={} flush_p95_us={} load_64_batches_us={load_us} load_checkpoint_us={checkpoint_load_us} refresh_unchanged_us={unchanged_us} refresh_one_new_us={incremental_us}",
            flush_us[flush_us.len() / 2],
            flush_us[(flush_us.len() * 95 / 100).min(flush_us.len() - 1)]
        );
    }
}
