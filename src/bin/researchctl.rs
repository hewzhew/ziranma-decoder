use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::thread;

use ziranma_core::{
    CURRENT_WISH_SCHEMA_VERSION, NativeCandidatePersonalization, NativeCandidateProvenance,
    NativeCandidateSource, NativeCandidateView, NativeFeedbackEvent, NativeSelectionSource,
    RESEARCH_FEEDBACK_DIRECTORY, ResearchHabitKind, ResearchHalfPairAnalysis,
    ResearchSceneAnalysis, TranspositionCalibrationLabel, WishCaptureScope, WishRuntimeIdentity,
    WishSnapshot, analyze_linked_research, analyze_runtime_half_pairs, list_wish_packages,
    repository_root_for_user_tool_executable, research_feedback_enabled,
    set_research_feedback_enabled,
};
#[cfg(windows)]
use ziranma_core::{WindowsUserDataProtector, load_wish_snapshot};

const MAX_REVIEW_BATCHES: usize = 4_096;
const MAX_REVIEW_ITEMS: usize = 12;
const PARALLEL_LOAD_THRESHOLD: usize = 32;
const MAX_PARALLEL_LOADERS: usize = 4;
const WISH_SCHEMA_VERSION_COUNT: usize = CURRENT_WISH_SCHEMA_VERSION as usize;
const CANDIDATE_PERSONALIZATION_KINDS: [(NativeCandidatePersonalization, &str); 6] = [
    (NativeCandidatePersonalization::PERSISTENT_EXACT, "持久精确"),
    (
        NativeCandidatePersonalization::PERSISTENT_ANCHORED,
        "持久尾简",
    ),
    (
        NativeCandidatePersonalization::PERSISTENT_DISCOVERY,
        "持久发现",
    ),
    (NativeCandidatePersonalization::SESSION_EXACT, "会话精确"),
    (NativeCandidatePersonalization::SESSION_ANCHORED, "会话尾简"),
    (NativeCandidatePersonalization::LEFT_CONTEXT, "左侧上下文"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Enable,
    Disable,
    Summary,
    Review,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    command: Command,
    root: Option<PathBuf>,
    confirmed: bool,
    read_private_feedback: bool,
    show_private_text: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("持续研究操作失败：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    let root = match options.root {
        Some(root) => root,
        None => env::current_exe()
            .ok()
            .and_then(|path| research_root_for_executable(&path))
            .ok_or("无法从当前 release 工具位置确定持续研究目录")?,
    };
    match options.command {
        Command::Status => print_status(&root),
        Command::Enable => {
            if !options.confirmed {
                return Err("开启会持续保存普通输入域里的原码、候选与提交原文；请加入 \
                     --confirm-continuous-private-feedback"
                    .into());
            }
            let changed = set_research_feedback_enabled(&root, true)?;
            println!(
                "持续研究已{}\n生效：正在运行的输入法宿主会在后续输入中自动发现\n范围：猫猫输入法的普通输入域\n保存：当前用户 DPAPI 加密\n密码、PIN 与受限输入域：不记录\n网络：不连接",
                if changed { "开启" } else { "经开启" }
            );
            Ok(())
        }
        Command::Disable => {
            let changed = set_research_feedback_enabled(&root, false)?;
            println!(
                "持续研究已{}\n新增保存：已停止，运行中的宿主不会再发布批次\n已有加密批次：保留",
                if changed {
                    "关闭"
                } else {
                    "处于关闭状态"
                }
            );
            Ok(())
        }
        Command::Summary => {
            if !options.read_private_feedback {
                return Err("摘要会在本机解密私人反馈并只显示聚合数字；请加入 \
                     --confirm-read-private-feedback"
                    .into());
            }
            print_summary(&root)
        }
        Command::Review => {
            if !options.show_private_text {
                return Err("回顾会解密并在当前终端显示真实编码和提交文字；请加入 \
                     --confirm-show-private-text"
                    .into());
            }
            print_review(&root)
        }
    }
}

#[cfg(not(windows))]
fn print_summary(_root: &Path) -> Result<(), Box<dyn Error>> {
    Err("持续研究摘要的当前用户解密目前只支持 Windows".into())
}

#[cfg(windows)]
fn print_summary(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut packages = list_wish_packages(root)?;
    let available_batches = packages.len();
    packages.truncate(MAX_REVIEW_BATCHES);
    packages.reverse();
    let snapshots = load_research_snapshots(root, &packages)?;
    let mut review = ResearchReview::default();
    for snapshot in &snapshots {
        review.observe(snapshot)?;
    }
    review.available_batches = available_batches;
    let latest_runtime = match latest_runtime_review(&snapshots)? {
        Some((identity, review)) => {
            let half_pairs = analyze_runtime_half_pairs(&snapshots, &identity)?;
            review.render_runtime_summary(&identity, &half_pairs)
        }
        None => "最新运行身份：尚无带版本标识的批次。".to_owned(),
    };
    println!("{}\n\n{}", latest_runtime, review.render_aggregate());
    Ok(())
}

#[cfg(not(windows))]
fn print_review(_root: &Path) -> Result<(), Box<dyn Error>> {
    Err("持续研究回顾的当前用户解密目前只支持 Windows".into())
}

#[cfg(windows)]
fn print_review(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut packages = list_wish_packages(root)?;
    let available_batches = packages.len();
    packages.truncate(MAX_REVIEW_BATCHES);
    packages.reverse();
    let snapshots = load_research_snapshots(root, &packages)?;
    let mut review = ResearchReview::default();
    for snapshot in &snapshots {
        review.observe(snapshot)?;
    }
    review.available_batches = available_batches;
    let latest_runtime = latest_runtime_review(&snapshots)?;
    let wishes = load_linkable_wishes(root)?;
    let scenes = analyze_linked_research(&snapshots, &wishes)?;
    let latest_runtime = match latest_runtime {
        Some((identity, review)) => {
            let half_pairs = analyze_runtime_half_pairs(&snapshots, &identity)?;
            review.render_runtime_summary(&identity, &half_pairs)
        }
        None => "最新运行身份：尚无带版本标识的批次。".to_owned(),
    };
    println!(
        "{}\n\n{}\n\n{}",
        latest_runtime,
        review.render(),
        render_scene_analysis(&scenes)
    );
    Ok(())
}

#[cfg(windows)]
fn load_research_snapshots(
    root: &Path,
    packages: &[ziranma_core::WishPackageInfo],
) -> Result<Vec<WishSnapshot>, ziranma_core::WishFeedbackError> {
    let available = thread::available_parallelism().map_or(1, usize::from);
    let workers = available.min(MAX_PARALLEL_LOADERS);
    ordered_bounded_map(packages, workers, |package| {
        load_wish_snapshot(root, package.id(), &WindowsUserDataProtector)
    })
}

fn ordered_bounded_map<T, U, E, F>(
    items: &[T],
    maximum_workers: usize,
    operation: F,
) -> Result<Vec<U>, E>
where
    T: Sync,
    U: Send,
    E: Send,
    F: Fn(&T) -> Result<U, E> + Sync,
{
    let worker_count = maximum_workers.max(1).min(items.len().max(1));
    if worker_count == 1 || items.len() < PARALLEL_LOAD_THRESHOLD {
        return items.iter().map(&operation).collect();
    }
    let chunk_size = items.len().div_ceil(worker_count);
    thread::scope(|scope| {
        let handles = items
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(|| chunk.iter().map(&operation).collect::<Result<Vec<_>, E>>())
            })
            .collect::<Vec<_>>();
        let mut output = Vec::with_capacity(items.len());
        for handle in handles {
            output.extend(
                handle
                    .join()
                    .expect("bounded research loader worker must not panic")?,
            );
        }
        Ok(output)
    })
}

#[cfg(windows)]
fn load_linkable_wishes(research_root: &Path) -> Result<Vec<WishSnapshot>, Box<dyn Error>> {
    if research_root.file_name().and_then(|name| name.to_str()) != Some(RESEARCH_FEEDBACK_DIRECTORY)
    {
        return Ok(Vec::new());
    }
    let Some(user_data_root) = research_root.parent() else {
        return Ok(Vec::new());
    };
    let wish_root = user_data_root.join("wishes");
    let mut packages = list_wish_packages(&wish_root)?;
    packages.truncate(MAX_REVIEW_BATCHES);
    packages
        .into_iter()
        .map(|package| {
            load_wish_snapshot(&wish_root, package.id(), &WindowsUserDataProtector)
                .map_err(|error| error.into())
        })
        .collect()
}

#[derive(Clone)]
struct PresentedFrame {
    code: String,
    view: NativeCandidateView,
    page_start: usize,
    provenance: Vec<NativeCandidateProvenance>,
    global_top_provenance: Option<NativeCandidateProvenance>,
}

impl PresentedFrame {
    fn next(
        code: &str,
        view: NativeCandidateView,
        page_start: usize,
        provenance: Vec<NativeCandidateProvenance>,
        previous: Option<&Self>,
    ) -> Self {
        let global_top_provenance = if page_start == 0 {
            provenance.first().copied()
        } else {
            previous
                .filter(|frame| frame.code == code && frame.view == view)
                .and_then(|frame| frame.global_top_provenance)
        };
        Self {
            code: code.to_owned(),
            view,
            page_start,
            provenance,
            global_top_provenance,
        }
    }

    fn provenance_for_rank(&self, absolute_rank: usize) -> Option<NativeCandidateProvenance> {
        let index = absolute_rank.checked_sub(self.page_start.saturating_add(1))?;
        self.provenance.get(index).copied()
    }
}

#[derive(Clone, Default)]
struct SelectionPattern {
    selections: usize,
    non_top_selections: usize,
    first_rank: Option<usize>,
    last_rank: Option<usize>,
    minimum_rank: usize,
    maximum_rank: usize,
    manual_selections: usize,
    paged_selections: usize,
    precise_personalization_observations: usize,
    personalization_frames: [usize; CANDIDATE_PERSONALIZATION_KINDS.len()],
    provenance_observations: usize,
    sources: [usize; 10],
    top_provenance_observations: usize,
    top_sources: [usize; 10],
}

impl SelectionPattern {
    fn observe(
        &mut self,
        rank: usize,
        selection: NativeSelectionSource,
        provenance: Option<NativeCandidateProvenance>,
        precise_personalization: bool,
    ) {
        self.selections += 1;
        self.non_top_selections += usize::from(rank > 1);
        self.first_rank.get_or_insert(rank);
        self.last_rank = Some(rank);
        self.minimum_rank = if self.minimum_rank == 0 {
            rank
        } else {
            self.minimum_rank.min(rank)
        };
        self.maximum_rank = self.maximum_rank.max(rank);
        self.manual_selections += usize::from(selection != NativeSelectionSource::FirstCandidate);
        self.paged_selections += usize::from(rank > 6);
        self.precise_personalization_observations += usize::from(precise_personalization);
        if let Some(provenance) = provenance {
            self.provenance_observations += 1;
            for (index, (bit, _)) in CANDIDATE_PERSONALIZATION_KINDS.iter().enumerate() {
                self.personalization_frames[index] +=
                    usize::from(provenance.personalization().contains(*bit));
            }
            self.sources[candidate_source_index(provenance.source())] += 1;
        }
    }

    fn observe_global_top_provenance(&mut self, provenance: Option<NativeCandidateProvenance>) {
        if let Some(provenance) = provenance {
            self.top_provenance_observations += 1;
            self.top_sources[candidate_source_index(provenance.source())] += 1;
        }
    }

    fn dominant_source(&self) -> NativeCandidateSource {
        self.sources
            .iter()
            .enumerate()
            .max_by_key(|(index, count)| (**count, std::cmp::Reverse(*index)))
            .map(|(index, _)| candidate_source_from_index(index))
            .unwrap_or(NativeCandidateSource::Unknown)
    }

    fn dominant_top_source(&self) -> NativeCandidateSource {
        self.top_sources
            .iter()
            .enumerate()
            .max_by_key(|(index, count)| (**count, std::cmp::Reverse(*index)))
            .map(|(index, _)| candidate_source_from_index(index))
            .unwrap_or(NativeCandidateSource::Unknown)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SelectionPressure {
    observed_codes: usize,
    multi_output_codes: usize,
    competing_non_top_codes: usize,
    single_output_non_top_codes: usize,
    first_non_top_identities: usize,
    single_observation_first_non_top_identities: usize,
    followup_first_non_top_identities: usize,
    first_non_top_later_top_identities: usize,
    followup_first_non_top_never_top_identities: usize,
    first_top_later_non_top_identities: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FollowupNonTopPressure {
    identities: usize,
    ambiguous_code_identities: usize,
    best_rank_two: usize,
    best_rank_three_to_six: usize,
    best_rank_after_first_page: usize,
    improved_rank_identities: usize,
    unimproved_rank_identities: usize,
    personalization_identities: [usize; CANDIDATE_PERSONALIZATION_KINDS.len()],
    complete_personalization_identities: usize,
    partial_personalization_identities: usize,
    legacy_personalization_identities: usize,
    complete_provenance_identities: usize,
    partial_provenance_identities: usize,
    missing_provenance_identities: usize,
    dominant_sources: [usize; 10],
    complete_top_provenance_identities: usize,
    partial_top_provenance_identities: usize,
    missing_top_provenance_identities: usize,
    all_observed_top_alias_identities: usize,
    some_observed_top_alias_identities: usize,
    no_observed_top_alias_identities: usize,
    dominant_top_sources: [usize; 10],
}

#[derive(Default)]
struct ResearchReview {
    available_batches: usize,
    batches: usize,
    complete_batches: usize,
    events: usize,
    source_events: usize,
    omitted_events: usize,
    candidate_frames: usize,
    candidate_commits: usize,
    top_one_commits: usize,
    non_top_commits: usize,
    paged_commits: usize,
    manual_commits: usize,
    unpaired_commits: usize,
    raw_commits: usize,
    cancellations: usize,
    post_commit_backspaces_routed: usize,
    shape_frames: usize,
    tab_assembly_frames: usize,
    maximum_loaded_candidates: usize,
    frames_allowing_more_load: usize,
    odd_code_frames: usize,
    long_decoder_primary_frames: usize,
    popup_ms: Vec<u32>,
    initial_popup_ms: Vec<u32>,
    slow_key_total_ms: Vec<u32>,
    slow_key_refresh_ms: Vec<u32>,
    slow_key_planning_ms: Vec<u32>,
    slow_key_edit_session_ms: Vec<u32>,
    slow_key_timing_capable_batches: usize,
    post_commit_backspace_capable_batches: usize,
    precise_personalization_capable_batches: usize,
    source_schema_versions: [usize; WISH_SCHEMA_VERSION_COUNT],
    transposition_accepted: usize,
    transposition_rejected: usize,
    transposition_unknown: usize,
    runtime_batches: HashMap<WishRuntimeIdentity, usize>,
    unidentified_runtime_batches: usize,
    selections: HashMap<(String, String), SelectionPattern>,
    cancelled_codes: HashMap<String, usize>,
    raw_codes: HashMap<String, usize>,
}

impl ResearchReview {
    fn observe(&mut self, snapshot: &WishSnapshot) -> Result<(), Box<dyn Error>> {
        if snapshot.capture_scope() != WishCaptureScope::ContinuousJournal {
            return Err("持续研究目录中出现了非持续批次".into());
        }
        self.batches += 1;
        self.slow_key_timing_capable_batches +=
            usize::from(snapshot.supports_slow_key_path_timing());
        self.post_commit_backspace_capable_batches +=
            usize::from(snapshot.supports_post_commit_backspace_routing());
        self.precise_personalization_capable_batches +=
            usize::from(snapshot.supports_precise_candidate_personalization());
        if let Some(count) = self.source_schema_versions.get_mut(usize::from(
            snapshot.source_schema_version().saturating_sub(1),
        )) {
            *count += 1;
        }
        if let Some(identity) = snapshot.runtime_identity() {
            *self.runtime_batches.entry(identity.clone()).or_insert(0) += 1;
        } else {
            self.unidentified_runtime_batches += 1;
        }
        self.complete_batches += usize::from(snapshot.source_complete());
        self.events += snapshot.events().len();
        self.source_events += snapshot.source_events();
        self.omitted_events += snapshot
            .omitted_before_window()
            .saturating_add(snapshot.omitted_untimed())
            .saturating_add(snapshot.omitted_by_event_limit());
        let mut frame: Option<PresentedFrame> = None;
        for wish_event in snapshot.events() {
            match wish_event.event() {
                NativeFeedbackEvent::CandidatesPresented {
                    code,
                    view,
                    page_start,
                    candidates,
                    may_have_more,
                } => {
                    self.observe_frame(
                        *view,
                        page_start.saturating_add(candidates.len()),
                        *may_have_more,
                        false,
                    );
                    self.observe_code_shape(code, None);
                    frame = Some(PresentedFrame::next(
                        code,
                        *view,
                        *page_start,
                        Vec::new(),
                        frame.as_ref(),
                    ));
                }
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                    code,
                    view,
                    page_start,
                    candidates: _,
                    provenance,
                    loaded_candidates,
                    tab_assembly,
                    may_have_more,
                    ..
                } => {
                    self.observe_frame(
                        *view,
                        *loaded_candidates,
                        *may_have_more,
                        tab_assembly.is_some(),
                    );
                    self.observe_code_shape(code, provenance.first().copied());
                    frame = Some(PresentedFrame::next(
                        code,
                        *view,
                        *page_start,
                        provenance.clone(),
                        frame.as_ref(),
                    ));
                }
                NativeFeedbackEvent::CandidateCommitted {
                    code,
                    text,
                    view,
                    source,
                    absolute_rank,
                    ..
                } => {
                    self.candidate_commits += 1;
                    self.top_one_commits += usize::from(*absolute_rank == 1);
                    self.non_top_commits += usize::from(*absolute_rank > 1);
                    self.paged_commits += usize::from(*absolute_rank > 6);
                    self.manual_commits +=
                        usize::from(*source != NativeSelectionSource::FirstCandidate);
                    let matching_frame = frame
                        .as_ref()
                        .filter(|frame| frame.code == *code && frame.view == *view);
                    let provenance =
                        matching_frame.and_then(|frame| frame.provenance_for_rank(*absolute_rank));
                    let global_top_provenance =
                        matching_frame.and_then(|frame| frame.global_top_provenance);
                    self.unpaired_commits += usize::from(
                        frame
                            .as_ref()
                            .is_none_or(|frame| frame.code != *code || frame.view != *view),
                    );
                    let pattern = self
                        .selections
                        .entry((code.clone(), text.clone()))
                        .or_default();
                    pattern.observe(
                        *absolute_rank,
                        *source,
                        provenance,
                        snapshot.supports_precise_candidate_personalization(),
                    );
                    pattern.observe_global_top_provenance(global_top_provenance);
                    frame = None;
                }
                NativeFeedbackEvent::RawCodeCommitted { code } => {
                    self.raw_commits += 1;
                    *self.raw_codes.entry(code.clone()).or_insert(0) += 1;
                    frame = None;
                }
                NativeFeedbackEvent::CompositionCancelled { code, .. } => {
                    self.cancellations += 1;
                    *self.cancelled_codes.entry(code.clone()).or_insert(0) += 1;
                    frame = None;
                }
                NativeFeedbackEvent::CandidatePopupTiming {
                    fully_visible_ms,
                    initial_show,
                    ..
                } => {
                    self.popup_ms.push(*fully_visible_ms);
                    if *initial_show {
                        self.initial_popup_ms.push(*fully_visible_ms);
                    }
                }
                NativeFeedbackEvent::SlowKeyPathTiming {
                    refresh_ms,
                    planning_ms,
                    edit_session_ms,
                    total_ms,
                } => {
                    self.slow_key_total_ms.push(*total_ms);
                    self.slow_key_refresh_ms.push(*refresh_ms);
                    self.slow_key_planning_ms.push(*planning_ms);
                    self.slow_key_edit_session_ms.push(*edit_session_ms);
                }
                NativeFeedbackEvent::PostCommitBackspaceRouted => {
                    self.post_commit_backspaces_routed += 1;
                    frame = None;
                }
            }
        }
        for observation in snapshot.automatic_transposition_observations()? {
            match observation.label() {
                TranspositionCalibrationLabel::Accepted => self.transposition_accepted += 1,
                TranspositionCalibrationLabel::Rejected => self.transposition_rejected += 1,
                TranspositionCalibrationLabel::Unknown => self.transposition_unknown += 1,
            }
        }
        Ok(())
    }

    fn observe_frame(
        &mut self,
        view: NativeCandidateView,
        loaded_candidates: usize,
        may_have_more: bool,
        tab_assembly: bool,
    ) {
        self.candidate_frames += 1;
        self.shape_frames += usize::from(view == NativeCandidateView::Shape);
        self.tab_assembly_frames += usize::from(tab_assembly);
        self.maximum_loaded_candidates = self.maximum_loaded_candidates.max(loaded_candidates);
        self.frames_allowing_more_load += usize::from(may_have_more);
    }

    fn observe_code_shape(
        &mut self,
        code: &str,
        first_candidate: Option<NativeCandidateProvenance>,
    ) {
        self.odd_code_frames += usize::from(code.len() % 2 == 1);
        self.long_decoder_primary_frames += usize::from(
            code.len() >= 5
                && first_candidate
                    .is_some_and(|candidate| candidate.source() == NativeCandidateSource::Decoder),
        );
    }

    fn render_capability_coverage(&self, output: &mut String) {
        let schemas = self
            .source_schema_versions
            .iter()
            .enumerate()
            .filter(|(_, count)| **count != 0)
            .map(|(index, count)| format!("V{} {}", index + 1, count))
            .collect::<Vec<_>>()
            .join("、");
        writeln!(
            output,
            "反馈格式：{}。",
            if schemas.is_empty() {
                "暂无"
            } else {
                &schemas
            }
        )
        .unwrap();
        writeln!(
            output,
            "诊断能力覆盖：慢按键分段 {}/{} 批；提交后退格 {}/{} 批；精确个性化原因 {}/{} 批。",
            self.slow_key_timing_capable_batches,
            self.batches,
            self.post_commit_backspace_capable_batches,
            self.batches,
            self.precise_personalization_capable_batches,
            self.batches,
        )
        .unwrap();
        writeln!(
            output,
            "{}",
            render_current_schema_readiness(&self.source_schema_versions, self.batches)
        )
        .unwrap();
    }

    fn selection_pressure(&self) -> SelectionPressure {
        let mut by_code: HashMap<&str, Vec<&SelectionPattern>> = HashMap::new();
        for ((code, _), pattern) in &self.selections {
            by_code.entry(code).or_default().push(pattern);
        }
        let mut pressure = SelectionPressure {
            observed_codes: by_code.len(),
            first_non_top_identities: self
                .selections
                .values()
                .filter(|pattern| pattern.first_rank.is_some_and(|rank| rank > 1))
                .count(),
            single_observation_first_non_top_identities: self
                .selections
                .values()
                .filter(|pattern| {
                    pattern.selections == 1 && pattern.first_rank.is_some_and(|rank| rank > 1)
                })
                .count(),
            followup_first_non_top_identities: self
                .selections
                .values()
                .filter(|pattern| {
                    pattern.selections >= 2 && pattern.first_rank.is_some_and(|rank| rank > 1)
                })
                .count(),
            first_non_top_later_top_identities: self
                .selections
                .values()
                .filter(|pattern| {
                    pattern.selections >= 2
                        && pattern.first_rank.is_some_and(|rank| rank > 1)
                        && pattern.minimum_rank == 1
                })
                .count(),
            followup_first_non_top_never_top_identities: self
                .selections
                .values()
                .filter(|pattern| {
                    pattern.selections >= 2
                        && pattern.first_rank.is_some_and(|rank| rank > 1)
                        && pattern.minimum_rank > 1
                })
                .count(),
            first_top_later_non_top_identities: self
                .selections
                .values()
                .filter(|pattern| pattern.first_rank == Some(1) && pattern.non_top_selections != 0)
                .count(),
            ..SelectionPressure::default()
        };
        for patterns in by_code.values() {
            let outputs_with_non_top = patterns
                .iter()
                .filter(|pattern| pattern.non_top_selections != 0)
                .count();
            pressure.multi_output_codes += usize::from(patterns.len() >= 2);
            pressure.competing_non_top_codes += usize::from(outputs_with_non_top >= 2);
            pressure.single_output_non_top_codes +=
                usize::from(patterns.len() == 1 && outputs_with_non_top == 1);
        }
        pressure
    }

    fn render_selection_pressure(&self, output: &mut String) {
        let pressure = self.selection_pressure();
        writeln!(
            output,
            "选择压力（不含文字）：观察码 {}；出现多种已提交文字 {}；至少两种文字都曾从非首选提交 {}；仅一种已见文字且曾从非首选提交 {}。",
            pressure.observed_codes,
            pressure.multi_output_codes,
            pressure.competing_non_top_codes,
            pressure.single_output_non_top_codes,
        )
        .unwrap();
        writeln!(
            output,
            "学习轨迹（不含文字）：首次非首选身份 {}；只提交过一次 {}；有后续提交 {}（其中到过首选 {}、仍未到首选 {}）；首次首选后又出现非首选 {}；原码身份 {}。",
            pressure.first_non_top_identities,
            pressure.single_observation_first_non_top_identities,
            pressure.followup_first_non_top_identities,
            pressure.first_non_top_later_top_identities,
            pressure.followup_first_non_top_never_top_identities,
            pressure.first_top_later_non_top_identities,
            self.raw_codes.len(),
        )
        .unwrap();
        let followup = self.followup_non_top_pressure();
        writeln!(
            output,
            "持续非首选（仅有后续提交且始终未到首选）：身份 {}；同码另有已提交文字 {}；最好第 2 名 {}、第 3–6 名 {}、第 7 名以后 {}；名次曾改善 {}、未改善 {}。",
            followup.identities,
            followup.ambiguous_code_identities,
            followup.best_rank_two,
            followup.best_rank_three_to_six,
            followup.best_rank_after_first_page,
            followup.improved_rank_identities,
            followup.unimproved_rank_identities,
        )
        .unwrap();
        writeln!(
            output,
            "持续非首选证据：候选来源完整 {}、部分 {}、缺失 {}；主要来源 {}。",
            followup.complete_provenance_identities,
            followup.partial_provenance_identities,
            followup.missing_provenance_identities,
            render_candidate_source_counts(&followup.dominant_sources),
        )
        .unwrap();
        writeln!(
            output,
            "持续非首选个性化：精确原因覆盖完整 {}、部分 {}、旧格式 {}；记录到 {}。",
            followup.complete_personalization_identities,
            followup.partial_personalization_identities,
            followup.legacy_personalization_identities,
            render_candidate_personalization_counts(&followup.personalization_identities),
        )
        .unwrap();
        writeln!(
            output,
            "首选阻挡证据：首选来源完整 {}、部分 {}、缺失 {}；已观测首选均为显式别名 {}、部分为显式别名 {}、从未为显式别名 {}；主要首选来源 {}。",
            followup.complete_top_provenance_identities,
            followup.partial_top_provenance_identities,
            followup.missing_top_provenance_identities,
            followup.all_observed_top_alias_identities,
            followup.some_observed_top_alias_identities,
            followup.no_observed_top_alias_identities,
            render_candidate_source_counts(&followup.dominant_top_sources),
        )
        .unwrap();
    }

    fn followup_non_top_pressure(&self) -> FollowupNonTopPressure {
        let mut outputs_by_code: HashMap<&str, usize> = HashMap::new();
        for code in self.selections.keys().map(|(code, _)| code.as_str()) {
            *outputs_by_code.entry(code).or_insert(0) += 1;
        }
        let mut pressure = FollowupNonTopPressure::default();
        for ((code, _), pattern) in self.selections.iter().filter(|(_, pattern)| {
            pattern.selections >= 2
                && pattern.first_rank.is_some_and(|rank| rank > 1)
                && pattern.minimum_rank > 1
        }) {
            pressure.identities += 1;
            pressure.ambiguous_code_identities +=
                usize::from(outputs_by_code.get(code.as_str()).copied().unwrap_or(0) >= 2);
            match pattern.minimum_rank {
                2 => pressure.best_rank_two += 1,
                3..=6 => pressure.best_rank_three_to_six += 1,
                _ => pressure.best_rank_after_first_page += 1,
            }
            if pattern
                .first_rank
                .is_some_and(|first_rank| pattern.minimum_rank < first_rank)
            {
                pressure.improved_rank_identities += 1;
            } else {
                pressure.unimproved_rank_identities += 1;
            }
            for (index, frames) in pattern.personalization_frames.iter().enumerate() {
                pressure.personalization_identities[index] += usize::from(*frames != 0);
            }
            match pattern.precise_personalization_observations {
                0 => pressure.legacy_personalization_identities += 1,
                observations if observations == pattern.selections => {
                    pressure.complete_personalization_identities += 1;
                }
                _ => pressure.partial_personalization_identities += 1,
            }
            match pattern.provenance_observations {
                0 => pressure.missing_provenance_identities += 1,
                observations if observations == pattern.selections => {
                    pressure.complete_provenance_identities += 1;
                    pressure.dominant_sources[candidate_source_index(pattern.dominant_source())] +=
                        1;
                }
                _ => {
                    pressure.partial_provenance_identities += 1;
                    pressure.dominant_sources[candidate_source_index(pattern.dominant_source())] +=
                        1;
                }
            }
            match pattern.top_provenance_observations {
                0 => pressure.missing_top_provenance_identities += 1,
                observations if observations == pattern.selections => {
                    pressure.complete_top_provenance_identities += 1;
                }
                _ => pressure.partial_top_provenance_identities += 1,
            }
            if pattern.top_provenance_observations != 0 {
                pressure.dominant_top_sources
                    [candidate_source_index(pattern.dominant_top_source())] += 1;
                let alias_observations = pattern.top_sources
                    [candidate_source_index(NativeCandidateSource::ExplicitAlias)];
                if alias_observations == 0 {
                    pressure.no_observed_top_alias_identities += 1;
                } else if alias_observations == pattern.top_provenance_observations {
                    pressure.all_observed_top_alias_identities += 1;
                } else {
                    pressure.some_observed_top_alias_identities += 1;
                }
            }
        }
        pressure
    }

    fn render_aggregate(&self) -> String {
        let mut output = String::new();
        writeln!(output, "持续研究摘要（不显示输入原文）").unwrap();
        writeln!(
            output,
            "批次：读取 {} / 可用 {}；完整 {}；事件 {} / 来源 {}，省略 {}。",
            self.batches,
            self.available_batches,
            self.complete_batches,
            self.events,
            self.source_events,
            self.omitted_events,
        )
        .unwrap();
        render_runtime_identities(
            &mut output,
            &self.runtime_batches,
            self.unidentified_runtime_batches,
        );
        self.render_capability_coverage(&mut output);
        writeln!(
            output,
            "提交：{}；首选 {}（{:.1}%）；非首选 {}；翻页后 {}；显式选择 {}；无法配对现场 {}。",
            self.candidate_commits,
            self.top_one_commits,
            percent(self.top_one_commits, self.candidate_commits),
            self.non_top_commits,
            self.paged_commits,
            self.manual_commits,
            self.unpaired_commits,
        )
        .unwrap();
        self.render_selection_pressure(&mut output);
        writeln!(
            output,
            "候选：{} 帧；最大已加载深度 {}；仍可继续加载 {} 帧；Tab 找字 {} 帧（逐字组词 {} 帧）。",
            self.candidate_frames,
            self.maximum_loaded_candidates,
            self.frames_allowing_more_load,
            self.shape_frames,
            self.tab_assembly_frames,
        )
        .unwrap();
        writeln!(
            output,
            "其他结束：原码上屏 {}；取消 {}；提交后紧接退格 {}（已交给宿主，结果未观测）。",
            self.raw_commits, self.cancellations, self.post_commit_backspaces_routed,
        )
        .unwrap();
        render_latency(&mut output, "候选窗完全显示", &self.popup_ms);
        render_latency(&mut output, "首次出现", &self.initial_popup_ms);
        render_latency(
            &mut output,
            "慢按键总耗时（仅 ≥16 ms）",
            &self.slow_key_total_ms,
        );
        render_latency(&mut output, "慢按键刷新阶段", &self.slow_key_refresh_ms);
        render_latency(
            &mut output,
            "慢按键候选规划阶段",
            &self.slow_key_planning_ms,
        );
        render_latency(
            &mut output,
            "慢按键编辑会话阶段",
            &self.slow_key_edit_session_ms,
        );
        render_slow_key_coverage(
            &mut output,
            &self.slow_key_total_ms,
            self.slow_key_timing_capable_batches,
            self.batches,
        );
        writeln!(
            output,
            "自动换序标签：采用 {}；未采用 {}；证据不足 {}。",
            self.transposition_accepted, self.transposition_rejected, self.transposition_unknown,
        )
        .unwrap();
        output.push_str(
            "口径：只读解密本地持续批次；只显示聚合数字，不显示原码、候选或提交文字；没有写模型、修改排序、联网或导出文件。",
        );
        output
    }

    fn render(&self) -> String {
        let mut output = String::new();
        writeln!(output, "持续研究回顾（包含私人输入）").unwrap();
        writeln!(
            output,
            "批次：读取 {} / 可用 {}；完整 {}；事件 {} / 来源 {}，省略 {}。",
            self.batches,
            self.available_batches,
            self.complete_batches,
            self.events,
            self.source_events,
            self.omitted_events,
        )
        .unwrap();
        render_runtime_identities(
            &mut output,
            &self.runtime_batches,
            self.unidentified_runtime_batches,
        );
        self.render_capability_coverage(&mut output);
        writeln!(
            output,
            "提交：{}；首选 {}（{:.1}%）；非首选 {}；翻页后 {}；显式选择 {}；无法配对现场 {}。",
            self.candidate_commits,
            self.top_one_commits,
            percent(self.top_one_commits, self.candidate_commits),
            self.non_top_commits,
            self.paged_commits,
            self.manual_commits,
            self.unpaired_commits,
        )
        .unwrap();
        writeln!(
            output,
            "候选：{} 帧；最大已加载深度 {}；仍可继续加载 {} 帧；Tab 找字 {} 帧（逐字组词 {} 帧）。",
            self.candidate_frames,
            self.maximum_loaded_candidates,
            self.frames_allowing_more_load,
            self.shape_frames,
            self.tab_assembly_frames,
        )
        .unwrap();
        writeln!(
            output,
            "其他结束：原码上屏 {}；取消 {}；提交后紧接退格 {}（已交给宿主，结果未观测）。",
            self.raw_commits, self.cancellations, self.post_commit_backspaces_routed,
        )
        .unwrap();
        render_latency(&mut output, "候选窗完全显示", &self.popup_ms);
        render_latency(&mut output, "首次出现", &self.initial_popup_ms);
        render_latency(
            &mut output,
            "慢按键总耗时（仅 ≥16 ms）",
            &self.slow_key_total_ms,
        );
        render_latency(&mut output, "慢按键刷新阶段", &self.slow_key_refresh_ms);
        render_latency(
            &mut output,
            "慢按键候选规划阶段",
            &self.slow_key_planning_ms,
        );
        render_latency(
            &mut output,
            "慢按键编辑会话阶段",
            &self.slow_key_edit_session_ms,
        );
        render_slow_key_coverage(
            &mut output,
            &self.slow_key_total_ms,
            self.slow_key_timing_capable_batches,
            self.batches,
        );
        writeln!(
            output,
            "自动换序标签：采用 {}；未采用 {}；证据不足 {}。",
            self.transposition_accepted, self.transposition_rejected, self.transposition_unknown,
        )
        .unwrap();

        let mut non_top = self
            .selections
            .iter()
            .filter(|(_, pattern)| pattern.non_top_selections > 0)
            .collect::<Vec<_>>();
        non_top.sort_by(|left, right| {
            right
                .1
                .non_top_selections
                .cmp(&left.1.non_top_selections)
                .then_with(|| right.1.maximum_rank.cmp(&left.1.maximum_rank))
                .then_with(|| left.0.cmp(right.0))
        });
        output.push_str("\n需要复查的非首选提交（不是自动判错）：\n");
        if non_top.is_empty() {
            output.push_str("- 暂无。\n");
        }
        for ((code, text), pattern) in non_top.into_iter().take(MAX_REVIEW_ITEMS) {
            writeln!(
                output,
                "- {code} → “{text}”：非首选 {}/{} 次；名次 {}–{}；显式选择 {} 次；翻页 {} 次；来源 {}；个性化机制 {}。",
                pattern.non_top_selections,
                pattern.selections,
                pattern.minimum_rank,
                pattern.maximum_rank,
                pattern.manual_selections,
                pattern.paged_selections,
                candidate_source_label(pattern.dominant_source()),
                render_candidate_personalization_counts(&pattern.personalization_frames),
            )
            .unwrap();
        }

        let mut learned = self
            .selections
            .iter()
            .filter(|(_, pattern)| {
                pattern.selections >= 2
                    && pattern.first_rank.is_some_and(|rank| rank > 1)
                    && pattern.last_rank == Some(1)
            })
            .collect::<Vec<_>>();
        learned.sort_by(|left, right| {
            right
                .1
                .selections
                .cmp(&left.1.selections)
                .then_with(|| left.0.cmp(right.0))
        });
        output.push_str("\n观察到“先选后升为首选”的身份：\n");
        if learned.is_empty() {
            output.push_str("- 暂无。\n");
        }
        for ((code, text), pattern) in learned.into_iter().take(MAX_REVIEW_ITEMS) {
            writeln!(
                output,
                "- {code} → “{text}”：首次第 {}，最近第 1；共提交 {} 次。",
                pattern.first_rank.unwrap_or(1),
                pattern.selections,
            )
            .unwrap();
        }

        render_code_counts(
            &mut output,
            "反复取消的组合（可能是修改、放弃或找字）",
            &self.cancelled_codes,
        );
        render_code_counts(&mut output, "原码上屏", &self.raw_codes);
        output.push_str(
            "\n口径：只读解密本地持续批次；没有写模型、修改排序、联网或导出文件。\n\
             非首选、取消和原码上屏只是待复查线索，不自动等于输入错误。",
        );
        output
    }

    fn render_runtime_summary(
        &self,
        identity: &WishRuntimeIdentity,
        half_pairs: &ResearchHalfPairAnalysis,
    ) -> String {
        let mut output = String::new();
        writeln!(output, "最新运行身份（与旧批次分开）").unwrap();
        writeln!(
            output,
            "DLL {}…；核心 {}；补充 {}。",
            &identity.module_sha256()[..12],
            identity.core_candidate_revision(),
            identity.supplemental_candidate_revision().unwrap_or("无"),
        )
        .unwrap();
        writeln!(
            output,
            "批次：{}；完整 {}；事件 {}，省略 {}。",
            self.batches, self.complete_batches, self.events, self.omitted_events,
        )
        .unwrap();
        self.render_capability_coverage(&mut output);
        writeln!(
            output,
            "提交：{}；首选 {}（{:.1}%）；非首选 {}；翻页后 {}；取消 {}；原码上屏 {}；提交后紧接退格 {}。",
            self.candidate_commits,
            self.top_one_commits,
            percent(self.top_one_commits, self.candidate_commits),
            self.non_top_commits,
            self.paged_commits,
            self.cancellations,
            self.raw_commits,
            self.post_commit_backspaces_routed,
        )
        .unwrap();
        self.render_selection_pressure(&mut output);
        writeln!(
            output,
            "候选：{} 帧；奇数键中间态 {} 帧；五码及以上且首选来自普通组合 {} 帧。",
            self.candidate_frames, self.odd_code_frames, self.long_decoder_primary_frames,
        )
        .unwrap();
        writeln!(
            output,
            "双拼暂态配对：{}；间隔 {}。",
            half_pairs.paired_frames(),
            render_half_pair_histogram(half_pairs.gap_histogram()),
        )
        .unwrap();
        writeln!(
            output,
            "暂态变化：首选改变 {} / {}（{:.1}%）；原可见项保留 {} / {}（{:.1}%）；完成帧首选仍来自普通组合 {} / {}。",
            half_pairs.top_candidate_changes(),
            half_pairs.top_candidate_comparisons(),
            percent(
                half_pairs.top_candidate_changes(),
                half_pairs.top_candidate_comparisons(),
            ),
            half_pairs.retained_candidates(),
            half_pairs.candidate_slots_before(),
            percent(
                half_pairs.retained_candidates(),
                half_pairs.candidate_slots_before(),
            ),
            half_pairs.decoder_top_after_completion(),
            half_pairs.provenance_comparisons(),
        )
        .unwrap();
        render_latency(&mut output, "候选窗完全显示", &self.popup_ms);
        render_latency(&mut output, "首次出现", &self.initial_popup_ms);
        render_latency(
            &mut output,
            "慢按键总耗时（仅 ≥16 ms）",
            &self.slow_key_total_ms,
        );
        render_slow_key_coverage(
            &mut output,
            &self.slow_key_total_ms,
            self.slow_key_timing_capable_batches,
            self.batches,
        );
        writeln!(
            output,
            "自动换序标签：采用 {}；未采用 {}；证据不足 {}。",
            self.transposition_accepted, self.transposition_rejected, self.transposition_unknown,
        )
        .unwrap();
        output.push_str("口径：这里只汇总时间上最新的已标识 DLL；首选提交不自动等于文字正确。");
        output
    }
}

fn render_half_pair_histogram(histogram: &[usize; 9]) -> String {
    let labels = [
        "<8", "8–15", "16–23", "24–31", "32–47", "48–63", "64–95", "96–159", "≥160",
    ];
    labels
        .into_iter()
        .zip(histogram)
        .map(|(label, count)| format!("{label} ms {count}"))
        .collect::<Vec<_>>()
        .join("；")
}

fn latest_runtime_review(
    snapshots: &[WishSnapshot],
) -> Result<Option<(WishRuntimeIdentity, ResearchReview)>, Box<dyn Error>> {
    let Some(identity) = snapshots
        .iter()
        .rev()
        .find_map(|snapshot| snapshot.runtime_identity().cloned())
    else {
        return Ok(None);
    };
    let mut review = ResearchReview::default();
    for snapshot in snapshots
        .iter()
        .filter(|snapshot| snapshot.runtime_identity() == Some(&identity))
    {
        review.observe(snapshot)?;
    }
    review.available_batches = review.batches;
    Ok(Some((identity, review)))
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn percentile(values: &[u32], percentile: usize) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len() - 1).saturating_mul(percentile) / 100;
    sorted.get(index).copied()
}

fn render_latency(output: &mut String, label: &str, values: &[u32]) {
    if let (Some(p50), Some(p95), Some(maximum)) = (
        percentile(values, 50),
        percentile(values, 95),
        values.iter().max().copied(),
    ) {
        writeln!(
            output,
            "{label}：{} 次；P50 {p50} ms，P95 {p95} ms，最大 {maximum} ms。",
            values.len()
        )
        .unwrap();
    } else {
        writeln!(output, "{label}：暂无样本。").unwrap();
    }
}

fn render_slow_key_coverage(
    output: &mut String,
    values: &[u32],
    capable_batches: usize,
    total_batches: usize,
) {
    let label = if total_batches == 0 {
        "未确认；没有可分析批次。".to_owned()
    } else if capable_batches == 0 {
        format!("未确认；0/{total_batches} 批支持该字段。")
    } else if values.is_empty() && capable_batches == total_batches {
        format!("已确认；{capable_batches}/{total_batches} 批均支持该字段，未观察到 ≥16 ms 按键。")
    } else if values.is_empty() {
        format!(
            "部分确认；{capable_batches}/{total_batches} 批支持且未观察到 ≥16 ms 按键，其余批次无法判断。"
        )
    } else if capable_batches == total_batches {
        format!("已确认；{capable_batches}/{total_batches} 批均支持，记录中存在分阶段耗时。")
    } else {
        format!(
            "部分确认；{capable_batches}/{total_batches} 批支持，记录中存在分阶段耗时，其余批次无法判断。"
        )
    };
    writeln!(output, "慢按键分段覆盖：{label}").unwrap();
}

fn render_current_schema_readiness(
    source_schema_versions: &[usize; WISH_SCHEMA_VERSION_COUNT],
    total_batches: usize,
) -> String {
    let current_batches = source_schema_versions
        .get(WISH_SCHEMA_VERSION_COUNT.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    if total_batches == 0 {
        return format!("当前 V{CURRENT_WISH_SCHEMA_VERSION} 采集就绪：无法判断；暂无可分析批次。");
    }
    if current_batches == total_batches {
        return format!(
            "当前 V{CURRENT_WISH_SCHEMA_VERSION} 采集就绪：已确认；{current_batches}/{total_batches} 批均为当前格式。"
        );
    }
    if current_batches != 0 {
        return format!(
            "当前 V{CURRENT_WISH_SCHEMA_VERSION} 采集就绪：部分确认；{current_batches}/{total_batches} 批为当前格式，其余历史批次不会补写。"
        );
    }
    let Some(highest_schema) = source_schema_versions
        .iter()
        .rposition(|count| *count != 0)
        .map(|index| index + 1)
    else {
        return format!(
            "当前 V{CURRENT_WISH_SCHEMA_VERSION} 采集就绪：无法判断；批次数量与格式分布不一致。"
        );
    };
    format!(
        "当前 V{CURRENT_WISH_SCHEMA_VERSION} 采集就绪：尚未在记录中观察到；这些批次最高为 V{highest_schema}，无法证明全部当前诊断字段均已上线。历史批次不会补写。"
    )
}

fn render_code_counts(output: &mut String, title: &str, counts: &HashMap<String, usize>) {
    let mut counts = counts.iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    writeln!(output, "\n{title}：").unwrap();
    if counts.is_empty() {
        output.push_str("- 暂无。\n");
    }
    for (code, count) in counts.into_iter().take(MAX_REVIEW_ITEMS) {
        writeln!(output, "- {code}：{count} 次。").unwrap();
    }
}

fn render_runtime_identities(
    output: &mut String,
    identities: &HashMap<WishRuntimeIdentity, usize>,
    unidentified_batches: usize,
) {
    let mut identities = identities.iter().collect::<Vec<_>>();
    identities.sort_by(|left, right| {
        right
            .1
            .cmp(left.1)
            .then_with(|| {
                left.0
                    .core_candidate_revision()
                    .cmp(right.0.core_candidate_revision())
            })
            .then_with(|| left.0.module_sha256().cmp(right.0.module_sha256()))
    });
    writeln!(
        output,
        "运行身份：已标识 {} 组、{} 批；旧批次未记录 {} 批。",
        identities.len(),
        identities.iter().map(|(_, count)| **count).sum::<usize>(),
        unidentified_batches,
    )
    .unwrap();
    for (identity, count) in identities.into_iter().take(MAX_REVIEW_ITEMS) {
        writeln!(
            output,
            "- DLL {}…；核心 {}；补充 {}：{} 批。",
            &identity.module_sha256()[..12],
            identity.core_candidate_revision(),
            identity.supplemental_candidate_revision().unwrap_or("无"),
            count,
        )
        .unwrap();
    }
}

fn render_scene_analysis(analysis: &ResearchSceneAnalysis) -> String {
    let mut output = String::new();
    output.push_str("自然片段（不把加密批次当作段落边界）\n");
    if analysis.linked_batches() == 0 {
        output.push_str("- 现有批次尚无连续链；换代后的 V8 批次开始积累后才进行分段。\n");
    } else {
        writeln!(
            output,
            "- {} 个有链批次，{} 条进程内连续流，{} 次完成输入，组成 {} 个自然片段。",
            analysis.linked_batches(),
            analysis.linked_streams(),
            analysis.episodes(),
            analysis.scenes(),
        )
        .unwrap();
        writeln!(
            output,
            "- 自适应停顿边界 {} ms；片段中位 {} 次输入 / {} ms；链缺口 {}。",
            analysis.gap_threshold_ms(),
            analysis.median_episodes_per_scene(),
            analysis.median_scene_duration_ms(),
            analysis.chain_breaks(),
        )
        .unwrap();
    }
    writeln!(
        output,
        "- 许愿：有锚点 {}，已连接片段 {}，旧版或未锚定 {}。",
        analysis.anchored_wishes(),
        analysis.linked_wishes(),
        analysis.unanchored_wishes(),
    )
    .unwrap();

    output.push_str("手癖线索（不是自动判错）：\n");
    if analysis.habit_clues().is_empty() {
        output.push_str("- 暂无达到证据门槛的线索。\n");
    }
    for clue in analysis.habit_clues().iter().take(MAX_REVIEW_ITEMS) {
        match clue.kind() {
            ResearchHabitKind::AcceptedTransposition => {
                writeln!(
                    output,
                    "- {} → “{}”：实际采用自动换序 {} 次；按键间隔中位 {} ms。",
                    clue.observed_code(),
                    clue.committed_text(),
                    clue.observations(),
                    clue.median_pair_gap_ms().unwrap_or(0),
                )
                .unwrap();
            }
            ResearchHabitKind::RepeatedCodeRevision => {
                writeln!(
                    output,
                    "- {} → {} → “{}”：输入中改码 {} 次；保留为待确认线索。",
                    clue.observed_code(),
                    clue.resulting_code(),
                    clue.committed_text(),
                    clue.observations(),
                )
                .unwrap();
            }
        }
    }
    output.push_str(
        "口径：失焦/宿主结束是硬边界；停顿是按本批证据自适应的软边界；单次改码不形成手癖结论。",
    );
    output
}

fn candidate_source_index(source: NativeCandidateSource) -> usize {
    match source {
        NativeCandidateSource::Unknown => 0,
        NativeCandidateSource::ExplicitAlias => 1,
        NativeCandidateSource::ProjectOverlay => 2,
        NativeCandidateSource::CoreExact => 3,
        NativeCandidateSource::SupplementalExact => 4,
        NativeCandidateSource::CharacterPair => 5,
        NativeCandidateSource::Decoder => 6,
        NativeCandidateSource::TranspositionRecovery => 7,
        NativeCandidateSource::Shape => 8,
        NativeCandidateSource::FourCharacterCorrection => 9,
    }
}

fn candidate_source_from_index(index: usize) -> NativeCandidateSource {
    match index {
        1 => NativeCandidateSource::ExplicitAlias,
        2 => NativeCandidateSource::ProjectOverlay,
        3 => NativeCandidateSource::CoreExact,
        4 => NativeCandidateSource::SupplementalExact,
        5 => NativeCandidateSource::CharacterPair,
        6 => NativeCandidateSource::Decoder,
        7 => NativeCandidateSource::TranspositionRecovery,
        8 => NativeCandidateSource::Shape,
        9 => NativeCandidateSource::FourCharacterCorrection,
        _ => NativeCandidateSource::Unknown,
    }
}

fn candidate_source_label(source: NativeCandidateSource) -> &'static str {
    match source {
        NativeCandidateSource::Unknown => "未知",
        NativeCandidateSource::ExplicitAlias => "显式别名",
        NativeCandidateSource::ProjectOverlay => "项目词",
        NativeCandidateSource::CoreExact => "核心整词",
        NativeCandidateSource::SupplementalExact => "补充整词/组合",
        NativeCandidateSource::CharacterPair => "双字自由组合",
        NativeCandidateSource::Decoder => "普通组合",
        NativeCandidateSource::TranspositionRecovery => "自动换序",
        NativeCandidateSource::Shape => "Tab 找字",
        NativeCandidateSource::FourCharacterCorrection => "四字纠错",
    }
}

fn render_candidate_source_counts(counts: &[usize; 10]) -> String {
    let rendered = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .map(|(index, count)| {
            format!(
                "{} {}",
                candidate_source_label(candidate_source_from_index(index)),
                count
            )
        })
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "暂无".to_owned()
    } else {
        rendered.join("、")
    }
}

fn render_candidate_personalization_counts(
    counts: &[usize; CANDIDATE_PERSONALIZATION_KINDS.len()],
) -> String {
    let rendered = counts
        .iter()
        .zip(CANDIDATE_PERSONALIZATION_KINDS)
        .filter(|(count, _)| **count != 0)
        .map(|(count, (_, label))| format!("{label} {count}"))
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "未观察到".to_owned()
    } else {
        rendered.join("、")
    }
}

fn print_status(root: &Path) -> Result<(), Box<dyn Error>> {
    let enabled = research_feedback_enabled(root)?;
    let packages = list_wish_packages(root)?.len();
    println!(
        "持续研究：{}\n已保存加密批次：{packages}\n原文显示：没有\n网络：没有",
        if enabled { "已开启" } else { "已关闭" }
    );
    Ok(())
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref() {
        Some("status") => Command::Status,
        Some("enable") => Command::Enable,
        Some("disable") => Command::Disable,
        Some("summary") => Command::Summary,
        Some("review") => Command::Review,
        _ => return Err(usage().into()),
    };
    let mut root = None;
    let mut confirmed = false;
    let mut read_private_feedback = false;
    let mut show_private_text = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(arguments.next().ok_or("--root 后缺少目录")?));
            }
            "--confirm-continuous-private-feedback" if !confirmed => confirmed = true,
            "--confirm-read-private-feedback" if !read_private_feedback => {
                read_private_feedback = true
            }
            "--confirm-show-private-text" if !show_private_text => show_private_text = true,
            _ => return Err(usage().into()),
        }
    }
    if command != Command::Enable && confirmed {
        return Err(usage().into());
    }
    if command != Command::Summary && read_private_feedback {
        return Err(usage().into());
    }
    if command != Command::Review && show_private_text {
        return Err(usage().into());
    }
    Ok(Options {
        command,
        root,
        confirmed,
        read_private_feedback,
        show_private_text,
    })
}

fn research_root_for_executable(executable: &Path) -> Option<PathBuf> {
    let repository = repository_root_for_user_tool_executable(executable, "researchctl")?;
    Some(
        repository
            .join(".local")
            .join("tsf-alpha")
            .join("user-data")
            .join(RESEARCH_FEEDBACK_DIRECTORY),
    )
}

fn usage() -> String {
    "用法：\n  researchctl status [--root <目录>]\n  researchctl summary \
     --confirm-read-private-feedback [--root <目录>]\n  researchctl review \
     --confirm-show-private-text [--root <目录>]\n  researchctl enable \
     --confirm-continuous-private-feedback [--root <目录>]\n  researchctl disable [--root <目录>]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_parallel_map_preserves_order_and_propagates_errors() {
        let inputs = (0_u32..96).collect::<Vec<_>>();
        assert_eq!(
            ordered_bounded_map(&inputs, 4, |value| Ok::<_, u32>(value * 3)).unwrap(),
            inputs.iter().map(|value| value * 3).collect::<Vec<_>>()
        );
        assert_eq!(
            ordered_bounded_map(&inputs, 4, |value| {
                if *value == 37 {
                    Err(*value)
                } else {
                    Ok(*value)
                }
            }),
            Err(37)
        );
        assert_eq!(
            ordered_bounded_map(&inputs[..8], 4, |value| Ok::<_, u32>(*value)).unwrap(),
            inputs[..8]
        );
    }

    #[test]
    fn parser_requires_the_private_capture_confirmation_only_for_enable() {
        assert_eq!(
            parse_options(["status".to_owned()]).unwrap(),
            Options {
                command: Command::Status,
                root: None,
                confirmed: false,
                read_private_feedback: false,
                show_private_text: false,
            }
        );
        assert!(!parse_options(["enable".to_owned()]).unwrap().confirmed);
        assert!(
            parse_options([
                "enable".to_owned(),
                "--confirm-continuous-private-feedback".to_owned(),
            ])
            .unwrap()
            .confirmed
        );
        assert!(
            parse_options([
                "disable".to_owned(),
                "--confirm-continuous-private-feedback".to_owned(),
            ])
            .is_err()
        );
        assert!(
            !parse_options(["summary".to_owned()])
                .unwrap()
                .read_private_feedback
        );
        let summary = parse_options([
            "summary".to_owned(),
            "--confirm-read-private-feedback".to_owned(),
        ])
        .unwrap();
        assert_eq!(summary.command, Command::Summary);
        assert!(summary.read_private_feedback);
        assert!(
            parse_options([
                "review".to_owned(),
                "--confirm-read-private-feedback".to_owned(),
            ])
            .is_err()
        );
        let review = parse_options([
            "review".to_owned(),
            "--confirm-show-private-text".to_owned(),
        ])
        .unwrap();
        assert_eq!(review.command, Command::Review);
        assert!(review.show_private_text);
    }

    #[test]
    fn aggregate_selection_pressure_distinguishes_competing_and_cold_identities() {
        let mut review = ResearchReview::default();
        for (code, text, ranks) in [
            ("same", "alpha", vec![2, 1, 2]),
            ("same", "beta", vec![2]),
            ("cold", "gamma", vec![3, 2]),
            ("easy", "delta", vec![1, 2]),
        ] {
            let pattern = review
                .selections
                .entry((code.to_owned(), text.to_owned()))
                .or_default();
            for rank in ranks {
                pattern.observe(rank, NativeSelectionSource::Numeric, None, false);
            }
        }

        let pressure = review.selection_pressure();
        assert_eq!(
            pressure,
            SelectionPressure {
                observed_codes: 3,
                multi_output_codes: 1,
                competing_non_top_codes: 1,
                single_output_non_top_codes: 2,
                first_non_top_identities: 3,
                single_observation_first_non_top_identities: 1,
                followup_first_non_top_identities: 2,
                first_non_top_later_top_identities: 1,
                followup_first_non_top_never_top_identities: 1,
                first_top_later_non_top_identities: 1,
            }
        );
        assert_eq!(
            pressure.first_non_top_identities,
            pressure.single_observation_first_non_top_identities
                + pressure.followup_first_non_top_identities
        );
        assert_eq!(
            pressure.followup_first_non_top_identities,
            pressure.first_non_top_later_top_identities
                + pressure.followup_first_non_top_never_top_identities
        );
        let mut rendered = String::new();
        review.render_selection_pressure(&mut rendered);
        assert!(rendered.contains("至少两种文字都曾从非首选提交 1"));
        assert!(rendered.contains("只提交过一次 1"));
        assert!(rendered.contains("有后续提交 2（其中到过首选 1、仍未到首选 1）"));
        assert!(rendered.contains("首次首选后又出现非首选 1"));
        for private_value in ["same", "alpha", "beta", "cold", "gamma"] {
            assert!(!rendered.contains(private_value));
        }
    }

    #[test]
    fn presented_frame_carries_global_top_source_only_across_matching_pages() {
        let alias = NativeCandidateProvenance::new(NativeCandidateSource::ExplicitAlias, false);
        let core = NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false);
        let first = PresentedFrame::next(
            "code",
            NativeCandidateView::Ordinary,
            0,
            vec![alias, core],
            None,
        );
        assert_eq!(first.global_top_provenance, Some(alias));
        let second = PresentedFrame::next(
            "code",
            NativeCandidateView::Ordinary,
            6,
            vec![core],
            Some(&first),
        );
        assert_eq!(second.global_top_provenance, Some(alias));
        let changed_code = PresentedFrame::next(
            "other",
            NativeCandidateView::Ordinary,
            6,
            vec![core],
            Some(&second),
        );
        assert_eq!(changed_code.global_top_provenance, None);
        let changed_view = PresentedFrame::next(
            "code",
            NativeCandidateView::Shape,
            6,
            vec![core],
            Some(&second),
        );
        assert_eq!(changed_view.global_top_provenance, None);
        let refreshed_first = PresentedFrame::next(
            "code",
            NativeCandidateView::Ordinary,
            0,
            vec![core],
            Some(&second),
        );
        assert_eq!(refreshed_first.global_top_provenance, Some(core));
    }

    #[test]
    fn followup_non_top_pressure_partitions_only_supported_evidence() {
        let alias = NativeCandidateProvenance::new(NativeCandidateSource::ExplicitAlias, false);
        let core = NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false);
        let promoted_core = NativeCandidateProvenance::with_personalization(
            NativeCandidateSource::CoreExact,
            NativeCandidatePersonalization::PERSISTENT_EXACT
                .with(NativeCandidatePersonalization::SESSION_EXACT),
        );
        let supplemental =
            NativeCandidateProvenance::new(NativeCandidateSource::SupplementalExact, false);
        let mut review = ResearchReview::default();
        for (code, text, observations) in [
            (
                "same",
                "alpha",
                vec![(4, Some(core)), (2, Some(promoted_core))],
            ),
            ("same", "beta", vec![(1, Some(core))]),
            ("cold", "gamma", vec![(3, None), (3, None)]),
            ("far", "zeta", vec![(8, None), (7, Some(supplemental))]),
            ("partial", "theta", vec![(5, Some(core)), (4, Some(core))]),
            ("missing", "iota", vec![(2, Some(core)), (2, Some(core))]),
            ("learned", "eta", vec![(2, Some(core)), (1, Some(core))]),
        ] {
            let pattern = review
                .selections
                .entry((code.to_owned(), text.to_owned()))
                .or_default();
            for (rank, provenance) in observations {
                pattern.observe(rank, NativeSelectionSource::Numeric, provenance, true);
            }
        }
        for (identity, top_sources) in [
            (("same", "alpha"), vec![alias, alias]),
            (("cold", "gamma"), vec![alias, core]),
            (("far", "zeta"), vec![core, core]),
            (("partial", "theta"), vec![core]),
        ] {
            let pattern = review
                .selections
                .get_mut(&(identity.0.to_owned(), identity.1.to_owned()))
                .unwrap();
            for source in top_sources {
                pattern.observe_global_top_provenance(Some(source));
            }
        }

        let mut dominant_sources = [0; 10];
        dominant_sources[candidate_source_index(NativeCandidateSource::CoreExact)] = 3;
        dominant_sources[candidate_source_index(NativeCandidateSource::SupplementalExact)] = 1;
        let mut dominant_top_sources = [0; 10];
        dominant_top_sources[candidate_source_index(NativeCandidateSource::ExplicitAlias)] = 2;
        dominant_top_sources[candidate_source_index(NativeCandidateSource::CoreExact)] = 2;
        let mut personalization_identities = [0; CANDIDATE_PERSONALIZATION_KINDS.len()];
        personalization_identities[0] = 1;
        personalization_identities[3] = 1;
        assert_eq!(
            review.followup_non_top_pressure(),
            FollowupNonTopPressure {
                identities: 5,
                ambiguous_code_identities: 1,
                best_rank_two: 2,
                best_rank_three_to_six: 2,
                best_rank_after_first_page: 1,
                improved_rank_identities: 3,
                unimproved_rank_identities: 2,
                personalization_identities,
                complete_personalization_identities: 5,
                partial_personalization_identities: 0,
                legacy_personalization_identities: 0,
                complete_provenance_identities: 3,
                partial_provenance_identities: 1,
                missing_provenance_identities: 1,
                dominant_sources,
                complete_top_provenance_identities: 3,
                partial_top_provenance_identities: 1,
                missing_top_provenance_identities: 1,
                all_observed_top_alias_identities: 1,
                some_observed_top_alias_identities: 1,
                no_observed_top_alias_identities: 2,
                dominant_top_sources,
            }
        );
        let mut rendered = String::new();
        review.render_selection_pressure(&mut rendered);
        assert!(rendered.contains("最好第 2 名 2、第 3–6 名 2、第 7 名以后 1"));
        assert!(rendered.contains("候选来源完整 3、部分 1、缺失 1"));
        assert!(rendered.contains("精确原因覆盖完整 5、部分 0、旧格式 0"));
        assert!(rendered.contains("记录到 持久精确 1、会话精确 1"));
        assert!(rendered.contains("主要来源 核心整词 3、补充整词/组合 1"));
        assert!(rendered.contains("首选来源完整 3、部分 1、缺失 1"));
        assert!(rendered.contains("主要首选来源 显式别名 2、核心整词 2"));
        for private_value in [
            "same", "alpha", "cold", "gamma", "far", "zeta", "partial", "theta", "missing", "iota",
        ] {
            assert!(!rendered.contains(private_value));
        }
    }

    #[test]
    fn review_separates_non_top_selection_from_later_personal_promotion() {
        use ziranma_core::{
            NativeFeedbackAuthorization, NativeFeedbackContext, NativeFeedbackFreezeAuthorization,
            NativeFeedbackLimits, NativeFeedbackRecordResult, NativeFeedbackSession,
        };

        let mut session = NativeFeedbackSession::default();
        session.start_memory(
            NativeFeedbackAuthorization::explicit_memory_only(),
            NativeFeedbackLimits::default(),
        );
        let events = [
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code: "dago".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["大国".to_owned(), "打过".to_owned()],
                provenance: vec![
                    NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false),
                    NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false),
                ],
                automatic_transposition: None,
                loaded_candidates: 2,
                tab_assembly: None,
                may_have_more: false,
            },
            NativeFeedbackEvent::CandidateCommitted {
                code: "dago".to_owned(),
                text: "打过".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::Numeric,
                absolute_rank: 2,
                visible_rank: 2,
            },
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code: "dago".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["打过".to_owned(), "大国".to_owned()],
                provenance: vec![
                    NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, true),
                    NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false),
                ],
                automatic_transposition: None,
                loaded_candidates: 2,
                tab_assembly: None,
                may_have_more: false,
            },
            NativeFeedbackEvent::CandidateCommitted {
                code: "dago".to_owned(),
                text: "打过".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
        ];
        for (index, event) in events.into_iter().enumerate() {
            assert_eq!(
                session.record_at(
                    NativeFeedbackContext::Eligible,
                    event,
                    u64::try_from(index + 1).unwrap(),
                ),
                NativeFeedbackRecordResult::Recorded
            );
        }
        let frozen = session
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                10,
                30_000,
                16,
            )
            .unwrap();
        let snapshot = WishSnapshot::from_frozen_with_runtime_identity(
            &frozen,
            WishCaptureScope::ContinuousJournal,
            ziranma_core::WishCategory::Other,
            Some(
                WishRuntimeIdentity::new("ab".repeat(32), "research-core-v1".to_owned(), None)
                    .unwrap(),
            ),
        )
        .unwrap();
        let mut review = ResearchReview::default();
        review.observe(&snapshot).unwrap();

        assert_eq!(review.candidate_commits, 2);
        assert_eq!(review.non_top_commits, 1);
        let pattern = review
            .selections
            .get(&("dago".to_owned(), "打过".to_owned()))
            .unwrap();
        assert_eq!(pattern.first_rank, Some(2));
        assert_eq!(pattern.last_rank, Some(1));
        assert_eq!(pattern.personalization_frames[3], 1);
        assert_eq!(pattern.top_provenance_observations, 2);
        assert_eq!(
            pattern.top_sources[candidate_source_index(NativeCandidateSource::CoreExact)],
            2
        );
        assert!(review.render().contains("首次第 2，最近第 1"));
        let aggregate = review.render_aggregate();
        assert!(aggregate.contains("持续研究摘要（不显示输入原文）"));
        assert!(!aggregate.contains("dago"));
        assert!(!aggregate.contains("大国"));
        assert!(!aggregate.contains("打过"));
        assert!(!aggregate.contains("需要复查"));
        assert!(aggregate.contains("慢按键分段覆盖：已确认"));
        assert!(aggregate.contains("精确个性化原因 1/1 批"));
        assert!(aggregate.contains("反馈格式：V11 1"));
        assert!(
            review
                .render()
                .contains("DLL abababababab…；核心 research-core-v1")
        );

        let newer = WishSnapshot::from_frozen_with_runtime_identity(
            &frozen,
            WishCaptureScope::ContinuousJournal,
            ziranma_core::WishCategory::Other,
            Some(
                WishRuntimeIdentity::new("cd".repeat(32), "research-core-v2".to_owned(), None)
                    .unwrap(),
            ),
        )
        .unwrap();
        let snapshots = [snapshot, newer];
        let (latest_identity, latest_review) = latest_runtime_review(&snapshots).unwrap().unwrap();
        let half_pairs = analyze_runtime_half_pairs(&snapshots, &latest_identity).unwrap();
        assert_eq!(latest_identity.module_sha256(), "cd".repeat(32));
        assert_eq!(latest_review.batches, 1);
        let latest_summary = latest_review.render_runtime_summary(&latest_identity, &half_pairs);
        assert!(latest_summary.contains("最新运行身份（与旧批次分开）"));
        assert!(!latest_summary.contains("dago"));
        assert!(!latest_summary.contains("大国"));
        assert!(!latest_summary.contains("打过"));
    }

    #[test]
    fn release_tool_derives_the_stable_installed_user_data_root() {
        assert_eq!(
            research_root_for_executable(Path::new(r"X:\workspace\target\release\researchctl.exe")),
            Some(PathBuf::from(
                r"X:\workspace\.local\tsf-alpha\user-data\research-inbox"
            ))
        );
        assert_eq!(
            research_root_for_executable(Path::new(
                r"X:\workspace\.local\tsf-alpha\user-tools\builds\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\researchctl.exe"
            )),
            Some(PathBuf::from(
                r"X:\workspace\.local\tsf-alpha\user-data\research-inbox"
            ))
        );
        assert!(research_root_for_executable(Path::new(r"X:\tools\researchctl.exe")).is_none());
    }

    #[test]
    fn slow_key_coverage_distinguishes_absent_partial_and_complete_capability() {
        let mut output = String::new();
        render_slow_key_coverage(&mut output, &[], 0, 4);
        assert!(output.contains("未确认；0/4 批支持"));

        output.clear();
        render_slow_key_coverage(&mut output, &[], 2, 4);
        assert!(output.contains("部分确认；2/4 批支持且未观察到"));

        output.clear();
        render_slow_key_coverage(&mut output, &[], 4, 4);
        assert!(output.contains("已确认；4/4 批均支持"));

        output.clear();
        render_slow_key_coverage(&mut output, &[21], 4, 4);
        assert!(output.contains("记录中存在分阶段耗时"));
    }

    #[test]
    fn current_schema_readiness_distinguishes_absent_legacy_mixed_and_complete_batches() {
        let empty = [0; WISH_SCHEMA_VERSION_COUNT];
        assert!(render_current_schema_readiness(&empty, 0).contains("无法判断"));

        let mut legacy = [0; WISH_SCHEMA_VERSION_COUNT];
        legacy[4] = 3;
        legacy[7] = 2;
        let rendered = render_current_schema_readiness(&legacy, 5);
        assert!(rendered.contains("尚未在记录中观察到"));
        assert!(rendered.contains("最高为 V8"));
        assert!(rendered.contains("历史批次不会补写"));

        let mut mixed = legacy;
        mixed[WISH_SCHEMA_VERSION_COUNT - 1] = 2;
        let rendered = render_current_schema_readiness(&mixed, 7);
        assert!(rendered.contains("部分确认"));
        assert!(rendered.contains("2/7 批为当前格式"));

        let mut current = [0; WISH_SCHEMA_VERSION_COUNT];
        current[WISH_SCHEMA_VERSION_COUNT - 1] = 4;
        let rendered = render_current_schema_readiness(&current, 4);
        assert!(rendered.contains("已确认"));
        assert!(rendered.contains("4/4 批均为当前格式"));
    }

    #[test]
    fn scene_rendering_distinguishes_unlinked_legacy_evidence_without_prescribing_a_result() {
        let analysis = analyze_linked_research(&[], &[]).unwrap();
        let rendered = render_scene_analysis(&analysis);

        assert!(rendered.contains("现有批次尚无连续链"));
        assert!(rendered.contains("单次改码不形成手癖结论"));
        for forbidden in ["最佳配置", "下一步建议", "应该采用", "用户打错"] {
            assert!(!rendered.contains(forbidden));
        }
    }
}
