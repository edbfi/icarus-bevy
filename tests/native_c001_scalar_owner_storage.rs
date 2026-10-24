//! Authored scalar persistence, recovery and terminal upgrade boundaries.
#[path = "common/c001_scalar_owners.rs"]
mod fixture;

use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{RecorderSource, ResolvePolicy, prepare_owner_restore},
    native_lifecycle_runtime::{native_owner_handle, scalar_owner_state, set_enabled, try_use},
    native_scalar_owners::ScalarUseOutcome,
    persistence::{SaveWorker, SnapshotStore},
    preservation::is_sidecar_name,
    simulation::{DomainId, WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

const SECRET: &str = "ScalarStoragePrivateSentinel";
fn disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn rewrite(root: &Path, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    rewrite_bytes(root, &bytes);
}
fn rewrite_bytes(root: &Path, bytes: &[u8]) {
    fs::write(root.join("snapshot-2.json"), bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    manifest["current"]["sha256"] = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .into();
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn duplicate_members_cannot_conceal_future_scalar_discriminators() {
    for duplicate in ["sibling", "nested", "header", "future_header"] {
        for future_first in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let before = state(10);
            let current = state(20);
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&before).unwrap();
            store.save(&current).unwrap();
            let mut wire =
                String::from_utf8(fs::read(root.join("snapshot-2.json")).unwrap()).unwrap();
            if duplicate == "nested" {
                let known = "\"callback\":\"recompute_use_availability_v1\"";
                let future = format!("\"callback\":\"{SECRET}Future\"");
                let pair = if future_first {
                    format!("{future},{known}")
                } else {
                    format!("{known},{future}")
                };
                assert!(wire.contains(known));
                wire = wire.replacen(known, &pair, 1);
            } else {
                wire = wire.replace("recompute_use_availability_v1", &format!("{SECRET}Future"));
                let extra = match duplicate {
                    "header" => "\"schema_version\":9",
                    "future_header" => "\"schema_version\":12",
                    _ => "\"native_relationships\":null",
                };
                if future_first {
                    wire.insert_str(wire.len() - 1, &format!(",{extra}"));
                } else {
                    wire.insert_str(1, &format!("{extra},"));
                }
            }
            rewrite_bytes(root, wire.as_bytes());
            let unchanged = disk(root);
            for error in [
                store.load().unwrap_err(),
                store.save(&current).unwrap_err(),
                store.save_new(&current).unwrap_err(),
            ] {
                assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
                private(&error);
                assert_eq!(disk(root), unchanged);
            }
        }
    }
}

#[test]
fn duplicate_known_members_remain_recoverable_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = state(10);
    let current = state(20);
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&before).unwrap();
    store.save(&current).unwrap();
    let mut wire = String::from_utf8(fs::read(root.join("snapshot-2.json")).unwrap()).unwrap();
    let known = "\"callback\":\"recompute_use_availability_v1\"";
    wire = wire.replacen(known, &format!("{known},{known}"), 1);
    wire.insert_str(1, "\"schema_version\":9,");
    // An unknown string at an unrelated path is not an upgrade discriminator.
    wire.insert_str(
        1,
        &format!("\"unrelated\":{{\"callback\":\"{SECRET}Future\"}},"),
    );
    rewrite_bytes(root, wire.as_bytes());
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot, before);
}
fn state(tick: u64) -> WorldSnapshot {
    let mut state = fixture::imported();
    state.tick = tick;
    state
}
fn private(error: &anyhow::Error) {
    assert!(!format!("{error:#?} {error:#}").contains(SECRET));
}
fn open_after_worker(path: &Path) -> SnapshotStore {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match SnapshotStore::open(path) {
            Ok(store) => return store,
            Err(error)
                if Instant::now() < until && format!("{error:#}").contains("already locked") =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("{error:#}"),
        }
    }
}

#[test]
fn schema_nine_roundtrip_keeps_contract_provenance_and_optional_links() {
    for links in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut initial = state(10);
        if !links {
            initial.native_relationships = None;
        }
        let mut store = SnapshotStore::open(root).unwrap();
        store.save_new(&initial).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, initial);
        let wire = read_json(&root.join("snapshot-1.json"));
        assert_eq!(wire["schema_version"], 9);
        assert!(wire.get("native_owner_import").is_none());
        assert!(wire.get("native_import").is_none());
        assert!(wire["preserved_prospect"]["encoded_sha256"].is_string());
        let source = initial
            .preserved_prospect
            .as_ref()
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap();
        assert_eq!(source, fixture::fixture().payload);
        let source_files: Vec<_> = disk(root)
            .into_iter()
            .filter(|(name, _)| is_sidecar_name(name))
            .collect();
        let mut current = initial.clone();
        current.tick = 20;
        current.native_world.as_mut().unwrap().actors[0].variables = fixture::scalar_pair(true, 7);
        store.save(&current).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, current);
        assert_eq!(
            disk(root)
                .into_iter()
                .filter(|(name, _)| is_sidecar_name(name))
                .collect::<Vec<_>>(),
            source_files
        );
    }
}

#[test]
fn worker_publication_preserves_consumption_and_never_replays_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let initial = state(0);
    let source = initial.preserved_prospect.clone();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
    assert!(matches!(
        try_use(app.world_mut(), owner).unwrap(),
        ScalarUseOutcome::Used(_)
    ));
    set_enabled(app.world_mut(), owner, false).unwrap();
    let expected = capture(app.world_mut()).unwrap();
    let mut writer = SaveWorker::spawn(SnapshotStore::open(root).unwrap()).unwrap();
    writer.submit(expected.clone()).unwrap();
    writer
        .finish_before(Instant::now() + Duration::from_secs(10))
        .unwrap();
    drop(writer);
    let loaded = open_after_worker(root).load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, expected);
    assert_eq!(loaded.preserved_prospect, source);
    let mut fresh = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(
        scalar_owner_state(fresh.world(), owner)
            .unwrap()
            .remaining_uses,
        0
    );
    assert!(matches!(
        try_use(fresh.world_mut(), owner).unwrap(),
        ScalarUseOutcome::Disabled(_)
    ));
    assert_eq!(capture(fresh.world_mut()).unwrap(), expected);
}

#[test]
fn future_discriminators_override_corruption_and_missing_source() {
    let paths = [
        "/schema_version",
        "/preserved_prospect/encoding",
        "/native_world/definitions/native_format",
        "/native_world/definitions/frame",
        "/native_world/definitions/classes/0/kind",
        "/native_world/definitions/classes/0/variables/0/default/type",
        "/native_world/actors/0/variables/0/value/type",
        "/native_lifecycle/definition/lifecycle_format",
        "/native_lifecycle/definition/slots/0/recorder_kind",
        "/native_lifecycle/slots/0/presence",
        "/native_lifecycle/slots/0/component/kind",
        "/native_lifecycle/completed/outcomes/0/disposition",
        "/native_relationships/definition/relationship_format",
        "/native_relationships/definition/actors/0/role",
        "/native_scalar_contracts/definition/format",
        "/native_scalar_contracts/definition/classes/0/callback",
        "/native_scalar_owner_import/format",
        "/native_scalar_owner_import/bindings/format",
        "/native_scalar_owner_import/bindings/adapter",
        "/native_scalar_owner_import/bindings/coordinates",
        "/native_scalar_owner_import/bindings/owners/0/owner/expected_presence",
        "/native_scalar_owner_import/resolved/0/source_policy",
        "/native_scalar_owner_import/initial_completion/outcomes/0/policy",
        "/native_scalar_owner_import/initial_completion/outcomes/0/source/type",
        "/native_scalar_owner_import/initial_completion/outcomes/0/before_presence",
        "/native_scalar_owner_import/initial_completion/outcomes/0/disposition",
        "/native_scalar_owner_import/initial_completion/outcomes/0/component_disposition",
    ];
    for path in paths {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = state(10);
        let current = state(20);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        *wire.pointer_mut(path).unwrap() = if path == "/schema_version" {
            json!(12)
        } else if path.ends_with("format") || path.ends_with("encoding") {
            json!(u64::MAX)
        } else {
            json!(format!("{SECRET}Future"))
        };
        // Structural corruption in a separate member must not conceal an upgrade.
        wire["tick"] = json!({"bad":SECRET});
        rewrite(root, &wire);
        for name in disk(root).keys().filter(|name| is_sidecar_name(name)) {
            fs::remove_file(root.join(name)).unwrap();
        }
        let unchanged = disk(root);
        for error in [
            store.load().unwrap_err(),
            store.save(&current).unwrap_err(),
            store.save_new(&current).unwrap_err(),
        ] {
            assert!(
                format!("{error:#}").contains("unsupported native snapshot schema"),
                "{path}: {error:#}"
            );
            private(&error);
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn malformed_known_state_recovers_exact_previous_values() {
    for (path, bad) in [
        (
            "/native_scalar_contracts/contract_id",
            json!("0".repeat(64)),
        ),
        (
            "/native_scalar_contracts/definition/classes/0/authored_max",
            json!(0),
        ),
        (
            "/native_scalar_contracts/definition/classes/0/callback",
            json!(7),
        ),
        (
            "/native_scalar_owner_import/bindings_id",
            json!("f".repeat(64)),
        ),
        (
            "/native_scalar_owner_import/initial_completion/request_digest",
            json!("0".repeat(64)),
        ),
        (
            "/native_scalar_owner_import/bindings/owners/0/enabled_source_name",
            json!([999]),
        ),
        (
            "/native_scalar_owner_import/resolved/0/source_index",
            json!(999),
        ),
        ("/preserved_prospect/encoding", json!(0)),
        (
            "/native_scalar_owner_import/bindings/format",
            json!({"bad":SECRET}),
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut before = state(10);
        before.native_world.as_mut().unwrap().actors[0].variables = fixture::scalar_pair(false, 8);
        let current = state(20);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        assert_ne!(wire.pointer(path).unwrap(), &bad);
        *wire.pointer_mut(path).unwrap() = bad;
        rewrite(root, &wire);
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.recovered_previous, "{path}");
        assert_eq!(loaded.snapshot, before);
    }
}

#[test]
fn invalid_snapshots_and_capacity_excess_never_change_store() {
    let initial = state(10);
    let mut invalid = vec![];
    let mut bad = initial.clone();
    bad.native_scalar_contracts = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_scalar_owner_import = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.preserved_prospect = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_scalar_owner_import
        .as_mut()
        .unwrap()
        .bindings
        .owners[0]
        .enabled_source_name
        .reserve_exact(4 * 1024 * 1024);
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_scalar_contracts
        .as_mut()
        .unwrap()
        .definition
        .classes
        .reserve_exact(10000);
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_world.as_mut().unwrap().actors[0].variables = fixture::scalar_pair(true, -1);
    invalid.push(bad);
    for schema in 1..=8 {
        let mut bad = initial.clone();
        bad.schema_version = schema;
        invalid.push(bad);
    }
    for bad in invalid {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        private(&SnapshotStore::preflight(&bad).unwrap_err());
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        let empty = disk(root);
        private(&store.save_new(&bad).unwrap_err());
        assert_eq!(disk(root), empty);
        store.save(&initial).unwrap();
        let saved = disk(root);
        private(&store.save(&bad).unwrap_err());
        assert_eq!(disk(root), saved);
    }
}

#[test]
fn later_restore_cannot_hide_lost_recorder_or_reset_source_state() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    let mut batch = fixture.initial_batch();
    batch.sequence = 2;
    batch.recorders.truncate(1);
    let recorder = &mut batch.recorders[0];
    recorder.policy = ResolvePolicy::RespawnOnly;
    recorder.expected_presence = icarus_bevy::native_lifecycle::Presence::Live;
    recorder.expected_incarnation = 1;
    recorder.source = RecorderSource::Detached {
        expected_prior_binding: Some(icarus_bevy::native_lifecycle::OwnerHandle {
            id: DomainId(3),
            incarnation: 1,
        }),
    };
    recorder.variables = fixture::scalar_pair(false, 8);
    let current = prepare_owner_restore(&initial, &batch)
        .unwrap()
        .into_snapshot();
    let mut bad = current.clone();
    let life = bad.native_lifecycle.as_mut().unwrap();
    let guid = life.slots[1].component.take().unwrap().guid;
    life.bindings.retain(|binding| binding.guid != guid);
    assert!(bad.validate().is_err());
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&initial).unwrap();
    store.save(&current).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, current);
    let repeated = prepare_owner_restore(&loaded, &batch).unwrap();
    assert!(repeated.already_completed());
    assert_eq!(repeated.snapshot(), &current);
    let before = disk(dir.path());
    assert!(store.save(&bad).is_err());
    assert_eq!(disk(dir.path()), before);
}

#[test]
fn save_new_refuses_existing_and_recoverable_scalar_generations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&state(10)).unwrap();
    store.save(&state(20)).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            fs::write(root.join("snapshot-2.json"), b"bad").unwrap();
        }
        let before = disk(root);
        assert!(store.save_new(&state(30)).is_err());
        assert_eq!(disk(root), before);
    }
}
