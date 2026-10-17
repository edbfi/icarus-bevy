//! Offline decoder for local capture files only: no networking, never pointed at
//! a live peer. Reads the server-to-client reply datagrams a probe recorded
//! (JSONL lines with `"event":"reply"` and `"hex"`) and runs them through this
//! repository's experimental codecs as the probe's client does: packet framing
//! (F-521..F-525, F-688), per-channel reliable ordering (F-537), partial-bunch
//! reassembly (`F-710`), control messages, and the actor-channel spawn record
//! and content blocks (F-569..F-576, F-709, F-710).
//!
//! Two passes over the same datagrams:
//! - `strict` uses only the original F-569..F-576 actor codec surface, so
//!   NetGUID export bunches and must-be-mapped GUID prefixes stay unparsed and
//!   are counted as such;
//! - `export_hypothesis` adds the statically verified F-709/F-710 readers:
//!   `UPackageMapClient::ReceiveNetGUIDBunch` (`0x1439b8bb0`,
//!   `PackageMapState::consume_export_prefix`), `UActorChannel::ReceivedBunch`
//!   must-be-mapped prefixes (`0x1437c43f0`,
//!   `PackageMapState::consume_must_be_mapped`), and
//!   `UChannel::ReceivedNextBunch` partial-bunch reassembly (`0x1437c5500`,
//!   `PartialBunchReassembler`).
//!
//! Stdout carries aggregate counts only; channel names appear as hard-coded
//! indices or the word `string`. Per-item detail, including any path or class
//! strings, goes to the second argument, which must lie under this
//! checkout's `work/` directory.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    actor_channel::{
        ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock,
        EXPORT_HAS_CHECKSUM, EXPORT_HAS_PATH, EXPORT_NO_LOAD, MAX_GUID_DEPTH, NetGuidRef,
        PackageMapState, SpawnRecord, SpawnVector, SubObjectClass,
        encode_player_controller_open_bunch_body, guid_kind, parse_actor_bunch,
        parse_player_controller_open_bunch, read_content_block, read_spawn, write_content_block,
    },
    archive::{ByteOrder, read_string_frame},
    control_messages::{ControlCodec, Decoded, read_control_messages},
    packet::{
        BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, ChannelCloseReason, Name,
        PacketConfig, Payload, RELIABLE_SEQUENCE_LIMIT, Receiver, read_packet,
    },
    peer_lab::PeerLabConfig,
    rep_layout::{
        MovementLevel, RepCommand, RepLayoutCodec, RepLayoutLimits, RotationLevel, TagEncoding,
        actionable_component_rep_layout, actor_component_rep_layout, actor_rep_layout,
        actor_state_rep_layout, audio_context_component_rep_layout,
        bp_atmosphere_controller_rep_layout, bp_bed_base_rep_layout, bp_building_base_rep_layout,
        bp_dropship_rep_layout, bp_dropship_seat_rep_layout, bp_flammable_actor_rep_layout,
        bp_icarus_player_character_survival_rep_layout, bp_icarus_player_state_rep_layout,
        bp_interactable_butcher_animal_rep_layout, bp_main_level_teleport_rep_layout,
        bp_map_manager_rep_layout, bp_meta_deposit_rep_layout, bp_mount_base_rep_layout,
        bp_mount_horse_rep_layout, bp_oil_geyser_rep_layout, bp_part_base_rep_layout,
        bp_part_btm_mk1_rep_layout, bp_part_mid_mk1_rep_layout, bp_portable_beacon_rep_layout,
        bp_prebuilt_base_rep_layout, bp_seat_base_rep_layout, bp_seat_mount_rep_layout,
        bp_sheltered_component_rep_layout, bp_swimming_component_rep_layout,
        bp_tame_base_rep_layout, bp_ui_projection_component_mount_status_rep_layout,
        bp_ui_projection_component_player_rep_layout, bp_world_boss_spawner_rep_layout,
        bp_world_object_rep_layout, bpc_environmental_buildup_rep_layout,
        character_state_rep_layout, child_actor_component_rep_layout,
        decayable_component_rep_layout, durable_component_rep_layout,
        fillable_component_rep_layout, flammable_actor_rep_layout, flod_actor_component_rep_layout,
        flod_rep_layout, focusable_component_rep_layout, genetics_component_rep_layout,
        icarus_game_state_survival_rep_layout, icarus_player_controller_survival_rep_layout,
        icarus_weather_action_rep_layout, interactable_behaviour_rep_layout,
        inventory_container_manager_rep_layout, inventory_rep_layout, inventory_slot_commands,
        itemable_component_rep_layout, modifier_state_component_rep_layout,
        mount_character_state_rep_layout, player_character_state_rep_layout,
        player_history_tracker_rep_layout, player_state_rep_layout, quest_manager_rep_layout,
        quest_rep_layout, radiation_manager_rep_layout, read_rep_properties,
        resource_network_component_rep_layout, scene_component_rep_layout,
        static_mesh_component_rep_layout, stomach_component_rep_layout,
        survival_character_state_rep_layout, trait_behaviour_rep_layout,
        trait_component_rep_layout, transmutable_component_rep_layout, voxel_resource_rep_layout,
        weather_controller_rep_layout, weather_forecast_bar_component_rep_layout,
        weather_manager_component_rep_layout, world_settings_rep_layout,
        world_talent_manager_rep_layout, write_rep_properties,
    },
    rep_rpc::{
        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
        BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND, BP_MOUNT_BASE_FIELD_INDEX_BOUND,
        BP_MOUNT_HORSE_FIELD_INDEX_BOUND, BP_TAME_BASE_FIELD_INDEX_BOUND, FAST_ARRAY_COUNT_CAP,
        FastArrayItemLayout, FieldHeader, INVENTORY_FIELD_INDEX_BOUND, RpcParameter,
        STAT_CONTAINER_FIELD_INDEX_BOUND, UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND,
        bp_icarus_player_state_rpc_parameters, encode_rpc_payload, icarus_character_rpc_parameters,
        parse_rpc_payload, player_controller_rpc_parameters, read_custom_delta_header,
        read_fast_array_delta, read_field_header, write_custom_delta_header,
        write_fast_array_delta, write_field_header,
    },
    sequencing::{
        MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PartialBunchReassembler, PartialReceive,
        ReliableChannel, ReliableReceive, is_newer, max_single_bunch_size_bits,
        reconstruct_reliable_sequence, sequence_difference, split_outgoing_bunch,
    },
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Parser)]
struct Options {
    /// Local probe JSONL capture (reply events with hex datagrams).
    input: PathBuf,
    /// Per-item detail JSONL; must lie under this checkout's `work/`.
    detail: PathBuf,
}

/// The probe's channel-index ceiling for replies (handshake_probe).
const MAX_CHANNELS: u32 = 32_767;
/// Engine network version of the client build (F-527).
const ENGINE_NET_VERSION: u32 = 17;
/// The control channel's hard-coded name index (F-525).
const CONTROL_NAME_INDEX: u32 = 255;
/// UE 4.27's cap on GUIDs per export bunch (hypothesis pass only).
const MAX_EXPORT_GUIDS: i32 = 2048;
/// Error samples kept in the detail file per error class.
const SAMPLES_PER_CLASS: u64 = 20;
/// Export flag bits known from F-572.
const EXPORT_KNOWN_FLAGS: u8 = EXPORT_HAS_PATH | EXPORT_NO_LOAD | EXPORT_HAS_CHECKSUM;
/// Research cap on export path units, terminator included.
const MAX_PATH_UNITS: usize = 4096;
/// Bound for "sane" spawn vector components.
const SANE_EXTENT: f32 = 1.0e6;

type Tally = BTreeMap<String, u64>;

fn bump(tally: &mut Tally, key: impl Into<String>) {
    *tally.entry(key.into()).or_default() += 1;
}

/// The full error chain with digit runs folded, so classes aggregate.
fn class(error: &anyhow::Error) -> String {
    let mut folded = String::new();
    let mut in_digits = false;
    for character in format!("{error:#}").chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                folded.push('N');
            }
            in_digits = true;
        } else {
            folded.push(character);
            in_digits = false;
        }
    }
    folded.chars().take(160).collect()
}

fn name_key(name: Option<&Name>) -> String {
    match name {
        Some(Name::Hardcoded(index)) => format!("hardcoded:{index}"),
        Some(Name::String { .. }) => "string".into(),
        None => "unknown".into(),
    }
}

/// Name text for the work-only detail file.
fn name_text(name: &Name) -> String {
    match name {
        Name::Hardcoded(index) => format!("hardcoded:{index}"),
        Name::String { frame, number } => match read_string_frame(frame, ByteOrder::Little) {
            Ok(parsed) => format!(
                "{}#{number}",
                String::from_utf16_lossy(&parsed.units().collect::<Vec<_>>())
            ),
            Err(_) => "invalid".into(),
        },
    }
}

/// Copy the reader's remaining bits.
fn rest(reader: &mut BitReader<'_>) -> Payload {
    let mut writer = BitWriter::new();
    append_reader(&mut writer, reader);
    Payload::from_writer(writer)
}

fn append_reader(writer: &mut BitWriter, reader: &mut BitReader<'_>) {
    let mut bits = reader.remaining();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

fn append(writer: &mut BitWriter, payload: &Payload) {
    append_reader(writer, &mut payload.reader());
}

/// One exported reference as the hypothesis reader sees it.
struct Exported {
    guid: u32,
    flags: u8,
    outer: Option<Box<Exported>>,
    path: Option<String>,
    checksum: Option<u32>,
}

/// The export prefix's rep-layout bit and i32 GUID count.
fn read_export_count(reader: &mut BitReader<'_>) -> Result<i32> {
    let rep_layout_export = reader.read_bit();
    ensure!(!reader.is_error(), "truncated export prefix");
    ensure!(!rep_layout_export, "rep-layout export prefix unparsed");
    let count = reader.read_u32() as i32;
    ensure!(!reader.is_error(), "truncated export GUID count");
    ensure!(
        (0..=MAX_EXPORT_GUIDS).contains(&count),
        "export GUID count out of range"
    );
    Ok(count)
}

/// HYPOTHESIS (UE 4.27 `ReceiveNetGUIDBunch` and `InternalLoadObject`, not a
/// finding): the same layout, but the checksum bit only adds a u32 inside the
/// path branch, so a checksum flag without the path flag reads nothing more.
fn read_export_prefix(reader: &mut BitReader<'_>) -> Result<Vec<Exported>> {
    let count = read_export_count(reader)?;
    (0..count)
        .map(|index| read_export_guid(reader, 0).with_context(|| format!("export GUID {index}")))
        .collect()
}

fn read_export_guid(reader: &mut BitReader<'_>, depth: usize) -> Result<Exported> {
    ensure!(
        depth <= MAX_GUID_DEPTH,
        "NetGUID outer recursion exceeds 16 levels"
    );
    let guid = reader.read_int_packed();
    ensure!(!reader.is_error(), "truncated NetGUID");
    let mut exported = Exported {
        guid,
        flags: 0,
        outer: None,
        path: None,
        checksum: None,
    };
    if guid == 0 {
        return Ok(exported);
    }
    exported.flags = reader.read_bits(8) as u8;
    ensure!(!reader.is_error(), "truncated NetGUID export flags");
    ensure!(
        exported.flags & !EXPORT_KNOWN_FLAGS == 0,
        "reserved NetGUID export flag bits"
    );
    if exported.flags & EXPORT_HAS_PATH != 0 {
        let outer = read_export_guid(reader, depth + 1).context("NetGUID outer")?;
        exported.outer = Some(Box::new(outer));
        exported.path = Some(read_path_string(reader)?);
        if exported.flags & EXPORT_HAS_CHECKSUM != 0 {
            exported.checksum = Some(reader.read_u32());
            ensure!(!reader.is_error(), "truncated NetGUID checksum");
        }
    }
    Ok(exported)
}

/// An i32-counted archive string: narrow bytes when positive, UTF-16LE units
/// when negative, terminator included (validated by the archive reader).
fn read_path_string(reader: &mut BitReader<'_>) -> Result<String> {
    let count = reader.read_u32() as i32;
    ensure!(!reader.is_error(), "truncated export path count");
    ensure!(count != i32::MIN, "export path count i32::MIN");
    let units = count.unsigned_abs() as usize;
    ensure!(
        units <= MAX_PATH_UNITS,
        "export path exceeds the unit limit"
    );
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated export path");
    let frame = [count.to_le_bytes().as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, ByteOrder::Little).context("invalid export path")?;
    ensure!(
        parsed.consumed() == frame.len(),
        "export path frame length mismatch"
    );
    Ok(String::from_utf16_lossy(
        &parsed.units().collect::<Vec<_>>(),
    ))
}

/// Read the `UActorChannel::ReceivedBunch` (`0x1437c43f0`) must-be-mapped
/// prefix via [`PackageMapState::consume_must_be_mapped`] (`F-709`, `F-710`).
fn strip_must_be_mapped(
    package_map: &PackageMapState,
    payload: &Payload,
) -> Result<(Vec<u32>, usize, Payload)> {
    let mut reader = payload.reader();
    let (guids, resolved) = package_map.consume_must_be_mapped(&mut reader, &actor_codec(true))?;
    Ok((guids, resolved, rest(&mut reader)))
}

fn actor_codec(has_level: bool) -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: ENGINE_NET_VERSION,
        exports_enabled: false,
        has_level,
        order: ByteOrder::Little,
        limits: ActorChannelLimits {
            max_payload_bits: 1 << 22,
            ..ActorChannelLimits::default()
        },
    }
}

fn rep_codec() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: actor_codec(true),
        checksums: false,
        limits: RepLayoutLimits {
            max_entries: 32_768,
            max_opaque_bits: 1 << 20,
            ..RepLayoutLimits::default()
        },
    }
}

/// Which typed schema a content block targets.
#[derive(Clone, Copy)]
enum TargetSchema {
    /// Standard `FRepLayout::ReceiveProperties` block (`has_rep_layout == true`).
    RepLayout(&'static str),
    /// `UIcarusStatContainer`: optional `UActorComponent` rep-layout properties
    /// followed by zero or more `ReplicatedStatArray` custom-delta field headers
    /// (`index_bound = STAT_CONTAINER_FIELD_INDEX_BOUND`, F-705, F-708).
    StatContainer,
    /// `UInventory`: optional `UInventory` rep-layout properties followed by
    /// zero or more `Slots` (`FInventorySlotsFastArray`) custom-delta field
    /// headers (`index_bound = INVENTORY_FIELD_INDEX_BOUND`, F-708).
    Inventory,
    /// `BP_UIProjectionComponent_MountStatus_C`: optional rep-layout properties
    /// followed by zero or more `ForceProjectionUpdate` 0-parameter net-RPC
    /// field headers (`index_bound = UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND`, F-708).
    UiProjectionMountStatus,
    /// Zero-bit subobject creation or deletion header (`has_rep_layout == false`
    /// and `payload.bit_len() == 0`).
    SubObjectLifecycle(&'static str),
}

/// Resolve an actor channel's archetype name to a known native or cooked
/// Blueprint rep-layout schema when every property in the hierarchy is known.
fn classify_actor_schema(archetype: &str) -> Option<TargetSchema> {
    if archetype.contains("BP_IcarusPlayerControllerSurvival") {
        Some(TargetSchema::RepLayout("AIcarusPlayerControllerSurvival"))
    } else if archetype.contains("BP_IcarusPlayerState")
        || archetype.contains("IcarusPlayerState_C")
    {
        Some(TargetSchema::RepLayout("BP_IcarusPlayerState_C"))
    } else if archetype == "Default__PlayerState" || archetype.ends_with(".Default__PlayerState") {
        Some(TargetSchema::RepLayout("APlayerState"))
    } else if archetype.contains("BP_IcarusFLOD_Default") {
        Some(TargetSchema::RepLayout("AFLOD"))
    } else if archetype.contains("WorldSettings") {
        Some(TargetSchema::RepLayout("AWorldSettings"))
    } else if archetype.contains("BP_DisasterController")
        || archetype.contains("WeatherForecastManager")
    {
        Some(TargetSchema::RepLayout("AActor"))
    } else if archetype.contains("BP_WeatherController") {
        Some(TargetSchema::RepLayout("AWeatherController"))
    } else if archetype.contains("MapManager_C") {
        Some(TargetSchema::RepLayout("MapManager_C"))
    } else if archetype.contains("Default__BPQ_") {
        Some(TargetSchema::RepLayout("AQuest"))
    } else if archetype.contains("QuestManager") {
        Some(TargetSchema::RepLayout("AQuestManager"))
    } else if archetype.contains("InventoryContainerManager") {
        Some(TargetSchema::RepLayout("AInventoryContainerManager"))
    } else if archetype.contains("PlayerHistoryTracker") {
        Some(TargetSchema::RepLayout("APlayerHistoryTracker"))
    } else if archetype.contains("RadiationManager") {
        Some(TargetSchema::RepLayout("ARadiationManager"))
    } else if archetype.contains("WorldTalentManager") {
        Some(TargetSchema::RepLayout("AWorldTalentManager"))
    } else if archetype.contains("BP_AtmosphereController") {
        Some(TargetSchema::RepLayout("BP_AtmosphereController_C"))
    } else if archetype.contains("BP_OilGeyser") {
        Some(TargetSchema::RepLayout("BP_OilGeyser_C"))
    } else if archetype.contains("BP_Mission_Blocker_")
        || archetype.contains("BP_Mission_Platform_")
    {
        Some(TargetSchema::RepLayout("BP_WorldObject_C"))
    } else if archetype.contains("BP_Prebuilt_Base") {
        Some(TargetSchema::RepLayout("BP_Prebuilt_Base_C"))
    } else if archetype.contains("BP_WorldBossSpawner_") {
        Some(TargetSchema::RepLayout("BP_WorldBossSpawner_C"))
    } else if archetype.contains("BP_MetaDeposit_") {
        Some(TargetSchema::RepLayout("BP_MetaDeposit_C"))
    } else if archetype.contains("BP_Mission_Meta_Voxel") || archetype.contains("BP_VR_ROCK_") {
        Some(TargetSchema::RepLayout("AVoxelResource"))
    } else if archetype.contains("BP_GH_DenEntrance_") {
        Some(TargetSchema::RepLayout("BP_MainLevelTeleport_C"))
    } else if archetype.contains("BP_DropShip_C") {
        Some(TargetSchema::RepLayout("BP_DropShip_C"))
    } else if archetype.contains("BP_DropshipSeat_C") {
        Some(TargetSchema::RepLayout("BP_DropshipSeat_C"))
    } else if archetype.contains("BP_Seat_Bed_C") || archetype.contains("BP_Seat_Chair_C") {
        Some(TargetSchema::RepLayout("BP_SeatBase_C"))
    } else if archetype.contains("BP_Seat_Mount_C") {
        Some(TargetSchema::RepLayout("BP_Seat_Mount_C"))
    } else if archetype.contains("BP_Part_TOP_MK1_C") {
        Some(TargetSchema::RepLayout("BP_PartBase_C"))
    } else if archetype.contains("BP_Part_BTM_MK1_C") {
        Some(TargetSchema::RepLayout("BP_Part_BTM_MK1_C"))
    } else if archetype.contains("BP_Part_MID_MK1_C") {
        Some(TargetSchema::RepLayout("BP_Part_MID_MK1_C"))
    } else if archetype.contains("BP_Bed_Bedroll_C") {
        Some(TargetSchema::RepLayout("BP_BedBase_C"))
    } else if archetype.contains("BP_Portable_Beacon_C") {
        Some(TargetSchema::RepLayout("BP_Portable_Beacon_C"))
    } else if archetype.contains("BP_IcarusGameState") {
        Some(TargetSchema::RepLayout("AIcarusGameStateSurvival"))
    } else if archetype.contains("BP_Mount_Horse") {
        Some(TargetSchema::RepLayout("BP_Mount_Horse_C"))
    } else if archetype.contains("BP_Mount_WoollyMammoth")
        || archetype.contains("BP_Mount_Buffalo")
        || archetype.contains("BP_Mount_Moa")
    {
        Some(TargetSchema::RepLayout("BP_Mount_Base_C"))
    } else if archetype.contains("BP_Tamed_") || archetype.contains("BP_Tame_") {
        Some(TargetSchema::RepLayout("BP_Tame_Base_C"))
    } else if archetype.contains("BP_IcarusPlayerCharacterSurvival") {
        Some(TargetSchema::RepLayout(
            "BP_IcarusPlayerCharacterSurvival_C",
        ))
    } else if archetype.contains("BP_Building_")
        || archetype.contains("BP_Floor_")
        || archetype.contains("BP_Wall_")
        || archetype.contains("BP_Roof_")
        || archetype.contains("BP_Beam_")
        || archetype.contains("BP_Ramp_")
        || archetype.contains("BP_Stairs_")
    {
        Some(TargetSchema::RepLayout("BP_Building_Base_C"))
    } else {
        None
    }
}

/// Resolve a subobject's class or stably-named object path to a known native
/// or cooked Blueprint component schema.
fn classify_subobject_schema(
    target_name: &str,
    channel_archetype: Option<&str>,
) -> Option<TargetSchema> {
    if target_name == "StatContainer"
        || target_name == "Stat Container"
        || target_name == "IcarusStatContainer"
    {
        return Some(TargetSchema::StatContainer);
    }
    if target_name == "Inventory" {
        return Some(TargetSchema::Inventory);
    }
    if target_name == "BP_UIProjectionComponent_MountStatus"
        || target_name == "BP_UIProjectionComponent_MountStatus_C"
    {
        return Some(TargetSchema::UiProjectionMountStatus);
    }
    let schema = match target_name {
        "FLODActor" | "FLODActorComponent" => "UFLODActorComponent",
        "AudioContext"
        | "AudioContextComponent"
        | "FeedbackAudioComponent"
        | "BP_PlayerMovementAudioComponent"
        | "BP_PlayerMusicComponent" => "UAudioContextComponent",
        "ResourceNetwork"
        | "ResourceNetworkComponent"
        | "WaterNetwork"
        | "Water Network"
        | "EnergyNetwork"
        | "Energy Network"
        | "Fuel Network"
        | "TraitFuel" => "UResourceNetworkComponent",
        "TraitComponent"
        | "InventoryComponent"
        | "Meshable"
        | "MeshableComponent"
        | "Interactable"
        | "InteractableComponent"
        | "HighlightableComponent"
        | "Usable"
        | "UsableComponent"
        | "Deployable"
        | "DeployableComponent"
        | "Experience"
        | "TraitConsumable"
        | "TraitBattery"
        | "TraitThermal"
        | "TraitTargetable"
        | "TraitWeight"
        | "TraitShelter"
        | "TraitSeat"
        | "BP_Weight"
        | "BP_Weight_C"
        | "BP_HitableBehaviour_Building_C"
        | "BP_HitableBehaviour_VoxelResource" => "UTraitComponent",
        "Durable" | "TraitDurable" | "DurableComponent" => "UDurableComponent",
        "Itemable" | "TraitItemable" | "ItemableComponent" => "UItemableComponent",
        "Decayable" | "TraitDecayable" | "DecayableComponent" => "UDecayableComponent",
        "Fillable" | "TraitFillable" | "FillableComponent" => "UFillableComponent",
        "Transmutable" | "TraitTransmutable" | "TransmutableComponent" | "Processing" => {
            "UTransmutableComponent"
        }
        "Focusable" | "TraitFocusable" | "FocusableComponent" => "UFocusableComponent",
        "Actionable" | "TraitActionable" | "ActionableComponent" => "UActionableComponent",
        "ModifierState"
        | "ModifierStateComponent"
        | "BP_Modifier_Tamed_C"
        | "BP_Modifier_Base_C"
        | "BP_Modifier_Aura_Base_C"
        | "BP_ModifierStateBehaviour_AfflictionCold_C" => "UModifierStateComponent",
        "ActorState" | "CharacterStateComponent" => match channel_archetype {
            Some(arch)
                if arch.contains("Tamed_") || arch.contains("Tame_") || arch.contains("Mount_") =>
            {
                "UMountCharacterState"
            }
            Some(arch) if arch.contains("PlayerCharacter") => "UPlayerCharacterState",
            Some(arch) if arch.contains("Character") => return None,
            _ if target_name == "ActorState" => "UActorState",
            _ => return None,
        },
        "CharacterState" => "UCharacterState",
        "SurvivalCharacterState" => "USurvivalCharacterState",
        "PlayerCharacterState" => "UPlayerCharacterState",
        "MountCharacterState" => "UMountCharacterState",
        "WeatherManager" | "WeatherManagerComponent" | "WeatherComp" => "UWeatherManagerComponent",
        "IcarusWeatherAction"
        | "BP_WeatherAction_Base_C"
        | "Tier1_Calm_C"
        | "Tier1_Light_C"
        | "Tier2_Heavy_C"
        | "Tier3_Medium_C"
        | "Tier4_Extreme_C" => "UIcarusWeatherAction",
        "BP_UIProjectionComponent_Player" | "BP_UIProjectionComponent_Player_C" => {
            "BP_UIProjectionComponent_Player_C"
        }
        "InteractableBehaviour"
        | "BP_Interactable_Pickup_Item_C"
        | "BP_Interactable_AttachToPost_C"
        | "BP_Interactable_Cart_Interaction_C"
        | "BP_Interactable_Cart_Toggle_C"
        | "BP_Interactable_Claim_Tameable_C"
        | "BP_Interactable_DropShip_C"
        | "BP_Interactable_Enter_Seat_Mount_C"
        | "BP_Interactable_HitchingPostRelease_C"
        | "BP_Interactable_Interact_BedSetSpawn_C"
        | "BP_Interactable_Interact_BedSleep_C"
        | "BP_Interactable_Interact_Deployable_C"
        | "BP_Interactable_Interact_RocketPart_C"
        | "BP_Interactable_Interact_Toggle_Device_On_C"
        | "BP_Interactable_NoInteraction_C"
        | "BP_Interactable_OpenMountInventory_C"
        | "BP_Interactable_Pet_Tame_C"
        | "BP_Interactable_Pickup_Deployable_C"
        | "BP_Interactable_Teleport_C"
        | "BP_Interactable_Teleport_RetrieveLoot_C" => "UInteractableBehaviour",
        "BP_Interactable_Butcher_Animal_C" => "BP_Interactable_Butcher_Animal_C",
        "RootComponent" | "DefaultSceneRoot" | "SceneComponent" | "SeatExitPoints" | "Audio"
        | "LightAnchor" | "DropshipSpawnPoint" | "CapsuleComponent" | "CollisionCylinder"
        | "SkeletalMesh" | "SeatMesh" | "FMOD_MetaLoop" => "USceneComponent",
        "ChildActorComponent" | "LightSlot1" | "LightSlot2" | "ChildActor_Seat" => {
            "UChildActorComponent"
        }
        "EntranceMeshes" => "UStaticMeshComponent",
        "BPC_EnvironmentalBuildup" | "BPC_EnvironmentalBuildup_C" => "BPC_EnvironmentalBuildup_C",
        "FlammableActor" => "UFlammableActor",
        "BP_Flammable_Actor"
        | "BP_Flammable_Actor_C"
        | "BP_Flammable_Building_C"
        | "BP_Flammable_Player" => "BP_Flammable_Actor_C",
        "Stomach" | "StomachComponent" => "UStomachComponent",
        "BP_SwimmingComponent" | "BP_SwimmingComponent_C" => "BP_SwimmingComponent_C",
        "Genetics" | "GeneticsComponent" => "UGeneticsComponent",
        "BP_ShelteredComponent" | "BP_ShelteredComponent_C" => "BP_ShelteredComponent_C",
        "PlayerData"
        | "PlayerDataComponent"
        | "Highlightable"
        | "ThermalComponent"
        | "BP_PlayerBuildingPlacement"
        | "ConnectedPlayers"
        | "DialogueSystem"
        | "BP_NetworkProxyComponent"
        | "BackendProxyComponent"
        | "BestiaryManagerComponent"
        | "ResourceNetworkDataRequesterComponent"
        | "BP_CriticalHitComponent"
        | "BP_HuntingManager"
        | "BPC_AccumulationComponent"
        | "BP_UIProjectionComponent_MountTooltip"
        | "BP_JumpLerpComponent"
        | "GameplayTasksComponent" => "UActorComponent",
        "TraitBehaviour" => "UTraitBehaviour",
        "WeatherForecastBarComponent" => "UWeatherForecastBarComponent",
        _ => return None,
    };
    Some(TargetSchema::RepLayout(schema))
}

fn schema_commands(schema: &str) -> Vec<RepCommand> {
    let loc = MovementLevel::Whole;
    let char_loc = MovementLevel::Hundredths;
    let vel = MovementLevel::Whole;
    let rot = RotationLevel::Bytes;
    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };
    match schema {
        "AIcarusPlayerControllerSurvival" => {
            icarus_player_controller_survival_rep_layout(loc, vel, rot, tag_encoding)
        }
        "APlayerState" => player_state_rep_layout(loc, vel, rot),
        "BP_IcarusPlayerState_C" => bp_icarus_player_state_rep_layout(loc, vel, rot),
        "AIcarusGameStateSurvival" => icarus_game_state_survival_rep_layout(loc, vel, rot),
        "BP_Mount_Base_C" => bp_mount_base_rep_layout(char_loc, vel, rot, tag_encoding),
        "BP_Tame_Base_C" => bp_tame_base_rep_layout(char_loc, vel, rot, tag_encoding),
        "BP_Mount_Horse_C" => bp_mount_horse_rep_layout(char_loc, vel, rot, tag_encoding),
        "BP_IcarusPlayerCharacterSurvival_C" => {
            bp_icarus_player_character_survival_rep_layout(char_loc, vel, rot, tag_encoding)
        }
        "AActor" => actor_rep_layout(loc, vel, rot),
        "AWorldSettings" => world_settings_rep_layout(loc, vel, rot),
        "AFLOD" => flod_rep_layout(loc, vel, rot),
        "AWeatherController" => weather_controller_rep_layout(loc, vel, rot),
        "MapManager_C" => bp_map_manager_rep_layout(loc, vel, rot),
        "AQuest" => quest_rep_layout(loc, vel, rot, tag_encoding),
        "AQuestManager" => quest_manager_rep_layout(loc, vel, rot, tag_encoding),
        "AInventoryContainerManager" => {
            inventory_container_manager_rep_layout(loc, vel, rot, tag_encoding)
        }
        "APlayerHistoryTracker" => player_history_tracker_rep_layout(loc, vel, rot, tag_encoding),
        "ARadiationManager" => radiation_manager_rep_layout(loc, vel, rot, tag_encoding),
        "AWorldTalentManager" => world_talent_manager_rep_layout(loc, vel, rot, tag_encoding),
        "BP_AtmosphereController_C" => {
            bp_atmosphere_controller_rep_layout(loc, vel, rot, tag_encoding)
        }
        "BP_WorldObject_C" => bp_world_object_rep_layout(loc, vel, rot, tag_encoding),
        "BP_OilGeyser_C" => bp_oil_geyser_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Prebuilt_Base_C" => bp_prebuilt_base_rep_layout(loc, vel, rot, tag_encoding),
        "BP_WorldBossSpawner_C" => bp_world_boss_spawner_rep_layout(loc, vel, rot, tag_encoding),
        "BP_MetaDeposit_C" => bp_meta_deposit_rep_layout(loc, vel, rot, tag_encoding),
        "AVoxelResource" => voxel_resource_rep_layout(loc, vel, rot, tag_encoding),
        "BP_MainLevelTeleport_C" => bp_main_level_teleport_rep_layout(loc, vel, rot, tag_encoding),
        "BP_DropShip_C" => bp_dropship_rep_layout(loc, vel, rot, tag_encoding),
        "BP_SeatBase_C" => bp_seat_base_rep_layout(loc, vel, rot, tag_encoding),
        "BP_DropshipSeat_C" => bp_dropship_seat_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Seat_Mount_C" => bp_seat_mount_rep_layout(loc, vel, rot, tag_encoding),
        "BP_PartBase_C" => bp_part_base_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Part_BTM_MK1_C" => bp_part_btm_mk1_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Part_MID_MK1_C" => bp_part_mid_mk1_rep_layout(loc, vel, rot, tag_encoding),
        "BP_BedBase_C" => bp_bed_base_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Portable_Beacon_C" => bp_portable_beacon_rep_layout(loc, vel, rot, tag_encoding),
        "BP_Building_Base_C" => bp_building_base_rep_layout(loc, vel, rot, tag_encoding),
        "UWeatherForecastBarComponent" => weather_forecast_bar_component_rep_layout(),
        "UActorComponent" => actor_component_rep_layout(),
        "UFLODActorComponent" => flod_actor_component_rep_layout(),
        "UAudioContextComponent" => audio_context_component_rep_layout(),
        "UTraitComponent" => trait_component_rep_layout(),
        "UResourceNetworkComponent" => resource_network_component_rep_layout(),
        "UTraitBehaviour" => trait_behaviour_rep_layout(),
        "UDurableComponent" => durable_component_rep_layout(),
        "UItemableComponent" => itemable_component_rep_layout(),
        "UDecayableComponent" => decayable_component_rep_layout(),
        "UFillableComponent" => fillable_component_rep_layout(),
        "UTransmutableComponent" => transmutable_component_rep_layout(),
        "UFocusableComponent" => focusable_component_rep_layout(),
        "UActionableComponent" => actionable_component_rep_layout(),
        "UModifierStateComponent" => modifier_state_component_rep_layout(),
        "UActorState" => actor_state_rep_layout(),
        "UCharacterState" => character_state_rep_layout(),
        "USurvivalCharacterState" => survival_character_state_rep_layout(),
        "UPlayerCharacterState" => player_character_state_rep_layout(),
        "UMountCharacterState" => mount_character_state_rep_layout(),
        "UWeatherManagerComponent" => weather_manager_component_rep_layout(),
        "UIcarusWeatherAction" => icarus_weather_action_rep_layout(),
        "BP_UIProjectionComponent_MountStatus_C" => {
            bp_ui_projection_component_mount_status_rep_layout()
        }
        "BP_UIProjectionComponent_Player_C" => bp_ui_projection_component_player_rep_layout(),
        "UInteractableBehaviour" => interactable_behaviour_rep_layout(),
        "BP_Interactable_Butcher_Animal_C" => bp_interactable_butcher_animal_rep_layout(),
        "USceneComponent" => scene_component_rep_layout(),
        "UChildActorComponent" => child_actor_component_rep_layout(),
        "UStaticMeshComponent" => static_mesh_component_rep_layout(),
        "BPC_EnvironmentalBuildup_C" => bpc_environmental_buildup_rep_layout(),
        "UFlammableActor" => flammable_actor_rep_layout(),
        "BP_Flammable_Actor_C" => bp_flammable_actor_rep_layout(),
        "UStomachComponent" => stomach_component_rep_layout(),
        "BP_SwimmingComponent_C" => bp_swimming_component_rep_layout(),
        "UGeneticsComponent" => genetics_component_rep_layout(),
        "BP_ShelteredComponent_C" => bp_sheltered_component_rep_layout(),
        _ => unreachable!(),
    }
}

type RpcParameterLookup = fn(u32) -> Option<Vec<RpcParameter>>;

/// Resolve an actor schema name to its `FClassNetCache` `SerializeInt` bound
/// (`FieldsBase + Fields.Num() + 1`, F-708) and RPC parameter lookup when the
/// class sends actor net RPCs.
fn actor_rpc_schema(schema: &str) -> Option<(u32, RpcParameterLookup)> {
    match schema {
        "BP_IcarusPlayerState_C" => Some((
            BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND,
            bp_icarus_player_state_rpc_parameters,
        )),
        "BP_Mount_Base_C" => Some((
            BP_MOUNT_BASE_FIELD_INDEX_BOUND,
            icarus_character_rpc_parameters,
        )),
        "BP_Mount_Horse_C" => Some((
            BP_MOUNT_HORSE_FIELD_INDEX_BOUND,
            icarus_character_rpc_parameters,
        )),
        "BP_Tame_Base_C" => Some((
            BP_TAME_BASE_FIELD_INDEX_BOUND,
            icarus_character_rpc_parameters,
        )),
        "AIcarusPlayerControllerSurvival" => Some((
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            player_controller_rpc_parameters,
        )),
        _ => None,
    }
}

/// Decode and re-encode a standard actor or subobject block: optional
/// `FRepLayout::ReceiveProperties` handle stream (when `has_rep_layout` is set)
/// followed by zero or more `FClassNetCache` RPC field headers (`F-708`).
fn replay_rep_layout_and_rpc_block(
    schema_name: &str,
    payload: &Payload,
    has_rep_layout: bool,
    codec: &RepLayoutCodec,
) -> Result<(usize, usize, bool)> {
    let commands = schema_commands(schema_name);
    let mut reader = payload.reader();
    let mut writer = BitWriter::new();
    let prop_entries = if has_rep_layout {
        let stream = read_rep_properties(&mut reader, &commands, codec)?;
        let mut effective = *codec;
        effective.checksums = stream.terminator_checksum.is_some();
        write_rep_properties(&mut writer, &stream, &commands, &effective)?;
        stream.total_entries()
    } else {
        0
    };

    let mut rpc_calls = 0usize;
    if !reader.at_end() {
        let (index_bound, lookup) = actor_rpc_schema(schema_name)
            .with_context(|| format!("unexpected trailing field headers on {schema_name}"))?;
        while !reader.at_end() {
            let header = read_field_header(&mut reader, index_bound, codec.limits.max_opaque_bits)?;
            let params = lookup(header.index).with_context(|| {
                format!(
                    "unknown {schema_name} RPC field index {} ({} bits)",
                    header.index,
                    header.payload.bit_len()
                )
            })?;
            let args = parse_rpc_payload(&header.payload, &params, codec)
                .with_context(|| format!("{schema_name} RPC field index {}", header.index))?;
            let reencoded_rpc = encode_rpc_payload(&params, &args, codec)?;
            let rebuilt_header = FieldHeader {
                index: header.index,
                payload: reencoded_rpc,
            };
            write_field_header(&mut writer, &rebuilt_header, index_bound)?;
            rpc_calls += 1;
        }
    }
    let reencoded = Payload::from_writer(writer);
    Ok((prop_entries, rpc_calls, reencoded == *payload))
}

/// Decode and re-encode a `UIcarusStatContainer` (`Stat Container`) block:
/// optional `UActorComponent` rep-layout properties (when `has_rep_layout` is
/// set) followed by zero or more `ReplicatedStatArray` custom-delta field
/// headers (`index_bound = STAT_CONTAINER_FIELD_INDEX_BOUND`, F-705).
fn replay_stat_container_block(
    payload: &Payload,
    has_rep_layout: bool,
    codec: &RepLayoutCodec,
) -> Result<(usize, usize, bool)> {
    let layout = actor_component_rep_layout();
    let item_layout = FastArrayItemLayout::Delta(vec![RepCommand::StatPairRepState]);
    let mut reader = payload.reader();
    let mut writer = BitWriter::new();
    let prop_entries = if has_rep_layout {
        let props = read_rep_properties(&mut reader, &layout, codec)?;
        let mut effective = *codec;
        effective.checksums = props.terminator_checksum.is_some();
        write_rep_properties(&mut writer, &props, &layout, &effective)?;
        props.entries.len()
    } else {
        0
    };

    let mut stat_items = 0usize;
    while !reader.at_end() {
        let header = read_field_header(
            &mut reader,
            STAT_CONTAINER_FIELD_INDEX_BOUND,
            codec.limits.max_opaque_bits,
        )?;
        let mut field_reader = header.payload.reader();
        let delta_hdr = read_custom_delta_header(&mut field_reader, ENGINE_NET_VERSION, 1)?;
        let delta =
            read_fast_array_delta(&mut field_reader, &item_layout, codec, FAST_ARRAY_COUNT_CAP)?;
        ensure!(
            field_reader.at_end(),
            "trailing bits in Stat Container fast-array delta"
        );
        stat_items += delta.changed.len();

        let mut field_writer = BitWriter::new();
        write_custom_delta_header(&mut field_writer, &delta_hdr, ENGINE_NET_VERSION, 1)?;
        write_fast_array_delta(
            &mut field_writer,
            &delta,
            &item_layout,
            codec,
            FAST_ARRAY_COUNT_CAP,
        )?;
        let rebuilt_header = FieldHeader {
            index: header.index,
            payload: Payload::from_writer(field_writer),
        };
        write_field_header(
            &mut writer,
            &rebuilt_header,
            STAT_CONTAINER_FIELD_INDEX_BOUND,
        )?;
    }
    let reencoded = Payload::from_writer(writer);
    Ok((prop_entries, stat_items, reencoded == *payload))
}

/// Decode and re-encode a `UInventory` (`Inventory`) block: optional
/// `UInventory` rep-layout properties (when `has_rep_layout` is set) followed by
/// zero or more `Slots` (`FInventorySlotsFastArray`) custom-delta field headers
/// (`index_bound = INVENTORY_FIELD_INDEX_BOUND`).
fn replay_inventory_block(
    payload: &Payload,
    has_rep_layout: bool,
    codec: &RepLayoutCodec,
) -> Result<(usize, usize, bool)> {
    let layout = inventory_rep_layout();
    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };
    let item_layout = FastArrayItemLayout::Delta(inventory_slot_commands(tag_encoding));
    let mut reader = payload.reader();
    let mut writer = BitWriter::new();
    let prop_entries = if has_rep_layout {
        let props = read_rep_properties(&mut reader, &layout, codec)?;
        let mut effective = *codec;
        effective.checksums = props.terminator_checksum.is_some();
        write_rep_properties(&mut writer, &props, &layout, &effective)?;
        props.entries.len()
    } else {
        0
    };

    let mut slot_items = 0usize;
    while !reader.at_end() {
        let header = read_field_header(
            &mut reader,
            INVENTORY_FIELD_INDEX_BOUND,
            codec.limits.max_opaque_bits,
        )?;
        let mut field_reader = header.payload.reader();
        let delta_hdr = read_custom_delta_header(&mut field_reader, ENGINE_NET_VERSION, 1)?;
        let delta =
            read_fast_array_delta(&mut field_reader, &item_layout, codec, FAST_ARRAY_COUNT_CAP)?;
        ensure!(
            field_reader.at_end(),
            "trailing bits in Inventory fast-array delta"
        );
        slot_items += delta.changed.len();

        let mut field_writer = BitWriter::new();
        write_custom_delta_header(&mut field_writer, &delta_hdr, ENGINE_NET_VERSION, 1)?;
        write_fast_array_delta(
            &mut field_writer,
            &delta,
            &item_layout,
            codec,
            FAST_ARRAY_COUNT_CAP,
        )?;
        let rebuilt_header = FieldHeader {
            index: header.index,
            payload: Payload::from_writer(field_writer),
        };
        write_field_header(&mut writer, &rebuilt_header, INVENTORY_FIELD_INDEX_BOUND)?;
    }
    let reencoded = Payload::from_writer(writer);
    Ok((prop_entries, slot_items, reencoded == *payload))
}

/// Decode and re-encode a `BP_UIProjectionComponent_MountStatus_C` block:
/// optional rep-layout properties (when `has_rep_layout` is set) followed by
/// zero or more `ForceProjectionUpdate` 0-parameter net-RPC field headers
/// (`index = 5`, `index_bound = UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND`).
fn replay_ui_projection_mount_status_block(
    payload: &Payload,
    has_rep_layout: bool,
    codec: &RepLayoutCodec,
) -> Result<(usize, bool)> {
    let layout = bp_ui_projection_component_mount_status_rep_layout();
    let mut reader = payload.reader();
    let mut writer = BitWriter::new();
    let prop_entries = if has_rep_layout {
        let props = read_rep_properties(&mut reader, &layout, codec)?;
        let mut effective = *codec;
        effective.checksums = props.terminator_checksum.is_some();
        write_rep_properties(&mut writer, &props, &layout, &effective)?;
        props.entries.len()
    } else {
        0
    };

    while !reader.at_end() {
        let header = read_field_header(
            &mut reader,
            UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND,
            codec.limits.max_opaque_bits,
        )?;
        ensure!(
            header.index == 5 && header.payload.bit_len() == 0,
            "unexpected BP_UIProjectionComponent_MountStatus_C field index {} ({} bits)",
            header.index,
            header.payload.bit_len()
        );
        write_field_header(
            &mut writer,
            &header,
            UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND,
        )?;
    }
    let reencoded = Payload::from_writer(writer);
    Ok((prop_entries, reencoded == *payload))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Strict,
    ExportHypothesis,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::ExportHypothesis => "export_hypothesis",
        }
    }
}

/// One received bunch with where it came from. `sequence` is the unwrapped
/// reliable sequence, or the unwrapped packet sequence when unreliable.
struct Arrival {
    datagram: usize,
    ms: Option<u64>,
    sequence: i64,
    bunch: Bunch,
}

/// One partial bunch being assembled (one per channel, as the engine keeps).
struct Partial {
    reliable: bool,
    sequence: i64,
    header: BunchHeader,
    data: BitWriter,
    parts: usize,
    must_be_mapped: bool,
    /// Why the data is unknown, when a part's data could not be separated.
    opaque: Option<&'static str>,
}

#[derive(Default)]
struct ChannelState {
    reliable: Option<ReliableChannel<Arrival>>,
    name: Option<Name>,
    open: bool,
    partial: Option<Partial>,
    partial_reassembler: PartialBunchReassembler,
    partial_wire_parts: Vec<Bunch>,
    partial_export_payloads: Vec<Payload>,
    archetype_path: Option<String>,
}

/// A complete bunch handed to the channel: unpartitioned, or reassembled.
struct Complete {
    header: BunchHeader,
    payload: Option<Payload>,
    opaque: Option<&'static str>,
    parts: usize,
}

#[derive(Default, Serialize)]
struct Range {
    count: u64,
    min: Option<f64>,
    max: Option<f64>,
}

impl Range {
    fn add(&mut self, value: f64) {
        self.count += 1;
        self.min = Some(self.min.map_or(value, |min| min.min(value)));
        self.max = Some(self.max.map_or(value, |max| max.max(value)));
    }
}

#[derive(Default, Serialize)]
struct VectorStats {
    present: u64,
    raw: u64,
    quantized: u64,
    finite: u64,
    finite_within_1e6: u64,
    quantized_bits: Tally,
    x: Range,
    y: Range,
    z: Range,
}

impl VectorStats {
    fn add(&mut self, vector: Option<&SpawnVector>) {
        let Some(vector) = vector else {
            return;
        };
        self.present += 1;
        match vector {
            SpawnVector::Raw(_) => self.raw += 1,
            SpawnVector::Quantized { bits, .. } => {
                self.quantized += 1;
                bump(&mut self.quantized_bits, bits.to_string());
            }
        }
        let decoded = vector.decode();
        if decoded.iter().all(|value| value.is_finite()) {
            self.finite += 1;
            if decoded.iter().all(|value| value.abs() <= SANE_EXTENT) {
                self.finite_within_1e6 += 1;
            }
            for (range, value) in [&mut self.x, &mut self.y, &mut self.z]
                .into_iter()
                .zip(decoded)
            {
                range.add(f64::from(value));
            }
        }
    }
}

#[derive(Default, Serialize)]
struct SpawnStats {
    records: u64,
    actor_guid: Tally,
    archetype_guid: Tally,
    level_guid: Tally,
    archetype_resolved_by_earlier_export: u64,
    level_resolved_by_earlier_export: u64,
    location: VectorStats,
    rotation_present: u64,
    rotation_axes_present: Tally,
    pitch_degrees: Range,
    yaw_degrees: Range,
    roll_degrees: Range,
    scale: VectorStats,
    velocity: VectorStats,
}

#[derive(Default, Serialize)]
struct ActorStats {
    open_parsed: u64,
    open_failed: Tally,
    update_parsed: u64,
    update_failed: Tally,
}

#[derive(Default, Serialize)]
struct BlockStats {
    total: u64,
    by_kind: Tally,
    with_rep_layout: u64,
    payload_bits_total: u64,
    sub_object_guid: Tally,
    class_resolved_by_earlier_export: u64,
    class_unresolved: u64,
}

#[derive(Default, Serialize)]
struct RepReplayStats {
    /// Content blocks matched to a known native actor or component schema.
    typed_blocks_attempted: u64,
    typed_blocks_decoded: u64,
    typed_blocks_reencoded_exact: u64,
    typed_blocks_failed: Tally,
    /// `BlockStats::with_rep_layout` blocks (`has_rep_layout == true` in parsed
    /// actor bunches) matched, decoded and bit-identically re-encoded.
    rep_layout_blocks_attempted: u64,
    rep_layout_blocks_decoded: u64,
    rep_layout_blocks_reencoded_exact: u64,
    /// `has_rep_layout == false` blocks (custom-delta fast arrays, actor/component
    /// net RPCs, and zero-bit subobject creation/deletion headers) matched,
    /// decoded and bit-identically re-encoded.
    custom_delta_blocks_attempted: u64,
    custom_delta_blocks_decoded: u64,
    custom_delta_blocks_reencoded_exact: u64,
    /// Actor net-RPC `has_rep_layout == false` blocks decoded and re-encoded.
    actor_rpc_blocks_decoded: u64,
    actor_rpc_blocks_reencoded_exact: u64,
    /// Total net-RPC calls decoded across actor and component blocks.
    rpc_calls_decoded: u64,
    /// Zero-bit subobject creation/deletion blocks (`has_rep_layout == false`
    /// and `payload.bit_len() == 0`) matched, decoded and bit-identically
    /// re-encoded via `write_content_block` / `read_content_block`.
    subobject_lifecycle_blocks_attempted: u64,
    subobject_lifecycle_blocks_decoded: u64,
    subobject_lifecycle_blocks_reencoded_exact: u64,
    /// True when every `BlockStats::with_rep_layout` block was re-encoded
    /// bit-identically with zero unclassified targets and zero failures.
    rep_layout_coverage_complete: bool,
    /// True when every content block in `BlockStats::total` (`with_rep_layout`
    /// and `has_rep_layout == false`) was decoded and re-encoded bit-identically.
    all_blocks_coverage_complete: bool,
    /// Decoded and bit-identically re-encoded block counts by schema name.
    by_schema_decoded: Tally,
    by_schema_reencoded_exact: Tally,
    /// Total rep-layout property entries and `FStatPairRepState` fast-array
    /// items decoded across typed blocks.
    rep_entries_decoded: u64,
    stat_pair_items_decoded: u64,
    /// Channel 2 (`APlayerController`) open bunch decoded after the 8-bit
    /// `NetPlayerIndex` hook (`F-696`).
    player_controller_open_hook_blocks_decoded: u64,
    player_controller_open_hook_blocks_reencoded_exact: u64,
    /// Unclassified content blocks by resolved archetype / subobject target.
    unclassified_actor_targets: Tally,
    unclassified_subobject_targets: Tally,
}

#[derive(Default, Serialize)]
struct ExportStats {
    bunches: u64,
    /// The codec's F-572/F-709 reader (`PackageMapState::consume_export_prefix`)
    /// on the same prefix.
    codec_reader_parsed: u64,
    codec_reencoded_exact: u64,
    codec_reader_failed: Tally,
    package_map_entries: u64,
    parsed: u64,
    failed: Tally,
    guids: u64,
    guid_kind: Tally,
    flags: Tally,
    with_path: u64,
    without_path: u64,
    without_path_exported_earlier: u64,
    path_repeated_for_known_guid: u64,
    no_load: u64,
    with_checksum: u64,
    partial_export_bunches: u64,
    leftover_bits_after_exports: u64,
    max_path_units: usize,
}

#[derive(Default, Serialize)]
struct MustBeMappedStats {
    prefixes: u64,
    parsed: u64,
    codec_reencoded_exact: u64,
    failed: Tally,
    guids: u64,
    guids_exported_earlier: u64,
}

#[derive(Default, Serialize)]
struct PartialStats {
    initial: u64,
    continuation: u64,
    completed: u64,
    codec_reassembler_completed: u64,
    codec_resplit_exact: u64,
    completed_parts: Tally,
    completed_opaque: Tally,
    failures: Tally,
    in_progress_at_end: u64,
}

#[derive(Default, Serialize)]
struct ControlStats {
    bunches: u64,
    messages: Tally,
    unknown_ids: Tally,
    failed: Tally,
}

/// Pass-dependent results.
#[derive(Default, Serialize)]
struct PassStats {
    partial: PartialStats,
    control: ControlStats,
    /// Keyed by the channel's name key.
    actor: BTreeMap<String, ActorStats>,
    spawn: SpawnStats,
    blocks: BlockStats,
    rep_replay: RepReplayStats,
    exports: ExportStats,
    must_be_mapped: MustBeMappedStats,
    /// Open bunches parsed with the level reference present / absent.
    level_gate: Tally,
}

/// Pass-independent wire results.
#[derive(Default, Serialize)]
struct WireStats {
    datagrams: u64,
    packets_decoded: u64,
    packets_failed: Tally,
    packets_stale_or_duplicate: u64,
    packet_sequence_gaps: u64,
    packets_missing: u64,
    bunches: u64,
    bunches_reliable: u64,
    bunches_unreliable: u64,
    bunches_open: u64,
    bunches_close: u64,
    bunches_control_bit: u64,
    bunches_paused: u64,
    bunches_partial_initial: u64,
    bunches_partial_middle: u64,
    bunches_partial_final: u64,
    bunches_with_exports: u64,
    bunches_with_must_be_mapped: u64,
    bunches_on_unopened_channel: u64,
    reliable_delivered: u64,
    reliable_queued: u64,
    reliable_duplicates: u64,
    reliable_failed: Tally,
    reliable_held_at_end: u64,
    channel_indices: u64,
    max_channel_index: u32,
    channels_opened: u64,
    channels_closed: u64,
    channels_closed_dormant: u64,
    reopened_while_open: u64,
    close_reasons: Tally,
    opened_names: Tally,
    /// First reliable wire value per fresh channel index other than 0; one
    /// value is consistent with a shared initial reliable sequence (F-536).
    first_reliable_raw_distinct: u64,
}

struct Decoder<'a> {
    mode: Mode,
    config: PacketConfig,
    last_sequence: Option<u16>,
    packet_sequence: i64,
    channels: BTreeMap<u32, ChannelState>,
    first_raw: BTreeSet<u32>,
    package_map: PackageMapState,
    exported: BTreeMap<u32, String>,
    subobject_classes: BTreeMap<u32, String>,
    wire: WireStats,
    stats: PassStats,
    detail: &'a mut dyn Write,
    samples: Tally,
}

fn packet_config() -> PacketConfig {
    PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Client,
        limits: BunchLimits {
            max_channels: MAX_CHANNELS,
            ..PeerLabConfig::default().packet.packet.limits
        },
    }
}

impl<'a> Decoder<'a> {
    fn new(mode: Mode, detail: &'a mut dyn Write) -> Self {
        Self {
            mode,
            config: packet_config(),
            last_sequence: None,
            packet_sequence: 0,
            channels: BTreeMap::new(),
            first_raw: BTreeSet::new(),
            package_map: PackageMapState::new(),
            exported: BTreeMap::new(),
            subobject_classes: BTreeMap::new(),
            wire: WireStats::default(),
            stats: PassStats::default(),
            detail,
            samples: Tally::new(),
        }
    }

    fn write_detail(&mut self, mut record: Value) -> Result<()> {
        record["pass"] = json!(self.mode.label());
        writeln!(self.detail, "{record}")?;
        Ok(())
    }

    /// A bounded sample of one error class.
    fn sample(&mut self, area: &str, error_class: &str, datagram: usize) -> Result<()> {
        let key = format!("{area}: {error_class}");
        let seen = self.samples.entry(key).or_default();
        *seen += 1;
        if *seen <= SAMPLES_PER_CLASS {
            self.write_detail(
                json!({"kind":"error", "area":area, "class":error_class, "datagram":datagram}),
            )?;
        }
        Ok(())
    }

    fn feed(&mut self, datagram: usize, ms: Option<u64>, bytes: &[u8]) -> Result<()> {
        self.wire.datagrams += 1;
        let packet = match read_packet(bytes, &self.config) {
            Ok(packet) => packet,
            Err(error) => {
                let error_class = class(&error);
                bump(&mut self.wire.packets_failed, error_class.clone());
                return self.sample("packet", &error_class, datagram);
            }
        };
        self.wire.packets_decoded += 1;
        let sequence = packet.header.sequence;
        match self.last_sequence {
            None => self.packet_sequence = i64::from(sequence),
            Some(last) if is_newer(sequence, last) => {
                let delta = sequence_difference(sequence, last);
                if delta > 1 {
                    self.wire.packet_sequence_gaps += 1;
                    self.wire.packets_missing += u64::from(delta - 1);
                }
                self.packet_sequence += i64::from(delta);
            }
            Some(_) => {
                // The engine drops a stale or repeated packet whole.
                self.wire.packets_stale_or_duplicate += 1;
                return Ok(());
            }
        }
        self.last_sequence = Some(sequence);
        for bunch in packet.bunches {
            self.receive_raw(datagram, ms, bunch)?;
        }
        Ok(())
    }

    fn receive_raw(&mut self, datagram: usize, ms: Option<u64>, bunch: Bunch) -> Result<()> {
        let header = &bunch.header;
        let wire = &mut self.wire;
        wire.bunches += 1;
        if header.reliable {
            wire.bunches_reliable += 1;
        } else {
            wire.bunches_unreliable += 1;
        }
        wire.bunches_open += u64::from(header.open);
        wire.bunches_close += u64::from(header.close);
        wire.bunches_control_bit += u64::from(header.control);
        wire.bunches_paused += u64::from(header.paused);
        wire.bunches_with_exports += u64::from(header.has_package_map_exports);
        wire.bunches_with_must_be_mapped += u64::from(header.has_must_be_mapped_guids);
        if header.partial {
            match (header.partial_initial, header.partial_final) {
                (true, _) => wire.bunches_partial_initial += 1,
                (false, true) => wire.bunches_partial_final += 1,
                (false, false) => wire.bunches_partial_middle += 1,
            }
        }
        let index = header.channel_index;
        wire.max_channel_index = wire.max_channel_index.max(index);
        let fresh = !self.channels.contains_key(&index);
        let state = self.channels.entry(index).or_default();
        if fresh {
            self.wire.channel_indices += 1;
        }
        let Some(raw) = header.reliable_sequence_raw.filter(|_| header.reliable) else {
            let arrival = Arrival {
                datagram,
                ms,
                sequence: self.packet_sequence,
                bunch,
            };
            return self.next_bunch(arrival);
        };
        let channel = state.reliable.get_or_insert_with(|| {
            if index != 0 {
                self.first_raw.insert(raw);
            }
            // Seeded so the first observed value is the next expected one; the
            // capture starts after the handshake, so the true seed is unknown.
            ReliableChannel::new(raw + RELIABLE_SEQUENCE_LIMIT - 1, 0)
        });
        let sequence = reconstruct_reliable_sequence(raw, channel.in_reliable())?;
        let arrival = Arrival {
            datagram,
            ms,
            sequence,
            bunch,
        };
        match channel.receive_reliable(raw, arrival) {
            Ok(ReliableReceive::Delivered(batch)) => {
                self.wire.reliable_delivered += batch.len() as u64;
                for arrival in batch {
                    self.next_bunch(arrival)?;
                }
            }
            Ok(ReliableReceive::Queued) => self.wire.reliable_queued += 1,
            Ok(ReliableReceive::Duplicate) => self.wire.reliable_duplicates += 1,
            Err(error) => {
                let error_class = class(&error);
                bump(&mut self.wire.reliable_failed, error_class.clone());
                self.sample("reliable", &error_class, datagram)?;
            }
        }
        Ok(())
    }

    /// The engine's next-bunch step: channel open, export prefix, partial
    /// merge, then the channel's handler; close last.
    fn next_bunch(&mut self, arrival: Arrival) -> Result<()> {
        let header = arrival.bunch.header.clone();
        let index = header.channel_index;
        let state = self.channels.entry(index).or_default();
        if header.open {
            if state.open {
                self.wire.reopened_while_open += 1;
            }
            state.open = true;
            state.partial = None;
            state.partial_reassembler.clear();
            state.partial_wire_parts.clear();
            state.partial_export_payloads.clear();
            self.wire.channels_opened += 1;
            bump(
                &mut self.wire.opened_names,
                name_key(header.channel_name.as_ref()),
            );
        } else if !state.open && index != 0 {
            self.wire.bunches_on_unopened_channel += 1;
        }
        if let Some(name) = &header.channel_name {
            state.name = Some(name.clone());
        }

        // Export prefix: consumed per raw bunch, partial or not.
        let mut reader = arrival.bunch.payload.reader();
        let mut opaque = None;
        let mut exports_consumed = false;
        if header.has_package_map_exports {
            self.stats.exports.bunches += 1;
            self.stats.exports.partial_export_bunches += u64::from(header.partial);
            match self.mode {
                Mode::Strict => opaque = Some("export prefix unparsed (layout undocumented)"),
                Mode::ExportHypothesis => {
                    match self.package_map.consume_export_prefix(
                        &mut arrival.bunch.payload.reader(),
                        &actor_codec(true),
                    ) {
                        Ok(refs) => {
                            self.stats.exports.codec_reader_parsed += 1;
                            if self
                                .package_map
                                .encode_export_bunch_payload(&refs, &actor_codec(true))
                                .is_ok_and(|reencoded| reencoded == arrival.bunch.payload)
                            {
                                self.stats.exports.codec_reencoded_exact += 1;
                            }
                        }
                        Err(error) => {
                            bump(&mut self.stats.exports.codec_reader_failed, class(&error))
                        }
                    }
                    match read_export_prefix(&mut reader) {
                        Ok(references) => {
                            exports_consumed = true;
                            self.stats.exports.parsed += 1;
                            if reader.remaining() > 0 {
                                self.stats.exports.leftover_bits_after_exports += 1;
                            }
                            for reference in &references {
                                self.record_export(reference, arrival.datagram)?;
                            }
                        }
                        Err(error) => {
                            let error_class = class(&error);
                            bump(&mut self.stats.exports.failed, error_class.clone());
                            self.sample("exports", &error_class, arrival.datagram)?;
                            opaque = Some("export prefix failed");
                        }
                    }
                }
            }
        }

        let complete = if header.partial {
            let data = opaque.is_none().then(|| rest(&mut reader));
            self.merge_partial(&arrival, data, opaque)
        } else {
            let mut header = header.clone();
            // In the strict pass the export flag stays set, so the actor codec
            // itself rejects the bunch; its whole payload is passed on.
            let payload = if exports_consumed {
                header.has_package_map_exports = false;
                Some(rest(&mut reader))
            } else if opaque.is_some() && self.mode == Mode::Strict {
                Some(arrival.bunch.payload.clone())
            } else {
                opaque.is_none().then(|| rest(&mut reader))
            };
            Some(Complete {
                header,
                opaque: payload.is_none().then_some(opaque).flatten(),
                payload,
                parts: 1,
            })
        };
        if let Some(complete) = complete {
            self.dispatch(index, &arrival, complete)?;
        }
        if header.close {
            let state = self.channels.entry(index).or_default();
            state.open = false;
            state.partial = None;
            state.partial_reassembler.clear();
            state.partial_wire_parts.clear();
            state.partial_export_payloads.clear();
            self.wire.channels_closed += 1;
            if header
                .parsed_close_reason()
                .ok()
                .flatten()
                .is_some_and(ChannelCloseReason::is_dormant)
            {
                self.wire.channels_closed_dormant += 1;
            }
            bump(
                &mut self.wire.close_reasons,
                header
                    .close_reason
                    .map_or("none".into(), |reason| reason.to_string()),
            );
        }
        Ok(())
    }

    fn record_export(&mut self, reference: &Exported, datagram: usize) -> Result<()> {
        let exports = &mut self.stats.exports;
        exports.guids += 1;
        bump(
            &mut exports.guid_kind,
            format!("{:?}", guid_kind(reference.guid)),
        );
        if reference.guid == 0 {
            return Ok(());
        }
        bump(&mut exports.flags, format!("{:#04x}", reference.flags));
        exports.no_load += u64::from(reference.flags & EXPORT_NO_LOAD != 0);
        let (Some(outer), Some(path)) = (&reference.outer, &reference.path) else {
            exports.without_path += 1;
            if self.exported.contains_key(&reference.guid) {
                exports.without_path_exported_earlier += 1;
            }
            return Ok(());
        };
        exports.with_path += 1;
        exports.with_checksum += u64::from(reference.checksum.is_some());
        exports.max_path_units = exports.max_path_units.max(path.encode_utf16().count());
        if self.exported.insert(reference.guid, path.clone()).is_some() {
            exports.path_repeated_for_known_guid += 1;
        }
        self.write_detail(
            json!({"kind":"export", "datagram":datagram, "guid":reference.guid,
            "path":path, "outer_guid":outer.guid, "flags":reference.flags,
            "checksum":reference.checksum.is_some()}),
        )?;
        self.record_export(outer, datagram)
    }

    /// The engine's partial merge (`UChannel::ReceivedNextBunch` `0x1437c5500`,
    /// F-710): one assembly per channel; reliable parts must be consecutive,
    /// unreliable parts share or follow the packet sequence; an export-only
    /// part adds no data. Returns the complete bunch on the final part.
    fn merge_partial(
        &mut self,
        arrival: &Arrival,
        data: Option<Payload>,
        opaque: Option<&'static str>,
    ) -> Option<Complete> {
        let header = &arrival.bunch.header;
        let partial_stats = &mut self.stats.partial;
        let state = self.channels.entry(header.channel_index).or_default();
        if header.partial_initial {
            state.partial_wire_parts.clear();
            state.partial_export_payloads.clear();
        }
        state.partial_wire_parts.push(arrival.bunch.clone());
        if header.has_package_map_exports {
            state
                .partial_export_payloads
                .push(arrival.bunch.payload.clone());
        }
        if let Some(payload_after_exports) = &data
            && let Ok(PartialReceive::Complete { mut bunch, .. }) =
                state.partial_reassembler.receive_part(
                    arrival.sequence,
                    header,
                    payload_after_exports,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
        {
            partial_stats.codec_reassembler_completed += 1;
            if let Some(first) = state.partial_wire_parts.first() {
                bunch.header.reliable_sequence_raw = first.header.reliable_sequence_raw;
            }
            if let Ok(max_single_bits) = max_single_bunch_size_bits(1024, 2)
                && let Ok(resplit) = split_outgoing_bunch(
                    &bunch,
                    &state.partial_export_payloads,
                    max_single_bits,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
                && resplit == state.partial_wire_parts
            {
                partial_stats.codec_resplit_exact += 1;
            }
            state.partial_wire_parts.clear();
            state.partial_export_payloads.clear();
        }
        let aligned = data
            .as_ref()
            .is_none_or(|payload| payload.bit_len().is_multiple_of(8));
        if header.partial_initial {
            partial_stats.initial += 1;
            if let Some(existing) = &state.partial {
                if existing.reliable && !header.reliable {
                    bump(
                        &mut partial_stats.failures,
                        "unreliable initial ignored during reliable assembly",
                    );
                    return None;
                }
                bump(
                    &mut partial_stats.failures,
                    "assembly abandoned by new initial",
                );
            }
            if !aligned && !header.partial_final {
                bump(&mut partial_stats.failures, "initial part not byte aligned");
            }
            let mut writer = BitWriter::new();
            if let Some(payload) = &data {
                append(&mut writer, payload);
            }
            state.partial = Some(Partial {
                reliable: header.reliable,
                sequence: arrival.sequence,
                header: header.clone(),
                data: writer,
                parts: 1,
                must_be_mapped: header.has_must_be_mapped_guids,
                opaque,
            });
        } else {
            partial_stats.continuation += 1;
            let Some(partial) = state.partial.as_mut() else {
                bump(&mut partial_stats.failures, "continuation without initial");
                return None;
            };
            let next = arrival.sequence == partial.sequence + 1;
            let matches = if partial.reliable {
                next
            } else {
                next || arrival.sequence == partial.sequence
            };
            if !matches || partial.reliable != header.reliable {
                bump(&mut partial_stats.failures, "sequence or reliability break");
                state.partial = None;
                return None;
            }
            if !aligned && !header.partial_final {
                bump(
                    &mut partial_stats.failures,
                    "non-final part not byte aligned",
                );
            }
            if let Some(payload) = &data {
                append(&mut partial.data, payload);
            }
            partial.opaque = partial.opaque.or(opaque);
            partial.sequence = arrival.sequence;
            partial.parts += 1;
            partial.must_be_mapped |= header.has_must_be_mapped_guids;
        }
        if !header.partial_final {
            return None;
        }
        let partial = state.partial.take()?;
        partial_stats.completed += 1;
        bump(
            &mut partial_stats.completed_parts,
            partial.parts.to_string(),
        );
        if let Some(reason) = partial.opaque {
            bump(&mut partial_stats.completed_opaque, reason);
        }
        let mut merged = partial.header;
        merged.partial = false;
        merged.partial_initial = false;
        merged.partial_final = false;
        merged.has_package_map_exports = false;
        merged.has_must_be_mapped_guids = partial.must_be_mapped;
        merged.close = header.close;
        merged.close_reason = header.close_reason;
        Some(Complete {
            header: merged,
            payload: partial
                .opaque
                .is_none()
                .then(|| Payload::from_writer(partial.data)),
            opaque: partial.opaque,
            parts: partial.parts,
        })
    }

    fn dispatch(&mut self, index: u32, arrival: &Arrival, complete: Complete) -> Result<()> {
        let name = self
            .channels
            .get(&index)
            .and_then(|state| state.name.clone());
        let control = match &name {
            Some(Name::Hardcoded(name_index)) => *name_index == CONTROL_NAME_INDEX,
            Some(Name::String { .. }) => false,
            None => index == 0,
        };
        if control {
            return self.control(arrival, complete);
        }
        let key = name_key(name.as_ref());
        let open = complete.header.open;
        let parsed = self.actor(arrival, &complete, name.as_ref());
        let stats = self.stats.actor.entry(key.clone()).or_default();
        match parsed {
            Ok(()) if open => stats.open_parsed += 1,
            Ok(()) => stats.update_parsed += 1,
            Err(error) => {
                let error_class = class(&error);
                let tally = if open {
                    &mut stats.open_failed
                } else {
                    &mut stats.update_failed
                };
                bump(tally, error_class.clone());
                let area = format!("actor {key} {}", if open { "open" } else { "update" });
                self.sample(&area, &error_class, arrival.datagram)?;
            }
        }
        Ok(())
    }

    fn control(&mut self, arrival: &Arrival, complete: Complete) -> Result<()> {
        self.stats.control.bunches += 1;
        let Some(payload) = complete.payload else {
            bump(
                &mut self.stats.control.failed,
                complete.opaque.unwrap_or("no payload"),
            );
            return Ok(());
        };
        match read_control_messages(&mut payload.reader(), &ControlCodec::default()) {
            Ok(decoded) => {
                for item in decoded {
                    match item {
                        Decoded::Message(message) => {
                            bump(&mut self.stats.control.messages, message.name());
                        }
                        Decoded::Unknown { id } => {
                            bump(&mut self.stats.control.unknown_ids, id.to_string());
                        }
                    }
                }
            }
            Err(error) => {
                let error_class = class(&error);
                bump(&mut self.stats.control.failed, error_class.clone());
                self.sample("control", &error_class, arrival.datagram)?;
            }
        }
        Ok(())
    }

    /// Parse an actor-candidate bunch; `Ok` means the whole body was read to
    /// its exact end.
    fn actor(&mut self, arrival: &Arrival, complete: &Complete, name: Option<&Name>) -> Result<()> {
        let Some(payload) = &complete.payload else {
            bail!("data unknown: {}", complete.opaque.unwrap_or("no payload"));
        };
        let mut bunch = Bunch {
            header: complete.header.clone(),
            payload: payload.clone(),
        };
        if self.mode == Mode::ExportHypothesis && bunch.header.has_must_be_mapped_guids {
            self.stats.must_be_mapped.prefixes += 1;
            let (guids, resolved, remainder) =
                match strip_must_be_mapped(&self.package_map, &bunch.payload) {
                    Ok(stripped) => stripped,
                    Err(error) => {
                        bump(&mut self.stats.must_be_mapped.failed, class(&error));
                        return Err(error.context("must-be-mapped prefix"));
                    }
                };
            let reencoded_exact = self
                .package_map
                .prepend_must_be_mapped_guids(&guids, &remainder, &actor_codec(true))
                .is_ok_and(|reencoded| reencoded == bunch.payload);
            let must_be_mapped = &mut self.stats.must_be_mapped;
            must_be_mapped.parsed += 1;
            if reencoded_exact {
                must_be_mapped.codec_reencoded_exact += 1;
            }
            must_be_mapped.guids += guids.len() as u64;
            must_be_mapped.guids_exported_earlier += resolved as u64;
            bunch.header.has_must_be_mapped_guids = false;
            bunch.payload = remainder;
        }
        let codec = actor_codec(true);
        if bunch.header.open && self.mode == Mode::ExportHypothesis {
            let with_level = parse_actor_bunch(&bunch, &codec).is_ok();
            let without_level = parse_actor_bunch(&bunch, &actor_codec(false)).is_ok();
            bump(
                &mut self.stats.level_gate,
                match (with_level, without_level) {
                    (true, true) => "both",
                    (true, false) => "with_level_only",
                    (false, true) => "without_level_only",
                    (false, false) => "neither",
                },
            );
        }
        let body = match parse_actor_bunch(&bunch, &codec) {
            Ok(body) => body,
            Err(error) => {
                if bunch.header.open && !bunch.header.has_package_map_exports {
                    self.open_failure(arrival, &bunch, &codec)?;
                }
                return Err(error);
            }
        };
        self.record_body(arrival, complete, &body, name)
    }

    /// Where a failed open bunch stops: the spawn record's extent and each
    /// content block header read after it, for the work-only detail file.
    /// Also checks the `APlayerController` 8-bit `NetPlayerIndex` open-bunch
    /// hook (`F-696`) for `rep_replay`.
    fn open_failure(
        &mut self,
        arrival: &Arrival,
        bunch: &Bunch,
        codec: &ActorChannelCodec,
    ) -> Result<()> {
        let mut reader = bunch.payload.reader();
        let spawn = read_spawn(&mut reader, codec);
        let spawn_end = reader.position();
        if let Ok(spawn_rec) = &spawn
            && let Some(dynamic) = &spawn_rec.dynamic
            && let Some(arch) = self.resolve(&dynamic.archetype).cloned()
        {
            self.channels
                .entry(bunch.header.channel_index)
                .or_default()
                .archetype_path = Some(arch.clone());
            // Check the 8-bit `NetPlayerIndex` hook on PlayerController open bunches (F-696, F-709).
            if arch.contains("PlayerController")
                && let Ok(pc_body) = parse_player_controller_open_bunch(bunch, codec)
            {
                let body_reencoded_exact =
                    encode_player_controller_open_bunch_body(&pc_body, codec)
                        .is_ok_and(|reencoded| reencoded == bunch.payload);
                for block in &pc_body.blocks {
                    let (object_path, class_path) = self.resolve_block_paths(block);
                    if let Some((_schema_name, exact)) = self.replay_block(
                        bunch.header.channel_index,
                        block,
                        object_path.as_deref(),
                        class_path.as_deref(),
                        false,
                    ) {
                        self.stats
                            .rep_replay
                            .player_controller_open_hook_blocks_decoded += 1;
                        if exact && body_reencoded_exact {
                            self.stats
                                .rep_replay
                                .player_controller_open_hook_blocks_reencoded_exact += 1;
                        }
                    }
                }
            }
        }
        let mut blocks = Vec::new();
        if spawn.is_ok() {
            while !reader.at_end() && blocks.len() < 64 {
                let start = reader.position();
                match read_content_block(&mut reader, codec) {
                    Ok(block) => blocks.push(json!({"start":start, "summary":block.summary()})),
                    Err(error) => {
                        blocks.push(json!({"start":start, "error":format!("{error:#}")}));
                        break;
                    }
                }
            }
        }
        self.write_detail(json!({"kind":"open_failure", "datagram":arrival.datagram,
            "channel":bunch.header.channel_index, "reliable":bunch.header.reliable,
            "payload_bits":bunch.payload.bit_len(),
            "spawn":spawn.as_ref().map(|spawn| spawn.summary()).map_err(|error| format!("{error:#}")),
            "spawn_end_bit":spawn_end, "blocks":blocks,
            "payload_hex":bunch.payload.bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>()}))
    }

    fn resolve(&self, reference: &NetGuidRef) -> Option<&String> {
        self.exported.get(&reference.guid)
    }

    fn resolve_block_paths(&mut self, block: &ContentBlock) -> (Option<String>, Option<String>) {
        let BlockTarget::SubObject { object, class } = &block.target else {
            return (None, None);
        };
        let object_path = self.exported.get(&object.guid).cloned();
        let class_path = match class {
            SubObjectClass::Class(class_ref) => {
                let resolved = self.exported.get(&class_ref.guid).cloned();
                if let Some(path) = &resolved {
                    self.subobject_classes.insert(object.guid, path.clone());
                }
                resolved
            }
            SubObjectClass::StablyNamed | SubObjectClass::Delete => {
                self.subobject_classes.get(&object.guid).cloned()
            }
        };
        (object_path, class_path)
    }

    fn replay_block(
        &mut self,
        channel_index: u32,
        block: &ContentBlock,
        object_path: Option<&str>,
        class_path: Option<&str>,
        in_record_body: bool,
    ) -> Option<(&'static str, bool)> {
        if in_record_body && block.has_rep_layout {
            self.stats.rep_replay.rep_layout_blocks_attempted += 1;
        }
        let channel_arch = self
            .channels
            .get(&channel_index)
            .and_then(|state| state.archetype_path.as_deref());
        let target_schema = match &block.target {
            BlockTarget::Actor => {
                let Some(arch) = channel_arch else {
                    bump(
                        &mut self.stats.rep_replay.unclassified_actor_targets,
                        "<unknown_channel_archetype>",
                    );
                    return None;
                };
                let Some(schema) = classify_actor_schema(arch) else {
                    bump(&mut self.stats.rep_replay.unclassified_actor_targets, arch);
                    return None;
                };
                schema
            }
            BlockTarget::SubObject { class, .. } => {
                let Some(target_name) = class_path.or(object_path) else {
                    bump(
                        &mut self.stats.rep_replay.unclassified_subobject_targets,
                        "<unresolved_subobject>",
                    );
                    return None;
                };
                let Some(schema) = classify_subobject_schema(target_name, channel_arch) else {
                    bump(
                        &mut self.stats.rep_replay.unclassified_subobject_targets,
                        target_name,
                    );
                    return None;
                };
                if !block.has_rep_layout && block.payload.bit_len() == 0 {
                    let lifecycle_schema = if matches!(class, SubObjectClass::Delete) {
                        "SubObjectDelete"
                    } else {
                        match schema {
                            TargetSchema::RepLayout(name) => name,
                            TargetSchema::StatContainer => "UIcarusStatContainer",
                            TargetSchema::Inventory => "UInventory",
                            TargetSchema::UiProjectionMountStatus => {
                                "BP_UIProjectionComponent_MountStatus_C"
                            }
                            TargetSchema::SubObjectLifecycle(name) => name,
                        }
                    };
                    TargetSchema::SubObjectLifecycle(lifecycle_schema)
                } else {
                    schema
                }
            }
        };

        let codec = rep_codec();
        let replay = &mut self.stats.rep_replay;
        replay.typed_blocks_attempted += 1;
        if in_record_body && !block.has_rep_layout {
            if matches!(target_schema, TargetSchema::SubObjectLifecycle(_)) {
                replay.subobject_lifecycle_blocks_attempted += 1;
            } else {
                replay.custom_delta_blocks_attempted += 1;
            }
        }
        match target_schema {
            TargetSchema::SubObjectLifecycle(schema_name) => {
                let block_codec = actor_codec(true);
                let mut writer = BitWriter::new();
                match write_content_block(&mut writer, block, &block_codec).and_then(|()| {
                    let payload = Payload::from_writer(writer);
                    let mut reader = payload.reader();
                    let decoded = read_content_block(&mut reader, &block_codec)?;
                    ensure!(
                        reader.at_end(),
                        "trailing bits after re-encoded subobject lifecycle block"
                    );
                    Ok(decoded == *block)
                }) {
                    Ok(exact) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        if in_record_body {
                            replay.subobject_lifecycle_blocks_decoded += 1;
                            replay.subobject_lifecycle_blocks_reencoded_exact += u64::from(exact);
                        }
                        if exact {
                            replay.typed_blocks_reencoded_exact += 1;
                            bump(&mut replay.by_schema_reencoded_exact, schema_name);
                        }
                        Some((schema_name, exact))
                    }
                    Err(error) => {
                        bump(
                            &mut replay.typed_blocks_failed,
                            format!("{schema_name}: {}", class(&error)),
                        );
                        None
                    }
                }
            }
            TargetSchema::StatContainer => {
                let schema_name = "UIcarusStatContainer";
                match replay_stat_container_block(&block.payload, block.has_rep_layout, &codec) {
                    Ok((prop_entries, stat_items, exact)) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        replay.rep_entries_decoded += prop_entries as u64;
                        replay.stat_pair_items_decoded += stat_items as u64;
                        if exact {
                            replay.typed_blocks_reencoded_exact += 1;
                            bump(&mut replay.by_schema_reencoded_exact, schema_name);
                        }
                        Some((schema_name, exact))
                    }
                    Err(error) => {
                        bump(
                            &mut replay.typed_blocks_failed,
                            format!("{schema_name}: {}", class(&error)),
                        );
                        None
                    }
                }
            }
            TargetSchema::Inventory => {
                let schema_name = "UInventory";
                match replay_inventory_block(&block.payload, block.has_rep_layout, &codec) {
                    Ok((prop_entries, _slot_items, exact)) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        replay.rep_entries_decoded += prop_entries as u64;
                        if exact {
                            replay.typed_blocks_reencoded_exact += 1;
                            bump(&mut replay.by_schema_reencoded_exact, schema_name);
                        }
                        Some((schema_name, exact))
                    }
                    Err(error) => {
                        bump(
                            &mut replay.typed_blocks_failed,
                            format!("{schema_name}: {}", class(&error)),
                        );
                        None
                    }
                }
            }
            TargetSchema::UiProjectionMountStatus => {
                let schema_name = "BP_UIProjectionComponent_MountStatus_C";
                match replay_ui_projection_mount_status_block(
                    &block.payload,
                    block.has_rep_layout,
                    &codec,
                ) {
                    Ok((prop_entries, exact)) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        replay.rep_entries_decoded += prop_entries as u64;
                        if !block.has_rep_layout {
                            replay.rpc_calls_decoded += 1;
                        }
                        if exact {
                            replay.typed_blocks_reencoded_exact += 1;
                            bump(&mut replay.by_schema_reencoded_exact, schema_name);
                        }
                        Some((schema_name, exact))
                    }
                    Err(error) => {
                        bump(
                            &mut replay.typed_blocks_failed,
                            format!("{schema_name}: {}", class(&error)),
                        );
                        None
                    }
                }
            }
            TargetSchema::RepLayout(schema_name) => {
                match replay_rep_layout_and_rpc_block(
                    schema_name,
                    &block.payload,
                    block.has_rep_layout,
                    &codec,
                ) {
                    Ok((prop_entries, rpc_calls, exact)) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        replay.rep_entries_decoded += prop_entries as u64;
                        replay.rpc_calls_decoded += rpc_calls as u64;
                        if in_record_body
                            && matches!(block.target, BlockTarget::Actor)
                            && !block.has_rep_layout
                        {
                            replay.actor_rpc_blocks_decoded += 1;
                            replay.actor_rpc_blocks_reencoded_exact += u64::from(exact);
                        }
                        if exact {
                            replay.typed_blocks_reencoded_exact += 1;
                            bump(&mut replay.by_schema_reencoded_exact, schema_name);
                        }
                        Some((schema_name, exact))
                    }
                    Err(error) => {
                        bump(
                            &mut replay.typed_blocks_failed,
                            format!("{schema_name}: {}", class(&error)),
                        );
                        None
                    }
                }
            }
        }
    }

    fn record_body(
        &mut self,
        arrival: &Arrival,
        complete: &Complete,
        body: &ActorBunchBody,
        name: Option<&Name>,
    ) -> Result<()> {
        let spawn = body
            .spawn
            .as_ref()
            .map(|spawn| self.record_spawn(complete.header.channel_index, spawn))
            .transpose()?;
        let mut blocks = Vec::new();
        for block in &body.blocks {
            let (object_path, class_path) = self.resolve_block_paths(block);
            let stats = &mut self.stats.blocks;
            stats.total += 1;
            stats.with_rep_layout += u64::from(block.has_rep_layout);
            stats.payload_bits_total += block.payload.bit_len() as u64;
            let summary = block.summary();
            bump(&mut stats.by_kind, format!("{:?}", summary.kind));
            if let BlockTarget::SubObject { object, class } = &block.target {
                bump(&mut stats.sub_object_guid, format!("{:?}", object.kind()));
                if matches!(class, SubObjectClass::Class(_)) {
                    if class_path.is_some() {
                        stats.class_resolved_by_earlier_export += 1;
                    } else {
                        stats.class_unresolved += 1;
                    }
                }
            }
            let replayed = self.replay_block(
                complete.header.channel_index,
                block,
                object_path.as_deref(),
                class_path.as_deref(),
                true,
            );
            let replay = &mut self.stats.rep_replay;
            if let Some((_, exact)) = replayed {
                if block.has_rep_layout {
                    replay.rep_layout_blocks_decoded += 1;
                    replay.rep_layout_blocks_reencoded_exact += u64::from(exact);
                } else if block.payload.bit_len() > 0 {
                    replay.custom_delta_blocks_decoded += 1;
                    replay.custom_delta_blocks_reencoded_exact += u64::from(exact);
                }
            }
            blocks.push(json!({
                "summary": summary,
                "object_path": object_path,
                "class_path": class_path,
                "schema": replayed.map(|(s, _)| s),
                "reencoded_exact": replayed.map(|(_, exact)| exact),
            }));
        }
        self.write_detail(json!({"kind":"actor_bunch", "datagram":arrival.datagram,
            "ms":arrival.ms, "channel":complete.header.channel_index,
            "channel_name":name.map(name_text), "open":complete.header.open,
            "close":complete.header.close, "reliable":complete.header.reliable,
            "parts":complete.parts, "spawn":spawn, "blocks":blocks}))
    }

    fn record_spawn(&mut self, channel_index: u32, spawn: &SpawnRecord) -> Result<Value> {
        let archetype_path = spawn
            .dynamic
            .as_ref()
            .and_then(|dynamic| self.resolve(&dynamic.archetype).cloned())
            .or_else(|| self.resolve(&spawn.actor).cloned());
        if let Some(arch) = &archetype_path {
            self.channels
                .entry(channel_index)
                .or_default()
                .archetype_path = Some(arch.clone());
        }
        let level_path = spawn
            .dynamic
            .as_ref()
            .and_then(|dynamic| dynamic.level.as_ref())
            .and_then(|level| self.resolve(level).cloned());
        let stats = &mut self.stats.spawn;
        stats.records += 1;
        bump(&mut stats.actor_guid, format!("{:?}", spawn.actor.kind()));
        let Some(dynamic) = &spawn.dynamic else {
            return Ok(json!({"actor_guid":spawn.actor.guid, "kind":"static"}));
        };
        bump(
            &mut stats.archetype_guid,
            format!("{:?}", dynamic.archetype.kind()),
        );
        if let Some(level) = &dynamic.level {
            bump(&mut stats.level_guid, format!("{:?}", level.kind()));
        }
        stats.archetype_resolved_by_earlier_export += u64::from(archetype_path.is_some());
        stats.level_resolved_by_earlier_export += u64::from(level_path.is_some());
        stats.location.add(dynamic.location.as_ref());
        stats.scale.add(dynamic.scale.as_ref());
        stats.velocity.add(dynamic.velocity.as_ref());
        let rotation = dynamic.rotation.map(|rotation| rotation.degrees());
        if let Some(rotation) = dynamic.rotation {
            stats.rotation_present += 1;
            bump(
                &mut stats.rotation_axes_present,
                rotation.summary().axes_present.to_string(),
            );
            let degrees = rotation.degrees();
            for ((axis, range), value) in [rotation.pitch, rotation.yaw, rotation.roll]
                .into_iter()
                .zip([
                    &mut stats.pitch_degrees,
                    &mut stats.yaw_degrees,
                    &mut stats.roll_degrees,
                ])
                .zip(degrees)
            {
                if axis.is_some() {
                    range.add(f64::from(value));
                }
            }
        }
        let decode = |vector: Option<&SpawnVector>| vector.map(SpawnVector::decode);
        Ok(json!({"actor_guid":spawn.actor.guid, "kind":"dynamic",
            "archetype_guid":dynamic.archetype.guid, "archetype_path":archetype_path,
            "level_guid":dynamic.level.as_ref().map(|level| level.guid), "level_path":level_path,
            "location":decode(dynamic.location.as_ref()), "location_summary":dynamic.location.map(|v| v.summary()),
            "rotation_degrees":rotation, "scale":decode(dynamic.scale.as_ref()),
            "velocity":decode(dynamic.velocity.as_ref())}))
    }

    fn finish(&mut self) -> Result<Value> {
        for state in self.channels.values() {
            self.stats.partial.in_progress_at_end += u64::from(state.partial.is_some());
            if let Some(channel) = &state.reliable {
                self.wire.reliable_held_at_end += channel.queued_len() as u64;
            }
        }
        self.wire.first_reliable_raw_distinct = self.first_raw.len() as u64;
        self.stats.exports.package_map_entries = self.package_map.len() as u64;
        let replay = &mut self.stats.rep_replay;
        replay.rep_layout_coverage_complete = self.stats.blocks.with_rep_layout > 0
            && replay.rep_layout_blocks_reencoded_exact == self.stats.blocks.with_rep_layout
            && replay.typed_blocks_failed.is_empty()
            && replay.unclassified_actor_targets.is_empty()
            && replay.unclassified_subobject_targets.is_empty();
        replay.all_blocks_coverage_complete = self.stats.blocks.total > 0
            && replay.rep_layout_blocks_reencoded_exact
                + replay.custom_delta_blocks_reencoded_exact
                + replay.subobject_lifecycle_blocks_reencoded_exact
                == self.stats.blocks.total
            && replay.typed_blocks_failed.is_empty()
            && replay.unclassified_actor_targets.is_empty()
            && replay.unclassified_subobject_targets.is_empty();
        Ok(serde_json::to_value(&self.stats)?)
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    ensure!(text.len().is_multiple_of(2), "odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|start| {
            u8::from_str_radix(text.get(start..start + 2).context("non-ASCII hex")?, 16)
                .context("invalid hex digit")
        })
        .collect()
}

/// The detail file must stay under a `work/` directory.
fn check_detail_path(path: &Path, work_root: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical = parent
        .canonicalize()
        .context("detail directory must exist")?;
    let work_root = work_root
        .canonicalize()
        .context("work directory must exist")?;
    ensure!(
        canonical.starts_with(&work_root),
        "detail output must lie under {}",
        work_root.display()
    );
    Ok(())
}

/// This checkout's ignored `work/` directory.
fn repository_work() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("work")
}

struct Reply {
    ms: Option<u64>,
    bytes: Vec<u8>,
}

fn read_replies(path: &Path) -> Result<(Vec<Reply>, Value)> {
    let file = File::open(path).context("open capture")?;
    let (mut lines, mut not_json, mut other_events, mut bad_hex) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut replies = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        lines += 1;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            not_json += 1;
            continue;
        };
        let Some(hex) = value
            .get("hex")
            .and_then(Value::as_str)
            .filter(|_| value.get("event").and_then(Value::as_str) == Some("reply"))
        else {
            other_events += 1;
            continue;
        };
        match decode_hex(hex) {
            Ok(bytes) => replies.push(Reply {
                ms: value.get("ms").and_then(Value::as_u64),
                bytes,
            }),
            Err(_) => bad_hex += 1,
        }
    }
    let first_ms = replies.iter().find_map(|reply| reply.ms);
    let last_ms = replies.iter().rev().find_map(|reply| reply.ms);
    let bytes: usize = replies.iter().map(|reply| reply.bytes.len()).sum();
    let input = json!({"lines":lines, "not_json":not_json, "other_events":other_events,
        "bad_hex":bad_hex, "replies":replies.len(), "reply_bytes":bytes,
        "first_ms":first_ms, "last_ms":last_ms});
    Ok((replies, input))
}

/// Decode every reply in `input`, write per-item detail to `detail` (which
/// must lie under this checkout's `work/`) and return the aggregate report.
pub fn run(input: &Path, detail: &Path) -> Result<Value> {
    run_in(input, detail, &repository_work())
}

/// [`run`] with the detail confined to `work_root` instead.
pub fn run_in(input: &Path, detail: &Path, work_root: &Path) -> Result<Value> {
    check_detail_path(detail, work_root)?;
    let (replies, input) = read_replies(input)?;
    let mut detail = BufWriter::new(File::create(detail).context("create detail")?);
    let mut passes = serde_json::Map::new();
    let mut wire = Value::Null;
    for mode in [Mode::Strict, Mode::ExportHypothesis] {
        let mut decoder = Decoder::new(mode, &mut detail);
        for (index, reply) in replies.iter().enumerate() {
            decoder.feed(index, reply.ms, &reply.bytes)?;
        }
        let stats = decoder.finish()?;
        if mode == Mode::Strict {
            wire = serde_json::to_value(&decoder.wire)?;
        }
        passes.insert(mode.label().into(), stats);
    }
    detail.flush()?;
    Ok(json!({"input":input, "wire":wire, "passes":passes}))
}

fn main() -> Result<()> {
    let options = Options::parse();
    let report = run(&options.input, &options.detail)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
