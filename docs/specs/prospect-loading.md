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
hash, inspected metadata and exported 44 selected
functions in nine bounded Ghidra batches. No original execution or packaged asset
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
contracts. Later E-017 through E-024 cover selected-class dispatch, metadata,
capture and collection cleanup, as described below. The case report links them to
findings and call paths; strict review verified all artifact hashes with zero
errors/warnings. Primary decompilation, disassembly and exact commands remain private.

### Selected singleton component (C013)

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
That probe interpreted no property values and scanned no other saved classes.

Metadata evidence E-018 identifies one declared struct field and four immediate-base
fields carrying the necessary save-selection flag: enum, name, string and boolean.
These descriptors alone are not a required-field list. The declared
struct has one integer child and selects nested tagged serialization despite its
four-byte in-memory size. The enum's loading path also reads an archive name;
its byte-sized underlying type does not imply a one-byte serialized value.

E-019 follows the remaining native superclass descriptors through the root's
self-parent-to-null conversion. No further descriptor carries the selection bit:
five positive entries occur among 31 total entries, which include inner-property
helpers. Inspected type/link flag additions preserve that conclusion, but later
runtime mutation was not exhaustively audited. The private exact comparison map
then matches the two selected newer-save records' five headers, nested integer tag,
three string encodings and empty boolean body (save-triage E-024). Neither static
ancestry nor this sample agreement establishes mandatory field presence or world
reconstruction semantics.

The eighth batch (static E-020/E-021) identifies a concrete role for the selected
struct: callback `0x1418a1850` copies four bytes from a validated owner's `+0x250`
into recorder `+0xd8`. Related callback `0x1418ad0c0` uses owner `+0x248` as an array
pointer and `+0x250` as its count, iterating eight-byte elements. The supported
interpretation is a captured owner-collection count; the collection's gameplay
identity and the saved scalar values remain unknown.

The ninth batch and aligned callsite follow-up (static E-022/E-023) place capture
before blob serialization on a conditional path. Caller `0x14189aa00` invokes
`0x1418a3d90` only when owner validation passes and a caller-supplied byte is
nonzero. That method refreshes inherited recorder fields and invokes capture slot
`+0x448`; the caller then serializes through `0x1418aafb0`. Capture is therefore
not established as mandatory for every save operation.

Caller `0x1414bf530` invokes the related callback only for a live returned recorder,
state byte `+0xb2 == 3`, and a successful virtual `+0x470` gate. That callback builds
two pointer sets and calls helper `0x141899760`, now resolved as set intersection.
The caller discards that result without applying its contents or count.

The callback does have a preceding mutation: helper `0x14148a160` compacts the
owner's pointer array in place, preserving retained order and removing null entries
or entries with the inspected non-live object flag. It uses `memmove`, updates the
array count and returns the number removed (static E-024; no additional Ghidra
export). Neither this cleanup nor the discarded intersection reads recorder
`+0xd8` or restores the collection to its saved count. The supported observations
are conditional count capture and pointer-collection cleanup; relationship
reconstruction and the broader lifecycle meaning remain unresolved.

### Selected repeated component (C028)

The separate static case `work/component-c028-20261012/` matches the private C028
package/class exactly and traces registration `0x1406ff5d0` through getter
`0x141d2d770`, constructor callback `0x141d2e710` and instance constructor
`0x1418c22f0` to vtable `0x144a70e90`. Its load and post-load methods differ from
C013, so identical restoration must not be inferred from a shared serializer.

Load slot `+0x4a0` targets `0x1418a9bb0`, which calls base `0x1418a9c80` first with
the component and recorder arguments unchanged. After the base reader is destroyed,
it conditionally passes the retained load context to virtual `+0x4b0`. Serialize
slot `+0xa0` remains base `0x1418aaf70`. Post-load slot `+0x488` targets
`0x14189d380`: when a component integer is below one, it traverses nested in-memory
arrays and conditionally replaces name fields. This complete override never reads
or preserves the incoming archive argument. These paths support a base-framing
hypothesis while leaving migration and context-restoration work unimplemented.

Metadata E-004/E-005 in that case closes the native ancestor chain: 3 entries in
C028, 27 in its immediate parent, 11 in the recorder base, 19 in the next ancestor
and none in the root. Of these 60 entries, 23 top-level descriptors carry the
save-selection bit: three integers, three names, two booleans, six structs, seven
arrays, one enum and one string. Seven preceding struct descriptors are array-inner
helpers, not additional top-level fields. Exact private names, index-zero candidates
and struct/enum/array header extras are available for comparison; field order,
mandatory presence and arbitrary later runtime mutation remain unproved.

Independent and release-native scans of four selected C028 records—the first
minimum- and maximum-sized record in each copy—consume exactly 3,325 or 3,358 bytes.
Each has 23 properties, 1,509 header/terminator bytes, 1,812 or 1,845 opaque value
bytes and a zero four-byte trailer. A separate private comparison matches all 23
header descriptors without assuming order (save-triage E-025/E-026/E-027). This
qualifies those four records through both readers.

A subsequent independent scan covers all 1,294 C028 records in the two supplied
copies and matches the same exact 23-header metadata (save-triage E-028). Each copy
contains 647 components and 14,881 fields: 976,323 header/terminator bytes,
1,184,111 opaque value bytes and 2,588 zero-trailer bytes, totaling 2,163,022 bytes.
The scan validates each complete outer payload before reading selected bodies.
This extends observed framing and header agreement to every C028 record in these
two copies. Release-native batch qualification independently matches the combined
totals for all 1,294 bodies: 29,762 fields and 4,326,044 bytes, divided into 1,952,646
header/terminator bytes, 2,368,222 opaque value bytes and 5,176 zero-trailer bytes
(save-triage E-029). It establishes neither mandatory fields for other saves nor
gameplay correctness.

The follow-up `work/c028-structs-20261012/` resolves all six direct struct types to
tagged serialization under the established older-build `MemoryArchive522` path.
They are six distinct types using four operations tables. One initially missing
ops factory is supplied by name-matched deferred registration `0x140729c20`; its
capability checks, like the other tables, do not select native or post-serialization
hooks on this path. This supplies a candidate nested framing contract, not a fixed
disk width. Both full scans leave all property payloads opaque.

Immediate child metadata adds 16 optional descriptors across those six structs
(3/7/1/1/1/3): three structs, one name, four signed integers, seven booleans and one
unsigned 32-bit integer. Their descriptors carry the save-selection bit; the
struct-property path preserves the archive context through the child predicate.
This does not establish mandatory presence, field order or value semantics.

The unsigned field's generation kind was initially mistaken for a float. Tracing
the actual factory and virtual type-name getter corrects that hypothesis:
`0x14216eba6` installs vtable `0x144eebf50`; virtual `+0x100` resolves to
`0x14214a630`, returning the class name token. Constructor `0x142017040` strips the
leading UTF-16 `F` from the class literal before interning `UInt32Property`.
Built-in name ID 25 independently agrees, and the tag reader adds no type-specific
metadata before its common GUID flag. The native reader now supports this header
while retaining opaque values. Evidence: static E-005 through E-008 in
`work/c028-structs-20261012/`.

The three struct children of the first tagged struct resolve to two distinct
types whose flags select binary serialization. Static follow-up in
`work/c028-binary-children-20261012/` derives their successful transfer lengths
from instructions: N01 uses four ordered four-byte property transfers, totaling
16 bytes; N02 uses three explicit four-byte transfers, totaling 12 bytes. The
selected archive performs raw little-endian copies. Neither path adds a count,
terminator or post-serialization tail, but an original read error can stop position
advancement and neither wrapper establishes exact EOF. Any replacement validator
must enforce those lengths separately. Values and mathematical meaning remain
uninterpreted; the observed parent's aggregate length is not the static proof.

### Seven selected array contracts

The private case `work/c028-arrays-20261012/` resolves seven distinct element
types and operations tables. Each selects tagged serialization under the same
older archive profile: its initial flags and concrete capability probes select
neither a forced binary path nor native/post-serialization hooks.

Array serializer `0x1421580d0` reads a signed 32-bit count, then one shared struct
tag before the element loop, including when the count is zero. It invokes the
inner struct serializer once per element. Each element is a variable-length,
`None`-terminated property sequence without an object trailer. The shared tag's
declared size spans the complete element sequence, not one element or a multiple
of the in-memory stride.

The tag constructor supplies index zero, the inner property name and the element
struct name; all seven inner names equal their parent field names in these
descriptors. The original loader does not separately enforce index/name/type
equality, although it does handle struct name/GUID compatibility and may skip an
incompatible declared span. Exact name/index/type checks in a strict subset
must be identified as comparison policy.

The follow-up `work/c028-array-children-20261012/` resolves 17 optional child
fields across these types: seven names, three signed integers, three floats, one
boolean, one text, one array and one struct. One additional raw descriptor belongs
to the nested array's inner property. The float and text factories, class-name
getters and built-in IDs independently identify their serialized header types;
both use only the common property-GUID extension. The current eleven-kind native
header reader supports both types. Later static descendant work resolves the
nested array target D01 to eight optional tagged fields and the distinct Struct
target D02 to its own four-four-byte native transfer demand; these are not inferred
from the earlier 16/12-byte types. D01's four array children now have concrete
tagged registrations and immediate schemas of two Int; Int/Str/Int; one Str; and
Int/Str, respectively. These eight fields remain optional. Int demands four
opaque bytes under the reviewed profile. A subsequent exact StrProperty adapter
trace links to the existing FString contract, with original normalization broader
than the native strict subset; see `work/str-value-contract-20261012/`.
Independent semantic and strict review pass in
`work/c028-d01-array-children-20261012/`; no supplied populated descendants or
numeric/string values were read.

Independent inspection of all seven arrays in each of the four selected parent
records finds 28 empty arrays (save-triage E-035). All include their required
shared headers: 112 count bytes plus 2,512 shared-header bytes, with exact EOF
and no element payload. These samples provide no populated-element coverage.
The follow-up metadata inventory covers all 9,058 arrays in the 1,294 records of
this class: every count and element span is zero (E-036). The 848,864 array bytes
comprise 36,232 count bytes and 812,632 shared-header bytes. Both outer archives
and all parent header sets passed before this inventory; no element headers were
scanned and no populated example was found in either copy.
A bounded release-native batch matches the original 28 selected empty arrays and
their exact byte totals with unchanged inputs (E-037).
Newer-build method equivalence and complete world restoration remain unresolved.

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

## Third selected class: bounded top-header comparison

The older C034 registration and complete relevant vtable path resolve to the
reviewed base byte-loading/serialization path and the same postload target as
C028. Its 21 optional descriptors comprise seven Struct, two Int, seven Array,
two Name, one Enum, one Str and one Bool property. Private static bindings are
retained in `work/component-c034-20261012/`.

After validating both complete outer archives, one reviewed probe selected the
first minimum and maximum body per copy: four bodies totaling 71,388 bytes.
Each minimum is 3,830 bytes and maximum 31,864 bytes. Each matches all 21 exact
header descriptors, has 1,451 header/terminator bytes and ends with a zero
four-byte trailer at exact EOF. Opaque payload totals are 2,375 and 30,409 bytes.
The strict presence comparison applies only to these samples; it does not make
the static descriptors mandatory in every valid save.

Evidence E-038 and strict case review 30 preserve unchanged input hashes and
the reviewed session, scope, scripts and static bindings under
`work/save-triage-20261012/component-c034-headers/`. Nested payloads, GUID values
and Bool header values were not compared. Equal framing totals do not establish
equal state, newer-build semantics, native-reader agreement or import fidelity.

A separately scoped immediate own-struct scan revalidates both archives and all
four parent results before selecting its spans. Each selected minimum owns a
572-byte stream and maximum a 28,606-byte stream. Each has eight headers and 449
header/terminator bytes; all four conserve 58,356 bytes and end at exact EOF.
The observed kinds are Array, Bool, three Int, Struct, Float and Name, matching
the eight optional static descriptors. No descriptors are missing here; the
comparison permits absence rather than declaring universal mandatory fields.
The native struct and array child payloads remain opaque, including their
widths/counts. Evidence E-039 and strict review 31 pass with unchanged inputs.
A fresh helper linked to the current eleven-kind Rust library now independently
matches all 32 field kinds, exact header/value spans and four terminator ranges.
The 58,356 bytes comprise 1,796 header/terminator bytes and 56,560 opaque bytes;
all four streams end exactly. Six authored helper tests cover limits, GUIDs,
truncation, unsupported headers and no partial/private output. The fresh scope,
reviewed session, source/library/helper snapshots, unchanged input hashes and
strict case review are retained in `work/native-c034-own-20261012/`.
No production code changed; Float values remain uninterpreted.

A later full-population qualification uses separate bounded per-copy native
batches. All 829 exact-class records per copy pass optional-schema validation and
exact native/independent header, payload, terminator and trailer span comparison.
Per copy, 4,199,203 bytes comprise 1,202,879 header/terminator bytes, 2,993,008
opaque payload bytes and 3,316 trailer bytes across 17,409 fields. All 21 optional
descriptors appear in these records, without becoming universally required.
Both copies retain their distinct source hashes; equal dimensions do not establish
equal values. This scan does not enter own or deeper payloads. Five authored
helper tests, independent harness review and unchanged bindings support the result:
`work/c034-all-headers-20261012/`.

A subsequent own-struct population comparison first reproduces every parent
span, then independently checks all 829 own structs per copy against the current
native reader. Per copy, 1,479,089 bytes contain 6,632 fields, 372,221
header/terminator bytes and 1,106,868 opaque value bytes. All eight optional
descriptors appear, without a mandatory-field claim. Own sizes range from 572 to
28,606 bytes. No inner array or numeric value is parsed in this step. Exact EOF,
source fixity, six helper tests and independent harness review pass; evidence:
`work/c034-all-own-headers-20261012/`.

The separate older-build own-array contract resolves child `0x145ee1220` through
its exact getter and operations table to tagged serialization. Its two immediate
optional descriptors are Struct and Float; the Struct getter resolves prior N02.
Under the reviewed MemoryArchive522 path, the array reads a signed count, one
shared Struct header (also for count zero), then count terminated tagged elements.
Shared size covers the aggregate element span. The 16-byte memory size is not an
on-disk stride. Exact names remain in the private comparison map; no fixed GUID,
required-field or Float-value rule is inferred. Contract and independent review:
`work/c034-own-array-continuation-20261012/`.

The completed fresh comparison enters only the four selected array bodies after
reproducing the complete prior parent/own results. Independent and native readers
agree on each shared-header, element, field and terminator span:

| Per selected array | Minimum | Maximum |
| --- | ---: | ---: |
| Body bytes | 86 | 28,120 |
| Elements | 0 | 214 |
| Immediate fields | 0 | 428 |
| Count/shared header bytes | 86 | 86 |
| Element headers/terminators | 0 | 24,610 |
| Opaque values | 0 | 3,424 |

Both copies have these dimensions; equal framing does not imply equal values.
Across four arrays, 428 elements and 856 fields conserve 56,412 bytes and reach
exact EOF. Every observed element has both optional Struct/Float descriptors;
none is made universally mandatory. That comparison leaves deeper widths and
values opaque.
Five authored helper tests pass; exact source/library/helper snapshots, the
explicit SHA-2-only build transition, fresh session, unchanged input hashes and
strict evidence review remain under `work/c034-own-array-compare-20261012/`.

A subsequent static review resolves the exact Float descriptor to a successful
four-byte transfer and its N02 Struct sibling to three four-byte transfers. A
separate guarded check uses only the frozen sanitized span summary: all 428 Struct
payloads are individually 12 bytes and all 428 Float payloads are individually
four bytes. This is not an inference from aggregate byte totals. No numeric values
were read, and no private payload file was reopened. Native callback success does
not certify archive success; an earlier transfer may advance before a later one
fails. Original global EOF policy, numeric representation and newer-build
equivalence remain unproved. Contract, independent review and strict result review:
`work/c034-value-widths-20261012/`.

Full own-array population qualification subsequently covers all 829 arrays per
copy, with exact native/independent spans and unchanged source bindings. Each
copy has 7,671 elements, 15,342 fields and 85 empty arrays; the largest observed
count is 214. The 1,076,195 bytes partition into 71,294 count/shared-header bytes,
882,165 element-header/terminator bytes and 122,736 opaque value bytes. Every
present Struct span is individually 12 bytes and every Float span four bytes,
with zero mismatches; no optional descriptor is missing in these records.
These are observations, not mandatory fields or universal count limits. Both
complete top/own populations are reproduced before array entry, and width checks
follow exact native agreement. Six helper tests pass, including under the
512 MiB native address-space limit. Evidence: `work/c034-all-own-arrays-20261012/`.

A separate inherited-array qualification joins all seven top-level Array
descriptors to their exact shared ancestor contracts. All 5,803 arrays per copy
are empty, with 543,824 count/shared-header bytes and zero element bytes. Every
family contributes 829 arrays per copy. Independent and native shared spans
agree exactly after reproducing all prior parent spans; inputs remain unchanged.
This result does not qualify populated inherited descendants. Six helper tests
and independent harness review pass: `work/c034-inherited-arrays-20261012/`.

The six inherited Struct families are separately joined to their exact child
schemas and concrete tagged operations, including S01's deferred registration.
All 4,974 bodies per copy match native/independent immediate spans and EOF, with
13,264 fields, 695,531 header/terminator bytes and 58,174 opaque value bytes.
Each family contributes 829 bodies; no optional descriptor is missing in these
records. Nested native payloads remain opaque. The sanitized span output omits
field identities, so it cannot alone assign distinct native width contracts by
order or size. Six helper tests and independent review support the comparison:
`work/c034-inherited-structs-20261012/`.

A subsequent bounded header replay supplies exact descriptor identities for
8,290 selected spans while reproducing all historical native top, own and
inherited spans. Each of S01.F01/F02/F03 and OS01.F06/F07 occurs 1,658 times;
their individual lengths agree with the respective 16/12/12/12/four-byte static
transfer demands. Selection uses exact metadata, never position or observed
size. No optional field is missing in this population. Numeric values and
original payload EOF remain unqualified: `work/c034-field-widths-20261012/`.

Matched-client generic tag/array functions now independently agree with the
examined older version gates and shared-header branch. A subsequent concrete
registration trace supports conditional C034 own-array tagged dispatch and a
version-522 loading archive at the restore-to-Serialize handoff. Downstream
profile propagation now reaches conditional ordinary-class dispatch in a bounded
follow-up, but later field callbacks and supplied-save provenance remain unresolved;
see [matched-client framing comparison](tagged-properties.md#matched-client-static-comparison).

## C009 top-header comparison

The exact older C009 registration, constructor and primary vtable reach the
previously reviewed recorder targets. Its exact ancestry contributes 31 optional
top fields: two own, nine immediate-parent and 20 common-ancestor fields.
Enum and Struct header identities are separately resolved; nested contents are
not inferred from them. Static evidence: `work/component-c009-20261012/`.

All 243 current and 243 backup bodies match this schema and independent/native
source ranges, including the zero trailer and exact comparator EOF. The observed
maximum is 71,956 bytes, so this comparison uses that explicit byte cap through
the bounded API; the default remains 64 KiB. Both copies contain 7,533 fields,
514,674 header/terminator bytes and 972 trailer bytes each. Current opaque values
total 2,877,588 bytes and backup values 2,872,878, giving 6,781,758 total body
bytes. Every optional descriptor appears in this population.

The release helper preserves borrowed spans and passes three authored tests,
including exact bounds, truncation and atomic failure. Independent review,
native/JS span equality and unchanged input hashes support the result:
`work/native-c009-top-20261012/`. That run qualified top framing only, leaving
child dispatch and values to subsequent work. Original load success and
newer-build equivalence remain open.

A subsequent static case resolves both own child schemas as conditionally
tagged: S02 has optional `ArrayProperty<IntProperty>` and Bool fields; S01 has
optional Str and Int fields. The ordinary non-Struct array path has a four-byte
count and four-byte element transfers, with no shared Struct header. This demand
is conditional on the examined archive mode and successful nonnegative-count
execution; original negative-count rejection and enclosing EOF are not proved.
Tagged Bool takes its value from the header and demands zero payload bytes.
The Int and Str targets join the existing scalar/FString contracts. These static
results alone do not qualify child populations: `work/c009-own-children-20261012/`.

The subsequent header comparison qualifies all 972 own child bodies across both
copies (134,182 bytes, 1,944 fields), after reproducing the complete parent result.
Each copy has 243 S02 bodies totaling 30,626 bytes and 243 S01 bodies totaling
36,465 bytes. No optional field is missing. The S02 array payloads total 980 bytes
per copy, with individual sizes from four to 12 bytes; count and element bytes
were left opaque in that header-only run. Exact native/independent child spans,
unchanged inputs and two
authored helper tests support the result: `work/native-c009-own-20261012/`.

Separate leaf comparisons then qualify all 486 exact S02 Int arrays, including
four opaque elements, and all 486 S01 Str values (14,124 bytes). Native and
independent string structure/consumption agree under the strict codec policy.
All 486 S01 Int spans individually have four bytes; all 486 S02 Bool spans have
zero payload bytes. Each run first reproduces the frozen parent/child spans and
joins exact descriptors. Array elements, string meanings and numerical values
remain uninterpreted. Evidence: `work/native-c009-int-array-20261012/execution-v2/`
and `work/c009-own-leaf-values-20261012/`.

The four immediate-parent Array element schemas and one standalone Struct now
have exact older-build descriptor/getter/factory joins. All five conditionally
select tagged framing, with null parents and disabled native/post callbacks.
Their 15 optional immediate fields comprise four Int, three Name, two Str, five
Bool and one enum-backed Byte; none is a recursive Struct or Array. Independent static review:
`work/c009-parent-children-20261012/`.

A separate preflight reproduces every prior native top span before selecting
these exact five parent fields. Both copies contain 243 of each, with no optional
omissions: 2,430 selected payloads and 741,404 bytes in total. Per-copy bounds are:

| Child | Total payload bytes | Minimum | Maximum |
| --- | ---: | ---: | ---: |
| PA01 Array | 191,290 | 692 | 950 |
| PA02 Array | 20,004 | 81 | 188 |
| PA03 Array | 22,599 | 93 | 93 |
| PA04 Array | 23,085 | 95 | 95 |
| PS01 Struct | 113,724 | 468 | 468 |

All 382 bound files remain unchanged. This selection reads no child headers,
array counts or leaf values; it establishes bounds for the next comparison and
requires no increase to the native array byte cap. Evidence:
`work/native-c009-parent-selection-20261012/`.

That case corrects the earlier own-child dimension provenance: the common
property factory reads descriptor offset `+0x20`, including for Bool. Its
`+0x24` word supplies Bool field size. All six affected Bool descriptors have
both words equal to one, so the earlier index-zero qualification and observed
header results are unchanged. Historical evidence is preserved with an additive
correction. A separate concrete Byte trace now establishes Name/FString transfer
under the reviewed enum-present loading conditions, not a one-byte serialized
payload. Enum resolution and supplied values remain unqualified; see the
[exact Byte contract](archive-string.md#concrete-enum-backed-byte-link).

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
