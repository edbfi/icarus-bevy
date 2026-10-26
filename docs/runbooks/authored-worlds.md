# Authored native worlds

Before restructure: 3e49565:docs/runbooks/authored-worlds.md

Running, autosave and recovery: [native server](native-server.md).

## Shared contracts

Creation preflights before destination access; `--dry-run` writes nothing,
omission publishes, existing/recoverable worlds reject. Strict JSON/TOML, format
1. Fixtures pin payload SHA-256, catalog/map, UTF-16 identities/unchanged
sidecar. Owner-fixture commands share `--source`, `--catalog`, `--lifecycle`,
`--bindings`, `--data-dir` and optional `--dry-run`; `run` needs no creation
inputs.

Coordinates: metres, right-handed, Z-up, quaternion `[x,y,z,w]`; bindings declare
`right_handed_z_up_meters_quat_xyzw_world_space`, without inferred conversion.
Objects must fit tiles (F-919). Every mode is independent authored behavior with
no official geometry/layout/stat/row meaning, physics/navigation, callbacks,
replication, general C001/D01/C009 restore, client/save compatibility or
ICARUS performance/RSS qualification; `world_imported`/`gameplay_ready` stay
false and `import-save` stays closed.
Definitions/contracts/bindings/metadata: 4 MiB each unless noted. Schemas 4–11:
64 MiB + 4 KiB generation including sections/sidecar reference. Earlier adapters
retain acceptance; older schemas reject new fields, even null.
Runtime calls validate membership, contexts/generations, indices and
domain-ID/incarnation handles; mutations stage changes and lifecycle modes
preflight storage. Missing, malformed, stale, transplanted or hidden unregistered
state rejects; replacement invalidates handles. Costs scale with source/world
size; tick-loop use is unqualified (F-926).

## Spatial worlds and inert source fixtures

```bash
cargo run --locked -- create-native-world --catalog config/native-world.example.toml \
  --map workshop --data-dir work/authored-world
cargo run --locked -- run --data-dir work/authored-world
cargo run --locked -- create-c001-inert-fixture --source authored.json \
  --catalog authored.toml --bindings bindings.json --data-dir work/inert-world --dry-run
```

Schema 4 fixes topology; `update_native_actor` validates edits. Schema 5 binds
source map/class/path/object-name units and positive GUIDs to placements/classes
once and fully interprets exact current C001 recorders; source identities/bytes
stay immutable, poses mutate, output reports counts/categories (F-927). No
variables, extra state, spawning, stitching or nonneutral prospect behavior.

## Foundation relationships

`--relationships FILE`: `relationship_format = 1`, `native_catalog_id`, `map`;
each `placement` declares positive `identity.guid`, UTF-16 `identity.name`,
`role` (`spatial`, `deployable`, `foundation`). Only deployables link
`foundation`; omission/null unlinks, nonempty must resolve. Identities are
explicit: match GUID plus ASCII-folded name (non-ASCII and unpaired surrogates
exact); duplicate/unresolved pairs reject. Names: 1–1,024 units, 1,048,576 total
including links (F-922). Assignment follows F-646/F-650/F-663 except stricter
unresolved-link rejection; other setter effects/releases are unqualified.

Schema 6 (otherwise as 4/5): immutable declarations, mutable domain-ID edges,
optional C001 metadata; links independent of source, unlinked components valid.
`reassign_native_foundation` edits (`None` unlinks; no-ops succeed);
`native_foundation_of`/`native_dependents_of` query. No
parenting/support/collapse, destruction propagation or tame permissions.

## Owner lifecycle

```bash
cargo run --locked -- create-native-world --catalog authored.toml --map workshop \
  --lifecycle lifecycle.json --restore restore.json --data-dir work/owners --dry-run
```

Lifecycle: catalog/map; each placement declares UTF-16 name, `spatial_state_v1`,
`live`/`absent`, optional positive GUID, spawn component
`missing`/`declared_kind`. Limits: 1–1,024 units/name, 1,048,576
logical/2,162,688 capacity units; 8 MiB lifecycle, 64 MiB restore, 16,384
slots/bindings/commands. Placement/tile counts have capacity guards. Schema 7
persists live/dormant slots/incarnations/bindings (F-923); only live slots
spawn. No C001 metadata; preservation/relationships optional.

`RestoreBatch`: lifecycle ID/sequence, nonempty commands with GUID/kind,
placement, name, policy, source mode, creation permission, expected
presence/incarnation, complete pose/variables. Detached lookup ASCII-folds
names; ambiguity rejects. Bound lookup uses GUID, requires unchanged state,
retains even `respawn_only`. Detachment supplies expected prior binding/null;
attachment may retire incumbent GUID binding without incarnation change.
Foundation identities stay separate.

Detached `find_only` attaches live; `find_or_respawn` attaches live/spawns absent,
rejecting failed attachment; `respawn_only` replaces live/spawns absent.
Replacement keeps domain ID/edited live-endpoint edges, increments incarnation.
All commands validate before mutation; no forced/manual resolution or
undeclared spawning. Sequence starts
at 1, increments, matches `restore_sequence`; exact latest retries preserve
edits, altered/older reject. Reload never replays policies. Completion is
durable only after autosave or shutdown publishes it.

Owner mutations and capture reject managed-owner ECS hooks, requirements and
lifecycle observers, even on untouched owners or uninstantiated optional types
(F-928; modifiers too). Unrelated entity observers are allowed; global raw
lifecycle-event observers (`On<RemoveEvent>`) block even autosave capture. Flush
pending commands/registrations first. `live_actors` counts live; `actors`
includes dormant.

## Authored owner modifiers

Schema 11 extends authored lifecycle 7 only (F-931), explicitly via
`enable_modifier_owners` or:

```bash
icarus-server create-native-world --catalog authored.toml --map workshop \
  --lifecycle lifecycle.json --modifiers --data-dir authored-world --dry-run
icarus-server apply-native-modifier --request modifier-request.json --data-dir authored-world
```

Omit creation's `--dry-run` to publish first. Preservation/foundation
relationships compose; C001/scalar/inventory fixtures never implicitly upgrade.
Live handles own per-owner modifiers; dormant slots have none; replacement
retires them.

```json
{"owner":{"id":1,"incarnation":1},"expected_revision":0,"action":{"type":"add","definition":{"name":"authored_boost","duration":{"type":"timed","seconds":5.0},"contributions":[{"stat":"authored_power","amount":3}]}}}
```

State format 1 stores owner handle, revision, next ID, entries and stat cache.
Definition names are unique per owner; local IDs are never reused. Duration:
`{type: indefinite}` or `{type: timed, seconds: f32}`, quantized to the nearest
nanosecond; positive durations below half a nanosecond or over 365 days reject.
Timer: `{type: indefinite}` or `{type: timed, remaining_nanos: u64, armed: bool}`;
positive timers arm, restored zero stays unarmed.

`ModifierRequest {owner, expected_revision, action}` adds a definition, removes
an ID or advances `elapsed_nanos: u64`. Flat i32 stat totals aggregate in i64
with a final range check; removal/expiry overflow rejects the whole operation
or all-owner step unchanged.
`native_owner_modifiers` returns a validated copy;
`native_lifecycle_runtime::{apply_native_modifier, advance_native_modifiers}`
stage changes with `SnapshotStore` preflight. Fixed time advances before
`Gameplay`; no wall-clock catch-up on restart.

Activation is system-owned; a private health anchor latches errors closed and
`run` checks it. The apply CLI requires an existing world and strict JSON
≤64 KiB; symlink path components and FIFOs reject. No `--dry-run`: opening the
store mutates its lock/directory.

Limits: 1,024 live owners, 64 entries/owner, 16 contributions/definition,
128 stat names/owner, 4,096 entries/world, 8 MiB charged capacities.

### Modifier advance costs

Full capture and preflight encode run every fixed step, even without armed
timers; catch-up repeats that cost (F-926). Release Rust 1.99.1/Bevy 0.20.0, two
pinned EPYC 9534 CPUs, five matched pairs (three fresh apps × ten calls/cell;
five-run median p95 ms; four entries per nonempty owner; exact state verified
after each call; setup excluded; no relationships/sidecars):

| State per owner | 1 owner | 32 | 256 | 1,024 |
| --- | --- | --- | --- | --- |
| empty | 0.055 | 0.386 | 2.754 | 11.685 |
| indefinite | 0.043 | 0.583 | 4.716 | 18.267 |
| armed timers | 0.056 | 0.579 | 4.400 | 18.521 |

1,024 owners is an admission cap, not a 30 Hz guarantee; whole-frame, health,
catch-up, uniform speedup and RSS stay unqualified.

```bash
cargo run --release --locked --example native_modifier_bench -- --seconds 120
```

## Authored C001 owners

`create-c001-owner-fixture`, binding mode `c001_authored_owners_v1`: lifecycle
ID, one recorder/binding per slot; explicit placement/class, lookup name, GUID,
creation permission/presence/incarnation/prior-binding expectations. Inert
restrictions except `FindOnly`, `FindOrRespawn` (default), `RespawnOnly`; no
policy/pose overrides. Sequence-1 detached restore completes all owners.
Independent `--relationships`; no `--restore`.

Schema 8 keeps immutable provenance/completed lifecycle (F-927). Load verifies
the initial receipt in a temporary world, then activates current state. Edits
and GUID retirement are allowed; imported slots stay live with recorders,
incarnation at least initial. Capture checks pinned state without source
reconstruction; restore preflight repeats reconstruction/receipt verification.

## Candidate Bool/I32 assessment

Library-only `native_import::c001_variables::prepare_c001_variables` (F-929)
pins payload, catalog/maps, recorder ordinals, UTF-16 owner/property identities,
placements/classes and native types, covering every retained, losslessly typed
C001 recorder and both arrays (exact shared headers even when empty). Bindings
assert `save_game = true`; official schema/handlers remain open (F-735, F-897).
Independent array/element-tag/record-type/name-field/value-field names support
multiple Bool/I32 properties and signed I32 extrema. Duplicate/incomplete
bindings and unsupported, dropped, unavailable, non-C001 or malformed terminal
records reject all. Limits: 16,384 owners, 64 combined properties/owner,
1,048,576 assignments, 32 MiB charged output/temporary lookup capacity, plus
candidate byte/element headroom, even empty (F-920). Debug/serialization redact
details; errors expose only ordinal/category. Every assignment retains
`UnqualifiedPropertyRestoreAndRepNotify` and never activates; no mutation,
callbacks, publication or approval of unrelated roots, and the use gate below
qualifies no handler here.

## Authored scalar owners

`create-c001-scalar-owner-fixture --contracts FILE`, binding mode
`c001_authored_scalar_owners_v1`, adds two single-record arrays with exact
UTF-16 array/element-tag/record-type/value-field/source-variable names. Require
`VariableName: NameProperty`, canonical Bool/I32; unknown fields (even empty
arrays), wrong headers/types, duplicates or incomplete coverage reject all.

Used classes bind `recompute_use_availability_v1`, positive `authored_max`,
exactly `enabled: Bool`, `remaining_uses: I32` in `0..=authored_max`. Saved
pairs replace defaults/placement overrides on attachment/spawn/replacement.
`try_use`: `Disabled`/`Exhausted` unchanged, else decrement once, return `Used`;
`set_enabled` sets, `scalar_owner_state` queries; distinct in-memory actions, no
retry/durable ack. Availability derives from variables; its authored callback
stages before mutation. RepNotify stays unqualified. Generic edits preserve
scalars; explicit restore supplies complete pairs. Schema 9 stores
contract/binding IDs/provenance and reloads availability without initial replay;
relationships compose.

## Authored inventory owners

`create-c001-inventory-owner-fixture --scalar-contracts FILE --inventory-contracts FILE`,
binding mode `c001_authored_inventory_owners_v1`: scalars plus authored item/inventory keys,
stack limits/capacities/eligibility; saved rows select exact bindings, never
packaged meanings. Pin source/catalog/lifecycle/scalar/inventory IDs and nested
array/struct/field names. UTF-16 limits: 1,023 header content units plus
terminator, 1,024 payload units. Every owner/inventory, even empty, is
required; unsupported final records reject all.

D01: location `0..=255`, nonempty static row, one `ItemableStack` dynamic
property (index 7), positive bounded count; explicitly empty additional stats,
alterations, living slots, item GUID; owner lookup ID `-1`. Selector 263 cannot
alias 7. Associated inventories/ammo/durability/decay/energy/other state reject;
canonical types/headers/terminators/widths required.

IDs follow recorder/inventory/item order. Eligible empty allocated slots accept
whole stacks. On rejection, zero slots may force-add one eligible slot; nonzero
slots defer whole stacks to ordered overflow; saturation rejects. Routing:
F-735; acceptance/growth authored.

`inventory_owner_state`/`inventory_stack` query owned state/location.
Move/split/merge/consume/place-overflow work across owners; overflow
destinations select an empty allocated slot or append one eligible next slot;
handles/global revisions guard writes. Move
keeps IDs; split allocates one; merge/full consumption retires IDs; only
consumption reduces totals.

`InventoryRestoreBatchV1` hashes complete owner/inventory state, contract IDs
and revision expectations under one lifecycle sequence/completion; no pose-only
restore. Replacement conserves targeted stack-ID/item/count multisets; bound
inventories must be unchanged. Scalar/pose/foundation edits preserve inventories.

Schema 10 embeds contracts, contents/allocator/revisions/consumed totals,
lifecycle and immutable initial provenance/source; optional relationships.
Reload never resurrects consumed stacks. Limits: 16,384 owners, 1,024 item
definitions, eight inventories/owner, 65,536 instances/stacks each, 256
slots/overflow entries per inventory, 512 source items/inventory, counts
≤1,000,000; 16 MiB state, 64 MiB joint graph allowance including decoder
growth/conversions, not RSS; shared caps apply (F-927). No weight behavior.
