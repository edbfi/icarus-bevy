use super::*;
use crate::{
    config::ServerConfig,
    native_player_runtime::{CheckpointWriter, ConsumptionRates, LeavePhase, PlayerRegistry},
    native_players::{PlayerCheckpoint, PlayerContext, PlayerInputs},
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, build_app},
};
use std::{
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    time::{Duration, Instant},
};

fn write(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
fn descriptor_file(root: &Path, phase: u8) -> PathBuf {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join(format!("descriptor-{phase}.json"));
    let d = serde_json::json!({"format":1,"case_id":root.file_name().unwrap().to_str().unwrap(),"phase":phase,"output":format!("phase-{phase}.jsonl"),"predecessor":(phase==2).then_some("phase-1.jsonl"),"exit_seal":(phase==2).then_some("phase-1-exit.json")});
    write(&path, &serde_json::to_vec(&d).unwrap());
    path
}
fn context_fixture() -> PlayerContext {
    context_with_growth("Player")
}
fn context_with_growth(row: &str) -> PlayerContext {
    context_with_rows(vec![row.into()])
}
fn context_with_rows(rows: Vec<String>) -> PlayerContext {
    PlayerContext::new(
        "PRIVATE-PROSPECT-SENTINEL".encode_utf16().collect(),
        "/Authored/Map".into(),
        PlayerInputs {
            authored_gameplay: [7; 32],
            spawn: [7; 32],
            authored_world: [7; 32],
            declaration: [7; 32],
            initial_values: [7; 32],
        },
        rows,
    )
    .unwrap()
}
fn checkpoint() -> PlayerCheckpoint {
    PlayerCheckpoint {
        location_cm: [-0., 2., 3.],
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
fn empty_snapshot() -> WorldSnapshot {
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(PlayerSnapshot::empty(context_fixture())),
        ..Default::default()
    }
}

// Synthetic pawn admission is isolated to this core state-machine fixture;
// actual composition, wire admission and loss behavior are peer-lab tests.
fn attach_with_checkpoint(
    proof: &PrivateLeaveProof,
    registry: &mut PlayerRegistry,
    key: PlayerKey,
    initial: PlayerCheckpoint,
) {
    proof.joined(1, ProofJoinKind::Join).unwrap();
    registry.enable_private_leave_proof(proof.clone()).unwrap();
    let p = registry.reserve(key, 1, initial).unwrap();
    assert!(registry.activate(1).is_err());
    proof.bind_reservation(1, key, 1, p, false).unwrap();
    proof
        .access(|s| {
            s.dir
                .publish(&format!("phase-{}-pawn.bin", s.descriptor.phase), &[0])?;
            s.emit(Event::Pawn {
                epoch: 1,
                channel: 3,
                pawn: 4,
                first: 0,
                last: 1,
                payload_bits: 8,
                payload_sha256: reader::digest(&[0]),
            })
        })
        .unwrap();
    registry.activate(1).unwrap();
    registry
        .consume_activated(ConsumptionRates::new(0, 0, 1).unwrap())
        .unwrap();
}
fn prepared(root: &Path) -> (PrivateLeaveProof, PlayerRegistry, PathBuf, PlayerKey) {
    prepared_with_context(root, context_fixture(), checkpoint())
}
fn prepared_with_context(
    root: &Path,
    ctx: PlayerContext,
    initial: PlayerCheckpoint,
) -> (PrivateLeaveProof, PlayerRegistry, PathBuf, PlayerKey) {
    let path = descriptor_file(root, 1);
    let proof =
        PrivateLeaveProof::open_bound(&path, root, "11111111111111111111111111111111").unwrap();
    proof.observe_load(None).unwrap();
    let snapshot = WorldSnapshot {
        native_players: Some(PlayerSnapshot::empty(ctx.clone())),
        ..empty_snapshot()
    };
    proof.observe_opened(&snapshot).unwrap();
    let mut registry = PlayerRegistry::new(PlayerSnapshot::empty(ctx.clone()), &ctx, 1).unwrap();
    let key = PlayerKey::new(76561197960265770, 3).unwrap();
    attach_with_checkpoint(&proof, &mut registry, key, initial);
    proof
        .normal_intent(1, key, 1, LeaveProspectSessionType::ReturnToCharacterSelect)
        .unwrap();
    let revision = registry.projection(1).unwrap().revision;
    registry
        .request_leave(key, 1, revision, Instant::now(), Duration::from_secs(60))
        .unwrap();
    (proof, registry, path, key)
}
fn complete(proof: &PrivateLeaveProof, registry: &mut PlayerRegistry) {
    proof.completion_admitted(1, 2, 0, 8).unwrap();
    proof.prefix_acked(1, 2, 0, 8).unwrap();
    registry.detach(1);
    proof.detached(1, true, true).unwrap();
}

#[test]
fn exact_writer_readback_is_retained_after_pruning_and_restart_guards_first_record() {
    writer_restart(context_fixture(), checkpoint());
}

#[test]
fn nonzero_installed_growth_row_survives_real_writer_readback_seal_and_restart() {
    let mut initial = checkpoint();
    initial.vitals.growth_row = 1;
    writer_restart(
        context_with_rows(vec!["Creature".into(), "Player".into()]),
        initial,
    );
}

fn writer_restart(ctx: PlayerContext, initial: PlayerCheckpoint) {
    let root = tempfile::tempdir().unwrap();
    let (proof, registry, path, key) = prepared_with_context(root.path(), ctx.clone(), initial);
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    app.insert_resource(registry);
    let store_dir = root.path().join("store");
    std::fs::create_dir(&store_dir).unwrap();
    std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(&store_dir).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    let saved = writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key, 1)
            .unwrap()
            .phase,
        LeavePhase::Durable {
            generation: saved.generation,
            tick: saved.tick
        }
    );
    complete(
        &proof,
        &mut app.world_mut().resource_mut::<PlayerRegistry>(),
    );
    assert!(proof.phase_ready().unwrap());
    let verdict = inspect_phase(&path, root.path(), "11111111111111111111111111111111").unwrap();
    assert!(verdict.food_debited && verdict.exact_durable);
    for _ in 0..3 {
        writer.submit(app.world_mut()).unwrap();
        writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
            .unwrap();
    }
    assert!(
        !store_dir
            .join(format!("snapshot-{}.json", saved.generation))
            .exists()
    );
    assert!(inspect_phase(&path, root.path(), "11111111111111111111111111111111").is_ok());
    drop(writer);
    let until = Instant::now() + Duration::from_secs(5);
    let store = loop {
        match SnapshotStore::open_existing(&store_dir) {
            Ok(s) => break s,
            Err(_) if Instant::now() < until => std::thread::yield_now(),
            Err(_) => panic!("writer lock not released"),
        }
    };
    let mut loaded = store.load().unwrap().unwrap();
    assert_eq!(
        loaded
            .snapshot
            .native_players
            .as_ref()
            .unwrap()
            .projection(key)
            .unwrap()
            .checkpoint
            .vitals
            .growth_row,
        0,
        "storage interns the sole persisted growth name at zero"
    );
    drop(store);
    seal_exit_store(
        &path,
        root.path(),
        "11111111111111111111111111111111",
        &store_dir,
    )
    .unwrap();
    let second = descriptor_file(root.path(), 2);
    let proof2 =
        PrivateLeaveProof::open_bound(&second, root.path(), "22222222222222222222222222222222")
            .unwrap();
    proof2.observe_load(Some(&loaded)).unwrap();
    // Follow actual startup: observe raw load, resolve against the installed
    // context, then observe the opened snapshot used to construct the registry.
    loaded
        .snapshot
        .native_players
        .as_mut()
        .unwrap()
        .open(
            &ctx,
            &crate::experimental::release::ReleasePin::parse(include_str!(
                "../../config/release.toml"
            ))
            .unwrap(),
        )
        .unwrap();
    proof2.observe_opened(&loaded.snapshot).unwrap();
    let players = loaded.snapshot.native_players.unwrap();
    let mut returning = PlayerRegistry::new(players, &ctx, 1).unwrap();
    proof2.joined(1, ProofJoinKind::Join).unwrap();
    returning
        .enable_private_leave_proof(proof2.clone())
        .unwrap();
    let p = returning.reserve(key, 1, initial).unwrap();
    assert!(!p.is_new);
    assert_eq!(p.checkpoint.vitals.food, 99);
    assert_eq!(p.checkpoint.vitals.growth_row, initial.vitals.growth_row);
    assert!(returning.activate(1).is_err());
    let mut changed = p;
    changed.checkpoint.location_cm[0] = 0.;
    assert!(proof2.bind_reservation(1, key, 1, changed, false).is_err());
    assert!(
        returning
            .consume_activated(ConsumptionRates::new(0, 0, 1).unwrap())
            .is_err()
    );
    assert!(
        !format!("{:?} {}", proof2, proof2.health().unwrap_err()).contains("76561197960265770")
    );
}

#[test]
fn proof_sink_failure_never_revokes_or_retries_the_successful_save() {
    let root = tempfile::tempdir().unwrap();
    let (proof, registry, _, key) = prepared(root.path());
    write(
        &root.path().join("phase-1-leave-manifest.json"),
        b"owned preexisting artifact",
    );
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    app.insert_resource(registry);
    let target = tempfile::tempdir().unwrap();
    std::fs::set_permissions(target.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(target.path()).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    let receipt = writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.generation, 1);
    assert!(proof.health().is_err());
    assert!(matches!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key, 1)
            .unwrap()
            .phase,
        LeavePhase::Durable { generation: 1, .. }
    ));
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .activate(1)
            .is_err()
    );
    assert_eq!(
        std::fs::read(root.path().join("phase-1-leave-manifest.json")).unwrap(),
        b"owned preexisting artifact"
    );
}

#[test]
fn wrong_current_tick_and_corrupt_readback_fail_closed_without_fallback() {
    for fault in ["current", "tick", "bytes"] {
        let root = tempfile::tempdir().unwrap();
        let (proof, registry, _, key) = prepared(root.path());
        let snapshot = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        };
        let r = snapshot
            .native_players
            .as_ref()
            .unwrap()
            .records
            .iter()
            .find(|r| r.key == key)
            .unwrap();
        let selection = proof
            .writer_selection(1, 1, r, snapshot.native_players.as_ref().unwrap())
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::set_permissions(target.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut store = SnapshotStore::open(target.path()).unwrap();
        let mut receipt = store.save(&snapshot).unwrap();
        match fault {
            "current" => {
                store.save(&snapshot).unwrap();
            }
            "tick" => {
                receipt.tick += 1;
            }
            _ => {
                std::fs::write(target.path().join("snapshot-1.json"), b"corrupt").unwrap();
            }
        }
        assert!(store.readback_private_leave(&receipt, selection).is_err());
        assert!(proof.health().is_err());
        assert!(!root.path().join("phase-1-leave-generation.json").exists());
    }
}

#[test]
fn confinement_bindings_existing_files_and_latched_failures_are_private() {
    let root = tempfile::tempdir().unwrap();
    let path = descriptor_file(root.path(), 1);
    assert!(validate_descriptor(&path, root.path()).is_ok());
    assert!(PrivateLeaveProof::open_bound(&path, root.path(), "BAD-PRIVATE-SENTINEL").is_err());
    let proof =
        PrivateLeaveProof::open_bound(&path, root.path(), "11111111111111111111111111111111")
            .unwrap();
    assert!(
        PrivateLeaveProof::open_bound(&path, root.path(), "11111111111111111111111111111111")
            .is_err()
    );
    assert!(read_private_input(root.path(), Path::new("../descriptor-1.json"), 4096).is_err());
    symlink(&path, root.path().join("link.json")).unwrap();
    assert!(read_private_input(root.path(), Path::new("link.json"), 4096).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_private_input(root.path(), &path, 4096).is_err());
    let err = proof.joined(1, ProofJoinKind::Join).unwrap_err();
    assert_eq!(err.0, ProofFailure::Order);
    assert!(proof.observe_load(None).is_err());
    assert!(!format!("{err} {proof:?}").contains("PRIVATE"));
}

#[test]
fn decoder_rejects_duplicate_unknown_and_reordered_fields_and_signed_zero_roundtrips() {
    assert!(decode::<Descriptor>(br#"{"format":1,"format":1,"case_id":"a","phase":1,"output":"a","predecessor":null,"exit_seal":null}"#).is_err());
    assert!(decode::<Descriptor>(br#"{"format":1,"case_id":"a","phase":1,"output":"a","predecessor":null,"exit_seal":null,"extra":1}"#).is_err());
    let key = PlayerKey::new(1, 0).unwrap();
    let record = Record::from_projection(
        key,
        PlayerProjection {
            player_id: u64::MAX - 1,
            revision: 1,
            checkpoint: checkpoint(),
            is_new: false,
        },
        "Player",
    )
    .unwrap();
    let roundtrip: Record = decode(&encode(&record).unwrap()).unwrap();
    assert!(roundtrip == record);
    assert_eq!(roundtrip.location[0], (-0f64).to_bits());
    let mut e = Evidence::new(1, None);
    assert!(e.apply(&Event::Ready).is_err());
    let root = tempfile::tempdir().unwrap();
    let path = descriptor_file(root.path(), 1);
    publish_abort(root.path()).unwrap();
    assert!(validate_descriptor(&path, root.path()).is_err());
    let live_root = tempfile::tempdir().unwrap();
    let live_path = descriptor_file(live_root.path(), 1);
    let live = PrivateLeaveProof::open_bound(
        &live_path,
        live_root.path(),
        "11111111111111111111111111111111",
    )
    .unwrap();
    publish_abort(live_root.path()).unwrap();
    assert!(live.health().is_err());
    assert!(live.observe_load(None).is_err());
}

#[test]
fn phase_two_zero_food_can_leave_without_claiming_another_debit() {
    let key = PlayerKey::new(1, 0).unwrap();
    let mut checkpoint = checkpoint();
    checkpoint.vitals.food = 0;
    let record = Record::from_projection(
        key,
        PlayerProjection {
            player_id: 1,
            revision: 101,
            checkpoint,
            is_new: false,
        },
        "Player",
    )
    .unwrap();
    let ctx = context(empty_snapshot().native_players.as_ref().unwrap()).unwrap();
    let mut e = Evidence::new(
        2,
        Some(RestartExpectation {
            record: record.clone(),
            context: ctx.clone(),
            generation: GenerationProof {
                number: 7,
                sha256: "a".repeat(64),
                tick: 3000,
            },
        }),
    );
    e.apply(&Event::Load {
        generation: Some(GenerationProof {
            number: 7,
            sha256: "a".repeat(64),
            tick: 3000,
        }),
        record: Some(record.clone()),
        context: Some(ctx.clone()),
    })
    .unwrap();
    e.apply(&Event::Opened {
        context: ctx,
        record: Some(record.clone()),
    })
    .unwrap();
    e.apply(&Event::Joined {
        connection: 1,
        kind: ProofJoinKind::Join,
    })
    .unwrap();
    e.apply(&Event::Reserved {
        epoch: 1,
        record: record.clone(),
        is_new: false,
    })
    .unwrap();
    e.apply(&Event::Bound {
        connection: 1,
        epoch: 1,
    })
    .unwrap();
    e.apply(&Event::Pawn {
        epoch: 1,
        channel: 3,
        pawn: 4,
        first: 0,
        last: 1,
        payload_bits: 8,
        payload_sha256: "a".repeat(64),
    })
    .unwrap();
    e.apply(&Event::Activated { epoch: 1 }).unwrap();
    e.apply(&Event::Intent { epoch: 1, kind: 4 }).unwrap();
    e.apply(&Event::Cutoff {
        epoch: 1,
        attempt: 1,
        record: record.clone(),
    })
    .unwrap();
    e.apply(&Event::Submitted {
        epoch: 1,
        attempt: 1,
        record: record.clone(),
    })
    .unwrap();
    e.apply(&Event::Readback {
        epoch: 1,
        attempt: 1,
        record,
        generation: GenerationProof {
            number: 8,
            sha256: "b".repeat(64),
            tick: 3001,
        },
    })
    .unwrap();
    e.apply(&Event::Durable {
        epoch: 1,
        attempt: 1,
        generation: 8,
        tick: 3001,
    })
    .unwrap();
    e.apply(&Event::Completion {
        epoch: 1,
        channel: 2,
        first: 0,
        last: 5,
    })
    .unwrap();
    e.apply(&Event::PrefixAcked {
        epoch: 1,
        channel: 2,
        first: 0,
        last: 5,
    })
    .unwrap();
    e.apply(&Event::Detached { epoch: 1 }).unwrap();
    e.apply(&Event::Ready).unwrap();
    let v = e.verdict("22222222222222222222222222222222");
    assert!(v.phase_ready && v.exact_durable && v.actual_load_equal);
    assert!(!v.food_debited);
}

#[test]
fn raw_stored_growth_name_cannot_be_normalized_into_an_equal_record() {
    let root = tempfile::tempdir().unwrap();
    let (proof, registry, _, key) = prepared(root.path());
    let players = registry.snapshot();
    let original = selected(&players, key).unwrap();
    let mut encoded = serde_json::to_value(&players).unwrap();
    encoded["records"][0]["growth_row"]["row"] = serde_json::json!("DifferentAuthoredRow");
    let changed: PlayerSnapshot = serde_json::from_value(encoded).unwrap();
    let decoded = selected(&changed, key).unwrap();
    // Both deserialized names occupy index zero; the raw stored identity must
    // still distinguish them before an installed-row resolver can remap them.
    assert_eq!(original.growth, decoded.growth);
    assert!(original != decoded);
    let record = players.records.iter().find(|r| r.key == key).unwrap();
    let selection = proof.writer_selection(1, 1, record, &players).unwrap();
    let store_dir = root.path().join("store");
    std::fs::create_dir(&store_dir).unwrap();
    std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SnapshotStore::open(&store_dir).unwrap();
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(changed),
        ..Default::default()
    };
    let saved = store.save(&snapshot).unwrap();
    assert!(store.readback_private_leave(&saved, selection).is_err());
    assert!(proof.health().is_err());
    assert!(!root.path().join("phase-1-leave-generation.json").exists());
    assert_eq!(
        store.load().unwrap().unwrap().selected_generation,
        saved.generation
    );
}

#[test]
fn registry_install_checks_its_actual_context_before_any_reservation() {
    let root = tempfile::tempdir().unwrap();
    let path = descriptor_file(root.path(), 1);
    let proof = PrivateLeaveProof::open_authored_fixture(
        &path,
        root.path(),
        "11111111111111111111111111111111",
    )
    .unwrap();
    proof.observe_load(None).unwrap();
    proof.observe_opened(&empty_snapshot()).unwrap();
    let mut encoded = serde_json::to_value(empty_snapshot().native_players.unwrap()).unwrap();
    encoded["context"]["map_url"] = serde_json::json!("/Authored/OtherMap");
    let players: PlayerSnapshot = serde_json::from_value(encoded).unwrap();
    let expected = players.context().clone();
    let mut registry = PlayerRegistry::new(players, &expected, 1).unwrap();
    assert!(registry.enable_private_leave_proof(proof.clone()).is_err());
    assert!(proof.health().is_err());
    assert!(registry.active_epochs().is_empty());
    assert!(registry.snapshot().is_empty());
}

#[test]
fn restart_load_requires_the_exact_sealed_current_generation_not_only_equal_record() {
    let key = PlayerKey::new(1, 0).unwrap();
    let record = Record::from_projection(
        key,
        PlayerProjection {
            player_id: 1,
            revision: 2,
            checkpoint: checkpoint(),
            is_new: false,
        },
        "Player",
    )
    .unwrap();
    let context = context(empty_snapshot().native_players.as_ref().unwrap()).unwrap();
    let sealed = GenerationProof {
        number: 7,
        sha256: "a".repeat(64),
        tick: 3000,
    };
    for mismatch in 0..3 {
        let mut actual = sealed.clone();
        match mismatch {
            0 => actual.number += 1,
            1 => actual.sha256 = "b".repeat(64),
            _ => actual.tick += 1,
        }
        let mut evidence = Evidence::new(
            2,
            Some(RestartExpectation {
                record: record.clone(),
                context: context.clone(),
                generation: sealed.clone(),
            }),
        );
        assert!(
            evidence
                .apply(&Event::Load {
                    generation: Some(actual),
                    record: Some(record.clone()),
                    context: Some(context.clone())
                })
                .is_err()
        );
    }
    let mut evidence = Evidence::new(
        2,
        Some(RestartExpectation {
            record: record.clone(),
            context: context.clone(),
            generation: sealed.clone(),
        }),
    );
    assert!(
        evidence
            .apply(&Event::Load {
                generation: Some(sealed),
                record: Some(record),
                context: Some(context)
            })
            .is_ok()
    );
}

#[test]
fn fresh_registry_install_cannot_substitute_another_installed_growth_context() {
    let root = tempfile::tempdir().unwrap();
    let path = descriptor_file(root.path(), 1);
    let proof = PrivateLeaveProof::open_authored_fixture(
        &path,
        root.path(),
        "11111111111111111111111111111111",
    )
    .unwrap();
    proof.observe_load(None).unwrap();
    proof.observe_opened(&empty_snapshot()).unwrap();
    let changed_context = context_with_growth("DifferentAuthoredRow");
    let mut registry = PlayerRegistry::new(
        PlayerSnapshot::empty(changed_context.clone()),
        &changed_context,
        1,
    )
    .unwrap();
    assert!(registry.enable_private_leave_proof(proof.clone()).is_err());
    assert!(proof.health().is_err());
    assert!(registry.active_epochs().is_empty());
}
