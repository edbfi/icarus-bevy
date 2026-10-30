//! Pinned 3.0.32 modifier timer transitions (F-991). These are caller-local
//! rules from M-1126, not a running component, persistence adapter or clock.
//! Calls outside the qualified fragments are supplied explicitly: they may
//! mutate the state before the next local read. A virtual destruction request
//! does not prove destruction; a callback request does not prove its effects.
//!
//! Floating operations assume default nearest-even rounding, masked exceptions
//! and gradual underflow. The original process's floating environment and
//! timer-manager scheduling have not been observed. Authored schema-11 timers
//! keep their separate integer-nanosecond policy.
use std::cmp::Ordering;

/// Raw remaining time and its independently stored wire field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemainingTime {
    /// Component `+0x110`, binary32 seconds, not reconstructed from the wire.
    pub seconds: f32,
    /// Component `+0x3bc`, whole tenths.
    pub replicated_tenths: u16,
}

/// The quantizer inside the pinned setter. `None` means retain the previous
/// wire field, not zero. `CVTSS2SI` overflow/invalid produces i32::MIN, whose
/// negated arithmetic half is above the u16 bound, so it also retains it.
pub fn quantize_remaining_time(seconds: f32) -> Option<u16> {
    let scaled = seconds * 10.0;
    let rounded = (-0.5 - (scaled + scaled)).round_ties_even();
    if !(i32::MIN as f32..-(i32::MIN as f32)).contains(&rounded) {
        return None;
    }
    u16::try_from((-(rounded as i32 >> 1)).max(0)).ok()
}

/// Setter-local writes and whether its tail call to OnRep_RemainingTime runs.
/// The caller must run that callback before continuing the surrounding method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemainingTimeChange {
    pub call_on_rep: bool,
    pub wire_changed: bool,
}

/// `SetRemainingTime` (0x1416f7160): unordered or equal comparisons return,
/// including signed-zero equality. A present owner with byte `+0xf0 == 3`
/// is required. Accepted input writes raw seconds first, conditionally writes
/// the wire field, and requests OnRep even when that field stays unchanged.
/// This stops before the callback; its effects are not part of this result.
pub fn set_remaining_time(
    state: &mut RemainingTime,
    owner_role_byte: Option<u8>,
    requested: f32,
) -> RemainingTimeChange {
    if !matches!(
        requested.partial_cmp(&state.seconds),
        Some(Ordering::Less | Ordering::Greater)
    ) || owner_role_byte != Some(3)
    {
        return RemainingTimeChange::default();
    }
    state.seconds = requested;
    let previous = state.replicated_tenths;
    if let Some(value) = quantize_remaining_time(requested) {
        state.replicated_tenths = value;
    }
    RemainingTimeChange {
        call_on_rep: true,
        wire_changed: state.replicated_tenths != previous,
    }
}

/// Fields read locally by TickComponent after its base call. Owner role byte
/// is the setter's direct owner lookup, distinct from GetOwnerRole's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModifierTickState {
    pub remaining: RemainingTime,
    pub owner_role_byte: Option<u8>,
    /// Byte `+0x264 != 0`.
    pub interval_enabled: bool,
    /// Float `+0x3b8`.
    pub accumulated_seconds: f32,
    /// Float `+0x280`.
    pub interval_seconds: f32,
}

/// External boundaries in TickComponent (0x1416fb670). Implementations own
/// their side effects and return values; no tag, row, Blueprint or owner
/// semantics are inferred here. All calls return normally in this model.
/// Temporary tag containers and their allocation/destruction are omitted.
pub trait ModifierTickCalls {
    /// The initial base call, 0x143474760.
    fn base_tick(&mut self, state: &mut ModifierTickState, delta_seconds: f32);
    /// GetOwnerRole, 0x143468d80. Not substituted with owner_role_byte.
    fn owner_role(&mut self, state: &mut ModifierTickState) -> i32;
    /// 0x1432597e0 on the query at `+0x340`.
    fn query_empty(&mut self, state: &mut ModifierTickState) -> bool;
    /// Owner tag acquisition and 0x143255660 against the query at `+0x340`.
    fn owner_tags_match(&mut self, state: &mut ModifierTickState) -> bool;
    /// Virtual slot `+0x3c0`, with the boolean argument false. No local
    /// destruction, resource cleanup or tick disabling is implied.
    fn request_destroy(&mut self, state: &mut ModifierTickState);
    /// Setter tail call 0x1416ef430, before the interval path's field reads.
    fn on_rep_remaining(&mut self, state: &mut ModifierTickState);
    /// The boolean result of 0x141ca1740.
    fn interval_condition(&mut self, state: &mut ModifierTickState) -> bool;
    /// The boolean result of 0x143dae8a0 on the row at `+0x268`.
    fn interval_row_valid(&mut self, state: &mut ModifierTickState) -> bool;
    /// Row conversion and 0x1414bcf90 on the owner, with both final
    /// arguments zero. Callee behavior and row contents remain opaque.
    fn apply_interval(&mut self, state: &mut ModifierTickState);
}

/// TickComponent's local control flow: base call, optional authority tag
/// refusal, setter, then optional interval path. It does not clamp raw time,
/// destroy at zero, or catch up several interval calls in a single tick.
/// Fields after callbacks are read again, as in the qualified instructions.
pub fn tick_modifier(
    state: &mut ModifierTickState,
    delta_seconds: f32,
    calls: &mut impl ModifierTickCalls,
) {
    calls.base_tick(state, delta_seconds);
    if calls.owner_role(state) == 3 && !calls.query_empty(state) && !calls.owner_tags_match(state) {
        calls.request_destroy(state);
        return;
    }
    let requested = state.remaining.seconds - delta_seconds;
    if set_remaining_time(&mut state.remaining, state.owner_role_byte, requested).call_on_rep {
        calls.on_rep_remaining(state);
    }
    if !state.interval_enabled {
        return;
    }
    let accumulated = delta_seconds + state.accumulated_seconds;
    state.accumulated_seconds = accumulated;
    // COMISS/JC skips both less-than and unordered comparisons.
    if !matches!(
        state
            .accumulated_seconds
            .partial_cmp(&state.interval_seconds),
        Some(Ordering::Equal | Ordering::Greater)
    ) || !calls.interval_condition(state)
    {
        return;
    }
    if calls.interval_row_valid(state) {
        calls.apply_interval(state);
    }
    state.accumulated_seconds -= state.interval_seconds;
}

/// AutoDestroy (0x1416cea30) tests only GetOwnerRole's result and requests
/// virtual slot +0x3c0(false) for role 3. It does not inspect remaining time.
/// The caller owns lookup, dispatch, and all destruction effects.
pub const fn auto_destroy_requested(owner_role_result: i32) -> bool {
    owner_role_result == 3
}
