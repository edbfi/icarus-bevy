//! Native snapshots only. Official ICARUS save import requires a recovered spec.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::simulation::WorldSnapshot;

const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

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
    fn filename(&self) -> String {
        format!("snapshot-{}.json", self.number)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    current: Generation,
    previous: Option<Generation>,
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
            .open(root.join("server.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
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
        let manifest: Manifest =
            serde_json::from_slice(&bytes).context("invalid snapshot manifest")?;
        ensure!(manifest.schema_version == 1, "unsupported manifest schema");
        ensure!(manifest.current.number > 0, "invalid snapshot generation");
        if let Some(previous) = &manifest.previous {
            ensure!(
                previous.number > 0 && previous.number < manifest.current.number,
                "invalid previous generation"
            );
        }
        Ok(Some(manifest))
    }

    fn read_generation(&self, generation: &Generation) -> Result<WorldSnapshot> {
        let bytes = read_bounded(&self.root.join(generation.filename()), MAX_SNAPSHOT_BYTES)?;
        ensure!(
            checksum(&bytes) == generation.sha256,
            "snapshot checksum mismatch"
        );
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("decode native snapshot")?;
        if value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        {
            return Err(UnsupportedSchema.into());
        }
        let snapshot: WorldSnapshot =
            serde_json::from_value(value).context("decode native snapshot fields")?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn select(&self, manifest: &Manifest) -> Result<(WorldSnapshot, Generation, bool)> {
        match self.read_generation(&manifest.current) {
            Ok(snapshot) => Ok((snapshot, manifest.current.clone(), false)),
            Err(error) if error.is::<UnsupportedSchema>() => Err(error),
            Err(current_error) => match &manifest.previous {
                Some(previous) => {
                    let snapshot = self.read_generation(previous).with_context(|| {
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

    // The writer boundary permits deterministic publication failures in tests.
    fn save_with_writer(
        &mut self,
        snapshot: &WorldSnapshot,
        mut write: impl FnMut(&Path, &str, &[u8]) -> Result<()>,
    ) -> Result<SaveReceipt> {
        snapshot.validate()?;
        let manifest = self.manifest()?;
        let (number, previous) = match &manifest {
            Some(manifest) => {
                let (_, valid_generation, _) = self.select(manifest)?;
                (
                    manifest
                        .current
                        .number
                        .checked_add(1)
                        .context("snapshot generation overflow")?,
                    Some(valid_generation),
                )
            }
            None => (1, None),
        };
        let bytes = serde_json::to_vec(snapshot)?;
        ensure!(
            bytes.len() as u64 <= MAX_SNAPSHOT_BYTES,
            "native snapshot exceeds 64 MiB limit"
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
        // The save is durable now. Cleanup is best effort and cannot revoke it.
        let entries = fs::read_dir(&self.root).ok().into_iter().flatten();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
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
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
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
        .context("synchronize snapshot directory")
}

fn atomic_write(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut temporary = NamedTempFile::new_in(root)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(root.join(name))
        .context("publish snapshot file")?;
    sync_directory(root)
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

    pub fn poll(&mut self) -> Result<Option<SaveReceipt>> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.busy = false;
                result.map(Some)
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.busy = false;
                Err(SaveWorkerDisconnected.into())
            }
        }
    }

    pub fn finish_before(&mut self, deadline: Instant) -> Result<Option<SaveReceipt>> {
        while self.busy {
            if let Some(receipt) = self.poll()? {
                return Ok(Some(receipt));
            }
            ensure!(
                Instant::now() < deadline,
                "snapshot shutdown deadline exceeded"
            );
            thread::sleep(Duration::from_millis(1));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod worker_failure_tests {
    use super::*;

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
