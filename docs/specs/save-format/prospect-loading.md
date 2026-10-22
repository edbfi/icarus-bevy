# Prospect loading: save dispatch and inner framing

## Status

Experimental; scope dedicated-server build 25621760 (supplied saves are user-reported from 25793767). Last reconciled: 2026-10-22.
Full text: d5b0ae6:docs/specs/save-format/prospect-loading.md

## Scope

Original prospect-load entry paths, inner `ProspectSaveState` version fixups
and dispatch, recorder and base-component framing, and C013, C028, C034 and
C009/D01 framing in the two supplied copies. Envelope: [envelope inspection](prospect-envelope.md); tags:
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
  fails (F-899), before `ResetActiveProspect`.

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
- Dispatch and tagged traversal (F-257, F-890, F-718; U-062 resolved):
  `UStruct::SerializeVersionedTaggedProperties` reads `FPropertyTag` headers
  until `None`, wraps to the head on out-of-order tags and keeps defaults for
  omitted tags.
- Version (F-885, F-715; U-059, U-060 resolved; `migrate_prospect_save_state`):
  `Version = 2` is the schema version, unrelated to game build.
  `PostLoadSerialize` (`0x1418e4250` / `0x1418e44f0`) at `Version == 0` also sets
  `LobbyPrivacy = 1` (`FriendsOnly`), clears `StateRecorderBlobs` and stamps
  `Version = 2`.

Recorders and base component

- Each recorder is a `None`-terminated tagged sequence after the array's shared
  inner tag; `BinaryData` is a signed 32-bit count and raw bytes (F-883, F-257,
  F-884).
- Restore (F-890, F-891, F-909): the class string resolves, the component is
  constructed, and `+0x4a0` (`0x1418a9c80` -> `+0xa0` = `0x1418aaf70` -> `0x14200ea70`) reads
  tagged `CPF_SaveGame` properties, a 4-byte GUID-trailer boolean and the
  `+0x488` version-upgrade hook.

C013, C028, C034, C009 and D01

- `C013` (`UFLODRecorderComponent`; F-887, F-350..F-353, F-646, F-716; U-039
  resolved): base load, serialize and post-load; two 548-byte bodies
  (5 properties, zero trailer, exact EOF); record, stitch and restore of
  `FLODTiles` per F-716.
- `C028` (`UActorStateRecorderComponent` subclass; F-645, F-716; U-061
  resolved): `+0x4a0` = `0x1418a9bb0` calls `ApplyRecorderTransform` and
  `+0x488` = `0x14189d380` upgrades `TreePrimitive` per F-716. Every body
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

- `stage_prospect_save_state` (`experimental::recorders`), `stage_prospect`
  (`prospect`) and `icarus-server inspect-save --stage-check` validate the
  `FProspectSaveState` root schema, out-of-order tags, `PostLoadSerialize`
  migration, 17 `FProspectInfo` tags, optional `ProspectID` matching, base
  framing, 65-class ctor defaults + tag overlays (`bSkipDefaultOwnerResolution`,
  `OwnerResolvePolicy`, `ParentCharacterUID` -> `RequiresStitching`), per-recorder
  `Validate` (`+0x408`, incl. `N018` `false` and `N022` rocket-spawn), and `C033`
  `bIsPhysicsDynamic` counts across all 15 saves (`dddd.json` stage counts
  F-680, F-747; `world_imported: false`; F-736, F-738, F-746).
- All 65 `RecorderComponent` `FClassParams`, ctor defaults and vtable overrides
  ([restore rules](restore-rules.md)), and `C001` provenance
  ([C001 digest](../../research/save-format/c001-constructor.md)) match across
  builds (F-469..F-747, F-902, F-905..F-910; U-003..U-006, U-020..U-024, U-032,
  U-041, U-042, U-046,
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
