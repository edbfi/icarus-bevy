# Authored native worlds

Before restructure: 3e49565:docs/runbooks/authored-worlds.md

Running, autosave and recovery: [native server](native-server.md).

## Shared contracts

Preflight before destination access. `--dry-run` writes nothing; omission
publishes, rejecting existing/recoverable worlds. `world_imported`/`gameplay_ready`
stay false; `import-save` stays closed. Strict JSON/TOML, format 1. Fixtures pin
payload SHA-256, catalog/map, exact UTF-16 identities/unchanged sidecar;
`run` needs no creation inputs.

Coordinates: metres, right-handed, Z-up, quaternion `[x,y,z,w]`; bindings declare
`right_handed_z_up_meters_quat_xyzw_world_space`, without inferred conversion.
Objects must fit tiles (F-919); no official behavior/geometry, physics/navigation,
layouts, callbacks, networking or performance qualification.
Definitions/contracts/bindings/metadata: 4 MiB each unless noted.
Schemas 4–10: 64 MiB + 4 KiB generation, including sections/sidecar
reference. Earlier adapters retain acceptance; older schemas reject new fields,
even null. Runtime helpers validate worlds/handles before queries/no-ops/capture,
stage mutations; lifecycle modes also preflight storage. Reject missing, invalid,
stale, transplanted or hidden
unregistered state; pin membership, contexts/generations, indices. Handles bind
domain ID/incarnation; replacement invalidates them. Costs depend on source/world
size; tick-loop use unqualified.

## Spatial worlds and inert source fixtures

```bash
cargo run --locked -- create-native-world --catalog config/native-world.example.toml \
  --map workshop --data-dir work/authored-world
cargo run --locked -- run --data-dir work/authored-world
cargo run --locked -- create-c001-inert-fixture --source authored.json \
  --catalog authored.toml --bindings bindings.json --data-dir work/inert-world --dry-run
```

Schema 4 fixes topology. `update_native_actor` validates edits; capture rejects
missing components, deleted/extra actors and changed bindings.
Bind source map/class/path/object-name units and positive GUIDs
to native placements/classes.
Exact current C001 recorders must be fully interpreted; placements bind once.
No variables, extra state, spawning, stitching or nonneutral prospect behavior.
Schema 5: immutable source identities/bytes, mutable poses, count/category output
(F-927).

## Foundation relationships

`--relationships FILE`: `relationship_format = 1`,
`native_catalog_id`, `map`; each `placement` declares positive `identity.guid`,
UTF-16 `identity.name`, `role` (`spatial`, `deployable`, `foundation`). Only
deployables link `foundation`; omission/null unlinks, nonempty must resolve.
Prepare identities, then match GUID/ASCII-folded name; non-ASCII/unpaired
surrogates match exactly. Duplicate/unresolved pairs reject; identities are
explicit. Names: 1–1,024 units, 1,048,576 total including links (F-922).
Assignment follows F-646/F-650/F-663 on server 25621760/client 25793767,
with stricter unresolved-link rejection. Other setter effects/releases remain
unqualified.

Schema 6: immutable declarations, mutable domain-ID edges, optional C001 metadata;
links independent of source. Otherwise schemas 4/5.
`reassign_native_foundation` updates edge/reverse index;
`native_foundation_of`/`native_dependents_of` query. `None` unlinks; no-ops succeed.
Unlinked components are valid. Only edge assignment, without parenting/support/collapse,
destruction propagation, tame permissions or full C009 restoration.

## Owner lifecycle

```bash
cargo run --locked -- create-native-world --catalog authored.toml --map workshop \
  --lifecycle lifecycle.json --restore restore.json --data-dir work/owners --dry-run
```

Lifecycle: catalog/map; each placement declares UTF-16 name,
`spatial_state_v1`, `live`/`absent`, optional positive GUID, spawn component
`missing`/`declared_kind`. Limits: 1–1,024 units/name, 1,048,576 logical/2,162,688
capacity units; 8 MiB lifecycle, 64 MiB restore, 16,384 slots/bindings/commands.
Placement/tile counts also have capacity guards. Schema 7 persists live/dormant
slots/incarnations/bindings (F-923). Only live slots spawn; missing actors/dormant
records reject capture. No C001 metadata; preservation/relationships optional.

`RestoreBatch`: lifecycle ID/sequence, nonempty commands with GUID/kind, placement,
name, policy, source mode, creation permission, expected presence/incarnation,
complete pose/variables. Detached lookup ASCII-folds all names; ambiguity rejects.
Bound lookup uses GUID, requires unchanged state, retains even `respawn_only`.
Detachment supplies expected prior binding/null; attachment may retire incumbent
GUID binding without incarnation change. Foundation identities stay separate.

Detached `find_only` attaches live; `find_or_respawn` attaches live/spawns absent,
rejecting failed attachment; `respawn_only` replaces live/spawns absent.
Replacement keeps domain ID/edited live-endpoint edges, increments incarnation.
Validate all commands before mutation; no forced/manual resolution/undeclared
spawning. Sequence starts at 1, increments, matches `restore_sequence`; exact
latest retries preserve edits, altered/older reject. Reload never replays policies.
In-memory completion is not durable until autosave or shutdown publishes it.

Runtime: `restore_native_owners`, edits, foundation reassignment.
Capture and mutations reject managed-owner ECS hooks, component requirements
and lifecycle observers, including callbacks on disabled or untouched owners
and optional types not yet instantiated (F-928). Unrelated entity observers
remain allowed. Global raw lifecycle-event observers (such as `On<RemoveEvent>`)
also block capture, including autosaves, even when intended for unrelated entities.
Flush pending commands/registrations before these APIs; arbitrary
component `Drop` side effects and activation setup are outside this contract.
`live_actors` counts live; `actors` includes dormant.

## Authored C001 owners

```bash
cargo run --locked -- create-c001-owner-fixture --source authored.json \
  --catalog authored.toml --lifecycle lifecycle.json --bindings owners.json \
  --data-dir work/source-owners --dry-run
```

`c001_authored_owners_v1`: lifecycle ID, one recorder/binding per slot; explicit
placement/class, lookup name, GUID, creation permission/presence/incarnation/prior-binding
expectations. Inert restrictions except `FindOnly`,
`FindOrRespawn` (default), `RespawnOnly`. No policy/pose overrides. Sequence-1
detached restore completes all owners. Independent `--relationships`; no `--restore`.

Schema 8: immutable provenance/completed lifecycle (F-927). Load verifies initial
receipt in temporary world, activates current state. Edits/GUID retirement allowed;
imported slots stay live with recorders, incarnation at least initial. Capture
checks pinned state without source reconstruction; storage/preflight repeats
reconstruction/receipt verification during restore.

## Candidate Bool/I32 assessment

Library-only `native_import::c001_variables::prepare_c001_variables` borrows a
decoded candidate, compiled catalog, layout and bindings (F-929). Pin payload,
catalog/maps, recorder ordinals, exact UTF-16 owner/property identities,
placements/classes and native types. Cover every retained, losslessly typed C001
recorder and both arrays, including exact shared headers for empty arrays.
Property bindings assert `save_game = true`; official schema qualification
and property handlers remain open (F-735, F-897).

Independent array/element-tag/record-type/name-field/value-field names support
multiple Bool/I32 properties and signed I32 extrema. Duplicate/incomplete bindings,
unsupported state, dropped/unavailable/non-C001 records and malformed terminal
records reject all. Limits: 16,384 owners, 64 combined properties/owner,
1,048,576 assignments, 32 MiB charged output/temporary lookup capacity;
candidate byte/element headroom independently constrains preparation (F-920),
including empty output. Charges are not RSS measurements.

Assignments borrow headers and provenance. Debug/serialization redact details;
errors expose only ordinal/category. Every assignment retains
`UnqualifiedPropertyRestoreAndRepNotify`; activation stays false even empty.
No world mutation, callbacks or publication; unrelated roots remain unapproved.
Use-gate behavior below qualifies no handler here.

## Authored scalar owners

```bash
cargo run --locked -- create-c001-scalar-owner-fixture --source authored.json \
  --catalog authored.toml --lifecycle lifecycle.json --contracts scalars.json \
  --bindings scalar-bindings.json --data-dir work/scalar-owners --dry-run
```

`c001_authored_scalar_owners_v1` adds two single-record
arrays with exact UTF-16 array/element-tag/record-type/value-field/source-variable
names. Require `VariableName: NameProperty`, canonical Bool/I32. Unknown fields
(even empty arrays), wrong headers/types, duplicates/incomplete coverage reject all.

Used classes bind `recompute_use_availability_v1`,
positive `authored_max`, exactly `enabled: Bool`, `remaining_uses: I32` in
`0..=authored_max`. Saved pairs replace validated defaults/placement overrides on
attachment/spawn/replacement. `try_use`: `Disabled`/`Exhausted` unchanged, else
decrement once, return `Used`; `set_enabled` sets, `scalar_owner_state` queries.
Distinct in-memory actions, no retry/durable ack. Availability derives from
variables; stage its authored callback before mutation, without RepNotify claims.
Generic edits preserve scalars; explicit restore supplies valid complete pairs.
Schema 9 stores contract/binding IDs/provenance, reloads current availability
without initial replay; relationships compose.

## Authored inventory owners

```bash
cargo run --locked -- create-c001-inventory-owner-fixture --source authored.json \
  --catalog authored.toml --lifecycle lifecycle.json --scalar-contracts scalars.json \
  --inventory-contracts inventories.json --bindings inventory-bindings.json \
  --data-dir work/inventory-owners --dry-run
```

`c001_authored_inventory_owners_v1`: scalars plus authored item/inventory keys,
stack limits/capacities/eligibility. Saved rows select exact bindings, never
packaged meanings. Pin source/catalog/lifecycle/scalar/inventory IDs, nested
array/struct/field names. Exact UTF-16 limits: 1,023 header content units plus
terminator, 1,024 payload units. Every owner/inventory, even empty, required;
unsupported final records reject all.

D01: location `0..=255`, nonempty static row, one `ItemableStack` dynamic property
(index 7), positive bounded count; explicitly empty additional stats, alterations,
living slots, item GUID; owner lookup ID `-1`. Selector 263 cannot alias 7.
Associated inventories/ammo/durability/decay/energy/other state reject. Canonical
types/headers/terminators/widths required.

IDs follow recorder/inventory/item order. Eligible empty allocated slots accept
whole stacks. Rejection: zero slots may force-add one eligible slot; nonzero slots
defer whole stacks in ordered overflow. Saturation rejects. Routing: F-735;
acceptance/growth authored.

`inventory_owner_state`/`inventory_stack`: owned state/location.
Move/split/merge/consume/place-overflow actions work across owners; overflow
destinations select an empty allocated slot or append one eligible next slot.
Handles/global revisions guard writes. Move keeps IDs; split allocates one;
merge/full consumption retires IDs; only consumption reduces totals.

`InventoryRestoreBatchV1`: hash complete owner/inventory state, contract IDs and
revision expectations under one lifecycle sequence/completion. No pose-only
restore. Replacement conserves targeted stack-ID/item/count multisets; bound
inventories must be unchanged. Scalar/pose/foundation edits preserve inventories.

Schema 10 embeds contracts, current contents/allocator/revisions/consumed totals,
lifecycle, immutable initial provenance/source; optional relationships. Reload
never resurrects consumed stacks. Limits: 16,384 owners, 1,024 item definitions,
eight inventories/owner, 65,536 instances/stacks each, 256 slots/overflow entries
per inventory, 512 source items/inventory, counts ≤1,000,000; 16 MiB state,
64 MiB joint graph allowance including decoder growth/conversions, not RSS;
shared caps apply (F-927).
No weight, replication, general D01/C001 restore or official-save compatibility.
