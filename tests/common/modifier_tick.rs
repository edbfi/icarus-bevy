//! Explicit modifier ticks with authored callbacks and real loopback delivery.
use super::*;
use icarus_bevy::experimental::{
    modifier_timing::{ModifierTickCalls, ModifierTickState, RemainingTime},
    peer_lab::RestoredModifierTickContext,
};

fn context() -> RestoredModifierTickContext {
    RestoredModifierTickContext {
        owner_role_byte: Some(3),
        interval_enabled: true,
        accumulated_seconds: 0.0,
        interval_seconds: 1.0,
    }
}

struct Calls {
    log: Vec<&'static str>,
    inputs: Vec<RemainingTime>,
    tags_match: bool,
    on_rep: Option<fn(&mut ModifierTickState)>,
    apply: Option<fn(&mut ModifierTickState)>,
    destroy: Option<fn(&mut ModifierTickState)>,
}

impl Default for Calls {
    fn default() -> Self {
        Self {
            log: Vec::new(),
            inputs: Vec::new(),
            tags_match: true,
            on_rep: None,
            apply: None,
            destroy: None,
        }
    }
}

impl ModifierTickCalls for Calls {
    fn base_tick(&mut self, state: &mut ModifierTickState, _: f32) {
        self.log.push("base");
        self.inputs.push(state.remaining);
    }
    fn owner_role(&mut self, _: &mut ModifierTickState) -> i32 {
        self.log.push("role");
        3
    }
    fn query_empty(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("query");
        false
    }
    fn owner_tags_match(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("tags");
        self.tags_match
    }
    fn request_destroy(&mut self, state: &mut ModifierTickState) {
        self.log.push("destroy");
        if let Some(effect) = self.destroy {
            effect(state);
        }
    }
    fn on_rep_remaining(&mut self, state: &mut ModifierTickState) {
        self.log.push("on_rep");
        if let Some(effect) = self.on_rep {
            effect(state);
        }
    }
    fn interval_condition(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("condition");
        true
    }
    fn interval_row_valid(&mut self, _: &mut ModifierTickState) -> bool {
        self.log.push("row");
        true
    }
    fn apply_interval(&mut self, state: &mut ModifierTickState) {
        self.log.push("apply");
        if let Some(effect) = self.apply {
            effect(state);
        }
    }
}

#[test]
fn modifier_ticks_validate_before_calls_and_reconfiguration_preserves_time() {
    let mut fixture = ModifierTimeFixture::new();
    let mut calls = Calls::default();
    let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
    let initial = lab.restored_modifier_time(fixture.connection, 1).unwrap();
    for (connection, uid) in [(u32::MAX, 1), (fixture.connection, 99)] {
        assert!(
            lab.configure_restored_modifier_tick(connection, uid, context())
                .is_err()
        );
        assert_eq!(lab.restored_modifier_tick_context(connection, uid), None);
        assert!(
            lab.tick_restored_modifier(connection, uid, 1.0, &mut calls)
                .is_err()
        );
    }
    assert!(
        lab.tick_restored_modifier(fixture.connection, 1, 1.0, &mut calls)
            .is_err()
    );
    assert!(calls.log.is_empty());
    lab.configure_restored_modifier_tick(fixture.connection, 1, context())
        .unwrap();
    let replacement = RestoredModifierTickContext {
        accumulated_seconds: 0.75,
        interval_seconds: 2.0,
        ..context()
    };
    lab.configure_restored_modifier_tick(fixture.connection, 1, replacement)
        .unwrap();
    assert_eq!(
        lab.restored_modifier_tick_context(fixture.connection, 1),
        Some(replacement)
    );
    assert_eq!(
        lab.restored_modifier_tick_context(fixture.connection, 2),
        None
    );
    assert_eq!(
        lab.restored_modifier_time(fixture.connection, 1),
        Some(initial)
    );
    assert!(calls.log.is_empty());
}

#[test]
fn modifier_ticks_read_latest_time_retain_callback_state_and_never_replay_for_delivery() {
    let mut fixture = ModifierTimeFixture::new();
    let mut calls = Calls::default();
    {
        let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
        lab.configure_restored_modifier_tick(fixture.connection, 1, context())
            .unwrap();
        lab.tick_restored_modifier(fixture.connection, 1, 0.25, &mut calls)
            .unwrap();
        lab.set_restored_modifier_time(fixture.connection, 1, 3.0, |state| {
            *state = RemainingTime {
                seconds: 3.5,
                replicated_tenths: 35,
            };
        })
        .unwrap();
        lab.tick_restored_modifier(fixture.connection, 1, 0.25, &mut calls)
            .unwrap();
    }
    fixture.refresh(&[(
        1,
        RemainingTime {
            seconds: 2.0,
            replicated_tenths: 20,
        },
    )]);
    calls.on_rep = Some(|state| {
        state.accumulated_seconds = 5.0;
        state.interval_seconds = 2.0;
    });
    calls.apply = Some(|state| {
        state.remaining = RemainingTime {
            seconds: 7.0,
            replicated_tenths: 17,
        };
        state.owner_role_byte = Some(2);
        state.interval_enabled = false;
        state.accumulated_seconds = 20.0;
        state.interval_seconds = 3.0;
    });
    let retained_context = RestoredModifierTickContext {
        owner_role_byte: Some(2),
        interval_enabled: false,
        accumulated_seconds: 17.0,
        interval_seconds: 3.0,
    };
    {
        let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
        lab.tick_restored_modifier(fixture.connection, 1, 0.25, &mut calls)
            .unwrap();
        assert_eq!(
            lab.restored_modifier_tick_context(fixture.connection, 1),
            Some(retained_context)
        );
        assert_eq!(
            lab.restored_modifier_time(fixture.connection, 1),
            Some(RemainingTime {
                seconds: 7.0,
                replicated_tenths: 17
            })
        );
        lab.poll();
    }
    assert_eq!(
        calls.inputs,
        [
            RemainingTime {
                seconds: 50.25,
                replicated_tenths: 503
            },
            RemainingTime {
                seconds: 3.5,
                replicated_tenths: 35
            },
            RemainingTime {
                seconds: 2.0,
                replicated_tenths: 20
            },
        ]
    );
    assert_eq!(calls.log.iter().filter(|call| **call == "apply").count(), 1);
    assert!(
        fixture.collect().is_empty(),
        "restore ACK gates publication"
    );
    let callbacks = calls.log.clone();
    let changed = fixture.ack();
    fixture.assert_time_blocks(&changed, &[(1, 17)]);
    assert!(fixture.ack().is_empty());
    for _ in 0..4 {
        fixture.app.world_mut().resource_mut::<PeerLab>().poll();
    }
    assert!(fixture.collect().is_empty());
    assert_eq!(calls.log, callbacks);
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<PeerLab>()
            .restored_modifier_tick_context(fixture.connection, 1),
        Some(retained_context)
    );

    calls.on_rep = None;
    calls.apply = None;
    {
        let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
        lab.configure_restored_modifier_tick(
            fixture.connection,
            1,
            RestoredModifierTickContext {
                interval_enabled: false,
                ..context()
            },
        )
        .unwrap();
        lab.tick_restored_modifier(fixture.connection, 1, 0.5, &mut calls)
            .unwrap();
        lab.poll();
    }
    let callbacks = calls.log.clone();
    let queued = fixture
        .app
        .world()
        .resource::<PeerLab>()
        .stats()
        .modifier_time_updates_queued;
    fixture
        .socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    assert!(fixture.socket.recv(&mut [0; 2048]).unwrap() > 0);
    assert!(fixture.ack().is_empty());
    let repaired = fixture.ack();
    fixture.assert_time_blocks(&repaired, &[(1, 65)]);
    assert!(fixture.ack().is_empty());
    assert_eq!(calls.log, callbacks, "NAK repair must not replay the tick");
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<PeerLab>()
            .stats()
            .modifier_time_updates_queued,
        queued
    );
}

#[test]
fn modifier_ticks_cross_zero_once_and_destruction_requests_retain_the_modifier() {
    let mut fixture = ModifierTimeFixture::new();
    fixture.refresh(&[(
        1,
        RemainingTime {
            seconds: 1.0,
            replicated_tenths: 10,
        },
    )]);
    let mut calls = Calls::default();
    {
        let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
        lab.configure_restored_modifier_tick(fixture.connection, 1, context())
            .unwrap();
        lab.tick_restored_modifier(fixture.connection, 1, 5.0, &mut calls)
            .unwrap();
        assert_eq!(
            lab.restored_modifier_time(fixture.connection, 1),
            Some(RemainingTime {
                seconds: -4.0,
                replicated_tenths: 0
            })
        );
        assert_eq!(
            lab.restored_modifier_tick_context(fixture.connection, 1)
                .unwrap()
                .accumulated_seconds,
            4.0
        );
    }
    assert_eq!(calls.log.iter().filter(|call| **call == "apply").count(), 1);
    assert!(!calls.log.contains(&"destroy"));
    let before = calls.log.len();
    calls.tags_match = false;
    calls.destroy = Some(|state| {
        state.remaining = RemainingTime {
            seconds: 8.0,
            replicated_tenths: 80,
        };
        state.interval_enabled = false;
        state.accumulated_seconds = 12.0;
    });
    {
        let mut lab = fixture.app.world_mut().resource_mut::<PeerLab>();
        lab.tick_restored_modifier(fixture.connection, 1, 99.0, &mut calls)
            .unwrap();
        assert_eq!(
            lab.restored_modifier_time(fixture.connection, 1),
            Some(RemainingTime {
                seconds: 8.0,
                replicated_tenths: 80
            })
        );
        assert_eq!(
            lab.restored_modifier_tick_context(fixture.connection, 1),
            Some(RestoredModifierTickContext {
                interval_enabled: false,
                accumulated_seconds: 12.0,
                ..context()
            })
        );
        assert_eq!(
            lab.restored_modifier_time(fixture.connection, 2),
            Some(RemainingTime {
                seconds: 50.25,
                replicated_tenths: 503
            })
        );
        assert_eq!(
            lab.restored_modifier_tick_context(fixture.connection, 2),
            None
        );
    }
    assert_eq!(
        &calls.log[before..],
        ["base", "role", "query", "tags", "destroy"]
    );
    let changed = fixture.ack();
    fixture.assert_time_blocks(&changed, &[(1, 80)]);
}

#[test]
fn modifier_tick_contexts_are_peer_and_uid_local_and_discarded_on_disconnect() {
    use icarus_bevy::{
        actor_uids::ActorUidLibrary,
        experimental::{
            modifier_restore::{ModifierRestoreTables, ModifierRow},
            peer_lab::PeerLabAdmission,
            prospect_info::SavedModifierState,
        },
        prospect,
    };
    let (mut lab, profile) = uid_lab(ActorUidLibrary::with_random(|| 7));
    lab.enable_player_state_open(
        profile
            .player_state_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    lab.enable_modifier_restore(ModifierRestoreTables {
        rows: vec![
            ModifierRow {
                saves: true,
                ..Default::default()
            };
            2
        ],
        row_index: std::collections::BTreeMap::from([("first".into(), 0), ("second".into(), 1)]),
    })
    .unwrap();
    let payload = returning_characters_payload_with_state(NET_ID_HEX, 3, &[70_001, 70_002], &[]);
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    let mut characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
    for character in &mut characters {
        character.modifiers = ["First", "Second"]
            .into_iter()
            .map(|name| SavedModifierState {
                row_name: name.encode_utf16().collect(),
                time_remaining: 100.5,
                initial_lifetime: 300.0,
                duration_buff: 2.0,
                effectiveness: -1,
            })
            .collect();
    }
    lab.enable_returning_characters(characters).unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let (first, _, _, _) = ModifierTimeFixture::join(&mut app, address, &profile);
    let first_id = app.world().resource::<PeerLab>().joined_connections()[0];
    let (second, _, _, _) = ModifierTimeFixture::join(&mut app, address, &profile);
    let second_id = app
        .world()
        .resource::<PeerLab>()
        .joined_connections()
        .into_iter()
        .find(|id| *id != first_id)
        .unwrap();
    let mut calls = Calls::default();
    {
        let mut lab = app.world_mut().resource_mut::<PeerLab>();
        for (connection, uid) in [(first_id, 1), (first_id, 2), (second_id, 1)] {
            lab.configure_restored_modifier_tick(connection, uid, context())
                .unwrap();
        }
        lab.tick_restored_modifier(first_id, 1, 0.5, &mut calls)
            .unwrap();
        assert_eq!(
            lab.restored_modifier_time(first_id, 1).unwrap().seconds,
            49.75
        );
        for (connection, uid) in [(first_id, 2), (second_id, 1)] {
            assert_eq!(
                lab.restored_modifier_tick_context(connection, uid),
                Some(context())
            );
            assert_eq!(
                lab.restored_modifier_time(connection, uid).unwrap().seconds,
                50.25
            );
        }
        lab.tick_restored_modifier(second_id, 1, 0.25, &mut calls)
            .unwrap();
        assert_eq!(
            lab.restored_modifier_time(second_id, 1).unwrap().seconds,
            50.0
        );
        assert_eq!(
            lab.restored_modifier_tick_context(first_id, 1)
                .unwrap()
                .accumulated_seconds,
            0.5
        );
        assert_eq!(
            lab.restored_modifier_tick_context(second_id, 1)
                .unwrap()
                .accumulated_seconds,
            0.25
        );
    }
    second.close().unwrap();
    app.world_mut().resource_mut::<PeerLab>().poll();
    let before = calls.log.clone();
    {
        let mut lab = app.world_mut().resource_mut::<PeerLab>();
        assert_eq!(lab.restored_modifier_tick_context(second_id, 1), None);
        assert!(
            lab.tick_restored_modifier(second_id, 1, 1.0, &mut calls)
                .is_err()
        );
        assert_eq!(
            lab.restored_modifier_time(first_id, 1).unwrap().seconds,
            49.75
        );
    }
    let (third, _, _, _) = ModifierTimeFixture::join(&mut app, address, &profile);
    let third_id = app
        .world()
        .resource::<PeerLab>()
        .joined_connections()
        .into_iter()
        .find(|id| *id != first_id)
        .unwrap();
    assert_ne!(third_id, second_id);
    {
        let mut lab = app.world_mut().resource_mut::<PeerLab>();
        assert_eq!(lab.restored_modifier_tick_context(third_id, 1), None);
        assert!(
            lab.tick_restored_modifier(third_id, 1, 1.0, &mut calls)
                .is_err()
        );
        assert_eq!(
            lab.restored_modifier_time(third_id, 1).unwrap().seconds,
            50.25
        );
    }
    assert_eq!(calls.log, before);
    first.close().unwrap();
    third.close().unwrap();
    app.world_mut().resource_mut::<PeerLab>().poll();
    assert_eq!(app.world().resource::<PeerLab>().peer_count(), 0);
}
