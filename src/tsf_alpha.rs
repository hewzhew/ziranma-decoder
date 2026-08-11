//! Build-only Windows TSF COM and composition probe.
//!
//! This module intentionally exports no registration functions. It proves
//! class-factory, activation, deactivation, server-lock, and unload behavior
//! without adding an input profile to Windows.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::error::Error as StdError;
use std::ffi::{OsString, c_void};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, Weak as SyncWeak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::candidate_snapshot::{
    FourCharacterCorrectionDecision, InteractiveCandidateQuery, InteractiveCandidateSource,
    MAX_TAB_SHAPE_SOURCE_RANK, layered_candidate_query_with_consensus_sources,
    layered_candidate_query_with_sources, layered_four_character_correction_decision,
};
use crate::composition::{MAX_TAB_ASSEMBLY_CHARACTERS, TabAssemblySelection, TabAssemblyStage};
use crate::personal_ranking::CandidateTextPromotion;
use crate::{
    CANDIDATE_RUNTIME_DIRECTORY, CandidatePackageError, CandidatePackageManifest,
    CandidateRuntimeError, CandidateRuntimeSupplemental, CandidateRuntimeSupplementalSelection,
    CandidateSnapshot, CharacterShapeIndex, CompositionEffect, CompositionInput,
    CompositionPunctuation, CompositionSession,
    DEFAULT_NATIVE_FEEDBACK_WISH_EPISODE_MAX_LOOKBACK_MS, DEFAULT_NATIVE_FEEDBACK_WISH_EPISODES,
    DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS, DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS, Decoder,
    ExactShortPageSession, ExactShortWordCatalog, ExactShortWordCatalogError,
    ExplicitAliasSnapshot, FrozenNativeFeedbackSnapshot, LoadedPersonalRanking,
    LoadedPersonalRankingSuppressions, MAX_CANDIDATE_SNAPSHOT_RANK,
    NATIVE_FEEDBACK_HALF_PAIR_GAP_BUCKET_UPPER_BOUNDS_MS, NativeAutomaticTranspositionDecision,
    NativeAutomaticTranspositionOutcome, NativeAutomaticTranspositionTier,
    NativeCancellationSource, NativeCandidatePersonalization, NativeCandidateProvenance,
    NativeCandidateSource, NativeCandidateSuppressionAction, NativeCandidateView,
    NativeFeedbackAuthorization, NativeFeedbackClearResult, NativeFeedbackContext,
    NativeFeedbackEvent, NativeFeedbackFreezeAuthorization, NativeFeedbackFreezeError,
    NativeFeedbackLifecycle, NativeFeedbackLimits, NativeFeedbackRecordResult,
    NativeFeedbackSession, NativeFeedbackStartResult, NativeFeedbackStopResult,
    NativeFeedbackSummary, NativePersonalPhraseAdjacency, NativeSelectionSource,
    NativeTabAssemblyState, PERSONAL_CONTEXT_SEARCH_DEPTH, PERSONAL_RANKING_SUPPRESSION_DIRECTORY,
    PersonalContextRanking, PersonalRankingBatch, PersonalRankingSelection,
    PersonalRankingSnapshot, PersonalRankingSuppressionAction,
    PersonalRankingSuppressionActionKind, PersonalRankingSuppressionSnapshot,
    RESEARCH_FEEDBACK_DIRECTORY, SessionSelectionMemory, WISH_ACK_COMPARTMENT_GUID,
    WISH_COMMAND_COMPARTMENT_GUID, WindowsUserDataProtector, WishCaptureScope, WishCategory,
    WishCommand, WishCommandAck, WishCommandAckStatus, WishJournalAnchor, WishJournalContext,
    WishJournalSpan, WishPublicCandidateOrderPolicy, WishRuntimeIdentity, WishSnapshot,
    candidate_sha256_hex, load_candidate_runtime_snapshots, load_candidate_runtime_supplemental,
    load_candidate_runtime_supplemental_selection, load_current_explicit_alias_snapshot,
    load_explicit_alias_slot_state, load_personal_ranking, load_personal_ranking_suppressions,
    parse_lexicon_tsv, parse_stroke_sequence_tsv, refresh_personal_ranking,
    refresh_personal_ranking_suppressions, research_feedback_enabled, save_personal_ranking_batch,
    save_personal_ranking_checkpoint, save_personal_ranking_suppression_action, save_wish_snapshot,
};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, COLORREF, E_INVALIDARG, E_POINTER,
    E_UNEXPECTED, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, S_FALSE, S_OK, SIZE,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_END_ELLIPSIS, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_TOP, DT_VCENTER,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, FillRect,
    FillRgn, GetMonitorInfoW, GetTextExtentPoint32W, GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, RGN_DIFF, RGN_ERROR, SRCCOPY, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    TEXTMETRICW, TRANSPARENT,
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
    GetKeyState, VK_0, VK_1, VK_6, VK_9, VK_A, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE,
    VK_ESCAPE, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5,
    VK_OEM_6, VK_OEM_7, VK_OEM_8, VK_OEM_102, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD,
    VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_Z,
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
    TF_DEFAULT_SELECTION, TF_ES_ASYNC, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
    TF_GRAVITY_BACKWARD, TF_IAS_NO_DEFAULT_COMPOSITION, TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT,
    TF_LBI_CLK_RIGHT, TF_LBI_ICON, TF_LBI_STATUS, TF_LBI_STATUS_DISABLED, TF_LBI_STATUS_HIDDEN,
    TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_BTN_MENU, TF_LBI_STYLE_SHOWNINTRAY,
    TF_LBI_STYLE_TEXTCOLORICON, TF_LBI_TEXT, TF_LBI_TOOLTIP, TF_LBMENUF_CHECKED, TF_LBMENUF_GRAYED,
    TF_LBMENUF_SEPARATOR, TF_POPF_ALL, TF_SELECTION, TF_SELECTIONSTYLE, TF_TF_MOVESTART,
    TfLBIClick,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, CreateIcon, CreatePopupMenu, CreateWindowExW, DI_NORMAL,
    DefWindowProcW, DestroyMenu, DestroyWindow, DrawIconEx, GWLP_USERDATA, GWLP_WNDPROC,
    GetClientRect, GetForegroundWindow, GetWindowLongPtrW, HICON, HMENU, HWND_TOPMOST, IMAGE_ICON,
    KillTimer, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, SET_WINDOW_POS_FLAGS, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenuEx, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ERASEBKGND, WM_NCDESTROY,
    WM_PAINT, WM_TIMER, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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
const TSF_PUBLIC_STROKE_SEQUENCES: &str =
    include_str!("../data/public/conway-stroke-data/sequence-characters.txt");
const CANDIDATE_PAGE_SIZE: usize = 6;
const CANDIDATE_LIMIT: usize = MAX_CANDIDATE_SNAPSHOT_RANK;
const SHAPE_CANDIDATE_POOL_CACHE_CAPACITY: usize = MAX_TAB_ASSEMBLY_CHARACTERS;
const EXACT_FULL_CODE_CANDIDATE_CACHE_CAPACITY: usize = 128;
const CANDIDATE_DISPLAY_MAX_CHARS: usize = 32;
const TSF_PUBLIC_CANDIDATE_ORDER_POLICY: WishPublicCandidateOrderPolicy =
    WishPublicCandidateOrderPolicy::ConservativeCoreFirst;
const AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS: u64 = 48;
const AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS: u64 = 64;
const AUTOMATIC_TRANSPOSITION_SHADOW_UPPER_GAP_MS: u64 = 96;
const PERSONAL_RANKING_FLUSH_SELECTIONS: usize = 8;
const BACKGROUND_PERSISTENCE_QUEUE_CAPACITY: usize = 16;
const CANDIDATE_RUNTIME_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const SLOW_KEY_PATH_THRESHOLD_MS: u32 = 16;
const INLINE_WISH_TRIGGER_CODE: &str = "xuy";
const INLINE_WISH_NOTICE_TIMER_ID: usize = 1;
const INLINE_WISH_NOTICE_DURATION_MS: u32 = 1_200;
const INLINE_WISH_NOTICE_ICON_RESOURCE_ID: usize = 103;
const CANDIDATE_UI_GUID: GUID = GUID::from_u128(0xb9fdad61_3f19_4d6c_86f7_72e9d3064f84);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InteractiveCandidateView {
    #[default]
    Primary,
    TranspositionRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutomaticTranspositionTier {
    Shadow,
    Secondary,
    Primary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AutomaticTranspositionPattern {
    first_syllable_index: usize,
    syllable_count: usize,
}

impl AutomaticTranspositionPattern {
    const fn single(syllable_index: usize) -> Self {
        Self {
            first_syllable_index: syllable_index,
            syllable_count: 1,
        }
    }

    const fn adjacent_pair(first_syllable_index: usize) -> Self {
        Self {
            first_syllable_index,
            syllable_count: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AutomaticTranspositionAttempt {
    pattern: AutomaticTranspositionPattern,
    cold_tier: AutomaticTranspositionTier,
    tier: AutomaticTranspositionTier,
    pair_gap_ms: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AutomaticTranspositionRequest {
    primary: AutomaticTranspositionAttempt,
    fallback: Option<AutomaticTranspositionAttempt>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletedPairTiming {
    syllable_index: usize,
    pair_gap_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AutomaticTranspositionTimingEvidence {
    current_pair_gap_ms: Option<u64>,
    previous_pair: Option<CompletedPairTiming>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum AutomaticTranspositionOutcome {
    #[default]
    NotRequested,
    Suppressed(AutomaticTranspositionTier),
    NoRecovery(AutomaticTranspositionTier),
    RecoveryAvailable(AutomaticTranspositionTier),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CandidateDataIdentity {
    core_revision: String,
    supplemental_revision: Option<String>,
}

#[derive(Clone)]
struct ExactShortCandidateLayer {
    catalog: Arc<ExactShortWordCatalog>,
    exact_promotions: usize,
}

trait CandidateProvider: Send + Sync {
    /// Returns one deterministic, bounded candidate page without learning or
    /// I/O. Implementations should decode once rather than once per rank.
    fn candidates(&self, code: &str, limit: usize, view: InteractiveCandidateView) -> Vec<String>;

    /// Immutable public candidate-data revisions used by this provider.
    /// Synthetic providers and legacy tests may leave the identity unknown.
    fn candidate_data_identity(&self) -> Option<CandidateDataIdentity> {
        None
    }

    /// Returns an optional authenticated exact-short layer for lazy pages.
    ///
    /// Production providers leave this disabled until a separately reviewed
    /// runtime slot is available. The candidate cache owns all page-state and
    /// never asks this layer to affect the first visible page.
    fn exact_short_layer(&self) -> Option<ExactShortCandidateLayer> {
        None
    }

    fn candidates_with_provenance(
        &self,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
    ) -> CandidateProviderOutput {
        let candidates = self.candidates(code, limit, view);
        let protected_prefix_len = self
            .protected_candidate_prefix_len(code, view)
            .min(candidates.len());
        let source = match view {
            InteractiveCandidateView::Primary => NativeCandidateSource::Unknown,
            InteractiveCandidateView::TranspositionRecovery => {
                NativeCandidateSource::TranspositionRecovery
            }
        };
        let provenance = vec![NativeCandidateProvenance::new(source, false); candidates.len()];
        CandidateProviderOutput {
            candidates,
            provenance,
            protected_prefix_len,
            automatic_transposition_blocked: false,
        }
    }

    /// Returns exact candidates for one conservatively identified reversed
    /// double-pinyin pair. Hosts decide separately whether timing evidence is
    /// strong enough to expose this lane automatically.
    fn automatic_transposition_candidates(
        &self,
        _code: &str,
        _pattern: AutomaticTranspositionPattern,
        _limit: usize,
    ) -> Option<CandidateProviderOutput> {
        None
    }

    /// Checks a small version pointer only at a new-composition boundary.
    /// Invalid updates retain the last known-good in-memory snapshot.
    fn refresh_at_safe_boundary(&self) -> bool {
        false
    }

    /// Number of leading candidates that transient session preferences may
    /// not cross. Explicit user aliases occupy this protected prefix.
    fn protected_candidate_prefix_len(
        &self,
        _code: &str,
        _view: InteractiveCandidateView,
    ) -> usize {
        0
    }

    /// Verifies that `text` is a public exact whole-word candidate for one
    /// complete double-pinyin code. Personal code-family inheritance uses
    /// this to reject aliases, free sentence segmentation, and arbitrary
    /// even-length input observations.
    fn is_exact_full_code_candidate(&self, _code: &str, _text: &str) -> bool {
        false
    }

    /// Returns the stable single-character pool for one explicit Tab slot.
    ///
    /// A two-key slot requests an exact full-code pool. A one-key slot requests
    /// complete exact readings beginning with that trailing initial. The shape
    /// prefix is interpreted as the auditable union of stroke and component
    /// prefixes; ordinary providers may leave this unsupported.
    fn shape_candidates(
        &self,
        _code: &str,
        _stroke_prefix: &str,
        _limit: usize,
    ) -> Vec<ShapeCandidate> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShapeCandidate {
    text: String,
    resolved_code: String,
}

#[derive(Default)]
struct ShapeCandidatePoolCache {
    entries: VecDeque<(String, Arc<[ShapeCandidate]>)>,
}

impl ShapeCandidatePoolCache {
    fn get(&mut self, code: &str) -> Option<Arc<[ShapeCandidate]>> {
        let index = self.entries.iter().position(|(cached, _)| cached == code)?;
        let entry = self.entries.remove(index)?;
        let pool = Arc::clone(&entry.1);
        self.entries.push_back(entry);
        Some(pool)
    }

    fn insert(&mut self, code: &str, pool: Arc<[ShapeCandidate]>) {
        if let Some(index) = self.entries.iter().position(|(cached, _)| cached == code) {
            self.entries.remove(index);
        }
        self.entries.push_back((code.to_owned(), pool));
        while self.entries.len() > SHAPE_CANDIDATE_POOL_CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

/// Bounded memory-only memoization for public whole-word verification.
///
/// Personal text may be presented to this lookup, so the cache deliberately
/// has no `Debug` or serialization surface. The supplemental revision is part
/// of every identity; a safe-boundary data refresh can therefore never reuse
/// a decision made against a different public snapshot.
#[derive(Default)]
struct ExactFullCodeCandidateCache {
    entries: VecDeque<ExactFullCodeCandidateCacheEntry>,
}

struct ExactFullCodeCandidateCacheEntry {
    code: String,
    text: String,
    supplemental_revision: Option<String>,
    exact: bool,
}

impl ExactFullCodeCandidateCache {
    fn get(&mut self, code: &str, text: &str, supplemental_revision: Option<&str>) -> Option<bool> {
        let index = self.entries.iter().position(|entry| {
            entry.code == code
                && entry.text == text
                && entry.supplemental_revision.as_deref() == supplemental_revision
        })?;
        let entry = self.entries.remove(index)?;
        let exact = entry.exact;
        self.entries.push_back(entry);
        Some(exact)
    }

    fn insert(&mut self, code: &str, text: &str, supplemental_revision: Option<&str>, exact: bool) {
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.code == code
                && entry.text == text
                && entry.supplemental_revision.as_deref() == supplemental_revision
        }) {
            self.entries.remove(index);
        }
        self.entries.push_back(ExactFullCodeCandidateCacheEntry {
            code: code.to_owned(),
            text: text.to_owned(),
            supplemental_revision: supplemental_revision.map(str::to_owned),
            exact,
        });
        while self.entries.len() > EXACT_FULL_CODE_CANDIDATE_CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

struct CandidateProviderOutput {
    candidates: Vec<String>,
    provenance: Vec<NativeCandidateProvenance>,
    protected_prefix_len: usize,
    automatic_transposition_blocked: bool,
}

#[derive(Clone, Default)]
struct CandidateBatch {
    candidates: Vec<String>,
    resolved_shape_codes: Vec<Option<String>>,
    provenance: Vec<NativeCandidateProvenance>,
    personalized: Vec<bool>,
    protected_prefix_len: usize,
    automatic_transposition: Option<NativeAutomaticTranspositionDecision>,
    may_have_more: bool,
    view: InteractiveCandidateView,
}

fn mirror_candidate_promotion(
    batch: &mut CandidateBatch,
    promotion: CandidateTextPromotion,
    personalization: NativeCandidatePersonalization,
) {
    let final_len = batch.candidates.len();
    if !promotion.mirror_into(
        &mut batch.provenance,
        NativeCandidateProvenance::default(),
        final_len,
    ) {
        batch.provenance = vec![NativeCandidateProvenance::default(); final_len];
    }
    if !promotion.mirror_into(&mut batch.personalized, false, final_len) {
        batch.personalized = vec![false; final_len];
    }
    if !promotion.mirror_into(&mut batch.resolved_shape_codes, None, final_len) {
        batch.resolved_shape_codes = vec![None; final_len];
    }
    if !personalization.is_empty() {
        if let Some(provenance) = batch.provenance.get_mut(promotion.index) {
            provenance.add_personalization(personalization);
            if promotion.changed {
                provenance.add_ranking_personalization(personalization);
            }
        }
        if let Some(marker) = batch.personalized.get_mut(promotion.index) {
            *marker = true;
        }
    }
}

#[derive(Default)]
struct CandidateCache {
    code: String,
    view: InteractiveCandidateView,
    candidates: Vec<String>,
    provenance: Vec<NativeCandidateProvenance>,
    protected_prefix_len: usize,
    requested_limit: usize,
    exhausted: bool,
    automatic_transposition_blocked: bool,
    automatic_transposition_request: Option<AutomaticTranspositionRequest>,
    automatic_transposition_effective_attempt: Option<AutomaticTranspositionAttempt>,
    automatic_transposition_outcome: AutomaticTranspositionOutcome,
    automatic_transposition_recovered_text: Option<String>,
    automatic_transposition_visible_rank: Option<usize>,
    exact_short_page_session: ExactShortPageSession,
    exact_short_layer_state: ExactShortLayerState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ExactShortLayerState {
    #[default]
    Unseen,
    Disabled,
    Enabled,
}

impl CandidateCache {
    #[cfg(test)]
    fn load(
        &mut self,
        provider: &dyn CandidateProvider,
        code: &str,
        requested_limit: usize,
        view: InteractiveCandidateView,
    ) -> CandidateBatch {
        self.load_with_automatic_transposition(provider, code, requested_limit, view, None)
    }

    fn candidate_batch(&self, view: InteractiveCandidateView) -> CandidateBatch {
        CandidateBatch {
            candidates: self.candidates.clone(),
            resolved_shape_codes: vec![None; self.candidates.len()],
            provenance: self.provenance.clone(),
            personalized: vec![false; self.candidates.len()],
            protected_prefix_len: self.protected_prefix_len,
            automatic_transposition: self.automatic_transposition_decision(),
            may_have_more: !self.exhausted && self.requested_limit < CANDIDATE_LIMIT,
            view,
        }
    }

    fn apply_exact_short_layer(
        &mut self,
        layer: &ExactShortCandidateLayer,
        code: &str,
        requested_limit: usize,
        output: &mut CandidateProviderOutput,
    ) -> std::result::Result<bool, ExactShortWordCatalogError> {
        let primary_candidates = output.candidates.clone();
        let primary_provenance = output.provenance.clone();
        let candidates = self
            .exact_short_page_session
            .extend(
                &layer.catalog,
                &primary_candidates,
                code,
                requested_limit,
                layer.exact_promotions,
                CANDIDATE_PAGE_SIZE,
            )?
            .to_vec();
        let primary_indices = self.exact_short_page_session.primary_indices();
        let mut provenance = Vec::with_capacity(primary_indices.len());
        for primary_index in primary_indices {
            provenance.push(match primary_index {
                Some(index) => *primary_provenance
                    .get(*index)
                    .ok_or(ExactShortWordCatalogError::UnstablePrimaryPrefix)?,
                None => NativeCandidateProvenance::new(
                    NativeCandidateSource::PublicConsensusExact,
                    false,
                ),
            });
        }
        debug_assert_eq!(candidates.len(), provenance.len());
        output.candidates = candidates;
        output.provenance = provenance;
        output.protected_prefix_len = output.protected_prefix_len.min(output.candidates.len());
        Ok(!self.exact_short_page_session.may_have_more())
    }

    fn load_with_automatic_transposition(
        &mut self,
        provider: &dyn CandidateProvider,
        code: &str,
        requested_limit: usize,
        view: InteractiveCandidateView,
        requested_transposition: Option<AutomaticTranspositionRequest>,
    ) -> CandidateBatch {
        let requested_limit = requested_limit.min(CANDIDATE_LIMIT);
        let same_query = self.code == code && self.view == view;
        if !same_query || view != InteractiveCandidateView::Primary {
            self.automatic_transposition_request = None;
            self.automatic_transposition_effective_attempt = None;
            self.automatic_transposition_outcome = AutomaticTranspositionOutcome::NotRequested;
            self.automatic_transposition_recovered_text = None;
            self.automatic_transposition_visible_rank = None;
            self.exact_short_page_session.clear();
            self.exact_short_layer_state = ExactShortLayerState::Unseen;
        }
        if let Some(request) = requested_transposition
            && self.automatic_transposition_request != Some(request)
        {
            self.automatic_transposition_request = Some(request);
            self.automatic_transposition_effective_attempt = None;
            self.automatic_transposition_outcome = AutomaticTranspositionOutcome::NotRequested;
            self.automatic_transposition_recovered_text = None;
            self.automatic_transposition_visible_rank = None;
        }
        let reusable = same_query && (self.exhausted || self.requested_limit >= requested_limit);
        if !reusable {
            let mut output = provider.candidates_with_provenance(code, requested_limit, view);
            if output.provenance.len() != output.candidates.len() {
                output.provenance =
                    vec![NativeCandidateProvenance::default(); output.candidates.len()];
            }
            let mut exhausted = output.candidates.len() < requested_limit;
            let exact_short_eligible = view == InteractiveCandidateView::Primary
                && requested_limit > CANDIDATE_PAGE_SIZE
                && code.len() == 4
                && code.bytes().all(|byte| byte.is_ascii_lowercase());
            if exact_short_eligible {
                let preserve_cached_prefix = match self.exact_short_layer_state {
                    ExactShortLayerState::Unseen => match provider.exact_short_layer() {
                        Some(layer) => {
                            match self.apply_exact_short_layer(
                                &layer,
                                code,
                                requested_limit,
                                &mut output,
                            ) {
                                Ok(projected_exhausted) => {
                                    exhausted = projected_exhausted;
                                    self.exact_short_layer_state = ExactShortLayerState::Enabled;
                                }
                                Err(_) => {
                                    self.exact_short_page_session.clear();
                                    self.exact_short_layer_state = ExactShortLayerState::Disabled;
                                }
                            }
                            false
                        }
                        None => {
                            self.exact_short_layer_state = ExactShortLayerState::Disabled;
                            false
                        }
                    },
                    ExactShortLayerState::Disabled => false,
                    ExactShortLayerState::Enabled => {
                        provider.exact_short_layer().is_none_or(|layer| {
                            self.apply_exact_short_layer(&layer, code, requested_limit, &mut output)
                                .map(|projected_exhausted| exhausted = projected_exhausted)
                                .is_err()
                        })
                    }
                };
                if preserve_cached_prefix && same_query && !self.candidates.is_empty() {
                    self.exhausted = true;
                    return self.candidate_batch(view);
                }
            } else if requested_limit > CANDIDATE_PAGE_SIZE
                && self.exact_short_layer_state == ExactShortLayerState::Unseen
            {
                self.exact_short_layer_state = ExactShortLayerState::Disabled;
            }
            self.code.clear();
            self.code.push_str(code);
            self.view = view;
            self.exhausted = exhausted;
            self.requested_limit = requested_limit;
            self.candidates = output.candidates;
            self.provenance = output.provenance;
            self.protected_prefix_len = output.protected_prefix_len.min(self.candidates.len());
            self.automatic_transposition_blocked = output.automatic_transposition_blocked;
            self.automatic_transposition_effective_attempt = None;
            self.automatic_transposition_outcome = AutomaticTranspositionOutcome::NotRequested;
            self.automatic_transposition_recovered_text = None;
            self.automatic_transposition_visible_rank = None;
        }
        if self.automatic_transposition_outcome == AutomaticTranspositionOutcome::NotRequested
            && let Some(request) = self.automatic_transposition_request
        {
            self.apply_automatic_transposition(provider, code, request);
        }
        self.candidate_batch(view)
    }

    fn automatic_transposition_decision(&self) -> Option<NativeAutomaticTranspositionDecision> {
        let request = self.automatic_transposition_effective_attempt?;
        let outcome = match self.automatic_transposition_outcome {
            AutomaticTranspositionOutcome::NotRequested => return None,
            AutomaticTranspositionOutcome::Suppressed(_) => {
                NativeAutomaticTranspositionOutcome::Suppressed
            }
            AutomaticTranspositionOutcome::NoRecovery(_) => {
                NativeAutomaticTranspositionOutcome::NoRecovery
            }
            AutomaticTranspositionOutcome::RecoveryAvailable(_) => {
                NativeAutomaticTranspositionOutcome::RecoveryAvailable
            }
        };
        Some(NativeAutomaticTranspositionDecision::new_span(
            request.pattern.first_syllable_index
                ..request
                    .pattern
                    .first_syllable_index
                    .saturating_add(request.pattern.syllable_count),
            request.pair_gap_ms,
            match request.cold_tier {
                AutomaticTranspositionTier::Shadow => NativeAutomaticTranspositionTier::Shadow,
                AutomaticTranspositionTier::Secondary => {
                    NativeAutomaticTranspositionTier::Secondary
                }
                AutomaticTranspositionTier::Primary => NativeAutomaticTranspositionTier::Primary,
            },
            match request.tier {
                AutomaticTranspositionTier::Shadow => NativeAutomaticTranspositionTier::Shadow,
                AutomaticTranspositionTier::Secondary => {
                    NativeAutomaticTranspositionTier::Secondary
                }
                AutomaticTranspositionTier::Primary => NativeAutomaticTranspositionTier::Primary,
            },
            outcome,
            self.automatic_transposition_recovered_text.clone(),
            self.automatic_transposition_visible_rank,
        ))
    }

    fn apply_automatic_transposition(
        &mut self,
        provider: &dyn CandidateProvider,
        code: &str,
        request: AutomaticTranspositionRequest,
    ) {
        self.automatic_transposition_effective_attempt = None;
        self.automatic_transposition_recovered_text = None;
        self.automatic_transposition_visible_rank = None;
        if self.requested_limit == 0
            || self.automatic_transposition_blocked
            || self.protected_prefix_len > 0
            || self.provenance.iter().enumerate().any(|(index, item)| {
                native_source_is_explicit_exact(item.source())
                    && (item.source() != NativeCandidateSource::PublicConsensusExact
                        || index < CANDIDATE_PAGE_SIZE)
            })
        {
            self.automatic_transposition_effective_attempt = Some(request.primary);
            self.automatic_transposition_outcome =
                AutomaticTranspositionOutcome::Suppressed(request.primary.tier);
            return;
        }
        let mut recovery = None;
        for attempt in [Some(request.primary), request.fallback]
            .into_iter()
            .flatten()
        {
            let recovery_limit = if attempt.tier == AutomaticTranspositionTier::Shadow {
                1
            } else {
                self.requested_limit
            };
            if let Some(recovered) =
                provider.automatic_transposition_candidates(code, attempt.pattern, recovery_limit)
                && !recovered.candidates.is_empty()
            {
                recovery = Some((attempt, recovered));
                break;
            }
        }
        let Some((attempt, mut recovered)) = recovery else {
            self.automatic_transposition_effective_attempt = Some(request.primary);
            self.automatic_transposition_outcome =
                AutomaticTranspositionOutcome::NoRecovery(request.primary.tier);
            return;
        };
        self.automatic_transposition_effective_attempt = Some(attempt);
        self.automatic_transposition_outcome =
            AutomaticTranspositionOutcome::RecoveryAvailable(attempt.tier);
        self.automatic_transposition_recovered_text = recovered.candidates.first().cloned();
        if attempt.tier == AutomaticTranspositionTier::Shadow {
            return;
        }
        if recovered.provenance.len() != recovered.candidates.len() {
            recovered.provenance =
                vec![NativeCandidateProvenance::default(); recovered.candidates.len()];
        }
        let insertion_index = match attempt.tier {
            AutomaticTranspositionTier::Shadow => unreachable!("shadow recovery returned above"),
            AutomaticTranspositionTier::Secondary => 1.min(self.candidates.len()),
            AutomaticTranspositionTier::Primary => 0,
        };
        let existing = self
            .candidates
            .iter()
            .cloned()
            .zip(self.provenance.iter().copied())
            .collect::<Vec<_>>();
        let mut candidates = Vec::with_capacity(self.requested_limit);
        let mut provenance = Vec::with_capacity(self.requested_limit);
        let mut seen = HashSet::new();
        let ordered = existing[..insertion_index]
            .iter()
            .cloned()
            .chain(recovered.candidates.into_iter().zip(recovered.provenance))
            .chain(existing[insertion_index..].iter().cloned());
        for (candidate, source) in ordered {
            if seen.insert(candidate.clone()) {
                candidates.push(candidate);
                provenance.push(source);
                if candidates.len() == self.requested_limit {
                    break;
                }
            }
        }
        self.candidates = candidates;
        self.provenance = provenance;
        self.automatic_transposition_visible_rank = self
            .automatic_transposition_recovered_text
            .as_ref()
            .and_then(|recovered| {
                self.candidates
                    .iter()
                    .position(|candidate| candidate == recovered)
            })
            .map(|index| index.saturating_add(1));
    }
}

fn native_source_is_explicit_exact(source: NativeCandidateSource) -> bool {
    matches!(
        source,
        NativeCandidateSource::ExplicitAlias
            | NativeCandidateSource::ProjectOverlay
            | NativeCandidateSource::CoreExact
            | NativeCandidateSource::PublicConsensusExact
            | NativeCandidateSource::SupplementalExact
    )
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

#[derive(Default)]
struct RefreshThrottle {
    last_check: Option<Instant>,
}

impl RefreshThrottle {
    fn allow(&mut self, now: Instant) -> bool {
        if self.last_check.is_some_and(|previous| {
            now.checked_duration_since(previous)
                .is_some_and(|elapsed| elapsed < CANDIDATE_RUNTIME_REFRESH_INTERVAL)
        }) {
            return false;
        }
        self.last_check = Some(now);
        true
    }
}

struct SnapshotCandidateProvider {
    snapshot: Arc<CandidateSnapshot>,
    supplemental: PublicSupplementRuntime,
    aliases: Option<ExplicitAliasRuntime>,
    refresh_throttle: Mutex<RefreshThrottle>,
    shape_candidate_pools: Mutex<ShapeCandidatePoolCache>,
    exact_full_code_candidates: Mutex<ExactFullCodeCandidateCache>,
}

impl SnapshotCandidateProvider {
    #[cfg(test)]
    fn new(
        snapshot: Arc<CandidateSnapshot>,
        supplemental: Option<(
            Arc<CandidateSnapshot>,
            crate::SupplementalCandidateLayerConfig,
        )>,
        alias_root: Option<PathBuf>,
    ) -> Self {
        Self {
            snapshot,
            supplemental: PublicSupplementRuntime::static_layer(supplemental),
            aliases: alias_root.map(ExplicitAliasRuntime::new),
            refresh_throttle: Mutex::new(RefreshThrottle::default()),
            shape_candidate_pools: Mutex::new(ShapeCandidatePoolCache::default()),
            exact_full_code_candidates: Mutex::new(ExactFullCodeCandidateCache::default()),
        }
    }

    fn new_with_runtime(
        snapshot: Arc<CandidateSnapshot>,
        supplemental: Option<CandidateRuntimeSupplemental>,
        supplemental_root: Option<PathBuf>,
        alias_root: Option<PathBuf>,
    ) -> Self {
        Self {
            snapshot,
            supplemental: PublicSupplementRuntime::managed(supplemental_root, supplemental),
            aliases: alias_root.map(ExplicitAliasRuntime::new),
            refresh_throttle: Mutex::new(RefreshThrottle::default()),
            shape_candidate_pools: Mutex::new(ShapeCandidatePoolCache::default()),
            exact_full_code_candidates: Mutex::new(ExactFullCodeCandidateCache::default()),
        }
    }

    fn shape_candidate_pool(&self, code: &str) -> Arc<[ShapeCandidate]> {
        if let Ok(mut cache) = self.shape_candidate_pools.lock()
            && let Some(pool) = cache.get(code)
        {
            return pool;
        }

        let candidates = if code.len() == 1 {
            self.snapshot
                .initial_single_character_candidates(code, MAX_TAB_SHAPE_SOURCE_RANK)
                .unwrap_or_default()
        } else {
            self.snapshot
                .exact_single_character_candidates(code, MAX_TAB_SHAPE_SOURCE_RANK)
                .unwrap_or_default()
        };
        let pool = Arc::<[ShapeCandidate]>::from(
            candidates
                .into_iter()
                .map(|candidate| ShapeCandidate {
                    text: candidate.text,
                    resolved_code: candidate.full_code,
                })
                .collect::<Vec<_>>(),
        );
        if let Ok(mut cache) = self.shape_candidate_pools.lock() {
            cache.insert(code, Arc::clone(&pool));
        }
        pool
    }

    fn refresh_at_safe_boundary_at(&self, now: Instant) -> bool {
        let due = self
            .refresh_throttle
            .lock()
            .map(|mut throttle| throttle.allow(now))
            .unwrap_or(false);
        if !due {
            return false;
        }
        let aliases_changed = self
            .aliases
            .as_ref()
            .is_some_and(ExplicitAliasRuntime::refresh);
        let supplemental_changed = self.supplemental.refresh();
        aliases_changed || supplemental_changed
    }

    fn candidate_output(
        &self,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
    ) -> CandidateProviderOutput {
        match view {
            InteractiveCandidateView::Primary => {
                let mut candidates = Vec::new();
                let mut provenance = Vec::new();
                let mut seen = HashSet::new();
                let mut protected_prefix_len = 0;
                let mut automatic_transposition_blocked = false;
                let mut has_explicit_exact_prefix = false;
                if let Some(alias) = self.aliases.as_ref().and_then(|aliases| aliases.text(code)) {
                    automatic_transposition_blocked = true;
                    has_explicit_exact_prefix = true;
                    seen.insert(alias.clone());
                    candidates.push(alias);
                    protected_prefix_len = 1;
                    provenance.push(NativeCandidateProvenance::new(
                        NativeCandidateSource::ExplicitAlias,
                        false,
                    ));
                }
                for candidate in project_overlay_decoder()
                    .decode_exact_full_code(code, limit)
                    .unwrap_or_default()
                {
                    automatic_transposition_blocked = true;
                    has_explicit_exact_prefix = true;
                    if seen.insert(candidate.text.clone()) {
                        candidates.push(candidate.text);
                        provenance.push(NativeCandidateProvenance::new(
                            NativeCandidateSource::ProjectOverlay,
                            false,
                        ));
                    }
                }
                let supplemental = self.supplemental.current();
                let mut snapshot_query = supplemental
                    .as_ref()
                    .and_then(|(supplemental, config)| {
                        match TSF_PUBLIC_CANDIDATE_ORDER_POLICY {
                            WishPublicCandidateOrderPolicy::ConservativeCoreFirst => {
                                // The broader cross-dictionary Top-1 consensus rule
                                // remains audit-only after losing correct Top-1s on
                                // its independent public holdout.
                                layered_candidate_query_with_sources(
                                    &self.snapshot,
                                    supplemental,
                                    code,
                                    limit,
                                    *config,
                                )
                            }
                            WishPublicCandidateOrderPolicy::ExperimentalCrossDictionaryConsensus => {
                                layered_candidate_query_with_consensus_sources(
                                    &self.snapshot,
                                    supplemental,
                                    code,
                                    limit,
                                    *config,
                                )
                            }
                            WishPublicCandidateOrderPolicy::Unrecorded => {
                                unreachable!("the TSF public candidate order policy is explicit")
                            }
                        }
                        .ok()
                    })
                    .unwrap_or_else(|| {
                        self.snapshot
                            .interactive_candidate_query(code, limit)
                            .unwrap_or_else(|_| InteractiveCandidateQuery {
                                candidates: Vec::new(),
                                automatic_transposition_blocked: true,
                            })
                    });
                if !has_explicit_exact_prefix
                    && limit >= 2
                    && !snapshot_query.candidates.is_empty()
                    && let Ok(FourCharacterCorrectionDecision::Offer(offer)) =
                        layered_four_character_correction_decision(
                            &self.snapshot,
                            supplemental.as_ref().map(|(snapshot, _)| snapshot.as_ref()),
                            code,
                            1,
                        )
                    && let Some(recovered) = offer.candidates.into_iter().next()
                {
                    let existing_index = snapshot_query
                        .candidates
                        .iter()
                        .position(|candidate| candidate.text == recovered.text);
                    if existing_index != Some(0) {
                        if let Some(existing_index) = existing_index {
                            snapshot_query.candidates.remove(existing_index);
                        }
                        let insertion_index = 1.min(snapshot_query.candidates.len());
                        snapshot_query.candidates.insert(
                            insertion_index,
                            crate::candidate_snapshot::InteractiveCandidateText {
                                text: recovered.text,
                                source: InteractiveCandidateSource::FourCharacterCorrection,
                            },
                        );
                        snapshot_query.candidates.truncate(limit);
                        automatic_transposition_blocked = true;
                    }
                }
                automatic_transposition_blocked |= snapshot_query.automatic_transposition_blocked;
                for candidate in snapshot_query.candidates {
                    if seen.insert(candidate.text.clone()) {
                        candidates.push(candidate.text);
                        provenance.push(NativeCandidateProvenance::new(
                            native_candidate_source(candidate.source),
                            false,
                        ));
                    }
                    if candidates.len() == limit {
                        break;
                    }
                }
                candidates.truncate(limit);
                provenance.truncate(candidates.len());
                protected_prefix_len = protected_prefix_len.min(candidates.len());
                CandidateProviderOutput {
                    candidates,
                    provenance,
                    protected_prefix_len,
                    automatic_transposition_blocked,
                }
            }
            InteractiveCandidateView::TranspositionRecovery => {
                let candidates = self
                    .snapshot
                    .transposition_recovery_texts(code, limit)
                    .unwrap_or_default();
                let provenance = vec![
                    NativeCandidateProvenance::new(
                        NativeCandidateSource::TranspositionRecovery,
                        false,
                    );
                    candidates.len()
                ];
                CandidateProviderOutput {
                    candidates,
                    provenance,
                    protected_prefix_len: 0,
                    automatic_transposition_blocked: true,
                }
            }
        }
    }
}

fn native_candidate_source(source: InteractiveCandidateSource) -> NativeCandidateSource {
    match source {
        InteractiveCandidateSource::CoreExact => NativeCandidateSource::CoreExact,
        InteractiveCandidateSource::PublicConsensusExact => {
            NativeCandidateSource::PublicConsensusExact
        }
        InteractiveCandidateSource::SupplementalExact => NativeCandidateSource::SupplementalExact,
        InteractiveCandidateSource::CharacterPair => NativeCandidateSource::CharacterPair,
        InteractiveCandidateSource::CompleteSentence
        | InteractiveCandidateSource::FinalInitialSentence => NativeCandidateSource::Decoder,
        InteractiveCandidateSource::Decoder => NativeCandidateSource::Decoder,
        InteractiveCandidateSource::FourCharacterCorrection => {
            NativeCandidateSource::FourCharacterCorrection
        }
    }
}

#[derive(Clone)]
struct CandidateProviderBlueprint {
    snapshot: Arc<CandidateSnapshot>,
    supplemental: Option<CandidateRuntimeSupplemental>,
    supplemental_root: Option<PathBuf>,
    alias_root: Option<PathBuf>,
}

impl CandidateProviderBlueprint {
    fn build(&self) -> Arc<dyn CandidateProvider> {
        Arc::new(SnapshotCandidateProvider::new_with_runtime(
            Arc::clone(&self.snapshot),
            self.supplemental.clone(),
            self.supplemental_root.clone(),
            self.alias_root.clone(),
        ))
    }
}

struct PublicSupplementRuntime {
    root: Option<PathBuf>,
    state: Mutex<PublicSupplementRuntimeState>,
}

struct PublicSupplementRuntimeState {
    package_id: Option<String>,
    snapshot: Option<Arc<CandidateSnapshot>>,
    config: Option<crate::SupplementalCandidateLayerConfig>,
}

impl PublicSupplementRuntime {
    #[cfg(test)]
    fn static_layer(
        supplemental: Option<(
            Arc<CandidateSnapshot>,
            crate::SupplementalCandidateLayerConfig,
        )>,
    ) -> Self {
        let (snapshot, config) = supplemental
            .map(|(snapshot, config)| (Some(snapshot), Some(config)))
            .unwrap_or_default();
        Self {
            root: None,
            state: Mutex::new(PublicSupplementRuntimeState {
                package_id: None,
                snapshot,
                config,
            }),
        }
    }

    fn managed(root: Option<PathBuf>, supplemental: Option<CandidateRuntimeSupplemental>) -> Self {
        let state = match supplemental {
            Some(supplemental) => PublicSupplementRuntimeState {
                package_id: Some(supplemental.package_id().to_owned()),
                snapshot: Some(Arc::clone(supplemental.snapshot())),
                config: Some(supplemental.config()),
            },
            None => PublicSupplementRuntimeState {
                package_id: None,
                snapshot: None,
                config: None,
            },
        };
        let runtime = Self {
            root,
            state: Mutex::new(state),
        };
        let _ = runtime.refresh();
        runtime
    }

    fn current(
        &self,
    ) -> Option<(
        Arc<CandidateSnapshot>,
        crate::SupplementalCandidateLayerConfig,
    )> {
        self.state
            .lock()
            .ok()
            .and_then(|state| Some((Arc::clone(state.snapshot.as_ref()?), state.config?)))
    }

    fn revision(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| {
            state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.revision().to_owned())
        })
    }

    fn refresh(&self) -> bool {
        let Some(root) = self.root.as_deref() else {
            return false;
        };
        let next = match load_candidate_runtime_supplemental_selection(root) {
            Ok(next) => next,
            Err(_) => return false,
        };
        let current = match self.state.lock() {
            Ok(state) => (
                state.package_id.clone(),
                state.snapshot.clone(),
                state.config,
            ),
            Err(_) => return false,
        };
        let next_state = match &next {
            CandidateRuntimeSupplementalSelection::Disabled => {
                if current.0.is_none() && current.1.is_none() {
                    return false;
                }
                PublicSupplementRuntimeState {
                    package_id: None,
                    snapshot: None,
                    config: None,
                }
            }
            CandidateRuntimeSupplementalSelection::Enabled { package_id, config } => {
                if current.0.as_deref() == Some(package_id)
                    && current.1.is_some()
                    && current.2 == Some(*config)
                {
                    return false;
                }
                if current.0.as_deref() == Some(package_id) {
                    PublicSupplementRuntimeState {
                        package_id: Some(package_id.clone()),
                        snapshot: current.1,
                        config: Some(*config),
                    }
                } else {
                    let loaded = match load_candidate_runtime_supplemental(root, &next) {
                        Ok(Some(loaded)) => loaded,
                        _ => return false,
                    };
                    PublicSupplementRuntimeState {
                        package_id: Some(loaded.package_id().to_owned()),
                        snapshot: Some(Arc::clone(loaded.snapshot())),
                        config: Some(loaded.config()),
                    }
                }
            }
        };
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        state.package_id = next_state.package_id;
        state.snapshot = next_state.snapshot;
        state.config = next_state.config;
        true
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
        self.candidate_output(code, limit, view).candidates
    }

    fn candidate_data_identity(&self) -> Option<CandidateDataIdentity> {
        Some(CandidateDataIdentity {
            core_revision: self.snapshot.revision().to_owned(),
            supplemental_revision: self.supplemental.revision(),
        })
    }

    fn candidates_with_provenance(
        &self,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
    ) -> CandidateProviderOutput {
        self.candidate_output(code, limit, view)
    }

    fn automatic_transposition_candidates(
        &self,
        code: &str,
        pattern: AutomaticTranspositionPattern,
        limit: usize,
    ) -> Option<CandidateProviderOutput> {
        let promotion = if pattern.syllable_count == 1 {
            self.snapshot
                .automatic_transposition_recovery_after_primary(
                    code,
                    pattern.first_syllable_index,
                    limit,
                )
        } else {
            self.snapshot
                .automatic_transposition_span_recovery_after_primary(
                    code,
                    pattern.first_syllable_index,
                    pattern.syllable_count,
                    limit,
                )
        }
        .ok()??;
        let candidates = promotion
            .candidates
            .into_iter()
            .take(limit)
            .collect::<Vec<_>>();
        let provenance = vec![
            NativeCandidateProvenance::new(
                NativeCandidateSource::TranspositionRecovery,
                false,
            );
            candidates.len()
        ];
        Some(CandidateProviderOutput {
            candidates,
            provenance,
            protected_prefix_len: 0,
            automatic_transposition_blocked: true,
        })
    }

    fn refresh_at_safe_boundary(&self) -> bool {
        self.refresh_at_safe_boundary_at(Instant::now())
    }

    fn protected_candidate_prefix_len(&self, code: &str, view: InteractiveCandidateView) -> usize {
        usize::from(
            view == InteractiveCandidateView::Primary
                && self
                    .aliases
                    .as_ref()
                    .and_then(|aliases| aliases.text(code))
                    .is_some(),
        )
    }

    fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
        let supplemental = self.supplemental.current();
        let supplemental_revision = supplemental
            .as_ref()
            .map(|(snapshot, _)| snapshot.revision());
        if let Ok(mut cache) = self.exact_full_code_candidates.lock()
            && let Some(exact) = cache.get(code, text, supplemental_revision)
        {
            return exact;
        }
        let exact = project_overlay_decoder()
            .decode_exact_full_code(code, CANDIDATE_LIMIT)
            .ok()
            .is_some_and(|candidates| candidates.iter().any(|candidate| candidate.text == text))
            || self
                .snapshot
                .exact_full_code_texts(code, CANDIDATE_LIMIT)
                .ok()
                .is_some_and(|candidates| candidates.iter().any(|candidate| candidate == text))
            || supplemental.as_ref().is_some_and(|(snapshot, _)| {
                snapshot
                    .exact_full_code_texts(code, CANDIDATE_LIMIT)
                    .ok()
                    .is_some_and(|candidates| candidates.iter().any(|candidate| candidate == text))
            });
        if let Ok(mut cache) = self.exact_full_code_candidates.lock() {
            cache.insert(code, text, supplemental_revision, exact);
        }
        exact
    }

    fn shape_candidates(
        &self,
        code: &str,
        stroke_prefix: &str,
        limit: usize,
    ) -> Vec<ShapeCandidate> {
        if !matches!(code.len(), 1 | 2)
            || limit == 0
            || !stroke_prefix.as_bytes().iter().all(u8::is_ascii_lowercase)
        {
            return Vec::new();
        }
        let shapes = if stroke_prefix.is_empty() {
            None
        } else {
            let Some(shapes) = public_shape_index() else {
                return Vec::new();
            };
            Some(shapes)
        };
        self.shape_candidate_pool(code)
            .iter()
            .filter(|candidate| {
                let mut characters = candidate.text.chars();
                let Some(character) = characters.next() else {
                    return false;
                };
                if characters.next().is_some() {
                    return false;
                }
                shapes.is_none_or(|shapes| {
                    shapes.get(character).is_some_and(|shape| {
                        shape
                            .stroke_codes()
                            .iter()
                            .any(|code| code.starts_with(stroke_prefix))
                            || shape
                                .component_codes()
                                .iter()
                                .any(|code| code.starts_with(stroke_prefix))
                    })
                })
            })
            .take(limit.min(CANDIDATE_LIMIT))
            .cloned()
            .collect()
    }
}

fn public_shape_index() -> Option<&'static CharacterShapeIndex> {
    static INDEX: OnceLock<Option<CharacterShapeIndex>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            parse_stroke_sequence_tsv(TSF_PUBLIC_STROKE_SEQUENCES)
                .ok()
                .and_then(|import| CharacterShapeIndex::new(import.into_shapes()).ok())
        })
        .as_ref()
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
    provenance: Vec<NativeCandidateProvenance>,
    personalized: Vec<bool>,
    automatic_transposition: Option<NativeAutomaticTranspositionDecision>,
    tab_assembly: Option<NativeTabAssemblyState>,
    page_start: usize,
    may_have_more: bool,
    view: InteractiveCandidateView,
    action_detail: Option<String>,
    notice: bool,
    notice_icon: CandidateNoticeIcon,
    mode: CandidateDisplayMode,
    mode_label_override: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CandidateNoticeIcon {
    #[default]
    None,
    WishReceived,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CandidateDisplayMode {
    #[default]
    Normal,
    Shape,
    ShapeAssemblyFirst,
    ShapeAssemblySecond,
    ForgetSelecting,
    ForgetUndo,
    ForgetProtected,
    ForgetNotPersonal,
    ForgetSaveFailed,
    ForgetRestored,
}

impl CandidateDisplayMode {
    fn footer_label(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Shape => Some("找字"),
            Self::ShapeAssemblyFirst => Some("找第 1 字"),
            Self::ShapeAssemblySecond => Some("找第 2 字"),
            Self::ForgetSelecting => Some("忘记 · 数字选择"),
            Self::ForgetUndo => Some("已忘记 · 退格撤销"),
            Self::ForgetProtected => Some("固定词"),
            Self::ForgetNotPersonal => Some("没有个人排序"),
            Self::ForgetSaveFailed => Some("未保存"),
            Self::ForgetRestored => Some("已恢复"),
        }
    }
}

fn shape_display_mode(session: &CompositionSession) -> CandidateDisplayMode {
    match session.tab_assembly_stage() {
        Some(TabAssemblyStage::First) => CandidateDisplayMode::ShapeAssemblyFirst,
        Some(TabAssemblyStage::Second | TabAssemblyStage::Later(_)) => {
            CandidateDisplayMode::ShapeAssemblySecond
        }
        None => CandidateDisplayMode::Shape,
    }
}

fn active_shape_code(session: &CompositionSession) -> &str {
    session.shape_pinyin().unwrap_or(session.phonetic())
}

fn shape_candidate_display(
    mut display: CandidateDisplay,
    session: &CompositionSession,
) -> CandidateDisplay {
    let position = session.tab_assembly_position().unwrap_or(1);
    let total = session.tab_assembly_character_count().unwrap_or(1);
    display.tab_assembly = Some(NativeTabAssemblyState::new(
        position,
        total,
        session.stroke_prefix(),
    ));
    let display = display.with_mode(shape_display_mode(session));
    let stage = match (
        session.tab_assembly_selected_text(),
        session.tab_assembly_position(),
    ) {
        (Some(selected), Some(position)) => format!("{selected} → 第 {position} 字"),
        (None, Some(position)) => format!("找第 {position} 字"),
        _ => "找字".to_owned(),
    };
    let slot = active_shape_code(session);
    let slot = if slot.len() == 1 {
        format!("{slot}·声母")
    } else {
        slot.to_owned()
    };
    let refinement = shape_prefix_display(session.stroke_prefix());
    display.with_mode_label(format!("{stage} · {slot} · {refinement}"))
}

fn shape_prefix_display(prefix: &str) -> String {
    if prefix.is_empty() {
        return "形码 —".to_owned();
    }
    if prefix
        .as_bytes()
        .iter()
        .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z'))
    {
        let strokes = prefix
            .bytes()
            .map(|byte| match byte {
                b'h' => "横",
                b's' => "竖",
                b'p' => "撇",
                b'n' => "捺",
                b'z' => "折",
                _ => unreachable!("stroke prefix was validated"),
            })
            .collect::<Vec<_>>()
            .join("·");
        format!("笔画 {strokes}")
    } else {
        format!("部件 {prefix}")
    }
}

fn tab_phonetic_segments(code: &str) -> Option<Vec<&str>> {
    if code.is_empty()
        || code.len() > MAX_TAB_ASSEMBLY_CHARACTERS.saturating_mul(2)
        || !code.as_bytes().iter().all(u8::is_ascii_lowercase)
    {
        return None;
    }
    let complete_end = code.len() / 2 * 2;
    let mut segments = (0..complete_end)
        .step_by(2)
        .map(|start| &code[start..start + 2])
        .collect::<Vec<_>>();
    if complete_end < code.len() {
        segments.push(&code[complete_end..]);
    }
    (segments.len() <= MAX_TAB_ASSEMBLY_CHARACTERS).then_some(segments)
}

impl CandidateDisplay {
    fn actions(actions: &[InlineWishAction]) -> Self {
        Self {
            candidates: actions
                .iter()
                .map(|action| action.label.to_owned())
                .collect(),
            provenance: vec![NativeCandidateProvenance::default(); actions.len()],
            personalized: vec![false; actions.len()],
            automatic_transposition: None,
            tab_assembly: None,
            page_start: 0,
            may_have_more: false,
            view: InteractiveCandidateView::Primary,
            action_detail: if actions.len() == 1 {
                actions.first().map(|action| action.detail.to_owned())
            } else {
                None
            },
            notice: false,
            notice_icon: CandidateNoticeIcon::None,
            mode: CandidateDisplayMode::Normal,
            mode_label_override: None,
        }
    }

    fn notice(label: &str, detail: &str) -> Self {
        Self::notice_with_icon(label, detail, CandidateNoticeIcon::None)
    }

    fn notice_with_icon(label: &str, detail: &str, notice_icon: CandidateNoticeIcon) -> Self {
        Self {
            candidates: vec![label.to_owned()],
            provenance: vec![NativeCandidateProvenance::default()],
            personalized: vec![false],
            automatic_transposition: None,
            tab_assembly: None,
            page_start: 0,
            may_have_more: false,
            view: InteractiveCandidateView::Primary,
            action_detail: Some(detail.to_owned()),
            notice: true,
            notice_icon,
            mode: CandidateDisplayMode::Normal,
            mode_label_override: None,
        }
    }

    #[cfg(test)]
    fn from_candidates(candidates: Vec<String>, requested_page_start: usize) -> Self {
        Self::from_batch(
            CandidateBatch {
                provenance: vec![NativeCandidateProvenance::default(); candidates.len()],
                personalized: vec![false; candidates.len()],
                resolved_shape_codes: vec![None; candidates.len()],
                protected_prefix_len: 0,
                candidates,
                automatic_transposition: None,
                may_have_more: false,
                view: InteractiveCandidateView::Primary,
            },
            requested_page_start,
        )
    }

    fn from_batch(batch: CandidateBatch, requested_page_start: usize) -> Self {
        let CandidateBatch {
            candidates,
            resolved_shape_codes: _,
            mut provenance,
            mut personalized,
            protected_prefix_len: _,
            automatic_transposition,
            may_have_more,
            view,
        } = batch;
        if provenance.len() != candidates.len() {
            provenance = vec![NativeCandidateProvenance::default(); candidates.len()];
        }
        if personalized.len() != candidates.len() {
            personalized = vec![false; candidates.len()];
        }
        let page_start = if candidates.is_empty() {
            0
        } else {
            requested_page_start
                .min((candidates.len() - 1) / CANDIDATE_PAGE_SIZE * CANDIDATE_PAGE_SIZE)
        };
        Self {
            candidates,
            provenance,
            personalized,
            automatic_transposition,
            tab_assembly: None,
            page_start,
            may_have_more,
            view,
            action_detail: None,
            notice: false,
            notice_icon: CandidateNoticeIcon::None,
            mode: CandidateDisplayMode::Normal,
            mode_label_override: None,
        }
    }

    fn with_mode(mut self, mode: CandidateDisplayMode) -> Self {
        self.mode = mode;
        self
    }

    fn with_mode_label(mut self, label: String) -> Self {
        self.mode_label_override = Some(label);
        self
    }

    fn visible(&self) -> &[String] {
        let end = self
            .page_start
            .saturating_add(CANDIDATE_PAGE_SIZE)
            .min(self.candidates.len());
        &self.candidates[self.page_start.min(end)..end]
    }

    fn visible_provenance(&self) -> &[NativeCandidateProvenance] {
        let end = self
            .page_start
            .saturating_add(CANDIDATE_PAGE_SIZE)
            .min(self.provenance.len());
        &self.provenance[self.page_start.min(end)..end]
    }

    fn visible_personalized(&self) -> &[bool] {
        let end = self
            .page_start
            .saturating_add(CANDIDATE_PAGE_SIZE)
            .min(self.personalized.len());
        &self.personalized[self.page_start.min(end)..end]
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

    fn is_notice(&self) -> bool {
        self.notice
    }

    fn notice_icon(&self) -> CandidateNoticeIcon {
        self.notice_icon
    }

    #[cfg(test)]
    fn mode(&self) -> CandidateDisplayMode {
        self.mode
    }

    fn mode_label(&self) -> Option<&str> {
        self.mode_label_override
            .as_deref()
            .or_else(|| self.mode.footer_label())
    }

    fn feedback_event(&self, code: &str, shape_mode: bool) -> NativeFeedbackEvent {
        let provenance = if shape_mode {
            self.visible_provenance()
                .iter()
                .map(|item| {
                    NativeCandidateProvenance::with_personalization(
                        NativeCandidateSource::Shape,
                        item.personalization(),
                    )
                })
                .collect()
        } else {
            self.visible_provenance().to_vec()
        };
        NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            code: code.to_owned(),
            view: native_candidate_view(self.view, shape_mode),
            page_start: self.page_start,
            candidates: self.visible().to_vec(),
            provenance,
            automatic_transposition: (!shape_mode)
                .then(|| self.automatic_transposition.clone())
                .flatten(),
            loaded_candidates: self.candidates.len(),
            tab_assembly: shape_mode.then(|| self.tab_assembly.clone()).flatten(),
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
    operation: InlineWishOperation,
    label: &'static str,
    detail: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineWishOperation {
    Command(WishCommand),
    Capture {
        scope: WishCaptureScope,
        category: WishCategory,
    },
}

fn inline_wish_actions(summary: NativeFeedbackSummary) -> Vec<InlineWishAction> {
    match summary.lifecycle {
        NativeFeedbackLifecycle::Disabled => vec![InlineWishAction {
            operation: InlineWishOperation::Command(WishCommand::Start),
            label: "开始反馈",
            detail: "暂不保存",
        }],
        NativeFeedbackLifecycle::Recording => vec![
            InlineWishAction {
                operation: InlineWishOperation::Capture {
                    scope: WishCaptureScope::RecentEpisodes,
                    category: WishCategory::Other,
                },
                label: "记录刚才的情况",
                detail: "自动分段",
            },
            InlineWishAction {
                operation: InlineWishOperation::Capture {
                    scope: WishCaptureScope::RecentWindow,
                    category: WishCategory::Other,
                },
                label: "记录更多内容",
                detail: "近30秒",
            },
        ],
        NativeFeedbackLifecycle::Stopped => vec![InlineWishAction {
            operation: InlineWishOperation::Command(WishCommand::ClearStopped),
            label: "清除反馈",
            detail: "已停止",
        }],
    }
}

fn inline_wish_notice(
    operation: InlineWishOperation,
    status: WishCommandAckStatus,
) -> CandidateDisplay {
    let label = match (operation, status) {
        (InlineWishOperation::Capture { .. }, WishCommandAckStatus::Applied) => "已经保存",
        (InlineWishOperation::Capture { .. }, WishCommandAckStatus::NoChange) => {
            "刚才没有可保存的内容"
        }
        (InlineWishOperation::Capture { .. }, WishCommandAckStatus::Failed) => "保存失败",
        (InlineWishOperation::Command(WishCommand::Start), WishCommandAckStatus::Applied) => {
            "反馈已开始"
        }
        (
            InlineWishOperation::Command(WishCommand::ClearStopped),
            WishCommandAckStatus::Applied,
        ) => "反馈已清除",
        (InlineWishOperation::Command(_), WishCommandAckStatus::Applied) => "操作已完成",
        (InlineWishOperation::Command(_), WishCommandAckStatus::NoChange) => "状态没有变化",
        (InlineWishOperation::Command(_), WishCommandAckStatus::Failed) => "操作未完成",
    };
    let detail = match status {
        WishCommandAckStatus::Applied => "可以继续输入",
        WishCommandAckStatus::NoChange => "继续输入后再试",
        WishCommandAckStatus::Failed => "稍后再试",
    };
    if matches!(operation, InlineWishOperation::Capture { .. })
        && status == WishCommandAckStatus::Applied
    {
        CandidateDisplay::notice_with_icon(label, detail, CandidateNoticeIcon::WishReceived)
    } else {
        CandidateDisplay::notice(label, detail)
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
                supplemental: None,
                supplemental_root: None,
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

#[cfg(test)]
fn candidate_provider_for_root(
    root: &Path,
    alias_root: Option<PathBuf>,
) -> CandidateProviderLoadResult {
    candidate_provider_for_roots(root, alias_root, None)
}

fn candidate_provider_for_roots(
    root: &Path,
    alias_root: Option<PathBuf>,
    supplemental_root: Option<&Path>,
) -> CandidateProviderLoadResult {
    match load_candidate_runtime_snapshots(root, supplemental_root)
        .map_err(CandidateProviderLoadError::Runtime)?
    {
        Some(runtime) => Ok(CandidateProviderBlueprint {
            snapshot: Arc::clone(runtime.core()),
            supplemental: runtime.supplemental().cloned(),
            supplemental_root: supplemental_root.map(Path::to_path_buf),
            alias_root,
        }),
        None => development_candidate_blueprint().map(|mut blueprint| {
            blueprint.supplemental_root = supplemental_root.map(Path::to_path_buf);
            blueprint.alias_root = alias_root;
            blueprint
        }),
    }
}

fn current_module_handle() -> std::result::Result<HMODULE, CandidateProviderLoadError> {
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
    Ok(module)
}

fn module_path() -> std::result::Result<PathBuf, CandidateProviderLoadError> {
    let module = current_module_handle()?;
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: `module` identifies the image containing this function and the
    // writable buffer remains alive for the synchronous call.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(CandidateProviderLoadError::ModuleLocation);
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn immutable_module_sha256(path: &Path) -> Option<String> {
    if !path
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("ziranma_core.dll")
    {
        return None;
    }
    let digest = path.parent()?.file_name()?.to_str()?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if !path
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("builds")
        || !path
            .parent()?
            .parent()?
            .parent()?
            .file_name()?
            .to_str()?
            .eq_ignore_ascii_case("tsf-alpha")
    {
        return None;
    }
    Some(digest.to_ascii_lowercase())
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

fn research_feedback_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, RESEARCH_FEEDBACK_DIRECTORY)
}

fn personal_ranking_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, "personal-ranking")
}

fn personal_ranking_suppression_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, PERSONAL_RANKING_SUPPRESSION_DIRECTORY)
}

fn public_supplement_root_for_module(module_path: &Path) -> Option<PathBuf> {
    installed_user_data_root_for_module(module_path, "public-supplement")
}

fn class_factory_candidate_provider() -> CandidateProviderLoadResult {
    static BLUEPRINT: OnceLock<CandidateProviderLoadResult> = OnceLock::new();
    BLUEPRINT
        .get_or_init(|| {
            let root = module_candidate_runtime_root()?;
            let module_path = module_path()?;
            let alias_root = explicit_alias_root_for_module(&module_path);
            let supplemental_root = public_supplement_root_for_module(&module_path);
            candidate_provider_for_roots(&root, alias_root, supplemental_root.as_deref())
        })
        .clone()
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
            supplemental: None,
            supplemental_root: None,
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
    overruled_text_to_remember: Option<String>,
    feedback_after_success: Option<NativeFeedbackEvent>,
    action_after_success: Option<PlannedAction>,
    candidate_forget_action_after_success: Option<PlannedCandidateForgetAction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedAction {
    Wish(InlineWishOperation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateForgetMessage {
    Select,
    Protected,
    NotPersonal,
    SaveFailed,
}

impl CandidateForgetMessage {
    fn display_mode(self) -> CandidateDisplayMode {
        match self {
            Self::Select => CandidateDisplayMode::ForgetSelecting,
            Self::Protected => CandidateDisplayMode::ForgetProtected,
            Self::NotPersonal => CandidateDisplayMode::ForgetNotPersonal,
            Self::SaveFailed => CandidateDisplayMode::ForgetSaveFailed,
        }
    }
}

#[derive(Clone, Default)]
enum CandidateForgetState {
    #[default]
    Inactive,
    Choosing(CandidateForgetMessage),
    UndoAvailable {
        code: String,
        text: String,
        restore_session: bool,
    },
}

impl fmt::Debug for CandidateForgetState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactive => formatter.write_str("Inactive"),
            Self::Choosing(message) => formatter.debug_tuple("Choosing").field(message).finish(),
            Self::UndoAvailable { .. } => formatter
                .debug_struct("UndoAvailable")
                .field("debug_contains_text", &false)
                .finish(),
        }
    }
}

enum PlannedCandidateForgetAction {
    Enter,
    Cancel,
    Message(CandidateForgetMessage),
    Suppress {
        code: String,
        text: String,
        restore_session: bool,
    },
    Restore {
        code: String,
        text: String,
        restore_session: bool,
    },
    Finalize,
}

fn plan_candidate_forget_ui(
    session: &CompositionSession,
    candidate_display: Option<CandidateDisplay>,
    action: PlannedCandidateForgetAction,
) -> PlannedKey {
    PlannedKey {
        before: session.clone(),
        after: session.clone(),
        edit: None,
        candidate_display,
        selection_to_remember: None,
        overruled_text_to_remember: None,
        feedback_after_success: None,
        action_after_success: None,
        candidate_forget_action_after_success: Some(action),
    }
}

#[derive(Clone)]
struct PlannedSelection {
    code: String,
    text: String,
    retractable_by_immediate_backspace: bool,
}

#[derive(Clone)]
struct PersonalPhraseComponent {
    code: String,
    text: String,
}

const MAX_PERSONAL_PHRASE_COMPONENTS: usize = 4;

#[derive(Default)]
struct PersonalPhraseComposer {
    components: Vec<PersonalPhraseComponent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersonalPhraseDocumentAdjacency {
    KeyboardFallback,
    NoPreviousAnchor,
    VerifiedAdjacent,
    CaretMoved,
    AnchorTextChanged,
    ContextChanged,
    RangeUnavailable,
}

impl PersonalPhraseDocumentAdjacency {
    fn allows_continuation(self) -> bool {
        matches!(
            self,
            Self::KeyboardFallback | Self::VerifiedAdjacent | Self::RangeUnavailable
        )
    }

    fn feedback_value(self) -> NativePersonalPhraseAdjacency {
        match self {
            Self::KeyboardFallback => NativePersonalPhraseAdjacency::KeyboardFallback,
            Self::NoPreviousAnchor => NativePersonalPhraseAdjacency::FirstAnchor,
            Self::VerifiedAdjacent => NativePersonalPhraseAdjacency::VerifiedAdjacent,
            Self::CaretMoved => NativePersonalPhraseAdjacency::CaretMoved,
            Self::AnchorTextChanged => NativePersonalPhraseAdjacency::AnchorTextChanged,
            Self::ContextChanged => NativePersonalPhraseAdjacency::ContextChanged,
            Self::RangeUnavailable => NativePersonalPhraseAdjacency::RangeUnavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PersonalPhraseAdjacencyObservation {
    adjacency: PersonalPhraseDocumentAdjacency,
    previous_components: usize,
    resulting_components: usize,
}

#[derive(Clone)]
struct PersonalPhraseDocumentAnchor {
    context: ITfContext,
    range: ITfRange,
    expected_text: String,
}

#[derive(Clone, Default)]
struct PersonalPhraseDocumentSnapshot {
    anchor: Option<PersonalPhraseDocumentAnchor>,
    range_fallback_pending: bool,
}

#[derive(Default)]
struct PersonalPhraseDocumentTracker {
    anchor: Option<PersonalPhraseDocumentAnchor>,
    range_fallback_pending: bool,
    completed_adjacency: Option<PersonalPhraseDocumentAdjacency>,
    last_consumed_adjacency: Option<PersonalPhraseDocumentAdjacency>,
}

impl PersonalPhraseDocumentTracker {
    fn observe_composition_start(
        &self,
        context: &ITfContext,
        range: &ITfRange,
        ec: u32,
        selection_replaced: bool,
    ) -> PersonalPhraseDocumentAdjacency {
        let Some(anchor) = self.anchor.as_ref() else {
            if self.range_fallback_pending && selection_replaced {
                return PersonalPhraseDocumentAdjacency::AnchorTextChanged;
            }
            return if self.range_fallback_pending {
                PersonalPhraseDocumentAdjacency::RangeUnavailable
            } else {
                PersonalPhraseDocumentAdjacency::NoPreviousAnchor
            };
        };
        match same_com_identity(&anchor.context, context) {
            Ok(true) => {}
            Ok(false) => return PersonalPhraseDocumentAdjacency::ContextChanged,
            Err(_) => return PersonalPhraseDocumentAdjacency::RangeUnavailable,
        }
        if selection_replaced {
            return PersonalPhraseDocumentAdjacency::AnchorTextChanged;
        }
        // SAFETY: both ranges belong to this context and `ec` is the active
        // synchronous read/write cookie. Neither comparison mutates text.
        match unsafe { range.CompareStart(ec, &anchor.range, TF_ANCHOR_END) } {
            Ok(0) => {}
            Ok(_) => return PersonalPhraseDocumentAdjacency::CaretMoved,
            Err(_) => return PersonalPhraseDocumentAdjacency::RangeUnavailable,
        }
        match range_text_equals(&anchor.range, ec, &anchor.expected_text) {
            Ok(true) => PersonalPhraseDocumentAdjacency::VerifiedAdjacent,
            Ok(false) => PersonalPhraseDocumentAdjacency::AnchorTextChanged,
            Err(_) => PersonalPhraseDocumentAdjacency::RangeUnavailable,
        }
    }

    fn complete_personal_commit(
        &mut self,
        context: &ITfContext,
        range: Option<ITfRange>,
        expected_text: String,
        adjacency: PersonalPhraseDocumentAdjacency,
        range_ready: bool,
    ) {
        self.completed_adjacency = Some(adjacency);
        let anchor = if range_ready {
            range.map(|range| PersonalPhraseDocumentAnchor {
                context: context.clone(),
                range,
                expected_text,
            })
        } else {
            None
        };
        self.range_fallback_pending = anchor.is_none();
        self.anchor = anchor;
    }

    fn take_completed_adjacency(&mut self) -> Option<PersonalPhraseDocumentAdjacency> {
        self.completed_adjacency.take()
    }

    fn snapshot(&self) -> PersonalPhraseDocumentSnapshot {
        PersonalPhraseDocumentSnapshot {
            anchor: self.anchor.clone(),
            range_fallback_pending: self.range_fallback_pending,
        }
    }

    fn restore(&mut self, snapshot: PersonalPhraseDocumentSnapshot) {
        self.anchor = snapshot.anchor;
        self.range_fallback_pending = snapshot.range_fallback_pending;
        self.completed_adjacency = None;
    }

    fn mark_range_fallback_after_commit(&mut self) {
        self.anchor = None;
        self.range_fallback_pending = true;
        self.completed_adjacency = None;
    }

    fn clear(&mut self) {
        self.anchor = None;
        self.range_fallback_pending = false;
        self.completed_adjacency = None;
        self.last_consumed_adjacency = None;
    }
}

fn range_text_equals(range: &ITfRange, ec: u32, expected: &str) -> Result<bool> {
    let expected: Vec<u16> = expected.encode_utf16().collect();
    let mut actual = vec![0_u16; expected.len().saturating_add(1)];
    let mut fetched = 0;
    // SAFETY: the clone stays inside this edit session. GetText advances only
    // the clone and the buffer has one extra unit to detect a longer range.
    let probe = unsafe { range.Clone() }?;
    unsafe { probe.GetText(ec, TF_TF_MOVESTART, &mut actual, &mut fetched) }?;
    let fetched = usize::try_from(fetched).map_err(|_| lifecycle_error(E_UNEXPECTED))?;
    Ok(fetched == expected.len() && actual[..fetched] == expected)
}

fn context_selection_replaces_text(context: &ITfContext, ec: u32) -> Result<bool> {
    // SAFETY: TF_SELECTION is an ABI container whose zeroed interface field is
    // a valid `None`; GetSelection initializes at most the one requested slot.
    let mut selection = [unsafe { std::mem::zeroed::<TF_SELECTION>() }];
    let mut fetched = 0;
    let result =
        unsafe { context.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched) };
    let replaces = result.and_then(|()| {
        if fetched != 1 {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        let range = selection[0]
            .range
            .as_ref()
            .cloned()
            .ok_or_else(|| lifecycle_error(E_UNEXPECTED))?;
        // SAFETY: compares the two endpoints of the same context-owned range.
        unsafe { range.CompareStart(ec, &range, TF_ANCHOR_END) }.map(|ordering| ordering != 0)
    });
    // SAFETY: GetSelection returned ownership of the interface in this ABI
    // slot; release it exactly once, including on validation failure.
    unsafe { std::mem::ManuallyDrop::drop(&mut selection[0].range) };
    replaces
}

struct PendingPersonalPhrase {
    selection: PlannedSelection,
    previous_session_text: Option<String>,
}

struct PendingPersonalSelection {
    selection: PlannedSelection,
    overruled_text: Option<String>,
    previous_session_text: Option<String>,
    phrase: Option<PendingPersonalPhrase>,
    previous_phrase_components: Vec<PersonalPhraseComponent>,
    previous_phrase_document: PersonalPhraseDocumentSnapshot,
    previous_left_context: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPersonalKeyResolution {
    None,
    Confirmed,
    Retracted,
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

fn is_candidate_forget_shortcut(vkey: u16, modifiers: KeyModifiers) -> bool {
    vkey == VK_DELETE.0
        && modifiers.control
        && !modifiers.shift
        && !modifiers.alt
        && !modifiers.windows
}

fn candidate_numeric_rank(vkey: u16, modifiers: KeyModifiers) -> Option<usize> {
    ((VK_1.0..=VK_6.0).contains(&vkey)
        && !modifiers.shift
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.windows)
        .then(|| usize::from(vkey - VK_1.0) + 1)
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
        key if key == VK_6.0 && modifiers.shift => Some(CompositionInput::Punctuation(
            CompositionPunctuation::Ellipsis,
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
        (_, CompositionEffect::Continue) if before.tab_mode() || after.tab_mode() => None,
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
        overruled_text_to_remember: None,
        feedback_after_success: None,
        action_after_success: None,
        candidate_forget_action_after_success: None,
    })
}

fn plan_tab_assembly_selection(
    session: &CompositionSession,
    input: &CompositionInput,
    selected_text: Option<String>,
    selected_resolved_code: Option<String>,
) -> Option<PlannedKey> {
    let (source, visible_rank) = match input {
        CompositionInput::Confirm => (NativeSelectionSource::FirstCandidate, 1),
        CompositionInput::Select(rank) => (NativeSelectionSource::Numeric, *rank),
        _ => return None,
    };
    let selected_text = selected_text.filter(|text| !text.is_empty())?;
    let selected_resolved_code = selected_resolved_code.filter(|code| !code.is_empty())?;
    let before = session.clone();
    let mut after = before.clone();
    let selection = after.accept_tab_assembly_candidate(&selected_text, &selected_resolved_code)?;
    let (edit, selection_to_remember, feedback_after_success) = match selection {
        TabAssemblySelection::Advanced => (None, None, None),
        TabAssemblySelection::Complete { text, full_code } => {
            debug_assert_eq!(full_code.len(), text.chars().count().saturating_mul(2));
            let selection = PlannedSelection {
                code: before.phonetic().to_owned(),
                text: text.clone(),
                retractable_by_immediate_backspace: true,
            };
            let feedback = NativeFeedbackEvent::CandidateCommitted {
                code: before.phonetic().to_owned(),
                text: text.clone(),
                view: NativeCandidateView::Shape,
                source,
                absolute_rank: before.candidate_page_start().saturating_add(visible_rank),
                visible_rank,
            };
            (
                Some(PendingDocumentEdit::Commit(text)),
                Some(selection),
                Some(feedback),
            )
        }
    };
    Some(PlannedKey {
        before,
        after,
        edit,
        candidate_display: None,
        selection_to_remember,
        overruled_text_to_remember: None,
        feedback_after_success,
        action_after_success: None,
        candidate_forget_action_after_success: None,
    })
}

fn automatic_transposition_tier(delivered_pair_gap_ms: u64) -> Option<AutomaticTranspositionTier> {
    if delivered_pair_gap_ms <= AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS {
        Some(AutomaticTranspositionTier::Primary)
    } else if delivered_pair_gap_ms < AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS {
        Some(AutomaticTranspositionTier::Secondary)
    } else if delivered_pair_gap_ms < AUTOMATIC_TRANSPOSITION_SHADOW_UPPER_GAP_MS {
        Some(AutomaticTranspositionTier::Shadow)
    } else {
        None
    }
}

fn automatic_transposition_request(
    input: &CompositionInput,
    after: &CompositionSession,
    timing: AutomaticTranspositionTimingEvidence,
) -> Option<AutomaticTranspositionRequest> {
    let CompositionInput::Letters(letters) = input else {
        return None;
    };
    let delivered_pair_gap_ms = timing.current_pair_gap_ms?;
    let tier = automatic_transposition_tier(delivered_pair_gap_ms)?;
    let code_len = after.phonetic().len();
    if letters.len() != 1
        || code_len < 2
        || !code_len.is_multiple_of(2)
        || after.tab_mode()
        || after.recovery_mode()
        || after.wish_prompt()
    {
        return None;
    }
    let syllable_index = code_len / 2 - 1;
    let primary = AutomaticTranspositionAttempt {
        pattern: AutomaticTranspositionPattern::single(syllable_index),
        cold_tier: tier,
        tier,
        pair_gap_ms: u32::try_from(delivered_pair_gap_ms).ok()?,
    };
    let fallback = (code_len == 4 && syllable_index == 1)
        .then_some(timing.previous_pair)
        .flatten()
        .filter(|previous| previous.syllable_index == 0)
        .and_then(|previous| {
            let combined_gap_ms = delivered_pair_gap_ms.max(previous.pair_gap_ms);
            let combined_tier = automatic_transposition_tier(combined_gap_ms)?;
            Some(AutomaticTranspositionAttempt {
                pattern: AutomaticTranspositionPattern::adjacent_pair(0),
                cold_tier: combined_tier,
                tier: combined_tier,
                pair_gap_ms: u32::try_from(combined_gap_ms).ok()?,
            })
        });
    Some(AutomaticTranspositionRequest { primary, fallback })
}

fn completed_pair_timing_after_key(
    after_code_len: usize,
    delivered_letter: bool,
    current_pair_gap_ms: Option<u64>,
    previous_pair: Option<CompletedPairTiming>,
) -> Option<CompletedPairTiming> {
    if !delivered_letter {
        return None;
    }
    if after_code_len >= 2 && after_code_len.is_multiple_of(2) {
        return current_pair_gap_ms.map(|pair_gap_ms| CompletedPairTiming {
            syllable_index: after_code_len / 2 - 1,
            pair_gap_ms,
        });
    }
    previous_pair
        .filter(|previous| after_code_len == previous.syllable_index.saturating_add(1) * 2 + 1)
}

fn plan_immediate_inline_wish(
    session: &CompositionSession,
    operation: InlineWishOperation,
) -> PlannedKey {
    let before = session.clone();
    let mut after = before.clone();
    after.finish_commit();
    PlannedKey {
        before,
        after,
        edit: Some(PendingDocumentEdit::Cancel),
        candidate_display: None,
        selection_to_remember: None,
        overruled_text_to_remember: None,
        feedback_after_success: None,
        action_after_success: Some(PlannedAction::Wish(operation)),
        candidate_forget_action_after_success: None,
    }
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
    personal_phrase_adjacency: PersonalPhraseDocumentAdjacency,
}

struct FinishedDocumentComposition {
    range: Option<ITfRange>,
    personal_phrase_adjacency: PersonalPhraseDocumentAdjacency,
    range_ready: bool,
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
const POPUP_FOOTER_CONTENT_INSET_LOGICAL: i32 = 10;
const POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL: i32 = 640;
const POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL: i32 = 54;
const POPUP_ACTION_MIN_WIDTH_LOGICAL: i32 = 210;
const POPUP_ACTION_DETAIL_GAP_LOGICAL: i32 = 12;
const POPUP_NOTICE_ICON_SIZE_LOGICAL: i32 = 24;
const POPUP_NOTICE_ICON_GAP_LOGICAL: i32 = 7;
const POPUP_CORNER_DIAMETER_LOGICAL: i32 = 16;
const POPUP_BORDER_WIDTH_LOGICAL: i32 = 1;
const POPUP_SELECTED_SURFACE_HEIGHT_LOGICAL: i32 = 28;
const POPUP_SELECTION_ACCENT_WIDTH_LOGICAL: i32 = 3;
const POPUP_SELECTION_ACCENT_FALLBACK_HEIGHT_LOGICAL: i32 = 14;
const POPUP_SELECTION_ACCENT_LEFT_INSET_LOGICAL: i32 = 5;
const POPUP_PERSONAL_MARK_SIZE_LOGICAL: i32 = 3;

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

fn horizontal_candidate_logical_width(candidate: &str, selected: bool) -> i32 {
    let text_width = candidate
        .chars()
        .take(CANDIDATE_DISPLAY_MAX_CHARS)
        .fold(0_i32, |width, character| {
            width.saturating_add(if character.is_ascii() { 9 } else { 18 })
        })
        .clamp(18, 144);
    let leading_inset = if selected {
        POPUP_SELECTED_TEXT_INSET_LOGICAL
    } else {
        POPUP_TEXT_PADDING_LOGICAL
    };
    leading_inset
        .saturating_add(POPUP_RANK_WIDTH_LOGICAL)
        .saturating_add(POPUP_RANK_GAP_LOGICAL)
        .saturating_add(text_width)
        .saturating_add(POPUP_TEXT_PADDING_LOGICAL)
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

fn action_popup_logical_width(display: &CandidateDisplay, label: &str, detail: &str) -> i32 {
    let label_width = estimated_popup_text_width(label, 9, 18);
    let detail_width = estimated_popup_text_width(detail, 7, 14);
    let notice_icon_width = if display.notice_icon() == CandidateNoticeIcon::WishReceived {
        POPUP_NOTICE_ICON_SIZE_LOGICAL.saturating_add(POPUP_NOTICE_ICON_GAP_LOGICAL)
    } else {
        0
    };
    POPUP_OUTER_PADDING_LOGICAL
        .saturating_mul(2)
        .saturating_add(POPUP_SELECTED_TEXT_INSET_LOGICAL)
        .saturating_add(POPUP_RANK_WIDTH_LOGICAL)
        .saturating_add(POPUP_RANK_GAP_LOGICAL)
        .saturating_add(notice_icon_width)
        .saturating_add(label_width)
        .saturating_add(POPUP_ACTION_DETAIL_GAP_LOGICAL)
        .saturating_add(detail_width)
        .saturating_add(POPUP_TEXT_PADDING_LOGICAL)
        .max(POPUP_ACTION_MIN_WIDTH_LOGICAL)
}

fn candidate_popup_mode_label(display: &CandidateDisplay) -> Option<&str> {
    display.mode_label().or_else(|| {
        (display.view() == InteractiveCandidateView::TranspositionRecovery).then_some("换序")
    })
}

fn candidate_popup_mode_logical_width(display: &CandidateDisplay) -> i32 {
    candidate_popup_mode_label(display)
        .map(|label| estimated_popup_text_width(label, 7, 14).saturating_add(32))
        .unwrap_or(0)
}

fn candidate_popup_footer_logical_width(display: &CandidateDisplay) -> i32 {
    let mode_width = candidate_popup_mode_logical_width(display);
    match (mode_width > 0, display.page_starts().len() > 1) {
        (true, true) => mode_width.saturating_add(48),
        (true, false) => mode_width,
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
        .enumerate()
        .map(|(index, candidate)| {
            popup_scale(
                dpi,
                horizontal_candidate_logical_width(candidate, index == 0),
            )
        })
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
            width: popup_scale(dpi, action_popup_logical_width(display, label, detail))
                .min(horizontal_limit),
            height: popup_scale(
                dpi,
                POPUP_OUTER_PADDING_LOGICAL
                    .saturating_mul(2)
                    .saturating_add(POPUP_ROW_HEIGHT_LOGICAL),
            ),
        };
    }
    let footer_needed =
        display.page_starts().len() > 1 || candidate_popup_mode_label(display).is_some();
    let footer_width = popup_scale(dpi, candidate_popup_footer_logical_width(display));
    let outer_width = popup_scale(dpi, POPUP_OUTER_PADDING_LOGICAL.saturating_mul(2));
    let horizontal_content_width =
        display
            .visible()
            .iter()
            .enumerate()
            .fold(outer_width, |width, (index, candidate)| {
                width.saturating_add(popup_scale(
                    dpi,
                    horizontal_candidate_logical_width(candidate, index == 0),
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
                        horizontal_candidate_logical_width(candidate, true)
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
    native_feedback: SyncWeak<Mutex<NativeFeedbackRuntime>>,
    native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    pending_timing: Option<PendingCandidatePopupTiming>,
    corner_strategy: CandidatePopupCornerStrategy,
    transient_notice: bool,
    transient_hidden: bool,
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
        native_feedback: SyncWeak<Mutex<NativeFeedbackRuntime>>,
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
        self.show_inner(owner, anchor, display, feedback_context, false)
    }

    fn show_notice(&mut self, display: &CandidateDisplay) -> Result<()> {
        let (Some(owner), Some(anchor)) = (self.owner, self.anchor) else {
            return Err(lifecycle_error(E_UNEXPECTED));
        };
        self.show_inner(owner, anchor, display, NativeFeedbackContext::Unknown, true)
    }

    fn show_inner(
        &mut self,
        owner: HWND,
        anchor: RECT,
        display: &CandidateDisplay,
        feedback_context: NativeFeedbackContext,
        transient_notice: bool,
    ) -> Result<()> {
        let timing_started_at = Instant::now();
        if let Some(hwnd) = self.hwnd {
            // SAFETY: this controller owns the popup and its fixed timer id.
            let _ = unsafe { KillTimer(Some(hwnd), INLINE_WISH_NOTICE_TIMER_ID) };
        }
        if self.paint.transient_hidden {
            self.visible = false;
            self.paint.transient_hidden = false;
        }
        self.paint.transient_notice = transient_notice;
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
        if let Some(hwnd) = self.hwnd {
            // SAFETY: this controller owns the popup and its fixed timer id.
            let _ = unsafe { KillTimer(Some(hwnd), INLINE_WISH_NOTICE_TIMER_ID) };
        }
        self.paint.transient_notice = false;
        self.paint.transient_hidden = false;
        self.set_visible(false);
    }

    fn destroy(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: this controller owns the popup and its fixed timer id.
            let _ = unsafe { KillTimer(Some(hwnd), INLINE_WISH_NOTICE_TIMER_ID) };
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
        self.paint.transient_notice = false;
        self.paint.transient_hidden = false;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PopupFontMetrics {
    height: i32,
    ascent: i32,
}

fn candidate_label_columns(mut content: RECT, dpi: u32) -> (RECT, RECT) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let mut rank = content;
    rank.right = rank
        .left
        .saturating_add(scale(POPUP_RANK_WIDTH_LOGICAL))
        .min(rank.right);
    content.left = rank.right.saturating_add(scale(POPUP_RANK_GAP_LOGICAL));
    (rank, content)
}

fn baseline_aligned_label_rects(
    content: RECT,
    dpi: u32,
    rank_metrics: PopupFontMetrics,
    text_metrics: PopupFontMetrics,
) -> (RECT, RECT) {
    let (mut rank, mut text) = candidate_label_columns(content, dpi);
    let available_height = content.bottom.saturating_sub(content.top).max(0);
    let text_height = text_metrics.height.clamp(0, available_height);
    text.top = content
        .top
        .saturating_add(available_height.saturating_sub(text_height) / 2);
    text.bottom = text.top.saturating_add(text_height);
    let baseline = text
        .top
        .saturating_add(text_metrics.ascent.min(text_height));
    let rank_height = rank_metrics.height.clamp(0, available_height);
    rank.top = baseline
        .saturating_sub(rank_metrics.ascent.min(rank_height))
        .max(content.top);
    rank.bottom = rank.top.saturating_add(rank_height).min(content.bottom);
    (rank, text)
}

unsafe fn selected_popup_font_metrics(hdc: HDC, font: HFONT) -> Option<PopupFontMetrics> {
    if font.is_invalid() {
        return None;
    }
    // SAFETY: the font and paint DC belong to the active WM_PAINT operation.
    unsafe {
        let _ = SelectObject(hdc, HGDIOBJ(font.0));
    }
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: the output structure is valid for the duration of this call.
    if !unsafe { GetTextMetricsW(hdc, &mut metrics) }.as_bool() {
        return None;
    }
    Some(PopupFontMetrics {
        height: metrics.tmHeight,
        ascent: metrics.tmAscent,
    })
}

fn candidate_personal_mark_rect(rank: RECT, dpi: u32) -> Option<RECT> {
    let size = popup_scale(dpi, POPUP_PERSONAL_MARK_SIZE_LOGICAL).max(2);
    let height = rank.bottom.saturating_sub(rank.top);
    if height < size || rank.right.saturating_sub(rank.left) < size {
        return None;
    }
    let top = rank.top.saturating_add(height.saturating_sub(size) / 2);
    Some(RECT {
        left: rank.left,
        top,
        right: rank.left.saturating_add(size),
        bottom: top.saturating_add(size),
    })
}

unsafe fn paint_candidate_personal_mark(hdc: HDC, rank: RECT, dpi: u32) {
    let Some(mark) = candidate_personal_mark_rect(rank, dpi) else {
        return;
    };
    // A tiny rounded dot lives inside the existing rank column, so personal
    // recall remains recognizable without widening or reflowing candidates.
    let diameter = mark.right.saturating_sub(mark.left).max(1);
    // SAFETY: the region and brush are bounded to the current candidate row
    // and released before returning.
    let region = unsafe {
        CreateRoundRectRgn(
            mark.left,
            mark.top,
            mark.right,
            mark.bottom,
            diameter,
            diameter,
        )
    };
    if region.is_invalid() {
        return;
    }
    // SAFETY: the fixed popup color creates one local GDI brush.
    let brush = unsafe { CreateSolidBrush(popup_color(POPUP_MODE_ACCENT_RGB)) };
    if !brush.is_invalid() {
        // SAFETY: hdc, region, and brush remain valid for this bounded fill.
        unsafe {
            let _ = FillRgn(hdc, region, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }
    // SAFETY: the region is no longer used after this call.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn paint_candidate_label(
    hdc: HDC,
    content: RECT,
    dpi: u32,
    index: usize,
    candidate: &str,
    action_detail: Option<&str>,
    show_rank: bool,
    selected: bool,
    personalized: bool,
    candidate_font: HFONT,
    selected_font: HFONT,
    metadata_font: HFONT,
) {
    let font = if selected {
        selected_font
    } else {
        candidate_font
    };
    let rank_metrics = unsafe { selected_popup_font_metrics(hdc, metadata_font) };
    let text_metrics = unsafe { selected_popup_font_metrics(hdc, font) };
    let baseline_aligned = rank_metrics.is_some() && text_metrics.is_some();
    let original_content = content;
    let (mut rank, mut content) = match (rank_metrics, text_metrics) {
        (Some(rank_metrics), Some(text_metrics)) => {
            baseline_aligned_label_rects(content, dpi, rank_metrics, text_metrics)
        }
        _ => candidate_label_columns(content, dpi),
    };
    if !show_rank {
        content.left = original_content.left;
    }
    if show_rank {
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
                DT_RIGHT
                    | DT_SINGLELINE
                    | if baseline_aligned { DT_TOP } else { DT_VCENTER }
                    | DT_NOPREFIX,
            );
        }
        if personalized {
            // SAFETY: the marker is confined to the already measured rank
            // rectangle and owns every temporary GDI object it creates.
            unsafe {
                paint_candidate_personal_mark(hdc, rank, dpi);
            }
        }
    }

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
                    top: rank.top,
                    right: content.right,
                    bottom: rank.bottom,
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
            DT_SINGLELINE
                | if baseline_aligned { DT_TOP } else { DT_VCENTER }
                | DT_NOPREFIX
                | DT_END_ELLIPSIS,
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
                DT_RIGHT
                    | DT_SINGLELINE
                    | if baseline_aligned { DT_TOP } else { DT_VCENTER }
                    | DT_NOPREFIX,
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

fn candidate_selection_rects(
    item: RECT,
    dpi: u32,
    selected_text_metrics: Option<PopupFontMetrics>,
) -> (RECT, RECT) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let center_vertical = |bounds: RECT, height: i32| {
        let available = bounds.bottom.saturating_sub(bounds.top).max(0);
        let height = height.clamp(0, available);
        let top = bounds
            .top
            .saturating_add(available.saturating_sub(height) / 2);
        (top, top.saturating_add(height))
    };
    let (selected_top, selected_bottom) =
        center_vertical(item, scale(POPUP_SELECTED_SURFACE_HEIGHT_LOGICAL));
    let selected = RECT {
        left: item.left.saturating_add(scale(1)),
        top: selected_top,
        right: item.right.saturating_sub(scale(5)),
        bottom: selected_bottom,
    };
    let accent_height = selected_text_metrics
        .map(|metrics| metrics.height)
        .unwrap_or_else(|| scale(POPUP_SELECTION_ACCENT_FALLBACK_HEIGHT_LOGICAL));
    let (accent_top, accent_bottom) = center_vertical(item, accent_height);
    let accent_left = selected
        .left
        .saturating_add(scale(POPUP_SELECTION_ACCENT_LEFT_INSET_LOGICAL));
    let accent = RECT {
        left: accent_left,
        top: accent_top,
        right: accent_left.saturating_add(scale(POPUP_SELECTION_ACCENT_WIDTH_LOGICAL)),
        bottom: accent_bottom,
    };
    (selected, accent)
}

unsafe fn paint_candidate_selection(
    hdc: HDC,
    item: RECT,
    dpi: u32,
    selected_text_metrics: Option<PopupFontMetrics>,
    selected_background: COLORREF,
    selection_accent: COLORREF,
) {
    let scale = |logical: i32| popup_scale(dpi, logical);
    let (selected, accent) = candidate_selection_rects(item, dpi, selected_text_metrics);
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
    if message == WM_TIMER && wparam.0 == INLINE_WISH_NOTICE_TIMER_ID && !state_pointer.is_null() {
        // SAFETY: the popup owns this fixed timer and remains valid throughout
        // the synchronous window callback.
        let _ = unsafe { KillTimer(Some(hwnd), INLINE_WISH_NOTICE_TIMER_ID) };
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        // SAFETY: the boxed state outlives this window.
        unsafe {
            (*state_pointer).transient_notice = false;
            (*state_pointer).transient_hidden = true;
        }
        return LRESULT(0);
    }
    if message == WM_PAINT && !state_pointer.is_null() {
        // SAFETY: the boxed state outlives this window.
        unsafe { paint_candidate_popup(hwnd, &mut *state_pointer) };
        // Start the short acknowledgement lifetime only after a complete frame
        // has been painted, so a busy host cannot consume the visible interval.
        if unsafe { (*state_pointer).transient_notice } {
            // SAFETY: the popup owns this fixed timer and uses no callback.
            let _ = unsafe {
                SetTimer(
                    Some(hwnd),
                    INLINE_WISH_NOTICE_TIMER_ID,
                    INLINE_WISH_NOTICE_DURATION_MS,
                    None,
                )
            };
        }
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

fn load_inline_wish_notice_icon(dpi: u32) -> Option<HICON> {
    let module = current_module_handle().ok()?;
    let size = popup_scale(dpi, POPUP_NOTICE_ICON_SIZE_LOGICAL);
    let resource = PCWSTR(INLINE_WISH_NOTICE_ICON_RESOURCE_ID as *const u16);
    // SAFETY: the fixed integer resource belongs to this loaded module. The
    // shared icon remains owned by Windows and must not be destroyed here.
    let image = unsafe {
        LoadImageW(
            Some(HINSTANCE(module.0)),
            resource,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR | LR_SHARED,
        )
    }
    .ok()?;
    Some(HICON(image.0))
}

unsafe fn paint_candidate_notice_icon(
    hdc: HDC,
    item: RECT,
    dpi: u32,
    icon: CandidateNoticeIcon,
) -> i32 {
    if icon != CandidateNoticeIcon::WishReceived {
        return 0;
    }
    let Some(icon) = load_inline_wish_notice_icon(dpi) else {
        return 0;
    };
    let size = popup_scale(dpi, POPUP_NOTICE_ICON_SIZE_LOGICAL);
    let gap = popup_scale(dpi, POPUP_NOTICE_ICON_GAP_LOGICAL);
    let left = item
        .left
        .saturating_add(popup_scale(dpi, POPUP_TEXT_PADDING_LOGICAL));
    let available_height = item.bottom.saturating_sub(item.top).max(0);
    let top = item
        .top
        .saturating_add(available_height.saturating_sub(size).max(0) / 2);
    // SAFETY: the shared resource icon and the current paint DC remain valid
    // for this bounded draw call.
    let _ = unsafe { DrawIconEx(hdc, left, top, icon, size, size, 0, None, DI_NORMAL) };
    size.saturating_add(gap)
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
    let selected_text_metrics = unsafe { selected_popup_font_metrics(hdc, selected_font) };

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
                if index == 0 && !state.display.is_notice() {
                    // SAFETY: the selected decoration is bounded to this
                    // candidate item and uses only local GDI objects.
                    unsafe {
                        paint_candidate_selection(
                            hdc,
                            item,
                            state.dpi,
                            selected_text_metrics,
                            selected_background,
                            selection_accent,
                        );
                    }
                }
                let notice_inset = unsafe {
                    paint_candidate_notice_icon(hdc, item, state.dpi, state.display.notice_icon())
                };
                item.left = item
                    .left
                    .saturating_add(if index == 0 && !state.display.is_notice() {
                        scale(POPUP_SELECTED_TEXT_INSET_LOGICAL)
                    } else {
                        text_padding
                    })
                    .saturating_add(notice_inset);
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
                        !state.display.is_notice(),
                        index == 0,
                        state
                            .display
                            .visible_personalized()
                            .get(index)
                            .copied()
                            .unwrap_or(false),
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
                if index == 0 && !state.display.is_notice() {
                    // SAFETY: the selected decoration is bounded to this row
                    // and uses only local GDI objects.
                    unsafe {
                        paint_candidate_selection(
                            hdc,
                            row,
                            state.dpi,
                            selected_text_metrics,
                            selected_background,
                            selection_accent,
                        );
                    }
                }
                let notice_inset = unsafe {
                    paint_candidate_notice_icon(hdc, row, state.dpi, state.display.notice_icon())
                };
                row.left = row
                    .left
                    .saturating_add(if index == 0 && !state.display.is_notice() {
                        scale(POPUP_SELECTED_TEXT_INSET_LOGICAL)
                    } else {
                        text_padding
                    })
                    .saturating_add(notice_inset);
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
                        !state.display.is_notice(),
                        index == 0,
                        state
                            .display
                            .visible_personalized()
                            .get(index)
                            .copied()
                            .unwrap_or(false),
                        candidate_font,
                        selected_font,
                        metadata_font,
                    );
                }
            }
        }
    }

    let pages = state.display.page_starts().len();
    let mode_label = candidate_popup_mode_label(&state.display);
    if pages > 1 || mode_label.is_some() {
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
        if let Some(mode_label) = mode_label {
            let mut mode = footer;
            let page_width = if pages > 1 { scale(48) } else { 0 };
            mode.right = mode.right.saturating_sub(page_width).max(mode.left);
            let mut label = mode_label.encode_utf16().collect::<Vec<_>>();
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
        native_feedback: SyncWeak<Mutex<NativeFeedbackRuntime>>,
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

    fn show_notice(&mut self, display: CandidateDisplay) -> bool {
        if !self.enabled || display.candidates.is_empty() {
            return false;
        }
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.display = Some(display.clone());
            state.shown = true;
        } else {
            return false;
        }
        #[cfg(test)]
        if self.headless {
            return true;
        }
        self.popup
            .try_borrow_mut()
            .map(|mut popup| popup.show_notice(&display).is_ok())
            .unwrap_or(false)
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
struct CompositionSinkShared {
    document_composition: Weak<RefCell<DocumentCompositionState>>,
    personal_phrase_composer: Weak<RefCell<PersonalPhraseComposer>>,
    personal_phrase_document_tracker: Weak<RefCell<PersonalPhraseDocumentTracker>>,
    logical_composition: Weak<RefCell<CompositionSession>>,
    candidate_ui: Weak<RefCell<CandidateUiController>>,
    native_feedback: SyncWeak<Mutex<NativeFeedbackRuntime>>,
    native_feedback_context: SyncWeak<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
}

#[implement(ITfCompositionSink)]
struct TsfCompositionSink {
    document_composition: Weak<RefCell<DocumentCompositionState>>,
    personal_phrase_composer: Weak<RefCell<PersonalPhraseComposer>>,
    personal_phrase_document_tracker: Weak<RefCell<PersonalPhraseDocumentTracker>>,
    logical_composition: Weak<RefCell<CompositionSession>>,
    candidate_ui: Weak<RefCell<CandidateUiController>>,
    native_feedback: SyncWeak<Mutex<NativeFeedbackRuntime>>,
    native_feedback_context: SyncWeak<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Weak<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
}

impl TsfCompositionSink {
    fn counted(shared: CompositionSinkShared) -> Self {
        object_created();
        Self {
            document_composition: shared.document_composition,
            personal_phrase_composer: shared.personal_phrase_composer,
            personal_phrase_document_tracker: shared.personal_phrase_document_tracker,
            logical_composition: shared.logical_composition,
            candidate_ui: shared.candidate_ui,
            native_feedback: shared.native_feedback,
            native_feedback_context: shared.native_feedback_context,
            native_feedback_language_bar_state: shared.native_feedback_language_bar_state,
            key_advice_mode: shared.key_advice_mode,
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
        if let Some(composer) = self.personal_phrase_composer.upgrade()
            && let Ok(mut composer) = composer.try_borrow_mut()
        {
            composer.components.clear();
        }
        if let Some(tracker) = self.personal_phrase_document_tracker.upgrade()
            && let Ok(mut tracker) = tracker.try_borrow_mut()
        {
            tracker.clear();
        }
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
    personal_phrase_composer: Rc<RefCell<PersonalPhraseComposer>>,
    personal_phrase_document_tracker: Rc<RefCell<PersonalPhraseDocumentTracker>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    native_feedback: Arc<Mutex<NativeFeedbackRuntime>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
}

struct DocumentEditRequest {
    action: PendingDocumentEdit,
    candidate_display: Option<CandidateDisplay>,
    feedback_after_success: Option<NativeFeedbackEvent>,
    personal_phrase_commit_text: Option<String>,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
}

#[implement(ITfEditSession)]
struct TsfDocumentEditSession {
    context: ITfContext,
    action: PendingDocumentEdit,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    personal_phrase_composer: Rc<RefCell<PersonalPhraseComposer>>,
    personal_phrase_document_tracker: Rc<RefCell<PersonalPhraseDocumentTracker>>,
    logical_composition: Rc<RefCell<CompositionSession>>,
    telemetry: Arc<Mutex<EditSessionTelemetry>>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    candidate_display: Option<CandidateDisplay>,
    feedback_after_success: Option<NativeFeedbackEvent>,
    personal_phrase_commit_text: Option<String>,
    native_feedback: Arc<Mutex<NativeFeedbackRuntime>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    key_advice_mode: KeyAdviceMode,
    mode: EditSessionMode,
    cleanup_target: Option<ITfComposition>,
}

impl TsfDocumentEditSession {
    fn counted(
        context: ITfContext,
        request: DocumentEditRequest,
        shared: EditSessionShared,
    ) -> Self {
        let DocumentEditRequest {
            action,
            candidate_display,
            feedback_after_success,
            personal_phrase_commit_text,
            mode,
            cleanup_target,
        } = request;
        object_created();
        Self {
            context,
            action,
            document_composition: shared.document_composition,
            personal_phrase_composer: shared.personal_phrase_composer,
            personal_phrase_document_tracker: shared.personal_phrase_document_tracker,
            logical_composition: shared.logical_composition,
            telemetry: shared.telemetry,
            candidate_ui: shared.candidate_ui,
            candidate_display,
            feedback_after_success,
            personal_phrase_commit_text,
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
        let state = self
            .document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        let Some(active) = state.active.as_ref() else {
            return Ok(false);
        };
        if active.context.as_raw() != self.context.as_raw() {
            return Ok(false);
        }
        same_com_identity(&active.composition, target)
    }

    fn start_composition(&self, ec: u32, text: &str) -> Result<()> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let insertion: ITfInsertAtSelection = self.context.cast()?;
        let context_composition: ITfContextComposition = self.context.cast()?;
        let selection_replaced = context_selection_replaces_text(&self.context, ec);
        // SAFETY: `ec` is the read/write cookie issued for this synchronous
        // session. The returned range owns the newly inserted synthetic text.
        let range =
            unsafe { insertion.InsertTextAtSelection(ec, TF_IAS_NO_DEFAULT_COMPOSITION, &utf16) }?;
        let personal_phrase_adjacency = selection_replaced
            .ok()
            .and_then(|selection_replaced| {
                self.personal_phrase_document_tracker
                    .try_borrow()
                    .ok()
                    .map(|tracker| {
                        tracker.observe_composition_start(
                            &self.context,
                            &range,
                            ec,
                            selection_replaced,
                        )
                    })
            })
            .unwrap_or(PersonalPhraseDocumentAdjacency::RangeUnavailable);
        let sink: ITfCompositionSink = TsfCompositionSink::counted(CompositionSinkShared {
            document_composition: Rc::downgrade(&self.document_composition),
            personal_phrase_composer: Rc::downgrade(&self.personal_phrase_composer),
            personal_phrase_document_tracker: Rc::downgrade(&self.personal_phrase_document_tracker),
            logical_composition: Rc::downgrade(&self.logical_composition),
            candidate_ui: Rc::downgrade(&self.candidate_ui),
            native_feedback: Arc::downgrade(&self.native_feedback),
            native_feedback_context: Arc::downgrade(&self.native_feedback_context),
            native_feedback_language_bar_state: Rc::downgrade(
                &self.native_feedback_language_bar_state,
            ),
            key_advice_mode: self.key_advice_mode,
        })
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
            personal_phrase_adjacency,
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

    fn finish_composition(
        &self,
        ec: u32,
        replacement: &str,
    ) -> Result<FinishedDocumentComposition> {
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
        // Keep an independent clone only for a qualifying personal component:
        // EndComposition is allowed to change the composition-owned range. A
        // backward end gravity leaves a later insertion at the committed
        // boundary outside this anchor. Hosts may reject either operation;
        // input still succeeds with an explicit keyboard-continuity fallback.
        let committed_range = self
            .personal_phrase_commit_text
            .as_ref()
            .and_then(|_| unsafe { active.range.Clone() }.ok());
        let range_ready = committed_range.as_ref().is_some_and(|range| {
            unsafe { range.SetGravity(ec, TF_GRAVITY_BACKWARD, TF_GRAVITY_BACKWARD) }.is_ok()
        });
        if let Err(error) = move_selection_after_range(&self.context, &active.range, ec) {
            self.restore_active(active)?;
            return Err(error);
        }
        // SAFETY: successful completion balances StartComposition.
        if let Err(error) = unsafe { active.composition.EndComposition(ec) } {
            self.restore_active(active)?;
            return Err(error);
        }
        Ok(FinishedDocumentComposition {
            range: committed_range,
            personal_phrase_adjacency: active.personal_phrase_adjacency,
            range_ready,
        })
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
        let guarded_cleanup = self.mode != EditSessionMode::KeySynchronous;
        // Input-scope classification also gates personal learning. Keep it
        // available even when the optional wish/feedback recorder is stopped;
        // the classification reads no surrounding text and stores no event.
        let feedback_context_before =
            if !matches!(&self.action, PendingDocumentEdit::UpdatePreedit(_)) {
                if guarded_cleanup && !self.cleanup_target_is_current()? {
                    NativeFeedbackContext::Unknown
                } else {
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
                }
            } else {
                NativeFeedbackContext::Unknown
            };
        let mut cleanup_applied = false;
        let mut finished_commit = None;
        match &self.action {
            PendingDocumentEdit::UpdatePreedit(text) => match self.active_composition()? {
                Some(active) => self.update_composition(ec, &active, text)?,
                None => self.start_composition(ec, text)?,
            },
            PendingDocumentEdit::Cancel if guarded_cleanup => {
                if self.cleanup_target_is_current()? {
                    let _ = self.finish_composition(ec, "")?;
                    cleanup_applied = true;
                }
            }
            PendingDocumentEdit::Cancel => {
                let _ = self.finish_composition(ec, "")?;
            }
            PendingDocumentEdit::Commit(text) => {
                finished_commit = Some(self.finish_composition(ec, text)?);
            }
            PendingDocumentEdit::Insert(text) => self.insert_text_at_selection(ec, text)?,
        }
        if let Some(finished) = finished_commit {
            if let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut() {
                if let Some(text) = self.personal_phrase_commit_text.clone() {
                    tracker.complete_personal_commit(
                        &self.context,
                        finished.range,
                        text,
                        finished.personal_phrase_adjacency,
                        finished.range_ready,
                    );
                } else {
                    tracker.clear();
                }
            }
        } else if (!guarded_cleanup || cleanup_applied)
            && !matches!(&self.action, PendingDocumentEdit::UpdatePreedit(_))
            && let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut()
        {
            tracker.clear();
        }
        let feedback_context = if let PendingDocumentEdit::UpdatePreedit(code) = &self.action {
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
            } else if (!guarded_cleanup || cleanup_applied)
                && let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut()
            {
                candidate_ui.end();
                true
            } else {
                false
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
    CleanupSynchronousHandoff,
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
    feedback: Arc<Mutex<NativeFeedbackRuntime>>,
    feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    input_mode: Rc<Cell<InputMode>>,
    sink: RefCell<Option<ITfLangBarItemSink>>,
    shown: Cell<bool>,
    wish_root: Option<PathBuf>,
    wish_save_status: Cell<WishSaveStatus>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum WishSaveStatus {
    #[default]
    Never,
    Saved {
        events: usize,
    },
    NothingRecent,
    Failed,
}

impl WishSaveStatus {
    fn acknowledgement(self) -> WishCommandAckStatus {
        match self {
            Self::Saved { .. } => WishCommandAckStatus::Applied,
            Self::Never | Self::NothingRecent => WishCommandAckStatus::NoChange,
            Self::Failed => WishCommandAckStatus::Failed,
        }
    }
}

fn freeze_recent_wish_with_context(
    feedback: &NativeFeedbackSession,
    marker_ms: u64,
) -> std::result::Result<Option<FrozenNativeFeedbackSnapshot>, NativeFeedbackFreezeError> {
    let authorization = NativeFeedbackFreezeAuthorization::explicit_private_snapshot();
    let Some(_) = feedback.freeze_recent_episodes(
        authorization,
        marker_ms,
        DEFAULT_NATIVE_FEEDBACK_WISH_EPISODE_MAX_LOOKBACK_MS,
        DEFAULT_NATIVE_FEEDBACK_WISH_EPISODES,
        DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
    )?
    else {
        return Ok(None);
    };
    // The episode probe above guarantees that the package has a meaningful
    // completed focus. Persist the wider bounded context as well:
    // `WishSnapshot` marks the latest completed episode as focus and keeps
    // earlier events as context. This avoids reducing a report to only the few
    // inputs immediately before `xuy`.
    feedback
        .freeze_recent(
            authorization,
            marker_ms,
            DEFAULT_NATIVE_FEEDBACK_WISH_EPISODE_MAX_LOOKBACK_MS,
            DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
        )
        .map(Some)
}

impl NativeFeedbackLanguageBarState {
    fn new(
        feedback: Arc<Mutex<NativeFeedbackRuntime>>,
        feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
        input_mode: Rc<Cell<InputMode>>,
    ) -> Self {
        let wish_root = module_path()
            .ok()
            .and_then(|module| wish_root_for_module(&module));
        Self::with_wish_root(feedback, feedback_context, input_mode, wish_root)
    }

    fn with_wish_root(
        feedback: Arc<Mutex<NativeFeedbackRuntime>>,
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

    fn save_wish(&self, scope: WishCaptureScope, category: WishCategory) -> WishSaveStatus {
        let Some(root) = self.wish_root.as_deref() else {
            self.wish_save_status.set(WishSaveStatus::Failed);
            self.notify();
            return WishSaveStatus::Failed;
        };
        let frozen = self.feedback.lock().map_err(|_| ()).and_then(|feedback| {
            let marker_ms = native_feedback_monotonic_ms();
            let authorization = NativeFeedbackFreezeAuthorization::explicit_private_snapshot();
            let frozen = match scope {
                WishCaptureScope::RecentEpisodes => {
                    match freeze_recent_wish_with_context(&feedback, marker_ms) {
                        Ok(Some(frozen)) => Ok((frozen, WishCaptureScope::RecentEpisodes)),
                        Ok(None) => feedback
                            .freeze_recent(
                                authorization,
                                marker_ms,
                                DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
                                DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
                            )
                            .map(|frozen| (frozen, WishCaptureScope::RecentWindow)),
                        Err(error) => Err(error),
                    }
                }
                WishCaptureScope::LegacyWindow
                | WishCaptureScope::RecentWindow
                | WishCaptureScope::ContinuousJournal => feedback
                    .freeze_recent(
                        authorization,
                        marker_ms,
                        DEFAULT_NATIVE_FEEDBACK_WISH_LOOKBACK_MS,
                        DEFAULT_NATIVE_FEEDBACK_WISH_MAX_EVENTS,
                    )
                    .map(|frozen| (frozen, WishCaptureScope::RecentWindow)),
            };
            match frozen {
                Ok((frozen, _)) if frozen.events().is_empty() => Ok(None),
                Ok((frozen, effective_scope)) => Ok(Some((
                    frozen,
                    effective_scope,
                    feedback.research.runtime_identity(),
                    feedback
                        .research
                        .current_anchor()
                        .map(WishJournalContext::WishAnchor),
                ))),
                Err(NativeFeedbackFreezeError::Disabled)
                | Err(NativeFeedbackFreezeError::NotAccepting) => Ok(None),
                Err(_) => Err(()),
            }
        });
        let status = match frozen {
            Err(()) => WishSaveStatus::Failed,
            Ok(None) => WishSaveStatus::NothingRecent,
            Ok(Some((frozen, effective_scope, runtime_identity, journal_context))) => {
                let snapshot = WishSnapshot::from_frozen_with_context_and_public_order_policy(
                    &frozen,
                    effective_scope,
                    category,
                    runtime_identity,
                    TSF_PUBLIC_CANDIDATE_ORDER_POLICY,
                    journal_context,
                );
                match snapshot {
                    Ok(snapshot) => {
                        let events = snapshot.events().len();
                        match save_wish_snapshot(root, &snapshot, &WindowsUserDataProtector) {
                            Ok(_) => WishSaveStatus::Saved { events },
                            Err(_) => WishSaveStatus::Failed,
                        }
                    }
                    Err(_) => WishSaveStatus::Failed,
                }
            }
        };
        self.wish_save_status.set(status);
        self.notify();
        status
    }

    fn perform_feedback_action(&self, action: u32) -> Result<bool> {
        if action == FEEDBACK_MENU_WISH {
            return Ok(self
                .save_wish(WishCaptureScope::RecentWindow, WishCategory::Other)
                .acknowledgement()
                == WishCommandAckStatus::Applied);
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
                return self
                    .save_wish(WishCaptureScope::RecentWindow, WishCategory::Other)
                    .acknowledgement();
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

    fn perform_inline_wish_operation(
        &self,
        operation: InlineWishOperation,
    ) -> WishCommandAckStatus {
        match operation {
            InlineWishOperation::Command(command) => self.perform_wish_command(command),
            InlineWishOperation::Capture { scope, category } => {
                self.save_wish(scope, category).acknowledgement()
            }
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
            WishSaveStatus::NothingRecent => items.push((
                FEEDBACK_MENU_STATUS + 4,
                TF_LBMENUF_GRAYED,
                "最近没有可保存的输入法事件".to_owned(),
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
        "许愿保存重点现场；持续研究由独立设置控制；不联网".to_owned(),
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
        | NativeFeedbackEvent::CandidatesPresentedWithProvenance { code, .. }
        | NativeFeedbackEvent::CandidateCommitted { code, .. }
        | NativeFeedbackEvent::RawCodeCommitted { code }
        | NativeFeedbackEvent::CompositionCancelled { code, .. }
        | NativeFeedbackEvent::CandidateSuppressionChanged { code, .. } => Some(code),
        NativeFeedbackEvent::CandidatePopupTiming { .. }
        | NativeFeedbackEvent::SlowKeyPathTiming { .. }
        | NativeFeedbackEvent::PostCommitBackspaceRouted
        | NativeFeedbackEvent::PersonalPhraseAdjacencyObserved { .. } => None,
    }
}

fn elapsed_milliseconds(started_at: Instant) -> u32 {
    started_at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

fn slow_key_path_timing_event(
    refresh_ms: u32,
    planning_ms: u32,
    edit_session_ms: u32,
    total_ms: u32,
) -> Option<NativeFeedbackEvent> {
    (total_ms >= SLOW_KEY_PATH_THRESHOLD_MS).then_some(NativeFeedbackEvent::SlowKeyPathTiming {
        refresh_ms,
        planning_ms,
        edit_session_ms,
        total_ms,
    })
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

fn provider_verifies_personal_character_composition(
    provider: &dyn CandidateProvider,
    full_code: &str,
    text: &str,
) -> bool {
    if !(4..=MAX_TAB_ASSEMBLY_CHARACTERS.saturating_mul(2)).contains(&full_code.len())
        || !full_code.len().is_multiple_of(2)
        || !full_code.as_bytes().iter().all(u8::is_ascii_lowercase)
    {
        return false;
    }
    let mut characters = text.chars();
    for start in (0..full_code.len()).step_by(2) {
        let Some(character) = characters.next() else {
            return false;
        };
        let pinyin = &full_code[start..start + 2];
        if !provider.is_exact_full_code_candidate(pinyin, &character.to_string()) {
            return false;
        }
    }
    characters.next().is_none()
}

enum BackgroundPersistenceCommand {
    Research {
        root: PathBuf,
        snapshot: WishSnapshot,
    },
    PersonalRanking {
        root: PathBuf,
        batch: PersonalRankingBatch,
    },
    Barrier(mpsc::Sender<()>),
    Shutdown,
}

#[derive(Default)]
struct BackgroundPersistenceHealth {
    research_failed: AtomicBool,
    personal_ranking_failed: AtomicBool,
    rejected_research_jobs: AtomicU64,
    rejected_personal_ranking_jobs: AtomicU64,
}

#[derive(Clone)]
struct BackgroundPersistenceHandle {
    sender: SyncSender<BackgroundPersistenceCommand>,
    health: Arc<BackgroundPersistenceHealth>,
}

impl BackgroundPersistenceHandle {
    fn enqueue_research(&self, root: PathBuf, snapshot: WishSnapshot) -> bool {
        if self.health.research_failed.load(Ordering::Acquire) {
            return false;
        }
        match self
            .sender
            .try_send(BackgroundPersistenceCommand::Research { root, snapshot })
        {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.health
                    .rejected_research_jobs
                    .fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.health.research_failed.store(true, Ordering::Release);
                false
            }
        }
    }

    fn enqueue_personal_ranking(&self, root: PathBuf, batch: PersonalRankingBatch) -> bool {
        if self.health.personal_ranking_failed.load(Ordering::Acquire) {
            return false;
        }
        match self
            .sender
            .try_send(BackgroundPersistenceCommand::PersonalRanking { root, batch })
        {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.health
                    .rejected_personal_ranking_jobs
                    .fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.health
                    .personal_ranking_failed
                    .store(true, Ordering::Release);
                false
            }
        }
    }

    fn wait_for_personal_ranking_idle(&self) -> bool {
        let (acknowledge, acknowledged) = mpsc::channel();
        if self
            .sender
            .send(BackgroundPersistenceCommand::Barrier(acknowledge))
            .is_err()
            || acknowledged.recv().is_err()
        {
            return false;
        }
        !self.health.personal_ranking_failed.load(Ordering::Acquire)
    }
}

struct BackgroundPersistence {
    sender: Option<SyncSender<BackgroundPersistenceCommand>>,
    worker: Option<JoinHandle<()>>,
    health: Arc<BackgroundPersistenceHealth>,
}

impl BackgroundPersistence {
    fn start() -> Self {
        let (sender, receiver) = mpsc::sync_channel(BACKGROUND_PERSISTENCE_QUEUE_CAPACITY);
        let health = Arc::new(BackgroundPersistenceHealth::default());
        let worker_health = Arc::clone(&health);
        let worker = std::thread::Builder::new()
            .name("ziranma-persistence".to_owned())
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    match command {
                        BackgroundPersistenceCommand::Research { root, snapshot } => {
                            if worker_health.research_failed.load(Ordering::Acquire) {
                                continue;
                            }
                            if research_feedback_enabled(&root) == Ok(true)
                                && save_wish_snapshot(&root, &snapshot, &WindowsUserDataProtector)
                                    .is_err()
                            {
                                worker_health.research_failed.store(true, Ordering::Release);
                            }
                        }
                        BackgroundPersistenceCommand::PersonalRanking { root, batch } => {
                            if worker_health
                                .personal_ranking_failed
                                .load(Ordering::Acquire)
                            {
                                continue;
                            }
                            if save_personal_ranking_batch(&root, &batch, &WindowsUserDataProtector)
                                .is_err()
                            {
                                worker_health
                                    .personal_ranking_failed
                                    .store(true, Ordering::Release);
                            }
                        }
                        BackgroundPersistenceCommand::Barrier(acknowledge) => {
                            let _ = acknowledge.send(());
                        }
                        BackgroundPersistenceCommand::Shutdown => break,
                    }
                }
            })
            .ok();
        if worker.is_none() {
            health.research_failed.store(true, Ordering::Release);
            health
                .personal_ranking_failed
                .store(true, Ordering::Release);
        }
        Self {
            sender: Some(sender),
            worker,
            health,
        }
    }

    fn handle(&self) -> BackgroundPersistenceHandle {
        BackgroundPersistenceHandle {
            sender: self
                .sender
                .as_ref()
                .expect("active background-persistence sender")
                .clone(),
            health: Arc::clone(&self.health),
        }
    }

    fn shutdown(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(BackgroundPersistenceCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for BackgroundPersistence {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct PersonalRankingRuntime {
    root: Option<PathBuf>,
    suppression_root: Option<PathBuf>,
    persisted: LoadedPersonalRanking,
    persisted_suppressions: LoadedPersonalRankingSuppressions,
    snapshot: PersonalRankingSnapshot,
    suppressions: PersonalRankingSuppressionSnapshot,
    unflushed: Vec<PersonalRankingSelection>,
    next_sequence: u64,
    next_suppression_sequence: u64,
    persistence: Option<BackgroundPersistenceHandle>,
}

impl PersonalRankingRuntime {
    #[cfg(test)]
    fn new(root: Option<PathBuf>) -> Self {
        let suppression_root = root
            .as_ref()
            .and_then(|root| root.parent())
            .map(|parent| parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY));
        Self::new_with_roots(root, suppression_root)
    }

    #[cfg(test)]
    fn new_with_roots(root: Option<PathBuf>, suppression_root: Option<PathBuf>) -> Self {
        Self::new_with_roots_and_persistence(root, suppression_root, None)
    }

    fn new_with_roots_and_persistence(
        root: Option<PathBuf>,
        suppression_root: Option<PathBuf>,
        persistence: Option<BackgroundPersistenceHandle>,
    ) -> Self {
        let Some(root) = root else {
            return Self::memory_only();
        };
        let loaded_ranking = load_personal_ranking(&root, &WindowsUserDataProtector);
        let loaded_suppressions = suppression_root
            .as_ref()
            .map(|root| load_personal_ranking_suppressions(root, &WindowsUserDataProtector))
            .transpose();
        match (loaded_ranking, loaded_suppressions) {
            (Ok(loaded), Ok(loaded_suppressions)) => {
                let loaded_suppressions = loaded_suppressions.unwrap_or_default();
                let checkpoint_base = loaded.checkpoint_batch_count();
                let checkpoint_due = loaded.batch_count()
                    >= crate::MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES
                    && (checkpoint_base == 0
                        || loaded.batch_count() >= checkpoint_base.saturating_mul(2));
                if checkpoint_due {
                    let _ =
                        save_personal_ranking_checkpoint(&root, &loaded, &WindowsUserDataProtector);
                }
                let next_sequence = u64::try_from(loaded.batch_count()).unwrap_or(u64::MAX);
                let next_suppression_sequence =
                    u64::try_from(loaded_suppressions.action_count()).unwrap_or(u64::MAX);
                let snapshot = loaded.snapshot().clone();
                let suppressions = loaded_suppressions.snapshot().clone();
                Self {
                    root: Some(root),
                    suppression_root,
                    persisted: loaded,
                    persisted_suppressions: loaded_suppressions,
                    next_sequence,
                    snapshot,
                    suppressions,
                    unflushed: Vec::new(),
                    next_suppression_sequence,
                    persistence,
                }
            }
            _ => Self::memory_only(),
        }
    }

    fn memory_only() -> Self {
        Self {
            root: None,
            suppression_root: None,
            persisted: LoadedPersonalRanking::default(),
            persisted_suppressions: LoadedPersonalRankingSuppressions::default(),
            snapshot: PersonalRankingSnapshot::default(),
            suppressions: PersonalRankingSuppressionSnapshot::default(),
            unflushed: Vec::new(),
            next_sequence: 0,
            next_suppression_sequence: 0,
            persistence: None,
        }
    }

    fn refresh(&mut self) -> bool {
        if self
            .persistence
            .as_ref()
            .is_some_and(|persistence| !persistence.wait_for_personal_ranking_idle())
        {
            return false;
        }
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        let Ok(loaded) = refresh_personal_ranking(root, &WindowsUserDataProtector, &self.persisted)
        else {
            return false;
        };
        let loaded_suppressions = match self.suppression_root.as_ref() {
            Some(root) => refresh_personal_ranking_suppressions(
                root,
                &WindowsUserDataProtector,
                &self.persisted_suppressions,
            ),
            None => Ok(self.persisted_suppressions.clone()),
        };
        let Ok(loaded_suppressions) = loaded_suppressions else {
            return false;
        };
        let mut snapshot = loaded.snapshot().clone();
        for selection in &self.unflushed {
            if snapshot.record(selection.code(), selection.text()).is_err() {
                return false;
            }
        }
        self.persisted = loaded;
        self.persisted_suppressions = loaded_suppressions;
        self.snapshot = snapshot;
        self.suppressions = self.persisted_suppressions.snapshot().clone();
        self.next_suppression_sequence = self
            .next_suppression_sequence
            .max(u64::try_from(self.persisted_suppressions.action_count()).unwrap_or(u64::MAX));
        true
    }

    fn record(&mut self, code: &str, text: &str) -> bool {
        let Ok(selection) = PersonalRankingSelection::new(code, text) else {
            return false;
        };
        if self.snapshot.record(code, text).is_err() {
            return false;
        }
        if self.root.is_none() {
            return true;
        }
        if self.unflushed.len() == crate::MAX_PERSONAL_RANKING_BATCH_EVENTS {
            self.unflushed.remove(0);
        }
        self.unflushed.push(selection);
        if self.unflushed.len() >= PERSONAL_RANKING_FLUSH_SELECTIONS {
            let _ = self.flush();
        }
        true
    }

    #[cfg(test)]
    fn promote_texts_after(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> bool {
        self.promote_texts_after_decision(code, candidates, protected_prefix)
            .is_some()
    }

    fn promote_texts_after_decision(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> Option<CandidateTextPromotion> {
        self.snapshot
            .promote_texts_after_with_suppressions_decision(
                code,
                candidates,
                protected_prefix,
                &self.suppressions,
            )
    }

    fn promote_anchored_suffix_texts_after_decision(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> Option<CandidateTextPromotion> {
        self.snapshot
            .promote_anchored_suffix_texts_after_with_suppressions_decision(
                code,
                candidates,
                protected_prefix,
                &self.suppressions,
                |source_code, text| provider.is_exact_full_code_candidate(source_code, text),
            )
    }

    fn promote_or_recall_verified_anchored_suffix_text_after_decision(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> Option<CandidateTextPromotion> {
        self.snapshot
            .promote_or_recall_verified_anchored_suffix_text_after_with_suppressions_decision(
                code,
                candidates,
                protected_prefix,
                &self.suppressions,
                |source_code, text| provider.is_exact_full_code_candidate(source_code, text),
            )
    }

    fn is_suppressed(&self, code: &str, text: &str) -> bool {
        self.suppressions.is_suppressed(code, text)
    }

    fn has_evidence(&self, code: &str, text: &str) -> bool {
        self.snapshot.has_evidence(code, text)
    }

    fn has_anchored_suffix_evidence(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        text: &str,
    ) -> bool {
        self.snapshot
            .has_anchored_suffix_evidence_with_suppressions(
                code,
                text,
                &self.suppressions,
                |source_code, text| provider.is_exact_full_code_candidate(source_code, text),
            )
    }

    fn recall_repeated_anchored_suffix_text_after_decision(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> Option<CandidateTextPromotion> {
        self.snapshot
            .recall_repeated_anchored_suffix_text_after_with_suppressions_decision(
                code,
                candidates,
                protected_prefix,
                &self.suppressions,
                |source_code, text| {
                    provider_verifies_personal_character_composition(provider, source_code, text)
                },
            )
    }

    fn has_repeated_anchored_suffix_evidence(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        text: &str,
    ) -> bool {
        self.snapshot
            .has_repeated_anchored_suffix_evidence_with_suppressions(
                code,
                text,
                &self.suppressions,
                |source_code, text| {
                    provider_verifies_personal_character_composition(provider, source_code, text)
                },
            )
    }

    fn append_suppression_action(
        &mut self,
        kind: PersonalRankingSuppressionActionKind,
        code: &str,
        text: &str,
    ) -> bool {
        let already_in_requested_state = match kind {
            PersonalRankingSuppressionActionKind::Suppress => {
                self.suppressions.is_suppressed(code, text)
            }
            PersonalRankingSuppressionActionKind::Restore => {
                !self.suppressions.is_suppressed(code, text)
            }
        };
        if already_in_requested_state {
            return false;
        }
        let mut updated_suppressions = self.suppressions.clone();
        let updated = match kind {
            PersonalRankingSuppressionActionKind::Suppress => {
                updated_suppressions.suppress(code, text)
            }
            PersonalRankingSuppressionActionKind::Restore => {
                updated_suppressions.restore(code, text)
            }
        };
        if !matches!(updated, Ok(true)) {
            return false;
        }
        let Ok(action) = PersonalRankingSuppressionAction::now(
            std::process::id(),
            self.next_suppression_sequence,
            kind,
            code,
            text,
        ) else {
            return false;
        };
        if let Some(root) = self.suppression_root.as_ref()
            && save_personal_ranking_suppression_action(root, &action, &WindowsUserDataProtector)
                .is_err()
        {
            return false;
        }
        self.suppressions = updated_suppressions;
        self.next_suppression_sequence = self.next_suppression_sequence.saturating_add(1);
        true
    }

    fn selected_is_preferred(&self, code: &str, text: &str) -> bool {
        self.snapshot
            .preferred_text_with_suppressions(code, &self.suppressions)
            == Some(text)
    }

    fn flush(&mut self) -> bool {
        if self.unflushed.is_empty() {
            return true;
        }
        let Some(root) = self.root.as_ref() else {
            self.unflushed.clear();
            return true;
        };
        let Ok(batch) = PersonalRankingBatch::now(
            std::process::id(),
            self.next_sequence,
            self.unflushed.clone(),
        ) else {
            return false;
        };
        let saved = match self.persistence.as_ref() {
            Some(persistence) => persistence.enqueue_personal_ranking(root.clone(), batch),
            None => save_personal_ranking_batch(root, &batch, &WindowsUserDataProtector).is_ok(),
        };
        if !saved {
            return false;
        }
        self.unflushed.clear();
        self.next_sequence = self.next_sequence.saturating_add(1);
        true
    }
}

#[implement(ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink)]
struct TsfTextService {
    activation: Mutex<ActivationState>,
    background_persistence: Option<BackgroundPersistence>,
    composition: Rc<RefCell<CompositionSession>>,
    document_composition: Rc<RefCell<DocumentCompositionState>>,
    candidate_provider: Option<Arc<dyn CandidateProvider>>,
    candidate_cache: RefCell<CandidateCache>,
    selection_memory: RefCell<SessionSelectionMemory>,
    pending_personal_selection: RefCell<Option<PendingPersonalSelection>>,
    personal_phrase_composer: Rc<RefCell<PersonalPhraseComposer>>,
    personal_phrase_document_tracker: Rc<RefCell<PersonalPhraseDocumentTracker>>,
    personal_context_ranking: RefCell<PersonalContextRanking>,
    personal_left_context: RefCell<Option<String>>,
    personal_ranking: RefCell<PersonalRankingRuntime>,
    candidate_forget_state: RefCell<CandidateForgetState>,
    candidate_ui: Rc<RefCell<CandidateUiController>>,
    edit_telemetry: Arc<Mutex<EditSessionTelemetry>>,
    native_feedback: Arc<Mutex<NativeFeedbackRuntime>>,
    native_feedback_context: Arc<Mutex<NativeFeedbackContextCache>>,
    native_feedback_language_bar_state: Rc<NativeFeedbackLanguageBarState>,
    native_feedback_language_bar: RefCell<NativeFeedbackLanguageBarController>,
    native_wish_commands: RefCell<NativeWishCommandController>,
    input_mode: Rc<Cell<InputMode>>,
    shift_tap_armed: Cell<bool>,
    shift_chord_pending: Cell<bool>,
    last_delivered_letter: Cell<Option<DeliveredLetterAnchor>>,
    last_completed_pair_timing: Cell<Option<CompletedPairTiming>>,
    key_advice_mode: KeyAdviceMode,
    #[cfg(test)]
    synthetic_key_modifiers: Cell<KeyModifiers>,
}

#[derive(Clone, Copy)]
struct DeliveredLetterAnchor {
    at: Instant,
    code_len_after: usize,
}

const RESEARCH_FEEDBACK_BATCH_EVENTS: usize = 256;
const RESEARCH_FEEDBACK_BATCH_EPISODES: usize = 8;
const RESEARCH_FEEDBACK_BATCH_MAX_SPAN_MS: u64 = 60_000;
const RESEARCH_FEEDBACK_CONSENT_POLL_MS: u64 = 1_000;

struct ResearchFeedbackJournal {
    root: Option<PathBuf>,
    persistence: Option<BackgroundPersistenceHandle>,
    module_sha256: Option<String>,
    candidate_identity: Option<CandidateDataIdentity>,
    stream_id: String,
    batch_sequence: u64,
    next_event_ordinal: u64,
    first_event_ordinal: Option<u64>,
    first_event_timestamp_ms: Option<u64>,
    previous_event_timestamp_ms: Option<u64>,
    enabled: bool,
    last_consent_check_ms: Option<u64>,
    events: Vec<(u64, NativeFeedbackEvent)>,
    completed_episodes: usize,
}

impl ResearchFeedbackJournal {
    fn for_mode(
        key_advice_mode: KeyAdviceMode,
        candidate_identity: Option<CandidateDataIdentity>,
        persistence: Option<BackgroundPersistenceHandle>,
    ) -> Self {
        let root = matches!(key_advice_mode, KeyAdviceMode::Foreground)
            .then(|| {
                module_path()
                    .ok()
                    .and_then(|module| research_feedback_root_for_module(&module))
            })
            .flatten();
        let enabled = root
            .as_deref()
            .is_some_and(|root| research_feedback_enabled(root).unwrap_or(false));
        let module_sha256 = matches!(key_advice_mode, KeyAdviceMode::Foreground)
            .then(|| {
                module_path()
                    .ok()
                    .as_deref()
                    .and_then(immutable_module_sha256)
            })
            .flatten();
        Self {
            root,
            persistence,
            module_sha256,
            candidate_identity,
            stream_id: new_research_stream_id(),
            batch_sequence: 0,
            next_event_ordinal: 0,
            first_event_ordinal: None,
            first_event_timestamp_ms: None,
            previous_event_timestamp_ms: None,
            enabled,
            last_consent_check_ms: None,
            events: Vec::new(),
            completed_episodes: 0,
        }
    }

    #[cfg(test)]
    fn with_root(root: Option<PathBuf>) -> Self {
        let enabled = root
            .as_deref()
            .is_some_and(|root| research_feedback_enabled(root).unwrap_or(false));
        Self {
            root,
            persistence: None,
            module_sha256: None,
            candidate_identity: None,
            stream_id: new_research_stream_id(),
            batch_sequence: 0,
            next_event_ordinal: 0,
            first_event_ordinal: None,
            first_event_timestamp_ms: None,
            previous_event_timestamp_ms: None,
            enabled,
            last_consent_check_ms: None,
            events: Vec::new(),
            completed_episodes: 0,
        }
    }

    fn can_refresh(&self) -> bool {
        self.root.is_some()
    }

    fn update_candidate_identity(&mut self, candidate_identity: Option<CandidateDataIdentity>) {
        if self.candidate_identity == candidate_identity {
            return;
        }
        // A continuous-journal batch has one runtime identity. Close any
        // buffered batch before applying a newly loaded candidate revision so
        // later events cannot be mislabeled with the prior supplement.
        let _ = self.flush();
        self.candidate_identity = candidate_identity;
    }

    fn refresh_consent(&mut self, monotonic_ms: u64) -> bool {
        let due = self.last_consent_check_ms.is_none_or(|previous| {
            monotonic_ms < previous
                || monotonic_ms.saturating_sub(previous) >= RESEARCH_FEEDBACK_CONSENT_POLL_MS
        });
        if !due {
            return self.enabled;
        }
        self.last_consent_check_ms = Some(monotonic_ms);
        let was_enabled = self.enabled;
        self.enabled = self
            .root
            .as_deref()
            .is_some_and(|root| research_feedback_enabled(root).unwrap_or(false));
        if !self.enabled {
            self.events.clear();
            self.completed_episodes = 0;
            self.reset_stream();
        } else if !was_enabled {
            self.reset_stream();
        }
        self.enabled
    }

    fn record(&mut self, event: NativeFeedbackEvent, monotonic_ms: u64) {
        let completes_episode = event.completes_input_episode();
        if completes_episode {
            if !self.refresh_consent(monotonic_ms) {
                return;
            }
        } else if !self.enabled {
            return;
        }
        if self.events.is_empty()
            && self
                .previous_event_timestamp_ms
                .is_some_and(|previous| monotonic_ms < previous)
        {
            self.reset_stream();
        }
        if self.next_event_ordinal == u64::MAX {
            let _ = self.flush();
            self.reset_stream();
        }
        if self.events.is_empty() {
            self.first_event_ordinal = Some(self.next_event_ordinal);
            self.first_event_timestamp_ms = Some(monotonic_ms);
        }
        self.events.push((monotonic_ms, event));
        self.next_event_ordinal = self.next_event_ordinal.saturating_add(1);
        if completes_episode {
            self.completed_episodes = self.completed_episodes.saturating_add(1);
        }
        let span_ms = self
            .events
            .first()
            .map(|(first, _)| monotonic_ms.saturating_sub(*first))
            .unwrap_or(0);
        if self.events.len() >= RESEARCH_FEEDBACK_BATCH_EVENTS
            || (completes_episode
                && (self.completed_episodes >= RESEARCH_FEEDBACK_BATCH_EPISODES
                    || span_ms >= RESEARCH_FEEDBACK_BATCH_MAX_SPAN_MS))
        {
            let _ = self.flush();
        }
    }

    fn flush(&mut self) -> bool {
        if self.events.is_empty() {
            return true;
        }
        let Some(root) = self.root.as_deref() else {
            self.events.clear();
            self.completed_episodes = 0;
            return false;
        };
        if self.persistence.is_none() && research_feedback_enabled(root) != Ok(true) {
            self.enabled = false;
            self.events.clear();
            self.completed_episodes = 0;
            return false;
        }
        let Some(marker_ms) = self.events.last().map(|(timestamp, _)| *timestamp) else {
            return true;
        };
        let Some(first_event_ordinal) = self.first_event_ordinal else {
            return false;
        };
        let Some(first_event_timestamp_ms) = self.first_event_timestamp_ms else {
            return false;
        };
        let previous_event_gap_ms = self
            .previous_event_timestamp_ms
            .and_then(|previous| first_event_timestamp_ms.checked_sub(previous));
        let journal_context = WishJournalSpan::new(
            self.stream_id.clone(),
            self.batch_sequence,
            first_event_ordinal,
            previous_event_gap_ms,
        )
        .ok()
        .map(WishJournalContext::ContinuousSpan);
        let runtime_identity = self.runtime_identity();
        let snapshot = match journal_context {
            Some(journal_context) => {
                crate::FrozenNativeFeedbackSnapshot::from_journal_events(marker_ms, &self.events)
                    .ok()
                    .map(|frozen| (frozen, journal_context))
            }
            None => None,
        }
        .and_then(|(frozen, journal_context)| {
            WishSnapshot::from_frozen_with_context_and_public_order_policy(
                &frozen,
                WishCaptureScope::ContinuousJournal,
                WishCategory::Other,
                runtime_identity,
                TSF_PUBLIC_CANDIDATE_ORDER_POLICY,
                Some(journal_context),
            )
            .ok()
        });
        let accepted = snapshot.is_some_and(|snapshot| match self.persistence.as_ref() {
            Some(persistence) => persistence.enqueue_research(root.to_path_buf(), snapshot),
            None => {
                research_feedback_enabled(root) == Ok(true)
                    && save_wish_snapshot(root, &snapshot, &WindowsUserDataProtector).is_ok()
            }
        });
        if accepted {
            self.events.clear();
            self.completed_episodes = 0;
            self.first_event_ordinal = None;
            self.first_event_timestamp_ms = None;
            self.previous_event_timestamp_ms = Some(marker_ms);
            self.batch_sequence = self.batch_sequence.saturating_add(1);
        } else {
            // A malformed marker or saturated/unavailable writer must never
            // retain an unbounded private buffer inside the host. The writer
            // records aggregate route health without putting content in it.
            self.root = None;
            self.enabled = false;
            self.events.clear();
            self.completed_episodes = 0;
            self.reset_stream();
        }
        accepted
    }

    fn runtime_identity(&self) -> Option<WishRuntimeIdentity> {
        let candidates = self.candidate_identity.as_ref()?;
        WishRuntimeIdentity::new(
            self.module_sha256.clone()?,
            candidates.core_revision.clone(),
            candidates.supplemental_revision.clone(),
        )
        .ok()
    }

    fn current_anchor(&self) -> Option<WishJournalAnchor> {
        self.enabled.then_some(())?;
        WishJournalAnchor::new(
            self.stream_id.clone(),
            self.next_event_ordinal.checked_sub(1)?,
        )
        .ok()
    }

    fn reset_stream(&mut self) {
        self.stream_id = new_research_stream_id();
        self.batch_sequence = 0;
        self.next_event_ordinal = 0;
        self.first_event_ordinal = None;
        self.first_event_timestamp_ms = None;
        self.previous_event_timestamp_ms = None;
    }
}

fn new_research_stream_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut seed = Vec::with_capacity(40);
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    seed.extend_from_slice(
        &NEXT
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    candidate_sha256_hex(&seed)
}

struct NativeFeedbackRuntime {
    session: NativeFeedbackSession,
    research: ResearchFeedbackJournal,
}

impl NativeFeedbackRuntime {
    fn for_mode(
        key_advice_mode: KeyAdviceMode,
        candidate_identity: Option<CandidateDataIdentity>,
        persistence: Option<BackgroundPersistenceHandle>,
    ) -> Self {
        let mut session = NativeFeedbackSession::default();
        if matches!(key_advice_mode, KeyAdviceMode::Foreground) {
            let started = session.start_rolling_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits::default(),
            );
            debug_assert_eq!(started, NativeFeedbackStartResult::Started);
        }
        Self {
            session,
            research: ResearchFeedbackJournal::for_mode(
                key_advice_mode,
                candidate_identity,
                persistence,
            ),
        }
    }

    #[cfg(test)]
    fn memory_only() -> Self {
        Self {
            session: NativeFeedbackSession::default(),
            research: ResearchFeedbackJournal::with_root(None),
        }
    }

    #[cfg(test)]
    fn with_research_root(root: PathBuf) -> Self {
        Self {
            session: NativeFeedbackSession::default(),
            research: ResearchFeedbackJournal::with_root(Some(root)),
        }
    }

    fn is_accepting(&self) -> bool {
        self.session.is_accepting() || self.research.can_refresh()
    }

    fn update_candidate_identity(&mut self, candidate_identity: Option<CandidateDataIdentity>) {
        self.research.update_candidate_identity(candidate_identity);
    }

    fn record_at(
        &mut self,
        context: NativeFeedbackContext,
        event: NativeFeedbackEvent,
        monotonic_ms: u64,
    ) -> NativeFeedbackRecordResult {
        let research_event = (context == NativeFeedbackContext::Eligible
            && event.validate_and_measure().is_some())
        .then(|| event.clone());
        let result = self.session.record_at(context, event, monotonic_ms);
        if let Some(event) = research_event {
            self.research.record(event, monotonic_ms);
        }
        result
    }

    fn flush_research(&mut self) -> bool {
        self.research.flush()
    }
}

impl Deref for NativeFeedbackRuntime {
    type Target = NativeFeedbackSession;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl DerefMut for NativeFeedbackRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}

impl Drop for NativeFeedbackRuntime {
    fn drop(&mut self) {
        let _ = self.flush_research();
    }
}

fn native_feedback_runtime_for_mode(
    key_advice_mode: KeyAdviceMode,
    candidate_identity: Option<CandidateDataIdentity>,
    persistence: Option<BackgroundPersistenceHandle>,
) -> NativeFeedbackRuntime {
    NativeFeedbackRuntime::for_mode(key_advice_mode, candidate_identity, persistence)
}

impl TsfTextService {
    fn counted_with_options(
        candidate_provider: Option<Arc<dyn CandidateProvider>>,
        key_advice_mode: KeyAdviceMode,
    ) -> Self {
        object_created();
        let candidate_identity = candidate_provider
            .as_deref()
            .and_then(CandidateProvider::candidate_data_identity);
        let background_persistence =
            matches!(key_advice_mode, KeyAdviceMode::Foreground).then(BackgroundPersistence::start);
        let persistence = background_persistence
            .as_ref()
            .map(BackgroundPersistence::handle);
        let native_feedback = Arc::new(Mutex::new(native_feedback_runtime_for_mode(
            key_advice_mode,
            candidate_identity,
            persistence.clone(),
        )));
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
        let personal_ranking_roots = matches!(key_advice_mode, KeyAdviceMode::Foreground)
            .then(|| {
                module_path().ok().map(|module| {
                    (
                        personal_ranking_root_for_module(&module),
                        personal_ranking_suppression_root_for_module(&module),
                    )
                })
            })
            .flatten()
            .unwrap_or_default();
        Self {
            activation: Mutex::new(ActivationState::default()),
            background_persistence,
            composition: Rc::new(RefCell::new(CompositionSession::default())),
            document_composition: Rc::new(RefCell::new(DocumentCompositionState::default())),
            candidate_provider,
            candidate_cache: RefCell::new(CandidateCache::default()),
            selection_memory: RefCell::new(SessionSelectionMemory::default()),
            pending_personal_selection: RefCell::new(None),
            personal_phrase_composer: Rc::new(RefCell::new(PersonalPhraseComposer::default())),
            personal_phrase_document_tracker: Rc::new(RefCell::new(
                PersonalPhraseDocumentTracker::default(),
            )),
            personal_context_ranking: RefCell::new(PersonalContextRanking::default()),
            personal_left_context: RefCell::new(None),
            personal_ranking: RefCell::new(PersonalRankingRuntime::new_with_roots_and_persistence(
                personal_ranking_roots.0,
                personal_ranking_roots.1,
                persistence,
            )),
            candidate_forget_state: RefCell::new(CandidateForgetState::Inactive),
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
            last_delivered_letter: Cell::new(None),
            last_completed_pair_timing: Cell::new(None),
            key_advice_mode,
            #[cfg(test)]
            synthetic_key_modifiers: Cell::new(KeyModifiers::default()),
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
        if let Some(pending) = self.pending_personal_selection.get_mut().take() {
            let ranking = self.personal_ranking.get_mut();
            let _ = ranking.record(&pending.selection.code, &pending.selection.text);
            if let Some(phrase) = pending.phrase {
                let _ = ranking.record(&phrase.selection.code, &phrase.selection.text);
            }
        }
        let _ = self.personal_ranking.get_mut().flush();
        if let Ok(mut feedback) = self.native_feedback.lock() {
            let _ = feedback.flush_research();
        }
        if let Some(persistence) = self.background_persistence.as_mut() {
            persistence.shutdown();
        }
        object_dropped();
    }
}

impl TsfTextService_Impl {
    fn observed_key_modifiers(&self) -> KeyModifiers {
        match self.key_advice_mode {
            KeyAdviceMode::Foreground => current_key_modifiers(),
            KeyAdviceMode::SyntheticHost => {
                #[cfg(test)]
                {
                    self.synthetic_key_modifiers.get()
                }
                #[cfg(not(test))]
                {
                    KeyModifiers::default()
                }
            }
        }
    }

    fn delivered_letter_timing(
        &self,
        wparam: WPARAM,
        modifiers: KeyModifiers,
    ) -> (Option<u64>, Option<DeliveredLetterAnchor>) {
        let Some(CompositionInput::Letters(letters)) = u16::try_from(wparam.0)
            .ok()
            .and_then(|vkey| decode_virtual_key(vkey, modifiers, self.input_mode.get()))
        else {
            return (None, None);
        };
        if letters.len() != 1 {
            return (None, None);
        }
        let Ok(composition) = self.composition.try_borrow() else {
            return (None, None);
        };
        let code_len = composition.phonetic().len();
        let now = Instant::now();
        let pair_gap_ms = self.last_delivered_letter.get().and_then(|anchor| {
            (code_len % 2 == 1 && anchor.code_len_after == code_len).then(|| {
                u64::try_from(now.saturating_duration_since(anchor.at).as_millis())
                    .unwrap_or(u64::MAX)
            })
        });
        (
            pair_gap_ms,
            Some(DeliveredLetterAnchor {
                at: now,
                code_len_after: code_len.saturating_add(1),
            }),
        )
    }

    fn load_candidate_batch(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
    ) -> Result<CandidateBatch> {
        self.load_candidate_batch_with_automatic_transposition(provider, code, limit, view, None)
    }

    fn load_shape_candidate_batch(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        stroke_prefix: &str,
        limit: usize,
    ) -> CandidateBatch {
        let resolved = provider.shape_candidates(code, stroke_prefix, limit);
        let candidates = resolved
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect::<Vec<_>>();
        let resolved_shape_codes = resolved
            .into_iter()
            .map(|candidate| Some(candidate.resolved_code))
            .collect::<Vec<_>>();
        CandidateBatch {
            provenance: vec![
                NativeCandidateProvenance::new(NativeCandidateSource::Shape, false);
                candidates.len()
            ],
            personalized: vec![false; candidates.len()],
            resolved_shape_codes,
            protected_prefix_len: 0,
            automatic_transposition: None,
            may_have_more: candidates.len() == limit && limit < CANDIDATE_LIMIT,
            candidates,
            view: InteractiveCandidateView::Primary,
        }
    }

    fn load_candidate_batch_with_automatic_transposition(
        &self,
        provider: &dyn CandidateProvider,
        code: &str,
        limit: usize,
        view: InteractiveCandidateView,
        automatic_transposition_request: Option<AutomaticTranspositionRequest>,
    ) -> Result<CandidateBatch> {
        let left_context = self
            .personal_left_context
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .clone();
        let contextual_search = view == InteractiveCandidateView::Primary
            && automatic_transposition_request.is_none()
            && left_context.as_deref().is_some_and(|previous| {
                let Ok(personal_ranking) = self.personal_ranking.try_borrow() else {
                    return false;
                };
                self.personal_context_ranking
                    .try_borrow()
                    .map(|ranking| {
                        ranking.has_eligible_preference(previous, code, |text| {
                            !personal_ranking.is_suppressed(code, text)
                                && personal_ranking.has_evidence(code, text)
                        })
                    })
                    .unwrap_or(false)
            });
        let load_limit = if contextual_search {
            limit.max(PERSONAL_CONTEXT_SEARCH_DEPTH)
        } else {
            limit
        };
        let mut batch = self
            .candidate_cache
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .load_with_automatic_transposition(
                provider,
                code,
                load_limit,
                view,
                automatic_transposition_request,
            );
        if view == InteractiveCandidateView::Primary {
            let protected_prefix = batch.protected_prefix_len.min(batch.candidates.len());
            let personal_ranking = self
                .personal_ranking
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            let selection_memory = self
                .selection_memory
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            let session_exact_text = selection_memory
                .remembered_text(code)
                .filter(|text| !personal_ranking.is_suppressed(code, text));
            let persistent_exact_promotion = personal_ranking.promote_texts_after_decision(
                code,
                &mut batch.candidates,
                protected_prefix,
            );
            if let Some(promotion) = persistent_exact_promotion {
                mirror_candidate_promotion(
                    &mut batch,
                    promotion,
                    NativeCandidatePersonalization::PERSISTENT_EXACT,
                );
            }
            let persistent_anchored_promotion = if persistent_exact_promotion.is_none()
                && session_exact_text.is_none()
            {
                if automatic_transposition_request.is_none() {
                    personal_ranking.promote_or_recall_verified_anchored_suffix_text_after_decision(
                        provider,
                        code,
                        &mut batch.candidates,
                        protected_prefix,
                    )
                } else {
                    personal_ranking.promote_anchored_suffix_texts_after_decision(
                        provider,
                        code,
                        &mut batch.candidates,
                        protected_prefix,
                    )
                }
            } else {
                None
            };
            if let Some(promotion) = persistent_anchored_promotion {
                mirror_candidate_promotion(
                    &mut batch,
                    promotion,
                    if promotion.source_index.is_none() {
                        NativeCandidatePersonalization::PERSISTENT_DISCOVERY
                    } else {
                        NativeCandidatePersonalization::PERSISTENT_ANCHORED
                    },
                );
            }
            let personal_discovery_promotion = if persistent_exact_promotion.is_none()
                && persistent_anchored_promotion.is_none()
                && session_exact_text.is_none()
                && automatic_transposition_request.is_none()
            {
                personal_ranking.recall_repeated_anchored_suffix_text_after_decision(
                    provider,
                    code,
                    &mut batch.candidates,
                    protected_prefix,
                )
            } else {
                None
            };
            if let Some(promotion) = personal_discovery_promotion {
                mirror_candidate_promotion(
                    &mut batch,
                    promotion,
                    NativeCandidatePersonalization::PERSISTENT_DISCOVERY,
                );
            }
            let persistent_anchored_discovered = persistent_anchored_promotion
                .is_some_and(|promotion| promotion.source_index.is_none());
            let session_anchored_promotion = if persistent_exact_promotion.is_none()
                && session_exact_text.is_none()
                && !persistent_anchored_discovered
            {
                selection_memory.promote_anchored_suffix_texts_after_decision(
                    code,
                    &mut batch.candidates,
                    protected_prefix,
                    |source_code, text| {
                        !personal_ranking.is_suppressed(code, text)
                            && !personal_ranking.is_suppressed(source_code, text)
                            && provider.is_exact_full_code_candidate(source_code, text)
                    },
                )
            } else {
                None
            };
            if let Some(promotion) = session_anchored_promotion {
                mirror_candidate_promotion(
                    &mut batch,
                    promotion,
                    NativeCandidatePersonalization::SESSION_ANCHORED,
                );
            }
            let session_exact_promotion = session_exact_text.and_then(|_| {
                selection_memory.promote_texts_after_decision(
                    code,
                    &mut batch.candidates,
                    protected_prefix,
                )
            });
            if let Some(promotion) = session_exact_promotion {
                mirror_candidate_promotion(
                    &mut batch,
                    promotion,
                    NativeCandidatePersonalization::SESSION_EXACT,
                );
            }
            if automatic_transposition_request.is_none()
                && let Some(previous) = left_context.as_deref()
            {
                let context_promotion = self
                    .personal_context_ranking
                    .try_borrow()
                    .map_err(|_| lifecycle_error(E_UNEXPECTED))?
                    .promote_existing_text_after_decision(
                        previous,
                        code,
                        &mut batch.candidates,
                        protected_prefix,
                        |text| {
                            !personal_ranking.is_suppressed(code, text)
                                && personal_ranking.has_evidence(code, text)
                        },
                    );
                if let Some(promotion) = context_promotion {
                    mirror_candidate_promotion(
                        &mut batch,
                        promotion,
                        if promotion.changed {
                            NativeCandidatePersonalization::LEFT_CONTEXT
                        } else {
                            NativeCandidatePersonalization::NONE
                        },
                    );
                }
            }
        }
        if batch.candidates.len() > limit {
            batch.may_have_more = true;
            batch.candidates.truncate(limit);
            batch.provenance.truncate(batch.candidates.len());
            batch.personalized.truncate(batch.candidates.len());
        }
        batch.protected_prefix_len = batch.protected_prefix_len.min(batch.candidates.len());
        Ok(batch)
    }

    fn restore_session_selection(
        &self,
        selection: &PlannedSelection,
        previous_session_text: Option<&str>,
    ) {
        let previous_is_suppressed = previous_session_text.is_some_and(|previous| {
            self.personal_ranking
                .try_borrow()
                .map(|ranking| ranking.is_suppressed(&selection.code, previous))
                .unwrap_or(true)
        });
        if let Ok(mut memory) = self.selection_memory.try_borrow_mut()
            && memory.forget_text(&selection.code, &selection.text)
            && let Some(previous) = previous_session_text
            && !previous_is_suppressed
        {
            memory.remember_text(&selection.code, previous);
        }
    }

    fn restore_session_selection_after_pending(&self, pending: &PendingPersonalSelection) {
        self.restore_session_selection(
            &pending.selection,
            pending.previous_session_text.as_deref(),
        );
    }

    fn clear_personal_phrase_components(&self) {
        if let Ok(mut composer) = self.personal_phrase_composer.try_borrow_mut() {
            composer.components.clear();
        }
    }

    fn clear_personal_phrase_composer(&self) {
        self.clear_personal_phrase_components();
        if let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut() {
            tracker.clear();
        }
    }

    fn personal_phrase_document_snapshot(&self) -> PersonalPhraseDocumentSnapshot {
        self.personal_phrase_document_tracker
            .try_borrow()
            .map(|tracker| tracker.snapshot())
            .unwrap_or_default()
    }

    fn take_personal_phrase_document_adjacency(&self) -> PersonalPhraseDocumentAdjacency {
        let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut() else {
            return PersonalPhraseDocumentAdjacency::RangeUnavailable;
        };
        let adjacency = tracker.take_completed_adjacency().unwrap_or_else(|| {
            tracker.mark_range_fallback_after_commit();
            PersonalPhraseDocumentAdjacency::KeyboardFallback
        });
        tracker.last_consumed_adjacency = Some(adjacency);
        adjacency
    }

    fn clear_personal_left_context(&self) {
        if let Ok(mut context) = self.personal_left_context.try_borrow_mut() {
            *context = None;
        }
    }

    fn set_personal_left_context(&self, text: &str) {
        if let Ok(mut context) = self.personal_left_context.try_borrow_mut() {
            *context = PersonalContextRanking::accepts_left_context(text).then(|| text.to_owned());
        }
    }

    fn personal_phrase_component(
        &self,
        selection: &PlannedSelection,
        learning_context: NativeFeedbackContext,
    ) -> Option<PersonalPhraseComponent> {
        (learning_context == NativeFeedbackContext::Eligible
            && selection.retractable_by_immediate_backspace
            && selection.code.len() == 2
            && selection.code.as_bytes().iter().all(u8::is_ascii_lowercase)
            && selection.text.chars().count() == 1
            && self.candidate_provider.as_ref().is_some_and(|provider| {
                provider.is_exact_full_code_candidate(&selection.code, &selection.text)
            }))
        .then(|| PersonalPhraseComponent {
            code: selection.code.clone(),
            text: selection.text.clone(),
        })
    }

    fn pending_personal_phrase(
        &self,
        components: &[PersonalPhraseComponent],
    ) -> PendingPersonalPhrase {
        debug_assert!((2..=MAX_PERSONAL_PHRASE_COMPONENTS).contains(&components.len()));
        let code: String = components
            .iter()
            .map(|component| component.code.as_str())
            .collect();
        let text: String = components
            .iter()
            .map(|component| component.text.as_str())
            .collect();
        let previous_session_text = self
            .selection_memory
            .try_borrow()
            .ok()
            .and_then(|memory| memory.remembered_text(&code).map(str::to_owned));
        PendingPersonalPhrase {
            selection: PlannedSelection {
                code,
                text,
                retractable_by_immediate_backspace: true,
            },
            previous_session_text,
        }
    }

    fn confirm_pending_personal_selection(&self) -> bool {
        let pending = match self.pending_personal_selection.try_borrow_mut() {
            Ok(mut slot) => slot.take(),
            Err(_) => return false,
        };
        let Some(pending) = pending else {
            return true;
        };
        let preferred = self
            .personal_ranking
            .try_borrow_mut()
            .ok()
            .and_then(|mut ranking| {
                if !ranking.record(&pending.selection.code, &pending.selection.text) {
                    return None;
                }
                let selection =
                    ranking.selected_is_preferred(&pending.selection.code, &pending.selection.text);
                let phrase = pending.phrase.as_ref().map(|phrase| {
                    ranking.record(&phrase.selection.code, &phrase.selection.text)
                        && ranking
                            .selected_is_preferred(&phrase.selection.code, &phrase.selection.text)
                });
                Some((selection, phrase))
            });
        let Some((selection_is_preferred, phrase_is_preferred)) = preferred else {
            if let Ok(mut slot) = self.pending_personal_selection.try_borrow_mut()
                && slot.is_none()
            {
                *slot = Some(pending);
            }
            return false;
        };
        if let Some(previous) = pending.previous_left_context.as_deref() {
            let context_allowed = self
                .personal_ranking
                .try_borrow()
                .map(|ranking| {
                    ranking.has_evidence(&pending.selection.code, &pending.selection.text)
                        && !ranking.is_suppressed(&pending.selection.code, &pending.selection.text)
                })
                .unwrap_or(false);
            if context_allowed
                && let Ok(mut context) = self.personal_context_ranking.try_borrow_mut()
            {
                let _ = context.record_choice(
                    previous,
                    &pending.selection.code,
                    &pending.selection.text,
                    pending.overruled_text.as_deref(),
                );
            }
        }
        if !selection_is_preferred {
            self.restore_session_selection_after_pending(&pending);
        }
        if phrase_is_preferred == Some(false)
            && let Some(phrase) = pending.phrase.as_ref()
        {
            self.restore_session_selection(
                &phrase.selection,
                phrase.previous_session_text.as_deref(),
            );
        }
        true
    }

    fn retract_pending_personal_selection(&self) -> bool {
        let pending = match self.pending_personal_selection.try_borrow_mut() {
            Ok(mut slot) => slot.take(),
            Err(_) => return false,
        };
        let Some(pending) = pending else {
            return false;
        };
        if let Some(phrase) = pending.phrase.as_ref() {
            self.restore_session_selection(
                &phrase.selection,
                phrase.previous_session_text.as_deref(),
            );
        }
        self.restore_session_selection_after_pending(&pending);
        if let Ok(mut composer) = self.personal_phrase_composer.try_borrow_mut() {
            composer.components = pending.previous_phrase_components;
        }
        if let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut() {
            tracker.restore(pending.previous_phrase_document);
        }
        if let Ok(mut context) = self.personal_left_context.try_borrow_mut() {
            *context = pending.previous_left_context;
        }
        true
    }

    fn should_route_pending_personal_key_down(&self) -> Result<bool> {
        let has_pending = self
            .pending_personal_selection
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .is_some();
        Ok(has_pending && !self.has_active_logical_composition()?)
    }

    fn resolve_pending_personal_selection_for_key(
        &self,
        vkey: u16,
        modifiers: KeyModifiers,
    ) -> Result<PendingPersonalKeyResolution> {
        let retractable = {
            let pending = self
                .pending_personal_selection
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            let Some(pending) = pending.as_ref() else {
                return Ok(PendingPersonalKeyResolution::None);
            };
            pending.selection.retractable_by_immediate_backspace
        };
        let immediate_plain_backspace = vkey == VK_BACK.0
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.windows
            && !self.has_active_logical_composition()?;
        if retractable && immediate_plain_backspace {
            return Ok(if self.retract_pending_personal_selection() {
                PendingPersonalKeyResolution::Retracted
            } else {
                PendingPersonalKeyResolution::None
            });
        }
        if self.confirm_pending_personal_selection() {
            Ok(PendingPersonalKeyResolution::Confirmed)
        } else {
            Ok(PendingPersonalKeyResolution::None)
        }
    }

    fn record_post_commit_backspace_routed(&self) {
        let Ok(mut feedback) = self.native_feedback.lock() else {
            return;
        };
        if !feedback.is_accepting() {
            return;
        }
        let result = feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::PostCommitBackspaceRouted,
            native_feedback_monotonic_ms(),
        );
        drop(feedback);
        if matches!(result, NativeFeedbackRecordResult::Stopped(_)) {
            self.native_feedback_language_bar_state.notify();
        }
    }

    fn record_personal_phrase_adjacency_observed(
        &self,
        observation: PersonalPhraseAdjacencyObservation,
    ) {
        let Ok(mut feedback) = self.native_feedback.lock() else {
            return;
        };
        if !feedback.is_accepting() {
            return;
        }
        let result = feedback.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::PersonalPhraseAdjacencyObserved {
                adjacency: observation.adjacency.feedback_value(),
                previous_components: observation.previous_components,
                resulting_components: observation.resulting_components,
            },
            native_feedback_monotonic_ms(),
        );
        drop(feedback);
        if matches!(result, NativeFeedbackRecordResult::Stopped(_)) {
            self.native_feedback_language_bar_state.notify();
        }
    }

    #[cfg(test)]
    fn remember_selection_after_success(&self, selection: PlannedSelection) {
        let learning_context = self
            .native_feedback_context
            .lock()
            .map(|cache| cache.context_for(&selection.code))
            .unwrap_or(NativeFeedbackContext::Unknown);
        self.remember_selection_after_success_in_context(selection, learning_context);
    }

    #[cfg(test)]
    fn remember_selection_after_success_in_context(
        &self,
        selection: PlannedSelection,
        learning_context: NativeFeedbackContext,
    ) {
        self.remember_selection_after_success_in_context_with_overrule(
            selection,
            learning_context,
            None,
        );
    }

    #[cfg(test)]
    fn remember_selection_after_success_in_context_with_overrule(
        &self,
        selection: PlannedSelection,
        learning_context: NativeFeedbackContext,
        overruled_text: Option<String>,
    ) {
        let previous_phrase_document = self.personal_phrase_document_snapshot();
        let _ = self.remember_selection_after_success_in_context_with_overrule_and_document(
            selection,
            learning_context,
            overruled_text,
            PersonalPhraseDocumentAdjacency::KeyboardFallback,
            previous_phrase_document,
        );
    }

    fn remember_selection_after_success_in_context_with_overrule_and_document(
        &self,
        selection: PlannedSelection,
        learning_context: NativeFeedbackContext,
        overruled_text: Option<String>,
        document_adjacency: PersonalPhraseDocumentAdjacency,
        previous_phrase_document: PersonalPhraseDocumentSnapshot,
    ) -> Option<PersonalPhraseAdjacencyObservation> {
        // A second successful selection is an explicit boundary for the
        // preceding transaction, even if an unusual host skipped the key that
        // began the new composition.
        let _ = self.confirm_pending_personal_selection();
        let previous_left_context = (learning_context == NativeFeedbackContext::Eligible)
            .then(|| {
                self.personal_left_context
                    .try_borrow()
                    .ok()
                    .and_then(|context| context.clone())
            })
            .flatten();
        let previous_session_text = self
            .selection_memory
            .try_borrow()
            .ok()
            .and_then(|memory| memory.remembered_text(&selection.code).map(str::to_owned));
        let selection_is_suppressed = self
            .personal_ranking
            .try_borrow()
            .map(|ranking| ranking.is_suppressed(&selection.code, &selection.text))
            .unwrap_or(true);
        if !selection_is_suppressed && let Ok(mut memory) = self.selection_memory.try_borrow_mut() {
            memory.remember_text(&selection.code, &selection.text);
        }
        let component = self.personal_phrase_component(&selection, learning_context);
        let component_is_eligible = component.is_some();
        let observed_previous_components = self
            .personal_phrase_composer
            .try_borrow()
            .ok()
            .map_or(0, |composer| composer.components.len());
        if !document_adjacency.allows_continuation() {
            self.clear_personal_phrase_components();
        }
        let previous_phrase_components = self
            .personal_phrase_composer
            .try_borrow()
            .ok()
            .map(|composer| composer.components.clone())
            .unwrap_or_default();
        let (next_phrase_components, phrase) = match component {
            Some(component)
                if previous_phrase_components.len() < MAX_PERSONAL_PHRASE_COMPONENTS =>
            {
                let mut next = previous_phrase_components.clone();
                next.push(component);
                let phrase = (next.len() >= 2).then(|| self.pending_personal_phrase(&next));
                (next, phrase)
            }
            Some(component) => (vec![component], None),
            None => (Vec::new(), None),
        };
        let resulting_components = next_phrase_components.len();
        if next_phrase_components.is_empty()
            && let Ok(mut tracker) = self.personal_phrase_document_tracker.try_borrow_mut()
        {
            tracker.clear();
        }
        if let Ok(mut composer) = self.personal_phrase_composer.try_borrow_mut() {
            composer.components = next_phrase_components;
        }
        if let Some(phrase) = phrase.as_ref() {
            let phrase_is_suppressed = self
                .personal_ranking
                .try_borrow()
                .map(|ranking| {
                    ranking.is_suppressed(&phrase.selection.code, &phrase.selection.text)
                })
                .unwrap_or(true);
            if !phrase_is_suppressed && let Ok(mut memory) = self.selection_memory.try_borrow_mut()
            {
                memory.remember_text(&phrase.selection.code, &phrase.selection.text);
            }
        }
        if learning_context == NativeFeedbackContext::Eligible
            && let Ok(mut slot) = self.pending_personal_selection.try_borrow_mut()
            && slot.is_none()
        {
            *slot = Some(PendingPersonalSelection {
                selection,
                overruled_text,
                previous_session_text,
                phrase,
                previous_phrase_components,
                previous_phrase_document,
                previous_left_context,
            });
        }
        component_is_eligible.then_some(PersonalPhraseAdjacencyObservation {
            adjacency: document_adjacency,
            previous_components: observed_previous_components.min(MAX_PERSONAL_PHRASE_COMPONENTS),
            resulting_components,
        })
    }

    fn has_active_logical_composition(&self) -> Result<bool> {
        Ok(!self
            .composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .phonetic()
            .is_empty())
    }

    fn key_continues_personal_phrase(&self, vkey: u16, modifiers: KeyModifiers) -> bool {
        self.input_mode.get() == InputMode::Chinese
            && decode_virtual_key(vkey, modifiers, InputMode::Chinese)
                .is_some_and(|input| matches!(input, CompositionInput::Letters(_)))
    }

    fn should_route_candidate_forget_key(
        &self,
        vkey: u16,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        if is_candidate_forget_shortcut(vkey, modifiers) {
            return Ok(true);
        }
        if !self.has_active_logical_composition()? {
            if let Ok(mut state) = self.candidate_forget_state.try_borrow_mut() {
                *state = CandidateForgetState::Inactive;
            }
            return Ok(false);
        }
        let state = self
            .candidate_forget_state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        Ok(match &*state {
            CandidateForgetState::Inactive => false,
            CandidateForgetState::Choosing(_) => {
                !modifiers.control && !modifiers.alt && !modifiers.windows
            }
            CandidateForgetState::UndoAvailable { .. } => {
                let plain_escape_or_backspace = !modifiers.shift
                    && !modifiers.control
                    && !modifiers.alt
                    && !modifiers.windows
                    && (vkey == VK_ESCAPE.0 || vkey == VK_BACK.0);
                plain_escape_or_backspace
                    || decode_virtual_key(vkey, modifiers, self.input_mode.get()).is_some()
            }
        })
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
        if let Ok(mut state) = self.candidate_forget_state.try_borrow_mut() {
            *state = CandidateForgetState::Inactive;
        }
        self.commit_active_composition(context)?;
        let _ = self.confirm_pending_personal_selection();
        self.clear_personal_phrase_composer();
        self.clear_personal_left_context();
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
        let mode = request.mode;
        let edit_session: ITfEditSession = TsfDocumentEditSession::counted(
            context.clone(),
            request,
            EditSessionShared {
                document_composition: Rc::clone(&self.document_composition),
                personal_phrase_composer: Rc::clone(&self.personal_phrase_composer),
                personal_phrase_document_tracker: Rc::clone(&self.personal_phrase_document_tracker),
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
        )
        .into();
        let scheduling = match mode {
            EditSessionMode::KeySynchronous | EditSessionMode::CleanupSynchronousHandoff => {
                TF_ES_SYNC
            }
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
                personal_phrase_commit_text: None,
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

    fn pending_focus_cleanup_routes_letter(
        &self,
        vkey: u16,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        if self.input_mode.get() != InputMode::Chinese
            || self.has_active_logical_composition()?
            || !decode_virtual_key(vkey, modifiers, InputMode::Chinese)
                .is_some_and(|input| matches!(input, CompositionInput::Letters(_)))
        {
            return Ok(false);
        }
        let state = self
            .document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        Ok(state.cleanup_scheduled && state.active.is_some())
    }

    fn complete_pending_focus_cleanup_before_letter(&self) -> Result<bool> {
        let Some(client_id) = self.active_client_id()? else {
            return Ok(false);
        };
        let pending = {
            let state = self
                .document_composition
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            if !state.cleanup_scheduled {
                return Ok(true);
            }
            state
                .active
                .as_ref()
                .map(|active| (active.context.clone(), active.composition.clone()))
        };
        let Some((context, cleanup_target)) = pending else {
            return Ok(false);
        };

        // OnTestKeyDown routes only a plain Chinese letter into this callback.
        // Try to finish the exact old composition synchronously before making
        // a new one. If the old Context refuses the request, OnKeyDown returns
        // FALSE and the host still receives the original key.
        let _ = self.request_document_edit_session(
            &context,
            client_id,
            DocumentEditRequest {
                action: PendingDocumentEdit::Cancel,
                candidate_display: None,
                feedback_after_success: None,
                personal_phrase_commit_text: None,
                mode: EditSessionMode::CleanupSynchronousHandoff,
                cleanup_target: Some(cleanup_target),
            },
        );

        let state = self
            .document_composition
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        Ok(!state.cleanup_scheduled && state.active.is_none())
    }

    fn cleanup_after_focus_loss(&self) -> Result<()> {
        self.shift_tap_armed.set(false);
        self.shift_chord_pending.set(false);
        self.last_delivered_letter.set(None);
        self.last_completed_pair_timing.set(None);
        if let Ok(mut state) = self.candidate_forget_state.try_borrow_mut() {
            *state = CandidateForgetState::Inactive;
        }
        let _ = self.confirm_pending_personal_selection();
        self.clear_personal_phrase_composer();
        self.clear_personal_left_context();
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
        if let Ok(mut ranking) = self.personal_ranking.try_borrow_mut() {
            let _ = ranking.refresh();
        }
        Ok(())
    }

    fn candidate_forget_display(
        &self,
        provider: &dyn CandidateProvider,
        session: &CompositionSession,
        limit: usize,
        mode: CandidateDisplayMode,
    ) -> Result<Option<CandidateDisplay>> {
        if session.phonetic().is_empty()
            || session.tab_mode()
            || session.recovery_mode()
            || session.wish_prompt()
        {
            return Ok(None);
        }
        let batch = self.load_candidate_batch(
            provider,
            session.phonetic(),
            limit,
            InteractiveCandidateView::Primary,
        )?;
        if batch.candidates.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            CandidateDisplay::from_batch(batch, session.candidate_page_start()).with_mode(mode),
        ))
    }

    fn plan_candidate_forget_enter(
        &self,
        provider: &dyn CandidateProvider,
        session: &CompositionSession,
    ) -> Result<Option<PlannedKey>> {
        if self.input_mode.get() != InputMode::Chinese
            || session.phonetic().is_empty()
            || session.tab_mode()
            || session.recovery_mode()
            || session.wish_prompt()
        {
            return Ok(None);
        }
        let display = self.candidate_forget_display(
            provider,
            session,
            candidate_visible_limit(session.candidate_page_start()),
            CandidateDisplayMode::ForgetSelecting,
        )?;
        Ok(display.map(|display| {
            plan_candidate_forget_ui(session, Some(display), PlannedCandidateForgetAction::Enter)
        }))
    }

    fn plan_candidate_forget_choosing(
        &self,
        provider: &dyn CandidateProvider,
        session: &CompositionSession,
        message: CandidateForgetMessage,
        vkey: u16,
        modifiers: KeyModifiers,
    ) -> Result<Option<PlannedKey>> {
        if modifiers.control || modifiers.alt || modifiers.windows {
            return Ok(None);
        }
        if (vkey == VK_ESCAPE.0 || vkey == VK_BACK.0) && !modifiers.shift {
            let display = self.candidate_forget_display(
                provider,
                session,
                candidate_visible_limit(session.candidate_page_start()),
                CandidateDisplayMode::Normal,
            )?;
            return Ok(Some(plan_candidate_forget_ui(
                session,
                display,
                PlannedCandidateForgetAction::Cancel,
            )));
        }

        if let Some(rank) = candidate_numeric_rank(vkey, modifiers) {
            let batch = self.load_candidate_batch(
                provider,
                session.phonetic(),
                candidate_visible_limit(session.candidate_page_start()),
                InteractiveCandidateView::Primary,
            )?;
            let absolute = session
                .candidate_page_start()
                .saturating_add(rank.saturating_sub(1));
            let Some(text) = batch.candidates.get(absolute).cloned() else {
                let display = CandidateDisplay::from_batch(batch, session.candidate_page_start())
                    .with_mode(CandidateDisplayMode::ForgetSelecting);
                return Ok(Some(plan_candidate_forget_ui(
                    session,
                    Some(display),
                    PlannedCandidateForgetAction::Message(CandidateForgetMessage::Select),
                )));
            };
            let protected_prefix = batch.protected_prefix_len.min(batch.candidates.len());
            if absolute < protected_prefix {
                let display = CandidateDisplay::from_batch(batch, session.candidate_page_start())
                    .with_mode(CandidateDisplayMode::ForgetProtected);
                return Ok(Some(plan_candidate_forget_ui(
                    session,
                    Some(display),
                    PlannedCandidateForgetAction::Message(CandidateForgetMessage::Protected),
                )));
            }
            let ranking = self
                .personal_ranking
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            let suppressed = ranking.is_suppressed(session.phonetic(), &text);
            let has_persistent_evidence = ranking.has_evidence(session.phonetic(), &text)
                || ranking.has_anchored_suffix_evidence(provider, session.phonetic(), &text)
                || ranking.has_repeated_anchored_suffix_evidence(
                    provider,
                    session.phonetic(),
                    &text,
                );
            let selection_memory = self
                .selection_memory
                .try_borrow()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
            let has_exact_session_evidence =
                selection_memory.remembered_text(session.phonetic()) == Some(text.as_str());
            let has_session_evidence = has_exact_session_evidence
                || selection_memory.has_anchored_suffix_evidence(
                    session.phonetic(),
                    &text,
                    |source_code, text| {
                        !ranking.is_suppressed(session.phonetic(), text)
                            && !ranking.is_suppressed(source_code, text)
                            && provider.is_exact_full_code_candidate(source_code, text)
                    },
                );
            if suppressed || (!has_persistent_evidence && !has_session_evidence) {
                let display = CandidateDisplay::from_batch(batch, session.candidate_page_start())
                    .with_mode(CandidateDisplayMode::ForgetNotPersonal);
                return Ok(Some(plan_candidate_forget_ui(
                    session,
                    Some(display),
                    PlannedCandidateForgetAction::Message(CandidateForgetMessage::NotPersonal),
                )));
            }
            return Ok(Some(plan_candidate_forget_ui(
                session,
                None,
                PlannedCandidateForgetAction::Suppress {
                    code: session.phonetic().to_owned(),
                    text,
                    restore_session: has_exact_session_evidence,
                },
            )));
        }

        let decoded = decode_virtual_key(vkey, modifiers, InputMode::Chinese);
        if matches!(
            decoded,
            Some(CompositionInput::PreviousPage | CompositionInput::NextPage)
        ) {
            let input = decoded.expect("the paging branch contains one decoded input");
            let limit = if matches!(input, CompositionInput::NextPage) {
                candidate_next_page_limit(session.candidate_page_start())
            } else {
                candidate_visible_limit(session.candidate_page_start())
            };
            let batch = self.load_candidate_batch(
                provider,
                session.phonetic(),
                limit,
                InteractiveCandidateView::Primary,
            )?;
            let Some(mut plan) = plan_session_input(session, input, None, batch.candidates.len())
            else {
                return Ok(None);
            };
            plan.after
                .normalize_candidate_page(batch.candidates.len(), CANDIDATE_PAGE_SIZE);
            plan.candidate_display = Some(
                CandidateDisplay::from_batch(batch, plan.after.candidate_page_start())
                    .with_mode(CandidateDisplayMode::ForgetSelecting),
            );
            plan.candidate_forget_action_after_success = Some(
                PlannedCandidateForgetAction::Message(CandidateForgetMessage::Select),
            );
            return Ok(Some(plan));
        }

        let display = self.candidate_forget_display(
            provider,
            session,
            candidate_visible_limit(session.candidate_page_start()),
            message.display_mode(),
        )?;
        Ok(Some(plan_candidate_forget_ui(
            session,
            display,
            PlannedCandidateForgetAction::Message(message),
        )))
    }

    fn refreshed_candidate_forget_display(
        &self,
        mode: CandidateDisplayMode,
    ) -> Option<CandidateDisplay> {
        let provider = self.candidate_provider.as_ref()?;
        let session = self.composition.try_borrow().ok()?.clone();
        self.candidate_forget_display(
            provider.as_ref(),
            &session,
            candidate_visible_limit(session.candidate_page_start()),
            mode,
        )
        .ok()
        .flatten()
    }

    fn record_candidate_suppression_change(
        &self,
        code: &str,
        text: &str,
        action: NativeCandidateSuppressionAction,
    ) {
        let context = self
            .native_feedback_context
            .lock()
            .map(|cache| cache.context_for(code))
            .unwrap_or(NativeFeedbackContext::Unknown);
        let Ok(mut feedback) = self.native_feedback.lock() else {
            return;
        };
        if !feedback.is_accepting() {
            return;
        }
        let result = feedback.record_at(
            context,
            NativeFeedbackEvent::CandidateSuppressionChanged {
                code: code.to_owned(),
                text: text.to_owned(),
                action,
            },
            native_feedback_monotonic_ms(),
        );
        drop(feedback);
        if matches!(result, NativeFeedbackRecordResult::Stopped(_)) {
            self.native_feedback_language_bar_state.notify();
        }
    }

    fn apply_candidate_forget_action(
        &self,
        action: PlannedCandidateForgetAction,
    ) -> Option<CandidateDisplay> {
        match action {
            PlannedCandidateForgetAction::Enter => {
                *self.candidate_forget_state.try_borrow_mut().ok()? =
                    CandidateForgetState::Choosing(CandidateForgetMessage::Select);
                None
            }
            PlannedCandidateForgetAction::Cancel | PlannedCandidateForgetAction::Finalize => {
                *self.candidate_forget_state.try_borrow_mut().ok()? =
                    CandidateForgetState::Inactive;
                None
            }
            PlannedCandidateForgetAction::Message(message) => {
                *self.candidate_forget_state.try_borrow_mut().ok()? =
                    CandidateForgetState::Choosing(message);
                None
            }
            PlannedCandidateForgetAction::Suppress {
                code,
                text,
                restore_session,
            } => {
                let saved = self
                    .personal_ranking
                    .try_borrow_mut()
                    .ok()?
                    .append_suppression_action(
                        PersonalRankingSuppressionActionKind::Suppress,
                        &code,
                        &text,
                    );
                if saved {
                    if let Ok(mut memory) = self.selection_memory.try_borrow_mut() {
                        memory.forget_text(&code, &text);
                    }
                    self.record_candidate_suppression_change(
                        &code,
                        &text,
                        NativeCandidateSuppressionAction::Suppress,
                    );
                    *self.candidate_forget_state.try_borrow_mut().ok()? =
                        CandidateForgetState::UndoAvailable {
                            code,
                            text,
                            restore_session,
                        };
                    self.refreshed_candidate_forget_display(CandidateDisplayMode::ForgetUndo)
                } else {
                    *self.candidate_forget_state.try_borrow_mut().ok()? =
                        CandidateForgetState::Choosing(CandidateForgetMessage::SaveFailed);
                    self.refreshed_candidate_forget_display(CandidateDisplayMode::ForgetSaveFailed)
                }
            }
            PlannedCandidateForgetAction::Restore {
                code,
                text,
                restore_session,
            } => {
                let restored = self
                    .personal_ranking
                    .try_borrow_mut()
                    .ok()?
                    .append_suppression_action(
                        PersonalRankingSuppressionActionKind::Restore,
                        &code,
                        &text,
                    );
                if restored {
                    if restore_session
                        && let Ok(mut memory) = self.selection_memory.try_borrow_mut()
                    {
                        memory.remember_text(&code, &text);
                    }
                    self.record_candidate_suppression_change(
                        &code,
                        &text,
                        NativeCandidateSuppressionAction::Restore,
                    );
                    *self.candidate_forget_state.try_borrow_mut().ok()? =
                        CandidateForgetState::Inactive;
                    self.refreshed_candidate_forget_display(CandidateDisplayMode::ForgetRestored)
                } else {
                    *self.candidate_forget_state.try_borrow_mut().ok()? =
                        CandidateForgetState::UndoAvailable {
                            code,
                            text,
                            restore_session,
                        };
                    self.refreshed_candidate_forget_display(CandidateDisplayMode::ForgetSaveFailed)
                }
            }
        }
    }

    fn plan_key(&self, wparam: WPARAM, modifiers: KeyModifiers) -> Result<Option<PlannedKey>> {
        self.plan_key_with_pair_gap(wparam, modifiers, None)
    }

    fn plan_key_with_pair_gap(
        &self,
        wparam: WPARAM,
        modifiers: KeyModifiers,
        delivered_pair_gap_ms: Option<u64>,
    ) -> Result<Option<PlannedKey>> {
        self.plan_key_with_transposition_timing(
            wparam,
            modifiers,
            AutomaticTranspositionTimingEvidence {
                current_pair_gap_ms: delivered_pair_gap_ms,
                previous_pair: None,
            },
        )
    }

    fn plan_key_with_transposition_timing(
        &self,
        wparam: WPARAM,
        modifiers: KeyModifiers,
        transposition_timing: AutomaticTranspositionTimingEvidence,
    ) -> Result<Option<PlannedKey>> {
        let Some(provider) = self.candidate_provider.as_ref() else {
            return Ok(None);
        };
        let Ok(vkey) = u16::try_from(wparam.0) else {
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
        if is_candidate_forget_shortcut(vkey, modifiers) {
            return self.plan_candidate_forget_enter(provider.as_ref(), &session);
        }
        let forget_state = self
            .candidate_forget_state
            .try_borrow()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?
            .clone();
        if let CandidateForgetState::Choosing(message) = &forget_state {
            return self.plan_candidate_forget_choosing(
                provider.as_ref(),
                &session,
                *message,
                vkey,
                modifiers,
            );
        }
        let mut finalize_candidate_forget = false;
        if let CandidateForgetState::UndoAvailable {
            code,
            text,
            restore_session,
        } = forget_state
        {
            let plain_key =
                !modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.windows;
            if plain_key && vkey == VK_BACK.0 && !session.phonetic().is_empty() {
                return Ok(Some(plan_candidate_forget_ui(
                    &session,
                    None,
                    PlannedCandidateForgetAction::Restore {
                        code,
                        text,
                        restore_session,
                    },
                )));
            }
            if plain_key && vkey == VK_ESCAPE.0 {
                let display = self.candidate_forget_display(
                    provider.as_ref(),
                    &session,
                    candidate_visible_limit(session.candidate_page_start()),
                    CandidateDisplayMode::Normal,
                )?;
                return Ok(Some(plan_candidate_forget_ui(
                    &session,
                    display,
                    PlannedCandidateForgetAction::Cancel,
                )));
            }
            finalize_candidate_forget = true;
        }
        let Some(mut input) = decode_virtual_key(vkey, modifiers, self.input_mode.get()) else {
            return Ok(None);
        };
        let exact_inline_wish_trigger = matches!(&input, CompositionInput::EnterTab)
            && session.phonetic() == INLINE_WISH_TRIGGER_CODE
            && !session.tab_mode()
            && !session.recovery_mode()
            && !session.wish_prompt();
        if exact_inline_wish_trigger {
            let summary = self
                .native_feedback_language_bar_state
                .summary()
                .unwrap_or_default();
            if summary.lifecycle == NativeFeedbackLifecycle::Recording {
                let mut plan = plan_immediate_inline_wish(
                    &session,
                    InlineWishOperation::Capture {
                        scope: WishCaptureScope::RecentEpisodes,
                        category: WishCategory::Other,
                    },
                );
                if finalize_candidate_forget {
                    plan.candidate_forget_action_after_success =
                        Some(PlannedCandidateForgetAction::Finalize);
                }
                return Ok(Some(plan));
            }
            input = CompositionInput::EnterWish;
        }
        if matches!(&input, CompositionInput::EnterTab)
            && !session.phonetic().is_empty()
            && !session.tab_mode()
            && !session.recovery_mode()
            && !session.wish_prompt()
        {
            let code = session.phonetic();
            let entry = tab_phonetic_segments(code).and_then(|pinyin_segments| {
                let first_batch = self.load_shape_candidate_batch(
                    provider.as_ref(),
                    pinyin_segments[0],
                    "",
                    CANDIDATE_PAGE_SIZE,
                );
                let remaining_available = pinyin_segments.iter().skip(1).all(|pinyin| {
                    !self
                        .load_shape_candidate_batch(provider.as_ref(), pinyin, "", 1)
                        .candidates
                        .is_empty()
                });
                (!first_batch.candidates.is_empty() && remaining_available).then(|| {
                    let mut after = session.clone();
                    if pinyin_segments.len() == 1 {
                        after.enter_tab(pinyin_segments[0].to_owned());
                    } else {
                        let entered = after.enter_tab_path(&pinyin_segments);
                        debug_assert!(entered, "validated bounded Tab slots must enter");
                    }
                    (after, first_batch)
                })
            });
            if let Some((after, batch)) = entry {
                let display =
                    shape_candidate_display(CandidateDisplay::from_batch(batch, 0), &after);
                let mut plan = PlannedKey {
                    before: session.clone(),
                    after,
                    edit: None,
                    selection_to_remember: None,
                    feedback_after_success: Some(display.feedback_event(session.phonetic(), true)),
                    candidate_display: Some(display),
                    action_after_success: None,
                    candidate_forget_action_after_success: None,
                    overruled_text_to_remember: None,
                };
                if finalize_candidate_forget {
                    plan.candidate_forget_action_after_success =
                        Some(PlannedCandidateForgetAction::Finalize);
                }
                return Ok(Some(plan));
            }
        }
        let wish_actions = (session.wish_prompt() || matches!(&input, CompositionInput::EnterWish))
            .then(|| {
                inline_wish_actions(
                    self.native_feedback_language_bar_state
                        .summary()
                        .unwrap_or_default(),
                )
            });
        let wish_selection_rank = session.wish_prompt().then_some(match &input {
            CompositionInput::Confirm => Some(1),
            CompositionInput::Select(rank) => Some(*rank),
            _ => None,
        });
        let wish_selection_rank = wish_selection_rank.flatten().filter(|rank| {
            wish_actions
                .as_ref()
                .is_some_and(|actions| (1..=actions.len()).contains(rank))
        });
        if wish_selection_rank.is_some_and(|rank| rank != 1) {
            // The host-independent composition state only needs to know that
            // one visible wish action was confirmed. Preserve the original
            // rank separately for the structured capture operation.
            input = CompositionInput::Select(1);
        }
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
            if session.tab_mode() {
                self.load_shape_candidate_batch(
                    provider.as_ref(),
                    active_shape_code(&session),
                    session.stroke_prefix(),
                    limit,
                )
            } else {
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
            }
        } else {
            CandidateBatch::default()
        };
        let selected_absolute_index = match &input {
            CompositionInput::Confirm | CompositionInput::Punctuation(_) => {
                Some(session.candidate_page_start())
            }
            CompositionInput::Select(rank) => Some(
                session
                    .candidate_page_start()
                    .saturating_add(rank.saturating_sub(1)),
            ),
            _ => None,
        };
        let selected_text = selected_absolute_index
            .and_then(|index| existing_batch.candidates.get(index))
            .cloned();
        let selected_resolved_shape_code = selected_absolute_index
            .and_then(|index| existing_batch.resolved_shape_codes.get(index))
            .cloned()
            .flatten();
        let tab_assembly_selection = session.tab_assembly_mode()
            && matches!(
                &input,
                CompositionInput::Confirm | CompositionInput::Select(_)
            );
        let explicit_primary_selection = matches!(&input, CompositionInput::Select(_))
            || (session.candidate_page_start() > 0
                && matches!(
                    &input,
                    CompositionInput::Confirm | CompositionInput::Punctuation(_)
                ));
        let repeated_personal_primary_confirmation = matches!(&input, CompositionInput::Confirm)
            && session.candidate_page_start() == 0
            && !session.recovery_mode()
            && !session.tab_mode()
            && selected_absolute_index.is_some_and(|index| {
                existing_batch
                    .personalized
                    .get(index)
                    .copied()
                    .unwrap_or(false)
            })
            && selected_text.as_ref().is_some_and(|text| {
                provider_verifies_personal_character_composition(
                    provider.as_ref(),
                    session.phonetic(),
                    text,
                )
            });
        let overruled_text_to_remember =
            (explicit_primary_selection && !session.recovery_mode() && !session.tab_mode())
                .then(|| {
                    let protected_prefix = existing_batch
                        .protected_prefix_len
                        .min(existing_batch.candidates.len());
                    selected_absolute_index
                        .filter(|index| *index > protected_prefix)
                        .and_then(|_| existing_batch.candidates.get(protected_prefix))
                        .filter(|overruled| selected_text.as_ref() != Some(*overruled))
                        .cloned()
                })
                .flatten();
        let selection_to_remember = ((explicit_primary_selection
            || repeated_personal_primary_confirmation)
            && !session.recovery_mode()
            && !session.tab_mode())
        .then(|| {
            selected_text.as_ref().map(|text| PlannedSelection {
                code: session.phonetic().to_owned(),
                text: text.clone(),
                // A punctuation commit appends a suffix after the
                // candidate. Its first Backspace normally removes that
                // suffix rather than retracting the selected text.
                retractable_by_immediate_backspace: !matches!(
                    &input,
                    CompositionInput::Punctuation(_)
                ),
            })
        })
        .flatten();
        let selection_feedback = (!session.tab_assembly_mode())
            .then(|| {
                selected_text.as_ref().and_then(|text| {
                    let view = native_candidate_view(
                        if session.recovery_mode() {
                            InteractiveCandidateView::TranspositionRecovery
                        } else {
                            InteractiveCandidateView::Primary
                        },
                        session.tab_mode(),
                    );
                    match &input {
                        CompositionInput::Confirm => {
                            Some(NativeFeedbackEvent::CandidateCommitted {
                                code: session.phonetic().to_owned(),
                                text: text.clone(),
                                view,
                                source: NativeSelectionSource::FirstCandidate,
                                absolute_rank: session.candidate_page_start().saturating_add(1),
                                visible_rank: 1,
                            })
                        }
                        CompositionInput::Select(rank) => {
                            Some(NativeFeedbackEvent::CandidateCommitted {
                                code: session.phonetic().to_owned(),
                                text: text.clone(),
                                view,
                                source: NativeSelectionSource::Numeric,
                                absolute_rank: session.candidate_page_start().saturating_add(*rank),
                                visible_rank: *rank,
                            })
                        }
                        CompositionInput::Punctuation(_) => {
                            Some(NativeFeedbackEvent::CandidateCommitted {
                                code: session.phonetic().to_owned(),
                                text: text.clone(),
                                view,
                                source: NativeSelectionSource::Punctuation,
                                absolute_rank: session.candidate_page_start().saturating_add(1),
                                visible_rank: 1,
                            })
                        }
                        _ => None,
                    }
                })
            })
            .flatten();
        let planned = if tab_assembly_selection {
            plan_tab_assembly_selection(
                &session,
                &input,
                selected_text.clone(),
                selected_resolved_shape_code,
            )
        } else {
            plan_session_input(
                &session,
                input.clone(),
                selected_text.clone(),
                existing_batch.candidates.len(),
            )
        };
        let mut plan = match planned {
            Some(plan) => plan,
            None => return Ok(None),
        };
        let mut automatic_transposition_request =
            automatic_transposition_request(&input, &plan.after, transposition_timing);
        if let Some(request) = automatic_transposition_request.as_mut()
            && let Ok(feedback) = self.native_feedback.lock()
            && let Some(recommendation) = feedback.automatic_transposition_recommendation(
                plan.after.phonetic(),
                request.primary.pattern.first_syllable_index,
                request.primary.pair_gap_ms,
                match request.primary.cold_tier {
                    AutomaticTranspositionTier::Shadow => NativeAutomaticTranspositionTier::Shadow,
                    AutomaticTranspositionTier::Secondary => {
                        NativeAutomaticTranspositionTier::Secondary
                    }
                    AutomaticTranspositionTier::Primary => {
                        NativeAutomaticTranspositionTier::Primary
                    }
                },
            )
        {
            request.primary.tier = match recommendation.recommended_tier {
                NativeAutomaticTranspositionTier::Shadow => AutomaticTranspositionTier::Shadow,
                NativeAutomaticTranspositionTier::Secondary => {
                    AutomaticTranspositionTier::Secondary
                }
                NativeAutomaticTranspositionTier::Primary => AutomaticTranspositionTier::Primary,
            };
        }
        if session.wish_prompt()
            && let Some(rank) = wish_selection_rank
            && let Some(action) = wish_actions
                .as_ref()
                .and_then(|actions| actions.get(rank.saturating_sub(1)))
        {
            plan.action_after_success = Some(PlannedAction::Wish(action.operation));
        }
        if !tab_assembly_selection {
            plan.selection_to_remember = selection_to_remember;
            plan.overruled_text_to_remember = overruled_text_to_remember;
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
                })
        }
        if plan.after.wish_prompt() {
            if let Some(actions) = wish_actions.as_deref() {
                plan.candidate_display = Some(CandidateDisplay::actions(actions));
            }
            plan.feedback_after_success = None;
        } else if !plan.after.phonetic().is_empty() {
            let batch = if plan.after.tab_mode() {
                self.load_shape_candidate_batch(
                    provider.as_ref(),
                    active_shape_code(&plan.after),
                    plan.after.stroke_prefix(),
                    candidate_visible_limit(plan.after.candidate_page_start()),
                )
            } else if plan.after.phonetic() == session.phonetic()
                && !existing_batch.candidates.is_empty()
            {
                existing_batch
            } else {
                self.load_candidate_batch_with_automatic_transposition(
                    provider.as_ref(),
                    plan.after.phonetic(),
                    candidate_visible_limit(plan.after.candidate_page_start()),
                    if plan.after.recovery_mode() {
                        InteractiveCandidateView::TranspositionRecovery
                    } else {
                        InteractiveCandidateView::Primary
                    },
                    automatic_transposition_request,
                )?
            };
            plan.after
                .normalize_candidate_page(batch.candidates.len(), CANDIDATE_PAGE_SIZE);
            let mut display =
                CandidateDisplay::from_batch(batch, plan.after.candidate_page_start());
            if plan.after.tab_mode() {
                display = shape_candidate_display(display, &plan.after);
            }
            if plan.feedback_after_success.is_none() {
                plan.feedback_after_success =
                    Some(display.feedback_event(plan.after.phonetic(), plan.after.tab_mode()));
            }
            plan.candidate_display = Some(display);
        }
        if finalize_candidate_forget {
            plan.candidate_forget_action_after_success =
                Some(PlannedCandidateForgetAction::Finalize);
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
        let key_started_at = Instant::now();
        let refresh_started_at = Instant::now();
        if self.input_mode.get() == InputMode::Chinese
            && !self.has_active_logical_composition()?
            && u16::try_from(wparam.0)
                .ok()
                .and_then(|vkey| decode_virtual_key(vkey, modifiers, self.input_mode.get()))
                .is_some_and(|input| matches!(input, CompositionInput::Letters(_)))
            && let Some(provider) = self.candidate_provider.as_ref()
            && provider.refresh_at_safe_boundary()
        {
            *self
                .candidate_cache
                .try_borrow_mut()
                .map_err(|_| lifecycle_error(E_UNEXPECTED))? = CandidateCache::default();
            if let Ok(mut feedback) = self.native_feedback.lock() {
                feedback.update_candidate_identity(provider.candidate_data_identity());
            }
        }
        let refresh_ms = elapsed_milliseconds(refresh_started_at);
        let (pair_gap_ms, next_letter_anchor) = self.delivered_letter_timing(wparam, modifiers);
        let previous_pair_timing = self.last_completed_pair_timing.get();
        let planning_started_at = Instant::now();
        let Some(plan) = self.plan_key_with_transposition_timing(
            wparam,
            modifiers,
            AutomaticTranspositionTimingEvidence {
                current_pair_gap_ms: pair_gap_ms,
                previous_pair: previous_pair_timing,
            },
        )?
        else {
            self.last_delivered_letter.set(None);
            self.last_completed_pair_timing.set(None);
            if u16::try_from(wparam.0)
                .ok()
                .is_none_or(|vkey| !self.key_continues_personal_phrase(vkey, modifiers))
            {
                self.clear_personal_phrase_composer();
            }
            if !self.has_active_logical_composition()? {
                self.clear_personal_left_context();
            }
            return Ok(false.into());
        };
        let planning_ms = elapsed_milliseconds(planning_started_at);
        self.last_delivered_letter.set(next_letter_anchor);
        let after_code_len = plan.after.phonetic().len();
        let next_completed_pair_timing = completed_pair_timing_after_key(
            after_code_len,
            next_letter_anchor.is_some(),
            pair_gap_ms,
            previous_pair_timing,
        );
        self.last_completed_pair_timing
            .set(next_completed_pair_timing);
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
            mut candidate_display,
            selection_to_remember,
            overruled_text_to_remember,
            feedback_after_success,
            action_after_success,
            candidate_forget_action_after_success,
        } = plan;
        let breaks_personal_phrase = selection_to_remember.is_none()
            && (edit
                .as_ref()
                .is_some_and(|edit| edit.kind() != DocumentEditKind::UpdatePreedit)
                || before.tab_mode() != after.tab_mode()
                || before.recovery_mode() != after.recovery_mode()
                || before.wish_prompt() != after.wish_prompt()
                || action_after_success.is_some()
                || candidate_forget_action_after_success.is_some());
        // A synchronous commit ends the candidate UI and clears its cached
        // input-scope classification. Capture that classification before the
        // document edit so explicit learning, process-local left context and
        // slow-key diagnostics stay inside the same eligibility boundary.
        let feedback_event_context = feedback_after_success
            .as_ref()
            .and_then(native_feedback_event_code)
            .map(|code| {
                self.native_feedback_context
                    .lock()
                    .map(|cache| cache.context_for(code))
                    .unwrap_or(NativeFeedbackContext::Unknown)
            })
            .unwrap_or(NativeFeedbackContext::Unknown);
        let candidate_learning_context = if feedback_after_success
            .as_ref()
            .is_some_and(|event| matches!(event, NativeFeedbackEvent::CandidateCommitted { .. }))
        {
            feedback_event_context
        } else {
            NativeFeedbackContext::Unknown
        };
        let personal_left_context_update = match feedback_after_success.as_ref() {
            Some(NativeFeedbackEvent::CandidateCommitted { text, source, .. }) => Some(
                if candidate_learning_context == NativeFeedbackContext::Eligible
                    && *source != NativeSelectionSource::Punctuation
                    && PersonalContextRanking::accepts_left_context(text)
                {
                    Some(text.clone())
                } else {
                    None
                },
            ),
            Some(NativeFeedbackEvent::RawCodeCommitted { .. }) => Some(None),
            _ if matches!(edit.as_ref(), Some(PendingDocumentEdit::Insert(_)))
                || action_after_success.is_some() =>
            {
                Some(None)
            }
            _ => None,
        };
        let ui_only = edit.is_none();
        let personal_phrase_commit_text = selection_to_remember
            .as_ref()
            .map(|selection| selection.text.clone());
        let previous_phrase_document = selection_to_remember
            .as_ref()
            .map(|_| self.personal_phrase_document_snapshot());
        let mut edit_session_ms = 0;
        if let Some(edit) = edit {
            let edit_started_at = Instant::now();
            self.request_document_edit_session(
                &context,
                client_id,
                DocumentEditRequest {
                    action: edit,
                    candidate_display: candidate_display.clone(),
                    feedback_after_success: feedback_after_success.clone(),
                    personal_phrase_commit_text,
                    mode: EditSessionMode::KeySynchronous,
                    cleanup_target: None,
                },
            )?;
            edit_session_ms = elapsed_milliseconds(edit_started_at);
        }
        let timing_context = feedback_event_context;

        let mut composition = self
            .composition
            .try_borrow_mut()
            .map_err(|_| lifecycle_error(E_UNEXPECTED))?;
        if *composition != before {
            return Err(lifecycle_error(E_UNEXPECTED));
        }
        *composition = after;
        drop(composition);
        if let Some(selection) = selection_to_remember {
            let document_adjacency = self.take_personal_phrase_document_adjacency();
            let observation = self
                .remember_selection_after_success_in_context_with_overrule_and_document(
                    selection,
                    candidate_learning_context,
                    overruled_text_to_remember,
                    document_adjacency,
                    previous_phrase_document.unwrap_or_default(),
                );
            if let Some(observation) = observation {
                self.record_personal_phrase_adjacency_observed(observation);
            }
        } else if breaks_personal_phrase {
            self.clear_personal_phrase_composer();
        }
        if let Some(update) = personal_left_context_update {
            match update {
                Some(text) => self.set_personal_left_context(&text),
                None => self.clear_personal_left_context(),
            }
        }
        if let Some(PlannedAction::Wish(operation)) = action_after_success {
            let status = self
                .native_feedback_language_bar_state
                .perform_inline_wish_operation(operation);
            if let Ok(mut candidate_ui) = self.candidate_ui.try_borrow_mut() {
                let _ = candidate_ui.show_notice(inline_wish_notice(operation, status));
            }
        }
        if let Some(action) = candidate_forget_action_after_success
            && let Some(updated_display) = self.apply_candidate_forget_action(action)
        {
            candidate_display = Some(updated_display);
        }
        if ui_only && let Some(display) = candidate_display {
            let feedback_context = feedback_event_context;
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
        let total_ms = elapsed_milliseconds(key_started_at);
        if let Some(event) =
            slow_key_path_timing_event(refresh_ms, planning_ms, edit_session_ms, total_ms)
            && let Ok(mut feedback) = self.native_feedback.lock()
            && feedback.is_accepting()
        {
            let result = feedback.record_at(timing_context, event, native_feedback_monotonic_ms());
            drop(feedback);
            if matches!(result, NativeFeedbackRecordResult::Stopped(_)) {
                self.native_feedback_language_bar_state.notify();
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
        self.last_delivered_letter.set(None);
        self.last_completed_pair_timing.set(None);
        if let Ok(mut state) = self.candidate_forget_state.try_borrow_mut() {
            *state = CandidateForgetState::Inactive;
        }
        let _ = self.confirm_pending_personal_selection();
        self.clear_personal_phrase_composer();
        self.clear_personal_left_context();
        if let Ok(mut context_ranking) = self.personal_context_ranking.try_borrow_mut() {
            *context_ranking = PersonalContextRanking::default();
        }
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
        if let Ok(mut ranking) = self.personal_ranking.try_borrow_mut() {
            let _ = ranking.flush();
        }
        let feedback_context_result = match self.native_feedback_context.lock() {
            Ok(mut context) => {
                context.clear();
                Ok(())
            }
            Err(_) => Err(lifecycle_error(E_UNEXPECTED)),
        };
        let native_feedback_result = match self.native_feedback.lock() {
            Ok(mut feedback) => {
                let _ = feedback.flush_research();
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
        if self.should_route_pending_personal_key_down()? {
            // Route the real callback through OnKeyDown. It may withdraw an
            // immediate Backspace transaction, or confirm the transaction and
            // return FALSE when the key still belongs to the host.
            return Ok(true.into());
        }
        if self.pending_focus_cleanup_routes_letter(vkey, modifiers)? {
            // The old document still owns a queued cancellation. Route the
            // real callback so it can attempt an exact synchronous handoff;
            // failure returns FALSE there and leaves this key to the host.
            return Ok(true.into());
        }
        if self.should_route_candidate_forget_key(vkey, modifiers)? {
            return Ok(self.plan_key(wparam, modifiers)?.is_some().into());
        }
        if self.direct_input_needs_commit(vkey, modifiers)? {
            // Ask TSF to route the real key-down callback through us so the
            // current preedit can be committed before the host receives the
            // shifted or Caps Lock character.
            return Ok(true.into());
        }
        let planned = self.plan_key(wparam, modifiers)?;
        if planned.is_none() && !self.has_active_logical_composition()? {
            self.clear_personal_left_context();
        }
        Ok(planned.is_some().into())
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
        match self.resolve_pending_personal_selection_for_key(vkey, modifiers)? {
            PendingPersonalKeyResolution::Retracted => {
                self.record_post_commit_backspace_routed();
                // The host still owns the actual Backspace. FALSE lets it
                // delete the committed text after learning is withdrawn.
                return Ok(false.into());
            }
            PendingPersonalKeyResolution::Confirmed
                if !self.key_continues_personal_phrase(vkey, modifiers) =>
            {
                self.clear_personal_phrase_composer();
            }
            PendingPersonalKeyResolution::None | PendingPersonalKeyResolution::Confirmed => {}
        }
        if vkey == VK_SHIFT.0 {
            if !self.can_handle_shift_tap(modifiers) {
                return Ok(false.into());
            }
            self.shift_chord_pending.set(false);
            self.shift_tap_armed.set(true);
            return Ok(true.into());
        }
        modifiers = self.observe_nonshift_key_down_modifiers(modifiers);
        if self.pending_focus_cleanup_routes_letter(vkey, modifiers)?
            && !self.complete_pending_focus_cleanup_before_letter()?
        {
            return Ok(false.into());
        }
        if self.should_route_candidate_forget_key(vkey, modifiers)? {
            return self.apply_key_with_modifiers(context, wparam, modifiers);
        }
        if self.direct_input_needs_commit(vkey, modifiers)? {
            self.commit_active_composition(context)?;
            self.clear_personal_phrase_composer();
            self.clear_personal_left_context();
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
        self.clear_personal_left_context();
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
        self.clear_personal_left_context();
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

    fn candidate_runtime_test_root(stem: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-{stem}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    fn install_candidate_runtime_test_package(
        root: &Path,
        revision: &str,
        payload: &str,
    ) -> String {
        let manifest = CandidatePackageManifest::from_payload(revision, false, payload).unwrap();
        let manifest_text = manifest.render();
        let provenance_text = crate::CandidatePackageProvenance::from_materials(
            "tsf-runtime-test-source",
            "MPL-2.0",
            "https://github.com/hewzhew/ziranma-decoder",
            &candidate_sha256_hex(payload.as_bytes()),
            &manifest_text,
            payload,
        )
        .unwrap()
        .render();
        let package_id =
            crate::candidate_package_storage_id(&provenance_text, &manifest_text, payload);
        let package_sha256 = crate::candidate_package_authentication_sha256(
            &provenance_text,
            &manifest_text,
            payload,
        );
        let package = root
            .join(crate::CANDIDATE_PACKAGES_DIRECTORY)
            .join(&package_id);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join(crate::CANDIDATE_PACKAGE_MANIFEST_FILE),
            manifest_text,
        )
        .unwrap();
        fs::write(
            package.join(crate::CANDIDATE_PACKAGE_PROVENANCE_FILE),
            provenance_text,
        )
        .unwrap();
        fs::write(package.join(crate::CANDIDATE_PACKAGE_PAYLOAD_FILE), payload).unwrap();
        let preflights = root.join(crate::CANDIDATE_PREFLIGHTS_DIRECTORY);
        fs::create_dir_all(&preflights).unwrap();
        fs::write(
            preflights.join(format!("{package_id}.zpf")),
            crate::candidate_preflight_receipt_body(&package_id, &package_sha256),
        )
        .unwrap();
        package_id
    }

    fn select_candidate_runtime_test_package(
        root: &Path,
        package_id: &str,
        exact_promotions: usize,
    ) {
        let mut slots = crate::CandidateSlotState::default();
        slots.adopt(package_id).unwrap();
        fs::write(root.join(crate::CANDIDATE_SLOT_STATE_FILE), slots.render()).unwrap();
        fs::write(
            root.join(crate::CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            crate::CandidateSupplementalState::enabled(package_id, exact_promotions)
                .unwrap()
                .render(),
        )
        .unwrap();
    }

    #[test]
    fn candidate_promotion_mirrors_existing_personal_markers_by_position() {
        let mut batch = CandidateBatch {
            candidates: vec!["丙".to_owned(), "甲".to_owned(), "乙".to_owned()],
            resolved_shape_codes: vec![None; 3],
            provenance: vec![
                NativeCandidateProvenance::default(),
                NativeCandidateProvenance::default(),
                NativeCandidateProvenance::default(),
            ],
            personalized: vec![false, true, false],
            protected_prefix_len: 0,
            automatic_transposition: None,
            may_have_more: false,
            view: InteractiveCandidateView::Primary,
        };

        mirror_candidate_promotion(
            &mut batch,
            CandidateTextPromotion {
                index: 0,
                source_index: Some(2),
                changed: true,
            },
            NativeCandidatePersonalization::SESSION_EXACT,
        );

        assert_eq!(batch.personalized, [true, false, true]);
        assert!(
            batch.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_EXACT)
        );
        assert!(
            batch.provenance[0]
                .ranking_personalization()
                .contains(NativeCandidatePersonalization::SESSION_EXACT)
        );
    }

    #[test]
    fn unchanged_public_position_keeps_evidence_separate_from_a_ranking_change() {
        let mut batch = CandidateBatch {
            candidates: vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()],
            resolved_shape_codes: vec![None; 3],
            provenance: vec![
                NativeCandidateProvenance::default(),
                NativeCandidateProvenance::default(),
                NativeCandidateProvenance::default(),
            ],
            personalized: vec![false; 3],
            protected_prefix_len: 0,
            automatic_transposition: None,
            may_have_more: false,
            view: InteractiveCandidateView::Primary,
        };

        mirror_candidate_promotion(
            &mut batch,
            CandidateTextPromotion {
                index: 0,
                source_index: Some(0),
                changed: false,
            },
            NativeCandidatePersonalization::PERSISTENT_EXACT,
        );

        assert_eq!(batch.candidates, ["甲", "乙", "丙"]);
        assert_eq!(batch.personalized, [true, false, false]);
        assert!(
            batch.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        assert!(batch.provenance[0].ranking_personalization().is_empty());
    }

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
            research_feedback_root_for_module(&module),
            Some(PathBuf::from(
                r"D:\repo\.local\tsf-alpha\user-data\research-inbox"
            ))
        );
        assert_eq!(
            personal_ranking_root_for_module(&module),
            Some(PathBuf::from(
                r"D:\repo\.local\tsf-alpha\user-data\personal-ranking"
            ))
        );
        assert_eq!(
            personal_ranking_suppression_root_for_module(&module),
            Some(PathBuf::from(
                r"D:\repo\.local\tsf-alpha\user-data\personal-suppression"
            ))
        );
        assert_eq!(
            public_supplement_root_for_module(&module),
            Some(PathBuf::from(
                r"D:\repo\.local\tsf-alpha\user-data\public-supplement"
            ))
        );
        assert_eq!(immutable_module_sha256(&module), Some(digest));
        assert_eq!(
            explicit_alias_root_for_module(Path::new(r"D:\repo\target\release\ziranma_core.dll")),
            None
        );
        assert_eq!(
            immutable_module_sha256(Path::new(r"D:\repo\target\release\ziranma_core.dll")),
            None
        );
    }

    #[test]
    fn personal_ranking_runtime_flushes_and_reloads_current_user_protected_batches() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-ranking-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let mut first = PersonalRankingRuntime::new(Some(root.clone()));
        assert!(first.record("ab", "乙"));
        assert!(first.flush());
        let second = PersonalRankingRuntime::new(Some(root));
        assert_eq!(second.snapshot.preferred_text("ab"), Some("乙"));

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn background_persistence_drains_personal_ranking_before_shutdown() {
        let parent = candidate_runtime_test_root("background-ranking");
        let root = parent.join("ranking");
        let mut persistence = BackgroundPersistence::start();
        let handle = persistence.handle();
        let mut runtime = PersonalRankingRuntime::new_with_roots_and_persistence(
            Some(root.clone()),
            None,
            Some(handle.clone()),
        );

        assert!(runtime.record("ab", "乙"));
        assert!(
            runtime.flush(),
            "the hot path should accept the bounded job"
        );
        assert!(
            handle.wait_for_personal_ranking_idle(),
            "the activation barrier should observe a durable background write"
        );
        let reloaded = PersonalRankingRuntime::new(Some(root));
        assert_eq!(reloaded.snapshot.preferred_text("ab"), Some("乙"));

        drop(runtime);
        persistence.shutdown();
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn full_background_queue_rejects_without_waiting_for_capacity() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let health = Arc::new(BackgroundPersistenceHealth::default());
        let handle = BackgroundPersistenceHandle {
            sender: sender.clone(),
            health: Arc::clone(&health),
        };
        let (acknowledge, _acknowledged) = mpsc::channel();
        sender
            .try_send(BackgroundPersistenceCommand::Barrier(acknowledge))
            .unwrap();
        let batch = PersonalRankingBatch::now(
            std::process::id(),
            0,
            vec![PersonalRankingSelection::new("ab", "乙").unwrap()],
        )
        .unwrap();

        assert!(!handle.enqueue_personal_ranking(PathBuf::from("unused"), batch));
        assert_eq!(
            health
                .rejected_personal_ranking_jobs
                .load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn personal_ranking_runtime_applies_and_refreshes_explicit_suppressions() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-suppression-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let ranking_root = parent.join("ranking");
        let suppression_root = parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);

        let mut writer = PersonalRankingRuntime::new(Some(ranking_root.clone()));
        assert!(writer.record("ab", "乙"));
        assert!(writer.flush());
        drop(writer);
        crate::save_personal_ranking_suppression_action(
            &suppression_root,
            &crate::PersonalRankingSuppressionAction::new(
                10,
                std::process::id(),
                0,
                crate::PersonalRankingSuppressionActionKind::Suppress,
                "ab",
                "乙",
            )
            .unwrap(),
            &WindowsUserDataProtector,
        )
        .unwrap();

        let mut runtime = PersonalRankingRuntime::new(Some(ranking_root));
        let mut suppressed = vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()];
        assert!(!runtime.promote_texts_after("ab", &mut suppressed, 0));
        assert_eq!(suppressed, ["甲", "乙", "丙"]);
        assert!(runtime.is_suppressed("ab", "乙"));
        assert!(!runtime.is_suppressed("cd", "乙"));

        crate::save_personal_ranking_suppression_action(
            &suppression_root,
            &crate::PersonalRankingSuppressionAction::new(
                20,
                std::process::id(),
                1,
                crate::PersonalRankingSuppressionActionKind::Restore,
                "ab",
                "乙",
            )
            .unwrap(),
            &WindowsUserDataProtector,
        )
        .unwrap();
        assert!(runtime.refresh());
        let mut restored = vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()];
        assert!(runtime.promote_texts_after("ab", &mut restored, 0));
        assert_eq!(restored, ["乙", "甲", "丙"]);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn persisted_suppression_survives_a_stale_positive_writer_and_remains_code_scoped() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-stale-personal-writer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let ranking_root = parent.join("ranking");
        let suppression_root = parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);

        let mut stale_writer = PersonalRankingRuntime::new_with_roots(
            Some(ranking_root.clone()),
            Some(suppression_root.clone()),
        );
        assert!(stale_writer.record("ab", "乙词"));
        assert!(stale_writer.record("ab", "丙词"));
        assert!(stale_writer.record("ab", "丙词"));
        assert!(stale_writer.record("cd", "丙词"));
        assert!(stale_writer.flush());

        let mut forgetter = PersonalRankingRuntime::new_with_roots(
            Some(ranking_root.clone()),
            Some(suppression_root.clone()),
        );
        assert!(forgetter.append_suppression_action(
            PersonalRankingSuppressionActionKind::Suppress,
            "ab",
            "丙词"
        ));

        assert!(
            !stale_writer.is_suppressed("ab", "丙词"),
            "a runtime that has not refreshed must remain a faithful stale-host simulation"
        );
        assert!(stale_writer.record("ab", "丙词"));
        assert!(stale_writer.flush());

        let mut restarted = PersonalRankingRuntime::new_with_roots(
            Some(ranking_root.clone()),
            Some(suppression_root.clone()),
        );
        assert!(restarted.has_evidence("ab", "丙词"));
        assert!(restarted.is_suppressed("ab", "丙词"));
        assert!(!restarted.is_suppressed("cd", "丙词"));

        let mut same_code = vec!["甲词".to_owned(), "乙词".to_owned(), "丙词".to_owned()];
        assert!(restarted.promote_texts_after("ab", &mut same_code, 0));
        assert_eq!(same_code, ["乙词", "甲词", "丙词"]);

        let mut other_code = vec!["甲词".to_owned(), "乙词".to_owned(), "丙词".to_owned()];
        assert!(restarted.promote_texts_after("cd", &mut other_code, 0));
        assert_eq!(other_code, ["丙词", "甲词", "乙词"]);

        assert!(restarted.append_suppression_action(
            PersonalRankingSuppressionActionKind::Restore,
            "ab",
            "丙词"
        ));
        drop(restarted);

        let restored =
            PersonalRankingRuntime::new_with_roots(Some(ranking_root), Some(suppression_root));
        let mut same_code = vec!["甲词".to_owned(), "乙词".to_owned(), "丙词".to_owned()];
        assert!(restored.promote_texts_after("ab", &mut same_code, 0));
        assert_eq!(same_code, ["丙词", "甲词", "乙词"]);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn personal_ranking_runtime_appends_suppression_only_after_safe_persistence() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-append-suppression-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let ranking_root = parent.join("ranking");
        let suppression_root = parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY);
        let mut runtime = PersonalRankingRuntime::new_with_roots(
            Some(ranking_root.clone()),
            Some(suppression_root.clone()),
        );
        assert!(runtime.record("ab", "乙"));
        assert!(runtime.has_evidence("ab", "乙"));
        assert!(runtime.append_suppression_action(
            PersonalRankingSuppressionActionKind::Suppress,
            "ab",
            "乙"
        ));
        assert!(runtime.is_suppressed("ab", "乙"));
        assert_eq!(fs::read_dir(&suppression_root).unwrap().count(), 1);
        assert!(runtime.append_suppression_action(
            PersonalRankingSuppressionActionKind::Restore,
            "ab",
            "乙"
        ));
        assert!(!runtime.is_suppressed("ab", "乙"));
        assert_eq!(fs::read_dir(&suppression_root).unwrap().count(), 2);

        let invalid_root = parent.join("not-a-directory");
        fs::write(&invalid_root, b"occupied").unwrap();
        let mut failing = PersonalRankingRuntime::memory_only();
        assert!(failing.record("ab", "乙"));
        failing.suppression_root = Some(invalid_root);
        assert!(!failing.append_suppression_action(
            PersonalRankingSuppressionActionKind::Suppress,
            "ab",
            "乙"
        ));
        assert!(!failing.is_suppressed("ab", "乙"));

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn candidate_forget_state_debug_never_contains_private_identity() {
        let state = CandidateForgetState::UndoAvailable {
            code: "private-code".to_owned(),
            text: "私人候选".to_owned(),
            restore_session: true,
        };
        let debug = format!("{state:?}");
        assert!(!debug.contains("private-code"));
        assert!(!debug.contains("私人候选"));
        assert!(debug.contains("debug_contains_text: false"));
    }

    #[test]
    fn explicit_suppression_also_blocks_session_candidate_promotion() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-session-suppression-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let ranking_root = parent.join("ranking");
        crate::save_personal_ranking_suppression_action(
            &parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY),
            &crate::PersonalRankingSuppressionAction::new(
                10,
                std::process::id(),
                0,
                crate::PersonalRankingSuppressionActionKind::Suppress,
                "ab",
                "乙",
            )
            .unwrap(),
            &WindowsUserDataProtector,
        )
        .unwrap();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(ranking_root)));
        service
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "乙");

        let candidates = service
            .load_candidate_batch(
                &SelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(candidates.candidates, ["甲", "乙", "丙"]);
        assert!(!candidates.provenance[0].session_promoted());

        drop(service);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn personal_marker_follows_the_remembered_candidate_inside_a_protected_prefix() {
        let _guard = test_lock();
        let persistent = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ProtectedSelectionCandidateProvider,
        ))));
        assert!(persistent.personal_ranking.borrow_mut().record("ab", "甲"));

        let candidates = persistent
            .load_candidate_batch(
                &ProtectedSelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(candidates.candidates, ["甲", "乙", "丙"]);
        assert_eq!(
            candidates.personalized,
            [true, false, false],
            "persistent evidence must mark its protected candidate, not the first unprotected slot"
        );
        assert!(
            candidates.provenance[0]
                .ranking_personalization()
                .is_empty()
        );

        let session = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ProtectedSelectionCandidateProvider,
        ))));
        session
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "甲");
        let candidates = session
            .load_candidate_batch(
                &ProtectedSelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(candidates.candidates, ["甲", "乙", "丙"]);
        assert_eq!(
            candidates.personalized,
            [true, false, false],
            "session evidence must keep the same marker identity"
        );
        assert!(
            candidates.provenance[0]
                .ranking_personalization()
                .is_empty()
        );
    }

    #[test]
    fn new_personal_ranking_runtime_checkpoints_a_large_verified_history() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-checkpoint-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");
        let mut writer = PersonalRankingRuntime::new(Some(root.clone()));
        for _ in 0..crate::MIN_PERSONAL_RANKING_CHECKPOINT_BATCHES {
            assert!(writer.record("ab", "乙"));
            assert!(writer.flush());
        }
        drop(writer);
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(
                    |entry| entry.path().extension().and_then(|value| value.to_str())
                        == Some(crate::PERSONAL_RANKING_CHECKPOINT_EXTENSION)
                )
                .count(),
            0
        );

        let reader = PersonalRankingRuntime::new(Some(root.clone()));
        assert_eq!(reader.snapshot.preferred_text("ab"), Some("乙"));
        assert_eq!(
            fs::read_dir(&root)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(
                    |entry| entry.path().extension().and_then(|value| value.to_str())
                        == Some(crate::PERSONAL_RANKING_CHECKPOINT_EXTENSION)
                )
                .count(),
            1
        );
        drop(reader);

        fs::remove_dir_all(parent).unwrap();
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

        let install = |code: &str, text: &str| {
            let mut snapshot = ExplicitAliasSnapshot::default();
            snapshot.set(code, text).unwrap();
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

        let first = install("aa", "甲");
        let mut slots = crate::ExplicitAliasSlotState::default();
        slots.adopt(&first).unwrap();
        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), slots.render()).unwrap();
        let runtime = ExplicitAliasRuntime::new(root.clone());
        assert_eq!(runtime.text("aa").as_deref(), Some("甲"));

        let second = install("aa", "乙");
        slots.stage(&second).unwrap();
        slots.promote().unwrap();
        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), slots.render()).unwrap();
        assert_eq!(runtime.text("aa").as_deref(), Some("甲"));
        assert!(runtime.refresh());
        assert_eq!(runtime.text("aa").as_deref(), Some("乙"));

        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), "broken\n").unwrap();
        assert!(!runtime.refresh());
        assert_eq!(runtime.text("aa").as_deref(), Some("乙"));

        const CORE: &str = "text\tpinyin\tfrequency\n啊\ta\t100\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "alias-priority-core-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 1,
            })
            .unwrap(),
        );
        let provider = SnapshotCandidateProvider {
            snapshot,
            supplemental: PublicSupplementRuntime::static_layer(None),
            aliases: Some(runtime),
            refresh_throttle: Mutex::new(RefreshThrottle::default()),
            shape_candidate_pools: Mutex::new(ShapeCandidatePoolCache::default()),
            exact_full_code_candidates: Mutex::new(ExactFullCodeCandidateCache::default()),
        };
        let mut candidates = provider.candidates("aa", 2, InteractiveCandidateView::Primary);
        assert_eq!(candidates, ["乙", "啊"]);
        let output =
            provider.candidates_with_provenance("aa", 2, InteractiveCandidateView::Primary);
        assert_eq!(output.candidates, candidates);
        assert_eq!(output.protected_prefix_len, 1);
        assert_eq!(
            output
                .provenance
                .iter()
                .map(|item| item.source())
                .collect::<Vec<_>>(),
            [
                NativeCandidateSource::ExplicitAlias,
                NativeCandidateSource::CoreExact
            ]
        );
        let mut memory = SessionSelectionMemory::default();
        memory.remember_text("aa", "啊");
        let protected =
            provider.protected_candidate_prefix_len("aa", InteractiveCandidateView::Primary);
        assert_eq!(protected, 1);
        assert!(memory.promote_texts_after("aa", &mut candidates, protected));
        assert_eq!(candidates, ["乙", "啊"]);

        let mixed = install("vtrayn", "v2rayN");
        slots.stage(&mixed).unwrap();
        slots.promote().unwrap();
        fs::write(root.join(crate::EXPLICIT_ALIAS_SLOT_FILE), slots.render()).unwrap();
        assert!(provider.aliases.as_ref().unwrap().refresh());
        let mixed_output =
            provider.candidates_with_provenance("vtrayn", 2, InteractiveCandidateView::Primary);
        assert_eq!(
            mixed_output.candidates.first().map(String::as_str),
            Some("v2rayN")
        );
        assert_eq!(mixed_output.protected_prefix_len, 1);
        assert_eq!(
            mixed_output.provenance[0].source(),
            NativeCandidateSource::ExplicitAlias
        );
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

    struct ExactShortPagingCandidateProvider {
        calls: AtomicUsize,
        layer_requests: AtomicUsize,
        layer_enabled: AtomicBool,
        catalog: Arc<ExactShortWordCatalog>,
    }

    impl ExactShortPagingCandidateProvider {
        fn new() -> Self {
            const EXACT: &str = "text\tpinyin\tfrequency\n\
收束\tshou shu\t90\n\
手术\tshou shu\t80\n\
首数\tshou shu\t70\n";
            let manifest = CandidatePackageManifest::from_payload(
                "tsf-exact-short-page-test-v1",
                false,
                EXACT,
            )
            .unwrap();
            Self {
                calls: AtomicUsize::new(0),
                layer_requests: AtomicUsize::new(0),
                layer_enabled: AtomicBool::new(true),
                catalog: Arc::new(ExactShortWordCatalog::load(&manifest, EXACT).unwrap()),
            }
        }

        fn primary_candidates(limit: usize) -> Vec<String> {
            (1..=limit.min(CANDIDATE_LIMIT))
                .map(|rank| {
                    if rank == 17 {
                        "收束".to_owned()
                    } else {
                        format!("基础{rank}")
                    }
                })
                .collect()
        }
    }

    impl CandidateProvider for ExactShortPagingCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if code == "ubuu" && view == InteractiveCandidateView::Primary {
                Self::primary_candidates(limit)
            } else {
                Vec::new()
            }
        }

        fn candidates_with_provenance(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> CandidateProviderOutput {
            let candidates = self.candidates(code, limit, view);
            CandidateProviderOutput {
                provenance: vec![
                    NativeCandidateProvenance::new(
                        NativeCandidateSource::Decoder,
                        false
                    );
                    candidates.len()
                ],
                candidates,
                protected_prefix_len: 0,
                automatic_transposition_blocked: false,
            }
        }

        fn exact_short_layer(&self) -> Option<ExactShortCandidateLayer> {
            self.layer_requests.fetch_add(1, Ordering::Relaxed);
            self.layer_enabled
                .load(Ordering::Relaxed)
                .then(|| ExactShortCandidateLayer {
                    catalog: Arc::clone(&self.catalog),
                    exact_promotions: 2,
                })
        }

        fn automatic_transposition_candidates(
            &self,
            code: &str,
            _pattern: AutomaticTranspositionPattern,
            limit: usize,
        ) -> Option<CandidateProviderOutput> {
            (code == "ubuu" && limit > 0).then(|| CandidateProviderOutput {
                candidates: vec!["换序恢复".to_owned()],
                provenance: vec![NativeCandidateProvenance::new(
                    NativeCandidateSource::TranspositionRecovery,
                    false,
                )],
                protected_prefix_len: 0,
                automatic_transposition_blocked: false,
            })
        }
    }

    struct CountingProtectedPrefixCandidateProvider {
        candidate_calls: AtomicUsize,
        protected_prefix_calls: AtomicUsize,
    }

    impl CandidateProvider for CountingProtectedPrefixCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            self.candidate_calls.fetch_add(1, Ordering::Relaxed);
            if code != "ab" || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            ["固定", "甲", "乙"]
                .into_iter()
                .take(limit)
                .map(str::to_owned)
                .collect()
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            self.protected_prefix_calls.fetch_add(1, Ordering::Relaxed);
            usize::from(code == "ab" && view == InteractiveCandidateView::Primary)
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

        fn candidates_with_provenance(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> CandidateProviderOutput {
            let candidates = self.candidates(code, limit, view);
            let sources = match view {
                InteractiveCandidateView::Primary => [
                    NativeCandidateSource::CoreExact,
                    NativeCandidateSource::SupplementalExact,
                    NativeCandidateSource::Decoder,
                ],
                InteractiveCandidateView::TranspositionRecovery => {
                    [NativeCandidateSource::TranspositionRecovery; 3]
                }
            };
            let provenance = sources
                .into_iter()
                .take(candidates.len())
                .map(|source| NativeCandidateProvenance::new(source, false))
                .collect();
            let candidate_count = candidates.len();
            CandidateProviderOutput {
                candidates,
                provenance,
                protected_prefix_len: 0,
                automatic_transposition_blocked: view == InteractiveCandidateView::Primary
                    && sources
                        .into_iter()
                        .take(candidate_count)
                        .any(native_source_is_explicit_exact),
            }
        }
    }

    struct ExactIdentityAuditCandidateProvider;

    impl CandidateProvider for ExactIdentityAuditCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            let candidates: &[&str] = match code {
                "ab" | "cd" => &["甲词", "乙词", "丙词"],
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|candidate| (*candidate).to_owned())
                .collect()
        }
    }

    struct ShapeCandidateProvider;

    impl CandidateProvider for ShapeCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "qt" || limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            ["却", "缺", "雀"]
                .into_iter()
                .take(limit)
                .map(str::to_owned)
                .collect()
        }

        fn shape_candidates(&self, code: &str, prefix: &str, limit: usize) -> Vec<ShapeCandidate> {
            if limit == 0 {
                return Vec::new();
            }
            let candidates: &[(&str, &str)] = match (code, prefix) {
                ("qt", "") => &[("却", "qt"), ("缺", "qt"), ("雀", "qt")],
                ("qt", "s") => &[("雀", "qt")],
                ("qt", "x") => &[("雀", "qt")],
                ("hp", "") => &[("很", "hp"), ("和", "hp"), ("魂", "hp")],
                ("hp", "p") => &[("魂", "hp")],
                ("lm", "") => &[("连", "lm"), ("脸", "lm"), ("练", "lm")],
                ("lm", "s") => &[("练", "lm")],
                ("xi", "") => &[("西", "xi"), ("系", "xi"), ("习", "xi")],
                ("xi", "z") => &[("习", "xi")],
                ("jd", "") => &[("甲", "jd")],
                ("j", "") => &[
                    ("乙", "ji"),
                    ("件", "jm"),
                    ("今", "jb"),
                    ("经", "jk"),
                    ("就", "jq"),
                    ("见", "jn"),
                    ("进", "jv"),
                    ("仅", "jy"),
                ],
                ("j", "h") => &[("件", "jm")],
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|(text, resolved_code)| ShapeCandidate {
                    text: (*text).to_owned(),
                    resolved_code: (*resolved_code).to_owned(),
                })
                .collect()
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            matches!(
                (code, text),
                ("qt", "却" | "缺" | "雀")
                    | ("hp", "很" | "和" | "魂")
                    | ("lm", "连" | "脸" | "练")
                    | ("xi", "西" | "系" | "习")
                    | ("jd", "甲")
            )
        }
    }

    struct PersonalShortRecallCandidateProvider;

    impl CandidateProvider for PersonalShortRecallCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "qth" || limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            ["固定", "其他", "雀跃", "去向"]
                .into_iter()
                .take(limit)
                .map(str::to_owned)
                .collect()
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            usize::from(code == "qth" && view == InteractiveCandidateView::Primary)
        }

        fn shape_candidates(&self, code: &str, prefix: &str, limit: usize) -> Vec<ShapeCandidate> {
            ShapeCandidateProvider.shape_candidates(code, prefix, limit)
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            ShapeCandidateProvider.is_exact_full_code_candidate(code, text)
        }
    }

    struct PersonalPhraseCandidateProvider;

    impl CandidateProvider for PersonalPhraseCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            let candidates: &[&str] = match code {
                "ui" => &["是", "试"],
                "ub" => &["受", "手"],
                "lm" => &["连", "练"],
                "xi" => &["西", "习"],
                "aa" => &["阿", "啊"],
                "uiub" => &["是受", "失手", "是手"],
                "uiublm" => &["是受连", "失手练", "是手连"],
                "uiublmxi" => &["是受联系", "失手练习", "是手联系"],
                "uiul" | "uiubl" | "uiulx" | "uiublx" | "uiublmx" => &["固定", "普通", "其他"],
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|candidate| (*candidate).to_owned())
                .collect()
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            matches!(
                (code, text),
                ("ui", "试") | ("ub", "手") | ("lm", "练") | ("xi", "习") | ("aa", "啊")
            )
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            usize::from(
                view == InteractiveCandidateView::Primary
                    && matches!(code, "uiul" | "uiubl" | "uiulx" | "uiublx" | "uiublmx"),
            )
        }
    }

    fn remember_verified_personal_character(service: &TsfTextService_Impl, code: &str, text: &str) {
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: code.to_owned(),
                text: text.to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
    }

    fn personal_phrase_component_codes(service: &TsfTextService_Impl) -> Vec<String> {
        let composer = service.personal_phrase_composer.borrow();
        composer
            .components
            .iter()
            .map(|component| component.code.clone())
            .collect()
    }

    fn seed_personal_phrase_document_fallback(service: &TsfTextService_Impl) {
        let mut tracker = service.personal_phrase_document_tracker.borrow_mut();
        tracker.range_fallback_pending = true;
        tracker.completed_adjacency = Some(PersonalPhraseDocumentAdjacency::RangeUnavailable);
        tracker.last_consumed_adjacency = Some(PersonalPhraseDocumentAdjacency::RangeUnavailable);
    }

    fn assert_personal_phrase_document_tracker_cleared(service: &TsfTextService_Impl) {
        let tracker = service.personal_phrase_document_tracker.borrow();
        assert!(tracker.anchor.is_none());
        assert!(!tracker.range_fallback_pending);
        assert!(tracker.completed_adjacency.is_none());
        assert!(tracker.last_consumed_adjacency.is_none());
    }

    struct CodeFamilyCandidateProvider;

    impl CandidateProvider for CodeFamilyCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            let candidates: &[&str] = match code {
                "jdjd" => &["讲讲", "将将"],
                "jdj" => &["简单", "降价", "讲讲"],
                "jd" => &["讲", "将"],
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|candidate| (*candidate).to_owned())
                .collect()
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            code == "jdjd" && text == "讲讲"
        }
    }

    struct CountingCodeFamilyCandidateProvider {
        exact_calls: AtomicUsize,
    }

    impl CandidateProvider for CountingCodeFamilyCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            CodeFamilyCandidateProvider.candidates(code, limit, view)
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            self.exact_calls.fetch_add(1, Ordering::Relaxed);
            CodeFamilyCandidateProvider.is_exact_full_code_candidate(code, text)
        }
    }

    struct ExactShortDiscoveryCandidateProvider;

    impl CandidateProvider for ExactShortDiscoveryCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            let candidates: &[&str] = match code {
                "jdjd" => &["讲讲", "将将"],
                "jdj" => &["固定", "简单", "降价", "降级"],
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|candidate| (*candidate).to_owned())
                .collect()
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            usize::from(code == "jdj" && view == InteractiveCandidateView::Primary)
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            code == "jdjd" && text == "讲讲"
        }
    }

    struct LongExactShortDiscoveryCandidateProvider;

    impl CandidateProvider for LongExactShortDiscoveryCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            let candidates: &[&str] = match code {
                "abcdef" => &["甲乙丙"],
                "abcdefgh" => &["甲乙丙丁"],
                "abce" | "abcde" | "abceg" | "abcdeg" | "abcdefg" => {
                    &["固定", "普通", "其他", "末尾"]
                }
                _ => &[],
            };
            candidates
                .iter()
                .take(limit)
                .map(|candidate| (*candidate).to_owned())
                .collect()
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            usize::from(
                matches!(code, "abce" | "abcde" | "abceg" | "abcdeg" | "abcdefg")
                    && view == InteractiveCandidateView::Primary,
            )
        }

        fn is_exact_full_code_candidate(&self, code: &str, text: &str) -> bool {
            matches!(
                (code, text),
                ("abcdef", "甲乙丙") | ("abcdefgh", "甲乙丙丁")
            )
        }
    }

    struct PersonalContextCandidateProvider;

    impl CandidateProvider for PersonalContextCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "ab" || limit == 0 || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            [
                "吧", "八", "巴", "爸", "疤", "芭", "罢", "坝", "拔", "把", "霸", "靶",
            ]
            .into_iter()
            .take(limit)
            .map(str::to_owned)
            .collect()
        }
    }

    struct ProtectedSelectionCandidateProvider;

    impl CandidateProvider for ProtectedSelectionCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            SelectionCandidateProvider.candidates(code, limit, view)
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            usize::from(code == "ab" && view == InteractiveCandidateView::Primary)
        }
    }

    struct PagedProtectedCandidateProvider;

    impl CandidateProvider for PagedProtectedCandidateProvider {
        fn candidates(
            &self,
            code: &str,
            limit: usize,
            view: InteractiveCandidateView,
        ) -> Vec<String> {
            if code != "ab" || view != InteractiveCandidateView::Primary {
                return Vec::new();
            }
            (1..=12)
                .take(limit)
                .map(|rank| format!("候选{rank}"))
                .collect()
        }

        fn protected_candidate_prefix_len(
            &self,
            code: &str,
            view: InteractiveCandidateView,
        ) -> usize {
            if code == "ab" && view == InteractiveCandidateView::Primary {
                CANDIDATE_PAGE_SIZE
            } else {
                0
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

    #[derive(Clone, Copy)]
    enum TestContextSelection {
        Start,
        All,
    }

    #[implement(ITfEditSession)]
    struct ContextSelectionSetter {
        context: ITfContext,
        selection: TestContextSelection,
    }

    #[implement(ITfEditSession)]
    struct ContextSuffixDeleter {
        context: ITfContext,
    }

    impl ITfEditSession_Impl for ContextSuffixDeleter_Impl {
        fn DoEditSession(&self, ec: u32) -> Result<()> {
            // SAFETY: the endpoint belongs to this context and the test edits
            // only one trailing UTF-16 unit from its synthetic BMP text.
            let range = unsafe { self.context.GetEnd(ec) }?;
            let mut shifted = 0;
            unsafe { range.ShiftStart(ec, -1, &mut shifted, ptr::null()) }?;
            if shifted != -1 {
                return Err(lifecycle_error(E_UNEXPECTED));
            }
            unsafe { range.SetText(ec, 0, &[]) }?;
            move_selection_after_range(&self.context, &range, ec)
        }
    }

    impl ITfEditSession_Impl for ContextSelectionSetter_Impl {
        fn DoEditSession(&self, ec: u32) -> Result<()> {
            // SAFETY: both endpoint ranges belong to this context and `ec`
            // grants synchronous read/write access for the test callback.
            let range = unsafe { self.context.GetStart(ec) }?;
            if matches!(self.selection, TestContextSelection::All) {
                let end = unsafe { self.context.GetEnd(ec) }?;
                unsafe { range.ShiftEndToRange(ec, &end, TF_ANCHOR_END) }?;
            }
            let mut selection = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(range)),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_NONE,
                    fInterimChar: false.into(),
                },
            };
            let result = unsafe {
                self.context
                    .SetSelection(ec, std::slice::from_ref(&selection))
            };
            // SAFETY: release the interface field owned by this ABI value once.
            unsafe { std::mem::ManuallyDrop::drop(&mut selection.range) };
            result
        }
    }

    fn set_context_selection(
        context: &ITfContext,
        client_id: u32,
        selection: TestContextSelection,
    ) {
        let edit_session: ITfEditSession = ContextSelectionSetter {
            context: context.clone(),
            selection,
        }
        .into();
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        let result = unsafe { context.RequestEditSession(client_id, &edit_session, flags) }
            .expect("selection edit-session request");
        result.ok().expect("selection edit session");
    }

    fn delete_context_suffix(context: &ITfContext, client_id: u32) {
        let edit_session: ITfEditSession = ContextSuffixDeleter {
            context: context.clone(),
        }
        .into();
        let flags = TF_CONTEXT_EDIT_CONTEXT_FLAGS(TF_ES_SYNC.0 | TF_ES_READWRITE.0);
        let result = unsafe { context.RequestEditSession(client_id, &edit_session, flags) }
            .expect("suffix deletion edit-session request");
        result.ok().expect("suffix deletion edit session");
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
        let core = provider.candidates_with_provenance(
            "nihk",
            CANDIDATE_LIMIT,
            InteractiveCandidateView::Primary,
        );
        let candidates = core.candidates;
        assert_eq!(candidates.first().map(String::as_str), Some("你好"));
        assert_eq!(
            core.provenance.first().map(|item| item.source()),
            Some(NativeCandidateSource::CoreExact)
        );
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
            ("rbrb", "揉揉"),
            ("yidair", "一大串"),
            ("duuuyu", "独属于"),
            ("bugfub", "不跟手"),
            ("jmpn", "简拼"),
        ] {
            let output =
                provider.candidates_with_provenance(code, 7, InteractiveCandidateView::Primary);
            assert_eq!(
                output.candidates.first().map(String::as_str),
                Some(expected),
                "an exact project overlay entry must precede snapshot fallbacks for {code}"
            );
            assert_eq!(
                output.provenance.first().map(|item| item.source()),
                Some(NativeCandidateSource::ProjectOverlay)
            );
        }
    }

    #[test]
    fn snapshot_provider_labels_character_pairs_decoder_and_recovery_without_reordering() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
只\tzhi\t100\n\
动\tdong\t90\n\
什么\tshen me\t80\n\
神\tshen\t70\n\
恶魔\te mo\t60\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-provenance-core-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 5,
            })
            .unwrap(),
        );
        let provider = SnapshotCandidateProvider::new(snapshot, None, None);

        let pair =
            provider.candidates_with_provenance("vids", 7, InteractiveCandidateView::Primary);
        let pair_index = pair
            .candidates
            .iter()
            .position(|candidate| candidate == "只动")
            .expect("the bounded character-pair lane should stay visible");
        assert_eq!(
            pair.provenance[pair_index].source(),
            NativeCandidateSource::CharacterPair
        );

        let decoder =
            provider.candidates_with_provenance("zzzzzzzz", 1, InteractiveCandidateView::Primary);
        assert_eq!(decoder.candidates, ["zzzzzzzz"]);
        assert_eq!(
            decoder.provenance[0].source(),
            NativeCandidateSource::Decoder
        );

        let recovery = provider.candidates_with_provenance(
            "ufem",
            1,
            InteractiveCandidateView::TranspositionRecovery,
        );
        assert!(
            recovery
                .candidates
                .iter()
                .any(|candidate| candidate == "什么")
        );
        assert!(recovery.provenance.iter().all(|item| {
            item.source() == NativeCandidateSource::TranspositionRecovery
                && !item.session_promoted()
        }));
    }

    #[test]
    fn snapshot_provider_places_one_unambiguous_four_character_recovery_second() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
楼\tlou\t1000\n\
开\tkai\t900\n\
揉\trou\t800\n\
碎\tsui\t700\n\
掰开揉碎\tbai kai rou sui\t600\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-four-character-correction-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 5,
            })
            .unwrap(),
        );
        let provider = SnapshotCandidateProvider::new(snapshot, None, None);
        let intended = crate::encode_pinyin_phrase("bai kai rou sui")
            .unwrap()
            .full_code;
        let mut observed = intended.as_str().as_bytes().to_vec();
        observed.swap(0, 1);
        let observed = std::str::from_utf8(&observed).unwrap();

        let shallow =
            provider.candidates_with_provenance(observed, 1, InteractiveCandidateView::Primary);
        assert_eq!(shallow.candidates, ["楼开揉碎"]);
        assert_eq!(
            shallow.provenance[0].source(),
            NativeCandidateSource::Decoder
        );

        let visible =
            provider.candidates_with_provenance(observed, 6, InteractiveCandidateView::Primary);
        assert_eq!(visible.candidates[0], "楼开揉碎");
        assert_eq!(visible.candidates[1], "掰开揉碎");
        assert_eq!(
            visible.provenance[1].source(),
            NativeCandidateSource::FourCharacterCorrection
        );
        assert!(visible.automatic_transposition_blocked);

        let exact = provider.candidates_with_provenance(
            intended.as_str(),
            6,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            exact.candidates.first().map(String::as_str),
            Some("掰开揉碎")
        );
        assert!(
            exact
                .provenance
                .iter()
                .all(|item| item.source() != NativeCandidateSource::FourCharacterCorrection)
        );
    }

    #[test]
    fn snapshot_provider_reuses_the_pinned_stroke_index_for_explicit_shape_filtering() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
却\tque\t300\n\
缺\tque\t200\n\
雀\tque\t100\n\
魂\thun\t90\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-shape-core-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 4,
            })
            .unwrap(),
        );
        let provider = SnapshotCandidateProvider::new(snapshot, None, None);

        assert_eq!(
            provider.shape_candidates("qt", "", 6),
            [
                ShapeCandidate {
                    text: "却".to_owned(),
                    resolved_code: "qt".to_owned(),
                },
                ShapeCandidate {
                    text: "缺".to_owned(),
                    resolved_code: "qt".to_owned(),
                },
                ShapeCandidate {
                    text: "雀".to_owned(),
                    resolved_code: "qt".to_owned(),
                },
            ]
        );
        assert_eq!(
            provider.shape_candidates("qt", "s", 6),
            [ShapeCandidate {
                text: "雀".to_owned(),
                resolved_code: "qt".to_owned(),
            }]
        );
        assert_eq!(
            provider.shape_candidates("hp", "", 6),
            [ShapeCandidate {
                text: "魂".to_owned(),
                resolved_code: "hp".to_owned(),
            }]
        );
        assert!(provider.shape_candidates("qthp", "", 6).is_empty());
        assert!(provider.shape_candidates("qt", "a", 6).is_empty());
    }

    #[test]
    fn shape_candidate_pool_cache_is_bounded_and_keeps_recent_slots() {
        let mut cache = ShapeCandidatePoolCache::default();
        for code in ["aa", "bb", "cc", "dd"] {
            let pool: Arc<[ShapeCandidate]> = vec![ShapeCandidate {
                text: code.to_owned(),
                resolved_code: code.to_owned(),
            }]
            .into();
            cache.insert(code, pool);
        }
        assert!(cache.get("aa").is_some());
        let newest: Arc<[ShapeCandidate]> = vec![ShapeCandidate {
            text: "ee".to_owned(),
            resolved_code: "ee".to_owned(),
        }]
        .into();
        cache.insert("ee", newest);

        assert!(
            cache.get("bb").is_none(),
            "the least-recent slot is evicted"
        );
        assert_eq!(cache.entries.len(), SHAPE_CANDIDATE_POOL_CACHE_CAPACITY);
        assert!(
            cache.get("aa").is_some(),
            "a recent assembly slot remains cached"
        );
    }

    #[test]
    fn exact_full_code_cache_is_bounded_lru_and_version_scoped() {
        let mut cache = ExactFullCodeCandidateCache::default();
        for index in 0..EXACT_FULL_CODE_CANDIDATE_CACHE_CAPACITY {
            cache.insert(
                &format!("code-{index}"),
                &format!("public-{index}"),
                Some("supplement-a"),
                index.is_multiple_of(2),
            );
        }
        assert_eq!(
            cache.get("code-0", "public-0", Some("supplement-a")),
            Some(true)
        );
        cache.insert("code-new", "public-new", Some("supplement-a"), false);

        assert_eq!(
            cache.entries.len(),
            EXACT_FULL_CODE_CANDIDATE_CACHE_CAPACITY
        );
        assert_eq!(
            cache.get("code-1", "public-1", Some("supplement-a")),
            None,
            "the least-recent identity is evicted"
        );
        assert_eq!(
            cache.get("code-0", "public-0", Some("supplement-a")),
            Some(true),
            "a recently read identity remains cached"
        );
        assert_eq!(
            cache.get("code-new", "public-new", Some("supplement-b")),
            None,
            "the same code and text cannot cross a public supplement revision"
        );
        assert_eq!(
            cache.get("code-new", "public-new", Some("supplement-a")),
            Some(false),
            "negative verification decisions are cached too"
        );
    }

    #[test]
    fn snapshot_provider_memoizes_positive_and_negative_whole_word_verification() {
        let provider = reversed_adjacent_pair_provider();
        assert_eq!(
            provider
                .exact_full_code_candidates
                .lock()
                .unwrap()
                .entries
                .len(),
            0
        );

        assert!(provider.is_exact_full_code_candidate("ufme", "什么"));
        assert_eq!(
            provider
                .exact_full_code_candidates
                .lock()
                .unwrap()
                .entries
                .len(),
            1
        );
        assert!(!provider.is_exact_full_code_candidate("ufme", "公开合成缺席词"));
        assert_eq!(
            provider
                .exact_full_code_candidates
                .lock()
                .unwrap()
                .entries
                .len(),
            2
        );

        assert!(provider.is_exact_full_code_candidate("ufme", "什么"));
        let cache = provider.exact_full_code_candidates.lock().unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries.back().map(|entry| entry.exact), Some(true));
    }

    #[test]
    fn snapshot_provider_filters_shape_evidence_before_the_visible_rank_limit() {
        let mut core = "text\tpinyin\tfrequency\n".to_owned();
        let mut expected = Vec::new();
        for offset in 0..60_u32 {
            let character = char::from_u32(0x4e00 + offset).unwrap();
            expected.push(character.to_string());
            core.push_str(&format!(
                "{character}\tji\t{}\n",
                1_000_u64 - u64::from(offset)
            ));
        }
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-deep-shape-source-v1",
                contains_private_text: false,
                lexicon_tsv: &core,
                expected_payload_bytes: core.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(core.as_bytes()),
                expected_entry_count: expected.len(),
            })
            .unwrap(),
        );
        let provider = SnapshotCandidateProvider::new(snapshot, None, None);
        let pool = provider.shape_candidate_pool("ji");
        assert_eq!(pool.len(), expected.len());
        assert_eq!(pool[50].text, expected[50]);
        assert!(
            provider
                .shape_candidates("ji", "", CANDIDATE_LIMIT)
                .iter()
                .all(|candidate| candidate.text != expected[50]),
            "the ordinary empty-prefix display remains capped before rank 51"
        );

        let target = expected[50].chars().next().unwrap();
        let stroke_code = public_shape_index()
            .and_then(|shapes| shapes.get(target))
            .and_then(|shape| shape.stroke_codes().first())
            .expect("the pinned public stroke table covers the synthetic public target");
        let filtered = provider.shape_candidates("ji", stroke_code, CANDIDATE_LIMIT);
        assert!(
            filtered
                .iter()
                .any(|candidate| candidate.text == expected[50]),
            "shape filtering must run against the deep source before applying rank 50"
        );

        let cached = provider.shape_candidate_pool("ji");
        assert!(Arc::ptr_eq(&pool, &cached));
    }

    fn reversed_single_pair_provider() -> SnapshotCandidateProvider {
        const CORE: &str = "text\tpinyin\tfrequency\n\
俺们\tan men\t1000\n\
马\tma\t900\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-fast-reversed-pair-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 2,
            })
            .unwrap(),
        );
        SnapshotCandidateProvider::new(snapshot, None, None)
    }

    fn reversed_adjacent_pair_provider() -> SnapshotCandidateProvider {
        const CORE: &str = "text\tpinyin\tfrequency\n\
什么\tshen me\t1000\n\
发射\tfa she\t900\n";
        let snapshot = Arc::new(
            CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision: "tsf-fast-reversed-adjacent-pairs-v1",
                contains_private_text: false,
                lexicon_tsv: CORE,
                expected_payload_bytes: CORE.len(),
                expected_payload_fingerprint: crate::candidate_payload_fingerprint(CORE.as_bytes()),
                expected_entry_count: 2,
            })
            .unwrap(),
        );
        SnapshotCandidateProvider::new(snapshot, None, None)
    }

    fn reversed_single_pair_request(
        tier: AutomaticTranspositionTier,
    ) -> AutomaticTranspositionRequest {
        AutomaticTranspositionRequest {
            primary: AutomaticTranspositionAttempt {
                pattern: AutomaticTranspositionPattern::single(0),
                cold_tier: tier,
                tier,
                pair_gap_ms: match tier {
                    AutomaticTranspositionTier::Primary => {
                        u32::try_from(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS).unwrap()
                    }
                    AutomaticTranspositionTier::Secondary => {
                        u32::try_from(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS + 1).unwrap()
                    }
                    AutomaticTranspositionTier::Shadow => {
                        u32::try_from(AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS).unwrap()
                    }
                },
            },
            fallback: None,
        }
    }

    #[test]
    fn candidate_cache_keeps_one_automatic_reversed_pair_until_the_code_changes() {
        let provider = reversed_single_pair_provider();
        let mut cache = CandidateCache::default();
        let ordinary = cache.load(&provider, "am", 6, InteractiveCandidateView::Primary);
        assert_eq!(
            ordinary.candidates.first().map(String::as_str),
            Some("俺们")
        );

        let promoted = cache.load_with_automatic_transposition(
            &provider,
            "am",
            6,
            InteractiveCandidateView::Primary,
            Some(reversed_single_pair_request(
                AutomaticTranspositionTier::Primary,
            )),
        );
        assert_eq!(promoted.candidates.first().map(String::as_str), Some("马"));
        assert_eq!(
            promoted.provenance[0].source(),
            NativeCandidateSource::TranspositionRecovery
        );
        assert_eq!(
            cache
                .load(&provider, "am", 6, InteractiveCandidateView::Primary,)
                .candidates
                .first()
                .map(String::as_str),
            Some("马"),
            "Space or paging must see the same promoted candidate order"
        );

        let exact = cache.load_with_automatic_transposition(
            &provider,
            "ma",
            6,
            InteractiveCandidateView::Primary,
            Some(reversed_single_pair_request(
                AutomaticTranspositionTier::Primary,
            )),
        );
        assert_eq!(exact.candidates.first().map(String::as_str), Some("马"));
        assert_eq!(
            exact.provenance[0].source(),
            NativeCandidateSource::CoreExact,
            "an exact observed code must never be relabeled as recovery"
        );
        assert_eq!(
            cache.automatic_transposition_outcome,
            AutomaticTranspositionOutcome::Suppressed(AutomaticTranspositionTier::Primary)
        );
    }

    #[test]
    fn timing_score_separates_primary_secondary_shadow_and_ignored_pairs() {
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS),
            Some(AutomaticTranspositionTier::Primary)
        );
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS + 1),
            Some(AutomaticTranspositionTier::Secondary)
        );
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS - 1),
            Some(AutomaticTranspositionTier::Secondary)
        );
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS),
            Some(AutomaticTranspositionTier::Shadow)
        );
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_SHADOW_UPPER_GAP_MS - 1),
            Some(AutomaticTranspositionTier::Shadow)
        );
        assert_eq!(
            automatic_transposition_tier(AUTOMATIC_TRANSPOSITION_SHADOW_UPPER_GAP_MS),
            None
        );
    }

    #[test]
    fn adjacent_fast_pair_evidence_is_retained_until_the_next_pair_completes() {
        let first = completed_pair_timing_after_key(2, true, Some(31), None).unwrap();
        assert_eq!(
            first,
            CompletedPairTiming {
                syllable_index: 0,
                pair_gap_ms: 31
            }
        );
        assert_eq!(
            completed_pair_timing_after_key(3, true, None, Some(first)),
            Some(first),
            "the odd frame must retain the preceding completed pair"
        );
        assert_eq!(
            completed_pair_timing_after_key(4, true, Some(42), Some(first)),
            Some(CompletedPairTiming {
                syllable_index: 1,
                pair_gap_ms: 42
            }),
            "the newest completed pair becomes the next adjacency anchor"
        );
        assert_eq!(
            completed_pair_timing_after_key(0, false, None, Some(first)),
            None,
            "commit, cancellation and host keys break the timing chain"
        );
    }

    #[test]
    fn timed_adjacent_pair_fallback_recovers_fu_em_as_shen_me() {
        let provider = reversed_adjacent_pair_provider();
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("fuem".to_owned()));
        let request = automatic_transposition_request(
            &CompositionInput::Letters("m".to_owned()),
            &session,
            AutomaticTranspositionTimingEvidence {
                current_pair_gap_ms: Some(42),
                previous_pair: Some(CompletedPairTiming {
                    syllable_index: 0,
                    pair_gap_ms: 31,
                }),
            },
        )
        .expect("two adjacent measured pairs should create a bounded fallback");
        assert_eq!(
            request.primary.pattern,
            AutomaticTranspositionPattern::single(1)
        );
        assert_eq!(
            request.fallback.map(|attempt| attempt.pattern),
            Some(AutomaticTranspositionPattern::adjacent_pair(0))
        );

        let mut cache = CandidateCache::default();
        let batch = cache.load_with_automatic_transposition(
            &provider,
            "fuem",
            6,
            InteractiveCandidateView::Primary,
            Some(request),
        );
        assert_eq!(batch.candidates.first().map(String::as_str), Some("什么"));
        assert_eq!(
            batch.provenance.first().map(|item| item.source()),
            Some(NativeCandidateSource::TranspositionRecovery)
        );
        let decision = batch
            .automatic_transposition
            .expect("the feedback frame should retain the two-pair decision");
        assert_eq!(decision.syllable_index(), 0);
        assert_eq!(decision.syllable_count(), 2);
        assert_eq!(decision.pair_gap_ms(), 42);
        assert_eq!(decision.visible_rank(), Some(1));
        assert_eq!(decision.recovered_text(), Some("什么"));
    }

    #[test]
    fn tsf_plan_exposes_shen_me_for_two_timed_reversed_pairs() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            reversed_adjacent_pair_provider(),
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("fue".to_owned()));
        let plan = service
            .plan_key_with_transposition_timing(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                AutomaticTranspositionTimingEvidence {
                    current_pair_gap_ms: Some(42),
                    previous_pair: Some(CompletedPairTiming {
                        syllable_index: 0,
                        pair_gap_ms: 31,
                    }),
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(plan.after.phonetic(), "fuem");
        assert_eq!(
            plan.candidate_display
                .as_ref()
                .and_then(|display| display.visible().first())
                .map(String::as_str),
            Some("什么")
        );
        let Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            automatic_transposition: Some(decision),
            ..
        }) = plan.feedback_after_success
        else {
            panic!("the two-pair recovery should remain visible to wish feedback");
        };
        assert_eq!(decision.syllable_count(), 2);
        assert_eq!(decision.recovered_text(), Some("什么"));
    }

    #[test]
    fn timed_adjacent_pair_fallback_uses_the_slower_pair_for_exposure() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("fuem".to_owned()));
        let request = automatic_transposition_request(
            &CompositionInput::Letters("m".to_owned()),
            &session,
            AutomaticTranspositionTimingEvidence {
                current_pair_gap_ms: Some(32),
                previous_pair: Some(CompletedPairTiming {
                    syllable_index: 0,
                    pair_gap_ms: 55,
                }),
            },
        )
        .unwrap();
        let fallback = request.fallback.unwrap();
        assert_eq!(fallback.pair_gap_ms, 55);
        assert_eq!(fallback.tier, AutomaticTranspositionTier::Secondary);

        assert!(
            automatic_transposition_request(
                &CompositionInput::Letters("m".to_owned()),
                &session,
                AutomaticTranspositionTimingEvidence {
                    current_pair_gap_ms: Some(32),
                    previous_pair: Some(CompletedPairTiming {
                        syllable_index: 0,
                        pair_gap_ms: 96,
                    }),
                },
            )
            .unwrap()
            .fallback
            .is_none(),
            "one slow pair must disable the combined automatic recovery"
        );
    }

    #[test]
    fn candidate_cache_places_secondary_recovery_after_primary_and_keeps_shadow_hidden() {
        let provider = reversed_single_pair_provider();

        let mut secondary_cache = CandidateCache::default();
        let secondary = secondary_cache.load_with_automatic_transposition(
            &provider,
            "am",
            6,
            InteractiveCandidateView::Primary,
            Some(reversed_single_pair_request(
                AutomaticTranspositionTier::Secondary,
            )),
        );
        assert_eq!(&secondary.candidates[..2], ["俺们", "马"]);
        assert_eq!(
            secondary.provenance[1].source(),
            NativeCandidateSource::TranspositionRecovery
        );
        assert_eq!(
            secondary_cache.automatic_transposition_outcome,
            AutomaticTranspositionOutcome::RecoveryAvailable(AutomaticTranspositionTier::Secondary)
        );
        assert_eq!(
            &secondary_cache
                .load(&provider, "am", 6, InteractiveCandidateView::Primary)
                .candidates[..2],
            ["俺们", "马"],
            "Space and paging must keep the same secondary placement"
        );

        let mut shadow_cache = CandidateCache::default();
        let ordinary = shadow_cache.load(&provider, "am", 6, InteractiveCandidateView::Primary);
        let shadow = shadow_cache.load_with_automatic_transposition(
            &provider,
            "am",
            6,
            InteractiveCandidateView::Primary,
            Some(reversed_single_pair_request(
                AutomaticTranspositionTier::Shadow,
            )),
        );
        assert_eq!(shadow.candidates, ordinary.candidates);
        assert_eq!(shadow.provenance, ordinary.provenance);
        assert_eq!(
            shadow_cache.automatic_transposition_outcome,
            AutomaticTranspositionOutcome::RecoveryAvailable(AutomaticTranspositionTier::Shadow)
        );
    }

    #[test]
    fn delivered_pair_tiers_change_exposure_without_rewriting_the_observed_code() {
        let _guard = test_lock();
        let fast_service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(reversed_single_pair_provider()),
        )));
        fast_service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        let fast = fast_service
            .plan_key_with_pair_gap(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                Some(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            fast.candidate_display
                .as_ref()
                .and_then(|display| display.visible().first())
                .map(String::as_str),
            Some("马")
        );
        let Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            automatic_transposition: Some(fast_decision),
            ..
        }) = fast.feedback_after_success.as_ref()
        else {
            panic!("the promoted frame should carry its automatic decision");
        };
        assert_eq!(fast_decision.pair_gap_ms(), 48);
        assert_eq!(
            fast_decision.tier(),
            NativeAutomaticTranspositionTier::Primary
        );
        assert_eq!(
            fast_decision.outcome(),
            NativeAutomaticTranspositionOutcome::RecoveryAvailable
        );
        assert_eq!(fast_decision.recovered_text(), Some("马"));
        assert_eq!(fast_decision.visible_rank(), Some(1));
        *fast_service.composition.borrow_mut() = fast.after;
        let confirmation = fast_service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            confirmation.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "马"
        ));

        let secondary_service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(reversed_single_pair_provider()),
        )));
        secondary_service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        let secondary = secondary_service
            .plan_key_with_pair_gap(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                Some(AUTOMATIC_TRANSPOSITION_PRIMARY_MAX_GAP_MS + 1),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            &secondary.candidate_display.as_ref().unwrap().visible()[..2],
            ["俺们", "马"]
        );
        let Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            automatic_transposition: Some(secondary_decision),
            ..
        }) = secondary.feedback_after_success.as_ref()
        else {
            panic!("the secondary frame should carry its automatic decision");
        };
        assert_eq!(secondary_decision.pair_gap_ms(), 49);
        assert_eq!(secondary_decision.visible_rank(), Some(2));

        let shadow_service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(reversed_single_pair_provider()),
        )));
        shadow_service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        let shadow = shadow_service
            .plan_key_with_pair_gap(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                Some(AUTOMATIC_TRANSPOSITION_SECONDARY_UPPER_GAP_MS),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            shadow
                .candidate_display
                .as_ref()
                .and_then(|display| display.visible().first())
                .map(String::as_str),
            Some("俺们")
        );
        assert_eq!(
            shadow_service
                .candidate_cache
                .borrow()
                .automatic_transposition_outcome,
            AutomaticTranspositionOutcome::RecoveryAvailable(AutomaticTranspositionTier::Shadow)
        );
        let Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            automatic_transposition: Some(shadow_decision),
            ..
        }) = shadow.feedback_after_success.as_ref()
        else {
            panic!("the shadow frame should carry its automatic decision");
        };
        assert_eq!(shadow_decision.pair_gap_ms(), 64);
        assert_eq!(
            shadow_decision.tier(),
            NativeAutomaticTranspositionTier::Shadow
        );
        assert_eq!(shadow_decision.recovered_text(), Some("马"));
        assert_eq!(shadow_decision.visible_rank(), None);

        let ignored_service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(reversed_single_pair_provider()),
        )));
        ignored_service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        let ignored = ignored_service
            .plan_key_with_pair_gap(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                Some(AUTOMATIC_TRANSPOSITION_SHADOW_UPPER_GAP_MS),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            ignored
                .candidate_display
                .as_ref()
                .and_then(|display| display.visible().first())
                .map(String::as_str),
            Some("俺们")
        );
        assert!(matches!(
            ignored.feedback_after_success,
            Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                automatic_transposition: None,
                ..
            })
        ));
        assert_eq!(
            ignored_service
                .candidate_cache
                .borrow()
                .automatic_transposition_outcome,
            AutomaticTranspositionOutcome::NotRequested
        );
    }

    #[test]
    fn supplemental_provider_rescues_exact_words_without_displacing_core_exact_top_one() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
属于是\tshu yu shi\t100000\n\
什么\tshen me\t100\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t100\n\
甚么\tshen me\t100000\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            Arc::new(
                CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                    schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                    revision,
                    contains_private_text: false,
                    lexicon_tsv: lexicon,
                    expected_payload_bytes: lexicon.len(),
                    expected_payload_fingerprint: crate::candidate_payload_fingerprint(
                        lexicon.as_bytes(),
                    ),
                    expected_entry_count,
                })
                .unwrap(),
            )
        };
        let core = load("tsf-supplemental-core-v1", CORE, 2);
        let supplemental = load("tsf-supplemental-public-v1", SUPPLEMENTAL, 2);

        let core_only = SnapshotCandidateProvider::new(Arc::clone(&core), None, None);
        assert_eq!(
            core_only
                .candidates("uuyu", 2, InteractiveCandidateView::Primary)
                .first()
                .map(String::as_str),
            Some("属于是")
        );

        let layered = SnapshotCandidateProvider::new(
            core,
            Some((
                supplemental,
                crate::SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )),
            None,
        );
        assert_eq!(
            layered.candidates("uuyu", 3, InteractiveCandidateView::Primary),
            ["属于"]
        );
        assert_eq!(
            layered.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "甚么"]
        );
        let provenance =
            layered.candidates_with_provenance("ufme", 3, InteractiveCandidateView::Primary);
        assert_eq!(provenance.candidates, ["什么", "甚么"]);
        assert_eq!(
            provenance
                .provenance
                .iter()
                .map(|item| item.source())
                .collect::<Vec<_>>(),
            [
                NativeCandidateSource::CoreExact,
                NativeCandidateSource::SupplementalExact
            ]
        );
        assert_eq!(
            layered.candidates("ufme", 1, InteractiveCandidateView::Primary),
            ["什么"]
        );
        assert!(
            layered
                .candidates("uuyu", 0, InteractiveCandidateView::Primary)
                .is_empty()
        );
    }

    #[test]
    fn supplemental_runtime_refreshes_only_between_compositions_and_retains_last_good_data() {
        const CORE: &str = "text\tpinyin\tfrequency\n什么\tshen me\t100\n";
        const FIRST: &str = "text\tpinyin\tfrequency\n神马\tshen me\t100\n";
        const SECOND: &str = "text\tpinyin\tfrequency\n甚么\tshen me\t100\n";
        const BROKEN: &str = "text\tpinyin\tfrequency\n神么\tshen me\t100\n";
        let load_core = || {
            Arc::new(
                CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                    schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                    revision: "tsf-hot-core-v1",
                    contains_private_text: false,
                    lexicon_tsv: CORE,
                    expected_payload_bytes: CORE.len(),
                    expected_payload_fingerprint: crate::candidate_payload_fingerprint(
                        CORE.as_bytes(),
                    ),
                    expected_entry_count: 1,
                })
                .unwrap(),
            )
        };
        let root = candidate_runtime_test_root("supplement-hot-refresh");
        let first = install_candidate_runtime_test_package(&root, "tsf-hot-first-v1", FIRST);
        let second = install_candidate_runtime_test_package(&root, "tsf-hot-second-v1", SECOND);
        let broken = install_candidate_runtime_test_package(&root, "tsf-hot-broken-v1", BROKEN);
        select_candidate_runtime_test_package(&root, &first, 1);
        let selection = load_candidate_runtime_supplemental_selection(&root).unwrap();
        let initial = load_candidate_runtime_supplemental(&root, &selection)
            .unwrap()
            .unwrap();
        let provider = SnapshotCandidateProvider::new_with_runtime(
            load_core(),
            Some(initial),
            Some(root.clone()),
            None,
        );

        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "神马"]
        );
        assert_eq!(
            provider
                .candidate_data_identity()
                .unwrap()
                .supplemental_revision
                .as_deref(),
            Some("tsf-hot-first-v1")
        );
        let mut refresh_at = Instant::now();

        let first_payload = root
            .join(crate::CANDIDATE_PACKAGES_DIRECTORY)
            .join(&first)
            .join(crate::CANDIDATE_PACKAGE_PAYLOAD_FILE);
        fs::write(&first_payload, "damaged\n").unwrap();
        assert!(!provider.refresh_at_safe_boundary_at(refresh_at));
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "神马"],
            "an unchanged small pointer must not reopen the large payload"
        );
        fs::write(&first_payload, FIRST).unwrap();

        let first_snapshot = provider.supplemental.current().unwrap().0;
        select_candidate_runtime_test_package(&root, &first, 2);
        assert!(
            !provider.refresh_at_safe_boundary_at(refresh_at + Duration::from_millis(500)),
            "a second composition inside the polling interval must not reopen state files"
        );
        refresh_at += CANDIDATE_RUNTIME_REFRESH_INTERVAL;
        assert!(provider.refresh_at_safe_boundary_at(refresh_at));
        let reconfigured_snapshot = provider.supplemental.current().unwrap().0;
        assert!(Arc::ptr_eq(&first_snapshot, &reconfigured_snapshot));

        select_candidate_runtime_test_package(&root, &second, 1);
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "神马"],
            "an active composition keeps the already selected snapshot"
        );
        refresh_at += CANDIDATE_RUNTIME_REFRESH_INTERVAL;
        assert!(provider.refresh_at_safe_boundary_at(refresh_at));
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "甚么"]
        );
        assert_eq!(
            provider
                .candidate_data_identity()
                .unwrap()
                .supplemental_revision
                .as_deref(),
            Some("tsf-hot-second-v1")
        );

        fs::remove_file(
            root.join(crate::CANDIDATE_PREFLIGHTS_DIRECTORY)
                .join(format!("{broken}.zpf")),
        )
        .unwrap();
        select_candidate_runtime_test_package(&root, &broken, 1);
        refresh_at += CANDIDATE_RUNTIME_REFRESH_INTERVAL;
        assert!(!provider.refresh_at_safe_boundary_at(refresh_at));
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "甚么"],
            "a damaged replacement must retain the last valid snapshot"
        );

        fs::write(
            root.join(crate::CANDIDATE_SUPPLEMENTAL_STATE_FILE),
            crate::CandidateSupplementalState::default().render(),
        )
        .unwrap();
        refresh_at += CANDIDATE_RUNTIME_REFRESH_INTERVAL;
        assert!(provider.refresh_at_safe_boundary_at(refresh_at));
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么"]
        );
        assert_eq!(
            provider
                .candidate_data_identity()
                .unwrap()
                .supplemental_revision,
            None
        );

        select_candidate_runtime_test_package(&root, &first, 1);
        refresh_at += CANDIDATE_RUNTIME_REFRESH_INTERVAL;
        assert!(provider.refresh_at_safe_boundary_at(refresh_at));
        assert_eq!(
            provider.candidates("ufme", 3, InteractiveCandidateView::Primary),
            ["什么", "神马"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supplemental_provider_does_not_hide_a_complete_core_sentence() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
打\tda\t107925\n\
达\tda\t9692\n\
成\tcheng\t33117\n\
称\tcheng\t13485\n\
了\tle\t1500186\n\
成了\tcheng le\t10802\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
达成了\tda cheng le\t4459\n\
打成了\tda cheng le\t1190\n\
称了\tcheng le\t1033\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            Arc::new(
                CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                    schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                    revision,
                    contains_private_text: false,
                    lexicon_tsv: lexicon,
                    expected_payload_bytes: lexicon.len(),
                    expected_payload_fingerprint: crate::candidate_payload_fingerprint(
                        lexicon.as_bytes(),
                    ),
                    expected_entry_count,
                })
                .unwrap(),
            )
        };
        let provider = SnapshotCandidateProvider::new(
            load("tsf-complete-core-survival-v1", CORE, 6),
            Some((
                load("tsf-complete-supplement-survival-v1", SUPPLEMENTAL, 3),
                crate::SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )),
            None,
        );

        let output =
            provider.candidates_with_provenance("daigle", 6, InteractiveCandidateView::Primary);
        assert_eq!(
            output.candidates.first().map(String::as_str),
            Some("达成了")
        );
        assert_eq!(output.candidates.get(1).map(String::as_str), Some("打成了"));
        assert_eq!(
            output.provenance.get(1).map(|item| item.source()),
            Some(NativeCandidateSource::Decoder)
        );
    }

    #[test]
    fn supplemental_provider_keeps_core_order_until_consensus_gate_passes() {
        assert_eq!(
            TSF_PUBLIC_CANDIDATE_ORDER_POLICY,
            WishPublicCandidateOrderPolicy::ConservativeCoreFirst
        );
        const CORE: &str = "text\tpinyin\tfrequency\n\
大国\tda guo\t1657\n\
打过\tda guo\t1390\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
打过\tda guo\t9480\n\
大国\tda guo\t8656\n";
        let load = |revision: &str, lexicon: &str| {
            Arc::new(
                CandidateSnapshot::load(crate::CandidateSnapshotDescriptor {
                    schema: crate::CANDIDATE_SNAPSHOT_SCHEMA_V1,
                    revision,
                    contains_private_text: false,
                    lexicon_tsv: lexicon,
                    expected_payload_bytes: lexicon.len(),
                    expected_payload_fingerprint: crate::candidate_payload_fingerprint(
                        lexicon.as_bytes(),
                    ),
                    expected_entry_count: 2,
                })
                .unwrap(),
            )
        };
        let provider = SnapshotCandidateProvider::new(
            load("tsf-cold-consensus-core-v1", CORE),
            Some((
                load("tsf-cold-consensus-supplement-v1", SUPPLEMENTAL),
                crate::SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )),
            None,
        );

        let output =
            provider.candidates_with_provenance("dago", 2, InteractiveCandidateView::Primary);
        assert_eq!(output.candidates, ["大国", "打过"]);
        assert_eq!(
            output
                .provenance
                .iter()
                .map(|item| item.source())
                .collect::<Vec<_>>(),
            [
                NativeCandidateSource::CoreExact,
                NativeCandidateSource::CoreExact
            ]
        );
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
    fn real_key_callbacks_learn_and_reuse_one_adjacent_personal_phrase() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(PersonalPhraseCandidateProvider),
        )));
        assert!(
            !service_object
                .native_feedback
                .lock()
                .unwrap()
                .is_accepting(),
            "the optional recorder stays stopped while input-scope classification gates learning"
        );
        assert_eq!(
            service_object.native_feedback.lock().unwrap().start_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits::default(),
            ),
            NativeFeedbackStartResult::Started
        );
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
                    .as_bool(),
                "OnTestKeyDown did not route virtual key {vkey}"
            );
            assert!(
                unsafe { key_sink.OnKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool(),
                "OnKeyDown did not handle virtual key {vkey}"
            );
        };

        press(VK_A.0 + u16::from(b'u' - b'a'));
        press(VK_A.0 + u16::from(b'i' - b'a'));
        press(VK_1.0 + 1);
        assert_eq!(read_context_text(&context, client_id), "试");
        assert!(service_object.pending_personal_selection.borrow().is_some());
        assert!(
            service_object
                .personal_phrase_composer
                .borrow()
                .components
                .first()
                .is_some_and(|component| component.code == "ui" && component.text == "试")
        );

        press(VK_A.0 + u16::from(b'u' - b'a'));
        assert!(service_object.pending_personal_selection.borrow().is_none());
        assert!(
            service_object
                .personal_phrase_composer
                .borrow()
                .components
                .first()
                .is_some_and(|component| component.code == "ui" && component.text == "试")
        );
        press(VK_A.0 + u16::from(b'b' - b'a'));
        press(VK_1.0 + 1);
        assert_eq!(read_context_text(&context, client_id), "试手");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::VerifiedAdjacent)
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("uiub"),
            Some("试手")
        );
        {
            let feedback = service_object.native_feedback.lock().unwrap();
            assert!(feedback.events().windows(2).any(|events| matches!(
                events,
                [
                    NativeFeedbackEvent::CandidateCommitted { code, .. },
                    NativeFeedbackEvent::PersonalPhraseAdjacencyObserved {
                        adjacency: NativePersonalPhraseAdjacency::FirstAnchor,
                        previous_components: 0,
                        resulting_components: 1,
                    },
                ] if code == "ui"
            )));
            assert!(feedback.events().windows(2).any(|events| matches!(
                events,
                [
                    NativeFeedbackEvent::CandidateCommitted { code, .. },
                    NativeFeedbackEvent::PersonalPhraseAdjacencyObserved {
                        adjacency: NativePersonalPhraseAdjacency::VerifiedAdjacent,
                        previous_components: 1,
                        resulting_components: 2,
                    },
                ] if code == "ub"
            )));
        }

        press(VK_OEM_COMMA.0);
        assert_eq!(read_context_text(&context, client_id), "试手，");
        assert!(
            service_object
                .personal_phrase_composer
                .borrow()
                .components
                .is_empty(),
            "punctuation must confirm the pending phrase and break the adjacency chain"
        );
        assert_personal_phrase_document_tracker_cleared(&service_object);

        for letter in b"uiub" {
            press(VK_A.0 + u16::from(*letter - b'a'));
        }
        press(VK_SPACE.0);
        assert_eq!(read_context_text(&context, client_id), "试手，试手");
        assert_eq!(
            service_object
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            Some("试手")
        );
        assert!(
            service_object
                .personal_context_ranking
                .borrow()
                .has_evidence("试", "ub")
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
    }

    #[test]
    fn real_key_callbacks_break_personal_phrase_learning_after_caret_move_or_replacement() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(PersonalPhraseCandidateProvider),
        )));
        assert_eq!(
            service_object.native_feedback.lock().unwrap().start_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits::default(),
            ),
            NativeFeedbackStartResult::Started
        );
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
        let press = |target: &ITfContext, vkey: u16| {
            let key = WPARAM(usize::from(vkey));
            assert!(
                unsafe { key_sink.OnTestKeyDown(target, key, lparam) }
                    .unwrap()
                    .as_bool()
            );
            assert!(
                unsafe { key_sink.OnKeyDown(target, key, lparam) }
                    .unwrap()
                    .as_bool()
            );
        };
        let choose_second = |target: &ITfContext, code: &str| {
            for letter in code.bytes() {
                press(target, VK_A.0 + u16::from(letter - b'a'));
            }
            press(target, VK_1.0 + 1);
        };

        choose_second(&context, "ui");
        assert_eq!(read_context_text(&context, client_id), "试");
        set_context_selection(&context, client_id, TestContextSelection::Start);
        choose_second(&context, "ub");
        assert_eq!(read_context_text(&context, client_id), "手试");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::CaretMoved)
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("uiub"),
            None,
            "a caret move must not create an adjacent personal phrase"
        );
        assert_eq!(personal_phrase_component_codes(&service_object), ["ub"]);

        let second_document =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("second document creation");
        let mut second_context = None;
        let mut second_text_store_cookie = 0;
        unsafe {
            second_document.CreateContext(
                client_id,
                0,
                None::<&IUnknown>,
                &mut second_context,
                &mut second_text_store_cookie,
            )
        }
        .expect("second synthetic context creation");
        let second_context = second_context.expect("second context should be returned");
        unsafe { second_document.Push(&second_context) }.expect("second context push");
        choose_second(&second_context, "lm");
        assert_eq!(read_context_text(&second_context, client_id), "练");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::ContextChanged)
        );
        assert_eq!(personal_phrase_component_codes(&service_object), ["lm"]);

        set_context_selection(&second_context, client_id, TestContextSelection::All);
        choose_second(&second_context, "xi");
        assert_eq!(read_context_text(&second_context, client_id), "习");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::AnchorTextChanged)
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("lmxi"),
            None,
            "replacing the previous selection must not create an adjacent personal phrase"
        );
        assert_eq!(personal_phrase_component_codes(&service_object), ["xi"]);

        {
            let mut tracker = service_object.personal_phrase_document_tracker.borrow_mut();
            tracker.anchor = None;
            tracker.range_fallback_pending = true;
            tracker.completed_adjacency = None;
        }
        set_context_selection(&second_context, client_id, TestContextSelection::All);
        choose_second(&second_context, "aa");
        assert_eq!(read_context_text(&second_context, client_id), "啊");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::AnchorTextChanged),
            "an observable replacement must break the chain even after range fallback"
        );
        assert_eq!(personal_phrase_component_codes(&service_object), ["aa"]);
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("xiaa"),
            None
        );
        let adjacency_events = service_object
            .native_feedback
            .lock()
            .unwrap()
            .events()
            .iter()
            .filter_map(|event| match event {
                NativeFeedbackEvent::PersonalPhraseAdjacencyObserved {
                    adjacency,
                    previous_components,
                    resulting_components,
                } => Some((*adjacency, *previous_components, *resulting_components)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(adjacency_events.contains(&(NativePersonalPhraseAdjacency::CaretMoved, 1, 1)));
        assert!(adjacency_events.contains(&(NativePersonalPhraseAdjacency::ContextChanged, 1, 1)));
        assert!(adjacency_events.contains(&(
            NativePersonalPhraseAdjacency::AnchorTextChanged,
            1,
            1
        )));

        unsafe { second_document.Pop(TF_POPF_ALL) }.expect("second context pop");
        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(second_context);
        drop(second_document);
        drop(key_sink);
        drop(service);
        drop(service_object);
        drop(thread_manager);
    }

    #[test]
    fn real_backspace_retraction_restores_the_previous_document_anchor() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(PersonalPhraseCandidateProvider),
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
        let choose_second = |code: &str| {
            for letter in code.bytes() {
                press(VK_A.0 + u16::from(letter - b'a'));
            }
            press(VK_1.0 + 1);
        };

        for code in ["ui", "ub", "lm"] {
            choose_second(code);
        }
        assert_eq!(read_context_text(&context, client_id), "试手练");
        assert_eq!(
            personal_phrase_component_codes(&service_object),
            ["ui", "ub", "lm"]
        );

        let backspace = WPARAM(usize::from(VK_BACK.0));
        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, backspace, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            !unsafe { key_sink.OnKeyDown(&context, backspace, lparam) }
                .unwrap()
                .as_bool(),
            "the host must own deletion after the personal transaction is retracted"
        );
        delete_context_suffix(&context, client_id);
        assert_eq!(read_context_text(&context, client_id), "试手");
        assert_eq!(
            personal_phrase_component_codes(&service_object),
            ["ui", "ub"]
        );

        choose_second("lm");
        assert_eq!(read_context_text(&context, client_id), "试手练");
        assert_eq!(
            service_object
                .personal_phrase_document_tracker
                .borrow()
                .last_consumed_adjacency,
            Some(PersonalPhraseDocumentAdjacency::VerifiedAdjacent)
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("uiublm"),
            Some("试手练"),
            "the replacement third character should extend the restored prefix anchor"
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
    }

    #[test]
    fn advised_key_sink_retracts_learning_but_returns_backspace_to_the_host() {
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

        assert_eq!(
            service_object
                .native_feedback
                .lock()
                .unwrap()
                .start_rolling_memory(
                    NativeFeedbackAuthorization::explicit_memory_only(),
                    NativeFeedbackLimits::default(),
                ),
            NativeFeedbackStartResult::Started
        );

        service_object
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service_object
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "丙");
        service_object.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        let backspace = WPARAM(usize::from(VK_BACK.0));
        let lparam = LPARAM(0);
        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, backspace, lparam) }
                .unwrap()
                .as_bool(),
            "pending learning must route the real Backspace callback"
        );
        assert!(
            !unsafe { key_sink.OnKeyDown(&context, backspace, lparam) }
                .unwrap()
                .as_bool(),
            "the host must still receive the Backspace after learning is retracted"
        );
        assert!(service_object.pending_personal_selection.borrow().is_none());
        assert!(matches!(
            service_object
                .native_feedback
                .lock()
                .unwrap()
                .events()
                .last(),
            Some(NativeFeedbackEvent::PostCommitBackspaceRouted)
        ));
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("ab"),
            Some("丙")
        );
        assert_eq!(
            service_object
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            None
        );

        service_object.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        let host_key = WPARAM(usize::from(VK_CAPITAL.0));
        assert!(
            unsafe { key_sink.OnTestKeyDown(&context, host_key, lparam) }
                .unwrap()
                .as_bool(),
            "an otherwise unhandled key must reach OnKeyDown as a confirmation boundary"
        );
        assert!(
            !unsafe { key_sink.OnKeyDown(&context, host_key, lparam) }
                .unwrap()
                .as_bool(),
            "the confirmation boundary must return an otherwise unhandled key to the host"
        );
        assert_eq!(
            service_object
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙")
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
    }

    #[test]
    fn advised_key_sink_forgets_and_restores_without_editing_the_active_preedit() {
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

        service_object
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "乙");
        let lparam = LPARAM(0);
        let press = |vkey: u16| {
            let key = WPARAM(usize::from(vkey));
            assert!(
                unsafe { key_sink.OnTestKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool(),
                "OnTestKeyDown did not route virtual key {vkey}"
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
        assert_eq!(read_context_text(&context, client_id), "ab");

        service_object.synthetic_key_modifiers.set(KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        });
        press(VK_DELETE.0);
        service_object
            .synthetic_key_modifiers
            .set(KeyModifiers::default());
        assert_eq!(read_context_text(&context, client_id), "ab");
        assert!(matches!(
            &*service_object.candidate_forget_state.borrow(),
            CandidateForgetState::Choosing(CandidateForgetMessage::Select)
        ));

        press(VK_1.0);
        assert_eq!(read_context_text(&context, client_id), "ab");
        assert!(
            service_object
                .personal_ranking
                .borrow()
                .is_suppressed("ab", "乙")
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("ab"),
            None
        );

        press(VK_BACK.0);
        assert_eq!(read_context_text(&context, client_id), "ab");
        assert!(
            !service_object
                .personal_ranking
                .borrow()
                .is_suppressed("ab", "乙")
        );
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("ab"),
            Some("乙")
        );

        press(VK_BACK.0);
        assert_eq!(
            read_context_text(&context, client_id),
            "a",
            "only the immediate first Backspace belongs to the undo transaction"
        );
        press(VK_ESCAPE.0);
        assert_eq!(read_context_text(&context, client_id), "");

        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(key_sink);
        drop(service);
        drop(service_object);
        drop(thread_manager);
    }

    #[test]
    fn advised_key_sink_commits_an_inherited_anchored_tail_candidate() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(CodeFamilyCandidateProvider),
        )));
        service_object
            .selection_memory
            .borrow_mut()
            .remember_text("jdjd", "讲讲");
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
        press(VK_A.0 + 9);
        press(VK_A.0 + 3);
        press(VK_A.0 + 9);
        assert_eq!(read_context_text(&context, client_id), "jdj");
        press(VK_SPACE.0);
        assert_eq!(read_context_text(&context, client_id), "讲讲");

        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(context);
        drop(document_manager);
        drop(key_sink);
        drop(service);
        drop(service_object);
        drop(thread_manager);
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
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
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
                NativeFeedbackEvent::CandidatesPresentedWithProvenance {
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
    fn process_test_hands_first_letter_across_pending_focus_cleanup() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(FixedCandidateProvider),
        )));
        let service: ITfTextInputProcessorEx = service_object.to_interface();
        let key_sink: ITfKeyEventSink = service_object.to_interface();
        let thread_event_sink: ITfThreadMgrEventSink = service_object.to_interface();
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

        let a = WPARAM(usize::from(VK_A.0));
        let b = WPARAM(usize::from(VK_A.0 + 1));
        let lparam = LPARAM(0);
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

        let second_document =
            unsafe { thread_manager.CreateDocumentMgr() }.expect("second document creation");
        let mut second_context = None;
        let mut second_text_store_cookie = 0;
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
        unsafe { second_document.Push(&second_context) }.expect("second context push");
        unsafe { thread_manager.SetFocus(&second_document) }.expect("second document focus");

        // Moving to another TSF document accepts an asynchronous cancellation
        // and clears the logical buffer immediately, while the old document
        // composition stays active until that edit session or host termination.
        unsafe { thread_event_sink.OnSetFocus(&second_document, &document_manager) }
            .expect("document focus cleanup");
        assert!(service_object.composition.borrow().phonetic().is_empty());
        assert!(
            service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_some()
        );
        let old_cleanup_target = service_object
            .document_composition
            .borrow()
            .active
            .as_ref()
            .expect("the old document composition should still be pending")
            .composition
            .clone();

        // Typing in the newly focused document before queued cleanup runs must
        // synchronously finish the exact old composition and establish the new
        // one. A failed handoff instead returns FALSE from OnKeyDown, so this
        // path never claims success without a document edit.
        assert!(
            unsafe { key_sink.OnTestKeyDown(&second_context, b, lparam) }
                .unwrap()
                .as_bool()
        );
        assert!(
            unsafe { key_sink.OnKeyDown(&second_context, b, lparam) }
                .unwrap()
                .as_bool()
        );
        assert_eq!(service_object.composition.borrow().phonetic(), "b");
        assert_eq!(read_context_text(&context, client_id), "");
        assert_eq!(read_context_text(&second_context, client_id), "b");
        assert!(
            !service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );

        // Model the already queued asynchronous edit arriving after the new
        // composition starts. Its exact old identity must make it a no-op;
        // otherwise a late focus cleanup could erase the recovered first key.
        service_object
            .request_document_edit_session(
                &context,
                client_id,
                DocumentEditRequest {
                    action: PendingDocumentEdit::Cancel,
                    candidate_display: None,
                    feedback_after_success: None,
                    personal_phrase_commit_text: None,
                    mode: EditSessionMode::CleanupSynchronousHandoff,
                    cleanup_target: Some(old_cleanup_target),
                },
            )
            .expect("a stale guarded cleanup should complete as a no-op");
        assert_eq!(service_object.composition.borrow().phonetic(), "b");
        assert_eq!(read_context_text(&second_context, client_id), "b");

        terminate_composition_from_host(&second_context);
        assert!(
            service_object
                .document_composition
                .borrow()
                .active
                .is_none()
        );
        assert!(
            !service_object
                .document_composition
                .borrow()
                .cleanup_scheduled
        );

        unsafe { second_document.Pop(TF_POPF_ALL) }.expect("second context pop");
        unsafe { document_manager.Pop(TF_POPF_ALL) }.expect("context pop");
        unsafe { service.Deactivate() }.expect("service deactivation");
        unsafe { thread_manager.Deactivate() }.expect("thread manager deactivation");
        drop(second_context);
        drop(second_document);
        drop(context);
        drop(document_manager);
        drop(thread_event_sink);
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
        assert_personal_phrase_document_tracker_cleared(&service_object);
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

        service_object
            .personal_phrase_composer
            .borrow_mut()
            .components
            .push(PersonalPhraseComponent {
                code: "ui".to_owned(),
                text: "试".to_owned(),
            });
        seed_personal_phrase_document_fallback(&service_object);
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
        assert!(
            service_object
                .personal_phrase_composer
                .borrow()
                .components
                .is_empty(),
            "host termination must break the pending personal-phrase component chain"
        );
        assert_personal_phrase_document_tracker_cleared(&service_object);
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
        seed_personal_phrase_document_fallback(&service_object);
        // SAFETY: directly exercises the advised key sink's foreground-loss
        // callback with no system registration.
        unsafe { key_sink.OnSetFocus(false) }.expect("foreground loss cleanup");
        assert_personal_phrase_document_tracker_cleared(&service_object);
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
        seed_personal_phrase_document_fallback(&service_object);
        // SAFETY: deactivation schedules the same bounded cancellation before
        // releasing both event subscriptions.
        unsafe { service.Deactivate() }.expect("active composition deactivation");
        assert_personal_phrase_document_tracker_cleared(&service_object);
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
            decode_virtual_key(VK_6.0, shifted, InputMode::Chinese),
            Some(CompositionInput::Punctuation(
                CompositionPunctuation::Ellipsis
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
    fn slow_key_path_diagnostic_starts_at_one_frame() {
        assert!(slow_key_path_timing_event(1, 8, 5, 15).is_none());
        assert!(matches!(
            slow_key_path_timing_event(1, 8, 5, 16),
            Some(NativeFeedbackEvent::SlowKeyPathTiming {
                refresh_ms: 1,
                planning_ms: 8,
                edit_session_ms: 5,
                total_ms: 16,
            })
        ));
    }

    #[test]
    fn foreground_hosts_keep_a_bounded_memory_only_wish_buffer_ready() {
        let foreground = native_feedback_runtime_for_mode(KeyAdviceMode::Foreground, None, None);
        assert_eq!(
            foreground.summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        assert_eq!(foreground.summary().events, 0);

        let synthetic = native_feedback_runtime_for_mode(KeyAdviceMode::SyntheticHost, None, None);
        assert_eq!(
            synthetic.summary().lifecycle,
            NativeFeedbackLifecycle::Disabled
        );
    }

    #[test]
    fn enabled_research_runtime_saves_non_overlapping_encrypted_episode_batches() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-research-feedback-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::set_research_feedback_enabled(&root, true).unwrap();
        let mut runtime = NativeFeedbackRuntime::with_research_root(root.clone());
        runtime.research.module_sha256 = Some("cd".repeat(32));
        runtime.research.candidate_identity = Some(CandidateDataIdentity {
            core_revision: "research-core-v1".to_owned(),
            supplemental_revision: Some("research-supplement-v2".to_owned()),
        });
        runtime.record_at(
            NativeFeedbackContext::Password,
            NativeFeedbackEvent::CandidateCommitted {
                code: "ab".to_owned(),
                text: "合成敏感范围".to_owned(),
                view: NativeCandidateView::Ordinary,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 1,
                visible_rank: 1,
            },
            99,
        );
        assert!(runtime.research.events.is_empty());
        for index in 0..RESEARCH_FEEDBACK_BATCH_EPISODES {
            let result = runtime.record_at(
                NativeFeedbackContext::Eligible,
                NativeFeedbackEvent::CandidateCommitted {
                    code: "ab".to_owned(),
                    text: format!("候选{index}"),
                    view: NativeCandidateView::Ordinary,
                    source: NativeSelectionSource::FirstCandidate,
                    absolute_rank: 1,
                    visible_rank: 1,
                },
                u64::try_from(index).unwrap().saturating_add(100),
            );
            assert_eq!(result, NativeFeedbackRecordResult::Disabled);
        }
        assert!(runtime.research.events.is_empty());

        let packages = crate::list_wish_packages(&root).unwrap();
        assert_eq!(packages.len(), 1);
        let snapshot =
            crate::load_wish_snapshot(&root, packages[0].id(), &WindowsUserDataProtector).unwrap();
        assert_eq!(
            snapshot.capture_scope(),
            WishCaptureScope::ContinuousJournal
        );
        assert_eq!(snapshot.events().len(), RESEARCH_FEEDBACK_BATCH_EPISODES);
        assert_eq!(
            snapshot.focus_event_range(),
            0..RESEARCH_FEEDBACK_BATCH_EPISODES
        );
        let Some(WishJournalContext::ContinuousSpan(span)) = snapshot.journal_context() else {
            panic!("continuous journal link missing");
        };
        assert_eq!(span.batch_sequence(), 0);
        assert_eq!(span.first_event_ordinal(), 0);
        assert_eq!(span.previous_event_gap_ms(), None);
        let identity = snapshot.runtime_identity().unwrap();
        assert_eq!(identity.module_sha256(), "cd".repeat(32));
        assert_eq!(identity.core_candidate_revision(), "research-core-v1");
        assert_eq!(
            identity.supplemental_candidate_revision(),
            Some("research-supplement-v2")
        );
        assert_eq!(
            snapshot.public_candidate_order_policy(),
            WishPublicCandidateOrderPolicy::ConservativeCoreFirst
        );
        assert_eq!(
            snapshot
                .public_candidate_order_policy()
                .public_consensus_reorder_enabled(),
            Some(false)
        );

        runtime.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "cd".to_owned(),
            },
            1_000,
        );
        assert!(runtime.flush_research());
        let linked = crate::list_wish_packages(&root)
            .unwrap()
            .into_iter()
            .map(|package| {
                crate::load_wish_snapshot(&root, package.id(), &WindowsUserDataProtector).unwrap()
            })
            .find(|snapshot| {
                matches!(
                    snapshot.journal_context(),
                    Some(WishJournalContext::ContinuousSpan(span)) if span.batch_sequence() == 1
                )
            })
            .expect("second linked batch");
        let Some(WishJournalContext::ContinuousSpan(span)) = linked.journal_context() else {
            unreachable!();
        };
        assert_eq!(span.first_event_ordinal(), 8);
        assert_eq!(span.previous_event_gap_ms(), Some(893));

        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_persistence_drains_encrypted_research_before_shutdown() {
        let _guard = test_lock();
        let root = candidate_runtime_test_root("background-research");
        crate::set_research_feedback_enabled(&root, true).unwrap();
        let mut persistence = BackgroundPersistence::start();
        let mut runtime = NativeFeedbackRuntime::with_research_root(root.clone());
        runtime.research.persistence = Some(persistence.handle());

        for index in 0..RESEARCH_FEEDBACK_BATCH_EPISODES {
            runtime.record_at(
                NativeFeedbackContext::Eligible,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "ab".to_owned(),
                },
                u64::try_from(index).unwrap(),
            );
        }
        assert!(runtime.research.events.is_empty());
        persistence.shutdown();

        let packages = crate::list_wish_packages(&root).unwrap();
        assert_eq!(packages.len(), 1);
        let snapshot =
            crate::load_wish_snapshot(&root, packages[0].id(), &WindowsUserDataProtector).unwrap();
        assert_eq!(snapshot.events().len(), RESEARCH_FEEDBACK_BATCH_EPISODES);

        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn candidate_revision_refresh_closes_the_old_research_batch_before_relabeling() {
        let _guard = test_lock();
        let root = candidate_runtime_test_root("research-candidate-refresh");
        crate::set_research_feedback_enabled(&root, true).unwrap();
        let mut runtime = NativeFeedbackRuntime::with_research_root(root.clone());
        runtime.research.module_sha256 = Some("ef".repeat(32));
        runtime.update_candidate_identity(Some(CandidateDataIdentity {
            core_revision: "research-core-v1".to_owned(),
            supplemental_revision: Some("research-supplement-a".to_owned()),
        }));
        runtime.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "ab".to_owned(),
            },
            100,
        );
        runtime.update_candidate_identity(Some(CandidateDataIdentity {
            core_revision: "research-core-v1".to_owned(),
            supplemental_revision: Some("research-supplement-b".to_owned()),
        }));
        runtime.record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "cd".to_owned(),
            },
            200,
        );
        assert!(runtime.flush_research());

        let mut snapshots = crate::list_wish_packages(&root)
            .unwrap()
            .into_iter()
            .map(|package| {
                crate::load_wish_snapshot(&root, package.id(), &WindowsUserDataProtector).unwrap()
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| match snapshot.journal_context() {
            Some(WishJournalContext::ContinuousSpan(span)) => span.batch_sequence(),
            _ => u64::MAX,
        });
        assert_eq!(snapshots.len(), 2);
        assert_eq!(
            snapshots[0]
                .runtime_identity()
                .and_then(WishRuntimeIdentity::supplemental_candidate_revision),
            Some("research-supplement-a")
        );
        assert_eq!(
            snapshots[1]
                .runtime_identity()
                .and_then(WishRuntimeIdentity::supplemental_candidate_revision),
            Some("research-supplement-b")
        );
        drop(runtime);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_wish_anchor_matches_the_continuous_stream() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-linked-wish-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let research_root = parent.join("research");
        let wish_root = parent.join("wishes");
        crate::set_research_feedback_enabled(&research_root, true).unwrap();
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::with_research_root(
            research_root.clone(),
        )));
        let state = NativeFeedbackLanguageBarState::with_wish_root(
            Arc::clone(&feedback),
            Arc::new(Mutex::new(NativeFeedbackContextCache::default())),
            Rc::new(Cell::new(InputMode::Chinese)),
            Some(wish_root.clone()),
        );
        assert!(state.perform_feedback_action(FEEDBACK_MENU_START).unwrap());
        feedback.lock().unwrap().record_at(
            NativeFeedbackContext::Eligible,
            NativeFeedbackEvent::RawCodeCommitted {
                code: "ab".to_owned(),
            },
            native_feedback_monotonic_ms(),
        );
        assert!(state.perform_feedback_action(FEEDBACK_MENU_WISH).unwrap());
        let wish_packages = crate::list_wish_packages(&wish_root).unwrap();
        let wish =
            crate::load_wish_snapshot(&wish_root, wish_packages[0].id(), &WindowsUserDataProtector)
                .unwrap();
        let Some(WishJournalContext::WishAnchor(anchor)) = wish.journal_context() else {
            panic!("wish journal anchor missing");
        };
        assert_eq!(anchor.event_ordinal(), 0);
        let stream_id = anchor.stream_id().to_owned();

        drop(state);
        drop(feedback);
        let research_packages = crate::list_wish_packages(&research_root).unwrap();
        let journal = crate::load_wish_snapshot(
            &research_root,
            research_packages[0].id(),
            &WindowsUserDataProtector,
        )
        .unwrap();
        let Some(WishJournalContext::ContinuousSpan(span)) = journal.journal_context() else {
            panic!("continuous journal span missing");
        };
        assert_eq!(span.stream_id(), stream_id);
        assert_eq!(span.first_event_ordinal(), 0);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn enabled_research_runtime_flushes_partial_batch_when_host_is_released() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-research-drop-flush-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::set_research_feedback_enabled(&root, true).unwrap();
        {
            let mut runtime = NativeFeedbackRuntime::with_research_root(root.clone());
            runtime.record_at(
                NativeFeedbackContext::Eligible,
                NativeFeedbackEvent::RawCodeCommitted {
                    code: "ab".to_owned(),
                },
                100,
            );
            assert_eq!(runtime.research.events.len(), 1);
        }

        assert_eq!(crate::list_wish_packages(&root).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loaded_research_runtime_observes_enable_and_disable_without_host_restart() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-research-consent-refresh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut runtime = NativeFeedbackRuntime::with_research_root(root.clone());
        let committed = |text: &str| NativeFeedbackEvent::CandidateCommitted {
            code: "ab".to_owned(),
            text: text.to_owned(),
            view: NativeCandidateView::Ordinary,
            source: NativeSelectionSource::FirstCandidate,
            absolute_rank: 1,
            visible_rank: 1,
        };

        runtime.record_at(
            NativeFeedbackContext::Eligible,
            committed("关闭时不保存"),
            100,
        );
        assert!(runtime.research.events.is_empty());

        crate::set_research_feedback_enabled(&root, true).unwrap();
        runtime.record_at(
            NativeFeedbackContext::Eligible,
            committed("开启后进入内存批次"),
            1_100,
        );
        assert_eq!(runtime.research.events.len(), 1);

        crate::set_research_feedback_enabled(&root, false).unwrap();
        assert!(!runtime.flush_research());
        assert!(runtime.research.events.is_empty());
        assert!(crate::list_wish_packages(&root).unwrap().is_empty());

        crate::set_research_feedback_enabled(&root, true).unwrap();
        for index in 0..RESEARCH_FEEDBACK_BATCH_EPISODES {
            runtime.record_at(
                NativeFeedbackContext::Eligible,
                committed(&format!("再次开启{index}")),
                u64::try_from(index).unwrap().saturating_add(2_200),
            );
        }
        assert_eq!(crate::list_wish_packages(&root).unwrap().len(), 1);

        drop(runtime);
        fs::remove_dir_all(root).unwrap();
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
            "许愿保存重点现场；持续研究由独立设置控制；不联网"
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
            "许愿保存重点现场；持续研究由独立设置控制；不联网"
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
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
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
        assert_eq!(
            loaded.public_candidate_order_policy(),
            WishPublicCandidateOrderPolicy::ConservativeCoreFirst
        );
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
    fn explicit_wish_action_reports_nothing_recent_without_claiming_storage_failure() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ziranma-tsf-wish-empty-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
        let state = NativeFeedbackLanguageBarState::with_wish_root(
            Arc::clone(&feedback),
            Arc::new(Mutex::new(NativeFeedbackContextCache::default())),
            Rc::new(Cell::new(InputMode::Chinese)),
            Some(root.clone()),
        );
        assert!(state.perform_feedback_action(FEEDBACK_MENU_START).unwrap());

        assert!(!state.perform_feedback_action(FEEDBACK_MENU_WISH).unwrap());
        assert_eq!(state.wish_save_status.get(), WishSaveStatus::NothingRecent);
        assert_eq!(
            feedback.lock().unwrap().summary().lifecycle,
            NativeFeedbackLifecycle::Recording
        );
        assert!(
            state
                .menu()
                .unwrap()
                .iter()
                .any(|(_, _, label)| label == "最近没有可保存的输入法事件")
        );
        assert!(!root.exists());
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
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
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
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
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
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
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
        let feedback = Arc::new(Mutex::new(NativeFeedbackRuntime::memory_only()));
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
            !service.direct_input_needs_commit(VK_6.0, shifted).unwrap(),
            "Shift+6 is handled as a Chinese ellipsis"
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
    fn inline_wish_notice_is_a_short_rankless_acknowledgement() {
        let notice = inline_wish_notice(
            InlineWishOperation::Capture {
                scope: WishCaptureScope::RecentEpisodes,
                category: WishCategory::Other,
            },
            WishCommandAckStatus::Applied,
        );
        assert_eq!(notice.visible(), ["已经保存"]);
        assert_eq!(notice.action_detail(), Some("可以继续输入"));
        assert!(notice.is_notice());
        assert_eq!(notice.notice_icon(), CandidateNoticeIcon::WishReceived);
        let plain_notice = CandidateDisplay::notice("已经保存", "可以继续输入");
        let icon_metrics = candidate_popup_metrics(&notice, 96, 1_920);
        let plain_metrics = candidate_popup_metrics(&plain_notice, 96, 1_920);
        assert_eq!(
            icon_metrics.width - plain_metrics.width,
            POPUP_NOTICE_ICON_SIZE_LOGICAL + POPUP_NOTICE_ICON_GAP_LOGICAL
        );

        let failure = inline_wish_notice(
            InlineWishOperation::Capture {
                scope: WishCaptureScope::RecentEpisodes,
                category: WishCategory::Other,
            },
            WishCommandAckStatus::Failed,
        );
        assert_eq!(failure.notice_icon(), CandidateNoticeIcon::None);

        let nothing_recent = inline_wish_notice(
            InlineWishOperation::Capture {
                scope: WishCaptureScope::RecentEpisodes,
                category: WishCategory::Other,
            },
            WishCommandAckStatus::NoChange,
        );
        assert_eq!(nothing_recent.visible(), ["刚才没有可保存的内容"]);
        assert_eq!(nothing_recent.action_detail(), Some("继续输入后再试"));
        assert_eq!(nothing_recent.notice_icon(), CandidateNoticeIcon::None);

        let lifecycle = inline_wish_notice(
            InlineWishOperation::Command(WishCommand::Start),
            WishCommandAckStatus::Applied,
        );
        assert_eq!(lifecycle.notice_icon(), CandidateNoticeIcon::None);

        let mut ui = CandidateUiController::new_headless();
        assert!(ui.show_notice(notice));
        assert_eq!(
            ui.state.borrow().display.as_ref().unwrap().visible(),
            ["已经保存"]
        );
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
            (CompositionPunctuation::Ellipsis, "……"),
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
            ("ugmu", "声母"),
            ("ypmu", "韵母"),
            ("hbxrxd", "候选项"),
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
            ("dmsl", "电赛"),
            ("dagoyici", "打过一次"),
            ("ubxrxd", "首选项"),
            ("uivd", "实装"),
            ("ubpi", "手癖"),
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
            Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
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
    fn confirmed_same_pair_feedback_can_raise_secondary_to_primary_in_memory() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test_with_feedback(
            Some(Arc::new(reversed_single_pair_provider())),
            NativeFeedbackLimits::default(),
        ));
        {
            let mut feedback = service.native_feedback.lock().unwrap();
            for index in 0..8_u64 {
                assert_eq!(
                    feedback.record_at(
                        NativeFeedbackContext::Eligible,
                        NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                            code: "am".to_owned(),
                            view: NativeCandidateView::Ordinary,
                            page_start: 0,
                            candidates: vec!["俺们".to_owned(), "马".to_owned()],
                            provenance: vec![
                                NativeCandidateProvenance::new(
                                    NativeCandidateSource::Decoder,
                                    false,
                                ),
                                NativeCandidateProvenance::new(
                                    NativeCandidateSource::TranspositionRecovery,
                                    false,
                                ),
                            ],
                            automatic_transposition: Some(
                                NativeAutomaticTranspositionDecision::new(
                                    0,
                                    55,
                                    NativeAutomaticTranspositionTier::Secondary,
                                    NativeAutomaticTranspositionTier::Secondary,
                                    NativeAutomaticTranspositionOutcome::RecoveryAvailable,
                                    Some("马".to_owned()),
                                    Some(2),
                                ),
                            ),
                            loaded_candidates: 2,
                            tab_assembly: None,
                            may_have_more: false,
                        },
                        index.saturating_mul(10),
                    ),
                    NativeFeedbackRecordResult::Recorded
                );
                assert_eq!(
                    feedback.record_at(
                        NativeFeedbackContext::Eligible,
                        NativeFeedbackEvent::CandidateCommitted {
                            code: "am".to_owned(),
                            text: "马".to_owned(),
                            view: NativeCandidateView::Ordinary,
                            source: NativeSelectionSource::Numeric,
                            absolute_rank: 2,
                            visible_rank: 2,
                        },
                        index.saturating_mul(10).saturating_add(1),
                    ),
                    NativeFeedbackRecordResult::Recorded
                );
            }
        }

        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("a".to_owned()));
        let plan = service
            .plan_key_with_pair_gap(
                WPARAM(usize::from(VK_A.0 + u16::from(b'm' - b'a'))),
                KeyModifiers::default(),
                Some(55),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.candidate_display
                .as_ref()
                .and_then(|display| display.visible().first())
                .map(String::as_str),
            Some("马")
        );
        let Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            automatic_transposition: Some(decision),
            ..
        }) = plan.feedback_after_success.as_ref()
        else {
            panic!("the calibrated frame should retain its decision evidence");
        };
        assert_eq!(
            decision.cold_tier(),
            NativeAutomaticTranspositionTier::Secondary
        );
        assert_eq!(decision.tier(), NativeAutomaticTranspositionTier::Primary);
    }

    #[test]
    fn space_and_punctuation_confirm_the_first_candidate_on_the_visible_page() {
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
        *service.composition.borrow_mut() = next_page.after;

        let space = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            space.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "候选7"
        ));
        assert!(matches!(
            space.feedback_after_success.as_ref(),
            Some(NativeFeedbackEvent::CandidateCommitted {
                text,
                source: NativeSelectionSource::FirstCandidate,
                absolute_rank: 7,
                visible_rank: 1,
                ..
            }) if text == "候选7"
        ));
        let remembered = space
            .selection_to_remember
            .as_ref()
            .expect("a page change makes Space an explicit non-first selection");
        assert_eq!(
            (remembered.code.as_str(), remembered.text.as_str()),
            ("ab", "候选7")
        );

        let punctuation = service
            .plan_key(WPARAM(usize::from(VK_OEM_COMMA.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            punctuation.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "候选7，"
        ));
        assert!(matches!(
            punctuation.feedback_after_success.as_ref(),
            Some(NativeFeedbackEvent::CandidateCommitted {
                text,
                source: NativeSelectionSource::Punctuation,
                absolute_rank: 7,
                visible_rank: 1,
                ..
            }) if text == "候选7"
        ));
        let remembered = punctuation
            .selection_to_remember
            .as_ref()
            .expect("punctuation on a later page confirms an explicit non-first selection");
        assert_eq!(
            (remembered.code.as_str(), remembered.text.as_str()),
            ("ab", "候选7")
        );
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
    fn ordinary_tab_enters_shape_filter_without_changing_preedit_or_learning_the_choice() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("qt".to_owned()));

        let enter = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .expect("an exact two-key pool should enter shape mode");
        assert!(enter.edit.is_none());
        assert!(enter.after.tab_mode());
        assert_eq!(enter.after.phonetic(), "qt");
        assert_eq!(enter.after.stroke_prefix(), "");
        assert_eq!(
            enter.candidate_display.as_ref().unwrap().visible(),
            ["却", "缺", "雀"]
        );
        assert_eq!(
            enter.candidate_display.as_ref().unwrap().mode(),
            CandidateDisplayMode::Shape
        );
        assert_eq!(
            candidate_popup_mode_label(enter.candidate_display.as_ref().unwrap()),
            Some("找字 · qt · 形码 —")
        );
        assert!(matches!(
            enter.feedback_after_success,
            Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                view: NativeCandidateView::Shape,
                tab_assembly: Some(tab),
                ..
            }) if tab.position() == 1
                && tab.total_characters() == 1
                && tab.shape_prefix().is_empty()
        ));
        *service.composition.borrow_mut() = enter.after;

        let component = service
            .plan_key(
                WPARAM(usize::from(VK_A.0 + u16::from(b'x' - b'a'))),
                KeyModifiers::default(),
            )
            .unwrap()
            .expect("an arbitrary lowercase component prefix should share the Tab protocol");
        assert_eq!(component.after.stroke_prefix(), "x");
        assert_eq!(
            component.candidate_display.as_ref().unwrap().visible(),
            ["雀"]
        );
        assert_eq!(
            candidate_popup_mode_label(component.candidate_display.as_ref().unwrap()),
            Some("找字 · qt · 部件 x")
        );
        assert!(matches!(
            component.feedback_after_success,
            Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                tab_assembly: Some(tab),
                ..
            }) if tab.position() == 1
                && tab.total_characters() == 1
                && tab.shape_prefix() == "x"
        ));
        *service.composition.borrow_mut() = component.after;

        let clear_component = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should remove one shape-prefix key without leaving Tab");
        assert_eq!(clear_component.after.stroke_prefix(), "");
        *service.composition.borrow_mut() = clear_component.after;

        let filtered = service
            .plan_key(
                WPARAM(usize::from(VK_A.0 + u16::from(b'u' - b'a'))),
                KeyModifiers::default(),
            )
            .unwrap()
            .expect("the natural-code vertical-stroke key should refresh the frozen shape pool");
        assert!(filtered.edit.is_none());
        assert!(filtered.after.tab_mode());
        assert_eq!(filtered.after.phonetic(), "qt");
        assert_eq!(filtered.after.stroke_prefix(), "s");
        assert_eq!(
            filtered.candidate_display.as_ref().unwrap().visible(),
            ["雀"]
        );
        assert_eq!(
            candidate_popup_mode_label(filtered.candidate_display.as_ref().unwrap()),
            Some("找字 · qt · 笔画 竖")
        );
        *service.composition.borrow_mut() = filtered.after;

        let commit = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .expect("Space should commit the first filtered character");
        assert!(matches!(
            commit.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "雀"
        ));
        assert!(matches!(
            commit.feedback_after_success,
            Some(NativeFeedbackEvent::CandidateCommitted {
                view: NativeCandidateView::Shape,
                ..
            })
        ));
        assert!(commit.selection_to_remember.is_none());
        assert!(commit.after.phonetic().is_empty());
        assert!(!commit.after.tab_mode());
    }

    #[test]
    fn odd_key_tab_path_pages_by_verified_identity_and_learns_the_original_short_code() {
        let _guard = test_lock();
        assert_eq!(tab_phonetic_segments("jdj"), Some(vec!["jd", "j"]));
        assert_eq!(tab_phonetic_segments("jdjd"), Some(vec!["jd", "jd"]));

        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("jdj".to_owned()));

        let enter = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .expect("a complete slot followed by one initial should enter staged Tab lookup");
        assert_eq!(enter.after.shape_pinyin(), Some("jd"));
        assert!(enter.after.tab_assembly_has_trailing_initial());
        assert_eq!(enter.candidate_display.as_ref().unwrap().visible(), ["甲"]);
        assert_eq!(
            candidate_popup_mode_label(enter.candidate_display.as_ref().unwrap()),
            Some("找第 1 字 · jd · 形码 —")
        );
        *service.composition.borrow_mut() = enter.after;

        let first = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .expect("Space should advance from the complete first slot");
        assert!(first.edit.is_none());
        assert_eq!(first.after.shape_pinyin(), Some("j"));
        assert_eq!(
            first.candidate_display.as_ref().unwrap().visible(),
            ["乙", "件", "今", "经", "就", "见"]
        );
        assert_eq!(
            candidate_popup_mode_label(first.candidate_display.as_ref().unwrap()),
            Some("甲 → 第 2 字 · j·声母 · 形码 —")
        );
        *service.composition.borrow_mut() = first.after;

        let next_page = service
            .plan_key(WPARAM(usize::from(VK_NEXT.0)), KeyModifiers::default())
            .unwrap()
            .expect("the trailing-initial pool should retain ordinary candidate paging");
        assert_eq!(next_page.after.candidate_page_start(), CANDIDATE_PAGE_SIZE);
        assert_eq!(
            next_page.candidate_display.as_ref().unwrap().visible(),
            ["进", "仅"]
        );
        *service.composition.borrow_mut() = next_page.after;

        let previous_page = service
            .plan_key(WPARAM(usize::from(VK_PRIOR.0)), KeyModifiers::default())
            .unwrap()
            .expect("PageUp should return to the first trailing-initial page");
        assert_eq!(previous_page.after.candidate_page_start(), 0);
        assert_eq!(
            previous_page.candidate_display.as_ref().unwrap().visible(),
            ["乙", "件", "今", "经", "就", "见"]
        );
        *service.composition.borrow_mut() = previous_page.after;

        let stroke = service
            .plan_key(
                WPARAM(usize::from(VK_A.0 + u16::from(b'h' - b'a'))),
                KeyModifiers::default(),
            )
            .unwrap()
            .expect("the trailing-initial pool should accept ordinary shape refinement");
        assert_eq!(stroke.after.stroke_prefix(), "h");
        assert_eq!(stroke.candidate_display.as_ref().unwrap().visible(), ["件"]);
        assert_eq!(
            candidate_popup_mode_label(stroke.candidate_display.as_ref().unwrap()),
            Some("甲 → 第 2 字 · j·声母 · 笔画 横")
        );
        *service.composition.borrow_mut() = stroke.after;

        let complete = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .expect("selecting the trailing-initial character should complete the word");
        assert!(matches!(
            complete.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "甲件"
        ));
        let learned = complete
            .selection_to_remember
            .expect("a fully resolved trailing initial should safely learn the assembled word");
        assert_eq!(learned.code, "jdj");
        assert_eq!(learned.text, "甲件");
        assert!(learned.retractable_by_immediate_backspace);
        assert!(!complete.after.tab_mode());

        service
            .remember_selection_after_success_in_context(learned, NativeFeedbackContext::Eligible);
        assert_eq!(
            service.selection_memory.borrow().remembered_text("jdj"),
            Some("甲件")
        );
        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();

        let recalled = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "jdj",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            recalled.candidates.first().map(String::as_str),
            Some("甲件"),
            "the original odd input remains the personal recall key"
        );
        assert_eq!(recalled.personalized.first(), Some(&true));
    }

    #[test]
    fn tab_assembly_refuses_missing_or_inconsistent_candidate_identity() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("jdj".to_owned()));
        assert!(session.enter_tab_path(&["jd", "j"]));
        assert_eq!(
            session.accept_tab_assembly_candidate("甲", "jd"),
            Some(TabAssemblySelection::Advanced)
        );

        assert!(
            plan_tab_assembly_selection(
                &session,
                &CompositionInput::Confirm,
                Some("件".to_owned()),
                None,
            )
            .is_none(),
            "text without aligned identity must not be committed or learned"
        );
        assert!(
            plan_tab_assembly_selection(
                &session,
                &CompositionInput::Confirm,
                Some("件".to_owned()),
                Some("am".to_owned()),
            )
            .is_none(),
            "an identity outside the active initial pool must be rejected"
        );
        let complete = plan_tab_assembly_selection(
            &session,
            &CompositionInput::Confirm,
            Some("件".to_owned()),
            Some("jm".to_owned()),
        )
        .expect("the aligned full identity should complete the assembly");
        assert_eq!(
            complete
                .selection_to_remember
                .as_ref()
                .map(|selection| (selection.code.as_str(), selection.text.as_str())),
            Some(("jdj", "甲件"))
        );
    }

    #[test]
    fn public_character_verification_covers_two_to_four_characters_without_partial_acceptance() {
        assert!(provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthp",
            "雀魂",
        ));
        assert!(provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthplm",
            "雀魂练",
        ));
        assert!(provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthplmxi",
            "雀魂练习",
        ));
        assert!(!provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthplmxi",
            "雀魂练",
        ));
        assert!(!provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthplm",
            "雀魂练习",
        ));
        assert!(!provider_verifies_personal_character_composition(
            &ShapeCandidateProvider,
            "qthplmxi",
            "雀魂西习",
        ));
    }

    #[test]
    fn four_key_tab_assembles_two_shape_characters_and_learns_only_the_whole_word() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("qthp".to_owned()));

        let enter = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .expect("two complete single-character pools should enter staged shape mode");
        assert!(enter.edit.is_none());
        assert_eq!(enter.after.phonetic(), "qthp");
        assert_eq!(enter.after.shape_pinyin(), Some("qt"));
        assert_eq!(
            enter.after.tab_assembly_stage(),
            Some(TabAssemblyStage::First)
        );
        let display = enter.candidate_display.as_ref().unwrap();
        assert_eq!(display.visible(), ["却", "缺", "雀"]);
        assert_eq!(display.mode(), CandidateDisplayMode::ShapeAssemblyFirst);
        assert_eq!(
            candidate_popup_mode_label(display),
            Some("找第 1 字 · qt · 形码 —")
        );
        *service.composition.borrow_mut() = enter.after;

        let first = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("selecting the first character should advance without editing the document");
        assert!(first.edit.is_none());
        assert!(first.selection_to_remember.is_none());
        assert_eq!(first.after.phonetic(), "qthp");
        assert_eq!(first.after.shape_pinyin(), Some("hp"));
        assert_eq!(
            first.after.tab_assembly_stage(),
            Some(TabAssemblyStage::Second)
        );
        let display = first.candidate_display.as_ref().unwrap();
        assert_eq!(display.visible(), ["很", "和", "魂"]);
        assert_eq!(display.mode(), CandidateDisplayMode::ShapeAssemblySecond);
        assert_eq!(
            candidate_popup_mode_label(display),
            Some("雀 → 第 2 字 · hp · 形码 —")
        );
        assert!(matches!(
            first.feedback_after_success,
            Some(NativeFeedbackEvent::CandidatesPresentedWithProvenance {
                code,
                view: NativeCandidateView::Shape,
                loaded_candidates: 3,
                tab_assembly: Some(tab),
                ..
            }) if code == "qthp"
                && tab.position() == 2
                && tab.total_characters() == 2
                && tab.stroke_prefix().is_empty()
        ));
        *service.composition.borrow_mut() = first.after;

        let complete = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("selecting the second character should commit the assembled word");
        assert!(matches!(
            complete.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "雀魂"
        ));
        let remembered = complete
            .selection_to_remember
            .as_ref()
            .expect("only the complete explicit assembly should enter personal learning")
            .clone();
        assert_eq!(
            (remembered.code.as_str(), remembered.text.as_str()),
            ("qthp", "雀魂")
        );
        assert!(remembered.retractable_by_immediate_backspace);
        assert!(matches!(
            complete.feedback_after_success,
            Some(NativeFeedbackEvent::CandidateCommitted {
                code,
                text,
                view: NativeCandidateView::Shape,
                source: NativeSelectionSource::Numeric,
                absolute_rank: 3,
                visible_rank: 3,
            }) if code == "qthp" && text == "雀魂"
        ));
        assert!(complete.after.phonetic().is_empty());
        assert!(!complete.after.tab_mode());

        service.remember_selection_after_success_in_context(
            remembered,
            NativeFeedbackContext::Eligible,
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("qthp"),
            Some("雀魂"),
            "the completed path must be recalled immediately in this host"
        );
        let immediate = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            immediate.candidates.first().map(String::as_str),
            Some("雀魂")
        );
        assert_eq!(immediate.personalized.first(), Some(&true));

        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();
        let persistent = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            persistent.candidates.first().map(String::as_str),
            Some("雀魂"),
            "confirmed assembly evidence must remain recallable without session memory"
        );
        assert_eq!(
            persistent.personalized.first(),
            Some(&true),
            "persistent Tab recall should retain its quiet personal-memory marker"
        );

        let mut ordinary = CompositionSession::default();
        ordinary.apply(CompositionInput::Letters("qthp".to_owned()));
        *service.composition.borrow_mut() = ordinary;
        let enter_forget = service
            .plan_key(
                WPARAM(usize::from(VK_DELETE.0)),
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .expect("the recalled Tab word should enter forget selection");
        service.apply_candidate_forget_action(
            enter_forget
                .candidate_forget_action_after_success
                .expect("forget entry should carry an action"),
        );
        let choose = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .expect("the recalled Tab word should be forgettable");
        service.apply_candidate_forget_action(
            choose
                .candidate_forget_action_after_success
                .expect("forget selection should carry a suppression"),
        );
        assert!(
            service
                .personal_ranking
                .borrow()
                .is_suppressed("qthp", "雀魂")
        );
        let forgotten = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert!(forgotten.candidates.is_empty());
        assert!(forgotten.personalized.is_empty());

        let undo = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should restore the forgotten Tab word");
        service.apply_candidate_forget_action(
            undo.candidate_forget_action_after_success
                .expect("forget undo should carry a restore"),
        );
        let restored = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            restored.candidates.first().map(String::as_str),
            Some("雀魂")
        );
        assert_eq!(restored.personalized.first(), Some(&true));
    }

    #[test]
    fn eight_key_tab_path_shows_progress_and_learns_only_the_complete_word() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("qthplmxi".to_owned()));

        let enter = service
            .plan_key(WPARAM(usize::from(VK_TAB.0)), KeyModifiers::default())
            .unwrap()
            .expect("four complete single-character pools should enter staged Tab mode");
        assert_eq!(
            enter.after.tab_assembly_stage(),
            Some(TabAssemblyStage::First)
        );
        assert_eq!(
            candidate_popup_mode_label(enter.candidate_display.as_ref().unwrap()),
            Some("找第 1 字 · qt · 形码 —")
        );
        *service.composition.borrow_mut() = enter.after;

        for (expected_stage, expected_label) in [
            (TabAssemblyStage::Second, "雀 → 第 2 字 · hp · 形码 —"),
            (TabAssemblyStage::Later(3), "雀魂 → 第 3 字 · lm · 形码 —"),
            (TabAssemblyStage::Later(4), "雀魂练 → 第 4 字 · xi · 形码 —"),
        ] {
            let advance = service
                .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
                .unwrap()
                .expect("each explicit character should advance exactly one stage");
            assert!(advance.edit.is_none());
            assert!(advance.selection_to_remember.is_none());
            assert_eq!(advance.after.tab_assembly_stage(), Some(expected_stage));
            assert_eq!(
                candidate_popup_mode_label(advance.candidate_display.as_ref().unwrap()),
                Some(expected_label)
            );
            *service.composition.borrow_mut() = advance.after;
        }

        let complete = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("the fourth explicit character should commit the complete word");
        assert!(matches!(
            complete.edit,
            Some(PendingDocumentEdit::Commit(ref text)) if text == "雀魂练习"
        ));
        let learned = complete
            .selection_to_remember
            .expect("only the complete four-character path should enter learning");
        assert_eq!(learned.code, "qthplmxi");
        assert_eq!(learned.text, "雀魂练习");
        service
            .remember_selection_after_success_in_context(learned, NativeFeedbackContext::Eligible);
        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();

        let recalled = service
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthplmxi",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            recalled.candidates.first().map(String::as_str),
            Some("雀魂练习")
        );
        assert_eq!(recalled.personalized.first(), Some(&true));
    }

    #[test]
    fn immediate_backspace_retracts_an_unconfirmed_tab_assembled_word() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthp".to_owned()));
        assert!(session.enter_tab_path(&["qt", "hp"]));
        assert_eq!(
            session.accept_tab_assembly_candidate("雀", "qt"),
            Some(TabAssemblySelection::Advanced)
        );
        let complete = plan_tab_assembly_selection(
            &session,
            &CompositionInput::Select(3),
            Some("魂".to_owned()),
            Some("hp".to_owned()),
        )
        .expect("the second Tab character should complete one word");
        let selection = complete
            .selection_to_remember
            .expect("the complete Tab word should be eligible for learning");
        service.remember_selection_after_success_in_context(
            selection,
            NativeFeedbackContext::Eligible,
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("qthp"),
            Some("雀魂")
        );

        assert_eq!(
            service
                .resolve_pending_personal_selection_for_key(VK_BACK.0, KeyModifiers::default(),)
                .unwrap(),
            PendingPersonalKeyResolution::Retracted
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("qthp"),
            None
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("qthp"),
            None
        );
    }

    #[test]
    fn confirmed_tab_assembled_word_survives_a_new_service_with_its_personal_marker() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-tab-assembly-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let first = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        first
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root.clone())));
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthp".to_owned()));
        assert!(session.enter_tab_path(&["qt", "hp"]));
        assert_eq!(
            session.accept_tab_assembly_candidate("雀", "qt"),
            Some(TabAssemblySelection::Advanced)
        );
        let complete = plan_tab_assembly_selection(
            &session,
            &CompositionInput::Select(3),
            Some("魂".to_owned()),
            Some("hp".to_owned()),
        )
        .expect("the explicit Tab path should complete");
        first.remember_selection_after_success_in_context(
            complete
                .selection_to_remember
                .expect("the complete Tab path should learn only the whole word"),
            NativeFeedbackContext::Eligible,
        );
        assert!(first.confirm_pending_personal_selection());
        assert!(first.personal_ranking.borrow_mut().flush());
        drop(first);

        let second = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ShapeCandidateProvider,
        ))));
        second
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root)));
        let recalled = second
            .load_candidate_batch(
                &ShapeCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            recalled.candidates.first().map(String::as_str),
            Some("雀魂")
        );
        assert_eq!(recalled.personalized.first(), Some(&true));
        assert!(
            recalled.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        drop(second);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn repeated_tab_word_enters_a_guarded_and_forgettable_short_code_lane() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalShortRecallCandidateProvider,
        ))));
        assert!(service.personal_ranking.borrow_mut().record("qthp", "雀魂"));

        let once = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(once.candidates, ["固定", "其他", "雀跃", "去向"]);
        assert!(once.personalized.iter().all(|personalized| !personalized));

        let full = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(full.candidates, ["雀魂"]);
        assert_eq!(full.personalized, [true]);
        let mut full_session = CompositionSession::default();
        full_session.apply(CompositionInput::Letters("qthp".to_owned()));
        *service.composition.borrow_mut() = full_session;
        let reuse = service
            .plan_key(WPARAM(usize::from(VK_SPACE.0)), KeyModifiers::default())
            .unwrap()
            .expect("confirming a recalled personal whole word should be observable");
        let repeated_evidence = reuse
            .selection_to_remember
            .expect("a verified personal whole-word reuse should strengthen its evidence");
        assert_eq!(repeated_evidence.code, "qthp");
        assert_eq!(repeated_evidence.text, "雀魂");
        service.remember_selection_after_success_in_context(
            repeated_evidence,
            NativeFeedbackContext::Eligible,
        );
        assert!(service.confirm_pending_personal_selection());

        let discovered = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(discovered.candidates, ["固定", "其他", "雀魂", "雀跃"]);
        assert_eq!(discovered.personalized, [false, false, true, false]);
        assert!(
            discovered.provenance[2]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_DISCOVERY)
        );

        let mut short_session = CompositionSession::default();
        short_session.apply(CompositionInput::Letters("qth".to_owned()));
        *service.composition.borrow_mut() = short_session;
        let enter_forget = service
            .plan_key(
                WPARAM(usize::from(VK_DELETE.0)),
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .expect("the discovered short-code candidate should enter forget mode");
        service.apply_candidate_forget_action(
            enter_forget
                .candidate_forget_action_after_success
                .expect("forget entry should carry an action"),
        );
        let forget = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("the third visible candidate should be forgettable");
        service.apply_candidate_forget_action(
            forget
                .candidate_forget_action_after_success
                .expect("short-code forget should carry a suppression"),
        );
        let hidden = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(hidden.candidates, ["固定", "其他", "雀跃", "去向"]);
        let full_still_available = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qthp",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(full_still_available.candidates, ["雀魂"]);

        let undo = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should restore only the short-code view");
        service.apply_candidate_forget_action(
            undo.candidate_forget_action_after_success
                .expect("forget undo should carry a restore"),
        );
        let restored = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(restored.candidates, ["固定", "其他", "雀魂", "雀跃"]);

        let choose_short = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("an explicit short-code choice should use the ordinary learning path");
        let exact_short_evidence = choose_short
            .selection_to_remember
            .expect("the explicit discovery choice should establish exact short-code evidence");
        assert_eq!(exact_short_evidence.code, "qth");
        assert_eq!(exact_short_evidence.text, "雀魂");
        service.remember_selection_after_success_in_context(
            exact_short_evidence,
            NativeFeedbackContext::Eligible,
        );
        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();
        let adopted = service
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(adopted.candidates, ["固定", "雀魂", "其他", "雀跃"]);
        assert_eq!(adopted.personalized, [false, true, false, false]);
        assert!(
            adopted.provenance[1]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn repeated_personal_short_code_discovery_survives_a_new_service() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-short-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let first = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalShortRecallCandidateProvider,
        ))));
        first
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root.clone())));
        assert!(first.personal_ranking.borrow_mut().record("qthp", "雀魂"));
        assert!(first.personal_ranking.borrow_mut().record("qthp", "雀魂"));
        assert!(first.personal_ranking.borrow_mut().flush());
        drop(first);

        let second = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalShortRecallCandidateProvider,
        ))));
        second
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root)));
        let discovered = second
            .load_candidate_batch(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(discovered.candidates, ["固定", "其他", "雀魂", "雀跃"]);
        assert_eq!(discovered.personalized, [false, false, true, false]);
        assert!(
            discovered.provenance[2]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_DISCOVERY)
        );
        drop(second);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn automatic_transposition_never_opens_the_personal_short_code_discovery_lane() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalShortRecallCandidateProvider,
        ))));
        assert!(service.personal_ranking.borrow_mut().record("qthp", "雀魂"));
        assert!(service.personal_ranking.borrow_mut().record("qthp", "雀魂"));

        let batch = service
            .load_candidate_batch_with_automatic_transposition(
                &PersonalShortRecallCandidateProvider,
                "qth",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
                Some(reversed_single_pair_request(
                    AutomaticTranspositionTier::Primary,
                )),
            )
            .unwrap();
        assert_eq!(batch.candidates, ["固定", "其他", "雀跃", "去向"]);
        assert!(
            batch.personalized.iter().all(|personalized| !personalized),
            "one automatic correction request must not be treated as an explicit personal discovery"
        );
    }

    #[test]
    fn real_key_callbacks_recall_a_tab_assembled_word_with_feedback_stopped() {
        let _guard = test_lock();
        let _apartment = ComApartment::enter();
        let service_object = ComObject::new(TsfTextService::counted_for_process_test(Some(
            Arc::new(ShapeCandidateProvider),
        )));
        assert!(
            !service_object
                .native_feedback
                .lock()
                .unwrap()
                .is_accepting()
        );
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
                    .as_bool(),
                "OnTestKeyDown did not route virtual key {vkey}"
            );
            assert!(
                unsafe { key_sink.OnKeyDown(&context, key, lparam) }
                    .unwrap()
                    .as_bool(),
                "OnKeyDown did not handle virtual key {vkey}"
            );
        };
        let type_code = || {
            for letter in b"qthp" {
                press(VK_A.0 + u16::from(*letter - b'a'));
            }
        };

        type_code();
        press(VK_TAB.0);
        press(VK_1.0 + 2);
        assert_eq!(
            service_object
                .composition
                .borrow()
                .tab_assembly_selected_text()
                .as_deref(),
            Some("雀"),
        );
        press(VK_1.0 + 2);
        assert_eq!(read_context_text(&context, client_id), "雀魂");
        assert_eq!(
            service_object
                .selection_memory
                .borrow()
                .remembered_text("qthp"),
            Some("雀魂")
        );
        assert!(service_object.pending_personal_selection.borrow().is_some());

        type_code();
        press(VK_SPACE.0);
        assert_eq!(read_context_text(&context, client_id), "雀魂雀魂");
        assert_eq!(
            service_object
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("qthp"),
            Some("雀魂")
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
            Some(PlannedAction::Wish(InlineWishOperation::Command(
                WishCommand::Start
            )))
        );
        assert!(confirm.feedback_after_success.is_none());
    }

    #[test]
    fn recording_xuy_tab_immediately_captures_recent_episodes() {
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
        assert!(matches!(prompt.edit, Some(PendingDocumentEdit::Cancel)));
        assert!(prompt.candidate_display.is_none());
        assert!(prompt.after.phonetic().is_empty());
        assert!(!prompt.after.wish_prompt());
        assert_eq!(
            prompt.action_after_success,
            Some(PlannedAction::Wish(InlineWishOperation::Capture {
                scope: WishCaptureScope::RecentEpisodes,
                category: WishCategory::Other,
            }))
        );
    }

    #[test]
    fn episode_wish_keeps_older_context_outside_the_three_episode_focus_probe() {
        let mut feedback = NativeFeedbackSession::default();
        assert_eq!(
            feedback.start_rolling_memory(
                NativeFeedbackAuthorization::explicit_memory_only(),
                NativeFeedbackLimits::default(),
            ),
            NativeFeedbackStartResult::Started
        );
        for (index, code) in ["aa", "bb", "cc", "dd"].into_iter().enumerate() {
            let timestamp = 1_000 + u64::try_from(index).unwrap() * 1_000;
            assert_eq!(
                feedback.record_at(
                    NativeFeedbackContext::Eligible,
                    NativeFeedbackEvent::CandidatesPresented {
                        code: code.to_owned(),
                        view: NativeCandidateView::Ordinary,
                        page_start: 0,
                        candidates: vec!["甲".to_owned()],
                        may_have_more: false,
                    },
                    timestamp,
                ),
                NativeFeedbackRecordResult::Recorded
            );
            assert_eq!(
                feedback.record_at(
                    NativeFeedbackContext::Eligible,
                    NativeFeedbackEvent::CandidateCommitted {
                        code: code.to_owned(),
                        text: "甲".to_owned(),
                        view: NativeCandidateView::Ordinary,
                        source: NativeSelectionSource::FirstCandidate,
                        absolute_rank: 1,
                        visible_rank: 1,
                    },
                    timestamp + 1,
                ),
                NativeFeedbackRecordResult::Recorded
            );
        }
        assert_eq!(
            feedback.record_at(
                NativeFeedbackContext::Eligible,
                NativeFeedbackEvent::CandidatesPresented {
                    code: "xuy".to_owned(),
                    view: NativeCandidateView::Ordinary,
                    page_start: 0,
                    candidates: vec!["许愿".to_owned()],
                    may_have_more: false,
                },
                4_500,
            ),
            NativeFeedbackRecordResult::Recorded
        );

        let frozen = freeze_recent_wish_with_context(&feedback, 5_000)
            .unwrap()
            .unwrap();
        assert_eq!(frozen.source_events(), 9);
        assert_eq!(frozen.events().len(), 9);
        assert_eq!(frozen.omitted_before_window(), 0);
        let wish = WishSnapshot::from_frozen_with_metadata(
            &frozen,
            WishCaptureScope::RecentEpisodes,
            WishCategory::Other,
        )
        .unwrap();
        assert_eq!(wish.focus_event_range(), 6..8);
        assert_eq!(wish.event_role(0), Some(crate::WishEventRole::Context));
        assert_eq!(wish.event_role(8), Some(crate::WishEventRole::Trigger));
    }

    #[test]
    fn an_open_wish_prompt_still_allows_the_bounded_window_fallback() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test_with_feedback(
            Some(Arc::new(SelectionCandidateProvider)),
            NativeFeedbackLimits::default(),
        ));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("xuy".to_owned()));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::EnterWish);

        let fallback = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 1)), KeyModifiers::default())
            .unwrap()
            .expect("the second visible wish action should be selectable");
        assert!(matches!(fallback.edit, Some(PendingDocumentEdit::Cancel)));
        assert_eq!(
            fallback.action_after_success,
            Some(PlannedAction::Wish(InlineWishOperation::Capture {
                scope: WishCaptureScope::RecentWindow,
                category: WishCategory::Other,
            }))
        );

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
    fn ctrl_delete_enters_candidate_forget_mode_only_for_an_ordinary_composition() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        let shortcut = KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        };
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
                .unwrap()
                .is_none()
        );

        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        let enter = service
            .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
            .unwrap()
            .expect("Ctrl+Delete should enter explicit candidate selection");
        assert_eq!(enter.before, enter.after);
        assert!(enter.edit.is_none());
        assert_eq!(
            enter.candidate_display.as_ref().unwrap().mode(),
            CandidateDisplayMode::ForgetSelecting
        );
        assert!(matches!(
            enter.candidate_forget_action_after_success,
            Some(PlannedCandidateForgetAction::Enter)
        ));
        assert_eq!(
            service.personal_ranking.borrow().suppressions.entry_count(),
            0,
            "entering the mode must not write a suppression"
        );

        let shifted_shortcut = KeyModifiers {
            shift: true,
            control: true,
            ..KeyModifiers::default()
        };
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_DELETE.0)), shifted_shortcut)
                .unwrap()
                .is_none()
        );
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::EnterRecovery);
        assert!(
            service
                .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn candidate_forget_requires_personal_evidence_and_protects_fixed_prefixes() {
        let _guard = test_lock();
        let shortcut = KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        };
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        let enter = service
            .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
            .unwrap()
            .unwrap();
        service.apply_candidate_forget_action(enter.candidate_forget_action_after_success.unwrap());
        let public_only = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            public_only.candidate_display.as_ref().unwrap().mode(),
            CandidateDisplayMode::ForgetNotPersonal
        );
        assert!(matches!(
            public_only.candidate_forget_action_after_success,
            Some(PlannedCandidateForgetAction::Message(
                CandidateForgetMessage::NotPersonal
            ))
        ));
        assert_eq!(
            service.personal_ranking.borrow().suppressions.entry_count(),
            0
        );

        let protected = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ProtectedSelectionCandidateProvider,
        ))));
        protected
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        assert!(protected.personal_ranking.borrow_mut().record("ab", "甲"));
        let enter = protected
            .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
            .unwrap()
            .unwrap();
        protected
            .apply_candidate_forget_action(enter.candidate_forget_action_after_success.unwrap());
        let fixed = protected
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            fixed.candidate_display.as_ref().unwrap().mode(),
            CandidateDisplayMode::ForgetProtected
        );
        assert!(matches!(
            fixed.candidate_forget_action_after_success,
            Some(PlannedCandidateForgetAction::Message(
                CandidateForgetMessage::Protected
            ))
        ));
        assert!(
            !protected
                .personal_ranking
                .borrow()
                .is_suppressed("ab", "甲")
        );
    }

    #[test]
    fn candidate_forget_suppresses_session_evidence_and_backspace_restores_it() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        assert_eq!(
            service
                .native_feedback
                .lock()
                .unwrap()
                .start_rolling_memory(
                    NativeFeedbackAuthorization::explicit_memory_only(),
                    NativeFeedbackLimits::default(),
                ),
            NativeFeedbackStartResult::Started
        );
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        service
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "乙");
        let shortcut = KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        };
        let enter = service
            .plan_key(WPARAM(usize::from(VK_DELETE.0)), shortcut)
            .unwrap()
            .unwrap();
        service.apply_candidate_forget_action(enter.candidate_forget_action_after_success.unwrap());
        let suppress = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .expect("the session-promoted first candidate should be forgettable");
        let action = suppress.candidate_forget_action_after_success.unwrap();
        assert!(matches!(
            &action,
            PlannedCandidateForgetAction::Suppress {
                code,
                text,
                restore_session: true,
            } if code == "ab" && text == "乙"
        ));
        let forgotten = service
            .apply_candidate_forget_action(action)
            .expect("forgetting should immediately rebuild the visible page");
        assert_eq!(forgotten.mode(), CandidateDisplayMode::ForgetUndo);
        assert_eq!(forgotten.visible(), ["甲", "乙", "丙"]);
        assert!(service.personal_ranking.borrow().is_suppressed("ab", "乙"));
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            None
        );
        assert!(matches!(
            &*service.candidate_forget_state.borrow(),
            CandidateForgetState::UndoAvailable { .. }
        ));

        let restore = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("the first Backspace should undo the forget action");
        assert_eq!(restore.before, restore.after);
        assert!(restore.edit.is_none());
        let restored = service
            .apply_candidate_forget_action(restore.candidate_forget_action_after_success.unwrap())
            .expect("restoring should immediately rebuild the visible page");
        assert_eq!(restored.mode(), CandidateDisplayMode::ForgetRestored);
        assert_eq!(restored.visible(), ["乙", "甲", "丙"]);
        assert!(!service.personal_ranking.borrow().is_suppressed("ab", "乙"));
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            Some("乙")
        );
        assert!(matches!(
            &*service.candidate_forget_state.borrow(),
            CandidateForgetState::Inactive
        ));
        let feedback = service.native_feedback.lock().unwrap();
        let actions = feedback
            .events()
            .iter()
            .filter_map(|event| match event {
                NativeFeedbackEvent::CandidateSuppressionChanged { code, text, action } => {
                    Some((code.as_str(), text.as_str(), *action))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            [
                ("ab", "乙", NativeCandidateSuppressionAction::Suppress),
                ("ab", "乙", NativeCandidateSuppressionAction::Restore),
            ]
        );
    }

    #[test]
    fn candidate_forget_numeric_selection_uses_the_current_page_absolute_index() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PagedProtectedCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        assert!(service.personal_ranking.borrow_mut().record("ab", "候选7"));
        let enter = service
            .plan_key(
                WPARAM(usize::from(VK_DELETE.0)),
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .unwrap();
        service.apply_candidate_forget_action(enter.candidate_forget_action_after_success.unwrap());
        let next = service
            .plan_key(WPARAM(usize::from(VK_NEXT.0)), KeyModifiers::default())
            .unwrap()
            .expect("paging should remain available in forget mode");
        assert_eq!(next.after.candidate_page_start(), CANDIDATE_PAGE_SIZE);
        *service.composition.borrow_mut() = next.after;
        service.apply_candidate_forget_action(next.candidate_forget_action_after_success.unwrap());

        let selected = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(matches!(
            selected.candidate_forget_action_after_success,
            Some(PlannedCandidateForgetAction::Suppress {
                ref code,
                ref text,
                restore_session: false,
            }) if code == "ab" && text == "候选7"
        ));
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
        assert!(promoted.provenance[0].session_promoted());
        assert_eq!(
            promoted.provenance[0].source(),
            NativeCandidateSource::SupplementalExact
        );
        assert!(
            promoted.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_EXACT)
        );
        assert!(
            promoted.provenance[1..]
                .iter()
                .all(|item| !item.session_promoted())
        );

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
        let punctuation = service
            .plan_key(WPARAM(usize::from(VK_OEM_COMMA.0)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert!(punctuation.selection_to_remember.is_none());
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
    fn complete_code_personal_evidence_inherits_only_into_its_verified_anchored_tail() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            CodeFamilyCandidateProvider,
        ))));
        assert!(service.personal_ranking.borrow_mut().record("jdjd", "讲讲"));

        let inherited = service
            .load_candidate_batch(
                &CodeFamilyCandidateProvider,
                "jdj",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(inherited.candidates, ["讲讲", "简单", "降价"]);
        assert!(
            inherited
                .provenance
                .iter()
                .all(|item| !item.session_promoted())
        );
        assert!(
            inherited.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_ANCHORED)
        );

        let unrelated = service
            .load_candidate_batch(
                &CodeFamilyCandidateProvider,
                "jd",
                2,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(unrelated.candidates, ["讲", "将"]);

        assert!(service.personal_ranking.borrow_mut().record("jdj", "降价"));
        let exact = service
            .load_candidate_batch(
                &CodeFamilyCandidateProvider,
                "jdj",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(exact.candidates, ["降价", "简单", "讲讲"]);
        assert!(
            exact.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn one_verified_complete_word_gets_a_guarded_short_code_discovery_position() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ExactShortDiscoveryCandidateProvider,
        ))));
        assert!(service.personal_ranking.borrow_mut().record("jdjd", "讲讲"));
        service
            .selection_memory
            .borrow_mut()
            .remember_text("jdjd", "讲讲");

        let automatic = service
            .load_candidate_batch_with_automatic_transposition(
                &ExactShortDiscoveryCandidateProvider,
                "jdj",
                4,
                InteractiveCandidateView::Primary,
                Some(reversed_single_pair_request(
                    AutomaticTranspositionTier::Primary,
                )),
            )
            .unwrap();
        assert_eq!(automatic.candidates, ["固定", "简单", "降价", "降级"]);
        assert!(
            automatic
                .personalized
                .iter()
                .all(|personalized| !personalized)
        );

        let discovered = service
            .load_candidate_batch(
                &ExactShortDiscoveryCandidateProvider,
                "jdj",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(discovered.candidates, ["固定", "简单", "讲讲", "降价"]);
        assert_eq!(discovered.personalized, [false, false, true, false]);
        assert!(
            discovered.provenance[2]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_DISCOVERY)
        );
        assert!(
            !discovered.provenance[2]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_ANCHORED),
            "session inheritance must not pull a newly discovered word across the ordinary guard"
        );

        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("jdj".to_owned()));
        let choose_short = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("the guarded discovery position should be selectable");
        let exact_short_evidence = choose_short
            .selection_to_remember
            .expect("selecting the discovery position should establish short-code evidence");
        assert_eq!(exact_short_evidence.code, "jdj");
        assert_eq!(exact_short_evidence.text, "讲讲");
        service.remember_selection_after_success_in_context(
            exact_short_evidence,
            NativeFeedbackContext::Eligible,
        );
        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();

        let adopted = service
            .load_candidate_batch(
                &ExactShortDiscoveryCandidateProvider,
                "jdj",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(adopted.candidates, ["固定", "讲讲", "简单", "降价"]);
        assert_eq!(adopted.personalized, [false, true, false, false]);
        assert!(
            adopted.provenance[1]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn verified_long_word_discovery_is_scoped_for_provenance_forget_and_restore() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            LongExactShortDiscoveryCandidateProvider,
        ))));
        assert!(
            service
                .personal_ranking
                .borrow_mut()
                .record("abcdef", "甲乙丙")
        );
        assert!(
            service
                .personal_ranking
                .borrow_mut()
                .record("abcdefgh", "甲乙丙丁")
        );

        let automatic = service
            .load_candidate_batch_with_automatic_transposition(
                &LongExactShortDiscoveryCandidateProvider,
                "abceg",
                4,
                InteractiveCandidateView::Primary,
                Some(reversed_single_pair_request(
                    AutomaticTranspositionTier::Primary,
                )),
            )
            .unwrap();
        assert_eq!(automatic.candidates, ["固定", "普通", "其他", "末尾"]);
        assert!(
            automatic
                .personalized
                .iter()
                .all(|personalized| !personalized)
        );

        for (short_code, text) in [
            ("abce", "甲乙丙"),
            ("abcde", "甲乙丙"),
            ("abceg", "甲乙丙丁"),
            ("abcdeg", "甲乙丙丁"),
            ("abcdefg", "甲乙丙丁"),
        ] {
            let discovered = service
                .load_candidate_batch(
                    &LongExactShortDiscoveryCandidateProvider,
                    short_code,
                    4,
                    InteractiveCandidateView::Primary,
                )
                .unwrap();
            assert_eq!(discovered.candidates, ["固定", "普通", text, "其他"]);
            assert_eq!(discovered.personalized, [false, false, true, false]);
            assert!(
                discovered.provenance[2]
                    .personalization()
                    .contains(NativeCandidatePersonalization::PERSISTENT_DISCOVERY)
            );
        }

        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("abceg".to_owned()));
        *service.composition.borrow_mut() = session;
        let enter = service
            .plan_key(
                WPARAM(usize::from(VK_DELETE.0)),
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .expect("the long-word discovery should enter forget mode");
        service.apply_candidate_forget_action(
            enter
                .candidate_forget_action_after_success
                .expect("forget entry should carry an action"),
        );
        let suppress = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .expect("the third discovered candidate should be forgettable");
        let action = suppress
            .candidate_forget_action_after_success
            .expect("forget selection should carry a suppression");
        assert!(matches!(
            &action,
            PlannedCandidateForgetAction::Suppress {
                code,
                text,
                restore_session: false,
            } if code == "abceg" && text == "甲乙丙丁"
        ));
        service.apply_candidate_forget_action(action);

        let hidden = service
            .load_candidate_batch(
                &LongExactShortDiscoveryCandidateProvider,
                "abceg",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(hidden.candidates, ["固定", "普通", "其他", "末尾"]);
        let sibling = service
            .load_candidate_batch(
                &LongExactShortDiscoveryCandidateProvider,
                "abcdeg",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(sibling.candidates, ["固定", "普通", "甲乙丙丁", "其他"]);
        let full = service
            .load_candidate_batch(
                &LongExactShortDiscoveryCandidateProvider,
                "abcdefgh",
                1,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(full.candidates, ["甲乙丙丁"]);

        let restore = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should restore only the forgotten short-code view");
        service.apply_candidate_forget_action(
            restore
                .candidate_forget_action_after_success
                .expect("restore should carry an action"),
        );
        let restored = service
            .load_candidate_batch(
                &LongExactShortDiscoveryCandidateProvider,
                "abceg",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(restored.candidates, ["固定", "普通", "甲乙丙丁", "其他"]);
    }

    #[test]
    fn inherited_personal_marker_reuses_the_single_public_verification_decision() {
        let _guard = test_lock();
        let persistent = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            CodeFamilyCandidateProvider,
        ))));
        assert!(
            persistent
                .personal_ranking
                .borrow_mut()
                .record("jdjd", "讲讲")
        );
        let persistent_provider = CountingCodeFamilyCandidateProvider {
            exact_calls: AtomicUsize::new(0),
        };
        let candidates = persistent
            .load_candidate_batch(
                &persistent_provider,
                "jdj",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(candidates.candidates, ["讲讲", "简单", "降价"]);
        assert_eq!(candidates.personalized, [true, false, false]);
        assert!(
            candidates.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_ANCHORED)
        );
        assert_eq!(
            persistent_provider.exact_calls.load(Ordering::Relaxed),
            1,
            "the marker must reuse the promotion decision instead of verifying the same source again"
        );

        let session = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            CodeFamilyCandidateProvider,
        ))));
        session
            .selection_memory
            .borrow_mut()
            .remember_text("jdjd", "讲讲");
        let session_provider = CountingCodeFamilyCandidateProvider {
            exact_calls: AtomicUsize::new(0),
        };
        let candidates = session
            .load_candidate_batch(
                &session_provider,
                "jdj",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(candidates.candidates, ["讲讲", "简单", "降价"]);
        assert_eq!(candidates.personalized, [true, false, false]);
        assert!(
            candidates.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_ANCHORED)
        );
        assert_eq!(
            session_provider.exact_calls.load(Ordering::Relaxed),
            1,
            "session markers must also reuse the verified promotion index"
        );
    }

    #[test]
    fn inherited_session_choice_can_be_forgotten_and_restored_only_for_the_short_code() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            CodeFamilyCandidateProvider,
        ))));
        service
            .selection_memory
            .borrow_mut()
            .remember_text("jdjd", "讲讲");
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("jdj".to_owned()));

        let inherited = service
            .load_candidate_batch(
                &CodeFamilyCandidateProvider,
                "jdj",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(inherited.candidates, ["讲讲", "简单", "降价"]);
        assert!(inherited.provenance[0].session_promoted());
        assert!(
            inherited.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_ANCHORED)
        );

        let enter = service
            .plan_key(
                WPARAM(usize::from(VK_DELETE.0)),
                KeyModifiers {
                    control: true,
                    ..KeyModifiers::default()
                },
            )
            .unwrap()
            .unwrap();
        service.apply_candidate_forget_action(enter.candidate_forget_action_after_success.unwrap());
        let suppress = service
            .plan_key(WPARAM(usize::from(VK_1.0)), KeyModifiers::default())
            .unwrap()
            .expect("the inherited session candidate should be forgettable");
        let action = suppress.candidate_forget_action_after_success.unwrap();
        assert!(matches!(
            &action,
            PlannedCandidateForgetAction::Suppress {
                code,
                text,
                restore_session: false,
            } if code == "jdj" && text == "讲讲"
        ));
        let forgotten = service
            .apply_candidate_forget_action(action)
            .expect("forgetting the inherited view should rebuild the page");
        assert_eq!(forgotten.visible(), ["简单", "降价", "讲讲"]);
        assert_eq!(
            service.selection_memory.borrow().remembered_text("jdjd"),
            Some("讲讲")
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("jdj"),
            None
        );

        let restore = service
            .plan_key(WPARAM(usize::from(VK_BACK.0)), KeyModifiers::default())
            .unwrap()
            .expect("Backspace should restore only the abbreviated-code inheritance");
        let restored = service
            .apply_candidate_forget_action(restore.candidate_forget_action_after_success.unwrap())
            .expect("restoring the inherited view should rebuild the page");
        assert_eq!(restored.visible(), ["讲讲", "简单", "降价"]);
        assert!(
            !service
                .personal_ranking
                .borrow()
                .is_suppressed("jdj", "讲讲")
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("jdjd"),
            Some("讲讲")
        );
    }

    #[test]
    fn exact_personal_runtime_is_causal_retractable_code_isolated_and_suppression_first() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ExactIdentityAuditCandidateProvider,
        ))));

        let baseline = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(baseline.candidates, ["甲词", "乙词", "丙词"]);

        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "丙词".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            None,
            "the successful choice must remain pending until a later boundary"
        );
        let next_same_code = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(next_same_code.candidates, ["丙词", "甲词", "乙词"]);
        assert!(next_same_code.personalized[0]);
        let same_text_other_code = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "cd",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            same_text_other_code.candidates,
            ["甲词", "乙词", "丙词"],
            "session evidence must retain the exact code identity"
        );

        assert!(service.retract_pending_personal_selection());
        let after_retraction = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(after_retraction.candidates, ["甲词", "乙词", "丙词"]);

        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "丙词".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();
        let confirmed = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(confirmed.candidates, ["丙词", "甲词", "乙词"]);
        assert!(confirmed.personalized[0]);
        assert!(
            service
                .personal_phrase_composer
                .borrow()
                .components
                .is_empty(),
            "multi-character exact selections must not enter the adjacent-character phrase lane"
        );

        service
            .personal_ranking
            .borrow_mut()
            .suppressions
            .suppress("ab", "丙词")
            .unwrap();
        let suppressed = service
            .load_candidate_batch(
                &ExactIdentityAuditCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(suppressed.candidates, ["甲词", "乙词", "丙词"]);
        assert_eq!(suppressed.personalized, [false, false, false]);
    }

    #[test]
    fn persistent_personal_ranking_requires_an_eligible_input_scope() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        let selection = PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        };

        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Password);
        service.remember_selection_after_success(PlannedSelection {
            code: selection.code.clone(),
            text: selection.text.clone(),
            retractable_by_immediate_backspace: true,
        });
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            None
        );

        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(selection);
        assert!(service.pending_personal_selection.borrow().is_some());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            None
        );
        assert!(service.confirm_pending_personal_selection());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙")
        );

        service.selection_memory.borrow_mut().clear();
        let promoted = service
            .load_candidate_batch(
                &SelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(promoted.candidates, ["乙", "甲", "丙"]);
        assert!(!promoted.provenance[0].session_promoted());
        assert!(
            promoted.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn adjacent_explicit_single_character_choices_form_a_personal_phrase_once() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ui", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ui".to_owned(),
            text: "试".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ub", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ub".to_owned(),
            text: "手".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiub"),
            Some("试手"),
            "the phrase should be available in the current host immediately"
        );
        assert!(
            service
                .pending_personal_selection
                .borrow()
                .as_ref()
                .is_some_and(|pending| pending.phrase.is_some())
        );
        let immediate = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiub",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            immediate.candidates.first().map(String::as_str),
            Some("试手")
        );

        assert!(service.confirm_pending_personal_selection());
        service.selection_memory.borrow_mut().clear();
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            Some("试手"),
            "one surviving adjacent use should be enough to persist the phrase"
        );
        let persistent = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiub",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            persistent.candidates.first().map(String::as_str),
            Some("试手")
        );
        assert!(!persistent.provenance[0].session_promoted());
        assert!(
            persistent.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn adjacent_verified_characters_extend_through_two_three_and_four_character_phrases() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        remember_verified_personal_character(&service, "ui", "试");
        assert_eq!(personal_phrase_component_codes(&service), ["ui"]);
        remember_verified_personal_character(&service, "ub", "手");
        assert_eq!(personal_phrase_component_codes(&service), ["ui", "ub"]);
        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiub"),
            Some("试手")
        );

        remember_verified_personal_character(&service, "lm", "练");
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            Some("试手"),
            "the two-character prefix must be confirmed before extending it"
        );
        assert_eq!(
            personal_phrase_component_codes(&service),
            ["ui", "ub", "lm"]
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiublm"),
            Some("试手练")
        );

        remember_verified_personal_character(&service, "xi", "习");
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiublm"),
            Some("试手练"),
            "the three-character prefix must be confirmed before extending it"
        );
        assert_eq!(
            personal_phrase_component_codes(&service),
            ["ui", "ub", "lm", "xi"]
        );
        assert_eq!(
            service
                .selection_memory
                .borrow()
                .remembered_text("uiublmxi"),
            Some("试手练习")
        );
        assert!(service.confirm_pending_personal_selection());

        service.selection_memory.borrow_mut().clear();
        let persistent = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiublmxi",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            persistent.candidates.first().map(String::as_str),
            Some("试手练习")
        );
        assert!(
            persistent.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
    }

    #[test]
    fn immediate_backspace_restores_the_prefix_chain_for_three_and_four_character_phrases() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        for (code, text) in [("ui", "试"), ("ub", "手"), ("lm", "练")] {
            remember_verified_personal_character(&service, code, text);
        }
        assert!(service.retract_pending_personal_selection());
        assert_eq!(personal_phrase_component_codes(&service), ["ui", "ub"]);
        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiublm"),
            None
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            Some("试手"),
            "retracting the third character must not retract the confirmed two-character prefix"
        );

        remember_verified_personal_character(&service, "lm", "练");
        remember_verified_personal_character(&service, "xi", "习");
        assert!(service.retract_pending_personal_selection());
        assert_eq!(
            personal_phrase_component_codes(&service),
            ["ui", "ub", "lm"]
        );
        assert_eq!(
            service
                .selection_memory
                .borrow()
                .remembered_text("uiublmxi"),
            None
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiublm"),
            Some("试手练"),
            "retracting the fourth character must retain the confirmed three-character prefix"
        );
    }

    #[test]
    fn a_fifth_verified_character_restarts_instead_of_learning_overlapping_phrases() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        for (code, text) in [
            ("ui", "试"),
            ("ub", "手"),
            ("lm", "练"),
            ("xi", "习"),
            ("aa", "啊"),
        ] {
            remember_verified_personal_character(&service, code, text);
        }

        assert_eq!(personal_phrase_component_codes(&service), ["aa"]);
        assert!(
            service
                .pending_personal_selection
                .borrow()
                .as_ref()
                .is_some_and(|pending| pending.phrase.is_none())
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiublmxi"),
            Some("试手练习")
        );
        assert_eq!(
            service
                .selection_memory
                .borrow()
                .remembered_text("ublmxiaa"),
            None,
            "the fifth character must not create a sliding four-character phrase"
        );
        assert_eq!(
            service
                .selection_memory
                .borrow()
                .remembered_text("uiublmxiaa"),
            None,
            "five-character phrase learning is outside the bounded lifecycle"
        );
        assert!(service.retract_pending_personal_selection());
        assert_eq!(
            personal_phrase_component_codes(&service),
            ["ui", "ub", "lm", "xi"],
            "deleting the fifth character restores the bounded prefix that remains in the document"
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("aa"),
            None,
            "the retracted fifth character must not leave its own personal evidence"
        );
    }

    #[test]
    fn an_unverified_character_breaks_a_longer_personal_phrase_chain() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        remember_verified_personal_character(&service, "ui", "试");
        remember_verified_personal_character(&service, "ub", "手");
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "lm".to_owned(),
                text: "林".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        assert!(personal_phrase_component_codes(&service).is_empty());
        assert!(
            service
                .pending_personal_selection
                .borrow()
                .as_ref()
                .is_some_and(|pending| pending.phrase.is_none())
        );

        remember_verified_personal_character(&service, "xi", "习");
        assert_eq!(personal_phrase_component_codes(&service), ["xi"]);
        assert_eq!(
            service
                .selection_memory
                .borrow()
                .remembered_text("uiublmxi"),
            None
        );
    }

    #[test]
    fn repeated_four_character_composition_opens_guarded_tail_discovery_and_respects_suppression() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        for round in 0..2 {
            for (code, text) in [("ui", "试"), ("ub", "手"), ("lm", "练"), ("xi", "习")] {
                remember_verified_personal_character(&service, code, text);
            }
            assert!(service.confirm_pending_personal_selection());
            if round == 0 {
                let single_use = service
                    .load_candidate_batch(
                        &PersonalPhraseCandidateProvider,
                        "uiulx",
                        4,
                        InteractiveCandidateView::Primary,
                    )
                    .unwrap();
                assert!(
                    !single_use.candidates.iter().any(|text| text == "试手练习"),
                    "one adjacent composition must not open pool-external short-code discovery"
                );
            }
        }
        service.selection_memory.borrow_mut().clear();

        for short_code in ["uiulx", "uiublx", "uiublmx"] {
            let discovered = service
                .load_candidate_batch(
                    &PersonalPhraseCandidateProvider,
                    short_code,
                    4,
                    InteractiveCandidateView::Primary,
                )
                .unwrap();
            assert_eq!(discovered.candidates, ["固定", "普通", "试手练习"]);
            assert!(
                discovered.provenance[2]
                    .personalization()
                    .contains(NativeCandidatePersonalization::PERSISTENT_DISCOVERY)
            );
        }

        {
            let mut ranking = service.personal_ranking.borrow_mut();
            assert!(ranking.suppressions.suppress("uiulx", "试手练习").unwrap());
        }
        let short_suppressed = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiulx",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert!(
            !short_suppressed
                .candidates
                .iter()
                .any(|text| text == "试手练习")
        );
        assert!(
            service
                .personal_ranking
                .borrow_mut()
                .suppressions
                .restore("uiulx", "试手练习")
                .unwrap()
        );

        {
            let mut ranking = service.personal_ranking.borrow_mut();
            assert!(
                ranking
                    .suppressions
                    .suppress("uiublmxi", "试手练习")
                    .unwrap()
            );
        }
        let source_suppressed = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiublx",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert!(
            !source_suppressed
                .candidates
                .iter()
                .any(|text| text == "试手练习")
        );
        assert!(
            service
                .personal_ranking
                .borrow_mut()
                .suppressions
                .restore("uiublmxi", "试手练习")
                .unwrap()
        );
        let restored = service
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiublx",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(restored.candidates[2], "试手练习");
    }

    #[test]
    fn confirmed_left_context_overrides_global_recency_only_for_the_matching_context() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalContextCandidateProvider,
        ))));

        service.set_personal_left_context("请");
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "把".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        service.set_personal_left_context("把");
        assert!(service.confirm_pending_personal_selection());

        service.set_personal_left_context("好");
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "吧".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        service.set_personal_left_context("吧");
        assert!(service.confirm_pending_personal_selection());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("吧")
        );

        service.set_personal_left_context("请");
        let matching = service
            .load_candidate_batch(
                service.candidate_provider.as_ref().unwrap().as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(matching.candidates.len(), CANDIDATE_PAGE_SIZE);
        assert_eq!(matching.candidates[0], "把");
        assert!(matching.may_have_more);
        assert!(matching.provenance[0].session_promoted());
        assert!(
            matching.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::LEFT_CONTEXT)
        );
        assert!(
            matching.provenance[0]
                .ranking_personalization()
                .contains(NativeCandidatePersonalization::LEFT_CONTEXT)
        );
        assert!(
            matching.provenance[1]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        assert!(
            matching.provenance[1].ranking_personalization().is_empty(),
            "persistent evidence that already matched the public top must not claim the later context reorder"
        );
        let NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            candidates,
            provenance,
            view,
            ..
        } = CandidateDisplay::from_batch(matching.clone(), 0).feedback_event("ab", false)
        else {
            panic!("ordinary candidate feedback must preserve provenance");
        };
        assert_eq!(view, NativeCandidateView::Ordinary);
        assert_eq!(candidates.first().map(String::as_str), Some("把"));
        assert!(
            provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::LEFT_CONTEXT),
            "the exact context cause must survive the candidate-display feedback boundary"
        );

        service.set_personal_left_context("无关");
        let unrelated = service
            .load_candidate_batch(
                service.candidate_provider.as_ref().unwrap().as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(unrelated.candidates[0], "吧");
    }

    #[test]
    fn explicit_later_selection_overrules_only_the_first_unprotected_candidate() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            ProtectedSelectionCandidateProvider,
        ))));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));

        let plan = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 2)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.selection_to_remember
                .as_ref()
                .map(|selection| selection.text.as_str()),
            Some("丙")
        );
        assert_eq!(plan.overruled_text_to_remember.as_deref(), Some("乙"));
        assert_ne!(
            plan.overruled_text_to_remember.as_deref(),
            Some("甲"),
            "the protected fixed candidate must never become negative evidence"
        );
    }

    #[test]
    fn repeated_context_overrides_are_gentle_and_immediate_backspace_retracts_them() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalContextCandidateProvider,
        ))));
        for _ in 0..4 {
            assert!(service.personal_ranking.borrow_mut().record("ab", "吧"));
            service
                .personal_context_ranking
                .borrow_mut()
                .record("请", "ab", "吧")
                .unwrap();
        }
        service.set_personal_left_context("请");
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));
        let plan = service
            .plan_key(WPARAM(usize::from(VK_1.0 + 1)), KeyModifiers::default())
            .unwrap()
            .unwrap();
        let selection = plan.selection_to_remember.unwrap();
        let overruled = plan.overruled_text_to_remember;
        assert_eq!(selection.text, "八");
        assert_eq!(overruled.as_deref(), Some("吧"));

        service.remember_selection_after_success_in_context_with_overrule(
            selection.clone(),
            NativeFeedbackContext::Eligible,
            overruled.clone(),
        );
        service.set_personal_left_context("八");
        assert!(service.retract_pending_personal_selection());
        service.set_personal_left_context("请");
        let after_retraction = service
            .load_candidate_batch(
                service.candidate_provider.as_ref().unwrap().as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(after_retraction.candidates[0], "吧");

        for expected in ["吧", "八"] {
            service.set_personal_left_context("请");
            service.remember_selection_after_success_in_context_with_overrule(
                selection.clone(),
                NativeFeedbackContext::Eligible,
                overruled.clone(),
            );
            service.set_personal_left_context("八");
            assert!(service.confirm_pending_personal_selection());
            service.set_personal_left_context("请");
            let reranked = service
                .load_candidate_batch(
                    service.candidate_provider.as_ref().unwrap().as_ref(),
                    "ab",
                    CANDIDATE_PAGE_SIZE,
                    InteractiveCandidateView::Primary,
                )
                .unwrap();
            assert_eq!(reranked.candidates[0], expected);
        }
    }

    #[test]
    fn immediate_retraction_restores_left_context_without_training_the_context_table() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalContextCandidateProvider,
        ))));
        service.set_personal_left_context("请");
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "把".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        service.set_personal_left_context("把");

        assert!(service.retract_pending_personal_selection());
        assert_eq!(
            service.personal_left_context.borrow().as_deref(),
            Some("请")
        );
        assert!(
            !service
                .personal_context_ranking
                .borrow()
                .has_evidence("请", "ab")
        );
    }

    #[test]
    fn exact_forget_suppression_wins_over_matching_context_evidence() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalContextCandidateProvider,
        ))));
        service.set_personal_left_context("请");
        service.remember_selection_after_success_in_context(
            PlannedSelection {
                code: "ab".to_owned(),
                text: "把".to_owned(),
                retractable_by_immediate_backspace: true,
            },
            NativeFeedbackContext::Eligible,
        );
        service.set_personal_left_context("把");
        assert!(service.confirm_pending_personal_selection());
        service
            .personal_ranking
            .borrow_mut()
            .suppressions
            .suppress("ab", "把")
            .unwrap();

        service.set_personal_left_context("请");
        let suppressed = service
            .load_candidate_batch(
                service.candidate_provider.as_ref().unwrap().as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(suppressed.candidates[0], "吧");
        assert_eq!(
            service.candidate_cache.borrow().requested_limit,
            CANDIDATE_PAGE_SIZE,
            "suppressed context evidence must not decode a deeper candidate page"
        );
    }

    #[test]
    fn personal_phrase_requires_verified_complete_single_character_components() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ui", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ui".to_owned(),
            text: "是".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ub", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ub".to_owned(),
            text: "手".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiub"),
            None
        );
        assert!(
            service
                .pending_personal_selection
                .borrow()
                .as_ref()
                .is_some_and(|pending| pending.phrase.is_none())
        );
        assert!(service.confirm_pending_personal_selection());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            None
        );
    }

    #[test]
    fn personal_phrase_range_and_keyboard_fallbacks_preserve_bounded_continuation() {
        let _guard = test_lock();
        for adjacency in [
            PersonalPhraseDocumentAdjacency::KeyboardFallback,
            PersonalPhraseDocumentAdjacency::RangeUnavailable,
        ] {
            let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
                PersonalPhraseCandidateProvider,
            ))));
            let first = service
                .remember_selection_after_success_in_context_with_overrule_and_document(
                    PlannedSelection {
                        code: "ui".to_owned(),
                        text: "试".to_owned(),
                        retractable_by_immediate_backspace: true,
                    },
                    NativeFeedbackContext::Eligible,
                    None,
                    PersonalPhraseDocumentAdjacency::NoPreviousAnchor,
                    PersonalPhraseDocumentSnapshot::default(),
                )
                .unwrap();
            assert_eq!(first.previous_components, 0);
            assert_eq!(first.resulting_components, 1);

            let second = service
                .remember_selection_after_success_in_context_with_overrule_and_document(
                    PlannedSelection {
                        code: "ub".to_owned(),
                        text: "手".to_owned(),
                        retractable_by_immediate_backspace: true,
                    },
                    NativeFeedbackContext::Eligible,
                    None,
                    adjacency,
                    PersonalPhraseDocumentSnapshot::default(),
                )
                .unwrap();
            assert_eq!(second.adjacency, adjacency);
            assert_eq!(second.previous_components, 1);
            assert_eq!(second.resulting_components, 2);
            assert_eq!(
                service.selection_memory.borrow().remembered_text("uiub"),
                Some("试手")
            );
        }
    }

    #[test]
    fn immediate_backspace_retracts_a_new_personal_phrase_but_keeps_its_first_component() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ui", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ui".to_owned(),
            text: "试".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ub", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ub".to_owned(),
            text: "手".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        assert_eq!(
            service
                .resolve_pending_personal_selection_for_key(VK_BACK.0, KeyModifiers::default())
                .unwrap(),
            PendingPersonalKeyResolution::Retracted
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("uiub"),
            None
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiub"),
            None
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ui"),
            Some("试"),
            "the first component crossed its own confirmation boundary"
        );
        assert!(
            service
                .personal_phrase_composer
                .borrow()
                .components
                .first()
                .is_some_and(|component| component.code == "ui" && component.text == "试")
        );
    }

    #[test]
    fn focus_loss_confirms_a_four_character_phrase_and_breaks_the_adjacency_chain() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));

        for (code, text) in [("ui", "试"), ("ub", "手"), ("lm", "练"), ("xi", "习")] {
            service
                .native_feedback_context
                .lock()
                .unwrap()
                .remember(code, NativeFeedbackContext::Eligible);
            service.remember_selection_after_success(PlannedSelection {
                code: code.to_owned(),
                text: text.to_owned(),
                retractable_by_immediate_backspace: true,
            });
        }

        seed_personal_phrase_document_fallback(&service);

        service.cleanup_after_focus_loss().unwrap();
        assert_personal_phrase_document_tracker_cleared(&service);
        assert!(service.pending_personal_selection.borrow().is_none());
        assert!(
            service
                .personal_phrase_composer
                .borrow()
                .components
                .is_empty()
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("uiublmxi"),
            Some("试手练习")
        );
    }

    #[test]
    fn one_confirmed_personal_phrase_survives_a_new_service() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-phrase-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let first = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));
        first
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root.clone())));
        for (code, text) in [("ui", "试"), ("ub", "手")] {
            first
                .native_feedback_context
                .lock()
                .unwrap()
                .remember(code, NativeFeedbackContext::Eligible);
            first.remember_selection_after_success(PlannedSelection {
                code: code.to_owned(),
                text: text.to_owned(),
                retractable_by_immediate_backspace: true,
            });
        }
        assert!(first.confirm_pending_personal_selection());
        assert!(first.personal_ranking.borrow_mut().flush());
        drop(first);

        let second = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));
        second
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root)));
        let promoted = second
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiub",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            promoted.candidates.first().map(String::as_str),
            Some("试手")
        );
        assert!(!promoted.provenance[0].session_promoted());
        assert!(
            promoted.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        drop(second);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn one_confirmed_four_character_personal_phrase_survives_a_new_service() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-long-phrase-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let first = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));
        first
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root.clone())));
        for (code, text) in [("ui", "试"), ("ub", "手"), ("lm", "练"), ("xi", "习")] {
            remember_verified_personal_character(&first, code, text);
        }
        assert!(first.confirm_pending_personal_selection());
        assert!(first.personal_ranking.borrow_mut().flush());
        drop(first);

        let second = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            PersonalPhraseCandidateProvider,
        ))));
        second
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root)));
        let promoted = second
            .load_candidate_batch(
                &PersonalPhraseCandidateProvider,
                "uiublmxi",
                4,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            promoted.candidates.first().map(String::as_str),
            Some("试手练习")
        );
        assert!(!promoted.provenance[0].session_promoted());
        assert!(
            promoted.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        drop(second);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn immediate_backspace_retracts_pending_personal_evidence_and_session_override() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service
            .selection_memory
            .borrow_mut()
            .remember_text("ab", "丙");
        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        assert!(service.should_route_pending_personal_key_down().unwrap());
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            Some("乙")
        );
        assert_eq!(
            service
                .resolve_pending_personal_selection_for_key(VK_BACK.0, KeyModifiers::default())
                .unwrap(),
            PendingPersonalKeyResolution::Retracted
        );
        assert!(service.pending_personal_selection.borrow().is_none());
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            Some("丙")
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            None
        );
    }

    #[test]
    fn a_following_key_confirms_pending_personal_evidence() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });

        assert_eq!(
            service
                .resolve_pending_personal_selection_for_key(VK_A.0, KeyModifiers::default())
                .unwrap(),
            PendingPersonalKeyResolution::Confirmed
        );
        assert!(service.pending_personal_selection.borrow().is_none());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙")
        );
    }

    #[test]
    fn confirmed_support_drops_one_incidental_session_override_but_allows_repetition() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        {
            let mut ranking = service.personal_ranking.borrow_mut();
            assert!(ranking.record("ab", "甲"));
            assert!(ranking.record("ab", "甲"));
        }

        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            Some("乙"),
            "the successful choice should take effect before its confirmation boundary"
        );
        assert!(service.confirm_pending_personal_selection());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("甲")
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            None
        );

        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        assert!(service.confirm_pending_personal_selection());
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙"),
            "repeated deliberate choices must still be able to change the preference"
        );
        assert_eq!(
            service.selection_memory.borrow().remembered_text("ab"),
            Some("乙")
        );
    }

    #[test]
    fn focus_loss_confirms_pending_personal_evidence() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service.set_personal_left_context("前");
        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        *service.candidate_forget_state.borrow_mut() = CandidateForgetState::UndoAvailable {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            restore_session: false,
        };

        service.cleanup_after_focus_loss().unwrap();
        assert!(service.pending_personal_selection.borrow().is_none());
        assert!(matches!(
            &*service.candidate_forget_state.borrow(),
            CandidateForgetState::Inactive
        ));
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙")
        );
        assert!(service.personal_left_context.borrow().is_none());
        assert!(
            service
                .personal_context_ranking
                .borrow()
                .has_evidence("前", "ab")
        );
    }

    #[test]
    fn backspace_after_punctuation_confirmation_keeps_candidate_evidence() {
        let _guard = test_lock();
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        service
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        service.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: false,
        });

        assert_eq!(
            service
                .resolve_pending_personal_selection_for_key(VK_BACK.0, KeyModifiers::default())
                .unwrap(),
            PendingPersonalKeyResolution::Confirmed
        );
        assert_eq!(
            service
                .personal_ranking
                .borrow()
                .snapshot
                .preferred_text("ab"),
            Some("乙")
        );
    }

    #[test]
    fn eligible_selection_survives_a_new_service_without_a_session_provenance_tag() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _guard = test_lock();
        let parent = std::env::temp_dir().join(format!(
            "ziranma-tsf-personal-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&parent).unwrap();
        let root = parent.join("ranking");

        let first = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        first
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root.clone())));
        first
            .native_feedback_context
            .lock()
            .unwrap()
            .remember("ab", NativeFeedbackContext::Eligible);
        first.remember_selection_after_success(PlannedSelection {
            code: "ab".to_owned(),
            text: "乙".to_owned(),
            retractable_by_immediate_backspace: true,
        });
        assert!(first.confirm_pending_personal_selection());
        assert!(first.personal_ranking.borrow_mut().flush());
        drop(first);

        let second = ComObject::new(TsfTextService::counted_for_process_test(Some(Arc::new(
            SelectionCandidateProvider,
        ))));
        second
            .personal_ranking
            .replace(PersonalRankingRuntime::new(Some(root)));
        let promoted = second
            .load_candidate_batch(
                &SelectionCandidateProvider,
                "ab",
                3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(promoted.candidates, ["乙", "甲", "丙"]);
        assert!(!promoted.provenance[0].session_promoted());
        assert!(
            promoted.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        drop(second);

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn candidate_batch_reuses_the_decoded_protected_prefix_during_selection() {
        let _guard = test_lock();
        let provider = Arc::new(CountingProtectedPrefixCandidateProvider {
            candidate_calls: AtomicUsize::new(0),
            protected_prefix_calls: AtomicUsize::new(0),
        });
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            provider.clone(),
        )));
        service
            .composition
            .borrow_mut()
            .apply(CompositionInput::Letters("ab".to_owned()));

        let first = service
            .load_candidate_batch(
                provider.as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(first.protected_prefix_len, 1);
        assert_eq!(provider.candidate_calls.load(Ordering::Relaxed), 1);
        assert_eq!(provider.protected_prefix_calls.load(Ordering::Relaxed), 1);

        let repeated = service
            .load_candidate_batch(
                provider.as_ref(),
                "ab",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(repeated.protected_prefix_len, 1);
        assert_eq!(provider.candidate_calls.load(Ordering::Relaxed), 1);
        assert_eq!(provider.protected_prefix_calls.load(Ordering::Relaxed), 1);

        let selection = service
            .plan_key(
                WPARAM(usize::from(VK_1.0.saturating_add(2))),
                KeyModifiers::default(),
            )
            .unwrap()
            .expect("the third candidate should remain selectable");
        assert_eq!(
            selection
                .selection_to_remember
                .as_ref()
                .map(|selection| selection.text.as_str()),
            Some("乙")
        );
        assert_eq!(selection.overruled_text_to_remember.as_deref(), Some("甲"));
        assert_eq!(provider.candidate_calls.load(Ordering::Relaxed), 1);
        assert_eq!(provider.protected_prefix_calls.load(Ordering::Relaxed), 1);
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
    fn optional_exact_short_layer_freezes_candidate_and_provenance_prefixes() {
        let provider = ExactShortPagingCandidateProvider::new();
        let mut cache = CandidateCache::default();

        let first = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            first.candidates,
            ExactShortPagingCandidateProvider::primary_candidates(6)
        );
        assert_eq!(provider.layer_requests.load(Ordering::Relaxed), 0);

        let second = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 2,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(&second.candidates[..6], first.candidates.as_slice());
        assert_eq!(&second.candidates[6..8], ["收束", "手术"]);
        assert_eq!(
            second.provenance[6].source(),
            NativeCandidateSource::PublicConsensusExact
        );
        assert_eq!(
            second.provenance[7].source(),
            NativeCandidateSource::PublicConsensusExact
        );
        assert!(
            second
                .provenance
                .iter()
                .enumerate()
                .all(|(index, provenance)| index == 6
                    || index == 7
                    || provenance.source() == NativeCandidateSource::Decoder)
        );
        assert!(second.personalized.iter().all(|personalized| !personalized));
        assert!(second.may_have_more);
        assert_eq!(provider.layer_requests.load(Ordering::Relaxed), 1);

        let third = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 3,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            &third.candidates[..second.candidates.len()],
            second.candidates.as_slice()
        );
        assert_eq!(
            &third.provenance[..second.provenance.len()],
            second.provenance.as_slice()
        );
        assert!(third.may_have_more);

        let deepest = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_LIMIT,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            &deepest.candidates[..third.candidates.len()],
            third.candidates.as_slice()
        );
        assert_eq!(
            &deepest.provenance[..third.provenance.len()],
            third.provenance.as_slice()
        );
        assert_eq!(deepest.candidates.len(), CANDIDATE_LIMIT);
        assert!(!deepest.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 4);
        assert_eq!(provider.layer_requests.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn enabled_exact_short_layer_disappearance_preserves_the_last_presented_page() {
        let provider = ExactShortPagingCandidateProvider::new();
        let mut cache = CandidateCache::default();
        cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE,
            InteractiveCandidateView::Primary,
        );
        let second = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 2,
            InteractiveCandidateView::Primary,
        );
        provider.layer_enabled.store(false, Ordering::Relaxed);

        let failed_extension = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 3,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            failed_extension.candidates.as_slice(),
            second.candidates.as_slice()
        );
        assert_eq!(
            failed_extension.provenance.as_slice(),
            second.provenance.as_slice()
        );
        assert!(!failed_extension.may_have_more);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 3);

        let repeated = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 3,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(repeated.candidates.as_slice(), second.candidates.as_slice());
        assert_eq!(provider.calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn disabled_exact_short_decision_cannot_appear_mid_composition() {
        let provider = ExactShortPagingCandidateProvider::new();
        let mut cache = CandidateCache::default();
        cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE,
            InteractiveCandidateView::Primary,
        );
        provider.layer_enabled.store(false, Ordering::Relaxed);
        let second = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 2,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            second.candidates,
            ExactShortPagingCandidateProvider::primary_candidates(12)
        );

        provider.layer_enabled.store(true, Ordering::Relaxed);
        let third = cache.load(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 3,
            InteractiveCandidateView::Primary,
        );
        assert_eq!(
            third.candidates,
            ExactShortPagingCandidateProvider::primary_candidates(18)
        );
        assert!(
            third
                .provenance
                .iter()
                .all(|provenance| provenance.source() == NativeCandidateSource::Decoder)
        );
        assert_eq!(provider.layer_requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exact_short_pages_remain_aligned_after_session_personalization() {
        let _guard = test_lock();
        let provider = Arc::new(ExactShortPagingCandidateProvider::new());
        let service = ComObject::new(TsfTextService::counted_for_process_test(Some(
            provider.clone(),
        )));
        service
            .selection_memory
            .borrow_mut()
            .remember_text("ubuu", "基础3");

        let first = service
            .load_candidate_batch(
                provider.as_ref(),
                "ubuu",
                CANDIDATE_PAGE_SIZE,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(first.candidates[0], "基础3");
        assert!(first.personalized[0]);
        assert!(
            first.provenance[0]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_EXACT)
        );

        let second = service
            .load_candidate_batch(
                provider.as_ref(),
                "ubuu",
                CANDIDATE_PAGE_SIZE * 2,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            &second.candidates[..first.candidates.len()],
            first.candidates.as_slice()
        );
        assert_eq!(
            &second.provenance[..first.provenance.len()],
            first.provenance.as_slice()
        );
        for exact in ["收束", "手术"] {
            let index = second
                .candidates
                .iter()
                .position(|candidate| candidate == exact)
                .unwrap();
            assert_eq!(
                second.provenance[index].source(),
                NativeCandidateSource::PublicConsensusExact
            );
            assert!(!second.personalized[index]);
        }

        let third = service
            .load_candidate_batch(
                provider.as_ref(),
                "ubuu",
                CANDIDATE_PAGE_SIZE * 3,
                InteractiveCandidateView::Primary,
            )
            .unwrap();
        assert_eq!(
            &third.candidates[..second.candidates.len()],
            second.candidates.as_slice()
        );
        assert_eq!(
            &third.provenance[..second.provenance.len()],
            second.provenance.as_slice()
        );
        assert_eq!(
            &third.personalized[..second.personalized.len()],
            second.personalized.as_slice()
        );
    }

    #[test]
    fn second_page_exact_words_do_not_retract_first_page_automatic_recovery() {
        let provider = ExactShortPagingCandidateProvider::new();
        let mut cache = CandidateCache::default();
        let request = reversed_single_pair_request(AutomaticTranspositionTier::Primary);

        let first = cache.load_with_automatic_transposition(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE,
            InteractiveCandidateView::Primary,
            Some(request),
        );
        assert_eq!(first.candidates[0], "换序恢复");
        assert_eq!(
            first.provenance[0].source(),
            NativeCandidateSource::TranspositionRecovery
        );

        let second = cache.load_with_automatic_transposition(
            &provider,
            "ubuu",
            CANDIDATE_PAGE_SIZE * 2,
            InteractiveCandidateView::Primary,
            Some(request),
        );
        assert_eq!(
            &second.candidates[..first.candidates.len()],
            first.candidates.as_slice()
        );
        assert_eq!(
            &second.provenance[..first.provenance.len()],
            first.provenance.as_slice()
        );
        assert!(
            second
                .candidates
                .iter()
                .any(|candidate| candidate == "收束")
        );
        assert!(
            second
                .candidates
                .iter()
                .any(|candidate| candidate == "手术")
        );
        assert_eq!(
            second
                .automatic_transposition
                .as_ref()
                .map(NativeAutomaticTranspositionDecision::outcome),
            Some(NativeAutomaticTranspositionOutcome::RecoveryAvailable)
        );
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
        let actions = inline_wish_actions(NativeFeedbackSummary::default());
        let display = CandidateDisplay::actions(&actions);
        let metrics = candidate_popup_metrics(&display, 96, 1920);
        assert_eq!(metrics.layout, CandidatePopupLayout::Horizontal);
        assert_eq!(metrics.height, 46);
        assert_eq!(metrics.width, POPUP_ACTION_MIN_WIDTH_LOGICAL);

        let widths = horizontal_candidate_widths(&display, 96, metrics.width);
        assert_eq!(widths, [metrics.width - POPUP_OUTER_PADDING_LOGICAL * 2]);
        assert_eq!(display.visible(), ["开始反馈"]);
        assert_eq!(display.action_detail(), Some("暂不保存"));
    }

    #[test]
    fn focused_wish_actions_fit_one_bounded_two_item_row() {
        let actions = inline_wish_actions(NativeFeedbackSummary {
            lifecycle: NativeFeedbackLifecycle::Recording,
            ..NativeFeedbackSummary::default()
        });
        let display = CandidateDisplay::actions(&actions);
        let metrics = candidate_popup_metrics(&display, 96, 1920);
        let widths = horizontal_candidate_widths(&display, 96, metrics.width);

        assert_eq!(display.visible().len(), 2);
        assert_eq!(metrics.layout, CandidatePopupLayout::Horizontal);
        assert!(metrics.width <= POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL);
        assert_eq!(widths.len(), 2);
        assert_eq!(
            widths.iter().sum::<i32>(),
            metrics.width - POPUP_OUTER_PADDING_LOGICAL * 2
        );
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
        let expected_width = POPUP_OUTER_PADDING_LOGICAL * 2
            + display
                .visible()
                .iter()
                .enumerate()
                .map(|(index, candidate)| horizontal_candidate_logical_width(candidate, index == 0))
                .sum::<i32>();
        assert_eq!(metrics.width, expected_width);
        assert!(metrics.width < POPUP_HORIZONTAL_MAX_WIDTH_LOGICAL);

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
    fn candidate_label_metadata_shares_one_measured_baseline() {
        let content = RECT {
            left: 10,
            top: 8,
            right: 110,
            bottom: 44,
        };
        let rank_metrics = PopupFontMetrics {
            height: 14,
            ascent: 11,
        };
        let text_metrics = PopupFontMetrics {
            height: 17,
            ascent: 13,
        };
        let (rank, text) = baseline_aligned_label_rects(content, 96, rank_metrics, text_metrics);

        assert_eq!(rank.left, 10);
        assert_eq!(rank.right, 26);
        assert_eq!(rank.top, 19);
        assert_eq!(rank.bottom, 33);
        assert_eq!(text.left, 30);
        assert_eq!(text.top, 17);
        assert_eq!(text.bottom, 34);
        assert_eq!(
            rank.top + rank_metrics.ascent,
            text.top + text_metrics.ascent
        );
    }

    #[test]
    fn personal_memory_marker_stays_inside_the_rank_column_without_reflow() {
        let rank = RECT {
            left: 10,
            top: 19,
            right: 26,
            bottom: 33,
        };
        let mark = candidate_personal_mark_rect(rank, 96).unwrap();

        assert_eq!(mark.left, rank.left);
        assert_eq!(mark.right - mark.left, POPUP_PERSONAL_MARK_SIZE_LOGICAL);
        assert_eq!(mark.bottom - mark.top, POPUP_PERSONAL_MARK_SIZE_LOGICAL);
        assert!(mark.top >= rank.top);
        assert!(mark.bottom <= rank.bottom);
        assert!(mark.right <= rank.right);
    }

    #[test]
    fn candidate_selection_surface_keeps_breathing_room() {
        let item = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: POPUP_ROW_HEIGHT_LOGICAL,
        };
        let selected_text_metrics = PopupFontMetrics {
            height: 17,
            ascent: 13,
        };
        let (selected, accent) = candidate_selection_rects(item, 96, Some(selected_text_metrics));

        assert_eq!(selected.left, 1);
        assert_eq!(selected.top, 4);
        assert_eq!(selected.right, 95);
        assert_eq!(selected.bottom, 32);
        assert_eq!(accent.left, 6);
        assert_eq!(accent.top, 9);
        assert_eq!(accent.right, 9);
        assert_eq!(accent.bottom, 26);
        assert!(
            (accent.top + accent.bottom - selected.top - selected.bottom).abs() <= 1,
            "odd font heights may land half a pixel above the even selection surface"
        );
        let (_, text) = baseline_aligned_label_rects(
            item,
            96,
            PopupFontMetrics {
                height: 14,
                ascent: 11,
            },
            selected_text_metrics,
        );
        assert_eq!((accent.top, accent.bottom), (text.top, text.bottom));
    }

    #[test]
    fn candidate_widths_are_the_sum_of_visible_gutters_and_text() {
        assert_eq!(horizontal_candidate_logical_width("输入法", true), 94);
        assert_eq!(horizontal_candidate_logical_width("输入法", false), 88);
        assert_eq!(
            horizontal_candidate_logical_width("输入法", true)
                - horizontal_candidate_logical_width("输入法", false),
            POPUP_SELECTED_TEXT_INSET_LOGICAL - POPUP_TEXT_PADDING_LOGICAL
        );
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
    fn shape_feedback_overrides_visible_candidate_sources_without_changing_text() {
        let display = CandidateDisplay::from_batch(
            CandidateBatch {
                candidates: vec!["甲".to_owned(), "乙".to_owned()],
                resolved_shape_codes: vec![None; 2],
                provenance: vec![
                    NativeCandidateProvenance::new(NativeCandidateSource::CoreExact, false),
                    NativeCandidateProvenance::with_personalization(
                        NativeCandidateSource::Decoder,
                        NativeCandidatePersonalization::PERSISTENT_EXACT
                            .with(NativeCandidatePersonalization::SESSION_ANCHORED),
                    ),
                ],
                personalized: vec![false, true],
                protected_prefix_len: 0,
                automatic_transposition: None,
                may_have_more: false,
                view: InteractiveCandidateView::Primary,
            },
            0,
        );

        let NativeFeedbackEvent::CandidatesPresentedWithProvenance {
            candidates,
            provenance,
            view,
            ..
        } = display.feedback_event("ab", true)
        else {
            panic!("shape feedback must include provenance");
        };
        assert_eq!(candidates, ["甲", "乙"]);
        assert_eq!(view, NativeCandidateView::Shape);
        assert!(
            provenance
                .iter()
                .all(|item| item.source() == NativeCandidateSource::Shape)
        );
        assert!(!provenance[0].session_promoted());
        assert!(provenance[1].session_promoted());
        assert!(
            provenance[1]
                .personalization()
                .contains(NativeCandidatePersonalization::PERSISTENT_EXACT)
        );
        assert!(
            provenance[1]
                .personalization()
                .contains(NativeCandidatePersonalization::SESSION_ANCHORED)
        );
    }

    #[test]
    fn recovery_mode_and_page_number_have_separate_footer_space() {
        let candidates = (0..10).map(|index| format!("候选{index}")).collect();
        let ordinary = CandidateDisplay::from_candidates(candidates, 0);
        let recovery = CandidateDisplay::from_batch(
            CandidateBatch {
                candidates: ordinary.candidates.clone(),
                resolved_shape_codes: vec![None; ordinary.candidates.len()],
                provenance: ordinary.provenance.clone(),
                personalized: ordinary.personalized.clone(),
                protected_prefix_len: 0,
                automatic_transposition: None,
                may_have_more: true,
                view: InteractiveCandidateView::TranspositionRecovery,
            },
            0,
        );
        let recovery_one_page = CandidateDisplay::from_batch(
            CandidateBatch {
                candidates: vec!["换序候选".to_owned()],
                resolved_shape_codes: vec![None],
                provenance: vec![NativeCandidateProvenance::new(
                    NativeCandidateSource::TranspositionRecovery,
                    false,
                )],
                personalized: vec![false],
                protected_prefix_len: 0,
                automatic_transposition: None,
                may_have_more: false,
                view: InteractiveCandidateView::TranspositionRecovery,
            },
            0,
        );
        let forget_one_page = CandidateDisplay::from_candidates(
            vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()],
            0,
        )
        .with_mode(CandidateDisplayMode::ForgetSelecting);

        assert_eq!(candidate_popup_footer_logical_width(&ordinary), 62);
        assert_eq!(candidate_popup_footer_logical_width(&recovery), 108);
        assert_eq!(candidate_popup_footer_logical_width(&recovery_one_page), 60);
        assert_eq!(
            candidate_popup_mode_label(&forget_one_page),
            Some("忘记 · 数字选择")
        );
        assert!(candidate_popup_footer_logical_width(&forget_one_page) > 60);
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
            horizontal_candidate_logical_width(&long.visible()[0], true)
        );
        assert!(
            widths[1..]
                .iter()
                .all(|width| *width >= POPUP_HORIZONTAL_MIN_ITEM_WIDTH_LOGICAL)
        );
        assert!(widths.iter().any(|width| {
            *width < horizontal_candidate_logical_width(&long.visible()[0], false)
        }));
    }
}
