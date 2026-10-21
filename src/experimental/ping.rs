//! Native player ping following the original's `APlayerState` rule (`F-815`).
//!
//! Each round-trip sample is clamped to [`MAX_RTT_SECONDS`] and taken in
//! milliseconds. Samples fall into four one-second buckets: a bucket keeps a
//! 13-bit sum of up to seven whole-millisecond samples and their 3-bit count.
//! When a sample arrives a second or more after the current bucket opened,
//! the average over all four buckets is recomputed first (sum over count),
//! then the next bucket is reset to hold the new sample. The replicated ping
//! byte is the average times 0.25, truncated and capped at 255. The original
//! also keeps each bucket's seven lowest samples for a second average that is
//! not replicated; that is not modelled. Arithmetic is `f32` and integer, in
//! the original's order; the round-trip times measured here are this
//! server's.

/// The longest round trip a sample keeps, in seconds.
pub const MAX_RTT_SECONDS: f32 = 1.1;

/// Samples per bucket; later ones in the same second are dropped.
const BUCKET_SAMPLES: u8 = 7;
/// A bucket's sum is 13 bits wide.
const SUM_MASK: u16 = 0x1fff;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bucket {
    sum: u16,
    count: u8,
}

/// The ping estimator of one player.
#[derive(Clone, Debug, PartialEq)]
pub struct PingEstimator {
    buckets: [Bucket; 4],
    current: usize,
    /// When the current bucket opened, in the caller's seconds (`f64`, so
    /// long uptimes keep sub-second resolution; the original uses `f32`).
    opened_at: f64,
    exact_ping_ms: f32,
    ping: u8,
}

impl Default for PingEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl PingEstimator {
    pub fn new() -> Self {
        Self {
            buckets: [Bucket::default(); 4],
            current: 0,
            opened_at: 0.0,
            exact_ping_ms: 0.0,
            ping: 0,
        }
    }

    /// The replicated ping byte (a quarter of the average in ms).
    pub fn ping(&self) -> u8 {
        self.ping
    }

    /// The average round trip in milliseconds at the last recomputation.
    pub fn exact_ping_ms(&self) -> f32 {
        self.exact_ping_ms
    }

    /// Add a round-trip sample of `rtt_seconds` taken at `now_seconds` (any
    /// monotonic clock in seconds). NaN or negative samples are ignored.
    pub fn update(&mut self, rtt_seconds: f32, now_seconds: f64) {
        if rtt_seconds.is_nan() || rtt_seconds < 0.0 {
            return;
        }
        let ms = rtt_seconds.min(MAX_RTT_SECONDS) * 1000.0;
        let whole = ms.floor() as u16;
        if now_seconds - self.opened_at < 1.0 {
            let bucket = &mut self.buckets[self.current];
            if bucket.count < BUCKET_SAMPLES {
                bucket.sum = bucket.sum.wrapping_add(whole) & SUM_MASK;
                bucket.count += 1;
            }
        } else {
            self.recalculate();
            self.opened_at = now_seconds;
            self.current = (self.current + 1) & 3;
            self.buckets[self.current] = Bucket {
                sum: whole & SUM_MASK,
                count: 1,
            };
        }
    }

    /// The average over every bucket's samples; the ping byte follows.
    fn recalculate(&mut self) {
        let (sum, count) = self.buckets.iter().fold((0u32, 0u32), |(sum, count), b| {
            (sum + u32::from(b.sum), count + u32::from(b.count))
        });
        self.exact_ping_ms = if count > 0 {
            sum as f32 / count as f32
        } else {
            0.0
        };
        self.ping = ((self.exact_ping_ms * 0.25) as i32).clamp(0, 255) as u8;
    }
}
