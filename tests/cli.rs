use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_icarus-server"))
}

#[test]
fn config_validation_does_not_create_runtime_directory() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let output = cli()
        .args(["check-config", "--data-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!state.exists());
    assert!(
        !cli()
            .args(["check-config", "--tick-hz", "0"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn unsupported_import_fails_without_reading_or_writing_paths() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("imported");
    let output = cli()
        .args([
            "import-save",
            "--source",
            "missing-official-save",
            "--destination",
        ])
        .arg(&destination)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable"));
    assert!(!destination.exists());
}

#[test]
fn headless_process_saves_then_resumes_its_clock() {
    let directory = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let output = cli()
            .args(["run", "--tick-hz", "120", "--max-ticks", "3", "--data-dir"])
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let logs = String::from_utf8_lossy(&output.stderr);
        assert!(logs.contains("save_committed"));
        assert!(logs.contains("\"gameplay_ready\":false"));
        assert!(logs.contains("\"event\":\"stopped\""));
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["current"]["number"], 2);
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("snapshot-2.json")).unwrap())
            .unwrap();
    assert!(saved["tick"].as_u64().unwrap() >= 6);
}

struct RunningServer {
    child: Child,
    logs: Receiver<String>,
}

impl RunningServer {
    fn start(directory: &std::path::Path) -> Self {
        Self::start_with_autosave(directory, "1")
    }

    fn start_with_autosave(directory: &std::path::Path, autosave: &str) -> Self {
        let mut child = cli()
            .args([
                "run",
                "--tick-hz",
                "120",
                "--autosave-seconds",
                autosave,
                "--data-dir",
            ])
            .arg(directory)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stream = child.stderr.take().unwrap();
        let (sender, logs) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self { child, logs }
    }

    fn wait_event(&self, event: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let line = self
                .logs
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {event}: {error}"));
            if serde_json::from_str::<serde_json::Value>(&line).unwrap()["event"] == event {
                return;
            }
        }
    }

    fn signal(&self, signal: &str) {
        assert!(
            Command::new("kill")
                .args([signal, &self.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "server did not exit within deadline"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn malformed_manifest_checksums_stop_startup_without_rewriting_state() {
    use icarus_bevy::{persistence::SnapshotStore, simulation::WorldSnapshot};

    for generation in ["current", "previous"] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        for tick in [10, 20] {
            store
                .save(&WorldSnapshot {
                    tick,
                    ..Default::default()
                })
                .unwrap();
        }
        drop(store);
        let manifest_path = directory.path().join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest[generation]["sha256"] = "".into();
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        let snapshots = [1, 2].map(|number| {
            fs::read(directory.path().join(format!("snapshot-{number}.json"))).unwrap()
        });
        let mut server = RunningServer::start(directory.path());
        assert!(!server.wait_exit().success());
        assert_eq!(fs::read(manifest_path).unwrap(), manifest_bytes);
        for (index, bytes) in snapshots.iter().enumerate() {
            assert_eq!(
                fs::read(
                    directory
                        .path()
                        .join(format!("snapshot-{}.json", index + 1))
                )
                .unwrap(),
                *bytes
            );
        }
        assert!(!directory.path().join("snapshot-3.json").exists());
    }
}

#[test]
fn termination_signals_commit_before_clean_exit() {
    for signal in ["-TERM", "-INT"] {
        let directory = tempfile::tempdir().unwrap();
        let mut server = RunningServer::start(directory.path());
        server.wait_event("started");
        server.signal(signal);
        assert!(server.wait_exit().success());
        server.wait_event("save_committed");
        server.wait_event("stopped");
        assert!(directory.path().join("manifest.json").exists());
    }
}

#[test]
fn abrupt_process_death_preserves_acknowledged_autosave() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = RunningServer::start(directory.path());
    server.wait_event("save_committed");
    server.signal("-KILL");
    assert!(!server.wait_exit().success());
    let store = icarus_bevy::persistence::SnapshotStore::open(directory.path()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert!(loaded.snapshot.tick > 0);
}

#[test]
fn special_snapshot_inputs_fail_promptly_without_publication() {
    for name in ["manifest.json", "snapshot-1.json", "server.lock"] {
        let directory = tempfile::tempdir().unwrap();
        if name == "snapshot-1.json" {
            fs::write(
                directory.path().join("manifest.json"),
                br#"{"schema_version":1,"current":{"number":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"},"previous":null}"#,
            )
            .unwrap();
        }
        let path = directory.path().join(name);
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let manifest_before = (name == "snapshot-1.json")
            .then(|| fs::read(directory.path().join("manifest.json")).unwrap());
        let mut server = RunningServer::start(directory.path());
        assert!(!server.wait_exit().success(), "accepted FIFO {name}");
        let logs = server.logs.iter().collect::<Vec<_>>().join("\n");
        assert!(logs.contains("regular file"), "{name}: {logs}");
        assert!(!logs.contains("save_committed"));
        assert!(!logs.contains("\"event\":\"started\""));
        use std::os::unix::fs::FileTypeExt;
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_fifo());
        if let Some(bytes) = manifest_before {
            assert_eq!(
                fs::read(directory.path().join("manifest.json")).unwrap(),
                bytes
            );
        }
        assert!(!directory.path().join("snapshot-2.json").exists());
    }
}

#[test]
fn fifo_current_generation_recovers_regular_previous_generation() {
    use icarus_bevy::simulation::{DomainId, EntityRecord, Position, WorldSnapshot};
    let directory = tempfile::tempdir().unwrap();
    let previous = WorldSnapshot {
        tick: 10,
        entities: vec![EntityRecord {
            id: DomainId(1),
            position: Position([1.0, 2.0, 3.0]),
        }],
        ..Default::default()
    };
    let current = WorldSnapshot {
        tick: 20,
        entities: vec![EntityRecord {
            id: DomainId(2),
            position: Position([4.0, 5.0, 6.0]),
        }],
        ..Default::default()
    };
    let mut store = icarus_bevy::persistence::SnapshotStore::open(directory.path()).unwrap();
    store.save(&previous).unwrap();
    store.save(&current).unwrap();
    drop(store);
    let manifest = fs::read(directory.path().join("manifest.json")).unwrap();
    let previous_bytes = fs::read(directory.path().join("snapshot-1.json")).unwrap();
    let path = directory.path().join("snapshot-2.json");
    fs::remove_file(&path).unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let mut server = RunningServer::start_with_autosave(directory.path(), "3600");
    server.wait_event("recovered_previous_generation");
    server.wait_event("started");
    assert_eq!(
        fs::read(directory.path().join("manifest.json")).unwrap(),
        manifest
    );
    assert_eq!(
        fs::read(directory.path().join("snapshot-1.json")).unwrap(),
        previous_bytes
    );
    server.signal("-TERM");
    assert!(server.wait_exit().success());
    server.wait_event("save_committed");
    let store = icarus_bevy::persistence::SnapshotStore::open(directory.path()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.snapshot.tick >= previous.tick);
    assert_eq!(loaded.snapshot.entities, previous.entities);
}
