//! Build-only Windows TSF COM and composition probe.
//!
//! This module intentionally exports no registration functions. It proves
//! class-factory, activation, deactivation, server-lock, and unload behavior
//! without adding an input profile to Windows.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::error::Error as StdError;
use std::ffi::{OsString, c_void};
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak as SyncWeak};
use std::time::Instant;

use crate::{
    CANDIDATE_RUNTIME_DIRECTORY, CandidatePackageError, CandidatePackageManifest,
    CandidateRuntimeError, CandidateSnapshot, CompositionEffect, CompositionInput,
    CompositionPunctuation, CompositionSession, DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
    DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS, Decoder, ExplicitAliasSnapshot,
    MAX_CANDIDATE_SNAPSHOT_RANK, NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS,
    NativeCancellationSource, NativeCandidateView, NativeFeedbackAuthorization,
    NativeFeedbackClearResult, NativeFeedbackContext, NativeFeedbackEvent,
    NativeFeedbackFreezeAuthorization, NativeFeedbackLifecycle, NativeFeedbackLimits,
    NativeFeedbackRecordResult, NativeFeedbackSession, NativeFeedbackStartResult,
    NativeFeedbackStopResult, NativeFeedbackSummary, NativeSelectionSource, SessionSelectionMemory,
    WISH_ACK_COMPARTMENT_GUID, WISH_COMMAND_COMPARTMENT_GUID, WindowsUserDataProtector,
    WishCommand, WishCommandAck, WishCommandAckStatus, WishSnapshot,
    load_current_candidate_snapshot, load_current_explicit_alias_snapshot,
    load_explicit_alias_slot_state, parse_lexicon_tsv, save_wish_snapshot,
};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, COLORREF, E_INVALIDARG, E_POINTER,
    E_UNEXPECTED, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, S_FALSE, S_OK, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect, FillRgn,
    GetMonitorInfoW, GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    RGN_DIFF, RGN_ERROR, SRCCOPY, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IClassFactory, IClassFactory_Impl,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::Ole::{
    CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_0, VK_1, VK_6, VK_9, VK_A, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_ESCAPE, VK_LWIN,
    VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7,
    VK_OEM_8, VK_OEM_102, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR,
    VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_Z,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, GUID_COMPARTMENT_EMPTYCONTEXT, GUID_COMPARTMENT_KEYBOARD_DISABLED,
    GUID_LBI_INPUTMODE, GUID_PROP_INPUTSCOPE, IS_ALPHANUMERIC_PIN, IS_ALPHANUMERIC_PIN_SET,
    IS_CHAT, IS_CHAT_WITHOUT_EMOJI, IS_CHINESE_FULLWIDTH, IS_CHINESE_HALFWIDTH, IS_DEFAULT,
    IS_NATIVE_SCRIPT, IS_NUMERIC_PASSWORD, IS_NUMERIC_PIN, IS_PASSWORD, IS_PRIVATE, IS_SEARCH,
    IS_SEARCH_INCREMENTAL, IS_TEXT, ITfCandidateListUIElement, ITfCandidateListUIElement_Impl,
    ITfCompartment, ITfCompartmentEventSink, ITfCompartmentEventSink_Impl, ITfCompartmentMgr,
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfContextComposition,
    ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl, ITfInputScope, ITfInsertAtSelection,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItem, ITfLangBarItem_Impl,
    ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemMgr, ITfLangBarItemSink,
    ITfMenu, ITfRange, ITfSource, ITfSource_Impl, ITfTextInputProcessor_Impl,
    ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
    ITfThreadMgrEventSink_Impl, ITfUIElement, ITfUIElement_Impl, ITfUIElementMgr, InputScope,
    TF_AE_NONE, TF_ANCHOR_END, TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR,
    TF_CLUIE_PAGEINDEX, TF_CLUIE_SELECTION, TF_CLUIE_STRING, TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    TF_ES_ASYNC, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_NO_DEFAULT_COMPOSITION,
    TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_ICON, TF_LBI_STATUS,
    TF_LBI_STATUS_DISABLED, TF_LBI_STATUS_HIDDEN, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_BTN_MENU,
    TF_LBI_STYLE_SHOWNINTRAY, TF_LBI_STYLE_TEXTCOLORICON, TF_LBI_TEXT, TF_LBI_TOOLTIP,
    TF_LBMENUF_CHECKED, TF_LBMENUF_GRAYED, TF_LBMENUF_SEPARATOR, TF_POPF_ALL, TF_SELECTION,
    TF_SELECTIONSTYLE, TF_TF_MOVESTART, TfLBIClick,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, GWLP_USERDATA, GWLP_WNDPROC, GetClientRect, GetForegroundWindow,
    GetWindowLongPtrW, HICON, HMENU, HWND_TOPMOST, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, SET_WINDOW_POS_FLAGS, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT,
    WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{
    BSTR, Error, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, Result, implement,
    w,
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
const TSF_TECHNICAL_OVERLAY: &str = include_str!("../data/public/ziranma-technical-overlay-v1.tsv");
const TSF_CONVERSATION_OVERLAY: &str =
    include_str!("../data/public/ziranma-conversation-overlay-v1.tsv");
const CANDIDATE_PAGE_SIZE: usize = 6;
const CANDIDATE_LIMIT: usize = MAX_CANDIDATE_SNAPSHOT_RANK;
const CANDIDATE_DISPLAY_MAX_CHARS: usize = 32;
const INLINE_WISH_TRIGGER_CODE: &str = "xuy";
const CANDIDATE_UI_GUID: GUID = GUID::from_u128(0xb9fdad61_3f19_4d6c_86f7_72e9d3064f84);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InteractiveCandidateView {
    #[default]
    Primary,
    TranspositionRecovery,
}

trait CandidateProvider: Send + Sync {
    /// Returns one deterministic, bounded candidate page without learning or
    /// I/O. Implementations should decode once rather than once per rank.
    fn candidates(&self, code: &str, limit: usize, view: InteractiveCandidateView) -> Vec<String>;

    /// Checks a small version pointer only at a new-composition boundary.
    /// Invalid updates retain the last known-good in-memory snapshot.
    fn refresh_at_safe_boundary(&self) -> bool {
        false
    }
}

#[derive(Clone, Default)]
struct CandidateBatch {
    candidates: Vec<String>,
    may_have_more: bool,
    view: InteractiveCandidateView,
}

#[derive(Default)]
struct CandidateCache {
    code: String,
    view: InteractiveCandidateView,
    candidates: Vec<String>,
    requested_limit: usize,
    exhausted: bool,
}

impl CandidateCache {
    fn load(
        &mut self,
        provider: &dyn CandidateProvider,
        code: &str,
        requested_limit: usize,
        view: InteractiveCandidateView,
    ) -> CandidateBatch {
        let requested_limit = requested_limit.min(CANDIDATE_LIMIT);
        let reusable = self.code == code
            && self.view == view
            && (self.exhausted || self.requested_limit >= requested_limit);
        if !reusable {
            let candidates = provider.candidates(code, requested_limit, view);
            self.code.clear();
            self.code.push_str(code);
            self.view = view;
            self.exhausted = candidates.len() < requested_limit;
            self.requested_limit = requested_limit;
            self.candidates = candidates;
        }
        CandidateBatch {
            candidates: self.candidates.clone(),
            may_have_more: !self.exhausted && self.requested_limit < CANDIDATE_LIMIT,
            view,
        }
    }
}

fn candidate_visible_limit(page_start: usize) -> usize {
    page_start
        .saturating_add(CANDIDATE_PAGE_SIZE)
        .clamp(CANDIDATE_PAGE_SIZE, CANDIDATE_LIMIT)
}

fn candidate_next_page_limit(page_start: usize) -> usize {
    page_start
        .saturating_add(CANDIDATE_PAGE_SIZE.saturating_mul(2))
        .clamp(CANDIDATE_PAGE_SIZE.saturating_mul(2), CANDIDATE_LIMIT)
}

type CandidateProviderLoadResult =
    std::result::Result<CandidateProviderBlueprint, CandidateProviderLoadError>;

#[derive(Clone, Debug, Eq, PartialEq)]
enum CandidateProviderLoadError {
    Embedded(CandidatePackageError),
    Runtime(CandidateRuntimeError),
    ModuleLocation,
}

struct SnapshotCandidateProvider {
    snapshot: Arc<CandidateSnapshot>,
    aliases: Option<ExplicitAliasRuntime>,
}

impl SnapshotCandidateProvider {
    fn new(snapshot: Arc<CandidateSnapshot>, alias_root: Option<PathBuf>) -> Self {
        Self {
            snapshot,
            aliases: alias_root.map(ExplicitAliasRuntime::new),
        }
    }
}

#[derive(Clone)]
struct CandidateProviderBlueprint {
    snapshot: Arc<CandidateSnapshot>,
    alias_root: Option<PathBuf>,
}

impl CandidateProviderBlueprint {
    fn build(&self) -> Arc<dyn CandidateProvider> {
        Arc::new(SnapshotCandidateProvider::new(
            Arc::clone(&self.snapshot),
            self.alias_root.clone(),
        ))
    }
}

struct ExplicitAliasRuntime {
    root: PathBuf,
    state: Mutex<ExplicitAliasRuntimeState>,
}

struct ExplicitAliasRuntimeState {
    package_id: Option<String>,
    snapshot: Arc<ExplicitAliasSnapshot>,
}

impl ExplicitAliasRuntime {
    fn new(root: PathBuf) -> Self {
        let runtime = Self {
            root,
            state: Mutex::new(ExplicitAliasRuntimeState {
                package_id: None,
                snapshot: Arc::new(ExplicitAliasSnapshot::default()),
            }),
        };
        let _ = runtime.refresh();
        runtime
    }

    fn refresh(&self) -> bool {
        let next_id = match load_explicit_alias_slot_state(&self.root) {
            Ok(Some(state)) => state.current().map(str::to_owned),
            Ok(None) => None,
            Err(_) => return false,
        };
        if self
            .state
            .lock()
            .map(|state| state.package_id == next_id)
            .unwrap_or(true)
        {
            return false;
        }
        let next_snapshot = match next_id.as_deref() {
            Some(_) => {
                match load_current_explicit_alias_snapshot(&self.root, &WindowsUserDataProtector) {
                    Ok(Some(loaded)) if Some(loaded.package_id()) == next_id.as_deref() => {
                        loaded.into_snapshot()
                    }
                    _ => return false,
                }
            }
            None => Arc::new(ExplicitAliasSnapshot::default()),
        };
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.package_id == next_id {
            return false;
        }
        state.package_id = next_id;
        state.snapshot = next_snapshot;
        true
    }

    fn text(&self, code: &str) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.snapshot.get(code).map(str::to_owned))
    }
}

impl CandidateProvider for SnapshotCandidateProvider {
    fn candidates(&self, code: &str, limit: usize, view: InteractiveCandidateView) -> Vec<String> {
        match view {
            InteractiveCandidateView::Primary => {
                let mut candidates = Vec::new();
                let mut seen = HashSet::new();
                if let Some(alias) = self.aliases.as_ref().and_then(|aliases| aliases.text(code)) {
                    seen.insert(alias.clone());
                    candidates.push(alias);
                }
                for candidate in project_overlay_decoder()
                    .decode_exact_full_code(code, limit)
                    .unwrap_or_default()
                {
                    if seen.insert(candidate.text.clone()) {
                        candidates.push(candidate.text);
                    }
                }
                for candidate in self
                    .snapshot
                    .candidate_texts(code, limit)
                    .unwrap_or_default()
                {
                    if seen.insert(candidate.clone()) {
                        candidates.push(candidate);
                    }
                    if candidates.len() == limit {
                        break;
                    }
                }
                candidates.truncate(limit);
                candidates
            }
            InteractiveCandidateView::TranspositionRecovery => self
                .snapshot
                .transposition_recovery_texts(code, limit)
                .unwrap_or_default(),
        }
    }

    fn refresh_at_safe_boundary(&self) -> bool {
        self.aliases
            .as_ref()
            .is_some_and(ExplicitAliasRuntime::refresh)
    }
}

fn project_overlay_decoder() -> &'static Decoder {
    static DECODER: OnceLock<Decoder> = OnceLock::new();
    DECODER.get_or_init(|| {
        let mut entries = parse_lexicon_tsv(TSF_CONVERSATION_OVERLAY)
            .expect("the project-owned conversation overlay must remain valid");
        entries.extend(
            parse_lexicon_tsv(TSF_TECHNICAL_OVERLAY)
                .expect("the project-owned technical overlay must remain valid"),
        );
        Decoder::new(entries)
    })
}

#[derive(Clone, Default)]
struct CandidateDisplay {
    candidates: Vec<String>,
    page_start: usize,
    may_have_more: bool,
    view: InteractiveCandidateView,
    action_detail: Option<String>,
}

impl CandidateDisplay {
    fn action(label: &str, detail: &str) -> Self {
        Self {
            candidates: vec![label.to_owned()],
            page_start: 0,
            may_have_more: false,
            view: InteractiveCandidateView::Primary,
            action_detail: Some(detail.to_owned()),
        }
    }

    #[cfg(test)]
    fn from_candidates(candidates: Vec<String>, requested_page_start: usize) -> Self {
        Self::from_batch(
            CandidateBatch {
                candidates,
                may_have_more: false,
                view: InteractiveCandidateView::Primary,
            },
            requested_page_start,
        )
    }

    fn from_batch(batch: CandidateBatch, requested_page_start: usize) -> Self {
        let CandidateBatch {
            candidates,
            may_have_more,
            view,
        } = batch;
        let page_start = if candidates.is_empty() {
            0
        } else {
            requested_page_start
                .min((candidates.len() - 1) / CANDIDATE_PAGE_SIZE * CANDIDATE_PAGE_SIZE)
        };
        Self {
            candidates,
            page_start,
            may_have_more,
            view,
            action_detail: None,
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

    fn may_have_more(&self) -> bool {
        self.may_have_more
    }

    fn view(&self) -> InteractiveCandidateView {
        self.view
    }

    fn action_detail(&self) -> Option<&str> {
        self.action_detail.as_deref()
    }

    fn feedback_event(&self, code: &str, shape_mode: bool) -> NativeFeedbackEvent {
        NativeFeedbackEvent::CandidatesPresented {
            code: code.to_owned(),
            view: native_candidate_view(self.view, shape_mode),
            page_start: self.page_start,
            candidates: self.visible().to_vec(),
            may_have_more: self.may_have_more,
        }
    }

    #[cfg(test)]
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

fn native_candidate_view(view: InteractiveCandidateView, shape_mode: bool) -> NativeCandidateView {
    if shape_mode {
        NativeCandidateView::Shape
    } else {
        match view {
            InteractiveCandidateView::Primary => NativeCandidateView::Ordinary,
            InteractiveCandidateView::TranspositionRecovery => {
                NativeCandidateView::TranspositionRecovery
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InlineWishAction {
    command: WishCommand,
    label: &'static str,
    detail: &'static str,
}

fn inline_wish_action(summary: NativeFeedbackSummary) -> InlineWishAction {
    match summary.lifecycle {
        NativeFeedbackLifecycle::Disabled => InlineWishAction {
            command: WishCommand::Start,
            label: "开始反馈",
            detail: "暂不保存",
        },
        NativeFeedbackLifecycle::Recording => InlineWishAction {
            command: WishCommand::SaveRecent,
            label: "向猫猫许愿",
            detail: "近30秒",
        },
        NativeFeedbackLifecycle::Stopped => InlineWishAction {
            command: WishCommand::ClearStopped,
            label: "清除反馈",
            detail: "已停止",
        },
    }
}

fn development_candidate_blueprint() -> CandidateProviderLoadResult {
    static BLUEPRINT: OnceLock<CandidateProviderLoadResult> = OnceLock::new();
    BLUEPRINT
        .get_or_init(|| {
            let manifest = CandidatePackageManifest::parse(TSF_DEVELOPMENT_MANIFEST)
                .map_err(CandidateProviderLoadError::Embedded)?;
            let snapshot = Arc::new(
                manifest
                    .load_snapshot(TSF_DEVELOPMENT_LEXICON)
                    .map_err(CandidateProviderLoadError::Embedded)?,
            );
            Ok(CandidateProviderBlueprint {
                snapshot,
                alias_root: None,
            })
        })
        .clone()
}

#[cfg(test)]
fn development_candidate_provider()
-> std::result::Result<Arc<dyn CandidateProvider>, CandidateProviderLoadError> {
    development_candidate_blueprint().map(|blueprint| blueprint.build())
}

fn candidate_provider_for_root(
    root: &Path,
    alias_root: Option<PathBuf>,
) -> CandidateProviderLoadResult {
    match load_current_candidate_snapshot(root).map_err(CandidateProviderLoadError::Runtime)? {
        Some(snapshot) => Ok(CandidateProviderBlueprint {
            snapshot,
            alias_root,
        }),
        None => development_candidate_blueprint().map(|mut blueprint| {
            blueprint.alias_root = alias_root;
            blueprint
        }),
    }
}

fn module_path() -> std::result::Result<PathBuf, CandidateProviderLoadError> {
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
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn module_candidate_runtime_root() -> std::result::Result<PathBuf, CandidateProviderLoadError> {
    let module_path = module_path()?;
    let parent = module_path
        .parent()
        .ok_or(CandidateProviderLoadError::ModuleLocation)?;
    Ok(parent.join(CANDIDATE_RUNTIME_DIRECTORY))
}

fn installed_user_data_root_for_module(module_path: &Path, leaf: &str) -> Option<PathBuf> {
    let build = module_path.parent()?;
    let builds = build.parent()?;
    let tsf_alpha = builds.parent()?;
    let digest = build.file_name()?.to_str()?;
    if builds.file_name()?.to_str()? != "builds"
        || tsf_alpha.file_name()?.to_str()? != "tsf-alpha"
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    if leaf.is_empty()
        || !leaf
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || *byte == b'-')
    {
        return None;
    }
    Some(tsf_alpha.join("user-data").join(leaf))
}

fn explicit_alias_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, "aliases")
}

fn wish_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, "wishes")
}

fn class_factory_candidate_provider() -> CandidateProviderLoadResult {
    let root = module_candidate_runtime_root()?;
    let alias_root = explicit_alias_root_for_module(&module_path()?);
    candidate_provider_for_root(&root, alias_root)
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
    let factory: IClassFactory = TsfClassFactory::counted_with_options(
        Ok(CandidateProviderBlueprint {
            snapshot,
            alias_root: None,
        }),
        KeyAdviceMode::SyntheticHost,
    )
    .into();
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
    Insert,
}

#[derive(Clone)]
enum PendingDocumentEdit {
    UpdatePreedit(String),
    Cancel,
    Commit(String),
    Insert(String),
}

impl PendingDocumentEdit {
    fn kind(&self) -> DocumentEditKind {
        match self {
            Self::UpdatePreedit(_) => DocumentEditKind::UpdatePreedit,
            Self::Cancel => DocumentEditKind::Cancel,
            Self::Commit(_) => DocumentEditKind::Commit,
            Self::Insert(_) => DocumentEditKind::Insert,
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
    selection_to_remember: Option<PlannedSelection>,
    feedback_after_success: Option<NativeFeedbackEvent>,
    action_after_success: Option<PlannedAction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedAction {
    Wish(WishCommand),
}

struct PlannedSelection {
    code: String,
    text: String,
}

#[derive(Clone, Copy, Default)]
struct KeyModifiers {
    shift: bool,
    caps_lock: bool,
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
        // The low bit is the documented toggle state for lock keys.
        caps_lock: unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) & 1 != 0 },
        control: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        windows: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InputMode {
    #[default]
    Chinese,
    English,
}

impl InputMode {
    fn toggled(self) -> Self {
        match self {
            Self::Chinese => Self::English,
            Self::English => Self::Chinese,
        }
    }
}

fn is_letter_key(vkey: u16) -> bool {
    (VK_A.0..=VK_Z.0).contains(&vkey)
}

fn is_host_printable_key(vkey: u16) -> bool {
    is_letter_key(vkey)
        || (VK_0.0..=VK_9.0).contains(&vkey)
        || matches!(
            vkey,
            key if key == VK_OEM_1.0
                || key == VK_OEM_PLUS.0
                || key == VK_OEM_COMMA.0
                || key == VK_OEM_MINUS.0
                || key == VK_OEM_PERIOD.0
                || key == VK_OEM_2.0
                || key == VK_OEM_3.0
                || key == VK_OEM_4.0
                || key == VK_OEM_5.0
                || key == VK_OEM_6.0
                || key == VK_OEM_7.0
                || key == VK_OEM_8.0
                || key == VK_OEM_102.0
        )
}

fn decode_virtual_key(
    vkey: u16,
    modifiers: KeyModifiers,
    input_mode: InputMode,
) -> Option<CompositionInput> {
    if modifiers.control || modifiers.alt || modifiers.windows {
        return None;
    }
    if input_mode == InputMode::English {
        return None;
    }
    if vkey == VK_TAB.0 && modifiers.shift {
        return Some(CompositionInput::EnterRecovery);
    }
    match vkey {
        key if key == VK_BACK.0 => Some(CompositionInput::Backspace),
        key if key == VK_TAB.0 => Some(CompositionInput::EnterTab),
        key if key == VK_RETURN.0 => Some(CompositionInput::CommitRaw),
        key if key == VK_SPACE.0 => Some(CompositionInput::Confirm),
        key if key == VK_OEM_COMMA.0 && !modifiers.shift => {
            Some(CompositionInput::Punctuation(CompositionPunctuation::Comma))
        }
        key if key == VK_OEM_PERIOD.0 && !modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::Period,
        )),
        key if key == VK_OEM_1.0 && !modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::Semicolon,
        )),
        key if key == VK_OEM_1.0 && modifiers.shift => {
            Some(CompositionInput::Punctuation(CompositionPunctuation::Colon))
        }
        key if key == VK_1.0 && modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::ExclamationMark,
        )),
        key if key == VK_9.0 && modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::LeftParenthesis,
        )),
        key if key == VK_0.0 && modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::RightParenthesis,
        )),
        key if key == VK_OEM_2.0 && modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::QuestionMark,
        )),
        key if key == VK_ESCAPE.0 => Some(CompositionInput::Escape),
        key if key == VK_PRIOR.0 || key == VK_OEM_MINUS.0 => Some(CompositionInput::PreviousPage),
        key if key == VK_NEXT.0 || key == VK_OEM_PLUS.0 => Some(CompositionInput::NextPage),
        key if is_letter_key(key) && !modifiers.shift && !modifiers.caps_lock => {
            Some(CompositionInput::Letters(
                char::from(b'a' + u8::try_from(key - VK_A.0).expect("A-Z offset fits u8"))
                    .to_string(),
            ))
        }
        key if (VK_1.0..=VK_6.0).contains(&key) && !modifiers.shift => {
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
        (_, CompositionEffect::Continue) if before.wish_prompt() && after.wish_prompt() => None,
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
        (CompositionInput::CommitRaw, CompositionEffect::CommitRaw) => {
            let text = before.phonetic().to_owned();
            after.finish_commit();
            Some(PendingDocumentEdit::Commit(text))
        }
        (
            CompositionInput::Confirm | CompositionInput::Select(1),
            CompositionEffect::ConfirmWish,
        ) => {
            after.finish_commit();
            Some(PendingDocumentEdit::Cancel)
        }
        (
            CompositionInput::Punctuation(punctuation),
            CompositionEffect::Punctuation(effect_punctuation),
        ) if punctuation == effect_punctuation => {
            if before.phonetic().is_empty() {
                Some(PendingDocumentEdit::Insert(punctuation.text().to_owned()))
            } else {
                let mut text = selected_text.filter(|text| !text.is_empty())?;
                text.push_str(punctuation.text());
                after.finish_commit();
                Some(PendingDocumentEdit::Commit(text))
            }
        }
        (CompositionInput::PreviousPage, CompositionEffect::PreviousPage) => {
            after.previous_candidate_page(CANDIDATE_PAGE_SIZE);
            None
        }
        (CompositionInput::NextPage, CompositionEffect::NextPage) => {
            after.next_candidate_page(candidate_count, CANDIDATE_PAGE_SIZE, CANDIDATE_LIMIT);
            None
        }
        (
            CompositionInput::EnterWish
            | CompositionInput::EnterRecovery
            | CompositionInput::EnterTab
            | CompositionInput::Backspace
            | CompositionInput::Escape,
            CompositionEffect::Continue,
        ) if before.recovery_mode() != after.recovery_mode()
            || before.wish_prompt() != after.wish_prompt() =>
        {
            None
        }
        _ => return None,
    };
    Some(PlannedKey {
        before,
        after,
        edit,
        candidate_display: None,
        selection_to_remember: None,
        feedback_after_success: None,
        action_after_success: None,
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
            development_candidate_blueprint(),
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
            Some(candidate_provider.build()),
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CandidatePopupLayout {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CandidatePopupMetrics {
    layout: CandidatePopupLayout,
    width: i32,
    height: i32,
}

fn popup_scale(dpi: u32, logical: i32) -> i32 {
    i32::try_from(
        i64::from(logical)
            .saturating_mul(i64::from(dpi.max(96)))
            .saturating_add(48)
            / 96,
    )
    .unwrap_or(i32::MAX)
}

const POPUP_OUTER_PADDING_LOGICAL: i32 = 5;
const POPUP_ROW_HEIGHT_LOGICAL: i32 = 36;
const POPUP_TEXT_PADDING_LOGICAL: i32 = 7;
const POPUP_SELECTED_TEXT_INSET_LOGICAL: i32 = 13;
const POPUP_RANK_WIDTH_LOGICAL: i32 = 16;
const POPUP_RANK_GAP_LOGICAL: i32 = 4;
const POPUP_METADATA_BASELINE_OFFSET_LOGICAL: i32 = 2;
const POPUP_CANDIDATE_CHROME_WIDTH_LOGICAL: i32 = 42;
const POPUP_FOOTER_CONTENT_INSET_LOGICAL: i32 = 10;
const POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL: i32 = 640;
const POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL: i32 = 54;
const POPUP_ACTION_MIN_WIDTH_LOGICAL: i32 = 210;
const POPUP_ACTION_DETAIL_GAP_LOGICAL: i32 = 12;
const POPUP_CORNER_DIAMETER_LOGICAL: i32 = 16;
const POPUP_BORDER_WIDTH_LOGICAL: i32 = 1;

fn candidate_popup_corner_diameter(dpi: u32) -> i32 {
    popup_scale(dpi, POPUP_CORNER_DIAMETER_LOGICAL)
}

#[derive(Clone, Copy, Debug)]
struct CandidatePopupBorderGeometry {
    outer: RECT,
    inner: RECT,
    outer_corner_diameter: i32,
    inner_corner_diameter: i32,
}

fn candidate_popup_border_geometry(client: RECT, dpi: u32) -> Option<CandidatePopupBorderGeometry> {
    let border_width = popup_scale(dpi, POPUP_BORDER_WIDTH_LOGICAL).max(1);
    let inner = RECT {
        left: client.left.saturating_add(border_width),
        top: client.top.saturating_add(border_width),
        right: client.right.saturating_sub(border_width),
        bottom: client.bottom.saturating_sub(border_width),
    };
    if inner.right <= inner.left || inner.bottom <= inner.top {
        return None;
    }
    let outer_corner_diameter = candidate_popup_corner_diameter(dpi);
    Some(CandidatePopupBorderGeometry {
        outer: client,
        inner,
        outer_corner_diameter,
        inner_corner_diameter: outer_corner_diameter
            .saturating_sub(border_width.saturating_mul(2))
            .max(1),
    })
}

fn horizontal_candidate_logical_width(candidate: &str) -> i32 {
    let text_width = candidate
        .chars()
        .take(CANDIDATE_DISPLAY_MAX_CHARS)
        .fold(0_i32, |width, character| {
            width.saturating_add(if character.is_ascii() { 9 } else { 18 })
        })
        .clamp(18, 144);
    POPUP_CANDIDATE_CHROME_WIDTH_LOGICAL.saturating_add(text_width)
}

fn estimated_popup_text_width(text: &str, ascii_width: i32, other_width: i32) -> i32 {
    text.chars().fold(0_i32, |width, character| {
        width.saturating_add(if character.is_ascii() {
            ascii_width
        } else {
            other_width
        })
    })
}

fn action_popup_logical_width(label: &str, detail: &str) -> i32 {
    let label_width = estimated_popup_text_width(label, 9, 18);
    let detail_width = estimated_popup_text_width(detail, 7, 14);
    POPUP_OUTER_PADDING_LOGICAL
        .saturating_mul(2)
        .saturating_add(POPUP_SELECTED_TEXT_INSET_LOGICAL)
        .saturating_add(POPUP_RANK_WIDTH_LOGICAL)
        .saturating_add(POPUP_RANK_GAP_LOGICAL)
        .saturating_add(label_width)
        .saturating_add(POPUP_ACTION_DETAIL_GAP_LOGICAL)
        .saturating_add(detail_width)
        .saturating_add(POPUP_TEXT_PADDING_LOGICAL)
        .max(POPUP_ACTION_MIN_WIDTH_LOGICAL)
}

fn candidate_popup_footer_logical_width(display: &CandidateDisplay) -> i32 {
    match (
        display.view() == InteractiveCandidateView::TranspositionRecovery,
        display.page_starts().len() > 1,
    ) {
        (true, true) => 108,
        (true, false) => 60,
        (false, true) => 62,
        (false, false) => 0,
    }
}

fn horizontal_candidate_widths(display: &CandidateDisplay, dpi: u32, popup_width: i32) -> Vec<i32> {
    let footer_width = popup_scale(dpi, candidate_popup_footer_logical_width(display));
    let padding = popup_scale(dpi, POPUP_OUTER_PADDING_LOGICAL);
    let budget = popup_width
        .saturating_sub(padding.saturating_mul(2))
        .saturating_sub(footer_width)
        .max(0);
    if display.action_detail().is_some() {
        return vec![budget];
    }
    let mut widths = display
        .visible()
        .iter()
        .map(|candidate| popup_scale(dpi, horizontal_candidate_logical_width(candidate)))
        .collect::<Vec<_>>();
    let minimums = widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            if index == 0 {
                *width
            } else {
                popup_scale(dpi, POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL)
            }
        })
        .collect::<Vec<_>>();

    while widths.iter().copied().sum::<i32>() > budget {
        let flexible = widths
            .iter()
            .zip(&minimums)
            .filter(|(width, minimum)| width > minimum)
            .count();
        if flexible == 0 {
            break;
        }
        let excess = widths.iter().copied().sum::<i32>().saturating_sub(budget);
        let share = excess.saturating_add(i32::try_from(flexible).unwrap_or(i32::MAX) - 1)
            / i32::try_from(flexible).unwrap_or(1);
        for (width, minimum) in widths.iter_mut().zip(&minimums) {
            *width = (*width).saturating_sub(share).max(*minimum);
        }
    }
    widths
}

fn candidate_popup_metrics(
    display: &CandidateDisplay,
    dpi: u32,
    available_width: i32,
) -> CandidatePopupMetrics {
    if let (Some(label), Some(detail)) = (display.visible().first(), display.action_detail()) {
        let horizontal_limit = popup_scale(dpi, POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL)
            .min(available_width.max(1).saturating_mul(4) / 5)
            .max(1);
        return CandidatePopupMetrics {
            layout: CandidatePopupLayout::Horizontal,
            width: popup_scale(dpi, action_popup_logical_width(label, detail))
                .min(horizontal_limit),
            height: popup_scale(
                dpi,
                POPUP_OUTER_PADDING_LOGICAL
                    .saturating_mul(2)
                    .saturating_add(POPUP_ROW_HEIGHT_LOGICAL),
            ),
        };
    }
    let footer_needed = display.page_starts().len() > 1
        || display.view() == InteractiveCandidateView::TranspositionRecovery;
    let footer_width = popup_scale(dpi, candidate_popup_footer_logical_width(display));
    let outer_width = popup_scale(dpi, POPUP_OUTER_PADDING_LOGICAL.saturating_mul(2));
    let horizontal_content_width =
        display
            .visible()
            .iter()
            .fold(outer_width, |width, candidate| {
                width.saturating_add(popup_scale(
                    dpi,
                    horizontal_candidate_logical_width(candidate),
                ))
            });
    let desired_horizontal_width = horizontal_content_width
        .saturating_add(footer_width)
        .max(popup_scale(dpi, 280));
    let horizontal_limit = popup_scale(dpi, POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL)
        .min(available_width.max(1).saturating_mul(4) / 5);
    let minimum_candidate_width =
        display
            .visible()
            .iter()
            .enumerate()
            .fold(0_i32, |width, (index, candidate)| {
                width.saturating_add(popup_scale(
                    dpi,
                    if index == 0 {
                        horizontal_candidate_logical_width(candidate)
                    } else {
                        POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL
                    },
                ))
            });
    let minimum_horizontal_width = outer_width
        .saturating_add(minimum_candidate_width)
        .saturating_add(footer_width);
    if minimum_horizontal_width <= horizontal_limit {
        return CandidatePopupMetrics {
            layout: CandidatePopupLayout::Horizontal,
            width: desired_horizontal_width.min(horizontal_limit),
            height: popup_scale(
                dpi,
                POPUP_OUTER_PADDING_LOGICAL
                    .saturating_mul(2)
                    .saturating_add(POPUP_ROW_HEIGHT_LOGICAL),
            ),
        };
    }

    let rows = i32::try_from(display.visible().len()).unwrap_or(5);
    let footer_height = if footer_needed { 24 } else { 0 };
    CandidatePopupMetrics {
        layout: CandidatePopupLayout::Vertical,
        width: popup_scale(dpi, 360).min(available_width.max(1)),
        height: popup_scale(
            dpi,
            POPUP_OUTER_PADDING_LOGICAL
                .saturating_mul(2)
                .saturating_add(POPUP_ROW_HEIGHT_LOGICAL.saturating_mul(rows))
                .saturating_add(footer_height),
        ),
    }
}

#[derive(Debug)]
struct PendingCandidatePopupTiming {
    started_at: Instant,
    context: NativeFeedbackContext,
    initial_show: bool,
}

#[derive(Default)]
struct CandidatePopupPaintState {
    display: CandidateDisplay,
    dpi: u32,
    layout: CandidatePopupLayout,
    original_window_proc: isize,
    native_feedback: SyncWeak<Mutex<NativeFeedbackSession>>,
    native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    pending_timing: Option<PendingCandidatePopupTiming>,
    corner_strategy: CandidatePopupCornerStrategy,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CandidatePopupCornerStrategy {
    SystemDwm,
    #[default]
    RegionFallback,
}

impl CandidatePopupCornerStrategy {
    fn uses_custom_region(self) -> bool {
        self == Self::RegionFallback
    }
}

fn configure_candidate_popup_corners(hwnd: HWND) -> CandidatePopupCornerStrategy {
    let preference = DWMWCP_ROUND;
    let border = popup_color(POPUP_BORDER_RGB).0;
    // Windows 11 can composite an anti-aliased corner and border for a custom
    // top-level popup. Earlier systems reject either attribute, in which case
    // the existing deterministic GDI region remains the compatibility path.
    let corner_result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            u32::try_from(std::mem::size_of_val(&preference)).unwrap_or(u32::MAX),
        )
    };
    let border_result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::from_ref(&border).cast(),
            u32::try_from(std::mem::size_of_val(&border)).unwrap_or(u32::MAX),
        )
    };
    if corner_result.is_ok() && border_result.is_ok() {
        CandidatePopupCornerStrategy::SystemDwm
    } else {
        CandidatePopupCornerStrategy::RegionFallback
    }
}

impl CandidatePopupPaintState {
    fn complete_pending_timing(&mut self) {
        let Some(pending) = self.pending_timing.take() else {
            return;
        };
        let elapsed_ms = pending
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u32::MAX)) as u32;
        let Some(feedback) = self.native_feedback.upgrade() else {
            return;
        };
        let Ok(mut feedback) = feedback.lock() else {
            return;
        };
        if !feedback.is_accepting() {
            return;
        }
        let result = feedback.record_at(
            pending.context,
            NativeFeedbackEvent::CandidatePopupTiming {
                first_frame_ms: elapsed_ms,
                fully_visible_ms: elapsed_ms,
                initial_show: pending.initial_show,
            },
            native_feedback_monotonic_ms(),
        );
        drop(feedback);
        if matches!(result, NativeFeedbackRecordResult::Stopped(_))
            && let Some(state) = self.native_feedback_language_bar_state.upgrade()
        {
            state.notify();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CandidatePopupPlacement {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl CandidatePopupPlacement {
    fn size_differs_from(self, previous: Option<Self>) -> bool {
        previous
            .is_none_or(|previous| previous.width != self.width || previous.height != self.height)
    }
}

#[derive(Default)]
struct CandidatePopup {
    hwnd: Option<HWND>,
    owner: Option<HWND>,
    anchor: Option<RECT>,
    placement: Option<CandidatePopupPlacement>,
    visible: bool,
    feedback_context: NativeFeedbackContext,
    paint: Box<CandidatePopupPaintState>,
}

impl CandidatePopup {
    fn attach_feedback(
        &mut self,
        native_feedback: SyncWeak<Mutex<NativeFeedbackSession>>,
        native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    ) {
        self.paint.native_feedback = native_feedback;
        self.paint.native_feedback_language_bar_state = native_feedback_language_bar_state;
    }

    fn show(
        &mut self,
        owner: HWND,
        anchor: RECT,
        display: &CandidateDisplay,
        feedback_context: NativeFeedbackContext,
    ) -> Result<()> {
        let timing_started_at = Instant::now();
        if display.visible().is_empty() {
            self.hide();
            return Ok(());
        }
        if self.owner.is_some_and(|current| current != owner) {
            self.destroy();
        }
        let dpi = if owner.is_invalid() {
            96
        } else {
            // SAFETY: GetDpiForWindow is read-only and accepts this host-owned
            // HWND. A zero result falls back to the platform baseline.
            unsafe { GetDpiForWindow(owner) }.max(96)
        };
        self.paint.display = display.clone();
        self.paint.dpi = dpi;
        self.feedback_context = feedback_context;

        // SAFETY: the anchor is initialized screen geometry from TSF.
        let monitor = unsafe { MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST) };
        let mut monitor_info = MONITORINFO {
            cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        // SAFETY: monitor_info is writable for the duration of the call.
        let work_area = (!monitor.is_invalid()
            && unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool())
        .then_some(monitor_info.rcWork);
        let available_width = work_area
            .map(|work| work.right.saturating_sub(work.left))
            .unwrap_or_else(|| popup_scale(dpi, 1920));
        let metrics = candidate_popup_metrics(display, dpi, available_width);
        self.paint.layout = metrics.layout;

        let hwnd = match self.hwnd {
            Some(hwnd) => hwnd,
            None => {
                let ex_style =
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0);
                let style = WINDOW_STYLE(WS_POPUP.0);
                // SAFETY: STATIC is a system window class. The window is an
                // owned, nonactivating popup. It is subclassed only for its
                // own lifetime and restored by window destruction before this
                // DLL can unload.
                let created = unsafe {
                    CreateWindowExW(
                        ex_style,
                        w!("STATIC"),
                        w!(""),
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
                let paint_pointer = self.paint.as_mut() as *mut CandidatePopupPaintState as isize;
                // SAFETY: the boxed paint state has a stable address until
                // after DestroyWindow returns.
                unsafe {
                    SetWindowLongPtrW(created, GWLP_USERDATA, paint_pointer);
                }
                // SAFETY: the system STATIC procedure is non-null. The
                // returned pointer is retained for every unhandled message.
                let original = unsafe {
                    SetWindowLongPtrW(
                        created,
                        GWLP_WNDPROC,
                        candidate_popup_window_proc as *const () as isize,
                    )
                };
                if original == 0 {
                    // SAFETY: best-effort cleanup of the just-created window.
                    unsafe {
                        SetWindowLongPtrW(created, GWLP_USERDATA, 0);
                        let _ = DestroyWindow(created);
                    }
                    return Err(Error::from_thread());
                }
                self.paint.original_window_proc = original;
                self.paint.corner_strategy = configure_candidate_popup_corners(created);
                self.hwnd = Some(created);
                self.owner = Some(owner);
                created
            }
        };

        let width = metrics.width;
        let height = metrics.height;
        let gap = popup_scale(dpi, 4);
        let mut x = anchor.left;
        let mut y = anchor.bottom.saturating_add(gap);

        if let Some(work) = work_area {
            if y.saturating_add(height) > work.bottom {
                y = anchor.top.saturating_sub(height).saturating_sub(gap);
            }
            let max_x = work.right.saturating_sub(width).max(work.left);
            let max_y = work.bottom.saturating_sub(height).max(work.top);
            x = x.clamp(work.left, max_x);
            y = y.clamp(work.top, max_y);
        }

        let placement = CandidatePopupPlacement {
            x,
            y,
            width,
            height,
        };
        if self.paint.corner_strategy.uses_custom_region()
            && placement.size_differs_from(self.placement)
        {
            let corner = candidate_popup_corner_diameter(dpi);
            // SAFETY: the region uses popup-local coordinates. On success
            // Windows owns it; on failure this method retains cleanup
            // responsibility. Content-only updates reuse the existing region
            // so Windows does not erase and reshape the popup for every key.
            let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner) };
            if !region.is_invalid() {
                // SAFETY: the popup belongs to this controller. The explicit
                // invalidation below redraws the completed frame once.
                if unsafe { SetWindowRgn(hwnd, Some(region), false) } == 0 {
                    // SAFETY: ownership did not transfer after failure.
                    unsafe {
                        let _ = DeleteObject(HGDIOBJ(region.0));
                    }
                }
            }
        }
        let was_visible = self.visible;
        if self.placement != Some(placement) || !was_visible {
            let flags = SET_WINDOW_POS_FLAGS(
                SWP_NOACTIVATE.0 | if self.visible { 0 } else { SWP_SHOWWINDOW.0 },
            );
            // SAFETY: the popup belongs to this controller. NOACTIVATE
            // preserves the editor's keyboard focus while TOPMOST keeps the
            // short-lived list above its owner. Stable content-only updates
            // skip this call entirely.
            unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, flags) }?;
            self.placement = Some(placement);
            self.visible = true;
        }
        self.paint.pending_timing = Some(PendingCandidatePopupTiming {
            started_at: timing_started_at,
            context: feedback_context,
            initial_show: !was_visible,
        });
        // SAFETY: the stable paint state and final client size are now ready.
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        self.anchor = Some(anchor);
        Ok(())
    }

    fn update(
        &mut self,
        display: &CandidateDisplay,
        feedback_context: NativeFeedbackContext,
    ) -> Result<()> {
        let (Some(owner), Some(anchor)) = (self.owner, self.anchor) else {
            return Ok(());
        };
        self.show(owner, anchor, display, feedback_context)
    }

    fn set_visible(&mut self, visible: bool) {
        let Some(hwnd) = self.hwnd else {
            return;
        };
        if self.visible == visible {
            return;
        }
        if visible {
            let (Some(owner), Some(anchor)) = (self.owner, self.anchor) else {
                return;
            };
            let display = self.paint.display.clone();
            // show() owns placement and nonactivating visibility changes.
            if self
                .show(owner, anchor, &display, self.feedback_context)
                .is_err()
            {
                self.destroy();
            }
            return;
        }
        self.visible = false;
        self.paint.pending_timing = None;
        // SAFETY: candidate dismissal is immediate so committed text is never
        // covered by a fading stale list.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
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
        self.paint.original_window_proc = 0;
        self.owner = None;
        self.anchor = None;
        self.placement = None;
        self.visible = false;
    }
}

impl Drop for CandidatePopup {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn popup_rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

const POPUP_BACKGROUND_RGB: (u8, u8, u8) = (31, 32, 35);
const POPUP_SELECTED_BACKGROUND_RGB: (u8, u8, u8) = (44, 47, 53);
const POPUP_SELECTED_TEXT_RGB: (u8, u8, u8) = (250, 251, 253);
const POPUP_CANDIDATE_TEXT_RGB: (u8, u8, u8) = (218, 223, 230);
const POPUP_SELECTED_RANK_RGB: (u8, u8, u8) = (198, 205, 215);
const POPUP_RANK_RGB: (u8, u8, u8) = (143, 151, 164);
const POPUP_PAGE_RGB: (u8, u8, u8) = (130, 139, 153);
const POPUP_SELECTION_ACCENT_RGB: (u8, u8, u8) = (72, 180, 232);
const POPUP_MODE_ACCENT_RGB: (u8, u8, u8) = (147, 184, 241);
const POPUP_BORDER_RGB: (u8, u8, u8) = (58, 62, 69);
const POPUP_FOOTER_DIVIDER_RGB: (u8, u8, u8) = (66, 70, 78);

fn popup_color((red, green, blue): (u8, u8, u8)) -> COLORREF {
    popup_rgb(red, green, blue)
}

#[cfg(test)]
fn popup_attention_lightness((red, green, blue): (u8, u8, u8)) -> u32 {
    2_126_u32
        .saturating_mul(u32::from(red))
        .saturating_add(7_152_u32.saturating_mul(u32::from(green)))
        .saturating_add(722_u32.saturating_mul(u32::from(blue)))
}

#[cfg(test)]
fn popup_relative_luminance((red, green, blue): (u8, u8, u8)) -> f64 {
    let linear = |channel: u8| {
        let channel = f64::from(channel) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

#[cfg(test)]
fn popup_contrast_ratio(foreground: (u8, u8, u8), background: (u8, u8, u8)) -> f64 {
    let foreground = popup_relative_luminance(foreground);
    let background = popup_relative_luminance(background);
    (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
}

unsafe fn create_candidate_popup_font(dpi: u32, logical_height: i32, weight: u32) -> HFONT {
    // SAFETY: the fixed face name is NUL-terminated. The caller owns and
    // releases a non-null returned font.
    unsafe {
        CreateFontW(
            -popup_scale(dpi, logical_height),
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

fn candidate_label_rects(mut content: RECT, dpi: u32) -> (RECT, RECT) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let mut rank = content;
    rank.right = rank
        .left
        .saturating_add(scale(POPUP_RANK_WIDTH_LOGICAL))
        .min(rank.right);
    rank.top = rank
        .top
        .saturating_add(scale(POPUP_METADATA_BASELINE_OFFSET_LOGICAL));
    rank.bottom = rank
        .bottom
        .saturating_add(scale(POPUP_METADATA_BASELINE_OFFSET_LOGICAL));
    content.left = rank.right.saturating_add(scale(POPUP_RANK_GAP_LOGICAL));
    (rank, content)
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_candidate_label(
    hdc: HDC,
    content: RECT,
    dpi: u32,
    index: usize,
    candidate: &str,
    action_detail: Option<&str>,
    selected: bool,
    candidate_font: HFONT,
    selected_font: HFONT,
    metadata_font: HFONT,
) {
    let (mut rank, mut content) = candidate_label_rects(content, dpi);
    let mut rank_label = (index + 1).to_string().encode_utf16().collect::<Vec<_>>();
    if !metadata_font.is_invalid() {
        // SAFETY: this font remains owned by the current paint operation.
        unsafe {
            let _ = SelectObject(hdc, HGDIOBJ(metadata_font.0));
        }
    }
    // SAFETY: the paint DC and bounded label rectangle are valid.
    unsafe {
        let _ = SetTextColor(
            hdc,
            popup_color(if selected {
                POPUP_SELECTED_RANK_RGB
            } else {
                POPUP_RANK_RGB
            }),
        );
        let _ = DrawTextW(
            hdc,
            &mut rank_label,
            &mut rank,
            DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
    }

    let font = if selected {
        selected_font
    } else {
        candidate_font
    };
    if !font.is_invalid() {
        // SAFETY: this font remains owned by the current paint operation.
        unsafe {
            let _ = SelectObject(hdc, HGDIOBJ(font.0));
        }
    }
    let mut detail_rect = None;
    if let Some(detail) = action_detail {
        let encoded = detail.encode_utf16().collect::<Vec<_>>();
        let mut detail_size = SIZE::default();
        let detail_measured = !metadata_font.is_invalid()
            && unsafe {
                let _ = SelectObject(hdc, HGDIOBJ(metadata_font.0));
                GetTextExtentPoint32W(hdc, &encoded, &mut detail_size).as_bool()
            };
        if detail_measured {
            let gap = popup_scale(dpi, POPUP_ACTION_DETAIL_GAP_LOGICAL);
            let label_encoded = candidate.encode_utf16().collect::<Vec<_>>();
            let mut label_size = SIZE::default();
            let label_measured = !font.is_invalid()
                && unsafe {
                    let _ = SelectObject(hdc, HGDIOBJ(font.0));
                    GetTextExtentPoint32W(hdc, &label_encoded, &mut label_size).as_bool()
                };
            if label_measured
                && label_size
                    .cx
                    .saturating_add(gap)
                    .saturating_add(detail_size.cx)
                    <= content.right.saturating_sub(content.left)
            {
                let detail_left = content.right.saturating_sub(detail_size.cx);
                detail_rect = Some(RECT {
                    left: detail_left,
                    top: content.top,
                    right: content.right,
                    bottom: content.bottom,
                });
                content.right = detail_left.saturating_sub(gap);
            }
        }
    }
    if !font.is_invalid() {
        unsafe {
            let _ = SelectObject(hdc, HGDIOBJ(font.0));
        }
    }
    let mut text =
        unsafe { candidate_popup_text(hdc, candidate, content.right.saturating_sub(content.left)) };
    // SAFETY: the paint DC and bounded candidate rectangle are valid.
    unsafe {
        let _ = SetTextColor(
            hdc,
            popup_color(if selected {
                POPUP_SELECTED_TEXT_RGB
            } else {
                POPUP_CANDIDATE_TEXT_RGB
            }),
        );
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut content,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
    }
    if let (Some(mut detail_rect), Some(detail)) = (detail_rect, action_detail) {
        let mut detail = detail.encode_utf16().collect::<Vec<_>>();
        unsafe {
            if !metadata_font.is_invalid() {
                let _ = SelectObject(hdc, HGDIOBJ(metadata_font.0));
            }
            let _ = SetTextColor(hdc, popup_color(POPUP_RANK_RGB));
            let _ = DrawTextW(
                hdc,
                &mut detail,
                &mut detail_rect,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
    }
}

unsafe fn candidate_popup_text(hdc: HDC, candidate: &str, maximum_width: i32) -> Vec<u16> {
    candidate_text_for_width(candidate, maximum_width, |text| {
        let encoded = text.encode_utf16().collect::<Vec<_>>();
        let mut size = SIZE::default();
        // SAFETY: encoded and size remain valid for this read-only font
        // measurement on the current paint DC.
        unsafe { GetTextExtentPoint32W(hdc, &encoded, &mut size).as_bool() }.then_some(size.cx)
    })
    .encode_utf16()
    .collect()
}

fn candidate_text_for_width(
    candidate: &str,
    maximum_width: i32,
    mut measure: impl FnMut(&str) -> Option<i32>,
) -> String {
    let characters = candidate
        .chars()
        .take(CANDIDATE_DISPLAY_MAX_CHARS)
        .collect::<Vec<_>>();
    let full = characters.iter().collect::<String>();
    let Some(full_width) = measure(&full) else {
        return full;
    };
    if characters.is_empty() || full_width <= maximum_width {
        return full;
    }

    let mut lower = 0;
    let mut upper = characters.len();
    while lower < upper {
        let middle = lower + (upper - lower).div_ceil(2);
        let mut trial = characters[..middle].iter().collect::<String>();
        trial.push('…');
        let Some(trial_width) = measure(&trial) else {
            return full;
        };
        if trial_width <= maximum_width {
            lower = middle;
        } else {
            upper = middle - 1;
        }
    }
    let mut clipped = characters[..lower].iter().collect::<String>();
    clipped.push('…');
    clipped
}

unsafe fn fill_rounded_popup_rect(hdc: HDC, rectangle: RECT, radius: i32, color: COLORREF) {
    if rectangle.right <= rectangle.left || rectangle.bottom <= rectangle.top {
        return;
    }
    // SAFETY: the region and brush are local GDI objects bounded to the
    // current paint DC and are both released before returning.
    let region = unsafe {
        CreateRoundRectRgn(
            rectangle.left,
            rectangle.top,
            rectangle.right.saturating_add(1),
            rectangle.bottom.saturating_add(1),
            radius,
            radius,
        )
    };
    let brush = unsafe { CreateSolidBrush(color) };
    if !region.is_invalid() && !brush.is_invalid() {
        unsafe {
            let _ = FillRgn(hdc, region, brush);
        }
    }
    if !brush.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
    if !region.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

unsafe fn paint_rounded_popup_border(hdc: HDC, client: RECT, dpi: u32, color: COLORREF) {
    let Some(geometry) = candidate_popup_border_geometry(client, dpi) else {
        return;
    };
    // Build a one-DPI-scaled-pixel ring from two rounded regions. Its outer
    // curve uses the exact same corner diameter as the window region, unlike
    // FrameRect's square joins at the four clipped corners.
    let outer = unsafe {
        CreateRoundRectRgn(
            geometry.outer.left,
            geometry.outer.top,
            geometry.outer.right.saturating_add(1),
            geometry.outer.bottom.saturating_add(1),
            geometry.outer_corner_diameter,
            geometry.outer_corner_diameter,
        )
    };
    let inner = unsafe {
        CreateRoundRectRgn(
            geometry.inner.left,
            geometry.inner.top,
            geometry.inner.right.saturating_add(1),
            geometry.inner.bottom.saturating_add(1),
            geometry.inner_corner_diameter,
            geometry.inner_corner_diameter,
        )
    };
    let brush = unsafe { CreateSolidBrush(color) };
    if !outer.is_invalid() && !inner.is_invalid() && !brush.is_invalid() {
        // SAFETY: all three objects are local to this paint call. GDI permits
        // the destination region to alias a source region for CombineRgn.
        unsafe {
            if CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF) != RGN_ERROR {
                let _ = FillRgn(hdc, outer, brush);
            }
        }
    }
    for object in [HGDIOBJ(brush.0), HGDIOBJ(inner.0), HGDIOBJ(outer.0)] {
        if !object.is_invalid() {
            unsafe {
                let _ = DeleteObject(object);
            }
        }
    }
}

fn candidate_selection_rects(item: RECT, dpi: u32) -> (RECT, RECT) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let selected = RECT {
        left: item.left.saturating_add(scale(1)),
        top: item.top.saturating_add(scale(4)),
        right: item.right.saturating_sub(scale(5)),
        bottom: item.bottom.saturating_sub(scale(4)),
    };
    let accent = RECT {
        left: selected.left.saturating_add(scale(5)),
        top: selected.top.saturating_add(scale(7)),
        right: selected.left.saturating_add(scale(8)),
        bottom: selected.bottom.saturating_sub(scale(7)),
    };
    (selected, accent)
}

unsafe fn paint_candidate_selection(
    hdc: HDC,
    item: RECT,
    dpi: u32,
    selected_background: COLORREF,
    selection_accent: COLORREF,
) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let (selected, accent) = candidate_selection_rects(item, dpi);
    unsafe {
        fill_rounded_popup_rect(hdc, selected, scale(6), selected_background);
    }
    unsafe {
        fill_rounded_popup_rect(hdc, accent, scale(4), selection_accent);
    }
}

unsafe extern "system" fn candidate_popup_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: this slot is written immediately after window creation and
    // cleared during WM_NCDESTROY.
    let state_pointer =
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CandidatePopupPaintState;
    if message == WM_PAINT && !state_pointer.is_null() {
        // SAFETY: the boxed state outlives this window.
        unsafe { paint_candidate_popup(hwnd, &mut *state_pointer) };
        return LRESULT(0);
    }
    if message == WM_ERASEBKGND && !state_pointer.is_null() {
        return LRESULT(1);
    }
    let original = if state_pointer.is_null() {
        0
    } else {
        // SAFETY: the stable state remains live through message dispatch.
        unsafe { (*state_pointer).original_window_proc }
    };
    if message == WM_NCDESTROY {
        // SAFETY: prevents any later message from observing a stale pointer.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
    }
    if original == 0 {
        // SAFETY: default handling for a window without a retained subclass.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    // SAFETY: SetWindowLongPtrW returned the system STATIC window procedure.
    let original: WNDPROC = unsafe { std::mem::transmute(original) };
    unsafe { CallWindowProcW(original, hwnd, message, wparam, lparam) }
}

unsafe fn paint_candidate_popup(hwnd: HWND, state: &mut CandidatePopupPaintState) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: standard WM_PAINT lifecycle for this popup.
    let paint_dc = unsafe { BeginPaint(hwnd, &mut paint) };
    if paint_dc.is_invalid() {
        return;
    }
    let mut client = RECT::default();
    // SAFETY: client is writable for this window.
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        // SAFETY: balances BeginPaint above.
        unsafe {
            let _ = EndPaint(hwnd, &paint);
        }
        return;
    }

    // Paint a complete frame off-screen and copy it to the popup once. This
    // avoids exposing the background-only interval between FillRect and the
    // candidate labels while fast composition updates are arriving.
    let client_width = client.right.saturating_sub(client.left);
    let client_height = client.bottom.saturating_sub(client.top);
    let mut buffer_dc = HDC::default();
    let mut buffer_bitmap = HBITMAP::default();
    let mut previous_bitmap = HGDIOBJ::default();
    let mut hdc = paint_dc;
    if client_width > 0 && client_height > 0 {
        // SAFETY: paint_dc is valid for this WM_PAINT operation.
        buffer_dc = unsafe { CreateCompatibleDC(Some(paint_dc)) };
        if !buffer_dc.is_invalid() {
            // SAFETY: dimensions come from this popup's bounded client area.
            buffer_bitmap =
                unsafe { CreateCompatibleBitmap(paint_dc, client_width, client_height) };
            if !buffer_bitmap.is_invalid() {
                // SAFETY: the bitmap and compatible DC remain live until the
                // frame has been copied and the original object restored.
                previous_bitmap = unsafe { SelectObject(buffer_dc, HGDIOBJ(buffer_bitmap.0)) };
                if !previous_bitmap.is_invalid() {
                    hdc = buffer_dc;
                }
            }
        }
    }

    let scale = |logical: i32| popup_scale(state.dpi, logical);
    let background = popup_color(POPUP_BACKGROUND_RGB);
    let selected_background = popup_color(POPUP_SELECTED_BACKGROUND_RGB);
    let selection_accent = popup_color(POPUP_SELECTION_ACCENT_RGB);
    let mode_accent = popup_color(POPUP_MODE_ACCENT_RGB);
    let page_color = popup_color(POPUP_PAGE_RGB);
    let border = popup_color(POPUP_BORDER_RGB);
    let footer_divider = popup_color(POPUP_FOOTER_DIVIDER_RGB);

    // SAFETY: each successful GDI allocation is released before EndPaint.
    let background_brush = unsafe { CreateSolidBrush(background) };
    if !background_brush.is_invalid() {
        // SAFETY: paint DC and client rectangle are valid.
        unsafe {
            let _ = FillRect(hdc, &client, background_brush);
        }
    }

    // The selected candidate carries typographic emphasis while ranks and
    // pagination use a smaller metadata face.
    let candidate_font = unsafe { create_candidate_popup_font(state.dpi, 17, FW_NORMAL.0) };
    let selected_font = unsafe { create_candidate_popup_font(state.dpi, 17, FW_SEMIBOLD.0) };
    let metadata_font = unsafe { create_candidate_popup_font(state.dpi, 14, FW_NORMAL.0) };
    let initial_font = [candidate_font, selected_font, metadata_font]
        .into_iter()
        .find(|font| !font.is_invalid())
        .unwrap_or_default();
    let previous_font = if initial_font.is_invalid() {
        HGDIOBJ::default()
    } else {
        // SAFETY: font and paint DC are valid.
        unsafe { SelectObject(hdc, HGDIOBJ(initial_font.0)) }
    };
    // SAFETY: transparent text is drawn over the filled background.
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
    }

    let padding = scale(POPUP_OUTER_PADDING_LOGICAL);
    let row_height = scale(POPUP_ROW_HEIGHT_LOGICAL);
    let text_padding = scale(POPUP_TEXT_PADDING_LOGICAL);
    match state.layout {
        CandidatePopupLayout::Horizontal => {
            let mut left = padding;
            let widths = horizontal_candidate_widths(&state.display, state.dpi, client.right);
            for ((index, candidate), width) in
                state.display.visible().iter().enumerate().zip(widths)
            {
                let mut item = RECT {
                    left,
                    top: padding,
                    right: left.saturating_add(width),
                    bottom: padding.saturating_add(row_height),
                };
                if index == 0 {
                    // SAFETY: the selected decoration is bounded to this
                    // candidate item and uses only local GDI objects.
                    unsafe {
                        paint_candidate_selection(
                            hdc,
                            item,
                            state.dpi,
                            selected_background,
                            selection_accent,
                        );
                    }
                }
                item.left = item.left.saturating_add(if index == 0 {
                    scale(POPUP_SELECTED_TEXT_INSET_LOGICAL)
                } else {
                    text_padding
                });
                item.right = item.right.saturating_sub(text_padding);
                // SAFETY: fonts and paint rectangles remain owned by this
                // WM_PAINT operation.
                unsafe {
                    paint_candidate_label(
                        hdc,
                        item,
                        state.dpi,
                        index,
                        candidate,
                        (index == 0)
                            .then(|| state.display.action_detail())
                            .flatten(),
                        index == 0,
                        candidate_font,
                        selected_font,
                        metadata_font,
                    );
                }
                left = left.saturating_add(width);
            }
        }
        CandidatePopupLayout::Vertical => {
            for (index, candidate) in state.display.visible().iter().enumerate() {
                let top = padding.saturating_add(
                    row_height.saturating_mul(i32::try_from(index).unwrap_or(i32::MAX)),
                );
                let mut row = RECT {
                    left: padding,
                    top,
                    right: client.right.saturating_sub(padding),
                    bottom: top.saturating_add(row_height),
                };
                if index == 0 {
                    // SAFETY: the selected decoration is bounded to this row
                    // and uses only local GDI objects.
                    unsafe {
                        paint_candidate_selection(
                            hdc,
                            row,
                            state.dpi,
                            selected_background,
                            selection_accent,
                        );
                    }
                }
                row.left = row.left.saturating_add(if index == 0 {
                    scale(POPUP_SELECTED_TEXT_INSET_LOGICAL)
                } else {
                    text_padding
                });
                row.right = row.right.saturating_sub(text_padding);
                // SAFETY: fonts and paint rectangles remain owned by this
                // WM_PAINT operation.
                unsafe {
                    paint_candidate_label(
                        hdc,
                        row,
                        state.dpi,
                        index,
                        candidate,
                        (index == 0)
                            .then(|| state.display.action_detail())
                            .flatten(),
                        index == 0,
                        candidate_font,
                        selected_font,
                        metadata_font,
                    );
                }
            }
        }
    }

    let pages = state.display.page_starts().len();
    let recovery = state.display.view() == InteractiveCandidateView::TranspositionRecovery;
    if pages > 1 || recovery {
        let footer_width = scale(candidate_popup_footer_logical_width(&state.display));
        let mut footer = match state.layout {
            CandidatePopupLayout::Horizontal => RECT {
                left: client.right.saturating_sub(footer_width),
                top: padding,
                right: client.right.saturating_sub(text_padding),
                bottom: padding.saturating_add(row_height),
            },
            CandidatePopupLayout::Vertical => RECT {
                left: padding,
                top: padding.saturating_add(row_height.saturating_mul(
                    i32::try_from(state.display.visible().len()).unwrap_or(i32::MAX),
                )),
                right: client.right.saturating_sub(text_padding),
                bottom: client.bottom.saturating_sub(scale(2)),
            },
        };
        let divider = match state.layout {
            CandidatePopupLayout::Horizontal => RECT {
                left: footer.left,
                top: footer.top.saturating_add(scale(7)),
                right: footer.left.saturating_add(scale(1)),
                bottom: footer.bottom.saturating_sub(scale(7)),
            },
            CandidatePopupLayout::Vertical => RECT {
                left: footer.left,
                top: footer.top,
                right: footer.right,
                bottom: footer.top.saturating_add(scale(1)),
            },
        };
        // SAFETY: the local divider brush and bounded footer rectangle are
        // valid for this paint operation.
        let divider_brush = unsafe { CreateSolidBrush(footer_divider) };
        if !divider_brush.is_invalid() {
            unsafe {
                let _ = FillRect(hdc, &divider, divider_brush);
                let _ = DeleteObject(HGDIOBJ(divider_brush.0));
            }
        }
        match state.layout {
            CandidatePopupLayout::Horizontal => {
                footer.left = footer
                    .left
                    .saturating_add(scale(POPUP_FOOTER_CONTENT_INSET_LOGICAL));
            }
            CandidatePopupLayout::Vertical => {
                footer.top = footer.top.saturating_add(scale(2));
            }
        }
        if recovery {
            let mut mode = footer;
            mode.right = mode.left.saturating_add(scale(44)).min(mode.right);
            let mut label = "换序".encode_utf16().collect::<Vec<_>>();
            // SAFETY: the mode label is bounded to the footer rectangle.
            unsafe {
                if !metadata_font.is_invalid() {
                    let _ = SelectObject(hdc, HGDIOBJ(metadata_font.0));
                }
                let _ = SetTextColor(hdc, mode_accent);
                let _ = DrawTextW(
                    hdc,
                    &mut label,
                    &mut mode,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            }
            footer.left = mode.right.saturating_add(scale(4));
        }
        if pages > 1 {
            let separator = match state.layout {
                CandidatePopupLayout::Horizontal => "/",
                CandidatePopupLayout::Vertical => " / ",
            };
            let mut page = if state.display.may_have_more() {
                format!(
                    "{}{separator}{}  ›",
                    state.display.current_page() + 1,
                    pages
                )
            } else {
                format!("{}{separator}{}", state.display.current_page() + 1, pages)
            }
            .encode_utf16()
            .collect::<Vec<_>>();
            // SAFETY: the page label is bounded to the remaining footer.
            unsafe {
                if !metadata_font.is_invalid() {
                    let _ = SelectObject(hdc, HGDIOBJ(metadata_font.0));
                }
                let _ = SetTextColor(hdc, page_color);
                let _ = DrawTextW(
                    hdc,
                    &mut page,
                    &mut footer,
                    DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
            }
        }
    }

    if state.corner_strategy.uses_custom_region() {
        // SAFETY: the compatibility border is bounded to this popup's paint
        // DC and shares its corner geometry with the fallback window region.
        unsafe {
            paint_rounded_popup_border(hdc, client, state.dpi, border);
        }
    }
    if !previous_font.is_invalid() {
        // SAFETY: restores the original DC object before deleting our font.
        unsafe {
            let _ = SelectObject(hdc, previous_font);
        }
    }
    for font in [candidate_font, selected_font, metadata_font] {
        if !font.is_invalid() {
            // SAFETY: each font is no longer selected into the DC.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
    }
    if !background_brush.is_invalid() {
        // SAFETY: the background brush is no longer used.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(background_brush.0));
        }
    }
    if hdc == buffer_dc && !buffer_dc.is_invalid() {
        // SAFETY: both DCs and the selected bitmap remain valid. One BitBlt
        // publishes the complete frame without an intermediate blank state.
        unsafe {
            let _ = BitBlt(
                paint_dc,
                0,
                0,
                client_width,
                client_height,
                Some(buffer_dc),
                0,
                0,
                SRCCOPY,
            );
            let _ = SelectObject(buffer_dc, previous_bitmap);
        }
    }
    if !buffer_bitmap.is_invalid() {
        // SAFETY: the bitmap is no longer selected into the compatible DC.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(buffer_bitmap.0));
        }
    }
    if !buffer_dc.is_invalid() {
        // SAFETY: the compatible DC is local to this paint operation.
        unsafe {
            let _ = DeleteDC(buffer_dc);
        }
    }
    // SAFETY: balances BeginPaint.
    unsafe {
        let _ = EndPaint(hwnd, &paint);
    }
    state.complete_pending_timing();
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
    #[cfg(test)]
    headless: bool,
    manager: Option<ITfUIElementMgr>,
    state: Rc<RefCell<CandidateElementState>>,
    popup: Rc<RefCell<CandidatePopup>>,
    element: Option<ITfCandidateListUIElement>,
    element_id: Option<u32>,
    show_native: bool,
}

fn candidate_popup_should_show(
    show_native: bool,
    element_visible: bool,
    _layout_clipped: bool,
) -> bool {
    // `ITfContextView::GetTextExt` can report a clipped range while the host
    // is still laying out a newly updated composition. Microsoft SampleIME
    // treats that flag as advisory and still shows its candidate window using
    // the returned rectangle. Hiding here can otherwise suppress every frame
    // of a fast synchronous composition update.
    show_native && element_visible
}

impl CandidateUiController {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            #[cfg(test)]
            headless: false,
            manager: None,
            state: Rc::new(RefCell::new(CandidateElementState::default())),
            popup: Rc::new(RefCell::new(CandidatePopup::default())),
            element: None,
            element_id: None,
            show_native: false,
        }
    }

    fn attach_feedback(
        &mut self,
        native_feedback: SyncWeak<Mutex<NativeFeedbackSession>>,
        native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    ) {
        if let Ok(mut popup) = self.popup.try_borrow_mut() {
            popup.attach_feedback(native_feedback, native_feedback_language_bar_state);
        }
    }

    #[cfg(test)]
    fn new_headless() -> Self {
        let mut controller = Self::new(true);
        controller.headless = true;
        controller
    }

    fn activate(&mut self, thread_manager: &ITfThreadMgr) {
        if self.enabled {
            self.manager = thread_manager.cast().ok();
        }
    }

    fn show(
        &mut self,
        context: &ITfContext,
        range: &ITfRange,
        ec: u32,
        display: CandidateDisplay,
        feedback_context: NativeFeedbackContext,
    ) -> bool {
        if !self.enabled || display.candidates.is_empty() {
            self.end();
            return false;
        }
        #[cfg(test)]
        if self.headless {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.display = Some(display);
                state.shown = true;
                return true;
            }
            self.end();
            return false;
        }
        let document_manager = match unsafe { context.GetDocumentMgr() } {
            Ok(manager) => manager,
            Err(_) => {
                self.end();
                return false;
            }
        };
        let view = match unsafe { context.GetActiveView() } {
            Ok(view) => view,
            Err(_) => {
                self.end();
                return false;
            }
        };
        let mut anchor = RECT::default();
        let mut clipped = false.into();
        // SAFETY: ec grants read access to the active composition range.
        if unsafe { view.GetTextExt(ec, range, &mut anchor, &mut clipped) }.is_err() {
            self.end();
            return false;
        }
        let owner = unsafe { view.GetWnd() }.unwrap_or_default();
        {
            let Ok(mut state) = self.state.try_borrow_mut() else {
                self.end();
                return false;
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
                return false;
            }
        }

        let element_visible = self
            .state
            .try_borrow()
            .map(|state| state.shown)
            .unwrap_or(false);
        let show_popup =
            candidate_popup_should_show(self.show_native, element_visible, clipped.as_bool());
        let popup_ready = match self.popup.try_borrow_mut() {
            Ok(mut popup) if show_popup => popup
                .show(owner, anchor, &display, feedback_context)
                .is_ok(),
            Ok(mut popup) => {
                popup.hide();
                true
            }
            Err(_) => false,
        };
        if !popup_ready {
            self.end();
            return false;
        }
        true
    }

    fn update_contents(
        &mut self,
        display: CandidateDisplay,
        feedback_context: NativeFeedbackContext,
    ) -> bool {
        if !self.enabled || display.candidates.is_empty() {
            self.end();
            return false;
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.display = Some(display.clone());
        } else {
            self.end();
            return false;
        }
        #[cfg(test)]
        if self.headless {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.shown = true;
            }
            return true;
        }
        if let (Some(manager), Some(element_id)) = (&self.manager, self.element_id) {
            // SAFETY: the id belongs to the element begun by this controller.
            if unsafe { manager.UpdateUIElement(element_id) }.is_err() {
                self.end();
                return false;
            }
        }
        let element_visible = self
            .state
            .try_borrow()
            .map(|state| state.shown)
            .unwrap_or(false);
        let popup_ready = match self.popup.try_borrow_mut() {
            Ok(mut popup) if self.show_native && element_visible => {
                popup.update(&display, feedback_context).is_ok()
            }
            Ok(mut popup) => {
                popup.hide();
                true
            }
            Err(_) => false,
        };
        if !popup_ready {
            self.end();
            return false;
        }
        true
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
    native_feedback: SyncWeak<Mutex<NativeFeedbackSession>>,
    native_feedback_context: SyncWeak<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfCompositionSink {
    fn counted(
        document_composition: Weak<RefCell<DocumentCompositionState>>,
        logical_composition: Weak<RefCell<CompositionSession>>,
        candidate_ui: Weak<RefCell<CandidateUiController>>,
        native_feedback: SyncWeak<Mutex<NativeFeedbackSession>>,
        native_feedback_context: SyncWeak<Mutex<NativeFeedbackContextCache>>,
        native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        Self {
            document_composition,
            logical_composition,
            candidate_ui,
            native_feedback,
            native_feedback_context,
            native_feedback_language_bar_state,
            key_advice_mode,
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
        let feedback_context = self
            .native_feedback
            .upgrade()
            .and_then(|feedback| feedback.lock().ok().map(|feedback| feedback.is_accepting()))
            .filter(|accepting| *accepting)
            .map(|_| {
                classify_feedback_context(
                    &active.context,
                    &active.range,
                    ecwrite,
                    self.key_advice_mode,
                )
            })
            .unwrap_or(NativeFeedbackContext::Unknown);
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
        let cancellation_code =
            if let Some(logical_composition) = self.logical_composition.upgrade() {
                let mut logical_composition = logical_composition
                    .try_borrow_mut()
                    .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
                let code = logical_composition.phonetic().to_owned();
                logical_composition.finish_commit();
                Some(code)
            } else {
                None
            };
        text_result?;
        selection_result?;
        if let Some(context) = self.native_feedback_context.upgrade()
            && let Ok(mut context) = context.lock()
        {
            context.clear();
        }
        if let Some(code) = cancellation_code.filter(|code| !code.is_empty())
            && let Some(native_feedback) = self.native_feedback.upgrade()
            && let Ok(mut native_feedback) = native_feedback.lock()
            && native_feedback.is_accepting()
        {
            let record_result = native_feedback.record_at(
                feedback_context,
                NativeFeedbackEvent::CompositionCancelled {
                    code,
                    source: NativeCancellationSource::HostTermination,
                },
                native_feedback_monotonic_ms(),
            );
            drop(native_feedback);
            if matches!(record_result, NativeFeedbackRecordResult::Stopped(_))
                && let Some(state) = self.native_feedback_language_bar_state.upgrade()
            {
                state.notify();
            }
        }
        Ok(())
    }
}

/// Applies one planned composition change inside a synchronous TSF edit session.
struct EditSessionShared {
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    native_feedback: Arc<Mutex<NativeFeedbackSession>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
}

struct DocumentEditRequest {
    action: PendingDocumentEdit,
    candidate_display: Option<CandidateDisplay>,
    feedback_after_success: Option<NativeFeedbackEvent>,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
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
    feedback_after_success: Option<NativeFeedbackEvent>,
    native_feedback: Arc<Mutex<NativeFeedbackSession>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
}

impl TsfDocumentEditSession {
    fn counted(
        context: ITfContext,
        action: PendingDocumentEdit,
        shared: EditSessionShared,
        candidate_display: Option<CandidateDisplay>,
        feedback_after_success: Option<NativeFeedbackEvent>,
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
            feedback_after_success,
            native_feedback: shared.native_feedback,
            native_feedback_context: shared.native_feedback_context,
            native_feedback_language_bar_state: shared.native_feedback_language_bar_state,
            key_advice_mode: shared.key_advice_mode,
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
            Arc::downgrade(&self.native_feedback),
            Arc::downgrade(&self.native_feedback_context),
            Rc::downgrade(&self.native_feedback_language_bar_state),
            self.key_advice_mode,
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

    fn insert_text_at_selection(&self, ec: u32, text: &str) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let insertion: ITfInsertAtSelection = self.context.cast()?;
        // SAFETY: `ec` is the current read/write cookie. This inserts ordinary
        // committed text without creating a TSF composition.
        let range =
            unsafe { insertion.InsertTextAtSelection(ec, TF_IAS_NO_DEFAULT_COMPOSITION, &utf16) }?;
        move_selection_after_range(&self.context, &range, ec)
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
        let feedback_accepting = self
            .native_feedback
            .lock()
            .map(|feedback| feedback.is_accepting())
            .unwrap_or(false);
        let feedback_context_before = if feedback_accepting
            && !matches!(&self.action, PendingDocumentEdit::UpdatePreedit(_))
        {
            self.active_composition()?
                .map(|active| {
                    classify_feedback_context(
                        &self.context,
                        &active.range,
                        ec,
                        self.key_advice_mode,
                    )
                })
                .unwrap_or(NativeFeedbackContext::Unknown)
        } else {
            NativeFeedbackContext::Unknown
        };
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
            PendingDocumentEdit::Insert(text) => self.insert_text_at_selection(ec, text)?,
        }
        let feedback_context =
            if feedback_accepting && let PendingDocumentEdit::UpdatePreedit(code) = &self.action {
                let context = self
                    .active_composition()?
                    .map(|active| {
                        classify_feedback_context(
                            &self.context,
                            &active.range,
                            ec,
                            self.key_advice_mode,
                        )
                    })
                    .unwrap_or(NativeFeedbackContext::Unknown);
                if let Ok(mut cache) = self.native_feedback_context.lock() {
                    cache.remember(code, context);
                }
                context
            } else {
                feedback_context_before
            };
        let feedback_action_succeeded =
            if matches!(self.action, PendingDocumentEdit::UpdatePreedit(_)) {
                if let (Some(active), Some(display)) =
                    (self.active_composition()?, self.candidate_display.clone())
                    && let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut()
                {
                    candidate_ui.show(&self.context, &active.range, ec, display, feedback_context)
                } else {
                    false
                }
            } else if let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut() {
                candidate_ui.end();
                self.mode != EditSessionMode::CleanupAsync || cleanup_applied
            } else {
                self.mode != EditSessionMode::CleanupAsync || cleanup_applied
            };
        if cleanup_applied {
            self.logical_composition
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                .finish_commit();
        }
        if feedback_action_succeeded {
            if !matches!(&self.action, PendingDocumentEdit::UpdatePreedit(_))
                && let Ok(mut cache) = self.native_feedback_context.lock()
            {
                cache.clear();
            }
            if let Some(event) = self.feedback_after_success.clone()
                && let Ok(mut feedback) = self.native_feedback.lock()
                && feedback.is_accepting()
            {
                let record_result =
                    feedback.record_at(feedback_context, event, native_feedback_monotonic_ms());
                drop(feedback);
                if matches!(record_result, NativeFeedbackRecordResult::Stopped(_)) {
                    self.native_feedback_language_bar_state.notify();
                }
            }
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

#[derive(Default)]
struct NativeFeedbackContextCache {
    code: String,
    context: NativeFeedbackContext,
}

impl NativeFeedbackContextCache {
    fn remember(&mut self, code: &str, context: NativeFeedbackContext) {
        self.code.clear();
        self.code.push_str(code);
        self.context = context;
    }

    fn context_for(&self, code: &str) -> NativeFeedbackContext {
        if self.code == code {
            self.context
        } else {
            NativeFeedbackContext::Unknown
        }
    }

    fn clear(&mut self) {
        self.code.clear();
        self.context = NativeFeedbackContext::Unknown;
    }
}

const FEEDBACK_MENU_START: u32 = 1;
const FEEDBACK_MENU_STOP: u32 = 2;
const FEEDBACK_MENU_CLEAR: u32 = 3;
const FEEDBACK_MENU_WISH: u32 = 4;
const FEEDBACK_MENU_STATUS: u32 = 100;
const FEEDBACK_MENU_TIMING_BUCKETS: u32 = 102;
const FEEDBACK_MENU_TIMING_COUNTS: u32 = 103;
const LANGUAGE_BAR_SINK_COOKIE: u32 = 1;
const LANGUAGE_BAR_ICON_SIZE: i32 = 16;
const LANGUAGE_BAR_ICON_ROW_BYTES: usize = 2;
const LANGUAGE_BAR_ICON_BYTES: usize =
    LANGUAGE_BAR_ICON_SIZE as usize * LANGUAGE_BAR_ICON_ROW_BYTES;

// Hand-tuned monochrome 16 px glyphs keep the modern taskbar item crisp
// without adding a resource compiler or a theme-specific bitmap. The system
// recolors their black pixels through TF_LBI_STYLE_TEXTCOLORICON.
const LANGUAGE_BAR_CHINESE_ICON_ROWS: [u16; LANGUAGE_BAR_ICON_SIZE as usize] = [
    0x0300, 0x0600, 0x3ff8, 0x2008, 0x2008, 0x3ff8, 0x2008, 0x2008, 0x3ff8, 0x2008, 0x2008, 0x2008,
    0x3ff8, 0x0000, 0x0000, 0x0000,
];
const LANGUAGE_BAR_ENGLISH_ICON_ROWS: [u16; LANGUAGE_BAR_ICON_SIZE as usize] = [
    0x0000, 0x0600, 0x0600, 0x1980, 0x1980, 0x6060, 0x6060, 0x6060, 0x6060, 0x7fe0, 0x7fe0, 0x6060,
    0x6060, 0x6060, 0x6060, 0x0000,
];
const LANGUAGE_BAR_RECORDING_DOT: u16 = 0x0006;

fn feedback_language_bar_icon_rows(
    mode: InputMode,
    summary: NativeFeedbackSummary,
) -> [u16; LANGUAGE_BAR_ICON_SIZE as usize] {
    let mut rows = match mode {
        InputMode::Chinese => LANGUAGE_BAR_CHINESE_ICON_ROWS,
        InputMode::English => LANGUAGE_BAR_ENGLISH_ICON_ROWS,
    };
    if summary.lifecycle == NativeFeedbackLifecycle::Recording {
        rows[13] |= LANGUAGE_BAR_RECORDING_DOT;
        rows[14] |= LANGUAGE_BAR_RECORDING_DOT;
    }
    rows
}

fn feedback_language_bar_icon_masks(
    mode: InputMode,
    summary: NativeFeedbackSummary,
) -> ([u8; LANGUAGE_BAR_ICON_BYTES], [u8; LANGUAGE_BAR_ICON_BYTES]) {
    let rows = feedback_language_bar_icon_rows(mode, summary);
    let mut and_mask = [0xff; LANGUAGE_BAR_ICON_BYTES];
    let xor_mask = [0_u8; LANGUAGE_BAR_ICON_BYTES];
    for (target_row, ink) in rows.iter().enumerate() {
        let transparent = !ink;
        let offset = target_row * LANGUAGE_BAR_ICON_ROW_BYTES;
        and_mask[offset] = (transparent >> 8) as u8;
        and_mask[offset + 1] = transparent as u8;
    }
    (and_mask, xor_mask)
}

fn feedback_language_bar_icon(mode: InputMode, summary: NativeFeedbackSummary) -> Result<HICON> {
    let (and_mask, xor_mask) = feedback_language_bar_icon_masks(mode, summary);
    // SAFETY: both one-bit masks contain exactly 16 word-aligned rows and
    // remain live for the complete CreateIcon call. The returned icon is
    // transferred to the TSF language-bar caller, matching SampleIME.
    unsafe {
        CreateIcon(
            None,
            LANGUAGE_BAR_ICON_SIZE,
            LANGUAGE_BAR_ICON_SIZE,
            1,
            1,
            and_mask.as_ptr(),
            xor_mask.as_ptr(),
        )
    }
}

struct NativeFeedbackLanguageBarState {
    feedback: Arc<Mutex<NativeFeedbackSession>>,
    feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    input_mode: Rc<Cell<InputMode>>,
    sink: RefCell<Option<ITfLangBarItemSink>>,
    shown: Cell<bool>,
    wish_root: Option<PathBuf>,
    wish_save_status: Cell<WishSaveStatus>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum WishSaveStatus {
    #[default]
    Never,
    Saved {
        events: usize,
    },
    Failed,
}

impl NativeFeedbackLanguageBarState {
    fn new(
        feedback: Arc<Mutex<NativeFeedbackSession>>,
        feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
        input_mode: Rc<Cell<InputMode>>,
    ) -> Self {
        let wish_root = module_path()
            .ok()
            .and_then(|module| wish_root_for_module(&module));
        Self::with_wish_root(feedback, feedback_context, input_mode, wish_root)
    }

    fn with_wish_root(
        feedback: Arc<Mutex<NativeFeedbackSession>>,
        feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
        input_mode: Rc<Cell<InputMode>>,
        wish_root: Option<PathBuf>,
    ) -> Self {
        Self {
            feedback,
            feedback_context,
            input_mode,
            sink: RefCell::new(None),
            shown: Cell::new(true),
            wish_root,
            wish_save_status: Cell::new(WishSaveStatus::Never),
        }
    }

    fn summary(&self) -> Result<NativeFeedbackSummary> {
        self.feedback
            .lock()
            .map(|feedback| feedback.summary())
            .map_err(|_| lifecycle_error(E_UNEXPECTED))
    }

    fn notify(&self) {
        let sink = self.sink.try_borrow().ok().and_then(|sink| sink.clone());
        if let Some(sink) = sink {
            // SAFETY: the language bar installed this sink for the lifetime of
            // the item. Failure only means the visible status cannot refresh.
            let _ = unsafe {
                sink.OnUpdate(TF_LBI_STATUS | TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP)
            };
        }
    }

    fn disconnect_sink(&self) {
        if let Ok(mut sink) = self.sink.try_borrow_mut() {
            *sink = None;
        }
    }

    fn clear_context_cache(&self) {
        if let Ok(mut context) = self.feedback_context.lock() {
            context.clear();
        }
    }

    fn menu(&self) -> Result<Vec<(u32, u32, String)>> {
        Ok(feedback_language_bar_menu(
            self.summary()?,
            self.wish_save_status.get(),
            self.wish_root.is_some(),
        ))
    }

    fn save_recent_wish(&self) -> bool {
        let Some(root) = self.wish_root.as_deref() else {
            self.wish_save_status.set(WishSaveStatus::Failed);
            self.notify();
            return false;
        };
        let frozen = self.feedback.lock().ok().and_then(|feedback| {
            let marker_ms = native_feedback_monotonic_ms();
            feedback
                .freeze_recent(
                    NativeFeedbackFreezeAuthorization::explicit_private_snapshot(),
                    marker_ms,
                    DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
                    DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
                )
                .ok()
        });
        let saved_events = frozen
            .and_then(|frozen| WishSnapshot::from_frozen(&frozen).ok())
            .and_then(|snapshot| {
                let event_count = snapshot.events().len();
                if event_count == 0 {
                    return None;
                }
                save_wish_snapshot(root, &snapshot, &WindowsUserDataProtector)
                    .ok()
                    .map(|_| event_count)
            });
        self.wish_save_status.set(match saved_events {
            Some(events) => WishSaveStatus::Saved { events },
            None => WishSaveStatus::Failed,
        });
        self.notify();
        saved_events.is_some()
    }

    fn perform_feedback_action(&self, action: u32) -> Result<bool> {
        if action == FEEDBACK_MENU_WISH {
            return Ok(self.save_recent_wish());
        }
        let changed = {
            let mut feedback = self
                .feedback
                .lock()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            match action {
                FEEDBACK_MENU_START => {
                    feedback.start_memory(
                        NativeFeedbackAuthorization::explicit_memory_only(),
                        NativeFeedbackLimits::default(),
                    ) == NativeFeedbackStartResult::Started
                }
                FEEDBACK_MENU_STOP => feedback.stop() == NativeFeedbackStopResult::Stopped,
                FEEDBACK_MENU_CLEAR => {
                    feedback.clear_stopped() == NativeFeedbackClearResult::Cleared
                }
                _ => return Err(lifecycle_error(E_INVALIDARG)),
            }
        };
        if changed {
            if action == FEEDBACK_MENU_CLEAR || action == FEEDBACK_MENU_START {
                self.wish_save_status.set(WishSaveStatus::Never);
            }
            self.clear_context_cache();
            self.notify();
        }
        Ok(changed)
    }

    fn perform_wish_command(&self, command: WishCommand) -> WishCommandAckStatus {
        let action = match command {
            WishCommand::Start => FEEDBACK_MENU_START,
            WishCommand::SaveRecent => {
                let event_count = self.summary().map_or(0, |summary| summary.events);
                if event_count == 0 {
                    return WishCommandAckStatus::NoChange;
                }
                return if self.save_recent_wish() {
                    WishCommandAckStatus::Applied
                } else {
                    WishCommandAckStatus::Failed
                };
            }
            WishCommand::Stop => FEEDBACK_MENU_STOP,
            WishCommand::ClearStopped => FEEDBACK_MENU_CLEAR,
        };
        match self.perform_feedback_action(action) {
            Ok(true) => WishCommandAckStatus::Applied,
            Ok(false) => WishCommandAckStatus::NoChange,
            Err(_) => WishCommandAckStatus::Failed,
        }
    }
}

fn feedback_language_bar_text(mode: InputMode, summary: NativeFeedbackSummary) -> String {
    let mode = match mode {
        InputMode::Chinese => "中",
        InputMode::English => "英",
    };
    if summary.lifecycle == NativeFeedbackLifecycle::Recording {
        format!("{mode} ●")
    } else {
        mode.to_owned()
    }
}

fn feedback_language_bar_tooltip(mode: InputMode, summary: NativeFeedbackSummary) -> String {
    let mode = match mode {
        InputMode::Chinese => "中文",
        InputMode::English => "英文",
    };
    match summary.lifecycle {
        NativeFeedbackLifecycle::Disabled => format!("自然码 Alpha · {mode} · 反馈未开始"),
        NativeFeedbackLifecycle::Recording => format!(
            "自然码 Alpha · {mode} · 反馈记录中（暂不保存，{} 条）",
            summary.events
        ),
        NativeFeedbackLifecycle::Stopped if summary.complete => {
            format!(
                "自然码 Alpha · {mode} · 反馈已停止（{} 条）",
                summary.events
            )
        }
        NativeFeedbackLifecycle::Stopped => format!(
            "自然码 Alpha · {mode} · 反馈已停止且不完整（{} 条）",
            summary.events
        ),
    }
}

fn feedback_half_pair_gap_bucket_labels() -> String {
    let bounds = NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS;
    let mut labels = Vec::with_capacity(bounds.len() + 1);
    labels.push(format!("<{}", bounds[0]));
    labels.extend(
        bounds
            .windows(2)
            .map(|pair| format!("{}–{}", pair[0], pair[1] - 1)),
    );
    labels.push(format!("≥{}", bounds[bounds.len() - 1]));
    labels.join(" / ")
}

fn feedback_language_bar_menu(
    summary: NativeFeedbackSummary,
    wish_status: WishSaveStatus,
    wish_storage_available: bool,
) -> Vec<(u32, u32, String)> {
    let mut items = match summary.lifecycle {
        NativeFeedbackLifecycle::Disabled => {
            vec![(FEEDBACK_MENU_START, 0, "开始反馈（暂不保存）".to_owned())]
        }
        NativeFeedbackLifecycle::Recording => vec![
            (
                FEEDBACK_MENU_WISH,
                if wish_storage_available && summary.events != 0 {
                    0
                } else {
                    TF_LBMENUF_GRAYED
                },
                if wish_storage_available {
                    "向猫猫许愿（保存近 30 秒）".to_owned()
                } else {
                    "向猫猫许愿（仅安装版可用）".to_owned()
                },
            ),
            (FEEDBACK_MENU_STOP, 0, "停止反馈".to_owned()),
            (
                FEEDBACK_MENU_STATUS,
                TF_LBMENUF_CHECKED | TF_LBMENUF_GRAYED,
                format!("记录中（{} 条）", summary.events),
            ),
        ],
        NativeFeedbackLifecycle::Stopped => vec![
            (FEEDBACK_MENU_CLEAR, 0, "清除本轮".to_owned()),
            (
                FEEDBACK_MENU_STATUS,
                TF_LBMENUF_GRAYED,
                if summary.complete {
                    format!("已停止（{} 条）", summary.events)
                } else {
                    format!("已停止且不完整（{} 条）", summary.events)
                },
            ),
        ],
    };
    if summary.enabled {
        match wish_status {
            WishSaveStatus::Never => {}
            WishSaveStatus::Saved { events } => items.push((
                FEEDBACK_MENU_STATUS + 4,
                TF_LBMENUF_CHECKED | TF_LBMENUF_GRAYED,
                format!("许愿已加密保存（{events} 条）"),
            )),
            WishSaveStatus::Failed => items.push((
                FEEDBACK_MENU_STATUS + 4,
                TF_LBMENUF_GRAYED,
                "上次许愿保存失败；反馈仍在记录".to_owned(),
            )),
        }
        items.push((
            FEEDBACK_MENU_TIMING_BUCKETS,
            TF_LBMENUF_GRAYED,
            format!("双拼间隔（ms）：{}", feedback_half_pair_gap_bucket_labels()),
        ));
        items.push((
            FEEDBACK_MENU_TIMING_COUNTS,
            TF_LBMENUF_GRAYED,
            format!(
                "计数：{}（共 {} 个）",
                summary
                    .half_pair_gap_histogram
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(" / "),
                summary.half_pair_gap_samples
            ),
        ));
    }
    items.push((
        FEEDBACK_MENU_STATUS + 1,
        TF_LBMENUF_GRAYED,
        "反馈默认暂不保存；许愿才写当前用户加密文件；不联网".to_owned(),
    ));
    items
}

fn feedback_native_menu_flags(flags: u32) -> MENU_ITEM_FLAGS {
    let mut native = if flags & TF_LBMENUF_SEPARATOR != 0 {
        MF_SEPARATOR
    } else {
        MF_STRING
    };
    if flags & TF_LBMENUF_GRAYED != 0 {
        native |= MF_GRAYED;
    }
    if flags & TF_LBMENUF_CHECKED != 0 {
        native |= MF_CHECKED;
    }
    native
}

struct NativeFeedbackPopupMenu(HMENU);

impl NativeFeedbackPopupMenu {
    fn create() -> Result<Self> {
        // SAFETY: the returned owned menu is destroyed by Drop.
        unsafe { CreatePopupMenu() }.map(Self)
    }

    fn append(&self, id: u32, flags: u32, label: &str) -> Result<()> {
        let native_flags = feedback_native_menu_flags(flags);
        let label = label
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: the owned menu remains valid and the null-terminated label
        // stays live for the complete AppendMenuW call.
        unsafe {
            AppendMenuW(
                self.0,
                native_flags,
                usize::try_from(id).map_err(|_| lifecycle_error(E_INVALIDARG))?,
                PCWSTR(label.as_ptr()),
            )
        }
    }

    fn track(&self, point: &POINT) -> Option<u32> {
        // SAFETY: this reads only the current foreground window selected by
        // Windows for the user's language-bar click.
        let owner = unsafe { GetForegroundWindow() };
        if owner.is_invalid() {
            return None;
        }
        let flags = (TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON).0;
        // SAFETY: the owned menu and foreground owner remain live while the
        // modal native popup tracks this explicit click.
        let command = unsafe { TrackPopupMenuEx(self.0, flags, point.x, point.y, owner, None) }.0;
        u32::try_from(command).ok().filter(|command| *command != 0)
    }
}

impl Drop for NativeFeedbackPopupMenu {
    fn drop(&mut self) {
        // SAFETY: this is the unique owned handle returned by CreatePopupMenu.
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

fn show_native_feedback_popup(state: &NativeFeedbackLanguageBarState, point: &POINT) -> Result<()> {
    let menu = NativeFeedbackPopupMenu::create()?;
    for (id, flags, label) in state.menu()? {
        menu.append(id, flags, &label)?;
    }
    if let Some(command) = menu.track(point) {
        state.perform_feedback_action(command)?;
    }
    Ok(())
}

#[implement(ITfLangBarItemButton, ITfSource)]
struct NativeFeedbackLanguageBarItem {
    state: Rc<NativeFeedbackLanguageBarState>,
    guid_item: GUID,
}

impl NativeFeedbackLanguageBarItem {
    #[cfg(test)]
    fn counted(state: Rc<NativeFeedbackLanguageBarState>) -> Self {
        Self::counted_with_guid(state, GUID_LBI_INPUTMODE)
    }

    fn counted_with_guid(state: Rc<NativeFeedbackLanguageBarState>, guid_item: GUID) -> Self {
        object_created();
        Self { state, guid_item }
    }
}

impl Drop for NativeFeedbackLanguageBarItem {
    fn drop(&mut self) {
        self.state.disconnect_sink();
        object_dropped();
    }
}

impl ITfLangBarItem_Impl for NativeFeedbackLanguageBarItem_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        let mut value = TF_LANGBARITEMINFO {
            clsidService: TSF_ALPHA_CLSID,
            guidItem: self.guid_item,
            dwStyle: TF_LBI_STYLE_BTN_BUTTON
                | TF_LBI_STYLE_BTN_MENU
                | TF_LBI_STYLE_SHOWNINTRAY
                | TF_LBI_STYLE_TEXTCOLORICON,
            ulSort: 0,
            ..TF_LANGBARITEMINFO::default()
        };
        let description_capacity = value.szDescription.len().saturating_sub(1);
        for (target, source) in value
            .szDescription
            .iter_mut()
            .take(description_capacity)
            .zip("自然码 Alpha 输入模式与反馈".encode_utf16())
        {
            *target = source;
        }
        // SAFETY: the caller supplied the required writable output.
        unsafe { info.write(value) };
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        let mut status = 0;
        if !self.state.shown.get() {
            status |= TF_LBI_STATUS_HIDDEN;
        }
        if self.state.feedback.lock().is_err() {
            status |= TF_LBI_STATUS_DISABLED;
        }
        Ok(status)
    }

    fn Show(&self, show: windows::core::BOOL) -> Result<()> {
        let show = show.as_bool();
        if self.state.shown.replace(show) != show {
            self.state.notify();
        }
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::from(
            feedback_language_bar_tooltip(self.state.input_mode.get(), self.state.summary()?)
                .as_str(),
        ))
    }
}

impl ITfLangBarItemButton_Impl for NativeFeedbackLanguageBarItem_Impl {
    fn OnClick(&self, click: TfLBIClick, point: &POINT, _area: *const RECT) -> Result<()> {
        if click != TF_LBI_CLK_LEFT && click != TF_LBI_CLK_RIGHT {
            return Ok(());
        }
        show_native_feedback_popup(&self.state, point)
    }

    fn InitMenu(&self, menu: Ref<ITfMenu>) -> Result<()> {
        let menu = menu.cloned().ok_or_else(|| lifecycle_error(E_POINTER))?;
        for (id, flags, label) in self.state.menu()? {
            let label = label.encode_utf16().collect::<Vec<_>>();
            // SAFETY: label is a bounded live UTF-16 slice. None of these
            // entries is a submenu, so the submenu output is intentionally
            // null.
            unsafe {
                menu.AddMenuItem(
                    id,
                    flags,
                    HBITMAP::default(),
                    HBITMAP::default(),
                    &label,
                    ptr::null_mut(),
                )
            }?;
        }
        Ok(())
    }

    fn OnMenuSelect(&self, id: u32) -> Result<()> {
        self.state.perform_feedback_action(id).map(|_| ())
    }

    fn GetIcon(&self) -> Result<HICON> {
        feedback_language_bar_icon(self.state.input_mode.get(), self.state.summary()?)
    }

    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(
            feedback_language_bar_text(self.state.input_mode.get(), self.state.summary()?).as_str(),
        ))
    }
}

impl ITfSource_Impl for NativeFeedbackLanguageBarItem_Impl {
    fn AdviseSink(&self, riid: *const GUID, unknown: Ref<IUnknown>) -> Result<u32> {
        if riid.is_null() || unknown.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        // SAFETY: the caller supplied a non-null interface identifier.
        if unsafe { *riid } != ITfLangBarItemSink::IID {
            return Err(lifecycle_error(CONNECT_E_CANNOTCONNECT));
        }
        let sink: ITfLangBarItemSink = unknown
            .cloned()
            .ok_or_else(|| lifecycle_error(E_POINTER))?
            .cast()
            .map_err(|_| lifecycle_error(CONNECT_E_CANNOTCONNECT))?;
        let mut current = self
            .state
            .sink
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if current.is_some() {
            return Err(lifecycle_error(CONNECT_E_ADVISELIMIT));
        }
        *current = Some(sink);
        Ok(LANGUAGE_BAR_SINK_COOKIE)
    }

    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        let mut sink = self
            .state
            .sink
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if cookie != LANGUAGE_BAR_SINK_COOKIE || sink.is_none() {
            return Err(lifecycle_error(CONNECT_E_NOCONNECTION));
        }
        *sink = None;
        Ok(())
    }
}

struct NativeFeedbackLanguageBarController {
    enabled: bool,
    state: Rc<NativeFeedbackLanguageBarState>,
    guid_item: GUID,
    manager: Option<ITfLangBarItemMgr>,
    item: Option<ITfLangBarItem>,
}

impl NativeFeedbackLanguageBarController {
    fn new(enabled: bool, state: Rc<NativeFeedbackLanguageBarState>) -> Self {
        Self {
            enabled,
            state,
            guid_item: GUID_LBI_INPUTMODE,
            manager: None,
            item: None,
        }
    }

    #[cfg(test)]
    fn new_with_guid(
        enabled: bool,
        state: Rc<NativeFeedbackLanguageBarState>,
        guid_item: GUID,
    ) -> Self {
        let mut controller = Self::new(enabled, state);
        controller.guid_item = guid_item;
        controller
    }

    fn activate(&mut self, thread_manager: &ITfThreadMgr) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.manager.is_some() || self.item.is_some() {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        let manager: ITfLangBarItemMgr = thread_manager.cast()?;
        let button: ITfLangBarItemButton = NativeFeedbackLanguageBarItem::counted_with_guid(
            Rc::clone(&self.state),
            self.guid_item,
        )
        .into();
        let item: ITfLangBarItem = button.cast()?;
        self.state.shown.set(true);
        self.state.disconnect_sink();
        // SAFETY: the manager retains the item until the matching RemoveItem.
        if let Err(error) = unsafe { manager.AddItem(&item) } {
            // SAFETY: best effort for an implementation that retained the
            // item before reporting a late failure.
            let _ = unsafe { manager.RemoveItem(&item) };
            self.state.disconnect_sink();
            return Err(error);
        }
        self.manager = Some(manager);
        self.item = Some(item);
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        let manager = self.manager.take();
        let item = self.item.take();
        let result = match (manager, item) {
            (Some(manager), Some(item)) => {
                // SAFETY: balances the successful AddItem owned above.
                unsafe { manager.RemoveItem(&item) }
            }
            (None, None) => Ok(()),
            _ => Err(lifecycle_error(E_UNEXPECTED)),
        };
        self.state.disconnect_sink();
        result
    }
}

impl Drop for NativeFeedbackLanguageBarController {
    fn drop(&mut self) {
        let _ = self.deactivate();
    }
}

struct NativeWishCommandShared {
    state: Weak<NativeFeedbackLanguageBarState>,
    command_guid: GUID,
    command_compartment: RefCell<Option<ITfCompartment>>,
    acknowledgement_compartment: RefCell<Option<ITfCompartment>>,
    client_id: Cell<u32>,
    last_sequence: Cell<Option<u32>>,
}

impl NativeWishCommandShared {
    fn new(state: Weak<NativeFeedbackLanguageBarState>, command_guid: GUID) -> Self {
        Self {
            state,
            command_guid,
            command_compartment: RefCell::new(None),
            acknowledgement_compartment: RefCell::new(None),
            client_id: Cell::new(0),
            last_sequence: Cell::new(None),
        }
    }

    fn configure(
        &self,
        command_compartment: ITfCompartment,
        acknowledgement_compartment: ITfCompartment,
        client_id: u32,
    ) {
        let baseline = read_wish_command(&command_compartment).map(|word| word.sequence());
        self.command_compartment.replace(Some(command_compartment));
        self.acknowledgement_compartment
            .replace(Some(acknowledgement_compartment));
        self.client_id.set(client_id);
        self.last_sequence.set(baseline);
    }

    fn clear(&self) {
        self.command_compartment.replace(None);
        self.acknowledgement_compartment.replace(None);
        self.client_id.set(0);
        self.last_sequence.set(None);
    }

    fn accepts_guid(&self, guid: GUID) -> bool {
        guid == self.command_guid
    }

    fn handle_change(&self) -> Result<()> {
        let command_compartment = self
            .command_compartment
            .borrow()
            .clone()
            .ok_or_else(|| lifecycle_error(E_UNEXPECTED))?;
        let Some(word) = read_wish_command(&command_compartment) else {
            return Ok(());
        };
        if self.last_sequence.get() == Some(word.sequence()) {
            return Ok(());
        }
        self.last_sequence.set(Some(word.sequence()));
        let status = self
            .state
            .upgrade()
            .map_or(WishCommandAckStatus::Failed, |state| {
                state.perform_wish_command(word.command())
            });
        self.publish_acknowledgement(
            WishCommandAck::new(word.sequence(), status)
                .ok_or_else(|| lifecycle_error(E_UNEXPECTED))?,
        )
    }

    fn publish_acknowledgement(&self, acknowledgement: WishCommandAck) -> Result<()> {
        let compartment = self
            .acknowledgement_compartment
            .borrow()
            .clone()
            .ok_or_else(|| lifecycle_error(E_UNEXPECTED))?;
        let current = read_wish_acknowledgement(&compartment);
        if current.is_some_and(|current| {
            current.sequence() == acknowledgement.sequence()
                && current.status() >= acknowledgement.status()
        }) {
            return Ok(());
        }
        let value = VARIANT::from(
            i32::try_from(acknowledgement.raw())
                .expect("wish acknowledgement words always fit in VT_I4"),
        );
        // SAFETY: this global compartment carries only a bounded integer ACK.
        unsafe { compartment.SetValue(self.client_id.get(), &value) }
    }
}

fn read_wish_command(compartment: &ITfCompartment) -> Option<crate::wish_command::WishCommandWord> {
    // SAFETY: GetValue initializes a VARIANT owned by the returned value.
    unsafe { compartment.GetValue() }
        .ok()
        .and_then(|value| i32::try_from(&value).ok())
        .and_then(|value| u32::try_from(value).ok())
        .and_then(crate::wish_command::WishCommandWord::parse)
}

fn read_wish_acknowledgement(compartment: &ITfCompartment) -> Option<WishCommandAck> {
    // SAFETY: GetValue initializes a VARIANT owned by the returned value.
    unsafe { compartment.GetValue() }
        .ok()
        .and_then(|value| i32::try_from(&value).ok())
        .and_then(|value| u32::try_from(value).ok())
        .and_then(WishCommandAck::parse)
}

#[implement(ITfCompartmentEventSink)]
struct NativeWishCommandSink {
    shared: Weak<NativeWishCommandShared>,
}

impl ITfCompartmentEventSink_Impl for NativeWishCommandSink_Impl {
    fn OnChange(&self, guid: *const GUID) -> Result<()> {
        if guid.is_null() {
            return Err(lifecycle_error(E_POINTER));
        }
        // SAFETY: TSF guarantees that OnChange receives a valid GUID pointer
        // for the duration of this synchronous callback.
        self.shared.upgrade().map_or(Ok(()), |shared| {
            if shared.accepts_guid(unsafe { *guid }) {
                shared.handle_change()
            } else {
                Ok(())
            }
        })
    }
}

struct NativeWishCommandController {
    enabled: bool,
    shared: Rc<NativeWishCommandShared>,
    acknowledgement_guid: GUID,
    source: Option<ITfSource>,
    sink: Option<ITfCompartmentEventSink>,
    cookie: Option<u32>,
}

impl NativeWishCommandController {
    fn new(enabled: bool, state: Weak<NativeFeedbackLanguageBarState>) -> Self {
        Self::new_with_guids(
            enabled,
            state,
            WISH_COMMAND_COMPARTMENT_GUID,
            WISH_ACK_COMPARTMENT_GUID,
        )
    }

    fn new_with_guids(
        enabled: bool,
        state: Weak<NativeFeedbackLanguageBarState>,
        command_guid: GUID,
        acknowledgement_guid: GUID,
    ) -> Self {
        Self {
            enabled,
            shared: Rc::new(NativeWishCommandShared::new(state, command_guid)),
            acknowledgement_guid,
            source: None,
            sink: None,
            cookie: None,
        }
    }

    fn activate(&mut self, thread_manager: &ITfThreadMgr, client_id: u32) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.source.is_some() || self.sink.is_some() || self.cookie.is_some() {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        // SAFETY: this accesses only the current user's integer-valued global
        // TSF compartments; it does not inspect any document or input text.
        let compartments = unsafe { thread_manager.GetGlobalCompartment() }?;
        let command_compartment =
            unsafe { compartments.GetCompartment(&self.shared.command_guid) }?;
        let acknowledgement_compartment =
            unsafe { compartments.GetCompartment(&self.acknowledgement_guid) }?;
        let source: ITfSource = command_compartment.cast()?;
        self.shared
            .configure(command_compartment, acknowledgement_compartment, client_id);
        let sink: ITfCompartmentEventSink = NativeWishCommandSink {
            shared: Rc::downgrade(&self.shared),
        }
        .into();
        // SAFETY: `source` retains the sink until the matching UnadviseSink.
        let cookie = match unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, &sink) } {
            Ok(cookie) => cookie,
            Err(error) => {
                self.shared.clear();
                return Err(error);
            }
        };
        self.source = Some(source);
        self.sink = Some(sink);
        self.cookie = Some(cookie);
        // A command published between the baseline read and AdviseSink is not
        // stale and should be handled once.
        if let Err(error) = self.shared.handle_change() {
            let _ = self.deactivate();
            return Err(error);
        }
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        let source = self.source.take();
        let sink = self.sink.take();
        let cookie = self.cookie.take();
        let result = match (source, sink, cookie) {
            (Some(source), Some(_sink), Some(cookie)) => {
                // SAFETY: balances the successful AdviseSink above.
                unsafe { source.UnadviseSink(cookie) }
            }
            (None, None, None) => Ok(()),
            _ => Err(lifecycle_error(E_UNEXPECTED)),
        };
        self.shared.clear();
        result
    }
}

impl Drop for NativeWishCommandController {
    fn drop(&mut self) {
        let _ = self.deactivate();
    }
}

fn native_feedback_event_code(event: &NativeFeedbackEvent) -> Option<&str> {
    match event {
        NativeFeedbackEvent::CandidatesPresented { code, .. }
        | NativeFeedbackEvent::CandidateCommitted { code, .. }
        | NativeFeedbackEvent::RawCodeCommitted { code }
        | NativeFeedbackEvent::CompositionCancelled { code, .. } => Some(code),
        NativeFeedbackEvent::CandidatePopupTiming { .. } => None,
    }
}

fn native_feedback_monotonic_ms() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    u64::try_from(ORIGIN.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn classify_input_scopes(scopes: &[InputScope]) -> NativeFeedbackContext {
    if scopes.is_empty() {
        return NativeFeedbackContext::Unknown;
    }
    if scopes.iter().any(|scope| {
        matches!(
            *scope,
            IS_PASSWORD
                | IS_NUMERIC_PASSWORD
                | IS_NUMERIC_PIN
                | IS_ALPHANUMERIC_PIN
                | IS_ALPHANUMERIC_PIN_SET
        )
    }) {
        return NativeFeedbackContext::Password;
    }
    if scopes.contains(&IS_PRIVATE) {
        return NativeFeedbackContext::Private;
    }
    if scopes.iter().any(|scope| !(-5..=68).contains(&scope.0)) {
        return NativeFeedbackContext::Unknown;
    }
    if scopes.iter().all(|scope| {
        matches!(
            *scope,
            IS_DEFAULT
                | IS_TEXT
                | IS_CHAT
                | IS_CHAT_WITHOUT_EMOJI
                | IS_SEARCH
                | IS_SEARCH_INCREMENTAL
                | IS_CHINESE_FULLWIDTH
                | IS_CHINESE_HALFWIDTH
                | IS_NATIVE_SCRIPT
        )
    }) {
        NativeFeedbackContext::Eligible
    } else {
        NativeFeedbackContext::Restricted
    }
}

fn read_context_compartment_flag(context: &ITfContext, guid: &GUID) -> Option<bool> {
    let manager: ITfCompartmentMgr = context.cast().ok()?;
    // SAFETY: the fixed GUID identifies a read-only context compartment.
    let compartment = unsafe { manager.GetCompartment(guid) }.ok()?;
    // SAFETY: reading a compartment value does not mutate host state.
    let value = unsafe { compartment.GetValue() }.ok()?;
    if value.is_empty() {
        return Some(false);
    }
    bool::try_from(&value)
        .ok()
        .or_else(|| i32::try_from(&value).ok().map(|value| value != 0))
        .or_else(|| u32::try_from(&value).ok().map(|value| value != 0))
}

fn classify_tsf_feedback_context(
    context: &ITfContext,
    range: &ITfRange,
    ec: u32,
) -> NativeFeedbackContext {
    match read_context_compartment_flag(context, &GUID_COMPARTMENT_KEYBOARD_DISABLED) {
        Some(true) => return NativeFeedbackContext::KeyboardDisabled,
        Some(false) => {}
        None => return NativeFeedbackContext::Unknown,
    }
    match read_context_compartment_flag(context, &GUID_COMPARTMENT_EMPTYCONTEXT) {
        Some(true) => return NativeFeedbackContext::Empty,
        Some(false) => {}
        None => return NativeFeedbackContext::Unknown,
    }

    // SAFETY: the edit cookie grants read access to this range. The app
    // property is owned by the context and the returned VARIANT owns its
    // interface reference.
    let property = match unsafe { context.GetAppProperty(&GUID_PROP_INPUTSCOPE) } {
        Ok(property) => property,
        Err(_) => return NativeFeedbackContext::Unknown,
    };
    let value = match unsafe { property.GetValue(ec, range) } {
        Ok(value) => value,
        Err(_) => return NativeFeedbackContext::Unknown,
    };
    let unknown = match IUnknown::try_from(&value) {
        Ok(unknown) => unknown,
        Err(_) => return NativeFeedbackContext::Unknown,
    };
    let input_scope: ITfInputScope = match unknown.cast() {
        Ok(input_scope) => input_scope,
        Err(_) => return NativeFeedbackContext::Unknown,
    };

    let mut scopes = ptr::null_mut();
    let mut count = 0_u32;
    // SAFETY: ITfInputScope allocates this output with CoTaskMemAlloc. It is
    // released below on both success and failure.
    let read_result = unsafe { input_scope.GetInputScopes(&mut scopes, &mut count) };
    let classification = if read_result.is_ok() && !scopes.is_null() {
        usize::try_from(count)
            .ok()
            .filter(|count| *count <= 32)
            .map(|count| {
                // SAFETY: a successful call returned exactly `count` entries.
                classify_input_scopes(unsafe { std::slice::from_raw_parts(scopes, count) })
            })
            .unwrap_or(NativeFeedbackContext::Unknown)
    } else {
        NativeFeedbackContext::Unknown
    };
    if !scopes.is_null() {
        // SAFETY: balances the ITfInputScope allocation above.
        unsafe { CoTaskMemFree(Some(scopes.cast())) };
    }
    classification
}

fn classify_feedback_context(
    context: &ITfContext,
    range: &ITfRange,
    ec: u32,
    mode: KeyAdviceMode,
) -> NativeFeedbackContext {
    match mode {
        KeyAdviceMode::Foreground => classify_tsf_feedback_context(context, range, ec),
        KeyAdviceMode::SyntheticHost => NativeFeedbackContext::Eligible,
    }
}

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
struct TsfTextService {
    activation: Mutex<ActivationState>,
    composition: Rc<RefCell<CompositionSession>>,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    candidate_provider: Option<Arc<dyn CandidateProvider>>,
    candidate_cache: RefCell<CandidateCache>,
    selection_memory: RefCell<SessionSelectionMemory>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    edit_telemetry: Arc<Mutex<EditSessionTelemetry>>,
    native_feedback: Arc<Mutex<NativeFeedbackSession>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    native_feedback_language_bar: RefCell<NativeFeedbackLanguageBarController>,
    native_wish_commands: RefCell<NativeWishCommandController>,
    input_mode: Rc<Cell<InputMode>>,
    shift_tap_armed: Cell<bool>,
    shift_chord_pending: Cell<bool>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfTextService {
    fn counted_with_options(
        candidate_provider: Option<Arc<dyn CandidateProvider>>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        let native_feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let native_feedback_context = Arc::new(Mutex::new(NativeFeedbackContextCache::default()));
        let input_mode = Rc::new(Cell::new(InputMode::Chinese));
        let native_feedback_language_bar_state = Rc::new(NativeFeedbackLanguageBarState::new(
            Arc::clone(&native_feedback),
            Arc::clone(&native_feedback_context),
            Rc::clone(&input_mode),
        ));
        let mut candidate_ui =
            CandidateUiController::new(matches!(key_advice_mode, KeyAdviceMode::Foreground));
        candidate_ui.attach_feedback(
            Arc::downgrade(&native_feedback),
            Rc::downgrade(&native_feedback_language_bar_state),
        );
        let native_wish_commands = RefCell::new(NativeWishCommandController::new(
            matches!(key_advice_mode, KeyAdviceMode::Foreground),
            Rc::downgrade(&native_feedback_language_bar_state),
        ));
        Self {
            activation: Mutex::new(ActivationState::default()),
            composition: Rc::new(RefCell::new(CompositionSession::default())),
            document_composition: Rc::new(RefCell::new(DocumentCompositionState::default())),
            candidate_provider,
            candidate_cache: RefCell::new(CandidateCache::default()),
            selection_memory: RefCell::new(SessionSelectionMemory::default()),
            candidate_ui: Rc::new(RefCell::new(candidate_ui)),
            edit_telemetry: Arc::new(Mutex::new(EditSessionTelemetry::default())),
            native_feedback,
            native_feedback_context,
            native_feedback_language_bar: RefCell::new(NativeFeedbackLanguageBarController::new(
                matches!(key_advice_mode, KeyAdviceMode::Foreground),
                Rc::clone(&native_feedback_language_bar_state),
            )),
            native_wish_commands,
            native_feedback_language_bar_state,
            input_mode,
            shift_tap_armed: Cell::new(false),
            shift_chord_pending: Cell::new(false),
            key_advice_mode,
        }
    }

    #[cfg(test)]
    fn counted_for_process_test(candidate_provider: Option<Arc<dyn CandidateProvider>>) -> Self {
        Self::counted_with_options(candidate_provider, KeyAdviceMode::SyntheticHost)
    }

    #[cfg(test)]
    fn counted_for_process_test_with_feedback(
        candidate_provider: Option<Arc<dyn CandidateProvider>>,
        limits: NativeFeedbackLimits,
    ) -> Self {
        let mut service =
            Self::counted_with_options(candidate_provider, KeyAdviceMode::SyntheticHost);
        service.candidate_ui = Rc::new(RefCell::new(CandidateUiController::new_headless()));
        assert_eq!(
            service
                .native_feedback
                .lock()
                .expect("fresh feedback lock")
                .start_memory(NativeFeedbackAuthorization::explicit_memory_only(), limits),
            crate::NativeFeedbackStartResult::Started
        );
        service
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

    fn load_candidate_batch(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
    ) -> Result<CandidateBatch> {
        let mut batch = self
            .candidate_cache
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .load(provider, code, limit, view);
        if view == InteractiveCandidateView::Primary {
            self.selection_memory
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                .promote_texts(code, &mut batch.candidates);
        }
        Ok(batch)
    }

    fn has_active_logical_composition(&self) -> Result<bool> {
        Ok(!self
            .composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .phonetic()
            .is_empty())
    }

    fn can_handle_shift_tap(&self, modifiers: KeyModifiers) -> bool {
        !modifiers.control && !modifiers.alt && !modifiers.windows
    }

    fn observe_nonshift_test_modifiers(&self, mut modifiers: KeyModifiers) -> KeyModifiers {
        let shift_chord = self.shift_tap_armed.replace(false) || self.shift_chord_pending.get();
        self.shift_chord_pending.set(shift_chord);
        modifiers.shift |= shift_chord;
        modifiers
    }

    fn observe_nonshift_key_down_modifiers(&self, mut modifiers: KeyModifiers) -> KeyModifiers {
        let shift_chord =
            self.shift_chord_pending.replace(false) || self.shift_tap_armed.replace(false);
        modifiers.shift |= shift_chord;
        modifiers
    }

    fn direct_input_needs_commit(&self, vkey: u16, modifiers: KeyModifiers) -> Result<bool> {
        if self.input_mode.get() != InputMode::Chinese || !self.has_active_logical_composition()? {
            return Ok(false);
        }
        if vkey == VK_CAPITAL.0 {
            return Ok(true);
        }
        if modifiers.control || modifiers.alt || modifiers.windows {
            return Ok(false);
        }
        Ok(is_host_printable_key(vkey)
            && decode_virtual_key(vkey, modifiers, InputMode::Chinese).is_none())
    }

    fn commit_active_composition(&self, context: Ref<ITfContext>) -> Result<()> {
        if !self.has_active_logical_composition()? {
            return Ok(());
        }
        if !self
            .apply_key(context, WPARAM(usize::from(VK_SPACE.0)))?
            .as_bool()
        {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        Ok(())
    }

    fn toggle_input_mode(&self, context: Ref<ITfContext>) -> Result<()> {
        self.commit_active_composition(context)?;
        self.input_mode.set(self.input_mode.get().toggled());
        self.native_feedback_language_bar_state.notify();
        Ok(())
    }

    fn request_document_edit_session(
        &self,
        context: &ITfContext,
        client_id: u32,
        request: DocumentEditRequest,
    ) -> Result<()> {
        let DocumentEditRequest {
            action,
            candidate_display,
            feedback_after_success,
            mode,
            cleanup_target,
        } = request;
        let edit_session: ITfEditSession = TsfDocumentEditSession::counted(
            context.clone(),
            action,
            EditSessionShared {
                document_composition: Rc::clone(&self.document_composition),
                logical_composition: Rc::clone(&self.composition),
                telemetry: Arc::clone(&self.edit_telemetry),
                candidate_ui: Rc::clone(&self.candidate_ui),
                native_feedback: Arc::clone(&self.native_feedback),
                native_feedback_context: Arc::clone(&self.native_feedback_context),
                native_feedback_language_bar_state: Rc::clone(
                    &self.native_feedback_language_bar_state,
                ),
                key_advice_mode: self.key_advice_mode,
            },
            candidate_display,
            feedback_after_success,
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
            DocumentEditRequest {
                action: PendingDocumentEdit::Cancel,
                candidate_display: None,
                feedback_after_success: self.composition.try_borrow().ok().and_then(
                    |composition| {
                        (!composition.phonetic().is_empty()).then(|| {
                            NativeFeedbackEvent::CompositionCancelled {
                                code: composition.phonetic().to_owned(),
                                source: NativeCancellationSource::FocusLoss,
                            }
                        })
                    },
                ),
                mode: EditSessionMode::CleanupAsync,
                cleanup_target: Some(cleanup_target),
            },
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
        self.shift_tap_armed.set(false);
        self.shift_chord_pending.set(false);
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
        if let Ok(mut language_bar) = self.native_feedback_language_bar.try_borrow_mut() {
            // The language-bar surface is optional. Failure keeps feedback
            // disabled and must not prevent the input method from activating.
            let _ = language_bar.activate(&ui_thread_manager);
        }
        if let Ok(mut commands) = self.native_wish_commands.try_borrow_mut() {
            // The external control surface is optional. Failure must not
            // prevent the input method itself from activating.
            let _ = commands.activate(&ui_thread_manager, client_id);
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
        let Some(mut input) = decode_virtual_key(vkey, modifiers, self.input_mode.get()) else {
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
        if matches!(&input, CompositionInput::EnterTab)
            && session.phonetic() == INLINE_WISH_TRIGGER_CODE
            && !session.tab_mode()
            && !session.recovery_mode()
            && !session.wish_prompt()
        {
            input = CompositionInput::EnterWish;
        }
        let wish_action = (session.wish_prompt() || matches!(&input, CompositionInput::EnterWish))
            .then(|| {
                inline_wish_action(
                    self.native_feedback_language_bar_state
                        .summary()
                        .unwrap_or_default(),
                )
            });
        let needs_existing_candidates = !session.wish_prompt()
            && matches!(
                &input,
                CompositionInput::Confirm
                    | CompositionInput::Punctuation(_)
                    | CompositionInput::Select(_)
                    | CompositionInput::PreviousPage
                    | CompositionInput::NextPage
            );
        let existing_batch = if needs_existing_candidates && !session.phonetic().is_empty() {
            let limit = if matches!(&input, CompositionInput::NextPage) {
                candidate_next_page_limit(session.candidate_page_start())
            } else {
                candidate_visible_limit(session.candidate_page_start())
            };
            self.load_candidate_batch(
                provider.as_ref(),
                session.phonetic(),
                limit,
                if session.recovery_mode() {
                    InteractiveCandidateView::TranspositionRecovery
                } else {
                    InteractiveCandidateView::Primary
                },
            )?
        } else {
            CandidateBatch::default()
        };
        let selected_text = match &input {
            CompositionInput::Confirm | CompositionInput::Punctuation(_) => {
                existing_batch.candidates.first().cloned()
            }
            CompositionInput::Select(rank) => {
                let absolute = session
                    .candidate_page_start()
                    .saturating_add(rank.saturating_sub(1));
                existing_batch.candidates.get(absolute).cloned()
            }
            _ => None,
        };
        let selection_to_remember = (matches!(&input, CompositionInput::Select(_))
            && !session.recovery_mode()
            && !session.tab_mode())
        .then(|| {
            selected_text.as_ref().map(|text| PlannedSelection {
                code: session.phonetic().to_owned(),
                text: text.clone(),
            })
        })
        .flatten();
        let selection_feedback = selected_text.as_ref().and_then(|text| {
            let view = native_candidate_view(
                if session.recovery_mode() {
                    InteractiveCandidateView::TranspositionRecovery
                } else {
                    InteractiveCandidateView::Primary
                },
                session.tab_mode(),
            );
            match &input {
                CompositionInput::Confirm => Some(NativeFeedbackEvent::CandidateCommitted {
                    code: session.phonetic().to_owned(),
                    text: text.clone(),
                    view,
                    source: NativeSelectionSource::FirstCandidate,
                    absolute_rank: 1,
                    visible_rank: 1,
                }),
                CompositionInput::Select(rank) => Some(NativeFeedbackEvent::CandidateCommitted {
                    code: session.phonetic().to_owned(),
                    text: text.clone(),
                    view,
                    source: NativeSelectionSource::Numeric,
                    absolute_rank: session.candidate_page_start().saturating_add(*rank),
                    visible_rank: *rank,
                }),
                CompositionInput::Punctuation(_) => Some(NativeFeedbackEvent::CandidateCommitted {
                    code: session.phonetic().to_owned(),
                    text: text.clone(),
                    view,
                    source: NativeSelectionSource::Punctuation,
                    absolute_rank: 1,
                    visible_rank: 1,
                }),
                _ => None,
            }
        });
        let mut plan = match plan_session_input(
            &session,
            input.clone(),
            selected_text.clone(),
            existing_batch.candidates.len(),
        ) {
            Some(plan) => plan,
            None => return Ok(None),
        };
        if session.wish_prompt()
            && matches!(
                &input,
                CompositionInput::Confirm | CompositionInput::Select(1)
            )
            && let Some(action) = wish_action
        {
            plan.action_after_success = Some(PlannedAction::Wish(action.command));
        }
        plan.selection_to_remember = selection_to_remember;
        plan.feedback_after_success = selection_feedback
            .or_else(|| {
                matches!(&input, CompositionInput::CommitRaw).then(|| {
                    NativeFeedbackEvent::RawCodeCommitted {
                        code: session.phonetic().to_owned(),
                    }
                })
            })
            .or_else(|| {
                (plan.action_after_success.is_none()
                    && matches!(&plan.edit, Some(PendingDocumentEdit::Cancel)))
                .then(|| NativeFeedbackEvent::CompositionCancelled {
                    code: session.phonetic().to_owned(),
                    source: match &input {
                        CompositionInput::Backspace => NativeCancellationSource::Backspace,
                        CompositionInput::Escape => NativeCancellationSource::Escape,
                        _ => NativeCancellationSource::HostTermination,
                    },
                })
            });
        if plan.after.wish_prompt() {
            if let Some(action) = wish_action {
                plan.candidate_display =
                    Some(CandidateDisplay::action(action.label, action.detail));
            }
            plan.feedback_after_success = None;
        } else if !plan.after.phonetic().is_empty() {
            let batch = if plan.after.phonetic() == session.phonetic()
                && !existing_batch.candidates.is_empty()
            {
                existing_batch
            } else {
                self.load_candidate_batch(
                    provider.as_ref(),
                    plan.after.phonetic(),
                    candidate_visible_limit(plan.after.candidate_page_start()),
                    if plan.after.recovery_mode() {
                        InteractiveCandidateView::TranspositionRecovery
                    } else {
                        InteractiveCandidateView::Primary
                    },
                )?
            };
            plan.after
                .normalize_candidate_page(batch.candidates.len(), CANDIDATE_PAGE_SIZE);
            let display = CandidateDisplay::from_batch(batch, plan.after.candidate_page_start());
            if plan.feedback_after_success.is_none() {
                plan.feedback_after_success =
                    Some(display.feedback_event(plan.after.phonetic(), plan.after.tab_mode()));
            }
            plan.candidate_display = Some(display);
        }
        Ok(Some(plan))
    }

    fn apply_key(&self, context: Ref<ITfContext>, wparam: WPARAM) -> Result<windows::core::BOOL> {
        self.apply_key_with_modifiers(context, wparam, self.observed_key_modifiers())
    }

    fn apply_key_with_modifiers(
        &self,
        context: Ref<ITfContext>,
        wparam: WPARAM,
        modifiers: KeyModifiers,
    ) -> Result<windows::core::BOOL> {
        let Some(context) = context.cloned() else {
            return Ok(false.into());
        };
        if self.input_mode.get() == InputMode::Chinese
            && !self.has_active_logical_composition()?
            && u16::try_from(wparam.0)
                .ok()
                .and_then(|vkey| decode_virtual_key(vkey, modifiers, self.input_mode.get()))
                .is_some_and(|input| matches!(input, CompositionInput::Letters(_)))
            && self
                .candidate_provider
                .as_ref()
                .is_some_and(|provider| provider.refresh_at_safe_boundary())
        {
            *self
                .candidate_cache
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))? = CandidateCache::default();
        }
        let Some(plan) = self.plan_key(wparam, modifiers)? else {
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
            selection_to_remember,
            feedback_after_success,
            action_after_success,
        } = plan;
        let ui_only = edit.is_none();
        if let Some(edit) = edit {
            self.request_document_edit_session(
                &context,
                client_id,
                DocumentEditRequest {
                    action: edit,
                    candidate_display: candidate_display.clone(),
                    feedback_after_success: feedback_after_success.clone(),
                    mode: EditSessionMode::KeySynchronous,
                    cleanup_target: None,
                },
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
        if let Some(selection) = selection_to_remember
            && let Ok(mut memory) = self.selection_memory.try_borrow_mut()
        {
            memory.remember_text(&selection.code, &selection.text);
        }
        if let Some(PlannedAction::Wish(command)) = action_after_success {
            let _ = self
                .native_feedback_language_bar_state
                .perform_wish_command(command);
        }
        if ui_only && let Some(display) = candidate_display {
            let feedback_context = feedback_after_success
                .as_ref()
                .and_then(native_feedback_event_code)
                .map(|code| {
                    self.native_feedback_context
                        .lock()
                        .map(|cache| cache.context_for(code))
                        .unwrap_or(NativeFeedbackContext::Unknown)
                })
                .unwrap_or(NativeFeedbackContext::Unknown);
            let presented = self
                .candidate_ui
                .try_borrow_mut()
                .map(|mut candidate_ui| candidate_ui.update_contents(display, feedback_context))
                .unwrap_or(false);
            if presented
                && let Some(event) = feedback_after_success
                && let Ok(mut feedback) = self.native_feedback.lock()
                && feedback.is_accepting()
            {
                let record_result =
                    feedback.record_at(feedback_context, event, native_feedback_monotonic_ms());
                drop(feedback);
                if matches!(record_result, NativeFeedbackRecordResult::Stopped(_)) {
                    self.native_feedback_language_bar_state.notify();
                }
            }
        }
        Ok(true.into())
    }
}

impl ITfTextInputProcessor_Impl for TsfTextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> Result<()> {
        self.activate_inner(ptim, tid, 0)
    }

    fn Deactivate(&self) -> Result<()> {
        self.shift_tap_armed.set(false);
        self.shift_chord_pending.set(false);
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
        let language_bar_result = self
            .native_feedback_language_bar
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))
            .and_then(|mut language_bar| language_bar.deactivate());
        let wish_command_result = self
            .native_wish_commands
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))
            .and_then(|mut commands| commands.deactivate());
        let composition_result = match self.composition.try_borrow_mut() {
            Ok(mut composition) => {
                composition.finish_commit();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let selection_memory_result = match self.selection_memory.try_borrow_mut() {
            Ok(mut memory) => {
                memory.clear();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let feedback_context_result = match self.native_feedback_context.lock() {
            Ok(mut context) => {
                context.clear();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let native_feedback_result = match self.native_feedback.lock() {
            Ok(mut feedback) => {
                let _ = feedback.stop();
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
        selection_memory_result?;
        feedback_context_result?;
        native_feedback_result?;
        wish_command_result?;
        language_bar_result?;
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
        let Ok(vkey) = u16::try_from(wparam.0) else {
            return Ok(false.into());
        };
        let mut modifiers = self.observed_key_modifiers();
        if vkey == VK_SHIFT.0 {
            return Ok(self.can_handle_shift_tap(modifiers).into());
        }
        // Any second key turns a held Shift into a chord rather than a
        // standalone Chinese/English mode toggle. This flag is structural
        // input state only; OnTestKeyDown still performs no document edit.
        modifiers = self.observe_nonshift_test_modifiers(modifiers);
        if self.direct_input_needs_commit(vkey, modifiers)? {
            // Ask TSF to route the real key-down callback through us so the
            // current preedit can be committed before the host receives the
            // shifted or Caps Lock character.
            return Ok(true.into());
        }
        Ok(self.plan_key(wparam, modifiers)?.is_some().into())
    }

    fn OnTestKeyUp(
        &self,
        _context: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        let Ok(vkey) = u16::try_from(wparam.0) else {
            return Ok(false.into());
        };
        if vkey != VK_SHIFT.0 {
            return Ok(false.into());
        }
        let standalone = self.shift_tap_armed.get();
        if !standalone {
            self.shift_chord_pending.set(false);
        }
        Ok(standalone.into())
    }

    fn OnKeyDown(
        &self,
        context: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        let Ok(vkey) = u16::try_from(wparam.0) else {
            return Ok(false.into());
        };
        let mut modifiers = self.observed_key_modifiers();
        if vkey == VK_SHIFT.0 {
            if !self.can_handle_shift_tap(modifiers) {
                return Ok(false.into());
            }
            self.shift_chord_pending.set(false);
            self.shift_tap_armed.set(true);
            return Ok(true.into());
        }
        modifiers = self.observe_nonshift_key_down_modifiers(modifiers);
        if self.direct_input_needs_commit(vkey, modifiers)? {
            self.commit_active_composition(context)?;
            // The preedit is finished, but the printable key or Caps Lock
            // key still belongs to the host application.
            return Ok(false.into());
        }
        self.apply_key_with_modifiers(context, wparam, modifiers)
    }

    fn OnKeyUp(
        &self,
        context: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<windows::core::BOOL> {
        let Ok(vkey) = u16::try_from(wparam.0) else {
            return Ok(false.into());
        };
        if vkey == VK_SHIFT.0 {
            self.shift_chord_pending.set(false);
            if self.shift_tap_armed.replace(false) {
                self.toggle_input_mode(context)?;
                return Ok(true.into());
            }
        }
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
        CLSID_TF_ThreadMgr, ITfCompositionView, ITfContextOwnerCompositionServices,
        ITfLangBarItemSink_Impl, TF_ES_READ, TF_POPF_ALL, TF_TF_MOVESTART,
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
    fn installed_builds_share_stable_user_data_roots() {
        let digest = "1".repeat(64);
        let module = PathBuf::from(format!(
            r"D:\repo\.local\tsf-alpha\builds\{digest}\ziranma_core.dll"
        ));
        assert_eq!(
            explicit_alias_root_for_module(&module),
            Some(PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\aliases"))
        );
        assert_eq!(
            wish_root_for_module(&module),
            Some(PathBuf::from(r"D:\repo\.local\tsf-alpha\user-data\wishes"))
        );
        assert_eq!(
            explicit_alias_root_for_module(Path::new(r"D:\repo\target\release\ziranma_core.dll")),
            None
        );
    }

    #[test]
    fn explicit_alias_runtime_refreshes_only_valid_promoted_packages() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-alias-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(crate::EXPLICIT_ALIAS_PACKAGES_DIRECTORY)).unwrap();

        let install = |text: &str| {
            let mut snapshot = ExplicitAliasSnapshot::default();
            snapshot.set("aa", text).unwrap();
            let package =
                crate::protect_explicit_alias_snapshot(&snapshot, &WindowsUserDataProtector)
                    .unwrap();
            let package_id = crate::explicit_alias_package_id(&package);
            let directory = root
                .join(crate::EXPLICIT_ALIAS_PACKAGES_DIRECTORY)
                .join(&package_id);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join(crate::EXPLICIT_ALIAS_PACKAGE_FILE), package).unwrap();
            package_id
        };

        let first = install("甲");
        let mut slots = crate::ExplicitAliasSlotState::default();
        slots.adopt(&first).unwrap();
        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), slots.render()).unwrap();
        let runtime = ExplicitAliasRuntime::new(root.clone());
        assert_eq!(runtime.text("aa").as_deref(), Some("甲"));

        let second = install("乙");
        slots.stage(&second).unwrap();
        slots.promote().unwrap();
        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), slots.render()).unwrap();
        assert_eq!(runtime.text("aa").as_deref(), Some("甲"));
        assert!(runtime.refresh());
        assert_eq!(runtime.text("aa").as_deref(), Some("乙"));

        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), "broken\n").unwrap();
        assert!(!runtime.refresh());
        assert_eq!(runtime.text("aa").as_deref(), Some("乙"));
        fs::remove_dir_all(root).unwrap();
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
        let result = candidate_provider_for_root(&root, None);
        fs::remove_dir(&root).unwrap();
        assert!(matches!(
            result,
            Err(CandidateProviderLoadError::Runtime(
                CandidateRuntimeError::SlotStateUnavailable
            ))
        ));
    }

    static TEST_COM_STATE_LOCK: Mutex<()> = Mutex::new(());

    struct TestLockGuard {
        _state: std::sync::MutexGuard<'static, ()>,
        _host: std::sync::MutexGuard<'static, ()>,
    }

    fn test_state_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_COM_STATE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn test_lock() -> TestLockGuard {
        let state = test_state_lock();
        let host = SYNTHETIC_HOST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TestLockGuard {
            _state: state,
            _host: host,
        }
    }

    #[implement(ITfLangBarItemSink)]
    struct TestLanguageBarSink {
        updates: Arc<AtomicUsize>,
        flags: Arc<AtomicUsize>,
    }

    impl ITfLangBarItemSink_Impl for TestLanguageBarSink_Impl {
        fn OnUpdate(&self, flags: u32) -> Result<()> {
            self.updates.fetch_add(1, Ordering::AcqRel);
            self.flags.fetch_or(
                usize::try_from(flags).unwrap_or(usize::MAX),
                Ordering::AcqRel,
            );
            Ok(())
        }
    }

    struct FixedCandidateProvider;

    impl CandidateProvider for FixedCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code == "a" && limit > 0 && view == InteractiveCandidateView::Primary {
                vec!["啊".to_owned()]
            } else {
                Vec::new()
            }
        }
    }

    struct CountingCandidateProvider {
        calls: AtomicUsize,
        total: usize,
    }

    impl CandidateProvider for CountingCandidateProvider {
        fn candidates(
            &self,
            _code: &str,
            limit: usize,
            _view: InteractiveCandidateView,
        ) -> Vec<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            (0..limit.min(self.total))
                .map(|index| format!("候选{}", index + 1))
                .collect()
        }
    }

    struct RecoveryCandidateProvider;

    impl CandidateProvider for RecoveryCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "ab" || limit == 0 {
                return Vec::new();
            }
            vec![match view {
                InteractiveCandidateView::Primary => "普通候选".to_owned(),
                InteractiveCandidateView::TranspositionRecovery => "换序候选".to_owned(),
            }]
        }
    }

    struct SelectionCandidateProvider;

    impl CandidateProvider for SelectionCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "ab" || limit == 0 {
                return Vec::new();
            }
            let candidates = match view {
                InteractiveCandidateView::Primary => ["甲", "乙", "丙"],
                InteractiveCandidateView::TranspositionRecovery => ["换序甲", "换序乙", "换序丙"],
            };
            candidates
                .into_iter()
                .take(limit)
                .map(str::to_owned)
                .collect()
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
        let candidates =
            provider.candidates("nihk", CANDIDATE_LIMIT, InteractiveCandidateView::Primary);
        assert_eq!(candidates.first().map(String::as_str), Some("你好"));
        assert!(candidates.len() <= CANDIDATE_LIMIT);
        assert!(
            provider
                .candidates("nihk", 0, InteractiveCandidateView::Primary)
                .is_empty()
        );
        assert_eq!(
            provider.candidates("zzzzzzzz", 1, InteractiveCandidateView::Primary),
            ["zzzzzzzz"]
        );
        for (code, expected) in [
            ("wuwa", "呜哇"),
            ("yidair", "一大串"),
            ("duuuyu", "独属于"),
            ("bugfub", "不跟手"),
        ] {
            assert_eq!(
                provider
                    .candidates(code, 7, InteractiveCandidateView::Primary)
                    .first()
                    .map(String::as_str),
                Some(expected),
                "an exact project overlay entry must precede snapshot fallbacks for {code}"
            );
        }
    }

    #[test]
    fn public_preflight_api_commits_snapshot_candidate_without_retaining_text() {
        let _guard = test_state_lock();
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

        let press = |vkey: u16| {
            let key = WPARAM(usize::from(vkey));
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
        };
        press(VK_OEM_COMMA.0);
        assert_eq!(read_context_text(&context, client_id), "你好，");

        for offset in [13_u16, 8, 7, 10] {
            press(VK_A.0 + offset);
        }
        press(VK_OEM_PERIOD.0);
        assert_eq!(read_context_text(&context, client_id), "你好，你好。");

        press(VK_A.0 + 9);
        press(VK_A.0 + 20);
        press(VK_RETURN.0);
        assert_eq!(read_context_text(&context, client_id), "你好，你好。ju");

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
    fn process_test_native_feedback_orders_present_commit_recovery_and_cancel_events() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object =
            ComObject::new(TsfTextService::counted_for_process_test_with_feedback(
                Some(Arc::new(SelectionCandidateProvider)),
                NativeFeedbackLimits::default(),
            ));
        let service: ITfTextInputProcessorEx = service_object.to_interface();
        let key_sink: ITfKeyEventSink = service_object.to_interface();

        // SAFETY: COM is initialized and every object remains on this
        // apartment thread until the exact reverse cleanup below.
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        let client_id = unsafe { thread_manager.Activate() }.expect("thread manager activation");
        unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect("process-test activation should succeed");
        let document_manager =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("document manager creation");
        let mut context = None;
        let mut text_store_cookie = 0;
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
        unsafe { document_manager.Push(&context) }.expect("context push");
        unsafe { thread_manager.SetFocus(&document_manager) }.expect("document focus");

        let lparam = LPARAM(0);
        let press = |vkey: u16| {
            let key = WPARAM(usize::from(vkey));
            assert!(
                unsafe { key_sink.OnTestKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool()
            );
            assert!(
                unsafe { key_sink.OnKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool(),
                "OnKeyDown did not handle virtual key {vkey}"
            );
        };

        press(VK_A.0);
        press(VK_A.0 + 1);
        press(VK_1.0 + 1);
        assert_eq!(read_context_text(&context, client_id), "乙");

        press(VK_A.0);
        press(VK_A.0 + 1);
        press(VK_SPACE.0);
        assert_eq!(read_context_text(&context, client_id), "乙乙");

        press(VK_A.0);
        press(VK_A.0 + 1);
        // The focused Shift-callback test covers TSF's paired test/delivery
        // callbacks. Here we inject the already-observed chord so this test
        // stays about feedback after the UI-only recovery transition.
        service_object.shift_chord_pending.set(true);
        assert!(
            unsafe { key_sink.OnKeyDown(&context, WPARAM(usize::from(VK_TAB.0)), lparam) }
                .unwrap()
                .as_bool()
        );
        press(VK_1.0);
        assert_eq!(read_context_text(&context, client_id), "乙乙换序甲");

        press(VK_A.0);
        press(VK_BACK.0);
        press(VK_A.0);
        press(VK_ESCAPE.0);
        assert_eq!(read_context_text(&context, client_id), "乙乙换序甲");

        {
            let feedback = service_object.native_feedback.lock().unwrap();
            let events = feedback.events();
            assert_eq!(events.len(), 9);
            assert!(matches!(
                &events[0],
                NativeFeedbackEvent::CandidatesPresented {
                    code,
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates,
                    ..
                } if code == "ab" && candidates == &["甲", "乙", "丙"]
            ));
            assert!(matches!(
                &events[1],
                NativeFeedbackEvent::CandidateCommitted {
                    code,
                    text,
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::Numeric,
                    absolute_rank: 2,
                    visible_rank: 2,
                } if code == "ab" && text == "乙"
            ));
            assert!(matches!(
                &events[3],
                NativeFeedbackEvent::CandidateCommitted {
                    source: NativeSelectionSource::FirstCandidate,
                    absolute_rank: 1,
                    visible_rank: 1,
                    ..
                }
            ));
            assert!(matches!(
                &events[5],
                NativeFeedbackEvent::CandidatesPresented {
                    view: NativeCandidateView::TranspositionRecovery,
                    ..
                }
            ));
            assert!(matches!(
                &events[6],
                NativeFeedbackEvent::CandidateCommitted {
                    view: NativeCandidateView::TranspositionRecovery,
                    source: NativeSelectionSource::Numeric,
                    ..
                }
            ));
            assert!(matches!(
                &events[7],
                NativeFeedbackEvent::CompositionCancelled {
                    code,
                    source: NativeCancellationSource::Backspace,
                } if code == "a"
            ));
            assert!(matches!(
                &events[8],
                NativeFeedbackEvent::CompositionCancelled {
                    code,
                    source: NativeCancellationSource::Escape,
                } if code == "a"
            ));
            let summary = feedback.summary();
            assert!(summary.complete);
            assert_eq!(summary.candidate_pages, 4);
            assert_eq!(summary.commits, 3);
            assert_eq!(summary.cancellations, 2);
        }

        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        {
            let feedback = service_object.native_feedback.lock().unwrap();
            let summary = feedback.summary();
            assert_eq!(summary.lifecycle, crate::NativeFeedbackLifecycle::Stopped);
            assert!(summary.complete);
            assert_eq!(summary.events, 9);
        }
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(key_sink);
        drop(service);
        drop(service_object);
        drop(thread_manager);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }

    #[test]
    fn process_test_xuy_tab_space_cancels_the_mnemonic_and_starts_feedback() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(SelectionCandidateProvider),
        )));
        let service: ITfTextInputProcessorEx = service_object.to_interface();
        let key_sink: ITfKeyEventSink = service_object.to_interface();
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        let client_id = unsafe { thread_manager.Activate() }.expect("thread manager activation");
        unsafe { service.ActivateEx(&thread_manager, client_id, 0) }
            .expect("process-test activation should succeed");
        let document_manager =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("document manager creation");
        let mut context = None;
        let mut text_store_cookie = 0;
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
        unsafe { document_manager.Push(&context) }.expect("context push");
        unsafe { thread_manager.SetFocus(&document_manager) }.expect("document focus");

        let lparam = LPARAM(0);
        let press = |vkey: u16| {
            let key = WPARAM(usize::from(vkey));
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
        };
        press(VK_A.0 + 23);
        press(VK_A.0 + 20);
        press(VK_A.0 + 24);
        assert_eq!(read_context_text(&context, client_id), "xuy");
        press(VK_TAB.0);
        assert_eq!(read_context_text(&context, client_id), "xuy");
        press(VK_SPACE.0);
        assert_eq!(read_context_text(&context, client_id), "");
        assert_eq!(
            service_object
                .native_feedback
                .lock()
                .unwrap()
                .summary()
                .lifecycle,
            NativeFeedbackLifecycle::Recording
        );

        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(key_sink);
        drop(service);
        drop(service_object);
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
        let shift = WPARAM(usize::from(VK_SHIFT.0));
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

        // A standalone Shift toggles the service-local mode only on key-up.
        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.input_mode.get(), InputMode::Chinese);
        assert!(
            unsafe { key_sink.OnTestKeyUp(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyUp(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.input_mode.get(), InputMode::English);
        assert!(
            !unsafe { key_sink.OnTestKeyDown(&context, b, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(read_context_text(&context, client_id), "啊");

        // Holding Shift with another key is a host chord and must not toggle
        // the mode when Shift is released.
        assert!(
            unsafe { key_sink.OnKeyDown(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            !unsafe { key_sink.OnTestKeyDown(&context, b, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            !unsafe { key_sink.OnTestKeyUp(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.input_mode.get(), InputMode::English);

        assert!(
            unsafe { key_sink.OnKeyDown(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyUp(&context, shift, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.input_mode.get(), InputMode::Chinese);

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
            decode_virtual_key(VK_A.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::Letters("a".to_owned()))
        );
        assert_eq!(
            decode_virtual_key(
                VK_A.0,
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese,
            ),
            None
        );
        assert_eq!(
            decode_virtual_key(
                VK_TAB.0,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese,
            ),
            Some(CompositionInput::EnterRecovery)
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_MINUS.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::PreviousPage)
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_PLUS.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::NextPage)
        );
        assert_eq!(
            decode_virtual_key(VK_6.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::Select(6))
        );
        assert_eq!(
            decode_virtual_key(VK_6.0 + 1, KeyModifiers::default(), InputMode::Chinese),
            None
        );
        assert_eq!(
            decode_virtual_key(VK_RETURN.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::CommitRaw)
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_COMMA.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::Punctuation(CompositionPunctuation::Comma))
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_PERIOD.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::Period
            ))
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_1.0, KeyModifiers::default(), InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::Semicolon
            ))
        );
        let shifted = KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        };
        assert_eq!(
            decode_virtual_key(VK_OEM_1.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(CompositionPunctuation::Colon))
        );
        assert_eq!(
            decode_virtual_key(VK_1.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::ExclamationMark
            ))
        );
        assert_eq!(
            decode_virtual_key(VK_1.0 + 1, shifted, InputMode::Chinese),
            None,
            "shifted digits without an assigned Chinese punctuation must not select candidates"
        );
        assert_eq!(
            decode_virtual_key(VK_9.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::LeftParenthesis
            ))
        );
        assert_eq!(
            decode_virtual_key(VK_0.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::RightParenthesis
            ))
        );
        assert_eq!(
            decode_virtual_key(VK_OEM_2.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::QuestionMark
            ))
        );
        assert_eq!(
            decode_virtual_key(
                VK_OEM_COMMA.0,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese
            ),
            None
        );
        assert_eq!(
            decode_virtual_key(
                VK_OEM_PERIOD.0,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese
            ),
            None
        );
    }

    #[test]
    fn english_mode_and_modified_letters_are_left_to_the_host() {
        assert_eq!(
            decode_virtual_key(VK_A.0, KeyModifiers::default(), InputMode::English),
            None
        );
        assert_eq!(
            decode_virtual_key(
                VK_A.0,
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese,
            ),
            None
        );
        assert_eq!(
            decode_virtual_key(
                VK_A.0,
                KeyModifiers {
                    caps_lock: true,
                    ..KeyModifiers::default()
                },
                InputMode::Chinese,
            ),
            None
        );
        assert_eq!(InputMode::Chinese.toggled(), InputMode::English);
        assert_eq!(InputMode::English.toggled(), InputMode::Chinese);
    }

    #[test]
    fn feedback_scope_policy_is_fail_closed_and_sensitive_scope_wins() {
        assert_eq!(
            classify_input_scopes(&[IS_TEXT]),
            NativeFeedbackContext::Eligible
        );
        assert_eq!(
            classify_input_scopes(&[IS_CHAT, IS_CHINESE_FULLWIDTH]),
            NativeFeedbackContext::Eligible
        );
        assert_eq!(
            classify_input_scopes(&[IS_TEXT, IS_PASSWORD]),
            NativeFeedbackContext::Password
        );
        assert_eq!(
            classify_input_scopes(&[IS_TEXT, IS_NUMERIC_PIN]),
            NativeFeedbackContext::Password
        );
        assert_eq!(
            classify_input_scopes(&[IS_TEXT, IS_PRIVATE]),
            NativeFeedbackContext::Private
        );
        assert_eq!(
            classify_input_scopes(&[InputScope(1)]),
            NativeFeedbackContext::Restricted
        );
        assert_eq!(
            classify_input_scopes(&[InputScope(999)]),
            NativeFeedbackContext::Unknown
        );
        assert_eq!(classify_input_scopes(&[]), NativeFeedbackContext::Unknown);
    }

    #[test]
    fn feedback_language_bar_menu_uses_plain_lifecycle_actions() {
        let disabled = feedback_language_bar_menu(
            NativeFeedbackSummary::default(),
            WishSaveStatus::Never,
            true,
        );
        assert_eq!(disabled[0].0, FEEDBACK_MENU_START);
        assert_eq!(disabled[0].2, "开始反馈（暂不保存）");
        assert_eq!(
            disabled.last().unwrap().2,
            "反馈默认暂不保存；许愿才写当前用户加密文件；不联网"
        );

        let recording = feedback_language_bar_menu(
            NativeFeedbackSummary {
                lifecycle: NativeFeedbackLifecycle::Recording,
                enabled: true,
                accepting: true,
                complete: true,
                events: 7,
                ..NativeFeedbackSummary::default()
            },
            WishSaveStatus::Never,
            true,
        );
        assert_eq!(recording[0].0, FEEDBACK_MENU_WISH);
        assert_eq!(recording[0].2, "向猫猫许愿（保存近 30 秒）");
        assert_eq!(recording[0].1 & TF_LBMENUF_GRAYED, 0);
        assert_eq!(recording[1].0, FEEDBACK_MENU_STOP);
        assert_eq!(recording[1].2, "停止反馈");
        assert_eq!(recording[2].2, "记录中（7 条）");
        assert_ne!(recording[2].1 & TF_LBMENUF_CHECKED, 0);
        assert_eq!(recording[3].0, FEEDBACK_MENU_TIMING_BUCKETS);
        assert_eq!(
            recording[3].2,
            "双拼间隔（ms）：<8 / 8–15 / 16–23 / 24–31 / 32–47 / 48–63 / 64–95 / 96–159 / ≥160"
        );
        assert_eq!(recording[4].0, FEEDBACK_MENU_TIMING_COUNTS);
        assert_eq!(
            recording[4].2,
            "计数：0 / 0 / 0 / 0 / 0 / 0 / 0 / 0 / 0（共 0 个）"
        );
        assert_eq!(
            recording.last().unwrap().2,
            "反馈默认暂不保存；许愿才写当前用户加密文件；不联网"
        );

        let stopped = feedback_language_bar_menu(
            NativeFeedbackSummary {
                lifecycle: NativeFeedbackLifecycle::Stopped,
                enabled: true,
                complete: false,
                events: 9,
                half_pair_gap_samples: 3,
                half_pair_gap_histogram: [0, 1, 0, 2, 0, 0, 0, 0, 0],
                ..NativeFeedbackSummary::default()
            },
            WishSaveStatus::Never,
            true,
        );
        assert_eq!(stopped[0].0, FEEDBACK_MENU_CLEAR);
        assert_eq!(stopped[0].2, "清除本轮");
        assert_eq!(stopped[1].2, "已停止且不完整（9 条）");
        assert_eq!(
            stopped[3].2,
            "计数：0 / 1 / 0 / 2 / 0 / 0 / 0 / 0 / 0（共 3 个）"
        );
    }

    #[test]
    fn feedback_menu_flags_map_to_native_popup_semantics() {
        assert_eq!(feedback_native_menu_flags(0), MF_STRING);
        assert_eq!(
            feedback_native_menu_flags(TF_LBMENUF_GRAYED | TF_LBMENUF_CHECKED),
            MF_STRING | MF_GRAYED | MF_CHECKED
        );
        assert_eq!(
            feedback_native_menu_flags(TF_LBMENUF_SEPARATOR),
            MF_SEPARATOR
        );
    }

    #[test]
    fn explicit_wish_action_saves_a_recent_dpapi_snapshot_and_keeps_recording() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-wish-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let context = Arc::new(Mutex::new(NativeFeedbackContextCache::default()));
        let mode = Rc::new(Cell::new(InputMode::Chinese));
        let state = NativeFeedbackLanguageBarState::with_wish_root(
            Arc::clone(&feedback),
            context,
            mode,
            Some(root.clone()),
        );
        assert!(state.perform_feedback_action(FEEDBACK_MENU_START).unwrap());
        let now = native_feedback_monotonic_ms();
        feedback.lock().unwrap().record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::CandidatesPresented {
                code: "aa".to_owned(),
                view: NativeCandidateView::Ordinary,
                page_start: 0,
                candidates: vec!["甲".to_owned()],
                may_have_more: false,
            },
            now,
        );

        assert!(state.perform_feedback_action(FEEDBACK_MENU_WISH).unwrap());
        assert!(matches!(
            state.wish_save_status.get(),
            WishSaveStatus::Saved { events: 1 }
        ));
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        let packages = crate::list_wish_packages(&root).unwrap();
        assert_eq!(packages.len(), 1);
        let loaded =
            crate::load_wish_snapshot(&root, packages[0].id(), &WindowsUserDataProtector).unwrap();
        assert_eq!(loaded.events().len(), 1);
        assert!(
            state
                .menu()
                .unwrap()
                .iter()
                .any(|(_, _, label)| { label == "许愿已加密保存（1 条）" })
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wish_storage_failure_never_stops_the_live_feedback_session() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-wish-invalid-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&root, b"not a directory").unwrap();
        let feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let state = NativeFeedbackLanguageBarState::with_wish_root(
            Arc::clone(&feedback),
            Arc::new(Mutex::new(NativeFeedbackContextCache::default())),
            Rc::new(Cell::new(InputMode::Chinese)),
            Some(root.clone()),
        );
        assert!(state.perform_feedback_action(FEEDBACK_MENU_START).unwrap());
        feedback.lock().unwrap().record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "aa".to_owned(),
            },
            native_feedback_monotonic_ms(),
        );

        assert!(!state.perform_feedback_action(FEEDBACK_MENU_WISH).unwrap());
        assert!(matches!(
            state.wish_save_status.get(),
            WishSaveStatus::Failed
        ));
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        fs::remove_file(root).unwrap();
    }

    #[test]
    fn native_feedback_popup_accepts_every_redacted_lifecycle_menu() {
        for summary in [
            NativeFeedbackSummary::default(),
            NativeFeedbackSummary {
                lifecycle: NativeFeedbackLifecycle::Recording,
                enabled: true,
                accepting: true,
                complete: true,
                events: 17,
                half_pair_gap_samples: 2,
                half_pair_gap_histogram: [0, 1, 0, 1, 0, 0, 0, 0, 0],
                ..NativeFeedbackSummary::default()
            },
            NativeFeedbackSummary {
                lifecycle: NativeFeedbackLifecycle::Stopped,
                enabled: true,
                complete: true,
                events: 17,
                half_pair_gap_samples: 2,
                half_pair_gap_histogram: [0, 1, 0, 1, 0, 0, 0, 0, 0],
                ..NativeFeedbackSummary::default()
            },
        ] {
            let menu = NativeFeedbackPopupMenu::create().expect("native popup menu");
            for (id, flags, label) in
                feedback_language_bar_menu(summary, WishSaveStatus::Never, true)
            {
                menu.append(id, flags, &label)
                    .expect("redacted lifecycle menu item");
            }
        }
    }

    #[test]
    fn language_bar_icons_are_theme_recolorable_and_mark_recording() {
        let disabled = NativeFeedbackSummary::default();
        let (disabled_and, disabled_xor) =
            feedback_language_bar_icon_masks(InputMode::Chinese, disabled);
        assert!(disabled_xor.iter().all(|byte| *byte == 0));
        let top_transparent = !LANGUAGE_BAR_CHINESE_ICON_ROWS[0];
        assert_eq!(disabled_and[0], (top_transparent >> 8) as u8);
        assert_eq!(disabled_and[1], top_transparent as u8);

        let recording = NativeFeedbackSummary {
            lifecycle: NativeFeedbackLifecycle::Recording,
            ..disabled
        };
        let (recording_and, recording_xor) =
            feedback_language_bar_icon_masks(InputMode::Chinese, recording);
        assert_eq!(recording_xor, disabled_xor);
        assert_ne!(recording_and, disabled_and);

        let chinese = feedback_language_bar_icon_rows(InputMode::Chinese, disabled);
        let english = feedback_language_bar_icon_rows(InputMode::English, disabled);
        assert_ne!(chinese, english);
        assert_eq!(chinese[13] & LANGUAGE_BAR_RECORDING_DOT, 0);
        assert_eq!(
            feedback_language_bar_icon_rows(InputMode::Chinese, recording)[13]
                & LANGUAGE_BAR_RECORDING_DOT,
            LANGUAGE_BAR_RECORDING_DOT
        );
    }

    #[test]
    fn language_bar_item_explicitly_starts_stops_and_clears_memory_feedback() {
        let _guard = test_state_lock();
        let before = ACTIVE_COM_OBJECTS.load(Ordering::Acquire);
        let feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let context = Arc::new(Mutex::new(NativeFeedbackContextCache::default()));
        let mode = Rc::new(Cell::new(InputMode::Chinese));
        let state = Rc::new(NativeFeedbackLanguageBarState::new(
            Arc::clone(&feedback),
            context,
            Rc::clone(&mode),
        ));
        let object = ComObject::new(NativeFeedbackLanguageBarItem::counted(Rc::clone(&state)));
        let button: ITfLangBarItemButton = object.to_interface();
        let item: ITfLangBarItem = button.cast().unwrap();
        let source: ITfSource = object.to_interface();

        let mut info = TF_LANGBARITEMINFO::default();
        unsafe { item.GetInfo(&mut info) }.unwrap();
        assert_eq!(info.clsidService, TSF_ALPHA_CLSID);
        assert_eq!(info.guidItem, GUID_LBI_INPUTMODE);
        assert_ne!(info.dwStyle & TF_LBI_STYLE_BTN_BUTTON, 0);
        assert_ne!(info.dwStyle & TF_LBI_STYLE_BTN_MENU, 0);
        assert_ne!(info.dwStyle & TF_LBI_STYLE_SHOWNINTRAY, 0);
        assert_ne!(info.dwStyle & TF_LBI_STYLE_TEXTCOLORICON, 0);
        assert_eq!(unsafe { button.GetText() }.unwrap().to_string(), "中");
        let point = POINT::default();
        unsafe { button.OnClick(TfLBIClick(99), point, ptr::null()) }.unwrap();
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Disabled,
            "unsupported clicks must not start feedback or open a popup"
        );
        let icon = unsafe { button.GetIcon() }.unwrap();
        assert!(!icon.is_invalid());
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::DestroyIcon(icon).unwrap();
        }
        assert!(
            unsafe { item.GetTooltipString() }
                .unwrap()
                .to_string()
                .contains("反馈未开始")
        );

        let updates = Arc::new(AtomicUsize::new(0));
        let flags = Arc::new(AtomicUsize::new(0));
        let sink: ITfLangBarItemSink = TestLanguageBarSink {
            updates: Arc::clone(&updates),
            flags: Arc::clone(&flags),
        }
        .into();
        let cookie = unsafe { source.AdviseSink(&ITfLangBarItemSink::IID, &sink) }.unwrap();
        assert_eq!(cookie, LANGUAGE_BAR_SINK_COOKIE);
        assert!(
            unsafe { source.AdviseSink(&ITfLangBarItemSink::IID, &sink) }.is_err(),
            "only one language-bar sink is accepted"
        );

        unsafe { button.OnMenuSelect(FEEDBACK_MENU_START) }.unwrap();
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        assert_eq!(unsafe { button.GetText() }.unwrap().to_string(), "中 ●");
        assert!(updates.load(Ordering::Acquire) >= 1);
        assert_ne!(flags.load(Ordering::Acquire) & TF_LBI_ICON as usize, 0);
        assert_ne!(flags.load(Ordering::Acquire) & TF_LBI_TEXT as usize, 0);

        mode.set(InputMode::English);
        state.notify();
        assert_eq!(unsafe { button.GetText() }.unwrap().to_string(), "英 ●");

        unsafe { button.OnMenuSelect(FEEDBACK_MENU_STOP) }.unwrap();
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Stopped
        );
        assert_eq!(unsafe { button.GetText() }.unwrap().to_string(), "英");
        unsafe { button.OnMenuSelect(FEEDBACK_MENU_START) }.unwrap();
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Stopped,
            "start must not overwrite a retained stopped session"
        );

        unsafe { button.OnMenuSelect(FEEDBACK_MENU_CLEAR) }.unwrap();
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Disabled
        );
        assert!(unsafe { button.OnMenuSelect(FEEDBACK_MENU_STATUS) }.is_err());

        unsafe { item.Show(false) }.unwrap();
        assert_ne!(
            unsafe { item.GetStatus() }.unwrap() & TF_LBI_STATUS_HIDDEN,
            0
        );
        unsafe { source.UnadviseSink(cookie) }.unwrap();
        assert!(unsafe { source.UnadviseSink(cookie) }.is_err());

        drop(sink);
        drop(source);
        drop(button);
        drop(item);
        drop(object);
        drop(state);
        drop(mode);
        drop(feedback);
        assert_eq!(ACTIVE_COM_OBJECTS.load(Ordering::Acquire), before);
    }

    #[test]
    fn language_bar_controller_adds_and_removes_without_starting_feedback() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let before = ACTIVE_COM_OBJECTS.load(Ordering::Acquire);
        let feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let context = Arc::new(Mutex::new(NativeFeedbackContextCache::default()));
        let mode = Rc::new(Cell::new(InputMode::Chinese));
        let state = Rc::new(NativeFeedbackLanguageBarState::new(
            Arc::clone(&feedback),
            context,
            mode,
        ));
        // A live installed Alpha can already own the production language-bar
        // identity while this unit test runs. Use a process-local test identity
        // so the lifecycle assertion does not compete with the user's IME.
        let mut controller = NativeFeedbackLanguageBarController::new_with_guid(
            true,
            Rc::clone(&state),
            GUID::from_u128(0x30f8d9c8_f67c_42dc_911e_3b354b0fcd60),
        );

        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        unsafe { thread_manager.Activate() }.expect("thread manager activation");
        controller
            .activate(&thread_manager)
            .expect("language-bar item activation");
        assert!(controller.manager.is_some());
        assert!(controller.item.is_some());
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Disabled
        );

        controller
            .deactivate()
            .expect("language-bar item deactivation");
        assert!(controller.manager.is_none());
        assert!(controller.item.is_none());
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(thread_manager);
        drop(controller);
        drop(state);
        drop(feedback);
        assert_eq!(ACTIVE_COM_OBJECTS.load(Ordering::Acquire), before);
    }

    #[test]
    fn wish_command_controller_applies_only_new_test_compartment_words() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let feedback = Arc::new(Mutex::new(NativeFeedbackSession::default()));
        let state = Rc::new(NativeFeedbackLanguageBarState::new(
            Arc::clone(&feedback),
            Arc::new(Mutex::new(NativeFeedbackContextCache::default())),
            Rc::new(Cell::new(InputMode::Chinese)),
        ));
        let command_guid = GUID::from_u128(0xdb053ed9_3187_48ec_8e24_fa971f7a9bd7);
        let acknowledgement_guid = GUID::from_u128(0xb679b7f3_27f5_412a_baa7_a53f6627f56b);
        let mut controller = NativeWishCommandController::new_with_guids(
            true,
            Rc::downgrade(&state),
            command_guid,
            acknowledgement_guid,
        );
        let thread_manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
        }
        .expect("TSF thread manager should be available");
        let client_id = unsafe { thread_manager.Activate() }.expect("thread manager activation");
        controller
            .activate(&thread_manager, client_id)
            .expect("wish command subscription");
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Disabled,
            "an old baseline value must not run during activation"
        );

        let compartments = unsafe { thread_manager.GetGlobalCompartment() }.unwrap();
        let command_compartment = unsafe { compartments.GetCompartment(&command_guid) }.unwrap();
        let previous = read_wish_command(&command_compartment);
        let word = crate::wish_command::WishCommandWord::next(previous, WishCommand::Start);
        let value = VARIANT::from(i32::try_from(word.raw()).unwrap());
        unsafe { command_compartment.SetValue(client_id, &value) }.unwrap();

        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        let acknowledgement_compartment =
            unsafe { compartments.GetCompartment(&acknowledgement_guid) }.unwrap();
        assert_eq!(
            read_wish_acknowledgement(&acknowledgement_compartment),
            WishCommandAck::new(word.sequence(), WishCommandAckStatus::Applied)
        );

        controller
            .deactivate()
            .expect("wish command unsubscription");
        unsafe { compartments.ClearCompartment(client_id, &command_guid) }.unwrap();
        unsafe { compartments.ClearCompartment(client_id, &acknowledgement_guid) }.unwrap();
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
    }

    #[test]
    fn host_printable_input_finishes_only_an_active_chinese_preedit() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            FixedCandidateProvider,
        ))));
        let shifted = KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        };
        assert!(!service.direct_input_needs_commit(VK_A.0, shifted).unwrap());

        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        assert!(service.direct_input_needs_commit(VK_A.0, shifted).unwrap());
        assert!(
            service
                .direct_input_needs_commit(VK_1.0 + 1, shifted)
                .unwrap(),
            "Shift+2 belongs to the host but must not enter a live preedit"
        );
        assert!(
            service
                .direct_input_needs_commit(VK_OEM_COMMA.0, shifted)
                .unwrap(),
            "an unassigned shifted OEM key must commit before host input"
        );
        assert!(
            service
                .direct_input_needs_commit(VK_6.0 + 1, KeyModifiers::default())
                .unwrap(),
            "a number outside the candidate shortcuts must commit before host input"
        );
        assert!(
            !service.direct_input_needs_commit(VK_1.0, shifted).unwrap(),
            "Shift+1 is handled as a Chinese exclamation mark"
        );
        assert!(
            !service
                .direct_input_needs_commit(VK_OEM_1.0, KeyModifiers::default())
                .unwrap(),
            "semicolon is handled by the Chinese punctuation path"
        );
        assert!(
            service
                .direct_input_needs_commit(VK_CAPITAL.0, KeyModifiers::default())
                .unwrap()
        );
        assert!(
            !service
                .direct_input_needs_commit(VK_A.0, KeyModifiers::default())
                .unwrap()
        );
        assert!(
            !service
                .direct_input_needs_commit(
                    VK_A.0,
                    KeyModifiers {
                        control: true,
                        ..KeyModifiers::default()
                    }
                )
                .unwrap(),
            "host shortcuts must remain untouched"
        );

        service.input_mode.set(InputMode::English);
        assert!(!service.direct_input_needs_commit(VK_A.0, shifted).unwrap());
    }

    #[test]
    fn candidate_display_pages_and_bounds_native_text() {
        let candidates = (1..=9)
            .map(|index| format!("候选{index}"))
            .collect::<Vec<_>>();
        let display = CandidateDisplay::from_candidates(candidates, 6);
        assert_eq!(display.visible(), ["候选7", "候选8", "候选9"]);
        assert_eq!(display.page_starts(), [0, 6]);
        assert_eq!(display.current_page(), 1);
        assert_eq!(display.selected_index(), 6);
        assert_eq!(display.native_text(), "1  候选7\n2  候选8\n3  候选9");

        let long = "甲".repeat(CANDIDATE_DISPLAY_MAX_CHARS + 1);
        let clipped = CandidateDisplay::from_candidates(vec![long], 0).native_text();
        assert!(clipped.ends_with('…'));
        assert_eq!(clipped.chars().count(), 3 + CANDIDATE_DISPLAY_MAX_CHARS + 1);
    }

    #[test]
    fn page_keys_are_ui_only_session_changes() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("nihk".to_owned()));
        let next = plan_session_input(&session, CompositionInput::NextPage, None, 9).unwrap();
        assert!(next.edit.is_none());
        assert_eq!(next.after.candidate_page_start(), 6);

        let third = plan_session_input(&next.after, CompositionInput::NextPage, None, 22).unwrap();
        assert!(third.edit.is_none());
        assert_eq!(third.after.candidate_page_start(), 12);

        let previous =
            plan_session_input(&third.after, CompositionInput::PreviousPage, None, 22).unwrap();
        assert!(previous.edit.is_none());
        assert_eq!(previous.after.candidate_page_start(), 6);
    }

    #[test]
    fn raw_enter_and_punctuation_plan_the_expected_document_edits() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("ju".to_owned()));

        let raw = plan_session_input(&session, CompositionInput::CommitRaw, None, 0).unwrap();
        assert!(matches!(
            raw.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "ju"
        ));
        assert!(raw.after.phonetic().is_empty());

        let comma = plan_session_input(
            &session,
            CompositionInput::Punctuation(CompositionPunctuation::Comma),
            Some("句".to_owned()),
            1,
        )
        .unwrap();
        assert!(matches!(
            comma.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "句，"
        ));
        assert!(comma.after.phonetic().is_empty());

        let idle = plan_session_input(
            &CompositionSession::default(),
            CompositionInput::Punctuation(CompositionPunctuation::Period),
            None,
            0,
        )
        .unwrap();
        assert!(matches!(
            idle.edit,
            Some(PendingDocumentEdit::Insert(ref text)) if text == "。"
        ));

        for (punctuation, expected) in [
            (CompositionPunctuation::Semicolon, "；"),
            (CompositionPunctuation::Colon, "："),
            (CompositionPunctuation::ExclamationMark, "！"),
            (CompositionPunctuation::LeftParenthesis, "（"),
            (CompositionPunctuation::RightParenthesis, "）"),
            (CompositionPunctuation::QuestionMark, "？"),
        ] {
            let idle = plan_session_input(
                &CompositionSession::default(),
                CompositionInput::Punctuation(punctuation),
                None,
                0,
            )
            .unwrap();
            assert!(matches!(
                idle.edit,
                Some(PendingDocumentEdit::Insert(ref text)) if text == expected
            ));
        }
    }

    #[test]
    fn project_overlays_supply_conversation_and_hardware_terms() {
        for (code, expected) in [
            ("siyn", "丝印"),
            ("udpn", "双拼"),
            ("oumu", "欧姆"),
            ("wlqr", "外圈"),
            ("wuwa", "呜哇"),
            ("yidair", "一大串"),
            ("duuuyu", "独属于"),
            ("bugfub", "不跟手"),
            ("drjuzi", "短句子"),
            ("jmru", "渐入"),
            ("jmrujmiu", "渐入渐出"),
            ("gdmn", "光敏"),
            ("gdmnxy", "光敏性"),
            ("vijcjuxy", "直角矩形"),
        ] {
            let candidates = project_overlay_decoder()
                .decode_exact_full_code(code, 7)
                .unwrap();
            assert_eq!(
                candidates.first().map(|candidate| candidate.text.as_str()),
                Some(expected),
                "project overlay must prioritize the complete code {code}"
            );
        }
    }

    #[test]
    fn stable_popup_content_update_reuses_geometry_and_region() {
        let placement = CandidatePopupPlacement {
            x: 100,
            y: 200,
            width: 480,
            height: 56,
        };
        assert!(placement.size_differs_from(None));
        assert!(!placement.size_differs_from(Some(placement)));
        assert!(
            !CandidatePopupPlacement {
                x: 120,
                ..placement
            }
            .size_differs_from(Some(placement))
        );
        assert!(
            CandidatePopupPlacement {
                width: 500,
                ..placement
            }
            .size_differs_from(Some(placement))
        );
    }

    #[test]
    fn feedback_selection_rank_tracks_the_visible_page() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            CountingCandidateProvider {
                calls: AtomicUsize::new(0),
                total: 12,
            },
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));

        let next_page = service
            .plan_key(WPARAM(usize::from(VK_OEM_PLUS.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(next_page.edit.is_none());
        assert_eq!(next_page.after.candidate_page_start(), 6);
        assert!(matches!(
            next_page.feedback_after_success.as_ref(),
            Some(NativeFeedbackEvent::CandidatesPresented {
                page_start: 6,
                candidates,
                ..
            }) if candidates.first().is_some_and(|candidate| candidate == "候选7")
        ));
        *service.composition.borrow_mut() = next_page.after;

        let selection = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 1)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            selection.feedback_after_success.as_ref(),
            Some(NativeFeedbackEvent::CandidateCommitted {
                text,
                source: NativeSelectionSource::Numeric,
                absolute_rank: 8,
                visible_rank: 2,
                ..
            }) if text == "候选8"
        ));
    }

    #[test]
    fn shift_tab_switches_to_the_explicit_recovery_view_and_escape_returns() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            RecoveryCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));

        let recovery = service
            .plan_key(
                WPARAM(usize::from(VK_TAB.0)),
                KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .unwrap();
        assert!(recovery.edit.is_none());
        assert!(recovery.after.recovery_mode());
        let display = recovery.candidate_display.as_ref().unwrap();
        assert_eq!(
            display.view(),
            InteractiveCandidateView::TranspositionRecovery
        );
        assert_eq!(display.visible(), ["换序候选"]);
        *service.composition.borrow_mut() = recovery.after;

        let primary = service
            .plan_key(WPARAM(usize::from(VK_ESCAPE.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(primary.edit.is_none());
        assert!(!primary.after.recovery_mode());
        let display = primary.candidate_display.as_ref().unwrap();
        assert_eq!(display.view(), InteractiveCandidateView::Primary);
        assert_eq!(display.visible(), ["普通候选"]);
    }

    #[test]
    fn exact_xuy_tab_opens_an_explicit_wish_prompt_without_stealing_ordinary_tab() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
                .unwrap()
                .is_none(),
            "non-trigger Tab must retain the existing shape-assistant request path"
        );

        service.composition.borrow_mut().finish_commit();
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("xuy".to_owned()));
        let prompt = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .expect("the exact trigger should open a confirmation prompt");
        assert!(prompt.edit.is_none());
        assert!(prompt.after.wish_prompt());
        assert_eq!(prompt.after.phonetic(), "xuy");
        assert_eq!(
            prompt.candidate_display.as_ref().unwrap().visible(),
            ["开始反馈"]
        );
        assert_eq!(
            prompt.candidate_display.as_ref().unwrap().action_detail(),
            Some("暂不保存")
        );
        assert!(prompt.feedback_after_success.is_none());
        assert!(prompt.action_after_success.is_none());
        *service.composition.borrow_mut() = prompt.after;

        let wrong_rank = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 1)), KeyModifiers::default())
            .unwrap()
            .expect("an unavailable action rank must stay inside the prompt");
        assert!(wrong_rank.after.wish_prompt());
        assert!(wrong_rank.edit.is_none());
        assert!(wrong_rank.action_after_success.is_none());

        let confirm = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .expect("Space should explicitly confirm the visible action");
        assert!(matches!(confirm.edit, Some(PendingDocumentEdit::Cancel)));
        assert!(confirm.after.phonetic().is_empty());
        assert!(!confirm.after.wish_prompt());
        assert_eq!(
            confirm.action_after_success,
            Some(PlannedAction::Wish(WishCommand::Start))
        );
        assert!(confirm.feedback_after_success.is_none());
    }

    #[test]
    fn recording_xuy_prompt_offers_a_bounded_snapshot_and_backspace_returns_to_text() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test_with_feedback(
            Some(Arc::new(SelectionCandidateProvider)),
            NativeFeedbackLimits::default(),
        ));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("xuy".to_owned()));
        let prompt = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            prompt.candidate_display.as_ref().unwrap().visible(),
            ["向猫猫许愿"]
        );
        assert_eq!(
            prompt.candidate_display.as_ref().unwrap().action_detail(),
            Some("近30秒")
        );
        *service.composition.borrow_mut() = prompt.after;

        let back = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should leave only the prompt, not delete xuy");
        assert!(!back.after.wish_prompt());
        assert_eq!(back.after.phonetic(), "xuy");
        assert!(back.edit.is_none());
        assert!(back.action_after_success.is_none());
    }

    #[test]
    fn shift_chord_state_survives_test_and_key_down_callbacks() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            RecoveryCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        service.shift_tap_armed.set(true);

        let tested_modifiers = service.observe_nonshift_test_modifiers(KeyModifiers::default());
        assert!(tested_modifiers.shift);
        assert!(!service.shift_tap_armed.get());
        assert!(service.shift_chord_pending.get());
        let tested = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), tested_modifiers)
            .unwrap()
            .unwrap();
        assert!(tested.after.recovery_mode());

        let delivered_modifiers =
            service.observe_nonshift_key_down_modifiers(KeyModifiers::default());
        assert!(delivered_modifiers.shift);
        assert!(!service.shift_chord_pending.get());
        let delivered = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), delivered_modifiers)
            .unwrap()
            .unwrap();
        assert!(delivered.after.recovery_mode());
        assert_eq!(delivered.candidate_display.unwrap().visible(), ["换序候选"]);
    }

    #[test]
    fn only_explicit_primary_selection_enters_service_session_memory() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));

        let selected = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 1)), KeyModifiers::default())
            .unwrap()
            .unwrap()
            .selection_to_remember
            .expect("an explicit ordinary selection should be remembered");
        assert_eq!(selected.code, "ab");
        assert_eq!(selected.text, "乙");
        service
            .selection_memory
            .borrow_mut()
            .remember_text(&selected.code, &selected.text);

        let promoted = service
            .load_candidate_batch(
                &SelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(promoted.candidates, ["乙", "甲", "丙"]);

        let confirmation = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(confirmation.selection_to_remember.is_none());
        assert!(matches!(
            confirmation.feedback_after_success,
            Some(NativeFeedbackEvent::CandidateCommitted {
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
                ..
            })
        ));
        let enter_confirmation = service
            .plan_key(WPARAM(usize::from(VK_RETURN.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            enter_confirmation.feedback_after_success,
            Some(NativeFeedbackEvent::RawCodeCommitted { ref code }) if code == "ab"
        ));
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_6.0 + 1)), KeyModifiers::default())
                .unwrap()
                .is_none(),
            "an unavailable rank must not create a commit or feedback event"
        );

        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::EnterRecovery);
        let recovery_selection = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(recovery_selection.selection_to_remember.is_none());
        let recovery = service
            .load_candidate_batch(
                &SelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::TranspositionRecovery,
            )
            .unwrap();
        assert_eq!(recovery.candidates, ["换序甲", "换序乙", "换序丙"]);
    }

    #[test]
    fn candidate_pages_expand_lazily_and_reuse_the_deepest_cached_frontier() {
        let provider = CountingCandidateProvider {
            calls: AtomicUsize::new(0),
            total: 30,
        };
        let mut cache = CandidateCache::default();

        let first = cache.load(
            &provider,
            "mkmvfhk",
            candidate_visible_limit(0),
            InteractiveCandidateView::Primary,
        );
        assert_eq!(first.candidates.len(), 6);
        assert!(first.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);

        let repeated = cache.load(
            &provider,
            "mkmvfhk",
            candidate_visible_limit(0),
            InteractiveCandidateView::Primary,
        );
        assert_eq!(repeated.candidates.len(), 6);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);

        let second_page = cache.load(
            &provider,
            "mkmvfhk",
            candidate_next_page_limit(0),
            InteractiveCandidateView::Primary,
        );
        assert_eq!(second_page.candidates.len(), 12);
        assert!(second_page.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 2);

        let third_page = cache.load(
            &provider,
            "mkmvfhk",
            candidate_next_page_limit(6),
            InteractiveCandidateView::Primary,
        );
        assert_eq!(third_page.candidates.len(), 18);
        assert!(third_page.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 3);

        let previous_page = cache.load(
            &provider,
            "mkmvfhk",
            candidate_visible_limit(0),
            InteractiveCandidateView::Primary,
        );
        assert_eq!(previous_page.candidates.len(), 18);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 3);

        let bounded_end = cache.load(
            &provider,
            "mkmvfhk",
            CANDIDATE_LIMIT,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(bounded_end.candidates.len(), 30);
        assert!(!bounded_end.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 4);
        let exhausted = cache.load(
            &provider,
            "mkmvfhk",
            CANDIDATE_LIMIT,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(exhausted.candidates.len(), 30);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 4);

        let recovery = cache.load(
            &provider,
            "mkmvfhk",
            candidate_visible_limit(0),
            InteractiveCandidateView::TranspositionRecovery,
        );
        assert_eq!(
            recovery.view,
            InteractiveCandidateView::TranspositionRecovery
        );
        assert_eq!(provider.calls.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn candidate_request_depth_is_visible_first_and_expands_on_navigation() {
        assert_eq!(candidate_visible_limit(0), 6);
        assert_eq!(candidate_visible_limit(6), 12);
        assert_eq!(candidate_visible_limit(42), 48);
        assert_eq!(candidate_visible_limit(usize::MAX), CANDIDATE_LIMIT);
        assert_eq!(candidate_next_page_limit(0), 12);
        assert_eq!(candidate_next_page_limit(6), 18);
        assert_eq!(candidate_next_page_limit(42), CANDIDATE_LIMIT);
        assert_eq!(candidate_next_page_limit(usize::MAX), CANDIDATE_LIMIT);
    }

    #[test]
    fn candidate_ui_element_exposes_the_same_bounded_page_without_a_window() {
        let _guard = test_state_lock();
        let before = ACTIVE_COM_OBJECTS.load(Ordering::Acquire);
        let state = Rc::new(RefCell::new(CandidateElementState {
            display: Some(CandidateDisplay::from_candidates(
                (1..=9)
                    .map(|index| format!("候选{index}"))
                    .collect::<Vec<_>>(),
                6,
            )),
            document_manager: None,
            shown: true,
        }));
        let popup = Rc::new(RefCell::new(CandidatePopup::default()));
        let element: ITfCandidateListUIElement =
            CandidateListElement::counted(state, Rc::downgrade(&popup)).into();
        assert_eq!(unsafe { element.GetCount() }.unwrap(), 9);
        assert_eq!(unsafe { element.GetSelection() }.unwrap(), 6);
        assert_eq!(
            unsafe { element.GetString(6) }.unwrap().to_string(),
            "候选7"
        );
        assert!(unsafe { element.GetString(9) }.is_err());
        let mut starts = [u32::MAX; 2];
        let mut page_count = 0;
        unsafe { element.GetPageIndex(&mut starts, &mut page_count) }.unwrap();
        assert_eq!(starts, [0, 6]);
        assert_eq!(page_count, 2);
        assert_eq!(unsafe { element.GetCurrentPage() }.unwrap(), 1);
        unsafe { element.SetPageIndex(&starts) }.unwrap();
        unsafe { element.Show(false) }.unwrap();
        assert!(!unsafe { element.IsShown() }.unwrap().as_bool());
        drop(element);
        assert_eq!(ACTIVE_COM_OBJECTS.load(Ordering::Acquire), before);
    }

    #[test]
    fn clipped_layout_does_not_hide_a_native_candidate_popup() {
        assert!(candidate_popup_should_show(true, true, true));
        assert!(!candidate_popup_should_show(false, true, true));
        assert!(!candidate_popup_should_show(true, false, true));
    }

    #[test]
    fn candidate_popup_prefers_a_compact_horizontal_row() {
        let display = CandidateDisplay::from_candidates(
            ["亲", "秦", "琴", "去年", "勤", "青年", "芹"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            0,
        );
        let metrics = candidate_popup_metrics(&display, 96, 1920);
        assert_eq!(metrics.layout, CandidatePopupLayout::Horizontal);
        assert_eq!(metrics.height, 46);
        assert!(metrics.width >= 280);
        assert!(metrics.width <= POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL);
    }

    #[test]
    fn inline_action_uses_a_compact_dedicated_row_without_ellipsis_pressure() {
        let display = CandidateDisplay::action("向猫猫许愿", "近30秒");
        let metrics = candidate_popup_metrics(&display, 96, 1920);
        assert_eq!(metrics.layout, CandidatePopupLayout::Horizontal);
        assert_eq!(metrics.height, 46);
        assert_eq!(metrics.width, POPUP_ACTION_MIN_WIDTH_LOGICAL);

        let widths = horizontal_candidate_widths(&display, 96, metrics.width);
        assert_eq!(widths, [metrics.width - POPUP_OUTER_PADDING_LOGICAL * 2]);
        assert_eq!(display.visible(), ["向猫猫许愿"]);
        assert_eq!(display.action_detail(), Some("近30秒"));
    }

    #[test]
    fn candidate_popup_border_tracks_window_rounding_at_common_dpis() {
        let client = RECT {
            left: 0,
            top: 0,
            right: 480,
            bottom: 46,
        };
        for (dpi, expected_border, expected_corner) in [(96, 1, 16), (144, 2, 24), (192, 2, 32)] {
            let geometry = candidate_popup_border_geometry(client, dpi).unwrap();
            assert_eq!(geometry.outer.left, 0);
            assert_eq!(geometry.outer.top, 0);
            assert_eq!(geometry.outer.right, 480);
            assert_eq!(geometry.outer.bottom, 46);
            assert_eq!(geometry.inner.left, expected_border);
            assert_eq!(geometry.inner.top, expected_border);
            assert_eq!(geometry.inner.right, 480 - expected_border);
            assert_eq!(geometry.inner.bottom, 46 - expected_border);
            assert_eq!(geometry.outer_corner_diameter, expected_corner);
            assert_eq!(
                geometry.inner_corner_diameter,
                expected_corner - expected_border * 2
            );
        }
    }

    #[test]
    fn system_composited_corners_never_stack_a_binary_window_region() {
        assert!(!CandidatePopupCornerStrategy::SystemDwm.uses_custom_region());
        assert!(CandidatePopupCornerStrategy::RegionFallback.uses_custom_region());
    }

    #[test]
    fn six_common_length_candidates_fit_while_the_selected_text_stays_complete() {
        let display = CandidateDisplay::from_candidates(
            [
                "第一项",
                "第二项",
                "第三候选",
                "第四候选",
                "第五候选",
                "第六候选",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            0,
        );
        assert_eq!(display.visible().len(), CANDIDATE_PAGE_SIZE);
        let metrics = candidate_popup_metrics(&display, 96, 1920);
        assert_eq!(metrics.layout, CandidatePopupLayout::Horizontal);
        assert_eq!(metrics.width, POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL);

        let widths = horizontal_candidate_widths(&display, 96, metrics.width);
        for (index, (candidate, width)) in display.visible().iter().zip(widths).enumerate() {
            let left_inset = if index == 0 {
                POPUP_SELECTED_TEXT_INSET_LOGICAL
            } else {
                POPUP_TEXT_PADDING_LOGICAL
            };
            let text_width = width
                - left_inset
                - POPUP_RANK_WIDTH_LOGICAL
                - POPUP_RANK_GAP_LOGICAL
                - POPUP_TEXT_PADDING_LOGICAL;
            assert!(text_width >= i32::try_from(candidate.chars().count()).unwrap() * 18);
        }
    }

    #[test]
    fn candidate_popup_palette_has_a_content_first_attention_hierarchy() {
        let selected_text = popup_attention_lightness(POPUP_SELECTED_TEXT_RGB);
        let candidate_text = popup_attention_lightness(POPUP_CANDIDATE_TEXT_RGB);
        let selected_rank = popup_attention_lightness(POPUP_SELECTED_RANK_RGB);
        let rank = popup_attention_lightness(POPUP_RANK_RGB);
        let page = popup_attention_lightness(POPUP_PAGE_RGB);
        let selected_background = popup_attention_lightness(POPUP_SELECTED_BACKGROUND_RGB);
        let background = popup_attention_lightness(POPUP_BACKGROUND_RGB);

        assert!(selected_text > candidate_text);
        assert!(candidate_text > selected_rank);
        assert!(selected_rank > rank);
        assert!(rank > page);
        assert!(page > selected_background);
        assert!(selected_background > background);
    }

    #[test]
    fn candidate_popup_text_meets_normal_text_contrast() {
        for (foreground, background) in [
            (POPUP_SELECTED_TEXT_RGB, POPUP_SELECTED_BACKGROUND_RGB),
            (POPUP_CANDIDATE_TEXT_RGB, POPUP_BACKGROUND_RGB),
            (POPUP_SELECTED_RANK_RGB, POPUP_SELECTED_BACKGROUND_RGB),
            (POPUP_RANK_RGB, POPUP_BACKGROUND_RGB),
            (POPUP_PAGE_RGB, POPUP_BACKGROUND_RGB),
        ] {
            assert!(popup_contrast_ratio(foreground, background) >= 4.5);
        }
    }

    #[test]
    fn candidate_label_metadata_shares_a_stable_visual_baseline() {
        let content = RECT {
            left: 10,
            top: 8,
            right: 110,
            bottom: 44,
        };
        let (rank, text) = candidate_label_rects(content, 96);

        assert_eq!(rank.left, 10);
        assert_eq!(rank.right, 26);
        assert_eq!(rank.top, 10);
        assert_eq!(rank.bottom, 46);
        assert_eq!(text.left, 30);
        assert_eq!(text.top, 8);
        assert_eq!(text.bottom, 44);
    }

    #[test]
    fn candidate_selection_surface_keeps_breathing_room() {
        let item = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: POPUP_ROW_HEIGHT_LOGICAL,
        };
        let (selected, accent) = candidate_selection_rects(item, 96);

        assert_eq!(selected.left, 1);
        assert_eq!(selected.top, 4);
        assert_eq!(selected.right, 95);
        assert_eq!(selected.bottom, 32);
        assert_eq!(accent.left, 6);
        assert_eq!(accent.top, 11);
        assert_eq!(accent.right, 9);
        assert_eq!(accent.bottom, 25);
    }

    #[test]
    fn candidate_popup_uses_one_ellipsis_and_never_elides_when_measurement_fails() {
        let width = |text: &str| Some(i32::try_from(text.chars().count()).unwrap() * 10);
        assert_eq!(candidate_text_for_width("省略号", 30, width), "省略号");
        assert_eq!(candidate_text_for_width("省略号", 20, width), "省…");
        assert_eq!(candidate_text_for_width("省略号", 10, width), "…");
        assert_eq!(candidate_text_for_width("省略号", 20, |_| None), "省略号");
    }

    #[test]
    fn recovery_mode_and_page_number_have_separate_footer_space() {
        let candidates = (0..10).map(|index| format!("候选{index}")).collect();
        let ordinary = CandidateDisplay::from_candidates(candidates, 0);
        let recovery = CandidateDisplay::from_batch(
            CandidateBatch {
                candidates: ordinary.candidates.clone(),
                may_have_more: true,
                view: InteractiveCandidateView::TranspositionRecovery,
            },
            0,
        );
        let recovery_one_page = CandidateDisplay::from_batch(
            CandidateBatch {
                candidates: vec!["换序候选".to_owned()],
                may_have_more: false,
                view: InteractiveCandidateView::TranspositionRecovery,
            },
            0,
        );

        assert_eq!(candidate_popup_footer_logical_width(&ordinary), 62);
        assert_eq!(candidate_popup_footer_logical_width(&recovery), 108);
        assert_eq!(candidate_popup_footer_logical_width(&recovery_one_page), 60);
    }

    #[test]
    fn candidate_popup_falls_back_to_vertical_when_horizontal_space_is_tight() {
        let ordinary = CandidateDisplay::from_candidates(
            ["亲", "秦", "琴", "去年", "勤"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            0,
        );
        let narrow = candidate_popup_metrics(&ordinary, 96, 300);
        assert_eq!(narrow.layout, CandidatePopupLayout::Vertical);
        assert_eq!(narrow.width, 300);

        let long = CandidateDisplay::from_candidates(
            (0..5)
                .map(|_| "甲".repeat(CANDIDATE_DISPLAY_MAX_CHARS))
                .collect(),
            0,
        );
        let wide_screen = candidate_popup_metrics(&long, 96, 1920);
        assert_eq!(wide_screen.layout, CandidatePopupLayout::Horizontal);
        assert_eq!(wide_screen.width, POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL);
        let widths = horizontal_candidate_widths(&long, 96, wide_screen.width);
        assert!(
            widths.iter().copied().sum::<i32>()
                <= wide_screen.width - POPUP_OUTER_PADDING_LOGICAL * 2
        );
        assert_eq!(
            widths[0],
            horizontal_candidate_logical_width(&long.visible()[0])
        );
        assert!(
            widths[1..]
                .iter()
                .all(|width| *width >= POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL)
        );
        assert!(
            widths
                .iter()
                .any(|width| *width < horizontal_candidate_logical_width(&long.visible()[0]))
        );
    }
}
