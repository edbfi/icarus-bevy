//! Authored setter fixtures: bounds and notification effects are supplied,
//! with no climate data or original runtime.
use icarus_bevy::experimental::temperature::{InternalTemperatureCalls, set_internal_temperature};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Stat { key: u16, observed: i32 },
    Updated { value: i32, observed: i32 },
}

struct Calls {
    upper: i32,
    lower: i32,
    after_upper: Option<i32>,
    after_lower: Option<i32>,
    after_notification: Option<i32>,
    log: Vec<Call>,
}

impl Calls {
    fn bounds(lower: i32, upper: i32) -> Self {
        Self {
            upper,
            lower,
            after_upper: None,
            after_lower: None,
            after_notification: None,
            log: Vec::new(),
        }
    }
}

impl InternalTemperatureCalls for Calls {
    fn stat(&mut self, state: &mut i32, key: u16) -> i32 {
        self.log.push(Call::Stat {
            key,
            observed: *state,
        });
        let (result, effect) = match key {
            0xa2 => (self.upper, self.after_upper),
            0xa0 => (self.lower, self.after_lower),
            _ => panic!("unexpected stat key"),
        };
        if let Some(value) = effect {
            *state = value;
        }
        result
    }

    fn temperature_updated(&mut self, state: &mut i32, value: i32) {
        self.log.push(Call::Updated {
            value,
            observed: *state,
        });
        if let Some(value) = self.after_notification {
            *state = value;
        }
    }
}

#[test]
fn internal_temperature_preserves_ordered_degenerate_and_inverted_bounds() {
    for (lower, upper, requested, expected) in [
        (10, 20, -1, 10),
        (10, 20, 10, 10),
        (10, 20, 15, 15),
        (10, 20, 20, 20),
        (10, 20, 21, 20),
        (10, 10, 9, 10),
        (10, 10, 10, 10),
        (10, 10, 11, 10),
        (20, 10, 9, 20),
        (20, 10, 15, 20),
        (20, 10, 20, 10),
        (20, 10, 21, 10),
        (i32::MIN, i32::MAX, i32::MIN, i32::MIN),
        (i32::MIN, i32::MAX, i32::MAX, i32::MAX),
        (i32::MAX, i32::MIN, i32::MIN, i32::MAX),
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
    ] {
        let mut state = 0;
        let mut calls = Calls::bounds(lower, upper);
        assert!(set_internal_temperature(&mut state, requested, &mut calls));
        assert_eq!(state, expected);
        assert_eq!(
            calls.log,
            [
                Call::Stat {
                    key: 0xa2,
                    observed: 0
                },
                Call::Stat {
                    key: 0xa0,
                    observed: 0
                },
                Call::Updated {
                    value: expected,
                    observed: expected
                },
            ]
        );
    }
}

#[test]
fn internal_temperature_snapshots_before_getters_and_overwrites_their_effects() {
    // The lower getter moves the field to the eventual resolved value. The
    // notification still compares against entry, rather than this mutation.
    let mut state = 15;
    let mut calls = Calls::bounds(10, 20);
    calls.after_upper = Some(17);
    calls.after_lower = Some(20);
    assert!(set_internal_temperature(&mut state, 25, &mut calls));
    assert_eq!(state, 20);
    assert_eq!(
        calls.log,
        [
            Call::Stat {
                key: 0xa2,
                observed: 15
            },
            Call::Stat {
                key: 0xa0,
                observed: 17
            },
            Call::Updated {
                value: 20,
                observed: 20
            },
        ]
    );

    // Conversely, changes made by both getters do not request notification
    // when the stored resolution matches entry. The store must still occur.
    let mut state = 15;
    let mut calls = Calls::bounds(10, 20);
    calls.after_upper = Some(i32::MAX);
    calls.after_lower = Some(i32::MIN);
    assert!(!set_internal_temperature(&mut state, 15, &mut calls));
    assert_eq!(state, 15);
    assert_eq!(
        calls.log,
        [
            Call::Stat {
                key: 0xa2,
                observed: 15
            },
            Call::Stat {
                key: 0xa0,
                observed: i32::MAX
            },
        ]
    );
}

#[test]
fn internal_temperature_notification_receives_stored_resolution_and_keeps_mutations() {
    let mut state = 15;
    let mut calls = Calls::bounds(10, 20);
    calls.after_lower = Some(77);
    calls.after_notification = Some(15);
    assert!(set_internal_temperature(&mut state, 25, &mut calls));
    assert_eq!(
        state, 15,
        "notification can restore entry without changing the result"
    );
    assert_eq!(
        calls.log,
        [
            Call::Stat {
                key: 0xa2,
                observed: 15
            },
            Call::Stat {
                key: 0xa0,
                observed: 15
            },
            Call::Updated {
                value: 20,
                observed: 20
            },
        ]
    );
}
