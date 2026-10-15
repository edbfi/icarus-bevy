# Tagged property and recorder framing

## Status

experimental; older-server build 25621760 profile `MemoryArchive522`, with bounded static comparison to client build 25793767. Last reconciled: 2026-10-15
Full text: d5b0ae6:docs/specs/save-format/tagged-properties.md

## Scope

Covers outer framing of an already verified prospect payload: root tagged
properties, the `StateRecorderBlobs` recorder array, and four caller-selected
library framers (base component, Int array, tagged struct array, nested struct
streams via `read_root`). The `MemoryArchive522` profile is based on build
25621760; selecting it does not identify a save's producing build, which the user
reports as 25793767 (F-323). Envelope integrity is in
[prospect envelope](prospect-envelope.md) and the string codec in
[archive string](archive-string.md); class-level results are in
[prospect loading](prospect-loading.md). Not covered: value
interpretation, version migration, class recognition, required-field coverage,
reference integrity and world import. Every reader reports
`world_imported: false`.

## Confirmed

Recovered tag layout (older server):

- Reader chain: tagged traversal `0x14200f5b0`; tag reader `0x14215f530`; names
  via archive virtual `+0x140` = `0x140962300`, string reader `0x141e15db0`. The
  archive resets to version 522 with byte swapping off (F-286, F-257).
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

- Type metadata: Struct adds struct name + 16-byte struct GUID; Array adds inner
  type name; Byte and Enum add enum name; Bool carries a one-byte value in the
  header; Int, UInt32, Str, Name, Float and Text add nothing (F-288). At
  version 509 and above, Map (built-in name 28) adds key-type then value-type
  names and Set (29) one inner-type name; Int64 (20) adds nothing. The client
  tag operator `0x14212b0e0` branches identically (F-556). `UInt32Property`
  is built-in name ID 25 and reaches the common GUID flag without extensions
  (F-290, F-247). Float/Text use only the common GUID flag (F-206, F-207).
- Version gates in `0x14215f530`: Struct GUID at 441, Array inner type at 282,
  property GUID at 503; all apply at 522. Other profiles are unsupported (F-289).
- Size counts only the value bytes after the complete header; `0x142166850`
  compares the property reader's position change against it. A C++ object offset
  is not a file offset (F-291).
- Struct-array body: count, then one shared inner tag, then elements; the root
  scanner leaves the whole body opaque (F-292).
- Recorder structure: `StateRecorderBlobs` = element count + one shared
  `StateRecorderBlob` struct tag whose payload covers all element bodies; each
  element is tagged fields ending in `None`; `ComponentClassName` is an archive
  string; `BinaryData` is a signed 32-bit count and that many plain bytes (F-298).
- Root `Version` is an index-zero `IntProperty` with exactly 4 payload bytes and
  signed value 2 in both copies: a serialization scalar, not the Steam build ID
  (F-260).
- Name values remain opaque; in-memory width is not serialized width (F-293).

Reader policy (authored; research limits, not recovered format limits):

- Root: complete sequence ending in canonical `None` at exact EOF; input 32 MiB;
  4,096 root records; 1,024 units per header string including terminator; counts
  checked before allocating. Reject negative sizes/indices, truncation,
  unsupported headers and noncanonical Bool/GUID-presence bytes (F-294).
- Accepted headers: `ArrayProperty`, `StructProperty`, `StrProperty`,
  `IntProperty`, `UInt32Property`, `NameProperty`, `FloatProperty`,
  `TextProperty`, `ByteProperty`, `EnumProperty`, `BoolProperty`,
  `MapProperty`, `SetProperty`, `Int64Property`; Map and Set payloads stay
  opaque. Empty header names and other kinds are unsupported (F-294, F-246,
  F-247, F-206, F-556).
- Preserve original header and value spans, including unknown field names,
  without rewriting. Report aggregate sizes and counts only; no names or values in
  results or errors. Opaque payloads are not validated by tag name, including
  lengths that are not a valid float or text (F-295).
- CLI: `--inner-tags` reports `experimental_root_tags_only`, profile
  `experimental_memory_archive_522`; `--recorders` implies root framing and
  reports `experimental_recorder_framing_only`. Without `--inner-tags` the
  envelope-only output is unchanged (F-295, F-244, F-245).
- Recorder reader: one selected array; declared count, shared size, every element
  terminator and the final boundary must agree. Each element has exactly one class
  string and one byte array at index 0; supported unknown fields keep borrowed
  spans; unsupported headers, duplicate/missing known fields or malformed values
  fail the whole scan. Empty class strings and raw UTF-16 are accepted as framing.
  Limits: 32 MiB input, 16,384 recorders, 65,536 total fields, 1,024 units per
  header or class string, checked before count-driven allocation. Raw containers
  have no `Serialize`/`Debug`; no partial result on nested failure (F-300).
- Base component (`experimental::component::read_base_component`): tagged
  properties, terminal `None`, 4-byte LE trailer flag 0, exact EOF. Flag 1 fails
  as unsupported GUID trailer even with 16 more bytes; other nonzero flags,
  truncation, missing terminator and trailing bytes fail. Default 64 KiB and 256
  properties; `read_base_component_with_limit` accepts 1 byte up to a hard 1 MiB
  ceiling with the same field cap; invalid limits fail even for small inputs.
  Summary adds `values_interpreted: false`. Not enabled by `--recorders`
  (F-302, F-248, F-198).
- Int array (`experimental::int_array::read_int_array`): LE signed count, then
  exactly 4 opaque bytes per element. Rejects negative counts, truncation,
  trailing bytes and more than 4,096 elements; early cap 64 KiB; largest accepted
  input 16,388 bytes. The caller must establish the ordinary non-swapping path:
  archive `+0x28` bit `0x20` and `+0x2b` bit `0x10` clear; no shared Struct header;
  bulk and property-list modes unsupported (F-312, F-313, F-200).
- Tagged struct array (`read_tagged_struct_array`): nonnegative LE 32-bit count,
  one shared `StructProperty` header even when empty, exactly count `None`-
  terminated variable-length elements inside the shared payload, no object
  trailers, full input consumed. Default 64 KiB; `_with_limit` accepts 1 byte to
  a hard 128 KiB ceiling, invalid limits fail before parsing, oversize fails before
  reading the count. 4,096 elements and 256 fields per element under either API;
  count and minimum terminator space checked before allocation. Name, index,
  struct identity and GUID are not compared to a schema. Summary partitions
  (prefix, element header/terminator, opaque payload) conserve input length
  (F-315, F-178, F-249).

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
  Struct 12 / Float 4 bytes (F-191, F-192); all 1,658 C034 own arrays, 15,342
  elements, 30,684 fields (F-316); 11,606 inherited C034 arrays, all empty (F-196);
  C009 3,402 arrays, 522 elements, CA01 66,257 bytes via `_with_limit` (F-174,
  F-203, F-317); C034 Float headers: 32 fields over 58,356 bytes (F-188).

## Candidate

- `experimental::property_tree` holds one stream as an owned value-opaque tree:
  tagged Struct payloads and tagged struct arrays nest to a depth limit, every
  other payload stays opaque, and writing checks each declared size. Every
  body of every class in both copies frames under 1 MiB and re-serializes
  byte-identically; names, types and values are not interpreted (F-557).
- Map and Set payloads load as an i32 removal count with removal items, then
  an i32 count with key-value pairs (Map) or elements (Set), each through the
  inner property's item serializer; Bool items are one byte and Struct items
  delegate to the struct (F-559, client build). The tree frames them when every
  item is a 1/4/8-byte scalar, an archive string or a tagged struct stream
  (scalar widths and string items inferred) and consumes the payload exactly;
  otherwise the payload stays opaque. Items count against the node budget; an
  attempt that would exceed it also stays opaque. All supplied Maps frame
  (F-562; unchanged under the budget, M-411, M-412).
- Applying the older-build framing to the newer saves is supported by prefix
  agreement and the scans above, not by a matched official load (F-297, U-025).
- Base-component layout is a caller-selected hypothesis per class; static
  evidence confirms base-method inheritance for the singleton class and that the
  C028 load override delegates unchanged to the base reader (F-306, F-258). The
  superclass chain has 31 entries, exactly 5 with the save-selection bit (F-305).
- C028 six direct structs are tagged with 16 optional fields; three child struct
  fields use binary/native serialization, so tagged framing is not recursive
  (F-308). D01: 8 optional leaves (5 Int, 3 Str); D02: four 4-byte native
  transfers (F-309, F-212). Conditional static grammars only.
- Every opaque Struct leaf is one of four fixed-width engine math types
  (F-565): three or four ordered 4-byte components, no count or tail (F-602).
- Client build 25793767: tag operator `0x14212b0e0`, Array serializer
  `0x142123da0`; same 441/282/503 gates; Map/Set gate 509. Array branch needs mask
  `0x20` clear, version >= 500 and the inner Struct class flag; reads a 4-byte
  count and a shared header before elements, including count 0; shared size is a
  32-bit end-minus-start difference (F-193, F-318, U-030).
- Client C034: schema `0x145ff68b0`, ops table `0x144faeff8`; restore builds version
  522 with loading on and binary/tagged, partial-selection and byte-swap flags
  clear before component Serialize, reaching conditional tagged dispatch
  (F-194, F-319, F-320). N02 native branch demands three 4-byte transfers (F-321).
  Float: generation 10, vtable `0x145026a20` selects `0x142123ce0` at virtual
  `0x90`, 4 bytes on every transfer path (F-322).

## Open questions

- Original normalization, EOF and error behavior versus the strict readers (U-026).
- Official restore/import and newer-build migration of these saves (U-025).
- Value meanings, Name identities and Enum lookups (U-020); empty-family
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
