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
        let mut child = cli()
            .args([
                "run",
                "--tick-hz",
                "120",
                "--autosave-seconds",
                "1",
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
