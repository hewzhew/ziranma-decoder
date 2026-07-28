use std::error::Error;
use std::fmt::Write as _;

use ziranma_decoder::LexiconEntry;

pub const TYPING_LAB_USAGE: &str = "\
连续输入实验台

用法：
  cargo run --release -- typing-lab [--limit <1～10>]

直接输入连续双拼；空格或 Enter 选择首项，数字选择候选，退格修改。
完整单字码可以按 Tab 进入笔画辅助，再按 h 横、s 竖、p 撇、n 捺、z 折。
Esc 返回或清空；输入为空时再按 Esc 退出。q 始终是普通双拼字母。
重定向输入时使用 :tab、:esc、:quit 等行命令，避免占用任何字母键。

实验台使用固定公开词典与笔画快照，不读取私人记录、不学习、不写文件。
输入、候选和已选文字会留在当前终端，请自行管理终端滚动记录。";

pub const DEFAULT_TYPING_LAB_LIMIT: usize = 5;
pub const MAX_TYPING_LAB_LIMIT: usize = 10;
const MAX_PHONETIC_KEYS: usize = 64;
const MAX_COMMITTED_CHARACTERS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypingLabOptions {
    pub visible_limit: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypingLabInput {
    Letters(String),
    Confirm,
    Select(usize),
    Backspace,
    EnterTab,
    Escape,
    Quit,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypingLabEffect {
    Continue,
    Confirm,
    Select(usize),
    RequestTab,
    Quit,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypingLabSession {
    committed: String,
    phonetic: String,
    shape_pinyin: Option<String>,
    stroke_prefix: String,
    notice: Option<String>,
}

impl TypingLabSession {
    pub fn committed(&self) -> &str {
        &self.committed
    }

    pub fn phonetic(&self) -> &str {
        &self.phonetic
    }

    pub fn tab_mode(&self) -> bool {
        self.shape_pinyin.is_some()
    }

    pub fn shape_pinyin(&self) -> Option<&str> {
        self.shape_pinyin.as_deref()
    }

    pub fn stroke_prefix(&self) -> &str {
        &self.stroke_prefix
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn enter_tab(&mut self, pinyin: impl Into<String>) {
        self.shape_pinyin = Some(pinyin.into());
        self.stroke_prefix.clear();
        self.notice = None;
    }

    pub fn commit(&mut self, text: &str) -> bool {
        if self
            .committed
            .chars()
            .count()
            .saturating_add(text.chars().count())
            > MAX_COMMITTED_CHARACTERS
        {
            self.set_notice("已选文字太长，请结束本轮");
            return false;
        }
        self.committed.push_str(text);
        self.phonetic.clear();
        self.leave_tab();
        self.notice = None;
        true
    }

    pub fn apply(&mut self, input: TypingLabInput) -> TypingLabEffect {
        self.notice = None;
        match input {
            TypingLabInput::Letters(letters) if self.tab_mode() => {
                if letters
                    .as_bytes()
                    .iter()
                    .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z'))
                {
                    self.stroke_prefix.push_str(&letters);
                } else {
                    self.set_notice("笔画只用 h s p n z");
                }
            }
            TypingLabInput::Letters(letters)
                if letters.as_bytes().iter().all(u8::is_ascii_lowercase) =>
            {
                let available = MAX_PHONETIC_KEYS.saturating_sub(self.phonetic.len());
                self.phonetic.extend(letters.chars().take(available));
                if letters.len() > available {
                    self.set_notice("本轮最多输入 64 个字母");
                }
            }
            TypingLabInput::Letters(_) | TypingLabInput::Invalid => {
                self.set_notice("没有这个操作");
            }
            TypingLabInput::Confirm if !self.phonetic.is_empty() => {
                return TypingLabEffect::Confirm;
            }
            TypingLabInput::Confirm => {}
            TypingLabInput::Select(rank) if !self.phonetic.is_empty() => {
                return TypingLabEffect::Select(rank);
            }
            TypingLabInput::Select(_) => {}
            TypingLabInput::Backspace if self.tab_mode() => {
                if self.stroke_prefix.pop().is_none() {
                    self.leave_tab();
                }
            }
            TypingLabInput::Backspace => {
                self.phonetic.pop();
            }
            TypingLabInput::EnterTab if !self.phonetic.is_empty() && !self.tab_mode() => {
                return TypingLabEffect::RequestTab;
            }
            TypingLabInput::EnterTab => {}
            TypingLabInput::Escape if self.tab_mode() => self.leave_tab(),
            TypingLabInput::Escape if !self.phonetic.is_empty() => self.phonetic.clear(),
            TypingLabInput::Escape | TypingLabInput::Quit => return TypingLabEffect::Quit,
        }
        TypingLabEffect::Continue
    }

    fn leave_tab(&mut self) {
        self.shape_pinyin = None;
        self.stroke_prefix.clear();
    }
}

pub fn parse_typing_lab_arguments(
    arguments: &[String],
) -> Result<TypingLabOptions, Box<dyn Error>> {
    let mut visible_limit = DEFAULT_TYPING_LAB_LIMIT;
    let mut limit_seen = false;
    let mut index = 0usize;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--limit" => {
                if limit_seen {
                    return Err("typing-lab 的 --limit 只能使用一次".into());
                }
                limit_seen = true;
                index = index.saturating_add(1);
                visible_limit = arguments
                    .get(index)
                    .ok_or("typing-lab 的 --limit 后面需要 1～10")?
                    .parse::<usize>()
                    .map_err(|_| "typing-lab 的 --limit 必须是 1～10 的整数")?;
            }
            value => return Err(format!("typing-lab 不认识参数 {value:?}").into()),
        }
        index = index.saturating_add(1);
    }
    if !(1..=MAX_TYPING_LAB_LIMIT).contains(&visible_limit) {
        return Err("typing-lab 的 --limit 必须是 1～10 的整数".into());
    }
    Ok(TypingLabOptions { visible_limit })
}

pub fn parse_typing_lab_input(raw: &str) -> TypingLabInput {
    let input = raw.trim().to_ascii_lowercase();
    match input.as_str() {
        "" | ":space" | ":enter" => TypingLabInput::Confirm,
        ":tab" => TypingLabInput::EnterTab,
        "-" | ":back" | ":backspace" => TypingLabInput::Backspace,
        ":esc" | ":escape" => TypingLabInput::Escape,
        ":quit" | ":exit" => TypingLabInput::Quit,
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_digit() => {
            let digit = usize::from(value.as_bytes()[0] - b'0');
            TypingLabInput::Select(if digit == 0 { 10 } else { digit })
        }
        value if !value.is_empty() && value.as_bytes().iter().all(u8::is_ascii_lowercase) => {
            TypingLabInput::Letters(value.to_owned())
        }
        _ => TypingLabInput::Invalid,
    }
}

pub fn find_single_character_pinyin<'a>(
    entries: &'a [LexiconEntry],
    phonetic_code: &str,
) -> Option<&'a str> {
    entries
        .iter()
        .filter(|entry| {
            entry.code.as_str() == phonetic_code
                && entry.syllable_codes.len() == 1
                && entry.text.chars().count() == 1
        })
        .max_by_key(|entry| entry.frequency)
        .map(|entry| entry.pinyin.as_str())
}

pub fn render_typing_lab_screen(
    session: &TypingLabSession,
    candidates: &[String],
    direct_keys: bool,
) -> String {
    let mut output = String::new();
    if !session.committed().is_empty() {
        writeln!(output, "已选：{}", session.committed()).expect("writing to String cannot fail");
    }
    if session.tab_mode() {
        writeln!(
            output,
            "输入：{}　Tab {}",
            session.phonetic(),
            session.stroke_prefix()
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(output, "输入：{}", session.phonetic()).expect("writing to String cannot fail");
    }
    writeln!(output).expect("writing to String cannot fail");

    if session.phonetic().is_empty() {
        writeln!(output, "（开始输入双拼）").expect("writing to String cannot fail");
    } else if candidates.is_empty() {
        writeln!(output, "（没有候选）").expect("writing to String cannot fail");
    } else {
        for (index, candidate) in candidates.iter().enumerate() {
            let rank = index + 1;
            let label = if rank == 10 { 0 } else { rank };
            writeln!(output, "{label} {candidate}").expect("writing to String cannot fail");
        }
    }

    writeln!(output).expect("writing to String cannot fail");
    if session.tab_mode() {
        if direct_keys {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　　数字选择　退格撤回　Esc返回"
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　　数字选择　-撤回　:esc返回（每次按 Enter）"
            )
            .expect("writing to String cannot fail");
        }
    } else if direct_keys {
        writeln!(
            output,
            "空格首选　数字选择　退格修改　Tab找字　Esc清空/退出"
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(
            output,
            "空行首选　数字选择　-退格　:tab找字　:esc清空　:quit退出（每次按 Enter）"
        )
        .expect("writing to String cannot fail");
    }
    if let Some(notice) = session.notice() {
        writeln!(output, "{notice}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ziranma_decoder::{KeySequence, LexiconEntry};

    fn entry(text: &str, pinyin: &str, code: &str, frequency: u64) -> LexiconEntry {
        LexiconEntry {
            text: text.to_owned(),
            pinyin: pinyin.to_owned(),
            code: KeySequence::new(code).unwrap(),
            syllable_codes: vec![KeySequence::new(code).unwrap()],
            frequency,
        }
    }

    #[test]
    fn parses_bounded_options_and_keeps_q_as_a_letter() {
        assert_eq!(
            parse_typing_lab_arguments(&[]).unwrap(),
            TypingLabOptions {
                visible_limit: DEFAULT_TYPING_LAB_LIMIT
            }
        );
        assert_eq!(
            parse_typing_lab_arguments(&["--limit".to_owned(), "10".to_owned()]).unwrap(),
            TypingLabOptions { visible_limit: 10 }
        );
        assert!(parse_typing_lab_arguments(&["--limit".to_owned(), "0".to_owned()]).is_err());
        assert_eq!(
            parse_typing_lab_input("q"),
            TypingLabInput::Letters("q".to_owned())
        );
        assert_eq!(
            parse_typing_lab_input("t"),
            TypingLabInput::Letters("t".to_owned())
        );
        assert_eq!(parse_typing_lab_input(":tab"), TypingLabInput::EnterTab);
        assert_eq!(parse_typing_lab_input(""), TypingLabInput::Confirm);
    }

    #[test]
    fn session_edits_selects_and_enters_shape_without_stealing_phonetic_keys() {
        let mut session = TypingLabSession::default();
        assert_eq!(
            session.apply(TypingLabInput::Letters("da".to_owned())),
            TypingLabEffect::Continue
        );
        assert_eq!(
            session.apply(TypingLabInput::EnterTab),
            TypingLabEffect::RequestTab
        );
        session.enter_tab("da");
        session.apply(TypingLabInput::Letters("nh".to_owned()));
        assert_eq!(session.stroke_prefix(), "nh");
        session.apply(TypingLabInput::Letters("a".to_owned()));
        assert_eq!(session.stroke_prefix(), "nh");
        assert_eq!(session.notice(), Some("笔画只用 h s p n z"));
        session.apply(TypingLabInput::Escape);
        assert!(!session.tab_mode());
        assert_eq!(
            session.apply(TypingLabInput::Select(2)),
            TypingLabEffect::Select(2)
        );
        assert!(session.commit("大"));
        assert_eq!(session.committed(), "大");
        assert!(session.phonetic().is_empty());
    }

    #[test]
    fn escape_clears_before_it_quits() {
        let mut session = TypingLabSession::default();
        session.apply(TypingLabInput::Letters("ni".to_owned()));
        assert_eq!(
            session.apply(TypingLabInput::Escape),
            TypingLabEffect::Continue
        );
        assert!(session.phonetic().is_empty());
        assert_eq!(session.apply(TypingLabInput::Escape), TypingLabEffect::Quit);
    }

    #[test]
    fn shape_lookup_prefers_the_highest_frequency_single_character() {
        let entries = vec![
            entry("答", "da", "da", 10),
            entry("大", "da", "da", 20),
            LexiconEntry {
                text: "搭配".to_owned(),
                pinyin: "da pei".to_owned(),
                code: KeySequence::new("dapw").unwrap(),
                syllable_codes: vec![
                    KeySequence::new("da").unwrap(),
                    KeySequence::new("pw").unwrap(),
                ],
                frequency: 100,
            },
        ];
        assert_eq!(find_single_character_pinyin(&entries, "da"), Some("da"));
        assert_eq!(find_single_character_pinyin(&entries, "xx"), None);
    }

    #[test]
    fn concise_screen_contains_only_the_live_input_candidates_and_controls() {
        let mut session = TypingLabSession::default();
        session.apply(TypingLabInput::Letters("mafmkm".to_owned()));
        let rendered = render_typing_lab_screen(
            &session,
            &["麻烦猫猫".to_owned(), "麻烦毛毛".to_owned()],
            true,
        );
        assert!(rendered.contains("输入：mafmkm"));
        assert!(rendered.contains("1 麻烦猫猫"));
        assert!(rendered.contains("Tab找字"));
        assert!(!rendered.contains("评分"));
        assert!(!rendered.contains("实验排序"));
    }
}
