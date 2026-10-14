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

Planned CLI interfaces (see the [native server runbook](runbooks/native-server.md) for
implemented commands and experimental inspection):

- `icarus-server run`
- `icarus-server check-config`
- `icarus-server import-save --source … --destination … --dry-run`

Precedence: explicit CLI overrides native configuration, which overrides defaults.
Provide official-settings import/mapping and report unsupported settings.

## Persistence

Persistence uses versioned domain snapshots: coherent simulation boundary,
worker serialization, checksummed new generation, synchronized storage, atomic
manifest publication, preceding valid generation retained. Autosave defaults to
60 seconds. Acknowledge only durable publication; validate/migrate before activation.
Capture requires all components declared by the current native domain schema;
an identified entity with missing required data must fail before publication.
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

Rust **1.99.0** and Bevy **0.20.0**, verified current stable on 2026-10-13 (F-483)
([Rust](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/),
[Bevy](https://crates.io/crates/bevy/0.20.0)). Pin Rust in `rust-toolchain.toml`
with matching Rustfmt and Clippy; commit `Cargo.lock`; use one toolchain for
development, CI, benchmarks and releases. Routine stable upgrades are authorized:
verify upstream release metadata, migrate APIs, update pins and lockfile together,
and record concrete blockers when an upgrade cannot pass.

The Bevy skill under `.agents/skills/bevy/` is installed from `edbfi/skills`
with `bunx skills`; `skills-lock.json` records source and content hash. Python
helpers target stable 3.15.0 under the basedpyright gate described in
[`.agents/rules/project.md`](../.agents/rules/project.md).

## Repository boundaries

| Location | Contents |
| --- | --- |
| `icarus-orig/` | Ignored original server files and preserved installations |
| `icarus-client-orig/` | Ignored user-owned original client binaries and manifests |
| `reference-data/` | Ignored user-supplied save archives and private inventories; preserved inputs, not backed up by Git |
| `work/` | Ignored cases, captures, dumps, private saves, generated analysis and benchmarks |
| `src/`, `tests/`, `benches/`, `tools/` | Reusable implementation, authored tests and research tools |
| `docs/`, `evidence/` | Plan, specifications, ledgers, digests, runbooks, frozen history and the evidence manifest |

Keep reference installations immutable during experiments; put runtime data and
disposable copies in a case directory. Commit only authored or reviewed,
redistributable fixtures. Keep raw saves, player identifiers, names and member
paths out of tracked fixtures, public reports, CI artifacts and uploads.
