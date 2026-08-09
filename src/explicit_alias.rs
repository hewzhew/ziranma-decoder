//! Explicit, user-managed `code -> text` aliases and their encrypted slots.
//!
//! The in-memory snapshot is platform independent and deterministic. The
//! storage loader follows only one explicitly supplied root and delegates
//! encryption to the existing `DataProtector` boundary; it never discovers
//! user files, learns from typing, or connects to a network.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::{DataProtector, candidate_sha256_hex};

/// Plaintext schema protected inside one immutable alias package.
pub const EXPLICIT_ALIAS_SCHEMA_V1: &str = "ziranma-explicit-aliases-v1";
/// Slot-state schema for current, staged, and rollback alias packages.
pub const EXPLICIT_ALIAS_SLOT_SCHEMA_V1: &str = "ziranma-explicit-alias-slots-v1";
/// Fixed filename for alias slot state.
pub const EXPLICIT_ALIAS_SLOT_FILE: &str = "slots.zas";
/// Immutable alias-package store below one explicitly supplied root.
pub const EXPLICIT_ALIAS_PACKAGES_DIRECTORY: &str = "packages";
/// Fixed encrypted payload filename inside one alias package.
pub const EXPLICIT_ALIAS_PACKAGE_FILE: &str = "aliases.zap";
/// Maximum aliases retained in one explicit snapshot.
pub const MAX_EXPLICIT_ALIAS_ENTRIES: usize = 1_024;
/// Maximum plaintext bytes accepted after current-user decryption.
pub const MAX_EXPLICIT_ALIAS_PLAINTEXT_BYTES: usize = 256 * 1024;
/// Maximum encrypted package bytes read from disk.
pub const MAX_EXPLICIT_ALIAS_PACKAGE_BYTES: usize = 512 * 1024;
/// Maximum bytes accepted for the small plaintext slot state.
pub const MAX_EXPLICIT_ALIAS_SLOT_BYTES: usize = 512;

const MAX_EXPLICIT_ALIAS_CODE_BYTES: usize = 64;
const MAX_EXPLICIT_ALIAS_TEXT_BYTES: usize = 256;
const MAX_EXPLICIT_ALIAS_TEXT_CHARS: usize = 64;
const PROTECTED_ALIAS_MAGIC: &[u8] = b"ziranma-alias-dpapi-v1\0";

/// A bounded, exact-match alias map. One code intentionally names one text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplicitAliasSnapshot {
    entries: BTreeMap<String, String>,
}

impl ExplicitAliasSnapshot {
    /// Parses the canonical plaintext stored inside a protected package.
    pub fn parse(input: &[u8]) -> Result<Self, ExplicitAliasError> {
        if input.is_empty() || input.len() > MAX_EXPLICIT_ALIAS_PLAINTEXT_BYTES {
            return Err(ExplicitAliasError::InvalidPlaintextSize);
        }
        let input = std::str::from_utf8(input).map_err(|_| ExplicitAliasError::InvalidUtf8)?;
        if input.contains('\r') || !input.ends_with('\n') {
            return Err(ExplicitAliasError::InvalidPlaintextStructure);
        }
        let (header, payload) = input
            .split_once("\n\n")
            .ok_or(ExplicitAliasError::InvalidPlaintextStructure)?;
        let header_lines = header.split('\n').collect::<Vec<_>>();
        if header_lines.len() != 3 || field(header_lines[0], "schema")? != EXPLICIT_ALIAS_SCHEMA_V1
        {
            return Err(ExplicitAliasError::InvalidPlaintextStructure);
        }
        let expected_entries = parse_canonical_usize(field(header_lines[1], "entry_count")?)?;
        let expected_payload_bytes =
            parse_canonical_usize(field(header_lines[2], "payload_bytes")?)?;
        if expected_entries > MAX_EXPLICIT_ALIAS_ENTRIES {
            return Err(ExplicitAliasError::TooManyEntries);
        }
        if payload.len() != expected_payload_bytes {
            return Err(ExplicitAliasError::PayloadLengthMismatch);
        }

        let mut entries = BTreeMap::new();
        if !payload.is_empty() {
            for line in payload.strip_suffix('\n').unwrap_or(payload).split('\n') {
                let Some((code, text)) = line.split_once('\t') else {
                    return Err(ExplicitAliasError::InvalidEntry);
                };
                validate_alias(code, text)?;
                if entries.insert(code.to_owned(), text.to_owned()).is_some() {
                    return Err(ExplicitAliasError::DuplicateCode);
                }
            }
        }
        if entries.len() != expected_entries {
            return Err(ExplicitAliasError::EntryCountMismatch);
        }
        Ok(Self { entries })
    }

    /// Renders deterministic plaintext before current-user protection.
    pub fn render(&self) -> Result<Vec<u8>, ExplicitAliasError> {
        if self.entries.len() > MAX_EXPLICIT_ALIAS_ENTRIES {
            return Err(ExplicitAliasError::TooManyEntries);
        }
        let mut payload = String::new();
        for (code, text) in &self.entries {
            validate_alias(code, text)?;
            payload.push_str(code);
            payload.push('\t');
            payload.push_str(text);
            payload.push('\n');
        }
        let output = format!(
            "schema={EXPLICIT_ALIAS_SCHEMA_V1}\nentry_count={}\npayload_bytes={}\n\n{payload}",
            self.entries.len(),
            payload.len()
        )
        .into_bytes();
        if output.len() > MAX_EXPLICIT_ALIAS_PLAINTEXT_BYTES {
            return Err(ExplicitAliasError::InvalidPlaintextSize);
        }
        Ok(output)
    }

    /// Returns the exact text for one complete alias code.
    pub fn get(&self, code: &str) -> Option<&str> {
        self.entries.get(code).map(String::as_str)
    }

    /// Returns the number of explicit aliases.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether the snapshot contains no aliases.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates in deterministic code order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(code, text)| (code.as_str(), text.as_str()))
    }

    /// Inserts or replaces one user-confirmed exact alias.
    pub fn set(&mut self, code: &str, text: &str) -> Result<Option<String>, ExplicitAliasError> {
        validate_alias(code, text)?;
        if !self.entries.contains_key(code) && self.entries.len() == MAX_EXPLICIT_ALIAS_ENTRIES {
            return Err(ExplicitAliasError::TooManyEntries);
        }
        Ok(self.entries.insert(code.to_owned(), text.to_owned()))
    }

    /// Removes one exact alias, returning its previous text when present.
    pub fn remove(&mut self, code: &str) -> Result<Option<String>, ExplicitAliasError> {
        validate_alias_code(code)?;
        Ok(self.entries.remove(code))
    }
}

/// current / candidate / previous references to immutable protected packages.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplicitAliasSlotState {
    current: Option<String>,
    candidate: Option<String>,
    previous: Option<String>,
}

impl ExplicitAliasSlotState {
    /// Parses the exact four-line slot state.
    pub fn parse(input: &str) -> Result<Self, ExplicitAliasError> {
        if input.is_empty() || input.len() > MAX_EXPLICIT_ALIAS_SLOT_BYTES {
            return Err(ExplicitAliasError::InvalidSlotState);
        }
        if input.contains('\r') || !input.ends_with('\n') {
            return Err(ExplicitAliasError::InvalidSlotState);
        }
        let lines = input.split('\n').collect::<Vec<_>>();
        if lines.len() != 5
            || !lines[4].is_empty()
            || field(lines[0], "schema")? != EXPLICIT_ALIAS_SLOT_SCHEMA_V1
        {
            return Err(ExplicitAliasError::InvalidSlotState);
        }
        let state = Self {
            current: optional_alias_id(field(lines[1], "current")?)?,
            candidate: optional_alias_id(field(lines[2], "candidate")?)?,
            previous: optional_alias_id(field(lines[3], "previous")?)?,
        };
        state.validate()?;
        Ok(state)
    }

    /// Renders the canonical four-line state.
    pub fn render(&self) -> String {
        format!(
            "schema={EXPLICIT_ALIAS_SLOT_SCHEMA_V1}\ncurrent={}\ncandidate={}\nprevious={}\n",
            self.current.as_deref().unwrap_or("-"),
            self.candidate.as_deref().unwrap_or("-"),
            self.previous.as_deref().unwrap_or("-")
        )
    }

    /// Adopts the first active package.
    pub fn adopt(&mut self, package_id: &str) -> Result<(), ExplicitAliasError> {
        validate_alias_package_id(package_id)?;
        if self.current.is_some() || self.candidate.is_some() || self.previous.is_some() {
            return Err(ExplicitAliasError::AlreadyConfigured);
        }
        self.current = Some(package_id.to_owned());
        Ok(())
    }

    /// Stages an independently written and validated package.
    pub fn stage(&mut self, package_id: &str) -> Result<(), ExplicitAliasError> {
        validate_alias_package_id(package_id)?;
        if self.current.is_none() {
            return Err(ExplicitAliasError::NotConfigured);
        }
        if self.current.as_deref() == Some(package_id)
            || self.previous.as_deref() == Some(package_id)
        {
            return Err(ExplicitAliasError::DuplicatePackage);
        }
        self.candidate = Some(package_id.to_owned());
        Ok(())
    }

    /// Promotes the staged package while retaining the old current package.
    pub fn promote(&mut self) -> Result<(), ExplicitAliasError> {
        let next = self
            .candidate
            .as_ref()
            .cloned()
            .ok_or(ExplicitAliasError::CandidateEmpty)?;
        let old = self
            .current
            .as_ref()
            .cloned()
            .ok_or(ExplicitAliasError::CurrentEmpty)?;
        self.current = Some(next);
        self.candidate = None;
        self.previous = Some(old);
        Ok(())
    }

    /// Swaps current and previous without deleting either package.
    pub fn rollback(&mut self) -> Result<(), ExplicitAliasError> {
        let current = self
            .current
            .as_ref()
            .cloned()
            .ok_or(ExplicitAliasError::CurrentEmpty)?;
        let previous = self
            .previous
            .as_ref()
            .cloned()
            .ok_or(ExplicitAliasError::PreviousEmpty)?;
        self.previous = Some(current);
        self.current = Some(previous);
        Ok(())
    }

    /// Drops only the staged reference; immutable package files remain.
    pub fn unstage(&mut self) -> Result<(), ExplicitAliasError> {
        self.candidate
            .take()
            .map(|_| ())
            .ok_or(ExplicitAliasError::CandidateEmpty)
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn candidate(&self) -> Option<&str> {
        self.candidate.as_deref()
    }

    pub fn previous(&self) -> Option<&str> {
        self.previous.as_deref()
    }

    fn validate(&self) -> Result<(), ExplicitAliasError> {
        if self.current.is_none() && (self.candidate.is_some() || self.previous.is_some()) {
            return Err(ExplicitAliasError::InvalidSlotState);
        }
        let occupied = [
            self.current.as_deref(),
            self.candidate.as_deref(),
            self.previous.as_deref(),
        ];
        for (index, package_id) in occupied.iter().enumerate() {
            if let Some(package_id) = package_id {
                validate_alias_package_id(package_id)?;
                if occupied[index + 1..].contains(&Some(*package_id)) {
                    return Err(ExplicitAliasError::DuplicatePackage);
                }
            }
        }
        Ok(())
    }
}

/// A validated protected alias package read from an explicit root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedExplicitAliasSnapshot {
    package_id: String,
    snapshot: Arc<ExplicitAliasSnapshot>,
}

impl LoadedExplicitAliasSnapshot {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn snapshot(&self) -> &Arc<ExplicitAliasSnapshot> {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> Arc<ExplicitAliasSnapshot> {
        self.snapshot
    }
}

/// Seals one deterministic snapshot into a current-user protected package.
pub fn protect_explicit_alias_snapshot(
    snapshot: &ExplicitAliasSnapshot,
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, ExplicitAliasError> {
    let mut plaintext = snapshot.render()?;
    let protected = protector.protect(&plaintext);
    plaintext.fill(0);
    let protected = protected.map_err(|_| ExplicitAliasError::Protection)?;
    if protected.is_empty() || protected.len() > MAX_EXPLICIT_ALIAS_PACKAGE_BYTES {
        return Err(ExplicitAliasError::InvalidProtectedPackage);
    }
    let protected_len =
        u32::try_from(protected.len()).map_err(|_| ExplicitAliasError::InvalidProtectedPackage)?;
    let mut output = Vec::with_capacity(PROTECTED_ALIAS_MAGIC.len() + 4 + protected.len());
    output.extend_from_slice(PROTECTED_ALIAS_MAGIC);
    output.extend_from_slice(&protected_len.to_le_bytes());
    output.extend_from_slice(&protected);
    if output.len() > MAX_EXPLICIT_ALIAS_PACKAGE_BYTES {
        return Err(ExplicitAliasError::InvalidProtectedPackage);
    }
    Ok(output)
}

/// Opens and validates one current-user protected alias package.
pub fn unprotect_explicit_alias_snapshot(
    package: &[u8],
    protector: &dyn DataProtector,
) -> Result<ExplicitAliasSnapshot, ExplicitAliasError> {
    if package.len() <= PROTECTED_ALIAS_MAGIC.len() + 4
        || package.len() > MAX_EXPLICIT_ALIAS_PACKAGE_BYTES
        || !package.starts_with(PROTECTED_ALIAS_MAGIC)
    {
        return Err(ExplicitAliasError::InvalidProtectedPackage);
    }
    let length_start = PROTECTED_ALIAS_MAGIC.len();
    let protected_len = u32::from_le_bytes(
        package[length_start..length_start + 4]
            .try_into()
            .expect("four-byte protected alias length"),
    ) as usize;
    let protected = &package[length_start + 4..];
    if protected.is_empty() || protected.len() != protected_len {
        return Err(ExplicitAliasError::InvalidProtectedPackage);
    }
    let mut plaintext = protector
        .unprotect(protected)
        .map_err(|_| ExplicitAliasError::Protection)?;
    let parsed = ExplicitAliasSnapshot::parse(&plaintext);
    plaintext.fill(0);
    parsed
}

/// Computes the immutable content identifier of exact protected bytes.
pub fn explicit_alias_package_id(package: &[u8]) -> String {
    format!("alias-{}", candidate_sha256_hex(package))
}

/// Reads only the fixed alias slot state from an explicit root.
///
/// `Ok(None)` means the root does not exist. A present but malformed root is
/// reported instead of being confused with an empty configuration.
pub fn load_explicit_alias_slot_state(
    root: &Path,
) -> Result<Option<ExplicitAliasSlotState>, ExplicitAliasError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ExplicitAliasError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ExplicitAliasError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let state = read_regular_bytes(
        &root.join(EXPLICIT_ALIAS_SLOT_FILE),
        MAX_EXPLICIT_ALIAS_SLOT_BYTES,
        ExplicitAliasError::SlotUnavailable,
    )?;
    let state = String::from_utf8(state).map_err(|_| ExplicitAliasError::InvalidSlotState)?;
    ExplicitAliasSlotState::parse(&state).map(Some)
}

/// Loads the active immutable package named by one explicit alias root.
pub fn load_current_explicit_alias_snapshot(
    root: &Path,
    protector: &dyn DataProtector,
) -> Result<Option<LoadedExplicitAliasSnapshot>, ExplicitAliasError> {
    let Some(state) = load_explicit_alias_slot_state(root)? else {
        return Ok(None);
    };
    let Some(package_id) = state.current() else {
        return Ok(None);
    };
    load_explicit_alias_package(root, package_id, protector).map(Some)
}

/// Loads one explicitly named immutable alias package below a validated root.
pub fn load_explicit_alias_package(
    root: &Path,
    package_id: &str,
    protector: &dyn DataProtector,
) -> Result<LoadedExplicitAliasSnapshot, ExplicitAliasError> {
    validate_alias_package_id(package_id)?;
    ensure_regular_directory(root, ExplicitAliasError::InvalidRoot)?;
    let packages = root.join(EXPLICIT_ALIAS_PACKAGES_DIRECTORY);
    ensure_regular_directory(&packages, ExplicitAliasError::InvalidPackageStore)?;
    let package_directory = packages.join(package_id);
    ensure_regular_directory(
        &package_directory,
        ExplicitAliasError::InvalidPackageDirectory,
    )?;
    let package = read_regular_bytes(
        &package_directory.join(EXPLICIT_ALIAS_PACKAGE_FILE),
        MAX_EXPLICIT_ALIAS_PACKAGE_BYTES,
        ExplicitAliasError::PackageUnavailable,
    )?;
    if explicit_alias_package_id(&package) != package_id {
        return Err(ExplicitAliasError::PackageIdentifierMismatch);
    }
    let snapshot = Arc::new(unprotect_explicit_alias_snapshot(&package, protector)?);
    Ok(LoadedExplicitAliasSnapshot {
        package_id: package_id.to_owned(),
        snapshot,
    })
}

/// Strict failures for explicit aliases and their protected storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplicitAliasError {
    InvalidPlaintextSize,
    InvalidUtf8,
    InvalidPlaintextStructure,
    InvalidField,
    InvalidNumber,
    PayloadLengthMismatch,
    EntryCountMismatch,
    TooManyEntries,
    InvalidEntry,
    InvalidCode,
    InvalidText,
    DuplicateCode,
    InvalidProtectedPackage,
    Protection,
    InvalidSlotState,
    InvalidPackageId,
    InvalidCombination,
    DuplicatePackage,
    AlreadyConfigured,
    NotConfigured,
    CurrentEmpty,
    CandidateEmpty,
    PreviousEmpty,
    RootUnavailable,
    InvalidRoot,
    SlotUnavailable,
    InvalidPackageStore,
    InvalidPackageDirectory,
    PackageUnavailable,
    PackageIdentifierMismatch,
}

impl fmt::Display for ExplicitAliasError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPlaintextSize => "别名数据大小无效",
            Self::InvalidUtf8 => "别名数据不是有效 UTF-8",
            Self::InvalidPlaintextStructure => "别名数据结构无效",
            Self::InvalidField => "别名数据字段无效",
            Self::InvalidNumber => "别名数据数字无效",
            Self::PayloadLengthMismatch => "别名载荷长度不符",
            Self::EntryCountMismatch => "别名条目数不符",
            Self::TooManyEntries => "别名条目超过上限",
            Self::InvalidEntry => "别名条目格式无效",
            Self::InvalidCode => "别名编码必须是 1 至 64 个小写字母",
            Self::InvalidText => "别名文字为空、过长或含控制字符",
            Self::DuplicateCode => "同一别名编码出现多次",
            Self::InvalidProtectedPackage => "加密别名包结构无效",
            Self::Protection => "当前用户无法加密或解密别名包",
            Self::InvalidSlotState | Self::InvalidCombination => "别名槽状态无效",
            Self::InvalidPackageId => "别名包标识无效",
            Self::DuplicatePackage => "同一别名包不能占用多个槽位",
            Self::AlreadyConfigured => "别名槽已经配置",
            Self::NotConfigured => "别名槽尚未配置",
            Self::CurrentEmpty => "当前别名槽为空",
            Self::CandidateEmpty => "待切换别名槽为空",
            Self::PreviousEmpty => "可回退别名槽为空",
            Self::RootUnavailable => "无法检查别名数据目录",
            Self::InvalidRoot => "别名数据目录无效",
            Self::SlotUnavailable => "别名槽状态不可用",
            Self::InvalidPackageStore => "别名包存储无效",
            Self::InvalidPackageDirectory => "别名包目录无效",
            Self::PackageUnavailable => "加密别名包不可用",
            Self::PackageIdentifierMismatch => "加密别名包与内容标识不符",
        };
        formatter.write_str(message)
    }
}

impl Error for ExplicitAliasError {}

fn validate_alias(code: &str, text: &str) -> Result<(), ExplicitAliasError> {
    validate_alias_code(code)?;
    if text.is_empty()
        || text.len() > MAX_EXPLICIT_ALIAS_TEXT_BYTES
        || text.chars().count() > MAX_EXPLICIT_ALIAS_TEXT_CHARS
        || text.chars().any(char::is_control)
        || text
            .chars()
            .any(|character| matches!(character, '\t' | '\r' | '\n'))
    {
        return Err(ExplicitAliasError::InvalidText);
    }
    Ok(())
}

fn validate_alias_code(code: &str) -> Result<(), ExplicitAliasError> {
    if code.is_empty()
        || code.len() > MAX_EXPLICIT_ALIAS_CODE_BYTES
        || !code.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(ExplicitAliasError::InvalidCode);
    }
    Ok(())
}

fn field<'a>(line: &'a str, expected: &str) -> Result<&'a str, ExplicitAliasError> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(ExplicitAliasError::InvalidField);
    };
    if key != expected || value.is_empty() || value.contains('=') {
        return Err(ExplicitAliasError::InvalidField);
    }
    Ok(value)
}

fn parse_canonical_usize(value: &str) -> Result<usize, ExplicitAliasError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ExplicitAliasError::InvalidNumber);
    }
    value
        .parse::<usize>()
        .map_err(|_| ExplicitAliasError::InvalidNumber)
}

fn optional_alias_id(value: &str) -> Result<Option<String>, ExplicitAliasError> {
    if value == "-" {
        return Ok(None);
    }
    validate_alias_package_id(value)?;
    Ok(Some(value.to_owned()))
}

fn validate_alias_package_id(value: &str) -> Result<(), ExplicitAliasError> {
    let Some(digest) = value.strip_prefix("alias-") else {
        return Err(ExplicitAliasError::InvalidPackageId);
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ExplicitAliasError::InvalidPackageId);
    }
    Ok(())
}

fn ensure_regular_directory(
    path: &Path,
    error: ExplicitAliasError,
) -> Result<(), ExplicitAliasError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| error.clone())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error);
    }
    Ok(())
}

fn read_regular_bytes(
    path: &Path,
    maximum_bytes: usize,
    error: ExplicitAliasError,
) -> Result<Vec<u8>, ExplicitAliasError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| error.clone())?;
    let maximum_u64 = u64::try_from(maximum_bytes).unwrap_or(u64::MAX);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum_u64
    {
        return Err(error);
    }
    let mut file = File::open(path).map_err(|_| error.clone())?;
    let opened = file.metadata().map_err(|_| error.clone())?;
    if !opened.is_file() || opened.len() == 0 || opened.len() > maximum_u64 {
        return Err(error);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| error.clone())?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(error);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContinuousCaptureError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

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

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ziranma-explicit-alias-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn install(root: &Path, snapshot: &ExplicitAliasSnapshot) -> String {
        let package = protect_explicit_alias_snapshot(snapshot, &TestProtector).unwrap();
        let package_id = explicit_alias_package_id(&package);
        let directory = root
            .join(EXPLICIT_ALIAS_PACKAGES_DIRECTORY)
            .join(&package_id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(EXPLICIT_ALIAS_PACKAGE_FILE), package).unwrap();
        package_id
    }

    #[test]
    fn snapshot_round_trips_in_code_order() {
        let mut snapshot = ExplicitAliasSnapshot::default();
        snapshot.set("vtrayn", "v2rayN").unwrap();
        snapshot.set("wuu", "呜呜").unwrap();
        snapshot.set("wua", "呜哇").unwrap();
        let bytes = snapshot.render().unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("vtrayn\tv2rayN\n"));
        assert!(String::from_utf8_lossy(&bytes).contains("wua\t呜哇\nwuu\t呜呜\n"));
        assert_eq!(ExplicitAliasSnapshot::parse(&bytes).unwrap(), snapshot);
    }

    #[test]
    fn invalid_aliases_and_noncanonical_plaintext_are_rejected() {
        let mut snapshot = ExplicitAliasSnapshot::default();
        assert_eq!(
            snapshot.set("WU", "文字"),
            Err(ExplicitAliasError::InvalidCode)
        );
        assert_eq!(
            snapshot.set("wu", "a\nb"),
            Err(ExplicitAliasError::InvalidText)
        );
        let duplicate = b"schema=ziranma-explicit-aliases-v1\nentry_count=2\npayload_bytes=12\n\na\t\xE7\x94\xB2\na\t\xE4\xB9\x99\n";
        assert_eq!(
            ExplicitAliasSnapshot::parse(duplicate),
            Err(ExplicitAliasError::DuplicateCode)
        );
    }

    #[test]
    fn protected_package_and_content_id_are_bound() {
        let mut snapshot = ExplicitAliasSnapshot::default();
        snapshot.set("abc", "合成别名").unwrap();
        let package = protect_explicit_alias_snapshot(&snapshot, &TestProtector).unwrap();
        assert_eq!(
            unprotect_explicit_alias_snapshot(&package, &TestProtector).unwrap(),
            snapshot
        );
        let mut changed = package.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_ne!(
            explicit_alias_package_id(&changed),
            explicit_alias_package_id(&package)
        );
    }

    #[test]
    fn slots_promote_rollback_and_unstage_without_mutating_on_failure() {
        let a = format!("alias-{}", "1".repeat(64));
        let b = format!("alias-{}", "2".repeat(64));
        let c = format!("alias-{}", "3".repeat(64));
        let mut state = ExplicitAliasSlotState::default();
        state.adopt(&a).unwrap();
        let adopted = state.clone();
        assert_eq!(state.rollback(), Err(ExplicitAliasError::PreviousEmpty));
        assert_eq!(state, adopted);
        state.stage(&b).unwrap();
        state.promote().unwrap();
        assert_eq!(state.current(), Some(b.as_str()));
        assert_eq!(state.previous(), Some(a.as_str()));
        state.rollback().unwrap();
        assert_eq!(state.current(), Some(a.as_str()));
        state.stage(&c).unwrap();
        state.unstage().unwrap();
        let before = state.clone();
        assert_eq!(state.promote(), Err(ExplicitAliasError::CandidateEmpty));
        assert_eq!(state, before);
        assert_eq!(
            ExplicitAliasSlotState::parse(&state.render()).unwrap(),
            state
        );
    }

    #[test]
    fn loader_follows_only_current_and_rejects_modified_bytes() {
        let root = TestDirectory::new();
        let mut first = ExplicitAliasSnapshot::default();
        first.set("aa", "甲").unwrap();
        let first_id = install(&root.0, &first);
        let mut second = ExplicitAliasSnapshot::default();
        second.set("aa", "乙").unwrap();
        let second_id = install(&root.0, &second);
        let mut state = ExplicitAliasSlotState::default();
        state.adopt(&first_id).unwrap();
        state.stage(&second_id).unwrap();
        fs::write(root.0.join(EXPLICIT_ALIAS_SLOT_FILE), state.render()).unwrap();

        let loaded = load_current_explicit_alias_snapshot(&root.0, &TestProtector)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.package_id(), first_id);
        assert_eq!(loaded.snapshot().get("aa"), Some("甲"));

        let path = root
            .0
            .join(EXPLICIT_ALIAS_PACKAGES_DIRECTORY)
            .join(first_id)
            .join(EXPLICIT_ALIAS_PACKAGE_FILE);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert_eq!(
            load_current_explicit_alias_snapshot(&root.0, &TestProtector),
            Err(ExplicitAliasError::PackageIdentifierMismatch)
        );
    }
}
