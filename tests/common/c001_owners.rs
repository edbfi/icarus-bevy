//! Shared independently authored source and native owner fixtures.
#![allow(dead_code)]
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_owners::{
        C001OwnerAdapter, C001OwnerBindingV1, C001OwnerBindingsV1, CompiledC001OwnerBindings,
        PreparedC001OwnerImport, prepare_c001_owner_import,
    },
    native_import::AuthoredCoordinatesV1,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, Presence, RecorderComponentRecord,
        RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
        SlotDeclaration, SpawnComponent, prepare_lifecycle_world,
    },
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition, prepare_relationship_world,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, prepare_authored_world,
    },
    simulation::WorldSnapshot,
};
use serde_json::json;
use sha1::{Digest, Sha1};
use std::{io::Write, sync::Arc};
const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
pub fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
pub fn string(value: &str) -> Vec<u8> {
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
pub struct Tag {
    pub name: &'static str,
    pub kind: &'static str,
    pub extras: Vec<u8>,
    pub payload: Vec<u8>,
    pub index: i32,
}
impl Tag {
    pub fn bytes(&self) -> Vec<u8> {
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
    pub fn integer(name: &'static str, value: i32) -> Self {
        Self {
            name,
            kind: "IntProperty",
            extras: vec![0],
            payload: value.to_le_bytes().to_vec(),
            index: 0,
        }
    }
    pub fn text(name: &'static str, kind: &'static str, value: &[u16]) -> Self {
        Self {
            name,
            kind,
            extras: vec![0],
            payload: wide(value),
            index: 0,
        }
    }
    pub fn boolean(name: &'static str, value: bool) -> Self {
        Self {
            name,
            kind: "BoolProperty",
            extras: vec![u8::from(value), 0],
            payload: vec![],
            index: 0,
        }
    }
    pub fn structure(name: &'static str, type_name: &str, payload: Vec<u8>) -> Self {
        Self {
            name,
            kind: "StructProperty",
            extras: [string(type_name), vec![0; 17]].concat(),
            payload,
            index: 0,
        }
    }
    pub fn array(name: &'static str, inner: &str, payload: Vec<u8>) -> Self {
        Self {
            name,
            kind: "ArrayProperty",
            extras: [string(inner), vec![0]].concat(),
            payload,
            index: 0,
        }
    }
    pub fn property_guid(&mut self) {
        *self.extras.last_mut().unwrap() = 1;
        self.extras.extend([0; 16]);
    }
    pub fn struct_guid(&mut self) {
        let index = self.extras.len() - 17;
        self.extras[index] = 1;
    }
}
pub fn stream(tags: &[Tag]) -> Vec<u8> {
    [
        tags.iter().flat_map(Tag::bytes).collect::<Vec<_>>(),
        string("None"),
    ]
    .concat()
}
pub fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
pub fn transform(pose: &NativeTransform) -> Tag {
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

pub struct Fixture {
    pub catalog: Arc<CompiledNativeCatalog>,
    pub base: WorldSnapshot,
    pub bindings: C001OwnerBindingsV1,
    pub payload: Vec<u8>,
    pub envelope: Vec<u8>,
    /// Source ordinal order: find zulu, spawn alpha, replace middle.
    pub transforms: Vec<NativeTransform>,
    pub owners: Vec<Vec<Tag>>,
    pub roots: Vec<Tag>,
    pub info: Vec<Tag>,
}
impl Fixture {
    pub fn candidate(&self) -> DecodedImportCandidate {
        DecodedImportCandidate::decode(self.envelope.as_slice()).unwrap()
    }
    pub fn prepared(&self) -> PreparedC001OwnerImport {
        prepare_c001_owner_import(
            &self.candidate(),
            self.base.clone(),
            &CompiledC001OwnerBindings::compile(self.bindings.clone()).unwrap(),
        )
        .unwrap()
    }
    pub fn snapshot(&self) -> WorldSnapshot {
        self.prepared().into_snapshot()
    }
    /// Rebuild source identities/digest after intentional authored wire edits.
    pub fn rebuild_source(&mut self) {
        self.payload = payload(&self.roots, &self.info, &self.owners);
        self.envelope = envelope(&self.payload);
        self.bindings.expected_payload_sha256 = sha2::Sha256::digest(&self.payload)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
    }
    pub fn initial_batch(&self) -> RestoreBatch {
        RestoreBatch {
            lifecycle_id: self.bindings.native_lifecycle_id.clone(),
            sequence: 1,
            recorders: self
                .bindings
                .owners
                .iter()
                .zip(&self.transforms)
                .enumerate()
                .map(|(i, (b, t))| RestoreRecorder {
                    guid: b.native_recorder_guid,
                    kind: RecorderKind::SpatialStateV1,
                    target_placement: b.native_placement.clone(),
                    owner_name: b.native_owner_name.clone(),
                    policy: [
                        ResolvePolicy::FindOnly,
                        ResolvePolicy::FindOrRespawn,
                        ResolvePolicy::RespawnOnly,
                    ][i],
                    source: RecorderSource::Detached {
                        expected_prior_binding: b.expected_prior_binding,
                    },
                    create_if_missing: b.create_if_missing,
                    expected_presence: b.expected_presence,
                    expected_incarnation: b.expected_incarnation,
                    transform: t.clone(),
                    variables: vec![],
                })
                .collect(),
        }
    }
}
pub fn envelope(payload: &[u8]) -> Vec<u8> {
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
fn payload(roots: &[Tag], info: &[Tag], owners: &[Vec<Tag>]) -> Vec<u8> {
    let records = owners
        .iter()
        .map(|fields| {
            let bytes = [stream(fields), vec![0; 4]].concat();
            stream(&[
                Tag::text("ComponentClassName", "StrProperty", &units(C001)),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [
                        i32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec(),
                        bytes,
                    ]
                    .concat(),
                ),
            ])
        })
        .collect::<Vec<_>>()
        .concat();
    let mut roots = roots.to_vec();
    roots.push(Tag::structure("ProspectInfo", "ProspectInfo", stream(info)));
    roots.push(Tag::array(
        "StateRecorderBlobs",
        "StructProperty",
        [
            i32::try_from(owners.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes(),
        ]
        .concat(),
    ));
    stream(&roots)
}
pub fn fixture() -> Fixture {
    fixture_variant(0)
}
pub fn fixture_variant(variant: u8) -> Fixture {
    let placements = ["alpha", "middle", "zulu"];
    let catalog = Arc::new(
        CompiledNativeCatalog::compile(NativeCatalogDefinition {
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
                placements: placements
                    .iter()
                    .map(|key| NativePlacement {
                        key: (*key).into(),
                        class: "crate".into(),
                        tile: "tile".into(),
                        transform: NativeTransform::default(),
                        initial_values: vec![],
                    })
                    .collect(),
            }],
        })
        .unwrap(),
    );
    let spatial = prepare_authored_world(catalog.clone(), "map")
        .unwrap()
        .initial_snapshot();
    let relationships = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            actors: placements
                .iter()
                .enumerate()
                .map(|(i, key)| NativeRelationshipActorV1 {
                    placement: (*key).into(),
                    identity: NativeActorLinkV1 {
                        name: units(&format!("Link{i}")),
                        guid: 800 + i as i32,
                    },
                    role: if i == 2 {
                        NativeRelationshipRole::Deployable
                    } else {
                        NativeRelationshipRole::Foundation
                    },
                    foundation: (i == 2).then(|| NativeActorLinkV1 {
                        name: units("Link1"),
                        guid: 801,
                    }),
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let spatial = prepare_relationship_world(spatial, &relationships)
        .unwrap()
        .snapshot()
        .clone();
    let names = ["SpawnOwner", "ReplaceOwner", "FindOwner"];
    let lifecycle = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            slots: placements
                .iter()
                .enumerate()
                .map(|(i, key)| SlotDeclaration {
                    placement: (*key).into(),
                    owner_name: units(names[i]),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: if i == 0 {
                        Presence::Absent
                    } else {
                        Presence::Live
                    },
                    initial_component: (i == 1).then_some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: 41,
                    }),
                    spawn_component: SpawnComponent::Missing,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let base = prepare_lifecycle_world(spatial, &lifecycle)
        .unwrap()
        .into_snapshot();
    let transforms = vec![pose(-4.0), pose(0.0), pose(4.0)];
    let owners = transforms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut fields = vec![
                Tag::text("ActorClassName", "NameProperty", &units("SourceCrate")),
                Tag::text(
                    "ActorPathName",
                    "StrProperty",
                    &units(&format!("/Authored/Owner{i}")),
                ),
                Tag::text(
                    "ObjectFName",
                    "NameProperty",
                    &units(&format!("SourceObject{i}")),
                ),
                Tag::integer("IcarusActorGUID", 700 + i as i32),
                Tag::integer("RecordedVersion", 3),
                Tag::boolean("bWasMovedToSubLevel", false),
                transform(t),
            ];
            if i != 1 {
                fields.push(Tag {
                    name: "OwnerResolvePolicy",
                    kind: "ByteProperty",
                    extras: [string("None"), vec![0]].concat(),
                    payload: vec![if i == 0 { 0 } else { 1 }],
                    index: 0,
                });
            }
            fields
        })
        .collect();
    let bindings = C001OwnerBindingsV1 {
        format: 1,
        adapter: C001OwnerAdapter::C001AuthoredOwnersV1,
        expected_payload_sha256: "0".repeat(64),
        native_catalog_id: catalog.identity().into(),
        native_lifecycle_id: lifecycle.identity().into(),
        source_map: units("AuthoredMap"),
        native_map: "map".into(),
        coordinates: AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace,
        owners: [2, 0, 1]
            .into_iter()
            .enumerate()
            .map(|(source, i)| C001OwnerBindingV1 {
                source_actor_class: units("SourceCrate"),
                source_actor_path: units(&format!("/Authored/Owner{source}")),
                source_object_name: units(&format!("SourceObject{source}")),
                source_icarus_guid: 700 + source as i32,
                native_placement: placements[i].into(),
                native_class: "crate".into(),
                native_owner_name: units(names[i]),
                native_recorder_guid: 100 + source as i32,
                create_if_missing: true,
                expected_presence: if i == 0 {
                    Presence::Absent
                } else {
                    Presence::Live
                },
                expected_incarnation: u64::from(i != 0),
                expected_prior_binding: None,
            })
            .collect(),
    };
    let mut fixture = Fixture {
        catalog,
        base,
        bindings,
        transforms,
        owners,
        payload: vec![],
        envelope: vec![],
        roots: vec![
            Tag::integer("Version", 2),
            Tag::text("ProspectMapName", "StrProperty", &units("AuthoredMap")),
            Tag::structure(
                "LastSavedDateTime",
                "DateTime",
                i64::from(variant).to_le_bytes().to_vec(),
            ),
        ],
        info: vec![
            Tag::text("ProspectID", "StrProperty", &[0xd800, 65]),
            Tag::text("LobbyName", "StrProperty", &units("Authored owner fixture")),
        ],
    };
    fixture.rebuild_source();
    fixture
}
pub fn imported() -> WorldSnapshot {
    fixture().snapshot()
}
pub fn imported_variant(variant: u8) -> WorldSnapshot {
    fixture_variant(variant).snapshot()
}
