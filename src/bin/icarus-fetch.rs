//! `icarus-fetch`: download one file from a public Steam app anonymously.
//!
//! Prints progress on stderr and JSON on stdout: with `--list`, one line per
//! manifest file of the selected depot and then the summary; otherwise only
//! the summary of the verified, atomically written file.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use icarus_bevy::steam_fetch::{FetchRequest, resolve};
use serde_json::json;

#[derive(Parser)]
#[command(
    version,
    about = "Anonymously fetch one file from a public Steam app's depot (no steamcmd)"
)]
struct Cli {
    /// Steam app id, for example 2089300 (ICARUS Dedicated Server).
    #[arg(long)]
    app: u32,
    /// Branch whose manifests are used.
    #[arg(long, default_value = "public")]
    branch: String,
    /// Depot-relative path of the wanted file.
    #[arg(long)]
    file: String,
    /// Output directory; the file keeps its depot-relative path below it.
    #[arg(long, required_unless_present = "list")]
    out: Option<PathBuf>,
    /// Print the selected depot's file list and download nothing.
    #[arg(long)]
    list: bool,
    /// Only consider this depot.
    #[arg(long)]
    depot: Option<u32>,
    /// Parallel chunk downloads.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=32))]
    workers: u16,
    /// Refuse files larger than this many bytes.
    #[arg(long, default_value_t = 32 * 1024 * 1024 * 1024)]
    max_file_bytes: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut request = FetchRequest::new(cli.app, &cli.branch, &cli.file);
    request.depot = cli.depot;
    request.workers = usize::from(cli.workers);
    request.assembly.max_file_bytes = cli.max_file_bytes;
    let progress = |message: &str| eprintln!("icarus-fetch: {message}");

    let resolved = resolve(&request, &progress)?;
    let file = resolved.file(&request)?;
    if cli.list {
        for entry in &resolved.manifest.files {
            println!(
                "{}",
                json!({
                    "path": entry.path,
                    "size": entry.size,
                    "directory": entry.is_directory(),
                })
            );
        }
        println!(
            "{}",
            json!({
                "mode": "list",
                "app": cli.app,
                "branch": cli.branch,
                "build_id": resolved.build_id,
                "depot": resolved.depot_id,
                "manifest_gid": resolved.manifest_gid.to_string(),
                "manifest_files": resolved.manifest.files.len(),
                "filenames_encrypted": resolved.manifest.filenames_encrypted,
                "file": file.path,
                "size": file.size,
                "chunks": file.chunks.len(),
                "sha1": icarus_bevy::steam_fetch::hex(&file.sha_content),
                "sha256": null,
            })
        );
        return Ok(());
    }

    let out = cli.out.context("--out is required without --list")?;
    let written = resolved.download(&request, &out)?;
    println!(
        "{}",
        json!({
            "mode": "download",
            "app": cli.app,
            "branch": cli.branch,
            "build_id": resolved.build_id,
            "depot": resolved.depot_id,
            "manifest_gid": resolved.manifest_gid.to_string(),
            "file": file.path,
            "path": written.path,
            "size": written.size,
            "chunks": file.chunks.len(),
            "sha1": written.sha1,
            "sha256": written.sha256,
        })
    );
    Ok(())
}
