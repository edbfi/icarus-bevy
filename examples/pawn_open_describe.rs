//! Describe a supplied pawn, (with `--game-state`) GameState or (with
//! `--container-manager`) InventoryContainerManager open body
//! block by block (one JSON line per block): the actor block and each
//! component block as rep-layout property entries, `Stat Container` and
//! runtime `Inventory` blocks as their custom deltas. Reads the
//! `pawn_open_compose` profile JSON and one schema name per component block,
//! in block order. Never reads original files; values are printed in full and
//! stay with the caller.
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::{
        ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock, read_actor_bunch_body,
    },
    archive::ByteOrder,
    packet::{BitWriter, Payload},
    release::ReleaseProfile,
    rep_layout::{
        GameplayTag, MovementLevel, PropertyStream, RepCommand, RepLayoutCodec, RepLayoutLimits,
        RepValue, RotationLevel, actor_component_rep_layout, audio_context_component_rep_layout,
        bp_flammable_actor_rep_layout, bp_icarus_player_character_survival_rep_layout,
        bp_sheltered_component_rep_layout, bp_swimming_component_rep_layout,
        bp_ui_projection_component_player_rep_layout, icarus_game_state_survival_rep_layout,
        inventory_container_manager_rep_layout, inventory_rep_layout, inventory_slot_commands,
        player_character_state_rep_layout, read_rep_properties, stomach_component_rep_layout,
        trait_component_rep_layout, transmutable_component_rep_layout,
        weather_manager_component_rep_layout,
    },
    rep_rpc::{
        FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
        read_fast_array_delta, read_field_header,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Parser)]
struct Options {
    /// Profile JSON (the `pawn_open_compose` shape; only the body is read).
    profile: PathBuf,
    /// One schema name per component block, in block order.
    #[arg(long = "schema")]
    schemas: Vec<String>,
    /// The body is the GameState's open (`BP_IcarusGameState_C`).
    #[arg(long)]
    game_state: bool,
    /// The body is the InventoryContainerManager's open.
    #[arg(long, conflicts_with = "game_state")]
    container_manager: bool,
}

#[derive(Deserialize)]
struct ProfileFile {
    body_hex: String,
    body_bits: usize,
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

fn tag(tag: &GameplayTag) -> Value {
    match tag {
        GameplayTag::NetIndex(index) => json!(index),
        GameplayTag::Name(name) => json!(format!("{name:?}")),
    }
}

/// A value as JSON: scalars, GUIDs, tag indices, row handles and stat pairs
/// in full, other kinds in their debug form.
fn render(value: &RepValue) -> Value {
    match value {
        RepValue::Bool(value) => json!(value),
        RepValue::Byte(value) => json!(value),
        RepValue::Enum(value) | RepValue::UInt32(value) => json!(value),
        RepValue::Int8(value) => json!(value),
        RepValue::Int16(value) => json!(value),
        RepValue::UInt16(value) => json!(value),
        RepValue::Int(value) => json!(value),
        RepValue::Int64(value) => json!(value),
        RepValue::UInt64(value) => json!(value),
        RepValue::Float(bits) => json!({ "float": f32::from_bits(*bits) }),
        RepValue::Vector2D(bits) => json!({ "vector2d": bits.map(f32::from_bits) }),
        RepValue::Object(reference) => json!({ "guid": reference.guid }),
        RepValue::Str(text) => json!({ "str": String::from_utf16_lossy(text.units()) }),
        RepValue::RowHandle(handle) => {
            json!({ "table": handle.table_index, "row": handle.row_index })
        }
        RepValue::StatPairRepState(pair) => json!({ "stat": pair.stat, "value": pair.value }),
        RepValue::GameplayTag(value) => json!({ "tag": tag(value) }),
        RepValue::GameplayTagContainer(tags) => {
            json!({ "tags": tags.as_ref().map(|tags| tags.iter().map(tag).collect::<Vec<_>>()) })
        }
        RepValue::DynamicArray(array) => {
            json!({ "count": array.count, "stream": stream(&array.stream) })
        }
        other => json!(format!("{other:?}")),
    }
}

fn stream(stream: &PropertyStream) -> Value {
    stream
        .entries
        .iter()
        .map(|entry| json!([entry.handle, render(&entry.value)]))
        .collect()
}

fn component_layout(schema: &str) -> Result<Vec<RepCommand>> {
    Ok(match schema {
        "UActorComponent" => actor_component_rep_layout(),
        "UAudioContextComponent" => audio_context_component_rep_layout(),
        "UTraitComponent" => trait_component_rep_layout(),
        "UTransmutableComponent" => transmutable_component_rep_layout(),
        "UPlayerCharacterState" => player_character_state_rep_layout(),
        "UStomachComponent" => stomach_component_rep_layout(),
        "BP_SwimmingComponent_C" => bp_swimming_component_rep_layout(),
        "BP_ShelteredComponent_C" => bp_sheltered_component_rep_layout(),
        "BP_Flammable_Actor_C" => bp_flammable_actor_rep_layout(),
        "BP_UIProjectionComponent_Player_C" => bp_ui_projection_component_player_rep_layout(),
        "UWeatherManagerComponent" => weather_manager_component_rep_layout(),
        other => bail!("no layout for {other}"),
    })
}

/// Optional rep-layout properties, then custom-delta fast-array fields.
fn describe_custom(
    block: &ContentBlock,
    layout: &[RepCommand],
    item: &FastArrayItemLayout,
    bound: u32,
    codec: &RepLayoutCodec,
    engine_net_version: u32,
) -> Result<Value> {
    let mut reader = block.payload.reader();
    let properties = if block.has_rep_layout {
        Some(stream(&read_rep_properties(&mut reader, layout, codec)?))
    } else {
        None
    };
    let mut fields = Vec::new();
    while !reader.at_end() {
        let header = read_field_header(&mut reader, bound, codec.limits.max_opaque_bits)?;
        let mut field = header.payload.reader();
        let delta_header = read_custom_delta_header(&mut field, engine_net_version, 1)?;
        let delta = read_fast_array_delta(&mut field, item, codec, FAST_ARRAY_COUNT_CAP)?;
        ensure!(field.at_end(), "trailing bits in a fast-array delta");
        fields.push(json!({
            "index": header.index,
            "header": format!("{delta_header:?}"),
            "array_replication_key": delta.array_replication_key,
            "base_replication_key": delta.base_replication_key,
            "deleted": delta.deleted_ids,
            "changed": delta.changed.iter().map(|(id, item)| json!({
                "id": id,
                "item": match item {
                    FastArrayItem::Delta(item) => stream(item),
                    FastArrayItem::Whole(items) => json!(format!("{items:?}")),
                },
            })).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({ "properties": properties, "fields": fields }))
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
    let tags = release.tags.tag_encoding();
    let body = payload(&file.body_hex, file.body_bits)?;
    let body = read_actor_bunch_body(&mut body.reader(), true, &codec)?;
    let mut schemas = options.schemas.iter();
    for (index, block) in body.blocks.iter().enumerate() {
        let described = match &block.target {
            BlockTarget::Actor => {
                let (schema, layout) = if options.container_manager {
                    (
                        "AInventoryContainerManager",
                        inventory_container_manager_rep_layout(
                            MovementLevel::Whole,
                            MovementLevel::Whole,
                            RotationLevel::Bytes,
                            tags,
                        ),
                    )
                } else if options.game_state {
                    (
                        "BP_IcarusGameState_C",
                        icarus_game_state_survival_rep_layout(
                            MovementLevel::Whole,
                            MovementLevel::Whole,
                            RotationLevel::Bytes,
                        ),
                    )
                } else {
                    (
                        "BP_IcarusPlayerCharacterSurvival_C",
                        bp_icarus_player_character_survival_rep_layout(
                            MovementLevel::Hundredths,
                            MovementLevel::Whole,
                            RotationLevel::Bytes,
                            tags,
                        ),
                    )
                };
                json!({
                    "schema": schema,
                    "properties": stream(&read_rep_properties(
                        &mut block.payload.reader(),
                        &layout,
                        &rep_codec,
                    )?),
                })
            }
            BlockTarget::SubObject { class, .. } => {
                let runtime = !matches!(
                    class,
                    icarus_bevy::experimental::actor_channel::SubObjectClass::StablyNamed
                );
                let schema = if runtime {
                    "UInventory"
                } else {
                    schemas.next().context("a schema per component block")?
                };
                match schema {
                    "UIcarusStatContainer" => json!({
                        "schema": schema,
                        "custom": describe_custom(
                            block,
                            &actor_component_rep_layout(),
                            &FastArrayItemLayout::Delta(vec![RepCommand::StatPairRepState]),
                            release.class_net_cache.stat_container,
                            &rep_codec,
                            release.engine_network_version,
                        )?,
                    }),
                    "UInventory" => json!({
                        "schema": schema,
                        "custom": describe_custom(
                            block,
                            &inventory_rep_layout(),
                            &FastArrayItemLayout::Delta(inventory_slot_commands(tags)),
                            release.class_net_cache.inventory,
                            &rep_codec,
                            release.engine_network_version,
                        )?,
                    }),
                    other => {
                        let layout = component_layout(other)?;
                        let properties = if block.has_rep_layout {
                            Some(stream(&read_rep_properties(
                                &mut block.payload.reader(),
                                &layout,
                                &rep_codec,
                            )?))
                        } else {
                            ensure!(
                                block.payload.bit_len() == 0,
                                "{other} block carries fields without a rep layout"
                            );
                            None
                        };
                        json!({ "schema": other, "properties": properties })
                    }
                }
            }
        };
        println!(
            "{}",
            json!({
                "block": index,
                "has_rep_layout": block.has_rep_layout,
                "bits": block.payload.bit_len(),
                "described": described,
            })
        );
    }
    ensure!(
        schemas.next().is_none(),
        "more schemas than component blocks"
    );
    Ok(())
}
