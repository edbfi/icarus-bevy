# Prospect loading: static observations and investigation contract

Status: **observed call paths, reflected inner types, tagged-property dispatch and
by-name missing/truncated-file rejection; complete binary encoding, component
payloads and other runtime behavior unresolved**. Applies
only to dedicated server tool build 25621760. Entry-path evidence IDs belong to
`work/icarus-server-baseline/` and are described in the
[targeted static report](../research/2026-10-04-targeted-static-report.md).

## Keep the two entry paths distinct

| Path | Evidence | Observed stages | Unknowns |
| --- | --- | --- | --- |
| By-name candidate, `0x14146e6c0` | E-009/E-017/E-018 | Empty-name check, file lookup, preliminary load, subsequent load/name comparison, continuation toward map loading | Dedicated command entry, field identities, case rules, complete outcomes |
| From-file candidate, `0x1414baf60` | E-010/E-014/E-015/E-019 | Two reader results gate apparent state application; either failure enters reporting | Caller, reader contracts, schemas, atomicity, dynamic error behavior |

The by-name path constructs names with a `.json` suffix. That alone does not prove
that every save is plain JSON or describe compressed/encoded fields, companion
files, versioning, checksums, references or content requirements. The shared
archive conversion includes indirect calls; its full schema is still unknown.
The subsequent overnight pass recovered a candidate outer JSON envelope and its
decode/decompression/hash chain. See [envelope inspection](prospect-envelope.md)
for the address-backed fields and the bounded read-only inspector. This does not
resolve the inner world archive or implement import.

In the from-file path, both reader candidates receive the same path object and
call the same initial helper. One immediately checks that helper's return; the
other proceeds to a conversion whose return becomes its result. There is no proof
yet that this difference loses an error or permits inconsistent reads.

On failure, the outer path calls a helper that sets a global flag and forwards
to output formatting and a virtual call (E-016/E-020). The overnight follow-up
[resolved its statically initialized output target](../research/2026-10-04-save-failure-output.md):
it can force process termination or raise an exception according to guard state.
This is not an ordinary log-only path. Runtime reachability and final process
outcome remain untested. Cleanup and a false-return path in the caller do not
prove that the reporting call returns.

## Inner state and version dispatch (2026-10-12)

Targeted follow-up in `work/save-schema-20261012/` reverified the preserved executable
hash, inspected metadata and exported forty selected
functions in seven bounded Ghidra batches. No original execution or packaged asset
extraction occurred. These observations belong to **build 25621760**; the user's
supplied save is reported from **25793767**, so cross-build compatibility remains
unvalidated.

The concrete state object has vtable `0x1449e5c88`. Its `+0x10` entry,
`0x1418d7b60`, jumps to `0x141cfd400`, which registers `ProspectSaveState`.
The adjacent header vtable `0x1449e5c70` resolves through `0x1418d7b70` and
`0x141cfd460` to `ProspectSaveStateHeader`. State metadata at `0x145e9cb50`
references the header constructor `0x141cfbc60` as its base; the header descriptor
is `0x145e9cbf0`. The declared in-memory sizes are `0xf8` and `0xe8` respectively.

| Reflected field | In-memory offset | Observed type/role |
| --- | --- | --- |
| `Version` | `0x08` | Integer tested by load-fixup callbacks |
| `LobbyPrivacy` | `0x0c` | Enum with a byte underlying descriptor |
| `ProspectInfo` | `0x10` | Nested prospect metadata structure |
| `ProspectMapName` | `0xb0` | String |
| `LastSavedDateTime` | `0xc0` | Date/time structure |
| `ProspectID` | `0xc8` | Legacy string read by pre-version-2 fixups |
| `ProspectDTKey` | `0xd8` | Legacy string read by pre-version-2 fixups |
| `StateRecorderBlobs` | `0xe8` | Array added by the derived state type |

The array's element metadata identifies `StateRecorderBlob`, a `0x20`-byte
in-memory structure containing `ComponentClassName` (string at offset zero) and
`BinaryData` (byte array at `0x10`). These are **object offsets, not save-file byte
offsets**. Descriptor order is not proof of serialized field order. The component
names and opaque byte arrays define the next decoding boundary; their existence
does not establish actor counts or any component's payload schema.

The by-name continuation is now linked to this same concrete state type:
historical call site `0x14146ef42` enters `0x1414831f0`, which constructs the state
vtable and calls decoded-blob reader `0x1418e6240` at `0x141483466`.
The from-file chain remains separately observed through `0x141823fb0` and
`0x1418e6140`; sharing a state type does not make the callers' failure behavior
equivalent.

The generic archive dispatcher `0x1418ee4a0` obtains the reflected type through
object vtable `+0x10`, then invokes the type's serializer at virtual slot `+0x2a0`.
Construction installs schema vtable `0x144ec5e20`; this slot resolves to
`0x14200ea70`. It selects `0x14200ea90` when archive byte `+0x28` has bit `0x20`
set, otherwise `0x14200f5b0`.

The blob reader's constructor/reset chain `0x141f58f60` → `0x141f5f450` clears
that bit. Its later setters establish loading bit `0x01` and clear bit `0x80`.
The dispatcher's version-use call and structured-archive wrapper leave the mode
unchanged on this path. Thus this loader selects `0x14200f5b0`, which reads tags
through `0x14215f530`, matches property identifiers and calls property readers.
This establishes **tagged-property dispatch for this reader**, not a universal
encoding for every ICARUS archive or recorder body. The type's native-operations
metadata does not select an alternative in this direct virtual-call path.

When archive flag byte `+0x28` has loading bit `0x01` set, the generic dispatcher
invokes object slot `+8`. The state callback at
`0x1418e4250` and header callback at `0x1418e44f0` both branch on stored versions
below 2 and pass legacy strings to nested offsets `+0x10`/`+0x38`, including calls
to `0x14089a1c0` whose exact semantics remain unresolved. The header's second-string
path contains explicit string-copy operations. The state callback has an
additional version-zero path through `0x14170a4c0`, then sets byte `+0x0c` to 1,
releases recorder allocations and clears the recorder count. The helper's complete
migration semantics and the enum meaning of that byte remain unresolved.

The dispatcher also uses a 16-byte key at `0x145dc12c8`. Lookup `0x141f5b5c0`
returns `-1` when that key is absent from the archive's version collection, and
the dispatcher stores the lookup result into object offset `+8` after load fixup.
This establishes version-dependent behavior, not a wire/header layout: it does
not prove that a version table starts at decoded byte zero or that serialized
`Version` equals the game build number. The observed version-use slot `+0x1d8`
returns without reading a table when loading; recovering the actual property
reader is therefore preferable to assuming a custom-version prefix.

The fourth batch recovered [root tag framing](tagged-properties.md), including
archive-version gates, name encoding and payload sizes. Native inspection now
preserves opaque root records under an explicit experimental profile.
The fifth batch resolves nested struct selector `0x14200e610` for this concrete
recorder type. Its native-operations factory `0x141cfbd60` supplies neither a
native serializer nor a post-serialization callback. The loading archive flags and
the structure's flags select tagged traversal through schema slot `+0x2a0`.
Each recorder therefore has its own `None`-terminated property sequence after the
array's single shared inner tag. This does not establish the encoding of arbitrary
nested structures or of the component's opaque data.

The `BinaryData` inner byte property has no enum binding. Reader `0x142158d40`
consumes one byte per element on that branch, so this array's payload is a signed
32-bit count followed by that many raw bytes. The independent bounded private
scan described in the [tag specification](tagged-properties.md) reaches the exact
recorder-array boundary on both selected saves. It does not interpret those bytes.

### Component byte dispatch

The concrete restore entry `0x1414b83c0` resolves
the recorder's class string, constructs a component and invokes virtual slot
`+0x4a0` with the recorder and load context. Failure to resolve or construct the
component returns null through separate diagnostic paths. The decoded-state array
handoff itself is not the component byte serializer.

The base state-recorder component vtable at `0x144a5eb40` resolves `+0x4a0` to
`0x1418a9c80`. This method constructs memory-reader vtable `0x1447a9470` over the
recorder's `BinaryData`, sets loading, clears flag `0x80`, adds flag mask
`0x04008000` at archive offset `+0x28`, then invokes component virtual `+0xa0`.
The base target for that second slot is `0x1418aaf70`. The wrapper shows no exact EOF
check. Neither that absence nor a decompiled class-mismatch branch establishes an
original defect or a supported migration.

The base serializer calls `0x14346c410` → structured wrapper `0x142123530` →
`0x1421235a0` → property selector `0x142123f50`. On this loading path, the selector
calls class-schema slot `+0x2a0`; both observed `UClass` vtables resolve that slot
to `0x14200ea70`, and the clear archive `0x20` bit selects tagged properties.
The concrete archive hooks checked on this path are return-only leaves.

The added `0x04000000` archive flag also affects field eligibility: predicate
`0x14215be10` rejects fields lacking property flag `0x01000000`, with other gates
still applying. Rejected fields are skipped by declared size. Separately, that
archive flag plus a false virtual `+0x200` result enables name/type-remapping
branches. This does not establish how every saved field should be interpreted.

After properties, `0x1421235a0` calls `0x14202f1f0`. With the observed flags it
reads a four-byte boolean; a nonzero value reads four further 32-bit words through
`0x141f1c300`, a 16-byte GUID, before registry handling. Thus property termination
does not mark component EOF. The original's direct-buffer boolean path accepts
nonzero values while its fallback flags values above one; a strict research reader
may accept only zero/one without claiming identical original tolerance.

Finally, the base component serializer invokes virtual `+0x488` when loading an
eligible object. Its base target is a return-only leaf, but derived components may
override this or earlier slots. Verify the concrete saved class's methods before
applying this base layout to its bytes. Component values, reference resolution,
post-load effects and newer-build fidelity remain unresolved. Preserve legacy fields
and version-zero migration requirements; an importer cannot safely flatten them
into the current native snapshot schema.

Case Evidence E-002 records registration/vtables, E-003 the property metadata,
E-004 version callbacks, E-005 the caller chains, E-006 schema construction,
E-007 serializer branches, E-008 archive mode initialization, E-009 tag/name
primitives, E-010 payload/array boundaries, E-011 nested recorder dispatch and
E-012 byte-array element encoding, E-013 recorder restoration, E-014 component
archive dispatch, E-015 base component serialization and E-016 filtering/trailer
contracts. The case report links them to
findings and call paths; strict review verified all artifact hashes with zero
errors/warnings. Primary decompilation, disassembly and exact commands remain private.

### Selected concrete component

Static follow-up E-017 resolves one private singleton class (case-local ID C013)
through registration, construction and its installed vtable. All three relevant
slots inherit the base methods: recorder loading, serialization and the empty
post-load hook. This identifies a candidate layout for that class in build 25621760;
it does not prove identical methods in the user-reported newer build.

The first bounded probe of that class's 548-byte saved body stopped at type-name
offset 281 because the research parser supported only seven header kinds. It
returned no successful framing result and did not scan the backup component body.
Both complete envelopes and recorder arrays had passed before body inspection.
The failure, source fixity and invocation are preserved as save-triage E-020; it is
an unsupported-reader result, not evidence of save corruption or an original defect.

The inherited metadata identifies `NameProperty` as a candidate missing kind.
The existing tag-reader switch maps it to built-in name ID 6 and adds no
type-specific header metadata, followed by the standard property-GUID field.
Its value reader `0x1421580b0` calls archive `+0x140`, the same string/name primitive.
The native opaque header reader now supports this kind; it does not decode values
or apply the component layout to arbitrary saved classes.

The subsequent equality-only discriminator matched `NameProperty` at the recorded
position (save-triage E-021). A new bounded probe with only that header extension
framed both selected 548-byte bodies to exact EOF: five properties, 312 header and
terminator bytes, 232 opaque value bytes and a zero four-byte trailer (E-022).
See the [component framing subset](tagged-properties.md#explicit-base-component-framing).
No property values were interpreted and no other saved classes were scanned.

Metadata evidence E-018 identifies one declared struct field and four immediate-base
fields carrying the necessary save-selection flag: enum, name, string and boolean.
This is not a complete ancestor schema or a required-field list. The declared
struct has one integer child and selects nested tagged serialization despite its
four-byte in-memory size. The enum's loading path also reads an archive name;
its byte-sized underlying type does not imply a one-byte serialized value.

## Controlled reproduction matrix

The overnight case exercised by-name missing-file rejection once and truncated-JSON
rejection twice; the second malformed case retained stateless handshake liveness.
See the [runtime report](../research/2026-10-04-prospect-rejection.md).
These observations establish neither a loaded-world preservation guarantee nor the
distinct from-file path. Remaining scenarios require authorized disposable fixtures;
do not use production saves, package assets, or extracted game data.

| Scenario | Question to resolve | Evidence to retain |
| --- | --- | --- |
| Missing file | Do both paths report failure and preserve current state? | Selected command/path, logs, exit status, before/after state |
| Empty or truncated file | Which reader rejects the file, and does the process continue? | Input hash, truncation point, error and process outcome |
| Malformed field or unsupported version | Is invalid data distinguished from missing data? | Minimized player-created fixture, schema/version, outcome |
| Embedded name differs from requested name | Which comparison fails; does case matter? | Both names, selected path, resulting state |
| File changes during load | Do staged readers consume one coherent generation? | Exact timing, file generations, resulting state; only after reader contracts are known |
| Interrupted save followed by restart | What recovery behavior is provided by the original? | Last acknowledged generation, partial files, restart result |

For every candidate, repeat with an unchanged valid fixture, minimize the failure,
and separate original-server behavior from Proton, missing platform services and
resource constraints. A failed startup in the isolated container is not a substitute
for these prospect-load experiments.

## Independent replacement requirements

The importer must load into staging state, resolve references and validate the
complete supported schema before publishing a playable world. Missing, malformed,
unsupported and inconsistent inputs need distinct actionable errors. Preserve
the source and existing native state on failure. A failed import must never silently
become an empty new world. Content-dependent fields require an explicit mapping
to independently authored data, with unresolved mappings reported as incompatibility.

The native snapshot store already has its own versioned format and durability
tests. It is not an official-save reader. `import-save` intentionally remains
unavailable until field coverage and stock-client validation can demonstrate a
correct migration. Those preventative design choices are not evidence of fixing
an original-server bug.
