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
hash, inspected metadata and exported twenty selected
functions in three bounded Ghidra batches. No original execution or packaged asset
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

Next: recover the tag reader's name/type/size framing and nested component blob
boundaries, then compare
bounded private copies without discarding unknown fields. Preserve legacy fields
and version-zero migration requirements; an importer cannot safely flatten them
into the current native snapshot schema.

Case Evidence E-002 records registration/vtables, E-003 the property metadata,
E-004 version callbacks, E-005 the caller chains, E-006 schema construction,
E-007 serializer branches and E-008 archive mode initialization. The case report links them to
findings and call paths; strict review verified all artifact hashes with zero
errors/warnings. Primary decompilation, disassembly and exact commands remain private.

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
