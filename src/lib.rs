//! Native, headless Bevy foundation. ICARUS interoperability remains unverified.
pub mod config;
pub mod experimental;
pub mod persistence;
pub mod prospect;
pub mod simulation;

/// Stable lowercase, zero-padded encoding for persisted and legacy checksums.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
