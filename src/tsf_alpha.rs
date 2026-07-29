//! Build-only Windows TSF COM and composition probe.
//!
//! This module intentionally exports no registration functions. It proves
//! class-factory, activation, deactivation, server-lock, and unload behavior
//! without adding an input profile to Windows.

use std::cell::RefCell;
use std::error::Error as StdError;
use std::ffi::{OsString, c_void};
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{
    CANDIDATE_RUNTIME_DIRECTORY, CandidatePackageError, CandidatePackageManifest,
    CandidateRuntimeError, CandidateSnapshot, CompositionEffect, CompositionInput,
    CompositionSession, load_current_candidate_snapshot,
};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER, E_UNEXPECTED,
    HMODULE, HWND, LPARAM, RECT, S_FALSE, S_OK, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IClassFactory, IClassFactory_Impl,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::SystemServices::{SS_LEFTNOWORDWRAP, SS_NOPREFIX};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_1, VK_5, VK_A, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_NEXT,
    VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_Z,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfCandidateListUIElement, ITfCandidateListUIElement_Impl, ITfComposition,
    ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition, ITfDocumentMgr,
    ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfKeyEventSink,
    ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfRange, ITfSource, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, ITfUIElement, ITfUIElement_Impl, ITfUIElementMgr, TF_AE_NONE,
    TF_ANCHOR_END, TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX,
    TF_CLUIE_SELECTION, TF_CLUIE_STRING, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_ES_ASYNC, TF_ES_READ,
    TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_NO_DEFAULT_COMPOSITION, TF_POPF_ALL, TF_SELECTION,
    TF_SELECTIONSTYLE, TF_TF_MOVESTART,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{
    BSTR, Error, GUID, HRESULT, HSTRING, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, Result,
    implement, w,
};

/// Fixed COM class identity reserved for the local TSF alpha.
pub const TSF_ALPHA_CLSID: GUID = GUID::from_u128(0x4cc8427b_d0f5_439e_b6af_d45eacd7e577);
/// Fixed Simplified Chinese language-profile identity reserved for the alpha.
pub const TSF_ALPHA_PROFILE_GUID: GUID = GUID::from_u128(0x8099d3f8_9f40_4da5_9b01_c12de0cd6370);
/// Simplified Chinese (zh-CN) language identifier used by the alpha profile.
pub const TSF_ALPHA_LANGID: u16 = 0x0804;

static ACTIVE_COM_OBJECTS: AtomicUsize = AtomicUsize::new(0);
static SERVER_LOCKS: AtomicUsize = AtomicUsize::new(0);
static SYNTHETIC_HOST_LOCK: Mutex<()> = Mutex::new(());

// This deliberately small, manually constructed public fixture is only a
// bridge between the COM lifecycle and the real decoder. Loading the complete
// Rime snapshot inside every host process is a separate data-layer decision.
const TSF_DEVELOPMENT_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
const TSF_DEVELOPMENT_MANIFEST: &str =
    include_str!("../tests/fixtures/public/demo_candidate_manifest.zcm");
const CANDIDATE_PAGE_SIZE: usize = 5;
const CANDIDATE_LIMIT: usize = 10;
const CANDIDATE_DISPLAY_MAX_CHARS: usize = 32;
const CANDIDATE_UI_GUID: GUID = GUID::from_u128(0xb9fdad61_3f19_4d6c_86f7_72e9d3064f84);

trait CandidateProvider: Send + Sync {
    /// Returns one deterministic, bounded candidate page without learning or
    /// I/O. Implementations should decode once rather than once per rank.
    fn candidates(&self, code: &str, limit: usize) -> Vec<String>;
}

type CandidateProviderLoadResult =
    std::result::Result<Arc<dyn CandidateProvider>, CandidateProviderLoadError>;

#[derive(Clone, Debug, Eq, PartialEq)]
enum CandidateProviderLoadError {
    Embedded(CandidatePackageError),
    Runtime(CandidateRuntimeError),
    ModuleLocation,
}

struct SnapshotCandidateProvider {
    snapshot: Arc<CandidateSnapshot>,
}

impl CandidateProvider for SnapshotCandidateProvider {
    fn candidates(&self, code: &str, limit: usize) -> Vec<String> {
        self.snapshot
            .candidate_texts(code, limit)
            .unwrap_or_default()
    }
}

#[derive(Clone, Default)]
struct CandidateDisplay {
    candidates: Vec<String>,
    page_start: usize,
}

impl CandidateDisplay {
    fn from_candidates(candidates: Vec<String>, requested_page_start: usize) -> Self {
        let page_start = if candidates.is_empty() {
            0
        } else {
            requested_page_start
                .min((candidates.len() - 1) / CANDIDATE_PAGE_SIZE * CANDIDATE_PAGE_SIZE)
        };
        Self {
            candidates,
            page_start,
        }
    }

    fn visible(&self) -> &[String] {
        let end = self
            .page_start
            .saturating_add(CANDIDATE_PAGE_SIZE)
            .min(self.candidates.len());
        &self.candidates[self.page_start.min(end)..end]
    }

    fn page_starts(&self) -> Vec<u32> {
        (0..self.candidates.len())
            .step_by(CANDIDATE_PAGE_SIZE)
            .filter_map(|index| u32::try_from(index).ok())
            .collect()
    }

    fn current_page(&self) -> u32 {
        u32::try_from(self.page_start / CANDIDATE_PAGE_SIZE).unwrap_or(0)
    }

    fn selected_index(&self) -> u32 {
        u32::try_from(self.page_start).unwrap_or(0)
    }

    fn native_text(&self) -> String {
        let mut output = String::new();
        for (index, candidate) in self.visible().iter().enumerate() {
            if index > 0 {
                output.push('\n');
            }
            let mut clipped = candidate
                .chars()
                .take(CANDIDATE_DISPLAY_MAX_CHARS)
                .collect::<String>();
            if candidate.chars().count() > CANDIDATE_DISPLAY_MAX_CHARS {
                clipped.push('…');
            }
            use std::fmt::Write as _;
            let _ = write!(output, "{}  {clipped}", index + 1);
        }
        output
    }
}

fn development_candidate_provider() -> CandidateProviderLoadResult {
    static PROVIDER: OnceLock<CandidateProviderLoadResult> = OnceLock::new();
    PROVIDER
        .get_or_init(|| {
            let manifest = CandidatePackageManifest::parse(TSF_DEVELOPMENT_MANIFEST)
                .map_err(CandidateProviderLoadError::Embedded)?;
            let snapshot = Arc::new(
                manifest
                    .load_snapshot(TSF_DEVELOPMENT_LEXICON)
                    .map_err(CandidateProviderLoadError::Embedded)?,
            );
            Ok(Arc::new(SnapshotCandidateProvider { snapshot }) as Arc<dyn CandidateProvider>)
        })
        .clone()
}

fn candidate_provider_for_root(root: &Path) -> CandidateProviderLoadResult {
    match load_current_candidate_snapshot(root).map_err(CandidateProviderLoadError::Runtime)? {
        Some(snapshot) => {
            Ok(Arc::new(SnapshotCandidateProvider { snapshot }) as Arc<dyn CandidateProvider>)
        }
        None => development_candidate_provider(),
    }
}

fn module_candidate_runtime_root() -> std::result::Result<PathBuf, CandidateProviderLoadError> {
    let mut module = HMODULE::default();
    let address = DllGetClassObject as *const () as *const u16;
    // SAFETY: FROM_ADDRESS treats the non-null value as an address within the
    // current module rather than a string. UNCHANGED_REFCOUNT avoids taking a
    // library reference that would interfere with COM unload accounting.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(address),
            &mut module,
        )
    }
    .map_err(|_| CandidateProviderLoadError::ModuleLocation)?;

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: `module` identifies the image containing this function and the
    // writable buffer remains alive for the synchronous call.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(CandidateProviderLoadError::ModuleLocation);
    }
    let module_path = PathBuf::from(OsString::from_wide(&buffer[..length]));
    let parent = module_path
        .parent()
        .ok_or(CandidateProviderLoadError::ModuleLocation)?;
    Ok(parent.join(CANDIDATE_RUNTIME_DIRECTORY))
}

fn class_factory_candidate_provider() -> CandidateProviderLoadResult {
    let root = module_candidate_runtime_root()?;
    candidate_provider_for_root(&root)
}

const MAX_TSF_PREFLIGHT_CODE_KEYS: usize = 64;
const MAX_TSF_PREFLIGHT_TEXT_CHARACTERS: usize = 256;

/// Redacted evidence from one real TSF synthetic-context candidate preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TsfCandidatePreflightReport {
    revision: String,
    input_keys: usize,
    committed_characters: usize,
}

impl TsfCandidatePreflightReport {
    /// Returns the immutable candidate data revision that was exercised.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Returns the number of lowercase letter keys routed through TSF.
    pub fn input_keys(&self) -> usize {
        self.input_keys
    }

    /// Returns the number of Unicode scalar values committed to the context.
    pub fn committed_characters(&self) -> usize {
        self.committed_characters
    }
}

/// Sanitized failures from a candidate snapshot's TSF synthetic-host preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TsfCandidatePreflightError {
    /// The probe code is empty, too long, or not lowercase ASCII.
    InvalidProbeCode,
    /// The expected commit is empty or exceeds the fixed bound.
    InvalidExpectedText,
    /// The snapshot does not rank the supplied expected text first.
    CandidateMismatch,
    /// Another synthetic-host operation is already using local COM state.
    HostBusy,
    /// The calling thread could not enter a compatible COM apartment.
    ComInitialization,
    /// The system TSF thread manager could not be created or activated.
    ThreadManager,
    /// The local class factory could not construct or activate its service.
    ServiceActivation,
    /// The synthetic document, context, or focus could not be prepared.
    ContextSetup,
    /// A probe key was rejected or its edit transaction failed.
    KeyRouting,
    /// The live preedit text differed from the supplied code.
    PreeditMismatch,
    /// The final context text differed from the expected candidate.
    CommitMismatch,
    /// TSF objects could not be cleanly released after the probe.
    Cleanup,
}

impl fmt::Display for TsfCandidatePreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProbeCode => write!(formatter, "TSF 预检按键无效"),
            Self::InvalidExpectedText => write!(formatter, "TSF 预检目标文字无效"),
            Self::CandidateMismatch => write!(formatter, "候选快照与 TSF 预检目标不符"),
            Self::HostBusy => write!(formatter, "TSF 合成宿主当前不可用"),
            Self::ComInitialization => write!(formatter, "TSF 预检无法初始化 COM"),
            Self::ThreadManager => write!(formatter, "TSF 预检无法建立线程管理器"),
            Self::ServiceActivation => write!(formatter, "TSF 预检无法激活文本服务"),
            Self::ContextSetup => write!(formatter, "TSF 预检无法建立合成输入框"),
            Self::KeyRouting => write!(formatter, "TSF 预检按键事务失败"),
            Self::PreeditMismatch => write!(formatter, "TSF 预检组合文字不符"),
            Self::CommitMismatch => write!(formatter, "TSF 预检上屏文字不符"),
            Self::Cleanup => write!(formatter, "TSF 预检清理失败"),
        }
    }
}

impl StdError for TsfCandidatePreflightError {}

/// Routes one snapshot candidate through a real system TSF synthetic context.
///
/// The caller supplies the expected first candidate explicitly. Neither the
/// probe code nor its candidate text is retained in the returned report.
pub fn preflight_candidate_snapshot(
    snapshot: Arc<CandidateSnapshot>,
    probe_code: &str,
    expected_text: &str,
) -> std::result::Result<TsfCandidatePreflightReport, TsfCandidatePreflightError> {
    if probe_code.is_empty()
        || probe_code.len() > MAX_TSF_PREFLIGHT_CODE_KEYS
        || !probe_code.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(TsfCandidatePreflightError::InvalidProbeCode);
    }
    let committed_characters = expected_text.chars().count();
    if committed_characters == 0 || committed_characters > MAX_TSF_PREFLIGHT_TEXT_CHARACTERS {
        return Err(TsfCandidatePreflightError::InvalidExpectedText);
    }
    if snapshot
        .candidate_text(probe_code, 1)
        .map_err(|_| TsfCandidatePreflightError::CandidateMismatch)?
        .as_deref()
        != Some(expected_text)
    {
        return Err(TsfCandidatePreflightError::CandidateMismatch);
    }

    let _host_guard = SYNTHETIC_HOST_LOCK
        .lock()
        .map_err(|_| TsfCandidatePreflightError::HostBusy)?;
    if !can_unload_now() {
        return Err(TsfCandidatePreflightError::HostBusy);
    }
    let revision = snapshot.revision().to_owned();
    let result = run_candidate_preflight(snapshot, probe_code, expected_text);
    if !can_unload_now() {
        return Err(TsfCandidatePreflightError::Cleanup);
    }
    result?;
    Ok(TsfCandidatePreflightReport {
        revision,
        input_keys: probe_code.len(),
        committed_characters,
    })
}

struct PreflightApartment;

impl PreflightApartment {
    fn enter() -> std::result::Result<Self, TsfCandidatePreflightError> {
        // SAFETY: the preflight owns this calling thread until the matching
        // guard is dropped and requests a single-threaded COM apartment.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|_| TsfCandidatePreflightError::ComInitialization)?;
        Ok(Self)
    }
}

impl Drop for PreflightApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
}

struct ThreadManagerActivation {
    manager: ITfThreadMgr,
    active: bool,
}

impl ThreadManagerActivation {
    fn close(&mut self) -> Result<()> {
        if self.active {
            // SAFETY: balances the successful Activate on this apartment.
            unsafe { self.manager.Deactivate() }?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ThreadManagerActivation {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct ServiceActivation {
    service: ITfTextInputProcessorEx,
    active: bool,
}

impl ServiceActivation {
    fn close(&mut self) -> Result<()> {
        if self.active {
            // SAFETY: balances the successful process-local ActivateEx.
            unsafe { self.service.Deactivate() }?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ServiceActivation {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct DocumentActivation {
    manager: ITfDocumentMgr,
    pushed: bool,
}

impl DocumentActivation {
    fn close(&mut self) -> Result<()> {
        if self.pushed {
            // SAFETY: removes every context pushed by this synthetic host.
            unsafe { self.manager.Pop(TF_POPF_ALL) }?;
            self.pushed = false;
        }
        Ok(())
    }
}

impl Drop for DocumentActivation {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[implement(ITfEditSession)]
struct PreflightContextTextReader {
    context: ITfContext,
    output: Arc<Mutex<Option<String>>>,
}

impl ITfEditSession_Impl for PreflightContextTextReader_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        // SAFETY: `ec` grants read access to this context for the callback.
        let range = unsafe { self.context.GetStart(ec) }?;
        // SAFETY: the end range belongs to the same context and cookie.
        let end = unsafe { self.context.GetEnd(ec) }?;
        // SAFETY: expands only this local range to cover the context.
        unsafe { range.ShiftEndToRange(ec, &end, TF_ANCHOR_END) }?;

        let mut utf16 = Vec::new();
        loop {
            let mut chunk = [0_u16; 64];
            let mut fetched = 0;
            // SAFETY: the writable chunk is valid for this synchronous read.
            unsafe { range.GetText(ec, TF_TF_MOVESTART, &mut chunk, &mut fetched) }?;
            let fetched = usize::try_from(fetched).map_err(|_| lifecycle_error(E_UNEXPECTED))?;
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

fn read_preflight_context_text(context: &ITfContext, client_id: u32) -> Result<String> {
    let output = Arc::new(Mutex::new(None));
    let reader: ITfEditSession = PreflightContextTextReader {
        context: context.clone(),
        output: Arc::clone(&output),
    }
    .into();
    let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READ.0);
    // SAFETY: every interface is apartment-local and the callback is sync.
    unsafe { context.RequestEditSession(client_id, &reader, flags) }?.ok()?;
    output
        .lock()
        .map_err(|_| lifecycle_error(E_UNEXPECTED))?
        .take()
        .ok_or_else(|| lifecycle_error(E_UNEXPECTED))
}

fn run_candidate_preflight(
    snapshot: Arc<CandidateSnapshot>,
    probe_code: &str,
    expected_text: &str,
) -> std::result::Result<(), TsfCandidatePreflightError> {
    let _apartment = PreflightApartment::enter()?;
    let provider = Arc::new(SnapshotCandidateProvider { snapshot }) as Arc<dyn CandidateProvider>;
    let factory: IClassFactory =
        TsfClassFactory::counted_with_options(Ok(provider), KeyAdviceMode::SyntheticHost).into();
    // SAFETY: aggregation is disabled and the local factory implements this interface.
    let service: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None::<&IUnknown>) }
        .map_err(|_| TsfCandidatePreflightError::ServiceActivation)?;
    let key_sink: ITfKeyEventSink = service
        .cast()
        .map_err(|_| TsfCandidatePreflightError::ServiceActivation)?;

    // SAFETY: COM is initialized on this thread and this is the system TSF manager.
    let thread_manager: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|_| TsfCandidatePreflightError::ThreadManager)?;
    // SAFETY: balanced by ThreadManagerActivation below.
    let client_id = unsafe { thread_manager.Activate() }
        .map_err(|_| TsfCandidatePreflightError::ThreadManager)?;
    let mut thread_activation = ThreadManagerActivation {
        manager: thread_manager.clone(),
        active: true,
    };
    // SAFETY: SyntheticHost skips foreground key advice but uses normal state.
    unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
        .map_err(|_| TsfCandidatePreflightError::ServiceActivation)?;
    let mut service_activation = ServiceActivation {
        service: service.clone(),
        active: true,
    };

    // SAFETY: all objects remain on this initialized apartment thread.
    let document_manager = unsafe { thread_manager.CreateDocumentMgr() }
        .map_err(|_| TsfCandidatePreflightError::ContextSetup)?;
    let mut context = None;
    let mut text_store_cookie = 0;
    // SAFETY: output storage remains live and no external text store is used.
    unsafe {
        document_manager.CreateContext(
            client_id,
            0,
            None::<&IUnknown>,
            &mut context,
            &mut text_store_cookie,
        )
    }
    .map_err(|_| TsfCandidatePreflightError::ContextSetup)?;
    let context = context.ok_or(TsfCandidatePreflightError::ContextSetup)?;
    // SAFETY: context and manager belong to this apartment.
    unsafe { document_manager.Push(&context) }
        .map_err(|_| TsfCandidatePreflightError::ContextSetup)?;
    let mut document_activation = DocumentActivation {
        manager: document_manager.clone(),
        pushed: true,
    };
    // SAFETY: focuses the synthetic document owned by this thread manager.
    unsafe { thread_manager.SetFocus(&document_manager) }
        .map_err(|_| TsfCandidatePreflightError::ContextSetup)?;

    let lparam = LPARAM(0);
    for byte in probe_code.bytes() {
        let key = WPARAM(usize::from(VK_A.0 + u16::from(byte - b'a')));
        // SAFETY: these virtual-key values contain no pointer data.
        let tested = unsafe { key_sink.OnTestKeyDown(&context, key, lparam) }
            .map_err(|_| TsfCandidatePreflightError::KeyRouting)?;
        // SAFETY: routes the same accepted key through the active service.
        let handled = unsafe { key_sink.OnKeyDown(&context, key, lparam) }
            .map_err(|_| TsfCandidatePreflightError::KeyRouting)?;
        if !tested.as_bool() || !handled.as_bool() {
            return Err(TsfCandidatePreflightError::KeyRouting);
        }
    }
    if read_preflight_context_text(&context, client_id)
        .map_err(|_| TsfCandidatePreflightError::KeyRouting)?
        != probe_code
    {
        return Err(TsfCandidatePreflightError::PreeditMismatch);
    }

    let space = WPARAM(usize::from(VK_SPACE.0));
    // SAFETY: routes one ordinary confirmation key through the same context.
    let tested = unsafe { key_sink.OnTestKeyDown(&context, space, lparam) }
        .map_err(|_| TsfCandidatePreflightError::KeyRouting)?;
    // SAFETY: commits the first candidate through the active service.
    let handled = unsafe { key_sink.OnKeyDown(&context, space, lparam) }
        .map_err(|_| TsfCandidatePreflightError::KeyRouting)?;
    if !tested.as_bool() || !handled.as_bool() {
        return Err(TsfCandidatePreflightError::KeyRouting);
    }
    if read_preflight_context_text(&context, client_id)
        .map_err(|_| TsfCandidatePreflightError::KeyRouting)?
        != expected_text
    {
        return Err(TsfCandidatePreflightError::CommitMismatch);
    }

    document_activation
        .close()
        .map_err(|_| TsfCandidatePreflightError::Cleanup)?;
    service_activation
        .close()
        .map_err(|_| TsfCandidatePreflightError::Cleanup)?;
    thread_activation
        .close()
        .map_err(|_| TsfCandidatePreflightError::Cleanup)?;
    drop(context);
    drop(document_manager);
    drop(key_sink);
    drop(service);
    drop(factory);
    drop(thread_manager);
    Ok(())
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
    edit: Option<PendingDocumentEdit>,
    candidate_display: Option<CandidateDisplay>,
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
        key if (VK_1.0..=VK_5.0).contains(&key) => {
            Some(CompositionInput::Select(usize::from(key - VK_1.0) + 1))
        }
        _ => None,
    }
}

fn plan_session_input(
    session: &CompositionSession,
    input: CompositionInput,
    selected_text: Option<String>,
    candidate_count: usize,
) -> Option<PlannedKey> {
    let before = session.clone();
    let mut after = before.clone();
    let effect = after.apply(input.clone());
    let edit = match (input, effect) {
        (CompositionInput::Letters(_), CompositionEffect::Continue) => Some(
            PendingDocumentEdit::UpdatePreedit(after.phonetic().to_owned()),
        ),
        (CompositionInput::Backspace | CompositionInput::Escape, CompositionEffect::Continue)
            if before.phonetic() != after.phonetic() && after.phonetic().is_empty() =>
        {
            Some(PendingDocumentEdit::Cancel)
        }
        (CompositionInput::Backspace | CompositionInput::Escape, CompositionEffect::Continue)
            if before.phonetic() != after.phonetic() =>
        {
            Some(PendingDocumentEdit::UpdatePreedit(
                after.phonetic().to_owned(),
            ))
        }
        (CompositionInput::Confirm, CompositionEffect::Confirm)
        | (CompositionInput::Select(_), CompositionEffect::Select(_)) => {
            let text = selected_text.filter(|text| !text.is_empty())?;
            after.finish_commit();
            Some(PendingDocumentEdit::Commit(text))
        }
        (CompositionInput::PreviousPage, CompositionEffect::PreviousPage) => {
            after.previous_candidate_page(CANDIDATE_PAGE_SIZE);
            None
        }
        (CompositionInput::NextPage, CompositionEffect::NextPage) => {
            after.next_candidate_page(candidate_count, CANDIDATE_PAGE_SIZE, CANDIDATE_LIMIT);
            after.normalize_candidate_page(candidate_count, CANDIDATE_PAGE_SIZE);
            None
        }
        _ => return None,
    };
    Some(PlannedKey {
        before,
        after,
        edit,
        candidate_display: None,
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
    candidate_provider: CandidateProviderLoadResult,
    key_advice_mode: KeyAdviceMode,
}

impl TsfClassFactory {
    fn counted() -> std::result::Result<Self, CandidateProviderLoadError> {
        let provider = class_factory_candidate_provider()?;
        Ok(Self::counted_with_options(
            Ok(provider),
            KeyAdviceMode::Foreground,
        ))
    }

    fn counted_with_options(
        candidate_provider: CandidateProviderLoadResult,
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
            KeyAdviceMode::SyntheticHost,
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

        let candidate_provider = self
            .candidate_provider
            .as_ref()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        let service: ITfTextInputProcessorEx = TsfTextService::counted_with_options(
            Some(Arc::clone(candidate_provider)),
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

#[derive(Default)]
struct CandidateElementState {
    display: Option<CandidateDisplay>,
    document_manager: Option<ITfDocumentMgr>,
    shown: bool,
}

#[derive(Default)]
struct CandidatePopup {
    hwnd: Option<HWND>,
    owner: Option<HWND>,
    anchor: Option<RECT>,
}

impl CandidatePopup {
    fn show(&mut self, owner: HWND, anchor: RECT, display: &CandidateDisplay) -> Result<()> {
        if display.visible().is_empty() {
            self.hide();
            return Ok(());
        }
        if self.owner.is_some_and(|current| current != owner) {
            self.destroy();
        }
        let text = HSTRING::from(display.native_text());
        let hwnd = match self.hwnd {
            Some(hwnd) => {
                // SAFETY: this process owns the popup handle until destroy.
                unsafe { SetWindowTextW(hwnd, &text) }?;
                hwnd
            }
            None => {
                let ex_style =
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0);
                let style =
                    WINDOW_STYLE(WS_POPUP.0 | WS_BORDER.0 | SS_LEFTNOWORDWRAP.0 | SS_NOPREFIX.0);
                // SAFETY: STATIC is a system window class. The window is an
                // owned, nonactivating popup and receives no application data
                // through lpParam.
                let created = unsafe {
                    CreateWindowExW(
                        ex_style,
                        w!("STATIC"),
                        &text,
                        style,
                        0,
                        0,
                        0,
                        0,
                        (!owner.is_invalid()).then_some(owner),
                        None,
                        None,
                        None,
                    )
                }?;
                self.hwnd = Some(created);
                self.owner = Some(owner);
                created
            }
        };

        let dpi = if owner.is_invalid() {
            96
        } else {
            // SAFETY: GetDpiForWindow is read-only and accepts this host-owned
            // HWND. A zero result falls back to the platform baseline.
            unsafe { GetDpiForWindow(owner) }.max(96)
        };
        let scale = |logical: i32| {
            i32::try_from(
                i64::from(logical)
                    .saturating_mul(i64::from(dpi))
                    .saturating_add(48)
                    / 96,
            )
            .unwrap_or(i32::MAX)
        };
        let width = scale(320);
        let height = scale(12_i32.saturating_add(
            28_i32.saturating_mul(i32::try_from(display.visible().len()).unwrap_or(5)),
        ));
        let gap = scale(4);
        let mut x = anchor.left;
        let mut y = anchor.bottom.saturating_add(gap);

        // SAFETY: the anchor is initialized screen geometry from TSF.
        let monitor = unsafe { MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST) };
        let mut monitor_info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        // SAFETY: monitor_info is writable for the duration of the call.
        if !monitor.is_invalid() && unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool()
        {
            let work = monitor_info.rcWork;
            if y.saturating_add(height) > work.bottom {
                y = anchor.top.saturating_sub(height).saturating_sub(gap);
            }
            let max_x = work.right.saturating_sub(width).max(work.left);
            let max_y = work.bottom.saturating_sub(height).max(work.top);
            x = x.clamp(work.left, max_x);
            y = y.clamp(work.top, max_y);
        }

        let flags = SET_WINDOW_POS_FLAGS(SWP_NOACTIVATE.0 | SWP_SHOWWINDOW.0);
        // SAFETY: the popup belongs to this controller. NOACTIVATE preserves
        // the editor's keyboard focus while TOPMOST keeps the short-lived list
        // above its owner.
        unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, flags) }?;
        self.anchor = Some(anchor);
        Ok(())
    }

    fn update(&mut self, display: &CandidateDisplay) -> Result<()> {
        let (Some(owner), Some(anchor)) = (self.owner, self.anchor) else {
            return Ok(());
        };
        self.show(owner, anchor, display)
    }

    fn set_visible(&mut self, visible: bool) {
        let Some(hwnd) = self.hwnd else {
            return;
        };
        // SAFETY: this process owns the popup handle. ShowWindow does not
        // transfer ownership and SW_SHOWNOACTIVATE preserves editor focus.
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    fn hide(&mut self) {
        self.set_visible(false);
    }

    fn destroy(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: this controller created and still owns the popup.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        self.owner = None;
        self.anchor = None;
    }
}

impl Drop for CandidatePopup {
    fn drop(&mut self) {
        self.destroy();
    }
}

#[implement(ITfCandidateListUIElement)]
struct CandidateListElement {
    state: Rc<RefCell<CandidateElementState>>,
    popup: Weak<RefCell<CandidatePopup>>,
}

impl CandidateListElement {
    fn counted(
        state: Rc<RefCell<CandidateElementState>>,
        popup: Weak<RefCell<CandidatePopup>>,
    ) -> Self {
        object_created();
        Self { state, popup }
    }
}

impl Drop for CandidateListElement {
    fn drop(&mut self) {
        object_dropped();
    }
}

impl ITfUIElement_Impl for CandidateListElement_Impl {
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("Ziranma Decoder Alpha candidates"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        Ok(CANDIDATE_UI_GUID)
    }

    fn Show(&self, show: windows::core::BOOL) -> Result<()> {
        let visible = show.as_bool();
        self.state
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .shown = visible;
        if let Some(popup) = self.popup.upgrade() {
            popup
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                .set_visible(visible);
        }
        Ok(())
    }

    fn IsShown(&self) -> Result<windows::core::BOOL> {
        Ok(self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .shown
            .into())
    }
}

impl ITfCandidateListUIElement_Impl for CandidateListElement_Impl {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        Ok(TF_CLUIE_DOCUMENTMGR
            | TF_CLUIE_COUNT
            | TF_CLUIE_SELECTION
            | TF_CLUIE_STRING
            | TF_CLUIE_PAGEINDEX
            | TF_CLUIE_CURRENTPAGE)
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .document_manager
            .clone()
            .ok_or_else(|| lifecycle_error(E_UNEXPECTED))
    }

    fn GetCount(&self) -> Result<u32> {
        let count = self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .display
            .as_ref()
            .map_or(0, |display| display.candidates.len());
        u32::try_from(count).map_err(|_| lifecycle_error(E_UNEXPECTED))
    }

    fn GetSelection(&self) -> Result<u32> {
        Ok(self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .display
            .as_ref()
            .map_or(0, CandidateDisplay::selected_index))
    }

    fn GetString(&self, index: u32) -> Result<BSTR> {
        let index = usize::try_from(index).map_err(|_| lifecycle_error(E_INVALIDARG))?;
        let state = self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        let candidate = state
            .display
            .as_ref()
            .and_then(|display| display.candidates.get(index))
            .ok_or_else(|| lifecycle_error(E_INVALIDARG))?;
        Ok(BSTR::from(candidate.as_str()))
    }

    fn GetPageIndex(&self, indices: *mut u32, size: u32, page_count: *mut u32) -> Result<()> {
        if page_count.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        let starts = self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .display
            .as_ref()
            .map_or_else(Vec::new, CandidateDisplay::page_starts);
        let capacity = usize::try_from(size).map_err(|_| lifecycle_error(E_INVALIDARG))?;
        let copied = starts.len().min(capacity);
        if copied > 0 && indices.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        // SAFETY: the caller supplies `size` writable u32 entries. We copy no
        // more than that bound and page_count is non-null.
        unsafe {
            if copied > 0 {
                ptr::copy_nonoverlapping(starts.as_ptr(), indices, copied);
            }
            *page_count = u32::try_from(starts.len()).map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        }
        Ok(())
    }

    fn SetPageIndex(&self, indices: *const u32, page_count: u32) -> Result<()> {
        let count = usize::try_from(page_count).map_err(|_| lifecycle_error(E_INVALIDARG))?;
        if count > 0 && indices.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        let expected = self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .display
            .as_ref()
            .map_or_else(Vec::new, CandidateDisplay::page_starts);
        // SAFETY: a non-null pointer is required above when count is nonzero;
        // the caller promises exactly page_count readable entries.
        let supplied = if count == 0 {
            &[]
        } else {
            // SAFETY: non-null was required above and the caller promises
            // exactly page_count readable entries.
            unsafe { std::slice::from_raw_parts(indices, count) }
        };
        if supplied == expected {
            Ok(())
        } else {
            Err(lifecycle_error(E_INVALIDARG))
        }
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        Ok(self
            .state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .display
            .as_ref()
            .map_or(0, CandidateDisplay::current_page))
    }
}

struct CandidateUiController {
    enabled: bool,
    manager: Option<ITfUIElementMgr>,
    state: Rc<RefCell<CandidateElementState>>,
    popup: Rc<RefCell<CandidatePopup>>,
    element: Option<ITfCandidateListUIElement>,
    element_id: Option<u32>,
    show_native: bool,
}

impl CandidateUiController {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            manager: None,
            state: Rc::new(RefCell::new(CandidateElementState::default())),
            popup: Rc::new(RefCell::new(CandidatePopup::default())),
            element: None,
            element_id: None,
            show_native: false,
        }
    }

    fn activate(&mut self, thread_manager: &ITfThreadMgr) {
        if self.enabled {
            self.manager = thread_manager.cast().ok();
        }
    }

    fn show(&mut self, context: &ITfContext, range: &ITfRange, ec: u32, display: CandidateDisplay) {
        if !self.enabled || display.candidates.is_empty() {
            self.end();
            return;
        }
        let document_manager = match unsafe { context.GetDocumentMgr() } {
            Ok(manager) => manager,
            Err(_) => {
                self.end();
                return;
            }
        };
        let view = match unsafe { context.GetActiveView() } {
            Ok(view) => view,
            Err(_) => {
                self.end();
                return;
            }
        };
        let mut anchor = RECT::default();
        let mut clipped = false.into();
        // SAFETY: ec grants read access to the active composition range.
        if unsafe { view.GetTextExt(ec, range, &mut anchor, &mut clipped) }.is_err() {
            self.end();
            return;
        }
        let owner = unsafe { view.GetWnd() }.unwrap_or_default();
        {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                self.end();
                return;
            };
            state.display = Some(display.clone());
            state.document_manager = Some(document_manager);
        }

        if self.element.is_none() {
            let element: ITfCandidateListUIElement =
                CandidateListElement::counted(Rc::clone(&self.state), Rc::downgrade(&self.popup))
                    .into();
            let mut show_native = true.into();
            let mut element_id = 0_u32;
            let began = self.manager.as_ref().is_none_or(|manager| {
                let base: Result<ITfUIElement> = element.cast();
                base.and_then(|base| unsafe {
                    manager.BeginUIElement(&base, &mut show_native, &mut element_id)
                })
                .is_ok()
            });
            if began {
                self.show_native = show_native.as_bool();
                self.element_id = self.manager.as_ref().map(|_| element_id);
                self.element = Some(element);
                if let Ok(mut state) = self.state.try_borrow_mut() {
                    state.shown = true;
                }
            } else {
                self.show_native = true;
                self.element = None;
                self.element_id = None;
                if let Ok(mut state) = self.state.try_borrow_mut() {
                    state.shown = true;
                }
            }
        } else if let (Some(manager), Some(element_id)) = (&self.manager, self.element_id) {
            // SAFETY: the id belongs to the element begun by this controller.
            if unsafe { manager.UpdateUIElement(element_id) }.is_err() {
                self.end();
                return;
            }
        }

        let element_visible = self
            .state
            .try_borrow()
            .map(|state| state.shown)
            .unwrap_or(false);
        let show_popup = self.show_native && element_visible && !clipped.as_bool();
        if let Ok(mut popup) = self.popup.try_borrow_mut() {
            if show_popup {
                let _ = popup.show(owner, anchor, &display);
            } else {
                popup.hide();
            }
        }
    }

    fn update_contents(&mut self, display: CandidateDisplay) {
        if !self.enabled || display.candidates.is_empty() {
            self.end();
            return;
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.display = Some(display.clone());
        } else {
            self.end();
            return;
        }
        if let (Some(manager), Some(element_id)) = (&self.manager, self.element_id) {
            // SAFETY: the id belongs to the element begun by this controller.
            if unsafe { manager.UpdateUIElement(element_id) }.is_err() {
                self.end();
                return;
            }
        }
        let element_visible = self
            .state
            .try_borrow()
            .map(|state| state.shown)
            .unwrap_or(false);
        if let Ok(mut popup) = self.popup.try_borrow_mut() {
            if self.show_native && element_visible {
                let _ = popup.update(&display);
            } else {
                popup.hide();
            }
        }
    }

    fn end(&mut self) {
        if let (Some(manager), Some(element_id)) = (&self.manager, self.element_id.take()) {
            // SAFETY: best-effort cleanup of the id begun by this controller.
            unsafe {
                let _ = manager.EndUIElement(element_id);
            }
        }
        self.element = None;
        self.show_native = false;
        if let Ok(mut popup) = self.popup.try_borrow_mut() {
            popup.hide();
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.display = None;
            state.document_manager = None;
            state.shown = false;
        }
    }

    fn deactivate(&mut self) {
        self.end();
        self.manager = None;
    }
}

impl Drop for CandidateUiController {
    fn drop(&mut self) {
        self.deactivate();
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
    candidate_ui: Weak<RefCell<CandidateUiController>>,
}

impl TsfCompositionSink {
    fn counted(
        document_composition: Weak<RefCell<DocumentCompositionState>>,
        logical_composition: Weak<RefCell<CompositionSession>>,
        candidate_ui: Weak<RefCell<CandidateUiController>>,
    ) -> Self {
        object_created();
        Self {
            document_composition,
            logical_composition,
            candidate_ui,
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
        if let Some(candidate_ui) = self.candidate_ui.upgrade()
            && let Ok(mut candidate_ui) = candidate_ui.try_borrow_mut()
        {
            candidate_ui.end();
        }
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
struct EditSessionShared {
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
}

#[implement(ITfEditSession)]
struct TsfDocumentEditSession {
    context: ITfContext,
    action: PendingDocumentEdit,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    candidate_display: Option<CandidateDisplay>,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
}

impl TsfDocumentEditSession {
    fn counted(
        context: ITfContext,
        action: PendingDocumentEdit,
        shared: EditSessionShared,
        candidate_display: Option<CandidateDisplay>,
        mode: EditSessionMode,
        cleanup_target: Option<ITfComposition>,
    ) -> Self {
        object_created();
        Self {
            context,
            action,
            document_composition: shared.document_composition,
            logical_composition: shared.logical_composition,
            telemetry: shared.telemetry,
            candidate_ui: shared.candidate_ui,
            candidate_display,
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
            Rc::downgrade(&self.candidate_ui),
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
        if matches!(self.action, PendingDocumentEdit::UpdatePreedit(_)) {
            if let (Some(active), Some(display)) =
                (self.active_composition()?, self.candidate_display.clone())
                && let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut()
            {
                candidate_ui.show(&self.context, &active.range, ec, display);
            }
        } else if let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut() {
            candidate_ui.end();
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
    SyntheticHost,
}

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
struct TsfTextService {
    activation: Mutex<ActivationState>,
    composition: Rc<RefCell<CompositionSession>>,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    candidate_provider: Option<Arc<dyn CandidateProvider>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
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
            candidate_ui: Rc::new(RefCell::new(CandidateUiController::new(matches!(
                key_advice_mode,
                KeyAdviceMode::Foreground
            )))),
            edit_telemetry: Arc::new(Mutex::new(EditSessionTelemetry::default())),
            key_advice_mode,
        }
    }

    #[cfg(test)]
    fn counted_for_process_test(candidate_provider: Option<Arc<dyn CandidateProvider>>) -> Self {
        Self::counted_with_options(candidate_provider, KeyAdviceMode::SyntheticHost)
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
            KeyAdviceMode::SyntheticHost => KeyModifiers::default(),
        }
    }

    fn request_document_edit_session(
        &self,
        context: &ITfContext,
        client_id: u32,
        action: PendingDocumentEdit,
        candidate_display: Option<CandidateDisplay>,
        mode: EditSessionMode,
        cleanup_target: Option<ITfComposition>,
    ) -> Result<()> {
        let edit_session: ITfEditSession = TsfDocumentEditSession::counted(
            context.clone(),
            action,
            EditSessionShared {
                document_composition: Rc::clone(&self.document_composition),
                logical_composition: Rc::clone(&self.composition),
                telemetry: Arc::clone(&self.edit_telemetry),
                candidate_ui: Rc::clone(&self.candidate_ui),
            },
            candidate_display,
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
            None,
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
        if let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut() {
            candidate_ui.end();
        }
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
                    KeyAdviceMode::SyntheticHost => Ok(None),
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
        let ui_thread_manager = thread_manager.clone();
        activation.thread_manager = Some(thread_manager);
        activation.keystroke_manager = keystroke_manager;
        activation.thread_source = Some(thread_source);
        activation.thread_event_cookie = Some(thread_event_cookie);
        activation.client_id = client_id;
        activation.flags = flags;
        activation.activating = false;
        drop(activation);
        if let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut() {
            candidate_ui.activate(&ui_thread_manager);
        }
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
        let needs_existing_candidates = matches!(
            &input,
            CompositionInput::Confirm
                | CompositionInput::Select(_)
                | CompositionInput::PreviousPage
                | CompositionInput::NextPage
        );
        let existing_candidates = if needs_existing_candidates && !session.phonetic().is_empty() {
            provider.candidates(session.phonetic(), CANDIDATE_LIMIT)
        } else {
            Vec::new()
        };
        let selected_text = match input {
            CompositionInput::Confirm => existing_candidates.first().cloned(),
            CompositionInput::Select(rank) => {
                let absolute = session
                    .candidate_page_start()
                    .saturating_add(rank.saturating_sub(1));
                existing_candidates.get(absolute).cloned()
            }
            _ => None,
        };
        let mut plan =
            match plan_session_input(&session, input, selected_text, existing_candidates.len()) {
                Some(plan) => plan,
                None => return Ok(None),
            };
        if !plan.after.phonetic().is_empty() {
            let candidates =
                if plan.after.phonetic() == session.phonetic() && !existing_candidates.is_empty() {
                    existing_candidates
                } else {
                    provider.candidates(plan.after.phonetic(), CANDIDATE_LIMIT)
                };
            plan.candidate_display = Some(CandidateDisplay::from_candidates(
                candidates,
                plan.after.candidate_page_start(),
            ));
        }
        Ok(Some(plan))
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

        let PlannedKey {
            before,
            after,
            edit,
            candidate_display,
        } = plan;
        let ui_only = edit.is_none();
        if let Some(edit) = edit {
            self.request_document_edit_session(
                &context,
                client_id,
                edit,
                candidate_display.clone(),
                EditSessionMode::KeySynchronous,
                None,
            )?;
        }

        let mut composition = self
            .composition
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if *composition != before {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        *composition = after;
        drop(composition);
        if ui_only
            && let Some(display) = candidate_display
            && let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut()
        {
            candidate_ui.update_contents(display);
        }
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
        let candidate_ui_result = match self.candidate_ui.try_borrow_mut() {
            Ok(mut candidate_ui) => {
                candidate_ui.deactivate();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
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
        composition_result?;
        candidate_ui_result
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

    let factory = match TsfClassFactory::counted() {
        Ok(factory) => factory,
        Err(_) => return E_UNEXPECTED,
    };
    let factory: IClassFactory = factory.into();
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
    use std::fs;
    use std::sync::atomic::AtomicU64;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::TextServices::{
        CLSID_TF_ThreadMgr, ITfCompositionView, ITfContextOwnerCompositionServices, TF_ES_READ,
        TF_POPF_ALL, TF_TF_MOVESTART,
    };
    use windows::core::ComObject;

    #[test]
    fn runtime_root_is_fixed_beside_the_loaded_module() {
        let root = module_candidate_runtime_root().unwrap();
        assert_eq!(
            root.file_name().and_then(|name| name.to_str()),
            Some(CANDIDATE_RUNTIME_DIRECTORY)
        );
    }

    #[test]
    fn present_invalid_runtime_root_never_falls_back_to_embedded_data() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-invalid-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let result = candidate_provider_for_root(&root);
        fs::remove_dir(&root).unwrap();
        assert!(matches!(
            result,
            Err(CandidateProviderLoadError::Runtime(
                CandidateRuntimeError::SlotStateUnavailable
            ))
        ));
    }

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        SYNTHETIC_HOST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct FixedCandidateProvider;

    impl CandidateProvider for FixedCandidateProvider {
        fn candidates(&self, code: &str, limit: usize) -> Vec<String> {
            if code == "a" && limit > 0 {
                vec!["啊".to_owned()]
            } else {
                Vec::new()
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
    fn class_factory_rejects_service_creation_when_snapshot_is_unavailable() {
        let _guard = test_lock();
        assert_eq!(DllCanUnloadNow(), S_OK);
        let factory: IClassFactory = TsfClassFactory::counted_with_options(
            Err(CandidateProviderLoadError::Embedded(
                CandidatePackageError::UnsupportedSchema,
            )),
            KeyAdviceMode::SyntheticHost,
        )
        .into();
        // SAFETY: aggregation is disabled and the requested interface is
        // valid; the unavailable snapshot is the intended failure.
        let result: Result<ITfTextInputProcessorEx> =
            unsafe { factory.CreateInstance(None::<&IUnknown>) };
        let error = result.expect_err("candidate snapshot failure must reject service creation");
        assert_eq!(error.code(), E_UNEXPECTED);
        drop(factory);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn development_provider_decodes_public_fixture_and_preserves_unknown_input() {
        let provider = development_candidate_provider().unwrap();
        let candidates = provider.candidates("nihk", CANDIDATE_LIMIT);
        assert_eq!(candidates.first().map(String::as_str), Some("你好"));
        assert!(candidates.len() <= CANDIDATE_LIMIT);
        assert!(provider.candidates("nihk", 0).is_empty());
        assert_eq!(provider.candidates("zzzzzzzz", 1), ["zzzzzzzz"]);
    }

    #[test]
    fn public_preflight_api_commits_snapshot_candidate_without_retaining_text() {
        let manifest = CandidatePackageManifest::parse(TSF_DEVELOPMENT_MANIFEST).unwrap();
        let snapshot = Arc::new(manifest.load_snapshot(TSF_DEVELOPMENT_LEXICON).unwrap());
        let report = preflight_candidate_snapshot(snapshot, "nihk", "你好").unwrap();
        assert_eq!(report.revision(), "tsf-public-demo-v1");
        assert_eq!(report.input_keys(), 4);
        assert_eq!(report.committed_characters(), 2);
        assert_eq!(
            format!("{report:?}"),
            "TsfCandidatePreflightReport { revision: \"tsf-public-demo-v1\", input_keys: 4, committed_characters: 2 }"
        );

        let manifest = CandidatePackageManifest::parse(TSF_DEVELOPMENT_MANIFEST).unwrap();
        let snapshot = Arc::new(manifest.load_snapshot(TSF_DEVELOPMENT_LEXICON).unwrap());
        assert_eq!(
            preflight_candidate_snapshot(snapshot, "nihk", "您好").unwrap_err(),
            TsfCandidatePreflightError::CandidateMismatch
        );
    }

    #[test]
    fn unavailable_snapshot_leaves_keys_unhandled() {
        let _guard = test_lock();
        assert_eq!(DllCanUnloadNow(), S_OK);
        let service = ComObject::new(TsfTextService::counted_for_process_test(None));
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_A.0)), KeyModifiers::default())
                .unwrap()
                .is_none()
        );
        drop(service);
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
        assert_eq!(
            decode_virtual_key(VK_5.0, KeyModifiers::default()),
            Some(CompositionInput::Select(5))
        );
        assert_eq!(
            decode_virtual_key(VK_5.0 + 1, KeyModifiers::default()),
            None
        );
    }

    #[test]
    fn candidate_display_pages_and_bounds_native_text() {
        let candidates = (1..=7)
            .map(|index| format!("候选{index}"))
            .collect::<Vec<_>>();
        let display = CandidateDisplay::from_candidates(candidates, 5);
        assert_eq!(display.visible(), ["候选6", "候选7"]);
        assert_eq!(display.page_starts(), [0, 5]);
        assert_eq!(display.current_page(), 1);
        assert_eq!(display.selected_index(), 5);
        assert_eq!(display.native_text(), "1  候选6\n2  候选7");

        let long = "甲".repeat(CANDIDATE_DISPLAY_MAX_CHARS + 1);
        let clipped = CandidateDisplay::from_candidates(vec![long], 0).native_text();
        assert!(clipped.ends_with('…'));
        assert_eq!(clipped.chars().count(), 3 + CANDIDATE_DISPLAY_MAX_CHARS + 1);
    }

    #[test]
    fn page_keys_are_ui_only_session_changes() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("nihk".to_owned()));
        let next = plan_session_input(&session, CompositionInput::NextPage, None, 7).unwrap();
        assert!(next.edit.is_none());
        assert_eq!(next.after.candidate_page_start(), 5);

        let previous =
            plan_session_input(&next.after, CompositionInput::PreviousPage, None, 7).unwrap();
        assert!(previous.edit.is_none());
        assert_eq!(previous.after.candidate_page_start(), 0);
    }

    #[test]
    fn candidate_ui_element_exposes_the_same_bounded_page_without_a_window() {
        let before = ACTIVE_COM_OBJECTS.load(Ordering::Acquire);
        let state = Rc::new(RefCell::new(CandidateElementState {
            display: Some(CandidateDisplay::from_candidates(
                (1..=7)
                    .map(|index| format!("候选{index}"))
                    .collect::<Vec<_>>(),
                5,
            )),
            document_manager: None,
            shown: true,
        }));
        let popup = Rc::new(RefCell::new(CandidatePopup::default()));
        let element: ITfCandidateListUIElement =
            CandidateListElement::counted(state, Rc::downgrade(&popup)).into();
        assert_eq!(unsafe { element.GetCount() }.unwrap(), 7);
        assert_eq!(unsafe { element.GetSelection() }.unwrap(), 5);
        assert_eq!(
            unsafe { element.GetString(5) }.unwrap().to_string(),
            "候选6"
        );
        assert!(unsafe { element.GetString(7) }.is_err());
        let mut starts = [u32::MAX; 2];
        let mut page_count = 0;
        unsafe { element.GetPageIndex(&mut starts, &mut page_count) }.unwrap();
        assert_eq!(starts, [0, 5]);
        assert_eq!(page_count, 2);
        assert_eq!(unsafe { element.GetCurrentPage() }.unwrap(), 1);
        unsafe { element.SetPageIndex(&starts) }.unwrap();
        unsafe { element.Show(false) }.unwrap();
        assert!(!unsafe { element.IsShown() }.unwrap().as_bool());
        drop(element);
        assert_eq!(ACTIVE_COM_OBJECTS.load(Ordering::Acquire), before);
    }
}
