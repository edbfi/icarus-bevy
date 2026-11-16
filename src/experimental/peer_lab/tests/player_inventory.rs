//! No listener or datagram I/O: synthetic post-Pawn failure association.
use super::*;
use crate::{
    config::ServerConfig,
    native_inventory::{InventoryDefinitionV1, ItemDefinitionV1, SlotRuleV1},
    native_player_inventory::{
        CompiledPlayerInventoryRecipe, PlayerInventoryRecipeV1, PlayerStarterItemV1,
    },
    native_player_runtime::{PlayerRegistry, drain_inventory_activations},
    native_players::{PlayerCheckpoint, PlayerContext, PlayerInputs, PlayerKey, PlayerSnapshot},
    simulation::{SNAPSHOT_SCHEMA_PLAYER_INVENTORY, WorldSnapshot, build_app, capture},
};
use std::net::Ipv4Addr;
fn post_pawn_peer(config: PacketLabConfig) -> PeerEntry {
    let cookie = [0; 20];
    let peer = LabPeer::from_cookie(&cookie, config).unwrap();
    let now = Instant::now();
    let mut entry = PeerEntry {
        controller_leave: None,
        checkpoint_epoch: None,
        checkpoint_projection: None,
        checkpoint_enqueued_revision: None,
        checkpoint_active: false,
        connected_player: None,
        connected_players_sent: Vec::new(),
        connected_players_pending: false,
        connected_players_retry_ready: false,
        connected_player_receipts: Vec::new(),
        connected_player_creations_settled: 0,
        controller_open_guids: None,
        controller_possession_pending: false,
        controller_possession_retry_ready: false,
        controller_possession_receipt: InitialisationReceipt::default(),
        controller_possession_failed: false,
        record_prerequisites: RecordPrerequisites::default(),
        frozen_release: FrozenMovementRelease::default(),
        pawn_inbound: PawnInboundCounts::default(),
        pawn_moves: PawnMoves::default(),
        placement: None,
        visible_levels: BTreeSet::new(),
        level_actors: BTreeMap::new(),
        level_actor_queue: VecDeque::new(),
        level_actors_retry_ready: false,
        level_actors_refused: false,
        level_channels_released: false,
        friends_query_pending: false,
        friends_query_retry_ready: false,
        friends_query_receipt: InitialisationReceipt::default(),
        friends_query_since: None,
        controller_field_diagnostics: ControllerFieldDiagnostics::default(),
        auth_request: None,
        auth_identity: None,
        auth_accepted: false,
        auth_ticket_digest: None,
        pawn_character_state: None,
        pawn_temperature: None,
        pawn_temperature_tick: None,
        queued_pawn_temperature: None,
        peer,
        pawn_uid: None,
        controller_startup_pawn_guid: None,
        controller_startup_next: None,
        controller_startup_queued: 0,
        controller_startup_settled: 0,
        initialisation_request_receipt: InitialisationReceipt::default(),
        initialisation_completion_receipt: InitialisationReceipt::default(),
        controller_startup_failed: false,
        controller_startup_retry_ready: false,
        pending_pawn_uid: None,
        restored_character: None,
        restored_modifiers: Vec::new(),
        restored_modifier_ticks: BTreeMap::new(),
        queued_modifier_times: BTreeMap::new(),
        pawn_property_retry_ready: false,
        pending_pawn_restore: None,
        pawn_restore_retry_ready: false,
        cookie,
        last_seen: now,
        delivery_progress_at: None,
        ack_pending: false,
        ack_capacity_blocked: false,
        pending_control_reply: None,
        control_reply_retry_ready: false,
        admission: ConnectionLoginAdmission::new(),
        login_url_units: Vec::new(),
        spectator: false,
        splitscreen_count: 0,
        current_net_speed: None,
        exports: ConnectionExports::new(),
        login_unique_id: None,
        player: None,
        connection_id: None,
        player_controller_channel: None,
        player_state_channel: None,
        initial_opens: None,
        initial_open_outcomes: InitialOpenOutcomes::default(),
        player_history_channel: None,
        player_history_pending: false,
        player_history_retry_ready: false,
        character_slot: None,
        backend_proxy: None,
        player_data: None,
        bestiary: None,
        controller_component_mapping: ControllerComponentMapping::default(),
        profiles: ProfileExchange::default(),
        pending_profile_exchange: None,
        profile_exchange_retry_ready: false,
        opened_actors: BTreeMap::new(),
        opened_incarnations: BTreeMap::new(),
        retiring_incarnations: BTreeMap::new(),
        closing_legacy_actors: BTreeMap::new(),
        pending_closes: BTreeMap::new(),
        actor_properties: BTreeMap::new(),
        actor_property_packets: BTreeMap::new(),
        game_state: None,
        ping: PingEstimator::new(),
    };
    entry.admission.state = ClientLoginState::Joined;
    entry.auth_accepted = true;
    entry.auth_request = Some(1);
    entry.auth_identity = Some(1234);
    entry.checkpoint_epoch = Some(1);
    entry.profiles.completed = true;
    entry.profiles.friend_ids_ready = true;
    entry.record_prerequisites.pawn = InitialisationReceipt {
        channel: Some(6),
        first: 1,
        last: 1,
        settled: true,
        abandoned: false,
    };
    entry.initial_open_outcomes.committed.pawn = 1;
    entry
}
fn inventory_state() -> WorldSnapshot {
    let recipe = CompiledPlayerInventoryRecipe::compile(PlayerInventoryRecipeV1 {
        format: 1,
        items: vec![ItemDefinitionV1 {
            key: "authored".into(),
            stack_limit: 10,
        }],
        inventories: vec![InventoryDefinitionV1 {
            key: "bag".into(),
            initial_slots: 1,
            slot_rules: vec![SlotRuleV1 {
                allowed_items: vec!["authored".into()],
            }],
            overflow_limit: 0,
            force_add_when_zero: false,
        }],
        starter: vec![PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: 0,
            item: "authored".into(),
            count: 3,
        }],
    })
    .unwrap();
    let context = PlayerContext::new(
        vec![65],
        "/Authored/Inventory".into(),
        PlayerInputs {
            authored_gameplay: [1; 32],
            spawn: [1; 32],
            authored_world: [1; 32],
            declaration: [1; 32],
            initial_values: [1; 32],
        },
        vec!["Player".into()],
    )
    .unwrap();
    let inventory = recipe.empty();
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        native_players: Some(PlayerSnapshot::empty(context)),
        player_inventory: Some(inventory),
        ..Default::default()
    }
}

#[test]
fn admitted_pawn_activation_failure_detaches_removes_and_never_issues() {
    let mut state = inventory_state();
    let mut length = serde_json::to_vec(&state).unwrap().len();
    let mut id = 1;
    loop {
        let entity = crate::simulation::EntityRecord {
            id: crate::simulation::DomainId(id),
            position: crate::simulation::Position([0.; 3]),
        };
        let delta =
            serde_json::to_vec(&entity).unwrap().len() + usize::from(!state.entities.is_empty());
        if length + delta > 64 * 1024 * 1024 {
            break;
        }
        length += delta;
        state.entities.push(entity);
        id += 1;
    }
    crate::persistence::SnapshotStore::preflight(&state).unwrap();
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    let vitals = crate::experimental::pawn_open::CharacterVitals {
        health: 100,
        max_health: 100,
        stamina: 100,
        max_stamina: 100,
        oxygen: 100,
        max_oxygen: 100,
        water: 100,
        max_water: 100,
        food: 100,
        max_food: 100,
        biome_row: 0,
        growth_row: 0,
    };
    let mut lab = PeerLab::prepare(PeerLabConfig::default()).unwrap();
    lab.checkpoint_initial_vitals = Some(vitals);
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
    let entry = post_pawn_peer(lab.config.packet);
    assert_eq!(entry.initial_open_outcomes.committed.pawn, 1);
    assert!(entry.record_prerequisites.pawn.settled);
    assert!(lab.socket.0.is_none());
    lab.peers.insert(address, entry);
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        let projection = registry
            .reserve(
                PlayerKey::new(1234, 0).unwrap(),
                1,
                PlayerCheckpoint {
                    location_cm: [0.; 3],
                    vitals,
                },
            )
            .unwrap();
        registry.request_inventory_activation(1);
        lab.peers.get_mut(&address).unwrap().checkpoint_projection = Some(projection);
    }
    let before = capture(app.world_mut()).unwrap();
    let failed = drain_inventory_activations(app.world_mut());
    assert_eq!(failed, vec![1]);
    lab.finish_inventory_activations(app.world().resource::<PlayerRegistry>(), &failed);
    assert!(!lab.has_peer(address));
    assert!(
        app.world()
            .resource::<PlayerRegistry>()
            .active_epochs()
            .is_empty()
    );
    assert_eq!(capture(app.world_mut()).unwrap(), before);
    assert!(before.native_players.unwrap().is_empty());
    assert!(before.player_inventory.unwrap().bags.is_empty());
}

#[test]
fn canceled_controller_completion_keeps_schema15_inventory_cutoff() {
    use crate::{
        experimental::{
            release::{
                ControllerLeaveQualification, ControllerLeaveRevision, ReleaseProfile,
                ReleaseQualifications,
            },
            rep_rpc::{ControllerLeaveRequest, LeaveProspectSessionType},
        },
        native_player_runtime::{CheckpointWriter, LeavePhase, activate_player},
        persistence::SnapshotStore,
    };
    let state = inventory_state();
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    let vitals = crate::experimental::pawn_open::CharacterVitals {
        health: 100,
        max_health: 100,
        stamina: 100,
        max_stamina: 100,
        oxygen: 0,
        max_oxygen: 100,
        water: 100,
        max_water: 100,
        food: 100,
        max_food: 100,
        biome_row: 0,
        growth_row: 0,
    };
    let mut lab = PeerLab::prepare(PeerLabConfig::default()).unwrap();
    lab.checkpoint_initial_vitals = Some(vitals);
    let mut release = ReleaseProfile::fixture("3.0.36");
    release.qualifications = Some(ReleaseQualifications {
        pin: release.pin.clone(),
        engine_network_version: release.engine_network_version,
        game_network_version: release.game_network_version,
        class_net_cache: release.class_net_cache,
        quest_catalog: None,
        visibility_decode: None,
        inbound_controller: None,
        controller_leave: Some(ControllerLeaveQualification {
            revision: ControllerLeaveRevision::SelectedControllerV1,
            player_controller: release.player_controller.clone().unwrap(),
        }),
    });
    lab.authored_controller_leave = release.controller_leave_plan().unwrap();
    let key = PlayerKey::new(1234, 0).unwrap();
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(
            key,
            1,
            PlayerCheckpoint {
                location_cm: [0.; 3],
                vitals,
            },
        )
        .unwrap();
    activate_player(app.world_mut(), 1).unwrap();
    crate::native_player_runtime::advance_player_survival(
        app.world_mut(),
        crate::native_player_runtime::ConsumptionRates::ZERO,
        crate::native_player_runtime::DepletionDamageRates::new(7, 0, 0).unwrap(),
    )
    .unwrap();
    let projection = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(1)
        .unwrap();
    assert_eq!(projection.checkpoint.vitals.health, 93);
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
    let mut entry = post_pawn_peer(lab.config.packet);
    entry.connection_id = Some(1);
    entry.character_slot = Some(0);
    entry.profiles.character_slot = Some(0);
    entry.checkpoint_projection = Some(projection);
    entry.checkpoint_active = true;
    lab.peers.insert(address, entry);
    lab.stage_authored_controller_leave(
        address,
        ControllerLeaveRequest::Normal(Some(LeaveProspectSessionType::Quit)),
    );
    lab.bridge_controller_leaves(&mut app.world_mut().resource_mut::<PlayerRegistry>());
    assert!(app.world().resource::<PlayerRegistry>().is_leaving(1));
    lab.stage_authored_controller_leave(address, ControllerLeaveRequest::Cancel);
    assert!(
        lab.peers[&address]
            .controller_leave
            .as_ref()
            .unwrap()
            .suppressed
    );
    let before = capture(app.world_mut()).unwrap();
    for _ in 0..3 {
        assert_eq!(
            crate::native_player_runtime::advance_player_survival(
                app.world_mut(),
                crate::native_player_runtime::ConsumptionRates::ZERO,
                crate::native_player_runtime::DepletionDamageRates::new(7, 0, 0).unwrap()
            )
            .unwrap(),
            0
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(dir.path()).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    lab.bridge_controller_leaves(&mut app.world_mut().resource_mut::<PlayerRegistry>());
    assert!(matches!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key, 1)
            .unwrap()
            .phase,
        LeavePhase::Durable { .. }
    ));
    assert_eq!(lab.stats.controller_leave.completions_admitted, 0);
    assert_eq!(lab.stats.controller_leave.cancel_suppressions, 1);
    assert!(
        lab.peers[&address]
            .controller_leave
            .as_ref()
            .unwrap()
            .receipt
            .channel
            .is_none()
    );
    assert_eq!(capture(app.world_mut()).unwrap(), before);
    assert!(lab.socket.0.is_none());
}

#[test]
fn authored_use_projection_refresh_coalesces_without_reexecuting_use() {
    use crate::{
        native_player_inventory::{
            PlayerConsumableV2, PlayerInventoryActionV1, PlayerInventoryRecipe,
            PlayerInventoryRecipeV2, PlayerInventoryRequestV1, PlayerStackEndpointV1,
        },
        native_player_runtime::{activate_player, apply_player_inventory_action},
    };
    let mut state = inventory_state();
    let PlayerInventoryRecipe::V1(r) = state.player_inventory.take().unwrap().recipe else {
        unreachable!()
    };
    state.player_inventory = Some(
        CompiledPlayerInventoryRecipe::compile_versioned(
            PlayerInventoryRecipeV2 {
                format: 2,
                items: r.items,
                inventories: r.inventories,
                starter: r.starter,
                consumables: vec![PlayerConsumableV2 {
                    item: "authored".into(),
                    oxygen: 15,
                    water: 25,
                    food: 35,
                }],
            }
            .into(),
        )
        .unwrap()
        .empty(),
    );
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    let key = PlayerKey::new(1234, 0).unwrap();
    let checkpoint = PlayerCheckpoint {
        location_cm: [0.; 3],
        vitals: crate::experimental::pawn_open::CharacterVitals {
            health: 100,
            max_health: 100,
            stamina: 100,
            max_stamina: 100,
            oxygen: 0,
            max_oxygen: 100,
            water: 0,
            max_water: 100,
            food: 0,
            max_food: 100,
            biome_row: 0,
            growth_row: 0,
        },
    };
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key, 1, checkpoint)
        .unwrap();
    let before_damage = activate_player(app.world_mut(), 1).unwrap();
    crate::native_player_runtime::advance_player_survival(
        app.world_mut(),
        crate::native_player_runtime::ConsumptionRates::ZERO,
        crate::native_player_runtime::DepletionDamageRates::new(2, 3, 4).unwrap(),
    )
    .unwrap();
    let initial = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(1)
        .unwrap();
    assert_eq!(initial.checkpoint.vitals.health, 91);
    let mut lab = PeerLab::prepare(PeerLabConfig::default()).unwrap();
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
    let mut entry = post_pawn_peer(lab.config.packet);
    entry.checkpoint_active = true;
    entry.checkpoint_projection = Some(before_damage);
    entry.checkpoint_enqueued_revision = Some(before_damage.revision);
    lab.peers.insert(address, entry);
    let damaged = capture(app.world_mut()).unwrap();
    for _ in 0..4 {
        lab.peers
            .get_mut(&address)
            .unwrap()
            .pawn_restore_retry_ready = true;
        lab.refresh_checkpoint_projections(app.world().resource::<PlayerRegistry>());
        assert_eq!(capture(app.world_mut()).unwrap(), damaged);
    }
    assert_eq!(lab.peers[&address].checkpoint_projection, Some(initial));
    for _ in 0..2 {
        let before = capture(app.world_mut()).unwrap();
        let bag = &before.player_inventory.as_ref().unwrap().bags[0];
        let projection = before
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(initial.player_id)
            .unwrap();
        let request = PlayerInventoryRequestV1 {
            player_id: initial.player_id,
            expected_player_revision: projection.revision,
            expected_bag_revision: bag.revision,
            action: PlayerInventoryActionV1::Use {
                source: PlayerStackEndpointV1 {
                    owner: bag.owner,
                    inventory: "bag".into(),
                    stack_id: bag.inventories[0].slots[0].as_ref().unwrap().id,
                },
            },
        };
        apply_player_inventory_action(app.world_mut(), key, 1, &request).unwrap();
        assert_eq!(
            crate::native_player_runtime::advance_player_survival(
                app.world_mut(),
                crate::native_player_runtime::ConsumptionRates::ZERO,
                crate::native_player_runtime::DepletionDamageRates::new(2, 3, 4).unwrap()
            )
            .unwrap(),
            0
        );
        let after = capture(app.world_mut()).unwrap();
        for _ in 0..4 {
            lab.refresh_checkpoint_projections(app.world().resource::<PlayerRegistry>());
        }
        assert_eq!(
            lab.peers[&address].checkpoint_enqueued_revision,
            Some(before_damage.revision)
        );
        assert_eq!(
            lab.peers[&address].checkpoint_projection,
            app.world().resource::<PlayerRegistry>().projection(1)
        );
        assert_eq!(capture(app.world_mut()).unwrap(), after);
    }
    let after = capture(app.world_mut()).unwrap();
    assert_eq!(
        after.player_inventory.as_ref().unwrap().bags[0].consumed[0].count,
        2
    );
    assert_eq!(
        lab.peers[&address]
            .checkpoint_projection
            .unwrap()
            .checkpoint
            .vitals
            .oxygen,
        30
    );
}

#[test]
fn depletion_damage_queued_revocation_wins_before_scheduled_health_loss() {
    use crate::native_player_runtime::{
        ConsumptionConfig, DepletionDamageConfig, activate_player, install_survival,
    };
    let state = inventory_state();
    let mut app = build_app(
        &ServerConfig {
            tick_hz: 1,
            ..Default::default()
        },
        &state,
    )
    .unwrap();
    let key = PlayerKey::new(1234, 0).unwrap();
    let vitals = crate::experimental::pawn_open::CharacterVitals {
        health: 100,
        max_health: 100,
        stamina: 100,
        max_stamina: 100,
        oxygen: 0,
        max_oxygen: 100,
        water: 0,
        max_water: 100,
        food: 0,
        max_food: 100,
        biome_row: 0,
        growth_row: 0,
    };
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(
            key,
            1,
            PlayerCheckpoint {
                location_cm: [0.; 3],
                vitals,
            },
        )
        .unwrap();
    let projection = activate_player(app.world_mut(), 1).unwrap();
    let mut lab = PeerLab::prepare(PeerLabConfig::default()).unwrap();
    lab.checkpoint_initial_vitals = Some(vitals);
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
    let mut entry = post_pawn_peer(lab.config.packet);
    entry.checkpoint_active = true;
    entry.checkpoint_projection = Some(projection);
    entry.checkpoint_enqueued_revision = Some(projection.revision);
    lab.peers.insert(address, entry);
    app.insert_resource(lab);
    install_survival(
        &mut app,
        ConsumptionConfig {
            oxygen: 0,
            water: 0,
            food: 1,
        },
        Some(DepletionDamageConfig {
            oxygen: 2,
            water: 3,
            food: 4,
        }),
        1,
    )
    .unwrap();
    let before = capture(app.world_mut()).unwrap();
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
    let after = capture(app.world_mut()).unwrap();
    assert_eq!(after.native_players, before.native_players);
    assert_eq!(after.player_inventory, before.player_inventory);
    assert_eq!(after.tick, before.tick + 1);
    assert!(
        app.world()
            .resource::<PlayerRegistry>()
            .active_epochs()
            .is_empty()
    );
    assert!(!app.world().resource::<PeerLab>().has_peer(address));
    assert!(app.world().resource::<PeerLab>().socket.0.is_none());
}
