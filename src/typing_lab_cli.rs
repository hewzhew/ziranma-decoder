use std::error::Error;
use std::fmt::Write as _;
use std::ops::Range;

use ziranma_decoder::{
    CompositionEffect, CompositionInput, CompositionSession, LexiconEntry, SessionSelectionMemory,
};

pub const TYPING_LAB_USAGE: &str = "\
连续输入实验台

用法：
  cargo run --release -- typing-lab [--limit <1～10>]

直接输入连续双拼；空格或 Enter 选择首项，数字选择候选，退格修改。
减号向前翻页，加号（或等号）向后翻页；PageUp / PageDown 也可使用。
本轮选过的同码候选会优先显示，退出后清空。
Shift+Tab 打开独立的换序候选，用于查看相邻两键颠倒的结果；Esc 返回。
完整单字码可以按 Tab 进入笔画辅助，再按 h 横、s 竖、p 撇、n 捺、z 折。
Esc 返回或清空；输入为空时再按 Esc 退出。q 始终是普通双拼字母。
重定向输入时也可使用 -、+、= 或 :prev、:next 等行命令。

实验台使用固定公开词典与笔画快照，不读取私人记录、不写文件。
它只在内存中记住本轮显式选择；输入和候选仍可能进入终端捕获或录屏。";

pub const DEFAULT_TYPING_LAB_LIMIT: usize = 5;
pub const MAX_TYPING_LAB_LIMIT: usize = 10;
pub const TYPING_LAB_CANDIDATE_POOL_DEPTH: usize = 200;
const MAX_COMMITTED_CHARACTERS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypingLabOptions {
    pub visible_limit: usize,
}

pub type TypingLabSelectionMemory = SessionSelectionMemory;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypingLabInput {
    Letters(String),
    Confirm,
    Select(usize),
    Backspace,
    PreviousPage,
    NextPage,
    EnterTab,
    EnterRecovery,
    Escape,
    Quit,
    Invalid,
}

impl TypingLabInput {
    fn into_composition(self) -> Option<CompositionInput> {
        Some(match self {
            Self::Letters(letters) => CompositionInput::Letters(letters),
            Self::Confirm => CompositionInput::Confirm,
            Self::Select(rank) => CompositionInput::Select(rank),
            Self::Backspace => CompositionInput::Backspace,
            Self::PreviousPage => CompositionInput::PreviousPage,
            Self::NextPage => CompositionInput::NextPage,
            Self::EnterTab => CompositionInput::EnterTab,
            Self::EnterRecovery => CompositionInput::EnterRecovery,
            Self::Escape => CompositionInput::Escape,
            Self::Invalid => CompositionInput::Invalid,
            Self::Quit => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypingLabEffect {
    Continue,
    Confirm,
    Select(usize),
    PreviousPage,
    NextPage,
    RequestTab,
    Quit,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypingLabSession {
    committed: String,
    composition: CompositionSession,
}

impl TypingLabSession {
    pub fn committed(&self) -> &str {
        &self.committed
    }

    pub fn phonetic(&self) -> &str {
        self.composition.phonetic()
    }

    pub fn tab_mode(&self) -> bool {
        self.composition.tab_mode()
    }

    pub fn recovery_mode(&self) -> bool {
        self.composition.recovery_mode()
    }

    pub fn shape_pinyin(&self) -> Option<&str> {
        self.composition.shape_pinyin()
    }

    pub fn stroke_prefix(&self) -> &str {
        self.composition.stroke_prefix()
    }

    pub fn notice(&self) -> Option<&str> {
        self.composition.notice()
    }

    pub fn candidate_page_start(&self) -> usize {
        self.composition.candidate_page_start()
    }

    pub fn visible_candidate_range(
        &self,
        candidate_count: usize,
        page_size: usize,
    ) -> Range<usize> {
        self.composition
            .visible_candidate_range(candidate_count, page_size)
    }

    pub fn previous_candidate_page(&mut self, page_size: usize) {
        self.composition.previous_candidate_page(page_size);
    }

    pub fn next_candidate_page(
        &mut self,
        candidate_count: usize,
        page_size: usize,
        candidate_limit: usize,
    ) {
        self.composition
            .next_candidate_page(candidate_count, page_size, candidate_limit);
    }

    pub fn normalize_candidate_page(&mut self, candidate_count: usize, page_size: usize) {
        self.composition
            .normalize_candidate_page(candidate_count, page_size);
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.composition.set_notice(notice);
    }

    pub fn enter_tab(&mut self, pinyin: impl Into<String>) {
        self.composition.enter_tab(pinyin);
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
        self.composition.finish_commit();
        true
    }

    pub fn apply(&mut self, input: TypingLabInput) -> TypingLabEffect {
        let exit_on_pass_through = matches!(input, TypingLabInput::Escape);
        let Some(input) = input.into_composition() else {
            return TypingLabEffect::Quit;
        };
        match self.composition.apply(input) {
            CompositionEffect::Continue => TypingLabEffect::Continue,
            CompositionEffect::Confirm => TypingLabEffect::Confirm,
            CompositionEffect::Select(rank) => TypingLabEffect::Select(rank),
            CompositionEffect::PreviousPage => TypingLabEffect::PreviousPage,
            CompositionEffect::NextPage => TypingLabEffect::NextPage,
            CompositionEffect::RequestTab => TypingLabEffect::RequestTab,
            CompositionEffect::PassThrough if exit_on_pass_through => TypingLabEffect::Quit,
            CompositionEffect::PassThrough => TypingLabEffect::Continue,
        }
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
        ":recover" | ":recovery" => TypingLabInput::EnterRecovery,
        "-" | ":prev" | ":pageup" => TypingLabInput::PreviousPage,
        "+" | "=" | ":next" | ":pagedown" => TypingLabInput::NextPage,
        ":back" | ":backspace" => TypingLabInput::Backspace,
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
    candidate_count: usize,
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
    } else if session.recovery_mode() {
        writeln!(output, "输入：{}　换序候选", session.phonetic())
            .expect("writing to String cannot fail");
    } else {
        writeln!(output, "输入：{}", session.phonetic()).expect("writing to String cannot fail");
    }
    if session.phonetic().is_empty() {
        writeln!(output, "开始输入双拼").expect("writing to String cannot fail");
    } else if candidates.is_empty() {
        writeln!(output, "没有候选").expect("writing to String cannot fail");
    } else {
        for (index, candidate) in candidates.iter().enumerate() {
            let rank = index + 1;
            let label = if rank == 10 { 0 } else { rank };
            if index > 0 {
                write!(output, "　").expect("writing to String cannot fail");
            }
            write!(output, "{label} {candidate}").expect("writing to String cannot fail");
        }
        writeln!(output).expect("writing to String cannot fail");
    }

    let range = session.visible_candidate_range(candidate_count, candidates.len().max(1));
    if candidate_count > candidates.len() && !candidates.is_empty() {
        write!(
            output,
            "{}–{} / {}　",
            range.start + 1,
            range.end,
            candidate_count
        )
        .expect("writing to String cannot fail");
    }
    if session.tab_mode() {
        if direct_keys {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　数字选择　-前页　+后页　退格撤回　Esc返回"
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "h横　s竖　p撇　n捺　z折　数字选择　-前页　+后页　:back撤回　:esc返回"
            )
            .expect("writing to String cannot fail");
        }
    } else if session.recovery_mode() {
        if direct_keys {
            writeln!(output, "空格首选　数字选择　Esc返回").expect("writing to String cannot fail");
        } else {
            writeln!(output, "空行首选　数字选择　:esc返回")
                .expect("writing to String cannot fail");
        }
    } else if direct_keys {
        writeln!(
            output,
            "空格首选　数字选择　-前页　+后页　Shift+Tab换序　Tab找字　退格修改　Esc清空/退出"
        )
        .expect("writing to String cannot fail");
    } else {
        writeln!(
            output,
            "空行首选　数字选择　-前页　+后页　:recover换序　:tab找字　:back退格　:esc清空　:quit退出"
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
        assert_eq!(
            parse_typing_lab_input(":recover"),
            TypingLabInput::EnterRecovery
        );
        assert_eq!(parse_typing_lab_input(":next"), TypingLabInput::NextPage);
        assert_eq!(parse_typing_lab_input("-"), TypingLabInput::PreviousPage);
        assert_eq!(parse_typing_lab_input("+"), TypingLabInput::NextPage);
        assert_eq!(parse_typing_lab_input("="), TypingLabInput::NextPage);
        assert_eq!(parse_typing_lab_input(":back"), TypingLabInput::Backspace);
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
    fn recovery_is_explicit_and_escape_returns_without_clearing_input() {
        let mut session = TypingLabSession::default();
        session.apply(TypingLabInput::Letters("mafkmm".to_owned()));
        assert_eq!(
            session.apply(TypingLabInput::EnterRecovery),
            TypingLabEffect::Continue
        );
        assert!(session.recovery_mode());
        assert_eq!(session.phonetic(), "mafkmm");

        session.apply(TypingLabInput::Escape);
        assert!(!session.recovery_mode());
        assert_eq!(session.phonetic(), "mafkmm");

        session.apply(TypingLabInput::EnterRecovery);
        session.apply(TypingLabInput::Backspace);
        assert!(!session.recovery_mode());
        assert_eq!(session.phonetic(), "mafkm");
    }

    #[test]
    fn pages_candidates_without_changing_the_phonetic_input() {
        let mut session = TypingLabSession::default();
        session.apply(TypingLabInput::Letters("wuwa".to_owned()));
        assert_eq!(
            session.apply(TypingLabInput::NextPage),
            TypingLabEffect::NextPage
        );
        session.next_candidate_page(17, 5, 17);
        assert_eq!(session.visible_candidate_range(17, 5), 5..10);
        session.previous_candidate_page(5);
        assert_eq!(session.visible_candidate_range(17, 5), 0..5);
        assert_eq!(session.phonetic(), "wuwa");

        let mut lazy_session = TypingLabSession::default();
        lazy_session.apply(TypingLabInput::Letters("da".to_owned()));
        lazy_session.next_candidate_page(5, 5, 200);
        assert_eq!(lazy_session.visible_candidate_range(200, 5), 5..10);
        lazy_session.normalize_candidate_page(5, 5);
        assert_eq!(lazy_session.visible_candidate_range(5, 5), 0..5);
        assert_eq!(lazy_session.notice(), Some("已经是最后一页"));
    }

    #[test]
    fn session_selection_memory_promotes_only_the_same_code() {
        let decoder = ziranma_decoder::Decoder::new(vec![
            entry("大", "da", "da", 20),
            entry("答", "da", "da", 10),
        ]);
        let mut candidates = decoder.decode_sentence("da", 10).unwrap();
        assert_eq!(candidates[0].text, "大");

        let mut memory = TypingLabSelectionMemory::default();
        let answer = candidates
            .iter()
            .find(|candidate| candidate.text == "答")
            .unwrap()
            .clone();
        memory.remember("da", &answer);
        assert!(memory.promote("da", &mut candidates));
        assert_eq!(candidates[0].text, "答");

        let answer = candidates[0].clone();
        let mut shallow_candidates = vec![
            decoder
                .decode_sentence("da", 1)
                .unwrap()
                .into_iter()
                .next()
                .unwrap(),
        ];
        assert_eq!(shallow_candidates[0].text, "大");
        let mut injected_memory = TypingLabSelectionMemory::default();
        injected_memory.remember("da", &answer);
        assert!(injected_memory.promote("da", &mut shallow_candidates));
        assert_eq!(shallow_candidates.len(), 1);
        assert_eq!(shallow_candidates[0].text, "答");

        let before = candidates.clone();
        assert!(!memory.promote("xx", &mut candidates));
        assert_eq!(candidates, before);
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
            12,
            true,
        );
        assert!(rendered.contains("输入：mafmkm"));
        assert!(rendered.contains("1 麻烦猫猫"));
        assert!(rendered.contains("1–2 / 12"));
        assert!(rendered.contains("Tab找字"));
        assert!(!rendered.contains("评分"));
        assert!(!rendered.contains("实验排序"));
    }
}
