//! Strict repository discovery for independently versioned user tools.
//!
//! Managed tools may run directly from Cargo's `target/{debug,release}` or
//! from one immutable `.local/tsf-alpha/user-tools/builds/<sha256>` bundle.
//! No current-directory, environment-variable, or neighboring-file search is
//! used to infer the private user-data root.

use std::path::{Path, PathBuf};

/// Derives the repository root from one explicitly supported executable layout.
pub fn repository_root_for_user_tool_executable(
    executable: &Path,
    expected_stem: &str,
) -> Option<PathBuf> {
    if expected_stem.is_empty()
        || !expected_stem.bytes().all(|byte| byte.is_ascii_lowercase())
        || executable.file_stem()?.to_str()? != expected_stem
    {
        return None;
    }

    let binary_directory = executable.parent()?;
    let parent = binary_directory.parent()?;
    if matches!(binary_directory.file_name()?.to_str()?, "debug" | "release")
        && parent.file_name()?.to_str()? == "target"
    {
        return parent.parent().map(Path::to_path_buf);
    }

    let digest = binary_directory.file_name()?.to_str()?;
    let user_tools = parent.parent()?;
    let tsf_alpha = user_tools.parent()?;
    let local = tsf_alpha.parent()?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || parent.file_name()?.to_str()? != "builds"
        || user_tools.file_name()?.to_str()? != "user-tools"
        || tsf_alpha.file_name()?.to_str()? != "tsf-alpha"
        || local.file_name()?.to_str()? != ".local"
    {
        return None;
    }
    local.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_cargo_and_immutable_user_tool_layouts() {
        assert_eq!(
            repository_root_for_user_tool_executable(
                Path::new(r"X:\repo\target\release\wishpad.exe"),
                "wishpad"
            ),
            Some(PathBuf::from(r"X:\repo"))
        );
        let digest = "a".repeat(64);
        let bundled = PathBuf::from(r"X:\repo\.local\tsf-alpha\user-tools\builds")
            .join(&digest)
            .join("wishpad.exe");
        assert_eq!(
            repository_root_for_user_tool_executable(&bundled, "wishpad"),
            Some(PathBuf::from(r"X:\repo"))
        );

        assert!(
            repository_root_for_user_tool_executable(&bundled, "aliaspad").is_none(),
            "the expected executable identity is part of the boundary"
        );
        assert!(
            repository_root_for_user_tool_executable(
                &PathBuf::from(r"X:\repo\.local\tsf-alpha\user-tools\builds")
                    .join("A".repeat(64))
                    .join("wishpad.exe"),
                "wishpad"
            )
            .is_none(),
            "published bundle ids are canonical lowercase SHA-256"
        );
        assert!(
            repository_root_for_user_tool_executable(Path::new(r"X:\tools\wishpad.exe"), "wishpad")
                .is_none()
        );
    }
}
