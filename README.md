# ICARUS Bevy server research

An independent native Linux dedicated-server project in Rust and Bevy.
**This currently runs a headless foundation, not an ICARUS-compatible server.**
Networking, authentication, gameplay, original-world compatibility and official-save
import remain unimplemented and unvalidated.

The [full plan](docs/linux-dedicated-server-plan.md) defines the goal and gates.
See [implementation status](docs/implementation-status.md), the
[compatibility matrix](docs/compatibility-matrix.md), and [bug ledger](docs/bug-ledger.md).

## Run the foundation

Requires native Linux and Rust 1.99.0 (pinned with Rustfmt and Clippy). Bevy 0.19.1
uses headless features; no GPU, display, original game files or Proton are required.

```bash
cargo build --release --locked
cargo run --locked --bin icarus-server -- check-config --config config/server.example.toml
cargo run --release --locked --bin icarus-server -- run --config config/server.example.toml
```

Use SIGINT or SIGTERM to stop and save. For a bounded smoke run:

```bash
target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

The development default is 30 Hz with at most four catch-up ticks per update.
This is provisional, not a discovered ICARUS tick rate. No game/query sockets open.
The process emits JSON lifecycle/status/save records to stderr and always reports
`gameplay_ready: false`. Tick timing currently measures only foundation systems,
not representative gameplay performance.

Configuration precedence is CLI → TOML → defaults. Unknown fields and invalid
limits are rejected. Relative paths resolve from the working directory.
`check-config` prints effective configuration without creating the data directory.
Use `--help` on each subcommand for supported overrides.

## Native persistence

Default state location: ignored `work/runtime/`. Each directory has one writer,
enforced by an advisory lock. Autosave runs every 60 seconds through a bounded
worker. Saves contain authored domain records and a simulation clock, not ICARUS
gameplay or its save format.

Each generation is checksummed and synchronized before the manifest is atomically
published. Keep two committed generations. Startup reports when a corrupt current
generation requires recovery from the previous one. Corrupt/missing manifests with
existing snapshots and unsupported schemas fail closed, rather than silently
creating a fresh world. Preserve a copy before manual recovery.

SIGINT/SIGTERM freezes simulation, waits for outstanding work, and commits the final
snapshot within a configured deadline. Failure exits nonzero. Durability relies on
the filesystem honoring synchronization and atomic rename; qualification so far is
for local Linux storage, not network filesystems or physical power-loss testing.

`import-save --source … --destination … --dry-run` is a reserved interface: it
returns an explicit unsupported error without reading/writing the supplied paths.
Do not use it to migrate a real world yet.

## Development and reference research

```bash
prek install
prek run --all-files --hook-stage manual
```

Rust checks use `--locked`; commit the lockfile. Use feature branches and
Conventional Commits. CI runs headless tests with authored fixtures, without
original server binaries. Read `.agents/rules/project.md` before changes.

Original files stay under ignored `icarus-orig/`; generated analysis, private saves,
captures and dumps stay under ignored `work/`. Reusable tools and sanitized reports
are tracked. No original assets are extracted or reused.

Read [research authorization](docs/research-authorization.md) before running
research scripts. Package-script approval and target scope are separate gates.
The inventory/static reports are not evidence of a working protocol or a reproduced
server bug. Original Windows/Proton runs remain separately authorized lab work.

Project source uses the repository's [AGPL-3.0 license](LICENSE); third-party
dependencies and vendored tools retain their own licenses.
