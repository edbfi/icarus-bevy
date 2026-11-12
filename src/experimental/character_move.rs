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
//! bits in all ([`write_good_move_ack`]). Corrections are never written.
use anyhow::{Context, Result, ensure};

use super::actor_channel::{CompressedRotator, read_packed, read_rotator, write_rotator};
use super::archive::ByteOrder;
use super::packet::{BitReader, BitWriter, Payload};
use super::rep_layout::{
    NetworkName, QuantizedVector, RepLayoutCodec, VectorPacking, append_bits, read_network_name,
    read_u32, read_vector, write_network_name, write_u32, write_vector,
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
