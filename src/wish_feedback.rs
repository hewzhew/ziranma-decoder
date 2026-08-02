//! Explicit, local-only wish snapshots protected for the current Windows user.
//!
//! The module accepts only an explicitly frozen native-feedback snapshot. It
//! neither discovers capture data nor starts feedback, and it performs no
//! networking. Private event types intentionally omit `Debug`.

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::{
    DataProtector, FrozenNativeFeedbackSnapshot, NativeCancellationSource, NativeCandidateView,
    NativeFeedbackEvent, NativeSelectionSource, candidate_sha256_hex,
};

pub const WISH_SCHEMA_V1: &str = "ziranma-wish-v1";
pub const WISH_PACKAGE_FILE_SUFFIX: &str = ".ziw";
pub const WISH_NOTE_FILE_SUFFIX: &str = ".note.ziw";
pub const MAX_WISH_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_WISH_NOTE_BYTES: usize = 8 * 1024;

const MAX_WISH_EVENTS: usize = 4_096;
const MAX_WISH_PLAINTEXT_BYTES: usize = 1536 * 1024;
const MAX_WISH_STRING_BYTES: usize = 64 * 1024;
const WISH_PLAINTEXT_MAGIC: &[u8] = b"ziranma-wish-v1\0";
const WISH_PROTECTED_MAGIC: &[u8] = b"ziranma-wish-dpapi-v1\0";
const WISH_NOTE_PLAINTEXT_MAGIC: &[u8] = b"ziranma-wish-note-v1\0";
const WISH_NOTE_PROTECTED_MAGIC: &[u8] = b"ziranma-wish-note-dpapi-v1\0";
const WISH_ID_PREFIX: &str = "wish-";
const WISH_ID_HEX_BYTES: usize = 64;
const WISH_TRASH_DIRECTORY: &str = "trash";
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One private event loaded from or prepared for a wish package.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishEvent {
    milliseconds_before_marker: u32,
    event: NativeFeedbackEvent,
}

impl WishEvent {
    pub fn milliseconds_before_marker(&self) -> u32 {
        self.milliseconds_before_marker
    }

    pub fn event(&self) -> &NativeFeedbackEvent {
        &self.event
    }
}

/// Canonical private contents of one local wish.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishSnapshot {
    lookback_ms: u32,
    source_complete: bool,
    source_events: usize,
    omitted_before_window: usize,
    omitted_untimed: usize,
    omitted_by_event_limit: usize,
    events: Vec<WishEvent>,
}

impl WishSnapshot {
    pub fn from_frozen(snapshot: &FrozenNativeFeedbackSnapshot) -> Result<Self, WishFeedbackError> {
        let value = Self {
            lookback_ms: snapshot.lookback_ms(),
            source_complete: snapshot.source_complete(),
            source_events: snapshot.source_events(),
            omitted_before_window: snapshot.omitted_before_window(),
            omitted_untimed: snapshot.omitted_untimed(),
            omitted_by_event_limit: snapshot.omitted_by_event_limit(),
            events: snapshot
                .events()
                .iter()
                .map(|event| WishEvent {
                    milliseconds_before_marker: event.milliseconds_before_marker(),
                    event: event.event().clone(),
                })
                .collect(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn lookback_ms(&self) -> u32 {
        self.lookback_ms
    }

    pub fn source_complete(&self) -> bool {
        self.source_complete
    }

    pub fn source_events(&self) -> usize {
        self.source_events
    }

    pub fn omitted_before_window(&self) -> usize {
        self.omitted_before_window
    }

    pub fn omitted_untimed(&self) -> usize {
        self.omitted_untimed
    }

    pub fn omitted_by_event_limit(&self) -> usize {
        self.omitted_by_event_limit
    }

    pub fn events(&self) -> &[WishEvent] {
        &self.events
    }

    fn validate(&self) -> Result<(), WishFeedbackError> {
        if self.lookback_ms == 0 || self.events.len() > MAX_WISH_EVENTS {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let accounted = self
            .events
            .len()
            .checked_add(self.omitted_before_window)
            .and_then(|count| count.checked_add(self.omitted_untimed))
            .and_then(|count| count.checked_add(self.omitted_by_event_limit))
            .ok_or(WishFeedbackError::InvalidSnapshot)?;
        if accounted != self.source_events {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let mut previous_age = u32::MAX;
        for event in &self.events {
            if event.milliseconds_before_marker > self.lookback_ms
                || event.milliseconds_before_marker > previous_age
                || event.event.validate_and_measure().is_none()
            {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            previous_age = event.milliseconds_before_marker;
        }
        Ok(())
    }

    fn render(&self) -> Result<Vec<u8>, WishFeedbackError> {
        self.validate()?;
        let mut output = Vec::new();
        output.extend_from_slice(WISH_PLAINTEXT_MAGIC);
        put_u32(&mut output, self.lookback_ms);
        output.push(u8::from(self.source_complete));
        put_usize(&mut output, self.source_events)?;
        put_usize(&mut output, self.omitted_before_window)?;
        put_usize(&mut output, self.omitted_untimed)?;
        put_usize(&mut output, self.omitted_by_event_limit)?;
        put_usize(&mut output, self.events.len())?;
        for event in &self.events {
            put_u32(&mut output, event.milliseconds_before_marker);
            render_event(&mut output, &event.event)?;
        }
        if output.len() > MAX_WISH_PLAINTEXT_BYTES {
            return Err(WishFeedbackError::PlaintextTooLarge);
        }
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, WishFeedbackError> {
        if input.len() <= WISH_PLAINTEXT_MAGIC.len() || input.len() > MAX_WISH_PLAINTEXT_BYTES {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let mut reader = SliceReader::new(input);
        reader.expect(WISH_PLAINTEXT_MAGIC)?;
        let lookback_ms = reader.u32()?;
        let source_complete = reader.boolean()?;
        let source_events = reader.usize()?;
        let omitted_before_window = reader.usize()?;
        let omitted_untimed = reader.usize()?;
        let omitted_by_event_limit = reader.usize()?;
        let event_count = reader.usize()?;
        if event_count > MAX_WISH_EVENTS {
            return Err(WishFeedbackError::InvalidSnapshot);
        }
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            events.push(WishEvent {
                milliseconds_before_marker: reader.u32()?,
                event: parse_event(&mut reader)?,
            });
        }
        if !reader.is_empty() {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let snapshot = Self {
            lookback_ms,
            source_complete,
            source_events,
            omitted_before_window,
            omitted_untimed,
            omitted_by_event_limit,
            events,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishCategory {
    Candidates,
    Ranking,
    Display,
    Latency,
    InputMode,
    Compatibility,
    Other,
}

impl WishCategory {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Candidates => "candidates",
            Self::Ranking => "ranking",
            Self::Display => "display",
            Self::Latency => "latency",
            Self::InputMode => "input-mode",
            Self::Compatibility => "compatibility",
            Self::Other => "other",
        }
    }

    pub fn parse_slug(value: &str) -> Option<Self> {
        match value {
            "candidates" => Some(Self::Candidates),
            "ranking" => Some(Self::Ranking),
            "display" => Some(Self::Display),
            "latency" => Some(Self::Latency),
            "input-mode" => Some(Self::InputMode),
            "compatibility" => Some(Self::Compatibility),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// One explicitly supplied private note bound to an immutable wish ID.
///
/// This type deliberately does not implement `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct WishNote {
    wish_id: String,
    category: WishCategory,
    text: String,
}

impl WishNote {
    pub fn new(
        wish_id: &str,
        category: WishCategory,
        text: &str,
    ) -> Result<Self, WishFeedbackError> {
        validate_wish_id(wish_id)?;
        if text.trim().is_empty() || text.len() > MAX_WISH_NOTE_BYTES || text.contains('\0') {
            return Err(WishFeedbackError::InvalidNote);
        }
        Ok(Self {
            wish_id: wish_id.to_owned(),
            category,
            text: text.to_owned(),
        })
    }

    pub fn wish_id(&self) -> &str {
        &self.wish_id
    }

    pub fn category(&self) -> WishCategory {
        self.category
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn render(&self) -> Result<Vec<u8>, WishFeedbackError> {
        let checked = Self::new(&self.wish_id, self.category, &self.text)?;
        let mut output = Vec::new();
        output.extend_from_slice(WISH_NOTE_PLAINTEXT_MAGIC);
        put_string(&mut output, &checked.wish_id)?;
        put_string(&mut output, checked.category.slug())?;
        put_string(&mut output, &checked.text)?;
        Ok(output)
    }

    fn parse(input: &[u8]) -> Result<Self, WishFeedbackError> {
        if input.len() <= WISH_NOTE_PLAINTEXT_MAGIC.len()
            || input.len() > MAX_WISH_NOTE_BYTES.saturating_add(512)
        {
            return Err(WishFeedbackError::InvalidNote);
        }
        let mut reader = SliceReader::new(input);
        reader.expect(WISH_NOTE_PLAINTEXT_MAGIC)?;
        let wish_id = reader.string()?;
        let category =
            WishCategory::parse_slug(&reader.string()?).ok_or(WishFeedbackError::InvalidNote)?;
        let text = reader.string()?;
        if !reader.is_empty() {
            return Err(WishFeedbackError::InvalidNote);
        }
        Self::new(&wish_id, category, &text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishPackageInfo {
    id: String,
    protected_bytes: u64,
}

impl WishPackageInfo {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn protected_bytes(&self) -> u64 {
        self.protected_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WishSaveReceipt {
    id: String,
    events: usize,
    protected_bytes: usize,
}

impl WishSaveReceipt {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn events(&self) -> usize {
        self.events
    }

    pub fn protected_bytes(&self) -> usize {
        self.protected_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WishFeedbackError {
    InvalidSnapshot,
    PlaintextTooLarge,
    InvalidPlaintext,
    Protection,
    InvalidProtectedPackage,
    InvalidRoot,
    RootUnavailable,
    InvalidWishId,
    WishUnavailable,
    WishAlreadyExists,
    InvalidNote,
    NoteUnavailable,
    NoteAlreadyExists,
    InvalidTrash,
    Io,
}

impl fmt::Display for WishFeedbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSnapshot => "wish snapshot is invalid",
            Self::PlaintextTooLarge => "wish plaintext exceeds its limit",
            Self::InvalidPlaintext => "wish plaintext is malformed",
            Self::Protection => "current-user wish protection failed",
            Self::InvalidProtectedPackage => "protected wish package is invalid",
            Self::InvalidRoot => "wish root is not a regular directory",
            Self::RootUnavailable => "wish root is unavailable",
            Self::InvalidWishId => "wish id is invalid",
            Self::WishUnavailable => "wish package is unavailable",
            Self::WishAlreadyExists => "wish package already exists",
            Self::InvalidNote => "wish note is invalid",
            Self::NoteUnavailable => "wish note is unavailable",
            Self::NoteAlreadyExists => "wish note already exists",
            Self::InvalidTrash => "wish trash is not a regular directory",
            Self::Io => "wish storage operation failed",
        })
    }
}

impl Error for WishFeedbackError {}

pub fn save_wish_snapshot(
    root: &Path,
    snapshot: &WishSnapshot,
    protector: &dyn DataProtector,
) -> Result<WishSaveReceipt, WishFeedbackError> {
    let mut plaintext = snapshot.render()?;
    let protected = protect_payload(&plaintext, WISH_PROTECTED_MAGIC, protector);
    plaintext.fill(0);
    let protected = protected?;
    let id = format!("{WISH_ID_PREFIX}{}", candidate_sha256_hex(&protected));
    prepare_root(root)?;
    let destination = root.join(wish_filename(&id)?);
    publish_new(
        root,
        &destination,
        &protected,
        WishFeedbackError::WishAlreadyExists,
    )?;
    Ok(WishSaveReceipt {
        id,
        events: snapshot.events.len(),
        protected_bytes: protected.len(),
    })
}

pub fn load_wish_snapshot(
    root: &Path,
    wish_id: &str,
    protector: &dyn DataProtector,
) -> Result<WishSnapshot, WishFeedbackError> {
    ensure_root(root)?;
    let path = root.join(wish_filename(wish_id)?);
    let protected = read_regular_bytes(
        &path,
        MAX_WISH_PACKAGE_BYTES,
        WishFeedbackError::WishUnavailable,
    )?;
    if format!("{WISH_ID_PREFIX}{}", candidate_sha256_hex(&protected)) != wish_id {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let mut plaintext = unprotect_payload(&protected, WISH_PROTECTED_MAGIC, protector)?;
    let snapshot = WishSnapshot::parse(&plaintext);
    plaintext.fill(0);
    snapshot
}

pub fn list_wish_packages(root: &Path) -> Result<Vec<WishPackageInfo>, WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(WishFeedbackError::RootUnavailable),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(WishFeedbackError::InvalidRoot);
        }
        Ok(_) => {}
    }
    let mut packages = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| WishFeedbackError::RootUnavailable)? {
        let entry = entry.map_err(|_| WishFeedbackError::RootUnavailable)?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| WishFeedbackError::RootUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = name.strip_suffix(WISH_PACKAGE_FILE_SUFFIX) else {
            continue;
        };
        if validate_wish_id(id).is_err()
            || metadata.len() == 0
            || metadata.len() > MAX_WISH_PACKAGE_BYTES as u64
        {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        packages.push((
            modified,
            WishPackageInfo {
                id: id.to_owned(),
                protected_bytes: metadata.len(),
            },
        ));
    }
    packages.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.id.cmp(&left.1.id))
    });
    Ok(packages.into_iter().map(|(_, info)| info).collect())
}

pub fn save_wish_note(
    root: &Path,
    note: &WishNote,
    protector: &dyn DataProtector,
) -> Result<(), WishFeedbackError> {
    // Refuse detached notes: the exact encrypted wish must already exist.
    ensure_regular_file(
        &root.join(wish_filename(note.wish_id())?),
        WishFeedbackError::WishUnavailable,
    )?;
    let mut plaintext = note.render()?;
    let protected = protect_payload(&plaintext, WISH_NOTE_PROTECTED_MAGIC, protector);
    plaintext.fill(0);
    let protected = protected?;
    if protected.len() > MAX_WISH_NOTE_BYTES.saturating_add(1024) {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let destination = root.join(note_filename(note.wish_id())?);
    publish_new(
        root,
        &destination,
        &protected,
        WishFeedbackError::NoteAlreadyExists,
    )
}

pub fn load_wish_note(
    root: &Path,
    wish_id: &str,
    protector: &dyn DataProtector,
) -> Result<WishNote, WishFeedbackError> {
    ensure_root(root)?;
    let path = root.join(note_filename(wish_id)?);
    let protected = read_regular_bytes(
        &path,
        MAX_WISH_NOTE_BYTES.saturating_add(1024),
        WishFeedbackError::NoteUnavailable,
    )?;
    let mut plaintext = unprotect_payload(&protected, WISH_NOTE_PROTECTED_MAGIC, protector)?;
    let note = WishNote::parse(&plaintext);
    plaintext.fill(0);
    let note = note?;
    if note.wish_id() != wish_id {
        return Err(WishFeedbackError::InvalidNote);
    }
    Ok(note)
}

/// Moves one exact wish and its optional note to a recoverable local trash.
pub fn move_wish_to_trash(root: &Path, wish_id: &str) -> Result<(), WishFeedbackError> {
    ensure_root(root)?;
    let trash = root.join(WISH_TRASH_DIRECTORY);
    ensure_or_create_directory(&trash, WishFeedbackError::InvalidTrash)?;
    let wish_name = wish_filename(wish_id)?;
    let source = root.join(&wish_name);
    ensure_regular_file(&source, WishFeedbackError::WishUnavailable)?;
    let destination = trash.join(&wish_name);
    if destination.exists() {
        return Err(WishFeedbackError::WishAlreadyExists);
    }
    let note_name = note_filename(wish_id)?;
    let note_source = root.join(&note_name);
    let note_destination = trash.join(&note_name);
    let has_note = match fs::symlink_metadata(&note_source) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => true,
        Ok(_) => return Err(WishFeedbackError::NoteUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(WishFeedbackError::NoteUnavailable),
    };
    if has_note && note_destination.exists() {
        return Err(WishFeedbackError::NoteAlreadyExists);
    }

    fs::rename(&source, &destination).map_err(|_| WishFeedbackError::Io)?;
    if has_note && fs::rename(&note_source, &note_destination).is_err() {
        // Best-effort rollback keeps the active wish and its note together
        // when the second recoverable move unexpectedly fails.
        let _ = fs::rename(&destination, &source);
        return Err(WishFeedbackError::Io);
    }
    Ok(())
}

fn protect_payload(
    plaintext: &[u8],
    magic: &[u8],
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, WishFeedbackError> {
    if plaintext.is_empty() || plaintext.len() > MAX_WISH_PLAINTEXT_BYTES {
        return Err(WishFeedbackError::PlaintextTooLarge);
    }
    let protected = protector
        .protect(plaintext)
        .map_err(|_| WishFeedbackError::Protection)?;
    if protected.is_empty() || protected.len() > MAX_WISH_PACKAGE_BYTES {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let protected_len =
        u32::try_from(protected.len()).map_err(|_| WishFeedbackError::InvalidProtectedPackage)?;
    let mut output = Vec::with_capacity(magic.len() + 4 + protected.len());
    output.extend_from_slice(magic);
    put_u32(&mut output, protected_len);
    output.extend_from_slice(&protected);
    if output.len() > MAX_WISH_PACKAGE_BYTES {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    Ok(output)
}

fn unprotect_payload(
    package: &[u8],
    magic: &[u8],
    protector: &dyn DataProtector,
) -> Result<Vec<u8>, WishFeedbackError> {
    if package.len() <= magic.len() + 4
        || package.len() > MAX_WISH_PACKAGE_BYTES
        || !package.starts_with(magic)
    {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    let length = u32::from_le_bytes(
        package[magic.len()..magic.len() + 4]
            .try_into()
            .map_err(|_| WishFeedbackError::InvalidProtectedPackage)?,
    ) as usize;
    let protected = &package[magic.len() + 4..];
    if protected.is_empty() || protected.len() != length {
        return Err(WishFeedbackError::InvalidProtectedPackage);
    }
    protector
        .unprotect(protected)
        .map_err(|_| WishFeedbackError::Protection)
}

fn render_event(
    output: &mut Vec<u8>,
    event: &NativeFeedbackEvent,
) -> Result<(), WishFeedbackError> {
    if event.validate_and_measure().is_none() {
        return Err(WishFeedbackError::InvalidSnapshot);
    }
    match event {
        NativeFeedbackEvent::CandidatesPresented {
            code,
            view,
            page_start,
            candidates,
            may_have_more,
        } => {
            output.push(1);
            put_string(output, code)?;
            output.push(view_tag(*view));
            put_usize(output, *page_start)?;
            put_usize(output, candidates.len())?;
            for candidate in candidates {
                put_string(output, candidate)?;
            }
            output.push(u8::from(*may_have_more));
        }
        NativeFeedbackEvent::CandidateCommitted {
            code,
            text,
            view,
            source,
            absolute_rank,
            visible_rank,
        } => {
            output.push(2);
            put_string(output, code)?;
            put_string(output, text)?;
            output.push(view_tag(*view));
            output.push(selection_tag(*source));
            put_usize(output, *absolute_rank)?;
            put_usize(output, *visible_rank)?;
        }
        NativeFeedbackEvent::RawCodeCommitted { code } => {
            output.push(3);
            put_string(output, code)?;
        }
        NativeFeedbackEvent::CompositionCancelled { code, source } => {
            output.push(4);
            put_string(output, code)?;
            output.push(cancellation_tag(*source));
        }
        NativeFeedbackEvent::CandidatePopupTiming {
            first_frame_ms,
            fully_visible_ms,
            initial_show,
        } => {
            output.push(5);
            put_u32(output, *first_frame_ms);
            put_u32(output, *fully_visible_ms);
            output.push(u8::from(*initial_show));
        }
    }
    Ok(())
}

fn parse_event(reader: &mut SliceReader<'_>) -> Result<NativeFeedbackEvent, WishFeedbackError> {
    let event = match reader.byte()? {
        1 => {
            let code = reader.string()?;
            let view = parse_view(reader.byte()?)?;
            let page_start = reader.usize()?;
            let count = reader.usize()?;
            if count > 7 {
                return Err(WishFeedbackError::InvalidSnapshot);
            }
            let mut candidates = Vec::with_capacity(count);
            for _ in 0..count {
                candidates.push(reader.string()?);
            }
            NativeFeedbackEvent::CandidatesPresented {
                code,
                view,
                page_start,
                candidates,
                may_have_more: reader.boolean()?,
            }
        }
        2 => NativeFeedbackEvent::CandidateCommitted {
            code: reader.string()?,
            text: reader.string()?,
            view: parse_view(reader.byte()?)?,
            source: parse_selection(reader.byte()?)?,
            absolute_rank: reader.usize()?,
            visible_rank: reader.usize()?,
        },
        3 => NativeFeedbackEvent::RawCodeCommitted {
            code: reader.string()?,
        },
        4 => NativeFeedbackEvent::CompositionCancelled {
            code: reader.string()?,
            source: parse_cancellation(reader.byte()?)?,
        },
        5 => NativeFeedbackEvent::CandidatePopupTiming {
            first_frame_ms: reader.u32()?,
            fully_visible_ms: reader.u32()?,
            initial_show: reader.boolean()?,
        },
        _ => return Err(WishFeedbackError::InvalidSnapshot),
    };
    if event.validate_and_measure().is_none() {
        return Err(WishFeedbackError::InvalidSnapshot);
    }
    Ok(event)
}

fn view_tag(value: NativeCandidateView) -> u8 {
    match value {
        NativeCandidateView::Ordinary => 1,
        NativeCandidateView::TranspositionRecovery => 2,
        NativeCandidateView::Shape => 3,
    }
}

fn parse_view(value: u8) -> Result<NativeCandidateView, WishFeedbackError> {
    match value {
        1 => Ok(NativeCandidateView::Ordinary),
        2 => Ok(NativeCandidateView::TranspositionRecovery),
        3 => Ok(NativeCandidateView::Shape),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn selection_tag(value: NativeSelectionSource) -> u8 {
    match value {
        NativeSelectionSource::FirstCandidate => 1,
        NativeSelectionSource::Numeric => 2,
        NativeSelectionSource::Punctuation => 3,
    }
}

fn parse_selection(value: u8) -> Result<NativeSelectionSource, WishFeedbackError> {
    match value {
        1 => Ok(NativeSelectionSource::FirstCandidate),
        2 => Ok(NativeSelectionSource::Numeric),
        3 => Ok(NativeSelectionSource::Punctuation),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn cancellation_tag(value: NativeCancellationSource) -> u8 {
    match value {
        NativeCancellationSource::Backspace => 1,
        NativeCancellationSource::Escape => 2,
        NativeCancellationSource::FocusLoss => 3,
        NativeCancellationSource::HostTermination => 4,
    }
}

fn parse_cancellation(value: u8) -> Result<NativeCancellationSource, WishFeedbackError> {
    match value {
        1 => Ok(NativeCancellationSource::Backspace),
        2 => Ok(NativeCancellationSource::Escape),
        3 => Ok(NativeCancellationSource::FocusLoss),
        4 => Ok(NativeCancellationSource::HostTermination),
        _ => Err(WishFeedbackError::InvalidSnapshot),
    }
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_usize(output: &mut Vec<u8>, value: usize) -> Result<(), WishFeedbackError> {
    put_u32(
        output,
        u32::try_from(value).map_err(|_| WishFeedbackError::PlaintextTooLarge)?,
    );
    Ok(())
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), WishFeedbackError> {
    if value.len() > MAX_WISH_STRING_BYTES || value.contains('\0') {
        return Err(WishFeedbackError::InvalidPlaintext);
    }
    put_usize(output, value.len())?;
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

struct SliceReader<'a> {
    remaining: &'a [u8],
}

impl<'a> SliceReader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    fn expect(&mut self, expected: &[u8]) -> Result<(), WishFeedbackError> {
        if !self.remaining.starts_with(expected) {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        self.remaining = &self.remaining[expected.len()..];
        Ok(())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WishFeedbackError> {
        if self.remaining.len() < count {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let (head, tail) = self.remaining.split_at(count);
        self.remaining = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, WishFeedbackError> {
        Ok(self.take(1)?[0])
    }

    fn boolean(&mut self) -> Result<bool, WishFeedbackError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WishFeedbackError::InvalidPlaintext),
        }
    }

    fn u32(&mut self) -> Result<u32, WishFeedbackError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| WishFeedbackError::InvalidPlaintext)?,
        ))
    }

    fn usize(&mut self) -> Result<usize, WishFeedbackError> {
        usize::try_from(self.u32()?).map_err(|_| WishFeedbackError::InvalidPlaintext)
    }

    fn string(&mut self) -> Result<String, WishFeedbackError> {
        let length = self.usize()?;
        if length > MAX_WISH_STRING_BYTES {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        let value = std::str::from_utf8(self.take(length)?)
            .map_err(|_| WishFeedbackError::InvalidPlaintext)?;
        if value.contains('\0') {
            return Err(WishFeedbackError::InvalidPlaintext);
        }
        Ok(value.to_owned())
    }

    fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn validate_wish_id(value: &str) -> Result<(), WishFeedbackError> {
    let Some(digest) = value.strip_prefix(WISH_ID_PREFIX) else {
        return Err(WishFeedbackError::InvalidWishId);
    };
    if digest.len() != WISH_ID_HEX_BYTES
        || !digest
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(WishFeedbackError::InvalidWishId);
    }
    Ok(())
}

fn wish_filename(wish_id: &str) -> Result<String, WishFeedbackError> {
    validate_wish_id(wish_id)?;
    Ok(format!("{wish_id}{WISH_PACKAGE_FILE_SUFFIX}"))
}

fn note_filename(wish_id: &str) -> Result<String, WishFeedbackError> {
    validate_wish_id(wish_id)?;
    Ok(format!("{wish_id}{WISH_NOTE_FILE_SUFFIX}"))
}

fn ensure_root(root: &Path) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(WishFeedbackError::InvalidRoot),
        Err(_) => Err(WishFeedbackError::RootUnavailable),
    }
}

fn prepare_root(root: &Path) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(WishFeedbackError::InvalidRoot),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| WishFeedbackError::Io)?;
            ensure_root(root)
        }
        Err(_) => Err(WishFeedbackError::RootUnavailable),
    }
}

fn ensure_or_create_directory(
    path: &Path,
    invalid: WishFeedbackError,
) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| WishFeedbackError::Io)
        }
        Err(_) => Err(WishFeedbackError::Io),
    }
}

fn ensure_regular_file(path: &Path, missing: WishFeedbackError) -> Result<(), WishFeedbackError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(missing),
        Err(_) => Err(missing),
    }
}

fn read_regular_bytes(
    path: &Path,
    maximum: usize,
    unavailable: WishFeedbackError,
) -> Result<Vec<u8>, WishFeedbackError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum as u64
    {
        return Err(unavailable);
    }
    let file = File::open(path).map_err(|_| unavailable)?;
    let mut bytes = Vec::new();
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(unavailable);
    }
    Ok(bytes)
}

fn publish_new(
    root: &Path,
    destination: &Path,
    contents: &[u8],
    exists_error: WishFeedbackError,
) -> Result<(), WishFeedbackError> {
    if destination.exists() {
        return Err(exists_error);
    }
    let counter = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = root.join(format!(".wish-{}-{counter}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| WishFeedbackError::Io)?;
        file.write_all(contents)
            .map_err(|_| WishFeedbackError::Io)?;
        file.sync_all().map_err(|_| WishFeedbackError::Io)?;
        drop(file);
        if destination.exists() {
            return Err(exists_error);
        }
        fs::rename(&temporary, destination).map_err(|_| WishFeedbackError::Io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NativeFeedbackAuthorization, NativeFeedbackContext, NativeFeedbackFreezeAuthorization,
        NativeFeedbackLimits, NativeFeedbackSession,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Clone, Copy)]
    struct TestProtector;

    impl DataProtector for TestProtector {
        fn protection_name(&self) -> &'static str {
            "test"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, crate::ContinuousCaptureError> {
            Ok(plaintext.iter().map(|byte| byte ^ 0x5a).collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, crate::ContinuousCaptureError> {
            self.protect(protected)
        }
    }

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ziranma-wish-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn private_snapshot() -> WishSnapshot {
        let mut feedback = NativeFeedbackSession::default();
        feedback.start_memory(
            NativeFeedbackAuthorization::explicit_memory_only(),
            NativeFeedbackLimits::default(),
        );
        feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::CandidatesPresented {
                code: "wua".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["呜哇".to_owned(), "无哇".to_owned()],
                may_have_more: false,
            },
            1_000,
        );
        feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::CandidateCommitted {
                code: "wua".to_owned(),
                text: "呜哇".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
            1_010,
        );
        let frozen = feedback
            .freeze_recent(
                NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                1_020,
                30_000,
                128,
            )
            .unwrap();
        WishSnapshot::from_frozen(&frozen).unwrap()
    }

    #[test]
    fn private_snapshot_round_trips_without_debug_surface() {
        let snapshot = private_snapshot();
        let rendered = snapshot.render().unwrap();
        let parsed = WishSnapshot::parse(&rendered).unwrap();
        assert_eq!(parsed.events().len(), 2);
        assert!(parsed == snapshot);
        assert!(WishSnapshot::parse(&rendered[..rendered.len() - 1]).is_err());
    }

    #[test]
    fn protected_package_is_immutable_and_bound_to_its_id() {
        let root = TemporaryDirectory::new();
        let snapshot = private_snapshot();
        let receipt = save_wish_snapshot(&root.0, &snapshot, &TestProtector).unwrap();
        assert_eq!(receipt.events(), 2);
        assert_eq!(list_wish_packages(&root.0).unwrap().len(), 1);
        assert!(load_wish_snapshot(&root.0, receipt.id(), &TestProtector).unwrap() == snapshot);

        let path = root.0.join(wish_filename(receipt.id()).unwrap());
        let mut changed = fs::read(&path).unwrap();
        *changed.last_mut().unwrap() ^= 1;
        fs::write(&path, changed).unwrap();
        assert!(matches!(
            load_wish_snapshot(&root.0, receipt.id(), &TestProtector),
            Err(WishFeedbackError::InvalidProtectedPackage)
        ));
    }

    #[test]
    fn private_note_is_bound_and_trash_is_recoverable() {
        let root = TemporaryDirectory::new();
        let receipt = save_wish_snapshot(&root.0, &private_snapshot(), &TestProtector).unwrap();
        let note = WishNote::new(receipt.id(), WishCategory::Ranking, "第一项不太对").unwrap();
        save_wish_note(&root.0, &note, &TestProtector).unwrap();
        assert!(load_wish_note(&root.0, receipt.id(), &TestProtector).unwrap() == note);
        assert!(matches!(
            save_wish_note(&root.0, &note, &TestProtector),
            Err(WishFeedbackError::NoteAlreadyExists)
        ));

        move_wish_to_trash(&root.0, receipt.id()).unwrap();
        assert!(list_wish_packages(&root.0).unwrap().is_empty());
        assert!(
            root.0
                .join(WISH_TRASH_DIRECTORY)
                .join(wish_filename(receipt.id()).unwrap())
                .is_file()
        );
        assert!(
            root.0
                .join(WISH_TRASH_DIRECTORY)
                .join(note_filename(receipt.id()).unwrap())
                .is_file()
        );
    }

    #[test]
    fn discovery_ignores_unrelated_and_symlink_like_entries() {
        let root = TemporaryDirectory::new();
        fs::write(root.0.join("not-a-wish.txt"), b"public").unwrap();
        fs::create_dir(root.0.join("wish-not-a-file.ziw")).unwrap();
        assert!(list_wish_packages(&root.0).unwrap().is_empty());
        assert!(validate_wish_id("../private").is_err());
    }
}
