//! Explicit consent state for the local continuous IME research inbox.
//!
//! The marker contains no private text. Its only purpose is to make automatic
//! current-user-protected journaling opt-in and independently reversible.

use std::error::Error;
use std::fmt;
#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const RESEARCH_FEEDBACK_DIRECTORY: &str = "research-inbox";
pub const RESEARCH_FEEDBACK_CONSENT_FILE: &str = "continuous-capture.enabled-v1";
pub const RESEARCH_FEEDBACK_CONSENT_SCHEMA_V1: &str = "ziranma-research-feedback-consent-v1";

const CONSENT_BYTES: &[u8] = b"schema=ziranma-research-feedback-consent-v1\n\
enabled=true\n\
contains_private_text=true\n\
scope=eligible-tsf-semantic-events\n\
encryption=windows-dpapi-current-user\n\
network=false\n";
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchFeedbackError {
    InvalidRoot,
    InvalidConsent,
    Io,
}

impl fmt::Display for ResearchFeedbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "research feedback root is not a regular directory",
            Self::InvalidConsent => "research feedback consent marker is invalid",
            Self::Io => "research feedback consent operation failed",
        })
    }
}

impl Error for ResearchFeedbackError {}

pub fn research_feedback_enabled(root: &Path) -> Result<bool, ResearchFeedbackError> {
    let marker = root.join(RESEARCH_FEEDBACK_CONSENT_FILE);
    let metadata = match fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ResearchFeedbackError::Io),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ResearchFeedbackError::InvalidConsent);
    }
    let bytes = fs::read(marker).map_err(|_| ResearchFeedbackError::Io)?;
    if bytes != CONSENT_BYTES {
        return Err(ResearchFeedbackError::InvalidConsent);
    }
    Ok(true)
}

/// Enables or disables future continuous feedback batches.
///
/// Disabling removes only the non-private consent marker. Existing encrypted
/// packages remain untouched and recoverable.
pub fn set_research_feedback_enabled(
    root: &Path,
    enabled: bool,
) -> Result<bool, ResearchFeedbackError> {
    if enabled {
        prepare_root(root)?;
        if research_feedback_enabled(root)? {
            return Ok(false);
        }
        publish_consent(root)
    } else {
        if !research_feedback_enabled(root)? {
            return Ok(false);
        }
        fs::remove_file(root.join(RESEARCH_FEEDBACK_CONSENT_FILE))
            .map_err(|_| ResearchFeedbackError::Io)?;
        sync_directory(root)?;
        Ok(true)
    }
}

fn prepare_root(root: &Path) -> Result<(), ResearchFeedbackError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(ResearchFeedbackError::InvalidRoot)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| ResearchFeedbackError::Io)?;
            let metadata = fs::symlink_metadata(root).map_err(|_| ResearchFeedbackError::Io)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ResearchFeedbackError::InvalidRoot);
            }
            Ok(())
        }
        Err(_) => Err(ResearchFeedbackError::Io),
    }
}

fn publish_consent(root: &Path) -> Result<bool, ResearchFeedbackError> {
    let counter = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = root.join(format!(
        ".{RESEARCH_FEEDBACK_CONSENT_FILE}-{}-{counter}.tmp",
        std::process::id()
    ));
    let destination = root.join(RESEARCH_FEEDBACK_CONSENT_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| ResearchFeedbackError::Io)?;
    let write_result = file
        .write_all(CONSENT_BYTES)
        .and_then(|_| file.sync_all())
        .map_err(|_| ResearchFeedbackError::Io);
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    match fs::rename(&temporary, &destination) {
        Ok(()) => {
            sync_directory(root)?;
            Ok(true)
        }
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            if research_feedback_enabled(root)? {
                Ok(false)
            } else {
                Err(ResearchFeedbackError::Io)
            }
        }
    }
}

#[cfg(not(windows))]
fn sync_directory(root: &Path) -> Result<(), ResearchFeedbackError> {
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ResearchFeedbackError::Io)
}

#[cfg(windows)]
fn sync_directory(_root: &Path) -> Result<(), ResearchFeedbackError> {
    // Rust's ordinary File::open does not request FILE_FLAG_BACKUP_SEMANTICS,
    // so it cannot portably fsync a directory handle on Windows. The marker
    // itself is synced before the same-directory rename.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary_root(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "ziranma-research-feedback-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn consent_is_explicit_idempotent_and_disable_preserves_packages() {
        let root = temporary_root("lifecycle");
        assert!(!research_feedback_enabled(&root).unwrap());
        assert!(set_research_feedback_enabled(&root, true).unwrap());
        assert!(!set_research_feedback_enabled(&root, true).unwrap());
        assert!(research_feedback_enabled(&root).unwrap());

        let package = root.join("wish-private.ziw");
        fs::write(&package, b"protected-placeholder").unwrap();
        assert!(set_research_feedback_enabled(&root, false).unwrap());
        assert!(!set_research_feedback_enabled(&root, false).unwrap());
        assert!(!research_feedback_enabled(&root).unwrap());
        assert!(package.is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_marker_fails_closed_without_replacement() {
        let root = temporary_root("malformed");
        fs::create_dir(&root).unwrap();
        let marker = root.join(RESEARCH_FEEDBACK_CONSENT_FILE);
        fs::write(&marker, b"enabled=true\n").unwrap();

        assert_eq!(
            research_feedback_enabled(&root),
            Err(ResearchFeedbackError::InvalidConsent)
        );
        assert_eq!(
            set_research_feedback_enabled(&root, true),
            Err(ResearchFeedbackError::InvalidConsent)
        );
        assert_eq!(fs::read(&marker).unwrap(), b"enabled=true\n");

        fs::remove_dir_all(root).unwrap();
    }
}
