//! File preflight for native_join_probe's explicitly opted-in private sink.
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use icarus_bevy::experimental::selected_rpc_capture::PrivateRpc82Capture;

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

pub fn prepare(output: Option<&Path>, work_root: &Path) -> Result<Option<PrivateRpc82Capture>> {
    prepare_impl(output, work_root, false, || {})
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
    let Some(output) = output else {
        return Ok(None);
    };
    ensure!(
        !output
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "capture parent traversal refused"
    );
    output.file_name().context("capture requires a filename")?;
    let output = std::path::absolute(output).context("capture output path")?;
    let root = std::path::absolute(work_root).context("capture work root")?;
    let relative = output
        .strip_prefix(&root)
        .context("capture must remain inside checkout work directory")?;
    ensure!(
        !relative.as_os_str().is_empty(),
        "capture requires a filename"
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
    .context("capture work root must exist without symlinks")?;
    before_create();
    // Resolution and exclusive creation happen in one kernel operation under
    // the bound root. Ancestor replacement cannot redirect creation outside it.
    // Unsupported openat2 kernels fail closed; there is no pathname fallback.
    let file = openat2(
        &directory,
        relative,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .context("exclusive private capture creation failed")?;
    let file = std::fs::File::from(file);
    Ok(Some(if headers {
        PrivateRpc82Capture::from_header_file(file)
    } else {
        PrivateRpc82Capture::from_file(file)
    }))
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
