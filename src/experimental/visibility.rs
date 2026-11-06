//! Conditional inbound visibility custom-struct decoding, with inert staging.
//!
//! This is a supported subset for server 26313077 / engine 17 / game 0, not
//! evidence that an original connection installed this profile. The caller must
//! qualify the normal (non-backcompat), loading, error-free, little-endian
//! reader, its null-map or qualified package-map name route, and custom struct
//! dispatch. Neither payload contents nor native struct size establish them.
//!
//! Input starts at the custom serializer's reuse bit. Outer RPC property
//! default-presence framing must already have been removed under a separately
//! qualified layout; this module does not guess those bits. Exact consumption,
//! canonical packed integers and strict strings are authored admission rules.
//! No captured payload replay or byte compatibility is claimed.

use anyhow::{Context, Result, ensure};

use super::actor_channel::read_packed;
use super::archive::{ByteOrder, MAX_STRING_UNITS, StringEncoding, read_string_frame};
use super::control::TokenEncoding;
use super::control_messages::ArchiveText;
use super::packet::{BitReader, MAX_HARDCODED_NAME_INDEX, Payload};
use super::release::{DEDICATED_SERVER_APP_ID, ReleaseProfile};
use super::rep_layout::{NetworkName, read_field, read_u32};

/// Caller assertions about the archive and custom dispatch, never detected
/// from payload bits. Both supported variants require normal RPC traversal,
/// the concrete loading FNetBitReader route, no byte swapping and the named
/// visibility custom serializer. Other reader/settings/map/dispatch forms
/// must remain unqualified, including backcompat and replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibilityArchiveProfile {
    Unqualified,
    NormalLittleEndianNullMapCustom,
    NormalLittleEndianPackageMapCustom,
}

/// Explicit qualification token. No default or deserializer enables it.
#[derive(Clone, Copy, Debug)]
pub struct VisibilityDecodeProfile {
    archive: VisibilityArchiveProfile,
}

impl VisibilityDecodeProfile {
    /// Select the supported profile using established release metadata and
    /// an explicit assertion of the archive/dispatch predicates above. This
    /// validates metadata, not the original runtime's installation or state.
    pub fn qualify(release: &ReleaseProfile, archive: VisibilityArchiveProfile) -> Result<Self> {
        ensure!(
            release.pin.app_id == DEDICATED_SERVER_APP_ID
                && release.pin.build_id == 26_313_077
                && release.engine_network_version == 17
                && release.game_network_version == 0,
            "unsupported visibility release/version profile"
        );
        ensure!(
            archive != VisibilityArchiveProfile::Unqualified,
            "unqualified visibility archive/custom dispatch unsupported"
        );
        Ok(Self { archive })
    }
}

/// Complete custom value, preserving the reuse bit and raw name forms.
/// Hardcoded indices are unresolved; string number bits are not interpreted
/// as suffixes or interned identities. Debug exposes no name contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedVisibility {
    pub reuse_package_name: bool,
    pub visible: bool,
    pub package_name: NetworkName,
    pub file_name: NetworkName,
    /// Exact custom payload length, excluding outer RPC presence framing.
    pub payload_bits: usize,
}

/// Parse all remaining reader bits as one custom value, at any bit cursor.
/// No alignment or padding is consumed. On error the cursor may have advanced;
/// no decoded value or visibility effect is published.
pub fn read_visibility(
    reader: &mut BitReader<'_>,
    profile: &VisibilityDecodeProfile,
) -> Result<DecodedVisibility> {
    // The private token can only be created by qualify. Keep this explicit
    // match so extending the archive enum cannot silently enable a route.
    ensure!(
        matches!(
            profile.archive,
            VisibilityArchiveProfile::NormalLittleEndianNullMapCustom
                | VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom
        ),
        "unqualified visibility archive/custom dispatch unsupported"
    );
    ensure!(!reader.is_error(), "visibility reader already in error");
    let payload_bits = reader.remaining();
    let reuse_package_name = read_field(reader, 1, "visibility reuse bit")? != 0;
    let visible = read_field(reader, 1, "visibility visible bit")? != 0;
    let package_name = read_name(reader).context("visibility package name")?;
    let file_name = if reuse_package_name {
        package_name.clone()
    } else {
        read_name(reader).context("visibility file name")?
    };
    ensure!(reader.at_end(), "trailing visibility custom payload bits");
    Ok(DecodedVisibility {
        reuse_package_name,
        visible,
        package_name,
        file_name,
        payload_bits,
    })
}

/// Decode an already isolated custom payload with its exact supplied bit length.
pub fn parse_visibility(
    payload: &Payload,
    profile: &VisibilityDecodeProfile,
) -> Result<DecodedVisibility> {
    read_visibility(&mut payload.reader(), profile)
}

/// Explicit caller-owned, single-value staging. Replacement happens only after
/// complete validation; failures preserve the slot. No callbacks, package/world
/// mutations, timeouts or PeerLab dispatch are installed. A caller handling a
/// whole bunch must validate its other fields before committing its own state.
pub fn stage_visibility(
    slot: &mut Option<DecodedVisibility>,
    payload: &Payload,
    profile: &VisibilityDecodeProfile,
) -> Result<()> {
    let decoded = parse_visibility(payload, profile)?;
    *slot = Some(decoded);
    Ok(())
}

fn read_name(reader: &mut BitReader<'_>) -> Result<NetworkName> {
    if read_field(reader, 1, "visibility name discriminator")? != 0 {
        // This existing helper requires canonical groups and rejects overflow
        // and a fifth continuation. Those are authored strict restrictions,
        // not a claim that the original packed reader rejects the same forms.
        let index = read_packed(reader, "visibility hardcoded name index")?;
        ensure!(
            index <= MAX_HARDCODED_NAME_INDEX,
            "unsupported visibility hardcoded name index"
        );
        return Ok(NetworkName::Hardcoded(index));
    }
    let text = read_text(reader)?;
    let number = read_u32(reader, ByteOrder::Little, "visibility name number")?;
    Ok(NetworkName::String { text, number })
}

fn read_text(reader: &mut BitReader<'_>) -> Result<ArchiveText> {
    let count = read_u32(reader, ByteOrder::Little, "visibility string count")? as i32;
    let magnitude = count
        .checked_abs()
        .context("visibility string count i32::MIN unsupported")? as usize;
    // MAX_STRING_UNITS (65,536 including terminator) is native resource policy,
    // not a recovered original constant or the unread archive count setting.
    ensure!(
        magnitude <= MAX_STRING_UNITS,
        "visibility string exceeds native unit limit"
    );
    let bytes = magnitude
        .checked_mul(if count < 0 { 2 } else { 1 })
        .context("visibility string byte count overflow")?;
    let bits = bytes
        .checked_mul(8)
        .context("visibility string bit count overflow")?;
    ensure!(bits <= reader.remaining(), "truncated visibility string");
    let capacity = bytes
        .checked_add(4)
        .context("visibility string frame size overflow")?;
    let mut frame = Vec::with_capacity(capacity);
    frame.extend_from_slice(&count.to_le_bytes());
    frame.extend_from_slice(&reader.read_bytes(bytes));
    // The shared strict subset rejects missing/embedded terminators, U+FFFF
    // and non-ASCII narrow units. Wide units, including unpaired surrogates,
    // stay raw; no Unicode, suffix or identity normalization is performed.
    let parsed = read_string_frame(&frame, ByteOrder::Little)?;
    let encoding = match parsed.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    ArchiveText::with_encoding(&parsed.units().collect::<Vec<_>>(), encoding)
}
