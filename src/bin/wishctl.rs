use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use ziranma_core::WindowsUserDataProtector;
use ziranma_core::{
    DataProtector, NativeAutomaticTranspositionDecision, NativeAutomaticTranspositionOutcome,
    NativeAutomaticTranspositionTier, NativeCancellationSource, NativeCandidateSource,
    NativeCandidateView, NativeFeedbackEvent, NativeSelectionSource, WishCaptureScope,
    WishCategory, WishCommand, WishCommandAckStatus, WishEventRole, WishFeedbackError,
    WishImportance, WishJournalContext, WishNote, WishReviewStatus, dispatch_wish_command,
    list_wish_packages, load_wish_note, load_wish_snapshot, move_wish_to_trash, save_wish_note,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum WishSelector {
    Exact(String),
    Latest,
}

#[derive(Clone, Eq, PartialEq)]
enum Command {
    Control(WishCommand),
    Status,
    List,
    Show {
        selector: WishSelector,
        show_private_text: bool,
    },
    Annotate {
        selector: WishSelector,
        category: WishCategory,
        text: String,
    },
    Trash {
        id: String,
        confirmed: bool,
    },
}

#[derive(Clone, Eq, PartialEq)]
struct Options {
    root: Option<PathBuf>,
    command: Command,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("许愿管理失败：{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run() -> Result<(), Box<dyn Error>> {
    Err("许愿包的当前用户加密目前只支持 Windows".into())
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    let protector = WindowsUserDataProtector;
    match options.command {
        Command::Control(command) => control(command),
        Command::Status => status(options.root.as_deref().expect("validated storage root")),
        Command::List => list(options.root.as_deref().expect("validated storage root")),
        Command::Show {
            selector,
            show_private_text,
        } => show(
            options.root.as_deref().expect("validated storage root"),
            &protector,
            &selector,
            show_private_text,
        ),
        Command::Annotate {
            selector,
            category,
            text,
        } => annotate(
            options.root.as_deref().expect("validated storage root"),
            &protector,
            &selector,
            category,
            &text,
        ),
        Command::Trash { id, confirmed } => trash(
            options.root.as_deref().expect("validated storage root"),
            &id,
            confirmed,
        ),
    }
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut arguments = arguments.into_iter();
    let command_name = arguments.next().ok_or_else(usage)?;
    let mut root = None;
    let mut id = None;
    let mut latest = false;
    let mut category = None;
    let mut text = None;
    let mut show_private_text = false;
    let mut confirm_trash = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_value(&mut root, arguments.next(), "--root")?,
            "--id" => set_value(&mut id, arguments.next(), "--id")?,
            "--latest" if !latest => latest = true,
            "--category" => set_value(&mut category, arguments.next(), "--category")?,
            "--text" => set_value(&mut text, arguments.next(), "--text")?,
            "--confirm-show-private-text" if !show_private_text => show_private_text = true,
            "--confirm-move-to-trash" if !confirm_trash => confirm_trash = true,
            _ => return Err(format!("无法识别或重复的参数：{argument}\n{}", usage()).into()),
        }
    }
    let root = root.map(PathBuf::from);
    let selector = || -> Result<WishSelector, Box<dyn Error>> {
        match (id.clone(), latest) {
            (Some(id), false) => Ok(WishSelector::Exact(id)),
            (None, true) => Ok(WishSelector::Latest),
            _ => Err("请只选择一个许愿包：--id <编号> 或 --latest".into()),
        }
    };
    let command = match command_name.as_str() {
        "start"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::Control(WishCommand::Start)
        }
        "mark"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::Control(WishCommand::SaveRecent)
        }
        "stop"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::Control(WishCommand::Stop)
        }
        "clear"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::Control(WishCommand::ClearStopped)
        }
        "status"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::Status
        }
        "list"
            if id.is_none()
                && !latest
                && category.is_none()
                && text.is_none()
                && !show_private_text
                && !confirm_trash =>
        {
            Command::List
        }
        "show" if category.is_none() && text.is_none() && !confirm_trash => Command::Show {
            selector: selector()?,
            show_private_text,
        },
        "annotate" if !show_private_text && !confirm_trash => Command::Annotate {
            selector: selector()?,
            category: WishCategory::parse_slug(&category.ok_or("annotate 缺少 --category")?)
                .ok_or("未知类别；可用 candidates/ranking/display/latency/input-mode/compatibility/other")?,
            text: text.ok_or("annotate 缺少 --text")?,
        },
        "trash"
            if !latest && category.is_none() && text.is_none() && !show_private_text =>
        {
            Command::Trash {
                id: id.ok_or("trash 缺少 --id")?,
                confirmed: confirm_trash,
            }
        }
        _ => return Err(usage().into()),
    };
    if !matches!(command, Command::Control(_)) && root.is_none() {
        return Err("缺少 --root".into());
    }
    Ok(Options { root, command })
}

fn set_value(
    destination: &mut Option<String>,
    value: Option<String>,
    option: &str,
) -> Result<(), Box<dyn Error>> {
    if destination.is_some() {
        return Err(format!("参数重复：{option}").into());
    }
    let value = value.ok_or_else(|| format!("{option} 缺少值"))?;
    if value.is_empty() {
        return Err(format!("{option} 的值不能为空").into());
    }
    *destination = Some(value);
    Ok(())
}

fn usage() -> String {
    "用法：\n  wishctl start | mark | stop | clear\n  wishctl status --root <目录>\n  wishctl list --root <目录>\n  wishctl show --root <目录> (--id <编号> | --latest) --confirm-show-private-text\n  wishctl annotate --root <目录> (--id <编号> | --latest) --category <类别> --text <说明>\n  wishctl trash --root <目录> --id <编号> --confirm-move-to-trash"
        .to_owned()
}

fn control(command: WishCommand) -> Result<(), Box<dyn Error>> {
    let receipt = dispatch_wish_command(command)?;
    match receipt.acknowledgement() {
        Some(WishCommandAckStatus::Applied) => println!(
            "{}",
            match command {
                WishCommand::Start => "反馈已开始；许愿前暂不保存。",
                WishCommand::SaveRecent => "已保存最近 30 秒的本地加密快照。",
                WishCommand::Stop => "反馈已停止；尚未保存的内存事件不会继续增长。",
                WishCommand::ClearStopped => "已清除停止后的内存会话。",
            }
        ),
        Some(WishCommandAckStatus::NoChange) => println!(
            "{}",
            match command {
                WishCommand::Start => "反馈已经在记录。",
                WishCommand::SaveRecent => "最近还没有可保存的输入法事件。",
                WishCommand::Stop => "反馈当前没有在记录。",
                WishCommand::ClearStopped => "当前没有可清除的已停止会话。",
            }
        ),
        Some(WishCommandAckStatus::Failed) => {
            return Err("输入法宿主收到了命令，但未能完成操作".into());
        }
        None => println!(
            "命令已发出，但没有新版输入法宿主响应。请新开一个使用自然码 Alpha 的输入框后重试。"
        ),
    }
    println!("未传输输入正文，未联网。");
    Ok(())
}

fn status(root: &Path) -> Result<(), Box<dyn Error>> {
    let packages = list_wish_packages(root)?;
    println!("本地许愿：{} 条", packages.len());
    if let Some(latest) = packages.first() {
        println!("  最近一条：{}", latest.id());
    }
    println!("  内容：Windows 当前用户加密");
    println!("  网络：未连接");
    Ok(())
}

fn list(root: &Path) -> Result<(), Box<dyn Error>> {
    let packages = list_wish_packages(root)?;
    if packages.is_empty() {
        println!("还没有本地许愿。");
        return Ok(());
    }
    println!("本地许愿 · {} 条（最近在前）", packages.len());
    for package in packages {
        println!(
            "{} · {} 字节（加密）",
            package.id(),
            package.protected_bytes()
        );
    }
    println!("未解密原文，未联网。");
    Ok(())
}

fn resolve_selector(root: &Path, selector: &WishSelector) -> Result<String, Box<dyn Error>> {
    match selector {
        WishSelector::Exact(id) => Ok(id.clone()),
        WishSelector::Latest => list_wish_packages(root)?
            .first()
            .map(|package| package.id().to_owned())
            .ok_or_else(|| "还没有可以选择的本地许愿".into()),
    }
}

fn show(
    root: &Path,
    protector: &dyn DataProtector,
    selector: &WishSelector,
    show_private_text: bool,
) -> Result<(), Box<dyn Error>> {
    if !show_private_text {
        return Err("show 会在当前终端显示私人输入；请显式加入 --confirm-show-private-text".into());
    }
    let id = resolve_selector(root, selector)?;
    let snapshot = load_wish_snapshot(root, &id, protector)?;
    println!(
        "许愿 {id} · {} · {} · {} 条事件",
        capture_scope_label(snapshot.capture_scope(), snapshot.lookback_ms()),
        category_label(snapshot.category()),
        snapshot.events().len()
    );
    let focus = snapshot.focus_event_range();
    println!(
        "重点片段：第 {}–{} 条；之前为参考上下文，之后为许愿入口",
        focus.start.saturating_add(1),
        focus.end,
    );
    println!("时间跨度 {} ms", snapshot.lookback_ms(),);
    println!(
        "来源 {} 条；窗口前省略 {}，无时间省略 {}，容量省略 {}；完整：{}",
        snapshot.source_events(),
        snapshot.omitted_before_window(),
        snapshot.omitted_untimed(),
        snapshot.omitted_by_event_limit(),
        if snapshot.source_complete() {
            "是"
        } else {
            "否"
        }
    );
    if let Some(identity) = snapshot.runtime_identity() {
        println!(
            "运行身份：DLL {}…；核心 {}；补充 {}",
            &identity.module_sha256()[..12],
            identity.core_candidate_revision(),
            identity.supplemental_candidate_revision().unwrap_or("无"),
        );
    } else {
        println!("运行身份：旧批次未记录");
    }
    println!("{}", journal_context_label(snapshot.journal_context()));
    let mut previous_role = None;
    let mut previous_completed_episode = false;
    let mut context_segment = 0_usize;
    for (index, event) in snapshot.events().iter().enumerate() {
        let role = snapshot.event_role(index).ok_or("许愿事件角色无效")?;
        if previous_role != Some(role)
            || (role == WishEventRole::Context && previous_completed_episode)
        {
            if role == WishEventRole::Context {
                context_segment = context_segment.saturating_add(1);
                println!("\n【参考片段 {context_segment}】");
            } else {
                println!("\n【{}】", event_role_label(role));
            }
            previous_role = Some(role);
        }
        print!("-{} ms  ", event.milliseconds_before_marker());
        print_event(event.event());
        previous_completed_episode = event_completes_episode(event.event());
    }
    match load_wish_note(root, &id, protector) {
        Ok(note) => {
            println!("说明 [{}]", note.category().slug());
            println!(
                "整理：{} · {}",
                review_status_label(note.review_status()),
                importance_label(note.importance())
            );
            if note.text().trim().is_empty() {
                println!("尚未补充文字说明");
            } else {
                println!("{}", note.text());
            }
        }
        Err(WishFeedbackError::NoteUnavailable) => println!("说明：尚未添加"),
        Err(error) => return Err(error.into()),
    }
    println!("原文只显示在当前终端；未写模型，未联网。");
    Ok(())
}

fn journal_context_label(context: Option<&WishJournalContext>) -> String {
    match context {
        Some(WishJournalContext::ContinuousSpan(span)) => format!(
            "连续位置：流 {}… · 批次 {} · 首事件 #{}{}",
            &span.stream_id()[..12],
            span.batch_sequence(),
            span.first_event_ordinal(),
            span.previous_event_gap_ms()
                .map(|gap_ms| format!(" · 距前批 {gap_ms} ms"))
                .unwrap_or_default(),
        ),
        Some(WishJournalContext::WishAnchor(anchor)) => format!(
            "连续位置：锚到流 {}… 的事件 #{}",
            &anchor.stream_id()[..12],
            anchor.event_ordinal(),
        ),
        None => "连续位置：旧版未记录".to_owned(),
    }
}

fn capture_scope_label(scope: WishCaptureScope, lookback_ms: u32) -> String {
    match scope {
        WishCaptureScope::LegacyWindow => format!("旧版时间窗 {lookback_ms} ms"),
        WishCaptureScope::RecentEpisodes => "按输入片段截取".to_owned(),
        WishCaptureScope::RecentWindow => format!("近 {lookback_ms} ms"),
        WishCaptureScope::ContinuousJournal => "持续研究批次".to_owned(),
    }
}

fn category_label(category: WishCategory) -> &'static str {
    match category {
        WishCategory::Candidates => "候选",
        WishCategory::Ranking => "候选排序",
        WishCategory::Display => "显示界面",
        WishCategory::Latency => "卡顿延迟",
        WishCategory::InputMode => "按键模式",
        WishCategory::Compatibility => "兼容性",
        WishCategory::Other => "未分类",
    }
}

fn event_role_label(role: WishEventRole) -> &'static str {
    match role {
        WishEventRole::Context => "参考上下文",
        WishEventRole::Focus => "重点片段",
        WishEventRole::Trigger => "许愿入口",
    }
}

fn event_completes_episode(event: &NativeFeedbackEvent) -> bool {
    matches!(
        event,
        NativeFeedbackEvent::CandidateCommitted { .. }
            | NativeFeedbackEvent::RawCodeCommitted { .. }
            | NativeFeedbackEvent::CompositionCancelled { .. }
    )
}

fn print_event(event: &NativeFeedbackEvent) {
    match event {
        NativeFeedbackEvent::CandidatesPresented {
            code,
            view,
            page_start,
            candidates,
            may_have_more,
        } => {
            let candidates = candidates
                .iter()
                .enumerate()
                .map(|(index, text)| format!("{} {text}", page_start + index + 1))
                .collect::<Vec<_>>()
                .join(" · ");
            println!(
                "候选  {code} [{}] → {candidates}{}",
                view_label(*view),
                if *may_have_more { " · …" } else { "" }
            );
        }
        NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code,
            view,
            page_start,
            candidates,
            provenance,
            automatic_transposition,
            loaded_candidates,
            tab_assembly,
            may_have_more,
        } => {
            let depth_label = candidate_depth_label(
                *page_start,
                candidates.len(),
                *loaded_candidates,
                *may_have_more,
            );
            let candidates = candidates
                .iter()
                .zip(provenance)
                .enumerate()
                .map(|(index, (text, provenance))| {
                    format!(
                        "{} {text}〔{}{}〕",
                        page_start + index + 1,
                        candidate_source_label(provenance.source()),
                        if provenance.session_promoted() {
                            "，个人/会话提升"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            println!(
                "候选  {code} [{}；已加载 {} 项，{}] → {candidates}",
                view_label(*view),
                loaded_candidates,
                depth_label,
            );
            if let Some(tab) = tab_assembly {
                println!(
                    "      Tab 组词：第 {}/{} 字；笔画前缀：{}",
                    tab.position(),
                    tab.total_characters(),
                    if tab.stroke_prefix().is_empty() {
                        "未输入"
                    } else {
                        tab.stroke_prefix()
                    }
                );
            }
            if let Some(decision) = automatic_transposition {
                println!("      {}", automatic_transposition_label(decision));
            }
        }
        NativeFeedbackEvent::CandidateCommitted {
            code,
            text,
            view,
            source,
            absolute_rank,
            visible_rank,
        } => println!(
            "上屏  {code} → “{text}” [{}；{}；总第 {absolute_rank}，页内第 {visible_rank}]",
            view_label(*view),
            selection_label(*source)
        ),
        NativeFeedbackEvent::RawCodeCommitted { code } => println!("原码  {code}"),
        NativeFeedbackEvent::CompositionCancelled { code, source } => {
            println!("取消  {code} [{}]", cancellation_label(*source));
        }
        NativeFeedbackEvent::CandidatePopupTiming {
            first_frame_ms,
            fully_visible_ms,
            initial_show,
        } => println!(
            "候选窗  首帧 {first_frame_ms} ms，完全显示 {fully_visible_ms} ms{}",
            if *initial_show {
                "（首次出现）"
            } else {
                ""
            }
        ),
        NativeFeedbackEvent::SlowKeyPathTiming {
            refresh_ms,
            planning_ms,
            edit_session_ms,
            total_ms,
        } => println!(
            "慢按键  总计 {total_ms} ms；刷新 {refresh_ms} ms，候选 {planning_ms} ms，编辑 {edit_session_ms} ms"
        ),
        NativeFeedbackEvent::PostCommitBackspaceRouted => {
            println!("提交后退格  已交给宿主；最终文档结果未观测");
        }
    }
}

fn candidate_depth_label(
    page_start: usize,
    visible_candidates: usize,
    loaded_candidates: usize,
    may_load_more: bool,
) -> &'static str {
    if loaded_candidates > page_start.saturating_add(visible_candidates) {
        "后面已有候选"
    } else if may_load_more {
        "还可继续加载"
    } else {
        "已到底"
    }
}

fn candidate_source_label(source: NativeCandidateSource) -> &'static str {
    match source {
        NativeCandidateSource::Unknown => "来源未知",
        NativeCandidateSource::ExplicitAlias => "显式别名",
        NativeCandidateSource::ProjectOverlay => "项目词",
        NativeCandidateSource::CoreExact => "核心整词",
        NativeCandidateSource::SupplementalExact => "补充整词/组合",
        NativeCandidateSource::CharacterPair => "双字自由组合",
        NativeCandidateSource::Decoder => "完整或普通组合",
        NativeCandidateSource::TranspositionRecovery => "自动纠序",
        NativeCandidateSource::Shape => "Tab 找字",
        NativeCandidateSource::FourCharacterCorrection => "四字纠错",
    }
}

fn automatic_transposition_label(decision: &NativeAutomaticTranspositionDecision) -> String {
    let tier_name = |tier| match tier {
        NativeAutomaticTranspositionTier::Primary => "高置信",
        NativeAutomaticTranspositionTier::Secondary => "中置信",
        NativeAutomaticTranspositionTier::Shadow => "影子",
    };
    let tier = if decision.cold_tier() == decision.tier() {
        tier_name(decision.tier()).to_owned()
    } else {
        format!(
            "{}→{}",
            tier_name(decision.cold_tier()),
            tier_name(decision.tier())
        )
    };
    let action = if decision.syllable_count() == 1 {
        "自动换序"
    } else {
        "双音节换序"
    };
    match decision.outcome() {
        NativeAutomaticTranspositionOutcome::Suppressed => {
            format!(
                "{action}：{tier}，原码证据优先，{} ms",
                decision.pair_gap_ms()
            )
        }
        NativeAutomaticTranspositionOutcome::NoRecovery => {
            format!(
                "{action}：{tier}，没有唯一结果，{} ms",
                decision.pair_gap_ms()
            )
        }
        NativeAutomaticTranspositionOutcome::RecoveryAvailable => {
            let text = decision.recovered_text().unwrap_or("候选");
            match decision.visible_rank() {
                Some(rank) => format!(
                    "{action}：{tier}，“{text}”进入第 {rank} 项，{} ms",
                    decision.pair_gap_ms()
                ),
                None => format!(
                    "{action}：{tier}，后台命中“{text}”，{} ms",
                    decision.pair_gap_ms()
                ),
            }
        }
    }
}

fn view_label(view: NativeCandidateView) -> &'static str {
    match view {
        NativeCandidateView::Ordinary => "普通",
        NativeCandidateView::TranspositionRecovery => "纠序",
        NativeCandidateView::Shape => "找字",
    }
}

fn selection_label(source: NativeSelectionSource) -> &'static str {
    match source {
        NativeSelectionSource::FirstCandidate => "首选",
        NativeSelectionSource::Numeric => "数字选择",
        NativeSelectionSource::Punctuation => "标点选择",
    }
}

fn cancellation_label(source: NativeCancellationSource) -> &'static str {
    match source {
        NativeCancellationSource::Backspace => "退格",
        NativeCancellationSource::Escape => "Esc",
        NativeCancellationSource::FocusLoss => "失去焦点",
        NativeCancellationSource::HostTermination => "宿主结束",
    }
}

fn review_status_label(status: WishReviewStatus) -> &'static str {
    match status {
        WishReviewStatus::Inbox => "待整理",
        WishReviewStatus::InProgress => "处理中",
        WishReviewStatus::Resolved => "已完成",
    }
}

fn importance_label(importance: WishImportance) -> &'static str {
    match importance {
        WishImportance::Normal => "普通",
        WishImportance::Important => "重要",
    }
}

fn annotate(
    root: &Path,
    protector: &dyn DataProtector,
    selector: &WishSelector,
    category: WishCategory,
    text: &str,
) -> Result<(), Box<dyn Error>> {
    let id = resolve_selector(root, selector)?;
    let note = WishNote::new(&id, category, text)?;
    save_wish_note(root, &note, protector)?;
    println!("已为 {id} 添加加密说明 [{}]。", category.slug());
    println!("未联网，未写模型。");
    Ok(())
}

fn trash(root: &Path, id: &str, confirmed: bool) -> Result<(), Box<dyn Error>> {
    if !confirmed {
        return Err("移动会让这条许愿离开当前列表；请显式加入 --confirm-move-to-trash".into());
    }
    move_wish_to_trash(root, id)?;
    println!("已移入本地 trash：{id}");
    println!("文件仍可恢复，没有联网。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organization_labels_are_plain_and_stable() {
        assert_eq!(review_status_label(WishReviewStatus::Inbox), "待整理");
        assert_eq!(review_status_label(WishReviewStatus::InProgress), "处理中");
        assert_eq!(review_status_label(WishReviewStatus::Resolved), "已完成");
        assert_eq!(importance_label(WishImportance::Normal), "普通");
        assert_eq!(importance_label(WishImportance::Important), "重要");
    }

    #[test]
    fn control_commands_do_not_require_a_storage_root() {
        for (name, expected) in [
            ("start", WishCommand::Start),
            ("mark", WishCommand::SaveRecent),
            ("stop", WishCommand::Stop),
            ("clear", WishCommand::ClearStopped),
        ] {
            let options = parse_options([name.to_owned()]).unwrap();
            assert!(matches!(options.command, Command::Control(command) if command == expected));
            assert!(options.root.is_none());
        }
        assert!(parse_options(["status".to_owned()]).is_err());
    }

    #[test]
    fn parsing_requires_one_selector_and_explicit_private_show() {
        let options = parse_options([
            "show".to_owned(),
            "--root".to_owned(),
            "wishes".to_owned(),
            "--latest".to_owned(),
            "--confirm-show-private-text".to_owned(),
        ])
        .unwrap();
        assert!(matches!(
            options.command,
            Command::Show {
                selector: WishSelector::Latest,
                show_private_text: true
            }
        ));
        assert!(
            parse_options(["show".to_owned(), "--root".to_owned(), "wishes".to_owned(),]).is_err()
        );
    }

    #[test]
    fn annotation_category_and_trash_confirmation_are_strict() {
        let annotate = parse_options([
            "annotate".to_owned(),
            "--root".to_owned(),
            "wishes".to_owned(),
            "--latest".to_owned(),
            "--category".to_owned(),
            "display".to_owned(),
            "--text".to_owned(),
            "边角不圆润".to_owned(),
        ])
        .unwrap();
        assert!(matches!(
            annotate.command,
            Command::Annotate {
                category: WishCategory::Display,
                ..
            }
        ));
        assert!(
            parse_options([
                "trash".to_owned(),
                "--root".to_owned(),
                "wishes".to_owned(),
                "--id".to_owned(),
                "wish-invalid".to_owned(),
            ])
            .is_ok(),
            "exact id validation belongs to the storage boundary"
        );
    }

    #[test]
    fn candidate_depth_distinguishes_loaded_pages_from_future_loading() {
        assert_eq!(candidate_depth_label(0, 6, 12, false), "后面已有候选");
        assert_eq!(candidate_depth_label(6, 6, 12, true), "还可继续加载");
        assert_eq!(candidate_depth_label(6, 6, 12, false), "已到底");
    }

    #[test]
    fn journal_context_label_keeps_linked_and_legacy_records_distinct() {
        let span = ziranma_core::WishJournalSpan::new("12".repeat(32), 3, 24, Some(420)).unwrap();
        let span_context = WishJournalContext::ContinuousSpan(span);
        assert_eq!(
            journal_context_label(Some(&span_context)),
            "连续位置：流 121212121212… · 批次 3 · 首事件 #24 · 距前批 420 ms"
        );

        let anchor = ziranma_core::WishJournalAnchor::new("34".repeat(32), 31).unwrap();
        let anchor_context = WishJournalContext::WishAnchor(anchor);
        assert_eq!(
            journal_context_label(Some(&anchor_context)),
            "连续位置：锚到流 343434343434… 的事件 #31"
        );
        assert_eq!(journal_context_label(None), "连续位置：旧版未记录");
    }
}
