//! File assembly from verified chunks and the atomic write.
//!
//! Chunks must tile the file exactly: sorted by offset they start at 0, abut
//! and end at the file size. Workers write each verified chunk at its offset
//! into a temporary file in the destination directory; the file is then
//! fsynced, re-read for SHA-1 (checked against the manifest) and SHA-256,
//! renamed into place and the directory fsynced.

use std::{
    fs,
    io::{BufReader, Read, Seek},
    os::unix::fs::{FileExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};
use sha2::Sha256;

use super::{
    chunk::{ChunkSpec, verify_chunk},
    manifest::ManifestFile,
};

/// Bounds on one assembled file.
#[derive(Debug, Clone, Copy)]
pub struct AssemblyLimits {
    pub max_file_bytes: u64,
    pub max_chunk_bytes: u32,
    pub max_chunks: usize,
}

impl Default for AssemblyLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            max_chunk_bytes: 64 * 1024 * 1024,
            max_chunks: 4_000_000,
        }
    }
}

/// The written file and its digests (lowercase hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha1: String,
    pub sha256: String,
}

/// Map a manifest path to a relative path with only normal components.
pub fn safe_relative_path(manifest_path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in manifest_path.split(['/', '\\']) {
        if part.is_empty() {
            continue;
        }
        let mut components = Path::new(part).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(normal)), None) if !part.contains('\0') => out.push(normal),
            _ => bail!("unsafe manifest path component {part:?}"),
        }
    }
    ensure!(out.components().next().is_some(), "empty manifest path");
    Ok(out)
}

/// Sort a file's chunks and check that they tile it exactly.
pub fn plan_chunks(file: &ManifestFile, limits: &AssemblyLimits) -> Result<Vec<ChunkSpec>> {
    ensure!(!file.is_directory(), "{} is a directory", file.path);
    ensure!(
        file.size <= limits.max_file_bytes,
        "{} is {} bytes, above the {} byte limit",
        file.path,
        file.size,
        limits.max_file_bytes
    );
    ensure!(
        file.chunks.len() <= limits.max_chunks,
        "{} has {} chunks, above {}",
        file.path,
        file.chunks.len(),
        limits.max_chunks
    );
    let mut chunks = file.chunks.clone();
    chunks.sort_by_key(|chunk| chunk.offset);
    let mut end = 0u64;
    for chunk in &chunks {
        ensure!(
            chunk.original_len > 0 && chunk.original_len <= limits.max_chunk_bytes,
            "chunk {} size {} is outside 1..={}",
            chunk.id_hex(),
            chunk.original_len,
            limits.max_chunk_bytes
        );
        ensure!(
            chunk.offset == end,
            "chunk {} at offset {} leaves a gap or overlap at {end}",
            chunk.id_hex(),
            chunk.offset
        );
        end = end
            .checked_add(u64::from(chunk.original_len))
            .context("chunk offsets overflow")?;
    }
    ensure!(
        end == file.size,
        "{} chunks cover {end} bytes, file is {}",
        file.path,
        file.size
    );
    Ok(chunks)
}

/// Fetch, verify and write `file` under `out_dir`, then rename it into place.
/// `fetch` returns a chunk's uncompressed bytes; each is checked against its
/// size, SHA-1 id and checksum before it is written.
pub fn assemble<F>(
    out_dir: &Path,
    file: &ManifestFile,
    limits: &AssemblyLimits,
    workers: usize,
    fetch: F,
) -> Result<AssembledFile>
where
    F: Fn(&ChunkSpec) -> Result<Vec<u8>> + Sync,
{
    let chunks = plan_chunks(file, limits)?;
    let relative = safe_relative_path(&file.path)?;
    let final_path = out_dir.join(&relative);
    let parent = final_path
        .parent()
        .context("output path has no parent")?
        .to_path_buf();
    fs::create_dir_all(&parent).with_context(|| format!("create {}", parent.display()))?;
    let temp = tempfile::Builder::new()
        .prefix(".icarus-fetch-")
        .suffix(".part")
        .tempfile_in(&parent)
        .with_context(|| format!("create temporary file in {}", parent.display()))?;
    temp.as_file()
        .set_len(file.size)
        .context("size temporary file")?;

    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let first_error: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let workers = workers.clamp(1, 32).min(chunks.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !failed.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(chunk) = chunks.get(index) else {
                        break;
                    };
                    let result = fetch(chunk).and_then(|data| {
                        verify_chunk(&data, chunk)?;
                        temp.as_file()
                            .write_all_at(&data, chunk.offset)
                            .context("write chunk to temporary file")
                    });
                    if let Err(error) = result {
                        failed.store(true, Ordering::Relaxed);
                        let mut slot = first_error.lock().unwrap_or_else(|p| p.into_inner());
                        slot.get_or_insert(error);
                        break;
                    }
                }
            });
        }
    });
    if let Some(error) = first_error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        return Err(error.context(format!("assemble {}", file.path)));
    }

    let handle = temp.as_file();
    handle.sync_all().context("fsync temporary file")?;
    let mut reader = BufReader::with_capacity(1 << 20, handle);
    reader.rewind().context("rewind temporary file")?;
    let (mut sha1, mut sha256) = (Sha1::new(), Sha256::new());
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buf).context("re-read temporary file")?;
        if read == 0 {
            break;
        }
        sha1.update(&buf[..read]);
        sha256.update(&buf[..read]);
        total += read as u64;
    }
    ensure!(
        total == file.size,
        "wrote {total} bytes, expected {}",
        file.size
    );
    let sha1 = sha1.finalize();
    if file.sha_content.len() == 20 && file.sha_content.iter().any(|&byte| byte != 0) {
        ensure!(
            sha1.as_slice() == file.sha_content.as_slice(),
            "{} SHA-1 does not match the manifest",
            file.path
        );
    }
    handle
        .set_permissions(fs::Permissions::from_mode(0o644))
        .context("set file mode")?;
    temp.persist(&final_path)
        .map_err(|error| error.error)
        .with_context(|| format!("rename into {}", final_path.display()))?;
    fs::File::open(&parent)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("fsync {}", parent.display()))?;
    Ok(AssembledFile {
        path: final_path,
        size: total,
        sha1: crate::hex_lower(&sha1),
        sha256: crate::hex_lower(&sha256.finalize()),
    })
}
