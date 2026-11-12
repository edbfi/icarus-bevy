//! File preflight for native_join_probe's explicitly opted-in private sink.
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use icarus_bevy::experimental::selected_rpc_capture::{
    PrivateRpc82Capture,
    pawn_bunches::{PawnBunchCaptureCounts, PrivatePawnBunchCapture},
};

pub fn add_counts(
    summary: &mut serde_json::Value,
    counts: Option<icarus_bevy::experimental::selected_rpc_capture::CaptureCounts>,
    headers: bool,
) {
    if let Some(counts) = counts {
        summary[if headers {
            "private_controller_headers_capture"
        } else {
            "private_rpc82_capture"
        }] = serde_json::json!(counts);
    }
}

/// Counts only, under `private_pawn_bunches_capture`.
pub fn add_pawn_counts(summary: &mut serde_json::Value, counts: Option<PawnBunchCaptureCounts>) {
    if let Some(counts) = counts {
        summary["private_pawn_bunches_capture"] = serde_json::json!(counts);
    }
}

pub fn prepare(output: Option<&Path>, work_root: &Path) -> Result<Option<PrivateRpc82Capture>> {
    prepare_impl(output, work_root, false, || {})
}

/// The pawn-channel bunch sink, under the same path rules and file mode.
pub fn prepare_pawn_bunches(
    output: Option<&Path>,
    work_root: &Path,
) -> Result<Option<PrivatePawnBunchCapture>> {
    Ok(open_private(output, work_root, || {})?.map(PrivatePawnBunchCapture::from_file))
}

pub fn prepare_headers(
    output: Option<&Path>,
    work_root: &Path,
) -> Result<Option<PrivateRpc82Capture>> {
    prepare_impl(output, work_root, true, || {})
}

fn prepare_impl(
    output: Option<&Path>,
    work_root: &Path,
    headers: bool,
    before_create: impl FnOnce(),
) -> Result<Option<PrivateRpc82Capture>> {
    Ok(open_private(output, work_root, before_create)?.map(|file| {
        if headers {
            PrivateRpc82Capture::from_header_file(file)
        } else {
            PrivateRpc82Capture::from_file(file)
        }
    }))
}

/// The bound `work_root` directory and `path` relative to it, refusing
/// traversal, paths outside it and symlinks in the root's ancestors.
pub fn confine(
    path: &Path,
    work_root: &Path,
    what: &str,
) -> Result<(rustix::fd::OwnedFd, std::path::PathBuf)> {
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "{what} parent traversal refused"
    );
    path.file_name()
        .with_context(|| format!("{what} requires a filename"))?;
    let path = std::path::absolute(path).with_context(|| format!("{what} path"))?;
    let root = std::path::absolute(work_root).with_context(|| format!("{what} work root"))?;
    let relative = path
        .strip_prefix(&root)
        .with_context(|| format!("{what} must remain inside checkout work directory"))?;
    ensure!(
        !relative.as_os_str().is_empty(),
        "{what} requires a filename"
    );
    use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
    // Bind the trusted root once, refusing symlinks in every ancestor. No
    // pathname validation followed by a separate pathname open is sufficient.
    let directory = openat2(
        CWD,
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .with_context(|| format!("{what} work root must exist without symlinks"))?;
    Ok((directory, relative.to_path_buf()))
}

/// Exclusive mode-0600 creation confined beneath `work_root`.
fn open_private(
    output: Option<&Path>,
    work_root: &Path,
    before_create: impl FnOnce(),
) -> Result<Option<std::fs::File>> {
    let Some(output) = output else {
        return Ok(None);
    };
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    let (directory, relative) = confine(output, work_root, "capture")?;
    before_create();
    // Resolution and exclusive creation happen in one kernel operation under
    // the bound root. Ancestor replacement cannot redirect creation outside it.
    // Unsupported openat2 kernels fail closed; there is no pathname fallback.
    let file = openat2(
        &directory,
        &relative,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .context("exclusive private capture creation failed")?;
    Ok(Some(std::fs::File::from(file)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_file_is_private_exclusive_and_public_summary_contains_counts_only() -> Result<()> {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = tempfile::tempdir()?;
        let work = root.path().join("work");
        fs::create_dir(&work)?;
        let path = work.join("headers.jsonl");
        let mut capture = prepare_headers(Some(&path), &work)?.unwrap();
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert!(prepare_headers(Some(&path), &work).is_err());
        let mut summary = serde_json::json!({});
        add_counts(&mut summary, Some(capture.finish()), true);
        assert!(summary.get("private_controller_headers_capture").is_some());
        assert!(summary.get("private_rpc82_capture").is_none());
        assert!(!summary.to_string().contains("guid"));
        assert!(fs::read(&path)?.is_empty());
        Ok(())
    }

    #[test]
    fn pawn_bunch_file_obeys_the_same_path_rules_and_mode() -> Result<()> {
        use std::{fs, os::unix::fs::PermissionsExt, os::unix::fs::symlink};
        let root = tempfile::tempdir()?;
        let work = root.path().join("work");
        let case = work.join("case");
        fs::create_dir_all(&case)?;
        // Omitted: nothing is created or installed.
        assert!(prepare_pawn_bunches(None, &work)?.is_none());
        let path = case.join("pawn-bunches.jsonl");
        let mut capture = prepare_pawn_bunches(Some(&path), &work)?.unwrap();
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        // Exclusive: an existing file is never reused or truncated.
        assert!(prepare_pawn_bunches(Some(&path), &work).is_err());
        // Outside work/, traversal, a missing parent, a symlinked parent and
        // the work root itself are refused.
        let outside = root.path().join("outside.jsonl");
        assert!(prepare_pawn_bunches(Some(&outside), &work).is_err());
        assert!(!outside.exists());
        let traversal = case.join("..").join("..").join("escape.jsonl");
        assert!(prepare_pawn_bunches(Some(&traversal), &work).is_err());
        assert!(prepare_pawn_bunches(Some(&work.join("missing/p.jsonl")), &work).is_err());
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&elsewhere)?;
        symlink(&elsewhere, work.join("linked"))?;
        assert!(prepare_pawn_bunches(Some(&work.join("linked/p.jsonl")), &work).is_err());
        assert!(!elsewhere.join("p.jsonl").exists());
        assert!(prepare_pawn_bunches(Some(&work), &work).is_err());
        let mut summary = serde_json::json!({});
        add_pawn_counts(&mut summary, None);
        assert!(summary.get("private_pawn_bunches_capture").is_none());
        add_pawn_counts(&mut summary, Some(capture.finish()));
        assert_eq!(summary["private_pawn_bunches_capture"]["observations"], 0);
        assert!(!summary.to_string().contains("payload_hex"));
        assert!(fs::read(&path)?.is_empty());
        Ok(())
    }

    #[test]
    fn private_capture_refuses_parent_replaced_after_root_binding() -> Result<()> {
        use std::{fs, os::unix::fs::symlink};
        let root = tempfile::tempdir()?;
        let work = root.path().join("work");
        let parent = work.join("case");
        let moved = work.join("original-case");
        let outside = root.path().join("outside");
        fs::create_dir_all(&parent)?;
        fs::create_dir(&outside)?;
        let output = parent.join("selected.jsonl");
        let result = prepare_impl(Some(&output), &work, false, || {
            fs::rename(&parent, &moved).unwrap();
            symlink(&outside, &parent).unwrap();
        });
        assert!(result.is_err());
        assert!(!outside.join("selected.jsonl").exists());
        assert!(!moved.join("selected.jsonl").exists());
        Ok(())
    }
}
