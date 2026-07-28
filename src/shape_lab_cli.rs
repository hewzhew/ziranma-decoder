use std::error::Error;
use std::fmt::Write as _;

use ziranma_decoder::ShapeLabSnapshot;

pub const SHAPE_LAB_USAGE: &str = "\
Tab 笔画实验台

用法：
  cargo run --release -- shape-lab <公开单字拼音> [选项]

选项：
  --expect <单字>       在课程界面显示一个公开目标
  --prefix <hspnz...>  直接查看一个前缀；省略时进入会话
  --limit <1～10>      最多显示多少个候选（默认 10）
  --details            显示排序、动作投影和公开笔画码后退出

Windows 交互终端中直接按 Tab 进入辅助；再逐笔按 h 横、s 竖、p 撇、n 捺、z 折。
数字选择候选，退格撤回一笔，Esc 返回普通候选，q 退出；全程不需要 Enter。
重定向或不支持逐键读取时，会回退为行命令：t、hspnz、数字、-、esc、q。
隐私：拼音、目标和候选会显示在终端；请只使用公开或合成材料。
口径：固定公开词典与笔画快照；只过滤、不重排；不学习、不写文件。";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeLabCliOptions {
    pub pinyin: String,
    pub expected_character: Option<char>,
    pub prefix: Option<String>,
    pub visible_limit: usize,
    pub details: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShapeLabInput {
    Noop,
    EnterTab,
    Stroke(String),
    Select(usize),
    Backspace,
    LeaveTab,
    Quit,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeLabSessionEffect {
    Continue,
    Select(usize),
    Quit,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShapeLabSession {
    tab_mode: bool,
    stroke_prefix: String,
    notice: Option<String>,
}

impl ShapeLabSession {
    pub fn tab_mode(&self) -> bool {
        self.tab_mode
    }

    pub fn active_prefix(&self) -> &str {
        if self.tab_mode {
            &self.stroke_prefix
        } else {
            ""
        }
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn apply(&mut self, input: ShapeLabInput) -> ShapeLabSessionEffect {
        self.notice = None;
        match input {
            ShapeLabInput::Noop => {}
            ShapeLabInput::EnterTab => self.tab_mode = true,
            ShapeLabInput::Stroke(strokes) if self.tab_mode => {
                self.stroke_prefix.push_str(&strokes);
            }
            ShapeLabInput::Stroke(_) => self.set_notice("先按 Tab 进入笔画辅助"),
            ShapeLabInput::Select(rank) => return ShapeLabSessionEffect::Select(rank),
            ShapeLabInput::Backspace if self.tab_mode => {
                if self.stroke_prefix.pop().is_none() {
                    self.tab_mode = false;
                }
            }
            ShapeLabInput::Backspace => {}
            ShapeLabInput::LeaveTab => {
                self.tab_mode = false;
                self.stroke_prefix.clear();
            }
            ShapeLabInput::Quit => return ShapeLabSessionEffect::Quit,
            ShapeLabInput::Invalid => self.set_notice("没有这个操作"),
        }
        ShapeLabSessionEffect::Continue
    }
}

pub fn parse_shape_lab_arguments(
    arguments: &[String],
) -> Result<ShapeLabCliOptions, Box<dyn Error>> {
    let mut pinyin = None;
    let mut expected_character = None;
    let mut prefix = None;
    let mut visible_limit = 10usize;
    let mut limit_seen = false;
    let mut details = false;

    let mut argument_index = 0usize;
    while argument_index < arguments.len() {
        match arguments[argument_index].as_str() {
            "--expect" => {
                if expected_character.is_some() {
                    return Err("shape-lab 的 --expect 只能使用一次".into());
                }
                argument_index += 1;
                let value = arguments
                    .get(argument_index)
                    .ok_or("shape-lab 的 --expect 后面需要一个目标单字")?;
                if value.starts_with('-') {
                    return Err("shape-lab 的 --expect 后面需要一个目标单字".into());
                }
                let mut characters = value.chars();
                let character = characters
                    .next()
                    .filter(|_| characters.next().is_none())
                    .ok_or("shape-lab 的 --expect 必须恰好是一个 Unicode 字符")?;
                expected_character = Some(character);
            }
            "--prefix" => {
                if prefix.is_some() {
                    return Err("shape-lab 的 --prefix 只能使用一次".into());
                }
                argument_index += 1;
                let value = arguments
                    .get(argument_index)
                    .ok_or("shape-lab 的 --prefix 后面需要笔画前缀")?;
                if value.is_empty() || value.starts_with('-') {
                    return Err("shape-lab 的 --prefix 后面需要非空笔画前缀".into());
                }
                prefix = Some(value.to_owned());
            }
            "--limit" => {
                if limit_seen {
                    return Err("shape-lab 的 --limit 只能使用一次".into());
                }
                limit_seen = true;
                argument_index += 1;
                visible_limit = arguments
                    .get(argument_index)
                    .ok_or("shape-lab 的 --limit 后面需要 1～10")?
                    .parse::<usize>()
                    .map_err(|_| "shape-lab 的 --limit 必须是 1～10 的整数")?;
            }
            "--details" => details = true,
            value if value.starts_with('-') => {
                return Err(format!("shape-lab 不认识选项 {value:?}").into());
            }
            value if pinyin.is_none() => pinyin = Some(value.to_owned()),
            _ => return Err("shape-lab 参数过多；请运行 shape-lab --help".into()),
        }
        argument_index += 1;
    }

    Ok(ShapeLabCliOptions {
        pinyin: pinyin.ok_or("shape-lab 需要一个公开单字的无声调拼音")?,
        expected_character,
        prefix,
        visible_limit,
        details,
    })
}

pub fn parse_shape_lab_input(raw: &str) -> ShapeLabInput {
    let without_newline = raw.trim_end_matches(['\r', '\n']);
    if without_newline == "\t" {
        return ShapeLabInput::EnterTab;
    }
    let input = without_newline.trim().to_ascii_lowercase();
    match input.as_str() {
        "" => ShapeLabInput::Noop,
        "t" | "tab" => ShapeLabInput::EnterTab,
        "-" | "back" | "backspace" => ShapeLabInput::Backspace,
        "esc" | "escape" => ShapeLabInput::LeaveTab,
        "q" | "quit" | "exit" => ShapeLabInput::Quit,
        value if value.len() == 1 && value.as_bytes().first().is_some_and(u8::is_ascii_digit) => {
            let digit = usize::from(value.as_bytes()[0] - b'0');
            ShapeLabInput::Select(if digit == 0 { 10 } else { digit })
        }
        value
            if !value.is_empty()
                && value
                    .as_bytes()
                    .iter()
                    .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z')) =>
        {
            ShapeLabInput::Stroke(value.to_owned())
        }
        _ => ShapeLabInput::Invalid,
    }
}

pub fn render_shape_lab_screen(
    snapshot: &ShapeLabSnapshot,
    tab_mode: bool,
    show_controls: bool,
    notice: Option<&str>,
    direct_keys: bool,
) -> String {
    let mut output = String::new();
    if let Some(expected) = snapshot.expected_character {
        writeln!(output, "目标：{expected}（{}）", snapshot.pinyin)
            .expect("writing to String cannot fail");
        writeln!(output).expect("writing to String cannot fail");
    }

    write!(output, "双拼：{}", snapshot.phonetic_code).expect("writing to String cannot fail");
    if tab_mode {
        write!(output, "　Tab　{}", snapshot.stroke_prefix).expect("writing to String cannot fail");
    }
    writeln!(output).expect("writing to String cannot fail");

    if snapshot.candidates.is_empty() {
        writeln!(output, "（没有匹配的字）").expect("writing to String cannot fail");
    } else {
        for candidate in &snapshot.candidates {
            let label = if candidate.filtered_rank == 10 {
                0
            } else {
                candidate.filtered_rank
            };
            write!(output, "{label} {}　", candidate.character)
                .expect("writing to String cannot fail");
        }
        writeln!(output).expect("writing to String cannot fail");
    }

    if show_controls {
        writeln!(output).expect("writing to String cannot fail");
        if tab_mode && direct_keys {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　　数字选择　退格撤回　Esc返回　q退出"
            )
            .expect("writing to String cannot fail");
        } else if tab_mode {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　　数字选择　-撤回　esc返回　q退出（每次按 Enter）"
            )
            .expect("writing to String cannot fail");
        } else if direct_keys {
            writeln!(output, "数字选择　Tab进入笔画辅助　q退出")
                .expect("writing to String cannot fail");
        } else {
            writeln!(output, "数字选择　t进入笔画辅助　q退出（每次按 Enter）")
                .expect("writing to String cannot fail");
        }
    }
    if let Some(notice) = notice {
        writeln!(output, "{notice}").expect("writing to String cannot fail");
    }
    output
}

pub fn render_shape_lab_details(snapshot: &ShapeLabSnapshot) -> String {
    let mut output = String::new();
    if snapshot.stroke_prefix.is_empty() {
        writeln!(output, "普通候选（尚未进入 Tab 模式）").expect("writing to String cannot fail");
        writeln!(
            output,
            "拼音 {} → 完整双拼码 {}；候选池 {}，其中 {} 个有公开笔画数据",
            snapshot.pinyin,
            snapshot.phonetic_code,
            snapshot.ordinary_pool_size,
            snapshot.candidates_with_stroke_data
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(output, "Tab 笔画过滤：{}", snapshot.stroke_prefix)
            .expect("writing to String cannot fail");
        writeln!(
            output,
            "候选池 {} → {}；保持普通候选的相对顺序",
            snapshot.ordinary_pool_size, snapshot.filtered_pool_size
        )
        .expect("writing to String cannot fail");
    }

    if snapshot.candidates.is_empty() {
        writeln!(output, "  （没有候选匹配）").expect("writing to String cannot fail");
    } else {
        for candidate in &snapshot.candidates {
            if snapshot.stroke_prefix.is_empty() {
                writeln!(
                    output,
                    "{}. {}",
                    candidate.filtered_rank, candidate.character
                )
                .expect("writing to String cannot fail");
            } else {
                writeln!(
                    output,
                    "{}. {}（普通池第 {}）",
                    candidate.filtered_rank, candidate.character, candidate.original_rank
                )
                .expect("writing to String cannot fail");
            }
        }
    }

    if snapshot.stroke_prefix.is_empty() {
        writeln!(
            output,
            "动作投影：{}（完整双拼 {} 字母 + 1 次选择）",
            snapshot.projected_actions_one_selection,
            snapshot.phonetic_code.len()
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(
            output,
            "动作投影：{}（完整双拼 {} 字母 + Tab + {} 个笔画键 + 1 次选择）",
            snapshot.projected_actions_one_selection,
            snapshot.phonetic_code.len(),
            snapshot.stroke_prefix.len()
        )
        .expect("writing to String cannot fail");
    }

    if let Some(expected) = snapshot.expected_character {
        let ordinary = rank_text(snapshot.expected_ordinary_rank);
        let filtered = rank_text(snapshot.expected_filtered_rank);
        let current_label = if snapshot.stroke_prefix.is_empty() {
            "当前视图"
        } else {
            "当前过滤"
        };
        writeln!(
            output,
            "目标 {expected}：普通池{ordinary}；{current_label}{filtered}"
        )
        .expect("writing to String cannot fail");
        if snapshot.expected_stroke_codes.is_empty() {
            writeln!(output, "公开笔画数据：没有记录").expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "公开笔画码：{}",
                snapshot.expected_stroke_codes.join(" / ")
            )
            .expect("writing to String cannot fail");
        }
    }

    output
}

fn rank_text(rank: Option<usize>) -> String {
    rank.map(|rank| format!("第 {rank} 名"))
        .unwrap_or_else(|| "未找到".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        ShapeLabInput, ShapeLabSession, ShapeLabSessionEffect, parse_shape_lab_arguments,
        parse_shape_lab_input, render_shape_lab_details, render_shape_lab_screen,
    };
    use ziranma_decoder::{ShapeLabCandidate, ShapeLabSnapshot};

    #[test]
    fn parses_one_shot_options_in_any_order() {
        let options = parse_shape_lab_arguments(&[
            "--expect".to_owned(),
            "龘".to_owned(),
            "da".to_owned(),
            "--limit".to_owned(),
            "3".to_owned(),
            "--prefix".to_owned(),
            "hp".to_owned(),
        ])
        .unwrap();

        assert_eq!(options.pinyin, "da");
        assert_eq!(options.expected_character, Some('龘'));
        assert_eq!(options.prefix.as_deref(), Some("hp"));
        assert_eq!(options.visible_limit, 3);
        assert!(!options.details);
    }

    #[test]
    fn rejects_multi_character_expectation() {
        let error = parse_shape_lab_arguments(&[
            "shi".to_owned(),
            "--expect".to_owned(),
            "事实".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("恰好"));
    }

    #[test]
    fn detailed_audit_is_explicit() {
        let options = parse_shape_lab_arguments(&[
            "da".to_owned(),
            "--details".to_owned(),
            "--prefix".to_owned(),
            "n".to_owned(),
        ])
        .unwrap();
        assert!(options.details);
    }

    #[test]
    fn filtered_render_keeps_original_rank_visible() {
        let snapshot = ShapeLabSnapshot {
            pinyin: "shi".to_owned(),
            phonetic_code: "ui".to_owned(),
            stroke_prefix: "n".to_owned(),
            ordinary_pool_size: 20,
            candidates_with_stroke_data: 20,
            filtered_pool_size: 2,
            candidates: vec![ShapeLabCandidate {
                character: '事',
                original_rank: 12,
                filtered_rank: 1,
            }],
            expected_character: Some('事'),
            expected_ordinary_rank: Some(12),
            expected_filtered_rank: Some(1),
            expected_stroke_codes: vec!["nsh".to_owned()],
            projected_actions_one_selection: 5,
        };

        let rendered = render_shape_lab_details(&snapshot);
        assert!(rendered.contains("事（普通池第 12）"));
        assert!(rendered.contains("目标 事：普通池第 12 名；当前过滤第 1 名"));
    }

    #[test]
    fn concise_screen_hides_audit_language_and_long_stroke_codes() {
        let snapshot = ShapeLabSnapshot {
            pinyin: "shi".to_owned(),
            phonetic_code: "ui".to_owned(),
            stroke_prefix: "n".to_owned(),
            ordinary_pool_size: 20,
            candidates_with_stroke_data: 20,
            filtered_pool_size: 2,
            candidates: vec![ShapeLabCandidate {
                character: '事',
                original_rank: 12,
                filtered_rank: 1,
            }],
            expected_character: Some('事'),
            expected_ordinary_rank: Some(12),
            expected_filtered_rank: Some(1),
            expected_stroke_codes: vec!["nsh".to_owned()],
            projected_actions_one_selection: 5,
        };

        let rendered = render_shape_lab_screen(&snapshot, true, true, None, true);
        assert!(rendered.contains("目标：事（shi）"));
        assert!(rendered.contains("双拼：ui　Tab　n"));
        assert!(rendered.contains("1 事"));
        assert!(!rendered.contains("候选池"));
        assert!(!rendered.contains("动作投影"));
        assert!(!rendered.contains("nsh"));
    }

    #[test]
    fn session_input_accepts_tab_strokes_selection_and_navigation() {
        assert_eq!(parse_shape_lab_input("t\n"), ShapeLabInput::EnterTab);
        assert_eq!(parse_shape_lab_input("\t\r\n"), ShapeLabInput::EnterTab);
        assert_eq!(
            parse_shape_lab_input("NH\n"),
            ShapeLabInput::Stroke("nh".to_owned())
        );
        assert_eq!(parse_shape_lab_input("0\n"), ShapeLabInput::Select(10));
        assert_eq!(parse_shape_lab_input("-\n"), ShapeLabInput::Backspace);
        assert_eq!(parse_shape_lab_input("esc\n"), ShapeLabInput::LeaveTab);
        assert_eq!(parse_shape_lab_input("q\n"), ShapeLabInput::Quit);
        assert_eq!(parse_shape_lab_input("x\n"), ShapeLabInput::Invalid);
    }

    #[test]
    fn session_scenario_enters_filters_rewinds_and_leaves_tab() {
        let mut session = ShapeLabSession::default();
        assert_eq!(
            session.apply(ShapeLabInput::EnterTab),
            ShapeLabSessionEffect::Continue
        );
        assert!(session.tab_mode());
        session.apply(ShapeLabInput::Stroke("nh".to_owned()));
        assert_eq!(session.active_prefix(), "nh");

        session.apply(ShapeLabInput::Backspace);
        assert_eq!(session.active_prefix(), "n");
        session.apply(ShapeLabInput::LeaveTab);
        assert!(!session.tab_mode());
        assert_eq!(session.active_prefix(), "");

        session.apply(ShapeLabInput::EnterTab);
        assert_eq!(session.active_prefix(), "");
    }

    #[test]
    fn session_scenario_keeps_selection_and_quit_as_effects() {
        let mut session = ShapeLabSession::default();
        assert_eq!(
            session.apply(ShapeLabInput::Select(3)),
            ShapeLabSessionEffect::Select(3)
        );
        assert_eq!(
            session.apply(ShapeLabInput::Quit),
            ShapeLabSessionEffect::Quit
        );
    }

    #[test]
    fn session_scenario_exits_empty_tab_with_backspace() {
        let mut session = ShapeLabSession::default();
        session.apply(ShapeLabInput::EnterTab);
        session.apply(ShapeLabInput::Backspace);
        assert!(!session.tab_mode());
    }

    #[test]
    fn session_scenario_rejects_strokes_before_tab() {
        let mut session = ShapeLabSession::default();
        session.apply(ShapeLabInput::Stroke("n".to_owned()));
        assert_eq!(session.active_prefix(), "");
        assert_eq!(session.notice(), Some("先按 Tab 进入笔画辅助"));
    }
}
