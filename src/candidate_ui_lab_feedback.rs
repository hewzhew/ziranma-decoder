//! Strict, bounded reader for reconstructable public candidate UI lab feedback.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::candidate_ui::{
    CandidateRgb, CandidateSceneLayout, CandidateSceneRect, CandidateSceneSemantic,
    CandidateVisualSpec,
};
use crate::candidate_ui_lab_annotation::{
    CANDIDATE_UI_LAB_ANNOTATION_BATCH_SCHEMA, CANDIDATE_UI_LAB_ANNOTATION_SCHEMA,
    CANDIDATE_UI_LAB_VISUAL_SPEC_SCHEMA, MAX_CANDIDATE_UI_LAB_ANNOTATIONS,
    MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS, candidate_visual_spec_sha256,
};
use crate::candidate_ui_lab_visual::reviewed_candidate_ui_lab_spec;

pub(crate) const MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES: usize = 1024 * 1024;
const MAX_FEEDBACK_GROUPS: usize = MAX_CANDIDATE_UI_LAB_ANNOTATIONS;
const MAX_FEEDBACK_HITS: usize = 64;
const MAX_FEEDBACK_COORDINATE: i32 = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabFeedbackError {
    TooLarge,
    InvalidUtf8,
    InvalidSyntax,
    UnsupportedSchema,
    InvalidValue,
    InvalidVisualSpec,
    HashMismatch,
    CountMismatch,
    GroupMismatch,
    TrailingData,
    ReadFile,
}

impl std::fmt::Display for CandidateUiLabFeedbackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooLarge => "候选窗实验反馈超过固定读取上限",
            Self::InvalidUtf8 => "候选窗实验反馈不是有效 UTF-8",
            Self::InvalidSyntax => "候选窗实验反馈不是规范 JSON",
            Self::UnsupportedSchema => "候选窗实验反馈版本不受支持",
            Self::InvalidValue => "候选窗实验反馈包含无效字段",
            Self::InvalidVisualSpec => "候选窗实验反馈包含不可重放的视觉规格",
            Self::HashMismatch => "候选窗实验反馈的视觉规格校验不一致",
            Self::CountMismatch => "候选窗实验反馈的条目计数不一致",
            Self::GroupMismatch => "候选窗实验反馈的规格分组不一致",
            Self::TrailingData => "候选窗实验反馈末尾包含额外数据",
            Self::ReadFile => "无法读取所选候选窗实验反馈文件",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabFeedbackBatch {
    pub(crate) groups: Vec<CandidateUiLabFeedbackSpecGroup>,
    pub(crate) annotations: Vec<CandidateUiLabFeedbackAnnotation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabFeedbackSpecGroup {
    pub(crate) variant_id: String,
    pub(crate) visual_spec_sha256: String,
    pub(crate) annotation_count: usize,
    pub(crate) visual_spec: CandidateVisualSpec,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabFeedbackAnnotation {
    pub(crate) scenario_id: String,
    pub(crate) variant_id: String,
    pub(crate) layout: CandidateSceneLayout,
    pub(crate) dpi: u32,
    pub(crate) selection: CandidateSceneRect,
    pub(crate) visual_spec_sha256: String,
    pub(crate) visual_spec: CandidateVisualSpec,
    pub(crate) hits: Vec<CandidateUiLabFeedbackHit>,
    pub(crate) note: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabFeedbackHit {
    pub(crate) semantic: CandidateSceneSemantic,
    pub(crate) candidate_index: Option<usize>,
    pub(crate) bounds: CandidateSceneRect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabFeedbackReviewError {
    EmptyBatch,
}

impl std::fmt::Display for CandidateUiLabFeedbackReviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::EmptyBatch => "所选候选窗实验反馈没有可浏览的批注",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabFeedbackReview {
    batch: CandidateUiLabFeedbackBatch,
    selected_index: usize,
}

impl CandidateUiLabFeedbackReview {
    pub(crate) fn new(
        batch: CandidateUiLabFeedbackBatch,
    ) -> Result<Self, CandidateUiLabFeedbackReviewError> {
        if batch.annotations.is_empty() {
            return Err(CandidateUiLabFeedbackReviewError::EmptyBatch);
        }
        Ok(Self {
            batch,
            selected_index: 0,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.batch.annotations.len()
    }

    pub(crate) const fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub(crate) fn selected_annotation(&self) -> &CandidateUiLabFeedbackAnnotation {
        &self.batch.annotations[self.selected_index]
    }

    pub(crate) fn selected_group(&self) -> Option<&CandidateUiLabFeedbackSpecGroup> {
        let annotation = self.selected_annotation();
        self.batch.groups.iter().find(|group| {
            group.variant_id == annotation.variant_id
                && group.visual_spec_sha256 == annotation.visual_spec_sha256
                && group.visual_spec == annotation.visual_spec
        })
    }

    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.len() || index == self.selected_index {
            return false;
        }
        self.selected_index = index;
        true
    }

    pub(crate) fn select_previous(&mut self) -> bool {
        self.selected_index
            .checked_sub(1)
            .is_some_and(|index| self.select(index))
    }

    pub(crate) fn select_next(&mut self) -> bool {
        self.selected_index
            .checked_add(1)
            .is_some_and(|index| self.select(index))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedSpecGroup {
    value: CandidateUiLabFeedbackSpecGroup,
    visual_spec_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedAnnotation {
    value: CandidateUiLabFeedbackAnnotation,
    visual_spec_json: String,
}

pub(crate) fn parse_candidate_ui_lab_feedback(
    input: &[u8],
) -> Result<CandidateUiLabFeedbackBatch, CandidateUiLabFeedbackError> {
    if input.len() > MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES {
        return Err(CandidateUiLabFeedbackError::TooLarge);
    }
    let input = std::str::from_utf8(input).map_err(|_| CandidateUiLabFeedbackError::InvalidUtf8)?;
    let mut cursor = Cursor::new(input);
    cursor.expect("{\"schema\":")?;
    if cursor.parse_string()? != CANDIDATE_UI_LAB_ANNOTATION_BATCH_SCHEMA {
        return Err(CandidateUiLabFeedbackError::UnsupportedSchema);
    }
    cursor.expect(",\"annotation_schema\":")?;
    if cursor.parse_string()? != CANDIDATE_UI_LAB_ANNOTATION_SCHEMA {
        return Err(CandidateUiLabFeedbackError::UnsupportedSchema);
    }
    cursor.expect(",\"count\":")?;
    let declared_count = cursor.parse_usize()?;
    if declared_count > MAX_CANDIDATE_UI_LAB_ANNOTATIONS {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    cursor.expect(",\"spec_groups\":[")?;
    let parsed_groups = parse_spec_groups(&mut cursor)?;
    cursor.expect(",\"annotations\":[")?;
    let parsed_annotations = parse_annotations(&mut cursor)?;
    cursor.expect("}")?;
    if !cursor.is_finished() {
        return Err(CandidateUiLabFeedbackError::TrailingData);
    }
    if parsed_annotations.len() != declared_count {
        return Err(CandidateUiLabFeedbackError::CountMismatch);
    }
    validate_groups(&parsed_groups, &parsed_annotations)?;
    Ok(CandidateUiLabFeedbackBatch {
        groups: parsed_groups.into_iter().map(|group| group.value).collect(),
        annotations: parsed_annotations
            .into_iter()
            .map(|annotation| annotation.value)
            .collect(),
    })
}

pub(crate) fn read_candidate_ui_lab_feedback_file(
    path: &Path,
) -> Result<CandidateUiLabFeedbackBatch, CandidateUiLabFeedbackError> {
    let file = File::open(path).map_err(|_| CandidateUiLabFeedbackError::ReadFile)?;
    let read_limit = u64::try_from(MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut input = Vec::with_capacity(MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES.min(64 * 1024));
    file.take(read_limit)
        .read_to_end(&mut input)
        .map_err(|_| CandidateUiLabFeedbackError::ReadFile)?;
    parse_candidate_ui_lab_feedback(&input)
}

fn parse_spec_groups(
    cursor: &mut Cursor<'_>,
) -> Result<Vec<ParsedSpecGroup>, CandidateUiLabFeedbackError> {
    let mut groups = Vec::new();
    if cursor.consume("]") {
        return Ok(groups);
    }
    loop {
        if groups.len() >= MAX_FEEDBACK_GROUPS {
            return Err(CandidateUiLabFeedbackError::InvalidValue);
        }
        groups.push(parse_spec_group(cursor)?);
        if cursor.consume(",") {
            continue;
        }
        cursor.expect("]")?;
        break;
    }
    Ok(groups)
}

fn parse_spec_group(
    cursor: &mut Cursor<'_>,
) -> Result<ParsedSpecGroup, CandidateUiLabFeedbackError> {
    cursor.expect("{\"variant\":")?;
    let variant_id = parse_variant(cursor)?;
    cursor.expect(",\"visual_spec_sha256\":")?;
    let visual_spec_sha256 = parse_hash(cursor)?;
    cursor.expect(",\"annotation_count\":")?;
    let annotation_count = cursor.parse_usize()?;
    if !(1..=MAX_CANDIDATE_UI_LAB_ANNOTATIONS).contains(&annotation_count) {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    cursor.expect(",\"visual_spec\":")?;
    let (visual_spec, visual_spec_json) = parse_visual_spec(cursor)?;
    cursor.expect("}")?;
    if candidate_visual_spec_sha256(visual_spec) != visual_spec_sha256 {
        return Err(CandidateUiLabFeedbackError::HashMismatch);
    }
    Ok(ParsedSpecGroup {
        value: CandidateUiLabFeedbackSpecGroup {
            variant_id,
            visual_spec_sha256,
            annotation_count,
            visual_spec,
        },
        visual_spec_json,
    })
}

fn parse_annotations(
    cursor: &mut Cursor<'_>,
) -> Result<Vec<ParsedAnnotation>, CandidateUiLabFeedbackError> {
    let mut annotations = Vec::new();
    if cursor.consume("]") {
        return Ok(annotations);
    }
    loop {
        if annotations.len() >= MAX_CANDIDATE_UI_LAB_ANNOTATIONS {
            return Err(CandidateUiLabFeedbackError::InvalidValue);
        }
        annotations.push(parse_annotation(cursor)?);
        if cursor.consume(",") {
            continue;
        }
        cursor.expect("]")?;
        break;
    }
    Ok(annotations)
}

fn parse_annotation(
    cursor: &mut Cursor<'_>,
) -> Result<ParsedAnnotation, CandidateUiLabFeedbackError> {
    cursor.expect("{\"schema\":")?;
    if cursor.parse_string()? != CANDIDATE_UI_LAB_ANNOTATION_SCHEMA {
        return Err(CandidateUiLabFeedbackError::UnsupportedSchema);
    }
    cursor.expect(",\"scenario\":")?;
    let scenario_id = parse_scenario(cursor)?;
    cursor.expect(",\"variant\":")?;
    let variant_id = parse_variant(cursor)?;
    cursor.expect(",\"layout\":")?;
    let layout = parse_layout(cursor)?;
    cursor.expect(",\"dpi\":")?;
    let dpi = u32::try_from(cursor.parse_usize()?)
        .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)?;
    if ![96, 120, 144, 192].contains(&dpi) {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    cursor.expect(",\"selection\":")?;
    let selection = parse_rect(cursor)?;
    cursor.expect(",\"visual_spec_sha256\":")?;
    let visual_spec_sha256 = parse_hash(cursor)?;
    cursor.expect(",\"visual_spec\":")?;
    let (visual_spec, visual_spec_json) = parse_visual_spec(cursor)?;
    if candidate_visual_spec_sha256(visual_spec) != visual_spec_sha256 {
        return Err(CandidateUiLabFeedbackError::HashMismatch);
    }
    cursor.expect(",\"hits\":[")?;
    let hits = parse_hits(cursor)?;
    cursor.expect(",\"note\":")?;
    let note = cursor.parse_string()?;
    if note.is_empty()
        || note.chars().count() > MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS
        || note.trim() != note
        || note.contains('\r')
        || note
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    cursor.expect("}")?;
    Ok(ParsedAnnotation {
        value: CandidateUiLabFeedbackAnnotation {
            scenario_id,
            variant_id,
            layout,
            dpi,
            selection,
            visual_spec_sha256,
            visual_spec,
            hits,
            note,
        },
        visual_spec_json,
    })
}

fn parse_hits(
    cursor: &mut Cursor<'_>,
) -> Result<Vec<CandidateUiLabFeedbackHit>, CandidateUiLabFeedbackError> {
    let mut hits = Vec::new();
    if cursor.consume("]") {
        return Ok(hits);
    }
    loop {
        if hits.len() >= MAX_FEEDBACK_HITS {
            return Err(CandidateUiLabFeedbackError::InvalidValue);
        }
        hits.push(parse_hit(cursor)?);
        if cursor.consume(",") {
            continue;
        }
        cursor.expect("]")?;
        break;
    }
    Ok(hits)
}

fn parse_hit(
    cursor: &mut Cursor<'_>,
) -> Result<CandidateUiLabFeedbackHit, CandidateUiLabFeedbackError> {
    cursor.expect("{\"semantic\":")?;
    let semantic = parse_semantic(cursor)?;
    cursor.expect(",\"candidate_index_zero_based\":")?;
    let candidate_index = if cursor.consume("null") {
        None
    } else {
        let index = cursor.parse_usize()?;
        if index >= MAX_FEEDBACK_HITS {
            return Err(CandidateUiLabFeedbackError::InvalidValue);
        }
        Some(index)
    };
    if semantic_has_candidate(semantic) != candidate_index.is_some() {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    cursor.expect(",\"bounds\":")?;
    let bounds = parse_rect(cursor)?;
    cursor.expect("}")?;
    Ok(CandidateUiLabFeedbackHit {
        semantic,
        candidate_index,
        bounds,
    })
}

fn validate_groups(
    groups: &[ParsedSpecGroup],
    annotations: &[ParsedAnnotation],
) -> Result<(), CandidateUiLabFeedbackError> {
    let mut expected = BTreeMap::new();
    let mut previous_key: Option<(String, String, String)> = None;
    for group in groups {
        let key = (
            group.value.variant_id.clone(),
            group.value.visual_spec_sha256.clone(),
            group.visual_spec_json.clone(),
        );
        if previous_key
            .as_ref()
            .is_some_and(|previous| previous >= &key)
        {
            return Err(CandidateUiLabFeedbackError::GroupMismatch);
        }
        previous_key = Some(key.clone());
        expected.insert(key, group.value.annotation_count);
    }
    let mut actual = BTreeMap::new();
    for annotation in annotations {
        let key = (
            annotation.value.variant_id.clone(),
            annotation.value.visual_spec_sha256.clone(),
            annotation.visual_spec_json.clone(),
        );
        if !expected.contains_key(&key) {
            return Err(CandidateUiLabFeedbackError::GroupMismatch);
        }
        *actual.entry(key).or_insert(0usize) += 1;
    }
    if actual != expected {
        return Err(CandidateUiLabFeedbackError::CountMismatch);
    }
    Ok(())
}

fn parse_variant(cursor: &mut Cursor<'_>) -> Result<String, CandidateUiLabFeedbackError> {
    let value = cursor.parse_string()?;
    if !matches!(value.as_str(), "baseline" | "draft") {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    Ok(value)
}

fn parse_scenario(cursor: &mut Cursor<'_>) -> Result<String, CandidateUiLabFeedbackError> {
    let value = cursor.parse_string()?;
    if !matches!(
        value.as_str(),
        "everyday" | "long-candidate" | "personalized"
    ) {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    Ok(value)
}

fn parse_layout(
    cursor: &mut Cursor<'_>,
) -> Result<CandidateSceneLayout, CandidateUiLabFeedbackError> {
    match cursor.parse_string()?.as_str() {
        "horizontal" => Ok(CandidateSceneLayout::Horizontal),
        "vertical" => Ok(CandidateSceneLayout::Vertical),
        _ => Err(CandidateUiLabFeedbackError::InvalidValue),
    }
}

fn parse_hash(cursor: &mut Cursor<'_>) -> Result<String, CandidateUiLabFeedbackError> {
    let value = cursor.parse_string()?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    Ok(value)
}

fn parse_semantic(
    cursor: &mut Cursor<'_>,
) -> Result<CandidateSceneSemantic, CandidateUiLabFeedbackError> {
    match cursor.parse_string()?.as_str() {
        "candidate.item" => Ok(CandidateSceneSemantic::CandidateItem),
        "candidate.selected.surface" => Ok(CandidateSceneSemantic::CandidateSelectedSurface),
        "candidate.rank" => Ok(CandidateSceneSemantic::CandidateRank),
        "candidate.text" => Ok(CandidateSceneSemantic::CandidateText),
        "candidate.action-detail" => Ok(CandidateSceneSemantic::CandidateActionDetail),
        "candidate.personal-mark" => Ok(CandidateSceneSemantic::CandidatePersonalMark),
        "notice.icon" => Ok(CandidateSceneSemantic::NoticeIcon),
        "selection.accent" => Ok(CandidateSceneSemantic::SelectionAccent),
        "footer" => Ok(CandidateSceneSemantic::Footer),
        "footer.divider" => Ok(CandidateSceneSemantic::FooterDivider),
        "footer.mode" => Ok(CandidateSceneSemantic::FooterMode),
        "footer.page" => Ok(CandidateSceneSemantic::FooterPage),
        _ => Err(CandidateUiLabFeedbackError::InvalidValue),
    }
}

const fn semantic_has_candidate(semantic: CandidateSceneSemantic) -> bool {
    matches!(
        semantic,
        CandidateSceneSemantic::CandidateItem
            | CandidateSceneSemantic::CandidateSelectedSurface
            | CandidateSceneSemantic::CandidateRank
            | CandidateSceneSemantic::CandidateText
            | CandidateSceneSemantic::CandidateActionDetail
            | CandidateSceneSemantic::CandidatePersonalMark
            | CandidateSceneSemantic::NoticeIcon
            | CandidateSceneSemantic::SelectionAccent
    )
}

fn parse_rect(cursor: &mut Cursor<'_>) -> Result<CandidateSceneRect, CandidateUiLabFeedbackError> {
    cursor.expect("{\"left\":")?;
    let left = cursor.parse_i32()?;
    cursor.expect(",\"top\":")?;
    let top = cursor.parse_i32()?;
    cursor.expect(",\"right\":")?;
    let right = cursor.parse_i32()?;
    cursor.expect(",\"bottom\":")?;
    let bottom = cursor.parse_i32()?;
    cursor.expect("}")?;
    let rectangle = CandidateSceneRect {
        left,
        top,
        right,
        bottom,
    };
    if left < 0
        || top < 0
        || right <= left
        || bottom <= top
        || right > MAX_FEEDBACK_COORDINATE
        || bottom > MAX_FEEDBACK_COORDINATE
    {
        return Err(CandidateUiLabFeedbackError::InvalidValue);
    }
    Ok(rectangle)
}

fn parse_visual_spec(
    cursor: &mut Cursor<'_>,
) -> Result<(CandidateVisualSpec, String), CandidateUiLabFeedbackError> {
    let start = cursor.offset;
    cursor.expect("{\"schema\":")?;
    if cursor.parse_string()? != CANDIDATE_UI_LAB_VISUAL_SPEC_SCHEMA {
        return Err(CandidateUiLabFeedbackError::UnsupportedSchema);
    }
    macro_rules! integer {
        ($name:literal) => {{
            cursor.expect(concat!(",\"", $name, "\":"))?;
            cursor.parse_i32()?
        }};
    }
    let spec = CandidateVisualSpec {
        outer_padding: integer!("outer_padding"),
        row_height: integer!("row_height"),
        text_padding: integer!("text_padding"),
        selected_text_inset: integer!("selected_text_inset"),
        rank_width: integer!("rank_width"),
        rank_gap: integer!("rank_gap"),
        footer_content_inset: integer!("footer_content_inset"),
        horizontal_max_width: integer!("horizontal_max_width"),
        horizontal_min_width: integer!("horizontal_min_width"),
        horizontal_min_item_width: integer!("horizontal_min_item_width"),
        horizontal_text_max_width: integer!("horizontal_text_max_width"),
        horizontal_selected_text_max_width: integer!("horizontal_selected_text_max_width"),
        vertical_min_width: integer!("vertical_min_width"),
        vertical_text_max_width: integer!("vertical_text_max_width"),
        vertical_max_width: integer!("vertical_max_width"),
        vertical_rounding_slack: integer!("vertical_rounding_slack"),
        action_min_width: integer!("action_min_width"),
        action_detail_gap: integer!("action_detail_gap"),
        notice_icon_size: integer!("notice_icon_size"),
        notice_icon_gap: integer!("notice_icon_gap"),
        corner_diameter: integer!("corner_diameter"),
        border_width: integer!("border_width"),
        selected_surface_height: integer!("selected_surface_height"),
        selected_surface_left_inset: integer!("selected_surface_left_inset"),
        selected_surface_right_inset: integer!("selected_surface_right_inset"),
        selected_surface_corner_diameter: integer!("selected_surface_corner_diameter"),
        selection_accent_width: integer!("selection_accent_width"),
        selection_accent_fallback_height: integer!("selection_accent_fallback_height"),
        selection_accent_left_inset: integer!("selection_accent_left_inset"),
        selection_accent_corner_diameter: integer!("selection_accent_corner_diameter"),
        personal_mark_size: integer!("personal_mark_size"),
        footer_height: integer!("footer_height"),
        footer_vertical_inset: integer!("footer_vertical_inset"),
        footer_divider_inset: integer!("footer_divider_inset"),
        footer_divider_width: integer!("footer_divider_width"),
        footer_page_width: integer!("footer_page_width"),
        footer_mode_gap: integer!("footer_mode_gap"),
        candidate_font_height: integer!("candidate_font_height"),
        metadata_font_height: integer!("metadata_font_height"),
        background: parse_color(cursor, "background")?,
        selected_background: parse_color(cursor, "selected_background")?,
        selected_text: parse_color(cursor, "selected_text")?,
        candidate_text: parse_color(cursor, "candidate_text")?,
        selected_rank: parse_color(cursor, "selected_rank")?,
        rank: parse_color(cursor, "rank")?,
        page: parse_color(cursor, "page")?,
        selection_accent: parse_color(cursor, "selection_accent")?,
        mode_accent: parse_color(cursor, "mode_accent")?,
        border: parse_color(cursor, "border")?,
        footer_divider: parse_color(cursor, "footer_divider")?,
    };
    cursor.expect("}")?;
    if !reviewed_candidate_ui_lab_spec(spec) {
        return Err(CandidateUiLabFeedbackError::InvalidVisualSpec);
    }
    Ok((spec, cursor.input[start..cursor.offset].to_owned()))
}

fn parse_color(
    cursor: &mut Cursor<'_>,
    field: &str,
) -> Result<CandidateRgb, CandidateUiLabFeedbackError> {
    cursor.expect(",\"")?;
    cursor.expect(field)?;
    cursor.expect("\":{\"red\":")?;
    let red = u8::try_from(cursor.parse_usize()?)
        .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)?;
    cursor.expect(",\"green\":")?;
    let green = u8::try_from(cursor.parse_usize()?)
        .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)?;
    cursor.expect(",\"blue\":")?;
    let blue = u8::try_from(cursor.parse_usize()?)
        .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)?;
    cursor.expect("}")?;
    Ok(CandidateRgb { red, green, blue })
}

struct Cursor<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn expect(&mut self, expected: &str) -> Result<(), CandidateUiLabFeedbackError> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(CandidateUiLabFeedbackError::InvalidSyntax)
        }
    }

    fn consume(&mut self, expected: &str) -> bool {
        if self.input[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    const fn is_finished(&self) -> bool {
        self.offset == self.input.len()
    }

    fn parse_i32(&mut self) -> Result<i32, CandidateUiLabFeedbackError> {
        let start = self.offset;
        let bytes = self.input.as_bytes();
        let negative = bytes.get(self.offset) == Some(&b'-');
        if negative {
            self.offset += 1;
        }
        let digits_start = self.offset;
        while bytes.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
        }
        if self.offset == digits_start
            || (bytes[digits_start] == b'0' && self.offset > digits_start + 1)
            || (negative && bytes[digits_start] == b'0')
        {
            return Err(CandidateUiLabFeedbackError::InvalidSyntax);
        }
        self.input[start..self.offset]
            .parse::<i32>()
            .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)
    }

    fn parse_usize(&mut self) -> Result<usize, CandidateUiLabFeedbackError> {
        let start = self.offset;
        let bytes = self.input.as_bytes();
        while bytes.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
        }
        if self.offset == start || (bytes[start] == b'0' && self.offset > start + 1) {
            return Err(CandidateUiLabFeedbackError::InvalidSyntax);
        }
        self.input[start..self.offset]
            .parse::<usize>()
            .map_err(|_| CandidateUiLabFeedbackError::InvalidValue)
    }

    fn parse_string(&mut self) -> Result<String, CandidateUiLabFeedbackError> {
        self.expect("\"")?;
        let mut output = String::new();
        loop {
            let character = self.input[self.offset..]
                .chars()
                .next()
                .ok_or(CandidateUiLabFeedbackError::InvalidSyntax)?;
            match character {
                '"' => {
                    self.offset += 1;
                    return Ok(output);
                }
                '\\' => {
                    self.offset += 1;
                    let escape = self.input[self.offset..]
                        .chars()
                        .next()
                        .ok_or(CandidateUiLabFeedbackError::InvalidSyntax)?;
                    self.offset += escape.len_utf8();
                    match escape {
                        '"' => output.push('"'),
                        '\\' => output.push('\\'),
                        'n' => output.push('\n'),
                        'r' => output.push('\r'),
                        't' => output.push('\t'),
                        'u' => output.push(self.parse_unicode_escape()?),
                        _ => return Err(CandidateUiLabFeedbackError::InvalidSyntax),
                    }
                }
                character if character.is_control() => {
                    return Err(CandidateUiLabFeedbackError::InvalidSyntax);
                }
                character => {
                    output.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, CandidateUiLabFeedbackError> {
        let digits = self
            .input
            .as_bytes()
            .get(self.offset..)
            .and_then(|remaining| remaining.get(..4))
            .ok_or(CandidateUiLabFeedbackError::InvalidSyntax)?;
        let mut scalar = 0_u32;
        for digit in digits {
            let digit = hex_value(*digit)?;
            scalar = scalar
                .checked_mul(16)
                .and_then(|value| value.checked_add(u32::from(digit)))
                .ok_or(CandidateUiLabFeedbackError::InvalidSyntax)?;
        }
        self.offset += 4;
        char::from_u32(scalar).ok_or(CandidateUiLabFeedbackError::InvalidSyntax)
    }
}

fn hex_value(byte: u8) -> Result<u8, CandidateUiLabFeedbackError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CandidateUiLabFeedbackError::InvalidSyntax),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::candidate_ui::{
        CandidateScene, CandidateSceneRequest, DEFAULT_CANDIDATE_VISUAL_SPEC, build_candidate_scene,
    };
    use crate::candidate_ui_lab_annotation::{
        CandidateUiLabAnnotationContext, CandidateUiLabAnnotationSession,
        capture_candidate_ui_lab_annotation_context,
    };

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ziranma-candidate-ui-feedback-reader-test-{}-{sequence}",
                std::process::id()
            ));
            assert!(!path.exists());
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FeedbackFixture {
        json: String,
        baseline: CandidateUiLabAnnotationContext,
        draft: CandidateUiLabAnnotationContext,
        draft_spec: CandidateVisualSpec,
    }

    fn horizontal_scene() -> CandidateScene {
        build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 110,
                height: 46,
                candidate_count: 1,
                horizontal_candidate_widths: &[100],
                footer_width: 0,
                footer_mode: false,
                footer_page: false,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[false],
                rank_metrics: None,
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
            },
        )
        .unwrap()
    }

    fn vertical_scene(spec: CandidateVisualSpec) -> CandidateScene {
        build_candidate_scene(
            spec,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Vertical,
                dpi: 144,
                width: 600,
                height: 70,
                candidate_count: 1,
                horizontal_candidate_widths: &[],
                footer_width: 0,
                footer_mode: false,
                footer_page: false,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[true],
                rank_metrics: None,
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
            },
        )
        .unwrap()
    }

    fn feedback_fixture() -> FeedbackFixture {
        let baseline_scene = horizontal_scene();
        let baseline = capture_candidate_ui_lab_annotation_context(
            "everyday",
            "baseline",
            CandidateSceneLayout::Horizontal,
            96,
            baseline_scene.client,
            &baseline_scene,
            DEFAULT_CANDIDATE_VISUAL_SPEC,
        )
        .unwrap();
        let mut draft_spec = DEFAULT_CANDIDATE_VISUAL_SPEC;
        draft_spec.rank_gap += 1;
        draft_spec.selection_accent.red += 1;
        let draft_scene = vertical_scene(draft_spec);
        let draft = capture_candidate_ui_lab_annotation_context(
            "long-candidate",
            "draft",
            CandidateSceneLayout::Vertical,
            144,
            draft_scene.client,
            &draft_scene,
            draft_spec,
        )
        .unwrap();
        let mut session = CandidateUiLabAnnotationSession::default();
        session
            .add(baseline.clone(), "绝不应出现在错误里的秘密\n含引号 \"喵\"")
            .unwrap();
        session.add(draft.clone(), "第二条\t😺").unwrap();
        FeedbackFixture {
            json: session.to_canonical_json(),
            baseline,
            draft,
            draft_spec,
        }
    }

    fn assert_hits_survive(
        parsed: &[CandidateUiLabFeedbackHit],
        expected: &[crate::candidate_ui::CandidateSceneHit],
    ) {
        assert_eq!(parsed.len(), expected.len());
        for (parsed, expected) in parsed.iter().zip(expected) {
            assert_eq!(parsed.semantic, expected.semantic);
            assert_eq!(parsed.candidate_index, expected.candidate_index);
            assert_eq!(parsed.bounds, expected.bounds);
        }
    }

    #[test]
    fn exact_exported_v3_batch_round_trips_without_losing_context() {
        let fixture = feedback_fixture();
        let parsed = parse_candidate_ui_lab_feedback(fixture.json.as_bytes()).unwrap();
        assert_eq!(parsed.groups.len(), 2);
        assert_eq!(parsed.annotations.len(), 2);
        assert_eq!(parsed.groups[0].variant_id, "baseline");
        assert_eq!(parsed.groups[0].annotation_count, 1);
        assert_eq!(parsed.groups[0].visual_spec, DEFAULT_CANDIDATE_VISUAL_SPEC);
        assert_eq!(parsed.groups[1].variant_id, "draft");
        assert_eq!(parsed.groups[1].annotation_count, 1);
        assert_eq!(parsed.groups[1].visual_spec, fixture.draft_spec);
        assert_eq!(
            parsed.groups[0].visual_spec_sha256,
            fixture.baseline.visual_spec_sha256
        );
        assert_eq!(
            parsed.groups[1].visual_spec_sha256,
            fixture.draft.visual_spec_sha256
        );

        let baseline = &parsed.annotations[0];
        assert_eq!(baseline.scenario_id, fixture.baseline.scenario_id);
        assert_eq!(baseline.variant_id, fixture.baseline.variant_id);
        assert_eq!(baseline.layout, CandidateSceneLayout::Horizontal);
        assert_eq!(baseline.dpi, 96);
        assert_eq!(baseline.selection, fixture.baseline.selection);
        assert_eq!(baseline.visual_spec, DEFAULT_CANDIDATE_VISUAL_SPEC);
        assert_eq!(baseline.note, "绝不应出现在错误里的秘密\n含引号 \"喵\"");
        assert_hits_survive(&baseline.hits, &fixture.baseline.hits);

        let draft = &parsed.annotations[1];
        assert_eq!(draft.scenario_id, fixture.draft.scenario_id);
        assert_eq!(draft.variant_id, fixture.draft.variant_id);
        assert_eq!(draft.layout, CandidateSceneLayout::Vertical);
        assert_eq!(draft.dpi, 144);
        assert_eq!(draft.selection, fixture.draft.selection);
        assert_eq!(draft.visual_spec, fixture.draft_spec);
        assert_eq!(draft.note, "第二条\t😺");
        assert_hits_survive(&draft.hits, &fixture.draft.hits);
    }

    #[test]
    fn explicit_file_reader_is_bounded_and_never_scans_for_an_input() {
        let directory = TestDirectory::new();
        let selected = directory.0.join("selected.json");
        let ignored = directory.0.join("ignored.json");
        let fixture = feedback_fixture();
        fs::write(&selected, &fixture.json).unwrap();
        fs::write(&ignored, b"not feedback").unwrap();
        let parsed = read_candidate_ui_lab_feedback_file(&selected).unwrap();
        assert_eq!(parsed.annotations.len(), 2);

        fs::write(
            &selected,
            vec![b' '; MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES + 1],
        )
        .unwrap();
        assert_eq!(
            read_candidate_ui_lab_feedback_file(&selected),
            Err(CandidateUiLabFeedbackError::TooLarge)
        );

        let missing = directory.0.join("private-path-marker.json");
        let error = read_candidate_ui_lab_feedback_file(&missing).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert_eq!(error, CandidateUiLabFeedbackError::ReadFile);
        assert!(!rendered.contains("private-path-marker"));
    }

    #[test]
    fn review_navigation_is_bounded_and_resolves_the_exact_spec_group() {
        let fixture = feedback_fixture();
        let batch = parse_candidate_ui_lab_feedback(fixture.json.as_bytes()).unwrap();
        let mut review = CandidateUiLabFeedbackReview::new(batch).unwrap();
        assert_eq!(review.len(), 2);
        assert_eq!(review.selected_index(), 0);
        assert_eq!(review.selected_annotation().variant_id, "baseline");
        assert_eq!(review.selected_group().unwrap().annotation_count, 1);
        assert!(!review.select_previous());
        assert!(review.select_next());
        assert_eq!(review.selected_index(), 1);
        assert_eq!(review.selected_annotation().visual_spec, fixture.draft_spec);
        assert_eq!(review.selected_group().unwrap().variant_id, "draft");
        assert!(!review.select_next());
        assert!(review.select_previous());
        assert!(!review.select(0));
        assert!(!review.select(usize::MAX));

        let empty = parse_candidate_ui_lab_feedback(
            b"{\"schema\":\"candidate-ui-lab-annotation-batch-v3\",\"annotation_schema\":\"candidate-ui-lab-annotation-v3\",\"count\":0,\"spec_groups\":[],\"annotations\":[]}",
        )
        .unwrap();
        assert_eq!(
            CandidateUiLabFeedbackReview::new(empty),
            Err(CandidateUiLabFeedbackReviewError::EmptyBatch)
        );
    }

    #[test]
    fn old_batch_annotation_and_visual_schemas_are_explicitly_unsupported() {
        let valid = feedback_fixture().json;
        let old_batch = valid.replacen(
            CANDIDATE_UI_LAB_ANNOTATION_BATCH_SCHEMA,
            "candidate-ui-lab-annotation-batch-v2",
            1,
        );
        assert_eq!(
            parse_candidate_ui_lab_feedback(old_batch.as_bytes()),
            Err(CandidateUiLabFeedbackError::UnsupportedSchema)
        );

        let old_annotation = valid.replacen(
            "\"schema\":\"candidate-ui-lab-annotation-v3\"",
            "\"schema\":\"candidate-ui-lab-annotation-v2\"",
            1,
        );
        assert_ne!(old_annotation, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(old_annotation.as_bytes()),
            Err(CandidateUiLabFeedbackError::UnsupportedSchema)
        );

        let old_visual = valid.replacen(
            CANDIDATE_UI_LAB_VISUAL_SPEC_SCHEMA,
            "candidate-ui-lab-visual-spec-v0",
            1,
        );
        assert_eq!(
            parse_candidate_ui_lab_feedback(old_visual.as_bytes()),
            Err(CandidateUiLabFeedbackError::UnsupportedSchema)
        );
    }

    #[test]
    fn hashes_counts_groups_and_visual_bounds_are_verified() {
        let valid = feedback_fixture().json;

        let changed_without_hash = valid.replacen("\"rank_gap\":4", "\"rank_gap\":5", 1);
        assert_ne!(changed_without_hash, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(changed_without_hash.as_bytes()),
            Err(CandidateUiLabFeedbackError::HashMismatch)
        );

        let changed_count = valid.replacen("\"annotation_count\":1", "\"annotation_count\":2", 1);
        assert_ne!(changed_count, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(changed_count.as_bytes()),
            Err(CandidateUiLabFeedbackError::CountMismatch)
        );

        let changed_group = valid.replacen(
            "\"scenario\":\"everyday\",\"variant\":\"baseline\"",
            "\"scenario\":\"everyday\",\"variant\":\"draft\"",
            1,
        );
        assert_ne!(changed_group, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(changed_group.as_bytes()),
            Err(CandidateUiLabFeedbackError::GroupMismatch)
        );

        let unsupported_token = valid.replacen("\"footer_height\":24", "\"footer_height\":25", 1);
        assert_ne!(unsupported_token, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(unsupported_token.as_bytes()),
            Err(CandidateUiLabFeedbackError::InvalidVisualSpec)
        );

        let invalid_color = valid.replacen("\"red\":31", "\"red\":256", 1);
        assert_ne!(invalid_color, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(invalid_color.as_bytes()),
            Err(CandidateUiLabFeedbackError::InvalidValue)
        );
    }

    #[test]
    fn fields_semantics_trailing_data_and_input_size_are_strict() {
        let fixture = feedback_fixture();
        let valid = fixture.json;

        let unknown = valid.replacen("\"count\":2", "\"unknown\":2", 1);
        assert_ne!(unknown, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(unknown.as_bytes()),
            Err(CandidateUiLabFeedbackError::InvalidSyntax)
        );

        let hash = &fixture.baseline.visual_spec_sha256;
        let ordered = format!("{{\"variant\":\"baseline\",\"visual_spec_sha256\":\"{hash}\"");
        let reordered = format!("{{\"visual_spec_sha256\":\"{hash}\",\"variant\":\"baseline\"");
        let reordered = valid.replacen(&ordered, &reordered, 1);
        assert_ne!(reordered, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(reordered.as_bytes()),
            Err(CandidateUiLabFeedbackError::InvalidSyntax)
        );

        let invalid_semantic = valid.replacen(
            "\"semantic\":\"candidate.item\",\"candidate_index_zero_based\":0",
            "\"semantic\":\"candidate.item\",\"candidate_index_zero_based\":null",
            1,
        );
        assert_ne!(invalid_semantic, valid);
        assert_eq!(
            parse_candidate_ui_lab_feedback(invalid_semantic.as_bytes()),
            Err(CandidateUiLabFeedbackError::InvalidValue)
        );

        let mut trailing = valid.clone();
        trailing.push_str("private trailing candidate");
        assert_eq!(
            parse_candidate_ui_lab_feedback(trailing.as_bytes()),
            Err(CandidateUiLabFeedbackError::TrailingData)
        );
        assert_eq!(
            parse_candidate_ui_lab_feedback(&vec![b' '; MAX_CANDIDATE_UI_LAB_FEEDBACK_BYTES + 1]),
            Err(CandidateUiLabFeedbackError::TooLarge)
        );
        assert_eq!(
            parse_candidate_ui_lab_feedback(&[0xff]),
            Err(CandidateUiLabFeedbackError::InvalidUtf8)
        );
    }

    #[test]
    fn parser_errors_never_echo_rejected_notes_or_candidate_content() {
        let mut invalid = feedback_fixture().json;
        invalid = invalid.replacen("\"dpi\":96", "\"dpi\":97", 1);
        invalid.push_str("private candidate content");
        let error = parse_candidate_ui_lab_feedback(invalid.as_bytes()).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("绝不应出现在错误里的秘密"));
        assert!(!rendered.contains("private candidate content"));
    }

    #[test]
    fn cursor_decodes_supported_strings_and_rejects_noncanonical_numbers() {
        let mut string = Cursor::new("\"猫\\n\\t\\\"\\\\\\u54c7\"");
        assert_eq!(string.parse_string().unwrap(), "猫\n\t\"\\哇");
        assert!(string.is_finished());

        for invalid in ["\"\\ud800\"", "\"raw\ncontrol\"", "\"\\/\""] {
            assert_eq!(
                Cursor::new(invalid).parse_string(),
                Err(CandidateUiLabFeedbackError::InvalidSyntax)
            );
        }

        let mut zero = Cursor::new("0");
        assert_eq!(zero.parse_i32(), Ok(0));
        assert!(zero.is_finished());
        let mut negative = Cursor::new("-12");
        assert_eq!(negative.parse_i32(), Ok(-12));
        for invalid in ["01", "-0", "-01", "+1"] {
            assert_eq!(
                Cursor::new(invalid).parse_i32(),
                Err(CandidateUiLabFeedbackError::InvalidSyntax)
            );
        }
        assert_eq!(
            Cursor::new("999999999999999999999999").parse_i32(),
            Err(CandidateUiLabFeedbackError::InvalidValue)
        );
        assert_eq!(
            Cursor::new("01").parse_usize(),
            Err(CandidateUiLabFeedbackError::InvalidSyntax)
        );
    }
}
