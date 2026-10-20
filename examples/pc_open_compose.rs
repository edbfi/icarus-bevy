//! Compose the PlayerController open's NetGUID export prefix from a profile
//! JSON file and print its bit length and hex payload (one JSON line). The
//! profile carries release identifiers and class checksums; this example only
//! runs `experimental::session_open` on it and never reads original files.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::{ActorChannelCodec, ActorChannelLimits},
    archive::ByteOrder,
    rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RotationLevel, TagEncoding,
        actor_component_rep_layout, icarus_player_controller_survival_rep_layout,
    },
    session_open::{
        ComponentBlock, ComponentExport, NetGuidCache, PathSegment, PlayerControllerOpenState,
        PlayerControllerProfile, compose_player_controller_open_body, encode_exports,
    },
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON: {"archetype": [[path, checksum, no_load]...], "level": [...],
    /// "components": [[name, checksum]...]}.
    profile: PathBuf,
}

#[derive(Deserialize)]
struct ProfileFile {
    archetype: Vec<(String, u32, bool)>,
    level: Vec<(String, u32, bool)>,
    /// `[name, checksum, block]`: block `null`, `"create"` or `[[handle, bool]...]`.
    components: Vec<(String, u32, serde_json::Value)>,
    spawn_location: [f32; 3],
}

fn segments(items: &[(String, u32, bool)]) -> Vec<PathSegment> {
    items
        .iter()
        .map(|(path, checksum, no_load)| PathSegment::new(path, *checksum, *no_load))
        .collect()
}

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let file: ProfileFile = serde_json::from_str(&text).context("parse profile")?;
    let profile = PlayerControllerProfile {
        archetype: segments(&file.archetype),
        level: segments(&file.level),
        components: file
            .components
            .into_iter()
            .map(|(name, checksum, block)| ComponentExport {
                name,
                checksum,
                block: match block {
                    serde_json::Value::Null => ComponentBlock::None,
                    serde_json::Value::String(_) => ComponentBlock::CreateOnly,
                    other => {
                        ComponentBlock::RepBools(serde_json::from_value(other).unwrap_or_default())
                    }
                },
            })
            .collect(),
    };
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let (guids, references) = cache.player_controller_exports(&profile)?;
    let payload = encode_exports(&references, &codec)?;
    let actor_layout = icarus_player_controller_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        TagEncoding::NetIndex {
            first_segment: 12,
            total_bits: 12,
        },
    );
    let rep_codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            has_level: true,
            ..codec
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let state = PlayerControllerOpenState {
        spawn_location: file.spawn_location,
        net_player_index: 0,
        remote_role: 2,
        role: 3,
    };
    let body = compose_player_controller_open_body(
        &guids,
        &profile,
        &state,
        &actor_layout,
        &actor_component_rep_layout(),
        &rep_codec,
    )?;
    let body_payload =
        icarus_bevy::experimental::actor_channel::encode_player_controller_open_bunch_body(
            &body, &codec,
        )?;
    let body_hex: String = body_payload
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let hex: String = payload
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!(
        "{}",
        json!({"bits": payload.bit_len(), "guids": references.iter().map(|r| r.guid).collect::<Vec<_>>(),
        "actor": guids.actor, "player_state": guids.player_state, "payload_hex": hex,
        "body_bits": body_payload.bit_len(), "body_hex": body_hex})
    );
    Ok(())
}
