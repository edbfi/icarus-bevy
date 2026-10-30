//! Authored transition fixtures; no original process or packaged data.
use icarus_bevy::experimental::modifier_timing::{
    ModifierTickCalls, ModifierTickState, RemainingTime, auto_destroy_requested,
    quantize_remaining_time, set_remaining_time, tick_modifier,
};

fn state() -> ModifierTickState {
    ModifierTickState {
        remaining: RemainingTime {
            seconds: 1.0,
            replicated_tenths: 10,
        },
        owner_role_byte: Some(3),
        interval_enabled: true,
        accumulated_seconds: 0.0,
        interval_seconds: 0.5,
    }
}

struct Calls {
    log: Vec<&'static str>,
    role: i32,
    query_empty: bool,
    tags_match: bool,
    condition: bool,
    row_valid: bool,
    base: Option<fn(&mut ModifierTickState)>,
    on_rep: Option<fn(&mut ModifierTickState)>,
    apply: Option<fn(&mut ModifierTickState)>,
}
impl Default for Calls {
    fn default() -> Self {
        Self {
            log: Vec::new(),
            role: 3,
            query_empty: false,
            tags_match: true,
            condition: true,
            row_valid: true,
            base: None,
            on_rep: None,
            apply: None,
        }
    }
}
impl ModifierTickCalls for Calls {
    fn base_tick(&mut self, state: &mut ModifierTickState, _: f32) {
        self.log.push("base");
        if let Some(effect) = self.base {
            effect(state);
        }
    }
    fn owner_role(&mut self, _: &mut ModifierTickState) -> i32 {
        self.log.push("role");
        self.role
    }
    fn query_empty(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("query");
        self.query_empty
    }
    fn owner_tags_match(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("tags");
        self.tags_match
    }
    fn request_destroy(&mut self, _: &mut ModifierTickState) {
        self.log.push("destroy");
    }
    fn on_rep_remaining(&mut self, state: &mut ModifierTickState) {
        self.log.push("on_rep");
        if let Some(effect) = self.on_rep {
            effect(state);
        }
    }
    fn interval_condition(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("condition");
        self.condition
    }
    fn interval_row_valid(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("row");
        self.row_valid
    }
    fn apply_interval(&mut self, state: &mut ModifierTickState) {
        self.log.push("apply");
        if let Some(effect) = self.apply {
            effect(state);
        }
    }
}

#[test]
fn setter_keeps_raw_and_wire_state_separate_and_requests_callbacks() {
    let mut value = state().remaining;
    let unchanged_wire = set_remaining_time(&mut value, Some(3), 0.99);
    assert!(unchanged_wire.call_on_rep);
    assert!(!unchanged_wire.wire_changed);
    assert_eq!(value.seconds, 0.99);
    assert_eq!(value.replicated_tenths, 10);
    let out_of_range = set_remaining_time(&mut value, Some(3), 7000.0);
    assert!(out_of_range.call_on_rep);
    assert!(!out_of_range.wire_changed);
    assert_eq!(value.seconds, 7000.0);
    assert_eq!(value.replicated_tenths, 10);
    let changed = set_remaining_time(&mut value, Some(3), -1.0);
    assert!(changed.call_on_rep && changed.wire_changed);
    assert_eq!(value.seconds, -1.0);
    assert_eq!(value.replicated_tenths, 0);
}

#[test]
fn setter_guards_preserve_bits_and_do_not_request_callbacks() {
    for role in [None, Some(0), Some(1), Some(2), Some(4), Some(255)] {
        let mut value = state().remaining;
        assert_eq!(
            set_remaining_time(&mut value, role, 0.5),
            Default::default()
        );
        assert_eq!(value, state().remaining);
    }
    for (old, requested) in [
        (1.0, 1.0),
        (-0.0, 0.0),
        (0.0, -0.0),
        (1.0, f32::NAN),
        (f32::from_bits(0x7fc0_1234), 1.0),
        (f32::INFINITY, f32::INFINITY),
    ] {
        let mut value = RemainingTime {
            seconds: old,
            replicated_tenths: 71,
        };
        assert_eq!(
            set_remaining_time(&mut value, Some(3), requested),
            Default::default()
        );
        assert_eq!(value.seconds.to_bits(), old.to_bits());
        assert_eq!(value.replicated_tenths, 71);
    }
}

#[test]
fn quantizer_distinguishes_zero_from_failed_conversion() {
    for input in [0.0, -0.0, -1.0, -10_000.0, f32::from_bits(1)] {
        assert_eq!(quantize_remaining_time(input), Some(0));
    }
    for input in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        -f32::MAX,
        6553.6,
    ] {
        assert_eq!(quantize_remaining_time(input), None);
    }
    for (input, expected) in [(0.05, 1), (295.49, 2955), (6553.5, 65535)] {
        assert_eq!(quantize_remaining_time(input), Some(expected));
    }
}

#[test]
fn infinite_requested_value_is_accepted_but_retains_wire_field() {
    let mut value = state().remaining;
    for next in [f32::INFINITY, f32::NEG_INFINITY] {
        let outcome = set_remaining_time(&mut value, Some(3), next);
        assert!(outcome.call_on_rep && !outcome.wire_changed);
        assert_eq!(value.seconds, next);
        assert_eq!(value.replicated_tenths, 10);
    }
}

#[test]
fn authority_tag_refusal_precedes_setter_and_interval() {
    let mut value = state();
    let mut calls = Calls {
        tags_match: false,
        ..Default::default()
    };
    tick_modifier(&mut value, 5.0, &mut calls);
    assert_eq!(calls.log, ["base", "role", "query", "tags", "destroy"]);
    assert_eq!(value, state());
}

#[test]
fn empty_query_skips_tag_lookup_but_does_not_skip_countdown() {
    let mut value = state();
    let mut calls = Calls {
        query_empty: true,
        tags_match: false,
        ..Default::default()
    };
    tick_modifier(&mut value, 0.25, &mut calls);
    assert_eq!(calls.log, ["base", "role", "query", "on_rep"]);
    assert_eq!(value.remaining.seconds, 0.75);
    assert_eq!(value.remaining.replicated_tenths, 8);
    assert_eq!(value.accumulated_seconds, 0.25);
}

#[test]
fn nonauthority_skips_tags_and_setter_but_still_runs_interval() {
    let mut value = state();
    value.owner_role_byte = Some(2);
    let mut calls = Calls {
        role: 2,
        ..Default::default()
    };
    tick_modifier(&mut value, 0.75, &mut calls);
    assert_eq!(calls.log, ["base", "role", "condition", "row", "apply"]);
    assert_eq!(value.remaining, state().remaining);
    assert_eq!(value.accumulated_seconds, 0.25);
}

#[test]
fn owner_role_call_is_distinct_from_setter_owner_byte() {
    let mut value = state();
    let mut calls = Calls {
        role: 2,
        ..Default::default()
    };
    tick_modifier(&mut value, 0.25, &mut calls);
    assert_eq!(calls.log, ["base", "role", "on_rep"]);
    assert_eq!(value.remaining.seconds, 0.75);
}

#[test]
fn zero_crossing_does_not_destroy_and_interval_consumes_at_most_one_period() {
    let mut value = state();
    let mut calls = Calls::default();
    tick_modifier(&mut value, 2.0, &mut calls);
    assert_eq!(
        calls.log,
        [
            "base",
            "role",
            "query",
            "tags",
            "on_rep",
            "condition",
            "row",
            "apply"
        ]
    );
    assert_eq!(value.remaining.seconds, -1.0);
    assert_eq!(value.remaining.replicated_tenths, 0);
    assert_eq!(value.accumulated_seconds, 1.5);
    tick_modifier(&mut value, 0.0, &mut calls);
    assert_eq!(value.accumulated_seconds, 1.0);
    assert_eq!(calls.log.iter().filter(|c| **c == "apply").count(), 2);
    assert!(!calls.log.contains(&"destroy"));
}

#[test]
fn failed_condition_preserves_accumulation_but_invalid_row_consumes_period() {
    for condition in [false, true] {
        let mut value = state();
        let mut calls = Calls {
            condition,
            row_valid: false,
            ..Default::default()
        };
        tick_modifier(&mut value, 0.75, &mut calls);
        assert_eq!(
            value.accumulated_seconds,
            if condition { 0.25 } else { 0.75 }
        );
        assert_eq!(calls.log.contains(&"row"), condition);
        assert!(!calls.log.contains(&"apply"));
    }
}

#[test]
fn interval_unordered_comparisons_skip_condition_and_preserve_accumulator() {
    for (accumulated_seconds, interval_seconds, delta) in [
        (f32::NAN, 1.0, 0.0),
        (0.0, f32::NAN, 0.5),
        (0.0, 1.0, f32::NAN),
    ] {
        let mut value = state();
        value.accumulated_seconds = accumulated_seconds;
        value.interval_seconds = interval_seconds;
        let mut calls = Calls::default();
        tick_modifier(&mut value, delta, &mut calls);
        assert!(!calls.log.contains(&"condition"));
        let expected = delta + accumulated_seconds;
        assert!(
            value.accumulated_seconds == expected
                || (value.accumulated_seconds.is_nan() && expected.is_nan())
        );
    }
}

#[test]
fn callbacks_mutate_state_before_later_reads() {
    let mut value = state();
    let mut calls = Calls {
        base: Some(|s| {
            s.remaining.seconds = 5.0;
        }),
        on_rep: Some(|s| {
            s.interval_enabled = false;
            s.remaining.seconds = 7.0;
        }),
        ..Default::default()
    };
    tick_modifier(&mut value, 0.5, &mut calls);
    assert_eq!(value.remaining.seconds, 7.0);
    assert_eq!(value.remaining.replicated_tenths, 45);
    assert_eq!(value.accumulated_seconds, 0.0);
    assert!(!calls.log.contains(&"condition"));

    let mut value = state();
    let mut calls = Calls {
        apply: Some(|s| {
            s.accumulated_seconds = 20.0;
            s.interval_seconds = 3.0;
        }),
        ..Default::default()
    };
    tick_modifier(&mut value, 0.5, &mut calls);
    assert_eq!(value.accumulated_seconds, 17.0);
}

#[test]
fn expiry_request_depends_only_on_supplied_role() {
    for role in [-1, 0, 1, 2, 3, 4, 255] {
        assert_eq!(auto_destroy_requested(role), role == 3);
    }
}
