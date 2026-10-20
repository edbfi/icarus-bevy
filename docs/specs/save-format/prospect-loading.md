# Prospect loading: save dispatch and inner framing

## Status

Experimental; scope dedicated-server build 25621760 (supplied saves are user-reported from 25793767). Last reconciled: 2026-10-19.
Full text: d5b0ae6:docs/specs/save-format/prospect-loading.md

## Scope

The two original prospect-load entry paths, inner `ProspectSaveState` type and
version fixups, archive-mode dispatch, recorder and base-component framing, and
C013, C028, C034 and C009 (with nested D01) framing across the two supplied
copies. Envelope: [envelope inspection](prospect-envelope.md); tags:
[tagged properties](tagged-properties.md); strings: [archive strings](archive-string.md);
restoration: [restore rules](restore-rules.md). Not covered: gameplay data-table
row meanings, reference resolution and world import.

## Confirmed

Entry paths and failure reporting

- By-name entry `LoadProspect` (`0x14146e590`) -> `LoadProspectByName`
  (`0x14146e6c0`) rejects empty names, strips `.json` (`IgnoreCase`), reads
  `.json` twice (`GetProspectBlob` `0x141817bd0`, `GetProspectInfo` `0x14181e530`),
  checks `Stricmp(ProspectID, Name) == 0`, and calls `TransitionToProspect`
  (`0x1414831f0`): missing, malformed-JSON, and ID-mismatch inputs return `false`
  with entry state preserved, whereas a corrupt inner `BinaryBlob` fails inside
  `ReadFromProspectBlob` (`0x1418e6240`) *after* `ResetActiveProspect`
  (`0x1418333b0`) and `SetActiveProspectInfo` (`0x141837100`) overwrite active
  `ProspectInfo` (F-717, F-733, F-737; U-057, U-063 resolved).
- From-file entry `SetupLoadedProspect` (`0x1414baf60`, from `InitGame`
  `0x1414acc40` with `?LocalSavePath=`) skips when `LocalSavePath.Num() <= 1`;
  when `> 1` and either `LoadProspectSaveHeader` (`0x141824760`) or
  `LoadLocalProspectState` (`0x141823fb0`) fails, `0x1414bb15f` logs Fatal
  `0x1449f6660` and calls `0x141ee8930` (`GIsCriticalError = 1` ->
  `RequestExit` / `RaiseException`) before `ResetActiveProspect` (F-717, F-732,
  F-733; U-056, U-058, U-063 resolved).

Inner state, version and archive mode

- State vtable `0x1449e5c88` registers `ProspectSaveState`; header vtable
  `0x1449e5c70` registers `ProspectSaveStateHeader`; sizes `0xf8`/`0xe8` (F-328, F-329).
  Reflected `FProspectInfo` (`0x145fae2d0`, `0xa0` B, 17 fields, F-738) carries
  `ProspectID` (`+0x00`), `ClaimedAccountID` (`+0x10`), `ClaimedAccountCharacter`
  (`+0x20`), `ProspectDTKey` (`+0x28`), `FactionMissionDTKey` (`+0x38`),
  `LobbyName` (`+0x48`), `ExpireTime` (`+0x58`), `ProspectState` (`+0x60`),
  `AssociatedMembers` (`+0x68`, `FAssociatedMemberInfo` `0x145fad8e0`, `0x40` B:
  `AccountName` `+0x00`, `CharacterName` `+0x10`, `UserID` `+0x20`, `ChrSlot`
  `+0x30`, `Experience` `+0x34`, `Status` `+0x38`, `Settled` `+0x39`,
  `IsCurrentlyPlaying` `+0x3a`), `Cost` (`+0x78`), `Reward` (`+0x7c`),
  `Difficulty` (`+0x80`), `Insurance` (`+0x84`), `NoRespawns` (`+0x85`),
  `ElapsedTime` (`+0x88`), `SelectedDropPoint` (`+0x8c`), and `CustomSettings`
  (`+0x90`, `0x145fadbf0`, `0x18` B).

| Field | Object offset | Role |
| --- | --- | --- |
| `Version` | `0x08` | Integer tested by load fixups |
| `LobbyPrivacy` | `0x0c` | Enum, byte underlying descriptor |
| `ProspectInfo` | `0x10` | Nested prospect metadata (`FProspectInfo`, `0xa0` B, F-738) |
| `ProspectMapName` | `0xb0` | String |
| `LastSavedDateTime` | `0xc0` | Date/time |
| `ProspectID` | `0xc8` | Legacy string, pre-version-2 fixups |
| `ProspectDTKey` | `0xd8` | Legacy string, pre-version-2 fixups |
| `StateRecorderBlobs` | `0xe8` | Array, derived state type |

- `StateRecorderBlob` is `0x20` B (`ComponentClassName` at 0, `BinaryData` at
  `0x10`); descriptor order is not serialized order (F-330, F-332).
- Dispatcher `0x1418ee4a0` uses tagged traversal via `0x14215f530` (F-257,
  F-335, F-336). `UStruct::SerializeVersionedTaggedProperties` (`0x14200f5b0`)
  reads `FPropertyTag` headers until `None`, wrapping `PropertyLinkNext` (`+0x58`)
  to head `PropertyLink` on out-of-order tags and keeping defaults when omitted
  (F-718; U-062 resolved).
- `ReadFromProspectBlob` (`0x1418e6240`) starts `FMemoryReader` at byte 0;
  `Version = 2` is the schema version, unrelated to game build (F-260, F-715;
  U-060 resolved). `PostLoadSerialize` (`0x1418e4250` / `0x1418e44f0`) copies
  legacy `ProspectID` and `ProspectDTKey` into `ProspectInfo` when `Version < 2`,
  and at `Version == 0` resets `ProspectInfo`, sets `LobbyPrivacy = 1`
  (`FriendsOnly`), clears `StateRecorderBlobs`, and stamps `Version = 2` (F-715;
  U-059 resolved; `migrate_prospect_save_state`).

Recorders and base component

- Each recorder is a `None`-terminated tagged sequence after the array's shared
  inner tag; `BinaryData` is a signed 32-bit count and raw bytes (2,744 / 2,741
  recorders across the two copies; F-245, F-257, F-340, F-341).
- Restore `0x1414b83c0` resolves the class string, constructs the component and
  calls `+0x4a0` (`0x1418a9c80` -> `+0xa0` = `0x1418aaf70` -> `0x14200ea70`
  tagged with `CPF_SaveGame` `0x01000000`, then a 4-byte GUID-trailer boolean
  and `+0x488` version-upgrade hook; F-342..F-347, F-669).

C013, C028, C034, C009 and D01

- `C013` (`UFLODRecorderComponent`): base load/serialize/post-load; two 548-byte
  bodies (5 properties, zero trailer, exact EOF); saves `FLODTiles.Num()` to
  `+0xd8` on record (`0x1418a1850`), compacts `FLODTiles` on stitch
  (`0x1418ad0c0`), and ignores `+0xd8` on restore (F-248, F-350..F-353, F-646,
  F-716; U-039 resolved).
- `C028` (`UActorStateRecorderComponent` subclass): `+0x4a0` = `0x1418a9bb0`
  calls `+0x4b0` (`ApplyRecorderTransform`) when context `FTransform` !=
  `Identity` (`> 1e-4`), and `+0x488` = `0x14189d380` upgrades `TreePrimitive`
  -> `Wood` if `recorder_version < 1` (F-645, F-716; U-061 resolved). All 1,294
  bodies frame (23 fields; 6 tagged direct structs; binary `N01` 16 B / `N02`
  12 B; 9,058 empty struct arrays via `0x1421580d0`; F-116, F-123..F-141, F-259,
  F-363..F-374, F-655, F-720; U-030 resolved).
- `C034`: base load/serialize, `C028` post-load; 21 optional fields; 829 records
  per copy; struct-array child `0x145ee1220` carries tagged optional `N02` (12 B)
  + `Float` (4 B) across both builds (F-146..F-153, F-188, F-195, F-196, F-211,
  F-376..F-385, F-659, F-664, F-720; U-029, U-031 resolved).
- `C009` and `D01` (`FInventorySlotSaveData`): 31 optional top fields (486 bodies,
  max 71,956 B; `S02` `ArrayProperty<IntProperty>` + `Bool`, `S01` `Str` + `Int`;
  parents `PA01..PA04`/`PS01`; common `CA01` up to 66,257 B and `ND01` up to
  66,056 B; `D01` 8 optional fields and 14 `EDynamicItemProperties` `0..=13` on
  restore; F-155..F-203, F-386..F-400, F-728, F-735; U-020 resolved).

Acceptance conditions for any importer

- Stage, resolve and validate the full supported schema before publishing, with
  distinct errors, preserved source and native state, never an empty world, and
  unmapped content-dependent fields reported as incompatibility (F-229, F-269,
  F-403). Strict readers enforce binary transfer lengths and 0/1 trailer booleans
  under explicit byte/element caps up to 1 MiB (F-134..F-200, F-249, F-346..F-382,
  F-634).

## Candidate

- `stage_prospect_save_state` (`experimental::recorders`), `stage_prospect`
  (`prospect`), and `icarus-server inspect-save --stage-check` validate the
  `FProspectSaveState` root schema, out-of-order tags, `PostLoadSerialize`
  migration, 17 `FProspectInfo` tags, optional `ProspectID` matching, base
  framing, 65-class ctor defaults + tag overlays (`bSkipDefaultOwnerResolution`,
  `OwnerResolvePolicy`, `ParentCharacterUID` -> `RequiresStitching`), per-recorder
  `Validate` (`+0x408`, incl. `N018` `false` and `N022` rocket-spawn), and `C033`
  `bIsPhysicsDynamic` counts across all 15 saves (`15 / 15`; `dddd.json`: `2,744` ->
  `1` invalid `C031`, `2,743` retained (`971` skip-owner, `52` `FindOnly`, `798`
  `RespawnOnly`, `658` `FindOrRespawn`, `264` `ManuallyResolved`, `821` stitching),
  `2` dynamic `C033` -> `2,741` on `M-691`; `world_imported: false`, F-680, F-736,
  F-738, F-746, F-747).
- All 65 `RecorderComponent` `FClassParams`, ctor defaults and vtable overrides
  ([restore rules](restore-rules.md)), and `C001` provenance
  ([C001 digest](../../research/save-format/c001-constructor.md)) match across
  builds (F-469..F-747; U-003..U-006, U-020..U-024, U-032, U-041, U-042, U-046,
  U-048, U-050 resolved).

## Open questions

- Original-server crash reproduction (U-047), supplied-save producing-build log
  confirmation (U-040), and native interpreted import / newer-build compatibility
  (U-025, U-045).

## Evidence

- Static: M-079..M-170, M-755..M-800; runtime: M-154, M-155, M-160; framing:
  M-049..M-118, M-172; C001: M-007..M-013, M-173, M-179..M-181, M-761, M-762.
- Tests: `tests/tagged.rs`, `tests/recorders.rs`, `tests/component.rs`,
  `tests/struct_array.rs`, `tests/int_array.rs`, `tests/archive_frame.rs`,
  `tests/restore_rules.rs`, `tests/prospect.rs`, `tests/cli.rs`.
