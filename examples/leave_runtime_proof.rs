//! Case-bound private observer. Menus require an explicit actual owner report.
//! All checkpoint decoding/comparison remains in the library's closed reader.
use clap::{Parser, Subcommand, ValueEnum};
use icarus_bevy::leave_runtime_proof::{
    OwnerConfirmation, inspect_phase, publish_abort, publish_completion,
    publish_owner_confirmation, read_private_input, seal_exit_store, validate_descriptor,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[path = "../src/cli/private_leave_proof_config.rs"]
mod private_leave_proof_config;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pure input/codec/proof preparation; creates no state, sink or listener.
    Prepare {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        descriptor: PathBuf,
    },
    /// Record only a menu personally reported by the owner, never a runtime cue.
    RecordMenu {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        manager_receipts: PathBuf,
        #[arg(long, value_enum)]
        reported_menu: ReportedMenu,
    },
    /// Validate one ready phase and a separately supplied actual owner report.
    Observe {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        manager_receipts: PathBuf,
        #[arg(long)]
        owner_confirmation: PathBuf,
    },
    /// After a successful manager exit, preserve the released store's pair.
    SealExit {
        #[arg(long)]
        descriptor: PathBuf,
        #[arg(long)]
        manager_receipts: PathBuf,
        #[arg(long)]
        store: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ReportedMenu {
    CharacterSelection,
    Title,
}

// No runtime identity or checkpoint fields are accepted from this descriptor.
// The core independently validates its exact case/path/phase binding.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescriptorBinding {
    format: u32,
    case_id: String,
    phase: u8,
    output: String,
    predecessor: Option<String>,
    exit_seal: Option<String>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReceiptKind {
    Created,
    BeforeStop,
    AfterExit,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagerReceipt {
    kind: ReceiptKind,
    phase: u8,
    invocation: String,
    result: String,
    native_executed: bool,
    exit_code: String,
    exit_status: i32,
    pid_zero: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerReport {
    phase: u8,
    invocation: String,
    menu_confirmed: bool,
}

type Checked<T> = Result<T, ()>;

fn invocation_valid(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn descriptor_phase(root: &Path, path: &Path) -> Checked<u8> {
    let bytes = read_private_input(root, path, 4096).map_err(|_| ())?;
    let binding: DescriptorBinding = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if binding.format != 1
        || !matches!(binding.phase, 1 | 2)
        || root.file_name().and_then(|name| name.to_str()) != Some(binding.case_id.as_str())
        || binding.output != format!("phase-{}.jsonl", binding.phase)
        || (binding.phase == 1 && (binding.predecessor.is_some() || binding.exit_seal.is_some()))
        || (binding.phase == 2
            && (binding.predecessor.as_deref() != Some("phase-1.jsonl")
                || binding.exit_seal.as_deref() != Some("phase-1-exit.json")))
    {
        return Err(());
    }
    Ok(binding.phase)
}

/// The supervisor's immutable prefix has exactly one created/before-stop/exit
/// triple per phase. Completion precedes stopping; sealing follows its exit.
fn manager_invocation(bytes: &[u8], phase: u8, sealing: bool) -> Checked<String> {
    if !matches!(phase, 1 | 2) || bytes.len() > 32 * 1024 || !bytes.ends_with(b"\n") {
        return Err(());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let rows: Vec<ManagerReceipt> = text
        .lines()
        .map(|line| {
            if line.is_empty() || line.len() > 4096 {
                return Err(());
            }
            serde_json::from_str(line).map_err(|_| ())
        })
        .collect::<Checked<_>>()?;
    let expected = usize::from(phase - 1) * 3 + if sealing { 3 } else { 1 };
    if rows.len() != expected {
        return Err(());
    }
    let mut previous_invocation: Option<&str> = None;
    for current in 1..=phase {
        let start = usize::from(current - 1) * 3;
        let count = if current < phase || sealing { 3 } else { 1 };
        let created = &rows[start];
        if created.phase != current
            || created.kind != ReceiptKind::Created
            || !invocation_valid(&created.invocation)
            || created.pid_zero
            || !created.native_executed
            || previous_invocation == Some(created.invocation.as_str())
        {
            return Err(());
        }
        if count == 3 {
            let before = &rows[start + 1];
            let exit = &rows[start + 2];
            if before.kind != ReceiptKind::BeforeStop
                || exit.kind != ReceiptKind::AfterExit
                || [before, exit]
                    .iter()
                    .any(|row| row.phase != current || row.invocation != created.invocation)
                || !exit.pid_zero
                || !exit.native_executed
                || exit.result != "success"
                || exit.exit_code != "exited"
                || exit.exit_status != 0
            {
                return Err(());
            }
        }
        previous_invocation = Some(&created.invocation);
    }
    Ok(rows[usize::from(phase - 1) * 3].invocation.clone())
}

fn prepare(root: &Path, config: PathBuf, descriptor: &Path) -> Checked<()> {
    use icarus_bevy::{config::ConfigOptions, network};
    let config = ConfigOptions {
        config: Some(config),
        ..Default::default()
    }
    .resolve()
    .map_err(|_| ())?;
    private_leave_proof_config::validate_config(&config).map_err(|_| ())?;
    descriptor_phase(root, descriptor)?;
    validate_descriptor(descriptor, root).map_err(|_| ())?;
    let (mut prepared, players) = network::prepare_run_with_tick_hz(
        &config.network,
        network::peer_lab_config(&config.network),
        config.tick_hz,
    )
    .map_err(|_| ())?;
    private_leave_proof_config::validate_initial_vitals(players.as_ref().ok_or(())?.initial_vitals)
        .map_err(|_| ())?;
    prepared
        .as_mut()
        .ok_or(())?
        .prepare_runtime(config.autosave_seconds)
        .map_err(|_| ())?;
    // Dropping pure plans cannot bind listeners, start Steam, acquire a store
    // lock or consume the exclusive proof output. Actual Run repeats binding.
    drop((prepared, players));
    Ok(())
}

fn run(root: &Path, command: Command) -> Checked<()> {
    match command {
        Command::Prepare { config, descriptor } => prepare(root, config, &descriptor),
        Command::RecordMenu {
            descriptor,
            manager_receipts,
            reported_menu,
        } => {
            let phase = descriptor_phase(root, &descriptor)?;
            if !matches!(
                (phase, reported_menu),
                (1, ReportedMenu::CharacterSelection) | (2, ReportedMenu::Title)
            ) {
                return Err(());
            }
            let manager = read_private_input(root, &manager_receipts, 32 * 1024).map_err(|_| ())?;
            let invocation = manager_invocation(&manager, phase, false)?;
            publish_owner_confirmation(
                root,
                &descriptor,
                &OwnerConfirmation {
                    phase,
                    invocation,
                    menu_confirmed: true,
                },
            )
            .map_err(|_| ())
        }
        Command::Observe {
            descriptor,
            manager_receipts,
            owner_confirmation,
        } => {
            let phase = descriptor_phase(root, &descriptor)?;
            let manager = read_private_input(root, &manager_receipts, 32 * 1024).map_err(|_| ())?;
            let invocation = manager_invocation(&manager, phase, false)?;
            let bytes = read_private_input(root, &owner_confirmation, 4096).map_err(|_| ())?;
            let owner: OwnerReport = serde_json::from_slice(&bytes).map_err(|_| ())?;
            if owner.phase != phase || owner.invocation != invocation || !owner.menu_confirmed {
                return Err(());
            }
            let verdict = inspect_phase(&descriptor, root, &invocation).map_err(|_| ())?;
            publish_completion(
                root,
                &descriptor,
                &verdict,
                &OwnerConfirmation {
                    phase: owner.phase,
                    invocation: owner.invocation,
                    menu_confirmed: owner.menu_confirmed,
                },
            )
            .map_err(|_| ())
        }
        Command::SealExit {
            descriptor,
            manager_receipts,
            store,
        } => {
            let phase = descriptor_phase(root, &descriptor)?;
            let manager = read_private_input(root, &manager_receipts, 32 * 1024).map_err(|_| ())?;
            let invocation = manager_invocation(&manager, phase, true)?;
            seal_exit_store(&descriptor, root, &invocation, &store).map_err(|_| ())
        }
    }
}

fn abort_on_failure(root: &Path, result: Checked<()>) -> Checked<()> {
    if result.is_err() {
        let _ = publish_abort(root);
    }
    result
}

fn main() -> std::process::ExitCode {
    let root = std::env::current_dir();
    // Clap's usual diagnostics may echo input values. Every failure here uses
    // one fixed record, including malformed arguments and receipt parsing.
    let mut prepared = std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "prepare");
    let mut recording = false;
    let result = root.as_ref().map_err(|_| ()).and_then(|root| {
        let result = Cli::try_parse().map_err(|_| ()).and_then(|cli| {
            prepared = matches!(&cli.command, Command::Prepare { .. });
            recording = matches!(&cli.command, Command::RecordMenu { .. });
            run(root, cli.command)
        });
        // Pure preflight must not create even an abort marker. A failed live
        // observation/seal/menu recording publishes abort and cannot mint success.
        if prepared {
            result
        } else {
            abort_on_failure(root, result)
        }
    });
    if result.is_ok() {
        if prepared {
            println!(
                "{{\"event\":\"private_proof_prepared\",\"qualified_preflight_passed\":true}}"
            );
        } else if recording {
            println!("{{\"event\":\"private_owner_menu_recorded\",\"recorded\":true}}");
        } else {
            println!("{{\"event\":\"private_proof_observed\",\"pass\":true}}");
        }
        std::process::ExitCode::SUCCESS
    } else {
        // Failure cannot create a completion marker. If abort publication
        // also fails, the nonzero observer exit leaves the deadline active.
        println!("{{\"event\":\"private_proof_refused\",\"pass\":false}}");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
#[path = "../tests/pak_writer/mod.rs"]
mod pak_writer;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    fn receipt(kind: &str, phase: u8, invocation: &str) -> serde_json::Value {
        serde_json::json!({"kind":kind,"phase":phase,"invocation":invocation,
            "result":"success","native_executed":true,"exit_code":"exited",
            "exit_status":0,"pid_zero":kind == "after_exit"})
    }
    fn journal(rows: &[serde_json::Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for row in rows {
            bytes.extend(serde_json::to_vec(row).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }
    fn menu_fixture(root: &Path, phase: u8, rows: &[serde_json::Value]) -> (PathBuf, PathBuf) {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let descriptor = root.join("descriptor.json");
        let manager = root.join("manager.jsonl");
        let binding = serde_json::json!({"format":1,
            "case_id":root.file_name().unwrap().to_str().unwrap(),"phase":phase,
            "output":format!("phase-{phase}.jsonl"),
            "predecessor":(phase==2).then_some("phase-1.jsonl"),
            "exit_seal":(phase==2).then_some("phase-1-exit.json")});
        for (path, bytes) in [
            (&descriptor, serde_json::to_vec(&binding).unwrap()),
            (&manager, journal(rows)),
        ] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        (descriptor, manager)
    }

    #[test]
    fn record_menu_binds_matching_menu_to_current_created_receipt() {
        let first = "0123456789abcdef0123456789abcdef";
        let second = "123456789abcdef0123456789abcdef0";
        for (phase, reported_menu, invocation) in [
            (1, ReportedMenu::CharacterSelection, first),
            (2, ReportedMenu::Title, second),
        ] {
            let root = tempfile::tempdir().unwrap();
            let rows = if phase == 1 {
                vec![receipt("created", 1, first)]
            } else {
                vec![
                    receipt("created", 1, first),
                    receipt("before_stop", 1, first),
                    receipt("after_exit", 1, first),
                    receipt("created", 2, second),
                ]
            };
            let (descriptor, manager_receipts) = menu_fixture(root.path(), phase, &rows);
            assert!(
                run(
                    root.path(),
                    Command::RecordMenu {
                        descriptor,
                        manager_receipts,
                        reported_menu,
                    }
                )
                .is_ok()
            );
            let bytes = fs::read(root.path().join(format!("phase-{phase}-owner.json"))).unwrap();
            let owner: OwnerReport = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(owner.phase, phase);
            assert_eq!(owner.invocation, invocation);
            assert!(owner.menu_confirmed);
            assert!(
                !root
                    .path()
                    .join(format!("phase-{phase}-complete.json"))
                    .exists()
            );
        }
    }

    #[test]
    fn record_menu_refuses_wrong_menu_stopped_duplicate_and_stale_prefixes() {
        let first = "0123456789abcdef0123456789abcdef";
        let second = "123456789abcdef0123456789abcdef0";
        for variant in ["menu1", "menu2", "stopped", "duplicate", "stale"] {
            let root = tempfile::tempdir().unwrap();
            let phase = if matches!(variant, "menu2" | "stale") {
                2
            } else {
                1
            };
            let mut rows = vec![receipt("created", 1, first)];
            if phase == 2 {
                rows.extend([
                    receipt("before_stop", 1, first),
                    receipt("after_exit", 1, first),
                    receipt(
                        "created",
                        2,
                        if variant == "stale" { first } else { second },
                    ),
                ]);
            } else if variant == "stopped" {
                rows.push(receipt("before_stop", 1, first));
            } else if variant == "duplicate" {
                rows.push(receipt("created", 1, first));
            }
            let reported_menu = match variant {
                "menu1" => ReportedMenu::Title,
                "menu2" => ReportedMenu::CharacterSelection,
                "stale" => ReportedMenu::Title,
                _ => ReportedMenu::CharacterSelection,
            };
            let (descriptor, manager_receipts) = menu_fixture(root.path(), phase, &rows);
            assert!(
                run(
                    root.path(),
                    Command::RecordMenu {
                        descriptor,
                        manager_receipts,
                        reported_menu,
                    }
                )
                .is_err()
            );
            assert!(
                !root
                    .path()
                    .join(format!("phase-{phase}-owner.json"))
                    .exists()
            );
            assert!(
                !root
                    .path()
                    .join(format!("phase-{phase}-owner.pending.json"))
                    .exists()
            );
        }
        assert!(
            Cli::try_parse_from([
                "observer",
                "record-menu",
                "--descriptor",
                "descriptor.json",
                "--manager-receipts",
                "manager.jsonl",
                "--reported-menu",
                "title",
                "--invocation",
                first,
            ])
            .is_err()
        );
    }

    #[test]
    fn manager_prefix_refuses_order_duplicates_stale_invocations_and_failed_exit() {
        let first = "0123456789abcdef0123456789abcdef";
        let second = "123456789abcdef0123456789abcdef0";
        let rows = vec![
            receipt("created", 1, first),
            receipt("before_stop", 1, first),
            receipt("after_exit", 1, first),
            receipt("created", 2, second),
        ];
        assert_eq!(
            manager_invocation(&journal(&rows), 2, false).unwrap(),
            second
        );
        for variant in 0..5 {
            let mut wrong = rows.clone();
            match variant {
                0 => wrong.swap(0, 1),
                1 => wrong.push(wrong[3].clone()),
                2 => wrong[3]["invocation"] = first.into(),
                3 => wrong[2]["exit_status"] = 1.into(),
                _ => wrong[1]["invocation"] = second.into(),
            }
            assert!(manager_invocation(&journal(&wrong), 2, false).is_err());
        }
        let mut truncated = journal(&rows);
        truncated.pop();
        assert!(manager_invocation(&truncated, 2, false).is_err());
    }
    #[test]
    fn sealing_needs_successful_exit_and_observing_requires_current_created_only() {
        let id = "0123456789abcdef0123456789abcdef";
        let mut rows = vec![receipt("created", 1, id)];
        assert!(manager_invocation(&journal(&rows), 1, false).is_ok());
        assert!(manager_invocation(&journal(&rows), 1, true).is_err());
        rows.extend([receipt("before_stop", 1, id), receipt("after_exit", 1, id)]);
        assert!(manager_invocation(&journal(&rows), 1, true).is_ok());
        assert!(manager_invocation(&journal(&rows), 1, false).is_err());
    }

    #[test]
    fn stale_owner_or_reordered_manager_aborts_without_completion_marker() {
        let current = "0123456789abcdef0123456789abcdef";
        let stale = "123456789abcdef0123456789abcdef0";
        for wrong_owner in [true, false] {
            let root = tempfile::Builder::new()
                .prefix("authored-proof-")
                .tempdir()
                .unwrap();
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let descriptor = root.path().join("descriptor.json");
            let manager = root.path().join("manager.jsonl");
            let owner = root.path().join("owner.json");
            let binding = serde_json::json!({"format":1,
                "case_id":root.path().file_name().unwrap().to_str().unwrap(),"phase":1,
                "output":"phase-1.jsonl","predecessor":null,"exit_seal":null});
            let report = serde_json::json!({"phase":1,"invocation":if wrong_owner {stale} else {current},
                "menu_confirmed":true});
            for (path, bytes) in [
                (&descriptor, serde_json::to_vec(&binding).unwrap()),
                (
                    &manager,
                    journal(&[receipt(
                        if wrong_owner {
                            "created"
                        } else {
                            "before_stop"
                        },
                        1,
                        current,
                    )]),
                ),
                (&owner, serde_json::to_vec(&report).unwrap()),
            ] {
                fs::write(path, bytes).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let result = run(
                root.path(),
                Command::Observe {
                    descriptor,
                    manager_receipts: manager,
                    owner_confirmation: owner,
                },
            );
            assert!(abort_on_failure(root.path(), result).is_err());
            assert!(root.path().join("abort.json").is_file());
            assert!(!root.path().join("phase-1-complete.json").exists());
        }
    }

    #[test]
    fn pure_preparation_qualifies_authored_inputs_with_ports_occupied_and_creates_nothing() {
        use icarus_bevy::{
            authored_world_bootstrap::AuthoredWorldBootstrap,
            config::{NetworkConfig, PlayerPersistenceConfig, ServerConfig},
            data_pak::DataPak,
            experimental::release::ReleaseProfile,
            gameplay_profile::{FreshPlayerValues, fresh_player_profile},
            native_hosting::HostingConfig,
            native_player_runtime::ConsumptionConfig,
        };
        use std::net::{Ipv4Addr, UdpSocket};
        let root = tempfile::Builder::new()
            .prefix("authored-proof-")
            .tempdir()
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let catalog = root.path().join("authored.pak");
        // Hosting resolves this declared prospect against the protocol catalog
        // during pure preparation, before any listener or service exists.
        fs::write(
            &catalog,
            pak_writer::fresh_join_pak_with(&[(
                "Prospects/D_ProspectList.json",
                pak_writer::table_json(&["AuthoredWorkshop"]),
            )]),
        )
        .unwrap();
        let recipe = root.path().join("authored.json");
        let gameplay = fresh_player_profile(
            &mut DataPak::open(&catalog).unwrap(),
            &FreshPlayerValues::default(),
        )
        .unwrap();
        fs::write(&recipe, serde_json::to_vec(&gameplay).unwrap()).unwrap();
        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        let query = UdpSocket::bind("127.0.0.1:0").unwrap();
        let network = NetworkConfig {
            enabled: true,
            max_players: 1,
            authored_controller_leave: true,
            player_consumption: Some(ConsumptionConfig {
                oxygen: 0,
                water: 0,
                food: 1,
            }),
            port: game.local_addr().unwrap().port(),
            player_persistence: Some(PlayerPersistenceConfig {
                prospect_id: "authored-proof".into(),
                map_url: "/Authored/Maps/Workshop".into(),
                prospect_row: "AuthoredWorkshop".into(),
                difficulty: 2,
            }),
            release_profile: Some(ReleaseProfile::fixture_path("3.0.36")),
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
                query_port: query.local_addr().unwrap().port(),
                private_ip: Ipv4Addr::LOCALHOST,
                seconds: 60,
                name: "Authored proof".into(),
                map_url: "/Authored/Maps/Workshop".into(),
                prospect_id: "authored-proof".into(),
                prospect_row: "AuthoredWorkshop".into(),
                lobby_name: "Authored proof".into(),
                difficulty: 2,
            }),
            ..Default::default()
        };
        let config = root.path().join("server.toml");
        let mut configured = toml::Value::try_from(&ServerConfig {
            data_dir: root.path().join("state"),
            network,
            ..Default::default()
        })
        .unwrap();
        configured["network"]
            .as_table_mut()
            .unwrap()
            .insert("join_password".into(), "authored-test-only".into());
        fs::write(&config, toml::to_string(&configured).unwrap()).unwrap();
        let descriptor = root.path().join("descriptor.json");
        let binding = serde_json::json!({"format":1,
            "case_id":root.path().file_name().unwrap().to_str().unwrap(),"phase":1,
            "output":"phase-1.jsonl","predecessor":null,"exit_seal":null});
        fs::write(&descriptor, serde_json::to_vec(&binding).unwrap()).unwrap();
        fs::set_permissions(&descriptor, fs::Permissions::from_mode(0o600)).unwrap();
        let mut before: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(prepare(root.path(), config.clone(), &descriptor).is_ok());
        for variant in ["empty_password", "oxygen", "water", "food"] {
            let mut invalid = configured.clone();
            match variant {
                "empty_password" => invalid["network"]["join_password"] = "".into(),
                "oxygen" | "water" => invalid["network"]["player_consumption"][variant] = 1.into(),
                _ => invalid["network"]["player_consumption"]["food"] = 2.into(),
            }
            fs::write(&config, toml::to_string(&invalid).unwrap()).unwrap();
            assert!(prepare(root.path(), config.clone(), &descriptor).is_err());
        }
        fs::write(&config, toml::to_string(&configured).unwrap()).unwrap();
        let oversized = fresh_player_profile(
            &mut DataPak::open(Path::new(
                configured["network"]["data_pak"].as_str().unwrap(),
            ))
            .unwrap(),
            &FreshPlayerValues {
                max_food: 101,
                ..Default::default()
            },
        )
        .unwrap();
        fs::write(
            configured["network"]["authored_gameplay"].as_str().unwrap(),
            serde_json::to_vec(&oversized).unwrap(),
        )
        .unwrap();
        assert!(prepare(root.path(), config, &descriptor).is_err());
        let mut after: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        before.sort();
        after.sort();
        assert_eq!(before, after);
        assert!(!root.path().join("state").exists());
        assert!(!root.path().join("phase-1.jsonl").exists());
        assert!(!root.path().join("abort.json").exists());
        assert!(!root.path().join("phase-1-complete.json").exists());
    }
}
