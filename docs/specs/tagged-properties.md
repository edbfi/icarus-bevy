# Experimental property and recorder framing

The CLI reader checks outer record boundaries in an already verified prospect
payload. Component framing is a separate caller-selected library operation,
described below. Neither operation validates gameplay values, applies version
migrations or imports a world. The explicit `MemoryArchive522` research profile is
based on the preserved build 25621760; selecting it does not identify a save's
producing build. The user reports build 25793767 for the supplied saves.

```bash
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --inner-tags
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --recorders
```

The command verifies envelope integrity first, then reports
`experimental_root_tags_only`, profile `experimental_memory_archive_522`, aggregate
record sizes and `world_imported: false`. Without `--inner-tags`, the existing
envelope-only behavior and result format are unchanged. `--recorders` implies root
framing and additionally validates the recorder structure described below. Its
result is `experimental_recorder_framing_only`, with aggregate recorder statistics
and `world_imported: false`; component byte contents remain opaque.

## Observed framing

The [loading investigation](prospect-loading.md) resolves the concrete reader to
tagged-property traversal `0x14200f5b0`. Its tag reader `0x14215f530` obtains names
through archive virtual `+0x140`, resolved to `0x140962300`, which uses the string
reader at `0x141e15db0`. The archive resets to version 522 with byte swapping off.

A canonical name uses a little-endian signed 32-bit string count. Positive counts
describe narrow bytes, negative counts describe UTF-16 code units, and nonzero
counts include a terminating zero. The native strict subset reuses the
[archive string codec](archive-string.md); it rejects malformed terminators and
unsupported normalizations instead of reproducing the original's corrections.

| Record component | Encoding in this profile |
| --- | --- |
| Property name | Archive string; canonical `None` terminates the root sequence |
| Property type | Archive string, absent after the terminator |
| Payload size | Little-endian signed 32-bit byte count |
| Array index | Little-endian signed 32-bit value |
| Type-specific metadata | Described below |
| Property GUID presence | One byte; followed by 16 bytes when present |
| Opaque value | Exactly the declared payload size |

Struct tags add a struct name and 16-byte struct GUID. Array tags add an inner type
name. Byte and enum tags add an enum name. Boolean tags carry a one-byte value in
the header. Signed integer, unsigned 32-bit integer, string and name tags have no
extra type-specific metadata. The older built-in name table identifies
`UInt32Property` as ID 25; it reaches the common GUID flag without type-specific
extensions in `0x14215f530`. Supporting this header does not decode or validate
its numeric payload.
The older tag reader gates struct GUIDs at archive version 441, array inner types
at 282 and property GUIDs at 503; all apply at 522. Other profiles are unsupported.

The size counts **only the value bytes after the complete header**. Function
`0x142166850` measures the property reader's position change against this value.
It does not make a C++ object offset a file offset. A struct array also has its own
body framing: a count followed by a shared inner tag before its elements in this
profile. The root scanner deliberately leaves that entire body opaque.

## Conservative reader policy

- Require a complete root sequence ending with canonical `None` and exact EOF.
- Bound input to 32 MiB, root records to 4,096 and each header string to 1,024 units
  including any terminator; check counts before allocating strings.
- Reject negative sizes or indices, truncated headers/values, unsupported type
  headers and noncanonical boolean/GUID-presence bytes.
- Accept canonical `ArrayProperty`, `StructProperty`, `StrProperty`, `IntProperty`, `UInt32Property`,
  `NameProperty`, `ByteProperty`, `EnumProperty` and `BoolProperty` headers. Empty header names and
  other property kinds are unsupported by this initial subset.
- Preserve original header and value spans, including unknown field names, without
  rewriting, normalizing or interpreting their contents.
- Report aggregate sizes and record counts only. No property names or values are
  included in CLI results or parser errors.

These are research limits and accepted subsets, not recovered universal format
constraints. A successful scan does not establish required-field coverage, value
types, ownership, reference integrity, recorder counts or migration fidelity.
Opaque integer or array values are not semantically validated by their tag names.
Name values likewise remain opaque; their in-memory width is not their serialized
width. The reference name-value reader uses the archive string/name primitive.

## Evidence and verification

Private static evidence, exact commands and selected exports are retained under
`work/save-schema-20261012/`: E-009 covers header/name primitives and E-010 covers
payload sizes and array dispatch. Independent inspection of only the first 256 decoded
bytes of two private copies, in `work/save-triage-20261012/` E-013/E-014, identifies
`StateRecorderBlobs` / `ArrayProperty`, an inner `StateRecorderBlob` tag and the first
`ComponentClassName` / `StrProperty` tag. Both envelopes passed independent length,
Base64, zlib and SHA-1 checks first. No private field values were published.

This prefix agreement supports investigating the same framing in the newer saves;
it does not prove the complete newer schema or replace a matched official load.
Tests use authored byte fixtures for headers and opaque values, including unknown
field names. Private saves remain outside tests, source control and CI.

A subsequent independent Node scan and the release Rust reader reached the root
terminator at exact EOF on both copies. Each contains six root tags and 378 header
and terminator bytes. Opaque payload totals are 18,512,856 bytes for the current
copy and 18,492,297 for the selected backup. These are byte/record measurements,
not counts of gameplay objects. Envelope-only output remained byte-identical and
both input hashes remained unchanged. Evidence: save-triage E-015/E-016 and
`work/native-tagged-20261012/`; strict case review and artifact hashes pass.

Six reader tests cover all supported headers, GUID variants, opaque embedded
terminators, every truncated prefix, exact record/string limits, invalid sizes and
flags, unsupported kinds and raw wide-name preservation. Fifteen prospect tests
include integrity-before-framing and CLI privacy/no-write checks. The full manual
suite, release build and independent code/static-contract review pass. No original
runtime or stock-client migration test was performed.

## Recorder framing

Static evidence E-011/E-012 resolves this concrete recorder structure to tagged
fields, each element ending in `None`. `StateRecorderBlobs` begins with its element
count and one shared `StateRecorderBlob` struct tag whose declared payload covers
all element bodies. `ComponentClassName` contains an archive string; `BinaryData`
is an array of plain bytes, encoded as a signed 32-bit count and that many bytes.
The root-only command leaves these bodies opaque; the optional recorder inspector
checks only the described field and array framing.

A separate private Node probe checked these boundaries after complete envelope
verification, with limits of 16,384 recorders, 65,536 total fields and 1,024 string
units, in addition to the input/output bounds. It traversed exactly the declared
count and consumed the shared payload exactly on both copies:

| Measurement | Current copy | Selected backup |
| --- | ---: | ---: |
| Recorder entries | 2,744 | 2,741 |
| Field tags | 5,488 | 5,482 |
| Opaque `BinaryData` bytes | 18,051,461 | 18,031,390 |

Every entry had one class-name field and one byte-array field; no other fields
were observed in these two copies. Class-name values and byte contents remain
private. These are recorder counts, not actor, building, animal or player counts.
Evidence E-017 in `work/save-triage-20261012/` binds probe versions, outputs and
input hashes. Independent code/static-contract review and strict case review pass.
The result supports further schema research, not a complete importer or a claim
that the older executable accepts these newer saves.

The native recorder reader shares the root header parser. It requires one selected
array and exact agreement between declared count, shared size, every element's
terminator and final boundary. Each element must have exactly one class string and
one byte array at index zero. Unknown fields with supported headers retain exact
borrowed spans; unsupported headers, duplicate/missing known fields or malformed
values fail the whole scan. Empty class strings and raw UTF-16 units are accepted
as framing; class resolution and semantic validity are not implied.

Input is bounded to 32 MiB, recorders to 16,384, total fields to 65,536 and header or
class strings to 1,024 units. Checks precede count-driven allocations. Parsed
record/field collections expose immutable slices, preserving the validity of
summary counts. Raw containers have no `Serialize` or `Debug` implementation;
the CLI exposes only summaries and publishes no partial result
when a nested check fails.

The release Rust reader matches every independent recorder aggregate on both
copies. Envelope-only and root-only CLI output remains byte-identical to the
preceding release, and input hashes are unchanged. Seven recorder, six root and
sixteen prospect tests pass, including nested failure/privacy/no-write checks.
Full manual checks, release build and independent static-contract/code review
pass. Qualification provenance and source snapshots: save-triage E-018,
`work/save-triage-20261012/recorder-native/` and
`work/native-recorders-20261012/`. Strict case review verifies the evidence graph
and artifact hashes with zero errors/warnings.

## Private class inventory and version scalar

A bounded follow-up (save-triage E-019) identifies 38 distinct class strings in the
current copy and 36 in the backup, with all 36 shared identifiers appearing at
equal frequencies. Two additional identifiers account for three current entries.
All strings have script-class path syntax. That classification does not establish
native implementation, available serializers or independence from packaged data.
Exact strings stay in the private case; neutral IDs are local to this inventory.
Six shared classes differ in aggregate opaque byte length, which alone does not
identify a semantic state change or establish chronology.

The known root `Version` field is an index-zero `IntProperty` with exactly four
payload bytes and signed value **2** in both copies. This is a serialization
scalar, not the producing Steam build ID. It does not replace user-reported build
25793767 or verify the older executable's migration behavior. All other root
values and component byte contents remain uninterpreted. Independent aggregate,
privacy and source-fixity checks and strict case review pass.

## Explicit base-component framing

The library's `experimental::component::read_base_component(bytes, profile)`
accepts a caller-selected candidate layout: supported tagged properties, a terminal
`None`, a four-byte little-endian zero trailer flag and exact EOF. The caller must
establish whether this layout applies to the concrete class. The API does not
recognize classes, validate required fields or interpret values; it is not enabled
by `inspect-save --recorders`.

Input is limited to 64 KiB and 256 properties. Unknown field names with supported
headers retain borrowed source spans. The parsed collection, terminator and
trailer expose immutable accessors. Only aggregate counts/sizes can be serialized
by the summary, which explicitly reports `world_imported: false` and
`values_interpreted: false`. A flag of one returns an unsupported-GUID-trailer
error even when 16 more bytes are present; other nonzero flags, truncation, missing
terminators and trailing bytes fail. The root reader still requires EOF directly
after its terminator.

Static case E-017 confirms the older build's base-method inheritance for one
private singleton class, and E-018 identifies its declared and immediate-base
field metadata. The first seven-kind private probe failed at type-name offset 281
(save-triage E-020). A separate equality-only check confirmed `NameProperty`
(E-021); a new probe adding only that header kind reached exact EOF on the two
selected 548-byte bodies (E-022):

| Measurement | Each selected body |
| --- | ---: |
| Properties | 5: struct, enum, name, string, boolean |
| Headers and terminator | 312 bytes |
| Opaque property values | 232 bytes |
| Zero object-trailer flag | 4 bytes |

Both complete envelopes and recorder arrays passed before either component scan.
Names, values and GUID bytes were not published; the earlier failure remains
preserved. These results establish this framing observation only. Equal totals
do not establish equal values, a complete class schema, newer-build method
equivalence or gameplay fidelity.

The release Rust library independently matches all four size/count aggregates on
both selected bodies (save-triage E-023). A bounded private helper receives the
selected bytes through stdin only after both envelopes and recorder arrays pass;
it emits summaries and creates no raw component artifact. Source and binary hashes,
invocations and unchanged input hashes are retained in the private case and
`work/native-component-20261012/`. Six component tests cover exact spans and limits,
every truncated prefix, embedded terminators, unsupported trailers, unchanged root
EOF policy and sanitized errors. Full manual checks and code review pass.

A further private comparison matches all five observed field names, types, indices
and struct/enum metadata against the older native descriptors, without assuming
field order (save-triage E-024). The selected struct contains one matching integer
tag with four opaque value bytes and exact termination: 47 header/terminator bytes
plus four value bytes. The three string-backed values have exact bounded archive
string framing; the boolean has no value-body bytes. This checks those two samples,
not a mandatory-field rule for every valid save. No scalar or string values were
interpreted or published; all source and prior evidence hashes remain unchanged.

Static E-019 closes the native superclass descriptor chain. Its 31 entries include
inner-property helpers; exactly five carry the save-selection bit, all in the
selected class and its immediate base. Inspected constructor/link flag updates do
not add that bit to ancestor fields. Arbitrary later runtime mutation is not
exhaustively ruled out. Agreement between the saved headers and these descriptors
does not establish reconstruction, post-load relationships or gameplay correctness.
Later static capture/cleanup evidence distinguishes a conditional count snapshot
before serialization from in-place pruning of null/non-live owner pointers and a
discarded set intersection. Those inspected paths do not read the saved integer
to reconstruct relationships; see the [loading investigation](prospect-loading.md).

### C028 framing and header coverage

The concrete C028 class has load and post-load overrides. Static evidence in
`work/component-c028-20261012/` verifies that the load override delegates unchanged
to the base reader before context work, and the post-load override changes
in-memory fields without reading its incoming archive. The shared base serializer
therefore supports a framing hypothesis, not equivalence of restoration behavior.

An independent probe selected the first minimum- and maximum-sized C028 record
in source order from each copy. Both complete envelopes, root sequences and recorder
arrays passed before those four component bodies were read. The release Rust reader
independently matched every selected body's framing aggregates:

| Measurement | Selected minimum | Selected maximum |
| --- | ---: | ---: |
| Component bytes | 3,325 | 3,358 |
| Properties | 23 | 23 |
| Headers and terminator | 1,509 bytes | 1,509 bytes |
| Opaque values | 1,812 bytes | 1,845 bytes |
| Zero object trailer | 4 bytes | 4 bytes |

A separate private comparison matched each selected record's exact names, types,
indices and type-specific header names against the 23 selected native descriptors:
integer 3, name 3, boolean 2, struct 6, array 7, enum 1 and string 1. Comparison did
not assume field order or invent fixed GUID/boolean values. It establishes agreement
for these four records without asserting mandatory presence in other saves.
Evidence: save-triage E-025/E-026/E-027; source, helper and static metadata hashes
are bound in the private cases. All property payloads remain opaque.

The later independent E-028 scan covers all 1,294 C028 records in the two supplied
copies after validating both complete outer payloads. Every record matches the same
23 exact header descriptors, with lengths from 3,325 to 3,358 bytes. Each copy's
647 components contain 14,881 fields, 976,323 header/terminator bytes, 1,184,111
opaque value bytes and 2,588 zero-trailer bytes, conserving all 2,163,022 component
bytes. The release-native batch helper independently matches all 1,294 bodies and
their combined totals in E-029: 29,762 fields and 4,326,044 bytes, comprising
1,952,646 header/terminator bytes, 2,368,222 opaque value bytes and 5,176 zero-trailer
bytes. The batch wrapper is an authored test transport, not an ICARUS file format.
Both scans interpret no values and establish neither mandatory fields nor
newer-build method equivalence.

Independent static follow-up in `work/c028-structs-20261012/` resolves the six
direct struct types to tagged serialization under the concrete older-build archive
flags, including one deferred operations registration. Six distinct types share
four operations tables; shared operations do not make their schemas identical.
Their immediate metadata contains 16 optional fields, including a `UInt32Property`
whose type is verified through the concrete factory and name getter. Three child
struct fields select binary/native serialization, so the tagged contract does not
extend recursively to every descendant. Seven array-element types have separate
tagged contracts and 17 optional immediate field descriptors. Deeper descendants,
runtime migration and newer-build equivalence remain unresolved.

### Selected nested struct streams

The initial eight-kind probe stopped at an unsupported S06 header and emitted no
successful result (save-triage E-030). A separate equality-only check identified
`UInt32Property` at that exact offset (E-031). The revised nine-kind scan then
framed all six direct struct payloads in each of the four selected C028 records,
after validating both complete outer payloads and all four parent header sets
(E-032). Every nested sequence ends in `None` and exact EOF, without an object
trailer.

| Neutral struct ID | Fields | Total bytes | Headers and terminator | Opaque values |
| --- | ---: | ---: | ---: | ---: |
| S01 | 3 | 253 | 213 | 40 |
| S02 | 7 | 343 | 318 | 25 |
| S03, S04, S05, each | 1 | 48 | 48 | 0 |
| S06 | 3 | 168 | 164 | 4 |

These dimensions occur in each selected parent: 24 nested sequences total,
64 fields and 3,632 bytes, divided into 3,356 header/terminator bytes and 276 opaque
value bytes. One bounded native batch using the release `read_root` API matches
all 24 sequences and their aggregates, with unchanged input hashes (E-033).
The separate E-034 comparison matches all 64 observed child headers to the optional
descriptors, including exact names, types, indices and struct type names. Missing
fields would be allowed; none are missing in these samples. Order and fixed GUID
values are not assumed. Grandchild encoding, scalar semantics and world
reconstruction remain separate steps.

## Explicit tagged struct-array framing

The separate library function
[`read_tagged_struct_array`](../../src/experimental/struct_array.rs) accepts a
caller-selected candidate array body and a `TagProfile`. It requires a nonnegative
little-endian 32-bit count, one shared `StructProperty` header even for an empty
array, and exactly that many `None`-terminated element sequences inside the shared
header's declared payload. Elements have variable lengths and no object trailers.
The full input must be consumed.

Limits are 64 KiB per array, 4,096 elements and 256 fields per element. Count and
minimum terminator space are checked before allocating element storage. Requiring
a shared `StructProperty` is explicit research policy. The reader does not compare
its name, nonnegative index, struct identity or GUID values against a caller's
schema; those checks belong to a separate comparison layer.

Immutable getters expose the exact borrowed prefix, shared header/payload, element
records, fields and terminators. Raw containers implement neither `Debug` nor
`Serialize`. The sanitized summary reports element/field counts, array prefix
bytes, element header/terminator bytes and opaque payload bytes; these partitions
conserve the input length. It reports `world_imported: false` and
`values_interpreted: false`.

Six authored tests cover empty arrays, variable elements with UInt32 and opaque
embedded terminators, GUID/truncation cases, incorrect counts/types/sizes, trailing
bytes, exact limits and failure after an earlier valid element. The API does not
recognize classes, recurse into values or change the CLI inspector. Static support
is recorded in `work/c028-arrays-20261012/`.

One bounded native batch independently matches 28 supplied arrays from the four
selected parents (save-triage E-037): all are empty, with 2,624 count/shared-header
bytes, zero element bytes and exact EOF. Source hashes are unchanged. The authored
stdin transport is separate from the game format and retains no body file.
Populated elements have authored-test coverage only. A wider independent metadata
inventory finds all 9,058 arrays in this class empty across both copies (E-036);
this is not native qualification of all those arrays.
