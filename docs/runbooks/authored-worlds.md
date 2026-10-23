# Authored native worlds

Creation and editing contracts for independently authored native state.
For running, autosave and recovery, see the [native server runbook](native-server.md).

## Spatial worlds and inert source fixtures

```bash
cargo run --locked -- create-native-world --catalog config/native-world.example.toml \
  --map workshop --data-dir work/authored-world
cargo run --locked -- run --data-dir work/authored-world
```

Every creation command preflights before destination access. Omit `--dry-run` to
publish; dry runs create nothing, existing/recoverable worlds reject. Both
`world_imported` and `gameplay_ready` remain false. Normal `run` needs no source
files after creation. Coordinates are metres, right-handed and Z-up; quaternions
are `[x,y,z,w]`. Transformed objects must fit their tiles. Bounds describe authored
geometry, without physics/navigation or ICARUS behavior (F-919).

Schema 4 has fixed topology. `native_runtime::update_native_actor` validates pose
and variable edits before applying them. Capture rejects missing components,
deleted/extra actors, changed bindings and invalid values.

```bash
cargo run --locked -- create-c001-inert-fixture --source authored.json \
  --catalog authored.toml --bindings bindings.json --data-dir work/inert-world --dry-run
```

The strict JSON/TOML `native_import::C001InertBindingsV1` format 1 pins payload
SHA-256, native catalog identity, exact source-map UTF-16 units, native map and
complete owner bindings. Each owner binds source class/path/object-name units
and a positive GUID to a native placement/class. Declare coordinates explicitly
as `right_handed_z_up_meters_quat_xyzw_world_space`; no original conversion is
inferred.

Every recorder must be exact, current-version C001 and fully interpreted; every
placement binds once. Only inert classes without variables are admitted. Extra
state, spawning, stitching or nonneutral prospect behavior rejects the whole
candidate. Schema 5 preserves immutable source identities and bytes alongside
mutable poses; diagnostics expose counts and static categories (F-925).
Ordinary `import-save` remains closed.

## Foundation relationships

Creation commands accept `--relationships FILE`: a strict JSON/TOML
`native_relationships::NativeRelationshipsDefinition` with `relationship_format = 1`,
`native_catalog_id`, selected `map` and one actor declaration per placement:

- `placement`: native key.
- `identity`: positive `guid` and nonempty exact UTF-16 `name` units.
- `role`: independently declared `spatial`, `deployable` or `foundation`.
- `foundation`: optional initial identity pair, only on deployables. Omission or
  null means unlinked; nonempty links must resolve to a foundation.

Prepare all identities before resolving links. Match GUID plus ASCII-folded
name; non-ASCII units, including unpaired surrogates, compare exactly. Duplicate
identity pairs reject. Source names and native class keys never supply implicit
link identities. Names have at most 1,024 units each and 1,048,576 in aggregate,
including initial links; declarations are bounded to 4 MiB.

F-922 uses F-646/F-650/F-663's assignment rule with stricter unresolved-link
rejection. Other setter effects and releases remain unqualified.

Schema 6 embeds immutable declarations and mutable domain-ID edges. Optional
C001 metadata requires its unchanged sidecar; authored links need not come from
the source. Without this option, schemas 4/5 retain their contracts.

`native_runtime::reassign_native_foundation` validates the whole world before
changing an edge and its reverse index. `native_foundation_of` and
`native_dependents_of` query it. `None` unlinks; valid no-ops succeed. Capture,
edits and queries reject damaged actors, changed identities/contexts and
inconsistent indices. A present unlinked component is valid; a missing required
component is corruption. Parenting, support/collapse, destruction propagation,
tame permissions and full C009 restoration remain unsupported.

## Owner lifecycle

```bash
cargo run --locked -- create-native-world --catalog authored.toml --map workshop \
  --lifecycle lifecycle.json --restore restore.json --data-dir work/owners --dry-run
```

`native_lifecycle::LifecycleDefinition` declares format 1, catalog identity, map
and every placement's exact UTF-16 owner name, `spatial_state_v1` recorder kind,
initial `live`/`absent` presence, optional positive-GUID component and
`missing`/`declared_kind` spawn component. Names have 1–1,024 units each and
1,048,576 logical units total; a separate 2,162,688-unit capacity guard permits
decoder growth. Placement/tile totals also separate counts from capacities.
Limits: 4 MiB definition, 8 MiB lifecycle section, 64 MiB restore input and
16,384 slots, bindings or commands. The complete generation cap still applies.

Schema 7 persists complete live/dormant slots, incarnations and recorder
bindings (F-923). Only live slots spawn entities; missing live actors or dormant
records reject capture. C001 metadata is forbidden; opaque preservation and
authored relationships are optional.

`RestoreBatch` supplies lifecycle identity, sequence and nonempty commands with
GUID/kind, target placement, owner name, policy, source mode, creation permission,
expected presence/incarnation and complete pose/variables. Detached lookup
compares names with ASCII-only folding across all declarations and rejects
ambiguity. Bound lookup uses recorder GUID, requires unchanged supplied state
and keeps the owner even for `respawn_only`. Explicit detachment supplies the
incoming GUID's expected prior binding, or null if unbound. Attachment may
retire a different incumbent GUID's binding without changing incarnation.
Lifecycle and foundation identities remain separate.

For detached commands, `find_only` requires a live owner; `find_or_respawn`
attaches live or spawns absent, rejecting failed live attachment; `respawn_only`
replaces live or spawns absent. Replacement keeps domain ID and increments
incarnation. Every command validates before any changes. Manual/forced resolution
and undeclared spawning are unsupported.

Sequences start at 1 and increment by one. Exact latest-request retries return
stored completion without resetting later edits; altered/older requests reject.
`restore_sequence` must agree with completion. Reload activates saved results
without executing policies. In-memory completion is not durable until autosave
or shutdown publishes it.

`native_lifecycle_runtime` exposes `restore_native_owners`, guarded pose/state
edits and foundation reassignment. Handles include domain ID and incarnation;
stale handles reject. Replacement preserves edited edges with live endpoints.
Capture and no-ops reject missing components, stale resource generations,
replaced contexts and inconsistent indices. `live_actors` counts materialized
owners; `actors` includes dormant slots. Helpers validate the whole world and
mutations preflight storage. Tick-loop cost and official lookup/spawn behavior
remain unqualified.

## Authored C001 owners

```bash
cargo run --locked -- create-c001-owner-fixture --source authored.json \
  --catalog authored.toml --lifecycle lifecycle.json --bindings owners.json \
  --data-dir work/source-owners --dry-run
```

`native_c001_owners::C001OwnerBindingsV1` format 1 uses adapter
`c001_authored_owners_v1`. It pins payload/catalog/lifecycle identities, maps and
the explicit authored coordinate declaration. Every recorder, binding and slot
corresponds once. Bindings supply exact source identities plus native
placement/class, owner name, recorder GUID, creation permission, expected
presence/incarnation and incoming-GUID prior binding. Native identities are not
inferred from source names.

The inert source/root restrictions remain, except policy may be `FindOnly`,
`FindOrRespawn` or `RespawnOnly`; omission uses qualified C001 `FindOrRespawn`.
Bindings cannot override policy or pose. The sequence-1 detached restore must
complete every attachment/spawn. Optional `--relationships` supplies authored
links independently. There is no `--restore` override. Bindings and metadata each
have a 4 MiB cap; complete generations remain bounded to 64 MiB plus 4 KiB.

Schema 8 requires immutable source metadata, unchanged sidecar and completed
lifecycle state (F-925). Load verifies the initial receipt against a temporary
authored world, then activates saved state. Later edits and GUID retirement are
valid: provenance is separate from current poses/bindings. Imported slots stay
live with a recorder component and cannot regress below initial incarnation.
Capture validates pinned current state without source reconstruction. Storage
validation/preflight reconstruct source and verify the receipt, repeatedly during
restore preparation. Autosave/edit/restore cost therefore depends on source and
world size; capture's shortcut does not qualify those paths for tick-loop use.

## Authored scalar owners

```bash
cargo run --locked -- create-c001-scalar-owner-fixture --source authored.json \
  --catalog authored.toml --lifecycle lifecycle.json --contracts scalars.json \
  --bindings scalar-bindings.json --data-dir work/scalar-owners --dry-run
```

The separate `c001_authored_scalar_owners_v1` adapter admits the owner contract
above plus two explicitly authored arrays, each containing one property record.
`native_c001_scalar_owners::C001ScalarOwnerBindingsV1` declares exact UTF-16 array,
element-tag, record-type, value-field and source-variable names. Each record has
exact `VariableName: NameProperty` plus canonical Bool or I32 value. Both saved
properties are required. Unknown fields, even empty arrays, wrong headers/types,
duplicates and incomplete owner coverage reject the whole candidate. These
layout declarations are not claims about official variable schemas.

`native_scalar_owners::ScalarOwnerContractsV1` binds every used native class to
`recompute_use_availability_v1` and positive `authored_max`. Classes have exactly
`enabled: Bool` and `remaining_uses: I32`; counts must be within `0..=authored_max`.
Defaults and placement overrides are authored and validated, then fully replaced
by saved values during initial attachment, spawn or replacement.

`native_lifecycle_runtime::try_use` returns `Disabled` or `Exhausted` without a
mutation, otherwise decrements once and returns `Used`. `set_enabled` changes the
switch; `scalar_owner_state` queries it. All require current owner handles and
valid complete worlds. Each invocation is a distinct in-memory action, not a
network retry protocol or durable acknowledgement. Availability is derived from
the authoritative variables; missing/stale/transplanted derived state rejects
capture. The callback is authored behavior, not original RepNotify.

Generic actor edits allow pose changes only when scalar values are unchanged.
Explicit owner restore can supply a new complete valid pair; exact restore
retries preserve subsequent consumption. Replacement invalidates old handles.
All validation, callback staging and storage preflight precede live mutation.
Schema 9 embeds contract and binding identities, source provenance and the
unchanged sidecar. Reload restores current values and recomputes availability
without reapplying saved initial values. Optional authored relationships compose;
schemas 1–8 and their old adapters retain their acceptance rules.

Contract, bindings and metadata each have a 4 MiB bound. Existing name, owner and
aggregate generation limits apply; source-size validation costs remain as above.
No official property resolution, callback compatibility, networking or performance
is established by this authored consumer.
