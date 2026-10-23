//! Fixed-topology authored spatial objects. Bounds are authored geometry limits,
//! not collision, navigation, gameplay or original-save restoration.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{Component, Entity, Resource, World};

use crate::{
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeActorRecord, NativeTransform, NativeVariableValue,
        NativeWorldSnapshot,
    },
    simulation::{DomainId, Position},
};

#[derive(Component, Clone, PartialEq, Eq)]
pub struct NativeActorIdentity {
    pub placement: String,
    pub class: String,
    pub tile: String,
}

#[derive(Component, Clone, Copy, PartialEq)]
pub struct NativeRotation(pub [f32; 4]);

#[derive(Component, Clone, Copy, PartialEq)]
pub struct NativeScale(pub [f32; 3]);

#[derive(Component, Clone, PartialEq)]
pub struct NativeVariables(pub Vec<NativeVariableValue>);

#[derive(Component, Clone, PartialEq)]
pub struct NativeLocalBounds(pub Aabb3);

/// Immutable loaded definitions and expected runtime membership. Bevy entities
/// remain confined to this resource and never enter persistent records.
#[derive(Resource, Clone)]
pub struct NativeWorldContext {
    catalog: Arc<CompiledNativeCatalog>,
    map: String,
    expected: BTreeMap<DomainId, Entity>,
}

pub(crate) fn activate(world: &mut World, native: &NativeWorldSnapshot) -> Result<()> {
    native.validate()?;
    let catalog = Arc::new(CompiledNativeCatalog::compile(native.definitions.clone())?);
    // Resolve every required definition before the first ECS mutation.
    let records = native
        .actors
        .iter()
        .map(|actor| {
            let bounds = catalog
                .class(&actor.class)
                .context("native actor class definition missing")?
                .local_bounds
                .clone();
            Ok((actor, bounds))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut expected = BTreeMap::new();
    for (actor, bounds) in records {
        let entity = world
            .spawn((
                actor.id,
                Position(actor.transform.translation),
                NativeActorIdentity {
                    placement: actor.placement.clone(),
                    class: actor.class.clone(),
                    tile: actor.tile.clone(),
                },
                NativeRotation(actor.transform.rotation),
                NativeScale(actor.transform.scale),
                NativeVariables(actor.variables.clone()),
                NativeLocalBounds(bounds),
            ))
            .id();
        expected.insert(actor.id, entity);
    }
    world.insert_resource(NativeWorldContext {
        catalog,
        map: native.map.clone(),
        expected,
    });
    Ok(())
}

pub(crate) fn capture_native(world: &mut World) -> Result<NativeWorldSnapshot> {
    let context = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?
        .clone();
    // The registry detects removal of identity or complete actor deletion;
    // optional-component queries alone cannot observe either failure.
    let expected_entities: BTreeSet<_> = context.expected.values().copied().collect();
    // Persistence checks must include entities hidden by Disabled or any
    // application-registered default query filter, without altering those filters.
    for entity in world.iter_entities() {
        if let Some(id) = entity.get::<DomainId>() {
            ensure!(
                context.expected.get(id) == Some(&entity.id()),
                "native world contains an unexpected or duplicate domain entity"
            );
        }
        if entity.contains::<NativeActorIdentity>()
            || entity.contains::<NativeRotation>()
            || entity.contains::<NativeScale>()
            || entity.contains::<NativeVariables>()
            || entity.contains::<NativeLocalBounds>()
        {
            ensure!(
                expected_entities.contains(&entity.id()),
                "native world contains an unregistered object"
            );
        }
    }
    let mut actors = Vec::with_capacity(context.expected.len());
    for (id, entity) in &context.expected {
        ensure!(
            world.get::<DomainId>(*entity) == Some(id),
            "native actor lacks its expected domain identity"
        );
        let identity = world
            .get::<NativeActorIdentity>(*entity)
            .context("native actor lacks required placement identity")?;
        let position = world
            .get::<Position>(*entity)
            .context("native actor lacks required position")?;
        let rotation = world
            .get::<NativeRotation>(*entity)
            .context("native actor lacks required rotation")?;
        let scale = world
            .get::<NativeScale>(*entity)
            .context("native actor lacks required scale")?;
        let variables = world
            .get::<NativeVariables>(*entity)
            .context("native actor lacks required variable state")?;
        let bounds = world
            .get::<NativeLocalBounds>(*entity)
            .context("native actor lacks required local bounds")?;
        let class = context
            .catalog
            .class(&identity.class)
            .context("native actor class is undeclared")?;
        ensure!(
            bounds.0 == class.local_bounds,
            "native actor local bounds differ from its definition"
        );
        actors.push(NativeActorRecord {
            id: *id,
            placement: identity.placement.clone(),
            class: identity.class.clone(),
            tile: identity.tile.clone(),
            transform: NativeTransform {
                translation: position.0,
                rotation: rotation.0,
                scale: scale.0,
            },
            variables: variables.0.clone(),
        });
    }
    let snapshot = NativeWorldSnapshot {
        catalog_id: context.catalog.identity().to_owned(),
        definitions: context.catalog.definition().clone(),
        map: context.map,
        actors,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

/// Replace one object's pose and typed state only after the complete proposed
/// state validates. Invalid edits leave every component unchanged.
pub fn update_native_actor(
    world: &mut World,
    id: DomainId,
    transform: NativeTransform,
    variables: Vec<NativeVariableValue>,
) -> Result<()> {
    let mut proposed = capture_native(world)?;
    let actor = proposed
        .actors
        .iter_mut()
        .find(|actor| actor.id == id)
        .context("native edit targets an unknown domain identity")?;
    actor.transform = transform;
    actor.variables = variables;
    proposed.validate()?;
    let actor = proposed
        .actors
        .iter()
        .find(|actor| actor.id == id)
        .context("native edit lost its candidate")?;
    let entity = *world
        .resource::<NativeWorldContext>()
        .expected
        .get(&id)
        .context("native edit lost its runtime identity")?;
    world.entity_mut(entity).insert((
        Position(actor.transform.translation),
        NativeRotation(actor.transform.rotation),
        NativeScale(actor.transform.scale),
        NativeVariables(actor.variables.clone()),
    ));
    Ok(())
}

/// World-space axis-aligned bounds of the authored geometry after its full pose.
pub fn native_world_bounds(world: &mut World, id: DomainId) -> Result<Aabb3> {
    let snapshot = capture_native(world)?;
    let actor = snapshot
        .actors
        .iter()
        .find(|actor| actor.id == id)
        .context("native bounds target an unknown domain identity")?;
    let class = world
        .resource::<NativeWorldContext>()
        .catalog
        .class(&actor.class)
        .context("native bounds class is undeclared")?;
    actor.transform.transformed_bounds(&class.local_bounds)
}
