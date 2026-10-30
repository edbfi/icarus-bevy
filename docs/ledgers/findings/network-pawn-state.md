# Findings: Network pawn state

Restored stats and temperature transitions. IDs remain in the shared finding space.

Before restructure: 9f684fb:docs/ledgers/findings/network.md

Last reconciled: 2026-10-30

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-983 | Offline 3.0.32's restore sends 90 pawn stats (16 changed, 32 added; key 58 to 222). Record 2's starting, equipped and module item, set-bonus and modifier grants via F-839's formulas and Base pass-throughs give 57; 23 unread formulas, weight and held-tool state and gear attachments' grants remain | observed | runtime-capture, static-bytes; M-1110, M-1116 | high; 1 run | U-069 | active |
| F-984 | Pinned 3.0.32 declares 1004 virtual stats; F-983's 23 keep 3.0.31's inputs and read as pass-throughs, capped sums and tool, time, atmosphere, mount or encumbrance selections; native models them: 58 of record 2's 90 restored stats then equal, each other one needing an unmodelled source | observed | static-decompile, authored-test; M-1110, M-1117 | high; 3.0.32 | U-069 | active |
| F-985 | Record 2's remaining restored-stat grants are the equipped gear's attachments: the items in the C016 inventories their property 13 names, each granting its Attachments row's D_Alterations Stats; with them 70 of 90 equal; overencumbrance, the held tool and scaled modifier grants remain | observed | runtime-capture, static-bytes; M-1110, M-1118 | high; 1 run | U-069 | active |
| F-986 | Pinned 3.0.32 scales a modifier's grant v as sign * CeilToInt(abs((E + 100) * v * 0.01f)), E its final effectiveness, and adds Overburdened (row 21) while WeightCapacity_+ * 1000 < its inventories' CurrentWeight sum; with both, 89 of record 2's 90 restored stats equal | observed | static-decompile, runtime-capture; M-1110, M-1119 | high; 3.0.32; the 90th F-987 | U-069 | active |
| F-987 | Pinned 3.0.32 RecalculateWeight adds 1 per item (not stack) matching D_TagQueries rows 13, 59, 57, 60, 58 (Any_Tool, AnimalCorpse, Radar, Exotic, Extractor) to stats 420, 419, 421, 422, 423 as the inventory's source; record 2's six Item.Tool items give its IsCarryingTool_? 6 | observed | static-decompile, runtime-capture; M-1110, M-1120 | high; 3.0.32 | U-069 | active |
| F-988 | Native stat_restore models a returning pawn's restored stats from F-983 to F-987's sources through F-984's formulas: for record 2 all 90 equal run init-03's after its restore bunch; the restore update does not send them yet | measured | authored-test, runtime-capture; M-1110, M-1121 | high; offline model; item IDs, keys and stat-bounded state not modelled | U-069 | active |
| F-989 | Native restore sends stat deltas and bounds (F-988); record 2 matches init-03. Incomplete inventory/modifier/stat inputs withhold stats and dependent saved levels; independent fields remain | measured | authored-test, runtime-capture; M-1110, M-1122, M-1140 | high; loopback; IDs, keys, Overburdened object differ; no stock-client proof | U-069 | active |
| F-990 | Offline 3.0.32's restore sets a returning pawn's TotalExperience (handle 15) to its character profile's XP, FOnlineProfileCharacter's third property, XP_Debt not subtracted (run init-03: 41); no native setter read copies it. Native run does the same | observed | runtime-capture, static-bytes, authored-test; M-1110, M-1123 | high; 1 run; new players unread | U-069 | active |
| F-997 | 3.0.32 internal-temperature setter snapshots entry, reads GetStat upper then lower, stores its ordered clamp and requests the temperature delegate only if that result differs from entry | observed | static-bytes; M-1144/ev_3d47e345fc06, M-1145 | high; exact 186 B; stat/listener effects and full TemperatureTick unqualified | U-069 | active |
