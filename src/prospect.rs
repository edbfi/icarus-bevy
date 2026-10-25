//! Experimental read-only envelope inspection, never world import.
//!
//! Recovered field names and algorithm observations are documented in
//! `docs/specs/save-format/prospect-envelope.md`. Payload serialization remains unknown.
use std::collections::BTreeMap;
use std::io::Read;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::experimental::archive::{ByteOrder, decode_string};
use crate::experimental::component::{
    BaseComponent, MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit,
};
use crate::experimental::property_tree::{
    BaseComponentTree, MAX_TREE_DEPTH, MAX_TREE_NODES, PropertyTree, TreeLimits,
};
use crate::experimental::prospect_info::{
    PlayerHistoryEntry, ProspectInfo, SavedContainerManager, SavedGameMode, SavedQuestActor,
    SavedQuestManager, saved_container_manager, saved_flod_tile_count, saved_player_history,
};
use crate::experimental::recorders::{
    RecorderSummary, StagedProspectSaveState, read_recorders, recorder_class_label,
    stage_prospect_save_state,
};
use crate::experimental::restore_rules::normalize_prospect_name;
use crate::experimental::struct_array::MAX_BOUNDED_STRUCT_ARRAY_BYTES;
use crate::experimental::tagged::{
    ParsedRecord, RootTagSummary, TagProfile, is_name, read_record, read_root,
};
use crate::experimental::typed_value::{Gap, TypedComponent, TypedSummary, TypedTree};
use crate::json::JsonObject;
use crate::preservation::{PreserveLimits, PreservedSection};

/// Bounded research policy, not a recovered ICARUS format limit. Observed inputs
/// reached 2,239,534 raw bytes and 19,131,027 decompressed bytes.
pub const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ProspectBlob")]
    blob: JsonObject<Blob>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Blob {
    // Identity and metadata fields are deliberately not returned to the caller.
    hash: String,
    total_length: i32,
    data_length: i32,
    uncompressed_length: i32,
    binary_blob: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvelopeInspection {
    pub validation: &'static str,
    pub compressed_bytes: usize,
    pub payload_bytes: usize,
    /// `FProspectBlob::DataLength` (`+0x24`): `WriteToProspectBlob` (`0x1418fa200`)
    /// writes `CompressedSize` to both `TotalLength` and `DataLength`, while
    /// `ReadFromProspectBlob` (`0x1418e6240`) leaves `DataLength` unread (`F-283`, `F-737`).
    pub declared_data_length: i32,
    pub legacy_sha1_matches: bool,
    pub world_imported: bool,
}

pub(crate) struct ValidatedEnvelope {
    pub(crate) inspection: EnvelopeInspection,
    pub(crate) payload: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct InnerTagInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub inner_tags: RootTagSummary,
    pub world_imported: bool,
}

#[derive(Debug, Serialize)]
pub struct RecorderInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub inner_tags: RootTagSummary,
    pub recorders: RecorderSummary,
    pub world_imported: bool,
}

#[derive(Debug, Serialize)]
pub struct StagedProspectInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub inner_tags: RootTagSummary,
    pub staged: StagedProspectSaveState,
    pub world_imported: bool,
}

/// Inspect one bounded JSON envelope without extracting payload or mutating state.
///
/// Unknown JSON fields are skipped. Known fields must occur once and use the
/// expected types. Canonical padded Base64 and exactly one zlib stream are required
/// by this inspector's conservative policy; original tolerance is unverified.
/// Envelopes may use UTF-8 or explicitly BOM-tagged UTF-16LE. Other encodings are
/// not guessed, and malformed UTF-16 is rejected without replacement characters.
pub fn inspect(reader: impl Read) -> Result<EnvelopeInspection> {
    Ok(decode_envelope(reader)?.inspection)
}

/// Opt-in root framing under an explicit research profile, after full envelope
/// integrity validation. Opaque decoded values are never returned or exported.
pub fn inspect_with_inner_tags(reader: impl Read) -> Result<InnerTagInspection> {
    let validated = decode_envelope(reader)?;
    let tags = read_root(&validated.payload, TagProfile::MemoryArchive522)
        .context("invalid experimental root tags")?;
    Ok(InnerTagInspection {
        validation: "experimental_root_tags_only",
        envelope: validated.inspection,
        inner_tags: tags.summary(),
        world_imported: false,
    })
}

/// Opt-in recorder framing, with component byte contents remaining opaque.
/// Reports aggregates only after all framing and envelope checks succeed.
pub fn inspect_with_recorders(reader: impl Read) -> Result<RecorderInspection> {
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    Ok(RecorderInspection {
        validation: "experimental_recorder_framing_only",
        envelope: validated.inspection,
        inner_tags: tags.summary(),
        recorders: recorders.summary(),
        world_imported: false,
    })
}

/// Stages and validates the full `FProspectSaveState` root schema, version
/// migration (`F-715`), `ProspectInfo` inner tags (`F-726`), and base-component
/// framing of every `StateRecorderBlob` (`F-403`, `F-733`) before returning any
/// summary. When `expected_prospect_name` is `Some`, normalizes it via
/// [`normalize_prospect_name`] and verifies that the post-migration
/// `ProspectInfo.ProspectID` matches ASCII case-insensitively (`F-717`).
pub fn stage_prospect(
    reader: impl Read,
    component_byte_limit: usize,
    expected_prospect_name: Option<&str>,
) -> Result<StagedProspectInspection> {
    ensure!(
        (1..=MAX_BOUNDED_COMPONENT_BYTES).contains(&component_byte_limit),
        "component byte limit must be between 1 byte and 1 MiB"
    );
    let expected_utf16 = match expected_prospect_name {
        Some(raw) => {
            let normalized =
                normalize_prospect_name(raw).context("empty expected prospect name")?;
            Some(normalized.encode_utf16().collect::<Vec<u16>>())
        }
        None => None,
    };
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let staged = stage_prospect_save_state(
        &tags.tags,
        profile,
        component_byte_limit,
        expected_utf16.as_deref(),
    )
    .context("invalid staged FProspectSaveState")?;
    Ok(StagedProspectInspection {
        validation: "experimental_staged_prospect_save_state_only",
        envelope: validated.inspection,
        inner_tags: tags.summary(),
        staged,
        world_imported: false,
    })
}

/// Typed-value coverage of every recorder body, by class pseudonym; counts only.
#[derive(Debug, Serialize)]
pub struct TypedProspectInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub staged: StagedProspectSaveState,
    pub recorder_count: usize,
    pub typed_recorders: usize,
    /// Bodies the base-component tree could not frame; kept opaque.
    pub unframed_recorders: usize,
    /// Recorders whose class string is not one of the 65 registered classes.
    pub unregistered_classes: usize,
    /// Every typed body re-encoded to its original bytes.
    pub round_trip_exact: bool,
    pub totals: TypedSummary,
    /// Per class pseudonym: recorders and uninterpreted leaves.
    pub classes: BTreeMap<String, ClassTypedCoverage>,
    pub world_imported: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ClassTypedCoverage {
    pub recorders: usize,
    pub properties: usize,
    pub uninterpreted: usize,
    pub gaps: BTreeMap<Gap, usize>,
}

/// Tree bounds for typing: the hard depth, node and struct-array ceilings, so a
/// large construction body frames instead of staying opaque.
const TYPED_TREE_LIMITS: TreeLimits = TreeLimits {
    max_depth: MAX_TREE_DEPTH,
    max_nodes: MAX_TREE_NODES,
    struct_array_bytes: MAX_BOUNDED_STRUCT_ARRAY_BYTES,
    container_struct_bytes: None,
};

/// Stage the prospect exactly as [`stage_prospect`] does, then frame every
/// recorder body as a base-component tree and type its values
/// ([`TypedComponent`]). Each typed body must re-encode to its original bytes or
/// the check fails. Reports counts by class pseudonym, never names or values.
pub fn typed_check(reader: impl Read) -> Result<TypedProspectInspection> {
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let staged = stage_prospect_save_state(&tags.tags, profile, MAX_BOUNDED_COMPONENT_BYTES, None)
        .context("invalid staged FProspectSaveState")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    let mut report = TypedProspectInspection {
        validation: "experimental_typed_values_only",
        envelope: validated.inspection,
        staged,
        recorder_count: recorders.records().len(),
        typed_recorders: 0,
        unframed_recorders: 0,
        unregistered_classes: 0,
        round_trip_exact: true,
        totals: TypedSummary::default(),
        classes: BTreeMap::new(),
        world_imported: false,
    };
    for recorder in recorders.records() {
        // read_recorders validated this exact FString; it is never reported.
        let class = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        let label = recorder_class_label(&class.units).unwrap_or_else(|| {
            report.unregistered_classes += 1;
            "unregistered".to_owned()
        });
        let coverage = report.classes.entry(label).or_default();
        coverage.recorders += 1;
        let Ok(tree) = BaseComponentTree::parse(
            recorder.binary_data,
            profile,
            MAX_BOUNDED_COMPONENT_BYTES,
            TYPED_TREE_LIMITS,
        ) else {
            report.unframed_recorders += 1;
            continue;
        };
        let (typed, summary) = TypedComponent::from_component(&tree)?;
        ensure!(
            typed.to_bytes()? == recorder.binary_data,
            "typed recorder body does not re-encode to its original bytes"
        );
        report.typed_recorders += 1;
        coverage.properties += summary.properties;
        coverage.uninterpreted += summary.uninterpreted;
        for (gap, count) in &summary.gaps {
            *coverage.gaps.entry(*gap).or_default() += count;
        }
        report.totals.add(&summary);
    }
    Ok(report)
}

/// What replicating a loaded prospect's GameState, managers and quest actors
/// needs from its save: the typed `ProspectInfo` (`F-738`) and the FLOD, game
/// mode, container manager, quest manager, quest actor and player history
/// recorders' values (`C013`, `C015`, `C016`, `C018`, `C019`, `C024`, `F-650`,
/// `F-934`), when those recorders exist.
/// The values are the save's; callers must keep them private.
pub struct SavedProspect {
    pub info: ProspectInfo,
    pub game_mode: Option<SavedGameMode>,
    /// The quest manager recorder's values (`C018`), when one exists.
    pub quest_manager: Option<SavedQuestManager>,
    /// The quest actor recorders (`C019`), in saved array order.
    pub quests: Vec<SavedQuestActor>,
    /// The player history recorder's entries (`C024`), when one exists.
    pub player_history: Option<Vec<PlayerHistoryEntry>>,
    /// The container manager recorder's inventories (`C016`), when one
    /// exists.
    pub inventory_containers: Option<SavedContainerManager>,
    /// The FLOD recorder's tile count (`C013`), when one exists.
    pub flod_tiles: Option<i32>,
}

/// Stage the prospect exactly as [`stage_prospect`] does, then type its root
/// `ProspectInfo`, its one `C013`, `C015`, `C016`, `C018` and `C024`
/// recorders, and its `C019` quest actor recorders in saved order.
pub fn read_saved_prospect(reader: impl Read) -> Result<SavedProspect> {
    saved_prospect_from_payload(&decode_envelope(reader)?.payload)
}

/// [`read_saved_prospect`] on an inner payload, such as a preserved
/// world's (`PreservedProspect::to_inner_payload`).
pub fn saved_prospect_from_payload(payload: &[u8]) -> Result<SavedProspect> {
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(payload, profile).context("invalid experimental root tags")?;
    stage_prospect_save_state(&tags.tags, profile, MAX_BOUNDED_COMPONENT_BYTES, None)
        .context("invalid staged FProspectSaveState")?;
    let mut info = None;
    for root in &tags.tags {
        let ParsedRecord::Property(property) = read_record(root.record, profile)? else {
            continue;
        };
        if is_name(&property.name, "ProspectInfo") {
            let tree = PropertyTree::parse(property.tag.payload, profile, TYPED_TREE_LIMITS)
                .context("invalid ProspectInfo tree")?;
            let (typed, _) = TypedTree::from_tree(&tree)?;
            info = Some(ProspectInfo::from_tree(&typed)?);
        }
    }
    let info = info.context("the prospect has no ProspectInfo")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    let mut game_mode = None;
    let mut quest_manager = None;
    let mut quests = Vec::new();
    let mut player_history = None;
    let mut inventory_containers = None;
    let mut flod_tiles = None;
    for recorder in recorders.records() {
        let class = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        let label = recorder_class_label(&class.units);
        let label = match label.as_deref() {
            Some(label @ ("C013" | "C015" | "C016" | "C018" | "C019" | "C024")) => label,
            _ => continue,
        };
        let tree = BaseComponentTree::parse(
            recorder.binary_data,
            profile,
            MAX_BOUNDED_COMPONENT_BYTES,
            TYPED_TREE_LIMITS,
        )
        .with_context(|| format!("invalid {label} recorder tree"))?;
        let (typed, _) = TypedComponent::from_component(&tree)?;
        let repeated = match label {
            "C015" => game_mode
                .replace(SavedGameMode::from_tree(&typed.tree)?)
                .is_some(),
            "C013" => flod_tiles
                .replace(saved_flod_tile_count(&typed.tree)?)
                .is_some(),
            "C016" => inventory_containers
                .replace(saved_container_manager(&typed.tree)?)
                .is_some(),
            "C018" => quest_manager
                .replace(SavedQuestManager::from_tree(&typed.tree)?)
                .is_some(),
            "C019" => {
                quests.push(SavedQuestActor::from_tree(&typed.tree)?);
                false
            }
            _ => player_history
                .replace(saved_player_history(&typed.tree)?)
                .is_some(),
        };
        ensure!(!repeated, "the prospect has more than one {label} recorder");
    }
    Ok(SavedProspect {
        info,
        game_mode,
        quest_manager,
        quests,
        player_history,
        inventory_containers,
        flod_tiles,
    })
}

/// Envelope observations plus a verified lossless preservation of its payload.
pub struct PreservedEnvelope {
    pub envelope: EnvelopeInspection,
    pub preserved: PreservedSection,
}

/// Fully validate the envelope, then preserve its inner payload losslessly and
/// value-opaquely. The decoded payload itself is never returned; the preserved
/// structure reconstructs it byte-identically or construction fails. Only the
/// compact form (verified native encoding and summary) is returned.
pub fn preserve(reader: impl Read, limits: PreserveLimits) -> Result<PreservedEnvelope> {
    let validated = decode_envelope(reader)?;
    let preserved = PreservedSection::from_inner_payload(&validated.payload, limits)
        .context("invalid experimental preservation")?;
    Ok(PreservedEnvelope {
        envelope: validated.inspection,
        preserved,
    })
}

/// One caller-selected recorder handed to a [`visit_class_components`] visitor.
///
/// `component` is the framing outcome as a value: `Ok` borrows the framed
/// component's exact source spans, and `Err` borrows the framing error. Both are
/// valid only for the duration of the call, so nothing escapes the visitor.
pub struct ComponentVisit<'a> {
    /// Zero-based position in the recorder array, counting every class.
    pub recorder_index: usize,
    /// Length of the recorder's opaque BinaryData bytes.
    pub binary_bytes: usize,
    pub component: std::result::Result<&'a BaseComponent<'a>, &'a anyhow::Error>,
}

/// Aggregate counts only; never the class string, field names or values.
#[derive(Debug, Serialize)]
pub struct ClassComponentReport {
    pub validation: &'static str,
    pub recorder_count: usize,
    pub matched_count: usize,
    pub framed_count: usize,
    pub failed_count: usize,
    pub world_imported: bool,
}

/// Explicitly experimental: after the same envelope, root-tag and recorder
/// validation as [`inspect_with_recorders`], frame the BinaryData of every
/// recorder whose decoded class string equals `class` exactly (UTF-16 unit
/// equality, array order) as a caller-selected base-component candidate under
/// `limit` bytes (1..=1 MiB, checked before any input is read). A framing
/// failure is passed to `visit` as a value and the walk continues; an `Err`
/// returned by `visit` aborts the walk and is returned unchanged. The visitor
/// receives borrowed spans and owns their privacy; the report holds counts only.
/// Matching a class string does not recognize the class or verify its schema.
pub fn visit_class_components(
    reader: impl Read,
    class: &str,
    limit: usize,
    mut visit: impl FnMut(ComponentVisit<'_>) -> Result<()>,
) -> Result<ClassComponentReport> {
    ensure!(
        (1..=MAX_BOUNDED_COMPONENT_BYTES).contains(&limit),
        "component byte limit must be between 1 byte and 1 MiB"
    );
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    let mut report = ClassComponentReport {
        validation: "experimental_class_component_framing_only",
        recorder_count: recorders.records().len(),
        matched_count: 0,
        framed_count: 0,
        failed_count: 0,
        world_imported: false,
    };
    for (recorder_index, recorder) in recorders.records().iter().enumerate() {
        // read_recorders validated this exact FString; decoding cannot leak it.
        let decoded = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        if !decoded.units.iter().copied().eq(class.encode_utf16()) {
            continue;
        }
        report.matched_count += 1;
        let framed = read_base_component_with_limit(recorder.binary_data, profile, limit);
        match &framed {
            Ok(_) => report.framed_count += 1,
            Err(_) => report.failed_count += 1,
        }
        visit(ComponentVisit {
            recorder_index,
            binary_bytes: recorder.binary_data.len(),
            component: framed.as_ref(),
        })?;
    }
    Ok(report)
}

pub(crate) fn decode_envelope(reader: impl Read) -> Result<ValidatedEnvelope> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_ENVELOPE_BYTES,
        "envelope exceeds 4 MiB research limit"
    );
    let envelope: JsonObject<Envelope> = if let Some(encoded) = bytes.strip_prefix(&[0xff, 0xfe]) {
        ensure!(encoded.len().is_multiple_of(2), "odd UTF-16LE byte length");
        let units = encoded
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes);
        let text = char::decode_utf16(units)
            .collect::<std::result::Result<String, _>>()
            .context("invalid UTF-16LE envelope")?;
        serde_json::from_str(&text)
    } else {
        // IgnoredAny skips unknown strings without validating their UTF-8 when
        // parsing bytes. Validate the whole document, including ignored metadata.
        let text = std::str::from_utf8(&bytes).context("invalid UTF-8 envelope")?;
        serde_json::from_str(text)
    }
    .map_err(|error| crate::json_parse_error("invalid prospect envelope JSON", error))?;
    let blob = envelope.0.blob.0;
    ensure!(
        blob.total_length >= 0 && blob.data_length >= 0 && blob.uncompressed_length >= 0,
        "negative prospect length"
    );
    let expected = blob.uncompressed_length as usize;
    ensure!(
        expected <= MAX_PAYLOAD_BYTES,
        "payload exceeds 32 MiB research limit"
    );
    ensure!(
        blob.hash.len() == 40
            && blob
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "expected lowercase 40-digit legacy SHA-1"
    );
    let compressed = STANDARD
        .decode(&blob.binary_blob)
        .context("invalid canonical Base64")?;
    ensure!(
        compressed.len() == blob.total_length as usize,
        "TotalLength differs from decoded byte count"
    );

    // One extra byte distinguishes an exact-length stream from excess output.
    // The caller's declared size cannot allocate beyond the fixed research cap.
    let mut output = vec![0_u8; expected + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(&compressed, &mut output, FlushDecompress::Finish)
        .context("invalid zlib payload")?;
    ensure!(
        status == Status::StreamEnd,
        "incomplete zlib stream or output exceeds declared length"
    );
    ensure!(
        decoder.total_in() == compressed.len() as u64,
        "trailing data after zlib stream"
    );
    ensure!(
        decoder.total_out() == expected as u64,
        "UncompressedLength differs from actual output"
    );
    output.truncate(expected);
    // SHA-1 is used only to inspect the legacy format, never for authentication.
    ensure!(
        crate::hex_lower(&Sha1::digest(&output)) == blob.hash,
        "legacy SHA-1 mismatch"
    );
    Ok(ValidatedEnvelope {
        inspection: EnvelopeInspection {
            validation: "experimental_envelope_only",
            compressed_bytes: compressed.len(),
            payload_bytes: output.len(),
            declared_data_length: blob.data_length,
            legacy_sha1_matches: true,
            world_imported: false,
        },
        payload: output,
    })
}
