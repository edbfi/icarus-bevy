# Prospect loading: save dispatch and inner framing

## Status

Experimental; scope dedicated-server build 25621760 (supplied saves are user-reported from 25793767). Last reconciled: 2026-10-18.
Full text: d5b0ae6:docs/specs/save-format/prospect-loading.md

## Scope

The two original prospect-load entry paths, inner `ProspectSaveState` type and
version fixups, archive-mode dispatch, recorder and base-component framing, and
the static contracts and framing populations of C013, C028, C034 and C009 (with
nested D01) in the two supplied copies. Envelope: [envelope inspection](prospect-envelope.md);
tag grammar, gates and readers: [tagged properties](tagged-properties.md); strings:
[archive strings](archive-string.md). Not covered: values, enum lookup, text or
numeric meaning, references, runtime restoration and world import. Offsets are
build-25621760 object offsets or code addresses unless stated as serialized bytes.

## Confirmed

Entry paths and failure reporting

- By-name entry `LoadProspect` (`0x14146e590`) -> `LoadProspectByName`
  (`0x14146e6c0`) rejects empty names, strips a trailing `.json` suffix
  (`RemoveFromEnd`, `IgnoreCase`), loads `.json` via `LoadProspectSaveHeader`
  (`0x141824760`), verifies `Stricmp(ProspectID, Name) == 0`, and transitions
  into `0x1414831f0` -> `ReadFromProspectBlob` (`0x1418e6240`). From-file entry
  `SetupLoadedProspect` (`0x1414baf60`, called from `AIcarusGameMode::InitGame`
  `0x1414b4600`) requires both `LoadProspectSaveHeader` (`0x141824760`) and
  `LoadLocalProspectState` (`0x141823fb0`, where `ReadFromArchive` `0x1418e6140`
  preserves its `bool` return in `ebx`) to succeed before setting `bHasLoaded`;
  either failure enters reporting (F-325, F-326, F-717; U-056, U-057 resolved;
  `normalize_prospect_name`, `evaluate_load_prospect_by_name` and
  `evaluate_setup_loaded_prospect` in `experimental::restore_rules`).
- Reporting is not log-only: it can force termination or raise (F-327). By-name
  missing and truncated inputs were rejected without an observed crash (F-256).

Inner state, version and archive mode

- State vtable `0x1449e5c88` registers `ProspectSaveState`; header vtable
  `0x1449e5c70` registers `ProspectSaveStateHeader`; sizes `0xf8`/`0xe8` (F-328, F-329).

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
  `BinaryData` at `0x10`; descriptor order is not serialized order (F-330, F-332).
- Dispatcher `0x1418ee4a0` picks the tagged serializer unless archive byte `+0x28`
  bit `0x20` is set; reset clears `0x20` and loading sets `0x01`, so this reader
  uses tagged traversal through tag reader `0x14215f530` (F-335, F-336, F-257).
  `UStruct::SerializeVersionedTaggedProperties` (`0x14200f5b0`) reads
  `FPropertyTag` headers until `None`, wrapping `PropertyLinkNext` (`+0x58`) back
  to head `PropertyLink` (`plVar2[0xe]`) on out-of-order tags and keeping
  constructor defaults when omitted (F-718; U-062 resolved).
- `ReadFromProspectBlob` (`0x1418e6240`) starts `FMemoryReader` at byte 0, and
  `UsingCustomVersion` reads no table on load, so `Version = 2` is the
  `FProspectSaveState` schema version, unrelated to game build (F-260, F-715;
  U-060 resolved). `PostLoadSerialize` (`0x1418e4250` / `0x1418e44f0`) copies
  legacy `ProspectID` (`+0xc8`) and `ProspectDTKey` (`+0xd8`) into `ProspectInfo`
  via `0x14089a1c0` when `Version < 2`, and at `Version == 0` resets
  `ProspectInfo` (`0x14170a4c0`), sets `LobbyPrivacy` (`+0x0c`) = `1`
  (`ELobbyPrivacy::FriendsOnly`) and clears `StateRecorderBlobs` (`+0xe8`), then
  stamps `Version = 2` (F-715; U-059 resolved; `migrate_prospect_save_state` in
  `experimental::restore_rules`).

Recorders and base component

- Each recorder is a `None`-terminated tagged sequence after the array's single
  shared inner tag; no native or post-serialization hook (F-340).
- `BinaryData`: signed 32-bit count, then that many raw bytes (F-341). Scans reach
  the exact recorder-array boundary: 2,744/2,741 recorders (F-245, F-257).
- Restore `0x1414b83c0` resolves the class string, constructs the component and
  calls `+0x4a0`; failures return null (F-342). Base `+0x4a0` = `0x1418a9c80` reads
  `BinaryData` through `+0xa0` = `0x1418aaf70`; no exact EOF check (F-343).
- Serialization reaches UClass slot `+0x2a0` = `0x14200ea70`, tagged (F-344). Flag
  `0x04000000` skips, by declared size, fields lacking property flag `0x01000000`
  and can enable name/type remapping (F-345).
- Trailer: four-byte boolean; nonzero adds a 16-byte GUID. Property termination is
  not component EOF; the original accepts any nonzero, its fallback flags values > 1
  (F-346). Base post-load `+0x488` is return-only; derived classes may override
  (F-347). Client PDB names agree: on C001 `+0x4a0` is the state restore, `+0xa0`
  Serialize(FArchive&), `+0x488` a version-upgrade hook (F-669).

C013 (singleton)

- Inherits base load, serialize and empty post-load slots (F-716). NameProperty is
  built-in name ID 6, no type-specific metadata, then the property-GUID field
  (F-350, F-246).
- Both 548-byte bodies: five properties, 312 header/terminator bytes, 232 opaque
  value bytes, zero four-byte trailer, exact EOF (F-351, F-248). Selection-bit
  descriptors: one struct with one integer child plus enum, name, string, bool;
  five of 31 ancestry entries (F-352, F-353).
- `UFLODRecorderComponent` saves `AFLOD::FLODTiles.Num()` (`+0x250`) to `+0xd8`
  on record (`0x1418a1850`), compacts `FLODTiles` (`+0x248`) on stitch
  (`0x1418ad0c0`), and never reads `+0xd8` on restore (F-646, F-716; U-039
  resolved).

C028 (repeated)

- Registration `0x1406ff5d0`, getter `0x141d2d770`, callback `0x141d2e710`,
  constructor `0x1418c22f0`, vtable `0x144a70e90`: `+0x4a0` = `0x1418a9bb0`
  (`UActorStateRecorderComponent::RestoreState`) calls base `0x1418a9c80` then
  invokes `+0x4b0` (`ApplyRecorderTransform`) only when the context `FTransform`
  differs from `Identity` (`> 1e-4`, skipped in world prospect restore); `+0xa0`
  is base; `+0x488` = `0x14189d380` (`UpgradeVersion`) renames legacy
  `TreePrimitive` to `Wood` when `recorder_version < 1` without reading the
  archive (F-645, F-716; U-061 resolved; `should_apply_recorder_transform` and
  `upgrade_actor_recorder_version` in `experimental::restore_rules`).
- 60 ancestry entries (3/27/11/19/0); 23 selection-bit fields: 3 Int, 3 Name,
  2 Bool, 6 Struct, 7 Array, 1 Enum, 1 Str (F-655). All 1,294 records frame:
  29,762 fields, 4,326,044 bytes; per copy 647 components (F-259, F-363, F-364, F-365).
- Six direct struct types are tagged under `MemoryArchive522`; their 16 children
  (3/7/1/1/1/3): 3 Struct, 1 Name, 4 Int, 7 Bool, 1 UInt32 (F-655). `UInt32Property`
  is built-in ID 25 (F-368, F-247). Binary N01 is four ordered four-byte
  little-endian transfers (16 bytes), N02 three (12 bytes), no count, terminator or
  tail (F-369, F-124).
- Array serializer `0x1421580d0`: signed 32-bit count, one shared struct tag even
  at count 0, then `None`-terminated elements without object trailers; the shared
  size spans all elements (F-370, F-720; U-030 resolved). Shared tag: index 0,
  inner name = field name, element struct name, equality not enforced (F-371).
  Children: 7 Name, 3 Int, 3 Float, 1 Bool, 1 Text, 1 Array, 1 Struct (F-655);
  D01 eight optional fields, D02 four four-byte transfers (F-655, F-373).
- All 9,058 arrays are empty: 848,864 bytes = 36,232 count + 812,632 shared-header
  (F-374, F-116). Top and child receipts: F-123..F-141.

C034

- Base load/serialize, C028 post-load; 21 optional fields: 7 Struct, 2 Int, 7 Array,
  2 Name, 1 Enum, 1 Str, 1 Bool (F-664, F-211). Per copy 829 records, 4,199,203 bytes,
  17,409 fields (F-376, F-378, F-195); own structs 572–28,606 bytes (F-377, F-379, F-188).
- Own array child `0x145ee1220`: tagged optional Struct (N02, 12 bytes) and Float
  (four bytes), signed count and shared Struct header even at zero; its 16-byte
  memory size is not a stride, and server 25621760 matches client 25793767 on
  `FFloatProperty::SerializeItem` (`0x142158010`), `FVector` (`0x142128a70`) and
  CRT struct-ops registration (F-659, F-664, F-720; U-029, U-031 resolved); max
  count 214 (F-381, F-382).
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
  binary widths 16/12/12/16 (F-172); ND01 cap 66,056 (F-398, F-165..F-167);
  descriptor fields F-399.
- D01 leaves and arrays: F-155..F-162, F-400.

Acceptance conditions for any importer

- Stage, resolve and validate the full supported schema before publishing, with
  distinct errors, preserved source and native state, never an empty world, and
  unmapped content-dependent fields reported as incompatibility (F-403).
  `import-save` stays unavailable until field coverage and stock-client validation
  demonstrate correct migration (F-229, F-269).
- Strict readers enforce binary transfer lengths and treat name/index/type equality
  and 0/1-only trailer booleans as comparison policy (F-346, F-369, F-371).
- Reader limits: component and struct-array default 64 KiB, explicit limit up to a
  hard 1 MiB ceiling (F-634) (measured caps 71,956, 66,056; D01 helper 1,050 bytes),
  4,096 elements and 256 fields per element, Int-array 4,096 elements, root 4,096
  tags, seven-field post-parse helper check and a 512 MiB native address-space test
  limit (F-134, F-161, F-178, F-198, F-200, F-249, F-382).

## Candidate

- Base-component framing is verified across builds for C001, C013, C028 and C034
  (F-669, F-670, F-716, F-720, F-722); native interpreted import stays gated
  (U-025, U-045).
- C001 (four records): getter `0x1419fe200` (`.pdata` 103518), construction
  callback `0x1419fe870` (305 B, `.pdata` 103538) and constructor `0x141788530`
  (296 B, `.pdata` 95461) are complete functions that call base constructor
  `0x1418824d0` (`rbx` kept) and install vftables `0x1449d9f90` / `0x1449da448`,
  whose `+0x4a0` (`0x1418a9bb0`) -> `0x1418a9c80` -> `+0xa0` (`0x1418aaf70`) ->
  `+0x488` (`0x14189d380`) bridge is verified in
  [C001 digest](../../research/save-format/c001-constructor.md) (F-469 to F-477,
  F-509, F-534, F-669, F-670, F-721, F-722; U-003 to U-006, U-023, U-041, U-042
  resolved).

## Open questions

- Runtime failure-reporting reachability and process termination vs return on
  disposable fixtures (U-058).
- Controlled reproduction matrix for both entry paths, retaining command/path,
  input digest, truncation point, logs, exit status and before/after state
  against an unchanged valid fixture, with Proton/platform effects separated
  (U-063, U-047).
- Values, empty families and other classes (U-020, U-021, U-022, U-032, U-046, U-048, U-050);
  native interpreted import and newer-build compatibility (U-025, U-045).

## Evidence

- Static: M-147, M-148, M-143, M-149, M-150, M-119, M-122, M-123, M-079, M-094,
  M-096, M-100, M-170, M-755..M-762; runtime and reports: M-154, M-155, M-160.
- Framing receipts: M-118, M-172, M-087..M-093, M-095, M-097..M-103, M-106,
  M-058..M-070, M-049..M-055; C001: M-007, M-008, M-011, M-013, M-173, M-179, M-180, M-181, M-761, M-762.
- Tests: `tests/tagged.rs`, `tests/recorders.rs`, `tests/component.rs`,
  `tests/struct_array.rs`, `tests/int_array.rs`, `tests/archive_frame.rs`,
  `tests/restore_rules.rs`, `tests/prospect.rs`, `tests/cli.rs`.

```bash
cargo test --locked --test tagged --test recorders --test component \
  --test struct_array --test int_array --test archive_frame --test restore_rules --test prospect
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --recorders
```
