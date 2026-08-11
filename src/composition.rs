//! Host-independent state for one active double-pinyin composition.
//!
//! Terminal rendering, Windows TSF edit sessions, candidate decoding, and
//! persistence stay outside this module. A host feeds semantic input actions,
//! responds to requested candidate operations, and calls `finish_commit`
//! after it has accepted a selected candidate.

use std::collections::VecDeque;
use std::ops::Range;

#[cfg(any(windows, test))]
use crate::personal_ranking::is_anchored_suffix_abbreviation;
use crate::{SentenceCandidate, personal_ranking::CandidateTextPromotion};

const MAX_COMPOSITION_KEYS: usize = 64;
const MAX_SESSION_SELECTIONS: usize = 128;
const MAX_SESSION_SELECTION_TEXT_CHARACTERS: usize = 128;
#[cfg(any(windows, test))]
pub(crate) const MAX_TAB_ASSEMBLY_CHARACTERS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionPunctuation {
    Comma,
    Period,
    Semicolon,
    Colon,
    ExclamationMark,
    Ellipsis,
    LeftParenthesis,
    RightParenthesis,
    QuestionMark,
}

impl CompositionPunctuation {
    pub fn text(self) -> &'static str {
        match self {
            Self::Comma => "，",
            Self::Period => "。",
            Self::Semicolon => "；",
            Self::Colon => "：",
            Self::ExclamationMark => "！",
            Self::Ellipsis => "……",
            Self::LeftParenthesis => "（",
            Self::RightParenthesis => "）",
            Self::QuestionMark => "？",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompositionInput {
    Letters(String),
    Confirm,
    CommitRaw,
    Punctuation(CompositionPunctuation),
    Select(usize),
    Backspace,
    PreviousPage,
    NextPage,
    EnterTab,
    EnterWish,
    EnterRecovery,
    Escape,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionEffect {
    Continue,
    Confirm,
    CommitRaw,
    Punctuation(CompositionPunctuation),
    Select(usize),
    PreviousPage,
    NextPage,
    RequestTab,
    ConfirmWish,
    PassThrough,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TabAssembly {
    pinyin_segments: Vec<String>,
    selected: Vec<String>,
    selected_codes: Vec<String>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TabAssemblyStage {
    First,
    Second,
    Later(usize),
}

#[cfg(any(windows, test))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TabAssemblySelection {
    Advanced,
    Complete { text: String, full_code: String },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompositionSession {
    phonetic: String,
    shape_pinyin: Option<String>,
    stroke_prefix: String,
    tab_assembly: Option<TabAssembly>,
    wish_prompt: bool,
    recovery_mode: bool,
    candidate_page_start: usize,
    notice: Option<String>,
}

/// Bounded, memory-only recall of candidates explicitly selected in one host
/// session.
///
/// The remembered key strings and text are intentionally not exposed through
/// `Debug`, serialization, or disk I/O. This is an interaction aid, not the
/// persistent personal model described in `docs/personal-model.md`.
#[derive(Default)]
pub struct SessionSelectionMemory {
    selections: VecDeque<SessionSelection>,
}

struct SessionSelection {
    code: String,
    text: String,
    candidate: Option<SentenceCandidate>,
}

impl SessionSelectionMemory {
    pub fn remember(&mut self, code: &str, candidate: &SentenceCandidate) {
        self.remember_entry(code, &candidate.text, Some(candidate.clone()));
    }

    pub fn remember_text(&mut self, code: &str, text: &str) {
        self.remember_entry(code, text, None);
    }

    /// Returns the text currently remembered for one exact input code.
    ///
    /// The borrowed text cannot outlive this memory and is not copied into a
    /// diagnostic or serialization surface.
    pub fn remembered_text(&self, code: &str) -> Option<&str> {
        self.selections
            .iter()
            .find(|selection| selection.code == code)
            .map(|selection| selection.text.as_str())
    }

    /// Removes the current entry for `code` only when it still names `text`.
    ///
    /// This conditional form lets a retractable selection restore its prior
    /// session value without overwriting a newer explicit choice.
    pub fn forget_text(&mut self, code: &str, text: &str) -> bool {
        let Some(index) = self
            .selections
            .iter()
            .position(|selection| selection.code == code && selection.text == text)
        else {
            return false;
        };
        self.selections.remove(index);
        true
    }

    fn remember_entry(&mut self, code: &str, text: &str, candidate: Option<SentenceCandidate>) {
        if code.is_empty()
            || code.len() > MAX_COMPOSITION_KEYS
            || !code.as_bytes().iter().all(u8::is_ascii_lowercase)
            || text.is_empty()
            || text.chars().count() > MAX_SESSION_SELECTION_TEXT_CHARACTERS
        {
            return;
        }
        if let Some(index) = self
            .selections
            .iter()
            .position(|selection| selection.code == code)
        {
            self.selections.remove(index);
        }
        self.selections.push_front(SessionSelection {
            code: code.to_owned(),
            text: text.to_owned(),
            candidate,
        });
        self.selections.truncate(MAX_SESSION_SELECTIONS);
    }

    pub fn promote(&self, code: &str, candidates: &mut Vec<SentenceCandidate>) -> bool {
        let Some(preferred) = self
            .selections
            .iter()
            .find(|selection| selection.code == code)
        else {
            return false;
        };
        let Some(index) = candidates
            .iter()
            .position(|candidate| candidate.text == preferred.text)
        else {
            let Some(candidate) = preferred.candidate.as_ref() else {
                return false;
            };
            let original_len = candidates.len();
            candidates.insert(0, candidate.clone());
            candidates.truncate(original_len.max(1));
            return true;
        };
        if index == 0 {
            return true;
        }
        let candidate = candidates.remove(index);
        candidates.insert(0, candidate);
        true
    }

    pub fn promote_texts(&self, code: &str, candidates: &mut Vec<String>) -> bool {
        self.promote_texts_after(code, candidates, 0)
    }

    /// Promotes the remembered text without crossing a caller-owned prefix.
    ///
    /// Interactive hosts use the prefix for explicit aliases that must remain
    /// above transient session evidence. Callers without such a lane should
    /// continue using [`Self::promote_texts`].
    pub fn promote_texts_after(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> bool {
        self.promote_texts_after_decision(code, candidates, protected_prefix)
            .is_some()
    }

    pub(crate) fn promote_texts_after_decision(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
    ) -> Option<CandidateTextPromotion> {
        let preferred = self
            .selections
            .iter()
            .find(|selection| selection.code == code)?;
        let protected_prefix = protected_prefix.min(candidates.len());
        let Some(index) = candidates
            .iter()
            .position(|candidate| candidate == &preferred.text)
        else {
            if protected_prefix > 0 && protected_prefix == candidates.len() {
                return None;
            }
            let original_len = candidates.len();
            candidates.insert(protected_prefix, preferred.text.clone());
            candidates.truncate(original_len.max(1));
            return (protected_prefix < candidates.len()).then_some(CandidateTextPromotion {
                index: protected_prefix,
                source_index: None,
                changed: true,
            });
        };
        if index <= protected_prefix {
            return Some(CandidateTextPromotion {
                index,
                source_index: Some(index),
                changed: false,
            });
        }
        let candidate = candidates.remove(index);
        candidates.insert(protected_prefix, candidate);
        Some(CandidateTextPromotion {
            index: protected_prefix,
            source_index: Some(index),
            changed: true,
        })
    }

    /// Promotes the most recent session choice learned under a compatible,
    /// caller-verified complete code into an anchored-tail abbreviation.
    ///
    /// Unlike exact session recall, this never injects an absent candidate.
    /// The host owns both public-dictionary verification and suppression
    /// policy through `eligible_source`.
    #[cfg(test)]
    pub(crate) fn promote_anchored_suffix_texts_after(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        mut eligible_source: impl FnMut(&str, &str) -> bool,
    ) -> bool {
        self.promote_anchored_suffix_texts_after_decision(
            code,
            candidates,
            protected_prefix,
            &mut eligible_source,
        )
        .is_some()
    }

    #[cfg(any(windows, test))]
    pub(crate) fn promote_anchored_suffix_texts_after_decision(
        &self,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        mut eligible_source: impl FnMut(&str, &str) -> bool,
    ) -> Option<CandidateTextPromotion> {
        let preferred = self.selections.iter().find(|selection| {
            is_anchored_suffix_abbreviation(&selection.code, code)
                && candidates
                    .iter()
                    .any(|candidate| candidate == &selection.text)
                && eligible_source(&selection.code, &selection.text)
        })?;
        let protected_prefix = protected_prefix.min(candidates.len());
        let index = candidates
            .iter()
            .position(|candidate| candidate == &preferred.text)?;
        if index <= protected_prefix {
            return Some(CandidateTextPromotion {
                index,
                source_index: Some(index),
                changed: false,
            });
        }
        let candidate = candidates.remove(index);
        candidates.insert(protected_prefix, candidate);
        Some(CandidateTextPromotion {
            index: protected_prefix,
            source_index: Some(index),
            changed: true,
        })
    }

    #[cfg(any(windows, test))]
    pub(crate) fn has_anchored_suffix_evidence(
        &self,
        code: &str,
        text: &str,
        mut eligible_source: impl FnMut(&str, &str) -> bool,
    ) -> bool {
        self.selections.iter().any(|selection| {
            selection.text == text
                && is_anchored_suffix_abbreviation(&selection.code, code)
                && eligible_source(&selection.code, &selection.text)
        })
    }

    pub fn clear(&mut self) {
        self.selections.clear();
    }
}

impl CompositionSession {
    pub fn phonetic(&self) -> &str {
        &self.phonetic
    }

    pub fn tab_mode(&self) -> bool {
        self.shape_pinyin.is_some()
    }

    pub(crate) fn tab_assembly_mode(&self) -> bool {
        self.tab_assembly.is_some()
    }

    #[cfg(any(windows, test))]
    pub(crate) fn tab_assembly_stage(&self) -> Option<TabAssemblyStage> {
        self.tab_assembly
            .as_ref()
            .map(|assembly| match assembly.selected.len() {
                0 => TabAssemblyStage::First,
                1 => TabAssemblyStage::Second,
                selected => TabAssemblyStage::Later(selected.saturating_add(1)),
            })
    }

    #[cfg(any(windows, test))]
    pub(crate) fn tab_assembly_selected_text(&self) -> Option<String> {
        let assembly = self.tab_assembly.as_ref()?;
        (!assembly.selected.is_empty()).then(|| assembly.selected.concat())
    }

    #[cfg(windows)]
    pub(crate) fn tab_assembly_position(&self) -> Option<usize> {
        self.tab_assembly
            .as_ref()
            .map(|assembly| assembly.selected.len().saturating_add(1))
    }

    #[cfg(windows)]
    pub(crate) fn tab_assembly_character_count(&self) -> Option<usize> {
        self.tab_assembly
            .as_ref()
            .map(|assembly| assembly.pinyin_segments.len())
    }

    #[cfg(test)]
    pub(crate) fn tab_assembly_has_trailing_initial(&self) -> bool {
        self.tab_assembly
            .as_ref()
            .and_then(|assembly| assembly.pinyin_segments.last())
            .is_some_and(|segment| segment.len() == 1)
    }

    pub fn recovery_mode(&self) -> bool {
        self.recovery_mode
    }

    pub fn wish_prompt(&self) -> bool {
        self.wish_prompt
    }

    pub fn shape_pinyin(&self) -> Option<&str> {
        self.shape_pinyin.as_deref()
    }

    pub fn stroke_prefix(&self) -> &str {
        &self.stroke_prefix
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn candidate_page_start(&self) -> usize {
        self.candidate_page_start
    }

    pub fn visible_candidate_range(
        &self,
        candidate_count: usize,
        page_size: usize,
    ) -> Range<usize> {
        let start = self.candidate_page_start.min(candidate_count);
        start..start.saturating_add(page_size).min(candidate_count)
    }

    pub fn previous_candidate_page(&mut self, page_size: usize) {
        self.candidate_page_start = self.candidate_page_start.saturating_sub(page_size);
    }

    pub fn next_candidate_page(
        &mut self,
        candidate_count: usize,
        page_size: usize,
        candidate_limit: usize,
    ) {
        let next = self.candidate_page_start.saturating_add(page_size);
        if next < candidate_count || next < candidate_limit {
            self.candidate_page_start = next;
        } else {
            self.set_notice("已经是最后一页");
        }
    }

    pub fn normalize_candidate_page(&mut self, candidate_count: usize, page_size: usize) {
        if self.candidate_page_start == 0 || self.candidate_page_start < candidate_count {
            return;
        }
        self.candidate_page_start = candidate_count
            .checked_sub(1)
            .map_or(0, |last| last / page_size * page_size);
        self.set_notice("已经是最后一页");
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn enter_tab(&mut self, pinyin: impl Into<String>) {
        self.wish_prompt = false;
        self.recovery_mode = false;
        self.tab_assembly = None;
        self.shape_pinyin = Some(pinyin.into());
        self.stroke_prefix.clear();
        self.candidate_page_start = 0;
        self.notice = None;
    }

    #[cfg(any(windows, test))]
    pub(crate) fn enter_tab_path(&mut self, pinyin_segments: &[&str]) -> bool {
        if !(2..=MAX_TAB_ASSEMBLY_CHARACTERS).contains(&pinyin_segments.len())
            || !valid_tab_assembly_segments(pinyin_segments)
        {
            return false;
        }
        self.wish_prompt = false;
        self.recovery_mode = false;
        self.shape_pinyin = Some(pinyin_segments[0].to_owned());
        self.stroke_prefix.clear();
        self.tab_assembly = Some(TabAssembly {
            pinyin_segments: pinyin_segments
                .iter()
                .map(|pinyin| (*pinyin).to_owned())
                .collect(),
            selected: Vec::with_capacity(pinyin_segments.len()),
            selected_codes: Vec::with_capacity(pinyin_segments.len()),
        });
        self.candidate_page_start = 0;
        self.notice = None;
        true
    }

    #[cfg(any(windows, test))]
    pub(crate) fn accept_tab_assembly_candidate(
        &mut self,
        text: &str,
        resolved_code: &str,
    ) -> Option<TabAssemblySelection> {
        let mut characters = text.chars();
        let character = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        let text = character.to_string();
        let assembly = self.tab_assembly.as_mut()?;
        let active_slot = assembly.pinyin_segments.get(assembly.selected.len())?;
        if !resolved_tab_slot_matches(active_slot, resolved_code) {
            return None;
        }
        if assembly.selected.len().saturating_add(1) < assembly.pinyin_segments.len() {
            assembly.selected.push(text);
            assembly.selected_codes.push(resolved_code.to_owned());
            self.shape_pinyin = assembly
                .pinyin_segments
                .get(assembly.selected.len())
                .cloned();
            self.stroke_prefix.clear();
            self.candidate_page_start = 0;
            self.notice = None;
            Some(TabAssemblySelection::Advanced)
        } else {
            let mut combined = assembly.selected.concat();
            combined.push(character);
            let mut full_code = assembly.selected_codes.concat();
            full_code.push_str(resolved_code);
            self.finish_commit();
            Some(TabAssemblySelection::Complete {
                text: combined,
                full_code,
            })
        }
    }

    /// Clears the active composition after the host has committed text.
    ///
    /// The committed document text deliberately remains owned by the host.
    pub fn finish_commit(&mut self) {
        self.phonetic.clear();
        self.candidate_page_start = 0;
        self.recovery_mode = false;
        self.wish_prompt = false;
        self.leave_tab();
        self.notice = None;
    }

    pub fn apply(&mut self, input: CompositionInput) -> CompositionEffect {
        self.notice = None;
        match input {
            CompositionInput::Confirm if self.wish_prompt => {
                return CompositionEffect::ConfirmWish;
            }
            CompositionInput::Select(1) if self.wish_prompt => {
                return CompositionEffect::ConfirmWish;
            }
            CompositionInput::Select(_) if self.wish_prompt => {
                self.set_notice("空格或 1 确认");
            }
            CompositionInput::Backspace | CompositionInput::Escape | CompositionInput::EnterTab
                if self.wish_prompt =>
            {
                self.wish_prompt = false;
                self.candidate_page_start = 0;
            }
            CompositionInput::Letters(letters) if self.wish_prompt => {
                self.wish_prompt = false;
                self.append_phonetic_letters(&letters);
            }
            CompositionInput::CommitRaw
            | CompositionInput::Punctuation(_)
            | CompositionInput::PreviousPage
            | CompositionInput::NextPage
            | CompositionInput::EnterRecovery
            | CompositionInput::EnterWish
                if self.wish_prompt =>
            {
                self.set_notice("空格确认，退格返回");
            }
            CompositionInput::Letters(letters) if self.tab_mode() => {
                if let Some(shape) = canonical_shape_letters(&letters) {
                    self.stroke_prefix.push_str(&shape);
                    self.candidate_page_start = 0;
                } else {
                    self.set_notice("形码用小写字母；笔画为 h u p n v");
                }
            }
            CompositionInput::Letters(letters)
                if letters.as_bytes().iter().all(u8::is_ascii_lowercase) =>
            {
                self.recovery_mode = false;
                self.append_phonetic_letters(&letters);
            }
            CompositionInput::Letters(_) | CompositionInput::Invalid => {
                self.set_notice("没有这个操作");
            }
            CompositionInput::Confirm | CompositionInput::Select(_) if self.tab_assembly_mode() => {
                self.set_notice("请选择一个字");
            }
            CompositionInput::Confirm if !self.phonetic.is_empty() => {
                return CompositionEffect::Confirm;
            }
            CompositionInput::Confirm => return CompositionEffect::PassThrough,
            CompositionInput::CommitRaw if !self.phonetic.is_empty() => {
                return CompositionEffect::CommitRaw;
            }
            CompositionInput::CommitRaw => return CompositionEffect::PassThrough,
            CompositionInput::Punctuation(_) if self.tab_assembly_mode() => {
                self.set_notice("选完整词后再输入标点");
            }
            CompositionInput::Punctuation(punctuation) => {
                return CompositionEffect::Punctuation(punctuation);
            }
            CompositionInput::Select(rank) if !self.phonetic.is_empty() => {
                return CompositionEffect::Select(rank);
            }
            CompositionInput::Select(_) => return CompositionEffect::PassThrough,
            CompositionInput::Backspace if self.tab_mode() => {
                if self.stroke_prefix.pop().is_none() {
                    let previous_pinyin = self.tab_assembly.as_mut().and_then(|assembly| {
                        assembly.selected.pop()?;
                        assembly.selected_codes.pop()?;
                        assembly
                            .pinyin_segments
                            .get(assembly.selected.len())
                            .cloned()
                    });
                    if previous_pinyin.is_some() {
                        self.shape_pinyin = previous_pinyin;
                    } else {
                        self.leave_tab();
                    }
                }
                self.candidate_page_start = 0;
            }
            CompositionInput::Backspace if !self.phonetic.is_empty() => {
                self.recovery_mode = false;
                self.phonetic.pop();
                self.candidate_page_start = 0;
            }
            CompositionInput::Backspace => return CompositionEffect::PassThrough,
            CompositionInput::PreviousPage if !self.phonetic.is_empty() => {
                return CompositionEffect::PreviousPage;
            }
            CompositionInput::NextPage if !self.phonetic.is_empty() => {
                return CompositionEffect::NextPage;
            }
            CompositionInput::PreviousPage | CompositionInput::NextPage => {
                return CompositionEffect::PassThrough;
            }
            CompositionInput::EnterTab if !self.phonetic.is_empty() && !self.tab_mode() => {
                return CompositionEffect::RequestTab;
            }
            CompositionInput::EnterTab => return CompositionEffect::PassThrough,
            CompositionInput::EnterWish if !self.phonetic.is_empty() && !self.tab_mode() => {
                self.recovery_mode = false;
                self.wish_prompt = true;
                self.candidate_page_start = 0;
            }
            CompositionInput::EnterWish => return CompositionEffect::PassThrough,
            CompositionInput::EnterRecovery if !self.phonetic.is_empty() && !self.tab_mode() => {
                self.recovery_mode = true;
                self.candidate_page_start = 0;
            }
            CompositionInput::EnterRecovery => return CompositionEffect::PassThrough,
            CompositionInput::Escape if self.tab_mode() => self.leave_tab(),
            CompositionInput::Escape if self.recovery_mode() => self.recovery_mode = false,
            CompositionInput::Escape if !self.phonetic.is_empty() => {
                self.phonetic.clear();
                self.candidate_page_start = 0;
            }
            CompositionInput::Escape => return CompositionEffect::PassThrough,
        }
        CompositionEffect::Continue
    }

    fn leave_tab(&mut self) {
        self.shape_pinyin = None;
        self.stroke_prefix.clear();
        self.tab_assembly = None;
    }

    fn append_phonetic_letters(&mut self, letters: &str) {
        if !letters.as_bytes().iter().all(u8::is_ascii_lowercase) {
            self.set_notice("没有这个操作");
            return;
        }
        let available = MAX_COMPOSITION_KEYS.saturating_sub(self.phonetic.len());
        self.phonetic.extend(letters.chars().take(available));
        self.candidate_page_start = 0;
        if letters.len() > available {
            self.set_notice("本轮最多输入 64 个字母");
        }
    }
}

#[cfg(any(windows, test))]
fn valid_tab_assembly_segments(segments: &[&str]) -> bool {
    let Some((last, complete)) = segments.split_last() else {
        return false;
    };
    complete
        .iter()
        .all(|segment| valid_tab_slot(segment, false))
        && valid_tab_slot(last, true)
}

#[cfg(any(windows, test))]
fn valid_tab_slot(slot: &str, trailing: bool) -> bool {
    (slot.len() == 2 || (trailing && slot.len() == 1))
        && slot.as_bytes().iter().all(u8::is_ascii_lowercase)
}

#[cfg(any(windows, test))]
fn resolved_tab_slot_matches(slot: &str, resolved_code: &str) -> bool {
    resolved_code.len() == 2
        && resolved_code.as_bytes().iter().all(u8::is_ascii_lowercase)
        && if slot.len() == 1 {
            resolved_code.starts_with(slot)
        } else {
            resolved_code == slot
        }
}

fn canonical_shape_letters(letters: &str) -> Option<String> {
    letters
        .bytes()
        .map(|letter| match letter {
            b'h' => Some('h'),
            b'u' | b's' => Some('s'),
            b'p' => Some('p'),
            b'n' => Some('n'),
            b'v' | b'z' => Some('z'),
            b'a'..=b'z' => Some(char::from(letter)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_owns_committed_text_and_can_reuse_the_session() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("mafmkm".to_owned()));
        assert_eq!(session.phonetic(), "mafmkm");

        session.finish_commit();
        assert!(session.phonetic().is_empty());
        assert!(!session.tab_mode());
        assert!(!session.recovery_mode());

        session.apply(CompositionInput::Letters("wuwa".to_owned()));
        assert_eq!(session.phonetic(), "wuwa");
    }

    #[test]
    fn explicit_recovery_and_shape_modes_remain_isolated() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("mafkmm".to_owned()));
        session.apply(CompositionInput::EnterRecovery);
        assert!(session.recovery_mode());

        assert_eq!(
            session.apply(CompositionInput::EnterTab),
            CompositionEffect::RequestTab
        );
        session.enter_tab("ma");
        assert!(session.tab_mode());
        assert!(!session.recovery_mode());
    }

    #[test]
    fn four_key_tab_assembly_advances_without_committing_a_partial_character() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthp".to_owned()));

        assert!(session.enter_tab_path(&["qt", "hp"]));
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::First));
        assert_eq!(session.shape_pinyin(), Some("qt"));
        assert_eq!(
            session.accept_tab_assembly_candidate("雀", "qt"),
            Some(TabAssemblySelection::Advanced)
        );
        assert_eq!(session.phonetic(), "qthp");
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::Second));
        assert_eq!(session.shape_pinyin(), Some("hp"));

        assert_eq!(
            session.accept_tab_assembly_candidate("魂", "hp"),
            Some(TabAssemblySelection::Complete {
                text: "雀魂".to_owned(),
                full_code: "qthp".to_owned(),
            })
        );
        assert!(session.phonetic().is_empty());
        assert!(!session.tab_mode());
        assert!(!session.tab_assembly_mode());
    }

    #[test]
    fn four_key_tab_assembly_backspace_rewinds_one_layer_and_escape_leaves_tab() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthp".to_owned()));
        assert!(session.enter_tab_path(&["qt", "hp"]));
        assert_eq!(
            session.accept_tab_assembly_candidate("雀", "qt"),
            Some(TabAssemblySelection::Advanced)
        );

        session.apply(CompositionInput::Letters("u".to_owned()));
        assert_eq!(
            session.stroke_prefix(),
            "s",
            "the natural-code sh key must map to the canonical vertical stroke"
        );
        session.apply(CompositionInput::Backspace);
        assert_eq!(session.stroke_prefix(), "");
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::Second));

        session.apply(CompositionInput::Backspace);
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::First));
        assert_eq!(session.shape_pinyin(), Some("qt"));
        assert_eq!(session.phonetic(), "qthp");

        session.apply(CompositionInput::Escape);
        assert!(!session.tab_mode());
        assert_eq!(session.phonetic(), "qthp");
    }

    #[test]
    fn four_key_tab_assembly_rejects_invalid_segments_and_multi_character_steps() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthp".to_owned()));
        assert!(!session.enter_tab_path(&["q", "hp"]));
        assert!(!session.tab_mode());

        assert!(session.enter_tab_path(&["qt", "hp"]));
        assert_eq!(session.accept_tab_assembly_candidate("雀魂", "qt"), None);
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::First));

        assert_eq!(
            session.apply(CompositionInput::Punctuation(CompositionPunctuation::Comma)),
            CompositionEffect::Continue
        );
        assert_eq!(session.notice(), Some("选完整词后再输入标点"));
        assert_eq!(session.phonetic(), "qthp");
    }

    #[test]
    fn tab_assembly_accepts_only_one_trailing_initial_slot() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("jdj".to_owned()));

        assert!(session.enter_tab_path(&["jd", "j"]));
        assert!(session.tab_assembly_has_trailing_initial());
        assert_eq!(session.shape_pinyin(), Some("jd"));
        assert_eq!(
            session.accept_tab_assembly_candidate("甲", "jd"),
            Some(TabAssemblySelection::Advanced)
        );
        assert_eq!(session.shape_pinyin(), Some("j"));

        session.apply(CompositionInput::Letters("x".to_owned()));
        assert_eq!(session.stroke_prefix(), "x");
        session.apply(CompositionInput::Backspace);
        assert_eq!(session.stroke_prefix(), "");
        session.apply(CompositionInput::Backspace);
        assert_eq!(session.shape_pinyin(), Some("jd"));

        assert!(!session.enter_tab_path(&["j", "jd"]));
        assert!(!session.enter_tab_path(&["jd", "j", "d"]));
    }

    #[test]
    fn tab_assembly_requires_each_selected_character_to_match_its_resolved_code() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("jdj".to_owned()));
        assert!(session.enter_tab_path(&["jd", "j"]));

        assert_eq!(
            session.accept_tab_assembly_candidate("甲", "jm"),
            None,
            "a complete two-key slot must reject a different exact identity"
        );
        assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::First));
        assert_eq!(
            session.accept_tab_assembly_candidate("甲", "jd"),
            Some(TabAssemblySelection::Advanced)
        );

        for invalid in ["", "j", "am", "Jm", "jmq"] {
            assert_eq!(
                session.accept_tab_assembly_candidate("件", invalid),
                None,
                "a trailing-initial slot must retain one valid full two-key identity"
            );
            assert_eq!(session.tab_assembly_stage(), Some(TabAssemblyStage::Second));
        }
        assert_eq!(
            session.accept_tab_assembly_candidate("件", "jm"),
            Some(TabAssemblySelection::Complete {
                text: "甲件".to_owned(),
                full_code: "jdjm".to_owned(),
            })
        );
    }

    #[test]
    fn bounded_tab_path_advances_and_rewinds_one_character_at_a_time() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("qthplmxi".to_owned()));
        assert!(session.enter_tab_path(&["qt", "hp", "lm", "xi"]));
        assert!(!session.enter_tab_path(&["qt"]));
        assert!(!session.enter_tab_path(&["qt", "hp", "lm", "xi", "ab"]));

        for (text, code, stage, pinyin, selected) in [
            ("雀", "qt", TabAssemblyStage::Second, "hp", "雀"),
            ("魂", "hp", TabAssemblyStage::Later(3), "lm", "雀魂"),
            ("练", "lm", TabAssemblyStage::Later(4), "xi", "雀魂练"),
        ] {
            assert_eq!(
                session.accept_tab_assembly_candidate(text, code),
                Some(TabAssemblySelection::Advanced)
            );
            assert_eq!(session.tab_assembly_stage(), Some(stage));
            assert_eq!(session.shape_pinyin(), Some(pinyin));
            assert_eq!(
                session.tab_assembly_selected_text().as_deref(),
                Some(selected)
            );
        }

        for (stage, pinyin, selected) in [
            (TabAssemblyStage::Later(3), "lm", Some("雀魂")),
            (TabAssemblyStage::Second, "hp", Some("雀")),
            (TabAssemblyStage::First, "qt", None),
        ] {
            session.apply(CompositionInput::Backspace);
            assert_eq!(session.tab_assembly_stage(), Some(stage));
            assert_eq!(session.shape_pinyin(), Some(pinyin));
            assert_eq!(session.tab_assembly_selected_text().as_deref(), selected);
        }

        for (text, code) in [("雀", "qt"), ("魂", "hp"), ("练", "lm")] {
            assert_eq!(
                session.accept_tab_assembly_candidate(text, code),
                Some(TabAssemblySelection::Advanced)
            );
        }
        assert_eq!(
            session.accept_tab_assembly_candidate("习", "xi"),
            Some(TabAssemblySelection::Complete {
                text: "雀魂练习".to_owned(),
                full_code: "qthplmxi".to_owned(),
            })
        );
        assert!(session.phonetic().is_empty());
        assert!(!session.tab_mode());

        let mut three_character = CompositionSession::default();
        three_character.apply(CompositionInput::Letters("qthplm".to_owned()));
        assert!(three_character.enter_tab_path(&["qt", "hp", "lm"]));
        for (text, code) in [("雀", "qt"), ("魂", "hp")] {
            assert_eq!(
                three_character.accept_tab_assembly_candidate(text, code),
                Some(TabAssemblySelection::Advanced)
            );
        }
        assert_eq!(
            three_character.accept_tab_assembly_candidate("练", "lm"),
            Some(TabAssemblySelection::Complete {
                text: "雀魂练".to_owned(),
                full_code: "qthplm".to_owned(),
            })
        );
    }

    #[test]
    fn explicit_wish_prompt_requires_confirmation_and_preserves_phonetic_text() {
        let mut session = CompositionSession::default();
        session.apply(CompositionInput::Letters("xuy".to_owned()));
        assert_eq!(
            session.apply(CompositionInput::EnterWish),
            CompositionEffect::Continue
        );
        assert!(session.wish_prompt());
        assert_eq!(session.phonetic(), "xuy");
        assert_eq!(
            session.apply(CompositionInput::Confirm),
            CompositionEffect::ConfirmWish
        );
        assert_eq!(session.phonetic(), "xuy");

        assert_eq!(
            session.apply(CompositionInput::Backspace),
            CompositionEffect::Continue
        );
        assert!(!session.wish_prompt());
        assert_eq!(session.phonetic(), "xuy");
        assert_eq!(
            session.apply(CompositionInput::EnterTab),
            CompositionEffect::RequestTab,
            "the ordinary Tab path must remain available after leaving the prompt"
        );

        session.apply(CompositionInput::EnterWish);
        session.apply(CompositionInput::Letters("h".to_owned()));
        assert!(!session.wish_prompt());
        assert_eq!(
            session.phonetic(),
            "xuyh",
            "continuing to type must never lose a letter to the action prompt"
        );
    }

    #[test]
    fn raw_commit_and_chinese_punctuation_have_distinct_semantics() {
        let mut session = CompositionSession::default();
        assert_eq!(
            session.apply(CompositionInput::CommitRaw),
            CompositionEffect::PassThrough
        );
        assert_eq!(
            session.apply(CompositionInput::Punctuation(CompositionPunctuation::Comma)),
            CompositionEffect::Punctuation(CompositionPunctuation::Comma)
        );

        session.apply(CompositionInput::Letters("ju".to_owned()));
        assert_eq!(
            session.apply(CompositionInput::CommitRaw),
            CompositionEffect::CommitRaw
        );
        assert_eq!(session.phonetic(), "ju");
        assert_eq!(
            session.apply(CompositionInput::Punctuation(
                CompositionPunctuation::Period
            )),
            CompositionEffect::Punctuation(CompositionPunctuation::Period)
        );

        for (punctuation, expected) in [
            (CompositionPunctuation::Comma, "，"),
            (CompositionPunctuation::Period, "。"),
            (CompositionPunctuation::Semicolon, "；"),
            (CompositionPunctuation::Colon, "："),
            (CompositionPunctuation::ExclamationMark, "！"),
            (CompositionPunctuation::Ellipsis, "……"),
            (CompositionPunctuation::LeftParenthesis, "（"),
            (CompositionPunctuation::RightParenthesis, "）"),
            (CompositionPunctuation::QuestionMark, "？"),
        ] {
            assert_eq!(punctuation.text(), expected);
        }
    }

    #[test]
    fn idle_controls_are_returned_to_the_host() {
        let mut session = CompositionSession::default();
        for input in [
            CompositionInput::Confirm,
            CompositionInput::CommitRaw,
            CompositionInput::Select(1),
            CompositionInput::Backspace,
            CompositionInput::PreviousPage,
            CompositionInput::NextPage,
            CompositionInput::EnterTab,
            CompositionInput::EnterWish,
            CompositionInput::EnterRecovery,
            CompositionInput::Escape,
        ] {
            assert_eq!(
                session.apply(input),
                CompositionEffect::PassThrough,
                "an idle composition must not swallow ordinary host controls"
            );
        }
    }

    #[test]
    fn session_text_selection_memory_is_bounded_to_valid_explicit_values() {
        let mut memory = SessionSelectionMemory::default();
        memory.remember_text("ab", "乙");

        let mut visible = vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()];
        assert!(memory.promote_texts("ab", &mut visible));
        assert_eq!(visible, ["乙", "甲", "丙"]);

        let mut shallow = vec!["甲".to_owned()];
        assert!(memory.promote_texts("ab", &mut shallow));
        assert_eq!(shallow, ["乙"]);

        memory.remember_text("AB", "无效");
        assert!(!memory.promote_texts("AB", &mut visible));
        memory.clear();
        assert!(!memory.promote_texts("ab", &mut visible));
    }

    #[test]
    fn session_text_selection_memory_never_crosses_a_protected_prefix() {
        let mut memory = SessionSelectionMemory::default();
        memory.remember_text("ab", "乙");

        let mut visible = vec![
            "固定".to_owned(),
            "甲".to_owned(),
            "乙".to_owned(),
            "丙".to_owned(),
        ];
        assert!(memory.promote_texts_after("ab", &mut visible, 1));
        assert_eq!(visible, ["固定", "乙", "甲", "丙"]);

        let mut shallow = vec!["固定".to_owned()];
        assert!(!memory.promote_texts_after("ab", &mut shallow, 1));
        assert_eq!(shallow, ["固定"]);
    }

    #[test]
    fn session_text_promotion_reports_change_without_a_second_candidate_copy() {
        let mut memory = SessionSelectionMemory::default();
        memory.remember_text("ab", "乙");

        let mut moved = vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()];
        assert_eq!(
            memory.promote_texts_after_decision("ab", &mut moved, 0),
            Some(CandidateTextPromotion {
                index: 0,
                source_index: Some(1),
                changed: true,
            })
        );
        assert_eq!(moved, ["乙", "甲", "丙"]);

        assert_eq!(
            memory.promote_texts_after_decision("ab", &mut moved, 0),
            Some(CandidateTextPromotion {
                index: 0,
                source_index: Some(0),
                changed: false,
            })
        );
    }

    #[test]
    fn session_selection_can_inherit_into_only_a_verified_visible_anchored_tail() {
        let mut memory = SessionSelectionMemory::default();
        memory.remember_text("jdjd", "讲讲");
        memory.remember_text("abef", "旁路");
        let mut candidates = vec!["固定".to_owned(), "简单".to_owned(), "讲讲".to_owned()];

        assert!(memory.promote_anchored_suffix_texts_after(
            "jdj",
            &mut candidates,
            1,
            |code, text| code == "jdjd" && text == "讲讲",
        ));
        assert_eq!(candidates, ["固定", "讲讲", "简单"]);
        assert!(memory.has_anchored_suffix_evidence("jdj", "讲讲", |_, _| true));
        assert!(!memory.has_anchored_suffix_evidence("jd", "讲讲", |_, _| true));

        let mut absent = vec!["简单".to_owned(), "降价".to_owned()];
        assert!(!memory.promote_anchored_suffix_texts_after("jdj", &mut absent, 0, |_, _| true,));
        assert_eq!(absent, ["简单", "降价"]);
    }
}
