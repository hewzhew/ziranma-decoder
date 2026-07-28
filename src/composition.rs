//! Host-independent state for one active double-pinyin composition.
//!
//! Terminal rendering, Windows TSF edit sessions, candidate decoding, and
//! persistence stay outside this module. A host feeds semantic input actions,
//! responds to requested candidate operations, and calls `finish_commit`
//! after it has accepted a selected candidate.

use std::collections::VecDeque;
use std::ops::Range;

use crate::SentenceCandidate;

const MAX_COMPOSITION_KEYS: usize = 64;
const MAX_SESSION_SELECTIONS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompositionInput {
    Letters(String),
    Confirm,
    Select(usize),
    Backspace,
    PreviousPage,
    NextPage,
    EnterTab,
    EnterRecovery,
    Escape,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionEffect {
    Continue,
    Confirm,
    Select(usize),
    PreviousPage,
    NextPage,
    RequestTab,
    PassThrough,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompositionSession {
    phonetic: String,
    shape_pinyin: Option<String>,
    stroke_prefix: String,
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
    selections: VecDeque<(String, SentenceCandidate)>,
}

impl SessionSelectionMemory {
    pub fn remember(&mut self, code: &str, candidate: &SentenceCandidate) {
        if code.is_empty() || candidate.text.is_empty() {
            return;
        }
        if let Some(index) = self
            .selections
            .iter()
            .position(|(remembered_code, _)| remembered_code == code)
        {
            self.selections.remove(index);
        }
        self.selections
            .push_front((code.to_owned(), candidate.clone()));
        self.selections.truncate(MAX_SESSION_SELECTIONS);
    }

    pub fn promote(&self, code: &str, candidates: &mut Vec<SentenceCandidate>) -> bool {
        let Some((_, preferred_candidate)) = self
            .selections
            .iter()
            .find(|(remembered_code, _)| remembered_code == code)
        else {
            return false;
        };
        let Some(index) = candidates
            .iter()
            .position(|candidate| candidate.text == preferred_candidate.text)
        else {
            let original_len = candidates.len();
            candidates.insert(0, preferred_candidate.clone());
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
}

impl CompositionSession {
    pub fn phonetic(&self) -> &str {
        &self.phonetic
    }

    pub fn tab_mode(&self) -> bool {
        self.shape_pinyin.is_some()
    }

    pub fn recovery_mode(&self) -> bool {
        self.recovery_mode
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
        self.recovery_mode = false;
        self.shape_pinyin = Some(pinyin.into());
        self.stroke_prefix.clear();
        self.candidate_page_start = 0;
        self.notice = None;
    }

    /// Clears the active composition after the host has committed text.
    ///
    /// The committed document text deliberately remains owned by the host.
    pub fn finish_commit(&mut self) {
        self.phonetic.clear();
        self.candidate_page_start = 0;
        self.recovery_mode = false;
        self.leave_tab();
        self.notice = None;
    }

    pub fn apply(&mut self, input: CompositionInput) -> CompositionEffect {
        self.notice = None;
        match input {
            CompositionInput::Letters(letters) if self.tab_mode() => {
                if letters
                    .as_bytes()
                    .iter()
                    .all(|byte| matches!(byte, b'h' | b's' | b'p' | b'n' | b'z'))
                {
                    self.stroke_prefix.push_str(&letters);
                    self.candidate_page_start = 0;
                } else {
                    self.set_notice("笔画只用 h s p n z");
                }
            }
            CompositionInput::Letters(letters)
                if letters.as_bytes().iter().all(u8::is_ascii_lowercase) =>
            {
                self.recovery_mode = false;
                let available = MAX_COMPOSITION_KEYS.saturating_sub(self.phonetic.len());
                self.phonetic.extend(letters.chars().take(available));
                self.candidate_page_start = 0;
                if letters.len() > available {
                    self.set_notice("本轮最多输入 64 个字母");
                }
            }
            CompositionInput::Letters(_) | CompositionInput::Invalid => {
                self.set_notice("没有这个操作");
            }
            CompositionInput::Confirm if !self.phonetic.is_empty() => {
                return CompositionEffect::Confirm;
            }
            CompositionInput::Confirm => return CompositionEffect::PassThrough,
            CompositionInput::Select(rank) if !self.phonetic.is_empty() => {
                return CompositionEffect::Select(rank);
            }
            CompositionInput::Select(_) => return CompositionEffect::PassThrough,
            CompositionInput::Backspace if self.tab_mode() => {
                if self.stroke_prefix.pop().is_none() {
                    self.leave_tab();
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
    }
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
    fn idle_controls_are_returned_to_the_host() {
        let mut session = CompositionSession::default();
        for input in [
            CompositionInput::Confirm,
            CompositionInput::Select(1),
            CompositionInput::Backspace,
            CompositionInput::PreviousPage,
            CompositionInput::NextPage,
            CompositionInput::EnterTab,
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
}
