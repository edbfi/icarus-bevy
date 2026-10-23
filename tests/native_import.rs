//! Independently authored wire fixtures. No original files or test execution
//! are required to construct these adapter inputs.
use std::{fs, io::Write, sync::Arc};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_import::{
        AuthoredCoordinatesV1, C001InertAdapter, C001InertBindingsV1, C001InertImportMetadataV1,
        C001PlacementBindingV1, CompiledC001InertBindings, MAX_C001_BINDINGS_BYTES,
        MAX_C001_METADATA_BYTES, SemanticImportError, prepare_c001_inert_import,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, NativeValue, VariableDefinition,
    },
    simulation::{DomainId, SNAPSHOT_SCHEMA_C001_INERT},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tempfile::TempDir;

const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
const COORDINATES: AuthoredCoordinatesV1 =
    AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace;

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
fn string(value: &str) -> Vec<u8> {
    if value.is_empty() {
        return 0_i32.to_le_bytes().to_vec();
    }
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
fn wide(value: &[u16]) -> Vec<u8> {
    if value.is_empty() {
        return 0_i32.to_le_bytes().to_vec();
    }
    // The lossless typed representation uses ASCII when every unit fits it.
    // Keep this fixture encoder independent of the implementation encoder.
    if value.iter().all(|unit| *unit <= 127) {
        return [
            i32::try_from(value.len() + 1)
                .unwrap()
                .to_le_bytes()
                .to_vec(),
            value.iter().map(|unit| *unit as u8).collect(),
            vec![0],
        ]
        .concat();
    }
    [
        (-i32::try_from(value.len() + 1).unwrap())
            .to_le_bytes()
            .to_vec(),
        value.iter().flat_map(|u| u.to_le_bytes()).collect(),
        vec![0, 0],
    ]
    .concat()
}
#[derive(Clone)]
struct Tag {
    name: &'static str,
    kind: &'static str,
    extras: Vec<u8>,
    payload: Vec<u8>,
    index: i32,
}
impl Tag {
    fn bytes(&self) -> Vec<u8> {
        [
            string(self.name),
            string(self.kind),
            i32::try_from(self.payload.len())
                .unwrap()
                .to_le_bytes()
                .to_vec(),
            self.index.to_le_bytes().to_vec(),
            self.extras.clone(),
            self.payload.clone(),
        ]
        .concat()
    }
    fn integer(name: &'static str, value: i32) -> Self {
        Self {
            name,
            kind: "IntProperty",
            extras: vec![0],
            payload: value.to_le_bytes().to_vec(),
            index: 0,
        }
    }
    fn text(name: &'static str, kind: &'static str, value: &[u16]) -> Self {
        Self {
            name,
            kind,
            extras: vec![0],
            payload: wide(value),
            index: 0,
        }
    }
    fn boolean(name: &'static str, value: bool) -> Self {
        Self {
            name,
            kind: "BoolProperty",
            extras: vec![u8::from(value), 0],
            payload: vec![],
            index: 0,
        }
    }
    fn structure(name: &'static str, type_name: &str, payload: Vec<u8>) -> Self {
        Self {
            name,
            kind: "StructProperty",
            extras: [string(type_name), vec![0; 17]].concat(),
            payload,
            index: 0,
        }
    }
    fn array(name: &'static str, inner: &str, payload: Vec<u8>) -> Self {
        Self {
            name,
            kind: "ArrayProperty",
            extras: [string(inner), vec![0]].concat(),
            payload,
            index: 0,
        }
    }
    fn property_guid(&mut self) {
        *self.extras.last_mut().unwrap() = 1;
        self.extras.extend([0; 16]);
    }
    fn struct_guid(&mut self) {
        let index = self.extras.len() - 17;
        self.extras[index] = 1;
    }
}
fn stream(tags: &[Tag]) -> Vec<u8> {
    [
        tags.iter().flat_map(Tag::bytes).collect::<Vec<_>>(),
        string("None"),
    ]
    .concat()
}
fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn transform(pose: &NativeTransform) -> Tag {
    Tag::structure(
        "ActorTransform",
        "Transform",
        stream(&[
            Tag::structure("Rotation", "Quat", floats(&pose.rotation)),
            Tag::structure("Translation", "Vector", floats(&pose.translation)),
            Tag::structure("Scale3D", "Vector", floats(&pose.scale)),
        ]),
    )
}
fn pose(x: f32) -> NativeTransform {
    NativeTransform {
        translation: [x, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.6, 0.8],
        scale: [1.25, 2.0, 0.75],
    }
}
#[derive(Clone)]
struct Fixture {
    info: Vec<Tag>,
    roots: Vec<Tag>,
    owners: Vec<Vec<Tag>>,
    recorders: Vec<Vec<u16>>,
    wrapper_extras: Vec<Tag>,
    trailer: [u8; 4],
}
impl Fixture {
    fn new() -> Self {
        let owner = |index: usize| {
            vec![
                Tag::text("ActorClassName", "NameProperty", &units("AuthoredCrate")),
                Tag::text(
                    "ActorPathName",
                    "StrProperty",
                    &units(&format!("/Authored/Owner{index}")),
                ),
                Tag::text(
                    "ObjectFName",
                    "NameProperty",
                    &units(&format!("Crate{index}")),
                ),
                Tag::integer("IcarusActorGUID", if index == 0 { 700 } else { 900 }),
                Tag::integer("RecordedVersion", 3),
                Tag::boolean("bWasMovedToSubLevel", false),
                transform(&pose(if index == 0 { -4.0 } else { 4.0 })),
            ]
        };
        Self {
            info: vec![
                Tag::text("ProspectID", "StrProperty", &[0xd800, 65]),
                Tag::text("LobbyName", "StrProperty", &units("Authored fixture")),
            ],
            roots: vec![
                Tag::integer("Version", 2),
                Tag::text("ProspectMapName", "StrProperty", &units("AuthoredMap")),
                Tag::structure(
                    "LastSavedDateTime",
                    "DateTime",
                    i64::MIN.to_le_bytes().to_vec(),
                ),
            ],
            owners: vec![owner(0), owner(1)],
            recorders: vec![units(C001), units(C001)],
            wrapper_extras: vec![],
            trailer: [0; 4],
        }
    }
    fn payload(&self) -> Vec<u8> {
        let records = self
            .owners
            .iter()
            .zip(&self.recorders)
            .map(|(fields, class)| {
                let bytes = [stream(fields), self.trailer.to_vec()].concat();
                let mut wrapper = vec![
                    Tag::text("ComponentClassName", "StrProperty", class),
                    Tag::array(
                        "BinaryData",
                        "ByteProperty",
                        [
                            i32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec(),
                            bytes,
                        ]
                        .concat(),
                    ),
                ];
                wrapper.extend(self.wrapper_extras.clone());
                stream(&wrapper)
            })
            .collect::<Vec<_>>()
            .concat();
        let shared = Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes();
        let mut roots = self.roots.clone();
        roots.push(Tag::structure(
            "ProspectInfo",
            "ProspectInfo",
            stream(&self.info),
        ));
        roots.push(Tag::array(
            "StateRecorderBlobs",
            "StructProperty",
            [
                i32::try_from(self.owners.len())
                    .unwrap()
                    .to_le_bytes()
                    .to_vec(),
                shared,
            ]
            .concat(),
        ));
        stream(&roots)
    }
    fn decode(&self) -> anyhow::Result<DecodedImportCandidate> {
        DecodedImportCandidate::decode(envelope(&self.payload()).as_slice())
    }
    fn owner_mut(&mut self, name: &str) -> &mut Tag {
        self.owners
            .last_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t.name == name)
            .unwrap()
    }
}
fn envelope(payload: &[u8]) -> Vec<u8> {
    let mut zlib = ZlibEncoder::new(Vec::new(), Compression::fast());
    zlib.write_all(payload).unwrap();
    let compressed = zlib.finish().unwrap();
    let hash: String = Sha1::digest(payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    serde_json::to_vec(&json!({"ProspectBlob": {"Hash": hash, "TotalLength": compressed.len(),
        "DataLength": 0, "UncompressedLength": payload.len(), "BinaryBlob": STANDARD.encode(compressed)}})).unwrap()
}
fn definition() -> NativeCatalogDefinition {
    let placement = |key: &str| NativePlacement {
        key: key.into(),
        class: "crate".into(),
        tile: "tile".into(),
        transform: NativeTransform::default(),
        initial_values: vec![],
    };
    NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "crate".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            variables: vec![],
        }],
        maps: vec![NativeMapDefinition {
            key: "map".into(),
            tiles: vec![NativeTileDefinition {
                key: "tile".into(),
                bounds: Aabb3 {
                    min: [-50.0; 3],
                    max: [50.0; 3],
                },
            }],
            // Canonical placement rank is deliberately opposite source order.
            placements: vec![placement("zulu"), placement("alpha")],
        }],
    }
}
fn catalog() -> Arc<CompiledNativeCatalog> {
    Arc::new(CompiledNativeCatalog::compile(definition()).unwrap())
}
fn bindings(
    candidate: &DecodedImportCandidate,
    catalog: &CompiledNativeCatalog,
) -> C001InertBindingsV1 {
    C001InertBindingsV1 {
        format: 1,
        expected_payload_sha256: candidate
            .preserved()
            .payload_sha256()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        native_catalog_id: catalog.identity().into(),
        source_map: units("AuthoredMap"),
        native_map: "map".into(),
        coordinates: COORDINATES,
        owners: (0..2)
            .map(|i| C001PlacementBindingV1 {
                source_actor_class: units("AuthoredCrate"),
                source_actor_path: units(&format!("/Authored/Owner{i}")),
                source_object_name: units(&format!("Crate{i}")),
                source_icarus_guid: if i == 0 { 700 } else { 900 },
                native_placement: if i == 0 { "zulu" } else { "alpha" }.into(),
                native_class: "crate".into(),
            })
            .collect(),
    }
}
fn prepare(
    fixture: &Fixture,
) -> Result<icarus_bevy::native_import::PreparedC001InertImport, SemanticImportError> {
    let candidate = fixture.decode().unwrap();
    let catalog = catalog();
    let bindings = CompiledC001InertBindings::compile(bindings(&candidate, &catalog)).unwrap();
    prepare_c001_inert_import(&candidate, catalog, &bindings)
}
fn rejected(fixture: &Fixture) {
    if let Ok(candidate) = fixture.decode() {
        let catalog = catalog();
        let bindings = CompiledC001InertBindings::compile(bindings(&candidate, &catalog)).unwrap();
        assert!(prepare_c001_inert_import(&candidate, catalog, &bindings).is_err());
    }
}
fn metadata() -> C001InertImportMetadataV1 {
    prepare(&Fixture::new())
        .unwrap()
        .snapshot()
        .native_import
        .clone()
        .unwrap()
}

#[test]
fn complete_multiowner_fixture_preserves_poses_identity_bytes_and_canonical_ids() {
    let fixture = Fixture::new();
    let candidate = fixture.decode().unwrap();
    for recorder in candidate.recorders() {
        match recorder.typing() {
            icarus_bevy::import_candidate::RecorderTyping::Lossless(_) => {
                let (_, summary) = recorder.typing().lossless().unwrap();
                assert_eq!(summary.uninterpreted, 0, "{summary:?}");
            }
            icarus_bevy::import_candidate::RecorderTyping::Unavailable(reason) => {
                panic!("authored baseline typing unavailable: {reason:?}");
            }
        }
    }
    let catalog = catalog();
    let mut definition = bindings(&candidate, &catalog);
    definition.owners.reverse();
    let compiled = CompiledC001InertBindings::compile(definition).unwrap();
    let prepared = prepare_c001_inert_import(&candidate, catalog, &compiled).unwrap();
    let snapshot = prepared.snapshot();
    assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_C001_INERT);
    assert_eq!(snapshot.tick, 0);
    assert!(snapshot.entities.is_empty());
    assert_eq!(
        snapshot
            .preserved_prospect
            .as_ref()
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        fixture.payload()
    );
    let native = snapshot.native_world.as_ref().unwrap();
    assert_eq!(native.actors[0].id, DomainId(1));
    assert_eq!(native.actors[0].placement, "alpha");
    assert_eq!(native.actors[0].transform, pose(4.0));
    assert_eq!(native.actors[1].id, DomainId(2));
    assert_eq!(native.actors[1].transform, pose(-4.0));
    let metadata = snapshot.native_import.as_ref().unwrap();
    assert_eq!(metadata.adapter, C001InertAdapter::C001InertPlacementV1);
    assert_eq!(metadata.coordinates, COORDINATES);
    assert_eq!(metadata.source_prospect_id, [0xd800, 65]);
    assert_eq!(metadata.source_lobby_name, units("Authored fixture"));
    assert_eq!(metadata.source_last_saved_date_time, Some(i64::MIN));
    assert_eq!(metadata.owners[0].source_index, 0);
    assert_eq!(metadata.owners[0].id, DomainId(2));
    assert_eq!(metadata.owners[0].source_icarus_guid, 700);
    assert_eq!(metadata.owners[1].id, DomainId(1));
    assert_eq!(prepared.inspection().recorders, 2);
    assert_eq!(
        prepared.inspection().metadata_bytes,
        serde_json::to_vec(metadata).unwrap().len()
    );
    assert!(prepared.inspection().metadata_bytes <= MAX_C001_METADATA_BYTES);
    snapshot.validate().unwrap();
    let again = prepare(&fixture).unwrap();
    assert_eq!(again.snapshot(), snapshot);
}

#[test]
fn metadata_checks_immutable_source_but_allows_native_pose_and_tick_changes() {
    let prepared = prepare(&Fixture::new()).unwrap();
    let mut snapshot = prepared.snapshot().clone();
    let original = snapshot.native_import.clone();
    snapshot.tick = 123;
    snapshot.native_world.as_mut().unwrap().actors[0].transform = pose(10.0);
    snapshot.validate().unwrap();
    assert_eq!(snapshot.native_import, original);
    let mut changes = Vec::new();
    let base = original.unwrap();
    let mut changed = base.clone();
    changed.source_prospect_id = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.source_lobby_name = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.source_map = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.source_last_saved_date_time = Some(0);
    changes.push(changed);
    let mut changed = base.clone();
    changed.root_presence.lobby_privacy = true;
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].source_object_name = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].source_actor_class = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].source_actor_path = units("replaced");
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].source_icarus_guid += 1;
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].source_index = 1;
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].id = DomainId(1);
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].native_class = "changed".into();
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners[0].native_placement = "alpha".into();
    changes.push(changed);
    let mut changed = base.clone();
    changed.native_catalog_id = "f".repeat(64);
    changes.push(changed);
    let mut changed = base.clone();
    changed.native_map = "changed".into();
    changes.push(changed);
    let mut changed = base.clone();
    changed.source_payload_sha256 = "f".repeat(64);
    changes.push(changed);
    let mut changed = base.clone();
    changed.source_version = 1;
    changes.push(changed);
    let mut changed = base.clone();
    changed.format = 0;
    changes.push(changed);
    let mut changed = base.clone();
    changed.owners.pop();
    changes.push(changed);
    let mut changed = base;
    changed.root_presence.last_saved_date_time = false;
    changes.push(changed);
    for changed in changes {
        snapshot.native_import = Some(changed);
        assert!(snapshot.validate().is_err());
    }
}

#[test]
fn persisted_metadata_cannot_bypass_noncanonical_component_rejection() {
    let forced_wide = |value: &[u16]| {
        [
            (-i32::try_from(value.len() + 1).unwrap())
                .to_le_bytes()
                .to_vec(),
            value.iter().flat_map(|unit| unit.to_le_bytes()).collect(),
            vec![0, 0],
        ]
        .concat()
    };
    for variant in 0..3 {
        let mut fixture = Fixture::new();
        if variant == 0 {
            fixture.owner_mut("ActorClassName").payload = forced_wide(&units("AuthoredCrate"));
        } else {
            fixture.owners[1].push(Tag {
                name: "OwnerResolvePolicy",
                kind: "ByteProperty",
                extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
                payload: if variant == 1 {
                    vec![0]
                } else {
                    forced_wide(&units("FindOnly"))
                },
                index: 0,
            });
        }
        let candidate = fixture.decode().unwrap();
        let catalog = catalog();
        let bindings = CompiledC001InertBindings::compile(bindings(&candidate, &catalog)).unwrap();
        assert!(prepare_c001_inert_import(&candidate, catalog, &bindings).is_err());
        let mut snapshot = prepare(&Fixture::new()).unwrap().snapshot().clone();
        let preserved = candidate.preserved();
        snapshot
            .native_import
            .as_mut()
            .unwrap()
            .source_payload_sha256 = preserved
            .payload_sha256()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        snapshot.preserved_prospect = Some(Arc::new(
            icarus_bevy::preservation::PreservedSection::from_native_encoding(
                preserved.native_encoding().to_vec(),
                preserved.payload_bytes(),
                preserved.payload_sha256(),
            )
            .unwrap(),
        ));
        assert!(snapshot.validate().is_err());
    }
}

#[test]
fn all_fifteen_nonidentity_prospect_fields_must_be_neutral() {
    let fields = vec![
        Tag::text("ClaimedAccountID", "StrProperty", &units("occupied")),
        Tag::integer("ClaimedAccountCharacter", 1),
        Tag::text("ProspectDTKey", "StrProperty", &units("occupied")),
        Tag::text("FactionMissionDTKey", "StrProperty", &units("occupied")),
        Tag {
            name: "ExpireTime",
            kind: "Int64Property",
            extras: vec![0],
            payload: 1_i64.to_le_bytes().to_vec(),
            index: 0,
        },
        Tag {
            name: "ProspectState",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![1],
            index: 0,
        },
        Tag::array(
            "AssociatedMembers",
            "StructProperty",
            [
                1_i32.to_le_bytes().to_vec(),
                Tag::structure("AssociatedMembers", "AssociatedMemberInfo", stream(&[])).bytes(),
            ]
            .concat(),
        ),
        Tag::integer("Cost", 1),
        Tag::integer("Reward", 1),
        Tag {
            name: "Difficulty",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![1],
            index: 0,
        },
        Tag::boolean("Insurance", true),
        Tag::boolean("NoRespawns", true),
        Tag::integer("ElapsedTime", 1),
        Tag::integer("SelectedDropPoint", 1),
        Tag::array(
            "CustomSettings",
            "StructProperty",
            [
                1_i32.to_le_bytes().to_vec(),
                Tag::structure("CustomSettings", "CustomGameSetting", stream(&[])).bytes(),
            ]
            .concat(),
        ),
    ];
    assert_eq!(fields.len(), 15);
    for field in fields {
        let mut fixture = Fixture::new();
        fixture.info.push(field);
        fixture.decode().unwrap();
        rejected(&fixture);
    }
    for (name, enum_name) in [
        ("ProspectState", "EProspectState"),
        ("Difficulty", "EMissionDifficulty"),
    ] {
        let mut fixture = Fixture::new();
        fixture.info.push(Tag {
            name,
            kind: "EnumProperty",
            extras: [string(enum_name), vec![0]].concat(),
            payload: string("UnresolvedPlaceholder"),
            index: 0,
        });
        let candidate = fixture.decode().unwrap();
        assert_eq!(
            candidate.root_values().source_enum_values()[0].resolved_value,
            None
        );
        assert!(prepare(&fixture).is_err());
    }
}

#[test]
fn roots_require_explicit_current_version_and_no_legacy_even_empty() {
    for version in [0, 1, 3] {
        let mut fixture = Fixture::new();
        fixture.roots[0] = Tag::integer("Version", version);
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.roots.remove(0);
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.roots.retain(|t| t.name != "ProspectMapName");
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.roots[1].payload = string("");
    rejected(&fixture);
    for field in ["ProspectID", "ProspectDTKey"] {
        let mut fixture = Fixture::new();
        fixture.roots.push(Tag::text(field, "StrProperty", &[]));
        rejected(&fixture);
    }
    for value in [1, 2] {
        let mut fixture = Fixture::new();
        fixture.roots.push(Tag {
            name: "LobbyPrivacy",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![value],
            index: 0,
        });
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.info.retain(|t| t.name != "ProspectID");
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.info[0].payload = string("");
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.owners.clear();
    fixture.recorders.clear();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.roots.push(Tag::integer("UnexpectedRoot", 0));
    rejected(&fixture);
}

#[test]
fn every_extra_component_field_rejects_the_last_owner_without_filtering() {
    for field in [
        "Label",
        "ParentCharacterUID",
        "BoolVariables",
        "TileName",
        "Respawn",
        "Unexpected",
        "actorclassname",
    ] {
        let mut fixture = Fixture::new();
        fixture
            .owners
            .last_mut()
            .unwrap()
            .push(Tag::integer(field, 0));
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.wrapper_extras.push(Tag::integer("Unexpected", 0));
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.recorders[1] = units("/Script/Icarus.NoRecorderComponent");
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.recorders[1] = units("/script/Icarus.ActorStateRecorderComponent");
    rejected(&fixture);
}

#[test]
fn required_fields_and_exact_types_apply_to_every_owner() {
    for field in [
        "ActorClassName",
        "ActorPathName",
        "ObjectFName",
        "IcarusActorGUID",
        "RecordedVersion",
        "bWasMovedToSubLevel",
        "ActorTransform",
    ] {
        let mut fixture = Fixture::new();
        fixture
            .owners
            .last_mut()
            .unwrap()
            .retain(|t| t.name != field);
        rejected(&fixture);
        let mut fixture = Fixture::new();
        let t = fixture.owner_mut(field);
        t.kind = "IntProperty";
        t.extras = vec![0];
        t.payload = 0_i32.to_le_bytes().to_vec();
        rejected(&fixture);
    }
    for field in ["ActorClassName", "ActorPathName", "ObjectFName"] {
        let mut fixture = Fixture::new();
        fixture.owner_mut(field).payload = string("");
        rejected(&fixture);
    }
    for value in [-1_i32, 0] {
        let mut fixture = Fixture::new();
        fixture.owner_mut("IcarusActorGUID").payload = value.to_le_bytes().to_vec();
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.owner_mut("IcarusActorGUID").payload = 700_i32.to_le_bytes().to_vec();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.owner_mut("RecordedVersion").payload = 2_i32.to_le_bytes().to_vec();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    *fixture.owner_mut("bWasMovedToSubLevel") = Tag::boolean("bWasMovedToSubLevel", true);
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.owner_mut("ActorPathName").payload = wide(&units("/Authored/Owner0"));
    rejected(&fixture);
}

#[test]
fn owner_policy_accepts_only_authored_find_modes_and_absence() {
    prepare(&Fixture::new()).unwrap();
    for value in [0, 2] {
        let mut fixture = Fixture::new();
        fixture.owners[1].push(Tag {
            name: "OwnerResolvePolicy",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![value],
            index: 0,
        });
        fixture.owners[1].push(Tag::boolean("bSkipDefaultOwnerResolution", false));
        prepare(&fixture).unwrap();
    }
    for kind in ["EnumProperty", "ByteProperty"] {
        for value in [
            "FindOnly",
            "FindOrRespawn",
            "EStateRecorderOwnerResolvePolicy::FindOnly",
            "EStateRecorderOwnerResolvePolicy::FindOrRespawn",
        ] {
            let mut fixture = Fixture::new();
            fixture.owners[1].push(Tag {
                name: "OwnerResolvePolicy",
                kind,
                extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
                payload: string(value),
                index: 0,
            });
            prepare(&fixture).unwrap();
        }
    }
    for value in [1, 3, 4] {
        let mut fixture = Fixture::new();
        fixture.owners[1].push(Tag {
            name: "OwnerResolvePolicy",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![value],
            index: 0,
        });
        rejected(&fixture);
    }
    for value in ["RespawnOnly", "ManuallyResolved", "findonly", "Unknown"] {
        let mut fixture = Fixture::new();
        fixture.owners[1].push(Tag {
            name: "OwnerResolvePolicy",
            kind: "EnumProperty",
            extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
            payload: string(value),
            index: 0,
        });
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.owners[1].push(Tag::boolean("bSkipDefaultOwnerResolution", true));
    rejected(&fixture);
}

#[test]
fn exact_nested_transform_schema_and_all_header_extensions_are_checked() {
    for child in ["Translation", "Rotation", "Scale3D"] {
        let good = [
            Tag::structure("Translation", "Vector", floats(&[0.0; 3])),
            Tag::structure("Rotation", "Quat", floats(&[0.0, 0.0, 0.0, 1.0])),
            Tag::structure("Scale3D", "Vector", floats(&[1.0; 3])),
        ];
        let mutate = |change: &dyn Fn(&mut Vec<Tag>)| {
            let mut children = good.to_vec();
            change(&mut children);
            let mut fixture = Fixture::new();
            fixture.owner_mut("ActorTransform").payload = stream(&children);
            rejected(&fixture);
        };
        mutate(&|tags| tags.retain(|t| t.name != child));
        mutate(&|tags| {
            let p = tags.iter_mut().find(|t| t.name == child).unwrap();
            p.property_guid();
        });
        mutate(&|tags| {
            let p = tags.iter_mut().find(|t| t.name == child).unwrap();
            p.struct_guid();
        });
        mutate(&|tags| {
            let p = tags.iter_mut().find(|t| t.name == child).unwrap();
            p.index = 1;
        });
        mutate(&|tags| {
            let p = tags.iter_mut().find(|t| t.name == child).unwrap();
            p.payload.push(0);
        });
        mutate(&|tags| {
            let p = tags.iter_mut().find(|t| t.name == child).unwrap();
            p.extras = [string("WrongStruct"), vec![0; 17]].concat();
        });
        mutate(&|tags| {
            let p = tags.iter().find(|t| t.name == child).unwrap().clone();
            tags.push(p);
        });
    }
    let mut fixture = Fixture::new();
    fixture.owner_mut("ActorTransform").payload = stream(&[Tag::integer("Extra", 0)]);
    rejected(&fixture);
    for field in [
        "ActorClassName",
        "ActorPathName",
        "ObjectFName",
        "IcarusActorGUID",
        "RecordedVersion",
        "bWasMovedToSubLevel",
        "ActorTransform",
    ] {
        let mut fixture = Fixture::new();
        fixture.owner_mut(field).property_guid();
        rejected(&fixture);
        let mut fixture = Fixture::new();
        fixture.owner_mut(field).index = 1;
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    fixture.owner_mut("ActorTransform").struct_guid();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.roots[0].property_guid();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.roots[2].struct_guid();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.info[0].property_guid();
    rejected(&fixture);
    let mut fixture = Fixture::new();
    fixture.trailer[0] = 1;
    rejected(&fixture);
}

#[test]
fn finite_quaternion_scale_and_transformed_tile_bounds_are_required() {
    let mut invalid = vec![];
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut changed = pose(0.0);
        changed.translation[0] = value;
        invalid.push(changed);
        let mut changed = pose(0.0);
        changed.rotation[0] = value;
        invalid.push(changed);
        let mut changed = pose(0.0);
        changed.scale[0] = value;
        invalid.push(changed);
    }
    for value in [0.0, -1.0] {
        let mut changed = pose(0.0);
        changed.scale[1] = value;
        invalid.push(changed);
    }
    let mut changed = pose(0.0);
    changed.rotation = [0.0; 4];
    invalid.push(changed);
    let mut changed = pose(0.0);
    changed.rotation = [0.0, 0.0, 0.0, 2.0];
    invalid.push(changed);
    invalid.push(pose(51.0));
    let mut changed = pose(49.9);
    changed.scale = [10.0; 3];
    invalid.push(changed);
    for changed in invalid {
        let mut fixture = Fixture::new();
        *fixture.owner_mut("ActorTransform") = transform(&changed);
        rejected(&fixture);
    }
}

#[test]
fn exact_utf16_binding_does_not_fold_case_normalize_or_replace_surrogates() {
    for (field, changed) in [
        ("ActorClassName", units("authoredcrate")),
        ("ActorPathName", units("/authored/Owner1")),
        ("ObjectFName", units("crate1")),
        ("ObjectFName", vec![0xd800]),
        ("ActorClassName", vec![0xfffd]),
        ("ActorPathName", units("/Authored/\u{00e9}")),
    ] {
        let mut fixture = Fixture::new();
        fixture.owner_mut(field).payload = wide(&changed);
        rejected(&fixture);
    }
    let mut fixture = Fixture::new();
    let names = [0xd800, 0x41, 0xdc00];
    fixture.owner_mut("ActorClassName").payload = wide(&names);
    fixture.owner_mut("ActorPathName").payload = wide(&names);
    fixture.owner_mut("ObjectFName").payload = wide(&names);
    let candidate = fixture.decode().unwrap();
    let catalog = catalog();
    let mut dto = bindings(&candidate, &catalog);
    dto.owners[1].source_actor_class = names.to_vec();
    dto.owners[1].source_actor_path = names.to_vec();
    dto.owners[1].source_object_name = names.to_vec();
    let compiled = CompiledC001InertBindings::compile(dto.clone()).unwrap();
    let prepared = prepare_c001_inert_import(&candidate, catalog.clone(), &compiled).unwrap();
    assert_eq!(
        prepared.snapshot().native_import.as_ref().unwrap().owners[1].source_actor_path,
        names
    );
    dto.owners[1].source_actor_path[0] = 0xfffd;
    assert!(
        prepare_c001_inert_import(
            &candidate,
            catalog,
            &CompiledC001InertBindings::compile(dto).unwrap()
        )
        .is_err()
    );
}

#[test]
fn digest_catalog_map_and_complete_placement_profiles_are_pinned() {
    let candidate = Fixture::new().decode().unwrap();
    let catalog = catalog();
    let base = bindings(&candidate, &catalog);
    let mut cases = vec![];
    let mut value = base.clone();
    value.expected_payload_sha256 = "f".repeat(64);
    cases.push(value);
    let mut value = base.clone();
    value.native_catalog_id = "f".repeat(64);
    cases.push(value);
    let mut value = base.clone();
    value.source_map = units("authoredmap");
    cases.push(value);
    let mut value = base.clone();
    value.native_map = "missing".into();
    cases.push(value);
    let mut value = base.clone();
    value.owners.pop();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].native_placement = "missing".into();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].native_class = "missing".into();
    cases.push(value);
    for dto in cases {
        assert!(
            prepare_c001_inert_import(
                &candidate,
                catalog.clone(),
                &CompiledC001InertBindings::compile(dto).unwrap()
            )
            .is_err()
        );
    }
    let mut dto = definition();
    dto.classes[0].variables.push(VariableDefinition {
        key: "state".into(),
        default: NativeValue::I32(0),
    });
    let with_variables = Arc::new(CompiledNativeCatalog::compile(dto).unwrap());
    let dto = CompiledC001InertBindings::compile(bindings(&candidate, &with_variables)).unwrap();
    assert!(prepare_c001_inert_import(&candidate, with_variables, &dto).is_err());
    let mut dto = definition();
    dto.maps[0].placements.pop();
    let incomplete = Arc::new(CompiledNativeCatalog::compile(dto).unwrap());
    let dto = CompiledC001InertBindings::compile(bindings(&candidate, &incomplete)).unwrap();
    assert!(prepare_c001_inert_import(&candidate, incomplete, &dto).is_err());
}

#[test]
fn bindings_compile_rejects_duplicate_empty_oversize_and_invalid_identities() {
    let candidate = Fixture::new().decode().unwrap();
    let catalog = catalog();
    let base = bindings(&candidate, &catalog);
    let mut cases = vec![];
    let mut value = base.clone();
    value.format = 0;
    cases.push(value);
    let mut value = base.clone();
    value.expected_payload_sha256 = "A".repeat(64);
    cases.push(value);
    let mut value = base.clone();
    value.native_catalog_id = "0".repeat(63);
    cases.push(value);
    let mut value = base.clone();
    value.source_map.clear();
    cases.push(value);
    let mut value = base.clone();
    value.source_map = vec![65; 1025];
    cases.push(value);
    let mut value = base.clone();
    value.native_map = "a".repeat(65);
    cases.push(value);
    let mut value = base.clone();
    value.owners.clear();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_icarus_guid = 700;
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_icarus_guid = 0;
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_actor_path = value.owners[0].source_actor_path.clone();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].native_placement = value.owners[0].native_placement.clone();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_actor_class.clear();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_actor_path.clear();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_object_name.clear();
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].source_object_name = vec![65; 1025];
    cases.push(value);
    let mut value = base.clone();
    value.owners[1].native_class = "../bad".into();
    cases.push(value);
    let mut value = base;
    value.owners[1].native_placement.clear();
    cases.push(value);
    for dto in cases {
        assert!(CompiledC001InertBindings::compile(dto).is_err());
    }
}

#[test]
fn bindings_and_metadata_use_strict_named_objects_in_json_and_toml() {
    let candidate = Fixture::new().decode().unwrap();
    let catalog = catalog();
    let dto = bindings(&candidate, &catalog);
    let encoded = serde_json::to_string(&dto).unwrap();
    assert_eq!(
        CompiledC001InertBindings::parse(&encoded)
            .unwrap()
            .definition(),
        &dto
    );
    let toml = toml::to_string(&dto).unwrap();
    assert_eq!(
        CompiledC001InertBindings::parse(&toml)
            .unwrap()
            .definition(),
        &dto
    );
    assert!(CompiledC001InertBindings::parse(&format!("{toml}\nformat=1")).is_err());
    assert!(CompiledC001InertBindings::parse("[]").is_err());
    assert!(
        CompiledC001InertBindings::parse(&format!("{{\"format\":1,{}", &encoded[1..])).is_err()
    );
    let mut value = serde_json::to_value(&dto).unwrap();
    value["extra"] = json!(0);
    assert!(CompiledC001InertBindings::parse(&value.to_string()).is_err());
    let mut value = serde_json::to_value(&dto).unwrap();
    value["owners"][0] = json!([]);
    assert!(CompiledC001InertBindings::parse(&value.to_string()).is_err());
    let mut value = serde_json::to_value(&dto).unwrap();
    value["owners"][0]["extra"] = json!(0);
    assert!(CompiledC001InertBindings::parse(&value.to_string()).is_err());
    assert!(serde_json::from_str::<C001InertBindingsV1>("[]").is_err());
    assert!(serde_json::from_str::<C001PlacementBindingV1>("[]").is_err());
    let owner_text = serde_json::to_string(&dto.owners[0]).unwrap();
    let duplicate = format!("{{\"source_icarus_guid\":700,{}", &owner_text[1..]);
    assert!(serde_json::from_str::<C001PlacementBindingV1>(&duplicate).is_err());
    let metadata = metadata();
    let encoded = serde_json::to_string(&metadata).unwrap();
    assert_eq!(
        serde_json::from_str::<C001InertImportMetadataV1>(&encoded).unwrap(),
        metadata
    );
    assert!(serde_json::from_str::<C001InertImportMetadataV1>("[]").is_err());
    let duplicate = format!("{{\"format\":1,{}", &encoded[1..]);
    assert!(serde_json::from_str::<C001InertImportMetadataV1>(&duplicate).is_err());
    for field in ["root_presence", "owners"] {
        let mut value = serde_json::to_value(&metadata).unwrap();
        if field == "owners" {
            value[field][0] = json!([]);
        } else {
            value[field] = json!([]);
        }
        assert!(serde_json::from_value::<C001InertImportMetadataV1>(value).is_err());
    }
    let mut value = serde_json::to_value(&metadata).unwrap();
    value["root_presence"]["extra"] = json!(true);
    assert!(serde_json::from_value::<C001InertImportMetadataV1>(value).is_err());
    let mut value = serde_json::to_value(&metadata).unwrap();
    value["owners"][0]["extra"] = json!(0);
    assert!(serde_json::from_value::<C001InertImportMetadataV1>(value).is_err());
    let mut value = serde_json::to_value(&dto).unwrap();
    value["coordinates"] = json!("unreal_centimeters");
    assert!(CompiledC001InertBindings::parse(&value.to_string()).is_err());
    let mut value = serde_json::to_value(&metadata).unwrap();
    value["adapter"] = json!("generic_import");
    assert!(serde_json::from_value::<C001InertImportMetadataV1>(value).is_err());
}

#[test]
fn file_admission_and_errors_are_bounded_regular_and_value_free() {
    let candidate = Fixture::new().decode().unwrap();
    let catalog = catalog();
    let dto = bindings(&candidate, &catalog);
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("secret-authored-bindings");
    fs::write(&path, serde_json::to_vec(&dto).unwrap()).unwrap();
    assert_eq!(
        CompiledC001InertBindings::load(&path).unwrap().definition(),
        &dto
    );
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    CompiledC001InertBindings::load(&link).unwrap();
    assert!(CompiledC001InertBindings::load(directory.path()).is_err());
    let fifo = directory.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(CompiledC001InertBindings::load(&fifo).is_err());
    fs::write(&path, vec![b' '; MAX_C001_BINDINGS_BYTES + 1]).unwrap();
    assert!(CompiledC001InertBindings::load(&path).is_err());
    assert!(CompiledC001InertBindings::parse(&" ".repeat(MAX_C001_BINDINGS_BYTES + 1)).is_err());
    let text = format!(
        "{{\"private_unknown_key\":\"private_value\",{}",
        &serde_json::to_string(&dto).unwrap()[1..]
    );
    let e = CompiledC001InertBindings::parse(&text).unwrap_err();
    let diagnostics = format!("{e:?} {e:#}");
    assert!(!diagnostics.contains("private_unknown_key"));
    assert!(!diagnostics.contains("private_value"));
    let e =
        CompiledC001InertBindings::load(&directory.path().join("secret_missing_path")).unwrap_err();
    assert!(!format!("{e:?} {e:#}").contains("secret_missing_path"));
    let mut fixture = Fixture::new();
    fixture.owners[1].push(Tag::text(
        "secret_field",
        "StrProperty",
        &units("secret_value"),
    ));
    let e = prepare(&fixture).unwrap_err();
    assert_eq!(e.recorder_index, Some(1));
    assert!(!format!("{e:?} {e}").contains("secret_field"));
    assert!(!format!("{e:?} {e}").contains("secret_value"));
    for debug in [
        format!("{dto:?}"),
        format!(
            "{:?}",
            CompiledC001InertBindings::compile(dto.clone()).unwrap()
        ),
        format!("{:?}", dto.owners[0]),
        format!("{:?}", metadata()),
        format!("{:?}", metadata().owners[0]),
    ] {
        for private in [
            "AuthoredMap",
            "AuthoredCrate",
            "/Authored/Owner",
            "Crate0",
            "Authored fixture",
        ] {
            assert!(!debug.contains(private));
        }
    }
    let inspection: Value =
        serde_json::to_value(prepare(&Fixture::new()).unwrap().inspection()).unwrap();
    assert!(
        inspection
            .as_object()
            .unwrap()
            .values()
            .all(Value::is_number)
    );
}
