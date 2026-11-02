use icarus_bevy::experimental::time_of_day::{
    FALLBACK_SECONDS_PER_GAME_DAY, MIN_TIME_SCALE, MINUTES_PER_DAY, RateCurve, TimeOfDayClock,
    seconds_per_game_day,
};

#[test]
fn without_a_curve_a_day_lasts_1440_seconds() {
    assert_eq!(seconds_per_game_day(None), 1440);
}

#[test]
fn a_constant_curve_reproduces_its_seconds_per_game_day() {
    let mut mismatches = Vec::new();
    for seconds in (1..=86_400).step_by(7).chain([3_960, 4_060, 86_400]) {
        let curve = RateCurve::for_seconds_per_game_day(seconds).expect("curve");
        let derived = seconds_per_game_day(Some(&curve));
        if derived != seconds {
            mismatches.push((seconds, derived));
        }
    }
    // Truncation may lose one second where 24 rounded hours sum just below.
    assert!(
        mismatches
            .iter()
            .all(|(seconds, derived)| seconds - derived == 1),
        "{mismatches:?}"
    );
}

#[test]
fn seconds_per_game_day_averages_each_hour_with_the_next() {
    // Hourly samples: rate 1 for hours 0..=12, 2 for 13..=23. Hours 0..=11
    // last 60 s each; hour 12 averages 1 and 2 (40 s); 13..=22 last 30 s
    // each; hour 23 averages 2 with hour 0's 1 (40 s).
    let curve =
        RateCurve::new(vec![(0.0, 1.0), (12.0, 1.0), (12.5, 2.0), (24.0, 2.0)]).expect("curve");
    assert_eq!(
        seconds_per_game_day(Some(&curve)),
        12 * 60 + 40 + 10 * 30 + 40
    );
}

#[test]
fn curves_interpolate_linearly_and_hold_their_ends() {
    let curve = RateCurve::new(vec![(6.0, 0.2), (18.0, 0.4)]).expect("curve");
    assert_eq!(curve.value(0.0), 0.2);
    assert_eq!(curve.value(6.0), 0.2);
    assert!((curve.value(12.0) - 0.3).abs() < 1e-6);
    assert_eq!(curve.value(18.0), 0.4);
    assert_eq!(curve.value(23.5), 0.4);
    assert_eq!(curve.value(f32::NAN), 0.2);
}

#[test]
fn invalid_curves_are_rejected() {
    for keys in [
        vec![],
        vec![(0.0, 0.0)],
        vec![(0.0, -1.0)],
        vec![(0.0, f32::NAN)],
        vec![(0.0, f32::INFINITY)],
        vec![(-1.0, 1.0)],
        vec![(24.5, 1.0)],
        vec![(f32::NAN, 1.0)],
        vec![(3.0, 1.0), (3.0, 2.0)],
        vec![(4.0, 1.0), (3.0, 2.0)],
        vec![(0.0, 1.0); 49],
    ] {
        assert!(RateCurve::new(keys.clone()).is_err(), "{keys:?}");
    }
    assert!(RateCurve::for_seconds_per_game_day(0).is_err());
    assert!(RateCurve::for_seconds_per_game_day(-60).is_err());
}

#[test]
fn the_clock_advances_by_rate_scale_and_delta() {
    let curve = RateCurve::constant(0.25).expect("curve");
    let mut clock = TimeOfDayClock::new(curve, 600.0);
    assert!(clock.advance(1.0));
    assert_eq!(clock.time_of_day(), 600.25);
    clock.set_time_scale(4.0);
    assert!(clock.advance(0.5));
    assert_eq!(clock.time_of_day(), 600.75);
    // Negative and NaN deltas advance nothing.
    clock.advance(-3.0);
    clock.advance(f32::NAN);
    assert_eq!(clock.time_of_day(), 600.75);
}

#[test]
fn the_rate_follows_the_hour_of_the_current_time() {
    let curve = RateCurve::new(vec![(0.0, 1.0), (12.0, 1.0), (12.0001, 0.5)]).expect("curve");
    let mut morning = TimeOfDayClock::new(curve.clone(), 60.0);
    morning.advance(2.0);
    assert_eq!(morning.time_of_day(), 62.0);
    let mut afternoon = TimeOfDayClock::new(curve, 13.0 * 60.0);
    afternoon.advance(2.0);
    assert_eq!(afternoon.time_of_day(), 13.0 * 60.0 + 1.0);
}

#[test]
fn a_minimum_step_holds_the_time_until_it_accumulates() {
    let curve = RateCurve::constant(1.0).expect("curve");
    let mut clock = TimeOfDayClock::new(curve, 100.0);
    // 30 game seconds: half a game minute.
    clock.set_minimum_time_step_seconds(30.0);
    assert!(!clock.advance(0.25));
    assert_eq!(clock.time_of_day(), 100.0);
    assert!(clock.advance(0.25));
    assert_eq!(clock.time_of_day(), 100.5);
    // Clearing the step commits every tick again.
    clock.set_minimum_time_step_seconds(-1.0);
    assert!(clock.advance(0.0));
}

#[test]
fn the_day_wraps_at_1440_minutes() {
    let curve = RateCurve::constant(1.0).expect("curve");
    let mut clock = TimeOfDayClock::new(curve, 1439.5);
    assert_eq!(clock.days(), 0);
    clock.advance(1.0);
    assert_eq!(clock.time_of_day(), 0.5);
    assert_eq!(clock.days(), 1);
    clock.advance(1.0);
    assert_eq!(clock.days(), 1);
}

#[test]
fn set_time_of_day_wraps_and_clamps() {
    let curve = RateCurve::constant(1.0).expect("curve");
    let mut clock = TimeOfDayClock::new(curve, 1500.0);
    assert_eq!(clock.time_of_day(), 60.0);
    clock.set_time_of_day(3.0 * MINUTES_PER_DAY + 10.0);
    assert_eq!(clock.time_of_day(), 10.0);
    assert_eq!(clock.days(), 1);
    clock.set_time_of_day(-5.0);
    assert_eq!(clock.time_of_day(), 0.0);
    for value in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1.0e30, f32::MAX] {
        clock.set_time_of_day(value);
        let time = clock.time_of_day();
        assert!((0.0..MINUTES_PER_DAY).contains(&time), "{value} -> {time}");
    }
}

#[test]
fn the_time_scale_keeps_a_floor() {
    let curve = RateCurve::constant(1.0).expect("curve");
    let mut clock = TimeOfDayClock::new(curve, 0.0);
    for scale in [0.0, -2.0, f32::NAN] {
        clock.set_time_scale(scale);
        assert_eq!(clock.time_scale(), MIN_TIME_SCALE);
    }
    clock.set_time_scale(2.0);
    assert_eq!(clock.time_scale(), 2.0);
    assert_eq!(clock.seconds_per_game_day(), 1440);
    assert_ne!(FALLBACK_SECONDS_PER_GAME_DAY, clock.seconds_per_game_day());
}
