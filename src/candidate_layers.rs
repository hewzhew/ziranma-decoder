//! Pure configuration for one optional public supplemental candidate layer.
//!
//! The state binds an explicit immutable package and a fixed influence cap.
//! It performs no file discovery, persistence, decoding, or network access.

use std::error::Error;
use std::fmt;

use crate::{MAX_CANDIDATE_SNAPSHOT_RANK, candidate_slots::validate_candidate_package_id};

/// Fixed state file inside an independently managed supplemental slot root.
pub const CANDIDATE_SUPPLEMENTAL_STATE_FILE: &str = "supplemental.zcl";
/// First supplemental candidate-layer state schema.
pub const CANDIDATE_SUPPLEMENTAL_STATE_SCHEMA_V1: &str = "ziranma-candidate-supplemental-v1";
/// Maximum accepted size of one supplemental-layer state file.
pub const MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES: usize = 512;

/// Explicit activation state for one public supplemental exact-word package.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateSupplementalState {
    package: Option<String>,
    exact_promotions: usize,
}

impl CandidateSupplementalState {
    /// Constructs an enabled state bound to one immutable package.
    pub fn enabled(
        package: &str,
        exact_promotions: usize,
    ) -> Result<Self, CandidateSupplementalStateError> {
        validate_candidate_package_id(package)
            .map_err(|_| CandidateSupplementalStateError::InvalidPackageId)?;
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&exact_promotions) {
            return Err(CandidateSupplementalStateError::InvalidPromotionLimit);
        }
        Ok(Self {
            package: Some(package.to_owned()),
            exact_promotions,
        })
    }

    /// Parses the exact four-line, LF-terminated state.
    pub fn parse(contents: &str) -> Result<Self, CandidateSupplementalStateError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES {
            return Err(CandidateSupplementalStateError::InvalidStateSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateSupplementalStateError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 5 || !lines[4].is_empty() {
            return Err(CandidateSupplementalStateError::InvalidStructure);
        }
        if field(lines[0], "schema")? != CANDIDATE_SUPPLEMENTAL_STATE_SCHEMA_V1 {
            return Err(CandidateSupplementalStateError::UnsupportedSchema);
        }
        let enabled = match field(lines[1], "enabled")? {
            "true" => true,
            "false" => false,
            _ => return Err(CandidateSupplementalStateError::InvalidField),
        };
        let package = field(lines[2], "package")?;
        let exact_promotions = canonical_usize(field(lines[3], "exact_promotions")?)?;
        match (enabled, package, exact_promotions) {
            (false, "-", 0) => Ok(Self::default()),
            (true, package, exact_promotions) => Self::enabled(package, exact_promotions),
            _ => Err(CandidateSupplementalStateError::InvalidCombination),
        }
    }

    /// Renders the canonical state representation.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_SUPPLEMENTAL_STATE_SCHEMA_V1}\nenabled={}\npackage={}\nexact_promotions={}\n",
            self.package.is_some(),
            self.package.as_deref().unwrap_or("-"),
            self.exact_promotions,
        )
    }

    /// Returns whether the supplemental lane is explicitly enabled.
    pub fn is_enabled(&self) -> bool {
        self.package.is_some()
    }

    /// Returns the immutable package bound to the enabled state.
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }

    /// Returns the maximum number of supplemental exact words per code.
    pub fn exact_promotions(&self) -> usize {
        self.exact_promotions
    }
}

/// Strict supplemental-layer state parsing errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSupplementalStateError {
    /// The input is empty or exceeds its fixed bound.
    InvalidStateSize,
    /// The input is not the exact canonical four-line shape.
    InvalidStructure,
    /// A field is missing, reordered, duplicated, or noncanonical.
    InvalidField,
    /// The schema identifier is not supported.
    UnsupportedSchema,
    /// The enabled package identifier is outside the internal grammar.
    InvalidPackageId,
    /// The enabled and disabled fields form an invalid combination.
    InvalidCombination,
    /// The exact-word influence cap is outside the fixed snapshot bound.
    InvalidPromotionLimit,
}

impl fmt::Display for CandidateSupplementalStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidStateSize => "补充词层状态大小无效",
            Self::InvalidStructure => "补充词层状态结构无效",
            Self::InvalidField => "补充词层状态字段无效",
            Self::UnsupportedSchema => "不支持的补充词层状态格式",
            Self::InvalidPackageId => "补充词层候选包标识无效",
            Self::InvalidCombination => "补充词层状态组合无效",
            Self::InvalidPromotionLimit => "补充词层影响上限无效",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateSupplementalStateError {}

fn field<'a>(
    line: &'a str,
    expected_key: &str,
) -> Result<&'a str, CandidateSupplementalStateError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateSupplementalStateError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateSupplementalStateError::InvalidField);
    }
    Ok(value)
}

fn canonical_usize(value: &str) -> Result<usize, CandidateSupplementalStateError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CandidateSupplementalStateError::InvalidField)?;
    if parsed.to_string() != value {
        return Err(CandidateSupplementalStateError::InvalidField);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "pkg-1111111111111111-aaaaaaaaaaaaaaaa";

    #[test]
    fn disabled_and_enabled_states_round_trip_canonically() {
        let disabled = CandidateSupplementalState::default();
        assert_eq!(
            disabled.render(),
            "schema=ziranma-candidate-supplemental-v1\n\
             enabled=false\n\
             package=-\n\
             exact_promotions=0\n"
        );
        assert_eq!(
            CandidateSupplementalState::parse(&disabled.render()).unwrap(),
            disabled
        );

        let enabled = CandidateSupplementalState::enabled(PACKAGE, 1).unwrap();
        assert!(enabled.is_enabled());
        assert_eq!(enabled.package(), Some(PACKAGE));
        assert_eq!(enabled.exact_promotions(), 1);
        assert_eq!(
            CandidateSupplementalState::parse(&enabled.render()).unwrap(),
            enabled
        );
    }

    #[test]
    fn parser_rejects_noncanonical_and_unbounded_states() {
        let valid = CandidateSupplementalState::enabled(PACKAGE, 1)
            .unwrap()
            .render();
        assert_eq!(
            CandidateSupplementalState::parse(&valid.replace("enabled=true", "enabled=yes"))
                .unwrap_err(),
            CandidateSupplementalStateError::InvalidField
        );
        assert_eq!(
            CandidateSupplementalState::parse(
                &valid.replace("exact_promotions=1", "exact_promotions=01")
            )
            .unwrap_err(),
            CandidateSupplementalStateError::InvalidField
        );
        assert_eq!(
            CandidateSupplementalState::parse(
                &valid.replace("exact_promotions=1", "exact_promotions=51")
            )
            .unwrap_err(),
            CandidateSupplementalStateError::InvalidPromotionLimit
        );
        assert_eq!(
            CandidateSupplementalState::parse(
                &valid.replace("package=pkg-1111111111111111-aaaaaaaaaaaaaaaa", "package=-")
            )
            .unwrap_err(),
            CandidateSupplementalStateError::InvalidPackageId
        );
        assert!(CandidateSupplementalState::parse(&valid.replace('\n', "\r\n")).is_err());
        assert!(CandidateSupplementalState::parse(&format!("{valid}extra=x\n")).is_err());
    }
}
