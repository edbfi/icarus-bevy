//! Offline decoder for local capture files only: no networking, never pointed at
//! a live peer. Reads the server-to-client reply datagrams a probe recorded
//! (JSONL lines with `"event":"reply"` and `"hex"`) and runs them through this
//! repository's experimental codecs as the probe's client does: packet framing
//! (F-521..F-525, F-688), per-channel reliable ordering (F-537), partial-bunch
//! reassembly, control messages, and the actor-channel spawn record and content
//! blocks (F-569..F-576).
//!
//! Two passes over the same datagrams:
//! - `strict` uses only the documented codecs, so NetGUID export bunches and
//!   must-be-mapped GUID prefixes stay unparsed and are counted as such;
//! - `export_hypothesis` adds two readers that no static observation backs:
//!   the UE 4.27 export prefix (a rep-layout export bit, an i32 GUID count,
//!   then that many F-572 references with export flags on every nonzero GUID;
//!   wire-consistent in F-694) and the must-be-mapped prefix (a u16 count of
//!   packed GUIDs, never exercised so far). Its results are a consistency check
//!   of that hypothesis, not observations of the original's reader.
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
        SpawnRecord, SpawnVector, SubObjectClass, guid_kind, parse_actor_bunch, read_content_block,
        read_guid_ref, read_spawn,
    },
    archive::{ByteOrder, read_string_frame},
    control_messages::{ControlCodec, Decoded, read_control_messages},
    packet::{
        BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, Name, PacketConfig, Payload,
        RELIABLE_SEQUENCE_LIMIT, Receiver, read_packet,
    },
    peer_lab::PeerLabConfig,
    rep_layout::{
        MovementLevel, RepCommand, RepLayoutCodec, RepLayoutLimits, RotationLevel, TagEncoding,
        actionable_component_rep_layout, actor_component_rep_layout, actor_state_rep_layout,
        audio_context_component_rep_layout, character_state_rep_layout,
        decayable_component_rep_layout, durable_component_rep_layout, encode_rep_properties,
        fillable_component_rep_layout, flod_actor_component_rep_layout,
        focusable_component_rep_layout, icarus_player_controller_survival_rep_layout,
        itemable_component_rep_layout, modifier_state_component_rep_layout, parse_rep_properties,
        player_character_state_rep_layout, player_state_rep_layout, read_rep_properties,
        resource_network_component_rep_layout, survival_character_state_rep_layout,
        trait_behaviour_rep_layout, trait_component_rep_layout, transmutable_component_rep_layout,
        write_rep_properties,
    },
    rep_rpc::{
        FAST_ARRAY_COUNT_CAP, FastArrayItemLayout, FieldHeader, read_custom_delta_header,
        read_fast_array_delta, read_field_header, write_custom_delta_header,
        write_fast_array_delta, write_field_header,
    },
    sequencing::{
        ReliableChannel, ReliableReceive, is_newer, reconstruct_reliable_sequence,
        sequence_difference,
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
/// `UClass::ClassNetCache` field-index bound (`FClassNetCache::GetMaxIndex() + 1`
/// = `FieldsBase + Fields.Num()`, `0x1437c27aa`) for `UIcarusStatContainer`:
/// 2 inherited `UActorComponent` net properties (`bReplicates`, `bIsActive`) +
/// 1 own net property (`ReplicatedStatArray`) + 1 own net function
/// (`OnServer_SerialiseStats`) = 4 (F-705).
const STAT_CONTAINER_FIELD_INDEX_BOUND: u32 = 4;

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

/// HYPOTHESIS (UE 4.27 `ReceiveNetGUIDBunch`, not a finding) read with the
/// codec's own F-572 reader, export flags on every nonzero GUID. Used only to
/// report where the codec stops on real export bunches.
fn read_export_prefix_codec(reader: &mut BitReader<'_>) -> Result<Vec<NetGuidRef>> {
    let count = read_export_count(reader)?;
    let codec = ActorChannelCodec {
        exports_enabled: true,
        ..actor_codec(true)
    };
    (0..count)
        .map(|index| read_guid_ref(reader, &codec).with_context(|| format!("export GUID {index}")))
        .collect()
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

/// HYPOTHESIS (UE 4.27 `UActorChannel::ReceivedBunch`, not a finding): a u16
/// count, then that many packed GUIDs, before the actor bunch body.
fn strip_must_be_mapped(payload: &Payload) -> Result<(Vec<u32>, Payload)> {
    let mut reader = payload.reader();
    let count = reader.read_bits(16);
    ensure!(!reader.is_error(), "truncated must-be-mapped count");
    let mut guids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let guid = reader.read_int_packed();
        ensure!(!reader.is_error(), "truncated must-be-mapped GUID");
        guids.push(guid);
    }
    Ok((guids, rest(&mut reader)))
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
            max_opaque_bits: 1 << 20,
            ..RepLayoutLimits::default()
        },
    }
}

#[derive(Clone, Copy)]
enum TargetSchema {
    RepLayout(&'static str),
    StatContainer,
}

fn classify_actor_schema(archetype_path: &str) -> Option<TargetSchema> {
    match archetype_path {
        "Default__IcarusPlayerState_C" => Some(TargetSchema::RepLayout("APlayerState")),
        "Default__BP_IcarusPlayerControllerSurvival_C" => {
            Some(TargetSchema::RepLayout("AIcarusPlayerControllerSurvival"))
        }
        _ => None,
    }
}

fn classify_subobject_schema(
    target_name: &str,
    channel_archetype: Option<&str>,
) -> Option<TargetSchema> {
    match target_name {
        "Stat Container" | "IcarusStatContainer" => Some(TargetSchema::StatContainer),
        "ModifierStateComponent" => Some(TargetSchema::RepLayout("UModifierStateComponent")),
        "TraitDurable" | "DurableComponent" => Some(TargetSchema::RepLayout("UDurableComponent")),
        "TraitItemable" | "ItemableComponent" => {
            Some(TargetSchema::RepLayout("UItemableComponent"))
        }
        "TraitDecayable" | "DecayableComponent" => {
            Some(TargetSchema::RepLayout("UDecayableComponent"))
        }
        "TraitFillable" | "FillableComponent" => {
            Some(TargetSchema::RepLayout("UFillableComponent"))
        }
        "TraitTransmutable" | "TransmutableComponent" => {
            Some(TargetSchema::RepLayout("UTransmutableComponent"))
        }
        "TraitFocusable" | "FocusableComponent" => {
            Some(TargetSchema::RepLayout("UFocusableComponent"))
        }
        "TraitActionable" | "ActionableComponent" => {
            Some(TargetSchema::RepLayout("UActionableComponent"))
        }
        "ResourceNetwork" | "WaterNetwork" | "EnergyNetwork" | "TraitFuel" => {
            Some(TargetSchema::RepLayout("UResourceNetworkComponent"))
        }
        "FLODActor" | "FLODActorComponent" => Some(TargetSchema::RepLayout("UFLODActorComponent")),
        "AudioContextComponent" => Some(TargetSchema::RepLayout("UAudioContextComponent")),
        "ActorState" => match channel_archetype {
            Some(arch)
                if arch.contains("Tamed_")
                    || arch.contains("Tame_")
                    || arch.contains("Mount_")
                    || arch.contains("PlayerCharacter")
                    || arch.contains("Character") =>
            {
                None
            }
            _ => Some(TargetSchema::RepLayout("UActorState")),
        },
        "CharacterState" => Some(TargetSchema::RepLayout("UCharacterState")),
        "SurvivalCharacterState" => Some(TargetSchema::RepLayout("USurvivalCharacterState")),
        "PlayerCharacterState" => Some(TargetSchema::RepLayout("UPlayerCharacterState")),
        "PlayerData" | "PlayerDataComponent" => Some(TargetSchema::RepLayout("UActorComponent")),
        "InventoryComponent"
        | "Meshable"
        | "MeshableComponent"
        | "Interactable"
        | "InteractableComponent"
        | "Highlightable"
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
        | "TraitSeat" => Some(TargetSchema::RepLayout("UTraitComponent")),
        "TraitBehaviour" => Some(TargetSchema::RepLayout("UTraitBehaviour")),
        _ => None,
    }
}

fn schema_commands(schema_key: &str) -> Vec<RepCommand> {
    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };
    match schema_key {
        "APlayerState" => player_state_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
        ),
        "AIcarusPlayerControllerSurvival" => icarus_player_controller_survival_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
            tag_encoding,
        ),
        "UActorComponent" => actor_component_rep_layout(),
        "UFLODActorComponent" => flod_actor_component_rep_layout(),
        "UAudioContextComponent" => audio_context_component_rep_layout(),
        "UTraitBehaviour" => trait_behaviour_rep_layout(),
        "UResourceNetworkComponent" => resource_network_component_rep_layout(),
        "UTraitComponent" => trait_component_rep_layout(),
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
        _ => unreachable!(),
    }
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
    /// The codec's F-572 reader on the same prefix, for comparison.
    codec_reader_parsed: u64,
    codec_reader_failed: Tally,
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
    failed: Tally,
    guids: u64,
    guids_exported_earlier: u64,
}

#[derive(Default, Serialize)]
struct PartialStats {
    initial: u64,
    continuation: u64,
    completed: u64,
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
            None => {}
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
                    match read_export_prefix_codec(&mut arrival.bunch.payload.reader()) {
                        Ok(_) => self.stats.exports.codec_reader_parsed += 1,
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
            self.wire.channels_closed += 1;
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

    /// The engine's partial merge (UE 4.27 `ReceivedNextBunch`, inferred):
    /// one assembly per channel; reliable parts must be consecutive, unreliable
    /// parts share or follow the packet sequence; an export-only part adds no
    /// data. Returns the complete bunch on the final part.
    fn merge_partial(
        &mut self,
        arrival: &Arrival,
        data: Option<Payload>,
        opaque: Option<&'static str>,
    ) -> Option<Complete> {
        let header = &arrival.bunch.header;
        let partial_stats = &mut self.stats.partial;
        let state = self.channels.entry(header.channel_index).or_default();
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
            let (guids, remainder) = match strip_must_be_mapped(&bunch.payload) {
                Ok(stripped) => stripped,
                Err(error) => {
                    bump(&mut self.stats.must_be_mapped.failed, class(&error));
                    return Err(error.context("must-be-mapped prefix"));
                }
            };
            let must_be_mapped = &mut self.stats.must_be_mapped;
            must_be_mapped.parsed += 1;
            must_be_mapped.guids += guids.len() as u64;
            must_be_mapped.guids_exported_earlier += guids
                .iter()
                .filter(|guid| self.exported.contains_key(guid))
                .count() as u64;
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
            // Check the 8-bit `NetPlayerIndex` hook on PlayerController open bunches (F-696).
            if arch.contains("PlayerController") {
                let mut hook_reader = bunch.payload.reader();
                if read_spawn(&mut hook_reader, codec).is_ok() && hook_reader.remaining() >= 8 {
                    let _net_player_index = hook_reader.read_bits(8);
                    let mut hook_blocks = Vec::new();
                    let mut ok = true;
                    while !hook_reader.at_end() {
                        match read_content_block(&mut hook_reader, codec) {
                            Ok(block) => hook_blocks.push(block),
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        for block in &hook_blocks {
                            let (object_path, class_path) = self.resolve_block_paths(block);
                            if let Some((_schema_name, exact)) = self.replay_block(
                                bunch.header.channel_index,
                                block,
                                object_path.as_deref(),
                                class_path.as_deref(),
                            ) {
                                self.stats
                                    .rep_replay
                                    .player_controller_open_hook_blocks_decoded += 1;
                                if exact {
                                    self.stats
                                        .rep_replay
                                        .player_controller_open_hook_blocks_reencoded_exact += 1;
                                }
                            }
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
    ) -> Option<(&'static str, bool)> {
        let channel_arch = self
            .channels
            .get(&channel_index)
            .and_then(|state| state.archetype_path.as_deref());
        let target_schema = match &block.target {
            BlockTarget::Actor => {
                if !block.has_rep_layout {
                    return None;
                }
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
            BlockTarget::SubObject { .. } => {
                let Some(target_name) = class_path.or(object_path) else {
                    if block.has_rep_layout {
                        bump(
                            &mut self.stats.rep_replay.unclassified_subobject_targets,
                            "<unresolved_subobject>",
                        );
                    }
                    return None;
                };
                let Some(schema) = classify_subobject_schema(target_name, channel_arch) else {
                    if block.has_rep_layout {
                        bump(
                            &mut self.stats.rep_replay.unclassified_subobject_targets,
                            target_name,
                        );
                    }
                    return None;
                };
                if matches!(schema, TargetSchema::RepLayout(_)) && !block.has_rep_layout {
                    return None;
                }
                schema
            }
        };

        let codec = rep_codec();
        let replay = &mut self.stats.rep_replay;
        replay.typed_blocks_attempted += 1;
        match target_schema {
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
            TargetSchema::RepLayout(schema_name) => {
                let commands = schema_commands(schema_name);
                match parse_rep_properties(&block.payload, &commands, &codec) {
                    Ok(stream) => {
                        replay.typed_blocks_decoded += 1;
                        bump(&mut replay.by_schema_decoded, schema_name);
                        replay.rep_entries_decoded += stream.total_entries() as u64;
                        let mut effective = codec;
                        effective.checksums = stream.terminator_checksum.is_some();
                        let exact = encode_rep_properties(&stream, &commands, &effective)
                            .is_ok_and(|reencoded| reencoded == block.payload);
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
            );
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
