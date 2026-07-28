//! Read only explicitly named, redacted tracker summaries and aggregate them.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use ziranma_core::{AggregatedSessionSummary, SessionSummaryV1};

const MAX_SUMMARY_BYTES: u64 = 64 * 1024;

enum Options {
    Help,
    Inputs(Vec<PathBuf>),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Inputs(inputs) => {
            let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let reports = read_explicit_summaries(manifest_dir, &inputs)?;
            let aggregate = AggregatedSessionSummary::from_reports(&reports)?;
            println!("{}", aggregate.terminal_line());
        }
    }
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let mut inputs = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--input" => {
                let value = arguments.next().ok_or("--input requires a path")?;
                inputs.push(PathBuf::from(value));
            }
            "--help" | "-h" if inputs.is_empty() && arguments.next().is_none() => {
                return Ok(Options::Help);
            }
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if inputs.is_empty() {
        return Err("at least one explicit --input path is required".into());
    }
    Ok(Options::Inputs(inputs))
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --bin summary-report -- \\\n         --input data/private/session-summaries/<FILE>.json \\\n         [--input data/private/session-summaries/<FILE>.json ...]"
    );
    eprintln!("Reads only named files, writes nothing, and never scans the private directory.");
}

fn read_explicit_summaries(
    manifest_dir: &Path,
    requested_inputs: &[PathBuf],
) -> Result<Vec<SessionSummaryV1>, Box<dyn std::error::Error>> {
    let summary_root = manifest_dir.join("data/private/session-summaries");
    let canonical_root = validate_summary_root(manifest_dir, &summary_root)?;
    let mut seen = HashSet::new();
    let mut reports = Vec::with_capacity(requested_inputs.len());

    for requested in requested_inputs {
        let target = resolve_summary_input(manifest_dir, &summary_root, requested)?;
        let metadata = fs::symlink_metadata(&target).map_err(|error| {
            format!(
                "cannot inspect explicitly named summary {}: {error}",
                target.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "summary input cannot be a symbolic link: {}",
                target.display()
            )
            .into());
        }
        if !metadata.is_file() {
            return Err(
                format!("summary input must be a regular file: {}", target.display()).into(),
            );
        }

        let canonical_target = fs::canonicalize(&target)?;
        if canonical_target.parent() != Some(canonical_root.as_path()) {
            return Err(format!(
                "summary input resolves outside the fixed private directory: {}",
                target.display()
            )
            .into());
        }
        if !seen.insert(canonical_target.clone()) {
            return Err(format!(
                "duplicate summary input is not allowed: {}",
                target.display()
            )
            .into());
        }

        let mut file = File::open(&canonical_target)?;
        let opened_metadata = file.metadata()?;
        if !opened_metadata.is_file() {
            return Err(format!(
                "summary input changed before it could be read: {}",
                target.display()
            )
            .into());
        }
        if opened_metadata.len() > MAX_SUMMARY_BYTES {
            return Err(format!(
                "summary input exceeds the {MAX_SUMMARY_BYTES}-byte limit: {}",
                target.display()
            )
            .into());
        }
        let mut json = String::new();
        file.by_ref()
            .take(MAX_SUMMARY_BYTES + 1)
            .read_to_string(&mut json)
            .map_err(|error| {
                format!(
                    "cannot read explicitly named summary {} as UTF-8: {error}",
                    target.display()
                )
            })?;
        if u64::try_from(json.len()).unwrap_or(u64::MAX) > MAX_SUMMARY_BYTES {
            return Err(format!(
                "summary input exceeds the {MAX_SUMMARY_BYTES}-byte limit: {}",
                target.display()
            )
            .into());
        }
        let report = SessionSummaryV1::from_json(&json).map_err(|error| {
            format!("invalid redacted v1 summary {}: {error}", target.display())
        })?;
        reports.push(report);
    }
    Ok(reports)
}

fn validate_summary_root(
    manifest_dir: &Path,
    summary_root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest_metadata = fs::symlink_metadata(manifest_dir)?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_dir() {
        return Err("repository root must be a real directory, not a symbolic link".into());
    }

    let mut current = manifest_dir.to_path_buf();
    for component in ["data", "private", "session-summaries"] {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "private summary directory is unavailable at {}: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "private summary directory contains a symbolic-link component: {}",
                current.display()
            )
            .into());
        }
        if !metadata.is_dir() {
            return Err(format!(
                "private summary directory component is not a directory: {}",
                current.display()
            )
            .into());
        }
    }

    let canonical_manifest = fs::canonicalize(manifest_dir)?;
    let canonical_root = fs::canonicalize(summary_root)?;
    if !canonical_root.starts_with(&canonical_manifest) {
        return Err("private summary directory resolves outside the repository".into());
    }
    Ok(canonical_root)
}

fn resolve_summary_input(
    manifest_dir: &Path,
    summary_root: &Path,
    requested: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if requested.as_os_str().is_empty() {
        return Err("summary input path cannot be empty".into());
    }
    let target = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        manifest_dir.join(requested)
    };
    if target.parent() != Some(summary_root) {
        return Err("summary input must be directly inside data/private/session-summaries".into());
    }
    if target.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Err("summary input must use the .json extension".into());
    }
    let file_name = target
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .ok_or("summary input file name must be valid Unicode")?;
    if file_name.starts_with('.') {
        return Err("summary input file name cannot be hidden".into());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SUMMARY_BYTES, Options, parse_options, read_explicit_summaries, resolve_summary_input,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use ziranma_core::{AggregatedSessionSummary, SessionSummaryCounts, SessionSummaryV1};

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestWorkspace {
        manifest: PathBuf,
        summary_root: PathBuf,
        files: Vec<PathBuf>,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let manifest = std::env::temp_dir().join(format!(
                "ziranma-summary-report-test-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            let summary_root = manifest.join("data/private/session-summaries");
            fs::create_dir_all(&summary_root).unwrap();
            Self {
                manifest,
                summary_root,
                files: Vec::new(),
            }
        }

        fn write(&mut self, name: &str, contents: &str) -> PathBuf {
            let path = self.summary_root.join(name);
            fs::write(&path, contents).unwrap();
            self.files.push(path.clone());
            path
        }

        fn relative(&self, name: &str) -> PathBuf {
            Path::new("data/private/session-summaries").join(name)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            for file in &self.files {
                let _ = fs::remove_file(file);
            }
            let _ = fs::remove_dir(&self.summary_root);
            let _ = fs::remove_dir(self.manifest.join("data/private"));
            let _ = fs::remove_dir(self.manifest.join("data"));
            let _ = fs::remove_dir(&self.manifest);
        }
    }

    fn report(elapsed_ms: u64) -> SessionSummaryV1 {
        SessionSummaryV1 {
            elapsed_ms,
            candidate_gap_limit_ms: 15_000,
            key_capture_requested: true,
            key_capture_ready: true,
            counts: SessionSummaryCounts {
                commits: 1,
                keys_complete_records: 1,
                logical_key_actions: 3,
                ..SessionSummaryCounts::default()
            },
        }
    }

    #[test]
    fn cli_requires_repeated_explicit_inputs() {
        assert!(parse_options(Vec::<String>::new()).is_err());
        assert!(matches!(
            parse_options(vec!["--help".to_owned()]).unwrap(),
            Options::Help
        ));
        let options = parse_options(vec![
            "--input".to_owned(),
            "data/private/session-summaries/a.json".to_owned(),
            "--input".to_owned(),
            "data/private/session-summaries/b.json".to_owned(),
        ])
        .unwrap();
        let Options::Inputs(inputs) = options else {
            panic!("expected inputs");
        };
        assert_eq!(inputs.len(), 2);
    }

    #[test]
    fn inputs_are_restricted_to_direct_nonhidden_json_children() {
        let manifest = Path::new(r"D:\repo");
        let root = manifest.join("data/private/session-summaries");
        assert_eq!(
            resolve_summary_input(
                manifest,
                &root,
                Path::new("data/private/session-summaries/run-001.json")
            )
            .unwrap(),
            root.join("run-001.json")
        );
        for invalid in [
            "run.json",
            "data/private/run.json",
            "data/private/session-summaries/nested/run.json",
            "data/private/session-summaries/run.txt",
            "data/private/session-summaries/.hidden.json",
        ] {
            assert!(
                resolve_summary_input(manifest, &root, Path::new(invalid)).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn only_named_files_are_read_and_aggregated() {
        let mut workspace = TestWorkspace::new();
        workspace.write("first.json", &report(1_000).to_json().unwrap());
        workspace.write("second.json", &report(2_000).to_json().unwrap());
        workspace.write("unlisted-malformed.json", "private text that is not JSON");

        let reports = read_explicit_summaries(
            &workspace.manifest,
            &[
                workspace.relative("first.json"),
                workspace.relative("second.json"),
            ],
        )
        .unwrap();
        let aggregate = AggregatedSessionSummary::from_reports(&reports).unwrap();
        assert_eq!(aggregate.files, 2);
        assert_eq!(aggregate.total_elapsed_ms, 3_000);
        assert_eq!(aggregate.counts.commits, 2);
    }

    #[test]
    fn duplicate_and_malformed_inputs_are_rejected_without_echoing_contents() {
        let mut workspace = TestWorkspace::new();
        let good = workspace.write("good.json", &report(1_000).to_json().unwrap());
        let relative = workspace.relative("good.json");
        let duplicate_error =
            read_explicit_summaries(&workspace.manifest, &[relative, good]).unwrap_err();
        assert!(duplicate_error.to_string().contains("duplicate"));

        let private_marker = "DO_NOT_ECHO_PRIVATE_MARKER";
        workspace.write("bad.json", private_marker);
        let malformed_error =
            read_explicit_summaries(&workspace.manifest, &[workspace.relative("bad.json")])
                .unwrap_err();
        assert!(!malformed_error.to_string().contains(private_marker));
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let mut workspace = TestWorkspace::new();
        let oversized = "0".repeat(usize::try_from(MAX_SUMMARY_BYTES + 1).unwrap());
        workspace.write("oversized.json", &oversized);
        let error =
            read_explicit_summaries(&workspace.manifest, &[workspace.relative("oversized.json")])
                .unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }
}
