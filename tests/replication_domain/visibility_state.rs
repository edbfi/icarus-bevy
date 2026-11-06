//! Authored identities/descriptors only; no wire-to-interned-name mapping.
use std::collections::BTreeMap;

use icarus_bevy::experimental::visibility_state::{
    AddAdmission, Assumption, ChannelDescriptor, Event, Limits, ModelError, NameIdentity,
    ObjectDescriptor, ObjectIdentity, ReceiverProfile, RejectedClosePolicy, VisibilityChange,
    VisibilityFacts, VisibilityState,
};

fn name(index: u32, number: u32) -> NameIdentity {
    NameIdentity::caller_qualified(index, number)
}

fn object(token: u64) -> ObjectIdentity {
    ObjectIdentity::caller_qualified(token)
}

fn limits() -> Limits {
    Limits {
        visible_names: 8,
        cache_entries: 8,
        events: 64,
        channel_descriptors: 8,
    }
}

fn state() -> VisibilityState {
    VisibilityState::new(limits())
}

fn level(package: NameIdentity, persistent: bool) -> ObjectDescriptor {
    ObjectDescriptor::Level {
        package,
        is_persistent: Assumption::Known(persistent),
    }
}

fn facts(objects: &BTreeMap<ObjectIdentity, ObjectDescriptor>) -> VisibilityFacts<'_> {
    VisibilityFacts {
        objects,
        persistent_world_names: Assumption::Unknown,
    }
}

fn accepted(package: NameIdentity) -> VisibilityChange<'static> {
    VisibilityChange::Add {
        name: package,
        admission: AddAdmission::Accepted {
            package_world: Assumption::Known(None),
            graph_named_entry: Assumption::Known(false),
        },
    }
}

fn rejected(package: NameIdentity, policy: RejectedClosePolicy) -> VisibilityChange<'static> {
    VisibilityChange::Add {
        name: package,
        admission: AddAdmission::Rejected(policy),
    }
}

fn hide(package: NameIdentity) -> VisibilityChange<'static> {
    VisibilityChange::Hide {
        name: package,
        channels: Assumption::Known(&[]),
    }
}

#[test]
fn accepted_add_and_hide_order_callbacks_channels_and_existing_cache_refresh() {
    let package = name(10, 3);
    let key = object(1);
    let receiver = object(2);
    let objects = BTreeMap::from([(key, level(package, false))]);
    let facts = facts(&objects);
    let mut state = state();
    assert!(
        !state
            .client_has_initialized_level_for(Some(key), &facts)
            .unwrap()
    );
    state.clear_events();
    state
        .apply(accepted(package), ReceiverProfile::Opaque(receiver), &facts)
        .unwrap();
    assert!(state.contains_name(package));
    assert_eq!(state.cached(Some(key)), Some(true));
    assert_eq!(
        state.events(),
        &[
            Event::NameInserted {
                name: package,
                was_new: true
            },
            Event::AddCallback {
                receiver,
                name: package,
                package_world: None
            },
            Event::CacheUpdated {
                key: Some(key),
                visible: true
            },
            Event::RefreshFinished,
        ]
    );

    let channels = [
        ChannelDescriptor {
            channel: object(5),
            package,
        },
        ChannelDescriptor {
            channel: object(6),
            package: name(10, 4),
        },
        ChannelDescriptor {
            channel: object(7),
            package,
        },
    ];
    state.clear_events();
    state
        .apply(
            VisibilityChange::Hide {
                name: package,
                channels: Assumption::Known(&channels),
            },
            ReceiverProfile::SelectedGraph(receiver),
            &facts,
        )
        .unwrap();
    assert!(!state.contains_name(package));
    assert_eq!(state.cached(Some(key)), Some(false));
    assert_eq!(
        state.events(),
        &[
            Event::NameRemoved {
                name: package,
                was_present: true
            },
            Event::RemoveCallback {
                receiver,
                name: package
            },
            Event::GraphRemoveBroadcast { name: package },
            Event::ChannelCall {
                channel: object(5),
                argument: 2
            },
            Event::ChannelCall {
                channel: object(7),
                argument: 2
            },
            Event::CacheUpdated {
                key: Some(key),
                visible: false
            },
            Event::RefreshFinished,
        ]
    );
}

#[test]
fn rejected_add_never_inserts_and_known_native_policy_alone_selects_close() {
    let package = name(20, 1);
    let key = object(1);
    let initial = BTreeMap::from([(key, level(package, false))]);
    // A changed caller descriptor proves rejection still refreshes the cache.
    let changed = BTreeMap::from([(key, level(package, true))]);
    let changed_facts = VisibilityFacts {
        objects: &changed,
        persistent_world_names: Assumption::Known((name(30, 2), name(30, 2))),
    };
    for (policy, close) in [
        (RejectedClosePolicy::NativeBit1Clear, true),
        (RejectedClosePolicy::NativeBit1Set, false),
    ] {
        let mut state = state();
        assert!(
            !state
                .client_has_initialized_level_for(Some(key), &facts(&initial))
                .unwrap()
        );
        state.clear_events();
        // Receiver is irrelevant to rejection; it is never called or guessed.
        state
            .apply(
                rejected(package, policy),
                ReceiverProfile::Unknown,
                &changed_facts,
            )
            .unwrap();
        assert!(!state.contains_name(package));
        assert_eq!(state.cached(Some(key)), Some(true));
        let mut expected = Vec::new();
        if close {
            expected.push(Event::Close);
        }
        expected.extend([
            Event::CacheUpdated {
                key: Some(key),
                visible: true,
            },
            Event::RefreshFinished,
        ]);
        assert_eq!(state.events(), expected);
    }
}

#[test]
fn full_name_equality_and_cache_object_identity_are_independent() {
    let package = name(40, 1);
    let other_number = name(40, 2);
    let other_index = name(41, 1);
    let objects = BTreeMap::from([
        (object(1), level(package, false)),
        (object(2), level(package, false)),
        (object(3), level(other_number, false)),
        (object(4), level(other_index, false)),
        (object(5), level(package, false)), // Uncached world level stays absent.
    ]);
    let facts = facts(&objects);
    let mut state = state();
    for token in 1..=4 {
        assert!(
            !state
                .client_has_initialized_level_for(Some(object(token)), &facts)
                .unwrap()
        );
    }
    state
        .apply(accepted(package), ReceiverProfile::Absent, &facts)
        .unwrap();
    assert_eq!(state.cached(Some(object(1))), Some(true));
    assert_eq!(state.cached(Some(object(2))), Some(true));
    assert_eq!(state.cached(Some(object(3))), Some(false));
    assert_eq!(state.cached(Some(object(4))), Some(false));
    assert_eq!(state.cached(Some(object(5))), None);
    assert!(!state.contains_name(other_number));
    assert!(!state.contains_name(other_index));
    // Hiding a different number does not remove the accepted full name.
    state
        .apply(hide(other_number), ReceiverProfile::Absent, &facts)
        .unwrap();
    assert!(state.contains_name(package));
    assert_eq!(state.cached(Some(object(1))), Some(true));
}

#[test]
fn cache_hits_bypass_unknown_facts_and_refresh_skips_null_and_nonlevels() {
    let package = name(50, 0);
    let key = object(1);
    let objects = BTreeMap::from([(key, level(package, false))]);
    let mut state = state();
    assert!(
        !state
            .client_has_initialized_level_for(Some(key), &facts(&objects))
            .unwrap()
    );
    assert!(
        state
            .client_has_initialized_level_for(None, &facts(&objects))
            .unwrap()
    );
    state.clear_events();
    let unknown = BTreeMap::new();
    assert!(
        !state
            .client_has_initialized_level_for(Some(key), &facts(&unknown))
            .unwrap()
    );
    assert!(state.events().is_empty());
    let changed = BTreeMap::from([(key, ObjectDescriptor::InvalidOrNonLevel)]);
    state
        .apply(accepted(package), ReceiverProfile::Absent, &facts(&changed))
        .unwrap();
    assert_eq!(state.cached(Some(key)), Some(false));
    assert_eq!(state.cached(None), Some(true));
    assert_eq!(
        state.events(),
        &[
            Event::NameInserted {
                name: package,
                was_new: true
            },
            Event::RefreshFinished,
        ]
    );
}

#[test]
fn null_and_invalid_candidate_misses_store_only_the_null_key() {
    let key = object(90);
    let objects = BTreeMap::from([(key, ObjectDescriptor::InvalidOrNonLevel)]);
    let facts = facts(&objects);
    let mut state = state();
    assert!(
        state
            .client_has_initialized_level_for(Some(key), &facts)
            .unwrap()
    );
    assert_eq!(state.cached(Some(key)), None);
    assert_eq!(state.cached(None), Some(true));
    assert_eq!(
        state.events(),
        &[Event::CacheUpdated {
            key: None,
            visible: true
        }]
    );
    state.clear_events();
    assert!(
        state
            .client_has_initialized_level_for(None, &facts)
            .unwrap()
    );
    assert!(state.events().is_empty());
    let before = state.clone();
    assert_eq!(
        state.client_has_initialized_level_for(Some(object(91)), &facts),
        Err(ModelError::UnknownObjectClassification)
    );
    assert_eq!(state, before);
}

#[test]
fn persistent_name_match_is_full_identity_and_mismatch_falls_back_to_membership() {
    let package = name(60, 1);
    let key = object(1);
    let objects = BTreeMap::from([(key, level(package, true))]);
    for connection in [name(70, 1), name(70, 2), name(71, 1)] {
        let facts = VisibilityFacts {
            objects: &objects,
            persistent_world_names: Assumption::Known((name(70, 1), connection)),
        };
        let mut state = state();
        assert_eq!(
            state
                .client_has_initialized_level_for(Some(key), &facts)
                .unwrap(),
            connection == name(70, 1)
        );
        state
            .apply(accepted(package), ReceiverProfile::Absent, &facts)
            .unwrap();
        assert_eq!(state.cached(Some(key)), Some(true));
        state
            .apply(hide(package), ReceiverProfile::Absent, &facts)
            .unwrap();
        assert_eq!(state.cached(Some(key)), Some(connection == name(70, 1)));
    }
}

#[test]
fn explicit_graph_profile_emits_global_then_optional_full_name_broadcast_even_for_null_world() {
    let objects = BTreeMap::new();
    let facts = facts(&objects);
    let package = name(80, 9);
    let receiver = object(2);
    for world in [None, Some(object(3))] {
        for per_name in [false, true] {
            let mut state = state();
            state
                .apply(
                    VisibilityChange::Add {
                        name: package,
                        admission: AddAdmission::Accepted {
                            package_world: Assumption::Known(world),
                            graph_named_entry: Assumption::Known(per_name),
                        },
                    },
                    ReceiverProfile::SelectedGraph(receiver),
                    &facts,
                )
                .unwrap();
            let mut expected = vec![
                Event::NameInserted {
                    name: package,
                    was_new: true,
                },
                Event::AddCallback {
                    receiver,
                    name: package,
                    package_world: world,
                },
                Event::GraphAddBroadcast {
                    name: package,
                    package_world: world,
                    per_name: false,
                },
            ];
            if per_name {
                expected.push(Event::GraphAddBroadcast {
                    name: package,
                    package_world: world,
                    per_name: true,
                });
            }
            expected.push(Event::RefreshFinished);
            assert_eq!(state.events(), expected);
        }
    }
    // Knowing an opaque receiver identity does not select the graph body.
    let mut state = state();
    state
        .apply(accepted(package), ReceiverProfile::Opaque(receiver), &facts)
        .unwrap();
    assert_eq!(state.events().len(), 3);
    assert!(matches!(state.events()[1], Event::AddCallback { .. }));
}

#[test]
fn unknown_handler_policies_never_publish_partial_state_or_events() {
    let package = name(90, 5);
    let receiver = object(1);
    let objects = BTreeMap::new();
    let facts = facts(&objects);
    let cases = [
        (
            VisibilityChange::Add {
                name: package,
                admission: AddAdmission::Unknown,
            },
            ReceiverProfile::Absent,
            ModelError::UnknownAcceptance,
        ),
        (
            rejected(package, RejectedClosePolicy::Unknown),
            ReceiverProfile::Absent,
            ModelError::UnknownClosePolicy,
        ),
        (
            accepted(package),
            ReceiverProfile::Unknown,
            ModelError::UnknownReceiver,
        ),
        (
            VisibilityChange::Add {
                name: package,
                admission: AddAdmission::Accepted {
                    package_world: Assumption::Unknown,
                    graph_named_entry: Assumption::Unknown,
                },
            },
            ReceiverProfile::Opaque(receiver),
            ModelError::UnknownPackageWorld,
        ),
        (
            VisibilityChange::Add {
                name: package,
                admission: AddAdmission::Accepted {
                    package_world: Assumption::Known(None),
                    graph_named_entry: Assumption::Unknown,
                },
            },
            ReceiverProfile::SelectedGraph(receiver),
            ModelError::UnknownGraphEntry,
        ),
        (
            VisibilityChange::Hide {
                name: package,
                channels: Assumption::Unknown,
            },
            ReceiverProfile::Absent,
            ModelError::UnknownChannels,
        ),
        (
            hide(package),
            ReceiverProfile::Unknown,
            ModelError::UnknownReceiver,
        ),
    ];
    for initial in [None, Some(package)] {
        let mut state = state();
        if let Some(package) = initial {
            state
                .apply(accepted(package), ReceiverProfile::Absent, &facts)
                .unwrap();
        }
        let before = state.clone();
        for (change, profile, error) in cases {
            assert_eq!(state.apply(change, profile, &facts), Err(error));
            assert_eq!(state, before);
        }
    }
}

#[test]
fn unknown_level_helpers_fail_queries_and_refresh_atomically() {
    let package = name(100, 0);
    let key = object(1);
    for (descriptor, error) in [
        (level(package, true), ModelError::UnknownWorldNames),
        (
            ObjectDescriptor::Level {
                package,
                is_persistent: Assumption::Unknown,
            },
            ModelError::UnknownPersistentLevel,
        ),
    ] {
        let objects = BTreeMap::from([(key, descriptor)]);
        let mut state = state();
        let empty = state.clone();
        assert_eq!(
            state.client_has_initialized_level_for(Some(key), &facts(&objects)),
            Err(error)
        );
        assert_eq!(state, empty);
        let known = BTreeMap::from([(key, level(package, false))]);
        state
            .client_has_initialized_level_for(Some(key), &facts(&known))
            .unwrap();
        let before = state.clone();
        assert_eq!(
            state.apply(accepted(package), ReceiverProfile::Absent, &facts(&objects)),
            Err(error)
        );
        assert_eq!(state, before);
    }
}

#[test]
fn missing_refresh_classification_rolls_back_close_and_prior_cache_updates() {
    let package = name(110, 0);
    let first = object(1);
    let missing = object(2);
    let objects = BTreeMap::from([
        (first, level(package, false)),
        (missing, level(package, false)),
    ]);
    let mut state = state();
    for key in [first, missing] {
        state
            .client_has_initialized_level_for(Some(key), &facts(&objects))
            .unwrap();
    }
    let incomplete = BTreeMap::from([(first, level(package, false))]);
    let before = state.clone();
    assert_eq!(
        state.apply(
            rejected(package, RejectedClosePolicy::NativeBit1Clear),
            ReceiverProfile::Unknown,
            &facts(&incomplete)
        ),
        Err(ModelError::UnknownObjectClassification)
    );
    assert_eq!(state, before);
}

#[test]
fn every_native_limit_rejects_without_publication_and_duplicate_names_fit() {
    let package = name(120, 0);
    let key = object(1);
    let objects = BTreeMap::from([(key, level(package, false))]);
    let facts = facts(&objects);
    let mut constrained = limits();
    constrained.visible_names = 1;
    let mut state = VisibilityState::new(constrained);
    state
        .apply(accepted(package), ReceiverProfile::Absent, &facts)
        .unwrap();
    state.clear_events();
    state
        .apply(accepted(package), ReceiverProfile::Absent, &facts)
        .unwrap();
    assert_eq!(
        state.events()[0],
        Event::NameInserted {
            name: package,
            was_new: false
        }
    );
    let before = state.clone();
    assert_eq!(
        state.apply(accepted(name(120, 1)), ReceiverProfile::Absent, &facts),
        Err(ModelError::VisibleNameLimit)
    );
    assert_eq!(state, before);

    for (constrained, change, error) in [
        (
            Limits {
                events: 1,
                ..limits()
            },
            accepted(package),
            ModelError::EventLimit,
        ),
        (
            Limits {
                channel_descriptors: 0,
                ..limits()
            },
            VisibilityChange::Hide {
                name: package,
                channels: Assumption::Known(&[ChannelDescriptor {
                    channel: key,
                    package,
                }]),
            },
            ModelError::ChannelLimit,
        ),
    ] {
        let mut state = VisibilityState::new(constrained);
        let before = state.clone();
        assert_eq!(
            state.apply(change, ReceiverProfile::Absent, &facts),
            Err(error)
        );
        assert_eq!(state, before);
    }
    // Even a query's cache write rolls back if its log cannot be admitted.
    for (constrained, error) in [
        (
            Limits {
                cache_entries: 0,
                ..limits()
            },
            ModelError::CacheLimit,
        ),
        (
            Limits {
                events: 0,
                ..limits()
            },
            ModelError::EventLimit,
        ),
    ] {
        let mut state = VisibilityState::new(constrained);
        let before = state.clone();
        assert_eq!(
            state.client_has_initialized_level_for(Some(key), &facts),
            Err(error)
        );
        assert_eq!(state, before);
    }
}
