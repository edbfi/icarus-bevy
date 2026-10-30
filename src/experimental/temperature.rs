//! Pinned internal-temperature setter local control flow (F-997). Bounds and
//! notification effects are caller supplied; this is not a climate model,
//! simulation schedule or persistence adapter. It applies no authority guard.

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
