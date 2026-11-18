# Prospect loading: save dispatch and inner framing

## Status

Experimental; scope dedicated-server build 25621760 (supplied-save build: F-323). Last reconciled: 2026-11-10.
Full text: d5b0ae6:docs/specs/save-format/prospect-loading.md

## Scope

Prospect-load entry paths, `ProspectSaveState` version fixups/dispatch,
recorder/base-component and C013/C028/C034/C009/D01 framing in both copies. Envelope: [envelope](prospect-envelope.md); tags:
[tagged properties](tagged-properties.md); strings: [archive strings](archive-string.md);
restoration: [restore rules](restore-rules.md). Not covered: gameplay data-table
row meanings, reference resolution and world import.

## Confirmed

Entry paths and failure reporting

- By name (F-899, F-733, F-737; U-057, U-063 resolved): `LoadProspect`
  (`0x14146e590`) -> `LoadProspectByName` rejects empty names, strips `.json`
  case-insensitively, reads it via `GetProspectBlob` (`0x141817bd0`) and
  `GetProspectInfo` (`0x14181e530`), and calls `TransitionToProspect`; missing,
  malformed-JSON and ID-mismatch inputs return `false` with state kept. A corrupt
  inner `BinaryBlob` fails in `ReadFromProspectBlob` *after*
  `ResetActiveProspect` (`0x1418333b0`) and `SetActiveProspectInfo`
  (`0x141837100`) overwrite the active `ProspectInfo`.
- From file (F-899, F-733; U-056, U-058, U-063 resolved):
  `SetupLoadedProspect`, called from `InitGame` (`0x1414acc40`) with
  `?LocalSavePath=`, logs Fatal at `0x1414bb15f` when a header or state load
  fails, before `ResetActiveProspect`.

Inner state, version and archive mode

- State and header registration and sizes: F-889. Reflected
  `FProspectInfo` (F-738; `0xa0` B) carries `ProspectID` `+0x00`,
  `ClaimedAccountID` `+0x10`, `ClaimedAccountCharacter` `+0x20`, `ProspectDTKey`
  `+0x28`, `FactionMissionDTKey` `+0x38`, `LobbyName` `+0x48`, `ExpireTime`
  `+0x58`, `ProspectState` `+0x60`, `AssociatedMembers` `+0x68`
  (`FAssociatedMemberInfo`, `0x40` B: `AccountName` `+0x00`, `CharacterName`
  `+0x10`, `UserID` `+0x20`, `ChrSlot` `+0x30`, `Experience` `+0x34`, `Status`
  `+0x38`, `Settled` `+0x39`, `IsCurrentlyPlaying` `+0x3a`), `Cost` `+0x78`,
  `Reward` `+0x7c`, `Difficulty` `+0x80`, `Insurance` `+0x84`, `NoRespawns`
  `+0x85`, `ElapsedTime` `+0x88`, `SelectedDropPoint` `+0x8c` and
  `CustomSettings` `+0x90` (`0x18` B).
- Root fields (object offsets F-889): `Version` (integer tested by load
  fixups), `LobbyPrivacy` (enum, byte underlying descriptor), `ProspectInfo`
  (`FProspectInfo`), `ProspectMapName` (string), `LastSavedDateTime` (date/time),
  legacy strings `ProspectID` and `ProspectDTKey` (`0xd8`) for pre-version-2 fixups, and
  `StateRecorderBlobs` (`0xe8`, array of a derived state type).
- `StateRecorderBlob` layout: F-884; descriptor order is not serialized order
  (F-889, F-884).
- Dispatch uses tagged traversal (F-257, F-890, F-718; U-062 resolved):
  out-of-order tags wrap the property search; omissions keep defaults.
- Version (F-885, F-715; U-059, U-060 resolved; `migrate_prospect_save_state`):
  `Version = 2` is the schema version, unrelated to game build.
  `PostLoadSerialize` (`0x1418e4250` / `0x1418e44f0`) at `Version == 0` also sets
  `LobbyPrivacy = 1` (`FriendsOnly`), clears `StateRecorderBlobs` and stamps
  `Version = 2`.

Recorders and base component

- Recorder array and `BinaryData` framing: [tagged properties](tagged-properties.md)
  (F-883, F-257, F-884).
- Restore resolves and constructs the class; `+0x4a0` then reads tagged
  `CPF_SaveGame` properties, a 4-byte GUID-trailer boolean and the `+0x488`
  version-upgrade hook (addresses: F-890, F-891, F-909).

C013, C028, C034, C009 and D01

- `C013` (`UFLODRecorderComponent`; F-887, F-350..F-353, F-646, F-716; U-039
  resolved): base load, serialize and post-load; two 548-byte bodies
  (5 properties, zero trailer, exact EOF); record, stitch and restore of
  `FLODTiles` per F-716.
- `C028` (`UActorStateRecorderComponent` subclass; F-645, F-716; U-061
  resolved): `+0x4a0` calls `ApplyRecorderTransform` and `+0x488` upgrades
  `TreePrimitive` (addresses: F-716). Every body
  frames (F-259, F-655; binary `N01` 16 B / `N02` 12 B; F-116, F-123..F-141,
  F-363..F-374, F-720; U-030 resolved).
- `C034`: base load/serialize, `C028` post-load; 829 records per copy; fields
  and own struct array per F-664 and F-380, matching across builds
  (F-146..F-153, F-188, F-195, F-196, F-211, F-376..F-385, F-659, F-720; U-029,
  U-031 resolved).
- `C009` and `D01` (`FInventorySlotSaveData`; field counts and sizes F-386,
  F-387, F-167, F-203, F-655): `S02` `ArrayProperty<IntProperty>` + `Bool`, `S01`
  `Str` + `Int`; parents `PA01..PA04`/`PS01`; common `CA01` and `ND01`;
  `EDynamicItemProperties` `0..=13` on restore (F-155..F-203, F-386..F-400,
  F-728, F-735; U-020 resolved).

Acceptance conditions for any importer

- Stage, resolve and validate the full supported schema before publishing, with
  distinct errors, preserved source and native state, never an empty world, and
  unmapped content-dependent fields reported as incompatibility (F-771, F-269,
  F-403). Strict readers enforce binary transfer lengths and
  0/1 trailer booleans under explicit byte/element caps up to 1 MiB
  (F-134..F-200, F-888, F-346..F-382, F-891, F-900).

## Candidate

- `C001InertPlacementV1` accepts an explicitly bound authored fixture only
  (F-921; storage and limits: [authored-worlds runbook](../../runbooks/authored-worlds.md#spatial-worlds-and-inert-source-fixtures)).
  It requires source version 2, recorder version 3, exact C001 identity, neutral
  prospect behavior and complete one-to-one native placement coverage. Class,
  path, object-name and GUID bind exactly; no CRC-only dispatch, dropped record,
  partial result, respawn or stitching is admitted. The closed component fields
  are class/path/object-name, positive GUID, recorded version, false sublevel
  flag and complete transform, with optional supported find policy and false
  skip-owner flag; any other field rejects. Independently authored inert
  geometry has no variables, and native-frame coordinates transfer without
  conversion. Schema 5 keeps the preserved source and immutable identity,
  timestamp, root-presence and binding metadata beside mutable poses,
  cross-checked on reload. Original coordinates, official loadability and
  faithful save import stay unqualified.
- `DecodedImportCandidate` (F-920; limits:
  [native server](../../runbooks/native-server.md#supplied-saves)) retains
  migrated roots, exact recorder identities, decisions, optional typing and
  unchanged preserved bytes from one decode. The import plan reports
  `unresolved_prospect_info_enums` when a named or raw enum remains unknown
  after migration. Version 0 resets the prospect info before this check.
  Restoration adapters remain unqualified and native world publication stays disabled.
- Dry-run quest checks cover typed, retained recorders (synthetic F-1263).
  `recorders.rs` rejects duplicate top-level names and nonzero array
  indices before typing. Wrong types for the required `NameProperty` fields
  count toward `unexpected_recorder_row_field_types`. Absent fields, empty
  names and `None` add no references. C019 `QuestName` takes the first ASCII
  case-insensitive `D_Quests` match below index 65,535. C018
  `FactionMissionName` takes the first exact `D_FactionMissions` match at 0–255
  ([protocol](../network/world-managers.md#confirmed)). Both feed `data_row`
  counts and `unmapped_content`.
- Authored catalogs (formats 1 and 2) accept either table in `[row_tables]`
  as an array of up to 65,536 names (1–1,024 UTF-16 units), indexed from zero.
  When both sources supply rows, differing lengths or positions under table
  lookup rules count affected references in `conflicting_recorder_row_tables`.
  Ambiguous paths and rowless pak members leave references unmapped. Absent
  members permit authored coverage. Generic lookup scans flat `data_row`
  lists and all pak JSON tables by exact path, including duplicate filenames.
  Read or parse errors stop the scan.
- Native staging (`inspect-save --stage-check`):
  [envelope spec](prospect-envelope.md#candidate) (F-738).
- `import-save --dry-run` totals occupied and empty `container_positions`
  when all retained C016 lists are assessable (F-651). Negative lengths,
  conflicting declarations, duplicate or out-of-range positions in typed
  lists add `invalid_container_positions`. Untyped recorders still block.
- Retained exact C026 records enable count-only `character_container_links`.
  Explicit `AssociatedInventoryId` −1 (selector 13, F-1066) counts as
  `no_link`. With exactly one retained, assessable C016, nonnegative IDs
  count as `occupied`, `unoccupied` (gap) or `out_of_range` by
  `InventoryIndex` (default zero for a present entry).
  A qualified array with no link selector adds no count. Unsupported arrays,
  invalid selectors, duplicate or aliased selector 13, missing link values,
  IDs below −1 and nonnegative IDs without that manager count as `unknown_items`.
  Typing or inventory array failures count as `unknown_recorders`, slot array
  failures as `unknown_inventories`. Live links and owner binding remain open
  (U-069, U-045).
- Cross-build classes/defaults/overrides: [restore rules](restore-rules.md)
  (F-902); constructor provenance: [C001 digest](../../research/save-format/c001-constructor.md)
  (F-905..F-910).

## Open questions

- Original-server crash reproduction (U-047), supplied-save producing-build log
  confirmation (U-040), and native interpreted import / newer-build compatibility
  (U-025, U-045).

## Evidence

- Static: M-079..M-170, M-755..M-800; runtime: M-154, M-155, M-160; framing:
  M-049..M-118, M-172; C001: M-007..M-013, M-173, M-179..M-181, M-761, M-762.
- Tests: the [tagged-property suites](tagged-properties.md#evidence),
  `tests/save_format/restore_rules.rs`, `tests/cli.rs`.
