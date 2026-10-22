//! Compose world manager opens (F-859) with `manager_open::compose_manager_open`
//! and compare them with supplied opens (one JSON line per manager). The
//! profile file maps channel indices to a supplied open (`exports_hex`,
//! `exports_bits`, `body_hex`, `body_bits`) and its GUIDs (`actor`,
//! `archetype`, `outer`, `components`); the plans are the built-in 3.0.31
//! release profile's (`manager_open_plans`), and the supplied actor state's
//! values are compared with the plan's. Never reads original files.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
    actor_open::Block,
    archive::ByteOrder,
    manager_open::{ManagerSlot, compose_manager_open},
    packet::{BitWriter, Payload},
    release::ReleaseProfile,
    rep_layout::{
        RepLayoutCodec, RepLayoutLimits, RepValue, actor_state_rep_layout, read_rep_properties,
    },
    session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON: `{"<channel>": Supplied, ...}`.
    profile: PathBuf,
}

#[derive(Deserialize)]
struct Supplied {
    exports_hex: String,
    exports_bits: usize,
    body_hex: String,
    body_bits: usize,
    /// The manager actor's GUID, its archetype's, the archetype's outer
    /// package's and the components' (in block order).
    actor: u32,
    archetype: u32,
    outer: u32,
    components: Vec<u32>,
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

/// The manager M-707 opened on `channel`.
fn slot(channel: &str) -> Option<ManagerSlot> {
    match channel {
        "124" => Some(ManagerSlot::Radiation),
        "125" => Some(ManagerSlot::WorldTalent),
        "127" => Some(ManagerSlot::Disaster),
        _ => None,
    }
}

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let profile: BTreeMap<String, Supplied> =
        serde_json::from_str(&text).context("parse profile")?;
    let codec = ActorChannelCodec {
        engine_net_version: 17,
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
    let plans = ReleaseProfile::release_3_0_31()
        .manager_open_plans("/Game/Maps/Terrain_016_OLY/Terrain_016")?;
    for (channel, supplied) in &profile {
        let Some(slot) = slot(channel) else {
            continue;
        };
        let plan = &plans
            .iter()
            .find(|(candidate, _)| *candidate == slot)
            .context("a plan for the manager")?
            .1;
        let icarus_actor = plan.components.len() > 1;
        let body = payload(&supplied.body_hex, supplied.body_bits)?;
        let decoded = read_actor_bunch_body(&mut body.reader(), true, &codec)
            .with_context(|| format!("decode the supplied body of channel {channel}"))?;
        // The supplied actor state's values against the plan's.
        let mut actor_state_equal = None;
        if icarus_actor {
            let block = decoded.blocks.get(2).context("an actor state block")?;
            let values: BTreeMap<u32, RepValue> = read_rep_properties(
                &mut block.payload.reader(),
                &actor_state_rep_layout(),
                &rep_codec,
            )?
            .entries
            .into_iter()
            .map(|entry| (entry.handle, entry.value))
            .collect();
            let Some(Block::Composed(planned)) = plan.components.get(1).map(|c| &c.block) else {
                bail!("the plan's actor state is not composed");
            };
            actor_state_equal = Some(planned.values == values);
        }
        // GUIDs before the open: the level and the archetype's outer package
        // were exported and acked; the actor's GUID was assigned at the
        // GameState's reference.
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        cache.export_static(&mut exports, &plan.level)?;
        let burn_static_to = |cache: &mut NetGuidCache, next: u32| -> Result<()> {
            let mut index = 0;
            loop {
                let probe = cache.assign_static(&[PathSegment::new(
                    &format!("probe{next}-{index}"),
                    0,
                    false,
                )]);
                index += 1;
                if probe + 2 >= next {
                    ensure!(probe + 2 == next, "static GUID {next} unreachable");
                    return Ok(());
                }
            }
        };
        if icarus_actor {
            burn_static_to(&mut cache, supplied.outer)?;
            let outer = cache.export_static(&mut exports, &plan.archetype[1..])?;
            ensure!(
                outer.guid == supplied.outer,
                "outer GUID {} != {}",
                outer.guid,
                supplied.outer
            );
        }
        burn_static_to(&mut cache, supplied.archetype)?;
        while cache.assign_dynamic() + 2 < supplied.actor {}
        let key = ActorKey::World(1);
        ensure!(cache.actor_guid(key) == supplied.actor, "actor GUID");
        let first = *supplied.components.first().context("a component GUID")?;
        while cache.assign_dynamic() + 2 < first {}
        exports.commit_bunch(1);
        exports.received_ack(1);
        let composed = compose_manager_open(
            &mut cache,
            &mut exports,
            plan,
            key,
            channel.parse()?,
            ByteOrder::Little,
        )
        .context("compose")?;
        let composed_exports = composed.exports.context("exports")?;
        let supplied_exports = payload(&supplied.exports_hex, supplied.exports_bits)?;
        let blocks: Vec<(bool, usize)> = decoded
            .blocks
            .iter()
            .map(|block| (block.has_rep_layout, block.payload.bit_len()))
            .collect();
        println!(
            "{}",
            json!({
                "channel": channel,
                "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
                "exports_identical": composed_exports == supplied_exports,
                "body_bits": [composed.bunch.payload.bit_len(), body.bit_len()],
                "body_identical": composed.bunch.payload == body,
                "supplied_blocks": blocks,
                "actor_state_equal": actor_state_equal,
            })
        );
    }
    Ok(())
}
