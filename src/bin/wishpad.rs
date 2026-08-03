#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("向猫猫许愿目前只支持 Windows");
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
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_3DFACE, COLOR_3DSHADOW, COLOR_GRAYTEXT,
        COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontW,
        CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DT_CENTER,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawFocusRect, DrawTextW,
        FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GetStockObject, GetSysColor, GetSysColorBrush,
        HGDIOBJ, OUT_DEFAULT_PRECIS, PS_SOLID, RoundRect, SelectObject, SetBkMode, SetTextColor,
        TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{
        DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, BS_DEFPUSHBUTTON, BS_OWNERDRAW, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL,
        CBS_DROPDOWNLIST, CBS_HASSTRINGS, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW,
        DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL, ES_LEFT,
        ES_MULTILINE, ES_WANTRETURN, FindWindowW, GetCursorPos, GetDlgCtrlID, GetDlgItem,
        GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IDI_APPLICATION,
        IsDialogMessageW, LoadCursorW, LoadIconW, MB_ICONERROR, MB_ICONWARNING, MB_OK,
        MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SW_HIDE, SW_SHOW, SendMessageW, SetForegroundWindow,
        SetWindowTextW, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
        TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
        WM_CONTEXTMENU, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM, WM_LBUTTONDBLCLK,
        WM_LBUTTONUP, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WM_SETFONT, WNDCLASSW, WS_BORDER,
        WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT, WS_EX_TOOLWINDOW, WS_SYSMENU,
        WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };
    use windows::core::{PCWSTR, Result as WindowsResult, w};
    use ziranma_core::{
        WindowsUserDataProtector, WishCategory, WishCommand, WishCommandAckStatus,
        WishCommandDispatchReceipt, WishFeedbackError, WishNote, dispatch_wish_command,
        list_wish_packages, save_wish_note,
    };

    const TRAY_ICON_ID: u32 = 1;
    const WISHPAD_ICON_RESOURCE_ID: usize = 101;
    const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 37;
    const OPEN_PANEL_MESSAGE: u32 = WM_APP + 38;
    const MENU_OPEN: u32 = 1;
    const MENU_CLEAR: u32 = 2;
    const MENU_EXIT: u32 = 100;
    const PANEL_STATUS_ID: i32 = 101;
    const PANEL_START_ID: i32 = 102;
    const PANEL_MARK_ID: i32 = 103;
    const PANEL_NOTE_ID: i32 = 104;
    const PANEL_STOP_ID: i32 = 105;
    const PANEL_TITLE_ID: i32 = 106;
    const PANEL_DESCRIPTION_ID: i32 = 107;
    const NOTE_CATEGORY_ID: i32 = 201;
    const NOTE_EDIT_ID: i32 = 202;
    const NOTE_SAVE_ID: i32 = 203;
    const NOTE_CANCEL_ID: i32 = 204;
    const EDIT_SET_LIMIT_TEXT: u32 = 0x00c5;
    const NOTE_TEXT_CHARACTER_LIMIT: usize = 2_048;
    static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
    static PANEL_HEADING_FONT: AtomicIsize = AtomicIsize::new(0);
    static PANEL_BODY_FONT: AtomicIsize = AtomicIsize::new(0);
    static PANEL_BUTTON_FONT: AtomicIsize = AtomicIsize::new(0);
    static NOTE_TARGET: Mutex<Option<NoteTarget>> = Mutex::new(None);

    #[derive(Clone)]
    struct NoteTarget {
        owner: isize,
        root: PathBuf,
        wish_id: String,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum PanelButtonTone {
        Primary,
        Secondary,
    }

    fn panel_button_tone(id: i32) -> Option<PanelButtonTone> {
        match id {
            PANEL_MARK_ID => Some(PanelButtonTone::Primary),
            PANEL_START_ID | PANEL_NOTE_ID | PANEL_STOP_ID => Some(PanelButtonTone::Secondary),
            _ => None,
        }
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        // SAFETY: this process owns the registered class and hidden window for
        // the duration of the message loop below.
        unsafe {
            let module = GetModuleHandleW(None)?;
            let instance = HINSTANCE(module.0);
            let icon = load_wishpad_icon(instance).or_else(|_| LoadIconW(None, IDI_APPLICATION))?;
            let class_name = w!("ZiranmaWishpadWindow");
            if let Ok(existing) = FindWindowW(class_name, PCWSTR::null()) {
                let _ = SendMessageW(existing, OPEN_PANEL_MESSAGE, None, None);
                let _ = SetForegroundWindow(existing);
                return Ok(());
            }
            TASKBAR_CREATED_MESSAGE.store(
                RegisterWindowMessageW(w!("TaskbarCreated")),
                Ordering::Release,
            );
            let class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: class_name,
                lpfnWndProc: Some(window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: icon,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err("无法注册许愿面板窗口".into());
            }
            let note_class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: w!("ZiranmaWishpadNoteWindow"),
                lpfnWndProc: Some(note_window_proc),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: icon,
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            if RegisterClassW(&note_class) == 0 {
                return Err("无法注册许愿说明窗口".into());
            }
            let window = CreateWindowExW(
                WS_EX_CONTROLPARENT | WS_EX_TOOLWINDOW,
                class_name,
                w!("向猫猫许愿"),
                WS_CAPTION | WS_SYSMENU,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                468,
                286,
                None,
                None,
                Some(instance),
                None,
            )?;
            add_tray_icon(window)?;
            show_panel(window);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    delete_tray_icon(window);
                    return Err("许愿面板消息循环失败".into());
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
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            delete_tray_icon(window);
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == TASKBAR_CREATED_MESSAGE.load(Ordering::Acquire) && message != 0 {
            // Explorer recreated its notification area; restore this process's
            // icon without changing any feedback state.
            let _ = unsafe { add_tray_icon(window) };
            return LRESULT(0);
        }
        match message {
            TRAY_CALLBACK_MESSAGE => {
                match lparam.0 as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => unsafe { show_panel(window) },
                    WM_RBUTTONUP | WM_CONTEXTMENU => unsafe { show_menu(window) },
                    _ => {}
                }
                LRESULT(0)
            }
            OPEN_PANEL_MESSAGE => {
                // SAFETY: the message targets our live single-instance panel.
                unsafe { show_panel(window) };
                LRESULT(0)
            }
            WM_CREATE => {
                configure_panel_window(window);
                if unsafe { create_panel_controls(window) } {
                    LRESULT(0)
                } else {
                    LRESULT(-1)
                }
            }
            WM_CTLCOLORSTATIC => unsafe { paint_panel_static(wparam, lparam) },
            WM_DRAWITEM => {
                if unsafe { draw_panel_button(lparam) } {
                    LRESULT(1)
                } else {
                    unsafe { DefWindowProcW(window, message, wparam, lparam) }
                }
            }
            WM_COMMAND => {
                match (wparam.0 & 0xffff) as i32 {
                    PANEL_START_ID => run_panel_command(window, WishCommand::Start),
                    PANEL_MARK_ID => run_panel_command(window, WishCommand::SaveRecent),
                    PANEL_NOTE_ID => open_note_window(window),
                    PANEL_STOP_ID => run_panel_command(window, WishCommand::Stop),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // Keep the warmed single instance available for the shortcut
                // and tray icon; exiting remains an explicit tray action.
                let _ = unsafe { ShowWindow(window, SW_HIDE) };
                LRESULT(0)
            }
            WM_DESTROY => {
                delete_panel_fonts();
                // SAFETY: terminates only this process's message loop.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => {
                // SAFETY: unhandled messages retain standard window behavior.
                unsafe { DefWindowProcW(window, message, wparam, lparam) }
            }
        }
    }

    fn configure_panel_window(window: HWND) {
        let corner = DWMWCP_ROUND;
        let border: u32 = unsafe { GetSysColor(COLOR_3DFACE) };
        // SAFETY: both attributes are fixed-size values valid for the life of
        // these synchronous calls. Unsupported older systems simply ignore
        // the best-effort visual refinement.
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

    unsafe fn create_panel_font(height: i32, weight: u32) -> windows::Win32::Graphics::Gdi::HFONT {
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

    fn store_panel_font(slot: &AtomicIsize, font: windows::Win32::Graphics::Gdi::HFONT) {
        slot.store(font.0 as isize, Ordering::Release);
    }

    fn load_panel_font(slot: &AtomicIsize) -> windows::Win32::Graphics::Gdi::HFONT {
        windows::Win32::Graphics::Gdi::HFONT(slot.load(Ordering::Acquire) as *mut c_void)
    }

    fn delete_panel_fonts() {
        for slot in [&PANEL_HEADING_FONT, &PANEL_BODY_FONT, &PANEL_BUTTON_FONT] {
            let handle = slot.swap(0, Ordering::AcqRel);
            if handle != 0 {
                // SAFETY: each font was created once for this window and is
                // released only while the process tears that window down.
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(handle as *mut c_void));
                }
            }
        }
    }

    unsafe fn paint_panel_static(wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        let id = unsafe { GetDlgCtrlID(control) };
        let color = COLORREF(match id {
            PANEL_STATUS_ID => unsafe { GetSysColor(COLOR_HIGHLIGHT) },
            PANEL_DESCRIPTION_ID => unsafe { GetSysColor(COLOR_GRAYTEXT) },
            _ => unsafe { GetSysColor(COLOR_WINDOWTEXT) },
        });
        unsafe {
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(hdc, color);
        }
        LRESULT(unsafe { GetSysColorBrush(COLOR_WINDOW) }.0 as isize)
    }

    unsafe fn draw_panel_button(lparam: LPARAM) -> bool {
        if lparam.0 == 0 {
            return false;
        }
        let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
        let Ok(id) = i32::try_from(item.CtlID) else {
            return false;
        };
        let Some(tone) = panel_button_tone(id) else {
            return false;
        };

        let primary = tone == PanelButtonTone::Primary;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let hot = item.itemState.0 & ODS_HOTLIGHT.0 != 0;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let fill_color = COLORREF(if primary {
            unsafe { GetSysColor(COLOR_HIGHLIGHT) }
        } else if pressed || hot {
            unsafe { GetSysColor(COLOR_3DFACE) }
        } else {
            unsafe { GetSysColor(COLOR_WINDOW) }
        });
        let border_color = COLORREF(if primary {
            unsafe { GetSysColor(COLOR_HIGHLIGHT) }
        } else {
            unsafe { GetSysColor(COLOR_3DSHADOW) }
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

        let font = load_panel_font(&PANEL_BUTTON_FONT);
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
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let mut focus = rect;
            focus.left = focus.left.saturating_add(4);
            focus.top = focus.top.saturating_add(4);
            focus.right = focus.right.saturating_sub(4);
            focus.bottom = focus.bottom.saturating_sub(4);
            unsafe {
                let _ = DrawFocusRect(item.hDC, &focus);
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

    unsafe fn create_panel_controls(window: HWND) -> bool {
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return false;
        };
        let instance = Some(HINSTANCE(module.0));
        let heading_font = unsafe { create_panel_font(20, FW_SEMIBOLD.0) };
        let body_font = unsafe { create_panel_font(14, FW_NORMAL.0) };
        let button_font = unsafe { create_panel_font(14, FW_SEMIBOLD.0) };
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
        store_panel_font(&PANEL_HEADING_FONT, heading_font);
        store_panel_font(&PANEL_BODY_FONT, body_font);
        store_panel_font(&PANEL_BUTTON_FONT, button_font);

        let controls = [
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("把刚才的不舒服留给猫猫"),
                    control_style(0),
                    24,
                    18,
                    404,
                    30,
                    Some(window),
                    Some(control_menu(PANEL_TITLE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!(
                        "先开始反馈，让猫猫留意输入过程。\r\n不舒服时，保存刚才 30 秒。需要的话，再补一句说明。"
                    ),
                    control_style(0),
                    24,
                    54,
                    404,
                    42,
                    Some(window),
                    Some(control_menu(PANEL_DESCRIPTION_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("开始反馈"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    24,
                    110,
                    196,
                    36,
                    Some(window),
                    Some(control_menu(PANEL_START_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("保存近 30 秒"),
                    control_style(BS_OWNERDRAW | BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32),
                    232,
                    110,
                    196,
                    36,
                    Some(window),
                    Some(control_menu(PANEL_MARK_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("停止反馈"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    24,
                    158,
                    196,
                    36,
                    Some(window),
                    Some(control_menu(PANEL_STOP_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("补一句说明…"),
                    control_style(BS_OWNERDRAW | WS_TABSTOP.0 as i32),
                    232,
                    158,
                    196,
                    36,
                    Some(window),
                    Some(control_menu(PANEL_NOTE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("●  准备好了，关闭后仍在托盘待命"),
                    control_style(0),
                    24,
                    210,
                    404,
                    24,
                    Some(window),
                    Some(control_menu(PANEL_STATUS_ID)),
                    instance,
                    None,
                )
            },
        ];
        if controls.iter().any(Result::is_err) {
            delete_panel_fonts();
            return false;
        }
        let controls = controls.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        for (control, font) in controls.iter().zip([
            heading_font,
            body_font,
            button_font,
            button_font,
            button_font,
            button_font,
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
        true
    }

    unsafe fn show_panel(window: HWND) {
        let _ = unsafe { ShowWindow(window, SW_SHOW) };
        let _ = unsafe { SetForegroundWindow(window) };
        if let Ok(save_button) = unsafe { GetDlgItem(Some(window), PANEL_MARK_ID) } {
            let _ = unsafe { SetFocus(Some(save_button)) };
        }
    }

    unsafe fn show_menu(window: HWND) {
        // SAFETY: every acquired menu is destroyed before this function exits.
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            notify(window, "向猫猫许愿", "暂时无法打开菜单。", true);
            return;
        };
        let built = unsafe {
            AppendMenuW(menu, MF_STRING, MENU_OPEN as usize, w!("打开许愿面板"))
                .and_then(|_| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
                .and_then(|_| {
                    AppendMenuW(menu, MF_STRING, MENU_CLEAR as usize, w!("清除已停止会话"))
                })
                .and_then(|_| AppendMenuW(menu, MF_STRING, MENU_EXIT as usize, w!("退出")))
        };
        if built.is_err() {
            let _ = unsafe { DestroyMenu(menu) };
            notify(window, "向猫猫许愿", "暂时无法建立菜单。", true);
            return;
        }
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            let _ = unsafe { DestroyMenu(menu) };
            return;
        }
        let _ = unsafe { SetForegroundWindow(window) };
        let selected = unsafe {
            TrackPopupMenuEx(
                menu,
                TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
                point.x,
                point.y,
                window,
                None,
            )
        }
        .0 as u32;
        let _ = unsafe { DestroyMenu(menu) };
        let _ = unsafe { PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0)) };
        match selected {
            MENU_OPEN => unsafe { show_panel(window) },
            MENU_CLEAR => run_menu_command(window, WishCommand::ClearStopped),
            MENU_EXIT => {
                let _ = unsafe { DestroyWindow(window) };
            }
            _ => {}
        }
    }

    fn run_panel_command(window: HWND, command: WishCommand) {
        match dispatch_wish_command(command) {
            Ok(receipt) => {
                let (message, failed) = receipt_message(&receipt);
                set_panel_status(window, message);
                if failed {
                    notify(window, "向猫猫许愿", message, true);
                }
            }
            Err(_) => {
                let message = "命令没有发出；输入法本身未受影响。";
                set_panel_status(window, message);
                notify(window, "向猫猫许愿", message, true);
            }
        }
    }

    fn run_menu_command(window: HWND, command: WishCommand) {
        match dispatch_wish_command(command) {
            Ok(receipt) => {
                let (message, failed) = receipt_message(&receipt);
                set_panel_status(window, message);
                notify(window, "向猫猫许愿", message, failed);
            }
            Err(_) => notify(
                window,
                "向猫猫许愿",
                "命令没有发出；输入法本身未受影响。",
                true,
            ),
        }
    }

    fn receipt_message(receipt: &WishCommandDispatchReceipt) -> (&'static str, bool) {
        match receipt.acknowledgement() {
            Some(WishCommandAckStatus::Applied) => (
                match receipt.command() {
                    WishCommand::Start => "反馈已开始；许愿前暂不保存。",
                    WishCommand::SaveRecent => "最近 30 秒已在本地加密保存。",
                    WishCommand::Stop => "反馈已停止。",
                    WishCommand::ClearStopped => "已清除停止后的内存会话。",
                },
                false,
            ),
            Some(WishCommandAckStatus::NoChange) => (
                match receipt.command() {
                    WishCommand::Start => "反馈已经在记录。",
                    WishCommand::SaveRecent => "最近还没有可保存的输入法事件。",
                    WishCommand::Stop => "反馈当前没有在记录。",
                    WishCommand::ClearStopped => "当前没有可清除的已停止会话。",
                },
                false,
            ),
            Some(WishCommandAckStatus::Failed) => ("宿主收到命令，但未能完成操作。", true),
            None => (
                "没有新版输入法宿主响应；请新开一个使用自然码 Alpha 的输入框后重试。",
                true,
            ),
        }
    }

    fn set_panel_status(window: HWND, message: &str) {
        let Ok(status) = (unsafe { GetDlgItem(Some(window), PANEL_STATUS_ID) }) else {
            return;
        };
        let wide = message
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let _ = unsafe { SetWindowTextW(status, PCWSTR(wide.as_ptr())) };
    }

    fn wish_root_for_executable(executable: &Path) -> Option<PathBuf> {
        let release = executable.parent()?;
        let target = release.parent()?;
        let repository = target.parent()?;
        if release.file_name()?.to_str()? != "release" || target.file_name()?.to_str()? != "target"
        {
            return None;
        }
        Some(
            repository
                .join(".local")
                .join("tsf-alpha")
                .join("user-data")
                .join("wishes"),
        )
    }

    fn open_note_window(owner: HWND) {
        // SAFETY: the discovered window belongs to this single-instance
        // process and may be foregrounded without changing feedback state.
        if let Ok(existing) = unsafe { FindWindowW(w!("ZiranmaWishpadNoteWindow"), PCWSTR::null()) }
        {
            let _ = unsafe { SetForegroundWindow(existing) };
            return;
        }
        let Some(root) = std::env::current_exe()
            .ok()
            .and_then(|path| wish_root_for_executable(&path))
        else {
            notify(
                owner,
                "向猫猫许愿",
                "无法定位本项目的许愿目录；没有读取或写入任何内容。",
                true,
            );
            return;
        };
        let wish_id = match list_wish_packages(&root) {
            Ok(packages) => match packages.first() {
                Some(package) => package.id().to_owned(),
                None => {
                    notify(
                        owner,
                        "向猫猫许愿",
                        "还没有已保存的许愿；请先保存最近 30 秒。",
                        false,
                    );
                    return;
                }
            },
            Err(WishFeedbackError::RootUnavailable) => {
                notify(
                    owner,
                    "向猫猫许愿",
                    "还没有已保存的许愿；请先保存最近 30 秒。",
                    false,
                );
                return;
            }
            Err(_) => {
                notify(
                    owner,
                    "向猫猫许愿",
                    "暂时无法读取许愿列表；没有解密或修改任何内容。",
                    true,
                );
                return;
            }
        };
        let Ok(mut target) = NOTE_TARGET.lock() else {
            notify(owner, "向猫猫许愿", "暂时无法打开说明窗口。", true);
            return;
        };
        *target = Some(NoteTarget {
            owner: owner.0 as isize,
            root,
            wish_id,
        });
        drop(target);

        // SAFETY: this process owns the registered class and the note target
        // remains available until WM_NCDESTROY clears it.
        let created = unsafe {
            GetModuleHandleW(None).and_then(|module| {
                CreateWindowExW(
                    WS_EX_CONTROLPARENT,
                    w!("ZiranmaWishpadNoteWindow"),
                    w!("给猫猫留一句说明"),
                    WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    480,
                    350,
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
                notify(owner, "向猫猫许愿", "暂时无法建立说明窗口。", true);
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
                if unsafe { create_note_controls(window) } {
                    LRESULT(0)
                } else {
                    LRESULT(-1)
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
                    w!("问题类别"),
                    control_style(0),
                    20,
                    20,
                    76,
                    24,
                    Some(window),
                    None,
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("COMBOBOX"),
                    PCWSTR::null(),
                    control_style(CBS_DROPDOWNLIST | CBS_HASSTRINGS | WS_TABSTOP.0 as i32),
                    104,
                    16,
                    336,
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
                    w!("想告诉猫猫什么？"),
                    control_style(0),
                    20,
                    58,
                    420,
                    24,
                    Some(window),
                    None,
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
                            | WS_BORDER.0 as i32
                            | WS_TABSTOP.0 as i32
                            | WS_VSCROLL.0 as i32,
                    ),
                    20,
                    84,
                    420,
                    142,
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
                    w!("说明会和最近一条许愿一起加密保存在本机；不会联网。"),
                    control_style(0),
                    20,
                    236,
                    420,
                    38,
                    Some(window),
                    None,
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("保存说明"),
                    control_style(BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32),
                    258,
                    280,
                    86,
                    30,
                    Some(window),
                    Some(control_menu(NOTE_SAVE_ID)),
                    instance,
                    None,
                )
            },
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("取消"),
                    control_style(WS_TABSTOP.0 as i32),
                    354,
                    280,
                    86,
                    30,
                    Some(window),
                    Some(control_menu(NOTE_CANCEL_ID)),
                    instance,
                    None,
                )
            },
        ];
        if controls.iter().any(Result::is_err) {
            return false;
        }
        let controls = controls.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
        for control in &controls {
            let _ = unsafe {
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                )
            };
        }
        let category = controls[1];
        for label in ["候选", "排序", "显示", "延迟", "输入模式", "兼容性", "其他"]
        {
            let wide = label
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let _ = unsafe {
                SendMessageW(
                    category,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(wide.as_ptr() as isize)),
                )
            };
        }
        let _ = unsafe { SendMessageW(category, CB_SETCURSEL, Some(WPARAM(0)), None) };
        let _ = unsafe {
            SendMessageW(
                controls[3],
                EDIT_SET_LIMIT_TEXT,
                Some(WPARAM(NOTE_TEXT_CHARACTER_LIMIT)),
                None,
            )
        };
        true
    }

    fn wish_category_at(index: usize) -> Option<WishCategory> {
        [
            WishCategory::Candidates,
            WishCategory::Ranking,
            WishCategory::Display,
            WishCategory::Latency,
            WishCategory::InputMode,
            WishCategory::Compatibility,
            WishCategory::Other,
        ]
        .get(index)
        .copied()
    }

    unsafe fn save_note(window: HWND) {
        let Some(target) = NOTE_TARGET.lock().ok().and_then(|target| target.clone()) else {
            show_note_message(window, "这条许愿已经不可用，请关闭窗口后重试。", true);
            return;
        };
        let Ok(category_control) = (unsafe { GetDlgItem(Some(window), NOTE_CATEGORY_ID) }) else {
            show_note_message(window, "无法读取问题类别。", true);
            return;
        };
        let category_index = unsafe { SendMessageW(category_control, CB_GETCURSEL, None, None) }.0;
        let Some(category) = usize::try_from(category_index)
            .ok()
            .and_then(wish_category_at)
        else {
            show_note_message(window, "请选择一个问题类别。", false);
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
        match save_wish_note(&target.root, &note, &WindowsUserDataProtector) {
            Ok(()) => {
                notify(
                    HWND(target.owner as *mut c_void),
                    "向猫猫许愿",
                    "说明已和最近一条许愿一起加密保存在本机。",
                    false,
                );
                let _ = unsafe { DestroyWindow(window) };
            }
            Err(WishFeedbackError::NoteAlreadyExists) => show_note_message(
                window,
                "最近一条许愿已经有说明；当前版本不会覆盖原说明。",
                false,
            ),
            Err(_) => {
                show_note_message(window, "说明保存失败；原许愿没有被修改，也没有联网。", true)
            }
        }
    }

    fn show_note_message(window: HWND, message: &str, failed: bool) {
        let wide = message
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let style = MB_OK | if failed { MB_ICONERROR } else { MB_ICONWARNING };
        // SAFETY: the message is NUL-terminated for this synchronous call.
        unsafe {
            let _ = MessageBoxW(
                Some(window),
                PCWSTR(wide.as_ptr()),
                w!("给猫猫留一句说明"),
                style,
            );
        }
    }

    unsafe fn add_tray_icon(window: HWND) -> WindowsResult<()> {
        let mut data = tray_data(window);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
        let module = unsafe { GetModuleHandleW(None) }?;
        data.hIcon = unsafe { load_wishpad_icon(HINSTANCE(module.0)) }
            .or_else(|_| unsafe { LoadIconW(None, IDI_APPLICATION) })?;
        copy_wide("向猫猫许愿", &mut data.szTip);
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            Ok(())
        } else {
            Err(windows::core::Error::from_thread())
        }
    }

    unsafe fn load_wishpad_icon(
        instance: HINSTANCE,
    ) -> WindowsResult<windows::Win32::UI::WindowsAndMessaging::HICON> {
        let resource = PCWSTR(WISHPAD_ICON_RESOURCE_ID as *const u16);
        unsafe { LoadIconW(Some(instance), resource) }
    }

    unsafe fn delete_tray_icon(window: HWND) {
        let data = tray_data(window);
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }

    fn notify(window: HWND, title: &str, message: &str, failed: bool) {
        let mut data = tray_data(window);
        data.uFlags = NIF_INFO;
        copy_wide(title, &mut data.szInfoTitle);
        copy_wide(message, &mut data.szInfo);
        data.dwInfoFlags = if failed { NIIF_ERROR } else { NIIF_INFO };
        // SAFETY: the fixed buffers are NUL-terminated and the icon belongs to
        // this live hidden window.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn tray_data(window: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>())
                .expect("NOTIFYICONDATAW size fits u32"),
            hWnd: window,
            uID: TRAY_ICON_ID,
            ..Default::default()
        }
    }

    fn copy_wide<const N: usize>(text: &str, destination: &mut [u16; N]) {
        destination.fill(0);
        for (slot, unit) in destination
            .iter_mut()
            .take(N.saturating_sub(1))
            .zip(text.encode_utf16())
        {
            *slot = unit;
        }
    }

    pub fn show_fatal_error(message: &str) {
        let mut wide = message.encode_utf16().collect::<Vec<_>>();
        wide.push(0);
        // SAFETY: both strings are NUL-terminated for this synchronous call.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(wide.as_ptr()),
                w!("向猫猫许愿无法启动"),
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
        }

        #[test]
        fn save_recent_is_the_only_primary_panel_action() {
            assert_eq!(
                panel_button_tone(PANEL_MARK_ID),
                Some(PanelButtonTone::Primary)
            );
            for id in [PANEL_START_ID, PANEL_NOTE_ID, PANEL_STOP_ID] {
                assert_eq!(panel_button_tone(id), Some(PanelButtonTone::Secondary));
            }
            assert_eq!(panel_button_tone(PANEL_STATUS_ID), None);
        }

        #[test]
        fn note_storage_root_is_derived_only_from_the_release_binary_layout() {
            assert_eq!(
                wish_root_for_executable(Path::new(r"D:\repo\target\release\wishpad.exe")),
                Some(PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\wishes"))
            );
            assert!(wish_root_for_executable(Path::new(r"D:\tools\wishpad.exe")).is_none());
        }
    }
}
