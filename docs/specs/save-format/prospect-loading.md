# Prospect loading: save dispatch and inner framing

## Status

Experimental; scope dedicated-server build 25621760 (supplied saves are user-reported from 25793767). Last reconciled: 2026-10-14.
Full text: d5b0ae6:docs/specs/save-format/prospect-loading.md

## Scope

Covers the two original prospect-load entry paths, the inner `ProspectSaveState`
type and version fixups, the archive-mode dispatch that selects tagged properties,
recorder and base-component byte framing, and the static contracts and observed
framing populations of the selected component classes C013, C028, C034 and C009
(with nested D01) in the two supplied copies. The outer envelope is in
[envelope inspection](prospect-envelope.md); tag grammar, version gates and reader
APIs are in [tagged properties](tagged-properties.md); string codecs are in
[archive strings](archive-string.md). Not covered: property values, enum lookup,
text or numeric meaning, reference resolution, newer-build equivalence, original
EOF/normalization parity, runtime restoration and world import. All offsets below
are object offsets or code addresses in build 25621760, not save-file offsets,
unless stated as serialized bytes.

## Confirmed

Entry paths and failure reporting

- By-name candidate `0x14146e6c0` checks for an empty name, looks up the file,
  loads, compares the loaded name and continues toward map loading; it appends
  `.json` (F-325). From-file candidate `0x1414baf60` gates state application on two
  readers sharing one path object; either failure enters reporting (F-326).
- Reporting sets a global flag and calls a statically initialized output target
  that can force termination or raise an exception by guard state; it is not
  log-only (F-327). By-name missing and truncated inputs were rejected without an
  observed crash (F-256).

Inner state, version and archive mode

- State vtable `0x1449e5c88`: `+0x10` → `0x1418d7b60` → `0x141cfd400` registers
  `ProspectSaveState`; header vtable `0x1449e5c70` → `0x1418d7b70` → `0x141cfd460`
  (`ProspectSaveStateHeader`). Metadata `0x145e9cb50` bases on header constructor
  `0x141cfbc60`; header descriptor `0x145e9cbf0`; sizes `0xf8`/`0xe8` (F-328, F-329).

| Field | Object offset | Role |
| --- | --- | --- |
| `Version` | `0x08` | Integer tested by load fixups |
| `LobbyPrivacy` | `0x0c` | Enum, byte underlying descriptor |
| `ProspectInfo` | `0x10` | Nested prospect metadata |
| `ProspectMapName` | `0xb0` | String |
| `LastSavedDateTime` | `0xc0` | Date/time |
| `ProspectID` | `0xc8` | Legacy string, pre-version-2 fixups |
| `ProspectDTKey` | `0xd8` | Legacy string, pre-version-2 fixups |
| `StateRecorderBlobs` | `0xe8` | Array, derived state type |

- `StateRecorderBlob` is `0x20` bytes in memory: `ComponentClassName` at 0,
  `BinaryData` at `0x10` (F-330, F-331, F-332). Descriptor order is not serialized order.
- By-name call site `0x14146ef42` → `0x1414831f0` constructs the state and calls
  blob reader `0x1418e6240` at `0x141483466`; the from-file chain runs through
  `0x141823fb0` and `0x1418e6140` (F-333, F-334).
- Dispatcher `0x1418ee4a0` calls serializer slot `+0x2a0` (schema vtable
  `0x144ec5e20` → `0x14200ea70`): `0x14200ea90` when archive byte `+0x28` bit `0x20`
  is set, else `0x14200f5b0`. Reset `0x141f58f60` → `0x141f5f450` clears `0x20`,
  then loading `0x01` is set and `0x80` cleared, so this reader uses tagged
  traversal through tag reader `0x14215f530` (F-335, F-336, F-257).
- With loading bit `0x01`, slot `+8` runs fixups: state `0x1418e4250`, header
  `0x1418e44f0`; both branch on `Version < 2` and pass legacy strings to nested
  `+0x10`/`+0x38` (calls to `0x14089a1c0`). State version 0 also calls
  `0x14170a4c0`, sets byte `+0x0c` to 1, releases recorder allocations and clears
  the count (F-337, F-338). Both supplied root `Version` integers are 2 (F-260).
- 16-byte key `0x145dc12c8`: lookup `0x141f5b5c0` returns `-1` when absent and is
  stored at object `+8` after fixup; version-use slot `+0x1d8` reads no table when
  loading (F-339).

Recorders and base component

- Recorder struct selector `0x14200e610`; ops factory `0x141cfbd60` supplies no
  native or post-serialization hook. Each recorder is a `None`-terminated tagged
  sequence after the array's single shared inner tag (F-340).
- `BinaryData` (no enum binding, reader `0x142158d40`): signed 32-bit count, then
  that many raw bytes (F-341). Scans reach the exact recorder-array boundary:
  2,744/2,741 recorders (F-245, F-257).
- Restore `0x1414b83c0` resolves the class string, constructs the component and
  calls `+0x4a0`; resolve/construct failures return null (F-342). Base vtable
  `0x144a5eb40` `+0x4a0` = `0x1418a9c80`: reader vtable `0x1447a9470` over
  `BinaryData`, loading set, `0x80` cleared, mask `0x04008000` added at `+0x28`,
  then `+0xa0` = `0x1418aaf70`; no exact EOF check (F-343).
- Serialize chain `0x14346c410` → `0x142123530` → `0x1421235a0` → selector
  `0x142123f50` → UClass slot `+0x2a0` = `0x14200ea70`, tagged (F-344). Flag
  `0x04000000`: predicate `0x14215be10` skips, by declared size, fields lacking
  property flag `0x01000000`; with false virtual `+0x200` it enables name/type
  remapping (F-345).
- Trailer `0x14202f1f0`: four-byte boolean; nonzero adds a 16-byte GUID (four
  32-bit words via `0x141f1c300`). Property termination is not component EOF.
  Original direct-buffer path accepts any nonzero; its fallback flags values > 1
  (F-346). Post-load `+0x488` base target is return-only; derived classes may
  override (F-347).

C013 (singleton)

- Inherits base load, serialize and empty post-load slots (F-348). NameProperty is
  built-in name ID 6, no type-specific metadata, then the property-GUID field;
  value reader `0x1421580b0` uses archive `+0x140` (F-350, F-246).
- Both 548-byte bodies: five properties, 312 header/terminator bytes, 232 opaque
  value bytes, zero four-byte trailer, exact EOF (F-351, F-248). Selection-bit
  descriptors: one struct with one integer child plus enum, name, string, bool;
  five of 31 ancestry entries (F-352, F-353).
- Callback `0x1418a1850` copies four bytes from owner `+0x250` to recorder `+0xd8`;
  `0x1418ad0c0` iterates owner `+0x248` (count `+0x250`, eight-byte elements) (F-354).
  Capture via `0x14189aa00` → `0x1418a3d90` (slot `+0x448`) is conditional, then
  serialize `0x1418aafb0` (F-356). `0x1414bf530` (byte `+0xb2 == 3`, virtual `+0x470`)
  discards set intersection `0x141899760`; `0x14148a160` compacts the pointer array
  by `memmove`; nothing reads `+0xd8` back (F-357, F-358).

C028 (repeated)

- Registration `0x1406ff5d0` → getter `0x141d2d770` → callback `0x141d2e710` →
  constructor `0x1418c22f0` → vtable `0x144a70e90` (F-359). `+0x4a0` = `0x1418a9bb0`
  (base, then conditional `+0x4b0` with load context); `+0xa0` base; `+0x488` =
  `0x14189d380`, which ignores the archive argument (F-360, F-361).
- 60 ancestry entries (3/27/11/19/0); 23 selection-bit fields: 3 Int, 3 Name,
  2 Bool, 6 Struct, 7 Array, 1 Enum, 1 Str (F-362). All 1,294 records frame:
  29,762 fields, 4,326,044 bytes; per copy 647 components (F-259, F-363, F-364, F-365).
- Six direct struct types are tagged under `MemoryArchive522`; deferred registration
  `0x140729c20` supplies one ops factory (F-366). Their 16 children (3/7/1/1/1/3):
  3 Struct, 1 Name, 4 Int, 7 Bool, 1 UInt32 (F-367). `UInt32Property` is built-in ID
  25 via vtable `0x144eebf50`, getter `0x14214a630` and constructor `0x142017040`
  (F-368, F-247). Binary N01 is four ordered four-byte little-endian transfers
  (16 bytes), N02 three (12 bytes), no count, terminator or tail (F-369, F-124).
- Array serializer `0x1421580d0`: signed 32-bit count, one shared struct tag even
  at count 0, then `None`-terminated elements without object trailers; the shared
  size spans all elements (F-370). Shared tag: index 0, inner name = field name,
  element struct name; the loader does not enforce equality (F-371). Children:
  7 Name, 3 Int, 3 Float, 1 Bool, 1 Text, 1 Array, 1 Struct (F-372); D01 eight
  optional fields, D02 four four-byte transfers (F-212, F-373).
- All 9,058 arrays are empty: 848,864 bytes = 36,232 count + 812,632 shared-header
  (F-374, F-116). Top and child receipts: F-123..F-141.

C034

- Base load/serialize, C028 post-load; 21 optional fields: 7 Struct, 2 Int, 7 Array,
  2 Name, 1 Enum, 1 Str, 1 Bool (F-375, F-211). Per copy 829 records, 4,199,203 bytes,
  17,409 fields (F-376, F-378, F-195); own structs 572–28,606 bytes (F-377, F-379, F-188).
- Own array child `0x145ee1220`: tagged, optional Struct (N02) and Float, signed
  count and shared Struct header even at zero; 16-byte memory size is not a stride
  (F-380). Struct spans 12 bytes and Float four (F-192); max count 214 (F-381, F-382).
- Inherited arrays all empty; inherited Struct widths 16/12/12/12/4 bytes
  (F-383, F-384, F-385, F-196). Scalars: F-146..F-153.

C009 and D01

- 31 optional top fields: 2 own, 9 immediate-parent, 20 common-ancestor (F-386);
  486 bodies, 6,781,758 bytes, max 71,956 under an explicit byte cap (F-387, F-182).
- Own: S02 `ArrayProperty<IntProperty>` + Bool, S01 Str + Int; a non-Struct array is
  a four-byte count plus four-byte elements, no shared header (F-388, F-389, F-390).
- Parents PA01–PA04/PS01 tagged with 15 fields (F-391); payload bounds per copy:
  PA01 692–950, PA02 81–188, PA03 93, PA04 95, PS01 468 bytes (F-392..F-395, F-202).
- Common: CA01 reaches 66,257 bytes (F-203); headers F-174, F-175, F-396, F-397;
  binary widths 16/12/12/16 (F-172); ND01 via descriptor `0x144aba698`, cap 66,056
  (F-398, F-165..F-167). Descriptor `+0x20` is dimension, `+0x24` Bool size (F-399).
- D01 leaves and arrays: F-155..F-162, F-400.

Acceptance conditions for any importer

- Load into staging state, resolve references and validate the complete supported
  schema before publishing; distinct actionable errors for missing, malformed,
  unsupported and inconsistent inputs; preserve the source and existing native state;
  never silently produce an empty world; report unmapped content-dependent fields as
  incompatibility (F-403). `import-save` stays unavailable until field coverage and
  stock-client validation demonstrate correct migration (F-229, F-269).
- Strict readers must enforce the binary transfer lengths and treat name/index/type
  equality and 0/1-only trailer booleans as comparison policy (F-346, F-369, F-371).
- Reader limits used for these populations: component and struct-array default
  64 KiB, explicit limit up to a hard 128 KiB ceiling (measured caps 71,956, 66,056;
  D01 helper 1,050 bytes), 4,096 elements and 256 fields per element, Int-array
  4,096 elements, root 4,096 tags, seven-field post-parse helper check and a
  512 MiB native address-space test limit (F-134, F-161, F-178, F-198, F-200,
  F-249, F-382).

## Candidate

- Base-component framing is a candidate layout for C013 and C028 in build 25621760;
  it is not established for the user-reported newer build (F-348, F-360, U-025).
- The C013 struct field captures an owner-collection count (inferred, F-355).
- C028/C034/C009 struct and array contracts are conditional older-build framing
  contracts, not fixed disk widths; the 16/12-byte transfers assume successful
  archive reads (F-366, F-369, F-380, F-172).
- C001 (four records): native getter `0x1419fe200`, callback `0x1419fe870`
  (`[0x1419fe870, 0x1419fe9a1)`, 305 bytes), reflection getter `0x141a00220`,
  candidate constructor `0x141788530` (`[0x141788530, 0x141788658)`, 296 bytes);
  one DEFAULT non-call reference from `0x14655d468` (F-032, F-034, F-040, F-121,
  F-122, F-401, F-402). Listing queries were rejected at `0x1419fe870` (F-014, F-019).
  Exact windows now decode as 51/49 instructions; both call `0x1418824d0` and
  share 40 typed post-call instructions, including a local store of
  `0x1449d9f90` via RBX (F-409, F-410). Receiver continuity across the call and
  archive slots remain unqualified; the common callee decodes all 106 physical
  bytes as 24 instructions without function or receiver proof (F-416). No C001
  payload is parsed. Nested direct calls at `0x142174bc0`/`0x143453c00` have
  measured 196/217-byte intervals, decoded as45/48 instructions without
  complete-function or receiver proof (F-419, F-420, F-424). Declared local CFG
  replay qualifies93edges, ten opaque calls and three return sites; both graphs
  remain open and receiver preservation unproved (F-426).
  Callback/candidate replay qualifies99local edges, two common-callee calls and
  two returns, with both graphs likewise open (F-430).
  Three12-byte v1 unwind records declare register3 pushes ending at offset2 and
  fixed48/32/48-byte allocations ending at offset6. Each has flags2 with an opaque
  handler RVA. Metadata intent qualifies; handler data is unread and local
  stack/receiver joins remain open (F-434).
  Installed-language query qualifies RBX/RCX/RSP storage offsets and32-bit
  variants; global/subbyte aliases and live values remain unproved (F-436).
  Conditional frame models are source-qualified, including opaque handler
  headers (F-437, F-438). Four spaces use byte units; RAM/constant addresses have
  64bits and register/unique addresses32bits, separate from register value widths
  (F-439). Exact input/prolog joins qualify. Earlier bitvector COPY-width stops
  lack locations; the narrow sibling reports three opcode26 row/index stops,
  without independent reachability. Calls, returns, receiver/escape/ABI/ownership
  proof remain open (F-440 to F-450).

## Open questions

- From-file contracts, by-name entry and the runtime failure outcome (U-056, U-057, U-058).
- Version fixup semantics and Version/build relation (U-059, U-060).
- C028 context restoration and migration (U-061); mandatory fields and order (U-062).
- Controlled reproduction matrix for both paths: retain command/path, input digest,
  truncation point, logs, exit status and before/after state; repeat with an unchanged
  valid fixture and separate Proton/platform effects (U-063, U-047).
- Values, empty families, other classes, C001 bridge (U-020, U-021, U-022, U-041).
- Import, newer builds, EOF/normalization parity (U-025, U-026, U-027, U-029, U-032, U-039).

## Evidence

- Static: M-147, M-148, M-143, M-149, M-150, M-119, M-122, M-123, M-079, M-094,
  M-096, M-100, M-170; runtime and reports: M-154, M-155, M-160.
- Framing receipts: M-118, M-172, M-087..M-093, M-095, M-097..M-103, M-106,
  M-058..M-070, M-049..M-055; C001: M-007, M-008, M-011, M-013, M-173, M-179, M-180, M-181.
- Tests: `tests/tagged.rs`, `tests/recorders.rs`, `tests/component.rs`,
  `tests/struct_array.rs`, `tests/int_array.rs`, `tests/archive_frame.rs`,
  `tests/prospect.rs`, `tests/cli.rs`.

```bash
cargo test --locked --test tagged --test recorders --test component \
  --test struct_array --test int_array --test archive_frame --test prospect
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --recorders
```
