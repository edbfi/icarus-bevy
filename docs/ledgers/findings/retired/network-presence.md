# Retired findings: Network presence

Rows superseded by consolidated active findings; IDs and full claims remain.

Last reconciled: 2026-11-08

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1012 | Readiness retains open/startup and initialization transport receipts, queue/channel counts, player phases and blockers; probe preserves progress/final counts | measured | authored-test; M-1181, M-1186, M-1199, M-1202, M-1203, M-1205 | high; opens need reliable-empty; initialization ACKs use batch spans; no client execution/world-entry proof | U-079, U-038 | superseded-by F-1040 |
| F-1037 | Frozen matched capture sent no configured controller startup actions; existing RestartPawn sends field40 after pawn/controller ACK | measured | source/capture; M-1293, M-1302 | high; omitted restart is a candidate; ACK is not execution or world entry | U-038, U-079 | superseded-by F-1038 |
| F-1018 | Visibility internal handling changes name sets, checks packages and can close; validation checks name bounds and long-package validity | observed | static-decompile; M-1208/ev_9e15bbc246e0, M-1208/ev_d7d3d915cd91 | bounded; public server 26313077; callbacks, name codec and complete contract unresolved (M-1209) | U-079 | superseded-by F-1020 |
| F-1040 | Diagnostics retain open/startup/initialization transport, player phases and parsed/rejected/deferred/admitted profile boundaries; unknown blocks retain no identity | measured | authored-test; M-1181, M-1186, M-1199, M-1202, M-1203, M-1205, M-1308 | high; ACK is not execution; unknown blocks are not identified profile traffic; no world-entry proof | U-038, U-079 | superseded-by F-1045 |
| F-1026 | 3.0.35 controller members match qualified 3.0.34 SHA256s: five local net properties and 22 non-overriding net functions | observed | static-metadata; M-1198, M-1265, M-1264/ev_a32f404544fc | high; exact two members only; native ancestry, full field bound and release compatibility remain unqualified | U-038, U-079 | superseded-by F-1032 |
| F-1027 | 3.0.35 ConstructUClass consumes 76-byte headers; four selected classes have property/function totals 36/72, 99/233, 9/68, 4/23. The link consumer steps 16 bytes | observed | static-metadata; M-1267/ev_c1b1a6eb5ca7, M-1268 | high; totals are not net counts; descriptors, ancestry, ordering/checksums and release remain unqualified | U-038, U-079 | superseded-by F-1028 |
| F-1038 | No-startup baseline; restart reached Begin/UserProfile wait; headers show configured BackendProxy relation in four parts, parsed profile0 | observed | bounded-capture/source; M-1293, M-1302, M-1303, M-1304, M-1309, M-1310, M-1315/ev_67e4d1b1fb69, M-1316 | high; header is not RPC identity/cause; no initialized player/world entry | U-038, U-079 | superseded-by F-1049 |
| F-1044 | Capture admits UserProfile and waits for CharacterProfile; one malformed body, UI Preparing geometry | measured | capture/source; M-1293, M-1302 to M-1304, M-1309, M-1310, M-1315/ev_67e4d1b1fb69, M-1316, M-1320/ev_2b3dc4d67469, M-1321 | bounded; e32d008; no world entry, compatibility or malformed-cause proof | U-038, U-079 | superseded-by F-1049 |
