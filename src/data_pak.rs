//! Data tables from the official dedicated server's `Icarus/Content/Data/data.pak`.
//!
//! The free, public dedicated server ships its data tables as JSON members of
//! an unencrypted Unreal pak (version 11, F-829); the native server may use
//! that basic data, fetched at install time, but never the game client's
//! (F-828). A replicated row handle's index is the row's position in its
//! table's `Rows` array (F-830). This reader is independently authored from
//! the pak layout the repository's own pak metadata tool reads; it accepts
//! only unencrypted version-11 paks with stored or Zlib members, within fixed
//! bounds, and reports names and counts only in its errors.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

/// Largest accepted pak file.
pub const MAX_PAK_BYTES: u64 = 256 * 1024 * 1024;
/// Largest accepted pak index or directory index.
pub const MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted member, stored or decompressed.
pub const MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
/// Most members a pak may list.
pub const MAX_MEMBERS: usize = 100_000;
/// Most rows a table may hold.
pub const MAX_TABLE_ROWS: usize = 65_536;

const FOOTER_BYTES: u64 = 221;
const PAK_MAGIC: u32 = 0x5A6F_12E1;
const PAK_VERSION: u32 = 11;
/// The serialized member header without compression blocks.
const MEMBER_HEADER_BYTES: u64 = 53;

/// One member's location and encoding, from the encoded index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    offset: u64,
    size: u64,
    uncompressed: u64,
    /// 0 for stored, else a 1-based index into the footer's method names.
    method: u32,
    blocks: u32,
}

/// An opened data pak: its member paths (mount-relative) and entries.
pub struct DataPak {
    file: File,
    file_bytes: u64,
    methods: Vec<String>,
    members: BTreeMap<String, Entry>,
}

impl std::fmt::Debug for DataPak {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataPak")
            .field("members", &self.members.len())
            .finish_non_exhaustive()
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .context("pak index truncated")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    /// An `FString`: positive counts are narrow bytes, negative UTF-16
    /// units, both with a terminator.
    fn string(&mut self) -> Result<String> {
        let count = self.i32()?;
        if count >= 0 {
            let count = usize::try_from(count)?;
            ensure!(count <= 4096, "pak string too long");
            let bytes = self.take(count)?;
            let text = bytes
                .strip_suffix(&[0])
                .context("pak string unterminated")?;
            Ok(std::str::from_utf8(text)
                .context("pak string is not UTF-8")?
                .to_owned())
        } else {
            let units = usize::try_from(count.unsigned_abs())?;
            ensure!(units <= 4096, "pak string too long");
            let bytes = self.take(units * 2)?;
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            let text = units
                .strip_suffix(&[0])
                .context("pak string unterminated")?;
            String::from_utf16(text).context("pak string is not UTF-16")
        }
    }
}

fn read_at(file: &mut File, offset: u64, length: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; usize::try_from(length)?];
    file.read_exact(&mut bytes).context("pak read truncated")?;
    Ok(bytes)
}

/// Decode one encoded index entry at `location`.
fn decode_entry(encoded: &[u8], location: usize) -> Result<Entry> {
    let mut cursor = Cursor {
        bytes: encoded,
        at: location,
    };
    let bits = cursor.u32()?;
    if bits & 63 == 63 {
        cursor.u32()?;
    }
    ensure!(
        (bits >> 22) & 1 == 0,
        "encrypted pak members are unsupported"
    );
    let method = (bits >> 23) & 63;
    let blocks = (bits >> 6) & 0xFFFF;
    let offset = if bits >> 31 == 1 {
        u64::from(cursor.u32()?)
    } else {
        cursor.u64()?
    };
    let uncompressed = if bits & 0x4000_0000 != 0 {
        u64::from(cursor.u32()?)
    } else {
        cursor.u64()?
    };
    let size = if method == 0 {
        uncompressed
    } else if bits & 0x2000_0000 != 0 {
        u64::from(cursor.u32()?)
    } else {
        cursor.u64()?
    };
    ensure!(
        uncompressed <= MAX_MEMBER_BYTES && size <= MAX_MEMBER_BYTES,
        "pak member exceeds the member bound"
    );
    ensure!(method != 0 || blocks == 0, "stored pak member lists blocks");
    Ok(Entry {
        offset,
        size,
        uncompressed,
        method,
        blocks,
    })
}

impl DataPak {
    /// Open a regular pak file and read its index (no member data).
    pub fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .context("open data pak")?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "data pak must be a regular file");
        let file_bytes = metadata.len();
        ensure!(
            (FOOTER_BYTES..=MAX_PAK_BYTES).contains(&file_bytes),
            "data pak size outside 221 B..=256 MiB"
        );
        let footer = read_at(&mut file, file_bytes - FOOTER_BYTES, FOOTER_BYTES)?;
        let mut cursor = Cursor {
            bytes: &footer,
            at: 16,
        };
        ensure!(
            cursor.take(1)?[0] == 0,
            "encrypted pak indexes are unsupported"
        );
        ensure!(cursor.u32()? == PAK_MAGIC, "not a pak file");
        ensure!(cursor.u32()? == PAK_VERSION, "unsupported pak version");
        let index_offset = cursor.u64()?;
        let index_bytes = cursor.u64()?;
        cursor.take(20)?;
        let mut methods = Vec::new();
        for _ in 0..5 {
            let slot = cursor.take(32)?;
            let end = slot.iter().position(|byte| *byte == 0).unwrap_or(32);
            methods.push(String::from_utf8_lossy(&slot[..end]).into_owned());
        }
        ensure!(
            index_bytes <= MAX_INDEX_BYTES
                && index_offset
                    .checked_add(index_bytes)
                    .is_some_and(|end| end <= file_bytes - FOOTER_BYTES),
            "pak index outside the file"
        );
        let index = read_at(&mut file, index_offset, index_bytes)?;
        let mut cursor = Cursor {
            bytes: &index,
            at: 0,
        };
        cursor.string()?;
        let count = usize::try_from(cursor.i32()?).context("negative pak member count")?;
        ensure!(count <= MAX_MEMBERS, "pak lists too many members");
        cursor.u64()?;
        let mut directory = None;
        for kind in ["path hash", "directory"] {
            match cursor.u32()? {
                0 => {}
                1 => {
                    let offset = cursor.u64()?;
                    let bytes = cursor.u64()?;
                    cursor.take(20)?;
                    ensure!(
                        bytes <= MAX_INDEX_BYTES
                            && offset
                                .checked_add(bytes)
                                .is_some_and(|end| end <= file_bytes - FOOTER_BYTES),
                        "pak {kind} index outside the file"
                    );
                    if kind == "directory" {
                        directory = Some((offset, bytes));
                    }
                }
                _ => bail!("invalid pak {kind} index flag"),
            }
        }
        let encoded_bytes = usize::try_from(cursor.i32()?).context("negative pak entry size")?;
        let encoded = cursor.take(encoded_bytes)?.to_vec();
        let (offset, bytes) = directory.context("pak has no directory index")?;
        let directory = read_at(&mut file, offset, bytes)?;
        let mut cursor = Cursor {
            bytes: &directory,
            at: 0,
        };
        let directories = usize::try_from(cursor.i32()?).context("negative directory count")?;
        ensure!(directories <= MAX_MEMBERS, "pak lists too many directories");
        let mut members = BTreeMap::new();
        for _ in 0..directories {
            let parent = cursor.string()?;
            let files = usize::try_from(cursor.i32()?).context("negative file count")?;
            ensure!(
                members.len() + files <= count,
                "pak directory lists more files than the index"
            );
            for _ in 0..files {
                let name = cursor.string()?;
                let location = usize::try_from(cursor.i32()?).context("negative entry location")?;
                let path = if parent == "/" {
                    name
                } else {
                    format!("{parent}{name}")
                };
                let entry = decode_entry(&encoded, location)?;
                ensure!(
                    members.insert(path, entry).is_none(),
                    "pak lists a member twice"
                );
            }
        }
        Ok(Self {
            file,
            file_bytes,
            methods,
            members,
        })
    }

    /// Member paths relative to the pak's mount point.
    pub fn members(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    /// The bytes of the one member whose path ends with `/suffix` (or is
    /// `suffix`).
    pub fn read(&mut self, suffix: &str) -> Result<Vec<u8>> {
        let mut matches = self
            .members
            .iter()
            .filter(|(path, _)| path.as_str() == suffix || path.ends_with(&format!("/{suffix}")));
        let (_, entry) = matches.next().context("data pak member not found")?;
        ensure!(matches.next().is_none(), "data pak member is ambiguous");
        let entry = *entry;
        let header_bytes = if entry.method == 0 {
            MEMBER_HEADER_BYTES
        } else {
            MEMBER_HEADER_BYTES + 4 + 16 * u64::from(entry.blocks)
        };
        let end = entry
            .offset
            .checked_add(header_bytes)
            .and_then(|start| start.checked_add(entry.size))
            .context("pak member outside the file")?;
        ensure!(end <= self.file_bytes, "pak member outside the file");
        let header = read_at(&mut self.file, entry.offset, header_bytes)?;
        let mut cursor = Cursor {
            bytes: &header,
            at: 0,
        };
        cursor.u64()?;
        ensure!(
            cursor.u64()? == entry.size && cursor.u64()? == entry.uncompressed,
            "pak member header disagrees with the index"
        );
        ensure!(
            cursor.u32()? == entry.method,
            "pak member method disagrees with the index"
        );
        cursor.take(20)?;
        let mut blocks = Vec::new();
        if entry.method != 0 {
            ensure!(
                cursor.u32()? == entry.blocks,
                "pak member block count disagrees with the index"
            );
            for _ in 0..entry.blocks {
                blocks.push((cursor.u64()?, cursor.u64()?));
            }
        }
        let data = read_at(&mut self.file, entry.offset + header_bytes, entry.size)?;
        if entry.method == 0 {
            return Ok(data);
        }
        let method = usize::try_from(entry.method - 1)
            .ok()
            .and_then(|index| self.methods.get(index))
            .context("pak member names no compression method")?;
        ensure!(method == "Zlib", "unsupported pak compression method");
        let mut out = Vec::with_capacity(usize::try_from(entry.uncompressed)?);
        for (start, end) in blocks {
            // Block offsets are relative to the member's header (version 11).
            let start = start
                .checked_sub(header_bytes)
                .and_then(|start| usize::try_from(start).ok())
                .context("pak block before the member data")?;
            let end = end
                .checked_sub(header_bytes)
                .and_then(|end| usize::try_from(end).ok())
                .filter(|end| *end >= start && *end <= data.len())
                .context("pak block outside the member data")?;
            let mut decoder = flate2::read::ZlibDecoder::new(&data[start..end]);
            let limit = entry.uncompressed + 1 - out.len() as u64;
            (&mut decoder).take(limit).read_to_end(&mut out)?;
            ensure!(
                out.len() as u64 <= entry.uncompressed,
                "pak member inflates past its size"
            );
        }
        ensure!(
            out.len() as u64 == entry.uncompressed,
            "pak member inflates short of its size"
        );
        Ok(out)
    }

    /// The row names of data table `table` (for example `D_SessionFlags`),
    /// in `Rows` order: a row handle's index is its position (F-830).
    pub fn table_rows(&mut self, table: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Table {
            #[serde(rename = "Rows")]
            rows: Vec<Row>,
        }
        #[derive(Deserialize)]
        struct Row {
            #[serde(rename = "Name")]
            name: String,
        }
        let bytes = self.read(&format!("{table}.json"))?;
        let parsed: Table = serde_json::from_slice(&bytes)
            .map_err(|error| crate::json_parse_error("invalid data table", error))?;
        ensure!(
            parsed.rows.len() <= MAX_TABLE_ROWS,
            "data table has too many rows"
        );
        Ok(parsed.rows.into_iter().map(|row| row.name).collect())
    }

    /// The bool `field` of every row of data table `table`, in `Rows`
    /// order; `None` where a row omits it, so its struct default applies.
    pub fn table_bool_column(&mut self, table: &str, field: &str) -> Result<Vec<Option<bool>>> {
        #[derive(Deserialize)]
        struct Table {
            #[serde(rename = "Rows")]
            rows: Vec<serde_json::Map<String, serde_json::Value>>,
        }
        let bytes = self.read(&format!("{table}.json"))?;
        let parsed: Table = serde_json::from_slice(&bytes)
            .map_err(|error| crate::json_parse_error("invalid data table", error))?;
        ensure!(
            parsed.rows.len() <= MAX_TABLE_ROWS,
            "data table has too many rows"
        );
        parsed
            .rows
            .iter()
            .map(|row| match row.get(field) {
                None => Ok(None),
                Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
                Some(_) => bail!("data table {table} has a non-bool {field}"),
            })
            .collect()
    }
}

/// Row names by table, loaded on demand from one [`DataPak`].
pub struct DataTables {
    pak: DataPak,
    rows: BTreeMap<String, Vec<String>>,
    bools: BTreeMap<(String, String), Vec<Option<bool>>>,
}

impl std::fmt::Debug for DataTables {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataTables")
            .field("loaded", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl DataTables {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            pak: DataPak::open(path)?,
            rows: BTreeMap::new(),
            bools: BTreeMap::new(),
        })
    }

    pub fn rows(&mut self, table: &str) -> Result<&[String]> {
        if !self.rows.contains_key(table) {
            let rows = self.pak.table_rows(table)?;
            self.rows.insert(table.to_owned(), rows);
        }
        Ok(&self.rows[table])
    }

    /// The row index of `name` (UTF-16) in `table`, if listed.
    pub fn row_index(&mut self, table: &str, name: &[u16]) -> Result<Option<usize>> {
        let name = String::from_utf16_lossy(name);
        Ok(self.rows(table)?.iter().position(|row| *row == name))
    }

    /// The bool `field` of row `index` in `table`: `None` where the row
    /// omits it or the table has no such row.
    pub fn row_bool(&mut self, table: &str, field: &str, index: usize) -> Result<Option<bool>> {
        let key = (table.to_owned(), field.to_owned());
        if !self.bools.contains_key(&key) {
            let column = self.pak.table_bool_column(table, field)?;
            self.bools.insert(key.clone(), column);
        }
        Ok(self.bools[&key].get(index).copied().flatten())
    }
}
