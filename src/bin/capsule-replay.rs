//! Explicit, read-only replay of private event capsules against public data.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use ziranma_decoder::{
    BigramLanguageModel, CAPTURE_INTEGRITY_SCHEMA_V1, CapsuleReplayReport, CaptureIntegrityV1,
    CaptureSessionKind, CharacterBigramLanguageModel, ContinuousSegmentMetadata, Decoder,
    DeltaPositionEvidence, EventCapsuleV1, PersonalCacheReplayState, RawKey, SegmentCloseReason,
    TrackerOutput, parse_rime_lexicon, parse_ud_conllu, select_public_bigram_training_sequences,
};
#[cfg(windows)]
use ziranma_decoder::{
    CODEX_CAPTURE_PROFILE_V1, CODEX_CAPTURE_PROFILE_V2, DataProtector, DecodedContinuousSegment,
    ProtectedSegmentEnvelopeV1, WindowsUserDataProtector,
};

const PUBLIC_RIME_LEXICON: &str =
    include_str!("../../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
const PUBLIC_UD_TRAIN: &str =
    include_str!("../../data/public/ud-chinese-gsdsimp/zh_gsdsimp-ud-train.conllu");
const MAX_CAPSULE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_SEGMENTS: u64 = 1_000_000;
const CAPTURE_HEALTH_REPORT_SCHEMA_V1: &str = "ziranma-capture-health-report-v1";
const CAPTURE_INTEGRITY_REPORT_SCHEMA_V1: &str = "ziranma-capture-integrity-report-v1";
const CAPTURE_HEALTH_PRIVACY_NOTICE: &str = "--health-only prints CAPTURE_HEALTH and CAPTURE_INTEGRITY; both contain behavioral \
     metadata; legacy v1/.zic integrity is unavailable, never zero-filled.";

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
            let history_inputs =
                expand_input_selectors(manifest_dir, &history_inputs).map_err(|_| {
                    RedactedPrivateSelectorError {
                        phase: ReplayInputPhase::History,
                    }
                })?;
            let inputs = expand_input_selectors(manifest_dir, &inputs).map_err(|_| {
                RedactedPrivateSelectorError {
                    phase: ReplayInputPhase::Evaluation,
                }
            })?;
            let mut loader = PrivateInputLoader::new(manifest_dir);
            let mut metadata_guard =
                SegmentMetadataGuard::new(personal_cache || personal_pair_cache);
            if health_only {
                let mut health = CaptureHealthReport::default();
                visit_private_loaded_inputs(
                    &mut loader,
                    &mut metadata_guard,
                    &inputs,
                    ReplayInputPhase::Evaluation,
                    |loaded| {
                        health.observe_loaded(loaded);
                        Ok(())
                    },
                )?;
                println!("{}", health.terminal_line());
                println!("{}", health.integrity_terminal_line());
                return Ok(());
            }
            let imported = parse_rime_lexicon(PUBLIC_RIME_LEXICON)?;
            let entries = imported.entries;
            let decoder = Decoder::new(entries.clone());
            let mut report = CapsuleReplayReport::with_window_gap_limit(window_gap_ms)?;
            if personal_cache || personal_pair_cache {
                let mut state = PersonalCacheReplayState::new();
                if !history_inputs.is_empty() {
                    let mut history_report =
                        CapsuleReplayReport::with_window_gap_limit(window_gap_ms)?;
                    visit_private_inputs(
                        &mut loader,
                        &mut metadata_guard,
                        &history_inputs,
                        ReplayInputPhase::History,
                        |capsule| {
                            if personal_pair_cache {
                                redact_private_analysis_error(
                                    history_report.observe_capsule_with_personal_pair_cache(
                                        &decoder, &mut state, capsule,
                                    ),
                                )?;
                            } else {
                                redact_private_analysis_error(
                                    history_report.observe_capsule_with_personal_cache(
                                        &decoder, &mut state, capsule,
                                    ),
                                )?;
                            }
                            Ok(())
                        },
                    )?;
                    report.record_personal_cache_history(&history_report, &state);
                }
                visit_private_inputs(
                    &mut loader,
                    &mut metadata_guard,
                    &inputs,
                    ReplayInputPhase::Evaluation,
                    |capsule| {
                        if personal_pair_cache {
                            redact_private_analysis_error(
                                report.observe_capsule_with_personal_pair_cache(
                                    &decoder, &mut state, capsule,
                                ),
                            )?;
                        } else {
                            redact_private_analysis_error(
                                report.observe_capsule_with_personal_cache(
                                    &decoder, &mut state, capsule,
                                ),
                            )?;
                        }
                        Ok(())
                    },
                )?;
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
                    visit_private_inputs(
                        &mut loader,
                        &mut metadata_guard,
                        &inputs,
                        ReplayInputPhase::Evaluation,
                        |capsule| {
                            redact_private_analysis_error(
                                report.observe_capsule_with_public_context(
                                    &decoder,
                                    &language_model,
                                    log_frequency_total,
                                    capsule,
                                ),
                            )?;
                            Ok(())
                        },
                    )?;
                } else {
                    let text_sequences = training
                        .sequences
                        .iter()
                        .map(|sequence| sequence.concat())
                        .collect::<Vec<_>>();
                    let language_model =
                        CharacterBigramLanguageModel::from_text_sequences(&text_sequences)?;
                    visit_private_inputs(
                        &mut loader,
                        &mut metadata_guard,
                        &inputs,
                        ReplayInputPhase::Evaluation,
                        |capsule| {
                            redact_private_analysis_error(
                                report.observe_capsule_with_public_character_context(
                                    &decoder,
                                    &language_model,
                                    capsule,
                                ),
                            )?;
                            Ok(())
                        },
                    )?;
                }
            } else {
                visit_private_inputs(
                    &mut loader,
                    &mut metadata_guard,
                    &inputs,
                    ReplayInputPhase::Evaluation,
                    |capsule| {
                        redact_private_analysis_error(report.observe_capsule(&decoder, capsule))?;
                        Ok(())
                    },
                )?;
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
            _ => return Err("unknown argument; value was suppressed".into()),
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
        "Usage (health): cargo run --bin capsule-replay -- \\\n         <--input <FILE.zic|FILE.zcs>|--session <SESSION>> ... --health-only"
    );
    eprintln!(
        "Usage (replay): cargo run --bin capsule-replay -- \\\n         [--history-input <OLDER.zic|OLDER.zcs>|--history-session <OLDER_SESSION> ...] \\\n         <--input <FILE.zic|FILE.zcs>|--session <SESSION>> ... \\\n         [--window-gap-ms <POSITIVE_MS> \\\n          [--public-context|--public-character-context|--personal-cache|\
           --personal-pair-cache]] [--compact]"
    );
    eprintln!(
        "Reads explicitly named private .zic capsules or current-user-protected .zcs segments, \
         prints redacted aggregates, and writes nothing."
    );
    eprintln!(
        "--session expands only contiguous, predictably named segments for that explicit id; \
         it does not scan the private directory"
    );
    eprintln!(
        "History selectors require --window-gap-ms plus --personal-cache or \
         --personal-pair-cache."
    );
    eprintln!("{CAPTURE_HEALTH_PRIVACY_NOTICE}");
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
    integrity_available_segments: u64,
    legacy_inputs_without_integrity: u64,
    last_integrity_epoch_by_session: HashMap<String, u64>,
    baseline_epochs_observed: u64,
    close_capacity: u64,
    close_timer: u64,
    close_continuity: u64,
    close_session_end: u64,
    integrity_counters: ziranma_decoder::CaptureIntegrityCountersV1,
}

impl CaptureHealthReport {
    fn observe_capsule(&mut self, capsule: &EventCapsuleV1) {
        self.capsules = self.capsules.saturating_add(1);
        for event in capsule.events() {
            self.events = self.events.saturating_add(1);
            let (keys, keys_complete, position) = match &event.output {
                TrackerOutput::Commit(commit) => {
                    self.commits = self.commits.saturating_add(1);
                    if commit
                        .keys
                        .iter()
                        .any(|key| matches!(key, RawKey::Backspace | RawKey::Delete))
                    {
                        self.commits_with_internal_edit_keys =
                            self.commits_with_internal_edit_keys.saturating_add(1);
                    }
                    (
                        commit.keys.as_slice(),
                        commit.keys_complete,
                        commit.document_change.position_evidence,
                    )
                }
                TrackerOutput::Revision(revision) => {
                    self.revisions = self.revisions.saturating_add(1);
                    match (
                        revision.change.deleted.is_empty(),
                        revision.change.inserted.is_empty(),
                    ) {
                        (false, true) => {
                            self.revisions_delete_only =
                                self.revisions_delete_only.saturating_add(1);
                        }
                        (true, false) => {
                            self.revisions_insert_only =
                                self.revisions_insert_only.saturating_add(1);
                        }
                        (false, false) => {
                            self.revisions_replace = self.revisions_replace.saturating_add(1);
                        }
                        (true, true) => {}
                    }
                    if revision.change.deleted.chars().count() > 1
                        || revision.change.inserted.chars().count() > 1
                    {
                        self.revisions_multi_character =
                            self.revisions_multi_character.saturating_add(1);
                    }
                    if revision.keys.iter().any(is_navigation_key) {
                        self.revisions_with_navigation_keys =
                            self.revisions_with_navigation_keys.saturating_add(1);
                    }
                    if revision.keys.iter().any(is_selection_key) {
                        self.revisions_with_selection_keys =
                            self.revisions_with_selection_keys.saturating_add(1);
                    }
                    (
                        revision.keys.as_slice(),
                        revision.keys_complete,
                        revision.change.position_evidence,
                    )
                }
            };
            self.logical_key_actions = self
                .logical_key_actions
                .saturating_add(u64::try_from(keys.len()).unwrap_or(u64::MAX));
            if keys_complete {
                self.keys_complete_records = self.keys_complete_records.saturating_add(1);
            } else {
                self.keys_incomplete_records = self.keys_incomplete_records.saturating_add(1);
            }
            if position == DeltaPositionEvidence::Ambiguous {
                self.ambiguous_positions = self.ambiguous_positions.saturating_add(1);
            }
        }
    }

    fn observe_loaded(&mut self, loaded: &LoadedPrivateInput) {
        self.observe_capsule(&loaded.capsule);
        let Some(integrity) = loaded.integrity.as_ref() else {
            self.legacy_inputs_without_integrity =
                self.legacy_inputs_without_integrity.saturating_add(1);
            return;
        };
        self.integrity_available_segments = self.integrity_available_segments.saturating_add(1);
        self.integrity_counters.accumulate(&integrity.counters);
        match integrity.close_reason {
            SegmentCloseReason::Capacity => {
                self.close_capacity = self.close_capacity.saturating_add(1)
            }
            SegmentCloseReason::Timer => self.close_timer = self.close_timer.saturating_add(1),
            SegmentCloseReason::Continuity => {
                self.close_continuity = self.close_continuity.saturating_add(1)
            }
            SegmentCloseReason::SessionEnd => {
                self.close_session_end = self.close_session_end.saturating_add(1)
            }
        }
        if let Some(metadata) = loaded.metadata.as_ref() {
            match self
                .last_integrity_epoch_by_session
                .get_mut(&metadata.session_id)
            {
                Some(previous) if *previous != integrity.baseline_epoch => {
                    *previous = integrity.baseline_epoch;
                    self.baseline_epochs_observed = self.baseline_epochs_observed.saturating_add(1);
                }
                Some(_) => {}
                None => {
                    self.last_integrity_epoch_by_session
                        .insert(metadata.session_id.clone(), integrity.baseline_epoch);
                    self.baseline_epochs_observed = self.baseline_epochs_observed.saturating_add(1);
                }
            }
        }
    }

    #[cfg(test)]
    fn from_capsules(capsules: &[EventCapsuleV1]) -> Self {
        let mut report = Self::default();
        for capsule in capsules {
            report.observe_capsule(capsule);
        }
        report
    }

    fn terminal_line(&self) -> String {
        format!(
            "CAPTURE_HEALTH contains_text=false contains_behavioral_metadata=true \
             report_schema={CAPTURE_HEALTH_REPORT_SCHEMA_V1} \
             capsules={} events={} commits={} revisions={} \
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

    fn integrity_terminal_line(&self) -> String {
        let counters = &self.integrity_counters;
        format!(
            "CAPTURE_INTEGRITY contains_text=false contains_behavioral_metadata=true \
             report_schema={CAPTURE_INTEGRITY_REPORT_SCHEMA_V1} \
             integrity_schema={CAPTURE_INTEGRITY_SCHEMA_V1} available_segments={} \
             legacy_inputs_without_integrity={} baseline_epochs_observed={} \
             close_capacity={} close_timer={} close_continuity={} close_session_end={} \
             key_actions_observed={} composition_callbacks_observed={} \
             composition_finalized_callbacks_observed={} value_callbacks_observed={} \
             value_read_errors={} composition_read_errors={} selection_read_errors={} \
             value_callbacks_without_output={} tracker_outputs_emitted={} \
             key_actions_not_emitted_at_boundary={} key_buffer_resets={} \
             counter_saturated={}",
            self.integrity_available_segments,
            self.legacy_inputs_without_integrity,
            self.baseline_epochs_observed,
            self.close_capacity,
            self.close_timer,
            self.close_continuity,
            self.close_session_end,
            counters.key_actions_observed,
            counters.composition_callbacks_observed,
            counters.composition_finalized_callbacks_observed,
            counters.value_callbacks_observed,
            counters.value_read_errors,
            counters.composition_read_errors,
            counters.selection_read_errors,
            counters.value_callbacks_without_output,
            counters.tracker_outputs_emitted,
            counters.key_actions_not_emitted_at_boundary,
            counters.key_buffer_resets,
            counters.counter_saturated,
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
        // PrivateInputLoader still canonicalizes and verifies every file
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplayInputPhase {
    History,
    Evaluation,
}

impl ReplayInputPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Evaluation => "evaluation",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RedactedPrivateSelectorError {
    phase: ReplayInputPhase,
}

impl std::fmt::Display for RedactedPrivateSelectorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "private {} selector could not be expanded; details were suppressed",
            self.phase.as_str()
        )
    }
}

impl std::fmt::Debug for RedactedPrivateSelectorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for RedactedPrivateSelectorError {}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RedactedPrivateAnalysisError;

impl std::fmt::Display for RedactedPrivateAnalysisError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "private replay analysis failed; input details were suppressed"
        )
    }
}

impl std::fmt::Debug for RedactedPrivateAnalysisError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for RedactedPrivateAnalysisError {}

fn redact_private_analysis_error<T, E>(
    result: Result<T, E>,
) -> Result<T, RedactedPrivateAnalysisError> {
    result.map_err(|_| RedactedPrivateAnalysisError)
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct RedactedPrivateInputError {
    phase: ReplayInputPhase,
    ordinal: usize,
    kind: PrivateInputLoadKind,
}

impl std::fmt::Display for RedactedPrivateInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "private {} input #{} could not be loaded: {}; path and content were suppressed",
            self.phase.as_str(),
            self.ordinal,
            self.kind.as_str()
        )
    }
}

impl std::fmt::Debug for RedactedPrivateInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for RedactedPrivateInputError {}

struct LoadedPrivateInput {
    capsule: EventCapsuleV1,
    metadata: Option<ContinuousSegmentMetadata>,
    integrity: Option<CaptureIntegrityV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateInputLoadKind {
    PathPolicy,
    UnsafeFile,
    Duplicate,
    Read,
    TooLarge,
    DecodeOrUnprotect,
}

impl PrivateInputLoadKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::PathPolicy => "path-policy",
            Self::UnsafeFile => "unsafe-file",
            Self::Duplicate => "duplicate",
            Self::Read => "read-failed",
            Self::TooLarge => "too-large",
            Self::DecodeOrUnprotect => "decode-or-unprotect-failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrivateInputLoadError {
    kind: PrivateInputLoadKind,
}

impl PrivateInputLoadError {
    fn new(kind: PrivateInputLoadKind) -> Self {
        Self { kind }
    }
}

impl std::fmt::Display for PrivateInputLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "private input load failed: {}",
            self.kind.as_str()
        )
    }
}

impl std::error::Error for PrivateInputLoadError {}

struct PrivateInputLoader {
    manifest_dir: PathBuf,
    capsule_root: PathBuf,
    protected_root: PathBuf,
    canonical_capsule_root: Option<PathBuf>,
    canonical_protected_root: Option<PathBuf>,
    seen: HashSet<PathBuf>,
}

impl PrivateInputLoader {
    fn new(manifest_dir: &Path) -> Self {
        Self {
            manifest_dir: manifest_dir.to_path_buf(),
            capsule_root: manifest_dir.join("data/private/event-capsules"),
            protected_root: manifest_dir.join("data/private/continuous-capture"),
            canonical_capsule_root: None,
            canonical_protected_root: None,
            seen: HashSet::new(),
        }
    }

    fn load_one(&mut self, requested: &Path) -> Result<LoadedPrivateInput, PrivateInputLoadError> {
        let (target, format) = resolve_private_input(
            &self.manifest_dir,
            &self.capsule_root,
            &self.protected_root,
            requested,
        )
        .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::PathPolicy))?;
        let canonical_root = match format {
            PrivateInputFormat::PlainCapsule => {
                if self.canonical_capsule_root.is_none() {
                    self.canonical_capsule_root = Some(
                        validate_capsule_root(&self.manifest_dir, &self.capsule_root).map_err(
                            |_| PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile),
                        )?,
                    );
                }
                self.canonical_capsule_root
                    .as_ref()
                    .expect("initialized above")
                    .clone()
            }
            PrivateInputFormat::ProtectedSegment => {
                if self.canonical_protected_root.is_none() {
                    self.canonical_protected_root = Some(
                        validate_private_root(
                            &self.manifest_dir,
                            &self.protected_root,
                            &["data", "private", "continuous-capture"],
                            "protected segment",
                        )
                        .map_err(|_| {
                            PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile)
                        })?,
                    );
                }
                self.canonical_protected_root
                    .as_ref()
                    .expect("initialized above")
                    .clone()
            }
        };
        let metadata = fs::symlink_metadata(&target)
            .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::Read))?;
        if metadata.file_type().is_symlink() {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile));
        }
        if !metadata.is_file() {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile));
        }

        let canonical_target = fs::canonicalize(&target)
            .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::Read))?;
        if canonical_target.parent() != Some(canonical_root.as_path()) {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile));
        }
        if !self.seen.insert(canonical_target.clone()) {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::Duplicate));
        }

        let mut file = File::open(&canonical_target)
            .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::Read))?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::Read))?;
        if !opened_metadata.is_file() {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::UnsafeFile));
        }
        if opened_metadata.len() > MAX_CAPSULE_FILE_BYTES {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::TooLarge));
        }
        let mut encoded = Vec::new();
        file.by_ref()
            .take(MAX_CAPSULE_FILE_BYTES + 1)
            .read_to_end(&mut encoded)
            .map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::Read))?;
        if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CAPSULE_FILE_BYTES {
            return Err(PrivateInputLoadError::new(PrivateInputLoadKind::TooLarge));
        }
        let decoded = decode_private_input(format, &encoded, &target);
        if format == PrivateInputFormat::PlainCapsule {
            encoded.fill(0);
        }
        decoded.map_err(|_| PrivateInputLoadError::new(PrivateInputLoadKind::DecodeOrUnprotect))
    }
}

#[derive(Debug)]
struct SessionMetadataProgress {
    phase: ReplayInputPhase,
    session_kind: CaptureSessionKind,
    producer_version: String,
    capture_profile: String,
    last_sequence: u64,
    last_ended_unix_ms: u64,
    last_baseline_epoch: Option<u64>,
    last_close_reason: Option<SegmentCloseReason>,
}

struct SegmentMetadataGuard {
    sessions: HashMap<String, SessionMetadataProgress>,
    latest_history_end_unix_ms: Option<u64>,
    latest_evaluation_end_unix_ms: Option<u64>,
    enforce_history_before_evaluation: bool,
}

impl SegmentMetadataGuard {
    fn new(enforce_history_before_evaluation: bool) -> Self {
        Self {
            sessions: HashMap::new(),
            latest_history_end_unix_ms: None,
            latest_evaluation_end_unix_ms: None,
            enforce_history_before_evaluation,
        }
    }

    #[cfg(test)]
    fn observe(
        &mut self,
        metadata: Option<&ContinuousSegmentMetadata>,
        phase: ReplayInputPhase,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.observe_with_integrity(metadata, None, phase)
    }

    fn observe_with_integrity(
        &mut self,
        metadata: Option<&ContinuousSegmentMetadata>,
        integrity: Option<&CaptureIntegrityV1>,
        phase: ReplayInputPhase,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(metadata) = metadata else {
            if integrity.is_some() {
                return Err("integrity evidence requires protected segment metadata".into());
            }
            return Ok(());
        };
        if phase == ReplayInputPhase::Evaluation && self.enforce_history_before_evaluation {
            if self
                .latest_history_end_unix_ms
                .is_some_and(|history_end| metadata.started_unix_ms < history_end)
            {
                return Err("protected evaluation segment predates protected history".into());
            }
            if self
                .latest_evaluation_end_unix_ms
                .is_some_and(|evaluation_end| metadata.started_unix_ms < evaluation_end)
            {
                return Err("protected evaluation segments move backwards globally".into());
            }
        }
        if let Some(progress) = self.sessions.get_mut(&metadata.session_id) {
            if progress.phase != phase {
                return Err("one protected session cannot cross history/evaluation phases".into());
            }
            if progress.session_kind != metadata.session_kind
                || progress.producer_version != metadata.producer_version
                || progress.capture_profile != metadata.capture_profile
            {
                return Err("protected session metadata changed between segments".into());
            }
            if metadata.sequence <= progress.last_sequence {
                return Err("protected session segments are not in increasing order".into());
            }
            if metadata.started_unix_ms < progress.last_ended_unix_ms {
                return Err("protected session segment times move backwards".into());
            }
            if progress.last_baseline_epoch.is_some() != integrity.is_some() {
                return Err("protected session integrity availability changed".into());
            }
            if let Some(integrity) = integrity {
                let previous_epoch = progress
                    .last_baseline_epoch
                    .expect("availability checked above");
                if integrity.baseline_epoch < previous_epoch {
                    return Err("protected session baseline epoch moved backwards".into());
                }
                if progress.last_close_reason == Some(SegmentCloseReason::SessionEnd) {
                    return Err("protected session continued after session end".into());
                }
                if integrity.baseline_epoch == previous_epoch
                    && progress.last_close_reason == Some(SegmentCloseReason::Continuity)
                {
                    return Err("protected session continued a closed baseline epoch".into());
                }
                progress.last_baseline_epoch = Some(integrity.baseline_epoch);
                progress.last_close_reason = Some(integrity.close_reason);
            }
            progress.last_sequence = metadata.sequence;
            progress.last_ended_unix_ms = metadata.ended_unix_ms;
        } else {
            self.sessions.insert(
                metadata.session_id.clone(),
                SessionMetadataProgress {
                    phase,
                    session_kind: metadata.session_kind,
                    producer_version: metadata.producer_version.clone(),
                    capture_profile: metadata.capture_profile.clone(),
                    last_sequence: metadata.sequence,
                    last_ended_unix_ms: metadata.ended_unix_ms,
                    last_baseline_epoch: integrity.map(|value| value.baseline_epoch),
                    last_close_reason: integrity.map(|value| value.close_reason),
                },
            );
        }
        if phase == ReplayInputPhase::History {
            self.latest_history_end_unix_ms = Some(
                self.latest_history_end_unix_ms
                    .map_or(metadata.ended_unix_ms, |latest| {
                        latest.max(metadata.ended_unix_ms)
                    }),
            );
        } else if self.enforce_history_before_evaluation {
            self.latest_evaluation_end_unix_ms = Some(metadata.ended_unix_ms);
        }
        Ok(())
    }
}

fn visit_private_inputs(
    loader: &mut PrivateInputLoader,
    metadata_guard: &mut SegmentMetadataGuard,
    requested_inputs: &[PathBuf],
    phase: ReplayInputPhase,
    mut visitor: impl FnMut(&EventCapsuleV1) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    visit_private_loaded_inputs(loader, metadata_guard, requested_inputs, phase, |loaded| {
        visitor(&loaded.capsule)
    })
}

fn visit_private_loaded_inputs(
    loader: &mut PrivateInputLoader,
    metadata_guard: &mut SegmentMetadataGuard,
    requested_inputs: &[PathBuf],
    phase: ReplayInputPhase,
    mut visitor: impl FnMut(&LoadedPrivateInput) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (index, requested) in requested_inputs.iter().enumerate() {
        let loaded = loader
            .load_one(requested)
            .map_err(|error| RedactedPrivateInputError {
                phase,
                ordinal: index.saturating_add(1),
                kind: error.kind,
            })?;
        metadata_guard.observe_with_integrity(
            loaded.metadata.as_ref(),
            loaded.integrity.as_ref(),
            phase,
        )?;
        visitor(&loaded)?;
    }
    Ok(())
}

#[cfg(test)]
fn read_explicit_capsules(
    manifest_dir: &Path,
    requested_inputs: &[PathBuf],
) -> Result<Vec<EventCapsuleV1>, Box<dyn std::error::Error>> {
    let mut loader = PrivateInputLoader::new(manifest_dir);
    let capsules = requested_inputs
        .iter()
        .map(|requested| loader.load_one(requested).map(|loaded| loaded.capsule))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(capsules)
}

fn decode_private_input(
    format: PrivateInputFormat,
    encoded: &[u8],
    target: &Path,
) -> Result<LoadedPrivateInput, Box<dyn std::error::Error>> {
    match format {
        PrivateInputFormat::PlainCapsule => {
            let text = std::str::from_utf8(encoded).map_err(|error| {
                format!(
                    "cannot read explicitly named capsule {} as UTF-8: {error}",
                    target.display()
                )
            })?;
            let capsule = EventCapsuleV1::from_text(text)
                .map_err(|error| {
                    format!(
                        "invalid private event capsule {}: {error}",
                        target.display()
                    )
                })
                .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            Ok(LoadedPrivateInput {
                capsule,
                metadata: None,
                integrity: None,
            })
        }
        PrivateInputFormat::ProtectedSegment => decode_protected_segment(encoded, target),
    }
}

#[cfg(windows)]
fn decode_protected_segment(
    encoded: &[u8],
    target: &Path,
) -> Result<LoadedPrivateInput, Box<dyn std::error::Error>> {
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
        plaintext.fill(0);
        return Err(format!(
            "unprotected segment exceeds the {MAX_CAPSULE_FILE_BYTES}-byte limit: {}",
            target.display()
        )
        .into());
    }
    let decoded = DecodedContinuousSegment::from_plaintext(&plaintext).map_err(|error| {
        format!(
            "invalid unprotected continuous segment {}: {error}",
            target.display()
        )
    });
    plaintext.fill(0);
    let segment = decoded?;
    let (metadata, integrity, capsule) = segment.into_parts();
    let expected_profile = if integrity.is_some() {
        CODEX_CAPTURE_PROFILE_V2
    } else {
        CODEX_CAPTURE_PROFILE_V1
    };
    if metadata.capture_profile != expected_profile {
        return Err(format!(
            "unsupported protected segment capture profile {:?} in {}",
            metadata.capture_profile,
            target.display()
        )
        .into());
    }
    let expected_name = format!(
        "segment-{}-{:08}.zcs",
        metadata.session_id, metadata.sequence
    );
    if target.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(format!(
            "protected segment file name does not match encrypted session metadata: {}",
            target.display()
        )
        .into());
    }
    Ok(LoadedPrivateInput {
        capsule,
        metadata: Some(metadata),
        integrity,
    })
}

#[cfg(not(windows))]
fn decode_protected_segment(
    _encoded: &[u8],
    target: &Path,
) -> Result<LoadedPrivateInput, Box<dyn std::error::Error>> {
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
        CAPTURE_HEALTH_PRIVACY_NOTICE, CaptureHealthReport, InputSelector, MAX_CAPSULE_FILE_BYTES,
        Options, PUBLIC_RIME_LEXICON, PUBLIC_UD_TRAIN, PrivateInputLoader,
        RedactedPrivateSelectorError, ReplayInputPhase, SegmentMetadataGuard,
        expand_session_selector, parse_options, read_explicit_capsules,
        redact_private_analysis_error, resolve_capsule_input, resolve_protected_input,
        visit_private_inputs,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use ziranma_decoder::{
        BigramLanguageModel, CapsuleReplayReport, CaptureIntegrityCountersV1, CaptureIntegrityV1,
        CaptureSessionKind, CharacterBigramLanguageModel, CommitRecord, ContinuousSegmentMetadata,
        Decoder, DeltaPositionEvidence, EventCapsuleV1, KeySequence, PersonalCacheReplayState,
        RawKey, SegmentCloseReason, TextDelta, TimedTrackerOutput, TrackerOutput,
        parse_rime_lexicon, parse_ud_conllu, select_public_bigram_training_sequences,
    };
    #[cfg(windows)]
    use ziranma_decoder::{
        CODEX_CAPTURE_PROFILE_V1, CODEX_CAPTURE_PROFILE_V2, ContinuousSegmentV1,
        ContinuousSegmentV2, DataProtector, ProtectedSegmentEnvelopeV1, WindowsUserDataProtector,
    };

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn health_help_discloses_behavioral_metadata_and_legacy_unavailability() {
        assert!(CAPTURE_HEALTH_PRIVACY_NOTICE.contains("behavioral metadata"));
        assert!(CAPTURE_HEALTH_PRIVACY_NOTICE.contains("unavailable"));
        assert!(CAPTURE_HEALTH_PRIVACY_NOTICE.contains("never zero-filled"));
    }

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
        capsule_with_text("猫")
    }

    fn capsule_with_text(text: &str) -> EventCapsuleV1 {
        EventCapsuleV1::new(vec![TimedTrackerOutput {
            elapsed_ms: 100,
            output: TrackerOutput::Commit(CommitRecord {
                keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space],
                keys_complete: true,
                composition: "mao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "mao".to_owned(),
                    inserted: text.to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: text.to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }),
        }])
        .unwrap()
    }

    fn segment_metadata(
        session_id: &str,
        sequence: u64,
        started_unix_ms: u64,
        ended_unix_ms: u64,
        producer_version: &str,
    ) -> ContinuousSegmentMetadata {
        ContinuousSegmentMetadata::new(
            session_id.to_owned(),
            sequence,
            started_unix_ms,
            ended_unix_ms,
            CaptureSessionKind::Daily,
            producer_version.to_owned(),
            "codex-uia-v1".to_owned(),
        )
        .unwrap()
    }

    fn integrity(epoch: u64, reason: SegmentCloseReason) -> CaptureIntegrityV1 {
        CaptureIntegrityV1::new(
            epoch,
            reason,
            CaptureIntegrityCountersV1 {
                value_callbacks_observed: 1,
                tracker_outputs_emitted: 1,
                ..CaptureIntegrityCountersV1::default()
            },
            1,
        )
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

    #[cfg(windows)]
    #[test]
    fn protected_v2_segment_preserves_integrity_while_reusing_v1_events() {
        let mut workspace = TestWorkspace::new();
        let expected = capsule();
        let metadata = ContinuousSegmentMetadata::new(
            "test-v2".to_owned(),
            0,
            10,
            20,
            CaptureSessionKind::Daily,
            "0.1.0+continuous.7".to_owned(),
            CODEX_CAPTURE_PROFILE_V2.to_owned(),
        )
        .unwrap();
        let counters = CaptureIntegrityCountersV1 {
            key_actions_observed: 3,
            value_callbacks_observed: 1,
            tracker_outputs_emitted: 1,
            ..CaptureIntegrityCountersV1::default()
        };
        let integrity =
            CaptureIntegrityV1::new(1, SegmentCloseReason::SessionEnd, counters, 1).unwrap();
        let segment =
            ContinuousSegmentV2::new(metadata, integrity.clone(), expected.clone()).unwrap();
        let protected = WindowsUserDataProtector
            .protect(&segment.to_plaintext().unwrap())
            .unwrap();
        let bytes = ProtectedSegmentEnvelopeV1::new(protected)
            .unwrap()
            .to_bytes()
            .unwrap();
        workspace.write_protected("segment-test-v2-00000000.zcs", &bytes);

        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let loaded = loader
            .load_one(&workspace.relative_protected("segment-test-v2-00000000.zcs"))
            .unwrap();
        assert_eq!(loaded.capsule, expected);
        assert_eq!(loaded.integrity, Some(integrity));
    }

    #[cfg(windows)]
    #[test]
    fn protected_segment_schema_profile_mismatches_are_rejected_and_redacted() {
        let mut workspace = TestWorkspace::new();
        let marker = "PRIVATE_PROFILE_MARKER";

        let v1_metadata = ContinuousSegmentMetadata::new(
            "bad-v1".to_owned(),
            0,
            10,
            20,
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            CODEX_CAPTURE_PROFILE_V2.to_owned(),
        )
        .unwrap();
        let v1 = ContinuousSegmentV1::new(v1_metadata, capsule_with_text(marker)).unwrap();
        let v1_bytes = ProtectedSegmentEnvelopeV1::new(
            WindowsUserDataProtector
                .protect(&v1.to_plaintext().unwrap())
                .unwrap(),
        )
        .unwrap()
        .to_bytes()
        .unwrap();
        workspace.write_protected("segment-bad-v1-00000000.zcs", &v1_bytes);

        let v2_metadata = ContinuousSegmentMetadata::new(
            "bad-v2".to_owned(),
            0,
            20,
            30,
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            CODEX_CAPTURE_PROFILE_V1.to_owned(),
        )
        .unwrap();
        let v2 = ContinuousSegmentV2::new(
            v2_metadata,
            integrity(1, SegmentCloseReason::SessionEnd),
            capsule_with_text(marker),
        )
        .unwrap();
        let v2_bytes = ProtectedSegmentEnvelopeV1::new(
            WindowsUserDataProtector
                .protect(&v2.to_plaintext().unwrap())
                .unwrap(),
        )
        .unwrap()
        .to_bytes()
        .unwrap();
        workspace.write_protected("segment-bad-v2-00000000.zcs", &v2_bytes);

        for name in ["segment-bad-v1-00000000.zcs", "segment-bad-v2-00000000.zcs"] {
            let mut loader = PrivateInputLoader::new(&workspace.manifest);
            let mut guard = SegmentMetadataGuard::new(false);
            let error = visit_private_inputs(
                &mut loader,
                &mut guard,
                &[workspace.relative_protected(name)],
                ReplayInputPhase::Evaluation,
                |_| Ok(()),
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("decode-or-unprotect-failed"));
            assert!(!error.contains(marker));
            assert!(!error.contains(name));
            assert!(!error.contains("codex-uia"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn streaming_loader_accepts_mixed_plain_and_protected_inputs_in_order() {
        let mut workspace = TestWorkspace::new();
        workspace.write("plain.zic", &capsule_with_text("甲").to_text().unwrap());
        let metadata = ContinuousSegmentMetadata::new(
            "mixed-1".to_owned(),
            0,
            10,
            20,
            CaptureSessionKind::Daily,
            "0.1.0".to_owned(),
            CODEX_CAPTURE_PROFILE_V1.to_owned(),
        )
        .unwrap();
        let segment = ContinuousSegmentV1::new(metadata, capsule_with_text("乙")).unwrap();
        let protected = WindowsUserDataProtector
            .protect(&segment.to_plaintext().unwrap())
            .unwrap();
        let bytes = ProtectedSegmentEnvelopeV1::new(protected)
            .unwrap()
            .to_bytes()
            .unwrap();
        workspace.write_protected("segment-mixed-1-00000000.zcs", &bytes);
        let inputs = vec![
            workspace.relative("plain.zic"),
            workspace.relative_protected("segment-mixed-1-00000000.zcs"),
        ];

        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(false);
        let mut inserted = Vec::new();
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                let TrackerOutput::Commit(commit) = &capsule.events()[0].output else {
                    panic!("synthetic capsule must contain a commit");
                };
                inserted.push(commit.document_change.inserted.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(inserted, vec!["甲", "乙"]);
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
        let marker = "PRIVATE_PATH_MARKER";
        let unknown = match parse_options(vec![marker.to_owned()]) {
            Err(error) => error,
            Ok(_) => panic!("a positional private value must not be accepted"),
        };
        assert!(!unknown.to_string().contains(marker));
        assert!(!format!("{unknown:?}").contains(marker));
        assert!(unknown.to_string().contains("value was suppressed"));
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
        assert!(line.contains("contains_behavioral_metadata=true"));
        assert!(line.contains("report_schema=ziranma-capture-health-report-v1"));
        assert!(!line.contains('猫'));
        assert!(!line.contains("mao"));
    }

    #[test]
    fn integrity_health_is_aggregate_marks_legacy_as_unavailable_and_never_echoes_text() {
        let counters = CaptureIntegrityCountersV1 {
            key_actions_observed: 3,
            composition_callbacks_observed: 2,
            composition_finalized_callbacks_observed: 1,
            value_callbacks_observed: 2,
            value_read_errors: 0,
            composition_read_errors: 0,
            selection_read_errors: 1,
            value_callbacks_without_output: 1,
            tracker_outputs_emitted: 1,
            key_actions_not_emitted_at_boundary: 0,
            key_buffer_resets: 0,
            counter_saturated: false,
        };
        let loaded = super::LoadedPrivateInput {
            capsule: capsule(),
            metadata: Some(
                ContinuousSegmentMetadata::new(
                    "synthetic-session".to_owned(),
                    0,
                    10,
                    20,
                    CaptureSessionKind::Daily,
                    "0.1.0+continuous.7".to_owned(),
                    "codex-uia-v2".to_owned(),
                )
                .unwrap(),
            ),
            integrity: Some(
                CaptureIntegrityV1::new(1, SegmentCloseReason::Timer, counters, 1).unwrap(),
            ),
        };
        let mut report = CaptureHealthReport::default();
        report.observe_loaded(&loaded);
        report.observe_loaded(&super::LoadedPrivateInput {
            capsule: capsule(),
            metadata: None,
            integrity: None,
        });
        assert_eq!(
            report.integrity_available_segments + report.legacy_inputs_without_integrity,
            report.capsules
        );
        assert_eq!(
            report.close_capacity
                + report.close_timer
                + report.close_continuity
                + report.close_session_end,
            report.integrity_available_segments
        );
        assert_eq!(report.integrity_counters.tracker_outputs_emitted, 1);
        let line = report.integrity_terminal_line();
        assert!(line.starts_with("CAPTURE_INTEGRITY contains_text=false"));
        assert!(line.contains("report_schema=ziranma-capture-integrity-report-v1"));
        assert!(line.contains("integrity_schema=ziranma-codex-uia-integrity-v1"));
        assert!(line.contains("available_segments=1"));
        assert!(line.contains("legacy_inputs_without_integrity=1"));
        assert!(line.contains("baseline_epochs_observed=1"));
        assert!(line.contains("close_timer=1"));
        assert!(!line.contains('猫'));
        assert!(!line.contains("mao"));
        assert!(!line.contains("synthetic-session"));
    }

    #[test]
    fn private_analysis_errors_never_echo_the_rejected_key_sequence() {
        let marker = "PRIVATE_KEY_MARKER";
        let source = KeySequence::new(marker).unwrap_err();
        let error = redact_private_analysis_error::<(), _>(Err(source)).unwrap_err();
        let redacted = error.to_string();
        assert!(!redacted.contains(marker));
        assert_eq!(
            redacted,
            "private replay analysis failed; input details were suppressed"
        );
        assert_eq!(format!("{error:?}"), redacted);
    }

    #[test]
    fn selector_errors_use_redacted_debug_output_for_main_termination() {
        for phase in [ReplayInputPhase::History, ReplayInputPhase::Evaluation] {
            let error = RedactedPrivateSelectorError { phase };
            let display = error.to_string();
            assert_eq!(format!("{error:?}"), display);
            assert!(!display.contains("session-id-or-path"));
        }
    }

    #[test]
    fn streaming_health_report_matches_collected_capsules() {
        let capsules = vec![capsule(), capsule()];
        let collected = CaptureHealthReport::from_capsules(&capsules);
        let mut streamed = CaptureHealthReport::default();
        for capsule in &capsules {
            streamed.observe_capsule(capsule);
        }
        assert_eq!(streamed, collected);
        assert_eq!(streamed.terminal_line(), collected.terminal_line());
    }

    #[test]
    fn streaming_baseline_report_matches_collected_loading() {
        let mut workspace = TestWorkspace::new();
        workspace.write("first.zic", &capsule().to_text().unwrap());
        workspace.write("second.zic", &capsule().to_text().unwrap());
        let inputs = vec![
            workspace.relative("first.zic"),
            workspace.relative("second.zic"),
        ];
        let collected_capsules = read_explicit_capsules(&workspace.manifest, &inputs).unwrap();
        let entries = parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap().entries;
        let decoder = Decoder::new(entries);

        let mut collected = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        for capsule in &collected_capsules {
            collected.observe_capsule(&decoder, capsule).unwrap();
        }

        let mut streamed = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(false);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                streamed.observe_capsule(&decoder, capsule)?;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(streamed, collected);
        assert_eq!(streamed.terminal_line(), collected.terminal_line());
        assert_eq!(
            streamed.compact_terminal_report(),
            collected.compact_terminal_report()
        );
    }

    #[test]
    fn streaming_public_context_reports_match_collected_loading() {
        let mut workspace = TestWorkspace::new();
        workspace.write("first.zic", &capsule().to_text().unwrap());
        workspace.write("second.zic", &capsule().to_text().unwrap());
        let inputs = vec![
            workspace.relative("first.zic"),
            workspace.relative("second.zic"),
        ];
        let collected_capsules = read_explicit_capsules(&workspace.manifest, &inputs).unwrap();
        let entries = parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap().entries;
        let decoder = Decoder::new(entries.clone());
        let corpus = parse_ud_conllu(PUBLIC_UD_TRAIN).unwrap();
        let training = select_public_bigram_training_sequences(&corpus, &entries);
        let word_model =
            BigramLanguageModel::from_token_sequences(&training.sequences, &entries).unwrap();
        let frequency_total = entries
            .iter()
            .map(|entry| entry.frequency as f64)
            .sum::<f64>();
        let log_frequency_total = frequency_total.ln();

        let mut collected_word = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        for capsule in &collected_capsules {
            collected_word
                .observe_capsule_with_public_context(
                    &decoder,
                    &word_model,
                    log_frequency_total,
                    capsule,
                )
                .unwrap();
        }
        let mut streamed_word = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(false);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                streamed_word.observe_capsule_with_public_context(
                    &decoder,
                    &word_model,
                    log_frequency_total,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(streamed_word, collected_word);

        let text_sequences = training
            .sequences
            .iter()
            .map(|sequence| sequence.concat())
            .collect::<Vec<_>>();
        let character_model =
            CharacterBigramLanguageModel::from_text_sequences(&text_sequences).unwrap();
        let mut collected_character =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        for capsule in &collected_capsules {
            collected_character
                .observe_capsule_with_public_character_context(&decoder, &character_model, capsule)
                .unwrap();
        }
        let mut streamed_character =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(false);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                streamed_character.observe_capsule_with_public_character_context(
                    &decoder,
                    &character_model,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(streamed_character, collected_character);
    }

    #[test]
    fn streaming_personal_history_and_evaluation_share_only_the_cache_state() {
        let mut workspace = TestWorkspace::new();
        workspace.write("history.zic", &capsule().to_text().unwrap());
        workspace.write("evaluation.zic", &capsule().to_text().unwrap());
        let history_inputs = vec![workspace.relative("history.zic")];
        let evaluation_inputs = vec![workspace.relative("evaluation.zic")];
        let history_capsules =
            read_explicit_capsules(&workspace.manifest, &history_inputs).unwrap();
        let evaluation_capsules =
            read_explicit_capsules(&workspace.manifest, &evaluation_inputs).unwrap();
        let entries = parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap().entries;
        let decoder = Decoder::new(entries);

        let mut collected_state = PersonalCacheReplayState::new();
        let mut collected_history =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        collected_history
            .observe_capsule_with_personal_pair_cache(
                &decoder,
                &mut collected_state,
                &history_capsules[0],
            )
            .unwrap();
        let mut collected = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        collected.record_personal_cache_history(&collected_history, &collected_state);
        collected
            .observe_capsule_with_personal_pair_cache(
                &decoder,
                &mut collected_state,
                &evaluation_capsules[0],
            )
            .unwrap();

        let mut streamed_state = PersonalCacheReplayState::new();
        let mut streamed_history =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut streamed = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(true);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &history_inputs,
            ReplayInputPhase::History,
            |capsule| {
                streamed_history.observe_capsule_with_personal_pair_cache(
                    &decoder,
                    &mut streamed_state,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();
        streamed.record_personal_cache_history(&streamed_history, &streamed_state);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &evaluation_inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                streamed.observe_capsule_with_personal_pair_cache(
                    &decoder,
                    &mut streamed_state,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(streamed, collected);
    }

    #[test]
    fn streaming_word_cache_matches_multiple_online_evaluation_capsules() {
        let mut workspace = TestWorkspace::new();
        workspace.write("history.zic", &capsule().to_text().unwrap());
        workspace.write("evaluation-1.zic", &capsule().to_text().unwrap());
        workspace.write("evaluation-2.zic", &capsule().to_text().unwrap());
        let history_inputs = vec![workspace.relative("history.zic")];
        let evaluation_inputs = vec![
            workspace.relative("evaluation-1.zic"),
            workspace.relative("evaluation-2.zic"),
        ];
        let history_capsules =
            read_explicit_capsules(&workspace.manifest, &history_inputs).unwrap();
        let evaluation_capsules =
            read_explicit_capsules(&workspace.manifest, &evaluation_inputs).unwrap();
        let entries = parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap().entries;
        let decoder = Decoder::new(entries);

        let mut collected_state = PersonalCacheReplayState::new();
        let mut collected_history =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        collected_history
            .observe_capsule_with_personal_cache(
                &decoder,
                &mut collected_state,
                &history_capsules[0],
            )
            .unwrap();
        let mut collected = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        collected.record_personal_cache_history(&collected_history, &collected_state);
        for capsule in &evaluation_capsules {
            collected
                .observe_capsule_with_personal_cache(&decoder, &mut collected_state, capsule)
                .unwrap();
        }

        let mut streamed_state = PersonalCacheReplayState::new();
        let mut streamed_history =
            CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut streamed = CapsuleReplayReport::with_window_gap_limit(Some(15_000)).unwrap();
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(true);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &history_inputs,
            ReplayInputPhase::History,
            |capsule| {
                streamed_history.observe_capsule_with_personal_cache(
                    &decoder,
                    &mut streamed_state,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();
        streamed.record_personal_cache_history(&streamed_history, &streamed_state);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &evaluation_inputs,
            ReplayInputPhase::Evaluation,
            |capsule| {
                streamed.observe_capsule_with_personal_cache(
                    &decoder,
                    &mut streamed_state,
                    capsule,
                )?;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(streamed, collected);
    }

    #[test]
    fn streaming_loader_rejects_a_duplicate_across_history_and_evaluation() {
        let mut workspace = TestWorkspace::new();
        workspace.write("same.zic", &capsule().to_text().unwrap());
        let input = vec![workspace.relative("same.zic")];
        let mut loader = PrivateInputLoader::new(&workspace.manifest);
        let mut guard = SegmentMetadataGuard::new(true);
        visit_private_inputs(
            &mut loader,
            &mut guard,
            &input,
            ReplayInputPhase::History,
            |_| Ok(()),
        )
        .unwrap();
        let error = visit_private_inputs(
            &mut loader,
            &mut guard,
            &input,
            ReplayInputPhase::Evaluation,
            |_| Ok(()),
        )
        .unwrap_err();
        let debug = format!("{error:?}");
        assert_eq!(
            error.to_string(),
            "private evaluation input #1 could not be loaded: duplicate; path and content were suppressed"
        );
        assert_eq!(debug, error.to_string());
        assert!(!error.to_string().contains("same.zic"));
    }

    #[test]
    fn protected_metadata_guard_rejects_reordering_and_future_leakage() {
        let mut ordered = SegmentMetadataGuard::new(true);
        ordered
            .observe(
                Some(&segment_metadata("history", 0, 10, 20, "old")),
                ReplayInputPhase::History,
            )
            .unwrap();
        ordered
            .observe(
                Some(&segment_metadata("history", 1, 20, 30, "old")),
                ReplayInputPhase::History,
            )
            .unwrap();
        ordered
            .observe(
                Some(&segment_metadata("evaluation", 0, 31, 40, "new")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();

        let mut backwards_sequence = SegmentMetadataGuard::new(false);
        backwards_sequence
            .observe(
                Some(&segment_metadata("same", 1, 10, 20, "v1")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            backwards_sequence
                .observe(
                    Some(&segment_metadata("same", 0, 20, 30, "v1")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut changed_version = SegmentMetadataGuard::new(false);
        changed_version
            .observe(
                Some(&segment_metadata("same", 0, 10, 20, "v1")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            changed_version
                .observe(
                    Some(&segment_metadata("same", 1, 20, 30, "v2")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut changed_profile = SegmentMetadataGuard::new(false);
        changed_profile
            .observe(
                Some(&segment_metadata("same", 0, 10, 20, "v1")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        let mut profile_mismatch = segment_metadata("same", 1, 20, 30, "v1");
        profile_mismatch.capture_profile = "other-profile".to_owned();
        assert!(
            changed_profile
                .observe(Some(&profile_mismatch), ReplayInputPhase::Evaluation)
                .is_err()
        );

        let mut changed_kind = SegmentMetadataGuard::new(false);
        changed_kind
            .observe(
                Some(&segment_metadata("same", 0, 10, 20, "v1")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        let mut kind_mismatch = segment_metadata("same", 1, 20, 30, "v1");
        kind_mismatch.session_kind = CaptureSessionKind::Theme;
        assert!(
            changed_kind
                .observe(Some(&kind_mismatch), ReplayInputPhase::Evaluation)
                .is_err()
        );

        let mut leaked_future = SegmentMetadataGuard::new(true);
        leaked_future
            .observe(
                Some(&segment_metadata("history", 0, 100, 200, "old")),
                ReplayInputPhase::History,
            )
            .unwrap();
        assert!(
            leaked_future
                .observe(
                    Some(&segment_metadata("evaluation", 0, 150, 250, "new")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut crossed_phase = SegmentMetadataGuard::new(true);
        crossed_phase
            .observe(
                Some(&segment_metadata("same", 0, 10, 20, "v1")),
                ReplayInputPhase::History,
            )
            .unwrap();
        assert!(
            crossed_phase
                .observe(
                    Some(&segment_metadata("same", 1, 20, 30, "v1")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut backwards_time = SegmentMetadataGuard::new(false);
        backwards_time
            .observe(
                Some(&segment_metadata("same", 0, 10, 20, "v1")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            backwards_time
                .observe(
                    Some(&segment_metadata("same", 1, 19, 30, "v1")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut reversed_evaluation_sessions = SegmentMetadataGuard::new(true);
        reversed_evaluation_sessions
            .observe(
                Some(&segment_metadata("newer", 0, 200, 300, "v2")),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            reversed_evaluation_sessions
                .observe(
                    Some(&segment_metadata("older", 0, 150, 180, "v1")),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );
    }

    #[test]
    fn protected_metadata_guard_keeps_v2_integrity_epochs_causal() {
        let metadata = |sequence, start, end| {
            let mut metadata = segment_metadata("integrity", sequence, start, end, "continuous.7");
            metadata.capture_profile = "codex-uia-v2".to_owned();
            metadata
        };

        let mut closed_epoch = SegmentMetadataGuard::new(false);
        closed_epoch
            .observe_with_integrity(
                Some(&metadata(0, 10, 20)),
                Some(&integrity(1, SegmentCloseReason::Timer)),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        closed_epoch
            .observe_with_integrity(
                Some(&metadata(1, 20, 30)),
                Some(&integrity(1, SegmentCloseReason::Continuity)),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            closed_epoch
                .observe_with_integrity(
                    Some(&metadata(2, 30, 40)),
                    Some(&integrity(1, SegmentCloseReason::Timer)),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut backwards_epoch = SegmentMetadataGuard::new(false);
        backwards_epoch
            .observe_with_integrity(
                Some(&metadata(0, 10, 20)),
                Some(&integrity(2, SegmentCloseReason::Timer)),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            backwards_epoch
                .observe_with_integrity(
                    Some(&metadata(1, 20, 30)),
                    Some(&integrity(1, SegmentCloseReason::Timer)),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );

        let mut ended = SegmentMetadataGuard::new(false);
        ended
            .observe_with_integrity(
                Some(&metadata(0, 10, 20)),
                Some(&integrity(1, SegmentCloseReason::SessionEnd)),
                ReplayInputPhase::Evaluation,
            )
            .unwrap();
        assert!(
            ended
                .observe_with_integrity(
                    Some(&metadata(1, 20, 30)),
                    Some(&integrity(2, SegmentCloseReason::Timer)),
                    ReplayInputPhase::Evaluation,
                )
                .is_err()
        );
    }

    #[test]
    fn a_late_invalid_input_returns_no_completed_report() {
        let mut workspace = TestWorkspace::new();
        workspace.write("first.zic", &capsule().to_text().unwrap());
        workspace.write("broken.zic", "PRIVATE_MARKER");
        let inputs = vec![
            workspace.relative("first.zic"),
            workspace.relative("broken.zic"),
        ];
        let entries = parse_rime_lexicon(PUBLIC_RIME_LEXICON).unwrap().entries;
        let decoder = Decoder::new(entries);
        let completed_report = (|| -> Result<String, Box<dyn std::error::Error>> {
            let mut report = CapsuleReplayReport::with_window_gap_limit(Some(15_000))?;
            let mut loader = PrivateInputLoader::new(&workspace.manifest);
            let mut guard = SegmentMetadataGuard::new(false);
            visit_private_inputs(
                &mut loader,
                &mut guard,
                &inputs,
                ReplayInputPhase::Evaluation,
                |capsule| {
                    report.observe_capsule(&decoder, capsule)?;
                    Ok(())
                },
            )?;
            Ok(report.terminal_line())
        })();
        let error = completed_report.unwrap_err().to_string();
        assert!(!error.contains("PRIVATE_MARKER"));
        assert!(!error.contains("broken.zic"));
        assert_eq!(
            error,
            "private evaluation input #2 could not be loaded: decode-or-unprotect-failed; path and content were suppressed"
        );
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
        assert!(oversized_error.to_string().contains("too-large"));
    }
}
