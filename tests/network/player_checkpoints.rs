//! Source-authored checkpoint preflight and offline retention contracts.
use super::*;
use icarus_bevy::{
    authored_world_bootstrap::AuthoredWorldBootstrap,
    config::PlayerPersistenceConfig,
    data_pak::DataPak,
    gameplay_profile::{FreshPlayerValues, fresh_player_profile},
    native_player_runtime::PlayerRegistry,
    native_players::{PlayerCheckpoint, PlayerKey},
    persistence::SnapshotStore,
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, capture},
};

fn inputs() -> (tempfile::TempDir, NetworkConfig) {
    let directory = tempfile::tempdir().unwrap();
    let pak = directory.path().join("authored.pak");
    fs::write(&pak, pak_writer::fresh_join_pak()).unwrap();
    let gameplay = directory.path().join("authored.json");
    let profile = fresh_player_profile(
        &mut DataPak::open(&pak).unwrap(),
        &FreshPlayerValues::default(),
    )
    .unwrap();
    fs::write(&gameplay, serde_json::to_vec(&profile).unwrap()).unwrap();
    let config = NetworkConfig {
        enabled: false,
        player_persistence: Some(PlayerPersistenceConfig {
            prospect_id: "authored-checkpoint".into(),
            map_url: "/Authored/Maps/Workshop".into(),
            prospect_row: "AuthoredWorkshop".into(),
            difficulty: 2,
        }),
        release_profile: Some(release_toml()),
        data_pak: Some(pak),
        authored_gameplay: Some(gameplay),
        authored_world: Some(AuthoredWorldBootstrap {
            clock: None,
            seed: 42,
            seed_initialised: true,
            flod_tile_count: 0,
        }),
        ..Default::default()
    };
    (directory, config)
}

#[test]
fn offline_context_is_prepared_and_old_schema_is_never_promoted() {
    let (_directory, config) = inputs();
    let (session, prepared) =
        network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    assert!(session.is_none());
    let prepared = prepared.unwrap();
    for schema in 1..=12 {
        let old = WorldSnapshot {
            schema_version: schema,
            ..Default::default()
        };
        assert!(network::validate_player_snapshot(&old, Some(&prepared)).is_err());
    }
    let fresh = network::fresh_player_snapshot(Some(&prepared));
    assert_eq!(fresh.schema_version, SNAPSHOT_SCHEMA_PLAYERS);
    assert!(network::validate_player_snapshot(&fresh, None).is_err());
    network::validate_player_snapshot(&fresh, Some(&prepared)).unwrap();
}

#[test]
fn exact_recipe_bytes_world_and_mission_identity_bind_the_context() {
    let (_directory, config) = inputs();
    let (_, prepared) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    let snapshot = network::fresh_player_snapshot(prepared.as_ref());
    for variant in ["row", "difficulty", "spawn", "world", "bytes"] {
        let mut changed = config.clone();
        match variant {
            "row" => changed
                .player_persistence
                .as_mut()
                .unwrap()
                .prospect_row
                .push('x'),
            "difficulty" => changed.player_persistence.as_mut().unwrap().difficulty = 3,
            "spawn" => changed
                .spawn
                .points
                .push(icarus_bevy::spawn_placement::SpawnPoint {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                    yaw: None,
                }),
            "world" => changed.authored_world.as_mut().unwrap().seed = 43,
            "bytes" => {
                let path = changed.authored_gameplay.as_ref().unwrap();
                let mut bytes = fs::read(path).unwrap();
                bytes.push(b'\n');
                fs::write(path, bytes).unwrap();
            }
            _ => unreachable!(),
        }
        let (_, other) =
            network::prepare_run(&changed, network::peer_lab_config(&changed)).unwrap();
        assert!(network::validate_player_snapshot(&snapshot, other.as_ref()).is_err());
    }
}

#[test]
fn offline_reopen_and_network_teardown_retain_committed_vital_checkpoint() {
    let (directory, network_config) = inputs();
    let (_, prepared) =
        network::prepare_run(&network_config, network::peer_lab_config(&network_config)).unwrap();
    let prepared = prepared.unwrap();
    let config = ServerConfig {
        network: network_config.clone(),
        ..Default::default()
    };
    let mut app = build_app(&config, &network::fresh_player_snapshot(Some(&prepared))).unwrap();
    network::install_player_registry(app.world_mut(), Some(&prepared), &network_config).unwrap();
    let key = PlayerKey::new(76_561_197_960_265_770, 3).unwrap();
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    let initial = PlayerCheckpoint {
        location_cm: [100.0, 200.0, 300.0],
        vitals: prepared.initial_vitals,
    };
    registry.reserve(key, 1, initial).unwrap();
    let first = registry.activate(1).unwrap();
    let mut checkpoint = first.checkpoint;
    checkpoint.vitals.health = 37;
    checkpoint.location_cm = [700.0, 800.0, 900.0];
    registry.edit(key, first.revision, checkpoint).unwrap();
    network::drain_player_bridge(app.world_mut());
    network::teardown(app.world_mut());
    let captured = capture(app.world_mut()).unwrap();
    let mut store = SnapshotStore::open(&directory.path().join("state")).unwrap();
    store.save(&captured).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    network::validate_player_snapshot(&loaded, Some(&prepared)).unwrap();
    let mut reopened = build_app(&config, &loaded).unwrap();
    let saved = capture(reopened.world_mut()).unwrap();
    assert_eq!(
        saved
            .native_players
            .unwrap()
            .projection(key)
            .unwrap()
            .checkpoint,
        checkpoint
    );
}

#[test]
fn reordered_growth_catalog_changes_context_without_changing_authored_json() {
    let (directory, config) = inputs();
    let gameplay_path = config.authored_gameplay.as_ref().unwrap();
    let gameplay_bytes = fs::read(gameplay_path).unwrap();
    let gameplay: serde_json::Value = serde_json::from_slice(&gameplay_bytes).unwrap();
    assert!(gameplay["tables"].get("D_CharacterGrowth").is_none());
    let (_, original) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    let original = original.unwrap();
    assert_eq!(original.initial_vitals.growth_row, 1);
    let snapshot = network::fresh_player_snapshot(Some(&original));
    let mut changed = config.clone();
    let reordered = directory.path().join("reordered-growth.pak");
    fs::write(
        &reordered,
        pak_writer::fresh_join_pak_with(&[(
            "Character/D_CharacterGrowth.json",
            pak_writer::table_json(&["Player", "Creature"]),
        )]),
    )
    .unwrap();
    changed.data_pak = Some(reordered);
    // Preparation succeeds from exactly the same authored JSON and pin.
    let (_, prepared) = network::prepare_run(&changed, network::peer_lab_config(&changed)).unwrap();
    let prepared = prepared.unwrap();
    assert_eq!(fs::read(gameplay_path).unwrap(), gameplay_bytes);
    assert_eq!(prepared.initial_vitals.growth_row, 0);
    assert_eq!(
        prepared.initial_vitals.max_health,
        original.initial_vitals.max_health
    );
    assert!(network::validate_player_snapshot(&snapshot, Some(&prepared)).is_err());
}

#[test]
fn actual_offline_runner_creates_reopens_and_refuses_unconfigured_player_store() {
    let (directory, network_config) = inputs();
    let state = directory.path().join("runner-state");
    let config_path = directory.path().join("runner.toml");
    let mut config = ServerConfig {
        data_dir: state.clone(),
        network: network_config.clone(),
        ..Default::default()
    };
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    // Reuse the suite's bounded subprocess owner; Drop kills/waits on failure.
    let run = || {
        let mut server = RunningServer::spawn(
            Command::new(env!("CARGO_BIN_EXE_icarus-server"))
                .arg("run")
                .arg("--config")
                .arg(&config_path)
                .args(["--compressed", "--max-ticks", "3"]),
        );
        let status = server.wait_exit();
        server.drain();
        (status, server.seen.clone())
    };
    let (status, records) = run();
    assert!(status.success(), "{records:?}");
    let started = records
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["event"] == "started")
        .unwrap();
    assert_eq!(started["native_schema"], SNAPSHOT_SCHEMA_PLAYERS);
    assert_eq!(started["network_enabled"], false);
    let (_, prepared) =
        network::prepare_run(&network_config, network::peer_lab_config(&network_config)).unwrap();
    let prepared = prepared.unwrap();
    let key = PlayerKey::new(76_561_197_960_265_770, 3).unwrap();
    let checkpoint = {
        let mut store = SnapshotStore::open(&state).unwrap();
        let mut snapshot = store.load().unwrap().unwrap().snapshot;
        assert_eq!(snapshot.tick, 3);
        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_PLAYERS);
        let mut registry = PlayerRegistry::new(
            snapshot.native_players.take().unwrap(),
            &prepared.context,
            8,
        )
        .unwrap();
        registry
            .reserve(
                key,
                1,
                PlayerCheckpoint {
                    location_cm: [100.0, 200.0, 300.0],
                    vitals: prepared.initial_vitals,
                },
            )
            .unwrap();
        let first = registry.activate(1).unwrap();
        let mut changed = first.checkpoint;
        changed.vitals.health = 37;
        changed.location_cm = [700.0, 800.0, 900.0];
        let accepted = registry.edit(key, first.revision, changed).unwrap();
        snapshot.native_players = Some(registry.snapshot());
        store.save(&snapshot).unwrap();
        accepted
    };
    let (status, records) = run();
    assert!(status.success(), "{records:?}");
    {
        let store = SnapshotStore::open(&state).unwrap();
        let snapshot = store.load().unwrap().unwrap().snapshot;
        assert_eq!(snapshot.tick, 6, "runner resumes the persisted tick");
        let reopened = snapshot.native_players.unwrap().projection(key).unwrap();
        assert_eq!(reopened.player_id, checkpoint.player_id);
        assert_eq!(reopened.revision, checkpoint.revision);
        assert_eq!(reopened.checkpoint, checkpoint.checkpoint);
    }
    let manifest_path = state.join("manifest.json");
    let manifest_before = fs::read(&manifest_path).unwrap();
    config.network.player_persistence = None;
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let (status, records) = run();
    assert!(!status.success());
    assert!(
        records
            .iter()
            .any(|line| line.contains("player persistence"))
    );
    assert!(
        !records
            .iter()
            .any(|line| line.contains("\"event\":\"started\""))
    );
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
    let store = SnapshotStore::open(&state).unwrap();
    let unchanged = store.load().unwrap().unwrap().snapshot;
    assert_eq!(unchanged.tick, 6);
    assert_eq!(
        unchanged
            .native_players
            .unwrap()
            .projection(key)
            .unwrap()
            .checkpoint,
        checkpoint.checkpoint
    );
}

#[test]
fn authored_clock_recipe_rejects_clockless_reopen_and_changed_declared_rate() {
    use icarus_bevy::authored_world_bootstrap::AuthoredClockConfig;
    let (_directory, mut config) = inputs();
    let (_, clockless) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    let old = network::fresh_player_snapshot(clockless.as_ref());
    config.authored_world.as_mut().unwrap().clock = Some(AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: 3600,
        tick_hz: 30,
    });
    let (_, clocked) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    assert!(network::validate_player_snapshot(&old, clocked.as_ref()).is_err());
    let snapshot = network::fresh_player_snapshot(clocked.as_ref());
    config
        .authored_world
        .as_mut()
        .unwrap()
        .clock
        .as_mut()
        .unwrap()
        .tick_hz = 31;
    let (_, changed) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    assert!(network::validate_player_snapshot(&snapshot, changed.as_ref()).is_err());
    let server = ServerConfig {
        network: config,
        tick_hz: 30,
        ..Default::default()
    };
    assert!(
        server.validate().is_err(),
        "offline global rate cannot reinterpret declared ticks"
    );
}

#[test]
fn authored_clock_prebind_requires_restored_owner_and_actual_fixed_timestep() {
    use icarus_bevy::{
        authored_world_bootstrap::AuthoredClockConfig, native_hosting::HostingConfig,
        simulation::SimulationClock,
    };
    let (directory, mut config) = inputs();
    let pak = directory.path().join("clock-prospect.pak");
    fs::write(
        &pak,
        pak_writer::fresh_join_pak_with(&[(
            "Prospects/D_ProspectList.json",
            pak_writer::table_json(&["AuthoredWorkshop"]),
        )]),
    )
    .unwrap();
    config.data_pak = Some(pak);
    config.enabled = true;
    config.hosting = Some(HostingConfig {
        query_bind: "127.0.0.1".parse().unwrap(),
        query_port: 27015,
        private_ip: "127.0.0.1".parse().unwrap(),
        seconds: 60,
        name: "Authored clock".into(),
        map_url: "/Authored/Maps/Workshop".into(),
        prospect_id: "authored-checkpoint".into(),
        prospect_row: "AuthoredWorkshop".into(),
        lobby_name: "Clock".into(),
        difficulty: 2,
    });
    config.authored_world.as_mut().unwrap().clock = Some(AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: 3600,
        tick_hz: 30,
    });
    let (session, prepared) =
        network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    let mut session = session.unwrap();
    let snapshot = WorldSnapshot {
        tick: 27_000,
        ..network::fresh_player_snapshot(prepared.as_ref())
    };
    let server = ServerConfig {
        network: config,
        ..Default::default()
    };
    let mut app = build_app(&server, &snapshot).unwrap();
    app.world_mut()
        .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
        .set_timestep(Duration::from_millis(50));
    let error = session.prepare_app(&mut app).unwrap_err();
    assert!(format!("{error:#}").contains("fixed timestep mismatch"));
    assert!(session.session_mut().lab.local_addr().is_err());
    app.world_mut()
        .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
        .set_timestep(server.tick_interval());
    app.world_mut().remove_resource::<SimulationClock>();
    let error = session.prepare_app(&mut app).unwrap_err();
    assert!(format!("{error:#}").contains("requires SimulationClock"));
    assert!(session.session_mut().lab.local_addr().is_err());
    app.insert_resource(SimulationClock {
        tick: snapshot.tick,
    });
    session.prepare_app(&mut app).unwrap();
    assert!(
        session.session_mut().lab.local_addr().is_err(),
        "preparation opens no listener"
    );
}

#[test]
fn consumption_rates_and_simulation_rate_bind_only_enabled_recipes() {
    use icarus_bevy::native_player_runtime::ConsumptionConfig;
    let (_directory, mut config) = inputs();
    let prepare = |config: &NetworkConfig, hz| {
        network::prepare_run_with_tick_hz(config, network::peer_lab_config(config), hz)
            .unwrap()
            .1
            .unwrap()
    };
    let disabled = prepare(&config, 30);
    assert_eq!(disabled.context, prepare(&config, 7).context);
    config.player_consumption = Some(ConsumptionConfig {
        oxygen: 1,
        water: 2,
        food: 3,
    });
    let enabled = prepare(&config, 30);
    assert_ne!(disabled.context, enabled.context);
    let snapshot = network::fresh_player_snapshot(Some(&enabled));
    assert!(network::validate_player_snapshot(&snapshot, Some(&prepare(&config, 7))).is_err());
    config.player_consumption.as_mut().unwrap().food = 4;
    assert!(network::validate_player_snapshot(&snapshot, Some(&prepare(&config, 30))).is_err());
    config.player_consumption = None;
    assert!(network::validate_player_snapshot(&snapshot, Some(&prepare(&config, 30))).is_err());
}

#[test]
fn scheduled_consumption_preserves_fractional_phase_across_durable_restart() {
    use bevy::prelude::FixedUpdate;
    use icarus_bevy::native_player_runtime::ConsumptionConfig;
    let (directory, mut network_config) = inputs();
    network_config.player_consumption = Some(ConsumptionConfig {
        oxygen: 2,
        water: 3,
        food: 4,
    });
    let (_, prepared) = network::prepare_run_with_tick_hz(
        &network_config,
        network::peer_lab_config(&network_config),
        7,
    )
    .unwrap();
    let prepared = prepared.unwrap();
    let config = ServerConfig {
        tick_hz: 7,
        network: network_config.clone(),
        ..Default::default()
    };
    let mut app = build_app(&config, &network::fresh_player_snapshot(Some(&prepared))).unwrap();
    let key = PlayerKey::new(1, 0).unwrap();
    let initial = PlayerCheckpoint {
        location_cm: [100.0, 200.0, 300.0],
        vitals: prepared.initial_vitals,
    };
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        registry.reserve(key, 1, initial).unwrap();
        registry.activate(1).unwrap();
    }
    for _ in 0..3 {
        app.world_mut().run_schedule(FixedUpdate);
    }
    let partial = capture(app.world_mut()).unwrap();
    assert_eq!(partial.tick, 3);
    let mut store = SnapshotStore::open(&directory.path().join("consumption-state")).unwrap();
    store.save(&partial).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    network::validate_player_snapshot(&loaded, Some(&prepared)).unwrap();
    let mut restored = build_app(&config, &loaded).unwrap();
    // Restart reserves, but does not activate: no accumulated offline charge.
    restored
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key, 1, initial)
        .unwrap();
    for _ in 0..7 {
        restored.world_mut().run_schedule(FixedUpdate);
    }
    assert_eq!(
        capture(restored.world_mut()).unwrap().native_players,
        partial.native_players
    );
    restored
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .activate(1)
        .unwrap();
    for _ in 0..4 {
        restored.world_mut().run_schedule(FixedUpdate);
    }
    let final_state = capture(restored.world_mut())
        .unwrap()
        .native_players
        .unwrap()
        .projection(key)
        .unwrap()
        .checkpoint;
    // Three active ticks, a whole idle second, then four active ticks span one
    // complete phase: exactly one second of authored units, no float rounding.
    assert_eq!(final_state.vitals.oxygen, initial.vitals.oxygen - 2);
    assert_eq!(final_state.vitals.water, initial.vitals.water - 3);
    assert_eq!(final_state.vitals.food, initial.vitals.food - 4);
    assert_eq!(final_state.location_cm, initial.location_cm);
    assert_eq!(final_state.vitals.health, initial.vitals.health);
}

#[test]
fn consumption_configuration_refuses_missing_owner_and_invalid_rates() {
    use icarus_bevy::native_player_runtime::ConsumptionConfig;
    let (_directory, mut config) = inputs();
    for values in [(0, 0, 0), (-1, 1, 1), (1, 10_000_001, 1)] {
        config.player_consumption = Some(ConsumptionConfig {
            oxygen: values.0,
            water: values.1,
            food: values.2,
        });
        assert!(config.validate().is_err());
    }
    config.player_consumption = Some(ConsumptionConfig {
        oxygen: 1,
        water: 0,
        food: 0,
    });
    config.validate().unwrap();
    config.player_persistence = None;
    assert!(config.validate().is_err());
}

#[test]
fn failed_consumption_remains_closed_after_registry_returns_and_refuses_capture() {
    use bevy::prelude::FixedUpdate;
    use icarus_bevy::native_player_runtime::{ConsumptionConfig, consumption_runtime_health};
    let (_directory, mut network_config) = inputs();
    network_config.player_consumption = Some(ConsumptionConfig {
        oxygen: 1,
        water: 1,
        food: 1,
    });
    assert!(
        network::prepare_run(&network_config, network::peer_lab_config(&network_config)).is_err()
    );
    let (_, prepared) = network::prepare_run_with_tick_hz(
        &network_config,
        network::peer_lab_config(&network_config),
        30,
    )
    .unwrap();
    let config = ServerConfig {
        network: network_config,
        ..Default::default()
    };
    let mut app = build_app(&config, &network::fresh_player_snapshot(prepared.as_ref())).unwrap();
    let registry = app.world_mut().remove_resource::<PlayerRegistry>().unwrap();
    app.world_mut().run_schedule(FixedUpdate);
    assert!(consumption_runtime_health(app.world()).is_err());
    app.world_mut().insert_resource(registry);
    app.world_mut().run_schedule(FixedUpdate);
    assert!(consumption_runtime_health(app.world()).is_err());
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn combined_clock_and_consumption_share_durable_tick_phase_and_activation() {
    use bevy::prelude::FixedUpdate;
    use icarus_bevy::{
        authored_world_bootstrap::AuthoredClockConfig, native_player_runtime::ConsumptionConfig,
        simulation::SimulationClock,
    };
    let (directory, mut network_config) = inputs();
    let clock = AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: 2,
        tick_hz: 7,
    };
    network_config.authored_world.as_mut().unwrap().clock = Some(clock);
    network_config.player_consumption = Some(ConsumptionConfig {
        oxygen: 2,
        water: 3,
        food: 4,
    });
    let (_, prepared) = network::prepare_run_with_tick_hz(
        &network_config,
        network::peer_lab_config(&network_config),
        7,
    )
    .unwrap();
    let prepared = prepared.unwrap();
    let server = ServerConfig {
        tick_hz: 7,
        network: network_config,
        ..Default::default()
    };
    let mut app = build_app(&server, &network::fresh_player_snapshot(Some(&prepared))).unwrap();
    let key = PlayerKey::new(2, 0).unwrap();
    let initial = PlayerCheckpoint {
        location_cm: [100.0, 200.0, 300.0],
        vitals: prepared.initial_vitals,
    };
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        registry.reserve(key, 1, initial).unwrap();
        registry.activate(1).unwrap();
    }
    for _ in 0..3 {
        app.world_mut().run_schedule(FixedUpdate);
    }
    let partial = capture(app.world_mut()).unwrap();
    assert_eq!(partial.tick, 3);
    // The same public projection used by network preparation reads completed
    // ticks, while scheduled consumption charges the preceding tick intervals.
    let plan = clock.prepare().unwrap();
    assert_eq!(plan.sample(partial.tick), (5520.0_f64 / 7.0) as f32);
    let mut store =
        SnapshotStore::open(&directory.path().join("combined-clock-consumption")).unwrap();
    store.save(&partial).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    network::validate_player_snapshot(&loaded, Some(&prepared)).unwrap();
    assert_eq!(loaded.tick, partial.tick);
    assert_eq!(
        plan.sample(loaded.tick).to_bits(),
        plan.sample(partial.tick).to_bits()
    );
    let mut restored = build_app(&server, &loaded).unwrap();
    restored
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key, 1, initial)
        .unwrap();
    // The world clock advances during a pending reconnect, but no vital charge
    // is incurred. One idle second preserves the later consumption phase.
    for _ in 0..7 {
        restored.world_mut().run_schedule(FixedUpdate);
    }
    let idle = capture(restored.world_mut()).unwrap();
    assert_eq!(idle.tick, 10);
    assert_eq!(plan.sample(idle.tick), (480.0_f64 / 7.0) as f32);
    assert_eq!(idle.native_players, partial.native_players);
    restored
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .activate(1)
        .unwrap();
    for _ in 0..4 {
        restored.world_mut().run_schedule(FixedUpdate);
    }
    let finished = capture(restored.world_mut()).unwrap();
    assert_eq!(finished.tick, 14);
    assert_eq!(
        restored.world().resource::<SimulationClock>().tick,
        finished.tick
    );
    assert_eq!(plan.sample(finished.tick), 480.0);
    let mut expected = initial;
    expected.vitals.oxygen -= 2;
    expected.vitals.water -= 3;
    expected.vitals.food -= 4;
    let saved = finished
        .native_players
        .as_ref()
        .unwrap()
        .projection(key)
        .unwrap();
    assert_eq!(saved.checkpoint, expected);
    // Confirm the final joint result, not just the intermediate phase, survives
    // the generation store and still satisfies the exact enabled recipe.
    store.save(&finished).unwrap();
    let reopened = store.load().unwrap().unwrap().snapshot;
    network::validate_player_snapshot(&reopened, Some(&prepared)).unwrap();
    assert_eq!(reopened.tick, finished.tick);
    assert_eq!(reopened.native_players, finished.native_players);
    assert_eq!(plan.sample(reopened.tick), 480.0);
}
