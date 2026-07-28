//! Build-only Windows TSF COM lifecycle probe.
//!
//! This module intentionally exports no registration functions. It proves
//! class-factory, activation, deactivation, server-lock, and unload behavior
//! without adding an input profile to Windows.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{CompositionEffect, CompositionInput, CompositionSession};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, E_UNEXPECTED, LPARAM, S_FALSE,
    S_OK, WPARAM,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_0, VK_9, VK_A, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_NEXT,
    VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_Z,
};
use windows::Win32::UI::TextServices::{
    ITfContext, ITfEditSession, ITfEditSession_Impl, ITfKeyEventSink, ITfKeyEventSink_Impl,
    ITfKeystrokeMgr, ITfTextInputProcessor_Impl, ITfTextInputProcessorEx,
    ITfTextInputProcessorEx_Impl, ITfThreadMgr, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_READWRITE,
    TF_ES_SYNC,
};
use windows::core::{
    Error, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result, implement,
};

/// Fixed COM class identity reserved for the local TSF alpha.
pub const TSF_ALPHA_CLSID: GUID = GUID::from_u128(0x4cc8427b_d0f5_439e_b6af_d45eacd7e577);

static ACTIVE_COM_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static SERVER_LOCKS: AtomicUsize = AtomicUsize::new(0);

trait CandidateProvider: Send + Sync {
    /// Returns one deterministic, one-based candidate without learning or I/O.
    fn candidate(&self, code: &str, rank: usize) -> Option<String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentEditKind {
    UpdatePreedit,
    Cancel,
    Commit,
}

#[derive(Clone)]
enum PendingDocumentEdit {
    UpdatePreedit(String),
    Cancel,
    Commit(String),
}

impl PendingDocumentEdit {
    fn kind(&self) -> DocumentEditKind {
        match self {
            Self::UpdatePreedit(_) => DocumentEditKind::UpdatePreedit,
            Self::Cancel => DocumentEditKind::Cancel,
            Self::Commit(_) => DocumentEditKind::Commit,
        }
    }

    fn transient_text(&self) -> Option<&str> {
        match self {
            Self::UpdatePreedit(text) | Self::Commit(text) => Some(text),
            Self::Cancel => None,
        }
    }
}

#[derive(Default)]
struct EditSessionTelemetry {
    completed: usize,
    last_kind: Option<DocumentEditKind>,
}

struct PlannedKey {
    before: CompositionSession,
    after: CompositionSession,
    edit: PendingDocumentEdit,
}

#[derive(Clone, Copy, Default)]
struct KeyModifiers {
    shift: bool,
    control: bool,
    alt: bool,
    windows: bool,
}

fn current_key_modifiers() -> KeyModifiers {
    fn down(key: u16) -> bool {
        // SAFETY: GetKeyState accepts any virtual-key value and has no pointer
        // preconditions. It observes only the calling thread's key state.
        unsafe { GetKeyState(i32::from(key)) < 0 }
    }

    KeyModifiers {
        shift: down(VK_SHIFT.0),
        control: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        windows: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}

fn decode_virtual_key(vkey: u16, modifiers: KeyModifiers) -> Option<CompositionInput> {
    if modifiers.control || modifiers.alt || modifiers.windows {
        return None;
    }
    if vkey == VK_TAB.0 && modifiers.shift {
        return Some(CompositionInput::EnterRecovery);
    }
    match vkey {
        key if key == VK_BACK.0 => Some(CompositionInput::Backspace),
        key if key == VK_TAB.0 => Some(CompositionInput::EnterTab),
        key if key == VK_RETURN.0 || key == VK_SPACE.0 => Some(CompositionInput::Confirm),
        key if key == VK_ESCAPE.0 => Some(CompositionInput::Escape),
        key if key == VK_PRIOR.0 || key == VK_OEM_MINUS.0 => Some(CompositionInput::PreviousPage),
        key if key == VK_NEXT.0 || key == VK_OEM_PLUS.0 => Some(CompositionInput::NextPage),
        key if (VK_A.0..=VK_Z.0).contains(&key) => Some(CompositionInput::Letters(
            char::from(b'a' + u8::try_from(key - VK_A.0).expect("A-Z offset fits u8")).to_string(),
        )),
        key if (VK_0.0..=VK_9.0).contains(&key) => {
            let digit = usize::from(key - VK_0.0);
            Some(CompositionInput::Select(if digit == 0 {
                10
            } else {
                digit
            }))
        }
        _ => None,
    }
}

fn plan_session_input(
    session: &CompositionSession,
    input: CompositionInput,
    selected_text: Option<String>,
) -> Option<PlannedKey> {
    let before = session.clone();
    let mut after = before.clone();
    let effect = after.apply(input.clone());
    let edit = match (input, effect) {
        (CompositionInput::Letters(_), CompositionEffect::Continue) => {
            PendingDocumentEdit::UpdatePreedit(after.phonetic().to_owned())
        }
        (CompositionInput::Backspace | CompositionInput::Escape, CompositionEffect::Continue)
            if before.phonetic() != after.phonetic() && after.phonetic().is_empty() =>
        {
            PendingDocumentEdit::Cancel
        }
        (CompositionInput::Backspace | CompositionInput::Escape, CompositionEffect::Continue)
            if before.phonetic() != after.phonetic() =>
        {
            PendingDocumentEdit::UpdatePreedit(after.phonetic().to_owned())
        }
        (CompositionInput::Confirm, CompositionEffect::Confirm)
        | (CompositionInput::Select(_), CompositionEffect::Select(_)) => {
            let text = selected_text.filter(|text| !text.is_empty())?;
            after.finish_commit();
            PendingDocumentEdit::Commit(text)
        }
        _ => return None,
    };
    Some(PlannedKey {
        before,
        after,
        edit,
    })
}

fn lifecycle_error(code: HRESULT) -> Error {
    Error::from_hresult(code)
}

fn object_created() {
    ACTIVE_COM_OBJECTS.fetch_add(1, Ordering::AcqRel);
}

fn object_dropped() {
    let previous = ACTIVE_COM_OBJECTS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0, "TSF alpha COM object counter underflow");
}

fn can_unload_now() -> bool {
    ACTIVE_COM_OBJECTS.load(Ordering::Acquire) == 0 && SERVER_LOCKS.load(Ordering::Acquire) == 0
}

#[implement(IClassFactory)]
struct TsfClassFactory;

impl TsfClassFactory {
    fn counted() -> Self {
        object_created();
        Self
    }
}

impl Drop for TsfClassFactory {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl IClassFactory_Impl for TsfClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if !ppvobject.is_null() {
            // SAFETY: the caller supplied writable COM output storage.
            unsafe { *ppvobject = ptr::null_mut() };
        }
        if !punkouter.is_null() {
            return Err(lifecycle_error(CLASS_E_NOAGGREGATION));
        }
        if riid.is_null() || ppvobject.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }

        let service: ITfTextInputProcessorEx = TsfTextService::counted().into();
        // SAFETY: `riid` and `ppvobject` were validated above. QueryInterface
        // owns the returned reference on success; dropping `service` releases
        // only the constructor's reference.
        unsafe { service.query(riid, ppvobject) }.ok()
    }

    fn LockServer(&self, flock: windows::core::BOOL) -> Result<()> {
        if flock.as_bool() {
            SERVER_LOCKS.fetch_add(1, Ordering::AcqRel);
            return Ok(());
        }
        SERVER_LOCKS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |locks| {
                locks.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| lifecycle_error(E_UNEXPECTED))
    }
}

/// A synchronous TSF edit-session boundary.
///
/// This build-only phase deliberately records only bounded structural
/// telemetry. Actual ITfRange mutation is the next gate; transient text stays
/// inside this object and is dropped as soon as the session completes.
#[implement(ITfEditSession)]
struct TsfEditSessionProbe {
    action: PendingDocumentEdit,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
}

impl TsfEditSessionProbe {
    fn counted(action: PendingDocumentEdit, telemetry: Arc<Mutex<EditSessionTelemetry>>) -> Self {
        object_created();
        Self { action, telemetry }
    }
}

impl Drop for TsfEditSessionProbe {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl ITfEditSession_Impl for TsfEditSessionProbe_Impl {
    fn DoEditSession(&self, _ec: u32) -> Result<()> {
        let _transient_text = self.action.transient_text();
        let mut telemetry = self
            .telemetry
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        telemetry.completed = telemetry.completed.saturating_add(1);
        telemetry.last_kind = Some(self.action.kind());
        Ok(())
    }
}

fn request_probe_edit_session(
    context: &ITfContext,
    client_id: u32,
    action: PendingDocumentEdit,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
) -> Result<()> {
    let edit_session: ITfEditSession = TsfEditSessionProbe::counted(action, telemetry).into();
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
    // SAFETY: keystroke callbacks are one of the documented situations where
    // a synchronous read/write edit session can be requested. Both COM
    // interfaces remain owned for the duration of this call.
    let session_result = unsafe { context.RequestEditSession(client_id, &edit_session, flags) }?;
    session_result.ok()
}

#[derive(Default)]
struct ActivationState {
    thread_manager: Option<ITfThreadMgr>,
    keystroke_manager: Option<ITfKeystrokeMgr>,
    client_id: u32,
    flags: u32,
    activating: bool,
}

#[derive(Clone, Copy)]
enum KeyAdviceMode {
    Foreground,
    #[cfg(test)]
    DisabledForProcessTest,
}

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink)]
struct TsfTextService {
    activation: Mutex<ActivationState>,
    composition: Mutex<CompositionSession>,
    candidate_provider: Option<Arc<dyn CandidateProvider>>,
    edit_telemetry: Arc<Mutex<EditSessionTelemetry>>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfTextService {
    fn counted() -> Self {
        Self::counted_with_options(None, KeyAdviceMode::Foreground)
    }

    fn counted_with_options(
        candidate_provider: Option<Arc<dyn CandidateProvider>>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        Self {
            activation: Mutex::new(ActivationState::default()),
            composition: Mutex::new(CompositionSession::default()),
            candidate_provider,
            edit_telemetry: Arc::new(Mutex::new(EditSessionTelemetry::default())),
            key_advice_mode,
        }
    }

    #[cfg(test)]
    fn counted_for_process_test(candidate_provider: Option<Arc<dyn CandidateProvider>>) -> Self {
        Self::counted_with_options(candidate_provider, KeyAdviceMode::DisabledForProcessTest)
    }
}

impl Drop for TsfTextService {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl TsfTextService_Impl {
    fn observed_key_modifiers(&self) -> KeyModifiers {
        match self.key_advice_mode {
            KeyAdviceMode::Foreground => current_key_modifiers(),
            #[cfg(test)]
            KeyAdviceMode::DisabledForProcessTest => KeyModifiers::default(),
        }
    }

    fn activate_inner(
        &self,
        thread_manager: Ref<ITfThreadMgr>,
        client_id: u32,
        flags: u32,
    ) -> Result<()> {
        let Some(thread_manager) = thread_manager.cloned() else {
            return Err(lifecycle_error(E_POINTER));
        };
        {
            let mut activation = self
                .activation
                .lock()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            if activation.activating || activation.thread_manager.is_some() {
                return Err(lifecycle_error(E_UNEXPECTED));
            }
            activation.activating = true;
        }

        let setup = (|| match self.key_advice_mode {
            KeyAdviceMode::Foreground => {
                let keystroke_manager: ITfKeystrokeMgr = thread_manager.cast()?;
                let sink = self.to_interface::<ITfKeyEventSink>();
                // SAFETY: a real TSF activation supplies a text-service client
                // id and owns `sink` until the matching Unadvise call.
                unsafe { keystroke_manager.AdviseKeyEventSink(client_id, &sink, true) }?;
                Ok(Some(keystroke_manager))
            }
            #[cfg(test)]
            KeyAdviceMode::DisabledForProcessTest => Ok(None),
        })();

        let keystroke_manager = match setup {
            Ok(manager) => manager,
            Err(error) => {
                if let Ok(mut activation) = self.activation.lock() {
                    activation.activating = false;
                }
                return Err(error);
            }
        };

        let mut activation = match self.activation.lock() {
            Ok(activation) => activation,
            Err(_) => {
                // SAFETY: balances the successful AdviseKeyEventSink above.
                if let Some(keystroke_manager) = &keystroke_manager {
                    let _ = unsafe { keystroke_manager.UnadviseKeyEventSink(client_id) };
                }
                return Err(lifecycle_error(E_UNEXPECTED));
            }
        };
        activation.thread_manager = Some(thread_manager);
        activation.keystroke_manager = keystroke_manager;
        activation.client_id = client_id;
        activation.flags = flags;
        activation.activating = false;
        Ok(())
    }

    fn plan_key(&self, wparam: WPARAM, modifiers: KeyModifiers) -> Result<Option<PlannedKey>> {
        let Some(provider) = self.candidate_provider.as_ref() else {
            return Ok(None);
        };
        let Ok(vkey) = u16::try_from(wparam.0) else {
            return Ok(None);
        };
        let Some(input) = decode_virtual_key(vkey, modifiers) else {
            return Ok(None);
        };
        let session = self
            .composition
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .clone();
        let selected_text = match input {
            CompositionInput::Confirm => provider.candidate(session.phonetic(), 1),
            CompositionInput::Select(rank) => provider.candidate(session.phonetic(), rank),
            _ => None,
        };
        Ok(plan_session_input(&session, input, selected_text))
    }

    fn apply_key(&self, context: Ref<ITfContext>, wparam: WPARAM) -> Result<windows::core::BOOL> {
        let Some(context) = context.cloned() else {
            return Ok(false.into());
        };
        let Some(plan) = self.plan_key(wparam, self.observed_key_modifiers())? else {
            return Ok(false.into());
        };
        let client_id = {
            let activation = self
                .activation
                .lock()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            if activation.thread_manager.is_none() {
                return Ok(false.into());
            }
            activation.client_id
        };

        request_probe_edit_session(
            &context,
            client_id,
            plan.edit,
            Arc::clone(&self.edit_telemetry),
        )?;

        let mut composition = self
            .composition
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if *composition != plan.before {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        *composition = plan.after;
        Ok(true.into())
    }
}

impl ITfTextInputProcessor_Impl for TsfTextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        self.activate_inner(ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        let previous = {
            let mut activation = self
                .activation
                .lock()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            std::mem::take(&mut *activation)
        };
        let composition_result = match self.composition.lock() {
            Ok(mut composition) => {
                composition.finish_commit();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let unadvise_result = if let Some(keystroke_manager) = previous.keystroke_manager {
            // SAFETY: balances the successful advice owned by `previous`.
            unsafe { keystroke_manager.UnadviseKeyEventSink(previous.client_id) }
        } else {
            Ok(())
        };
        unadvise_result?;
        composition_result
    }
}

impl ITfTextInputProcessorEx_Impl for TsfTextService_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()> {
        self.activate_inner(ptim, tid, dwflags)
    }
}

impl ITfKeyEventSink_Impl for TsfTextService_Impl {
    fn OnSetFocus(&self, _fforeground: windows::core::BOOL) -> Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _context: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        if self
            .activation
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .thread_manager
            .is_none()
        {
            return Ok(false.into());
        }
        Ok(self
            .plan_key(wparam, self.observed_key_modifiers())?
            .is_some()
            .into())
    }

    fn OnTestKeyUp(
        &self,
        _context: Ref<ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        Ok(false.into())
    }

    fn OnKeyDown(
        &self,
        context: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        self.apply_key(context, wparam)
    }

    fn OnKeyUp(
        &self,
        _context: Ref<ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        Ok(false.into())
    }

    fn OnPreservedKey(
        &self,
        _context: Ref<ITfContext>,
        _rguid: *const GUID,
    ) -> Result<windows::core::BOOL> {
        Ok(false.into())
    }
}

/// Standard COM class-factory export. This function cannot register the DLL.
///
/// # Safety
///
/// The caller must supply COM ABI pointers. On success, `ppv` receives one
/// owned interface reference.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if !ppv.is_null() {
        // SAFETY: the caller supplied writable COM output storage.
        unsafe { *ppv = ptr::null_mut() };
    }
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // SAFETY: `rclsid` was checked for null and points to a GUID supplied by
    // the COM loader for the duration of this call.
    if unsafe { *rclsid } != TSF_ALPHA_CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    let factory: IClassFactory = TsfClassFactory::counted().into();
    // SAFETY: `riid` and `ppv` were validated above. QueryInterface transfers
    // one owned reference to `ppv` on success.
    unsafe { factory.query(riid, ppv) }
}

/// Standard COM unload query for the build-only TSF alpha.
#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if can_unload_now() { S_OK } else { S_FALSE }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::TextServices::{CLSID_TF_ThreadMgr, TF_POPF_ALL};
    use windows::core::ComObject;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct FixedCandidateProvider;

    impl CandidateProvider for FixedCandidateProvider {
        fn candidate(&self, code: &str, rank: usize) -> Option<String> {
            match (code, rank) {
                ("a", 1) => Some("啊".to_owned()),
                _ => None,
            }
        }
    }

    struct ComApartment;

    impl ComApartment {
        fn enter() -> Self {
            // SAFETY: the test owns its worker thread for the duration of the
            // guard and balances this call in Drop.
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .expect("COM apartment should initialize");
            Self
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            // SAFETY: balances the successful CoInitializeEx in `enter` on
            // the same test thread.
            unsafe { CoUninitialize() };
        }
    }

    fn class_factory() -> IClassFactory {
        let mut raw = ptr::null_mut();
        // SAFETY: all pointers reference stack storage valid for the call.
        let result = unsafe { DllGetClassObject(&TSF_ALPHA_CLSID, &IClassFactory::IID, &mut raw) };
        assert_eq!(result, S_OK);
        assert!(!raw.is_null());
        // SAFETY: DllGetClassObject returned one owned IClassFactory reference.
        unsafe { IClassFactory::from_raw(raw) }
    }

    #[test]
    fn class_factory_rejects_unknown_classes_without_leaking_an_object() {
        let _guard = test_lock();
        assert_eq!(DllCanUnloadNow(), S_OK);
        let unknown = GUID::from_u128(1);
        let mut raw = ptr::dangling_mut::<c_void>();
        // SAFETY: all pointers reference stack storage valid for the call.
        let result = unsafe { DllGetClassObject(&unknown, &IClassFactory::IID, &mut raw) };
        assert_eq!(result, CLASS_E_CLASSNOTAVAILABLE);
        assert!(raw.is_null());
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn class_factory_clears_output_before_rejecting_invalid_input() {
        let _guard = test_lock();
        let mut raw = ptr::dangling_mut::<c_void>();
        // SAFETY: `raw` is valid output storage; the null class pointer is the
        // failure condition under test.
        let result = unsafe { DllGetClassObject(ptr::null(), &IClassFactory::IID, &mut raw) };
        assert_eq!(result, E_POINTER);
        assert!(raw.is_null());
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn object_and_server_locks_control_the_unload_boundary() {
        let _guard = test_lock();
        assert_eq!(DllCanUnloadNow(), S_OK);
        let factory = class_factory();
        assert_eq!(DllCanUnloadNow(), S_FALSE);

        // SAFETY: the class factory is a valid local COM interface.
        unsafe { factory.LockServer(true) }.unwrap();
        // SAFETY: balances the preceding local LockServer call.
        unsafe { factory.LockServer(false) }.unwrap();
        drop(factory);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn unregistered_process_cannot_fake_foreground_text_service_activation() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let factory = class_factory();
        // SAFETY: aggregation is disabled and the requested interface is
        // implemented by the local text-service object.
        let service: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None::<&IUnknown>) }
            .expect("class factory should create the text service");
        drop(factory);
        assert_eq!(DllCanUnloadNow(), S_FALSE);

        // SAFETY: COM is initialized on this thread and the CLSID is the
        // system TSF thread manager.
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        // SAFETY: the real thread manager is used only on this initialized
        // apartment thread and is deactivated below.
        let client_id =
            unsafe { thread_manager.Activate() }.expect("TSF thread manager should activate");

        let key_sink: ITfKeyEventSink = service
            .cast()
            .expect("class-factory service should expose its key-event sink");
        drop(key_sink);

        // SAFETY: this deliberately supplies an application client id instead
        // of a client id created by registered TSF profile activation.
        let activation_error = unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect_err("an unregistered process must not become the foreground text service");
        assert_eq!(
            activation_error.code(),
            windows::Win32::Foundation::E_INVALIDARG
        );
        // A failed advice must clear the transient activation marker.
        let repeated_error = unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect_err("the same invalid manual activation should fail cleanly again");
        assert_eq!(
            repeated_error.code(),
            windows::Win32::Foundation::E_INVALIDARG
        );
        // SAFETY: deactivation is deliberately idempotent for cleanup paths.
        unsafe { service.Deactivate() }.expect("repeated cleanup should be harmless");
        // SAFETY: balances ITfThreadMgr::Activate above.
        unsafe { thread_manager.Deactivate() }.expect("thread manager should deactivate");

        drop(service);
        drop(thread_manager);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn process_test_routes_keys_through_real_synchronous_edit_sessions() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(FixedCandidateProvider),
        )));
        let service: ITfTextInputProcessorEx = service_object.to_interface();
        let key_sink: ITfKeyEventSink = service_object.to_interface();

        // SAFETY: COM is initialized on this test thread.
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        // SAFETY: balanced below on the same apartment thread.
        let client_id = unsafe { thread_manager.Activate() }.expect("thread manager activation");
        // SAFETY: this object uses the explicit process-test mode, which skips
        // foreground advice but exercises the same activation state.
        unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect("process-test activation should succeed");

        // A manager-owned empty context is enough to validate synchronous
        // RequestEditSession scheduling without implementing a fake text store.
        let document_manager =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("document manager creation");
        let mut context = None;
        let mut text_store_cookie = 0;
        // SAFETY: output pointers remain valid for this call; no external text
        // store is attached to this build-only context.
        unsafe {
            document_manager.CreateContext(
                client_id,
                0,
                None::<&IUnknown>,
                &mut context,
                &mut text_store_cookie,
            )
        }
        .expect("empty TSF context creation");
        let context = context.expect("CreateContext should return a context");
        // SAFETY: the context and document manager belong to this thread.
        unsafe { document_manager.Push(&context) }.expect("context push");
        // SAFETY: establishes the context used by the direct key-sink probe.
        unsafe { thread_manager.SetFocus(&document_manager) }.expect("document focus");

        let a = WPARAM(usize::from(VK_A.0));
        let space = WPARAM(usize::from(VK_SPACE.0));
        let b = WPARAM(usize::from(VK_A.0 + 1));
        let escape = WPARAM(usize::from(VK_ESCAPE.0));
        let lparam = LPARAM(0);

        // SAFETY: all interfaces belong to this apartment and the virtual-key
        // values contain no pointer data.
        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, a, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, a, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.composition.lock().unwrap().phonetic(), "a");
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 1);
            assert_eq!(telemetry.last_kind, Some(DocumentEditKind::UpdatePreedit));
        }

        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, space, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, space, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            service_object
                .composition
                .lock()
                .unwrap()
                .phonetic()
                .is_empty()
        );
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 2);
            assert_eq!(telemetry.last_kind, Some(DocumentEditKind::Commit));
        }

        assert!(
            !unsafe { key_sink.OnTestKeyDown(&context, space, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, b, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, escape, lparam) }
                .unwrap()
                .as_bool()
        );
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 4);
            assert_eq!(telemetry.last_kind, Some(DocumentEditKind::Cancel));
        }
        assert!(
            !unsafe { key_sink.OnKeyUp(&context, a, lparam) }
                .unwrap()
                .as_bool()
        );

        // SAFETY: cleanup is the exact reverse of the setup above.
        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(key_sink);
        drop(service);
        drop(context);
        drop(document_manager);
        drop(thread_manager);
        drop(service_object);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn virtual_key_decoder_preserves_host_shortcuts_and_explicit_controls() {
        assert_eq!(
            decode_virtual_key(VK_A.0, KeyModifiers::default()),
            Some(CompositionInput::Letters("a".to_owned()))
        );
        assert_eq!(
            decode_virtual_key(
                VK_A.0,
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                }
            ),
            None
        );
        assert_eq!(
            decode_virtual_key(
                VK_TAB.0,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                }
            ),
            Some(CompositionInput::EnterRecovery)
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_MINUS.0, KeyModifiers::default()),
            Some(CompositionInput::PreviousPage)
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_PLUS.0, KeyModifiers::default()),
            Some(CompositionInput::NextPage)
        );
    }
}
