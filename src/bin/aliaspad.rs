#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("固定候选面板目前只支持 Windows");
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
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_3DFACE, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
        COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject,
        FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GetSysColor, GetSysColorBrush, HGDIOBJ,
        OUT_DEFAULT_PRECIS, SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
        DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_LEFT, FindWindowW,
        GetDlgItem, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IDYES,
        IsDialogMessageW, LoadCursorW, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO, MSG,
        MessageBoxW, PostQuitMessage, RegisterClassW, SW_SHOW, SendMessageW, SetForegroundWindow,
        SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
        WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_BORDER,
        WS_CAPTION, WS_CHILD, WS_EX_CONTROLPARENT, WS_MINIMIZEBOX, WS_SYSMENU, WS_TABSTOP,
        WS_VISIBLE,
    };
    use windows::core::{PCWSTR, w};

    const CODE_EDIT_ID: i32 = 101;
    const TEXT_EDIT_ID: i32 = 102;
    const PIN_BUTTON_ID: i32 = 103;
    const UNPIN_BUTTON_ID: i32 = 104;
    const ROLLBACK_BUTTON_ID: i32 = 105;
    const REFRESH_BUTTON_ID: i32 = 106;
    const STATUS_ID: i32 = 107;
    const DESCRIPTION_ID: i32 = 108;
    const TITLE_ID: i32 = 109;
    const CODE_LABEL_ID: i32 = 110;
    const TEXT_LABEL_ID: i32 = 111;
    const EDIT_SET_LIMIT_TEXT: u32 = 0x00c5;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const MAX_CHILD_OUTPUT_BYTES: usize = 8 * 1024;
    const MAX_CODE_BYTES: usize = 64;
    const MAX_TEXT_CHARACTERS: usize = 64;

    static HEADING_FONT: AtomicIsize = AtomicIsize::new(0);
    static BODY_FONT: AtomicIsize = AtomicIsize::new(0);
    static BUTTON_FONT: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        static APP_STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
    }

    #[derive(Clone)]
    struct AppState {
        aliasctl: PathBuf,
        alias_root: PathBuf,
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let executable = std::env::current_exe()?;
        let state = paths_from_executable(&executable)?;
        if !state.aliasctl.is_file() {
            return Err("缺少 aliasctl.exe；请先构建 aliasctl 与 aliaspad".into());
        }

        // SAFETY: this process owns the class and message loop for the full
        // lifetime of the window created below.
        unsafe {
            let module = GetModuleHandleW(None)?;
            let instance = HINSTANCE(module.0);
            let class_name = w!("ZiranmaAliaspadWindow");
            if let Ok(existing) = FindWindowW(class_name, PCWSTR::null()) {
                let _ = ShowWindow(existing, SW_SHOW);
                let _ = SetForegroundWindow(existing);
                return Ok(());
            }
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
                return Err("无法注册固定候选面板窗口".into());
            }
            APP_STATE.with(|slot| slot.replace(Some(state)));
            let window = match CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class_name,
                w!("固定候选"),
                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                540,
                390,
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
            refresh_status(window, None);
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetForegroundWindow(window);
            focus_code(window);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0).0;
                if result == -1 {
                    return Err("固定候选面板消息循环失败".into());
                }
                if result == 0 {
                    break;
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
                match (wparam.0 & 0xffff) as i32 {
                    PIN_BUTTON_ID => pin_candidate(window),
                    UNPIN_BUTTON_ID => unpin_candidate(window),
                    ROLLBACK_BUTTON_ID => rollback(window),
                    REFRESH_BUTTON_ID => refresh_status(window, None),
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
        // SAFETY: each synchronous DWM call receives a correctly sized fixed
        // value. Older systems may reject these best-effort refinements.
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
        let heading_font = unsafe { create_font(20, FW_SEMIBOLD.0) };
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
                w!("STATIC"),
                w!("把喜欢的候选稳稳固定下来"),
                24,
                18,
                480,
                30,
                TITLE_ID,
                0,
                instance,
            ),
            create_control(
                window,
                w!("STATIC"),
                w!("只保存你主动填写的映射。新的输入组合立即生效，换代后仍会保留。"),
                24,
                54,
                480,
                38,
                DESCRIPTION_ID,
                0,
                instance,
            ),
            create_control(
                window,
                w!("STATIC"),
                w!("输入码"),
                24,
                104,
                112,
                24,
                CODE_LABEL_ID,
                0,
                instance,
            ),
            create_control(
                window,
                w!("EDIT"),
                PCWSTR::null(),
                24,
                128,
                200,
                30,
                CODE_EDIT_ID,
                WS_BORDER.0 as i32 | WS_TABSTOP.0 as i32 | ES_LEFT | ES_AUTOHSCROLL,
                instance,
            ),
            create_control(
                window,
                w!("STATIC"),
                w!("首选文字"),
                240,
                104,
                264,
                24,
                TEXT_LABEL_ID,
                0,
                instance,
            ),
            create_control(
                window,
                w!("EDIT"),
                PCWSTR::null(),
                240,
                128,
                264,
                30,
                TEXT_EDIT_ID,
                WS_BORDER.0 as i32 | WS_TABSTOP.0 as i32 | ES_LEFT | ES_AUTOHSCROLL,
                instance,
            ),
            create_control(
                window,
                w!("BUTTON"),
                w!("固定为首选"),
                24,
                178,
                232,
                38,
                PIN_BUTTON_ID,
                BS_PUSHBUTTON | BS_DEFPUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                w!("BUTTON"),
                w!("移除这个码"),
                272,
                178,
                232,
                38,
                UNPIN_BUTTON_ID,
                BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                w!("BUTTON"),
                w!("撤销上次切换"),
                24,
                228,
                232,
                34,
                ROLLBACK_BUTTON_ID,
                BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                w!("BUTTON"),
                w!("刷新状态"),
                272,
                228,
                232,
                34,
                REFRESH_BUTTON_ID,
                BS_PUSHBUTTON | WS_TABSTOP.0 as i32,
                instance,
            ),
            create_control(
                window,
                w!("STATIC"),
                w!("正在读取本地状态…"),
                24,
                280,
                480,
                54,
                STATUS_ID,
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
            } else if (6..=9).contains(&index) {
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
        if let Ok(code) = unsafe { GetDlgItem(Some(window), CODE_EDIT_ID) } {
            let _ = unsafe {
                SendMessageW(
                    code,
                    EDIT_SET_LIMIT_TEXT,
                    Some(WPARAM(MAX_CODE_BYTES)),
                    None,
                )
            };
        }
        if let Ok(text) = unsafe { GetDlgItem(Some(window), TEXT_EDIT_ID) } {
            let _ = unsafe {
                SendMessageW(
                    text,
                    EDIT_SET_LIMIT_TEXT,
                    Some(WPARAM(MAX_TEXT_CHARACTERS * 2)),
                    None,
                )
            };
        }
        true
    }

    // The small wrapper deliberately mirrors CreateWindowExW's positional
    // geometry so each static layout row remains readable at its call site.
    #[allow(clippy::too_many_arguments)]
    fn create_control(
        window: HWND,
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
        // SAFETY: parent, class strings and labels remain valid for this
        // synchronous standard-control creation call.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
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
                // SAFETY: fonts are process-owned and no longer needed while
                // the window and all child controls are being destroyed.
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
            DESCRIPTION_ID => unsafe { GetSysColor(COLOR_GRAYTEXT) },
            STATUS_ID => unsafe { GetSysColor(COLOR_HIGHLIGHT) },
            _ => unsafe { GetSysColor(COLOR_WINDOWTEXT) },
        });
        unsafe {
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = SetTextColor(hdc, color);
        }
        LRESULT(unsafe { GetSysColorBrush(COLOR_WINDOW) }.0 as isize)
    }

    fn pin_candidate(window: HWND) {
        let code = match read_control(window, CODE_EDIT_ID) {
            Ok(code) => code,
            Err(error) => return notify_error(window, &error),
        };
        let text = match read_control(window, TEXT_EDIT_ID) {
            Ok(text) => text,
            Err(error) => return notify_error(window, &error),
        };
        if let Err(error) = validate_code(&code).and_then(|_| validate_text(&text)) {
            notify_error(window, error);
            return;
        }
        let mut private_input = format!("{code}\n{text}\n").into_bytes();
        let result = run_aliasctl("pin", Some(&private_input));
        private_input.fill(0);
        match result {
            Ok(_) => {
                clear_control(window, CODE_EDIT_ID);
                clear_control(window, TEXT_EDIT_ID);
                refresh_status(window, Some("已固定。新的输入组合会使用它。"));
                focus_code(window);
            }
            Err(error) => notify_error(window, &error),
        }
    }

    fn unpin_candidate(window: HWND) {
        let code = match read_control(window, CODE_EDIT_ID) {
            Ok(code) => code,
            Err(error) => return notify_error(window, &error),
        };
        if let Err(error) = validate_code(&code) {
            notify_error(window, error);
            return;
        }
        let mut private_input = format!("{code}\n").into_bytes();
        let result = run_aliasctl("unpin", Some(&private_input));
        private_input.fill(0);
        match result {
            Ok(output) if output.contains("没有变化") => {
                refresh_status(window, Some("这个码原本没有固定首选。"));
            }
            Ok(_) => {
                clear_control(window, CODE_EDIT_ID);
                clear_control(window, TEXT_EDIT_ID);
                refresh_status(window, Some("已移除。需要时仍可撤销上次切换。"));
                focus_code(window);
            }
            Err(error) => notify_error(window, &error),
        }
    }

    fn rollback(window: HWND) {
        // SAFETY: the modal question belongs to this panel and does not expose
        // any private alias content.
        let choice = unsafe {
            MessageBoxW(
                Some(window),
                w!("回到上一个已启用版本吗？当前版本会保留下来，之后仍可再次撤销。"),
                w!("撤销上次切换"),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        if choice != IDYES {
            return;
        }
        match run_aliasctl("rollback", None) {
            Ok(_) => refresh_status(window, Some("已回到上一个版本。")),
            Err(error) => notify_error(window, &error),
        }
    }

    fn refresh_status(window: HWND, action: Option<&str>) {
        let status = match run_aliasctl("status", None) {
            Ok(output) => compact_status(&output),
            Err(error) => {
                notify_error(window, &error);
                "状态暂时不可用。".to_owned()
            }
        };
        let display = match action {
            Some(action) => format!("●  {action}\r\n{status}"),
            None => format!("●  {status}"),
        };
        set_control_text(window, STATUS_ID, &display);
    }

    fn compact_status(output: &str) -> String {
        let mut lines = output
            .lines()
            .filter(|line| !line.contains("加密：") && !line.contains("网络："))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return "状态暂时不可用。".to_owned();
        }
        if lines.len() > 4 {
            lines.truncate(4);
        }
        lines.join(" · ")
    }

    fn run_aliasctl(action: &str, private_input: Option<&[u8]>) -> Result<String, String> {
        APP_STATE.with(|slot| {
            let state = slot
                .borrow()
                .as_ref()
                .cloned()
                .ok_or_else(|| "固定候选面板尚未准备好".to_owned())?;
            let mut command = Command::new(&state.aliasctl);
            command
                .arg(action)
                .arg("--root")
                .arg(&state.alias_root)
                .stdin(if private_input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW);
            if private_input.is_some() {
                command.arg("--private-stdin");
            }
            let mut child = command
                .spawn()
                .map_err(|_| "无法启动本地别名管理器".to_owned())?;
            if let Some(input) = private_input {
                let Some(mut stdin) = child.stdin.take() else {
                    return Err("无法建立私密输入通道".to_owned());
                };
                stdin
                    .write_all(input)
                    .and_then(|_| stdin.flush())
                    .map_err(|_| "无法写入私密输入通道".to_owned())?;
            }
            let output = child
                .wait_with_output()
                .map_err(|_| "本地别名管理器没有正常结束".to_owned())?;
            if output.stdout.len() > MAX_CHILD_OUTPUT_BYTES
                || output.stderr.len() > MAX_CHILD_OUTPUT_BYTES
            {
                return Err("本地别名管理器输出超过上限".to_owned());
            }
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .map_err(|_| "本地别名管理器返回了无效文字".to_owned())
            } else {
                let error = String::from_utf8(output.stderr)
                    .ok()
                    .and_then(|text| text.lines().next().map(str::trim).map(str::to_owned))
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "固定候选操作失败".to_owned());
                Err(error)
            }
        })
    }

    fn read_control(window: HWND, id: i32) -> Result<String, String> {
        let control =
            unsafe { GetDlgItem(Some(window), id) }.map_err(|_| "无法读取输入框".to_owned())?;
        let length = unsafe { GetWindowTextLengthW(control) };
        if !(0..=256).contains(&length) {
            return Err("输入内容超过上限".to_owned());
        }
        let mut buffer = vec![0_u16; usize::try_from(length).unwrap_or(0).saturating_add(1)];
        let copied = unsafe { GetWindowTextW(control, &mut buffer) };
        if copied < 0 {
            return Err("无法读取输入框".to_owned());
        }
        buffer.truncate(usize::try_from(copied).unwrap_or(0));
        String::from_utf16(&buffer).map_err(|_| "输入内容不是有效文字".to_owned())
    }

    fn validate_code(code: &str) -> Result<(), &'static str> {
        if code.is_empty()
            || code.len() > MAX_CODE_BYTES
            || !code.as_bytes().iter().all(u8::is_ascii_lowercase)
        {
            return Err("输入码请使用 1–64 个小写字母。");
        }
        Ok(())
    }

    fn validate_text(text: &str) -> Result<(), &'static str> {
        let characters = text.chars().count();
        if characters == 0 || characters > MAX_TEXT_CHARACTERS || text.chars().any(char::is_control)
        {
            return Err("首选文字请填写 1–64 个普通字符。");
        }
        Ok(())
    }

    fn clear_control(window: HWND, id: i32) {
        set_control_text(window, id, "");
    }

    fn set_control_text(window: HWND, id: i32, text: &str) {
        if let Ok(control) = unsafe { GetDlgItem(Some(window), id) } {
            let text = wide(text);
            let _ = unsafe { SetWindowTextW(control, PCWSTR(text.as_ptr())) };
        }
    }

    fn focus_code(window: HWND) {
        if let Ok(control) = unsafe { GetDlgItem(Some(window), CODE_EDIT_ID) } {
            let _ = unsafe { SetFocus(Some(control)) };
        }
    }

    fn notify_error(window: HWND, message: &str) {
        let message = wide(message);
        // SAFETY: the UTF-16 buffer lives through the modal call.
        unsafe {
            let _ = MessageBoxW(
                Some(window),
                PCWSTR(message.as_ptr()),
                w!("固定候选"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    pub fn show_fatal_error(message: &str) {
        let message = wide(message);
        // SAFETY: the UTF-16 buffer lives through the modal call.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                w!("固定候选无法启动"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn paths_from_executable(executable: &Path) -> Result<AppState, Box<dyn Error>> {
        let binary_directory = executable.parent().ok_or("无法确定程序目录")?;
        let target_directory = binary_directory.parent().ok_or("无法确定 target 目录")?;
        if !matches!(
            binary_directory.file_name().and_then(|name| name.to_str()),
            Some("debug" | "release")
        ) || target_directory.file_name().and_then(|name| name.to_str()) != Some("target")
        {
            return Err("固定候选面板必须从项目的 target\\debug 或 target\\release 运行".into());
        }
        let repository = target_directory.parent().ok_or("无法确定项目目录")?;
        Ok(AppState {
            aliasctl: binary_directory.join("aliasctl.exe"),
            alias_root: repository
                .join(".local")
                .join("tsf-alpha")
                .join("user-data")
                .join("aliases"),
        })
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
        use super::*;

        #[test]
        fn executable_layout_keeps_private_aliases_outside_versioned_binaries() {
            let state =
                paths_from_executable(Path::new(r"D:\repo\target\release\aliaspad.exe")).unwrap();
            assert_eq!(
                state.aliasctl,
                PathBuf::from(r"D:\repo\target\release\aliasctl.exe")
            );
            assert_eq!(
                state.alias_root,
                PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\aliases")
            );
            assert!(paths_from_executable(Path::new(r"D:\repo\aliaspad.exe")).is_err());
        }

        #[test]
        fn compact_status_keeps_counts_and_drops_repeated_transport_details() {
            let display = compact_status(
                "显式别名：已启用\n  当前：2 条，校验通过\n  待切换：无\n  可回退：1 条，校验通过\n  加密：Windows 当前用户\n  网络：未连接\n",
            );
            assert_eq!(
                display,
                "显式别名：已启用 · 当前：2 条，校验通过 · 待切换：无 · 可回退：1 条，校验通过"
            );
        }

        #[test]
        fn local_validation_matches_the_bounded_alias_fields() {
            assert!(validate_code("qnq").is_ok());
            assert!(validate_code("Qnq").is_err());
            assert!(validate_text("亲亲").is_ok());
            assert!(validate_text("含\n换行").is_err());
        }
    }
}
