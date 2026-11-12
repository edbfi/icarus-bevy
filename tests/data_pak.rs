//! The official server's `data.pak` reader against authored paks.
#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::fs;

use icarus_bevy::data_pak::{DataPak, DataTables};
use pak_writer::{table_json, write_pak, write_pak_blocks};
use tempfile::TempDir;

fn pak_file(root: &TempDir, bytes: &[u8]) -> std::path::PathBuf {
    let path = root.path().join("data.pak");
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn stored_and_zlib_members_read_back_exactly() {
    let root = TempDir::new().unwrap();
    let big: Vec<u8> = (0..50_000_u32)
        .flat_map(|i| (i % 251).to_le_bytes())
        .collect();
    let path = pak_file(
        &root,
        &write_pak(&[
            ("Flags/D_SessionFlags.json", &table_json(&["A", "B"]), true),
            ("Stats/D_Stats.json", &table_json(&["Health"]), false),
            ("Blob/big.bin", &big, true),
        ]),
    );
    let mut pak = DataPak::open(&path).unwrap();
    assert_eq!(pak.members().count(), 3);
    assert_eq!(pak.read("big.bin").unwrap(), big);
    assert_eq!(pak.read("Blob/big.bin").unwrap(), big);
    assert_eq!(pak.table_rows("D_SessionFlags").unwrap(), ["A", "B"]);
    assert_eq!(pak.table_rows("D_Stats").unwrap(), ["Health"]);
    assert!(pak.read("missing.json").is_err());
    // A suffix that is not a whole path component does not match.
    assert!(pak.read("ig.bin").is_err());
    assert!(!format!("{pak:?}").contains("Flags"));
}

#[test]
fn row_indices_follow_the_rows_order() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &write_pak(&[(
            "Flags/D_SessionFlags.json",
            &table_json(&["Tutorial", "Solo", "Mission"]),
            true,
        )]),
    );
    let mut tables = DataTables::open(&path).unwrap();
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    assert_eq!(
        tables
            .row_index("D_SessionFlags", &units("Mission"))
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        tables
            .row_index("D_SessionFlags", &units("Tutorial"))
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        tables
            .row_index("D_SessionFlags", &units("Absent"))
            .unwrap(),
        None
    );
    assert!(tables.rows("D_Missing").is_err());
}

#[test]
fn runtime_reads_authored_values_with_catalog_row_identity() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &pak_writer::official_pak(&[("Flags/D_SessionFlags.json", table_json(&["Authored_Flag"]))]),
    );
    let authored = pak_writer::authored_gameplay(&path);
    let mut profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&authored).unwrap()).unwrap();
    profile["tables"]["D_CharacterStartingStats"][0]["StatsGranted"]["(Value=\"BaseMaximumHealth_+\")"] =
        713.into();
    profile["tables"]["D_FactionMissions"][0]["bUseOpenWorldRetryTimeout"] = false.into();
    fs::write(&authored, serde_json::to_vec(&profile).unwrap()).unwrap();
    let mut bound = icarus_bevy::network::bind(&icarus_bevy::config::NetworkConfig {
        enabled: true,
        port: 0,
        data_pak: Some(path.clone()),
        authored_gameplay: Some(authored.clone()),
        ..Default::default()
    })
    .unwrap()
    .unwrap();
    let tables = bound.tables.as_mut().unwrap();
    assert_eq!(
        icarus_bevy::network::fresh_player_vitals(tables)
            .unwrap()
            .health,
        713
    );
    assert_eq!(
        tables
            .row_bool("D_FactionMissions", "bUseOpenWorldRetryTimeout", 0)
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        tables.rows("D_CharacterGrowth").unwrap(),
        ["Creature", "Player"]
    );
    assert_eq!(
        tables
            .row_index(
                "D_CharacterGrowth",
                &"Player".encode_utf16().collect::<Vec<_>>()
            )
            .unwrap(),
        Some(1)
    );
    let mut reference = DataTables::open(&path).unwrap();
    assert_eq!(
        icarus_bevy::network::fresh_player_vitals(&mut reference)
            .unwrap()
            .health,
        250
    );
    assert_eq!(
        reference
            .row_bool("D_FactionMissions", "bUseOpenWorldRetryTimeout", 0)
            .unwrap(),
        None
    );

    // Names remain available for a metadata-only table, but neither object nor
    // bool access can fall back to the packaged values, including after caching.
    profile["tables"]
        .as_object_mut()
        .unwrap()
        .remove("D_CharacterStartingStats");
    fs::write(&authored, serde_json::to_vec(&profile).unwrap()).unwrap();
    let mut missing = DataTables::runtime(&path, &authored).unwrap();
    assert_eq!(
        missing.rows("D_CharacterStartingStats").unwrap(),
        ["Base_Stats"]
    );
    for _ in 0..2 {
        assert!(
            missing
                .objects("D_CharacterStartingStats")
                .unwrap_err()
                .to_string()
                .contains("missing authored gameplay")
        );
        assert!(
            missing
                .row_bool("D_CharacterStartingStats", "Flag", 0)
                .is_err()
        );
    }
}

#[test]
fn authored_gameplay_rejects_misaligned_rows_and_invalid_inputs() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &write_pak(&[("D_Test.json", &table_json(&["A", "B"]), false)]),
    );
    let authored = root.path().join("gameplay.json");
    for rows in [
        serde_json::json!([{"Name":"A"}]),
        serde_json::json!([{"Name":"B"},{"Name":"A"}]),
        serde_json::json!([{"Name":"A"},{"Name":"A"}]),
        serde_json::json!([{"Name":"A"},{"Name":"Unknown"}]),
    ] {
        fs::write(
            &authored,
            serde_json::to_vec(&serde_json::json!({"schema_version":1,"tables":{"D_Test": rows}}))
                .unwrap(),
        )
        .unwrap();
        assert!(
            DataTables::runtime(&path, &authored)
                .unwrap_err()
                .to_string()
                .contains("catalog count, names and order")
        );
    }
    for value in [
        serde_json::json!({"schema_version":2,"tables":{}}),
        serde_json::json!({"schema_version":1,"tables":{},"unknown":1}),
        serde_json::json!({"schema_version":1,"tables":{"D_Missing":[]}}),
    ] {
        fs::write(&authored, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(DataTables::runtime(&path, &authored).is_err());
    }
    fs::File::create(&authored)
        .unwrap()
        .set_len(icarus_bevy::data_pak::MAX_GAMEPLAY_BYTES + 1)
        .unwrap();
    assert!(
        DataTables::runtime(&path, &authored)
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
    assert!(DataTables::runtime(&path, root.path()).is_err());
    let fifo = root.path().join("gameplay.fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(DataTables::runtime(&path, &fifo).is_err());
}

#[test]
fn runtime_requires_both_identity_catalog_and_authored_gameplay() {
    for (pak, gameplay) in [(true, false), (false, true)] {
        let config = icarus_bevy::config::NetworkConfig {
            enabled: true,
            port: 0,
            data_pak: pak.then(|| "absent.pak".into()),
            authored_gameplay: gameplay.then(|| "absent.json".into()),
            ..Default::default()
        };
        let error = icarus_bevy::network::bind(&config).err().unwrap();
        assert!(error.to_string().contains("configured together"));
    }
}

#[test]
fn installed_pak_does_not_enable_packaged_gameplay_without_an_authored_profile() {
    let root = TempDir::new().unwrap();
    let pak = root
        .path()
        .join(icarus_bevy::network::OFFICIAL_DIR)
        .join(icarus_bevy::network::OFFICIAL_DATA_PAK);
    fs::create_dir_all(pak.parent().unwrap()).unwrap();
    // Discovery checks existence, but startup must reject the absent authored
    // profile before trying to interpret even the catalog.
    fs::write(&pak, b"not a pak").unwrap();
    let config = root.path().join("server.toml");
    fs::write(&config, "[network]\nenabled = true\n").unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["run", "--data-dir"])
        .arg(root.path())
        .arg("--config")
        .arg(config)
        .args(["--network-port", "0", "--max-ticks", "1", "--compressed"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(
        error.contains("network.authored_gameplay must be configured together"),
        "{error}"
    );
}

#[test]
fn malformed_paks_fail_closed() {
    let root = TempDir::new().unwrap();
    let good = write_pak(&[("D_X.json", &table_json(&["A"]), false)]);
    let footer = good.len() - 221;
    for (what, mutate) in [
        (
            "encrypted index",
            Box::new(|bytes: &mut Vec<u8>| bytes[footer + 16] = 1) as Box<dyn Fn(&mut Vec<u8>)>,
        ),
        (
            "magic",
            Box::new(|bytes: &mut Vec<u8>| bytes[footer + 17] ^= 0xff),
        ),
        (
            "version",
            Box::new(|bytes: &mut Vec<u8>| bytes[footer + 21] = 10),
        ),
        (
            "index outside the file",
            Box::new(|bytes: &mut Vec<u8>| bytes[footer + 25 + 7] = 0x7f),
        ),
        (
            "truncated",
            Box::new(|bytes: &mut Vec<u8>| bytes.truncate(100)),
        ),
    ] {
        let mut bytes = good.clone();
        mutate(&mut bytes);
        let path = pak_file(&root, &bytes);
        assert!(DataPak::open(&path).is_err(), "{what}");
    }
    // A directory or FIFO is not a regular file.
    assert!(DataPak::open(root.path()).is_err());
    let fifo = root.path().join("fifo.pak");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(DataPak::open(&fifo).is_err());
    // A table that is not JSON with named rows is rejected.
    let path = pak_file(
        &root,
        &write_pak(&[("D_Bad.json", b"{\"Rows\": [1]}", false)]),
    );
    assert!(DataPak::open(&path).unwrap().table_rows("D_Bad").is_err());
}

#[test]
fn multi_block_members_inflate_in_order_within_their_size() {
    let root = TempDir::new().unwrap();
    let data: Vec<u8> = (0..40_000_u32)
        .flat_map(|i| (i * 7).to_le_bytes())
        .collect();
    let path = pak_file(&root, &write_pak_blocks(&[("D_Big.bin", &data, 3)], 0));
    let mut pak = DataPak::open(&path).unwrap();
    assert_eq!(pak.read("D_Big.bin").unwrap(), data);
    // A member whose blocks inflate past its declared size is rejected.
    let path = pak_file(&root, &write_pak_blocks(&[("D_Big.bin", &data, 3)], 10));
    let error = DataPak::open(&path).unwrap().read("D_Big.bin").unwrap_err();
    assert!(format!("{error:#}").contains("past its size"), "{error:#}");
}

#[test]
fn a_data_pak_without_the_session_flags_table_fails_before_binding() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &write_pak(&[("D_Other.json", &table_json(&["A"]), false)]),
    );
    let network = icarus_bevy::config::NetworkConfig {
        enabled: true,
        port: 0,
        authored_gameplay: Some(pak_writer::authored_gameplay(&path)),
        data_pak: Some(path),
        ..Default::default()
    };
    let error = icarus_bevy::network::bind(&network)
        .err()
        .expect("bind fails");
    let text = format!("{error:#}");
    assert!(text.contains("network.data_pak"), "{text}");
    assert!(
        text.contains("data.pak"),
        "the message names the file: {text}"
    );
    assert!(text.contains("not found"), "the cause is kept: {text}");
}

#[test]
fn the_install_record_is_published_whole_and_replaced() {
    let root = TempDir::new().unwrap();
    let first = serde_json::json!({"build_id": 1});
    icarus_bevy::network::write_install_record(root.path(), &first).unwrap();
    let read = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_slice(&fs::read(path.join("install.json")).unwrap()).unwrap()
    };
    assert_eq!(read(root.path()), first);
    let second = serde_json::json!({"build_id": 2});
    icarus_bevy::network::write_install_record(root.path(), &second).unwrap();
    assert_eq!(read(root.path()), second);
    assert!(!root.path().join("install.json.tmp").exists());
}

#[test]
fn bool_columns_keep_omitted_fields_apart_and_reject_other_types() {
    let root = TempDir::new().unwrap();
    let table = serde_json::to_vec(&serde_json::json!({"Rows": [
        {"Name": "A"},
        {"Name": "B", "bFlag": false},
        {"Name": "C", "bFlag": true},
        {"Name": "D", "bOther": 3},
    ]}))
    .unwrap();
    let wrong =
        serde_json::to_vec(&serde_json::json!({"Rows": [{"Name": "A", "bFlag": 1}]})).unwrap();
    let path = pak_file(
        &root,
        &write_pak(&[
            ("Factions/D_Table.json", &table, true),
            ("D_Wrong.json", &wrong, false),
        ]),
    );
    let mut pak = DataPak::open(&path).unwrap();
    assert_eq!(
        pak.table_bool_column("D_Table", "bFlag").unwrap(),
        [None, Some(false), Some(true), None]
    );
    assert!(pak.table_bool_column("D_Table", "bOther").is_err());
    assert!(pak.table_bool_column("D_Wrong", "bFlag").is_err());
    let mut tables = DataTables::open(&path).unwrap();
    assert_eq!(tables.row_bool("D_Table", "bFlag", 1).unwrap(), Some(false));
    assert_eq!(tables.row_bool("D_Table", "bFlag", 2).unwrap(), Some(true));
    assert_eq!(tables.row_bool("D_Table", "bFlag", 0).unwrap(), None);
    assert_eq!(tables.row_bool("D_Table", "bFlag", 9).unwrap(), None);
    assert!(tables.row_bool("D_Missing", "bFlag", 0).is_err());
}

#[test]
fn a_data_pak_without_the_faction_missions_table_fails_before_binding() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &write_pak(&[("Flags/D_SessionFlags.json", &table_json(&["A"]), false)]),
    );
    let network = icarus_bevy::config::NetworkConfig {
        enabled: true,
        port: 0,
        authored_gameplay: Some(pak_writer::authored_gameplay(&path)),
        data_pak: Some(path),
        ..Default::default()
    };
    let error = icarus_bevy::network::bind(&network)
        .err()
        .expect("bind fails");
    let text = format!("{error:#}");
    assert!(text.contains("network.data_pak"), "{text}");
    assert!(text.contains("not found"), "the cause is kept: {text}");
}

#[test]
fn rows_read_back_as_json_objects_by_name() {
    let root = TempDir::new().unwrap();
    let path = pak_file(&root, &pak_writer::official_pak(&[]));
    let mut tables = DataTables::open(&path).unwrap();
    let row = tables
        .row("D_InventoryInfo", "Hands")
        .unwrap()
        .expect("the Hands row");
    assert_eq!(row["StartingSlots"], 3);
    assert!(tables.row("D_InventoryInfo", "Absent").unwrap().is_none());
    assert!(tables.row("D_Missing", "Hands").is_err());
}

/// A fresh player's pawn content from the data tables (F-836): vitals full
/// at the granted maxima, growth row `Player`, and the player's inventories
/// in listed order with their slots' queries.
#[test]
fn a_fresh_players_vitals_and_inventories_come_from_the_tables() {
    use icarus_bevy::experimental::pawn_open::{
        CharacterVitals, FastArrayContent, InventorySlot, PawnInventory,
    };
    let root = TempDir::new().unwrap();
    let path = pak_file(&root, &pak_writer::official_pak(&[]));
    let mut tables = DataTables::open(&path).unwrap();
    let content = icarus_bevy::network::pawn_content(Some(&mut tables)).unwrap();
    let vitals = CharacterVitals {
        health: 250,
        max_health: 250,
        stamina: 150,
        max_stamina: 150,
        oxygen: 220,
        max_oxygen: 220,
        water: 230,
        max_water: 230,
        food: 240,
        max_food: 240,
        biome_row: 0,
        growth_row: 1,
    };
    assert_eq!(content.character_state, vitals.values());
    assert!(content.frozen_movement);
    let slot = InventorySlot::empty;
    assert_eq!(
        content.inventories,
        [
            PawnInventory {
                info_row: 2,
                current_weight: None,
                slots: FastArrayContent {
                    replication_key: 2,
                    items: Vec::new(),
                },
            },
            PawnInventory {
                info_row: 1,
                current_weight: None,
                slots: FastArrayContent {
                    replication_key: 2,
                    items: vec![slot(1, 0), slot(1, 1), slot(2, 2)],
                },
            },
        ]
    );
    // Without tables: lab vitals and no inventories.
    let lab = icarus_bevy::network::pawn_content(None).unwrap();
    assert!(lab.inventories.is_empty());
    assert_ne!(lab.character_state, vitals.values());
}

#[test]
fn missing_or_malformed_player_rows_fail_before_binding() {
    let root = TempDir::new().unwrap();
    let bind = |pak: Vec<u8>| {
        let path = pak_file(&root, &pak);
        let network = icarus_bevy::config::NetworkConfig {
            enabled: true,
            port: 0,
            authored_gameplay: Some(pak_writer::authored_gameplay(&path)),
            data_pak: Some(path),
            ..Default::default()
        };
        format!(
            "{:#}",
            icarus_bevy::network::bind(&network)
                .err()
                .expect("bind fails")
        )
    };
    let flags = ("Flags/D_SessionFlags.json", table_json(&["A"]));
    // The complete authored set binds.
    let path = pak_file(
        &root,
        &pak_writer::official_pak(std::slice::from_ref(&flags)),
    );
    let network = icarus_bevy::config::NetworkConfig {
        enabled: true,
        port: 0,
        authored_gameplay: Some(pak_writer::authored_gameplay(&path)),
        data_pak: Some(path),
        ..Default::default()
    };
    assert!(icarus_bevy::network::bind(&network).unwrap().is_some());
    // No Player row, a slot query the tag queries lack, no starting stats.
    for (member, bytes, expected) in [
        (
            "Traits/D_Inventory.json",
            table_json(&["Crate"]),
            "no Player row",
        ),
        (
            "Tags/D_TagQueries.json",
            table_json(&["Q0", "Any"]),
            "lacks a slot query",
        ),
        (
            "Stats/D_CharacterStartingStats.json",
            table_json(&["Base_Stats"]),
            "grant no map",
        ),
        (
            "Inventory/D_InventoryInfo.json",
            serde_json::to_vec(&serde_json::json!({"Rows": [
                {"Name": "Hands", "SlotTemplate": {"RowName": "Any"}, "StartingSlots": 1,
                 "SlotOverrides": [{"Query": {"RowName": "Fists"}}]},
                {"Name": "Bag"},
            ]}))
            .unwrap(),
            "lacks its Location or Query row",
        ),
    ] {
        let text = bind(pak_writer::official_pak(&[flags.clone(), (member, bytes)]));
        assert!(text.contains("network.data_pak"), "{text}");
        assert!(text.contains(expected), "{expected}: {text}");
    }
}

/// `has_table` reports whether a data table member is in the pak, by the
/// same path suffix rule `read` uses.
#[test]
fn table_presence_follows_the_member_paths() {
    let root = TempDir::new().unwrap();
    let path = pak_file(
        &root,
        &pak_writer::write_pak(&[(
            "Flags/D_SessionFlags.json",
            &pak_writer::table_json(&["Flag_A"]),
            true,
        )]),
    );
    let tables = icarus_bevy::data_pak::DataTables::open(&path).unwrap();
    assert!(tables.has_table("D_SessionFlags"));
    assert!(!tables.has_table("D_ProspectList"));
    assert!(!tables.has_table("SessionFlags"));
}

/// A fresh-player profile over a pak's catalog (F-993): the catalog's row
/// names in order, only the values authored here, and a fresh pawn's stats
/// exactly the five speeds at their stat IDs (F-838, F-1164) and values:
/// never the maxima at stats 0 and 3, which the client divides by the
/// character state's (F-1166), though the vitals still carry them.
#[test]
fn a_fresh_player_profile_authors_every_value_over_the_catalog_names() {
    use icarus_bevy::gameplay_profile::{FreshPlayerValues, fresh_player_profile};
    let root = TempDir::new().unwrap();
    let path = pak_file(&root, &pak_writer::fresh_join_pak());
    let values = FreshPlayerValues::default();
    let profile = fresh_player_profile(&mut DataPak::open(&path).unwrap(), &values).unwrap();
    let tables = profile["tables"].as_object().unwrap();
    // The fresh-join tables this pak holds (no quests or modifier states).
    assert_eq!(
        tables.keys().collect::<Vec<_>>(),
        [
            "D_CharacterStartingStats",
            "D_FactionMissions",
            "D_Inventory",
            "D_Stats"
        ]
    );
    let mut pak = DataPak::open(&path).unwrap();
    for (table, rows) in tables {
        let names: Vec<&str> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["Name"].as_str().unwrap())
            .collect();
        assert_eq!(names, pak.table_rows(table).unwrap(), "{table}");
    }
    // The packaged filler's bIsReplicated is not copied.
    assert!(tables["D_Stats"][20].get("bIsReplicated").is_none());
    assert_eq!(tables["D_Stats"][58]["bIsReplicated"], true);
    // Only the five speeds replicate: not the maxima at 0 and 3, no base.
    let replicated: Vec<usize> = tables["D_Stats"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, row)| row.get("bIsReplicated").is_some())
        .map(|(id, _)| id)
        .collect();
    assert_eq!(replicated, [58, 61, 65, 69, 73]);
    assert_eq!(
        tables["D_Inventory"][1]["Inventories"],
        serde_json::json!([])
    );
    let authored = root.path().join("fresh.json");
    fs::write(&authored, serde_json::to_vec(&profile).unwrap()).unwrap();
    let network = icarus_bevy::config::NetworkConfig {
        data_pak: Some(path.clone()),
        authored_gameplay: Some(authored),
        ..Default::default()
    };
    let mut loaded = icarus_bevy::network::data_tables(&network)
        .unwrap()
        .unwrap();
    // Vitals at the authored maxima, not the pak's 250 and more.
    let vitals = icarus_bevy::network::fresh_player_vitals(&mut loaded).unwrap();
    assert_eq!(
        (
            vitals.health,
            vitals.max_stamina,
            vitals.oxygen,
            vitals.water,
            vitals.food
        ),
        (100, 100, 100, 100, 100)
    );
    let stats = icarus_bevy::pawn_stats::fresh_player_stats(
        &mut loaded,
        "D_CharacterStartingStats",
        "Base_Stats",
    )
    .unwrap();
    assert_eq!(stats.replication_key, 5);
    let pairs: Vec<(u32, i32)> = stats
        .items
        .iter()
        .map(|pair| (pair.stat, pair.value))
        .collect();
    assert_eq!(
        pairs,
        [(58, 600), (61, 900), (65, 300), (69, 300), (73, 450)]
    );
    assert_eq!(
        icarus_bevy::network::fresh_pawn_stat_count(Some(&mut loaded)).unwrap(),
        5
    );
    assert_eq!(
        icarus_bevy::network::fresh_pawn_stat_count(None).unwrap(),
        0
    );
    // Other authored speeds reach the stats as given.
    let slow = FreshPlayerValues {
        walk_speed: 123,
        ..values
    };
    let profile = fresh_player_profile(&mut DataPak::open(&path).unwrap(), &slow).unwrap();
    let authored = root.path().join("slow.json");
    fs::write(&authored, serde_json::to_vec(&profile).unwrap()).unwrap();
    let mut loaded = DataTables::runtime(&path, &authored).unwrap();
    let stats = icarus_bevy::pawn_stats::fresh_player_stats(
        &mut loaded,
        "D_CharacterStartingStats",
        "Base_Stats",
    )
    .unwrap();
    assert!(
        stats
            .items
            .iter()
            .any(|pair| (pair.stat, pair.value) == (58, 123))
    );
}

#[test]
fn fresh_player_profiles_refuse_unusable_values_and_incomplete_catalogs() {
    use icarus_bevy::{
        experimental::stat_restore::OVERBURDENED_WEIGHT_PER_CAPACITY,
        gameplay_profile::{
            FreshPlayerValues, MAX_VALUE, MAX_WEIGHT_CAPACITY, catalog_skeleton,
            fresh_player_profile,
        },
    };
    let root = TempDir::new().unwrap();
    let path = pak_file(&root, &pak_writer::fresh_join_pak());
    let defaults = FreshPlayerValues::default();
    for bad in [
        FreshPlayerValues {
            walk_speed: 0,
            ..defaults
        },
        FreshPlayerValues {
            max_health: -1,
            ..defaults
        },
        FreshPlayerValues {
            sprint_speed: MAX_VALUE + 1,
            ..defaults
        },
        FreshPlayerValues {
            weight_capacity: MAX_WEIGHT_CAPACITY + 1,
            ..defaults
        },
        FreshPlayerValues {
            weight_capacity: MAX_VALUE,
            ..defaults
        },
    ] {
        assert!(bad.validate().is_err());
        assert!(fresh_player_profile(&mut DataPak::open(&path).unwrap(), &bad).is_err());
    }
    // The largest accepted values: the weight capacity times the restore's
    // Overburdened scale (F-986) still fits 32 bits; one more would not.
    let largest = FreshPlayerValues {
        walk_speed: MAX_VALUE,
        weight_capacity: MAX_WEIGHT_CAPACITY,
        ..defaults
    };
    largest.validate().unwrap();
    fresh_player_profile(&mut DataPak::open(&path).unwrap(), &largest).unwrap();
    assert!(
        MAX_WEIGHT_CAPACITY
            .checked_mul(OVERBURDENED_WEIGHT_PER_CAPACITY)
            .is_some()
    );
    assert!(
        (MAX_WEIGHT_CAPACITY + 1)
            .checked_mul(OVERBURDENED_WEIGHT_PER_CAPACITY)
            .is_none()
    );
    // A catalog that puts a speed at a stat the client rescales vitals by
    // (0 or 3, F-1166) is refused rather than replicated there.
    let mut names = DataPak::open(&path).unwrap().table_rows("D_Stats").unwrap();
    names.swap(0, 58);
    let swapped = root.path().join("swapped.pak");
    fs::write(
        &swapped,
        pak_writer::fresh_join_pak_with(&[(
            "Stats/D_Stats.json",
            table_json(&names.iter().map(String::as_str).collect::<Vec<_>>()),
        )]),
    )
    .unwrap();
    let error = fresh_player_profile(&mut DataPak::open(&swapped).unwrap(), &defaults)
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("MovementSpeed_+ at stat 0"),
        "{error:#}"
    );
    // The plain player fixture's D_Stats names no sprint speed base.
    let plain = root.path().join("plain.pak");
    fs::write(&plain, pak_writer::official_pak(&[])).unwrap();
    let error = fresh_player_profile(&mut DataPak::open(&plain).unwrap(), &defaults)
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("no BaseSprintSpeed_+ row"),
        "{error:#}"
    );
    // A pak without the stats table.
    let bare = root.path().join("bare.pak");
    fs::write(
        &bare,
        pak_writer::write_pak(&[("Flags/D_SessionFlags.json", &table_json(&["Flag_A"]), true)]),
    )
    .unwrap();
    assert!(fresh_player_profile(&mut DataPak::open(&bare).unwrap(), &defaults).is_err());
    // A skeleton lists names only, in catalog order; an absent table fails.
    let skeleton = catalog_skeleton(&mut DataPak::open(&path).unwrap(), &["D_Inventory"]).unwrap();
    assert_eq!(
        skeleton,
        serde_json::json!({"schema_version": 1, "tables": {"D_Inventory": [
            {"Name": "Crate"}, {"Name": "Player"}
        ]}})
    );
    assert!(catalog_skeleton(&mut DataPak::open(&path).unwrap(), &["D_Absent"]).is_err());
}
