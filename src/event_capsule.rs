//! Strict, local-only event capsules for explicitly approved private replay.
//!
//! Capsules contain real committed text and key histories. The hexadecimal
//! string representation is only an unambiguous transport encoding; it is not
//! encryption. This module performs no I/O and never chooses a file path.

use std::error::Error;
use std::fmt;

use crate::{
    CommitRecord, DeltaPositionEvidence, RawKey, RevisionRecord, TextDelta, TrackerOutput,
};

pub const EVENT_CAPSULE_SCHEMA_V1: &str = "ziranma-event-capsule-v1";
pub const MAX_EVENT_CAPSULE_EVENTS: usize = 4_096;
pub const MAX_EVENT_CAPSULE_KEYS_PER_EVENT: usize = 128;
pub const MAX_EVENT_CAPSULE_TEXT_BYTES_PER_FIELD: usize = 64 * 1024;
pub const MAX_EVENT_CAPSULE_TOTAL_TEXT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedTrackerOutput {
    pub elapsed_ms: u64,
    pub output: TrackerOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventCapsuleV1 {
    events: Vec<TimedTrackerOutput>,
}

impl EventCapsuleV1 {
    pub fn new(events: Vec<TimedTrackerOutput>) -> Result<Self, EventCapsuleError> {
        validate_events(&events)?;
        Ok(Self { events })
    }

    pub fn events(&self) -> &[TimedTrackerOutput] {
        &self.events
    }

    pub fn into_events(self) -> Vec<TimedTrackerOutput> {
        self.events
    }

    pub fn to_text(&self) -> Result<String, EventCapsuleError> {
        validate_events(&self.events)?;
        let mut output = format!(
            "{EVENT_CAPSULE_SCHEMA_V1}\n\
             contains_private_text=true\n\
             encryption=none\n\
             string_encoding=utf8-hex\n\
             events={}\n",
            self.events.len()
        );
        for event in &self.events {
            output.push_str(&encode_event(event));
            output.push('\n');
        }
        output.push_str("end");
        Ok(output)
    }

    pub fn from_text(input: &str) -> Result<Self, EventCapsuleError> {
        let input = input
            .strip_suffix("\r\n")
            .or_else(|| input.strip_suffix('\n'))
            .unwrap_or(input);
        let mut lines = input.split('\n');
        expect_line(&mut lines, 1, EVENT_CAPSULE_SCHEMA_V1)?;
        expect_line(&mut lines, 2, "contains_private_text=true")?;
        expect_line(&mut lines, 3, "encryption=none")?;
        expect_line(&mut lines, 4, "string_encoding=utf8-hex")?;
        let count_line = lines.next().ok_or(EventCapsuleError::InvalidSyntax {
            line: 5,
            field: "event count",
        })?;
        let count = count_line
            .strip_prefix("events=")
            .ok_or(EventCapsuleError::InvalidSyntax {
                line: 5,
                field: "event count",
            })
            .and_then(|value| parse_usize(value, 5, "event count"))?;
        if count == 0 {
            return Err(EventCapsuleError::Empty);
        }
        if count > MAX_EVENT_CAPSULE_EVENTS {
            return Err(EventCapsuleError::LimitExceeded("event count"));
        }

        let mut events = Vec::with_capacity(count);
        for index in 0..count {
            let line_number = index + 6;
            let line = lines.next().ok_or(EventCapsuleError::InvalidSyntax {
                line: line_number,
                field: "event",
            })?;
            events.push(parse_event(line, line_number)?);
        }
        let end_line = count + 6;
        expect_line(&mut lines, end_line, "end")?;
        if lines.next().is_some() {
            return Err(EventCapsuleError::InvalidSyntax {
                line: end_line + 1,
                field: "end of file",
            });
        }
        Self::new(events)
    }
}

#[derive(Debug, Default)]
pub struct EventCapsuleRecorder {
    events: Vec<TimedTrackerOutput>,
    total_text_bytes: usize,
    failure: Option<EventCapsuleError>,
}

impl EventCapsuleRecorder {
    pub fn reset(&mut self) {
        self.events.clear();
        self.total_text_bytes = 0;
        self.failure = None;
    }

    pub fn observe(
        &mut self,
        elapsed_ms: u64,
        output: TrackerOutput,
    ) -> Result<(), EventCapsuleError> {
        if let Some(error) = self.failure.clone() {
            return Err(error);
        }
        let event = TimedTrackerOutput { elapsed_ms, output };
        let event_index = self.events.len();
        let result = validate_event(
            &event,
            event_index,
            self.events.last().map(|previous| previous.elapsed_ms),
        )
        .and_then(|event_bytes| {
            if self.events.len() == MAX_EVENT_CAPSULE_EVENTS {
                return Err(EventCapsuleError::LimitExceeded("event count"));
            }
            let total = self
                .total_text_bytes
                .checked_add(event_bytes)
                .ok_or(EventCapsuleError::LimitExceeded("total private text bytes"))?;
            if total > MAX_EVENT_CAPSULE_TOTAL_TEXT_BYTES {
                return Err(EventCapsuleError::LimitExceeded("total private text bytes"));
            }
            self.total_text_bytes = total;
            Ok(())
        });
        if let Err(error) = result {
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.events.push(event);
        Ok(())
    }

    pub fn finish(&self) -> Result<EventCapsuleV1, EventCapsuleError> {
        if let Some(error) = self.failure.clone() {
            return Err(error);
        }
        EventCapsuleV1::new(self.events.clone())
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventCapsuleError {
    Empty,
    LimitExceeded(&'static str),
    InvalidInvariant { event: usize, field: &'static str },
    InvalidSyntax { line: usize, field: &'static str },
    InvalidNumber { line: usize, field: &'static str },
    InvalidUtf8 { line: usize, field: &'static str },
}

impl fmt::Display for EventCapsuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "private event capsule contains no events"),
            Self::LimitExceeded(field) => {
                write!(
                    formatter,
                    "private event capsule exceeded its {field} limit"
                )
            }
            Self::InvalidInvariant { event, field } => {
                write!(
                    formatter,
                    "invalid private event capsule invariant at event {event}: {field}"
                )
            }
            Self::InvalidSyntax { line, field } => {
                write!(
                    formatter,
                    "invalid private event capsule syntax at line {line}: {field}"
                )
            }
            Self::InvalidNumber { line, field } => {
                write!(
                    formatter,
                    "invalid private event capsule number at line {line}: {field}"
                )
            }
            Self::InvalidUtf8 { line, field } => {
                write!(
                    formatter,
                    "invalid private event capsule UTF-8 at line {line}: {field}"
                )
            }
        }
    }
}

impl Error for EventCapsuleError {}

fn validate_events(events: &[TimedTrackerOutput]) -> Result<(), EventCapsuleError> {
    if events.is_empty() {
        return Err(EventCapsuleError::Empty);
    }
    if events.len() > MAX_EVENT_CAPSULE_EVENTS {
        return Err(EventCapsuleError::LimitExceeded("event count"));
    }
    let mut total_text_bytes = 0_usize;
    let mut previous_elapsed_ms = None;
    for (index, event) in events.iter().enumerate() {
        let event_bytes = validate_event(event, index, previous_elapsed_ms)?;
        total_text_bytes = total_text_bytes
            .checked_add(event_bytes)
            .ok_or(EventCapsuleError::LimitExceeded("total private text bytes"))?;
        if total_text_bytes > MAX_EVENT_CAPSULE_TOTAL_TEXT_BYTES {
            return Err(EventCapsuleError::LimitExceeded("total private text bytes"));
        }
        previous_elapsed_ms = Some(event.elapsed_ms);
    }
    Ok(())
}

fn validate_event(
    event: &TimedTrackerOutput,
    index: usize,
    previous_elapsed_ms: Option<u64>,
) -> Result<usize, EventCapsuleError> {
    if previous_elapsed_ms.is_some_and(|previous| event.elapsed_ms < previous) {
        return Err(EventCapsuleError::InvalidInvariant {
            event: index,
            field: "elapsed time moved backward",
        });
    }
    let (keys, fields): (&[RawKey], &[&str]) = match &event.output {
        TrackerOutput::Commit(record) => (
            &record.keys,
            &[
                &record.composition,
                &record.change.deleted,
                &record.change.inserted,
                &record.document_change.deleted,
                &record.document_change.inserted,
            ],
        ),
        TrackerOutput::Revision(record) => (
            &record.keys,
            &[&record.change.deleted, &record.change.inserted],
        ),
    };
    if keys.len() > MAX_EVENT_CAPSULE_KEYS_PER_EVENT {
        return Err(EventCapsuleError::LimitExceeded("keys per event"));
    }
    for key in keys {
        validate_key(key, index, 0)?;
    }
    let mut total = 0_usize;
    for field in fields {
        if field.len() > MAX_EVENT_CAPSULE_TEXT_BYTES_PER_FIELD {
            return Err(EventCapsuleError::LimitExceeded(
                "private text bytes per field",
            ));
        }
        total = total
            .checked_add(field.len())
            .ok_or(EventCapsuleError::LimitExceeded(
                "private text bytes per event",
            ))?;
    }
    Ok(total)
}

fn validate_key(key: &RawKey, event: usize, shift_depth: usize) -> Result<(), EventCapsuleError> {
    match key {
        RawKey::Letter(letter) if letter.is_ascii_lowercase() => Ok(()),
        RawKey::Digit(digit) if *digit <= 9 => Ok(()),
        RawKey::Shift(inner) if shift_depth < 4 => validate_key(inner, event, shift_depth + 1),
        RawKey::Backspace
        | RawKey::Delete
        | RawKey::Space
        | RawKey::Escape
        | RawKey::Left
        | RawKey::Right
        | RawKey::Up
        | RawKey::Down
        | RawKey::Home
        | RawKey::End => Ok(()),
        RawKey::Letter(_) | RawKey::Digit(_) | RawKey::Shift(_) => {
            Err(EventCapsuleError::InvalidInvariant {
                event,
                field: "unsupported key value",
            })
        }
    }
}

fn encode_event(event: &TimedTrackerOutput) -> String {
    match &event.output {
        TrackerOutput::Commit(record) => format!(
            "C|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            event.elapsed_ms,
            record.keys_complete,
            encode_keys(&record.keys),
            encode_text(&record.composition),
            encode_position(record.change.position_evidence),
            record.change.start,
            encode_text(&record.change.deleted),
            encode_text(&record.change.inserted),
            encode_position(record.document_change.position_evidence),
            record.document_change.start,
            encode_text(&record.document_change.deleted),
            encode_text(&record.document_change.inserted)
        ),
        TrackerOutput::Revision(record) => format!(
            "R|{}|{}|{}|{}|{}|{}|{}",
            event.elapsed_ms,
            record.keys_complete,
            encode_keys(&record.keys),
            encode_position(record.change.position_evidence),
            record.change.start,
            encode_text(&record.change.deleted),
            encode_text(&record.change.inserted)
        ),
    }
}

fn parse_event(line: &str, line_number: usize) -> Result<TimedTrackerOutput, EventCapsuleError> {
    let fields = line.split('|').take(14).collect::<Vec<_>>();
    match fields.first().copied() {
        Some("C") if fields.len() == 13 => {
            let elapsed_ms = parse_u64(fields[1], line_number, "elapsed milliseconds")?;
            let keys_complete = parse_bool(fields[2], line_number, "keys complete")?;
            let keys = decode_keys(fields[3], line_number)?;
            let composition = decode_text(fields[4], line_number, "composition")?;
            let change = TextDelta {
                position_evidence: decode_position(fields[5], line_number, "preedit position")?,
                start: parse_usize(fields[6], line_number, "preedit start")?,
                deleted: decode_text(fields[7], line_number, "preedit deleted text")?,
                inserted: decode_text(fields[8], line_number, "preedit inserted text")?,
            };
            let document_change = TextDelta {
                position_evidence: decode_position(fields[9], line_number, "document position")?,
                start: parse_usize(fields[10], line_number, "document start")?,
                deleted: decode_text(fields[11], line_number, "document deleted text")?,
                inserted: decode_text(fields[12], line_number, "document inserted text")?,
            };
            Ok(TimedTrackerOutput {
                elapsed_ms,
                output: TrackerOutput::Commit(CommitRecord {
                    keys,
                    keys_complete,
                    composition,
                    change,
                    document_change,
                }),
            })
        }
        Some("R") if fields.len() == 8 => {
            let elapsed_ms = parse_u64(fields[1], line_number, "elapsed milliseconds")?;
            let keys_complete = parse_bool(fields[2], line_number, "keys complete")?;
            let keys = decode_keys(fields[3], line_number)?;
            let change = TextDelta {
                position_evidence: decode_position(fields[4], line_number, "revision position")?,
                start: parse_usize(fields[5], line_number, "revision start")?,
                deleted: decode_text(fields[6], line_number, "revision deleted text")?,
                inserted: decode_text(fields[7], line_number, "revision inserted text")?,
            };
            Ok(TimedTrackerOutput {
                elapsed_ms,
                output: TrackerOutput::Revision(RevisionRecord {
                    keys,
                    keys_complete,
                    change,
                }),
            })
        }
        Some("C") => Err(EventCapsuleError::InvalidSyntax {
            line: line_number,
            field: "commit field count",
        }),
        Some("R") => Err(EventCapsuleError::InvalidSyntax {
            line: line_number,
            field: "revision field count",
        }),
        _ => Err(EventCapsuleError::InvalidSyntax {
            line: line_number,
            field: "event kind",
        }),
    }
}

fn encode_keys(keys: &[RawKey]) -> String {
    if keys.is_empty() {
        return "-".to_owned();
    }
    keys.iter().map(encode_key).collect::<Vec<_>>().join(",")
}

fn encode_key(key: &RawKey) -> String {
    match key {
        RawKey::Letter(letter) => format!("L{:02x}", u32::from(*letter)),
        RawKey::Digit(digit) => format!("N{digit}"),
        RawKey::Backspace => "Backspace".to_owned(),
        RawKey::Delete => "Delete".to_owned(),
        RawKey::Space => "Space".to_owned(),
        RawKey::Escape => "Escape".to_owned(),
        RawKey::Left => "Left".to_owned(),
        RawKey::Right => "Right".to_owned(),
        RawKey::Up => "Up".to_owned(),
        RawKey::Down => "Down".to_owned(),
        RawKey::Home => "Home".to_owned(),
        RawKey::End => "End".to_owned(),
        RawKey::Shift(inner) => format!("Shift({})", encode_key(inner)),
    }
}

fn decode_keys(value: &str, line: usize) -> Result<Vec<RawKey>, EventCapsuleError> {
    if value == "-" {
        return Ok(Vec::new());
    }
    let tokens = value.split(',').collect::<Vec<_>>();
    if tokens.len() > MAX_EVENT_CAPSULE_KEYS_PER_EVENT {
        return Err(EventCapsuleError::LimitExceeded("keys per event"));
    }
    tokens
        .into_iter()
        .map(|token| decode_key(token, line, 0))
        .collect()
}

fn decode_key(token: &str, line: usize, shift_depth: usize) -> Result<RawKey, EventCapsuleError> {
    let key = match token {
        "Backspace" => RawKey::Backspace,
        "Delete" => RawKey::Delete,
        "Space" => RawKey::Space,
        "Escape" => RawKey::Escape,
        "Left" => RawKey::Left,
        "Right" => RawKey::Right,
        "Up" => RawKey::Up,
        "Down" => RawKey::Down,
        "Home" => RawKey::Home,
        "End" => RawKey::End,
        _ if token.len() == 3 && token.starts_with('L') => {
            let byte = decode_hex_byte(&token.as_bytes()[1..], line, "letter key")?;
            let letter = char::from(byte);
            if !letter.is_ascii_lowercase() {
                return Err(EventCapsuleError::InvalidSyntax {
                    line,
                    field: "letter key",
                });
            }
            RawKey::Letter(letter)
        }
        _ if token.len() == 2 && token.starts_with('N') => {
            let digit = token.as_bytes()[1];
            if !digit.is_ascii_digit() {
                return Err(EventCapsuleError::InvalidSyntax {
                    line,
                    field: "digit key",
                });
            }
            RawKey::Digit(digit - b'0')
        }
        _ if shift_depth < 4 && token.starts_with("Shift(") && token.ends_with(')') => {
            let inner = &token[6..token.len() - 1];
            RawKey::Shift(Box::new(decode_key(inner, line, shift_depth + 1)?))
        }
        _ => {
            return Err(EventCapsuleError::InvalidSyntax { line, field: "key" });
        }
    };
    Ok(key)
}

fn encode_text(value: &str) -> String {
    if value.is_empty() {
        return "-".to_owned();
    }
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

fn decode_text(value: &str, line: usize, field: &'static str) -> Result<String, EventCapsuleError> {
    if value == "-" {
        return Ok(String::new());
    }
    if !value.len().is_multiple_of(2)
        || value
            .as_bytes()
            .iter()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte))
    {
        return Err(EventCapsuleError::InvalidSyntax { line, field });
    }
    let byte_count = value.len() / 2;
    if byte_count > MAX_EVENT_CAPSULE_TEXT_BYTES_PER_FIELD {
        return Err(EventCapsuleError::LimitExceeded(
            "private text bytes per field",
        ));
    }
    let mut bytes = Vec::with_capacity(byte_count);
    for pair in value.as_bytes().chunks_exact(2) {
        bytes.push(decode_hex_byte(pair, line, field)?);
    }
    String::from_utf8(bytes).map_err(|_| EventCapsuleError::InvalidUtf8 { line, field })
}

fn decode_hex_byte(pair: &[u8], line: usize, field: &'static str) -> Result<u8, EventCapsuleError> {
    if pair.len() != 2 {
        return Err(EventCapsuleError::InvalidSyntax { line, field });
    }
    let high =
        decode_hex_nibble(pair[0]).ok_or(EventCapsuleError::InvalidSyntax { line, field })?;
    let low = decode_hex_nibble(pair[1]).ok_or(EventCapsuleError::InvalidSyntax { line, field })?;
    Ok(high * 16 + low)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn encode_position(position: DeltaPositionEvidence) -> &'static str {
    match position {
        DeltaPositionEvidence::UniqueText => "UniqueText",
        DeltaPositionEvidence::Caret => "Caret",
        DeltaPositionEvidence::Ambiguous => "Ambiguous",
    }
}

fn decode_position(
    value: &str,
    line: usize,
    field: &'static str,
) -> Result<DeltaPositionEvidence, EventCapsuleError> {
    match value {
        "UniqueText" => Ok(DeltaPositionEvidence::UniqueText),
        "Caret" => Ok(DeltaPositionEvidence::Caret),
        "Ambiguous" => Ok(DeltaPositionEvidence::Ambiguous),
        _ => Err(EventCapsuleError::InvalidSyntax { line, field }),
    }
}

fn parse_bool(value: &str, line: usize, field: &'static str) -> Result<bool, EventCapsuleError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(EventCapsuleError::InvalidSyntax { line, field }),
    }
}

fn parse_u64(value: &str, line: usize, field: &'static str) -> Result<u64, EventCapsuleError> {
    if value.is_empty()
        || !value.as_bytes().iter().all(u8::is_ascii_digit)
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(EventCapsuleError::InvalidNumber { line, field });
    }
    value
        .parse()
        .map_err(|_| EventCapsuleError::InvalidNumber { line, field })
}

fn parse_usize(value: &str, line: usize, field: &'static str) -> Result<usize, EventCapsuleError> {
    let value = parse_u64(value, line, field)?;
    usize::try_from(value).map_err(|_| EventCapsuleError::InvalidNumber { line, field })
}

fn expect_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    line: usize,
    expected: &'static str,
) -> Result<(), EventCapsuleError> {
    if lines.next() == Some(expected) {
        Ok(())
    } else {
        Err(EventCapsuleError::InvalidSyntax {
            line,
            field: expected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_CAPSULE_SCHEMA_V1, EventCapsuleError, EventCapsuleRecorder, EventCapsuleV1,
        TimedTrackerOutput,
    };
    use crate::{
        CommitRecord, DeltaPositionEvidence, RawKey, RevisionRecord, TextDelta, TrackerOutput,
    };

    fn delta(
        start: usize,
        deleted: &str,
        inserted: &str,
        position_evidence: DeltaPositionEvidence,
    ) -> TextDelta {
        TextDelta {
            start,
            deleted: deleted.to_owned(),
            inserted: inserted.to_owned(),
            position_evidence,
        }
    }

    fn synthetic_events() -> Vec<TimedTrackerOutput> {
        vec![
            TimedTrackerOutput {
                elapsed_ms: 100,
                output: TrackerOutput::Commit(CommitRecord {
                    keys: vec![
                        RawKey::Letter('m'),
                        RawKey::Letter('k'),
                        RawKey::Letter('m'),
                        RawKey::Letter('j'),
                        RawKey::Backspace,
                        RawKey::Letter('k'),
                        RawKey::Space,
                    ],
                    keys_complete: true,
                    composition: "mao'mao".to_owned(),
                    change: delta(0, "mao'mao", "猫猫", DeltaPositionEvidence::UniqueText),
                    document_change: delta(0, "", "猫猫", DeltaPositionEvidence::UniqueText),
                }),
            },
            TimedTrackerOutput {
                elapsed_ms: 200,
                output: TrackerOutput::Revision(RevisionRecord {
                    keys: vec![
                        RawKey::Home,
                        RawKey::Shift(Box::new(RawKey::Right)),
                        RawKey::Delete,
                    ],
                    keys_complete: false,
                    change: delta(0, "猫", "", DeltaPositionEvidence::Caret),
                }),
            },
        ]
    }

    #[test]
    fn private_capsule_round_trips_arbitrary_utf8_without_claiming_encryption() {
        let capsule = EventCapsuleV1::new(synthetic_events()).unwrap();
        let encoded = capsule.to_text().unwrap();
        assert!(encoded.starts_with(EVENT_CAPSULE_SCHEMA_V1));
        assert!(encoded.contains("contains_private_text=true"));
        assert!(encoded.contains("encryption=none"));
        assert!(!encoded.contains("猫猫"));
        assert_eq!(EventCapsuleV1::from_text(&encoded), Ok(capsule.clone()));
        assert_eq!(
            EventCapsuleV1::from_text(&format!("{encoded}\n")),
            Ok(capsule)
        );
    }

    #[test]
    fn recorder_resets_and_refuses_empty_or_backward_time() {
        let mut recorder = EventCapsuleRecorder::default();
        assert_eq!(recorder.finish(), Err(EventCapsuleError::Empty));
        for event in synthetic_events() {
            recorder.observe(event.elapsed_ms, event.output).unwrap();
        }
        assert_eq!(recorder.len(), 2);
        recorder.reset();
        assert!(recorder.is_empty());

        let mut events = synthetic_events();
        let first = events.remove(0);
        let second = events.remove(0);
        recorder.observe(200, first.output).unwrap();
        assert!(recorder.observe(199, second.output).is_err());
        assert!(recorder.finish().is_err());
    }

    #[test]
    fn strict_parser_rejects_schema_drift_reformatting_and_private_echo() {
        let encoded = EventCapsuleV1::new(synthetic_events())
            .unwrap()
            .to_text()
            .unwrap();
        for invalid in [
            format!(" {encoded}"),
            encoded.replace("encryption=none", "encryption=secret"),
            encoded.replace("events=2", "events=02"),
            encoded.replace("L6d", "L6D"),
            format!("{encoded}\nextra"),
        ] {
            let error = EventCapsuleV1::from_text(&invalid).unwrap_err();
            assert!(!error.to_string().contains("猫"));
            assert!(!error.to_string().contains("mao"));
        }
    }

    #[test]
    fn parser_rejects_invalid_utf8_without_echoing_payload() {
        let encoded = EventCapsuleV1::new(synthetic_events())
            .unwrap()
            .to_text()
            .unwrap();
        let invalid = encoded.replacen("6d616f276d616f", "ff", 1);
        assert!(matches!(
            EventCapsuleV1::from_text(&invalid),
            Err(EventCapsuleError::InvalidUtf8 { .. })
        ));
    }
}
