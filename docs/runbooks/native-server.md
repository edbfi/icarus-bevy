# Native server: run, persistence and inspection

Operating guide for the headless foundation. Design constraints are in
[architecture](../architecture.md); what is proven is in
[implementation status](../implementation-status.md).

## Run

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
Config inputs must resolve to an opened regular file of at most64KiB; regular
symlinks are supported. FIFO and oversized inputs fail before runtime state opens
(F-443).
`check-config` prints effective configuration without creating the data directory.
Use `--help` on each subcommand for supported overrides.

## Persistence and recovery

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
existing snapshots, noncanonical checksum fields and unsupported schemas fail closed, rather than silently
creating a fresh world. Each manifest digest must contain exactly 64 lowercase
ASCII hexadecimal digits (F-417). Preserve a copy before manual recovery.
Storage roots, generation references and entity records require named JSON
objects; position triples remain arrays. Malformed array headers can recover a
valid previous generation, while proper future-schema objects refuse rollback
(F-428).
Current-generation I/O errors other than a missing file stop loading and saving
before backup selection. Repair permissions or the storage fault, then retry;
an unreadable file does not establish corrupt state. Missing or corrupt current
generations can still recover the preceding valid generation (F-452).

SIGINT/SIGTERM freezes simulation, drains outstanding work and saves a final snapshot.
Deadline checks reject overdue completion and final submission; failure exits nonzero.
Synchronous capture, OS writes and scheduler pauses cannot be preempted by these
checks, so this is not a hard process-exit-time guarantee. Durability relies on
the filesystem honoring synchronization and atomic rename; qualification so far is
for local Linux storage, not network filesystems or physical power-loss testing.

`import-save --source … --destination … --dry-run` is a reserved interface: it
returns an explicit unsupported error without reading/writing the supplied paths.
Do not use it to migrate a real world yet.

`inspect-save --source PATH` opens nonblocking and admits only an opened regular
file; regular symlinks remain supported in every mode. Path-swap/FIFO regression
controls qualify this boundary, without a deadline for arbitrary regular-file
latency (F-468). It performs bounded, read-only
[experimental envelope checks](../specs/save-format/prospect-envelope.md). It reports sizes
and integrity results without extracting the payload or importing a world.
The optional `--inner-tags` also checks [root property framing](../specs/save-format/tagged-properties.md)
under an explicit experimental profile and reports counts; record values remain opaque.
Use `--recorders` to additionally check recorder field and byte-array boundaries;
it implies root framing and reports aggregates without class names or component data.
Validation covers authored fixtures and supplied-save envelope integrity; the
inner world schema and original-save round trips remain unvalidated.
The [synthetic workload](synthetic-workload.md) exercises native scheduling
and durability; it is not an ICARUS performance benchmark.
