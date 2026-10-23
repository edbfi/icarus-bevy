//! Independently authored Bevy lifecycle fixtures; no original inputs or CLI.
use std::{collections::BTreeMap, sync::Arc};

use bevy::{
    ecs::entity_disabling::{DefaultQueryFilters, Disabled},
    prelude::*,
};
use icarus_bevy::{
    config::ServerConfig,
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition,
    },
    native_runtime::{
        NativeActorIdentity, NativeFoundation, NativeFoundationDependents, NativeLocalBounds,
        NativeRelationshipIdentity, NativeRelationshipsContext, NativeRotation, NativeScale,
        NativeVariables, NativeWorldContext, native_dependents_of, native_foundation_of,
        native_world_bounds, reassign_native_foundation, update_native_actor,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, prepare_authored_world,
    },
    persistence::SnapshotStore,
    simulation::{
        DomainId, Position, SNAPSHOT_SCHEMA_RELATIONSHIPS, SimulationClock, SnapshotMode,
        WorldSnapshot, build_app, capture,
    },
};

const A: DomainId = DomainId(1);
const B: DomainId = DomainId(2);
const FIRST: DomainId = DomainId(3);
const SECOND: DomainId = DomainId(4);
const SPATIAL: DomainId = DomainId(5);

fn link(index: usize) -> NativeActorLinkV1 {
    NativeActorLinkV1 {
        name: format!("SecretAuthoredActor{index}")
            .encode_utf16()
            .collect(),
        guid: i32::try_from(index + 10).unwrap(),
    }
}

fn snapshot() -> WorldSnapshot {
    let placements = [
        "a_foundation",
        "b_foundation",
        "c_deployable",
        "d_deployable",
        "e_spatial",
    ];
    let catalog = Arc::new(
        CompiledNativeCatalog::compile(NativeCatalogDefinition {
            native_format: 1,
            frame: NativeFrame::RightHandedZUpMeters,
            classes: vec![NativeClassDefinition {
                key: "secret_box".into(),
                kind: NativeClassKind::SpatialObjectV1,
                local_bounds: Aabb3 {
                    min: [-1.0; 3],
                    max: [1.0; 3],
                },
                variables: Vec::new(),
            }],
            maps: vec![NativeMapDefinition {
                key: "secret_map".into(),
                tiles: vec![NativeTileDefinition {
                    key: "secret_tile".into(),
                    bounds: Aabb3 {
                        min: [-100.0; 3],
                        max: [100.0; 3],
                    },
                }],
                placements: placements
                    .iter()
                    .map(|key| NativePlacement {
                        key: (*key).into(),
                        class: "secret_box".into(),
                        tile: "secret_tile".into(),
                        transform: NativeTransform::default(),
                        initial_values: Vec::new(),
                    })
                    .collect(),
            }],
        })
        .unwrap(),
    );
    let declarations = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "secret_map".into(),
            actors: placements
                .iter()
                .enumerate()
                .rev()
                .map(|(index, placement)| NativeRelationshipActorV1 {
                    placement: (*placement).into(),
                    identity: link(index),
                    role: match index {
                        0 | 1 => NativeRelationshipRole::Foundation,
                        2 | 3 => NativeRelationshipRole::Deployable,
                        _ => NativeRelationshipRole::Spatial,
                    },
                    foundation: (index == 2).then(|| link(0)),
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let mut snapshot = prepare_authored_world(catalog, "secret_map")
        .unwrap()
        .initial_snapshot();
    snapshot.schema_version = SNAPSHOT_SCHEMA_RELATIONSHIPS;
    snapshot.native_relationships = Some(declarations.initial_snapshot());
    snapshot
}

fn app() -> App {
    build_app(&ServerConfig::default(), &snapshot()).unwrap()
}

fn entity(world: &World, id: DomainId) -> Entity {
    world
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&id))
        .unwrap()
        .id()
}

fn edges(world: &World) -> BTreeMap<DomainId, Option<DomainId>> {
    world
        .iter_entities()
        .filter_map(|entity| {
            Some((
                *entity.get::<DomainId>()?,
                entity.get::<NativeFoundation>()?.0,
            ))
        })
        .collect()
}

fn reverse(world: &World) -> Option<BTreeMap<DomainId, Vec<DomainId>>> {
    world
        .get_resource::<NativeFoundationDependents>()
        .map(|index| index.0.clone())
}

/// Every public operation checks unrelated damage before any proposed write.
fn reject_all(world: &mut World) {
    let before_edges = edges(world);
    let before_reverse = reverse(world);
    let spatial = entity(world, SPATIAL);
    let before_position = *world.get::<Position>(spatial).unwrap();
    assert!(capture(world).is_err());
    assert!(native_foundation_of(world, FIRST).is_err());
    assert!(native_dependents_of(world, A).is_err());
    assert!(reassign_native_foundation(world, FIRST, Some(B)).is_err());
    // No-op edits must reject damage too, rather than bypassing validation.
    assert!(reassign_native_foundation(world, SECOND, None).is_err());
    assert!(native_world_bounds(world, SPATIAL).is_err());
    assert!(
        update_native_actor(
            world,
            SPATIAL,
            NativeTransform {
                translation: [7.0, 8.0, 9.0],
                ..Default::default()
            },
            Vec::new(),
        )
        .is_err()
    );
    assert_eq!(edges(world), before_edges);
    assert_eq!(reverse(world), before_reverse);
    assert_eq!(*world.get::<Position>(spatial).unwrap(), before_position);
}

#[test]
fn activation_installs_exact_roles_required_edges_and_empty_reverse_entries() {
    let mut app = app();
    app.update();
    assert_eq!(native_foundation_of(app.world(), FIRST).unwrap(), Some(A));
    assert_eq!(native_foundation_of(app.world(), SECOND).unwrap(), None);
    assert_eq!(native_dependents_of(app.world(), A).unwrap(), [FIRST]);
    assert!(native_dependents_of(app.world(), B).unwrap().is_empty());
    assert_eq!(
        reverse(app.world()).unwrap(),
        BTreeMap::from([(A, vec![FIRST]), (B, vec![])])
    );
    for (id, role) in [
        (A, NativeRelationshipRole::Foundation),
        (B, NativeRelationshipRole::Foundation),
        (FIRST, NativeRelationshipRole::Deployable),
        (SECOND, NativeRelationshipRole::Deployable),
        (SPATIAL, NativeRelationshipRole::Spatial),
    ] {
        let entity = entity(app.world(), id);
        let actual = app
            .world()
            .get::<NativeRelationshipIdentity>(entity)
            .unwrap();
        assert!(actual.role == role);
        assert!(actual.identity == link(usize::try_from(id.0 - 1).unwrap()));
        assert_eq!(
            app.world().get::<NativeFoundation>(entity).is_some(),
            role == NativeRelationshipRole::Deployable
        );
    }
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
}

#[test]
fn reassignment_shared_reverse_noops_and_reload_preserve_edited_links() {
    let initial = snapshot();
    let mut app = app();
    reassign_native_foundation(app.world_mut(), FIRST, Some(A)).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    reassign_native_foundation(app.world_mut(), FIRST, Some(B)).unwrap();
    assert!(native_dependents_of(app.world(), A).unwrap().is_empty());
    assert_eq!(native_dependents_of(app.world(), B).unwrap(), [FIRST]);
    reassign_native_foundation(app.world_mut(), SECOND, Some(B)).unwrap();
    assert_eq!(
        native_dependents_of(app.world(), B).unwrap(),
        [FIRST, SECOND]
    );
    let shared = capture(app.world_mut()).unwrap();
    reassign_native_foundation(app.world_mut(), SECOND, Some(B)).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), shared);
    reassign_native_foundation(app.world_mut(), FIRST, None).unwrap();
    reassign_native_foundation(app.world_mut(), FIRST, None).unwrap();
    assert_eq!(native_foundation_of(app.world(), FIRST).unwrap(), None);
    assert_eq!(native_dependents_of(app.world(), B).unwrap(), [SECOND]);
    app.world_mut().resource_mut::<SimulationClock>().tick = 17;
    let changed = capture(app.world_mut()).unwrap();
    assert_eq!(changed.schema_version, 6);
    assert_eq!(
        changed.native_relationships.as_ref().unwrap().definition,
        initial.native_relationships.as_ref().unwrap().definition
    );
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&initial).unwrap();
    store.save(&changed).unwrap();
    drop(store);
    let loaded = SnapshotStore::open(directory.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded, changed);
    let mut restored = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(capture(restored.world_mut()).unwrap(), changed);
    assert_eq!(native_foundation_of(restored.world(), FIRST).unwrap(), None);
    assert_eq!(native_dependents_of(restored.world(), B).unwrap(), [SECOND]);
}

#[test]
fn invalid_owner_or_target_rejects_without_writes_or_payload_in_errors() {
    let mut app = app();
    let initial = capture(app.world_mut()).unwrap();
    for (owner, target) in [
        (A, Some(B)),
        (SPATIAL, Some(A)),
        (DomainId(99), None),
        (FIRST, Some(SPATIAL)),
        (FIRST, Some(SECOND)),
        (FIRST, Some(FIRST)),
        (FIRST, Some(DomainId(99))),
    ] {
        let error = reassign_native_foundation(app.world_mut(), owner, target).unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(!diagnostic.contains("Secret"));
        assert!(!diagnostic.contains("secret_"));
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
    }
    for id in [A, B, SPATIAL, DomainId(99)] {
        assert!(native_foundation_of(app.world(), id).is_err());
    }
    for id in [FIRST, SECOND, SPATIAL, DomainId(99)] {
        assert!(native_dependents_of(app.world(), id).is_err());
    }
}

fn missing_component<T: Component + Clone>(id: DomainId) {
    let mut app = app();
    let actor = entity(app.world(), id);
    let original = app.world().get::<T>(actor).unwrap().clone();
    app.world_mut().entity_mut(actor).remove::<T>();
    reject_all(app.world_mut());
    app.world_mut().entity_mut(actor).insert(original);
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    reassign_native_foundation(app.world_mut(), FIRST, Some(B)).unwrap();
    assert_eq!(native_foundation_of(app.world(), FIRST).unwrap(), Some(B));
}

#[test]
fn missing_required_actor_components_block_all_operations_until_repaired() {
    for id in [A, FIRST, SECOND, SPATIAL] {
        missing_component::<NativeRelationshipIdentity>(id);
    }
    for id in [FIRST, SECOND] {
        missing_component::<NativeFoundation>(id);
    }
    missing_component::<DomainId>(FIRST);
    missing_component::<Position>(FIRST);
    missing_component::<NativeActorIdentity>(FIRST);
    missing_component::<NativeRotation>(FIRST);
    missing_component::<NativeScale>(FIRST);
    missing_component::<NativeVariables>(FIRST);
    missing_component::<NativeLocalBounds>(FIRST);
}

fn missing_resource<T: Resource>() {
    let mut app = app();
    let resource = app.world_mut().remove_resource::<T>().unwrap();
    reject_all(app.world_mut());
    app.insert_resource(resource);
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
}

#[test]
fn missing_context_index_or_capture_contract_never_downgrades_state() {
    missing_resource::<NativeWorldContext>();
    missing_resource::<NativeRelationshipsContext>();
    missing_resource::<NativeFoundationDependents>();
    missing_resource::<SnapshotMode>();
    let mut app = app();
    // Removing all public relationship state must still fail against the pin.
    let context = app
        .world_mut()
        .remove_resource::<NativeRelationshipsContext>()
        .unwrap();
    let index = app
        .world_mut()
        .remove_resource::<NativeFoundationDependents>()
        .unwrap();
    let actors: Vec<_> = app
        .world()
        .iter_entities()
        .filter_map(|entity| {
            Some((
                entity.id(),
                entity.get::<NativeRelationshipIdentity>()?.clone(),
                entity.get::<NativeFoundation>().copied(),
            ))
        })
        .collect();
    for (entity, _, _) in &actors {
        app.world_mut()
            .entity_mut(*entity)
            .remove::<(NativeRelationshipIdentity, NativeFoundation)>();
    }
    reject_all(app.world_mut());
    app.insert_resource(context).insert_resource(index);
    for (entity, identity, foundation) in actors {
        let mut actor = app.world_mut().entity_mut(entity);
        actor.insert(identity);
        if let Some(foundation) = foundation {
            actor.insert(foundation);
        }
    }
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
}

#[test]
fn immutable_identity_and_role_changes_are_rejected_without_hidden_repairs() {
    for id in [A, FIRST, SPATIAL] {
        for change in 0..3 {
            let mut app = app();
            let actor = entity(app.world(), id);
            let original = app
                .world()
                .get::<NativeRelationshipIdentity>(actor)
                .unwrap()
                .clone();
            let mut changed = original.clone();
            match change {
                0 => changed.identity.name = "OtherSecret".encode_utf16().collect(),
                1 => changed.identity.guid += 1,
                _ => changed.role = NativeRelationshipRole::Deployable,
            }
            if changed == original {
                changed.role = NativeRelationshipRole::Spatial;
            }
            app.world_mut().entity_mut(actor).insert(changed.clone());
            reject_all(app.world_mut());
            assert!(app.world().get::<NativeRelationshipIdentity>(actor) == Some(&changed));
            app.world_mut().entity_mut(actor).insert(original);
            assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
        }
    }
}

#[test]
fn wrong_role_edges_dangling_targets_and_bad_reverse_indexes_fail_closed() {
    for id in [A, B, SPATIAL] {
        let mut app = app();
        let actor = entity(app.world(), id);
        app.world_mut()
            .entity_mut(actor)
            .insert(NativeFoundation(None));
        reject_all(app.world_mut());
        app.world_mut()
            .entity_mut(actor)
            .remove::<NativeFoundation>();
        assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    }
    for target in [FIRST, SECOND, SPATIAL, DomainId(99)] {
        let mut app = app();
        let actor = entity(app.world(), FIRST);
        app.world_mut()
            .entity_mut(actor)
            .insert(NativeFoundation(Some(target)));
        reject_all(app.world_mut());
        app.world_mut()
            .entity_mut(actor)
            .insert(NativeFoundation(Some(A)));
        assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    }
    let bad_indexes = [
        BTreeMap::from([(A, vec![]), (B, vec![])]),
        BTreeMap::from([(A, vec![FIRST, FIRST]), (B, vec![])]),
        BTreeMap::from([(A, vec![SECOND]), (B, vec![])]),
        BTreeMap::from([(A, vec![FIRST])]),
        BTreeMap::from([(A, vec![FIRST]), (B, vec![]), (SPATIAL, vec![])]),
    ];
    for bad in bad_indexes {
        let mut app = app();
        let original = app.world().resource::<NativeFoundationDependents>().clone();
        app.insert_resource(NativeFoundationDependents(bad));
        reject_all(app.world_mut());
        app.insert_resource(original);
        assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    }
    let mut app = app();
    reassign_native_foundation(app.world_mut(), SECOND, Some(A)).unwrap();
    let correct = app.world().resource::<NativeFoundationDependents>().clone();
    app.world_mut()
        .resource_mut::<NativeFoundationDependents>()
        .0
        .get_mut(&A)
        .unwrap()
        .reverse();
    reject_all(app.world_mut());
    app.insert_resource(correct);
    assert_eq!(
        native_dependents_of(app.world(), A).unwrap(),
        [FIRST, SECOND]
    );
}

#[test]
fn valid_edge_with_stale_reverse_cannot_be_repaired_by_an_edit() {
    let mut app = app();
    let actor = entity(app.world(), FIRST);
    app.world_mut()
        .entity_mut(actor)
        .insert(NativeFoundation(Some(B)));
    reject_all(app.world_mut());
    assert!(reassign_native_foundation(app.world_mut(), FIRST, Some(A)).is_err());
    assert_eq!(
        app.world().get::<NativeFoundation>(actor).unwrap().0,
        Some(B)
    );
    assert_eq!(
        reverse(app.world()).unwrap(),
        BTreeMap::from([(A, vec![FIRST]), (B, vec![])])
    );
    app.world_mut()
        .entity_mut(actor)
        .insert(NativeFoundation(Some(A)));
    reassign_native_foundation(app.world_mut(), FIRST, Some(B)).unwrap();
    assert_eq!(native_foundation_of(app.world(), FIRST).unwrap(), Some(B));
}

#[test]
fn fresh_contexts_with_identical_catalog_and_entity_values_fail_activation_pins() {
    let mut app = app();
    let mut other = build_app(&ServerConfig::default(), &snapshot()).unwrap();
    for id in [A, B, FIRST, SECOND, SPATIAL] {
        assert_eq!(entity(app.world(), id), entity(other.world(), id));
    }
    let original_native = app.world().resource::<NativeWorldContext>().clone();
    let original_relationships = app.world().resource::<NativeRelationshipsContext>().clone();
    let foreign_native = other
        .world_mut()
        .remove_resource::<NativeWorldContext>()
        .unwrap();
    let foreign_relationships = other
        .world_mut()
        .remove_resource::<NativeRelationshipsContext>()
        .unwrap();
    app.insert_resource(foreign_native.clone());
    reject_all(app.world_mut());
    app.insert_resource(original_native.clone());
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    app.insert_resource(foreign_relationships.clone());
    reject_all(app.world_mut());
    app.insert_resource(foreign_native);
    reject_all(app.world_mut());
    app.insert_resource(original_native)
        .insert_resource(original_relationships);
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    reassign_native_foundation(app.world_mut(), FIRST, Some(B)).unwrap();
}

#[derive(Component, Default)]
struct Dormant;

fn hidden_entities<T: Component + Default>() {
    let mut app = app();
    let disabling = app.world_mut().register_component::<T>();
    app.world_mut()
        .resource_mut::<DefaultQueryFilters>()
        .register_disabling_component(disabling);
    // Valid disabled actors retain full membership and state.
    let actor = entity(app.world(), FIRST);
    app.world_mut().entity_mut(actor).insert(T::default());
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    reassign_native_foundation(app.world_mut(), FIRST, Some(B)).unwrap();
    assert_eq!(native_foundation_of(app.world(), FIRST).unwrap(), Some(B));
    reassign_native_foundation(app.world_mut(), FIRST, Some(A)).unwrap();
    app.world_mut()
        .entity_mut(actor)
        .remove::<NativeRelationshipIdentity>();
    reject_all(app.world_mut());
    app.world_mut()
        .entity_mut(actor)
        .insert(NativeRelationshipIdentity {
            identity: link(2),
            role: NativeRelationshipRole::Deployable,
        });
    let identity = app
        .world()
        .get::<NativeRelationshipIdentity>(actor)
        .unwrap()
        .clone();
    let extra = app.world_mut().spawn((T::default(), identity)).id();
    reject_all(app.world_mut());
    app.world_mut().despawn(extra);
    let extra = app
        .world_mut()
        .spawn((T::default(), NativeFoundation(None)))
        .id();
    reject_all(app.world_mut());
    app.world_mut().despawn(extra);
    let extra = app
        .world_mut()
        .spawn((T::default(), DomainId(99), NativeFoundation(None)))
        .id();
    reject_all(app.world_mut());
    app.world_mut().despawn(extra);
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
}

#[test]
fn disabled_and_application_filtered_entities_cannot_hide_relationship_damage() {
    hidden_entities::<Disabled>();
    hidden_entities::<Dormant>();
}

#[test]
fn unregistered_components_without_domain_ids_and_removed_actors_are_detected() {
    let mut app = app();
    let extra = app.world_mut().spawn(NativeFoundation(None)).id();
    reject_all(app.world_mut());
    app.world_mut().despawn(extra);
    let extra = app
        .world_mut()
        .spawn(NativeRelationshipIdentity {
            identity: link(8),
            role: NativeRelationshipRole::Spatial,
        })
        .id();
    reject_all(app.world_mut());
    app.world_mut().despawn(extra);
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot());
    let actor = entity(app.world(), FIRST);
    app.world_mut().despawn(actor);
    reject_all(app.world_mut());
}

#[test]
fn spatial_and_legacy_modes_reject_unexpected_relationship_state() {
    let mut spatial = snapshot();
    spatial.schema_version = 4;
    spatial.native_relationships = None;
    for initial in [spatial, WorldSnapshot::default()] {
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let extra = app.world_mut().spawn(NativeFoundation(None)).id();
        assert!(capture(app.world_mut()).is_err());
        app.world_mut().despawn(extra);
        let extra = app
            .world_mut()
            .spawn(NativeRelationshipIdentity {
                identity: link(8),
                role: NativeRelationshipRole::Spatial,
            })
            .id();
        assert!(capture(app.world_mut()).is_err());
        app.world_mut().despawn(extra);
        app.insert_resource(NativeFoundationDependents(BTreeMap::new()));
        assert!(capture(app.world_mut()).is_err());
        app.world_mut()
            .remove_resource::<NativeFoundationDependents>();
        let donor = build_app(&ServerConfig::default(), &snapshot()).unwrap();
        app.insert_resource(
            donor
                .world()
                .resource::<NativeRelationshipsContext>()
                .clone(),
        );
        assert!(capture(app.world_mut()).is_err());
        app.world_mut()
            .remove_resource::<NativeRelationshipsContext>();
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
    }
}
