#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("猫猫应愿目前只支持 Windows");
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
    use std::error::Error;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::time::{Duration, SystemTime};

    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_3DFACE, COLOR_GRAYTEXT,
        COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontW,
        CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawFocusRect, DrawTextW, EndPaint,
        FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect, GetStockObject, GetSysColor,
        GetSysColorBrush, HGDIOBJ, InvalidateRect, NULL_BRUSH, OPAQUE, OUT_DEFAULT_PRECIS,
        PAINTSTRUCT, PS_SOLID, RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor,
        TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::SystemServices::SS_CENTER;
    use windows::Win32::UI::Controls::{
        DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_NOFOCUSRECT, ODS_SELECTED,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        BS_DEFPUSHBUTTON, BS_OWNERDRAW, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CW_USEDEFAULT,
        CreateWindowExW, DI_NORMAL, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW,
        DrawIconEx, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE, ES_READONLY, ES_WANTRETURN, FindWindowW,
        GetDlgCtrlID, GetDlgItem, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU,
        IDC_ARROW, IDI_APPLICATION, IMAGE_ICON, IsDialogMessageW, LB_ADDSTRING, LB_GETCURSEL,
        LB_RESETCONTENT, LB_SETCURSEL, LB_SETITEMHEIGHT, LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT,
        LBS_NOTIFY, LBS_OWNERDRAWFIXED, LR_DEFAULTCOLOR, LoadCursorW, LoadIconW, LoadImageW,
        MB_ICONERROR, MB_ICONWARNING, MB_OK, MSG, MessageBoxW, PostMessageW, PostQuitMessage,
        RegisterClassW, SW_HIDE, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowTextW,
        ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE,
        WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM, WM_NCDESTROY, WM_PAINT,
        WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT,
        WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };
    use windows::core::{PCWSTR, w};
    use ziranma_core::{
        NativeAutomaticTranspositionDecision, NativeAutomaticTranspositionOutcome,
        NativeAutomaticTranspositionTier, NativeFeedbackEvent, WindowsUserDataProtector,
        WishCaptureScope, WishCategory, WishEventRole, WishFeedbackError, WishNote,
        WishPackageInfo, list_wish_packages, load_wish_note, load_wish_snapshot,
        repository_root_for_user_tool_executable, save_or_replace_wish_note,
    };

    const WISHPAD_ICON_RESOURCE_ID: usize = 101;
    const WISHPAD_LISTENING_ICON_RESOURCE_ID: usize = 102;
    const WISHPAD_ORGANIZING_ICON_RESOURCE_ID: usize = 104;
    const OPEN_MANAGER_MESSAGE: u32 = WM_APP + 38;
    const RECORDS_CHANGED_MESSAGE: u32 = WM_APP + 39;
    const MANAGER_TITLE_ID: i32 = 101;
    const MANAGER_DESCRIPTION_ID: i32 = 102;
    const MANAGER_LIST_ID: i32 = 103;
    const MANAGER_DETAIL_TITLE_ID: i32 = 104;
    const MANAGER_DETAIL_ID: i32 = 105;
    const MANAGER_EDIT_ID: i32 = 106;
    const MANAGER_REFRESH_ID: i32 = 107;
    const MANAGER_CONTEXT_ID: i32 = 108;
    const MANAGER_STATUS_ID: i32 = 109;
    const MANAGER_LIST_TITLE_ID: i32 = 110;
    const MANAGER_EMPTY_TITLE_ID: i32 = 111;
    const MANAGER_EMPTY_BODY_ID: i32 = 112;
    const NOTE_CATEGORY_ID: i32 = 201;
    const NOTE_EDIT_ID: i32 = 202;
    const NOTE_SAVE_ID: i32 = 203;
    const NOTE_CANCEL_ID: i32 = 204;
    const NOTE_TITLE_ID: i32 = 205;
    const NOTE_SUBTITLE_ID: i32 = 206;
    const NOTE_CATEGORY_LABEL_ID: i32 = 207;
    const NOTE_BODY_LABEL_ID: i32 = 208;
    const NOTE_FOOTNOTE_ID: i32 = 209;
    const EDIT_SET_LIMIT_TEXT: u32 = 0x00c5;
    const NOTE_TEXT_CHARACTER_LIMIT: usize = 2_048;
    static MANAGER_HEADING_FONT: AtomicIsize = AtomicIsize::new(0);
    static MANAGER_BODY_FONT: AtomicIsize = AtomicIsize::new(0);
    static MANAGER_SECTION_FONT: AtomicIsize = AtomicIsize::new(0);
    static MANAGER_BUTTON_FONT: AtomicIsize = AtomicIsize::new(0);
    static MANAGER_STATE: Mutex<Option<ManagerState>> = Mutex::new(None);
    static NOTE_TARGET: Mutex<Option<NoteTarget>> = Mutex::new(None);

    struct ManagerRecord {
        info: WishPackageInfo,
        note: Option<WishNote>,
        note_unavailable: bool,
    }

    struct ManagerState {
        root: PathBuf,
        records: Vec<ManagerRecord>,
        selected: Option<usize>,
        show_context: bool,
    }

    #[derive(Clone)]
    struct NoteTarget {
        owner: isize,
        root: PathBuf,
        wish_id: String,
        existing: Option<WishNote>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ManagerButtonTone {
        Primary,
        Secondary,
    }

    fn manager_button_tone(id: i32) -> Option<ManagerButtonTone> {
        match id {
            MANAGER_EDIT_ID | NOTE_SAVE_ID => Some(ManagerButtonTone::Primary),
            MANAGER_REFRESH_ID | MANAGER_CONTEXT_ID | NOTE_CANCEL_ID => {
                Some(ManagerButtonTone::Secondary)
            }
            _ => None,
        }
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        // SAFETY: this process owns both registered classes and all controls
        // for the duration of the message loop below.
        unsafe {
            let module = GetModuleHandleW(None)?;
            let instance = HINSTANCE(module.0);
            let icon = load_wishpad_icon(instance).or_else(|_| LoadIconW(None, IDI_APPLICATION))?;
            let class_name = w!("ZiranmaWishpadWindow");
            if let Ok(existing) = FindWindowW(class_name, PCWSTR::null()) {
                let _ = SendMessageW(existing, OPEN_MANAGER_MESSAGE, None, None);
                let _ = SetForegroundWindow(existing);
                return Ok(());
            }
            let class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: class_name,
                lpfnWndProc: Some(window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: icon,
                hbrBackground: GetSysColorBrush(COLOR_3DFACE),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err("无法注册猫猫应愿窗口".into());
            }
            let note_class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: w!("ZiranmaWishpadNoteWindow"),
                lpfnWndProc: Some(note_window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: icon,
                hbrBackground: GetSysColorBrush(COLOR_3DFACE),
                ..Default::default()
            };
            if RegisterClassW(&note_class) == 0 {
                return Err("无法注册许愿整理窗口".into());
            }
            let window = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class_name,
                w!("猫猫应愿"),
                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                820,
                560,
                None,
                None,
                Some(instance),
                None,
            )?;
            show_manager(window);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    return Err("猫猫应愿消息循环失败".into());
                }
                if result == 0 {
                    break;
                }
                if let Ok(note_window) = FindWindowW(w!("ZiranmaWishpadNoteWindow"), PCWSTR::null())
                    && IsDialogMessageW(note_window, &message).as_bool()
                {
                    continue;
                }
                if IsDialogMessageW(window, &message).as_bool() {
                    continue;
                }
                let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&message);
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
            OPEN_MANAGER_MESSAGE => {
                unsafe { show_manager(window) };
                refresh_records(window, selected_wish_id());
                LRESULT(0)
            }
            RECORDS_CHANGED_MESSAGE => {
                refresh_records(window, selected_wish_id());
                set_manager_status(window, "整理已保存");
                LRESULT(0)
            }
            WM_CREATE => {
                configure_manager_window(window);
                if unsafe { create_manager_controls(window) } {
                    refresh_records(window, None);
                    LRESULT(0)
                } else {
                    LRESULT(-1)
                }
            }
            WM_PAINT => {
                unsafe { paint_manager_background(window) };
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => unsafe { paint_manager_static(wparam, lparam) },
            WM_CTLCOLORLISTBOX => unsafe { paint_manager_listbox(wparam) },
            WM_DRAWITEM => {
                if unsafe { draw_manager_list_item(lparam) }
                    || unsafe { draw_manager_button(lparam) }
                {
                    LRESULT(1)
                } else {
                    unsafe { DefWindowProcW(window, message, wparam, lparam) }
                }
            }
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    MANAGER_LIST_ID => select_from_list(window),
                    MANAGER_EDIT_ID => open_note_window(window),
                    MANAGER_REFRESH_ID => refresh_records(window, selected_wish_id()),
                    MANAGER_CONTEXT_ID => toggle_context(window),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(window) };
                LRESULT(0)
            }
            WM_DESTROY => {
                if let Ok(mut state) = MANAGER_STATE.lock() {
                    *state = None;
                }
                delete_manager_fonts();
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn configure_manager_window(window: HWND) {
        let corner = DWMWCP_ROUND;
        let border: u32 = unsafe { GetSysColor(COLOR_3DFACE) };
        // SAFETY: both attributes are fixed-size values valid for these
        // synchronous best-effort calls.
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

    unsafe fn paint_manager_background(window: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(window, &mut paint) };
        let background = unsafe { GetSysColorBrush(COLOR_3DFACE) };
        let mut client = RECT::default();
        let _ =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut client) };
        unsafe {
            let _ = FillRect(hdc, &client, background);
        }
        let has_records = MANAGER_STATE
            .lock()
            .ok()
            .and_then(|state| state.as_ref().map(|state| !state.records.is_empty()))
            .unwrap_or(false);
        if has_records {
            unsafe {
                draw_manager_card(
                    hdc,
                    RECT {
                        left: 24,
                        top: 98,
                        right: 278,
                        bottom: 452,
                    },
                );
                draw_manager_card(
                    hdc,
                    RECT {
                        left: 294,
                        top: 98,
                        right: 780,
                        bottom: 400,
                    },
                );
            }
        } else if let Ok(module) = unsafe { GetModuleHandleW(None) }
            && let Ok(icon) = unsafe {
                load_wishpad_illustration(
                    HINSTANCE(module.0),
                    WISHPAD_LISTENING_ICON_RESOURCE_ID,
                    88,
                )
            }
        {
            let _ = unsafe { DrawIconEx(hdc, 358, 130, icon, 88, 88, 0, None, DI_NORMAL) };
            let _ = unsafe { DestroyIcon(icon) };
        }
        unsafe {
            let _ = EndPaint(window, &paint);
        }
    }

    unsafe fn draw_manager_card(hdc: windows::Win32::Graphics::Gdi::HDC, rect: RECT) {
        let brush = unsafe { CreateSolidBrush(COLORREF(GetSysColor(COLOR_WINDOW))) };
        let pen = unsafe { CreatePen(PS_SOLID, 1, COLORREF(0x00d8_d8d8)) };
        let old_brush = unsafe { SelectObject(hdc, HGDIOBJ(brush.0)) };
        let old_pen = unsafe { SelectObject(hdc, HGDIOBJ(pen.0)) };
        unsafe {
            let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 12, 12);
            let _ = SelectObject(hdc, old_pen);
            let _ = SelectObject(hdc, old_brush);
            let _ = DeleteObject(HGDIOBJ(pen.0));
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    unsafe fn create_manager_font(
        height: i32,
        weight: u32,
    ) -> windows::Win32::Graphics::Gdi::HFONT {
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

    fn store_manager_font(slot: &AtomicIsize, font: windows::Win32::Graphics::Gdi::HFONT) {
        slot.store(font.0 as isize, Ordering::Release);
    }

    fn load_manager_font(slot: &AtomicIsize) -> windows::Win32::Graphics::Gdi::HFONT {
        windows::Win32::Graphics::Gdi::HFONT(slot.load(Ordering::Acquire) as *mut c_void)
    }

    fn delete_manager_fonts() {
        for slot in [
            &MANAGER_HEADING_FONT,
            &MANAGER_BODY_FONT,
            &MANAGER_SECTION_FONT,
            &MANAGER_BUTTON_FONT,
        ] {
            let handle = slot.swap(0, Ordering::AcqRel);
            if handle != 0 {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(handle as *mut c_void));
                }
            }
        }
    }

    unsafe fn paint_manager_static(wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        let id = unsafe { GetDlgCtrlID(control) };
        let color = COLORREF(match id {
            MANAGER_STATUS_ID | MANAGER_DESCRIPTION_ID | MANAGER_EMPTY_BODY_ID => unsafe {
                GetSysColor(COLOR_GRAYTEXT)
            },
            _ => unsafe { GetSysColor(COLOR_WINDOWTEXT) },
        });
        let on_card = matches!(
            id,
            MANAGER_LIST_TITLE_ID | MANAGER_DETAIL_TITLE_ID | MANAGER_DETAIL_ID
        );
        let background = if on_card { COLOR_WINDOW } else { COLOR_3DFACE };
        unsafe {
            let background_mode = if manager_static_needs_opaque_background(id) {
                OPAQUE
            } else {
                TRANSPARENT
            };
            let _ = SetBkMode(hdc, background_mode);
            let _ = SetBkColor(hdc, COLORREF(GetSysColor(background)));
            let _ = SetTextColor(hdc, color);
        }
        LRESULT(unsafe { GetSysColorBrush(background) }.0 as isize)
    }

    fn manager_static_needs_opaque_background(id: i32) -> bool {
        // Read-only EDIT controls also send WM_CTLCOLORSTATIC. Unlike ordinary
        // STATIC labels, their changing multiline contents must erase the
        // previous glyphs before repainting or refreshes leave text stacked.
        id == MANAGER_DETAIL_ID
    }

    unsafe fn paint_manager_listbox(wparam: WPARAM) -> LRESULT {
        let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut c_void);
        unsafe {
            let _ = SetBkColor(hdc, COLORREF(GetSysColor(COLOR_WINDOW)));
            let _ = SetTextColor(hdc, COLORREF(GetSysColor(COLOR_WINDOWTEXT)));
        }
        LRESULT(unsafe { GetSysColorBrush(COLOR_WINDOW) }.0 as isize)
    }

    unsafe fn draw_manager_list_item(lparam: LPARAM) -> bool {
        if lparam.0 == 0 {
            return false;
        }
        let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
        if item.CtlID != MANAGER_LIST_ID as u32 {
            return false;
        }
        let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
        let fill = unsafe { GetSysColorBrush(if selected { COLOR_3DFACE } else { COLOR_WINDOW }) };
        unsafe {
            let _ = FillRect(item.hDC, &item.rcItem, fill);
        }
        let Ok(index) = usize::try_from(item.itemID) else {
            return true;
        };
        let lines = MANAGER_STATE.lock().ok().and_then(|state| {
            let record = state.as_ref()?.records.get(index)?;
            Some(manager_record_lines(record, SystemTime::now()))
        });
        let Some((primary, secondary)) = lines else {
            return true;
        };
        let mut primary = primary.encode_utf16().collect::<Vec<_>>();
        let mut secondary = secondary.encode_utf16().collect::<Vec<_>>();
        if selected {
            let accent = unsafe { GetSysColorBrush(COLOR_HIGHLIGHT) };
            let accent_rect = RECT {
                left: item.rcItem.left,
                top: item.rcItem.top.saturating_add(8),
                right: item.rcItem.left.saturating_add(3),
                bottom: item.rcItem.bottom.saturating_sub(8),
            };
            unsafe {
                let _ = FillRect(item.hDC, &accent_rect, accent);
            }
        }
        let mut primary_rect = item.rcItem;
        primary_rect.left = primary_rect.left.saturating_add(14);
        primary_rect.right = primary_rect.right.saturating_sub(10);
        primary_rect.top = primary_rect.top.saturating_add(6);
        primary_rect.bottom = primary_rect.top.saturating_add(22);
        let mut secondary_rect = primary_rect;
        secondary_rect.top = primary_rect.bottom.saturating_add(1);
        secondary_rect.bottom = item.rcItem.bottom.saturating_sub(5);
        unsafe {
            let _ = SetBkMode(item.hDC, TRANSPARENT);
            let _ = SetTextColor(item.hDC, COLORREF(GetSysColor(COLOR_WINDOWTEXT)));
        }
        let heading = load_manager_font(&MANAGER_SECTION_FONT);
        let old_font = if heading.is_invalid() {
            HGDIOBJ::default()
        } else {
            unsafe { SelectObject(item.hDC, HGDIOBJ(heading.0)) }
        };
        unsafe {
            let _ = DrawTextW(
                item.hDC,
                &mut primary,
                &mut primary_rect,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        let body = load_manager_font(&MANAGER_BODY_FONT);
        if !body.is_invalid() {
            let _ = unsafe { SelectObject(item.hDC, HGDIOBJ(body.0)) };
        }
        unsafe {
            let _ = SetTextColor(item.hDC, COLORREF(GetSysColor(COLOR_GRAYTEXT)));
            let _ = DrawTextW(
                item.hDC,
                &mut secondary,
                &mut secondary_rect,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
            let mut focus = item.rcItem;
            focus.left = focus.left.saturating_add(5);
            focus.top = focus.top.saturating_add(3);
            focus.right = focus.right.saturating_sub(3);
            focus.bottom = focus.bottom.saturating_sub(3);
            unsafe {
                let _ = DrawFocusRect(item.hDC, &focus);
            }
        }
        if !old_font.is_invalid() {
            let _ = unsafe { SelectObject(item.hDC, old_font) };
        }
        true
    }

    unsafe fn draw_manager_button(lparam: LPARAM) -> bool {
        if lparam.0 == 0 {
            return false;
        }
        let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
        let Ok(id) = i32::try_from(item.CtlID) else {
            return false;
        };
        let Some(tone) = manager_button_tone(id) else {
            return false;
        };

        let primary = tone == ManagerButtonTone::Primary;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let hot = item.itemState.0 & ODS_HOTLIGHT.0 != 0;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let fill_color = COLORREF(if primary && !disabled {
            unsafe { GetSysColor(COLOR_HIGHLIGHT) }
        } else if disabled {
            unsafe { GetSysColor(COLOR_3DFACE) }
        } else if pressed {
            0x00e8_e8e8
        } else if hot {
            0x00f6_f6f6
        } else {
            unsafe { GetSysColor(COLOR_WINDOW) }
        });
        let border_color = COLORREF(if primary && !disabled {
            unsafe { GetSysColor(COLOR_HIGHLIGHT) }
        } else {
            0x00d1_d1d1
        });
        let text_color = COLORREF(if disabled {
            unsafe { GetSysColor(COLOR_GRAYTEXT) }
        } else if primary {
            unsafe { GetSysColor(COLOR_HIGHLIGHTTEXT) }
        } else {
            unsafe { GetSysColor(COLOR_WINDOWTEXT) }
        });

        let brush = unsafe { CreateSolidBrush(fill_color) };
        let pen = unsafe { CreatePen(PS_SOLID, 1, border_color) };
        let old_brush = unsafe { SelectObject(item.hDC, HGDIOBJ(brush.0)) };
        let old_pen = unsafe { SelectObject(item.hDC, HGDIOBJ(pen.0)) };
        let rect = item.rcItem;
        unsafe {
            let _ = RoundRect(
                item.hDC,
                rect.left,
                rect.top,
                rect.right.saturating_sub(1),
                rect.bottom.saturating_sub(1),
                8,
                8,
            );
        }

        let font = load_manager_font(&MANAGER_BUTTON_FONT);
        let old_font = if font.is_invalid() {
            HGDIOBJ::default()
        } else {
            unsafe { SelectObject(item.hDC, HGDIOBJ(font.0)) }
        };
        let length = unsafe { GetWindowTextLengthW(item.hwndItem) }.max(0) as usize;
        let mut label = vec![0_u16; length.saturating_add(1)];
        let copied = unsafe { GetWindowTextW(item.hwndItem, &mut label) }.max(0) as usize;
        label.truncate(copied);
        let mut text_rect = rect;
        if pressed {
            text_rect.top = text_rect.top.saturating_add(1);
            text_rect.left = text_rect.left.saturating_add(1);
        }
        unsafe {
            let _ = SetBkMode(item.hDC, TRANSPARENT);
            let _ = SetTextColor(item.hDC, text_color);
            let _ = DrawTextW(
                item.hDC,
                &mut label,
                &mut text_rect,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
            let mut focus = rect;
            focus.left = focus.left.saturating_add(3);
            focus.top = focus.top.saturating_add(3);
            focus.right = focus.right.saturating_sub(3);
            focus.bottom = focus.bottom.saturating_sub(3);
            let focus_color = COLORREF(unsafe { GetSysColor(COLOR_HIGHLIGHT) });
            let focus_pen = unsafe { CreatePen(PS_SOLID, 2, focus_color) };
            let null_brush = unsafe { GetStockObject(NULL_BRUSH) };
            let prior_pen = unsafe { SelectObject(item.hDC, HGDIOBJ(focus_pen.0)) };
            let prior_brush = unsafe { SelectObject(item.hDC, null_brush) };
            unsafe {
                let _ = RoundRect(
                    item.hDC,
                    focus.left,
                    focus.top,
                    focus.right,
                    focus.bottom,
                    6,
                    6,
                );
                let _ = SelectObject(item.hDC, prior_brush);
                let _ = SelectObject(item.hDC, prior_pen);
                let _ = DeleteObject(HGDIOBJ(focus_pen.0));
            }
        }

        unsafe {
            if !old_font.is_invalid() {
                let _ = SelectObject(item.hDC, old_font);
            }
            let _ = SelectObject(item.hDC, old_pen);
            let _ = SelectObject(item.hDC, old_brush);
            let _ = DeleteObject(HGDIOBJ(pen.0));
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
        true
    }

    unsafe fn create_manager_controls(window: HWND) -> bool {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return false;
        };
        let instance = Some(HINSTANCE(module.0));
        let heading_font = unsafe { create_manager_font(21, FW_SEMIBOLD.0) };
        let body_font = unsafe { create_manager_font(14, FW_NORMAL.0) };
        let section_font = unsafe { create_manager_font(14, FW_SEMIBOLD.0) };
        let button_font = unsafe { create_manager_font(14, FW_NORMAL.0) };
        if [heading_font, body_font, section_font, button_font]
            .iter()
            .any(|font| font.is_invalid())
        {
            for font in [heading_font, body_font, section_font, button_font] {
                if !font.is_invalid() {
                    let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
                }
            }
            return false;
        }
        store_manager_font(&MANAGER_HEADING_FONT, heading_font);
        store_manager_font(&MANAGER_BODY_FONT, body_font);
        store_manager_font(&MANAGER_SECTION_FONT, section_font);
        store_manager_font(&MANAGER_BUTTON_FONT, button_font);

        let controls = [
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("许愿记录"),
                    control_style(0),
                    24,
                    18,
                    620,
                    30,
                    Some(window),
                    Some(control_menu(MANAGER_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("在输入法里输入 xuy 后按 Tab 留下现场；这里负责查看和整理。"),
                    control_style(0),
                    24,
                    53,
                    636,
                    24,
                    Some(window),
                    Some(control_menu(MANAGER_DESCRIPTION_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("刷新"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    696,
                    21,
                    84,
                    32,
                    Some(window),
                    Some(control_menu(MANAGER_REFRESH_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("所有记录"),
                    control_style(0),
                    40,
                    114,
                    222,
                    24,
                    Some(window),
                    Some(control_menu(MANAGER_LIST_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("LISTBOX"),
                    PCWSTR::null(),
                    control_style(
                        LBS_NOTIFY
                            | LBS_NOINTEGRALHEIGHT
                            | LBS_OWNERDRAWFIXED
                            | LBS_HASSTRINGS
                            | WS_TABSTOP.0 as i32
                            | WS_VSCROLL.0 as i32,
                    ),
                    32,
                    144,
                    238,
                    296,
                    Some(window),
                    Some(control_menu(MANAGER_LIST_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("记录详情"),
                    control_style(0),
                    310,
                    114,
                    454,
                    24,
                    Some(window),
                    Some(control_menu(MANAGER_DETAIL_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    PCWSTR::null(),
                    control_style(
                        ES_LEFT
                            | ES_MULTILINE
                            | ES_AUTOVSCROLL
                            | ES_READONLY
                            | WS_TABSTOP.0 as i32
                            | WS_VSCROLL.0 as i32,
                    ),
                    306,
                    144,
                    462,
                    240,
                    Some(window),
                    Some(control_menu(MANAGER_DETAIL_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("展开前后记录"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    294,
                    417,
                    150,
                    32,
                    Some(window),
                    Some(control_menu(MANAGER_CONTEXT_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("整理这条记录…"),
                    control_style(BS_OWNERDRAW | BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32),
                    630,
                    417,
                    150,
                    32,
                    Some(window),
                    Some(control_menu(MANAGER_EDIT_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("正在读取本机许愿记录…"),
                    control_style(0),
                    24,
                    478,
                    756,
                    24,
                    Some(window),
                    Some(control_menu(MANAGER_STATUS_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("猫猫正在听"),
                    control_style(SS_CENTER.0 as i32),
                    100,
                    232,
                    604,
                    32,
                    Some(window),
                    Some(control_menu(MANAGER_EMPTY_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!(
                        "遇到不舒服的地方时，在输入法里输入 xuy，再按 Tab，\r\n就能把现场留在这里。"
                    ),
                    control_style(SS_CENTER.0 as i32),
                    120,
                    272,
                    564,
                    58,
                    Some(window),
                    Some(control_menu(MANAGER_EMPTY_BODY_ID)),
                    instance,
                    None,
                )
            },
        ];
        if controls.iter().any(Result::is_err) {
            delete_manager_fonts();
            return false;
        }
        let controls = controls.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        for (control, font) in controls.iter().zip([
            heading_font,
            body_font,
            button_font,
            section_font,
            body_font,
            section_font,
            body_font,
            button_font,
            button_font,
            body_font,
            heading_font,
            body_font,
        ]) {
            let _ = unsafe {
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                )
            };
        }
        let _ = unsafe {
            SendMessageW(
                controls[4],
                LB_SETITEMHEIGHT,
                Some(WPARAM(0)),
                Some(LPARAM(58)),
            )
        };
        true
    }

    unsafe fn show_manager(window: HWND) {
        let _ = unsafe { ShowWindow(window, SW_SHOW) };
        let _ = unsafe { SetForegroundWindow(window) };
        if manager_has_records() {
            if let Ok(list) = unsafe { GetDlgItem(Some(window), MANAGER_LIST_ID) } {
                let _ = unsafe { SetFocus(Some(list)) };
            }
        } else {
            let _ = unsafe { SetFocus(Some(window)) };
        }
    }

    fn manager_has_records() -> bool {
        MANAGER_STATE
            .lock()
            .ok()
            .and_then(|state| state.as_ref().map(|state| !state.records.is_empty()))
            .unwrap_or(false)
    }

    fn wish_root_for_executable(executable: &Path) -> Option<PathBuf> {
        let repository = repository_root_for_user_tool_executable(executable, "wishpad")?;
        Some(
            repository
                .join(".local")
                .join("tsf-alpha")
                .join("user-data")
                .join("wishes"),
        )
    }

    fn selected_wish_id() -> Option<String> {
        MANAGER_STATE.lock().ok().and_then(|state| {
            let state = state.as_ref()?;
            state
                .selected
                .and_then(|index| state.records.get(index))
                .map(|record| record.info.id().to_owned())
        })
    }

    fn refresh_records(window: HWND, preserve_id: Option<String>) {
        let Some(root) = std::env::current_exe()
            .ok()
            .and_then(|path| wish_root_for_executable(&path))
        else {
            if let Ok(mut state) = MANAGER_STATE.lock() {
                *state = None;
            }
            set_control_text(window, MANAGER_EMPTY_TITLE_ID, "暂时无法打开许愿记录");
            set_control_text(
                window,
                MANAGER_EMPTY_BODY_ID,
                "无法定位许愿记录的位置。可以稍后再刷新。",
            );
            set_empty_state(window, true);
            set_manager_status(window, "无法定位本项目的许愿目录");
            return;
        };
        let packages = match list_wish_packages(&root) {
            Ok(packages) => packages,
            Err(WishFeedbackError::RootUnavailable) => Vec::new(),
            Err(_) => {
                if let Ok(mut state) = MANAGER_STATE.lock() {
                    *state = None;
                }
                set_control_text(window, MANAGER_EMPTY_TITLE_ID, "暂时无法打开许愿记录");
                set_control_text(
                    window,
                    MANAGER_EMPTY_BODY_ID,
                    "许愿记录目前不可用。原有内容没有被修改，可以稍后再刷新。",
                );
                set_empty_state(window, true);
                set_manager_status(window, "暂时无法读取许愿记录");
                return;
            }
        };
        let records = packages
            .into_iter()
            .map(
                |info| match load_wish_note(&root, info.id(), &WindowsUserDataProtector) {
                    Ok(note) => ManagerRecord {
                        info,
                        note: Some(note),
                        note_unavailable: false,
                    },
                    Err(WishFeedbackError::NoteUnavailable) => ManagerRecord {
                        info,
                        note: None,
                        note_unavailable: false,
                    },
                    Err(_) => ManagerRecord {
                        info,
                        note: None,
                        note_unavailable: true,
                    },
                },
            )
            .collect::<Vec<_>>();
        let selected = preserve_id
            .as_deref()
            .and_then(|id| records.iter().position(|record| record.info.id() == id))
            .or_else(|| (!records.is_empty()).then_some(0));
        let labels = records
            .iter()
            .map(|record| manager_record_label(record, SystemTime::now()))
            .collect::<Vec<_>>();

        if let Ok(mut state) = MANAGER_STATE.lock() {
            *state = Some(ManagerState {
                root,
                records,
                selected,
                show_context: false,
            });
        }
        let empty = labels.is_empty();
        if empty {
            set_control_text(window, MANAGER_EMPTY_TITLE_ID, "猫猫正在听");
            set_control_text(
                window,
                MANAGER_EMPTY_BODY_ID,
                "遇到不舒服的地方时，在输入法里输入 xuy，再按 Tab，\r\n就能把现场留在这里。",
            );
        }
        set_empty_state(window, empty);
        if let Ok(list) = unsafe { GetDlgItem(Some(window), MANAGER_LIST_ID) } {
            let _ = unsafe { SendMessageW(list, LB_RESETCONTENT, None, None) };
            for label in &labels {
                let wide = wide(label);
                let _ = unsafe {
                    SendMessageW(
                        list,
                        LB_ADDSTRING,
                        None,
                        Some(LPARAM(wide.as_ptr() as isize)),
                    )
                };
            }
            if let Some(index) = selected {
                let _ = unsafe { SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None) };
            }
        }
        set_manager_status(
            window,
            if labels.is_empty() {
                "还没有许愿记录"
            } else {
                "选择一条记录查看重点现场"
            },
        );
        render_selected_record(window);
    }

    fn manager_record_label(record: &ManagerRecord, now: SystemTime) -> String {
        let (primary, secondary) = manager_record_lines(record, now);
        format!("{primary}　{secondary}")
    }

    fn manager_record_lines(record: &ManagerRecord, now: SystemTime) -> (String, String) {
        let primary = relative_time(record.info.modified(), now);
        let secondary = if record.note_unavailable {
            "说明暂时不可用".to_owned()
        } else if let Some(note) = &record.note {
            let preview = compact_text(note.text(), 24);
            if preview.is_empty() {
                category_label(note.category()).to_owned()
            } else {
                format!("{}　{preview}", category_label(note.category()))
            }
        } else {
            "待补充说明".to_owned()
        };
        (primary, secondary)
    }

    fn relative_time(then: SystemTime, now: SystemTime) -> String {
        let age = now.duration_since(then).unwrap_or(Duration::ZERO);
        match age.as_secs() {
            0..=59 => "刚刚".to_owned(),
            60..=3_599 => format!("{} 分钟前", age.as_secs() / 60),
            3_600..=86_399 => format!("{} 小时前", age.as_secs() / 3_600),
            seconds => format!("{} 天前", seconds / 86_400),
        }
    }

    fn select_from_list(window: HWND) {
        let Ok(list) = (unsafe { GetDlgItem(Some(window), MANAGER_LIST_ID) }) else {
            return;
        };
        let value = unsafe { SendMessageW(list, LB_GETCURSEL, None, None) }.0;
        let selected = usize::try_from(value).ok();
        if let Ok(mut state) = MANAGER_STATE.lock()
            && let Some(state) = state.as_mut()
        {
            state.selected = selected.filter(|index| *index < state.records.len());
            state.show_context = false;
        }
        render_selected_record(window);
    }

    fn toggle_context(window: HWND) {
        if let Ok(mut state) = MANAGER_STATE.lock()
            && let Some(state) = state.as_mut()
            && state.selected.is_some()
        {
            state.show_context = !state.show_context;
        }
        render_selected_record(window);
    }

    fn render_selected_record(window: HWND) {
        let selected = MANAGER_STATE.lock().ok().and_then(|state| {
            let state = state.as_ref()?;
            let index = state.selected?;
            let record = state.records.get(index)?;
            Some((
                state.root.clone(),
                record.info.id().to_owned(),
                record.info.modified(),
                record.note.clone(),
                record.note_unavailable,
                state.show_context,
            ))
        });
        let Some((root, wish_id, modified, note, note_unavailable, show_context)) = selected else {
            show_empty_details(
                window,
                "还没有许愿记录。\r\n\r\n遇到不舒服时，在输入法里输入 xuy，再按 Tab。",
            );
            set_manager_actions(window, false, false, false);
            return;
        };
        let snapshot = match load_wish_snapshot(&root, &wish_id, &WindowsUserDataProtector) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                show_empty_details(window, "这条记录暂时无法读取；原文件没有被修改。");
                set_manager_actions(window, true, false, false);
                return;
            }
        };
        let category = note
            .as_ref()
            .map(WishNote::category)
            .unwrap_or_else(|| snapshot.category());
        let mut output = String::new();
        output.push_str(category_label(category));
        output.push_str("　·　");
        output.push_str(&relative_time(modified, SystemTime::now()));
        output.push_str("\r\n");
        output.push_str(capture_scope_label(snapshot.capture_scope()));
        output.push_str(&format!(
            "　·　{} 条现场记录\r\n\r\n",
            snapshot.events().len()
        ));
        output.push_str("宝宝想说\r\n");
        if note_unavailable {
            output.push_str("这条说明暂时无法读取。\r\n");
        } else if let Some(note) = &note {
            output.push_str(note.text());
            output.push_str("\r\n");
        } else {
            output.push_str("还没有补充说明。\r\n");
        }
        output.push_str("\r\n重点现场\r\n");
        let focus = snapshot.focus_event_range();
        let indices = if show_context {
            0..snapshot.events().len()
        } else {
            focus.clone()
        };
        if indices.is_empty() {
            output.push_str("没有可显示的重点片段。\r\n");
        } else {
            for index in indices {
                let Some(event) = snapshot.events().get(index) else {
                    continue;
                };
                let role = snapshot.event_role(index).unwrap_or(WishEventRole::Focus);
                output.push_str(&format!(
                    "{}　{}　{}\r\n",
                    event_role_label(role),
                    event_age_label(event.milliseconds_before_marker()),
                    event_summary(event.event()),
                ));
            }
        }
        let has_context = focus.start > 0 || focus.end < snapshot.events().len();
        set_control_text(window, MANAGER_DETAIL_ID, &output);
        set_manager_actions(window, true, has_context, show_context);
    }

    fn show_empty_details(window: HWND, message: &str) {
        set_control_text(window, MANAGER_DETAIL_ID, message);
    }

    fn set_empty_state(window: HWND, empty: bool) {
        for id in [
            MANAGER_LIST_TITLE_ID,
            MANAGER_LIST_ID,
            MANAGER_DETAIL_TITLE_ID,
            MANAGER_DETAIL_ID,
            MANAGER_CONTEXT_ID,
            MANAGER_EDIT_ID,
            MANAGER_STATUS_ID,
        ] {
            if let Ok(control) = unsafe { GetDlgItem(Some(window), id) } {
                let _ = unsafe { ShowWindow(control, if empty { SW_HIDE } else { SW_SHOW }) };
            }
        }
        for id in [MANAGER_EMPTY_TITLE_ID, MANAGER_EMPTY_BODY_ID] {
            if let Ok(control) = unsafe { GetDlgItem(Some(window), id) } {
                let _ = unsafe { ShowWindow(control, if empty { SW_SHOW } else { SW_HIDE }) };
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(window), None, true);
        }
    }

    fn set_manager_actions(window: HWND, can_edit: bool, has_context: bool, showing: bool) {
        if let Ok(edit) = unsafe { GetDlgItem(Some(window), MANAGER_EDIT_ID) } {
            let _ = unsafe { EnableWindow(edit, can_edit) };
        }
        if let Ok(context) = unsafe { GetDlgItem(Some(window), MANAGER_CONTEXT_ID) } {
            let _ = unsafe { EnableWindow(context, has_context) };
            let label = if showing {
                "只看重点"
            } else {
                "展开前后记录"
            };
            let text = wide(label);
            let _ = unsafe { SetWindowTextW(context, PCWSTR(text.as_ptr())) };
        }
    }

    fn category_label(category: WishCategory) -> &'static str {
        match category {
            WishCategory::Candidates => "候选",
            WishCategory::Ranking => "排序",
            WishCategory::Display => "显示",
            WishCategory::Latency => "延迟",
            WishCategory::InputMode => "输入模式",
            WishCategory::Compatibility => "兼容性",
            WishCategory::Other => "其他",
        }
    }

    fn capture_scope_label(scope: WishCaptureScope) -> &'static str {
        match scope {
            WishCaptureScope::LegacyWindow => "旧版现场",
            WishCaptureScope::RecentEpisodes => "刚才的输入片段",
            WishCaptureScope::RecentWindow => "较长现场",
            WishCaptureScope::ContinuousJournal => "持续研究批次",
        }
    }

    fn event_role_label(role: WishEventRole) -> &'static str {
        match role {
            WishEventRole::Context => "前文",
            WishEventRole::Focus => "重点",
            WishEventRole::Trigger => "触发",
        }
    }

    fn event_age_label(milliseconds: u32) -> String {
        if milliseconds < 1_000 {
            format!("{milliseconds} 毫秒前")
        } else {
            format!("{:.1} 秒前", f64::from(milliseconds) / 1_000.0)
        }
    }

    fn event_summary(event: &NativeFeedbackEvent) -> String {
        match event {
            NativeFeedbackEvent::CandidatesPresented {
                code, candidates, ..
            } => format!(
                "候选　{} → {}",
                compact_text(code, 28),
                candidates
                    .iter()
                    .map(|candidate| compact_text(candidate, 16))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                candidates,
                automatic_transposition,
                ..
            } => {
                let mut summary = format!(
                    "候选　{} → {}",
                    compact_text(code, 28),
                    candidates
                        .iter()
                        .map(|candidate| compact_text(candidate, 16))
                        .collect::<Vec<_>>()
                        .join("、")
                );
                if let Some(decision) = automatic_transposition {
                    summary.push_str("　·　");
                    summary.push_str(&automatic_transposition_summary(decision));
                }
                summary
            }
            NativeFeedbackEvent::CandidateCommitted {
                code,
                text,
                visible_rank,
                ..
            } => format!(
                "上屏　{} → {}（第 {} 项）",
                compact_text(code, 28),
                compact_text(text, 40),
                visible_rank,
            ),
            NativeFeedbackEvent::RawCodeCommitted { code } => {
                format!("原码　{}", compact_text(code, 60))
            }
            NativeFeedbackEvent::CompositionCancelled { code, .. } => {
                format!("取消　{}", compact_text(code, 60))
            }
            NativeFeedbackEvent::CandidatePopupTiming {
                first_frame_ms,
                fully_visible_ms,
                ..
            } => format!("显示　首帧 {first_frame_ms} ms，完成 {fully_visible_ms} ms"),
            NativeFeedbackEvent::SlowKeyPathTiming {
                refresh_ms,
                planning_ms,
                edit_session_ms,
                total_ms,
            } => format!(
                "慢按键　总计 {total_ms} ms；刷新 {refresh_ms} ms，候选 {planning_ms} ms，编辑 {edit_session_ms} ms"
            ),
        }
    }

    fn automatic_transposition_summary(decision: &NativeAutomaticTranspositionDecision) -> String {
        let tier_name = |tier| match tier {
            NativeAutomaticTranspositionTier::Primary => "高置信",
            NativeAutomaticTranspositionTier::Secondary => "中置信",
            NativeAutomaticTranspositionTier::Shadow => "影子",
        };
        let tier = if decision.cold_tier() == decision.tier() {
            tier_name(decision.tier()).to_owned()
        } else {
            format!(
                "{}→{}",
                tier_name(decision.cold_tier()),
                tier_name(decision.tier())
            )
        };
        let action = if decision.syllable_count() == 1 {
            "换序"
        } else {
            "双音节换序"
        };
        match decision.outcome() {
            NativeAutomaticTranspositionOutcome::Suppressed => {
                format!(
                    "{action} {tier} · 原码证据优先 · {} ms",
                    decision.pair_gap_ms()
                )
            }
            NativeAutomaticTranspositionOutcome::NoRecovery => {
                format!(
                    "{action} {tier} · 没有唯一结果 · {} ms",
                    decision.pair_gap_ms()
                )
            }
            NativeAutomaticTranspositionOutcome::RecoveryAvailable => {
                let text = decision
                    .recovered_text()
                    .map(|text| compact_text(text, 16))
                    .unwrap_or_else(|| "候选".to_owned());
                match decision.visible_rank() {
                    Some(rank) => format!(
                        "{action} {tier} · {text} 第 {rank} 项 · {} ms",
                        decision.pair_gap_ms()
                    ),
                    None => format!(
                        "{action} {tier} · 后台命中 {text} · {} ms",
                        decision.pair_gap_ms()
                    ),
                }
            }
        }
    }

    fn compact_text(text: &str, maximum_chars: usize) -> String {
        let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = flattened.chars();
        let prefix = chars.by_ref().take(maximum_chars).collect::<String>();
        if chars.next().is_some() {
            format!("{prefix}…")
        } else {
            prefix
        }
    }

    fn set_manager_status(window: HWND, message: &str) {
        set_control_text(window, MANAGER_STATUS_ID, message);
    }

    fn set_control_text(window: HWND, id: i32, value: &str) {
        let Ok(control) = (unsafe { GetDlgItem(Some(window), id) }) else {
            return;
        };
        let text = wide(value);
        let _ = unsafe { SetWindowTextW(control, PCWSTR(text.as_ptr())) };
    }

    fn open_note_window(owner: HWND) {
        if let Ok(existing) = unsafe { FindWindowW(w!("ZiranmaWishpadNoteWindow"), PCWSTR::null()) }
        {
            let _ = unsafe { SetForegroundWindow(existing) };
            return;
        }
        let selected = MANAGER_STATE.lock().ok().and_then(|state| {
            let state = state.as_ref()?;
            let record = state.records.get(state.selected?)?;
            Some((
                state.root.clone(),
                record.info.id().to_owned(),
                record.note.clone(),
            ))
        });
        let Some((root, wish_id, existing)) = selected else {
            show_note_message(owner, "请先选择一条许愿记录。", false);
            return;
        };
        let Ok(mut target) = NOTE_TARGET.lock() else {
            show_note_message(owner, "暂时无法打开整理窗口。", true);
            return;
        };
        *target = Some(NoteTarget {
            owner: owner.0 as isize,
            root,
            wish_id,
            existing,
        });
        drop(target);

        let created = unsafe {
            GetModuleHandleW(None).and_then(|module| {
                CreateWindowExW(
                    WS_EX_CONTROLPARENT,
                    w!("ZiranmaWishpadNoteWindow"),
                    w!("整理许愿"),
                    WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    520,
                    390,
                    Some(owner),
                    None,
                    Some(HINSTANCE(module.0)),
                    None,
                )
            })
        };
        match created {
            Ok(window) => {
                let _ = unsafe { SetForegroundWindow(window) };
                if let Ok(edit) = unsafe { GetDlgItem(Some(window), NOTE_EDIT_ID) } {
                    let _ = unsafe { SetFocus(Some(edit)) };
                }
            }
            Err(_) => {
                if let Ok(mut target) = NOTE_TARGET.lock() {
                    *target = None;
                }
                show_note_message(owner, "暂时无法建立整理窗口。", true);
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
                configure_manager_window(window);
                if unsafe { create_note_controls(window) } {
                    LRESULT(0)
                } else {
                    LRESULT(-1)
                }
            }
            WM_PAINT => {
                unsafe { paint_note_background(window) };
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC => unsafe { paint_note_static(wparam, lparam) },
            WM_DRAWITEM => {
                if unsafe { draw_manager_button(lparam) } {
                    LRESULT(1)
                } else {
                    unsafe { DefWindowProcW(window, message, wparam, lparam) }
                }
            }
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    NOTE_SAVE_ID => unsafe { save_note(window) },
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
            WM_NCDESTROY => {
                if let Ok(mut target) = NOTE_TARGET.lock() {
                    *target = None;
                }
                unsafe { DefWindowProcW(window, message, wparam, lparam) }
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    unsafe fn paint_note_background(window: HWND) {
        let mut paint = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(window, &mut paint) };
        let background = unsafe { GetSysColorBrush(COLOR_3DFACE) };
        let mut client = RECT::default();
        let _ =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut client) };
        let _ = unsafe { FillRect(hdc, &client, background) };
        if let Ok(module) = unsafe { GetModuleHandleW(None) }
            && let Ok(icon) = unsafe {
                load_wishpad_illustration(
                    HINSTANCE(module.0),
                    WISHPAD_ORGANIZING_ICON_RESOURCE_ID,
                    52,
                )
            }
        {
            let _ = unsafe { DrawIconEx(hdc, 24, 18, icon, 52, 52, 0, None, DI_NORMAL) };
            let _ = unsafe { DestroyIcon(icon) };
        }
        let _ = unsafe { EndPaint(window, &paint) };
    }

    unsafe fn paint_note_static(wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        let id = unsafe { GetDlgCtrlID(control) };
        let text_color = if matches!(id, NOTE_SUBTITLE_ID | NOTE_FOOTNOTE_ID) {
            COLORREF(unsafe { GetSysColor(COLOR_GRAYTEXT) })
        } else {
            COLORREF(unsafe { GetSysColor(COLOR_WINDOWTEXT) })
        };
        unsafe {
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetBkColor(hdc, COLORREF(GetSysColor(COLOR_3DFACE)));
            let _ = SetTextColor(hdc, text_color);
        }
        LRESULT(unsafe { GetSysColorBrush(COLOR_3DFACE) }.0 as isize)
    }

    fn control_menu(id: i32) -> HMENU {
        HMENU(id as usize as *mut c_void)
    }

    fn control_style(extra: i32) -> WINDOW_STYLE {
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | u32::try_from(extra).unwrap_or_default())
    }

    unsafe fn create_note_controls(window: HWND) -> bool {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return false;
        };
        let instance = Some(HINSTANCE(module.0));
        let controls = [
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("整理这条记录"),
                    control_style(0),
                    88,
                    18,
                    390,
                    28,
                    Some(window),
                    Some(control_menu(NOTE_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("补充类别和说明，之后仍然可以修改。"),
                    control_style(0),
                    88,
                    49,
                    390,
                    24,
                    Some(window),
                    Some(control_menu(NOTE_SUBTITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("类别"),
                    control_style(0),
                    24,
                    92,
                    68,
                    24,
                    Some(window),
                    Some(control_menu(NOTE_CATEGORY_LABEL_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("COMBOBOX"),
                    PCWSTR::null(),
                    control_style(
                        windows::Win32::UI::WindowsAndMessaging::CBS_DROPDOWNLIST
                            | windows::Win32::UI::WindowsAndMessaging::CBS_HASSTRINGS
                            | WS_TABSTOP.0 as i32,
                    ),
                    104,
                    86,
                    374,
                    220,
                    Some(window),
                    Some(control_menu(NOTE_CATEGORY_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("宝宝想补充"),
                    control_style(0),
                    24,
                    130,
                    454,
                    24,
                    Some(window),
                    Some(control_menu(NOTE_BODY_LABEL_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    w!("EDIT"),
                    PCWSTR::null(),
                    control_style(
                        ES_LEFT
                            | ES_MULTILINE
                            | ES_AUTOVSCROLL
                            | ES_WANTRETURN
                            | WS_TABSTOP.0 as i32
                            | WS_VSCROLL.0 as i32,
                    ),
                    24,
                    156,
                    454,
                    116,
                    Some(window),
                    Some(control_menu(NOTE_EDIT_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("类别和说明以后仍可继续修改。"),
                    control_style(0),
                    24,
                    284,
                    250,
                    24,
                    Some(window),
                    Some(control_menu(NOTE_FOOTNOTE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("取消"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    292,
                    310,
                    88,
                    32,
                    Some(window),
                    Some(control_menu(NOTE_CANCEL_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("保存整理"),
                    control_style(BS_OWNERDRAW | BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32),
                    390,
                    310,
                    88,
                    32,
                    Some(window),
                    Some(control_menu(NOTE_SAVE_ID)),
                    instance,
                    None,
                )
            },
        ];
        if controls.iter().any(Result::is_err) {
            return false;
        }
        let controls = controls.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        let heading_font = load_manager_font(&MANAGER_HEADING_FONT);
        let body_font = load_manager_font(&MANAGER_BODY_FONT);
        let section_font = load_manager_font(&MANAGER_SECTION_FONT);
        let button_font = load_manager_font(&MANAGER_BUTTON_FONT);
        for (control, font) in controls.iter().zip([
            heading_font,
            body_font,
            section_font,
            body_font,
            section_font,
            body_font,
            body_font,
            button_font,
            button_font,
        ]) {
            let _ = unsafe {
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                )
            };
        }
        let category = controls[3];
        for label in ["候选", "排序", "显示", "延迟", "输入模式", "兼容性", "其他"]
        {
            let text = wide(label);
            let _ = unsafe {
                SendMessageW(
                    category,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(text.as_ptr() as isize)),
                )
            };
        }
        let existing = NOTE_TARGET
            .lock()
            .ok()
            .and_then(|target| target.as_ref().and_then(|target| target.existing.clone()));
        let selected_category = existing
            .as_ref()
            .map(|note| wish_category_index(note.category()))
            .unwrap_or(0);
        let _ = unsafe {
            SendMessageW(
                category,
                CB_SETCURSEL,
                Some(WPARAM(selected_category)),
                None,
            )
        };
        let _ = unsafe {
            SendMessageW(
                controls[5],
                EDIT_SET_LIMIT_TEXT,
                Some(WPARAM(NOTE_TEXT_CHARACTER_LIMIT)),
                None,
            )
        };
        if let Some(note) = existing {
            let text = wide(note.text());
            let _ = unsafe { SetWindowTextW(controls[5], PCWSTR(text.as_ptr())) };
        }
        true
    }

    fn wish_categories() -> [WishCategory; 7] {
        [
            WishCategory::Candidates,
            WishCategory::Ranking,
            WishCategory::Display,
            WishCategory::Latency,
            WishCategory::InputMode,
            WishCategory::Compatibility,
            WishCategory::Other,
        ]
    }

    fn wish_category_at(index: usize) -> Option<WishCategory> {
        wish_categories().get(index).copied()
    }

    fn wish_category_index(category: WishCategory) -> usize {
        wish_categories()
            .iter()
            .position(|candidate| *candidate == category)
            .unwrap_or(6)
    }

    unsafe fn save_note(window: HWND) {
        let Some(target) = NOTE_TARGET.lock().ok().and_then(|target| target.clone()) else {
            show_note_message(window, "这条许愿已经不可用，请关闭窗口后重试。", true);
            return;
        };
        let Ok(category_control) = (unsafe { GetDlgItem(Some(window), NOTE_CATEGORY_ID) }) else {
            show_note_message(window, "无法读取类别。", true);
            return;
        };
        let category_index = unsafe { SendMessageW(category_control, CB_GETCURSEL, None, None) }.0;
        let Some(category) = usize::try_from(category_index)
            .ok()
            .and_then(wish_category_at)
        else {
            show_note_message(window, "请选择一个类别。", false);
            return;
        };
        let Ok(edit) = (unsafe { GetDlgItem(Some(window), NOTE_EDIT_ID) }) else {
            show_note_message(window, "无法读取说明内容。", true);
            return;
        };
        let length = unsafe { GetWindowTextLengthW(edit) };
        let Ok(length) = usize::try_from(length) else {
            show_note_message(window, "说明内容过长。", false);
            return;
        };
        let mut buffer = vec![0_u16; length.saturating_add(1)];
        let copied = unsafe { GetWindowTextW(edit, &mut buffer) };
        let Ok(copied) = usize::try_from(copied) else {
            show_note_message(window, "无法读取说明内容。", true);
            return;
        };
        let Ok(text) = String::from_utf16(&buffer[..copied]) else {
            show_note_message(window, "说明内容无法识别。", false);
            return;
        };
        let note = match WishNote::new(&target.wish_id, category, text.trim()) {
            Ok(note) => note,
            Err(_) => {
                show_note_message(window, "请写下一句不为空、长度适中的说明。", false);
                return;
            }
        };
        match save_or_replace_wish_note(&target.root, &note, &WindowsUserDataProtector) {
            Ok(()) => {
                let owner = HWND(target.owner as *mut c_void);
                let _ = unsafe {
                    PostMessageW(Some(owner), RECORDS_CHANGED_MESSAGE, WPARAM(0), LPARAM(0))
                };
                let _ = unsafe { DestroyWindow(window) };
            }
            Err(_) => show_note_message(window, "整理内容保存失败；原始许愿现场没有被修改。", true),
        }
    }

    fn show_note_message(window: HWND, message: &str, failed: bool) {
        let text = wide(message);
        let style = MB_OK | if failed { MB_ICONERROR } else { MB_ICONWARNING };
        unsafe {
            let _ = MessageBoxW(Some(window), PCWSTR(text.as_ptr()), w!("猫猫应愿"), style);
        }
    }

    unsafe fn load_wishpad_icon(
        instance: HINSTANCE,
    ) -> windows::core::Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
        let resource = PCWSTR(WISHPAD_ICON_RESOURCE_ID as *const u16);
        unsafe { LoadIconW(Some(instance), resource) }
    }

    unsafe fn load_wishpad_illustration(
        instance: HINSTANCE,
        resource_id: usize,
        size: i32,
    ) -> windows::core::Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
        let resource = PCWSTR(resource_id as *const u16);
        let handle = unsafe {
            LoadImageW(
                Some(instance),
                resource,
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        }?;
        Ok(windows::Win32::UI::WindowsAndMessaging::HICON(handle.0))
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn show_fatal_error(message: &str) {
        let text = wide(message);
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                w!("猫猫应愿无法启动"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn note_categories_have_one_stable_ui_order() {
            assert_eq!(wish_category_at(0), Some(WishCategory::Candidates));
            assert_eq!(wish_category_at(3), Some(WishCategory::Latency));
            assert_eq!(wish_category_at(6), Some(WishCategory::Other));
            assert_eq!(wish_category_at(7), None);
            assert_eq!(wish_category_index(WishCategory::Display), 2);
        }

        #[test]
        fn manager_and_note_dialog_keep_primary_and_secondary_actions_distinct() {
            assert_eq!(
                manager_button_tone(MANAGER_EDIT_ID),
                Some(ManagerButtonTone::Primary)
            );
            assert_eq!(
                manager_button_tone(MANAGER_CONTEXT_ID),
                Some(ManagerButtonTone::Secondary)
            );
            assert_eq!(
                manager_button_tone(NOTE_SAVE_ID),
                Some(ManagerButtonTone::Primary)
            );
            assert_eq!(
                manager_button_tone(NOTE_CANCEL_ID),
                Some(ManagerButtonTone::Secondary)
            );
            assert_eq!(manager_button_tone(MANAGER_STATUS_ID), None);
        }

        #[test]
        fn changing_detail_text_repaints_on_an_opaque_background() {
            assert!(manager_static_needs_opaque_background(MANAGER_DETAIL_ID));
            assert!(!manager_static_needs_opaque_background(
                MANAGER_DETAIL_TITLE_ID
            ));
            assert!(!manager_static_needs_opaque_background(
                MANAGER_DESCRIPTION_ID
            ));
        }

        #[test]
        fn note_storage_root_is_derived_only_from_managed_binary_layouts() {
            assert_eq!(
                wish_root_for_executable(Path::new(r"D:\repo\target\release\wishpad.exe")),
                Some(PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\wishes"))
            );
            assert_eq!(
                wish_root_for_executable(Path::new(
                    r"D:\repo\.local\tsf-alpha\user-tools\builds\aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\wishpad.exe"
                )),
                Some(PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\wishes"))
            );
            assert!(wish_root_for_executable(Path::new(r"D:\tools\wishpad.exe")).is_none());
        }

        #[test]
        fn relative_time_and_private_text_preview_are_bounded() {
            let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
            assert_eq!(relative_time(now - Duration::from_secs(45), now), "刚刚");
            assert_eq!(
                relative_time(now - Duration::from_secs(120), now),
                "2 分钟前"
            );
            assert_eq!(compact_text("一行\r\n二行", 20), "一行 二行");
            assert_eq!(compact_text("一二三四五", 3), "一二三…");
        }

        #[test]
        fn event_preview_uses_plain_user_facing_language() {
            assert_eq!(
                event_summary(&NativeFeedbackEvent::RawCodeCommitted {
                    code: "abc".to_owned(),
                }),
                "原码　abc"
            );
            assert_eq!(
                capture_scope_label(WishCaptureScope::RecentEpisodes),
                "刚才的输入片段"
            );
            assert_eq!(event_role_label(WishEventRole::Focus), "重点");
        }
    }
}
