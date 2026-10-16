//! EXPERIMENTAL pin of the official release this server answers to.
//!
//! ICARUS ships weekly, and each release changes the project version string
//! that the client hashes into its Hello network version (F-541, F-685); the
//! original server compares that value for exact equality and answers a
//! mismatch with Upgrade (F-563). Server code changes between weekly builds
//! too (F-686), so the pin names one build, `config/release.toml` holds it,
//! and `tools/release_watch.py` moves it when Steam's public branch moves.
//! Nothing here claims compatibility with the pinned build.

use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use super::default_version_string_checksum;

/// Image project name the version string starts with (F-541).
pub const PROJECT_NAME: &str = "Icarus";
/// Network changelist without an override (F-542).
pub const NETWORK_CHANGELIST: u32 = 0;
/// Engine network protocol version (F-541).
pub const ENGINE_NETWORK_VERSION: u32 = 17;
/// Game network protocol version, zero-filled (F-541).
pub const GAME_NETWORK_VERSION: u32 = 0;

/// The string the engine hashes into the Hello network version.
pub fn network_version_string(project_version: &str) -> String {
    format!(
        "{PROJECT_NAME} {project_version}, NetCL: {NETWORK_CHANGELIST}, \
         EngineNetVer: {ENGINE_NETWORK_VERSION}, GameNetVer: {GAME_NETWORK_VERSION}"
    )
}

/// The Hello network version a client of `project_version` sends when no
/// override delegate is bound (F-541).
pub fn network_checksum(project_version: &str) -> u32 {
    default_version_string_checksum(&network_version_string(project_version))
}

/// One official release, as `config/release.toml` pins it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePin {
    /// Steam app of the dedicated server.
    pub app_id: u32,
    /// Steam build of the public branch.
    pub build_id: u64,
    /// The project version the server logs and the client hashes.
    pub project_version: String,
    /// `network_checksum(project_version)`, stored so a reviewer sees the
    /// value a client sends.
    pub network_checksum: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseFile {
    release: ReleasePin,
}

impl ReleasePin {
    /// Parse a pin file and check its stored checksum.
    pub fn parse(text: &str) -> Result<Self> {
        let pin = toml::from_str::<ReleaseFile>(text)
            .context("release pin")?
            .release;
        ensure!(
            pin.project_version.is_ascii() && !pin.project_version.is_empty(),
            "project_version must be nonempty ASCII"
        );
        let expected = network_checksum(&pin.project_version);
        ensure!(
            pin.network_checksum == expected,
            "network_checksum {} does not match {expected} for {}",
            pin.network_checksum,
            pin.project_version
        );
        Ok(pin)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
    }
}
