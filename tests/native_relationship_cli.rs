//! Independently authored CLI fixtures only; no original-game compatibility claim.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_import::{CompiledC001InertBindings, prepare_c001_inert_import},
    native_world::{CompiledNativeCatalog, NativeTransform, prepare_authored_world},
    persistence::SnapshotStore,
    simulation::{DomainId, WorldSnapshot},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

// Hold through child exit and store teardown: fork can briefly inherit another
// test's advisory lock before exec closes the descriptors. Every test takes it.
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
key = "private_zeta"
class = "private_box"
tile = "private_tile"
transform = { translation=[0.0,0.0,0.0], rotation=[0.0,0.0,0.0,1.0], scale=[1.0,1.0,1.0] }
[[maps.placements]]
key = "private_alpha"
class = "private_box"
tile = "private_tile"
transform = { translation=[0.0,0.0,0.0], rotation=[0.0,0.0,0.0,1.0], scale=[1.0,1.0,1.0] }
"#;

fn units(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

fn link_name(value: &str) -> Vec<u16> {
    // Preserve a non-scalar unit while exercising ASCII-only case folding.
    [units(value), vec![0xd800]].concat()
}

fn relationships(catalog: &CompiledNativeCatalog, reverse: bool) -> Value {
    let mut actors = vec![
        json!({
            "placement":"private_alpha", "role":"foundation",
            "identity":{"name":link_name("SecretFoundation"),"guid":314159265},
            "foundation":null
        }),
        json!({
            "placement":"private_zeta", "role":"deployable",
            "identity":{"name":link_name("SecretDeployable"),"guid":271828182},
            "foundation":{"name":link_name("sECRETfOUNDATION"),"guid":314159265}
        }),
    ];
    if reverse {
        actors.reverse();
    }
    json!({
        "relationship_format":1, "native_catalog_id":catalog.identity(),
        "map":"private_map", "actors":actors
    })
}

// Minimal authored C001 framing, kept local to this CLI executable so it does
// not couple unrelated integration tests or use supplied save material.
fn string(value: &str) -> Vec<u8> {
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .to_vec(),
        value.as_bytes().to_vec(),
        vec![0],
    ]
    .concat()
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
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
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

fn source() -> Vec<u8> {
    let records: Vec<_> = (0..2)
        .map(|index| {
            let pose = pose(index);
            let transform = [
                structure("Translation", "Vector", &floats(&pose.translation)),
                structure("Rotation", "Quat", &floats(&pose.rotation)),
                structure("Scale3D", "Vector", &floats(&pose.scale)),
                string("None"),
            ]
            .concat();
            let body = [
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
                string("None"),
                vec![0; 4],
            ]
            .concat();
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
            .concat()
        })
        .collect();
    let prospect_id = [
        (-4_i32).to_le_bytes().to_vec(),
        [0x70_u16, 0xd800, 0x71, 0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
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
        "Hash":hash, "TotalLength":compressed.len(), "DataLength":0,
        "UncompressedLength":payload.len(), "BinaryBlob":STANDARD.encode(compressed)
    }}))
    .unwrap()
}

struct Inputs {
    catalog: Arc<CompiledNativeCatalog>,
    catalog_path: PathBuf,
    relationships_path: PathBuf,
    source_path: PathBuf,
    bindings_path: PathBuf,
    bytes: Vec<u8>,
    bindings: Value,
}

impl Inputs {
    fn new(root: &Path) -> Self {
        // Both maps are valid: selecting the other map in relationships must
        // still reject composition with the CLI's selected/bound native map.
        let (_, map) = CATALOG.split_once("[[maps]]").unwrap();
        let text = format!(
            "{CATALOG}\n[[maps]]{}",
            map.replace("private_map", "private_other_map")
        );
        let catalog = Arc::new(CompiledNativeCatalog::parse(&text).unwrap());
        let bytes = source();
        let candidate = DecodedImportCandidate::decode(bytes.as_slice()).unwrap();
        let digest: String = candidate
            .preserved()
            .payload_sha256()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let bindings = json!({
            "format":1, "expected_payload_sha256":digest, "native_catalog_id":catalog.identity(),
            "source_map":units("SecretSourceMap"), "native_map":"private_map",
            "coordinates":"right_handed_z_up_meters_quat_xyzw_world_space",
            "owners": (["private_zeta", "private_alpha"].iter().enumerate().map(|(index, placement)| json!({
                "source_actor_class":units("SecretAuthoredClass"),
                "source_actor_path":units(&format!("SecretPath{index}")),
                "source_object_name":units(&format!("SecretObject{index}")), "source_icarus_guid":99+index,
                "native_placement":placement, "native_class":"private_box"
            })).collect::<Vec<_>>())
        });
        let inputs = Self {
            catalog,
            bytes,
            bindings,
            catalog_path: root.join("SecretCatalog.toml"),
            relationships_path: root.join("SecretRelationships.json"),
            source_path: root.join("SecretSource.json"),
            bindings_path: root.join("SecretBindings.json"),
        };
        fs::write(&inputs.catalog_path, text).unwrap();
        fs::write(&inputs.source_path, &inputs.bytes).unwrap();
        fs::write(&inputs.bindings_path, inputs.bindings.to_string()).unwrap();
        inputs.write_relationships(&relationships(&inputs.catalog, true));
        inputs
    }

    fn write_relationships(&self, definition: &Value) {
        fs::write(&self.relationships_path, definition.to_string()).unwrap();
    }

    fn create(&self, c001: bool, destination: &Path, dry: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
        if c001 {
            command
                .arg("create-c001-inert-fixture")
                .arg("--source")
                .arg(&self.source_path)
                .arg("--bindings")
                .arg(&self.bindings_path);
        } else {
            command
                .arg("create-native-world")
                .args(["--map", "private_map"]);
        }
        command
            .arg("--catalog")
            .arg(&self.catalog_path)
            .arg("--relationships")
            .arg(&self.relationships_path)
            .arg("--data-dir")
            .arg(destination);
        if dry {
            command.arg("--dry-run");
        }
        command.output().unwrap()
    }

    fn base(&self, c001: bool) -> WorldSnapshot {
        if c001 {
            prepare_c001_inert_import(
                &DecodedImportCandidate::decode(self.bytes.as_slice()).unwrap(),
                Arc::clone(&self.catalog),
                &CompiledC001InertBindings::parse(&self.bindings.to_string()).unwrap(),
            )
            .unwrap()
            .snapshot()
            .clone()
        } else {
            prepare_authored_world(Arc::clone(&self.catalog), "private_map")
                .unwrap()
                .initial_snapshot()
        }
    }

    fn assert_diagnostics(&self, output: &Output) {
        for bytes in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(bytes);
            for forbidden in [
                "Secret",
                "private_",
                "314159265",
                "271828182",
                "271828181",
                self.catalog.identity(),
                self.bindings["expected_payload_sha256"].as_str().unwrap(),
                &serde_json::to_string(&link_name("SecretFoundation")).unwrap(),
                &serde_json::to_string(&link_name("SecretDeployable")).unwrap(),
                &serde_json::to_string(&link_name("sECRETfOUNDATION")).unwrap(),
                &serde_json::to_string(&units("SecretSourceMap")).unwrap(),
            ] {
                assert!(!text.contains(forbidden), "fixture value leaked: {text}");
            }
        }
    }

    fn assert_created(&self, output: &Output, c001: bool, dry: bool) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.assert_diagnostics(output);
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["schema_version"], 6);
        assert_eq!(report["world_imported"], false);
        assert_eq!(report["gameplay_ready"], false);
        assert_eq!(
            report["event"],
            match (c001, dry) {
                (false, true) => "native_world_prepared",
                (false, false) => "native_world_created",
                (true, true) => "c001_inert_fixture_prepared",
                (true, false) => "c001_inert_fixture_materialized",
            }
        );
        if dry {
            assert_eq!(report["dry_run"], true);
        }
        if !c001 {
            assert_eq!(report["actors"], 2);
        } else {
            let inspection = report["inspection"].as_object().unwrap();
            assert_eq!(inspection.len(), 5);
            for key in ["recorders", "bindings", "placements"] {
                assert_eq!(inspection[key], 2);
            }
            for key in ["payload_bytes", "metadata_bytes"] {
                assert!(inspection[key].as_u64().is_some_and(|count| count > 0));
            }
        }
        // Apart from the static event, successful diagnostics expose only
        // flags, schema/count fields and the numeric publication receipt.
        let fields = report.as_object().unwrap();
        assert_eq!(fields.len(), 6);
        for (key, value) in fields {
            match key.as_str() {
                "event" => {}
                "world_imported" | "gameplay_ready" | "dry_run" => {
                    assert!(value.is_boolean());
                }
                "schema_version" | "actors" => assert!(value.is_u64()),
                "receipt" => {
                    let receipt = value.as_object().unwrap();
                    assert_eq!(receipt.len(), 2);
                    assert!(receipt["generation"].is_u64());
                    assert!(receipt["tick"].is_u64());
                }
                "inspection" => {}
                _ => panic!("unexpected diagnostic field: {key}"),
            }
        }
    }

    fn remove_inputs(&self) {
        for path in [
            &self.catalog_path,
            &self.relationships_path,
            &self.source_path,
            &self.bindings_path,
        ] {
            fs::remove_file(path).unwrap();
        }
    }
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

fn load(path: &Path) -> WorldSnapshot {
    SnapshotStore::open(path)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot
}

fn assert_links(snapshot: &WorldSnapshot) {
    assert_eq!(snapshot.schema_version, 6);
    let relationships = snapshot.native_relationships.as_ref().unwrap();
    let actors = &relationships.definition.actors;
    assert_eq!(actors.len(), 2);
    assert_eq!(actors[0].placement, "private_alpha");
    assert_eq!(actors[1].placement, "private_zeta");
    assert_eq!(actors[0].identity.name, link_name("SecretFoundation"));
    assert_eq!(actors[1].identity.name, link_name("SecretDeployable"));
    assert_eq!(relationships.foundations.len(), 1);
    assert_eq!(relationships.foundations[0].owner, DomainId(2));
    assert_eq!(relationships.foundations[0].foundation, Some(DomainId(1)));
    let native = snapshot.native_world.as_ref().unwrap();
    assert_eq!(native.actors[0].id, DomainId(1));
    assert_eq!(native.actors[0].placement, "private_alpha");
    assert_eq!(native.actors[1].id, DomainId(2));
    assert_eq!(native.actors[1].placement, "private_zeta");
}

fn successful_round_trip(c001: bool) {
    let mut canonical = None;
    for reverse in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = Inputs::new(temporary.path());
        inputs.write_relationships(&relationships(&inputs.catalog, reverse));
        let base = inputs.base(c001);
        let destination = temporary.path().join("world");
        let output = inputs.create(c001, &destination, false);
        inputs.assert_created(&output, c001, false);
        let initial = load(&destination);
        assert_links(&initial);
        assert_eq!(initial.tick, 0);
        assert_eq!(initial.native_world, base.native_world);
        assert_eq!(initial.native_import, base.native_import);
        assert_eq!(initial.preserved_prospect, base.preserved_prospect);
        if let Some(expected) = &canonical {
            assert_eq!(&initial.native_relationships, expected);
        } else {
            canonical = Some(initial.native_relationships.clone());
        }
        if c001 {
            let metadata = initial.native_import.as_ref().unwrap();
            assert_eq!(metadata.source_prospect_id, [0x70, 0xd800, 0x71]);
            assert_eq!(metadata.source_lobby_name, units("SecretLobby"));
            assert_eq!(metadata.source_last_saved_date_time, Some(-42));
            assert_eq!(metadata.owners.len(), 2);
            assert_eq!(
                initial.native_world.as_ref().unwrap().actors[0].transform,
                pose(1)
            );
            assert_eq!(
                initial.native_world.as_ref().unwrap().actors[1].transform,
                pose(0)
            );
        }
        inputs.remove_inputs();
        let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .args(["run", "--data-dir"])
            .arg(&destination)
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
        inputs.assert_diagnostics(&output);
        let events: Vec<Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        // An intermediate commit proves autosave, independently of final save.
        assert!(events.iter().any(|event| {
            event["event"] == "save_committed"
                && event["tick"]
                    .as_u64()
                    .is_some_and(|tick| tick > 0 && tick < 65)
        }));
        let reloaded = load(&destination);
        assert_links(&reloaded);
        assert_eq!(reloaded.tick, 65);
        let mut expected = initial;
        expected.tick = 65;
        assert_eq!(reloaded, expected);
        if c001 {
            assert_eq!(
                reloaded
                    .preserved_prospect
                    .as_ref()
                    .unwrap()
                    .decode()
                    .unwrap()
                    .to_inner_payload()
                    .unwrap(),
                DecodedImportCandidate::decode(inputs.bytes.as_slice())
                    .unwrap()
                    .payload()
            );
        }
    }
}

#[test]
fn native_relationship_cli_canonicalizes_order_and_survives_source_free_autosave_reload() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    successful_round_trip(false);
}

#[test]
fn c001_relationship_cli_keeps_metadata_and_unchanged_payload_through_autosave_reload() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    successful_round_trip(true);
}

#[test]
fn both_relationship_dry_runs_leave_absent_and_sentinel_destinations_untouched() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let inputs = Inputs::new(temporary.path());
    for c001 in [false, true] {
        for exists in [false, true] {
            let destination = temporary.path().join(format!("dry-{c001}-{exists}"));
            if exists {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("sentinel"), b"unchanged").unwrap();
            }
            let before = inventory(&destination);
            let output = inputs.create(c001, &destination, true);
            inputs.assert_created(&output, c001, true);
            assert_eq!(destination.exists(), exists);
            assert_eq!(inventory(&destination), before);
        }
    }
}

#[test]
fn invalid_final_link_catalog_map_and_dto_never_touch_either_destination() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let inputs = Inputs::new(temporary.path());
    for invalid in ["final_link", "catalog", "map", "unknown_field", "role"] {
        let mut definition = relationships(&inputs.catalog, false);
        match invalid {
            // The first declaration is valid; the final edge cannot resolve.
            "final_link" => definition["actors"][1]["foundation"]["guid"] = 271828181.into(),
            "catalog" => definition["native_catalog_id"] = "0".repeat(64).into(),
            "map" => definition["map"] = "private_other_map".into(),
            "unknown_field" => definition["SecretUnknownField"] = "SecretValue".into(),
            "role" => definition["actors"][1]["role"] = "SecretUnsupportedRole".into(),
            _ => unreachable!(),
        }
        inputs.write_relationships(&definition);
        for c001 in [false, true] {
            for exists in [false, true] {
                let destination = temporary
                    .path()
                    .join(format!("bad-{invalid}-{c001}-{exists}"));
                if exists {
                    fs::create_dir(&destination).unwrap();
                    fs::write(destination.join("sentinel"), b"unchanged").unwrap();
                }
                let before = inventory(&destination);
                for dry in [false, true] {
                    let output = inputs.create(c001, &destination, dry);
                    assert!(!output.status.success(), "{invalid}");
                    assert!(output.stdout.is_empty());
                    inputs.assert_diagnostics(&output);
                    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
                    assert_eq!(error["event"], "fatal");
                    assert_eq!(destination.exists(), exists);
                    assert_eq!(inventory(&destination), before);
                }
            }
        }
    }
    assert_eq!(fs::read(&inputs.source_path).unwrap(), inputs.bytes);
}

#[test]
fn both_relationship_commands_refuse_existing_and_recoverable_worlds_without_writes() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let inputs = Inputs::new(temporary.path());
    for c001 in [false, true] {
        for recoverable in [false, true] {
            let destination = temporary
                .path()
                .join(format!("existing-{c001}-{recoverable}"));
            let output = inputs.create(c001, &destination, false);
            inputs.assert_created(&output, c001, false);
            let initial = load(&destination);
            if recoverable {
                {
                    let mut store = SnapshotStore::open(&destination).unwrap();
                    let mut newer = initial.clone();
                    newer.tick = 10;
                    assert_eq!(store.save(&newer).unwrap().generation, 2);
                }
                fs::write(destination.join("snapshot-2.json"), b"SecretCorruptCurrent").unwrap();
                let recovered = SnapshotStore::open(&destination)
                    .unwrap()
                    .load()
                    .unwrap()
                    .unwrap();
                assert!(recovered.recovered_previous);
                assert_eq!(recovered.snapshot, initial);
            }
            let before = inventory(&destination);
            // Both publishers refuse either kind of world, even when its
            // original creator used the other command.
            for attempted_c001 in [false, true] {
                let output = inputs.create(attempted_c001, &destination, false);
                assert!(!output.status.success());
                assert!(output.stdout.is_empty());
                inputs.assert_diagnostics(&output);
                assert_eq!(inventory(&destination), before);
            }
            assert_eq!(load(&destination), initial);
        }
    }
}
