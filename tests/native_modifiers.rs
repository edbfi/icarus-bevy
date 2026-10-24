//! Authored owner-state contract only. The lead exports/integrates the module
//! and runs these tests; no original-save or callback behavior is asserted.
use icarus_bevy::{native_lifecycle::OwnerHandle, native_modifiers::*, simulation::DomainId};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn owner(incarnation: u64) -> OwnerHandle {
    OwnerHandle {
        id: DomainId(7),
        incarnation,
    }
}

fn stat(name: &str, amount: i32) -> FlatStatContribution {
    FlatStatContribution {
        stat: name.into(),
        amount,
    }
}

fn definition(name: &str, duration: ModifierDuration, amount: i32) -> ModifierDefinition {
    ModifierDefinition {
        name: name.into(),
        duration,
        contributions: vec![stat("power", amount)],
    }
}

fn request(state: &OwnerModifierState, action: ModifierAction) -> ModifierRequest {
    ModifierRequest {
        owner: state.owner,
        expected_revision: state.revision,
        action,
    }
}

fn add(state: &OwnerModifierState, definition: ModifierDefinition) -> OwnerModifierState {
    prepare_modifiers(state, &request(state, ModifierAction::Add { definition }))
        .unwrap()
        .apply(state)
        .unwrap()
}

fn advance(state: &OwnerModifierState, elapsed_seconds: f32) -> PreparedModifierChange {
    prepare_modifiers(
        state,
        &request(
            state,
            ModifierAction::Advance {
                elapsed_nanos: if elapsed_seconds == f32::MAX {
                    u64::MAX
                } else {
                    (f64::from(elapsed_seconds) * 1_000_000_000.0).round() as u64
                },
            },
        ),
    )
    .unwrap()
}

fn assert_failed_unchanged(state: &OwnerModifierState, request: &ModifierRequest) {
    let before = state.clone();
    let encoded = serde_json::to_vec(state).unwrap();
    assert!(prepare_modifiers(state, request).is_err());
    assert_eq!(*state, before);
    assert_eq!(serde_json::to_vec(state).unwrap(), encoded);
}

fn reject_redacted<T: DeserializeOwned>(value: Value) {
    let error = match serde_json::from_value::<T>(value) {
        Ok(_) => panic!("invalid authored value accepted"),
        Err(error) => error,
    };
    assert!(!format!("{error:?} {error:#}").contains("private_sentinel"));
}

#[test]
fn restored_partial_duration_survives_advance_capture_reload_and_advance() {
    // Deliberately hand-authored current state: restoring the 12-second
    // definition must keep 3.5 seconds, the cached contribution and identity.
    let restored = OwnerModifierState {
        format: NATIVE_MODIFIERS_FORMAT,
        owner: owner(3),
        revision: 19,
        next_id: 20,
        entries: vec![ModifierEntry {
            id: ModifierId::new(11).unwrap(),
            definition: definition("partial", ModifierDuration::Timed { seconds: 12.0 }, 9),
            timer: ModifierTimer::Timed {
                remaining_nanos: 3500000000,
                armed: true,
            },
        }],
        cache: vec![stat("power", 9)],
    };
    let loaded = OwnerModifierState::parse(&serde_json::to_string(&restored).unwrap()).unwrap();
    assert_eq!(loaded, restored);
    let first = advance(&loaded, 1.0).apply(&loaded).unwrap();
    assert_eq!(
        first.entries[0].timer,
        ModifierTimer::Timed {
            remaining_nanos: 2500000000,
            armed: true
        }
    );
    assert_eq!(first.cache, vec![stat("power", 9)]);
    let captured = first.canonical().unwrap();
    let reloaded = OwnerModifierState::parse(&serde_json::to_string(&captured).unwrap()).unwrap();
    assert_eq!(reloaded, first);
    let last = advance(&reloaded, 2.5);
    assert_eq!(last.expired_ids(), &[ModifierId::new(11).unwrap()]);
    let expired = last.apply(&reloaded).unwrap();
    assert!(expired.entries.is_empty());
    assert!(expired.cache.is_empty());
    assert_eq!(expired.revision, 21);
    assert_eq!(expired.next_id, 20);
}

#[test]
fn expiration_removes_exact_contribution_once_and_retains_unrelated_entries() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let timed = add(
        &initial,
        definition("short", ModifierDuration::Timed { seconds: 2.0 }, 8),
    );
    let state = add(&timed, definition("kept", ModifierDuration::Indefinite, -3));
    let kept = state.entries[1].clone();
    assert_eq!(state.cache, vec![stat("power", 5)]);
    let prepared = advance(&state, 2.0);
    assert_eq!(prepared.expired_ids(), &[state.entries[0].id]);
    assert_eq!(state.entries.len(), 2);
    let expired = prepared.apply(&state).unwrap();
    assert_eq!(expired.entries, vec![kept]);
    assert_eq!(expired.cache, vec![stat("power", -3)]);
    let again = advance(&expired, f32::MAX);
    assert!(again.expired_ids().is_empty());
    assert_eq!(again.state(), &expired);
    let after = again.apply(&expired).unwrap();
    assert_failed_unchanged(
        &after,
        &request(
            &after,
            ModifierAction::Remove {
                id: state.entries[0].id,
            },
        ),
    );
}

#[test]
fn indefinite_and_restored_zero_duration_entries_never_age() {
    let zero = OwnerModifierState {
        format: NATIVE_MODIFIERS_FORMAT,
        owner: owner(1),
        revision: 4,
        next_id: 5,
        entries: vec![ModifierEntry {
            id: ModifierId::new(4).unwrap(),
            definition: definition(
                "zero_restored",
                ModifierDuration::Timed { seconds: 50.0 },
                2,
            ),
            timer: ModifierTimer::Timed {
                remaining_nanos: 0,
                armed: false,
            },
        }],
        cache: vec![stat("power", 2)],
    };
    let restored = OwnerModifierState::parse(&serde_json::to_string(&zero).unwrap()).unwrap();
    let state = add(
        &restored,
        definition("forever", ModifierDuration::Indefinite, 4),
    );
    let state = add(
        &state,
        definition("zero_new", ModifierDuration::Timed { seconds: 0.0 }, 1),
    );
    for elapsed in [0.0, 1.0, f32::MAX] {
        let prepared = advance(&state, elapsed);
        assert!(prepared.expired_ids().is_empty());
        assert_eq!(prepared.apply(&state).unwrap(), state);
    }
    assert_eq!(state.cache, vec![stat("power", 7)]);
    let id = state.entries[0].id;
    let removed = prepare_modifiers(&state, &request(&state, ModifierAction::Remove { id }))
        .unwrap()
        .apply(&state)
        .unwrap();
    assert_eq!(removed.cache, vec![stat("power", 5)]);
}

#[test]
fn exact_owner_incarnation_revision_and_complete_apply_prestate_are_checked() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let state = add(
        &initial,
        definition("current", ModifierDuration::Timed { seconds: 5.0 }, 1),
    );
    let action = ModifierAction::Remove {
        id: state.entries[0].id,
    };
    for wrong_owner in [
        owner(2),
        OwnerHandle {
            id: DomainId(8),
            incarnation: 1,
        },
    ] {
        let mut stale = request(&state, action.clone());
        stale.owner = wrong_owner;
        assert_failed_unchanged(&state, &stale);
    }
    let mut stale_revision = request(&state, action.clone());
    stale_revision.expected_revision = 0;
    assert_failed_unchanged(&state, &stale_revision);

    let prepared = prepare_modifiers(&state, &request(&state, action.clone())).unwrap();
    let mut replacement = state.clone();
    replacement.owner = owner(2);
    let before = replacement.clone();
    assert!(prepared.apply(&replacement).is_err());
    assert_eq!(replacement, before);

    let prepared = prepare_modifiers(&state, &request(&state, action.clone())).unwrap();
    let newer = advance(&state, 1.0).apply(&state).unwrap();
    assert!(prepared.apply(&newer).is_err());
    // Even a same-revision, otherwise valid edited pre-state cannot apply.
    let mut edited = state.clone();
    edited.entries[0].timer = ModifierTimer::Timed {
        remaining_nanos: 4000000000,
        armed: true,
    };
    edited.validate().unwrap();
    let prepared = prepare_modifiers(&state, &request(&state, action)).unwrap();
    assert!(prepared.apply(&edited).is_err());
}

#[test]
fn retired_ids_are_not_reused_and_old_incarnations_cannot_target_replacements() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let old = add(
        &initial,
        definition("old", ModifierDuration::Timed { seconds: 1.0 }, 1),
    );
    let stale = request(
        &old,
        ModifierAction::Remove {
            id: old.entries[0].id,
        },
    );
    let expired = advance(&old, 1.0).apply(&old).unwrap();
    let next = add(&expired, definition("new", ModifierDuration::Indefinite, 2));
    assert_ne!(old.entries[0].id, next.entries[0].id);
    assert_failed_unchanged(
        &next,
        &request(
            &next,
            ModifierAction::Remove {
                id: old.entries[0].id,
            },
        ),
    );
    let replacement = add(
        &OwnerModifierState::empty(owner(2)).unwrap(),
        definition("replacement", ModifierDuration::Indefinite, 3),
    );
    // Local IDs and revision coincide, but the incarnation must still reject.
    assert_eq!(old.entries[0].id, replacement.entries[0].id);
    assert_eq!(stale.expected_revision, replacement.revision);
    assert_failed_unchanged(&replacement, &stale);
}

#[test]
fn last_bad_config_duplicate_name_and_nonfinite_elapsed_preserve_prior_state() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let state = add(
        &initial,
        definition("kept", ModifierDuration::Indefinite, 7),
    );
    for seconds in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
        assert_failed_unchanged(
            &state,
            &request(
                &state,
                ModifierAction::Add {
                    definition: definition("bad", ModifierDuration::Timed { seconds }, 2),
                },
            ),
        );
    }
    assert_failed_unchanged(
        &state,
        &request(
            &state,
            ModifierAction::Add {
                definition: definition("kept", ModifierDuration::Indefinite, 0),
            },
        ),
    );
    let mut bad = definition("bad_last", ModifierDuration::Indefinite, 0);
    bad.contributions = (0..MAX_MODIFIER_CONTRIBUTIONS)
        .map(|i| stat(&format!("stat_{i}"), 1))
        .collect();
    bad.contributions.last_mut().unwrap().stat = "stat_0".into();
    assert_failed_unchanged(
        &state,
        &request(&state, ModifierAction::Add { definition: bad }),
    );
}

#[test]
fn last_entry_capacity_failure_preserves_ids_revision_timers_and_cache() {
    let mut state = OwnerModifierState::empty(owner(1)).unwrap();
    for index in 0..MAX_MODIFIERS_PER_OWNER {
        state = add(
            &state,
            definition(&format!("entry_{index}"), ModifierDuration::Indefinite, 1),
        );
    }
    assert_eq!(
        state.cache,
        vec![stat("power", MAX_MODIFIERS_PER_OWNER as i32)]
    );
    assert_failed_unchanged(
        &state,
        &request(
            &state,
            ModifierAction::Add {
                definition: definition("too_many", ModifierDuration::Timed { seconds: 5.0 }, 1),
            },
        ),
    );
    let mut oversized = state.clone();
    oversized.entries.reserve(3 * MAX_MODIFIERS_PER_OWNER);
    assert!(oversized.validate().is_err());
}

#[test]
fn full_signed_contributions_and_order_independent_cancellation_are_supported() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let minimum = add(
        &initial,
        definition("minimum", ModifierDuration::Indefinite, i32::MIN),
    );
    assert_eq!(minimum.cache, vec![stat("power", i32::MIN)]);
    let maximum = add(
        &minimum,
        definition("maximum", ModifierDuration::Indefinite, i32::MAX),
    );
    assert_eq!(maximum.cache, vec![stat("power", -1)]);
    let canceled = add(&maximum, definition("one", ModifierDuration::Indefinite, 1));
    assert_eq!(canceled.cache, vec![stat("power", 0)]);
    // Reordering IDs moves an overflowing partial sum first, but the exact
    // mathematical total remains zero and must still validate.
    let mut reordered = canceled.clone();
    reordered.entries[0].definition = canceled.entries[1].definition.clone();
    reordered.entries[1].definition = canceled.entries[2].definition.clone();
    reordered.entries[2].definition = canceled.entries[0].definition.clone();
    reordered.validate().unwrap();
    assert_eq!(
        OwnerModifierState::parse(&serde_json::to_string(&reordered).unwrap()).unwrap(),
        reordered
    );
    let removed_minimum = request(
        &canceled,
        ModifierAction::Remove {
            id: canceled.entries[0].id,
        },
    );
    assert_failed_unchanged(&canceled, &removed_minimum);
    assert_failed_unchanged(
        &minimum,
        &request(
            &minimum,
            ModifierAction::Add {
                definition: definition("underflow", ModifierDuration::Indefinite, -1),
            },
        ),
    );
    let max_alone = add(
        &initial,
        definition("max_alone", ModifierDuration::Indefinite, i32::MAX),
    );
    assert_failed_unchanged(
        &max_alone,
        &request(
            &max_alone,
            ModifierAction::Add {
                definition: definition("overflow", ModifierDuration::Indefinite, 1),
            },
        ),
    );
}

#[test]
fn expiration_overflow_rejects_the_whole_advance_before_any_timer_changes() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let negative = add(
        &initial,
        definition(
            "temporary_negative",
            ModifierDuration::Timed { seconds: 1.0 },
            -1,
        ),
    );
    let maximum = add(
        &negative,
        definition("maximum", ModifierDuration::Indefinite, i32::MAX),
    );
    let state = add(
        &maximum,
        definition("one", ModifierDuration::Timed { seconds: 10.0 }, 1),
    );
    assert_eq!(state.cache, vec![stat("power", i32::MAX)]);
    assert_failed_unchanged(
        &state,
        &request(
            &state,
            ModifierAction::Advance {
                elapsed_nanos: 1_000_000_000,
            },
        ),
    );
}

#[test]
fn stat_name_capacity_and_last_contribution_limit_fail_atomically() {
    let mut state = OwnerModifierState::empty(owner(1)).unwrap();
    for batch in 0..(MAX_MODIFIER_STATS / MAX_MODIFIER_CONTRIBUTIONS) {
        let mut authored = definition(&format!("batch_{batch}"), ModifierDuration::Indefinite, 0);
        authored.contributions = (0..MAX_MODIFIER_CONTRIBUTIONS)
            .map(|i| {
                stat(
                    &format!("stat_{:03}", batch * MAX_MODIFIER_CONTRIBUTIONS + i),
                    1,
                )
            })
            .collect();
        state = add(&state, authored);
    }
    assert_eq!(state.cache.len(), MAX_MODIFIER_STATS);
    assert_failed_unchanged(
        &state,
        &request(
            &state,
            ModifierAction::Add {
                definition: definition("too_many_stats", ModifierDuration::Indefinite, 1),
            },
        ),
    );
    let mut bad = definition("too_many_contributions", ModifierDuration::Indefinite, 0);
    bad.contributions = (0..=MAX_MODIFIER_CONTRIBUTIONS)
        .map(|i| stat(&format!("field_{i}"), 1))
        .collect();
    assert_failed_unchanged(
        &state,
        &request(&state, ModifierAction::Add { definition: bad }),
    );
}

#[test]
fn revision_and_id_exhaustion_fail_unchanged_but_true_noops_remain_valid() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let mut exhausted = initial.clone();
    exhausted.revision = u64::MAX;
    assert_eq!(
        advance(&exhausted, 1.0).apply(&exhausted).unwrap(),
        exhausted
    );
    assert_failed_unchanged(
        &exhausted,
        &request(
            &exhausted,
            ModifierAction::Add {
                definition: definition("revision_overflow", ModifierDuration::Indefinite, 1),
            },
        ),
    );
    exhausted.revision = 0;
    exhausted.next_id = u64::MAX;
    assert_failed_unchanged(
        &exhausted,
        &request(
            &exhausted,
            ModifierAction::Add {
                definition: definition("id_overflow", ModifierDuration::Indefinite, 1),
            },
        ),
    );
    let mut running = add(
        &initial,
        definition("running", ModifierDuration::Timed { seconds: 1.0 }, 1),
    );
    running.revision = u64::MAX;
    assert_failed_unchanged(
        &running,
        &request(
            &running,
            ModifierAction::Advance {
                elapsed_nanos: 1_000_000_000,
            },
        ),
    );
    assert_failed_unchanged(
        &running,
        &request(
            &running,
            ModifierAction::Remove {
                id: running.entries[0].id,
            },
        ),
    );
}

#[test]
fn strict_serde_rejects_corruption_and_redacts_error_chains_and_debug() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let state = add(
        &initial,
        definition(
            "private_sentinel",
            ModifierDuration::Timed { seconds: 5.0 },
            6,
        ),
    );
    let encoded = serde_json::to_value(&state).unwrap();
    for (path, replacement) in [
        ("/format", json!(2)),
        ("/owner/id", json!(0)),
        ("/owner/incarnation", json!(0)),
        ("/next_id", json!(1)),
        ("/entries/0/id", json!(0)),
        ("/entries/0/timer/remaining_nanos", json!(6_000_000_000u64)),
        ("/entries/0/timer/armed", json!(false)),
        ("/entries/0/timer/type", json!("private_sentinel")),
        ("/entries/0/definition/duration/seconds", json!(-1.0)),
        (
            "/entries/0/definition/contributions/0/amount",
            json!(2147483648_i64),
        ),
        ("/cache/0/amount", json!(5)),
    ] {
        let mut corrupt = encoded.clone();
        *corrupt.pointer_mut(path).unwrap() = replacement;
        reject_redacted::<OwnerModifierState>(corrupt);
    }
    let mut last_bad = encoded.clone();
    last_bad["entries"]
        .as_array_mut()
        .unwrap()
        .push(encoded["entries"][0].clone());
    reject_redacted::<OwnerModifierState>(last_bad);
    let mut unknown = encoded.clone();
    unknown["private_sentinel"] = json!("private_sentinel");
    reject_redacted::<OwnerModifierState>(unknown);
    let mut missing = encoded.clone();
    missing.as_object_mut().unwrap().remove("cache");
    reject_redacted::<OwnerModifierState>(missing);
    reject_redacted::<OwnerModifierState>(json!([1, {}, 0, 1, [], []]));
    let mut missing_arm = encoded.clone();
    missing_arm["entries"][0]["timer"]
        .as_object_mut()
        .unwrap()
        .remove("armed");
    reject_redacted::<OwnerModifierState>(missing_arm);
    reject_redacted::<ModifierRequest>(
        json!({"owner":{"id":7,"incarnation":1},"expected_revision":0,
        "action":{"type":"private_sentinel"}}),
    );

    reject_redacted::<ModifierDuration>(json!({"type":"indefinite", "private_sentinel":1}));
    reject_redacted::<ModifierTimer>(json!({"type":"indefinite", "private_sentinel":1}));
    reject_redacted::<ModifierDuration>(
        json!({"type":"timed", "seconds":1.0, "private_sentinel":1}),
    );
    reject_redacted::<ModifierTimer>(json!({"type":"timed", "remaining_nanos":0, "armed":true}));
    reject_redacted::<ModifierTimer>(
        json!({"type":"timed", "remaining_nanos":1_000_000_000, "armed":false}),
    );

    let req = request(
        &state,
        ModifierAction::Add {
            definition: state.entries[0].definition.clone(),
        },
    );
    let err = prepare_modifiers(&state, &req).unwrap_err();
    assert!(!format!("{err:?} {err:#}").contains("private_sentinel"));
    let prepared = advance(&state, 1.0);
    for output in [
        format!("{state:?}"),
        format!("{:?}", state.entries[0]),
        format!("{:?}", state.entries[0].definition),
        format!("{req:?}"),
        format!("{prepared:?}"),
    ] {
        assert!(!output.contains("private_sentinel"));
    }
    let duplicate_field = r#"{"format":1,"format":1,"owner":{"id":7,"incarnation":1},"revision":0,"next_id":1,"entries":[],"cache":[],"private_sentinel":0}"#;
    let error = OwnerModifierState::parse(duplicate_field).unwrap_err();
    assert!(!format!("{error:?} {error:#}").contains("private_sentinel"));
    assert!(OwnerModifierState::parse(&" ".repeat(MAX_MODIFIER_STATE_BYTES + 1)).is_err());
}

#[test]
fn canonical_requests_and_snapshots_retain_complete_current_state() {
    let initial = OwnerModifierState::empty(owner(1)).unwrap();
    let mut authored = definition("authored", ModifierDuration::Timed { seconds: 8.0 }, 0);
    authored.contributions = vec![stat("z", -2), stat("a", 3)];
    let req = request(
        &initial,
        ModifierAction::Add {
            definition: authored.clone(),
        },
    );
    let mut reversed = req.clone();
    if let ModifierAction::Add { definition } = &mut reversed.action {
        definition.contributions.reverse();
    }
    assert_eq!(req.canonical().unwrap(), reversed.canonical().unwrap());
    let state = add(&initial, authored);
    let advanced = advance(&state, 3.0).apply(&state).unwrap();
    let canonical = advanced.canonical().unwrap();
    assert_eq!(
        canonical.entries[0].timer,
        ModifierTimer::Timed {
            remaining_nanos: 5000000000,
            armed: true
        }
    );
    assert_eq!(
        canonical.entries[0].definition.duration,
        ModifierDuration::Timed { seconds: 8.0 }
    );
    assert_eq!(canonical.cache, vec![stat("a", 3), stat("z", -2)]);
    assert_eq!(canonical, advanced);
    let decoded: ModifierRequest =
        serde_json::from_value(serde_json::to_value(&req).unwrap()).unwrap();
    assert_eq!(decoded, req);
}

#[test]
fn small_integer_steps_survive_large_duration_and_reload_without_rounding_away() {
    let state = add(
        &OwnerModifierState::empty(owner(1)).unwrap(),
        definition(
            "large",
            ModifierDuration::Timed {
                seconds: 16_777_216.0,
            },
            3,
        ),
    );
    let initial = 16_777_216u64 * NANOS_PER_SECOND;
    let mut current = state;
    for _ in 0..4 {
        current = advance(&current, 0.5).apply(&current).unwrap();
        current = OwnerModifierState::parse(&serde_json::to_string(&current).unwrap()).unwrap();
    }
    assert_eq!(
        current.entries[0].timer,
        ModifierTimer::Timed {
            remaining_nanos: initial - 2 * NANOS_PER_SECOND,
            armed: true,
        }
    );
    let request = request(
        &current,
        ModifierAction::Advance {
            elapsed_nanos: initial - 2 * NANOS_PER_SECOND,
        },
    );
    let expired = prepare_modifiers(&current, &request)
        .unwrap()
        .apply(&current)
        .unwrap();
    assert!(expired.entries.is_empty());
    assert!(expired.cache.is_empty());
}

#[test]
fn authored_clock_bounds_reject_subresolution_and_excessive_durations() {
    let state = OwnerModifierState::empty(owner(1)).unwrap();
    for seconds in [
        f32::from_bits(1),
        0.000_000_000_1,
        MAX_MODIFIER_SECONDS * 2.0,
    ] {
        assert_failed_unchanged(
            &state,
            &request(
                &state,
                ModifierAction::Add {
                    definition: definition("bad_clock", ModifierDuration::Timed { seconds }, 1),
                },
            ),
        );
    }
    let accepted = add(
        &state,
        definition(
            "one_ns",
            ModifierDuration::Timed {
                seconds: 0.000_000_001,
            },
            1,
        ),
    );
    assert_eq!(
        accepted.entries[0].timer,
        ModifierTimer::Timed {
            remaining_nanos: 1,
            armed: true
        }
    );
    let change = prepare_modifiers(
        &accepted,
        &request(&accepted, ModifierAction::Advance { elapsed_nanos: 1 }),
    )
    .unwrap();
    assert_eq!(change.expired_ids(), &[accepted.entries[0].id]);
}

#[test]
fn whole_world_entry_budget_is_checked_across_individually_valid_owners() {
    let mut one = OwnerModifierState::empty(owner(1)).unwrap();
    for index in 0..MAX_MODIFIERS_PER_OWNER {
        one = add(
            &one,
            definition(&format!("entry_{index}"), ModifierDuration::Indefinite, 1),
        );
    }
    let mut world = ModifierWorldSnapshot {
        format: 1,
        owners: (1..=64)
            .map(|id| {
                let mut state = one.clone();
                state.owner.id = DomainId(id);
                state
            })
            .collect(),
    };
    world.validate_structure().unwrap();
    one.owner.id = DomainId(65);
    world.owners.push(one);
    assert!(world.validate_structure().is_err());
}

#[test]
fn world_byte_limit_rejects_individually_valid_owners_before_entry_count_exhaustion() {
    let stat_names: Vec<_> = (0..16)
        .map(|index| format!("{index:03}{}", "x".repeat(125)))
        .collect();
    let mut state = OwnerModifierState::empty(owner(1)).unwrap();
    for index in 0..MAX_MODIFIERS_PER_OWNER {
        state = add(
            &state,
            ModifierDefinition {
                name: format!("entry_{index}"),
                duration: ModifierDuration::Indefinite,
                contributions: stat_names
                    .iter()
                    .map(|stat| FlatStatContribution {
                        stat: stat.clone(),
                        amount: 1,
                    })
                    .collect(),
            },
        );
    }
    let mut world = ModifierWorldSnapshot {
        format: 1,
        owners: Vec::new(),
    };
    let mut rejected = false;
    for id in 1..=60 {
        let mut next = state.clone();
        next.owner.id = DomainId(id);
        next.validate().unwrap();
        world.owners.push(next);
        if let Err(error) = world.validate_structure() {
            assert!(error.to_string().contains("allocation limit"));
            assert!(world.owners.len() * MAX_MODIFIERS_PER_OWNER < MAX_WORLD_MODIFIERS);
            rejected = true;
            break;
        }
    }
    assert!(
        rejected,
        "byte admission must fail before the entry allowance is exhausted"
    );
}
