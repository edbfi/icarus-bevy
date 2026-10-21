//! Native `ServerFPS` measurement following the original's rule.
//!
//! The original's game mode tick stores each positive frame time, clamped to
//! [`MAX_FRAME_SECONDS`], as whole ten-thousandths of a second in a
//! power-of-two ring and adds it to an accumulator. Once the accumulator
//! reaches [`WINDOW_SECONDS`] it is cleared and the GameState's `ServerFPS`
//! becomes `1 / mean` over the ring's positive entries, truncated (0 when
//! there are none). The ring keeps entries from earlier windows until they
//! are overwritten. Its capacity is set outside the code that was read:
//! [`DEFAULT_SAMPLES`] is a lab choice. Arithmetic is `f32` and `i32`, in
//! the original's order; the frame times measured here are this server's.
use anyhow::{Result, ensure};

/// The longest frame time a sample keeps.
pub const MAX_FRAME_SECONDS: f32 = 1677.0;

/// Frame time accumulated before `ServerFPS` is computed again.
pub const WINDOW_SECONDS: f32 = 2.0;

/// Lab choice for the ring capacity: about two seconds at 30 frames per
/// second (the original's capacity was not read).
pub const DEFAULT_SAMPLES: usize = 64;

/// At most this many samples.
pub const MAX_SAMPLES: usize = 4096;

/// Ten-thousandths of a second per second.
const SAMPLE_SCALE: f32 = 10_000.0;

/// The `ServerFPS` meter.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerFpsMeter {
    samples: Vec<i32>,
    head: usize,
    accumulated: f32,
    fps: i32,
}

impl ServerFpsMeter {
    /// A meter with at least `capacity` samples, rounded up to a power of
    /// two as a circular buffer is; `capacity` is 1 to [`MAX_SAMPLES`].
    pub fn new(capacity: usize) -> Result<Self> {
        ensure!(
            (1..=MAX_SAMPLES).contains(&capacity),
            "a ServerFPS ring holds 1 to {MAX_SAMPLES} samples"
        );
        Ok(Self {
            samples: vec![0; capacity.next_power_of_two()],
            head: 0,
            accumulated: 0.0,
            fps: 0,
        })
    }

    /// The ring's capacity.
    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    /// The last computed `ServerFPS` (0 before the first window closes).
    pub fn fps(&self) -> i32 {
        self.fps
    }

    /// Observe one frame of `delta_seconds`. Returns the recomputed
    /// `ServerFPS` when this frame closes a window; a delta that is not
    /// positive (or NaN) is ignored.
    pub fn observe(&mut self, delta_seconds: f32) -> Option<i32> {
        if delta_seconds.is_nan() || delta_seconds <= 0.0 {
            return None;
        }
        let delta = delta_seconds.min(MAX_FRAME_SECONDS);
        let mask = self.samples.len() - 1;
        // Below 1677 s the scaled value stays within `i32`.
        self.samples[self.head & mask] = (delta * SAMPLE_SCALE) as i32;
        let accumulated = delta + self.accumulated;
        self.head = (self.head + 1) & mask;
        self.accumulated = accumulated;
        if accumulated < WINDOW_SECONDS {
            return None;
        }
        self.accumulated = 0.0;
        let (mut count, mut sum) = (0i32, 0i32);
        for sample in self.samples.iter().filter(|sample| **sample > 0) {
            count += 1;
            sum = sum.wrapping_add(*sample);
        }
        self.fps = 0;
        if count != 0 {
            let mean = (sum as f32 / count as f32) * 1.0e-4;
            if mean != 0.0 {
                self.fps = truncate(1.0 / mean);
            }
        }
        Some(self.fps)
    }
}

/// `CVTTSS2SI`: truncation; NaN and out-of-range values give `i32::MIN`.
fn truncate(value: f32) -> i32 {
    if value > -2_147_483_904.0 && value < 2_147_483_648.0 {
        value as i32
    } else {
        i32::MIN
    }
}
