//! Strict fields for an initial control archive body.
//!
//! The caller must establish ID0 and body boundaries. `read_initial_control_body`
//! takes a caller-selected archive byte order and leaves the first byte
//! uninterpreted. The separate experimental
//! `read_initial_control_body_with_sender_order` derives the order from the
//! first byte with a rule inferred statically from the client and server
//! receive paths (F-519); no live exchange has validated it. Neither decodes
//! UDP packet-to-bunch framing, interprets the token, selects a local version,
//! validates identity or admits a game connection. The exact-EOF policy and
//! string restrictions are independent research choices.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, MAX_STRING_UNITS, StringEncoding, read_string_frame};

const PREFIX_BYTES: usize = 5;
/// Research ceiling derived from the existing string-unit cap, including its
/// terminating unit. This is not a recovered original network/token size limit.
pub const MAX_INITIAL_CONTROL_BODY_BYTES: usize = PREFIX_BYTES + 4 + 2 * MAX_STRING_UNITS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEncoding {
    /// Zero signed count; no token payload bytes.
    Empty,
    Narrow,
    Wide,
}

/// Validated immutable source spans; deliberately neither Debug nor Serialize.
pub struct InitialControlBody<'a> {
    body: &'a [u8],
    remote_version: u32,
    token_encoding: TokenEncoding,
    token_units: usize,
}

#[derive(Debug, Serialize)]
pub struct InitialControlBodySummary {
    pub body_bytes: usize,
    pub prefix_bytes: usize,
    /// Complete signed-count string frame, including any terminator.
    pub token_bytes: usize,
    /// UTF-16 code units excluding the terminator; no decoded units are emitted.
    pub token_units: usize,
    pub token_encoding: TokenEncoding,
    pub negotiation_validated: bool,
    pub identity_verified: bool,
    pub game_connection_established: bool,
}

impl<'a> InitialControlBody<'a> {
    pub fn body(&self) -> &'a [u8] {
        self.body
    }

    /// Raw byte. Only `read_initial_control_body_with_sender_order` derives an
    /// archive byte order from it (inferred, F-519); it selects no other policy.
    pub fn first_field(&self) -> u8 {
        self.body[0]
    }

    pub fn first_field_bytes(&self) -> &'a [u8] {
        &self.body[..1]
    }

    /// Decoded archive integer, without a compatibility/acceptance decision.
    pub fn remote_version(&self) -> u32 {
        self.remote_version
    }

    pub fn remote_version_bytes(&self) -> &'a [u8] {
        &self.body[1..PREFIX_BYTES]
    }

    pub fn prefix(&self) -> &'a [u8] {
        &self.body[..PREFIX_BYTES]
    }

    /// Original token frame; no text conversion, normalization or digest occurs.
    pub fn token_frame(&self) -> &'a [u8] {
        &self.body[PREFIX_BYTES..]
    }

    pub fn summary(&self) -> InitialControlBodySummary {
        InitialControlBodySummary {
            body_bytes: self.body.len(),
            prefix_bytes: PREFIX_BYTES,
            token_bytes: self.token_frame().len(),
            token_units: self.token_units,
            token_encoding: self.token_encoding,
            negotiation_validated: false,
            identity_verified: false,
            game_connection_established: false,
        }
    }
}

/// Frame a byte, an explicit-endian u32 and one strict archive string at EOF.
///
/// Selecting this body layout is the caller's responsibility. The first byte
/// remains uninterpreted; zero/nonempty tokens and any remote version receive
/// identical structural validation. Original normalization cases unsupported by
/// `archive::decode_string` stay unsupported. No connection state is modified.
pub fn read_initial_control_body(input: &[u8], order: ByteOrder) -> Result<InitialControlBody<'_>> {
    ensure!(
        input.len() <= MAX_INITIAL_CONTROL_BODY_BYTES,
        "initial control body exceeds research byte limit"
    );
    input
        .first()
        .context("truncated initial control first field")?;
    let version: [u8; 4] = input
        .get(1..PREFIX_BYTES)
        .context("truncated initial control remote version")?
        .try_into()?;
    let remote_version = match order {
        ByteOrder::Little => u32::from_le_bytes(version),
        ByteOrder::Big => u32::from_be_bytes(version),
    };
    let token = &input[PREFIX_BYTES..];
    let frame = read_string_frame(token, order).context("invalid initial control token frame")?;
    ensure!(
        frame.consumed() == token.len(),
        "trailing data after initial control token"
    );
    let token_encoding = match frame.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    let token_units = frame.unit_count();
    Ok(InitialControlBody {
        body: input,
        remote_version,
        token_encoding,
        token_units,
    })
}

/// Experimental archive byte order implied by the ID0 first field (F-519).
///
/// Inferred statically from the client and server receive paths on their
/// little-endian hosts: exactly 1 means a little-endian sender (no swap); every
/// other value means the opposite endianness, so big-endian here. No live
/// exchange has validated it; independent of this process's host byte order.
pub fn sender_byte_order(first_field: u8) -> ByteOrder {
    if first_field == 1 {
        ByteOrder::Little
    } else {
        ByteOrder::Big
    }
}

/// Experimental: `read_initial_control_body` with the order from `sender_byte_order`.
///
/// The rule is inferred statically (F-519), not validated by a live exchange, and
/// packet-to-bunch framing is still not decoded. Ceiling, truncation, token and
/// exact-EOF validation are those of the explicit-order reader, unchanged. The
/// summary still reports no negotiation, identity or game connection.
pub fn read_initial_control_body_with_sender_order(input: &[u8]) -> Result<InitialControlBody<'_>> {
    // Empty input fails on the missing first field before any order is used.
    let order = input
        .first()
        .map_or(ByteOrder::Little, |&first| sender_byte_order(first));
    read_initial_control_body(input, order)
}
