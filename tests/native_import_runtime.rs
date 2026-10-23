//! Authored framing and native geometry only; no supplied or original inputs.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::{
    config::ServerConfig,
    import_candidate::DecodedImportCandidate,
    native_import::{
        CompiledC001InertBindings, prepare_c001_inert_import, publish_new_c001_inert_import,
    },
    native_runtime::{
        NativeRotation, NativeWorldContext, native_world_bounds, update_native_actor,
    },
    native_world::{CompiledNativeCatalog, NativeTransform, prepare_authored_world},
    persistence::SnapshotStore,
    simulation::{
        C001InertImportState, DomainId, PreservedProspectState, SimulationClock, SnapshotMode,
        build_app, capture,
    },
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

// Hold through child exit and fixture teardown: fork can briefly inherit a
// sibling test's advisory store lock before exec closes its descriptors.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

const CATALOG: &str = r#"
native_format = 1
frame = "right_handed_z_up_meters"
[[classes]]
key = "private_box"
kind = "spatial_object_v1"
local_bounds = { min = [-1.0,-2.0,-1.0], max = [1.0,2.0,1.0] }
variables = []
[[maps]]
key = "private_map"
[[maps.tiles]]
key = "private_tile"
bounds = { min = [-100.0,-100.0,-100.0], max = [100.0,100.0,100.0] }
[[maps.placements]]
key = "zeta"
class = "private_box"
tile = "private_tile"
transform = { translation=[0.0,0.0,0.0], rotation=[0.0,0.0,0.0,1.0], scale=[1.0,1.0,1.0] }
[[maps.placements]]
key = "alpha"
class = "private_box"
tile = "private_tile"
transform = { translation=[0.0,0.0,0.0], rotation=[0.0,0.0,0.0,1.0], scale=[1.0,1.0,1.0] }
"#;

fn string(value: &str) -> Vec<u8> {
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        value.as_bytes(),
        &[0],
    ]
    .concat()
}

fn units(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

fn tag(name: &str, kind: &str, extra: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name),
        string(kind),
        i32::try_from(payload.len()).unwrap().to_le_bytes().to_vec(),
        vec![0; 4],
        extra.to_vec(),
        payload.to_vec(),
    ]
    .concat()
}

fn structure(name: &str, kind: &str, payload: &[u8]) -> Vec<u8> {
    tag(
        name,
        "StructProperty",
        &[string(kind), vec![0; 17]].concat(),
        payload,
    )
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn pose(index: usize) -> NativeTransform {
    NativeTransform {
        translation: [5.0 + index as f32, 7.0, -2.0],
        rotation: [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ],
        scale: [2.0, 1.0, 3.0],
    }
}

fn source(unsupported_last: bool) -> Vec<u8> {
    let mut records = Vec::new();
    for index in 0..2 {
        let pose = pose(index);
        let transform = [
            structure("Translation", "Vector", &floats(&pose.translation)),
            structure("Rotation", "Quat", &floats(&pose.rotation)),
            structure("Scale3D", "Vector", &floats(&pose.scale)),
            string("None"),
        ]
        .concat();
        let mut body = [
            tag(
                "ActorClassName",
                "NameProperty",
                &[0],
                &string("SecretAuthoredClass"),
            ),
            tag(
                "ActorPathName",
                "StrProperty",
                &[0],
                &string(&format!("SecretPath{index}")),
            ),
            tag(
                "ObjectFName",
                "NameProperty",
                &[0],
                &string(&format!("SecretObject{index}")),
            ),
            tag("bWasMovedToSubLevel", "BoolProperty", &[0, 0], &[]),
            tag("RecordedVersion", "IntProperty", &[0], &3_i32.to_le_bytes()),
            tag(
                "IcarusActorGUID",
                "IntProperty",
                &[0],
                &(99_i32 + index as i32).to_le_bytes(),
            ),
            structure("ActorTransform", "Transform", &transform),
        ]
        .concat();
        if unsupported_last && index == 1 {
            body.extend(tag(
                "SecretUnsupported",
                "IntProperty",
                &[0],
                &0_i32.to_le_bytes(),
            ));
        }
        body.extend(string("None"));
        body.extend([0; 4]);
        records.push(
            [
                tag(
                    "ComponentClassName",
                    "StrProperty",
                    &[0],
                    &string("/Script/Icarus.ActorStateRecorderComponent"),
                ),
                tag(
                    "BinaryData",
                    "ArrayProperty",
                    &[string("ByteProperty"), vec![0]].concat(),
                    &[
                        i32::try_from(body.len()).unwrap().to_le_bytes().to_vec(),
                        body,
                    ]
                    .concat(),
                ),
                string("None"),
            ]
            .concat(),
        );
    }
    let prospect_id = [
        (-4_i32).to_le_bytes().to_vec(),
        [0x70_u16, 0xd800, 0x71, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    ]
    .concat();
    let info = [
        tag("ProspectID", "StrProperty", &[0], &prospect_id),
        tag("LobbyName", "StrProperty", &[0], &string("SecretLobby")),
        string("None"),
    ]
    .concat();
    let payload = [
        tag("Version", "IntProperty", &[0], &2_i32.to_le_bytes()),
        structure("ProspectInfo", "ProspectInfo", &info),
        tag(
            "ProspectMapName",
            "StrProperty",
            &[0],
            &string("SecretSourceMap"),
        ),
        structure("LastSavedDateTime", "DateTime", &(-42_i64).to_le_bytes()),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &[string("StructProperty"), vec![0]].concat(),
            &[
                2_i32.to_le_bytes().to_vec(),
                structure("StateRecorderBlobs", "StateRecorderBlob", &records.concat()),
            ]
            .concat(),
        ),
        string("None"),
    ]
    .concat();
    let mut compressed = ZlibEncoder::new(Vec::new(), Compression::fast());
    compressed.write_all(&payload).unwrap();
    let compressed = compressed.finish().unwrap();
    let hash: String = Sha1::digest(&payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    serde_json::to_vec(&json!({"ProspectBlob": {
        "Hash": hash,
        "TotalLength":compressed.len(),"DataLength":0,"UncompressedLength":payload.len(),"BinaryBlob":STANDARD.encode(compressed)
    }})).unwrap()
}

fn inputs(unsupported_last: bool) -> (Vec<u8>, Arc<CompiledNativeCatalog>, Value) {
    let bytes = source(unsupported_last);
    let candidate = DecodedImportCandidate::decode(bytes.as_slice()).unwrap();
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let digest: String = candidate
        .preserved()
        .payload_sha256()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    let bindings = json!({
        "format":1,"expected_payload_sha256":digest,"native_catalog_id":catalog.identity(),
        "source_map": units("SecretSourceMap"), "native_map":"private_map",
        "coordinates":"right_handed_z_up_meters_quat_xyzw_world_space",
        "owners": (["zeta","alpha"].iter().enumerate().map(|(index, placement)| json!({
            "source_actor_class":units("SecretAuthoredClass"),"source_actor_path":units(&format!("SecretPath{index}")),
            "source_object_name":units(&format!("SecretObject{index}")),"source_icarus_guid":99+index,
            "native_placement":placement,"native_class":"private_box"
        })).collect::<Vec<_>>())
    });
    (bytes, catalog, bindings)
}

fn prepared() -> icarus_bevy::native_import::PreparedC001InertImport {
    let (bytes, catalog, bindings) = inputs(false);
    prepare_c001_inert_import(
        &DecodedImportCandidate::decode(bytes.as_slice()).unwrap(),
        catalog,
        &CompiledC001InertBindings::parse(&bindings.to_string()).unwrap(),
    )
    .unwrap()
}

#[test]
fn complete_authored_import_survives_storage_activation_pose_edits_and_capture() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let prepared = prepared();
    let expected = prepared.snapshot().clone();
    assert_eq!(expected.schema_version, 5);
    let metadata = expected.native_import.as_ref().unwrap();
    assert_eq!(metadata.source_prospect_id, [0x70, 0xd800, 0x71]);
    assert_eq!(metadata.source_last_saved_date_time, Some(-42));
    let actors = &expected.native_world.as_ref().unwrap().actors;
    assert_eq!(actors[0].placement, "alpha");
    assert_eq!(actors[0].id, DomainId(1));
    assert_eq!(actors[0].transform, pose(1));
    assert_eq!(actors[1].id, DomainId(2));
    assert_eq!(actors[1].transform, pose(0));
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("world");
    publish_new_c001_inert_import(prepared, &data).unwrap();
    let mut store = SnapshotStore::open(&data).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, expected);
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), expected);
    let old_bounds = native_world_bounds(app.world_mut(), DomainId(1)).unwrap();
    let changed = NativeTransform {
        translation: [-7.0, 8.0, 9.0],
        scale: [1.0, 2.0, 1.0],
        ..pose(0)
    };
    update_native_actor(app.world_mut(), DomainId(1), changed.clone(), Vec::new()).unwrap();
    app.world_mut().resource_mut::<SimulationClock>().tick = 19;
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(captured.schema_version, 5);
    assert_eq!(captured.tick, 19);
    assert_eq!(captured.native_import, expected.native_import);
    assert_eq!(captured.preserved_prospect, expected.preserved_prospect);
    assert_eq!(
        captured.native_world.as_ref().unwrap().actors[0].transform,
        changed
    );
    assert_ne!(
        native_world_bounds(app.world_mut(), DomainId(1)).unwrap(),
        old_bounds
    );
    store.save(&captured).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, captured);
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(capture(reloaded.world_mut()).unwrap(), captured);
    assert_eq!(
        captured
            .preserved_prospect
            .as_ref()
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        DecodedImportCandidate::decode(source(false).as_slice())
            .unwrap()
            .payload()
    );
}

#[test]
fn imported_capture_refuses_missing_or_changed_context_and_allows_repair() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let initial = prepared().snapshot().clone();
    let temporary = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(temporary.path()).unwrap();
    store.save_new(&initial).unwrap();
    let prior = fs::read(temporary.path().join("manifest.json")).unwrap();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let context = app
        .world_mut()
        .remove_resource::<C001InertImportState>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(context.clone());
    let mut changed = context.0.as_ref().clone();
    changed.source_lobby_name = units("DifferentSecret");
    app.insert_resource(C001InertImportState(Arc::new(changed)));
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(context);
    let preserved = app
        .world_mut()
        .remove_resource::<PreservedProspectState>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(preserved);
    let mode = app.world_mut().remove_resource::<SnapshotMode>().unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(mode);
    let entity = app
        .world()
        .iter_entities()
        .find(|e| e.get::<DomainId>() == Some(&DomainId(1)))
        .unwrap()
        .id();
    let rotation = *app.world().get::<NativeRotation>(entity).unwrap();
    app.world_mut()
        .entity_mut(entity)
        .remove::<NativeRotation>();
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().entity_mut(entity).insert(rotation);
    let original_context = app.world().resource::<NativeWorldContext>().clone();
    for change_map in [false, true] {
        let mut definition = initial.native_world.as_ref().unwrap().definitions.clone();
        if change_map {
            definition.maps[0].key = "other_map".into();
        } else {
            // Keep every current actor valid while changing the catalog identity.
            definition.maps[0].tiles[0].bounds.max[0] += 1.0;
        }
        let map = definition.maps[0].key.clone();
        let catalog = Arc::new(CompiledNativeCatalog::compile(definition).unwrap());
        let other = prepare_authored_world(catalog, &map)
            .unwrap()
            .initial_snapshot();
        let mut other_app = build_app(&ServerConfig::default(), &other).unwrap();
        let replacement = other_app
            .world_mut()
            .remove_resource::<NativeWorldContext>()
            .unwrap();
        app.insert_resource(replacement);
        // Even valid replacement definitions cannot replace the pinned activation.
        assert!(native_world_bounds(app.world_mut(), DomainId(1)).is_err());
        assert!(capture(app.world_mut()).is_err());
        app.insert_resource(original_context.clone());
    }
    assert_eq!(
        fs::read(temporary.path().join("manifest.json")).unwrap(),
        prior
    );
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
}

fn inventory(path: &Path) -> BTreeMap<String, Vec<u8>> {
    if !path.exists() {
        return BTreeMap::new();
    }
    fs::read_dir(path)
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

fn cli(
    source: &Path,
    catalog: &Path,
    bindings: &Path,
    destination: &Path,
    dry: bool,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("create-c001-inert-fixture")
        .arg("--source")
        .arg(source)
        .arg("--catalog")
        .arg(catalog)
        .arg("--bindings")
        .arg(bindings)
        .arg("--data-dir")
        .arg(destination);
    if dry {
        command.arg("--dry-run");
    }
    command.output().unwrap()
}

#[test]
fn authored_cli_dry_run_rejections_and_existing_world_refusal_never_change_destinations() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join("SecretSource.json");
    let catalog_path = temporary.path().join("SecretCatalog.toml");
    let bindings_path = temporary.path().join("SecretBindings.json");
    fs::write(&catalog_path, CATALOG).unwrap();
    for invalid in [false, true] {
        let (bytes, _, bindings) = inputs(invalid);
        fs::write(&source_path, &bytes).unwrap();
        fs::write(&bindings_path, bindings.to_string()).unwrap();
        for exists in [false, true] {
            let destination = temporary.path().join(format!("dest-{invalid}-{exists}"));
            if exists {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("sentinel"), b"unchanged").unwrap();
            }
            let before = inventory(&destination);
            for dry in [true, false] {
                if !invalid && !dry {
                    continue;
                }
                let output = cli(
                    &source_path,
                    &catalog_path,
                    &bindings_path,
                    &destination,
                    dry,
                );
                assert_eq!(output.status.success(), !invalid);
                assert_eq!(destination.exists(), exists);
                assert_eq!(inventory(&destination), before);
                assert_eq!(fs::read(&source_path).unwrap(), bytes);
                for diagnostic in [&output.stdout, &output.stderr] {
                    assert!(!String::from_utf8_lossy(diagnostic).contains("Secret"));
                }
                if invalid {
                    assert!(output.stdout.is_empty());
                } else {
                    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(report["world_imported"], false);
                    assert_eq!(report["gameplay_ready"], false);
                }
            }
        }
    }
    let (bytes, _, bindings) = inputs(false);
    fs::write(&source_path, &bytes).unwrap();
    fs::write(&bindings_path, bindings.to_string()).unwrap();
    let data = temporary.path().join("published");
    let output = cli(&source_path, &catalog_path, &bindings_path, &data, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let published: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(published["event"], "c001_inert_fixture_materialized");
    assert_eq!(published["world_imported"], false);
    let before = inventory(&data);
    assert!(
        !cli(&source_path, &catalog_path, &bindings_path, &data, false)
            .status
            .success()
    );
    assert_eq!(inventory(&data), before);
    // Normal run must keep schema 5 through an autosave and final save.
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["run", "--data-dir"])
        .arg(&data)
        .args([
            "--max-ticks",
            "65",
            "--compressed",
            "--autosave-seconds",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let loaded = SnapshotStore::open(&data)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded.schema_version, 5);
    assert!(loaded.tick >= 65);
    assert_eq!(
        loaded.native_import.as_ref().unwrap().source_prospect_id,
        [0x70, 0xd800, 0x71]
    );
    assert_eq!(
        loaded
            .preserved_prospect
            .as_ref()
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        DecodedImportCandidate::decode(bytes.as_slice())
            .unwrap()
            .payload()
    );
}
