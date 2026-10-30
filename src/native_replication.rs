//! Opt-in, independently authored spatial/lifecycle host integration. This declaration
//! supplies lab identities and coordinates; it makes no original-save mapping
//! or stock-client compatibility claim. Native state remains authoritative.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{
    App, Component, Entity, FixedUpdate, IntoScheduleConfigs, Resource, SystemSet, World,
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    experimental::{
        archive::ByteOrder,
        peer_lab::PeerLab,
        session_open::{
            ActorKey, ActorOpenSpec, ConnectionExports, NetGuidCache, PathSegment,
            authored_movement, compose_actor_open, world_actor_state_block,
        },
    },
    native_lifecycle_runtime::{NativeLifecycleContext, replication_slots},
    native_runtime::{NativeWorldContext, replication_actors},
    native_world::NativeTransform,
    replication::{
        NetActor, NetActorFlags, NetParameters, NetSchedule, NetViewer, ReplicationConfig,
        ReplicationFrame, ReplicationPlugin, ReplicationSet, ReplicationState, SpatialMode,
    },
    session_replication::{
        NetArchetype, NetIncarnation, NetMotion, SessionReplication, SessionReplicationPlugin,
    },
    simulation::{DomainId, Position, SNAPSHOT_SCHEMA_NATIVE, SimulationSet, capture},
};

pub const MAX_DECLARATION_BYTES: usize = 1024 * 1024;
pub const MAX_BINDINGS: usize = 1024;
pub const MAX_PATH_DEPTH: usize = 8;
pub const MAX_STRING_BYTES: usize = 1024;

// Named objects only, including nested objects; serde's default struct decoder
// would also accept positional arrays. No names/values enter parse diagnostics.
macro_rules! object_dto {
    (pub struct $name:ident { $($(#[$meta:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$meta])* $field: $ty),* }
                let fields: Fields = crate::json::object(deserializer)
                    .map_err(|_| <D::Error as serde::de::Error>::custom("native replication object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
    };
}

object_dto! {
    pub struct AuthoredReplication {
        pub format: u32,
        pub catalog_id: String,
        /// Required only in format 2: pins the authored lifecycle definition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub lifecycle_id: Option<String>,
        #[serde(default, skip_serializing_if = "OwnerReplacement::is_default")]
        pub owner_replacement: OwnerReplacement,
        pub map: String,
        pub frame: AuthoredFrame,
        pub schedule: AuthoredSchedule,
        /// Lowest candidate channel, never an assigned channel or wire GUID.
        pub first_channel: u32,
        pub bindings: Vec<AuthoredBinding>,
    }
}
/// Authored transport policy, independent of original owner replacement semantics.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerReplacement {
    #[default]
    RetireBridge,
    AfterCloseAck,
}

impl OwnerReplacement {
    fn is_default(&self) -> bool {
        *self == Self::RetireBridge
    }
}

fn net_incarnation(
    policy: OwnerReplacement,
    source: Option<(Entity, u64)>,
) -> Option<NetIncarnation> {
    if policy == OwnerReplacement::AfterCloseAck {
        source.map(|(_, incarnation)| NetIncarnation(incarnation))
    } else {
        None
    }
}

object_dto! {
    pub struct AuthoredFrame {
        /// Output axes select signed native axes: 1=x, 2=y, 3=z.
        pub axes: [i8; 3],
        pub units: f32,
    }
}
object_dto! {
    pub struct AuthoredSchedule {
        pub connection_budget: usize,
        pub max_consider_list: usize,
        pub max_connections: usize,
        pub max_viewers_per_connection: usize,
        pub net_server_max_tick_rate: i32,
        pub max_connections_to_tick_per_server_frame: i32,
        pub relevant_timeout_seconds: f64,
        pub spawn_priority_seconds: f32,
        pub use_adaptive_net_update_frequency: bool,
        pub seed: u64,
    }
}
object_dto! {
    pub struct AuthoredBinding {
        pub placement: String,
        pub archetype: Vec<AuthoredPath>,
        pub parameters: AuthoredParameters,
        pub replicate_movement: bool,
        #[serde(default)]
        pub flags: Option<AuthoredFlags>,
    }
}
object_dto! {
    pub struct AuthoredPath {
        pub path: String,
        pub checksum: u32,
        pub no_load: bool,
    }
}
object_dto! {
    pub struct AuthoredParameters {
        pub net_cull_distance_squared: f32,
        pub net_update_frequency: f32,
        pub min_net_update_frequency: f32,
        pub net_priority: f32,
    }
}
object_dto! {
    pub struct AuthoredFlags {
        pub hidden: bool,
        pub can_be_damaged: bool,
    }
}

impl AuthoredReplication {
    /// JSON only. Unknown fields, duplicate fields and positional objects reject.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_DECLARATION_BYTES,
            "native replication byte limit exceeded"
        );
        let declaration: Self = serde_json::from_str(text)
            .map_err(|error| crate::json_parse_error("native replication JSON", error))?;
        declaration.validate()?;
        Ok(declaration)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .context("open native replication declaration")?;
        let metadata = file
            .metadata()
            .context("native replication declaration metadata")?;
        ensure!(
            metadata.is_file(),
            "native replication input must be a regular file"
        );
        ensure!(
            metadata.len() <= MAX_DECLARATION_BYTES as u64,
            "native replication byte limit exceeded"
        );
        let mut text = String::new();
        file.take(MAX_DECLARATION_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .context("read native replication declaration")?;
        Self::parse(&text)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            match self.format {
                1 =>
                    self.lifecycle_id.is_none()
                        && self.owner_replacement == OwnerReplacement::RetireBridge,
                2 => self.lifecycle_id.as_ref().is_some_and(|id| {
                    id.len() == 64
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }),
                _ => false,
            },
            "native replication format or lifecycle identity invalid"
        );
        ensure!(
            self.catalog_id.len() == 64
                && self
                    .catalog_id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "native replication catalog identity invalid"
        );
        string(&self.map)?;
        self.frame.validate()?;
        self.schedule.validate()?;
        ensure!(
            (1..=MAX_BINDINGS).contains(&self.bindings.len()),
            "native replication binding limit exceeded"
        );
        let mut placements = BTreeSet::new();
        let mut path_identities = BTreeMap::new();
        for binding in &self.bindings {
            string(&binding.placement)?;
            ensure!(
                placements.insert(&binding.placement),
                "native replication duplicate placement"
            );
            ensure!(
                (2..=MAX_PATH_DEPTH).contains(&binding.archetype.len()),
                "native replication path depth invalid"
            );
            ensure!(
                binding.archetype[0].checksum != 0
                    && binding
                        .archetype
                        .last()
                        .is_some_and(|segment| segment.checksum == 0),
                "native replication archetype checksum chain invalid"
            );
            let mut paths = BTreeSet::new();
            for (index, segment) in binding.archetype.iter().enumerate() {
                string(&segment.path)?;
                ensure!(
                    paths.insert(&segment.path),
                    "native replication duplicate path segment"
                );
                let key: Vec<_> = binding.archetype[index..]
                    .iter()
                    .map(|part| part.path.as_str())
                    .collect();
                let identity = (segment.checksum, segment.no_load);
                ensure!(
                    path_identities
                        .insert(key, identity)
                        .is_none_or(|previous| previous == identity),
                    "native replication path identity conflict"
                );
                let package = index + 1 == binding.archetype.len();
                ensure!(
                    if package {
                        segment.path.starts_with('/')
                            && segment.path.split('/').skip(1).all(path_part)
                    } else {
                        path_part(&segment.path)
                    },
                    "native replication path chain invalid"
                );
            }
            binding.parameters.validate()?;
        }
        // Apply the same cap to programmatic declarations; strings and counts
        // were bounded before serialization allocates this scratch.
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_DECLARATION_BYTES,
            "native replication byte limit exceeded"
        );
        Ok(())
    }
}

fn string(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_STRING_BYTES
            && !value.chars().any(char::is_control),
        "native replication string invalid"
    );
    Ok(())
}

fn path_part(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl AuthoredFrame {
    fn validate(&self) -> Result<()> {
        let mut axes = self.axes.map(i8::unsigned_abs);
        axes.sort_unstable();
        ensure!(
            axes == [1, 2, 3],
            "native replication axes must be a signed permutation"
        );
        ensure!(
            self.units.is_finite() && self.units > 0.0,
            "native replication unit scale invalid"
        );
        Ok(())
    }

    fn position(&self, pose: &NativeTransform) -> Result<[f32; 3]> {
        self.position_after(pose, None)
    }

    /// `accepted` is a position this frame produced earlier. The codec check
    /// is a pure function of its input, so a bit-identical result is not rechecked.
    fn position_after(
        &self,
        pose: &NativeTransform,
        accepted: Option<[f32; 3]>,
    ) -> Result<[f32; 3]> {
        ensure!(
            pose.rotation == [0.0, 0.0, 0.0, 1.0] && pose.scale == [1.0; 3],
            "native replication v1 requires identity rotation and unit native scale"
        );
        let position = self.axes.map(|axis| {
            let value = f64::from(pose.translation[usize::from(axis.unsigned_abs() - 1)])
                * f64::from(self.units)
                * f64::from(axis.signum());
            value as f32
        });
        ensure!(
            position.iter().all(|v| v.is_finite()),
            "native replication transformed position invalid"
        );
        // Use the existing property quantizer even when movement is disabled:
        // later edits must not introduce a position this contract cannot send.
        if accepted.is_none_or(|accepted| !same_bits(accepted, position)) {
            authored_movement(position, [0.0; 3], [0.0; 3])
                .map_err(|_| anyhow::anyhow!("native replication position outside codec limits"))?;
        }
        Ok(position)
    }
}

fn same_bits(a: [f32; 3], b: [f32; 3]) -> bool {
    a.map(f32::to_bits) == b.map(f32::to_bits)
}

impl AuthoredParameters {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.net_cull_distance_squared.is_finite()
                && self.net_cull_distance_squared > 0.0
                && self.net_update_frequency.is_finite()
                && self.net_update_frequency > 0.0
                && self.net_update_frequency <= 10_000.0
                && self.min_net_update_frequency.is_finite()
                && self.min_net_update_frequency > 0.0
                && self.min_net_update_frequency <= self.net_update_frequency
                && self.net_priority.is_finite()
                && self.net_priority > 0.0
                && self.net_priority <= 10_000.0,
            "native replication parameters invalid"
        );
        Ok(())
    }

    fn parameters(&self) -> NetParameters {
        NetParameters {
            net_cull_distance_squared: self.net_cull_distance_squared,
            net_update_frequency: self.net_update_frequency,
            min_net_update_frequency: self.min_net_update_frequency,
            net_priority: self.net_priority,
        }
    }
}

impl AuthoredSchedule {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_BINDINGS).contains(&self.connection_budget)
                && (1..=MAX_BINDINGS).contains(&self.max_consider_list)
                && (1..=64).contains(&self.max_connections)
                && (1..=4).contains(&self.max_viewers_per_connection)
                && (1..=1000).contains(&self.net_server_max_tick_rate)
                && (0..=64).contains(&self.max_connections_to_tick_per_server_frame)
                && self.relevant_timeout_seconds.is_finite()
                && self.relevant_timeout_seconds > 0.0
                && self.relevant_timeout_seconds <= 3600.0
                && self.spawn_priority_seconds.is_finite()
                && self.spawn_priority_seconds > 0.0
                && self.spawn_priority_seconds <= 3600.0,
            "native replication schedule invalid"
        );
        Ok(())
    }

    fn config(&self) -> ReplicationConfig {
        ReplicationConfig {
            spatial: SpatialMode::Naive,
            connection_budget: self.connection_budget,
            max_consider_list: self.max_consider_list,
            max_connections: self.max_connections,
            max_viewers_per_connection: self.max_viewers_per_connection,
            net_server_max_tick_rate: self.net_server_max_tick_rate,
            max_connections_to_tick_per_server_frame: self.max_connections_to_tick_per_server_frame,
            relevant_timeout_seconds: self.relevant_timeout_seconds,
            spawn_priority_seconds: self.spawn_priority_seconds,
            use_adaptive_net_update_frequency: self.use_adaptive_net_update_frequency,
            seed: self.seed,
        }
    }
}

impl AuthoredFlags {
    fn flags(&self) -> NetActorFlags {
        NetActorFlags {
            hidden: self.hidden,
            can_be_damaged: self.can_be_damaged,
        }
    }
}

enum Activation {
    Spatial(NativeWorldContext),
    Lifecycle(Box<NativeLifecycleContext>),
}

struct SourceSlot<'a> {
    id: DomainId,
    live: Option<(Entity, u64)>,
    placement: Cow<'a, str>,
    class: Cow<'a, str>,
    tile: Cow<'a, str>,
    transform: NativeTransform,
}

impl Activation {
    /// Preserve the spatial borrowed path. Lifecycle capture validates the entire
    /// live/dormant world once, never once per binding.
    fn slots<'a>(&self, world: &'a World) -> Result<Vec<SourceSlot<'a>>> {
        match self {
            Self::Spatial(pin) => Ok(replication_actors(world, pin)?
                .into_iter()
                .map(|(entity, actor)| SourceSlot {
                    id: actor.id,
                    live: Some((entity, 0)),
                    placement: actor.placement.into(),
                    class: actor.class.into(),
                    tile: actor.tile.into(),
                    transform: actor.transform,
                })
                .collect()),
            Self::Lifecycle(pin) => Ok(replication_slots(world, pin)?
                .into_iter()
                .map(|slot| SourceSlot {
                    id: slot.id,
                    live: slot.live,
                    placement: slot.placement.into(),
                    class: slot.class.into(),
                    tile: slot.tile.into(),
                    transform: slot.transform,
                })
                .collect()),
        }
    }
}

struct Binding {
    id: DomainId,
    source: Option<(Entity, u64)>,
    placement: String,
    class: String,
    tile: String,
    pose: NativeTransform,
    position: [f32; 3],
    actor: NetActor,
    archetype: NetArchetype,
    flags: Option<NetActorFlags>,
    proxy: Option<Entity>,
}

/// No runtime entity or input strings are exposed through Debug/diagnostics.
pub struct PreparedReplication {
    activation: Activation,
    declaration: AuthoredReplication,
    session: SessionReplication,
    bindings: Vec<Binding>,
}

/// Aggregate diagnostics only. Transport errors remain in PeerLab statistics.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NativeReplicationCounts {
    pub activated: usize,
    pub rejected: usize,
    pub binding_errors: u64,
    pub active: bool,
}

#[derive(Resource)]
struct ActiveReplication {
    plan: PreparedReplication,
    failed: bool,
}

/// Transient identity, deliberately distinct from the persistent DomainId
/// component. Native capture rejects extra domain entities.
#[derive(Component)]
struct NativeProxy(DomainId);

/// Native edits in Gameplay (including deferred edits) are visible before this
/// set; consumers may explicitly place their edit producer before this set.
#[derive(SystemSet, Clone, Debug, Hash, PartialEq, Eq)]
pub struct NativeReplicationSet;

fn ensure_uninstalled(world: &World) -> Result<()> {
    ensure!(
        !world.contains_resource::<ActiveReplication>()
            && !world.contains_resource::<SessionReplication>()
            && !world.contains_resource::<ReplicationConfig>()
            && !world.contains_resource::<ReplicationState>()
            && !world.contains_resource::<ReplicationFrame>(),
        "native replication resources conflict"
    );
    for entity in world.iter_entities() {
        ensure!(
            !entity.contains::<NetActor>()
                && !entity.contains::<NetArchetype>()
                && !entity.contains::<NetMotion>()
                && !entity.contains::<NetIncarnation>()
                && !entity.contains::<NetActorFlags>()
                && !entity.contains::<NetViewer>()
                && !entity.contains::<NativeProxy>(),
            "native replication components conflict"
        );
    }
    Ok(())
}

/// Prepare the entire declaration without modifying world/lab resources,
/// components, GUIDs, channels or export state. Session level/version come from
/// the selected production session plan; the declaration cannot override them.
pub fn prepare(
    world: &mut World,
    declaration: AuthoredReplication,
    mut session: SessionReplication,
    max_channels: u32,
) -> Result<PreparedReplication> {
    declaration.validate()?;
    ensure_uninstalled(world)?;
    ensure!(
        declaration.first_channel >= session.first_channel
            && declaration.first_channel > 0
            && declaration.first_channel < max_channels,
        "native replication channel floor invalid or reserved"
    );
    session.first_channel = declaration.first_channel;
    ensure!(
        (2..=MAX_PATH_DEPTH).contains(&session.level.len()),
        "native replication session path depth invalid"
    );
    for segment in &session.level {
        string(&segment.path)?;
    }
    ensure!(
        session.engine_net_version > 0 && session.viewer_location.iter().all(|v| v.is_finite()),
        "native replication session parameters invalid"
    );
    {
        let mut identities = BTreeMap::new();
        for (index, segment) in session.level.iter().enumerate() {
            identities.insert(
                session.level[index..]
                    .iter()
                    .map(|part| part.path.as_str())
                    .collect::<Vec<_>>(),
                (segment.checksum, segment.no_load),
            );
        }
        for binding in &declaration.bindings {
            for (index, segment) in binding.archetype.iter().enumerate() {
                let key = binding.archetype[index..]
                    .iter()
                    .map(|part| part.path.as_str())
                    .collect::<Vec<_>>();
                let identity = (segment.checksum, segment.no_load);
                ensure!(
                    identities
                        .insert(key, identity)
                        .is_none_or(|previous| previous == identity),
                    "native replication session path identity conflict"
                );
            }
        }
    }
    let snapshot = capture(world)?;
    let activation = match declaration.format {
        1 => {
            ensure!(
                snapshot.schema_version == SNAPSHOT_SCHEMA_NATIVE
                    && snapshot.preserved_prospect.is_none(),
                "native replication requires authored schema 4 without preserved state"
            );
            let pin = world
                .get_resource::<NativeWorldContext>()
                .context("native replication activation missing")?
                .clone();
            let (catalog, map) = pin.replication_identity();
            ensure!(
                catalog == declaration.catalog_id && map == declaration.map,
                "native replication catalog or map binding mismatch"
            );
            Activation::Spatial(pin)
        }
        2 => {
            ensure!(
                snapshot.preserved_prospect.is_none(),
                "native replication preserved state unsupported"
            );
            let pin = world
                .get_resource::<NativeLifecycleContext>()
                .context("native replication lifecycle activation missing")?
                .clone();
            let (catalog, map, lifecycle) = pin.replication_identity();
            ensure!(
                catalog == declaration.catalog_id
                    && map == declaration.map
                    && Some(lifecycle) == declaration.lifecycle_id.as_deref(),
                "native replication lifecycle binding mismatch"
            );
            Activation::Lifecycle(Box::new(pin))
        }
        _ => unreachable!("declaration validated"),
    };
    let actors = activation.slots(world)?;
    let by_placement: BTreeMap<_, _> = actors
        .iter()
        .map(|actor| (actor.placement.as_ref(), actor))
        .collect();
    let mut bindings = Vec::with_capacity(declaration.bindings.len());
    for authored in &declaration.bindings {
        let native = by_placement
            .get(authored.placement.as_str())
            .context("native replication placement missing")?;
        let position = declaration.frame.position(&native.transform)?;
        let archetype = NetArchetype {
            archetype: authored
                .archetype
                .iter()
                .map(|segment| PathSegment::new(&segment.path, segment.checksum, segment.no_load))
                .collect(),
            replicate_movement: authored.replicate_movement,
        };
        let flags = authored.flags.as_ref().map(AuthoredFlags::flags);
        validate_open(native.id, position, &archetype, flags, &session)?;
        bindings.push(Binding {
            id: native.id,
            source: native.live,
            placement: native.placement.to_string(),
            class: native.class.to_string(),
            tile: native.tile.to_string(),
            pose: native.transform.clone(),
            position,
            actor: NetActor {
                id: native.id,
                params: authored.parameters.parameters(),
            },
            archetype,
            flags,
            proxy: None,
        });
    }
    bindings.sort_by_key(|binding| binding.id);
    ensure!(
        bindings.windows(2).all(|pair| pair[0].id != pair[1].id),
        "native replication duplicate domain binding"
    );
    Ok(PreparedReplication {
        activation,
        declaration,
        session,
        bindings,
    })
}

fn validate_open(
    id: DomainId,
    position: [f32; 3],
    archetype: &NetArchetype,
    flags: Option<NetActorFlags>,
    session: &SessionReplication,
) -> Result<()> {
    let movement = archetype
        .replicate_movement
        .then(|| authored_movement(position, [0.0; 3], [0.0; 3]))
        .transpose()
        .map_err(|_| anyhow::anyhow!("native replication movement invalid"))?;
    let block = world_actor_state_block(session.engine_net_version, movement, flags)
        .map_err(|_| anyhow::anyhow!("native replication state block invalid"))?;
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        let spec = ActorOpenSpec {
            actor: cache.actor_guid(ActorKey::World(id.0))?,
            archetype: archetype.archetype.clone(),
            level: session.level.clone(),
            location: Some(position),
            rotation: None,
            velocity: None,
            blocks: vec![block.clone()],
        };
        compose_actor_open(
            &mut cache,
            &mut exports,
            &spec,
            session.first_channel,
            session.engine_net_version,
            order,
        )
        .map_err(|_| anyhow::anyhow!("native replication composed open invalid"))?;
    }
    Ok(())
}

struct Projection {
    position: [f32; 3],
    stale: bool,
    source: Option<(Entity, u64)>,
}

/// Stage all bindings before publishing any change, including dormant activation.
fn current_positions(
    world: &World,
    plan: &PreparedReplication,
    startup: bool,
) -> Result<Vec<Projection>> {
    ensure!(
        !world.contains_resource::<crate::simulation::PreservedProspectState>()
            && !world.contains_resource::<crate::simulation::C001InertImportState>(),
        "native replication source mode changed"
    );
    // Full validation visits the private native registry in BTreeMap key order,
    // so this vector is strictly ordered by domain ID, including unbound actors.
    let actors = plan.activation.slots(world)?;
    let mut positions = Vec::with_capacity(plan.bindings.len());
    for binding in &plan.bindings {
        let index = actors
            .binary_search_by_key(&binding.id, |actor| actor.id)
            .ok()
            .context("native replication binding lost")?;
        let actor = &actors[index];
        ensure!(
            (actor.live == binding.source
                || (!startup && binding.source.is_none() && actor.live.is_some())
                || (!startup
                    && plan.declaration.owner_replacement == OwnerReplacement::AfterCloseAck
                    && matches!((binding.source, actor.live), (Some((old_entity, old_inc)), Some((entity, inc))) if old_entity != entity && old_inc < inc)))
                && actor.placement == binding.placement
                && actor.class == binding.class
                && actor.tile == binding.tile,
            "native replication binding changed"
        );
        ensure!(
            !startup || actor.transform == binding.pose,
            "native replication prepared pose changed"
        );
        // binding.position is always a position this frame accepted earlier.
        let position = plan
            .declaration
            .frame
            .position_after(&actor.transform, Some(binding.position))?;
        if startup || position != binding.position {
            validate_open(
                binding.id,
                position,
                &binding.archetype,
                binding.flags,
                &plan.session,
            )?;
        }
        let mut stale = false;
        if let Some(proxy) = binding.proxy {
            let proxy = world.get_entity(proxy).ok();
            let current = proxy.and_then(|proxy| proxy.get::<Position>());
            ensure!(
                proxy.is_some_and(|proxy| {
                    proxy
                        .get::<NativeProxy>()
                        .is_some_and(|proxy| proxy.0 == binding.id)
                        && proxy.get::<NetActor>() == Some(&binding.actor)
                        && proxy.get::<NetArchetype>() == Some(&binding.archetype)
                        && proxy.contains::<NetSchedule>()
                        && current == Some(&Position(binding.position))
                        && proxy.get::<NetActorFlags>().copied() == binding.flags
                        && !proxy.contains::<NetMotion>()
                        && proxy.get::<NetIncarnation>().copied()
                            == net_incarnation(plan.declaration.owner_replacement, binding.source)
                        && !proxy.contains::<bevy::ecs::entity_disabling::Disabled>()
                }),
                "native replication proxy lost or changed"
            );
            stale = current.is_none_or(|current| !same_bits(current.0, position));
        }
        ensure!(
            actor
                .live
                .and_then(|(entity, _)| world.get_entity(entity).ok())
                .is_none_or(|source| {
                    !source.contains::<NetActor>() && !source.contains::<NetArchetype>()
                }),
            "native replication source components conflict"
        );
        positions.push(Projection {
            position,
            stale,
            source: actor.live,
        });
    }
    if !startup {
        let session = world
            .get_resource::<SessionReplication>()
            .context("native replication session binding lost")?;
        ensure!(
            session.level == plan.session.level
                && session.engine_net_version == plan.session.engine_net_version
                && session.first_channel == plan.session.first_channel
                && session.viewer_location == plan.session.viewer_location,
            "native replication session binding changed"
        );
        // Bound proxies passed the loop above: each carries NetActor and its
        // binding's unique NativeProxy ID, so they are distinct carriers. Equal
        // counts then mean no other entity, including a Disabled or otherwise
        // filtered one, carries a bridge component. An unregistered type has no
        // carrier; IDs are resolved from this world on every call.
        let components = [
            world.component_id::<NetActor>(),
            world.component_id::<NetArchetype>(),
            world.component_id::<NetMotion>(),
            world.component_id::<NetIncarnation>(),
            world.component_id::<NetActorFlags>(),
            world.component_id::<NativeProxy>(),
        ];
        let carriers: usize = world
            .archetypes()
            .iter()
            .filter(|archetype| {
                components
                    .iter()
                    .flatten()
                    .any(|id| archetype.contains(*id))
            })
            .map(|archetype| archetype.entities().len())
            .sum();
        let proxies = plan
            .bindings
            .iter()
            .filter(|binding| binding.proxy.is_some())
            .count();
        ensure!(carriers == proxies, "native replication topology changed");
    }
    Ok(positions)
}

/// Publish only after revalidating the activation and every staged pose. Called
/// by network::install_configured, the same installation path used by `run`.
pub fn install(app: &mut App, mut plan: PreparedReplication) -> Result<()> {
    ensure_uninstalled(app.world())?;
    ensure!(
        !app.is_plugin_added::<ReplicationPlugin>()
            && !app.is_plugin_added::<SessionReplicationPlugin>(),
        "native replication plugins conflict"
    );
    let snapshot = capture(app.world_mut())?;
    ensure!(
        snapshot.preserved_prospect.is_none(),
        "native replication preserved state unsupported"
    );
    // Revalidate the pinned mode, whole source world and staged membership.
    current_positions(app.world(), &plan, true)?;
    let mut count = 0;
    for binding in &mut plan.bindings {
        if binding.source.is_some() {
            spawn_proxy(app.world_mut(), binding, plan.declaration.owner_replacement);
            count += 1;
        }
    }
    app.add_plugins((
        ReplicationPlugin {
            config: plan.declaration.schedule.config(),
        },
        SessionReplicationPlugin,
    ))
    .insert_resource(plan.session.clone())
    .insert_resource(ActiveReplication {
        plan,
        failed: false,
    })
    .insert_resource(NativeReplicationCounts {
        activated: count,
        active: true,
        ..Default::default()
    })
    .configure_sets(
        FixedUpdate,
        NativeReplicationSet
            .after(SimulationSet::Gameplay)
            .after(crate::session_replication::SessionReplicationSet::Viewers)
            .before(ReplicationSet),
    )
    .add_systems(FixedUpdate, extract.in_set(NativeReplicationSet));
    Ok(())
}

fn spawn_proxy(world: &mut World, binding: &mut Binding, policy: OwnerReplacement) {
    let mut proxy = world.spawn((
        NativeProxy(binding.id),
        binding.actor,
        Position(binding.position),
        binding.archetype.clone(),
    ));
    if let Some(flags) = binding.flags {
        proxy.insert(flags);
    }
    if let Some(incarnation) = net_incarnation(policy, binding.source) {
        proxy.insert(incarnation);
    }
    binding.proxy = Some(proxy.id());
}

/// Explicit authored flag edits reuse the bridge's per-field transport. No
/// native variable or class default supplies these values.
pub fn update_flags(world: &mut World, id: DomainId, flags: Option<AuthoredFlags>) -> Result<()> {
    let (proxy, flags) = {
        let state = world
            .get_resource::<ActiveReplication>()
            .context("native replication inactive")?;
        ensure!(!state.failed, "native replication failed closed");
        current_positions(world, &state.plan, false)?;
        let binding = state
            .plan
            .bindings
            .iter()
            .find(|binding| binding.id == id)
            .context("native replication flag binding missing")?;
        (binding.proxy, flags.as_ref().map(AuthoredFlags::flags))
    };
    if let Some(proxy) = proxy {
        let mut entity = world.entity_mut(proxy);
        if let Some(flags) = flags {
            entity.insert(flags);
        } else {
            entity.remove::<NetActorFlags>();
        }
    }
    let mut state = world.resource_mut::<ActiveReplication>();
    if let Some(binding) = state
        .plan
        .bindings
        .iter_mut()
        .find(|binding| binding.id == id)
    {
        binding.flags = flags;
    }
    Ok(())
}

fn retire(world: &mut World, plan: &PreparedReplication) {
    if let Some(mut lab) = world.get_resource_mut::<PeerLab>() {
        lab.stop_admission();
        if plan.declaration.owner_replacement == OwnerReplacement::AfterCloseAck {
            // An empty desired set suppresses every old update while closes drain.
            let _ = lab.reconcile_actor_incarnations(&BTreeMap::new());
        }
        for connection in lab.joined_connections() {
            for binding in &plan.bindings {
                // Existing explicit channel retirement; no replacement/despawn
                // semantics are inferred. Framing pressure is retried by lab.
                lab.close_actor(connection, binding.id.0, 3);
            }
        }
        lab.retry_pending_closes();
    }
}

fn remove_proxies(world: &mut World, plan: &PreparedReplication) {
    for binding in &plan.bindings {
        if let Some(proxy) = binding.proxy {
            world.despawn(proxy);
        }
    }
}

fn extract(world: &mut World) {
    let Some(mut state) = world.remove_resource::<ActiveReplication>() else {
        return;
    };
    if world
        .get_resource::<PeerLab>()
        .is_none_or(|lab| !lab.is_admitting() && lab.peer_count() == 0)
    {
        remove_proxies(world, &state.plan);
        world.remove_resource::<SessionReplication>();
        crate::session_replication::clear_viewers(world);
        if let Some(mut counts) = world.get_resource_mut::<NativeReplicationCounts>() {
            counts.active = false;
        }
        return;
    }
    if !state.failed {
        match current_positions(world, &state.plan, false) {
            Ok(positions) => {
                let mut activated = 0;
                for (binding, projection) in state.plan.bindings.iter_mut().zip(positions) {
                    if projection.stale
                        && let Some(proxy) = binding.proxy
                    {
                        world
                            .entity_mut(proxy)
                            .insert(Position(projection.position));
                    }
                    binding.position = projection.position;
                    let was_dormant = binding.source.is_none();
                    let replaced = binding.source != projection.source;
                    binding.source = projection.source;
                    if was_dormant && binding.source.is_some() {
                        spawn_proxy(world, binding, state.plan.declaration.owner_replacement);
                        activated += 1;
                    } else if replaced
                        && let Some(proxy) = binding.proxy
                        && let Some(incarnation) = net_incarnation(
                            state.plan.declaration.owner_replacement,
                            binding.source,
                        )
                    {
                        world.entity_mut(proxy).insert(incarnation);
                    }
                }
                if let Some(mut counts) = world.get_resource_mut::<NativeReplicationCounts>() {
                    counts.activated += activated;
                }
            }
            Err(_) => {
                state.failed = true;
                remove_proxies(world, &state.plan);
                world.remove_resource::<SessionReplication>();
                crate::session_replication::clear_viewers(world);
                if let Some(mut counts) = world.get_resource_mut::<NativeReplicationCounts>() {
                    counts.active = false;
                    counts.rejected = counts.activated;
                    counts.binding_errors = counts.binding_errors.saturating_add(1);
                }
            }
        }
    }
    if state.failed {
        retire(world, &state.plan);
    }
    world.insert_resource(state);
}

/// Idempotent host teardown. Network teardown then drops all transport state;
/// explicit removal while the lab lives keeps a failed plan to retry closes.
pub fn teardown(world: &mut World) {
    if !world.contains_resource::<ActiveReplication>()
        && !world.contains_resource::<NativeReplicationCounts>()
    {
        return;
    }
    if let Some(mut state) = world.remove_resource::<ActiveReplication>() {
        remove_proxies(world, &state.plan);
        retire(world, &state.plan);
        state.failed = true;
        if world.contains_resource::<PeerLab>() {
            world.insert_resource(state);
        }
    }
    world.remove_resource::<SessionReplication>();
    crate::session_replication::clear_viewers(world);
    if let Some(mut counts) = world.get_resource_mut::<NativeReplicationCounts>() {
        counts.active = false;
    }
}
