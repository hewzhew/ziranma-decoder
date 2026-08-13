//! Strict runtime resolution for the independently refreshed user tools.
//!
//! The desktop launcher follows one fixed slot file and verifies the complete
//! immutable bundle before starting a GUI tool. It never searches the current
//! directory, `PATH`, or neighboring executables.

use std::error::Error;
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const SLOT_SCHEMA: &str = "ziranma-user-tools-slots-v1";
const LEGACY_BUNDLE_SCHEMA: &str = "ziranma-user-tools-bundle-v1";
const BUNDLE_SCHEMA: &str = "ziranma-user-tools-bundle-v2";
const MAX_SLOT_BYTES: u64 = 512;
const MAX_MANIFEST_BYTES: u64 = 4_096;

const LEGACY_TOOL_NAMES: [&str; 7] = [
    "aliasctl",
    "aliaspad",
    "candidatectl",
    "personalctl",
    "researchctl",
    "wishctl",
    "wishpad",
];

/// Exact executable set stored in newly published user-tool bundles.
pub const MANAGED_USER_TOOL_NAMES: [&str; 8] = [
    "aliasctl",
    "aliaspad",
    "candidatectl",
    "personalctl",
    "researchctl",
    "wishctl",
    "wishpad",
    "ziranma-launcher",
];

/// GUI tools that the native desktop launcher is allowed to start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchableUserTool {
    AliasPad,
    WishPad,
}

impl LaunchableUserTool {
    fn executable_name(self) -> &'static str {
        match self {
            Self::AliasPad => "aliaspad.exe",
            Self::WishPad => "wishpad.exe",
        }
    }
}

/// Fail-closed errors produced while resolving one current user tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserToolRuntimeError {
    LayoutUnavailable,
    ReparsePoint,
    SlotUnavailable,
    InvalidSlotState,
    BundleUnavailable,
    ManifestUnavailable,
    InvalidManifest,
    BundleIdentifierMismatch,
    ToolUnavailable,
    ToolIdentifierMismatch,
    UnexpectedBundleEntry,
}

impl fmt::Display for UserToolRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::LayoutUnavailable => "用户工具目录不可用",
            Self::ReparsePoint => "用户工具路径不能是重解析点",
            Self::SlotUnavailable => "用户工具版本槽不可用，请先运行 refresh-ime.cmd refresh",
            Self::InvalidSlotState => "用户工具版本槽格式无效，请运行 refresh-ime.cmd status",
            Self::BundleUnavailable => "当前用户工具包不可用",
            Self::ManifestUnavailable => "当前用户工具清单不可用",
            Self::InvalidManifest => "当前用户工具清单格式无效",
            Self::BundleIdentifierMismatch => "当前用户工具包与清单标识不符",
            Self::ToolUnavailable => "当前用户工具不完整",
            Self::ToolIdentifierMismatch => "当前用户工具与清单摘要不符",
            Self::UnexpectedBundleEntry => "当前用户工具包含有意外文件",
        };
        formatter.write_str(message)
    }
}

impl Error for UserToolRuntimeError {}

/// Resolves one launcher-approved GUI tool from the verified current bundle.
pub fn resolve_current_user_tool(
    repository: &Path,
    tool: LaunchableUserTool,
) -> Result<PathBuf, UserToolRuntimeError> {
    ensure_normal_directory(repository, UserToolRuntimeError::LayoutUnavailable)?;
    let local = repository.join(".local");
    let tsf_alpha = local.join("tsf-alpha");
    let user_tools = tsf_alpha.join("user-tools");
    let builds = user_tools.join("builds");
    for directory in [&local, &tsf_alpha, &user_tools, &builds] {
        ensure_normal_directory(directory, UserToolRuntimeError::LayoutUnavailable)?;
    }

    let slot_path = user_tools.join("slots.zut");
    ensure_normal_file(&slot_path, UserToolRuntimeError::SlotUnavailable)?;
    let slot_bytes = read_bounded(&slot_path, MAX_SLOT_BYTES)
        .map_err(|_| UserToolRuntimeError::InvalidSlotState)?;
    let current = parse_current_slot(&slot_bytes)?;

    let bundle = builds.join(current);
    ensure_normal_directory(&bundle, UserToolRuntimeError::BundleUnavailable)?;
    let manifest_path = bundle.join("manifest.zut");
    ensure_normal_file(&manifest_path, UserToolRuntimeError::ManifestUnavailable)?;
    let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES)
        .map_err(|_| UserToolRuntimeError::InvalidManifest)?;
    if sha256_hex(&manifest_bytes) != current {
        return Err(UserToolRuntimeError::BundleIdentifierMismatch);
    }
    let manifest = parse_manifest(&manifest_bytes)?;
    verify_bundle(&bundle, &manifest)?;

    let executable = bundle.join(tool.executable_name());
    if !manifest
        .iter()
        .any(|(name, _)| name == tool.executable_name())
    {
        return Err(UserToolRuntimeError::ToolUnavailable);
    }
    Ok(executable)
}

fn parse_current_slot(bytes: &[u8]) -> Result<&str, UserToolRuntimeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| UserToolRuntimeError::InvalidSlotState)?;
    let lines = text.split("\r\n").collect::<Vec<_>>();
    if lines.len() != 4
        || !lines[3].is_empty()
        || lines[0] != format!("schema={SLOT_SCHEMA}")
        || !lines[1].starts_with("current=")
        || !lines[2].starts_with("previous=")
    {
        return Err(UserToolRuntimeError::InvalidSlotState);
    }
    let current = &lines[1]["current=".len()..];
    let previous = &lines[2]["previous=".len()..];
    if !is_digest(current) || (previous != "-" && !is_digest(previous)) || previous == current {
        return Err(UserToolRuntimeError::InvalidSlotState);
    }
    Ok(current)
}

fn parse_manifest(bytes: &[u8]) -> Result<Vec<(String, String)>, UserToolRuntimeError> {
    let text = std::str::from_utf8(bytes).map_err(|_| UserToolRuntimeError::InvalidManifest)?;
    if text.contains('\r') || !text.ends_with('\n') {
        return Err(UserToolRuntimeError::InvalidManifest);
    }
    let lines = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect::<Vec<_>>();
    let names: &[&str] = match lines.first().copied() {
        Some(line) if line == format!("schema={BUNDLE_SCHEMA}") => &MANAGED_USER_TOOL_NAMES,
        Some(line) if line == format!("schema={LEGACY_BUNDLE_SCHEMA}") => &LEGACY_TOOL_NAMES,
        _ => return Err(UserToolRuntimeError::InvalidManifest),
    };
    if lines.len() != names.len() + 1 {
        return Err(UserToolRuntimeError::InvalidManifest);
    }
    let mut manifest = Vec::with_capacity(names.len());
    for (line, name) in lines[1..].iter().zip(names) {
        let executable = format!("{name}.exe");
        let prefix = format!("tool.{executable}=");
        let Some(digest) = line.strip_prefix(&prefix) else {
            return Err(UserToolRuntimeError::InvalidManifest);
        };
        if !is_digest(digest) {
            return Err(UserToolRuntimeError::InvalidManifest);
        }
        manifest.push((executable, digest.to_owned()));
    }
    Ok(manifest)
}

fn verify_bundle(bundle: &Path, manifest: &[(String, String)]) -> Result<(), UserToolRuntimeError> {
    for (name, expected) in manifest {
        let path = bundle.join(name);
        ensure_normal_file(&path, UserToolRuntimeError::ToolUnavailable)?;
        if sha256_file(&path)? != *expected {
            return Err(UserToolRuntimeError::ToolIdentifierMismatch);
        }
    }

    let mut expected = manifest
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    expected.push("manifest.zut".to_owned());
    expected.sort_unstable();
    let mut actual = fs::read_dir(bundle)
        .map_err(|_| UserToolRuntimeError::BundleUnavailable)?
        .map(|entry| {
            let entry = entry.map_err(|_| UserToolRuntimeError::UnexpectedBundleEntry)?;
            entry
                .file_name()
                .into_string()
                .map_err(|_| UserToolRuntimeError::UnexpectedBundleEntry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    actual.sort_unstable();
    if actual != expected {
        return Err(UserToolRuntimeError::UnexpectedBundleEntry);
    }
    Ok(())
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    if metadata.len() == 0 || metadata.len() > maximum {
        return Err(());
    }
    fs::read(path).map_err(|_| ())
}

fn ensure_normal_file(
    path: &Path,
    missing: UserToolRuntimeError,
) -> Result<(), UserToolRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| missing)?;
    if is_reparse_point(&metadata) {
        return Err(UserToolRuntimeError::ReparsePoint);
    }
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(missing);
    }
    Ok(())
}

fn ensure_normal_directory(
    path: &Path,
    missing: UserToolRuntimeError,
) -> Result<(), UserToolRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| missing)?;
    if is_reparse_point(&metadata) {
        return Err(UserToolRuntimeError::ReparsePoint);
    }
    if !metadata.is_dir() {
        return Err(missing);
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_file(path: &Path) -> Result<String, UserToolRuntimeError> {
    let mut file = File::open(path).map_err(|_| UserToolRuntimeError::ToolUnavailable)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| UserToolRuntimeError::ToolUnavailable)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ziranma-user-tool-runtime-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_bundle(repository: &Path, legacy: bool) -> (String, PathBuf) {
        let user_tools = repository.join(".local/tsf-alpha/user-tools");
        let builds = user_tools.join("builds");
        fs::create_dir_all(&builds).unwrap();
        let names: &[&str] = if legacy {
            &LEGACY_TOOL_NAMES
        } else {
            &MANAGED_USER_TOOL_NAMES
        };
        let mut payloads = Vec::new();
        let mut manifest = format!(
            "schema={}\n",
            if legacy {
                LEGACY_BUNDLE_SCHEMA
            } else {
                BUNDLE_SCHEMA
            }
        );
        for name in names {
            let payload = format!("synthetic {name}").into_bytes();
            manifest.push_str(&format!("tool.{name}.exe={}\n", sha256_hex(&payload)));
            payloads.push((*name, payload));
        }
        let bundle_id = sha256_hex(manifest.as_bytes());
        let bundle = builds.join(&bundle_id);
        fs::create_dir(&bundle).unwrap();
        fs::write(bundle.join("manifest.zut"), manifest).unwrap();
        for (name, payload) in payloads {
            fs::write(bundle.join(format!("{name}.exe")), payload).unwrap();
        }
        fs::write(
            user_tools.join("slots.zut"),
            format!("schema={SLOT_SCHEMA}\r\ncurrent={bundle_id}\r\nprevious=-\r\n"),
        )
        .unwrap();
        (bundle_id, bundle)
    }

    #[test]
    fn resolves_launchable_tools_from_current_and_legacy_bundles() {
        for legacy in [false, true] {
            let root = TestDirectory::new();
            let (_, bundle) = write_bundle(&root.0, legacy);
            assert_eq!(
                resolve_current_user_tool(&root.0, LaunchableUserTool::WishPad).unwrap(),
                bundle.join("wishpad.exe")
            );
            assert_eq!(
                resolve_current_user_tool(&root.0, LaunchableUserTool::AliasPad).unwrap(),
                bundle.join("aliaspad.exe")
            );
        }
    }

    #[test]
    fn rejects_noncanonical_slots_modified_tools_and_extra_entries() {
        let root = TestDirectory::new();
        let (_, bundle) = write_bundle(&root.0, false);
        fs::write(bundle.join("wishpad.exe"), b"modified").unwrap();
        assert_eq!(
            resolve_current_user_tool(&root.0, LaunchableUserTool::WishPad),
            Err(UserToolRuntimeError::ToolIdentifierMismatch)
        );

        let root = TestDirectory::new();
        let (_, bundle) = write_bundle(&root.0, false);
        fs::write(bundle.join("extra.exe"), b"unexpected").unwrap();
        assert_eq!(
            resolve_current_user_tool(&root.0, LaunchableUserTool::WishPad),
            Err(UserToolRuntimeError::UnexpectedBundleEntry)
        );

        let root = TestDirectory::new();
        let (bundle_id, _) = write_bundle(&root.0, false);
        fs::write(
            root.0.join(".local/tsf-alpha/user-tools/slots.zut"),
            format!("schema={SLOT_SCHEMA}\ncurrent={bundle_id}\nprevious=-\n"),
        )
        .unwrap();
        assert_eq!(
            resolve_current_user_tool(&root.0, LaunchableUserTool::WishPad),
            Err(UserToolRuntimeError::InvalidSlotState)
        );
    }
}
