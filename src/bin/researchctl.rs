use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::thread;

use ziranma_core::{
    NativeCandidateProvenance, NativeCandidateSource, NativeCandidateView, NativeFeedbackEvent,
    NativeSelectionSource, RESEARCH_FEEDBACK_DIRECTORY, ResearchHabitKind,
    ResearchHalfPairAnalysis, ResearchSceneAnalysis, TranspositionCalibrationLabel,
    WishCaptureScope, WishRuntimeIdentity, WishSnapshot, analyze_linked_research,
    analyze_runtime_half_pairs, list_wish_packages, repository_root_for_user_tool_executable,
    research_feedback_enabled, set_research_feedback_enabled,
};
#[cfg(windows)]
use ziranma_core::{WindowsUserDataProtector, load_wish_snapshot};

const MAX_REVIEW_BATCHES: usize = 4_096;
const MAX_REVIEW_ITEMS: usize = 12;
const PARALLEL_LOAD_THRESHOLD: usize = 32;
const MAX_PARALLEL_LOADERS: usize = 4;

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
}

impl PresentedFrame {
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
    session_promoted_frames: usize,
    sources: [usize; 10],
}

impl SelectionPattern {
    fn observe(
        &mut self,
        rank: usize,
        selection: NativeSelectionSource,
        provenance: Option<NativeCandidateProvenance>,
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
        if let Some(provenance) = provenance {
            self.session_promoted_frames += usize::from(provenance.session_promoted());
            self.sources[candidate_source_index(provenance.source())] += 1;
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
                    frame = Some(PresentedFrame {
                        code: code.clone(),
                        view: *view,
                        page_start: *page_start,
                        provenance: Vec::new(),
                    });
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
                    frame = Some(PresentedFrame {
                        code: code.clone(),
                        view: *view,
                        page_start: *page_start,
                        provenance: provenance.clone(),
                    });
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
                    let provenance = frame
                        .as_ref()
                        .filter(|frame| frame.code == *code && frame.view == *view)
                        .and_then(|frame| frame.provenance_for_rank(*absolute_rank));
                    self.unpaired_commits += usize::from(
                        frame
                            .as_ref()
                            .is_none_or(|frame| frame.code != *code || frame.view != *view),
                    );
                    self.selections
                        .entry((code.clone(), text.clone()))
                        .or_default()
                        .observe(*absolute_rank, *source, provenance);
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
        render_slow_key_coverage(&mut output, &self.slow_key_total_ms);
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
        render_slow_key_coverage(&mut output, &self.slow_key_total_ms);
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
                "- {code} → “{text}”：非首选 {}/{} 次；名次 {}–{}；显式选择 {} 次；翻页 {} 次；来源 {}；会话提升标记 {} 次。",
                pattern.non_top_selections,
                pattern.selections,
                pattern.minimum_rank,
                pattern.maximum_rank,
                pattern.manual_selections,
                pattern.paged_selections,
                candidate_source_label(pattern.dominant_source()),
                pattern.session_promoted_frames,
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
        render_slow_key_coverage(&mut output, &self.slow_key_total_ms);
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

fn render_slow_key_coverage(output: &mut String, values: &[u32]) {
    if values.is_empty() {
        output.push_str(
            "慢按键分段覆盖：未确认；0 条既可能表示没有 ≥16 ms 按键，也可能表示运行 DLL 尚未采集该字段。\n",
        );
    } else {
        output.push_str("慢按键分段覆盖：已确认；记录中存在分阶段耗时。\n");
    }
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
                pattern.observe(rank, NativeSelectionSource::Numeric, None);
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
        assert_eq!(pattern.session_promoted_frames, 1);
        assert!(review.render().contains("首次第 2，最近第 1"));
        let aggregate = review.render_aggregate();
        assert!(aggregate.contains("持续研究摘要（不显示输入原文）"));
        assert!(!aggregate.contains("dago"));
        assert!(!aggregate.contains("大国"));
        assert!(!aggregate.contains("打过"));
        assert!(!aggregate.contains("需要复查"));
        assert!(aggregate.contains("慢按键分段覆盖：未确认"));
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
