//! native_join_probe's private join password input.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use icarus_bevy::experimental::admission::JoinPassword;

use super::native_join_capture::confine;

/// Largest join password file read, in bytes.
const MAX_JOIN_PASSWORD_FILE_BYTES: u64 = 1_024;

/// The operator's join password from `path`, under the capture path rules
/// (inside checkout `work/`, no traversal, no symlinks) read-only: a
/// regular file of at most 1 KiB that neither group nor others may access,
/// UTF-8 with one trailing newline trimmed, non-empty and valid as
/// `[network] join_password`. Errors never quote the file's text.
pub fn read_join_password(path: &Path, work_root: &Path) -> Result<JoinPassword> {
    use std::{io::Read, os::unix::fs::PermissionsExt};

    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    let what = "join password file";
    let (directory, relative) = confine(path, work_root, what)?;
    // Non-blocking, so a FIFO cannot stall startup; refused below anyway.
    let file = std::fs::File::from(
        openat2(
            &directory,
            &relative,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .context("join password file could not be opened")?,
    );
    let metadata = file.metadata().context("join password file metadata")?;
    ensure!(
        metadata.is_file(),
        "join password file must be a regular file"
    );
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "join password file must not be accessible to group or others (chmod 600)"
    );
    let mut bytes = Vec::new();
    file.take(MAX_JOIN_PASSWORD_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read join password file")?;
    ensure!(
        bytes.len() as u64 <= MAX_JOIN_PASSWORD_FILE_BYTES,
        "join password file exceeds {MAX_JOIN_PASSWORD_FILE_BYTES} bytes"
    );
    let text = String::from_utf8(bytes)
        .ok()
        .context("join password file must be UTF-8")?;
    let text = text.strip_suffix('\n').map_or(text.as_str(), |line| {
        line.strip_suffix('\r').unwrap_or(line)
    });
    ensure!(!text.is_empty(), "join password file is empty");
    JoinPassword::new(text).map_err(|rules| anyhow::anyhow!("join password {rules}"))
}
