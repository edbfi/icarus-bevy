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

/// A pak holding `members` (`path` under the mount, bytes, Zlib or stored).
pub fn write_pak(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut file = Vec::new();
    let mut encoded = Vec::new();
    let mut directories: Vec<(String, Vec<(String, i32)>)> = Vec::new();
    for (path, data, zlib) in members {
        let offset = file.len() as u64;
        let (stored, method) = if *zlib {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(data).unwrap();
            (encoder.finish().unwrap(), 1_u32)
        } else {
            (data.to_vec(), 0)
        };
        let header_bytes: u64 = if method == 0 { 53 } else { 53 + 4 + 16 };
        // Member header.
        file.extend_from_slice(&0_u64.to_le_bytes());
        file.extend_from_slice(&(stored.len() as u64).to_le_bytes());
        file.extend_from_slice(&(data.len() as u64).to_le_bytes());
        file.extend_from_slice(&method.to_le_bytes());
        file.extend_from_slice(&[0; 20]);
        if method != 0 {
            file.extend_from_slice(&1_u32.to_le_bytes());
            file.extend_from_slice(&header_bytes.to_le_bytes());
            file.extend_from_slice(&(header_bytes + stored.len() as u64).to_le_bytes());
        }
        file.push(0);
        let block_size: u32 = if method == 0 { 0 } else { 64 * 1024 };
        file.extend_from_slice(&block_size.to_le_bytes());
        file.extend_from_slice(&stored);
        // Encoded index entry: sizes and offset as u32 (bits 29..31).
        let location = i32::try_from(encoded.len()).unwrap();
        let blocks: u32 = u32::from(method != 0);
        let size_code: u32 = if method == 0 { 0 } else { 32 };
        let bits = size_code | (blocks << 6) | (method << 23) | (1 << 29) | (1 << 30) | (1 << 31);
        encoded.extend_from_slice(&bits.to_le_bytes());
        encoded.extend_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
        encoded.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        if method != 0 {
            encoded.extend_from_slice(&u32::try_from(stored.len()).unwrap().to_le_bytes());
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
