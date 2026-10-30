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
//!   GUID at its block, whose header names its class; the class is exported;
//! - a reference inside a component or subobject block resolves at that
//!   block, after its header.
//!
//! Exports follow [`ConnectionExports`] (sent until acked, `F-796`). GUIDs
//! of keyed actors, components and subobjects are driver-wide, so every
//! connection receives the same objects. The caller supplies identifiers and
//! content (values and encoded blocks); nothing here reads original files.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};

use super::actor_channel::{
    BlockTarget, ContentBlock, DynamicSpawn, NetGuidRef, SpawnRecord, SubObjectClass,
};
use super::control_messages::ArchiveText;
use super::packet::{BitWriter, Payload};
use super::rep_layout::{
    NetworkName, PropertyStream, RepArray, RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue,
    encode_rep_properties, write_rep_properties,
};
use super::rep_rpc::{FieldHeader, write_field_header};
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
    /// A dynamic array of keyed actors, resolved in element order.
    Actors(Vec<ActorKey>),
    /// A `(Str, Object)` dynamic array of keyed actors (`FRelevantQuestActor`).
    KeyedActors(Vec<(ArchiveText, ActorKey)>),
    /// A `(NetworkName, Object)` dynamic array of keyed actors (`FSubQuest`).
    NamedActors(Vec<(NetworkName, ActorKey)>),
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

/// One replicator field after a block's properties (a custom-delta
/// property such as a fast array, `F-708`): its index in the class's
/// `FClassNetCache` and its exact payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomField {
    pub index: u32,
    pub payload: Payload,
}

/// A block composed where it is serialized: its rep-layout values and
/// references, then its custom-delta fields. The property stream is written,
/// and `has_rep_layout` set, exactly when it has an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProperties {
    /// The object's rep layout (a prefix suffices for low handles).
    pub layout: Vec<RepCommand>,
    /// Object properties, by handle.
    pub references: BTreeMap<u32, Reference>,
    /// The other values, by handle; disjoint from `references`.
    pub values: BTreeMap<u32, RepValue>,
    /// Fields after the properties, in order.
    pub fields: Vec<CustomField>,
    /// The class's field-index bound (`FieldsBase + Fields.Num() + 1`,
    /// `F-708`); every field index is below it.
    pub field_bound: u32,
}

/// What a component or subobject block carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Already encoded by the caller.
    Encoded(BlockContent),
    /// Composed at its serialization point.
    Composed(BlockProperties),
}

impl From<BlockContent> for Block {
    fn from(content: BlockContent) -> Self {
        Self::Encoded(content)
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
    pub block: Option<Block>,
}

/// A runtime subobject of the actor: its class chain and its block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSubobject {
    pub class: Vec<PathSegment>,
    pub block: Block,
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

/// Validate one set of references and values against the components.
fn validate_references(
    references: &BTreeMap<u32, Reference>,
    values: &BTreeMap<u32, RepValue>,
    components: usize,
    what: &str,
) -> Result<()> {
    for (handle, reference) in references {
        ensure!(
            !values.contains_key(handle),
            "{what} handle {handle} has both a reference and a value"
        );
        match reference {
            Reference::Component(index) => {
                ensure!(
                    *index < components,
                    "{what} handle {handle} references component {index} of {components}"
                );
            }
            Reference::Actors(items) => {
                ensure!(
                    u16::try_from(items.len()).is_ok(),
                    "{what} handle {handle} array length {} exceeds u16::MAX",
                    items.len()
                );
            }
            Reference::KeyedActors(items) => {
                ensure!(
                    u16::try_from(items.len()).is_ok(),
                    "{what} handle {handle} array length {} exceeds u16::MAX",
                    items.len()
                );
            }
            Reference::NamedActors(items) => {
                ensure!(
                    u16::try_from(items.len()).is_ok(),
                    "{what} handle {handle} array length {} exceeds u16::MAX",
                    items.len()
                );
            }
            Reference::Static(_) | Reference::Actor(_) | Reference::Itself | Reference::Guid(_) => {
            }
        }
    }
    Ok(())
}

/// GUID assignment and export collection in serialization order.
struct Serializer<'a> {
    cache: &'a mut NetGuidCache,
    exports: &'a mut ConnectionExports,
    open: &'a ComponentActorOpen,
    actor: u32,
    exported: Vec<NetGuidRef>,
    components: Vec<Option<u32>>,
}

impl Serializer<'_> {
    /// A component's GUID, assigned and exported on first serialization.
    fn component(&mut self, index: usize) -> Result<u32> {
        if let Some(guid) = self.components[index] {
            return Ok(guid);
        }
        let item = &self.open.components[index];
        let reference = self.cache.export_keyed_subobject(
            self.exports,
            self.open.key,
            self.actor,
            &ComponentExport {
                name: item.name.clone(),
                checksum: item.checksum,
                block: ComponentBlock::None,
            },
        )?;
        self.components[index] = Some(reference.guid);
        let guid = reference.guid;
        self.exported.push(reference);
        Ok(guid)
    }

    fn static_object(&mut self, chain: &[PathSegment]) -> Result<u32> {
        let reference = self.cache.export_static(self.exports, chain)?;
        let guid = reference.guid;
        self.exported.push(reference);
        Ok(guid)
    }

    /// `values` with every reference resolved, in handle order.
    fn resolve(
        &mut self,
        references: &BTreeMap<u32, Reference>,
        values: &BTreeMap<u32, RepValue>,
    ) -> Result<BTreeMap<u32, RepValue>> {
        let mut values = values.clone();
        for (handle, reference) in references {
            let value = match reference {
                Reference::Static(chain) => {
                    RepValue::Object(NetGuidRef::plain(self.static_object(chain)?))
                }
                Reference::Component(index) => {
                    RepValue::Object(NetGuidRef::plain(self.component(*index)?))
                }
                Reference::Actor(key) => {
                    RepValue::Object(NetGuidRef::plain(self.cache.actor_guid(*key)?))
                }
                Reference::Actors(keys) => {
                    let entries = keys
                        .iter()
                        .zip(1u32..)
                        .map(|(key, item_handle)| {
                            Ok(entry(
                                item_handle,
                                RepValue::Object(NetGuidRef::plain(self.cache.actor_guid(*key)?)),
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    RepValue::DynamicArray(RepArray {
                        count: u16::try_from(keys.len())?,
                        stream: PropertyStream {
                            entries,
                            terminator_checksum: None,
                        },
                    })
                }
                Reference::KeyedActors(items) => {
                    let mut entries = Vec::with_capacity(2 * items.len());
                    for ((key, actor), index) in items.iter().zip(0u32..) {
                        let guid = self.cache.actor_guid(*actor)?;
                        entries.push(entry(2 * index + 1, RepValue::Str(key.clone())));
                        entries.push(entry(
                            2 * index + 2,
                            RepValue::Object(NetGuidRef::plain(guid)),
                        ));
                    }
                    RepValue::DynamicArray(RepArray {
                        count: u16::try_from(items.len())?,
                        stream: PropertyStream {
                            entries,
                            terminator_checksum: None,
                        },
                    })
                }
                Reference::NamedActors(items) => {
                    let mut entries = Vec::with_capacity(2 * items.len());
                    for ((name, actor), index) in items.iter().zip(0u32..) {
                        let guid = self.cache.actor_guid(*actor)?;
                        entries.push(entry(2 * index + 1, RepValue::NetworkName(name.clone())));
                        entries.push(entry(
                            2 * index + 2,
                            RepValue::Object(NetGuidRef::plain(guid)),
                        ));
                    }
                    RepValue::DynamicArray(RepArray {
                        count: u16::try_from(items.len())?,
                        stream: PropertyStream {
                            entries,
                            terminator_checksum: None,
                        },
                    })
                }
                Reference::Itself => RepValue::Object(NetGuidRef::plain(self.actor)),
                Reference::Guid(guid) => RepValue::Object(NetGuidRef::plain(*guid)),
            };
            values.insert(*handle, value);
        }
        Ok(values)
    }

    /// Encode a block after its header, resolving its references.
    fn block(&mut self, block: &Block, codec: &RepLayoutCodec) -> Result<BlockContent> {
        let properties = match block {
            Block::Encoded(content) => return Ok(content.clone()),
            Block::Composed(properties) => properties,
        };
        let values = self.resolve(&properties.references, &properties.values)?;
        let mut writer = BitWriter::new();
        let has_rep_layout = !values.is_empty();
        if has_rep_layout {
            write_rep_properties(&mut writer, &stream(values), &properties.layout, codec)?;
        }
        for field in &properties.fields {
            write_field_header(
                &mut writer,
                &FieldHeader {
                    index: field.index,
                    payload: field.payload.clone(),
                },
                properties.field_bound,
            )?;
        }
        Ok(BlockContent {
            has_rep_layout,
            payload: Payload::from_writer(writer),
        })
    }
}

fn stream(values: BTreeMap<u32, RepValue>) -> PropertyStream {
    PropertyStream {
        entries: values
            .into_iter()
            .map(|(handle, value)| entry(handle, value))
            .collect(),
        terminator_checksum: None,
    }
}

/// Compose `open` for one connection on `channel_index` in the connection's
/// byte `order`. Fails before any assignment on a duplicate component name,
/// an invalid reference or a handle given both a reference and a value.
pub fn compose_component_actor_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    open: &ComponentActorOpen,
    channel_index: u32,
    order: super::archive::ByteOrder,
) -> Result<(ComposedActorOpen, ComponentActorGuids)> {
    // A name keys one component GUID: two components with one name would
    // be one object with two blocks.
    let mut names = BTreeSet::new();
    for component in &open.components {
        ensure!(
            names.insert(component.name.as_str()),
            "duplicate component {}",
            component.name
        );
    }
    let count = open.components.len();
    validate_references(&open.references, &open.values, count, "actor")?;
    let nested = open
        .components
        .iter()
        .filter_map(|component| component.block.as_ref())
        .chain(open.subobjects.iter().map(|subobject| &subobject.block));
    for block in nested {
        if let Block::Composed(properties) = block {
            validate_references(&properties.references, &properties.values, count, "block")?;
        }
    }
    let codec = actor_codec(open.engine_net_version, order);
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let actor = cache.actor_guid(open.key)?;
    exports.begin_bunch();
    let mut serializer = Serializer {
        cache,
        exports,
        open,
        actor,
        exported: Vec::new(),
        components: vec![None; count],
    };
    let archetype = serializer
        .cache
        .export_static(serializer.exports, &open.archetype)?;
    let level = serializer
        .cache
        .export_static(serializer.exports, &open.level)?;
    serializer.exported = vec![archetype.clone(), level.clone()];
    let values = serializer.resolve(&open.references, &open.values)?;
    let mut blocks = vec![ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&stream(values), &open.layout, &rep_codec)?,
    }];
    for (index, item) in open.components.iter().enumerate() {
        let Some(block) = &item.block else {
            continue;
        };
        let guid = serializer.component(index)?;
        let content = serializer.block(block, &rep_codec)?;
        blocks.push(ContentBlock {
            has_rep_layout: content.has_rep_layout,
            target: BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::StablyNamed,
            },
            payload: content.payload,
        });
    }
    let mut subobject_guids = Vec::with_capacity(open.subobjects.len());
    for (index, item) in open.subobjects.iter().enumerate() {
        let class = serializer.static_object(&item.class)?;
        let guid = serializer
            .cache
            .subobject_guid(open.key, u32::try_from(index)?)?;
        subobject_guids.push(guid);
        let content = serializer.block(&item.block, &rep_codec)?;
        blocks.push(ContentBlock {
            has_rep_layout: content.has_rep_layout,
            target: BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::Class(NetGuidRef::plain(class)),
            },
            payload: content.payload,
        });
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
        exports: exports_of(&serializer.exported, &codec)?,
        bunch: actor_open_bunch(spawn, blocks, channel_index, &codec)?,
    };
    Ok((
        composed,
        ComponentActorGuids {
            actor,
            components: serializer.components,
            subobjects: subobject_guids,
        },
    ))
}
