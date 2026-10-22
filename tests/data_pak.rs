//! The official server's `data.pak` reader against authored paks.
#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::fs;

use icarus_bevy::data_pak::{DataPak, DataTables};
use pak_writer::{table_json, write_pak};
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
