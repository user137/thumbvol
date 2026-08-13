//! Platform-agnostic wheel-tick -> volume-step logic.
//!
//! Platform backends normalize their raw HID delta into "notch units" (one
//! physical detent of the thumb wheel == 1) before feeding it here, so this
//! module never sees Windows' WHEEL_DELTA=120 convention or evdev's raw
//! REL_HWHEEL units directly.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeStep {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelConfigError {
    InvalidNotchesPerStep(i32),
    InvalidPressesPerStep(i32),
}

impl std::fmt::Display for WheelConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WheelConfigError::InvalidNotchesPerStep(n) => {
                write!(f, "notches_per_step must be positive, got {n}")
            }
            WheelConfigError::InvalidPressesPerStep(n) => {
                write!(
                    f,
                    "sensitivity (presses_per_step) must be positive, got {n}"
                )
            }
        }
    }
}

impl std::error::Error for WheelConfigError {}

/// Upper bound on volume steps a single `feed()` call can emit. A real
/// thumb-wheel HID report never represents more than a handful of detents,
/// and `sensitivity` is meant for "a couple of presses per notch", not
/// hundreds; this cap is what keeps the loop below provably O(1) instead of
/// relying on "no device will ever send a huge delta" — a glitching or
/// malicious device is exactly the kind of input this boundary must survive.
const MAX_STEPS_PER_FEED: usize = 64;

#[derive(Debug)]
pub struct WheelAccumulator {
    notches_per_step: i32,
    invert: bool,
    /// How many volume-key presses to emit per notch that crosses the
    /// threshold — the "sensitivity" knob for when a single OS volume step
    /// (~2%) per physical detent feels too slow.
    presses_per_step: i32,
    accumulated: i32,
}

impl WheelAccumulator {
    pub fn new(
        notches_per_step: i32,
        invert: bool,
        presses_per_step: i32,
    ) -> Result<Self, WheelConfigError> {
        if notches_per_step <= 0 {
            return Err(WheelConfigError::InvalidNotchesPerStep(notches_per_step));
        }
        if presses_per_step <= 0 {
            return Err(WheelConfigError::InvalidPressesPerStep(presses_per_step));
        }
        Ok(Self {
            notches_per_step,
            invert,
            presses_per_step,
            accumulated: 0,
        })
    }

    /// Feed a raw, already-normalized notch delta (positive = wheel toward
    /// user/"forward", negative = away, per platform convention). Returns
    /// the volume steps this tick produced, in order — `presses_per_step`
    /// entries per notch crossed.
    pub fn feed(&mut self, delta: i32) -> Vec<VolumeStep> {
        let delta = if self.invert {
            delta.saturating_neg()
        } else {
            delta
        };
        self.accumulated = self.accumulated.saturating_add(delta);

        let mut steps = Vec::new();
        while self.accumulated >= self.notches_per_step && steps.len() < MAX_STEPS_PER_FEED {
            self.accumulated -= self.notches_per_step;
            push_capped(&mut steps, VolumeStep::Up, self.presses_per_step);
        }
        while self.accumulated <= -self.notches_per_step && steps.len() < MAX_STEPS_PER_FEED {
            self.accumulated += self.notches_per_step;
            push_capped(&mut steps, VolumeStep::Down, self.presses_per_step);
        }
        if steps.len() >= MAX_STEPS_PER_FEED {
            // Hit the cap: drop the remaining backlog instead of replaying
            // a step storm on the next tick once more room opens up.
            self.accumulated = 0;
        }
        steps
    }
}

/// Push up to `count` copies of `step` onto `steps`, never growing it past
/// `MAX_STEPS_PER_FEED` regardless of `count`.
fn push_capped(steps: &mut Vec<VolumeStep>, step: VolumeStep, count: i32) {
    let room = MAX_STEPS_PER_FEED.saturating_sub(steps.len());
    let count = usize::try_from(count).unwrap_or(0).min(room);
    steps.extend(std::iter::repeat_n(step, count));
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- correctness ---

    #[test]
    fn single_notch_up_emits_one_step() {
        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        assert_eq!(acc.feed(1), vec![VolumeStep::Up]);
    }

    #[test]
    fn single_notch_down_emits_one_step() {
        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        assert_eq!(acc.feed(-1), vec![VolumeStep::Down]);
    }

    #[test]
    fn sub_threshold_delta_accumulates_across_calls() {
        let mut acc = WheelAccumulator::new(3, false, 1).unwrap();
        assert_eq!(acc.feed(2), Vec::<VolumeStep>::new());
        assert_eq!(acc.feed(2), vec![VolumeStep::Up]); // 2+2=4 >= 3, one step, remainder 1
    }

    #[test]
    fn fast_spin_emits_multiple_steps_in_one_tick() {
        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        assert_eq!(
            acc.feed(3),
            vec![VolumeStep::Up, VolumeStep::Up, VolumeStep::Up]
        );
    }

    #[test]
    fn invert_flips_direction() {
        let mut acc = WheelAccumulator::new(1, true, 1).unwrap();
        assert_eq!(acc.feed(1), vec![VolumeStep::Down]);
        assert_eq!(acc.feed(-1), vec![VolumeStep::Up]);
    }

    #[test]
    fn opposite_delta_cancels_pending_accumulation_before_stepping() {
        let mut acc = WheelAccumulator::new(10, false, 1).unwrap();
        assert_eq!(acc.feed(6), Vec::<VolumeStep>::new()); // accumulated = 6
        assert_eq!(acc.feed(-6), Vec::<VolumeStep>::new()); // accumulated = 0, no step
        assert_eq!(acc.feed(-10), vec![VolumeStep::Down]);
    }

    #[test]
    fn presses_per_step_multiplies_each_notch() {
        let mut acc = WheelAccumulator::new(1, false, 3).unwrap();
        assert_eq!(
            acc.feed(1),
            vec![VolumeStep::Up, VolumeStep::Up, VolumeStep::Up]
        );
    }

    // --- rejection: invalid construction ---

    #[test]
    fn zero_notches_per_step_is_rejected() {
        assert_eq!(
            WheelAccumulator::new(0, false, 1).unwrap_err(),
            WheelConfigError::InvalidNotchesPerStep(0)
        );
    }

    #[test]
    fn negative_notches_per_step_is_rejected() {
        assert_eq!(
            WheelAccumulator::new(-5, false, 1).unwrap_err(),
            WheelConfigError::InvalidNotchesPerStep(-5)
        );
    }

    #[test]
    fn zero_presses_per_step_is_rejected() {
        assert_eq!(
            WheelAccumulator::new(1, false, 0).unwrap_err(),
            WheelConfigError::InvalidPressesPerStep(0)
        );
    }

    #[test]
    fn negative_presses_per_step_is_rejected() {
        assert_eq!(
            WheelAccumulator::new(1, false, -2).unwrap_err(),
            WheelConfigError::InvalidPressesPerStep(-2)
        );
    }

    // --- misuse: degenerate/extreme input, no partial-state corruption ---

    #[test]
    fn zero_delta_is_a_noop() {
        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        assert_eq!(acc.feed(0), Vec::<VolumeStep>::new());
    }

    #[test]
    fn extreme_delta_saturates_and_is_bounded_instead_of_overflowing() {
        // A glitching/malicious device could in principle send a huge delta;
        // this is the system boundary, so it must neither panic (overflow)
        // nor allocate/loop unboundedly (billions of steps for one HID
        // report) — it must terminate in O(MAX_STEPS_PER_FEED) regardless
        // of how large the input is.
        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        let steps = acc.feed(i32::MAX);
        assert!(!steps.is_empty());
        assert!(steps.len() <= MAX_STEPS_PER_FEED);
        assert!(steps.iter().all(|s| *s == VolumeStep::Up));

        let mut acc = WheelAccumulator::new(1, false, 1).unwrap();
        let steps = acc.feed(i32::MIN);
        assert!(!steps.is_empty());
        assert!(steps.len() <= MAX_STEPS_PER_FEED);
        assert!(steps.iter().all(|s| *s == VolumeStep::Down));
    }

    #[test]
    fn extreme_presses_per_step_is_still_bounded() {
        // Misuse via config: a user could set an absurdly large sensitivity.
        // A single notch must still never emit more than the global cap.
        let mut acc = WheelAccumulator::new(1, false, i32::MAX).unwrap();
        let steps = acc.feed(1);
        assert_eq!(steps.len(), MAX_STEPS_PER_FEED);
        assert!(steps.iter().all(|s| *s == VolumeStep::Up));
    }
}

/// Property-based coverage of `feed()` against arbitrary (including
/// adversarial-shaped) input — a proportionate stand-in for full-blown
/// `cargo fuzz` given this is one small, pure, already-bounded function, not
/// a parser over a complex untrusted format. Exists because the exact bug
/// class this guards against (an input-dependent unbounded loop) was found
/// by hand, not by the fixed-example unit tests above — see
/// `extreme_delta_saturates_and_is_bounded_instead_of_overflowing`'s history
/// in DECISIONS.md/CLAUDE.md.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn feed_never_panics_and_respects_the_cap(
            notches_per_step in 1..1_000_000i32,
            presses_per_step in 1..1_000_000i32,
            invert in any::<bool>(),
            delta in any::<i32>(),
        ) {
            let mut acc = WheelAccumulator::new(notches_per_step, invert, presses_per_step).unwrap();
            let steps = acc.feed(delta);
            prop_assert!(steps.len() <= MAX_STEPS_PER_FEED);
        }

        #[test]
        fn feed_sequence_never_panics_regardless_of_history(
            deltas in proptest::collection::vec(any::<i32>(), 0..200),
        ) {
            let mut acc = WheelAccumulator::new(3, false, 2).unwrap();
            for delta in deltas {
                let steps = acc.feed(delta);
                prop_assert!(steps.len() <= MAX_STEPS_PER_FEED);
            }
        }
    }
}
