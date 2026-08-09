//! Bounded, read-only candidate snapshots for interactive hosts.
//!
//! This layer validates already-available bytes. It deliberately performs no
//! file discovery, persistence, decryption, learning, or network access.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use crate::{
    Correction, Decoder, KeySequence, KeySequenceError, MAX_LEXICON_SYLLABLES, ScoreBreakdown,
    parse_lexicon_tsv, spelling_is_complete_or_anchored_suffix,
};

/// First read-only candidate snapshot schema.
pub const CANDIDATE_SNAPSHOT_SCHEMA_V1: &str = "ziranma-candidate-snapshot-v1";
/// Maximum lexicon payload accepted by one in-memory candidate snapshot.
pub const MAX_CANDIDATE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum entries accepted before constructing the decoder index.
pub const MAX_CANDIDATE_SNAPSHOT_ENTRIES: usize = 131_072;
/// Maximum candidate rank exposed by the interactive snapshot interface.
///
/// Interactive hosts may reveal this bounded frontier lazily; they should not
/// request all ranks on every composition update.
pub const MAX_CANDIDATE_SNAPSHOT_RANK: usize = 50;
const MAX_CANDIDATE_SNAPSHOT_REVISION_BYTES: usize = 64;
const MAX_TRANSPOSITION_RECOVERY_KEYS: usize = 16;
const AUTOMATIC_TRANSPOSITION_CANDIDATE_DEPTH: usize = 6;
const FULL_CODE_CHARACTER_PAIR_DEPTH: usize = 24;
const MAX_FULL_CODE_CHARACTER_PAIRS: usize = MAX_CANDIDATE_SNAPSHOT_RANK;
/// Per-span core exact candidates retained by the mixed-layer path search.
pub const SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH: usize = 4;
const SUPPLEMENTAL_COMPOSITION_PATHS_PER_BOUNDARY: usize = 4;
/// Supplemental-only exact candidates retained for one mixed-layer span.
pub const SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH: usize = 4;
const SUPPLEMENTAL_COMPOSITION_SEARCH_DEPTH: usize = 8;
/// Longest complete-code input considered by the extra mixed-layer lane.
///
/// Longer input keeps the ordinary core result and avoids an unbounded
/// boundary matrix in an interactive host.
pub const MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InteractiveCandidateSource {
    CoreExact,
    SupplementalExact,
    CharacterPair,
    CompleteSentence,
    Decoder,
    FourCharacterCorrection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InteractiveCandidateText {
    pub(crate) text: String,
    pub(crate) source: InteractiveCandidateSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InteractiveCandidateQuery {
    pub(crate) candidates: Vec<InteractiveCandidateText>,
    pub(crate) automatic_transposition_blocked: bool,
}

/// A conservative, host-independent decision about one likely reversed
/// double-pinyin pair.
///
/// The decision is advisory: interactive hosts do not consume it unless they
/// opt in separately. This keeps policy experiments out of the primary
/// candidate path while making every acceptance and rejection observable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomaticTranspositionDecision {
    /// Keep the ordinary candidate order unchanged.
    KeepPrimary(AutomaticTranspositionKeepReason),
    /// One unique within-syllable swap produced exact whole-word evidence.
    PromoteExactFullCode(AutomaticTranspositionPromotion),
}

/// Structural reason why automatic transposition did not change the order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomaticTranspositionKeepReason {
    /// Automatic recovery only handles bounded, even-length full-code shapes.
    UnsupportedInputShape,
    /// The observed keys already name at least one exact whole-word entry.
    OriginalHasExactFullCode,
    /// The ordinary first sentence already uses complete, uncorrected pairs.
    OriginalFirstCandidateIsComplete,
    /// No within-syllable swap produced an exact whole-word entry.
    NoExactFullCodeRecovery,
    /// More than one syllable position produced exact whole-word evidence.
    AmbiguousSwapLocations,
}

/// Exact evidence admitted by the automatic transposition gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomaticTranspositionPromotion {
    /// Zero-based double-pinyin syllable whose two keys were reversed.
    pub syllable_index: usize,
    /// Corrected complete double-pinyin code.
    pub intended_code: String,
    /// Exact whole-word candidates in their ordinary lexicon order.
    pub candidates: Vec<String>,
}

/// One complete four-character whole word reached by exactly one key edit.
///
/// The snapshot only exposes evidence here. It does not promote or commit the
/// candidate, and callers can inspect the intended code, correction, and
/// score before choosing an interaction policy.
#[derive(Clone, Debug, PartialEq)]
pub struct FourCharacterCorrectionCandidate {
    /// Public whole-word text recorded by the snapshot.
    pub text: String,
    /// Full pinyin recorded by the public lexicon.
    pub pinyin: String,
    /// Corrected canonical eight-key double-pinyin code.
    pub intended_code: String,
    /// The one supported edit relating the observed keys to the full code.
    pub correction: Correction,
    /// Transparent score produced by the ordinary decoder configuration.
    pub score: ScoreBreakdown,
}

/// Host-independent decision for the narrow four-character correction lane.
#[derive(Clone, Debug, PartialEq)]
pub enum FourCharacterCorrectionDecision {
    /// Preserve the ordinary candidates without inserting a recovery.
    KeepOrdinary(FourCharacterCorrectionKeepReason),
    /// One corrected canonical code is safe to expose as an advisory lane.
    Offer(FourCharacterCorrectionOffer),
}

/// Structural reason why four-character recovery stayed hidden.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FourCharacterCorrectionKeepReason {
    /// A four-syllable complete code cannot be one edit from this input shape.
    UnsupportedInputShape,
    /// The observed eight keys already name a complete four-character word.
    OriginalHasExactFullCode,
    /// Neither public layer contained a complete four-character one-edit word.
    NoSingleEditRecovery,
    /// One edit led to more than one canonical eight-key code.
    AmbiguousIntendedCodes,
}

/// One unambiguous corrected code and its bounded exact-word candidates.
#[derive(Clone, Debug, PartialEq)]
pub struct FourCharacterCorrectionOffer {
    /// The sole canonical code reached by a supported single edit.
    pub intended_code: String,
    /// Core candidates first, then new supplemental candidates, without
    /// comparing unrelated raw frequency scales across the two sources.
    pub candidates: Vec<FourCharacterCorrectionCandidate>,
}

/// Explicit influence bound for one independent supplemental public lexicon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupplementalCandidateLayerConfig {
    /// Maximum unique supplemental exact words admitted beside core exact
    /// words. A core exact Top-1, when present, always stays first; once a new
    /// supplemental exact word is admitted, permissive sentence paths do not
    /// fill the rest of that result.
    pub exact_promotions: usize,
}

/// Invalid configuration for the pure supplemental candidate merge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupplementalCandidateLayerError {
    /// The requested promotion count exceeds the snapshot rank boundary.
    PromotionLimit,
}

/// Metadata and payload supplied explicitly to the snapshot validator.
#[derive(Clone, Copy, Debug)]
pub struct CandidateSnapshotDescriptor<'a> {
    /// Exact snapshot schema.
    pub schema: &'a str,
    /// Bounded ASCII data revision.
    pub revision: &'a str,
    /// Whether the payload contains private text.
    pub contains_private_text: bool,
    /// Auditable lexicon TSV payload.
    pub lexicon_tsv: &'a str,
    /// Expected payload size in bytes.
    pub expected_payload_bytes: usize,
    /// Expected stable FNV-1a payload fingerprint.
    pub expected_payload_fingerprint: u64,
    /// Expected number of lexicon entries after strict parsing.
    pub expected_entry_count: usize,
}

/// Validated, immutable candidate data and its decoder index.
#[derive(Debug)]
pub struct CandidateSnapshot {
    revision: String,
    contains_private_text: bool,
    payload_bytes: usize,
    payload_fingerprint: u64,
    entry_count: usize,
    decoder: Decoder,
}

impl CandidateSnapshot {
    /// Validates one explicitly supplied snapshot and builds its decoder.
    pub fn load(
        descriptor: CandidateSnapshotDescriptor<'_>,
    ) -> Result<Self, CandidateSnapshotError> {
        if descriptor.schema != CANDIDATE_SNAPSHOT_SCHEMA_V1 {
            return Err(CandidateSnapshotError::UnsupportedSchema);
        }
        if !valid_candidate_snapshot_revision(descriptor.revision) {
            return Err(CandidateSnapshotError::InvalidRevision);
        }

        let actual_payload_bytes = descriptor.lexicon_tsv.len();
        let bounded_bytes = actual_payload_bytes.max(descriptor.expected_payload_bytes);
        if bounded_bytes > MAX_CANDIDATE_SNAPSHOT_BYTES {
            return Err(CandidateSnapshotError::PayloadTooLarge {
                actual: bounded_bytes,
                maximum: MAX_CANDIDATE_SNAPSHOT_BYTES,
            });
        }
        if actual_payload_bytes != descriptor.expected_payload_bytes {
            return Err(CandidateSnapshotError::PayloadLengthMismatch {
                expected: descriptor.expected_payload_bytes,
                actual: actual_payload_bytes,
            });
        }

        let actual_fingerprint = candidate_payload_fingerprint(descriptor.lexicon_tsv.as_bytes());
        if actual_fingerprint != descriptor.expected_payload_fingerprint {
            return Err(CandidateSnapshotError::PayloadFingerprintMismatch {
                expected: descriptor.expected_payload_fingerprint,
                actual: actual_fingerprint,
            });
        }
        if descriptor.expected_entry_count == 0
            || descriptor.expected_entry_count > MAX_CANDIDATE_SNAPSHOT_ENTRIES
        {
            return Err(CandidateSnapshotError::InvalidExpectedEntryCount {
                count: descriptor.expected_entry_count,
                maximum: MAX_CANDIDATE_SNAPSHOT_ENTRIES,
            });
        }

        let entries = parse_lexicon_tsv(descriptor.lexicon_tsv)
            .map_err(|_| CandidateSnapshotError::Lexicon)?;
        if entries.len() != descriptor.expected_entry_count {
            return Err(CandidateSnapshotError::EntryCountMismatch {
                expected: descriptor.expected_entry_count,
                actual: entries.len(),
            });
        }

        Ok(Self {
            revision: descriptor.revision.to_owned(),
            contains_private_text: descriptor.contains_private_text,
            payload_bytes: actual_payload_bytes,
            payload_fingerprint: actual_fingerprint,
            entry_count: entries.len(),
            decoder: Decoder::new(entries),
        })
    }

    /// Returns the validated data revision.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Reports whether the descriptor marked the payload as private text.
    pub fn contains_private_text(&self) -> bool {
        self.contains_private_text
    }

    /// Returns the exact validated payload size.
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    /// Returns the stable corruption-detection fingerprint.
    pub fn payload_fingerprint(&self) -> u64 {
        self.payload_fingerprint
    }

    /// Returns the exact validated lexicon entry count.
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// Returns one one-based candidate for an interactive host.
    ///
    /// A first candidate containing unresolved input is replaced with the raw
    /// composition. This keeps a small or stale snapshot from swallowing text
    /// or exposing the research decoder's unresolved markers to the host.
    pub fn candidate_text(
        &self,
        code: &str,
        rank: usize,
    ) -> Result<Option<String>, KeySequenceError> {
        if !(1..=MAX_CANDIDATE_SNAPSHOT_RANK).contains(&rank) {
            return Ok(None);
        }
        let mut candidates = self.interactive_candidate_texts(code, rank)?;
        Ok((candidates.len() == rank)
            .then(|| candidates.pop())
            .flatten())
    }

    /// Returns one contiguous candidate page for an interactive host.
    ///
    /// The decoder runs once at the requested bounded depth. Fully resolved
    /// candidates retain their order. If the first result is unresolved, the
    /// raw composition is returned as the only fallback; later unresolved
    /// results end the visible page instead of exposing research markers.
    pub fn candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.interactive_candidate_texts(code, limit)
    }

    /// Returns exact whole-word candidates for a complete double-pinyin code.
    ///
    /// Unlike [`Self::candidate_texts`], this view does not include sentence
    /// segmentation, abbreviations, corrections, or unresolved fallbacks.
    pub fn exact_full_code_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
        self.decoder
            .decode_exact_full_code(code, limit)
            .map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| candidate.text)
                    .collect()
            })
    }

    /// Finds complete four-character public whole words behind one key edit.
    ///
    /// Only four full double-pinyin syllables are accepted. Mixed initials,
    /// sentence paths, unresolved input, and a second correction are excluded.
    /// Results remain advisory and do not alter the ordinary candidate order.
    pub fn four_character_correction_candidates(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<FourCharacterCorrectionCandidate>, KeySequenceError> {
        let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
        self.decoder
            .decode_complete_word_single_edit(code, 4, limit)
            .map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| FourCharacterCorrectionCandidate {
                        text: candidate.text,
                        pinyin: candidate.pinyin,
                        intended_code: candidate.code.as_str().to_owned(),
                        correction: candidate.correction,
                        score: candidate.score,
                    })
                    .collect()
            })
    }

    fn interactive_candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        self.decoder.interactive_candidate_texts(code, limit)
    }

    pub(crate) fn interactive_candidate_query(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<InteractiveCandidateQuery, KeySequenceError> {
        self.decoder.interactive_candidate_query(code, limit)
    }

    /// Returns the explicitly requested adjacent-transposition recovery view.
    ///
    /// This does not merge corrected candidates into the conservative primary
    /// ordering. Interactive hosts can expose it behind a deliberate action
    /// such as Shift+Tab. Only fully resolved snapshot candidates that need no
    /// second correction are returned.
    pub fn transposition_recovery_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
        if limit == 0 {
            KeySequence::new(code)?;
            return Ok(Vec::new());
        }
        let observed = KeySequence::new(code)?;
        let code = observed.as_str();
        if code.len() < 2 || code.len() > MAX_TRANSPOSITION_RECOVERY_KEYS {
            return Ok(Vec::new());
        }
        let primary = self.decoder.decode_sentence(code, limit)?;
        let primary_texts = primary
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect::<HashSet<_>>();
        let mut recovered = Vec::new();
        let original = code.as_bytes();
        for swap_start in 0..original.len() - 1 {
            if original[swap_start] == original[swap_start + 1] {
                continue;
            }
            let mut swapped = original.to_vec();
            swapped.swap(swap_start, swap_start + 1);
            let swapped = std::str::from_utf8(&swapped)
                .expect("a validated lowercase ASCII key sequence remains UTF-8 after swapping");
            for candidate in self.decoder.decode_sentence(swapped, limit)? {
                if candidate.used_error
                    || candidate.unresolved_key_count != 0
                    || !candidate
                        .segments
                        .iter()
                        .all(|segment| spelling_is_complete_or_anchored_suffix(&segment.candidate))
                    || primary_texts.contains(candidate.text.as_str())
                {
                    continue;
                }
                recovered.push((candidate.total_score, swap_start, candidate.text));
            }
        }
        recovered.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        let mut seen = HashSet::new();
        recovered.retain(|(_, _, text)| seen.insert(text.clone()));
        recovered.truncate(limit);
        Ok(recovered.into_iter().map(|(_, _, text)| text).collect())
    }

    /// Evaluates the conservative automatic within-syllable transposition
    /// gate without changing the snapshot's ordinary candidate order.
    pub fn automatic_transposition_decision(
        &self,
        code: &str,
    ) -> Result<AutomaticTranspositionDecision, KeySequenceError> {
        self.decoder.automatic_transposition_decision(code)
    }

    pub(crate) fn automatic_transposition_recovery_after_primary(
        &self,
        code: &str,
        syllable_index: usize,
        limit: usize,
    ) -> Result<Option<AutomaticTranspositionPromotion>, KeySequenceError> {
        self.decoder
            .automatic_transposition_recovery_after_primary(code, syllable_index, limit)
    }

    pub(crate) fn automatic_transposition_span_recovery_after_primary(
        &self,
        code: &str,
        first_syllable_index: usize,
        syllable_count: usize,
        limit: usize,
    ) -> Result<Option<AutomaticTranspositionPromotion>, KeySequenceError> {
        // The TSF caller supplies the just-completed pair identified by its
        // local delivery interval, or two adjacent completed pairs whose
        // intervals both remain available. Unlike the host-independent decision
        // above, this causal probe must not let an unrelated earlier swappable
        // pair erase evidence for the requested location.
        self.decoder
            .automatic_transposition_span_recovery_after_primary(
                code,
                first_syllable_index,
                syllable_count,
                limit,
            )
    }
}

/// Decides whether one complete four-character correction is unambiguous
/// across the core and optional supplemental public snapshots.
///
/// Exact observed words win immediately. Otherwise every supported one-edit
/// result must agree on one canonical code; raw frequencies from different
/// snapshots are never compared.
pub fn layered_four_character_correction_decision(
    core: &CandidateSnapshot,
    supplemental: Option<&CandidateSnapshot>,
    code: &str,
    limit: usize,
) -> Result<FourCharacterCorrectionDecision, KeySequenceError> {
    let observed = KeySequence::new(code)?;
    if !(7..=9).contains(&observed.as_str().len()) {
        return Ok(FourCharacterCorrectionDecision::KeepOrdinary(
            FourCharacterCorrectionKeepReason::UnsupportedInputShape,
        ));
    }
    let supplemental_has_exact = if let Some(snapshot) = supplemental {
        !snapshot.exact_full_code_texts(code, 1)?.is_empty()
    } else {
        false
    };
    if observed.as_str().len() == 8
        && (!core.exact_full_code_texts(code, 1)?.is_empty() || supplemental_has_exact)
    {
        return Ok(FourCharacterCorrectionDecision::KeepOrdinary(
            FourCharacterCorrectionKeepReason::OriginalHasExactFullCode,
        ));
    }

    let mut candidates = core
        .decoder
        .decode_complete_word_single_edit(code, 4, usize::MAX)?;
    if let Some(supplemental) = supplemental {
        candidates.extend(supplemental.decoder.decode_complete_word_single_edit(
            code,
            4,
            usize::MAX,
        )?);
    }
    let mut seen = HashSet::new();
    candidates.retain(|candidate| {
        seen.insert((candidate.text.clone(), candidate.code.as_str().to_owned()))
    });
    if candidates.is_empty() {
        return Ok(FourCharacterCorrectionDecision::KeepOrdinary(
            FourCharacterCorrectionKeepReason::NoSingleEditRecovery,
        ));
    }
    let intended_codes = candidates
        .iter()
        .map(|candidate| candidate.code.as_str())
        .collect::<HashSet<_>>();
    if intended_codes.len() != 1 {
        return Ok(FourCharacterCorrectionDecision::KeepOrdinary(
            FourCharacterCorrectionKeepReason::AmbiguousIntendedCodes,
        ));
    }
    let intended_code = intended_codes
        .into_iter()
        .next()
        .expect("a non-empty correction set has one intended code")
        .to_owned();
    let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
    let candidates = candidates
        .into_iter()
        .take(limit)
        .map(|candidate| FourCharacterCorrectionCandidate {
            text: candidate.text,
            pinyin: candidate.pinyin,
            intended_code: candidate.code.as_str().to_owned(),
            correction: candidate.correction,
            score: candidate.score,
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(FourCharacterCorrectionDecision::KeepOrdinary(
            FourCharacterCorrectionKeepReason::NoSingleEditRecovery,
        ));
    }
    Ok(FourCharacterCorrectionDecision::Offer(
        FourCharacterCorrectionOffer {
            intended_code,
            candidates,
        },
    ))
}

/// Merges one core and one supplemental public snapshot without comparing
/// their unrelated raw frequency scales.
///
/// Supplemental exact whole words can precede permissive core sentence paths,
/// but they never displace an existing core exact Top-1. Once a new
/// supplemental exact word is admitted, the visible result stays within the
/// exact whole-word lanes and the bounded two-character full-code lane instead
/// of filling unused ranks with permissive core abbreviation paths. With no
/// admitted supplemental word, the core result is preserved unchanged.
pub fn layered_candidate_texts(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<Vec<String>, LayeredCandidateTextsError> {
    layered_candidate_texts_with_sources(core, supplemental, code, limit, config).map(
        |candidates| {
            candidates
                .into_iter()
                .map(|candidate| candidate.text)
                .collect()
        },
    )
}

pub(crate) fn layered_candidate_texts_with_sources(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<Vec<InteractiveCandidateText>, LayeredCandidateTextsError> {
    layered_candidate_query_with_sources(core, supplemental, code, limit, config)
        .map(|query| query.candidates)
}

/// Applies one host-facing cold-order calibration on top of the ordinary
/// public layer merge.
///
/// Raw frequency scales from unrelated dictionaries remain incomparable. A
/// supplemental Top-1 may therefore move to the front only when the core
/// dictionary independently confirms the same text as an exact candidate for
/// the same complete code. New supplemental-only words keep the conservative
/// merge order.
pub fn layered_candidate_texts_with_consensus(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<Vec<String>, LayeredCandidateTextsError> {
    layered_candidate_query_with_consensus_sources(core, supplemental, code, limit, config).map(
        |query| {
            query
                .candidates
                .into_iter()
                .map(|candidate| candidate.text)
                .collect()
        },
    )
}

pub(crate) fn layered_candidate_query_with_consensus_sources(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<InteractiveCandidateQuery, LayeredCandidateTextsError> {
    let mut query = layered_candidate_query_with_sources(core, supplemental, code, limit, config)?;
    let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
    if config.exact_promotions == 0 || limit == 0 {
        return Ok(query);
    }
    let core_exact = core.exact_full_code_texts(code, MAX_CANDIDATE_SNAPSHOT_RANK)?;
    let supplemental_top = supplemental
        .exact_full_code_texts(code, 1)?
        .into_iter()
        .next();
    let Some(consensus_top) =
        supplemental_top.filter(|candidate| core_exact.iter().any(|core| core == candidate))
    else {
        return Ok(query);
    };
    if let Some(index) = query
        .candidates
        .iter()
        .position(|candidate| candidate.text == consensus_top)
        .filter(|index| *index != 0)
    {
        let candidate = query.candidates.remove(index);
        query.candidates.insert(0, candidate);
    } else if query
        .candidates
        .first()
        .is_none_or(|candidate| candidate.text != consensus_top)
    {
        query.candidates.insert(
            0,
            InteractiveCandidateText {
                text: consensus_top,
                source: InteractiveCandidateSource::CoreExact,
            },
        );
        query.candidates.truncate(limit);
    }
    Ok(query)
}

pub(crate) fn layered_candidate_query_with_sources(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<InteractiveCandidateQuery, LayeredCandidateTextsError> {
    if limit == 0 {
        KeySequence::new(code)?;
        validate_supplemental_config(config)?;
        return Ok(InteractiveCandidateQuery {
            candidates: Vec::new(),
            automatic_transposition_blocked: false,
        });
    }
    let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
    let core_exact = core.exact_full_code_texts(code, limit)?;
    let supplemental_exact = supplemental.exact_full_code_texts(code, limit)?;
    let core_query = core.interactive_candidate_query(code, limit)?;
    let mut automatic_transposition_blocked =
        core_query.automatic_transposition_blocked || !supplemental_exact.is_empty();
    let core_primary = core_query.candidates;
    let core_primary_texts = core_primary
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect::<Vec<_>>();
    let mut merged = merge_candidate_text_layers(
        &core_exact,
        &supplemental_exact,
        &core_primary_texts,
        limit,
        config,
    )?;
    let promoted_supplemental_exact = supplemental_exact
        .iter()
        .filter(|candidate| !core_exact.iter().any(|core| core == *candidate))
        .take(config.exact_promotions)
        .cloned()
        .collect::<HashSet<_>>();
    let mut seen = merged.iter().cloned().collect::<HashSet<_>>();
    // A supplemental whole word may suppress permissive core abbreviation
    // paths, but it must not erase complete two-key-per-syllable paths. Those
    // are the ordinary way an interactive user reaches a phrase assembled
    // from known public words (for example `打` + `成了`). Preserve both
    // strong composition lanes before considering one extra mixed-layer path.
    for candidate in core_primary
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.source,
                InteractiveCandidateSource::CharacterPair
                    | InteractiveCandidateSource::CompleteSentence
            )
        })
        .map(|candidate| &candidate.text)
    {
        if merged.len() == limit {
            break;
        }
        push_unique(&mut merged, &mut seen, candidate, limit);
    }
    let mut promoted_composition = None;
    if config.exact_promotions != 0 {
        let composition_candidates = supplemental_complete_composition_texts(
            core,
            supplemental,
            code,
            SUPPLEMENTAL_COMPOSITION_SEARCH_DEPTH,
        )?;
        let whole_exact_texts = core_exact
            .iter()
            .chain(&supplemental_exact)
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let whole_exact_prefix = merged
            .iter()
            .take_while(|text| whole_exact_texts.contains(text.as_str()))
            .count();
        let core_primary_top_boundary = core_primary
            .iter()
            .find(|candidate| {
                candidate.text != code && !whole_exact_texts.contains(candidate.text.as_str())
            })
            .and_then(|candidate| {
                merged
                    .iter()
                    .position(|text| text == &candidate.text)
                    .map(|index| index + 1)
            })
            .unwrap_or(0);
        let insertion_index = whole_exact_prefix.max(core_primary_top_boundary);
        for candidate in composition_candidates {
            if whole_exact_texts.contains(candidate.as_str()) {
                continue;
            }
            if let Some(existing_index) = merged.iter().position(|text| text == &candidate) {
                if existing_index < insertion_index {
                    continue;
                }
                merged.remove(existing_index);
            }
            merged.insert(insertion_index.min(merged.len()), candidate.clone());
            merged.truncate(limit);
            promoted_composition = Some(candidate);
            automatic_transposition_blocked = true;
            break;
        }
    }
    let core_exact = core_exact.into_iter().collect::<HashSet<_>>();
    Ok(InteractiveCandidateQuery {
        candidates: merged
            .into_iter()
            .map(|text| {
                let source = if core_exact.contains(&text) {
                    InteractiveCandidateSource::CoreExact
                } else if promoted_supplemental_exact.contains(&text)
                    || promoted_composition.as_ref() == Some(&text)
                {
                    InteractiveCandidateSource::SupplementalExact
                } else {
                    core_primary
                        .iter()
                        .find(|candidate| candidate.text == text)
                        .map(|candidate| candidate.source)
                        .unwrap_or(InteractiveCandidateSource::Decoder)
                };
                InteractiveCandidateText { text, source }
            })
            .collect(),
        automatic_transposition_blocked,
    })
}

/// Deterministically merges already decoded candidate lanes.
pub fn merge_candidate_text_layers(
    core_exact: &[String],
    supplemental_exact: &[String],
    core_primary: &[String],
    limit: usize,
    config: SupplementalCandidateLayerConfig,
) -> Result<Vec<String>, SupplementalCandidateLayerError> {
    validate_supplemental_config(config)?;
    let limit = limit.min(MAX_CANDIDATE_SNAPSHOT_RANK);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut merged = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    let core_exact_texts = core_exact
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if let Some(core_top) = core_exact.first() {
        push_unique(&mut merged, &mut seen, core_top, limit);
    }
    let mut promoted = 0;
    for candidate in supplemental_exact {
        if promoted == config.exact_promotions || merged.len() == limit {
            break;
        }
        if core_exact_texts.contains(candidate.as_str()) {
            continue;
        }
        if push_unique(&mut merged, &mut seen, candidate, limit) {
            promoted += 1;
        }
    }
    for candidate in core_exact {
        if merged.len() == limit {
            break;
        }
        push_unique(&mut merged, &mut seen, candidate, limit);
    }
    if promoted == 0 {
        for candidate in core_primary {
            if merged.len() == limit {
                break;
            }
            push_unique(&mut merged, &mut seen, candidate, limit);
        }
    }
    Ok(merged)
}

fn validate_supplemental_config(
    config: SupplementalCandidateLayerConfig,
) -> Result<(), SupplementalCandidateLayerError> {
    if config.exact_promotions > MAX_CANDIDATE_SNAPSHOT_RANK {
        Err(SupplementalCandidateLayerError::PromotionLimit)
    } else {
        Ok(())
    }
}

fn push_unique(
    output: &mut Vec<String>,
    seen: &mut HashSet<String>,
    candidate: &str,
    limit: usize,
) -> bool {
    if output.len() == limit || !seen.insert(candidate.to_owned()) {
        return false;
    }
    output.push(candidate.to_owned());
    true
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CoreCompletePath {
    text: String,
    segment_lengths: Vec<usize>,
    edge_ranks: Vec<usize>,
    segments: Vec<CoreCompleteSegment>,
}

impl CoreCompletePath {
    fn empty() -> Self {
        Self {
            text: String::new(),
            segment_lengths: Vec::new(),
            edge_ranks: Vec::new(),
            segments: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CoreCompleteSegment {
    text: String,
    syllable_count: usize,
    local_rank: usize,
}

/// Source layer for one exact segment in a supplemental composition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupplementalCompositionSegmentSource {
    /// Exact segment supplied by the unchanged core snapshot.
    Core,
    /// The one exact multi-syllable segment supplied only by the supplemental snapshot.
    Supplemental,
}

/// Explainable segment evidence for one bounded supplemental composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupplementalCompositionSegment {
    text: String,
    source: SupplementalCompositionSegmentSource,
    syllable_count: usize,
    local_rank: usize,
}

impl SupplementalCompositionSegment {
    /// Returns the exact candidate text contributed by this segment.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether this segment came from the core or supplemental snapshot.
    pub fn source(&self) -> SupplementalCompositionSegmentSource {
        self.source
    }

    /// Returns the number of complete double-pinyin syllables consumed by this segment.
    pub fn syllable_count(&self) -> usize {
        self.syllable_count
    }

    /// Returns the one-based exact-candidate rank inside the segment's own source snapshot.
    pub fn local_rank(&self) -> usize {
        self.local_rank
    }
}

/// One deduplicated mixed-layer composition with its exact word-boundary evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupplementalCompositionCandidate {
    text: String,
    segments: Vec<SupplementalCompositionSegment>,
}

impl SupplementalCompositionCandidate {
    /// Returns the complete text produced by concatenating every exact segment.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the ordered, non-empty exact segments that form this candidate.
    pub fn segments(&self) -> &[SupplementalCompositionSegment] {
        &self.segments
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SupplementalCompleteComposition {
    candidate: SupplementalCompositionCandidate,
    supplemental_syllables: usize,
    core_segment_count: usize,
    supplemental_rank: usize,
    core_edge_ranks: Vec<usize>,
    supplemental_start: usize,
}

/// Fixed, frequency-scale-independent orders available to public audits.
///
/// The interactive path uses [`SupplementalCompositionOrder::StructuralV1`].
/// The other variants let aggregate public audits test one ordering assumption
/// at a time without changing the runtime policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupplementalCompositionOrder {
    /// Prefer a longer supplemental span, then fewer surrounding core words.
    StructuralV1,
    /// Prefer fewer surrounding core words before supplemental span length.
    FewerSegmentsFirst,
    /// Prefer each source's bounded local candidate ranks before structure.
    LocalRanksFirst,
}

/// Builds a deliberately narrow mixed-layer lane:
///
/// - the observed input consists only of complete two-key syllables;
/// - exactly one multi-syllable word comes from the supplemental exact lane;
/// - that word is absent from the bounded core exact lane for the same span;
/// - every non-empty prefix and suffix is fully covered by core exact words.
///
/// Raw frequencies from the two snapshots are never compared. Structural
/// specificity is considered first, followed by each layer's own candidate
/// order. The caller may promote at most one result.
pub fn supplemental_complete_composition_texts(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
) -> Result<Vec<String>, KeySequenceError> {
    supplemental_complete_composition_texts_with_order(
        core,
        supplemental,
        code,
        limit,
        SupplementalCompositionOrder::StructuralV1,
    )
}

/// Builds the same bounded mixed-layer lane under one explicit audit order.
///
/// This function never compares raw weights across snapshots. Runtime callers
/// should use [`supplemental_complete_composition_texts`]; alternate orders are
/// intended for frozen public comparisons before any policy change.
pub fn supplemental_complete_composition_texts_with_order(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    order: SupplementalCompositionOrder,
) -> Result<Vec<String>, KeySequenceError> {
    Ok(
        supplemental_complete_compositions_with_order(core, supplemental, code, limit, order)?
            .into_iter()
            .map(|candidate| candidate.text)
            .collect(),
    )
}

/// Builds the bounded mixed-layer lane with exact word-boundary evidence.
///
/// This is the explainable audit counterpart of
/// [`supplemental_complete_composition_texts_with_order`]. It runs the same
/// search, order, text deduplication, and limit; it only retains the already
/// known segment boundaries and each source-local rank. It does not score or
/// promote a candidate.
pub fn supplemental_complete_compositions_with_order(
    core: &CandidateSnapshot,
    supplemental: &CandidateSnapshot,
    code: &str,
    limit: usize,
    order: SupplementalCompositionOrder,
) -> Result<Vec<SupplementalCompositionCandidate>, KeySequenceError> {
    let observed = KeySequence::new(code)?;
    let code = observed.as_str();
    if limit == 0
        || code.len() < 6
        || !code.len().is_multiple_of(2)
        || code.len() / 2 > MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES
    {
        return Ok(Vec::new());
    }

    let syllable_count = code.len() / 2;
    let mut supplemental_spans = Vec::new();
    for start in 0..syllable_count {
        let maximum_end = syllable_count.min(start + MAX_LEXICON_SYLLABLES);
        for end in start + 2..=maximum_end {
            if start == 0 && end == syllable_count {
                continue;
            }
            let span = &code[start * 2..end * 2];
            let exact = supplemental.exact_full_code_texts(span, MAX_CANDIDATE_SNAPSHOT_RANK)?;
            if !exact.is_empty() {
                supplemental_spans.push((start, end, exact));
            }
        }
    }
    if supplemental_spans.is_empty() {
        return Ok(Vec::new());
    }

    let core_edges = exact_core_edges(core, code, syllable_count)?;
    let (prefixes, suffixes) = core_complete_boundary_paths(&core_edges, syllable_count);
    let mut compositions = Vec::new();

    for (supplemental_start, supplemental_end, supplemental_exact) in supplemental_spans {
        if prefixes[supplemental_start].is_empty() || suffixes[supplemental_end].is_empty() {
            continue;
        }
        let core_exact = core_edges[supplemental_start][supplemental_end]
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let supplemental_only = supplemental_exact
            .into_iter()
            .enumerate()
            .filter(|(_, text)| !core_exact.contains(text.as_str()))
            .take(SUPPLEMENTAL_COMPOSITION_EDGE_DEPTH)
            .collect::<Vec<_>>();

        for prefix in &prefixes[supplemental_start] {
            for (supplemental_rank, supplemental_text) in &supplemental_only {
                for suffix in &suffixes[supplemental_end] {
                    let mut core_edge_ranks = prefix.edge_ranks.clone();
                    core_edge_ranks.extend_from_slice(&suffix.edge_ranks);
                    let mut segments = prefix
                        .segments
                        .iter()
                        .map(|segment| SupplementalCompositionSegment {
                            text: segment.text.clone(),
                            source: SupplementalCompositionSegmentSource::Core,
                            syllable_count: segment.syllable_count,
                            local_rank: segment.local_rank,
                        })
                        .collect::<Vec<_>>();
                    segments.push(SupplementalCompositionSegment {
                        text: supplemental_text.clone(),
                        source: SupplementalCompositionSegmentSource::Supplemental,
                        syllable_count: supplemental_end - supplemental_start,
                        local_rank: supplemental_rank + 1,
                    });
                    segments.extend(suffix.segments.iter().map(|segment| {
                        SupplementalCompositionSegment {
                            text: segment.text.clone(),
                            source: SupplementalCompositionSegmentSource::Core,
                            syllable_count: segment.syllable_count,
                            local_rank: segment.local_rank,
                        }
                    }));
                    compositions.push(SupplementalCompleteComposition {
                        candidate: SupplementalCompositionCandidate {
                            text: format!("{}{}{}", prefix.text, supplemental_text, suffix.text),
                            segments,
                        },
                        supplemental_syllables: supplemental_end - supplemental_start,
                        core_segment_count: prefix.segment_lengths.len()
                            + suffix.segment_lengths.len(),
                        supplemental_rank: *supplemental_rank,
                        core_edge_ranks,
                        supplemental_start,
                    });
                }
            }
        }
    }

    compositions.sort_by(|left, right| supplemental_composition_order(left, right, order));
    let mut seen = HashSet::new();
    let mut visible = Vec::with_capacity(limit.min(compositions.len()));
    for composition in compositions {
        if seen.insert(composition.candidate.text.clone()) {
            visible.push(composition.candidate);
            if visible.len() == limit {
                break;
            }
        }
    }
    Ok(visible)
}

fn supplemental_composition_order(
    left: &SupplementalCompleteComposition,
    right: &SupplementalCompleteComposition,
    order: SupplementalCompositionOrder,
) -> std::cmp::Ordering {
    let stable_tail = || {
        left.supplemental_start
            .cmp(&right.supplemental_start)
            .then_with(|| left.candidate.text.cmp(&right.candidate.text))
    };
    match order {
        SupplementalCompositionOrder::StructuralV1 => right
            .supplemental_syllables
            .cmp(&left.supplemental_syllables)
            .then_with(|| left.core_segment_count.cmp(&right.core_segment_count))
            .then_with(|| left.supplemental_rank.cmp(&right.supplemental_rank))
            .then_with(|| left.core_edge_ranks.cmp(&right.core_edge_ranks))
            .then_with(stable_tail),
        SupplementalCompositionOrder::FewerSegmentsFirst => left
            .core_segment_count
            .cmp(&right.core_segment_count)
            .then_with(|| {
                right
                    .supplemental_syllables
                    .cmp(&left.supplemental_syllables)
            })
            .then_with(|| left.supplemental_rank.cmp(&right.supplemental_rank))
            .then_with(|| left.core_edge_ranks.cmp(&right.core_edge_ranks))
            .then_with(stable_tail),
        SupplementalCompositionOrder::LocalRanksFirst => left
            .supplemental_rank
            .cmp(&right.supplemental_rank)
            .then_with(|| left.core_edge_ranks.cmp(&right.core_edge_ranks))
            .then_with(|| left.core_segment_count.cmp(&right.core_segment_count))
            .then_with(|| {
                right
                    .supplemental_syllables
                    .cmp(&left.supplemental_syllables)
            })
            .then_with(stable_tail),
    }
}

fn exact_core_edges(
    core: &CandidateSnapshot,
    code: &str,
    syllable_count: usize,
) -> Result<Vec<Vec<Vec<String>>>, KeySequenceError> {
    let mut edges = vec![vec![Vec::new(); syllable_count + 1]; syllable_count + 1];
    for start in 0..syllable_count {
        let maximum_end = syllable_count.min(start + MAX_LEXICON_SYLLABLES);
        for end in start + 1..=maximum_end {
            edges[start][end] =
                core.exact_full_code_texts(&code[start * 2..end * 2], MAX_CANDIDATE_SNAPSHOT_RANK)?;
        }
    }
    Ok(edges)
}

fn core_complete_boundary_paths(
    edges: &[Vec<Vec<String>>],
    syllable_count: usize,
) -> (Vec<Vec<CoreCompletePath>>, Vec<Vec<CoreCompletePath>>) {
    let mut prefixes = vec![Vec::new(); syllable_count + 1];
    prefixes[0].push(CoreCompletePath::empty());
    for start in 0..syllable_count {
        let starting_paths = prefixes[start].clone();
        if starting_paths.is_empty() {
            continue;
        }
        let maximum_end = syllable_count.min(start + MAX_LEXICON_SYLLABLES);
        for end in start + 1..=maximum_end {
            for (edge_rank, edge_text) in edges[start][end]
                .iter()
                .take(SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH)
                .enumerate()
            {
                for prefix in &starting_paths {
                    let mut path = prefix.clone();
                    path.text.push_str(edge_text);
                    path.segment_lengths.push(end - start);
                    path.edge_ranks.push(edge_rank);
                    path.segments.push(CoreCompleteSegment {
                        text: edge_text.clone(),
                        syllable_count: end - start,
                        local_rank: edge_rank + 1,
                    });
                    retain_core_boundary_path(&mut prefixes[end], path);
                }
            }
        }
    }

    let mut suffixes = vec![Vec::new(); syllable_count + 1];
    suffixes[syllable_count].push(CoreCompletePath::empty());
    for start in (0..syllable_count).rev() {
        let maximum_end = syllable_count.min(start + MAX_LEXICON_SYLLABLES);
        for end in start + 1..=maximum_end {
            let ending_paths = suffixes[end].clone();
            if ending_paths.is_empty() {
                continue;
            }
            for (edge_rank, edge_text) in edges[start][end]
                .iter()
                .take(SUPPLEMENTAL_COMPOSITION_CORE_EDGE_DEPTH)
                .enumerate()
            {
                for suffix in &ending_paths {
                    let mut segment_lengths = vec![end - start];
                    segment_lengths.extend_from_slice(&suffix.segment_lengths);
                    let mut edge_ranks = vec![edge_rank];
                    edge_ranks.extend_from_slice(&suffix.edge_ranks);
                    let mut segments = vec![CoreCompleteSegment {
                        text: edge_text.clone(),
                        syllable_count: end - start,
                        local_rank: edge_rank + 1,
                    }];
                    segments.extend_from_slice(&suffix.segments);
                    retain_core_boundary_path(
                        &mut suffixes[start],
                        CoreCompletePath {
                            text: format!("{edge_text}{}", suffix.text),
                            segment_lengths,
                            edge_ranks,
                            segments,
                        },
                    );
                }
            }
        }
    }
    (prefixes, suffixes)
}

fn retain_core_boundary_path(paths: &mut Vec<CoreCompletePath>, candidate: CoreCompletePath) {
    paths.push(candidate);
    paths.sort_by(core_complete_path_order);
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.text.clone()));
    paths.truncate(SUPPLEMENTAL_COMPOSITION_PATHS_PER_BOUNDARY);
}

fn core_complete_path_order(
    left: &CoreCompletePath,
    right: &CoreCompletePath,
) -> std::cmp::Ordering {
    left.segment_lengths
        .len()
        .cmp(&right.segment_lengths.len())
        .then_with(|| right.segment_lengths.cmp(&left.segment_lengths))
        .then_with(|| left.edge_ranks.cmp(&right.edge_ranks))
        .then_with(|| left.text.cmp(&right.text))
}

/// Errors from decoding or configuring a layered candidate query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayeredCandidateTextsError {
    /// The observed code was not lowercase ASCII.
    KeySequence(KeySequenceError),
    /// The explicit supplemental influence bound was invalid.
    Config(SupplementalCandidateLayerError),
}

impl fmt::Display for LayeredCandidateTextsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeySequence(error) => error.fmt(formatter),
            Self::Config(_) => write!(formatter, "补充候选层的影响上限无效"),
        }
    }
}

impl Error for LayeredCandidateTextsError {}

impl From<KeySequenceError> for LayeredCandidateTextsError {
    fn from(error: KeySequenceError) -> Self {
        Self::KeySequence(error)
    }
}

impl From<SupplementalCandidateLayerError> for LayeredCandidateTextsError {
    fn from(error: SupplementalCandidateLayerError) -> Self {
        Self::Config(error)
    }
}

impl Decoder {
    /// Applies the same bounded candidate ordering used by interactive
    /// snapshots without performing snapshot I/O or changing host behavior.
    pub(crate) fn interactive_candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        self.interactive_candidate_texts_with_sources(code, limit)
            .map(|candidates| {
                candidates
                    .into_iter()
                    .map(|candidate| candidate.text)
                    .collect()
            })
    }

    fn interactive_candidate_texts_with_sources(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<InteractiveCandidateText>, KeySequenceError> {
        self.interactive_candidate_query(code, limit)
            .map(|query| query.candidates)
    }

    fn interactive_candidate_query(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<InteractiveCandidateQuery, KeySequenceError> {
        let exact = self.decode_exact_full_code(code, limit)?;
        let original_has_exact_full_code = !exact.is_empty();
        let sentence_limit = limit
            .saturating_add(exact.len())
            .min(MAX_CANDIDATE_SNAPSHOT_RANK);
        let candidates = self.decode_sentence(code, sentence_limit)?;
        let original_first_candidate_is_complete =
            candidates.first().is_some_and(sentence_is_complete);
        let complete_candidates = self.decode_complete_sentence(code, sentence_limit)?;
        let mut visible = Vec::with_capacity(limit);
        let mut seen = HashSet::new();
        for candidate in exact {
            if seen.insert(candidate.text.clone()) {
                visible.push(InteractiveCandidateText {
                    text: candidate.text,
                    source: InteractiveCandidateSource::CoreExact,
                });
            }
        }

        // Exactly two complete syllables may also be an ad-hoc character
        // combination that no word dictionary should be expected to contain.
        // Keep this lane deliberately narrow so longer sentence composition
        // remains governed by the ordinary decoder.
        for candidate in self.full_code_character_pair_texts(code)? {
            if visible.len() == limit {
                break;
            }
            if seen.insert(candidate.clone()) {
                visible.push(InteractiveCandidateText {
                    text: candidate,
                    source: InteractiveCandidateSource::CharacterPair,
                });
            }
        }

        // A complete two-key-per-syllable sentence is stronger interaction
        // evidence than a freely abbreviated path. Keep the research decoder's
        // ordinary order intact underneath this small host-facing lane.
        for candidate in &complete_candidates {
            if visible.len() == limit {
                break;
            }
            if seen.insert(candidate.text.clone()) {
                visible.push(InteractiveCandidateText {
                    text: candidate.text.clone(),
                    source: InteractiveCandidateSource::CompleteSentence,
                });
            }
        }
        for (index, candidate) in candidates.into_iter().enumerate() {
            if visible.len() == limit {
                break;
            }
            if candidate.unresolved_key_count == 0 {
                if seen.insert(candidate.text.clone()) {
                    visible.push(InteractiveCandidateText {
                        text: candidate.text,
                        source: InteractiveCandidateSource::Decoder,
                    });
                }
            } else if index == 0 && visible.is_empty() {
                visible.push(InteractiveCandidateText {
                    text: code.to_owned(),
                    source: InteractiveCandidateSource::Decoder,
                });
                break;
            } else {
                break;
            }
        }
        Ok(InteractiveCandidateQuery {
            candidates: visible,
            automatic_transposition_blocked: original_has_exact_full_code
                || original_first_candidate_is_complete,
        })
    }

    fn full_code_character_pair_texts(&self, code: &str) -> Result<Vec<String>, KeySequenceError> {
        let observed = KeySequence::new(code)?;
        let code = observed.as_str();
        if code.len() != 4 {
            return Ok(Vec::new());
        }

        let left = self
            .decode_exact_full_code(&code[..2], FULL_CODE_CHARACTER_PAIR_DEPTH)?
            .into_iter()
            .filter(|candidate| candidate.text.chars().count() == 1)
            .collect::<Vec<_>>();
        let right = self
            .decode_exact_full_code(&code[2..], FULL_CODE_CHARACTER_PAIR_DEPTH)?
            .into_iter()
            .filter(|candidate| candidate.text.chars().count() == 1)
            .collect::<Vec<_>>();

        let mut combinations = Vec::with_capacity(left.len().saturating_mul(right.len()));
        for (left_rank, left_candidate) in left.iter().enumerate() {
            for (right_rank, right_candidate) in right.iter().enumerate() {
                combinations.push((
                    format!("{}{}", left_candidate.text, right_candidate.text),
                    left_candidate.score.total + right_candidate.score.total,
                    left_rank,
                    right_rank,
                ));
            }
        }
        combinations.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut visible = Vec::with_capacity(MAX_FULL_CODE_CHARACTER_PAIRS);
        let mut seen = HashSet::new();
        for (text, _, _, _) in combinations {
            if seen.insert(text.clone()) {
                visible.push(text);
                if visible.len() == MAX_FULL_CODE_CHARACTER_PAIRS {
                    break;
                }
            }
        }
        Ok(visible)
    }

    fn automatic_transposition_decision(
        &self,
        code: &str,
    ) -> Result<AutomaticTranspositionDecision, KeySequenceError> {
        let observed = KeySequence::new(code)?;
        let code = observed.as_str();
        if code.len() < 2
            || code.len() > MAX_TRANSPOSITION_RECOVERY_KEYS
            || !code.len().is_multiple_of(2)
        {
            return Ok(AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::UnsupportedInputShape,
            ));
        }

        if !self.decode_exact_full_code(code, 1)?.is_empty() {
            return Ok(AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::OriginalHasExactFullCode,
            ));
        }

        if self
            .decode_sentence(code, 1)?
            .first()
            .is_some_and(sentence_is_complete)
        {
            return Ok(AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::OriginalFirstCandidateIsComplete,
            ));
        }

        let original = code.as_bytes();
        let mut recovery = None;
        for syllable_index in 0..original.len() / 2 {
            let swap_start = syllable_index * 2;
            if original[swap_start] == original[swap_start + 1] {
                continue;
            }
            let mut swapped = original.to_vec();
            swapped.swap(swap_start, swap_start + 1);
            let intended_code = std::str::from_utf8(&swapped)
                .expect("a validated lowercase ASCII key sequence remains UTF-8 after swapping");
            let exact = self
                .decode_exact_full_code(intended_code, AUTOMATIC_TRANSPOSITION_CANDIDATE_DEPTH)?;
            if exact.is_empty() {
                continue;
            }
            if recovery.is_some() {
                return Ok(AutomaticTranspositionDecision::KeepPrimary(
                    AutomaticTranspositionKeepReason::AmbiguousSwapLocations,
                ));
            }
            let mut seen = HashSet::new();
            let candidates = exact
                .into_iter()
                .map(|candidate| candidate.text)
                .filter(|text| seen.insert(text.clone()))
                .collect();
            recovery = Some(AutomaticTranspositionPromotion {
                syllable_index,
                intended_code: intended_code.to_owned(),
                candidates,
            });
        }

        Ok(recovery.map_or_else(
            || {
                AutomaticTranspositionDecision::KeepPrimary(
                    AutomaticTranspositionKeepReason::NoExactFullCodeRecovery,
                )
            },
            AutomaticTranspositionDecision::PromoteExactFullCode,
        ))
    }

    fn automatic_transposition_recovery_after_primary(
        &self,
        code: &str,
        requested_syllable_index: usize,
        limit: usize,
    ) -> Result<Option<AutomaticTranspositionPromotion>, KeySequenceError> {
        self.automatic_transposition_span_recovery_after_primary(
            code,
            requested_syllable_index,
            1,
            limit,
        )
    }

    fn automatic_transposition_span_recovery_after_primary(
        &self,
        code: &str,
        first_syllable_index: usize,
        syllable_count: usize,
        limit: usize,
    ) -> Result<Option<AutomaticTranspositionPromotion>, KeySequenceError> {
        let observed = KeySequence::new(code)?;
        let code = observed.as_str();
        if limit == 0
            || code.len() < 2
            || code.len() > MAX_TRANSPOSITION_RECOVERY_KEYS
            || !code.len().is_multiple_of(2)
            || !(1..=2).contains(&syllable_count)
            || first_syllable_index
                .checked_add(syllable_count)
                .is_none_or(|end| end > code.len() / 2)
            || (syllable_count == 2 && (code.len() != 4 || first_syllable_index != 0))
        {
            return Ok(None);
        }

        let original = code.as_bytes();
        for syllable_index in first_syllable_index..first_syllable_index + syllable_count {
            let swap_start = syllable_index * 2;
            if original[swap_start] == original[swap_start + 1] {
                return Ok(None);
            }
            if syllable_count == 2 {
                let mut singly_swapped = original.to_vec();
                singly_swapped.swap(swap_start, swap_start + 1);
                let singly_swapped_code = std::str::from_utf8(&singly_swapped).expect(
                    "a validated lowercase ASCII key sequence remains UTF-8 after swapping",
                );
                if !self
                    .decode_exact_full_code(singly_swapped_code, 1)?
                    .is_empty()
                {
                    return Ok(None);
                }
            }
        }
        let mut swapped = original.to_vec();
        for syllable_index in first_syllable_index..first_syllable_index + syllable_count {
            let swap_start = syllable_index * 2;
            swapped.swap(swap_start, swap_start + 1);
        }
        let intended_code = std::str::from_utf8(&swapped)
            .expect("a validated lowercase ASCII key sequence remains UTF-8 after swapping");
        let exact = self.decode_exact_full_code(
            intended_code,
            limit.min(AUTOMATIC_TRANSPOSITION_CANDIDATE_DEPTH),
        )?;
        if exact.is_empty() {
            return Ok(None);
        }
        let mut seen = HashSet::new();
        Ok(Some(AutomaticTranspositionPromotion {
            syllable_index: first_syllable_index,
            intended_code: intended_code.to_owned(),
            candidates: exact
                .into_iter()
                .map(|candidate| candidate.text)
                .filter(|text| seen.insert(text.clone()))
                .collect(),
        }))
    }
}

fn sentence_is_complete(candidate: &crate::SentenceCandidate) -> bool {
    candidate.unresolved_key_count == 0
        && !candidate.used_error
        && candidate
            .segments
            .iter()
            .all(|segment| segment.candidate.spelling.abbreviated_syllables.is_empty())
}

/// Errors raised before an interactive host can use a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateSnapshotError {
    /// The schema is not the exact supported value.
    UnsupportedSchema,
    /// The revision is empty, too long, or contains unsupported characters.
    InvalidRevision,
    /// The actual or declared payload exceeds the fixed memory boundary.
    PayloadTooLarge {
        /// Larger of the actual and declared byte lengths.
        actual: usize,
        /// Accepted maximum.
        maximum: usize,
    },
    /// The declared and actual payload sizes differ.
    PayloadLengthMismatch {
        /// Declared byte length.
        expected: usize,
        /// Actual byte length.
        actual: usize,
    },
    /// The stable payload fingerprint differs.
    PayloadFingerprintMismatch {
        /// Declared fingerprint.
        expected: u64,
        /// Actual fingerprint.
        actual: u64,
    },
    /// The declared entry count is outside the fixed boundary.
    InvalidExpectedEntryCount {
        /// Declared entry count.
        count: usize,
        /// Accepted maximum.
        maximum: usize,
    },
    /// The lexicon payload is structurally invalid.
    Lexicon,
    /// The parsed entry count differs from the descriptor.
    EntryCountMismatch {
        /// Declared count.
        expected: usize,
        /// Parsed count.
        actual: usize,
    },
}

impl fmt::Display for CandidateSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema => write!(formatter, "不支持的候选快照格式"),
            Self::InvalidRevision => write!(formatter, "候选快照版本标识无效"),
            Self::PayloadTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "候选快照载荷过大：{actual} 字节，上限 {maximum} 字节"
                )
            }
            Self::PayloadLengthMismatch { expected, actual } => write!(
                formatter,
                "候选快照载荷长度不符：标注 {expected} 字节，实际 {actual} 字节"
            ),
            Self::PayloadFingerprintMismatch { .. } => {
                write!(formatter, "候选快照载荷指纹不符")
            }
            Self::InvalidExpectedEntryCount { count, maximum } => {
                write!(formatter, "候选快照词条数无效：{count}，上限 {maximum}")
            }
            Self::Lexicon => write!(formatter, "候选快照词典结构无效"),
            Self::EntryCountMismatch { expected, actual } => write!(
                formatter,
                "候选快照词条数不符：标注 {expected}，实际 {actual}"
            ),
        }
    }
}

impl Error for CandidateSnapshotError {}

/// Stable FNV-1a payload fingerprint used only for corruption detection.
pub const fn candidate_payload_fingerprint(bytes: &[u8]) -> u64 {
    let mut fingerprint = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        fingerprint ^= bytes[index] as u64;
        fingerprint = fingerprint.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    fingerprint
}

pub(crate) fn valid_candidate_snapshot_revision(revision: &str) -> bool {
    !revision.is_empty()
        && revision.len() <= MAX_CANDIDATE_SNAPSHOT_REVISION_BYTES
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::hint::black_box;
    use std::time::Instant;

    use super::*;

    const DEMO_LEXICON: &str = include_str!("../tests/fixtures/public/demo_lexicon.tsv");
    const DEMO_DESCRIPTOR: CandidateSnapshotDescriptor<'static> = CandidateSnapshotDescriptor {
        schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
        revision: "public-demo-v1",
        contains_private_text: false,
        lexicon_tsv: DEMO_LEXICON,
        expected_payload_bytes: 1_132,
        expected_payload_fingerprint: 0x592a_4dbb_4b33_efa6,
        expected_entry_count: 50,
    };

    fn load_test_snapshot(
        revision: &str,
        lexicon_tsv: &str,
        expected_entry_count: usize,
    ) -> CandidateSnapshot {
        CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision,
            contains_private_text: false,
            lexicon_tsv,
            expected_payload_bytes: lexicon_tsv.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(lexicon_tsv.as_bytes()),
            expected_entry_count,
        })
        .unwrap()
    }

    #[test]
    fn validated_snapshot_exposes_metadata_and_bounded_candidates() {
        let snapshot = CandidateSnapshot::load(DEMO_DESCRIPTOR).unwrap();
        assert_eq!(snapshot.revision(), "public-demo-v1");
        assert!(!snapshot.contains_private_text());
        assert_eq!(snapshot.payload_bytes(), 1_132);
        assert_eq!(snapshot.payload_fingerprint(), 0x592a_4dbb_4b33_efa6);
        assert_eq!(snapshot.entry_count(), 50);
        assert_eq!(
            snapshot.candidate_text("nihk", 1).unwrap().as_deref(),
            Some("你好")
        );
        assert_eq!(snapshot.candidate_text("nihk", 0).unwrap(), None);
        assert_eq!(snapshot.candidate_text("nihk", 51).unwrap(), None);
        assert_eq!(
            snapshot.candidate_text("zzzzzzzz", 1).unwrap().as_deref(),
            Some("zzzzzzzz")
        );
        let page = snapshot.candidate_texts("nihk", 5).unwrap();
        assert_eq!(page.first().map(String::as_str), Some("你好"));
        assert!(page.len() <= 5);
        assert!(snapshot.candidate_texts("nihk", 0).unwrap().is_empty());
        assert_eq!(
            snapshot.candidate_texts("zzzzzzzz", 5).unwrap(),
            ["zzzzzzzz"]
        );
    }

    #[test]
    fn interactive_snapshot_exposes_at_most_fifty_ranked_candidates() {
        let mut lexicon = String::from("text\tpinyin\tfrequency\n");
        for index in 0..60 {
            use std::fmt::Write as _;
            writeln!(lexicon, "候选{index}\tqin\t{}", 1_000 - index).unwrap();
        }
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "fifty-candidate-test-v1",
            contains_private_text: false,
            lexicon_tsv: &lexicon,
            expected_payload_bytes: lexicon.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
            expected_entry_count: 60,
        })
        .unwrap();

        let candidates = snapshot.candidate_texts("qn", usize::MAX).unwrap();
        assert_eq!(candidates.len(), MAX_CANDIDATE_SNAPSHOT_RANK);
        assert!(
            snapshot
                .candidate_text("qn", MAX_CANDIDATE_SNAPSHOT_RANK)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            snapshot
                .candidate_text("qn", MAX_CANDIDATE_SNAPSHOT_RANK + 1)
                .unwrap(),
            None
        );
    }

    #[test]
    fn interactive_two_key_full_syllable_precedes_initial_abbreviations() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
参加\tcan jia\t100000\n\
惨\tcan\t100\n\
残\tcan\t90\n\
测试\tce shi\t80000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "short-full-code-priority-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 4,
        })
        .unwrap();

        assert_eq!(
            snapshot.candidate_texts("cj", 3).unwrap(),
            ["惨", "残", "参加"]
        );
        assert_eq!(
            snapshot.candidate_text("cj", 1).unwrap().as_deref(),
            Some("惨")
        );
        assert_eq!(
            snapshot.candidate_texts("ceui", 1).unwrap(),
            ["测试"],
            "longer input keeps the ordinary sentence ranking"
        );
    }

    #[test]
    fn exact_full_code_survives_more_than_fifty_abbreviation_paths() {
        let mut lexicon = String::from("text\tpinyin\tfrequency\n句\tju\t1\n");
        for index in 0..60 {
            use std::fmt::Write as _;
            writeln!(lexicon, "即时词{index}\tji shi\t{}", 100_000 - index).unwrap();
        }
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "exact-full-code-frontier-v1",
            contains_private_text: false,
            lexicon_tsv: &lexicon,
            expected_payload_bytes: lexicon.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
            expected_entry_count: 61,
        })
        .unwrap();

        assert_eq!(snapshot.candidate_texts("ju", 1).unwrap(), ["句"]);
    }

    #[test]
    fn exact_multi_syllable_word_precedes_a_longer_free_abbreviation() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t10\n\
属于是\tshu yu shi\t100000\n\
属于说\tshu yu shuo\t90000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "exact-multi-syllable-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 3,
        })
        .unwrap();

        assert_eq!(
            snapshot.candidate_texts("uuyu", 3).unwrap(),
            ["属于", "属于是", "属于说"]
        );
    }

    #[test]
    fn pinned_public_interactive_lane_keeps_an_arbitrary_full_code_character_pair_visible() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        let visible = decoder.interactive_candidate_texts("vids", 6).unwrap();
        assert_eq!(visible.first().map(String::as_str), Some("制动"));
        assert!(
            visible.iter().any(|candidate| candidate == "只动"),
            "zero-error full-code character composition should stay visible without becoming a lexicon word: {visible:?}"
        );
    }

    #[test]
    fn pinned_simplified_lane_does_not_compose_shadowed_traditional_single_characters() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        let candidates = decoder.interactive_candidate_texts("biruuo", 10).unwrap();
        assert_eq!(candidates.first().map(String::as_str), Some("比如说"));
        assert!(
            !candidates.iter().any(|candidate| candidate == "比如說"),
            "a shadowed traditional single character must not be composed into the simplified lane: {candidates:?}"
        );
    }

    #[test]
    fn pinned_public_pair_frontier_keeps_reported_exact_character_compositions_reachable() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        for (code, expected) in [("qthp", "雀魂"), ("jmpn", "简拼"), ("ujzi", "删字")] {
            let query = decoder
                .interactive_candidate_query(code, MAX_CANDIDATE_SNAPSHOT_RANK)
                .unwrap();
            let index = query
                .candidates
                .iter()
                .position(|candidate| candidate.text == expected)
                .unwrap_or_else(|| panic!("{expected} should remain reachable for {code}"));
            assert!(matches!(
                query.candidates[index].source,
                InteractiveCandidateSource::CoreExact | InteractiveCandidateSource::CharacterPair
            ));
            assert!(
                query.candidates[..index]
                    .iter()
                    .all(|candidate| { candidate.source != InteractiveCandidateSource::Decoder })
            );
        }
    }

    #[test]
    fn character_pair_lane_is_bounded_deterministic_and_deduplicated() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
制\tzhi\t1000\n\
只\tzhi\t900\n\
动\tdong\t800\n\
懂\tdong\t700\n\
制动\tzhi dong\t600\n";
        let decoder = Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());

        let first = decoder.full_code_character_pair_texts("vids").unwrap();
        let second = decoder.full_code_character_pair_texts("vids").unwrap();
        assert_eq!(first, second);
        assert_eq!(first, ["制动", "只动", "制懂", "只懂"]);
        assert_eq!(first.iter().collect::<HashSet<_>>().len(), first.len());
        assert!(first.len() <= MAX_FULL_CODE_CHARACTER_PAIRS);

        let visible = decoder.interactive_candidate_texts("vids", 4).unwrap();
        assert_eq!(visible, ["制动", "只动", "制懂", "只懂"]);
    }

    #[test]
    fn character_pair_lane_does_not_expand_to_longer_input() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
只\tzhi\t1000\n\
动\tdong\t900\n";
        let decoder = Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());

        assert!(
            decoder
                .full_code_character_pair_texts("vidsvi")
                .unwrap()
                .is_empty()
        );
        assert!(
            decoder
                .full_code_character_pair_texts("vid")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn supplemental_exact_word_suppresses_the_core_free_abbreviation_tail() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
属于是\tshu yu shi\t100000\n\
属于说\tshu yu shuo\t90000\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t100\n";
        let core = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-core-abbreviation-v1",
            contains_private_text: false,
            lexicon_tsv: CORE,
            expected_payload_bytes: CORE.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(CORE.as_bytes()),
            expected_entry_count: 2,
        })
        .unwrap();
        let supplemental = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-supplemental-exact-v1",
            contains_private_text: false,
            lexicon_tsv: SUPPLEMENTAL,
            expected_payload_bytes: SUPPLEMENTAL.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(SUPPLEMENTAL.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();

        assert_eq!(core.candidate_texts("uuyu", 2).unwrap()[0], "属于是");
        assert_eq!(
            layered_candidate_texts(
                &core,
                &supplemental,
                "uuyu",
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap(),
            ["属于"]
        );
    }

    #[test]
    fn supplemental_exact_word_keeps_a_distinct_complete_core_sentence_visible() {
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
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let core = load("complete-core-survival-v1", CORE, 6);
        let supplemental = load("complete-supplement-survival-v1", SUPPLEMENTAL, 3);

        let visible = layered_candidate_texts(
            &core,
            &supplemental,
            "daigle",
            6,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )
        .unwrap();

        assert_eq!(visible.first().map(String::as_str), Some("达成了"));
        assert_eq!(visible.get(1).map(String::as_str), Some("打成了"));
    }

    #[test]
    fn supplemental_exact_word_keeps_the_bounded_character_pair_lane() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
制\tzhi\t1000\n\
只\tzhi\t900\n\
动\tdong\t800\n\
懂\tdong\t700\n\
制动\tzhi dong\t600\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
制动\tzhi dong\t1000\n\
只懂\tzhi dong\t900\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let core = load("pair-layer-core-v1", CORE, 5);
        let supplemental = load("pair-layer-supplement-v1", SUPPLEMENTAL, 2);

        let visible = layered_candidate_texts(
            &core,
            &supplemental,
            "vids",
            6,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )
        .unwrap();
        assert_eq!(visible.first().map(String::as_str), Some("制动"));
        assert_eq!(visible.get(1).map(String::as_str), Some("只懂"));
        assert!(visible.iter().any(|candidate| candidate == "只动"));
        assert_eq!(visible.iter().collect::<HashSet<_>>().len(), visible.len());
    }

    #[test]
    fn one_supplemental_multi_syllable_word_can_complete_a_core_prefix() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
掰开\tbai kai\t1000\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
揉碎\trou sui\t1000\n";
        let load = |revision: &str, lexicon: &str| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count: 1,
            })
            .unwrap()
        };
        let core = load("mixed-layer-prefix-core-v1", CORE);
        let supplemental = load("mixed-layer-prefix-supplement-v1", SUPPLEMENTAL);

        let first = layered_candidate_query_with_sources(
            &core,
            &supplemental,
            "blklrbsv",
            6,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )
        .unwrap();
        let second = layered_candidate_query_with_sources(
            &core,
            &supplemental,
            "blklrbsv",
            6,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )
        .unwrap();
        assert_eq!(first, second, "the mixed-layer lane must be deterministic");
        assert_eq!(
            first.candidates.first(),
            Some(&InteractiveCandidateText {
                text: "掰开揉碎".to_owned(),
                source: InteractiveCandidateSource::SupplementalExact,
            })
        );
        assert!(first.candidates.len() <= 6);
        assert!(first.automatic_transposition_blocked);
    }

    #[test]
    fn one_supplemental_multi_syllable_word_can_appear_between_core_paths() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
这\tzhe\t1000\n\
一种\tyi zhong\t900\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t1000\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let core = load("mixed-layer-middle-core-v1", CORE, 2);
        let supplemental = load("mixed-layer-middle-supplement-v1", SUPPLEMENTAL, 1);

        assert_eq!(
            layered_candidate_texts(
                &core,
                &supplemental,
                "veuuyuyivs",
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap()
            .first()
            .map(String::as_str),
            Some("这属于一种")
        );
    }

    #[test]
    fn supplemental_composition_preserves_a_non_literal_core_primary_top() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
正常\tzheng chang\t1000\n\
处理\tchu li\t900\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
整场\tzheng chang\t1000\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let core = load("mixed-layer-core-top-v1", CORE, 2);
        let supplemental = load("mixed-layer-core-top-supplement-v1", SUPPLEMENTAL, 1);
        let code = "vgihiuli";
        let core_top = core.candidate_texts(code, 1).unwrap();
        assert_eq!(core_top, ["正常处理"]);

        let layered = layered_candidate_texts(
            &core,
            &supplemental,
            code,
            3,
            SupplementalCandidateLayerConfig {
                exact_promotions: 1,
            },
        )
        .unwrap();
        assert_eq!(layered.first().map(String::as_str), Some("正常处理"));
        assert_eq!(layered.get(1).map(String::as_str), Some("整场处理"));

        let structural =
            supplemental_complete_composition_texts(&core, &supplemental, code, 8).unwrap();
        assert_eq!(
            structural,
            supplemental_complete_composition_texts_with_order(
                &core,
                &supplemental,
                code,
                8,
                SupplementalCompositionOrder::StructuralV1,
            )
            .unwrap(),
            "the explicit audit order must reproduce the runtime order"
        );
        let explained = supplemental_complete_compositions_with_order(
            &core,
            &supplemental,
            code,
            8,
            SupplementalCompositionOrder::StructuralV1,
        )
        .unwrap();
        assert_eq!(
            explained
                .iter()
                .map(SupplementalCompositionCandidate::text)
                .collect::<Vec<_>>(),
            structural.iter().map(String::as_str).collect::<Vec<_>>(),
            "retaining boundaries must not change deduplication or order"
        );
        let mixed = explained
            .iter()
            .find(|candidate| candidate.text() == "整场处理")
            .unwrap();
        assert_eq!(mixed.segments().len(), 2);
        assert_eq!(mixed.segments()[0].text(), "整场");
        assert_eq!(
            mixed.segments()[0].source(),
            SupplementalCompositionSegmentSource::Supplemental
        );
        assert_eq!(mixed.segments()[0].syllable_count(), 2);
        assert_eq!(mixed.segments()[0].local_rank(), 1);
        assert_eq!(mixed.segments()[1].text(), "处理");
        assert_eq!(
            mixed.segments()[1].source(),
            SupplementalCompositionSegmentSource::Core
        );
        assert_eq!(mixed.segments()[1].syllable_count(), 2);
        assert_eq!(mixed.segments()[1].local_rank(), 1);
        for order in [
            SupplementalCompositionOrder::StructuralV1,
            SupplementalCompositionOrder::FewerSegmentsFirst,
            SupplementalCompositionOrder::LocalRanksFirst,
        ] {
            let first = supplemental_complete_composition_texts_with_order(
                &core,
                &supplemental,
                code,
                8,
                order,
            )
            .unwrap();
            let second = supplemental_complete_composition_texts_with_order(
                &core,
                &supplemental,
                code,
                8,
                order,
            )
            .unwrap();
            assert_eq!(first, second);
            assert!(first.len() <= 8);
        }
    }

    #[test]
    fn supplemental_composition_rejects_single_characters_and_two_supplemental_words() {
        const SINGLE_CORE: &str = "text\tpinyin\tfrequency\n\
掰开\tbai kai\t1000\n\
碎\tsui\t900\n";
        const SINGLE_SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
揉\trou\t1000\n";
        const DOUBLE_CORE: &str = "text\tpinyin\tfrequency\n\
和\the\t1000\n";
        const DOUBLE_SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
掰开\tbai kai\t1000\n\
揉碎\trou sui\t900\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let config = SupplementalCandidateLayerConfig {
            exact_promotions: 1,
        };
        let single_core = load("mixed-layer-single-core-v1", SINGLE_CORE, 2);
        let single_supplemental = load("mixed-layer-single-supplement-v1", SINGLE_SUPPLEMENTAL, 1);
        assert!(
            !layered_candidate_texts(&single_core, &single_supplemental, "blklrbsv", 6, config,)
                .unwrap()
                .iter()
                .any(|candidate| candidate == "掰开揉碎")
        );

        let double_core = load("mixed-layer-double-core-v1", DOUBLE_CORE, 1);
        let double_supplemental = load("mixed-layer-double-supplement-v1", DOUBLE_SUPPLEMENTAL, 2);
        assert!(
            !layered_candidate_texts(&double_core, &double_supplemental, "blklherbsv", 6, config,)
                .unwrap()
                .iter()
                .any(|candidate| candidate == "掰开和揉碎")
        );

        let oversized = "bl".repeat(MAX_SUPPLEMENTAL_COMPOSITION_SYLLABLES + 1);
        assert!(
            supplemental_complete_composition_texts(
                &double_core,
                &double_supplemental,
                &oversized,
                1,
            )
            .unwrap()
            .is_empty(),
            "the extra mixed-layer search must stay inside its fixed syllable bound"
        );
    }

    #[test]
    fn supplemental_composition_never_displaces_a_core_whole_word_top_one() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
掰开揉碎\tbai kai rou sui\t1000\n\
掰开\tbai kai\t900\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
揉碎\trou sui\t1000\n";
        let load = |revision: &str, lexicon: &str, expected_entry_count| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count,
            })
            .unwrap()
        };
        let core = load("mixed-layer-whole-core-v1", CORE, 2);
        let supplemental = load("mixed-layer-whole-supplement-v1", SUPPLEMENTAL, 1);

        assert_eq!(
            layered_candidate_texts(
                &core,
                &supplemental,
                "blklrbsv",
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap()
            .first()
            .map(String::as_str),
            Some("掰开揉碎")
        );
    }

    #[test]
    fn a_duplicate_only_supplement_preserves_the_core_primary_lane() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t10\n\
属于是\tshu yu shi\t100000\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
属于\tshu yu\t100\n";
        let core = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-core-duplicate-v1",
            contains_private_text: false,
            lexicon_tsv: CORE,
            expected_payload_bytes: CORE.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(CORE.as_bytes()),
            expected_entry_count: 2,
        })
        .unwrap();
        let supplemental = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-supplemental-duplicate-v1",
            contains_private_text: false,
            lexicon_tsv: SUPPLEMENTAL,
            expected_payload_bytes: SUPPLEMENTAL.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(SUPPLEMENTAL.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();

        let core_primary = core.candidate_texts("uuyu", 3).unwrap();
        assert_eq!(
            layered_candidate_texts(
                &core,
                &supplemental,
                "uuyu",
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap(),
            core_primary
        );
    }

    #[test]
    fn supplemental_collision_never_displaces_core_exact_top_one() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
什么\tshen me\t100\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
甚么\tshen me\t100000\n\
什么\tshen me\t90000\n";
        let core = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-core-top-v1",
            contains_private_text: false,
            lexicon_tsv: CORE,
            expected_payload_bytes: CORE.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(CORE.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();
        let supplemental = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-supplemental-collision-v1",
            contains_private_text: false,
            lexicon_tsv: SUPPLEMENTAL,
            expected_payload_bytes: SUPPLEMENTAL.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(SUPPLEMENTAL.as_bytes()),
            expected_entry_count: 2,
        })
        .unwrap();

        assert_eq!(
            layered_candidate_texts(
                &core,
                &supplemental,
                "ufme",
                3,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 2,
                },
            )
            .unwrap(),
            ["什么", "甚么"]
        );
    }

    #[test]
    fn shared_supplemental_top_calibrates_cold_exact_order() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
大国\tda guo\t1657\n\
打过\tda guo\t1390\n";
        const SUPPLEMENTAL: &str = "text\tpinyin\tfrequency\n\
打过\tda guo\t9480\n\
大国\tda guo\t8656\n";
        let load = |revision: &str, lexicon: &str| {
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
                revision,
                contains_private_text: false,
                lexicon_tsv: lexicon,
                expected_payload_bytes: lexicon.len(),
                expected_payload_fingerprint: candidate_payload_fingerprint(lexicon.as_bytes()),
                expected_entry_count: 2,
            })
            .unwrap()
        };
        let core = load("cold-consensus-core-v1", CORE);
        let supplemental = load("cold-consensus-supplement-v1", SUPPLEMENTAL);

        assert_eq!(
            layered_candidate_texts_with_consensus(
                &core,
                &supplemental,
                "dago",
                4,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap(),
            ["打过", "大国"]
        );
        assert_eq!(
            layered_candidate_texts_with_consensus(
                &core,
                &supplemental,
                "dago",
                1,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 1,
                },
            )
            .unwrap(),
            ["打过"]
        );
        assert_eq!(
            layered_candidate_texts_with_consensus(
                &core,
                &supplemental,
                "dago",
                2,
                SupplementalCandidateLayerConfig {
                    exact_promotions: 0,
                },
            )
            .unwrap(),
            ["大国", "打过"]
        );
    }

    #[test]
    fn supplemental_merge_is_deduplicated_bounded_and_rejects_invalid_config() {
        let core_exact = ["核心首选".to_owned(), "重复".to_owned()];
        let supplemental_exact = ["重复".to_owned(), "补一".to_owned(), "补二".to_owned()];
        let core_primary = [
            "核心首选".to_owned(),
            "核心句".to_owned(),
            "重复".to_owned(),
        ];
        let config = SupplementalCandidateLayerConfig {
            exact_promotions: 2,
        };

        assert_eq!(
            merge_candidate_text_layers(
                &core_exact,
                &supplemental_exact,
                &core_primary,
                4,
                config,
            )
            .unwrap(),
            ["核心首选", "补一", "补二", "重复"]
        );
        let many_supplemental = (0..60)
            .map(|index| format!("补充{index}"))
            .collect::<Vec<_>>();
        assert_eq!(
            merge_candidate_text_layers(
                &[],
                &many_supplemental,
                &[],
                usize::MAX,
                SupplementalCandidateLayerConfig {
                    exact_promotions: MAX_CANDIDATE_SNAPSHOT_RANK,
                },
            )
            .unwrap()
            .len(),
            MAX_CANDIDATE_SNAPSHOT_RANK,
            "the fixed snapshot rank bound applies"
        );
        assert_eq!(
            merge_candidate_text_layers(
                &core_exact,
                &supplemental_exact,
                &core_primary,
                6,
                SupplementalCandidateLayerConfig {
                    exact_promotions: MAX_CANDIDATE_SNAPSHOT_RANK + 1,
                },
            )
            .unwrap_err(),
            SupplementalCandidateLayerError::PromotionLimit
        );
    }

    #[test]
    fn pinned_public_dictionary_accepts_standard_u_spelling_for_umlaut_syllables() {
        let imported = crate::parse_simplified_rime_lexicon(include_str!(
            "../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"
        ))
        .unwrap();
        let decoder = Decoder::new(imported.entries);

        let ju = decoder.decode_exact_full_code("ju", 7).unwrap();
        assert!(ju.iter().any(|candidate| candidate.text == "句"));
        let uuyu = decoder.decode_exact_full_code("uuyu", 3).unwrap();
        assert_eq!(
            uuyu.first().map(|candidate| candidate.text.as_str()),
            Some("属于")
        );
    }

    #[test]
    fn interactive_sentence_accepts_standard_u_spelling_across_word_boundaries() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
短\tduan\t100\n\
句\tju\t100\n\
子\tzi\t100\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "umlaut-sentence-boundary-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 3,
        })
        .unwrap();

        assert_eq!(snapshot.candidate_texts("drjuzi", 1).unwrap(), ["短句子"]);
    }

    #[test]
    fn interactive_sentence_prefers_complete_pairs_to_free_initials() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
惨\tcan\t10\n\
家\tjia\t10\n\
参加今晚\tcan jia jin wan\t100000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "complete-sentence-priority-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 3,
        })
        .unwrap();

        assert_eq!(snapshot.candidate_texts("cjjw", 2).unwrap()[0], "惨家");
    }

    #[test]
    fn complete_sentence_frontier_survives_free_abbreviation_crowding() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
晚上\twan shang\t100000\n\
无\twu\t100\n\
误\twu\t90\n\
提交\tti jiao\t100\n";
        let decoder = Decoder::new(parse_lexicon_tsv(LEXICON).unwrap());

        let ordinary = decoder.decode_sentence("wutijc", 2).unwrap();
        assert!(
            ordinary.iter().all(|candidate| candidate.text != "误提交"),
            "the fixture must reproduce free-abbreviation crowding: {ordinary:?}"
        );

        let complete = decoder.decode_complete_sentence("wutijc", 2).unwrap();
        assert_eq!(
            complete
                .iter()
                .map(|candidate| candidate.text.as_str())
                .collect::<Vec<_>>(),
            ["无提交", "误提交"]
        );
        assert!(
            complete.iter().all(sentence_is_complete),
            "the protected frontier must contain complete exact paths only"
        );
        assert_eq!(
            decoder.interactive_candidate_texts("wutijc", 2).unwrap(),
            ["无提交", "误提交"]
        );
    }

    #[test]
    fn pinned_public_complete_frontier_keeps_mistaken_submission_reachable() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        let complete = decoder
            .decode_complete_sentence("wutijc", MAX_CANDIDATE_SNAPSHOT_RANK)
            .unwrap();
        assert!(
            complete.iter().any(|candidate| candidate.text == "误提交"),
            "a complete core composition must remain reachable: {complete:?}"
        );
        assert!(complete.iter().all(sentence_is_complete));

        let question = decoder
            .decode_complete_sentence("veuuyunayivs", MAX_CANDIDATE_SNAPSHOT_RANK)
            .unwrap();
        let question_rank = question
            .iter()
            .position(|candidate| candidate.text == "这属于哪一种")
            .map(|index| index + 1);
        assert_eq!(
            question_rank,
            Some(6),
            "complete core components must preserve the interrogative composition"
        );
        assert!(question.iter().all(sentence_is_complete));
    }

    #[test]
    fn explicit_recovery_exposes_a_full_code_adjacent_transposition() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
看看\tkan kan\t1000\n\
考\tkao\t900\n\
见\tjian\t800\n\
测试\tce shi\t700\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-transposition-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 4,
        })
        .unwrap();

        let primary = snapshot.candidate_texts("kkjjceui", 1).unwrap();
        assert_ne!(primary.first().map(String::as_str), Some("看看测试"));
        let recovery = snapshot
            .transposition_recovery_texts("kkjjceui", 1)
            .unwrap();
        assert_eq!(recovery.first().map(String::as_str), Some("看看测试"));
        assert!(
            snapshot
                .transposition_recovery_texts("kkjjceui", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn four_character_correction_view_bypasses_sentence_crowding_without_promoting() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
掰开揉碎\tbai kai rou sui\t1000\n\
掰\tbai\t900\n\
开\tkai\t800\n\
揉\trou\t700\n\
碎\tsui\t600\n\
三个汉\tsan ge han\t500\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "four-character-correction-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 6,
        })
        .unwrap();
        let intended = crate::encode_pinyin_phrase("bai kai rou sui")
            .unwrap()
            .full_code;
        let mut observed = intended.as_str().as_bytes().to_vec();
        observed.swap(0, 1);
        let observed = std::str::from_utf8(&observed).unwrap();

        let recovered = snapshot
            .four_character_correction_candidates(observed, 7)
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].text, "掰开揉碎");
        assert_eq!(recovered[0].pinyin, "bai kai rou sui");
        assert_eq!(recovered[0].intended_code, intended.as_str());
        assert!(matches!(
            recovered[0].correction,
            Correction::AdjacentTransposition { start: 0, .. }
        ));
        assert!(
            snapshot
                .four_character_correction_candidates(intended.as_str(), 7)
                .unwrap()
                .is_empty(),
            "an exact whole word is not correction evidence"
        );
        assert!(
            snapshot
                .four_character_correction_candidates(observed, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn four_character_correction_view_keeps_the_existing_one_edit_channel() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
掰开揉碎\tbai kai rou sui\t1000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "four-character-error-channel-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();
        let intended = crate::encode_pinyin_phrase("bai kai rou sui")
            .unwrap()
            .full_code;
        let intended = intended.as_str().as_bytes();

        let mut substitution = intended.to_vec();
        let (substitution_index, neighbor) = intended
            .iter()
            .enumerate()
            .find_map(|(index, &key)| {
                (b'a'..=b'z')
                    .find(|&actual| crate::are_qwerty_neighbors(key, actual))
                    .map(|actual| (index, actual))
            })
            .unwrap();
        substitution[substitution_index] = neighbor;
        let mut transposition = intended.to_vec();
        transposition.swap(0, 1);
        let mut missing = intended.to_vec();
        missing.remove(2);
        let mut extra = intended.to_vec();
        extra.insert(2, b'x');

        for (observed, expected) in [
            (substitution, "邻键替换"),
            (transposition, "顺序颠倒"),
            (missing, "遗漏"),
            (extra, "多按"),
        ] {
            let observed = std::str::from_utf8(&observed).unwrap();
            let recovered = snapshot
                .four_character_correction_candidates(observed, 1)
                .unwrap();
            assert_eq!(recovered.len(), 1, "{expected}: {observed}");
            assert_eq!(recovered[0].text, "掰开揉碎");
            assert!(
                recovered[0].correction.description().contains(expected),
                "{:?}",
                recovered[0].correction
            );
        }
    }

    #[test]
    fn layered_four_character_gate_requires_one_corrected_canonical_code() {
        const CORE: &str = "text\tpinyin\tfrequency\n\
掰开揉碎\tbai kai rou sui\t1000\n";
        let core = load_test_snapshot("four-character-gate-core-v1", CORE, 1);
        let intended = crate::encode_pinyin_phrase("bai kai rou sui")
            .unwrap()
            .full_code;
        let mut observed = intended.as_str().as_bytes().to_vec();
        observed.swap(0, 1);
        let observed = std::str::from_utf8(&observed).unwrap();

        let decision =
            layered_four_character_correction_decision(&core, None, observed, 1).unwrap();
        let FourCharacterCorrectionDecision::Offer(offer) = decision else {
            panic!("one corrected code should be offered: {decision:?}");
        };
        assert_eq!(offer.intended_code, intended.as_str());
        assert_eq!(offer.candidates.len(), 1);
        assert_eq!(offer.candidates[0].text, "掰开揉碎");

        assert_eq!(
            layered_four_character_correction_decision(&core, None, intended.as_str(), 1).unwrap(),
            FourCharacterCorrectionDecision::KeepOrdinary(
                FourCharacterCorrectionKeepReason::OriginalHasExactFullCode
            )
        );
        assert_eq!(
            layered_four_character_correction_decision(&core, None, "abcdef", 1).unwrap(),
            FourCharacterCorrectionDecision::KeepOrdinary(
                FourCharacterCorrectionKeepReason::UnsupportedInputShape
            )
        );
        assert_eq!(
            layered_four_character_correction_decision(&core, None, "aaaaaaaa", 1).unwrap(),
            FourCharacterCorrectionDecision::KeepOrdinary(
                FourCharacterCorrectionKeepReason::NoSingleEditRecovery
            )
        );
    }

    #[test]
    fn layered_four_character_gate_rejects_two_one_edit_target_codes() {
        const AMBIGUOUS: &str = "text\tpinyin\tfrequency\n\
甲乙丙跨\tjia yi bing kua\t1000\n\
甲乙丙宽\tjia yi bing kuan\t900\n";
        let snapshot = load_test_snapshot("four-character-gate-ambiguous-v1", AMBIGUOUS, 2);
        let first = crate::encode_pinyin_phrase("jia yi bing kua")
            .unwrap()
            .full_code;
        let second = crate::encode_pinyin_phrase("jia yi bing kuan")
            .unwrap()
            .full_code;
        assert_eq!(&first.as_str()[..6], &second.as_str()[..6]);
        assert_eq!(&first.as_str()[6..7], &second.as_str()[6..7]);
        assert_ne!(&first.as_str()[7..], &second.as_str()[7..]);
        let mut observed = first.as_str().as_bytes().to_vec();
        observed[7] = b'e';
        assert!(crate::are_qwerty_neighbors(
            first.as_str().as_bytes()[7],
            b'e'
        ));
        assert!(crate::are_qwerty_neighbors(
            second.as_str().as_bytes()[7],
            b'e'
        ));
        let observed = std::str::from_utf8(&observed).unwrap();

        assert_eq!(
            layered_four_character_correction_decision(&snapshot, None, observed, 7).unwrap(),
            FourCharacterCorrectionDecision::KeepOrdinary(
                FourCharacterCorrectionKeepReason::AmbiguousIntendedCodes
            )
        );
    }

    #[test]
    fn explicit_recovery_rescues_a_reversed_double_pinyin_pair() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
什么\tshen me\t295403\n\
神\tshen\t120000\n\
恶魔\te mo\t80000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "synthetic-reversed-pair-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 3,
        })
        .unwrap();

        let primary = snapshot.candidate_texts("ufem", 1).unwrap();
        assert!(
            !primary.iter().any(|candidate| candidate == "什么"),
            "recovered text unexpectedly remained in the primary lane: {primary:?}"
        );
        let recovery = snapshot.transposition_recovery_texts("ufem", 1).unwrap();
        assert_eq!(recovery.first().map(String::as_str), Some("什么"));
    }

    #[test]
    fn automatic_gate_promotes_one_unique_reversed_pair_without_touching_primary() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
什么\tshen me\t295403\n\
神\tshen\t120000\n\
恶魔\te mo\t80000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "automatic-reversed-pair-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 3,
        })
        .unwrap();

        let before = snapshot.candidate_texts("ufem", 3).unwrap();
        let decision = snapshot.automatic_transposition_decision("ufem").unwrap();
        assert_eq!(
            decision,
            AutomaticTranspositionDecision::PromoteExactFullCode(AutomaticTranspositionPromotion {
                syllable_index: 1,
                intended_code: "ufme".to_owned(),
                candidates: vec!["什么".to_owned()],
            })
        );
        assert!(
            !snapshot
                .interactive_candidate_query("ufem", 3)
                .unwrap()
                .automatic_transposition_blocked
        );
        assert_eq!(
            snapshot
                .automatic_transposition_recovery_after_primary("ufem", 1, 1)
                .unwrap(),
            Some(AutomaticTranspositionPromotion {
                syllable_index: 1,
                intended_code: "ufme".to_owned(),
                candidates: vec!["什么".to_owned()],
            })
        );
        assert_eq!(snapshot.candidate_texts("ufem", 3).unwrap(), before);
    }

    #[test]
    fn automatic_gate_can_recover_one_reversed_complete_syllable() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n马\tma\t1000\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "automatic-single-pair-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();

        assert_eq!(
            snapshot.automatic_transposition_decision("am").unwrap(),
            AutomaticTranspositionDecision::PromoteExactFullCode(AutomaticTranspositionPromotion {
                syllable_index: 0,
                intended_code: "ma".to_owned(),
                candidates: vec!["马".to_owned()],
            })
        );
        assert_eq!(
            snapshot.automatic_transposition_decision("ma").unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::OriginalHasExactFullCode
            )
        );
    }

    #[test]
    fn pinned_public_gate_recovers_a_fast_reduplicated_syllable_pair() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        assert_eq!(
            decoder.automatic_transposition_decision("wuuw").unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::AmbiguousSwapLocations
            ),
            "the host-independent gate has no timing evidence for choosing a location"
        );
        let promotion = decoder
            .automatic_transposition_recovery_after_primary("wuuw", 1, 6)
            .unwrap()
            .expect("the timed host request identifies the just-completed pair");
        assert_eq!(promotion.syllable_index, 1);
        assert_eq!(promotion.intended_code, "wuwu");
        assert_eq!(
            promotion.candidates.first().map(String::as_str),
            Some("呜呜")
        );
        assert!(
            !decoder
                .interactive_candidate_query("wuuw", 6)
                .unwrap()
                .automatic_transposition_blocked
        );
    }

    #[test]
    fn pinned_public_gate_recovers_two_adjacent_reversed_syllables_only_as_one_span() {
        const PUBLIC_RIME: &str =
            include_str!("../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml");
        let imported = crate::parse_simplified_rime_lexicon(PUBLIC_RIME).unwrap();
        let decoder = Decoder::new(imported.entries);

        assert!(
            decoder
                .automatic_transposition_recovery_after_primary("fuem", 0, 6)
                .unwrap()
                .is_none(),
            "repairing only fu must not invent a whole-word recovery"
        );
        assert!(
            decoder
                .automatic_transposition_recovery_after_primary("fuem", 1, 6)
                .unwrap()
                .is_none(),
            "repairing only em must not invent a whole-word recovery"
        );
        let promotion = decoder
            .automatic_transposition_span_recovery_after_primary("fuem", 0, 2, 6)
            .unwrap()
            .expect("two measured adjacent pairs should expose the combined full-code recovery");
        assert_eq!(promotion.syllable_index, 0);
        assert_eq!(promotion.intended_code, "ufme");
        assert_eq!(
            promotion.candidates.first().map(String::as_str),
            Some("什么")
        );
        assert!(
            !decoder
                .interactive_candidate_query("fuem", 6)
                .unwrap()
                .automatic_transposition_blocked
        );
    }

    #[test]
    fn automatic_gate_never_reinterprets_an_existing_exact_full_code() {
        const LEXICON: &str = "text\tpinyin\tfrequency\n\
什么\tshen me\t295403\n";
        let snapshot = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "automatic-exact-block-v1",
            contains_private_text: false,
            lexicon_tsv: LEXICON,
            expected_payload_bytes: LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(LEXICON.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap();

        assert_eq!(
            snapshot.automatic_transposition_decision("ufme").unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::OriginalHasExactFullCode
            )
        );
        assert!(
            snapshot
                .interactive_candidate_query("ufme", 1)
                .unwrap()
                .automatic_transposition_blocked
        );
        assert_eq!(
            snapshot.automatic_transposition_decision("ufe").unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::UnsupportedInputShape
            )
        );
    }

    #[test]
    fn automatic_gate_keeps_complete_sentences_and_ambiguous_swap_locations() {
        const COMPLETE_SENTENCE_LEXICON: &str = "text\tpinyin\tfrequency\n\
林\tlin\t1000\n\
好\thao\t900\n\
奶号\tnai hao\t800\n";
        let complete_sentence = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "automatic-complete-sentence-block-v1",
            contains_private_text: false,
            lexicon_tsv: COMPLETE_SENTENCE_LEXICON,
            expected_payload_bytes: COMPLETE_SENTENCE_LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(
                COMPLETE_SENTENCE_LEXICON.as_bytes(),
            ),
            expected_entry_count: 3,
        })
        .unwrap();
        assert_eq!(
            complete_sentence
                .automatic_transposition_decision("lnhk")
                .unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::OriginalFirstCandidateIsComplete
            )
        );
        assert!(
            complete_sentence
                .interactive_candidate_query("lnhk", 1)
                .unwrap()
                .automatic_transposition_blocked
        );

        const AMBIGUOUS_LEXICON: &str = "text\tpinyin\tfrequency\n\
奶号\tnai hao\t1000\n\
林康\tlin kang\t900\n";
        let ambiguous = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "automatic-ambiguous-pair-v1",
            contains_private_text: false,
            lexicon_tsv: AMBIGUOUS_LEXICON,
            expected_payload_bytes: AMBIGUOUS_LEXICON.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(
                AMBIGUOUS_LEXICON.as_bytes(),
            ),
            expected_entry_count: 2,
        })
        .unwrap();
        assert_eq!(
            ambiguous.automatic_transposition_decision("lnhk").unwrap(),
            AutomaticTranspositionDecision::KeepPrimary(
                AutomaticTranspositionKeepReason::AmbiguousSwapLocations
            )
        );
        assert_eq!(
            ambiguous
                .automatic_transposition_recovery_after_primary("lnhk", 1, 1)
                .unwrap(),
            Some(AutomaticTranspositionPromotion {
                syllable_index: 1,
                intended_code: "lnkh".to_owned(),
                candidates: vec!["林康".to_owned()],
            }),
            "a timed host request may use the causally identified final pair"
        );
    }

    #[test]
    #[ignore = "explicit full public-dictionary collision audit"]
    fn pinned_public_dictionary_transposition_collision_audit() {
        let imported = crate::parse_simplified_rime_lexicon(include_str!(
            "../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"
        ))
        .unwrap();
        let exact_codes = imported
            .entries
            .iter()
            .map(|entry| entry.code.as_str().to_owned())
            .collect::<HashSet<_>>();
        let mut intended_codes_by_observed = HashMap::<String, HashSet<String>>::new();

        for entry in &imported.entries {
            let intended = entry.code.as_str();
            if intended.len() < 2
                || intended.len() > MAX_TRANSPOSITION_RECOVERY_KEYS
                || !intended.len().is_multiple_of(2)
            {
                continue;
            }
            for syllable_index in 0..intended.len() / 2 {
                let swap_start = syllable_index * 2;
                if intended.as_bytes()[swap_start] == intended.as_bytes()[swap_start + 1] {
                    continue;
                }
                let mut observed = intended.as_bytes().to_vec();
                observed.swap(swap_start, swap_start + 1);
                let observed = String::from_utf8(observed).unwrap();
                intended_codes_by_observed
                    .entry(observed)
                    .or_default()
                    .insert(intended.to_owned());
            }
        }

        let exact_collisions = intended_codes_by_observed
            .keys()
            .filter(|observed| exact_codes.contains(*observed))
            .count();
        let unambiguous_non_collisions = intended_codes_by_observed
            .iter()
            .filter(|(observed, intended)| !exact_codes.contains(*observed) && intended.len() == 1)
            .count();
        let ambiguous_non_collisions = intended_codes_by_observed
            .iter()
            .filter(|(observed, intended)| !exact_codes.contains(*observed) && intended.len() > 1)
            .count();

        eprintln!(
            "exact_codes={} observed_forms={} exact_collisions={} unambiguous_non_collisions={} ambiguous_non_collisions={}",
            exact_codes.len(),
            intended_codes_by_observed.len(),
            exact_collisions,
            unambiguous_non_collisions,
            ambiguous_non_collisions,
        );
        assert!(!exact_codes.contains("ufem"));
        assert!(!exact_codes.contains("am"));
        assert_eq!(
            intended_codes_by_observed
                .get("am")
                .expect("the public dictionary should expose ma through the reversed pair am"),
            &HashSet::from(["ma".to_owned()])
        );
        assert_eq!(
            intended_codes_by_observed
                .get("ufem")
                .expect("the public dictionary should expose 什么 through ufme"),
            &HashSet::from(["ufme".to_owned()])
        );

        let decoder = Decoder::new(imported.entries);
        assert!(matches!(
            decoder.automatic_transposition_decision("ufem").unwrap(),
            AutomaticTranspositionDecision::PromoteExactFullCode(
                AutomaticTranspositionPromotion { ref candidates, .. }
            ) if candidates.first().map(String::as_str) == Some("什么")
        ));
    }

    #[test]
    #[ignore = "explicit release-only public transposition hot-path benchmark"]
    fn pinned_public_transposition_probe_reuses_the_primary_gate() {
        assert!(
            !cfg!(debug_assertions),
            "run this benchmark with cargo test --release"
        );
        let imported = crate::parse_simplified_rime_lexicon(include_str!(
            "../data/public/rime-pinyin-simp/pinyin_simp.dict.yaml"
        ))
        .unwrap();
        let decoder = Decoder::new(imported.entries);
        let observed_codes = [
            "ufem",
            "dmls",
            "rbbr",
            "ubxrdx",
            "uidv",
            "ubip",
            "ugum",
            "ypum",
            "ouum",
            "vijcjuyx",
            "gdmnxy",
            "buuidiyige",
        ];
        let eligible = observed_codes
            .into_iter()
            .filter(|code| {
                !decoder
                    .interactive_candidate_query(code, 7)
                    .unwrap()
                    .automatic_transposition_blocked
            })
            .collect::<Vec<_>>();
        assert!(!eligible.is_empty());

        for _ in 0..2 {
            for code in &eligible {
                black_box(decoder.automatic_transposition_decision(code).unwrap());
                black_box(
                    decoder
                        .automatic_transposition_recovery_after_primary(code, code.len() / 2 - 1, 1)
                        .unwrap(),
                );
            }
        }

        const REPETITIONS: usize = 20;
        let legacy_started = Instant::now();
        for _ in 0..REPETITIONS {
            for code in &eligible {
                black_box(decoder.automatic_transposition_decision(code).unwrap());
            }
        }
        let legacy = legacy_started.elapsed();
        let reused_started = Instant::now();
        for _ in 0..REPETITIONS {
            for code in &eligible {
                black_box(
                    decoder
                        .automatic_transposition_recovery_after_primary(code, code.len() / 2 - 1, 1)
                        .unwrap(),
                );
            }
        }
        let reused = reused_started.elapsed();
        let samples = REPETITIONS * eligible.len();
        println!(
            "PUBLIC_TRANSPOSITION_PROBE eligible_codes={} samples={} repeated_gate_us={:.3} reused_gate_us={:.3}",
            eligible.len(),
            samples,
            legacy.as_secs_f64() * 1_000_000.0 / samples as f64,
            reused.as_secs_f64() * 1_000_000.0 / samples as f64,
        );
    }

    #[test]
    fn descriptor_rejects_schema_revision_length_fingerprint_and_count_drift() {
        assert_eq!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                schema: "future",
                ..DEMO_DESCRIPTOR
            })
            .unwrap_err(),
            CandidateSnapshotError::UnsupportedSchema
        );
        assert_eq!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                revision: "私人版本",
                ..DEMO_DESCRIPTOR
            })
            .unwrap_err(),
            CandidateSnapshotError::InvalidRevision
        );
        assert!(matches!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                expected_payload_bytes: DEMO_LEXICON.len() + 1,
                ..DEMO_DESCRIPTOR
            }),
            Err(CandidateSnapshotError::PayloadLengthMismatch { .. })
        ));
        assert!(matches!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                expected_payload_fingerprint: 0,
                ..DEMO_DESCRIPTOR
            }),
            Err(CandidateSnapshotError::PayloadFingerprintMismatch { .. })
        ));
        assert!(matches!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                expected_entry_count: 49,
                ..DEMO_DESCRIPTOR
            }),
            Err(CandidateSnapshotError::EntryCountMismatch { .. })
        ));
    }

    #[test]
    fn descriptor_rejects_declared_sizes_and_counts_above_fixed_limits() {
        assert!(matches!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                expected_payload_bytes: MAX_CANDIDATE_SNAPSHOT_BYTES + 1,
                ..DEMO_DESCRIPTOR
            }),
            Err(CandidateSnapshotError::PayloadTooLarge { .. })
        ));
        assert!(matches!(
            CandidateSnapshot::load(CandidateSnapshotDescriptor {
                expected_entry_count: MAX_CANDIDATE_SNAPSHOT_ENTRIES + 1,
                ..DEMO_DESCRIPTOR
            }),
            Err(CandidateSnapshotError::InvalidExpectedEntryCount { .. })
        ));
    }

    #[test]
    fn malformed_private_lexicon_error_does_not_echo_payload_text() {
        const MALFORMED: &str = "text\tpinyin\tfrequency\n秘密词\tprivate@reading\t1\n";
        let error = CandidateSnapshot::load(CandidateSnapshotDescriptor {
            schema: CANDIDATE_SNAPSHOT_SCHEMA_V1,
            revision: "private-test-v1",
            contains_private_text: true,
            lexicon_tsv: MALFORMED,
            expected_payload_bytes: MALFORMED.len(),
            expected_payload_fingerprint: candidate_payload_fingerprint(MALFORMED.as_bytes()),
            expected_entry_count: 1,
        })
        .unwrap_err();
        assert_eq!(error, CandidateSnapshotError::Lexicon);
        assert_eq!(error.to_string(), "候选快照词典结构无效");
        assert!(!format!("{error:?}").contains("private@reading"));
        assert!(!format!("{error:?}").contains("秘密词"));
    }
}
