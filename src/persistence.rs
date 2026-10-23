//! Native snapshots only. Official ICARUS save import requires a recovered spec.
mod discriminators;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::json::JsonObject;
use crate::native_c001_owners::C001OwnerImportMetadataV1;
use crate::native_c001_scalar_owners::C001ScalarOwnerImportMetadataV1;
use crate::native_import::C001InertImportMetadataV1;
use crate::native_lifecycle::LifecycleSnapshot;
use crate::native_relationships::NativeRelationshipsSnapshot;
use crate::native_scalar_owners::ScalarOwnerContractsSnapshotV1;
use crate::native_world::NativeWorldSnapshot;
use crate::preservation::{MAX_PRESERVED_BASE64_BYTES, SectionReference, is_sidecar_name};
use crate::simulation::{
    EntityRecord, SNAPSHOT_SCHEMA_C001_INERT, SNAPSHOT_SCHEMA_C001_OWNERS,
    SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS, SNAPSHOT_SCHEMA_DOMAIN, SNAPSHOT_SCHEMA_LIFECYCLE,
    SNAPSHOT_SCHEMA_NATIVE, SNAPSHOT_SCHEMA_PRESERVED, SNAPSHOT_SCHEMA_REFERENCED,
    SNAPSHOT_SCHEMA_RELATIONSHIPS, WorldSnapshot,
};

/// Schema 1 bound, unchanged: domain records and the clock only.
const MAX_DOMAIN_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
/// Schema 2 adds at most one 64 MiB Base64 preserved section (whose decoded
/// size is checked separately), so a generation file may reach 128 MiB.
const MAX_SNAPSHOT_BYTES: u64 = MAX_DOMAIN_SNAPSHOT_BYTES + MAX_PRESERVED_BASE64_BYTES as u64;
/// Aggregate generation cap for schemas 3 through 9, including inline native
/// definitions, metadata, relationship/lifecycle state and any fixed-size
/// sidecar reference. Individual section limits do not reserve extra space:
/// preflight and publication reject a combined encoding above this cap.
/// The separate sidecar is bounded by the 48 MiB native encoding cap.
const MAX_REFERENCED_SNAPSHOT_BYTES: u64 = MAX_DOMAIN_SNAPSHOT_BYTES + 4 * 1024;
/// Streaming verification buffer, so checking a sidecar never holds it whole.
const VERIFY_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
struct UnsupportedSchema;

impl std::fmt::Display for UnsupportedSchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .write_str("unsupported native snapshot schema; upgrade or explicit migration required")
    }
}

impl std::error::Error for UnsupportedSchema {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    number: u64,
    sha256: String,
}

impl Generation {
    fn validate_checksum(&self) -> Result<()> {
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid snapshot checksum in manifest"
        );
        Ok(())
    }

    fn filename(&self) -> String {
        format!("snapshot-{}.json", self.number)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    #[serde(deserialize_with = "crate::json::object")]
    current: Generation,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    previous: Option<Generation>,
}

// Probe only the schema before typed decoding. Ignoring the entity fields avoids
// allocating an intermediate JSON object tree for every saved entity.
#[derive(Deserialize)]
struct SnapshotHeader {
    #[serde(default)]
    schema_version: serde_json::Value,
}

/// Probe embedded version discriminators without decoding actor records or
/// retaining catalog values. Unknown versions are upgrade requirements, never
/// evidence that an older committed world may replace this one.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct NativeProbe<T> {
    #[serde(deserialize_with = "crate::json::object")]
    native_world: DefinitionProbe<T>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct DefinitionProbe<T> {
    #[serde(deserialize_with = "crate::json::object")]
    definitions: T,
}

#[derive(Deserialize)]
struct NativeFormatProbe {
    #[serde(default)]
    native_format: serde_json::Value,
}

#[derive(Deserialize)]
struct NativeClassesProbe {
    #[serde(deserialize_with = "crate::json::objects")]
    classes: Vec<NativeClassProbe>,
}

#[derive(Deserialize)]
struct NativeClassProbe {
    #[serde(default)]
    kind: serde_json::Value,
}

#[derive(Deserialize)]
struct ImportProbe {
    #[serde(deserialize_with = "crate::json::object")]
    native_import: ImportDiscriminators,
}

#[derive(Deserialize)]
struct ImportDiscriminators {
    #[serde(default)]
    format: serde_json::Value,
    #[serde(default)]
    adapter: serde_json::Value,
    #[serde(default)]
    coordinates: serde_json::Value,
}

#[derive(Deserialize)]
struct OptionalImportProbe {
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_import: Option<ImportDiscriminators>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct RelationshipsProbe<T> {
    #[serde(deserialize_with = "crate::json::object")]
    native_relationships: RelationshipDefinitionProbe<T>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct RelationshipDefinitionProbe<T> {
    #[serde(deserialize_with = "crate::json::object")]
    definition: T,
}

#[derive(Deserialize)]
struct RelationshipFormatProbe {
    #[serde(default)]
    relationship_format: serde_json::Value,
}

#[derive(Deserialize)]
struct RelationshipRawProbe {
    native_relationships: serde_json::Value,
}

/// Preserve the exact pre-relationship field set for schemas 1 and 2. Decoding
/// those schemas through the growing in-memory DTO would silently admit newly
/// added optional keys, even when their value is null.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(default, with = "crate::preservation::snapshot_section")]
    preserved_prospect: Option<Arc<crate::preservation::PreservedSection>>,
    #[serde(default)]
    native_world: Option<NativeWorldSnapshot>,
    #[serde(default)]
    native_import: Option<C001InertImportMetadataV1>,
}

/// Schema 3 generation as written: schema 1 fields in their schema 1 order,
/// then the sidecar reference. The preserved data itself is never inline.
#[derive(Serialize)]
struct ReferencedOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    preserved_prospect: &'a SectionReference,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferencedIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    preserved_prospect: SectionReference,
}

/// Schema 4 embeds the authored catalog and native records. Preservation is
/// optional and, when present, uses the same durable sidecar as schema 3.
#[derive(Serialize)]
struct NativeOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserved_prospect: Option<&'a SectionReference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    preserved_prospect: Option<SectionReference>,
}

/// Schema 5 always retains both the original-player-format authored source and
/// its explicit native identity metadata. Neither field is optional on disk.
#[derive(Serialize)]
struct C001InertOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    preserved_prospect: &'a SectionReference,
    native_import: &'a C001InertImportMetadataV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct C001InertIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    preserved_prospect: SectionReference,
    #[serde(deserialize_with = "crate::json::object")]
    native_import: C001InertImportMetadataV1,
}

/// Schema 6 requires both independently authored native contracts. Optional
/// C001 metadata retains its existing requirement for a verified source.
#[derive(Serialize)]
struct DedicatedRelationshipsOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    native_relationships: &'a NativeRelationshipsSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserved_prospect: Option<&'a SectionReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_import: Option<&'a C001InertImportMetadataV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DedicatedRelationshipsIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_relationships: NativeRelationshipsSnapshot,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    preserved_prospect: Option<SectionReference>,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_import: Option<C001InertImportMetadataV1>,
}

/// Schema 7 retains complete native slot records, including dormant owners.
/// C001 import metadata is deliberately outside this authored contract.
#[derive(Serialize)]
struct DedicatedLifecycleOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    native_lifecycle: &'a LifecycleSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_relationships: Option<&'a NativeRelationshipsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserved_prospect: Option<&'a SectionReference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DedicatedLifecycleIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_lifecycle: LifecycleSnapshot,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_relationships: Option<NativeRelationshipsSnapshot>,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    preserved_prospect: Option<SectionReference>,
}

/// Schema 8 binds complete lifecycle state to immutable authored C001 source
/// provenance. Preservation is a required sidecar reference, never inline.
#[derive(Serialize)]
struct C001OwnersOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    native_lifecycle: &'a LifecycleSnapshot,
    native_owner_import: &'a C001OwnerImportMetadataV1,
    preserved_prospect: &'a SectionReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_relationships: Option<&'a NativeRelationshipsSnapshot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct C001OwnersIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_lifecycle: LifecycleSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_owner_import: C001OwnerImportMetadataV1,
    #[serde(deserialize_with = "crate::json::object")]
    preserved_prospect: SectionReference,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_relationships: Option<NativeRelationshipsSnapshot>,
}

/// Raw section probes deliberately accept malformed member shapes. Typed
/// decoding rejects them only after every independent discriminator was scanned.
#[derive(Deserialize)]
struct C001OwnersProbe {
    #[serde(default)]
    native_world: serde_json::Value,
    #[serde(default)]
    native_lifecycle: serde_json::Value,
    #[serde(default)]
    native_relationships: serde_json::Value,
    #[serde(default)]
    native_owner_import: serde_json::Value,
    #[serde(default)]
    preserved_prospect: serde_json::Value,
}

#[derive(Serialize)]
struct C001ScalarOwnersOut<'a> {
    schema_version: u32,
    tick: u64,
    entities: &'a [EntityRecord],
    native_world: &'a NativeWorldSnapshot,
    native_lifecycle: &'a LifecycleSnapshot,
    native_scalar_contracts: &'a ScalarOwnerContractsSnapshotV1,
    native_scalar_owner_import: &'a C001ScalarOwnerImportMetadataV1,
    preserved_prospect: &'a SectionReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_relationships: Option<&'a NativeRelationshipsSnapshot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct C001ScalarOwnersIn {
    schema_version: u32,
    tick: u64,
    #[serde(deserialize_with = "crate::json::objects")]
    entities: Vec<EntityRecord>,
    #[serde(deserialize_with = "crate::json::object")]
    native_world: NativeWorldSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_lifecycle: LifecycleSnapshot,
    #[serde(deserialize_with = "crate::json::object")]
    native_scalar_contracts: ScalarOwnerContractsSnapshotV1,
    #[serde(deserialize_with = "crate::json::object")]
    native_scalar_owner_import: C001ScalarOwnerImportMetadataV1,
    #[serde(deserialize_with = "crate::json::object")]
    preserved_prospect: SectionReference,
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_relationships: Option<NativeRelationshipsSnapshot>,
}

#[derive(Deserialize)]
struct LifecycleProbe {
    #[serde(default)]
    native_lifecycle: serde_json::Value,
}

#[derive(Deserialize)]
struct LifecycleNativeProbe {
    #[serde(default)]
    native_world: serde_json::Value,
}

#[derive(Deserialize)]
struct LifecycleFormatEnvelope {
    #[serde(deserialize_with = "crate::json::object")]
    native_lifecycle: LifecycleDefinitionProbe,
}

#[derive(Deserialize)]
struct LifecycleDefinitionProbe {
    #[serde(deserialize_with = "crate::json::object")]
    definition: LifecycleFormatProbe,
}

#[derive(Deserialize)]
struct LifecycleFormatProbe {
    #[serde(default)]
    lifecycle_format: serde_json::Value,
}

#[derive(Deserialize)]
struct OptionalRelationshipsProbe {
    #[serde(default, deserialize_with = "crate::json::optional_object")]
    native_relationships: Option<RelationshipDefinitionProbe<RelationshipFormatProbe>>,
}

fn future_enum(value: &serde_json::Value, accepted: &[&str]) -> bool {
    value
        .as_str()
        .is_some_and(|value| !accepted.contains(&value))
}

fn future_format(value: &serde_json::Value, current: u32) -> bool {
    value
        .as_u64()
        .is_some_and(|format| format > u64::from(current))
}

fn future_completion_discriminator(completion: &serde_json::Value) -> bool {
    completion["outcomes"].as_array().is_some_and(|outcomes| {
        outcomes.iter().any(|outcome| {
            future_enum(
                &outcome["policy"],
                &["find_only", "find_or_respawn", "respawn_only"],
            ) || future_enum(&outcome["source"]["type"], &["bound", "detached"])
                || future_enum(&outcome["before_presence"], &["absent", "live"])
                || future_enum(
                    &outcome["disposition"],
                    &["kept_bound", "attached", "spawned", "replaced"],
                )
                || future_enum(&outcome["component_disposition"], &["existing", "created"])
        })
    })
}

fn future_native_discriminator(native: &serde_json::Value) -> bool {
    let future_values = |values: &serde_json::Value| {
        values.as_array().is_some_and(|values| {
            values.iter().any(|value| {
                future_enum(&value["value"]["type"], &["bool", "i32", "f32", "string"])
            })
        })
    };
    future_format(
        &native["definitions"]["native_format"],
        crate::native_world::NATIVE_FORMAT,
    ) || future_enum(
        &native["definitions"]["frame"],
        &["right_handed_z_up_meters"],
    ) || native["definitions"]["classes"]
        .as_array()
        .is_some_and(|classes| {
            classes.iter().any(|class| {
                future_enum(&class["kind"], &["spatial_object_v1"])
                    || class["variables"].as_array().is_some_and(|variables| {
                        variables.iter().any(|variable| {
                            future_enum(
                                &variable["default"]["type"],
                                &["bool", "i32", "f32", "string"],
                            )
                        })
                    })
            })
        })
        || native["definitions"]["maps"]
            .as_array()
            .is_some_and(|maps| {
                maps.iter().any(|map| {
                    map["placements"].as_array().is_some_and(|placements| {
                        placements
                            .iter()
                            .any(|placement| future_values(&placement["initial_values"]))
                    })
                })
            })
        || native["actors"].as_array().is_some_and(|actors| {
            actors
                .iter()
                .any(|actor| future_values(&actor["variables"]))
        })
}

fn future_lifecycle_discriminator(lifecycle: &serde_json::Value) -> bool {
    future_format(
        &lifecycle["definition"]["lifecycle_format"],
        crate::native_lifecycle::NATIVE_LIFECYCLE_FORMAT,
    ) || lifecycle["definition"]["slots"]
        .as_array()
        .is_some_and(|slots| {
            slots.iter().any(|slot| {
                future_enum(&slot["recorder_kind"], &["spatial_state_v1"])
                    || future_enum(&slot["initial_presence"], &["absent", "live"])
                    || future_enum(&slot["spawn_component"], &["missing", "declared_kind"])
                    || future_enum(&slot["initial_component"]["kind"], &["spatial_state_v1"])
            })
        })
        || lifecycle["slots"].as_array().is_some_and(|slots| {
            slots.iter().any(|slot| {
                future_enum(&slot["presence"], &["absent", "live"])
                    || future_enum(&slot["component"]["kind"], &["spatial_state_v1"])
            })
        })
        || lifecycle["bindings"].as_array().is_some_and(|bindings| {
            bindings
                .iter()
                .any(|binding| future_enum(&binding["kind"], &["spatial_state_v1"]))
        })
        || future_completion_discriminator(&lifecycle["completed"])
}

fn future_relationship_discriminator(relationships: &serde_json::Value) -> bool {
    future_format(
        &relationships["definition"]["relationship_format"],
        crate::native_relationships::NATIVE_RELATIONSHIP_FORMAT,
    ) || relationships["definition"]["actors"]
        .as_array()
        .is_some_and(|actors| {
            actors
                .iter()
                .any(|actor| future_enum(&actor["role"], &["spatial", "deployable", "foundation"]))
        })
}

fn future_owner_import_discriminator(metadata: &serde_json::Value) -> bool {
    future_format(
        &metadata["format"],
        crate::native_c001_owners::C001_OWNER_IMPORT_FORMAT,
    ) || future_enum(&metadata["adapter"], &["c001_authored_owners_v1"])
        || future_enum(
            &metadata["coordinates"],
            &["right_handed_z_up_meters_quat_xyzw_world_space"],
        )
        || metadata["owners"].as_array().is_some_and(|owners| {
            owners.iter().any(|owner| {
                future_enum(
                    &owner["source_policy"],
                    &["find_only", "find_or_respawn", "respawn_only"],
                ) || future_enum(&owner["binding"]["expected_presence"], &["absent", "live"])
            })
        })
        || future_completion_discriminator(&metadata["initial_completion"])
}

fn encode_bounded_generation(value: &impl Serialize, limit: u64) -> Result<Vec<u8>> {
    struct Counter {
        bytes: u64,
        limit: u64,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len() as u64)
                .filter(|total| *total <= self.limit)
                .ok_or_else(|| std::io::Error::other("native snapshot exceeds its size limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // Count all inline sections and the sidecar reference together before
    // allocating the complete encoding, with exactly the publication serializer.
    serde_json::to_writer(&mut Counter { bytes: 0, limit }, value)?;
    Ok(serde_json::to_vec(value)?)
}

/// Pure generation preparation shared by preflight and durable publication.
fn encode_generation(snapshot: &WorldSnapshot) -> Result<(Vec<u8>, Option<SectionReference>)> {
    snapshot.validate()?;
    let reference = snapshot
        .preserved_prospect
        .as_ref()
        .map(|preserved| SectionReference::of(preserved));
    if let Some(reference) = &reference {
        reference.validate()?;
    }
    // Dispatch by schema before considering an optional source sidecar.
    let (bytes, limit) = match snapshot.schema_version {
        SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS => (
            encode_bounded_generation(
                &C001ScalarOwnersOut {
                    schema_version: SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS,
                    tick: snapshot.tick,
                    entities: &snapshot.entities,
                    native_world: snapshot
                        .native_world
                        .as_ref()
                        .context("native world required")?,
                    native_lifecycle: snapshot
                        .native_lifecycle
                        .as_ref()
                        .context("native lifecycle required")?,
                    native_scalar_contracts: snapshot
                        .native_scalar_contracts
                        .as_ref()
                        .context("scalar contracts required")?,
                    native_scalar_owner_import: snapshot
                        .native_scalar_owner_import
                        .as_ref()
                        .context("scalar owner provenance required")?,
                    preserved_prospect: reference.as_ref().context("preserved source required")?,
                    native_relationships: snapshot.native_relationships.as_ref(),
                },
                MAX_REFERENCED_SNAPSHOT_BYTES,
            )?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        SNAPSHOT_SCHEMA_C001_OWNERS => (
            encode_bounded_generation(
                &C001OwnersOut {
                    schema_version: SNAPSHOT_SCHEMA_C001_OWNERS,
                    tick: snapshot.tick,
                    entities: &snapshot.entities,
                    native_world: snapshot
                        .native_world
                        .as_ref()
                        .context("native world required")?,
                    native_lifecycle: snapshot
                        .native_lifecycle
                        .as_ref()
                        .context("native lifecycle required")?,
                    native_owner_import: snapshot
                        .native_owner_import
                        .as_ref()
                        .context("native owner import metadata required")?,
                    preserved_prospect: reference.as_ref().context("preserved source required")?,
                    native_relationships: snapshot.native_relationships.as_ref(),
                },
                MAX_REFERENCED_SNAPSHOT_BYTES,
            )?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        SNAPSHOT_SCHEMA_LIFECYCLE => (
            serde_json::to_vec(&DedicatedLifecycleOut {
                schema_version: SNAPSHOT_SCHEMA_LIFECYCLE,
                tick: snapshot.tick,
                entities: &snapshot.entities,
                native_world: snapshot
                    .native_world
                    .as_ref()
                    .context("native world required")?,
                native_lifecycle: snapshot
                    .native_lifecycle
                    .as_ref()
                    .context("native lifecycle required")?,
                native_relationships: snapshot.native_relationships.as_ref(),
                preserved_prospect: reference.as_ref(),
            })?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        SNAPSHOT_SCHEMA_RELATIONSHIPS => (
            serde_json::to_vec(&DedicatedRelationshipsOut {
                schema_version: SNAPSHOT_SCHEMA_RELATIONSHIPS,
                tick: snapshot.tick,
                entities: &snapshot.entities,
                native_world: snapshot
                    .native_world
                    .as_ref()
                    .context("native world required")?,
                native_relationships: snapshot
                    .native_relationships
                    .as_ref()
                    .context("native relationships required")?,
                preserved_prospect: reference.as_ref(),
                native_import: snapshot.native_import.as_ref(),
            })?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        SNAPSHOT_SCHEMA_C001_INERT => (
            serde_json::to_vec(&C001InertOut {
                schema_version: SNAPSHOT_SCHEMA_C001_INERT,
                tick: snapshot.tick,
                entities: &snapshot.entities,
                native_world: snapshot
                    .native_world
                    .as_ref()
                    .context("native world required")?,
                preserved_prospect: reference.as_ref().context("preserved source required")?,
                native_import: snapshot
                    .native_import
                    .as_ref()
                    .context("native import metadata required")?,
            })?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        SNAPSHOT_SCHEMA_NATIVE => (
            serde_json::to_vec(&NativeOut {
                schema_version: SNAPSHOT_SCHEMA_NATIVE,
                tick: snapshot.tick,
                entities: &snapshot.entities,
                native_world: snapshot
                    .native_world
                    .as_ref()
                    .context("native world required")?,
                preserved_prospect: reference.as_ref(),
            })?,
            MAX_REFERENCED_SNAPSHOT_BYTES,
        ),
        _ => match &reference {
            Some(reference) => (
                serde_json::to_vec(&ReferencedOut {
                    schema_version: SNAPSHOT_SCHEMA_REFERENCED,
                    tick: snapshot.tick,
                    entities: &snapshot.entities,
                    preserved_prospect: reference,
                })?,
                MAX_REFERENCED_SNAPSHOT_BYTES,
            ),
            None => (serde_json::to_vec(snapshot)?, MAX_DOMAIN_SNAPSHOT_BYTES),
        },
    };
    ensure!(
        bytes.len() as u64 <= limit,
        "native snapshot exceeds {} MiB limit",
        limit / (1024 * 1024)
    );
    Ok((bytes, reference))
}

/// What a save needs from a committed generation: its tick and the sidecar it
/// keeps retained.
#[derive(Clone, Debug)]
struct Committed {
    tick: u64,
    sidecar: Option<SectionReference>,
}

#[derive(Debug)]
pub struct LoadedSnapshot {
    pub snapshot: WorldSnapshot,
    pub recovered_previous: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SaveReceipt {
    pub generation: u64,
    pub tick: u64,
}

/// Holds the single-writer advisory lock for its entire lifetime.
pub struct SnapshotStore {
    root: PathBuf,
    _lock: File,
    /// Generation checksum to its committed facts, recorded only after that
    /// exact generation decoded completely here (or was written here from a
    /// validated snapshot). Decoding is a pure function of the checksummed
    /// generation and digest-verified sidecar bytes, so a save re-reads and
    /// re-verifies both but does not decode the preserved section again.
    /// Pruned to the retained generations after every publication.
    verified: Mutex<BTreeMap<String, Committed>>,
}

impl SnapshotStore {
    /// Validate and encode the exact bounded generation representation without
    /// opening, reading or modifying any destination.
    pub fn preflight(snapshot: &WorldSnapshot) -> Result<()> {
        encode_generation(snapshot).map(|_| ())
    }

    pub fn open(root: &Path) -> Result<Self> {
        let mut created_ancestors = Vec::new();
        let mut ancestor = root;
        while !ancestor.as_os_str().is_empty() && !ancestor.exists() {
            created_ancestors.push(ancestor.to_path_buf());
            ancestor = ancestor.parent().unwrap_or(Path::new("."));
        }
        fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(crate::nonblocking_flag())
            .open(root.join("server.lock"))?;
        ensure!(
            lock.metadata()?.is_file(),
            "server lock must be a regular file"
        );
        lock.try_lock()
            .map_err(std::io::Error::from)
            .context("data directory is already locked by another server")?;
        // Persist newly created directory entries before acknowledging later saves.
        sync_directory(root)?;
        for directory in created_ancestors {
            let parent = directory
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            sync_directory(parent)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            _lock: lock,
            verified: Mutex::new(BTreeMap::new()),
        })
    }

    fn manifest(&self) -> Result<Option<Manifest>> {
        let path = self.root.join("manifest.json");
        let bytes = match read_bounded(&path, 16 * 1024) {
            Ok(bytes) => bytes,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                // An empty directory is a new world; an orphan generation is not.
                // A stranded sidecar alone references nothing and is not a world.
                for entry in fs::read_dir(&self.root)? {
                    let name = entry?.file_name();
                    ensure!(
                        !name.to_string_lossy().starts_with("snapshot-"),
                        "manifest missing with existing snapshots; manual recovery required"
                    );
                }
                return Ok(None);
            }
            Err(error) => return Err(error.context("read native snapshot manifest")),
        };
        let manifest = serde_json::from_slice::<JsonObject<Manifest>>(&bytes)
            .map_err(|error| crate::json_parse_error("invalid snapshot manifest JSON", error))?
            .0;
        ensure!(manifest.schema_version == 1, "unsupported manifest schema");
        ensure!(manifest.current.number > 0, "invalid snapshot generation");
        manifest.current.validate_checksum()?;
        if let Some(previous) = &manifest.previous {
            ensure!(
                previous.number > 0 && previous.number < manifest.current.number,
                "invalid previous generation"
            );
            previous.validate_checksum()?;
        }
        Ok(Some(manifest))
    }

    /// Read a generation and verify the manifest checksum over its bytes.
    fn read_checked(&self, generation: &Generation) -> Result<Vec<u8>> {
        let bytes = read_bounded(&self.root.join(generation.filename()), MAX_SNAPSHOT_BYTES)?;
        ensure!(
            checksum(&bytes) == generation.sha256,
            "snapshot checksum mismatch"
        );
        Ok(bytes)
    }

    fn read_generation(&self, generation: &Generation) -> Result<WorldSnapshot> {
        let bytes = self.read_checked(generation)?;
        let (snapshot, sidecar) = self.decode_generation(&bytes)?;
        self.remember(
            generation,
            Committed {
                tick: snapshot.tick,
                sidecar,
            },
        );
        Ok(snapshot)
    }

    /// Decode checksummed generation bytes completely, including any referenced
    /// sidecar, which is verified by size and digest before it is decoded.
    fn decode_generation(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        if discriminators::header(bytes)? {
            ensure!(
                bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
                "schema 9 native snapshot exceeds its size limit"
            );
            discriminators::scalar_owners(bytes)?;
        }
        let header = serde_json::from_slice::<JsonObject<SnapshotHeader>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot JSON", error))?
            .0;
        match header.schema_version.as_u64() {
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_DOMAIN) => ensure!(
                bytes.len() as u64 <= MAX_DOMAIN_SNAPSHOT_BYTES,
                "schema 1 native snapshot exceeds 64 MiB limit"
            ),
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_PRESERVED) => {}
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_REFERENCED) => {
                return self.decode_referenced(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_NATIVE) => {
                return self.decode_native(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_INERT) => {
                return self.decode_c001_inert(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_RELATIONSHIPS) => {
                return self.decode_relationships(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_LIFECYCLE) => {
                return self.decode_lifecycle(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_OWNERS) => {
                return self.decode_c001_owners(bytes);
            }
            Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS) => {
                return self.decode_c001_scalar_owners(bytes);
            }
            _ => return Err(UnsupportedSchema.into()),
        }
        let legacy = serde_json::from_slice::<JsonObject<LegacyIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let snapshot = WorldSnapshot {
            schema_version: legacy.schema_version,
            tick: legacy.tick,
            entities: legacy.entities,
            preserved_prospect: legacy.preserved_prospect,
            native_world: legacy.native_world,
            native_import: legacy.native_import,
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, None))
    }

    fn decode_referenced(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 3 native snapshot exceeds its size limit"
        );
        let referenced = serde_json::from_slice::<JsonObject<ReferencedIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = referenced.preserved_prospect;
        reference.validate()?;
        // A missing sidecar is a NotFound I/O error: this generation is
        // corrupt, so recovery selects the previous one exactly as it would
        // for a missing generation. Other I/O errors stay operational.
        let encoded = read_bounded(
            &self.root.join(reference.sidecar_name()),
            reference.encoded_bytes(),
        )?;
        // Verified once, streaming, to prove it reconstructs the recorded
        // payload; only the verified encoding is kept and no value tree is built.
        let preserved = reference.decode(encoded)?;
        let snapshot = WorldSnapshot {
            schema_version: referenced.schema_version,
            tick: referenced.tick,
            entities: referenced.entities,
            preserved_prospect: Some(Arc::new(preserved)),
            native_world: None,
            native_import: None,
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, Some(reference)))
    }

    fn decode_native(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 4 native snapshot exceeds its size limit"
        );
        Self::probe_native_discriminators(bytes)?;
        let native = serde_json::from_slice::<JsonObject<NativeIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        let preserved = reference
            .as_ref()
            .map(|reference| {
                reference.validate()?;
                let encoded = read_bounded(
                    &self.root.join(reference.sidecar_name()),
                    reference.encoded_bytes(),
                )?;
                reference.decode(encoded).map(Arc::new)
            })
            .transpose()?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            preserved_prospect: preserved,
            native_import: None,
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        // Validation recompiles the embedded catalog and verifies its identity
        // and every native record before this generation can be activated.
        snapshot.validate()?;
        Ok((snapshot, reference))
    }

    fn decode_c001_inert(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 5 native snapshot exceeds its size limit"
        );
        let discriminators = serde_json::from_slice::<JsonObject<ImportProbe>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native import metadata JSON", error))?
            .0
            .native_import;
        Self::probe_import_discriminators(&discriminators)?;
        Self::probe_native_discriminators(bytes)?;
        let native = serde_json::from_slice::<JsonObject<C001InertIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        reference.validate()?;
        let encoded = read_bounded(
            &self.root.join(reference.sidecar_name()),
            reference.encoded_bytes(),
        )?;
        let preserved = reference.decode(encoded)?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            preserved_prospect: Some(Arc::new(preserved)),
            native_import: Some(native.native_import),
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, Some(reference)))
    }

    fn probe_import_discriminators(discriminators: &ImportDiscriminators) -> Result<()> {
        if discriminators
            .format
            .as_u64()
            .is_some_and(|format| format > 1)
            || discriminators
                .adapter
                .as_str()
                .is_some_and(|adapter| adapter != "c001_inert_placement_v1")
            || discriminators
                .coordinates
                .as_str()
                .is_some_and(|coordinates| {
                    coordinates != "right_handed_z_up_meters_quat_xyzw_world_space"
                })
        {
            return Err(UnsupportedSchema.into());
        }
        Ok(())
    }

    fn decode_relationships(
        &self,
        bytes: &[u8],
    ) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 6 native snapshot exceeds its size limit"
        );
        // Discriminator probes precede typed state decoding and all sidecar
        // access. Future layouts therefore cannot trigger silent rollback.
        let native_probe = Self::probe_native_discriminators(bytes);
        let relationship_probe = Self::probe_relationship_discriminators(bytes);
        let import_probe = serde_json::from_slice::<JsonObject<OptionalImportProbe>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native import metadata JSON", error))
            .and_then(|probe| match probe.0.native_import {
                Some(import) => Self::probe_import_discriminators(&import),
                None => Ok(()),
            });
        let probes = [native_probe, relationship_probe, import_probe];
        // An upgrade requirement in any independent section must win over
        // recoverable corruption elsewhere, or recovery could erase newer state.
        if probes.iter().any(|probe| {
            probe
                .as_ref()
                .is_err_and(|error| error.is::<UnsupportedSchema>())
        }) {
            return Err(UnsupportedSchema.into());
        }
        for probe in probes {
            probe?;
        }
        let native = serde_json::from_slice::<JsonObject<DedicatedRelationshipsIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        let preserved = reference
            .as_ref()
            .map(|reference| {
                reference.validate()?;
                let encoded = read_bounded(
                    &self.root.join(reference.sidecar_name()),
                    reference.encoded_bytes(),
                )?;
                reference.decode(encoded).map(Arc::new)
            })
            .transpose()?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            preserved_prospect: preserved,
            native_import: native.native_import,
            native_relationships: Some(native.native_relationships),
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, reference))
    }

    fn probe_relationship_discriminators(bytes: &[u8]) -> Result<()> {
        let format = serde_json::from_slice::<
            JsonObject<RelationshipsProbe<RelationshipFormatProbe>>,
        >(bytes)
        .map_err(|error| crate::json_parse_error("decode native relationship fields JSON", error));
        if format
            .as_ref()
            .ok()
            .and_then(|probe| {
                probe
                    .0
                    .native_relationships
                    .definition
                    .relationship_format
                    .as_u64()
            })
            .is_some_and(|format| {
                format > u64::from(crate::native_relationships::NATIVE_RELATIONSHIP_FORMAT)
            })
        {
            return Err(UnsupportedSchema.into());
        }
        // Scan all role discriminators before typed actor admission. A malformed
        // earlier member must not hide a later upgrade requirement and permit
        // recovery to an older generation.
        let raw = serde_json::from_slice::<JsonObject<RelationshipRawProbe>>(bytes)
            .map_err(|error| {
                crate::json_parse_error("decode native relationship fields JSON", error)
            })?
            .0
            .native_relationships;
        if future_relationship_discriminator(&raw) {
            return Err(UnsupportedSchema.into());
        }
        format?;
        Ok(())
    }

    fn decode_lifecycle(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 7 native snapshot exceeds its size limit"
        );
        let native_probe = Self::probe_lifecycle_native_discriminators(bytes);
        let lifecycle_probe = Self::probe_lifecycle_discriminators(bytes);
        let relationship_probe =
            serde_json::from_slice::<JsonObject<OptionalRelationshipsProbe>>(bytes)
                .map_err(|error| {
                    crate::json_parse_error("decode native relationship fields JSON", error)
                })
                .and_then(|probe| match probe.0.native_relationships {
                    Some(_) => Self::probe_relationship_discriminators(bytes),
                    None => Ok(()),
                });
        // Evaluate independent sections completely before accepting a recoverable
        // error: an upgrade requirement anywhere forbids selecting older state.
        let probes = [native_probe, lifecycle_probe, relationship_probe];
        if probes.iter().any(|probe| {
            probe
                .as_ref()
                .is_err_and(|error| error.is::<UnsupportedSchema>())
        }) {
            return Err(UnsupportedSchema.into());
        }
        for probe in probes {
            probe?;
        }
        let native = serde_json::from_slice::<JsonObject<DedicatedLifecycleIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        let preserved = reference
            .as_ref()
            .map(|reference| {
                reference.validate()?;
                let encoded = read_bounded(
                    &self.root.join(reference.sidecar_name()),
                    reference.encoded_bytes(),
                )?;
                reference.decode(encoded).map(Arc::new)
            })
            .transpose()?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            native_lifecycle: Some(native.native_lifecycle),
            native_relationships: native.native_relationships,
            preserved_prospect: preserved,
            native_import: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, reference))
    }

    fn decode_c001_owners(
        &self,
        bytes: &[u8],
    ) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 8 native snapshot exceeds its size limit"
        );
        Self::probe_c001_owner_discriminators(bytes)?;
        let native = serde_json::from_slice::<JsonObject<C001OwnersIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        reference.validate()?;
        let encoded = read_bounded(
            &self.root.join(reference.sidecar_name()),
            reference.encoded_bytes(),
        )?;
        let preserved = reference.decode(encoded)?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            native_lifecycle: Some(native.native_lifecycle),
            native_owner_import: Some(native.native_owner_import),
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
            native_relationships: native.native_relationships,
            preserved_prospect: Some(Arc::new(preserved)),
            native_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, Some(reference)))
    }

    fn decode_c001_scalar_owners(
        &self,
        bytes: &[u8],
    ) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 9 native snapshot exceeds its size limit"
        );
        let native = serde_json::from_slice::<JsonObject<C001ScalarOwnersIn>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
        let reference = native.preserved_prospect;
        reference.validate()?;
        let encoded = read_bounded(
            &self.root.join(reference.sidecar_name()),
            reference.encoded_bytes(),
        )?;
        let preserved = reference.decode(encoded)?;
        let snapshot = WorldSnapshot {
            schema_version: native.schema_version,
            tick: native.tick,
            entities: native.entities,
            native_world: Some(native.native_world),
            native_lifecycle: Some(native.native_lifecycle),
            native_scalar_contracts: Some(native.native_scalar_contracts),
            native_scalar_owner_import: Some(native.native_scalar_owner_import),
            native_relationships: native.native_relationships,
            preserved_prospect: Some(Arc::new(preserved)),
            native_import: None,
            native_owner_import: None,
        };
        snapshot.validate()?;
        Ok((snapshot, Some(reference)))
    }

    fn probe_c001_owner_discriminators(bytes: &[u8]) -> Result<()> {
        let probe = serde_json::from_slice::<JsonObject<C001OwnersProbe>>(bytes)
            .map_err(|error| {
                crate::json_parse_error("decode native snapshot discriminators JSON", error)
            })?
            .0;
        // All scans are independent of typed field admission. In particular,
        // malformed objects or enums early in an array cannot hide later
        // upgrade requirements or cause a missing sidecar to select old state.
        if future_native_discriminator(&probe.native_world)
            || future_lifecycle_discriminator(&probe.native_lifecycle)
            || future_relationship_discriminator(&probe.native_relationships)
            || future_owner_import_discriminator(&probe.native_owner_import)
            || future_format(
                &probe.preserved_prospect["encoding"],
                crate::preservation::PRESERVED_ENCODING_VERSION,
            )
        {
            return Err(UnsupportedSchema.into());
        }
        Ok(())
    }

    fn probe_lifecycle_discriminators(bytes: &[u8]) -> Result<()> {
        // A format probe intentionally ignores the rest of the layout. Future
        // definitions may have entirely different slot and completion shapes.
        let format = serde_json::from_slice::<JsonObject<LifecycleFormatEnvelope>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native lifecycle fields JSON", error))
            .map(|probe| probe.0.native_lifecycle.definition.lifecycle_format);
        if format
            .as_ref()
            .ok()
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|format| {
                format > u64::from(crate::native_lifecycle::NATIVE_LIFECYCLE_FORMAT)
            })
        {
            return Err(UnsupportedSchema.into());
        }
        // Keep discriminator inspection separate from typed DTO decoding, so a
        // malformed known value cannot hide a future enum in another record.
        let lifecycle = serde_json::from_slice::<JsonObject<LifecycleProbe>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native lifecycle fields JSON", error))?
            .0
            .native_lifecycle;
        if future_lifecycle_discriminator(&lifecycle) {
            return Err(UnsupportedSchema.into());
        }
        format?;
        Ok(())
    }

    fn probe_lifecycle_native_discriminators(bytes: &[u8]) -> Result<()> {
        // Retain the separate format-first probe, including future catalogs
        // whose class list no longer has the current shape.
        let probe = Self::probe_native_discriminators(bytes);
        if probe
            .as_ref()
            .is_err_and(|error| error.is::<UnsupportedSchema>())
        {
            return Err(UnsupportedSchema.into());
        }
        let native = serde_json::from_slice::<JsonObject<LifecycleNativeProbe>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0
            .native_world;
        if future_native_discriminator(&native) {
            return Err(UnsupportedSchema.into());
        }
        probe
    }

    fn probe_native_discriminators(bytes: &[u8]) -> Result<()> {
        // Probe the format separately first: a future format is terminal even
        // when its class definitions no longer have the current shape.
        let format = serde_json::from_slice::<JsonObject<NativeProbe<NativeFormatProbe>>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0
            .native_world
            .definitions
            .native_format;
        if format
            .as_u64()
            .is_some_and(|format| format > u64::from(crate::native_world::NATIVE_FORMAT))
        {
            return Err(UnsupportedSchema.into());
        }
        let classes = serde_json::from_slice::<JsonObject<NativeProbe<NativeClassesProbe>>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0
            .native_world
            .definitions
            .classes;
        if classes.iter().any(|class| {
            class
                .kind
                .as_str()
                .is_some_and(|kind| kind != "spatial_object_v1")
        }) {
            return Err(UnsupportedSchema.into());
        }
        Ok(())
    }

    /// The committed facts a save needs, with the outcomes of a full read: the
    /// generation checksum and any sidecar size and digest are re-verified on
    /// every call; the preserved section is decoded only on a cache miss.
    fn probe_generation(&self, generation: &Generation) -> Result<Committed> {
        let bytes = self.read_checked(generation)?;
        let cached = self
            .verified
            .lock()
            .ok()
            .and_then(|verified| verified.get(&generation.sha256).cloned());
        if let Some(committed) = cached {
            if let Some(reference) = &committed.sidecar {
                ensure!(
                    sidecar_contents_match(&self.root.join(reference.sidecar_name()), reference)?,
                    "preserved sidecar size or checksum mismatch"
                );
            }
            return Ok(committed);
        }
        let (snapshot, sidecar) = self.decode_generation(&bytes)?;
        let committed = Committed {
            tick: snapshot.tick,
            sidecar,
        };
        self.remember(generation, committed.clone());
        Ok(committed)
    }

    fn remember(&self, generation: &Generation, committed: Committed) {
        if let Ok(mut verified) = self.verified.lock() {
            verified.insert(generation.sha256.clone(), committed);
        }
    }

    fn select(&self, manifest: &Manifest) -> Result<(WorldSnapshot, Generation, bool)> {
        self.select_with_reader(manifest, |generation| self.read_generation(generation))
    }

    // The reader boundary permits operational read faults without requiring a
    // failing disk or privileged permission changes in regression tests.
    fn select_with_reader<T>(
        &self,
        manifest: &Manifest,
        mut read: impl FnMut(&Generation) -> Result<T>,
    ) -> Result<(T, Generation, bool)> {
        match read(&manifest.current) {
            Ok(snapshot) => Ok((snapshot, manifest.current.clone(), false)),
            Err(error)
                if error.is::<UnsupportedSchema>()
                    || error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() != std::io::ErrorKind::NotFound) =>
            {
                // Inaccessible storage does not establish corrupt state. Keep
                // the acknowledged generation intact until its read can retry.
                Err(error)
            }
            Err(current_error) => match &manifest.previous {
                Some(previous) => {
                    let snapshot = read(previous).with_context(|| {
                        format!("both snapshots unusable; current: {current_error:#}")
                    })?;
                    Ok((snapshot, previous.clone(), true))
                }
                None => Err(current_error.context("no previous committed generation available")),
            },
        }
    }

    pub fn load(&self) -> Result<Option<LoadedSnapshot>> {
        self.manifest()?
            .map(|manifest| {
                let (snapshot, _, recovered_previous) = self.select(&manifest)?;
                Ok(LoadedSnapshot {
                    snapshot,
                    recovered_previous,
                })
            })
            .transpose()
    }

    pub fn save(&mut self, snapshot: &WorldSnapshot) -> Result<SaveReceipt> {
        self.save_with_writer(snapshot, atomic_write)
    }

    /// Publish exactly one durable initial generation. Existing committed
    /// worlds, including one recovered from its previous generation, are
    /// refused; malformed stores and orphan generations fail through `load`.
    pub fn save_new(&mut self, snapshot: &WorldSnapshot) -> Result<SaveReceipt> {
        ensure!(
            self.load()?.is_none(),
            "destination already contains a world; refusing to overwrite it"
        );
        self.save(snapshot)
    }

    // The writer boundary permits deterministic publication failures in tests.
    // A preserved snapshot loaded from schema 2 or 3 writes schema 3; a native
    // world stays schema 4, 5, 6, 7 or 8. Its sidecar is durable (data, rename
    // and directory synchronized) before the generation referencing it is
    // written, and a sidecar that already verifies is reused, never rewritten.
    fn save_with_writer(
        &mut self,
        snapshot: &WorldSnapshot,
        mut write: impl FnMut(&Path, &str, &[u8]) -> Result<()>,
    ) -> Result<SaveReceipt> {
        let (bytes, reference) = encode_generation(snapshot)?;
        let manifest = self.manifest()?;
        let (number, previous, previous_sidecar) = match &manifest {
            Some(manifest) => {
                let (committed, valid_generation, _) = self
                    .select_with_reader(manifest, |generation| self.probe_generation(generation))?;
                ensure!(
                    snapshot.tick >= committed.tick,
                    "stale snapshot tick {} precedes committed tick {}",
                    snapshot.tick,
                    committed.tick
                );
                (
                    manifest
                        .current
                        .number
                        .checked_add(1)
                        .context("snapshot generation overflow")?,
                    Some(valid_generation),
                    committed.sidecar,
                )
            }
            None => (1, None, None),
        };
        if let Some(reference) = &reference {
            if previous_sidecar.as_ref() == Some(reference) {
                // The probe just verified this sidecar's size and digest,
                // and the committed generation's publication made it durable.
                #[cfg(test)]
                durability_probe::record(|| {
                    durability_probe::Effect::ReuseSidecar(reference.sidecar_name())
                });
            } else {
                self.publish_sidecar(snapshot, reference, &mut write)?;
            }
        }
        let generation = Generation {
            number,
            sha256: checksum(&bytes),
        };
        // Replacing an uncommitted orphan at this number is safe: the manifest
        // still references only the previous committed generations.
        write(&self.root, &generation.filename(), &bytes)?;
        let next_manifest = Manifest {
            schema_version: 1,
            current: generation,
            previous,
        };
        write(
            &self.root,
            "manifest.json",
            &serde_json::to_vec(&next_manifest)?,
        )?;
        // The save is durable now. Bookkeeping and cleanup cannot revoke it.
        if let Ok(mut verified) = self.verified.lock() {
            verified.retain(|sha256, _| {
                next_manifest
                    .previous
                    .as_ref()
                    .is_some_and(|previous| previous.sha256 == *sha256)
            });
            // Serialized here from a validated snapshot whose section was
            // verified at construction, so a full decode would succeed.
            verified.insert(
                next_manifest.current.sha256.clone(),
                Committed {
                    tick: snapshot.tick,
                    sidecar: reference.clone(),
                },
            );
        }
        // Exactly the sidecars of the two retained generations survive.
        let retained_sidecars: Vec<String> = [&reference, &previous_sidecar]
            .into_iter()
            .flatten()
            .map(SectionReference::sidecar_name)
            .collect();
        let entries = fs::read_dir(&self.root).ok().into_iter().flatten();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A crash can strand an unpublished temporary; the lock makes this
            // the only writer, so any regular file with the prefix is stale.
            if name.starts_with(TEMPORARY_PREFIX)
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            // Only regular files with the exact content-addressed name are
            // collected; the new manifest is durable, so no retained
            // generation can reference a removed sidecar after a crash.
            if is_sidecar_name(&name) {
                if !retained_sidecars.contains(&name)
                    && entry.file_type().is_ok_and(|kind| kind.is_file())
                {
                    let _ = fs::remove_file(entry.path());
                }
                continue;
            }
            let number = name
                .strip_prefix("snapshot-")
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|number| number.parse::<u64>().ok());
            if let Some(number) = number
                && number != next_manifest.current.number
                && !next_manifest
                    .previous
                    .as_ref()
                    .is_some_and(|previous| previous.number == number)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(SaveReceipt {
            generation: number,
            tick: snapshot.tick,
        })
    }

    /// Make the snapshot's sidecar durable before anything references it. An
    /// existing regular file with the exact size and digest is kept, never
    /// rewritten; anything else under that name is atomically replaced.
    fn publish_sidecar(
        &self,
        snapshot: &WorldSnapshot,
        reference: &SectionReference,
        write: &mut impl FnMut(&Path, &str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let name = reference.sidecar_name();
        let path = self.root.join(&name);
        let existing = match sidecar_contents_match(&path, reference) {
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                false
            }
            outcome => outcome?,
        };
        if existing {
            // An earlier process may have exited before its data or entry
            // reached storage; make both durable before referencing them.
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(crate::nonblocking_flag())
                .open(&path)?;
            ensure!(
                file.metadata()?.is_file(),
                "preserved sidecar must be a regular file"
            );
            file.sync_all().context("synchronize preserved sidecar")?;
            sync_directory(&self.root)?;
            #[cfg(test)]
            durability_probe::record(|| durability_probe::Effect::ReuseSidecar(name.clone()));
            return Ok(());
        }
        let preserved = snapshot
            .preserved_prospect
            .as_ref()
            .context("preserved section required for its sidecar")?;
        // The held encoding was proven to decode at construction; it is written
        // as is, after a digest check against the identity it is named by.
        let encoded = preserved.native_encoding();
        ensure!(
            encoded.len() as u64 == reference.encoded_bytes()
                && checksum(encoded) == reference.encoded_sha256(),
            "preserved native encoding differs from its recorded identity"
        );
        write(&self.root, &name, encoded)
    }
}

/// Stream a sidecar through SHA-256 with a fixed buffer, so verification never
/// holds it whole. `Ok(false)` is corruption (wrong type, size or digest); a
/// missing file is a `NotFound` I/O error and other I/O errors stay operational.
fn sidecar_contents_match(path: &Path, reference: &SectionReference) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(crate::nonblocking_flag())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != reference.encoded_bytes() {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; VERIFY_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > reference.encoded_bytes() {
            return Ok(false);
        }
        hasher.update(&buffer[..read]);
    }
    Ok(total == reference.encoded_bytes()
        && crate::hex_lower(&hasher.finalize()) == reference.encoded_sha256())
}

fn checksum(bytes: &[u8]) -> String {
    crate::hex_lower(&Sha256::digest(bytes))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    // A byte limit does not bound opening a FIFO. Nonblocking open followed by
    // handle metadata also rejects special files swapped in during path lookup.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(crate::nonblocking_flag())
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "snapshot input must be a regular file: {}",
        path.display()
    );
    // Size the buffer once from the opened file, never beyond the limit, so a
    // large sidecar is not grown by doubling into a larger held allocation.
    let expected = metadata.len().min(limit);
    let mut bytes = Vec::with_capacity(usize::try_from(expected)?.saturating_add(1));
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "file exceeds size limit: {}",
        path.display()
    );
    Ok(bytes)
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?
        .sync_all()
        .context("synchronize snapshot directory")?;
    #[cfg(test)]
    durability_probe::record(|| durability_probe::Effect::SyncDirectory(path.to_path_buf()));
    Ok(())
}

/// Prefix of this server's unpublished temporaries; cleanup removes only these.
const TEMPORARY_PREFIX: &str = ".snapshot-write-";

fn atomic_write(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::Builder::new()
        .prefix(TEMPORARY_PREFIX)
        .tempfile_in(root)?;
    #[cfg(test)]
    let temporary_name = durability_probe::file_name(temporary.path());
    #[cfg(test)]
    durability_probe::record(|| durability_probe::Effect::CreateTemporary(temporary_name.clone()));
    temporary.write_all(bytes)?;
    #[cfg(test)]
    durability_probe::record(|| {
        durability_probe::Effect::WriteData(temporary_name.clone(), bytes.to_vec())
    });
    temporary.as_file().sync_all()?;
    #[cfg(test)]
    durability_probe::record(|| durability_probe::Effect::SyncFile(temporary_name.clone()));
    temporary
        .persist(root.join(name))
        .context("publish snapshot file")?;
    #[cfg(test)]
    durability_probe::record(|| durability_probe::Effect::Rename {
        from: temporary_name.clone(),
        to: name.to_owned(),
    });
    sync_directory(root)
}

/// Test-only trace of durability-relevant effects in the order the real code
/// issues them. Disabled unless a test enables it on its own thread.
#[cfg(test)]
mod durability_probe {
    use std::{
        cell::RefCell,
        path::{Path, PathBuf},
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum Effect {
        /// New directory entry for an empty inode, not durable until a directory sync.
        CreateTemporary(String),
        /// File data, not durable until a file sync.
        WriteData(String, Vec<u8>),
        SyncFile(String),
        /// Directory entry change, not durable until a directory sync.
        Rename {
            from: String,
            to: String,
        },
        SyncDirectory(PathBuf),
        /// Directory entry removal, not durable until a directory sync.
        Unlink(String),
        /// A save referenced this existing, verified sidecar instead of
        /// writing it; no storage change. The model checks the decision.
        ReuseSidecar(String),
    }

    thread_local! {
        static TRACE: RefCell<Option<Vec<Effect>>> = const { RefCell::new(None) };
    }

    pub(super) fn record(effect: impl FnOnce() -> Effect) {
        TRACE.with_borrow_mut(|trace| {
            if let Some(trace) = trace {
                trace.push(effect());
            }
        });
    }

    pub(super) fn file_name(path: &Path) -> String {
        path.file_name()
            .expect("temporary file has a name")
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn start() {
        TRACE.with_borrow_mut(|trace| *trace = Some(Vec::new()));
    }

    pub(super) fn drain() -> Vec<Effect> {
        TRACE.with_borrow_mut(|trace| trace.as_mut().map(std::mem::take).unwrap_or_default())
    }

    pub(super) fn stop() -> Vec<Effect> {
        TRACE.with_borrow_mut(|trace| trace.take().unwrap_or_default())
    }
}

/// One in-flight save; owned snapshots never borrow the live ECS world.
pub struct SaveWorker {
    sender: SyncSender<WorldSnapshot>,
    receiver: Receiver<Result<SaveReceipt>>,
    busy: bool,
}

/// Terminal failure: this worker cannot accept or acknowledge another save.
#[derive(Debug)]
pub struct SaveWorkerDisconnected;

impl std::fmt::Display for SaveWorkerDisconnected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("snapshot worker disconnected; persistence unavailable")
    }
}

impl std::error::Error for SaveWorkerDisconnected {}

impl SaveWorker {
    pub fn spawn(mut store: SnapshotStore) -> Result<Self> {
        let (sender, requests) = mpsc::sync_channel::<WorldSnapshot>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("snapshot-writer".into())
            .spawn(move || {
                while let Ok(snapshot) = requests.recv() {
                    if results.send(store.save(&snapshot)).is_err() {
                        break;
                    }
                }
            })
            .context("start snapshot worker")?;
        Ok(Self {
            sender,
            receiver,
            busy: false,
        })
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    pub fn submit(&mut self, snapshot: WorldSnapshot) -> Result<()> {
        ensure!(!self.busy, "snapshot worker busy");
        self.sender
            .try_send(snapshot)
            .map_err(|error| match error {
                mpsc::TrySendError::Disconnected(_) => anyhow::Error::new(SaveWorkerDisconnected),
                mpsc::TrySendError::Full(_) => anyhow::anyhow!("snapshot request queue full"),
            })?;
        self.busy = true;
        Ok(())
    }

    /// Check the budget after synchronous snapshot capture, before enqueueing.
    /// This does not cancel an already running write or make scheduling realtime.
    pub fn submit_before(&mut self, snapshot: WorldSnapshot, deadline: Instant) -> Result<()> {
        ensure!(
            Instant::now() < deadline,
            "snapshot shutdown deadline exceeded"
        );
        self.submit(snapshot)
    }

    pub fn poll(&mut self) -> Result<Option<SaveReceipt>> {
        self.poll_completion()?.transpose()
    }

    /// The outer error is the terminal disconnect; the inner one is the save's own.
    fn poll_completion(&mut self) -> Result<Option<Result<SaveReceipt>>> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.busy = false;
                Ok(Some(result))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.busy = false;
                Err(SaveWorkerDisconnected.into())
            }
        }
    }

    /// Success requires observing a pending completion within the budget. An idle
    /// worker has nothing to drain. An expired entry leaves queued results intact;
    /// expiry during polling returns no receipt even if that poll consumed one.
    /// Synchronous filesystem operations and scheduler pauses are not cancellable.
    pub fn finish_before(&mut self, deadline: Instant) -> Result<Option<SaveReceipt>> {
        self.settle_before(deadline)?.transpose()
    }

    /// Like `finish_before`, but returns the in-flight save's own failure as the
    /// inner result so shutdown can still attempt its final save. Deadline expiry
    /// and a disconnected worker remain outer errors.
    pub fn settle_before(&mut self, deadline: Instant) -> Result<Option<Result<SaveReceipt>>> {
        while self.busy {
            ensure!(
                Instant::now() < deadline,
                "snapshot shutdown deadline exceeded"
            );
            if let Some(result) = self.poll_completion()? {
                ensure!(
                    Instant::now() < deadline,
                    "snapshot shutdown deadline exceeded"
                );
                return Ok(Some(result));
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(1)),
            );
        }
        Ok(None)
    }
}

#[cfg(test)]
mod worker_failure_tests {
    use super::*;

    fn controlled_worker() -> (
        SaveWorker,
        Receiver<WorldSnapshot>,
        SyncSender<Result<SaveReceipt>>,
    ) {
        let (sender, requests) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::sync_channel(1);
        (
            SaveWorker {
                sender,
                receiver,
                busy: false,
            },
            requests,
            results,
        )
    }

    #[test]
    fn shutdown_deadline_rejects_late_ready_receipt_without_consuming_it() {
        let (mut worker, requests, results) = controlled_worker();
        worker.submit(WorldSnapshot::default()).unwrap();
        requests.recv().unwrap();
        let deadline = Instant::now();
        // The completion is created after the deadline, not merely consumed late.
        results
            .send(Ok(SaveReceipt {
                generation: 1,
                tick: 0,
            }))
            .unwrap();
        assert!(worker.finish_before(deadline).is_err());
        assert!(worker.is_busy());
        assert!(worker.submit(WorldSnapshot::default()).is_err());
        let receipt = worker.poll().unwrap().unwrap();
        assert_eq!(receipt.generation, 1);
        assert!(!worker.is_busy());
    }

    #[test]
    fn shutdown_deadline_pending_timeout_keeps_request_in_flight() {
        let (mut worker, requests, results) = controlled_worker();
        worker.submit(WorldSnapshot::default()).unwrap();
        requests.recv().unwrap();
        assert!(worker.finish_before(Instant::now()).is_err());
        assert!(worker.is_busy());
        assert!(worker.poll().unwrap().is_none());
        assert!(worker.submit(WorldSnapshot::default()).is_err());
        results
            .send(Ok(SaveReceipt {
                generation: 1,
                tick: 0,
            }))
            .unwrap();
        assert_eq!(
            worker
                .finish_before(Instant::now() + Duration::from_secs(5))
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        assert!(!worker.is_busy());
    }

    #[test]
    fn shutdown_settle_reports_failed_in_flight_save_and_accepts_final_save() {
        let (mut worker, requests, results) = controlled_worker();
        worker.submit(WorldSnapshot::default()).unwrap();
        requests.recv().unwrap();
        results
            .send(Err(anyhow::anyhow!("transient write failure")))
            .unwrap();
        let settled = worker
            .settle_before(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(settled.is_err());
        assert!(!worker.is_busy());
        worker.submit(WorldSnapshot::default()).unwrap();
        assert_eq!(requests.recv().unwrap().tick, 0);
    }

    #[test]
    fn shutdown_settle_keeps_deadline_and_disconnect_terminal() {
        let (mut worker, requests, results) = controlled_worker();
        worker.submit(WorldSnapshot::default()).unwrap();
        requests.recv().unwrap();
        assert!(worker.settle_before(Instant::now()).is_err());
        assert!(worker.is_busy());
        drop(results);
        assert!(
            worker
                .settle_before(Instant::now() + Duration::from_secs(5))
                .unwrap_err()
                .is::<SaveWorkerDisconnected>()
        );
    }

    #[test]
    fn shutdown_deadline_idle_worker_has_no_completion_to_wait_for() {
        let (mut worker, _requests, _results) = controlled_worker();
        assert!(worker.finish_before(Instant::now()).unwrap().is_none());
        assert!(!worker.is_busy());
    }

    #[test]
    fn shutdown_deadline_expired_submission_does_not_enqueue_final_snapshot() {
        let (mut worker, requests, _results) = controlled_worker();
        let deadline = Instant::now();
        // Models capture completing after its caller's earlier budget check.
        let snapshot = WorldSnapshot::default();
        assert!(worker.submit_before(snapshot, deadline).is_err());
        assert!(!worker.is_busy());
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
        worker
            .submit_before(
                WorldSnapshot::default(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert!(worker.is_busy());
        assert_eq!(requests.recv().unwrap().tick, 0);
    }

    #[test]
    fn disconnected_worker_clears_in_flight_state_and_returns_terminal_error() {
        let (sender, requests) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let mut worker = SaveWorker {
            sender,
            receiver,
            busy: false,
        };
        worker.submit(WorldSnapshot::default()).unwrap();
        // Model the writer exiting after receipt, before any acknowledgement.
        requests.recv().unwrap();
        drop(requests);
        drop(results);
        assert!(worker.poll().unwrap_err().is::<SaveWorkerDisconnected>());
        assert!(!worker.is_busy());
        assert!(
            worker
                .submit(WorldSnapshot::default())
                .unwrap_err()
                .is::<SaveWorkerDisconnected>()
        );
        assert!(!worker.is_busy());
        assert!(worker.poll().unwrap_err().is::<SaveWorkerDisconnected>());
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    fn snapshot(tick: u64) -> WorldSnapshot {
        WorldSnapshot {
            tick,
            ..Default::default()
        }
    }

    #[test]
    fn operational_current_read_faults_do_not_select_the_previous_generation() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path()).unwrap();
        let manifest = Manifest {
            schema_version: 1,
            current: Generation {
                number: 2,
                sha256: "a".repeat(64),
            },
            previous: Some(Generation {
                number: 1,
                sha256: "b".repeat(64),
            }),
        };
        for code in [13, 5] {
            let mut reads = Vec::new();
            let error = store
                .select_with_reader(&manifest, |generation| {
                    reads.push(generation.number);
                    if generation.number == 2 {
                        Err(anyhow::Error::new(std::io::Error::from_raw_os_error(code))
                            .context("injected current generation read"))
                    } else {
                        Ok(snapshot(10))
                    }
                })
                .unwrap_err();
            assert_eq!(reads, [2]);
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(code)
            );
        }
    }

    #[test]
    fn save_removes_stranded_own_temporaries_but_not_other_entries() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&snapshot(10)).unwrap();
        let stranded = directory.path().join(format!("{TEMPORARY_PREFIX}crashed"));
        fs::write(&stranded, b"unpublished").unwrap();
        let foreign = directory.path().join(".tmp-operator-note");
        fs::write(&foreign, b"keep").unwrap();
        let prefixed_directory = directory.path().join(format!("{TEMPORARY_PREFIX}dir"));
        fs::create_dir(&prefixed_directory).unwrap();
        store.save(&snapshot(20)).unwrap();
        assert!(!stranded.exists());
        assert!(foreign.exists());
        assert!(prefixed_directory.is_dir());
        assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(20));
    }

    #[test]
    fn disk_full_before_manifest_publication_preserves_acknowledged_generation() {
        for fail_at in ["snapshot-2.json", "manifest.json"] {
            let directory = tempfile::tempdir().unwrap();
            let mut store = SnapshotStore::open(directory.path()).unwrap();
            store.save(&snapshot(10)).unwrap();
            let error = store
                .save_with_writer(&snapshot(20), |root, name, bytes| {
                    if name == fail_at {
                        return Err(std::io::Error::from_raw_os_error(28).into()); // Linux ENOSPC
                    }
                    atomic_write(root, name, bytes)
                })
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(28)
            );
            drop(store);
            let mut reopened = SnapshotStore::open(directory.path()).unwrap();
            assert_eq!(reopened.load().unwrap().unwrap().snapshot.tick, 10);
            // A failed manifest publication may leave an orphan; retry replaces it.
            assert_eq!(reopened.save(&snapshot(30)).unwrap().generation, 2);
            assert_eq!(reopened.load().unwrap().unwrap().snapshot.tick, 30);
        }
    }

    #[test]
    fn error_after_publication_never_acknowledges_but_preserves_readable_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&snapshot(10)).unwrap();
        let outcome = store.save_with_writer(&snapshot(20), |root, name, bytes| {
            atomic_write(root, name, bytes)?;
            if name == "manifest.json" {
                // Models an uncertain result after publication. This is not a
                // physical power-loss or failing-filesystem qualification.
                anyhow::bail!("injected completion failure after publication");
            }
            Ok(())
        });
        assert!(outcome.is_err());
        drop(store);
        let mut reopened = SnapshotStore::open(directory.path()).unwrap();
        assert_eq!(reopened.load().unwrap().unwrap().snapshot.tick, 20);
        assert_eq!(reopened.save(&snapshot(30)).unwrap().generation, 3);
        fs::write(directory.path().join("snapshot-3.json"), b"corrupt").unwrap();
        assert_eq!(reopened.load().unwrap().unwrap().snapshot.tick, 20);
    }
}

#[cfg(test)]
fn native_snapshot(tick: u64) -> WorldSnapshot {
    let catalog = Arc::new(crate::native_world::CompiledNativeCatalog::parse(r#"
native_format = 1
frame = "right_handed_z_up_meters"
[[classes]]
key = "box"
kind = "spatial_object_v1"
local_bounds = { min = [-0.5, -0.5, -0.5], max = [0.5, 0.5, 0.5] }
variables = []
[[maps]]
key = "demo"
[[maps.tiles]]
key = "tile"
bounds = { min = [-10.0, -10.0, -10.0], max = [10.0, 10.0, 10.0] }
[[maps.placements]]
key = "box1"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#).unwrap());
    let prepared = crate::native_world::prepare_authored_world(catalog, "demo").unwrap();
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_NATIVE,
        tick,
        native_world: Some(prepared.snapshot().clone()),
        ..Default::default()
    }
}

/// Simulated power loss over the real save path (U-035, power-loss part).
///
/// The effect log comes from `durability_probe` inside `atomic_write` and
/// `sync_directory`, plus cleanup unlinks observed by diffing the directory
/// after every writer call. For each crash point the model exposes every state
/// a POSIX filesystem may leave: effects before the last directory sync are
/// durable; each later create/rename/unlink is independently kept or lost;
/// unsynced file data is whole, empty or half-written. Each state is
/// materialized and recovered through the real `open` + `load`.
#[cfg(test)]
mod crash_consistency_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::durability_probe::{self, Effect};
    use super::*;
    use crate::preservation::{PreserveLimits, PreservedSection};
    use crate::simulation::{DomainId, Position};

    /// File name to bytes, excluding the lock file that `open` recreates.
    type DiskState = BTreeMap<String, Vec<u8>>;

    const LOCK: &str = "server.lock";
    /// Bounds the subset enumeration at 2^12 directory states per crash point.
    const MAX_PENDING_DIRECTORY_EFFECTS: usize = 12;

    enum Step {
        Effect(Effect),
        Attempt(WorldSnapshot),
        Acknowledge(WorldSnapshot),
        /// Durable external damage lowers the floor to the generation it leaves.
        Damage(WorldSnapshot),
        /// Generations numbered at most this may reference a sidecar that
        /// injected damage removed; later generations are always checked.
        Exempt(u64),
    }

    struct Trace {
        root: PathBuf,
        base: DiskState,
        /// Durable recovered snapshot the trace starts from, if any.
        initial: Option<WorldSnapshot>,
        /// Sidecar-check exemption in effect at the start of the trace.
        exempt: u64,
        /// Snapshots attempted in enclosing traces. Rewriting a lost sidecar
        /// makes generations that reference it readable again, so one of
        /// these may load (never below the floor) until the next publication.
        history: Vec<WorldSnapshot>,
        steps: Vec<Step>,
    }

    #[derive(Debug, Default)]
    struct Counts {
        crash_points: usize,
        states: usize,
        empty_worlds: usize,
        recovered_previous: usize,
        refused_orphans: usize,
        /// Intact manifest-referenced schema-3/4/5/6/7/8 generations whose sidecar was
        /// present with its recorded size and digest.
        referenced_sidecars: usize,
        /// Loads of a snapshot from an enclosing trace that a rewritten
        /// sidecar made readable again; never below the floor.
        resurrected: usize,
    }

    #[derive(Debug, Default)]
    struct Tally {
        main: Counts,
        after_recovery: Counts,
        recovery_saves: usize,
    }

    type Writer = fn(&Path, &str, &[u8]) -> Result<()>;

    fn snapshot(tick: u64) -> WorldSnapshot {
        WorldSnapshot {
            tick,
            entities: (1..=tick / 10 + 1)
                .map(|id| EntityRecord {
                    id: DomainId(id),
                    position: Position([id as f32, 0.5, -(tick as f32)]),
                })
                .collect(),
            ..Default::default()
        }
    }

    // Authored framing only; every name and value is invented for this model.
    fn string(value: &str) -> Vec<u8> {
        let length = i32::try_from(value.len() + 1).unwrap().to_le_bytes();
        [length.as_slice(), value.as_bytes(), &[0]].concat()
    }

    fn tag(name: &str, kind: &str, extra: &[u8], value: &[u8]) -> Vec<u8> {
        let length = i32::try_from(value.len()).unwrap().to_le_bytes();
        [
            string(name).as_slice(),
            &string(kind),
            &length,
            &[0; 4],
            extra,
            &[0],
            value,
        ]
        .concat()
    }

    /// One recorder whose opaque body is `variant` repeated, so each variant
    /// is a distinct section with a distinct sidecar.
    fn section(variant: u8) -> Arc<PreservedSection> {
        let body = vec![variant; 8];
        let binary = [
            i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
            &body,
        ]
        .concat();
        let record = [
            tag(
                "ComponentClassName",
                "StrProperty",
                &[],
                &string("UModelAuthoredClass"),
            ),
            tag(
                "BinaryData",
                "ArrayProperty",
                &string("ByteProperty"),
                &binary,
            ),
            string("None"),
        ]
        .concat();
        let shared = tag(
            "StateRecorderBlobs",
            "StructProperty",
            &[string("StateRecorderBlob"), vec![7; 16]].concat(),
            &record,
        );
        let array = [1_i32.to_le_bytes().as_slice(), &shared].concat();
        let payload = [
            tag(
                "StateRecorderBlobs",
                "ArrayProperty",
                &string("StructProperty"),
                &array,
            ),
            string("None"),
        ]
        .concat();
        Arc::new(PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap())
    }

    fn preserved(tick: u64, section: &Arc<PreservedSection>) -> WorldSnapshot {
        WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_REFERENCED,
            preserved_prospect: Some(Arc::clone(section)),
            ..snapshot(tick)
        }
    }

    fn sidecar_name(section: &PreservedSection) -> String {
        SectionReference::of(section).sidecar_name()
    }

    fn native(tick: u64, section: Option<&Arc<PreservedSection>>) -> WorldSnapshot {
        WorldSnapshot {
            preserved_prospect: section.cloned(),
            ..native_snapshot(tick)
        }
    }

    fn lifecycle(tick: u64, section: Option<&Arc<PreservedSection>>, rounds: u64) -> WorldSnapshot {
        use crate::native_lifecycle::{
            CompiledNativeLifecycle, LifecycleDefinition, Presence, RecorderComponentRecord,
            RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
            SlotDeclaration, SpawnComponent, prepare_lifecycle_world, prepare_owner_restore,
        };
        let base = native(tick, section);
        let spatial = base.native_world.as_ref().unwrap();
        let catalog =
            crate::native_world::CompiledNativeCatalog::compile(spatial.definitions.clone())
                .unwrap();
        let compiled = CompiledNativeLifecycle::compile(
            LifecycleDefinition {
                lifecycle_format: 1,
                native_catalog_id: catalog.identity().into(),
                map: spatial.map.clone(),
                slots: vec![SlotDeclaration {
                    placement: spatial.actors[0].placement.clone(),
                    owner_name: vec![65],
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: Presence::Live,
                    initial_component: Some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: 1,
                    }),
                    spawn_component: SpawnComponent::DeclaredKind,
                }],
            },
            &catalog,
        )
        .unwrap();
        let mut state = prepare_lifecycle_world(base, &compiled)
            .unwrap()
            .snapshot()
            .clone();
        for sequence in 1..=rounds {
            let lifecycle = state.native_lifecycle.as_ref().unwrap();
            let actor = &state.native_world.as_ref().unwrap().actors[0];
            let mut transform = actor.transform.clone();
            transform.translation[0] = sequence as f32 / 10.0;
            let batch = RestoreBatch {
                lifecycle_id: lifecycle.lifecycle_id.clone(),
                sequence,
                recorders: vec![RestoreRecorder {
                    guid: 10 + i32::try_from(sequence).unwrap(),
                    kind: RecorderKind::SpatialStateV1,
                    target_placement: actor.placement.clone(),
                    owner_name: vec![65],
                    policy: ResolvePolicy::RespawnOnly,
                    source: RecorderSource::Detached {
                        expected_prior_binding: None,
                    },
                    create_if_missing: false,
                    expected_presence: Presence::Live,
                    expected_incarnation: lifecycle.slots[0].incarnation,
                    transform,
                    variables: actor.variables.clone(),
                }],
            };
            state = prepare_owner_restore(&state, &batch)
                .unwrap()
                .snapshot()
                .clone();
        }
        state
    }

    fn relationships(
        tick: u64,
        section: Option<&Arc<PreservedSection>>,
        target: Option<DomainId>,
    ) -> WorldSnapshot {
        use crate::native_relationships::{
            CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
            NativeRelationshipRole, NativeRelationshipsDefinition,
        };
        let mut definition = native_snapshot(0).native_world.unwrap().definitions;
        let placement = definition.maps[0].placements[0].clone();
        for key in ["box2", "box3"] {
            let mut added = placement.clone();
            added.key = key.into();
            definition.maps[0].placements.push(added);
        }
        let catalog =
            Arc::new(crate::native_world::CompiledNativeCatalog::compile(definition).unwrap());
        let mut native = crate::native_world::prepare_authored_world(Arc::clone(&catalog), "demo")
            .unwrap()
            .snapshot()
            .clone();
        native.actors[0].transform.translation[0] = tick as f32 / 100.0;
        let link = |guid| NativeActorLinkV1 {
            name: vec![65],
            guid,
        };
        let model = CompiledNativeRelationships::compile(
            NativeRelationshipsDefinition {
                relationship_format: 1,
                native_catalog_id: catalog.identity().into(),
                map: "demo".into(),
                actors: native
                    .actors
                    .iter()
                    .enumerate()
                    .map(|(index, actor)| NativeRelationshipActorV1 {
                        placement: actor.placement.clone(),
                        identity: link(i32::try_from(index + 1).unwrap()),
                        role: if index == 0 {
                            NativeRelationshipRole::Deployable
                        } else {
                            NativeRelationshipRole::Foundation
                        },
                        foundation: (index == 0).then(|| link(2)),
                    })
                    .collect(),
            },
            &catalog,
        )
        .unwrap();
        let mut state = model.initial_snapshot();
        model
            .reassign_foundation(&mut state, DomainId(1), target)
            .unwrap();
        WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_RELATIONSHIPS,
            tick,
            native_world: Some(native),
            native_relationships: Some(state),
            preserved_prospect: section.cloned(),
            ..Default::default()
        }
    }

    /// Complete authored C001 source; metadata is bound to its actual bytes.
    fn inert(tick: u64, variant: u8) -> WorldSnapshot {
        let structure = |name: &str, kind: &str, value: &[u8]| {
            tag(
                name,
                "StructProperty",
                &[string(kind), vec![0; 16]].concat(),
                value,
            )
        };
        let floats = |values: &[f32]| {
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>()
        };
        let transform = [
            structure("Translation", "Vector", &floats(&[0.0; 3])),
            structure("Rotation", "Quat", &floats(&[0.0, 0.0, 0.0, 1.0])),
            structure("Scale3D", "Vector", &floats(&[1.0; 3])),
            string("None"),
        ]
        .concat();
        let path = format!("/Authored/Model{variant}");
        let body = [
            tag("ActorClassName", "NameProperty", &[], &string("ModelClass")),
            tag("ActorPathName", "StrProperty", &[], &string(&path)),
            tag("ObjectFName", "NameProperty", &[], &string("ModelObject")),
            tag(
                "IcarusActorGUID",
                "IntProperty",
                &[],
                &i32::from(variant).to_le_bytes(),
            ),
            tag("RecordedVersion", "IntProperty", &[], &3_i32.to_le_bytes()),
            tag("bWasMovedToSubLevel", "BoolProperty", &[0], &[]),
            structure("ActorTransform", "Transform", &transform),
            string("None"),
            vec![0; 4],
        ]
        .concat();
        let record = [
            tag(
                "ComponentClassName",
                "StrProperty",
                &[],
                &string("/Script/Icarus.ActorStateRecorderComponent"),
            ),
            tag(
                "BinaryData",
                "ArrayProperty",
                &string("ByteProperty"),
                &[
                    i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
                    &body,
                ]
                .concat(),
            ),
            string("None"),
        ]
        .concat();
        let shared = structure("StateRecorderBlobs", "StateRecorderBlob", &record);
        let info = [
            tag("ProspectID", "StrProperty", &[], &string("ModelProspect")),
            string("None"),
        ]
        .concat();
        let payload = [
            tag("Version", "IntProperty", &[], &2_i32.to_le_bytes()),
            structure("ProspectInfo", "ProspectInfo", &info),
            tag("ProspectMapName", "StrProperty", &[], &string("ModelMap")),
            tag(
                "StateRecorderBlobs",
                "ArrayProperty",
                &string("StructProperty"),
                &[1_i32.to_le_bytes().as_slice(), &shared].concat(),
            ),
            string("None"),
        ]
        .concat();
        let source = Arc::new(
            PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap(),
        );
        let mut snapshot = native_snapshot(tick);
        let native = snapshot.native_world.as_ref().unwrap();
        let units = |value: &str| value.encode_utf16().collect::<Vec<_>>();
        snapshot.native_import = Some(serde_json::from_value(serde_json::json!({
            "format": 1, "adapter": "c001_inert_placement_v1",
            "coordinates": "right_handed_z_up_meters_quat_xyzw_world_space",
            "source_version": 2, "source_payload_sha256": crate::hex_lower(&source.payload_sha256()),
            "native_catalog_id": native.catalog_id, "native_map": native.map,
            "source_map": units("ModelMap"), "source_prospect_id": units("ModelProspect"),
            "source_lobby_name": [], "source_last_saved_date_time": null,
            "root_presence": {
                "version": true, "lobby_privacy": false, "prospect_info": true,
                "prospect_map_name": true, "last_saved_date_time": false,
                "legacy_prospect_id": false, "legacy_prospect_dt_key": false,
                "state_recorder_blobs": true
            },
            "owners": [{
                "source_index": 0, "id": native.actors[0].id,
                "source_actor_class": units("ModelClass"), "source_actor_path": units(&path),
                "source_object_name": units("ModelObject"), "source_icarus_guid": i32::from(variant),
                "native_placement": native.actors[0].placement, "native_class": native.actors[0].class
            }]
        })).unwrap());
        snapshot.schema_version = SNAPSHOT_SCHEMA_C001_INERT;
        snapshot.preserved_prospect = Some(source);
        snapshot
    }

    /// Independently authored provenance with an initial detached live attach.
    /// Source variants exercise sidecar replacement, reuse and collection.
    fn scalar_owners(tick: u64, variant: u8, remaining: i32) -> WorldSnapshot {
        let mut state = crate::scalar_owner_fixture::imported_variant(variant);
        state.tick = tick;
        state.native_world.as_mut().unwrap().actors[0].variables =
            crate::scalar_owner_fixture::scalar_pair(true, remaining);
        state.validate().unwrap();
        state
    }

    fn owners(tick: u64, variant: u8, rounds: u64) -> WorldSnapshot {
        use crate::native_c001_owners::{
            C001OwnerAdapter, C001OwnerBindingV1, C001OwnerImportMetadataV1, ResolvedC001OwnerV1,
        };
        use crate::native_lifecycle::{
            Presence, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
            prepare_owner_restore,
        };
        let source = inert(tick, variant);
        let old = source.native_import.unwrap();
        let mut state = lifecycle(tick, None, 0);
        let spatial = state.native_world.as_ref().unwrap();
        let life = state.native_lifecycle.as_ref().unwrap();
        let binding = C001OwnerBindingV1 {
            source_actor_class: old.owners[0].source_actor_class.clone(),
            source_actor_path: old.owners[0].source_actor_path.clone(),
            source_object_name: old.owners[0].source_object_name.clone(),
            source_icarus_guid: old.owners[0].source_icarus_guid,
            native_placement: spatial.actors[0].placement.clone(),
            native_class: spatial.actors[0].class.clone(),
            native_owner_name: vec![65],
            native_recorder_guid: 100 + i32::from(variant),
            create_if_missing: false,
            expected_presence: Presence::Live,
            expected_incarnation: 1,
            expected_prior_binding: None,
        };
        let batch = RestoreBatch {
            lifecycle_id: life.lifecycle_id.clone(),
            sequence: 1,
            recorders: vec![RestoreRecorder {
                guid: binding.native_recorder_guid,
                kind: RecorderKind::SpatialStateV1,
                target_placement: binding.native_placement.clone(),
                owner_name: binding.native_owner_name.clone(),
                policy: ResolvePolicy::FindOrRespawn,
                source: RecorderSource::Detached {
                    expected_prior_binding: None,
                },
                create_if_missing: binding.create_if_missing,
                expected_presence: binding.expected_presence,
                expected_incarnation: binding.expected_incarnation,
                transform: spatial.actors[0].transform.clone(),
                variables: Vec::new(),
            }],
        };
        let lifecycle_id = life.lifecycle_id.clone();
        state = prepare_owner_restore(&state, &batch)
            .unwrap()
            .snapshot()
            .clone();
        let initial_completion = state
            .native_lifecycle
            .as_ref()
            .unwrap()
            .completed
            .clone()
            .unwrap();
        state.native_owner_import = Some(C001OwnerImportMetadataV1 {
            format: 1,
            adapter: C001OwnerAdapter::C001AuthoredOwnersV1,
            source_version: old.source_version,
            source_payload_sha256: old.source_payload_sha256,
            native_catalog_id: old.native_catalog_id,
            native_lifecycle_id: lifecycle_id,
            native_map: old.native_map,
            coordinates: old.coordinates,
            source_map: old.source_map,
            source_prospect_id: old.source_prospect_id,
            source_lobby_name: old.source_lobby_name,
            source_last_saved_date_time: old.source_last_saved_date_time,
            root_presence: old.root_presence,
            owners: vec![ResolvedC001OwnerV1 {
                source_index: 0,
                id: old.owners[0].id,
                binding,
                source_policy: ResolvePolicy::FindOrRespawn,
            }],
            initial_completion,
        });
        state.preserved_prospect = source.preserved_prospect;
        state.schema_version = SNAPSHOT_SCHEMA_C001_OWNERS;
        for sequence in 2..=rounds {
            let lifecycle = state.native_lifecycle.as_ref().unwrap();
            let actor = &state.native_world.as_ref().unwrap().actors[0];
            let mut recorder = batch.recorders[0].clone();
            recorder.guid = 1000 + i32::try_from(sequence).unwrap();
            recorder.policy = ResolvePolicy::RespawnOnly;
            recorder.expected_incarnation = lifecycle.slots[0].incarnation;
            recorder.transform = actor.transform.clone();
            recorder.transform.translation[0] = sequence as f32 / 10.0;
            let request = RestoreBatch {
                lifecycle_id: lifecycle.lifecycle_id.clone(),
                sequence,
                recorders: vec![recorder],
            };
            state = prepare_owner_restore(&state, &request)
                .unwrap()
                .snapshot()
                .clone();
        }
        state
    }

    #[test]
    fn relationship_preflight_and_publication_share_exact_generation_encoding() {
        assert_eq!(MAX_REFERENCED_SNAPSHOT_BYTES, 64 * 1024 * 1024 + 4 * 1024);
        for preserve in [false, true] {
            let source = section(15);
            let state = relationships(10, preserve.then_some(&source), Some(DomainId(3)));
            SnapshotStore::preflight(&state).unwrap();
            let (encoded, reference) = encode_generation(&state).unwrap();
            assert_eq!(
                encoded,
                serde_json::to_vec(&DedicatedRelationshipsOut {
                    schema_version: SNAPSHOT_SCHEMA_RELATIONSHIPS,
                    tick: state.tick,
                    entities: &state.entities,
                    native_world: state.native_world.as_ref().unwrap(),
                    native_relationships: state.native_relationships.as_ref().unwrap(),
                    preserved_prospect: reference.as_ref(),
                    native_import: None,
                })
                .unwrap()
            );
            let directory = tempfile::tempdir().unwrap();
            let mut store = SnapshotStore::open(directory.path()).unwrap();
            store.save(&state).unwrap();
            assert_eq!(
                fs::read(directory.path().join("snapshot-1.json")).unwrap(),
                encoded
            );
        }
    }

    #[test]
    fn lifecycle_preflight_and_publication_share_exact_bounded_encoding() {
        assert_eq!(MAX_REFERENCED_SNAPSHOT_BYTES, 64 * 1024 * 1024 + 4 * 1024);
        for preserve in [false, true] {
            let source = section(16);
            let state = lifecycle(10, preserve.then_some(&source), 1);
            SnapshotStore::preflight(&state).unwrap();
            let (encoded, reference) = encode_generation(&state).unwrap();
            assert_eq!(
                encoded,
                serde_json::to_vec(&DedicatedLifecycleOut {
                    schema_version: SNAPSHOT_SCHEMA_LIFECYCLE,
                    tick: state.tick,
                    entities: &state.entities,
                    native_world: state.native_world.as_ref().unwrap(),
                    native_lifecycle: state.native_lifecycle.as_ref().unwrap(),
                    native_relationships: None,
                    preserved_prospect: reference.as_ref(),
                })
                .unwrap()
            );
            let directory = tempfile::tempdir().unwrap();
            let mut store = SnapshotStore::open(directory.path()).unwrap();
            store.save(&state).unwrap();
            assert_eq!(
                fs::read(directory.path().join("snapshot-1.json")).unwrap(),
                encoded
            );
        }
    }

    #[test]
    fn owner_import_preflight_and_publication_share_exact_aggregate_encoding() {
        assert_eq!(MAX_REFERENCED_SNAPSHOT_BYTES, 64 * 1024 * 1024 + 4 * 1024);
        let state = owners(10, 1, 2);
        SnapshotStore::preflight(&state).unwrap();
        let (encoded, reference) = encode_generation(&state).unwrap();
        let dto = C001OwnersOut {
            schema_version: SNAPSHOT_SCHEMA_C001_OWNERS,
            tick: state.tick,
            entities: &state.entities,
            native_world: state.native_world.as_ref().unwrap(),
            native_lifecycle: state.native_lifecycle.as_ref().unwrap(),
            native_owner_import: state.native_owner_import.as_ref().unwrap(),
            preserved_prospect: reference.as_ref().unwrap(),
            native_relationships: None,
        };
        assert_eq!(encoded, serde_json::to_vec(&dto).unwrap());
        assert_eq!(
            encode_bounded_generation(&dto, encoded.len() as u64).unwrap(),
            encoded
        );
        assert!(encode_bounded_generation(&dto, encoded.len() as u64 - 1).is_err());
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&state).unwrap();
        assert_eq!(
            fs::read(directory.path().join("snapshot-1.json")).unwrap(),
            encoded
        );
        assert_eq!(store.load().unwrap().unwrap().snapshot, state);
    }

    #[test]
    fn pre_owner_storage_dtos_reject_even_null_owner_metadata_in_otherwise_valid_generations() {
        let source = section(22);
        let mut inline = preserved(10, &source);
        inline.schema_version = SNAPSHOT_SCHEMA_PRESERVED;
        let states = [
            snapshot(10),
            inline,
            preserved(10, &source),
            native(10, Some(&source)),
            inert(10, 1),
            relationships(10, Some(&source), Some(DomainId(2))),
            lifecycle(10, Some(&source), 1),
        ];
        for state in states {
            let directory = tempfile::tempdir().unwrap();
            let store = SnapshotStore::open(directory.path()).unwrap();
            let (mut bytes, reference) = encode_generation(&state).unwrap();
            if let Some(reference) = reference {
                fs::write(
                    directory.path().join(reference.sidecar_name()),
                    state.preserved_prospect.as_ref().unwrap().native_encoding(),
                )
                .unwrap();
            }
            if state.schema_version == SNAPSHOT_SCHEMA_PRESERVED {
                bytes = serde_json::to_vec(&state).unwrap();
            }
            assert_eq!(store.decode_generation(&bytes).unwrap().0, state);
            let mut wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            wire["native_owner_import"] = serde_json::Value::Null;
            let error = store
                .decode_generation(&serde_json::to_vec(&wire).unwrap())
                .unwrap_err();
            assert!(!error.is::<UnsupportedSchema>());
        }
    }

    #[test]
    fn scalar_owner_preflight_counts_the_complete_generation_before_encoding() {
        let state = scalar_owners(10, 1, 8);
        let (encoded, reference) = encode_generation(&state).unwrap();
        let dto = C001ScalarOwnersOut {
            schema_version: SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS,
            tick: state.tick,
            entities: &state.entities,
            native_world: state.native_world.as_ref().unwrap(),
            native_lifecycle: state.native_lifecycle.as_ref().unwrap(),
            native_scalar_contracts: state.native_scalar_contracts.as_ref().unwrap(),
            native_scalar_owner_import: state.native_scalar_owner_import.as_ref().unwrap(),
            preserved_prospect: reference.as_ref().unwrap(),
            native_relationships: state.native_relationships.as_ref(),
        };
        assert_eq!(
            encode_bounded_generation(&dto, encoded.len() as u64).unwrap(),
            encoded
        );
        assert!(encode_bounded_generation(&dto, encoded.len() as u64 - 1).is_err());
        assert_eq!(serde_json::to_vec(&dto).unwrap(), encoded);
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&state).unwrap();
        assert_eq!(
            fs::read(directory.path().join("snapshot-1.json")).unwrap(),
            encoded
        );
        assert_eq!(store.load().unwrap().unwrap().snapshot, state);
    }

    #[test]
    fn native_publication_fault_matrix_preserves_committed_state() {
        let (first, second) = (section(5), section(6));
        for mode in 0..9 {
            let preserve = matches!(mode, 1 | 2 | 4 | 6 | 7 | 8);
            for fail_at in ["sidecar", "snapshot-2.json", "manifest.json"] {
                if fail_at == "sidecar" && !preserve {
                    continue;
                }
                for after_write in [false, true] {
                    let directory = tempfile::tempdir().unwrap();
                    let root = directory.path();
                    let before = if mode == 8 {
                        scalar_owners(10, 5, 3)
                    } else if mode == 7 {
                        owners(10, 5, 1)
                    } else if mode == 2 {
                        inert(10, 5)
                    } else if mode >= 5 {
                        lifecycle(10, preserve.then_some(&first), 0)
                    } else if mode >= 3 {
                        relationships(10, preserve.then_some(&first), Some(DomainId(2)))
                    } else {
                        native(10, preserve.then_some(&first))
                    };
                    let attempted = if mode == 8 {
                        scalar_owners(20, 6, 2)
                    } else if mode == 7 {
                        owners(20, 6, 2)
                    } else if mode == 2 {
                        inert(20, 6)
                    } else if mode >= 5 {
                        lifecycle(20, preserve.then_some(&second), 1)
                    } else if mode >= 3 {
                        relationships(20, preserve.then_some(&second), Some(DomainId(3)))
                    } else {
                        native(20, preserve.then_some(&second))
                    };
                    let mut store = SnapshotStore::open(root).unwrap();
                    store.save(&before).unwrap();
                    let error = store
                        .save_with_writer(&attempted, |root, name, bytes| {
                            let fail =
                                name == fail_at || (fail_at == "sidecar" && is_sidecar_name(name));
                            if fail && !after_write {
                                return Err(std::io::Error::from_raw_os_error(28).into());
                            }
                            atomic_write(root, name, bytes)?;
                            if fail {
                                return Err(std::io::Error::from_raw_os_error(28).into());
                            }
                            Ok(())
                        })
                        .unwrap_err();
                    assert_eq!(
                        error
                            .downcast_ref::<std::io::Error>()
                            .unwrap()
                            .raw_os_error(),
                        Some(28)
                    );
                    drop(store);
                    let mut reopened = SnapshotStore::open(root).unwrap();
                    let published = after_write && fail_at == "manifest.json";
                    let expected = if published { &attempted } else { &before };
                    assert_eq!(&reopened.load().unwrap().unwrap().snapshot, expected);
                    let next = if mode == 8 {
                        scalar_owners(30, 6, 1)
                    } else if mode == 7 {
                        owners(30, 6, 3)
                    } else if mode == 2 {
                        inert(30, 6)
                    } else if mode >= 5 {
                        lifecycle(30, preserve.then_some(&second), 2)
                    } else if mode >= 3 {
                        relationships(30, preserve.then_some(&second), None)
                    } else {
                        native(30, preserve.then_some(&second))
                    };
                    assert_eq!(
                        reopened.save(&next).unwrap().generation,
                        if published { 3 } else { 2 }
                    );
                    assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
                }
            }
        }
    }

    #[test]
    fn legacy_schemas_reject_scalar_fields_even_when_null() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(directory.path()).unwrap();
        let source = section(19);
        for state in [
            WorldSnapshot::default(),
            WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PRESERVED,
                preserved_prospect: Some(Arc::clone(&source)),
                ..WorldSnapshot::default()
            },
            WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_REFERENCED,
                preserved_prospect: Some(Arc::clone(&source)),
                ..WorldSnapshot::default()
            },
            native(10, None),
            inert(10, 1),
            relationships(10, None, None),
            lifecycle(10, None, 0),
            owners(10, 1, 1),
        ] {
            // Publish any required sidecar before exercising the original valid decoder.
            if let Some(preserved) = &state.preserved_prospect {
                atomic_write(
                    directory.path(),
                    &sidecar_name(preserved),
                    preserved.native_encoding(),
                )
                .unwrap();
            }
            let mut bytes = encode_generation(&state).unwrap().0;
            if state.schema_version == SNAPSHOT_SCHEMA_PRESERVED {
                bytes = serde_json::to_vec(&state).unwrap();
            }
            assert_eq!(store.decode_generation(&bytes).unwrap().0, state);
            for field in ["native_scalar_contracts", "native_scalar_owner_import"] {
                let mut wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                wire[field] = serde_json::Value::Null;
                let error = store
                    .decode_generation(&serde_json::to_vec(&wire).unwrap())
                    .unwrap_err();
                assert!(!error.is::<UnsupportedSchema>());
            }
        }
    }

    /// Every intact, manifest-referenced schema-3/4/5/6/7/8/9 generation must find its
    /// sidecar present with the recorded size and digest, whether or not the
    /// generation is ultimately selected by recovery.
    fn referenced_sidecars(state: &DiskState, exempt: u64) -> Result<usize, String> {
        let Some(manifest) = state
            .get("manifest.json")
            .and_then(|bytes| serde_json::from_slice::<Manifest>(bytes).ok())
        else {
            return Ok(0);
        };
        let mut intact = 0;
        for generation in std::iter::once(&manifest.current).chain(&manifest.previous) {
            let Some(bytes) = state.get(&generation.filename()) else {
                continue;
            };
            if checksum(bytes) != generation.sha256 {
                continue;
            }
            let reference = match serde_json::from_slice::<SnapshotHeader>(bytes)
                .ok()
                .and_then(|header| header.schema_version.as_u64())
            {
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_REFERENCED) => {
                    serde_json::from_slice::<ReferencedIn>(bytes)
                        .ok()
                        .map(|referenced| referenced.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_NATIVE) => {
                    serde_json::from_slice::<NativeIn>(bytes)
                        .ok()
                        .and_then(|native| native.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_INERT) => {
                    serde_json::from_slice::<C001InertIn>(bytes)
                        .ok()
                        .map(|native| native.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_RELATIONSHIPS) => {
                    serde_json::from_slice::<DedicatedRelationshipsIn>(bytes)
                        .ok()
                        .and_then(|native| native.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_LIFECYCLE) => {
                    serde_json::from_slice::<DedicatedLifecycleIn>(bytes)
                        .ok()
                        .and_then(|native| native.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_OWNERS) => {
                    serde_json::from_slice::<C001OwnersIn>(bytes)
                        .ok()
                        .map(|native| native.preserved_prospect)
                }
                Some(schema) if schema == u64::from(SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS) => {
                    serde_json::from_slice::<C001ScalarOwnersIn>(bytes)
                        .ok()
                        .map(|native| native.preserved_prospect)
                }
                _ => None,
            };
            let Some(reference) = reference else {
                continue;
            };
            let present = state.get(&reference.sidecar_name()).is_some_and(|sidecar| {
                sidecar.len() as u64 == reference.encoded_bytes()
                    && checksum(sidecar) == reference.encoded_sha256()
            });
            if !present && generation.number <= exempt {
                continue;
            }
            if !present {
                return Err(format!(
                    "generation {} references a missing or damaged sidecar",
                    generation.number
                ));
            }
            intact += 1;
        }
        Ok(intact)
    }

    fn listing(root: &Path) -> BTreeSet<String> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name != LOCK)
            .collect()
    }

    /// Moves probed effects into the log and records unprobed removals as
    /// unlinks at this point; unprobed creations fail the test.
    fn drain_into(root: &Path, steps: &mut Vec<Step>, names: &mut BTreeSet<String>) {
        for effect in durability_probe::drain() {
            match &effect {
                Effect::CreateTemporary(name) => {
                    names.insert(name.clone());
                }
                Effect::Rename { from, to } => {
                    names.remove(from);
                    names.insert(to.clone());
                }
                Effect::Unlink(name) => {
                    names.remove(name);
                }
                Effect::WriteData(..)
                | Effect::SyncFile(_)
                | Effect::SyncDirectory(_)
                | Effect::ReuseSidecar(_) => {}
            }
            steps.push(Step::Effect(effect));
        }
        let actual = listing(root);
        let unobserved: Vec<_> = actual.difference(names).collect();
        assert!(unobserved.is_empty(), "unprobed entries: {unobserved:?}");
        for removed in names.difference(&actual) {
            steps.push(Step::Effect(Effect::Unlink(removed.clone())));
        }
        *names = actual;
    }

    fn traced_save(
        store: &mut SnapshotStore,
        root: &Path,
        snapshot: &WorldSnapshot,
        steps: &mut Vec<Step>,
        writer: Writer,
    ) -> Result<SaveReceipt> {
        steps.push(Step::Attempt(snapshot.clone()));
        let mut names = listing(root);
        durability_probe::start();
        let outcome = store.save_with_writer(snapshot, |root, name, bytes| {
            let written = writer(root, name, bytes);
            drain_into(root, steps, &mut names);
            written
        });
        drain_into(root, steps, &mut names);
        durability_probe::stop();
        if outcome.is_ok() {
            steps.push(Step::Acknowledge(snapshot.clone()));
        }
        outcome
    }

    #[derive(Clone, Copy)]
    enum Inode<'a> {
        Durable(&'a [u8]),
        Temporary(&'a str),
    }

    fn crash_states(trace: &Trace, prefix: &[Step]) -> BTreeSet<DiskState> {
        let effects: Vec<&Effect> = prefix
            .iter()
            .filter_map(|step| match step {
                Step::Effect(effect) => Some(effect),
                _ => None,
            })
            .collect();
        let barrier = effects
            .iter()
            .rposition(
                |effect| matches!(effect, Effect::SyncDirectory(path) if *path == trace.root),
            )
            .map_or(0, |index| index + 1);
        let directory_effects: Vec<(usize, &Effect)> = effects
            .iter()
            .enumerate()
            .filter(|(_, effect)| {
                matches!(
                    effect,
                    Effect::CreateTemporary(_) | Effect::Rename { .. } | Effect::Unlink(_)
                )
            })
            .map(|(index, effect)| (index, *effect))
            .collect();
        let pending = directory_effects
            .iter()
            .filter(|(index, _)| *index >= barrier)
            .count();
        assert!(
            pending <= MAX_PENDING_DIRECTORY_EFFECTS,
            "{pending} unsynced directory effects exceed the enumeration bound"
        );
        // Inode data: last written bytes and whether a file sync followed.
        let mut data: BTreeMap<&str, (&[u8], bool)> = BTreeMap::new();
        for effect in &effects {
            match effect {
                Effect::CreateTemporary(name) => {
                    data.insert(name, (&[], true));
                }
                Effect::WriteData(name, bytes) => {
                    data.insert(name, (bytes, false));
                }
                Effect::SyncFile(name) => {
                    data.get_mut(name.as_str())
                        .expect("sync of created inode")
                        .1 = true;
                }
                _ => {}
            }
        }
        let mut states = BTreeSet::new();
        for mask in 0u32..1 << pending {
            let mut names: BTreeMap<&str, Inode> = trace
                .base
                .iter()
                .map(|(name, bytes)| (name.as_str(), Inode::Durable(bytes)))
                .collect();
            let mut bit = 0;
            for &(index, effect) in &directory_effects {
                if index >= barrier {
                    let kept = mask >> bit & 1 == 1;
                    bit += 1;
                    if !kept {
                        continue;
                    }
                }
                match effect {
                    Effect::CreateTemporary(name) => {
                        names.insert(name, Inode::Temporary(name));
                    }
                    Effect::Rename { from, to } => {
                        names.remove(from.as_str());
                        names.insert(to, Inode::Temporary(from));
                    }
                    Effect::Unlink(name) => {
                        names.remove(name.as_str());
                    }
                    _ => unreachable!(),
                }
            }
            // Unsynced inode contents vary independently, once per inode.
            let unsynced: BTreeSet<&str> = names
                .values()
                .filter_map(|inode| match inode {
                    Inode::Temporary(inode) if !data[inode].1 => Some(*inode),
                    _ => None,
                })
                .collect();
            let mut choices: Vec<BTreeMap<&str, &[u8]>> = vec![BTreeMap::new()];
            for inode in unsynced {
                let bytes = data[inode].0;
                let options: [&[u8]; 3] = [bytes, &[], &bytes[..bytes.len() / 2]];
                choices = choices
                    .into_iter()
                    .flat_map(|choice| {
                        options.into_iter().map(move |option| {
                            let mut choice = choice.clone();
                            choice.insert(inode, option);
                            choice
                        })
                    })
                    .collect();
            }
            for choice in choices {
                states.insert(
                    names
                        .iter()
                        .map(|(name, inode)| {
                            let bytes = match inode {
                                Inode::Durable(bytes) => bytes,
                                Inode::Temporary(inode) => {
                                    choice.get(inode).copied().unwrap_or(data[inode].0)
                                }
                            };
                            ((*name).to_owned(), bytes.to_vec())
                        })
                        .collect(),
                );
            }
        }
        states
    }

    /// Enumerates every crash point of `trace`; `depth > 0` also saves after
    /// each recovery and enumerates that save's own crash points.
    fn enumerate(trace: &Trace, depth: u32, tally: &mut Tally) -> Result<(), String> {
        let mut attempted: Vec<WorldSnapshot> = trace.initial.iter().cloned().collect();
        let mut floor = trace.initial.clone();
        let mut exempt = trace.exempt;
        for point in 0..=trace.steps.len() {
            if let Some(step) = point.checked_sub(1).map(|index| &trace.steps[index]) {
                match step {
                    Step::Attempt(snapshot) => attempted.push(snapshot.clone()),
                    Step::Acknowledge(snapshot) | Step::Damage(snapshot) => {
                        floor = Some(snapshot.clone());
                    }
                    Step::Exempt(through) => exempt = exempt.max(*through),
                    Step::Effect(_) => {}
                }
            }
            let counts = if depth > 0 {
                &mut tally.main
            } else {
                &mut tally.after_recovery
            };
            counts.crash_points += 1;
            for state in crash_states(trace, &trace.steps[..point]) {
                let names: Vec<_> = state.keys().collect();
                let admissible = Admissible {
                    attempted: &attempted,
                    history: &trace.history,
                    floor: floor.as_ref(),
                    exempt,
                };
                check_state(&state, &admissible, depth, tally)
                    .map_err(|violation| format!("crash point {point}, {names:?}: {violation}"))?;
            }
        }
        Ok(())
    }

    /// What recovery may load at one crash point.
    struct Admissible<'a> {
        /// Snapshots attempted in this trace, including its initial one.
        attempted: &'a [WorldSnapshot],
        history: &'a [WorldSnapshot],
        floor: Option<&'a WorldSnapshot>,
        exempt: u64,
    }

    fn check_state(
        state: &DiskState,
        admissible: &Admissible<'_>,
        depth: u32,
        tally: &mut Tally,
    ) -> Result<(), String> {
        let Admissible {
            attempted,
            history,
            floor,
            exempt,
        } = *admissible;
        let counts = if depth > 0 {
            &mut tally.main
        } else {
            &mut tally.after_recovery
        };
        counts.states += 1;
        counts.referenced_sidecars += referenced_sidecars(state, exempt)?;
        let directory = tempfile::tempdir().unwrap();
        for (name, bytes) in state {
            fs::write(directory.path().join(name), bytes).unwrap();
        }
        let mut store = SnapshotStore::open(directory.path())
            .map_err(|error| format!("open failed: {error:#}"))?;
        let loaded = match store.load() {
            Ok(loaded) => loaded,
            Err(error) => {
                let error = format!("{error:#}");
                // Fail-closed refusal is tolerated only before any acknowledgement.
                if floor.is_none() && error.contains("manifest missing with existing snapshots") {
                    counts.refused_orphans += 1;
                    return Ok(());
                }
                return Err(match floor {
                    Some(floor) => {
                        format!("acknowledged tick {} unrecoverable: {error}", floor.tick)
                    }
                    None => format!("unexpected recovery failure: {error}"),
                });
            }
        };
        match (&loaded, floor) {
            (None, Some(floor)) => {
                return Err(format!(
                    "reset to empty after acknowledged tick {}",
                    floor.tick
                ));
            }
            (None, None) => counts.empty_worlds += 1,
            (Some(loaded), floor) => {
                if !attempted.contains(&loaded.snapshot) && history.contains(&loaded.snapshot) {
                    counts.resurrected += 1;
                } else if !attempted.contains(&loaded.snapshot) {
                    return Err(format!(
                        "tick {} is not a saved snapshot",
                        loaded.snapshot.tick
                    ));
                }
                if let Some(floor) = floor
                    && loaded.snapshot.tick < floor.tick
                {
                    return Err(format!(
                        "loaded tick {} precedes acknowledged tick {}",
                        loaded.snapshot.tick, floor.tick
                    ));
                }
                counts.recovered_previous += usize::from(loaded.recovered_previous);
            }
        }
        if depth == 0 {
            return Ok(());
        }
        let recovered = loaded.map(|loaded| loaded.snapshot);
        let tick = recovered.as_ref().map_or(0, |snapshot| snapshot.tick) + 1;
        // Like capture, the recovery save carries a recovered section forward.
        let next = match (
            &recovered,
            recovered
                .as_ref()
                .and_then(|state| state.preserved_prospect.as_ref()),
        ) {
            (Some(recovered), _) if recovered.native_world.is_some() => WorldSnapshot {
                tick,
                ..recovered.clone()
            },
            (_, Some(section)) => preserved(tick, section),
            _ => snapshot(tick),
        };
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            directory.path(),
            &next,
            &mut steps,
            atomic_write,
        )
        .map_err(|error| format!("save after recovery failed: {error:#}"))?;
        tally.recovery_saves += 1;
        drop(store);
        let reopened = SnapshotStore::open(directory.path()).unwrap();
        let reloaded = reopened.load().map_err(|error| format!("{error:#}"))?;
        if reloaded.map(|loaded| loaded.snapshot) != Some(next) {
            return Err("save after recovery did not reload exactly".into());
        }
        drop(reopened);
        if listing(directory.path())
            .iter()
            .any(|name| name.starts_with(TEMPORARY_PREFIX))
        {
            return Err("save after recovery left a stale temporary".into());
        }
        let trace = Trace {
            root: directory.path().to_path_buf(),
            base: state.clone(),
            initial: recovered,
            exempt,
            history: [history, attempted].concat(),
            steps,
        };
        enumerate(&trace, depth - 1, tally)
            .map_err(|violation| format!("after recovery save: {violation}"))
    }

    fn effect_count(steps: &[Step]) -> usize {
        steps
            .iter()
            .filter(|step| matches!(step, Step::Effect(_)))
            .count()
    }

    #[test]
    fn power_loss_at_every_crash_point_preserves_acknowledged_generation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        for (tick, generation) in [(10, 1), (20, 2), (30, 3), (40, 4)] {
            let receipt =
                traced_save(&mut store, &root, &snapshot(tick), &mut steps, atomic_write).unwrap();
            assert_eq!(receipt.generation, generation);
        }
        // Durable loss of the current generation forces a recovered-previous save.
        steps.push(Step::Damage(snapshot(30)));
        durability_probe::start();
        fs::remove_file(root.join("snapshot-4.json")).unwrap();
        durability_probe::record(|| Effect::Unlink("snapshot-4.json".into()));
        sync_directory(&root).unwrap();
        steps.extend(durability_probe::stop().into_iter().map(Step::Effect));
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, snapshot(30));
        for (tick, generation) in [(50, 5), (60, 6)] {
            let receipt =
                traced_save(&mut store, &root, &snapshot(tick), &mut steps, atomic_write).unwrap();
            assert_eq!(receipt.generation, generation);
        }
        drop(store);
        // Cleanup of generations 1, 2 and 3 plus the injected loss of 4; the
        // recovered-previous save finds generation 4 already gone.
        let unlinks: Vec<&str> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Effect(Effect::Unlink(name)) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            unlinks,
            [
                "snapshot-1.json",
                "snapshot-2.json",
                "snapshot-4.json",
                "snapshot-3.json"
            ]
        );

        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        eprintln!("effects {}, {tally:?}", effect_count(&trace.steps));
        assert_eq!(tally.main.crash_points, trace.steps.len() + 1);
        // Recovery is exercised from every outcome class the save path admits.
        assert!(tally.main.empty_worlds > 0);
        assert!(tally.main.refused_orphans > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.after_recovery.states > tally.main.states);
        assert_eq!(
            tally.recovery_saves,
            tally.main.states - tally.main.refused_orphans
        );
    }

    /// Harness self-check: publishing before the data sync must be caught.
    fn publish_before_data_sync(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        let temporary_name = durability_probe::file_name(temporary.path());
        durability_probe::record(|| Effect::CreateTemporary(temporary_name.clone()));
        temporary.write_all(bytes)?;
        durability_probe::record(|| Effect::WriteData(temporary_name.clone(), bytes.to_vec()));
        temporary.persist(root.join(name))?;
        durability_probe::record(|| Effect::Rename {
            from: temporary_name.clone(),
            to: name.to_owned(),
        });
        sync_directory(root)
    }

    #[test]
    fn power_loss_model_rejects_publication_before_data_sync() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(&mut store, &root, &snapshot(10), &mut steps, atomic_write).unwrap();
        traced_save(
            &mut store,
            &root,
            &snapshot(20),
            &mut steps,
            publish_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(violation.contains("acknowledged tick 10"), "{violation}");
    }

    /// Sidecar writes, reuses and collections in trace order.
    fn sidecar_events(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Effect(Effect::Rename { to, .. }) if is_sidecar_name(to) => {
                    Some(format!("write {to}"))
                }
                Step::Effect(Effect::ReuseSidecar(name)) => Some(format!("reuse {name}")),
                Step::Effect(Effect::Unlink(name)) if is_sidecar_name(name) => {
                    Some(format!("unlink {name}"))
                }
                _ => None,
            })
            .collect()
    }

    /// Each sidecar rename is made durable by a directory sync before the
    /// temporary of the generation that references it is created.
    fn assert_sidecar_durable_before_generation(steps: &[Step]) {
        let effects: Vec<&Effect> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Effect(effect) => Some(effect),
                _ => None,
            })
            .collect();
        for (index, effect) in effects.iter().enumerate() {
            if let Effect::Rename { to, .. } = effect
                && is_sidecar_name(to)
            {
                let rest = &effects[index + 1..];
                let sync = rest
                    .iter()
                    .position(|effect| matches!(effect, Effect::SyncDirectory(_)))
                    .expect("sidecar rename is synchronized");
                let generation = rest
                    .iter()
                    .position(|effect| matches!(effect, Effect::CreateTemporary(_)))
                    .expect("a generation follows its sidecar");
                assert!(sync < generation, "sidecar published after its generation");
            }
        }
    }

    /// Durable external loss of one sidecar; the floor drops to `floor`, and
    /// generations up to `through` may reference the lost sidecar.
    fn damage(root: &Path, steps: &mut Vec<Step>, name: &str, floor: WorldSnapshot, through: u64) {
        steps.push(Step::Damage(floor));
        steps.push(Step::Exempt(through));
        durability_probe::start();
        fs::remove_file(root.join(name)).unwrap();
        durability_probe::record(|| Effect::Unlink(name.into()));
        sync_directory(root).unwrap();
        steps.extend(durability_probe::stop().into_iter().map(Step::Effect));
    }

    #[test]
    fn power_loss_during_sidecar_publication_and_collection_preserves_acknowledged_generation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let (first, second) = (section(1), section(2));
        let (one, two) = (sidecar_name(&first), sidecar_name(&second));
        let mut steps = Vec::new();
        let save = |store: &mut SnapshotStore, steps: &mut Vec<Step>, tick, section| {
            traced_save(store, &root, &preserved(tick, section), steps, atomic_write).unwrap();
        };
        for (tick, section) in [(10, &first), (20, &first), (30, &second)] {
            save(&mut store, &mut steps, tick, section);
        }
        // Losing only the current generation's sidecar makes it corrupt; the
        // previous generation, with the other sidecar, is recovered.
        damage(&root, &mut steps, &two, preserved(20, &first), 3);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, preserved(20, &first));
        for (tick, section) in [(40, &second), (50, &second)] {
            save(&mut store, &mut steps, tick, section);
        }
        drop(store);
        // Write once, reuse while referenced, rewrite after loss, and collect
        // only after neither retained generation (50, 40) references it.
        assert_eq!(
            sidecar_events(&steps),
            [
                format!("write {one}"),
                format!("reuse {one}"),
                format!("write {two}"),
                format!("unlink {two}"),
                format!("write {two}"),
                format!("reuse {two}"),
                format!("unlink {one}"),
            ]
        );
        assert_sidecar_durable_before_generation(&steps);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        eprintln!("effects {}, {tally:?}", effect_count(&trace.steps));
        assert_eq!(tally.main.crash_points, trace.steps.len() + 1);
        assert!(tally.main.empty_worlds > 0);
        assert!(tally.main.refused_orphans > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.main.referenced_sidecars > 0);
        assert!(tally.after_recovery.referenced_sidecars > tally.main.referenced_sidecars);
        assert_eq!(
            tally.recovery_saves,
            tally.main.states - tally.main.refused_orphans
        );
    }

    #[test]
    fn power_loss_during_schema_two_migration_preserves_acknowledged_generation() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let section = section(3);
        // A durable schema-2 world written by an earlier binary: inline section.
        let legacy = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PRESERVED,
            ..preserved(10, &section)
        };
        let generation_bytes = serde_json::to_vec(&legacy).unwrap();
        let manifest = Manifest {
            schema_version: 1,
            current: Generation {
                number: 1,
                sha256: checksum(&generation_bytes),
            },
            previous: None,
        };
        let base = DiskState::from([
            ("snapshot-1.json".to_owned(), generation_bytes),
            (
                "manifest.json".to_owned(),
                serde_json::to_vec(&manifest).unwrap(),
            ),
        ]);
        for (name, bytes) in &base {
            fs::write(root.join(name), bytes).unwrap();
        }
        let mut store = SnapshotStore::open(&root).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.snapshot, legacy);
        let name = sidecar_name(&section);
        let mut steps = Vec::new();
        let save = |store: &mut SnapshotStore, steps: &mut Vec<Step>, tick| {
            traced_save(
                store,
                &root,
                &preserved(tick, &section),
                steps,
                atomic_write,
            )
            .unwrap();
        };
        save(&mut store, &mut steps, 20);
        // A lost sidecar recovers the inline schema-2 generation, and the next
        // save writes the sidecar again.
        damage(&root, &mut steps, &name, legacy.clone(), 2);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, legacy);
        save(&mut store, &mut steps, 30);
        save(&mut store, &mut steps, 40);
        drop(store);
        // The first schema-3 save writes the sidecar; the next one reuses it.
        assert_eq!(
            sidecar_events(&steps),
            [
                format!("write {name}"),
                format!("unlink {name}"),
                format!("write {name}"),
                format!("reuse {name}"),
            ]
        );
        assert_sidecar_durable_before_generation(&steps);
        let trace = Trace {
            root,
            base,
            initial: Some(legacy),
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        eprintln!("effects {}, {tally:?}", effect_count(&trace.steps));
        assert_eq!(tally.main.empty_worlds, 0);
        assert_eq!(tally.main.refused_orphans, 0);
        // A recovery save from the legacy generation rewrites the lost sidecar
        // before its own generation, so a crash in between can load the newer
        // acknowledged generation 2 again; the floor check still applies.
        assert!(tally.after_recovery.resurrected > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.main.referenced_sidecars > 0);
        assert_eq!(tally.recovery_saves, tally.main.states);
    }

    #[test]
    fn native_power_loss_preserves_world_and_optional_sidecars_through_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let (first, second) = (section(7), section(8));
        let mut steps = Vec::new();
        for (tick, section) in [(10, None), (20, Some(&first)), (30, Some(&second))] {
            traced_save(
                &mut store,
                &root,
                &native(tick, section),
                &mut steps,
                atomic_write,
            )
            .unwrap();
        }
        damage(
            &root,
            &mut steps,
            &sidecar_name(&second),
            native(20, Some(&first)),
            3,
        );
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, native(20, Some(&first)));
        for (tick, section) in [(40, Some(&second)), (50, None), (60, None)] {
            traced_save(
                &mut store,
                &root,
                &native(tick, section),
                &mut steps,
                atomic_write,
            )
            .unwrap();
        }
        assert!(!root.join(sidecar_name(&first)).exists());
        assert!(!root.join(sidecar_name(&second)).exists());
        assert_sidecar_durable_before_generation(&steps);
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        assert!(tally.main.empty_worlds > 0);
        assert!(tally.main.refused_orphans > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.main.referenced_sidecars > 0);
        assert!(tally.after_recovery.referenced_sidecars > tally.main.referenced_sidecars);
        assert_eq!(
            tally.recovery_saves,
            tally.main.states - tally.main.refused_orphans
        );
    }

    #[test]
    fn inert_import_power_loss_preserves_metadata_source_and_sidecar_retention() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        for (tick, variant) in [(10, 1), (20, 1), (30, 2)] {
            traced_save(
                &mut store,
                &root,
                &inert(tick, variant),
                &mut steps,
                atomic_write,
            )
            .unwrap();
        }
        let before = inert(20, 1);
        let current = inert(30, 2);
        damage(
            &root,
            &mut steps,
            &sidecar_name(current.preserved_prospect.as_ref().unwrap()),
            before.clone(),
            3,
        );
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, before);
        for (tick, variant) in [(40, 2), (50, 3), (60, 3)] {
            traced_save(
                &mut store,
                &root,
                &inert(tick, variant),
                &mut steps,
                atomic_write,
            )
            .unwrap();
        }
        for obsolete in [&before, &current] {
            assert!(
                !root
                    .join(sidecar_name(obsolete.preserved_prospect.as_ref().unwrap()))
                    .exists()
            );
        }
        assert_sidecar_durable_before_generation(&steps);
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        assert!(tally.main.empty_worlds > 0);
        assert!(tally.main.refused_orphans > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.main.referenced_sidecars > 0);
        assert!(tally.after_recovery.referenced_sidecars > tally.main.referenced_sidecars);
        assert_eq!(
            tally.recovery_saves,
            tally.main.states - tally.main.refused_orphans
        );
    }

    #[test]
    fn owner_import_power_loss_preserves_provenance_current_owners_and_sidecar_retention() {
        imported_power_loss(owners);
    }

    #[test]
    fn scalar_owner_power_loss_preserves_mutated_values_provenance_and_sidecars() {
        imported_power_loss(|tick, variant, rounds| {
            scalar_owners(tick, variant, i32::try_from(rounds).unwrap())
        });
    }

    fn imported_power_loss(make: fn(u64, u8, u64) -> WorldSnapshot) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        for (tick, variant, rounds) in [(10, 1, 1), (20, 1, 2), (30, 2, 3)] {
            traced_save(
                &mut store,
                &root,
                &make(tick, variant, rounds),
                &mut steps,
                atomic_write,
            )
            .unwrap();
        }
        let before = make(20, 1, 2);
        let current = make(30, 2, 3);
        damage(
            &root,
            &mut steps,
            &sidecar_name(current.preserved_prospect.as_ref().unwrap()),
            before.clone(),
            3,
        );
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, before);
        for (tick, variant, rounds) in [(40, 2, 4), (50, 3, 5), (60, 3, 6)] {
            let state = make(tick, variant, rounds);
            traced_save(&mut store, &root, &state, &mut steps, atomic_write).unwrap();
            assert_eq!(store.load().unwrap().unwrap().snapshot, state);
        }
        for obsolete in [&before, &current] {
            assert!(
                !root
                    .join(sidecar_name(obsolete.preserved_prospect.as_ref().unwrap()))
                    .exists()
            );
        }
        assert_sidecar_durable_before_generation(&steps);
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let mut tally = Tally::default();
        if let Err(violation) = enumerate(&trace, 1, &mut tally) {
            panic!("{violation}");
        }
        assert!(tally.main.empty_worlds > 0);
        assert!(tally.main.refused_orphans > 0);
        assert!(tally.main.recovered_previous > 0);
        assert!(tally.main.referenced_sidecars > 0);
        assert!(tally.after_recovery.referenced_sidecars > tally.main.referenced_sidecars);
        assert_eq!(
            tally.recovery_saves,
            tally.main.states - tally.main.refused_orphans
        );
    }

    #[test]
    fn owner_import_sidecar_checker_rejects_an_intact_generation_with_missing_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let snapshot = owners(10, 1, 1);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&snapshot).unwrap();
        let mut state: DiskState = fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        assert_eq!(referenced_sidecars(&state, 0).unwrap(), 1);
        state.remove(&sidecar_name(snapshot.preserved_prospect.as_ref().unwrap()));
        assert!(
            referenced_sidecars(&state, 0)
                .unwrap_err()
                .contains("references a missing or damaged sidecar")
        );
    }

    #[test]
    fn relationship_power_loss_recovers_exact_edges_and_saves_again() {
        for preserve in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().to_path_buf();
            let mut store = SnapshotStore::open(&root).unwrap();
            let (first, second) = (section(11), section(12));
            let before = relationships(10, preserve.then_some(&first), Some(DomainId(2)));
            let current = relationships(20, preserve.then_some(&second), Some(DomainId(3)));
            let mut steps = Vec::new();
            for state in [&before, &current] {
                traced_save(&mut store, &root, state, &mut steps, atomic_write).unwrap();
            }
            let damaged = if preserve {
                sidecar_name(&second)
            } else {
                "snapshot-2.json".into()
            };
            damage(&root, &mut steps, &damaged, before.clone(), 2);
            let recovered = store.load().unwrap().unwrap();
            assert!(recovered.recovered_previous);
            assert_eq!(recovered.snapshot, before);
            for state in [
                relationships(30, preserve.then_some(&second), None),
                relationships(40, None, Some(DomainId(3))),
                relationships(50, None, Some(DomainId(2))),
            ] {
                traced_save(&mut store, &root, &state, &mut steps, atomic_write).unwrap();
                assert_eq!(store.load().unwrap().unwrap().snapshot, state);
            }
            assert!(!root.join(sidecar_name(&first)).exists());
            assert!(!root.join(sidecar_name(&second)).exists());
            assert_sidecar_durable_before_generation(&steps);
            drop(store);
            let trace = Trace {
                root,
                base: DiskState::new(),
                initial: None,
                exempt: 0,
                history: Vec::new(),
                steps,
            };
            let mut tally = Tally::default();
            if let Err(violation) = enumerate(&trace, 1, &mut tally) {
                panic!("{violation}");
            }
            assert!(tally.main.recovered_previous > 0);
            assert!(tally.main.refused_orphans > 0);
            if preserve {
                assert!(tally.main.referenced_sidecars > 0);
            }
            assert_eq!(
                tally.recovery_saves,
                tally.main.states - tally.main.refused_orphans
            );
        }
    }

    #[test]
    fn relationship_sidecar_checker_rejects_an_intact_generation_with_missing_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = section(13);
        let mut store = SnapshotStore::open(root).unwrap();
        store
            .save(&relationships(10, Some(&source), Some(DomainId(2))))
            .unwrap();
        let mut state: DiskState = fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        assert_eq!(referenced_sidecars(&state, 0).unwrap(), 1);
        state.remove(&sidecar_name(&source));
        assert!(
            referenced_sidecars(&state, 0)
                .unwrap_err()
                .contains("references a missing or damaged sidecar")
        );
    }

    #[test]
    fn lifecycle_power_loss_recovers_exact_incarnations_bindings_completion_and_saves_again() {
        for preserve in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().to_path_buf();
            let mut store = SnapshotStore::open(&root).unwrap();
            let (first, second) = (section(17), section(18));
            let before = lifecycle(10, preserve.then_some(&first), 1);
            let current = lifecycle(20, preserve.then_some(&second), 2);
            let mut steps = Vec::new();
            for state in [&before, &current] {
                traced_save(&mut store, &root, state, &mut steps, atomic_write).unwrap();
            }
            let damaged = if preserve {
                sidecar_name(&second)
            } else {
                "snapshot-2.json".into()
            };
            damage(&root, &mut steps, &damaged, before.clone(), 2);
            let recovered = store.load().unwrap().unwrap();
            assert!(recovered.recovered_previous);
            assert_eq!(recovered.snapshot, before);
            for state in [
                lifecycle(30, preserve.then_some(&second), 3),
                lifecycle(40, None, 4),
                lifecycle(50, None, 5),
            ] {
                traced_save(&mut store, &root, &state, &mut steps, atomic_write).unwrap();
                assert_eq!(store.load().unwrap().unwrap().snapshot, state);
            }
            assert!(!root.join(sidecar_name(&first)).exists());
            assert!(!root.join(sidecar_name(&second)).exists());
            assert_sidecar_durable_before_generation(&steps);
            drop(store);
            let trace = Trace {
                root,
                base: DiskState::new(),
                initial: None,
                exempt: 0,
                history: Vec::new(),
                steps,
            };
            let mut tally = Tally::default();
            if let Err(violation) = enumerate(&trace, 1, &mut tally) {
                panic!("{violation}");
            }
            assert!(tally.main.recovered_previous > 0);
            assert!(tally.main.refused_orphans > 0);
            if preserve {
                assert!(tally.main.referenced_sidecars > 0);
            }
            assert_eq!(
                tally.recovery_saves,
                tally.main.states - tally.main.refused_orphans
            );
        }
    }

    #[test]
    fn lifecycle_sidecar_checker_rejects_an_intact_generation_with_missing_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let source = section(19);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&lifecycle(10, Some(&source), 1)).unwrap();
        let mut state: DiskState = fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        assert_eq!(referenced_sidecars(&state, 0).unwrap(), 1);
        state.remove(&sidecar_name(&source));
        assert!(
            referenced_sidecars(&state, 0)
                .unwrap_err()
                .contains("references a missing or damaged sidecar")
        );
    }

    /// Harness self-check: a sidecar renamed before its data sync must be caught.
    fn sidecar_before_data_sync(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
        if is_sidecar_name(name) {
            publish_before_data_sync(root, name, bytes)
        } else {
            atomic_write(root, name, bytes)
        }
    }

    #[test]
    fn power_loss_model_rejects_sidecar_publication_before_data_sync() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            &root,
            &preserved(10, &section(4)),
            &mut steps,
            sidecar_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(
            violation.contains("references a missing or damaged sidecar"),
            "{violation}"
        );
    }

    #[test]
    fn native_power_loss_model_checks_schema_four_sidecar_references() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            &root,
            &native(10, Some(&section(9))),
            &mut steps,
            sidecar_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(
            violation.contains("references a missing or damaged sidecar"),
            "{violation}"
        );
    }

    #[test]
    fn relationship_power_loss_model_rejects_undurable_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            &root,
            &relationships(10, Some(&section(14)), Some(DomainId(3))),
            &mut steps,
            sidecar_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(
            violation.contains("references a missing or damaged sidecar"),
            "{violation}"
        );
    }

    #[test]
    fn lifecycle_power_loss_model_rejects_undurable_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            &root,
            &lifecycle(10, Some(&section(20)), 1),
            &mut steps,
            sidecar_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(
            violation.contains("references a missing or damaged sidecar"),
            "{violation}"
        );
    }

    #[test]
    fn owner_import_power_loss_model_rejects_undurable_sidecar() {
        import_model_rejects_undurable_sidecar(owners(10, 1, 1));
    }

    #[test]
    fn scalar_owner_power_loss_model_rejects_undurable_sidecar() {
        import_model_rejects_undurable_sidecar(scalar_owners(10, 1, 1));
    }

    fn import_model_rejects_undurable_sidecar(state: WorldSnapshot) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = SnapshotStore::open(&root).unwrap();
        let mut steps = Vec::new();
        traced_save(
            &mut store,
            &root,
            &state,
            &mut steps,
            sidecar_before_data_sync,
        )
        .unwrap();
        drop(store);
        let trace = Trace {
            root,
            base: DiskState::new(),
            initial: None,
            exempt: 0,
            history: Vec::new(),
            steps,
        };
        let violation = enumerate(&trace, 0, &mut Tally::default()).unwrap_err();
        assert!(
            violation.contains("references a missing or damaged sidecar"),
            "{violation}"
        );
    }
}
