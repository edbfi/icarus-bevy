//! Strict fields for an explicitly caller-selected initial control archive body.
//!
//! The caller must establish ID0, body boundaries and archive byte order. This
//! module does not decode UDP/bunch framing, interpret the first byte or token,
//! select a local version, validate identity or admit a game connection. Its
//! exact-EOF policy and string restrictions are independent research choices.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, MAX_STRING_UNITS, decode_string};

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

    /// Uninterpreted byte; it does not select archive byte order or a policy.
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
    let decoded = decode_string(token, order).context("invalid initial control token frame")?;
    ensure!(
        decoded.consumed == token.len(),
        "trailing data after initial control token"
    );
    // Successful decode checked the four-byte count and entire declared frame.
    let count: [u8; 4] = token[..4].try_into()?;
    let count = match order {
        ByteOrder::Little => i32::from_le_bytes(count),
        ByteOrder::Big => i32::from_be_bytes(count),
    };
    let token_encoding = match count {
        0 => TokenEncoding::Empty,
        1.. => TokenEncoding::Narrow,
        _ => TokenEncoding::Wide,
    };
    let token_units = decoded.units.len();
    // Retain only the original frame and aggregate structure, never decoded units.
    drop(decoded);
    Ok(InitialControlBody {
        body: input,
        remote_version,
        token_encoding,
        token_units,
    })
}
