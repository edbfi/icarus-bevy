//! Authored tick fixtures; callbacks supply all environmental and stat effects.
use icarus_bevy::experimental::temperature::{
    TemperatureTickCalls, TemperatureTickState, tick_temperature,
};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Stat(u16),
    Cache,
    Sample,
    External(i32),
    Internal(i32),
}

struct Calls {
    enabled: i32,
    sampled: i32,
    external_offset: i32,
    internal_offset: i32,
    rate: i32,
    effect: Option<fn(&mut TemperatureTickState, u16)>,
    notify_effect: Option<fn(&mut TemperatureTickState)>,
    cache_effect: Option<fn(&mut TemperatureTickState)>,
    log: Vec<Call>,
}

impl Default for Calls {
    fn default() -> Self {
        Self {
            enabled: 1,
            sampled: 20,
            external_offset: 0,
            internal_offset: 0,
            rate: 100,
            effect: None,
            notify_effect: None,
            cache_effect: None,
            log: Vec::new(),
        }
    }
}

impl TemperatureTickCalls for Calls {
    fn stat(&mut self, state: &mut TemperatureTickState, key: u16) -> i32 {
        self.log.push(Call::Stat(key));
        if let Some(effect) = self.effect {
            effect(state, key);
        }
        match key {
            0x9d => self.enabled,
            0x3c3 => self.external_offset,
            0x3c5 => self.internal_offset,
            0xa8 | 0xaa => self.rate,
            _ => panic!("unexpected stat key {key:x}"),
        }
    }

    fn cache_temperature_singleton(&mut self, state: &mut TemperatureTickState) {
        self.log.push(Call::Cache);
        state.singleton_cached = true;
        if let Some(effect) = self.cache_effect {
            effect(state);
        }
    }

    fn environmental_temperature(&mut self, state: &mut TemperatureTickState) -> i32 {
        self.log.push(Call::Sample);
        // The sample stores overwrite these callee mutations.
        state.external = -900;
        state.modified_external = -901;
        self.sampled
    }

    fn external_temperature_updated(&mut self, state: &mut TemperatureTickState, value: i32) {
        self.log.push(Call::External(value));
        assert_eq!(state.modified_external, value);
        if let Some(effect) = self.notify_effect {
            effect(state);
        }
    }

    fn set_internal_temperature(&mut self, state: &mut TemperatureTickState, requested: i32) {
        self.log.push(Call::Internal(requested));
        state.internal = requested;
    }
}

fn state() -> TemperatureTickState {
    TemperatureTickState {
        required_reference_present: true,
        singleton_cached: true,
        sample_interval_seconds: 1.0,
        sample_elapsed_seconds: 0.0,
        internal_remainder: 0.0,
        external: 20,
        modified_external: 20,
        modified_internal: 20,
        internal: 20,
    }
}

#[test]
fn owner_and_enable_guards_precede_clock_cache_and_sampling() {
    let mut value = state();
    value.required_reference_present = false;
    value.singleton_cached = false;
    let mut calls = Calls::default();
    tick_temperature(&mut value, 4.0, &mut calls);
    assert!(calls.log.is_empty());
    assert_eq!(value.sample_elapsed_seconds, 0.0);
    for enabled in [i32::MIN, -1, 0] {
        value.required_reference_present = true;
        let mut calls = Calls {
            enabled,
            ..Default::default()
        };
        tick_temperature(&mut value, 4.0, &mut calls);
        assert_eq!(calls.log, [Call::Stat(0x9d)]);
        assert!(!value.singleton_cached);
        assert_eq!(value.sample_elapsed_seconds, 0.0);
    }
}

#[test]
fn sample_cadence_is_strict_and_consumes_only_one_interval() {
    let mut value = state();
    value.sample_elapsed_seconds = 0.5;
    let mut calls = Calls::default();
    tick_temperature(&mut value, 0.5, &mut calls);
    assert_eq!(value.sample_elapsed_seconds, 1.0);
    assert_eq!(calls.log, [Call::Stat(0x9d)]);
    calls.log.clear();
    tick_temperature(&mut value, 3.0, &mut calls);
    assert_eq!(value.sample_elapsed_seconds, 3.0);
    assert_eq!(
        calls.log,
        [
            Call::Stat(0x9d),
            Call::Sample,
            Call::Stat(0x3c3),
            Call::Stat(0x3c5)
        ]
    );
    calls.log.clear();
    value.sample_elapsed_seconds = f32::NAN;
    tick_temperature(&mut value, 1.0, &mut calls);
    assert!(value.sample_elapsed_seconds.is_nan());
    assert_eq!(calls.log, [Call::Stat(0x9d)]);
}

#[test]
fn singleton_lookup_effects_precede_elapsed_accumulation() {
    let mut value = state();
    value.singleton_cached = false;
    let mut calls = Calls {
        cache_effect: Some(|state| {
            state.sample_elapsed_seconds = 4.0;
            state.sample_interval_seconds = 2.0;
        }),
        ..Default::default()
    };
    tick_temperature(&mut value, 1.0, &mut calls);
    assert_eq!(value.sample_elapsed_seconds, 3.0);
    assert_eq!(
        calls.log,
        [
            Call::Stat(0x9d),
            Call::Cache,
            Call::Sample,
            Call::Stat(0x3c3),
            Call::Stat(0x3c5)
        ]
    );
    calls.log.clear();
    tick_temperature(&mut value, 0.0, &mut calls);
    assert!(!calls.log.contains(&Call::Cache));
}

#[test]
fn sampled_offsets_reread_state_after_each_external_call() {
    let mut value = state();
    value.modified_external = 10;
    value.internal = 53;
    let mut calls = Calls {
        external_offset: 2,
        internal_offset: 3,
        effect: Some(|state, key| match key {
            0x3c3 => state.modified_external = 30,
            0x3c5 => {
                assert_eq!(state.modified_external, 40);
                state.modified_external = 50;
            }
            _ => {}
        }),
        notify_effect: Some(|state| state.modified_external = 40),
        ..Default::default()
    };
    tick_temperature(&mut value, 2.0, &mut calls);
    assert_eq!(
        (
            value.external,
            value.modified_external,
            value.modified_internal
        ),
        (20, 50, 53)
    );
    assert_eq!(
        calls.log,
        [
            Call::Stat(0x9d),
            Call::Sample,
            Call::Stat(0x3c3),
            Call::External(32),
            Call::Stat(0x3c5)
        ]
    );
}

#[test]
fn rate_call_uses_captured_difference_and_remainder_but_current_internal_for_setter() {
    for (internal, target, key, requested) in [(100, 0, 0xa8, 950), (0, 100, 0xaa, 1050)] {
        let mut value = state();
        value.internal = internal;
        value.modified_internal = target;
        value.internal_remainder = 0.25;
        let mut calls = Calls {
            effect: Some(|state, key| {
                if matches!(key, 0xa8 | 0xaa) {
                    state.internal = 1000;
                    state.modified_internal = 9999;
                    state.internal_remainder = 88.0;
                }
            }),
            ..Default::default()
        };
        tick_temperature(&mut value, 0.5, &mut calls);
        assert_eq!(value.internal, requested);
        assert_eq!(value.internal_remainder, 0.25);
        assert_eq!(
            calls.log,
            [Call::Stat(0x9d), Call::Stat(key), Call::Internal(requested)]
        );
    }
}

#[test]
fn remainder_conversion_preserves_native_negative_and_overflow_boundaries() {
    for (remainder, residue, increment) in [
        (0.25_f32, 0.25_f32, 0),
        (1.25, 0.25, 1),
        (-0.25, 0.75, -1),
        (-0.75, 0.25, -1),
        (-1.25, 0.75, -2),
        // Integer conversion of the doubled value overflows, rather than saturating.
        (1_073_741_824.0, 0.0, -1_073_741_824),
    ] {
        let mut value = state();
        value.internal_remainder = remainder;
        let mut calls = Calls::default();
        tick_temperature(&mut value, 0.0, &mut calls);
        assert_eq!(value.internal_remainder.to_bits(), residue.to_bits());
        assert_eq!(value.internal, 20_i32.wrapping_add(increment));
        assert_eq!(
            calls.log.contains(&Call::Internal(value.internal)),
            increment != 0
        );
    }
    for remainder in [0.0_f32, -0.0, f32::from_bits(0x7fc0_1234)] {
        let mut value = state();
        value.internal_remainder = remainder;
        let mut calls = Calls::default();
        tick_temperature(&mut value, 0.0, &mut calls);
        assert_eq!(value.internal_remainder.to_bits(), remainder.to_bits());
        assert_eq!(calls.log, [Call::Stat(0x9d)]);
    }
}

#[test]
fn unordered_rate_selects_difference_and_still_updates_internal() {
    let mut value = state();
    value.internal = 45;
    value.internal_remainder = 0.25;
    let mut calls = Calls::default();
    tick_temperature(&mut value, f32::NAN, &mut calls);
    assert!(value.sample_elapsed_seconds.is_nan());
    assert_eq!(value.internal, 20);
    assert_eq!(value.internal_remainder, 0.25);
    assert_eq!(
        calls.log,
        [Call::Stat(0x9d), Call::Stat(0xa8), Call::Internal(20)]
    );
}

#[test]
fn exact_cancellation_skips_remainder_store_even_after_rate_callback_mutation() {
    let mut value = state();
    value.internal = 21;
    value.internal_remainder = 0.5;
    let mut calls = Calls {
        effect: Some(|state, key| {
            if key == 0xa8 {
                state.internal_remainder = 77.0;
            }
        }),
        ..Default::default()
    };
    tick_temperature(&mut value, 0.5, &mut calls);
    assert_eq!(value.internal_remainder, 77.0);
    assert_eq!(value.internal, 21);
    assert_eq!(calls.log, [Call::Stat(0x9d), Call::Stat(0xa8)]);
}
