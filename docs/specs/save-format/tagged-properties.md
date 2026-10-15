# Tagged property and recorder framing

## Status

experimental; older-server build 25621760 profile `MemoryArchive522`, with bounded static comparison to client build 25793767. Last reconciled: 2026-10-15
Full text: d5b0ae6:docs/specs/save-format/tagged-properties.md

## Scope

Outer framing of an already verified prospect payload: root tagged properties,
the `StateRecorderBlobs` recorder array, and four caller-selected library framers
(base component, Int array, tagged struct array, nested struct streams via
`read_root`). The `MemoryArchive522` profile is based on build 25621760 and does
not identify a save's producing build, user-reported as 25793767 (F-323).
Envelope: [prospect envelope](prospect-envelope.md); strings:
[archive string](archive-string.md); class results: [prospect loading](prospect-loading.md).
Not covered: values, version migration, class recognition, required-field
coverage, reference integrity and world import. Every reader reports
`world_imported: false`.

## Confirmed

Recovered tag layout (older server):

- Reader chain and archive reset to version 522, no byte swapping: F-286, F-257.
- Names use the archive string: little-endian signed 32-bit count; positive =
  narrow bytes, negative = UTF-16 code units; nonzero counts include a
  terminating zero. The native strict subset rejects malformed terminators and
  unsupported normalizations instead of reproducing the original's corrections
  (F-287, U-026).
- Tag record, in order (F-287):

  | Component | Encoding |
  | --- | --- |
  | Property name | Archive string; canonical `None` terminates the sequence |
  | Property type | Archive string, absent after the terminator |
  | Payload size | LE signed 32-bit byte count |
  | Array index | LE signed 32-bit value |
  | Type-specific metadata | see next bullet |
  | Property GUID presence | 1 byte; followed by 16 bytes when present |
  | Value | exactly the declared payload size |

- Type metadata per kind: F-288; from version 509, Map (name 28), Set (29) and
  Int64 (20): F-556, with identical client branching. `UInt32Property` is built-in
  name ID 25 (F-290, F-247); Float/Text use only the common GUID flag (F-206, F-207).
- Version gates in `0x14215f530`: Struct GUID at 441, Array inner type at 282,
  property GUID at 503; all apply at 522; other profiles unsupported (F-289).
- Size counts only the value bytes after the complete header; a C++ object
  offset is not a file offset (F-291).
- Struct-array body: count, one shared inner tag, then elements; the root scanner
  leaves it opaque (F-292). Recorder array layout: F-298.
- Root `Version` is an index-zero `IntProperty`, 4 payload bytes, signed value 2
  in both copies: a serialization scalar, not the Steam build ID (F-260).
- Name values remain opaque; in-memory width is not serialized width (F-293).

Reader policy (authored; research limits, not recovered format limits):

- Root: complete sequence ending in canonical `None` at exact EOF; input 32 MiB;
  4,096 root records; 1,024 units per header string including terminator; counts
  checked before allocating; negative, truncated, unsupported and noncanonical
  Bool/GUID-presence input rejected (F-294).
- Accepted headers: `ArrayProperty`, `StructProperty`, `StrProperty`,
  `IntProperty`, `UInt32Property`, `NameProperty`, `FloatProperty`,
  `TextProperty`, `ByteProperty`, `EnumProperty`, `BoolProperty`,
  `MapProperty`, `SetProperty`, `Int64Property`; Map and Set payloads stay
  opaque. Empty header names and other kinds are unsupported (F-294, F-246,
  F-247, F-206, F-556).
- Original header and value spans, including unknown field names, are preserved
  unrewritten; results and errors carry aggregates only, never names or values;
  opaque payloads are not validated by tag name (F-295).
- CLI: `--inner-tags` reports `experimental_root_tags_only`, profile
  `experimental_memory_archive_522`; `--recorders` implies root framing and
  reports `experimental_recorder_framing_only`. Without `--inner-tags` the
  envelope-only output is unchanged (F-295, F-244, F-245).
- Recorder reader: declared count, shared size, every element terminator and the
  final boundary must agree; one class string and one byte array at index 0 per
  element; unknown supported fields keep borrowed spans; unsupported headers,
  duplicate or missing known fields and malformed values fail the whole scan.
  Empty class strings and raw UTF-16 are accepted as framing. Limits 32 MiB,
  16,384 recorders, 65,536 fields, 1,024 units per header or class string,
  checked before count-driven allocation. Raw
  containers have no `Serialize`/`Debug`; no partial result (F-300).
- Base component (`experimental::component::read_base_component`): tagged
  properties, terminal `None`, 4-byte LE trailer flag 0, exact EOF; flag 1 fails
  as unsupported GUID trailer even with 16 more bytes; other nonzero flags,
  truncation, missing terminator and trailing bytes fail. Default 64 KiB and 256
  properties; `read_base_component_with_limit` accepts 1 byte up to a hard 1 MiB
  ceiling with the same field cap; invalid limits fail even for small inputs. Summary adds `values_interpreted: false`. Not
  enabled by `--recorders` (F-302, F-248, F-198).
- Int array (`experimental::int_array::read_int_array`): LE signed count, then
  exactly 4 opaque bytes per element; rejects negative counts, truncation,
  trailing bytes and more than 4,096 elements; early cap 64 KiB,
  largest accepted input 16,388 bytes (F-313, F-200). Caller-established archive
  path conditions: F-312.
- Tagged struct array (`read_tagged_struct_array`): nonnegative count, one shared
  `StructProperty` header even when empty, count `None`-terminated elements in the
  shared payload, no trailers, full input consumed (F-315). Default 64 KiB;
  `_with_limit` accepts 1 byte to a hard 1 MiB ceiling (F-634); invalid limits fail
  before parsing and oversize before reading the count; 4,096 elements and 256
  fields per element, with count and minimum terminator space checked before
  allocation; no schema comparison; summary partitions (prefix, element
  header/terminator, opaque payload) conserve input length (F-178, F-249).

Supplied-data measurements (two private copies; byte/record counts, not gameplay
objects):

- Root: six root tags, 378 header/terminator bytes per copy, exact EOF; opaque
  totals 18,512,856 and 18,492,297 bytes (F-244, F-296).
- Recorders: 2,744 / 2,741 entries; 5,488 / 5,482 field tags; 18,051,461 /
  18,031,390 opaque `BinaryData` bytes; each entry has one class-name and one
  byte-array field (F-245, F-299). Class strings: 38 / 36, all 36 shared at equal
  frequency (F-260, F-301).
- Singleton component: two 548-byte bodies, 5 properties (struct, enum, name,
  string, boolean): 312 header/terminator, 232 opaque, 4 trailer bytes; the
  struct holds one Int tag, 47 header/terminator + 4 value bytes (F-303, F-304).
- C028: all 1,294 bodies frame; four selected records match 23 descriptors (Int 3,
  Name 3, Bool 2, Struct 6, Array 7, Enum 1, Str 1); all 9,058 arrays empty
  (F-259, F-307). Nested streams S01 3/253/213/40, S02 7/343/318/25, S03-S05 each
  1/48/48/0, S06 3/168/164/4 (fields/total/header+terminator/opaque); 24
  sequences, 64 fields, 3,632 = 3,356 + 276 bytes (F-310, F-311).
- Int arrays: 486 C009 S02 arrays, 980 bytes (F-314). Struct arrays: 28 C028 arrays,
  2,624 bytes, all empty (F-249); C034 four arrays, 428 elements, 56,412 bytes,
  Struct 12 / Float 4 bytes (F-191, F-664); all 1,658 C034 own arrays, 15,342
  elements, 30,684 fields (F-316); 11,606
  inherited C034 arrays, all empty (F-196); C009 3,402 arrays, 522 elements, CA01
  66,257 bytes via `_with_limit` (F-174, F-203, F-317); C034 Float headers: 32
  fields over 58,356 bytes (F-188).

## Candidate

- `experimental::property_tree` holds one stream as an owned value-opaque tree:
  tagged Struct payloads and struct arrays nest to a depth limit, other payloads
  stay opaque and writing checks each declared size; every body in both copies
  frames under 1 MiB and re-serializes byte-identically; names, types and values
  are not interpreted (F-557).
- Map and Set item layout (client build): F-559. The tree frames them when every
  item is a 1/4/8-byte scalar, an archive string or a tagged struct stream and the
  payload is consumed exactly, within the node budget; otherwise the payload stays
  opaque. All supplied Maps frame (F-562; unchanged under the budget, M-411, M-412).
- Applying the older-build framing to the newer saves is supported by prefix
  agreement and the scans above, not by a matched official load (F-297, U-025).
- Base-component layout is a caller-selected hypothesis per class; static
  evidence confirms singleton base-method inheritance and unchanged C028 delegation
  to the base reader (F-306, F-258); superclass selection bits: F-305.
- C028 six direct structs are tagged with 16 optional fields; three child struct
  fields use binary/native serialization, so tagged framing is not recursive
  (F-655). D01 leaves and D02 transfers: F-309, F-655. Conditional static grammars only.
- Every opaque Struct leaf is one of four fixed-width engine math types
  (F-565): three or four ordered 4-byte components, no count or tail (F-602).
- Client build 25793767: tag operator `0x14212b0e0`, Array serializer
  `0x142123da0`; same 441/282/503 gates; Map/Set gate 509; Array branch conditions
  and count/shared-header layout match, shared size a 32-bit end-minus-start
  difference (F-193, F-318, U-030).
- Client C034: restore reaches conditional tagged dispatch with version 522 and
  loading on (F-659); N02 native branch demands three 4-byte transfers (F-667).
  Float: generation 10, vtable `0x145026a20` selects `0x142123ce0` at virtual
  `0x90`, 4 bytes on every transfer path (F-659).

## Open questions

- Original normalization, EOF and error behavior versus the strict readers (U-026).
- Official restore/import and newer-build migration of these saves (U-025).
- Value meanings and Name identities (U-020); empty-family
  population (U-021); deeper C028 children (U-048).
- Client large-span safety (U-030) and dedicated-server correspondence (U-031).
- Singleton saved integer's restoration role (U-039); remaining classes (U-050).
- Supplied Text-bearing body (U-053); runtime save-bit mutation (U-054); Map/Set
  payload layout and other kinds (U-055).

## Evidence

- M-147 static exports (E-009 to E-012); M-118 save triage (E-013 to E-037);
  M-140 root, M-141 and M-171 recorders, M-142 Name, M-143 C028 structs, M-144
  component, M-148 C028 overrides, M-150 C028 arrays, M-112 and M-120 Float/Text,
  M-096 and M-098 Int array, M-070, M-077, M-089, M-090, M-122; client M-083 to
  M-086; Map/Set/Int64 headers M-387, M-388; all-class framing M-389, M-390;
  container items M-392 to M-396, M-401 to M-404.
- Tests: `tests/tagged.rs`, `tests/recorders.rs`, `tests/component.rs`,
  `tests/int_array.rs`, `tests/struct_array.rs`, `tests/prospect.rs`; authored
  byte fixtures only. Private saves stay outside tests, source control and CI.

```bash
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --inner-tags
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --recorders
cargo test --locked --test tagged --test recorders --test component --test int_array --test struct_array --test property_tree --test prospect
```
