//! EXPERIMENTAL bridge from the native replication stage to the session layer.
//!
//! Each fixed tick, before [`ReplicationSet`], every joined lab connection
//! gets one [`NetViewer`] at the session's spawn location (the PlayerController
//! has no pawn yet); after it, every actor the stage selected for a connection
//! that carries a [`NetArchetype`] is opened on that connection's lowest free
//! actor channel through [`PeerLab::open_actor`], once per connection, with
//! its position as the spawn location and one actor block carrying the roles
//! the capture's world-actor opens carry (F-791), plus its authored or resting
//! `ReplicatedMovement` when it replicates movement; such an actor, selected
//! again after a quantized change, gets an unreliable property update once its
//! open is acked ([`PeerLab::update_actor_properties`]). Optional authored
//! [`NetActorFlags`] adds explicit handles 1 and 4 to opens and updates; removal
//! requests the lab reset `(false, false)` for previously managed actors.
//! A channel the stage closes (no longer relevant `3`, torn off `4`; F-749)
//! gets its close bunch first ([`PeerLab::close_actor`]). Actor GUIDs are
//! driver-wide and stable per [`DomainId`], or per [`NetIncarnation`] when opted
//! in. Incarnation replacement waits for each peer's reliable close ACK. Other
//! properties are not replicated.
//! Archetypes are caller data; nothing here reads original files or demonstrates
//! ICARUS compatibility.
use bevy::prelude::*;

use crate::{
    experimental::{
        actor_channel::ContentBlock,
        peer_lab::PeerLab,
        session_open::{
            ActorOpenSpec, PathSegment, actor_role_block, authored_movement,
            world_actor_state_block,
        },
    },
    replication::{NetActor, NetActorFlags, NetViewer, ReplicationFrame, ReplicationSet},
    simulation::{DomainId, Position, SimulationSet},
};

/// The archetype chain (`Default__<Class>_C`, then its package) a replicated
/// actor is opened with.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct NetArchetype {
    pub archetype: Vec<PathSegment>,
    /// `bReplicateMovement`: the open carries `ReplicatedMovement`, and
    /// changes to its quantized fields are sent as updates.
    pub replicate_movement: bool,
}

/// Optional incarnation identity for reference-free authored spatial actors.
/// The native bridge owns this marker and rejects missing or altered values.
/// Replacement waits for each peer's old reliable close ACK independently.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetIncarnation(pub u64);

/// Optional authored motion for this bridge only. Position remains the spatial
/// input to replication scheduling. Absence means zero rotation and velocity;
/// this component neither advances physics nor interprets saved poses.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct NetMotion {
    /// Pitch, yaw and roll in degrees.
    pub rotation_degrees: [f32; 3],
    pub linear_velocity: [f32; 3],
}

/// Session parameters the bridge opens actors with.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct SessionReplication {
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    pub engine_net_version: u32,
    /// Actor channels are allocated from here up (the lowest free index).
    pub first_channel: u32,
    /// Where each connection's viewer stands.
    pub viewer_location: [f32; 3],
    /// Every opened actor's initial block ([`actor_role_block`], F-791).
    actor_block: ContentBlock,
    /// Open attempts skipped because authored motion could not be quantized or
    /// an authored movement/flag block could not be composed. The actor stays
    /// unopened and is retried, and counted, each time it is selected. Update
    /// failures count in the lab's `actor_update_errors`.
    pub movement_errors: u64,
}

impl SessionReplication {
    /// Session parameters with the role block encoded once up front.
    pub fn new(
        level: Vec<PathSegment>,
        engine_net_version: u32,
        first_channel: u32,
        viewer_location: [f32; 3],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            level,
            engine_net_version,
            first_channel,
            viewer_location,
            actor_block: actor_role_block(engine_net_version)?,
            movement_errors: 0,
        })
    }
}

/// Marks the viewer entity of one lab connection.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
struct SessionViewer(u32);

/// Explicit bridge order, including deferred viewer spawns/removals.
#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)]
pub enum SessionReplicationSet {
    Viewers,
    Send,
}

/// Remove only viewers owned by this bridge, including disabled entities.
pub(crate) fn clear_viewers(world: &mut World) {
    let entities: Vec<_> = world
        .iter_entities()
        .filter(|entity| entity.contains::<SessionViewer>())
        .map(|entity| entity.id())
        .collect();
    for entity in entities {
        world.despawn(entity);
    }
}

/// Add after [`crate::replication::ReplicationPlugin`] with a [`PeerLab`] and
/// a [`SessionReplication`] resource. Sending skips ticks without them; viewer
/// synchronization still runs so removing either resource clears owned viewers.
pub struct SessionReplicationPlugin;

impl Plugin for SessionReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedUpdate,
            (
                SessionReplicationSet::Viewers
                    .after(SimulationSet::Gameplay)
                    .before(ReplicationSet),
                SessionReplicationSet::Send
                    .after(ReplicationSet)
                    .before(SimulationSet::Finish),
            ),
        )
        .add_systems(
            FixedUpdate,
            (
                sync_viewers.in_set(SessionReplicationSet::Viewers),
                open_selected_actors
                    .in_set(SessionReplicationSet::Send)
                    .run_if(resource_exists::<PeerLab>)
                    .run_if(resource_exists::<SessionReplication>),
            ),
        );
    }
}

fn sync_viewers(
    mut commands: Commands,
    lab: Option<Res<PeerLab>>,
    session: Option<Res<SessionReplication>>,
    viewers: Query<(Entity, &SessionViewer)>,
) {
    let (Some(lab), Some(session)) = (lab, session) else {
        for (entity, _) in &viewers {
            commands.entity(entity).despawn();
        }
        return;
    };
    let joined = lab.joined_connections();
    let mut present = Vec::new();
    for (entity, viewer) in &viewers {
        if joined.binary_search(&viewer.0).is_ok() {
            present.push(viewer.0);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for connection in joined {
        if !present.contains(&connection) {
            commands.spawn((
                SessionViewer(connection),
                NetViewer {
                    connection,
                    location: session.viewer_location,
                    direction: [1.0, 0.0, 0.0],
                },
            ));
        }
    }
}

type AuthoredActors<'world, 'state> = Query<
    'world,
    'state,
    (
        &'static NetActor,
        &'static Position,
        &'static NetArchetype,
        Option<&'static NetMotion>,
        Option<&'static NetActorFlags>,
        Option<&'static NetIncarnation>,
    ),
>;

fn open_selected_actors(
    mut lab: ResMut<PeerLab>,
    mut session: ResMut<SessionReplication>,
    frame: Res<ReplicationFrame>,
    actors: AuthoredActors,
    incarnations: Query<(&NetActor, &NetIncarnation)>,
) {
    // Reconcile ALL desired versions, including unscheduled actors and zero
    // peers. Replacement retirement cannot depend on relevance or tick budget.
    let mut desired = std::collections::BTreeMap::new();
    for (actor, incarnation) in &incarnations {
        if desired.insert(actor.id.0, incarnation.0).is_some() {
            lab.stop_admission();
            let _ = lab.reconcile_actor_incarnations(&std::collections::BTreeMap::new());
            return;
        }
    }
    if lab.reconcile_actor_incarnations(&desired).is_err() {
        lab.stop_admission();
        let _ = lab.reconcile_actor_incarnations(&std::collections::BTreeMap::new());
        return;
    }
    lab.retry_pending_closes();
    if frame
        .decisions
        .connections
        .iter()
        .all(|c| c.selected.is_empty() && c.closed.is_empty())
    {
        return;
    }
    // Closes first, so an actor closed and selected again reopens.
    for connection in &frame.decisions.connections {
        for closed in &connection.closed {
            lab.close_actor(
                connection.connection,
                closed.id.0,
                u32::from(closed.close_reason),
            );
        }
    }
    let by_id: std::collections::BTreeMap<DomainId, _> = actors
        .iter()
        .map(|(actor, position, archetype, motion, flags, incarnation)| {
            (actor.id, (position, archetype, motion, flags, incarnation))
        })
        .collect();
    for connection in &frame.decisions.connections {
        for selected in &connection.selected {
            let Some((position, archetype, motion, flags, incarnation)) = by_id.get(&selected.id)
            else {
                continue;
            };
            let is_open = match incarnation {
                Some(incarnation) => lab.is_actor_incarnation_open(
                    connection.connection,
                    selected.id.0,
                    incarnation.0,
                ),
                None => lab.is_actor_open(connection.connection, selected.id.0),
            };
            if is_open {
                let movement = archetype.replicate_movement.then(|| {
                    let motion = motion.copied().unwrap_or_default();
                    [position.0, motion.rotation_degrees, motion.linear_velocity]
                });
                lab.update_actor_properties(
                    connection.connection,
                    selected.id.0,
                    movement,
                    flags.copied(),
                    session.engine_net_version,
                );
                continue;
            }
            let block = if archetype.replicate_movement {
                let motion = motion.copied().unwrap_or_default();
                match authored_movement(position.0, motion.rotation_degrees, motion.linear_velocity)
                    .and_then(|movement| {
                        world_actor_state_block(
                            session.engine_net_version,
                            Some(movement),
                            flags.copied(),
                        )
                    }) {
                    Ok(block) => block,
                    Err(_) => {
                        session.movement_errors = session.movement_errors.saturating_add(1);
                        continue;
                    }
                }
            } else if flags.is_some() {
                // Typed Boolean encoding uses the existing supported prefix.
                match world_actor_state_block(session.engine_net_version, None, flags.copied()) {
                    Ok(block) => block,
                    Err(_) => {
                        session.movement_errors = session.movement_errors.saturating_add(1);
                        continue;
                    }
                }
            } else {
                session.actor_block.clone()
            };
            let spec = ActorOpenSpec {
                // Placeholder: `open_actor` assigns the driver-wide GUID.
                actor: 0,
                archetype: archetype.archetype.clone(),
                level: session.level.clone(),
                location: Some(position.0),
                rotation: motion.map(|motion| motion.rotation_degrees),
                velocity: motion.map(|motion| motion.linear_velocity),
                blocks: vec![block],
            };
            if let Some(incarnation) = incarnation {
                lab.open_actor_incarnation(
                    connection.connection,
                    selected.id.0,
                    incarnation.0,
                    spec,
                    session.first_channel,
                    session.engine_net_version,
                );
            } else {
                lab.open_actor(
                    connection.connection,
                    selected.id.0,
                    spec,
                    session.first_channel,
                    session.engine_net_version,
                );
            }
        }
    }
}
