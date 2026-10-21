//! Compose the PlayerState open from a profile JSON file and print its export
//! payload and actor-block payload as hex (one JSON line). The profile carries
//! release identifiers and per-player values; this example only runs
//! `experimental::session_open` on it and never reads original files.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
    archive::ByteOrder,
    control_messages::{ArchiveText, NetIdType, NetIdValue, UniqueNetId},
    rep_layout::{MovementLevel, RotationLevel, bp_icarus_player_state_rep_layout},
    session_open::{
        ConnectionExports, NetGuidCache, PathSegment, PlayerStateOpenPlan, PlayerStateOpenState,
        compose_player_state_open,
    },
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON: archetype and level chains `[[path, checksum, no_load]...]`,
    /// `burn_static` (static GUIDs assigned before the archetype), and the
    /// player values.
    profile: PathBuf,
}

#[derive(Deserialize)]
struct ProfileFile {
    archetype: Vec<(String, u32, bool)>,
    level: Vec<(String, u32, bool)>,
    burn_static: u32,
    player_state: u32,
    owner: u32,
    player_id: i32,
    unique_id_index: u8,
    unique_id_text: String,
    player_name: String,
    is_host: bool,
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

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let file: ProfileFile = serde_json::from_str(&text).context("parse profile")?;
    let plan = PlayerStateOpenPlan {
        archetype: segments(&file.archetype),
        level: segments(&file.level),
        layout: bp_icarus_player_state_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
        ),
        engine_net_version: 17,
        channel_index: 6,
    };
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    // The level and earlier statics were exported before this open.
    cache.export_static(&mut exports, &plan.level)?;
    for index in 0..file.burn_static {
        cache.export_static(
            &mut exports,
            &[PathSegment::new(&format!("burn{index}"), 0, false)],
        )?;
    }
    // The client had acked those exports when the capture's open went out.
    exports.commit_bunch(1);
    exports.received_ack(1);
    let state = PlayerStateOpenState {
        player_state: file.player_state,
        owner: file.owner,
        player_id: file.player_id,
        unique_id: UniqueNetId {
            kind: NetIdType::Indexed(file.unique_id_index),
            value: NetIdValue::Text(ArchiveText::text(&file.unique_id_text)?),
        },
        player_name: ArchiveText::text(&file.player_name)?,
        is_host: file.is_host,
    };
    let open =
        compose_player_state_open(&mut cache, &mut exports, &plan, &state, ByteOrder::Little)?;
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec)?;
    let block = body.blocks.first().context("actor block")?;
    println!(
        "{}",
        json!({
            "exports_bits": open.exports.as_ref().map(|payload| payload.bit_len()),
            "exports_hex": open.exports.as_ref().map(|payload| hex(payload.bytes())),
            "body_bits": open.bunch.payload.bit_len(),
            "body_hex": hex(open.bunch.payload.bytes()),
            "block_bits": block.payload.bit_len(),
            "block_hex": hex(block.payload.bytes()),
        })
    );
    Ok(())
}
