//! EXPERIMENTAL bridge from the native replication stage to the session layer.
//!
//! Each fixed tick, before [`ReplicationSet`], every joined lab connection
//! gets one [`NetViewer`] at the session's spawn location (the PlayerController
//! has no pawn yet); after it, every actor the stage selected for a connection
//! that carries a [`NetArchetype`] is opened on that connection's lowest free
//! actor channel through [`PeerLab::open_actor`], once per connection, with
//! its position as the spawn location and one actor block carrying the roles
//! the capture's world-actor opens carry (F-791), plus its resting
//! `ReplicatedMovement` when it replicates movement; such an actor, selected
//! again after moving, gets an unreliable movement update once its open is
//! acked ([`PeerLab::update_actor_movement`]); a channel the stage closes
//! (no longer relevant `3`, torn off `4`; F-749) gets its close bunch first
//! ([`PeerLab::close_actor`]). Actor GUIDs are driver-wide and stable per
//! [`DomainId`]. Other properties are not replicated. Archetypes are
//! caller data; nothing here reads original files or demonstrates ICARUS
//! compatibility.
use bevy::prelude::*;

use crate::{
    experimental::{
        actor_channel::ContentBlock,
        peer_lab::PeerLab,
        session_open::{
            ActorOpenSpec, PathSegment, actor_role_block, resting_movement, world_actor_block,
        },
    },
    replication::{NetActor, NetViewer, ReplicationFrame, ReplicationSet},
    simulation::{DomainId, Position},
};

/// The archetype chain (`Default__<Class>_C`, then its package) a replicated
/// actor is opened with.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct NetArchetype {
    pub archetype: Vec<PathSegment>,
    /// `bReplicateMovement`: the open carries the actor's resting
    /// `ReplicatedMovement`, and a changed position is sent as an update.
    pub replicate_movement: bool,
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
    /// Open attempts skipped because a position could not be quantized for
    /// `ReplicatedMovement` (outside the packed range). It counts attempts:
    /// the actor stays unopened and is retried, and counted, each time it is
    /// selected. Update failures count in the lab's `actor_update_errors`.
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

/// Add after [`crate::replication::ReplicationPlugin`] with a [`PeerLab`] and
/// a [`SessionReplication`] resource; both systems skip ticks without them.
pub struct SessionReplicationPlugin;

impl Plugin for SessionReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                sync_viewers.before(ReplicationSet),
                open_selected_actors.after(ReplicationSet),
            )
                .run_if(resource_exists::<PeerLab>)
                .run_if(resource_exists::<SessionReplication>),
        );
    }
}

fn sync_viewers(
    mut commands: Commands,
    lab: Res<PeerLab>,
    session: Res<SessionReplication>,
    viewers: Query<(Entity, &SessionViewer)>,
) {
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

fn open_selected_actors(
    mut lab: ResMut<PeerLab>,
    mut session: ResMut<SessionReplication>,
    frame: Res<ReplicationFrame>,
    actors: Query<(&NetActor, &Position, &NetArchetype)>,
) {
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
    let by_id: std::collections::BTreeMap<DomainId, (&Position, &NetArchetype)> = actors
        .iter()
        .map(|(actor, position, archetype)| (actor.id, (position, archetype)))
        .collect();
    for connection in &frame.decisions.connections {
        for selected in &connection.selected {
            let Some((position, archetype)) = by_id.get(&selected.id) else {
                continue;
            };
            if lab.is_actor_open(connection.connection, selected.id.0) {
                // A selected open actor replicates its changed movement.
                if archetype.replicate_movement {
                    lab.update_actor_movement(
                        connection.connection,
                        selected.id.0,
                        position.0,
                        session.engine_net_version,
                    );
                }
                continue;
            }
            let block = if archetype.replicate_movement {
                match resting_movement(position.0).and_then(|movement| {
                    world_actor_block(session.engine_net_version, Some(movement))
                }) {
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
                blocks: vec![block],
            };
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
