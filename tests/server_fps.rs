use icarus_bevy::experimental::server_fps::{MAX_SAMPLES, ServerFpsMeter};

#[test]
fn rings_round_up_to_a_power_of_two_within_bounds() {
    assert_eq!(ServerFpsMeter::new(60).unwrap().capacity(), 64);
    assert_eq!(ServerFpsMeter::new(1).unwrap().capacity(), 1);
    assert_eq!(
        ServerFpsMeter::new(MAX_SAMPLES).unwrap().capacity(),
        MAX_SAMPLES
    );
    assert!(ServerFpsMeter::new(0).is_err());
    assert!(ServerFpsMeter::new(MAX_SAMPLES + 1).is_err());
}

#[test]
fn a_window_of_two_seconds_yields_the_mean_frame_rate() {
    let mut meter = ServerFpsMeter::new(64).unwrap();
    let mut closed = None;
    let mut frames = 0;
    while closed.is_none() {
        assert_eq!(meter.fps(), 0, "no value before the window closes");
        closed = meter.observe(1.0 / 30.0);
        frames += 1;
    }
    // 1/30 s is stored as 333 ten-thousandths: 1 / 0.0333 truncates to 30.
    assert_eq!(closed, Some(30));
    assert_eq!(meter.fps(), 30);
    assert!((60..=61).contains(&frames), "{frames} frames");
}

#[test]
fn older_samples_stay_in_the_ring_until_overwritten() {
    let mut meter = ServerFpsMeter::new(4).unwrap();
    // Two 1 s frames close a window: mean 1 s, 1 frame per second.
    assert_eq!(meter.observe(1.0), None);
    assert_eq!(meter.observe(1.0), Some(1));
    // Twenty 0.1 s frames close the next; the ring keeps only the last four.
    for _ in 0..19 {
        assert_eq!(meter.observe(0.1), None);
    }
    assert_eq!(meter.observe(0.1), Some(10));
    // A 1.5 s frame then a 0.5 s frame: ring 0.1, 0.1, 1.5, 0.5 -> mean 0.55.
    assert_eq!(meter.observe(1.5), None);
    assert_eq!(meter.observe(0.5), Some(1));
}

#[test]
fn non_positive_deltas_are_ignored_and_long_frames_clamped() {
    let mut meter = ServerFpsMeter::new(8).unwrap();
    for delta in [0.0, -1.0, f32::NAN, f32::NEG_INFINITY] {
        assert_eq!(meter.observe(delta), None);
    }
    // A frame longer than 1677 s counts as 1677 s: well below one frame
    // per second.
    assert_eq!(meter.observe(f32::INFINITY), Some(0));
    assert_eq!(meter.observe(5_000.0), Some(0));
}
