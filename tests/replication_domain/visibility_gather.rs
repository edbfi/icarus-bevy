//! Authored names and opaque tokens only; no recovered map or live bindings.
use std::collections::{BTreeMap, BTreeSet};

use icarus_bevy::experimental::visibility_gather::{
    GatherBindings, GatherError, GatherRecord, GatherState, Limits,
};
use icarus_bevy::experimental::visibility_state::{
    AddAdmission, Assumption, NameIdentity, ObjectIdentity, ReceiverProfile, VisibilityChange,
    VisibilityFacts, VisibilityState,
};

fn name(index: u32, number: u32) -> NameIdentity {
    NameIdentity::caller_qualified(index, number)
}

fn object(token: u64) -> ObjectIdentity {
    ObjectIdentity::caller_qualified(token)
}

fn bindings() -> GatherBindings {
    GatherBindings {
        graph_map_owner: Assumption::Known(object(1)),
        supplied_names_source: Assumption::Known(object(2)),
        cache_lifetime: Assumption::Known(object(3)),
    }
}

fn limits() -> Limits {
    Limits {
        records: 8,
        supplied_names: 8,
        output_references: 8,
    }
}

fn state() -> GatherState {
    GatherState::new(limits(), bindings(), Assumption::Known(name(0, 0))).unwrap()
}

fn record(index: u32, number: u32, count: i32, token: u64) -> GatherRecord {
    GatherRecord {
        name: name(index, number),
        count,
        list_reference: object(token),
    }
}

#[test]
fn full_cached_name_bypasses_membership_but_number_mismatch_does_not() {
    let mut state = state();
    state
        .gather(
            bindings(),
            &[record(0, 1, 1, 10), record(0, 0, 1, 11)],
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(state.output(), &[object(11)]);
    assert_eq!(state.last_name(), name(0, 0));
}

#[test]
fn accepted_search_updates_cache_and_rejected_search_preserves_it() {
    let mut state = state();
    state
        .gather(
            bindings(),
            &[record(7, 2, 1, 10)],
            &BTreeSet::from([name(7, 2)]),
        )
        .unwrap();
    state
        .gather(
            bindings(),
            &[record(8, 2, 1, 11), record(7, 2, 1, 12)],
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(state.last_name(), name(7, 2));
    assert_eq!(state.output(), &[object(10), object(12)]);
}

#[test]
fn signed_counts_skip_before_cache_and_never_expand_reference_counts() {
    let mut state = state();
    state
        .gather(
            bindings(),
            &[
                record(1, 0, i32::MIN, 10),
                record(2, 0, -1, 11),
                record(3, 0, 0, 12),
                record(4, 0, i32::MAX, 13),
            ],
            &BTreeSet::from([name(1, 0), name(2, 0), name(3, 0), name(4, 0)]),
        )
        .unwrap();
    assert_eq!(state.output(), &[object(13)]);
    assert_eq!(state.last_name(), name(4, 0));
}

#[test]
fn supplied_traversal_and_duplicate_references_are_preserved() {
    let mut state = state();
    state
        .gather(
            bindings(),
            &[
                record(2, 0, 1, 12),
                record(1, 0, 1, 11),
                record(2, 0, 1, 12),
            ],
            &BTreeSet::from([name(1, 0), name(2, 0)]),
        )
        .unwrap();
    assert_eq!(state.output(), &[object(12), object(11), object(12)]);
    assert_eq!(state.last_name(), name(2, 0));
}

#[test]
fn unknown_bindings_and_initial_name_never_construct_state() {
    for (unknown, error) in unknown_bindings() {
        assert_eq!(
            GatherState::new(limits(), unknown, Assumption::Known(name(0, 0))),
            Err(error)
        );
    }
    assert_eq!(
        GatherState::new(limits(), bindings(), Assumption::Unknown),
        Err(GatherError::UnknownInitialName)
    );
}

fn unknown_bindings() -> [(GatherBindings, GatherError); 3] {
    [
        (
            GatherBindings {
                graph_map_owner: Assumption::Unknown,
                ..bindings()
            },
            GatherError::UnknownMapOwner,
        ),
        (
            GatherBindings {
                supplied_names_source: Assumption::Unknown,
                ..bindings()
            },
            GatherError::UnknownNamesSource,
        ),
        (
            GatherBindings {
                cache_lifetime: Assumption::Unknown,
                ..bindings()
            },
            GatherError::UnknownCacheLifetime,
        ),
    ]
}

#[test]
fn unknown_or_changed_domains_reject_even_empty_calls_without_mutation() {
    let mut state = state();
    let before = state.clone();
    for (unknown, error) in unknown_bindings() {
        assert_eq!(state.gather(unknown, &[], &BTreeSet::new()), Err(error));
        assert_eq!(state, before);
    }
    for changed in [
        GatherBindings {
            graph_map_owner: Assumption::Known(object(90)),
            ..bindings()
        },
        GatherBindings {
            supplied_names_source: Assumption::Known(object(90)),
            ..bindings()
        },
        GatherBindings {
            cache_lifetime: Assumption::Known(object(90)),
            ..bindings()
        },
    ] {
        assert_eq!(
            state.gather(changed, &[], &BTreeSet::new()),
            Err(GatherError::BindingMismatch)
        );
        assert_eq!(state, before);
    }
}

#[test]
fn input_limits_reject_before_any_cache_or_output_publication() {
    for (limits, records, names, error) in [
        (
            Limits {
                records: 0,
                ..limits()
            },
            vec![record(1, 0, 1, 10)],
            BTreeSet::from([name(1, 0)]),
            GatherError::RecordLimit,
        ),
        (
            Limits {
                supplied_names: 0,
                ..limits()
            },
            vec![],
            BTreeSet::from([name(1, 0)]),
            GatherError::SuppliedNameLimit,
        ),
    ] {
        let mut state =
            GatherState::new(limits, bindings(), Assumption::Known(name(0, 0))).unwrap();
        let before = state.clone();
        assert_eq!(state.gather(bindings(), &records, &names), Err(error));
        assert_eq!(state, before);
    }
}

#[test]
fn late_output_limit_rolls_back_prior_appends_and_cache_changes() {
    let mut state = GatherState::new(
        Limits {
            output_references: 2,
            ..limits()
        },
        bindings(),
        Assumption::Known(name(0, 0)),
    )
    .unwrap();
    state
        .gather(bindings(), &[record(0, 0, 1, 10)], &BTreeSet::new())
        .unwrap();
    let before = state.clone();
    assert_eq!(
        state.gather(
            bindings(),
            &[record(1, 0, 1, 11), record(2, 0, 1, 12)],
            &BTreeSet::from([name(1, 0), name(2, 0)])
        ),
        Err(GatherError::OutputLimit)
    );
    assert_eq!(state, before);
    state
        .gather(
            bindings(),
            &[record(1, 0, 1, 11)],
            &BTreeSet::from([name(1, 0)]),
        )
        .unwrap();
    assert_eq!(state.output(), &[object(10), object(11)]);
}

#[test]
fn full_output_accepts_empty_and_ineligible_inputs_and_clear_keeps_cache() {
    let mut state = GatherState::new(
        Limits {
            output_references: 1,
            ..limits()
        },
        bindings(),
        Assumption::Known(name(0, 0)),
    )
    .unwrap();
    state
        .gather(
            bindings(),
            &[record(1, 0, 1, 10)],
            &BTreeSet::from([name(1, 0)]),
        )
        .unwrap();
    state.gather(bindings(), &[], &BTreeSet::new()).unwrap();
    state
        .gather(
            bindings(),
            &[record(1, 0, 0, 11), record(2, 0, 1, 12)],
            &BTreeSet::new(),
        )
        .unwrap();
    let before = state.clone();
    assert_eq!(
        state.gather(bindings(), &[record(1, 0, 1, 13)], &BTreeSet::new()),
        Err(GatherError::OutputLimit)
    );
    assert_eq!(state, before);
    state.clear_output();
    assert_eq!(state.last_name(), name(1, 0));
    state
        .gather(bindings(), &[record(1, 0, 1, 13)], &BTreeSet::new())
        .unwrap();
    assert_eq!(state.output(), &[object(13)]);
}

#[test]
fn zero_limits_allow_only_empty_inputs_or_skipped_records_within_record_limit() {
    let mut state = GatherState::new(
        Limits {
            records: 1,
            supplied_names: 0,
            output_references: 0,
        },
        bindings(),
        Assumption::Known(name(0, 0)),
    )
    .unwrap();
    state.gather(bindings(), &[], &BTreeSet::new()).unwrap();
    state
        .gather(bindings(), &[record(0, 0, 0, 10)], &BTreeSet::new())
        .unwrap();
    assert_eq!(
        state.gather(bindings(), &[record(0, 0, 1, 10)], &BTreeSet::new()),
        Err(GatherError::OutputLimit)
    );
    assert!(state.output().is_empty());
}

#[test]
fn visibility_callbacks_never_supply_gather_names_or_invalidate_cached_name() {
    let package = name(7, 2);
    let mut visibility =
        VisibilityState::new(icarus_bevy::experimental::visibility_state::Limits {
            visible_names: 8,
            cache_entries: 8,
            events: 32,
            channel_descriptors: 8,
        });
    let objects = BTreeMap::new();
    let facts = VisibilityFacts {
        objects: &objects,
        persistent_world_names: Assumption::Unknown,
    };
    let receiver = ReceiverProfile::SelectedGraph(object(40));
    visibility
        .apply(
            VisibilityChange::Add {
                name: package,
                admission: AddAdmission::Accepted {
                    package_world: Assumption::Known(None),
                    graph_named_entry: Assumption::Known(true),
                },
            },
            receiver,
            &facts,
        )
        .unwrap();
    assert!(visibility.contains_name(package));
    let mut gather = state();
    let records = [record(7, 2, 1, 10)];
    gather
        .gather(bindings(), &records, &BTreeSet::new())
        .unwrap();
    assert!(gather.output().is_empty());
    // Explicit caller input, rather than callback intentions, enables search.
    gather
        .gather(bindings(), &records, &BTreeSet::from([package]))
        .unwrap();
    visibility
        .apply(
            VisibilityChange::Hide {
                name: package,
                channels: Assumption::Known(&[]),
            },
            receiver,
            &facts,
        )
        .unwrap();
    assert!(!visibility.contains_name(package));
    gather.clear_output();
    gather
        .gather(bindings(), &records, &BTreeSet::new())
        .unwrap();
    assert_eq!(gather.output(), &[object(10)]);
    // Caller must assert a new cache lifetime and initialization to reset it.
    let fresh_bindings = GatherBindings {
        cache_lifetime: Assumption::Known(object(99)),
        ..bindings()
    };
    let mut fresh =
        GatherState::new(limits(), fresh_bindings, Assumption::Known(name(0, 0))).unwrap();
    fresh
        .gather(fresh_bindings, &records, &BTreeSet::new())
        .unwrap();
    assert!(fresh.output().is_empty());
}
