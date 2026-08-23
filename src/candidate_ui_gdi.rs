//! Shared Windows GDI painter for an already measured candidate scene.
//!
//! Window creation, TSF state, double buffering, font lifetime, and text
//! measurement remain with the caller. This module consumes only the shared
//! scene plus bounded display strings, so the production popup and a future
//! native lab can render through the same drawing path.

use crate::candidate_ui::{
    CandidateRgb, CandidateScene, CandidateSceneItem, CandidateSceneRect, CandidateVisualSpec,
    candidate_ui_scale,
};
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, CreateSolidBrush, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_TOP, DT_VCENTER, DeleteObject, DrawTextW, FillRect, FillRgn, GetTextExtentPoint32W, HDC,
    HFONT, HGDIOBJ, SelectObject, SetTextColor,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON};

#[derive(Clone, Copy)]
pub(crate) struct CandidateSceneFonts {
    pub(crate) candidate: HFONT,
    pub(crate) selected: HFONT,
    pub(crate) metadata: HFONT,
}

pub(crate) struct CandidateScenePaintContent<'a> {
    pub(crate) candidates: &'a [String],
    pub(crate) action_detail: Option<&'a str>,
    pub(crate) mode_label: Option<&'a str>,
    pub(crate) page_label: Option<&'a str>,
    pub(crate) notice_icon: Option<HICON>,
    pub(crate) max_candidate_characters: usize,
}

fn scene_rect(rect: CandidateSceneRect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn color_ref(color: CandidateRgb) -> COLORREF {
    COLORREF(u32::from(color.red) | (u32::from(color.green) << 8) | (u32::from(color.blue) << 16))
}

unsafe fn fill_rounded_rect(hdc: HDC, rectangle: RECT, diameter: i32, color: CandidateRgb) {
    if rectangle.right <= rectangle.left || rectangle.bottom <= rectangle.top {
        return;
    }
    let region = unsafe {
        CreateRoundRectRgn(
            rectangle.left,
            rectangle.top,
            rectangle.right.saturating_add(1),
            rectangle.bottom.saturating_add(1),
            diameter,
            diameter,
        )
    };
    let brush = unsafe { CreateSolidBrush(color_ref(color)) };
    if !region.is_invalid() && !brush.is_invalid() {
        unsafe {
            let _ = FillRgn(hdc, region, brush);
        }
    }
    if !brush.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
    if !region.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

unsafe fn paint_personal_mark(hdc: HDC, mark: CandidateSceneRect, color: CandidateRgb) {
    let mark = scene_rect(mark);
    let diameter = mark.right.saturating_sub(mark.left).max(1);
    let region = unsafe {
        CreateRoundRectRgn(
            mark.left,
            mark.top,
            mark.right,
            mark.bottom,
            diameter,
            diameter,
        )
    };
    if region.is_invalid() {
        return;
    }
    let brush = unsafe { CreateSolidBrush(color_ref(color)) };
    if !brush.is_invalid() {
        unsafe {
            let _ = FillRgn(hdc, region, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
    unsafe {
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
}

fn text_for_width(
    candidate: &str,
    maximum_width: i32,
    max_characters: usize,
    mut measure: impl FnMut(&str) -> Option<i32>,
) -> String {
    let mut source = candidate.chars();
    let characters = source.by_ref().take(max_characters).collect::<Vec<_>>();
    let source_truncated = source.next().is_some();
    let mut full = characters.iter().collect::<String>();
    if source_truncated {
        full.push('…');
    }
    let Some(full_width) = measure(&full) else {
        return full;
    };
    if characters.is_empty() || full_width <= maximum_width {
        return full;
    }

    let mut lower = 0;
    let mut upper = characters.len();
    while lower < upper {
        let middle = lower + (upper - lower).div_ceil(2);
        let mut trial = characters[..middle].iter().collect::<String>();
        trial.push('…');
        let Some(trial_width) = measure(&trial) else {
            return full;
        };
        if trial_width <= maximum_width {
            lower = middle;
        } else {
            upper = middle - 1;
        }
    }
    let mut clipped = characters[..lower].iter().collect::<String>();
    clipped.push('…');
    clipped
}

unsafe fn popup_text(
    hdc: HDC,
    candidate: &str,
    maximum_width: i32,
    max_characters: usize,
) -> Vec<u16> {
    text_for_width(candidate, maximum_width, max_characters, |text| {
        let encoded = text.encode_utf16().collect::<Vec<_>>();
        let mut size = SIZE::default();
        unsafe { GetTextExtentPoint32W(hdc, &encoded, &mut size).as_bool() }.then_some(size.cx)
    })
    .encode_utf16()
    .collect()
}

unsafe fn paint_label(
    hdc: HDC,
    spec: CandidateVisualSpec,
    item: &CandidateSceneItem,
    candidate: &str,
    action_detail: Option<&str>,
    fonts: CandidateSceneFonts,
    max_candidate_characters: usize,
) {
    let selected = item.index == 0;
    let mut rank = scene_rect(item.metadata_line);
    let mut text_bounds = scene_rect(item.text);
    let text_font = if selected {
        fonts.selected
    } else {
        fonts.candidate
    };
    if item.rank.is_some() {
        let mut rank_label = (item.index + 1)
            .to_string()
            .encode_utf16()
            .collect::<Vec<_>>();
        if !fonts.metadata.is_invalid() {
            unsafe {
                let _ = SelectObject(hdc, HGDIOBJ(fonts.metadata.0));
            }
        }
        unsafe {
            let _ = SetTextColor(
                hdc,
                color_ref(if selected {
                    spec.selected_rank
                } else {
                    spec.rank
                }),
            );
            let _ = DrawTextW(
                hdc,
                &mut rank_label,
                &mut rank,
                DT_RIGHT
                    | DT_SINGLELINE
                    | if item.baseline_aligned {
                        DT_TOP
                    } else {
                        DT_VCENTER
                    }
                    | DT_NOPREFIX,
            );
        }
        if let Some(mark) = item.personal_mark {
            unsafe {
                paint_personal_mark(hdc, mark, spec.mode_accent);
            }
        }
    }

    if !text_font.is_invalid() {
        unsafe {
            let _ = SelectObject(hdc, HGDIOBJ(text_font.0));
        }
    }
    let mut text = unsafe {
        popup_text(
            hdc,
            candidate,
            text_bounds.right.saturating_sub(text_bounds.left),
            max_candidate_characters,
        )
    };
    unsafe {
        let _ = SetTextColor(
            hdc,
            color_ref(if selected {
                spec.selected_text
            } else {
                spec.candidate_text
            }),
        );
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut text_bounds,
            DT_SINGLELINE
                | if item.baseline_aligned {
                    DT_TOP
                } else {
                    DT_VCENTER
                }
                | DT_NOPREFIX
                | DT_END_ELLIPSIS,
        );
    }
    if let (Some(detail_bounds), Some(detail)) = (item.action_detail, action_detail) {
        let mut detail_bounds = scene_rect(detail_bounds);
        let mut detail = detail.encode_utf16().collect::<Vec<_>>();
        unsafe {
            if !fonts.metadata.is_invalid() {
                let _ = SelectObject(hdc, HGDIOBJ(fonts.metadata.0));
            }
            let _ = SetTextColor(hdc, color_ref(spec.rank));
            let _ = DrawTextW(
                hdc,
                &mut detail,
                &mut detail_bounds,
                DT_RIGHT
                    | DT_SINGLELINE
                    | if item.baseline_aligned {
                        DT_TOP
                    } else {
                        DT_VCENTER
                    }
                    | DT_NOPREFIX,
            );
        }
    }
}

/// Paints one complete candidate scene into the caller's current GDI target.
///
/// The caller owns the HDC, fonts, buffering, and icon handle. This function
/// creates and releases only frame-local brushes and regions.
pub(crate) unsafe fn paint_candidate_scene(
    hdc: HDC,
    dpi: u32,
    spec: CandidateVisualSpec,
    scene: &CandidateScene,
    fonts: CandidateSceneFonts,
    content: CandidateScenePaintContent<'_>,
) {
    let client = scene_rect(scene.client);
    let background = unsafe { CreateSolidBrush(color_ref(spec.background)) };
    if !background.is_invalid() {
        unsafe {
            let _ = FillRect(hdc, &client, background);
            let _ = DeleteObject(HGDIOBJ(background.0));
        }
    }

    for (item, candidate) in scene.items.iter().zip(content.candidates) {
        if let Some(selected) = item.selected_surface {
            unsafe {
                fill_rounded_rect(
                    hdc,
                    scene_rect(selected),
                    candidate_ui_scale(dpi, spec.selected_surface_corner_diameter),
                    spec.selected_background,
                );
            }
        }
        if let Some(accent) = item.selection_accent {
            unsafe {
                fill_rounded_rect(
                    hdc,
                    scene_rect(accent),
                    candidate_ui_scale(dpi, spec.selection_accent_corner_diameter),
                    spec.selection_accent,
                );
            }
        }
        if let (Some(bounds), Some(icon)) = (item.notice_icon, content.notice_icon) {
            let bounds = scene_rect(bounds);
            unsafe {
                let _ = DrawIconEx(
                    hdc,
                    bounds.left,
                    bounds.top,
                    icon,
                    bounds.right.saturating_sub(bounds.left).max(0),
                    bounds.bottom.saturating_sub(bounds.top).max(0),
                    0,
                    None,
                    DI_NORMAL,
                );
            }
        }
        unsafe {
            paint_label(
                hdc,
                spec,
                item,
                candidate,
                (item.index == 0).then_some(content.action_detail).flatten(),
                fonts,
                content.max_candidate_characters,
            );
        }
    }

    if let Some(divider) = scene.footer_divider {
        let divider = scene_rect(divider);
        let brush = unsafe { CreateSolidBrush(color_ref(spec.footer_divider)) };
        if !brush.is_invalid() {
            unsafe {
                let _ = FillRect(hdc, &divider, brush);
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
    }
    if let (Some(bounds), Some(label)) = (scene.footer_mode, content.mode_label) {
        let mut bounds = scene_rect(bounds);
        let mut label = label.encode_utf16().collect::<Vec<_>>();
        unsafe {
            if !fonts.metadata.is_invalid() {
                let _ = SelectObject(hdc, HGDIOBJ(fonts.metadata.0));
            }
            let _ = SetTextColor(hdc, color_ref(spec.mode_accent));
            let _ = DrawTextW(
                hdc,
                &mut label,
                &mut bounds,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
    }
    if let (Some(bounds), Some(label)) = (scene.footer_page, content.page_label) {
        let mut bounds = scene_rect(bounds);
        let mut label = label.encode_utf16().collect::<Vec<_>>();
        unsafe {
            if !fonts.metadata.is_invalid() {
                let _ = SelectObject(hdc, HGDIOBJ(fonts.metadata.0));
            }
            let _ = SetTextColor(hdc, color_ref(spec.page));
            let _ = DrawTextW(
                hdc,
                &mut label,
                &mut bounds,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_text_uses_one_ellipsis_and_preserves_measurement_failure() {
        let width = |text: &str| Some(i32::try_from(text.chars().count()).unwrap() * 10);
        assert_eq!(text_for_width("省略号", 30, 32, width), "省略号");
        assert_eq!(text_for_width("省略号", 20, 32, width), "省…");
        assert_eq!(text_for_width("省略号", 10, 32, width), "…");
        assert_eq!(text_for_width("省略号", 20, 32, |_| None), "省略号");

        let over_limit = "甲".repeat(33);
        let complete_prefix = text_for_width(&over_limit, 330, 32, width);
        assert_eq!(complete_prefix.chars().count(), 33);
        assert!(complete_prefix.ends_with('…'));
        let clipped_prefix = text_for_width(&over_limit, 320, 32, width);
        assert_eq!(clipped_prefix.chars().count(), 32);
        assert!(clipped_prefix.ends_with('…'));
    }
}
