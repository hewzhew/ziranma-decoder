//! Build-only Windows TSF COM and composition probe.
//!
//! This module intentionally exports no registration functions. It proves
//! class-factory, activation, deactivation, server-lock, and unload behavior
//! without adding an input profile to Windows.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{CompositionEffect, CompositionInput, CompositionSession, Decoder, parse_lexicon_tsv};
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
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfSource, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, TF_AE_NONE, TF_ANCHOR_END, TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    TF_ES_ASYNC, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_NO_DEFAULT_COMPOSITION, TF_SELECTION,
    TF_SELECTIONSTYLE,
};
use windows::core::{
    Error, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, Ref, Result, implement,
};

/// Fixed COM class identity reserved for the local TSF alpha.
pub const TSF_ALPHA_CLSID: GUID = GUID::from_u128(0x4cc8427b_d0f5_439e_b6af_d45eacd7e577);
/// Fixed Simplified Chinese language-profile identity reserved for the alpha.
pub const TSF_ALPHA_PROFILE_GUID: GUID = GUID::from_u128(0x8099d3f8_9f40_4da5_9b01_c12de0cd6370);
/// Simplified Chinese (zh-CN) language identifier used by the alpha profile.
pub const TSF_ALPHA_LANGID: u16 = 0x0804;

static ACTIVE_COM_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static SERVER_LOCKS: AtomicUsize = AtomicUsize::new(0);

// This deliberately small, manually constructed public fixture is only a
// bridge between the COM lifecycle and the real decoder. Loading the complete
// Rime snapshot inside every host process is a separate data-layer decision.
const TSF_DEVELOPMENT_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");

trait CandidateProvider: Send + Sync {
    /// Returns one deterministic, one-based candidate without learning or I/O.
    fn candidate(&self, code: &str, rank: usize) -> Option<String>;
}

struct DevelopmentCandidateProvider {
    decoder: Decoder,
}

impl CandidateProvider for DevelopmentCandidateProvider {
    fn candidate(&self, code: &str, rank: usize) -> Option<String> {
        if !(1..=10).contains(&rank) {
            return None;
        }
        let candidates = self.decoder.decode_sentence(code, rank).ok()?;
        let candidate = candidates.get(rank - 1)?;
        if candidate.unresolved_key_count == 0 {
            return Some(candidate.text.clone());
        }
        // The development lexicon is intentionally tiny. Confirming an
        // unknown first candidate must preserve the user's raw composition
        // instead of inserting the decoder's visible unresolved markers.
        (rank == 1).then(|| code.to_owned())
    }
}

fn development_candidate_provider() -> Arc<dyn CandidateProvider> {
    static PROVIDER: OnceLock<Arc<dyn CandidateProvider>> = OnceLock::new();
    Arc::clone(PROVIDER.get_or_init(|| {
        let entries = parse_lexicon_tsv(TSF_DEVELOPMENT_LEXICON)
            .expect("the checked-in TSF development lexicon must remain valid");
        Arc::new(DevelopmentCandidateProvider {
            decoder: Decoder::new(entries),
        })
    }))
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

fn same_com_identity<L: Interface, R: Interface>(left: &L, right: &R) -> Result<bool> {
    let left_identity: IUnknown = left.cast()?;
    let right_identity: IUnknown = right.cast()?;
    Ok(left_identity.as_raw() == right_identity.as_raw())
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
struct TsfClassFactory {
    candidate_provider: Arc<dyn CandidateProvider>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfClassFactory {
    fn counted() -> Self {
        Self::counted_with_options(development_candidate_provider(), KeyAdviceMode::Foreground)
    }

    fn counted_with_options(
        candidate_provider: Arc<dyn CandidateProvider>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        Self {
            candidate_provider,
            key_advice_mode,
        }
    }

    #[cfg(test)]
    fn counted_for_process_test() -> Self {
        Self::counted_with_options(
            development_candidate_provider(),
            KeyAdviceMode::DisabledForProcessTest,
        )
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

        let service: ITfTextInputProcessorEx = TsfTextService::counted_with_options(
            Some(Arc::clone(&self.candidate_provider)),
            self.key_advice_mode,
        )
        .into();
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

#[derive(Clone)]
struct ActiveDocumentComposition {
    context: ITfContext,
    composition: ITfComposition,
    range: ITfRange,
}

#[derive(Default)]
struct DocumentCompositionState {
    active: Option<ActiveDocumentComposition>,
    cleanup_scheduled: bool,
}

impl DocumentCompositionState {
    fn active_for_context(
        &self,
        context: &ITfContext,
    ) -> Result<Option<ActiveDocumentComposition>> {
        let Some(active) = self.active.as_ref() else {
            return Ok(None);
        };
        if active.context.as_raw() != context.as_raw() {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        Ok(Some(active.clone()))
    }
}

fn move_selection_after_range(context: &ITfContext, range: &ITfRange, ec: u32) -> Result<()> {
    let caret = range.clone();
    // SAFETY: this clone is an independent range owned by the context;
    // collapsing it does not mutate document text.
    unsafe { caret.Collapse(ec, TF_ANCHOR_END) }?;
    let mut selection = TF_SELECTION {
        range: std::mem::ManuallyDrop::new(Some(caret)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: false.into(),
        },
    };
    // SAFETY: the selection range belongs to this context and remains alive
    // through the call. TSF copies the selection synchronously.
    let result = unsafe { context.SetSelection(ec, std::slice::from_ref(&selection)) };
    // SAFETY: TF_SELECTION is an ABI struct whose interface field is
    // ManuallyDrop; release our cloned range exactly once after the call.
    unsafe { std::mem::ManuallyDrop::drop(&mut selection.range) };
    result
}

/// Receives host-driven termination without keeping the service state alive.
#[implement(ITfCompositionSink)]
struct TsfCompositionSink {
    document_composition: Weak<RefCell<DocumentCompositionState>>,
    logical_composition: Weak<RefCell<CompositionSession>>,
}

impl TsfCompositionSink {
    fn counted(
        document_composition: Weak<RefCell<DocumentCompositionState>>,
        logical_composition: Weak<RefCell<CompositionSession>>,
    ) -> Self {
        object_created();
        Self {
            document_composition,
            logical_composition,
        }
    }
}

impl Drop for TsfCompositionSink {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl ITfCompositionSink_Impl for TsfCompositionSink_Impl {
    fn OnCompositionTerminated(
        &self,
        ecwrite: u32,
        composition: Ref<ITfComposition>,
    ) -> Result<()> {
        let Some(composition) = composition.cloned() else {
            return Err(lifecycle_error(E_POINTER));
        };
        let Some(document_composition) = self.document_composition.upgrade() else {
            return Ok(());
        };
        let mut state = document_composition
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        let terminated_active = state
            .active
            .as_ref()
            .map(|active| same_com_identity(&active.composition, &composition))
            .transpose()?
            .unwrap_or(false);
        let active = if terminated_active {
            state.cleanup_scheduled = false;
            state.active.take()
        } else {
            None
        };
        drop(state);
        let Some(active) = active else {
            return Ok(());
        };
        // The context owner terminated uncommitted preedit. Use the supplied
        // write cookie to erase it instead of leaving raw phonetic text behind.
        // SAFETY: OnCompositionTerminated supplies write access for this range.
        let text_result = unsafe { active.range.SetText(ecwrite, 0, &[]) };
        let selection_result = if text_result.is_ok() {
            move_selection_after_range(&active.context, &active.range, ecwrite)
        } else {
            Ok(())
        };
        let logical_result = if let Some(logical_composition) = self.logical_composition.upgrade() {
            logical_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                .finish_commit();
            Ok(())
        } else {
            Ok(())
        };
        text_result?;
        selection_result?;
        logical_result
    }
}

/// Applies one planned composition change inside a synchronous TSF edit session.
#[implement(ITfEditSession)]
struct TsfDocumentEditSession {
    context: ITfContext,
    action: PendingDocumentEdit,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
}

impl TsfDocumentEditSession {
    fn counted(
        context: ITfContext,
        action: PendingDocumentEdit,
        document_composition: Rc<RefCell<DocumentCompositionState>>,
        logical_composition: Rc<RefCell<CompositionSession>>,
        telemetry: Arc<Mutex<EditSessionTelemetry>>,
        mode: EditSessionMode,
        cleanup_target: Option<ITfComposition>,
    ) -> Self {
        object_created();
        Self {
            context,
            action,
            document_composition,
            logical_composition,
            telemetry,
            mode,
            cleanup_target,
        }
    }

    fn active_composition(&self) -> Result<Option<ActiveDocumentComposition>> {
        self.document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .active_for_context(&self.context)
    }

    fn cleanup_target_is_current(&self) -> Result<bool> {
        let Some(target) = self.cleanup_target.as_ref() else {
            return Ok(false);
        };
        self.active_composition()?
            .map(|active| same_com_identity(&active.composition, target))
            .transpose()
            .map(|matches| matches.unwrap_or(false))
    }

    fn start_composition(&self, ec: u32, text: &str) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let insertion: ITfInsertAtSelection = self.context.cast()?;
        let context_composition: ITfContextComposition = self.context.cast()?;
        // SAFETY: `ec` is the read/write cookie issued for this synchronous
        // session. The returned range owns the newly inserted synthetic text.
        let range =
            unsafe { insertion.InsertTextAtSelection(ec, TF_IAS_NO_DEFAULT_COMPOSITION, &utf16) }?;
        let sink: ITfCompositionSink = TsfCompositionSink::counted(
            Rc::downgrade(&self.document_composition),
            Rc::downgrade(&self.logical_composition),
        )
        .into();
        // SAFETY: the same write cookie and inserted range remain valid. The
        // weak sink lets host-driven termination clear our active handle.
        let composition = match unsafe { context_composition.StartComposition(ec, &range, &sink) } {
            Ok(composition) => composition,
            Err(error) => {
                // SAFETY: roll back the insertion while the write cookie is
                // still valid if TSF refuses to create the composition.
                let _ = unsafe { range.SetText(ec, 0, &[]) };
                return Err(error);
            }
        };

        let mut state = self
            .document_composition
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if state.active.is_some() {
            drop(state);
            // SAFETY: this defensive race cleanup uses the same write cookie.
            let _ = unsafe { range.SetText(ec, 0, &[]) };
            // SAFETY: balances StartComposition above before returning.
            let _ = unsafe { composition.EndComposition(ec) };
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        state.active = Some(ActiveDocumentComposition {
            context: self.context.clone(),
            composition,
            range,
        });
        state.cleanup_scheduled = false;
        Ok(())
    }

    fn update_composition(
        &self,
        ec: u32,
        active: &ActiveDocumentComposition,
        text: &str,
    ) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the range belongs to the active composition in this context,
        // and `ec` grants synchronous write access for this call.
        unsafe { active.range.SetText(ec, 0, &utf16) }
    }

    fn finish_composition(&self, ec: u32, replacement: &str) -> Result<()> {
        let active = {
            let mut state = self
                .document_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            state.active_for_context(&self.context)?;
            state.cleanup_scheduled = false;
            state
                .active
                .take()
                .ok_or_else(|| lifecycle_error(E_UNEXPECTED))?
        };
        let utf16: Vec<u16> = replacement.encode_utf16().collect();
        // SAFETY: the active range and composition belong to this context and
        // `ec` is the current write cookie.
        if let Err(error) = unsafe { active.range.SetText(ec, 0, &utf16) } {
            self.restore_active(active)?;
            return Err(error);
        }
        if let Err(error) = move_selection_after_range(&self.context, &active.range, ec) {
            self.restore_active(active)?;
            return Err(error);
        }
        // SAFETY: successful completion balances StartComposition.
        if let Err(error) = unsafe { active.composition.EndComposition(ec) } {
            self.restore_active(active)?;
            return Err(error);
        }
        Ok(())
    }

    fn restore_active(&self, active: ActiveDocumentComposition) -> Result<()> {
        let mut state = self
            .document_composition
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if state.active.is_some() {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        state.active = Some(active);
        state.cleanup_scheduled = false;
        Ok(())
    }
}

impl Drop for TsfDocumentEditSession {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl ITfEditSession_Impl for TsfDocumentEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let mut cleanup_applied = false;
        match &self.action {
            PendingDocumentEdit::UpdatePreedit(text) => match self.active_composition()? {
                Some(active) => self.update_composition(ec, &active, text)?,
                None => self.start_composition(ec, text)?,
            },
            PendingDocumentEdit::Cancel if self.mode == EditSessionMode::CleanupAsync => {
                if self.cleanup_target_is_current()? {
                    self.finish_composition(ec, "")?;
                    cleanup_applied = true;
                }
            }
            PendingDocumentEdit::Cancel => self.finish_composition(ec, "")?,
            PendingDocumentEdit::Commit(text) => self.finish_composition(ec, text)?,
        }
        if cleanup_applied {
            self.logical_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                .finish_commit();
        }
        let mut telemetry = self
            .telemetry
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        telemetry.completed = telemetry.completed.saturating_add(1);
        telemetry.last_kind = Some(self.action.kind());
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum EditSessionMode {
    KeySynchronous,
    CleanupAsync,
}

#[derive(Default)]
struct ActivationState {
    thread_manager: Option<ITfThreadMgr>,
    keystroke_manager: Option<ITfKeystrokeMgr>,
    thread_source: Option<ITfSource>,
    thread_event_cookie: Option<u32>,
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

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
struct TsfTextService {
    activation: Mutex<ActivationState>,
    composition: Rc<RefCell<CompositionSession>>,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    candidate_provider: Option<Arc<dyn CandidateProvider>>,
    edit_telemetry: Arc<Mutex<EditSessionTelemetry>>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfTextService {
    fn counted_with_options(
        candidate_provider: Option<Arc<dyn CandidateProvider>>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        Self {
            activation: Mutex::new(ActivationState::default()),
            composition: Rc::new(RefCell::new(CompositionSession::default())),
            document_composition: Rc::new(RefCell::new(DocumentCompositionState::default())),
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

    fn request_document_edit_session(
        &self,
        context: &ITfContext,
        client_id: u32,
        action: PendingDocumentEdit,
        mode: EditSessionMode,
        cleanup_target: Option<ITfComposition>,
    ) -> Result<()> {
        let edit_session: ITfEditSession = TsfDocumentEditSession::counted(
            context.clone(),
            action,
            Rc::clone(&self.document_composition),
            Rc::clone(&self.composition),
            Arc::clone(&self.edit_telemetry),
            mode,
            cleanup_target,
        )
        .into();
        let scheduling = match mode {
            EditSessionMode::KeySynchronous => TF_ES_SYNC,
            EditSessionMode::CleanupAsync => TF_ES_ASYNC,
        };
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(scheduling.0 | TF_ES_READWRITE.0);
        // SAFETY: keystroke callbacks use the documented synchronous mode.
        // Focus cleanup is always queued so it cannot re-enter while TSF is
        // changing focus. TSF owns the edit-session reference after accepting
        // that work.
        let session_result =
            unsafe { context.RequestEditSession(client_id, &edit_session, flags) }?;
        session_result.ok()
    }

    fn active_client_id(&self) -> Result<Option<u32>> {
        let activation = self
            .activation
            .lock()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        Ok(activation
            .thread_manager
            .as_ref()
            .map(|_| activation.client_id))
    }

    fn active_document_has_focus(&self, focused: Ref<ITfDocumentMgr>) -> Result<bool> {
        let active_context = self
            .document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .active
            .as_ref()
            .map(|active| active.context.clone());
        let Some(active_context) = active_context else {
            return Ok(true);
        };
        let Some(focused) = focused.cloned() else {
            return Ok(false);
        };
        // SAFETY: the active context remains connected to its document while
        // its composition is tracked.
        let active_document = unsafe { active_context.GetDocumentMgr() }?;
        Ok(active_document.as_raw() == focused.as_raw())
    }

    fn schedule_active_composition_cleanup(&self, client_id: u32) -> Result<()> {
        let (context, cleanup_target) = {
            let mut state = self
                .document_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            if state.cleanup_scheduled {
                return Ok(());
            }
            let Some((context, composition)) = state
                .active
                .as_ref()
                .map(|active| (active.context.clone(), active.composition.clone()))
            else {
                return Ok(());
            };
            state.cleanup_scheduled = true;
            (context, composition)
        };

        let request = self.request_document_edit_session(
            &context,
            client_id,
            PendingDocumentEdit::Cancel,
            EditSessionMode::CleanupAsync,
            Some(cleanup_target),
        );
        if let Err(error) = request {
            let mut state = self
                .document_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.context.as_raw() == context.as_raw())
            {
                state.cleanup_scheduled = false;
            }
            return Err(error);
        }

        // An asynchronous cleanup may run after this callback returns. Stop
        // treating the old phonetic buffer as active as soon as TSF accepts it.
        if let Ok(mut composition) = self.composition.try_borrow_mut() {
            composition.finish_commit();
        }
        Ok(())
    }

    fn cleanup_after_focus_loss(&self) -> Result<()> {
        if let Some(client_id) = self.active_client_id()? {
            self.schedule_active_composition_cleanup(client_id)?;
        }
        Ok(())
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

        let setup = (|| {
            let thread_source: ITfSource = thread_manager.cast()?;
            let event_sink = self.to_interface::<ITfThreadMgrEventSink>();
            // SAFETY: the thread manager source owns the event-sink reference
            // until the matching UnadviseSink call.
            let thread_event_cookie =
                unsafe { thread_source.AdviseSink(&ITfThreadMgrEventSink::IID, &event_sink) }?;
            let key_setup: Result<Option<ITfKeystrokeMgr>> = (|| {
                match self.key_advice_mode {
                    KeyAdviceMode::Foreground => {
                        let manager: ITfKeystrokeMgr = thread_manager.cast()?;
                        let sink = self.to_interface::<ITfKeyEventSink>();
                        // SAFETY: a real TSF activation supplies a text-service
                        // client id and owns `sink` until matching Unadvise.
                        unsafe { manager.AdviseKeyEventSink(client_id, &sink, true) }?;
                        Ok(Some(manager))
                    }
                    #[cfg(test)]
                    KeyAdviceMode::DisabledForProcessTest => Ok(None),
                }
            })();
            let keystroke_manager = match key_setup {
                Ok(manager) => manager,
                Err(error) => {
                    // SAFETY: rolls back the successful thread-event advice.
                    let _ = unsafe { thread_source.UnadviseSink(thread_event_cookie) };
                    return Err(error);
                }
            };
            Ok((keystroke_manager, thread_source, thread_event_cookie))
        })();

        let (keystroke_manager, thread_source, thread_event_cookie) = match setup {
            Ok(state) => state,
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
                // SAFETY: balances the successful AdviseSink above.
                let _ = unsafe { thread_source.UnadviseSink(thread_event_cookie) };
                return Err(lifecycle_error(E_UNEXPECTED));
            }
        };
        activation.thread_manager = Some(thread_manager);
        activation.keystroke_manager = keystroke_manager;
        activation.thread_source = Some(thread_source);
        activation.thread_event_cookie = Some(thread_event_cookie);
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
        if self
            .document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .cleanup_scheduled
        {
            return Ok(None);
        }
        let session = self
            .composition
            .try_borrow()
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

        self.request_document_edit_session(
            &context,
            client_id,
            plan.edit,
            EditSessionMode::KeySynchronous,
            None,
        )?;

        let mut composition = self
            .composition
            .try_borrow_mut()
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
        let cleanup_result = self.schedule_active_composition_cleanup(previous.client_id);
        let composition_result = match self.composition.try_borrow_mut() {
            Ok(mut composition) => {
                composition.finish_commit();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let key_unadvise_result = if let Some(keystroke_manager) = previous.keystroke_manager {
            // SAFETY: balances the successful advice owned by `previous`.
            unsafe { keystroke_manager.UnadviseKeyEventSink(previous.client_id) }
        } else {
            Ok(())
        };
        let thread_unadvise_result = match (previous.thread_source, previous.thread_event_cookie) {
            (Some(source), Some(cookie)) => {
                // SAFETY: balances the successful thread-event advice.
                unsafe { source.UnadviseSink(cookie) }
            }
            (None, None) => Ok(()),
            _ => Err(lifecycle_error(E_UNEXPECTED)),
        };
        cleanup_result?;
        key_unadvise_result?;
        thread_unadvise_result?;
        composition_result
    }
}

impl ITfTextInputProcessorEx_Impl for TsfTextService_Impl {
    fn ActivateEx(&self, ptim: Ref<ITfThreadMgr>, tid: u32, dwflags: u32) -> Result<()> {
        self.activate_inner(ptim, tid, dwflags)
    }
}

impl ITfKeyEventSink_Impl for TsfTextService_Impl {
    fn OnSetFocus(&self, fforeground: windows::core::BOOL) -> Result<()> {
        if !fforeground.as_bool() {
            self.cleanup_after_focus_loss()?;
        }
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

impl ITfThreadMgrEventSink_Impl for TsfTextService_Impl {
    fn OnInitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _document: Ref<ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnSetFocus(
        &self,
        focused: Ref<ITfDocumentMgr>,
        _previous: Ref<ITfDocumentMgr>,
    ) -> Result<()> {
        if !self.active_document_has_focus(focused)? {
            self.cleanup_after_focus_loss()?;
        }
        Ok(())
    }

    fn OnPushContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        Ok(())
    }

    fn OnPopContext(&self, _context: Ref<ITfContext>) -> Result<()> {
        Ok(())
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
    use windows::Win32::UI::TextServices::{
        CLSID_TF_ThreadMgr, ITfCompositionView, ITfContextOwnerCompositionServices, TF_ES_READ,
        TF_POPF_ALL, TF_TF_MOVESTART,
    };
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

    #[implement(ITfEditSession)]
    struct ContextTextReader {
        context: ITfContext,
        output: Arc<Mutex<Option<String>>>,
    }

    impl ITfEditSession_Impl for ContextTextReader_Impl {
        fn DoEditSession(&self, ec: u32) -> Result<()> {
            // SAFETY: `ec` is the read cookie issued for this callback. Moving
            // a cloned start range does not change the document or selection.
            let range = unsafe { self.context.GetStart(ec) }?;
            // SAFETY: the end range belongs to the same context and cookie.
            let end = unsafe { self.context.GetEnd(ec) }?;
            // SAFETY: expands the local range to cover the full context.
            unsafe { range.ShiftEndToRange(ec, &end, TF_ANCHOR_END) }?;

            let mut utf16 = Vec::new();
            loop {
                let mut chunk = [0u16; 64];
                let mut fetched = 0;
                // SAFETY: the output buffer is valid and TF_TF_MOVESTART only
                // advances this local range while reading successive chunks.
                unsafe { range.GetText(ec, TF_TF_MOVESTART, &mut chunk, &mut fetched) }?;
                let fetched =
                    usize::try_from(fetched).map_err(|_| lifecycle_error(E_UNEXPECTED))?;
                utf16.extend_from_slice(&chunk[..fetched]);
                if fetched < chunk.len() {
                    break;
                }
            }
            let text = String::from_utf16(&utf16).map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            *self
                .output
                .lock()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))? = Some(text);
            Ok(())
        }
    }

    fn read_context_text(context: &ITfContext, client_id: u32) -> String {
        let output = Arc::new(Mutex::new(None));
        let reader: ITfEditSession = ContextTextReader {
            context: context.clone(),
            output: Arc::clone(&output),
        }
        .into();
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
        // SAFETY: this synthetic context is focused on the current apartment;
        // the read session owns all interfaces until the synchronous return.
        let session_result = unsafe { context.RequestEditSession(client_id, &reader, flags) }
            .expect("context read session should be scheduled");
        session_result
            .ok()
            .expect("context read session should complete");
        output
            .lock()
            .unwrap()
            .take()
            .expect("read callback should populate its output")
    }

    fn terminate_composition_from_host(context: &ITfContext) {
        let owner: ITfContextOwnerCompositionServices = context
            .cast()
            .expect("synthetic context should expose owner composition services");
        // SAFETY: a null view asks the context owner to terminate every active
        // composition, modeling focus loss or host-driven cleanup.
        unsafe { owner.TerminateComposition(None::<&ITfCompositionView>) }
            .expect("host termination should succeed");
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
    fn development_provider_decodes_public_fixture_and_preserves_unknown_input() {
        let provider = development_candidate_provider();
        assert_eq!(provider.candidate("nihk", 1).as_deref(), Some("你好"));
        assert_eq!(provider.candidate("nihk", 0), None);
        assert_eq!(provider.candidate("nihk", 11), None);
        assert_eq!(
            provider.candidate("zzzzzzzz", 1).as_deref(),
            Some("zzzzzzzz")
        );
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
    fn process_test_factory_service_decodes_and_commits_public_candidate() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        assert_eq!(DllCanUnloadNow(), S_OK);

        let factory: IClassFactory = TsfClassFactory::counted_for_process_test().into();
        // SAFETY: aggregation is disabled and the factory implements the
        // requested text-service interface.
        let service: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None::<&IUnknown>) }
            .expect("process-test factory should create the text service");
        let key_sink: ITfKeyEventSink = service
            .cast()
            .expect("factory service should expose its key-event sink");

        // SAFETY: COM is initialized on this test thread.
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        // SAFETY: balanced below on this apartment thread.
        let client_id = unsafe { thread_manager.Activate() }.expect("thread manager activation");
        // SAFETY: this factory uses the explicit process-test advice mode.
        unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect("process-test activation should succeed");

        let document_manager =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("document manager creation");
        let mut context = None;
        let mut text_store_cookie = 0;
        // SAFETY: output pointers remain valid for the call and the synthetic
        // context needs no external text store.
        unsafe {
            document_manager.CreateContext(
                client_id,
                0,
                None::<&IUnknown>,
                &mut context,
                &mut text_store_cookie,
            )
        }
        .expect("synthetic context creation");
        let context = context.expect("CreateContext should return a context");
        // SAFETY: all objects belong to this apartment thread.
        unsafe { document_manager.Push(&context) }.expect("context push");
        unsafe { thread_manager.SetFocus(&document_manager) }.expect("document focus");

        let lparam = LPARAM(0);
        for offset in [13_u16, 8, 7, 10] {
            let key = WPARAM(usize::from(VK_A.0 + offset));
            // SAFETY: virtual-key values contain no pointer data and every
            // interface belongs to the current apartment.
            assert!(
                unsafe { key_sink.OnTestKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool()
            );
            assert!(
                unsafe { key_sink.OnKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool()
            );
        }
        assert_eq!(read_context_text(&context, client_id), "nihk");

        let space = WPARAM(usize::from(VK_SPACE.0));
        // SAFETY: same apartment-local key routing as above.
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
        assert_eq!(read_context_text(&context, client_id), "你好");

        // SAFETY: exact reverse of the apartment-local setup.
        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(key_sink);
        drop(service);
        drop(factory);
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
        let thread_event_sink: ITfThreadMgrEventSink = service_object.to_interface();

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
        let backspace = WPARAM(usize::from(VK_BACK.0));
        let space = WPARAM(usize::from(VK_SPACE.0));
        let b = WPARAM(usize::from(VK_A.0 + 1));
        let c = WPARAM(usize::from(VK_A.0 + 2));
        let d = WPARAM(usize::from(VK_A.0 + 3));
        let e = WPARAM(usize::from(VK_A.0 + 4));
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
        assert_eq!(service_object.composition.borrow().phonetic(), "a");
        assert_eq!(read_context_text(&context, client_id), "a");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 1);
            assert_eq!(telemetry.last_kind, Some(DocumentEditKind::UpdatePreedit));
        }

        assert!(
            unsafe { key_sink.OnKeyDown(&context, a, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.composition.borrow().phonetic(), "aa");
        assert_eq!(read_context_text(&context, client_id), "aa");
        assert!(
            unsafe { key_sink.OnKeyDown(&context, backspace, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.composition.borrow().phonetic(), "a");
        assert_eq!(read_context_text(&context, client_id), "a");

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
        assert!(service_object.composition.borrow().phonetic().is_empty());
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 4);
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
        assert_eq!(read_context_text(&context, client_id), "啊b");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, escape, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        {
            let telemetry = service_object.edit_telemetry.lock().unwrap();
            assert_eq!(telemetry.completed, 6);
            assert_eq!(telemetry.last_kind, Some(DocumentEditKind::Cancel));
        }
        assert!(
            !unsafe { key_sink.OnKeyUp(&context, a, lparam) }
                .unwrap()
                .as_bool()
        );

        assert!(
            unsafe { key_sink.OnKeyDown(&context, c, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊c");
        let second_document =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("second document creation");
        let mut second_context = None;
        let mut second_text_store_cookie = 0;
        // SAFETY: output pointers remain valid and this is another synthetic
        // context owned by the same apartment thread.
        unsafe {
            second_document.CreateContext(
                client_id,
                0,
                None::<&IUnknown>,
                &mut second_context,
                &mut second_text_store_cookie,
            )
        }
        .expect("second context creation");
        let second_context = second_context.expect("second context should be returned");
        // SAFETY: both documents and contexts belong to this apartment.
        unsafe { second_document.Push(&second_context) }.expect("second context push");
        unsafe { thread_manager.SetFocus(&second_document) }.expect("second document focus");
        unsafe { thread_event_sink.OnSetFocus(&second_document, &document_manager) }
            .expect("document focus cleanup notification");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        assert!(
            service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );
        assert!(service_object.composition.borrow().phonetic().is_empty());
        terminate_composition_from_host(&context);
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        // SAFETY: restore the original focus before the next direct key call.
        unsafe { thread_manager.SetFocus(&document_manager) }.expect("original document focus");

        assert!(
            unsafe { key_sink.OnKeyDown(&context, c, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊c");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        terminate_composition_from_host(&context);
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(service_object.composition.borrow().phonetic().is_empty());
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        assert!(
            !unsafe { key_sink.OnTestKeyDown(&context, space, lparam) }
                .unwrap()
                .as_bool()
        );

        assert!(
            unsafe { key_sink.OnKeyDown(&context, d, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊d");
        // SAFETY: directly exercises the advised key sink's foreground-loss
        // callback with no system registration.
        unsafe { key_sink.OnSetFocus(false) }.expect("foreground loss cleanup");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        assert!(
            service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );
        terminate_composition_from_host(&context);
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(service_object.composition.borrow().phonetic().is_empty());
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        // SAFETY: models the service becoming foreground again.
        unsafe { key_sink.OnSetFocus(true) }.expect("foreground restore");

        assert!(
            unsafe { key_sink.OnKeyDown(&context, e, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊e");
        // SAFETY: deactivation schedules the same bounded cancellation before
        // releasing both event subscriptions.
        unsafe { service.Deactivate() }.expect("active composition deactivation");
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        assert!(
            service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );
        terminate_composition_from_host(&context);
        assert_eq!(read_context_text(&context, client_id), "啊");
        assert!(service_object.composition.borrow().phonetic().is_empty());
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );

        // SAFETY: cleanup is the exact reverse of the setup above.
        unsafe { second_document.Pop(TF_POPF_ALL) }.expect("second context pop");
        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("repeated service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(thread_event_sink);
        drop(key_sink);
        drop(service);
        drop(second_context);
        drop(second_document);
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
