//! EXPERIMENTAL replicator field headers, RPC parameters and fast-array delta
//! bodies, against caller-supplied layouts.
//!
//! Static client observations (F-644) and wire-validated `FClassNetCache`
//! RPC parameter layouts (F-708); no RPC is dispatched. Writers mirror the
//! readers for round-trip research only.
//!
//! After a content block's rep-layout properties, the client replicator reads
//! field headers until its payload ends: a field index bounded like the
//! engine's `SerializeInt` by the class cache's maximum index plus one, a
//! packed payload-bit count, then exactly that payload. An unknown index is
//! skipped by its payload. Function fields carry RPC parameters; property
//! fields carry custom-delta payloads: from engine net version 11 a
//! struct-delta bit, a packed static-array index when the property is a
//! static array, then the struct's own delta serializer, left opaque here.
//!
//! RPC parameters: every parameter (each static-array element separately)
//! that is a bool is just its one bit; any other parameter has a presence bit
//! and, when set, its commands in order through the rep-layout item encodings,
//! without handles or checksums. A dynamic array is a 2-byte count, at most
//! [`RPC_ARRAY_COUNT_MAX`], then each element's commands in order. The client
//! requires a parameter payload to be consumed exactly. Which values a sender
//! marks present is not read here.
//!
//! Fast-array delta bodies (F-644): four signed 4-byte fields (the array
//! replication key, the base replication key, the delete count and the change
//! count, each count at most its cap), a 4-byte ID per deleted element, then
//! per changed element a 4-byte ID and either the item struct's commands in
//! order (whole items) or, when the sender's struct-delta bit is set and the
//! array uses delta serialization, a set bit and the item's rep-layout property
//! stream (delta items). Negative counts, which the client treats as empty,
//! fail closed here.
use anyhow::{Context, Result, ensure};

use super::actor_channel::read_packed;
use super::packet::{BitReader, BitWriter, Payload};
use super::rep_layout::{
    Budget, PropertyStream, RepCommand, RepLayoutCodec, RepValue, append_bits, copy_bits,
    enum_byte_bits, read_stream, read_u16, read_u32, read_value, validate_layout, write_stream,
    write_u16, write_u32, write_value,
};

/// Largest dynamic-array count the client accepts in RPC parameters.
pub const RPC_ARRAY_COUNT_MAX: u16 = 0xfffe;

/// One replicator field: its index and its exact payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldHeader {
    pub index: u32,
    pub payload: Payload,
}

/// Read one field header and its payload. `index_bound` is the class cache's
/// maximum index plus one; the index is below it.
pub fn read_field_header(
    reader: &mut BitReader<'_>,
    index_bound: u32,
    max_payload_bits: usize,
) -> Result<FieldHeader> {
    ensure!(index_bound >= 1, "field index bound must be at least one");
    let index = reader.read_int_max(index_bound);
    ensure!(!reader.is_error(), "truncated field index");
    let bits = read_packed(reader, "field payload bit count")? as usize;
    ensure!(
        bits <= max_payload_bits,
        "field payload exceeds the payload bit limit"
    );
    ensure!(bits <= reader.remaining(), "truncated field payload");
    Ok(FieldHeader {
        index,
        payload: copy_bits(reader, bits),
    })
}

pub fn write_field_header(
    writer: &mut BitWriter,
    header: &FieldHeader,
    index_bound: u32,
) -> Result<()> {
    ensure!(
        header.index < index_bound,
        "field index must be below its bound"
    );
    let bits = u32::try_from(header.payload.bit_len()).context("field payload too long")?;
    writer.write_int_max(header.index, index_bound)?;
    writer.write_int_packed(bits);
    append_bits(writer, &header.payload);
    Ok(())
}

/// First engine net version whose custom-delta payloads start with the
/// struct-delta bit.
pub const STRUCT_DELTA_NET_VERSION: u32 = 11;

/// The client's custom-delta payload prefix; the struct's own delta
/// serializer reads the rest and must consume it exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomDeltaHeader {
    /// The struct-delta bit, present from [`STRUCT_DELTA_NET_VERSION`].
    pub struct_delta: Option<bool>,
    /// A packed static-array index, present when the property's array
    /// dimension is not 1; below that dimension.
    pub array_index: Option<u32>,
}

pub fn read_custom_delta_header(
    reader: &mut BitReader<'_>,
    engine_net_version: u32,
    array_dim: u32,
) -> Result<CustomDeltaHeader> {
    let struct_delta = (engine_net_version >= STRUCT_DELTA_NET_VERSION).then(|| reader.read_bit());
    ensure!(!reader.is_error(), "truncated struct-delta bit");
    let array_index = if array_dim == 1 {
        None
    } else {
        let index = read_packed(reader, "custom-delta array index")?;
        ensure!(index < array_dim, "custom-delta array index out of range");
        Some(index)
    };
    Ok(CustomDeltaHeader {
        struct_delta,
        array_index,
    })
}

pub fn write_custom_delta_header(
    writer: &mut BitWriter,
    header: &CustomDeltaHeader,
    engine_net_version: u32,
    array_dim: u32,
) -> Result<()> {
    ensure!(
        header.struct_delta.is_some() == (engine_net_version >= STRUCT_DELTA_NET_VERSION),
        "struct-delta bit presence must follow the engine net version"
    );
    match header.array_index {
        None => ensure!(array_dim == 1, "a static array needs its index"),
        Some(index) => ensure!(
            array_dim != 1 && index < array_dim,
            "custom-delta array index out of range"
        ),
    }
    if let Some(bit) = header.struct_delta {
        writer.write_bit(bit);
    }
    if let Some(index) = header.array_index {
        writer.write_int_packed(index);
    }
    Ok(())
}

/// One RPC parameter as the function's layout declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcParameter {
    /// A bool parameter: one bit, no presence bit.
    Bool,
    /// Any other parameter: its commands in order.
    Value(Vec<RepCommand>),
}

/// One command's value inside a sent parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcItem {
    Value(RepValue),
    /// A dynamic array's elements, each its element commands' items.
    Array(Vec<Vec<RpcItem>>),
}

/// One received parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcArgument {
    Bool(bool),
    /// The presence bit was clear; the receiver keeps the initialized value.
    NotSent,
    Sent(Vec<RpcItem>),
}

fn validate_parameters(parameters: &[RpcParameter], codec: &RepLayoutCodec) -> Result<()> {
    for (index, parameter) in parameters.iter().enumerate() {
        if let RpcParameter::Value(commands) = parameter {
            ensure!(
                !commands.is_empty(),
                "RPC parameter {index} has no commands"
            );
            validate_layout(commands, &codec.limits)
                .with_context(|| format!("RPC parameter {index}"))?;
        }
    }
    Ok(())
}

fn array_limit(codec: &RepLayoutCodec) -> u16 {
    codec.limits.max_array_elements.min(RPC_ARRAY_COUNT_MAX)
}

fn read_items(
    reader: &mut BitReader<'_>,
    commands: &[RepCommand],
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<Vec<RpcItem>> {
    ensure!(
        depth <= codec.limits.max_depth,
        "RPC parameter nesting exceeds the depth limit"
    );
    let mut items = Vec::with_capacity(commands.len());
    for command in commands {
        budget.take()?;
        items.push(match command {
            RepCommand::DynamicArray { element } => {
                let count = read_u16(reader, codec.actor.order, "RPC array count")?;
                ensure!(
                    count <= array_limit(codec),
                    "RPC array count exceeds the element limit"
                );
                let mut elements = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    elements.push(read_items(reader, element, codec, depth + 1, budget)?);
                }
                RpcItem::Array(elements)
            }
            command => RpcItem::Value(read_value(reader, command, codec, depth, budget)?),
        });
    }
    Ok(items)
}

/// Read every parameter in order, leaving the reader after the last.
pub fn read_rpc_arguments(
    reader: &mut BitReader<'_>,
    parameters: &[RpcParameter],
    codec: &RepLayoutCodec,
) -> Result<Vec<RpcArgument>> {
    validate_parameters(parameters, codec)?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    let mut arguments = Vec::with_capacity(parameters.len());
    for (index, parameter) in parameters.iter().enumerate() {
        let bit = reader.read_bit();
        ensure!(!reader.is_error(), "truncated RPC parameter {index}");
        arguments.push(match parameter {
            RpcParameter::Bool => RpcArgument::Bool(bit),
            RpcParameter::Value(_) if !bit => RpcArgument::NotSent,
            RpcParameter::Value(commands) => RpcArgument::Sent(
                read_items(reader, commands, codec, 0, &mut budget)
                    .with_context(|| format!("RPC parameter {index}"))?,
            ),
        });
    }
    Ok(arguments)
}

/// Parse a field payload that must hold exactly the parameters.
pub fn parse_rpc_payload(
    payload: &Payload,
    parameters: &[RpcParameter],
    codec: &RepLayoutCodec,
) -> Result<Vec<RpcArgument>> {
    let mut reader = payload.reader();
    let arguments = read_rpc_arguments(&mut reader, parameters, codec)?;
    ensure!(reader.at_end(), "trailing bits after the RPC parameters");
    Ok(arguments)
}

fn write_items(
    writer: &mut BitWriter,
    commands: &[RepCommand],
    items: &[RpcItem],
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<()> {
    ensure!(
        depth <= codec.limits.max_depth,
        "RPC parameter nesting exceeds the depth limit"
    );
    ensure!(
        commands.len() == items.len(),
        "RPC items must match their commands"
    );
    for (command, item) in commands.iter().zip(items) {
        budget.take()?;
        match (command, item) {
            (RepCommand::DynamicArray { element }, RpcItem::Array(elements)) => {
                let count = u16::try_from(elements.len())
                    .ok()
                    .filter(|count| *count <= array_limit(codec))
                    .context("RPC array count exceeds the element limit")?;
                write_u16(writer, count, codec.actor.order);
                for items in elements {
                    write_items(writer, element, items, codec, depth + 1, budget)?;
                }
            }
            (RepCommand::DynamicArray { .. }, RpcItem::Value(_)) | (_, RpcItem::Array(_)) => {
                anyhow::bail!("RPC item kind does not match its command")
            }
            (command, RpcItem::Value(value)) => {
                write_value(writer, command, value, codec, depth, budget)?;
            }
        }
    }
    Ok(())
}

pub fn write_rpc_arguments(
    writer: &mut BitWriter,
    parameters: &[RpcParameter],
    arguments: &[RpcArgument],
    codec: &RepLayoutCodec,
) -> Result<()> {
    validate_parameters(parameters, codec)?;
    ensure!(
        parameters.len() == arguments.len(),
        "RPC arguments must match their parameters"
    );
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    for (index, (parameter, argument)) in parameters.iter().zip(arguments).enumerate() {
        match (parameter, argument) {
            (RpcParameter::Bool, RpcArgument::Bool(bit)) => writer.write_bit(*bit),
            (RpcParameter::Value(_), RpcArgument::NotSent) => writer.write_bit(false),
            (RpcParameter::Value(commands), RpcArgument::Sent(items)) => {
                writer.write_bit(true);
                write_items(writer, commands, items, codec, 0, &mut budget)
                    .with_context(|| format!("RPC parameter {index}"))?;
            }
            _ => anyhow::bail!("RPC parameter {index} does not match its argument"),
        }
    }
    Ok(())
}

/// Encode the parameters as one field payload.
pub fn encode_rpc_payload(
    parameters: &[RpcParameter],
    arguments: &[RpcArgument],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_rpc_arguments(&mut writer, parameters, arguments, codec)?;
    Ok(Payload::from_writer(writer))
}

/// `UActorChannel::ReadFieldHeaderAndPayload` (`0x1437c2650`) `SerializeInt`
/// bound (`FClassNetCache::GetMaxIndex() + 1` = `FieldsBase + Fields.Num() + 1`,
/// F-644, F-705, F-708) for `UIcarusStatContainer` (`2 + 1 + 1 + 1 = 5`).
pub const STAT_CONTAINER_FIELD_INDEX_BOUND: u32 = 5;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `UInventory` (`2 + 1 + 6 + 1 + 1 = 11`).
pub const INVENTORY_FIELD_INDEX_BOUND: u32 = 11;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_UIProjectionComponent_MountStatus_C` (`2 + 2 + 1 + 1 + 1 = 7`).
pub const UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND: u32 = 7;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_IcarusPlayerState_C` (`10 + 0 + 11 + 6 + 10 + 1 = 38`).
pub const BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND: u32 = 38;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_Mount_Base_C` (`10 + 3 + 17 + 25 + 6 + 6 + 10 + 6 + 1 = 84`).
pub const BP_MOUNT_BASE_FIELD_INDEX_BOUND: u32 = 84;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_Mount_Horse_C` (`83 + 1 + 1 = 85`).
pub const BP_MOUNT_HORSE_FIELD_INDEX_BOUND: u32 = 85;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_Tame_Base_C` (`83 + 2 + 1 = 86`).
pub const BP_TAME_BASE_FIELD_INDEX_BOUND: u32 = 86;
/// `SerializeInt` field-index bound (`FieldsBase + Fields.Num() + 1`, F-708)
/// for `BP_IcarusPlayerControllerSurvival_C`
/// (`10 + 4 + 74 + 13 + 29 + 25 + 27 + 1 = 183`).
pub const BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND: u32 = 183;

/// F-951 / controller net fields: reliable NetServer RPC on the pinned
/// Survival PlayerController, with one non-bool int32 parameter `ChrSlot`.
/// Flags 0x280cc0: Net | NetReliable | Native | Event | NetServer.
pub const SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX: u32 = 124;

/// Decode exactly one `ChrSlot`; an absent value retains the initialized zero
/// as the RPC parameter reader specifies. F-951 / controller net fields.
pub fn parse_begin_player_initialisation(payload: &Payload, codec: &RepLayoutCodec) -> Result<i32> {
    let arguments = parse_rpc_payload(
        payload,
        &[RpcParameter::Value(vec![RepCommand::Int])],
        codec,
    )?;
    match arguments.as_slice() {
        [RpcArgument::NotSent] => Ok(0),
        [RpcArgument::Sent(items)] => match items.as_slice() {
            [RpcItem::Value(RepValue::Int(slot))] => Ok(*slot),
            _ => anyhow::bail!("invalid character slot argument"),
        },
        _ => anyhow::bail!("invalid character slot parameters"),
    }
}

/// Maximum registered enumerator value (`EViewTargetBlendFunction::VTBlend_MAX = 5`,
/// `0x143df5290`, F-708).
pub const VIEW_TARGET_BLEND_FUNCTION_MAX_VALUE: i64 = 5;

/// Flattened child commands of `FViewTargetTransitionParams` (`0x143df52c0`,
/// F-708): `BlendTime` (`Float`), `BlendFunction` (`Byte { enum_bits: Some(3) }`),
/// `BlendExp` (`Float`), and `bLockOutgoing` (`Bool`).
pub fn view_target_transition_params_commands() -> [RepCommand; 4] {
    [
        RepCommand::Float,
        RepCommand::Byte {
            enum_bits: Some(enum_byte_bits(VIEW_TARGET_BLEND_FUNCTION_MAX_VALUE)),
        },
        RepCommand::Float,
        RepCommand::Bool,
    ]
}

/// Flattened child commands of `FUpdateLevelStreamingLevelStatus`
/// (`0x143df4350`, F-708): `PackageName` (`NetworkName`), `LODIndex` (`Int`),
/// `bNewShouldBeLoaded` (`Bool`), `bNewShouldBeVisible` (`Bool`), and
/// `bNewShouldBlockOnLoad` (`Bool`).
pub fn update_level_streaming_level_status_commands() -> [RepCommand; 5] {
    [
        RepCommand::NetworkName,
        RepCommand::Int,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
    ]
}

/// Resolve an observed `BP_IcarusPlayerState_C` net-RPC `FClassNetCache` field
/// index (`index_bound = BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND`, F-708) to
/// its RPC parameter layout:
/// - `34`: `BP_IcarusPlayerState_C::Server_UpdateHealthValue(NewHealth: float)`.
pub fn bp_icarus_player_state_rpc_parameters(field_index: u32) -> Option<Vec<RpcParameter>> {
    match field_index {
        34 => Some(vec![RpcParameter::Value(vec![RepCommand::Float])]),
        _ => None,
    }
}

/// Resolve an observed `AIcarusCharacter` / `BP_Mount_Base_C` /
/// `BP_Mount_Horse_C` / `BP_Tame_Base_C` net-RPC `FClassNetCache` field index
/// (F-708) to its RPC parameter layout:
/// - `51`: `AIcarusCharacter::Multicast_PlayReplicatedMontage(Montage: UAnimMontage*, StartingSection: FName, PlayRate: float, bSkipServer: bool)`;
/// - `53`: `AIcarusCharacter::Multicast_StopReplicatedMontage(Montage: UAnimMontage*)`.
pub fn icarus_character_rpc_parameters(field_index: u32) -> Option<Vec<RpcParameter>> {
    match field_index {
        51 => Some(vec![
            RpcParameter::Value(vec![RepCommand::Object]),
            RpcParameter::Value(vec![RepCommand::NetworkName]),
            RpcParameter::Value(vec![RepCommand::Float]),
            RpcParameter::Bool,
        ]),
        53 => Some(vec![RpcParameter::Value(vec![RepCommand::Object])]),
        _ => None,
    }
}

/// Resolve an observed `BP_IcarusPlayerControllerSurvival_C` net-RPC
/// `FClassNetCache` field index (`index_bound = BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND`,
/// F-708) to its RPC parameter layout:
/// - `13`: `AController::ClientSetRotation(NewRotation: FRotator, bResetCamera: bool)`;
/// - `18`: `APlayerController::ClientCapBandwidth(Cap: int32)`;
/// - `21`: `APlayerController::ClientEnableNetworkVoice(bEnable: bool)`;
/// - `23`: `APlayerController::ClientFlushLevelStreaming()`;
/// - `40`: `APlayerController::ClientRestart(NewPawn: APawn*)`;
/// - `46`: `APlayerController::ClientSetCameraMode(NewCamMode: FName)`;
/// - `49`: `APlayerController::ClientSetHUD(NewHUDClass: TSubclassOf<AHUD>)`;
/// - `51`: `APlayerController::ClientSetViewTarget(A: AActor*, TransitionParams: FViewTargetTransitionParams)`;
/// - `54`: `APlayerController::ClientStartOnlineSession()`;
/// - `62`: `APlayerController::ClientUpdateMultipleLevelsStreamingStatus(LevelStatuses: TArray<FUpdateLevelStreamingLevelStatus>)`;
/// - `63`: `APlayerController::ClientVoiceHandshakeComplete()`;
/// - `124`: `AIcarusPlayerController::Server_BeginPlayerInitialisation(ChrSlot: int32)`
///   (F-951 / controller net fields; client-to-server, reliable).
pub fn player_controller_rpc_parameters(field_index: u32) -> Option<Vec<RpcParameter>> {
    match field_index {
        13 => Some(vec![
            RpcParameter::Value(vec![RepCommand::Rotator]),
            RpcParameter::Bool,
        ]),
        18 | SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX => {
            Some(vec![RpcParameter::Value(vec![RepCommand::Int])])
        }
        21 => Some(vec![RpcParameter::Bool]),
        23 | 54 | 63 => Some(vec![]),
        40 | 49 => Some(vec![RpcParameter::Value(vec![RepCommand::Object])]),
        46 => Some(vec![RpcParameter::Value(vec![RepCommand::NetworkName])]),
        51 => Some(vec![
            RpcParameter::Value(vec![RepCommand::Object]),
            RpcParameter::Value(view_target_transition_params_commands().to_vec()),
        ]),
        62 => Some(vec![RpcParameter::Value(vec![RepCommand::DynamicArray {
            element: update_level_streaming_level_status_commands().to_vec(),
        }])]),
        _ => None,
    }
}

/// The client's initial fast-array delete and change caps; both are runtime
/// console values.
pub const FAST_ARRAY_COUNT_CAP: i32 = 2048;

/// How a fast array's changed items are carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FastArrayItemLayout {
    /// Each item's commands in order, as for RPC array elements.
    Whole(Vec<RepCommand>),
    /// Each item as a set bit and a rep-layout property stream over this
    /// element layout.
    Delta(Vec<RepCommand>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FastArrayItem {
    Whole(Vec<RpcItem>),
    Delta(PropertyStream),
}

/// One fast-array delta body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FastArrayDelta {
    pub array_replication_key: i32,
    pub base_replication_key: i32,
    pub deleted_ids: Vec<i32>,
    /// Changed elements by ID, in wire order.
    pub changed: Vec<(i32, FastArrayItem)>,
}

fn read_i32(reader: &mut BitReader<'_>, codec: &RepLayoutCodec, what: &str) -> Result<i32> {
    Ok(read_u32(reader, codec.actor.order, what)? as i32)
}

fn read_count(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
    cap: i32,
    what: &str,
) -> Result<usize> {
    let count = read_i32(reader, codec, what)?;
    ensure!(count >= 0, "negative fast-array {what}");
    ensure!(count <= cap, "fast-array {what} exceeds its cap");
    Ok(count as usize)
}

pub fn read_fast_array_delta(
    reader: &mut BitReader<'_>,
    layout: &FastArrayItemLayout,
    codec: &RepLayoutCodec,
    cap: i32,
) -> Result<FastArrayDelta> {
    let element = match layout {
        FastArrayItemLayout::Whole(element) | FastArrayItemLayout::Delta(element) => element,
    };
    validate_layout(element, &codec.limits)?;
    let array_replication_key = read_i32(reader, codec, "array replication key")?;
    let base_replication_key = read_i32(reader, codec, "base replication key")?;
    let deletes = read_count(reader, codec, cap, "delete count")?;
    let changes = read_count(reader, codec, cap, "change count")?;
    let deleted_ids = (0..deletes)
        .map(|_| read_i32(reader, codec, "deleted element ID"))
        .collect::<Result<Vec<_>>>()?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    // Each change carries at least its 4-byte ID, so the reserve never
    // exceeds what the remaining bits can back.
    let mut changed = Vec::with_capacity(changes.min(reader.remaining() / 32));
    for index in 0..changes {
        let id = read_i32(reader, codec, "changed element ID")?;
        let item = match layout {
            FastArrayItemLayout::Whole(element) => FastArrayItem::Whole(
                read_items(reader, element, codec, 0, &mut budget)
                    .with_context(|| format!("fast-array item {index}"))?,
            ),
            FastArrayItemLayout::Delta(element) => {
                let bit = reader.read_bit();
                ensure!(!reader.is_error(), "truncated fast-array item {index}");
                ensure!(bit, "fast-array delta item {index} without its set bit");
                // Every item draws on the body's one entry budget.
                FastArrayItem::Delta(
                    read_stream(reader, element, 1, codec, 0, &mut budget)
                        .with_context(|| format!("fast-array item {index}"))?,
                )
            }
        };
        changed.push((id, item));
    }
    Ok(FastArrayDelta {
        array_replication_key,
        base_replication_key,
        deleted_ids,
        changed,
    })
}

pub fn write_fast_array_delta(
    writer: &mut BitWriter,
    delta: &FastArrayDelta,
    layout: &FastArrayItemLayout,
    codec: &RepLayoutCodec,
    cap: i32,
) -> Result<()> {
    let element = match layout {
        FastArrayItemLayout::Whole(element) | FastArrayItemLayout::Delta(element) => element,
    };
    validate_layout(element, &codec.limits)?;
    let count = |len: usize, what: &str| {
        i32::try_from(len)
            .ok()
            .filter(|count| *count <= cap)
            .with_context(|| format!("fast-array {what} exceeds its cap"))
    };
    let deletes = count(delta.deleted_ids.len(), "delete count")?;
    let changes = count(delta.changed.len(), "change count")?;
    let order = codec.actor.order;
    for value in [
        delta.array_replication_key,
        delta.base_replication_key,
        deletes,
        changes,
    ]
    .into_iter()
    .chain(delta.deleted_ids.iter().copied())
    {
        write_u32(writer, value as u32, order);
    }
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    for (index, (id, item)) in delta.changed.iter().enumerate() {
        write_u32(writer, *id as u32, order);
        match (layout, item) {
            (FastArrayItemLayout::Whole(element), FastArrayItem::Whole(items)) => {
                write_items(writer, element, items, codec, 0, &mut budget)
                    .with_context(|| format!("fast-array item {index}"))?;
            }
            (FastArrayItemLayout::Delta(element), FastArrayItem::Delta(stream)) => {
                writer.write_bit(true);
                write_stream(writer, stream, element, 1, codec, 0, &mut budget)
                    .with_context(|| format!("fast-array item {index}"))?;
            }
            _ => anyhow::bail!("fast-array item {index} does not match its layout"),
        }
    }
    Ok(())
}
