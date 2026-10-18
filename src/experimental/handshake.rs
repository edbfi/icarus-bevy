//! Stateless handshake framing for the inspected builds (`25621760` / `25793767`).
//!
//! Covers the three bit-lengths accepted by `ParseHandshakePacket` (`0x1437419a0`
//! server, `0x1439dec65` client) after the 1-bit `bHandshakePacket` marker and
//! trailing termination-bit strip:
//! - 1 remaining bit (3 bits total, `0x07`): server-to-client restart-handshake request
//! - 226 remaining bits (228 bits total, 29 bytes): standard handshake packet
//! - 386 remaining bits (388 bits total, 49 bytes): extended restart response carrying `OrigCookie`
//!
//! Also models the `net.StatelessConnectComponent.MagicHeader` CVar bit-string
//! parser (`0x14372d5d0`), which is unconfigured (`0` bits) in stock ICARUS.
//! See `docs/specs/network/stateless-handshake.md`.
use anyhow::{Result, bail, ensure};

/// Server-to-client 1-byte (`3`-bit: `bHandshakePacket=1`, `bRestartHandshake=1`,
/// termination bit `1`) restart-handshake request sent by `SendRestartHandshakeRequest`
/// (`0x143745ee0`) when `IncomingConnectionless` (`0x14373f0f0`) receives a packet
/// with `bHandshakePacket == 0` on an un-handshaked address.
pub const RESTART_REQUEST_FRAME: [u8; 1] = [0x07];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyHandshake {
    pub restart: bool,
    pub secret_id: bool,
    pub timestamp: f64,
    pub cookie: [u8; 20],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestartChallengeResponse {
    pub secret_id: bool,
    pub timestamp: f64,
    pub cookie: [u8; 20],
    pub orig_cookie: [u8; 20],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HandshakeFrame {
    /// 1-byte (`0x07`, 1 remaining bit after marker) server-to-client restart request.
    RestartRequest,
    /// 29-byte (226 remaining bits after marker) standard handshake packet.
    Standard(LegacyHandshake),
    /// 49-byte (386 remaining bits after marker) extended restart response with `OrigCookie`.
    RestartResponse(RestartChallengeResponse),
}

/// Parse the `net.StatelessConnectComponent.MagicHeader` CVar string using the
/// constructor rule in `0x14372d5d0`: accepts `1..=32` ASCII `'0'`/`'1'` characters;
/// empty strings, strings longer than 32 characters, or strings containing any
/// other character reset the prefix length (`this + 0xe8`) to `0` bits.
pub fn parse_magic_header_bits(raw: &str) -> Vec<bool> {
    let chars: Vec<char> = raw.chars().collect();
    if chars.is_empty() || chars.len() > 32 {
        return Vec::new();
    }
    let mut bits = Vec::with_capacity(chars.len());
    for ch in chars {
        match ch {
            '0' => bits.push(false),
            '1' => bits.push(true),
            _ => return Vec::new(),
        }
    }
    bits
}

/// Construct the zero-timestamp initial request with no optional prefix.
pub fn initial_request() -> [u8; 29] {
    let mut bytes = [0; 29];
    bytes[0] = 1; // Handshake marker, followed by zero restart/secret bits.
    bytes[28] = 8; // Termination at bit 227, with zero byte padding.
    bytes
}

/// Decode only the exact 29-byte short framing, with strict termination/padding policy.
pub fn decode(bytes: &[u8]) -> Result<LegacyHandshake> {
    ensure!(bytes.len() == 29, "unsupported handshake datagram length");
    ensure!(bytes[0] & 1 == 1, "missing handshake marker");
    ensure!(
        bytes[28] & 0xf8 == 8,
        "invalid handshake termination or padding"
    );
    let timestamp = f64::from_le_bytes(read_unaligned::<8>(bytes, 3));
    ensure!(timestamp.is_finite(), "nonfinite handshake timestamp");
    Ok(LegacyHandshake {
        restart: bytes[0] & 2 != 0,
        secret_id: bytes[0] & 4 != 0,
        timestamp,
        cookie: read_unaligned::<20>(bytes, 67),
    })
}

/// Encode the 29-byte short form only. This does not authorize any connection or cookie.
pub fn encode(packet: &LegacyHandshake) -> Result<[u8; 29]> {
    ensure!(
        packet.timestamp.is_finite(),
        "nonfinite handshake timestamp"
    );
    let mut bytes = initial_request();
    bytes[0] |= u8::from(packet.restart) << 1 | u8::from(packet.secret_id) << 2;
    write_unaligned(&mut bytes, 3, &packet.timestamp.to_le_bytes());
    write_unaligned(&mut bytes, 67, &packet.cookie);
    Ok(bytes)
}

/// Encode the 49-byte (`388`-bit) extended restart-handshake challenge response
/// (`SendChallengeResponse` at `0x143745810` when `bRestartedHandshake != 0`).
pub fn encode_restart_response(packet: &RestartChallengeResponse) -> Result<[u8; 49]> {
    ensure!(
        packet.timestamp.is_finite(),
        "nonfinite handshake timestamp"
    );
    let mut bytes = [0u8; 49];
    // Bit 0: bHandshakePacket = 1; Bit 1: bRestartHandshake = 1; Bit 2: SecretId.
    bytes[0] = 0b011 | (u8::from(packet.secret_id) << 2);
    write_unaligned(&mut bytes, 3, &packet.timestamp.to_le_bytes());
    write_unaligned(&mut bytes, 67, &packet.cookie);
    write_unaligned(&mut bytes, 227, &packet.orig_cookie);
    // Bit 387 (byte 48, bit 3): termination bit 1 with zero upper padding.
    bytes[48] |= 0x08;
    Ok(bytes)
}

/// Decode any of the three `ParseHandshakePacket` (`0x1437419a0`) wire forms
/// (1-byte restart request, 29-byte standard handshake, or 49-byte extended
/// restart response) with strict termination and finite-timestamp checks.
pub fn decode_frame(bytes: &[u8]) -> Result<HandshakeFrame> {
    match bytes.len() {
        1 => {
            ensure!(
                bytes[0] == RESTART_REQUEST_FRAME[0],
                "invalid 1-byte restart handshake request frame"
            );
            Ok(HandshakeFrame::RestartRequest)
        }
        29 => Ok(HandshakeFrame::Standard(decode(bytes)?)),
        49 => {
            ensure!(bytes[0] & 1 == 1, "missing handshake marker");
            ensure!(
                bytes[0] & 2 != 0,
                "extended 49-byte handshake frame requires restart bit"
            );
            ensure!(
                bytes[48] & 0xf8 == 8,
                "invalid extended handshake termination or padding"
            );
            let timestamp = f64::from_le_bytes(read_unaligned::<8>(bytes, 3));
            ensure!(timestamp.is_finite(), "nonfinite handshake timestamp");
            Ok(HandshakeFrame::RestartResponse(RestartChallengeResponse {
                secret_id: bytes[0] & 4 != 0,
                timestamp,
                cookie: read_unaligned::<20>(bytes, 67),
                orig_cookie: read_unaligned::<20>(bytes, 227),
            }))
        }
        _ => bail!("unsupported handshake datagram length: {}", bytes.len()),
    }
}

fn write_unaligned(bytes: &mut [u8], start_bit: usize, input: &[u8]) {
    for (index, byte) in input.iter().enumerate() {
        for shift in 0..8 {
            let target_bit = start_bit + index * 8 + shift;
            bytes[target_bit / 8] |= ((byte >> shift) & 1) << (target_bit % 8);
        }
    }
}

fn read_unaligned<const N: usize>(bytes: &[u8], start_bit: usize) -> [u8; N] {
    let mut output = [0; N];
    for (index, byte) in output.iter_mut().enumerate() {
        for shift in 0..8 {
            let source_bit = start_bit + index * 8 + shift;
            *byte |= ((bytes[source_bit / 8] >> (source_bit % 8)) & 1) << shift;
        }
    }
    output
}
