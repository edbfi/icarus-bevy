//! Native, headless Bevy foundation. ICARUS interoperability remains unverified.
pub mod config;
pub mod data_pak;
pub mod experimental;
pub mod import_candidate;
pub mod import_plan;
mod json;
pub mod metrics;
pub mod native_c001_inventory_owners;
pub mod native_c001_owners;
pub mod native_c001_scalar_owners;
pub mod native_import;
pub mod native_inventory;
pub mod native_inventory_restore;
pub mod native_lifecycle;
pub mod native_lifecycle_runtime;
pub mod native_modifiers;
pub mod native_relationships;
pub mod native_replication;
pub mod native_runtime;
pub mod native_scalar_owners;

// Reuse the complete authored source fixture in storage crash-model unit tests.
#[cfg(test)]
extern crate self as icarus_bevy;
pub mod native_world;
pub mod network;
pub mod pawn_stats;
pub mod persistence;
pub mod preservation;
pub mod prospect;
pub mod replication;
#[cfg(test)]
#[path = "../tests/common/c001_scalar_owners.rs"]
mod scalar_owner_fixture;
#[cfg(test)]
pub(crate) use scalar_owner_fixture as scalar;
#[cfg(test)]
#[path = "../tests/common/c001_inventory_owners.rs"]
pub(crate) mod inventory_owner_fixture;
pub mod session_replication;
pub mod simulation;
#[cfg(test)]
#[path = "../tests/common/c001_inventory_source.rs"]
pub(crate) mod source;
pub mod steam_fetch;

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
