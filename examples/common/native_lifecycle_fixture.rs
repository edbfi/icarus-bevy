//! Scalable, independently authored native worlds. No original inputs or semantics.
// Reuse only the repository's authored wire builders; never call its fixed fixture.
#[path = "../../tests/common/c001_scalar_owners.rs"]
mod wire;

use std::sync::Arc;

use anyhow::{Result, ensure};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_owners::{
        C001OwnerAdapter, C001OwnerBindingV1, C001OwnerBindingsV1, CompiledC001OwnerBindings,
        prepare_c001_owner_import,
    },
    native_c001_scalar_owners::{
        C001ScalarArrayV1, C001ScalarLayoutV1, C001ScalarOwnerAdapter, C001ScalarOwnerBindingV1,
        C001ScalarOwnerBindingsV1, CompiledC001ScalarOwnerBindings,
        prepare_c001_scalar_owner_import,
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
    native_scalar_owners::{
        CompiledScalarOwnerContracts, ScalarOwnerCallback, ScalarOwnerClassV1,
        ScalarOwnerContractsV1,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, NativeValue, VariableDefinition, prepare_authored_world,
    },
    simulation::{DomainId, WorldSnapshot},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use wire::{Tag, stream, units};

pub const STARTING_USES: i32 = 4;
pub const FOUNDATION: DomainId = DomainId(1);
pub const DEPLOYABLE: DomainId = DomainId(2);

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Serialize)]
pub struct Identity {
    pub fixture_sha256: String,
    pub catalog_id: String,
    pub lifecycle_id: String,
    pub relationships_id: String,
    pub scalar_contract_id: Option<String>,
    pub source_payload_sha256: Option<String>,
    pub source_bindings_id: Option<String>,
    pub source_bindings_id_kind: Option<&'static str>,
    pub source_payload_bytes: usize,
    pub source_envelope_bytes: usize,
    pub preserved_encoded_bytes: usize,
    /// Inline snapshot serialization, not the store's generation representation.
    pub snapshot_inline_json_bytes: usize,
    pub source_recorders: usize,
    pub dormant_slots: usize,
}

pub struct Fixture {
    pub snapshot: WorldSnapshot,
    pub identity: Identity,
}

fn key(index: usize) -> String {
    format!("slot{index:04}")
}
fn owner_name(index: usize) -> Vec<u16> {
    units(&format!("AuthoredOwner{index:04}"))
}
fn link(index: usize) -> NativeActorLinkV1 {
    NativeActorLinkV1 {
        name: units(&format!("AuthoredLink{index:04}")),
        guid: 30_000 + index as i32,
    }
}
fn pose(index: usize) -> NativeTransform {
    NativeTransform {
        translation: [(index % 32) as f32 * 2.0, (index / 32) as f32 * 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    }
}
fn source_pose(index: usize) -> NativeTransform {
    let mut transform = pose(index);
    transform.translation[0] += 0.25;
    transform.translation[1] += 0.5;
    transform.translation[2] += 0.75;
    transform.rotation = [0.0, 0.0, 1.0, 0.0];
    transform.scale = [0.75, 1.25, 1.5];
    transform
}
fn source_enabled(index: usize) -> bool {
    index.is_multiple_of(2)
}
fn source_uses(index: usize) -> i32 {
    STARTING_USES + (index % 4) as i32
}
fn absent(index: usize) -> bool {
    index >= 2 && index % 3 == 2
}

impl Fixture {
    pub fn new(schema: u32, slots: usize) -> Result<Self> {
        ensure!((7..=9).contains(&schema), "schema must be 7, 8 or 9");
        ensure!((3..=1024).contains(&slots), "slots must be 3..1024");
        let scalar = schema == 9;
        let catalog = Arc::new(CompiledNativeCatalog::compile(NativeCatalogDefinition {
            native_format: 1,
            frame: NativeFrame::RightHandedZUpMeters,
            classes: vec![NativeClassDefinition {
                key: "authored_box".into(),
                kind: NativeClassKind::SpatialObjectV1,
                local_bounds: Aabb3 {
                    min: [-0.5; 3],
                    max: [0.5; 3],
                },
                variables: if scalar {
                    vec![
                        VariableDefinition {
                            key: "enabled".into(),
                            default: NativeValue::Bool(false),
                        },
                        VariableDefinition {
                            key: "remaining_uses".into(),
                            default: NativeValue::I32(0),
                        },
                    ]
                } else {
                    vec![]
                },
            }],
            maps: vec![NativeMapDefinition {
                key: "authored_map".into(),
                tiles: vec![NativeTileDefinition {
                    key: "authored_tile".into(),
                    bounds: Aabb3 {
                        min: [-100.0; 3],
                        max: [100.0; 3],
                    },
                }],
                placements: (0..slots)
                    .map(|i| NativePlacement {
                        key: key(i),
                        class: "authored_box".into(),
                        tile: "authored_tile".into(),
                        transform: pose(i),
                        initial_values: vec![],
                    })
                    .collect(),
            }],
        })?);
        let relationships = CompiledNativeRelationships::compile(
            NativeRelationshipsDefinition {
                relationship_format: 1,
                native_catalog_id: catalog.identity().into(),
                map: "authored_map".into(),
                actors: (0..slots)
                    .map(|i| NativeRelationshipActorV1 {
                        placement: key(i),
                        identity: link(i),
                        role: match i {
                            0 => NativeRelationshipRole::Foundation,
                            1 => NativeRelationshipRole::Deployable,
                            _ => NativeRelationshipRole::Spatial,
                        },
                        foundation: (i == 1).then(|| link(0)),
                    })
                    .collect(),
            },
            &catalog,
        )?;
        let spatial = prepare_relationship_world(
            prepare_authored_world(catalog.clone(), "authored_map")?.initial_snapshot(),
            &relationships,
        )?
        .snapshot()
        .clone();
        let lifecycle = CompiledNativeLifecycle::compile(
            LifecycleDefinition {
                lifecycle_format: 1,
                native_catalog_id: catalog.identity().into(),
                map: "authored_map".into(),
                slots: (0..slots)
                    .map(|i| SlotDeclaration {
                        placement: key(i),
                        owner_name: owner_name(i),
                        recorder_kind: RecorderKind::SpatialStateV1,
                        initial_presence: if absent(i) {
                            Presence::Absent
                        } else {
                            Presence::Live
                        },
                        initial_component: (!absent(i)).then_some(RecorderComponentRecord {
                            kind: RecorderKind::SpatialStateV1,
                            guid: 10_000 + i as i32,
                        }),
                        spawn_component: SpawnComponent::DeclaredKind,
                    })
                    .collect(),
            },
            &catalog,
        )?;
        let base = prepare_lifecycle_world(spatial, &lifecycle)?.into_snapshot();
        let mut source_payload = None;
        let mut source_bindings = None;
        let mut payload_bytes = 0;
        let mut envelope_bytes = 0;
        let snapshot = if schema == 7 {
            base
        } else {
            // Reverse source order intentionally differs from canonical placement order.
            // Every slot has exactly one source recorder and explicit binding.
            let owners: Vec<_> = (0..slots)
                .rev()
                .map(|i| {
                    let mut fields = vec![
                        Tag::text(
                            "ActorClassName",
                            "NameProperty",
                            &units("AuthoredSourceBox"),
                        ),
                        Tag::text(
                            "ActorPathName",
                            "StrProperty",
                            &units(&format!("/Authored/Box{i:04}")),
                        ),
                        Tag::text(
                            "ObjectFName",
                            "NameProperty",
                            &units(&format!("AuthoredObject{i:04}")),
                        ),
                        Tag::integer("IcarusActorGUID", 40_000 + i as i32),
                        Tag::integer("RecordedVersion", 3),
                        Tag::boolean("bWasMovedToSubLevel", false),
                        wire::transform(&source_pose(i)),
                        Tag {
                            name: "OwnerResolvePolicy",
                            kind: "ByteProperty",
                            extras: [wire::string("None"), vec![0]].concat(),
                            payload: vec![match i % 3 {
                                0 => 0,
                                1 => 1,
                                _ => 2,
                            }],
                            index: 0,
                        },
                    ];
                    if scalar {
                        fields.push(wire::scalar_array(
                            "AuthoredEnabled",
                            "EnabledElement",
                            "AuthoredBoolRecord",
                            Tag::boolean("SavedBool", source_enabled(i)),
                            &units("Enabled"),
                        ));
                        fields.push(wire::scalar_array(
                            "AuthoredUses",
                            "UsesElement",
                            "AuthoredIntRecord",
                            Tag::integer("SavedInt", source_uses(i)),
                            &units("Uses"),
                        ));
                    }
                    fields
                })
                .collect();
            let payload = payload(&owners);
            let envelope = wire::envelope(&payload);
            let source_digest = digest(&payload);
            payload_bytes = payload.len();
            envelope_bytes = envelope.len();
            source_payload = Some(source_digest.clone());
            let candidate = DecodedImportCandidate::decode(envelope.as_slice())?;
            let bindings: Vec<_> = (0..slots)
                .rev()
                .map(|i| C001OwnerBindingV1 {
                    source_actor_class: units("AuthoredSourceBox"),
                    source_actor_path: units(&format!("/Authored/Box{i:04}")),
                    source_object_name: units(&format!("AuthoredObject{i:04}")),
                    source_icarus_guid: 40_000 + i as i32,
                    native_placement: key(i),
                    native_class: "authored_box".into(),
                    native_owner_name: owner_name(i),
                    native_recorder_guid: 20_000 + i as i32,
                    create_if_missing: true,
                    expected_presence: if absent(i) {
                        Presence::Absent
                    } else {
                        Presence::Live
                    },
                    expected_incarnation: u64::from(!absent(i)),
                    expected_prior_binding: None,
                })
                .collect();
            if scalar {
                let contracts = CompiledScalarOwnerContracts::compile(
                    ScalarOwnerContractsV1 {
                        format: 1,
                        native_catalog_id: catalog.identity().into(),
                        native_map: "authored_map".into(),
                        classes: vec![ScalarOwnerClassV1 {
                            native_class: "authored_box".into(),
                            authored_max: 8,
                            callback: ScalarOwnerCallback::RecomputeUseAvailabilityV1,
                        }],
                    },
                    &catalog,
                )?;
                let bindings =
                    CompiledC001ScalarOwnerBindings::compile(C001ScalarOwnerBindingsV1 {
                        format: 1,
                        adapter: C001ScalarOwnerAdapter::C001AuthoredScalarOwnersV1,
                        expected_payload_sha256: source_digest,
                        native_catalog_id: catalog.identity().into(),
                        native_lifecycle_id: lifecycle.identity().into(),
                        native_contract_id: contracts.identity().into(),
                        source_map: units("AuthoredSourceMap"),
                        native_map: "authored_map".into(),
                        coordinates: AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace,
                        layout: C001ScalarLayoutV1 {
                            enabled: C001ScalarArrayV1 {
                                array_name: units("AuthoredEnabled"),
                                element_tag_name: units("EnabledElement"),
                                record_type: units("AuthoredBoolRecord"),
                                value_field: units("SavedBool"),
                            },
                            remaining_uses: C001ScalarArrayV1 {
                                array_name: units("AuthoredUses"),
                                element_tag_name: units("UsesElement"),
                                record_type: units("AuthoredIntRecord"),
                                value_field: units("SavedInt"),
                            },
                        },
                        owners: bindings
                            .into_iter()
                            .map(|owner| C001ScalarOwnerBindingV1 {
                                owner,
                                enabled_source_name: units("Enabled"),
                                remaining_uses_source_name: units("Uses"),
                            })
                            .collect(),
                    })?;
                source_bindings = Some(bindings.identity().into());
                prepare_c001_scalar_owner_import(&candidate, base, &bindings, &contracts)?
                    .into_snapshot()
            } else {
                let bindings = CompiledC001OwnerBindings::compile(C001OwnerBindingsV1 {
                    format: 1,
                    adapter: C001OwnerAdapter::C001AuthoredOwnersV1,
                    expected_payload_sha256: source_digest,
                    native_catalog_id: catalog.identity().into(),
                    native_lifecycle_id: lifecycle.identity().into(),
                    source_map: units("AuthoredSourceMap"),
                    native_map: "authored_map".into(),
                    coordinates: AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace,
                    owners: bindings,
                })?;
                // Schema 8 has no public compiled-binding identity API. Pin its exact
                // compiled definition serialization, explicitly labeled as SHA-256.
                source_bindings = Some(digest(&serde_json::to_vec(bindings.definition())?));
                prepare_c001_owner_import(&candidate, base, &bindings)?.into_snapshot()
            }
        };
        snapshot.validate()?;
        ensure!(snapshot.schema_version == schema, "fixture schema mismatch");
        let inline = serde_json::to_vec(&snapshot)?;
        let identity = Identity {
            fixture_sha256: digest(&inline),
            catalog_id: catalog.identity().into(),
            lifecycle_id: lifecycle.identity().into(),
            relationships_id: snapshot
                .native_relationships
                .as_ref()
                .unwrap()
                .relationship_id
                .clone(),
            scalar_contract_id: snapshot
                .native_scalar_contracts
                .as_ref()
                .map(|c| c.contract_id.clone()),
            source_payload_sha256: source_payload,
            source_bindings_id: source_bindings,
            source_bindings_id_kind: match schema {
                8 => Some("sha256_compiled_definition_json"),
                9 => Some("c001_scalar_owner_bindings_v1"),
                _ => None,
            },
            source_payload_bytes: payload_bytes,
            source_envelope_bytes: envelope_bytes,
            preserved_encoded_bytes: snapshot
                .preserved_prospect
                .as_ref()
                .map_or(0, |p| p.encoded_bytes()),
            snapshot_inline_json_bytes: inline.len(),
            source_recorders: if schema == 7 { 0 } else { slots },
            dormant_slots: snapshot
                .native_lifecycle
                .as_ref()
                .unwrap()
                .slots
                .iter()
                .filter(|s| s.presence == Presence::Absent)
                .count(),
        };
        Ok(Self { snapshot, identity })
    }
}

fn payload(owners: &[Vec<Tag>]) -> Vec<u8> {
    let records: Vec<_> = owners
        .iter()
        .flat_map(|fields| {
            let bytes = [stream(fields), vec![0; 4]].concat();
            stream(&[
                Tag::text(
                    "ComponentClassName",
                    "StrProperty",
                    &units("/Script/Icarus.ActorStateRecorderComponent"),
                ),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [(bytes.len() as i32).to_le_bytes().to_vec(), bytes].concat(),
                ),
            ])
        })
        .collect();
    stream(&[
        Tag::integer("Version", 2),
        Tag::text(
            "ProspectMapName",
            "StrProperty",
            &units("AuthoredSourceMap"),
        ),
        Tag::structure(
            "LastSavedDateTime",
            "DateTime",
            0_i64.to_le_bytes().to_vec(),
        ),
        Tag::structure(
            "ProspectInfo",
            "ProspectInfo",
            stream(&[
                Tag::text("ProspectID", "StrProperty", &units("AuthoredBenchmark")),
                Tag::text(
                    "LobbyName",
                    "StrProperty",
                    &units("Authored native benchmark"),
                ),
            ]),
        ),
        Tag::array(
            "StateRecorderBlobs",
            "StructProperty",
            [
                (owners.len() as i32).to_le_bytes().to_vec(),
                Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes(),
            ]
            .concat(),
        ),
    ])
}

/// A fresh GUID, detached replacement of the live foundation. Requests are
/// constructed before measurement; all other slots and source provenance stay intact.
pub fn replacement(snapshot: &WorldSnapshot) -> RestoreBatch {
    let life = snapshot.native_lifecycle.as_ref().unwrap();
    let actor = snapshot
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.id == FOUNDATION)
        .unwrap();
    let slot = life.slots.iter().find(|s| s.id == FOUNDATION).unwrap();
    RestoreBatch {
        lifecycle_id: life.lifecycle_id.clone(),
        sequence: life.restore_sequence + 1,
        recorders: vec![RestoreRecorder {
            guid: 900_000,
            kind: RecorderKind::SpatialStateV1,
            target_placement: actor.placement.clone(),
            owner_name: owner_name(0),
            policy: ResolvePolicy::RespawnOnly,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: slot.presence,
            expected_incarnation: slot.incarnation,
            transform: actor.transform.clone(),
            variables: actor.variables.clone(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icarus_bevy::{
        config::ServerConfig,
        native_lifecycle::{
            CompletedRecorder, ComponentDisposition, OwnerHandle, RecorderBinding,
            RestoreDisposition, SlotRecord,
        },
        native_lifecycle_runtime::{native_owner_handle, scalar_owner_state},
        native_world::{NativeActorRecord, NativeVariableValue},
        simulation::{build_app, capture},
    };

    #[test]
    fn every_slot_restores_its_own_source_state_and_binding() -> Result<()> {
        // Cross the coordinate row boundary and exercise all three policies,
        // both initial presences, and each distinct scalar combination.
        let slots = 35;
        for schema in 7..=9 {
            let fixture = Fixture::new(schema, slots)?;
            let snapshot = &fixture.snapshot;
            let world = snapshot.native_world.as_ref().unwrap();
            let life = snapshot.native_lifecycle.as_ref().unwrap();
            let relationships = snapshot.native_relationships.as_ref().unwrap();
            assert_eq!(world.actors.len(), slots);
            assert_eq!(life.slots.len(), slots);
            assert_eq!(life.bindings.len(), if schema == 7 { 24 } else { slots });
            assert_eq!(relationships.foundations.len(), 1);
            assert_eq!(relationships.foundations[0].owner, DEPLOYABLE);
            assert_eq!(relationships.foundations[0].foundation, Some(FOUNDATION));
            if schema == 7 {
                assert_eq!(life.restore_sequence, 0);
                assert!(life.completed.is_none());
            } else {
                let completed = life.completed.as_ref().unwrap();
                assert_eq!(life.restore_sequence, 1);
                assert_eq!(completed.sequence, 1);
                assert_eq!(completed.outcomes.len(), slots);
                if schema == 8 {
                    let metadata = snapshot.native_owner_import.as_ref().unwrap();
                    assert_eq!(metadata.owners.len(), slots);
                    assert_eq!(&metadata.initial_completion, completed);
                } else {
                    let metadata = snapshot.native_scalar_owner_import.as_ref().unwrap();
                    assert_eq!(metadata.resolved.len(), slots);
                    assert_eq!(metadata.bindings.owners.len(), slots);
                    assert_eq!(&metadata.initial_completion, completed);
                }
            }
            let mut app = build_app(&ServerConfig::default(), snapshot)?;
            assert_eq!(capture(app.world_mut())?, *snapshot);
            for i in 0..slots {
                let id = DomainId(i as u64 + 1);
                let initial_absent = i % 3 == 2;
                let live = schema != 7 || !initial_absent;
                let incarnation = if !live {
                    0
                } else if schema != 7 && i % 3 == 1 {
                    2
                } else {
                    1
                };
                let handle = OwnerHandle { id, incarnation };
                let guid = if schema == 7 { 10_000 } else { 20_000 } + i as i32;
                let actor = world.actors.iter().find(|a| a.id == id).unwrap();
                let expected_transform = NativeTransform {
                    translation: [
                        (i % 32) as f32 * 2.0 + if schema == 7 { 0.0 } else { 0.25 },
                        (i / 32) as f32 * 2.0 + if schema == 7 { 0.0 } else { 0.5 },
                        if schema == 7 { 3.0 } else { 3.75 },
                    ],
                    rotation: if schema == 7 {
                        [0.0, 0.0, 0.0, 1.0]
                    } else {
                        [0.0, 0.0, 1.0, 0.0]
                    },
                    scale: if schema == 7 {
                        [1.0; 3]
                    } else {
                        [0.75, 1.25, 1.5]
                    },
                };
                let enabled = i & 1 == 0;
                let uses = 4 + (i % 4) as i32;
                let variables = if schema == 9 {
                    vec![
                        NativeVariableValue {
                            key: "enabled".into(),
                            value: NativeValue::Bool(enabled),
                        },
                        NativeVariableValue {
                            key: "remaining_uses".into(),
                            value: NativeValue::I32(uses),
                        },
                    ]
                } else {
                    vec![]
                };
                assert_eq!(
                    actor,
                    &NativeActorRecord {
                        id,
                        placement: format!("slot{i:04}"),
                        class: "authored_box".into(),
                        tile: "authored_tile".into(),
                        transform: expected_transform,
                        variables,
                    }
                );
                assert_eq!(
                    life.slots.iter().find(|s| s.id == id).unwrap(),
                    &SlotRecord {
                        id,
                        presence: if live {
                            Presence::Live
                        } else {
                            Presence::Absent
                        },
                        incarnation,
                        component: live.then_some(RecorderComponentRecord {
                            kind: RecorderKind::SpatialStateV1,
                            guid,
                        }),
                    }
                );
                let binding = life.bindings.iter().find(|b| b.owner.id == id);
                if live {
                    assert_eq!(
                        binding,
                        Some(&RecorderBinding {
                            guid,
                            kind: RecorderKind::SpatialStateV1,
                            owner: handle
                        })
                    );
                    assert_eq!(native_owner_handle(app.world(), id)?, handle);
                } else {
                    assert!(binding.is_none());
                    assert!(native_owner_handle(app.world(), id).is_err());
                }
                if schema == 7 {
                    continue;
                }
                let placement = world.definitions.maps[0]
                    .placements
                    .iter()
                    .find(|p| p.key == actor.placement)
                    .unwrap();
                assert_ne!(actor.transform, placement.transform);
                let policy = match i % 3 {
                    0 => ResolvePolicy::FindOnly,
                    1 => ResolvePolicy::RespawnOnly,
                    _ => ResolvePolicy::FindOrRespawn,
                };
                let expected_binding = C001OwnerBindingV1 {
                    source_actor_class: units("AuthoredSourceBox"),
                    source_actor_path: units(&format!("/Authored/Box{i:04}")),
                    source_object_name: units(&format!("AuthoredObject{i:04}")),
                    source_icarus_guid: 40_000 + i as i32,
                    native_placement: format!("slot{i:04}"),
                    native_class: "authored_box".into(),
                    native_owner_name: units(&format!("AuthoredOwner{i:04}")),
                    native_recorder_guid: guid,
                    create_if_missing: true,
                    expected_presence: if initial_absent {
                        Presence::Absent
                    } else {
                        Presence::Live
                    },
                    expected_incarnation: u64::from(!initial_absent),
                    expected_prior_binding: None,
                };
                let source_index = slots - 1 - i;
                if schema == 8 {
                    let resolved = snapshot
                        .native_owner_import
                        .as_ref()
                        .unwrap()
                        .owners
                        .iter()
                        .find(|r| r.id == id)
                        .unwrap();
                    assert_eq!(resolved.source_index, source_index);
                    assert_eq!(resolved.source_policy, policy);
                    assert_eq!(resolved.binding, expected_binding);
                } else {
                    let metadata = snapshot.native_scalar_owner_import.as_ref().unwrap();
                    let resolved = metadata.resolved.iter().find(|r| r.id == id).unwrap();
                    assert_eq!(resolved.source_index as usize, source_index);
                    assert_eq!(resolved.binding_index as usize, i);
                    assert_eq!(resolved.source_policy, policy);
                    let binding = &metadata.bindings.owners[resolved.binding_index as usize];
                    assert_eq!(binding.owner, expected_binding);
                    assert_eq!(binding.enabled_source_name, units("Enabled"));
                    assert_eq!(binding.remaining_uses_source_name, units("Uses"));
                    let state = scalar_owner_state(app.world(), handle)?;
                    assert_eq!(state.owner, handle);
                    assert_eq!(state.enabled, enabled);
                    assert_eq!(state.remaining_uses, uses);
                    assert_eq!(state.available, enabled && uses > 0);
                    for variable in &actor.variables {
                        let default = world.definitions.classes[0]
                            .variables
                            .iter()
                            .find(|v| v.key == variable.key)
                            .unwrap();
                        if variable.key == "remaining_uses" {
                            assert_ne!(variable.value, default.default);
                        }
                    }
                }
                let completed = life.completed.as_ref().unwrap();
                assert_eq!(
                    completed.outcomes.iter().find(|o| o.target == id).unwrap(),
                    &CompletedRecorder {
                        guid,
                        target: id,
                        policy,
                        source: RecorderSource::Detached {
                            expected_prior_binding: None
                        },
                        before_presence: if initial_absent {
                            Presence::Absent
                        } else {
                            Presence::Live
                        },
                        before_incarnation: u64::from(!initial_absent),
                        owner: handle,
                        disposition: match i % 3 {
                            0 => RestoreDisposition::Attached,
                            1 => RestoreDisposition::Replaced,
                            _ => RestoreDisposition::Spawned,
                        },
                        component_disposition: ComponentDisposition::Existing,
                    }
                );
            }
        }
        Ok(())
    }

    #[test]
    fn deterministic_complete_authored_fixtures() -> Result<()> {
        for schema in 7..=9 {
            for slots in [3, 8] {
                let a = Fixture::new(schema, slots)?;
                let b = Fixture::new(schema, slots)?;
                assert_eq!(a.snapshot, b.snapshot);
                assert_eq!(a.identity.fixture_sha256, b.identity.fixture_sha256);
                assert_eq!(
                    a.snapshot.native_world.as_ref().unwrap().actors.len(),
                    slots
                );
                assert_eq!(
                    a.snapshot.native_lifecycle.as_ref().unwrap().slots.len(),
                    slots
                );
                if schema == 7 {
                    assert!(a.identity.dormant_slots > 0);
                } else {
                    assert_eq!(a.identity.source_recorders, slots);
                    assert_eq!(a.identity.dormant_slots, 0);
                    assert!(a.snapshot.preserved_prospect.is_some());
                }
                if schema == 8 {
                    assert_eq!(
                        a.snapshot
                            .native_owner_import
                            .as_ref()
                            .unwrap()
                            .owners
                            .len(),
                        slots
                    );
                }
                if schema == 9 {
                    assert_eq!(
                        a.snapshot
                            .native_scalar_owner_import
                            .as_ref()
                            .unwrap()
                            .resolved
                            .len(),
                        slots
                    );
                }
            }
        }
        assert!(Fixture::new(6, 3).is_err());
        assert!(Fixture::new(7, 2).is_err());
        assert!(Fixture::new(9, 1025).is_err());
        Ok(())
    }
}
