use std::error::Error;
use std::fmt::Write as _;

use ziranma_decoder::{
    MAX_INTERACTIVE_SHAPE_COURSE_TASKS, ShapeCourseDifficulty, ShapeLabSnapshot,
};

use crate::shape_lab_cli::{ShapeLabInput, ShapeLabSession, render_shape_lab_screen};

pub const SHAPE_COURSE_USAGE: &str = "\
Tab 笔画连续课程

用法：
  cargo run --release -- shape-course [选项]

选项：
  --count <1～50>                题数（默认 10）
  --level <easy|medium|hard|mixed>  一画、两画、三画或混合（默认 mixed）

Windows 交互终端中每键立即生效；Enter 跳过当前题，q 随时结束。
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShapeCourseProgress {
    pub correct: usize,
    pub skipped: usize,
    pub wrong_selections: usize,
    pub tab_entries: usize,
    pub stroke_keys: usize,
    pub backspaces: usize,
}

impl ShapeCourseProgress {
    pub fn observe_input(&mut self, input: &ShapeLabInput, session: &ShapeLabSession) {
        match input {
            ShapeLabInput::EnterTab if !session.tab_mode() => self.tab_entries += 1,
            ShapeLabInput::Stroke(strokes) if session.tab_mode() => {
                self.stroke_keys += strokes.len();
            }
            ShapeLabInput::Backspace
                if session.tab_mode() && !session.active_prefix().is_empty() =>
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
    session: &ShapeLabSession,
    direct_keys: bool,
) -> String {
    let mut output = format!("第 {question_index}/{question_count} 题\n\n");
    output.push_str(&render_shape_lab_screen(
        snapshot,
        session.tab_mode(),
        false,
        session.notice(),
        direct_keys,
    ));
    writeln!(output).expect("writing to String cannot fail");
    match (session.tab_mode(), direct_keys) {
        (true, true) => writeln!(
            output,
            "h横　s竖　p撇　n捺　z折　　数字选择　退格撤回　Esc返回　Enter跳过　q结束"
        ),
        (true, false) => writeln!(
            output,
            "h横　s竖　p撇　n捺　z折　　数字选择　-撤回　esc返回　Enter跳过　q结束"
        ),
        (false, true) => writeln!(output, "Tab进入笔画辅助　Enter跳过　q结束"),
        (false, false) => writeln!(output, "t进入笔画辅助　Enter跳过　q结束"),
    }
    .expect("writing to String cannot fail");
    output
}

pub fn render_shape_course_summary(progress: &ShapeCourseProgress, total: usize) -> String {
    format!(
        "课程记录\n进度：{}/{}　选中：{}　跳过：{}\nTab：{}　笔画键：{}　退格：{}　误选：{}\n",
        progress.answered(),
        total,
        progress.correct,
        progress.skipped,
        progress.tab_entries,
        progress.stroke_keys,
        progress.backspaces,
        progress.wrong_selections,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ShapeCourseProgress, parse_shape_course_arguments, render_shape_course_screen,
        render_shape_course_summary,
    };
    use crate::shape_lab_cli::{ShapeLabInput, ShapeLabSession};
    use ziranma_decoder::{ShapeCourseDifficulty, ShapeLabCandidate, ShapeLabSnapshot};

    #[test]
    fn parses_bounded_course_options() {
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
    }

    #[test]
    fn progress_counts_only_effective_course_actions() {
        let mut progress = ShapeCourseProgress::default();
        let mut session = ShapeLabSession::default();
        progress.observe_input(&ShapeLabInput::Stroke("n".to_owned()), &session);
        progress.observe_input(&ShapeLabInput::EnterTab, &session);
        session.apply(ShapeLabInput::EnterTab);
        progress.observe_input(&ShapeLabInput::Stroke("nh".to_owned()), &session);
        session.apply(ShapeLabInput::Stroke("nh".to_owned()));
        progress.observe_input(&ShapeLabInput::Backspace, &session);
        progress.correct = 1;
        progress.skipped = 1;
        assert_eq!(progress.tab_entries, 1);
        assert_eq!(progress.stroke_keys, 2);
        assert_eq!(progress.backspaces, 1);
        assert_eq!(progress.answered(), 2);
    }

    #[test]
    fn course_screen_and_summary_stay_compact() {
        let snapshot = ShapeLabSnapshot {
            pinyin: "da".to_owned(),
            phonetic_code: "da".to_owned(),
            stroke_prefix: "n".to_owned(),
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
        };
        let mut session = ShapeLabSession::default();
        session.apply(ShapeLabInput::EnterTab);
        session.apply(ShapeLabInput::Stroke("n".to_owned()));
        let screen = render_shape_course_screen(&snapshot, 2, 10, &session, true);
        assert!(screen.contains("第 2/10 题"));
        assert!(screen.contains("Enter跳过"));
        assert!(!screen.contains("普通池第"));

        let summary = render_shape_course_summary(
            &ShapeCourseProgress {
                correct: 1,
                skipped: 1,
                wrong_selections: 2,
                tab_entries: 2,
                stroke_keys: 4,
                backspaces: 1,
            },
            10,
        );
        assert!(summary.contains("进度：2/10"));
        assert!(!summary.contains('龘'));
    }
}
