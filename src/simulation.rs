use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use bevy::prelude::{
    App, Component, Fixed, FixedUpdate, IntoScheduleConfigs, MinimalPlugins, Res, ResMut, Resource,
    SystemSet, Time, Virtual, World,
};
use serde::{Deserialize, Serialize};

use crate::{
    config::ServerConfig,
    metrics::LatencyHistogram,
    native_c001_owners::C001OwnerImportMetadataV1,
    native_import::C001InertImportMetadataV1,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleSnapshot, validate_lifecycle_relationships,
    },
    native_lifecycle_runtime::NativeLifecycleContext,
    native_relationships::{CompiledNativeRelationships, NativeRelationshipsSnapshot},
    native_runtime::{NativeRelationshipsContext, NativeWorldContext},
    native_world::{NativeWorldSnapshot, PreparedNativeWorld},
    preservation::PreservedSection,
};

/// Native schema without a preserved section; still written when none exists.
pub const SNAPSHOT_SCHEMA_DOMAIN: u32 = 1;
/// Native schema 1 plus exactly one verified preserved prospect section,
/// inline as Base64. Still loaded; no longer written (migrates to schema 3).
pub const SNAPSHOT_SCHEMA_PRESERVED: u32 = 2;
/// Native schema 1 plus a reference to exactly one preserved prospect section
/// held in a write-once, content-addressed sidecar file of the snapshot store.
pub const SNAPSHOT_SCHEMA_REFERENCED: u32 = 3;
/// Independently authored spatial world, embedded definitions and optional
/// opaque preserved section. The store writes any preserved section by reference.
pub const SNAPSHOT_SCHEMA_NATIVE: u32 = 4;
/// Explicit authored C001 inert-placement interpretation, with required source
/// preservation and immutable source identities. This is not official import.
pub const SNAPSHOT_SCHEMA_C001_INERT: u32 = 5;
/// Independently authored actor identities and mutable native foundation edges.
pub const SNAPSHOT_SCHEMA_RELATIONSHIPS: u32 = 6;
/// Complete declared spatial slots, with explicit live/dormant owner state and
/// transactional recorder bindings. Loading never replays a restore request.
pub const SNAPSHOT_SCHEMA_LIFECYCLE: u32 = 7;
/// Strict authored C001 owner restoration, with immutable source provenance.
pub const SNAPSHOT_SCHEMA_C001_OWNERS: u32 = 8;

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
    /// generations; required in schemas 2, 3, 5 and 8, optional in 4, 6 and 7.
    /// Shared and held compactly as its verified native encoding. Schema 5
    /// separately records its explicitly accepted authored interpretation.
    /// Serde writes the section inline under the stored `schema_version`, which is
    /// a loadable document only for schema 2; the snapshot store writes schemas
    /// 3 through 8 itself, with a sidecar reference.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::preservation::snapshot_section"
    )]
    pub preserved_prospect: Option<Arc<PreservedSection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_world: Option<NativeWorldSnapshot>,
    /// Required for schema 5, optional in schema 6; immutable source interpretation,
    /// independent of mutable native actor poses and simulation ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_import: Option<C001InertImportMetadataV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_relationships: Option<NativeRelationshipsSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_lifecycle: Option<LifecycleSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_owner_import: Option<C001OwnerImportMetadataV1>,
}

impl Default for WorldSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_DOMAIN,
            tick: 0,
            entities: Vec::new(),
            preserved_prospect: None,
            native_world: None,
            native_import: None,
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
        }
    }
}

impl WorldSnapshot {
    pub fn validate(&self) -> Result<()> {
        if matches!(
            self.schema_version,
            SNAPSHOT_SCHEMA_LIFECYCLE | SNAPSHOT_SCHEMA_C001_OWNERS
        ) {
            ensure!(
                self.entities.is_empty(),
                "lifecycle schema cannot hold foundation entities"
            );
            ensure!(
                self.native_import.is_none(),
                "lifecycle schema cannot hold inert import metadata"
            );
            let native = self
                .native_world
                .as_ref()
                .context("lifecycle schema requires spatial slots")?;
            let lifecycle = self
                .native_lifecycle
                .as_ref()
                .context("lifecycle schema requires lifecycle state")?;
            CompiledNativeLifecycle::from_snapshot(lifecycle, native)?;
            validate_lifecycle_relationships(
                native,
                lifecycle,
                self.native_relationships.as_ref(),
            )?;
            if self.schema_version == SNAPSHOT_SCHEMA_C001_OWNERS {
                self.native_owner_import
                    .as_ref()
                    .context("schema 8 requires owner import metadata")?
                    .validate(
                        native,
                        lifecycle,
                        self.preserved_prospect
                            .as_deref()
                            .context("schema 8 requires its preserved source")?,
                    )?;
            } else {
                ensure!(
                    self.native_owner_import.is_none(),
                    "schema 7 cannot hold owner import metadata"
                );
            }
            return Ok(());
        }
        ensure!(
            self.native_owner_import.is_none(),
            "this snapshot schema cannot hold owner import metadata"
        );
        ensure!(
            self.native_lifecycle.is_none(),
            "this snapshot schema cannot hold lifecycle state"
        );
        if self.schema_version == SNAPSHOT_SCHEMA_RELATIONSHIPS {
            ensure!(
                self.entities.is_empty(),
                "schema 6 cannot hold foundation entities"
            );
            let native = self
                .native_world
                .as_ref()
                .context("schema 6 requires native world records")?;
            native.validate()?;
            CompiledNativeRelationships::from_snapshot(
                self.native_relationships
                    .as_ref()
                    .context("schema 6 requires relationship state")?,
                native,
            )?;
            if let Some(metadata) = &self.native_import {
                metadata.validate(
                    native,
                    self.preserved_prospect
                        .as_deref()
                        .context("import metadata requires its preserved source")?,
                )?;
            }
            return Ok(());
        }
        ensure!(
            self.native_relationships.is_none(),
            "this snapshot schema cannot hold relationships"
        );
        if self.schema_version == SNAPSHOT_SCHEMA_C001_INERT {
            ensure!(
                self.entities.is_empty(),
                "schema 5 cannot hold foundation entities"
            );
            let native = self
                .native_world
                .as_ref()
                .context("schema 5 requires native world records")?;
            native.validate()?;
            return self
                .native_import
                .as_ref()
                .context("schema 5 requires import metadata")?
                .validate(
                    native,
                    self.preserved_prospect
                        .as_deref()
                        .context("schema 5 requires its preserved source")?,
                );
        }
        ensure!(
            self.native_import.is_none(),
            "this snapshot schema cannot hold import metadata"
        );
        if self.schema_version == SNAPSHOT_SCHEMA_NATIVE {
            ensure!(
                self.entities.is_empty(),
                "schema 4 cannot hold foundation entities"
            );
            return self
                .native_world
                .as_ref()
                .context("schema 4 requires native world records")?
                .validate();
        }
        ensure!(
            self.native_world.is_none(),
            "legacy schema cannot hold native world records"
        );
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

impl PreparedNativeWorld {
    /// A complete authored world at tick zero; this performs no I/O or import.
    pub fn initial_snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_NATIVE,
            native_world: Some(self.snapshot().clone()),
            ..Default::default()
        }
    }
}

/// Required capture contract, installed only by `build_app`. Losing it is an
/// incomplete world, never a reason to select a smaller persistence schema.
#[derive(Resource, Clone)]
pub struct SnapshotMode {
    native: bool,
    native_activation: Option<NativeWorldContext>,
    relationship_activation: Option<NativeRelationshipsContext>,
    lifecycle_activation: Option<NativeLifecycleContext>,
    preserved: Option<Arc<PreservedSection>>,
    imported: Option<Arc<C001InertImportMetadataV1>>,
}

/// Validate private activation pins before any native query, edit or capture.
pub(crate) fn validate_native_activation(
    world: &World,
    context: &NativeWorldContext,
) -> Result<()> {
    crate::native_lifecycle_runtime::ensure_no_lifecycle_state(world)?;
    let mode = world
        .get_resource::<SnapshotMode>()
        .context("world lacks required snapshot mode")?;
    ensure!(
        mode.native
            && mode
                .native_activation
                .as_ref()
                .is_some_and(|expected| expected.same_activation(context)),
        "world native context differs from its loaded activation"
    );
    ensure!(
        match (
            world.get_resource::<NativeRelationshipsContext>(),
            &mode.relationship_activation
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual.same_activation(expected),
            _ => false,
        },
        "world relationship context differs from its loaded activation"
    );
    Ok(())
}

/// Pin the lifecycle mode independently of its mutable entity registry. A
/// replacement updates that registry, but cannot replace the loaded contract.
pub(crate) fn validate_lifecycle_activation(
    world: &World,
    context: &NativeLifecycleContext,
) -> Result<()> {
    let mode = world
        .get_resource::<SnapshotMode>()
        .context("world lacks required snapshot mode")?;
    ensure!(
        mode.lifecycle_activation
            .as_ref()
            .is_some_and(|expected| expected.same_activation(context)),
        "world lifecycle context differs from its loaded activation"
    );
    ensure!(
        mode.native_activation.is_none()
            && mode.relationship_activation.is_none()
            && mode.imported.is_none(),
        "lifecycle world contains an incompatible capture contract"
    );
    ensure!(
        world.contains_resource::<SimulationClock>(),
        "world lacks required simulation clock"
    );
    ensure!(
        !world.contains_resource::<C001InertImportState>(),
        "lifecycle world contains source import metadata"
    );
    let actual = world
        .get_resource::<PreservedProspectState>()
        .map(|state| &state.0);
    ensure!(
        match (actual, &mode.preserved) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(actual, expected)
                    || actual.encoded_sha256() == expected.encoded_sha256(),
            _ => false,
        },
        "world preserved section differs from its loaded capture contract"
    );
    Ok(())
}

/// The loaded preserved section, handed back unchanged to every capture.
#[derive(Resource, Clone)]
pub struct PreservedProspectState(pub Arc<PreservedSection>);

/// Immutable source interpretation retained through native edits and saves.
/// Capture checks it against the privately pinned loaded contract; losing it or
/// replacing it with different metadata cannot downgrade the persistence mode.
#[derive(Resource, Clone)]
pub struct C001InertImportState(pub Arc<C001InertImportMetadataV1>);

#[derive(Resource, Default)]
pub struct SimulationClock {
    pub tick: u64,
}

/// Elapsed time on a run's clock: wall time, or exact simulated time when
/// compressed (the truncated tick interval would drift).
#[derive(Clone, Copy, Debug)]
pub struct RunClock {
    pub compressed: bool,
    pub start: Instant,
    pub start_tick: u64,
    pub tick_hz: u32,
}

impl RunClock {
    /// `tick` must not precede `start_tick`; simulation ticks only increase.
    #[must_use]
    pub fn elapsed(&self, tick: u64) -> Duration {
        if self.compressed {
            let nanos =
                u128::from(tick - self.start_tick) * 1_000_000_000 / u128::from(self.tick_hz);
            Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
        } else {
            self.start.elapsed()
        }
    }
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
    let imported = snapshot
        .native_import
        .as_ref()
        .map(|metadata| Arc::new(metadata.clone()));
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_duration(config.tick_interval()))
        .insert_resource(SimulationClock {
            tick: snapshot.tick,
        })
        .insert_resource(SnapshotMode {
            native: snapshot.native_world.is_some(),
            native_activation: None,
            relationship_activation: None,
            lifecycle_activation: None,
            preserved: snapshot.preserved_prospect.clone(),
            imported: imported.clone(),
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
    if matches!(
        snapshot.schema_version,
        SNAPSHOT_SCHEMA_LIFECYCLE | SNAPSHOT_SCHEMA_C001_OWNERS
    ) {
        crate::native_lifecycle_runtime::activate_lifecycle(app.world_mut(), snapshot)?;
    } else if let Some(native) = &snapshot.native_world {
        crate::native_runtime::activate(app.world_mut(), native)?;
        if let Some(relationships) = &snapshot.native_relationships {
            crate::native_runtime::activate_relationships(app.world_mut(), native, relationships)?;
        }
    }
    if let Some(preserved) = &snapshot.preserved_prospect {
        app.insert_resource(PreservedProspectState(Arc::clone(preserved)));
    }
    if let Some(imported) = imported {
        app.insert_resource(C001InertImportState(imported));
    }
    let native_activation = app.world().get_resource::<NativeWorldContext>().cloned();
    let relationship_activation = app
        .world()
        .get_resource::<NativeRelationshipsContext>()
        .cloned();
    let lifecycle_activation = app
        .world()
        .get_resource::<NativeLifecycleContext>()
        .cloned();
    let mut mode = app.world_mut().resource_mut::<SnapshotMode>();
    mode.native_activation = native_activation;
    mode.relationship_activation = relationship_activation;
    mode.lifecycle_activation = lifecycle_activation;
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
    let tick = world
        .get_resource::<SimulationClock>()
        .context("world lacks required simulation clock")?
        .tick;
    // Borrow the immutable capture contract; cloning it also copies the pinned
    // actor registries and relationship identities on every autosave.
    let mode = world
        .get_resource::<SnapshotMode>()
        .context("world lacks required snapshot mode")?;
    let preserved_prospect = world
        .get_resource::<PreservedProspectState>()
        .map(|state| Arc::clone(&state.0));
    let same_preserved = match (&preserved_prospect, &mode.preserved) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            Arc::ptr_eq(actual, expected) || actual.encoded_sha256() == expected.encoded_sha256()
        }
        _ => false,
    };
    ensure!(
        same_preserved,
        "world preserved section differs from its loaded capture contract"
    );
    let actual_import = world
        .get_resource::<C001InertImportState>()
        .map(|state| &state.0);
    ensure!(
        match (actual_import, &mode.imported) {
            (None, None) => true,
            (Some(actual), Some(expected)) => Arc::ptr_eq(actual, expected) || actual == expected,
            _ => false,
        },
        "world import metadata differs from its loaded capture contract"
    );
    if mode.lifecycle_activation.is_some() {
        return crate::native_lifecycle_runtime::capture_lifecycle(world);
    }
    crate::native_lifecycle_runtime::ensure_no_lifecycle_state(world)?;
    if mode.native {
        let (native, relationships) = crate::native_runtime::capture_native(world)?;
        if let Some(metadata) = &mode.imported {
            ensure!(
                native.catalog_id == metadata.native_catalog_id
                    && native.map == metadata.native_map,
                "world native context differs from its loaded import contract"
            );
        }
        let snapshot = WorldSnapshot {
            schema_version: if mode.relationship_activation.is_some() {
                SNAPSHOT_SCHEMA_RELATIONSHIPS
            } else if mode.imported.is_some() {
                SNAPSHOT_SCHEMA_C001_INERT
            } else {
                SNAPSHOT_SCHEMA_NATIVE
            },
            tick,
            entities: Vec::new(),
            preserved_prospect,
            native_world: Some(native),
            native_import: mode.imported.as_deref().cloned(),
            native_relationships: relationships,
            native_lifecycle: None,
            native_owner_import: None,
        };
        // capture_native validates every record against the immutable loaded
        // catalog. Storage independently validates the serialized snapshot;
        // recompiling that catalog again on this thread adds no check here.
        return Ok(snapshot);
    }
    crate::native_runtime::ensure_no_relationship_state(world)?;
    ensure!(
        !world.contains_resource::<crate::native_runtime::NativeWorldContext>(),
        "foundation world contains an unexpected native catalog context"
    );
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
    let snapshot = WorldSnapshot {
        schema_version: if preserved_prospect.is_some() {
            SNAPSHOT_SCHEMA_REFERENCED
        } else {
            SNAPSHOT_SCHEMA_DOMAIN
        },
        tick,
        entities,
        preserved_prospect,
        native_world: None,
        native_import: None,
        native_relationships: None,
        native_lifecycle: None,
        native_owner_import: None,
    };
    snapshot.validate()?;
    Ok(snapshot)
}
