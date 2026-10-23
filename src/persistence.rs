//! Native snapshots only. Official ICARUS save import requires a recovered spec.
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
use crate::native_world::NativeWorldSnapshot;
use crate::preservation::{MAX_PRESERVED_BASE64_BYTES, SectionReference, is_sidecar_name};
use crate::simulation::{
    EntityRecord, SNAPSHOT_SCHEMA_DOMAIN, SNAPSHOT_SCHEMA_NATIVE, SNAPSHOT_SCHEMA_PRESERVED,
    SNAPSHOT_SCHEMA_REFERENCED, WorldSnapshot,
};

/// Schema 1 bound, unchanged: domain records and the clock only.
const MAX_DOMAIN_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
/// Schema 2 adds at most one 64 MiB Base64 preserved section (whose decoded
/// size is checked separately), so a generation file may reach 128 MiB.
const MAX_SNAPSHOT_BYTES: u64 = MAX_DOMAIN_SNAPSHOT_BYTES + MAX_PRESERVED_BASE64_BYTES as u64;
/// Schemas 3 and 4 add only a fixed-size sidecar reference to the domain bound;
/// the sidecar itself is bounded by the 48 MiB native encoding cap.
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
            _ => return Err(UnsupportedSchema.into()),
        }
        let snapshot = serde_json::from_slice::<JsonObject<WorldSnapshot>>(bytes)
            .map_err(|error| crate::json_parse_error("decode native snapshot fields JSON", error))?
            .0;
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
        };
        snapshot.validate()?;
        Ok((snapshot, Some(reference)))
    }

    fn decode_native(&self, bytes: &[u8]) -> Result<(WorldSnapshot, Option<SectionReference>)> {
        ensure!(
            bytes.len() as u64 <= MAX_REFERENCED_SNAPSHOT_BYTES,
            "schema 4 native snapshot exceeds its size limit"
        );
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
        };
        // Validation recompiles the embedded catalog and verifies its identity
        // and every native record before this generation can be activated.
        snapshot.validate()?;
        Ok((snapshot, reference))
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
    // world stays schema 4. Its sidecar is durable (data, rename
    // and directory synchronized) before the generation referencing it is
    // written, and a sidecar that already verifies is reused, never rewritten.
    fn save_with_writer(
        &mut self,
        snapshot: &WorldSnapshot,
        mut write: impl FnMut(&Path, &str, &[u8]) -> Result<()>,
    ) -> Result<SaveReceipt> {
        snapshot.validate()?;
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
        let reference = snapshot
            .preserved_prospect
            .as_ref()
            .map(|preserved| SectionReference::of(preserved));
        if let Some(reference) = &reference {
            reference.validate()?;
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
        // Choose schema 4 first: its optional preserved section must never
        // route a native world through the schema-3 serializer.
        let (bytes, limit) = if snapshot.schema_version == SNAPSHOT_SCHEMA_NATIVE {
            let bytes = serde_json::to_vec(&NativeOut {
                schema_version: SNAPSHOT_SCHEMA_NATIVE,
                tick: snapshot.tick,
                entities: &snapshot.entities,
                native_world: snapshot
                    .native_world
                    .as_ref()
                    .context("native world required")?,
                preserved_prospect: reference.as_ref(),
            })?;
            (bytes, MAX_REFERENCED_SNAPSHOT_BYTES)
        } else {
            match &reference {
                Some(reference) => {
                    let bytes = serde_json::to_vec(&ReferencedOut {
                        schema_version: SNAPSHOT_SCHEMA_REFERENCED,
                        tick: snapshot.tick,
                        entities: &snapshot.entities,
                        preserved_prospect: reference,
                    })?;
                    (bytes, MAX_REFERENCED_SNAPSHOT_BYTES)
                }
                // Schema 1 serialization is unchanged.
                None => (serde_json::to_vec(snapshot)?, MAX_DOMAIN_SNAPSHOT_BYTES),
            }
        };
        ensure!(
            bytes.len() as u64 <= limit,
            "native snapshot exceeds {} MiB limit",
            limit / (1024 * 1024)
        );
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
        /// Intact manifest-referenced schema-3/4 generations whose sidecar was
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

    #[test]
    fn native_publication_fault_matrix_preserves_committed_state() {
        let (first, second) = (section(5), section(6));
        for preserve in [false, true] {
            for fail_at in ["sidecar", "snapshot-2.json", "manifest.json"] {
                if fail_at == "sidecar" && !preserve {
                    continue;
                }
                for after_write in [false, true] {
                    let directory = tempfile::tempdir().unwrap();
                    let root = directory.path();
                    let before = native(10, preserve.then_some(&first));
                    let attempted = native(20, preserve.then_some(&second));
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
                    let next = native(30, preserve.then_some(&second));
                    assert_eq!(
                        reopened.save(&next).unwrap().generation,
                        if published { 3 } else { 2 }
                    );
                    assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
                }
            }
        }
    }

    /// Every intact, manifest-referenced schema-3/4 generation must find its
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
}
