# World manager opens

## Status

Experimental: wire-checked against the 3.0.31 capture (F-859, F-861,
F-862, F-866) and static observations of server build 25793607 (F-864,
F-865) and the 3.0.32 server paks (F-863). Last reconciled: 2026-10-22.

## Scope

The actor opens of the managers the GameState references (F-794): their
archetypes, components and replicated values, and which of a save's records
they carry. Framing is in [actor channel bunches](actor-channel.md).

## Confirmed

- Every manager is a dynamic actor with roles 1 and 3 and a created-only
  `Stat Container`; Icarus actors add an `ActorState` (handles 16 and 17,
  `CurrentBiome` row 0). An archetype outer the connection already holds is
  exported with only the bare checksum flag (F-859;
  `experimental::manager_open`).
- QuestManager (`Default__QuestManager`, checksum 3539440926): handles 19..26
  carry the save's C018 record: the faction mission's 1-byte
  `D_FactionMissions` row, nondefault difficulty, flags and delay,
  `InitialQuest` and the keyed relevant persistent actors and characters as
  `(Str, Object)` arrays, each actor keyed by its saved Icarus actor GUID
  (`ActorKey::Saved`, apart from domain ids; F-861; `with_quest_state`).
- PlayerHistoryTracker (`Default__PlayerHistoryTracker`, checksum 243959002):
  handle 19 `History` carries the save's C024 `SavedHistoryData`, every
  element field (`UserId`, `ChrSlot`, `CachedCharacterName`) in saved order
  (F-862; `with_player_history`). The original appends a player in
  `InitialisePlayerForProspect` unless an entry has the same user ID
  (case-insensitive) and slot; an existing entry is not refreshed (F-865).
- InventoryContainerManager (`Default__InventoryContainerManager`, checksum
  3504939521, no components): handle 19 `Inventories` lists its runtime
  `UInventory` subobjects' GUIDs in block order. Each block sends `bIsActive`,
  its `D_InventoryInfo` row and its slots (the pawn's slot layout); an item
  slot sends its `D_ItemsStatic` row (handle 2 and `LastItem`), its saved
  `DynamicData` (3) and named `AdditionalStats` as `CustomProperties.Stats`
  (7) and `bIsItemInstance`; the slots' key is 2 plus the items (F-866;
  `with_container_inventories`, `network::container_inventories`).
- `run` opens the radiation, world-talent and disaster managers after each
  PlayerState, then the QuestManager, InventoryContainerManager and
  PlayerHistoryTracker when the loaded save supplies their records (F-859,
  F-861, F-862, F-866).
- Quest actors: 1,772 of 3.0.32's 2,044 `D_Quests` classes keep native
  `Quest`'s checksum 4033596864, 270 add replicated properties (F-863).
  `AQuest::RunQuest` keeps a sub-quest in `ActiveQuests` only while its
  Blueprint `Check` reports it incomplete (F-864).

## Candidate

- The 3.0.32 manager checksums are the M-820 derivation's, with the component
  set and order assumed unchanged (F-780).

## Open questions

- Opening a save's quest actors: the 270 classes' checksums and each quest's
  `ActiveQuests` without its Blueprint logic (U-074).
- Appending joining players to `History` in `run`: the user ID and slot of a
  stock client (F-865; U-038).
- `CurrentWeight`, sent for 2 of the captured item inventories only, and
  saved container lists with empty positions, which `run` skips (U-075).
- The FLOD manager, which references world actors (F-794).

## Evidence

- M-707, M-820, M-879, M-881 to M-886.
