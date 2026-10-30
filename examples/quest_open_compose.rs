//! Compose a save's `C019` quest actor opens (`F-934`, `F-935`, `U-074`, `U-076`) with
//! `quest_open::{stitch_saved_quests, compose_quest_open}` across one shared
//! connection `NetGuidCache` (plus optional `C018` `AQuestManager` on channel
//! `126`) and compare every export bunch and actor bunch body bit for bit
//! against supplied capture profiles (`M-707` channels `36, 37, 42..47, 126`).
//! Prints counts and bit-equality comparisons only, never saved values.
use std::{collections::BTreeMap, fs::File, iter::Peekable, path::PathBuf, slice::Iter};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::{
    data_pak::DataTables,
    experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        manager_open::{ManagerSlot, compose_manager_open, quest_handle},
        packet::{BitWriter, Payload},
        quest_open::{compose_quest_open, quest_table_rows, stitch_saved_quests},
        release::ReleaseProfile,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
        session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
    },
    prospect::read_saved_prospect,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Profile JSON: `{"quests": [SuppliedQuest, ...], "quest_manager": Option<SuppliedManager>}`.
    profile: PathBuf,
    /// Save file whose `C019`, `C018` and `C015` records are stitched.
    #[arg(long)]
    save: PathBuf,
    /// Official dedicated server `data.pak` (`D_Quests`, `D_FactionMissions`).
    #[arg(long)]
    data_pak: PathBuf,
}

#[derive(Deserialize)]
struct Profile {
    quests: Vec<SuppliedQuest>,
    quest_manager: Option<SuppliedManager>,
}

#[derive(Deserialize)]
struct SuppliedQuest {
    channel: u32,
    next_static: Option<u32>,
    next_dynamic: Option<u32>,
    actor_guid: u32,
    exports_hex: String,
    exports_bits: usize,
    body_hex: String,
    body_bits: usize,
}

#[derive(Deserialize)]
struct SuppliedManager {
    channel: u32,
    actor_guid: u32,
    next_static: u32,
    first_component: u32,
    exports_hex: String,
    exports_bits: usize,
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
    ensure!(bits <= bytes.len() * 8, "bit count exceeds hex length");
    let mut writer = BitWriter::new();
    for index in 0..bits {
        writer.write_bit(bytes[index / 8] >> (index % 8) & 1 == 1);
    }
    Ok(Payload::from_writer(writer))
}

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

fn advance_dynamic_to(
    cache: &mut NetGuidCache,
    preassigned: &mut Peekable<Iter<'_, (u32, ActorKey)>>,
    next: u32,
) -> Result<()> {
    while let Some((guid, key)) = preassigned.next_if(|(guid, _)| *guid < next) {
        while u64::from(cache.assign_dynamic()?) + 2 < u64::from(*guid) {}
        ensure!(
            cache.actor_guid(*key)? == *guid,
            "preassigned dynamic GUID {guid} unreachable"
        );
    }
    while u64::from(cache.assign_dynamic()?) + 2 < u64::from(next) {}
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse();
    let profile: Profile =
        serde_json::from_str(&std::fs::read_to_string(&options.profile).context("read profile")?)
            .context("parse profile")?;
    let saved = read_saved_prospect(File::open(&options.save).context("open save")?)?;
    let mut tables = DataTables::open(&options.data_pak)?;
    let rows = quest_table_rows(tables.objects("D_Quests")?)?;
    let flags = saved
        .game_mode
        .as_ref()
        .map_or(&[][..], |gm| gm.session_flags.as_slice());
    let stitched = stitch_saved_quests(&saved.quests, saved.quest_manager.as_ref(), flags, &rows)?;
    ensure!(
        stitched.quests.len() == profile.quests.len(),
        "stitched quest count {} != profile count {}",
        stitched.quests.len(),
        profile.quests.len()
    );

    let release = ReleaseProfile::release_3_0_31();
    let map = "/Game/Maps/Terrain_016_OLY/Terrain_016";
    let base = release
        .quest_open_base_plan(map)?
        .context("quest open base plan")?;
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

    let prepared_qm = if let Some(supplied) = &profile.quest_manager {
        let quest_mgr = stitched
            .quest_manager
            .as_ref()
            .context("stitched quest_manager missing")?;
        let row = tables
            .row_index("D_FactionMissions", &quest_mgr.faction_mission)?
            .and_then(|idx| u8::try_from(idx).ok())
            .context("faction mission row")?;
        let qm_plan = release
            .manager_open_plan(ManagerSlot::Quest, map)?
            .context("QuestManager base plan")?
            .with_quest_state(quest_mgr, row)?;
        let supplied_body = payload(&supplied.body_hex, supplied.body_bits)?;
        let decoded = read_actor_bunch_body(&mut supplied_body.reader(), true, &codec)?;
        let supplied_values: BTreeMap<u32, RepValue> = read_rep_properties(
            &mut decoded
                .blocks
                .first()
                .context("actor block")?
                .payload
                .reader(),
            &qm_plan.layout,
            &rep_codec,
        )?
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
        let mut preassigned = vec![(supplied.actor_guid, ManagerSlot::Quest.key())];
        for handle in [
            quest_handle::RELEVANT_PERSISTENT_ACTORS,
            quest_handle::RELEVANT_PERSISTENT_CHARACTERS,
        ] {
            let keys = qm_plan
                .keyed_actors
                .get(&handle)
                .map_or(&[][..], Vec::as_slice);
            let guids = array_guids(supplied_values.get(&handle));
            ensure!(guids.len() == keys.len(), "handle {handle} element count");
            preassigned.extend(guids.into_iter().zip(keys.iter().map(|(_, key)| *key)));
        }
        preassigned.sort_unstable_by_key(|(guid, _)| *guid);
        Some((supplied, qm_plan, supplied_body, preassigned))
    } else {
        None
    };

    let empty_preassigned: Vec<(u32, ActorKey)> = Vec::new();
    let mut preassigned_iter = prepared_qm
        .as_ref()
        .map_or(empty_preassigned.as_slice(), |(_, _, _, list)| {
            list.as_slice()
        })
        .iter()
        .peekable();

    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &base.level)?;
    burn_static_to(&mut cache, 23)?;
    cache.export_static(
        &mut exports,
        &[PathSegment::new("/Script/Icarus", 0, false)],
    )?;
    exports.commit_bunch(1);
    exports.received_ack(1);

    for (quest, supplied) in stitched.quests.iter().zip(&profile.quests) {
        if let Some(next_stat) = supplied.next_static {
            burn_static_to(&mut cache, next_stat)?;
        }
        if let Some(next_dyn) = supplied.next_dynamic {
            advance_dynamic_to(&mut cache, &mut preassigned_iter, next_dyn)?;
        }
        let plan = base.quest_plan(quest)?;
        let composed = compose_quest_open(
            &mut cache,
            &mut exports,
            &plan,
            supplied.channel,
            ByteOrder::Little,
        )?;
        let actor_guid = cache.actor_guid(quest.key)?;
        let composed_exports = composed.exports.context("quest open exports")?;
        let supplied_exports = payload(&supplied.exports_hex, supplied.exports_bits)?;
        let supplied_body = payload(&supplied.body_hex, supplied.body_bits)?;
        exports.commit_bunch(supplied.channel);
        exports.received_ack(supplied.channel);

        let composed_blocks =
            read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &codec)?.blocks;
        let supplied_blocks =
            read_actor_bunch_body(&mut supplied_body.reader(), true, &codec)?.blocks;
        let differing: Vec<(usize, usize, usize)> = composed_blocks
            .iter()
            .zip(&supplied_blocks)
            .enumerate()
            .filter(|(_, (ours, theirs))| ours.payload != theirs.payload)
            .map(|(idx, (ours, theirs))| (idx, ours.payload.bit_len(), theirs.payload.bit_len()))
            .collect();

        println!(
            "{}",
            json!({
                "channel": supplied.channel,
                "kind": "AQuest",
                "actor_guid_identical": actor_guid == supplied.actor_guid,
                "active_quests": quest.active_quests.len(),
                "subquests": quest.subquests.len(),
                "variables": quest.variables.len(),
                "has_parent": quest.parent.is_some(),
                "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
                "exports_identical": composed_exports == supplied_exports,
                "body_bits": [composed.bunch.payload.bit_len(), supplied_body.bit_len()],
                "body_identical": composed.bunch.payload == supplied_body,
                "blocks": [composed_blocks.len(), supplied_blocks.len()],
                "differing_blocks": differing,
            })
        );
    }

    if let Some((supplied, qm_plan, supplied_body, _)) = &prepared_qm {
        // Persistent world actors had their NetGUIDs assigned at world spawn
        // time (`preassigned_iter`); `InitialQuest` (`632`) is already held in
        // `cache` from channel 46 above.
        burn_static_to(&mut cache, supplied.next_static)?;
        advance_dynamic_to(&mut cache, &mut preassigned_iter, supplied.first_component)?;
        let composed = compose_manager_open(
            &mut cache,
            &mut exports,
            qm_plan,
            ManagerSlot::Quest.key(),
            supplied.channel,
            ByteOrder::Little,
        )?;
        let composed_exports = composed.exports.context("QuestManager exports")?;
        let supplied_exports = payload(&supplied.exports_hex, supplied.exports_bits)?;
        println!(
            "{}",
            json!({
                "channel": supplied.channel,
                "kind": "AQuestManager",
                "exports_bits": [composed_exports.bit_len(), supplied_exports.bit_len()],
                "exports_identical": composed_exports == supplied_exports,
                "body_bits": [composed.bunch.payload.bit_len(), supplied_body.bit_len()],
                "body_identical": composed.bunch.payload == *supplied_body,
            })
        );
    }

    Ok(())
}
