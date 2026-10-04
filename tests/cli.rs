use std::{fs, process::Command};

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
