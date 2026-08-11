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
        || !expected_stem
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
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

/// Derives the repository root for the fixed desktop-launcher installation.
///
/// Development and immutable user-tool bundle layouts remain accepted so the
/// same binary can be exercised before it is copied to the stable location.
pub fn repository_root_for_desktop_launcher_executable(executable: &Path) -> Option<PathBuf> {
    if let Some(repository) =
        repository_root_for_user_tool_executable(executable, "ziranma-launcher")
    {
        return Some(repository);
    }
    if executable.file_name()?.to_str()? != "ziranma-launcher.exe" {
        return None;
    }
    let desktop_launcher = executable.parent()?;
    let tsf_alpha = desktop_launcher.parent()?;
    let local = tsf_alpha.parent()?;
    if desktop_launcher.file_name()?.to_str()? != "desktop-launcher"
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
        let repository = PathBuf::from("test-root").join("repo");
        assert_eq!(
            repository_root_for_user_tool_executable(
                &repository
                    .join("target")
                    .join("release")
                    .join("wishpad.exe"),
                "wishpad"
            ),
            Some(repository.clone())
        );
        let digest = "a".repeat(64);
        let bundled = repository
            .join(".local")
            .join("tsf-alpha")
            .join("user-tools")
            .join("builds")
            .join(&digest)
            .join("wishpad.exe");
        assert_eq!(
            repository_root_for_user_tool_executable(&bundled, "wishpad"),
            Some(repository.clone())
        );

        assert!(
            repository_root_for_user_tool_executable(&bundled, "aliaspad").is_none(),
            "the expected executable identity is part of the boundary"
        );
        assert!(
            repository_root_for_user_tool_executable(
                &repository
                    .join(".local")
                    .join("tsf-alpha")
                    .join("user-tools")
                    .join("builds")
                    .join("A".repeat(64))
                    .join("wishpad.exe"),
                "wishpad"
            )
            .is_none(),
            "published bundle ids are canonical lowercase SHA-256"
        );
        assert!(
            repository_root_for_user_tool_executable(
                &PathBuf::from("test-root").join("tools").join("wishpad.exe"),
                "wishpad"
            )
            .is_none()
        );
        assert_eq!(
            repository_root_for_user_tool_executable(
                &repository
                    .join("target")
                    .join("release")
                    .join("ziranma-launcher.exe"),
                "ziranma-launcher"
            ),
            Some(repository)
        );
    }

    #[test]
    fn desktop_launcher_recognizes_only_its_fixed_or_build_layouts() {
        let repository = PathBuf::from("test-root").join("repo");
        assert_eq!(
            repository_root_for_desktop_launcher_executable(
                &repository
                    .join(".local")
                    .join("tsf-alpha")
                    .join("desktop-launcher")
                    .join("ziranma-launcher.exe")
            ),
            Some(repository.clone())
        );
        assert_eq!(
            repository_root_for_desktop_launcher_executable(
                &repository
                    .join("target")
                    .join("release")
                    .join("ziranma-launcher.exe")
            ),
            Some(repository.clone())
        );
        assert!(
            repository_root_for_desktop_launcher_executable(
                &repository
                    .join(".local")
                    .join("desktop-launcher")
                    .join("ziranma-launcher.exe")
            )
            .is_none()
        );
        assert!(
            repository_root_for_desktop_launcher_executable(
                &repository
                    .join(".local")
                    .join("tsf-alpha")
                    .join("desktop-launcher")
                    .join("other.exe")
            )
            .is_none()
        );
    }
}
