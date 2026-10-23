use icarus_bevy::experimental::inventory_weight::InventoryWeightState;

#[test]
fn constructor_keeps_the_seed_clean_and_ticks_do_not_calculate() {
    for seed in [0, -17, i32::MIN, i32::MAX] {
        let mut state = InventoryWeightState::new(seed);
        assert_eq!(state.cached_weight(), seed);
        assert!(!state.is_dirty());
        for role in [None, Some(0), Some(3), Some(4), Some(u8::MAX)] {
            assert_eq!(
                state.tick(role, || panic!("clean cache recalculated")),
                None
            );
            assert_eq!(state.cached_weight(), seed);
            assert!(!state.is_dirty());
        }
    }
}

#[test]
fn missing_owner_and_low_roles_preserve_pending_recalculation_and_cache() {
    let mut state = InventoryWeightState::new(-123);
    state.mark_dirty();
    state.mark_dirty();
    for role in [None, Some(0), Some(1), Some(2)] {
        assert_eq!(
            state.tick(role, || panic!("ineligible tick calculated")),
            None
        );
        assert!(state.is_dirty());
        assert_eq!(state.cached_weight(), -123);
    }
    assert_eq!(state.tick(Some(3), || 19), Some(19));
    assert_eq!(state.cached_weight(), 19);
    assert!(!state.is_dirty());
}

#[test]
fn eligible_roles_recalculate_once_per_dirty_period() {
    for role in [3, 4, 128, u8::MAX] {
        let mut state = InventoryWeightState::new(91);
        state.mark_dirty();
        let mut calls = 0;
        // Moving this String into the closure also exercises an FnOnce-only
        // calculation; no repeat-call capability is required.
        let signed = String::from("-47");
        assert_eq!(
            state.tick(Some(role), move || {
                let weight = signed.parse().unwrap();
                drop(signed);
                weight
            }),
            Some(-47)
        );
        assert_eq!(state.cached_weight(), -47);
        assert!(!state.is_dirty());
        for _ in 0..2 {
            assert_eq!(
                state.tick(Some(role), || panic!("clean tick calculated")),
                None
            );
        }
        state.mark_dirty();
        assert_eq!(
            state.tick(Some(role), || {
                calls += 1;
                0
            }),
            Some(0)
        );
        assert_eq!(calls, 1);
        assert_eq!(state.cached_weight(), 0);
        assert!(!state.is_dirty());
    }
}
