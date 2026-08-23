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
#[path = "../candidate_ui_lab_annotation.rs"]
mod candidate_ui_lab_annotation;

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_app::run() {
        windows_app::show_fatal_error(&error);
    }
}

#[cfg(windows)]
mod windows_app {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::path::PathBuf;

    use crate::candidate_ui::{
        CandidateRgb, CandidateScene, CandidateSceneFontMetrics, CandidateSceneLayout,
        CandidateSceneRect, CandidateSceneRequest, DEFAULT_CANDIDATE_VISUAL_SPEC,
        allocate_horizontal_candidate_widths, build_candidate_scene,
        candidate_horizontal_logical_width, candidate_ui_scale, candidate_vertical_logical_width,
    };
    use crate::candidate_ui_gdi::{
        CandidateSceneFonts, CandidateScenePaintContent, paint_candidate_scene,
    };
    use crate::candidate_ui_lab_annotation::{
        CandidateUiLabAnnotationContext, CandidateUiLabAnnotationSession,
        MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS, capture_candidate_ui_lab_annotation_context,
        export_candidate_ui_lab_annotations,
    };
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
        DeleteDC, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect, FrameRect,
        GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, TEXTMETRICW, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        EnableWindow, ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
        DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE,
        ES_WANTRETURN, GetClientRect, GetDlgItem, GetMessageW, GetWindowRect, GetWindowTextLengthW,
        GetWindowTextW, HMENU, IDC_ARROW, IsDialogMessageW, IsWindow, LoadCursorW, MB_ICONERROR,
        MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW, PostQuitMessage, RegisterClassW,
        SET_WINDOW_POS_FLAGS, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageW,
        SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_ERASEBKGND,
        WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WNDCLASSW, WS_BORDER,
        WS_CAPTION, WS_CHILD, WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP,
        WS_VISIBLE, WS_VSCROLL,
    };
    use windows::core::{PCWSTR, w};

    const DPIS: [u32; 4] = [96, 120, 144, 192];
    const MAX_CANDIDATE_CHARACTERS: usize = 32;
    const NOTE_EDIT_ID: i32 = 201;
    const NOTE_SAVE_ID: i32 = 202;
    const NOTE_CANCEL_ID: i32 = 203;
    const EDIT_SET_LIMIT_TEXT: u32 = 0x00c5;

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

        const fn stable_id(self) -> &'static str {
            match self {
                Self::Everyday => "everyday",
                Self::LongCandidate => "long-candidate",
                Self::Personalized => "personalized",
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
        drag_anchor: Option<(i32, i32)>,
        selection: Option<CandidateSceneRect>,
        annotations: CandidateUiLabAnnotationSession,
        pending_annotation: Option<CandidateUiLabAnnotationContext>,
        note_window: Option<HWND>,
        note_owner: Option<HWND>,
    }

    impl Default for LabState {
        fn default() -> Self {
            Self {
                layout: CandidateSceneLayout::Horizontal,
                dpi_index: 0,
                scenario_index: 0,
                last_hit: None,
                last_scene: None,
                drag_anchor: None,
                selection: None,
                annotations: CandidateUiLabAnnotationSession::default(),
                pending_annotation: None,
                note_window: None,
                note_owner: None,
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
            self.drag_anchor = None;
            self.selection = None;
            self.pending_annotation = None;
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
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).map_err(
                |_| "无法启用逐显示器 DPI；实验室拒绝显示可能被二次缩放的预览".to_owned(),
            )?;
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
            let note_class_name = w!("ZiranmaCandidateUiLabNoteWindow");
            let note_class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: note_class_name,
                lpfnWndProc: Some(note_window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)
                    .map_err(|_| "无法载入批注窗口光标".to_owned())?,
                ..Default::default()
            };
            if RegisterClassW(&note_class) == 0 {
                return Err("无法注册候选窗批注窗口".to_owned());
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
                let note_window = APP_STATE
                    .with(|slot| slot.borrow().as_ref().and_then(|state| state.note_window));
                if note_window.is_some_and(|window| IsDialogMessageW(window, &message).as_bool()) {
                    continue;
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
                if u32::try_from(wparam.0).unwrap_or_default() == 0x4e {
                    open_note_window(window);
                    return LRESULT(0);
                }
                if u32::try_from(wparam.0).unwrap_or_default() == 0x45 {
                    export_annotations(window);
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
                let point = message_point(lparam);
                APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(state) = slot.as_mut() else {
                        return;
                    };
                    state.drag_anchor = Some(point);
                    state.selection = Some(selection_rect(point, point));
                    state.last_hit = None;
                });
                unsafe {
                    let _ = SetCapture(window);
                }
                refresh_window(window, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let point = message_point(lparam);
                let changed = APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(state) = slot.as_mut() else {
                        return false;
                    };
                    let Some(anchor) = state.drag_anchor else {
                        return false;
                    };
                    state.selection = Some(selection_rect(anchor, point));
                    true
                });
                if changed {
                    invalidate_window(window);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let point = message_point(lparam);
                APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(state) = slot.as_mut() else {
                        return;
                    };
                    let Some(anchor) = state.drag_anchor.take() else {
                        return;
                    };
                    let selection = selection_rect(anchor, point);
                    state.selection = Some(selection);
                    state.last_hit = state
                        .last_scene
                        .as_ref()
                        .and_then(|scene| selection_summary(scene, selection));
                });
                unsafe {
                    let _ = ReleaseCapture();
                }
                refresh_window(window, false);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(window) };
                LRESULT(0)
            }
            WM_DESTROY => {
                let note_window = APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let state = slot.as_mut()?;
                    state.note_owner = None;
                    state.pending_annotation = None;
                    state.note_window.take()
                });
                if let Some(note_window) = note_window
                    .filter(|note_window| unsafe { IsWindow(Some(*note_window)) }.as_bool())
                {
                    let _ = unsafe { DestroyWindow(note_window) };
                }
                APP_STATE.with(|slot| slot.replace(None));
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn open_note_window(owner: HWND) {
        let existing =
            APP_STATE.with(|slot| slot.borrow().as_ref().and_then(|state| state.note_window));
        if existing.is_some_and(|window| unsafe { IsWindow(Some(window)) }.as_bool()) {
            if let Some(window) = existing {
                let _ = unsafe { SetForegroundWindow(window) };
            }
            return;
        }

        let context = APP_STATE.with(|slot| {
            let slot = slot.borrow();
            let state = slot
                .as_ref()
                .ok_or_else(|| "候选窗实验室尚未准备好".to_owned())?;
            let selection = state
                .selection
                .ok_or_else(|| "请先在候选窗上圈选需要批注的区域".to_owned())?;
            let scene = state
                .last_scene
                .as_ref()
                .ok_or_else(|| "候选窗预览尚未完成绘制".to_owned())?;
            capture_candidate_ui_lab_annotation_context(
                state.scenario().stable_id(),
                state.layout,
                state.dpi(),
                selection,
                scene,
                DEFAULT_CANDIDATE_VISUAL_SPEC,
            )
            .map_err(|error| error.to_string())
        });
        let context = match context {
            Ok(context) => context,
            Err(error) => return notify_error(owner, &error),
        };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.pending_annotation = Some(context);
                state.note_owner = Some(owner);
            }
        });

        let window = unsafe {
            let module = match GetModuleHandleW(None) {
                Ok(module) => module,
                Err(_) => {
                    clear_pending_note();
                    return notify_error(owner, "无法读取批注窗口模块");
                }
            };
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                w!("ZiranmaCandidateUiLabNoteWindow"),
                w!("给这个区域写一句批注"),
                WS_CAPTION | WS_SYSMENU,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                520,
                270,
                Some(owner),
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
        };
        let window = match window {
            Ok(window) => window,
            Err(_) => {
                clear_pending_note();
                return notify_error(owner, "无法创建批注输入窗口");
            }
        };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.note_window = Some(window);
            }
        });
        unsafe {
            let _ = EnableWindow(owner, false);
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetForegroundWindow(window);
            if let Ok(editor) = GetDlgItem(Some(window), NOTE_EDIT_ID) {
                let _ = SetFocus(Some(editor));
            }
        }
    }

    unsafe extern "system" fn note_window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                if create_note_controls(window) {
                    LRESULT(0)
                } else {
                    LRESULT(-1)
                }
            }
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    NOTE_SAVE_ID => save_note(window),
                    NOTE_CANCEL_ID => {
                        let _ = unsafe { DestroyWindow(window) };
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(window) };
                LRESULT(0)
            }
            WM_DESTROY => {
                let owner = APP_STATE.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let state = slot.as_mut()?;
                    if state.note_window != Some(window) {
                        return None;
                    }
                    state.note_window = None;
                    state.pending_annotation = None;
                    state.note_owner.take()
                });
                if let Some(owner) =
                    owner.filter(|owner| unsafe { IsWindow(Some(*owner)) }.as_bool())
                {
                    unsafe {
                        let _ = EnableWindow(owner, true);
                        let _ = SetForegroundWindow(owner);
                    }
                    refresh_window(owner, false);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn create_note_controls(window: HWND) -> bool {
        let instance = unsafe { GetModuleHandleW(None) }
            .ok()
            .map(|module| HINSTANCE(module.0));
        let controls = unsafe {
            [
                create_control(
                    window,
                    w!("STATIC"),
                    w!("这条批注只关联当前公开合成预览，并暂存在本次实验中。"),
                    20,
                    18,
                    470,
                    28,
                    0,
                    0,
                    instance,
                ),
                create_control(
                    window,
                    w!("EDIT"),
                    PCWSTR::null(),
                    20,
                    50,
                    470,
                    120,
                    NOTE_EDIT_ID,
                    WS_BORDER.0 as i32
                        | WS_TABSTOP.0 as i32
                        | WS_VSCROLL.0 as i32
                        | ES_LEFT
                        | ES_MULTILINE
                        | ES_AUTOVSCROLL
                        | ES_WANTRETURN,
                    instance,
                ),
                create_control(
                    window,
                    w!("BUTTON"),
                    w!("加入本次实验"),
                    276,
                    188,
                    104,
                    34,
                    NOTE_SAVE_ID,
                    BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32,
                    instance,
                ),
                create_control(
                    window,
                    w!("BUTTON"),
                    w!("取消"),
                    390,
                    188,
                    100,
                    34,
                    NOTE_CANCEL_ID,
                    BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                    instance,
                ),
            ]
        };
        if controls.iter().any(Result::is_err) {
            return false;
        }
        if let Ok(editor) = unsafe { GetDlgItem(Some(window), NOTE_EDIT_ID) } {
            let _ = unsafe {
                SendMessageW(
                    editor,
                    EDIT_SET_LIMIT_TEXT,
                    Some(WPARAM(MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS * 2)),
                    None,
                )
            };
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn create_control(
        parent: HWND,
        class_name: PCWSTR,
        label: PCWSTR,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        id: i32,
        extra_style: i32,
        instance: Option<HINSTANCE>,
    ) -> windows::core::Result<HWND> {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                label,
                WS_CHILD
                    | WS_VISIBLE
                    | WINDOW_STYLE(u32::try_from(extra_style).unwrap_or_default()),
                x,
                y,
                width,
                height,
                Some(parent),
                (id != 0).then_some(HMENU(id as usize as *mut c_void)),
                instance,
                None,
            )
        }
    }

    fn save_note(window: HWND) {
        let note = match read_note(window) {
            Ok(note) => note,
            Err(error) => return notify_error(window, &error),
        };
        let result = APP_STATE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot
                .as_mut()
                .ok_or_else(|| "候选窗实验室尚未准备好".to_owned())?;
            let context = state
                .pending_annotation
                .clone()
                .ok_or_else(|| "这次圈选已经失效，请重新圈选".to_owned())?;
            state
                .annotations
                .add(context, &note)
                .map_err(|error| error.to_string())?;
            debug_assert!(
                !state
                    .annotations
                    .annotations()
                    .last()
                    .expect("the just-added annotation exists")
                    .to_canonical_json()
                    .is_empty()
            );
            state.pending_annotation = None;
            Ok::<_, String>(())
        });
        match result {
            Ok(()) => {
                let _ = unsafe { DestroyWindow(window) };
            }
            Err(error) => notify_error(window, &error),
        }
    }

    fn read_note(window: HWND) -> Result<String, String> {
        let editor = unsafe { GetDlgItem(Some(window), NOTE_EDIT_ID) }
            .map_err(|_| "无法读取批注输入框".to_owned())?;
        let length = unsafe { GetWindowTextLengthW(editor) };
        let maximum = i32::try_from(MAX_CANDIDATE_UI_LAB_NOTE_CHARACTERS.saturating_mul(2))
            .unwrap_or(i32::MAX);
        if !(0..=maximum).contains(&length) {
            return Err("批注超过输入上限".to_owned());
        }
        let mut buffer = vec![0_u16; usize::try_from(length).unwrap_or(0).saturating_add(1)];
        let copied = unsafe { GetWindowTextW(editor, &mut buffer) };
        if copied < 0 {
            return Err("无法读取批注输入框".to_owned());
        }
        buffer.truncate(usize::try_from(copied).unwrap_or(0));
        String::from_utf16(&buffer).map_err(|_| "批注不是有效文字".to_owned())
    }

    fn clear_pending_note() {
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.pending_annotation = None;
                state.note_owner = None;
                state.note_window = None;
            }
        });
    }

    fn export_annotations(window: HWND) {
        let session = APP_STATE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|state| state.annotations.clone())
                .ok_or_else(|| "候选窗实验室尚未准备好".to_owned())
        });
        let session = match session {
            Ok(session) => session,
            Err(error) => return notify_error(window, &error),
        };
        match export_candidate_ui_lab_annotations(&session, &annotation_export_directory()) {
            Ok(path) => notify_information(
                window,
                &format!("已将 {} 条批注保存到：\n{}", session.len(), path.display()),
            ),
            Err(error) => notify_error(window, &error.to_string()),
        }
    }

    fn annotation_export_directory() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".local")
            .join("ui-lab")
            .join("wishes")
    }

    fn notify_error(window: HWND, message: &str) {
        let message = message.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        unsafe {
            let _ = MessageBoxW(
                Some(window),
                PCWSTR(message.as_ptr()),
                w!("候选窗实验室"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn notify_information(window: HWND, message: &str) {
        let message = message.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        unsafe {
            let _ = MessageBoxW(
                Some(window),
                PCWSTR(message.as_ptr()),
                w!("候选窗实验室"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    fn message_point(lparam: LPARAM) -> (i32, i32) {
        let packed = lparam.0 as u32;
        (
            i32::from(packed as u16 as i16),
            i32::from((packed >> 16) as u16 as i16),
        )
    }

    fn selection_rect(anchor: (i32, i32), point: (i32, i32)) -> CandidateSceneRect {
        CandidateSceneRect {
            left: anchor.0.min(point.0),
            top: anchor.1.min(point.1),
            right: anchor.0.max(point.0).saturating_add(1),
            bottom: anchor.1.max(point.1).saturating_add(1),
        }
    }

    fn selection_summary(scene: &CandidateScene, selection: CandidateSceneRect) -> Option<String> {
        let hits = scene.semantic_hits_in(selection);
        let first = hits.first()?;
        let identity = match first.candidate_index {
            Some(index) => format!("{} · 候选 {}", first.semantic.stable_id(), index + 1),
            None => first.semantic.stable_id().to_owned(),
        };
        Some(format!("{identity} · 共 {} 层", hits.len()))
    }

    fn invalidate_window(window: HWND) {
        // SAFETY: invalidation is bounded to this process-owned window and
        // suppresses background erasure because painting is double buffered.
        unsafe {
            let _ = InvalidateRect(Some(window), None, false);
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
        }
        invalidate_window(window);
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
        let annotations = if state.annotations.len() == 0 {
            String::new()
        } else {
            format!(" · 批注 {}（仅内存）", state.annotations.len())
        };
        format!(
            "候选窗实验室 · {} · {} DPI · {}{}{}    H 布局 / D 缩放 / S 场景 / 拖拽圈选 / N 批注 / E 导出 / Esc 退出",
            layout,
            state.dpi(),
            state.scenario().label(),
            hit,
            annotations,
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
        if let Some(selection) = snapshot.selection {
            unsafe {
                paint_selection_frame(
                    hdc,
                    selection,
                    DEFAULT_CANDIDATE_VISUAL_SPEC.selection_accent,
                );
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

    unsafe fn paint_selection_frame(hdc: HDC, bounds: CandidateSceneRect, color: CandidateRgb) {
        let bounds = RECT {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
        };
        let color = COLORREF(
            u32::from(color.red) | (u32::from(color.green) << 8) | (u32::from(color.blue) << 16),
        );
        let brush = unsafe { CreateSolidBrush(color) };
        if !brush.is_invalid() {
            unsafe {
                let _ = FrameRect(hdc, &bounds, brush);
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

        #[test]
        fn drag_selection_is_direction_independent_and_keeps_clicks_nonempty() {
            assert_eq!(
                selection_rect((8, 9), (3, 2)),
                CandidateSceneRect {
                    left: 3,
                    top: 2,
                    right: 9,
                    bottom: 10,
                }
            );
            assert_eq!(
                selection_rect((3, 2), (8, 9)),
                selection_rect((8, 9), (3, 2))
            );
            assert_eq!(
                selection_rect((5, 7), (5, 7)),
                CandidateSceneRect {
                    left: 5,
                    top: 7,
                    right: 6,
                    bottom: 8,
                }
            );
        }
    }
}
