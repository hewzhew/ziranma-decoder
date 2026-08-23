//! Pure configuration for one optional public supplemental candidate layer.
//!
//! The state binds an explicit immutable package and a fixed influence cap.
//! It performs no file discovery, persistence, decoding, or network access.

use std::error::Error;
use std::fmt;

use crate::{
    MAX_CANDIDATE_SNAPSHOT_RANK, MAX_EXACT_SHORT_WORDS_PER_CODE,
    candidate_slots::validate_candidate_package_id,
};

/// Fixed state file inside an independently managed supplemental slot root.
pub const CANDIDATE_SUPPLEMENTAL_STATE_FILE: &str = "supplemental.zcl";
/// First supplemental candidate-layer state schema.
pub const CANDIDATE_SUPPLEMENTAL_STATE_SCHEMA_V1: &str = "ziranma-candidate-supplemental-v1";
/// Maximum accepted size of one supplemental-layer state file.
pub const MAX_CANDIDATE_SUPPLEMENTAL_STATE_BYTES: usize = 512;

/// Fixed state file inside an independently managed exact-short slot root.
pub const CANDIDATE_EXACT_SHORT_STATE_FILE: &str = "exact-short.zcl";
/// First exact-short candidate-layer state schema.
pub const CANDIDATE_EXACT_SHORT_STATE_SCHEMA_V1: &str = "ziranma-candidate-exact-short-v1";
/// Maximum accepted size of one exact-short layer state file.
pub const MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES: usize = 512;
/// Fixed receipt written only after the combined TSF second-page path passes.
pub const CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_FILE: &str = "exact-short-preflight.zep";
/// First combined exact-short preflight receipt schema.
pub const CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_SCHEMA_V1: &str =
    "ziranma-candidate-exact-short-preflight-v1";
/// Synthetic TSF path exercised by the combined exact-short preflight.
pub const CANDIDATE_EXACT_SHORT_PREFLIGHT_HOST_V1: &str = "tsf-exact-short-second-page-context-v1";
/// Page width authenticated by the combined exact-short preflight.
pub const CANDIDATE_EXACT_SHORT_PREFLIGHT_PAGE_SIZE: usize = 6;
/// Maximum accepted size of one combined exact-short preflight receipt.
pub const MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES: usize = 1024;

/// Fixed state file inside an independently managed exact-phrase slot root.
pub const CANDIDATE_EXACT_PHRASE_STATE_FILE: &str = "exact-phrase.zcl";
/// First exact three-character phrase activation schema.
pub const CANDIDATE_EXACT_PHRASE_STATE_SCHEMA_V1: &str = "ziranma-candidate-exact-phrase-v1";
/// Maximum accepted size of one exact-phrase activation state.
pub const MAX_CANDIDATE_EXACT_PHRASE_STATE_BYTES: usize = 512;
/// Fixed receipt reserved for the future real TSF first-page composition gate.
pub const CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_FILE: &str = "exact-phrase-preflight.zep";
/// First combined exact-phrase preflight receipt schema.
pub const CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_SCHEMA_V1: &str =
    "ziranma-candidate-exact-phrase-preflight-v1";
/// Real TSF path that must be exercised before this layer can load.
pub const CANDIDATE_EXACT_PHRASE_PREFLIGHT_HOST_V1: &str = "tsf-exact-phrase-first-page-context-v1";
/// Candidate page width bound by the exact-phrase combined receipt.
pub const CANDIDATE_EXACT_PHRASE_PREFLIGHT_PAGE_SIZE: usize = 6;
/// Maximum accepted size of one exact-phrase combined receipt.
pub const MAX_CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_BYTES: usize = 1024;

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

/// Explicit activation state for one public exact-short package.
///
/// This state is deliberately separate from [`CandidateSupplementalState`]:
/// the supplemental layer participates in the primary query, while this
/// layer may insert only after the first candidate page has been frozen.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateExactShortState {
    package: Option<String>,
    exact_promotions: usize,
}

impl CandidateExactShortState {
    /// Constructs an enabled state bound to one immutable package.
    pub fn enabled(
        package: &str,
        exact_promotions: usize,
    ) -> Result<Self, CandidateExactShortStateError> {
        validate_candidate_package_id(package)
            .map_err(|_| CandidateExactShortStateError::InvalidPackageId)?;
        if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
            return Err(CandidateExactShortStateError::InvalidPromotionLimit);
        }
        Ok(Self {
            package: Some(package.to_owned()),
            exact_promotions,
        })
    }

    /// Parses the exact four-line, LF-terminated state.
    pub fn parse(contents: &str) -> Result<Self, CandidateExactShortStateError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_EXACT_SHORT_STATE_BYTES {
            return Err(CandidateExactShortStateError::InvalidStateSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateExactShortStateError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 5 || !lines[4].is_empty() {
            return Err(CandidateExactShortStateError::InvalidStructure);
        }
        if exact_short_field(lines[0], "schema")? != CANDIDATE_EXACT_SHORT_STATE_SCHEMA_V1 {
            return Err(CandidateExactShortStateError::UnsupportedSchema);
        }
        let enabled = match exact_short_field(lines[1], "enabled")? {
            "true" => true,
            "false" => false,
            _ => return Err(CandidateExactShortStateError::InvalidField),
        };
        let package = exact_short_field(lines[2], "package")?;
        let exact_promotions =
            exact_short_canonical_usize(exact_short_field(lines[3], "exact_promotions")?)?;
        match (enabled, package, exact_promotions) {
            (false, "-", 0) => Ok(Self::default()),
            (true, package, exact_promotions) => Self::enabled(package, exact_promotions),
            _ => Err(CandidateExactShortStateError::InvalidCombination),
        }
    }

    /// Renders the canonical state representation.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_EXACT_SHORT_STATE_SCHEMA_V1}\nenabled={}\npackage={}\nexact_promotions={}\n",
            self.package.is_some(),
            self.package.as_deref().unwrap_or("-"),
            self.exact_promotions,
        )
    }

    /// Returns whether the exact-short lane is explicitly enabled.
    pub fn is_enabled(&self) -> bool {
        self.package.is_some()
    }

    /// Returns the immutable package bound to the enabled state.
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }

    /// Returns the maximum number of exact-short insertions per code.
    pub fn exact_promotions(&self) -> usize {
        self.exact_promotions
    }
}

/// Strict exact-short layer state parsing errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateExactShortStateError {
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
    /// The insertion cap is outside the fixed per-code catalog depth.
    InvalidPromotionLimit,
}

impl fmt::Display for CandidateExactShortStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidStateSize => "精确短词层状态大小无效",
            Self::InvalidStructure => "精确短词层状态结构无效",
            Self::InvalidField => "精确短词层状态字段无效",
            Self::UnsupportedSchema => "不支持的精确短词层状态格式",
            Self::InvalidPackageId => "精确短词层候选包标识无效",
            Self::InvalidCombination => "精确短词层状态组合无效",
            Self::InvalidPromotionLimit => "精确短词层影响上限无效",
        };
        formatter.write_str(message)
    }
}

impl Error for CandidateExactShortStateError {}

/// Explicit activation state for one exact three-character phrase package.
///
/// The package identity is the only variable. Its insertion rule is fixed by
/// the schema and must later be authenticated by a separate combined receipt.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateExactPhraseState {
    package: Option<String>,
}

impl CandidateExactPhraseState {
    /// Constructs an enabled state bound to one immutable package.
    pub fn enabled(package: &str) -> Result<Self, CandidateExactPhraseStateError> {
        validate_candidate_package_id(package)
            .map_err(|_| CandidateExactPhraseStateError::InvalidPackageId)?;
        Ok(Self {
            package: Some(package.to_owned()),
        })
    }

    /// Parses the exact three-line, LF-terminated state.
    pub fn parse(contents: &str) -> Result<Self, CandidateExactPhraseStateError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_EXACT_PHRASE_STATE_BYTES {
            return Err(CandidateExactPhraseStateError::InvalidStateSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateExactPhraseStateError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 4 || !lines[3].is_empty() {
            return Err(CandidateExactPhraseStateError::InvalidStructure);
        }
        if exact_phrase_state_field(lines[0], "schema")? != CANDIDATE_EXACT_PHRASE_STATE_SCHEMA_V1 {
            return Err(CandidateExactPhraseStateError::UnsupportedSchema);
        }
        let enabled = match exact_phrase_state_field(lines[1], "enabled")? {
            "true" => true,
            "false" => false,
            _ => return Err(CandidateExactPhraseStateError::InvalidField),
        };
        let package = exact_phrase_state_field(lines[2], "package")?;
        match (enabled, package) {
            (false, "-") => Ok(Self::default()),
            (true, package) => Self::enabled(package),
            _ => Err(CandidateExactPhraseStateError::InvalidCombination),
        }
    }

    /// Renders the canonical activation state.
    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_EXACT_PHRASE_STATE_SCHEMA_V1}\nenabled={}\npackage={}\n",
            self.package.is_some(),
            self.package.as_deref().unwrap_or("-"),
        )
    }

    /// Returns whether the exact-phrase lane is explicitly enabled.
    pub fn is_enabled(&self) -> bool {
        self.package.is_some()
    }

    /// Returns the immutable package selected by this state.
    pub fn package(&self) -> Option<&str> {
        self.package.as_deref()
    }
}

/// Strict exact-phrase activation-state errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateExactPhraseStateError {
    InvalidStateSize,
    InvalidStructure,
    InvalidField,
    UnsupportedSchema,
    InvalidPackageId,
    InvalidCombination,
}

impl fmt::Display for CandidateExactPhraseStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStateSize => "三字精确层状态大小无效",
            Self::InvalidStructure => "三字精确层状态结构无效",
            Self::InvalidField => "三字精确层状态字段无效",
            Self::UnsupportedSchema => "不支持的三字精确层状态格式",
            Self::InvalidPackageId => "三字精确层候选包标识无效",
            Self::InvalidCombination => "三字精确层状态组合无效",
        })
    }
}

impl Error for CandidateExactPhraseStateError {}

/// Immutable evidence reserved for a real TSF first-page three-layer gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateExactPhrasePreflightReceipt {
    phrase_package: String,
    phrase_sha256: String,
    core_sha256: String,
    supplemental_sha256: String,
    supplemental_promotions: usize,
}

impl CandidateExactPhrasePreflightReceipt {
    pub fn new(
        phrase_package: &str,
        phrase_sha256: &str,
        core_sha256: &str,
        supplemental_sha256: &str,
        supplemental_promotions: usize,
    ) -> Result<Self, CandidateExactPhrasePreflightReceiptError> {
        validate_candidate_package_id(phrase_package)
            .map_err(|_| CandidateExactPhrasePreflightReceiptError::InvalidPackageId)?;
        if !exact_phrase_sha256(phrase_sha256)
            || !exact_phrase_sha256(core_sha256)
            || !exact_phrase_sha256(supplemental_sha256)
        {
            return Err(CandidateExactPhrasePreflightReceiptError::InvalidSha256);
        }
        if supplemental_promotions != 1 {
            return Err(CandidateExactPhrasePreflightReceiptError::InvalidPromotionLimit);
        }
        Ok(Self {
            phrase_package: phrase_package.to_owned(),
            phrase_sha256: phrase_sha256.to_owned(),
            core_sha256: core_sha256.to_owned(),
            supplemental_sha256: supplemental_sha256.to_owned(),
            supplemental_promotions,
        })
    }

    pub fn parse(contents: &str) -> Result<Self, CandidateExactPhrasePreflightReceiptError> {
        if contents.is_empty()
            || contents.len() > MAX_CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_BYTES
        {
            return Err(CandidateExactPhrasePreflightReceiptError::InvalidReceiptSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateExactPhrasePreflightReceiptError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 9 || !lines[8].is_empty() {
            return Err(CandidateExactPhrasePreflightReceiptError::InvalidStructure);
        }
        if exact_phrase_receipt_field(lines[0], "schema")?
            != CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_SCHEMA_V1
            || exact_phrase_receipt_field(lines[6], "page_size")?
                != CANDIDATE_EXACT_PHRASE_PREFLIGHT_PAGE_SIZE.to_string()
            || exact_phrase_receipt_field(lines[7], "host")?
                != CANDIDATE_EXACT_PHRASE_PREFLIGHT_HOST_V1
        {
            return Err(CandidateExactPhrasePreflightReceiptError::UnsupportedProfile);
        }
        Self::new(
            exact_phrase_receipt_field(lines[1], "phrase_package")?,
            exact_phrase_receipt_field(lines[2], "phrase_sha256")?,
            exact_phrase_receipt_field(lines[3], "core_sha256")?,
            exact_phrase_receipt_field(lines[4], "supplemental_sha256")?,
            exact_phrase_receipt_usize(exact_phrase_receipt_field(
                lines[5],
                "supplemental_promotions",
            )?)?,
        )
    }

    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_EXACT_PHRASE_PREFLIGHT_RECEIPT_SCHEMA_V1}\nphrase_package={}\nphrase_sha256={}\ncore_sha256={}\nsupplemental_sha256={}\nsupplemental_promotions={}\npage_size={CANDIDATE_EXACT_PHRASE_PREFLIGHT_PAGE_SIZE}\nhost={CANDIDATE_EXACT_PHRASE_PREFLIGHT_HOST_V1}\n",
            self.phrase_package,
            self.phrase_sha256,
            self.core_sha256,
            self.supplemental_sha256,
            self.supplemental_promotions,
        )
    }

    pub fn phrase_package(&self) -> &str {
        &self.phrase_package
    }

    pub fn phrase_sha256(&self) -> &str {
        &self.phrase_sha256
    }

    pub fn matches_runtime(
        &self,
        core_sha256: &str,
        supplemental_sha256: &str,
        supplemental_promotions: usize,
    ) -> bool {
        self.core_sha256 == core_sha256
            && self.supplemental_sha256 == supplemental_sha256
            && self.supplemental_promotions == supplemental_promotions
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateExactPhrasePreflightReceiptError {
    InvalidReceiptSize,
    InvalidStructure,
    InvalidField,
    UnsupportedProfile,
    InvalidPackageId,
    InvalidSha256,
    InvalidPromotionLimit,
}

impl fmt::Display for CandidateExactPhrasePreflightReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidReceiptSize => "三字精确层专项预检凭据大小无效",
            Self::InvalidStructure => "三字精确层专项预检凭据结构无效",
            Self::InvalidField => "三字精确层专项预检凭据字段无效",
            Self::UnsupportedProfile => "三字精确层专项预检配置不受支持",
            Self::InvalidPackageId => "三字精确层专项预检包标识无效",
            Self::InvalidSha256 => "三字精确层专项预检摘要无效",
            Self::InvalidPromotionLimit => "三字精确层专项预检补充影响上限无效",
        })
    }
}

impl Error for CandidateExactPhrasePreflightReceiptError {}

fn exact_phrase_state_field<'a>(
    line: &'a str,
    expected_key: &str,
) -> Result<&'a str, CandidateExactPhraseStateError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateExactPhraseStateError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateExactPhraseStateError::InvalidField);
    }
    Ok(value)
}

fn exact_phrase_receipt_field<'a>(
    line: &'a str,
    expected_key: &str,
) -> Result<&'a str, CandidateExactPhrasePreflightReceiptError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateExactPhrasePreflightReceiptError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateExactPhrasePreflightReceiptError::InvalidField);
    }
    Ok(value)
}

fn exact_phrase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn exact_phrase_receipt_usize(
    value: &str,
) -> Result<usize, CandidateExactPhrasePreflightReceiptError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CandidateExactPhrasePreflightReceiptError::InvalidField)?;
    if parsed.to_string() != value {
        return Err(CandidateExactPhrasePreflightReceiptError::InvalidField);
    }
    Ok(parsed)
}

/// Immutable evidence that one exact-short package and its complete public
/// runtime context passed the real TSF second-page path together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateExactShortPreflightReceipt {
    exact_package: String,
    exact_sha256: String,
    exact_promotions: usize,
    core_sha256: String,
    supplemental_sha256: Option<String>,
    supplemental_promotions: usize,
}

impl CandidateExactShortPreflightReceipt {
    pub fn new(
        exact_package: &str,
        exact_sha256: &str,
        exact_promotions: usize,
        core_sha256: &str,
        supplemental: Option<(&str, usize)>,
    ) -> Result<Self, CandidateExactShortPreflightReceiptError> {
        validate_candidate_package_id(exact_package)
            .map_err(|_| CandidateExactShortPreflightReceiptError::InvalidPackageId)?;
        if !exact_short_sha256(exact_sha256) || !exact_short_sha256(core_sha256) {
            return Err(CandidateExactShortPreflightReceiptError::InvalidSha256);
        }
        if !(1..=MAX_EXACT_SHORT_WORDS_PER_CODE).contains(&exact_promotions) {
            return Err(CandidateExactShortPreflightReceiptError::InvalidPromotionLimit);
        }
        let (supplemental_sha256, supplemental_promotions) = match supplemental {
            Some((sha256, promotions))
                if exact_short_sha256(sha256)
                    && (1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&promotions) =>
            {
                (Some(sha256.to_owned()), promotions)
            }
            Some((sha256, _)) if !exact_short_sha256(sha256) => {
                return Err(CandidateExactShortPreflightReceiptError::InvalidSha256);
            }
            Some(_) => {
                return Err(CandidateExactShortPreflightReceiptError::InvalidPromotionLimit);
            }
            None => (None, 0),
        };
        Ok(Self {
            exact_package: exact_package.to_owned(),
            exact_sha256: exact_sha256.to_owned(),
            exact_promotions,
            core_sha256: core_sha256.to_owned(),
            supplemental_sha256,
            supplemental_promotions,
        })
    }

    pub fn parse(contents: &str) -> Result<Self, CandidateExactShortPreflightReceiptError> {
        if contents.is_empty() || contents.len() > MAX_CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_BYTES
        {
            return Err(CandidateExactShortPreflightReceiptError::InvalidReceiptSize);
        }
        if contents.contains('\r') || !contents.ends_with('\n') {
            return Err(CandidateExactShortPreflightReceiptError::InvalidStructure);
        }
        let lines = contents.split('\n').collect::<Vec<_>>();
        if lines.len() != 10 || !lines[9].is_empty() {
            return Err(CandidateExactShortPreflightReceiptError::InvalidStructure);
        }
        if exact_short_receipt_field(lines[0], "schema")?
            != CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_SCHEMA_V1
            || exact_short_receipt_field(lines[7], "page_size")?
                != CANDIDATE_EXACT_SHORT_PREFLIGHT_PAGE_SIZE.to_string()
            || exact_short_receipt_field(lines[8], "host")?
                != CANDIDATE_EXACT_SHORT_PREFLIGHT_HOST_V1
        {
            return Err(CandidateExactShortPreflightReceiptError::UnsupportedProfile);
        }
        let exact_package = exact_short_receipt_field(lines[1], "exact_package")?;
        let exact_sha256 = exact_short_receipt_field(lines[2], "exact_sha256")?;
        let exact_promotions =
            exact_short_receipt_usize(exact_short_receipt_field(lines[3], "exact_promotions")?)?;
        let core_sha256 = exact_short_receipt_field(lines[4], "core_sha256")?;
        let supplemental_sha256 = exact_short_receipt_field(lines[5], "supplemental_sha256")?;
        let supplemental_promotions = exact_short_receipt_usize(exact_short_receipt_field(
            lines[6],
            "supplemental_promotions",
        )?)?;
        let supplemental = match (supplemental_sha256, supplemental_promotions) {
            ("-", 0) => None,
            (sha256, promotions) => Some((sha256, promotions)),
        };
        Self::new(
            exact_package,
            exact_sha256,
            exact_promotions,
            core_sha256,
            supplemental,
        )
    }

    pub fn render(&self) -> String {
        format!(
            "schema={CANDIDATE_EXACT_SHORT_PREFLIGHT_RECEIPT_SCHEMA_V1}\nexact_package={}\nexact_sha256={}\nexact_promotions={}\ncore_sha256={}\nsupplemental_sha256={}\nsupplemental_promotions={}\npage_size={CANDIDATE_EXACT_SHORT_PREFLIGHT_PAGE_SIZE}\nhost={CANDIDATE_EXACT_SHORT_PREFLIGHT_HOST_V1}\n",
            self.exact_package,
            self.exact_sha256,
            self.exact_promotions,
            self.core_sha256,
            self.supplemental_sha256.as_deref().unwrap_or("-"),
            self.supplemental_promotions,
        )
    }

    pub fn exact_package(&self) -> &str {
        &self.exact_package
    }

    pub fn exact_sha256(&self) -> &str {
        &self.exact_sha256
    }

    pub fn exact_promotions(&self) -> usize {
        self.exact_promotions
    }

    pub fn matches_runtime(&self, core_sha256: &str, supplemental: Option<(&str, usize)>) -> bool {
        self.core_sha256 == core_sha256
            && match (self.supplemental_sha256.as_deref(), supplemental) {
                (None, None) => true,
                (Some(expected), Some((actual, promotions))) => {
                    expected == actual && self.supplemental_promotions == promotions
                }
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateExactShortPreflightReceiptError {
    InvalidReceiptSize,
    InvalidStructure,
    InvalidField,
    UnsupportedProfile,
    InvalidPackageId,
    InvalidSha256,
    InvalidPromotionLimit,
}

impl fmt::Display for CandidateExactShortPreflightReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidReceiptSize => "精确短词专项预检凭据大小无效",
            Self::InvalidStructure => "精确短词专项预检凭据结构无效",
            Self::InvalidField => "精确短词专项预检凭据字段无效",
            Self::UnsupportedProfile => "精确短词专项预检配置不受支持",
            Self::InvalidPackageId => "精确短词专项预检包标识无效",
            Self::InvalidSha256 => "精确短词专项预检摘要无效",
            Self::InvalidPromotionLimit => "精确短词专项预检影响上限无效",
        })
    }
}

impl Error for CandidateExactShortPreflightReceiptError {}

fn exact_short_receipt_field<'a>(
    line: &'a str,
    expected_key: &str,
) -> Result<&'a str, CandidateExactShortPreflightReceiptError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateExactShortPreflightReceiptError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateExactShortPreflightReceiptError::InvalidField);
    }
    Ok(value)
}

fn exact_short_receipt_usize(
    value: &str,
) -> Result<usize, CandidateExactShortPreflightReceiptError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CandidateExactShortPreflightReceiptError::InvalidField)?;
    if parsed.to_string() != value {
        return Err(CandidateExactShortPreflightReceiptError::InvalidField);
    }
    Ok(parsed)
}

fn exact_short_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn exact_short_field<'a>(
    line: &'a str,
    expected_key: &str,
) -> Result<&'a str, CandidateExactShortStateError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(CandidateExactShortStateError::InvalidField);
    };
    if key != expected_key || value.is_empty() || value.contains('=') {
        return Err(CandidateExactShortStateError::InvalidField);
    }
    Ok(value)
}

fn exact_short_canonical_usize(value: &str) -> Result<usize, CandidateExactShortStateError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| CandidateExactShortStateError::InvalidField)?;
    if parsed.to_string() != value {
        return Err(CandidateExactShortStateError::InvalidField);
    }
    Ok(parsed)
}

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

    #[test]
    fn exact_short_disabled_and_enabled_states_round_trip_canonically() {
        let disabled = CandidateExactShortState::default();
        assert_eq!(
            disabled.render(),
            "schema=ziranma-candidate-exact-short-v1\n\
             enabled=false\n\
             package=-\n\
             exact_promotions=0\n"
        );
        assert_eq!(
            CandidateExactShortState::parse(&disabled.render()).unwrap(),
            disabled
        );

        let enabled = CandidateExactShortState::enabled(PACKAGE, 2).unwrap();
        assert!(enabled.is_enabled());
        assert_eq!(enabled.package(), Some(PACKAGE));
        assert_eq!(enabled.exact_promotions(), 2);
        assert_eq!(
            CandidateExactShortState::parse(&enabled.render()).unwrap(),
            enabled
        );
    }

    #[test]
    fn exact_short_parser_rejects_cross_layer_and_noncanonical_states() {
        let valid = CandidateExactShortState::enabled(PACKAGE, 2)
            .unwrap()
            .render();
        assert_eq!(
            CandidateExactShortState::parse(&valid.replace(
                "ziranma-candidate-exact-short-v1",
                "ziranma-candidate-supplemental-v1"
            ))
            .unwrap_err(),
            CandidateExactShortStateError::UnsupportedSchema
        );
        assert_eq!(
            CandidateExactShortState::parse(
                &valid.replace("exact_promotions=2", "exact_promotions=02")
            )
            .unwrap_err(),
            CandidateExactShortStateError::InvalidField
        );
        assert_eq!(
            CandidateExactShortState::parse(
                &valid.replace("exact_promotions=2", "exact_promotions=9")
            )
            .unwrap_err(),
            CandidateExactShortStateError::InvalidPromotionLimit
        );
        assert!(CandidateExactShortState::parse(&valid.replace('\n', "\r\n")).is_err());
    }

    #[test]
    fn exact_phrase_state_is_canonical_and_separate_from_other_layers() {
        let disabled = CandidateExactPhraseState::default();
        assert_eq!(
            disabled.render(),
            "schema=ziranma-candidate-exact-phrase-v1\n\
             enabled=false\n\
             package=-\n"
        );
        assert_eq!(
            CandidateExactPhraseState::parse(&disabled.render()).unwrap(),
            disabled
        );

        let enabled = CandidateExactPhraseState::enabled(PACKAGE).unwrap();
        assert!(enabled.is_enabled());
        assert_eq!(enabled.package(), Some(PACKAGE));
        assert_eq!(
            CandidateExactPhraseState::parse(&enabled.render()).unwrap(),
            enabled
        );
        assert!(
            CandidateExactPhraseState::parse(
                &enabled
                    .render()
                    .replace("exact-phrase-v1", "exact-short-v1")
            )
            .is_err()
        );
        assert!(
            CandidateExactPhraseState::parse(
                &enabled.render().replace("enabled=true", "enabled=yes")
            )
            .is_err()
        );
        assert!(CandidateExactPhraseState::parse(&enabled.render().replace('\n', "\r\n")).is_err());
    }

    #[test]
    fn exact_phrase_receipt_binds_all_three_runtime_packages_and_host() {
        let receipt = CandidateExactPhrasePreflightReceipt::new(
            PACKAGE,
            &"1".repeat(64),
            &"2".repeat(64),
            &"3".repeat(64),
            1,
        )
        .unwrap();
        let rendered = receipt.render();
        assert_eq!(
            CandidateExactPhrasePreflightReceipt::parse(&rendered).unwrap(),
            receipt
        );
        assert!(receipt.matches_runtime(&"2".repeat(64), &"3".repeat(64), 1));
        assert!(!receipt.matches_runtime(&"4".repeat(64), &"3".repeat(64), 1));
        assert!(!receipt.matches_runtime(&"2".repeat(64), &"4".repeat(64), 1));
        assert!(!receipt.matches_runtime(&"2".repeat(64), &"3".repeat(64), 2));
        assert!(
            CandidateExactPhrasePreflightReceipt::parse(
                &rendered.replace("page_size=6", "page_size=7")
            )
            .is_err()
        );
        assert!(
            CandidateExactPhrasePreflightReceipt::parse(&rendered.replace(
                CANDIDATE_EXACT_PHRASE_PREFLIGHT_HOST_V1,
                "pure-candidate-preview-v1"
            ))
            .is_err()
        );
    }

    #[test]
    fn exact_short_preflight_receipt_binds_every_runtime_layer_and_cap() {
        let receipt = CandidateExactShortPreflightReceipt::new(
            PACKAGE,
            &"1".repeat(64),
            2,
            &"2".repeat(64),
            Some((&"3".repeat(64), 1)),
        )
        .unwrap();
        let rendered = receipt.render();
        assert_eq!(
            CandidateExactShortPreflightReceipt::parse(&rendered).unwrap(),
            receipt
        );
        assert!(receipt.matches_runtime(&"2".repeat(64), Some((&"3".repeat(64), 1))));
        assert!(!receipt.matches_runtime(&"4".repeat(64), Some((&"3".repeat(64), 1))));
        assert!(!receipt.matches_runtime(&"2".repeat(64), Some((&"3".repeat(64), 2))));
        assert!(!receipt.matches_runtime(&"2".repeat(64), None));
        assert!(
            CandidateExactShortPreflightReceipt::parse(
                &rendered.replace("page_size=6", "page_size=7")
            )
            .is_err()
        );
        assert!(
            CandidateExactShortPreflightReceipt::parse(
                &rendered.replace("exact_promotions=2", "exact_promotions=0")
            )
            .is_err()
        );

        let no_supplement = CandidateExactShortPreflightReceipt::new(
            PACKAGE,
            &"1".repeat(64),
            1,
            &"2".repeat(64),
            None,
        )
        .unwrap();
        assert!(no_supplement.matches_runtime(&"2".repeat(64), None));
        assert_eq!(
            CandidateExactShortPreflightReceipt::parse(&no_supplement.render()).unwrap(),
            no_supplement
        );
    }
}
