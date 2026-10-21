//! Native composition of the server's first post-Join NetGUID exports.
//!
//! The original's first bunch after Join opens the PlayerController channel;
//! its first partial part carries an export prefix (`F-778`):
//!
//! - the archetype (`Default__<Class>_C`, its class network checksum) with its
//!   package as outer (checksum `0`) and the null GUID above that;
//! - the level (`PersistentLevel`, world, map package), each `no_load`, `0`;
//! - every replicated component in serialization order: a dynamic GUID with
//!   its name, `no_load` and its class checksum, whose outer is the actor's
//!   dynamic GUID written without a path but with `no_load` and the bare
//!   checksum flag (`F-692`).
//!
//! GUIDs follow a deterministic policy that reproduces the capture's numbering:
//! static GUIDs are odd and assigned leaf-first along an outer chain, dynamic
//! GUIDs are even and assigned in serialization order, and each connection
//! exports a given static path once. Identifiers and checksums are caller
//! data (per release); nothing here reads original files. Checksums come from
//! [`super::class_checksum`].
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};

use super::actor_channel::{
    ActorChannelCodec, BlockTarget, ContentBlock, DynamicSpawn, ExportPath, GuidExport, NetGuidRef,
    PlayerControllerOpenBunchBody, SpawnRecord, SpawnVector, SubObjectClass, write_export_prefix,
};
use super::control_messages::ArchiveText;
use super::packet::{BitWriter, Payload};
use super::rep_layout::{
    PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepValue, encode_rep_properties,
};

/// One segment of a static object's outer chain, leaf first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSegment {
    pub path: String,
    /// The class network checksum for objects; `0` for packages and levels.
    pub checksum: u32,
    pub no_load: bool,
}

impl PathSegment {
    pub fn new(path: &str, checksum: u32, no_load: bool) -> Self {
        Self {
            path: path.to_owned(),
            checksum,
            no_load,
        }
    }
}

/// What a component contributes to the open body (release data, F-778).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ComponentBlock {
    /// No content block (referenced by property only).
    #[default]
    None,
    /// A stably named subobject block without properties.
    CreateOnly,
    /// A stably named subobject block with these `UActorComponent` bool handles.
    RepBools(Vec<(u32, bool)>),
}

/// A replicated component of the PlayerController, in serialization order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentExport {
    pub name: String,
    pub checksum: u32,
    pub block: ComponentBlock,
}

/// Release-specific identifiers for the PlayerController open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerControllerProfile {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    pub components: Vec<ComponentExport>,
}

/// Driver-wide NetGUID assignment, shared by every connection.
#[derive(Clone, Debug, Default)]
pub struct NetGuidCache {
    static_index: u32,
    dynamic_index: u32,
    statics: BTreeMap<Vec<String>, u32>,
    /// Dynamic GUIDs of keyed world objects: an actor `(key, None)` or one
    /// of its components `(key, Some(name))`.
    keyed: BTreeMap<(u64, Option<String>), u32>,
}

/// Which GUIDs one connection has already received with their paths.
#[derive(Clone, Debug, Default)]
pub struct ConnectionExports {
    exported: BTreeSet<u32>,
}

impl ConnectionExports {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, guid: u32) -> bool {
        self.exported.contains(&guid)
    }
}

/// GUIDs assigned while composing a PlayerController open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerControllerGuids {
    pub actor: u32,
    pub archetype: u32,
    pub level: u32,
    pub player_state: u32,
    pub components: Vec<u32>,
}

impl NetGuidCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Next dynamic GUID (even, from 2).
    pub fn assign_dynamic(&mut self) -> u32 {
        self.dynamic_index += 1;
        self.dynamic_index << 1
    }

    /// The driver-wide dynamic GUID of the world actor `key`, assigned on
    /// first use: the original gives an actor its GUID when it is first
    /// serialized, whether by its own open or by a reference from another
    /// actor's properties (the GameState's references precede the opens of
    /// the actors they name, F-794).
    pub fn actor_guid(&mut self, key: u64) -> u32 {
        self.keyed_guid(key, None)
    }

    fn keyed_guid(&mut self, key: u64, component: Option<&str>) -> u32 {
        let entry = (key, component.map(str::to_owned));
        if let Some(guid) = self.keyed.get(&entry) {
            return *guid;
        }
        let guid = self.assign_dynamic();
        self.keyed.insert(entry, guid);
        guid
    }

    /// A component of the keyed world actor `key` (actor GUID `actor`): its
    /// driver-wide GUID, assigned on first use, exported with its name under
    /// the actor the first time `exports`' connection sees it and referenced
    /// plainly afterwards (F-794).
    pub fn export_keyed_subobject(
        &mut self,
        exports: &mut ConnectionExports,
        key: u64,
        actor: u32,
        component: &ComponentExport,
    ) -> Result<NetGuidRef> {
        let guid = self.keyed_guid(key, Some(&component.name));
        if exports.contains(guid) {
            return Ok(NetGuidRef::plain(guid));
        }
        let reference = subobject_export(guid, actor, component)?;
        exports.exported.insert(guid);
        Ok(reference)
    }

    /// The static GUID of the leaf of `chain` (leaf first), assigning
    /// unassigned segments leaf-first (odd, from 3).
    pub fn assign_static(&mut self, chain: &[PathSegment]) -> u32 {
        let mut leaf = 0;
        for depth in 0..chain.len() {
            let key: Vec<String> = chain[depth..].iter().map(|s| s.path.clone()).collect();
            let guid = match self.statics.get(&key) {
                Some(guid) => *guid,
                None => {
                    self.static_index += 1;
                    let guid = (self.static_index << 1) | 1;
                    self.statics.insert(key, guid);
                    guid
                }
            };
            if depth == 0 {
                leaf = guid;
            }
        }
        leaf
    }

    /// The reference for a static chain: a full export (with outers) the first
    /// time `exports`' connection sees the leaf, a plain reference afterwards.
    pub fn export_static(
        &mut self,
        exports: &mut ConnectionExports,
        chain: &[PathSegment],
    ) -> Result<NetGuidRef> {
        let guid = self.assign_static(chain);
        if !exports.exported.insert(guid) {
            return Ok(NetGuidRef::plain(guid));
        }
        let segment = chain.first().context("empty static chain")?;
        let outer = if chain.len() > 1 {
            self.export_static(exports, &chain[1..])?
        } else {
            NetGuidRef::null()
        };
        Ok(NetGuidRef {
            guid,
            export: Some(GuidExport {
                no_load: segment.no_load,
                path: Some(ExportPath {
                    outer: Box::new(outer),
                    path: ArchiveText::text(&segment.path)?,
                }),
                checksum: Some(segment.checksum),
                bare_checksum_flag: false,
            }),
        })
    }

    /// A component (subobject) export under a dynamic actor GUID.
    pub fn export_subobject(
        &mut self,
        exports: &mut ConnectionExports,
        actor: u32,
        component: &ComponentExport,
    ) -> Result<(u32, NetGuidRef)> {
        let guid = self.assign_dynamic();
        exports.exported.insert(guid);
        Ok((guid, subobject_export(guid, actor, component)?))
    }

    /// Register the level chain at world start, as the original does before
    /// any player joins.
    pub fn register_level(&mut self, level: &[PathSegment]) -> u32 {
        self.assign_static(level)
    }

    /// Compose the export references of the PlayerController open in the
    /// original's serialization order: actor, archetype, level, the player
    /// state (dynamic, not exported), then the components.
    pub fn player_controller_exports(
        &mut self,
        exports: &mut ConnectionExports,
        profile: &PlayerControllerProfile,
    ) -> Result<(PlayerControllerGuids, Vec<NetGuidRef>)> {
        let actor = self.assign_dynamic();
        let archetype_ref = self.export_static(exports, &profile.archetype)?;
        let level_ref = self.export_static(exports, &profile.level)?;
        let player_state = self.assign_dynamic();
        let mut references = vec![archetype_ref.clone(), level_ref.clone()];
        let mut components = Vec::with_capacity(profile.components.len());
        for component in &profile.components {
            let (guid, reference) = self.export_subobject(exports, actor, component)?;
            components.push(guid);
            references.push(reference);
        }
        Ok((
            PlayerControllerGuids {
                actor,
                archetype: archetype_ref.guid,
                level: level_ref.guid,
                player_state,
                components,
            },
            references,
        ))
    }
}

/// The export of component `guid` under the dynamic actor `actor`: its name,
/// `no_load` and class checksum, with the actor as a pathless outer carrying
/// `no_load` and the bare checksum flag (F-692, F-778).
fn subobject_export(guid: u32, actor: u32, component: &ComponentExport) -> Result<NetGuidRef> {
    let outer = NetGuidRef {
        guid: actor,
        export: Some(GuidExport {
            no_load: true,
            path: None,
            checksum: None,
            bare_checksum_flag: true,
        }),
    };
    Ok(NetGuidRef {
        guid,
        export: Some(GuidExport {
            no_load: true,
            path: Some(ExportPath {
                outer: Box::new(outer),
                path: ArchiveText::text(&component.name)?,
            }),
            checksum: Some(component.checksum),
            bare_checksum_flag: false,
        }),
    })
}

/// Encode an export-only prefix (`ExportNetGUIDHeader` + `ExportNetGUID`).
pub fn encode_exports(references: &[NetGuidRef], codec: &ActorChannelCodec) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_export_prefix(&mut writer, references, codec)?;
    Ok(Payload::from_writer(writer))
}

/// The export-only payload of the references that carry an export, in
/// order, or `None` when the connection already has every path.
pub(super) fn exports_of(
    references: &[NetGuidRef],
    codec: &ActorChannelCodec,
) -> Result<Option<Payload>> {
    let exported: Vec<NetGuidRef> = references
        .iter()
        .filter(|reference| reference.export.is_some())
        .cloned()
        .collect();
    if exported.is_empty() {
        return Ok(None);
    }
    encode_exports(&exported, codec).map(Some)
}

/// `WritePackedVector<10, 24>`: each component rounded at a scale of 10; the
/// bit count is `CeilLogTwo(1 + max |v|) - 1` (capped at 24), biased by
/// `1 << (bits + 1)` (F-571, F-573).
pub fn quantize_spawn_vector(value: [f32; 3]) -> anyhow::Result<SpawnVector> {
    anyhow::ensure!(
        value.iter().all(|v| v.is_finite()),
        "spawn vector must be finite"
    );
    let scaled = value.map(|v| (v * 10.0 + 0.5).floor() as i64);
    let max = scaled.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
    // Bits needed for max |v|, i.e. CeilLogTwo(1 + max); checked before any cast.
    let ceil_log2 = 64 - max.leading_zeros();
    anyhow::ensure!(
        ceil_log2 <= 24,
        "spawn vector exceeds the 24-bit packed range"
    );
    let bits = ceil_log2.max(1) - 1;
    let bias = 1i64 << (bits + 1);
    let components = scaled.map(|v| (v + bias) as u32);
    Ok(SpawnVector::Quantized { bits, components })
}

/// Server-chosen values of a PlayerController open (F-778).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerControllerOpenState {
    pub spawn_location: [f32; 3],
    pub net_player_index: u8,
    /// `ENetRole` as seen by the owning client (`ROLE_AutonomousProxy` = 2).
    pub remote_role: u8,
    /// `ROLE_Authority` = 3.
    pub role: u8,
}

/// Handles of the PlayerController layout this composer writes (F-778).
const HANDLE_REMOTE_ROLE: u32 = 5;
const HANDLE_ROLE: u32 = 14;
/// `AActor::ReplicatedMovement` (F-703).
const HANDLE_REPLICATED_MOVEMENT: u32 = 6;
const HANDLE_PLAYER_STATE: u32 = 16;
const HANDLE_SPAWN_LOCATION: u32 = 19;
/// The first `COMPONENT_HANDLES` components (`BackendProxyComponent`,
/// `PlayerDataComponent`, `BestiaryManagerComponent` in 3.0.31) are also
/// referenced by actor properties; later components only get their own
/// export and block (F-778).
const HANDLE_FIRST_COMPONENT: u32 = 22;
pub const COMPONENT_HANDLES: usize = 3;

pub(super) fn entry(handle: u32, value: RepValue) -> RepEntry {
    RepEntry {
        handle,
        checksum: None,
        value,
    }
}

/// Compose the PlayerController open body: spawn record, `NetPlayerIndex`,
/// the actor's initial properties and the component blocks, using `profile`
/// for component block kinds and the caller's rep layouts.
pub fn compose_player_controller_open_body(
    guids: &PlayerControllerGuids,
    profile: &PlayerControllerProfile,
    state: &PlayerControllerOpenState,
    actor_layout: &[RepCommand],
    component_layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<PlayerControllerOpenBunchBody> {
    anyhow::ensure!(
        guids.components.len() == profile.components.len(),
        "component GUIDs and profile components differ in count"
    );
    anyhow::ensure!(
        profile.components.len() >= COMPONENT_HANDLES,
        "the PlayerController references {COMPONENT_HANDLES} components by property"
    );
    let spawn = SpawnRecord {
        actor: NetGuidRef::plain(guids.actor),
        dynamic: Some(DynamicSpawn {
            archetype: NetGuidRef::plain(guids.archetype),
            level: Some(NetGuidRef::plain(guids.level)),
            location: Some(quantize_spawn_vector(state.spawn_location)?),
            rotation: None,
            scale: None,
            velocity: None,
        }),
    };
    let mut entries = vec![
        entry(HANDLE_REMOTE_ROLE, RepValue::Byte(state.remote_role)),
        entry(HANDLE_ROLE, RepValue::Byte(state.role)),
        entry(
            HANDLE_PLAYER_STATE,
            RepValue::Object(NetGuidRef::plain(guids.player_state)),
        ),
        entry(
            HANDLE_SPAWN_LOCATION,
            RepValue::Vector(state.spawn_location.map(f32::to_bits)),
        ),
    ];
    for (offset, guid) in guids.components[..COMPONENT_HANDLES].iter().enumerate() {
        entries.push(entry(
            HANDLE_FIRST_COMPONENT + offset as u32,
            RepValue::Object(NetGuidRef::plain(*guid)),
        ));
    }
    let actor_stream = PropertyStream {
        entries,
        terminator_checksum: None,
    };
    let mut blocks = vec![ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&actor_stream, actor_layout, codec)?,
    }];
    for (component, guid) in profile.components.iter().zip(&guids.components) {
        let target = BlockTarget::SubObject {
            object: NetGuidRef::plain(*guid),
            class: SubObjectClass::StablyNamed,
        };
        match &component.block {
            ComponentBlock::None => {}
            ComponentBlock::CreateOnly => blocks.push(ContentBlock {
                has_rep_layout: false,
                target,
                payload: Payload::from_writer(BitWriter::new()),
            }),
            ComponentBlock::RepBools(values) => {
                let stream = PropertyStream {
                    entries: values
                        .iter()
                        .map(|(handle, flag)| entry(*handle, RepValue::Bool(*flag)))
                        .collect(),
                    terminator_checksum: None,
                };
                blocks.push(ContentBlock {
                    has_rep_layout: true,
                    target,
                    payload: encode_rep_properties(&stream, component_layout, codec)?,
                });
            }
        }
    }
    Ok(PlayerControllerOpenBunchBody {
        spawn,
        net_player_index: state.net_player_index,
        blocks,
    })
}

/// `NAME_Actor`, the hard-coded channel name of actor channels (F-778).
pub const ACTOR_CHANNEL_NAME_INDEX: u32 = 102;
/// The capture's PlayerController channel index (F-778): the first after the
/// control (0) and voice (1) channels.
pub const PLAYER_CONTROLLER_CHANNEL_INDEX: u32 = 2;
/// The PlayerState's channel: the next free index (a lab choice; the capture
/// opened three other actors first and used 6, F-784).
pub const PLAYER_STATE_CHANNEL_INDEX: u32 = 3;

/// The level chain of a map package (`/Game/Maps/.../Name`, URL options
/// ignored): `PersistentLevel`, the world (the package's short name) and the
/// package, each `no_load` with checksum `0` (F-778).
pub fn map_level_chain(map_url: &str) -> Result<Vec<PathSegment>> {
    let package = map_url.split('?').next().unwrap_or_default();
    let world = package.rsplit('/').next().unwrap_or_default();
    anyhow::ensure!(
        package.starts_with('/') && !world.is_empty() && package.is_ascii(),
        "map URL does not name a map package"
    );
    Ok(vec![
        PathSegment::new("PersistentLevel", 0, true),
        PathSegment::new(world, 0, true),
        PathSegment::new(package, 0, true),
    ])
}

/// Everything a session needs to open a joined player's PlayerController:
/// release identifiers with the map's level chain, the rep layouts and codec
/// parameters, and lab-chosen state.
#[derive(Clone, Debug)]
pub struct PlayerControllerOpenPlan {
    pub profile: PlayerControllerProfile,
    pub actor_layout: Vec<RepCommand>,
    pub component_layout: Vec<RepCommand>,
    pub engine_net_version: u32,
    /// Server-chosen spawn location (no world geometry is consulted).
    pub spawn_location: [f32; 3],
    pub channel_index: u32,
}

/// One composed open: the export-only payload and the open bunch (its
/// reliable sequence still unset), ready for `UChannel::SendBunch` splitting.
#[derive(Clone, Debug)]
pub struct ComposedOpen {
    pub guids: PlayerControllerGuids,
    pub exports: Payload,
    pub bunch: super::packet::Bunch,
}

/// Compose the PlayerController open for one connection (F-778, F-779): GUID
/// assignment in `cache`, per-connection export state in `exports`, then the
/// export prefix and the body in the connection's byte `order`.
pub fn compose_player_controller_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PlayerControllerOpenPlan,
    net_player_index: u8,
    order: super::archive::ByteOrder,
) -> Result<ComposedOpen> {
    use super::actor_channel::{ActorChannelLimits, encode_player_controller_open_bunch_body};
    use super::packet::{Bunch, BunchHeader, Name};
    use super::rep_layout::RepLayoutLimits;
    let codec = ActorChannelCodec {
        engine_net_version: plan.engine_net_version,
        exports_enabled: false,
        has_level: true,
        order,
        limits: ActorChannelLimits::default(),
    };
    let (guids, references) = cache.player_controller_exports(exports, &plan.profile)?;
    let export_payload = encode_exports(&references, &codec)?;
    let state = PlayerControllerOpenState {
        spawn_location: plan.spawn_location,
        net_player_index,
        remote_role: 2,
        role: 3,
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let body = compose_player_controller_open_body(
        &guids,
        &plan.profile,
        &state,
        &plan.actor_layout,
        &plan.component_layout,
        &rep_codec,
    )?;
    let payload = encode_player_controller_open_bunch_body(&body, &codec)?;
    let bunch = Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index: plan.channel_index,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
        },
        payload,
    };
    Ok(ComposedOpen {
        guids,
        exports: export_payload,
        bunch,
    })
}

/// The PlayerState actor block's handles in the original's first open (F-784):
/// `RemoteRole`, `Owner`, `Role`, `PlayerId`, `UniqueId`, `PlayerNamePrivate`
/// and `AIcarusPlayerState::bIsHost`.
const PS_HANDLE_REMOTE_ROLE: u32 = 5;
const PS_HANDLE_OWNER: u32 = 13;
const PS_HANDLE_ROLE: u32 = 14;
const PS_HANDLE_PLAYER_ID: u32 = 17;
const PS_HANDLE_UNIQUE_ID: u32 = 25;
const PS_HANDLE_PLAYER_NAME: u32 = 26;
const PS_HANDLE_IS_HOST: u32 = 79;
/// `ROLE_SimulatedProxy` and `ROLE_Authority`, as captured (F-784).
const PS_REMOTE_ROLE: u8 = 1;
const PS_ROLE: u8 = 3;

/// Everything a session needs to open a joined player's PlayerState.
#[derive(Clone, Debug)]
pub struct PlayerStateOpenPlan {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    pub layout: Vec<RepCommand>,
    pub engine_net_version: u32,
    pub channel_index: u32,
}

/// Server-side PlayerState values for one player (F-784).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerStateOpenState {
    /// The PlayerState's dynamic GUID (assigned with the PlayerController).
    pub player_state: u32,
    /// The owning PlayerController's dynamic GUID.
    pub owner: u32,
    pub player_id: i32,
    pub unique_id: super::control_messages::UniqueNetId,
    pub player_name: ArchiveText,
    pub is_host: bool,
}

/// One composed actor open: the export-only payload (absent when every path
/// was already exported to the connection) and the open bunch.
#[derive(Clone, Debug)]
pub struct ComposedActorOpen {
    pub exports: Option<Payload>,
    pub bunch: super::packet::Bunch,
}

/// The type a server replicates a login's unique net id under
/// (`FUniqueNetIdRepl::NetSerialize`): a named type that has a replication
/// index is written by index (`NULL` -> 1 in the capture, F-636, F-784);
/// every other form is kept.
pub fn replicated_unique_net_id(
    login: &super::control_messages::UniqueNetId,
    indexed: &[(u8, &str)],
) -> super::control_messages::UniqueNetId {
    use super::control_messages::NetIdType;
    let kind = match &login.kind {
        NetIdType::Named(name) => {
            let text = String::from_utf16_lossy(name.units());
            indexed
                .iter()
                .find(|(_, known)| known.eq_ignore_ascii_case(&text))
                .map_or_else(
                    || login.kind.clone(),
                    |(index, _)| NetIdType::Indexed(*index),
                )
        }
        other => other.clone(),
    };
    super::control_messages::UniqueNetId {
        kind,
        value: login.value.clone(),
    }
}

/// Compose the PlayerState open for one connection (F-784): static exports
/// not yet sent to this connection, a dynamic spawn without location, and one
/// actor block with the captured handles.
pub fn compose_player_state_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PlayerStateOpenPlan,
    state: &PlayerStateOpenState,
    order: super::archive::ByteOrder,
) -> Result<ComposedActorOpen> {
    use super::rep_layout::RepLayoutLimits;
    let stream = PropertyStream {
        entries: vec![
            entry(PS_HANDLE_REMOTE_ROLE, RepValue::Byte(PS_REMOTE_ROLE)),
            entry(
                PS_HANDLE_OWNER,
                RepValue::Object(NetGuidRef::plain(state.owner)),
            ),
            entry(PS_HANDLE_ROLE, RepValue::Byte(PS_ROLE)),
            entry(PS_HANDLE_PLAYER_ID, RepValue::Int(state.player_id)),
            entry(
                PS_HANDLE_UNIQUE_ID,
                RepValue::UniqueNetId(state.unique_id.clone()),
            ),
            entry(
                PS_HANDLE_PLAYER_NAME,
                RepValue::Str(state.player_name.clone()),
            ),
            entry(PS_HANDLE_IS_HOST, RepValue::Bool(state.is_host)),
        ],
        terminator_checksum: None,
    };
    let rep_codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, order),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let spec = ActorOpenSpec {
        actor: state.player_state,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: None,
        blocks: vec![ContentBlock {
            has_rep_layout: true,
            target: BlockTarget::Actor,
            payload: encode_rep_properties(&stream, &plan.layout, &rep_codec)?,
        }],
    };
    compose_actor_open(
        cache,
        exports,
        &spec,
        plan.channel_index,
        plan.engine_net_version,
        order,
    )
}

/// The actor-channel codec the opens use: exports outside the body, levels
/// present, the connection's byte order.
pub(super) fn actor_codec(
    engine_net_version: u32,
    order: super::archive::ByteOrder,
) -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version,
        exports_enabled: false,
        has_level: true,
        order,
        limits: super::actor_channel::ActorChannelLimits::default(),
    }
}

/// A dynamic actor to open on a channel: its GUID, archetype and level chains,
/// an optional quantized location, and its initial content blocks.
#[derive(Clone, Debug)]
pub struct ActorOpenSpec {
    /// The actor's dynamic GUID. [`super::peer_lab::PeerLab::open_actor`]
    /// ignores the caller's value and assigns its driver-wide GUID.
    pub actor: u32,
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    pub location: Option<[f32; 3]>,
    pub blocks: Vec<ContentBlock>,
}

/// Compose a dynamic actor's open for one connection, as the original's
/// PlayerState open is framed (F-784): exports for the static paths the
/// connection has not received, the spawn record (location quantized like
/// the PlayerController's, F-779) and the content blocks.
pub fn compose_actor_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    spec: &ActorOpenSpec,
    channel_index: u32,
    engine_net_version: u32,
    order: super::archive::ByteOrder,
) -> Result<ComposedActorOpen> {
    let codec = actor_codec(engine_net_version, order);
    let archetype = cache.export_static(exports, &spec.archetype)?;
    let level = cache.export_static(exports, &spec.level)?;
    let export_payload = exports_of(&[archetype.clone(), level.clone()], &codec)?;
    let spawn = SpawnRecord {
        actor: NetGuidRef::plain(spec.actor),
        dynamic: Some(DynamicSpawn {
            archetype: NetGuidRef::plain(archetype.guid),
            level: Some(NetGuidRef::plain(level.guid)),
            location: spec.location.map(quantize_spawn_vector).transpose()?,
            rotation: None,
            scale: None,
            velocity: None,
        }),
    };
    let bunch = actor_open_bunch(spawn, spec.blocks.clone(), channel_index, &codec)?;
    Ok(ComposedActorOpen {
        exports: export_payload,
        bunch,
    })
}

/// The open bunch of a dynamic actor: control, open and reliable on
/// `channel_index` under `NAME_Actor`, its reliable sequence still unset,
/// carrying `spawn` and `blocks` (F-778, F-784).
pub(super) fn actor_open_bunch(
    spawn: SpawnRecord,
    blocks: Vec<ContentBlock>,
    channel_index: u32,
    codec: &ActorChannelCodec,
) -> Result<super::packet::Bunch> {
    use super::actor_channel::{ActorBunchBody, encode_actor_bunch_body};
    use super::packet::{Bunch, BunchHeader, Name};
    let body = ActorBunchBody {
        spawn: Some(spawn),
        blocks,
    };
    Ok(Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
        },
        payload: encode_actor_bunch_body(&body, codec)?,
    })
}

/// `ROLE_SimulatedProxy` and `ROLE_Authority`: the roles 286 of the capture's
/// 287 actor opens carry (F-791). Independent of the PlayerState constants,
/// which match them by capture (F-784).
const WORLD_REMOTE_ROLE: u8 = 1;
const WORLD_ROLE: u8 = 3;

/// The initial actor block of a world actor's open: `RemoteRole =
/// ROLE_SimulatedProxy` (handle 5) and `Role = ROLE_Authority` (handle 14),
/// as 286 of the capture's 287 actor opens carry (the other is the joining
/// player's own pawn, `RemoteRole = 2`; F-791). Encoded against the `AActor`
/// handle prefix, which every class layout shares (F-703); handles are packed.
/// Both values are single bytes, so the block is the same in either byte
/// order and one encoding serves every connection.
pub fn actor_role_block(engine_net_version: u32) -> Result<ContentBlock> {
    world_actor_block(engine_net_version, None)
}

/// `AActor::ReplicatedMovement` (handle 6) of a resting actor at `location`:
/// no flags, zero rotation (every byte-rotator axis absent) and zero linear
/// velocity, quantized at the `AActor` defaults the opens are encoded with
/// (whole-number location and velocity, byte rotation; F-588, F-589).
pub fn resting_movement(location: [f32; 3]) -> Result<super::rep_layout::RepMovement> {
    use super::rep_layout::{ByteRotator, MovementRotation, RepMovement, quantize_vector};
    Ok(RepMovement {
        flags: 0,
        location: quantize_vector(location, super::rep_layout::MovementLevel::Whole.packing())?,
        rotation: Some(MovementRotation::Bytes(ByteRotator {
            pitch: None,
            yaw: None,
            roll: None,
        })),
        linear_velocity: quantize_vector(
            [0.0; 3],
            super::rep_layout::MovementLevel::Whole.packing(),
        )?,
        angular_velocity: None,
    })
}

/// An actor block on the `AActor` handle prefix (F-703): the world-actor
/// roles (F-791) when `roles`, and `ReplicatedMovement` (handle 6) when given.
/// Every value is bit-serialized, so the encoding does not depend on the
/// connection's byte order.
fn actor_handles_block(
    engine_net_version: u32,
    roles: bool,
    movement: Option<super::rep_layout::RepMovement>,
) -> Result<ContentBlock> {
    use super::rep_layout::{MovementLevel, RepLayoutLimits, RotationLevel, actor_rep_layout};
    let mut entries = Vec::new();
    if roles {
        entries.push(entry(HANDLE_REMOTE_ROLE, RepValue::Byte(WORLD_REMOTE_ROLE)));
    }
    if let Some(movement) = movement {
        entries.push(entry(
            HANDLE_REPLICATED_MOVEMENT,
            RepValue::Movement(movement),
        ));
    }
    if roles {
        entries.push(entry(HANDLE_ROLE, RepValue::Byte(WORLD_ROLE)));
    }
    let codec = RepLayoutCodec {
        actor: actor_codec(engine_net_version, super::archive::ByteOrder::Little),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let layout = actor_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    let stream = PropertyStream {
        entries,
        terminator_checksum: None,
    };
    Ok(ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&stream, &layout, &codec)?,
    })
}

/// A world actor's initial block: the roles (F-791), plus its resting
/// `ReplicatedMovement` when `movement` is given (30 of the capture's 287
/// opens carry handle 6).
pub fn world_actor_block(
    engine_net_version: u32,
    movement: Option<super::rep_layout::RepMovement>,
) -> Result<ContentBlock> {
    actor_handles_block(engine_net_version, true, movement)
}

/// A property update carrying only `ReplicatedMovement` (handle 6).
pub fn movement_update_block(
    engine_net_version: u32,
    movement: super::rep_layout::RepMovement,
) -> Result<ContentBlock> {
    actor_handles_block(engine_net_version, false, Some(movement))
}
