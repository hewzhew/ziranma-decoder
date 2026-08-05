//! Pure state transitions for immutable candidate-package slots.
//!
//! This module parses and changes only an explicitly supplied state string.
//! It performs no file discovery, persistence, package loading, or network I/O.

use std::error::Error;
use std::fmt;

/// First candidate-package slot-state schema.
pub const CANDIDATE_SLOT_STATE_SCHEMA_V1: &str = "ziranma-candidate-slots-v1";
/// Maximum bytes accepted for one textual slot-state file.
pub const MAX_CANDIDATE_SLOT_STATE_BYTES: usize = 512;

/// Immutable-package references for the active, staged, and rollback slots.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateSlotState {
    current: Option<String>,
    candidate: Option<String>,
    previous: Option<String>,
}

impl CandidateSlotState {
    /// Parses the exact four-line, LF-terminated v1 state.
    pub fn parse(contents: &str) -> Result<Self, CandidateSlotError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_SLOT_STATE_BYTES {
            return Err(CandidateSlotError::InvalidStateSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateSlotError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 5 || !lines[4].is_empty() {
            return Err(CandidateSlotError::InvalidStructure);
        }
        if field(lines[0], "schema")? != CANDIDATE_SLOT_STATE_SCHEMA_V1 {
            return Err(CandidateSlotError::UnsupportedSchema);
        }
        let state = Self {
            current: optional_package_id(field(lines[1], "current")?)?,
            candidate: optional_package_id(field(lines[2], "candidate")?)?,
            previous: optional_package_id(field(lines[3], "previous")?)?,
        };
        state.validate()?;
        Ok(state)
    }

    /// Renders the canonical four-line v1 state.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_SLOT_STATE_SCHEMA_V1}\ncurrent={}\ncandidate={}\nprevious={}\n",
            self.current.as_deref().unwrap_or("-"),
            self.candidate.as_deref().unwrap_or("-"),
            self.previous.as_deref().unwrap_or("-")
        )
    }

    /// Sets the first known-good package. Existing state is never replaced.
    pub fn adopt(&mut self, package_id: &str) -> Result<(), CandidateSlotError> {
        validate_candidate_package_id(package_id)?;
        if self.current.is_some() || self.candidate.is_some() || self.previous.is_some() {
            return Err(CandidateSlotError::AlreadyConfigured);
        }
        self.current = Some(package_id.to_owned());
        Ok(())
    }

    /// Places one independently validated package in the candidate slot.
    pub fn stage(&mut self, package_id: &str) -> Result<(), CandidateSlotError> {
        validate_candidate_package_id(package_id)?;
        if self.current.is_none() {
            return Err(CandidateSlotError::NotConfigured);
        }
        if self.current.as_deref() == Some(package_id)
            || self.previous.as_deref() == Some(package_id)
        {
            return Err(CandidateSlotError::DuplicatePackage);
        }
        self.candidate = Some(package_id.to_owned());
        Ok(())
    }

    /// Promotes the staged package and retains the old current package.
    pub fn promote(&mut self) -> Result<(), CandidateSlotError> {
        let next = self
            .candidate
            .as_ref()
            .cloned()
            .ok_or(CandidateSlotError::CandidateEmpty)?;
        let old = self
            .current
            .as_ref()
            .cloned()
            .ok_or(CandidateSlotError::CurrentEmpty)?;
        self.current = Some(next);
        self.candidate = None;
        self.previous = Some(old);
        Ok(())
    }

    /// Swaps current and previous without deleting either package.
    pub fn rollback(&mut self) -> Result<(), CandidateSlotError> {
        let current = self
            .current
            .as_ref()
            .cloned()
            .ok_or(CandidateSlotError::CurrentEmpty)?;
        let previous = self
            .previous
            .as_ref()
            .cloned()
            .ok_or(CandidateSlotError::PreviousEmpty)?;
        self.current = Some(previous);
        self.previous = Some(current);
        Ok(())
    }

    /// Returns the installed package identifier in the current slot.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Returns the installed package identifier in the candidate slot.
    pub fn candidate(&self) -> Option<&str> {
        self.candidate.as_deref()
    }

    /// Returns the installed package identifier in the previous slot.
    pub fn previous(&self) -> Option<&str> {
        self.previous.as_deref()
    }

    fn validate(&self) -> Result<(), CandidateSlotError> {
        if self.current.is_none() && (self.candidate.is_some() || self.previous.is_some()) {
            return Err(CandidateSlotError::InvalidCombination);
        }
        let occupied = [
            self.current.as_deref(),
            self.candidate.as_deref(),
            self.previous.as_deref(),
        ];
        for (index, package_id) in occupied.iter().enumerate() {
            if let Some(package_id) = package_id {
                validate_candidate_package_id(package_id)?;
                if occupied[index + 1..].contains(&Some(*package_id)) {
                    return Err(CandidateSlotError::DuplicatePackage);
                }
            }
        }
        Ok(())
    }
}

/// Errors returned by candidate slot parsing and transitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateSlotError {
    /// The state is empty or exceeds its fixed bound.
    InvalidStateSize,
    /// The state does not use the exact four-line shape.
    InvalidStructure,
    /// A field is missing, reordered, duplicated, or empty.
    InvalidField,
    /// The state schema is unsupported.
    UnsupportedSchema,
    /// A package identifier is outside the fixed internal grammar.
    InvalidPackageId,
    /// Candidate or previous exists without a current package.
    InvalidCombination,
    /// One immutable package occupies more than one slot.
    DuplicatePackage,
    /// The first package has already been adopted.
    AlreadyConfigured,
    /// A stage operation was requested before initial adoption.
    NotConfigured,
    /// The current slot is empty.
    CurrentEmpty,
    /// The candidate slot is empty.
    CandidateEmpty,
    /// The previous slot is empty.
    PreviousEmpty,
}

impl fmt::Display for CandidateSlotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStateSize => write!(formatter, "候选槽状态大小无效"),
            Self::InvalidStructure => write!(formatter, "候选槽状态结构无效"),
            Self::InvalidField => write!(formatter, "候选槽状态字段无效"),
            Self::UnsupportedSchema => write!(formatter, "不支持的候选槽状态格式"),
            Self::InvalidPackageId => write!(formatter, "候选包内部标识无效"),
            Self::InvalidCombination => write!(formatter, "候选槽状态组合无效"),
            Self::DuplicatePackage => write!(formatter, "同一候选包不能占用多个槽位"),
            Self::AlreadyConfigured => write!(formatter, "当前候选包已经配置"),
            Self::NotConfigured => write!(formatter, "尚未配置当前候选包"),
            Self::CurrentEmpty => write!(formatter, "当前候选槽为空"),
            Self::CandidateEmpty => write!(formatter, "待切换候选槽为空"),
            Self::PreviousEmpty => write!(formatter, "回退候选槽为空"),
        }
    }
}

impl Error for CandidateSlotError {}

fn field<'a>(line: &'a str, expected_key: &str) -> Result<&'a str, CandidateSlotError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateSlotError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateSlotError::InvalidField);
    }
    Ok(value)
}

fn optional_package_id(value: &str) -> Result<Option<String>, CandidateSlotError> {
    if value == "-" {
        return Ok(None);
    }
    validate_candidate_package_id(value)?;
    Ok(Some(value.to_owned()))
}

pub(crate) fn validate_candidate_package_id(value: &str) -> Result<(), CandidateSlotError> {
    let Some(rest) = value.strip_prefix("pkg-") else {
        return Err(CandidateSlotError::InvalidPackageId);
    };
    let Some((manifest, payload)) = rest.split_once('-') else {
        return Err(CandidateSlotError::InvalidPackageId);
    };
    if manifest.len() != 16
        || payload.len() != 16
        || !manifest
            .bytes()
            .chain(payload.bytes())
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CandidateSlotError::InvalidPackageId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "pkg-1111111111111111-aaaaaaaaaaaaaaaa";
    const B: &str = "pkg-2222222222222222-bbbbbbbbbbbbbbbb";
    const C: &str = "pkg-3333333333333333-cccccccccccccccc";

    #[test]
    fn state_round_trips_only_the_exact_canonical_shape() {
        let mut state = CandidateSlotState::default();
        state.adopt(A).unwrap();
        state.stage(B).unwrap();
        let rendered = state.render();
        assert_eq!(CandidateSlotState::parse(&rendered).unwrap(), state);
        assert_eq!(
            rendered,
            "schema=ziranma-candidate-slots-v1\n\
             current=pkg-1111111111111111-aaaaaaaaaaaaaaaa\n\
             candidate=pkg-2222222222222222-bbbbbbbbbbbbbbbb\n\
             previous=-\n"
        );
        assert!(CandidateSlotState::parse(&rendered.replace('\n', "\r\n")).is_err());
        assert!(CandidateSlotState::parse(&format!("{rendered}extra=x\n")).is_err());
    }

    #[test]
    fn promote_and_rollback_retain_known_good_packages() {
        let mut state = CandidateSlotState::default();
        state.adopt(A).unwrap();
        state.stage(B).unwrap();
        state.promote().unwrap();
        assert_eq!(state.current(), Some(B));
        assert_eq!(state.candidate(), None);
        assert_eq!(state.previous(), Some(A));

        state.rollback().unwrap();
        assert_eq!(state.current(), Some(A));
        assert_eq!(state.previous(), Some(B));

        state.stage(C).unwrap();
        state.promote().unwrap();
        assert_eq!(state.current(), Some(C));
        assert_eq!(state.previous(), Some(A));
    }

    #[test]
    fn invalid_or_duplicate_transitions_never_mutate_state() {
        let mut state = CandidateSlotState::default();
        let empty = state.clone();
        assert_eq!(state.stage(A), Err(CandidateSlotError::NotConfigured));
        assert_eq!(state, empty);
        assert_eq!(state.promote(), Err(CandidateSlotError::CandidateEmpty));
        assert_eq!(state, empty);

        state.adopt(A).unwrap();
        let adopted = state.clone();
        assert_eq!(state.adopt(B), Err(CandidateSlotError::AlreadyConfigured));
        assert_eq!(state.stage(A), Err(CandidateSlotError::DuplicatePackage));
        assert_eq!(state, adopted);
        assert_eq!(state.rollback(), Err(CandidateSlotError::PreviousEmpty));
        assert_eq!(state, adopted);
    }
}
