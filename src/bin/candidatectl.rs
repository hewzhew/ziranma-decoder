//! Explicit construction, inspection, and local slotting of candidate packages.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
use ziranma_core::preflight_candidate_snapshot;
use ziranma_core::{
    CANDIDATE_PACKAGE_MANIFEST_FILE, CANDIDATE_PACKAGE_PAYLOAD_FILE, CANDIDATE_PACKAGES_DIRECTORY,
    CANDIDATE_PREFLIGHTS_DIRECTORY, CANDIDATE_SLOT_STATE_FILE, CandidatePackageManifest,
    CandidateSlotState, CandidateSnapshot, MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES, MAX_CANDIDATE_SLOT_STATE_BYTES,
    MAX_CANDIDATE_SNAPSHOT_BYTES, candidate_package_storage_id, candidate_preflight_receipt_body,
    parse_lexicon_tsv,
};

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Inspect {
        manifest: PathBuf,
        payload: PathBuf,
    },
    Build {
        source: PathBuf,
        output: PathBuf,
        revision: String,
    },
    Preflight {
        package: PathBuf,
    },
    Status {
        root: PathBuf,
    },
    Adopt {
        root: PathBuf,
        package: PathBuf,
    },
    Stage {
        root: PathBuf,
        package: PathBuf,
    },
    Promote {
        root: PathBuf,
    },
    Rollback {
        root: PathBuf,
    },
}

struct LoadedPackage {
    manifest_text: String,
    payload_text: String,
    manifest: CandidatePackageManifest,
    snapshot: Arc<CandidateSnapshot>,
}

struct PreflightSummary {
    revision: String,
    input_keys: usize,
    committed_characters: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = match parse_options(std::env::args().skip(1))? {
        Options::Help => {
            print_usage();
            return Ok(());
        }
        Options::Inspect { manifest, payload } => inspect(&manifest, &payload)?,
        Options::Build {
            source,
            output,
            revision,
        } => build_public_package(&source, &output, &revision)?,
        Options::Preflight { package } => preflight(&package)?,
        Options::Status { root } => status(&root)?,
        Options::Adopt { root, package } => adopt(&root, &package)?,
        Options::Stage { root, package } => stage(&root, &package)?,
        Options::Promote { root } => promote(&root)?,
        Options::Rollback { root } => rollback(&root)?,
    };
    print!("{output}");
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(Options::Help);
    };
    if command == "--help" || command == "-h" || command == "help" {
        reject_extra(arguments)?;
        return Ok(Options::Help);
    }

    match command.as_str() {
        "inspect" => parse_inspect(arguments),
        "build" => parse_build(arguments),
        "preflight" => Ok(Options::Preflight {
            package: parse_package_only(arguments, "preflight")?,
        }),
        "status" => Ok(Options::Status {
            root: parse_root_only(arguments, "status")?,
        }),
        "adopt" => {
            let (root, package) = parse_root_and_package(arguments, "adopt")?;
            Ok(Options::Adopt { root, package })
        }
        "stage" => {
            let (root, package) = parse_root_and_package(arguments, "stage")?;
            Ok(Options::Stage { root, package })
        }
        "promote" => Ok(Options::Promote {
            root: parse_root_only(arguments, "promote")?,
        }),
        "rollback" => Ok(Options::Rollback {
            root: parse_root_only(arguments, "rollback")?,
        }),
        _ => Err("unknown candidatectl command; value was suppressed".into()),
    }
}

fn parse_inspect(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut manifest = None;
    let mut payload = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--manifest" => set_path(&mut manifest, &mut arguments, "--manifest")?,
            "--payload" => set_path(&mut payload, &mut arguments, "--payload")?,
            _ => return Err("unknown inspect argument; value was suppressed".into()),
        }
    }
    Ok(Options::Inspect {
        manifest: manifest.ok_or("inspect requires exactly one --manifest path")?,
        payload: payload.ok_or("inspect requires exactly one --payload path")?,
    })
}

fn parse_build(
    mut arguments: impl Iterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut source = None;
    let mut output = None;
    let mut revision = None;
    let mut public = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source" => set_path(&mut source, &mut arguments, "--source")?,
            "--output" => set_path(&mut output, &mut arguments, "--output")?,
            "--revision" => {
                if revision.is_some() {
                    return Err("--revision can be given only once".into());
                }
                revision = Some(arguments.next().ok_or("--revision requires a value")?);
            }
            "--public" => {
                if public {
                    return Err("--public can be given only once".into());
                }
                public = true;
            }
            _ => return Err("unknown build argument; value was suppressed".into()),
        }
    }
    if !public {
        return Err(
            "build requires explicit --public; private package building is unavailable".into(),
        );
    }
    Ok(Options::Build {
        source: source.ok_or("build requires exactly one --source path")?,
        output: output.ok_or("build requires exactly one --output path")?,
        revision: revision.ok_or("build requires exactly one --revision value")?,
    })
}

fn parse_root_only(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut root = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    root.ok_or_else(|| format!("{command} requires exactly one --root path").into())
}

fn parse_root_and_package(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let mut root = None;
    let mut package = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_path(&mut root, &mut arguments, "--root")?,
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    Ok((
        root.ok_or_else(|| format!("{command} requires exactly one --root path"))?,
        package.ok_or_else(|| format!("{command} requires exactly one --package path"))?,
    ))
}

fn parse_package_only(
    mut arguments: impl Iterator<Item = String>,
    command: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut package = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--package" => set_path(&mut package, &mut arguments, "--package")?,
            _ => return Err(format!("unknown {command} argument; value was suppressed").into()),
        }
    }
    package.ok_or_else(|| format!("{command} requires exactly one --package path").into())
}

fn set_path(
    slot: &mut Option<PathBuf>,
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if slot.is_some() {
        return Err(format!("{option} can be given only once").into());
    }
    *slot = Some(PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| format!("{option} requires a path"))?,
    ));
    Ok(())
}

fn reject_extra(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.next().is_some() {
        return Err("unexpected argument; value was suppressed".into());
    }
    Ok(())
}

fn print_usage() {
    eprintln!("candidatectl commands:");
    eprintln!("  inspect --manifest <PACKAGE.zcm> --payload <LEXICON.tsv>");
    eprintln!(
        "  build --source <LEXICON.tsv> --output <NEW_PACKAGE_DIR> --revision <REV> --public"
    );
    eprintln!("  preflight --package <PACKAGE_DIR>");
    eprintln!("  status --root <SLOT_DIR>");
    eprintln!("  adopt|stage --root <SLOT_DIR> --package <PACKAGE_DIR>");
    eprintln!("  promote|rollback --root <SLOT_DIR>");
}

fn inspect(
    manifest_path: &Path,
    payload_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
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
    Ok(render_inspect_report(&snapshot))
}

fn build_public_package(
    source: &Path,
    output: &Path,
    revision: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    ensure_path_absent(output, "package output")?;
    let payload = read_explicit_text(
        source,
        "public lexicon source",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let manifest = CandidatePackageManifest::from_payload(revision, false, &payload)?;
    let manifest_text = manifest.render();

    fs::create_dir(output).map_err(|_| "cannot create explicitly named package output")?;
    write_new_synced(
        &output.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        payload.as_bytes(),
    )?;
    write_new_synced(
        &output.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        manifest_text.as_bytes(),
    )?;

    let loaded = load_package_directory(output)?;
    Ok(render_build_report(&loaded.snapshot))
}

fn status(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let state = read_slot_state(root)?;
    render_slot_report(root, &state)
}

fn preflight(package: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let loaded = load_public_package_directory(package)?;
    let summary = preflight_loaded_package(&loaded)?;
    Ok(render_preflight_report(&summary))
}

fn adopt(root: &Path, package: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    if state.current().is_some() {
        return Err("current candidate package is already configured".into());
    }
    let loaded = load_public_package_directory(package)?;
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    preflight_loaded_package(&installed)?;
    write_preflight_receipt(root, &package_id)?;
    state.adopt(&package_id)?;
    write_slot_state(root, &state)?;
    Ok(render_preflight_change_report(
        "当前候选包已建立",
        &revision,
    ))
}

fn stage(root: &Path, package: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    if state.current().is_none() {
        return Err("current candidate package is not configured".into());
    }
    let loaded = load_public_package_directory(package)?;
    let revision = loaded.snapshot.revision().to_owned();
    prepare_slot_root(root)?;
    let package_id = install_package(root, &loaded)?;
    let installed = load_installed_package(root, &package_id)?;
    preflight_loaded_package(&installed)?;
    write_preflight_receipt(root, &package_id)?;
    state.stage(&package_id)?;
    write_slot_state(root, &state)?;
    Ok(render_preflight_change_report(
        "待切换候选包已暂存",
        &revision,
    ))
}

fn promote(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    validate_installed_slot(root, state.current())?;
    let next = validate_installed_slot(root, state.candidate())?;
    validate_preflight_receipt(
        root,
        state
            .candidate()
            .ok_or("required candidate slot is empty")?,
    )?;
    let revision = next.snapshot.revision().to_owned();
    state.promote()?;
    write_slot_state(root, &state)?;
    Ok(render_change_report("候选数据槽已切换", &revision))
}

fn rollback(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut state = read_slot_state(root)?;
    validate_installed_slot(root, state.current())?;
    let previous = validate_installed_slot(root, state.previous())?;
    validate_preflight_receipt(
        root,
        state.previous().ok_or("required previous slot is empty")?,
    )?;
    let revision = previous.snapshot.revision().to_owned();
    state.rollback()?;
    write_slot_state(root, &state)?;
    Ok(render_change_report("候选数据槽已回退", &revision))
}

fn render_inspect_report(snapshot: &CandidateSnapshot) -> String {
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

fn render_build_report(snapshot: &CandidateSnapshot) -> String {
    format!(
        "公开候选包已生成\n版本：{}\n词条：{}\n载荷：{} 字节\n写入：2 个新文件\n",
        snapshot.revision(),
        snapshot.entry_count(),
        snapshot.payload_bytes()
    )
}

fn render_preflight_report(summary: &PreflightSummary) -> String {
    format!(
        "TSF 候选预检\n版本：{}\n输入：{} 键\n上屏：{} 字\n结果：通过\n本次操作：不写文件\n",
        summary.revision, summary.input_keys, summary.committed_characters
    )
}

#[cfg(windows)]
fn preflight_loaded_package(
    loaded: &LoadedPackage,
) -> Result<PreflightSummary, Box<dyn std::error::Error>> {
    let entries = parse_lexicon_tsv(&loaded.payload_text)
        .map_err(|_| "candidate package has no usable TSF preflight probe")?;
    let probe = entries
        .first()
        .ok_or("candidate package has no usable TSF preflight probe")?;
    let code = probe.code.as_str();
    let expected = loaded
        .snapshot
        .candidate_text(code, 1)?
        .ok_or("candidate package produced no TSF preflight candidate")?;
    let report =
        preflight_candidate_snapshot(Arc::clone(&loaded.snapshot), code, expected.as_str())?;
    Ok(PreflightSummary {
        revision: report.revision().to_owned(),
        input_keys: report.input_keys(),
        committed_characters: report.committed_characters(),
    })
}

#[cfg(not(windows))]
fn preflight_loaded_package(
    _loaded: &LoadedPackage,
) -> Result<PreflightSummary, Box<dyn std::error::Error>> {
    Err("TSF candidate preflight requires Windows".into())
}

fn render_slot_report(
    root: &Path,
    state: &CandidateSlotState,
) -> Result<String, Box<dyn std::error::Error>> {
    let current = slot_revision(root, state.current())?;
    let candidate = slot_revision(root, state.candidate())?;
    let previous = slot_revision(root, state.previous())?;
    Ok(format!(
        "候选数据槽\n当前：{}\n待切换：{}\n可回退：{}\n本次操作：只读\n",
        current.as_deref().unwrap_or("未配置"),
        candidate.as_deref().unwrap_or("无"),
        previous.as_deref().unwrap_or("无")
    ))
}

fn render_change_report(action: &str, revision: &str) -> String {
    format!("{action}\n版本：{revision}\n")
}

fn render_preflight_change_report(action: &str, revision: &str) -> String {
    format!("{action}\n版本：{revision}\nTSF 预检：通过\n")
}

fn slot_revision(
    root: &Path,
    package_id: Option<&str>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match package_id {
        Some(package_id) => Ok(Some(
            load_installed_package(root, package_id)?
                .snapshot
                .revision()
                .to_owned(),
        )),
        None => Ok(None),
    }
}

fn load_public_package_directory(
    package: &Path,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let loaded = load_package_directory(package)?;
    if loaded.snapshot.contains_private_text() {
        return Err(
            "plaintext private candidate packages are not accepted by this slot store".into(),
        );
    }
    Ok(loaded)
}

fn load_package_directory(package: &Path) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    ensure_regular_directory(package, "candidate package")?;
    let manifest_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        "candidate manifest",
        MAX_CANDIDATE_PACKAGE_MANIFEST_BYTES,
    )?;
    let payload_text = read_explicit_text(
        &package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        "candidate payload",
        MAX_CANDIDATE_SNAPSHOT_BYTES,
    )?;
    let manifest = CandidatePackageManifest::parse(&manifest_text)?;
    let snapshot = Arc::new(manifest.load_snapshot(&payload_text)?);
    Ok(LoadedPackage {
        manifest_text,
        payload_text,
        manifest,
        snapshot,
    })
}

fn validate_installed_slot(
    root: &Path,
    package_id: Option<&str>,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let package_id = package_id.ok_or("required candidate slot is empty")?;
    load_installed_package(root, package_id)
}

fn load_installed_package(
    root: &Path,
    package_id: &str,
) -> Result<LoadedPackage, Box<dyn std::error::Error>> {
    let loaded = load_package_directory(&root.join(CANDIDATE_PACKAGES_DIRECTORY).join(package_id))?;
    if loaded.snapshot.contains_private_text() {
        return Err("candidate slot unexpectedly contains plaintext private text".into());
    }
    if candidate_package_storage_id(&loaded.manifest_text, &loaded.payload_text) != package_id {
        return Err("installed candidate package no longer matches its storage identifier".into());
    }
    Ok(loaded)
}

fn install_package(
    root: &Path,
    loaded: &LoadedPackage,
) -> Result<String, Box<dyn std::error::Error>> {
    let package_id = candidate_package_storage_id(&loaded.manifest_text, &loaded.payload_text);
    let packages = root.join(CANDIDATE_PACKAGES_DIRECTORY);
    let destination = packages.join(&package_id);

    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            let installed = load_public_package_directory(&destination)?;
            if installed.manifest_text != loaded.manifest_text
                || installed.payload_text != loaded.payload_text
            {
                return Err("candidate package storage identifier collision".into());
            }
            return Ok(package_id);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("cannot inspect candidate package destination".into()),
    }

    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = packages.join(format!(".install-{}-{stamp}", std::process::id()));
    fs::create_dir(&temporary).map_err(|_| "cannot create temporary candidate package")?;
    write_new_synced(
        &temporary.join(CANDIDATE_PACKAGE_PAYLOAD_FILE),
        loaded.payload_text.as_bytes(),
    )?;
    write_new_synced(
        &temporary.join(CANDIDATE_PACKAGE_MANIFEST_FILE),
        loaded.manifest_text.as_bytes(),
    )?;
    fs::rename(&temporary, &destination).map_err(|_| "cannot install candidate package")?;
    let installed = load_public_package_directory(&destination)?;
    if installed.manifest != loaded.manifest || installed.payload_text != loaded.payload_text {
        return Err("installed candidate package failed exact verification".into());
    }
    Ok(package_id)
}

fn preflight_receipt_path(root: &Path, package_id: &str) -> PathBuf {
    root.join(CANDIDATE_PREFLIGHTS_DIRECTORY)
        .join(format!("{package_id}.zpf"))
}

fn write_preflight_receipt(
    root: &Path,
    package_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = candidate_preflight_receipt_body(package_id);
    let path = preflight_receipt_path(root, package_id);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let existing = read_explicit_text(
                &path,
                "candidate preflight receipt",
                MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
            )?;
            if existing != expected {
                return Err("candidate preflight receipt does not match its package".into());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_new_synced(&path, expected.as_bytes())
        }
        Err(_) => Err("cannot inspect candidate preflight receipt".into()),
    }
}

fn validate_preflight_receipt(
    root: &Path,
    package_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = read_explicit_text(
        &preflight_receipt_path(root, package_id),
        "candidate preflight receipt",
        MAX_CANDIDATE_PREFLIGHT_RECEIPT_BYTES,
    )?;
    if receipt != candidate_preflight_receipt_body(package_id) {
        return Err("candidate preflight receipt does not match its package".into());
    }
    Ok(())
}

fn prepare_slot_root(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| "cannot create candidate slot root")?;
            ensure_regular_directory(root, "candidate slot root")?;
        }
        Err(_) => return Err("cannot inspect candidate slot root".into()),
    }
    let packages = root.join(CANDIDATE_PACKAGES_DIRECTORY);
    match fs::symlink_metadata(&packages) {
        Ok(_) => ensure_regular_directory(&packages, "candidate package store")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&packages).map_err(|_| "cannot create candidate package store")?;
        }
        Err(_) => return Err("cannot inspect candidate package store".into()),
    }
    let preflights = root.join(CANDIDATE_PREFLIGHTS_DIRECTORY);
    match fs::symlink_metadata(&preflights) {
        Ok(_) => ensure_regular_directory(&preflights, "candidate preflight store")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&preflights).map_err(|_| "cannot create candidate preflight store")?;
        }
        Err(_) => return Err("cannot inspect candidate preflight store".into()),
    }
    Ok(())
}

fn read_slot_state(root: &Path) -> Result<CandidateSlotState, Box<dyn std::error::Error>> {
    match fs::symlink_metadata(root) {
        Ok(_) => ensure_regular_directory(root, "candidate slot root")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CandidateSlotState::default());
        }
        Err(_) => return Err("cannot inspect candidate slot root".into()),
    }
    let path = root.join(CANDIDATE_SLOT_STATE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let contents = read_explicit_text(
                &path,
                "candidate slot state",
                MAX_CANDIDATE_SLOT_STATE_BYTES,
            )?;
            Ok(CandidateSlotState::parse(&contents)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(CandidateSlotState::default())
        }
        Err(_) => Err("cannot inspect candidate slot state".into()),
    }
}

fn write_slot_state(
    root: &Path,
    state: &CandidateSlotState,
) -> Result<(), Box<dyn std::error::Error>> {
    prepare_slot_root(root)?;
    let body = state.render();
    CandidateSlotState::parse(&body)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(".slots-{}-{stamp}.tmp", std::process::id()));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = move_replace(&temporary, &root.join(CANDIDATE_SLOT_STATE_FILE));
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_path_absent(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!("{label} already exists; refusing to overwrite it").into()),
        Err(_) => Err(format!("cannot inspect explicitly named {label}").into()),
    }
}

fn ensure_regular_directory(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("cannot inspect {label}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{label} must be a regular directory").into());
    }
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
    Read::by_ref(&mut file)
        .take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("cannot read explicitly named {label}"))?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(format!("{label} changed to an invalid size").into());
    }
    String::from_utf8(bytes).map_err(|_| format!("{label} must be UTF-8").into())
}

fn write_new_synced(path: &Path, contents: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let source_wide = wide_path(source)?;
    let destination_wide = wide_path(destination)?;
    // SAFETY: Both NUL-terminated buffers live through the synchronous call.
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, Box<dyn std::error::Error>> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err("path contains an embedded NUL".into());
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(not(windows))]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    const MANIFEST: &str = include_str!("../../tests/fixtures/public/demo_candidate_manifest.zcm");
    const LEXICON: &str = include_str!("../../tests/fixtures/public/demo_lexicon.tsv");

    #[test]
    fn parser_requires_explicit_write_intent_and_paths() {
        assert_eq!(
            parse_options([
                "build".to_owned(),
                "--revision".to_owned(),
                "demo-v2".to_owned(),
                "--public".to_owned(),
                "--source".to_owned(),
                "words.tsv".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
            ])
            .unwrap(),
            Options::Build {
                source: PathBuf::from("words.tsv"),
                output: PathBuf::from("package"),
                revision: "demo-v2".to_owned(),
            }
        );
        assert!(parse_options(["build".to_owned()]).is_err());
        assert_eq!(
            parse_options([
                "preflight".to_owned(),
                "--package".to_owned(),
                "package".to_owned(),
            ])
            .unwrap(),
            Options::Preflight {
                package: PathBuf::from("package")
            }
        );
        assert!(parse_options(["preflight".to_owned()]).is_err());
        assert!(
            parse_options([
                "build".to_owned(),
                "--source".to_owned(),
                "private.tsv".to_owned(),
                "--output".to_owned(),
                "package".to_owned(),
                "--revision".to_owned(),
                "private-v1".to_owned(),
            ])
            .is_err()
        );
        let error = parse_options(["secret-command".to_owned()]).unwrap_err();
        assert!(!error.to_string().contains("secret-command"));
    }

    #[test]
    fn report_is_compact_and_never_echoes_candidate_text_or_fingerprint() {
        let manifest = CandidatePackageManifest::parse(MANIFEST).unwrap();
        let snapshot = manifest.load_snapshot(LEXICON).unwrap();
        let report = render_inspect_report(&snapshot);
        assert_eq!(
            report,
            "候选包检查\n版本：tsf-public-demo-v1\n内容：公开\n词条：50\n\
             载荷：1132 字节\n校验：通过\n本次操作：只读\n"
        );
        assert!(!report.contains("你好"));
        assert!(!report.contains("nihk"));
        assert!(!report.contains("592a4dbb4b33efa6"));

        let preflight = render_preflight_report(&PreflightSummary {
            revision: "tsf-public-demo-v1".to_owned(),
            input_keys: 4,
            committed_characters: 2,
        });
        assert_eq!(
            preflight,
            "TSF 候选预检\n版本：tsf-public-demo-v1\n输入：4 键\n上屏：2 字\n\
             结果：通过\n本次操作：不写文件\n"
        );
        assert!(!preflight.contains("你好"));
        assert!(!preflight.contains("nihk"));
    }

    #[test]
    fn public_build_and_slot_lifecycle_round_trip_real_files() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();

        let built_a = build_public_package(&source, &package_a, "public-a").unwrap();
        let built_b = build_public_package(&source, &package_b, "public-b").unwrap();
        assert!(built_a.contains("版本：public-a"));
        assert!(built_b.contains("版本：public-b"));
        assert_eq!(
            fs::read_to_string(package_a.join(CANDIDATE_PACKAGE_PAYLOAD_FILE)).unwrap(),
            LEXICON
        );
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：未配置\n待切换：无\n可回退：无\n本次操作：只读\n"
        );
        assert!(!slots.exists());

        adopt(&slots, &package_a).unwrap();
        let adopted_state = read_slot_state(&slots).unwrap();
        let receipt = fs::read_to_string(preflight_receipt_path(
            &slots,
            adopted_state.current().unwrap(),
        ))
        .unwrap();
        assert!(!receipt.contains("你好"));
        assert!(!receipt.contains("nihk"));
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：无\n可回退：无\n本次操作：只读\n"
        );
        stage(&slots, &package_b).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：public-b\n可回退：无\n本次操作：只读\n"
        );
        promote(&slots).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-b\n待切换：无\n可回退：public-a\n本次操作：只读\n"
        );
        rollback(&slots).unwrap();
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：public-a\n待切换：无\n可回退：public-b\n本次操作：只读\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_preflight_receipt_blocks_promotion_without_changing_slots() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        build_public_package(&source, &package_a, "receipt-a").unwrap();
        build_public_package(&source, &package_b, "receipt-b").unwrap();
        adopt(&slots, &package_a).unwrap();
        stage(&slots, &package_b).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let candidate = before.candidate().unwrap();
        fs::remove_file(preflight_receipt_path(&slots, candidate)).unwrap();
        assert!(promote(&slots).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_installed_package_cannot_reuse_an_old_preflight_receipt() {
        let root = temporary_test_root();
        let source = root.join("source.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source, LEXICON).unwrap();
        build_public_package(&source, &package_a, "immutable-a").unwrap();
        build_public_package(&source, &package_b, "immutable-b").unwrap();
        adopt(&slots, &package_a).unwrap();
        stage(&slots, &package_b).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let candidate = before.candidate().unwrap();
        let changed_manifest =
            CandidatePackageManifest::from_payload("changed-after-preflight", false, LEXICON)
                .unwrap()
                .render();
        fs::write(
            slots
                .join(CANDIDATE_PACKAGES_DIRECTORY)
                .join(candidate)
                .join(CANDIDATE_PACKAGE_MANIFEST_FILE),
            changed_manifest,
        )
        .unwrap();
        assert!(promote(&slots).is_err());
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_tsf_preflight_leaves_current_and_candidate_unchanged() {
        let root = temporary_test_root();
        let source_a = root.join("source-a.tsv");
        let source_b = root.join("source-b.tsv");
        let package_a = root.join("package-a");
        let package_b = root.join("package-b");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::write(&source_a, LEXICON).unwrap();
        fs::write(
            &source_b,
            format!("text\tpinyin\tfrequency\n{}\ta\t1\n", "测".repeat(257)),
        )
        .unwrap();
        build_public_package(&source_a, &package_a, "preflight-good").unwrap();
        build_public_package(&source_b, &package_b, "preflight-rejected").unwrap();
        adopt(&slots, &package_a).unwrap();

        let before = read_slot_state(&slots).unwrap();
        let error = stage(&slots, &package_b).unwrap_err().to_string();
        assert!(!error.contains('测'));
        assert_eq!(read_slot_state(&slots).unwrap(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn build_refuses_overwrite_and_explicit_reader_rejects_bad_files() {
        let root = temporary_test_root();
        fs::create_dir(&root).unwrap();
        let file = root.join("input");

        fs::write(&file, []).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, b"12345").unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, [0xff]).unwrap();
        assert!(read_explicit_text(&file, "test input", 4).is_err());
        fs::write(&file, LEXICON).unwrap();

        let package = root.join("package");
        build_public_package(&file, &package, "public-once").unwrap();
        assert!(build_public_package(&file, &package, "public-twice").is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_slot_store_rejects_plaintext_private_package_without_state_change() {
        let root = temporary_test_root();
        let package = root.join("private-package");
        let slots = root.join("slots");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&package).unwrap();
        let manifest = CandidatePackageManifest::from_payload("private-v1", true, LEXICON)
            .unwrap()
            .render();
        fs::write(package.join(CANDIDATE_PACKAGE_PAYLOAD_FILE), LEXICON).unwrap();
        fs::write(package.join(CANDIDATE_PACKAGE_MANIFEST_FILE), manifest).unwrap();

        assert!(adopt(&slots, &package).is_err());
        assert!(!slots.exists());
        assert_eq!(
            status(&slots).unwrap(),
            "候选数据槽\n当前：未配置\n待切换：无\n可回退：无\n本次操作：只读\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ziranma-candidatectl-test-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
