use std::{collections::BTreeSet, sync::Arc, time::Instant};

use anyhow::{Result, bail, ensure};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{config::ServerConfig, metrics::LatencyHistogram, preservation::PreservedSection};

/// Native schema without a preserved section; still written when none exists.
pub const SNAPSHOT_SCHEMA_DOMAIN: u32 = 1;
/// Native schema 1 plus exactly one verified preserved prospect section,
/// inline as Base64. Still loaded; no longer written (migrates to schema 3).
pub const SNAPSHOT_SCHEMA_PRESERVED: u32 = 2;
/// Native schema 1 plus a reference to exactly one preserved prospect section
/// held in a write-once, content-addressed sidecar file of the snapshot store.
pub const SNAPSHOT_SCHEMA_REFERENCED: u32 = 3;

/// Stable domain identity, independent of Bevy's runtime entity allocation.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DomainId(pub u64);

#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position(pub [f32; 3]);

/// Authored foundation records, NOT the recovered ICARUS save format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRecord {
    pub id: DomainId,
    pub position: Position,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSnapshot {
    pub schema_version: u32,
    pub tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    pub entities: Vec<EntityRecord>,
    /// Lossless, value-opaque supplied-save state carried unchanged between
    /// generations; present exactly in schemas 2 and 3. Shared, never interpreted,
    /// and held compactly as its verified native encoding (no value tree).
    /// Serialization emits the legacy inline schema-2 form; the snapshot store
    /// writes schema 3 itself.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::preservation::snapshot_section"
    )]
    pub preserved_prospect: Option<Arc<PreservedSection>>,
}

impl Default for WorldSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_DOMAIN,
            tick: 0,
            entities: Vec::new(),
            preserved_prospect: None,
        }
    }
}

impl WorldSnapshot {
    pub fn validate(&self) -> Result<()> {
        match (self.schema_version, &self.preserved_prospect) {
            (SNAPSHOT_SCHEMA_DOMAIN, None)
            | (SNAPSHOT_SCHEMA_PRESERVED | SNAPSHOT_SCHEMA_REFERENCED, Some(_)) => {}
            (SNAPSHOT_SCHEMA_DOMAIN, Some(_)) => {
                bail!("native snapshot schema 1 cannot hold a preserved section")
            }
            (schema @ (SNAPSHOT_SCHEMA_PRESERVED | SNAPSHOT_SCHEMA_REFERENCED), None) => {
                bail!("native snapshot schema {schema} requires its preserved section")
            }
            (other, _) => bail!("unsupported native snapshot schema {other}"),
        }
        let mut ids = BTreeSet::new();
        for entity in &self.entities {
            ensure!(
                entity.id.0 != 0 && ids.insert(entity.id),
                "invalid or duplicate domain ID {}",
                entity.id.0
            );
            ensure!(
                entity
                    .position
                    .0
                    .iter()
                    .all(|coordinate| coordinate.is_finite()),
                "nonfinite position for {}",
                entity.id.0
            );
        }
        Ok(())
    }
}

/// The loaded preserved section, handed back unchanged to every capture.
#[derive(Resource, Clone)]
pub struct PreservedProspectState(pub Arc<PreservedSection>);

#[derive(Resource, Default)]
pub struct SimulationClock {
    pub tick: u64,
}

/// Tick-duration histogram range: 1 µs buckets up to 131,072 µs (1 MiB).
pub const TICK_HISTOGRAM_BUCKETS: usize = 131_072;

#[derive(Resource)]
pub struct TickMetrics {
    pub deadline_misses: u64,
    pub last_tick_micros: u64,
    pub max_tick_micros: u64,
    /// Every completed tick's duration since start, in fixed memory.
    pub tick_micros: LatencyHistogram,
    started: Option<Instant>,
}

impl Default for TickMetrics {
    fn default() -> Self {
        Self {
            deadline_misses: 0,
            last_tick_micros: 0,
            max_tick_micros: 0,
            tick_micros: LatencyHistogram::new(1, TICK_HISTOGRAM_BUCKETS),
            started: None,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum SimulationSet {
    Begin,
    Input,
    Gameplay,
    Finish,
}

pub fn build_app(config: &ServerConfig, snapshot: &WorldSnapshot) -> Result<App> {
    config.validate()?;
    snapshot.validate()?;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_duration(config.tick_interval()))
        .insert_resource(SimulationClock {
            tick: snapshot.tick,
        })
        .init_resource::<TickMetrics>()
        .configure_sets(
            FixedUpdate,
            (
                SimulationSet::Begin,
                SimulationSet::Input,
                SimulationSet::Gameplay,
                SimulationSet::Finish,
            )
                .chain(),
        )
        .add_systems(FixedUpdate, begin_tick.in_set(SimulationSet::Begin))
        .add_systems(FixedUpdate, finish_tick.in_set(SimulationSet::Finish));
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .set_max_delta(config.tick_interval() * config.max_catch_up_ticks);
    for entity in &snapshot.entities {
        app.world_mut().spawn((entity.id, entity.position));
    }
    if let Some(preserved) = &snapshot.preserved_prospect {
        app.insert_resource(PreservedProspectState(Arc::clone(preserved)));
    }
    Ok(app)
}

fn begin_tick(mut metrics: ResMut<TickMetrics>) {
    metrics.started = Some(Instant::now());
}

fn finish_tick(
    mut clock: ResMut<SimulationClock>,
    mut metrics: ResMut<TickMetrics>,
    time: Res<Time<Fixed>>,
) {
    clock.tick = clock.tick.saturating_add(1);
    if let Some(started) = metrics.started.take() {
        let elapsed = started.elapsed();
        metrics.last_tick_micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        metrics.max_tick_micros = metrics.max_tick_micros.max(metrics.last_tick_micros);
        let last = metrics.last_tick_micros;
        metrics.tick_micros.record(last);
        if elapsed > time.timestep() {
            metrics.deadline_misses += 1;
        }
    }
}

pub fn capture(world: &mut World) -> Result<WorldSnapshot> {
    let tick = world.resource::<SimulationClock>().tick;
    // Domain identity opts an entity into the current native snapshot schema.
    // Missing required data is an incomplete world, not a despawn to persist.
    let mut entities = Vec::new();
    for (id, position) in world.query::<(&DomainId, Option<&Position>)>().iter(world) {
        let position = anyhow::Context::with_context(position, || {
            format!("domain entity {} lacks required Position", id.0)
        })?;
        entities.push(EntityRecord {
            id: *id,
            position: *position,
        });
    }
    entities.sort_by_key(|entity| entity.id);
    // Sharing the immutable section keeps capture cheap; the worker writes its
    // held encoding only when no verified sidecar already holds it.
    let preserved_prospect = world
        .get_resource::<PreservedProspectState>()
        .map(|state| Arc::clone(&state.0));
    let snapshot = WorldSnapshot {
        schema_version: if preserved_prospect.is_some() {
            SNAPSHOT_SCHEMA_REFERENCED
        } else {
            SNAPSHOT_SCHEMA_DOMAIN
        },
        tick,
        entities,
        preserved_prospect,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
