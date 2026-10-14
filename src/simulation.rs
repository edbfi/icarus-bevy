use std::{collections::BTreeSet, time::Instant};

use anyhow::{Result, ensure};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{config::ServerConfig, metrics::LatencyHistogram};

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
}

impl Default for WorldSnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            tick: 0,
            entities: Vec::new(),
        }
    }
}

impl WorldSnapshot {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported native snapshot schema {}",
            self.schema_version
        );
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
    let snapshot = WorldSnapshot {
        schema_version: 1,
        tick,
        entities,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
