use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ziranma_core::{
    NativeCandidateProvenance, NativeCandidateSource, NativeCandidateView, NativeFeedbackEvent,
    NativeSelectionSource, RESEARCH_FEEDBACK_DIRECTORY, ResearchHabitKind, ResearchSceneAnalysis,
    TranspositionCalibrationLabel, WishCaptureScope, WishRuntimeIdentity, WishSnapshot,
    analyze_linked_research, list_wish_packages, research_feedback_enabled,
    set_research_feedback_enabled,
};
#[cfg(windows)]
use ziranma_core::{WindowsUserDataProtector, load_wish_snapshot};

const MAX_REVIEW_BATCHES: usize = 4_096;
const MAX_REVIEW_ITEMS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Enable,
    Disable,
    Review,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    command: Command,
    root: Option<PathBuf>,
    confirmed: bool,
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
fn print_review(_root: &Path) -> Result<(), Box<dyn Error>> {
    Err("持续研究回顾的当前用户解密目前只支持 Windows".into())
}

#[cfg(windows)]
fn print_review(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut packages = list_wish_packages(root)?;
    let available_batches = packages.len();
    packages.truncate(MAX_REVIEW_BATCHES);
    packages.reverse();
    let mut review = ResearchReview::default();
    let mut snapshots = Vec::with_capacity(packages.len());
    for package in packages {
        let snapshot = load_wish_snapshot(root, package.id(), &WindowsUserDataProtector)?;
        review.observe(&snapshot)?;
        snapshots.push(snapshot);
    }
    review.available_batches = available_batches;
    let wishes = load_linkable_wishes(root)?;
    let scenes = analyze_linked_research(&snapshots, &wishes)?;
    println!("{}\n\n{}", review.render(), render_scene_analysis(&scenes));
    Ok(())
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
    sources: [usize; 9],
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
    shape_frames: usize,
    tab_assembly_frames: usize,
    maximum_loaded_candidates: usize,
    frames_allowing_more_load: usize,
    popup_ms: Vec<u32>,
    initial_popup_ms: Vec<u32>,
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
            "其他结束：原码上屏 {}；取消 {}。",
            self.raw_commits, self.cancellations,
        )
        .unwrap();
        render_latency(&mut output, "候选窗完全显示", &self.popup_ms);
        render_latency(&mut output, "首次出现", &self.initial_popup_ms);
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
        Some("review") => Command::Review,
        _ => return Err(usage().into()),
    };
    let mut root = None;
    let mut confirmed = false;
    let mut show_private_text = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(arguments.next().ok_or("--root 后缺少目录")?));
            }
            "--confirm-continuous-private-feedback" if !confirmed => confirmed = true,
            "--confirm-show-private-text" if !show_private_text => show_private_text = true,
            _ => return Err(usage().into()),
        }
    }
    if command != Command::Enable && confirmed {
        return Err(usage().into());
    }
    if command != Command::Review && show_private_text {
        return Err(usage().into());
    }
    Ok(Options {
        command,
        root,
        confirmed,
        show_private_text,
    })
}

fn research_root_for_executable(executable: &Path) -> Option<PathBuf> {
    let release = executable.parent()?;
    let target = release.parent()?;
    let repository = target.parent()?;
    if release.file_name()?.to_str()? != "release"
        || target.file_name()?.to_str()? != "target"
        || executable.file_stem()?.to_str()? != "researchctl"
    {
        return None;
    }
    Some(
        repository
            .join(".local")
            .join("tsf-alpha")
            .join("user-data")
            .join(RESEARCH_FEEDBACK_DIRECTORY),
    )
}

fn usage() -> String {
    "用法：\n  researchctl status [--root <目录>]\n  researchctl review \
     --confirm-show-private-text [--root <目录>]\n  researchctl enable \
     --confirm-continuous-private-feedback [--root <目录>]\n  researchctl disable [--root <目录>]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_the_private_capture_confirmation_only_for_enable() {
        assert_eq!(
            parse_options(["status".to_owned()]).unwrap(),
            Options {
                command: Command::Status,
                root: None,
                confirmed: false,
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
        let review = parse_options([
            "review".to_owned(),
            "--confirm-show-private-text".to_owned(),
        ])
        .unwrap();
        assert_eq!(review.command, Command::Review);
        assert!(review.show_private_text);
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
        assert!(
            review
                .render()
                .contains("DLL abababababab…；核心 research-core-v1")
        );
    }

    #[test]
    fn release_tool_derives_the_stable_installed_user_data_root() {
        assert_eq!(
            research_root_for_executable(Path::new(
                r"D:\IME\ziranma-decoder\target\release\researchctl.exe"
            )),
            Some(PathBuf::from(
                r"D:\IME\ziranma-decoder\.local\tsf-alpha\user-data\research-inbox"
            ))
        );
        assert!(research_root_for_executable(Path::new(r"D:\tools\researchctl.exe")).is_none());
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
