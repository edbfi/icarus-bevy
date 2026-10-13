//! Candidate 29-byte stateless handshake framing for the inspected build.
//!
//! No authentication policy, cookie generation, connection state or gameplay
//! transport is implemented here. Optional prefix and extended restart framing
//! remain unsupported. See `docs/specs/network/stateless-handshake.md`.
use anyhow::{Result, ensure};

#[derive(Debug, PartialEq)]
pub struct LegacyHandshake {
    pub restart: bool,
    pub secret_id: bool,
    pub timestamp: f64,
    pub cookie: [u8; 20],
}

/// Construct the zero-timestamp initial request with no optional prefix.
pub fn initial_request() -> [u8; 29] {
    let mut bytes = [0; 29];
    bytes[0] = 1; // Handshake marker, followed by zero restart/secret bits.
    bytes[28] = 8; // Termination at bit 227, with zero byte padding.
    bytes
}

/// Decode only the exact short framing, with strict termination/padding policy.
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

/// Encode the short form only. This does not authorize any connection or cookie.
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

fn write_unaligned(bytes: &mut [u8; 29], start_bit: usize, input: &[u8]) {
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
