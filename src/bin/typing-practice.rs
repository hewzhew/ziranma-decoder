#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("打字练习实验室目前只支持 Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_app::run() {
        windows_app::show_fatal_error(&error.to_string());
    }
}

#[cfg(windows)]
mod windows_app {
    use std::cell::RefCell;
    use std::error::Error;
    use std::ffi::c_void;
    use std::fmt;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::time::Instant;

    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_3DFACE, COLOR_GRAYTEXT, COLOR_WINDOW,
        COLOR_WINDOWTEXT, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, FF_DONTCARE,
        FW_NORMAL, FW_SEMIBOLD, GetSysColor, GetSysColorBrush, HGDIOBJ, OUT_DEFAULT_PRECIS,
        SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_F2};
    use windows::Win32::UI::WindowsAndMessaging::{
        BN_CLICKED, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EN_CHANGE,
        ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GetDlgItem, GetMessageW,
        GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IsDialogMessageW, LoadCursorW,
        MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostQuitMessage, RegisterClassW, SW_SHOW,
        SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow, TranslateMessage,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC,
        WM_DESTROY, WM_KEYDOWN, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
        WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };
    use windows::core::{PCWSTR, w};

    const TITLE_ID: i32 = 101;
    const DESCRIPTION_ID: i32 = 102;
    const STATUS_ID: i32 = 103;
    const EDITOR_LABEL_ID: i32 = 104;
    const EDITOR_ID: i32 = 105;
    const REVIEW_LABEL_ID: i32 = 106;
    const REVIEW_ID: i32 = 107;
    const START_ID: i32 = 108;
    const MARK_ID: i32 = 109;
    const FINISH_ID: i32 = 110;
    const FOOTER_ID: i32 = 111;

    const EDIT_LIMIT_TEXT: u32 = 0x00c5;
    const EDIT_GET_SELECTION: u32 = 0x00b0;
    const MAX_DOCUMENT_UTF16: usize = 32 * 1024;
    const MAX_TIMELINE_EVENTS: usize = 1_024;
    const MAX_CAPTURED_UTF16: usize = 64 * 1024;

    static HEADING_FONT: AtomicIsize = AtomicIsize::new(0);
    static BODY_FONT: AtomicIsize = AtomicIsize::new(0);
    static BUTTON_FONT: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    enum SessionMode {
        #[default]
        Idle,
        Recording,
        Review,
    }

    struct DocumentDelta {
        elapsed_ms: u64,
        start_utf16: usize,
        removed: Vec<u16>,
        inserted: Vec<u16>,
        selection_start_utf16: u32,
        selection_end_utf16: u32,
    }

    impl fmt::Debug for DocumentDelta {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("DocumentDelta")
                .field("elapsed_ms", &self.elapsed_ms)
                .field("start_utf16", &self.start_utf16)
                .field("removed_utf16", &self.removed.len())
                .field("inserted_utf16", &self.inserted.len())
                .field("selection_start_utf16", &self.selection_start_utf16)
                .field("selection_end_utf16", &self.selection_end_utf16)
                .finish()
        }
    }

    impl DocumentDelta {
        fn between(
            before: &[u16],
            after: &[u16],
            elapsed_ms: u64,
            selection: (u32, u32),
        ) -> Option<Self> {
            if before == after {
                return None;
            }
            let prefix = before
                .iter()
                .zip(after)
                .take_while(|(left, right)| left == right)
                .count();
            let remaining_before = before.len().saturating_sub(prefix);
            let remaining_after = after.len().saturating_sub(prefix);
            let suffix = before[prefix..]
                .iter()
                .rev()
                .zip(after[prefix..].iter().rev())
                .take_while(|(left, right)| left == right)
                .count()
                .min(remaining_before)
                .min(remaining_after);
            let before_end = before.len().saturating_sub(suffix);
            let after_end = after.len().saturating_sub(suffix);
            Some(Self {
                elapsed_ms,
                start_utf16: prefix,
                removed: before[prefix..before_end].to_vec(),
                inserted: after[prefix..after_end].to_vec(),
                selection_start_utf16: selection.0,
                selection_end_utf16: selection.1,
            })
        }

        fn captured_utf16(&self) -> usize {
            self.removed.len().saturating_add(self.inserted.len())
        }
    }

    #[derive(Debug)]
    struct TimelineMarker {
        elapsed_ms: u64,
        after_event: usize,
        selection_start_utf16: u32,
        selection_end_utf16: u32,
    }

    #[derive(Debug)]
    enum PracticeEvent {
        DocumentDelta(DocumentDelta),
        Marker(TimelineMarker),
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ReviewDocumentKind {
        Insert,
        Delete,
        Replace,
    }

    struct ReviewDocumentGroup {
        kind: ReviewDocumentKind,
        first_elapsed_ms: u64,
        last_elapsed_ms: u64,
        start_utf16: usize,
        removed: Vec<u16>,
        inserted: Vec<u16>,
        selection_start_utf16: u32,
        selection_end_utf16: u32,
        steps: usize,
    }

    impl ReviewDocumentGroup {
        fn from_delta(delta: &DocumentDelta) -> Self {
            Self {
                kind: delta_kind(delta),
                first_elapsed_ms: delta.elapsed_ms,
                last_elapsed_ms: delta.elapsed_ms,
                start_utf16: delta.start_utf16,
                removed: delta.removed.clone(),
                inserted: delta.inserted.clone(),
                selection_start_utf16: delta.selection_start_utf16,
                selection_end_utf16: delta.selection_end_utf16,
                steps: 1,
            }
        }

        fn try_extend(&mut self, delta: &DocumentDelta) -> bool {
            if self.kind != delta_kind(delta) {
                return false;
            }
            let merged = match self.kind {
                ReviewDocumentKind::Insert => {
                    if delta.start_utf16 == self.start_utf16.saturating_add(self.inserted.len()) {
                        self.inserted.extend_from_slice(&delta.inserted);
                        true
                    } else {
                        false
                    }
                }
                ReviewDocumentKind::Delete => {
                    if delta.start_utf16.saturating_add(delta.removed.len()) == self.start_utf16 {
                        let mut removed = delta.removed.clone();
                        removed.extend_from_slice(&self.removed);
                        self.removed = removed;
                        self.start_utf16 = delta.start_utf16;
                        true
                    } else if delta.start_utf16 == self.start_utf16 {
                        self.removed.extend_from_slice(&delta.removed);
                        true
                    } else {
                        false
                    }
                }
                ReviewDocumentKind::Replace => {
                    if delta.start_utf16 == self.start_utf16 && delta.removed == self.inserted {
                        self.inserted.clone_from(&delta.inserted);
                        true
                    } else {
                        false
                    }
                }
            };
            if merged {
                self.last_elapsed_ms = delta.elapsed_ms;
                self.selection_start_utf16 = delta.selection_start_utf16;
                self.selection_end_utf16 = delta.selection_end_utf16;
                self.steps = self.steps.saturating_add(1);
            }
            merged
        }
    }

    enum ReviewGroup<'a> {
        Document(ReviewDocumentGroup),
        Marker(&'a TimelineMarker),
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RecordOutcome {
        Ignored,
        Recorded,
        LimitReached,
    }

    #[derive(Default)]
    struct AppState {
        mode: SessionMode,
        started_at: Option<Instant>,
        last_text: Vec<u16>,
        events: Vec<PracticeEvent>,
        captured_utf16: usize,
        markers: usize,
    }

    impl AppState {
        fn begin(&mut self, current_text: Vec<u16>) {
            self.mode = SessionMode::Recording;
            self.started_at = Some(Instant::now());
            self.last_text = current_text;
            self.events.clear();
            self.captured_utf16 = 0;
            self.markers = 0;
        }

        fn elapsed_ms(&self) -> u64 {
            self.started_at
                .map(|started_at| started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
                .unwrap_or(0)
        }

        fn record_change(&mut self, after: Vec<u16>, selection: (u32, u32)) -> RecordOutcome {
            if self.mode != SessionMode::Recording {
                return RecordOutcome::Ignored;
            }
            let Some(delta) =
                DocumentDelta::between(&self.last_text, &after, self.elapsed_ms(), selection)
            else {
                return RecordOutcome::Ignored;
            };
            let captured_utf16 = delta.captured_utf16();
            self.last_text = after;
            if self.events.len() >= MAX_TIMELINE_EVENTS
                || self.captured_utf16.saturating_add(captured_utf16) > MAX_CAPTURED_UTF16
            {
                self.mode = SessionMode::Review;
                return RecordOutcome::LimitReached;
            }
            self.captured_utf16 = self.captured_utf16.saturating_add(captured_utf16);
            self.events.push(PracticeEvent::DocumentDelta(delta));
            RecordOutcome::Recorded
        }

        fn mark(&mut self, selection: (u32, u32)) -> RecordOutcome {
            if self.mode != SessionMode::Recording {
                return RecordOutcome::Ignored;
            }
            if self.events.len() >= MAX_TIMELINE_EVENTS {
                self.mode = SessionMode::Review;
                return RecordOutcome::LimitReached;
            }
            self.events.push(PracticeEvent::Marker(TimelineMarker {
                elapsed_ms: self.elapsed_ms(),
                after_event: self.events.len(),
                selection_start_utf16: selection.0,
                selection_end_utf16: selection.1,
            }));
            self.markers = self.markers.saturating_add(1);
            RecordOutcome::Recorded
        }

        fn finish(&mut self) {
            if self.mode == SessionMode::Recording {
                self.mode = SessionMode::Review;
            }
        }
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        // SAFETY: this process owns the registered class and message loop for
        // the complete lifetime of the one top-level window.
        unsafe {
            let module = GetModuleHandleW(None)?;
            let instance = HINSTANCE(module.0);
            let class_name = w!("ZiranmaTypingPracticeWindow");
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                hInstance: instance,
                lpszClassName: class_name,
                lpfnWndProc: Some(window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err("无法注册打字练习实验室窗口".into());
            }
            APP_STATE.with(|slot| slot.replace(Some(AppState::default())));
            let window = match CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class_name,
                w!("打字练习实验室"),
                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                840,
                760,
                None,
                None,
                Some(instance),
                None,
            ) {
                Ok(window) => window,
                Err(error) => {
                    APP_STATE.with(|slot| slot.replace(None));
                    return Err(error.into());
                }
            };
            update_session_controls(window, None);
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetForegroundWindow(window);
            focus_editor(window);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    return Err("打字练习实验室消息循环失败".into());
                }
                if result == 0 {
                    break;
                }
                if is_editor_f2(window, &message) {
                    mark_session(window);
                    continue;
                }
                if IsDialogMessageW(window, &message).as_bool() {
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
            WM_CREATE => {
                configure_window(window);
                if unsafe { create_controls(window) } {
                    LRESULT(0)
                } else {
                    LRESULT(-1)
                }
            }
            WM_CTLCOLORSTATIC => unsafe { paint_static(wparam, lparam) },
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as i32;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                match (id, notification) {
                    (EDITOR_ID, EN_CHANGE) => record_editor_change(window),
                    (START_ID, BN_CLICKED) => begin_session(window),
                    (MARK_ID, BN_CLICKED) => mark_session(window),
                    (FINISH_ID, BN_CLICKED) => finish_session(window, None),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(window) };
                LRESULT(0)
            }
            WM_DESTROY => {
                delete_fonts();
                APP_STATE.with(|slot| slot.replace(None));
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn configure_window(window: HWND) {
        let corner = DWMWCP_ROUND;
        let border: u32 = unsafe { GetSysColor(COLOR_3DFACE) };
        // SAFETY: both DWM calls receive fixed-size values for this live
        // process-owned window. Older systems may reject the refinements.
        unsafe {
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&corner).cast(),
                u32::try_from(std::mem::size_of_val(&corner)).unwrap_or(u32::MAX),
            );
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_BORDER_COLOR,
                std::ptr::from_ref(&border).cast(),
                u32::try_from(std::mem::size_of_val(&border)).unwrap_or(u32::MAX),
            );
        }
    }

    unsafe fn create_controls(window: HWND) -> bool {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return false;
        };
        let instance = Some(HINSTANCE(module.0));
        let heading_font = unsafe { create_font(21, FW_SEMIBOLD.0) };
        let body_font = unsafe { create_font(14, FW_NORMAL.0) };
        let button_font = unsafe { create_font(14, FW_SEMIBOLD.0) };
        if [heading_font, body_font, button_font]
            .iter()
            .any(|font| font.is_invalid())
        {
            for font in [heading_font, body_font, button_font] {
                if !font.is_invalid() {
                    let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
                }
            }
            return false;
        }
        HEADING_FONT.store(heading_font.0 as isize, Ordering::Release);
        BODY_FONT.store(body_font.0 as isize, Ordering::Release);
        BUTTON_FONT.store(button_font.0 as isize, Ordering::Release);

        let controls = [
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("在自己的练习区里，慢慢看清打字过程"),
                24,
                18,
                780,
                32,
                TITLE_ID,
                0,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("开始后只记录下面文本框的变化；可以随时标一下现场，结束后再统一查看。"),
                24,
                56,
                780,
                28,
                DESCRIPTION_ID,
                0,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("尚未开始 · 此时输入不会进入本轮时间线"),
                24,
                92,
                780,
                28,
                STATUS_ID,
                0,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("练习区"),
                24,
                132,
                780,
                24,
                EDITOR_LABEL_ID,
                0,
                instance,
            ),
            create_control(
                window,
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                PCWSTR::null(),
                24,
                158,
                780,
                260,
                EDITOR_ID,
                WS_TABSTOP.0 as i32
                    | WS_VSCROLL.0 as i32
                    | ES_LEFT
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_WANTRETURN,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("本轮时间线"),
                24,
                432,
                780,
                24,
                REVIEW_LABEL_ID,
                0,
                instance,
            ),
            create_control(
                window,
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!("结束本轮后，这里会按顺序显示插入、删除、替换和现场标记。"),
                24,
                458,
                780,
                138,
                REVIEW_ID,
                WS_VSCROLL.0 as i32 | ES_LEFT | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("开始本轮"),
                24,
                614,
                244,
                42,
                START_ID,
                BS_PUSHBUTTON | BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("标一下这里（F2）"),
                292,
                614,
                244,
                42,
                MARK_ID,
                BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("结束并查看"),
                560,
                614,
                244,
                42,
                FINISH_ID,
                BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("仅观察这个窗口 · 退出即清空 · 当前不写文件"),
                24,
                674,
                780,
                24,
                FOOTER_ID,
                0,
                instance,
            ),
        ];
        if controls.iter().any(Result::is_err) {
            delete_fonts();
            return false;
        }
        let controls = controls.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        for (index, control) in controls.iter().enumerate() {
            let font = if index == 0 {
                heading_font
            } else if (7..=9).contains(&index) {
                button_font
            } else {
                body_font
            };
            let _ = unsafe {
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                )
            };
        }
        if let Ok(editor) = unsafe { GetDlgItem(Some(window), EDITOR_ID) } {
            let _ = unsafe {
                SendMessageW(
                    editor,
                    EDIT_LIMIT_TEXT,
                    Some(WPARAM(MAX_DOCUMENT_UTF16)),
                    None,
                )
            };
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn create_control(
        window: HWND,
        extended_style: WINDOW_EX_STYLE,
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
        // SAFETY: parent, class and label remain valid for the synchronous
        // standard-control creation call.
        unsafe {
            CreateWindowExW(
                extended_style,
                class_name,
                label,
                control_style(extra_style),
                x,
                y,
                width,
                height,
                Some(window),
                Some(control_menu(id)),
                instance,
                None,
            )
        }
    }

    unsafe fn create_font(height: i32, weight: u32) -> windows::Win32::Graphics::Gdi::HFONT {
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

    fn delete_fonts() {
        for font in [&HEADING_FONT, &BODY_FONT, &BUTTON_FONT] {
            let handle = font.swap(0, Ordering::AcqRel);
            if handle != 0 {
                // SAFETY: these process-owned fonts are no longer selected
                // after the parent and child windows begin destruction.
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(handle as *mut c_void));
                }
            }
        }
    }

    unsafe fn paint_static(wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        let id = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(control) };
        let color = COLORREF(match id {
            DESCRIPTION_ID | FOOTER_ID => unsafe { GetSysColor(COLOR_GRAYTEXT) },
            _ => unsafe { GetSysColor(COLOR_WINDOWTEXT) },
        });
        unsafe {
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(hdc, color);
        }
        LRESULT(unsafe { GetSysColorBrush(COLOR_WINDOW) }.0 as isize)
    }

    fn begin_session(window: HWND) {
        let Ok(current_text) = read_control_utf16(window, EDITOR_ID) else {
            return notify_error(window, "无法读取练习区文字");
        };
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.begin(current_text);
            }
        });
        set_control_text(
            window,
            REVIEW_ID,
            "本轮正在进行。结束后再统一显示时间线，不在打字时判断好坏。",
        );
        update_session_controls(window, None);
        focus_editor(window);
    }

    fn record_editor_change(window: HWND) {
        let Ok(after) = read_control_utf16(window, EDITOR_ID) else {
            return;
        };
        let selection = read_editor_selection(window).unwrap_or((0, 0));
        let outcome = APP_STATE.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|state| state.record_change(after, selection))
                .unwrap_or(RecordOutcome::Ignored)
        });
        match outcome {
            RecordOutcome::Recorded => update_session_controls(window, None),
            RecordOutcome::LimitReached => finish_session(
                window,
                Some("本轮已达到有界记录上限，已自动停止并保留现有时间线。"),
            ),
            RecordOutcome::Ignored => {}
        }
    }

    fn mark_session(window: HWND) {
        let selection = read_editor_selection(window).unwrap_or((0, 0));
        let outcome = APP_STATE.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|state| state.mark(selection))
                .unwrap_or(RecordOutcome::Ignored)
        });
        match outcome {
            RecordOutcome::Recorded => update_session_controls(window, Some("已标记这个现场。")),
            RecordOutcome::LimitReached => finish_session(
                window,
                Some("本轮已达到有界记录上限，已自动停止并保留现有时间线。"),
            ),
            RecordOutcome::Ignored => {}
        }
        focus_editor(window);
    }

    fn finish_session(window: HWND, notice: Option<&str>) {
        APP_STATE.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.finish();
            }
        });
        render_review(window);
        update_session_controls(window, notice);
    }

    fn update_session_controls(window: HWND, notice: Option<&str>) {
        let (mode, events, markers) = APP_STATE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|state| (state.mode, state.events.len(), state.markers))
                .unwrap_or((SessionMode::Idle, 0, 0))
        });
        let status = notice.map(str::to_owned).unwrap_or_else(|| match mode {
            SessionMode::Idle => "尚未开始 · 此时输入不会进入本轮时间线".to_owned(),
            SessionMode::Recording => {
                format!("正在记录这个练习区 · {events} 条事实 · {markers} 个现场标记")
            }
            SessionMode::Review => {
                format!("本轮已停止 · {events} 条事实 · {markers} 个现场标记")
            }
        });
        set_control_text(window, STATUS_ID, &status);
        set_control_text(
            window,
            START_ID,
            if mode == SessionMode::Review {
                "开始新一轮"
            } else {
                "开始本轮"
            },
        );
        set_enabled(window, START_ID, mode != SessionMode::Recording);
        set_enabled(window, MARK_ID, mode == SessionMode::Recording);
        set_enabled(window, FINISH_ID, mode == SessionMode::Recording);
        set_enabled(window, EDITOR_ID, mode != SessionMode::Review);
    }

    fn render_review(window: HWND) {
        let review = APP_STATE.with(|slot| {
            let state = slot.borrow();
            let Some(state) = state.as_ref() else {
                return String::new();
            };
            render_timeline(&state.events)
        });
        set_control_text(window, REVIEW_ID, &review);
    }

    fn render_timeline(events: &[PracticeEvent]) -> String {
        if events.is_empty() {
            return "本轮没有观察到文本变化或现场标记。".to_owned();
        }
        let groups = group_timeline(events);
        let mut output = String::new();
        for (index, group) in groups.iter().enumerate() {
            if index > 0 {
                output.push_str("\r\n");
            }
            match group {
                ReviewGroup::Document(document) => {
                    let removed = String::from_utf16_lossy(&document.removed);
                    let inserted = String::from_utf16_lossy(&document.inserted);
                    let step_label = if document.steps > 1 {
                        format!("{} 次连续", document.steps)
                    } else {
                        String::new()
                    };
                    let action = match document.kind {
                        ReviewDocumentKind::Insert => format!("{step_label}插入“{inserted}”"),
                        ReviewDocumentKind::Delete => format!("{step_label}删除“{removed}”"),
                        ReviewDocumentKind::Replace => {
                            format!("{step_label}把“{removed}”改为“{inserted}”")
                        }
                    };
                    let elapsed = if document.first_elapsed_ms == document.last_elapsed_ms {
                        format!("+{} ms", document.first_elapsed_ms)
                    } else {
                        format!(
                            "+{}–{} ms",
                            document.first_elapsed_ms, document.last_elapsed_ms
                        )
                    };
                    output.push_str(&format!(
                        "{} · {} · {} · 起点 {} · 光标 {}–{}",
                        index + 1,
                        elapsed,
                        action,
                        document.start_utf16,
                        document.selection_start_utf16,
                        document.selection_end_utf16,
                    ));
                }
                ReviewGroup::Marker(marker) => output.push_str(&format!(
                    "{} · +{} ms · ★ 现场标记 · 位于第 {} 条事实后 · 范围 {}–{}",
                    index + 1,
                    marker.elapsed_ms,
                    marker.after_event,
                    marker.selection_start_utf16,
                    marker.selection_end_utf16,
                )),
            }
        }
        output
    }

    fn group_timeline(events: &[PracticeEvent]) -> Vec<ReviewGroup<'_>> {
        let mut groups = Vec::<ReviewGroup<'_>>::new();
        for event in events {
            match event {
                PracticeEvent::DocumentDelta(delta) => {
                    if let Some(ReviewGroup::Document(group)) = groups.last_mut()
                        && group.try_extend(delta)
                    {
                        continue;
                    }
                    groups.push(ReviewGroup::Document(ReviewDocumentGroup::from_delta(
                        delta,
                    )));
                }
                PracticeEvent::Marker(marker) => groups.push(ReviewGroup::Marker(marker)),
            }
        }
        groups
    }

    fn delta_kind(delta: &DocumentDelta) -> ReviewDocumentKind {
        match (delta.removed.is_empty(), delta.inserted.is_empty()) {
            (true, false) => ReviewDocumentKind::Insert,
            (false, true) => ReviewDocumentKind::Delete,
            (false, false) => ReviewDocumentKind::Replace,
            (true, true) => unreachable!("document delta always changes text"),
        }
    }

    fn read_control_utf16(window: HWND, id: i32) -> Result<Vec<u16>, ()> {
        let control = unsafe { GetDlgItem(Some(window), id) }.map_err(|_| ())?;
        let length = unsafe { GetWindowTextLengthW(control) };
        if length < 0 || usize::try_from(length).unwrap_or(usize::MAX) > MAX_DOCUMENT_UTF16 {
            return Err(());
        }
        let mut buffer = vec![0_u16; usize::try_from(length).unwrap_or(0).saturating_add(1)];
        let copied = unsafe { GetWindowTextW(control, &mut buffer) };
        if copied < 0 {
            return Err(());
        }
        buffer.truncate(usize::try_from(copied).unwrap_or(0));
        Ok(buffer)
    }

    fn read_editor_selection(window: HWND) -> Result<(u32, u32), ()> {
        let editor = unsafe { GetDlgItem(Some(window), EDITOR_ID) }.map_err(|_| ())?;
        let mut start = 0_u32;
        let mut end = 0_u32;
        // SAFETY: the standard EDIT control writes at most one u32 to each
        // valid process-local pointer during this synchronous message.
        unsafe {
            let _ = SendMessageW(
                editor,
                EDIT_GET_SELECTION,
                Some(WPARAM(std::ptr::from_mut(&mut start) as usize)),
                Some(LPARAM(std::ptr::from_mut(&mut end) as isize)),
            );
        }
        Ok((start, end))
    }

    fn is_editor_f2(window: HWND, message: &MSG) -> bool {
        message.message == WM_KEYDOWN
            && message.wParam == WPARAM(VK_F2.0 as usize)
            && unsafe { GetDlgItem(Some(window), EDITOR_ID) }.ok() == Some(message.hwnd)
    }

    fn set_control_text(window: HWND, id: i32, text: &str) {
        if let Ok(control) = unsafe { GetDlgItem(Some(window), id) } {
            let text = wide(text);
            let _ = unsafe { SetWindowTextW(control, PCWSTR(text.as_ptr())) };
        }
    }

    fn set_enabled(window: HWND, id: i32, enabled: bool) {
        if let Ok(control) = unsafe { GetDlgItem(Some(window), id) } {
            let _ = unsafe { EnableWindow(control, enabled) };
        }
    }

    fn focus_editor(window: HWND) {
        if let Ok(editor) = unsafe { GetDlgItem(Some(window), EDITOR_ID) } {
            let _ = unsafe { SetFocus(Some(editor)) };
        }
    }

    fn notify_error(window: HWND, message: &str) {
        let message = wide(message);
        // SAFETY: the UTF-16 message remains alive for the synchronous dialog.
        unsafe {
            let _ = MessageBoxW(
                Some(window),
                PCWSTR(message.as_ptr()),
                w!("打字练习实验室"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    pub fn show_fatal_error(message: &str) {
        let message = wide(message);
        // SAFETY: both strings remain alive for the synchronous dialog.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                w!("打字练习实验室"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn control_style(extra: i32) -> WINDOW_STYLE {
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(u32::try_from(extra).unwrap_or_default())
    }

    fn control_menu(id: i32) -> HMENU {
        HMENU(id as usize as *mut c_void)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::{
            AppState, DocumentDelta, PracticeEvent, RecordOutcome, SessionMode, group_timeline,
            render_timeline,
        };

        #[test]
        fn utf16_delta_distinguishes_append_tail_trim_and_replacement() {
            let append = DocumentDelta::between(&[], &wide("线束缚"), 12, (3, 3)).unwrap();
            assert_eq!(append.start_utf16, 0);
            assert!(append.removed.is_empty());
            assert_eq!(String::from_utf16(&append.inserted).unwrap(), "线束缚");

            let trim = DocumentDelta::between(&wide("线束缚"), &wide("线束"), 24, (2, 2)).unwrap();
            assert_eq!(trim.start_utf16, 2);
            assert_eq!(String::from_utf16(&trim.removed).unwrap(), "缚");
            assert!(trim.inserted.is_empty());

            let replace =
                DocumentDelta::between(&wide("甲😀乙"), &wide("甲猫乙"), 36, (2, 2)).unwrap();
            assert_eq!(replace.start_utf16, 1);
            assert_eq!(replace.removed.len(), 2, "emoji occupies one UTF-16 pair");
            assert_eq!(String::from_utf16(&replace.inserted).unwrap(), "猫");
        }

        #[test]
        fn session_ignores_prestart_text_and_groups_recorded_facts_in_memory() {
            let mut state = AppState::default();
            assert_eq!(
                state.record_change(wide("开始前"), (3, 3)),
                RecordOutcome::Ignored
            );
            state.begin(wide("开始前"));
            assert_eq!(
                state.record_change(wide("开始前甲"), (4, 4)),
                RecordOutcome::Recorded
            );
            assert_eq!(state.mark((4, 4)), RecordOutcome::Recorded);
            assert_eq!(
                state.record_change(wide("开始前"), (3, 3)),
                RecordOutcome::Recorded
            );
            state.finish();

            assert_eq!(state.mode, SessionMode::Review);
            assert_eq!(state.events.len(), 3);
            assert_eq!(state.markers, 1);
            let review = render_timeline(&state.events);
            assert!(review.contains("插入“甲”"));
            assert!(review.contains("★ 现场标记"));
            assert!(review.contains("删除“甲”"));
        }

        #[test]
        fn delta_debug_never_contains_the_captured_text() {
            let delta = DocumentDelta::between(&wide("秘密"), &wide("心意"), 5, (2, 2)).unwrap();
            let debug = format!("{delta:?}");
            assert!(!debug.contains("秘密"));
            assert!(!debug.contains("心意"));
            assert!(debug.contains("removed_utf16"));
            assert!(debug.contains("inserted_utf16"));

            let timeline = vec![PracticeEvent::DocumentDelta(delta)];
            assert!(render_timeline(&timeline).contains("“秘密”改为“心意”"));
        }

        #[test]
        fn session_stops_before_exceeding_either_in_memory_bound() {
            let mut event_bound = AppState::default();
            event_bound.begin(Vec::new());
            for _ in 0..super::MAX_TIMELINE_EVENTS {
                assert_eq!(event_bound.mark((0, 0)), RecordOutcome::Recorded);
            }
            assert_eq!(event_bound.mark((0, 0)), RecordOutcome::LimitReached);
            assert_eq!(event_bound.mode, SessionMode::Review);
            assert_eq!(event_bound.events.len(), super::MAX_TIMELINE_EVENTS);

            let mut text_bound = AppState::default();
            text_bound.begin(Vec::new());
            assert_eq!(
                text_bound.record_change(vec![b'a' as u16; 32 * 1024], (0, 0)),
                RecordOutcome::Recorded
            );
            assert_eq!(
                text_bound.record_change(vec![b'b' as u16; 32 * 1024], (0, 0)),
                RecordOutcome::LimitReached
            );
            assert_eq!(text_bound.mode, SessionMode::Review);
            assert_eq!(text_bound.events.len(), 1);
        }

        #[test]
        fn review_groups_only_structurally_continuous_same_kind_edits() {
            let mut state = AppState::default();
            state.begin(Vec::new());
            assert_eq!(
                state.record_change(wide("你"), (1, 1)),
                RecordOutcome::Recorded
            );
            assert_eq!(
                state.record_change(wide("你好"), (2, 2)),
                RecordOutcome::Recorded
            );
            assert_eq!(state.mark((2, 2)), RecordOutcome::Recorded);
            assert_eq!(
                state.record_change(wide("你好呀"), (3, 3)),
                RecordOutcome::Recorded
            );
            assert_eq!(
                state.record_change(wide("你好"), (2, 2)),
                RecordOutcome::Recorded
            );

            let groups = group_timeline(&state.events);
            assert_eq!(
                groups.len(),
                4,
                "marker and insert-then-delete are boundaries"
            );
            let review = render_timeline(&state.events);
            assert!(review.contains("2 次连续插入“你好”"));
            assert!(review.contains("★ 现场标记"));
            assert!(review.contains("插入“呀”"));
            assert!(review.contains("删除“呀”"));
        }

        #[test]
        fn review_collapses_same_span_replacement_evolution_to_the_final_text() {
            let first = DocumentDelta::between(&wide("原"), &wide("中间"), 10, (2, 2)).unwrap();
            let second = DocumentDelta::between(&wide("中间"), &wide("最终"), 20, (2, 2)).unwrap();
            let events = vec![
                PracticeEvent::DocumentDelta(first),
                PracticeEvent::DocumentDelta(second),
            ];
            let review = render_timeline(&events);
            assert!(review.contains("2 次连续把“原”改为“最终”"));
            assert!(!review.contains("中间"));
            assert!(review.contains("+10–20 ms"));
        }

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().collect()
        }
    }
}
