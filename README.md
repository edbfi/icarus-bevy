# ICARUS Bevy server research

An independent native Linux dedicated-server project in Rust and Bevy.
**This currently runs a headless foundation, not an ICARUS-compatible server.**
Networking, authentication, gameplay, original-world compatibility and official-save
import remain unimplemented and unvalidated.

The [canonical plan](docs/linux-dedicated-server-plan.md) defines the goal and gates.
See the [current checkpoint](docs/implementation-status.md), the
[compatibility matrix](docs/compatibility-matrix.md), and [bug ledger](docs/bug-ledger.md).

Research is paused at the [2026-10-13 checkpoint](docs/implementation-status.md#current-paused-checkpoint--2026-10-13-0628-utc).
Documentation maintenance does not resume research or extend authorization windows.

## Run the foundation

Requires native Linux and Rust 1.99.0 (pinned with Rustfmt and Clippy). Bevy 0.20.0
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
Snapshot capture requires every entity with a domain ID to have its required
position; incomplete state fails before submission instead of silently dropping
that entity from the durable generation. Entities without domain identity remain
outside the current native snapshot schema.

Each generation is checksummed and synchronized before the manifest is atomically
published. The store retains two committed generations. Startup reports when a
corrupt current generation requires recovery from the previous one. Corrupt/missing manifests with
existing snapshots and unsupported schemas fail closed, rather than silently
creating a fresh world. Preserve a copy before manual recovery.

SIGINT/SIGTERM freezes simulation, drains outstanding work and saves a final snapshot.
Deadline checks reject overdue completion and final submission; failure exits nonzero.
Synchronous capture, OS writes and scheduler pauses cannot be preempted by these
checks, so this is not a hard process-exit-time guarantee. Durability relies on
the filesystem honoring synchronization and atomic rename; qualification so far is
for local Linux storage, not network filesystems or physical power-loss testing.

`import-save --source … --destination … --dry-run` is a reserved interface: it
returns an explicit unsupported error without reading/writing the supplied paths.
Do not use it to migrate a real world yet.

`inspect-save --source PATH` performs bounded, read-only
[experimental envelope checks](docs/specs/prospect-envelope.md). It reports sizes
and integrity results without extracting the payload or importing a world.
The optional `--inner-tags` also checks [root property framing](docs/specs/tagged-properties.md)
under an explicit experimental profile and reports counts; record values remain opaque.
Use `--recorders` to additionally check recorder field and byte-array boundaries;
it implies root framing and reports aggregates without class names or component data.
Validation covers authored fixtures and supplied-save envelope integrity; the
inner world schema and original-save round trips remain unvalidated.
The [synthetic workload](docs/runbooks/synthetic-workload.md) exercises native scheduling
and durability; it is not an ICARUS performance benchmark.

## Development and reference research

```bash
prek install
prek run --all-files --hook-stage manual
```

Rust checks use `--locked`; commit the lockfile. Use feature branches and
signed-off Conventional Commits (`git commit -s`). Push completed, checked
increments with hooks enabled. CI runs headless tests with authored fixtures, without
original server binaries. Read `.agents/rules/project.md` before changes.

Authored Python helpers target Python 3.15.0 and require `uv` for type checking.
Use the `basedpyright-python-gate` skill for Python work. Run
`python3 tools/typing_gate.py --diff-ignores`, `uvx basedpyright@latest --warnings`
and `python3 -m unittest discover -s tools/tests -v`; prek and CI enforce these
checks too. Recommended mode must finish with zero errors and warnings. The
checked roots are `src`, `tests` and `tools`; `.agents/` and skills are outside
this project-authored-code gate.

Offline research scope fixtures use Node.js and run through prek and CI:

```bash
node --test tools/tests/research-scope.test.cjs
```

Scope validation checks existing authorization and deadlines; it grants neither.

### Research helpers

See the [research helpers runbook](docs/runbooks/research-helpers.md).

### Reference inputs and authorization

Original files stay under ignored `icarus-orig/`; generated analysis, private saves,
captures and dumps stay under ignored `work/`. Reusable tools and sanitized reports
are tracked. No original assets are extracted or reused.

Preserved user-supplied saves live under ignored `reference-data/`, with private
checksums and inventories. Use copies under `work/` for experiments. The supplied
heavy-world archive and its role in save research are covered by the
[continuation plan](docs/linux-dedicated-server-plan.md#supplied-heavy-world-save-and-continuation-order).

Historical approvals are indexed in [docs/history](docs/history/authorizations-index.md);
the [launcher runbook](docs/runbooks/original-server-launcher.md) describes current runs. Package-script approval and target scope are separate gates.
The inventory/static reports are not evidence of a working protocol or a reproduced
server bug. Original Windows/Proton runs remain separately authorized lab work.

Project source uses the repository's [AGPL-3.0 license](LICENSE); third-party
dependencies and vendored tools retain their own licenses.
