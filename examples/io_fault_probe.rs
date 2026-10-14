//! Real ENOSPC/EACCES probe, only for the approved disposable 256 KiB tmpfs lab.
use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use icarus_bevy::{
    persistence::SnapshotStore,
    simulation::{DomainId, EntityRecord, Position, WorldSnapshot},
};
use serde_json::json;

fn snapshot(tick: u64, entities: u64) -> WorldSnapshot {
    WorldSnapshot {
        tick,
        entities: (1..=entities)
            .map(|id| EntityRecord {
                id: DomainId(id),
                position: Position([id as f32, 1.25, -2.5]),
            })
            .collect(),
        ..Default::default()
    }
}

fn io_error_kind(error: &anyhow::Error) -> Option<ErrorKind> {
    // tempfile adds path context in a custom io::Error. Its public contract
    // preserves kind(), but not raw_os_error() or the underlying OS error source.
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
    })
}

fn main() -> Result<()> {
    ensure!(
        std::env::var("ICARUS_BOUNDED_TMPFS_BYTES").as_deref() == Ok("262144"),
        "run only through the authorized 256 KiB tmpfs harness"
    );
    let status = fs::read_to_string("/proc/self/status")?;
    let uid_line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .context("missing UID")?;
    ensure!(
        uid_line.split_whitespace().skip(1).all(|id| id != "0"),
        "probe must be unprivileged"
    );
    let root = Path::new("/tmp/icarus-io-faults");
    ensure!(!root.exists(), "probe needs a fresh disposable directory");
    let mut store = SnapshotStore::open(root)?;
    let initial = snapshot(10, 100);
    let committed = snapshot(20, 200);
    store.save(&initial)?;
    let receipt = store.save(&committed)?;
    let manifest = fs::read(root.join("manifest.json"))?;

    // Hard cap: even an incorrectly configured harness writes at most 512 KiB.
    let filler_path = root.join("authored-filler");
    let mut filler = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&filler_path)?;
    let chunk = [0xa5; 4096];
    let mut written = 0;
    let mut full = false;
    while written < 512 * 1024 {
        match filler.write(&chunk) {
            Ok(0) => anyhow::bail!("unexpected zero-length filler write"),
            Ok(bytes) => written += bytes,
            Err(error) if error.kind() == ErrorKind::StorageFull => {
                full = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    ensure!(
        full,
        "bounded filesystem did not report ENOSPC; refuse further writes"
    );
    let newer = snapshot(30, 1000);
    let error = store
        .save(&newer)
        .expect_err("full filesystem must reject snapshot publication");
    ensure!(
        io_error_kind(&error) == Some(ErrorKind::StorageFull),
        "expected ENOSPC, got {error:#}"
    );
    ensure!(
        fs::read(root.join("manifest.json"))? == manifest,
        "ENOSPC changed committed manifest"
    );
    ensure!(
        store.load()?.context("missing committed state")?.snapshot == committed,
        "ENOSPC lost acknowledged state"
    );
    println!(
        "{}",
        json!({"event":"enospc_verified", "filler_bytes":written,
        "acknowledged_generation":receipt.generation, "acknowledged_tick":receipt.tick,
        "manifest_unchanged":true, "state_verified":true, "error":format!("{error:#}")})
    );
    drop(filler);
    fs::remove_file(filler_path)?;
    ensure!(
        store.save(&newer)?.generation == 3,
        "ENOSPC retry generation mismatch"
    );
    ensure!(
        store.load()?.context("missing retried state")?.snapshot == newer,
        "retry state mismatch"
    );

    let before_denied = fs::read(root.join("manifest.json"))?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o555))?;
    let denied = store.save(&snapshot(40, 1100));
    // Restore permissions before inspecting the result, including unexpected success.
    fs::set_permissions(root, fs::Permissions::from_mode(0o755))?;
    let error = denied.expect_err("unwritable directory must reject snapshot publication");
    ensure!(
        io_error_kind(&error) == Some(ErrorKind::PermissionDenied),
        "expected EACCES, got {error:#}"
    );
    ensure!(
        fs::read(root.join("manifest.json"))? == before_denied,
        "EACCES changed manifest"
    );
    ensure!(
        store
            .load()?
            .context("missing state after EACCES")?
            .snapshot
            == newer,
        "EACCES lost state"
    );
    println!(
        "{}",
        json!({"event":"eacces_verified", "manifest_unchanged":true,
        "state_verified":true, "error":format!("{error:#}")})
    );
    let final_snapshot = snapshot(40, 1100);
    ensure!(
        store.save(&final_snapshot)?.generation == 4,
        "EACCES retry generation mismatch"
    );
    drop(store);
    let reopened = SnapshotStore::open(root)?;
    ensure!(
        reopened.load()?.context("missing reopened state")?.snapshot == final_snapshot,
        "reopened state mismatch"
    );
    println!(
        "{}",
        json!({"event":"io_fault_probe_complete", "generation":4,
        "tick":40, "entities":1100, "state_verified":true,
        "physical_power_loss_tested":false})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempfile_path_context_preserves_io_classification() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-directory");
        fs::write(&file, b"authored").unwrap();
        let error = tempfile::NamedTempFile::new_in(&file)
            .context("create snapshot temporary file")
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            None
        );
        assert_eq!(io_error_kind(&error), Some(ErrorKind::NotADirectory));
    }

    #[test]
    fn classifies_direct_and_wrapped_faults_without_matching_messages() {
        for (errno, kind) in [
            (28, ErrorKind::StorageFull),
            (13, ErrorKind::PermissionDenied),
        ] {
            let direct = anyhow::Error::from(std::io::Error::from_raw_os_error(errno));
            assert_eq!(io_error_kind(&direct), Some(kind));
            let wrapped = std::io::Error::new(kind, "path context hides raw errno");
            let contextual = anyhow::Error::from(wrapped).context("save failed");
            assert_eq!(io_error_kind(&contextual), Some(kind));
        }
        let text_only = anyhow::anyhow!("No space left on device (os error 28)");
        assert_eq!(io_error_kind(&text_only), None);
        let unrelated =
            anyhow::Error::from(std::io::Error::other("Permission denied (os error 13)"));
        assert_eq!(io_error_kind(&unrelated), Some(ErrorKind::Other));
    }
}
