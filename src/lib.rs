//! Native, headless Bevy foundation. ICARUS interoperability remains unverified.
pub mod config;
pub mod experimental;
pub mod import_plan;
mod json;
pub mod metrics;
pub mod network;
pub mod persistence;
pub mod preservation;
pub mod prospect;
pub mod simulation;

/// Open without blocking so a FIFO or device cannot stall a read; callers then
/// check the opened handle's file type.
pub fn nonblocking_flag() -> i32 {
    rustix::fs::OFlags::NONBLOCK.bits() as i32
}

/// Keep input strings and unknown keys out of JSON diagnostics and error causes.
fn json_parse_error(context: &'static str, error: serde_json::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "{context}: {:?} at line {}, column {}",
        error.classify(),
        error.line(),
        error.column()
    )
}

/// Stable lowercase, zero-padded encoding for persisted and legacy checksums.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
