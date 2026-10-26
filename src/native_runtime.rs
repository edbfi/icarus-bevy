//! Fixed-topology authored spatial objects. Bounds are authored geometry limits,
//! not collision, navigation, gameplay or original-save restoration.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{Component, Entity, Resource, World};

use crate::{
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeFoundationRecord,
        NativeRelationshipRole, NativeRelationshipsSnapshot,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeActorRecord, NativeActorView, NativeTransform,
        NativeVariableValue, NativeWorldSnapshot,
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

/// Immutable authored identity and capability; class names never imply a role.
#[derive(Component, Clone, PartialEq)]
pub struct NativeRelationshipIdentity {
    pub identity: NativeActorLinkV1,
    pub role: NativeRelationshipRole,
}

/// Authoritative mutable edge, required exactly on declared deployables.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct NativeFoundation(pub Option<DomainId>);

/// Deterministic derived index, including foundations with no dependents.
#[derive(Resource, Clone, PartialEq, Eq)]
pub struct NativeFoundationDependents(pub BTreeMap<DomainId, Vec<DomainId>>);

#[derive(Resource, Clone)]
pub struct NativeRelationshipsContext {
    compiled: Arc<CompiledNativeRelationships>,
    expected: BTreeMap<DomainId, NativeRelationshipIdentity>,
    activation: Arc<()>,
}

impl NativeRelationshipsContext {
    pub(crate) fn same_activation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.activation, &other.activation)
    }
}

/// Immutable loaded definitions and expected runtime membership. Bevy entities
/// remain confined to this resource and never enter persistent records.
#[derive(Resource, Clone)]
pub struct NativeWorldContext {
    catalog: Arc<CompiledNativeCatalog>,
    map: String,
    expected: BTreeMap<DomainId, Entity>,
    /// The values of `expected`, sorted for membership checks.
    entities: Arc<[Entity]>,
    activation: Arc<()>,
}

impl NativeWorldContext {
    pub(crate) fn same_activation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.activation, &other.activation)
    }

    pub(crate) fn replication_identity(&self) -> (&str, &str) {
        (self.catalog.identity(), &self.map)
    }
}

/// Read-only bridge seam. Retain the activation pin, validate the entire native
/// state, and resolve private ECS bindings by domain ID rather than query order.
pub(crate) fn replication_actors<'a>(
    world: &'a World,
    activation: &NativeWorldContext,
) -> Result<Vec<(Entity, NativeActorView<'a>)>> {
    let context = world
        .get_resource::<NativeWorldContext>()
        .context("native replication context missing")?;
    ensure!(
        context.same_activation(activation),
        "native replication activation changed"
    );
    let state = validated_native_state(world, context)?;
    ensure!(
        state.foundations.is_none(),
        "native replication requires a spatial-only world"
    );
    // Validation emits exactly one actor per registry entry, in registry order.
    Ok(context
        .expected
        .values()
        .copied()
        .zip(state.actors)
        .collect())
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
    let mut entities: Vec<_> = expected.values().copied().collect();
    entities.sort_unstable();
    world.insert_resource(NativeWorldContext {
        catalog,
        map: native.map.clone(),
        expected,
        entities: entities.into(),
        activation: Arc::new(()),
    });
    Ok(())
}

/// Startup-only seam: resolve and stage every component and edge before writing.
/// SnapshotMode pins are installed by the caller after activation completes.
pub(crate) fn activate_relationships(
    world: &mut World,
    native: &NativeWorldSnapshot,
    relationships: &NativeRelationshipsSnapshot,
) -> Result<()> {
    ensure_no_relationship_state(world)?;
    let context = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?;
    let actors = validated_native_actors(world, context)?;
    ensure!(
        context.catalog.identity() == native.catalog_id && context.map == native.map,
        "native relationship activation catalog binding mismatch"
    );
    ensure!(
        actors.len() == native.actors.len(),
        "native relationship activation actor coverage mismatch"
    );
    let compiled = Arc::new(CompiledNativeRelationships::from_snapshot(
        relationships,
        native,
    )?);
    let bindings: BTreeMap<_, _> = actors
        .iter()
        .map(|actor| (actor.placement, actor.id))
        .collect();
    let mut expected = BTreeMap::new();
    let mut staged = Vec::new();
    let edges: BTreeMap<_, _> = relationships
        .foundations
        .iter()
        .map(|row| (row.owner, row.foundation))
        .collect();
    for declaration in &compiled.definition().actors {
        let id = *bindings
            .get(declaration.placement.as_str())
            .context("native relationship activation placement missing")?;
        let entity = *context
            .expected
            .get(&id)
            .context("native relationship activation actor missing")?;
        let identity = NativeRelationshipIdentity {
            identity: declaration.identity.clone(),
            role: declaration.role,
        };
        let foundation = if declaration.role == NativeRelationshipRole::Deployable {
            Some(NativeFoundation(*edges.get(&id).context(
                "native relationship activation deployable state missing",
            )?))
        } else {
            None
        };
        ensure!(
            expected.insert(id, identity.clone()).is_none(),
            "native relationship activation duplicate actor"
        );
        staged.push((entity, identity, foundation));
    }
    ensure!(
        expected.len() == context.expected.len(),
        "native relationship activation coverage incomplete"
    );
    let dependents = derive_dependents(&expected, &relationships.foundations)?;
    let context = NativeRelationshipsContext {
        compiled,
        expected,
        activation: Arc::new(()),
    };
    for (entity, identity, foundation) in staged {
        let mut entity = world.entity_mut(entity);
        entity.insert(identity);
        if let Some(foundation) = foundation {
            entity.insert(foundation);
        }
    }
    world.insert_resource(dependents);
    world.insert_resource(context);
    Ok(())
}

/// Legacy and spatial-only modes cannot silently omit relationship state.
pub(crate) fn ensure_no_relationship_state(world: &World) -> Result<()> {
    ensure!(
        !world.contains_resource::<NativeRelationshipsContext>()
            && !world.contains_resource::<NativeFoundationDependents>(),
        "world contains unexpected native relationship resources"
    );
    // Resolve from this world on every call: a type absent at an earlier
    // validation may have been registered and inserted since then.
    let forbidden = [
        world.component_id::<NativeRelationshipIdentity>(),
        world.component_id::<NativeFoundation>(),
        world.component_id::<NativeRelationshipsContext>(),
        world.component_id::<NativeFoundationDependents>(),
    ];
    for archetype in world.archetypes().iter().filter(|a| !a.is_empty()) {
        ensure!(
            !forbidden.iter().flatten().any(|id| archetype.contains(*id)),
            "world contains unexpected native relationship components"
        );
    }
    Ok(())
}

fn derive_dependents(
    expected: &BTreeMap<DomainId, NativeRelationshipIdentity>,
    foundations: &[NativeFoundationRecord],
) -> Result<NativeFoundationDependents> {
    let mut reverse: BTreeMap<_, Vec<_>> = expected
        .iter()
        .filter(|(_, identity)| identity.role == NativeRelationshipRole::Foundation)
        .map(|(id, _)| (*id, Vec::new()))
        .collect();
    for row in foundations {
        if let Some(target) = row.foundation {
            reverse
                .get_mut(&target)
                .context("native relationship target is not a foundation")?
                .push(row.owner);
        }
    }
    for owners in reverse.values_mut() {
        owners.sort_unstable();
    }
    Ok(NativeFoundationDependents(reverse))
}

fn validated_relationships(
    world: &World,
    native: &NativeWorldContext,
) -> Result<Option<Vec<NativeFoundationRecord>>> {
    let Some(context) = world.get_resource::<NativeRelationshipsContext>() else {
        ensure_no_relationship_state(world)?;
        return Ok(None);
    };
    ensure!(
        context.expected.len() == native.expected.len(),
        "native relationship actor coverage incomplete"
    );
    for entity in world.iter_entities() {
        if entity.contains::<NativeRelationshipIdentity>() || entity.contains::<NativeFoundation>()
        {
            let id = entity
                .get::<DomainId>()
                .context("native relationship component lacks domain identity")?;
            ensure!(
                native.expected.get(id) == Some(&entity.id()) && context.expected.contains_key(id),
                "native relationship component belongs to an unregistered actor"
            );
        }
    }
    let mut foundations = Vec::new();
    for (id, expected) in &context.expected {
        let entity = *native
            .expected
            .get(id)
            .context("native relationship actor binding missing")?;
        let identity = world
            .get::<NativeRelationshipIdentity>(entity)
            .context("native actor lacks required relationship identity")?;
        ensure!(
            identity == expected,
            "native relationship identity or role differs from its declaration"
        );
        match (expected.role, world.get::<NativeFoundation>(entity)) {
            (NativeRelationshipRole::Deployable, Some(foundation)) => {
                foundations.push(NativeFoundationRecord {
                    owner: *id,
                    foundation: foundation.0,
                });
            }
            (NativeRelationshipRole::Deployable, None) => {
                anyhow::bail!("native deployable lacks required foundation state");
            }
            (_, Some(_)) => anyhow::bail!("native foundation state belongs to an invalid role"),
            (_, None) => {}
        }
    }
    context.compiled.validate_foundations(&foundations)?;
    let reverse = world
        .get_resource::<NativeFoundationDependents>()
        .context("native relationships lack required reverse index")?;
    ensure!(
        *reverse == derive_dependents(&context.expected, &foundations)?,
        "native relationship reverse index differs from authoritative edges"
    );
    Ok(Some(foundations))
}

struct ValidatedNativeState<'a> {
    actors: Vec<NativeActorView<'a>>,
    foundations: Option<Vec<NativeFoundationRecord>>,
}

fn validated_native_state<'a>(
    world: &'a World,
    context: &NativeWorldContext,
) -> Result<ValidatedNativeState<'a>> {
    crate::simulation::validate_native_activation(world, context)?;
    let actors = validated_native_actors(world, context)?;
    let foundations = validated_relationships(world, context)?;
    Ok(ValidatedNativeState {
        actors,
        foundations,
    })
}

/// Check ECS membership and every required component, then validate all mutable
/// records using the catalog verified at activation. No variable state is cloned.
fn validated_native_actors<'a>(
    world: &'a World,
    context: &NativeWorldContext,
) -> Result<Vec<NativeActorView<'a>>> {
    // The registry detects removal of identity or complete actor deletion;
    // optional-component queries alone cannot observe either failure.
    // Persistence checks must include entities hidden by Disabled or any
    // application-registered default query filter, without altering those filters.
    // Archetypes are visited in `iter_entities` order and skipped only when no
    // entity in them can fail, so the first error is unchanged. Component IDs
    // are resolved on every call; an unregistered type cannot be present.
    let domain = world.component_id::<DomainId>();
    let native = [
        world.component_id::<NativeActorIdentity>(),
        world.component_id::<NativeRotation>(),
        world.component_id::<NativeScale>(),
        world.component_id::<NativeVariables>(),
        world.component_id::<NativeLocalBounds>(),
    ];
    for archetype in world.archetypes().iter() {
        let has_domain = domain.is_some_and(|id| archetype.contains(id));
        let has_native = native.iter().flatten().any(|id| archetype.contains(*id));
        if !has_domain && !has_native {
            continue;
        }
        for entity in archetype.entities() {
            let entity = entity.id();
            let id = if has_domain {
                world.entity(entity).get::<DomainId>()
            } else {
                None
            };
            if let Some(id) = id {
                ensure!(
                    context.expected.get(id) == Some(&entity),
                    "native world contains an unexpected or duplicate domain entity"
                );
            }
            // A domain entity that passed above is its registry entry.
            ensure!(
                !has_native || id.is_some() || context.entities.binary_search(&entity).is_ok(),
                "native world contains an unregistered object"
            );
        }
    }
    let mut actors = Vec::with_capacity(context.expected.len());
    for (id, entity) in &context.expected {
        let entity = world
            .get_entity(*entity)
            .ok()
            .filter(|entity| entity.get::<DomainId>() == Some(id))
            .context("native actor lacks its expected domain identity")?;
        let identity = entity
            .get::<NativeActorIdentity>()
            .context("native actor lacks required placement identity")?;
        let position = entity
            .get::<Position>()
            .context("native actor lacks required position")?;
        let rotation = entity
            .get::<NativeRotation>()
            .context("native actor lacks required rotation")?;
        let scale = entity
            .get::<NativeScale>()
            .context("native actor lacks required scale")?;
        let variables = entity
            .get::<NativeVariables>()
            .context("native actor lacks required variable state")?;
        let bounds = entity
            .get::<NativeLocalBounds>()
            .context("native actor lacks required local bounds")?;
        let class = context
            .catalog
            .class(&identity.class)
            .context("native actor class is undeclared")?;
        ensure!(
            bounds.0 == class.local_bounds,
            "native actor local bounds differ from its definition"
        );
        actors.push(NativeActorView {
            id: *id,
            placement: &identity.placement,
            class: &identity.class,
            tile: &identity.tile,
            transform: NativeTransform {
                translation: position.0,
                rotation: rotation.0,
                scale: scale.0,
            },
            variables: &variables.0,
        });
    }
    context
        .catalog
        .validate_records(&context.map, actors.iter().cloned())?;
    Ok(actors)
}

pub(crate) fn capture_native(
    world: &World,
) -> Result<(NativeWorldSnapshot, Option<NativeRelationshipsSnapshot>)> {
    let context = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?;
    let state = validated_native_state(world, context)?;
    let relationships = state.foundations.map(|foundations| {
        // Complete state validation above established this context's presence.
        let context = world.resource::<NativeRelationshipsContext>();
        NativeRelationshipsSnapshot {
            relationship_id: context.compiled.identity().to_owned(),
            definition: context.compiled.definition().clone(),
            foundations,
        }
    });
    // Only snapshot output needs owned definitions and actor payloads. The
    // complete record validation above does not recompile or hash the catalog.
    let native = NativeWorldSnapshot {
        catalog_id: context.catalog.identity().to_owned(),
        definitions: context.catalog.definition().clone(),
        map: context.map.clone(),
        actors: state
            .actors
            .into_iter()
            .map(|actor| NativeActorRecord {
                id: actor.id,
                placement: actor.placement.to_owned(),
                class: actor.class.to_owned(),
                tile: actor.tile.to_owned(),
                transform: actor.transform,
                variables: actor.variables.to_vec(),
            })
            .collect(),
    };
    Ok((native, relationships))
}

/// Replace one object's pose and typed state only after the complete proposed
/// state validates. Invalid edits leave every component unchanged.
pub fn update_native_actor(
    world: &mut World,
    id: DomainId,
    transform: NativeTransform,
    variables: Vec<NativeVariableValue>,
) -> Result<()> {
    let entity = {
        let context = world
            .get_resource::<NativeWorldContext>()
            .context("native world lacks required catalog context")?;
        // Validate current state first, so an edit cannot silently repair or
        // conceal existing corruption, even on the targeted actor.
        let state = validated_native_state(world, context)?;
        let entity = *context
            .expected
            .get(&id)
            .context("native edit targets an unknown domain identity")?;
        context.catalog.validate_records(
            &context.map,
            state.actors.into_iter().map(|actor| {
                if actor.id == id {
                    NativeActorView {
                        transform: transform.clone(),
                        variables: &variables,
                        ..actor
                    }
                } else {
                    actor
                }
            }),
        )?;
        entity
    };
    world.entity_mut(entity).insert((
        Position(transform.translation),
        NativeRotation(transform.rotation),
        NativeScale(transform.scale),
        NativeVariables(variables),
    ));
    Ok(())
}

/// World-space axis-aligned bounds of the authored geometry after its full pose.
pub fn native_world_bounds(world: &mut World, id: DomainId) -> Result<Aabb3> {
    let context = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?;
    let state = validated_native_state(world, context)?;
    let actor = state
        .actors
        .into_iter()
        .find(|actor| actor.id == id)
        .context("native bounds target an unknown domain identity")?;
    let class = context
        .catalog
        .class(actor.class)
        .context("native bounds class is undeclared")?;
    actor.transform.transformed_bounds(&class.local_bounds)
}

fn require_relationship_role(
    context: &NativeRelationshipsContext,
    id: DomainId,
    role: NativeRelationshipRole,
) -> Result<()> {
    ensure!(
        context.expected.get(&id).map(|identity| identity.role) == Some(role),
        "native relationship domain identity missing or wrong role"
    );
    Ok(())
}

/// Atomically replace one authoritative edge and its deterministic reverse index.
/// Existing damage is rejected even when the proposed edit would repair it.
pub fn reassign_native_foundation(
    world: &mut World,
    owner: DomainId,
    target: Option<DomainId>,
) -> Result<()> {
    let (entity, dependents) = {
        let native = world
            .get_resource::<NativeWorldContext>()
            .context("native world lacks required catalog context")?;
        let state = validated_native_state(world, native)?;
        let context = world
            .get_resource::<NativeRelationshipsContext>()
            .context("native world lacks required relationship context")?;
        require_relationship_role(context, owner, NativeRelationshipRole::Deployable)?;
        if let Some(target) = target {
            require_relationship_role(context, target, NativeRelationshipRole::Foundation)?;
        }
        let mut foundations = state
            .foundations
            .context("native world lacks required foundation state")?;
        let row = foundations
            .iter_mut()
            .find(|row| row.owner == owner)
            .context("native relationship owner state missing")?;
        if row.foundation == target {
            return Ok(());
        }
        row.foundation = target;
        let dependents = derive_dependents(&context.expected, &foundations)?;
        let entity = *native
            .expected
            .get(&owner)
            .context("native relationship owner binding missing")?;
        (entity, dependents)
    };
    // All fallible work precedes exclusive writes. No system can interleave
    // between the authoritative component update and its derived index update.
    world.entity_mut(entity).insert(NativeFoundation(target));
    world.insert_resource(dependents);
    Ok(())
}

pub fn native_foundation_of(world: &World, owner: DomainId) -> Result<Option<DomainId>> {
    let native = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?;
    let state = validated_native_state(world, native)?;
    let context = world
        .get_resource::<NativeRelationshipsContext>()
        .context("native world lacks required relationship context")?;
    require_relationship_role(context, owner, NativeRelationshipRole::Deployable)?;
    Ok(state
        .foundations
        .context("native world lacks required foundation state")?
        .into_iter()
        .find(|row| row.owner == owner)
        .context("native relationship owner state missing")?
        .foundation)
}

pub fn native_dependents_of(world: &World, foundation: DomainId) -> Result<Vec<DomainId>> {
    let native = world
        .get_resource::<NativeWorldContext>()
        .context("native world lacks required catalog context")?;
    validated_native_state(world, native)?;
    let context = world
        .get_resource::<NativeRelationshipsContext>()
        .context("native world lacks required relationship context")?;
    require_relationship_role(context, foundation, NativeRelationshipRole::Foundation)?;
    Ok(world
        .get_resource::<NativeFoundationDependents>()
        .context("native relationships lack required reverse index")?
        .0
        .get(&foundation)
        .context("native foundation reverse index entry missing")?
        .clone())
}

#[cfg(test)]
mod relationship_absence_tests {
    use super::*;
    use bevy::ecs::{
        entity_disabling::{DefaultQueryFilters, Disabled},
        resource::IsResource,
    };

    use crate::{
        config::ServerConfig,
        native_relationships::{NativeRelationshipActorV1, NativeRelationshipsDefinition},
        native_world::prepare_authored_world,
        simulation::{WorldSnapshot, build_app, capture},
    };

    #[derive(Component)]
    struct Dormant;

    #[derive(Resource)]
    struct UnrelatedResource;

    fn context() -> NativeRelationshipsContext {
        let catalog =
            CompiledNativeCatalog::parse(include_str!("../config/native-world.example.toml"))
                .unwrap();
        let definition = NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "workshop".into(),
            actors: ["marker_a", "marker_b"]
                .into_iter()
                .enumerate()
                .map(|(index, placement)| NativeRelationshipActorV1 {
                    placement: placement.into(),
                    identity: NativeActorLinkV1 {
                        name: vec![65 + index as u16],
                        guid: 1 + index as i32,
                    },
                    role: NativeRelationshipRole::Spatial,
                    foundation: None,
                })
                .collect(),
        };
        NativeRelationshipsContext {
            compiled: Arc::new(CompiledNativeRelationships::compile(definition, &catalog).unwrap()),
            expected: BTreeMap::new(),
            activation: Arc::new(()),
        }
    }

    fn rejects(world: &World, expected: &str) {
        assert_eq!(
            ensure_no_relationship_state(world).unwrap_err().to_string(),
            expected
        );
    }

    fn check_component<T: Component>(make: impl Fn() -> T, expected: &str) {
        let mut world = World::new();
        assert!(world.component_id::<T>().is_none());
        ensure_no_relationship_state(&world).unwrap();
        let component = world.register_component::<T>();
        ensure_no_relationship_state(&world).unwrap();
        let offender = world.spawn(make()).id();
        let archetype = world.entity(offender).archetype().id();
        rejects(&world, expected);
        // Resource-derived fixtures also own IsResource. Removing both while
        // the entity is alive flushes its discard hook and clears the singleton
        // registration before despawn or reuse of the resource type.
        world.entity_mut(offender).remove::<(T, IsResource)>();
        assert!(world.archetypes()[archetype].is_empty());
        assert!(world.archetypes()[archetype].contains(component));
        ensure_no_relationship_state(&world).unwrap();
        world.despawn(offender);
        let offender = world.spawn(make()).id();
        assert_eq!(world.entity(offender).archetype().id(), archetype);
        rejects(&world, expected);
        world.entity_mut(offender).remove::<(T, IsResource)>();
        world.despawn(offender);
        ensure_no_relationship_state(&world).unwrap();

        let offender = world.spawn((Disabled, make())).id();
        rejects(&world, expected);
        world.entity_mut(offender).remove::<(T, IsResource)>();
        world.despawn(offender);
        ensure_no_relationship_state(&world).unwrap();
        let disabling = world.register_component::<Dormant>();
        world
            .resource_mut::<DefaultQueryFilters>()
            .register_disabling_component(disabling);
        let offender = world.spawn((Dormant, make())).id();
        rejects(&world, expected);
        world.entity_mut(offender).remove::<(T, IsResource)>();
        world.despawn(offender);
        ensure_no_relationship_state(&world).unwrap();

        // The existing singleton marker belongs to UnrelatedResource, so even
        // Resource-derived forbidden types must be found as components here.
        world.insert_resource(UnrelatedResource);
        let offender = world.resource_entity::<UnrelatedResource>().unwrap();
        world.entity_mut(offender).insert(make());
        rejects(
            &world,
            "world contains unexpected native relationship components",
        );
        world.entity_mut(offender).remove::<T>();
        assert!(world.contains_resource::<UnrelatedResource>());
        ensure_no_relationship_state(&world).unwrap();
    }

    #[test]
    fn all_four_types_reject_live_hidden_and_resource_entities_after_clean_checks() {
        check_component(
            || NativeRelationshipIdentity {
                identity: NativeActorLinkV1 {
                    name: vec![65],
                    guid: 1,
                },
                role: NativeRelationshipRole::Spatial,
            },
            "world contains unexpected native relationship components",
        );
        check_component(
            || NativeFoundation(None),
            "world contains unexpected native relationship components",
        );
        let context = context();
        check_component(
            || context.clone(),
            "world contains unexpected native relationship resources",
        );
        check_component(
            || NativeFoundationDependents(BTreeMap::new()),
            "world contains unexpected native relationship resources",
        );
    }

    fn check_resource_precedence<T: Resource>(make: impl Fn() -> T) {
        let mut world = World::new();
        ensure_no_relationship_state(&world).unwrap();
        let offender = world.spawn((Disabled, NativeFoundation(None))).id();
        world.insert_resource(make());
        rejects(
            &world,
            "world contains unexpected native relationship resources",
        );
        world.remove_resource::<T>().unwrap();
        rejects(
            &world,
            "world contains unexpected native relationship components",
        );
        world.despawn(offender);
        ensure_no_relationship_state(&world).unwrap();
    }

    #[test]
    fn each_resource_error_precedes_component_errors_and_repair_allows_retry() {
        let context = context();
        check_resource_precedence(|| context.clone());
        check_resource_precedence(|| NativeFoundationDependents(BTreeMap::new()));
    }

    #[test]
    fn late_relationship_component_blocks_legacy_capture_until_repaired() {
        let initial = WorldSnapshot::default();
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
        assert!(app.world().component_id::<NativeFoundation>().is_none());
        let offender = app
            .world_mut()
            .spawn((Disabled, NativeFoundation(None)))
            .id();
        assert_eq!(
            capture(app.world_mut())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "world contains unexpected native relationship components"
        );
        app.world_mut().despawn(offender);
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
    }

    #[test]
    fn relationship_activation_rejects_stray_components_before_writes_then_retries() {
        let catalog = Arc::new(
            CompiledNativeCatalog::parse(include_str!("../config/native-world.example.toml"))
                .unwrap(),
        );
        let initial = prepare_authored_world(catalog, "workshop")
            .unwrap()
            .initial_snapshot();
        let relationships = context().compiled.initial_snapshot();
        let native = initial.native_world.as_ref().unwrap();
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
        let offender = app
            .world_mut()
            .spawn((Disabled, NativeFoundation(None)))
            .id();
        assert_eq!(
            activate_relationships(app.world_mut(), native, &relationships)
                .unwrap_err()
                .to_string(),
            "world contains unexpected native relationship components"
        );
        assert!(
            !app.world()
                .contains_resource::<NativeRelationshipsContext>()
        );
        assert!(
            !app.world()
                .contains_resource::<NativeFoundationDependents>()
        );
        assert!(
            app.world()
                .iter_entities()
                .all(|entity| !entity.contains::<NativeRelationshipIdentity>())
        );
        app.world_mut().despawn(offender);
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
        activate_relationships(app.world_mut(), native, &relationships).unwrap();
        assert_eq!(
            app.world()
                .resource::<NativeRelationshipsContext>()
                .expected
                .len(),
            native.actors.len()
        );
    }
}
