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
  `visibility_decode` and `inbound_controller` revisions. Missing declarations
  disable the capability; unknown revisions fail parsing. Its complete pin,
  engine/game versions and class-bound snapshot must equal the profile before
  planning or decoder output. Runtime qualification reads no test fixture.
  Editing both sides requires qualification; metadata cannot prove provenance.
- `quest_classes_v1` selects the existing reviewed `QUEST_CLASSES` catalog
  (F-935, M-1030), only declared on 3.0.31 server/client aliases and 3.0.32.
  Visibility `normal_custom_v1` is declared only on 3.0.34/3.0.35 for conditional
  decoding (F-1034). Caller archive/layout assertions, production switch refusal
  and the [visibility gate](gates.md#before-enabling-visibility-dispatch-or-effects)
  remain; metadata installs no dispatch or effects. Inbound
  `fixed_component_bounds_v1` is declared only on 3.0.32/3.0.34, retaining full
  identity/framing checks and fixed component bounds 149/27/8 (F-1015, F-1016,
  M-1197). Output stays metadata-only; unknown components stay opaque. Active
  3.0.36 declares none; F-1177 qualifies none of these capabilities.
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

Persistence uses versioned domain snapshots: coherent simulation boundary,
worker serialization, checksummed new generation, synchronized storage, atomic
manifest publication, preceding valid generation retained. Autosave defaults to
60 seconds. `SaveReceipt` acknowledges its submitted generation/tick after durable
publication, not later in-memory edits. Validate or explicitly migrate before activation.
Capture requires all components declared by the current native domain schema;
an identified entity with missing required data must fail before publication.
Authored schemas 4–11 (F-919, F-922, F-923, F-927, F-931) pin their
activation, reject incomplete state and never promote implicitly; bounds imply
neither physics nor navigation. Contracts: the
[authored-worlds runbook](runbooks/authored-worlds.md).

Opt-in schema 14 combines foundation entities/clock with authored player checkpoints
in one save generation. `native_players` defines private records;
`native_player_runtime::PlayerRegistry` owns live reservations and committed state
independently of `PeerLab`. Only fresh, non-preserved foundation worlds are supported.
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

Authored gameplay schema 1 retains its strict full catalog row-count/name/order
contract. Schema 2 supplies sparse rows keyed by name, in any order. The loader
normalizes each supplied table to installed catalog order, creating name-only
objects with parser defaults for omitted rows. Missing required tables,
unknown/duplicate/invalid authored names and ambiguous catalogs reject. No
packaged gameplay values fill omitted rows or fields.
Schema-2 expansion is capped across supplied tables at 262,144 rows and 8 MiB
of UTF-8 row-name bytes, independently of the 8 MiB recipe-file limit.

The fresh-player generator emits a deterministic minimal schema-2 recipe:
unrelated catalog row insertions, removals or reordering do not change its bytes.
Its skeleton mode remains a full schema-1 template. A schema-14 store still binds
the exact recipe bytes; schema-2 stores can therefore survive unrelated catalog
churn without editing their recipe. Schema-1 recipes still require a matching
full catalog; converting an existing store's recipe to schema 2 changes the input
and refuses (`network.authored_gameplay differs`). Persistence remains schema 14;
no conversion, automatic reset or new client qualification is implied.

Keys require an accepted live ticket request, Joined admission and matching
nonnegative client-chosen Begin/Character slots. Login strings, names, profile XP
and client move reports confer no authority. Limits: 4,096 retained records,
64 per account, no eviction. One account/slot has one live attachment; a duplicate
newcomer refuses until detach/timeout, using shared profile-failure counters.

Controller/GameState/PlayerState bootstrap precedes validated slot lookup. The
controller spawn record and handle 19 use a provisional uncommitted spawn peek;
the later pawn uses its resolved checkpoint, so their locations may differ.
Client effects of this divergence/order are unqualified. A new reservation commits
one spawn point/counter increment immediately, preventing concurrent reservations
from sharing that selection. Cancellation drops the pending player record without
refunding the point/counter. Retries reuse reservations; committed reconnects
consume no point. The disabled path retains its legacy spawn/History/restore behavior.

`PlayerRegistry::edit` is a library API with test callers only; no operator
surface exists. It validates an expected revision and complete checkpoint
atomically, preserving maxima/biome/growth. Reliable vital projection
coalesces unsent revisions; enqueue success is neither client ACK nor disk publication.
Placement edits affect the next pawn open only, without live teleport/movement
simulation. Disconnect/revocation releases attachments and retains committed state.
Shutdown drains the bridge before teardown and captures the registry through the
existing save worker. An edit accepted during an in-flight save belongs to a later
capture; the earlier receipt gives it no durability promise. No player inventory
owner, client-driven progression or original-save import is inferred.

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
