//! CLI boundaries only: authored files, no sockets, services or private inputs.
use super::*;
use icarus_bevy::{
    authored_world_bootstrap::AuthoredWorldBootstrap,
    config::PlayerPersistenceConfig,
    data_pak::DataPak,
    gameplay_profile::{FreshPlayerValues, fresh_player_profile},
    native_hosting::HostingConfig,
    native_player_runtime::ConsumptionConfig,
};
use std::{net::Ipv4Addr, os::unix::fs::PermissionsExt, path::Path};

fn cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command.current_dir(root);
    command
}

fn config(root: &Path) -> std::path::PathBuf {
    // Entirely authored input preparation must complete before the exclusive
    // descriptor/environment check, while storage/services remain unopened.
    let catalog = root.join("authored.pak");
    fs::write(
        &catalog,
        pak_writer::fresh_join_pak_with(&[(
            "Prospects/D_ProspectList.json",
            pak_writer::table_json(&["AuthoredWorkshop"]),
        )]),
    )
    .unwrap();
    let recipe = root.join("authored.json");
    let gameplay = fresh_player_profile(
        &mut DataPak::open(&catalog).unwrap(),
        &FreshPlayerValues::default(),
    )
    .unwrap();
    fs::write(&recipe, serde_json::to_vec(&gameplay).unwrap()).unwrap();
    let players = PlayerPersistenceConfig {
        prospect_id: "authored-proof".into(),
        map_url: "/Authored/Maps/Workshop".into(),
        prospect_row: "AuthoredWorkshop".into(),
        difficulty: 2,
    };
    let config = ServerConfig {
        data_dir: root.join("state"),
        network: NetworkConfig {
            enabled: true,
            max_players: 1,
            authored_controller_leave: true,
            player_consumption: Some(ConsumptionConfig {
                oxygen: 0,
                water: 0,
                food: 1,
            }),
            player_persistence: Some(players.clone()),
            release_profile: Some(release_toml()),
            data_pak: Some(catalog),
            authored_gameplay: Some(recipe),
            authored_world: Some(AuthoredWorldBootstrap {
                seed: 1,
                seed_initialised: true,
                flod_tile_count: 0,
                clock: None,
            }),
            hosting: Some(HostingConfig {
                query_bind: Ipv4Addr::LOCALHOST,
                query_port: 17778,
                private_ip: Ipv4Addr::LOCALHOST,
                seconds: 60,
                name: "Authored proof".into(),
                map_url: players.map_url,
                prospect_id: players.prospect_id,
                prospect_row: players.prospect_row,
                lobby_name: "Authored proof".into(),
                difficulty: players.difficulty,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let path = root.join("server.toml");
    // Configuration serialization deliberately omits passwords. This authored
    // fixture explicitly supplies one; it is never a real delivered password.
    let mut value = toml::Value::try_from(&config).unwrap();
    value["network"]
        .as_table_mut()
        .unwrap()
        .insert("join_password".into(), "authored-test-only".into());
    fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    let parsed: ServerConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let (mut prepared, players) = network::prepare_run_with_tick_hz(
        &parsed.network,
        network::peer_lab_config(&parsed.network),
        parsed.tick_hz,
    )
    .expect("authored fixture must reach descriptor/invocation validation");
    assert!(players.is_some());
    prepared
        .as_mut()
        .unwrap()
        .prepare_runtime(parsed.autosave_seconds)
        .unwrap();
    assert!(!root.join("state").exists());
    path
}

#[test]
fn private_proof_requires_actual_password_and_exact_food_only_rules_before_sink() {
    let directory = tempfile::Builder::new()
        .prefix("authored-proof-")
        .tempdir()
        .unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(directory.path());
    let original: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    for variant in [
        "empty_password",
        "no_consumption",
        "oxygen",
        "water",
        "food",
    ] {
        let mut changed = original.clone();
        match variant {
            "empty_password" => changed["network"]["join_password"] = "".into(),
            "no_consumption" => {
                changed["network"]
                    .as_table_mut()
                    .unwrap()
                    .remove("player_consumption");
            }
            "oxygen" | "water" => changed["network"]["player_consumption"][variant] = 1.into(),
            _ => changed["network"]["player_consumption"]["food"] = 2.into(),
        }
        fs::write(&config, toml::to_string(&changed).unwrap()).unwrap();
        let output = cli(directory.path())
            .args(["run", "--config"])
            .arg(&config)
            .args(["--private-leave-proof", "unopened-descriptor.json"])
            .env("INVOCATION_ID", "0123456789abcdef0123456789abcdef")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
            serde_json::json!({"event":"fatal","error":"private leave proof failed"})
        );
        assert!(!directory.path().join("state").exists());
        assert!(!directory.path().join("phase-1.jsonl").exists());
    }
}

#[test]
fn private_proof_is_run_only_and_disabled_run_creates_no_proof_files() {
    let directory = tempfile::tempdir().unwrap();
    let output = cli(directory.path())
        .args(["check-config", "--private-leave-proof", "descriptor.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    let output = cli(directory.path())
        .args([
            "run",
            "--compressed",
            "--max-ticks",
            "1",
            "--data-dir",
            "state",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "disabled run must retain ordinary behavior"
    );
    let names: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from("state")]);
}

#[test]
fn private_proof_rejects_descriptor_or_invocation_before_state_and_redacts_values() {
    let directory = tempfile::Builder::new()
        .prefix("authored-proof-")
        .tempdir()
        .unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(directory.path());
    let descriptor = directory.path().join("descriptor.json");
    let case_id = directory.path().file_name().unwrap().to_str().unwrap();
    let valid = serde_json::json!({
        "format": 1, "case_id": case_id, "phase": 1,
        "output": "phase-1.jsonl", "predecessor": null, "exit_seal": null,
    });
    let sentinel = "PRIVATE-SENTINEL-NEVER-PUBLIC-9e2f";
    let output = cli(directory.path())
        .args([
            "run",
            "--private-leave-proof",
            "descriptor.json",
            "--max-ticks",
            sentinel,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap(),
        serde_json::json!({"event":"fatal","error":"private leave proof failed"})
    );
    for (data, invocation) in [
        (
            sentinel.as_bytes().to_vec(),
            Some("0123456789abcdef0123456789abcdef"),
        ),
        (serde_json::to_vec(&valid).unwrap(), None),
        (serde_json::to_vec(&valid).unwrap(), Some(sentinel)),
        (
            serde_json::to_vec(&valid).unwrap(),
            Some("0123456789ABCDEF0123456789ABCDEF"),
        ),
    ] {
        fs::write(&descriptor, data).unwrap();
        fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600)).unwrap();
        let mut command = cli(directory.path());
        command
            .args(["run", "--config"])
            .arg(&config)
            .arg("--private-leave-proof")
            .arg(&descriptor)
            .env_remove("INVOCATION_ID");
        if let Some(invocation) = invocation {
            command.env("INVOCATION_ID", invocation);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let records: Vec<serde_json::Value> = String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            records,
            [serde_json::json!({
                "event": "fatal", "error": "private leave proof failed"
            })]
        );
        assert!(!directory.path().join("state").exists());
        assert!(!directory.path().join("phase-1.jsonl").exists());
        assert!(!directory.path().join("phase-1-complete.json").exists());
    }
}
