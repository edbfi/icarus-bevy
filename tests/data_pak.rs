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
        CharacterVitals, EmptySlot, FastArrayContent, PawnInventory,
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
    let slot = |query_row, index| EmptySlot { query_row, index };
    assert_eq!(
        content.inventories,
        [
            PawnInventory {
                info_row: 2,
                slots: FastArrayContent {
                    replication_key: 2,
                    items: Vec::new(),
                },
            },
            PawnInventory {
                info_row: 1,
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
