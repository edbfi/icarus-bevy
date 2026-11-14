# Architecture and interfaces

Design constraints for the native server, repository boundaries and the
toolchain. The [plan](linux-dedicated-server-plan.md) sets goals and exit
criteria; [implementation status](implementation-status.md) records what exists.

Before restructure: 96ddb0b:docs/architecture.md

## Structure

Start with one root package, library and `icarus-server` binary. Use subsystem
modules/plugins; split crates only for concrete boundaries. Disable Bevy defaults;
start with `std`, `async_executor`, `multi_threaded`, `bevy_state`, `bevy_log` and
`MinimalPlugins`. Explicitly install services when used. No shipping display, GPU,
audio, original installation or compatibility layer dependency.

Flow: bounded network decoding → session/command validation → fixed-step Bevy
simulation → replication and persistence output.

Separate protocol codecs/version profiles; identity/admission/reconnect/platform
callbacks; authoritative simulation; replication; versioned persistence; and
operations. Never expose Bevy runtime `Entity` through wire, save or SDK interfaces.
Use domain IDs, session generations and explicit network mappings.

Blocking work uses bounded queues and owned results, never background mutations of
the ECS world. Discard obsolete results. Configure `FixedUpdate` from evidence;
order input, movement, gameplay, structural changes and extraction explicitly.
Preserve independent parallelism and apply deferred commands before consumers.
Use simulation time for gameplay, real monotonic time for deadlines. Bound catch-up
and backlog, reject new admissions under overload, and expose missed deadlines.
Seed randomness and stabilize command order for replay; demonstrate determinism.
Validate physics/navigation semantics before accepting a backend or optimization.

## Interfaces

Planned CLI interfaces (implemented commands: [native server runbook](runbooks/native-server.md)):

- `icarus-server run`
- `icarus-server check-config`
- `icarus-server import-save --source … --destination … --dry-run`

Precedence: explicit CLI overrides native configuration, which overrides defaults.
Provide official-settings import/mapping and report unsupported settings.

## Release tracking

ICARUS ships weekly. The Hello network version hashes the project version
string and is compared for exact equality (F-541, F-563); server code,
gameplay-tag tables and paks change between builds (F-700, F-702).
Release-specific protocol facts are therefore data, not code: a release is
one complete profile file, its build id is only a label, and the library holds
no per-release values.

- `config/release.toml` is the active release, validated by
  `experimental::release::ReleaseProfile`. `[release]` (app, build, project
  version, checked network checksum), `[tags]`, `[class_net_cache]` and
  `[row_handles]` are required, since every enabled open takes its tag
  encoding, field bounds and row widths from them; a missing one fails with
  an error naming it, so pin-only files no longer load. `[player_controller]`,
  `[player_state]`, `[game_state]`, `[pawn]`, `[managers]` and
  `[level_actors]` are optional: absent turns that open off, never another
  release's values.
- Frozen profiles of selected historical releases live under
  `tests/fixtures/releases/` (`3.0.32.toml`) for tests that need that
  layout; tests and examples load them with `ReleaseProfile::fixture("3.0.32")`
  or `fixture_path`. A release gets no fixture by default; where one exists
  for the active release, a test requires the active profile to equal it.
  The remaining gates keyed on build-id lists (quest class opens, visibility
  decoding, inbound controller bounds) are the next piece of this design.
- `tools/release_watch.py check` compares the pin with Steam's public branch
  (anonymous app info); the daily `Release watch` workflow opens an issue when
  the pin falls behind and closes resolved or superseded release issues.
  `diff --older DIR --newer DIR` compares build IDs, versions, checksums, PE
  section sizes and tag/node counts.
- `pin --install DIR` rewrites only `[release]` in place and verifies the
  result. The other sections stay but are unqualified for a new build, so
  `[release]` records `sections_qualified_for_build_id`, the build they were
  last qualified for, and the server refuses the file, naming both builds,
  until the key is removed. Re-pinning that build clears the key; tests keep
  it out of a committed pin. Only `pin` writes the key, so the refusal guards
  only files it re-pinned: a hand edit of build id, version and checksum still
  loads and serves the previous build's sections under the new version.
- A weekly bump, until a qualifier tool generates profiles, is a qualified
  edit of the data file: fetch the public server, run `diff` and `pin`,
  re-derive each section for the new build (tags, class checksums, row
  registry, layouts), write the values, remove the key and re-run the
  compatibility scenarios. That re-derivation is the guard; the only other is
  a frozen fixture for the active release, added only when a test needs it.
  The release is only provenance to the player store, so neither a new
  release nor any edit of this file refuses it ([persistence](#persistence)).
  No Rust change is needed unless a wire structure changed.
  Compatibility claims name the build they were made against; the native
  server answers only the pinned version by default.

## Persistence

Versioned domain snapshots cross a coherent simulation boundary: worker
serialization, checksummed generation, synchronized storage, atomic manifest
publication and retention of the preceding valid generation. Autosave defaults to
60 seconds. `SaveReceipt` acknowledges its submitted generation/tick after durable
publication, never later edits. Validate or explicitly migrate before activation;
missing required domain components refuse capture/publication. Authored schemas
4–11 (F-919, F-922, F-923, F-927, F-931) pin activation, reject incomplete state
and never promote implicitly; bounds imply neither physics nor navigation.
Contracts: [authored worlds](runbooks/authored-worlds.md).

Opt-in schema 14 saves foundation entities/clock and authored player checkpoints
together. `native_players` defines private records; `native_player_runtime::PlayerRegistry`
owns live reservations/committed state independently of `PeerLab`.
Only fresh, non-preserved foundation worlds are supported.
Existing schemas 1–12, preserved/imported worlds and native actor/lifecycle/inventory
composition refuse before Steam or sockets. Disabling the feature cannot load or
downgrade schema 14. Schema 13, the retired release-bound format, is never
migrated: it refuses with a message to move the state directory aside, and its
files stay untouched.

Configure release/catalog, independently authored gameplay and
`[network.authored_world]`, then add context matching hosting:

```toml
[network.player_persistence]
prospect_id = "stable-authored-workshop"
map_url = "/Authored/Maps/Workshop"
prospect_row = "AuthoredProspect"
difficulty = 2
```

Online use requires authenticated hosting. Offline reopen sets `network.enabled = false`
and omits `[network.hosting]`, retaining the player feature/context and all prepared
inputs.

A store binds its world (prospect id, map URL) and one digest per authored input:
the exact `network.authored_gameplay` bytes (formatting counts), `network.spawn`,
`network.authored_world`, `[network.player_persistence]` and the resolved initial
values (starting maxima, growth row by name). A change refuses, naming the input,
instead of refilling or resetting players. Passwords, tickets and names are excluded.
[Authored survival](runbooks/authored-survival.md): omission preserves
exact JSON/hash bytes and schema 14. Enabled rates hash `serde_json::to_vec` of
`("authored_player_rules_v1", declaration, rates, actual_tick_hz)` with SHA-256
into the declaration digest; clock settings bind the authored-world digest.
Authored tests qualify this contract (F-1183); stock-client behavior is unqualified.
The release is not bound: its build, version and checksum are recorded as
provenance at every open (event `player_store_reopened`) and never compared.

Stored state names game-data rows and never stores their index: a checkpoint's
`D_CharacterGrowth` row is stored as table and row name and resolved to the
installed index at open, so rows inserted, removed or reordered around it change
nothing; a missing row, or one moved past index 255, refuses naming table and
row. `CurrentBiome` is not stored: no biome table is loaded, every pawn opens
with row 0 and a checkpoint with another value refuses. No wire handle is stored.

Gameplay schema 1 binds full catalog row counts/names/order. Schema 2 accepts
sparse named rows in any order, normalized to installed catalog order; omitted
rows become name-only objects with parser defaults. Missing required tables,
unknown/duplicate/invalid names and ambiguous catalogs reject. Packaged gameplay
values never fill omissions. Expansion caps: 262,144 rows and 8 MiB UTF-8 row names
across tables, separate from the 8 MiB recipe limit.

The fresh-player generator's minimal schema-2 recipe is byte-stable under unrelated
catalog insertions/removals/reordering; skeleton mode emits a full schema-1 template.
Schema 14 binds exact recipe bytes, so schema 2 tolerates that churn unchanged;
schema 1 needs its matching catalog. Converting a bound recipe refuses
(`network.authored_gameplay differs`). No schema conversion, reset or client
qualification follows.

Keys require an accepted live ticket request, Joined admission and matching
nonnegative client-chosen Begin/Character slots. Login strings, names, profile XP
and client move reports confer no authority. Limits: 4,096 retained records,
64 per account, no eviction. One account/slot has one live attachment; a duplicate
newcomer refuses until detach/timeout, using shared profile-failure counters.

Controller/GameState/PlayerState bootstrap precedes validated slot lookup.
Controller spawn record/handle 19 use an uncommitted spawn peek; the later pawn
uses its checkpoint. Locations may differ; client effects remain unqualified.
New reservations immediately commit one spawn point/counter increment, preventing
concurrent reuse. Cancellation drops the pending record without refund; retries
reuse reservations and committed reconnects consume no point. The disabled path
retains legacy spawn/History/restore behavior.

`PlayerRegistry::edit` has only test callers and no operator surface: expected
revision and full checkpoint validate atomically, preserving maxima/biome/growth.
Reliable vital projection
coalesces unsent revisions; enqueue is neither client ACK nor disk publication.
Placement changes only the next pawn open. Disconnect/revocation detaches but
retains committed state.

[Authored leave cutoffs](runbooks/authored-survival.md#checkpoint-leave) use
`CheckpointWriter`, including shutdown after bridge drain and teardown. Only
the successful result of the exact submission containing the pinned record and
attempt marks it durable; generation/tick equality cannot. Cutoffs accepted after
capture wait for an idle writer, ahead of periodic autosave. No inventory owner,
client-driven progression or original-save import is inferred.

Supplied-save state may ride along as one lossless, value-opaque preserved
section that reconstructs the verified payload byte-identically, is carried
unchanged into every later generation and is written once as a content-addressed
sidecar (F-567). Preserving a section alone grants no semantic import acceptance.
Reverse export to official saves is not promised. Rollback uses original backups;
replacement-only progress is not assumed transferable.

## Bounds, operations and scale

Bound parsing/reassembly/peer queues/jobs. Invalid commands must not mutate state;
storage/input failures must not panic. Make teardown idempotent. Expose tick
progress, deadline misses, queues, save age/failures, sessions and subsystem cost.
Separate liveness/readiness. Shutdown stops admission, drains bounded work, saves
and exits with a timeout. Unexpected restarts count as reliability failures.

For large worlds, measure spatial indexing, replication relevancy, AI scheduling,
incremental structure updates and snapshots. Sleeping or slower updates must
preserve verified gameplay semantics.

## Toolchain

Rust **1.100.0** and Bevy **0.20.1**, verified current stable on 2026-11-12 (F-879)
([Rust](https://static.rust-lang.org/dist/channel-rust-1.100.0.toml),
[Bevy](https://crates.io/crates/bevy/0.20.1)). Pin Rust in `rust-toolchain.toml`
with matching Rustfmt and Clippy; commit `Cargo.lock`; use one toolchain for
development, CI, benchmarks and releases. Routine stable upgrades are authorized
and migrate APIs, pins and lockfile together; the Bevy skill and upgrade rules
are in [`.agents/rules/project.md`](../.agents/rules/project.md). Python helpers
target stable 3.15.0 under the basedpyright gate.

## Repository boundaries

| Location | Contents |
| --- | --- |
| `icarus-orig/`, `icarus-client-orig/` | Ignored original server files and installations; user-owned client binaries and manifests |
| `reference-data/` | Ignored user-supplied save archives and private inventories; preserved inputs, not backed up by Git |
| `work/` | Ignored scratch: cases, captures, dumps, private saves, analysis, benchmarks |
| `src/`, `tests/`, `benches/`, `tools/` | Reusable implementation, authored tests and research tools |
| `docs/`, `evidence/` | Plan, specifications, ledgers, digests, runbooks, frozen history and the evidence index |

Keep reference installations immutable during experiments; put runtime data and
disposable copies in a case directory. Commit only authored or reviewed,
redistributable fixtures; keep raw saves, player identifiers, names and member
paths out of fixtures, public reports, CI artifacts and uploads.
