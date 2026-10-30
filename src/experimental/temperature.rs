//! Pinned temperature setter/tick local control flow (F-997, F-999). Sampling,
//! stats and notification effects are caller supplied; this is not a climate
//! model, simulation schedule or persistence adapter. No local authority guard.

/// External boundaries in `SetInternalTemperature`. Each call may change
/// state before the next local operation. Calls return normally in this model.
pub trait InternalTemperatureCalls {
    /// Resolve one stat key. The setter requests `0xa2` (upper bound) before
    /// `0xa0` (lower bound), retaining each returned value independently.
    fn stat(&mut self, state: &mut i32, key: u16) -> i32;

    /// Receive the captured resolved temperature after its local store.
    /// This boundary owns sparse-delegate lookup, conditional multicast,
    /// shared-reference release and their external effects. A request does
    /// not establish that a listener exists or that one was invoked.
    fn temperature_updated(&mut self, state: &mut i32, value: i32);
}

/// Snapshot entry temperature, read the upper then lower bound, and store
/// the resolved value. Bounds are not reordered: below the lower bound selects
/// it; otherwise below the upper bound retains the request; otherwise the
/// upper bound is selected. This also defines inverted and equal bounds.
///
/// Notify exactly when the resolved value differs from the entry snapshot,
/// independent of mutations made by stat calls. Notification mutations are
/// retained. The return value describes that local notification request,
/// not whether the final callback-adjusted state differs from entry.
pub fn set_internal_temperature(
    state: &mut i32,
    requested: i32,
    calls: &mut impl InternalTemperatureCalls,
) -> bool {
    let previous = *state;
    let upper = calls.stat(state, 0xa2);
    let lower = calls.stat(state, 0xa0);
    let resolved = if requested < lower {
        lower
    } else if requested < upper {
        requested
    } else {
        upper
    };
    *state = resolved;
    let notify = resolved != previous;
    if notify {
        calls.temperature_updated(state, resolved);
    }
    notify
}

/// Fields read locally by the pinned `TemperatureTick`. All initial values
/// must be supplied; no climate, constructor defaults or saved mapping is
/// inferred. The presence flags abstract pointer tests, not object identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemperatureTickState {
    /// Whether the reference tested at receiver +0x218 is non-null.
    pub required_reference_present: bool,
    /// Whether the temperature singleton is cached; false requests acquisition.
    pub singleton_cached: bool,
    /// Sampling threshold in seconds; a tick samples only after exceeding it.
    pub sample_interval_seconds: f32,
    /// Accumulated sampling time in seconds; one interval is removed per sample.
    pub sample_elapsed_seconds: f32,
    /// Retained fractional internal-temperature adjustment in temperature units.
    pub internal_remainder: f32,
    /// Latest environmental sample, in integer temperature units.
    pub external: i32,
    /// Environmental temperature after the external stat adjustment.
    pub modified_external: i32,
    /// Internal target after the internal stat adjustment.
    pub modified_internal: i32,
    /// Current internal temperature, in integer temperature units.
    pub internal: i32,
}

/// External effects in `TemperatureTick`. Calls may mutate the state and
/// return normally. Implementations own lookup, sampling, stat resolution and
/// delegates; a requested callback does not prove a listener exists.
pub trait TemperatureTickCalls {
    fn stat(&mut self, state: &mut TemperatureTickState, key: u16) -> i32;
    /// Owns singleton class lookup, acquisition, type check and final cache
    /// assignment. Failure may leave the cache absent; ticking still continues.
    fn cache_temperature_singleton(&mut self, state: &mut TemperatureTickState);
    fn environmental_temperature(&mut self, state: &mut TemperatureTickState) -> i32;
    fn external_temperature_updated(&mut self, state: &mut TemperatureTickState, value: i32);
    /// Owns the internal setter, including its stat reads and notifications.
    /// The caller can compose [`set_internal_temperature`] here.
    fn set_internal_temperature(&mut self, state: &mut TemperatureTickState, requested: i32);
}

/// Execute one pinned 3.0.32 temperature tick (F-999). Sampling consumes at
/// most one interval, only when accumulated time is strictly greater. Internal
/// adjustment runs on every admitted call, using the retained sample otherwise.
/// There is no local authority guard; outer scheduling remains caller owned.
///
/// Binary32 operations assume nearest-even rounding, masked exceptions and
/// gradual underflow. Integer arithmetic wraps as the original instructions
/// do. The original process's floating environment is not qualified.
pub fn tick_temperature(
    state: &mut TemperatureTickState,
    delta_seconds: f32,
    calls: &mut impl TemperatureTickCalls,
) {
    if !state.required_reference_present || calls.stat(state, 0x9d) < 1 {
        return;
    }
    if !state.singleton_cached {
        calls.cache_temperature_singleton(state);
    }
    let elapsed = delta_seconds + state.sample_elapsed_seconds;
    let interval = state.sample_interval_seconds;
    state.sample_elapsed_seconds = elapsed;
    if elapsed > interval {
        let previous = state.modified_external;
        state.sample_elapsed_seconds = elapsed - interval;
        let sampled = calls.environmental_temperature(state);
        state.external = sampled;
        state.modified_external = sampled;
        let adjustment = calls.stat(state, 0x3c3);
        state.modified_external = state.modified_external.wrapping_add(adjustment);
        let resolved = state.modified_external;
        if resolved != previous {
            calls.external_temperature_updated(state, resolved);
        }
        let adjustment = calls.stat(state, 0x3c5);
        state.modified_internal = state.modified_external.wrapping_add(adjustment);
    }

    let internal = state.internal;
    let target = state.modified_internal;
    let mut remainder = state.internal_remainder;
    if internal != target {
        let cooling = internal > target;
        let difference = if cooling {
            internal.wrapping_sub(target)
        } else {
            target.wrapping_sub(internal)
        } as f32;
        let rate = calls.stat(state, if cooling { 0xa8 } else { 0xaa }) as f32;
        let scaled = difference * f32::from_bits(0x3c23_d70a);
        let change = (rate * delta_seconds) * scaled;
        // MINSS selects its second operand for equal or unordered inputs.
        let change = if change < difference {
            change
        } else {
            difference
        };
        remainder = if cooling {
            remainder - change
        } else {
            remainder + change
        };
    }
    // UCOMISS followed by JZ skips both equality and unordered comparison.
    if remainder == 0.0 || remainder.is_nan() {
        return;
    }
    let truncated = temperature_integer(remainder.trunc());
    let floor = if truncated == i32::MIN || truncated as f32 == remainder {
        remainder
    } else {
        truncated.wrapping_sub(i32::from(remainder.is_sign_negative())) as f32
    };
    state.internal_remainder = remainder - floor;
    let doubled = remainder + remainder;
    let increment = temperature_integer((doubled - 0.5).round_ties_even()) >> 1;
    if increment != 0 {
        calls.set_internal_temperature(state, state.internal.wrapping_add(increment));
    }
}

// CVT[T]SS2SI's masked invalid conversion is i32::MIN, not Rust's
// saturating cast. The caller chooses truncation or nearest-even rounding.
fn temperature_integer(value: f32) -> i32 {
    if (i32::MIN as f32..-(i32::MIN as f32)).contains(&value) {
        value as i32
    } else {
        i32::MIN
    }
}
