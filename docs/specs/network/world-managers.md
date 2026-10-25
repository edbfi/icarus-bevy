# World manager opens

## Status

Experimental: wire-checked against the 3.0.31 capture (F-859, F-861,
F-862, F-866, F-914, F-933, F-934) and static observations of server build
25793607 (F-864, F-865, F-933, F-934) and the 3.0.31/3.0.32 server paks
(F-935). Last reconciled: 2026-10-25.

## Scope

The actor opens of the managers the GameState references (F-794) and the
quest actors the QuestManager references (F-934): their archetypes,
components and replicated values, and which of a save's records they carry.
Framing is in [actor channel bunches](actor-channel.md).

## Confirmed

- Every manager is a dynamic actor with roles 1 and 3 and a created-only
  `Stat Container`; Icarus actors add an `ActorState` (handles 16 and 17,
  `CurrentBiome` row 0). An archetype outer the connection already holds is
  exported with only the bare checksum flag (F-859;
  `experimental::manager_open`).
- Quest actors (`AQuest`, `experimental::quest_open`, `quest_table`):
  `C019::RecordOwnerState` (`0x1418e8de0`, M-1029/ev_781838ae9011) records
  `IcarusActorGUID`, `ActorClassName`, `ActorTransform`, `QuestName`,
  `RelevantActors`, `RelevantCharacters`, `SubQuestRecords` and
  `VariableRecords`. On load, `C019::StitchingOperation` (`0x1418f66a0`,
  M-1029/ev_f6fb50604a40) wires `SubQuests` (`FName` canonical `_<decimal>`
  suffix split to `number = parsed + 1`) and `QuestParent` on live `C019`
  actors opened in reverse `C019` order (`bQuestComplete` false, matching CDO
  `0`, omitted on wire); `C018::StitchingOperation` (`0x1418f5ef0`,
  M-1029/ev_81be602d4584) binds `AQuestManager::InitialQuest` to the live
  `C019` actor matching `InitialQuestRecord` (null when zero `C019` records
  remain); and `AQuestManager::Reload` (`0x1418ecd30`, M-1029/ev_539ee5e69452;
  `Initialise` `0x1418dd710`, M-1029/ev_da9cdcb1548f;
  `InitialiseModifiers` `0x1418dd740`, M-1029/ev_8256054d4787;
  `Check_Implementation` `0x14089a1d0`, M-1029/ev_6cc23f5685a9;
  `GetBooleanVariable`/`GetFloatVariable`/`GetIntVariable`/`GetQuestFlag`
  `0x1418d39c0`/`0x1418d4000`/`0x1418d4470`/`0x1418d8460`,
  M-1029/ev_86a436f88d36, ev_c41fa5b50531, ev_7fedbcff43cb, ev_1cc2919010b1)
  runs `RunQuest` (F-864), populating `ActiveQuests` where `Check()` is false.
  Across `3.0.31`/`3.0.32` server paks all `2,044` `D_Quests` checksums and CDO
  guards match (`1,774` keep `4033596864` incl. `Quest` and case-resolved
  `BPQ_Extermination_Croc_C`; `270` add `CPF_Net` without user-struct arrays),
  and Kismet `Check()` compiles `1,635` pure + `44` false-only rules vs `365`
  `Impure`, matching `8/8` M-707 quest opens + ch `126` bit for bit (F-934,
  F-935, M-1030; U-074 resolved, U-076).
- QuestManager (`Default__QuestManager`, checksum 3539440926): handles 19..26
  carry the save's C018 record: the faction mission's 1-byte
  `D_FactionMissions` row, nondefault difficulty, flags and delay,
  `InitialQuest` (`ActorKey::Saved`, or null when no live `C019` root remains)
  and the keyed relevant persistent actors and characters as `(Str, Object)`
  arrays, each actor keyed by its saved Icarus actor GUID (`ActorKey::Saved`,
  apart from domain ids; F-861, F-934; `with_quest_state`). A saved faction
  mission the tables lack or an unopenable `C019` quest skips the QuestManager.
- PlayerHistoryTracker (`Default__PlayerHistoryTracker`, checksum 243959002):
  handle 19 `History` carries the save's C024 `SavedHistoryData`, every
  element field (`UserId`, `ChrSlot`, `CachedCharacterName`) in saved order
  (F-862; `with_player_history`). The original appends a player in
  `InitialisePlayerForProspect` unless an entry has the same user ID
  (case-insensitive) and slot; an existing entry is not refreshed (F-865).
- InventoryContainerManager (`Default__InventoryContainerManager`, checksum
  3504939521, no components): handle 19 `Inventories` lists its runtime
  `UInventory` subobjects' GUIDs in block order. Each block sends `bIsActive`,
  its `D_InventoryInfo` row, nonzero `CurrentWeight` (handle 4) and its slots
  (the pawn's slot layout); an item slot sends its `D_ItemsStatic` row (handle
  2 and `LastItem`), its saved `DynamicData` (3) and named `AdditionalStats`
  as `CustomProperties.Stats` (7) and `bIsItemInstance`; the slots' key is 2
  plus the items (F-866, F-933; `with_container_inventories`,
  `network::container_inventories`).
- `C016::RecordOwnerState` (`0x1416f0b50`, M-1027/ev_c0f01470c374) saves
  `bInventoryWantsTick = IsComponentTickEnabled()`. `C016::UpdateOwnerToDatabaseState`
  (`0x1416fccb0`, M-1027/ev_a99e12baab7a) calls `CreateInventoryInIndex`
  (`0x1416d38a0`, M-1027/ev_c60be6e23515: `OwnerPrivate = AInventoryContainerManager`,
  role `3`, `Initialise` setting `bDirtyWeight` and disabling tick; tick
  capability/registration guards in M-903/ev_44975023ea86,
  M-908/ev_aa563a322a20, M-906/ev_be342f7e3311, M-909/ev_dbfc5e7474c5),
  restores `SetComponentTickEnabled(bInventoryWantsTick)` and writes saved
  slots via `SetItem` (`+0x331 = 0x0101`, keeping `bDirtyWeight`). Unticked
  containers (`96/98` in M-707) stay at CDO `CurrentWeight == 0` and omit
  handle 4; ticked ones (`2/98` in M-707) run `RecalculateWeight`
  (`0x14197db50`, M-1027/ev_d89f1ec2a578) and `GetItemWeight` (`0x1416e0460`,
  M-1027/ev_2035950b2f15; `OnContainerManagerUpdated` `0x1419781f0`,
  M-1027/ev_6cb8d023357c), skipping slots without dynamic property `7`
  (`ItemableStack`), summing `(D_Itemable.Weight + fillable 10 on
  Any_Weighable_Resource_Container 0x16c) * stack` and matching all `98/98`
  captured blocks bit for bit (`run` fails closed on ticked custom stats,
  nested `13 >= 0` or static `Weight_+`; F-933, M-1028; U-075 resolved).
- FLOD manager (`Default__BP_IcarusFLOD_Default_C`, checksum 519276901, no
  components): the Blueprint archetype and its package are exported in the
  open, the actor spawns at (0, 0, -100000) as captured, and handle 16 lists
  the save's C013 `NumTiles` tile actors, keyed by position
  (`ActorKey::FlodTile`) and given GUIDs at this open in array order
  (F-914; `with_flod_tiles`).
- `run` (`DEFAULT_MAX_CHANNELS = 256` so multi-stage `C019` DAGs and managers
  fit above `0..=8`, F-858) opens the radiation, world-talent and disaster
  managers after each PlayerState, then atomically reserves and opens any
  stitched `C019` quests with `QuestManager`, followed by
  InventoryContainerManager, PlayerHistoryTracker and FLOD (F-859, F-861,
  F-862, F-866, F-914, F-933, F-934). Across the 15 qualified prospects 6 open
  8 quests each and 9 have 0 `C019` records (`0` skipped quests); list gaps,
  unopenable/overflowing quests or unknown missions skip cleanly (F-867, F-934).

## Candidate

- The 3.0.32 manager checksums are the M-820 derivation's, with the component
  set and order assumed unchanged (F-780).

## Open questions

- Saved `C019` quest actors using the 270 `CPF_Net`-extending classes, the 365
  impure `Check()` graphs, or true values on the 44 `AllowChecksAfterCompletion`
  quests, which fail closed in `run` and skip `QuestManager` (F-934, F-935;
  U-076).
- Appending joining players to `History` in `run`: the user ID and slot of a
  stock client (F-865; U-038).
- Saved container lists with empty positions (`count != containers.len()` or
  non-contiguous `index`, which `run` skips) and live `C016::StitchingOperation`
  nulling of unreferenced containers, whose wire encoding is unobserved
  (F-866, F-867).
- The FLOD tile actors the FLOD manager references, which M-707 never opens
  (F-914).

## Evidence

- M-707, M-820, M-879, M-881 to M-887, M-891, M-892, M-894 to M-896, M-901 to M-913, M-1027 to M-1030.
