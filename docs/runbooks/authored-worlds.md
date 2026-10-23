# Authored native worlds

Creation and editing contracts for independently authored native state.
For running, autosave and recovery, see the [native server runbook](native-server.md).

## Spatial worlds and inert source fixtures

Create an independently authored spatial world, then run it without the source
catalog file:

```bash
cargo run --locked -- create-native-world --catalog config/native-world.example.toml \
  --map workshop --data-dir work/authored-world --dry-run
cargo run --locked -- create-native-world --catalog config/native-world.example.toml \
  --map workshop --data-dir work/authored-world
cargo run --locked -- run --data-dir work/authored-world
```

Preparation validates every placement before opening the destination. Dry runs
create nothing; publication refuses existing or recoverable worlds. Coordinates
are metres, right-handed and Z-up; quaternions are `[x,y,z,w]`. Every transformed
object must fit its tile. `native_runtime::update_native_actor` validates pose
and variable edits before applying them; capture rejects missing components,
deleted or extra actors, changed bindings and invalid values. Topology is fixed.
Bounds describe authored geometry only, without physics/navigation or ICARUS
behavior; `world_imported` and `gameplay_ready` remain false (F-919).

The separate `create-c001-inert-fixture` command accepts only the authored
`C001InertPlacementV1` contract (F-921):

```bash
cargo run --locked -- create-c001-inert-fixture --source authored.json \
  --catalog authored.toml --bindings bindings.json --data-dir work/inert-world --dry-run
```

Omit `--dry-run` to publish a new world. The strict JSON/TOML binding DTO is
`native_import::C001InertBindingsV1`: format 1, payload SHA-256, native catalog
identity, exact source-map UTF-16 units, native map and complete owner bindings.
Each owner binds source class/path/object-name units and positive source GUID
to a native placement/class. Coordinates must explicitly declare
`right_handed_z_up_meters_quat_xyzw_world_space`; the fixture author supplies
those coordinates, with no original-coordinate conversion inferred.

Every recorder must be exact C001, current-version, fully interpreted and
supported; every selected-map placement must bind once. Only inert spatial
classes without variables are accepted. Additional state, owner spawning,
stitching and nonneutral prospect behavior reject the whole input. Schema 5
retains accepted source identities, metadata and unchanged source sidecar
alongside mutable native poses. Normal `run` preserves that contract through
capture and autosave. Diagnostics contain counts and static categories only.
This proves an authored contract; ordinary `import-save` still refuses writing,
and both readiness flags remain false (F-921).

## Foundation relationships

The authored relationship lifecycle is qualified by F-922.

Both creation commands accept `--relationships FILE`. Supply a strict JSON/TOML
`native_relationships::NativeRelationshipsDefinition` with `relationship_format = 1`,
the compiled catalog's `native_catalog_id`, selected `map` and exactly one actor
declaration per placement. Each declaration contains:

- `placement`: the native placement key.
- `identity`: a positive `guid` and a nonempty `name` array of exact UTF-16 units.
- `role`: `spatial`, `deployable` or `foundation`, independently declared.
- `foundation`: optional initial identity pair, admitted only on a deployable.
  Omission or null means unlinked. Nonempty links must resolve to a foundation.

All identities are prepared before links resolve. Matching requires both GUID
and name, folding ASCII letters only; other UTF-16 units compare exactly,
including unpaired surrogates. Duplicate identity pairs reject the whole
declaration. Native class keys and source recorder/object names never supply
implicit link identities. Names are bounded to 1,024 units each and 1,048,576
units in aggregate, including initial links; declarations are limited to 4 MiB.

This reuses the assignment evaluator qualified in F-646, F-650 and F-663 for
server 25621760/client 25793767. The authored admission rule is deliberately
stricter: an unresolved nonempty link rejects the complete world. It does not
qualify later releases or every consequence of the original foundation setter.

```bash
cargo run --locked -- create-native-world --catalog authored.toml --map workshop \
  --relationships relationships.json --data-dir work/linked-world --dry-run
```

Omit `--dry-run` to publish. Complete preparation and bounded stored encoding
checks precede destination access. Schema 6 embeds immutable declarations and
their identity, with mutable domain-ID edges. Normal `run` loads without either
input file and preserves edited edges through autosave and recovery. Optional
C001 metadata and its required unchanged source sidecar remain provenance; they
do not establish that these authored relationships came from the source.

`native_runtime::reassign_native_foundation` validates the entire current world
and proposed target before changing an edge and its derived reverse index.
`native_foundation_of` and `native_dependents_of` expose the relationship.
Reassignment to `None` unlinks; no-op edits succeed. Capture, edits and queries
reject damaged actors, changed identities, missing or replaced activation
contexts and inconsistent reverse indices. A present unlinked component is
valid; a missing required component is corruption.

These links do not parent transforms, implement structural support, collapse,
destruction propagation or tame permissions. Full C009 import also requires
inherited item state and owner lifecycle behavior; no such adapter is admitted.
Schemas 4/5 retain their contracts when `--relationships` is absent. Ordinary
`import-save` remains closed and both readiness flags remain false.
