//! Adjacent module for the existing capture_decode example.
//! Errors deliberately discard clap/input/path details. No network/dispatch.
use std::{
    ffi::OsString,
    fs::File,
    path::{Component, Path, PathBuf},
};

use clap::{Parser, ValueEnum};
use icarus_bevy::experimental::{
    release::ReleaseProfile,
    selected_rpc_capture::MAX_OUTPUT_BYTES,
    selected_visibility_replay::{ReplayProfile, ReplaySummary, replay},
    visibility::{VisibilityArchiveProfile, VisibilityRpc82Layout},
};

#[derive(Parser)]
#[command(about = "Conditional offline selected RPC82 replay; provenance unverified")]
struct Options {
    input: PathBuf,
    detail: PathBuf,
    #[arg(long)]
    release_profile: PathBuf,
    #[arg(long, value_enum)]
    archive: Archive,
    #[arg(long, value_enum)]
    layout: Layout,
}

#[derive(Clone, Copy, ValueEnum)]
enum Archive {
    NormalLittleEndianNullMapCustom,
    NormalLittleEndianPackageMapCustom,
}

#[derive(Clone, Copy, ValueEnum)]
enum Layout {
    NormalSingleCustomStruct,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    Usage,
    Profile,
    Path,
    Input,
    Output,
    Capture,
}

/// The caller's early-dispatch branch must return this status before legacy
/// capture_decode parsing/anyhow error formatting. All stderr codes are fixed.
pub fn dispatch(args: &[OsString], work_root: &Path) -> Option<i32> {
    if args.get(1).map(OsString::as_os_str) != Some(std::ffi::OsStr::new("selected-visibility")) {
        return None;
    }
    Some(match run_cli_in(args.iter().skip(1).cloned(), work_root) {
        Ok(summary) => {
            if summary.write_json(std::io::stdout().lock()).is_err() {
                eprintln!("selected_visibility:Output");
                1
            } else if summary.records == 0 || !summary.all_decoded() {
                2
            } else {
                0
            }
        }
        Err(code) => {
            eprintln!("selected_visibility:{code:?}");
            1
        }
    })
}

pub fn run_cli_in<I, T>(args: I, work_root: &Path) -> Result<ReplaySummary, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    // No clap .exit()/error display: invalid values can contain private paths.
    let options = Options::try_parse_from(args).map_err(|_| CliError::Usage)?;
    let release = ReleaseProfile::load(&options.release_profile).map_err(|_| CliError::Profile)?;
    let archive = match options.archive {
        Archive::NormalLittleEndianNullMapCustom => {
            VisibilityArchiveProfile::NormalLittleEndianNullMapCustom
        }
        Archive::NormalLittleEndianPackageMapCustom => {
            VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom
        }
    };
    let layout = match options.layout {
        Layout::NormalSingleCustomStruct => VisibilityRpc82Layout::NormalSingleCustomStruct,
    };
    let profile =
        ReplayProfile::qualify(&release, archive, layout).map_err(|_| CliError::Profile)?;
    let (input, mut output) = open_files(&options.input, &options.detail, work_root, || {})?;
    let summary = replay(input, &profile).map_err(|_| CliError::Capture)?;
    summary
        .write_json(&mut output)
        .map_err(|_| CliError::Output)?;
    output.sync_all().map_err(|_| CliError::Output)?;
    Ok(summary)
}

fn relative(path: &Path, root: &Path) -> Result<PathBuf, CliError> {
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(CliError::Path);
    }
    let absolute = std::path::absolute(path).map_err(|_| CliError::Path)?;
    let relative = absolute.strip_prefix(root).map_err(|_| CliError::Path)?;
    if relative.as_os_str().is_empty() || relative.file_name().is_none() {
        return Err(CliError::Path);
    }
    Ok(relative.to_path_buf())
}

fn open_files(
    input: &Path,
    output: &Path,
    work_root: &Path,
    before_create: impl FnOnce(),
) -> Result<(File, File), CliError> {
    use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
    if work_root
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(CliError::Path);
    }
    let root = std::path::absolute(work_root).map_err(|_| CliError::Path)?;
    let input_path = relative(input, &root)?;
    let output_path = relative(output, &root)?;
    let directory = openat2(
        CWD,
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|_| CliError::Path)?;
    let input = File::from(
        openat2(
            &directory,
            &input_path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| CliError::Input)?,
    );
    let metadata = input.metadata().map_err(|_| CliError::Input)?;
    if !metadata.is_file() || metadata.len() > MAX_OUTPUT_BYTES as u64 {
        return Err(CliError::Input);
    }
    before_create();
    // Existing files, input aliases and hardlinks all fail EXCL. Symlinks in
    // any ancestor fail inside the same kernel resolution operation.
    let output = File::from(
        openat2(
            &directory,
            &output_path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| CliError::Output)?,
    );
    Ok((input, output))
}

#[cfg(test)]
#[path = "selected_visibility_cli_tests.rs"]
mod tests;
