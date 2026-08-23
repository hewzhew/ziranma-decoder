//! Bounded A/B visual-token editing for the public candidate UI lab.

use crate::candidate_ui::{CandidateRgb, CandidateVisualSpec, DEFAULT_CANDIDATE_VISUAL_SPEC};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabVariant {
    Baseline,
    Draft,
}

impl CandidateUiLabVariant {
    pub(crate) const fn stable_id(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Draft => "draft",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "A 基线",
            Self::Draft => "B 草案",
        }
    }

    pub(crate) const fn other(self) -> Self {
        match self {
            Self::Baseline => Self::Draft,
            Self::Draft => Self::Baseline,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabToken {
    OuterPadding,
    RowHeight,
    TextPadding,
    SelectedTextInset,
    RankWidth,
    RankGap,
    HorizontalMaxWidth,
    HorizontalMinItemWidth,
    VerticalMinWidth,
    CornerDiameter,
    SelectedSurfaceHeight,
    SelectionAccentWidth,
    CandidateFontHeight,
    MetadataFontHeight,
}

impl CandidateUiLabToken {
    pub(crate) const ALL: [Self; 14] = [
        Self::OuterPadding,
        Self::RowHeight,
        Self::TextPadding,
        Self::SelectedTextInset,
        Self::RankWidth,
        Self::RankGap,
        Self::HorizontalMaxWidth,
        Self::HorizontalMinItemWidth,
        Self::VerticalMinWidth,
        Self::CornerDiameter,
        Self::SelectedSurfaceHeight,
        Self::SelectionAccentWidth,
        Self::CandidateFontHeight,
        Self::MetadataFontHeight,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::OuterPadding => "外边距",
            Self::RowHeight => "候选行高",
            Self::TextPadding => "文字内边距",
            Self::SelectedTextInset => "首选文字缩进",
            Self::RankWidth => "序号宽度",
            Self::RankGap => "序号文字间距",
            Self::HorizontalMaxWidth => "横排最大宽度",
            Self::HorizontalMinItemWidth => "横排单项最小宽度",
            Self::VerticalMinWidth => "竖排最小宽度",
            Self::CornerDiameter => "外框圆角直径",
            Self::SelectedSurfaceHeight => "首选底色高度",
            Self::SelectionAccentWidth => "首选蓝条宽度",
            Self::CandidateFontHeight => "候选字号高度",
            Self::MetadataFontHeight => "序号字号高度",
        }
    }

    pub(crate) const fn bounds(self) -> CandidateUiLabTokenBounds {
        match self {
            Self::OuterPadding => CandidateUiLabTokenBounds::new(0, 16, 1),
            Self::RowHeight => CandidateUiLabTokenBounds::new(28, 56, 1),
            Self::TextPadding => CandidateUiLabTokenBounds::new(2, 16, 1),
            Self::SelectedTextInset => CandidateUiLabTokenBounds::new(4, 24, 1),
            Self::RankWidth => CandidateUiLabTokenBounds::new(10, 28, 1),
            Self::RankGap => CandidateUiLabTokenBounds::new(0, 12, 1),
            Self::HorizontalMaxWidth => CandidateUiLabTokenBounds::new(740, 1200, 10),
            Self::HorizontalMinItemWidth => CandidateUiLabTokenBounds::new(48, 160, 2),
            Self::VerticalMinWidth => CandidateUiLabTokenBounds::new(280, 620, 10),
            Self::CornerDiameter => CandidateUiLabTokenBounds::new(0, 24, 1),
            Self::SelectedSurfaceHeight => CandidateUiLabTokenBounds::new(20, 48, 1),
            Self::SelectionAccentWidth => CandidateUiLabTokenBounds::new(1, 8, 1),
            Self::CandidateFontHeight => CandidateUiLabTokenBounds::new(13, 24, 1),
            Self::MetadataFontHeight => CandidateUiLabTokenBounds::new(10, 20, 1),
        }
    }

    pub(crate) const fn value(self, spec: CandidateVisualSpec) -> i32 {
        match self {
            Self::OuterPadding => spec.outer_padding,
            Self::RowHeight => spec.row_height,
            Self::TextPadding => spec.text_padding,
            Self::SelectedTextInset => spec.selected_text_inset,
            Self::RankWidth => spec.rank_width,
            Self::RankGap => spec.rank_gap,
            Self::HorizontalMaxWidth => spec.horizontal_max_width,
            Self::HorizontalMinItemWidth => spec.horizontal_min_item_width,
            Self::VerticalMinWidth => spec.vertical_min_width,
            Self::CornerDiameter => spec.corner_diameter,
            Self::SelectedSurfaceHeight => spec.selected_surface_height,
            Self::SelectionAccentWidth => spec.selection_accent_width,
            Self::CandidateFontHeight => spec.candidate_font_height,
            Self::MetadataFontHeight => spec.metadata_font_height,
        }
    }

    fn set(self, spec: &mut CandidateVisualSpec, value: i32) {
        match self {
            Self::OuterPadding => spec.outer_padding = value,
            Self::RowHeight => spec.row_height = value,
            Self::TextPadding => spec.text_padding = value,
            Self::SelectedTextInset => spec.selected_text_inset = value,
            Self::RankWidth => spec.rank_width = value,
            Self::RankGap => spec.rank_gap = value,
            Self::HorizontalMaxWidth => spec.horizontal_max_width = value,
            Self::HorizontalMinItemWidth => spec.horizontal_min_item_width = value,
            Self::VerticalMinWidth => spec.vertical_min_width = value,
            Self::CornerDiameter => spec.corner_diameter = value,
            Self::SelectedSurfaceHeight => spec.selected_surface_height = value,
            Self::SelectionAccentWidth => spec.selection_accent_width = value,
            Self::CandidateFontHeight => spec.candidate_font_height = value,
            Self::MetadataFontHeight => spec.metadata_font_height = value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateUiLabColorRole {
    Background,
    SelectedBackground,
    SelectedText,
    CandidateText,
    SelectedRank,
    Rank,
    Page,
    SelectionAccent,
    ModeAccent,
    Border,
    FooterDivider,
}

impl CandidateUiLabColorRole {
    pub(crate) const ALL: [Self; 11] = [
        Self::Background,
        Self::SelectedBackground,
        Self::SelectedText,
        Self::CandidateText,
        Self::SelectedRank,
        Self::Rank,
        Self::Page,
        Self::SelectionAccent,
        Self::ModeAccent,
        Self::Border,
        Self::FooterDivider,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Background => "主背景",
            Self::SelectedBackground => "首选底色",
            Self::SelectedText => "首选正文",
            Self::CandidateText => "候选正文",
            Self::SelectedRank => "首选序号",
            Self::Rank => "普通序号",
            Self::Page => "页码",
            Self::SelectionAccent => "首选标记",
            Self::ModeAccent => "模式标记",
            Self::Border => "外边框",
            Self::FooterDivider => "页脚分隔线",
        }
    }

    pub(crate) const fn value(self, spec: CandidateVisualSpec) -> CandidateRgb {
        match self {
            Self::Background => spec.background,
            Self::SelectedBackground => spec.selected_background,
            Self::SelectedText => spec.selected_text,
            Self::CandidateText => spec.candidate_text,
            Self::SelectedRank => spec.selected_rank,
            Self::Rank => spec.rank,
            Self::Page => spec.page,
            Self::SelectionAccent => spec.selection_accent,
            Self::ModeAccent => spec.mode_accent,
            Self::Border => spec.border,
            Self::FooterDivider => spec.footer_divider,
        }
    }

    fn set(self, spec: &mut CandidateVisualSpec, value: CandidateRgb) {
        match self {
            Self::Background => spec.background = value,
            Self::SelectedBackground => spec.selected_background = value,
            Self::SelectedText => spec.selected_text = value,
            Self::CandidateText => spec.candidate_text = value,
            Self::SelectedRank => spec.selected_rank = value,
            Self::Rank => spec.rank = value,
            Self::Page => spec.page = value,
            Self::SelectionAccent => spec.selection_accent = value,
            Self::ModeAccent => spec.mode_accent = value,
            Self::Border => spec.border = value,
            Self::FooterDivider => spec.footer_divider = value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabTokenBounds {
    minimum: i32,
    maximum: i32,
    step: i32,
}

impl CandidateUiLabTokenBounds {
    const fn new(minimum: i32, maximum: i32, step: i32) -> Self {
        Self {
            minimum,
            maximum,
            step,
        }
    }

    pub(crate) const fn minimum(self) -> i32 {
        self.minimum
    }

    pub(crate) const fn maximum(self) -> i32 {
        self.maximum
    }

    pub(crate) const fn step(self) -> i32 {
        self.step
    }

    fn shifted(self, value: i32, steps: i32) -> i32 {
        value
            .saturating_add(self.step.saturating_mul(steps))
            .clamp(self.minimum, self.maximum)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CandidateUiLabVisualState {
    baseline: CandidateVisualSpec,
    draft: CandidateVisualSpec,
    active: CandidateUiLabVariant,
    token_index: usize,
    color_role_index: usize,
}

impl Default for CandidateUiLabVisualState {
    fn default() -> Self {
        Self {
            baseline: DEFAULT_CANDIDATE_VISUAL_SPEC,
            draft: DEFAULT_CANDIDATE_VISUAL_SPEC,
            active: CandidateUiLabVariant::Draft,
            token_index: 0,
            color_role_index: 0,
        }
    }
}

impl CandidateUiLabVisualState {
    pub(crate) const fn active_variant(self) -> CandidateUiLabVariant {
        self.active
    }

    pub(crate) const fn active_spec(self) -> CandidateVisualSpec {
        self.spec(self.active)
    }

    pub(crate) const fn spec(self, variant: CandidateUiLabVariant) -> CandidateVisualSpec {
        match variant {
            CandidateUiLabVariant::Baseline => self.baseline,
            CandidateUiLabVariant::Draft => self.draft,
        }
    }

    pub(crate) const fn comparison_variant(self) -> CandidateUiLabVariant {
        self.active.other()
    }

    pub(crate) const fn comparison_spec(self) -> CandidateVisualSpec {
        self.spec(self.comparison_variant())
    }

    pub(crate) fn selected_token(self) -> CandidateUiLabToken {
        CandidateUiLabToken::ALL[self.token_index]
    }

    pub(crate) const fn selected_token_index(self) -> usize {
        self.token_index
    }

    pub(crate) fn selected_color_role(self) -> CandidateUiLabColorRole {
        CandidateUiLabColorRole::ALL[self.color_role_index]
    }

    pub(crate) const fn selected_color_role_index(self) -> usize {
        self.color_role_index
    }

    pub(crate) fn selected_color(self) -> CandidateRgb {
        self.selected_color_role().value(self.active_spec())
    }

    pub(crate) fn selected_baseline_color(self) -> CandidateRgb {
        self.selected_color_role().value(self.baseline)
    }

    pub(crate) fn selected_draft_color(self) -> CandidateRgb {
        self.selected_color_role().value(self.draft)
    }

    pub(crate) fn selected_value(self) -> i32 {
        self.selected_token().value(self.active_spec())
    }

    pub(crate) fn selected_baseline_value(self) -> i32 {
        self.selected_token().value(self.baseline)
    }

    pub(crate) fn selected_draft_value(self) -> i32 {
        self.selected_token().value(self.draft)
    }

    pub(crate) fn select_token(&mut self, index: usize) -> bool {
        if index >= CandidateUiLabToken::ALL.len() || index == self.token_index {
            return false;
        }
        self.token_index = index;
        true
    }

    pub(crate) fn select_color_role(&mut self, index: usize) -> bool {
        if index >= CandidateUiLabColorRole::ALL.len() || index == self.color_role_index {
            return false;
        }
        self.color_role_index = index;
        true
    }

    pub(crate) fn cycle_token(&mut self) {
        self.token_index = (self.token_index + 1) % CandidateUiLabToken::ALL.len();
    }

    pub(crate) fn toggle_variant(&mut self) -> bool {
        self.active = match self.active {
            CandidateUiLabVariant::Baseline => CandidateUiLabVariant::Draft,
            CandidateUiLabVariant::Draft => CandidateUiLabVariant::Baseline,
        };
        true
    }

    pub(crate) fn adjust_draft(&mut self, steps: i32) -> bool {
        let token = self.selected_token();
        let current = token.value(self.draft);
        let value = token.bounds().shifted(current, steps);
        if value == current {
            return false;
        }
        let mut draft = self.draft;
        token.set(&mut draft, value);
        if !valid_draft(draft) {
            return false;
        }
        self.draft = draft;
        self.active = CandidateUiLabVariant::Draft;
        true
    }

    pub(crate) fn set_draft_color(&mut self, color: CandidateRgb) -> bool {
        let role = self.selected_color_role();
        if role.value(self.draft) == color {
            return false;
        }
        role.set(&mut self.draft, color);
        self.active = CandidateUiLabVariant::Draft;
        true
    }

    pub(crate) fn reset_draft(&mut self) -> bool {
        let changed = self.draft != self.baseline || self.active != CandidateUiLabVariant::Draft;
        self.draft = self.baseline;
        self.active = CandidateUiLabVariant::Draft;
        changed
    }
}

fn valid_draft(spec: CandidateVisualSpec) -> bool {
    spec.outer_padding >= 0
        && spec.row_height > 0
        && spec.text_padding >= 0
        && spec.rank_width > 0
        && spec.horizontal_min_width <= spec.horizontal_max_width
        && spec.horizontal_min_item_width <= spec.horizontal_max_width
        && spec.vertical_min_width <= spec.vertical_max_width
        && spec.selected_surface_height > 0
        && spec.selected_surface_height <= spec.row_height
        && spec.selection_accent_width > 0
        && spec.candidate_font_height > 0
        && spec.candidate_font_height <= spec.row_height
        && spec.metadata_font_height > 0
        && spec.metadata_font_height <= spec.row_height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_immutable_and_draft_reset_is_exact() {
        let mut state = CandidateUiLabVisualState::default();
        let baseline = state.active_spec();
        assert_eq!(CandidateUiLabVariant::Baseline.stable_id(), "baseline");
        assert_eq!(CandidateUiLabVariant::Draft.stable_id(), "draft");
        assert_ne!(
            CandidateUiLabVariant::Baseline.stable_id(),
            CandidateUiLabVariant::Draft.stable_id()
        );
        assert_eq!(state.active_variant(), CandidateUiLabVariant::Draft);
        assert_eq!(state.comparison_variant(), CandidateUiLabVariant::Baseline);
        assert_eq!(state.comparison_spec(), baseline);
        assert!(state.adjust_draft(1));
        assert_ne!(state.active_spec(), baseline);
        assert_eq!(state.comparison_spec(), baseline);
        assert!(state.toggle_variant());
        assert_eq!(state.active_variant(), CandidateUiLabVariant::Baseline);
        assert_eq!(state.active_spec(), baseline);
        assert_eq!(state.comparison_variant(), CandidateUiLabVariant::Draft);
        assert_ne!(state.comparison_spec(), baseline);
        assert!(state.reset_draft());
        assert_eq!(state.active_variant(), CandidateUiLabVariant::Draft);
        assert_eq!(state.active_spec(), baseline);
        assert!(!state.reset_draft());
    }

    #[test]
    fn every_token_is_bounded_and_cycling_has_one_stable_order() {
        let mut state = CandidateUiLabVisualState::default();
        for (index, expected) in CandidateUiLabToken::ALL.into_iter().enumerate() {
            assert_eq!(state.selected_token(), expected);
            assert_eq!(state.selected_token_index(), index);
            let bounds = expected.bounds();
            assert!(bounds.minimum() <= state.selected_baseline_value());
            assert!(bounds.maximum() >= state.selected_baseline_value());
            assert!(bounds.step() > 0);
            let original = state.selected_value();
            let _ = state.adjust_draft(i32::MAX);
            let maximum = state.selected_value();
            assert!(maximum >= original);
            let _ = state.adjust_draft(i32::MIN);
            let minimum = state.selected_value();
            assert!(minimum <= original);
            assert!(valid_draft(state.active_spec()));
            state.reset_draft();
            state.cycle_token();
        }
        assert_eq!(state.selected_token(), CandidateUiLabToken::ALL[0]);
        assert!(!state.select_token(CandidateUiLabToken::ALL.len()));
        assert_eq!(state.selected_token_index(), 0);
        assert!(state.select_token(CandidateUiLabToken::ALL.len() - 1));
        assert_eq!(
            state.selected_token(),
            CandidateUiLabToken::ALL[CandidateUiLabToken::ALL.len() - 1]
        );
        assert!(!state.select_token(CandidateUiLabToken::ALL.len() - 1));
    }

    #[test]
    fn every_semantic_color_edits_only_the_selected_draft_role() {
        let mut state = CandidateUiLabVisualState::default();
        for (index, expected) in CandidateUiLabColorRole::ALL.into_iter().enumerate() {
            assert_eq!(state.selected_color_role(), expected);
            assert_eq!(state.selected_color_role_index(), index);
            let baseline = state.selected_baseline_color();
            assert_eq!(state.selected_color(), baseline);
            assert_eq!(state.selected_draft_color(), baseline);
            assert!(!state.set_draft_color(baseline));

            let replacement = CandidateRgb {
                red: baseline.red.wrapping_add(1),
                green: baseline.green,
                blue: baseline.blue,
            };
            assert!(state.set_draft_color(replacement));
            assert_eq!(state.active_variant(), CandidateUiLabVariant::Draft);
            assert_eq!(state.selected_color(), replacement);
            assert_eq!(state.selected_draft_color(), replacement);
            assert_eq!(state.selected_baseline_color(), baseline);
            assert_eq!(expected.value(state.comparison_spec()), baseline);
            assert!(state.reset_draft());
            assert_eq!(state.selected_draft_color(), baseline);

            if index + 1 < CandidateUiLabColorRole::ALL.len() {
                assert!(state.select_color_role(index + 1));
            }
        }
        assert!(!state.select_color_role(CandidateUiLabColorRole::ALL.len()));
        assert_eq!(
            state.selected_color_role(),
            CandidateUiLabColorRole::ALL[CandidateUiLabColorRole::ALL.len() - 1]
        );
        assert!(!state.select_color_role(CandidateUiLabColorRole::ALL.len() - 1));
    }

    #[test]
    fn cross_token_constraints_reject_invalid_intermediate_specs() {
        let token_index = CandidateUiLabToken::ALL
            .iter()
            .position(|token| *token == CandidateUiLabToken::SelectedSurfaceHeight)
            .unwrap();
        let mut state = CandidateUiLabVisualState {
            token_index,
            ..CandidateUiLabVisualState::default()
        };
        assert!(state.adjust_draft(8));
        assert_eq!(state.active_spec().selected_surface_height, 36);
        assert!(!state.adjust_draft(1));

        state.token_index = CandidateUiLabToken::ALL
            .iter()
            .position(|token| *token == CandidateUiLabToken::RowHeight)
            .unwrap();
        assert!(!state.adjust_draft(-1));
        assert_eq!(state.active_spec().row_height, 36);
    }
}
