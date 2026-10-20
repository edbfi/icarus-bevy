# Native server: run, persistence and inspection

Operating guide for the headless foundation. Design constraints are in
[architecture](../architecture.md); what is proven is in
[implementation status](../implementation-status.md).

## Run

Requires native Linux and Rust 1.99.1 (pinned with Rustfmt and Clippy). Bevy 0.20.0
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

For qualification, `--compressed` (requires `--max-ticks`) runs those ticks
as simulated time without pacing: autosave and status intervals count
simulated seconds, and a due autosave awaits one still in flight, so a long
schedule of saves runs in minutes ([synthetic workload](synthetic-workload.md)).

The development default is 30 Hz with at most four catch-up ticks per update.
This is provisional, not a discovered ICARUS tick rate. No game/query sockets
open unless the experimental `[network]` section below is enabled.
The process emits JSON lifecycle/status/save records to stderr and always reports
`gameplay_ready: false`. Tick timing currently measures only foundation systems,
not representative gameplay performance.

Configuration precedence is CLI → TOML → defaults. Unknown fields and invalid
limits are rejected. Relative paths resolve from the working directory.
Config inputs must resolve to an opened regular file of at most64KiB; regular
symlinks are supported. FIFO and oversized inputs fail before runtime state opens
(F-671).
`check-config` prints effective configuration without creating the data directory.
Use `--help` on each subcommand for supported overrides.

## Network sessions (experimental)

An opt-in `[network]` table (disabled by default) runs the loopback peer lab
inside `run`, beside the fixed-step loop, autosave, signals and final save:

```toml
[network]
enabled = true
bind = "127.0.0.1"          # IPv4 loopback only
port = 17777                # 0 selects an ephemeral port; CLI: --network-port
max_players = 8             # admission occupancy limit, 1..=64
session_timeout_seconds = 30  # idle eviction, 1..=600, monotonic real time
```

The socket binds before the data directory opens; a bind failure exits
nonzero without touching world state. Each update polls at most 32 datagrams
before the fixed simulation ticks. Sessions follow the inferred handshake,
Hello/Challenge, Login/Welcome and Join admission model; capacity beyond
`max_players` gets an admission Failure, and a client control close bunch or
the idle timeout removes a session. SIGINT/SIGTERM first stops admission (new
handshakes ignored, Login/Join refused), then closes every session and runs
the normal drain and final save. A `network_listening` record gives the bound
address; `status` and `stopped` records carry `sessions` counts only, never
names, net ids or tokens.

It does not demonstrate compatibility with any ICARUS client, verify identity,
replicate actors or run gameplay; `gameplay_ready` stays false. It is tested
only with the authored probe on loopback:

```bash
cargo test --locked --test network
target/release/icarus-server run --config server.toml --data-dir work/net-smoke --max-ticks 600
```

## Persistence and recovery

Default state location: ignored `work/runtime/`. Each directory has one writer,
enforced by an advisory lock. Autosave runs every 60 seconds through a bounded
worker. Saves contain authored domain records and a simulation clock, not ICARUS
gameplay or its save format.
Snapshot capture requires every entity with a domain ID to have its required
position; incomplete state fails before submission instead of silently dropping
that entity from the durable generation. Entities without domain identity remain
outside the current native snapshot schema.
Native schema 1 holds only those records and the clock (64 MiB per generation).
A world with a lossless, value-opaque preserved prospect section saves schema 3:
the generation references a content-addressed `preserved-<sha256>.bin` sidecar,
written and synchronized once before any generation that references it and
verified by size and digest on load. A missing or damaged sidecar makes its
generation corrupt, so the previous one recovers. After publication, sidecars
no retained generation references are removed (exact name pattern, regular
files only). Schema-2 worlds with an inline section still load and move to
schema 3 at their next save; other schemas fail closed (F-567).

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

`preserve-save --source … --data-dir …` seeds a new native world whose only
content is the lossless preservation of a supplied prospect (schema 3, tick 0,
`world_imported: false`); it refuses a directory that already holds a world.
The world then runs and autosaves like any other; nothing in it is interpreted.

`import-save --source … --destination … --dry-run` plans a native import and
prints the plan (`native_import_plan_only`, counts only): full staging with each
recorder's `Validate` decision, lossless typing of every retained recorder, base
recorder fields, actor transforms and every name or `/Game/`/`/Script/` path
reference checked against the native content catalog (empty today). Blockers
list unmapped content, uninterpreted values, non-finite transforms or an empty
world. Without `--dry-run` the import is refused; nothing is ever written and
the destination is not touched.

`inspect-save --source PATH` opens nonblocking and admits only an opened regular
file; regular symlinks remain supported in every mode. Path-swap/FIFO regression
controls qualify this boundary, without a deadline for arbitrary regular-file
latency (F-671). It performs bounded, read-only
[experimental envelope checks](../specs/save-format/prospect-envelope.md). It reports sizes
and integrity results without extracting the payload or importing a world.
The optional `--inner-tags` also checks [root property framing](../specs/save-format/tagged-properties.md)
under an explicit experimental profile and reports counts; record values remain opaque.
Use `--recorders` to additionally check recorder field and byte-array boundaries;
it implies root framing and reports aggregates without class names or component data.
Use `--stage-check` (optionally with `--expect-prospect-name NAME`) to validate
the full `FProspectSaveState` root schema, `PostLoadSerialize` migration,
`FProspectInfo` nested metadata (including `AssociatedMembers` and
`CustomSettings`), optional `ProspectID` matching, and base-component framing of
every `StateRecorderBlob` (`experimental_stage_only`, `world_imported: false`,
F-738).
`--typed-check` stages the prospect, then types every recorder value (scalars,
strings, text, tagged and fixed-width engine structs, arrays, Map/Set items)
under the hard tree ceilings; each body must re-encode to its original bytes,
and leaves that cannot be typed are counted by reason per class pseudonym.
`--preserve-check` preserves the payload losslessly, saves and reloads it through
a temporary schema-3 store that it removes, saving twice to show the sidecar is
not rewritten, and reports counts, sizes, timings and resident memory with
`world_imported: false` (F-567). A running server holds the section as its
verified encoding, not as a decoded tree, and verifies it on load in one
streaming pass without building the tree (F-584).
Validation covers authored fixtures, supplied-save envelope integrity and the
lossless preservation; value meanings and original-server round trips remain
unvalidated.
The [synthetic workload](synthetic-workload.md) exercises native scheduling
and durability; it is not an ICARUS performance benchmark.
