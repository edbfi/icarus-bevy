//! EXPERIMENTAL bridge from the native replication stage to the session layer.
//!
//! Each fixed tick, before [`ReplicationSet`], every joined lab connection
//! gets one [`NetViewer`] at the session's spawn location (the PlayerController
//! has no pawn yet); after it, every actor the stage selected for a connection
//! that carries a [`NetArchetype`] is opened on that connection's lowest free
//! actor channel through [`PeerLab::open_actor`], once per connection, with
//! its position as the spawn location and no content blocks yet. Actor GUIDs
//! are driver-wide and stable per [`DomainId`]. Closing channels and property
//! replication are not modelled. Archetypes are caller data; nothing here
//! reads original files or demonstrates ICARUS compatibility.
use bevy::prelude::*;

use crate::{
    experimental::{
        peer_lab::PeerLab,
        session_open::{ActorOpenSpec, PathSegment},
    },
    replication::{NetActor, NetViewer, ReplicationFrame, ReplicationSet},
    simulation::{DomainId, Position},
};

/// The archetype chain (`Default__<Class>_C`, then its package) a replicated
/// actor is opened with.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct NetArchetype {
    pub archetype: Vec<PathSegment>,
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
    session: Res<SessionReplication>,
    frame: Res<ReplicationFrame>,
    actors: Query<(&NetActor, &Position, &NetArchetype)>,
) {
    if frame
        .decisions
        .connections
        .iter()
        .all(|c| c.selected.is_empty())
    {
        return;
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
            let spec = ActorOpenSpec {
                actor: 0,
                archetype: archetype.archetype.clone(),
                level: session.level.clone(),
                location: Some(position.0),
                blocks: Vec::new(),
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
