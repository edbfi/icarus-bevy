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
    session_open::{
        ComponentExport, NetGuidCache, PathSegment, PlayerControllerProfile, encode_exports,
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
    components: Vec<(String, u32)>,
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
            .map(|(name, checksum)| ComponentExport { name, checksum })
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
    let hex: String = payload
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!(
        "{}",
        json!({"bits": payload.bit_len(), "guids": references.iter().map(|r| r.guid).collect::<Vec<_>>(),
        "actor": guids.actor, "player_state": guids.player_state, "payload_hex": hex})
    );
    Ok(())
}
