use std::error::Error;
use std::fmt::Write as _;

use ziranma_decoder::{
    MAX_INTERACTIVE_SHAPE_COURSE_TASKS, ShapeCourseDifficulty, ShapeLabSnapshot,
};

use crate::shape_lab_cli::{
    ShapeLabInput, ShapeLabSession, ShapeLabSessionEffect, normalize_shape_lab_input,
    render_shape_lab_screen,
};

pub const SHAPE_COURSE_USAGE: &str = "\
Tab 笔画连续课程

用法：
  cargo run --release -- shape-course [选项]

选项：
  --count <1～50>                题数（默认 10）
  --level <easy|medium|hard|mixed>  一画、两画、三画或混合（默认 mixed）

每题先输入完整双拼，再按 Tab、笔画键和数字；Enter 跳过，Esc 可结束。
课程只使用固定公开快照，不学习、不写文件、不读取私人记录。";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShapeCourseCliOptions {
    pub count: usize,
    pub difficulty: ShapeCourseDifficulty,
}

pub fn parse_shape_course_arguments(
    arguments: &[String],
) -> Result<ShapeCourseCliOptions, Box<dyn Error>> {
    let mut count = 10usize;
    let mut difficulty = ShapeCourseDifficulty::Mixed;
    let mut count_seen = false;
    let mut level_seen = false;
    let mut index = 0usize;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--count" => {
                if count_seen {
                    return Err("shape-course 的 --count 只能使用一次".into());
                }
                count_seen = true;
                index += 1;
                count = arguments
                    .get(index)
                    .ok_or("shape-course 的 --count 后面需要 1～50")?
                    .parse()
                    .map_err(|_| "shape-course 的 --count 必须是 1～50 的整数")?;
                if !(1..=MAX_INTERACTIVE_SHAPE_COURSE_TASKS).contains(&count) {
                    return Err("shape-course 的 --count 必须是 1～50 的整数".into());
                }
            }
            "--level" => {
                if level_seen {
                    return Err("shape-course 的 --level 只能使用一次".into());
                }
                level_seen = true;
                index += 1;
                difficulty = match arguments.get(index).map(String::as_str) {
                    Some("easy") => ShapeCourseDifficulty::Easy,
                    Some("medium") => ShapeCourseDifficulty::Medium,
                    Some("hard") => ShapeCourseDifficulty::Hard,
                    Some("mixed") => ShapeCourseDifficulty::Mixed,
                    _ => {
                        return Err(
                            "shape-course 的 --level 必须是 easy、medium、hard 或 mixed".into()
                        );
                    }
                };
            }
            option => return Err(format!("shape-course 不认识选项 {option:?}").into()),
        }
        index += 1;
    }
    Ok(ShapeCourseCliOptions { count, difficulty })
}

pub fn parse_shape_course_input(raw: &str) -> ShapeLabInput {
    let without_newline = raw.trim_end_matches(['\r', '\n']);
    if without_newline == "\t" {
        return ShapeLabInput::EnterTab;
    }
    let input = without_newline.trim().to_ascii_lowercase();
    match input.as_str() {
        "" | "skip" | "next" => ShapeLabInput::Skip,
        "tab" => ShapeLabInput::EnterTab,
        "-" | "back" | "backspace" => ShapeLabInput::Backspace,
        "esc" | "escape" => ShapeLabInput::LeaveTab,
        "quit" | "exit" => ShapeLabInput::Quit,
        value if value.len() == 1 && value.as_bytes().first().is_some_and(u8::is_ascii_digit) => {
            let digit = usize::from(value.as_bytes()[0] - b'0');
            ShapeLabInput::Select(if digit == 0 { 10 } else { digit })
        }
        value if !value.is_empty() && value.as_bytes().iter().all(u8::is_ascii_alphabetic) => {
            ShapeLabInput::Letters(value.to_owned())
        }
        _ => ShapeLabInput::Invalid,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeCourseAttemptEffect {
    Continue,
    Select(usize),
    Skip,
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeCourseAttempt {
    expected_code: String,
    typed_code: String,
    shape: ShapeLabSession,
}

impl ShapeCourseAttempt {
    pub fn new(expected_code: String) -> Self {
        debug_assert!(!expected_code.is_empty());
        Self {
            expected_code,
            typed_code: String::new(),
            shape: ShapeLabSession::default(),
        }
    }

    pub fn typed_code(&self) -> &str {
        &self.typed_code
    }

    pub fn expected_code(&self) -> &str {
        &self.expected_code
    }

    pub fn phonetic_complete(&self) -> bool {
        self.typed_code == self.expected_code
    }

    pub fn tab_mode(&self) -> bool {
        self.shape.tab_mode()
    }

    pub fn active_prefix(&self) -> &str {
        self.shape.active_prefix()
    }

    pub fn notice(&self) -> Option<&str> {
        self.shape.notice()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.shape.set_notice(notice);
    }

    pub fn apply(&mut self, input: ShapeLabInput) -> ShapeCourseAttemptEffect {
        if !self.phonetic_complete() {
            return self.apply_phonetic_input(input);
        }

        if !self.shape.tab_mode() {
            match input {
                ShapeLabInput::Backspace => {
                    self.typed_code.pop();
                    self.shape.clear_notice();
                    return ShapeCourseAttemptEffect::Continue;
                }
                ShapeLabInput::LeaveTab => {
                    self.typed_code.clear();
                    self.shape.clear_notice();
                    return ShapeCourseAttemptEffect::Continue;
                }
                _ => {}
            }
        }

        match self.shape.apply(normalize_shape_lab_input(input)) {
            ShapeLabSessionEffect::Continue => ShapeCourseAttemptEffect::Continue,
            ShapeLabSessionEffect::Select(rank) => ShapeCourseAttemptEffect::Select(rank),
            ShapeLabSessionEffect::Skip => ShapeCourseAttemptEffect::Skip,
            ShapeLabSessionEffect::Quit => ShapeCourseAttemptEffect::Quit,
        }
    }

    fn apply_phonetic_input(&mut self, input: ShapeLabInput) -> ShapeCourseAttemptEffect {
        self.shape.clear_notice();
        match input {
            ShapeLabInput::Letters(letters) => {
                let remaining = self
                    .expected_code
                    .len()
                    .saturating_sub(self.typed_code.len());
                if letters.len() > remaining {
                    self.shape.set_notice("双拼已满，退格修改");
                } else {
                    self.typed_code.push_str(&letters);
                    if self.typed_code.len() == self.expected_code.len()
                        && !self.phonetic_complete()
                    {
                        self.shape.set_notice("双拼不符，退格修改");
                    }
                }
            }
            ShapeLabInput::Backspace => {
                self.typed_code.pop();
            }
            ShapeLabInput::Skip => return ShapeCourseAttemptEffect::Skip,
            ShapeLabInput::Quit | ShapeLabInput::LeaveTab => {
                return ShapeCourseAttemptEffect::Quit;
            }
            ShapeLabInput::EnterTab => self.shape.set_notice("先输入完整双拼"),
            ShapeLabInput::Stroke(_) | ShapeLabInput::Select(_) | ShapeLabInput::Invalid => {
                self.shape.set_notice("先输入完整双拼")
            }
        }
        ShapeCourseAttemptEffect::Continue
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShapeCourseProgress {
    pub correct: usize,
    pub skipped: usize,
    pub wrong_selections: usize,
    pub phonetic_keys: usize,
    pub tab_entries: usize,
    pub stroke_keys: usize,
    pub backspaces: usize,
}

impl ShapeCourseProgress {
    pub fn observe_input(&mut self, input: &ShapeLabInput, attempt: &ShapeCourseAttempt) {
        match input {
            ShapeLabInput::Letters(letters) if !attempt.phonetic_complete() => {
                self.phonetic_keys += letters.len();
            }
            ShapeLabInput::EnterTab if attempt.phonetic_complete() && !attempt.tab_mode() => {
                self.tab_entries += 1;
            }
            ShapeLabInput::Letters(letters)
                if attempt.phonetic_complete() && !attempt.tab_mode() && letters == "t" =>
            {
                self.tab_entries += 1;
            }
            ShapeLabInput::Letters(letters)
                if attempt.tab_mode()
                    && letters
                        .as_bytes()
                        .iter()
                        .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z')) =>
            {
                self.stroke_keys += letters.len();
            }
            ShapeLabInput::Stroke(strokes) if attempt.tab_mode() => {
                self.stroke_keys += strokes.len();
            }
            ShapeLabInput::Backspace
                if (!attempt.tab_mode() && !attempt.typed_code().is_empty())
                    || (attempt.tab_mode() && !attempt.active_prefix().is_empty()) =>
            {
                self.backspaces += 1;
            }
            _ => {}
        }
    }

    pub fn answered(&self) -> usize {
        self.correct + self.skipped
    }
}

pub fn render_shape_course_screen(
    snapshot: &ShapeLabSnapshot,
    question_index: usize,
    question_count: usize,
    attempt: &ShapeCourseAttempt,
    direct_keys: bool,
) -> String {
    let mut output = format!("第 {question_index}/{question_count} 题\n\n");
    if !attempt.phonetic_complete() {
        if let Some(expected) = snapshot.expected_character {
            writeln!(output, "目标：{expected}（{}）", snapshot.pinyin)
                .expect("writing to String cannot fail");
            writeln!(output).expect("writing to String cannot fail");
        }
        write!(output, "双拼：{}", attempt.typed_code()).expect("writing to String cannot fail");
        for _ in attempt.typed_code().len()..attempt.expected_code().len() {
            output.push('＿');
        }
        writeln!(output).expect("writing to String cannot fail");
        if let Some(notice) = attempt.notice() {
            writeln!(output, "{notice}").expect("writing to String cannot fail");
        }
        writeln!(output).expect("writing to String cannot fail");
        if direct_keys {
            writeln!(output, "字母输入　退格修改　Enter跳过　Esc结束")
                .expect("writing to String cannot fail");
        } else {
            writeln!(output, "输入双拼后按 Enter　-退格　空行跳过　esc结束")
                .expect("writing to String cannot fail");
        }
        return output;
    }

    output.push_str(&render_shape_lab_screen(
        snapshot,
        attempt.tab_mode(),
        false,
        attempt.notice(),
        direct_keys,
    ));
    writeln!(output).expect("writing to String cannot fail");
    match (attempt.tab_mode(), direct_keys) {
        (true, true) => writeln!(
            output,
            "h横　s竖　p撇　n捺　z折　　数字选择　退格撤回　Esc返回　Enter跳过　q结束"
        ),
        (true, false) => writeln!(
            output,
            "h横　s竖　p撇　n捺　z折　　数字选择　-撤回　esc返回　Enter跳过　q结束"
        ),
        (false, true) => writeln!(output, "Tab进入笔画辅助　退格修改双拼　Enter跳过　q结束"),
        (false, false) => writeln!(output, "t进入笔画辅助　-修改双拼　空行跳过　q结束"),
    }
    .expect("writing to String cannot fail");
    output
}

pub fn render_shape_course_summary(progress: &ShapeCourseProgress, total: usize) -> String {
    format!(
        "课程记录\n进度：{}/{}　选中：{}　跳过：{}\n双拼键：{}　Tab：{}　笔画键：{}　退格：{}　误选：{}\n",
        progress.answered(),
        total,
        progress.correct,
        progress.skipped,
        progress.phonetic_keys,
        progress.tab_entries,
        progress.stroke_keys,
        progress.backspaces,
        progress.wrong_selections,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ShapeCourseAttempt, ShapeCourseAttemptEffect, ShapeCourseProgress,
        parse_shape_course_arguments, parse_shape_course_input, render_shape_course_screen,
        render_shape_course_summary,
    };
    use crate::shape_lab_cli::ShapeLabInput;
    use ziranma_decoder::{ShapeCourseDifficulty, ShapeLabCandidate, ShapeLabSnapshot};

    fn snapshot(prefix: &str) -> ShapeLabSnapshot {
        ShapeLabSnapshot {
            pinyin: "da".to_owned(),
            phonetic_code: "da".to_owned(),
            stroke_prefix: prefix.to_owned(),
            ordinary_pool_size: 20,
            candidates_with_stroke_data: 20,
            filtered_pool_size: 1,
            candidates: vec![ShapeLabCandidate {
                character: '龘',
                original_rank: 17,
                filtered_rank: 1,
            }],
            expected_character: Some('龘'),
            expected_ordinary_rank: Some(17),
            expected_filtered_rank: Some(1),
            expected_stroke_codes: vec!["nh".to_owned()],
            projected_actions_one_selection: 5,
        }
    }

    #[test]
    fn parses_bounded_course_options_and_raw_letters() {
        let options = parse_shape_course_arguments(&[
            "--level".to_owned(),
            "hard".to_owned(),
            "--count".to_owned(),
            "5".to_owned(),
        ])
        .unwrap();
        assert_eq!(options.count, 5);
        assert_eq!(options.difficulty, ShapeCourseDifficulty::Hard);
        assert!(parse_shape_course_arguments(&["--count".to_owned(), "0".to_owned()]).is_err());
        assert_eq!(
            parse_shape_course_input("qn\n"),
            ShapeLabInput::Letters("qn".to_owned())
        );
        assert_eq!(
            parse_shape_course_input("q\n"),
            ShapeLabInput::Letters("q".to_owned())
        );
    }

    #[test]
    fn attempt_runs_the_complete_phonetic_tab_shape_flow() {
        let mut attempt = ShapeCourseAttempt::new("da".to_owned());
        assert_eq!(
            attempt.apply(ShapeLabInput::Letters("d".to_owned())),
            ShapeCourseAttemptEffect::Continue
        );
        attempt.apply(ShapeLabInput::Letters("x".to_owned()));
        assert!(!attempt.phonetic_complete());
        assert_eq!(attempt.notice(), Some("双拼不符，退格修改"));
        attempt.apply(ShapeLabInput::Backspace);
        attempt.apply(ShapeLabInput::Letters("a".to_owned()));
        assert!(attempt.phonetic_complete());
        attempt.apply(ShapeLabInput::EnterTab);
        assert!(attempt.tab_mode());
        attempt.apply(ShapeLabInput::Letters("nh".to_owned()));
        assert_eq!(attempt.active_prefix(), "nh");
        assert_eq!(
            attempt.apply(ShapeLabInput::Select(2)),
            ShapeCourseAttemptEffect::Select(2)
        );
        attempt.apply(ShapeLabInput::LeaveTab);
        assert!(!attempt.tab_mode());
        attempt.apply(ShapeLabInput::Backspace);
        assert_eq!(attempt.typed_code(), "d");
    }

    #[test]
    fn q_is_a_phonetic_letter_before_completion_and_quit_afterward() {
        let mut attempt = ShapeCourseAttempt::new("qn".to_owned());
        assert_eq!(
            attempt.apply(ShapeLabInput::Letters("q".to_owned())),
            ShapeCourseAttemptEffect::Continue
        );
        assert_eq!(attempt.typed_code(), "q");
        attempt.apply(ShapeLabInput::Letters("n".to_owned()));
        assert_eq!(
            attempt.apply(ShapeLabInput::Letters("q".to_owned())),
            ShapeCourseAttemptEffect::Quit
        );
    }

    #[test]
    fn progress_and_screens_distinguish_phonetic_and_shape_keys() {
        let mut progress = ShapeCourseProgress::default();
        let mut attempt = ShapeCourseAttempt::new("da".to_owned());
        let letters = ShapeLabInput::Letters("da".to_owned());
        progress.observe_input(&letters, &attempt);
        attempt.apply(letters);
        progress.observe_input(&ShapeLabInput::EnterTab, &attempt);
        attempt.apply(ShapeLabInput::EnterTab);
        let strokes = ShapeLabInput::Letters("nh".to_owned());
        progress.observe_input(&strokes, &attempt);
        attempt.apply(strokes);
        progress.observe_input(&ShapeLabInput::Backspace, &attempt);
        progress.correct = 1;
        assert_eq!(progress.phonetic_keys, 2);
        assert_eq!(progress.tab_entries, 1);
        assert_eq!(progress.stroke_keys, 2);
        assert_eq!(progress.backspaces, 1);

        let mut line_attempt = ShapeCourseAttempt::new("da".to_owned());
        line_attempt.apply(ShapeLabInput::Letters("da".to_owned()));
        let line_tab = ShapeLabInput::Letters("t".to_owned());
        progress.observe_input(&line_tab, &line_attempt);
        line_attempt.apply(line_tab);
        assert_eq!(progress.tab_entries, 2);

        let initial = render_shape_course_screen(
            &snapshot(""),
            1,
            10,
            &ShapeCourseAttempt::new("da".to_owned()),
            true,
        );
        assert!(initial.contains("双拼：＿＿"));
        assert!(!initial.contains("1 龘"));
        let filtered = render_shape_course_screen(&snapshot("nh"), 1, 10, &attempt, true);
        assert!(filtered.contains("1 龘"));

        let summary = render_shape_course_summary(&progress, 10);
        assert!(summary.contains("双拼键：2"));
        assert!(!summary.contains('龘'));
    }
}
