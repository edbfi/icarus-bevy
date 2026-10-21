use icarus_bevy::experimental::ping::PingEstimator;

#[test]
fn the_ping_is_a_quarter_of_the_average_once_a_bucket_closes() {
    let mut ping = PingEstimator::new();
    assert_eq!((ping.ping(), ping.exact_ping_ms()), (0, 0.0));
    // Samples within the first second fill one bucket; nothing is
    // recomputed until a sample arrives a second after it opened.
    ping.update(0.100, 0.0);
    ping.update(0.0609, 0.5);
    assert_eq!(ping.ping(), 0);
    ping.update(0.050, 1.0);
    // (100 + 60) / 2 = 80 ms, a quarter of it 20.
    assert_eq!(ping.exact_ping_ms(), 80.0);
    assert_eq!(ping.ping(), 20);
}

#[test]
fn a_bucket_keeps_seven_samples_and_four_buckets_roll() {
    let mut ping = PingEstimator::new();
    for i in 0..10 {
        // Ten 40 ms samples in one second: only seven count.
        ping.update(0.040, f64::from(i) * 0.05);
    }
    ping.update(0.200, 1.0);
    assert_eq!(ping.exact_ping_ms(), 40.0);
    // Buckets for seconds 1..=4 hold 200 ms each; at second 5 the first
    // bucket (40 ms) is overwritten, so only 200 ms samples remain.
    for second in 2..=5 {
        ping.update(0.200, f64::from(second));
    }
    assert_eq!(ping.exact_ping_ms(), 200.0);
    assert_eq!(ping.ping(), 50);
}

#[test]
fn long_round_trips_clamp_and_the_byte_caps() {
    let mut ping = PingEstimator::new();
    ping.update(5.0, 0.0);
    ping.update(5.0, 1.0);
    // 1.1 s clamps to 1,100 ms; a quarter is 275, capped at 255.
    assert_eq!(ping.exact_ping_ms(), 1_100.0);
    assert_eq!(ping.ping(), 255);
}

#[test]
fn invalid_samples_are_ignored() {
    let mut ping = PingEstimator::new();
    ping.update(-0.5, 0.0);
    ping.update(f32::NAN, 0.2);
    ping.update(0.020, 0.4);
    ping.update(0.020, 1.5);
    assert_eq!(ping.exact_ping_ms(), 20.0);
    assert_eq!(ping.ping(), 5);
}
