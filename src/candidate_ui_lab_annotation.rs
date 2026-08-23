//! Bounded, deterministic annotations for the public-data-only candidate UI lab.
//!
//! This module does not open files, inspect TSF state, or retain candidate
//! text. Persistence is a separate explicit step; the first consumer keeps a
//! bounded session in memory.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::candidate_ui::{
    CandidateRgb, CandidateScene, CandidateSceneHit, CandidateSceneLayout, CandidateSceneRect,
    CandidateVisualSpec,
};

pub(crate) const CANDIDATE_UI_LAB_ANNOTATION_SCHEMA: &str = "candidate-ui-lab-annotation-v1";
pub(crate) const MAX_CANDIDATE_UI_LAB_ANNOTATIONS: usize = 64;
pub(crate) const MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS: usize = 240;
const MAX_CANDIDATE_UI_LAB_HITS: usize = 64;
const REVIEWED_DPIS: [u32; 4] = [96, 120, 144, 192];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabAnnotationContext {
    pub(crate) scenario_id: String,
    pub(crate) layout: CandidateSceneLayout,
    pub(crate) dpi: u32,
    pub(crate) selection: CandidateSceneRect,
    pub(crate) visual_spec_sha256: String,
    pub(crate) hits: Vec<CandidateSceneHit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabAnnotation {
    pub(crate) context: CandidateUiLabAnnotationContext,
    pub(crate) note: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabAnnotationError {
    InvalidScenario,
    InvalidDpi,
    SelectionOutsideScene,
    TooManySemanticHits,
    EmptyNote,
    NoteTooLong,
    UnsupportedNoteControl,
    SessionFull,
}

impl std::fmt::Display for CandidateUiLabAnnotationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidScenario => "公开场景标识无效",
            Self::InvalidDpi => "DPI 不在实验室固定集合中",
            Self::SelectionOutsideScene => "圈选区域没有落在候选窗内",
            Self::TooManySemanticHits => "圈选命中的语义区域超过固定上限",
            Self::EmptyNote => "批注不能为空",
            Self::NoteTooLong => "批注超过 240 个字符",
            Self::UnsupportedNoteControl => "批注包含不支持的控制字符",
            Self::SessionFull => "本次实验已达到 64 条批注上限",
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CandidateUiLabAnnotationSession {
    annotations: Vec<CandidateUiLabAnnotation>,
}

impl CandidateUiLabAnnotationSession {
    pub(crate) fn len(&self) -> usize {
        self.annotations.len()
    }

    pub(crate) fn annotations(&self) -> &[CandidateUiLabAnnotation] {
        &self.annotations
    }

    pub(crate) fn add(
        &mut self,
        context: CandidateUiLabAnnotationContext,
        note: &str,
    ) -> Result<(), CandidateUiLabAnnotationError> {
        if self.annotations.len() >= MAX_CANDIDATE_UI_LAB_ANNOTATIONS {
            return Err(CandidateUiLabAnnotationError::SessionFull);
        }
        let note = normalize_note(note)?;
        self.annotations
            .push(CandidateUiLabAnnotation { context, note });
        Ok(())
    }
}

pub(crate) fn capture_candidate_ui_lab_annotation_context(
    scenario_id: &str,
    layout: CandidateSceneLayout,
    dpi: u32,
    selection: CandidateSceneRect,
    scene: &CandidateScene,
    spec: CandidateVisualSpec,
) -> Result<CandidateUiLabAnnotationContext, CandidateUiLabAnnotationError> {
    if !valid_scenario_id(scenario_id) {
        return Err(CandidateUiLabAnnotationError::InvalidScenario);
    }
    if !REVIEWED_DPIS.contains(&dpi) {
        return Err(CandidateUiLabAnnotationError::InvalidDpi);
    }
    let selection = clip_rect(selection, scene.client)
        .ok_or(CandidateUiLabAnnotationError::SelectionOutsideScene)?;
    let hits = scene.semantic_hits_in(selection);
    if hits.len() > MAX_CANDIDATE_UI_LAB_HITS {
        return Err(CandidateUiLabAnnotationError::TooManySemanticHits);
    }
    Ok(CandidateUiLabAnnotationContext {
        scenario_id: scenario_id.to_owned(),
        layout,
        dpi,
        selection,
        visual_spec_sha256: candidate_visual_spec_sha256(spec),
        hits,
    })
}

impl CandidateUiLabAnnotation {
    pub(crate) fn to_canonical_json(&self) -> String {
        let context = &self.context;
        let layout = match context.layout {
            CandidateSceneLayout::Horizontal => "horizontal",
            CandidateSceneLayout::Vertical => "vertical",
        };
        let mut output = String::new();
        let _ = write!(
            output,
            "{{\"schema\":\"{}\",\"scenario\":\"{}\",\"layout\":\"{}\",\"dpi\":{},\"selection\":{{\"left\":{},\"top\":{},\"right\":{},\"bottom\":{}}},\"visual_spec_sha256\":\"{}\",\"hits\":[",
            CANDIDATE_UI_LAB_ANNOTATION_SCHEMA,
            json_escape(&context.scenario_id),
            layout,
            context.dpi,
            context.selection.left,
            context.selection.top,
            context.selection.right,
            context.selection.bottom,
            context.visual_spec_sha256,
        );
        for (index, hit) in context.hits.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            let candidate_index = hit
                .candidate_index
                .map_or_else(|| "null".to_owned(), |index| index.to_string());
            let _ = write!(
                output,
                "{{\"semantic\":\"{}\",\"candidate_index_zero_based\":{},\"bounds\":{{\"left\":{},\"top\":{},\"right\":{},\"bottom\":{}}}}}",
                hit.semantic.stable_id(),
                candidate_index,
                hit.bounds.left,
                hit.bounds.top,
                hit.bounds.right,
                hit.bounds.bottom,
            );
        }
        let _ = write!(output, "],\"note\":\"{}\"}}", json_escape(&self.note));
        output
    }
}

fn valid_scenario_id(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn clip_rect(
    rectangle: CandidateSceneRect,
    client: CandidateSceneRect,
) -> Option<CandidateSceneRect> {
    let clipped = CandidateSceneRect {
        left: rectangle.left.max(client.left),
        top: rectangle.top.max(client.top),
        right: rectangle.right.min(client.right),
        bottom: rectangle.bottom.min(client.bottom),
    };
    (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
}

fn normalize_note(note: &str) -> Result<String, CandidateUiLabAnnotationError> {
    let normalized = note.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return Err(CandidateUiLabAnnotationError::EmptyNote);
    }
    if normalized.chars().count() > MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS {
        return Err(CandidateUiLabAnnotationError::NoteTooLong);
    }
    if normalized
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(CandidateUiLabAnnotationError::UnsupportedNoteControl);
    }
    Ok(normalized.to_owned())
}

fn candidate_visual_spec_sha256(spec: CandidateVisualSpec) -> String {
    // Keep this destructure exhaustive so a newly added visual token cannot be
    // silently omitted from the annotation binding.
    let CandidateVisualSpec {
        outer_padding,
        row_height,
        text_padding,
        selected_text_inset,
        rank_width,
        rank_gap,
        footer_content_inset,
        horizontal_max_width,
        horizontal_min_width,
        horizontal_min_item_width,
        horizontal_text_max_width,
        horizontal_selected_text_max_width,
        vertical_min_width,
        vertical_text_max_width,
        vertical_max_width,
        vertical_rounding_slack,
        action_min_width,
        action_detail_gap,
        notice_icon_size,
        notice_icon_gap,
        corner_diameter,
        border_width,
        selected_surface_height,
        selected_surface_left_inset,
        selected_surface_right_inset,
        selected_surface_corner_diameter,
        selection_accent_width,
        selection_accent_fallback_height,
        selection_accent_left_inset,
        selection_accent_corner_diameter,
        personal_mark_size,
        footer_height,
        footer_vertical_inset,
        footer_divider_inset,
        footer_divider_width,
        footer_page_width,
        footer_mode_gap,
        candidate_font_height,
        metadata_font_height,
        background,
        selected_background,
        selected_text,
        candidate_text,
        selected_rank,
        rank,
        page,
        selection_accent,
        mode_accent,
        border,
        footer_divider,
    } = spec;
    let mut hasher = Sha256::new();
    hasher.update(b"ziranma-candidate-visual-spec-v1\0");
    for value in [
        outer_padding,
        row_height,
        text_padding,
        selected_text_inset,
        rank_width,
        rank_gap,
        footer_content_inset,
        horizontal_max_width,
        horizontal_min_width,
        horizontal_min_item_width,
        horizontal_text_max_width,
        horizontal_selected_text_max_width,
        vertical_min_width,
        vertical_text_max_width,
        vertical_max_width,
        vertical_rounding_slack,
        action_min_width,
        action_detail_gap,
        notice_icon_size,
        notice_icon_gap,
        corner_diameter,
        border_width,
        selected_surface_height,
        selected_surface_left_inset,
        selected_surface_right_inset,
        selected_surface_corner_diameter,
        selection_accent_width,
        selection_accent_fallback_height,
        selection_accent_left_inset,
        selection_accent_corner_diameter,
        personal_mark_size,
        footer_height,
        footer_vertical_inset,
        footer_divider_inset,
        footer_divider_width,
        footer_page_width,
        footer_mode_gap,
        candidate_font_height,
        metadata_font_height,
    ] {
        hasher.update(value.to_le_bytes());
    }
    for color in [
        background,
        selected_background,
        selected_text,
        candidate_text,
        selected_rank,
        rank,
        page,
        selection_accent,
        mode_accent,
        border,
        footer_divider,
    ] {
        update_color(&mut hasher, color);
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn update_color(hasher: &mut Sha256, color: CandidateRgb) {
    hasher.update([color.red, color.green, color.blue]);
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                let _ = write!(output, "\\u{:04x}", u32::from(character));
            }
            character => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate_ui::{
        CandidateSceneRequest, DEFAULT_CANDIDATE_VISUAL_SPEC, build_candidate_scene,
    };

    fn test_scene() -> CandidateScene {
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

    #[test]
    fn context_clips_selection_and_binds_semantics_and_visual_tokens() {
        let scene = test_scene();
        let context = capture_candidate_ui_lab_annotation_context(
            "everyday",
            CandidateSceneLayout::Horizontal,
            96,
            CandidateSceneRect {
                left: -10,
                top: 10,
                right: 50,
                bottom: 30,
            },
            &scene,
            DEFAULT_CANDIDATE_VISUAL_SPEC,
        )
        .unwrap();
        assert_eq!(
            context.selection,
            CandidateSceneRect {
                left: 0,
                top: 10,
                right: 50,
                bottom: 30,
            }
        );
        assert!(!context.hits.is_empty());
        assert_eq!(context.visual_spec_sha256.len(), 64);

        let mut changed = DEFAULT_CANDIDATE_VISUAL_SPEC;
        changed.rank_gap += 1;
        let changed = capture_candidate_ui_lab_annotation_context(
            "everyday",
            CandidateSceneLayout::Horizontal,
            96,
            context.selection,
            &scene,
            changed,
        )
        .unwrap();
        assert_ne!(context.visual_spec_sha256, changed.visual_spec_sha256);
    }

    #[test]
    fn annotation_json_is_canonical_bounded_and_contains_no_candidate_text() {
        let scene = test_scene();
        let context = capture_candidate_ui_lab_annotation_context(
            "long-candidate",
            CandidateSceneLayout::Horizontal,
            144,
            CandidateSceneRect {
                left: 20,
                top: 14,
                right: 60,
                bottom: 31,
            },
            &scene,
            DEFAULT_CANDIDATE_VISUAL_SPEC,
        )
        .unwrap();
        let mut session = CandidateUiLabAnnotationSession::default();
        session
            .add(context, "  字号\r\n想再清楚一点 \"呀\"  ")
            .unwrap();
        let json = session.annotations()[0].to_canonical_json();
        assert!(json.starts_with("{\"schema\":\"candidate-ui-lab-annotation-v1\""));
        assert!(json.contains("\"scenario\":\"long-candidate\""));
        assert!(json.contains("字号\\n想再清楚一点 \\\"呀\\\""));
        assert!(!json.contains("春风"));
        assert!(!json.contains("\r"));
    }

    #[test]
    fn invalid_context_notes_and_capacity_fail_without_partial_mutation() {
        let scene = test_scene();
        assert_eq!(
            capture_candidate_ui_lab_annotation_context(
                "Bad Scenario",
                CandidateSceneLayout::Horizontal,
                96,
                scene.client,
                &scene,
                DEFAULT_CANDIDATE_VISUAL_SPEC,
            ),
            Err(CandidateUiLabAnnotationError::InvalidScenario)
        );
        assert_eq!(
            capture_candidate_ui_lab_annotation_context(
                "everyday",
                CandidateSceneLayout::Horizontal,
                110,
                scene.client,
                &scene,
                DEFAULT_CANDIDATE_VISUAL_SPEC,
            ),
            Err(CandidateUiLabAnnotationError::InvalidDpi)
        );
        assert_eq!(
            capture_candidate_ui_lab_annotation_context(
                "everyday",
                CandidateSceneLayout::Horizontal,
                96,
                CandidateSceneRect {
                    left: scene.client.right + 1,
                    top: scene.client.bottom + 1,
                    right: scene.client.right + 4,
                    bottom: scene.client.bottom + 4,
                },
                &scene,
                DEFAULT_CANDIDATE_VISUAL_SPEC,
            ),
            Err(CandidateUiLabAnnotationError::SelectionOutsideScene)
        );
        let context = capture_candidate_ui_lab_annotation_context(
            "everyday",
            CandidateSceneLayout::Horizontal,
            96,
            scene.client,
            &scene,
            DEFAULT_CANDIDATE_VISUAL_SPEC,
        )
        .unwrap();
        let mut session = CandidateUiLabAnnotationSession::default();
        assert_eq!(
            session.add(context.clone(), " \n "),
            Err(CandidateUiLabAnnotationError::EmptyNote)
        );
        assert_eq!(session.len(), 0);
        assert_eq!(
            session.add(
                context.clone(),
                &"甲".repeat(MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS + 1)
            ),
            Err(CandidateUiLabAnnotationError::NoteTooLong)
        );
        for index in 0..MAX_CANDIDATE_UI_LAB_ANNOTATIONS {
            session
                .add(context.clone(), &format!("批注 {index}"))
                .unwrap();
        }
        assert_eq!(
            session.add(context, "再加一条"),
            Err(CandidateUiLabAnnotationError::SessionFull)
        );
        assert_eq!(session.len(), MAX_CANDIDATE_UI_LAB_ANNOTATIONS);
    }

    #[test]
    fn note_limit_counts_unicode_characters_and_rejects_other_controls() {
        let scene = test_scene();
        let context = capture_candidate_ui_lab_annotation_context(
            "everyday",
            CandidateSceneLayout::Horizontal,
            96,
            scene.client,
            &scene,
            DEFAULT_CANDIDATE_VISUAL_SPEC,
        )
        .unwrap();
        let mut session = CandidateUiLabAnnotationSession::default();
        session
            .add(
                context.clone(),
                &"😺".repeat(MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS),
            )
            .unwrap();
        assert_eq!(
            session.annotations()[0].note.chars().count(),
            MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS
        );
        assert_eq!(
            session.add(context, "这里包含\u{000b}垂直制表符"),
            Err(CandidateUiLabAnnotationError::UnsupportedNoteControl)
        );
        assert_eq!(session.len(), 1);
    }
}
