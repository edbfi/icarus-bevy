//! Native time-of-day clock following the original's rule (`F-806`).
//!
//! The original keeps the GameState's `TimeOfDay` in game minutes
//! (`0..1440`). Each tick adds `curve(TimeOfDay / 60) * TimeScale *
//! DeltaSeconds` to an accumulator; once the accumulator reaches the
//! minimum step it is committed through `SetTimeOfDay`, which wraps at 1440
//! (a value below the previous one starts a new day). Prospect setup writes
//! `SecondsPerGameDay` from the same curve: the truncated sum, over the 24
//! hours, of `3600 / (60 * mean)`, the mean taken between each hour's curve
//! value and the next hour's.
//!
//! The original's curve is per-world packaged data and stays out of scope:
//! [`RateCurve`] is authored by the caller. [`RateCurve::for_seconds_per_game_day`]
//! is the constant curve whose day lasts a given (for example a save's)
//! `SecondsPerGameDay`. Arithmetic is `f32`, in the original's order.
use anyhow::{Result, ensure};

/// Game minutes per day; `TimeOfDay` wraps here.
pub const MINUTES_PER_DAY: f32 = 1440.0;

/// `CalcSecondsPerGameDay`'s result when its sum is not positive.
pub const FALLBACK_SECONDS_PER_GAME_DAY: i32 = 3960;

/// The smallest `TimeScale` `SetTimeScale` keeps.
pub const MIN_TIME_SCALE: f32 = 1.0e-4;

/// At most this many curve keys.
pub const MAX_CURVE_KEYS: usize = 48;

/// One over 60 as the original's `f32` constant.
const ONE_SIXTIETH: f32 = 1.0 / 60.0;

/// An authored rate curve: game minutes per world second by hour of day.
/// Values between keys interpolate linearly; outside them they hold the end
/// keys' rates.
#[derive(Clone, Debug, PartialEq)]
pub struct RateCurve {
    keys: Vec<(f32, f32)>,
}

impl RateCurve {
    /// `keys` are `(hour, rate)` pairs: 1 to [`MAX_CURVE_KEYS`] of them,
    /// hours strictly increasing within `0..=24`, rates finite and positive.
    pub fn new(keys: Vec<(f32, f32)>) -> Result<Self> {
        ensure!(
            (1..=MAX_CURVE_KEYS).contains(&keys.len()),
            "a rate curve has 1 to {MAX_CURVE_KEYS} keys"
        );
        for (hour, rate) in &keys {
            ensure!(
                hour.is_finite() && (0.0..=24.0).contains(hour),
                "curve hours lie within 0..=24"
            );
            ensure!(
                rate.is_finite() && *rate > 0.0,
                "curve rates are finite and positive"
            );
        }
        ensure!(
            keys.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "curve hours strictly increase"
        );
        Ok(Self { keys })
    }

    /// A constant rate in game minutes per world second.
    pub fn constant(rate: f32) -> Result<Self> {
        Self::new(vec![(0.0, rate)])
    }

    /// The constant curve whose day lasts `seconds` world seconds:
    /// `1440 / seconds` game minutes per world second.
    pub fn for_seconds_per_game_day(seconds: i32) -> Result<Self> {
        ensure!(seconds > 0, "a day lasts a positive number of seconds");
        Self::constant(MINUTES_PER_DAY / seconds as f32)
    }

    /// The `(hour, rate)` keys.
    pub fn keys(&self) -> &[(f32, f32)] {
        &self.keys
    }

    /// The rate at `hour`.
    pub fn value(&self, hour: f32) -> f32 {
        let first = self.keys[0];
        let last = self.keys[self.keys.len() - 1];
        if hour.is_nan() || hour <= first.0 {
            return first.1;
        }
        if hour >= last.0 {
            return last.1;
        }
        let next = self.keys.partition_point(|(key, _)| *key <= hour);
        let (h0, r0) = self.keys[next - 1];
        let (h1, r1) = self.keys[next];
        r0 + (r1 - r0) * ((hour - h0) / (h1 - h0))
    }
}

/// `CalcSecondsPerGameDay`: for hours 0..23 the curve value (1.0 without a
/// curve) times 60, each hour's mean with the next hour's (hour 23 pairs
/// with hour 0), `3600 / mean` summed, then truncated; a sum that is not
/// positive gives [`FALLBACK_SECONDS_PER_GAME_DAY`]. The time scale is not
/// applied, as in the original.
pub fn seconds_per_game_day(curve: Option<&RateCurve>) -> i32 {
    let samples: [f32; 24] =
        std::array::from_fn(|hour| curve.map_or(1.0, |curve| curve.value(hour as f32)) * 60.0);
    let mut sum = 0.0f32;
    for hour in 0..24 {
        let (here, next) = (samples[hour], samples[(hour + 1) % 24]);
        let mean = if here == next {
            here
        } else if here <= next {
            (next - here) * 0.5 + here
        } else {
            here - (here - next) * 0.5
        };
        sum += 3600.0 / mean;
    }
    if sum > 0.0 {
        // `CVTTSS2SI`: truncation; out of range gives `i32::MIN`.
        if sum < 2_147_483_648.0 {
            sum as i32
        } else {
            i32::MIN
        }
    } else {
        FALLBACK_SECONDS_PER_GAME_DAY
    }
}

/// The world's time of day, advanced as the original's subsystem ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeOfDayClock {
    curve: RateCurve,
    time_scale: f32,
    /// The minimum step in game minutes.
    min_step: f32,
    time_of_day: f32,
    accumulated: f32,
    /// Days started since the clock was created.
    days: u32,
}

impl TimeOfDayClock {
    /// A clock at `time_of_day` (normalized as `SetTimeOfDay` does) with
    /// `TimeScale` 1 and no minimum step.
    pub fn new(curve: RateCurve, time_of_day: f32) -> Self {
        let mut clock = Self {
            curve,
            time_scale: 1.0,
            min_step: 0.0,
            time_of_day: 0.0,
            accumulated: 0.0,
            days: 0,
        };
        clock.time_of_day = normalize(time_of_day);
        clock
    }

    /// `SetTimeScale`: at least [`MIN_TIME_SCALE`]; NaN keeps the minimum.
    pub fn set_time_scale(&mut self, scale: f32) {
        self.time_scale = if scale > MIN_TIME_SCALE {
            scale
        } else {
            MIN_TIME_SCALE
        };
    }

    /// `SetMinimumTimeStepSeconds`: stored in game minutes, negative or NaN
    /// as 0; clears the accumulator.
    pub fn set_minimum_time_step_seconds(&mut self, seconds: f32) {
        self.accumulated = 0.0;
        self.min_step = if seconds > 0.0 { seconds } else { 0.0 } * ONE_SIXTIETH;
    }

    /// The current `TimeOfDay` in game minutes.
    pub fn time_of_day(&self) -> f32 {
        self.time_of_day
    }

    pub fn time_scale(&self) -> f32 {
        self.time_scale
    }

    pub fn curve(&self) -> &RateCurve {
        &self.curve
    }

    /// Days started since the clock was created.
    pub fn days(&self) -> u32 {
        self.days
    }

    /// `SecondsPerGameDay` for this clock's curve.
    pub fn seconds_per_game_day(&self) -> i32 {
        seconds_per_game_day(Some(&self.curve))
    }

    /// `Tick`: advance by `delta_seconds` of world time. Returns whether the
    /// time of day was committed (it may still be unchanged). A negative or
    /// NaN delta advances nothing.
    pub fn advance(&mut self, delta_seconds: f32) -> bool {
        let delta_seconds = if delta_seconds > 0.0 {
            delta_seconds
        } else {
            0.0
        };
        let step =
            self.curve.value(self.time_of_day * ONE_SIXTIETH) * self.time_scale * delta_seconds;
        let accumulated = step + self.accumulated;
        if accumulated < self.min_step {
            self.accumulated = accumulated;
            return false;
        }
        let mut next = step + self.accumulated + self.time_of_day;
        if MINUTES_PER_DAY <= next {
            next += -MINUTES_PER_DAY;
        }
        self.set_time_of_day(next);
        true
    }

    /// `SetTimeOfDay`: wrap into `0..1440` (clamping below 0); a value below
    /// the current one starts a new day. Clears the accumulator.
    pub fn set_time_of_day(&mut self, minutes: f32) {
        let minutes = normalize(minutes);
        if self.time_of_day >= 0.0 && minutes < self.time_of_day {
            self.days = self.days.saturating_add(1);
        }
        self.time_of_day = minutes;
        self.accumulated = 0.0;
    }
}

/// Subtract whole days while at or above 1440; below 0 (or NaN) becomes 0.
/// Lab bounds where the original would loop without end: infinity becomes
/// 0, and values of 64 days or more are first reduced by remainder.
fn normalize(mut minutes: f32) -> f32 {
    if minutes.is_infinite() {
        return 0.0;
    }
    if minutes >= MINUTES_PER_DAY * 64.0 {
        minutes %= MINUTES_PER_DAY;
    }
    while MINUTES_PER_DAY <= minutes {
        minutes += -MINUTES_PER_DAY;
    }
    if minutes > 0.0 { minutes } else { 0.0 }
}
