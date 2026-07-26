//! Explicit, read-only replay of private event capsules against public data.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use ziranma_decoder::{
    BigramLanguageModel, CapsuleReplayReport, CharacterBigramLanguageModel, Decoder,
    DeltaPositionEvidence, EventCapsuleV1, PersonalCacheReplayState, RawKey, TrackerOutput,
    parse_rime_lexicon, parse_ud_conllu, select_public_bigram_training_sequences,
};
#[cfg(windows)]
use ziranma_decoder::{
    CODEX_CAPTURE_PROFILE_V1, ContinuousSegmentV1, DataProtector, ProtectedSegmentEnvelopeV1,
    WindowsUserDataProtector,
};

const PUBLIC_RIME_LEXICON: &str =
    include_str!("../../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
const PUBLIC_UD_TRAIN: &str =
    include_str!("../../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu");
const MAX_CAPSULE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_SEGMENTS: u64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
enum InputSelector {
    Path(PathBuf),
    Session(String),
}

enum Options {
    Help,
    Inputs {
        history_inputs: Vec<InputSelector>,
        inputs: Vec<InputSelector>,
        window_gap_ms: Option<u64>,
        compact: bool,
        public_context: bool,
        public_character_context: bool,
        personal_cache: bool,
        personal_pair_cache: bool,
        health_only: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Inputs {
            history_inputs,
            inputs,
            window_gap_ms,
            compact,
            public_context,
            public_character_context,
            personal_cache,
            personal_pair_cache,
            health_only,
        } => {
            let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let history_inputs = expand_input_selectors(manifest_dir, &history_inputs)?;
            let inputs = expand_input_selectors(manifest_dir, &inputs)?;
            let history_count = history_inputs.len();
            let mut requested_inputs = history_inputs;
            requested_inputs.extend(inputs);
            let mut history_capsules = read_explicit_capsules(manifest_dir, &requested_inputs)?;
            let capsules = history_capsules.split_off(history_count);
            if health_only {
                println!(
                    "{}",
                    CaptureHealthReport::from_capsules(&capsules).terminal_line()
                );
                return Ok(());
            }
            let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
            let entries = imported.entries;
            let decoder = Decoder::new(entries.clone());
            let mut report = CapsuleReplayReport::with_window_gap_limit(window_gap_ms)?;
            if personal_cache || personal_pair_cache {
                let mut state = PersonalCacheReplayState::new();
                if !history_capsules.is_empty() {
                    let mut history_report =
                        CapsuleReplayReport::with_window_gap_limit(window_gap_ms)?;
                    for capsule in &history_capsules {
                        if personal_pair_cache {
                            history_report.observe_capsule_with_personal_pair_cache(
                                &decoder, &mut state, capsule,
                            )?;
                        } else {
                            history_report.observe_capsule_with_personal_cache(
                                &decoder, &mut state, capsule,
                            )?;
                        }
                    }
                    report.record_personal_cache_history(&history_report, &state);
                }
                for capsule in &capsules {
                    if personal_pair_cache {
                        report.observe_capsule_with_personal_pair_cache(
                            &decoder, &mut state, capsule,
                        )?;
                    } else {
                        report
                            .observe_capsule_with_personal_cache(&decoder, &mut state, capsule)?;
                    }
                }
            } else if public_context || public_character_context {
                let train_corpus = parse_ud_conllu(PUBLIC_UD_TRAIN)?;
                let training = select_public_bigram_training_sequences(&train_corpus, &entries);
                if public_context {
                    let language_model =
                        BigramLanguageModel::from_token_sequences(&training.sequences, &entries)?;
                    let frequency_total = entries
                        .iter()
                        .map(|entry| entry.frequency as f64)
                        .sum::<f64>();
                    let log_frequency_total = if frequency_total > 0.0 {
                        frequency_total.ln()
                    } else {
                        0.0
                    };
                    for capsule in &capsules {
                        report.observe_capsule_with_public_context(
                            &decoder,
                            &language_model,
                            log_frequency_total,
                            capsule,
                        )?;
                    }
                } else {
                    let text_sequences = training
                        .sequences
                        .iter()
                        .map(|sequence| sequence.concat())
                        .collect::<Vec<_>>();
                    let language_model =
                        CharacterBigramLanguageModel::from_text_sequences(&text_sequences)?;
                    for capsule in &capsules {
                        report.observe_capsule_with_public_character_context(
                            &decoder,
                            &language_model,
                            capsule,
                        )?;
                    }
                }
            } else {
                for capsule in &capsules {
                    report.observe_capsule(&decoder, capsule)?;
                }
            }
            if compact {
                println!("{}", report.compact_terminal_report());
            } else {
                println!("{}", report.terminal_line());
            }
        }
    }
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let mut history_inputs = Vec::new();
    let mut inputs = Vec::new();
    let mut window_gap_ms = None;
    let mut compact = false;
    let mut public_context = false;
    let mut public_character_context = false;
    let mut personal_cache = false;
    let mut personal_pair_cache = false;
    let mut health_only = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--history-input" => {
                let value = arguments.next().ok_or("--history-input requires a path")?;
                history_inputs.push(InputSelector::Path(PathBuf::from(value)));
            }
            "--history-session" => {
                let value = arguments
                    .next()
                    .ok_or("--history-session requires a session id")?;
                validate_session_selector(&value)?;
                history_inputs.push(InputSelector::Session(value));
            }
            "--input" => {
                let value = arguments.next().ok_or("--input requires a path")?;
                inputs.push(InputSelector::Path(PathBuf::from(value)));
            }
            "--session" => {
                let value = arguments.next().ok_or("--session requires a session id")?;
                validate_session_selector(&value)?;
                inputs.push(InputSelector::Session(value));
            }
            "--window-gap-ms" => {
                if window_gap_ms.is_some() {
                    return Err("--window-gap-ms can be given only once".into());
                }
                let value = arguments
                    .next()
                    .ok_or("--window-gap-ms requires a value")?
                    .parse::<u64>()?;
                if value == 0 {
                    return Err("--window-gap-ms must be greater than zero".into());
                }
                window_gap_ms = Some(value);
            }
            "--compact" => {
                if compact {
                    return Err("--compact can be given only once".into());
                }
                compact = true;
            }
            "--public-context" => {
                if public_context {
                    return Err("--public-context can be given only once".into());
                }
                public_context = true;
            }
            "--public-character-context" => {
                if public_character_context {
                    return Err("--public-character-context can be given only once".into());
                }
                public_character_context = true;
            }
            "--personal-cache" => {
                if personal_cache {
                    return Err("--personal-cache can be given only once".into());
                }
                personal_cache = true;
            }
            "--personal-pair-cache" => {
                if personal_pair_cache {
                    return Err("--personal-pair-cache can be given only once".into());
                }
                personal_pair_cache = true;
            }
            "--health-only" => {
                if health_only {
                    return Err("--health-only can be given only once".into());
                }
                health_only = true;
            }
            "--help" | "-h"
                if history_inputs.is_empty()
                    && inputs.is_empty()
                    && window_gap_ms.is_none()
                    && !compact
                    && !public_context
                    && !public_character_context
                    && !personal_cache
                    && !personal_pair_cache
                    && !health_only
                    && arguments.next().is_none() =>
            {
                return Ok(Options::Help);
            }
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    if inputs.is_empty() {
        return Err(
            "at least one explicit evaluation --input path or --session id is required".into(),
        );
    }
    if public_context && window_gap_ms.is_none() {
        return Err("--public-context requires --window-gap-ms".into());
    }
    if public_character_context && window_gap_ms.is_none() {
        return Err("--public-character-context requires --window-gap-ms".into());
    }
    if personal_cache && window_gap_ms.is_none() {
        return Err("--personal-cache requires --window-gap-ms".into());
    }
    if personal_pair_cache && window_gap_ms.is_none() {
        return Err("--personal-pair-cache requires --window-gap-ms".into());
    }
    if usize::from(public_context)
        + usize::from(public_character_context)
        + usize::from(personal_cache)
        + usize::from(personal_pair_cache)
        > 1
    {
        return Err(
            "--public-context, --public-character-context, --personal-cache, and \
             --personal-pair-cache are mutually exclusive"
                .into(),
        );
    }
    if !history_inputs.is_empty() && !personal_cache && !personal_pair_cache {
        return Err(
            "--history-input/--history-session requires --personal-cache or \
             --personal-pair-cache"
                .into(),
        );
    }
    if health_only
        && (!history_inputs.is_empty()
            || window_gap_ms.is_some()
            || compact
            || public_context
            || public_character_context
            || personal_cache
            || personal_pair_cache)
    {
        return Err(
            "--health-only cannot be combined with history, decoding, cache, or compact options"
                .into(),
        );
    }
    Ok(Options::Inputs {
        history_inputs,
        inputs,
        window_gap_ms,
        compact,
        public_context,
        public_character_context,
        personal_cache,
        personal_pair_cache,
        health_only,
    })
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --bin capsule-replay -- \\\n         [--history-input <OLDER.zic|OLDER.zcs>|--history-session <OLDER_SESSION> ...] \\\n         [--input <FILE.zic|FILE.zcs>|--session <SESSION>] ... \\\n         [--window-gap-ms <POSITIVE_MS> \\\n          [--public-context|--public-character-context|--personal-cache|\
           --personal-pair-cache]] [--compact] [--health-only]"
    );
    eprintln!(
        "Reads explicitly named private .zic capsules or current-user-protected .zcs segments, \
         prints redacted aggregates, and writes nothing."
    );
    eprintln!(
        "--session expands only contiguous, predictably named segments for that explicit id; \
         it does not scan the private directory"
    );
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CaptureHealthReport {
    capsules: u64,
    events: u64,
    commits: u64,
    revisions: u64,
    keys_complete_records: u64,
    keys_incomplete_records: u64,
    logical_key_actions: u64,
    commits_with_internal_edit_keys: u64,
    revisions_delete_only: u64,
    revisions_insert_only: u64,
    revisions_replace: u64,
    revisions_multi_character: u64,
    revisions_with_navigation_keys: u64,
    revisions_with_selection_keys: u64,
    ambiguous_positions: u64,
}

impl CaptureHealthReport {
    fn from_capsules(capsules: &[EventCapsuleV1]) -> Self {
        let mut report = Self {
            capsules: u64::try_from(capsules.len()).unwrap_or(u64::MAX),
            ..Self::default()
        };
        for capsule in capsules {
            for event in capsule.events() {
                report.events = report.events.saturating_add(1);
                let (keys, keys_complete, position) = match &event.output {
                    TrackerOutput::Commit(commit) => {
                        report.commits = report.commits.saturating_add(1);
                        if commit
                            .keys
                            .iter()
                            .any(|key| matches!(key, RawKey::Backspace | RawKey::Delete))
                        {
                            report.commits_with_internal_edit_keys =
                                report.commits_with_internal_edit_keys.saturating_add(1);
                        }
                        (
                            commit.keys.as_slice(),
                            commit.keys_complete,
                            commit.document_change.position_evidence,
                        )
                    }
                    TrackerOutput::Revision(revision) => {
                        report.revisions = report.revisions.saturating_add(1);
                        match (
                            revision.change.deleted.is_empty(),
                            revision.change.inserted.is_empty(),
                        ) {
                            (false, true) => {
                                report.revisions_delete_only =
                                    report.revisions_delete_only.saturating_add(1);
                            }
                            (true, false) => {
                                report.revisions_insert_only =
                                    report.revisions_insert_only.saturating_add(1);
                            }
                            (false, false) => {
                                report.revisions_replace =
                                    report.revisions_replace.saturating_add(1);
                            }
                            (true, true) => {}
                        }
                        if revision.change.deleted.chars().count() > 1
                            || revision.change.inserted.chars().count() > 1
                        {
                            report.revisions_multi_character =
                                report.revisions_multi_character.saturating_add(1);
                        }
                        if revision.keys.iter().any(is_navigation_key) {
                            report.revisions_with_navigation_keys =
                                report.revisions_with_navigation_keys.saturating_add(1);
                        }
                        if revision.keys.iter().any(is_selection_key) {
                            report.revisions_with_selection_keys =
                                report.revisions_with_selection_keys.saturating_add(1);
                        }
                        (
                            revision.keys.as_slice(),
                            revision.keys_complete,
                            revision.change.position_evidence,
                        )
                    }
                };
                report.logical_key_actions = report
                    .logical_key_actions
                    .saturating_add(u64::try_from(keys.len()).unwrap_or(u64::MAX));
                if keys_complete {
                    report.keys_complete_records = report.keys_complete_records.saturating_add(1);
                } else {
                    report.keys_incomplete_records =
                        report.keys_incomplete_records.saturating_add(1);
                }
                if position == DeltaPositionEvidence::Ambiguous {
                    report.ambiguous_positions = report.ambiguous_positions.saturating_add(1);
                }
            }
        }
        report
    }

    fn terminal_line(&self) -> String {
        format!(
            "CAPTURE_HEALTH contains_text=false capsules={} events={} commits={} revisions={} \
             keys_complete_records={} keys_incomplete_records={} logical_key_actions={} \
             commits_with_internal_edit_keys={} revisions_delete_only={} \
             revisions_insert_only={} revisions_replace={} revisions_multi_character={} \
             revisions_with_navigation_keys={} revisions_with_selection_keys={} \
             ambiguous_positions={}",
            self.capsules,
            self.events,
            self.commits,
            self.revisions,
            self.keys_complete_records,
            self.keys_incomplete_records,
            self.logical_key_actions,
            self.commits_with_internal_edit_keys,
            self.revisions_delete_only,
            self.revisions_insert_only,
            self.revisions_replace,
            self.revisions_multi_character,
            self.revisions_with_navigation_keys,
            self.revisions_with_selection_keys,
            self.ambiguous_positions
        )
    }
}

fn is_navigation_key(key: &RawKey) -> bool {
    matches!(
        key,
        RawKey::Left
            | RawKey::Right
            | RawKey::Up
            | RawKey::Down
            | RawKey::Home
            | RawKey::End
            | RawKey::Shift(_)
    )
}

fn is_selection_key(key: &RawKey) -> bool {
    matches!(
        key,
        RawKey::Shift(inner)
            if matches!(
                inner.as_ref(),
                RawKey::Left
                    | RawKey::Right
                    | RawKey::Up
                    | RawKey::Down
                    | RawKey::Home
                    | RawKey::End
            )
    )
}

fn expand_input_selectors(
    manifest_dir: &Path,
    selectors: &[InputSelector],
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut inputs = Vec::new();
    for selector in selectors {
        match selector {
            InputSelector::Path(path) => inputs.push(path.clone()),
            InputSelector::Session(session_id) => {
                inputs.extend(expand_session_selector(manifest_dir, session_id)?);
            }
        }
    }
    Ok(inputs)
}

fn expand_session_selector(
    manifest_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    validate_session_selector(session_id)?;
    let root = manifest_dir.join("data/private/continuous-capture");
    validate_private_root(
        manifest_dir,
        &root,
        &["data", "private", "continuous-capture"],
        "protected segment",
    )?;
    let mut paths = Vec::new();
    for sequence in 0..MAX_SESSION_SEGMENTS {
        // Keep the syntactic path rooted at the manifest directory here. On
        // Windows, fs::canonicalize may add a `\\?\` verbatim prefix. Passing
        // that spelling into the later fixed-directory check would make the
        // same directory compare unequal to its ordinary `D:\...` spelling.
        // read_explicit_capsules still canonicalizes and verifies every file
        // immediately before opening it.
        let path = root.join(format!("segment-{session_id}-{sequence:08}.zcs"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "session segment cannot be a symbolic link: {}",
                    path.display()
                )
                .into());
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(
                    format!("session segment must be a regular file: {}", path.display()).into(),
                );
            }
            Ok(_) => paths.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && sequence == 0 => {
                return Err(
                    format!("session has no initial protected segment: {session_id}").into(),
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
            Err(error) => {
                return Err(format!(
                    "cannot inspect predictable session segment {}: {error}",
                    path.display()
                )
                .into());
            }
        }
    }
    Err(
        format!("session exceeds the {MAX_SESSION_SEGMENTS}-segment safety limit: {session_id}")
            .into(),
    )
}

fn validate_session_selector(value: &str) -> Result<(), Box<dyn std::error::Error>> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("session id must be 1-80 ASCII letters, digits, or hyphens".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateInputFormat {
    PlainCapsule,
    ProtectedSegment,
}

fn read_explicit_capsules(
    manifest_dir: &Path,
    requested_inputs: &[PathBuf],
) -> Result<Vec<EventCapsuleV1>, Box<dyn std::error::Error>> {
    let capsule_root = manifest_dir.join("data/private/event-capsules");
    let protected_root = manifest_dir.join("data/private/continuous-capture");
    let mut canonical_capsule_root = None;
    let mut canonical_protected_root = None;
    let mut seen = HashSet::new();
    let mut capsules = Vec::with_capacity(requested_inputs.len());

    for requested in requested_inputs {
        let (target, format) =
            resolve_private_input(manifest_dir, &capsule_root, &protected_root, requested)?;
        let canonical_root = match format {
            PrivateInputFormat::PlainCapsule => {
                if canonical_capsule_root.is_none() {
                    canonical_capsule_root =
                        Some(validate_capsule_root(manifest_dir, &capsule_root)?);
                }
                canonical_capsule_root.as_ref().expect("initialized above")
            }
            PrivateInputFormat::ProtectedSegment => {
                if canonical_protected_root.is_none() {
                    canonical_protected_root = Some(validate_private_root(
                        manifest_dir,
                        &protected_root,
                        &["data", "private", "continuous-capture"],
                        "protected segment",
                    )?);
                }
                canonical_protected_root
                    .as_ref()
                    .expect("initialized above")
            }
        };
        let metadata = fs::symlink_metadata(&target).map_err(|error| {
            format!(
                "cannot inspect explicitly named private input {}: {error}",
                target.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "private input cannot be a symbolic link: {}",
                target.display()
            )
            .into());
        }
        if !metadata.is_file() {
            return Err(
                format!("private input must be a regular file: {}", target.display()).into(),
            );
        }

        let canonical_target = fs::canonicalize(&target)?;
        if canonical_target.parent() != Some(canonical_root.as_path()) {
            return Err(format!(
                "private input resolves outside its fixed private directory: {}",
                target.display()
            )
            .into());
        }
        if !seen.insert(canonical_target.clone()) {
            return Err(format!(
                "duplicate capsule input is not allowed: {}",
                target.display()
            )
            .into());
        }

        let mut file = File::open(&canonical_target)?;
        let opened_metadata = file.metadata()?;
        if !opened_metadata.is_file() {
            return Err(format!(
                "private input changed before it could be read: {}",
                target.display()
            )
            .into());
        }
        if opened_metadata.len() > MAX_CAPSULE_FILE_BYTES {
            return Err(format!(
                "private input exceeds the {MAX_CAPSULE_FILE_BYTES}-byte limit: {}",
                target.display()
            )
            .into());
        }
        let mut encoded = Vec::new();
        file.by_ref()
            .take(MAX_CAPSULE_FILE_BYTES + 1)
            .read_to_end(&mut encoded)
            .map_err(|error| {
                format!(
                    "cannot read explicitly named private input {}: {error}",
                    target.display()
                )
            })?;
        if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CAPSULE_FILE_BYTES {
            return Err(format!(
                "private input exceeds the {MAX_CAPSULE_FILE_BYTES}-byte limit: {}",
                target.display()
            )
            .into());
        }
        let capsule = decode_private_input(format, &encoded, &target)?;
        capsules.push(capsule);
    }
    Ok(capsules)
}

fn decode_private_input(
    format: PrivateInputFormat,
    encoded: &[u8],
    target: &Path,
) -> Result<EventCapsuleV1, Box<dyn std::error::Error>> {
    match format {
        PrivateInputFormat::PlainCapsule => {
            let text = std::str::from_utf8(encoded).map_err(|error| {
                format!(
                    "cannot read explicitly named capsule {} as UTF-8: {error}",
                    target.display()
                )
            })?;
            EventCapsuleV1::from_text(text)
                .map_err(|error| {
                    format!(
                        "invalid private event capsule {}: {error}",
                        target.display()
                    )
                })
                .map_err(Into::into)
        }
        PrivateInputFormat::ProtectedSegment => decode_protected_segment(encoded, target),
    }
}

#[cfg(windows)]
fn decode_protected_segment(
    encoded: &[u8],
    target: &Path,
) -> Result<EventCapsuleV1, Box<dyn std::error::Error>> {
    let envelope = ProtectedSegmentEnvelopeV1::from_bytes(encoded).map_err(|error| {
        format!(
            "invalid protected segment envelope {}: {error}",
            target.display()
        )
    })?;
    let mut plaintext = WindowsUserDataProtector
        .unprotect(envelope.protected())
        .map_err(|error| {
            format!(
                "cannot unprotect segment for the current Windows user {}: {error}",
                target.display()
            )
        })?;
    if plaintext.len() > MAX_CAPSULE_FILE_BYTES as usize {
        return Err(format!(
            "unprotected segment exceeds the {MAX_CAPSULE_FILE_BYTES}-byte limit: {}",
            target.display()
        )
        .into());
    }
    let decoded = ContinuousSegmentV1::from_plaintext(&plaintext).map_err(|error| {
        format!(
            "invalid unprotected continuous segment {}: {error}",
            target.display()
        )
    });
    plaintext.fill(0);
    let segment = decoded?;
    if segment.capture_profile() != CODEX_CAPTURE_PROFILE_V1 {
        return Err(format!(
            "unsupported protected segment capture profile {:?} in {}",
            segment.capture_profile(),
            target.display()
        )
        .into());
    }
    let expected_name = format!(
        "segment-{}-{:08}.zcs",
        segment.session_id(),
        segment.sequence()
    );
    if target.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(format!(
            "protected segment file name does not match encrypted session metadata: {}",
            target.display()
        )
        .into());
    }
    Ok(segment.into_capsule())
}

#[cfg(not(windows))]
fn decode_protected_segment(
    _encoded: &[u8],
    target: &Path,
) -> Result<EventCapsuleV1, Box<dyn std::error::Error>> {
    Err(format!(
        "protected segment requires the same Windows user account that created it: {}",
        target.display()
    )
    .into())
}

fn validate_capsule_root(
    manifest_dir: &Path,
    capsule_root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    validate_private_root(
        manifest_dir,
        capsule_root,
        &["data", "private", "event-capsules"],
        "capsule",
    )
}

fn validate_private_root(
    manifest_dir: &Path,
    private_root: &Path,
    components: &[&str],
    description: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest_metadata = fs::symlink_metadata(manifest_dir)?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_dir() {
        return Err("repository root must be a real directory, not a symbolic link".into());
    }

    let mut current = manifest_dir.to_path_buf();
    for component in components {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "private {description} directory is unavailable at {}: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "private {description} directory contains a symbolic-link component: {}",
                current.display()
            )
            .into());
        }
        if !metadata.is_dir() {
            return Err(format!(
                "private {description} directory component is not a directory: {}",
                current.display()
            )
            .into());
        }
    }

    let canonical_manifest = fs::canonicalize(manifest_dir)?;
    let canonical_root = fs::canonicalize(private_root)?;
    if !canonical_root.starts_with(&canonical_manifest) {
        return Err(
            format!("private {description} directory resolves outside the repository").into(),
        );
    }
    Ok(canonical_root)
}

fn resolve_private_input(
    manifest_dir: &Path,
    capsule_root: &Path,
    protected_root: &Path,
    requested: &Path,
) -> Result<(PathBuf, PrivateInputFormat), Box<dyn std::error::Error>> {
    match requested
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("zic") => Ok((
            resolve_capsule_input(manifest_dir, capsule_root, requested)?,
            PrivateInputFormat::PlainCapsule,
        )),
        Some("zcs") => Ok((
            resolve_protected_input(manifest_dir, protected_root, requested)?,
            PrivateInputFormat::ProtectedSegment,
        )),
        _ => Err("private input must use the .zic or .zcs extension".into()),
    }
}

fn resolve_protected_input(
    manifest_dir: &Path,
    protected_root: &Path,
    requested: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if requested.as_os_str().is_empty() {
        return Err("protected segment input path cannot be empty".into());
    }
    let target = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        manifest_dir.join(requested)
    };
    if target.parent() != Some(protected_root) {
        return Err(
            "protected segment input must be directly inside data/private/continuous-capture"
                .into(),
        );
    }
    if target.extension().and_then(|extension| extension.to_str()) != Some("zcs") {
        return Err("protected segment input must use the .zcs extension".into());
    }
    let file_name = target
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .ok_or("protected segment file name must be valid Unicode")?;
    if file_name.starts_with('.') {
        return Err("protected segment file name cannot be hidden".into());
    }
    Ok(target)
}

fn resolve_capsule_input(
    manifest_dir: &Path,
    capsule_root: &Path,
    requested: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if requested.as_os_str().is_empty() {
        return Err("capsule input path cannot be empty".into());
    }
    let target = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        manifest_dir.join(requested)
    };
    if target.parent() != Some(capsule_root) {
        return Err("capsule input must be directly inside data/private/event-capsules".into());
    }
    if target.extension().and_then(|extension| extension.to_str()) != Some("zic") {
        return Err("capsule input must use the .zic extension".into());
    }
    let file_name = target
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .ok_or("capsule input file name must be valid Unicode")?;
    if file_name.starts_with('.') {
        return Err("capsule input file name cannot be hidden".into());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureHealthReport, InputSelector, MAX_CAPSULE_FILE_BYTES, Options,
        expand_session_selector, parse_options, read_explicit_capsules, resolve_capsule_input,
        resolve_protected_input,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(windows)]
    use ziranma_decoder::{
        CODEX_CAPTURE_PROFILE_V1, CaptureSessionKind, ContinuousSegmentMetadata,
        ContinuousSegmentV1, DataProtector, ProtectedSegmentEnvelopeV1, WindowsUserDataProtector,
    };
    use ziranma_decoder::{
        CommitRecord, DeltaPositionEvidence, EventCapsuleV1, RawKey, TextDelta, TimedTrackerOutput,
        TrackerOutput,
    };

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestWorkspace {
        manifest: PathBuf,
        capsule_root: PathBuf,
        protected_root: PathBuf,
        files: Vec<PathBuf>,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let manifest = std::env::temp_dir().join(format!(
                "ziranma-capsule-replay-test-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            let capsule_root = manifest.join("data/private/event-capsules");
            let protected_root = manifest.join("data/private/continuous-capture");
            fs::create_dir_all(&capsule_root).unwrap();
            fs::create_dir_all(&protected_root).unwrap();
            Self {
                manifest,
                capsule_root,
                protected_root,
                files: Vec::new(),
            }
        }

        fn write(&mut self, name: &str, contents: &str) -> PathBuf {
            let path = self.capsule_root.join(name);
            fs::write(&path, contents).unwrap();
            self.files.push(path.clone());
            path
        }

        fn relative(&self, name: &str) -> PathBuf {
            Path::new("data/private/event-capsules").join(name)
        }

        fn write_protected(&mut self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.protected_root.join(name);
            fs::write(&path, contents).unwrap();
            self.files.push(path.clone());
            path
        }

        #[cfg(windows)]
        fn relative_protected(&self, name: &str) -> PathBuf {
            Path::new("data/private/continuous-capture").join(name)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            for file in &self.files {
                let _ = fs::remove_file(file);
            }
            let _ = fs::remove_dir(&self.capsule_root);
            let _ = fs::remove_dir(&self.protected_root);
            let _ = fs::remove_dir(self.manifest.join("data/private"));
            let _ = fs::remove_dir(self.manifest.join("data"));
            let _ = fs::remove_dir(&self.manifest);
        }
    }

    fn capsule() -> EventCapsuleV1 {
        EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 100,
            output: TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space],
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "mao".to_owned(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "猫".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }),
        }])
        .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn explicitly_named_protected_segment_decrypts_without_plaintext_export() {
        let mut workspace = TestWorkspace::new();
        let expected = capsule();
        let metadata = ContinuousSegmentMetadata::new(
            "test-1".to_owned(),
            0,
            10,
            20,
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            CODEX_CAPTURE_PROFILE_V1.to_owned(),
        )
        .unwrap();
        let segment = ContinuousSegmentV1::new(metadata, expected.clone()).unwrap();
        let protected = WindowsUserDataProtector
            .protect(&segment.to_plaintext().unwrap())
            .unwrap();
        let bytes = ProtectedSegmentEnvelopeV1::new(protected)
            .unwrap()
            .to_bytes()
            .unwrap();
        workspace.write_protected("segment-test-1-00000000.zcs", &bytes);
        assert!(!String::from_utf8_lossy(&bytes).contains('猫'));

        let loaded = read_explicit_capsules(
            &workspace.manifest,
            &[workspace.relative_protected("segment-test-1-00000000.zcs")],
        )
        .unwrap();
        assert_eq!(loaded, vec![expected]);
    }

    #[test]
    fn protected_input_is_restricted_to_the_fixed_private_directory() {
        let manifest = Path::new(r"D:\repo");
        let root = manifest.join("data/private/continuous-capture");
        assert_eq!(
            resolve_protected_input(
                manifest,
                &root,
                Path::new("data/private/continuous-capture/segment-test-1-00000000.zcs"),
            )
            .unwrap(),
            root.join("segment-test-1-00000000.zcs")
        );
        for invalid in [
            "segment.zcs",
            "data/private/segment.zcs",
            "data/private/continuous-capture/nested/segment.zcs",
            "data/private/continuous-capture/segment.zic",
            "data/private/continuous-capture/.hidden.zcs",
        ] {
            assert!(
                resolve_protected_input(manifest, &root, Path::new(invalid)).is_err(),
                "{invalid} must be rejected"
            );
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
            "data/private/event-capsules/a.zic".to_owned(),
            "--input".to_owned(),
            "data/private/event-capsules/b.zic".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            history_inputs,
            inputs,
            window_gap_ms,
            compact,
            public_context,
            public_character_context,
            personal_cache,
            personal_pair_cache,
            health_only,
        } = options
        else {
            panic!("expected inputs");
        };
        assert!(history_inputs.is_empty());
        assert_eq!(inputs.len(), 2);
        assert_eq!(window_gap_ms, None);
        assert!(!compact);
        assert!(!public_context);
        assert!(!public_character_context);
        assert!(!personal_cache);
        assert!(!personal_pair_cache);
        assert!(!health_only);

        let by_session = parse_options(vec!["--session".to_owned(), "1234-77".to_owned()]).unwrap();
        let Options::Inputs { inputs, .. } = by_session else {
            panic!("expected inputs");
        };
        assert_eq!(inputs, vec![InputSelector::Session("1234-77".to_owned())]);

        let health = parse_options(vec![
            "--session".to_owned(),
            "1234-77".to_owned(),
            "--health-only".to_owned(),
        ])
        .unwrap();
        let Options::Inputs { health_only, .. } = health else {
            panic!("expected inputs");
        };
        assert!(health_only);
        assert!(
            parse_options(vec![
                "--session".to_owned(),
                "1234-77".to_owned(),
                "--health-only".to_owned(),
                "--compact".to_owned(),
            ])
            .is_err()
        );

        let with_window = parse_options(vec![
            "--input".to_owned(),
            "data/private/event-capsules/a.zic".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--compact".to_owned(),
            "--public-context".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            window_gap_ms,
            compact,
            public_context,
            public_character_context,
            personal_cache,
            ..
        } = with_window
        else {
            panic!("expected inputs");
        };
        assert_eq!(window_gap_ms, Some(5_000));
        assert!(compact);
        assert!(public_context);
        assert!(!public_character_context);
        assert!(!personal_cache);
        let with_character_context = parse_options(vec![
            "--input".to_owned(),
            "data/private/event-capsules/a.zic".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--public-character-context".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            public_context,
            public_character_context,
            ..
        } = with_character_context
        else {
            panic!("expected inputs");
        };
        assert!(!public_context);
        assert!(public_character_context);
        let with_personal_cache = parse_options(vec![
            "--input".to_owned(),
            "data/private/event-capsules/a.zic".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--personal-cache".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            public_context,
            public_character_context,
            personal_cache,
            ..
        } = with_personal_cache
        else {
            panic!("expected inputs");
        };
        assert!(!public_context);
        assert!(!public_character_context);
        assert!(personal_cache);
        let with_personal_pair_cache = parse_options(vec![
            "--input".to_owned(),
            "data/private/event-capsules/a.zic".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--personal-pair-cache".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            public_context,
            public_character_context,
            personal_cache,
            personal_pair_cache,
            ..
        } = with_personal_pair_cache
        else {
            panic!("expected inputs");
        };
        assert!(!public_context);
        assert!(!public_character_context);
        assert!(!personal_cache);
        assert!(personal_pair_cache);
        let with_history_session = parse_options(vec![
            "--history-session".to_owned(),
            "1000-1".to_owned(),
            "--session".to_owned(),
            "2000-2".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--personal-pair-cache".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            history_inputs,
            inputs,
            ..
        } = with_history_session
        else {
            panic!("expected inputs");
        };
        assert_eq!(
            history_inputs,
            vec![InputSelector::Session("1000-1".to_owned())]
        );
        assert_eq!(inputs, vec![InputSelector::Session("2000-2".to_owned())]);
        let with_history = parse_options(vec![
            "--history-input".to_owned(),
            "data/private/event-capsules/older.zic".to_owned(),
            "--input".to_owned(),
            "data/private/event-capsules/evaluation.zic".to_owned(),
            "--window-gap-ms".to_owned(),
            "5000".to_owned(),
            "--personal-pair-cache".to_owned(),
        ])
        .unwrap();
        let Options::Inputs {
            history_inputs,
            inputs,
            personal_pair_cache,
            ..
        } = with_history
        else {
            panic!("expected inputs");
        };
        assert_eq!(history_inputs.len(), 1);
        assert_eq!(inputs.len(), 1);
        assert!(personal_pair_cache);
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "0".to_owned(),
            ])
            .is_err()
        );
        assert!(parse_options(vec!["--session".to_owned(), "../escape".to_owned()]).is_err());
        assert!(
            parse_options(vec![
                "--history-input".to_owned(),
                "data/private/event-capsules/older.zic".to_owned(),
                "--input".to_owned(),
                "data/private/event-capsules/evaluation.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--personal-pair-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--personal-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--personal-pair-cache".to_owned(),
                "--personal-pair-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--personal-cache".to_owned(),
                "--personal-pair-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--public-character-context".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--personal-cache".to_owned(),
                "--personal-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--public-context".to_owned(),
                "--personal-cache".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--public-context".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--public-character-context".to_owned(),
                "--public-character-context".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--public-context".to_owned(),
                "--public-character-context".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--window-gap-ms".to_owned(),
                "5000".to_owned(),
                "--public-context".to_owned(),
                "--public-context".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options(vec![
                "--input".to_owned(),
                "data/private/event-capsules/a.zic".to_owned(),
                "--compact".to_owned(),
                "--compact".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn health_report_is_fast_shape_only_and_never_echoes_text() {
        let report = CaptureHealthReport::from_capsules(&[capsule()]);
        assert_eq!(report.capsules, 1);
        assert_eq!(report.events, 1);
        assert_eq!(report.commits, 1);
        assert_eq!(report.revisions, 0);
        assert_eq!(report.logical_key_actions, 3);
        let line = report.terminal_line();
        assert!(line.starts_with("CAPTURE_HEALTH contains_text=false"));
        assert!(!line.contains('猫'));
        assert!(!line.contains("mao"));
    }

    #[test]
    fn session_selector_expands_only_contiguous_predictable_names() {
        let mut workspace = TestWorkspace::new();
        workspace.write_protected("segment-1234-77-00000000.zcs", b"zero");
        workspace.write_protected("segment-1234-77-00000001.zcs", b"one");
        workspace.write_protected("segment-1234-77-00000003.zcs", b"after-gap");
        workspace.write_protected("segment-other-00000000.zcs", b"other");

        let expanded = expand_session_selector(&workspace.manifest, "1234-77").unwrap();
        assert_eq!(expanded.len(), 2);
        assert_eq!(
            expanded[0].parent(),
            Some(workspace.protected_root.as_path())
        );
        assert!(expanded[0].ends_with("segment-1234-77-00000000.zcs"));
        assert!(expanded[1].ends_with("segment-1234-77-00000001.zcs"));
        assert!(expand_session_selector(&workspace.manifest, "missing-1").is_err());
    }

    #[test]
    fn inputs_are_restricted_to_direct_nonhidden_zic_children() {
        let manifest = Path::new(r"D:\repo");
        let root = manifest.join("data/private/event-capsules");
        assert_eq!(
            resolve_capsule_input(
                manifest,
                &root,
                Path::new("data/private/event-capsules/run-001.zic")
            )
            .unwrap(),
            root.join("run-001.zic")
        );
        for invalid in [
            "run.zic",
            "data/private/run.zic",
            "data/private/event-capsules/nested/run.zic",
            "data/private/event-capsules/run.json",
            "data/private/event-capsules/.hidden.zic",
        ] {
            assert!(
                resolve_capsule_input(manifest, &root, Path::new(invalid)).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn only_named_capsules_are_read() {
        let mut workspace = TestWorkspace::new();
        workspace.write("first.zic", &capsule().to_text().unwrap());
        workspace.write("unlisted-malformed.zic", "PRIVATE_MARKER");
        let loaded =
            read_explicit_capsules(&workspace.manifest, &[workspace.relative("first.zic")])
                .unwrap();
        assert_eq!(loaded, vec![capsule()]);
    }

    #[test]
    fn duplicate_malformed_and_oversized_inputs_are_rejected_without_content_echo() {
        let mut workspace = TestWorkspace::new();
        let good = workspace.write("good.zic", &capsule().to_text().unwrap());
        let duplicate_error =
            read_explicit_capsules(&workspace.manifest, &[workspace.relative("good.zic"), good])
                .unwrap_err();
        assert!(duplicate_error.to_string().contains("duplicate"));

        let private_marker = "DO_NOT_ECHO_PRIVATE_MARKER";
        workspace.write("bad.zic", private_marker);
        let malformed_error =
            read_explicit_capsules(&workspace.manifest, &[workspace.relative("bad.zic")])
                .unwrap_err();
        assert!(!malformed_error.to_string().contains(private_marker));

        let oversized = "0".repeat(usize::try_from(MAX_CAPSULE_FILE_BYTES + 1).unwrap());
        workspace.write("oversized.zic", &oversized);
        let oversized_error =
            read_explicit_capsules(&workspace.manifest, &[workspace.relative("oversized.zic")])
                .unwrap_err();
        assert!(oversized_error.to_string().contains("exceeds"));
    }
}
