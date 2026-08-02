//! Bounded, read-only candidate snapshots for interactive hosts.
//!
//! This layer validates already-available bytes. It deliberately performs no
//! file discovery, persistence, decryption, learning, or network access.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use crate::{
    Decoder, KeySequence, KeySequenceError, parse_lexicon_tsv,
    spelling_is_complete_or_anchored_suffix,
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

    fn interactive_candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        self.decoder.interactive_candidate_texts(code, limit)
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
}

impl Decoder {
    /// Applies the same bounded candidate ordering used by interactive
    /// snapshots without performing snapshot I/O or changing host behavior.
    pub(crate) fn interactive_candidate_texts(
        &self,
        code: &str,
        limit: usize,
    ) -> Result<Vec<String>, KeySequenceError> {
        let exact = self.decode_exact_full_code(code, limit)?;
        let sentence_limit = limit
            .saturating_add(exact.len())
            .min(MAX_CANDIDATE_SNAPSHOT_RANK);
        let candidates = self.decode_sentence(code, sentence_limit)?;
        let canonical_code = canonicalize_umlaut_full_code(code);
        let canonical_candidates = match canonical_code.as_deref() {
            Some(canonical) => self.decode_sentence(canonical, sentence_limit)?,
            None => Vec::new(),
        };
        let mut visible = Vec::with_capacity(limit);
        let mut seen = HashSet::new();
        for candidate in exact {
            if seen.insert(candidate.text.clone()) {
                visible.push(candidate.text);
            }
        }

        // A complete two-key-per-syllable sentence is stronger interaction
        // evidence than a freely abbreviated path. Keep the research decoder's
        // ordinary order intact underneath this small host-facing lane.
        for candidate in canonical_candidates.iter().chain(&candidates) {
            if visible.len() == limit {
                break;
            }
            if sentence_is_complete(candidate) && seen.insert(candidate.text.clone()) {
                visible.push(candidate.text.clone());
            }
        }
        for (index, candidate) in candidates.into_iter().enumerate() {
            if visible.len() == limit {
                break;
            }
            if candidate.unresolved_key_count == 0 {
                if seen.insert(candidate.text.clone()) {
                    visible.push(candidate.text);
                }
            } else if index == 0 && visible.is_empty() {
                visible.push(code.to_owned());
                break;
            } else {
                break;
            }
        }
        Ok(visible)
    }
}

fn canonicalize_umlaut_full_code(code: &str) -> Option<String> {
    if !code.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = code.as_bytes().to_vec();
    let mut changed = false;
    for chunk in bytes.chunks_exact_mut(2) {
        if chunk[1] == b'u' && matches!(chunk[0], b'j' | b'q' | b'x' | b'y') {
            chunk[1] = b'v';
            changed = true;
        }
    }
    changed.then(|| {
        String::from_utf8(bytes)
            .expect("lowercase ASCII remains valid UTF-8 after umlaut code normalization")
    })
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
    fn pinned_public_dictionary_accepts_standard_u_spelling_for_umlaut_syllables() {
        let imported = crate::parse_rime_lexicon(include_str!(
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
