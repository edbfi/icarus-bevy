# Retired findings: Network (2)

Rows retired from [the active ledger](../network.md) after
[the first retired file](network.md) reached its cap: superseded by a summary row,
execution receipts, or historical session state. Full text is kept and IDs stay
permanent; cite the superseding row where one exists.

Last reconciled: 2026-10-28

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-863 | 3.0.32 D_Quests names 2,044 quest classes, 2,042 rooted at native Quest; 1,772 add no CPF_Net property, so they keep Quest's checksum 4033596864 (all 8 M-707 opened); 270 add some, mostly from common Blueprint bases; 2 do not resolve | measured | static-bytes; M-882, M-820 | high; property flags only, 270 checksums underived | U-074 | superseded-by F-935 |
| F-915 | 3.0.31 inventory tick requires actor owner (+0xa0) and local role byte >=3 unsigned; dirty +0x330 calls RecalculateWeight then clears. GetOwner binds +0xa0 | observed | static; M-895/ev_ef0eeb5824d8, M-904/ev_20ec98a03b35, M-896, M-892 | high; instance activation/load order unproved | U-075 | superseded-by F-933 |
| F-917 | 3.0.31 inventory ctor clears dirty; authority Initialise sets it. Component ctor sets start-tick flag; registration requests enable if flag or state is set | observed | static; M-901/ev_7ce002932bdc, M-892, M-905/ev_e99660acefd4, M-907/ev_ef4f5b87e0a3 | high; instance overrides/load order unproved | U-075 | superseded-by F-933 |
| F-918 | Native experimental inventory weight cache recalculates only when dirty with actor-owner role >=3; an explicit optional weight encodes at handle 4 alongside slots, including signed/zero values | measured | authored-test; M-913 | high; 35 focused tests and manual suite; no automatic load inference or item calculation | U-075 | superseded-by F-933 |
| F-953 | Native `run` takes the client's Server_BeginPlayerInitialisation slot on the controller channel (reliable, ordered; -1, repeats, malformed ignored) and gives a returning pawn its saved C026 UID by a reliable IcarusUID update, never a held UID | measured | authored-test; M-1090 | high; loopback authored peers; profiles, pawn content, failure wire not native | U-069 | superseded-by F-962 |
| F-962 | Native run initialises as the original: slot, user then character profile (replicated; 3 failures close), loadout, bestiary and AssociatedProspect requests; both answers with the pawn open complete it: PlayerCharacterID, then a matching saved character's unheld UID | measured | authored-test; M-1090, M-1096 | high; loopback; payloads, kick wire unsent | U-069 | superseded-by F-975 |
