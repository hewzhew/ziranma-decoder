//! Window-independent visual specification and geometry for the candidate UI.
//!
//! The production TSF popup and future public-data-only UI labs must share
//! these values and rectangles.  This module deliberately has no HWND, HDC,
//! font, or candidate-text dependency, so geometry stays deterministic and
//! testable on every supported build target.

/// One semantic RGB color used by the candidate surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateRgb {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
}

impl CandidateRgb {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

/// The reviewed production visual tokens, expressed at 96 DPI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateVisualSpec {
    pub(crate) outer_padding: i32,
    pub(crate) row_height: i32,
    pub(crate) text_padding: i32,
    pub(crate) selected_text_inset: i32,
    pub(crate) rank_width: i32,
    pub(crate) rank_gap: i32,
    pub(crate) footer_content_inset: i32,
    pub(crate) horizontal_max_width: i32,
    pub(crate) horizontal_min_width: i32,
    pub(crate) horizontal_min_item_width: i32,
    pub(crate) horizontal_text_max_width: i32,
    pub(crate) horizontal_selected_text_max_width: i32,
    pub(crate) vertical_min_width: i32,
    pub(crate) vertical_text_max_width: i32,
    pub(crate) vertical_max_width: i32,
    pub(crate) vertical_rounding_slack: i32,
    pub(crate) action_min_width: i32,
    pub(crate) action_detail_gap: i32,
    pub(crate) notice_icon_size: i32,
    pub(crate) notice_icon_gap: i32,
    pub(crate) corner_diameter: i32,
    pub(crate) border_width: i32,
    pub(crate) selected_surface_height: i32,
    pub(crate) selected_surface_left_inset: i32,
    pub(crate) selected_surface_right_inset: i32,
    pub(crate) selection_accent_width: i32,
    pub(crate) selection_accent_fallback_height: i32,
    pub(crate) selection_accent_left_inset: i32,
    pub(crate) personal_mark_size: i32,
    pub(crate) footer_height: i32,
    pub(crate) footer_vertical_inset: i32,
    pub(crate) footer_divider_inset: i32,
    pub(crate) footer_divider_width: i32,
    pub(crate) footer_page_width: i32,
    pub(crate) footer_mode_gap: i32,
    pub(crate) candidate_font_height: i32,
    pub(crate) metadata_font_height: i32,
    pub(crate) background: CandidateRgb,
    pub(crate) selected_background: CandidateRgb,
    pub(crate) selected_text: CandidateRgb,
    pub(crate) candidate_text: CandidateRgb,
    pub(crate) selected_rank: CandidateRgb,
    pub(crate) rank: CandidateRgb,
    pub(crate) page: CandidateRgb,
    pub(crate) selection_accent: CandidateRgb,
    pub(crate) mode_accent: CandidateRgb,
    pub(crate) border: CandidateRgb,
    pub(crate) footer_divider: CandidateRgb,
}

/// The single reviewed visual specification compiled into the production IME.
pub(crate) const DEFAULT_CANDIDATE_VISUAL_SPEC: CandidateVisualSpec = CandidateVisualSpec {
    outer_padding: 5,
    row_height: 36,
    text_padding: 7,
    selected_text_inset: 13,
    rank_width: 16,
    rank_gap: 4,
    footer_content_inset: 10,
    horizontal_max_width: 760,
    horizontal_min_width: 280,
    horizontal_min_item_width: 70,
    horizontal_text_max_width: 144,
    horizontal_selected_text_max_width: 288,
    vertical_min_width: 360,
    vertical_text_max_width: 594,
    vertical_max_width: 660,
    vertical_rounding_slack: 2,
    action_min_width: 210,
    action_detail_gap: 12,
    notice_icon_size: 24,
    notice_icon_gap: 7,
    corner_diameter: 16,
    border_width: 1,
    selected_surface_height: 28,
    selected_surface_left_inset: 1,
    selected_surface_right_inset: 5,
    selection_accent_width: 3,
    selection_accent_fallback_height: 14,
    selection_accent_left_inset: 5,
    personal_mark_size: 3,
    footer_height: 24,
    footer_vertical_inset: 2,
    footer_divider_inset: 7,
    footer_divider_width: 1,
    footer_page_width: 48,
    footer_mode_gap: 4,
    candidate_font_height: 17,
    metadata_font_height: 14,
    background: CandidateRgb::new(31, 32, 35),
    selected_background: CandidateRgb::new(44, 47, 53),
    selected_text: CandidateRgb::new(250, 251, 253),
    candidate_text: CandidateRgb::new(218, 223, 230),
    selected_rank: CandidateRgb::new(198, 205, 215),
    rank: CandidateRgb::new(143, 151, 164),
    page: CandidateRgb::new(130, 139, 153),
    selection_accent: CandidateRgb::new(72, 180, 232),
    mode_accent: CandidateRgb::new(147, 184, 241),
    border: CandidateRgb::new(58, 62, 69),
    footer_divider: CandidateRgb::new(66, 70, 78),
};

/// Candidate arrangement chosen before the scene is constructed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CandidateSceneLayout {
    #[default]
    Horizontal,
    Vertical,
}

/// A physical-pixel rectangle with right and bottom edges excluded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CandidateSceneRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

impl CandidateSceneRect {
    fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }
}

/// Stable semantic identities exposed by the shared candidate scene.
// The first production consumer needs geometry; the future native lab will
// consume these names for hit testing and local annotations.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateSceneSemantic {
    CandidateItem,
    CandidateSelectedSurface,
    SelectionAccent,
    Footer,
    FooterDivider,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CandidateSceneSemantic {
    pub(crate) const fn stable_id(self) -> &'static str {
        match self {
            Self::CandidateItem => "candidate.item",
            Self::CandidateSelectedSurface => "candidate.selected.surface",
            Self::SelectionAccent => "selection.accent",
            Self::Footer => "footer",
            Self::FooterDivider => "footer.divider",
        }
    }
}

/// One candidate item's shared geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSceneItem {
    pub(crate) index: usize,
    pub(crate) bounds: CandidateSceneRect,
    pub(crate) selected_surface: Option<CandidateSceneRect>,
    pub(crate) selection_accent: Option<CandidateSceneRect>,
}

/// Shared geometry consumed by the production GDI painter and future labs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateScene {
    pub(crate) client: CandidateSceneRect,
    pub(crate) items: Vec<CandidateSceneItem>,
    pub(crate) footer: Option<CandidateSceneRect>,
    pub(crate) footer_divider: Option<CandidateSceneRect>,
}

/// Fixed inputs that contain no candidate text or platform window handles.
pub(crate) struct CandidateSceneRequest<'a> {
    pub(crate) layout: CandidateSceneLayout,
    pub(crate) dpi: u32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) candidate_count: usize,
    /// Exact physical widths selected by the horizontal allocation policy.
    /// Vertical scenes ignore this slice.
    pub(crate) horizontal_candidate_widths: &'a [i32],
    pub(crate) footer_width: i32,
    pub(crate) footer_needed: bool,
    pub(crate) selected_surface: bool,
    /// The selected font's physical-pixel height, when GDI measured it.
    pub(crate) selected_text_height: Option<i32>,
}

/// Scales a reviewed 96-DPI token to physical pixels with deterministic
/// nearest-integer rounding.
pub(crate) fn candidate_ui_scale(dpi: u32, logical: i32) -> i32 {
    i32::try_from(
        i64::from(logical)
            .saturating_mul(i64::from(dpi.max(96)))
            .saturating_add(48)
            / 96,
    )
    .unwrap_or(i32::MAX)
}

fn centered_vertical(bounds: CandidateSceneRect, height: i32) -> (i32, i32) {
    let height = height.clamp(0, bounds.height());
    let top = bounds
        .top
        .saturating_add(bounds.height().saturating_sub(height) / 2);
    (top, top.saturating_add(height))
}

fn selected_geometry(
    spec: CandidateVisualSpec,
    dpi: u32,
    item: CandidateSceneRect,
    selected_text_height: Option<i32>,
) -> (CandidateSceneRect, CandidateSceneRect) {
    let scale = |logical| candidate_ui_scale(dpi, logical);
    let (selected_top, selected_bottom) =
        centered_vertical(item, scale(spec.selected_surface_height));
    let selected = CandidateSceneRect {
        left: item
            .left
            .saturating_add(scale(spec.selected_surface_left_inset)),
        top: selected_top,
        right: item
            .right
            .saturating_sub(scale(spec.selected_surface_right_inset)),
        bottom: selected_bottom,
    };
    let accent_height =
        selected_text_height.unwrap_or_else(|| scale(spec.selection_accent_fallback_height));
    let (accent_top, accent_bottom) = centered_vertical(item, accent_height);
    let accent_left = selected
        .left
        .saturating_add(scale(spec.selection_accent_left_inset));
    let accent = CandidateSceneRect {
        left: accent_left,
        top: accent_top,
        right: accent_left.saturating_add(scale(spec.selection_accent_width)),
        bottom: accent_bottom,
    };
    (selected, accent)
}

/// Builds the complete candidate-item, selection, and footer rectangles used
/// by the production popup. Invalid or inconsistent dimensions fail closed.
pub(crate) fn build_candidate_scene(
    spec: CandidateVisualSpec,
    request: CandidateSceneRequest<'_>,
) -> Option<CandidateScene> {
    if request.width <= 0
        || request.height <= 0
        || (request.layout == CandidateSceneLayout::Horizontal
            && (request.horizontal_candidate_widths.len() != request.candidate_count
                || request
                    .horizontal_candidate_widths
                    .iter()
                    .any(|width| *width <= 0)
                || (request.footer_needed && request.footer_width <= 0)))
    {
        return None;
    }
    let scale = |logical| candidate_ui_scale(request.dpi, logical);
    let client = CandidateSceneRect {
        left: 0,
        top: 0,
        right: request.width,
        bottom: request.height,
    };
    let padding = scale(spec.outer_padding);
    let row_height = scale(spec.row_height);
    let mut items = Vec::with_capacity(request.candidate_count);
    let mut horizontal_left = padding;
    for index in 0..request.candidate_count {
        let top = match request.layout {
            CandidateSceneLayout::Horizontal => padding,
            CandidateSceneLayout::Vertical => padding.saturating_add(
                row_height.saturating_mul(i32::try_from(index).unwrap_or(i32::MAX)),
            ),
        };
        let bounds = match request.layout {
            CandidateSceneLayout::Horizontal => {
                let width = request.horizontal_candidate_widths[index];
                let bounds = CandidateSceneRect {
                    left: horizontal_left,
                    top,
                    right: horizontal_left.saturating_add(width),
                    bottom: top.saturating_add(row_height),
                };
                horizontal_left = bounds.right;
                bounds
            }
            CandidateSceneLayout::Vertical => CandidateSceneRect {
                left: padding,
                top,
                right: client.right.saturating_sub(padding),
                bottom: top.saturating_add(row_height),
            },
        };
        if bounds.width() <= 0
            || bounds.height() <= 0
            || bounds.left < client.left
            || bounds.top < client.top
            || bounds.right > client.right
            || bounds.bottom > client.bottom
        {
            return None;
        }
        let (selected_surface, selection_accent) = if index == 0 && request.selected_surface {
            let (selected, accent) =
                selected_geometry(spec, request.dpi, bounds, request.selected_text_height);
            (Some(selected), Some(accent))
        } else {
            (None, None)
        };
        items.push(CandidateSceneItem {
            index,
            bounds,
            selected_surface,
            selection_accent,
        });
    }

    let candidate_right_limit =
        if request.footer_needed && request.layout == CandidateSceneLayout::Horizontal {
            client.right.saturating_sub(request.footer_width)
        } else {
            client.right.saturating_sub(padding)
        };
    if request.layout == CandidateSceneLayout::Horizontal
        && items
            .last()
            .is_some_and(|item| item.bounds.right > candidate_right_limit)
    {
        return None;
    }

    let (footer, footer_divider) = if request.footer_needed {
        let footer = match request.layout {
            CandidateSceneLayout::Horizontal => CandidateSceneRect {
                left: client.right.saturating_sub(request.footer_width),
                top: padding,
                right: client.right.saturating_sub(scale(spec.text_padding)),
                bottom: padding.saturating_add(row_height),
            },
            CandidateSceneLayout::Vertical => CandidateSceneRect {
                left: padding,
                top: padding
                    .saturating_add(row_height.saturating_mul(
                        i32::try_from(request.candidate_count).unwrap_or(i32::MAX),
                    )),
                right: client.right.saturating_sub(scale(spec.text_padding)),
                bottom: client
                    .bottom
                    .saturating_sub(scale(spec.footer_vertical_inset)),
            },
        };
        let divider = match request.layout {
            CandidateSceneLayout::Horizontal => CandidateSceneRect {
                left: footer.left,
                top: footer.top.saturating_add(scale(spec.footer_divider_inset)),
                right: footer.left.saturating_add(scale(spec.footer_divider_width)),
                bottom: footer
                    .bottom
                    .saturating_sub(scale(spec.footer_divider_inset)),
            },
            CandidateSceneLayout::Vertical => CandidateSceneRect {
                left: footer.left,
                top: footer.top,
                right: footer.right,
                bottom: footer.top.saturating_add(scale(spec.footer_divider_width)),
            },
        };
        let footer = match request.layout {
            CandidateSceneLayout::Horizontal => CandidateSceneRect {
                left: footer.left.saturating_add(scale(spec.footer_content_inset)),
                ..footer
            },
            CandidateSceneLayout::Vertical => CandidateSceneRect {
                top: footer.top.saturating_add(scale(spec.footer_vertical_inset)),
                ..footer
            },
        };
        if divider.width() <= 0
            || divider.height() <= 0
            || footer.width() <= 0
            || footer.height() <= 0
            || footer.left < client.left
            || footer.top < client.top
            || footer.right > client.right
            || footer.bottom > client.bottom
        {
            return None;
        }
        (Some(footer), Some(divider))
    } else {
        (None, None)
    };

    Some(CandidateScene {
        client,
        items,
        footer,
        footer_divider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scene_tokens_keep_stable_semantic_names() {
        assert_eq!(
            CandidateSceneSemantic::CandidateSelectedSurface.stable_id(),
            "candidate.selected.surface"
        );
        assert_eq!(
            CandidateSceneSemantic::SelectionAccent.stable_id(),
            "selection.accent"
        );
        assert_eq!(
            CandidateSceneSemantic::CandidateItem.stable_id(),
            "candidate.item"
        );
        assert_eq!(CandidateSceneSemantic::Footer.stable_id(), "footer");
        assert_eq!(
            CandidateSceneSemantic::FooterDivider.stable_id(),
            "footer.divider"
        );
    }

    #[test]
    fn horizontal_scene_matches_the_reviewed_production_rectangles() {
        let scene = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 480,
                height: 46,
                candidate_count: 2,
                horizontal_candidate_widths: &[100, 120],
                footer_width: 62,
                footer_needed: true,
                selected_surface: true,
                selected_text_height: Some(17),
            },
        )
        .unwrap();

        assert_eq!(
            scene.items[0].bounds,
            CandidateSceneRect {
                left: 5,
                top: 5,
                right: 105,
                bottom: 41,
            }
        );
        assert_eq!(
            scene.items[1].bounds,
            CandidateSceneRect {
                left: 105,
                top: 5,
                right: 225,
                bottom: 41,
            }
        );
        assert_eq!(
            scene.items[0].selected_surface,
            Some(CandidateSceneRect {
                left: 6,
                top: 9,
                right: 100,
                bottom: 37,
            })
        );
        assert_eq!(
            scene.items[0].selection_accent,
            Some(CandidateSceneRect {
                left: 11,
                top: 14,
                right: 14,
                bottom: 31,
            })
        );
        assert_eq!(
            scene.footer_divider,
            Some(CandidateSceneRect {
                left: 418,
                top: 12,
                right: 419,
                bottom: 34,
            })
        );
        assert_eq!(
            scene.footer,
            Some(CandidateSceneRect {
                left: 428,
                top: 5,
                right: 473,
                bottom: 41,
            })
        );
    }

    #[test]
    fn vertical_scene_scales_rows_and_places_footer_below_them() {
        let scene = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Vertical,
                dpi: 144,
                width: 540,
                height: 207,
                candidate_count: 3,
                horizontal_candidate_widths: &[],
                footer_width: 93,
                footer_needed: true,
                selected_surface: true,
                selected_text_height: None,
            },
        )
        .unwrap();

        assert_eq!(scene.items[0].bounds.top, 8);
        assert_eq!(scene.items[1].bounds.top, 62);
        assert_eq!(scene.items[2].bounds.top, 116);
        assert_eq!(scene.items[2].bounds.bottom, 170);
        assert_eq!(scene.footer_divider.unwrap().top, 170);
        assert_eq!(scene.footer.unwrap().top, 173);
        assert_eq!(scene.footer.unwrap().bottom, 204);
        assert!(scene.items[0].selection_accent.is_some());
    }

    #[test]
    fn inconsistent_or_nonpositive_scene_requests_fail_closed() {
        let invalid_widths = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 100,
                height: 46,
                candidate_count: 2,
                horizontal_candidate_widths: &[50],
                footer_width: 0,
                footer_needed: false,
                selected_surface: true,
                selected_text_height: None,
            },
        );
        assert!(invalid_widths.is_none());

        let invalid_size = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Vertical,
                dpi: 96,
                width: 0,
                height: 46,
                candidate_count: 1,
                horizontal_candidate_widths: &[],
                footer_width: 0,
                footer_needed: false,
                selected_surface: true,
                selected_text_height: None,
            },
        );
        assert!(invalid_size.is_none());

        let overlapping_footer = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 160,
                height: 46,
                candidate_count: 2,
                horizontal_candidate_widths: &[80, 80],
                footer_width: 62,
                footer_needed: true,
                selected_surface: true,
                selected_text_height: None,
            },
        );
        assert!(overlapping_footer.is_none());
    }
}
