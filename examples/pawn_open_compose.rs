//! Compose a pawn open with `experimental::actor_open` from a profile JSON
//! file and compare it with a supplied open body and export payload (one
//! JSON line). The profile carries identifiers, the GUIDs assigned before
//! the open, the object references by handle and the supplied body. The
//! actor block's other values and every component and subobject block's
//! content are decoded from that body; GUIDs, references, exports and
//! framing come from the composer. Never reads original files.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
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
    release::ReleaseProfile,
    rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
        bp_icarus_player_character_survival_rep_layout, read_rep_properties,
    },
    session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON (see `ProfileFile`).
    profile: PathBuf,
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
            block: Some(BlockContent {
                has_rep_layout: block.has_rep_layout,
                payload: block.payload.clone(),
            }),
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
            },
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
        values,
        components,
        subobjects,
        engine_net_version: release.engine_network_version,
    };
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &open.level)?;
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
    let (composed, guids) = compose_component_actor_open(
        &mut cache,
        &mut exports,
        &open,
        file.channel_index,
        ByteOrder::Little,
    )?;
    let composed_exports = composed.exports.context("exports")?;
    let supplied_exports = payload(&file.exports_hex, file.exports_bits)?;
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
