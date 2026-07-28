#[cfg(not(windows))]
fn main() {
    eprintln!("codex-recorder is available only on Windows");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_recorder::run()
}

#[cfg(windows)]
mod windows_recorder {
    use std::ffi::c_void;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::mem::ManuallyDrop;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows::Win32::Foundation::{LPARAM, LRESULT, WAIT_FAILED, WPARAM};
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, SAFEARRAY,
    };
    use windows::Win32::System::Ole::{
        SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    };
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0_0, VT_BSTR, VT_I4, VariantClear};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomation3, IUIAutomationCacheRequest,
        IUIAutomationCondition, IUIAutomationElement, IUIAutomationEventHandler,
        IUIAutomationEventHandler_Impl, IUIAutomationFocusChangedEventHandler,
        IUIAutomationFocusChangedEventHandler_Impl, IUIAutomationTextEditPattern,
        IUIAutomationTextEditTextChangedEventHandler,
        IUIAutomationTextEditTextChangedEventHandler_Impl, IUIAutomationTextPattern,
        IUIAutomationValuePattern, TextEditChangeType, TextEditChangeType_Composition,
        TextEditChangeType_CompositionFinalized, TextPatternRangeEndpoint_End,
        TextPatternRangeEndpoint_Start, TreeScope_Descendants, TreeScope_Element,
        UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_E_ELEMENTNOTAVAILABLE,
        UIA_E_TIMEOUT, UIA_EditControlTypeId, UIA_NamePropertyId, UIA_Text_TextChangedEventId,
        UIA_TextEditPatternId, UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
        VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F10, VK_F12, VK_HOME,
        VK_LEFT, VK_LWIN, VK_MENU, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetWindowThreadProcessId, HC_ACTION,
        KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, MSG, MWMO_INPUTAVAILABLE,
        MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_QUIT,
        WM_SYSKEYDOWN,
    };
    use windows::core::{BSTR, Interface, PCWSTR, Ref, Result as WindowsResult, implement};

    use ziranma_core::{
        CODEX_CAPTURE_PROFILE_V2, CONTINUOUS_PRODUCER_VERSION, CaptureIntegrityCountersV1,
        CaptureSessionKind, LocalInputTracker, ProtectedSegmentWriter,
        ProtectedSegmentWriterConfig, RawKey, SegmentCloseReason, SegmentWriteReceipt,
        TextSelection, WindowsUserDataProtector,
    };

    const TARGET_NAME: &str = "随心输入";
    const TARGET_FRAMEWORK: &str = "Chrome";
    const TARGET_CLASS: &str = "ProseMirror";
    const TARGET_DOCUMENT_NAME: &str = "Codex";
    const CONTROL_STATE_SCHEMA: &str = "ziranma-recorder-active-v1";
    const PAUSE_HOTKEY_ID: i32 = 0x5A58;
    const STOP_HOTKEY_ID: i32 = 0x5A59;
    const TARGET_POLL_INTERVAL: Duration = Duration::from_millis(250);
    const TARGET_HEALTH_INTERVAL: Duration = Duration::from_secs(1);
    const RECONNECT_NOTICE_DELAY: Duration = Duration::from_secs(2);
    const MESSAGE_WAIT_TIMEOUT_MS: u32 = 250;
    const DEFAULT_SEGMENT_EVENTS: usize = 128;
    const DEFAULT_FLUSH_SECONDS: u64 = 60;
    const MAX_SEGMENT_EVENTS: usize = 512;
    const MIN_FLUSH_SECONDS: u64 = 5;
    const MAX_FLUSH_SECONDS: u64 = 15 * 60;

    #[derive(Debug)]
    struct Options {
        check_only: bool,
        metadata_only: bool,
        control_state: bool,
        session_kind: CaptureSessionKind,
        segment_events: usize,
        flush_seconds: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum DiscoveryStatus {
        Waiting,
        Ambiguous(usize),
    }

    #[derive(Debug)]
    enum AttachAttemptError {
        Retryable(&'static str),
        Fatal(String),
    }

    impl std::fmt::Display for AttachAttemptError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Retryable(reason) => formatter.write_str(reason),
                Self::Fatal(error) => formatter.write_str(error),
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    struct TargetPolicyAudit {
        named_edits: usize,
        safe_patterns: usize,
        chrome_framework: usize,
        prose_mirror_class: usize,
        codex_document_ancestor: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ControlPhase {
        Running,
        Paused,
        Stopped,
        Failed,
    }

    impl ControlPhase {
        fn as_str(self) -> &'static str {
            match self {
                Self::Running => "running",
                Self::Paused => "paused",
                Self::Stopped => "stopped",
                Self::Failed => "failed",
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ControlTarget {
        Waiting,
        Connected,
    }

    impl ControlTarget {
        fn as_str(self) -> &'static str {
            match self {
                Self::Waiting => "waiting",
                Self::Connected => "connected",
            }
        }
    }

    struct ControlStatePublisher {
        path: PathBuf,
        session_id: String,
        session_kind: CaptureSessionKind,
        started_unix_ms: u64,
        phase: ControlPhase,
        target: ControlTarget,
        saved_segments: u64,
        saved_events: u64,
        last_flush_unix_ms: Option<u64>,
    }

    impl ControlStatePublisher {
        fn new(
            session_id: String,
            session_kind: CaptureSessionKind,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            Ok(Self {
                path: prepare_control_state_path()?,
                session_id,
                session_kind,
                started_unix_ms: current_unix_ms()?,
                phase: ControlPhase::Running,
                target: ControlTarget::Waiting,
                saved_segments: 0,
                saved_events: 0,
                last_flush_unix_ms: None,
            })
        }

        fn publish(&self) -> Result<(), Box<dyn std::error::Error>> {
            write_control_state_atomic(&self.path, self.serialized().as_bytes())
        }

        fn serialized(&self) -> String {
            format!(
                "schema={CONTROL_STATE_SCHEMA}\npid={}\nsession={}\nkind={}\n\
                 producer_version={CONTINUOUS_PRODUCER_VERSION}\n\
                 capture_profile={CODEX_CAPTURE_PROFILE_V2}\nstarted_unix_ms={}\nphase={}\n\
                 target={}\nsaved_segments={}\nsaved_events={}\nlast_flush_unix_ms={}\n",
                std::process::id(),
                self.session_id,
                self.session_kind.as_str(),
                self.started_unix_ms,
                self.phase.as_str(),
                self.target.as_str(),
                self.saved_segments,
                self.saved_events,
                self.last_flush_unix_ms
                    .map_or_else(|| "-".to_owned(), |value| value.to_string())
            )
        }

        fn set_target(&mut self, target: ControlTarget) -> Result<(), Box<dyn std::error::Error>> {
            if self.target != target {
                self.target = target;
                self.publish()?;
            }
            Ok(())
        }

        fn set_paused(&mut self, paused: bool) -> Result<(), Box<dyn std::error::Error>> {
            let phase = if paused {
                ControlPhase::Paused
            } else {
                ControlPhase::Running
            };
            if self.phase != phase {
                self.phase = phase;
                self.publish()?;
            }
            Ok(())
        }

        fn record_receipt(
            &mut self,
            receipt: &SegmentWriteReceipt,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.saved_segments = self.saved_segments.max(receipt.sequence.saturating_add(1));
            self.saved_events = self.saved_events.saturating_add(receipt.events as u64);
            self.last_flush_unix_ms = Some(current_unix_ms()?);
            self.publish()
        }

        fn finish(&mut self, phase: ControlPhase) -> Result<(), Box<dyn std::error::Error>> {
            self.phase = phase;
            self.target = ControlTarget::Waiting;
            self.publish()
        }
    }

    type SharedControlState = Option<Arc<Mutex<ControlStatePublisher>>>;

    struct ControlLifecycleGuard {
        control_state: SharedControlState,
        finished: bool,
    }

    impl ControlLifecycleGuard {
        fn new(control_state: SharedControlState) -> Self {
            Self {
                control_state,
                finished: false,
            }
        }

        fn finish(&mut self, phase: ControlPhase) -> Result<(), Box<dyn std::error::Error>> {
            finish_control_state(&self.control_state, phase)?;
            self.finished = true;
            Ok(())
        }
    }

    impl Drop for ControlLifecycleGuard {
        fn drop(&mut self) {
            if self.finished {
                return;
            }
            let Some(control_state) = &self.control_state else {
                return;
            };
            let mut publisher = match control_state.lock() {
                Ok(publisher) => publisher,
                Err(poisoned) => poisoned.into_inner(),
            };
            let _ = publisher.finish(ControlPhase::Failed);
        }
    }

    struct RecorderState {
        tracker: LocalInputTracker,
        pending_integrity: CaptureIntegrityCountersV1,
    }

    struct RecorderShared {
        pipeline_gate: Mutex<()>,
        state: Mutex<RecorderState>,
        writer: Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
        control_state: SharedControlState,
        paused: Arc<AtomicBool>,
        accepting_events: AtomicBool,
        composition_active: AtomicBool,
        fatal_error: Arc<Mutex<Option<String>>>,
    }

    impl RecorderShared {
        fn new(
            initial_value: String,
            writer: Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
            control_state: SharedControlState,
            paused: Arc<AtomicBool>,
            fatal_error: Arc<Mutex<Option<String>>>,
        ) -> Self {
            let mut tracker = LocalInputTracker::new(TARGET_NAME.to_owned(), initial_value);
            tracker.set_key_capture_enabled(true);
            Self {
                pipeline_gate: Mutex::new(()),
                state: Mutex::new(RecorderState {
                    tracker,
                    pending_integrity: CaptureIntegrityCountersV1::default(),
                }),
                writer,
                control_state,
                paused,
                accepting_events: AtomicBool::new(false),
                composition_active: AtomicBool::new(false),
                fatal_error,
            }
        }

        fn composition(&self, value: String) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            let result = self
                .state
                .lock()
                .map_err(|_| "recorder state lock was poisoned".to_owned())
                .map(|mut state| {
                    state.pending_integrity.observe_composition_callback();
                    let starts_new_composition =
                        !value.is_empty() && !state.tracker.has_active_composition();
                    if starts_new_composition && state.tracker.pending_keys_is_empty() {
                        state.tracker.mark_pending_keys_incomplete();
                    }
                    state.tracker.observe_composition(value);
                    self.composition_active
                        .store(state.tracker.has_active_composition(), Ordering::Release);
                });
            if let Err(error) = result {
                self.fail_closed(error);
            }
        }

        fn composition_read_error(&self) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            match self.state.lock() {
                Ok(mut state) => state.pending_integrity.observe_composition_read_error(),
                Err(_) => self.fail_closed("recorder state lock was poisoned".to_owned()),
            }
        }

        fn composition_finalized(&self) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            match self.state.lock() {
                Ok(mut state) => state
                    .pending_integrity
                    .observe_composition_finalized_callback(),
                Err(_) => self.fail_closed("recorder state lock was poisoned".to_owned()),
            }
        }

        fn key(&self, key: RawKey) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            match self.state.lock() {
                Ok(mut state) => {
                    let reset = state.tracker.observe_key_with_buffer_status(key);
                    state.pending_integrity.observe_key_action(reset);
                }
                Err(_) => self.fail_closed("recorder state lock was poisoned".to_owned()),
            }
        }

        fn value(
            &self,
            value: String,
            selection: Option<TextSelection>,
            selection_read_failed: bool,
        ) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            let observed = self
                .state
                .lock()
                .map_err(|_| "recorder state lock was poisoned".to_owned())
                .map(|mut state| {
                    let output = state.tracker.observe_value_with_selection(value, selection);
                    state
                        .pending_integrity
                        .observe_value_callback(output.is_some());
                    if selection_read_failed {
                        state.pending_integrity.observe_selection_read_error();
                    }
                    self.composition_active
                        .store(state.tracker.has_active_composition(), Ordering::Release);
                    let integrity = std::mem::take(&mut state.pending_integrity);
                    (integrity, output)
                });
            let (integrity, output) = match observed {
                Ok(observed) => observed,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            let result = self
                .writer
                .lock()
                .map_err(|_| "protected writer lock was poisoned".to_owned())
                .and_then(|mut writer| {
                    writer
                        .observe_batch(integrity, output)
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(Some(receipt)) => {
                    print_receipt(&receipt);
                    if let Err(error) = record_control_receipt(&self.control_state, &receipt) {
                        self.fail_closed(format!("control state publication failed: {error}"));
                    }
                }
                Ok(None) => {}
                Err(error) => self.fail_closed(error),
            }
        }

        fn value_read_error(&self) {
            let _pipeline = match self.lock_pipeline() {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    self.fail_closed(error);
                    return;
                }
            };
            if self.is_suspended() {
                return;
            }
            match self.state.lock() {
                Ok(mut state) => state.pending_integrity.observe_value_read_error(),
                Err(_) => self.fail_closed("recorder state lock was poisoned".to_owned()),
            }
        }

        fn rebaseline(&self, value: String) -> Result<(), String> {
            let _pipeline = self.lock_pipeline()?;
            self.rebaseline_under_pipeline(value, true)
        }

        fn activate(
            &self,
            read_baseline: impl FnOnce() -> Result<String, AttachAttemptError>,
            install_key_session: impl FnOnce() -> Result<(), AttachAttemptError>,
        ) -> Result<(), AttachAttemptError> {
            let _pipeline = self.lock_pipeline().map_err(AttachAttemptError::Fatal)?;
            let baseline = read_baseline()?;
            self.rebaseline_under_pipeline(baseline, true)
                .map_err(AttachAttemptError::Fatal)?;
            install_key_session()?;
            self.accepting_events.store(true, Ordering::Release);
            Ok(())
        }

        fn rebaseline_under_pipeline(
            &self,
            value: String,
            setup_gap_is_possible: bool,
        ) -> Result<(), String> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "recorder state lock was poisoned".to_owned())?;
            let mut tracker = LocalInputTracker::new(TARGET_NAME.to_owned(), value);
            tracker.set_key_capture_enabled(true);
            if setup_gap_is_possible {
                tracker.mark_pending_keys_incomplete();
            }
            state.tracker = tracker;
            state.pending_integrity = CaptureIntegrityCountersV1::default();
            self.composition_active.store(false, Ordering::Release);
            Ok(())
        }

        fn flush_integrity(&self) -> Result<(), String> {
            let _pipeline = self.lock_pipeline()?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "recorder state lock was poisoned".to_owned())?;
            if state.pending_integrity == CaptureIntegrityCountersV1::default() {
                return Ok(());
            }
            self.writer
                .lock()
                .map_err(|_| "protected writer lock was poisoned".to_owned())?
                .absorb_integrity(state.pending_integrity.clone())
                .map_err(|error| error.to_string())?;
            state.pending_integrity = CaptureIntegrityCountersV1::default();
            Ok(())
        }

        fn pause(&self) -> Result<(), String> {
            let _pipeline = self.lock_pipeline()?;
            self.pause_under_pipeline()
        }

        fn pause_under_pipeline(&self) -> Result<(), String> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "recorder state lock was poisoned".to_owned())?;
            let pending = state.tracker.pending_key_count();
            state.pending_integrity.observe_boundary_discard(pending);
            state.tracker.cancel_composition();
            self.composition_active.store(false, Ordering::Release);
            if state.pending_integrity == CaptureIntegrityCountersV1::default() {
                return Ok(());
            }
            self.writer
                .lock()
                .map_err(|_| "protected writer lock was poisoned".to_owned())?
                .absorb_integrity(state.pending_integrity.clone())
                .map_err(|error| error.to_string())?;
            state.pending_integrity = CaptureIntegrityCountersV1::default();
            Ok(())
        }

        fn disconnect(&self) -> Result<(), String> {
            self.accepting_events.store(false, Ordering::Release);
            let _pipeline = self.lock_pipeline()?;
            self.pause_under_pipeline()
        }

        fn is_suspended(&self) -> bool {
            self.paused.load(Ordering::Acquire) || !self.accepting_events.load(Ordering::Acquire)
        }

        fn lock_pipeline(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
            self.pipeline_gate
                .lock()
                .map_err(|_| "recorder pipeline lock was poisoned".to_owned())
        }

        fn fail_closed(&self, error: String) {
            self.paused.store(true, Ordering::Release);
            if let Ok(mut fatal) = self.fatal_error.lock()
                && fatal.is_none()
            {
                *fatal = Some(error);
            }
        }
    }

    #[implement(IUIAutomationEventHandler)]
    struct ValueChangedHandler {
        shared: Arc<RecorderShared>,
    }

    #[allow(non_snake_case)]
    impl IUIAutomationEventHandler_Impl for ValueChangedHandler_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: Ref<IUIAutomationElement>,
            _eventid: windows::Win32::UI::Accessibility::UIA_EVENT_ID,
        ) -> WindowsResult<()> {
            let sender = match sender.ok() {
                Ok(sender) => sender,
                Err(_) => {
                    self.shared.value_read_error();
                    return Ok(());
                }
            };
            let value = match read_value(sender) {
                Ok(value) => value,
                Err(_) => {
                    self.shared.value_read_error();
                    return Ok(());
                }
            };
            let (selection, selection_read_failed) = match read_selection(sender, &value) {
                Ok(selection) => (selection, false),
                Err(_) => (None, true),
            };
            self.shared.value(value, selection, selection_read_failed);
            Ok(())
        }
    }

    #[implement(IUIAutomationTextEditTextChangedEventHandler)]
    struct CompositionHandler {
        shared: Arc<RecorderShared>,
    }

    #[allow(non_snake_case)]
    impl IUIAutomationTextEditTextChangedEventHandler_Impl for CompositionHandler_Impl {
        fn HandleTextEditTextChangedEvent(
            &self,
            _sender: Ref<IUIAutomationElement>,
            change_type: TextEditChangeType,
            event_strings: *const SAFEARRAY,
        ) -> WindowsResult<()> {
            if change_type == TextEditChangeType_Composition {
                match read_event_strings(event_strings) {
                    Ok(strings) => self.shared.composition(strings.join("|")),
                    Err(_) => self.shared.composition_read_error(),
                }
            } else if change_type == TextEditChangeType_CompositionFinalized {
                self.shared.composition_finalized();
                // The ordinary Text_TextChanged event supplies the bounded
                // before/after delta; keep the last composition until then.
            }
            Ok(())
        }
    }

    #[implement(IUIAutomationFocusChangedEventHandler)]
    struct FocusChangedHandler {
        automation: IUIAutomation,
        target: IUIAutomationElement,
        target_active: Arc<AtomicBool>,
    }

    #[allow(non_snake_case)]
    impl IUIAutomationFocusChangedEventHandler_Impl for FocusChangedHandler_Impl {
        fn HandleFocusChangedEvent(&self, sender: Ref<IUIAutomationElement>) -> WindowsResult<()> {
            let active = sender
                .as_ref()
                .and_then(|sender| {
                    // SAFETY: Both elements are live COM interfaces retained
                    // by this handler. CompareElements checks runtime identity,
                    // not merely a shared name or process.
                    unsafe { self.automation.CompareElements(sender, &self.target) }.ok()
                })
                .is_some_and(|same| same.as_bool());
            self.target_active.store(active, Ordering::Release);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct KeySession {
        process_id: u32,
        target_active: Arc<AtomicBool>,
        shared: Arc<RecorderShared>,
    }

    struct KeyHookContext {
        session: Mutex<Option<KeySession>>,
        paused: Arc<AtomicBool>,
    }

    impl KeyHookContext {
        fn set_session(&self, session: KeySession) -> Result<(), String> {
            let mut slot = self
                .session
                .lock()
                .map_err(|_| "keyboard hook session lock was poisoned".to_owned())?;
            *slot = Some(session);
            Ok(())
        }

        fn clear_session(&self) {
            if let Ok(mut slot) = self.session.lock() {
                *slot = None;
            }
        }

        fn current_session(&self) -> Option<KeySession> {
            self.session.lock().ok()?.clone()
        }
    }

    static KEY_HOOK_CONTEXT: OnceLock<KeyHookContext> = OnceLock::new();

    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32
            && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN)
            && let Some(context) = KEY_HOOK_CONTEXT.get()
            && !context.paused.load(Ordering::Acquire)
            && let Some(session) = context.current_session()
            && key_capture_allowed(
                session.target_active.load(Ordering::Acquire),
                session.shared.composition_active.load(Ordering::Acquire),
                foreground_process_id() == session.process_id,
            )
        {
            // SAFETY: Windows supplies a valid KBDLLHOOKSTRUCT pointer while
            // code == HC_ACTION.
            let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if !data
                .flags
                .intersects(LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED)
                && !modifier_is_down()
                && let Some(key) = map_key(data.vkCode, shift_is_down())
            {
                session.shared.key(key);
            }
        }
        // SAFETY: Every low-level hook callback must be forwarded.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    struct Attachment {
        edit: IUIAutomationElement,
        process_id: i32,
        value_handler: IUIAutomationEventHandler,
        composition_handler: IUIAutomationTextEditTextChangedEventHandler,
        focus_handler: IUIAutomationFocusChangedEventHandler,
        target_active: Arc<AtomicBool>,
        shared: Arc<RecorderShared>,
    }

    impl Attachment {
        fn is_live(&self) -> bool {
            // SAFETY: Read-only property checks; a disconnected UIA provider
            // simply returns an error and causes a safe detach.
            unsafe {
                self.edit.CurrentProcessId().ok() == Some(self.process_id)
                    && self
                        .edit
                        .CurrentIsEnabled()
                        .ok()
                        .is_some_and(|value| value.as_bool())
                    && self
                        .edit
                        .CurrentIsPassword()
                        .ok()
                        .is_some_and(|value| !value.as_bool())
            }
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let Some(options) = parse_options()? else {
            return Ok(());
        };

        if options.metadata_only {
            println!(
                "CODEX_RECORDER_METADATA producer_version={} capture_profile={} \
                 control_state_schema={} target=codex-only \
                 protection=windows-dpapi-current-user network=false",
                CONTINUOUS_PRODUCER_VERSION, CODEX_CAPTURE_PROFILE_V2, CONTROL_STATE_SCHEMA
            );
            return Ok(());
        }

        // SAFETY: COM is initialized once on this thread and balanced by guard.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        let _com = ComGuard;
        // SAFETY: CUIAutomation8 is an in-process Windows COM class.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
        let automation3: IUIAutomation3 = automation.cast()?;

        if options.check_only {
            let (candidates, audit) = find_codex_targets_with_audit(&automation)?;
            println!(
                "CODEX_RECORDER_CHECK candidates={} named_edits={} safe_patterns={} \
                 chrome_framework={} prose_mirror_class={} codex_document_ancestor={} \
                 exact_policy=true disk_writes=false listeners=false producer_version={} \
                 capture_profile={} control_state_schema={}",
                candidates.len(),
                audit.named_edits,
                audit.safe_patterns,
                audit.chrome_framework,
                audit.prose_mirror_class,
                audit.codex_document_ancestor,
                CONTINUOUS_PRODUCER_VERSION,
                CODEX_CAPTURE_PROFILE_V2,
                CONTROL_STATE_SCHEMA
            );
            return Ok(());
        }

        let control_state_requested = options.control_state;
        let capture_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let root = prepare_continuous_root()?;
            let session_id = new_session_id()?;
            let writer_config = ProtectedSegmentWriterConfig::new(
                root,
                session_id.clone(),
                options.session_kind,
                CONTINUOUS_PRODUCER_VERSION.to_owned(),
                CODEX_CAPTURE_PROFILE_V2.to_owned(),
                options.segment_events,
                Duration::from_secs(options.flush_seconds),
            )?;
            let writer = Arc::new(Mutex::new(ProtectedSegmentWriter::new(
                writer_config,
                WindowsUserDataProtector,
            )?));
            let paused = Arc::new(AtomicBool::new(false));
            let fatal_error = Arc::new(Mutex::new(None));
            let control_state: SharedControlState = if options.control_state {
                Some(Arc::new(Mutex::new(ControlStatePublisher::new(
                    session_id.clone(),
                    options.session_kind,
                )?)))
            } else {
                None
            };
            let mut control_lifecycle = ControlLifecycleGuard::new(control_state.clone());
            KEY_HOOK_CONTEXT
                .set(KeyHookContext {
                    session: Mutex::new(None),
                    paused: Arc::clone(&paused),
                })
                .map_err(|_| "keyboard hook context was already initialized")?;

            // SAFETY: callback has static ABI and is alive through the message loop.
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)? };
            // SAFETY: Registrations are process-owned and cleaned up below.
            unsafe {
                RegisterHotKey(
                    None,
                    PAUSE_HOTKEY_ID,
                    MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                    VK_F10.0 as u32,
                )?;
                RegisterHotKey(
                    None,
                    STOP_HOTKEY_ID,
                    MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                    VK_F12.0 as u32,
                )?;
            }
            publish_control_state(&control_state)?;

            println!(
                "CODEX_RECORDER_RUNNING session={} kind={} protection=windows-dpapi-current-user \
             target=codex-only segment_events={} flush_seconds={} preview_text=false \
             producer_version={} capture_profile={} control_state={} network=false \
             startup_installed=false contains_text=false contains_behavioral_metadata=true",
                session_id,
                options.session_kind.as_str(),
                options.segment_events,
                options.flush_seconds,
                CONTINUOUS_PRODUCER_VERSION,
                CODEX_CAPTURE_PROFILE_V2,
                options.control_state
            );
            println!("Ctrl+Shift+F10 pauses or resumes. Ctrl+Shift+F12 stops and flushes.");

            let mut result = recorder_loop(
                &automation,
                &automation3,
                &writer,
                &control_state,
                &paused,
                &fatal_error,
            );

            if let Some(context) = KEY_HOOK_CONTEXT.get() {
                context.clear_session();
            }
            // SAFETY: Each call matches a successful registration above.
            unsafe {
                let _ = UnregisterHotKey(None, PAUSE_HOTKEY_ID);
                let _ = UnregisterHotKey(None, STOP_HOTKEY_ID);
                let _ = UnhookWindowsHookEx(hook);
            }
            let final_close_reason = SegmentCloseReason::SessionEnd;
            match flush_writer(&writer, final_close_reason) {
                Ok(Some(receipt)) => {
                    print_receipt(&receipt);
                    if let Err(error) = record_control_receipt(&control_state, &receipt) {
                        keep_first_error(&mut result, error);
                    }
                }
                Ok(None) => {}
                Err(error) => keep_first_error(&mut result, error),
            }
            let written_counts = writer
                .lock()
                .map(|writer| (writer.written_segments(), writer.written_events()))
                .map_err(|_| -> Box<dyn std::error::Error> {
                    "protected writer lock was poisoned".into()
                });
            match written_counts {
                Ok((written_segments, written_events)) => {
                    println!(
                        "CODEX_RECORDER_FEEDBACK session={} segments={} events={} \
                     producer_version={} capture_profile={} contains_text=false \
                     contains_behavioral_metadata=true replay_selector=--session",
                        session_id,
                        written_segments,
                        written_events,
                        CONTINUOUS_PRODUCER_VERSION,
                        CODEX_CAPTURE_PROFILE_V2
                    );
                    if written_segments > 0 {
                        println!(
                            "FEEDBACK_COMMAND cargo run --release --bin capsule-replay -- \
                         --session {} --window-gap-ms 15000 --compact",
                            session_id
                        );
                    }
                }
                Err(error) => keep_first_error(&mut result, error),
            }
            let phase = if result.is_ok() {
                ControlPhase::Stopped
            } else {
                ControlPhase::Failed
            };
            if let Err(error) = control_lifecycle.finish(phase) {
                keep_first_error(&mut result, error);
            }
            result?;
            println!(
                "CODEX_RECORDER_STOPPED flushed=true contains_text=false \
                 contains_behavioral_metadata=true"
            );
            Ok(())
        })();
        if capture_result.is_err() {
            eprintln!(
                "{}",
                recorder_failure_terminal_line(control_state_requested)
            );
            return Err("continuous recorder failed; private details were suppressed".into());
        }
        Ok(())
    }

    fn keep_first_error(
        result: &mut Result<(), Box<dyn std::error::Error>>,
        error: Box<dyn std::error::Error>,
    ) {
        if result.is_ok() {
            *result = Err(error);
        }
    }

    fn recorder_loop(
        automation: &IUIAutomation,
        automation3: &IUIAutomation3,
        writer: &Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
        control_state: &SharedControlState,
        paused: &Arc<AtomicBool>,
        fatal_error: &Arc<Mutex<Option<String>>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut attachment: Option<Attachment> = None;
        let mut last_discovery = Instant::now() - TARGET_POLL_INTERVAL;
        let mut last_health = Instant::now();
        let mut last_status = None;
        let mut disconnected_at = None;

        let loop_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            loop {
                let mut stop = false;
                let mut message = MSG::default();
                // SAFETY: message is writable; PM_REMOVE drains this thread's queue.
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    if message.message == WM_QUIT {
                        stop = true;
                        break;
                    }
                    if message.message == WM_HOTKEY {
                        if message.wParam.0 == STOP_HOTKEY_ID as usize {
                            stop = true;
                            break;
                        }
                        if message.wParam.0 == PAUSE_HOTKEY_ID as usize {
                            toggle_pause(paused, attachment.as_ref(), writer, control_state)?;
                        }
                    }
                    // SAFETY: Standard dispatch for messages not consumed above.
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                if stop {
                    break;
                }

                if let Some(error) = fatal_error.lock().ok().and_then(|value| value.clone()) {
                    return Err(format!(
                        "recording paused after a fail-closed storage error: {error}"
                    )
                    .into());
                }

                if attachment.is_some()
                    && last_health.elapsed() >= TARGET_HEALTH_INTERVAL
                    && !attachment
                        .as_ref()
                        .is_some_and(|current| attachment_still_current(automation, current))
                {
                    let current = attachment.take().expect("checked as present");
                    detach(automation, automation3, current)?;
                    if let Some(receipt) = flush_writer(writer, SegmentCloseReason::Continuity)? {
                        print_receipt(&receipt);
                        record_control_receipt(control_state, &receipt)?;
                    }
                    set_control_target(control_state, ControlTarget::Waiting)?;
                    disconnected_at = Some(Instant::now());
                    last_status = None;
                    last_discovery = Instant::now() - TARGET_POLL_INTERVAL;
                }
                if last_health.elapsed() >= TARGET_HEALTH_INTERVAL {
                    last_health = Instant::now();
                    if !paused.load(Ordering::Acquire)
                        && let Some(current) = attachment.as_ref()
                    {
                        current
                            .shared
                            .flush_integrity()
                            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
                    }
                    if let Some(receipt) = flush_writer_if_due(writer)? {
                        print_receipt(&receipt);
                        record_control_receipt(control_state, &receipt)?;
                    }
                }

                if should_attempt_target_discovery(
                    paused.load(Ordering::Acquire),
                    attachment.is_some(),
                ) && last_discovery.elapsed() >= TARGET_POLL_INTERVAL
                {
                    match choose_codex_target(automation)? {
                        Ok(edit) => {
                            let attach_result = attach(
                                automation,
                                automation3,
                                edit,
                                Arc::clone(writer),
                                control_state.clone(),
                                Arc::clone(paused),
                                Arc::clone(fatal_error),
                            );
                            let current = match attach_result {
                                Ok(current) => current,
                                Err(AttachAttemptError::Retryable(reason)) => {
                                    if let Some(receipt) =
                                        flush_writer(writer, SegmentCloseReason::Continuity)?
                                    {
                                        print_receipt(&receipt);
                                        record_control_receipt(control_state, &receipt)?;
                                    }
                                    set_control_target(control_state, ControlTarget::Waiting)?;
                                    if last_status != Some(DiscoveryStatus::Waiting) {
                                        println!(
                                            "CODEX_TARGET_RETRYING reason={reason} \
                                             continuity_boundary=flushed_if_nonempty \
                                             contains_behavioral_metadata=true"
                                        );
                                    }
                                    last_status = Some(DiscoveryStatus::Waiting);
                                    last_discovery = Instant::now();
                                    continue;
                                }
                                Err(AttachAttemptError::Fatal(error)) => {
                                    return Err(error.into());
                                }
                            };
                            if disconnected_at.take().is_some() {
                                println!(
                                    "CODEX_TARGET_REBOUND pid={} continuity_boundary=flushed_if_nonempty \
                                     key_pairing=automatic existing_text_saved=false \
                                     contains_behavioral_metadata=true",
                                    current.process_id
                                );
                            } else {
                                println!(
                                    "CODEX_TARGET_CONNECTED pid={} key_pairing=automatic \
                                     existing_text_saved=false contains_behavioral_metadata=true",
                                    current.process_id
                                );
                            }
                            attachment = Some(current);
                            set_control_target(control_state, ControlTarget::Connected)?;
                            last_status = None;
                        }
                        Err(status)
                            if last_status != Some(status)
                                && (status != DiscoveryStatus::Waiting
                                    || disconnected_at.is_none_or(|when| {
                                        when.elapsed() >= RECONNECT_NOTICE_DELAY
                                    })) =>
                        {
                            match status {
                                DiscoveryStatus::Waiting => {
                                    println!(
                                        "CODEX_TARGET_WAITING continuity_boundary=flushed_if_nonempty \
                                         app_restart_reconnect=true contains_behavioral_metadata=true"
                                    )
                                }
                                DiscoveryStatus::Ambiguous(count) => eprintln!(
                                    "CODEX_TARGET_REFUSED candidates={count} \
                                     reason=no_unique_focused_exact_codex_edit \
                                     contains_behavioral_metadata=true"
                                ),
                            }
                            last_status = Some(status);
                        }
                        Err(_) => {}
                    }
                    last_discovery = Instant::now();
                }

                wait_for_message_or_poll();
            }
            Ok(())
        })();
        let cleanup_result = attachment
            .take()
            .map_or(Ok(()), |current| detach(automation, automation3, current));
        match (loop_result, cleanup_result) {
            (Err(primary), Err(cleanup)) => {
                Err(format!("{primary}; recorder attachment cleanup also failed: {cleanup}").into())
            }
            (Err(primary), _) => Err(primary),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    fn wait_for_message_or_poll() {
        // SAFETY: No kernel handles are supplied. The call waits only for this
        // thread's message queue (including registered hotkeys) or the bounded
        // poll timeout, so hotkeys wake immediately without a 40 Hz busy loop.
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(
                None,
                MESSAGE_WAIT_TIMEOUT_MS,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if result == WAIT_FAILED {
            std::thread::sleep(Duration::from_millis(u64::from(MESSAGE_WAIT_TIMEOUT_MS)));
        }
    }

    fn should_attempt_target_discovery(paused: bool, attachment_present: bool) -> bool {
        !paused && !attachment_present
    }

    fn attachment_still_current(automation: &IUIAutomation, attachment: &Attachment) -> bool {
        if !attachment.is_live() {
            return false;
        }
        // A connected target does not need a full desktop descendant scan on
        // every health tick. If focus is elsewhere (another app, a candidate
        // list, or a message), the retained live target remains valid. When a
        // new exact Codex edit receives focus after a DOM/task rebuild, compare
        // its runtime identity with the retained element and rebind if needed.
        let Ok(focused) = (unsafe { automation.GetFocusedElement() }) else {
            return true;
        };
        if !exact_target_policy_matches(automation, &focused) {
            return true;
        }
        // SAFETY: Both interfaces are live for this call. Runtime identity
        // detects an in-process UIA element rebuild even if PID and name stay
        // unchanged.
        unsafe {
            automation
                .CompareElements(&attachment.edit, &focused)
                .map(|same| same.as_bool())
                .unwrap_or(false)
        }
    }

    fn exact_target_policy_matches(
        automation: &IUIAutomation,
        element: &IUIAutomationElement,
    ) -> bool {
        if validate_target(element).is_err() {
            return false;
        }
        // SAFETY: Read-only framework property on a validated candidate.
        let framework_matches = unsafe {
            element
                .CurrentFrameworkId()
                .map(|value| String::from_utf16_lossy(&value) == TARGET_FRAMEWORK)
                .unwrap_or(false)
        };
        framework_matches && has_codex_document_ancestor(automation, element)
    }

    fn attach(
        automation: &IUIAutomation,
        automation3: &IUIAutomation3,
        edit: IUIAutomationElement,
        writer: Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
        control_state: SharedControlState,
        paused: Arc<AtomicBool>,
        fatal_error: Arc<Mutex<Option<String>>>,
    ) -> Result<Attachment, AttachAttemptError> {
        validate_target(&edit)
            .map_err(|_| AttachAttemptError::Retryable("target_changed_during_attach"))?;
        let initial_value = read_value(&edit)
            .map_err(|_| AttachAttemptError::Retryable("target_changed_during_attach"))?;
        // SAFETY: Read-only property on the already validated target.
        let process_id = unsafe { edit.CurrentProcessId() }
            .map_err(|_| AttachAttemptError::Retryable("target_changed_during_attach"))?;
        writer
            .lock()
            .map_err(|_| {
                AttachAttemptError::Fatal("protected writer lock was poisoned".to_owned())
            })?
            .start_new_baseline_epoch()
            .map_err(|error| AttachAttemptError::Fatal(error.to_string()))?;
        let shared = Arc::new(RecorderShared::new(
            initial_value,
            writer,
            control_state,
            paused,
            fatal_error,
        ));
        let value_handler: IUIAutomationEventHandler = ValueChangedHandler {
            shared: Arc::clone(&shared),
        }
        .into();
        let composition_handler: IUIAutomationTextEditTextChangedEventHandler =
            CompositionHandler {
                shared: Arc::clone(&shared),
            }
            .into();
        let target_active = Arc::new(AtomicBool::new(
            // SAFETY: Read-only property on the live exact target.
            unsafe { edit.CurrentHasKeyboardFocus() }
                .map(|value| value.as_bool())
                .unwrap_or(false),
        ));
        let focus_handler: IUIAutomationFocusChangedEventHandler = FocusChangedHandler {
            automation: automation.clone(),
            target: edit.clone(),
            target_active: Arc::clone(&target_active),
        }
        .into();

        let mut value_registered = false;
        let mut composition_registered = false;
        let mut focus_registered = false;
        let registration_result = (|| -> WindowsResult<()> {
            // SAFETY: Handlers and target stay alive until either rollback or
            // the returned Attachment is detached.
            unsafe {
                automation.AddAutomationEventHandler(
                    UIA_Text_TextChangedEventId,
                    &edit,
                    TreeScope_Element,
                    None::<&IUIAutomationCacheRequest>,
                    &value_handler,
                )?;
                value_registered = true;
                automation3.AddTextEditTextChangedEventHandler(
                    &edit,
                    TreeScope_Element,
                    TextEditChangeType_Composition,
                    None::<&IUIAutomationCacheRequest>,
                    &composition_handler,
                )?;
                composition_registered = true;
                automation3.AddTextEditTextChangedEventHandler(
                    &edit,
                    TreeScope_Element,
                    TextEditChangeType_CompositionFinalized,
                    None::<&IUIAutomationCacheRequest>,
                    &composition_handler,
                )?;
                automation.AddFocusChangedEventHandler(
                    None::<&IUIAutomationCacheRequest>,
                    &focus_handler,
                )?;
                focus_registered = true;
            }
            Ok(())
        })();
        if let Err(error) = registration_result {
            let cleanup_result = shared.disconnect();
            remove_registered_handlers(
                automation,
                automation3,
                &edit,
                &value_handler,
                &composition_handler,
                &focus_handler,
                value_registered,
                composition_registered,
                focus_registered,
            );
            if let Err(cleanup) = cleanup_result {
                return Err(AttachAttemptError::Fatal(format!(
                    "UIA handler registration failed: {error}; recorder pipeline cleanup failed: {cleanup}"
                )));
            }
            if retryable_uia_error_code(error.code().0 as u32) {
                return Err(AttachAttemptError::Retryable(
                    "target_changed_during_handler_registration",
                ));
            }
            return Err(AttachAttemptError::Fatal(format!(
                "UIA handler registration failed with HRESULT {:?}",
                error.code()
            )));
        }
        let Some(key_context) = KEY_HOOK_CONTEXT.get() else {
            let cleanup_result = shared.disconnect();
            remove_registered_handlers(
                automation,
                automation3,
                &edit,
                &value_handler,
                &composition_handler,
                &focus_handler,
                true,
                true,
                true,
            );
            cleanup_result.map_err(AttachAttemptError::Fatal)?;
            return Err(AttachAttemptError::Fatal(
                "keyboard hook context is unavailable".to_owned(),
            ));
        };
        let key_session = KeySession {
            process_id: process_id as u32,
            target_active: Arc::clone(&target_active),
            shared: Arc::clone(&shared),
        };
        let activation_result = shared.activate(
            || {
                read_value(&edit)
                    .map_err(|_| AttachAttemptError::Retryable("target_changed_during_activation"))
            },
            || {
                key_context
                    .set_session(key_session)
                    .map_err(AttachAttemptError::Fatal)
            },
        );
        if let Err(error) = activation_result {
            key_context.clear_session();
            target_active.store(false, Ordering::Release);
            let cleanup_result = shared.disconnect();
            remove_registered_handlers(
                automation,
                automation3,
                &edit,
                &value_handler,
                &composition_handler,
                &focus_handler,
                true,
                true,
                true,
            );
            if let Err(cleanup) = cleanup_result {
                return Err(AttachAttemptError::Fatal(format!(
                    "recorder activation failed: {error}; recorder pipeline cleanup failed: {cleanup}"
                )));
            }
            return Err(error);
        }
        Ok(Attachment {
            edit,
            process_id,
            value_handler,
            composition_handler,
            focus_handler,
            target_active,
            shared,
        })
    }

    fn detach(
        automation: &IUIAutomation,
        automation3: &IUIAutomation3,
        attachment: Attachment,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(context) = KEY_HOOK_CONTEXT.get() {
            context.clear_session();
        }
        attachment.target_active.store(false, Ordering::Release);
        let disconnect_result = attachment.shared.disconnect();
        remove_registered_handlers(
            automation,
            automation3,
            &attachment.edit,
            &attachment.value_handler,
            &attachment.composition_handler,
            &attachment.focus_handler,
            true,
            true,
            true,
        );
        disconnect_result.map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    fn remove_registered_handlers(
        automation: &IUIAutomation,
        automation3: &IUIAutomation3,
        edit: &IUIAutomationElement,
        value_handler: &IUIAutomationEventHandler,
        composition_handler: &IUIAutomationTextEditTextChangedEventHandler,
        focus_handler: &IUIAutomationFocusChangedEventHandler,
        value_registered: bool,
        composition_registered: bool,
        focus_registered: bool,
    ) {
        // SAFETY: Each attempted removal corresponds to a completed
        // registration. Provider-exit errors are expected during teardown.
        unsafe {
            if focus_registered {
                let _ = automation.RemoveFocusChangedEventHandler(focus_handler);
            }
            if value_registered {
                let _ = automation.RemoveAutomationEventHandler(
                    UIA_Text_TextChangedEventId,
                    edit,
                    value_handler,
                );
            }
            if composition_registered {
                let _ =
                    automation3.RemoveTextEditTextChangedEventHandler(edit, composition_handler);
            }
        }
    }

    fn retryable_uia_error_code(code: u32) -> bool {
        matches!(code, UIA_E_ELEMENTNOTAVAILABLE | UIA_E_TIMEOUT)
    }

    fn toggle_pause(
        paused: &AtomicBool,
        attachment: Option<&Attachment>,
        writer: &Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
        control_state: &SharedControlState,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !paused.swap(true, Ordering::AcqRel) {
            if let Some(attachment) = attachment {
                attachment
                    .shared
                    .pause()
                    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
            }
            if let Some(receipt) = flush_writer(writer, SegmentCloseReason::Continuity)? {
                print_receipt(&receipt);
                record_control_receipt(control_state, &receipt)?;
            }
            set_control_paused(control_state, true)?;
            println!("CODEX_RECORDER_PAUSED disk_flush=true contains_behavioral_metadata=true");
            return Ok(());
        }

        if let Some(attachment) = attachment {
            if !attachment.is_live() {
                println!(
                    "CODEX_RECORDER_RESUME_WAITING target_connected=false \
                     contains_behavioral_metadata=true"
                );
                paused.store(false, Ordering::Release);
                set_control_paused(control_state, false)?;
                set_control_target(control_state, ControlTarget::Waiting)?;
                return Ok(());
            }
            let baseline = read_value(&attachment.edit)?;
            writer
                .lock()
                .map_err(|_| "protected writer lock was poisoned")?
                .start_new_baseline_epoch()?;
            attachment
                .shared
                .rebaseline(baseline)
                .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        }
        paused.store(false, Ordering::Release);
        set_control_paused(control_state, false)?;
        println!(
            "CODEX_RECORDER_RESUMED target_connected={} paused_text_saved=false \
             contains_behavioral_metadata=true",
            attachment.is_some()
        );
        Ok(())
    }

    fn flush_writer(
        writer: &Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
        reason: SegmentCloseReason,
    ) -> Result<Option<SegmentWriteReceipt>, Box<dyn std::error::Error>> {
        Ok(writer
            .lock()
            .map_err(|_| "protected writer lock was poisoned")?
            .flush_with_reason(reason)?)
    }

    fn flush_writer_if_due(
        writer: &Arc<Mutex<ProtectedSegmentWriter<WindowsUserDataProtector>>>,
    ) -> Result<Option<SegmentWriteReceipt>, Box<dyn std::error::Error>> {
        Ok(writer
            .lock()
            .map_err(|_| "protected writer lock was poisoned")?
            .flush_if_due()?)
    }

    fn publish_control_state(
        control_state: &SharedControlState,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control_state) = control_state {
            control_state
                .lock()
                .map_err(|_| "control state lock was poisoned")?
                .publish()?;
        }
        Ok(())
    }

    fn set_control_target(
        control_state: &SharedControlState,
        target: ControlTarget,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control_state) = control_state {
            control_state
                .lock()
                .map_err(|_| "control state lock was poisoned")?
                .set_target(target)?;
        }
        Ok(())
    }

    fn set_control_paused(
        control_state: &SharedControlState,
        paused: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control_state) = control_state {
            control_state
                .lock()
                .map_err(|_| "control state lock was poisoned")?
                .set_paused(paused)?;
        }
        Ok(())
    }

    fn record_control_receipt(
        control_state: &SharedControlState,
        receipt: &SegmentWriteReceipt,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control_state) = control_state {
            control_state
                .lock()
                .map_err(|_| "control state lock was poisoned")?
                .record_receipt(receipt)?;
        }
        Ok(())
    }

    fn finish_control_state(
        control_state: &SharedControlState,
        phase: ControlPhase,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control_state) = control_state {
            control_state
                .lock()
                .map_err(|_| "control state lock was poisoned")?
                .finish(phase)?;
        }
        Ok(())
    }

    fn print_receipt(receipt: &SegmentWriteReceipt) {
        println!("{}", receipt_terminal_line(receipt));
    }

    fn receipt_terminal_line(receipt: &SegmentWriteReceipt) -> String {
        format!(
            "PROTECTED_SEGMENT_SAVED sequence={} events={} bytes={} protection={} \
             contains_plaintext=false path_disclosed=false \
             contains_behavioral_metadata=true",
            receipt.sequence, receipt.events, receipt.protected_bytes, receipt.protection
        )
    }

    fn recorder_failure_terminal_line(control_state_requested: bool) -> String {
        format!(
            "CODEX_RECORDER_FAILED kind=owned-error contains_text=false \
             contains_behavioral_metadata=true control_state_requested={} \
             error_details_suppressed=true",
            control_state_requested
        )
    }

    fn parse_options() -> Result<Option<Options>, Box<dyn std::error::Error>> {
        parse_options_from(std::env::args().skip(1))
    }

    fn parse_options_from(
        arguments: impl IntoIterator<Item = String>,
    ) -> Result<Option<Options>, Box<dyn std::error::Error>> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        if arguments
            .iter()
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        {
            if arguments.len() == 1 {
                print_usage();
                return Ok(None);
            }
            return Err("--help must be used by itself".into());
        }
        let mut run = false;
        let mut check_only = false;
        let mut metadata_only = false;
        let mut control_state = false;
        let mut session_kind = CaptureSessionKind::Daily;
        let mut segment_events = DEFAULT_SEGMENT_EVENTS;
        let mut flush_seconds = DEFAULT_FLUSH_SECONDS;
        let mut args = arguments.into_iter();
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--run" => run = true,
                "--check" => check_only = true,
                "--metadata" => metadata_only = true,
                "--control-state" => control_state = true,
                "--session-kind" => {
                    session_kind = CaptureSessionKind::parse(
                        &args
                            .next()
                            .ok_or("--session-kind requires daily, course, or theme")?,
                    )?;
                }
                "--segment-events" => {
                    segment_events = args
                        .next()
                        .ok_or("--segment-events requires a value")?
                        .parse()?;
                }
                "--flush-seconds" => {
                    flush_seconds = args
                        .next()
                        .ok_or("--flush-seconds requires a value")?
                        .parse()?;
                }
                _ => return Err("unknown argument; value was suppressed".into()),
            }
        }
        if usize::from(run) + usize::from(check_only) + usize::from(metadata_only) != 1 {
            print_usage();
            return Err("choose exactly one of --check, --metadata, or --run".into());
        }
        if (check_only || metadata_only)
            && (session_kind != CaptureSessionKind::Daily
                || segment_events != DEFAULT_SEGMENT_EVENTS
                || flush_seconds != DEFAULT_FLUSH_SECONDS
                || control_state)
        {
            return Err(
                "--check and --metadata cannot be combined with recording configuration".into(),
            );
        }
        if !(1..=MAX_SEGMENT_EVENTS).contains(&segment_events) {
            return Err(
                format!("--segment-events must be between 1 and {MAX_SEGMENT_EVENTS}").into(),
            );
        }
        if !(MIN_FLUSH_SECONDS..=MAX_FLUSH_SECONDS).contains(&flush_seconds) {
            return Err(format!(
                "--flush-seconds must be between {MIN_FLUSH_SECONDS} and {MAX_FLUSH_SECONDS}"
            )
            .into());
        }
        Ok(Some(Options {
            check_only,
            metadata_only,
            control_state,
            session_kind,
            segment_events,
            flush_seconds,
        }))
    }

    fn print_usage() {
        eprintln!("usage: codex-recorder --metadata");
        eprintln!("       codex-recorder --check");
        eprintln!(
            "       codex-recorder --run [--control-state] \
             [--session-kind daily|course|theme] \
             [--segment-events <1..={MAX_SEGMENT_EVENTS}>] \
             [--flush-seconds <{MIN_FLUSH_SECONDS}..={MAX_FLUSH_SECONDS}>]"
        );
        eprintln!("--check installs no listeners and writes no files");
        eprintln!("--metadata reports static build metadata without initializing UI Automation");
        eprintln!(
            "--control-state publishes redacted low-frequency lifecycle status under .local/recorder"
        );
        eprintln!(
            "--run tracks only the unique safe Codex edit; ProseMirror class is audit-only; \
             protected segments stay under data/private/continuous-capture"
        );
        eprintln!("the recorder does not install itself at startup");
    }

    fn choose_codex_target(
        automation: &IUIAutomation,
    ) -> Result<Result<IUIAutomationElement, DiscoveryStatus>, Box<dyn std::error::Error>> {
        let candidates = find_codex_targets(automation)?;
        match candidates.len() {
            0 => Ok(Err(DiscoveryStatus::Waiting)),
            1 => Ok(Ok(candidates.into_iter().next().expect("one candidate"))),
            count => {
                let mut focused = candidates.into_iter().filter(|element| {
                    // SAFETY: Read-only focus query on a validated candidate.
                    unsafe { element.CurrentHasKeyboardFocus() }
                        .map(|value| value.as_bool())
                        .unwrap_or(false)
                });
                let first = focused.next();
                match (first, focused.next()) {
                    (Some(first), None) => Ok(Ok(first)),
                    _ => Ok(Err(DiscoveryStatus::Ambiguous(count))),
                }
            }
        }
    }

    fn find_codex_targets(
        automation: &IUIAutomation,
    ) -> Result<Vec<IUIAutomationElement>, Box<dyn std::error::Error>> {
        Ok(find_codex_targets_with_audit(automation)?.0)
    }

    fn find_codex_targets_with_audit(
        automation: &IUIAutomation,
    ) -> Result<(Vec<IUIAutomationElement>, TargetPolicyAudit), Box<dyn std::error::Error>> {
        let control = property_condition_i32(
            automation,
            UIA_ControlTypePropertyId,
            UIA_EditControlTypeId.0,
        )?;
        let name = property_condition_string(automation, UIA_NamePropertyId, TARGET_NAME)?;
        // SAFETY: Both live condition interfaces are valid parameters.
        let combined = unsafe { automation.CreateAndCondition(&control, &name)? };
        // SAFETY: Root and combined condition remain alive through FindAll.
        let matches = unsafe {
            automation
                .GetRootElement()?
                .FindAll(TreeScope_Descendants, &combined)?
        };
        // SAFETY: The returned array stays alive in this function.
        let count = unsafe { matches.Length()? };
        let mut candidates = Vec::new();
        let mut audit = TargetPolicyAudit {
            named_edits: usize::try_from(count).unwrap_or(usize::MAX),
            ..TargetPolicyAudit::default()
        };
        for index in 0..count {
            // SAFETY: index is bounded by Length.
            let element = unsafe { matches.GetElement(index)? };
            if validate_target(&element).is_err() {
                continue;
            }
            audit.safe_patterns += 1;
            // SAFETY: Read-only string property on a validated candidate.
            let framework_matches = unsafe {
                element
                    .CurrentFrameworkId()
                    .map(|value| String::from_utf16_lossy(&value) == TARGET_FRAMEWORK)
                    .unwrap_or(false)
            };
            if !framework_matches {
                continue;
            }
            audit.chrome_framework += 1;
            // SAFETY: Read-only string property on a validated candidate.
            let class_matches = unsafe {
                element
                    .CurrentClassName()
                    .map(|value| String::from_utf16_lossy(&value) == TARGET_CLASS)
                    .unwrap_or(false)
            };
            if class_matches {
                audit.prose_mirror_class += 1;
            }
            if has_codex_document_ancestor(automation, &element) {
                audit.codex_document_ancestor += 1;
                candidates.push(element);
            }
        }
        Ok((candidates, audit))
    }

    fn has_codex_document_ancestor(
        automation: &IUIAutomation,
        element: &IUIAutomationElement,
    ) -> bool {
        // SAFETY: The control-view walker is owned by automation. Parent
        // queries are bounded and retain no text.
        let Ok(walker) = (unsafe { automation.ControlViewWalker() }) else {
            return false;
        };
        let mut current = element.clone();
        for _ in 0..32 {
            let Ok(parent) = (unsafe { walker.GetParentElement(&current) }) else {
                break;
            };
            // SAFETY: Read-only control type and name queries.
            let is_codex_document = unsafe {
                parent.CurrentControlType().ok() == Some(UIA_DocumentControlTypeId)
                    && parent
                        .CurrentName()
                        .map(|value| String::from_utf16_lossy(&value) == TARGET_DOCUMENT_NAME)
                        .unwrap_or(false)
            };
            if is_codex_document {
                return true;
            }
            current = parent;
        }
        false
    }

    fn validate_target(edit: &IUIAutomationElement) -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: Read-only UIA properties and pattern queries.
        unsafe {
            if edit.CurrentProcessId()? <= 0
                || edit.CurrentControlType()? != UIA_EditControlTypeId
                || String::from_utf16_lossy(&edit.CurrentName()?) != TARGET_NAME
                || !edit.CurrentIsEnabled()?.as_bool()
                || !edit.CurrentIsKeyboardFocusable()?.as_bool()
                || edit.CurrentIsPassword()?.as_bool()
            {
                return Err("candidate does not satisfy exact safe target policy".into());
            }
            let _: IUIAutomationTextEditPattern =
                edit.GetCurrentPatternAs(UIA_TextEditPatternId)?;
            let _: IUIAutomationValuePattern = edit.GetCurrentPatternAs(UIA_ValuePatternId)?;
        }
        Ok(())
    }

    fn prepare_continuous_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.join("data/private/continuous-capture");
        let mut current = manifest.to_path_buf();
        for component in ["data", "private", "continuous-capture"] {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "continuous capture directory contains a symbolic-link component: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "continuous capture path component is not a directory: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        fs::create_dir_all(&root)?;
        let canonical_manifest = fs::canonicalize(manifest)?;
        let canonical_root = fs::canonicalize(&root)?;
        if !canonical_root.starts_with(canonical_manifest) {
            return Err("continuous capture directory resolves outside the repository".into());
        }
        Ok(canonical_root)
    }

    fn prepare_control_state_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut root = manifest.to_path_buf();
        for component in [".local", "recorder"] {
            root.push(component);
            match fs::symlink_metadata(&root) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "control state path contains a symbolic-link component: {}",
                        root.display()
                    )
                    .into());
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "control state path component is not a directory: {}",
                        root.display()
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&root)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let canonical_manifest = fs::canonicalize(manifest)?;
        let canonical_root = fs::canonicalize(&root)?;
        if !canonical_root.starts_with(canonical_manifest) {
            return Err("control state directory resolves outside the repository".into());
        }
        let path = canonical_root.join("active-v1.txt");
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err("control state target must be a regular non-symlink file".into());
        }
        Ok(path)
    }

    fn write_control_state_atomic(
        path: &Path,
        body: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        if body.len() > 4096 {
            return Err("control state exceeds its fixed size limit".into());
        }
        let parent = path.parent().ok_or("control state path has no parent")?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let temporary = parent.join(format!(".active-{}-{stamp}.tmp", std::process::id()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(body)?;
            file.sync_all()?;
            let source = wide_path(&temporary)?;
            let destination = wide_path(path)?;
            // SAFETY: Both NUL-terminated path buffers live through the call.
            unsafe {
                MoveFileExW(
                    PCWSTR(source.as_ptr()),
                    PCWSTR(destination.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )?
            };
            Ok::<(), Box<dyn std::error::Error>>(())
        })();
        if result.is_err() && temporary.exists() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn wide_path(path: &Path) -> Result<Vec<u16>, Box<dyn std::error::Error>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err("control state path contains an embedded NUL".into());
        }
        wide.push(0);
        Ok(wide)
    }

    fn current_unix_ms() -> Result<u64, Box<dyn std::error::Error>> {
        Ok(u64::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        )?)
    }

    fn new_session_id() -> Result<String, Box<dyn std::error::Error>> {
        Ok(format!("{}-{}", current_unix_ms()?, std::process::id()))
    }

    fn read_value(element: &IUIAutomationElement) -> WindowsResult<String> {
        // SAFETY: The caller supplies a live exact target element.
        let value: BSTR = unsafe {
            element
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)?
                .CurrentValue()?
        };
        Ok(String::from_utf16_lossy(&value))
    }

    fn read_selection(
        element: &IUIAutomationElement,
        value: &str,
    ) -> WindowsResult<Option<TextSelection>> {
        // SAFETY: Ranges belong to the live exact target and are retained only
        // long enough to derive character offsets.
        unsafe {
            let pattern: IUIAutomationTextPattern =
                element.GetCurrentPatternAs(UIA_TextPatternId)?;
            let selections = pattern.GetSelection()?;
            if selections.Length()? != 1 {
                return Ok(None);
            }
            let selection = selections.GetElement(0)?;
            let document = pattern.DocumentRange()?;
            let document_text = String::from_utf16_lossy(&document.GetText(-1)?);
            let Some(content_start) = unique_value_offset(&document_text, value) else {
                return Ok(None);
            };
            let start_prefix = document.Clone()?;
            start_prefix.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )?;
            let start_in_document = String::from_utf16_lossy(&start_prefix.GetText(-1)?)
                .chars()
                .count();
            let end_prefix = document.Clone()?;
            end_prefix.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_End,
            )?;
            let end_in_document = String::from_utf16_lossy(&end_prefix.GetText(-1)?)
                .chars()
                .count();
            let value_len = value.chars().count();
            let Some(start) = start_in_document.checked_sub(content_start) else {
                return Ok(None);
            };
            let Some(end) = end_in_document.checked_sub(content_start) else {
                return Ok(None);
            };
            if start > end || end > value_len {
                return Ok(None);
            }
            Ok(Some(TextSelection { start, end }))
        }
    }

    fn unique_value_offset(document: &str, value: &str) -> Option<usize> {
        if value.is_empty() {
            return None;
        }
        let mut matches = document.match_indices(value);
        let (byte_offset, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(document[..byte_offset].chars().count())
    }

    fn read_event_strings(array: *const SAFEARRAY) -> WindowsResult<Vec<String>> {
        if array.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: UIA owns a valid one-dimensional SAFEARRAY for the callback.
        let lower = unsafe { SafeArrayGetLBound(array, 1)? };
        // SAFETY: Same SAFEARRAY as above.
        let upper = unsafe { SafeArrayGetUBound(array, 1)? };
        let mut result = Vec::new();
        for index in lower..=upper {
            let mut value = BSTR::new();
            // SAFETY: SafeArrayGetElement copies the BSTR into output.
            unsafe {
                SafeArrayGetElement(array, &index, (&mut value as *mut BSTR).cast::<c_void>())?;
            }
            result.push(String::from_utf16_lossy(&value));
        }
        Ok(result)
    }

    fn property_condition_i32(
        automation: &IUIAutomation,
        property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
        value: i32,
    ) -> WindowsResult<IUIAutomationCondition> {
        let mut variant = variant_i32(value);
        // SAFETY: initialized VARIANT lives through the synchronous call.
        let result = unsafe { automation.CreatePropertyCondition(property, &variant) };
        // SAFETY: Clears the initialized VARIANT exactly once.
        unsafe { VariantClear(&mut variant) }?;
        result
    }

    fn property_condition_string(
        automation: &IUIAutomation,
        property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
        value: &str,
    ) -> WindowsResult<IUIAutomationCondition> {
        let mut variant = variant_bstr(value);
        // SAFETY: initialized VARIANT lives through the synchronous call.
        let result = unsafe { automation.CreatePropertyCondition(property, &variant) };
        // SAFETY: Clears the initialized BSTR VARIANT exactly once.
        unsafe { VariantClear(&mut variant) }?;
        result
    }

    fn variant_i32(value: i32) -> VARIANT {
        let mut variant = VARIANT::default();
        // SAFETY: Select VT_I4 and initialize the matching union field.
        unsafe {
            let inner = variant_inner(&mut variant);
            inner.vt = VT_I4;
            inner.Anonymous.lVal = value;
        }
        variant
    }

    fn variant_bstr(value: &str) -> VARIANT {
        let mut variant = VARIANT::default();
        // SAFETY: Select VT_BSTR and initialize the owning BSTR field.
        unsafe {
            let inner = variant_inner(&mut variant);
            inner.vt = VT_BSTR;
            inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
        }
        variant
    }

    unsafe fn variant_inner(variant: &mut VARIANT) -> &mut VARIANT_0_0 {
        // SAFETY: ManuallyDrop<T> has the same layout as T.
        unsafe {
            &mut *(&mut variant.Anonymous.Anonymous as *mut ManuallyDrop<VARIANT_0_0>
                as *mut VARIANT_0_0)
        }
    }

    fn foreground_process_id() -> u32 {
        // SAFETY: Null foreground window leaves pid zero, never a valid match.
        unsafe {
            let mut process_id = 0;
            let window = GetForegroundWindow();
            if !window.0.is_null() {
                GetWindowThreadProcessId(window, Some(&mut process_id));
            }
            process_id
        }
    }

    fn modifier_is_down() -> bool {
        // SAFETY: Fixed virtual-key state queries are read-only.
        unsafe {
            [VK_CONTROL.0, VK_MENU.0, VK_LWIN.0, VK_RWIN.0]
                .into_iter()
                .any(|key| GetAsyncKeyState(key as i32) < 0)
        }
    }

    fn shift_is_down() -> bool {
        // SAFETY: Fixed virtual-key state query is read-only.
        unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 }
    }

    fn key_capture_allowed(
        target_active: bool,
        composition_active: bool,
        foreground_process_matches: bool,
    ) -> bool {
        foreground_process_matches && (target_active || composition_active)
    }

    fn map_key(virtual_key: u32, shift: bool) -> Option<RawKey> {
        let key = match virtual_key {
            0x41..=0x5A => Some(RawKey::Letter(
                char::from_u32(virtual_key + 0x20).expect("ASCII virtual key"),
            )),
            0x30..=0x39 => Some(RawKey::Digit((virtual_key - 0x30) as u8)),
            value if value == VK_BACK.0 as u32 => Some(RawKey::Backspace),
            value if value == VK_DELETE.0 as u32 => Some(RawKey::Delete),
            value if value == VK_SPACE.0 as u32 => Some(RawKey::Space),
            value if value == VK_ESCAPE.0 as u32 => Some(RawKey::Escape),
            value if value == VK_LEFT.0 as u32 => Some(RawKey::Left),
            value if value == VK_RIGHT.0 as u32 => Some(RawKey::Right),
            value if value == VK_UP.0 as u32 => Some(RawKey::Up),
            value if value == VK_DOWN.0 as u32 => Some(RawKey::Down),
            value if value == VK_HOME.0 as u32 => Some(RawKey::Home),
            value if value == VK_END.0 as u32 => Some(RawKey::End),
            _ => None,
        }?;
        Some(if shift {
            RawKey::Shift(Box::new(key))
        } else {
            key
        })
    }

    struct ComGuard;

    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: run initialized COM successfully on this thread.
            unsafe { CoUninitialize() };
        }
    }

    trait HookFlagsExt {
        fn intersects(self, other: Self) -> bool;
    }

    impl HookFlagsExt for windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT_FLAGS {
        fn intersects(self, other: Self) -> bool {
            (self.0 & other.0) != 0
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            AttachAttemptError, CONTROL_STATE_SCHEMA, ControlLifecycleGuard, ControlPhase,
            ControlStatePublisher, ControlTarget, DEFAULT_FLUSH_SECONDS, DEFAULT_SEGMENT_EVENTS,
            DiscoveryStatus, RecorderShared, TargetPolicyAudit, VK_SHIFT, key_capture_allowed,
            map_key, parse_options_from, receipt_terminal_line, recorder_failure_terminal_line,
            retryable_uia_error_code, should_attempt_target_discovery, unique_value_offset,
            write_control_state_atomic,
        };
        use std::fs;
        use std::path::PathBuf;
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::Duration;
        use std::time::{SystemTime, UNIX_EPOCH};
        use ziranma_core::{
            CaptureSessionKind, ContinuousSegmentV2, DataProtector, ProtectedSegmentEnvelopeV1,
            ProtectedSegmentWriter, ProtectedSegmentWriterConfig, RawKey, SegmentCloseReason,
            SegmentWriteReceipt, WindowsUserDataProtector,
        };

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn explicit_run_and_bounded_rotation_options_are_required() {
            let options = parse_options_from(arguments(&["--run"])).unwrap().unwrap();
            assert!(!options.check_only);
            assert!(!options.metadata_only);
            assert!(!options.control_state);
            assert_eq!(options.session_kind, CaptureSessionKind::Daily);
            assert_eq!(options.segment_events, DEFAULT_SEGMENT_EVENTS);
            assert_eq!(options.flush_seconds, DEFAULT_FLUSH_SECONDS);

            assert!(parse_options_from(arguments(&[])).is_err());
            assert!(parse_options_from(arguments(&["--run", "--check"])).is_err());
            assert!(parse_options_from(arguments(&["--check", "--metadata"])).is_err());
            assert!(parse_options_from(arguments(&["--run", "--segment-events", "0"])).is_err());
            assert!(parse_options_from(arguments(&["--run", "--flush-seconds", "1"])).is_err());
            let controlled = parse_options_from(arguments(&["--run", "--control-state"]))
                .unwrap()
                .unwrap();
            assert!(controlled.control_state);
            assert!(
                parse_options_from(arguments(&["--help"]))
                    .unwrap()
                    .is_none()
            );
            assert!(parse_options_from(arguments(&["--run", "--help"])).is_err());
            assert!(parse_options_from(arguments(&["--help", "--run"])).is_err());
        }

        #[test]
        fn check_mode_cannot_hide_recording_configuration() {
            assert!(
                parse_options_from(arguments(&["--check", "--session-kind", "theme"])).is_err()
            );
            let check = parse_options_from(arguments(&["--check"]))
                .unwrap()
                .unwrap();
            assert!(check.check_only);
            assert!(!check.metadata_only);
            assert!(!check.control_state);

            let metadata = parse_options_from(arguments(&["--metadata"]))
                .unwrap()
                .unwrap();
            assert!(metadata.metadata_only);
            assert!(!metadata.check_only);
            assert!(!metadata.control_state);
            assert!(
                parse_options_from(arguments(&["--metadata", "--session-kind", "theme"])).is_err()
            );
            assert!(parse_options_from(arguments(&["--check", "--control-state"])).is_err());
        }

        #[test]
        fn unknown_argument_is_suppressed_instead_of_echoed() {
            let marker = r"Z:\synthetic-private\PRIVATE_PATH_MARKER";
            let error = parse_options_from(arguments(&[marker]))
                .unwrap_err()
                .to_string();
            assert_eq!(error, "unknown argument; value was suppressed");
            assert!(!error.contains("PRIVATE_PATH_MARKER"));
        }

        #[test]
        fn control_state_is_redacted_bounded_lifecycle_metadata() {
            let publisher = ControlStatePublisher {
                path: PathBuf::from("unused"),
                session_id: "1234-5678".to_owned(),
                session_kind: CaptureSessionKind::Daily,
                started_unix_ms: 1234,
                phase: ControlPhase::Running,
                target: ControlTarget::Connected,
                saved_segments: 2,
                saved_events: 17,
                last_flush_unix_ms: Some(1300),
            };
            let serialized = publisher.serialized();
            assert!(serialized.starts_with(&format!("schema={CONTROL_STATE_SCHEMA}\n")));
            assert!(serialized.contains("session=1234-5678\n"));
            assert!(serialized.contains("target=connected\n"));
            assert!(serialized.contains("saved_events=17\n"));
            assert!(!serialized.contains("猫猫"));
            assert!(!serialized.contains("keys"));
            assert!(serialized.len() < 4096);
        }

        #[test]
        fn control_state_atomically_replaces_one_fixed_file() {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "ziranma-control-state-test-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("active-v1.txt");
            write_control_state_atomic(&path, b"first").unwrap();
            write_control_state_atomic(&path, b"second").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"second");
            fs::remove_file(path).unwrap();
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn control_state_segment_count_never_moves_backward() {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "ziranma-control-receipt-order-test-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("active-v1.txt");
            let mut publisher = ControlStatePublisher {
                path: path.clone(),
                session_id: "1234-5678".to_owned(),
                session_kind: CaptureSessionKind::Daily,
                started_unix_ms: 1234,
                phase: ControlPhase::Running,
                target: ControlTarget::Connected,
                saved_segments: 0,
                saved_events: 0,
                last_flush_unix_ms: None,
            };
            let receipt = |sequence, events| SegmentWriteReceipt {
                path: PathBuf::from("redacted-test-segment.zcs"),
                sequence,
                events,
                protected_bytes: 1,
                protection: "test-protection",
            };

            publisher.record_receipt(&receipt(4, 3)).unwrap();
            publisher.record_receipt(&receipt(2, 2)).unwrap();

            assert_eq!(publisher.saved_segments, 5);
            assert_eq!(publisher.saved_events, 5);
            let serialized = fs::read_to_string(&path).unwrap();
            assert!(serialized.contains("saved_segments=5\n"));
            assert!(serialized.contains("saved_events=5\n"));
            fs::remove_file(path).unwrap();
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn protected_segment_receipt_never_discloses_its_local_path() {
            let receipt = SegmentWriteReceipt {
                path: PathBuf::from("synthetic-private-marker/secret-segment.zcs"),
                sequence: 4,
                events: 3,
                protected_bytes: 99,
                protection: "test-protection",
            };
            let line = receipt_terminal_line(&receipt);
            assert!(line.starts_with("PROTECTED_SEGMENT_SAVED sequence=4 events=3"));
            assert!(line.contains("path_disclosed=false"));
            assert!(!line.contains("private-marker"));
            assert!(!line.contains("secret-segment"));
        }

        #[test]
        fn recorder_failure_line_is_redacted_and_reports_control_state_truthfully() {
            let direct = recorder_failure_terminal_line(false);
            assert!(direct.contains("control_state_requested=false"));
            assert!(direct.contains("error_details_suppressed=true"));
            assert!(direct.contains("contains_behavioral_metadata=true"));
            assert!(!direct.contains("Users"));
            assert!(recorder_failure_terminal_line(true).contains("control_state_requested=true"));
        }

        #[test]
        fn recorder_batches_pipeline_counters_with_the_synthetic_commit() {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "ziranma-recorder-integrity-test-{}-{stamp}",
                std::process::id()
            ));
            let config = ProtectedSegmentWriterConfig::new(
                directory.clone(),
                "synthetic-integrity".to_owned(),
                CaptureSessionKind::Daily,
                "0.1.0+continuous.7".to_owned(),
                "codex-uia-v2".to_owned(),
                1,
                Duration::from_secs(60),
            )
            .unwrap();
            let mut protected_writer =
                ProtectedSegmentWriter::new(config, WindowsUserDataProtector).unwrap();
            protected_writer.start_new_baseline_epoch().unwrap();
            let writer = Arc::new(Mutex::new(protected_writer));
            let paused = Arc::new(AtomicBool::new(false));
            let fatal_error = Arc::new(Mutex::new(None));
            let shared = RecorderShared::new(
                String::new(),
                Arc::clone(&writer),
                None,
                paused,
                Arc::clone(&fatal_error),
            );
            shared.key(RawKey::Letter('x'));
            shared.value("ignored-before-activation".to_owned(), None, false);
            shared.activate(|| Ok(String::new()), || Ok(())).unwrap();

            shared.key(RawKey::Letter('m'));
            shared.composition("m".to_owned());
            shared.value("m".to_owned(), None, false);
            shared.key(RawKey::Letter('k'));
            shared.composition("mao".to_owned());
            shared.value("mao".to_owned(), None, true);
            shared.composition_finalized();
            shared.key(RawKey::Space);
            shared.value("猫".to_owned(), None, false);

            assert!(fatal_error.lock().unwrap().is_none());
            assert_eq!(writer.lock().unwrap().written_events(), 1);
            let path = fs::read_dir(&directory)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let bytes = fs::read(&path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains('猫'));
            let envelope = ProtectedSegmentEnvelopeV1::from_bytes(&bytes).unwrap();
            let plaintext = WindowsUserDataProtector
                .unprotect(envelope.protected())
                .unwrap();
            let segment = ContinuousSegmentV2::from_plaintext(&plaintext).unwrap();
            let counters = &segment.integrity().counters;
            assert_eq!(counters.key_actions_observed, 3);
            assert_eq!(counters.composition_callbacks_observed, 2);
            assert_eq!(counters.composition_finalized_callbacks_observed, 1);
            assert_eq!(counters.value_callbacks_observed, 3);
            assert_eq!(counters.value_callbacks_without_output, 2);
            assert_eq!(counters.selection_read_errors, 1);
            assert_eq!(counters.tracker_outputs_emitted, 1);
            let ziranma_core::TrackerOutput::Commit(commit) = &segment.capsule().events()[0].output
            else {
                panic!("synthetic input should produce one commit");
            };
            assert!(!commit.keys_complete);

            fs::remove_file(path).unwrap();
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn callback_waiting_at_pause_boundary_cannot_enter_the_next_baseline() {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "ziranma-recorder-pipeline-boundary-test-{}-{stamp}",
                std::process::id()
            ));
            let config = ProtectedSegmentWriterConfig::new(
                directory.clone(),
                "synthetic-pipeline".to_owned(),
                CaptureSessionKind::Daily,
                "0.1.0+continuous.7".to_owned(),
                "codex-uia-v2".to_owned(),
                4,
                Duration::from_secs(60),
            )
            .unwrap();
            let mut protected_writer =
                ProtectedSegmentWriter::new(config, WindowsUserDataProtector).unwrap();
            protected_writer.start_new_baseline_epoch().unwrap();
            let writer = Arc::new(Mutex::new(protected_writer));
            let paused = Arc::new(AtomicBool::new(false));
            let fatal_error = Arc::new(Mutex::new(None));
            let shared = Arc::new(RecorderShared::new(
                String::new(),
                Arc::clone(&writer),
                None,
                Arc::clone(&paused),
                Arc::clone(&fatal_error),
            ));
            shared.activate(|| Ok(String::new()), || Ok(())).unwrap();
            let pipeline = shared.lock_pipeline().unwrap();
            let rendezvous = Arc::new(std::sync::Barrier::new(2));
            let worker_shared = Arc::clone(&shared);
            let worker_rendezvous = Arc::clone(&rendezvous);
            let worker = std::thread::spawn(move || {
                worker_rendezvous.wait();
                worker_shared.value("synthetic".to_owned(), None, false);
            });

            rendezvous.wait();
            paused.store(true, Ordering::Release);
            drop(pipeline);
            worker.join().unwrap();
            shared.pause().unwrap();

            assert!(fatal_error.lock().unwrap().is_none());
            let mut protected_writer = writer.lock().unwrap();
            assert_eq!(protected_writer.pending_events(), 0);
            assert!(
                protected_writer
                    .flush_with_reason(SegmentCloseReason::Continuity)
                    .unwrap()
                    .is_none()
            );
            drop(protected_writer);
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn lifecycle_guard_distinguishes_clean_and_unwound_exits() {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "ziranma-lifecycle-guard-test-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("active-v1.txt");
            let make_state = || {
                Some(Arc::new(Mutex::new(ControlStatePublisher {
                    path: path.clone(),
                    session_id: "1234-5678".to_owned(),
                    session_kind: CaptureSessionKind::Daily,
                    started_unix_ms: 1234,
                    phase: ControlPhase::Running,
                    target: ControlTarget::Connected,
                    saved_segments: 2,
                    saved_events: 17,
                    last_flush_unix_ms: Some(1300),
                })))
            };

            {
                let _guard = ControlLifecycleGuard::new(make_state());
            }
            let failed = fs::read_to_string(&path).unwrap();
            assert!(failed.contains("phase=failed\n"));
            assert!(failed.contains("target=waiting\n"));

            {
                let mut guard = ControlLifecycleGuard::new(make_state());
                guard.finish(ControlPhase::Stopped).unwrap();
            }
            let stopped = fs::read_to_string(&path).unwrap();
            assert!(stopped.contains("phase=stopped\n"));
            assert!(!stopped.contains("phase=failed\n"));

            fs::remove_file(path).unwrap();
            fs::remove_dir(directory).unwrap();
        }

        #[test]
        fn key_capture_requires_foreground_and_exact_focus_or_active_composition() {
            assert!(key_capture_allowed(true, false, true));
            assert!(key_capture_allowed(false, true, true));
            assert!(!key_capture_allowed(false, false, true));
            assert!(!key_capture_allowed(true, true, false));
        }

        #[test]
        fn shifted_allowed_key_retains_its_modifier() {
            assert_eq!(
                map_key(0x25, true),
                Some(RawKey::Shift(Box::new(RawKey::Left)))
            );
            assert_eq!(map_key(VK_SHIFT.0 as u32, false), None);
        }

        #[test]
        fn value_offset_requires_one_unique_match() {
            assert_eq!(unique_value_offset("\n猫猫", "猫猫"), Some(1));
            assert_eq!(unique_value_offset("猫猫猫猫", "猫猫"), None);
            assert_eq!(unique_value_offset("随心输入", ""), None);
        }

        #[test]
        fn ambiguity_status_never_contains_candidate_text() {
            assert_eq!(DiscoveryStatus::Ambiguous(2), DiscoveryStatus::Ambiguous(2));
            assert_ne!(DiscoveryStatus::Waiting, DiscoveryStatus::Ambiguous(0));
            assert_eq!(TargetPolicyAudit::default().named_edits, 0);
        }

        #[test]
        fn paused_waiting_state_never_opens_a_target_baseline() {
            assert!(!should_attempt_target_discovery(true, false));
            assert!(!should_attempt_target_discovery(true, true));
            assert!(!should_attempt_target_discovery(false, true));
            assert!(should_attempt_target_discovery(false, false));
        }

        #[test]
        fn target_rebuild_during_attach_is_retryable_but_pipeline_failure_is_fatal() {
            let rebuild = AttachAttemptError::Retryable("target_changed_during_activation");
            assert!(matches!(&rebuild, AttachAttemptError::Retryable(_)));
            assert_eq!(rebuild.to_string(), "target_changed_during_activation");

            let pipeline = AttachAttemptError::Fatal("synthetic pipeline failure".to_owned());
            assert!(matches!(pipeline, AttachAttemptError::Fatal(_)));
        }

        #[test]
        fn only_documented_transient_uia_codes_retry_handler_registration() {
            assert!(retryable_uia_error_code(0x8004_0201));
            assert!(retryable_uia_error_code(0x8013_1505));
            assert!(!retryable_uia_error_code(0x8004_0204));
            assert!(!retryable_uia_error_code(0x8013_1509));
        }
    }
}
