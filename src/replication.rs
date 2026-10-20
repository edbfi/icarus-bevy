//! EXPERIMENTAL native replication scheduling; nothing is sent on a wire.
//!
//! Each fixed tick, per viewer connection, this stage decides which network
//! actors to consider, which are relevant and in what priority order, then
//! applies an authored per-connection selection budget. Every decision is
//! delegated to the `experimental::actor_channel` evaluators (F-745, F-748,
//! F-749, F-751); this module only gathers their inputs and applies their
//! outcomes. It models no channel bunches, bandwidth, dormancy, ownership or
//! destruction info, and demonstrates no ICARUS compatibility. Net parameters
//! default to Unreal Engine `AActor`/`UNetDriver` defaults, never claimed to be
//! ICARUS values.
use std::{
    cmp::{Ordering, Reverse},
    collections::BTreeMap,
    time::Instant,
};

use bevy::prelude::*;
use serde::Serialize;

use crate::{
    experimental::actor_channel::{
        ActorPriorityInputs, ActorPriorityOutcome, ActorRelevancyInputs, ActorRelevancyOutcome,
        ConsiderListActorInputs, ConsiderListActorOutcome, EngineNetMode,
        MarkRelevantActorStepInputs, PrioritizeActorInputs, PrioritizeActorOutcome,
        ProcessPrioritizedActorInputs, ProcessPrioritizedActorOutcome,
        ServerReplicateActorsFrameInputs, ServerReplicateActorsFrameOutcome,
        evaluate_actor_net_priority, evaluate_actor_net_relevancy, evaluate_consider_list_actor,
        evaluate_game_engine_max_tick_rate, evaluate_mark_relevant_actor_step,
        evaluate_prep_connections_tick_budget, evaluate_prioritize_actor,
        evaluate_process_prioritized_actor, evaluate_server_replicate_actors_frame,
        round_net_priority_to_int,
    },
    simulation::{DomainId, Position, SimulationSet},
};

/// UE `AActor::NetCullDistanceSquared` default (`15,000²`); not an ICARUS value.
pub const DEFAULT_NET_CULL_DISTANCE_SQUARED: f32 = 225_000_000.0;
/// UE `AActor::NetUpdateFrequency` default (Hz); not an ICARUS value.
pub const DEFAULT_NET_UPDATE_FREQUENCY: f32 = 100.0;
/// UE `AActor::MinNetUpdateFrequency` default (Hz); not an ICARUS value.
pub const DEFAULT_MIN_NET_UPDATE_FREQUENCY: f32 = 2.0;
/// UE `AActor::NetPriority` default; not an ICARUS value.
pub const DEFAULT_NET_PRIORITY: f32 = 1.0;
/// UE `UNetDriver::NetServerMaxTickRate` default (Hz); not an ICARUS value.
pub const DEFAULT_NET_SERVER_MAX_TICK_RATE: i32 = 30;
/// UE `UNetDriver::RelevantTimeout` default (seconds); not an ICARUS value.
pub const DEFAULT_RELEVANT_TIMEOUT_SECONDS: f64 = 5.0;
/// UE `UNetDriver::SpawnPrioritySeconds` default; not an ICARUS value.
pub const DEFAULT_SPAWN_PRIORITY_SECONDS: f32 = 1.0;
/// Authored actors selected per connection per frame; no engine or ICARUS value.
pub const DEFAULT_CONNECTION_BUDGET: usize = 256;
/// Authored bound on actors evaluated per frame; the window rotates beyond it.
pub const DEFAULT_MAX_CONSIDER_LIST: usize = 65_536;
/// Authored bound on connections scheduled per frame (lowest IDs kept).
pub const DEFAULT_MAX_CONNECTIONS: usize = 64;
/// Authored bound on viewers per connection (UE connection plus children).
pub const DEFAULT_MAX_VIEWERS_PER_CONNECTION: usize = 4;

/// `ROLE_SimulatedProxy`: every native net actor replicates.
const REMOTE_ROLE_SIMULATED_PROXY: u8 = 1;
/// `DORM_Awake`, the UE `AActor::NetDormancy` default; dormancy is not modeled.
const NET_DORMANCY_AWAKE: u8 = 1;

/// Grid cells exceed the largest cull distance by this factor, far above the
/// f32 rounding of the evaluator's squared distance, so an actor outside the
/// viewer's 27 neighbouring cells can never pass the strict `<` cull test.
const GRID_CELL_MARGIN: f64 = 1.0 + 1.0 / 1024.0;
/// Scaled coordinates beyond this go unindexed (always checked), so cell
/// arithmetic cannot overflow and huge coordinates stay exact.
const GRID_INDEX_LIMIT: f64 = (1_i64 << 40) as f64;

/// Per-actor net parameters. Defaults are UE engine defaults, not ICARUS values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NetParameters {
    pub net_cull_distance_squared: f32,
    pub net_update_frequency: f32,
    pub min_net_update_frequency: f32,
    pub net_priority: f32,
}

impl Default for NetParameters {
    fn default() -> Self {
        Self {
            net_cull_distance_squared: DEFAULT_NET_CULL_DISTANCE_SQUARED,
            net_update_frequency: DEFAULT_NET_UPDATE_FREQUENCY,
            min_net_update_frequency: DEFAULT_MIN_NET_UPDATE_FREQUENCY,
            net_priority: DEFAULT_NET_PRIORITY,
        }
    }
}

/// A replicated actor; its location is the entity's [`Position`]. Domain IDs
/// must be unique; later duplicates are skipped and counted.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(NetSchedule)]
pub struct NetActor {
    pub id: DomainId,
    pub params: NetParameters,
}

/// The `FNetworkObjectInfo` scheduling fields the evaluators read and update.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct NetSchedule {
    next_update_time: f64,
    last_net_replicate_time: f64,
    optimal_net_update_delta: f32,
    pending_net_update: bool,
    force_relevant_frame: i32,
}

/// One viewpoint of a connection. Several viewers may share a connection ID
/// (UE child connections): relevant to any, priority is the maximum.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct NetViewer {
    pub connection: u32,
    pub location: [f32; 3],
    pub direction: [f32; 3],
}

/// How relevant actors are found; both give exactly the same relevant set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialMode {
    /// Every considered actor is tested against every viewer.
    Naive,
    /// A uniform grid sized from the largest cull distance prunes the tests;
    /// rebuilt every frame.
    Grid,
}

/// Explicit, authored stage configuration; see the constants for provenance.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct ReplicationConfig {
    pub spatial: SpatialMode,
    pub connection_budget: usize,
    pub max_consider_list: usize,
    pub max_connections: usize,
    pub max_viewers_per_connection: usize,
    pub net_server_max_tick_rate: i32,
    /// `net.MaxConnectionsToTickPerServerFrame`; `0` ticks every connection.
    pub max_connections_to_tick_per_server_frame: i32,
    pub relevant_timeout_seconds: f64,
    pub spawn_priority_seconds: f32,
    /// `net.UseAdaptiveNetUpdateFrequency`; off as in UE.
    pub use_adaptive_net_update_frequency: bool,
    /// Seeds the evaluators' `srand` inputs, so a seed replays a schedule.
    pub seed: u64,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            // The grid's per-frame build cost exceeded its savings at ~2,700
            // actors and up to 8 viewers (`replication_bench`).
            spatial: SpatialMode::Naive,
            connection_budget: DEFAULT_CONNECTION_BUDGET,
            max_consider_list: DEFAULT_MAX_CONSIDER_LIST,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_viewers_per_connection: DEFAULT_MAX_VIEWERS_PER_CONNECTION,
            net_server_max_tick_rate: DEFAULT_NET_SERVER_MAX_TICK_RATE,
            max_connections_to_tick_per_server_frame: 0,
            relevant_timeout_seconds: DEFAULT_RELEVANT_TIMEOUT_SECONDS,
            spawn_priority_seconds: DEFAULT_SPAWN_PRIORITY_SECONDS,
            use_adaptive_net_update_frequency: false,
            seed: 0x1ca7_05b0_0000_0001,
        }
    }
}

/// How the frame-level `ServerReplicateActors` evaluator disposed the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameDisposition {
    #[default]
    NotRun,
    NoConnections,
    Aborted,
    Replicated,
}

/// One actor chosen for a connection, in send order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SelectedActor {
    pub id: DomainId,
    /// `appRoundToInt(65536 * max viewer GetNetPriority)`.
    pub priority: i32,
}

/// One ticked connection's outcome. Connections appear in tick order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ConnectionFrame {
    pub connection: u32,
    pub viewers: usize,
    /// Considered actors relevant to at least one viewer.
    pub relevant: usize,
    /// Entries the prioritize evaluator ranked.
    pub prioritized: usize,
    pub selected: Vec<SelectedActor>,
    pub saturated: bool,
    pub closed_channels: usize,
    pub open_channels: usize,
}

/// Every scheduling decision of a frame; timing-free, so comparable.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameDecisions {
    pub disposition: FrameDisposition,
    pub replication_frame: i32,
    pub server_tick_time: f32,
    pub cpu_saturated: bool,
    /// Distinct net actors present.
    pub actors: usize,
    pub duplicate_ids: usize,
    /// Actors left out of this frame's rotating consider window.
    pub consider_overflow: usize,
    pub considered: usize,
    pub connections_overflow: usize,
    pub viewers_overflow: usize,
    pub connections_not_ticked: usize,
    pub connections: Vec<ConnectionFrame>,
}

/// Real monotonic stage cost; diagnostic only, never an input to decisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FrameTimings {
    pub consider_nanos: u64,
    pub index_nanos: u64,
    pub relevancy_nanos: u64,
    pub prioritize_nanos: u64,
    pub total_nanos: u64,
}

/// The latest frame's results. Identifies actors by domain ID only.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize)]
pub struct ReplicationFrame {
    pub decisions: FrameDecisions,
    pub timings: FrameTimings,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct ReplicationSet;

/// Runs the stage in `FixedUpdate` after gameplay and before the tick closes,
/// so its cost counts toward the tick deadline.
#[derive(Default)]
pub struct ReplicationPlugin {
    pub config: ReplicationConfig,
}

impl Plugin for ReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.config)
            .insert_resource(ReplicationState::new(self.config.seed))
            .init_resource::<ReplicationFrame>()
            .configure_sets(
                FixedUpdate,
                ReplicationSet
                    .after(SimulationSet::Gameplay)
                    .before(SimulationSet::Finish),
            )
            .add_systems(FixedUpdate, replicate_actors.in_set(ReplicationSet));
    }
}

/// xorshift64*: the seeded source of the evaluators' `srand` inputs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mixed = seed ^ 0x9e37_79b9_7f4a_7c15;
        Self(if mixed == 0 { 1 } else { mixed })
    }

    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (value >> 40) as f32 / (1_u32 << 24) as f32
    }
}

#[derive(Clone, Copy)]
struct Channel {
    last_update_time: f64,
    relevant_time: f64,
}

#[derive(Default)]
struct ConnectionState {
    channels: BTreeMap<DomainId, Channel>,
    last_processed_frame: i32,
}

/// Persistent driver and per-connection state between frames.
#[derive(Resource)]
pub struct ReplicationState {
    rng: Rng,
    replication_frame: i32,
    driver_net_tag: i32,
    consider_cursor: usize,
    connection_order: Vec<u32>,
    connections: BTreeMap<u32, ConnectionState>,
    marks: Vec<u32>,
    stamp: u32,
    /// Ascending distinct actor IDs of the previous frame.
    live_ids: Vec<DomainId>,
}

impl ReplicationState {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            replication_frame: 0,
            driver_net_tag: 0,
            consider_cursor: 0,
            connection_order: Vec::new(),
            connections: BTreeMap::new(),
            marks: Vec::new(),
            stamp: 0,
            live_ids: Vec::new(),
        }
    }

    fn next_stamp(&mut self, len: usize) -> u32 {
        if self.marks.len() < len {
            self.marks.resize(len, 0);
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.marks.fill(0);
            self.stamp = 1;
        }
        self.stamp
    }
}

/// An actor's relevancy-test inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelevancyCandidate {
    pub location: [f32; 3],
    pub net_cull_distance_squared: f32,
}

/// `AActor::IsNetRelevantFor` for a visible, ownerless actor with a colliding
/// root component under distance-based relevancy, via the evaluator.
pub fn is_relevant_to_viewer(candidate: &RelevancyCandidate, viewer: [f32; 3]) -> bool {
    let outcome = evaluate_actor_net_relevancy(&ActorRelevancyInputs {
        b_always_relevant: false,
        b_only_relevant_to_owner: false,
        b_hidden: false,
        b_use_owner_relevancy: false,
        is_owned_by_view_target_or_real_viewer: false,
        view_target_is_instigator: false,
        has_owner: false,
        has_root_component: true,
        root_collision_enabled: true,
        attach_parent_delegates_to_owner: false,
        b_use_distance_based_relevancy: true,
        actor_location: candidate.location,
        src_location: viewer,
        net_cull_distance_squared: candidate.net_cull_distance_squared,
    });
    outcome == ActorRelevancyOutcome::Relevant
}

/// All-pairs reference: writes the ascending indices relevant to `viewer`.
pub fn naive_relevant(candidates: &[RelevancyCandidate], viewer: [f32; 3], out: &mut Vec<usize>) {
    out.clear();
    out.extend(
        candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| is_relevant_to_viewer(candidate, viewer))
            .map(|(index, _)| index),
    );
}

/// Exact `floor` for values already inside `GRID_INDEX_LIMIT`.
fn floor_to_i64(value: f64) -> i64 {
    let truncated = value as i64;
    if (truncated as f64) > value {
        truncated - 1
    } else {
        truncated
    }
}

/// Signed cell coordinates, or `None` when out of range or non-finite.
fn grid_cell(location: [f32; 3], inverse_cell: f64) -> Option<[i64; 3]> {
    let mut cell = [0_i64; 3];
    for (slot, coordinate) in cell.iter_mut().zip(location) {
        let scaled = f64::from(coordinate) * inverse_cell;
        // NaN fails the range test too.
        if !(-GRID_INDEX_LIMIT..=GRID_INDEX_LIMIT).contains(&scaled) {
            return None;
        }
        *slot = floor_to_i64(scaled);
    }
    Some(cell)
}

/// Uniform grid over candidates, hashed into a power-of-two bucket table
/// filled by counting sort. Cells are the largest finite positive cull
/// distance plus [`GRID_CELL_MARGIN`]; actors with a non-finite cull distance
/// or an out-of-range cell are always tested. A viewer tests every actor in
/// the buckets of its 27 neighbouring cells; hash collisions only add tests,
/// so the result is exactly [`naive_relevant`]'s set.
pub struct RelevancyGrid {
    inverse_cell: Option<f64>,
    bucket_mask: u64,
    /// `starts[b]..starts[b + 1]` indexes `members` for bucket `b`.
    starts: Vec<u32>,
    members: Vec<u32>,
    unindexed: Vec<usize>,
}

impl RelevancyGrid {
    pub fn build(candidates: &[RelevancyCandidate]) -> Self {
        let largest = candidates
            .iter()
            .map(|candidate| candidate.net_cull_distance_squared)
            .filter(|cull| cull.is_finite() && *cull > 0.0)
            .fold(0.0_f32, f32::max);
        let inverse_cell = Some(1.0 / (f64::from(largest).sqrt() * GRID_CELL_MARGIN))
            .filter(|inverse| inverse.is_finite() && *inverse > 0.0)
            .filter(|_| u32::try_from(candidates.len()).is_ok());
        let buckets = (candidates.len() * 2).next_power_of_two().max(16);
        let mut grid = Self {
            inverse_cell,
            bucket_mask: buckets as u64 - 1,
            starts: Vec::new(),
            members: Vec::new(),
            unindexed: Vec::new(),
        };
        let Some(inverse_cell) = inverse_cell else {
            grid.unindexed.extend(0..candidates.len());
            return grid;
        };
        let mut bucket_of = Vec::with_capacity(candidates.len());
        let mut starts = vec![0_u32; buckets + 1];
        for (index, candidate) in candidates.iter().enumerate() {
            let cell = Some(candidate)
                .filter(|candidate| candidate.net_cull_distance_squared.is_finite())
                .and_then(|candidate| grid_cell(candidate.location, inverse_cell));
            let bucket = cell.map(|cell| grid.bucket(cell));
            match bucket {
                Some(bucket) => starts[bucket + 1] += 1,
                None => grid.unindexed.push(index),
            }
            bucket_of.push(bucket);
        }
        for bucket in 0..buckets {
            starts[bucket + 1] += starts[bucket];
        }
        let mut cursor = starts.clone();
        let mut members = vec![0_u32; starts[buckets] as usize];
        for (index, bucket) in bucket_of.into_iter().enumerate() {
            if let Some(bucket) = bucket {
                members[cursor[bucket] as usize] = index as u32;
                cursor[bucket] += 1;
            }
        }
        grid.starts = starts;
        grid.members = members;
        grid
    }

    fn bucket(&self, cell: [i64; 3]) -> usize {
        let hash = (cell[0] as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (cell[1] as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ (cell[2] as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
        ((hash ^ (hash >> 29)) & self.bucket_mask) as usize
    }

    /// Writes the ascending indices relevant to `viewer`.
    pub fn relevant(
        &self,
        candidates: &[RelevancyCandidate],
        viewer: [f32; 3],
        out: &mut Vec<usize>,
    ) {
        let Some(center) = self
            .inverse_cell
            .and_then(|inverse_cell| grid_cell(viewer, inverse_cell))
        else {
            naive_relevant(candidates, viewer, out);
            return;
        };
        let mut buckets = [0_usize; 27];
        let mut slot = 0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    buckets[slot] = self.bucket([center[0] + dx, center[1] + dy, center[2] + dz]);
                    slot += 1;
                }
            }
        }
        // Neighbouring cells may share a bucket; test each bucket once.
        buckets.sort_unstable();
        out.clear();
        let mut previous = None;
        for bucket in buckets {
            if previous == Some(bucket) {
                continue;
            }
            previous = Some(bucket);
            let members =
                &self.members[self.starts[bucket] as usize..self.starts[bucket + 1] as usize];
            out.extend(
                members
                    .iter()
                    .map(|&index| index as usize)
                    .filter(|&index| is_relevant_to_viewer(&candidates[index], viewer)),
            );
        }
        out.extend(
            self.unindexed
                .iter()
                .copied()
                .filter(|&index| is_relevant_to_viewer(&candidates[index], viewer)),
        );
        out.sort_unstable();
    }
}

struct ActorRow {
    entity: Entity,
    id: DomainId,
    location: [f32; 3],
    params: NetParameters,
    schedule: NetSchedule,
}

struct ConsiderEntry {
    row: usize,
    id: DomainId,
    net_tag: i32,
}

struct Prioritized {
    entry: usize,
    priority: i32,
}

struct FrameClock {
    world_time: f32,
    elapsed: f64,
    delta: f32,
}

/// Frame-wide inputs shared by every connection.
struct FrameContext<'a> {
    config: &'a ReplicationConfig,
    clock: &'a FrameClock,
    frame_number: i32,
    candidates: &'a [RelevancyCandidate],
}

/// One connection's viewers and this frame's relevancy stamp.
struct ConnectionJob<'a> {
    id: u32,
    viewers: &'a [NetViewer],
    stamp: u32,
}

fn nanos_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn compare_viewers(left: &NetViewer, right: &NetViewer) -> Ordering {
    left.connection.cmp(&right.connection).then_with(|| {
        left.location
            .iter()
            .chain(&left.direction)
            .zip(right.location.iter().chain(&right.direction))
            .map(|(a, b)| a.total_cmp(b))
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    })
}

fn replicate_actors(
    time: Res<Time>,
    config: Res<ReplicationConfig>,
    mut state: ResMut<ReplicationState>,
    mut frame: ResMut<ReplicationFrame>,
    mut actors: Query<(Entity, &NetActor, &Position, &mut NetSchedule)>,
    viewers: Query<&NetViewer>,
) {
    let started = Instant::now();
    let clock = FrameClock {
        world_time: time.elapsed_secs(),
        elapsed: time.elapsed_secs_f64(),
        delta: time.delta_secs(),
    };
    let mut rows: Vec<ActorRow> = actors
        .iter()
        .map(|(entity, actor, position, schedule)| ActorRow {
            entity,
            id: actor.id,
            location: position.0,
            params: actor.params,
            schedule: *schedule,
        })
        .collect();
    let viewers: Vec<NetViewer> = viewers.iter().copied().collect();
    let mut result = ReplicationFrame::default();
    state.run_frame(&config, &clock, &mut rows, viewers, &mut result);
    // Rows keep query order, and nothing changes the archetypes in between.
    for ((entity, _, _, mut schedule), row) in actors.iter_mut().zip(&rows) {
        debug_assert_eq!(entity, row.entity);
        if *schedule != row.schedule {
            *schedule = row.schedule;
        }
    }
    result.timings.total_nanos = nanos_since(started);
    *frame = result;
}

impl ReplicationState {
    fn sync_connections(
        &mut self,
        config: &ReplicationConfig,
        mut viewers: Vec<NetViewer>,
        decisions: &mut FrameDecisions,
    ) -> BTreeMap<u32, Vec<NetViewer>> {
        viewers.sort_by(compare_viewers);
        let mut grouped: BTreeMap<u32, Vec<NetViewer>> = BTreeMap::new();
        for viewer in viewers {
            let known = grouped.contains_key(&viewer.connection);
            if !known && grouped.len() >= config.max_connections {
                decisions.connections_overflow += 1;
                continue;
            }
            let group = grouped.entry(viewer.connection).or_default();
            if group.len() < config.max_viewers_per_connection {
                group.push(viewer);
            } else {
                decisions.viewers_overflow += 1;
            }
        }
        grouped.retain(|_, group| !group.is_empty());
        // Keep the rotated tick order; new connections join at the end.
        self.connection_order.retain(|id| grouped.contains_key(id));
        self.connections.retain(|id, _| grouped.contains_key(id));
        for id in grouped.keys() {
            if !self.connections.contains_key(id) {
                self.connections.insert(*id, ConnectionState::default());
                self.connection_order.push(*id);
            }
        }
        grouped
    }

    fn run_frame(
        &mut self,
        config: &ReplicationConfig,
        clock: &FrameClock,
        rows: &mut [ActorRow],
        viewers: Vec<NetViewer>,
        result: &mut ReplicationFrame,
    ) {
        let decisions = &mut result.decisions;
        let grouped = self.sync_connections(config, viewers, decisions);

        // Distinct actors in domain-ID order; later duplicates are skipped.
        let mut sorted: Vec<usize> = (0..rows.len()).collect();
        sorted.sort_unstable_by_key(|&index| (rows[index].id, rows[index].entity));
        let mut live: Vec<usize> = Vec::with_capacity(rows.len());
        for index in sorted {
            if live
                .last()
                .is_some_and(|&last| rows[last].id == rows[index].id)
            {
                decisions.duplicate_ids += 1;
            } else {
                live.push(index);
            }
        }
        decisions.actors = live.len();
        // Close channels of despawned actors when the actor set changed: both
        // sides ascend, so merge.
        let live_ids: Vec<DomainId> = live.iter().map(|&index| rows[index].id).collect();
        if live_ids != self.live_ids {
            for connection in self.connections.values_mut() {
                let mut cursor = 0;
                connection.channels.retain(|id, _| {
                    cursor += live_ids[cursor..].partition_point(|live_id| live_id < id);
                    live_ids.get(cursor) == Some(id)
                });
            }
            self.live_ids = live_ids;
        }

        let max_tick_rate = evaluate_game_engine_max_tick_rate(
            EngineNetMode::DedicatedServer,
            false,
            config.net_server_max_tick_rate,
            0.0,
        );
        let connection_count = self.connection_order.len();
        let (num_clients_to_tick, _) = evaluate_prep_connections_tick_budget(
            connection_count,
            false,
            clock.delta,
            0.0,
            max_tick_rate,
            false,
            config.max_connections_to_tick_per_server_frame,
        );
        let outcome = evaluate_server_replicate_actors_frame(&ServerReplicateActorsFrameInputs {
            num_client_connections: i32::try_from(connection_count).unwrap_or(i32::MAX),
            has_replication_driver: false,
            replication_driver_updated_count: 0,
            current_replication_frame: self.replication_frame,
            has_world: true,
            num_clients_to_tick: i32::try_from(num_clients_to_tick).unwrap_or(i32::MAX),
            delta_seconds: clock.delta,
            max_tick_rate,
        });
        let (frame_number, server_tick_time, rotate) = match outcome {
            ServerReplicateActorsFrameOutcome::NoClientConnections
            | ServerReplicateActorsFrameOutcome::DelegatedToReplicationDriver { .. } => {
                decisions.disposition = FrameDisposition::NoConnections;
                decisions.replication_frame = self.replication_frame;
                return;
            }
            ServerReplicateActorsFrameOutcome::AbortedAfterFrameIncrement {
                next_replication_frame,
            } => {
                self.replication_frame = next_replication_frame;
                decisions.disposition = FrameDisposition::Aborted;
                decisions.replication_frame = next_replication_frame;
                decisions.connections_not_ticked = connection_count;
                return;
            }
            ServerReplicateActorsFrameOutcome::RunConnectionReplication {
                next_replication_frame,
                server_tick_time,
                b_cpu_saturated,
                rotate_connections_left_by,
            } => {
                decisions.cpu_saturated = b_cpu_saturated;
                (
                    next_replication_frame,
                    server_tick_time,
                    usize::try_from(rotate_connections_left_by).unwrap_or(0),
                )
            }
        };
        self.replication_frame = frame_number;
        decisions.disposition = FrameDisposition::Replicated;
        decisions.replication_frame = frame_number;
        decisions.server_tick_time = server_tick_time;

        // BuildConsiderList over a bounded window that rotates when the
        // population exceeds it, so no actor is starved by its ID.
        let consider_started = Instant::now();
        let window = live.len().min(config.max_consider_list);
        decisions.consider_overflow = live.len() - window;
        let mut window_rows: Vec<usize> = if window == live.len() {
            self.consider_cursor = 0;
            live.clone()
        } else {
            let start = self.consider_cursor % live.len();
            self.consider_cursor = (start + window) % live.len();
            live.iter()
                .cycle()
                .skip(start)
                .take(window)
                .copied()
                .collect()
        };
        window_rows.sort_unstable_by_key(|&index| rows[index].id);
        let mut entries: Vec<ConsiderEntry> = Vec::with_capacity(window_rows.len());
        for row_index in window_rows {
            let row = &mut rows[row_index];
            let outcome = evaluate_consider_list_actor(&ConsiderListActorInputs {
                b_pending_net_update: row.schedule.pending_net_update,
                next_update_time: row.schedule.next_update_time,
                last_net_replicate_time: row.schedule.last_net_replicate_time,
                optimal_net_update_delta: row.schedule.optimal_net_update_delta,
                world_time_seconds: clock.world_time,
                driver_elapsed_time: clock.elapsed,
                is_pending_kill: false,
                remote_role: REMOTE_ROLE_SIMULATED_PROXY,
                net_driver_name_matches: true,
                b_net_startup: false,
                net_dormancy: NET_DORMANCY_AWAKE,
                level_initialized: true,
                net_update_frequency: row.params.net_update_frequency,
                min_net_update_frequency: row.params.min_net_update_frequency,
                b_use_adaptive_net_update_frequency: config.use_adaptive_net_update_frequency,
                random_unit_float: self.rng.unit(),
                server_tick_time,
            });
            if let ConsiderListActorOutcome::AddToConsiderListAndPreReplicate {
                next_update_time,
                last_net_replicate_time,
                optimal_net_update_delta,
                ..
            } = outcome
            {
                row.schedule.next_update_time = next_update_time;
                row.schedule.last_net_replicate_time = last_net_replicate_time;
                row.schedule.optimal_net_update_delta = optimal_net_update_delta;
                row.schedule.pending_net_update = false;
                entries.push(ConsiderEntry {
                    row: row_index,
                    id: row.id,
                    net_tag: self.driver_net_tag,
                });
            }
        }
        decisions.considered = entries.len();
        result.timings.consider_nanos = nanos_since(consider_started);

        let index_started = Instant::now();
        let candidates: Vec<RelevancyCandidate> = entries
            .iter()
            .map(|entry| RelevancyCandidate {
                location: rows[entry.row].location,
                net_cull_distance_squared: rows[entry.row].params.net_cull_distance_squared,
            })
            .collect();
        let grid = (config.spatial == SpatialMode::Grid).then(|| RelevancyGrid::build(&candidates));
        // Force-relevant entries; mark steps only raise the frame, so the
        // list only grows within a frame.
        let mut forced: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| rows[entry.row].schedule.force_relevant_frame >= frame_number)
            .map(|(index, _)| index)
            .collect();
        result.timings.index_nanos = nanos_since(index_started);

        let ticked = num_clients_to_tick.min(connection_count);
        decisions.connections_not_ticked = connection_count - ticked;
        let order: Vec<u32> = self.connection_order[..ticked].to_vec();
        let mut scratch = Vec::new();
        for connection_id in order {
            let viewers = &grouped[&connection_id];
            let relevancy_started = Instant::now();
            let stamp = self.next_stamp(entries.len());
            let mut candidate_entries = Vec::new();
            for viewer in viewers {
                match &grid {
                    Some(grid) => grid.relevant(&candidates, viewer.location, &mut scratch),
                    None => naive_relevant(&candidates, viewer.location, &mut scratch),
                }
                for &index in &scratch {
                    if self.marks[index] != stamp {
                        self.marks[index] = stamp;
                        candidate_entries.push(index);
                    }
                }
            }
            let relevant_count = candidate_entries.len();
            result.timings.relevancy_nanos += nanos_since(relevancy_started);

            let prioritize_started = Instant::now();
            let context = FrameContext {
                config,
                clock,
                frame_number,
                candidates: &candidates,
            };
            let job = ConnectionJob {
                id: connection_id,
                viewers,
                stamp,
            };
            let connection_frame = self.replicate_connection(
                &context,
                &job,
                rows,
                &mut entries,
                candidate_entries,
                &mut forced,
            );
            result.timings.prioritize_nanos += nanos_since(prioritize_started);
            decisions.connections.push(ConnectionFrame {
                relevant: relevant_count,
                ..connection_frame
            });
        }
        if rotate > 0 && rotate < self.connection_order.len() {
            self.connection_order.rotate_left(rotate);
        }
    }

    /// `PrioritizeActors`, `ProcessPrioritizedActors` and `MarkRelevantActors`
    /// for one connection, over the entries that can be selected: relevant to a
    /// viewer, holding a channel, or force-relevant this frame. Every other
    /// entry would evaluate to `SkipNotRelevantNoClose`.
    fn replicate_connection(
        &mut self,
        context: &FrameContext<'_>,
        job: &ConnectionJob<'_>,
        rows: &mut [ActorRow],
        entries: &mut [ConsiderEntry],
        mut candidate_entries: Vec<usize>,
        forced: &mut Vec<usize>,
    ) -> ConnectionFrame {
        let FrameContext {
            config,
            clock,
            frame_number,
            candidates,
        } = *context;
        let ConnectionJob {
            id: connection_id,
            viewers,
            stamp,
        } = *job;
        let mut connection = self.connections.remove(&connection_id).unwrap_or_default();
        for id in connection.channels.keys() {
            if let Ok(index) = entries.binary_search_by_key(id, |entry| entry.id) {
                candidate_entries.push(index);
            }
        }
        // Includes entries an earlier connection's mark step forced.
        candidate_entries.extend_from_slice(forced);
        self.driver_net_tag = self.driver_net_tag.wrapping_add(1);
        let driver_net_tag = self.driver_net_tag;

        let mut prioritized = Vec::with_capacity(candidate_entries.len());
        for index in candidate_entries {
            let entry = &mut entries[index];
            let row = &rows[entry.row];
            let channel = connection.channels.get(&row.id).copied();
            // The evaluator yields the disposition and the elapsed time GetNetPriority
            // scales; the viewer maximum is rounded by the same evaluator rule.
            let outcome = evaluate_prioritize_actor(&PrioritizeActorInputs {
                has_channel: channel.is_some(),
                b_only_relevant_to_owner: false,
                is_relevant_to_at_least_one_viewer: self.marks[index] == stamp,
                driver_elapsed_time: clock.elapsed,
                channel_relevant_time: channel.map_or(0.0, |channel| channel.relevant_time),
                relevant_timeout: config.relevant_timeout_seconds,
                actor_net_tag: entry.net_tag,
                driver_net_tag,
                is_actor_dormant_on_connection: false,
                actor_net_dormancy: NET_DORMANCY_AWAKE,
                channel_b_pending_dormancy: false,
                channel_dormant: false,
                get_net_dormancy_returns_dormant_partial: false,
                level_initialized_for_actor: true,
                channel_last_update_time: channel.map_or(0.0, |channel| channel.last_update_time),
                channel_open_elapsed_time: f64::from(config.spawn_priority_seconds),
                max_net_priority_across_viewers: 0.0,
            });
            let PrioritizeActorOutcome::Prioritized {
                new_actor_net_tag,
                time_since_last_update,
                ..
            } = outcome
            else {
                continue;
            };
            entry.net_tag = new_actor_net_tag;
            let mut best = 0.0_f32;
            for viewer in viewers {
                if let ActorPriorityOutcome::Computed(priority) =
                    evaluate_actor_net_priority(&ActorPriorityInputs {
                        b_use_owner_relevancy: false,
                        has_owner: false,
                        is_view_target_or_instigator: false,
                        b_hidden: false,
                        root_location: Some(candidates[index].location),
                        viewer_location: viewer.location,
                        viewer_dir: viewer.direction,
                        time: time_since_last_update,
                        net_priority: row.params.net_priority,
                    })
                {
                    best = best.max(priority);
                }
            }
            prioritized.push(Prioritized {
                entry: index,
                priority: round_net_priority_to_int(best),
            });
        }
        prioritized.sort_unstable_by_key(|item| (Reverse(item.priority), entries[item.entry].id));

        let mut selected = Vec::new();
        let mut closed_channels = 0;
        let mut saturated = false;
        let mut processed = prioritized.len();
        for (position, item) in prioritized.iter().enumerate() {
            let row = &rows[entries[item.entry].row];
            let channel = connection.channels.get(&row.id).copied();
            let outcome = evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
                connection_is_net_ready: selected.len() < config.connection_budget,
                is_destruction_info: false,
                destruction_level_initialized: false,
                has_channel: channel.is_some(),
                driver_elapsed_time: clock.elapsed,
                channel_relevant_time: channel.map_or(0.0, |channel| channel.relevant_time),
                relevant_timeout: config.relevant_timeout_seconds,
                is_relevant_to_connection: self.marks[item.entry] == stamp,
                actor_b_tear_off: false,
                force_relevant_frame: row.schedule.force_relevant_frame,
                connection_last_processed_frame: connection.last_processed_frame,
                channel_b_is_replicating_actor: false,
                actor_b_always_relevant: false,
                actor_b_net_temporary: false,
                channel_open_acked: channel.is_some(),
                actor_level_initialized: true,
                package_map_supports_object: true,
                actor_net_update_frequency: row.params.net_update_frequency,
                world_time_seconds: clock.world_time,
                random_unit_float: self.rng.unit(),
                can_create_actor_channel: true,
                replicate_actor_wrote_anything: true,
                connection_is_net_ready_after_replicate: selected.len() + 1
                    < config.connection_budget,
                actor_is_net_startup: false,
            });
            let replicated = match outcome {
                ProcessPrioritizedActorOutcome::StopConnectionSaturated => {
                    saturated = true;
                    processed = position;
                    break;
                }
                ProcessPrioritizedActorOutcome::ReplicateActorAndStopSaturated {
                    updated_relevant_time,
                    ..
                } => {
                    saturated = true;
                    processed = position + 1;
                    Some(updated_relevant_time)
                }
                ProcessPrioritizedActorOutcome::ReplicateActorContinue {
                    updated_relevant_time,
                    ..
                } => Some(updated_relevant_time),
                ProcessPrioritizedActorOutcome::SkipAlreadyReplicatingOrAckedNetTemporary {
                    updated_relevant_time: Some(relevant_time),
                } => {
                    if let Some(channel) = connection.channels.get_mut(&row.id) {
                        channel.relevant_time = relevant_time;
                    }
                    None
                }
                ProcessPrioritizedActorOutcome::CloseChannel { .. } => {
                    connection.channels.remove(&row.id);
                    closed_channels += 1;
                    None
                }
                _ => None,
            };
            if let Some(updated_relevant_time) = replicated {
                // Authored stand-in for `ReplicateActor`: the channel records
                // the send time that the next frame's priority scales.
                let channel = connection.channels.entry(row.id).or_insert(Channel {
                    last_update_time: clock.elapsed,
                    relevant_time: clock.elapsed,
                });
                channel.last_update_time = clock.elapsed;
                if let Some(relevant_time) = updated_relevant_time {
                    channel.relevant_time = relevant_time;
                }
                selected.push(SelectedActor {
                    id: row.id,
                    priority: item.priority,
                });
            }
            if saturated {
                break;
            }
        }

        for item in &prioritized[processed..] {
            let row = &mut rows[entries[item.entry].row];
            let channel = connection.channels.get_mut(&row.id);
            let outcome = evaluate_mark_relevant_actor_step(&MarkRelevantActorStepInputs {
                actor_info_present: true,
                has_channel: channel.is_some(),
                driver_elapsed_time: clock.elapsed,
                channel_relevant_time: channel
                    .as_ref()
                    .map_or(0.0, |channel| channel.relevant_time),
                is_relevant_to_any_viewer: self.marks[item.entry] == stamp,
                random_unit_float: self.rng.unit(),
                current_b_pending_net_update: row.schedule.pending_net_update,
                current_force_relevant_frame: row.schedule.force_relevant_frame,
                connection_last_processed_frame: connection.last_processed_frame,
                driver_replication_frame: frame_number,
            });
            row.schedule.pending_net_update = outcome.b_pending_net_update;
            if row.schedule.force_relevant_frame < frame_number
                && outcome.force_relevant_frame >= frame_number
            {
                forced.push(item.entry);
            }
            row.schedule.force_relevant_frame = outcome.force_relevant_frame;
            if let (Some(channel), Some(relevant_time)) =
                (channel, outcome.updated_channel_relevant_time)
            {
                channel.relevant_time = relevant_time;
            }
        }
        connection.last_processed_frame = frame_number;
        let frame = ConnectionFrame {
            connection: connection_id,
            viewers: viewers.len(),
            relevant: 0,
            prioritized: prioritized.len(),
            selected,
            saturated,
            closed_channels,
            open_channels: connection.channels.len(),
        };
        self.connections.insert(connection_id, connection);
        frame
    }
}
