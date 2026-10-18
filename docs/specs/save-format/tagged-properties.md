# Tagged property and recorder framing

## Status

experimental; older-server build 25621760 profile `MemoryArchive522`, with bounded static comparison to client build 25793767. Last reconciled: 2026-10-18
Full text: d5b0ae6:docs/specs/save-format/tagged-properties.md

## Scope

Outer framing of an already verified prospect payload: root tagged properties,
the `StateRecorderBlobs` recorder array, and four caller-selected library framers
(base component, Int array, tagged struct array, nested struct streams via
`read_root`). The `MemoryArchive522` profile is based on build 25621760 and does
not identify a save's producing build, user-reported as 25793767 (F-323).
Envelope: [prospect envelope](prospect-envelope.md); strings:
[archive string](archive-string.md); class results: [prospect loading](prospect-loading.md).
Not covered: gameplay data-table row meanings, reference integrity and world
import. Every reader reports `world_imported: false`.

## Confirmed

Recovered tag layout (older server):

- Reader chain and archive reset to version 522, no byte swapping: F-286, F-257.
- Names use the archive string: little-endian signed 32-bit count; positive =
  narrow bytes, negative = UTF-16 code units; nonzero counts include a
  terminating zero. The native strict subset rejects malformed terminators and
  unsupported normalizations instead of reproducing the original's lossy `?` /
  zeroing corrections (F-287, F-719; U-026, U-033 resolved).
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
  Int64 (20): F-556, with identical client branching. `UInt32Property` is
  built-in name ID 25 (F-290, F-247); Float/Text use only the common GUID flag
  (F-206, F-207).
- Version gates in `0x14215f530`: Struct GUID at 441, Array inner type at 282,
  property GUID at 503; all apply at 522; other profiles unsupported (F-289).
- Size counts only the value bytes after the complete header; a C++ object
  offset is not a file offset (F-291).
- Struct-array body: count, one shared inner tag, then elements; the root scanner
  leaves it opaque (F-292). Recorder array layout: F-298.
- Root `Version` is an index-zero `IntProperty`, 4 payload bytes, signed value 2
  in both copies: a serialization scalar, not the Steam build ID (F-260, F-715;
  U-060 resolved).
- Name values remain opaque; in-memory width is not serialized width (F-293).

Reader policy (authored; research limits, not recovered format limits):

- Root: complete sequence ending in canonical `None` at exact EOF; input 32 MiB;
  4,096 root records; 1,024 units per header string including terminator; counts
  checked before allocating; negative, truncated, unsupported and noncanonical
  Bool/GUID-presence input rejected across the 14 accepted `TagKind`s
  (`ArrayProperty`, `StructProperty`, `StrProperty`, `IntProperty`,
  `UInt32Property`, `NameProperty`, `FloatProperty`, `TextProperty`,
  `ByteProperty`, `EnumProperty`, `BoolProperty`, `MapProperty`, `SetProperty`,
  `Int64Property`; F-206, F-246, F-247, F-294, F-295, F-556).
- CLI: `--inner-tags` reports `experimental_root_tags_only`
  (`experimental_memory_archive_522`); `--recorders` implies root framing and
  reports `experimental_recorder_framing_only` (F-244, F-245, F-295).
- Recorder, base-component, Int-array and tagged-struct-array readers enforce
  exact EOF, terminal `None`, trailer flag `0` (rejecting GUID trailer `1`), and
  pre-allocation caps (32 MiB / 16,384 recorders / 65,536 fields; 64 KiB default
  up to 1 MiB `_with_limit` / 256 fields per component or struct element / 4,096
  elements; F-178, F-198, F-200, F-248, F-249, F-300, F-302, F-312, F-313,
  F-315, F-634).

Supplied-data measurements (two private copies):

- Root: six root tags, 378 header/terminator bytes per copy, exact EOF; opaque
  totals 18,512,856 and 18,492,297 bytes (F-244, F-296).
- Recorders: 2,744 / 2,741 entries across 38 / 36 classes (all 36 shared at equal
  frequency); 18,051,461 / 18,031,390 opaque `BinaryData` bytes (F-245, F-260,
  F-299, F-301).
- Component, Int-array and struct-array populations (`C013`, `C028`, `C034`,
  `C009`): all bodies and nested streams frame to exact EOF under 1 MiB (F-174,
  F-188, F-191, F-196, F-203, F-249, F-259, F-303, F-304, F-307, F-310, F-311,
  F-314, F-316, F-317, F-664).

## Candidate

- `experimental::property_tree` holds one stream as an owned value-opaque tree:
  tagged Struct payloads and struct arrays nest to a depth limit, other payloads
  stay opaque and writing checks each declared size; every body in both copies
  frames under 1 MiB and re-serializes byte-identically (F-557).
- Map and Set item layout (`experimental::archive_frame`, F-559, F-725): server
  (`0x142159080`/`0x14215a4b0`) and client (`0x142124c50`/`0x142126080`) match
  across all 14 `TagKind`s, and `UScriptStruct::SerializeItem` (`0x14200e610`)
  branches on `StructFlags & 0x800` (`FIntPoint` 8 B vs tagged). All 28 `C030`
  `SignTextRecord` `TextProperty` bodies (`0x142182820` -> `0x141eb4390`, history
  `-1`) and all 163 Maps per copy (`C015`, `C022`, `C032`, F-562, F-725, F-726;
  U-053, U-055 resolved) frame to exact EOF.
- Base-component layout and all 65 `RecorderComponent` `FClassParams` (including all
  38 `StateRecorderBlobs` classes' `+0xa0`/`+0x488`/`+0x4a0`/`+0x4b0` overrides) match
  across builds; `ConstructFProperty` (`0x14216e730`) sets `PropertyFlags` (`+0x40`)
  from static `Descriptor->PropertyFlags` and `LinkInternal` (`0x1408ad190`) never
  mutates `+0x40` (`CPF_SaveGame` `0x01000000`, F-258, F-305, F-716, F-720, F-727,
  F-728, F-734 to F-736; U-020 to U-022, U-024, U-032, U-039, U-046, U-048, U-050,
  U-054, U-061 resolved).
- Every opaque Struct leaf is one of four fixed-width engine math types (F-565,
  F-602; U-027 resolved), and client 25793767 matches server 25621760 on tag,
  Array, `FFloatProperty`, `FVector`, and `CppStructOps` serializers (F-193,
  F-318, F-659, F-667, F-720; U-029 to U-031 resolved).

## Open questions

- Official restore/import and newer-build migration of these saves (U-025, U-045).

## Evidence

- M-147 static exports; M-118 save triage; M-140 to M-144, M-148, M-150, M-171,
  M-112, M-120, M-096, M-098, M-070, M-077, M-089, M-090, M-122; client M-083 to
  M-086; Map/Set/Int64 and all-class framing M-387 to M-390, M-392 to M-396,
  M-401 to M-404; server migration, entry, correspondence and 65-class / restore
  audits M-755 to M-760, M-766 to M-769, M-772 to M-775.
- Tests: `tests/tagged.rs`, `tests/recorders.rs`, `tests/component.rs`,
  `tests/int_array.rs`, `tests/struct_array.rs`, `tests/archive_frame.rs`,
  `tests/prospect.rs`.
