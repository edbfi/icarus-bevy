//! Source-authored checkpoint preflight and offline retention contracts.
use super::*;
use icarus_bevy::{
    authored_world_bootstrap::AuthoredWorldBootstrap,
    config::PlayerPersistenceConfig,
    data_pak::DataPak,
    experimental::release::ReleaseProfile,
    gameplay_profile::{FreshPlayerValues, fresh_player_profile},
    native_player_runtime::PlayerRegistry,
    native_players::{PlayerCheckpoint, PlayerKey},
    network::PreparedPlayers,
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
    for schema in 1..SNAPSHOT_SCHEMA_PLAYERS {
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
fn changed_world_or_authored_input_refuses_naming_it() {
    let (_directory, config) = inputs();
    let (_, snapshot) = stored_players(&config, &[(3, "Player")]);
    for (variant, named) in [
        ("prospect", "network.player_persistence.prospect_id"),
        ("map", "network.player_persistence.map_url"),
        (
            "row",
            "player store input changed: network.player_persistence differs",
        ),
        (
            "difficulty",
            "player store input changed: network.player_persistence differs",
        ),
        ("spawn", "player store input changed: network.spawn differs"),
        (
            "world",
            "player store input changed: network.authored_world differs",
        ),
        (
            "bytes",
            "player store input changed: network.authored_gameplay differs",
        ),
    ] {
        // Unchanged inputs open, so each refusal below is its input's.
        let (_, same) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
        network::open_player_snapshot(&mut snapshot.clone(), same.as_ref()).unwrap();
        let mut changed = config.clone();
        match variant {
            "prospect" => changed
                .player_persistence
                .as_mut()
                .unwrap()
                .prospect_id
                .push('x'),
            "map" => {
                changed.player_persistence.as_mut().unwrap().map_url = "/Authored/Maps/Other".into()
            }
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
        let mut refused = snapshot.clone();
        let error = network::open_player_snapshot(&mut refused, other.as_ref()).unwrap_err();
        assert!(format!("{error:#}").contains(named), "{variant}: {error:#}");
        assert_eq!(refused, snapshot, "{variant}: a refusal changes nothing");
        if variant == "bytes" {
            let path = config.authored_gameplay.as_ref().unwrap();
            let mut bytes = fs::read(path).unwrap();
            bytes.pop();
            fs::write(path, bytes).unwrap();
        }
    }
}

/// Prepare `config`, commit one player per `(slot, growth row name)` with
/// distinct vitals and positions, publish it and load it back as a stored
/// generation, as `run` would on its next start.
fn stored_players(
    config: &NetworkConfig,
    players: &[(i32, &str)],
) -> (PreparedPlayers, WorldSnapshot) {
    let (_, prepared) = network::prepare_run(config, network::peer_lab_config(config)).unwrap();
    let prepared = prepared.unwrap();
    let mut fresh = network::fresh_player_snapshot(Some(&prepared));
    let mut registry =
        PlayerRegistry::new(fresh.native_players.take().unwrap(), &prepared.context, 8).unwrap();
    let rows = growth_rows(config);
    for (epoch, (slot, row)) in (1..).zip(players) {
        let mut vitals = prepared.initial_vitals;
        vitals.growth_row = rows.iter().position(|name| name == row).unwrap() as u8;
        vitals.health = 10 + *slot;
        let key = PlayerKey::new(76_561_197_960_265_770, *slot).unwrap();
        let location = f64::from(*slot);
        registry
            .reserve(
                key,
                epoch,
                PlayerCheckpoint {
                    location_cm: [location, 2.0 * location, 3.0 * location],
                    vitals,
                },
            )
            .unwrap();
        registry.activate(epoch).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(state.path()).unwrap();
    store
        .save(&WorldSnapshot {
            native_players: Some(registry.snapshot()),
            ..fresh
        })
        .unwrap();
    (prepared, store.load().unwrap().unwrap().snapshot)
}

fn growth_rows(config: &NetworkConfig) -> Vec<String> {
    let gameplay = fs::read(config.authored_gameplay.as_ref().unwrap()).unwrap();
    icarus_bevy::data_pak::DataTables::runtime_bytes(config.data_pak.as_ref().unwrap(), &gameplay)
        .unwrap()
        .rows(network::CHARACTER_GROWTH_TABLE)
        .unwrap()
        .to_vec()
}

fn with_growth_rows(
    config: &NetworkConfig,
    directory: &std::path::Path,
    rows: &[&str],
) -> NetworkConfig {
    let pak = directory.join(format!("growth-{}.pak", rows.join("-")));
    fs::write(
        &pak,
        pak_writer::fresh_join_pak_with(&[(
            "Character/D_CharacterGrowth.json",
            pak_writer::table_json(rows),
        )]),
    )
    .unwrap();
    NetworkConfig {
        data_pak: Some(pak),
        ..config.clone()
    }
}

/// Checkpoints of a stored snapshot opened against the inputs it was written with.
fn opened_checkpoints(
    stored: &WorldSnapshot,
    prepared: &PreparedPlayers,
    slots: &[i32],
) -> Vec<PlayerCheckpoint> {
    let mut snapshot = stored.clone();
    network::open_player_snapshot(&mut snapshot, Some(prepared)).unwrap();
    checkpoints(&snapshot, slots)
}

fn checkpoints(snapshot: &WorldSnapshot, slots: &[i32]) -> Vec<PlayerCheckpoint> {
    let players = snapshot.native_players.as_ref().unwrap();
    slots
        .iter()
        .map(|slot| {
            let key = PlayerKey::new(76_561_197_960_265_770, *slot).unwrap();
            players.projection(key).unwrap().checkpoint
        })
        .collect()
}

#[test]
fn store_written_under_one_release_reopens_under_another_with_identical_players() {
    let (_directory, mut config) = inputs();
    let (older, newer) = (
        ReleaseProfile::fixture_path("3.0.35"),
        ReleaseProfile::fixture_path("3.0.36"),
    );
    config.release_profile = Some(older.clone());
    let (written, mut snapshot) = stored_players(&config, &[(3, "Player"), (4, "Creature")]);
    let before = opened_checkpoints(&snapshot, &written, &[3, 4]);
    let older_pin = ReleaseProfile::fixture("3.0.35").pin;
    let newer_pin = ReleaseProfile::fixture("3.0.36").pin;
    assert_eq!(written.release, older_pin);
    assert_ne!(fs::read(&older).unwrap(), fs::read(&newer).unwrap());
    assert_ne!(older_pin.build_id, newer_pin.build_id);
    assert_ne!(older_pin.project_version, newer_pin.project_version);
    assert_ne!(older_pin.network_checksum, newer_pin.network_checksum);
    let players = snapshot.native_players.as_ref().unwrap();
    assert_eq!(players.written_under(), Some(&older_pin));

    config.release_profile = Some(newer);
    let (_, prepared) = network::prepare_run(&config, network::peer_lab_config(&config)).unwrap();
    let prepared = prepared.unwrap();
    let last = network::open_player_snapshot(&mut snapshot, Some(&prepared)).unwrap();
    assert_eq!(last, Some(older_pin), "provenance reports the old release");
    assert_eq!(checkpoints(&snapshot, &[3, 4]), before);
    let players = snapshot.native_players.as_ref().unwrap();
    assert_eq!(players.written_under(), Some(&newer_pin));
    assert_eq!(players.context(), &prepared.context);
    // The opened store installs and captures under the new release.
    let server = ServerConfig {
        network: config.clone(),
        ..Default::default()
    };
    let mut app = build_app(&server, &snapshot).unwrap();
    network::install_player_registry(app.world_mut(), Some(&prepared), &config).unwrap();
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(checkpoints(&captured, &[3, 4]), before);
    let state = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(state.path()).unwrap();
    store.save(&captured).unwrap();
    let reloaded = store.load().unwrap().unwrap().snapshot;
    let players = reloaded.native_players.as_ref().unwrap();
    assert_eq!(players.written_under(), Some(&newer_pin));
}

#[test]
fn inserted_and_reordered_growth_rows_resolve_stored_rows_by_name() {
    let (directory, config) = inputs();
    let gameplay_path = config.authored_gameplay.as_ref().unwrap();
    let gameplay_bytes = fs::read(gameplay_path).unwrap();
    let gameplay: serde_json::Value = serde_json::from_slice(&gameplay_bytes).unwrap();
    assert!(gameplay["tables"].get("D_CharacterGrowth").is_none());
    assert_eq!(growth_rows(&config), ["Creature", "Player"]);
    let (original, stored) = stored_players(&config, &[(3, "Player"), (4, "Creature")]);
    assert_eq!(original.initial_vitals.growth_row, 1);
    let before = opened_checkpoints(&stored, &original, &[3, 4]);
    assert_eq!(
        before
            .iter()
            .map(|c| c.vitals.growth_row)
            .collect::<Vec<_>>(),
        [1, 0]
    );
    for rows in [
        &["Inserted_A", "Inserted_B", "Creature", "Player"][..],
        &["Player", "Inserted", "Creature"][..],
    ] {
        let changed = with_growth_rows(&config, directory.path(), rows);
        let (_, prepared) =
            network::prepare_run(&changed, network::peer_lab_config(&changed)).unwrap();
        let prepared = prepared.unwrap();
        let installed = growth_rows(&changed);
        assert_eq!(installed, rows);
        let mut snapshot = stored.clone();
        network::open_player_snapshot(&mut snapshot, Some(&prepared)).unwrap();
        let after = checkpoints(&snapshot, &[3, 4]);
        for ((was, now), name) in before.iter().zip(&after).zip(["Player", "Creature"]) {
            assert_eq!(installed[usize::from(now.vitals.growth_row)], name);
            let mut moved = *was;
            moved.vitals.growth_row = now.vitals.growth_row;
            assert_eq!(*now, moved, "only the row index moves");
        }
        assert_eq!(
            prepared.initial_vitals.growth_row,
            after[0].vitals.growth_row
        );
        // The authored gameplay is unchanged; only the installed rows moved.
        assert_eq!(fs::read(gameplay_path).unwrap(), gameplay_bytes);
    }
}

#[test]
fn removed_growth_row_refuses_naming_table_and_row_without_changes() {
    let (directory, config) = inputs();
    let (_, stored) = stored_players(&config, &[(3, "Player"), (4, "Creature")]);
    let changed = with_growth_rows(&config, directory.path(), &["Player", "Renamed"]);
    let (_, prepared) = network::prepare_run(&changed, network::peer_lab_config(&changed)).unwrap();
    let mut snapshot = stored.clone();
    let error = network::open_player_snapshot(&mut snapshot, prepared.as_ref()).unwrap_err();
    assert_eq!(
        format!("{error:#}"),
        "player store references D_CharacterGrowth row \"Creature\", which the installed game data no longer has; restore data with that row or move the state directory aside to start a fresh store"
    );
    assert_eq!(
        snapshot, stored,
        "a refused store is never reset or guessed"
    );
}

#[test]
fn ambiguous_installed_growth_rows_refuse_startup_naming_table_and_reason() {
    let (directory, config) = inputs();
    let changed = with_growth_rows(&config, directory.path(), &["Creature", "Player", "Player"]);
    let error = network::prepare_run(&changed, network::peer_lab_config(&changed))
        .err()
        .unwrap();
    assert_eq!(
        format!("{error:#}"),
        "installed game data table D_CharacterGrowth cannot back player checkpoints: row name \"Player\" appears more than once; checkpoints store rows by name, so names must be non-empty, unique and bounded"
    );
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
    let mut loaded = store.load().unwrap().unwrap().snapshot;
    network::open_player_snapshot(&mut loaded, Some(&prepared)).unwrap();
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
        network::open_player_snapshot(&mut snapshot, Some(&prepared)).unwrap();
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
    let reopened = records
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["event"] == "player_store_reopened")
        .unwrap();
    assert_eq!(reopened["build_id"], prepared.release.build_id);
    assert_eq!(reopened["last_written_build_id"], prepared.release.build_id);
    {
        let store = SnapshotStore::open(&state).unwrap();
        let mut snapshot = store.load().unwrap().unwrap().snapshot;
        assert_eq!(snapshot.tick, 6, "runner resumes the persisted tick");
        network::open_player_snapshot(&mut snapshot, Some(&prepared)).unwrap();
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
    let mut unchanged = store.load().unwrap().unwrap().snapshot;
    assert_eq!(unchanged.tick, 6);
    network::open_player_snapshot(&mut unchanged, Some(&prepared)).unwrap();
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
fn actual_offline_runner_refuses_release_bound_store_and_leaves_it_untouched() {
    use sha2::{Digest, Sha256};
    let (directory, network_config) = inputs();
    let state = directory.path().join("release-bound-state");
    fs::create_dir(&state).unwrap();
    // A store written by the retired release-bound player schema.
    let generation = format!(
        concat!(
            r#"{{"schema_version":13,"tick":3,"entities":[],"native_players":{{"context":{{"#,
            r#""prospect_id":[65],"map_url":"/Authored/Maps/Workshop","app_id":2089300,"#,
            r#""build_id":7,"project_version":"authored","network_checksum":{},"#,
            r#""recipe_sha256":{:?}}},"high_water":0,"records":[]}}}}"#
        ),
        icarus_bevy::experimental::release::network_checksum("authored"),
        [1_u8; 32],
    );
    let digest: String = Sha256::digest(generation.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let manifest =
        format!(r#"{{"schema_version":1,"current":{{"number":1,"sha256":"{digest}"}}}}"#);
    fs::write(state.join("snapshot-1.json"), &generation).unwrap();
    fs::write(state.join("manifest.json"), &manifest).unwrap();
    let config_path = directory.path().join("release-bound.toml");
    let config = ServerConfig {
        data_dir: state.clone(),
        network: network_config,
        ..Default::default()
    };
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let mut server = RunningServer::spawn(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(&config_path)
            .args(["--compressed", "--max-ticks", "3"]),
    );
    let status = server.wait_exit();
    server.drain();
    assert!(!status.success(), "{:?}", server.seen);
    let fatal = server
        .seen
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["event"] == "fatal")
        .unwrap();
    let error = fatal["error"].as_str().unwrap();
    assert!(
        error.starts_with(
            "player store schema 13 predates the release-independent player store format (schema 14) and cannot be verified"
        ),
        "{error}"
    );
    assert!(
        !server
            .seen
            .iter()
            .any(|line| line.contains("\"event\":\"started\""))
    );
    assert_eq!(
        fs::read(state.join("snapshot-1.json")).unwrap(),
        generation.as_bytes()
    );
    assert_eq!(
        fs::read(state.join("manifest.json")).unwrap(),
        manifest.as_bytes()
    );
}
