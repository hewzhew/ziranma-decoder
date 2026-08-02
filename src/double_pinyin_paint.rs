//! Experimental paint scheduling for the odd/even rhythm of complete
//! double-pinyin input.
//!
//! This module does not own a window, decode candidates, read timing data, or
//! change the TSF alpha. It only decides when an already-computed candidate
//! frame is eligible to replace the currently painted frame.

/// One accepted input revision and whether it displaced an unpublished ready
/// frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HalfPairInputEffect {
    /// False when the revision or timestamp did not move forward.
    pub accepted: bool,
    /// A ready odd-key frame was superseded before it was painted.
    pub suppressed_ready_frame: bool,
}

/// One frame that became eligible for painting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HalfPairPaint {
    /// Monotonic input revision represented by the frame.
    pub revision: u64,
    /// Time between the matching input and this paint decision.
    pub waited_ms: u64,
}

/// Outcome when an asynchronous candidate frame becomes ready.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HalfPairFrameDisposition {
    /// The frame no longer belongs to the latest input revision.
    Stale,
    /// The same revision has already been painted.
    AlreadyPainted,
    /// The latest odd-key frame is ready but remains unpublished until this
    /// deadline, unless the completing rhyme key supersedes it first.
    Deferred { due_ms: u64 },
    /// The frame can be painted immediately.
    Paint(HalfPairPaint),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveHalfPairInput {
    revision: u64,
    code_keys: usize,
    input_at_ms: u64,
    due_ms: u64,
    ready: bool,
    painted: bool,
}

/// Bounded, revision-aware coalescer for an odd initial-key frame followed by
/// its even complete-syllable frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HalfPairPaintCoalescer {
    delay_ms: u64,
    last_revision: Option<u64>,
    last_input_at_ms: Option<u64>,
    active: Option<ActiveHalfPairInput>,
}

impl HalfPairPaintCoalescer {
    /// Creates a scheduler with a fixed maximum odd-frame delay.
    pub fn new(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            last_revision: None,
            last_input_at_ms: None,
            active: None,
        }
    }

    /// Registers the newest composition input before its candidate frame is
    /// computed. Revisions and timestamps must both be monotonic.
    pub fn on_input(&mut self, revision: u64, code_keys: usize, at_ms: u64) -> HalfPairInputEffect {
        let monotonic_revision = self
            .last_revision
            .is_none_or(|previous| revision > previous);
        let monotonic_time = self
            .last_input_at_ms
            .is_none_or(|previous| at_ms >= previous);
        if !monotonic_revision || !monotonic_time {
            return HalfPairInputEffect::default();
        }

        let suppressed_ready_frame = self
            .active
            .is_some_and(|active| active.ready && !active.painted);
        self.last_revision = Some(revision);
        self.last_input_at_ms = Some(at_ms);
        self.active = Some(ActiveHalfPairInput {
            revision,
            code_keys,
            input_at_ms: at_ms,
            due_ms: at_ms.saturating_add(self.delay_ms),
            ready: false,
            painted: false,
        });
        HalfPairInputEffect {
            accepted: true,
            suppressed_ready_frame,
        }
    }

    /// Supplies the candidate frame for one input revision.
    pub fn on_frame_ready(&mut self, revision: u64, at_ms: u64) -> HalfPairFrameDisposition {
        let Some(active) = self.active.as_mut() else {
            return HalfPairFrameDisposition::Stale;
        };
        if active.revision != revision || at_ms < active.input_at_ms {
            return HalfPairFrameDisposition::Stale;
        }
        if active.painted {
            return HalfPairFrameDisposition::AlreadyPainted;
        }
        active.ready = true;
        let odd_half_pair = !active.code_keys.is_multiple_of(2);
        if odd_half_pair && at_ms < active.due_ms {
            return HalfPairFrameDisposition::Deferred {
                due_ms: active.due_ms,
            };
        }
        active.painted = true;
        HalfPairFrameDisposition::Paint(HalfPairPaint {
            revision,
            waited_ms: at_ms.saturating_sub(active.input_at_ms),
        })
    }

    /// Paints a ready odd frame whose bounded deadline has expired.
    pub fn on_timer(&mut self, at_ms: u64) -> Option<HalfPairPaint> {
        let active = self.active.as_mut()?;
        if active.painted
            || !active.ready
            || active.code_keys.is_multiple_of(2)
            || at_ms < active.due_ms
        {
            return None;
        }
        active.painted = true;
        Some(HalfPairPaint {
            revision: active.revision,
            waited_ms: at_ms.saturating_sub(active.input_at_ms),
        })
    }

    /// Forces the latest ready frame visible for an explicit UI action such as
    /// paging. It never resurrects a stale frame.
    pub fn force_latest(&mut self, at_ms: u64) -> Option<HalfPairPaint> {
        let active = self.active.as_mut()?;
        if active.painted || !active.ready || at_ms < active.input_at_ms {
            return None;
        }
        active.painted = true;
        Some(HalfPairPaint {
            revision: active.revision,
            waited_ms: at_ms.saturating_sub(active.input_at_ms),
        })
    }

    /// Cancels the current composition frame and returns whether a ready frame
    /// was discarded before painting.
    pub fn cancel(&mut self) -> bool {
        self.active
            .take()
            .is_some_and(|active| active.ready && !active.painted)
    }
}

/// One fixed paint-delay row. Values are structural timer bounds, not fitted
/// parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HalfPairPaintProfile {
    pub label: &'static str,
    pub delay_ms: u64,
}

/// Fixed timer sweep for the synthetic engineering audit.
pub const HALF_PAIR_PAINT_PROFILES: [HalfPairPaintProfile; 5] = [
    HalfPairPaintProfile {
        label: "immediate",
        delay_ms: 0,
    },
    HalfPairPaintProfile {
        label: "delay-16",
        delay_ms: 16,
    },
    HalfPairPaintProfile {
        label: "delay-24",
        delay_ms: 24,
    },
    HalfPairPaintProfile {
        label: "delay-32",
        delay_ms: 32,
    },
    HalfPairPaintProfile {
        label: "delay-48",
        delay_ms: 48,
    },
];

/// One fixed synthetic within-syllable timing pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HalfPairSyntheticCadence {
    pub label: &'static str,
    pub within_syllable_gaps_ms: &'static [u64],
}

const BURST_GAPS_MS: &[u64] = &[8, 12, 16, 20];
const MIXED_GAPS_MS: &[u64] = &[12, 20, 28, 40, 80];
const DELIBERATE_GAPS_MS: &[u64] = &[60, 80, 100, 120];

/// Fixed synthetic cadence sweep. These rows are not claims about user speed.
pub const HALF_PAIR_SYNTHETIC_CADENCES: [HalfPairSyntheticCadence; 3] = [
    HalfPairSyntheticCadence {
        label: "burst-8-20",
        within_syllable_gaps_ms: BURST_GAPS_MS,
    },
    HalfPairSyntheticCadence {
        label: "mixed-12-80",
        within_syllable_gaps_ms: MIXED_GAPS_MS,
    },
    HalfPairSyntheticCadence {
        label: "deliberate-60-120",
        within_syllable_gaps_ms: DELIBERATE_GAPS_MS,
    },
];

/// Deterministic paint-count evidence for one delay/cadence pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HalfPairPaintAuditReport {
    pub profile: HalfPairPaintProfile,
    pub cadence: HalfPairSyntheticCadence,
    pub syllables: usize,
    pub immediate_baseline_frames: usize,
    pub painted_frames: usize,
    pub odd_frames_deferred: usize,
    pub odd_frames_painted: usize,
    pub odd_frames_suppressed: usize,
    pub even_frames_painted: usize,
    pub maximum_odd_wait_ms: u64,
}

impl HalfPairPaintAuditReport {
    /// Frames avoided relative to repainting after every letter key.
    pub fn frames_avoided(&self) -> usize {
        self.immediate_baseline_frames
            .saturating_sub(self.painted_frames)
    }
}

/// Runs fixed synthetic timings through the same revision-aware scheduler that
/// a future UI experiment can reuse.
pub fn audit_half_pair_paint_profiles(
    syllables: usize,
    profiles: &[HalfPairPaintProfile],
    cadences: &[HalfPairSyntheticCadence],
) -> Vec<HalfPairPaintAuditReport> {
    let mut reports = Vec::with_capacity(profiles.len().saturating_mul(cadences.len()));
    for profile in profiles {
        for cadence in cadences {
            if cadence.within_syllable_gaps_ms.is_empty() {
                continue;
            }
            let mut scheduler = HalfPairPaintCoalescer::new(profile.delay_ms);
            let mut revision = 0_u64;
            let mut at_ms = 0_u64;
            let mut report = HalfPairPaintAuditReport {
                profile: *profile,
                cadence: *cadence,
                syllables,
                immediate_baseline_frames: syllables.saturating_mul(2),
                painted_frames: 0,
                odd_frames_deferred: 0,
                odd_frames_painted: 0,
                odd_frames_suppressed: 0,
                even_frames_painted: 0,
                maximum_odd_wait_ms: 0,
            };

            for syllable_index in 0..syllables {
                let gap_ms = cadence.within_syllable_gaps_ms
                    [syllable_index % cadence.within_syllable_gaps_ms.len()];
                revision = revision.saturating_add(1);
                let odd_effect = scheduler.on_input(revision, 1, at_ms);
                debug_assert!(odd_effect.accepted);
                match scheduler.on_frame_ready(revision, at_ms) {
                    HalfPairFrameDisposition::Paint(paint) => {
                        report.painted_frames += 1;
                        report.odd_frames_painted += 1;
                        report.maximum_odd_wait_ms =
                            report.maximum_odd_wait_ms.max(paint.waited_ms);
                    }
                    HalfPairFrameDisposition::Deferred { due_ms } => {
                        report.odd_frames_deferred += 1;
                        if due_ms <= at_ms.saturating_add(gap_ms)
                            && let Some(paint) = scheduler.on_timer(due_ms)
                        {
                            report.painted_frames += 1;
                            report.odd_frames_painted += 1;
                            report.maximum_odd_wait_ms =
                                report.maximum_odd_wait_ms.max(paint.waited_ms);
                        }
                    }
                    HalfPairFrameDisposition::Stale | HalfPairFrameDisposition::AlreadyPainted => {
                        unreachable!("a fresh synthetic odd frame is neither stale nor duplicate")
                    }
                }

                let even_at_ms = at_ms.saturating_add(gap_ms);
                revision = revision.saturating_add(1);
                let effect = scheduler.on_input(revision, 2, even_at_ms);
                debug_assert!(effect.accepted);
                report.odd_frames_suppressed += usize::from(effect.suppressed_ready_frame);
                match scheduler.on_frame_ready(revision, even_at_ms) {
                    HalfPairFrameDisposition::Paint(_) => {
                        report.painted_frames += 1;
                        report.even_frames_painted += 1;
                    }
                    HalfPairFrameDisposition::Stale
                    | HalfPairFrameDisposition::AlreadyPainted
                    | HalfPairFrameDisposition::Deferred { .. } => {
                        unreachable!("a fresh complete syllable paints immediately")
                    }
                }
                at_ms = even_at_ms.saturating_add(40);
            }

            debug_assert_eq!(report.even_frames_painted, syllables);
            debug_assert_eq!(
                report.odd_frames_painted + report.odd_frames_suppressed,
                syllables
            );
            debug_assert_eq!(
                report.painted_frames,
                report.odd_frames_painted + report.even_frames_painted
            );
            reports.push(report);
        }
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::{
        HALF_PAIR_PAINT_PROFILES, HALF_PAIR_SYNTHETIC_CADENCES, HalfPairFrameDisposition,
        HalfPairPaintCoalescer, audit_half_pair_paint_profiles,
    };

    #[test]
    fn fast_rhyme_key_suppresses_one_ready_odd_frame() {
        let mut scheduler = HalfPairPaintCoalescer::new(32);
        assert!(scheduler.on_input(1, 1, 100).accepted);
        assert_eq!(
            scheduler.on_frame_ready(1, 104),
            HalfPairFrameDisposition::Deferred { due_ms: 132 }
        );

        let effect = scheduler.on_input(2, 2, 120);
        assert!(effect.accepted);
        assert!(effect.suppressed_ready_frame);
        assert!(matches!(
            scheduler.on_frame_ready(2, 124),
            HalfPairFrameDisposition::Paint(_)
        ));
        assert_eq!(scheduler.on_timer(132), None);
    }

    #[test]
    fn paused_odd_frame_paints_at_deadline_and_explicit_ui_can_force_it() {
        let mut scheduler = HalfPairPaintCoalescer::new(24);
        assert!(scheduler.on_input(1, 1, 10).accepted);
        assert!(matches!(
            scheduler.on_frame_ready(1, 12),
            HalfPairFrameDisposition::Deferred { due_ms: 34 }
        ));
        assert_eq!(scheduler.on_timer(33), None);
        assert_eq!(scheduler.on_timer(34).unwrap().waited_ms, 24);

        assert!(scheduler.on_input(2, 3, 50).accepted);
        assert!(matches!(
            scheduler.on_frame_ready(2, 51),
            HalfPairFrameDisposition::Deferred { .. }
        ));
        assert_eq!(scheduler.force_latest(52).unwrap().revision, 2);
    }

    #[test]
    fn stale_decode_and_nonmonotonic_input_cannot_replace_the_latest_frame() {
        let mut scheduler = HalfPairPaintCoalescer::new(32);
        assert!(scheduler.on_input(10, 1, 100).accepted);
        assert!(scheduler.on_input(11, 2, 110).accepted);
        assert_eq!(
            scheduler.on_frame_ready(10, 115),
            HalfPairFrameDisposition::Stale
        );
        assert!(!scheduler.on_input(11, 3, 120).accepted);
        assert!(!scheduler.on_input(12, 3, 109).accepted);
        assert!(matches!(
            scheduler.on_frame_ready(11, 115),
            HalfPairFrameDisposition::Paint(_)
        ));
    }

    #[test]
    fn fixed_matrix_preserves_every_even_frame_and_partitions_odd_frames() {
        let reports = audit_half_pair_paint_profiles(
            20,
            &HALF_PAIR_PAINT_PROFILES,
            &HALF_PAIR_SYNTHETIC_CADENCES,
        );
        assert_eq!(reports.len(), 15);
        for report in reports {
            assert_eq!(report.immediate_baseline_frames, 40);
            assert_eq!(report.even_frames_painted, 20);
            assert_eq!(report.odd_frames_painted + report.odd_frames_suppressed, 20);
            assert_eq!(report.frames_avoided(), report.odd_frames_suppressed);
        }
    }

    #[test]
    fn empty_custom_cadence_is_skipped_instead_of_dividing_by_zero() {
        let empty = [super::HalfPairSyntheticCadence {
            label: "empty",
            within_syllable_gaps_ms: &[],
        }];
        assert!(
            audit_half_pair_paint_profiles(1, &HALF_PAIR_PAINT_PROFILES[..1], &empty).is_empty()
        );
    }
}
