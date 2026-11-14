//! Authored checkpoint authority and storage, not client or gameplay qualification.
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        pawn_open::CharacterVitals,
        release::{ReleasePin, network_checksum},
    },
    native_player_runtime::{PlayerRegistry, install},
    native_players::{
        InstalledRowsRefusal, MAX_PLAYERS_PER_ACCOUNT, PlayerCheckpoint, PlayerContext,
        PlayerError, PlayerInputs, PlayerKey, PlayerSnapshot, RowsDefect, StoreRefusal,
        check_installed_growth_rows,
    },
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_PLAYERS, WorldSnapshot, build_app,
        capture,
    },
};
use std::time::{Duration, Instant};

fn inputs(byte: u8) -> PlayerInputs {
    PlayerInputs {
        authored_gameplay: [byte; 32],
        spawn: [byte; 32],
        authored_world: [byte; 32],
        declaration: [byte; 32],
        initial_values: [byte; 32],
    }
}
fn rows() -> Vec<String> {
    vec!["Player".into()]
}
fn context() -> PlayerContext {
    context_with_rows(rows())
}
fn context_with_rows(rows: Vec<String>) -> PlayerContext {
    PlayerContext::new(
        "authored-prospect".encode_utf16().collect(),
        "/Authored/Checkpoint".into(),
        inputs(1),
        rows,
    )
    .unwrap()
}
fn checkpoint() -> PlayerCheckpoint {
    PlayerCheckpoint {
        location_cm: [100., 200., 300.],
        vitals: CharacterVitals {
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
        },
    }
}
fn snapshot() -> WorldSnapshot {
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(PlayerSnapshot::empty(context())),
        entities: vec![EntityRecord {
            id: DomainId(42),
            position: Position([1., 2., 3.]),
        }],
        ..Default::default()
    }
}
fn registry() -> PlayerRegistry {
    PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 64).unwrap()
}

#[test]
fn checkpoint_survives_capture_teardown_independence_and_same_generation_reload() {
    let key = PlayerKey::new(123456789, 3).unwrap();
    let mut app = build_app(&ServerConfig::default(), &snapshot()).unwrap();
    install(app.world_mut(), &context(), 2).unwrap();
    {
        let mut players = app.world_mut().resource_mut::<PlayerRegistry>();
        let initial = players.reserve(key, 1, checkpoint()).unwrap();
        assert!(initial.is_new);
        assert_eq!(players.snapshot().len(), 0);
        players.activate(1).unwrap();
        let mut next = checkpoint();
        next.vitals.health = 37;
        next.location_cm = [400., 500., 600.];
        assert_eq!(players.edit(key, 1, next).unwrap().revision, 2);
        players.detach(1);
    }
    let state = capture(app.world_mut()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    {
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        assert_eq!(store.save(&state).unwrap().generation, 1);
    }
    let store = SnapshotStore::open_existing(directory.path()).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded.entities, state.entities);
    assert_eq!(loaded.native_players, state.native_players);
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    let mut players = app.world_mut().resource_mut::<PlayerRegistry>();
    let restored = players.reserve(key, 1, checkpoint()).unwrap();
    assert!(!restored.is_new);
    assert_eq!(restored.revision, 2);
    assert_eq!(restored.checkpoint.vitals.health, 37);
    assert_eq!(restored.checkpoint.location_cm, [400., 500., 600.]);
}

#[test]
fn atomic_revisions_reservations_and_epoch_conflicts_preserve_records() {
    let key = PlayerKey::new(1, 0).unwrap();
    let mut players = registry();
    let first = players.reserve(key, 1, checkpoint()).unwrap();
    assert_eq!(players.reserve(key, 1, checkpoint()).unwrap(), first);
    assert_eq!(
        players.reserve(key, 2, checkpoint()),
        Err(PlayerError::Conflict)
    );
    players.activate(1).unwrap();
    let before = players.snapshot();
    let mut bad = checkpoint();
    bad.vitals.health = 101;
    assert_eq!(players.edit(key, 1, bad), Err(PlayerError::InvalidState));
    assert_eq!(
        players.edit(key, 0, checkpoint()),
        Err(PlayerError::StaleRevision)
    );
    let mut changed = checkpoint();
    changed.vitals.max_health = 200;
    assert_eq!(
        players.edit(key, 1, changed),
        Err(PlayerError::InvalidContext)
    );
    assert_eq!(players.snapshot(), before);
    players.detach(1);
    assert_eq!(
        players.reserve(key, 1, checkpoint()),
        Err(PlayerError::Conflict)
    );
    let restored = players.reserve(key, 2, checkpoint()).unwrap();
    assert_eq!(restored.player_id, first.player_id);
    assert!(!players.cancel(1));
    assert_eq!(players.projection(2).unwrap().revision, 1);
}

#[test]
fn per_account_limit_counts_pending_and_cancelled_ids_are_never_reused() {
    let mut players = registry();
    let first = players
        .reserve(PlayerKey::new(1, 0).unwrap(), 1, checkpoint())
        .unwrap();
    players.cancel(1);
    for slot in 0..MAX_PLAYERS_PER_ACCOUNT {
        players
            .reserve(
                PlayerKey::new(1, slot as i32).unwrap(),
                slot as u64 + 2,
                checkpoint(),
            )
            .unwrap();
        players.activate(slot as u64 + 2).unwrap();
        players.detach(slot as u64 + 2);
    }
    let before = players.snapshot();
    assert_eq!(
        players.reserve(PlayerKey::new(1, 64).unwrap(), 100, checkpoint()),
        Err(PlayerError::Limit)
    );
    assert_eq!(players.snapshot(), before);
    let next = players
        .reserve(PlayerKey::new(2, 0).unwrap(), 101, checkpoint())
        .unwrap();
    assert!(next.player_id > first.player_id);
    assert!(
        players
            .reserve(PlayerKey::new(2, 1).unwrap(), 102, checkpoint())
            .is_ok()
    );
}

#[test]
fn capture_refuses_missing_or_unexpected_registry_instead_of_downgrading() {
    let mut app = build_app(&ServerConfig::default(), &snapshot()).unwrap();
    app.world_mut().remove_resource::<PlayerRegistry>();
    assert!(capture(app.world_mut()).is_err());
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    app.insert_resource(registry());
    assert!(capture(app.world_mut()).is_err());
    assert!(install(app.world_mut(), &context(), 0).is_err());
}

#[test]
fn legacy_schemas_reject_present_checkpoint_field_even_null() {
    let private = serde_json::to_value(PlayerSnapshot::empty(context())).unwrap();
    for schema in 1..=12 {
        for value in [serde_json::Value::Null, private.clone()] {
            let input = serde_json::json!({"schema_version":schema,"tick":0,"entities":[],"native_players":value});
            assert!(serde_json::from_value::<WorldSnapshot>(input).is_err());
        }
    }
    let legacy = WorldSnapshot {
        native_players: Some(PlayerSnapshot::empty(context())),
        ..Default::default()
    };
    assert!(legacy.validate().is_err());
}

#[test]
fn malformed_identity_duplicate_records_and_diagnostics_are_private() {
    let key = PlayerKey::new(123456789, 3).unwrap();
    let mut players = registry();
    players.reserve(key, 1, checkpoint()).unwrap();
    players.activate(1).unwrap();
    let good = serde_json::to_value(players.snapshot()).unwrap();
    for field in ["account", "slot", "id", "revision"] {
        let mut bad = good.clone();
        bad["records"][0][field] = serde_json::json!("SECRET_PLAYER_SENTINEL");
        let error = serde_json::from_value::<PlayerSnapshot>(bad)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("SECRET_PLAYER_SENTINEL"));
    }
    let mut duplicate = good.clone();
    let record = duplicate["records"][0].clone();
    duplicate["records"].as_array_mut().unwrap().push(record);
    assert!(serde_json::from_value::<PlayerSnapshot>(duplicate).is_err());
    for field in ["id", "revision"] {
        let mut bad = good.clone();
        bad["records"][0][field] = serde_json::json!(0);
        assert!(serde_json::from_value::<PlayerSnapshot>(bad).is_err());
    }
    assert!(!format!("{key:?} {:?} {:?}", players, players.snapshot()).contains("123456789"));
    assert!(PlayerKey::new(0, 0).is_err());
    assert!(PlayerKey::new(1, -1).is_err());
}

#[test]
fn worker_receipt_does_not_ack_later_in_memory_revision() {
    let key = PlayerKey::new(1, 0).unwrap();
    let mut players = registry();
    players.reserve(key, 1, checkpoint()).unwrap();
    players.activate(1).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = SnapshotStore::open(directory.path()).unwrap();
    let mut worker = SaveWorker::spawn(store).unwrap();
    let mut first = snapshot();
    first.native_players = Some(players.snapshot());
    worker.submit(first).unwrap();
    let mut next = checkpoint();
    next.vitals.health = 37;
    players.edit(key, 1, next).unwrap();
    let receipt = worker
        .finish_before(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.generation, 1);
    // Inspect the exact committed generation while the worker retains its store lock.
    let old: WorldSnapshot =
        serde_json::from_slice(&std::fs::read(directory.path().join("snapshot-1.json")).unwrap())
            .unwrap();
    assert_eq!(
        old.native_players
            .unwrap()
            .projection(key)
            .unwrap()
            .revision,
        1
    );
    let mut second = snapshot();
    second.native_players = Some(players.snapshot());
    worker.submit(second).unwrap();
    worker
        .finish_before(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let next: WorldSnapshot =
        serde_json::from_slice(&std::fs::read(directory.path().join("snapshot-2.json")).unwrap())
            .unwrap();
    assert_eq!(
        next.native_players
            .unwrap()
            .projection(key)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn total_bound_and_id_revision_exhaustion_are_atomic() {
    let key = PlayerKey::new(1, 0).unwrap();
    let mut players = registry();
    players.reserve(key, 1, checkpoint()).unwrap();
    players.activate(1).unwrap();
    let seed = serde_json::to_value(players.snapshot()).unwrap();
    let mut full = seed.clone();
    let records = (0..4096u64)
        .map(|index| {
            let mut record = seed["records"][0].clone();
            record["account"] = serde_json::json!(index + 1);
            record["id"] = serde_json::json!(index + 1);
            record
        })
        .collect::<Vec<_>>();
    full["high_water"] = serde_json::json!(4096);
    full["records"] = serde_json::json!(records);
    let snapshot = serde_json::from_value::<PlayerSnapshot>(full.clone()).unwrap();
    let mut registry = PlayerRegistry::new(snapshot.clone(), &context(), 64).unwrap();
    assert_eq!(
        registry.reserve(PlayerKey::new(5000, 0).unwrap(), 1, checkpoint()),
        Err(PlayerError::Limit)
    );
    assert_eq!(registry.snapshot(), snapshot);
    let duplicate = full["records"][0].clone();
    full["records"].as_array_mut().unwrap().push(duplicate);
    assert!(serde_json::from_value::<PlayerSnapshot>(full).is_err());
    let mut exhausted = seed.clone();
    exhausted["high_water"] = serde_json::json!(u64::MAX);
    let state = serde_json::from_value::<PlayerSnapshot>(exhausted).unwrap();
    let mut registry = PlayerRegistry::new(state.clone(), &context(), 64).unwrap();
    assert_eq!(
        registry.reserve(PlayerKey::new(2, 0).unwrap(), 1, checkpoint()),
        Err(PlayerError::Exhausted)
    );
    assert_eq!(registry.snapshot(), state);
    let mut exhausted = seed;
    exhausted["records"][0]["revision"] = serde_json::json!(u64::MAX);
    let state = serde_json::from_value::<PlayerSnapshot>(exhausted).unwrap();
    let mut registry = PlayerRegistry::new(state.clone(), &context(), 64).unwrap();
    assert_eq!(
        registry.edit(key, u64::MAX, checkpoint()),
        Err(PlayerError::Exhausted)
    );
    assert_eq!(registry.snapshot(), state);
}

#[test]
fn live_limit_context_and_checkpoint_bounds_refuse_without_state_changes() {
    let empty = PlayerSnapshot::empty(context());
    let mut players = PlayerRegistry::new(empty, &context(), 1).unwrap();
    players
        .reserve(PlayerKey::new(1, 0).unwrap(), 1, checkpoint())
        .unwrap();
    let before = players.snapshot();
    assert_eq!(
        players.reserve(PlayerKey::new(2, 0).unwrap(), 2, checkpoint()),
        Err(PlayerError::Limit)
    );
    assert_eq!(players.snapshot(), before);
    players.cancel(1);
    let mut invalid = checkpoint();
    invalid.location_cm[0] = f64::NAN;
    assert_eq!(
        players.reserve(PlayerKey::new(2, 0).unwrap(), 2, invalid),
        Err(PlayerError::InvalidState)
    );
    assert_eq!(players.snapshot(), before);
    let mut biome = checkpoint();
    biome.vitals.biome_row = 1;
    let mut growth = checkpoint();
    growth.vitals.growth_row = 1;
    for unstorable in [biome, growth] {
        assert_eq!(
            players.reserve(PlayerKey::new(2, 0).unwrap(), 3, unstorable),
            Err(PlayerError::InvalidState)
        );
    }
    assert_eq!(players.snapshot(), before);
    let changed = PlayerContext::new(vec![65], "/Other/Map".into(), inputs(2), rows()).unwrap();
    assert!(matches!(
        PlayerRegistry::new(before, &changed, 1),
        Err(PlayerError::InvalidContext)
    ));
    assert!(PlayerContext::new(vec![0xd800], "/Other/Map".into(), inputs(2), rows()).is_err());
    assert!(PlayerContext::new(vec![65], "/Other/Map".into(), inputs(0), rows()).is_err());
}

#[test]
fn reservation_epochs_follow_readiness_not_connection_creation_order() {
    let mut players = registry();
    // Connection created second becomes ready first; registry epochs follow readiness.
    let second_connection_key = PlayerKey::new(2, 0).unwrap();
    let first_connection_key = PlayerKey::new(1, 0).unwrap();
    players
        .reserve(second_connection_key, 1, checkpoint())
        .unwrap();
    players
        .reserve(first_connection_key, 2, checkpoint())
        .unwrap();
    players.activate(2).unwrap();
    players.activate(1).unwrap();
    assert_eq!(players.snapshot().len(), 2);
    players.detach(1);
    players.detach(2);
    assert_eq!(
        players.reserve(second_connection_key, 1, checkpoint()),
        Err(PlayerError::Conflict)
    );
    assert!(
        players
            .reserve(second_connection_key, 3, checkpoint())
            .is_ok()
    );
}

#[test]
fn programmatic_context_rejects_spare_capacity_and_unbounded_rows_before_copying() {
    let mut prospect = Vec::with_capacity(100_000);
    prospect.push(65);
    assert!(matches!(
        PlayerContext::new(prospect, "/Authored/Map".into(), inputs(1), rows()),
        Err(PlayerError::Limit)
    ));
    let mut map = String::with_capacity(100_000);
    map.push_str("/Authored/Map");
    assert!(matches!(
        PlayerContext::new(vec![65], map, inputs(1), rows()),
        Err(PlayerError::Limit)
    ));
    let new = |rows| PlayerContext::new(vec![65], "/Authored/Map".into(), inputs(1), rows);
    assert!(matches!(
        new(vec!["A".repeat(1025)]),
        Err(PlayerError::Limit)
    ));
    assert!(matches!(
        new((0..65_537).map(|row| row.to_string()).collect()),
        Err(PlayerError::Limit)
    ));
    // A duplicate or empty name could not resolve back to one row.
    for invalid in [vec!["Player".into(), "Player".into()], vec![String::new()]] {
        assert!(matches!(new(invalid), Err(PlayerError::InvalidContext)));
    }
}

#[test]
fn installed_row_refusal_names_the_table_and_the_defect() {
    let refusal = |rows: Vec<String>| check_installed_growth_rows(&rows).unwrap_err();
    for (rows, defect, reason) in [
        (
            vec!["Player".into(), "Player".into()],
            RowsDefect::Duplicate("Player".into()),
            "row name \"Player\" appears more than once",
        ),
        (
            vec!["Player".into(), String::new()],
            RowsDefect::EmptyName(1),
            "row 1 has an empty name",
        ),
        (
            vec!["Player".into(), "A".repeat(1025)],
            RowsDefect::LongName(1),
            "the name of row 1 exceeds 1024 bytes",
        ),
        (
            (0..65_537).map(|row| row.to_string()).collect(),
            RowsDefect::TooMany(65_537),
            "it has 65537 rows, over the 65536-row limit",
        ),
    ] {
        let refused = refusal(rows);
        assert_eq!(
            refused,
            InstalledRowsRefusal {
                table: "D_CharacterGrowth",
                defect,
            }
        );
        assert_eq!(
            refused.to_string(),
            format!(
                "installed game data table D_CharacterGrowth cannot back player checkpoints: {reason}; checkpoints store rows by name, so names must be non-empty, unique and bounded"
            )
        );
    }
    check_installed_growth_rows(&rows()).unwrap();
}

#[test]
fn current_release_is_validated_provenance_and_never_part_of_the_binding() {
    let mut release = ReleasePin {
        app_id: 2089300,
        build_id: 7,
        project_version: "authored".into(),
        network_checksum: network_checksum("authored"),
    };
    let fresh = PlayerSnapshot::fresh(context(), &release).unwrap();
    assert_eq!(fresh.written_under(), Some(&release));
    // Same binding under any release: the registry accepts it unchanged.
    PlayerRegistry::new(fresh, &context(), 1).unwrap();
    // The release being recorded now must be valid; stored provenance need not
    // be (stored_provenance_never_makes_a_store_unreadable).
    release.project_version = "A".repeat(513);
    release.network_checksum = network_checksum(&release.project_version);
    assert_eq!(
        PlayerSnapshot::fresh(context(), &release).err(),
        Some(PlayerError::InvalidContext)
    );
}

#[test]
fn stored_provenance_never_makes_a_store_unreadable() {
    let release = ReleasePin {
        app_id: 2089300,
        build_id: 7,
        project_version: "authored".into(),
        network_checksum: network_checksum("authored"),
    };
    let mut players = PlayerRegistry::new(
        PlayerSnapshot::fresh(context(), &release).unwrap(),
        &context(),
        64,
    )
    .unwrap();
    let key = PlayerKey::new(1, 0).unwrap();
    let mut edited = checkpoint();
    edited.vitals.health = 37;
    players.reserve(key, 1, edited).unwrap();
    players.activate(1).unwrap();
    let mut stored = serde_json::to_value(players.snapshot()).unwrap();
    // Provenance inconsistent with today's formula and constants: a checksum
    // that does not match its version, another app id and build zero.
    stored["written_under"] = serde_json::json!({
        "app_id": 1,
        "build_id": 0,
        "project_version": "9.9.999",
        "network_checksum": network_checksum("authored"),
    });
    let mut snapshot = serde_json::from_value::<PlayerSnapshot>(stored).unwrap();
    let previous = snapshot.open(&context(), &release).unwrap().unwrap();
    assert_eq!(
        (
            previous.app_id,
            previous.build_id,
            previous.project_version.as_str()
        ),
        (1, 0, "9.9.999")
    );
    assert_eq!(snapshot.written_under(), Some(&release));
    assert_eq!(snapshot, players.snapshot());
    let projection = snapshot.projection(key).unwrap();
    assert_eq!(projection.checkpoint, edited);
    assert_eq!(projection, players.snapshot().projection(key).unwrap());
}

#[test]
fn stored_row_moved_past_the_row_handle_range_refuses_instead_of_truncating() {
    let mut players = registry();
    players
        .reserve(PlayerKey::new(1, 0).unwrap(), 1, checkpoint())
        .unwrap();
    players.activate(1).unwrap();
    let stored =
        serde_json::from_value::<PlayerSnapshot>(serde_json::to_value(players.snapshot()).unwrap())
            .unwrap();
    let mut moved = (0..300)
        .map(|row| format!("Inserted_{row}"))
        .collect::<Vec<_>>();
    moved.push("Player".into());
    let release = ReleasePin {
        app_id: 2089300,
        build_id: 7,
        project_version: "authored".into(),
        network_checksum: network_checksum("authored"),
    };
    let mut snapshot = stored.clone();
    assert_eq!(
        snapshot.open(&context_with_rows(moved), &release),
        Err(StoreRefusal::RowOutOfRange {
            table: "D_CharacterGrowth",
            row: "Player".into(),
            index: 300,
        })
    );
    assert_eq!(snapshot, stored);
    // The same store opens where the row stays representable.
    snapshot.open(&context(), &release).unwrap();
}
