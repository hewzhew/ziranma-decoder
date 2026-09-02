#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("自然码输入模式图标目前只支持 Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_indicator::run() {
        windows_indicator::show_error(&error.to_string());
    }
}

#[cfg(windows)]
mod windows_indicator {
    use std::error::Error;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr;
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT,
        WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{
        BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HGDIOBJ,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::Shell::{
        NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::TextServices::{
        CLSID_TF_ThreadMgr, ITfCompartment, ITfCompartmentEventSink, ITfCompartmentEventSink_Impl,
        ITfSource, ITfThreadMgr,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DestroyWindow, GWLP_USERDATA, GetCursorPos, GetMessageW, HICON,
        ICONINFO, MB_ICONERROR, MB_OK, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
        PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
        SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON,
        TrackPopupMenuEx, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
        WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    };
    use windows::core::{GUID, IUnknown, Interface, PCWSTR, Result as WindowsResult, implement, w};
    use ziranma_core::{INPUT_MODE_STATUS_COMPARTMENT_GUID, PublishedInputMode};

    const WINDOW_CLASS: PCWSTR = w!("ZiranmaModeIndicatorWindowV1");
    const INSTANCE_MUTEX: PCWSTR = w!("Local\\ZiranmaModeIndicatorV1");
    const TRAY_CALLBACK: u32 = WM_APP + 1;
    const MODE_CHANGED: u32 = WM_APP + 2;
    const TRAY_ICON_ID: u32 = 1;
    const MENU_EXIT: usize = 1;
    const ICON_SIZE: usize = 32;
    const TRAY_ICON_GUID: GUID = GUID::from_u128(0x68494ee1_88b6_4ed1_87c9_ad8ce40a08a7);

    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum IndicatorMode {
        Waiting,
        Chinese,
        English,
    }

    impl From<Option<PublishedInputMode>> for IndicatorMode {
        fn from(mode: Option<PublishedInputMode>) -> Self {
            match mode {
                Some(PublishedInputMode::Chinese) => Self::Chinese,
                Some(PublishedInputMode::English) => Self::English,
                None => Self::Waiting,
            }
        }
    }

    impl IndicatorMode {
        fn tooltip(self) -> &'static str {
            match self {
                Self::Waiting => "自然码 · 等待输入模式",
                Self::Chinese => "自然码 · 中文模式",
                Self::English => "自然码 · 英文模式",
            }
        }

        fn menu_status(self) -> &'static str {
            match self {
                Self::Waiting => "状态：等待新版输入法宿主",
                Self::Chinese => "状态：中文（中）",
                Self::English => "状态：英文（A）",
            }
        }
    }

    struct OwnedMutex(HANDLE);

    impl Drop for OwnedMutex {
        fn drop(&mut self) {
            // SAFETY: this is the one handle returned by CreateMutexW.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    struct ComApartment;

    impl ComApartment {
        fn enter() -> WindowsResult<Self> {
            // SAFETY: this GUI thread owns the matching CoUninitialize.
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
            Ok(Self)
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            // SAFETY: balances this thread's successful CoInitializeEx.
            unsafe { CoUninitialize() };
        }
    }

    #[implement(ITfCompartmentEventSink)]
    struct ModeCompartmentSink {
        window: HWND,
    }

    impl ITfCompartmentEventSink_Impl for ModeCompartmentSink_Impl {
        fn OnChange(&self, guid: *const GUID) -> WindowsResult<()> {
            if guid.is_null() {
                return Ok(());
            }
            // SAFETY: TSF keeps this GUID pointer valid for the synchronous
            // callback. Posting only a text-free wake-up avoids COM reentry.
            if unsafe { *guid } == INPUT_MODE_STATUS_COMPARTMENT_GUID {
                let _ =
                    unsafe { PostMessageW(Some(self.window), MODE_CHANGED, WPARAM(0), LPARAM(0)) };
            }
            Ok(())
        }
    }

    struct ModeSubscription {
        manager: ITfThreadMgr,
        client_id: u32,
        compartment: ITfCompartment,
        source: ITfSource,
        _sink: ITfCompartmentEventSink,
        cookie: u32,
    }

    impl ModeSubscription {
        fn connect(window: HWND) -> WindowsResult<Self> {
            // SAFETY: CLSID_TF_ThreadMgr is the system TSF thread manager.
            let manager: ITfThreadMgr = unsafe {
                CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
            }?;
            // SAFETY: this owned activation is balanced by Drop.
            let client_id = unsafe { manager.Activate() }?;
            let connected = (|| {
                // SAFETY: the project compartment contains one bounded VT_I4.
                let compartments = unsafe { manager.GetGlobalCompartment() }?;
                let compartment =
                    unsafe { compartments.GetCompartment(&INPUT_MODE_STATUS_COMPARTMENT_GUID) }?;
                let source: ITfSource = compartment.cast()?;
                let sink: ITfCompartmentEventSink = ModeCompartmentSink { window }.into();
                // SAFETY: source retains the sink until the matching unadvise.
                let cookie = unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, &sink) }?;
                Ok(Self {
                    manager: manager.clone(),
                    client_id,
                    compartment,
                    source,
                    _sink: sink,
                    cookie,
                })
            })();
            if connected.is_err() {
                // SAFETY: balances Activate on the partial failure path.
                let _ = unsafe { manager.Deactivate() };
            }
            connected
        }

        fn current(&self) -> Option<PublishedInputMode> {
            // SAFETY: GetValue returns an owned VARIANT and reads no input text.
            unsafe { self.compartment.GetValue() }
                .ok()
                .and_then(|value| i32::try_from(&value).ok())
                .and_then(PublishedInputMode::parse)
        }
    }

    impl Drop for ModeSubscription {
        fn drop(&mut self) {
            // SAFETY: balances the successful AdviseSink and Activate calls.
            let _ = unsafe { self.source.UnadviseSink(self.cookie) };
            let _ = unsafe { self.manager.Deactivate() };
            self.client_id = 0;
        }
    }

    struct TrayIcon {
        window: HWND,
        mode: IndicatorMode,
        icon: HICON,
    }

    impl TrayIcon {
        fn add(window: HWND, mode: IndicatorMode) -> WindowsResult<Self> {
            let icon = create_indicator_icon(mode)?;
            let tray = Self { window, mode, icon };
            if let Err(error) = tray.publish(NIM_ADD) {
                // SAFETY: icon was created exclusively for this tray object.
                let _ = unsafe { DestroyIcon(icon) };
                return Err(error);
            }
            if let Err(error) = tray.set_version() {
                let _ = tray.remove();
                // SAFETY: icon was created exclusively for this tray object.
                let _ = unsafe { DestroyIcon(icon) };
                return Err(error);
            }
            Ok(tray)
        }

        fn update(&mut self, mode: IndicatorMode) -> WindowsResult<()> {
            if self.mode == mode {
                return Ok(());
            }
            let icon = create_indicator_icon(mode)?;
            let old_icon = self.icon;
            let old_mode = self.mode;
            self.icon = icon;
            self.mode = mode;
            if let Err(error) = self.publish(NIM_MODIFY) {
                self.icon = old_icon;
                self.mode = old_mode;
                // SAFETY: the rejected replacement icon has no other owner.
                let _ = unsafe { DestroyIcon(icon) };
                return Err(error);
            }
            // SAFETY: Explorer copied the replacement icon synchronously.
            let _ = unsafe { DestroyIcon(old_icon) };
            Ok(())
        }

        fn restore_after_explorer_restart(&self) -> WindowsResult<()> {
            self.publish(NIM_ADD)?;
            self.set_version()
        }

        fn publish(
            &self,
            message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE,
        ) -> WindowsResult<()> {
            let data = self.data(NIF_GUID | NIF_ICON | NIF_MESSAGE | NIF_SHOWTIP | NIF_TIP);
            // SAFETY: the structure and icon remain live for this call.
            if unsafe { Shell_NotifyIconW(message, &data) }.as_bool() {
                Ok(())
            } else {
                Err(windows::core::Error::from_thread())
            }
        }

        fn set_version(&self) -> WindowsResult<()> {
            let mut data = self.data(NIF_GUID);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            // SAFETY: the structure remains live for this synchronous call.
            if unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
                Ok(())
            } else {
                Err(windows::core::Error::from_thread())
            }
        }

        fn data(
            &self,
            flags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS,
        ) -> NOTIFYICONDATAW {
            let mut data = NOTIFYICONDATAW {
                cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or(u32::MAX),
                hWnd: self.window,
                uID: TRAY_ICON_ID,
                uFlags: flags,
                uCallbackMessage: TRAY_CALLBACK,
                hIcon: self.icon,
                guidItem: TRAY_ICON_GUID,
                ..NOTIFYICONDATAW::default()
            };
            copy_wide_bounded(self.mode.tooltip(), &mut data.szTip);
            data
        }

        fn remove(&self) -> WindowsResult<()> {
            let data = self.data(NIF_GUID);
            // SAFETY: deletion identifies the exact icon owned by this object.
            if unsafe { Shell_NotifyIconW(NIM_DELETE, &data) }.as_bool() {
                Ok(())
            } else {
                Err(windows::core::Error::from_thread())
            }
        }
    }

    impl Drop for TrayIcon {
        fn drop(&mut self) {
            let _ = self.remove();
            // SAFETY: this object owns the current icon handle.
            let _ = unsafe { DestroyIcon(self.icon) };
        }
    }

    struct AppState {
        subscription: ModeSubscription,
        tray: TrayIcon,
    }

    impl AppState {
        fn refresh(&mut self) {
            let _ = self.tray.update(self.subscription.current().into());
        }

        fn show_menu(&self) {
            let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
                return;
            };
            let status = wide(self.tray.mode.menu_status());
            let exit = wide("退出状态图标");
            // SAFETY: menu and UTF-16 buffers remain valid for these calls.
            let appended = unsafe {
                AppendMenuW(menu, MF_GRAYED | MF_STRING, 0, PCWSTR(status.as_ptr()))
                    .and_then(|_| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
                    .and_then(|_| AppendMenuW(menu, MF_STRING, MENU_EXIT, PCWSTR(exit.as_ptr())))
            };
            if appended.is_ok() {
                let mut point = POINT::default();
                // SAFETY: point is writable and the hidden window owns the menu.
                if unsafe { GetCursorPos(&mut point) }.is_ok() {
                    let _ = unsafe { SetForegroundWindow(self.tray.window) };
                    let flags =
                        (TPM_BOTTOMALIGN | TPM_RIGHTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0;
                    // SAFETY: the menu and owner are live for modal tracking.
                    let selected = unsafe {
                        TrackPopupMenuEx(menu, flags, point.x, point.y, self.tray.window, None)
                    }
                    .0;
                    if usize::try_from(selected).ok() == Some(MENU_EXIT) {
                        // SAFETY: ends only this indicator's message loop.
                        unsafe { PostQuitMessage(0) };
                    } else {
                        let _ = unsafe {
                            PostMessageW(Some(self.tray.window), WM_NULL, WPARAM(0), LPARAM(0))
                        };
                    }
                }
            }
            // SAFETY: this is the one menu handle created above.
            let _ = unsafe { DestroyMenu(menu) };
        }
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let mutex = unsafe { CreateMutexW(None, false, INSTANCE_MUTEX) }?;
        let mutex = OwnedMutex(mutex);
        // GetLastError immediately follows CreateMutexW as required.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _apartment = ComApartment::enter()?;
        // SAFETY: fixed string registration reads no external state.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        TASKBAR_CREATED.store(taskbar_created, Ordering::Release);

        // SAFETY: a null module name requests the current executable module.
        let instance = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: WINDOW_CLASS,
            ..WNDCLASSW::default()
        };
        // SAFETY: class contains stable static strings and one valid procedure.
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        // SAFETY: this hidden top-level window exists only to receive tray and
        // compartment notifications; it is never activated or displayed.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                WINDOW_CLASS,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }?;
        let subscription = ModeSubscription::connect(window)?;
        let tray = TrayIcon::add(window, subscription.current().into())?;
        let mut state = Box::new(AppState { subscription, tray });
        // SAFETY: the box keeps this stable address until the loop exits.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, ptr::from_mut(&mut *state) as isize) };

        let mut message = MSG::default();
        loop {
            // SAFETY: standard message loop for this GUI thread.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
            if result == -1 {
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
                drop(state);
                let _ = unsafe { DestroyWindow(window) };
                return Err(windows::core::Error::from_thread().into());
            }
            if result == 0 {
                break;
            }
            // SAFETY: no accelerator table or modeless dialog is used.
            unsafe {
                let _ = TranslateMessage(&message);
                windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&message);
            }
        }

        // SAFETY: no later message may observe the soon-to-be-dropped state.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
        drop(state);
        let _ = unsafe { DestroyWindow(window) };
        drop(mutex);
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let state = unsafe {
            (windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(window, GWLP_USERDATA)
                as *mut AppState)
                .as_mut()
        };
        if message == TASKBAR_CREATED.load(Ordering::Acquire) {
            if let Some(state) = state {
                let _ = state.tray.restore_after_explorer_restart();
            }
            return LRESULT(0);
        }
        match message {
            MODE_CHANGED => {
                if let Some(state) = state {
                    state.refresh();
                }
                LRESULT(0)
            }
            TRAY_CALLBACK => {
                let notification = (lparam.0 as u32) & 0xffff;
                if notification == WM_CONTEXTMENU || notification == WM_RBUTTONUP {
                    if let Some(state) = state {
                        state.show_menu();
                    }
                } else if notification == NIN_SELECT {
                    // A primary click is intentionally informational only.
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // SAFETY: ends only this indicator's message loop.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: defensive fallback if Windows destroys the owner.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    fn create_indicator_icon(mode: IndicatorMode) -> WindowsResult<HICON> {
        let pixels = indicator_pixels(mode);
        let header = BITMAPV5HEADER {
            bV5Size: u32::try_from(size_of::<BITMAPV5HEADER>()).unwrap_or(u32::MAX),
            bV5Width: ICON_SIZE as i32,
            bV5Height: -(ICON_SIZE as i32),
            bV5Planes: 1,
            bV5BitCount: 32,
            bV5Compression: BI_BITFIELDS,
            bV5SizeImage: u32::try_from(pixels.len() * size_of::<u32>()).unwrap_or(u32::MAX),
            bV5RedMask: 0x00ff_0000,
            bV5GreenMask: 0x0000_ff00,
            bV5BlueMask: 0x0000_00ff,
            bV5AlphaMask: 0xff00_0000,
            ..BITMAPV5HEADER::default()
        };
        let mut bits: *mut c_void = ptr::null_mut();
        // SAFETY: the V5 header prefix is accepted through BITMAPINFO and the
        // returned writable allocation is owned by the color bitmap.
        let color = unsafe {
            CreateDIBSection(
                None,
                ptr::from_ref(&header).cast::<BITMAPINFO>(),
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }?;
        if bits.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: the DIB section has exactly the byte size declared above.
        unsafe {
            ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        }
        // A 1-bpp zero mask lets the 32-bpp alpha channel define shape. Each
        // scanline is DWORD-aligned at this fixed 32 px size.
        let mask_bits = [0_u8; ICON_SIZE * 4];
        let mask = unsafe {
            CreateBitmap(
                ICON_SIZE as i32,
                ICON_SIZE as i32,
                1,
                1,
                Some(mask_bits.as_ptr().cast()),
            )
        };
        if mask.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
            return Err(windows::core::Error::from_thread());
        }
        let info = ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..ICONINFO::default()
        };
        // SAFETY: both bitmaps remain live for the complete icon copy.
        let icon = unsafe { CreateIconIndirect(&info) };
        let _ = unsafe { DeleteObject(HGDIOBJ(mask.0)) };
        let _ = unsafe { DeleteObject(HGDIOBJ(color.0)) };
        icon
    }

    fn indicator_pixels(mode: IndicatorMode) -> [u32; ICON_SIZE * ICON_SIZE] {
        const CAT: [u16; 16] = [
            0x0000, 0x0000, 0x4100, 0x6300, 0x9c80, 0x8080, 0xa280, 0x8080, 0x8880, 0x9480, 0x6300,
            0x3e00, 0x0000, 0x0000, 0x0000, 0x0000,
        ];
        const CHINESE: [u16; 16] = [
            0x0000, 0x0000, 0x0008, 0x003e, 0x002a, 0x002a, 0x002a, 0x003e, 0x0008, 0x0008, 0x0008,
            0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
        ];
        const ENGLISH: [u16; 16] = [
            0x0000, 0x0000, 0x0008, 0x0014, 0x0014, 0x0022, 0x0022, 0x003e, 0x0022, 0x0022, 0x0022,
            0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
        ];
        const WAITING: [u16; 16] = [
            0x0000, 0x0000, 0x001c, 0x0022, 0x0002, 0x0004, 0x0008, 0x0008, 0x0000, 0x0008, 0x0000,
            0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
        ];
        const TRANSPARENT: u32 = 0x0000_0000;
        const CAT_OUTLINE: u32 = 0xff4b_403c;
        const CAT_FILL: u32 = 0xffe8_cfc2;
        const BADGE_BLUE: u32 = 0xff5d_a9e8;
        const BADGE_WAITING: u32 = 0xff8b_929a;
        const BADGE_INK: u32 = 0xffff_ffff;

        let badge = match mode {
            IndicatorMode::Waiting => WAITING,
            IndicatorMode::Chinese => CHINESE,
            IndicatorMode::English => ENGLISH,
        };
        let mut pixels = [TRANSPARENT; ICON_SIZE * ICON_SIZE];
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let source_x = x / 2;
                let source_y = y / 2;
                let cat = CAT[source_y] & (1 << (15 - source_x)) != 0;
                let near_cat = !cat
                    && (-1_i32..=1).any(|dy| {
                        (-1_i32..=1).any(|dx| {
                            let neighbor_x = source_x as i32 + dx;
                            let neighbor_y = source_y as i32 + dy;
                            (0..16).contains(&neighbor_x)
                                && (0..16).contains(&neighbor_y)
                                && CAT[neighbor_y as usize] & (1 << (15 - neighbor_x)) != 0
                        })
                    });
                if near_cat {
                    pixels[y * ICON_SIZE + x] = CAT_OUTLINE;
                }
                if cat {
                    pixels[y * ICON_SIZE + x] = CAT_FILL;
                }
            }
        }

        // The badge is a separate rounded capsule with a two-pixel gap from
        // the cat. It remains readable on both light and dark taskbars.
        for y in 4_usize..25 {
            for x in 19_usize..32 {
                let corner_x = if x < 23 { 23 - x } else { x.saturating_sub(28) };
                let corner_y = if y < 8 { 8 - y } else { y.saturating_sub(20) };
                if corner_x * corner_x + corner_y * corner_y <= 20 {
                    pixels[y * ICON_SIZE + x] = if mode == IndicatorMode::Waiting {
                        BADGE_WAITING
                    } else {
                        BADGE_BLUE
                    };
                }
            }
        }
        for (source_y, badge_row) in badge.iter().enumerate() {
            for source_x in 10..15 {
                if badge_row & (1 << (15 - source_x)) != 0 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let x = source_x * 2 + dx;
                            let y = source_y * 2 + dy;
                            pixels[y * ICON_SIZE + x] = BADGE_INK;
                        }
                    }
                }
            }
        }
        pixels
    }

    fn copy_wide_bounded(text: &str, target: &mut [u16]) {
        if target.is_empty() {
            return;
        }
        let capacity = target.len().saturating_sub(1);
        target.fill(0);
        for (slot, unit) in target.iter_mut().take(capacity).zip(text.encode_utf16()) {
            *slot = unit;
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn show_error(message: &str) {
        let message = wide(message);
        let title = wide("自然码输入模式图标");
        // SAFETY: both strings remain live for this synchronous call.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn mode_labels_and_tooltips_are_distinct() {
            assert_ne!(
                IndicatorMode::Chinese.tooltip(),
                IndicatorMode::English.tooltip()
            );
            assert_ne!(
                IndicatorMode::Chinese.menu_status(),
                IndicatorMode::English.menu_status()
            );
            assert!(IndicatorMode::Waiting.tooltip().contains("等待"));
        }

        #[test]
        fn icon_keeps_cat_and_mode_badge_in_separate_regions() {
            let chinese = indicator_pixels(IndicatorMode::Chinese);
            let english = indicator_pixels(IndicatorMode::English);
            assert!(chinese[..].contains(&0xffe8_cfc2));
            assert!(chinese[..].contains(&0xff5d_a9e8));
            for y in 0..ICON_SIZE {
                for x in 0..19 {
                    assert_eq!(
                        chinese[y * ICON_SIZE + x],
                        english[y * ICON_SIZE + x],
                        "the stable cat region must not change with the mode"
                    );
                }
            }
            assert_ne!(chinese, english);
        }

        #[test]
        fn bounded_utf16_copy_always_leaves_a_terminator() {
            let mut target = [0xffff; 5];
            copy_wide_bounded("自然码输入模式", &mut target);
            assert_eq!(target[4], 0);
        }
    }
}
