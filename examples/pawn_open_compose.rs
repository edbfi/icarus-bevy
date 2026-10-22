//! Compose a pawn open with `experimental::actor_open` from a profile JSON
//! file and compare it with a supplied open body and export payload (one
//! JSON line). The profile carries identifiers, the GUIDs assigned before
//! the open, the object references by handle and the supplied body. The
//! actor block's other values and every component and subobject block's
//! content are decoded from that body; GUIDs, references, exports and
//! framing come from the composer. With `--native`, the open is composed by
//! `pawn_open::compose_pawn_open` from the built-in 3.0.31 pawn identifiers
//! instead, and only the session's values (`IcarusUID`, tags, character
//! state, stats, inventory rows and slots) are decoded from the body; with
//! `--data-pak` the character state, inventories and stats come from the
//! official server's data tables instead (F-836, F-839) and are compared
//! with the decoded ones. Reads no original file but that data.pak.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::ContentBlock,
    actor_channel::{
        ActorChannelCodec, ActorChannelLimits, BlockTarget, DynamicSpawn, NetGuidRef, SpawnVector,
        SubObjectClass, read_actor_bunch_body,
    },
    actor_open::{
        BlockContent, ComponentActorOpen, OpenComponent, OpenSubobject, Reference,
        compose_component_actor_open,
    },
    archive::ByteOrder,
    packet::{BitWriter, Payload},
    pawn_open::{
        EmptySlot, FastArrayContent, PawnContent, PawnInventory, PawnOpenState, compose_pawn_open,
    },
    release::ReleaseProfile,
    rep_layout::{
        GameplayTag, PropertyStream, RepCommand, inventory_rep_layout, inventory_slot_commands,
        player_character_state_rep_layout,
    },
    rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
        bp_icarus_player_character_survival_rep_layout, read_rep_properties,
    },
    rep_rpc::{
        FAST_ARRAY_COUNT_CAP, FastArrayDelta, FastArrayItem, FastArrayItemLayout,
        read_custom_delta_header, read_fast_array_delta, read_field_header,
    },
    session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON (see `ProfileFile`).
    profile: PathBuf,
    /// Compose with `compose_pawn_open` and the built-in identifiers.
    #[arg(long)]
    native: bool,
    /// With `--native`: take the character state, inventories and stats
    /// from this official server `data.pak` (F-836, F-839).
    #[arg(long, requires = "native")]
    data_pak: Option<PathBuf>,
}

/// How a handle's object reference is composed.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileReference {
    Component(usize),
    Itself,
    Guid(u32),
}

#[derive(Deserialize)]
struct ProfileFile {
    archetype: Vec<(String, u32, bool)>,
    level: Vec<(String, u32, bool)>,
    /// `[name, checksum]` of each component, in block order.
    components: Vec<(String, u32)>,
    /// The runtime subobjects' class chain.
    subobject_class: Vec<(String, u32, bool)>,
    references: BTreeMap<u32, ProfileReference>,
    /// Static GUIDs assigned after the level and before the archetype.
    burn_static: u32,
    /// Dynamic GUIDs assigned before the pawn.
    burn_dynamic: u32,
    /// Whether the client had acked the earlier exports.
    earlier_acked: bool,
    channel_index: u32,
    body_hex: String,
    body_bits: usize,
    exports_hex: String,
    exports_bits: usize,
}

fn segments(items: &[(String, u32, bool)]) -> Vec<PathSegment> {
    items
        .iter()
        .map(|(path, checksum, no_load)| PathSegment::new(path, *checksum, *no_load))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn payload(text: &str, bits: usize) -> Result<Payload> {
    ensure!(
        text.is_ascii() && text.len().is_multiple_of(2),
        "hex must be ASCII digit pairs"
    );
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .context("hex")?;
    ensure!(bits <= bytes.len() * 8, "bit count exceeds the hex");
    let mut writer = BitWriter::new();
    for index in 0..bits {
        writer.write_bit(bytes[index / 8] >> (index % 8) & 1 == 1);
    }
    Ok(Payload::from_writer(writer))
}

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let file: ProfileFile = serde_json::from_str(&text).context("parse profile")?;
    let release = ReleaseProfile::release_3_0_31();
    let codec = ActorChannelCodec {
        engine_net_version: release.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let layout = bp_icarus_player_character_survival_rep_layout(
        MovementLevel::Hundredths,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        release.tags.tag_encoding(),
    );
    let supplied_body = payload(&file.body_hex, file.body_bits)?;
    let supplied = read_actor_bunch_body(&mut supplied_body.reader(), true, &codec)?;
    let spawn = supplied.spawn.context("spawn")?;
    let dynamic = spawn.dynamic.as_ref().context("dynamic spawn")?;
    let location = dynamic
        .location
        .as_ref()
        .map(|_| spawn_location(dynamic))
        .transpose()?;
    let actor_block = supplied.blocks.first().context("actor block")?;
    ensure!(matches!(actor_block.target, BlockTarget::Actor));
    let stream = read_rep_properties(&mut actor_block.payload.reader(), &layout, &rep_codec)?;
    let mut values = BTreeMap::new();
    let mut supplied_references = BTreeMap::new();
    for entry in stream.entries {
        if file.references.contains_key(&entry.handle) {
            let RepValue::Object(NetGuidRef { guid, .. }) = entry.value else {
                bail!("handle {} is not an object", entry.handle);
            };
            supplied_references.insert(entry.handle, guid);
        } else {
            values.insert(entry.handle, entry.value);
        }
    }
    let mut components = Vec::new();
    let mut subobjects = Vec::new();
    let mut blocks = supplied.blocks.iter().skip(1);
    for (name, checksum) in &file.components {
        let block = blocks.next().context("component block")?;
        ensure!(
            matches!(
                &block.target,
                BlockTarget::SubObject {
                    class: SubObjectClass::StablyNamed,
                    ..
                }
            ),
            "component {name} block is not stably named"
        );
        components.push(OpenComponent {
            name: name.clone(),
            checksum: *checksum,
            block: Some(
                BlockContent {
                    has_rep_layout: block.has_rep_layout,
                    payload: block.payload.clone(),
                }
                .into(),
            ),
        });
    }
    for block in blocks {
        ensure!(
            matches!(
                &block.target,
                BlockTarget::SubObject {
                    class: SubObjectClass::Class(_),
                    ..
                }
            ),
            "trailing block is not a runtime subobject"
        );
        subobjects.push(OpenSubobject {
            class: segments(&file.subobject_class),
            block: BlockContent {
                has_rep_layout: block.has_rep_layout,
                payload: block.payload.clone(),
            }
            .into(),
        });
    }
    let references = file
        .references
        .iter()
        .map(|(handle, reference)| {
            (
                *handle,
                match reference {
                    ProfileReference::Component(index) => Reference::Component(*index),
                    ProfileReference::Itself => Reference::Itself,
                    ProfileReference::Guid(guid) => Reference::Guid(*guid),
                },
            )
        })
        .collect();
    let open = ComponentActorOpen {
        key: ActorKey::Session(1),
        archetype: segments(&file.archetype),
        level: segments(&file.level),
        location,
        layout,
        references,
        values: values.clone(),
        components,
        subobjects,
        engine_net_version: release.engine_network_version,
    };
    let (mut cache, mut exports) = prepared(&file, &open.level)?;
    if options.native {
        return native(
            &file,
            &release,
            &supplied.blocks,
            &values,
            &supplied_references,
            location,
            &rep_codec,
            (&supplied_body, &supplied_exports(&file)?),
            options.data_pak.as_deref(),
        );
    }
    let (composed, guids) = compose_component_actor_open(
        &mut cache,
        &mut exports,
        &open,
        file.channel_index,
        ByteOrder::Little,
    )?;
    let composed_exports = composed.exports.context("exports")?;
    let supplied_exports = supplied_exports(&file)?;
    let composed_references: BTreeMap<u32, u32> = {
        let body = read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &codec)?;
        read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &open.layout,
            &rep_codec,
        )?
        .entries
        .into_iter()
        .filter_map(|entry| match entry.value {
            RepValue::Object(reference) if file.references.contains_key(&entry.handle) => {
                Some((entry.handle, reference.guid))
            }
            _ => None,
        })
        .collect()
    };
    println!(
        "{}",
        json!({
            "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
            "exports_identical": composed_exports == supplied_exports,
            "body_bits": [composed.bunch.payload.bit_len(), supplied_body.bit_len()],
            "body_identical": composed.bunch.payload == supplied_body,
            "references_identical": composed_references == supplied_references,
            "actor": guids.actor,
            "components": guids.components,
            "subobjects": guids.subobjects,
            "composed_exports_hex": hex(composed_exports.bytes()),
        })
    );
    Ok(())
}

/// The spawn location a quantized spawn vector stands for: each component
/// is the packed value divided by the scale of 10 (F-571).
fn spawn_location(dynamic: &DynamicSpawn) -> Result<[f32; 3]> {
    match dynamic.location.as_ref().context("location")? {
        SpawnVector::Quantized { bits, components } => {
            let bias = 1i64 << (bits + 1);
            Ok(components.map(|value| (i64::from(value) - bias) as f32 / 10.0))
        }
        other => bail!("unexpected spawn vector {other:?}"),
    }
}

fn supplied_exports(file: &ProfileFile) -> Result<Payload> {
    payload(&file.exports_hex, file.exports_bits)
}

/// The GUID cache and exports as the capture had them before the open: the
/// level exported, static and dynamic GUIDs burned, acked as recorded.
fn prepared(
    file: &ProfileFile,
    level: &[PathSegment],
) -> Result<(NetGuidCache, ConnectionExports)> {
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, level)?;
    for index in 0..file.burn_static {
        cache.export_static(
            &mut exports,
            &[PathSegment::new(&format!("burn{index}"), 0, false)],
        )?;
    }
    for _ in 0..file.burn_dynamic {
        cache.assign_dynamic();
    }
    exports.commit_bunch(1);
    if file.earlier_acked {
        exports.received_ack(1);
    }
    Ok((cache, exports))
}

/// The one fast-array field a block ends with, after its optional properties.
fn fast_array(
    block: &ContentBlock,
    layout: &[RepCommand],
    element: Vec<RepCommand>,
    bound: u32,
    rep_codec: &RepLayoutCodec,
    engine_net_version: u32,
) -> Result<(Option<PropertyStream>, FastArrayDelta)> {
    let mut reader = block.payload.reader();
    let properties = block
        .has_rep_layout
        .then(|| read_rep_properties(&mut reader, layout, rep_codec))
        .transpose()?;
    let header = read_field_header(&mut reader, bound, rep_codec.limits.max_opaque_bits)?;
    ensure!(reader.at_end(), "one field per block");
    let mut field = header.payload.reader();
    read_custom_delta_header(&mut field, engine_net_version, 1)?;
    let delta = read_fast_array_delta(
        &mut field,
        &FastArrayItemLayout::Delta(element),
        rep_codec,
        FAST_ARRAY_COUNT_CAP,
    )?;
    ensure!(field.at_end(), "trailing bits in a fast-array delta");
    Ok((properties, delta))
}

fn handle_value(stream: &PropertyStream, handle: u32) -> Option<&RepValue> {
    stream
        .entries
        .iter()
        .find(|entry| entry.handle == handle)
        .map(|entry| &entry.value)
}

/// Compose with `compose_pawn_open` and the built-in 3.0.31 identifiers,
/// decoding only the session's values from the supplied body.
#[allow(clippy::too_many_arguments)]
fn native(
    file: &ProfileFile,
    release: &ReleaseProfile,
    supplied: &[ContentBlock],
    actor_values: &BTreeMap<u32, RepValue>,
    references: &BTreeMap<u32, u32>,
    location: Option<[f32; 3]>,
    rep_codec: &RepLayoutCodec,
    (supplied_body, supplied_exports): (&Payload, &Payload),
    data_pak: Option<&std::path::Path>,
) -> Result<()> {
    let mut plan = release
        .pawn_open_plan("/Game/Maps/Unused/Unused")?
        .context("built-in pawn identifiers")?;
    plan.level = segments(&file.level);
    let engine = release.engine_network_version;
    let mut content = PawnContent::default();
    for (handle, value) in actor_values {
        match (handle, value) {
            (5 | 14, _) => {}
            (46, RepValue::Int(uid)) => content.icarus_uid = Some(*uid),
            (50, RepValue::GameplayTagContainer(Some(tags))) => {
                content.gameplay_tags = tags
                    .iter()
                    .map(|tag| match tag {
                        GameplayTag::NetIndex(index) => Ok(*index),
                        GameplayTag::Name(_) => bail!("named tag"),
                    })
                    .collect::<Result<_>>()?;
            }
            (67, RepValue::Bool(true)) => content.frozen_movement = true,
            (handle, _) => bail!("actor handle {handle} has no content field"),
        }
    }
    let blocks: Vec<&ContentBlock> = supplied.iter().skip(1).collect();
    let stat_block = blocks[plan.stat_container];
    let (_, stats) = fast_array(
        stat_block,
        &plan.content.component_layout,
        vec![RepCommand::StatPairRepState],
        plan.content.stat_container_bound,
        rep_codec,
        engine,
    )?;
    content.stats = Some(FastArrayContent {
        replication_key: stats.array_replication_key,
        items: stats
            .changed
            .iter()
            .map(|(_, item)| match item {
                FastArrayItem::Delta(stream) => match handle_value(stream, 1) {
                    Some(RepValue::StatPairRepState(pair)) => Ok(*pair),
                    _ => bail!("stat item without its pair"),
                },
                FastArrayItem::Whole(_) => bail!("whole stat item"),
            })
            .collect::<Result<_>>()?,
    });
    let state_block = blocks[plan.actor_state];
    content.character_state = read_rep_properties(
        &mut state_block.payload.reader(),
        &player_character_state_rep_layout(),
        rep_codec,
    )?
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect();
    for block in &blocks[plan.components.len()..] {
        let (properties, slots) = fast_array(
            block,
            &inventory_rep_layout(),
            inventory_slot_commands(release.tags.tag_encoding()),
            plan.content.inventory_bound,
            rep_codec,
            engine,
        )?;
        let properties = properties.context("inventory properties")?;
        let Some(RepValue::Byte(info_row)) = handle_value(&properties, 7) else {
            bail!("inventory without its info row");
        };
        content.inventories.push(PawnInventory {
            info_row: *info_row,
            slots: FastArrayContent {
                replication_key: slots.array_replication_key,
                items: slots
                    .changed
                    .iter()
                    .map(|(_, item)| {
                        let FastArrayItem::Delta(stream) = item else {
                            bail!("whole slot item");
                        };
                        match (handle_value(stream, 14), handle_value(stream, 18)) {
                            (Some(RepValue::UInt16(query_row)), Some(RepValue::Int(index))) => {
                                Ok(EmptySlot {
                                    query_row: *query_row,
                                    index: *index,
                                })
                            }
                            _ => bail!("slot without its query row and index"),
                        }
                    })
                    .collect::<Result<_>>()?,
            },
        });
    }
    // From the data tables: whether they equal the decoded values, which
    // they then replace.
    let mut from_data_pak = None;
    if let Some(path) = data_pak {
        let mut tables = icarus_bevy::data_pak::DataTables::open(path)?;
        let tabled = icarus_bevy::network::pawn_content(Some(&mut tables))?;
        from_data_pak = Some(json!({
            "character_state_equal": tabled.character_state == content.character_state,
            "inventories_equal": tabled.inventories == content.inventories,
            "stats_equal": tabled.stats == content.stats,
        }));
        content.character_state = tabled.character_state;
        content.inventories = tabled.inventories;
        content.stats = tabled.stats;
    }
    let state = PawnOpenState {
        key: ActorKey::Session(1),
        controller: *references.get(&13).context("Owner")?,
        player_state: *references.get(&17).context("PlayerState")?,
        location: location.context("spawn location")?,
        content,
    };
    let (mut cache, mut exports) = prepared(file, &plan.level)?;
    let composed = compose_pawn_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        file.channel_index,
        ByteOrder::Little,
    )?;
    let composed_exports = composed.exports.context("exports")?;
    println!(
        "{}",
        json!({
            "mode": "native",
            "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
            "exports_identical": composed_exports == *supplied_exports,
            "body_bits": [composed.bunch.payload.bit_len(), supplied_body.bit_len()],
            "body_identical": composed.bunch.payload == *supplied_body,
            "from_data_pak": from_data_pak,
        })
    );
    Ok(())
}
