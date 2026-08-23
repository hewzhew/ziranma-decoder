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
    pub(crate) selected_surface_corner_diameter: i32,
    pub(crate) selection_accent_width: i32,
    pub(crate) selection_accent_fallback_height: i32,
    pub(crate) selection_accent_left_inset: i32,
    pub(crate) selection_accent_corner_diameter: i32,
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
    selected_surface_corner_diameter: 6,
    selection_accent_width: 3,
    selection_accent_fallback_height: 14,
    selection_accent_left_inset: 5,
    selection_accent_corner_diameter: 4,
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

/// Physical font measurements supplied by the active renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSceneFontMetrics {
    pub(crate) height: i32,
    pub(crate) ascent: i32,
}

/// Physical widths measured for one selected action label and its detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSceneActionDetailMetrics {
    pub(crate) label_width: i32,
    pub(crate) detail_width: i32,
}

impl CandidateSceneRect {
    fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }

    fn contains(self, other: Self) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }

    fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
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
    CandidateRank,
    CandidateText,
    CandidateActionDetail,
    CandidatePersonalMark,
    NoticeIcon,
    SelectionAccent,
    Footer,
    FooterDivider,
    FooterMode,
    FooterPage,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CandidateSceneSemantic {
    pub(crate) const fn stable_id(self) -> &'static str {
        match self {
            Self::CandidateItem => "candidate.item",
            Self::CandidateSelectedSurface => "candidate.selected.surface",
            Self::CandidateRank => "candidate.rank",
            Self::CandidateText => "candidate.text",
            Self::CandidateActionDetail => "candidate.action-detail",
            Self::CandidatePersonalMark => "candidate.personal-mark",
            Self::NoticeIcon => "notice.icon",
            Self::SelectionAccent => "selection.accent",
            Self::Footer => "footer",
            Self::FooterDivider => "footer.divider",
            Self::FooterMode => "footer.mode",
            Self::FooterPage => "footer.page",
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
    pub(crate) rank: Option<CandidateSceneRect>,
    pub(crate) text: CandidateSceneRect,
    pub(crate) action_detail: Option<CandidateSceneRect>,
    pub(crate) metadata_line: CandidateSceneRect,
    pub(crate) personal_mark: Option<CandidateSceneRect>,
    pub(crate) notice_icon: Option<CandidateSceneRect>,
    pub(crate) baseline_aligned: bool,
}

/// Shared geometry consumed by the production GDI painter and future labs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateScene {
    pub(crate) client: CandidateSceneRect,
    pub(crate) items: Vec<CandidateSceneItem>,
    pub(crate) footer: Option<CandidateSceneRect>,
    pub(crate) footer_divider: Option<CandidateSceneRect>,
    pub(crate) footer_mode: Option<CandidateSceneRect>,
    pub(crate) footer_page: Option<CandidateSceneRect>,
}

/// One semantic region under a scene point. Candidate indices are present
/// only for regions that belong to a candidate item.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateSceneHit {
    pub(crate) semantic: CandidateSceneSemantic,
    pub(crate) candidate_index: Option<usize>,
    pub(crate) bounds: CandidateSceneRect,
}

#[cfg_attr(not(test), allow(dead_code))]
impl CandidateScene {
    /// Returns every semantic region at a physical-pixel point, ordered from
    /// the most specific painted feature to its enclosing container.
    ///
    /// Rectangles follow GDI's right-and-bottom-exclusive convention. The
    /// stable order makes overlapping regions deterministic for future native
    /// inspection and annotation tools without changing production input.
    pub(crate) fn semantic_hits_at(&self, x: i32, y: i32) -> Vec<CandidateSceneHit> {
        if !self.client.contains_point(x, y) {
            return Vec::new();
        }

        let mut hits = Vec::new();
        let mut push = |semantic, candidate_index, bounds: Option<CandidateSceneRect>| {
            if let Some(bounds) = bounds.filter(|bounds| bounds.contains_point(x, y)) {
                hits.push(CandidateSceneHit {
                    semantic,
                    candidate_index,
                    bounds,
                });
            }
        };

        // Footer labels are painted after candidates. The current scene
        // builder keeps them disjoint, while this order also stays defined if
        // a future layout intentionally overlays a compact footer.
        push(CandidateSceneSemantic::FooterPage, None, self.footer_page);
        push(CandidateSceneSemantic::FooterMode, None, self.footer_mode);
        push(
            CandidateSceneSemantic::FooterDivider,
            None,
            self.footer_divider,
        );
        push(CandidateSceneSemantic::Footer, None, self.footer);

        for item in &self.items {
            if !item.bounds.contains_point(x, y) {
                continue;
            }
            let candidate_index = Some(item.index);
            push(
                CandidateSceneSemantic::CandidatePersonalMark,
                candidate_index,
                item.personal_mark,
            );
            push(
                CandidateSceneSemantic::CandidateActionDetail,
                candidate_index,
                item.action_detail,
            );
            push(
                CandidateSceneSemantic::NoticeIcon,
                candidate_index,
                item.notice_icon,
            );
            push(
                CandidateSceneSemantic::CandidateRank,
                candidate_index,
                item.rank,
            );
            push(
                CandidateSceneSemantic::CandidateText,
                candidate_index,
                Some(item.text),
            );
            push(
                CandidateSceneSemantic::SelectionAccent,
                candidate_index,
                item.selection_accent,
            );
            push(
                CandidateSceneSemantic::CandidateSelectedSurface,
                candidate_index,
                item.selected_surface,
            );
            push(
                CandidateSceneSemantic::CandidateItem,
                candidate_index,
                Some(item.bounds),
            );
            break;
        }
        hits
    }
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
    pub(crate) footer_mode: bool,
    pub(crate) footer_page: bool,
    pub(crate) selected_surface: bool,
    pub(crate) show_rank: bool,
    pub(crate) notice_icon: bool,
    pub(crate) personalized: &'a [bool],
    pub(crate) rank_metrics: Option<CandidateSceneFontMetrics>,
    pub(crate) candidate_text_metrics: Option<CandidateSceneFontMetrics>,
    pub(crate) selected_text_metrics: Option<CandidateSceneFontMetrics>,
    pub(crate) action_detail_metrics: Option<CandidateSceneActionDetailMetrics>,
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

fn label_columns(
    spec: CandidateVisualSpec,
    dpi: u32,
    content: CandidateSceneRect,
) -> (CandidateSceneRect, CandidateSceneRect) {
    let mut rank = content;
    rank.right = rank
        .left
        .saturating_add(candidate_ui_scale(dpi, spec.rank_width))
        .min(rank.right);
    let mut text = content;
    text.left = rank
        .right
        .saturating_add(candidate_ui_scale(dpi, spec.rank_gap));
    (rank, text)
}

fn label_rects(
    spec: CandidateVisualSpec,
    dpi: u32,
    content: CandidateSceneRect,
    rank_metrics: Option<CandidateSceneFontMetrics>,
    text_metrics: Option<CandidateSceneFontMetrics>,
) -> (CandidateSceneRect, CandidateSceneRect, bool) {
    let (mut rank, mut text) = label_columns(spec, dpi, content);
    let (Some(rank_metrics), Some(text_metrics)) = (rank_metrics, text_metrics) else {
        return (rank, text, false);
    };
    let available_height = content.height();
    let text_height = text_metrics.height.clamp(0, available_height);
    text.top = content
        .top
        .saturating_add(available_height.saturating_sub(text_height) / 2);
    text.bottom = text.top.saturating_add(text_height);
    let baseline = text
        .top
        .saturating_add(text_metrics.ascent.min(text_height));
    let rank_height = rank_metrics.height.clamp(0, available_height);
    rank.top = baseline
        .saturating_sub(rank_metrics.ascent.min(rank_height))
        .max(content.top);
    rank.bottom = rank.top.saturating_add(rank_height).min(content.bottom);
    (rank, text, true)
}

fn personal_mark_rect(
    spec: CandidateVisualSpec,
    dpi: u32,
    rank: CandidateSceneRect,
) -> Option<CandidateSceneRect> {
    let size = candidate_ui_scale(dpi, spec.personal_mark_size).max(2);
    if rank.height() < size || rank.width() < size {
        return None;
    }
    let top = rank
        .top
        .saturating_add(rank.height().saturating_sub(size) / 2);
    Some(CandidateSceneRect {
        left: rank.left,
        top,
        right: rank.left.saturating_add(size),
        bottom: top.saturating_add(size),
    })
}

fn selected_geometry(
    spec: CandidateVisualSpec,
    dpi: u32,
    item: CandidateSceneRect,
    selected_text_metrics: Option<CandidateSceneFontMetrics>,
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
    let accent_height = selected_text_metrics
        .map(|metrics| metrics.height)
        .unwrap_or_else(|| scale(spec.selection_accent_fallback_height));
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
    let footer_needed = request.footer_mode || request.footer_page;
    let invalid_metrics = [
        request.rank_metrics,
        request.candidate_text_metrics,
        request.selected_text_metrics,
    ]
    .into_iter()
    .flatten()
    .any(|metrics| metrics.height <= 0 || metrics.ascent < 0 || metrics.ascent > metrics.height);
    let invalid_action_detail = request
        .action_detail_metrics
        .is_some_and(|metrics| metrics.label_width <= 0 || metrics.detail_width <= 0);
    if request.width <= 0
        || request.height <= 0
        || (request.layout == CandidateSceneLayout::Horizontal
            && (request.horizontal_candidate_widths.len() != request.candidate_count
                || request
                    .horizontal_candidate_widths
                    .iter()
                    .any(|width| *width <= 0)
                || (footer_needed && request.footer_width <= 0)))
        || request.personalized.len() != request.candidate_count
        || invalid_metrics
        || invalid_action_detail
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
                selected_geometry(spec, request.dpi, bounds, request.selected_text_metrics);
            (Some(selected), Some(accent))
        } else {
            (None, None)
        };
        let notice_icon = if index == 0 && request.notice_icon {
            let size = scale(spec.notice_icon_size);
            let top = bounds
                .top
                .saturating_add(bounds.height().saturating_sub(size).max(0) / 2);
            Some(CandidateSceneRect {
                left: bounds.left.saturating_add(scale(spec.text_padding)),
                top,
                right: bounds
                    .left
                    .saturating_add(scale(spec.text_padding))
                    .saturating_add(size),
                bottom: top.saturating_add(size),
            })
        } else {
            None
        };
        let leading_inset = if index == 0 && request.selected_surface {
            scale(spec.selected_text_inset)
        } else {
            scale(spec.text_padding)
        };
        let notice_inset = notice_icon
            .map(|_| scale(spec.notice_icon_size.saturating_add(spec.notice_icon_gap)))
            .unwrap_or(0);
        let content = CandidateSceneRect {
            left: bounds
                .left
                .saturating_add(leading_inset)
                .saturating_add(notice_inset),
            top: bounds.top,
            right: bounds.right.saturating_sub(scale(spec.text_padding)),
            bottom: bounds.bottom,
        };
        if content.width() <= 0 || content.height() <= 0 {
            return None;
        }
        let text_metrics = if index == 0 {
            request.selected_text_metrics
        } else {
            request.candidate_text_metrics
        };
        let (metadata_line, mut text, baseline_aligned) = label_rects(
            spec,
            request.dpi,
            content,
            request.rank_metrics,
            text_metrics,
        );
        let rank = request.show_rank.then_some(metadata_line);
        if rank.is_none() {
            text.left = content.left;
        }
        let action_detail = if index == 0 {
            request.action_detail_metrics.and_then(|metrics| {
                let gap = scale(spec.action_detail_gap);
                (metrics
                    .label_width
                    .saturating_add(gap)
                    .saturating_add(metrics.detail_width)
                    <= text.width())
                .then(|| {
                    let detail_left = text.right.saturating_sub(metrics.detail_width);
                    let detail = CandidateSceneRect {
                        left: detail_left,
                        top: metadata_line.top,
                        right: text.right,
                        bottom: metadata_line.bottom,
                    };
                    text.right = detail_left.saturating_sub(gap);
                    detail
                })
            })
        } else {
            None
        };
        let personal_mark = if request.personalized[index] {
            rank.and_then(|rank| personal_mark_rect(spec, request.dpi, rank))
        } else {
            None
        };
        if text.width() <= 0
            || text.height() <= 0
            || !bounds.contains(text)
            || selected_surface
                .is_some_and(|selected| selected.width() <= 0 || !bounds.contains(selected))
            || selection_accent
                .is_some_and(|accent| accent.width() <= 0 || !bounds.contains(accent))
            || rank.is_some_and(|rank| rank.width() <= 0 || !bounds.contains(rank))
            || personal_mark.is_some_and(|mark| !bounds.contains(mark))
            || notice_icon.is_some_and(|icon| !bounds.contains(icon))
            || action_detail.is_some_and(|detail| detail.width() <= 0 || !bounds.contains(detail))
        {
            return None;
        }
        items.push(CandidateSceneItem {
            index,
            bounds,
            selected_surface,
            selection_accent,
            rank,
            text,
            action_detail,
            metadata_line,
            personal_mark,
            notice_icon,
            baseline_aligned,
        });
    }

    let candidate_right_limit =
        if footer_needed && request.layout == CandidateSceneLayout::Horizontal {
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

    let (footer, footer_divider, footer_mode, footer_page) = if footer_needed {
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
        let mode = request.footer_mode.then(|| CandidateSceneRect {
            right: if request.footer_page {
                footer
                    .right
                    .saturating_sub(scale(spec.footer_page_width))
                    .max(footer.left)
            } else {
                footer.right
            },
            ..footer
        });
        let page = request.footer_page.then(|| CandidateSceneRect {
            left: mode
                .map(|mode| mode.right.saturating_add(scale(spec.footer_mode_gap)))
                .unwrap_or(footer.left),
            ..footer
        });
        if mode.is_some_and(|mode| mode.width() <= 0 || !footer.contains(mode))
            || page.is_some_and(|page| page.width() <= 0 || !footer.contains(page))
        {
            return None;
        }
        (Some(footer), Some(divider), mode, page)
    } else {
        (None, None, None, None)
    };

    Some(CandidateScene {
        client,
        items,
        footer,
        footer_divider,
        footer_mode,
        footer_page,
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
        assert_eq!(
            CandidateSceneSemantic::CandidateRank.stable_id(),
            "candidate.rank"
        );
        assert_eq!(
            CandidateSceneSemantic::CandidateText.stable_id(),
            "candidate.text"
        );
        assert_eq!(
            CandidateSceneSemantic::CandidateActionDetail.stable_id(),
            "candidate.action-detail"
        );
        assert_eq!(
            CandidateSceneSemantic::CandidatePersonalMark.stable_id(),
            "candidate.personal-mark"
        );
        assert_eq!(
            CandidateSceneSemantic::NoticeIcon.stable_id(),
            "notice.icon"
        );
        assert_eq!(CandidateSceneSemantic::Footer.stable_id(), "footer");
        assert_eq!(
            CandidateSceneSemantic::FooterDivider.stable_id(),
            "footer.divider"
        );
        assert_eq!(
            CandidateSceneSemantic::FooterMode.stable_id(),
            "footer.mode"
        );
        assert_eq!(
            CandidateSceneSemantic::FooterPage.stable_id(),
            "footer.page"
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
                footer_mode: false,
                footer_page: true,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[true, false],
                rank_metrics: Some(CandidateSceneFontMetrics {
                    height: 14,
                    ascent: 11,
                }),
                candidate_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                selected_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                action_detail_metrics: None,
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
            scene.items[0].rank,
            Some(CandidateSceneRect {
                left: 18,
                top: 16,
                right: 34,
                bottom: 30,
            })
        );
        assert_eq!(
            scene.items[0].text,
            CandidateSceneRect {
                left: 38,
                top: 14,
                right: 98,
                bottom: 31,
            }
        );
        assert_eq!(
            scene.items[0].personal_mark,
            Some(CandidateSceneRect {
                left: 18,
                top: 21,
                right: 21,
                bottom: 24,
            })
        );
        let rank = scene.items[0].rank.unwrap();
        assert_eq!(rank.top + 11, scene.items[0].text.top + 13);
        let mark = scene.items[0].personal_mark.unwrap();
        assert!(mark.left >= rank.left && mark.right <= rank.right);
        assert!(mark.top >= rank.top && mark.bottom <= rank.bottom);
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
        assert_eq!(scene.footer_page, scene.footer);
        assert!(scene.footer_mode.is_none());
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
                footer_mode: false,
                footer_page: true,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[false; 3],
                rank_metrics: None,
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
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
    fn notice_scene_reserves_one_icon_without_a_rank_column() {
        let scene = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 210,
                height: 46,
                candidate_count: 1,
                horizontal_candidate_widths: &[200],
                footer_width: 0,
                footer_mode: false,
                footer_page: false,
                selected_surface: false,
                show_rank: false,
                notice_icon: true,
                personalized: &[false],
                rank_metrics: Some(CandidateSceneFontMetrics {
                    height: 14,
                    ascent: 11,
                }),
                candidate_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                selected_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                action_detail_metrics: None,
            },
        )
        .unwrap();

        let item = scene.items[0];
        assert!(item.selected_surface.is_none());
        assert!(item.rank.is_none());
        assert_eq!(
            item.notice_icon,
            Some(CandidateSceneRect {
                left: 12,
                top: 11,
                right: 36,
                bottom: 35,
            })
        );
        assert_eq!(item.text.left, 43);
        assert_eq!((item.text.top, item.text.bottom), (14, 31));
    }

    #[test]
    fn action_detail_and_footer_labels_receive_disjoint_semantic_regions() {
        let scene = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: CandidateSceneLayout::Horizontal,
                dpi: 96,
                width: 480,
                height: 46,
                candidate_count: 1,
                horizontal_candidate_widths: &[200],
                footer_width: 108,
                footer_mode: true,
                footer_page: true,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[false],
                rank_metrics: Some(CandidateSceneFontMetrics {
                    height: 14,
                    ascent: 11,
                }),
                candidate_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                selected_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                action_detail_metrics: Some(CandidateSceneActionDetailMetrics {
                    label_width: 60,
                    detail_width: 40,
                }),
            },
        )
        .unwrap();

        assert_eq!(scene.items[0].text.right, 146);
        assert_eq!(
            scene.items[0].action_detail,
            Some(CandidateSceneRect {
                left: 158,
                top: 16,
                right: 198,
                bottom: 30,
            })
        );
        assert_eq!(
            scene.footer_mode,
            Some(CandidateSceneRect {
                left: 382,
                top: 5,
                right: 425,
                bottom: 41,
            })
        );
        assert_eq!(
            scene.footer_page,
            Some(CandidateSceneRect {
                left: 429,
                top: 5,
                right: 473,
                bottom: 41,
            })
        );
    }

    #[test]
    fn semantic_hit_testing_prefers_specific_features_and_excludes_far_edges() {
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
                footer_mode: false,
                footer_page: true,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[true, false],
                rank_metrics: Some(CandidateSceneFontMetrics {
                    height: 14,
                    ascent: 11,
                }),
                candidate_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                selected_text_metrics: Some(CandidateSceneFontMetrics {
                    height: 17,
                    ascent: 13,
                }),
                action_detail_metrics: None,
            },
        )
        .unwrap();

        let semantics = |x, y| {
            scene
                .semantic_hits_at(x, y)
                .into_iter()
                .map(|hit| (hit.semantic, hit.candidate_index))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            semantics(19, 22),
            vec![
                (CandidateSceneSemantic::CandidatePersonalMark, Some(0)),
                (CandidateSceneSemantic::CandidateRank, Some(0)),
                (CandidateSceneSemantic::CandidateSelectedSurface, Some(0)),
                (CandidateSceneSemantic::CandidateItem, Some(0)),
            ]
        );
        assert_eq!(
            semantics(40, 20),
            vec![
                (CandidateSceneSemantic::CandidateText, Some(0)),
                (CandidateSceneSemantic::CandidateSelectedSurface, Some(0)),
                (CandidateSceneSemantic::CandidateItem, Some(0)),
            ]
        );
        assert_eq!(
            semantics(12, 20),
            vec![
                (CandidateSceneSemantic::SelectionAccent, Some(0)),
                (CandidateSceneSemantic::CandidateSelectedSurface, Some(0)),
                (CandidateSceneSemantic::CandidateItem, Some(0)),
            ]
        );
        assert_eq!(
            semantics(430, 20),
            vec![
                (CandidateSceneSemantic::FooterPage, None),
                (CandidateSceneSemantic::Footer, None),
            ]
        );
        assert_eq!(
            semantics(418, 20),
            vec![(CandidateSceneSemantic::FooterDivider, None)]
        );

        // The first item's right edge belongs to the adjacent item, while the
        // client's right and bottom edges lie outside the scene altogether.
        assert_eq!(
            semantics(105, 6),
            vec![(CandidateSceneSemantic::CandidateItem, Some(1))]
        );
        assert!(semantics(480, 20).is_empty());
        assert!(semantics(20, 46).is_empty());
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
                footer_mode: false,
                footer_page: false,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[false; 2],
                rank_metrics: None,
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
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
                footer_mode: false,
                footer_page: true,
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &[false; 2],
                rank_metrics: None,
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
            },
        );
        assert!(overlapping_footer.is_none());

        let invalid_metrics = build_candidate_scene(
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
                rank_metrics: Some(CandidateSceneFontMetrics {
                    height: 14,
                    ascent: 15,
                }),
                candidate_text_metrics: None,
                selected_text_metrics: None,
                action_detail_metrics: None,
            },
        );
        assert!(invalid_metrics.is_none());
    }
}
