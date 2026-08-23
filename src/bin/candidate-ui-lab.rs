#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("候选窗实验室目前只支持 Windows");
    std::process::exit(1);
}

// Reuse the exact source modules without exposing the production DLL's
// internal rendering API as a public library surface.
#[cfg(windows)]
#[path = "../candidate_ui.rs"]
mod candidate_ui;
#[cfg(windows)]
#[path = "../candidate_ui_gdi.rs"]
mod candidate_ui_gdi;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_app::run() {
        windows_app::show_fatal_error(&error);
    }
}

#[cfg(windows)]
mod windows_app {
    use std::cell::RefCell;

    use crate::candidate_ui::{
        CandidateRgb, CandidateScene, CandidateSceneFontMetrics, CandidateSceneLayout,
        CandidateSceneRequest, DEFAULT_CANDIDATE_VISUAL_SPEC, allocate_horizontal_candidate_widths,
        build_candidate_scene, candidate_horizontal_logical_width, candidate_ui_scale,
        candidate_vertical_logical_width,
    };
    use crate::candidate_ui_gdi::{
        CandidateSceneFonts, CandidateScenePaintContent, paint_candidate_scene,
    };
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
        DeleteDC, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect,
        GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, TEXTMETRICW, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GetClientRect, GetMessageW, GetWindowRect, IDC_ARROW, LoadCursorW,
        MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostQuitMessage, RegisterClassW,
        SET_WINDOW_POS_FLAGS, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
        WINDOW_EX_STYLE, WM_CLOSE, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
        WM_LBUTTONDOWN, WM_PAINT, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_SYSMENU,
    };
    use windows::core::{PCWSTR, w};

    const DPIS: [u32; 4] = [96, 120, 144, 192];
    const MAX_CANDIDATE_CHARACTERS: usize = 32;

    thread_local! {
        static APP_STATE: RefCell<Option<LabState>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LabScenario {
        Everyday,
        LongCandidate,
        Personalized,
    }

    impl LabScenario {
        const ALL: [Self; 3] = [Self::Everyday, Self::LongCandidate, Self::Personalized];

        const fn label(self) -> &'static str {
            match self {
                Self::Everyday => "日常短词",
                Self::LongCandidate => "长候选",
                Self::Personalized => "个人标记",
            }
        }

        fn content(self) -> LabContent {
            match self {
                Self::Everyday => LabContent {
                    candidates: ["春风", "清泉", "远山", "新芽", "流云", "灯火"]
                        .map(str::to_owned)
                        .to_vec(),
                    personalized: vec![false; 6],
                    mode_label: None,
                    page_label: Some("1/3"),
                    footer_logical_width: 62,
                },
                Self::LongCandidate => LabContent {
                    candidates: [
                        "春夏秋冬".repeat(8),
                        "远山".to_owned(),
                        "清泉".to_owned(),
                        "松林".to_owned(),
                        "竹影".to_owned(),
                        "云海".to_owned(),
                    ]
                    .to_vec(),
                    personalized: vec![false; 6],
                    mode_label: None,
                    page_label: None,
                    footer_logical_width: 0,
                },
                Self::Personalized => LabContent {
                    candidates: ["自然码", "候选窗", "个人词", "纠错", "找字", "许愿"]
                        .map(str::to_owned)
                        .to_vec(),
                    personalized: vec![true, false, true, false, false, true],
                    mode_label: Some("个人 · Ctrl+Del"),
                    page_label: None,
                    footer_logical_width: 108,
                },
            }
        }
    }

    struct LabContent {
        candidates: Vec<String>,
        personalized: Vec<bool>,
        mode_label: Option<&'static str>,
        page_label: Option<&'static str>,
        footer_logical_width: i32,
    }

    #[derive(Clone, Debug)]
    struct LabState {
        layout: CandidateSceneLayout,
        dpi_index: usize,
        scenario_index: usize,
        last_hit: Option<String>,
        last_scene: Option<CandidateScene>,
    }

    impl Default for LabState {
        fn default() -> Self {
            Self {
                layout: CandidateSceneLayout::Horizontal,
                dpi_index: 0,
                scenario_index: 0,
                last_hit: None,
                last_scene: None,
            }
        }
    }

    impl LabState {
        fn dpi(&self) -> u32 {
            DPIS[self.dpi_index]
        }

        fn scenario(&self) -> LabScenario {
            LabScenario::ALL[self.scenario_index]
        }

        fn cycle_dpi(&mut self) {
            self.dpi_index = (self.dpi_index + 1) % DPIS.len();
            self.reset_inspection();
        }

        fn cycle_scenario(&mut self) {
            self.scenario_index = (self.scenario_index + 1) % LabScenario::ALL.len();
            self.reset_inspection();
        }

        fn toggle_layout(&mut self) {
            self.layout = match self.layout {
                CandidateSceneLayout::Horizontal => CandidateSceneLayout::Vertical,
                CandidateSceneLayout::Vertical => CandidateSceneLayout::Horizontal,
            };
            self.reset_inspection();
        }

        fn reset_inspection(&mut self) {
            self.last_hit = None;
            self.last_scene = None;
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct LabFrameMetrics {
        width: i32,
        height: i32,
        footer_width: i32,
    }

    pub fn run() -> Result<(), String> {
        // SAFETY: this process owns the registered class, state, window and
        // message loop for their complete lifetimes.
        unsafe {
            let module = GetModuleHandleW(None).map_err(|_| "无法读取实验室模块".to_owned())?;
            let instance = HINSTANCE(module.0);
            let class_name = w!("ZiranmaCandidateUiLabWindow");
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                hInstance: instance,
                lpszClassName: class_name,
                lpfnWndProc: Some(window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)
                    .map_err(|_| "无法载入实验室光标".to_owned())?,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err("无法注册候选窗实验室窗口".to_owned());
            }
            APP_STATE.with(|slot| slot.replace(Some(LabState::default())));
            let window = match CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("候选窗实验室"),
                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                640,
                160,
                None,
                None,
                Some(instance),
                None,
            ) {
                Ok(window) => window,
                Err(_) => {
                    APP_STATE.with(|slot| slot.replace(None));
                    return Err("无法创建候选窗实验室窗口".to_owned());
                }
            };
            refresh_window(window, true);
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetForegroundWindow(window);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    return Err("候选窗实验室消息循环失败".to_owned());
                }
                if result == 0 {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => LRESULT(0),
            WM_PAINT => {
                unsafe { paint_window(window) };
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_KEYDOWN => {
                if wparam.0 == usize::from(VK_ESCAPE.0) {
                    let _ = unsafe { DestroyWindow(window) };
                    return LRESULT(0);
                }
                let changed = APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(state) = slot.as_mut() else {
                        return false;
                    };
                    match u32::try_from(wparam.0).unwrap_or_default() {
                        0x44 => state.cycle_dpi(),
                        0x48 => state.toggle_layout(),
                        0x53 => state.cycle_scenario(),
                        _ => return false,
                    }
                    true
                });
                if changed {
                    refresh_window(window, true);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let packed = lparam.0 as u32;
                let x = i32::from(packed as u16 as i16);
                let y = i32::from((packed >> 16) as u16 as i16);
                APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(state) = slot.as_mut() else {
                        return;
                    };
                    state.last_hit = state
                        .last_scene
                        .as_ref()
                        .and_then(|scene| scene.semantic_hits_at(x, y).into_iter().next())
                        .map(|hit| match hit.candidate_index {
                            Some(index) => {
                                format!("{} · 候选 {}", hit.semantic.stable_id(), index + 1)
                            }
                            None => hit.semantic.stable_id().to_owned(),
                        });
                });
                refresh_window(window, false);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(window) };
                LRESULT(0)
            }
            WM_DESTROY => {
                APP_STATE.with(|slot| slot.replace(None));
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn refresh_window(window: HWND, resize: bool) {
        let (metrics, title) = APP_STATE.with(|slot| {
            let slot = slot.borrow();
            let state = slot.as_ref().cloned().unwrap_or_default();
            (frame_metrics(&state), window_title(&state))
        });
        if resize {
            resize_client(window, metrics.width, metrics.height);
        }
        let title = title.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        // SAFETY: the title is NUL-terminated for the synchronous call.
        unsafe {
            let _ = SetWindowTextW(window, PCWSTR(title.as_ptr()));
            let _ = InvalidateRect(Some(window), None, false);
        }
    }

    fn window_title(state: &LabState) -> String {
        let layout = match state.layout {
            CandidateSceneLayout::Horizontal => "横排",
            CandidateSceneLayout::Vertical => "竖排",
        };
        let hit = state
            .last_hit
            .as_deref()
            .map(|hit| format!(" · 命中 {hit}"))
            .unwrap_or_default();
        format!(
            "候选窗实验室 · {} · {} DPI · {}{}    H 布局 / D 缩放 / S 场景 / 点击检查 / Esc 退出",
            layout,
            state.dpi(),
            state.scenario().label(),
            hit
        )
    }

    fn frame_metrics(state: &LabState) -> LabFrameMetrics {
        let spec = DEFAULT_CANDIDATE_VISUAL_SPEC;
        let dpi = state.dpi();
        let content = state.scenario().content();
        let scale = |logical| candidate_ui_scale(dpi, logical);
        let footer_width = scale(content.footer_logical_width);
        let padding = scale(spec.outer_padding);
        let outer_width = padding.saturating_mul(2);
        match state.layout {
            CandidateSceneLayout::Horizontal => {
                let natural = content.candidates.iter().enumerate().fold(
                    outer_width.saturating_add(footer_width),
                    |width, (index, candidate)| {
                        width.saturating_add(scale(candidate_horizontal_logical_width(
                            spec,
                            candidate,
                            index == 0,
                            MAX_CANDIDATE_CHARACTERS,
                        )))
                    },
                );
                let minimum = content.candidates.iter().enumerate().fold(
                    outer_width.saturating_add(footer_width),
                    |width, (index, candidate)| {
                        width.saturating_add(if index == 0 {
                            scale(candidate_horizontal_logical_width(
                                spec,
                                candidate,
                                true,
                                MAX_CANDIDATE_CHARACTERS,
                            ))
                        } else {
                            scale(spec.horizontal_min_item_width)
                        })
                    },
                );
                LabFrameMetrics {
                    width: natural
                        .max(minimum)
                        .max(scale(spec.horizontal_min_width))
                        .min(scale(spec.horizontal_max_width)),
                    height: padding
                        .saturating_mul(2)
                        .saturating_add(scale(spec.row_height)),
                    footer_width,
                }
            }
            CandidateSceneLayout::Vertical => {
                let candidate_width = content
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| {
                        scale(candidate_vertical_logical_width(
                            spec,
                            candidate,
                            index == 0,
                            MAX_CANDIDATE_CHARACTERS,
                        ))
                    })
                    .max()
                    .unwrap_or_default();
                let footer_height = if content.mode_label.is_some() || content.page_label.is_some()
                {
                    spec.footer_height
                } else {
                    0
                };
                LabFrameMetrics {
                    width: outer_width
                        .saturating_add(
                            candidate_width
                                .saturating_add(scale(spec.vertical_rounding_slack))
                                .max(footer_width),
                        )
                        .max(scale(spec.vertical_min_width))
                        .min(scale(spec.vertical_max_width)),
                    height: padding
                        .saturating_mul(2)
                        .saturating_add(scale(spec.row_height).saturating_mul(
                            i32::try_from(content.candidates.len()).unwrap_or(i32::MAX),
                        ))
                        .saturating_add(scale(footer_height)),
                    footer_width,
                }
            }
        }
    }

    fn resize_client(window: HWND, width: i32, height: i32) {
        let mut client = RECT::default();
        let mut outer = RECT::default();
        // SAFETY: both output rectangles remain valid for these synchronous
        // queries. Failure simply leaves the current window size unchanged.
        if unsafe { GetClientRect(window, &mut client) }.is_err()
            || unsafe { GetWindowRect(window, &mut outer) }.is_err()
        {
            return;
        }
        let nonclient_width = outer
            .right
            .saturating_sub(outer.left)
            .saturating_sub(client.right.saturating_sub(client.left));
        let nonclient_height = outer
            .bottom
            .saturating_sub(outer.top)
            .saturating_sub(client.bottom.saturating_sub(client.top));
        let flags: SET_WINDOW_POS_FLAGS = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        // SAFETY: only this window's dimensions change; position and z-order
        // remain fixed by flags.
        unsafe {
            let _ = SetWindowPos(
                window,
                None,
                0,
                0,
                width.saturating_add(nonclient_width),
                height.saturating_add(nonclient_height),
                flags,
            );
        }
    }

    unsafe fn paint_window(window: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let paint_dc = unsafe { BeginPaint(window, &mut paint) };
        if paint_dc.is_invalid() {
            return;
        }
        let mut client = RECT::default();
        if unsafe { GetClientRect(window, &mut client) }.is_err() {
            unsafe {
                let _ = EndPaint(window, &paint);
            }
            return;
        }
        let width = client.right.saturating_sub(client.left);
        let height = client.bottom.saturating_sub(client.top);
        let mut buffer_dc = HDC::default();
        let mut bitmap = HBITMAP::default();
        let mut previous_bitmap = HGDIOBJ::default();
        let mut hdc = paint_dc;
        if width > 0 && height > 0 {
            buffer_dc = unsafe { CreateCompatibleDC(Some(paint_dc)) };
            if !buffer_dc.is_invalid() {
                bitmap = unsafe { CreateCompatibleBitmap(paint_dc, width, height) };
                if !bitmap.is_invalid() {
                    previous_bitmap = unsafe { SelectObject(buffer_dc, HGDIOBJ(bitmap.0)) };
                    if !previous_bitmap.is_invalid() {
                        hdc = buffer_dc;
                    }
                }
            }
        }

        let snapshot = APP_STATE.with(|slot| slot.borrow().as_ref().cloned().unwrap_or_default());
        let dpi = snapshot.dpi();
        let content = snapshot.scenario().content();
        let fonts = unsafe { create_fonts(dpi) };
        let initial_font = [fonts.candidate, fonts.selected, fonts.metadata]
            .into_iter()
            .find(|font| !font.is_invalid())
            .unwrap_or_default();
        let previous_font = if initial_font.is_invalid() {
            HGDIOBJ::default()
        } else {
            unsafe { SelectObject(hdc, HGDIOBJ(initial_font.0)) }
        };
        unsafe {
            let _ = SetBkMode(hdc, TRANSPARENT);
        }
        let rank_metrics = unsafe { font_metrics(hdc, fonts.metadata) };
        let candidate_metrics = unsafe { font_metrics(hdc, fonts.candidate) };
        let selected_metrics = unsafe { font_metrics(hdc, fonts.selected) };
        let metrics = frame_metrics(&snapshot);
        let horizontal_widths = if snapshot.layout == CandidateSceneLayout::Horizontal {
            allocate_horizontal_candidate_widths(
                DEFAULT_CANDIDATE_VISUAL_SPEC,
                dpi,
                width,
                metrics.footer_width,
                &content.candidates,
                false,
                MAX_CANDIDATE_CHARACTERS,
            )
        } else {
            Vec::new()
        };
        let scene = build_candidate_scene(
            DEFAULT_CANDIDATE_VISUAL_SPEC,
            CandidateSceneRequest {
                layout: snapshot.layout,
                dpi,
                width,
                height,
                candidate_count: content.candidates.len(),
                horizontal_candidate_widths: &horizontal_widths,
                footer_width: metrics.footer_width,
                footer_mode: content.mode_label.is_some(),
                footer_page: content.page_label.is_some(),
                selected_surface: true,
                show_rank: true,
                notice_icon: false,
                personalized: &content.personalized,
                rank_metrics,
                candidate_text_metrics: candidate_metrics,
                selected_text_metrics: selected_metrics,
                action_detail_metrics: None,
            },
        );
        if let Some(scene) = scene.as_ref() {
            unsafe {
                paint_candidate_scene(
                    hdc,
                    dpi,
                    DEFAULT_CANDIDATE_VISUAL_SPEC,
                    scene,
                    fonts,
                    CandidateScenePaintContent {
                        candidates: &content.candidates,
                        action_detail: None,
                        mode_label: content.mode_label,
                        page_label: content.page_label,
                        notice_icon: None,
                        max_candidate_characters: MAX_CANDIDATE_CHARACTERS,
                    },
                );
            }
        } else {
            unsafe {
                fill_background(hdc, &client, DEFAULT_CANDIDATE_VISUAL_SPEC.background);
            }
        }
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.last_scene = scene;
            }
        });

        if !previous_font.is_invalid() {
            unsafe {
                let _ = SelectObject(hdc, previous_font);
            }
        }
        for font in [fonts.candidate, fonts.selected, fonts.metadata] {
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
        if hdc == buffer_dc && !buffer_dc.is_invalid() {
            unsafe {
                let _ = BitBlt(
                    paint_dc,
                    0,
                    0,
                    width,
                    height,
                    Some(buffer_dc),
                    0,
                    0,
                    SRCCOPY,
                );
                let _ = SelectObject(buffer_dc, previous_bitmap);
            }
        }
        if !bitmap.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
        }
        if !buffer_dc.is_invalid() {
            unsafe {
                let _ = DeleteDC(buffer_dc);
            }
        }
        unsafe {
            let _ = EndPaint(window, &paint);
        }
    }

    unsafe fn create_fonts(dpi: u32) -> CandidateSceneFonts {
        let spec = DEFAULT_CANDIDATE_VISUAL_SPEC;
        CandidateSceneFonts {
            candidate: unsafe {
                create_font(
                    candidate_ui_scale(dpi, spec.candidate_font_height),
                    FW_NORMAL.0,
                )
            },
            selected: unsafe {
                create_font(
                    candidate_ui_scale(dpi, spec.candidate_font_height),
                    FW_SEMIBOLD.0,
                )
            },
            metadata: unsafe {
                create_font(
                    candidate_ui_scale(dpi, spec.metadata_font_height),
                    FW_NORMAL.0,
                )
            },
        }
    }

    unsafe fn fill_background(hdc: HDC, bounds: &RECT, color: CandidateRgb) {
        let color = COLORREF(
            u32::from(color.red) | (u32::from(color.green) << 8) | (u32::from(color.blue) << 16),
        );
        let brush = unsafe { CreateSolidBrush(color) };
        if !brush.is_invalid() {
            unsafe {
                let _ = FillRect(hdc, bounds, brush);
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
    }

    unsafe fn create_font(height: i32, weight: u32) -> HFONT {
        unsafe {
            CreateFontW(
                -height,
                0,
                0,
                0,
                i32::try_from(weight).unwrap_or(400),
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
                w!("Microsoft YaHei UI"),
            )
        }
    }

    unsafe fn font_metrics(hdc: HDC, font: HFONT) -> Option<CandidateSceneFontMetrics> {
        if font.is_invalid() {
            return None;
        }
        unsafe {
            let _ = SelectObject(hdc, HGDIOBJ(font.0));
        }
        let mut metrics = TEXTMETRICW::default();
        if !unsafe { GetTextMetricsW(hdc, &mut metrics) }.as_bool() {
            return None;
        }
        Some(CandidateSceneFontMetrics {
            height: metrics.tmHeight,
            ascent: metrics.tmAscent,
        })
    }

    pub fn show_fatal_error(message: &str) {
        let message = message.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        // SAFETY: both strings are NUL-terminated and used synchronously.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                w!("候选窗实验室"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_public_scenario_builds_at_every_reviewed_dpi_and_layout() {
            for scenario_index in 0..LabScenario::ALL.len() {
                for dpi_index in 0..DPIS.len() {
                    for layout in [
                        CandidateSceneLayout::Horizontal,
                        CandidateSceneLayout::Vertical,
                    ] {
                        let state = LabState {
                            layout,
                            dpi_index,
                            scenario_index,
                            ..LabState::default()
                        };
                        let content = state.scenario().content();
                        let metrics = frame_metrics(&state);
                        let widths = if layout == CandidateSceneLayout::Horizontal {
                            allocate_horizontal_candidate_widths(
                                DEFAULT_CANDIDATE_VISUAL_SPEC,
                                state.dpi(),
                                metrics.width,
                                metrics.footer_width,
                                &content.candidates,
                                false,
                                MAX_CANDIDATE_CHARACTERS,
                            )
                        } else {
                            Vec::new()
                        };
                        let scene = build_candidate_scene(
                            DEFAULT_CANDIDATE_VISUAL_SPEC,
                            CandidateSceneRequest {
                                layout,
                                dpi: state.dpi(),
                                width: metrics.width,
                                height: metrics.height,
                                candidate_count: content.candidates.len(),
                                horizontal_candidate_widths: &widths,
                                footer_width: metrics.footer_width,
                                footer_mode: content.mode_label.is_some(),
                                footer_page: content.page_label.is_some(),
                                selected_surface: true,
                                show_rank: true,
                                notice_icon: false,
                                personalized: &content.personalized,
                                rank_metrics: None,
                                candidate_text_metrics: None,
                                selected_text_metrics: None,
                                action_detail_metrics: None,
                            },
                        )
                        .unwrap_or_else(|| {
                            panic!(
                                "scene failed: scenario={:?} dpi={} layout={:?} metrics={:?} widths={:?}",
                                state.scenario(),
                                state.dpi(),
                                layout,
                                metrics,
                                widths
                            )
                        });
                        assert_eq!(scene.items.len(), content.candidates.len());
                        assert_eq!(scene.client.right, metrics.width);
                        assert_eq!(scene.client.bottom, metrics.height);
                    }
                }
            }
        }

        #[test]
        fn controls_cycle_only_fixed_public_states() {
            let mut state = LabState::default();
            for expected in [120, 144, 192, 96] {
                state.cycle_dpi();
                assert_eq!(state.dpi(), expected);
            }
            state.toggle_layout();
            assert_eq!(state.layout, CandidateSceneLayout::Vertical);
            state.toggle_layout();
            assert_eq!(state.layout, CandidateSceneLayout::Horizontal);
            for expected in [
                LabScenario::LongCandidate,
                LabScenario::Personalized,
                LabScenario::Everyday,
            ] {
                state.cycle_scenario();
                assert_eq!(state.scenario(), expected);
            }
        }
    }
}
