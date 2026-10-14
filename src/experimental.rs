//! Independently authored compatibility primitives supported by static evidence.
//!
//! The separate loopback lab exercises short handshake framing only. The normal
//! server does not enable it. None of these primitives implements game admission.

pub mod archive;
pub mod component;
pub mod control;
pub mod handshake;
pub mod handshake_lab;
pub mod int_array;
pub mod packet;
pub mod recorders;
pub mod struct_array;
pub mod tagged;

/// Reflected CRC-32 of zero-extended UTF-16 units in little-endian order.
///
/// Each unit contributes four bytes, not two. Processing stops before the first
/// zero unit. `seed` is a finalized prior CRC, so an empty input returns it.
/// Arbitrary UTF-16 units are accepted without Unicode normalization.
pub fn checksum_utf16(units: &[u16], seed: u32) -> u32 {
    checksum_units(units.iter().copied(), seed)
}

/// Apply ASCII-only lowercasing before the checksum used in default construction.
///
/// This does not select the actual version string, engine/game version numbers,
/// cache state or optional callbacks. It cannot determine client compatibility.
pub fn default_version_string_checksum(text: &str) -> u32 {
    checksum_units(
        text.encode_utf16().map(|unit| match unit {
            0x41..=0x5a => unit + 0x20,
            _ => unit,
        }),
        0,
    )
}

fn checksum_units(units: impl IntoIterator<Item = u16>, seed: u32) -> u32 {
    let mut crc = !seed;
    for unit in units.into_iter().take_while(|unit| *unit != 0) {
        for byte in u32::from(unit).to_le_bytes() {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
            }
        }
    }
    !crc
}
