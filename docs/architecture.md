# Architecture and interfaces

Design constraints, repository boundaries and toolchain for the native server.
Goals: [plan](linux-dedicated-server-plan.md); state:
[status](implementation-status.md).

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

Commands and arguments: [native server runbook](runbooks/native-server.md).

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
  only, unknown components opaque. The 3.0.37 profile omits these capabilities.
- Controller `selected_controller_v1` binds identity/pin/versions/bounds.
  F-1197 qualifies the 3.0.36 codec only. The
  [default-off leave experiment](runbooks/authored-survival.md#checkpoint-leave)
  follows F-1188. Live archive/cache/OnRep/menu behavior, coverage and original
  durability remain unproved.
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

Coherent snapshots use the [worker/generation protocol](runbooks/native-server.md#persistence-and-recovery).
`SaveReceipt` acknowledges its submitted generation/tick, never later
edits. Validate or explicitly migrate before activation; missing domain components
refuse capture/publication. Authored schemas 4–11 (F-919, F-922, F-923, F-927,
F-931) pin activation and forbid implicit promotion; bounds prove neither physics
nor navigation. [Contracts](runbooks/authored-worlds.md).

Opt-in schema 14 saves foundation entities/clock and private checkpoints;
`PlayerRegistry` retains reservations/committed state independently of PeerLab.
Fresh foundation only: schemas 1–12, preserved/imported and native spatial
actor/lifecycle/inventory composition refuse before Steam/sockets. Disabled
checkpoints cannot load/downgrade player stores. Retired release-bound schema 13
refuses load/migration/writes: move its state directory aside. Optional
[player bags](runbooks/authored-survival.md#player-inventory) select fresh schema
15 (local) or 16 (transfers), preserving context/release binding.

Configure release/catalog, independent gameplay and `network.authored_world`.
Declare stable `prospect_id`, `map_url`, `prospect_row` and `difficulty` in
`[network.player_persistence]`, matching hosting.

Online requires authenticated hosting; offline reopen disables networking,
omits hosting and retains context/prepared inputs. A store binds prospect id/map URL and
digests of exact authored_gameplay bytes, spawn, authored_world, player_persistence
and resolved initial maxima/growth row name. Changes refuse by name;
passwords/tickets/names are excluded. Survival omission keeps schema-14 bytes;
enabled rates bind SHA-256 of
`serde_json::to_vec(("authored_player_rules_v1", declaration, rates, actual_tick_hz))`.
Transfers wrap the base digest with
`("authored_player_inventory_transfers_v1", true, 16_u32, 2_u32, base_digest)`;
false keeps it exact. Clock binds authored_world (F-1183; authored only).
Release build/version/checksum record provenance at `player_store_reopened`,
never compared. [Operations](runbooks/authored-survival.md).

Checkpoints store `D_CharacterGrowth` names resolved at open; missing rows/indices
>255 refuse, unrelated catalog churn does not. No wire/biome handles persist:
pawns use biome 0, rejecting other checkpoint values.

Gameplay recipe schemas, caps and refusals:
[native server](runbooks/native-server.md#network-sessions-experimental) (F-1179);
the [generator](runbooks/network-readiness.md#fresh-player-stats)'s minimal
schema-2 recipes survive unrelated churn and its skeleton emits schema 1.

Authority needs accepted live tickets, Joined and matching nonnegative
client-chosen Begin/Character slots; login strings/names/profile XP/move reports
grant none. Retain ≤4096 records, 64/account, no eviction; one live account/slot
attachment. Duplicates refuse until detach/timeout, sharing profile-failure counts.
Bootstrap precedes slot validation; controller spawn/handle 19 peek uncommitted,
pawns use checkpoints (client effects unqualified). New reservations increment
spawn once: no cancellation refund, retries reuse it, committed reconnects use
none. Disabled checkpoints retain legacy spawn/History/restore.

Checkpoint edits preserve maxima/biome/growth and affect next pawn placement;
bag worlds use exclusive preflight wrappers ([contract](runbooks/authored-survival.md#player-inventory)).
Generic registry edits have test callers only. Vital projections coalesce unsent
revisions; enqueue proves neither ACK nor publication. Detach/revocation retains state.

[Leave operations](runbooks/authored-survival.md#checkpoint-leave): cutoffs, writer
pins, cancellation, completion and barriers. Default-off
`network.authored_controller_leave` needs authenticated checkpoints and
codec/controller/checksum binding before state/services. `player_leave` and
`sessions.controller_leave` totals survive teardown (F-1188); aggregate counters/
transport ACK prove neither player records nor client menus.

Run-only [private leave proof](runbooks/native-server.md#network-sessions-experimental)
requires schema 14; qualification is separate.

Supplied saves may ride as one lossless opaque preserved section, carried unchanged
into every later generation, reconstructing the verified payload byte-identically
and written once as a content-addressed sidecar (F-567).
Preservation grants no semantic import acceptance.
Reverse export is not promised; rollback uses original backups. Replacement-only
progress is not assumed transferable.

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
target stable 3.15.2 under the basedpyright gate.

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
