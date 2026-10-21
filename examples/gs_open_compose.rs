//! Compose the GameState open from a profile JSON file and compare it with a
//! supplied open body and export payload (one JSON line). The profile carries
//! release identifiers, the GUIDs assigned before the open, and the supplied
//! body; the state values (every handle the composer does not write, and the
//! weather component's properties) are decoded from that body, while roles,
//! class, component and manager references, GUIDs, exports and framing come
//! from `experimental::game_state_open`. Never reads original files.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use icarus_bevy::experimental::game_state_open::active_prospect_values;
use icarus_bevy::experimental::{
    actor_channel::{
        ActorChannelCodec, ActorChannelLimits, BlockTarget, NetGuidRef, read_actor_bunch_body,
    },
    archive::ByteOrder,
    game_state_open::{
        GameStateComponents, GameStateManagers, GameStateOpenPlan, GameStateOpenState,
        compose_game_state_open, handle,
    },
    packet::{BitWriter, Payload},
    rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue,
        icarus_game_state_survival_rep_layout, read_rep_properties,
        weather_manager_component_rep_layout,
    },
    session_open::{
        ActorKey, ComponentBlock, ComponentExport, ConnectionExports, NetGuidCache, PathSegment,
    },
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON (see `ProfileFile`).
    profile: PathBuf,
    /// The prospect save the capture's server loaded: handles 51..=67
    /// (`ReplicatedActiveProspect`) come from its `ProspectInfo` and level
    /// clock through `ProspectInfo::for_session` (no member playing) and
    /// `game_state_open::active_prospect_values` instead of the body.
    #[arg(long)]
    prospect: Option<PathBuf>,
}

#[derive(Deserialize)]
struct ProfileFile {
    archetype: Vec<(String, u32, bool)>,
    level: Vec<(String, u32, bool)>,
    game_mode_class: Vec<(String, u32, bool)>,
    spectator_class: Vec<(String, u32, bool)>,
    /// `[name, checksum]` for ConnectedPlayers, DialogueSystem, WeatherComp.
    components: [(String, u32); 3],
    /// Static GUIDs assigned after the level and before the archetype.
    burn_static: u32,
    /// Dynamic GUIDs assigned before the GameState.
    burn_dynamic: u32,
    channel_index: u32,
    /// The supplied open body and export payload, as hex and bit counts.
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
    anyhow::ensure!(
        text.is_ascii() && text.len().is_multiple_of(2),
        "hex must be ASCII digit pairs"
    );
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .context("hex")?;
    anyhow::ensure!(bits <= bytes.len() * 8, "bit count exceeds the hex");
    let mut writer = BitWriter::new();
    for index in 0..bits {
        writer.write_bit(bytes[index / 8] >> (index % 8) & 1 == 1);
    }
    Ok(Payload::from_writer(writer))
}

const MANAGER_HANDLES: [u32; 7] = [
    handle::QUEST_MANAGER,
    handle::WORLD_TALENT_MANAGER,
    handle::RADIATION_MANAGER,
    handle::INVENTORY_CONTAINER_MANAGER,
    handle::PLAYER_HISTORY_TRACKER,
    handle::FLOD,
    handle::DISASTER_CONTROLLER,
];

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let file: ProfileFile = serde_json::from_str(&text).context("parse profile")?;
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
    let layout = icarus_game_state_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        icarus_bevy::experimental::rep_layout::RotationLevel::Bytes,
    );
    let weather_layout = weather_manager_component_rep_layout();
    let supplied_body = payload(&file.body_hex, file.body_bits)?;
    let supplied = read_actor_bunch_body(&mut supplied_body.reader(), true, &codec)?;
    let actor_block = supplied
        .blocks
        .iter()
        .find(|block| matches!(block.target, BlockTarget::Actor))
        .context("actor block")?;
    let stream = read_rep_properties(&mut actor_block.payload.reader(), &layout, &rep_codec)?;
    // Keyed managers: the supplied GUIDs are recorded only to check that the
    // composer assigns the same ones.
    let mut supplied_managers = BTreeMap::new();
    let mut values = BTreeMap::new();
    for entry in stream.entries {
        let composed = [
            handle::REMOTE_ROLE,
            handle::ROLE,
            handle::GAME_MODE_CLASS,
            handle::SPECTATOR_CLASS,
            handle::CONNECTED_PLAYERS,
            handle::DIALOGUE_SYSTEM,
        ];
        if MANAGER_HANDLES.contains(&entry.handle) {
            let RepValue::Object(NetGuidRef { guid, .. }) = entry.value else {
                bail!("manager handle {} is not an object", entry.handle);
            };
            supplied_managers.insert(entry.handle, guid);
        } else if !composed.contains(&entry.handle) {
            values.insert(entry.handle, entry.value);
        }
    }
    // Per prospect handle: whether the save's value equals the supplied one
    // (no values are printed).
    let mut prospect_handles = BTreeMap::new();
    let mut array_differences = BTreeMap::new();
    let mut int_differences = BTreeMap::new();
    if let Some(path) = &options.prospect {
        let file = std::fs::File::open(path).context("open the prospect save")?;
        let saved = icarus_bevy::prospect::read_saved_prospect(file)?;
        // The capture's only client was no member.
        let info = saved.info.for_session(
            saved
                .level_time_elapsed_sec
                .context("the save records no level time")?,
            |_| false,
        );
        let native = active_prospect_values(&info)?;
        let range = handle::ACTIVE_PROSPECT_FIRST..handle::ACTIVE_PROSPECT_FIRST + 17;
        for handle in range.clone() {
            let supplied = values.get(&handle);
            let mapped = native.get(&handle);
            prospect_handles.insert(
                handle,
                match (supplied, mapped) {
                    (None, None) => "both_default",
                    (Some(a), Some(b)) if a == b => "equal",
                    (Some(RepValue::DynamicArray(a)), Some(RepValue::DynamicArray(b))) => {
                        // Which element entries differ, by position only.
                        let positions: Vec<u32> = a
                            .stream
                            .entries
                            .iter()
                            .zip(&b.stream.entries)
                            .filter(|(x, y)| x != y)
                            .map(|(x, _)| x.handle)
                            .collect();
                        array_differences.insert(handle, (a.count, b.count, positions));
                        "differ"
                    }
                    (Some(RepValue::Int(a)), Some(RepValue::Int(b))) => {
                        int_differences.insert(handle, i64::from(*a) - i64::from(*b));
                        "differ"
                    }
                    (Some(_), Some(_)) => "differ",
                    (Some(_), None) => "supplied_only",
                    (None, Some(_)) => "save_only",
                },
            );
        }
        values.retain(|handle, _| !range.contains(handle));
        values.extend(native);
    }
    let weather_block = supplied.blocks.get(3).context("weather block")?;
    let weather = if weather_block.has_rep_layout {
        read_rep_properties(
            &mut weather_block.payload.reader(),
            &weather_layout,
            &rep_codec,
        )?
        .entries
    } else {
        Vec::new()
    };
    let component = |index: usize| ComponentExport {
        name: file.components[index].0.clone(),
        checksum: file.components[index].1,
        block: ComponentBlock::None,
    };
    let plan = GameStateOpenPlan {
        archetype: segments(&file.archetype),
        level: segments(&file.level),
        game_mode_class: segments(&file.game_mode_class),
        spectator_class: segments(&file.spectator_class),
        components: GameStateComponents {
            connected_players: component(0),
            dialogue_system: component(1),
            weather: component(2),
        },
        layout,
        weather_layout,
        engine_net_version: 17,
    };
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    // The level and earlier GUIDs were assigned and exported before this open.
    cache.export_static(&mut exports, &plan.level)?;
    for index in 0..file.burn_static {
        cache.export_static(
            &mut exports,
            &[PathSegment::new(&format!("burn{index}"), 0, false)],
        )?;
    }
    for _ in 0..file.burn_dynamic {
        cache.assign_dynamic();
    }
    // The client had acked those exports when the capture's open went out.
    exports.commit_bunch(1);
    exports.received_ack(1);
    // Managers keyed 1..=7 in handle order; the GameState is a session key.
    let key = |index: u64| {
        supplied_managers
            .contains_key(&MANAGER_HANDLES[index as usize])
            .then_some(ActorKey::World(index + 1))
    };
    let state = GameStateOpenState {
        key: ActorKey::Session(0),
        managers: GameStateManagers {
            quest_manager: key(0),
            world_talent_manager: key(1),
            radiation_manager: key(2),
            inventory_container_manager: key(3),
            player_history_tracker: key(4),
            flod: key(5),
            disaster_controller: key(6),
        },
        values,
        weather,
    };
    let open = compose_game_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        file.channel_index,
        ByteOrder::Little,
    )?;
    let composed_managers: BTreeMap<u32, u32> = MANAGER_HANDLES
        .iter()
        .enumerate()
        .filter(|(_, handle)| supplied_managers.contains_key(handle))
        .map(|(index, handle)| (*handle, cache.actor_guid(ActorKey::World(index as u64 + 1))))
        .collect();
    let composed_exports = open.exports.context("exports")?;
    let supplied_exports = payload(&file.exports_hex, file.exports_bits)?;
    println!(
        "{}",
        json!({
            "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
            "exports_identical": composed_exports == supplied_exports,
            "body_bits": [open.bunch.payload.bit_len(), supplied_body.bit_len()],
            "body_identical": open.bunch.payload == supplied_body,
            "manager_guids_identical": composed_managers == supplied_managers,
            "manager_guids": composed_managers,
            "prospect_handles": prospect_handles,
            "array_differences": array_differences,
            "int_differences": int_differences,
            "composed_exports_hex": hex(composed_exports.bytes()),
        })
    );
    Ok(())
}
