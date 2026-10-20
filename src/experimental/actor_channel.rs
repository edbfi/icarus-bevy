//! EXPERIMENTAL disconnected codec for statically observed actor-channel bunches
//! and pure server-side replication relevancy, priority, and tick-rate
//! evaluation (`F-745`).
//!
//! Client-build and server-build static observations (F-569..F-573, F-692,
//! F-694, F-696, F-709, F-710, F-745): NetGUID references, NetGUID export
//! prefixes (`ReceiveNetGUIDBunch` `0x1439b8bb0`, `ExportNetGUIDHeader`
//! `0x1439ab670`), must-be-mapped GUID prefixes (`UActorChannel::ReceivedBunch`
//! `0x1437c43f0`), disconnected package-map export state (`PackageMapState`),
//! the open-bunch spawn record, the `APlayerController` 1-byte `NetPlayerIndex`
//! open-bunch hook (`UActorChannel::ProcessBunch` `0x1437c0c30`,
//! `APlayerController::OnActorChannelOpen` `0x143ae73a0`,
//! `APlayerController::OnSerializeNewActor` `0x143ae79d0`), the compressed
//! rotator, the content-block headers that frame replicated state, and pure
//! server-side replication relevancy, priority, and tick-rate evaluation
//! (`UGameEngine::GetMaxTickRate` `0x1435a90e0` server / `0x143836250` client,
//! `AActor::IsWithinNetRelevancyDistance` `0x143340200` server / `0x1435d6240`
//! client, `AActor::IsNetRelevantFor` `0x14333fae0` server / `0x1435d5be0`
//! client, `APawn::IsNetRelevantFor` `0x1437b95a0` server / `0x143a681d0`
//! client, `APlayerController::IsNetRelevantFor` `0x1432510a0` server /
//! `0x1434e9040` client, `AActor::GetNetPriority` `0x14333b2e0` server /
//! `0x1435d14e0` client, `APlayerController::GetNetPriority` `0x143830f20`
//! server / `0x143adfc40` client). Nothing here is gameplay-integrated, and no
//! connection state, UObject resolution, admission or gameplay occurs. Writers
//! mirror the readers for round-trip research only.
//!
//! Caller-selected, because their original sources are unread or outside this
//! codec: the engine net version, whether the archive has exports enabled,
//! whether the level reference is present (its archive-flag gate is unread),
//! the byte order of 2- and 4-byte fields and string counts, and every limit.
//!
//! Unsupported and opaque on purpose:
//! - content-block payload bits stay opaque: the property stream and the
//!   per-property net serializers are not decoded here;
//! - [`parse_actor_bunch`] and [`parse_player_controller_open_bunch`] expect
//!   export and must-be-mapped prefixes to be stripped first via
//!   [`read_export_prefix`] and [`read_must_be_mapped_guids`], and reject
//!   partial bunches;
//! - rep-layout export bunches (`bHasRepLayoutExport == 1` in
//!   `ReceiveNetGUIDBunch` `0x1439b8bb0`);
//! - export flags with reserved bits 3-7;
//! - non-canonical packed integers (the original reader is more lenient), so
//!   every accepted structure re-encodes bit-identically.
//!
//! Interpretations, not observations: below engine net version 13 no quantized
//! bit is read and the vector is taken as quantized (INFERRED from the gate);
//! the component bit count is read as a capped integer with maximum 24, like
//! the components with maximum `1 << (bits + 2)`; "dynamic sub-object" is read
//! as a sub-object whose stably-named bit is clear; recursion depth counts
//! outer references, so a top-level reference plus 16 outers is the deepest
//! accepted chain. Angle rounding (half up) is inferred (F-573). Deletion
//! blocks (zero class reference) carry no payload count (F-576, F-709).
//!
//! `Debug` output and summaries carry kinds, counts and lengths only, never
//! GUID values, paths, checksums, vector or angle values, player indices, or
//! payload bits.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, StringEncoding, read_string_frame};
use super::control::TokenEncoding;
use super::control_messages::ArchiveText;
use super::packet::{BitReader, BitWriter, Bunch, Payload};

/// Deepest outer-reference recursion accepted below a top-level reference.
pub const MAX_GUID_DEPTH: usize = 16;
/// Maximum `NumGUIDsInBunch` accepted by `UPackageMapClient::ReceiveNetGUIDBunch`
/// (`0x1439b8bb0`, `(uint)NumGUIDsInBunch < 0x801`, F-694, F-709).
pub const MAX_EXPORT_GUIDS: u32 = 2048;
/// The default GUID, which always carries an export-flags byte (F-572).
pub const DEFAULT_GUID: u32 = 1;
/// Export flag bit 0: a recursive outer reference and a path string follow.
pub const EXPORT_HAS_PATH: u8 = 1;
/// Export flag bit 1: no-load.
pub const EXPORT_NO_LOAD: u8 = 2;
/// Export flag bit 2: a 4-byte network checksum follows the path; without
/// the path bit nothing follows (F-692).
pub const EXPORT_HAS_CHECKSUM: u8 = 4;
const EXPORT_KNOWN: u8 = EXPORT_HAS_PATH | EXPORT_NO_LOAD | EXPORT_HAS_CHECKSUM;
/// First engine net version whose spawn vectors read a quantized bit (F-571).
pub const QUANTIZED_BIT_NET_VERSION: u32 = 13;
/// Maximum of the capped component-bit-count field (F-571).
pub const VECTOR_BITS_MAX: u32 = 24;
/// Quantized spawn-vector scale (F-573).
pub const VECTOR_SCALE: f32 = 0.1;

/// Default `NetServerMaxTickRate` from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_NET_SERVER_MAX_TICK_RATE: i32 = 30;
/// Minimum clamped `NetServerMaxTickRate` in `UGameEngine::GetMaxTickRate`
/// (`0x1435a90e0` server / `0x143836250` client, F-745).
pub const MIN_NET_SERVER_MAX_TICK_RATE: i32 = 1;
/// Maximum clamped `NetServerMaxTickRate` (`0x3e8`) in
/// `UGameEngine::GetMaxTickRate` (`0x1435a90e0` server / `0x143836250` client,
/// F-745).
pub const MAX_NET_SERVER_MAX_TICK_RATE: i32 = 1000;
/// Default `MaxClientRate` from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_INI_MAX_CLIENT_RATE: i32 = 750_000;
/// Default `MaxInternetClientRate` from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_INI_MAX_INTERNET_CLIENT_RATE: i32 = 750_000;
/// Default `ConfiguredInternetSpeed` from `DefaultEngine.ini`
/// (`[/Script/Engine.Player]`, F-745).
pub const DEFAULT_CONFIGURED_INTERNET_SPEED: i32 = 640_000;
/// Default `ConfiguredLanSpeed` from `DefaultEngine.ini`
/// (`[/Script/Engine.Player]`, F-745).
pub const DEFAULT_CONFIGURED_LAN_SPEED: i32 = 912_000;
/// Default `ConnectionTimeout` in seconds from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_CONNECTION_TIMEOUT_SECONDS: f32 = 360.0;
/// Default `InitialConnectTimeout` in seconds from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_INITIAL_CONNECT_TIMEOUT_SECONDS: f32 = 360.0;
/// Default `ResolutionConnectionTimeout` in seconds from `DefaultEngine.ini`
/// (`[/Script/OnlineSubsystemUtils.IpNetDriver]`, F-745).
pub const DEFAULT_RESOLUTION_CONNECTION_TIMEOUT_SECONDS: f32 = 120.0;

/// Research resource policy; the original bounds are unknown.
#[derive(Clone, Copy, Debug)]
pub struct ActorChannelLimits {
    /// Content blocks accepted per bunch body.
    pub max_blocks: usize,
    /// Opaque payload bits accepted per content block.
    pub max_payload_bits: usize,
    /// Declared path-string units, including the terminator.
    pub max_path_units: usize,
}

impl Default for ActorChannelLimits {
    fn default() -> Self {
        Self {
            max_blocks: 1024,
            max_payload_bits: 65_536,
            max_path_units: 1024,
        }
    }
}

/// Caller-selected codec parameters; see the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct ActorChannelCodec {
    pub engine_net_version: u32,
    /// Export-flags bytes follow every nonzero GUID, not only the default one.
    pub exports_enabled: bool,
    /// Dynamic spawn records carry the level reference.
    pub has_level: bool,
    /// Order of raw vector floats, checksums and path-string counts.
    pub order: ByteOrder,
    pub limits: ActorChannelLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidKind {
    /// GUID 0: no object.
    Null,
    /// Odd GUID.
    Static,
    /// Nonzero even GUID.
    Dynamic,
}

pub fn guid_kind(guid: u32) -> GuidKind {
    match guid {
        0 => GuidKind::Null,
        guid if guid.is_multiple_of(2) => GuidKind::Dynamic,
        _ => GuidKind::Static,
    }
}

/// One NetGUID reference (F-572). `Debug` shows kinds and lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct NetGuidRef {
    pub guid: u32,
    /// Present exactly when the GUID is nonzero and is the default GUID or the
    /// codec has exports enabled.
    pub export: Option<GuidExport>,
}

/// The export-flags byte and what it adds.
#[derive(Clone, PartialEq, Eq)]
pub struct GuidExport {
    pub no_load: bool,
    /// Flag bit 0.
    pub path: Option<ExportPath>,
    /// Flag bit 2 with a path: the checksum read after the path.
    pub checksum: Option<u32>,
    /// Flag bit 2 without a path, which carries no checksum (F-692).
    pub bare_checksum_flag: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExportPath {
    pub outer: Box<NetGuidRef>,
    pub path: ArchiveText,
}

impl NetGuidRef {
    pub fn null() -> Self {
        Self {
            guid: 0,
            export: None,
        }
    }

    /// A reference without an export-flags byte.
    pub fn plain(guid: u32) -> Self {
        Self { guid, export: None }
    }

    pub fn kind(&self) -> GuidKind {
        guid_kind(self.guid)
    }

    pub fn summary(&self) -> GuidSummary {
        let export = self.export.as_ref();
        let path = export.and_then(|export| export.path.as_ref());
        GuidSummary {
            kind: self.kind(),
            export: export.is_some(),
            no_load: export.is_some_and(|export| export.no_load),
            checksum: export
                .is_some_and(|export| export.checksum.is_some() || export.bare_checksum_flag),
            path_units: path.map(|path| path.path.units().len()),
            outer: path.map(|path| Box::new(path.outer.summary())),
        }
    }
}

impl std::fmt::Debug for NetGuidRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetGuidRef")
            .field("kind", &self.kind())
            .field("export", &self.export)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for GuidExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuidExport")
            .field("no_load", &self.no_load)
            .field("path", &self.path)
            .field("checksum", &self.checksum.is_some())
            .field("bare_checksum_flag", &self.bare_checksum_flag)
            .finish()
    }
}

impl std::fmt::Debug for ExportPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExportPath")
            .field("outer", &self.outer)
            .field("path", &self.path)
            .finish()
    }
}

/// One registered NetGUID export entry in [`PackageMapState`]
/// (`UPackageMapClient::ReceiveNetGUIDBunch` `0x1439b8bb0`,
/// `UPackageMapClient::InternalLoadObject` `0x1439acc90`, F-709, F-710).
/// `Debug` shows lengths and flags only.
#[derive(Clone, PartialEq, Eq)]
pub struct NetGuidExportEntry {
    pub outer_guid: u32,
    pub path: String,
    pub no_load: bool,
    pub checksum: Option<u32>,
}

impl std::fmt::Debug for NetGuidExportEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetGuidExportEntry")
            .field("has_outer", &(self.outer_guid != 0))
            .field("path_len", &self.path.len())
            .field("no_load", &self.no_load)
            .field("has_checksum", &self.checksum.is_some())
            .finish()
    }
}

/// Disconnected package-map export table (`FNetGUIDCache` / `UPackageMapClient`,
/// `0x1439b8bb0`, `0x1439acc90`, `0x1437c43f0`, F-709, F-710).
///
/// Tracks `(NetGUID -> outer_guid, path, no_load, checksum)` mappings from
/// `ReceiveNetGUIDBunch` export prefixes and inline `NetGuidRef` exports, and
/// checks `bHasMustBeMappedGUIDs` prefixes against registered GUIDs. `Debug`
/// shows the entry count only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PackageMapState {
    entries: BTreeMap<u32, NetGuidExportEntry>,
}

impl std::fmt::Debug for PackageMapState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackageMapState")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl PackageMapState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, guid: u32) -> bool {
        self.entries.contains_key(&guid)
    }

    pub fn get(&self, guid: u32) -> Option<&NetGuidExportEntry> {
        self.entries.get(&guid)
    }

    /// Resolve the direct exported path segment for `guid`.
    pub fn resolve_path(&self, guid: u32) -> Option<&str> {
        self.entries.get(&guid).map(|entry| entry.path.as_str())
    }

    /// Resolve the outer GUID registered for `guid`.
    pub fn resolve_outer_guid(&self, guid: u32) -> Option<u32> {
        self.entries.get(&guid).map(|entry| entry.outer_guid)
    }

    /// Walk outer GUIDs up to [`MAX_GUID_DEPTH`] and reconstruct the full
    /// `outer/leaf` or `outer.leaf` path string for `guid`.
    pub fn resolve_full_path(&self, guid: u32) -> Option<String> {
        let mut segments = Vec::new();
        let mut current = guid;
        for _ in 0..=MAX_GUID_DEPTH {
            let entry = self.entries.get(&current)?;
            segments.push(entry.path.as_str());
            if entry.outer_guid == 0 {
                break;
            }
            if !self.entries.contains_key(&entry.outer_guid) {
                break;
            }
            current = entry.outer_guid;
        }
        segments.reverse();
        let mut full = String::new();
        for segment in segments {
            if full.is_empty() || segment.starts_with('/') {
                full.push_str(segment);
            } else {
                full.push('.');
                full.push_str(segment);
            }
        }
        Some(full)
    }

    /// Recursively record any path-carrying export in `reference` (outer first,
    /// then `reference.guid`). Returns the number of path exports recorded.
    pub fn ingest_guid_ref(&mut self, reference: &NetGuidRef) -> usize {
        if reference.guid == 0 {
            return 0;
        }
        let Some(export) = &reference.export else {
            return 0;
        };
        let Some(export_path) = &export.path else {
            return 0;
        };
        let mut count = self.ingest_guid_ref(&export_path.outer);
        let path = String::from_utf16_lossy(export_path.path.units());
        self.entries.insert(
            reference.guid,
            NetGuidExportEntry {
                outer_guid: export_path.outer.guid,
                path,
                no_load: export.no_load,
                checksum: export.checksum,
            },
        );
        count += 1;
        count
    }

    /// Read a `ReceiveNetGUIDBunch` (`0x1439b8bb0`) export prefix via
    /// [`read_export_prefix`] and ingest every exported path into `self`.
    pub fn consume_export_prefix(
        &mut self,
        reader: &mut BitReader<'_>,
        codec: &ActorChannelCodec,
    ) -> Result<Vec<NetGuidRef>> {
        let references = read_export_prefix(reader, codec)?;
        for reference in &references {
            self.ingest_guid_ref(reference);
        }
        Ok(references)
    }

    /// Read a `UActorChannel::ReceivedBunch` (`0x1437c43f0`) must-be-mapped
    /// GUID prefix via [`read_must_be_mapped_guids`] and return `(guids,
    /// resolved_count)` where `resolved_count` is how many of those GUIDs are
    /// registered in `self`.
    pub fn consume_must_be_mapped(
        &self,
        reader: &mut BitReader<'_>,
        codec: &ActorChannelCodec,
    ) -> Result<(Vec<u32>, usize)> {
        let guids = read_must_be_mapped_guids(reader, codec)?;
        let resolved = guids.iter().filter(|&&guid| self.contains(guid)).count();
        Ok((guids, resolved))
    }

    /// Encode an export-only bunch payload (`UPackageMapClient::ExportNetGUIDHeader`
    /// `0x1439ab670` and `ExportNetGUID` `0x1439aabb0`) and record every
    /// exported path into `self`.
    pub fn encode_export_bunch_payload(
        &mut self,
        references: &[NetGuidRef],
        codec: &ActorChannelCodec,
    ) -> Result<Payload> {
        let mut writer = BitWriter::new();
        write_export_prefix(&mut writer, references, codec)?;
        for reference in references {
            self.ingest_guid_ref(reference);
        }
        Ok(Payload::from_writer(writer))
    }

    /// Prepend a `bHasMustBeMappedGUIDs` prefix (`u16` count + packed GUIDs) in
    /// front of `body` (`UActorChannel::AppendMustBeMappedGuids` `0x1437b0490`).
    pub fn prepend_must_be_mapped_guids(
        &self,
        guids: &[u32],
        body: &Payload,
        codec: &ActorChannelCodec,
    ) -> Result<Payload> {
        let mut writer = BitWriter::new();
        write_must_be_mapped_guids(&mut writer, guids, codec)?;
        let mut reader = body.reader();
        let mut rem = reader.remaining();
        while rem > 0 {
            let chunk = rem.min(32) as u32;
            writer.write_bits(reader.read_bits(chunk), chunk);
            rem -= chunk as usize;
        }
        Ok(Payload::from_writer(writer))
    }
}

/// Spawn vector (F-571). Quantized integers are canonical; no float is applied
/// on read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpawnVector {
    /// Three 4-byte float bit patterns, X then Y then Z (version 13+ only).
    Raw([u32; 3]),
    /// The capped bit-count field (below 24) and three components, each below
    /// `1 << (bits + 2)`.
    Quantized { bits: u32, components: [u32; 3] },
}

impl SpawnVector {
    /// Decode: raw bit patterns as floats; quantized as
    /// `(value - (1 << (bits + 1))) * 0.1` in f32 (F-573). Equality with the
    /// original's float arithmetic is not claimed.
    pub fn decode(&self) -> [f32; 3] {
        match *self {
            Self::Raw(bits) => bits.map(f32::from_bits),
            Self::Quantized { bits, components } => {
                let bias = 1i64 << (bits.min(VECTOR_BITS_MAX) + 1);
                components.map(|value| (i64::from(value) - bias) as f32 * VECTOR_SCALE)
            }
        }
    }

    pub fn summary(&self) -> VectorSummary {
        match self {
            Self::Raw(_) => VectorSummary {
                form: VectorForm::Raw,
                bits: None,
            },
            Self::Quantized { bits, .. } => VectorSummary {
                form: VectorForm::Quantized,
                bits: Some(*bits),
            },
        }
    }
}

impl std::fmt::Debug for SpawnVector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw(_) => formatter.write_str("Raw"),
            Self::Quantized { bits, .. } => formatter
                .debug_struct("Quantized")
                .field("bits", bits)
                .finish_non_exhaustive(),
        }
    }
}

/// Compressed rotator (F-573): per axis, `None` when the bit is clear, else the
/// raw 16-bit quantized angle. `Some(0)` is preserved as read, although the
/// observed writer sets the bit only for nonzero angles.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CompressedRotator {
    pub pitch: Option<u16>,
    pub yaw: Option<u16>,
    pub roll: Option<u16>,
}

/// `round(degrees * 65536 / 360)` modulo 65536, rounding half up (inferred).
pub fn quantize_angle(degrees: f32) -> Result<u16> {
    ensure!(degrees.is_finite(), "rotator angle must be finite");
    let scaled = (degrees * 65536.0 / 360.0 + 0.5).floor();
    ensure!(
        scaled.abs() < 2f32.powi(62),
        "rotator angle is out of the quantization range"
    );
    Ok((scaled as i64 & 0xffff) as u16)
}

/// `value * 360 / 65536` degrees.
pub fn dequantize_angle(value: u16) -> f32 {
    f32::from(value) * 360.0 / 65536.0
}

impl CompressedRotator {
    /// Canonical encoding: an axis is present exactly when its quantized value
    /// is nonzero.
    pub fn from_degrees(pitch: f32, yaw: f32, roll: f32) -> Result<Self> {
        let axis = |degrees| quantize_angle(degrees).map(|value| (value != 0).then_some(value));
        Ok(Self {
            pitch: axis(pitch)?,
            yaw: axis(yaw)?,
            roll: axis(roll)?,
        })
    }

    /// Pitch, yaw and roll in degrees; absent axes are zero.
    pub fn degrees(&self) -> [f32; 3] {
        [self.pitch, self.yaw, self.roll].map(|axis| dequantize_angle(axis.unwrap_or(0)))
    }

    fn axes(&self) -> [Option<u16>; 3] {
        [self.pitch, self.yaw, self.roll]
    }

    pub fn summary(&self) -> RotatorSummary {
        RotatorSummary {
            axes_present: self.axes().iter().filter(|axis| axis.is_some()).count(),
        }
    }
}

impl std::fmt::Debug for CompressedRotator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompressedRotator")
            .field("pitch", &self.pitch.is_some())
            .field("yaw", &self.yaw.is_some())
            .field("roll", &self.roll.is_some())
            .finish()
    }
}

/// The fields after a dynamic actor GUID (F-569).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicSpawn {
    pub archetype: NetGuidRef,
    /// Present exactly when the codec selects the level reference.
    pub level: Option<NetGuidRef>,
    pub location: Option<SpawnVector>,
    pub rotation: Option<CompressedRotator>,
    pub scale: Option<SpawnVector>,
    pub velocity: Option<SpawnVector>,
}

/// Open-bunch spawn record (F-569).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRecord {
    pub actor: NetGuidRef,
    /// Present exactly when the actor GUID is dynamic.
    pub dynamic: Option<DynamicSpawn>,
}

/// Class of a non-actor content block (F-576).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubObjectClass {
    /// Stably-named bit set; no class reference follows.
    StablyNamed,
    /// A nonzero class reference.
    Class(NetGuidRef),
    /// A zero class reference: the sub-object is deleted.
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockTarget {
    Actor,
    SubObject {
        object: NetGuidRef,
        class: SubObjectClass,
    },
}

/// One content block; `payload` bits are opaque replicator input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentBlock {
    pub has_rep_layout: bool,
    pub target: BlockTarget,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorBunchBody {
    /// Present exactly when the bunch opens the channel.
    pub spawn: Option<SpawnRecord>,
    pub blocks: Vec<ContentBlock>,
}

/// Open-bunch body for `APlayerController` and its subclasses (`0x1437c0c30`,
/// `0x143ae73a0`, `0x143ae79d0`, F-696, F-709): the spawn record, the 1-byte
/// `NetPlayerIndex` serialized by `OnSerializeNewActor` / `OnActorChannelOpen`,
/// and the trailing content blocks.
#[derive(Clone, PartialEq, Eq)]
pub struct PlayerControllerOpenBunchBody {
    pub spawn: SpawnRecord,
    pub net_player_index: u8,
    pub blocks: Vec<ContentBlock>,
}

impl std::fmt::Debug for PlayerControllerOpenBunchBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlayerControllerOpenBunchBody")
            .field("spawn", &self.spawn)
            .field("blocks", &self.blocks)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct GuidSummary {
    pub kind: GuidKind,
    pub export: bool,
    pub no_load: bool,
    pub checksum: bool,
    pub path_units: Option<usize>,
    pub outer: Option<Box<GuidSummary>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorForm {
    Raw,
    Quantized,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct VectorSummary {
    pub form: VectorForm,
    pub bits: Option<u32>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct RotatorSummary {
    pub axes_present: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpawnSummary {
    pub actor: GuidSummary,
    pub archetype: Option<GuidSummary>,
    pub level: Option<GuidSummary>,
    pub location: Option<VectorSummary>,
    pub rotation: Option<RotatorSummary>,
    pub scale: Option<VectorSummary>,
    pub velocity: Option<VectorSummary>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Actor,
    StablyNamed,
    Class,
    Delete,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockSummary {
    pub has_rep_layout: bool,
    pub kind: BlockKind,
    pub object: Option<GuidSummary>,
    pub class: Option<GuidSummary>,
    pub payload_bits: usize,
}

/// Kinds, counts and lengths only.
#[derive(Clone, Debug, Serialize)]
pub struct ActorBunchSummary {
    pub spawn: Option<SpawnSummary>,
    pub blocks: Vec<BlockSummary>,
}

impl SpawnRecord {
    pub fn summary(&self) -> SpawnSummary {
        let dynamic = self.dynamic.as_ref();
        SpawnSummary {
            actor: self.actor.summary(),
            archetype: dynamic.map(|spawn| spawn.archetype.summary()),
            level: dynamic
                .and_then(|spawn| spawn.level.as_ref())
                .map(NetGuidRef::summary),
            location: dynamic
                .and_then(|spawn| spawn.location)
                .map(|vector| vector.summary()),
            rotation: dynamic
                .and_then(|spawn| spawn.rotation)
                .map(|rotation| rotation.summary()),
            scale: dynamic
                .and_then(|spawn| spawn.scale)
                .map(|vector| vector.summary()),
            velocity: dynamic
                .and_then(|spawn| spawn.velocity)
                .map(|vector| vector.summary()),
        }
    }
}

impl ContentBlock {
    pub fn summary(&self) -> BlockSummary {
        let (kind, object, class) = match &self.target {
            BlockTarget::Actor => (BlockKind::Actor, None, None),
            BlockTarget::SubObject { object, class } => {
                let (kind, class) = match class {
                    SubObjectClass::StablyNamed => (BlockKind::StablyNamed, None),
                    SubObjectClass::Class(class) => (BlockKind::Class, Some(class.summary())),
                    SubObjectClass::Delete => (BlockKind::Delete, None),
                };
                (kind, Some(object.summary()), class)
            }
        };
        BlockSummary {
            has_rep_layout: self.has_rep_layout,
            kind,
            object,
            class,
            payload_bits: self.payload.bit_len(),
        }
    }
}

impl ActorBunchBody {
    pub fn summary(&self) -> ActorBunchSummary {
        ActorBunchSummary {
            spawn: self.spawn.as_ref().map(SpawnRecord::summary),
            blocks: self.blocks.iter().map(ContentBlock::summary).collect(),
        }
    }
}

impl PlayerControllerOpenBunchBody {
    pub fn summary(&self) -> ActorBunchSummary {
        ActorBunchSummary {
            spawn: Some(self.spawn.summary()),
            blocks: self.blocks.iter().map(ContentBlock::summary).collect(),
        }
    }
}

fn read_flag(reader: &mut BitReader<'_>, what: &str) -> Result<bool> {
    let bit = reader.read_bit();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(bit)
}

fn read_capped(reader: &mut BitReader<'_>, max: u32, what: &str) -> Result<u32> {
    let value = reader.read_int_max(max);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

/// Packed integer that must re-encode to exactly the bytes read.
pub(crate) fn read_packed(reader: &mut BitReader<'_>, what: &str) -> Result<u32> {
    let mut raw = [0u8; 5];
    let mut len = 0;
    let mut value = 0u32;
    for group in 0..5u32 {
        let byte = reader.read_bits(8);
        ensure!(!reader.is_error(), "truncated {what}");
        raw[len] = byte as u8;
        len += 1;
        value |= (byte >> 1) << (group * 7);
        if byte & 1 == 0 {
            break;
        }
    }
    let mut canonical = BitWriter::new();
    canonical.write_int_packed(value);
    ensure!(
        canonical.as_bytes() == &raw[..len],
        "non-canonical packed {what} unsupported"
    );
    Ok(value)
}

fn read_u16(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u16> {
    let value = reader.read_bits(16) as u16;
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

fn write_u16(writer: &mut BitWriter, value: u16, order: ByteOrder) {
    let value = match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    };
    writer.write_bits(u32::from(value), 16);
}

fn read_u32(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u32> {
    let value = reader.read_u32();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

fn write_u32(writer: &mut BitWriter, value: u32, order: ByteOrder) {
    writer.write_u32(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    });
}

/// Copy `bits` from the reader; the caller has checked the bound.
fn copy_bits(reader: &mut BitReader<'_>, mut bits: usize) -> Payload {
    let mut writer = BitWriter::new();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
    Payload::from_writer(writer)
}

fn write_payload(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    let mut bits = payload.bit_len();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

fn read_path(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<ArchiveText> {
    let count = read_u32(reader, codec.order, "export path count")? as i32;
    ensure!(count != i32::MIN, "export path count i32::MIN unsupported");
    let units = count.unsigned_abs() as usize;
    ensure!(
        units <= codec.limits.max_path_units,
        "export path exceeds the path unit limit"
    );
    // The cap bounds the allocation; overflowing reads allocate nothing.
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated export path payload");
    let prefix = match codec.order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let frame = [prefix.as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, codec.order).context("invalid export path")?;
    ensure!(
        parsed.consumed() == frame.len(),
        "export path frame length mismatch"
    );
    let encoding = match parsed.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    ArchiveText::with_encoding(&parsed.units().collect::<Vec<_>>(), encoding)
}

fn write_path(writer: &mut BitWriter, text: &ArchiveText, codec: &ActorChannelCodec) -> Result<()> {
    let declared = text.declared_units();
    ensure!(
        declared <= codec.limits.max_path_units,
        "export path exceeds the path unit limit"
    );
    let wide = text.encoding() == TokenEncoding::Wide;
    let count = if wide {
        -(declared as i32)
    } else {
        declared as i32
    };
    write_u32(writer, count as u32, codec.order);
    if text.encoding() == TokenEncoding::Empty {
        return Ok(());
    }
    for unit in text.units().iter().copied().chain(std::iter::once(0)) {
        if wide {
            writer.write_bytes(&match codec.order {
                ByteOrder::Little => unit.to_le_bytes(),
                ByteOrder::Big => unit.to_be_bytes(),
            });
        } else {
            writer.write_bits(u32::from(unit), 8);
        }
    }
    Ok(())
}

fn export_expected(guid: u32, codec: &ActorChannelCodec) -> bool {
    guid != 0 && (guid == DEFAULT_GUID || codec.exports_enabled)
}

/// Read one NetGUID reference (F-572).
pub fn read_guid_ref(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<NetGuidRef> {
    read_guid_at(reader, codec, 0)
}

fn read_guid_at(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    depth: usize,
) -> Result<NetGuidRef> {
    ensure!(
        depth <= MAX_GUID_DEPTH,
        "NetGUID outer recursion exceeds 16 levels"
    );
    let guid = read_packed(reader, "NetGUID")?;
    if !export_expected(guid, codec) {
        return Ok(NetGuidRef::plain(guid));
    }
    let flags = reader.read_bits(8) as u8;
    ensure!(!reader.is_error(), "truncated NetGUID export flags");
    ensure!(
        flags & !EXPORT_KNOWN == 0,
        "reserved NetGUID export flag bits unsupported"
    );
    let path = if flags & EXPORT_HAS_PATH != 0 {
        let outer = read_guid_at(reader, codec, depth + 1).context("NetGUID outer")?;
        let path = read_path(reader, codec)?;
        Some(ExportPath {
            outer: Box::new(outer),
            path,
        })
    } else {
        None
    };
    // The checksum is read inside the path branch only (F-692).
    let checksum = if flags & EXPORT_HAS_CHECKSUM != 0 && path.is_some() {
        Some(read_u32(reader, codec.order, "NetGUID checksum")?)
    } else {
        None
    };
    Ok(NetGuidRef {
        guid,
        export: Some(GuidExport {
            no_load: flags & EXPORT_NO_LOAD != 0,
            bare_checksum_flag: flags & EXPORT_HAS_CHECKSUM != 0 && path.is_none(),
            path,
            checksum,
        }),
    })
}

pub fn write_guid_ref(
    writer: &mut BitWriter,
    reference: &NetGuidRef,
    codec: &ActorChannelCodec,
) -> Result<()> {
    write_guid_at(writer, reference, codec, 0)
}

fn write_guid_at(
    writer: &mut BitWriter,
    reference: &NetGuidRef,
    codec: &ActorChannelCodec,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_GUID_DEPTH,
        "NetGUID outer recursion exceeds 16 levels"
    );
    ensure!(
        reference.export.is_some() == export_expected(reference.guid, codec),
        "export flags present exactly for a nonzero default or export-enabled GUID"
    );
    writer.write_int_packed(reference.guid);
    let Some(export) = &reference.export else {
        return Ok(());
    };
    ensure!(
        export.checksum.is_none() || export.path.is_some(),
        "a NetGUID checksum value needs a path; use bare_checksum_flag"
    );
    ensure!(
        !(export.bare_checksum_flag && export.path.is_some()),
        "bare_checksum_flag is for exports without a path"
    );
    let mut flags = 0u8;
    for (set, bit) in [
        (export.path.is_some(), EXPORT_HAS_PATH),
        (export.no_load, EXPORT_NO_LOAD),
        (
            export.checksum.is_some() || export.bare_checksum_flag,
            EXPORT_HAS_CHECKSUM,
        ),
    ] {
        if set {
            flags |= bit;
        }
    }
    writer.write_bits(u32::from(flags), 8);
    if let Some(path) = &export.path {
        write_guid_at(writer, &path.outer, codec, depth + 1)?;
        write_path(writer, &path.path, codec)?;
    }
    if let Some(checksum) = export.checksum {
        write_u32(writer, checksum, codec.order);
    }
    Ok(())
}

/// Read the `UPackageMapClient::ReceiveNetGUIDBunch` (`0x1439b8bb0`) export
/// prefix (`header.has_package_map_exports`, F-694, F-709): 1-bit
/// `bHasRepLayoutExport` (must be `0`), a 4-byte `int32` `NumGUIDsInBunch`
/// (`0..=MAX_EXPORT_GUIDS`), then that many [`read_guid_ref`] calls with
/// `exports_enabled = true`.
pub fn read_export_prefix(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
) -> Result<Vec<NetGuidRef>> {
    let has_rep_layout_export = read_flag(reader, "export prefix rep-layout bit")?;
    ensure!(
        !has_rep_layout_export,
        "rep-layout export prefix unsupported"
    );
    let count = read_u32(reader, codec.order, "export GUID count")? as i32;
    ensure!(
        (0..=MAX_EXPORT_GUIDS as i32).contains(&count),
        "export GUID count out of range"
    );
    let export_codec = ActorChannelCodec {
        exports_enabled: true,
        ..*codec
    };
    (0..count)
        .map(|index| {
            read_guid_ref(reader, &export_codec).with_context(|| format!("export GUID {index}"))
        })
        .collect()
}

/// Write the `UPackageMapClient::ExportNetGUIDHeader` (`0x1439ab670`) +
/// `ExportNetGUID` (`0x1439aa980`) prefix (F-694, F-709).
pub fn write_export_prefix(
    writer: &mut BitWriter,
    references: &[NetGuidRef],
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        references.len() <= MAX_EXPORT_GUIDS as usize,
        "export GUID count out of range"
    );
    writer.write_bit(false);
    write_u32(writer, references.len() as u32, codec.order);
    let export_codec = ActorChannelCodec {
        exports_enabled: true,
        ..*codec
    };
    for (index, reference) in references.iter().enumerate() {
        write_guid_ref(writer, reference, &export_codec)
            .with_context(|| format!("export GUID {index}"))?;
    }
    Ok(())
}

/// Read the `UActorChannel::ReceivedBunch` (`0x1437c43f0`) must-be-mapped GUID
/// prefix (`header.has_must_be_mapped_guids`, F-709): a 2-byte `uint16`
/// `NumMustBeMappedGUIDs` in `codec.order`, then that many packed NetGUIDs.
pub fn read_must_be_mapped_guids(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
) -> Result<Vec<u32>> {
    let count = read_u16(reader, codec.order, "must-be-mapped GUID count")?;
    let mut guids = Vec::with_capacity(usize::from(count));
    for index in 0..count {
        let guid = read_packed(reader, "must-be-mapped NetGUID")
            .with_context(|| format!("must-be-mapped GUID {index}"))?;
        guids.push(guid);
    }
    Ok(guids)
}

/// Mirror of [`read_must_be_mapped_guids`] (F-709).
pub fn write_must_be_mapped_guids(
    writer: &mut BitWriter,
    guids: &[u32],
    codec: &ActorChannelCodec,
) -> Result<()> {
    let count = u16::try_from(guids.len()).context("must-be-mapped GUID count exceeds u16::MAX")?;
    write_u16(writer, count, codec.order);
    for &guid in guids {
        writer.write_int_packed(guid);
    }
    Ok(())
}

fn read_vector(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    what: &str,
) -> Result<SpawnVector> {
    let quantized = if codec.engine_net_version >= QUANTIZED_BIT_NET_VERSION {
        read_flag(reader, what)?
    } else {
        true
    };
    if !quantized {
        let mut raw = [0u32; 3];
        for component in &mut raw {
            *component = read_u32(reader, codec.order, what)?;
        }
        return Ok(SpawnVector::Raw(raw));
    }
    let bits = read_capped(reader, VECTOR_BITS_MAX, what)?;
    let max = 1u32 << (bits + 2);
    let mut components = [0u32; 3];
    for component in &mut components {
        *component = read_capped(reader, max, what)?;
    }
    Ok(SpawnVector::Quantized { bits, components })
}

fn write_vector(
    writer: &mut BitWriter,
    vector: &SpawnVector,
    codec: &ActorChannelCodec,
) -> Result<()> {
    let versioned = codec.engine_net_version >= QUANTIZED_BIT_NET_VERSION;
    match vector {
        SpawnVector::Raw(raw) => {
            ensure!(
                versioned,
                "raw spawn vectors need engine net version 13 or later"
            );
            writer.write_bit(false);
            for component in raw {
                write_u32(writer, *component, codec.order);
            }
        }
        SpawnVector::Quantized { bits, components } => {
            ensure!(
                *bits < VECTOR_BITS_MAX,
                "spawn vector bit count must be below 24"
            );
            let max = 1u32 << (bits + 2);
            ensure!(
                components.iter().all(|component| *component < max),
                "spawn vector component exceeds its bit count"
            );
            if versioned {
                writer.write_bit(true);
            }
            writer.write_int_max(*bits, VECTOR_BITS_MAX)?;
            for component in components {
                writer.write_int_max(*component, max)?;
            }
        }
    }
    Ok(())
}

fn read_optional_vector(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    what: &str,
) -> Result<Option<SpawnVector>> {
    if read_flag(reader, what)? {
        Ok(Some(read_vector(reader, codec, what)?))
    } else {
        Ok(None)
    }
}

fn write_optional_vector(
    writer: &mut BitWriter,
    vector: Option<&SpawnVector>,
    codec: &ActorChannelCodec,
) -> Result<()> {
    writer.write_bit(vector.is_some());
    if let Some(vector) = vector {
        write_vector(writer, vector, codec)?;
    }
    Ok(())
}

/// Read a compressed rotator (F-573): per axis a bit, then a 16-bit value.
pub fn read_rotator(reader: &mut BitReader<'_>) -> Result<CompressedRotator> {
    let mut axes = [None; 3];
    for axis in &mut axes {
        if read_flag(reader, "rotator axis")? {
            let value = reader.read_bits(16) as u16;
            ensure!(!reader.is_error(), "truncated rotator axis");
            *axis = Some(value);
        }
    }
    let [pitch, yaw, roll] = axes;
    Ok(CompressedRotator { pitch, yaw, roll })
}

pub fn write_rotator(writer: &mut BitWriter, rotator: &CompressedRotator) {
    for axis in rotator.axes() {
        writer.write_bit(axis.is_some());
        if let Some(value) = axis {
            writer.write_bits(u32::from(value), 16);
        }
    }
}

/// Read the open-bunch spawn record (F-569).
pub fn read_spawn(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<SpawnRecord> {
    let actor = read_guid_ref(reader, codec).context("spawn actor")?;
    let dynamic = if actor.kind() == GuidKind::Dynamic {
        let archetype = read_guid_ref(reader, codec).context("spawn archetype")?;
        let level = if codec.has_level {
            Some(read_guid_ref(reader, codec).context("spawn level")?)
        } else {
            None
        };
        let location = read_optional_vector(reader, codec, "spawn location")?;
        let rotation = if read_flag(reader, "spawn rotation")? {
            Some(read_rotator(reader).context("spawn rotation")?)
        } else {
            None
        };
        let scale = read_optional_vector(reader, codec, "spawn scale")?;
        let velocity = read_optional_vector(reader, codec, "spawn velocity")?;
        Some(DynamicSpawn {
            archetype,
            level,
            location,
            rotation,
            scale,
            velocity,
        })
    } else {
        None
    };
    Ok(SpawnRecord { actor, dynamic })
}

pub fn write_spawn(
    writer: &mut BitWriter,
    spawn: &SpawnRecord,
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        spawn.dynamic.is_some() == (spawn.actor.kind() == GuidKind::Dynamic),
        "dynamic spawn fields present exactly for a dynamic actor GUID"
    );
    write_guid_ref(writer, &spawn.actor, codec)?;
    let Some(dynamic) = &spawn.dynamic else {
        return Ok(());
    };
    ensure!(
        dynamic.level.is_some() == codec.has_level,
        "spawn level present exactly when the codec selects it"
    );
    write_guid_ref(writer, &dynamic.archetype, codec)?;
    if let Some(level) = &dynamic.level {
        write_guid_ref(writer, level, codec)?;
    }
    write_optional_vector(writer, dynamic.location.as_ref(), codec)?;
    writer.write_bit(dynamic.rotation.is_some());
    if let Some(rotation) = &dynamic.rotation {
        write_rotator(writer, rotation);
    }
    write_optional_vector(writer, dynamic.scale.as_ref(), codec)?;
    write_optional_vector(writer, dynamic.velocity.as_ref(), codec)?;
    Ok(())
}

/// Read one content block header and its opaque payload (F-576). A deletion
/// block (zero class reference) carries no payload count: the client reads the
/// count only when the header did not delete the sub-object (M-424, M-425).
pub fn read_content_block(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
) -> Result<ContentBlock> {
    let has_rep_layout = read_flag(reader, "content block rep-layout bit")?;
    let is_actor = read_flag(reader, "content block actor bit")?;
    let target = if is_actor {
        BlockTarget::Actor
    } else {
        let object = read_guid_ref(reader, codec).context("content block sub-object")?;
        let class = if read_flag(reader, "content block stably-named bit")? {
            SubObjectClass::StablyNamed
        } else {
            let class = read_guid_ref(reader, codec).context("content block class")?;
            if class.guid == 0 {
                SubObjectClass::Delete
            } else {
                SubObjectClass::Class(class)
            }
        };
        BlockTarget::SubObject { object, class }
    };
    if matches!(
        target,
        BlockTarget::SubObject {
            class: SubObjectClass::Delete,
            ..
        }
    ) {
        return Ok(ContentBlock {
            has_rep_layout,
            target,
            payload: copy_bits(reader, 0),
        });
    }
    let bits = read_packed(reader, "content block payload bit count")? as usize;
    ensure!(
        bits <= codec.limits.max_payload_bits,
        "content block payload exceeds the payload bit limit"
    );
    ensure!(
        bits <= reader.remaining(),
        "content block payload exceeds the bunch"
    );
    Ok(ContentBlock {
        has_rep_layout,
        target,
        payload: copy_bits(reader, bits),
    })
}

pub fn write_content_block(
    writer: &mut BitWriter,
    block: &ContentBlock,
    codec: &ActorChannelCodec,
) -> Result<()> {
    let bits = block.payload.bit_len();
    ensure!(
        bits <= codec.limits.max_payload_bits,
        "content block payload exceeds the payload bit limit"
    );
    let bits = u32::try_from(bits).context("content block payload exceeds 32 bits")?;
    writer.write_bit(block.has_rep_layout);
    match &block.target {
        BlockTarget::Actor => writer.write_bit(true),
        BlockTarget::SubObject { object, class } => {
            writer.write_bit(false);
            write_guid_ref(writer, object, codec)?;
            match class {
                SubObjectClass::StablyNamed => writer.write_bit(true),
                SubObjectClass::Class(class) => {
                    ensure!(class.guid != 0, "a zero class reference is a deletion");
                    writer.write_bit(false);
                    write_guid_ref(writer, class, codec)?;
                }
                SubObjectClass::Delete => {
                    ensure!(bits == 0, "a deletion block carries no payload");
                    writer.write_bit(false);
                    write_guid_ref(writer, &NetGuidRef::null(), codec)?;
                    return Ok(());
                }
            }
        }
    }
    writer.write_int_packed(bits);
    write_payload(writer, &block.payload);
    Ok(())
}

/// Read a whole actor bunch body: the spawn record when `open`, then content
/// blocks until the reader is exactly at its end.
pub fn read_actor_bunch_body(
    reader: &mut BitReader<'_>,
    open: bool,
    codec: &ActorChannelCodec,
) -> Result<ActorBunchBody> {
    let spawn = if open {
        Some(read_spawn(reader, codec).context("spawn record")?)
    } else {
        None
    };
    let mut blocks = Vec::new();
    while !reader.at_end() {
        let index = blocks.len();
        ensure!(
            index < codec.limits.max_blocks,
            "actor bunch exceeds the content block limit"
        );
        blocks.push(
            read_content_block(reader, codec).with_context(|| format!("content block {index}"))?,
        );
    }
    Ok(ActorBunchBody { spawn, blocks })
}

pub fn write_actor_bunch_body(
    writer: &mut BitWriter,
    body: &ActorBunchBody,
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        body.blocks.len() <= codec.limits.max_blocks,
        "actor bunch exceeds the content block limit"
    );
    if let Some(spawn) = &body.spawn {
        write_spawn(writer, spawn, codec).context("spawn record")?;
    }
    for (index, block) in body.blocks.iter().enumerate() {
        write_content_block(writer, block, codec)
            .with_context(|| format!("content block {index}"))?;
    }
    Ok(())
}

/// Encode a body as a bunch payload starting at bit 0.
pub fn encode_actor_bunch_body(
    body: &ActorBunchBody,
    codec: &ActorChannelCodec,
) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_actor_bunch_body(&mut writer, body, codec)?;
    Ok(Payload::from_writer(writer))
}

/// Parse a complete, unpartitioned bunch's payload as an actor bunch body.
/// Export and must-be-mapped prefixes must be stripped beforehand.
pub fn parse_actor_bunch(bunch: &Bunch, codec: &ActorChannelCodec) -> Result<ActorBunchBody> {
    let header = &bunch.header;
    ensure!(!header.partial, "partial actor bunches unsupported");
    ensure!(
        !header.has_package_map_exports,
        "NetGUID export bunches unsupported"
    );
    ensure!(
        !header.has_must_be_mapped_guids,
        "must-be-mapped GUID prefixes unsupported"
    );
    let mut reader = bunch.payload.reader();
    read_actor_bunch_body(&mut reader, header.open, codec)
}

/// Read an `APlayerController` open-bunch body (`0x1437c0c30`, `0x143ae73a0`,
/// F-696, F-709): the spawn record, the 1-byte `NetPlayerIndex` read by
/// `APlayerController::OnActorChannelOpen`, then content blocks until the
/// reader is at its end.
pub fn read_player_controller_open_bunch_body(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
) -> Result<PlayerControllerOpenBunchBody> {
    let spawn = read_spawn(reader, codec).context("spawn record")?;
    let net_player_index = reader.read_bits(8) as u8;
    ensure!(
        !reader.is_error(),
        "truncated PlayerController NetPlayerIndex"
    );
    let mut blocks = Vec::new();
    while !reader.at_end() {
        let index = blocks.len();
        ensure!(
            index < codec.limits.max_blocks,
            "actor bunch exceeds the content block limit"
        );
        blocks.push(
            read_content_block(reader, codec).with_context(|| format!("content block {index}"))?,
        );
    }
    Ok(PlayerControllerOpenBunchBody {
        spawn,
        net_player_index,
        blocks,
    })
}

/// Write an `APlayerController` open-bunch body (`0x1437c7f00`, `0x143ae79d0`,
/// F-696, F-709).
pub fn write_player_controller_open_bunch_body(
    writer: &mut BitWriter,
    body: &PlayerControllerOpenBunchBody,
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        body.blocks.len() <= codec.limits.max_blocks,
        "actor bunch exceeds the content block limit"
    );
    write_spawn(writer, &body.spawn, codec).context("spawn record")?;
    writer.write_bits(u32::from(body.net_player_index), 8);
    for (index, block) in body.blocks.iter().enumerate() {
        write_content_block(writer, block, codec)
            .with_context(|| format!("content block {index}"))?;
    }
    Ok(())
}

/// Encode an `APlayerController` open-bunch body starting at bit 0 (F-696, F-709).
pub fn encode_player_controller_open_bunch_body(
    body: &PlayerControllerOpenBunchBody,
    codec: &ActorChannelCodec,
) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_player_controller_open_bunch_body(&mut writer, body, codec)?;
    Ok(Payload::from_writer(writer))
}

/// Parse a complete, unpartitioned open bunch's payload as an
/// `APlayerController` open-bunch body (`0x1437c0c30`, `0x143ae73a0`, F-696,
/// F-709). Export and must-be-mapped prefixes must be stripped beforehand.
pub fn parse_player_controller_open_bunch(
    bunch: &Bunch,
    codec: &ActorChannelCodec,
) -> Result<PlayerControllerOpenBunchBody> {
    let header = &bunch.header;
    ensure!(
        header.open,
        "PlayerController open bunch requires header.open"
    );
    ensure!(!header.partial, "partial actor bunches unsupported");
    ensure!(
        !header.has_package_map_exports,
        "NetGUID export bunches unsupported"
    );
    ensure!(
        !header.has_must_be_mapped_guids,
        "must-be-mapped GUID prefixes unsupported"
    );
    let mut reader = bunch.payload.reader();
    read_player_controller_open_bunch_body(&mut reader, codec)
}

/// Engine network mode (`ENetMode`, `UNetDriver::GetNetMode` `0x14370ac50`
/// server / `0x1439ad400` client -> `UWorld::InternalGetNetMode` `0x143be5e10`
/// server / `0x143eb25e0` client, F-745).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineNetMode {
    /// `NM_Standalone` (`0`).
    Standalone = 0,
    /// `NM_DedicatedServer` (`1`).
    DedicatedServer = 1,
    /// `NM_ListenServer` (`2`).
    ListenServer = 2,
    /// `NM_Client` (`3`).
    Client = 3,
}

/// Evaluate `UGameEngine::GetMaxTickRate` (`0x1435a90e0` server / `0x143836250`
/// client, F-745).
///
/// - If `base_engine_max_tick_rate != 0.0` (`ucomiss xmm0, xmm7` where
///   `xmm7 = 0.0`), returns `base_engine_max_tick_rate`.
/// - Otherwise, when `net_mode == EngineNetMode::DedicatedServer` (`1`) or
///   (`net_mode == EngineNetMode::ListenServer` (`2`) and
///   `b_use_client_max_tick_rate_when_listen_server` (`[NetDriver+0x54] & 1 != 0`)),
///   returns `net_server_max_tick_rate.clamp(1, 1000) as f32` (`[NetDriver+0x58]`
///   clamped to `1..=0x3e8`).
/// - Otherwise returns `0.0`.
pub fn evaluate_game_engine_max_tick_rate(
    net_mode: EngineNetMode,
    b_use_client_max_tick_rate_when_listen_server: bool,
    net_server_max_tick_rate: i32,
    base_engine_max_tick_rate: f32,
) -> f32 {
    if base_engine_max_tick_rate != 0.0 {
        return base_engine_max_tick_rate;
    }
    let is_dedicated = net_mode == EngineNetMode::DedicatedServer;
    let is_listen_with_tick_rate =
        net_mode == EngineNetMode::ListenServer && b_use_client_max_tick_rate_when_listen_server;
    if is_dedicated || is_listen_with_tick_rate {
        let clamped = net_server_max_tick_rate
            .clamp(MIN_NET_SERVER_MAX_TICK_RATE, MAX_NET_SERVER_MAX_TICK_RATE);
        return clamped as f32;
    }
    0.0
}

/// Evaluate `AActor::IsWithinNetRelevancyDistance` (`0x143340200` server /
/// `0x1435d6240` client) and the distance check in `AActor::IsNetRelevantFor`
/// (`0x14333fc63` server, F-745).
///
/// Uses `comiss xmm2, xmm1; setb al` where `xmm1 = [rdi+0x100]`
/// (`NetCullDistanceSquared`) and `xmm2 = dist_sq`, which is strict `<`
/// (`dist_sq < net_cull_distance_squared`, returning `false` on `NaN`).
pub fn is_within_net_relevancy_distance(
    actor_location: [f32; 3],
    src_location: [f32; 3],
    net_cull_distance_squared: f32,
) -> bool {
    let dx = actor_location[0] - src_location[0];
    let dy = actor_location[1] - src_location[1];
    let dz = actor_location[2] - src_location[2];
    let dist_sq = dx * dx + dy * dy + dz * dz;
    dist_sq < net_cull_distance_squared
}

/// Outcome of `AActor::IsNetRelevantFor` (`0x14333fae0` server / `0x1435d5be0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorRelevancyOutcome {
    Relevant,
    NotRelevant {
        missing_root_component_warning: bool,
    },
    DelegateToOwner,
    DelegateToAttachParentOwner,
}

/// Inputs to `AActor::IsNetRelevantFor` (`0x14333fae0` server / `0x1435d5be0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorRelevancyInputs {
    /// `[this+0x58] & 0x8 != 0`.
    pub b_always_relevant: bool,
    /// `[this+0x58] & 0x4 != 0`.
    pub b_only_relevant_to_owner: bool,
    /// `[this+0x58] & 0x20 != 0`.
    pub b_hidden: bool,
    /// `[this+0x59] & 0x4 != 0`.
    pub b_use_owner_relevancy: bool,
    /// `this` or any `Owner` in `[actor+0xe0]` equals `ViewTarget` or `RealViewer`.
    pub is_owned_by_view_target_or_real_viewer: bool,
    /// `ViewTarget != null && ViewTarget == this->Instigator` at `[this+0x118]`.
    pub view_target_is_instigator: bool,
    /// `[this+0xe0] != null`.
    pub has_owner: bool,
    /// `[this+0x130] != null`.
    pub has_root_component: bool,
    /// `RootComponent->IsCollisionEnabled()` at `vtable + 0x4e8`.
    pub root_collision_enabled: bool,
    /// `RootComponent` && `AttachParent` (`+0xc0`) && `AttachParent->Owner`
    /// (`+0xa0`) && (`AttachParent->IsA(USkeletalMeshComponent)` ||
    /// `AttachParent->Owner == this->Owner`).
    pub attach_parent_delegates_to_owner: bool,
    /// `GetDefault<AGameNetworkManager>()->bUseDistanceBasedRelevancy` at `+0x2c1`.
    pub b_use_distance_based_relevancy: bool,
    /// `RootComponent->ComponentToWorld.Translation` at `+0x1d0`.
    pub actor_location: [f32; 3],
    /// Viewer `SrcLocation`.
    pub src_location: [f32; 3],
    /// `[this+0x100]` (`NetCullDistanceSquared`).
    pub net_cull_distance_squared: f32,
}

/// Evaluate `AActor::IsNetRelevantFor` (`0x14333fae0` server / `0x1435d5be0`
/// client, F-745) in exact branch order.
pub fn evaluate_actor_net_relevancy(inputs: &ActorRelevancyInputs) -> ActorRelevancyOutcome {
    if inputs.b_always_relevant
        || inputs.is_owned_by_view_target_or_real_viewer
        || inputs.view_target_is_instigator
    {
        return ActorRelevancyOutcome::Relevant;
    }
    if inputs.b_use_owner_relevancy && inputs.has_owner {
        return ActorRelevancyOutcome::DelegateToOwner;
    }
    if inputs.b_only_relevant_to_owner {
        return ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        };
    }
    if inputs.has_root_component && inputs.attach_parent_delegates_to_owner {
        return ActorRelevancyOutcome::DelegateToAttachParentOwner;
    }
    if inputs.b_hidden && (!inputs.has_root_component || !inputs.root_collision_enabled) {
        return ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        };
    }
    if !inputs.has_root_component {
        return ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: true,
        };
    }
    if !inputs.b_use_distance_based_relevancy {
        return ActorRelevancyOutcome::Relevant;
    }
    if is_within_net_relevancy_distance(
        inputs.actor_location,
        inputs.src_location,
        inputs.net_cull_distance_squared,
    ) {
        ActorRelevancyOutcome::Relevant
    } else {
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        }
    }
}

/// Outcome of `APawn::IsNetRelevantFor` (`0x1437b95a0` server / `0x143a681d0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PawnRelevancyOutcome {
    Relevant,
    NotRelevant,
    DelegateToMovementBaseOwner,
}

/// Inputs to `APawn::IsNetRelevantFor` (`0x1437b95a0` server / `0x143a681d0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PawnRelevancyInputs {
    pub b_always_relevant: bool,
    pub real_viewer_is_controller: bool,
    pub is_owned_by_view_target_or_real_viewer: bool,
    pub view_target_is_instigator: bool,
    pub based_on_view_target_or_vice_versa: bool,
    pub b_hidden: bool,
    pub b_only_relevant_to_owner: bool,
    pub has_root_component: bool,
    pub root_collision_enabled: bool,
    /// `MovementBase` && `MovementBase->Owner` && `MovementComponent` &&
    /// (`MovementBase->IsA(USkeletalMeshComponent)` ||
    /// `MovementBase->Owner == this->Owner`).
    pub movement_base_delegates_to_owner: bool,
    pub b_use_distance_based_relevancy: bool,
    pub actor_location: [f32; 3],
    pub src_location: [f32; 3],
    pub net_cull_distance_squared: f32,
}

/// Evaluate `APawn::IsNetRelevantFor` (`0x1437b95a0` server / `0x143a681d0`
/// client, F-745) in exact branch order.
pub fn evaluate_pawn_net_relevancy(inputs: &PawnRelevancyInputs) -> PawnRelevancyOutcome {
    if inputs.b_always_relevant
        || inputs.real_viewer_is_controller
        || inputs.is_owned_by_view_target_or_real_viewer
        || inputs.view_target_is_instigator
        || inputs.based_on_view_target_or_vice_versa
    {
        return PawnRelevancyOutcome::Relevant;
    }
    if (inputs.b_hidden || inputs.b_only_relevant_to_owner)
        && (!inputs.has_root_component || !inputs.root_collision_enabled)
    {
        return PawnRelevancyOutcome::NotRelevant;
    }
    if inputs.movement_base_delegates_to_owner {
        return PawnRelevancyOutcome::DelegateToMovementBaseOwner;
    }
    if !inputs.b_use_distance_based_relevancy {
        return PawnRelevancyOutcome::Relevant;
    }
    let within_distance = is_within_net_relevancy_distance(
        inputs.actor_location,
        inputs.src_location,
        inputs.net_cull_distance_squared,
    );
    if inputs.has_root_component && within_distance {
        PawnRelevancyOutcome::Relevant
    } else {
        PawnRelevancyOutcome::NotRelevant
    }
}

/// Evaluate `APlayerController::IsNetRelevantFor` (`0x1432510a0` server /
/// `0x1434e9040` client, F-745): `cmp rcx, rdx; sete al; ret` (`this == RealViewer`).
pub fn evaluate_player_controller_net_relevancy<T: PartialEq>(
    controller: T,
    real_viewer: T,
) -> bool {
    controller == real_viewer
}

/// Outcome of `AActor::GetNetPriority` (`0x14333b2e0` server / `0x1435d14e0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActorPriorityOutcome {
    DelegateToOwner,
    Computed(f32),
}

/// Inputs to `AActor::GetNetPriority` (`0x14333b2e0` server / `0x1435d14e0`
/// client, F-745).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorPriorityInputs {
    pub b_use_owner_relevancy: bool,
    pub has_owner: bool,
    /// `ViewTarget != null && (this == ViewTarget || this->Instigator == ViewTarget)`.
    pub is_view_target_or_instigator: bool,
    pub b_hidden: bool,
    /// `RootComponent->ComponentToWorld.Translation` at `+0x1d0`, or `None`
    /// when `RootComponent` is null.
    pub root_location: Option<[f32; 3]>,
    pub viewer_location: [f32; 3],
    pub viewer_dir: [f32; 3],
    pub time: f32,
    pub net_priority: f32,
}

/// Evaluate `AActor::GetNetPriority` (`0x14333b2e0` server / `0x1435d14e0`
/// client, F-745).
pub fn evaluate_actor_net_priority(inputs: &ActorPriorityInputs) -> ActorPriorityOutcome {
    if inputs.b_use_owner_relevancy && inputs.has_owner {
        return ActorPriorityOutcome::DelegateToOwner;
    }
    let mut time = inputs.time;
    if inputs.is_view_target_or_instigator {
        time *= 4.0;
    } else if !inputs.b_hidden
        && let Some(actor_location) = inputs.root_location
    {
        let dir = [
            actor_location[0] - inputs.viewer_location[0],
            actor_location[1] - inputs.viewer_location[1],
            actor_location[2] - inputs.viewer_location[2],
        ];
        let dist_sq = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        let dot = inputs.viewer_dir[0] * dir[0]
            + inputs.viewer_dir[1] * dir[1]
            + inputs.viewer_dir[2] * dir[2];
        if dot < 0.0 {
            if dist_sq > 4_000_000.0 {
                time *= 0.2;
            } else if dist_sq > 250_000.0 {
                time *= 0.4;
            }
        } else if dist_sq < 64_000_000.0 && dot * dot > 0.5 * dist_sq {
            time += time;
        } else if dist_sq > 9_998_244.0 {
            time *= 0.4;
        }
    }
    ActorPriorityOutcome::Computed(time * inputs.net_priority)
}

/// Evaluate `APlayerController::GetNetPriority` (`0x143830f20` server /
/// `0x143adfc40` client, F-745): `time * (if is_in_viewer { 4.0 } else { 1.0 }) * net_priority`.
pub fn evaluate_player_controller_net_priority(
    time: f32,
    net_priority: f32,
    is_in_viewer: bool,
) -> f32 {
    let viewer_multiplier = if is_in_viewer { 4.0 } else { 1.0 };
    time * viewer_multiplier * net_priority
}

/// Maximum receive idle time (`Driver->ElapsedTime - Conn->LastReceiveTime < 1.5`
/// at `0x1453e4ba0` server / `0x14553fa98` client) for a `UNetConnection` to be
/// treated as ready in `UNetDriver::ServerReplicateActors_PrepConnections`
/// (`0x143723dc0` server / `0x1439c0b20` client, F-748).
pub const PREP_CONNECTIONS_RECEIVE_TIMEOUT_SECONDS: f64 = 1.5;

/// Delay threshold (`World->TimeSeconds - LastNetReplicateTime > 2.0` at
/// `0x14474a53c` server / `0x14489ef7c` client) before adaptive net-update
/// frequency scaling activates in
/// `UNetDriver::ServerReplicateActors_BuildConsiderList` (`0x1437235a0` server
/// / `0x1439c0530` client, F-748).
pub const ADAPTIVE_NET_UPDATE_DELAY_THRESHOLD_SECONDS: f32 = 2.0;

/// Scale factor (`0.2` at `0x14476f360` server / `0x1448ca8fc` client) applied
/// to `LastNetUpdateDelta - 2.0` when computing the adaptive interpolation
/// alpha in `UNetDriver::ServerReplicateActors_BuildConsiderList` (F-748).
pub const ADAPTIVE_NET_UPDATE_LERP_SCALE: f32 = 0.2;

/// Default `AActor::MinNetUpdateFrequency` (`[actor + 0x10c] = 0x40000000` =
/// `2.0f`) written when `MinNetUpdateFrequency == 0.0` in
/// `UNetDriver::ServerReplicateActors_BuildConsiderList` (F-748).
pub const DEFAULT_MIN_NET_UPDATE_FREQUENCY: f32 = 2.0;

/// Evaluate `AOnlineBeacon::IsNetRelevantFor` (`0x1409a5780` server /
/// `0x1409cc210` client, `97` B, F-748).
///
/// Calls `SrcActor->GetNetConnection()` (`vtable + 0x4b0`) and
/// `this->GetNetConnection()` (`vtable + 0x4b0`), and compares `this->Class`
/// (`[this + 0x10]`) with `RealViewer->Class` (`[RealViewer + 0x10]`). When
/// `b_only_relevant_to_owner` (`([this + 0x58] & 0x04) != 0`) is set, returns
/// `beacon_net_connection == src_actor_net_connection`; otherwise returns
/// `beacon_class == real_viewer_class`.
pub fn evaluate_online_beacon_net_relevancy<C: PartialEq, N: PartialEq>(
    b_only_relevant_to_owner: bool,
    beacon_class: C,
    real_viewer_class: C,
    beacon_net_connection: N,
    src_actor_net_connection: N,
) -> bool {
    let same_net_connection = beacon_net_connection == src_actor_net_connection;
    let same_class = beacon_class == real_viewer_class;
    if b_only_relevant_to_owner {
        same_net_connection
    } else {
        same_class
    }
}

/// Evaluate `AMatineeActor::GetNetPriority` (`0x143894660` server /
/// `0x143b3f6c0` client, `147` B, F-748).
///
/// Calls `AActor::GetNetPriority` (`0x14333b2e0` server / `0x1435d14e0`
/// client); when `has_open_channel` (`InChannel != nullptr` at `[rsp + 0x88]`)
/// is `false`, adds `1.0 - (creation_time / world_time_seconds)` (`0x1446d24e8`
/// = `1.0f`, `CreationTime` at `[this + 0x9c]`, `World->TimeSeconds` at
/// `[world + 0x5a0]`) to the base priority.
pub fn evaluate_matinee_actor_net_priority(
    actor_inputs: &ActorPriorityInputs,
    has_open_channel: bool,
    creation_time: f32,
    world_time_seconds: f32,
) -> ActorPriorityOutcome {
    match evaluate_actor_net_priority(actor_inputs) {
        ActorPriorityOutcome::DelegateToOwner => ActorPriorityOutcome::DelegateToOwner,
        ActorPriorityOutcome::Computed(base_priority) => {
            if has_open_channel {
                ActorPriorityOutcome::Computed(base_priority)
            } else {
                let age_bonus = 1.0 - (creation_time / world_time_seconds);
                ActorPriorityOutcome::Computed(base_priority + age_bonus)
            }
        }
    }
}

/// Evaluate the per-frame client-connection tick budget and carryover delta in
/// `UNetDriver::ServerReplicateActors_PrepConnections` (`0x143723dc0` server /
/// `0x1439c0b20` client, F-748).
///
/// Returns `(num_clients_to_tick, updated_carryover_delta_seconds)`.
pub fn evaluate_prep_connections_tick_budget(
    client_connection_count: usize,
    throttle_enabled: bool,
    delta_seconds: f32,
    carryover_delta_seconds: f32,
    max_net_tick_rate: f32,
    lan_multiplier_two: bool,
    max_connections_to_tick_per_server_frame: i32,
) -> (usize, f32) {
    let mut num_clients_to_tick = client_connection_count;
    let mut updated_carryover = carryover_delta_seconds;
    if throttle_enabled {
        let combined_delta = delta_seconds + carryover_delta_seconds;
        let lan_multiplier = if lan_multiplier_two { 2.0_f32 } else { 1.0_f32 };
        let raw_budget = (combined_delta * max_net_tick_rate * lan_multiplier) as i32;
        let clamped_budget =
            raw_budget.min(i32::try_from(client_connection_count).unwrap_or(i32::MAX));
        if clamped_budget <= 0 {
            return (0, combined_delta);
        }
        updated_carryover = 0.0;
        num_clients_to_tick = clamped_budget as usize;
    }
    if max_connections_to_tick_per_server_frame > 0 {
        num_clients_to_tick =
            num_clients_to_tick.min(max_connections_to_tick_per_server_frame as usize);
    }
    (num_clients_to_tick, updated_carryover)
}

/// Evaluate whether a `UNetConnection` is ready for actor replication in
/// `UNetDriver::ServerReplicateActors_PrepConnections` (`0x143723dc0` server /
/// `0x1439c0b20` client, F-748).
///
/// Requires `Conn->ViewTarget != nullptr` (`[conn + 0x98]`),
/// `Conn->State == USOCK_Open (3)` (`DWORD [conn + 0x134] == 3`), and
/// `Driver->ElapsedTime - Conn->LastReceiveTime < 1.5` (`[driver + 0x218] -
/// [conn + 0x1d0] < 1.5`).
pub fn is_connection_ready_for_replication(
    has_view_target: bool,
    connection_state: u32,
    driver_elapsed_time: f64,
    last_receive_time: f64,
) -> bool {
    has_view_target
        && connection_state == 3
        && (driver_elapsed_time - last_receive_time) < PREP_CONNECTIONS_RECEIVE_TIMEOUT_SECONDS
}

/// Inputs for one `FNetworkObjectInfo` entry in
/// `UNetDriver::ServerReplicateActors_BuildConsiderList` (`0x1437235a0` server
/// / `0x1439c0530` client, F-748).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConsiderListActorInputs {
    /// `FNetworkObjectInfo::bPendingNetUpdate` (`([info + 0xd0] & 1) != 0`).
    pub b_pending_net_update: bool,
    /// `FNetworkObjectInfo::NextUpdateTime` (`[info + 0x10]`, `f64`).
    pub next_update_time: f64,
    /// `FNetworkObjectInfo::LastNetReplicateTime` (`[info + 0x18]`, `f64`).
    pub last_net_replicate_time: f64,
    /// `FNetworkObjectInfo::OptimalNetUpdateDelta` (`[info + 0x20]`, `f32`).
    pub optimal_net_update_delta: f32,
    /// `World->TimeSeconds` (`[world + 0x5a0]`, `f32`).
    pub world_time_seconds: f32,
    /// `Driver->ElapsedTime` (`[driver + 0x218]`, `f64`).
    pub driver_elapsed_time: f64,
    /// `Actor->bPendingKillPending` (`([actor + 0x5c] & 0x10) != 0`) or
    /// `GUObjectArray` `PendingKill` bit (`bit 29`).
    pub is_pending_kill: bool,
    /// `Actor->RemoteRole` (`BYTE [actor + 0x5f]`, `ROLE_None == 0`).
    pub remote_role: u8,
    /// `Actor->NetDriverName == Driver->NetDriverName` (`[actor + 0xe8] ==
    /// [driver + 0x190]`).
    pub net_driver_name_matches: bool,
    /// `Actor->bNetStartup` (`([actor + 0x5b] & 0x40) != 0`).
    pub b_net_startup: bool,
    /// `Actor->NetDormancy` (`BYTE [actor + 0xf1]`, `DORM_Initial == 4`).
    pub net_dormancy: u8,
    /// Whether the actor's `ULevel` is initialized (`0x14333fe20` server /
    /// `0x1435d5f20` client).
    pub level_initialized: bool,
    /// `Actor->NetUpdateFrequency` (`[actor + 0x108]`, `f32`).
    pub net_update_frequency: f32,
    /// `Actor->MinNetUpdateFrequency` (`[actor + 0x10c]`, `f32`, defaulted to
    /// `2.0` if `0.0`).
    pub min_net_update_frequency: f32,
    /// `bUseAdaptiveNetUpdateFrequency` (`net.UseAdaptiveNetUpdateFrequency > 0`
    /// at `[rbp + 0xc0]`).
    pub b_use_adaptive_net_update_frequency: bool,
    /// Random float in `[0.0, 1.0]` (`srands` at `0x141ef10e0` server) when
    /// scheduling `NextUpdateTime`.
    pub random_unit_float: f32,
    /// `ServerTickTime` (`CallPreReplicationDelta` at `[rsp + 0xd8]`).
    pub server_tick_time: f32,
}

/// Disposition and updated `FNetworkObjectInfo` fields for one active network
/// object in `UNetDriver::ServerReplicateActors_BuildConsiderList` (`0x1437235a0`
/// server / `0x1439c0530` client, F-748).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConsiderListActorOutcome {
    /// `!bPendingNetUpdate && (world_time_seconds as f64) <= next_update_time`.
    SkipNotReadyForUpdate,
    /// `is_pending_kill || remote_role == 0 || (b_net_startup && net_dormancy == 4 && level_initialized)`:
    /// appended to `ActorsToRemove` (`RemoveNetworkActor`).
    RemoveFromNetworkObjectList,
    /// `Actor->NetDriverName != Driver->NetDriverName`.
    SkipMismatchedNetDriverName,
    /// Appended to `OutConsiderList` with `bPendingNetUpdate` cleared to `false`
    /// (`[info + 0xd0] &= ~1`) and `Actor->CallPreReplication(Driver)` invoked.
    AddToConsiderListAndPreReplicate {
        next_update_time: f64,
        last_net_replicate_time: f64,
        optimal_net_update_delta: f32,
        updated_min_net_update_frequency: f32,
        updated_last_net_update_time: Option<f64>,
    },
}

/// Evaluate `UNetDriver::ServerReplicateActors_BuildConsiderList` (`0x1437235a0`
/// server / `0x1439c0530` client, F-748) for one `FNetworkObjectInfo` entry in
/// exact branch order.
pub fn evaluate_consider_list_actor(inputs: &ConsiderListActorInputs) -> ConsiderListActorOutcome {
    let world_time_f64 = f64::from(inputs.world_time_seconds);
    if !inputs.b_pending_net_update && world_time_f64 <= inputs.next_update_time {
        return ConsiderListActorOutcome::SkipNotReadyForUpdate;
    }
    if inputs.is_pending_kill || inputs.remote_role == 0 {
        return ConsiderListActorOutcome::RemoveFromNetworkObjectList;
    }
    if !inputs.net_driver_name_matches {
        return ConsiderListActorOutcome::SkipMismatchedNetDriverName;
    }
    if inputs.b_net_startup && inputs.net_dormancy == 4 && inputs.level_initialized {
        return ConsiderListActorOutcome::RemoveFromNetworkObjectList;
    }

    let mut last_net_replicate_time = inputs.last_net_replicate_time;
    let mut optimal_net_update_delta = inputs.optimal_net_update_delta;
    if last_net_replicate_time == 0.0 {
        last_net_replicate_time = world_time_f64;
        optimal_net_update_delta = 1.0 / inputs.net_update_frequency;
    }

    let mut min_net_update_frequency = inputs.min_net_update_frequency;
    let last_net_update_delta = (world_time_f64 - last_net_replicate_time) as f32;
    if last_net_update_delta > ADAPTIVE_NET_UPDATE_DELAY_THRESHOLD_SECONDS {
        if min_net_update_frequency == 0.0 {
            min_net_update_frequency = DEFAULT_MIN_NET_UPDATE_FREQUENCY;
        }
        let min_optimal_delta = 1.0 / inputs.net_update_frequency;
        let max_optimal_delta = (1.0 / min_net_update_frequency).max(min_optimal_delta);
        let alpha = ((last_net_update_delta - ADAPTIVE_NET_UPDATE_DELAY_THRESHOLD_SECONDS)
            * ADAPTIVE_NET_UPDATE_LERP_SCALE)
            .clamp(0.0, 1.0);
        optimal_net_update_delta =
            min_optimal_delta + (max_optimal_delta - min_optimal_delta) * alpha;
    }

    let mut next_update_time = inputs.next_update_time;
    let mut updated_last_net_update_time = None;
    if !inputs.b_pending_net_update {
        let next_update_delta = if inputs.b_use_adaptive_net_update_frequency {
            optimal_net_update_delta
        } else {
            1.0 / inputs.net_update_frequency
        };
        let jitter = inputs.random_unit_float * inputs.server_tick_time;
        next_update_time = f64::from(inputs.world_time_seconds + jitter + next_update_delta);
        updated_last_net_update_time = Some(inputs.driver_elapsed_time);
    }

    ConsiderListActorOutcome::AddToConsiderListAndPreReplicate {
        next_update_time,
        last_net_replicate_time,
        optimal_net_update_delta,
        updated_min_net_update_frequency: min_net_update_frequency,
        updated_last_net_update_time,
    }
}

/// Priority fixed-point scale multiplier (`65536.0f` at `0x144905214` server)
/// applied in `UNetDriver::ServerReplicateActors_PrioritizeActors`
/// (`0x1437241e0` server / `0x1439c0e70` client, F-749).
pub const PRIORITY_SCALE_MULTIPLIER: f32 = 65536.0;

/// Near squared-distance threshold (`250_000.0f` = `500.0^2` at `0x1449ca03c`
/// server) behind the viewer (`dot < 0.0`) for `DestroyedStartupOrDormantActors`
/// priority scaling in `UNetDriver::ServerReplicateActors_PrioritizeActors`
/// (F-749).
pub const DESTRUCTION_INFO_BEHIND_NEAR_DIST_SQ: f32 = 250_000.0;

/// Far squared-distance threshold (`4_000_000.0f` = `2000.0^2` at `0x14530fb94`
/// server) behind the viewer (`dot < 0.0`) for `DestroyedStartupOrDormantActors`
/// priority scaling in `UNetDriver::ServerReplicateActors_PrioritizeActors`
/// (F-749).
pub const DESTRUCTION_INFO_BEHIND_FAR_DIST_SQ: f32 = 4_000_000.0;

/// Far squared-distance threshold (`9_998_244.0f` = `3162.0^2` at `0x14530fb98`
/// server) in front of the viewer (`!(dot < 0.0)`) for
/// `DestroyedStartupOrDormantActors` priority scaling in
/// `UNetDriver::ServerReplicateActors_PrioritizeActors` (F-749).
pub const DESTRUCTION_INFO_FRONT_FAR_DIST_SQ: f32 = 9_998_244.0;

/// Near distance priority factor (`0.4f` at `0x1449075f0` server) applied to
/// `DestroyedStartupOrDormantActors` in
/// `UNetDriver::ServerReplicateActors_PrioritizeActors` (F-749).
pub const DESTRUCTION_INFO_NEAR_FACTOR: f32 = 0.4;

/// Far distance priority factor (`0.2f` at `0x14476f360` server) applied to
/// `DestroyedStartupOrDormantActors` in
/// `UNetDriver::ServerReplicateActors_PrioritizeActors` (F-749).
pub const DESTRUCTION_INFO_FAR_FACTOR: f32 = 0.2;

/// Fast-path relevancy grace window (`1.0` second at `0x14479ef90` server)
/// checked via `Driver->ElapsedTime - Channel->RelevantTime <= 1.0` in
/// `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors`
/// (`0x143725030` server / `0x1439c1bf0` client, F-749).
pub const PROCESS_PRIORITIZED_RELEVANCY_FAST_PATH_SECONDS: f64 = 1.0;

/// Jitter scale (`0.5` seconds at `0x1446dc8b8` server) multiplied by `srands()`
/// and added to `Driver->ElapsedTime` when updating `Channel->RelevantTime` in
/// `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors` (F-749).
pub const PROCESS_PRIORITIZED_RELEVANT_TIME_JITTER_SCALE: f64 = 0.5;

/// Low-frequency `NetUpdateFrequency` threshold (`1.0f` at `0x1446cdf18` server)
/// below which deferred actors without an open channel receive a short random
/// `NextUpdateTime` delay in
/// `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors` (F-749).
pub const PROCESS_PRIORITIZED_LOW_FREQ_THRESHOLD: f32 = 1.0;

/// Random delay scale (`0.2f` at `0x14476f360` server) multiplied by `srands()`
/// and added to `World->TimeSeconds` when deferring a low-frequency actor in
/// `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors` (F-749).
pub const PROCESS_PRIORITIZED_LOW_FREQ_DELAY_SCALE: f32 = 0.2;

fn cvttss2si_i32(value: f32) -> i32 {
    if !value.is_finite() || value < (i32::MIN as f32) || value >= 2_147_483_648.0 {
        i32::MIN
    } else {
        value as i32
    }
}

fn cvtss2si_i32(value: f32) -> i32 {
    let rounded = value.round_ties_even();
    if !rounded.is_finite() || rounded < (i32::MIN as f32) || rounded >= 2_147_483_648.0 {
        i32::MIN
    } else {
        rounded as i32
    }
}

/// Evaluate `appRoundToInt(65536.0f * priority)` (`0x14372482a..0x14372483b`
/// server / `0x1439c144a..0x1439c145a` client, `cvtss2si` at `0x143724837`,
/// F-749) in `UNetDriver::ServerReplicateActors_PrioritizeActors`.
pub fn round_net_priority_to_int(priority: f32) -> i32 {
    let scaled = priority * PRIORITY_SCALE_MULTIPLIER;
    let doubled_plus_half = scaled + scaled + 0.5_f32;
    let raw = cvtss2si_i32(doubled_plus_half);
    raw >> 1
}

/// Evaluate the `DestroyedStartupOrDormantActors` priority calculation
/// (`0x14372492f..0x143724f16` server, `cvttss2si` at `0x143724c2a` /
/// `0x1439c153f..0x1439c1b19` client, `cvttss2si` at `0x1439c17ca`,
/// F-749) in `UNetDriver::ServerReplicateActors_PrioritizeActors`.
pub fn evaluate_destruction_info_priority(
    last_send_time_delta: f32,
    destroyed_position: [f32; 3],
    viewer_location: [f32; 3],
    viewer_dir: [f32; 3],
) -> i32 {
    let dx = destroyed_position[0] - viewer_location[0];
    let dy = destroyed_position[1] - viewer_location[1];
    let dz = destroyed_position[2] - viewer_location[2];
    let dist_sq = dx * dx + dy * dy + dz * dz;
    let dot = viewer_dir[0] * dx + viewer_dir[1] * dy + viewer_dir[2] * dz;

    let mut time = last_send_time_delta;
    if dot < 0.0 {
        if dist_sq > DESTRUCTION_INFO_BEHIND_FAR_DIST_SQ {
            time *= DESTRUCTION_INFO_FAR_FACTOR;
        } else if dist_sq > DESTRUCTION_INFO_BEHIND_NEAR_DIST_SQ {
            time *= DESTRUCTION_INFO_NEAR_FACTOR;
        }
    } else if dist_sq > DESTRUCTION_INFO_FRONT_FAR_DIST_SQ {
        time *= DESTRUCTION_INFO_NEAR_FACTOR;
    }

    let scaled = time * PRIORITY_SCALE_MULTIPLIER;
    cvttss2si_i32(scaled)
}

/// Per-actor inputs for the consider-list loop in
/// `UNetDriver::ServerReplicateActors_PrioritizeActors` (`0x1437241e0..0x143724830`
/// server / `0x1439c0e70..0x1439c14b5` client, F-749).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrioritizeActorInputs {
    pub has_channel: bool,
    pub b_only_relevant_to_owner: bool,
    pub is_relevant_to_at_least_one_viewer: bool,
    pub driver_elapsed_time: f64,
    pub channel_relevant_time: f64,
    pub relevant_timeout: f64,
    pub actor_net_tag: i32,
    pub driver_net_tag: i32,
    pub is_actor_dormant_on_connection: bool,
    pub actor_net_dormancy: u8,
    pub channel_b_pending_dormancy: bool,
    pub channel_dormant: bool,
    pub get_net_dormancy_returns_dormant_partial: bool,
    pub level_initialized_for_actor: bool,
    pub channel_last_update_time: f64,
    pub channel_open_elapsed_time: f64,
    pub max_net_priority_across_viewers: f32,
}

/// Per-actor disposition in `UNetDriver::ServerReplicateActors_PrioritizeActors`
/// (`0x1437241e0..0x143724830` server / `0x1439c0e70..0x1439c14b5` client,
/// F-749).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PrioritizeActorOutcome {
    SkipNotRelevantToOwner,
    SkipAlreadyTagged,
    CloseChannelOwnerIrrelevantTimeout {
        close_reason: u8,
    },
    SkipOwnerIrrelevantWithinTimeout,
    SkipDormant,
    StartBecomingDormantAndSkip,
    SkipLevelNotInitialized,
    Prioritized {
        new_actor_net_tag: i32,
        time_since_last_update: f32,
        priority: i32,
    },
}

/// Evaluate `UNetDriver::ServerReplicateActors_PrioritizeActors`
/// (`0x1437241e0..0x143724830` server / `0x1439c0e70..0x1439c14b5` client,
/// F-749) for one `FNetworkObjectInfo` entry in exact branch order.
pub fn evaluate_prioritize_actor(inputs: &PrioritizeActorInputs) -> PrioritizeActorOutcome {
    if !inputs.has_channel
        && inputs.b_only_relevant_to_owner
        && !inputs.is_relevant_to_at_least_one_viewer
    {
        return PrioritizeActorOutcome::SkipNotRelevantToOwner;
    }
    if inputs.actor_net_tag == inputs.driver_net_tag {
        return PrioritizeActorOutcome::SkipAlreadyTagged;
    }
    if inputs.has_channel
        && inputs.b_only_relevant_to_owner
        && !inputs.is_relevant_to_at_least_one_viewer
    {
        if inputs.driver_elapsed_time - inputs.channel_relevant_time > inputs.relevant_timeout {
            return PrioritizeActorOutcome::CloseChannelOwnerIrrelevantTimeout { close_reason: 3 };
        }
        return PrioritizeActorOutcome::SkipOwnerIrrelevantWithinTimeout;
    }
    if inputs.is_actor_dormant_on_connection {
        return PrioritizeActorOutcome::SkipDormant;
    }
    if inputs.actor_net_dormancy > 1
        && inputs.has_channel
        && !inputs.channel_b_pending_dormancy
        && !inputs.channel_dormant
        && (inputs.actor_net_dormancy == 2
            || (inputs.actor_net_dormancy == 3 && inputs.get_net_dormancy_returns_dormant_partial))
    {
        return PrioritizeActorOutcome::StartBecomingDormantAndSkip;
    }
    if !inputs.level_initialized_for_actor {
        return PrioritizeActorOutcome::SkipLevelNotInitialized;
    }
    // `0x1437246a6..0x1437247c1` server / `0x1439c12b2..0x1439c13c6` client:
    // branches strictly on `test r14, r14` (`Channel != nullptr`) and subtracts
    // `Channel->LastUpdateTime` (`[r14+0x80]`) from `Driver->ElapsedTime` (`[rax+0x218]`).
    let time_since_last_update = if !inputs.has_channel {
        inputs.channel_open_elapsed_time as f32
    } else {
        (inputs.driver_elapsed_time - inputs.channel_last_update_time) as f32
    };
    let priority = round_net_priority_to_int(inputs.max_net_priority_across_viewers);
    PrioritizeActorOutcome::Prioritized {
        new_actor_net_tag: inputs.driver_net_tag,
        time_since_last_update,
        priority,
    }
}

/// Per-entry inputs for `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors`
/// (`0x143725030..0x143725cf0` server / `0x1439c1bf0..0x1439c24a6` client,
/// F-749).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessPrioritizedActorInputs {
    pub connection_is_net_ready: bool,
    pub is_destruction_info: bool,
    pub destruction_level_initialized: bool,
    pub has_channel: bool,
    pub driver_elapsed_time: f64,
    pub channel_relevant_time: f64,
    pub relevant_timeout: f64,
    pub is_relevant_to_connection: bool,
    pub actor_b_tear_off: bool,
    pub force_relevant_frame: i32,
    pub driver_replication_frame: i32,
    pub channel_b_is_replicating_actor: bool,
    pub actor_b_always_relevant: bool,
    pub actor_b_net_temporary: bool,
    pub channel_open_acked: bool,
    pub actor_level_initialized: bool,
    pub package_map_supports_object: bool,
    pub actor_net_update_frequency: f32,
    pub world_time_seconds: f32,
    pub random_unit_float: f32,
    pub can_create_actor_channel: bool,
    pub replicate_actor_wrote_anything: bool,
    pub connection_is_net_ready_after_replicate: bool,
    pub actor_is_net_startup: bool,
}

/// Per-entry disposition in
/// `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors`
/// (`0x143725030..0x143725cf0` server / `0x1439c1bf0..0x1439c24a6` client,
/// F-749).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProcessPrioritizedActorOutcome {
    StopConnectionSaturated,
    SendDestructionInfo,
    SkipDestructionInfoLevelUnloaded,
    SkipAlreadyReplicatingOrAckedNetTemporary {
        updated_relevant_time: Option<f64>,
    },
    DeferCannotOpenChannel {
        updated_next_update_time: Option<f32>,
    },
    DeferChannelCreationFailed,
    ReplicateActorAndStopSaturated {
        opened_new_channel: bool,
        wrote_anything: bool,
        updated_relevant_time: Option<f64>,
    },
    ReplicateActorContinue {
        opened_new_channel: bool,
        wrote_anything: bool,
        updated_relevant_time: Option<f64>,
    },
    CloseChannel {
        close_reason: u8,
    },
    SkipNotRelevantNoClose,
}

/// Evaluate `UNetDriver::ServerReplicateActors_ProcessPrioritizedActors`
/// (`0x143725030..0x143725cf0` server / `0x1439c1bf0..0x1439c24a6` client,
/// F-749) for one `FActorPriority` entry in exact branch order.
pub fn evaluate_process_prioritized_actor(
    inputs: &ProcessPrioritizedActorInputs,
) -> ProcessPrioritizedActorOutcome {
    if !inputs.connection_is_net_ready {
        return ProcessPrioritizedActorOutcome::StopConnectionSaturated;
    }
    if inputs.is_destruction_info {
        if inputs.destruction_level_initialized {
            return ProcessPrioritizedActorOutcome::SendDestructionInfo;
        }
        return ProcessPrioritizedActorOutcome::SkipDestructionInfoLevelUnloaded;
    }

    let relevancy_fast_path = inputs.has_channel
        && (inputs.driver_elapsed_time - inputs.channel_relevant_time
            <= PROCESS_PRIORITIZED_RELEVANCY_FAST_PATH_SECONDS);
    let is_relevant =
        !inputs.actor_b_tear_off && (relevancy_fast_path || inputs.is_relevant_to_connection);
    let is_recently_relevant = is_relevant
        || (inputs.has_channel
            && inputs.driver_elapsed_time - inputs.channel_relevant_time < inputs.relevant_timeout)
        || (inputs.force_relevant_frame >= inputs.driver_replication_frame);

    if is_recently_relevant {
        let jittered_relevant_time = inputs.driver_elapsed_time
            + PROCESS_PRIORITIZED_RELEVANT_TIME_JITTER_SCALE * f64::from(inputs.random_unit_float);
        let updated_relevant_time = if inputs.has_channel && is_relevant {
            Some(jittered_relevant_time)
        } else {
            None
        };

        if (inputs.has_channel && inputs.channel_b_is_replicating_actor)
            || (inputs.actor_b_always_relevant
                && inputs.actor_b_net_temporary
                && inputs.has_channel
                && inputs.channel_open_acked)
        {
            return ProcessPrioritizedActorOutcome::SkipAlreadyReplicatingOrAckedNetTemporary {
                updated_relevant_time,
            };
        }

        if !inputs.has_channel {
            if !inputs.actor_level_initialized || !inputs.package_map_supports_object {
                let updated_next_update_time = if inputs.actor_net_update_frequency
                    < PROCESS_PRIORITIZED_LOW_FREQ_THRESHOLD
                {
                    Some(
                        inputs.world_time_seconds
                            + PROCESS_PRIORITIZED_LOW_FREQ_DELAY_SCALE * inputs.random_unit_float,
                    )
                } else {
                    None
                };
                return ProcessPrioritizedActorOutcome::DeferCannotOpenChannel {
                    updated_next_update_time,
                };
            }
            if !inputs.can_create_actor_channel {
                return ProcessPrioritizedActorOutcome::DeferChannelCreationFailed;
            }
        }

        let final_relevant_time = if !inputs.has_channel || is_relevant {
            Some(jittered_relevant_time)
        } else {
            None
        };

        if !inputs.connection_is_net_ready_after_replicate {
            ProcessPrioritizedActorOutcome::ReplicateActorAndStopSaturated {
                opened_new_channel: !inputs.has_channel,
                wrote_anything: inputs.replicate_actor_wrote_anything,
                updated_relevant_time: final_relevant_time,
            }
        } else {
            ProcessPrioritizedActorOutcome::ReplicateActorContinue {
                opened_new_channel: !inputs.has_channel,
                wrote_anything: inputs.replicate_actor_wrote_anything,
                updated_relevant_time: final_relevant_time,
            }
        }
    } else if inputs.has_channel
        && (!inputs.actor_level_initialized || !inputs.actor_is_net_startup)
    {
        ProcessPrioritizedActorOutcome::CloseChannel {
            close_reason: if inputs.actor_b_tear_off { 4 } else { 3 },
        }
    } else {
        ProcessPrioritizedActorOutcome::SkipNotRelevantNoClose
    }
}

/// Evaluate `UNetDriver::ServerReplicateActors_MarkRelevantActors`
/// (`0x143723ce0` server / `0x1439c0a30` client, F-749).
///
/// Returns `(updated_force_relevant_frame, updated_b_pending_net_update)`.
pub fn evaluate_mark_relevant_actor(
    has_actor_owner: bool,
    has_open_channel: bool,
    connection_tick_count: i32,
    current_force_relevant_frame: i32,
    current_b_pending_net_update: bool,
) -> (i32, bool) {
    if has_actor_owner && !has_open_channel {
        (connection_tick_count.wrapping_add(1), true)
    } else {
        (current_force_relevant_frame, current_b_pending_net_update)
    }
}

/// CPU-saturation multiplier (`1.2f32`, `0x3f99999a` at `0x1446d2508` server) in
/// `UNetDriver::ServerReplicateActors` (`0x1437221b0` server / `0x1439bf100` client).
pub const SERVER_REPLICATE_ACTORS_CPU_SATURATION_FACTOR: f32 = 1.2;

/// Maximum reliable output record count (`RELIABLE_BUFFER - 1 = 255`, `0xff`)
/// before `UChannel::IsNetReady` (`0x1435327b0` server / `0x1437bd690` client)
/// reports the channel saturated.
pub const CHANNEL_RELIABLE_BUFFER_NET_READY_LIMIT: i32 = 255;

/// Inputs for `UNetConnection::IsNetReady` (`0x1436eb380` server / `0x143991d80`
/// client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConnectionNetReadyInputs {
    /// `Saturate` (`dl != 0`): when `true`, resets `QueuedBits = -SendBuffer.GetNumBits()`.
    pub saturate: bool,
    /// `QueuedBits` (`[Connection + 0x1f8]`).
    pub queued_bits: i32,
    /// `SendBuffer.GetNumBits()` (`[Connection + 0x5e0]`).
    pub send_buffer_num_bits: i64,
    /// `bUseAdaptiveNetFrequency` / packet-budget mode (`BYTE [Connection + 0x1ba0] != 0`).
    pub b_use_adaptive_net_frequency: bool,
    /// `[Connection + 0x1b7c]`.
    pub adaptive_queued_bytes: i32,
    /// `[Connection + 0x1b88]`.
    pub adaptive_max_bytes_per_sec: f64,
    /// `[Connection + 0x1b90]`.
    pub adaptive_delta_time: f64,
    /// `[Connection + 0x1b48]`.
    pub adaptive_out_packet_id: u32,
    /// `[Connection + 0x1b4c]`.
    pub adaptive_out_ack_packet_id: u32,
    /// `[Connection + 0x1b98]`.
    pub adaptive_max_in_flight_packets: u32,
}

/// Evaluates `UNetConnection::IsNetReady` (`0x1436eb380` server / `0x143991d80`
/// client), returning `(is_net_ready, updated_queued_bits)`.
pub fn evaluate_connection_is_net_ready(inputs: &ConnectionNetReadyInputs) -> (bool, i32) {
    let updated_queued_bits = if inputs.saturate {
        -(inputs.send_buffer_num_bits as i32)
    } else {
        inputs.queued_bits
    };
    if inputs.b_use_adaptive_net_frequency {
        let byte_budget = inputs.adaptive_max_bytes_per_sec * inputs.adaptive_delta_time;
        let in_flight = inputs
            .adaptive_out_packet_id
            .wrapping_sub(inputs.adaptive_out_ack_packet_id);
        let ready = f64::from(inputs.adaptive_queued_bytes) < byte_budget
            && in_flight < inputs.adaptive_max_in_flight_packets;
        (ready, updated_queued_bits)
    } else {
        let total_bits = i64::from(updated_queued_bits).saturating_add(inputs.send_buffer_num_bits);
        (total_bits <= 0, updated_queued_bits)
    }
}

/// Evaluates `UChannel::IsNetReady` (`0x1435327b0` server / `0x1437bd690`
/// client), which rejects `NumOutRec >= 255` (`0xff`) and otherwise tail-calls
/// `Connection->IsNetReady(Saturate)`.
pub fn evaluate_channel_is_net_ready(
    num_out_rec: i32,
    connection_inputs: &ConnectionNetReadyInputs,
) -> (bool, i32) {
    if num_out_rec >= CHANNEL_RELIABLE_BUFFER_NET_READY_LIMIT {
        return (false, connection_inputs.queued_bits);
    }
    evaluate_connection_is_net_ready(connection_inputs)
}

/// Inputs for the inlined `ServerReplicateActors_MarkRelevantActors` loop inside
/// `UNetDriver::ServerReplicateActors` (`0x143722a74..0x143722c40` server /
/// `0x1439bfa04..0x1439bfbd0` client) over unprocessed prioritized actors
/// `PriorityActors[LastProcessedActor..FinalSortedCount]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkRelevantActorStepInputs {
    pub actor_info_present: bool,
    pub has_channel: bool,
    pub driver_elapsed_time: f64,
    pub channel_relevant_time: f64,
    pub is_relevant_to_any_viewer: bool,
    /// Uniform random float in `[0.0, 1.0)` from `FRand()`.
    pub random_unit_float: f32,
    pub current_b_pending_net_update: bool,
    pub current_force_relevant_frame: i32,
    pub connection_last_processed_frame: i32,
    pub driver_replication_frame: i32,
}

/// Outcome of one iteration in the inlined `ServerReplicateActors_MarkRelevantActors`
/// loop (`0x143722a74..0x143722c40` server / `0x1439bfa04..0x1439bfbd0` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkRelevantActorStepOutcome {
    pub b_pending_net_update: bool,
    pub updated_channel_relevant_time: Option<f64>,
    pub force_relevant_frame: i32,
}

/// Evaluates one unprocessed `FActorPriority` entry (`k in LastProcessedActor..FinalSortedCount`)
/// in `UNetDriver::ServerReplicateActors`'s inlined `MarkRelevantActors` loop
/// (`0x143722a74..0x143722c40` server / `0x1439bfa04..0x1439bfbd0` client).
pub fn evaluate_mark_relevant_actor_step(
    inputs: &MarkRelevantActorStepInputs,
) -> MarkRelevantActorStepOutcome {
    if !inputs.actor_info_present {
        return MarkRelevantActorStepOutcome {
            b_pending_net_update: inputs.current_b_pending_net_update,
            updated_channel_relevant_time: None,
            force_relevant_frame: inputs.current_force_relevant_frame,
        };
    }
    let recent_channel_fast_path = inputs.has_channel
        && (inputs.driver_elapsed_time - inputs.channel_relevant_time
            <= PROCESS_PRIORITIZED_RELEVANCY_FAST_PATH_SECONDS);
    let (b_pending_net_update, updated_channel_relevant_time) = if recent_channel_fast_path {
        (true, None)
    } else if inputs.is_relevant_to_any_viewer {
        let new_rel_time = if inputs.has_channel {
            Some(inputs.driver_elapsed_time + 0.5 * f64::from(1.0_f32 + inputs.random_unit_float))
        } else {
            None
        };
        (true, new_rel_time)
    } else {
        (inputs.current_b_pending_net_update, None)
    };
    let force_relevant_frame =
        if inputs.current_force_relevant_frame >= inputs.connection_last_processed_frame {
            inputs.driver_replication_frame.wrapping_add(1)
        } else {
            inputs.current_force_relevant_frame
        };
    MarkRelevantActorStepOutcome {
        b_pending_net_update,
        updated_channel_relevant_time,
        force_relevant_frame,
    }
}

/// Frame-level inputs for `UNetDriver::ServerReplicateActors`
/// (`0x1437221b0` server / `0x1439bf100` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerReplicateActorsFrameInputs {
    pub num_client_connections: i32,
    pub has_replication_driver: bool,
    pub replication_driver_updated_count: i32,
    pub current_replication_frame: i32,
    pub has_world: bool,
    pub num_clients_to_tick: i32,
    pub delta_seconds: f32,
    /// `Engine->GetMaxTickRate(DeltaSeconds, false)`: `ServerTickTime = (MaxTickRate <= 0.0) ? 0.0 : 1.0 / MaxTickRate`.
    pub max_tick_rate: f32,
}

/// Frame-level disposition of `UNetDriver::ServerReplicateActors`
/// (`0x1437221b0` server / `0x1439bf100` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ServerReplicateActorsFrameOutcome {
    /// `ClientConnections.Num() == 0` (`[NetDriver + 0x98] == 0`): returns `0`
    /// before incrementing `ReplicationFrame`.
    NoClientConnections,
    /// `ReplicationDriver` (`[NetDriver + 0x700]`) is non-null: delegates to
    /// `ReplicationDriver->ServerReplicateActors(DeltaSeconds)`.
    DelegatedToReplicationDriver { updated: i32 },
    /// Incremented `ReplicationFrame` (`+0x420`), then returned `0` because
    /// `World == nullptr` (`+0x140`) or `PrepConnections` (`0x143723dc0` /
    /// `0x1439c0b20`) returned `0`.
    AbortedAfterFrameIncrement { next_replication_frame: i32 },
    /// Runs `BuildConsiderList` (`0x1437235a0` / `0x1439c0530`) and the
    /// per-connection `PrioritizeActors` -> `ProcessPrioritizedActors` ->
    /// `MarkRelevantActors` loop, then rotates `ClientConnections` left by
    /// `rotate_connections_left_by` when `0 < num_clients_to_tick < num_client_connections`.
    RunConnectionReplication {
        next_replication_frame: i32,
        server_tick_time: f32,
        clamped_delta_seconds: f32,
        b_cpu_saturated: bool,
        rotate_connections_left_by: i32,
    },
}

/// Evaluates the top-level frame setup and post-loop connection rotation of
/// `UNetDriver::ServerReplicateActors` (`0x1437221b0` server / `0x1439bf100`
/// client).
pub fn evaluate_server_replicate_actors_frame(
    inputs: &ServerReplicateActorsFrameInputs,
) -> ServerReplicateActorsFrameOutcome {
    if inputs.num_client_connections <= 0 {
        return ServerReplicateActorsFrameOutcome::NoClientConnections;
    }
    if inputs.has_replication_driver {
        return ServerReplicateActorsFrameOutcome::DelegatedToReplicationDriver {
            updated: inputs.replication_driver_updated_count,
        };
    }
    let next_replication_frame = inputs.current_replication_frame.wrapping_add(1);
    if !inputs.has_world || inputs.num_clients_to_tick <= 0 {
        return ServerReplicateActorsFrameOutcome::AbortedAfterFrameIncrement {
            next_replication_frame,
        };
    }
    let server_tick_time = if inputs.max_tick_rate <= 0.0 {
        0.0
    } else {
        1.0 / inputs.max_tick_rate
    };
    let (clamped_delta_seconds, b_cpu_saturated) = if server_tick_time > 0.0 {
        (
            server_tick_time,
            inputs.delta_seconds > SERVER_REPLICATE_ACTORS_CPU_SATURATION_FACTOR * server_tick_time,
        )
    } else {
        (inputs.delta_seconds, false)
    };
    let rotate_connections_left_by = if inputs.num_clients_to_tick > 0
        && inputs.num_clients_to_tick < inputs.num_client_connections
    {
        inputs.num_clients_to_tick
    } else {
        0
    };
    ServerReplicateActorsFrameOutcome::RunConnectionReplication {
        next_replication_frame,
        server_tick_time,
        clamped_delta_seconds,
        b_cpu_saturated,
        rotate_connections_left_by,
    }
}

/// Computed `FReplicationFlags` and role-handling plan inside
/// `UActorChannel::ReplicateActor` (`0x14353a820` server / `0x1437c7f00` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplicateActorRepFlags {
    /// Bit 0 (`0x1`): `NetOwner == Connection` or `NetOwner->NetConnection == Connection`.
    pub b_net_owner: bool,
    /// Bit 1 (`0x2`): initial bunch (`OpenPacketId.First == -1` or `ResendAllDataState != 0`
    /// unless `ResendAllDataState == 2 && bSpawnedForReplay`).
    pub b_net_initial: bool,
    /// Bit 2 (`0x4`): `Actor->RemoteRole == ROLE_SimulatedProxy (1)` after any non-owner
    /// autonomous-proxy downgrade.
    pub b_net_simulated: bool,
    /// Bit 3 (`0x8`): `Actor->ReplicatedMovement.bRepPhysics` (`([Actor + 0x90] & 2) != 0`).
    pub b_rep_physics: bool,
    /// Bit 4 (`0x10`): `Connection` is a replay connection (`([Connection + 0xa4] >> 2) & 1 != 0`).
    pub b_replay: bool,
    /// Packed `u32` value matching `[rsp + 0x58]` in `UActorChannel::ReplicateActor`.
    pub raw_bits: u32,
    /// Whether `PackageMap->SerializeNewActor` (`vtable[+0x290]`) and
    /// `Actor->OnSerializeNewActor` (`vtable[+0x3f8]`) are invoked (`b_net_initial && channel_spawn_acked`).
    pub serialize_new_actor: bool,
    /// Whether `Actor->RemoteRole == ROLE_AutonomousProxy (2)` is temporarily downgraded
    /// to `ROLE_SimulatedProxy (1)` during property replication because `!b_net_owner`,
    /// then restored at exit (`0x14353b0b1..0x14353b0cd` and `0x14353bbbb..0x14353bbed`).
    pub downgraded_autonomous_proxy_for_non_owner: bool,
}

/// Inputs for `UActorChannel::ReplicateActor` (`0x14353a820` server /
/// `0x1437c7f00` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplicateActorInputs {
    /// Bit 8 of `[Channel + 0x30]` (`Closing`).
    pub channel_closing: bool,
    /// `BYTE [Connection + 0x1508]` (`ResendAllDataState`: `0 = None`, `1 = Resending`, `2 = ReplayCheckpoint`).
    pub connection_resend_all_data_state: u8,
    /// Bit 2 (`0x4`) of `[Channel + 0x88]` (`bIsReplicatingActor`).
    pub channel_b_is_replicating_actor: bool,
    /// Bit 5 (`0x20`) of `[Channel + 0x88]` (`bActorIsPendingKill`).
    pub channel_b_actor_is_pending_kill: bool,
    /// `GUObjectArray` item flags `& 0x30000000 != 0` (`PendingKill | Unreachable`).
    pub actor_uobject_pending_kill_or_unreachable: bool,
    /// `OpenPacketId.First == -1` (`[Channel + 0x38] == -1`).
    pub open_packet_id_first_is_none: bool,
    /// Whether all required level/component prerequisites (`[World + 0x350..0x358]`) are ready.
    pub all_level_components_ready: bool,
    /// Return of `this->vtable[+0x300]()` when `!open_packet_id_first_is_none`.
    pub channel_clean_up_or_has_bunch_check: bool,
    /// `FOutBunch::IsError()` (`BYTE [Bunch + 0x29] & 1 != 0`).
    pub out_bunch_is_error: bool,
    /// Bit 13 (`0x2000`) of `[Channel + 0x30]` (`bSpawnedForReplay`, `(Flags30 >> 12) & 2 != 0`).
    pub channel_b_spawned_for_replay: bool,
    /// Bit 12 (`0x1000`) of `[Channel + 0x30]` (`SpawnAcked`).
    pub channel_spawn_acked: bool,
    /// Whether `Actor->GetNetOwner()` (`vtable[+0x4b0]`) resolves to `Connection`.
    pub actor_owned_by_connection: bool,
    /// `BYTE [Actor + 0x5f]` (`Actor->RemoteRole`: `1 = SimulatedProxy`, `2 = AutonomousProxy`).
    pub actor_remote_role: u8,
    /// `([Actor + 0x90] & 2) != 0` (`ReplicatedMovement.bRepPhysics`).
    pub actor_b_rep_physics: bool,
    /// `([Connection + 0xa4] >> 2) & 1 != 0`.
    pub connection_is_replay: bool,
    /// Whether `ActorReplicator->ReplicateProperties` (`0x14356c9a0` / `0x1437f6b80`) wrote bits.
    pub actor_properties_wrote: bool,
    /// Whether `Actor->ReplicateSubobjects` (`vtable[+0x2b8]`) wrote bits.
    pub subobjects_wrote: bool,
    /// Whether stale subobject deletion headers wrote bits (`ResendAllDataState == 0`).
    pub deleted_subobjects_wrote: bool,
    /// Total bits in `Bunch` sent via `SendBunch` (`vtable[+0x2c8]`).
    pub bunch_bits_written: i64,
    /// Whether `Bunch.bPartial` (`BYTE [Bunch + 0xf4] & 1` or `[Actor + 0x58] & 1` partial retry) is set.
    pub bunch_is_partial_initial: bool,
    /// `NetDriver->ElapsedTime` (`[NetDriver + 0x218]`).
    pub driver_elapsed_time: f64,
}

/// Outcome of `UActorChannel::ReplicateActor` (`0x14353a820` server /
/// `0x1437c7f00` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReplicateActorOutcome {
    /// `Closing != 0 && ResendAllDataState == 0` (`returns 0`).
    EarlyExitChannelClosing,
    /// `bIsReplicatingActor != 0` (`Flags88 & 0x4`, logs recursion warning, `returns 0`).
    EarlyExitRecursiveReplication,
    /// `bActorIsPendingKill != 0` (`Flags88 & 0x20`, `returns 0`).
    EarlyExitAlreadyPendingKill,
    /// `GUObjectArray` item has `RF_PendingKill | RF_Unreachable` (`0x30000000`): sets
    /// `bActorIsPendingKill = 1` (`Flags88 |= 0x20`), releases `ActorReplicator`
    /// (`[this + 0x90]`), and `returns 0`.
    EarlyExitMarkActorPendingKillAndReleaseReplicator,
    /// Channel already opened (`OpenPacketId.First != -1`), level/component check passed,
    /// and `vtable[+0x300]()` returned `true` (`returns 0`).
    EarlyExitNothingToReplicatePreBunch,
    /// `FOutBunch(this, 0)` constructed in error state (`returns 0`).
    EarlyExitOutBunchError,
    /// Completed replication pass: sends bunch iff `sent_bunch` is `true`, updates
    /// `LastUpdateTime = NetDriver->ElapsedTime` (`[this + 0x80]`), clears
    /// `bForceCompareProperties` (`Flags88 &= ~2`), restores `RemoteRole` if downgraded,
    /// and returns `bits_written`.
    Completed {
        rep_flags: ReplicateActorRepFlags,
        sent_bunch: bool,
        bits_written: i64,
        updated_last_update_time: f64,
        clear_b_force_compare_properties: bool,
        queue_partial_initial_retry: bool,
    },
}

/// Evaluates `UActorChannel::ReplicateActor` (`0x14353a820` server /
/// `0x1437c7f00` client) in exact branch order.
pub fn evaluate_actor_channel_replicate_actor(
    inputs: &ReplicateActorInputs,
) -> ReplicateActorOutcome {
    if inputs.channel_closing && inputs.connection_resend_all_data_state == 0 {
        return ReplicateActorOutcome::EarlyExitChannelClosing;
    }
    if inputs.channel_b_is_replicating_actor {
        return ReplicateActorOutcome::EarlyExitRecursiveReplication;
    }
    if inputs.channel_b_actor_is_pending_kill {
        return ReplicateActorOutcome::EarlyExitAlreadyPendingKill;
    }
    if inputs.actor_uobject_pending_kill_or_unreachable {
        return ReplicateActorOutcome::EarlyExitMarkActorPendingKillAndReleaseReplicator;
    }
    if !inputs.open_packet_id_first_is_none
        && inputs.all_level_components_ready
        && inputs.channel_clean_up_or_has_bunch_check
    {
        return ReplicateActorOutcome::EarlyExitNothingToReplicatePreBunch;
    }
    if inputs.out_bunch_is_error {
        return ReplicateActorOutcome::EarlyExitOutBunchError;
    }

    let b_net_initial =
        if inputs.open_packet_id_first_is_none || inputs.connection_resend_all_data_state != 0 {
            !(inputs.connection_resend_all_data_state == 2 && inputs.channel_b_spawned_for_replay)
        } else {
            false
        };
    let b_net_owner = inputs.actor_owned_by_connection;
    let serialize_new_actor = b_net_initial && inputs.channel_spawn_acked;
    let downgraded_autonomous_proxy_for_non_owner = inputs.actor_remote_role == 2 && !b_net_owner;
    let effective_remote_role = if downgraded_autonomous_proxy_for_non_owner {
        1
    } else {
        inputs.actor_remote_role
    };
    let b_net_simulated = effective_remote_role == 1;
    let b_rep_physics = inputs.actor_b_rep_physics;
    let b_replay = inputs.connection_is_replay;
    let raw_bits = u32::from(b_net_owner)
        | (u32::from(b_net_initial) << 1)
        | (u32::from(b_net_simulated) << 2)
        | (u32::from(b_rep_physics) << 3)
        | (u32::from(b_replay) << 4);

    let rep_flags = ReplicateActorRepFlags {
        b_net_owner,
        b_net_initial,
        b_net_simulated,
        b_rep_physics,
        b_replay,
        raw_bits,
        serialize_new_actor,
        downgraded_autonomous_proxy_for_non_owner,
    };

    let wrote_anything = serialize_new_actor
        || inputs.actor_properties_wrote
        || inputs.subobjects_wrote
        || (inputs.connection_resend_all_data_state == 0 && inputs.deleted_subobjects_wrote);
    let bits_written = if wrote_anything {
        inputs.bunch_bits_written
    } else {
        0
    };

    ReplicateActorOutcome::Completed {
        rep_flags,
        sent_bunch: wrote_anything,
        bits_written,
        updated_last_update_time: inputs.driver_elapsed_time,
        clear_b_force_compare_properties: true,
        queue_partial_initial_retry: inputs.bunch_is_partial_initial,
    }
}
