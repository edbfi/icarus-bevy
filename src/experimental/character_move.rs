//! EXPERIMENTAL: the client's packed character move and the server's
//! good-move acknowledgement on the joining player's pawn (F-1127).
//!
//! After entering the world a client sends pawn field 39,
//! `ACharacter::ServerMovePacked`, unreliable, alone in its bunch (F-1106).
//! Its parameter is one `FCharacterServerMovePackedBits`: a presence bit, a
//! packed bit count, then exactly that many container bits:
//!
//! - the new move: a 32-bit float `TimeStamp`, `Acceleration` as a packed
//!   quantized vector (`NetQuantize10`), `Location` (`NetQuantize100`, world
//!   coordinates in centimetres when there is no movement base), the control
//!   rotation as a compressed rotator, and an optional byte of move flags
//!   (absent means 0); then, for the new move only, an optional movement base
//!   (a packed NetGUID), an optional base bone name (a network name) and an
//!   optional movement mode byte (absent means 1, walking);
//! - a pending-move bit, then a hybrid root-motion bit and the pending move;
//! - an old-move bit, then the old move (pending and old moves carry only
//!   the fields before the base);
//! - one final bit (combined-move scoping disabled).
//!
//! [`read_server_move_packed`] is strict: the container must fill the field
//! payload exactly and consume exactly its declared count, at most
//! [`MAX_SERVER_MOVE_DATA_BITS`]. A movement base is kept as its raw NetGUID
//! and never resolved, so an unknown base is not an error.
//!
//! The server answers with pawn field 30, `ACharacter::ClientMoveResponsePacked`,
//! unreliable: for a good move its parameter is a presence bit, the packed
//! count 33, the `bAckGoodMove` bit (set) and the acknowledged `TimeStamp`, 42
//! bits in all ([`write_good_move_ack`]).
//!
//! A correction has `bAckGoodMove` clear; [`MoveCorrection`] documents its
//! layout, read statically from public dedicated server 26613688
//! (`FCharacterMoveResponseDataContainer::Serialize`, `0x1434b0c30`; the
//! serializer of 26425113 differs only in relocated displacements). It is
//! written by [`write_move_correction`], read strictly by
//! [`read_move_response`] and framed as a field-30 actor block by
//! [`compose_move_correction_block`]. No session sends one yet.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::actor_channel::{
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, CompressedRotator,
    ContentBlock, encode_actor_bunch_body, read_packed, read_rotator, write_rotator,
};
use super::archive::ByteOrder;
use super::packet::{BitReader, BitWriter, Payload};
use super::rep_layout::{
    NetworkName, QuantizedVector, RepLayoutCodec, RepLayoutLimits, VectorPacking, append_bits,
    read_network_name, read_u32, read_vector, write_network_name, write_u32, write_vector,
};
use super::rep_rpc::{
    BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND, CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX,
    FieldHeader, read_field_header, write_field_header,
};

/// Authored cap on a packed move's declared container bit count, far above
/// the largest observed (F-1127).
pub const MAX_SERVER_MOVE_DATA_BITS: u32 = 4096;

/// `MOVE_None`: the client has no movement mode.
pub const MOVEMENT_MODE_NONE: u8 = 0;
/// `MOVE_Walking`, the mode an absent mode byte means.
pub const MOVEMENT_MODE_WALKING: u8 = 1;

/// The container bits of a good-move acknowledgement: `bAckGoodMove` and
/// the 32-bit timestamp.
pub const GOOD_MOVE_ACK_DATA_BITS: u32 = 33;
/// A good-move acknowledgement's whole field payload.
pub const GOOD_MOVE_ACK_PAYLOAD_BITS: usize = 42;

/// The fields every move carries.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveFields {
    pub timestamp: f32,
    /// `NetQuantize10`; components are tenths after unbiasing.
    pub acceleration: QuantizedVector,
    /// `NetQuantize100`; components are hundredths of a centimetre after
    /// unbiasing.
    pub location: QuantizedVector,
    pub rotation: CompressedRotator,
    /// The move-flags byte when its presence bit is set.
    pub flags: Option<u8>,
}

impl MoveFields {
    /// The move flags; absent means none.
    pub fn compressed_flags(&self) -> u8 {
        self.flags.unwrap_or(0)
    }

    /// `Location` in centimetres (world coordinates without a base).
    pub fn location_cm(&self) -> [f64; 3] {
        self.location.unbiased().map(|value| value as f64 / 100.0)
    }
}

/// The new move: [`MoveFields`] and the movement base, bone and mode.
#[derive(Clone, Debug, PartialEq)]
pub struct NewMove {
    pub fields: MoveFields,
    /// The movement base's NetGUID as sent, never resolved.
    pub base: Option<u32>,
    pub bone: Option<NetworkName>,
    /// The movement-mode byte when its presence bit is set.
    pub mode: Option<u8>,
}

impl NewMove {
    /// The movement mode; absent means walking.
    pub fn movement_mode(&self) -> u8 {
        self.mode.unwrap_or(MOVEMENT_MODE_WALKING)
    }
}

/// One decoded `ServerMovePacked` parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct PackedMove {
    pub new: NewMove,
    /// The hybrid root-motion bit and the pending move, when present.
    pub pending: Option<(bool, MoveFields)>,
    pub old: Option<MoveFields>,
    pub disable_combined: bool,
}

fn bit(reader: &mut BitReader<'_>, what: &str) -> Result<bool> {
    let value = reader.read_bit();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

fn byte(reader: &mut BitReader<'_>, what: &str) -> Result<u8> {
    let value = reader.read_bits(8);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value as u8)
}

fn optional_byte(reader: &mut BitReader<'_>, what: &str) -> Result<Option<u8>> {
    if bit(reader, what)? {
        Ok(Some(byte(reader, what)?))
    } else {
        Ok(None)
    }
}

fn read_fields(reader: &mut BitReader<'_>, order: ByteOrder) -> Result<MoveFields> {
    let timestamp = f32::from_bits(read_u32(reader, order, "move timestamp")?);
    let acceleration =
        read_vector(reader, VectorPacking::NetQuantize10).context("move acceleration")?;
    let location = read_vector(reader, VectorPacking::NetQuantize100).context("move location")?;
    let rotation = read_rotator(reader).context("move rotation")?;
    let flags = optional_byte(reader, "move flags")?;
    Ok(MoveFields {
        timestamp,
        acceleration,
        location,
        rotation,
        flags,
    })
}

fn write_fields(writer: &mut BitWriter, fields: &MoveFields, order: ByteOrder) -> Result<()> {
    write_u32(writer, fields.timestamp.to_bits(), order);
    write_vector(writer, &fields.acceleration, VectorPacking::NetQuantize10)?;
    write_vector(writer, &fields.location, VectorPacking::NetQuantize100)?;
    write_rotator(writer, &fields.rotation);
    writer.write_bit(fields.flags.is_some());
    if let Some(flags) = fields.flags {
        writer.write_bits(u32::from(flags), 8);
    }
    Ok(())
}

/// Read a pawn field-39 payload. `codec` supplies the connection's byte
/// order, engine net version (for the bone name) and name limits.
pub fn read_server_move_packed(payload: &Payload, codec: &RepLayoutCodec) -> Result<PackedMove> {
    let order = codec.actor.order;
    let mut reader = payload.reader();
    ensure!(
        bit(&mut reader, "move presence bit")?,
        "packed move parameter absent"
    );
    let declared = read_packed(&mut reader, "packed move bit count")?;
    ensure!(
        declared <= MAX_SERVER_MOVE_DATA_BITS,
        "packed move exceeds its bit cap"
    );
    let declared = declared as usize;
    ensure!(
        declared == reader.remaining(),
        "packed move bit count differs from its field payload"
    );
    let start = reader.position();
    let fields = read_fields(&mut reader, order).context("new move")?;
    let base = if bit(&mut reader, "movement base bit")? {
        Some(read_packed(&mut reader, "movement base NetGUID")?)
    } else {
        None
    };
    let bone = if bit(&mut reader, "movement base bone bit")? {
        Some(read_network_name(&mut reader, codec).context("movement base bone")?)
    } else {
        None
    };
    let mode = optional_byte(&mut reader, "movement mode")?;
    let pending = if bit(&mut reader, "pending move bit")? {
        let hybrid = bit(&mut reader, "hybrid root motion bit")?;
        Some((
            hybrid,
            read_fields(&mut reader, order).context("pending move")?,
        ))
    } else {
        None
    };
    let old = if bit(&mut reader, "old move bit")? {
        Some(read_fields(&mut reader, order).context("old move")?)
    } else {
        None
    };
    let disable_combined = bit(&mut reader, "combined move bit")?;
    ensure!(
        reader.position() - start == declared && reader.at_end(),
        "packed move leaves trailing bits"
    );
    Ok(PackedMove {
        new: NewMove {
            fields,
            base,
            bone,
            mode,
        },
        pending,
        old,
        disable_combined,
    })
}

/// Write a field-39 payload; [`read_server_move_packed`]'s inverse.
pub fn write_server_move_packed(packed: &PackedMove, codec: &RepLayoutCodec) -> Result<Payload> {
    let order = codec.actor.order;
    let mut data = BitWriter::new();
    write_fields(&mut data, &packed.new.fields, order)?;
    data.write_bit(packed.new.base.is_some());
    if let Some(base) = packed.new.base {
        data.write_int_packed(base);
    }
    data.write_bit(packed.new.bone.is_some());
    if let Some(bone) = &packed.new.bone {
        write_network_name(&mut data, bone, codec)?;
    }
    data.write_bit(packed.new.mode.is_some());
    if let Some(mode) = packed.new.mode {
        data.write_bits(u32::from(mode), 8);
    }
    data.write_bit(packed.pending.is_some());
    if let Some((hybrid, pending)) = &packed.pending {
        data.write_bit(*hybrid);
        write_fields(&mut data, pending, order)?;
    }
    data.write_bit(packed.old.is_some());
    if let Some(old) = &packed.old {
        write_fields(&mut data, old, order)?;
    }
    data.write_bit(packed.disable_combined);
    let declared = u32::try_from(data.bit_len()).context("packed move too long")?;
    ensure!(
        declared <= MAX_SERVER_MOVE_DATA_BITS,
        "packed move exceeds its bit cap"
    );
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_int_packed(declared);
    append_bits(&mut writer, &Payload::from_writer(data));
    Ok(Payload::from_writer(writer))
}

/// A field-30 good-move acknowledgement of `timestamp`: presence, packed
/// count 33, `bAckGoodMove` set, then the timestamp's 32 bits.
pub fn write_good_move_ack(timestamp: f32, order: ByteOrder) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_int_packed(GOOD_MOVE_ACK_DATA_BITS);
    writer.write_bit(true);
    write_u32(&mut writer, timestamp.to_bits(), order);
    Payload::from_writer(writer)
}

/// Read a field-30 payload that must be a good-move acknowledgement and
/// return its timestamp; corrections are refused (never sent here).
pub fn read_good_move_ack(payload: &Payload, order: ByteOrder) -> Result<f32> {
    let mut reader = payload.reader();
    ensure!(
        bit(&mut reader, "move response presence bit")?,
        "move response parameter absent"
    );
    let declared = read_packed(&mut reader, "move response bit count")?;
    ensure!(
        declared == GOOD_MOVE_ACK_DATA_BITS && reader.remaining() == declared as usize,
        "not a good-move acknowledgement"
    );
    ensure!(
        bit(&mut reader, "good move bit")?,
        "a correction, not an acknowledgement"
    );
    let timestamp = f32::from_bits(read_u32(&mut reader, order, "acknowledged timestamp")?);
    ensure!(reader.at_end(), "move response leaves trailing bits");
    Ok(timestamp)
}

/// The original's limit on a move response's declared container bits:
/// `FCharacterNetworkSerializationPackedBits::NetSerialize` (`0x1434a64a0`)
/// fails a count above the integer at `0x145f73984`, 2048 in the shipped data
/// of 26613688 and 26425113. It backs a console variable, so a configured
/// original may differ; this codec keeps the shipped value.
pub const MAX_MOVE_RESPONSE_DATA_BITS: u32 = 2048;

/// `MOVE_Falling`, the mode byte of the observed corrections (F-1132).
pub const MOVEMENT_MODE_FALLING: u8 = 3;

/// `UCharacterMovementComponent::PackNetworkMovementMode` (`0x1434a7510`)
/// sends a non-custom mode as `mode | ground << 3`, with `ground` set when the
/// ground movement mode is not walking, and custom mode `n` as `16 + n`
/// (8-bit sum). The shift 3, threshold 16 and mask 7 are set by the static
/// initializers at `0x1407fdf00`, `0x1407fde30` and `0x1407fdee0`.
pub const PACKED_MOVEMENT_MODE_GROUND_BIT: u8 = 1 << 3;
/// The first custom-mode byte; see [`PACKED_MOVEMENT_MODE_GROUND_BIT`].
pub const PACKED_MOVEMENT_MODE_CUSTOM_BASE: u8 = 16;

/// `UPackageMap::StaticSerializeName` (`0x14212df00`) sends a name as a
/// hardcoded index only when that index is at most 410; longer indices and
/// other names go as strings.
pub const MAX_SENT_HARDCODED_NAME_INDEX: u32 = 410;

/// The default NetGUID: `UPackageMapClient::InternalWriteObject`
/// (`0x143731120`) follows it with an export-flags byte and a path, which
/// this codec does not model.
const DEFAULT_NET_GUID: u32 = 1;

/// Whether `byte` is a packed movement mode the original's packer produces
/// without custom-mode wraparound: modes 0 to 5 with or without the ground
/// bit, or any custom byte. 6, 7, 14 and 15 come only from custom modes 246
/// and above wrapping, and 7 and 15 unpack to `MOVE_MAX`
/// (`UnpackNetworkMovementMode`, `0x1434bc0d0`).
pub fn is_packed_movement_mode(byte: u8) -> bool {
    byte >= PACKED_MOVEMENT_MODE_CUSTOM_BASE || byte & 7 <= 5
}

/// The parts of a root-motion montage correction (its flag set).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootMotionMontage {
    /// `RootMotionTrackPosition`, a 32-bit float.
    pub track_position: f32,
    /// `RootMotionRotation` pitch, yaw and roll as sent: 16 bits each,
    /// `WriteFixedCompressedFloat<1, 16>` (`0x14344ed20`) of the rotation
    /// scaled to -1..1, `round(32767 * value) + 32768`.
    pub rotation: [u16; 3],
}

/// One `ClientMoveResponsePacked` correction (`bAckGoodMove` clear). Its
/// container bits, in order, as `FCharacterMoveResponseDataContainer::Serialize`
/// (`0x1434b0c30`) writes them into the response bit writer:
///
/// 1. `bAckGoodMove`, 1 bit, clear;
/// 2. `TimeStamp`, 32-bit float (archive byte order);
/// 3. four flag bits: `bHasBase` (set exactly when a base is present; must
///    equal item 7's bit), `bHasRotation`, `bRootMotionMontageCorrection` and
///    `bRootMotionSourceCorrection`;
/// 4. `NewLoc`, three 32-bit floats (`FVector::NetSerialize`, `0x141f0a110`,
///    no quantization and no finiteness check);
/// 5. `NewVel`, likewise;
/// 6. when `bHasRotation`: `NewRot` as a compressed rotator
///    (`FRotator::SerializeCompressedShort`, `0x141f0e360`): per pitch, yaw,
///    roll a bit, then a 16-bit value in archive byte order when nonzero;
/// 7. a bit, then the movement base as a packed NetGUID (nothing else
///    outside a NetGUID export: `InternalWriteObject`, `0x143731120`);
/// 8. a bit, then the base bone as a network name;
/// 9. a bit, then the packed movement mode byte (absent means 1, walking);
/// 10. `bBaseRelativePosition`, 1 bit;
/// 11. when `bRootMotionMontageCorrection`: `RootMotionTrackPosition`, a
///     32-bit float;
/// 12. when `bRootMotionSourceCorrection`: the root-motion source group
///     (`FRootMotionSourceGroup::NetSerialize`, `0x1435cf670`, not read);
/// 13. when either root-motion flag is set: `RootMotionRotation`, three 16-bit
///     `SerializeInt(65536)` values.
///
/// The server's `ServerFillResponseData` (`0x1434b1590`) never sets
/// `bHasRotation`, so its corrections carry no rotation; the codec supports
/// it anyway. Source corrections (12) are refused: their width depends on the
/// server's active root-motion sources. The reader also refuses what the
/// original writer cannot produce: a mode byte of 1 or outside
/// [`is_packed_movement_mode`], a rotator axis sent as zero, a bone name that
/// is `None` or a hardcoded index above [`MAX_SENT_HARDCODED_NAME_INDEX`],
/// and the default NetGUID as a base. Every float must be finite, an authored
/// choice: the original copies any bit pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveCorrection {
    /// The pending adjustment's `TimeStamp` (the corrected client move's,
    /// not read here).
    pub timestamp: f32,
    /// `NewLoc`.
    pub location: [f32; 3],
    /// `NewVel`.
    pub velocity: [f32; 3],
    pub rotation: Option<CompressedRotator>,
    /// The movement base's NetGUID as sent, never resolved.
    pub base: Option<u32>,
    pub bone: Option<NetworkName>,
    /// The packed movement mode byte when its presence bit is set.
    pub mode: Option<u8>,
    /// `bBaseRelativePosition`, copied from the pending adjustment; its
    /// producer was not read here.
    pub base_relative: bool,
    pub root_motion: Option<RootMotionMontage>,
}

impl MoveCorrection {
    /// The packed movement mode; absent means walking.
    pub fn movement_mode(&self) -> u8 {
        self.mode.unwrap_or(MOVEMENT_MODE_WALKING)
    }

    /// Which optional parts are present, without their values.
    pub fn presence(&self) -> CorrectionPresence {
        CorrectionPresence {
            rotation: self.rotation.is_some(),
            base: self.base.is_some(),
            bone: self.bone.is_some(),
            mode: self.mode.is_some(),
            root_motion_montage: self.root_motion.is_some(),
        }
    }
}

/// The optional parts a correction carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CorrectionPresence {
    pub rotation: bool,
    pub base: bool,
    pub bone: bool,
    pub mode: bool,
    pub root_motion_montage: bool,
}

impl CorrectionPresence {
    /// The names of the present parts, in wire order.
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.rotation, "rotation"),
            (self.base, "base"),
            (self.bone, "bone"),
            (self.mode, "mode"),
            (self.root_motion_montage, "root_motion_montage"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect()
    }
}

/// One decoded field-30 parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum MoveResponse {
    /// A good-move acknowledgement of this timestamp.
    Ack(f32),
    Correction(MoveCorrection),
}

/// The codec settings a move response needs: byte order, and the engine net
/// version and name limits for the base bone.
pub fn move_response_codec(engine_net_version: u32, order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version,
            exports_enabled: false,
            has_level: true,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

fn finite(value: f32, what: &str) -> Result<f32> {
    ensure!(value.is_finite(), "{what} is not finite");
    Ok(value)
}

fn read_float(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<f32> {
    finite(f32::from_bits(read_u32(reader, order, what)?), what)
}

fn read_float_vector(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<[f32; 3]> {
    Ok([
        read_float(reader, order, what)?,
        read_float(reader, order, what)?,
        read_float(reader, order, what)?,
    ])
}

/// The rotator's 16-bit values go through `Ar << uint16`, swapped with the
/// archive byte order; the shared primitive reads them little-endian.
fn swap_rotator(rotator: &CompressedRotator, order: ByteOrder) -> CompressedRotator {
    let swap = |axis: Option<u16>| match order {
        ByteOrder::Little => axis,
        ByteOrder::Big => axis.map(u16::swap_bytes),
    };
    CompressedRotator {
        pitch: swap(rotator.pitch),
        yaw: swap(rotator.yaw),
        roll: swap(rotator.roll),
    }
}

/// The refusals both directions share; see [`MoveCorrection`].
fn check_correction(correction: &MoveCorrection) -> Result<()> {
    finite(correction.timestamp, "correction timestamp")?;
    for value in correction.location {
        finite(value, "correction location")?;
    }
    for value in correction.velocity {
        finite(value, "correction velocity")?;
    }
    if let Some(rotation) = &correction.rotation {
        ensure!(
            [rotation.pitch, rotation.yaw, rotation.roll]
                .iter()
                .all(|axis| *axis != Some(0)),
            "correction rotation sends a zero axis"
        );
    }
    ensure!(
        correction.base != Some(DEFAULT_NET_GUID),
        "default movement base NetGUID unsupported"
    );
    match &correction.bone {
        Some(NetworkName::Hardcoded(index)) => ensure!(
            (1..=MAX_SENT_HARDCODED_NAME_INDEX).contains(index),
            "movement base bone index is None or above 410"
        ),
        Some(NetworkName::String { .. }) | None => {}
    }
    if let Some(mode) = correction.mode {
        ensure!(
            mode != MOVEMENT_MODE_WALKING,
            "movement mode byte sent as its default"
        );
        ensure!(
            is_packed_movement_mode(mode),
            "movement mode byte out of range"
        );
    }
    if let Some(root_motion) = &correction.root_motion {
        finite(root_motion.track_position, "root-motion track position")?;
    }
    Ok(())
}

/// Write a correction's container bits without [`check_correction`]; the
/// flag arguments let tests build what the original cannot produce.
fn encode_correction(
    correction: &MoveCorrection,
    has_base_flag: bool,
    source_flag: bool,
    codec: &RepLayoutCodec,
) -> Result<BitWriter> {
    let order = codec.actor.order;
    let mut data = BitWriter::new();
    data.write_bit(false);
    write_u32(&mut data, correction.timestamp.to_bits(), order);
    data.write_bit(has_base_flag);
    data.write_bit(correction.rotation.is_some());
    data.write_bit(correction.root_motion.is_some());
    data.write_bit(source_flag);
    for value in correction.location.iter().chain(&correction.velocity) {
        write_u32(&mut data, value.to_bits(), order);
    }
    if let Some(rotation) = &correction.rotation {
        write_rotator(&mut data, &swap_rotator(rotation, order));
    }
    data.write_bit(correction.base.is_some());
    if let Some(base) = correction.base {
        data.write_int_packed(base);
    }
    data.write_bit(correction.bone.is_some());
    if let Some(bone) = &correction.bone {
        write_network_name(&mut data, bone, codec).context("movement base bone")?;
    }
    data.write_bit(correction.mode.is_some());
    if let Some(mode) = correction.mode {
        data.write_bits(u32::from(mode), 8);
    }
    data.write_bit(correction.base_relative);
    if let Some(root_motion) = &correction.root_motion {
        write_u32(&mut data, root_motion.track_position.to_bits(), order);
        for axis in root_motion.rotation {
            data.write_bits(u32::from(axis), 16);
        }
    }
    Ok(data)
}

/// The field-30 parameter around container bits: presence, packed count.
fn frame_move_response(data: BitWriter) -> Result<Payload> {
    let declared = u32::try_from(data.bit_len()).context("move response too long")?;
    ensure!(
        declared <= MAX_MOVE_RESPONSE_DATA_BITS,
        "move response exceeds its bit cap"
    );
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_int_packed(declared);
    append_bits(&mut writer, &Payload::from_writer(data));
    Ok(Payload::from_writer(writer))
}

/// Write a field-30 correction payload: presence bit, packed count, then the
/// container bits [`MoveCorrection`] lists. Refuses what
/// [`read_move_response`] refuses.
pub fn write_move_correction(
    correction: &MoveCorrection,
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    check_correction(correction)?;
    frame_move_response(encode_correction(
        correction,
        correction.base.is_some(),
        false,
        codec,
    )?)
}

/// Write either response; an acknowledgement is [`write_good_move_ack`]'s.
pub fn write_move_response(response: &MoveResponse, codec: &RepLayoutCodec) -> Result<Payload> {
    match response {
        MoveResponse::Ack(timestamp) => Ok(write_good_move_ack(*timestamp, codec.actor.order)),
        MoveResponse::Correction(correction) => write_move_correction(correction, codec),
    }
}

fn read_correction(
    reader: &mut BitReader<'_>,
    timestamp: f32,
    codec: &RepLayoutCodec,
) -> Result<MoveCorrection> {
    let order = codec.actor.order;
    let has_base = bit(reader, "has-base flag")?;
    let has_rotation = bit(reader, "has-rotation flag")?;
    let montage = bit(reader, "root-motion montage flag")?;
    ensure!(
        !bit(reader, "root-motion source flag")?,
        "root-motion source corrections are not supported"
    );
    let location = read_float_vector(reader, order, "correction location")?;
    let velocity = read_float_vector(reader, order, "correction velocity")?;
    let rotation = if has_rotation {
        let rotator = read_rotator(reader).context("correction rotation")?;
        Some(swap_rotator(&rotator, order))
    } else {
        None
    };
    let base = if bit(reader, "movement base bit")? {
        Some(read_packed(reader, "movement base NetGUID")?)
    } else {
        None
    };
    ensure!(
        base.is_some() == has_base,
        "has-base flag differs from the movement base bit"
    );
    let bone = if bit(reader, "movement base bone bit")? {
        Some(read_network_name(reader, codec).context("movement base bone")?)
    } else {
        None
    };
    let mode = optional_byte(reader, "movement mode")?;
    let base_relative = bit(reader, "base-relative bit")?;
    let root_motion = if montage {
        let track_position = read_float(reader, order, "root-motion track position")?;
        let mut rotation = [0; 3];
        for axis in &mut rotation {
            *axis = reader.read_bits(16) as u16;
            ensure!(!reader.is_error(), "truncated root-motion rotation");
        }
        Some(RootMotionMontage {
            track_position,
            rotation,
        })
    } else {
        None
    };
    let correction = MoveCorrection {
        timestamp: finite(timestamp, "correction timestamp")?,
        location,
        velocity,
        rotation,
        base,
        bone,
        mode,
        base_relative,
        root_motion,
    };
    check_correction(&correction)?;
    Ok(correction)
}

/// Read a field-30 payload strictly, an acknowledgement or a correction: a
/// set presence bit, a canonical packed count of at most
/// [`MAX_MOVE_RESPONSE_DATA_BITS`] equal to the remaining bits, and a
/// container that consumes exactly those bits. Error messages carry no
/// values.
pub fn read_move_response(payload: &Payload, codec: &RepLayoutCodec) -> Result<MoveResponse> {
    let order = codec.actor.order;
    let mut reader = payload.reader();
    ensure!(
        bit(&mut reader, "move response presence bit")?,
        "move response parameter absent"
    );
    let declared = read_packed(&mut reader, "move response bit count")?;
    ensure!(
        declared <= MAX_MOVE_RESPONSE_DATA_BITS,
        "move response exceeds its bit cap"
    );
    ensure!(
        declared as usize == reader.remaining(),
        "move response bit count differs from its field payload"
    );
    let good = bit(&mut reader, "good move bit")?;
    let timestamp = f32::from_bits(read_u32(&mut reader, order, "move response timestamp")?);
    let response = if good {
        MoveResponse::Ack(timestamp)
    } else {
        MoveResponse::Correction(read_correction(&mut reader, timestamp, codec)?)
    };
    ensure!(reader.at_end(), "move response leaves trailing bits");
    Ok(response)
}

/// Field-30 bits for a response payload: the index under the pawn's bound
/// 129 (7 bits), the packed payload bit count, then the payload.
pub fn write_move_response_field(payload: &Payload) -> Result<Payload> {
    let mut field = BitWriter::new();
    write_field_header(
        &mut field,
        &FieldHeader {
            index: CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX,
            payload: payload.clone(),
        },
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
    )?;
    Ok(Payload::from_writer(field))
}

/// An actor bunch body without spawn holding one actor block without a rep
/// layout whose only field is field 30 carrying `payload`: the framing of
/// `pawn_open::compose_pawn_move_ack`, for any response payload.
pub fn compose_move_response_block(
    payload: &Payload,
    codec: &ActorChannelCodec,
) -> Result<Payload> {
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::Actor,
                payload: write_move_response_field(payload)?,
            }],
        },
        codec,
    )
}

/// [`compose_move_response_block`] for a correction.
pub fn compose_move_correction_block(
    correction: &MoveCorrection,
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    compose_move_response_block(&write_move_correction(correction, codec)?, &codec.actor)
}

/// What a verification input line holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveResponseInput {
    /// A field-30 parameter payload (presence bit onward).
    Payload,
    /// A content block payload holding field 30 alone (index, count,
    /// payload).
    Field,
}

/// Whether a decoded input is an acknowledgement or a correction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveResponseKind {
    Ack,
    Correction,
}

/// A verification result carrying no decoded values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MoveResponseCheck {
    pub bits: usize,
    pub kind: MoveResponseKind,
    pub present: Vec<&'static str>,
    /// The writer reproduces the input bit for bit.
    pub reencode_identical: bool,
}

/// Decode `input` strictly, then re-encode it and compare. An `Err` is a
/// refusal; its outermost message is static text without values.
pub fn check_move_response(
    input: &Payload,
    kind: MoveResponseInput,
    codec: &RepLayoutCodec,
) -> Result<MoveResponseCheck> {
    let payload = match kind {
        MoveResponseInput::Payload => input.clone(),
        MoveResponseInput::Field => {
            let mut reader = input.reader();
            let field = read_field_header(
                &mut reader,
                BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
                usize::MAX,
            )
            .context("field header")?;
            ensure!(
                field.index == CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX,
                "not field 30"
            );
            ensure!(reader.at_end(), "field 30 is not alone in the input");
            field.payload
        }
    };
    let response = read_move_response(&payload, codec)?;
    let (kind_out, present) = match &response {
        MoveResponse::Ack(_) => (MoveResponseKind::Ack, Vec::new()),
        MoveResponse::Correction(correction) => {
            (MoveResponseKind::Correction, correction.presence().names())
        }
    };
    let reencoded = write_move_response(&response, codec).and_then(|payload| match kind {
        MoveResponseInput::Payload => Ok(payload),
        MoveResponseInput::Field => write_move_response_field(&payload),
    });
    Ok(MoveResponseCheck {
        bits: input.bit_len(),
        kind: kind_out,
        present,
        reencode_identical: reencoded.is_ok_and(|bits| bits == *input),
    })
}

#[cfg(test)]
mod tests {
    use super::super::control_messages::ArchiveText;
    use super::super::rep_layout::{RepLayoutLimits, quantize_vector};
    use super::super::session_open::actor_codec;
    use super::*;

    fn codec(order: ByteOrder) -> RepLayoutCodec {
        RepLayoutCodec {
            actor: actor_codec(17, order),
            checksums: false,
            limits: RepLayoutLimits::default(),
        }
    }

    fn fields(timestamp: f32, location: [f32; 3], flags: Option<u8>) -> MoveFields {
        MoveFields {
            timestamp,
            acceleration: quantize_vector([12.5, -3.0, 0.0], VectorPacking::NetQuantize10).unwrap(),
            location: quantize_vector(location, VectorPacking::NetQuantize100).unwrap(),
            rotation: CompressedRotator {
                pitch: Some(0x0100),
                yaw: Some(0xc000),
                roll: None,
            },
            flags,
        }
    }

    /// Authored moves covering every optional part.
    fn authored() -> Vec<PackedMove> {
        let simple = PackedMove {
            new: NewMove {
                fields: fields(1.5, [100.0, -200.0, 50.25], None),
                base: None,
                bone: None,
                mode: None,
            },
            pending: None,
            old: None,
            disable_combined: false,
        };
        let full = PackedMove {
            new: NewMove {
                fields: fields(812.25, [-123_456.78, 98_765.4, -1_000.0], Some(0x14)),
                base: Some(0x0003_d2e5),
                bone: Some(NetworkName::String {
                    text: ArchiveText::text("AuthoredBone").unwrap(),
                    number: 0,
                }),
                mode: Some(3),
            },
            pending: Some((
                true,
                fields(812.2, [-123_450.0, 98_760.0, -1_000.0], Some(1)),
            )),
            old: Some(fields(811.9, [-123_400.0, 98_700.0, -999.0], None)),
            disable_combined: true,
        };
        let hardcoded_bone = PackedMove {
            new: NewMove {
                fields: fields(0.0, [0.0; 3], Some(0)),
                base: Some(7),
                bone: Some(NetworkName::Hardcoded(0)),
                mode: Some(MOVEMENT_MODE_NONE),
            },
            pending: Some((false, fields(0.01, [1.0, 1.0, 1.0], None))),
            old: None,
            disable_combined: false,
        };
        vec![simple, full, hardcoded_bone]
    }

    #[test]
    fn authored_moves_round_trip_in_both_byte_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = codec(order);
            for packed in authored() {
                let payload = write_server_move_packed(&packed, &codec).unwrap();
                assert_eq!(read_server_move_packed(&payload, &codec).unwrap(), packed);
            }
        }
        let [simple, full, _] = &authored()[..] else {
            unreachable!()
        };
        assert_eq!(simple.new.movement_mode(), MOVEMENT_MODE_WALKING);
        assert_eq!(simple.new.fields.compressed_flags(), 0);
        assert_eq!(full.new.movement_mode(), 3);
        assert_eq!(simple.new.fields.location_cm(), [100.0, -200.0, 50.25]);
    }

    /// Every bit of a payload counts: each shorter prefix and each longer
    /// payload (a stray trailing bit) is refused, as are a container whose
    /// declared count exceeds the cap or disagrees with the payload.
    #[test]
    fn truncation_trailing_bits_and_the_cap_are_refused() {
        let codec = codec(ByteOrder::Little);
        for packed in authored() {
            let payload = write_server_move_packed(&packed, &codec).unwrap();
            let mut reader = payload.reader();
            for length in 0..payload.bit_len() {
                let mut prefix = BitWriter::new();
                let mut source = payload.reader();
                for _ in 0..length {
                    prefix.write_bit(source.read_bit());
                }
                assert!(
                    read_server_move_packed(&Payload::from_writer(prefix), &codec).is_err(),
                    "prefix of {length} bits"
                );
            }
            let mut longer = BitWriter::new();
            for _ in 0..payload.bit_len() {
                longer.write_bit(reader.read_bit());
            }
            longer.write_bit(false);
            assert!(read_server_move_packed(&Payload::from_writer(longer), &codec).is_err());
        }
        // A container declaring more than the cap, though present in full.
        let mut over = BitWriter::new();
        over.write_bit(true);
        over.write_int_packed(MAX_SERVER_MOVE_DATA_BITS + 1);
        for _ in 0..=MAX_SERVER_MOVE_DATA_BITS {
            over.write_bit(false);
        }
        let error = read_server_move_packed(&Payload::from_writer(over), &codec).unwrap_err();
        assert!(error.to_string().contains("cap"), "{error}");
        // Absent parameter.
        let mut absent = BitWriter::new();
        absent.write_bit(false);
        assert!(read_server_move_packed(&Payload::from_writer(absent), &codec).is_err());
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    /// No input panics: every single-bit flip of each authored move, and
    /// random bit strings (some framed with a plausible presence bit and
    /// matching count), either decode to a move that re-encodes to the same
    /// bits or are refused. Acknowledgement reads likewise.
    #[test]
    fn mutated_and_random_inputs_never_panic() {
        let codec = codec(ByteOrder::Little);
        let check = |payload: &Payload| {
            if let Ok(packed) = read_server_move_packed(payload, &codec) {
                assert_eq!(&write_server_move_packed(&packed, &codec).unwrap(), payload);
            }
            let _ = read_good_move_ack(payload, ByteOrder::Little);
        };
        for packed in authored() {
            let payload = write_server_move_packed(&packed, &codec).unwrap();
            for flip in 0..payload.bit_len() {
                let mut source = payload.reader();
                let mut mutated = BitWriter::new();
                for index in 0..payload.bit_len() {
                    mutated.write_bit(source.read_bit() ^ (index == flip));
                }
                check(&Payload::from_writer(mutated));
            }
        }
        let mut rng = Rng(0x6d6f_7665_5f61_636b);
        for round in 0..20_000 {
            let mut writer = BitWriter::new();
            let bits = if round % 2 == 0 {
                // Framed: presence and a count matching the random bits.
                let data = rng.below(800);
                writer.write_bit(true);
                writer.write_int_packed(data as u32);
                writer.bit_len() + data
            } else {
                rng.below(900)
            };
            while writer.bit_len() < bits {
                writer.write_bit(rng.next() & 1 == 1);
            }
            check(&Payload::from_writer(writer));
        }
    }

    /// Fields 39 and 30 take 7 bits each under the pawn's bound 129 (F-1105)
    /// and read back as themselves.
    #[test]
    fn pawn_move_field_indices_are_seven_bits_under_bound_129() {
        use super::super::rep_rpc::{
            BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND as BOUND,
            CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX, SERVER_MOVE_PACKED_FIELD_INDEX,
        };
        assert_eq!(BOUND, 129);
        assert_eq!(SERVER_MOVE_PACKED_FIELD_INDEX, 39);
        assert_eq!(CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX, 30);
        for index in [
            SERVER_MOVE_PACKED_FIELD_INDEX,
            CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX,
        ] {
            let mut writer = BitWriter::new();
            writer.write_int_max(index, BOUND).unwrap();
            assert_eq!(writer.bit_len(), 7, "field {index}");
            let payload = Payload::from_writer(writer);
            assert_eq!(payload.reader().read_int_max(BOUND), index);
        }
    }

    /// Golden acknowledgement bits for authored timestamps: 42 bits, read
    /// back to the same timestamp; a cleared good-move bit is refused.
    #[test]
    fn good_move_ack_payload_is_42_bits_and_reads_back() {
        for timestamp in [0.0_f32, 1.5, 812.25, 3.25e4] {
            for order in [ByteOrder::Little, ByteOrder::Big] {
                let payload = write_good_move_ack(timestamp, order);
                assert_eq!(payload.bit_len(), GOOD_MOVE_ACK_PAYLOAD_BITS);
                assert_eq!(
                    read_good_move_ack(&payload, order).unwrap().to_bits(),
                    timestamp.to_bits()
                );
            }
        }
        // 1.5 = 0x3fc00000, little-endian: presence 1, packed 33 = 0x42
        // (group 33 << 1, no continuation), good-move 1, then the float.
        let payload = write_good_move_ack(1.5, ByteOrder::Little);
        let mut reader = payload.reader();
        assert!(reader.read_bit());
        assert_eq!(reader.read_bits(8), 0x42);
        assert!(reader.read_bit());
        assert_eq!(reader.read_u32(), 0x3fc0_0000);
        assert!(reader.at_end());
        let mut correction = BitWriter::new();
        correction.write_bit(true);
        correction.write_int_packed(GOOD_MOVE_ACK_DATA_BITS);
        correction.write_bit(false);
        correction.write_u32(0);
        assert!(read_good_move_ack(&Payload::from_writer(correction), ByteOrder::Little).is_err());
    }
}

#[cfg(test)]
mod correction_tests {
    use super::super::actor_channel::read_actor_bunch_body;
    use super::super::control_messages::ArchiveText;
    use super::*;

    const ORDERS: [ByteOrder; 2] = [ByteOrder::Little, ByteOrder::Big];

    fn codec(order: ByteOrder) -> RepLayoutCodec {
        move_response_codec(17, order)
    }

    fn name(text: &str) -> NetworkName {
        NetworkName::String {
            text: ArchiveText::text(text).unwrap(),
            number: 0,
        }
    }

    /// No optional part: walking, no base, bone, rotation or root motion.
    fn bare() -> MoveCorrection {
        MoveCorrection {
            timestamp: 47.023,
            location: [-123_456.78, 98_765.4, 31_000.5],
            velocity: [0.0, -0.0, -981.0],
            rotation: None,
            base: None,
            bone: None,
            mode: None,
            base_relative: false,
            root_motion: None,
        }
    }

    /// Location, velocity and the falling mode only (F-1132's shape).
    fn falling() -> MoveCorrection {
        MoveCorrection {
            mode: Some(MOVEMENT_MODE_FALLING),
            ..bare()
        }
    }

    /// `falling` with a two-group base NetGUID and an eight-character bone.
    fn based() -> MoveCorrection {
        MoveCorrection {
            base: Some(0x2a6),
            bone: Some(name("AuthBone")),
            ..falling()
        }
    }

    /// Every optional part.
    fn full() -> MoveCorrection {
        MoveCorrection {
            timestamp: 812.25,
            location: [10.5, -20.25, 30.125],
            velocity: [400.0, -1.5e3, 7.25],
            rotation: Some(CompressedRotator {
                pitch: Some(0x1234),
                yaw: None,
                roll: Some(0xfedc),
            }),
            base: Some(0x0003_d2e5),
            bone: Some(name("AuthoredBone")),
            mode: Some(MOVEMENT_MODE_FALLING | PACKED_MOVEMENT_MODE_GROUND_BIT),
            base_relative: true,
            root_motion: Some(RootMotionMontage {
                track_position: 1.25,
                rotation: [1, 0x8000, 0xffff],
            }),
        }
    }

    /// Each optional part alone, with its expected extra container bits.
    fn singles() -> Vec<(&'static str, MoveCorrection, usize)> {
        vec![
            (
                "rotation",
                MoveCorrection {
                    rotation: Some(CompressedRotator {
                        pitch: None,
                        yaw: Some(0x4000),
                        roll: None,
                    }),
                    ..bare()
                },
                3 + 16,
            ),
            (
                "base",
                MoveCorrection {
                    base: Some(6),
                    ..bare()
                },
                8,
            ),
            (
                "bone",
                MoveCorrection {
                    bone: Some(NetworkName::Hardcoded(300)),
                    ..bare()
                },
                1 + 16,
            ),
            (
                "mode",
                MoveCorrection {
                    mode: Some(5),
                    ..bare()
                },
                8,
            ),
            (
                "base_relative",
                MoveCorrection {
                    base_relative: true,
                    ..bare()
                },
                0,
            ),
            (
                "root_motion_montage",
                MoveCorrection {
                    root_motion: Some(RootMotionMontage {
                        track_position: 0.5,
                        rotation: [0x8000, 0x7fff, 0],
                    }),
                    ..bare()
                },
                32 + 48,
            ),
        ]
    }

    /// Container bits of a correction without optional contents: the good-move
    /// bit, timestamp, four flags, two 3 x f32 vectors, the base, bone and
    /// mode presence bits and the base-relative bit.
    const BARE_DATA_BITS: usize = 1 + 32 + 4 + 96 + 96 + 1 + 1 + 1 + 1;

    /// A field-30 payload's bits for `data` container bits (two packed-count
    /// groups from 128 up).
    fn payload_bits(data: usize) -> usize {
        1 + if data < 128 { 8 } else { 16 } + data
    }

    fn reframe(data: &BitWriter, declared: u32) -> Payload {
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        writer.write_int_packed(declared);
        append_bits(&mut writer, &Payload::from_writer(data.clone()));
        Payload::from_writer(writer)
    }

    fn bits_of(payload: &Payload) -> Vec<bool> {
        let mut reader = payload.reader();
        (0..payload.bit_len()).map(|_| reader.read_bit()).collect()
    }

    fn from_bits(bits: &[bool]) -> Payload {
        let mut writer = BitWriter::new();
        for bit in bits {
            writer.write_bit(*bit);
        }
        Payload::from_writer(writer)
    }

    fn encode(correction: &MoveCorrection, order: ByteOrder) -> Payload {
        frame_move_response(
            encode_correction(correction, correction.base.is_some(), false, &codec(order)).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn corrections_round_trip_in_both_byte_orders() {
        let mut cases = vec![
            ("bare", bare(), 0),
            ("falling", falling(), 8),
            ("full", full(), 0),
        ];
        cases.extend(singles());
        for order in ORDERS {
            let codec = codec(order);
            for (label, correction, extra) in &cases {
                let payload = write_move_correction(correction, &codec).unwrap();
                let read = read_move_response(&payload, &codec).unwrap();
                assert_eq!(
                    read,
                    MoveResponse::Correction(correction.clone()),
                    "{label}"
                );
                assert_eq!(
                    write_move_response(&read, &codec).unwrap(),
                    payload,
                    "{label}"
                );
                if *label != "full" {
                    assert_eq!(
                        payload.bit_len(),
                        payload_bits(BARE_DATA_BITS + extra),
                        "{label}"
                    );
                }
            }
        }
        // Each single carries exactly its own part.
        for (label, correction, _) in singles() {
            let names = correction.presence().names();
            assert!(
                names.is_empty() && label == "base_relative" || names == [label],
                "{label}"
            );
        }
        assert_eq!(
            full().presence().names(),
            ["rotation", "base", "bone", "mode", "root_motion_montage"]
        );
        assert_eq!(bare().movement_mode(), MOVEMENT_MODE_WALKING);
    }

    /// Golden little-endian bits of `falling`: presence, packed 241 (groups
    /// 0xe3 0x02), good-move clear, the timestamp, four clear flags, then the
    /// six floats; big-endian swaps each float's bytes and nothing else.
    #[test]
    fn falling_correction_golden_prefix() {
        let correction = falling();
        let payload = write_move_correction(&correction, &codec(ByteOrder::Little)).unwrap();
        let mut reader = payload.reader();
        assert!(reader.read_bit());
        assert_eq!(reader.read_bits(8), 0xe3);
        assert_eq!(reader.read_bits(8), 0x02);
        assert!(!reader.read_bit());
        assert_eq!(reader.read_u32(), correction.timestamp.to_bits());
        assert_eq!(reader.read_bits(4), 0);
        for value in correction.location.iter().chain(&correction.velocity) {
            assert_eq!(reader.read_u32(), value.to_bits());
        }
        // Base and bone absent, mode present (3), not base-relative.
        assert!(!reader.read_bit());
        assert!(!reader.read_bit());
        assert!(reader.read_bit());
        assert_eq!(reader.read_bits(8), 3);
        assert!(!reader.read_bit());
        assert!(reader.at_end());
        let big = write_move_correction(&correction, &codec(ByteOrder::Big)).unwrap();
        let mut reader = big.reader();
        reader.read_bits(18);
        assert_eq!(
            reader.read_u32(),
            correction.timestamp.to_bits().swap_bytes()
        );
    }

    /// The layout's lengths for the observed shapes: location, velocity and
    /// mode give a 258-bit payload (281-bit field, 299-bit block); with a base
    /// and bone the payload is 411 bits exactly when the base NetGUID's packed
    /// groups and the narrow bone name's characters add up to 10.
    #[test]
    fn observed_shapes_predict_payload_field_and_block_bits() {
        for order in ORDERS {
            let codec = codec(order);
            for (correction, payload, field, block) in
                [(falling(), 258, 281, 299), (based(), 411, 434, 452)]
            {
                let bits = write_move_correction(&correction, &codec).unwrap();
                assert_eq!(bits.bit_len(), payload);
                assert_eq!(write_move_response_field(&bits).unwrap().bit_len(), field);
                let body = compose_move_correction_block(&correction, &codec).unwrap();
                assert_eq!(body.bit_len(), block);
            }
            for groups in 1..=5_u32 {
                let base = if groups == 1 {
                    2
                } else {
                    1 << (7 * (groups - 1))
                };
                for length in 1..=16_usize {
                    let correction = MoveCorrection {
                        base: Some(base),
                        bone: Some(name(&"B".repeat(length))),
                        ..falling()
                    };
                    let bits = write_move_correction(&correction, &codec)
                        .unwrap()
                        .bit_len();
                    assert_eq!(bits, 258 + 8 * groups as usize + 65 + 8 * (length + 1));
                    assert_eq!(bits == 411, groups as usize + length == 10);
                }
            }
        }
    }

    #[test]
    fn wrong_count_trailing_bits_truncation_and_cap_are_refused() {
        for order in ORDERS {
            let codec = codec(order);
            for correction in [bare(), falling(), based(), full()] {
                let data = encode_correction(&correction, correction.base.is_some(), false, &codec)
                    .unwrap();
                let exact = data.bit_len() as u32;
                assert!(read_move_response(&reframe(&data, exact), &codec).is_ok());
                for declared in [exact - 1, exact + 1] {
                    assert!(read_move_response(&reframe(&data, declared), &codec).is_err());
                }
                // A stray bit counted in the container.
                let mut longer = data.clone();
                longer.write_bit(false);
                let error = read_move_response(&reframe(&longer, exact + 1), &codec).unwrap_err();
                assert!(error.to_string().contains("trailing"), "{error}");
                // Every prefix.
                let bits = bits_of(&reframe(&data, exact));
                for length in 0..bits.len() {
                    assert!(read_move_response(&from_bits(&bits[..length]), &codec).is_err());
                }
            }
            // A container above the cap, present in full.
            let mut over = BitWriter::new();
            for _ in 0..=MAX_MOVE_RESPONSE_DATA_BITS {
                over.write_bit(false);
            }
            let error =
                read_move_response(&reframe(&over, MAX_MOVE_RESPONSE_DATA_BITS + 1), &codec)
                    .unwrap_err();
            assert!(error.to_string().contains("cap"), "{error}");
            let long_bone = MoveCorrection {
                bone: Some(name(&"B".repeat(240))),
                ..bare()
            };
            let error = write_move_correction(&long_bone, &codec).unwrap_err();
            assert!(error.to_string().contains("cap"), "{error}");
            // Absent parameter.
            assert!(read_move_response(&from_bits(&[false]), &codec).is_err());
        }
    }

    #[test]
    fn non_finite_floats_are_refused_both_ways() {
        for order in ORDERS {
            let codec = codec(order);
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let variants = [
                    MoveCorrection {
                        timestamp: bad,
                        ..falling()
                    },
                    MoveCorrection {
                        location: [0.0, bad, 0.0],
                        ..falling()
                    },
                    MoveCorrection {
                        velocity: [0.0, 0.0, bad],
                        ..falling()
                    },
                    MoveCorrection {
                        root_motion: Some(RootMotionMontage {
                            track_position: bad,
                            rotation: [0; 3],
                        }),
                        ..falling()
                    },
                ];
                for correction in variants {
                    let error = write_move_correction(&correction, &codec).unwrap_err();
                    assert!(error.to_string().contains("not finite"), "{error}");
                    let error =
                        read_move_response(&encode(&correction, order), &codec).unwrap_err();
                    assert!(error.to_string().contains("not finite"), "{error}");
                }
            }
        }
    }

    #[test]
    fn movement_mode_range_and_default_are_enforced() {
        let codec = codec(ByteOrder::Little);
        for mode in 0..=u8::MAX {
            let correction = MoveCorrection {
                mode: Some(mode),
                ..bare()
            };
            let valid = mode != MOVEMENT_MODE_WALKING && !matches!(mode, 6 | 7 | 14 | 15);
            assert_eq!(
                is_packed_movement_mode(mode),
                !matches!(mode, 6 | 7 | 14 | 15)
            );
            assert_eq!(
                write_move_correction(&correction, &codec).is_ok(),
                valid,
                "{mode}"
            );
            assert_eq!(
                read_move_response(&encode(&correction, ByteOrder::Little), &codec).is_ok(),
                valid,
                "{mode}"
            );
        }
    }

    /// Forms the original writer cannot produce: a root-motion source
    /// correction, a has-base flag disagreeing with the base bit, the default
    /// NetGUID as a base, a bone that is `None` or a hardcoded index above
    /// 410, and a rotator axis sent as zero.
    #[test]
    fn unsupported_and_non_canonical_corrections_are_refused() {
        for order in ORDERS {
            let codec = codec(order);
            let read = |correction: &MoveCorrection, has_base: bool, source: bool| {
                let data = encode_correction(correction, has_base, source, &codec).unwrap();
                read_move_response(&frame_move_response(data).unwrap(), &codec)
            };
            let error = read(&falling(), false, true).unwrap_err();
            assert!(error.to_string().contains("source"), "{error}");
            let error = read(&based(), false, false).unwrap_err();
            assert!(error.to_string().contains("has-base"), "{error}");
            let error = read(&falling(), true, false).unwrap_err();
            assert!(error.to_string().contains("has-base"), "{error}");
            let refused = [
                MoveCorrection {
                    base: Some(1),
                    ..falling()
                },
                MoveCorrection {
                    bone: Some(NetworkName::Hardcoded(0)),
                    ..falling()
                },
                MoveCorrection {
                    bone: Some(NetworkName::Hardcoded(MAX_SENT_HARDCODED_NAME_INDEX + 1)),
                    ..falling()
                },
                MoveCorrection {
                    rotation: Some(CompressedRotator {
                        pitch: Some(0),
                        yaw: None,
                        roll: None,
                    }),
                    ..falling()
                },
            ];
            for correction in refused {
                assert!(write_move_correction(&correction, &codec).is_err());
                assert!(read(&correction, correction.base.is_some(), false).is_err());
            }
            let accepted = [
                MoveCorrection {
                    base: Some(0),
                    ..falling()
                },
                MoveCorrection {
                    bone: Some(NetworkName::Hardcoded(MAX_SENT_HARDCODED_NAME_INDEX)),
                    ..falling()
                },
            ];
            for correction in accepted {
                let payload = write_move_correction(&correction, &codec).unwrap();
                assert_eq!(
                    read_move_response(&payload, &codec).unwrap(),
                    MoveResponse::Correction(correction)
                );
            }
        }
    }

    /// Rotator values follow the archive byte order like the floats.
    #[test]
    fn rotation_axes_swap_with_the_byte_order() {
        let correction = MoveCorrection {
            rotation: Some(CompressedRotator {
                pitch: Some(0x1234),
                yaw: None,
                roll: None,
            }),
            ..bare()
        };
        let little = write_move_correction(&correction, &codec(ByteOrder::Little)).unwrap();
        let big = write_move_correction(&correction, &codec(ByteOrder::Big)).unwrap();
        let axis = |payload: &Payload| {
            let mut reader = payload.reader();
            // Presence, packed count, good-move bit, timestamp, flags.
            reader.read_bits(17);
            assert!(!reader.read_bit());
            reader.read_u32();
            assert_eq!(reader.read_bits(4), 0b0010);
            for _ in 0..6 {
                reader.read_u32();
            }
            assert!(reader.read_bit());
            reader.read_bits(16)
        };
        assert_eq!(axis(&little), 0x1234);
        assert_eq!(axis(&big), 0x3412);
    }

    /// The response reader takes acknowledgements too, agreeing with
    /// `read_good_move_ack`.
    #[test]
    fn acknowledgements_read_through_the_response_reader() {
        for order in ORDERS {
            let codec = codec(order);
            for timestamp in [0.0_f32, 1.5, 812.25] {
                let payload = write_good_move_ack(timestamp, order);
                let MoveResponse::Ack(read) = read_move_response(&payload, &codec).unwrap() else {
                    panic!("an acknowledgement");
                };
                assert_eq!(read.to_bits(), timestamp.to_bits());
                assert_eq!(
                    read_good_move_ack(&payload, order).unwrap().to_bits(),
                    read.to_bits()
                );
                let correction = write_move_correction(&falling(), &codec).unwrap();
                assert!(read_good_move_ack(&correction, order).is_err());
            }
        }
    }

    /// The field-30 framing reproduces the acknowledgement's 67-bit block, and
    /// a correction's block reads back as field 30 alone with its payload.
    #[test]
    fn correction_block_uses_the_acknowledgement_framing() {
        use super::super::pawn_open::compose_pawn_move_ack;
        use super::super::peer_lab::PeerLabAdmission;
        use super::super::release::ReleaseProfile;
        let map = PeerLabAdmission::default().map_url;
        let plan = ReleaseProfile::fixture("3.0.35")
            .pawn_open_plan(map)
            .unwrap()
            .unwrap();
        for order in ORDERS {
            let codec = move_response_codec(plan.engine_net_version, order);
            let ack = write_good_move_ack(1.5, order);
            assert_eq!(
                compose_move_response_block(&ack, &codec.actor).unwrap(),
                compose_pawn_move_ack(&plan, 1.5, order).unwrap()
            );
            for correction in [falling(), based(), full()] {
                let body = compose_move_correction_block(&correction, &codec).unwrap();
                let read = read_actor_bunch_body(&mut body.reader(), false, &codec.actor).unwrap();
                let [block] = read.blocks.as_slice() else {
                    panic!("one block");
                };
                assert!(!block.has_rep_layout);
                assert_eq!(block.target, BlockTarget::Actor);
                let mut reader = block.payload.reader();
                let field = read_field_header(
                    &mut reader,
                    BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
                    usize::MAX,
                )
                .unwrap();
                assert!(reader.at_end());
                assert_eq!(field.index, CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX);
                assert_eq!(
                    read_move_response(&field.payload, &codec).unwrap(),
                    MoveResponse::Correction(correction)
                );
            }
        }
    }

    #[test]
    fn verification_check_reports_kind_presence_and_identity() {
        for order in ORDERS {
            let codec = codec(order);
            let ack = write_good_move_ack(2.0, order);
            let correction = write_move_correction(&based(), &codec).unwrap();
            for (input, kind) in [
                (ack.clone(), MoveResponseInput::Payload),
                (
                    write_move_response_field(&ack).unwrap(),
                    MoveResponseInput::Field,
                ),
            ] {
                let check = check_move_response(&input, kind, &codec).unwrap();
                assert_eq!(check.kind, MoveResponseKind::Ack);
                assert!(check.present.is_empty() && check.reencode_identical);
                assert_eq!(check.bits, input.bit_len());
            }
            for (input, kind) in [
                (correction.clone(), MoveResponseInput::Payload),
                (
                    write_move_response_field(&correction).unwrap(),
                    MoveResponseInput::Field,
                ),
            ] {
                let check = check_move_response(&input, kind, &codec).unwrap();
                assert_eq!(check.kind, MoveResponseKind::Correction);
                assert_eq!(check.present, ["base", "bone", "mode"]);
                assert!(check.reencode_identical);
            }
            // A payload given as a field, and a refused correction.
            assert!(check_move_response(&correction, MoveResponseInput::Field, &codec).is_err());
            let source =
                frame_move_response(encode_correction(&falling(), false, true, &codec).unwrap())
                    .unwrap();
            let error =
                check_move_response(&source, MoveResponseInput::Payload, &codec).unwrap_err();
            assert_eq!(
                error.to_string(),
                "root-motion source corrections are not supported"
            );
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    /// No input panics: single-bit flips of authored corrections and random
    /// bit strings (half framed with a presence bit and matching count)
    /// either decode to a response that re-encodes to the same bits or are
    /// refused, as payloads and as fields.
    #[test]
    fn mutated_and_random_inputs_never_panic() {
        for order in ORDERS {
            let codec = codec(order);
            let check = |payload: &Payload| {
                if let Ok(response) = read_move_response(payload, &codec) {
                    assert_eq!(&write_move_response(&response, &codec).unwrap(), payload);
                }
                for kind in [MoveResponseInput::Payload, MoveResponseInput::Field] {
                    if let Ok(check) = check_move_response(payload, kind, &codec) {
                        assert!(check.reencode_identical);
                    }
                }
            };
            for correction in [bare(), based(), full()] {
                let bits = bits_of(&write_move_correction(&correction, &codec).unwrap());
                for flip in 0..bits.len() {
                    let mut mutated = bits.clone();
                    mutated[flip] = !mutated[flip];
                    check(&from_bits(&mutated));
                }
            }
            let mut rng = Rng(0x636f_7272_6563_7421);
            for round in 0..20_000 {
                let mut writer = BitWriter::new();
                let bits = if round % 2 == 0 {
                    let data = rng.below(700);
                    writer.write_bit(true);
                    writer.write_int_packed(data as u32);
                    if data > 0 {
                        // Mostly corrections: a clear good-move bit first.
                        writer.write_bit(round % 4 == 0);
                    }
                    writer.bit_len() + data.saturating_sub(1)
                } else {
                    rng.below(800)
                };
                while writer.bit_len() < bits {
                    writer.write_bit(rng.next() & 1 == 1);
                }
                check(&Payload::from_writer(writer));
            }
        }
    }
}
