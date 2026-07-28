//! Explicit local inspection of one encrypted private capture session.

use std::fmt::Write as _;
use std::path::Path;

use ziranma_decoder::{
    DeltaPositionEvidence, ProtectedSessionSegment, RawKey, RevisionRecord, TextDelta,
    TimedTrackerOutput, TrackerOutput,
};
#[cfg(windows)]
use ziranma_decoder::{ProtectedSessionReader, WindowsUserDataProtector};

const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 500;
const DEFAULT_TEXT_CHARACTERS: usize = 160;

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Review {
        session_id: String,
        from: usize,
        limit: usize,
        full_text: bool,
        details: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Review {
            session_id,
            from,
            limit,
            full_text,
            details,
        } => review_session(&session_id, from, limit, full_text, details)?,
    }
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(Options::Help);
    };
    if command == "--help" || command == "-h" {
        if arguments.next().is_some() {
            return Err("--help must be used by itself".into());
        }
        return Ok(Options::Help);
    }
    if command != "review" {
        return Err("unknown personal-lab command; value was suppressed".into());
    }

    let mut session_id = None;
    let mut from = 1_usize;
    let mut limit = DEFAULT_LIMIT;
    let mut from_seen = false;
    let mut limit_seen = false;
    let mut full_text = false;
    let mut details = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--session" => {
                if session_id.is_some() {
                    return Err("--session can be given only once".into());
                }
                let value = arguments.next().ok_or("--session requires a session id")?;
                validate_session_id(&value)?;
                session_id = Some(value);
            }
            "--from" => {
                if from_seen {
                    return Err("--from can be given only once".into());
                }
                from = arguments.next().ok_or("--from requires a value")?.parse()?;
                if from == 0 {
                    return Err("--from uses one-based event numbers and must be positive".into());
                }
                from_seen = true;
            }
            "--limit" => {
                if limit_seen {
                    return Err("--limit can be given only once".into());
                }
                limit = arguments
                    .next()
                    .ok_or("--limit requires a value")?
                    .parse()?;
                if limit == 0 || limit > MAX_LIMIT {
                    return Err(format!("--limit must be between 1 and {MAX_LIMIT}").into());
                }
                limit_seen = true;
            }
            "--full-text" => {
                if full_text {
                    return Err("--full-text can be given only once".into());
                }
                full_text = true;
            }
            "--details" => {
                if details {
                    return Err("--details can be given only once".into());
                }
                details = true;
            }
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err("unknown review argument; value was suppressed".into()),
        }
    }

    Ok(Options::Review {
        session_id: session_id.ok_or("review requires exactly one --session id")?,
        from,
        limit,
        full_text,
        details,
    })
}

fn validate_session_id(value: &str) -> Result<(), Box<dyn std::error::Error>> {
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

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --bin personal-lab -- review --session <SESSION> \
         [--from <ONE_BASED_EVENT>] [--limit <1..={MAX_LIMIT}>] [--full-text] [--details]"
    );
    eprintln!(
        "Decrypts only the explicitly named local session, prints private text to this terminal, \
         and performs no learning, file writes, or network access."
    );
    eprintln!(
        "The default is a compact one-line timeline with {DEFAULT_LIMIT} events per page. \
         --details reveals segment, timing, positions, and location evidence; --full-text disables \
         long-text shortening."
    );
}

#[cfg(windows)]
fn review_session(
    session_id: &str,
    from: usize,
    limit: usize,
    full_text: bool,
    details: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let segments =
        ProtectedSessionReader::new(manifest_dir, WindowsUserDataProtector).load(session_id)?;
    print_review(session_id, &segments, from, limit, full_text, details)
}

#[cfg(not(windows))]
fn review_session(
    _session_id: &str,
    _from: usize,
    _limit: usize,
    _full_text: bool,
    _details: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("private session review requires the same Windows user that recorded it".into())
}

fn print_review(
    session_id: &str,
    segments: &[ProtectedSessionSegment],
    from: usize,
    limit: usize,
    full_text: bool,
    details: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let total_events = segments
        .iter()
        .map(|segment| segment.capsule.events().len())
        .sum::<usize>();
    if from > total_events {
        return Err(
            format!("--from {from} exceeds this session's {total_events} recorded events").into(),
        );
    }
    let first = segments
        .first()
        .ok_or("the protected session contains no segments")?;
    let end_exclusive = from
        .saturating_sub(1)
        .saturating_add(limit)
        .min(total_events);

    let detail_metadata = if details {
        Some((
            segments.len(),
            first.metadata.session_kind.as_str(),
            first.metadata.producer_version.as_str(),
        ))
    } else {
        None
    };
    print!(
        "{}",
        format_review_header(
            session_id,
            from,
            end_exclusive,
            total_events,
            detail_metadata
        )
    );

    let mut global_index = 0_usize;
    for segment in segments {
        for event in segment.capsule.events() {
            global_index = global_index.saturating_add(1);
            if global_index < from || global_index > end_exclusive {
                continue;
            }
            let rendered = if details {
                format_detailed_event(global_index, segment.metadata.sequence, event, full_text)
            } else {
                format_compact_event(global_index, event, full_text)
            };
            print!("{rendered}");
        }
    }

    if end_exclusive < total_events {
        println!("\n下一页：--from {}", end_exclusive + 1);
    }
    Ok(())
}

fn format_review_header(
    session_id: &str,
    from: usize,
    through: usize,
    total_events: usize,
    details: Option<(usize, &str, &str)>,
) -> String {
    let mut rendered = String::new();
    if details.is_some() {
        let _ = writeln!(
            rendered,
            "PRIVATE_REVIEW schema=ziranma-private-review-v1 contains_text=true writes=false learns=false network=false"
        );
    }
    let _ = writeln!(
        rendered,
        "会话 {session_id} · {from}–{through} / {total_events}"
    );
    if let Some((segments, session_kind, producer_version)) = details {
        let _ = writeln!(
            rendered,
            "{segments} 个加密分段 · {session_kind} · 记录器 {producer_version}"
        );
    }
    rendered.push('\n');
    rendered
}

fn format_compact_event(number: usize, event: &TimedTrackerOutput, full_text: bool) -> String {
    match &event.output {
        TrackerOutput::Commit(commit) => format!(
            "{number:>4}  输入　{}{} → {} → {}\n",
            format_keys(&commit.keys),
            completeness_note(commit.keys_complete),
            format_private_text(&commit.composition, full_text),
            format_compact_delta(&commit.document_change, full_text)
        ),
        TrackerOutput::Revision(revision) => {
            let keys = if revision.keys.is_empty() {
                if revision.keys_complete {
                    String::new()
                } else {
                    "（待确认） → ".to_owned()
                }
            } else {
                format!(
                    "{}{} → ",
                    format_keys(&revision.keys),
                    completeness_note(revision.keys_complete)
                )
            };
            format!(
                "{number:>4}  {}　{keys}{}\n",
                revision_label(revision),
                format_compact_delta(&revision.change, full_text)
            )
        }
    }
}

fn format_detailed_event(
    number: usize,
    segment_sequence: u64,
    event: &TimedTrackerOutput,
    full_text: bool,
) -> String {
    match &event.output {
        TrackerOutput::Commit(commit) => format!(
            "[{number}] 输入\n\
             \x20 按键：{}{}\n\
             \x20 组合：{}\n\
             \x20 预编辑：{}\n\
             \x20 文档：{}\n\
             \x20 位置：预编辑 @{}（{}）；文档 @{}（{}）\n\
             \x20 记录：分段 {segment_sequence}，分段计时 +{} ms\n\n",
            format_keys(&commit.keys),
            completeness_note(commit.keys_complete),
            format_private_text(&commit.composition, full_text),
            format_delta_action(&commit.change, full_text),
            format_delta_action(&commit.document_change, full_text),
            commit.change.start,
            format_position(commit.change.position_evidence),
            commit.document_change.start,
            format_position(commit.document_change.position_evidence),
            event.elapsed_ms
        ),
        TrackerOutput::Revision(revision) => format!(
            "[{number}] {}\n\
             \x20 按键：{}{}\n\
             \x20 文档：{}\n\
             \x20 位置：@{}（{}）\n\
             \x20 记录：分段 {segment_sequence}，分段计时 +{} ms\n\n",
            revision_label(revision),
            format_keys(&revision.keys),
            completeness_note(revision.keys_complete),
            format_delta_action(&revision.change, full_text),
            revision.change.start,
            format_position(revision.change.position_evidence),
            event.elapsed_ms
        ),
    }
}

fn completeness_note(complete: bool) -> &'static str {
    if complete { "" } else { "（待确认）" }
}

fn revision_label(revision: &RevisionRecord) -> &'static str {
    let change = &revision.change;
    match (change.deleted.is_empty(), change.inserted.is_empty()) {
        (true, false) => "写入",
        (false, true)
            if revision
                .keys
                .iter()
                .any(|key| matches!(key, RawKey::Escape)) =>
        {
            "取消"
        }
        (false, true) if revision.keys.is_empty() && change.start == 0 => "清空",
        (false, true) if !revision.keys.is_empty() => "回删",
        (false, true) => "删除",
        (false, false) => "替换",
        (true, true) => "变化",
    }
}

fn format_keys(keys: &[RawKey]) -> String {
    if keys.is_empty() {
        return "（未记录）".to_owned();
    }
    keys.iter().map(format_key).collect::<Vec<_>>().join(" ")
}

fn format_key(key: &RawKey) -> String {
    match key {
        RawKey::Letter(value) => value.to_string(),
        RawKey::Digit(value) => value.to_string(),
        RawKey::Backspace => "退格".to_owned(),
        RawKey::Delete => "Delete".to_owned(),
        RawKey::Space => "空格".to_owned(),
        RawKey::Escape => "Esc".to_owned(),
        RawKey::Left => "左".to_owned(),
        RawKey::Right => "右".to_owned(),
        RawKey::Up => "上".to_owned(),
        RawKey::Down => "下".to_owned(),
        RawKey::Home => "Home".to_owned(),
        RawKey::End => "End".to_owned(),
        RawKey::Shift(inner) => format!("Shift+{}", format_key(inner)),
    }
}

fn format_compact_delta(delta: &TextDelta, full_text: bool) -> String {
    match (delta.deleted.is_empty(), delta.inserted.is_empty()) {
        (true, true) => "没有文字变化".to_owned(),
        (true, false) => format_private_text(&delta.inserted, full_text),
        (false, true) => format!("删除 {}", format_private_text(&delta.deleted, full_text)),
        (false, false) => format!(
            "{} → {}",
            format_private_text(&delta.deleted, full_text),
            format_private_text(&delta.inserted, full_text)
        ),
    }
}

fn format_delta_action(delta: &TextDelta, full_text: bool) -> String {
    match (delta.deleted.is_empty(), delta.inserted.is_empty()) {
        (true, true) => "没有文字变化".to_owned(),
        (true, false) => format!("写入 {}", format_private_text(&delta.inserted, full_text)),
        (false, true) => format!("删除 {}", format_private_text(&delta.deleted, full_text)),
        (false, false) => format!(
            "{} → {}",
            format_private_text(&delta.deleted, full_text),
            format_private_text(&delta.inserted, full_text)
        ),
    }
}

fn format_position(position: DeltaPositionEvidence) -> &'static str {
    match position {
        DeltaPositionEvidence::UniqueText => "文字匹配",
        DeltaPositionEvidence::Caret => "光标",
        DeltaPositionEvidence::Ambiguous => "无法确定",
    }
}

fn format_private_text(value: &str, full_text: bool) -> String {
    if value.is_empty() {
        return "（空）".to_owned();
    }
    let total = value.chars().count();
    let visible = if full_text {
        total
    } else {
        total.min(DEFAULT_TEXT_CHARACTERS)
    };
    let mut rendered = String::new();
    rendered.push('“');
    for character in value.chars().take(visible) {
        match character {
            '\\' => rendered.push_str("\\\\"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(rendered, "\\u{{{:x}}}", character as u32);
            }
            character => rendered.push(character),
        }
    }
    rendered.push('”');
    if visible < total {
        let _ = write!(rendered, "…（共 {total} 字）");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LIMIT, Options, format_compact_delta, format_compact_event, format_detailed_event,
        format_keys, format_private_text, format_review_header, parse_options,
    };
    use ziranma_decoder::{
        CommitRecord, DeltaPositionEvidence, RawKey, RevisionRecord, TextDelta, TimedTrackerOutput,
        TrackerOutput,
    };

    #[test]
    fn parses_bounded_explicit_review() {
        assert_eq!(
            parse_options([
                "review".to_owned(),
                "--session".to_owned(),
                "1000-1".to_owned(),
            ])
            .unwrap(),
            Options::Review {
                session_id: "1000-1".to_owned(),
                from: 1,
                limit: DEFAULT_LIMIT,
                full_text: false,
                details: false,
            }
        );
        assert!(
            parse_options([
                "review".to_owned(),
                "--session".to_owned(),
                "../private".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_options([
                "review".to_owned(),
                "--session".to_owned(),
                "1000-1".to_owned(),
                "--limit".to_owned(),
                "501".to_owned(),
            ])
            .is_err()
        );
        assert_eq!(
            parse_options([
                "review".to_owned(),
                "--details".to_owned(),
                "--session".to_owned(),
                "1000-1".to_owned(),
                "--full-text".to_owned(),
            ])
            .unwrap(),
            Options::Review {
                session_id: "1000-1".to_owned(),
                from: 1,
                limit: DEFAULT_LIMIT,
                full_text: true,
                details: true,
            }
        );
    }

    #[test]
    fn renders_private_text_without_hiding_chinese() {
        assert_eq!(format_private_text("猫\n猫", false), "“猫\\n猫”");
        let long = "猫".repeat(161);
        let shortened = format_private_text(&long, false);
        assert!(shortened.ends_with("…（共 161 字）"));
        assert_eq!(format_private_text(&long, true), format!("“{long}”"));
    }

    #[test]
    fn renders_keys_and_compact_document_edits() {
        assert_eq!(
            format_keys(&[
                RawKey::Letter('m'),
                RawKey::Shift(Box::new(RawKey::Right)),
                RawKey::Backspace,
            ]),
            "m Shift+右 退格"
        );
        assert_eq!(
            format_compact_delta(
                &TextDelta {
                    start: 1,
                    deleted: "错".to_owned(),
                    inserted: "在".to_owned(),
                    position_evidence: DeltaPositionEvidence::Caret,
                },
                false,
            ),
            "“错” → “在”"
        );
    }

    #[test]
    fn compact_timeline_keeps_internal_location_evidence_out_of_the_main_line() {
        let commit = TimedTrackerOutput {
            elapsed_ms: 25_356,
            output: TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('n'),
                    RawKey::Letter('i'),
                    RawKey::Letter('h'),
                    RawKey::Letter('k'),
                    RawKey::Space,
                ],
                keys_complete: false,
                composition: "ni'hao".to_owned(),
                change: TextDelta {
                    start: 0,
                    deleted: "ni'hao".to_owned(),
                    inserted: "你好".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 0,
                    deleted: String::new(),
                    inserted: "你好".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }),
        };
        let punctuation = TimedTrackerOutput {
            elapsed_ms: 25_505,
            output: TrackerOutput::Revision(RevisionRecord {
                keys: Vec::new(),
                keys_complete: true,
                change: TextDelta {
                    start: 2,
                    deleted: String::new(),
                    inserted: "，".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }),
        };
        let backspace = TimedTrackerOutput {
            elapsed_ms: 27_399,
            output: TrackerOutput::Revision(RevisionRecord {
                keys: vec![
                    RawKey::Letter('k'),
                    RawKey::Letter('m'),
                    RawKey::Backspace,
                    RawKey::Backspace,
                ],
                keys_complete: true,
                change: TextDelta {
                    start: 5,
                    deleted: "k".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }),
        };

        let rendered = format!(
            "{}{}{}",
            format_compact_event(1, &commit, false),
            format_compact_event(2, &punctuation, false),
            format_compact_event(4, &backspace, false)
        );
        assert_eq!(
            rendered,
            concat!(
                "   1  输入　n i h k 空格（待确认） → “ni'hao” → “你好”\n",
                "   2  写入　“，”\n",
                "   4  回删　k m 退格 退格 → 删除 “k”\n",
            )
        );
        assert!(!rendered.contains("唯一文字"));
        assert!(!rendered.contains("分段"));

        let detailed = format_detailed_event(1, 0, &commit, false);
        assert!(detailed.contains("位置：预编辑 @0（文字匹配）；文档 @0（文字匹配）"));
        assert!(detailed.contains("记录：分段 0，分段计时 +25356 ms"));
    }

    #[test]
    fn review_header_keeps_default_output_compact_and_details_explicit() {
        assert_eq!(
            format_review_header("1000-1", 1, 10, 1_352, None),
            "会话 1000-1 · 1–10 / 1352\n\n"
        );
        assert_eq!(
            format_review_header(
                "1000-1",
                1,
                10,
                1_352,
                Some((47, "daily", "0.1.0+continuous.7"))
            ),
            concat!(
                "PRIVATE_REVIEW schema=ziranma-private-review-v1 contains_text=true writes=false learns=false network=false\n",
                "会话 1000-1 · 1–10 / 1352\n",
                "47 个加密分段 · daily · 记录器 0.1.0+continuous.7\n\n"
            )
        );
    }
}
