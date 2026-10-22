//! Compose the QuestManager open (F-861) from a save's quest manager record
//! (`C018`) with `manager_open::compose_manager_open` and compare it with a
//! supplied open. The profile file maps channel indices to a supplied open
//! (`exports_hex`, `exports_bits`, `body_hex`, `body_bits`) and its GUIDs
//! (`actor`, `archetype`, `outer`, `components`); the plan is the built-in
//! 3.0.31 release profile's, the faction mission's row comes from the
//! official server's data.pak. The referenced actors' GUIDs are read from
//! the supplied body so the composed open names the same ones. Prints
//! counts and comparisons only, never saved values.
use std::{collections::BTreeMap, fs::File, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::{
    data_pak::DataTables,
    experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        manager_open::{compose_manager_open, quest_handle},
        packet::{BitWriter, Payload},
        release::ReleaseProfile,
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            quest_manager_rep_layout, read_rep_properties,
        },
        session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
    },
    prospect::read_saved_prospect,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON: `{"<channel>": Supplied, ...}`.
    profile: PathBuf,
    /// The save whose `C018` record the open replicates.
    #[arg(long)]
    save: PathBuf,
    /// The official dedicated server's `data.pak`.
    #[arg(long)]
    data_pak: PathBuf,
}

#[derive(Deserialize)]
struct Supplied {
    exports_hex: String,
    exports_bits: usize,
    body_hex: String,
    body_bits: usize,
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

/// The object GUIDs of a `(Str, Object)` array value, in element order.
fn array_guids(value: Option<&RepValue>) -> Vec<u32> {
    match value {
        Some(RepValue::DynamicArray(array)) => array
            .stream
            .entries
            .iter()
            .filter_map(|entry| match &entry.value {
                RepValue::Object(reference) => Some(reference.guid),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Assign static GUIDs until the next one is `next`.
fn burn_static_to(cache: &mut NetGuidCache, next: u32) -> Result<()> {
    for index in 0.. {
        let probe = cache.assign_static(&[PathSegment::new(&format!("probe{index}"), 0, false)]);
        if probe + 2 >= next {
            ensure!(probe + 2 == next, "static GUID {next} unreachable");
            return Ok(());
        }
    }
    unreachable!()
}

fn main() -> Result<()> {
    let options = Options::parse();
    let text = std::fs::read_to_string(&options.profile).context("read profile")?;
    let profile: BTreeMap<String, Supplied> =
        serde_json::from_str(&text).context("parse profile")?;
    let saved = read_saved_prospect(File::open(&options.save).context("open save")?)?
        .quest_manager
        .context("the save has no quest manager record")?;
    let mut tables = DataTables::open(&options.data_pak)?;
    let row = tables
        .row_index("D_FactionMissions", &saved.faction_mission)?
        .and_then(|index| u8::try_from(index).ok())
        .context("the faction mission has no 1-byte row")?;
    let plan = ReleaseProfile::release_3_0_31()
        .quest_manager_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")?
        .context("a QuestManager plan")?
        .with_quest_state(&saved, row)?;
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
    for (channel, supplied) in &profile {
        let body = payload(&supplied.body_hex, supplied.body_bits)?;
        let decoded = read_actor_bunch_body(&mut body.reader(), true, &codec)?;
        let actor_block = decoded.blocks.first().context("an actor block")?;
        let supplied_values: BTreeMap<u32, RepValue> = read_rep_properties(
            &mut actor_block.payload.reader(),
            &quest_manager_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                ReleaseProfile::release_3_0_31().tags.tag_encoding(),
            ),
            &rep_codec,
        )?
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
        // The referenced actors' supplied GUIDs, keyed as the plan keys them.
        let mut keyed: Vec<(u32, ActorKey)> = Vec::new();
        if let (Some(key), Some(RepValue::Object(reference))) = (
            plan.actors.get(&quest_handle::INITIAL_QUEST),
            supplied_values.get(&quest_handle::INITIAL_QUEST),
        ) {
            keyed.push((reference.guid, *key));
        }
        for handle in [
            quest_handle::RELEVANT_PERSISTENT_ACTORS,
            quest_handle::RELEVANT_PERSISTENT_CHARACTERS,
        ] {
            let keys = plan
                .keyed_actors
                .get(&handle)
                .map_or(&[][..], Vec::as_slice);
            let guids = array_guids(supplied_values.get(&handle));
            ensure!(guids.len() == keys.len(), "handle {handle} element count");
            keyed.extend(guids.into_iter().zip(keys.iter().map(|(_, key)| *key)));
        }
        keyed.sort_unstable_by_key(|(guid, _)| *guid);
        let first_component = *supplied.components.first().context("a component GUID")?;
        ensure!(
            keyed
                .iter()
                .all(|(guid, _)| *guid < supplied.actor.max(first_component)),
            "a referenced GUID follows the manager's"
        );
        // GUIDs before the open: the level and the archetype's outer were
        // exported and acked; referenced actors and the manager were
        // assigned earlier.
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        cache.export_static(&mut exports, &plan.level)?;
        burn_static_to(&mut cache, supplied.outer)?;
        let outer = cache.export_static(&mut exports, &plan.archetype[1..])?;
        ensure!(outer.guid == supplied.outer, "outer GUID");
        burn_static_to(&mut cache, supplied.archetype)?;
        let manager = ActorKey::Session(1);
        let mut assigned: Vec<(u32, ActorKey)> = keyed.clone();
        assigned.push((supplied.actor, manager));
        assigned.sort_unstable_by_key(|(guid, _)| *guid);
        for (guid, key) in &assigned {
            while cache.assign_dynamic() + 2 < *guid {}
            ensure!(cache.actor_guid(*key) == *guid, "GUID {guid} unreachable");
        }
        while cache.assign_dynamic() + 2 < first_component {}
        exports.commit_bunch(1);
        exports.received_ack(1);
        let composed = compose_manager_open(
            &mut cache,
            &mut exports,
            &plan,
            manager,
            channel.parse()?,
            ByteOrder::Little,
        )
        .context("compose")?;
        let composed_exports = composed.exports.context("exports")?;
        let supplied_exports = payload(&supplied.exports_hex, supplied.exports_bits)?;
        println!(
            "{}",
            json!({
                "channel": channel,
                "referenced_actors": keyed.len(),
                "supplied_handles": supplied_values.keys().collect::<Vec<_>>(),
                "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
                "exports_identical": composed_exports == supplied_exports,
                "body_bits": [composed.bunch.payload.bit_len(), body.bit_len()],
                "body_identical": composed.bunch.payload == body,
            })
        );
    }
    Ok(())
}
