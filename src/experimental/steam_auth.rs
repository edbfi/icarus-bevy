//! Experimental strict codec for explicitly supplied, byte-aligned Steam-auth
//! component message bodies, independently authored from qualified field facts.
//!
//! The leading type byte is included. Datagram helpers wrap the body with
//! caller-selected magic bits, a clear stateless bit, a set auth bit and one
//! handler termination bit. There is no connection termination or packet
//! sequence header. Transport/session state and the authentication service are
//! outside this module. Successful decoding grants no admission and demonstrates
//! neither stock-client authentication nor compatibility.
//!
//! Tickets use the existing strict archive-string subset, preserving supported
//! raw UTF-16 units (including unpaired surrogates). The archive byte order applies
//! to the string and Steam ID; the four-byte Boolean storage is always Windows
//! raw-serialization little-endian. No Steam identity or ticket hexadecimal
//! semantics are validated. Debug output and errors omit ticket and ID values;
//! callers can intentionally access the payload fields.

use std::fmt;

use anyhow::{Result, bail, ensure};

use super::archive::{ByteOrder, encode_string, read_string_frame};
use super::packet::{BitReader, BitWriter, finish_with_termination, split_termination};

/// Authored resource bound on content UTF-16 units, excluding any terminator.
/// This is not a universal maximum of the original wire format.
pub const MAX_TICKET_UNITS: usize = 1_024;

/// Authored encoded-body bound: type + count + maximum wide ticket + Steam ID.
pub const MAX_MESSAGE_BYTES: usize = 1 + 4 + 2 * (MAX_TICKET_UNITS + 1) + 8;

/// Maximum datagram with a 32-bit magic header, two handler bits and termination.
/// An authored resource bound, not a negotiated network packet size.
pub const MAX_DATAGRAM_BYTES: usize = (32 + 2 + MAX_MESSAGE_BYTES * 8 + 1).div_ceil(8);

fn validate_magic(magic_bits: u32, magic: u32) -> Result<()> {
    ensure!(magic_bits <= 32, "Steam-auth magic header exceeds 32 bits");
    ensure!(
        magic_bits == 32 || magic >> magic_bits == 0,
        "Steam-auth magic value exceeds its bit width"
    );
    Ok(())
}

/// Encode one handler datagram without touching packet or reliable sequences.
/// `magic_bits = 0, magic = 0` selects the observed no-magic framing. Other
/// widths are explicit caller-selected framing, not an observed configuration.
pub fn encode_datagram(
    message: &SteamAuthMessage,
    order: ByteOrder,
    magic_bits: u32,
    magic: u32,
) -> Result<Vec<u8>> {
    validate_magic(magic_bits, magic)?;
    let body = encode_message(message, order)?;
    let mut writer = BitWriter::new();
    writer.write_bits(magic, magic_bits);
    writer.write_bit(false);
    writer.write_bit(true);
    for byte in body {
        writer.write_bits(u32::from(byte), 8);
    }
    Ok(finish_with_termination(writer))
}

/// Decode exactly one handler datagram, checking the selected magic and both
/// handler bits before allocating a bounded body. The highest final set bit is
/// the one handler termination; the remaining body must contain whole bytes.
/// Extra body bytes and a second termination bit are rejected. No sequence or
/// authentication state is consumed, and diagnostics omit input values.
pub fn decode_datagram(
    input: &[u8],
    order: ByteOrder,
    magic_bits: u32,
    magic: u32,
) -> Result<SteamAuthMessage> {
    validate_magic(magic_bits, magic)?;
    let maximum = (magic_bits as usize + 2 + MAX_MESSAGE_BYTES * 8 + 1).div_ceil(8);
    ensure!(
        input.len() <= maximum,
        "Steam-auth datagram exceeds authored byte limit"
    );
    let (bytes, bits) = split_termination(input)
        .map_err(|_| anyhow::anyhow!("invalid Steam-auth datagram termination"))?;
    let mut reader = BitReader::new(bytes, bits)?;
    let actual_magic = reader.read_bits(magic_bits);
    let stateless = reader.read_bit();
    let auth = reader.read_bit();
    ensure!(!reader.is_error(), "truncated Steam-auth datagram prefix");
    ensure!(actual_magic == magic, "unexpected Steam-auth magic header");
    ensure!(!stateless && auth, "invalid Steam-auth handler prefix");
    ensure!(
        reader.remaining().is_multiple_of(8),
        "Steam-auth body does not contain whole bytes"
    );
    let body_bytes = reader.remaining() / 8;
    ensure!(
        body_bytes <= MAX_MESSAGE_BYTES,
        "Steam-auth body exceeds authored byte limit"
    );
    let body = reader.read_bytes(body_bytes);
    decode_message(&body, order)
}

/// One field body, including its type byte, with no authentication authority.
#[derive(Clone, PartialEq, Eq)]
pub enum SteamAuthMessage {
    /// Type 1: strict FString followed by an archive-ordered u64.
    UserData {
        /// Raw UTF-16 content units, excluding the terminator.
        ticket: Vec<u16>,
        steam_id: u64,
    },
    /// Type 2: a canonical four-byte Boolean. This value grants no admission.
    Result { accepted: bool },
    /// Type 3, without following fields.
    RequestResendKey,
    /// Type 4, without following fields.
    RequestResendResult,
}

impl fmt::Debug for SteamAuthMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserData { .. } => formatter
                .debug_struct("UserData")
                .field("ticket", &"[REDACTED]")
                .field("steam_id", &"[REDACTED]")
                .finish(),
            Self::Result { accepted } => formatter
                .debug_struct("Result")
                .field("accepted", accepted)
                .finish(),
            Self::RequestResendKey => formatter.write_str("RequestResendKey"),
            Self::RequestResendResult => formatter.write_str("RequestResendResult"),
        }
    }
}

/// Encode a complete body after validating its authored resource bound and the
/// archive's strict string subset. Empty tickets use the archive's zero count;
/// ASCII is preferred over wide encoding. Errors never include payload values.
pub fn encode_message(message: &SteamAuthMessage, order: ByteOrder) -> Result<Vec<u8>> {
    match message {
        SteamAuthMessage::UserData { ticket, steam_id } => {
            ensure!(
                ticket.len() <= MAX_TICKET_UNITS,
                "Steam-auth ticket exceeds authored unit limit"
            );
            // The archive validates all units before allocating its output.
            let ticket = encode_string(ticket, order)
                .map_err(|_| anyhow::anyhow!("invalid strict Steam-auth ticket"))?;
            let encoded_len = 1 + ticket.len() + 8;
            ensure!(
                encoded_len <= MAX_MESSAGE_BYTES,
                "Steam-auth body exceeds authored byte limit"
            );
            let mut output = Vec::with_capacity(encoded_len);
            output.push(1);
            output.extend_from_slice(&ticket);
            output.extend_from_slice(&match order {
                ByteOrder::Little => steam_id.to_le_bytes(),
                ByteOrder::Big => steam_id.to_be_bytes(),
            });
            Ok(output)
        }
        SteamAuthMessage::Result { accepted } => {
            let mut output = Vec::with_capacity(5);
            output.push(2);
            output.extend_from_slice(&u32::from(*accepted).to_le_bytes());
            Ok(output)
        }
        SteamAuthMessage::RequestResendKey => Ok(vec![3]),
        SteamAuthMessage::RequestResendResult => Ok(vec![4]),
    }
}

/// Decode exactly one complete body, rejecting unknown types, trailing bytes,
/// noncanonical Booleans and invalid strict archive strings. Input length and
/// declared string magnitude are bounded before any ticket allocation. Errors
/// never include payload values. Decoding does not validate an identity or ticket.
pub fn decode_message(input: &[u8], order: ByteOrder) -> Result<SteamAuthMessage> {
    ensure!(
        input.len() <= MAX_MESSAGE_BYTES,
        "Steam-auth body exceeds authored byte limit"
    );
    let (&kind, body) = input
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("truncated Steam-auth type"))?;
    match kind {
        1 => {
            let prefix: [u8; 4] = body
                .get(..4)
                .ok_or_else(|| anyhow::anyhow!("truncated Steam-auth ticket count"))?
                .try_into()?;
            let count = match order {
                ByteOrder::Little => i32::from_le_bytes(prefix),
                ByteOrder::Big => i32::from_be_bytes(prefix),
            };
            // unsigned_abs handles INT_MIN without negation overflow. The bound
            // includes a terminator for nonzero counts; zero means no payload.
            ensure!(
                count.unsigned_abs() <= (MAX_TICKET_UNITS + 1) as u32,
                "Steam-auth ticket exceeds authored unit limit"
            );
            let frame = read_string_frame(body, order)
                .map_err(|_| anyhow::anyhow!("invalid strict Steam-auth ticket"))?;
            // The validated magnitude bounds consumed() and this addition.
            let end = frame.consumed() + 8;
            let id_bytes: [u8; 8] = body
                .get(frame.consumed()..end)
                .ok_or_else(|| anyhow::anyhow!("truncated Steam-auth Steam ID"))?
                .try_into()?;
            ensure!(body.len() == end, "trailing Steam-auth body bytes");
            let steam_id = match order {
                ByteOrder::Little => u64::from_le_bytes(id_bytes),
                ByteOrder::Big => u64::from_be_bytes(id_bytes),
            };
            // Materialize only after validating the entire body, including EOF.
            Ok(SteamAuthMessage::UserData {
                ticket: frame.units().collect(),
                steam_id,
            })
        }
        2 => {
            let storage: [u8; 4] = body
                .get(..4)
                .ok_or_else(|| anyhow::anyhow!("truncated Steam-auth Boolean"))?
                .try_into()?;
            let accepted = match u32::from_le_bytes(storage) {
                0 => false,
                1 => true,
                _ => bail!("noncanonical Steam-auth Boolean"),
            };
            ensure!(body.len() == 4, "trailing Steam-auth body bytes");
            Ok(SteamAuthMessage::Result { accepted })
        }
        3 | 4 => {
            ensure!(body.is_empty(), "trailing Steam-auth body bytes");
            Ok(if kind == 3 {
                SteamAuthMessage::RequestResendKey
            } else {
                SteamAuthMessage::RequestResendResult
            })
        }
        _ => bail!("unknown Steam-auth message type"),
    }
}
