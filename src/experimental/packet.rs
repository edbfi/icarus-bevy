//! EXPERIMENTAL codec for the statically observed client packet receive format.
//!
//! Static client-build observations only (F-521..F-525): the current-version
//! path with every version gate on and the internal-ack flag clear. Not
//! wire-validated against any original packet. The sender side was not
//! observed; writers mirror the readers (and the observed `WriteIntWrapped`
//! loop) for round-trip research only. Negotiated version values, the
//! connection's channel limit and maximum packet size are unknown and supplied
//! by the caller. No connection state, ack tracking, relative reliable-sequence
//! reconstruction, partial-bunch assembly, channel routing or admission occurs.
//! Field roles beyond their encodings are inferred.
use anyhow::{Context, Result, ensure};

use super::archive::{ByteOrder, read_string_frame};

/// Highest hard-coded name index accepted on the current-version path (F-525).
pub const MAX_HARDCODED_NAME_INDEX: u32 = 702;
/// Maximum ack-history words after the packet word (F-521).
pub const MAX_HISTORY_WORDS: usize = 8;
/// Packet-info jitter values are below this bound (F-525).
pub const JITTER_CLOCK_LIMIT: u32 = 1024;
/// Reliable sequences are serialized below this bound, before reconstruction.
pub const RELIABLE_SEQUENCE_LIMIT: u32 = 1024;
/// Close reasons are serialized below this bound.
pub const CLOSE_REASON_LIMIT: u32 = 15;
const SEQUENCE_MASK: u32 = 0x3fff;

/// LSB-first bit reader with a sticky overflow state.
///
/// A read that does not fit sets the error state, consumes nothing and returns
/// zero (`false`, an empty byte vector). Once in error, every read does the same,
/// so values read after `is_error()` became true are meaningless.
pub struct BitReader<'a> {
    bytes: &'a [u8],
    bit_len: usize,
    position: usize,
    error: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8], bit_len: usize) -> Result<Self> {
        ensure!(
            bytes
                .len()
                .checked_mul(8)
                .is_some_and(|bits| bit_len <= bits),
            "bit length exceeds the supplied bytes"
        );
        Ok(Self {
            bytes,
            bit_len,
            position: 0,
            error: false,
        })
    }

    pub fn is_error(&self) -> bool {
        self.error
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.bit_len - self.position
    }

    pub fn at_end(&self) -> bool {
        self.remaining() == 0
    }

    fn fits(&mut self, bits: usize) -> bool {
        if self.error || bits > self.remaining() {
            self.error = true;
            return false;
        }
        true
    }

    fn bit_at(&self, index: usize) -> bool {
        (self.bytes[index / 8] >> (index % 8)) & 1 == 1
    }

    pub fn read_bit(&mut self) -> bool {
        if !self.fits(1) {
            return false;
        }
        let bit = self.bit_at(self.position);
        self.position += 1;
        bit
    }

    /// Read `bits` (at most 32) LSB-first; more than 32 sets the error state.
    pub fn read_bits(&mut self, bits: u32) -> u32 {
        if bits > 32 {
            self.error = true;
            return 0;
        }
        if !self.fits(bits as usize) {
            return 0;
        }
        let mut value = 0;
        for shift in 0..bits {
            value |= u32::from(self.bit_at(self.position)) << shift;
            self.position += 1;
        }
        value
    }

    /// Read `count` bytes of 8 LSB-first bits each, from any bit position.
    /// Overflow returns an empty vector without allocating `count` bytes.
    pub fn read_bytes(&mut self, count: usize) -> Vec<u8> {
        if !count.checked_mul(8).is_some_and(|bits| self.fits(bits)) {
            self.error = true;
            return Vec::new();
        }
        (0..count).map(|_| self.read_bits(8) as u8).collect()
    }

    /// 32 bits LSB-first: little-endian when byte aligned.
    pub fn read_u32(&mut self) -> u32 {
        self.read_bits(32)
    }

    /// `SerializeInt(V, Max)` (F-523): masks 1, 2, 4, ... while value + mask is
    /// below `max`. Reads nothing when `max <= 1`. The result is below `max`.
    pub fn read_int_max(&mut self, max: u32) -> u32 {
        let mut value = 0u32;
        let mut mask = 1u32;
        while u64::from(value) + u64::from(mask) < u64::from(max) && mask != 0 {
            if self.read_bit() {
                value |= mask;
            }
            mask <<= 1;
        }
        value
    }

    /// `SerializeIntPacked` (F-523): up to five bytes from the current, possibly
    /// unaligned position; bit 0 continues, bits 1-7 are the next 7-bit group.
    /// Running out of bits sets the error state and returns the groups read so far.
    /// Bits above 32 in the fifth group are discarded.
    pub fn read_int_packed(&mut self) -> u32 {
        let mut value = 0u32;
        for group in 0..5u32 {
            if !self.fits(8) {
                return value;
            }
            let byte = self.read_bits(8);
            value |= (byte >> 1) << (group * 7);
            if byte & 1 == 0 {
                break;
            }
        }
        value
    }
}

/// Growable, zero-initialized LSB-first bit writer.
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    /// Written bytes; bits past `bit_len` in the last byte are zero.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn write_bit(&mut self, bit: bool) {
        if self.bit_len.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if bit {
            self.bytes[self.bit_len / 8] |= 1 << (self.bit_len % 8);
        }
        self.bit_len += 1;
    }

    /// Write the low `bits` (at most 32) of `value` LSB-first.
    ///
    /// # Panics
    /// When `bits > 32`.
    pub fn write_bits(&mut self, value: u32, bits: u32) {
        assert!(bits <= 32, "at most 32 bits per write");
        for shift in 0..bits {
            self.write_bit((value >> shift) & 1 == 1);
        }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_bits(u32::from(*byte), 8);
        }
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write_bits(value, 32);
    }

    /// Mirror of `read_int_max`; `value >= max` is clamped to `max - 1`.
    /// Writes nothing when `max == 1`; `max == 0` is rejected.
    pub fn write_int_max(&mut self, value: u32, max: u32) -> Result<()> {
        ensure!(max >= 1, "integer maximum must be at least one");
        self.write_int_wrapped(value.min(max - 1), max);
        Ok(())
    }

    /// The observed sender's `WriteIntWrapped` loop: as `write_int_max` but
    /// without clamping. For a power-of-two `max` an out-of-range value keeps its
    /// low log2(max) bits; otherwise the loop stops once the bits written so far
    /// plus the next mask reach `max`, so the decoded value is the greedy low-bit
    /// prefix of `value` (for example 20 below 10 decodes as 4). Writes nothing
    /// when `max <= 1`.
    pub fn write_int_wrapped(&mut self, value: u32, max: u32) {
        let mut mask = 1u32;
        let mut written = 0u32;
        while u64::from(mask) + u64::from(written) < u64::from(max) && mask != 0 {
            let bit = value & mask != 0;
            self.write_bit(bit);
            if bit {
                written += mask;
            }
            mask <<= 1;
        }
    }

    /// Mirror of `read_int_packed`; at least one byte.
    pub fn write_int_packed(&mut self, mut value: u32) {
        loop {
            let group = value & 0x7f;
            value >>= 7;
            let more = value != 0;
            self.write_bits((group << 1) | u32::from(more), 8);
            if !more {
                break;
            }
        }
    }

    fn write_reader_bits(&mut self, reader: &mut BitReader<'_>, mut bits: usize) {
        while bits > 0 {
            let chunk = bits.min(32) as u32;
            self.write_bits(reader.read_bits(chunk), chunk);
            bits -= chunk as usize;
        }
    }
}

/// Split the termination bit (F-524): the highest set bit of the nonzero last
/// byte ends the packet bits. Returns the datagram and the bit length before it.
pub fn split_termination(datagram: &[u8]) -> Result<(&[u8], usize)> {
    let last = *datagram.last().context("empty datagram")?;
    ensure!(last != 0, "datagram last byte lacks a termination bit");
    let highest = 7 - last.leading_zeros() as usize;
    Ok((datagram, (datagram.len() - 1) * 8 + highest))
}

/// Append a termination bit and zero padding to a byte boundary.
pub fn finish_with_termination(mut writer: BitWriter) -> Vec<u8> {
    writer.write_bit(true);
    writer.into_bytes()
}

/// A stateless-handshake packet reached the data-packet reader; not parsed here.
#[derive(Debug)]
pub struct HandshakePacket;

impl std::fmt::Display for HandshakePacket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("handshake packet, not a data packet")
    }
}

impl std::error::Error for HandshakePacket {}

/// Read `magic_bits` (0..=32) magic-header bits, then the handshake bit, which
/// must be 0 for data. A 1 returns a downcastable [`HandshakePacket`] error.
/// The magic value is returned unchecked; its expected value is unknown.
pub fn read_data_prefix(reader: &mut BitReader<'_>, magic_bits: u32) -> Result<u32> {
    ensure!(magic_bits <= 32, "magic header exceeds 32 bits");
    let magic = reader.read_bits(magic_bits);
    let handshake = reader.read_bit();
    ensure!(!reader.is_error(), "truncated data packet prefix");
    if handshake {
        return Err(HandshakePacket.into());
    }
    Ok(magic)
}

pub fn write_data_prefix(writer: &mut BitWriter, magic_bits: u32, magic: u32) -> Result<()> {
    ensure!(magic_bits <= 32, "magic header exceeds 32 bits");
    ensure!(
        magic_bits == 32 || magic >> magic_bits == 0,
        "magic value exceeds its bit width"
    );
    writer.write_bits(magic, magic_bits);
    writer.write_bit(false);
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    /// 14-bit packet sequence.
    pub sequence: u16,
    /// 14-bit acknowledged sequence.
    pub acked_sequence: u16,
    /// 1..=8 raw ack-history words; not interpreted.
    pub history: Vec<u32>,
}

/// Which side receives the packet; only a client reads the frame-time byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    Client,
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketInfo {
    /// Below 1024.
    pub jitter_clock: u32,
    /// The frame-time bit, read by either receiver.
    pub has_frame_time: bool,
    /// Present exactly when `has_frame_time` and the receiver is a client.
    pub server_frame_time: Option<u8>,
}

/// Packet word, history words and the optional packet info (F-521, F-525).
/// A history count field above 7 is capped at 8 words, as observed.
pub fn read_packet_header(
    reader: &mut BitReader<'_>,
    receiver: Receiver,
) -> Result<(PacketHeader, Option<PacketInfo>)> {
    let word = reader.read_u32();
    let count = ((word & 0xf) as usize + 1).min(MAX_HISTORY_WORDS);
    let history = (0..count).map(|_| reader.read_u32()).collect();
    ensure!(!reader.is_error(), "truncated packet header");
    let header = PacketHeader {
        sequence: ((word >> 18) & SEQUENCE_MASK) as u16,
        acked_sequence: ((word >> 4) & SEQUENCE_MASK) as u16,
        history,
    };
    let info = if reader.read_bit() {
        let jitter_clock = reader.read_int_max(JITTER_CLOCK_LIMIT);
        let has_frame_time = reader.read_bit();
        let server_frame_time =
            (has_frame_time && receiver == Receiver::Client).then(|| reader.read_bits(8) as u8);
        Some(PacketInfo {
            jitter_clock,
            has_frame_time,
            server_frame_time,
        })
    } else {
        None
    };
    ensure!(!reader.is_error(), "truncated packet info");
    Ok((header, info))
}

pub fn write_packet_header(
    writer: &mut BitWriter,
    header: &PacketHeader,
    info: Option<&PacketInfo>,
    receiver: Receiver,
) -> Result<()> {
    ensure!(
        (1..=MAX_HISTORY_WORDS).contains(&header.history.len()),
        "packet history must hold 1..=8 words"
    );
    ensure!(
        u32::from(header.sequence) <= SEQUENCE_MASK
            && u32::from(header.acked_sequence) <= SEQUENCE_MASK,
        "packet sequence exceeds 14 bits"
    );
    if let Some(info) = info {
        ensure!(
            info.jitter_clock < JITTER_CLOCK_LIMIT,
            "jitter clock out of range"
        );
        ensure!(
            info.server_frame_time.is_some()
                == (info.has_frame_time && receiver == Receiver::Client),
            "frame-time byte must match the frame-time bit and receiver"
        );
    }
    writer.write_u32(
        (header.history.len() as u32 - 1)
            | u32::from(header.acked_sequence) << 4
            | u32::from(header.sequence) << 18,
    );
    for word in &header.history {
        writer.write_u32(*word);
    }
    writer.write_bit(info.is_some());
    if let Some(info) = info {
        writer.write_int_max(info.jitter_clock, JITTER_CLOCK_LIMIT)?;
        writer.write_bit(info.has_frame_time);
        if let Some(frame_time) = info.server_frame_time {
            writer.write_bits(u32::from(frame_time), 8);
        }
    }
    Ok(())
}

/// Caller-supplied limits; the original negotiated values are unknown.
#[derive(Clone, Copy, Debug)]
pub struct BunchLimits {
    /// Channel indices must be below this.
    pub max_channels: u32,
    /// Payload bit counts must be below eight times this.
    pub max_packet_bytes: u32,
    /// Archive-string unit cap for channel names, including the terminator.
    pub max_string_units: usize,
}

impl BunchLimits {
    fn payload_bit_limit(&self) -> Result<u32> {
        self.max_packet_bytes
            .checked_mul(8)
            .context("maximum packet size overflows the bit count")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Name {
    /// Hard-coded name index, 0..=702; 0 is the None name.
    Hardcoded(u32),
    /// A strict little-endian archive-string frame (signed count included) and
    /// its 32-bit number.
    String { frame: Vec<u8>, number: u32 },
}

impl Name {
    /// Strict archive-string name from UTF-16 units (ASCII preferred).
    pub fn string(units: &[u16], number: u32) -> Result<Self> {
        Ok(Self::String {
            frame: super::archive::encode_string(units, ByteOrder::Little)?,
            number,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BunchHeader {
    /// The reader accepts a set control bit without open/close; the writer
    /// requires `control == open || close`, as the sender writes it.
    pub control: bool,
    pub open: bool,
    pub close: bool,
    /// Present exactly when `close`; below 15.
    pub close_reason: Option<u32>,
    pub paused: bool,
    pub reliable: bool,
    pub channel_index: u32,
    pub has_package_map_exports: bool,
    pub has_must_be_mapped_guids: bool,
    pub partial: bool,
    /// Present exactly when `reliable`: the raw 10-bit serialized value. The
    /// receiver's reconstruction relative to the channel's last sequence is not
    /// implemented.
    pub reliable_sequence_raw: Option<u32>,
    /// False unless `partial`.
    pub partial_initial: bool,
    pub partial_final: bool,
    /// Present exactly when `open || reliable`.
    pub channel_name: Option<Name>,
}

/// Owned payload bits starting at bit 0; Debug shows only the length.
#[derive(Clone, PartialEq, Eq)]
pub struct Payload {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Payload")
            .field("bit_len", &self.bit_len)
            .finish_non_exhaustive()
    }
}

impl Payload {
    /// Payload from a writer; its trailing pad bits are zero.
    pub fn from_writer(writer: BitWriter) -> Self {
        Self {
            bit_len: writer.bit_len,
            bytes: writer.bytes,
        }
    }

    pub fn bit_len(&self) -> usize {
        self.bit_len
    }

    /// Bits from bit 0; unused high bits of the last byte are zero.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn reader(&self) -> BitReader<'_> {
        BitReader {
            bytes: &self.bytes,
            bit_len: self.bit_len,
            position: 0,
            error: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bunch {
    pub header: BunchHeader,
    pub payload: Payload,
}

fn read_name(reader: &mut BitReader<'_>, limits: &BunchLimits) -> Result<Name> {
    if reader.read_bit() {
        let index = reader.read_int_packed();
        ensure!(!reader.is_error(), "truncated hard-coded name");
        ensure!(
            index <= MAX_HARDCODED_NAME_INDEX,
            "hard-coded name index exceeds 702"
        );
        return Ok(Name::Hardcoded(index));
    }
    let count = reader.read_u32() as i32;
    ensure!(!reader.is_error(), "truncated archive string count");
    ensure!(
        count != i32::MIN,
        "archive string count i32::MIN unsupported"
    );
    let units = count.unsigned_abs() as usize;
    ensure!(
        units <= limits.max_string_units,
        "archive string exceeds the name unit limit"
    );
    let payload_bytes = units * if count < 0 { 2 } else { 1 };
    // Checked before allocation: overflow returns an empty vector.
    let payload = reader.read_bytes(payload_bytes);
    ensure!(!reader.is_error(), "truncated archive string payload");
    let frame = [count.to_le_bytes().as_slice(), &payload].concat();
    let consumed = read_string_frame(&frame, ByteOrder::Little)?.consumed();
    ensure!(
        consumed == frame.len(),
        "archive string frame length mismatch"
    );
    let number = reader.read_u32();
    ensure!(!reader.is_error(), "truncated name number");
    Ok(Name::String { frame, number })
}

fn write_name(writer: &mut BitWriter, name: &Name, limits: &BunchLimits) -> Result<()> {
    match name {
        Name::Hardcoded(index) => {
            ensure!(
                *index <= MAX_HARDCODED_NAME_INDEX,
                "hard-coded name index exceeds 702"
            );
            writer.write_bit(true);
            writer.write_int_packed(*index);
        }
        Name::String { frame, number } => {
            let parsed = read_string_frame(frame, ByteOrder::Little)?;
            ensure!(
                parsed.consumed() == frame.len(),
                "archive string frame length mismatch"
            );
            ensure!(
                parsed.declared_units() <= limits.max_string_units,
                "archive string exceeds the name unit limit"
            );
            writer.write_bit(false);
            writer.write_bytes(frame);
            writer.write_u32(*number);
        }
    }
    Ok(())
}

/// Read one bunch header and its payload (F-522), current-version path.
pub fn read_bunch(reader: &mut BitReader<'_>, limits: &BunchLimits) -> Result<Bunch> {
    let payload_limit = limits.payload_bit_limit()?;
    let control = reader.read_bit();
    let (open, close) = if control {
        (reader.read_bit(), reader.read_bit())
    } else {
        (false, false)
    };
    let close_reason = close.then(|| reader.read_int_max(CLOSE_REASON_LIMIT));
    let paused = reader.read_bit();
    let reliable = reader.read_bit();
    let channel_index = reader.read_int_packed();
    ensure!(!reader.is_error(), "truncated bunch header");
    ensure!(
        channel_index < limits.max_channels,
        "bunch channel index exceeds the channel limit"
    );
    let has_package_map_exports = reader.read_bit();
    let has_must_be_mapped_guids = reader.read_bit();
    let partial = reader.read_bit();
    let reliable_sequence_raw = reliable.then(|| reader.read_int_max(RELIABLE_SEQUENCE_LIMIT));
    let (partial_initial, partial_final) = if partial {
        (reader.read_bit(), reader.read_bit())
    } else {
        (false, false)
    };
    ensure!(!reader.is_error(), "truncated bunch header");
    let channel_name = if open || reliable {
        Some(read_name(reader, limits)?)
    } else {
        None
    };
    let payload_bits = reader.read_int_max(payload_limit) as usize;
    ensure!(!reader.is_error(), "truncated bunch payload size");
    ensure!(
        payload_bits <= reader.remaining(),
        "truncated bunch payload"
    );
    let mut payload = BitWriter::new();
    payload.write_reader_bits(reader, payload_bits);
    Ok(Bunch {
        header: BunchHeader {
            control,
            open,
            close,
            close_reason,
            paused,
            reliable,
            channel_index,
            has_package_map_exports,
            has_must_be_mapped_guids,
            partial,
            reliable_sequence_raw,
            partial_initial,
            partial_final,
            channel_name,
        },
        payload: Payload::from_writer(payload),
    })
}

/// Read bunches until the reader is exactly at its end.
pub fn read_bunches(reader: &mut BitReader<'_>, limits: &BunchLimits) -> Result<Vec<Bunch>> {
    let mut bunches = Vec::new();
    while !reader.at_end() {
        let index = bunches.len();
        bunches.push(read_bunch(reader, limits).with_context(|| format!("bunch {index}"))?);
    }
    Ok(bunches)
}

/// Mirror of `read_bunch`, validating the header's conditional fields.
pub fn write_bunch(writer: &mut BitWriter, bunch: &Bunch, limits: &BunchLimits) -> Result<()> {
    let header = &bunch.header;
    let payload_limit = limits.payload_bit_limit()?;
    ensure!(
        header.control == (header.open || header.close),
        "control bit must equal open || close"
    );
    match header.close_reason {
        Some(reason) => ensure!(
            header.close && reason < CLOSE_REASON_LIMIT,
            "close reason requires close and must be below 15"
        ),
        None => ensure!(!header.close, "close requires a close reason"),
    }
    ensure!(
        header.channel_index < limits.max_channels,
        "bunch channel index exceeds the channel limit"
    );
    match header.reliable_sequence_raw {
        Some(sequence) => ensure!(
            header.reliable && sequence < RELIABLE_SEQUENCE_LIMIT,
            "reliable sequence requires reliable and must be below 1024"
        ),
        None => ensure!(!header.reliable, "reliable requires a sequence"),
    }
    ensure!(
        header.partial || !(header.partial_initial || header.partial_final),
        "partial flags require partial"
    );
    ensure!(
        header.channel_name.is_some() == (header.open || header.reliable),
        "channel name present exactly when open or reliable"
    );
    ensure!(
        (bunch.payload.bit_len as u64) < u64::from(payload_limit),
        "payload exceeds the packet bit limit"
    );
    writer.write_bit(header.control);
    if header.control {
        writer.write_bit(header.open);
        writer.write_bit(header.close);
    }
    if let Some(reason) = header.close_reason {
        writer.write_int_max(reason, CLOSE_REASON_LIMIT)?;
    }
    writer.write_bit(header.paused);
    writer.write_bit(header.reliable);
    writer.write_int_packed(header.channel_index);
    writer.write_bit(header.has_package_map_exports);
    writer.write_bit(header.has_must_be_mapped_guids);
    writer.write_bit(header.partial);
    if let Some(sequence) = header.reliable_sequence_raw {
        writer.write_int_wrapped(sequence, RELIABLE_SEQUENCE_LIMIT);
    }
    if header.partial {
        writer.write_bit(header.partial_initial);
        writer.write_bit(header.partial_final);
    }
    if let Some(name) = &header.channel_name {
        write_name(writer, name, limits)?;
    }
    writer.write_int_wrapped(bunch.payload.bit_len as u32, payload_limit);
    let mut payload = bunch.payload.reader();
    writer.write_reader_bits(&mut payload, bunch.payload.bit_len);
    Ok(())
}

/// Caller-supplied framing parameters for a whole data datagram.
#[derive(Clone, Copy, Debug)]
pub struct PacketConfig {
    /// Optional magic-header width, 0..=32; the original value is unknown.
    pub magic_bits: u32,
    pub receiver: Receiver,
    pub limits: BunchLimits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Unchecked magic-header value (0 when `magic_bits` is 0).
    pub magic: u32,
    pub header: PacketHeader,
    pub info: Option<PacketInfo>,
    pub bunches: Vec<Bunch>,
}

/// Termination, data prefix, packet header and bunches to the exact end.
pub fn read_packet(datagram: &[u8], config: &PacketConfig) -> Result<Packet> {
    let (bytes, bit_len) = split_termination(datagram)?;
    let mut reader = BitReader::new(bytes, bit_len)?;
    let magic = read_data_prefix(&mut reader, config.magic_bits)?;
    let (header, info) = read_packet_header(&mut reader, config.receiver)?;
    let bunches = read_bunches(&mut reader, &config.limits)?;
    Ok(Packet {
        magic,
        header,
        info,
        bunches,
    })
}

pub fn write_packet(packet: &Packet, config: &PacketConfig) -> Result<Vec<u8>> {
    let mut writer = BitWriter::new();
    write_data_prefix(&mut writer, config.magic_bits, packet.magic)?;
    write_packet_header(
        &mut writer,
        &packet.header,
        packet.info.as_ref(),
        config.receiver,
    )?;
    for bunch in &packet.bunches {
        write_bunch(&mut writer, bunch, &config.limits)?;
    }
    Ok(finish_with_termination(writer))
}
