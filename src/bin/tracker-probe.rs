#[cfg(not(windows))]
fn main() {
    eprintln!("tracker-probe is available only on Windows");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::run()
}

#[cfg(windows)]
mod windows_probe {
    use std::ffi::c_void;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::mem::ManuallyDrop;
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
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
        UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_NamePropertyId,
        UIA_ProcessIdPropertyId, UIA_Text_TextChangedEventId, UIA_TextEditPatternId,
        UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
        VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F11, VK_F12, VK_HOME,
        VK_LEFT, VK_LWIN, VK_MENU, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HC_ACTION,
        KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, MSG, SetWindowsHookExW,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_SYSKEYDOWN,
    };
    use windows::core::{BSTR, Interface, Ref, Result as WindowsResult, implement};

    use ziranma_core::{
        CommitRecord, CorrectionCandidate, CorrectionCandidateDetector, EVENT_CAPSULE_SCHEMA_V1,
        EventCapsuleError, EventCapsuleRecorder, EventCapsuleV1, LocalInputTracker, RawKey,
        RevisionRecord, SESSION_SUMMARY_SCHEMA_V1, SessionSummaryCounts, SessionSummaryV1,
        TextSelection, TrackerOutput,
    };

    const READY_HOTKEY_ID: i32 = 0x5A48;
    const STOP_HOTKEY_ID: i32 = 0x5A49;
    const TARGET_FIND_ATTEMPTS: usize = 5;
    const TARGET_FIND_RETRY_DELAY: Duration = Duration::from_millis(100);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TargetDiscoveryDecision {
        Accept,
        Retry,
        Refuse,
    }

    fn target_discovery_decision(count: i32, attempt: usize) -> TargetDiscoveryDecision {
        if count == 1 {
            TargetDiscoveryDecision::Accept
        } else if count == 0 && attempt < TARGET_FIND_ATTEMPTS {
            TargetDiscoveryDecision::Retry
        } else {
            TargetDiscoveryDecision::Refuse
        }
    }

    #[derive(Debug)]
    struct Options {
        process_id: i32,
        target_name: String,
        check_only: bool,
        capture_keys: bool,
        preview_text: bool,
        candidate_gap_ms: Option<u64>,
        save_summary: Option<PathBuf>,
        save_capsule: Option<PathBuf>,
    }

    struct CandidatePreviewState {
        detector: CorrectionCandidateDetector,
        max_gap_ms: u64,
        started: Instant,
        summary: SessionSummaryCounts,
    }

    impl CandidatePreviewState {
        fn new(max_gap_ms: u64) -> Self {
            Self {
                detector: CorrectionCandidateDetector::new(max_gap_ms),
                max_gap_ms,
                started: Instant::now(),
                summary: SessionSummaryCounts::default(),
            }
        }

        fn reset(&mut self) {
            self.detector = CorrectionCandidateDetector::new(self.max_gap_ms);
            self.started = Instant::now();
            self.summary = SessionSummaryCounts::default();
        }

        fn elapsed_ms(&self) -> u64 {
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
        }
    }

    struct PrivateCapsuleState {
        started: Instant,
        recorder: EventCapsuleRecorder,
        warning_emitted: bool,
    }

    impl PrivateCapsuleState {
        fn new() -> Self {
            Self {
                started: Instant::now(),
                recorder: EventCapsuleRecorder::default(),
                warning_emitted: false,
            }
        }

        fn reset(&mut self) {
            self.started = Instant::now();
            self.recorder.reset();
            self.warning_emitted = false;
        }

        fn observe(&mut self, output: TrackerOutput) {
            let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if let Err(error) = self.recorder.observe(elapsed_ms, output)
                && !self.warning_emitted
            {
                eprintln!("warning: private event capsule stopped accepting events: {error}");
                self.warning_emitted = true;
            }
        }
    }

    struct TrackerState {
        tracker: LocalInputTracker,
        candidate_preview: Option<CandidatePreviewState>,
        private_capsule: Option<PrivateCapsuleState>,
    }

    struct SharedTracker {
        state: Mutex<TrackerState>,
        composition_active: AtomicBool,
        capture_keys: bool,
        key_capture_ready: AtomicBool,
        preview_text: bool,
    }

    impl SharedTracker {
        fn composition(&self, value: String) {
            if let Ok(mut state) = self.state.lock() {
                let tracker = &mut state.tracker;
                let starts_new_composition = !value.is_empty() && !tracker.has_active_composition();
                if self.capture_keys
                    && self.key_capture_ready.load(Ordering::Acquire)
                    && starts_new_composition
                    && tracker.pending_keys_is_empty()
                {
                    tracker.mark_pending_keys_incomplete();
                    eprintln!(
                        "WARNING key_prefix_incomplete=true \
                         reason=composition_started_before_first_scoped_key"
                    );
                }
                tracker.observe_composition(value);
                self.composition_active
                    .store(tracker.has_active_composition(), Ordering::Release);
            }
        }

        fn key(&self, key: RawKey) {
            if let Ok(mut state) = self.state.lock() {
                state.tracker.observe_key(key);
            }
        }

        fn enable_key_capture(&self) {
            if let Ok(mut state) = self.state.lock() {
                state.tracker.cancel_composition();
                state.tracker.set_key_capture_enabled(true);
                if let Some(candidate_preview) = state.candidate_preview.as_mut() {
                    candidate_preview.reset();
                }
                if let Some(private_capsule) = state.private_capsule.as_mut() {
                    private_capsule.reset();
                }
                self.composition_active.store(false, Ordering::Release);
                self.key_capture_ready.store(true, Ordering::Release);
            }
        }

        fn value(&self, value: String, selection: Option<TextSelection>) {
            let observed = self.state.lock().ok().map(|mut state| {
                let output = state.tracker.observe_value_with_selection(value, selection);
                self.composition_active
                    .store(state.tracker.has_active_composition(), Ordering::Release);
                let candidate = match (state.candidate_preview.as_mut(), output.as_ref()) {
                    (Some(preview), Some(output)) => {
                        preview.summary.observe_output(output);
                        let candidate = preview
                            .detector
                            .observe(preview.elapsed_ms(), output.clone());
                        if let Ok(Some(candidate)) = candidate.as_ref() {
                            preview.summary.observe_candidate(candidate);
                        }
                        Some(candidate)
                    }
                    _ => None,
                };
                if self.key_capture_ready.load(Ordering::Acquire)
                    && let (Some(private_capsule), Some(output)) =
                        (state.private_capsule.as_mut(), output.as_ref())
                {
                    private_capsule.observe(output.clone());
                }
                (output, candidate)
            });
            if let Some((Some(output), candidate)) = observed {
                print_output(&output, self.preview_text);
                match candidate {
                    Some(Ok(Some(candidate))) => print_candidate(&candidate, self.preview_text),
                    Some(Err(error)) => {
                        eprintln!("warning: correction candidate detector rejected event: {error}")
                    }
                    _ => {}
                }
            }
        }

        fn candidate_session_report(&self) -> Option<SessionSummaryV1> {
            let state = self.state.lock().ok()?;
            let preview = state.candidate_preview.as_ref()?;
            Some(SessionSummaryV1 {
                candidate_gap_limit_ms: preview.max_gap_ms,
                elapsed_ms: preview.elapsed_ms(),
                key_capture_requested: self.capture_keys,
                key_capture_ready: self.key_capture_ready.load(Ordering::Acquire),
                counts: preview.summary.clone(),
            })
        }

        fn private_event_capsule(&self) -> Result<Option<EventCapsuleV1>, EventCapsuleError> {
            let state = self
                .state
                .lock()
                .map_err(|_| EventCapsuleError::InvalidInvariant {
                    event: 0,
                    field: "tracker state lock was poisoned",
                })?;
            state
                .private_capsule
                .as_ref()
                .map(|capsule| capsule.recorder.finish())
                .transpose()
        }
    }

    #[implement(IUIAutomationEventHandler)]
    struct ValueChangedHandler {
        shared: Arc<SharedTracker>,
    }

    #[allow(non_snake_case)]
    impl IUIAutomationEventHandler_Impl for ValueChangedHandler_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: Ref<IUIAutomationElement>,
            _eventid: windows::Win32::UI::Accessibility::UIA_EVENT_ID,
        ) -> WindowsResult<()> {
            let sender = sender.ok()?;
            match read_value(sender) {
                Ok(value) => {
                    let selection = read_selection(sender, &value).ok().flatten();
                    self.shared.value(value, selection);
                }
                Err(error) => eprintln!("warning: could not read target value: {error}"),
            }
            Ok(())
        }
    }

    #[implement(IUIAutomationTextEditTextChangedEventHandler)]
    struct CompositionHandler {
        shared: Arc<SharedTracker>,
        preview_text: bool,
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
                let composition = read_event_strings(event_strings)?.join("|");
                if self.preview_text {
                    println!("COMPOSITION {composition:?}");
                } else {
                    println!("COMPOSITION chars={}", composition.chars().count());
                }
                self.shared.composition(composition);
            } else if change_type == TextEditChangeType_CompositionFinalized {
                let finalized = read_event_strings(event_strings)?.join("|");
                if self.preview_text {
                    println!("FINALIZED {finalized:?}");
                } else {
                    println!("FINALIZED chars={}", finalized.chars().count());
                }
                // Keep the last pinyin composition active. The ordinary
                // Text_TextChanged event supplies the bounded before/after
                // delta, including on providers that raise Finalized first.
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    struct TargetIdentity {
        process_id: i32,
        target_name: String,
    }

    #[implement(IUIAutomationFocusChangedEventHandler)]
    struct FocusChangedHandler {
        target: TargetIdentity,
        target_active: Arc<AtomicBool>,
    }

    #[allow(non_snake_case)]
    impl IUIAutomationFocusChangedEventHandler_Impl for FocusChangedHandler_Impl {
        fn HandleFocusChangedEvent(&self, sender: Ref<IUIAutomationElement>) -> WindowsResult<()> {
            let is_target = sender
                .as_ref()
                .is_some_and(|sender| is_exact_safe_target(sender, &self.target));
            self.target_active.store(is_target, Ordering::Release);
            Ok(())
        }
    }

    struct KeyHookContext {
        target_process_id: u32,
        target_active: Arc<AtomicBool>,
        shared: Arc<SharedTracker>,
    }

    static KEY_HOOK_CONTEXT: OnceLock<KeyHookContext> = OnceLock::new();

    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32
            && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN)
            && let Some(context) = KEY_HOOK_CONTEXT.get()
            && key_capture_allowed(
                context.shared.key_capture_ready.load(Ordering::Acquire),
                context.target_active.load(Ordering::Acquire),
                context.shared.composition_active.load(Ordering::Acquire),
                foreground_process_id() == context.target_process_id,
            )
        {
            // SAFETY: Windows supplies a valid KBDLLHOOKSTRUCT pointer for a
            // WH_KEYBOARD_LL callback while code == HC_ACTION.
            let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if !data
                .flags
                .intersects(LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED)
                && !modifier_is_down()
                && let Some(key) = map_key(data.vkCode, shift_is_down())
            {
                context.shared.key(key);
            }
        }

        // SAFETY: Forwarding every hook invocation is required by the Win32
        // hook contract. This probe never blocks or alters input.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let Some(options) = parse_options()? else {
            return Ok(());
        };
        let summary_target = options
            .save_summary
            .as_deref()
            .map(prepare_summary_target)
            .transpose()?;
        let capsule_target = options
            .save_capsule
            .as_deref()
            .map(prepare_capsule_target)
            .transpose()?;

        // SAFETY: COM is initialized once on this thread and balanced by the
        // guard below after every COM interface has been released.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        let _com = ComGuard;

        // SAFETY: CUIAutomation8 is an in-process Windows COM class with the
        // requested IUIAutomation interface.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)? };
        let automation3: IUIAutomation3 = automation.cast()?;

        let edit = find_target(&automation, &options)?;
        validate_target(&edit, &options)?;
        if options.check_only {
            println!(
                "CHECKED pid={} name={:?} enabled=true focusable=true password=false \
                 textedit=true value=true",
                options.process_id, options.target_name
            );
            return Ok(());
        }
        let initial_value = read_value(&edit)?;

        let mut tracker = LocalInputTracker::new(options.target_name.clone(), initial_value);
        tracker.set_key_capture_enabled(false);
        let shared = Arc::new(SharedTracker {
            state: Mutex::new(TrackerState {
                tracker,
                candidate_preview: options.candidate_gap_ms.map(CandidatePreviewState::new),
                private_capsule: capsule_target.as_ref().map(|_| PrivateCapsuleState::new()),
            }),
            composition_active: AtomicBool::new(false),
            capture_keys: options.capture_keys,
            key_capture_ready: AtomicBool::new(false),
            preview_text: options.preview_text,
        });

        let value_handler: IUIAutomationEventHandler = ValueChangedHandler {
            shared: Arc::clone(&shared),
        }
        .into();
        let composition_handler: IUIAutomationTextEditTextChangedEventHandler =
            CompositionHandler {
                shared: Arc::clone(&shared),
                preview_text: options.preview_text,
            }
            .into();

        // SAFETY: All handlers and the exact target element remain alive until
        // their matching removal calls below.
        unsafe {
            automation.AddAutomationEventHandler(
                UIA_Text_TextChangedEventId,
                &edit,
                TreeScope_Element,
                None::<&IUIAutomationCacheRequest>,
                &value_handler,
            )?;
            automation3.AddTextEditTextChangedEventHandler(
                &edit,
                TreeScope_Element,
                TextEditChangeType_Composition,
                None::<&IUIAutomationCacheRequest>,
                &composition_handler,
            )?;
            automation3.AddTextEditTextChangedEventHandler(
                &edit,
                TreeScope_Element,
                TextEditChangeType_CompositionFinalized,
                None::<&IUIAutomationCacheRequest>,
                &composition_handler,
            )?;
        }

        let target_active = Arc::new(AtomicBool::new(
            // SAFETY: The validated UIA element remains alive.
            unsafe { edit.CurrentHasKeyboardFocus() }
                .map(|focused| focused.as_bool())
                .unwrap_or(false),
        ));
        let mut focus_handler = None;
        let mut hook = None;

        if options.capture_keys {
            let handler: IUIAutomationFocusChangedEventHandler = FocusChangedHandler {
                target: TargetIdentity {
                    process_id: options.process_id,
                    target_name: options.target_name.clone(),
                },
                target_active: Arc::clone(&target_active),
            }
            .into();
            // SAFETY: The focus handler stays alive until it is removed.
            unsafe {
                automation
                    .AddFocusChangedEventHandler(None::<&IUIAutomationCacheRequest>, &handler)?;
            }
            focus_handler = Some(handler);

            KEY_HOOK_CONTEXT
                .set(KeyHookContext {
                    target_process_id: options.process_id as u32,
                    target_active: Arc::clone(&target_active),
                    shared: Arc::clone(&shared),
                })
                .map_err(|_| "keyboard hook context was already initialized")?;
            // SAFETY: keyboard_hook has the required static callback ABI and
            // this process keeps it alive for the duration of the message loop.
            hook =
                Some(unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)? });
        }

        // SAFETY: The process owns this thread-level hotkey registration and
        // unregisters it before returning.
        unsafe {
            if options.capture_keys {
                RegisterHotKey(
                    None,
                    READY_HOTKEY_ID,
                    MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                    VK_F11.0 as u32,
                )?;
            }
            RegisterHotKey(
                None,
                STOP_HOTKEY_ID,
                MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                VK_F12.0 as u32,
            )?;
        }

        println!(
            "ARMED pid={} name={:?} keys={} candidate_preview={} summary_export={} \
             private_capsule_export={} streaming_disk_logging=false",
            options.process_id,
            options.target_name,
            options.capture_keys,
            options.candidate_gap_ms.is_some(),
            summary_target.is_some(),
            capsule_target.is_some()
        );
        if let Some(candidate_gap_ms) = options.candidate_gap_ms {
            println!(
                "CANDIDATE_PREVIEW_READY max_gap_ms={candidate_gap_ms} text_preview={}",
                options.preview_text
            );
        }
        if options.capture_keys {
            println!("Focus the target edit and press Ctrl+Shift+F11 to begin key pairing.");
        }
        if summary_target.is_some() {
            println!("Press Ctrl+Shift+F12 to stop. One redacted summary file will be created.");
        } else if capsule_target.is_none() {
            println!("Press Ctrl+Shift+F12 to stop. No file will be written.");
        }
        if capsule_target.is_some() {
            println!("PRIVATE_CAPSULE_EXPORT_READY contains_private_text=true encryption=none");
            println!(
                "Press Ctrl+Shift+F12 to stop. One private plaintext capsule will be created."
            );
        }
        message_loop(&edit, &options, &target_active, &shared)?;

        // SAFETY: Each cleanup call matches a successful registration above.
        unsafe {
            if options.capture_keys {
                let _ = UnregisterHotKey(None, READY_HOTKEY_ID);
            }
            let _ = UnregisterHotKey(None, STOP_HOTKEY_ID);
            if let Some(hook) = hook {
                let _ = UnhookWindowsHookEx(hook);
            }
            if let Some(handler) = focus_handler.as_ref() {
                let _ = automation.RemoveFocusChangedEventHandler(handler);
            }
            let _ = automation.RemoveAutomationEventHandler(
                UIA_Text_TextChangedEventId,
                &edit,
                &value_handler,
            );
            let _ = automation3.RemoveTextEditTextChangedEventHandler(&edit, &composition_handler);
        }

        let report = shared.candidate_session_report();
        let capsule = if capsule_target.is_some() {
            shared.private_event_capsule()?
        } else {
            None
        };
        let summary_json = report.as_ref().map(SessionSummaryV1::to_json).transpose()?;
        let capsule_text = capsule.as_ref().map(EventCapsuleV1::to_text).transpose()?;
        if let Some(report) = report.as_ref() {
            println!("{}", report.terminal_line());
        }
        if let Some(target) = summary_target.as_ref() {
            let json = summary_json
                .as_ref()
                .ok_or("summary export requested but candidate session report is unavailable")?;
            save_private_file_create_new(target, json)?;
            println!(
                "SUMMARY_SAVED path={target:?} schema={SESSION_SUMMARY_SCHEMA_V1} \
                 contains_text=false"
            );
        }
        if let Some(target) = capsule_target.as_ref() {
            let capsule = capsule
                .as_ref()
                .ok_or("private event capsule export was requested but is unavailable")?;
            let text = capsule_text
                .as_ref()
                .ok_or("private event capsule serialization is unavailable")?;
            save_private_file_create_new(target, text)?;
            println!(
                "PRIVATE_CAPSULE_SAVED path={target:?} schema={EVENT_CAPSULE_SCHEMA_V1} \
                 contains_private_text=true encryption=none events={}",
                capsule.events().len()
            );
        }
        println!(
            "STOPPED records_were_memory_only={} summary_saved={} private_capsule_saved={}",
            capsule_target.is_none(),
            summary_target.is_some(),
            capsule_target.is_some()
        );
        Ok(())
    }

    fn parse_options() -> Result<Option<Options>, Box<dyn std::error::Error>> {
        parse_options_from(std::env::args().skip(1))
    }

    fn parse_options_from(
        arguments: impl IntoIterator<Item = String>,
    ) -> Result<Option<Options>, Box<dyn std::error::Error>> {
        let mut args = arguments.into_iter();
        let mut process_id = None;
        let mut target_name = "随心输入".to_owned();
        let mut capture_keys = false;
        let mut preview_text = false;
        let mut preview_candidates = false;
        let mut candidate_gap_ms = None;
        let mut save_summary = None;
        let mut save_capsule = None;
        let mut allow_private_plaintext = false;
        let mut armed = false;
        let mut check_only = false;

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--pid" => {
                    let value = args.next().ok_or("--pid requires a value")?;
                    process_id = Some(value.parse::<i32>()?);
                }
                "--target-name" => {
                    target_name = args.next().ok_or("--target-name requires a value")?;
                }
                "--capture-keys" => capture_keys = true,
                "--preview-text" => preview_text = true,
                "--preview-candidates" => preview_candidates = true,
                "--candidate-gap-ms" => {
                    let value = args.next().ok_or("--candidate-gap-ms requires a value")?;
                    candidate_gap_ms = Some(value.parse::<u64>()?);
                }
                "--save-summary" => {
                    let value = args.next().ok_or("--save-summary requires a path")?;
                    save_summary = Some(PathBuf::from(value));
                }
                "--save-capsule" => {
                    let value = args.next().ok_or("--save-capsule requires a path")?;
                    save_capsule = Some(PathBuf::from(value));
                }
                "--allow-private-plaintext" => allow_private_plaintext = true,
                "--arm" => armed = true,
                "--check" => check_only = true,
                "--help" | "-h" => {
                    print_usage();
                    return Ok(None);
                }
                other => return Err(format!("unknown argument: {other}").into()),
            }
        }

        if armed == check_only {
            print_usage();
            return Err("choose exactly one of --check or --arm".into());
        }
        let process_id = process_id.ok_or("--pid is required")?;
        if process_id <= 0 {
            return Err("--pid must be a positive process id".into());
        }
        if target_name.is_empty() {
            return Err("--target-name cannot be empty".into());
        }
        if check_only
            && (capture_keys
                || preview_text
                || preview_candidates
                || candidate_gap_ms.is_some()
                || save_summary.is_some()
                || save_capsule.is_some()
                || allow_private_plaintext)
        {
            return Err("--check cannot be combined with capture, preview, or export".into());
        }
        match (preview_candidates, candidate_gap_ms) {
            (true, None) => {
                return Err("--preview-candidates requires --candidate-gap-ms".into());
            }
            (false, Some(_)) => {
                return Err("--candidate-gap-ms requires --preview-candidates".into());
            }
            (true, Some(0)) => {
                return Err("--candidate-gap-ms must be greater than zero".into());
            }
            _ => {}
        }
        if save_summary.is_some() && !preview_candidates {
            return Err("--save-summary requires --preview-candidates".into());
        }
        if save_capsule.is_some() && !capture_keys {
            return Err("--save-capsule requires --capture-keys".into());
        }
        if save_capsule.is_some() && !allow_private_plaintext {
            return Err(
                "--save-capsule requires the separate --allow-private-plaintext acknowledgement"
                    .into(),
            );
        }
        if allow_private_plaintext && save_capsule.is_none() {
            return Err("--allow-private-plaintext requires --save-capsule".into());
        }

        Ok(Some(Options {
            process_id,
            target_name,
            check_only,
            capture_keys,
            preview_text,
            candidate_gap_ms,
            save_summary,
            save_capsule,
        }))
    }

    fn print_usage() {
        eprintln!(
            "usage: tracker-probe --pid <PID> (--check|--arm) [--target-name 随心输入] \
             [--preview-text] [--capture-keys] \
             [--preview-candidates --candidate-gap-ms <MS>] \
             [--save-summary data/private/session-summaries/<NEW>.json] \
             [--save-capsule data/private/event-capsules/<NEW>.zic \
              --allow-private-plaintext]"
        );
        eprintln!("--check reads no input text and installs no listeners");
        eprintln!("--arm defaults to memory-only, text redacted, no keyboard hook, no disk writes");
        eprintln!("--capture-keys remains idle until Ctrl+Shift+F11 confirms exact target focus");
        eprintln!(
            "--preview-candidates requires an explicit positive candidate gap in milliseconds"
        );
        eprintln!("--save-summary writes only the redacted v1 summary and refuses existing files");
        eprintln!(
            "--save-capsule writes bounded private text and keys once at stop; it is not encrypted"
        );
    }

    fn prepare_summary_target(requested: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let summary_root = manifest_dir.join("data/private/session-summaries");
        let target = resolve_summary_target(manifest_dir, &summary_root, requested)?;
        ensure_private_summary_root(manifest_dir, &summary_root)?;

        let canonical_manifest = fs::canonicalize(manifest_dir)?;
        let canonical_root = fs::canonicalize(&summary_root)?;
        if !canonical_root.starts_with(&canonical_manifest) {
            return Err("private summary directory resolves outside the repository".into());
        }
        let file_name = target
            .file_name()
            .ok_or("summary target must include a file name")?;
        let canonical_target = canonical_root.join(file_name);
        match fs::symlink_metadata(&canonical_target) {
            Ok(_) => return Err("summary target already exists; refusing to overwrite".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(canonical_target)
    }

    fn resolve_summary_target(
        manifest_dir: &Path,
        summary_root: &Path,
        requested: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        if requested.as_os_str().is_empty() {
            return Err("summary target path cannot be empty".into());
        }
        let target = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            manifest_dir.join(requested)
        };
        if target.parent() != Some(summary_root) {
            return Err(
                "summary target must be directly inside data/private/session-summaries".into(),
            );
        }
        if target.extension().and_then(|extension| extension.to_str()) != Some("json") {
            return Err("summary target must use the .json extension".into());
        }
        let file_name = target
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .ok_or("summary target file name must be valid Unicode")?;
        if file_name.starts_with('.') {
            return Err("summary target file name cannot be hidden".into());
        }
        Ok(target)
    }

    fn ensure_private_summary_root(
        manifest_dir: &Path,
        summary_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut current = manifest_dir.to_path_buf();
        for component in ["data", "private", "session-summaries"] {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "private summary directory contains a symbolic-link component: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "private summary directory component is not a directory: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        fs::create_dir_all(summary_root)?;
        Ok(())
    }

    fn prepare_capsule_target(requested: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let capsule_root = manifest_dir.join("data/private/event-capsules");
        let target = resolve_capsule_target(manifest_dir, &capsule_root, requested)?;
        ensure_private_capsule_root(manifest_dir, &capsule_root)?;

        let canonical_manifest = fs::canonicalize(manifest_dir)?;
        let canonical_root = fs::canonicalize(&capsule_root)?;
        if !canonical_root.starts_with(&canonical_manifest) {
            return Err("private capsule directory resolves outside the repository".into());
        }
        let file_name = target
            .file_name()
            .ok_or("capsule target must include a file name")?;
        let canonical_target = canonical_root.join(file_name);
        match fs::symlink_metadata(&canonical_target) {
            Ok(_) => return Err("capsule target already exists; refusing to overwrite".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(canonical_target)
    }

    fn resolve_capsule_target(
        manifest_dir: &Path,
        capsule_root: &Path,
        requested: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        if requested.as_os_str().is_empty() {
            return Err("capsule target path cannot be empty".into());
        }
        let target = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            manifest_dir.join(requested)
        };
        if target.parent() != Some(capsule_root) {
            return Err(
                "capsule target must be directly inside data/private/event-capsules".into(),
            );
        }
        if target.extension().and_then(|extension| extension.to_str()) != Some("zic") {
            return Err("capsule target must use the .zic extension".into());
        }
        let file_name = target
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .ok_or("capsule target file name must be valid Unicode")?;
        if file_name.starts_with('.') {
            return Err("capsule target file name cannot be hidden".into());
        }
        Ok(target)
    }

    fn ensure_private_capsule_root(
        manifest_dir: &Path,
        capsule_root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut current = manifest_dir.to_path_buf();
        for component in ["data", "private", "event-capsules"] {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "private capsule directory contains a symbolic-link component: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(format!(
                        "private capsule directory component is not a directory: {}",
                        current.display()
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        fs::create_dir_all(capsule_root)?;
        Ok(())
    }

    fn save_private_file_create_new(
        target: &Path,
        contents: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let parent = target
            .parent()
            .ok_or("summary target must have a parent directory")?;
        let file_name = target
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .ok_or("summary target file name must be valid Unicode")?;
        let mut temporary = None;
        for attempt in 0..100_u32 {
            let candidate = parent.join(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                attempt
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    temporary = Some((candidate, file));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        let (temporary_path, mut file) =
            temporary.ok_or("could not allocate a private temporary file")?;
        let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            file.write_all(contents.as_bytes())?;
            file.write_all(b"\n")?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::hard_link(&temporary_path, target)?;
            fs::remove_file(&temporary_path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result
    }

    fn find_target(
        automation: &IUIAutomation,
        options: &Options,
    ) -> Result<IUIAutomationElement, Box<dyn std::error::Error>> {
        // SAFETY: UI Automation copies each condition value. The temporary
        // variants are cleared immediately after condition construction.
        let process =
            property_condition_i32(automation, UIA_ProcessIdPropertyId, options.process_id)?;
        let control = property_condition_i32(
            automation,
            UIA_ControlTypePropertyId,
            UIA_EditControlTypeId.0,
        )?;
        let name = property_condition_string(automation, UIA_NamePropertyId, &options.target_name)?;
        // SAFETY: The three live condition interfaces are valid parameters.
        let combined = unsafe {
            let first = automation.CreateAndCondition(&process, &control)?;
            automation.CreateAndCondition(&first, &name)?
        };
        for attempt in 1..=TARGET_FIND_ATTEMPTS {
            // SAFETY: The root and combined condition remain alive for the call.
            let matches = unsafe {
                automation
                    .GetRootElement()?
                    .FindAll(TreeScope_Descendants, &combined)?
            };
            // SAFETY: The returned element array remains alive for both calls.
            let count = unsafe { matches.Length()? };
            match target_discovery_decision(count, attempt) {
                TargetDiscoveryDecision::Accept => {
                    if attempt > 1 {
                        eprintln!("TARGET_DISCOVERY_RETRIED attempts={attempt}");
                    }
                    // SAFETY: count == 1 proves that index zero exists.
                    return Ok(unsafe { matches.GetElement(0)? });
                }
                TargetDiscoveryDecision::Retry => {
                    std::thread::sleep(TARGET_FIND_RETRY_DELAY);
                }
                TargetDiscoveryDecision::Refuse => {
                    let attempts = if count == 0 {
                        format!(" after {attempt} attempts")
                    } else {
                        String::new()
                    };
                    return Err(format!(
                        "expected exactly one safe target, found {count}{attempts}; \
                         refusing to choose implicitly"
                    )
                    .into());
                }
            }
        }
        unreachable!("the final target discovery attempt cannot request another retry")
    }

    fn validate_target(
        edit: &IUIAutomationElement,
        options: &Options,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: These are read-only properties/pattern queries on the target.
        unsafe {
            if edit.CurrentProcessId()? != options.process_id {
                return Err("target process changed during discovery".into());
            }
            if !edit.CurrentIsEnabled()?.as_bool() || !edit.CurrentIsKeyboardFocusable()?.as_bool()
            {
                return Err("target is disabled or not keyboard-focusable".into());
            }
            if edit.CurrentIsPassword()?.as_bool() {
                return Err("refusing to track a password element".into());
            }
            let _: IUIAutomationTextEditPattern =
                edit.GetCurrentPatternAs(UIA_TextEditPatternId)?;
            let _: IUIAutomationValuePattern = edit.GetCurrentPatternAs(UIA_ValuePatternId)?;
        }
        Ok(())
    }

    fn read_value(element: &IUIAutomationElement) -> WindowsResult<String> {
        // SAFETY: The caller supplies a live UIA element. The returned BSTR
        // owns its storage and is converted without retaining a raw pointer.
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
        // SAFETY: Every range comes from the live, exact target element. Text
        // is retained only long enough to count character offsets and is never
        // printed or stored by this helper.
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

    fn property_condition_i32(
        automation: &IUIAutomation,
        property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
        value: i32,
    ) -> WindowsResult<IUIAutomationCondition> {
        let mut variant = variant_i32(value);
        // SAFETY: The initialized VARIANT remains live for the synchronous call.
        let result = unsafe { automation.CreatePropertyCondition(property, &variant) };
        // SAFETY: Clears exactly the initialized VARIANT once.
        let clear = unsafe { VariantClear(&mut variant) };
        clear?;
        result
    }

    fn property_condition_string(
        automation: &IUIAutomation,
        property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
        value: &str,
    ) -> WindowsResult<IUIAutomationCondition> {
        let mut variant = variant_bstr(value);
        // SAFETY: The initialized VARIANT remains live for the synchronous call.
        let result = unsafe { automation.CreatePropertyCondition(property, &variant) };
        // SAFETY: Clears exactly the initialized BSTR VARIANT once.
        let clear = unsafe { VariantClear(&mut variant) };
        clear?;
        result
    }

    fn variant_i32(value: i32) -> VARIANT {
        let mut variant = VARIANT::default();
        // SAFETY: We select VT_I4 and initialize the matching lVal union field.
        unsafe {
            let inner = variant_inner(&mut variant);
            inner.vt = VT_I4;
            inner.Anonymous.lVal = value;
        }
        variant
    }

    fn variant_bstr(value: &str) -> VARIANT {
        let mut variant = VARIANT::default();
        // SAFETY: We select VT_BSTR and initialize the matching owning BSTR
        // union field. VariantClear later releases it.
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

    fn read_event_strings(array: *const SAFEARRAY) -> WindowsResult<Vec<String>> {
        if array.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: UIA owns a valid one-dimensional SAFEARRAY for the callback.
        let lower = unsafe { SafeArrayGetLBound(array, 1)? };
        // SAFETY: Same valid SAFEARRAY as above.
        let upper = unsafe { SafeArrayGetUBound(array, 1)? };
        let mut result = Vec::new();
        for index in lower..=upper {
            let mut value = BSTR::new();
            // SAFETY: SafeArrayGetElement copies the BSTR into the initialized
            // output slot; BSTR drop releases the returned copy.
            unsafe {
                SafeArrayGetElement(array, &index, (&mut value as *mut BSTR).cast::<c_void>())?;
            }
            result.push(String::from_utf16_lossy(&value));
        }
        Ok(result)
    }

    fn is_exact_safe_target(element: &IUIAutomationElement, identity: &TargetIdentity) -> bool {
        // SAFETY: All operations are read-only UIA property queries. Other
        // elements' names are never printed or retained.
        unsafe {
            let is_password = match element.CurrentIsPassword() {
                Ok(value) => value.as_bool(),
                Err(_) => true,
            };
            if element.CurrentProcessId().ok() != Some(identity.process_id)
                || is_password
                || element.CurrentControlType().ok() != Some(UIA_EditControlTypeId)
            {
                return false;
            }
            element
                .CurrentName()
                .map(|name| String::from_utf16_lossy(&name) == identity.target_name)
                .unwrap_or(false)
        }
    }

    fn foreground_process_id() -> u32 {
        // SAFETY: The HWND may be null; GetWindowThreadProcessId then leaves
        // pid at zero, which cannot match a validated positive target pid.
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
        // SAFETY: GetAsyncKeyState is a read-only query for fixed virtual keys.
        unsafe {
            [VK_CONTROL.0, VK_MENU.0, VK_LWIN.0, VK_RWIN.0]
                .into_iter()
                .any(|key| GetAsyncKeyState(key as i32) < 0)
        }
    }

    fn shift_is_down() -> bool {
        // SAFETY: GetAsyncKeyState is a read-only query for a fixed virtual key.
        unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 }
    }

    fn key_capture_allowed(
        capture_ready: bool,
        target_active: bool,
        composition_active: bool,
        foreground_process_matches: bool,
    ) -> bool {
        capture_ready && foreground_process_matches && (target_active || composition_active)
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

    fn message_loop(
        edit: &IUIAutomationElement,
        options: &Options,
        target_active: &AtomicBool,
        shared: &SharedTracker,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut message = MSG::default();
        loop {
            // SAFETY: message points to valid writable storage and no HWND
            // filter is used.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 == -1 {
                return Err(windows::core::Error::from_thread().into());
            }
            if !result.as_bool() {
                break;
            }
            if message.message != WM_HOTKEY {
                continue;
            }
            if message.wParam.0 == STOP_HOTKEY_ID as usize {
                break;
            }
            if message.wParam.0 == READY_HOTKEY_ID as usize && options.capture_keys {
                if ready_target_is_focused(edit, options.process_id) {
                    target_active.store(true, Ordering::Release);
                    shared.enable_key_capture();
                    println!(
                        "KEY_CAPTURE_READY pid={} name={:?}",
                        options.process_id, options.target_name
                    );
                } else {
                    eprintln!("KEY_CAPTURE_REFUSED reason=target_edit_does_not_have_exact_focus");
                }
            }
        }
        Ok(())
    }

    fn ready_target_is_focused(edit: &IUIAutomationElement, process_id: i32) -> bool {
        if foreground_process_id() != process_id as u32 {
            return false;
        }
        // SAFETY: These are read-only checks on the already validated exact
        // target element, performed outside the low-level keyboard callback.
        unsafe {
            edit.CurrentProcessId().ok() == Some(process_id)
                && edit
                    .CurrentHasKeyboardFocus()
                    .ok()
                    .is_some_and(|focused| focused.as_bool())
                && edit
                    .CurrentIsEnabled()
                    .ok()
                    .is_some_and(|enabled| enabled.as_bool())
                && edit
                    .CurrentIsPassword()
                    .ok()
                    .is_some_and(|password| !password.as_bool())
        }
    }

    fn print_output(output: &TrackerOutput, preview_text: bool) {
        match output {
            TrackerOutput::Commit(record) => print_commit(record, preview_text),
            TrackerOutput::Revision(change) => print_revision(change, preview_text),
        }
    }

    fn print_commit(record: &CommitRecord, preview_text: bool) {
        if preview_text {
            println!(
                "COMMIT keys_complete={} keys={:?} composition={:?} \
                 preedit_position={:?} preedit_start={} preedit_deleted={:?} \
                 preedit_inserted={:?} document_position={:?} document_start={} \
                 document_deleted={:?} document_inserted={:?}",
                record.keys_complete,
                record.keys,
                record.composition,
                record.change.position_evidence,
                record.change.start,
                record.change.deleted,
                record.change.inserted,
                record.document_change.position_evidence,
                record.document_change.start,
                record.document_change.deleted,
                record.document_change.inserted
            );
        } else {
            println!(
                "COMMIT keys_complete={} keys={} composition_chars={} \
                 preedit_position={:?} preedit_start={} preedit_deleted_chars={} \
                 preedit_inserted_chars={} document_position={:?} document_start={} \
                 document_deleted_chars={} document_inserted_chars={}",
                record.keys_complete,
                record.keys.len(),
                record.composition.chars().count(),
                record.change.position_evidence,
                record.change.start,
                record.change.deleted.chars().count(),
                record.change.inserted.chars().count(),
                record.document_change.position_evidence,
                record.document_change.start,
                record.document_change.deleted.chars().count(),
                record.document_change.inserted.chars().count()
            );
        }
    }

    fn print_revision(record: &RevisionRecord, preview_text: bool) {
        if preview_text {
            println!(
                "REVISION keys_complete={} keys={:?} position={:?} start={} deleted={:?} \
                 inserted={:?}",
                record.keys_complete,
                record.keys,
                record.change.position_evidence,
                record.change.start,
                record.change.deleted,
                record.change.inserted
            );
        } else {
            println!(
                "REVISION keys_complete={} keys={} position={:?} start={} deleted_chars={} \
                 inserted_chars={}",
                record.keys_complete,
                record.keys.len(),
                record.change.position_evidence,
                record.change.start,
                record.change.deleted.chars().count(),
                record.change.inserted.chars().count()
            );
        }
    }

    fn print_candidate(candidate: &CorrectionCandidate, preview_text: bool) {
        println!("{}", format_candidate(candidate, preview_text));
    }

    fn format_candidate(candidate: &CorrectionCandidate, preview_text: bool) -> String {
        if preview_text {
            format!(
                "CORRECTION_CANDIDATE form={:?} kind={:?} source_commit_sequence={:?} \
                 deletion_sequence={} replacement_sequence={} gap_ms={} start={} \
                 deleted={:?} inserted={:?} keys_complete={} deletion_keys={:?} \
                 replacement_keys={:?} deletion_position={:?} replacement_position={:?} \
                 replacement_composition={:?}",
                candidate.form,
                candidate.kind,
                candidate.source_commit_sequence,
                candidate.deletion_sequence,
                candidate.replacement_sequence,
                candidate.gap_ms(),
                candidate.start,
                candidate.deleted,
                candidate.inserted,
                candidate.keys_complete,
                candidate.deletion_keys,
                candidate.replacement_keys,
                candidate.deletion_position_evidence,
                candidate.replacement_position_evidence,
                candidate.replacement_composition
            )
        } else {
            format!(
                "CORRECTION_CANDIDATE form={:?} kind={:?} source_commit_sequence={:?} \
                 deletion_sequence={} replacement_sequence={} gap_ms={} start={} \
                 deleted_chars={} inserted_chars={} keys_complete={} deletion_keys={} \
                 replacement_keys={} deletion_position={:?} replacement_position={:?} \
                 replacement_composition_chars={}",
                candidate.form,
                candidate.kind,
                candidate.source_commit_sequence,
                candidate.deletion_sequence,
                candidate.replacement_sequence,
                candidate.gap_ms(),
                candidate.start,
                candidate.deleted.chars().count(),
                candidate.inserted.chars().count(),
                candidate.keys_complete,
                candidate.deletion_keys.len(),
                candidate.replacement_keys.len(),
                candidate.deletion_position_evidence,
                candidate.replacement_position_evidence,
                candidate
                    .replacement_composition
                    .as_ref()
                    .map_or(0, |composition| composition.chars().count())
            )
        }
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
            CandidatePreviewState, PrivateCapsuleState, TARGET_FIND_ATTEMPTS,
            TargetDiscoveryDecision, VK_SHIFT, format_candidate, key_capture_allowed, map_key,
            parse_options_from, resolve_capsule_target, resolve_summary_target,
            save_private_file_create_new, target_discovery_decision, unique_value_offset,
        };
        use std::fs;
        use std::path::Path;
        use std::sync::atomic::{AtomicU64, Ordering};
        use ziranma_core::{
            CommitRecord, CorrectionCandidate, CorrectionCandidateForm, CorrectionCandidateKind,
            DeltaPositionEvidence, RawKey, RevisionRecord, SessionSummaryCounts, SessionSummaryV1,
            TextDelta, TrackerOutput,
        };

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn key_capture_allows_candidate_focus_only_during_an_active_composition() {
            assert!(!key_capture_allowed(false, true, false, true));
            assert!(key_capture_allowed(true, true, false, true));
            assert!(key_capture_allowed(true, false, true, true));
            assert!(!key_capture_allowed(true, false, false, true));
            assert!(!key_capture_allowed(true, true, true, false));
        }

        #[test]
        fn target_discovery_retries_only_zero_matches_and_never_ambiguity() {
            assert_eq!(
                target_discovery_decision(1, 1),
                TargetDiscoveryDecision::Accept
            );
            assert_eq!(
                target_discovery_decision(0, 1),
                TargetDiscoveryDecision::Retry
            );
            assert_eq!(
                target_discovery_decision(0, TARGET_FIND_ATTEMPTS - 1),
                TargetDiscoveryDecision::Retry
            );
            assert_eq!(
                target_discovery_decision(0, TARGET_FIND_ATTEMPTS),
                TargetDiscoveryDecision::Refuse
            );
            assert_eq!(
                target_discovery_decision(2, 1),
                TargetDiscoveryDecision::Refuse
            );
        }

        #[test]
        fn value_offset_accepts_one_exact_or_wrapped_document_match() {
            assert_eq!(unique_value_offset("猫猫", "猫猫"), Some(0));
            assert_eq!(unique_value_offset("\n猫猫", "猫猫"), Some(1));
            assert_eq!(unique_value_offset("猫猫猫猫", "猫猫"), None);
            assert_eq!(unique_value_offset("随心输入", ""), None);
        }

        #[test]
        fn shifted_allowed_key_retains_its_modifier() {
            assert_eq!(
                map_key(0x25, true),
                Some(RawKey::Shift(Box::new(RawKey::Left)))
            );
            assert_eq!(map_key(0x25, false), Some(RawKey::Left));
            assert_eq!(map_key(VK_SHIFT.0 as u32, false), None);
        }

        #[test]
        fn candidate_preview_requires_an_explicit_positive_gap() {
            let options = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--preview-candidates",
                "--candidate-gap-ms",
                "5000",
            ]))
            .expect("valid options")
            .expect("not help");
            assert_eq!(options.candidate_gap_ms, Some(5_000));

            let missing_gap = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--preview-candidates",
            ]))
            .expect_err("gap must be explicit");
            assert_eq!(
                missing_gap.to_string(),
                "--preview-candidates requires --candidate-gap-ms"
            );

            let zero_gap = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--preview-candidates",
                "--candidate-gap-ms",
                "0",
            ]))
            .expect_err("zero gap is not a useful session boundary");
            assert_eq!(
                zero_gap.to_string(),
                "--candidate-gap-ms must be greater than zero"
            );
        }

        #[test]
        fn candidate_gap_cannot_silently_enable_preview_or_check_mode() {
            let missing_preview = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--candidate-gap-ms",
                "5000",
            ]))
            .expect_err("candidate preview must be explicit");
            assert_eq!(
                missing_preview.to_string(),
                "--candidate-gap-ms requires --preview-candidates"
            );

            let check_mode = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--check",
                "--preview-candidates",
                "--candidate-gap-ms",
                "5000",
            ]))
            .expect_err("check mode must not install candidate listeners");
            assert_eq!(
                check_mode.to_string(),
                "--check cannot be combined with capture, preview, or export"
            );

            let summary_without_preview = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--save-summary",
                "data/private/session-summaries/run.json",
            ]))
            .expect_err("summary export cannot silently enable candidate tracking");
            assert_eq!(
                summary_without_preview.to_string(),
                "--save-summary requires --preview-candidates"
            );
        }

        #[test]
        fn summary_target_is_restricted_to_one_new_json_in_the_private_directory() {
            let manifest = Path::new(r"D:\repo");
            let root = manifest.join("data/private/session-summaries");
            assert_eq!(
                resolve_summary_target(
                    manifest,
                    &root,
                    Path::new("data/private/session-summaries/run-001.json"),
                )
                .unwrap(),
                root.join("run-001.json")
            );

            for invalid in [
                "run.json",
                "data/private/run.json",
                "data/private/session-summaries/nested/run.json",
                "data/private/session-summaries/run.txt",
                "data/private/session-summaries/.hidden.json",
            ] {
                assert!(
                    resolve_summary_target(manifest, &root, Path::new(invalid)).is_err(),
                    "{invalid} must be rejected"
                );
            }
        }

        #[test]
        fn private_capsule_requires_key_capture_and_separate_plaintext_acknowledgement() {
            let path = "data/private/event-capsules/run-001.zic";
            let missing_key_capture = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--save-capsule",
                path,
                "--allow-private-plaintext",
            ]))
            .expect_err("capsules must contain explicitly scoped keys");
            assert_eq!(
                missing_key_capture.to_string(),
                "--save-capsule requires --capture-keys"
            );

            let missing_acknowledgement = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--capture-keys",
                "--save-capsule",
                path,
            ]))
            .expect_err("private plaintext must never be enabled implicitly");
            assert_eq!(
                missing_acknowledgement.to_string(),
                "--save-capsule requires the separate --allow-private-plaintext acknowledgement"
            );

            let options = parse_options_from(arguments(&[
                "--pid",
                "4242",
                "--arm",
                "--capture-keys",
                "--save-capsule",
                path,
                "--allow-private-plaintext",
            ]))
            .unwrap()
            .unwrap();
            assert_eq!(options.save_capsule.as_deref(), Some(Path::new(path)));
        }

        #[test]
        fn capsule_target_is_restricted_to_one_private_zic_file() {
            let manifest = Path::new(r"D:\repo");
            let root = manifest.join("data/private/event-capsules");
            assert_eq!(
                resolve_capsule_target(
                    manifest,
                    &root,
                    Path::new("data/private/event-capsules/run-001.zic"),
                )
                .unwrap(),
                root.join("run-001.zic")
            );

            for invalid in [
                "run.zic",
                "data/private/run.zic",
                "data/private/event-capsules/nested/run.zic",
                "data/private/event-capsules/run.json",
                "data/private/event-capsules/.hidden.zic",
            ] {
                assert!(
                    resolve_capsule_target(manifest, &root, Path::new(invalid)).is_err(),
                    "{invalid} must be rejected"
                );
            }
        }

        #[test]
        fn redacted_candidate_output_never_contains_text_or_key_values() {
            let candidate = CorrectionCandidate {
                source_commit_sequence: None,
                deletion_sequence: 0,
                replacement_sequence: 0,
                deletion_elapsed_ms: 10,
                replacement_elapsed_ms: 10,
                start: 1,
                deleted: "私密旧字".to_owned(),
                inserted: "私密新字".to_owned(),
                deletion_keys: Vec::new(),
                replacement_keys: vec![RawKey::Letter('z'), RawKey::Space],
                keys_complete: true,
                deletion_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_composition: Some("simi".to_owned()),
                kind: CorrectionCandidateKind::ReplacedWithDifferentText,
                form: CorrectionCandidateForm::DirectReplacement,
            };

            let redacted = format_candidate(&candidate, false);
            assert!(!redacted.contains("私密"));
            assert!(!redacted.contains("Letter"));
            assert!(!redacted.contains("simi"));
            assert!(redacted.contains("deleted_chars=4"));
            assert!(redacted.contains("replacement_keys=2"));

            let preview = format_candidate(&candidate, true);
            assert!(preview.contains("私密旧字"));
            assert!(preview.contains("Letter('z')"));
            assert!(preview.contains("simi"));
        }

        #[test]
        fn session_summary_counts_atomic_evidence_and_separates_delete_then_gaps() {
            let mut summary = SessionSummaryCounts::default();
            summary.observe_output(&TrackerOutput::Commit(CommitRecord {
                keys: vec![
                    RawKey::Letter('z'),
                    RawKey::Letter('k'),
                    RawKey::Backspace,
                    RawKey::Letter('l'),
                    RawKey::Space,
                ],
                keys_complete: true,
                composition: "zai".to_owned(),
                change: TextDelta {
                    start: 1,
                    deleted: "zai".to_owned(),
                    inserted: "在".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
                document_change: TextDelta {
                    start: 1,
                    deleted: "错".to_owned(),
                    inserted: "在".to_owned(),
                    position_evidence: DeltaPositionEvidence::UniqueText,
                },
            }));
            summary.observe_output(&TrackerOutput::Revision(RevisionRecord {
                keys: vec![RawKey::Backspace],
                keys_complete: false,
                change: TextDelta {
                    start: 1,
                    deleted: "在".to_owned(),
                    inserted: String::new(),
                    position_evidence: DeltaPositionEvidence::Ambiguous,
                },
            }));

            summary.observe_candidate(&CorrectionCandidate {
                source_commit_sequence: Some(0),
                deletion_sequence: 1,
                replacement_sequence: 1,
                deletion_elapsed_ms: 100,
                replacement_elapsed_ms: 100,
                start: 1,
                deleted: "错".to_owned(),
                inserted: "在".to_owned(),
                deletion_keys: Vec::new(),
                replacement_keys: vec![RawKey::Letter('z'), RawKey::Space],
                keys_complete: true,
                deletion_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_composition: Some("zai".to_owned()),
                kind: CorrectionCandidateKind::ReplacedWithDifferentText,
                form: CorrectionCandidateForm::DirectReplacement,
            });
            summary.observe_candidate(&CorrectionCandidate {
                source_commit_sequence: Some(1),
                deletion_sequence: 2,
                replacement_sequence: 3,
                deletion_elapsed_ms: 200,
                replacement_elapsed_ms: 2_418,
                start: 1,
                deleted: "在".to_owned(),
                inserted: "在".to_owned(),
                deletion_keys: vec![RawKey::Backspace],
                replacement_keys: vec![RawKey::Letter('z'), RawKey::Space],
                keys_complete: true,
                deletion_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_position_evidence: DeltaPositionEvidence::UniqueText,
                replacement_composition: Some("zai".to_owned()),
                kind: CorrectionCandidateKind::RestoredSameText,
                form: CorrectionCandidateForm::DeleteThenInsert,
            });

            let formatted = SessionSummaryV1 {
                candidate_gap_limit_ms: 15_000,
                elapsed_ms: 5_000,
                key_capture_requested: true,
                key_capture_ready: true,
                counts: summary,
            }
            .terminal_line();
            assert_eq!(
                formatted,
                "SESSION_SUMMARY elapsed_ms=5000 candidate_gap_limit_ms=15000 \
                 key_capture_requested=true key_capture_ready=true commits=1 revisions=1 \
                 keys_complete_records=1 keys_incomplete_records=1 \
                 logical_key_actions=6 commits_with_internal_edit_keys=1 \
                 ambiguous_document_positions=1 direct_replacements=1 \
                 delete_then_insertions=1 restored_same_text=1 \
                 replaced_with_different_text=1 source_linked_candidates=2 \
                 delete_then_gap_count=1 delete_then_gap_min_ms=2218 \
                 delete_then_gap_max_ms=2218 delete_then_gap_mean_ms=2218 \
                 delete_then_gap_total_ms=2218"
            );
            assert!(!formatted.contains('错'));
            assert!(!formatted.contains('在'));
            assert!(!formatted.contains("zai"));
        }

        #[test]
        fn summary_json_is_redacted_deterministic_and_saved_without_overwrite() {
            let summary = SessionSummaryCounts {
                commits: 2,
                revisions: 1,
                keys_complete_records: 3,
                logical_key_actions: 7,
                delete_then_insertions: 1,
                restored_same_text: 1,
                source_linked_candidates: 1,
                delete_then_gap_count: 1,
                delete_then_gap_min_ms: Some(999),
                delete_then_gap_max_ms: Some(999),
                delete_then_gap_total_ms: 999,
                ..SessionSummaryCounts::default()
            };
            let report = SessionSummaryV1 {
                candidate_gap_limit_ms: 15_000,
                elapsed_ms: 4_491,
                key_capture_requested: true,
                key_capture_ready: true,
                counts: summary,
            };
            let json = report.to_json().unwrap();
            assert!(
                json.starts_with(
                    "{\"schema\":\"ziranma-session-summary-v1\",\"contains_text\":false"
                )
            );
            assert!(json.contains("\"delete_then_gap_min_ms\":999"));
            assert!(!json.contains('猫'));
            assert!(!json.contains("mao"));
            assert!(!json.contains("Letter"));

            static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);
            let test_directory = std::env::temp_dir().join(format!(
                "ziranma-summary-save-test-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&test_directory).unwrap();
            let target = test_directory.join("summary.json");
            save_private_file_create_new(&target, &json).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), format!("{json}\n"));

            save_private_file_create_new(&target, "{}")
                .expect_err("an existing target must never be overwritten");
            assert_eq!(fs::read_to_string(&target).unwrap(), format!("{json}\n"));
            assert_eq!(fs::read_dir(&test_directory).unwrap().count(), 1);

            fs::remove_file(&target).unwrap();
            fs::remove_dir(&test_directory).unwrap();
        }

        #[test]
        fn ready_reset_clears_candidate_summary_and_sequence() {
            let mut preview = CandidatePreviewState::new(15_000);
            preview.summary.commits = 9;
            let first = preview
                .detector
                .observe(
                    10,
                    TrackerOutput::Commit(CommitRecord {
                        keys: vec![RawKey::Letter('z'), RawKey::Space],
                        keys_complete: true,
                        composition: "zai".to_owned(),
                        change: TextDelta {
                            start: 1,
                            deleted: "zai".to_owned(),
                            inserted: "在".to_owned(),
                            position_evidence: DeltaPositionEvidence::UniqueText,
                        },
                        document_change: TextDelta {
                            start: 1,
                            deleted: "错".to_owned(),
                            inserted: "在".to_owned(),
                            position_evidence: DeltaPositionEvidence::UniqueText,
                        },
                    }),
                )
                .unwrap()
                .unwrap();
            assert_eq!(first.replacement_sequence, 0);

            preview.reset();
            assert_eq!(preview.summary, SessionSummaryCounts::default());
            let after_reset = preview
                .detector
                .observe(
                    1,
                    TrackerOutput::Commit(CommitRecord {
                        keys: vec![RawKey::Letter('z'), RawKey::Space],
                        keys_complete: true,
                        composition: "zai".to_owned(),
                        change: TextDelta {
                            start: 1,
                            deleted: "zai".to_owned(),
                            inserted: "在".to_owned(),
                            position_evidence: DeltaPositionEvidence::UniqueText,
                        },
                        document_change: TextDelta {
                            start: 1,
                            deleted: "错".to_owned(),
                            inserted: "在".to_owned(),
                            position_evidence: DeltaPositionEvidence::UniqueText,
                        },
                    }),
                )
                .unwrap()
                .unwrap();
            assert_eq!(after_reset.replacement_sequence, 0);
        }

        #[test]
        fn ready_reset_discards_pre_ready_private_capsule_events() {
            fn commit(inserted: &str) -> TrackerOutput {
                TrackerOutput::Commit(CommitRecord {
                    keys: vec![RawKey::Letter('m'), RawKey::Letter('k'), RawKey::Space],
                    keys_complete: true,
                    composition: "mao".to_owned(),
                    change: TextDelta {
                        start: 0,
                        deleted: "mao".to_owned(),
                        inserted: inserted.to_owned(),
                        position_evidence: DeltaPositionEvidence::UniqueText,
                    },
                    document_change: TextDelta {
                        start: 0,
                        deleted: String::new(),
                        inserted: inserted.to_owned(),
                        position_evidence: DeltaPositionEvidence::UniqueText,
                    },
                })
            }

            let mut capsule = PrivateCapsuleState::new();
            capsule.observe(commit("旧"));
            assert_eq!(capsule.recorder.finish().unwrap().events().len(), 1);
            capsule.reset();
            assert!(capsule.recorder.finish().is_err());
            capsule.observe(commit("猫"));
            let saved = capsule.recorder.finish().unwrap();
            assert_eq!(saved.events().len(), 1);
            let TrackerOutput::Commit(record) = &saved.events()[0].output else {
                panic!("expected commit");
            };
            assert_eq!(record.change.inserted, "猫");
        }
    }
}
