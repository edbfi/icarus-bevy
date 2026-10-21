//! Native composition of a dynamic actor's open with its components
//! (`F-794`, `F-797`).
//!
//! The original serializes an actor open in one order, and assigns GUIDs and
//! writes exports in that order: the actor and its spawn (archetype, level),
//! then the actor block's references in handle order, then one block per
//! component, then one block per runtime subobject. So:
//!
//! - a stably named component referenced by a property gets its GUID and
//!   export at that property; the others at their own block;
//! - a referenced actor gets a GUID on first reference and is never
//!   exported (its own open spawns it);
//! - a runtime subobject (created by the actor, not by its archetype) gets a
//!   GUID at its block, whose header names its class; the class is exported.
//!
//! Exports follow [`ConnectionExports`] (sent until acked, `F-796`). GUIDs
//! of keyed actors, components and subobjects are driver-wide, so every
//! connection receives the same objects. The caller supplies identifiers and
//! content (values and encoded blocks); nothing here reads original files.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::actor_channel::{
    BlockTarget, ContentBlock, DynamicSpawn, NetGuidRef, SpawnRecord, SubObjectClass,
};
use super::packet::{BitWriter, Payload};
use super::rep_layout::{
    PropertyStream, RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue, encode_rep_properties,
};
use super::session_open::{
    ActorKey, ComponentBlock, ComponentExport, ComposedActorOpen, ConnectionExports, NetGuidCache,
    PathSegment, actor_codec, actor_open_bunch, entry, exports_of, quantize_spawn_vector,
};

/// What an object property of the actor block references.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    /// A static object, by its outer chain (exported until acked).
    Static(Vec<PathSegment>),
    /// One of the actor's own components, by index into
    /// [`ComponentActorOpen::components`].
    Component(usize),
    /// A keyed actor: its driver-wide GUID, never exported.
    Actor(ActorKey),
    /// The opened actor itself (`Instigator` of a pawn, for example).
    Itself,
    /// An object whose GUID the caller already holds (a PlayerController).
    Guid(u32),
}

/// A content block's encoded properties: `has_rep_layout` and the payload
/// the replicator reads (`F-576`); an empty payload without a rep layout
/// creates the object only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockContent {
    pub has_rep_layout: bool,
    pub payload: Payload,
}

impl BlockContent {
    /// A block that only creates its object.
    pub fn create_only() -> Self {
        Self {
            has_rep_layout: false,
            payload: Payload::from_writer(BitWriter::new()),
        }
    }
}

/// A stably named component of the actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenComponent {
    pub name: String,
    /// Its class network checksum (`F-776`).
    pub checksum: u32,
    /// Its block; `None` writes none (the component is then serialized only
    /// if a property references it).
    pub block: Option<BlockContent>,
}

/// A runtime subobject of the actor: its class chain and its block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSubobject {
    pub class: Vec<PathSegment>,
    pub block: BlockContent,
}

/// One dynamic actor open with its components and runtime subobjects.
#[derive(Clone, Debug)]
pub struct ComponentActorOpen {
    pub key: ActorKey,
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    /// The spawn location (quantized like `WritePackedVector<10, 24>`).
    pub location: Option<[f32; 3]>,
    /// The actor's rep layout.
    pub layout: Vec<RepCommand>,
    /// Object properties of the actor block, by handle.
    pub references: BTreeMap<u32, Reference>,
    /// The other actor-block values, by handle; disjoint from `references`.
    pub values: BTreeMap<u32, RepValue>,
    pub components: Vec<OpenComponent>,
    pub subobjects: Vec<OpenSubobject>,
    pub engine_net_version: u32,
}

/// GUIDs a composed open used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentActorGuids {
    pub actor: u32,
    /// Per component, `None` when it was not serialized.
    pub components: Vec<Option<u32>>,
    pub subobjects: Vec<u32>,
}

/// Compose `open` for one connection on `channel_index` in the connection's
/// byte `order`. Fails before any assignment on an invalid reference or a
/// handle given both a reference and a value.
pub fn compose_component_actor_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    open: &ComponentActorOpen,
    channel_index: u32,
    order: super::archive::ByteOrder,
) -> Result<(ComposedActorOpen, ComponentActorGuids)> {
    for (handle, reference) in &open.references {
        ensure!(
            !open.values.contains_key(handle),
            "handle {handle} has both a reference and a value"
        );
        if let Reference::Component(index) = reference {
            ensure!(
                *index < open.components.len(),
                "handle {handle} references component {index} of {}",
                open.components.len()
            );
        }
    }
    let codec = actor_codec(open.engine_net_version, order);
    let actor = cache.actor_guid(open.key);
    exports.begin_bunch();
    let archetype = cache.export_static(exports, &open.archetype)?;
    let level = cache.export_static(exports, &open.level)?;
    let mut exported = vec![archetype.clone(), level.clone()];
    let mut component_guids: Vec<Option<u32>> = vec![None; open.components.len()];
    let mut component = |cache: &mut NetGuidCache,
                         exports: &mut ConnectionExports,
                         exported: &mut Vec<NetGuidRef>,
                         index: usize|
     -> Result<u32> {
        if let Some(guid) = component_guids[index] {
            return Ok(guid);
        }
        let item = &open.components[index];
        let reference = cache.export_keyed_subobject(
            exports,
            open.key,
            actor,
            &ComponentExport {
                name: item.name.clone(),
                checksum: item.checksum,
                block: ComponentBlock::None,
            },
        )?;
        component_guids[index] = Some(reference.guid);
        let guid = reference.guid;
        exported.push(reference);
        Ok(guid)
    };
    let mut values = open.values.clone();
    for (handle, reference) in &open.references {
        let guid = match reference {
            Reference::Static(chain) => {
                let reference = cache.export_static(exports, chain)?;
                let guid = reference.guid;
                exported.push(reference);
                guid
            }
            Reference::Component(index) => component(cache, exports, &mut exported, *index)?,
            Reference::Actor(key) => cache.actor_guid(*key),
            Reference::Itself => actor,
            Reference::Guid(guid) => *guid,
        };
        values.insert(*handle, RepValue::Object(NetGuidRef::plain(guid)));
    }
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream = PropertyStream {
        entries: values
            .into_iter()
            .map(|(handle, value)| entry(handle, value))
            .collect(),
        terminator_checksum: None,
    };
    let mut blocks = vec![ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&stream, &open.layout, &rep_codec)?,
    }];
    for (index, item) in open.components.iter().enumerate() {
        let Some(content) = &item.block else {
            continue;
        };
        let guid = component(cache, exports, &mut exported, index)?;
        blocks.push(ContentBlock {
            has_rep_layout: content.has_rep_layout,
            target: BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::StablyNamed,
            },
            payload: content.payload.clone(),
        });
    }
    let mut subobject_guids = Vec::with_capacity(open.subobjects.len());
    for (index, item) in open.subobjects.iter().enumerate() {
        let class = cache.export_static(exports, &item.class)?;
        let guid = cache.subobject_guid(open.key, u32::try_from(index)?);
        subobject_guids.push(guid);
        blocks.push(ContentBlock {
            has_rep_layout: item.block.has_rep_layout,
            target: BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::Class(NetGuidRef::plain(class.guid)),
            },
            payload: item.block.payload.clone(),
        });
        exported.push(class);
    }
    let spawn = SpawnRecord {
        actor: NetGuidRef::plain(actor),
        dynamic: Some(DynamicSpawn {
            archetype: NetGuidRef::plain(archetype.guid),
            level: Some(NetGuidRef::plain(level.guid)),
            location: open.location.map(quantize_spawn_vector).transpose()?,
            rotation: None,
            scale: None,
            velocity: None,
        }),
    };
    let composed = ComposedActorOpen {
        exports: exports_of(&exported, &codec)?,
        bunch: actor_open_bunch(spawn, blocks, channel_index, &codec)?,
    };
    Ok((
        composed,
        ComponentActorGuids {
            actor,
            components: component_guids,
            subobjects: subobject_guids,
        },
    ))
}
