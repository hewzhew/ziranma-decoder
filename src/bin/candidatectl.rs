//! Read-only inspection of one explicitly named candidate package.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use ziranma_core::{
    CandidatePackageManifest, CandidateSnapshot, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    MAX_CANDIDATE_SNAPSHOT_BYTES,
};

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Inspect { manifest: PathBuf, payload: PathBuf },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Inspect { manifest, payload } => inspect(&manifest, &payload)?,
    }
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(Options::Help);
    };
    if command == "--help" || command == "-h" {
        if arguments.next().is_some() {
            return Err("--help must be used by itself".into());
        }
        return Ok(Options::Help);
    }
    if command != "inspect" {
        return Err("unknown candidatectl command; value was suppressed".into());
    }

    let mut manifest = None;
    let mut payload = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--manifest" => {
                if manifest.is_some() {
                    return Err("--manifest can be given only once".into());
                }
                manifest = Some(PathBuf::from(
                    arguments.next().ok_or("--manifest requires a path")?,
                ));
            }
            "--payload" => {
                if payload.is_some() {
                    return Err("--payload can be given only once".into());
                }
                payload = Some(PathBuf::from(
                    arguments.next().ok_or("--payload requires a path")?,
                ));
            }
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err("unknown inspect argument; value was suppressed".into()),
        }
    }

    Ok(Options::Inspect {
        manifest: manifest.ok_or("inspect requires exactly one --manifest path")?,
        payload: payload.ok_or("inspect requires exactly one --payload path")?,
    })
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --bin candidatectl -- inspect \\\n+         --manifest <PACKAGE.zcm> --payload <LEXICON.tsv>"
    );
    eprintln!(
        "Reads only the two explicitly named files. It does not write, learn, scan directories, \
         change TSF configuration, or use the network."
    );
}

fn inspect(manifest_path: &Path, payload_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_text = read_explicit_text(
        manifest_path,
        "candidate manifest",
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    )?;
    let manifest = CandidatePackageManifest::parse(&manifest_text)?;
    let payload_text = read_explicit_text(
        payload_path,
        "candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let snapshot = manifest.load_snapshot(&payload_text)?;
    print!("{}", render_report(&snapshot));
    Ok(())
}

fn read_explicit_text(
    path: &Path,
    label: &str,
    maximum_bytes: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| format!("cannot inspect explicitly named {label}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} cannot be a symbolic link").into());
    }
    if !metadata.is_file() {
        return Err(format!("{label} must be a regular file").into());
    }
    let maximum_u64 = u64::try_from(maximum_bytes).unwrap_or(u64::MAX);
    if metadata.len() == 0 || metadata.len() > maximum_u64 {
        return Err(format!("{label} size is outside its fixed limit").into());
    }

    let mut file = File::open(path).map_err(|_| format!("cannot open explicitly named {label}"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| format!("cannot inspect opened {label}"))?;
    if !opened_metadata.is_file()
        || opened_metadata.len() == 0
        || opened_metadata.len() > maximum_u64
    {
        return Err(format!("{label} changed to an invalid file").into());
    }

    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("cannot read explicitly named {label}"))?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(format!("{label} changed to an invalid size").into());
    }
    String::from_utf8(bytes).map_err(|_| format!("{label} must be UTF-8").into())
}

fn render_report(snapshot: &CandidateSnapshot) -> String {
    let mut output = String::new();
    writeln!(output, "候选包检查").unwrap();
    writeln!(output, "版本：{}", snapshot.revision()).unwrap();
    writeln!(
        output,
        "内容：{}",
        if snapshot.contains_private_text() {
            "含私人文字"
        } else {
            "公开"
        }
    )
    .unwrap();
    writeln!(output, "词条：{}", snapshot.entry_count()).unwrap();
    writeln!(output, "载荷：{} 字节", snapshot.payload_bytes()).unwrap();
    writeln!(output, "校验：通过").unwrap();
    writeln!(output, "本次操作：只读").unwrap();
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    const MANIFEST: &str = include_str!("../../tests/fixtures/public/demo_candidate_manifest.zcm");
    const LEXICON: &str = include_str!("../../tests/fixtures/public/demo_lexicon.tsv");

    #[test]
    fn parser_requires_one_explicit_manifest_and_payload() {
        assert_eq!(
            parse_options([
                "inspect".to_owned(),
                "--payload".to_owned(),
                "words.tsv".to_owned(),
                "--manifest".to_owned(),
                "package.zcm".to_owned(),
            ])
            .unwrap(),
            Options::Inspect {
                manifest: PathBuf::from("package.zcm"),
                payload: PathBuf::from("words.tsv"),
            }
        );
        assert!(parse_options(["inspect".to_owned()]).is_err());
        let error = parse_options(["secret-command".to_owned()]).unwrap_err();
        assert!(!error.to_string().contains("secret-command"));
    }

    #[test]
    fn report_is_compact_and_never_echoes_candidate_text_or_fingerprint() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        let snapshot = manifest.load_snapshot(LEXICON).unwrap();
        let report = render_report(&snapshot);
        assert_eq!(
            report,
            "候选包检查\n版本：tsf-public-demo-v1\n内容：公开\n词条：50\n\
             载荷：1132 字节\n校验：通过\n本次操作：只读\n"
        );
        assert!(!report.contains("你好"));
        assert!(!report.contains("nihk"));
        assert!(!report.contains("592a4dbb4b33efa6"));
    }

    #[test]
    fn explicit_reader_rejects_empty_oversized_and_non_utf8_files() {
        let root = std::env::temp_dir().join(format!(
            "ziranma-candidatectl-test-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let file = root.join("input");

        fs::write(&file, []).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, b"12345").unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, [0xff]).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, b"ok").unwrap();
        assert_eq!(read_explicit_text(&file, "test input", 4).unwrap(), "ok");

        fs::remove_dir_all(root).unwrap();
    }
}
