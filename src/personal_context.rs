//! Bounded, process-local left-context evidence for personal candidate order.
//!
//! The table learns only from caller-confirmed selections. It never performs
//! I/O and can only move a candidate that is already present in the caller's
//! frozen pool. Candidate generation and persistent personal ranking remain
//! separate responsibilities.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::personal_ranking::CandidateTextPromotion;

pub const MAX_PERSONAL_CONTEXT_ENTRIES: usize = 2_048;
pub const MAX_PERSONAL_CONTEXT_CODE_KEYS: usize = 64;
pub const MAX_PERSONAL_CONTEXT_TEXT_CHARACTERS: usize = 64;
pub const PERSONAL_CONTEXT_SUPPORT_CAP: u64 = 4;
pub const PERSONAL_CONTEXT_REJECTION_CAP: u64 = 4;
pub const PERSONAL_CONTEXT_SEARCH_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PersonalContextEvidence {
    selections: u64,
    rejections: u64,
    last_selection_generation: u64,
    last_observation_generation: u64,
}

/// Fixed-capacity, in-memory evidence keyed by
/// `(previous committed text, current code, selected text)`.
///
/// The custom `Debug` output deliberately exposes only aggregate size and
/// generation, never the private identities held by the table.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct PersonalContextRanking {
    entries: BTreeMap<(String, String, String), PersonalContextEvidence>,
    generation: u64,
}

impl fmt::Debug for PersonalContextRanking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonalContextRanking")
            .field("entries", &self.entries.len())
            .field("generation", &self.generation)
            .field("debug_contains_text", &false)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersonalContextError {
    InvalidIdentity,
    GenerationOverflow,
}

impl fmt::Display for PersonalContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidIdentity => "personal context identity is invalid",
            Self::GenerationOverflow => "personal context generation overflow",
        })
    }
}

impl Error for PersonalContextError {}

impl PersonalContextRanking {
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn accepts_left_context(text: &str) -> bool {
        valid_text(text)
    }

    pub fn record(
        &mut self,
        previous_text: &str,
        code: &str,
        selected_text: &str,
    ) -> Result<(), PersonalContextError> {
        self.record_choice(previous_text, code, selected_text, None)
    }

    /// Records one confirmed choice and, when present, the first unprotected
    /// candidate that the choice explicitly overruled.
    ///
    /// Rejection evidence is deliberately gentle: each rejection cancels at
    /// most one bounded selection for the same identity. The two observations
    /// share one generation so an overruled candidate does not become more
    /// recent merely because it was rejected.
    pub fn record_choice(
        &mut self,
        previous_text: &str,
        code: &str,
        selected_text: &str,
        overruled_text: Option<&str>,
    ) -> Result<(), PersonalContextError> {
        validate_identity(previous_text, code, selected_text)?;
        if let Some(overruled) = overruled_text {
            validate_identity(previous_text, code, overruled)?;
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(PersonalContextError::GenerationOverflow)?;
        {
            let entry = self
                .entries
                .entry((
                    previous_text.to_owned(),
                    code.to_owned(),
                    selected_text.to_owned(),
                ))
                .or_insert(PersonalContextEvidence {
                    selections: 0,
                    rejections: 0,
                    last_selection_generation: self.generation,
                    last_observation_generation: self.generation,
                });
            entry.selections = entry.selections.saturating_add(1);
            entry.last_selection_generation = self.generation;
            entry.last_observation_generation = self.generation;
        }
        if let Some(overruled) = overruled_text.filter(|text| *text != selected_text) {
            let entry = self
                .entries
                .entry((
                    previous_text.to_owned(),
                    code.to_owned(),
                    overruled.to_owned(),
                ))
                .or_insert(PersonalContextEvidence {
                    selections: 0,
                    rejections: 0,
                    last_selection_generation: 0,
                    last_observation_generation: self.generation,
                });
            entry.rejections = entry.rejections.saturating_add(1);
            entry.last_observation_generation = self.generation;
        }
        while self.entries.len() > MAX_PERSONAL_CONTEXT_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|((previous, code, text), evidence)| {
                    (
                        evidence.last_observation_generation,
                        previous.as_str(),
                        code.as_str(),
                        text.as_str(),
                    )
                })
                .map(|(identity, _)| identity.clone())
                .expect("an over-capacity personal context table has one oldest entry");
            self.entries.remove(&oldest);
        }
        Ok(())
    }

    pub fn has_evidence(&self, previous_text: &str, code: &str) -> bool {
        valid_text(previous_text)
            && valid_code(code)
            && self
                .entries_for_context(previous_text, code)
                .next()
                .is_some()
    }

    /// Returns whether this context has any positive preference that the
    /// caller still permits.
    ///
    /// TSF uses this before widening its candidate search. Rejection-only,
    /// explicitly suppressed, or otherwise ineligible evidence must not make
    /// every ordinary key update decode a deeper page that cannot be promoted.
    pub fn has_eligible_preference(
        &self,
        previous_text: &str,
        code: &str,
        mut allowed: impl FnMut(&str) -> bool,
    ) -> bool {
        valid_text(previous_text)
            && valid_code(code)
            && self
                .entries_for_context(previous_text, code)
                .any(|((_, _, text), evidence)| effective_support(evidence) > 0 && allowed(text))
    }

    /// Moves at most one already-visible candidate to the first unprotected
    /// slot. Missing candidates are never inserted.
    pub fn promote_existing_text_after(
        &self,
        previous_text: &str,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        allowed: impl FnMut(&str) -> bool,
    ) -> bool {
        self.promote_existing_text_after_decision(
            previous_text,
            code,
            candidates,
            protected_prefix,
            allowed,
        )
        .is_some_and(|promotion| promotion.changed)
    }

    pub(crate) fn promote_existing_text_after_decision(
        &self,
        previous_text: &str,
        code: &str,
        candidates: &mut Vec<String>,
        protected_prefix: usize,
        mut allowed: impl FnMut(&str) -> bool,
    ) -> Option<CandidateTextPromotion> {
        if !valid_text(previous_text) || !valid_code(code) || candidates.is_empty() {
            return None;
        }
        let preferred = self
            .entries_for_context(previous_text, code)
            .filter(|((_, _, text), _)| {
                allowed(text) && candidates.iter().any(|candidate| candidate == text)
            })
            .max_by(|((_, _, left_text), left), ((_, _, right_text), right)| {
                effective_support(left)
                    .cmp(&effective_support(right))
                    .then_with(|| {
                        left.last_selection_generation
                            .cmp(&right.last_selection_generation)
                    })
                    .then_with(|| left.selections.cmp(&right.selections))
                    .then_with(|| right_text.cmp(left_text))
            })
            .and_then(|((_, _, text), evidence)| {
                (effective_support(evidence) > 0).then_some(text.as_str())
            });
        let preferred = preferred?;
        let index = candidates
            .iter()
            .position(|candidate| candidate == preferred)?;
        let protected_prefix = protected_prefix.min(candidates.len());
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

    fn entries_for_context<'a>(
        &'a self,
        previous_text: &str,
        code: &str,
    ) -> impl Iterator<Item = (&'a (String, String, String), &'a PersonalContextEvidence)> + 'a
    {
        let previous_text = previous_text.to_owned();
        let code = code.to_owned();
        let start = (previous_text.clone(), code.clone(), String::new());
        self.entries
            .range(start..)
            .take_while(move |((entry_previous, entry_code, _), _)| {
                entry_previous == &previous_text && entry_code == &code
            })
    }
}

fn effective_support(evidence: &PersonalContextEvidence) -> u64 {
    evidence
        .selections
        .min(PERSONAL_CONTEXT_SUPPORT_CAP)
        .saturating_sub(evidence.rejections.min(PERSONAL_CONTEXT_REJECTION_CAP))
}

fn validate_identity(
    previous_text: &str,
    code: &str,
    selected_text: &str,
) -> Result<(), PersonalContextError> {
    if !valid_text(previous_text) || !valid_code(code) || !valid_text(selected_text) {
        return Err(PersonalContextError::InvalidIdentity);
    }
    Ok(())
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_PERSONAL_CONTEXT_CODE_KEYS
        && code.as_bytes().iter().all(u8::is_ascii_lowercase)
}

fn valid_text(text: &str) -> bool {
    !text.is_empty()
        && !text.contains('\0')
        && text.chars().count() <= MAX_PERSONAL_CONTEXT_TEXT_CHARACTERS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_moves_only_an_existing_candidate_and_respects_the_protected_prefix() {
        let mut ranking = PersonalContextRanking::default();
        ranking.record("请", "ba", "把").unwrap();

        let mut candidates = vec!["吧".to_owned(), "八".to_owned(), "把".to_owned()];
        assert!(ranking.promote_existing_text_after("请", "ba", &mut candidates, 1, |_| true));
        assert_eq!(candidates, ["吧", "把", "八"]);

        let mut absent = vec!["吧".to_owned(), "八".to_owned()];
        assert!(!ranking.promote_existing_text_after("请", "ba", &mut absent, 0, |_| true));
        assert_eq!(absent, ["吧", "八"]);
    }

    #[test]
    fn exact_left_contexts_are_isolated_and_allowed_filter_wins() {
        let mut ranking = PersonalContextRanking::default();
        ranking.record("请", "ba", "把").unwrap();
        ranking.record("好", "ba", "吧").unwrap();
        let baseline = vec!["吧".to_owned(), "把".to_owned()];

        let mut matching = baseline.clone();
        assert!(ranking.promote_existing_text_after("请", "ba", &mut matching, 0, |_| true));
        assert_eq!(matching, ["把", "吧"]);

        let mut other = baseline.clone();
        assert!(!ranking.promote_existing_text_after("好", "ba", &mut other, 0, |_| true));
        assert_eq!(other, baseline);

        let mut suppressed = baseline.clone();
        assert!(
            !ranking
                .promote_existing_text_after("请", "ba", &mut suppressed, 0, |text| text != "把",)
        );
        assert_eq!(suppressed, baseline);
    }

    #[test]
    fn support_then_recency_produces_a_deterministic_bounded_preference() {
        let mut ranking = PersonalContextRanking::default();
        for _ in 0..2 {
            ranking.record("请", "ba", "把").unwrap();
        }
        ranking.record("请", "ba", "吧").unwrap();
        let mut candidates = vec!["吧".to_owned(), "把".to_owned()];
        assert!(ranking.promote_existing_text_after("请", "ba", &mut candidates, 0, |_| true));
        assert_eq!(candidates, ["把", "吧"]);

        ranking.record("请", "ba", "吧").unwrap();
        let mut tied = vec!["把".to_owned(), "吧".to_owned()];
        assert!(ranking.promote_existing_text_after("请", "ba", &mut tied, 0, |_| true));
        assert_eq!(tied, ["吧", "把"]);
    }

    #[test]
    fn repeated_explicit_overrides_gently_cancel_stale_context_support() {
        let mut ranking = PersonalContextRanking::default();
        for _ in 0..4 {
            ranking.record("请", "ba", "吧").unwrap();
        }

        ranking.record_choice("请", "ba", "把", Some("吧")).unwrap();
        let mut one_override = vec!["把".to_owned(), "吧".to_owned()];
        assert!(ranking.promote_existing_text_after("请", "ba", &mut one_override, 0, |_| true));
        assert_eq!(one_override, ["吧", "把"]);

        ranking.record_choice("请", "ba", "把", Some("吧")).unwrap();
        let mut repeated_override = vec!["吧".to_owned(), "把".to_owned()];
        assert!(ranking.promote_existing_text_after(
            "请",
            "ba",
            &mut repeated_override,
            0,
            |_| true
        ));
        assert_eq!(repeated_override, ["把", "吧"]);

        let mut unrelated = vec!["吧".to_owned(), "把".to_owned()];
        assert!(!ranking.promote_existing_text_after("好", "ba", &mut unrelated, 0, |_| true));
        assert_eq!(unrelated, ["吧", "把"]);
    }

    #[test]
    fn deep_search_gate_requires_positive_and_allowed_context_evidence() {
        let mut ranking = PersonalContextRanking::default();
        ranking.record("请", "ba", "吧").unwrap();
        ranking.record_choice("请", "ba", "把", Some("吧")).unwrap();

        assert!(ranking.has_evidence("请", "ba"));
        assert!(
            !ranking.has_eligible_preference("请", "ba", |text| text == "吧"),
            "one explicit rejection should cancel the stale one-count preference"
        );
        assert!(ranking.has_eligible_preference("请", "ba", |text| text == "把"));
        assert!(
            !ranking.has_eligible_preference("请", "ba", |_| false),
            "caller suppression must close the deep-search gate"
        );
    }

    #[test]
    fn capacity_validation_and_debug_output_are_private_and_bounded() {
        let mut ranking = PersonalContextRanking::default();
        assert_eq!(
            ranking.record("", "ba", "把"),
            Err(PersonalContextError::InvalidIdentity)
        );
        assert_eq!(
            ranking.record("请", "b1", "把"),
            Err(PersonalContextError::InvalidIdentity)
        );
        for index in 0..=MAX_PERSONAL_CONTEXT_ENTRIES {
            ranking.record(&format!("前{index}"), "ba", "把").unwrap();
        }
        assert_eq!(ranking.entry_count(), MAX_PERSONAL_CONTEXT_ENTRIES);
        let debug = format!("{ranking:?}");
        assert!(debug.contains("debug_contains_text: false"));
        assert!(!debug.contains("前2048"));
        assert!(!debug.contains('把'));
    }
}
