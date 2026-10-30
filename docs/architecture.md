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
Release-specific protocol facts are therefore data, not scattered code
constants:

- `config/release.toml` pins one official build (`[release]` plus optional
  `[tags]`, `[class_net_cache]`, `[row_handles]` and `[player_controller]`
  overrides, validated by `experimental::release::ReleaseProfile`).
- `tools/release_watch.py check` compares the pin with Steam's public branch
  (anonymous app info); the daily `Release watch` workflow opens an issue when
  the pin falls behind and closes resolved or superseded release issues.
  `pin --install DIR` rewrites the pin, and `diff --older DIR --newer DIR`
  compares build IDs, versions, checksums, PE section sizes and tag/node counts.
- `ReleaseProfile` isolates per-release protocol metadata (project version,
  network checksum, tag net-index widths and `NetworkGameplayTagNodeIndexHash`,
  `FClassNetCache` bounds and `FRowHandle` table widths) for `PeerLabConfig` and
  `examples/capture_decode`; with `release_watch.py diff` and capture replay,
  weekly updates are mechanical unless wire semantics change.
- `render_pin` writes `[release]` only, and `ReleaseProfile::parse` fails
  closed on an unknown `build_id` without `[tags]`: a new build needs a built-in
  profile or a qualified `[tags]` section.
- A bump is version- and profile-only until the weekly diff shows otherwise:
  compare the two builds' sections, public symbols and replicated layouts, and
  re-run the compatibility scenarios. Compatibility claims name the build they
  were made against; the native server answers only the pinned version by
  default.

## Persistence

Persistence uses versioned domain snapshots: coherent simulation boundary,
worker serialization, checksummed new generation, synchronized storage, atomic
manifest publication, preceding valid generation retained. Autosave defaults to
60 seconds. Acknowledge only durable publication; validate/migrate before activation.
Capture requires all components declared by the current native domain schema;
an identified entity with missing required data must fail before publication.
Authored schemas 4–11 (F-919, F-922, F-923, F-927, F-931) pin their
activation, reject incomplete state and never promote implicitly; bounds imply
neither physics nor navigation. Contracts: the
[authored-worlds runbook](runbooks/authored-worlds.md).
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

Rust **1.99.1** and Bevy **0.20.1**, verified current stable on 2026-10-30 (F-879)
([Rust](https://static.rust-lang.org/dist/channel-rust-1.99.1.toml),
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
