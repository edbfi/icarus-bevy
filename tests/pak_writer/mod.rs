//! Authored unencrypted pak version-11 writer for tests: stored or one-block
//! Zlib members under one mount point, laid out as `data_pak` reads them.
use std::io::Write;

use flate2::{Compression, write::ZlibEncoder};

fn fstring(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&i32::try_from(text.len() + 1).unwrap().to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    bytes
}

/// A pak holding `members` (`path` under the mount, bytes, Zlib or stored),
/// each Zlib member in one block.
pub fn write_pak(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let members: Vec<(&str, &[u8], usize)> = members
        .iter()
        .map(|(path, data, zlib)| (*path, *data, usize::from(*zlib)))
        .collect();
    write_pak_blocks(&members, 0)
}

/// A pak holding `members` (`path`, bytes, Zlib blocks: 0 stores the
/// member, n splits it into n equal Zlib blocks); `understate` bytes are
/// taken off every compressed member's declared size, to test bounds.
pub fn write_pak_blocks(members: &[(&str, &[u8], usize)], understate: u64) -> Vec<u8> {
    let mut file = Vec::new();
    let mut encoded = Vec::new();
    let mut directories: Vec<(String, Vec<(String, i32)>)> = Vec::new();
    for (path, data, blocks) in members {
        let offset = file.len() as u64;
        let block_count = *blocks;
        let mut compressed_blocks = Vec::new();
        if block_count > 0 {
            let step = data.len().div_ceil(block_count).max(1);
            for chunk in data.chunks(step) {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(chunk).unwrap();
                compressed_blocks.push(encoder.finish().unwrap());
            }
        }
        let (stored, method) = if block_count > 0 {
            (compressed_blocks.concat(), 1_u32)
        } else {
            (data.to_vec(), 0)
        };
        let declared = if method == 0 {
            data.len() as u64
        } else {
            data.len() as u64 - understate
        };
        let header_bytes: u64 = if method == 0 {
            53
        } else {
            53 + 4 + 16 * compressed_blocks.len() as u64
        };
        // Member header.
        file.extend_from_slice(&0_u64.to_le_bytes());
        file.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        file.extend_from_slice(&declared.to_le_bytes());
        file.extend_from_slice(&method.to_le_bytes());
        file.extend_from_slice(&[0; 20]);
        if method != 0 {
            file.extend_from_slice(
                &u32::try_from(compressed_blocks.len())
                    .unwrap()
                    .to_le_bytes(),
            );
            let mut start = header_bytes;
            for block in &compressed_blocks {
                file.extend_from_slice(&start.to_le_bytes());
                start += block.len() as u64;
                file.extend_from_slice(&start.to_le_bytes());
            }
        }
        file.push(0);
        let block_size: u32 = if method == 0 { 0 } else { 64 * 1024 };
        file.extend_from_slice(&block_size.to_le_bytes());
        file.extend_from_slice(&stored);
        // Encoded index entry: sizes and offset as u32 (bits 29..31).
        let location = i32::try_from(encoded.len()).unwrap();
        let blocks = u32::try_from(compressed_blocks.len()).unwrap();
        let size_code: u32 = if method == 0 { 0 } else { 32 };
        let bits = size_code | (blocks << 6) | (method << 23) | (1 << 29) | (1 << 30) | (1 << 31);
        encoded.extend_from_slice(&bits.to_le_bytes());
        encoded.extend_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
        encoded.extend_from_slice(&u32::try_from(declared).unwrap().to_le_bytes());
        if method != 0 {
            encoded.extend_from_slice(&u32::try_from(stored.len()).unwrap().to_le_bytes());
        }
        if blocks > 1 {
            // Per-block compressed sizes, as the engine lists them.
            for block in &compressed_blocks {
                encoded.extend_from_slice(&u32::try_from(block.len()).unwrap().to_le_bytes());
            }
        }
        let (parent, name) = match path.rfind('/') {
            Some(at) => (path[..=at].to_owned(), path[at + 1..].to_owned()),
            None => ("/".to_owned(), (*path).to_owned()),
        };
        match directories.iter_mut().find(|(dir, _)| *dir == parent) {
            Some((_, files)) => files.push((name, location)),
            None => directories.push((parent, vec![(name, location)])),
        }
    }
    let mut directory = Vec::new();
    directory.extend_from_slice(&i32::try_from(directories.len()).unwrap().to_le_bytes());
    for (parent, files) in &directories {
        directory.extend_from_slice(&fstring(parent));
        directory.extend_from_slice(&i32::try_from(files.len()).unwrap().to_le_bytes());
        for (name, location) in files {
            directory.extend_from_slice(&fstring(name));
            directory.extend_from_slice(&location.to_le_bytes());
        }
    }
    let directory_offset = file.len() as u64;
    file.extend_from_slice(&directory);
    let mut index = Vec::new();
    index.extend_from_slice(&fstring("../../../Icarus/Content/Data/"));
    index.extend_from_slice(&i32::try_from(members.len()).unwrap().to_le_bytes());
    index.extend_from_slice(&0_u64.to_le_bytes());
    index.extend_from_slice(&0_u32.to_le_bytes());
    index.extend_from_slice(&1_u32.to_le_bytes());
    index.extend_from_slice(&directory_offset.to_le_bytes());
    index.extend_from_slice(&(directory.len() as u64).to_le_bytes());
    index.extend_from_slice(&[0; 20]);
    index.extend_from_slice(&i32::try_from(encoded.len()).unwrap().to_le_bytes());
    index.extend_from_slice(&encoded);
    index.extend_from_slice(&0_i32.to_le_bytes());
    let index_offset = file.len() as u64;
    file.extend_from_slice(&index);
    // Footer: key GUID, unencrypted index, magic, version, index place,
    // hash, then five 32-byte method names.
    file.extend_from_slice(&[0; 16]);
    file.push(0);
    file.extend_from_slice(&0x5A6F_12E1_u32.to_le_bytes());
    file.extend_from_slice(&11_u32.to_le_bytes());
    file.extend_from_slice(&index_offset.to_le_bytes());
    file.extend_from_slice(&(index.len() as u64).to_le_bytes());
    file.extend_from_slice(&[0; 20]);
    let mut methods = [0_u8; 160];
    methods[..4].copy_from_slice(b"Zlib");
    file.extend_from_slice(&methods);
    file
}

/// A data table JSON with rows named `names`, as the server ships them.
pub fn table_json(names: &[&str]) -> Vec<u8> {
    let rows: Vec<serde_json::Value> = names
        .iter()
        .map(|name| serde_json::json!({"Name": name}))
        .collect();
    serde_json::to_vec_pretty(&serde_json::json!({
        "RowStruct": "/Script/Icarus.SessionFlag",
        "Defaults": {},
        "GenerateEnum": true,
        "Rows": rows,
    }))
    .unwrap()
}

/// Authored tables for a fresh player's pawn content (`network::pawn_content`):
/// `D_Inventory`'s `Player` lists `Bag` (no starting slots) then `Hands`
/// (three slots of query `Any`, slot 2 overridden by `Fists`); the starting
/// stats grant maxima 250, 150, 220, 230 and 240; `Player` is growth row 1.
pub fn player_tables() -> Vec<(&'static str, Vec<u8>)> {
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    vec![
        (
            "Traits/D_Inventory.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Crate", "Inventories": []},
                {"Name": "Player", "Inventories": [
                    {"RowName": "Bag", "DataTableName": "D_InventoryInfo"},
                    {"RowName": "Hands", "DataTableName": "D_InventoryInfo"},
                ]},
            ]})),
        ),
        (
            "Inventory/D_InventoryInfo.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Unused", "StartingSlots": 4},
                {"Name": "Hands", "SlotTemplate": {"RowName": "Any"}, "StartingSlots": 3,
                 "SlotOverrides": [{"Query": {"RowName": "Fists", "DataTableName": "D_TagQueries"}, "Location": 2}]},
                {"Name": "Bag", "SlotTemplate": {"RowName": "Any"}},
            ]})),
        ),
        (
            "Tags/D_TagQueries.json",
            table_json(&["Q0", "Any", "Fists"]),
        ),
        (
            "Stats/D_CharacterStartingStats.json",
            json(
                serde_json::json!({"Rows": [{"Name": "Base_Stats", "StatsGranted": {
                    "(Value=\"BaseMaximumHealth_+\")": 250,
                    "(Value=\"BaseMaximumStamina_+\")": 150,
                    "(Value=\"BaseMaximumOxygen_+\")": 220,
                    "(Value=\"BaseMaximumWater_+\")": 230,
                    "(Value=\"BaseMaximumFood_+\")": 240,
                    "(Value=\"BaseWeightCapacity_+\")": 100,
                }}]}),
            ),
        ),
        (
            "Character/D_CharacterGrowth.json",
            table_json(&["Creature", "Player"]),
        ),
        (
            "Factions/D_FactionMissions.json",
            table_json(&["Mission_Z"]),
        ),
    ]
}

/// A pak with `extra` members beside [`player_tables`] (a member of the
/// same path in `extra` wins).
pub fn official_pak(extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut members: Vec<(&str, Vec<u8>)> = player_tables()
        .into_iter()
        .filter(|(path, _)| extra.iter().all(|(other, _)| other != path))
        .collect();
    members.extend(extra.iter().map(|(path, bytes)| (*path, bytes.clone())));
    let borrowed: Vec<(&str, &[u8], bool)> = members
        .iter()
        .map(|(path, bytes)| (*path, bytes.as_slice(), true))
        .collect();
    write_pak(&borrowed)
}
