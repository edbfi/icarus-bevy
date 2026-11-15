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

Hello hashes the project version and compares it for exact equality (F-541,
F-563); weekly releases also change code, tags and paks (F-700, F-702).
A complete profile carries protocol values; its build id selects none.

- `config/release.toml` is active, validated by `ReleaseProfile`. Required
  tables are `[release]` (app, build, version, checked checksum), `[tags]`,
  `[class_net_cache]` and `[row_handles]`. Opens take their encoding, bounds
  and row widths from these tables. Optional `[player_controller]`,
  `[player_state]`, `[game_state]`, `[pawn]`, `[managers]` and `[level_actors]`
  supply identities; absence turns that open off, with no fallback.
- Optional `[qualifications]` declares typed `quest_catalog`,
  `visibility_decode`, `inbound_controller` and `controller_leave` revisions.
  Missing declarations disable the capability; unknown revisions fail parsing.
  Its complete pin, engine/game versions and class-bound snapshot must equal
  the profile before
  planning or decoder output. Runtime qualification reads no test fixture.
  Editing both sides requires qualification; metadata cannot prove provenance.
- `quest_classes_v1` binds the reviewed catalog on 3.0.31 aliases/3.0.32
  (F-935, M-1030). Visibility `normal_custom_v1` on 3.0.34/3.0.35 grants
  conditional decoding only (F-1034), retaining caller archive/layout assertions,
  production refusal and the [visibility gate](gates.md#before-enabling-visibility-dispatch-or-effects).
  Inbound `fixed_component_bounds_v1` on 3.0.32/3.0.34 keeps complete identity/
  framing checks and bounds 149/27/8 (F-1015, F-1016, M-1197); output is metadata
  only, unknown components opaque. Active 3.0.36 retains none of these.
- Controller `selected_controller_v1` binds identity/pin/versions/bounds.
  F-1197's conditional codec (M-1498 to M-1501) permits an explicit default-off
  experiment; metadata grants no dispatch. F-1188 requires current exact pinned
  cutoffs, cancel suppression and reliable admission/ACK; obsolete state cannot
  replace the current cutoff. Live archive/cache/OnRep/menu behavior, coverage and
  original durability remain unproved.
- Historical profiles under `tests/fixtures/releases/` are loaded explicitly
  with `ReleaseProfile::fixture` or `fixture_path`. No release gets a fixture
  automatically; an active fixture, where present, must equal the pin.
- `tools/release_watch.py check` compares Steam's public branch anonymously;
  the daily workflow manages release issues. `diff --older DIR --newer DIR`
  compares identities, PE section sizes and tag/node counts. `pin --install DIR`
  rewrites only `[release]`, marking carried-over tables with
  `sections_qualified_for_build_id`. Any marker refuses the profile, naming
  both builds; pin-only reads remain possible. Clearing it does not transfer
  capabilities: their bindings must also validate. Other sections still depend
  on review of hand edits without the marker.
- A weekly bump requires deriving and qualifying each retained section from
  allowed inputs, updating or removing capability declarations, clearing the
  marker and rerunning compatibility scenarios. No Rust change is needed
  unless a wire structure or reviewed revision changes. Release metadata is
  only provenance to the player store ([persistence](#persistence)). Claims name
  their qualified build; the server answers only the pinned Hello version.

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

A store binds prospect id/map URL and digests of exact `network.authored_gameplay`
bytes, `network.spawn`, `network.authored_world`, `[network.player_persistence]`
and resolved initial maxima/growth row name. Changed inputs refuse by name;
passwords/tickets/names are excluded. [Survival](runbooks/authored-survival.md)
omission preserves bytes/schema 14. Enabled rates bind SHA-256 of
`serde_json::to_vec(("authored_player_rules_v1", declaration, rates, actual_tick_hz))`;
clock settings bind the authored-world digest (F-1183; authored only). Release
build/version/checksum are provenance at each `player_store_reopened`, never
compared.

Checkpoints store `D_CharacterGrowth` table/row names, resolved at open; missing
rows or indices above 255 refuse by name. Unrelated catalog churn is harmless.
No wire handle or `CurrentBiome` is stored: no biome table is loaded, pawns open
at row 0 and checkpoints with another value refuse.

Gameplay schema 1 binds catalog counts/names/order; schema 2 normalizes sparse
named rows to installed order, defaulting omitted values independently of pak
values. Missing tables, invalid/duplicate/unknown names and ambiguous catalogs
reject. Expansion caps: 262,144 rows and 8 MiB UTF-8 names, separate from the
8 MiB recipe limit. The generator's minimal schema-2 recipe survives unrelated
catalog churn byte-identically; skeleton mode emits schema 1. Schema 14 binds
exact recipe bytes: conversion refuses (`network.authored_gameplay differs`),
without reset or client qualification (F-1179).

Keys require an accepted live ticket request, Joined admission and matching
nonnegative client-chosen Begin/Character slots. Login strings, names, profile XP
and client move reports confer no authority. Limits: 4,096 retained records,
64 per account, no eviction. One account/slot has one live attachment; a duplicate
newcomer refuses until detach/timeout, using shared profile-failure counters.

Bootstrap precedes validated slot lookup. Controller spawn record/handle 19 use
an uncommitted spawn peek; the pawn uses its checkpoint. Client effects remain
unqualified. New reservations commit one spawn point/counter increment; canceled
reservations get no refund, retries reuse it, committed reconnects consume none.
Disabling checkpoints retains legacy spawn/History/restore.

`PlayerRegistry::edit` has only test callers and no operator surface: expected
revision and full checkpoint validate atomically, preserving maxima/biome/growth.
Reliable vital projection
coalesces unsent revisions; enqueue is neither client ACK nor disk publication.
Placement changes only the next pawn open. Disconnect/revocation detaches but
retains committed state.

[Authored leave](runbooks/authored-survival.md#checkpoint-leave) uses
`CheckpointWriter` through shutdown. Durability requires the pinned attempt's
exact submission before completion, then prefix ACK before detach. Pre-admission
cancellation retains the frozen obligation and suppresses completion. Default-false
`network.authored_controller_leave` separately enables the policy only with
checkpoints, authenticated online hosting and matching codec/controller plans.
`check-config` checks shape; offline preparation binds before storage/services.
Opt-in status/stopped `player_leave` reports cutoff, durability/
failure gauges and last global generation/tick; `sessions.controller_leave`
reports intent/cancel/admission/prefix-ACK totals. These survive teardown;
aggregates or ACK cannot prove exact private record equality or menu success.
No full player-save/client qualification follows.

Default-off Run-only `--private-leave-proof` binds confined private receipts to a
case/manager InvocationID and one admission. Exact cutoff/readback, restart load
and first pawn admission guard activation. A typed observer requires ordered
receipts and owner menu confirmation; successful exit/sealing separately gates
restart. Failure is terminal; schema/configuration stay unchanged. This grants no
OnRep, terrain or stock-client qualification.

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
