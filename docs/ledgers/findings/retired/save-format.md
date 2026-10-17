# Retired findings: Save format

Rows retired from [the active ledger](../save-format.md): superseded by a summary row,
execution receipts, or historical session state. Full text is kept and IDs stay
permanent; cite the superseding row where one exists.

Last reconciled: 2026-10-17

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-206 | Float/Text (af8cf48) passed root integration, source review vs frozen contract, full manual suite and release build; prior outputs and input hashes kept. | observed | review; M-112; af8cf48 | high; two supplied copies; No supplied Float/Text values qualified |  | retired: receipt |
| F-208 | Prior release executable/library were hashed before rebuild; historical private helpers and snapshots still bind the earlier nine-kind reader. | observed | tool-receipt; M-112; af8cf48 | high; release artifacts; Historical helpers are not rebound to the new reader |  | retired: receipt |
| F-259 | Second selected class (C028 case): all 1,294 records frame (29,762 fields, 4,326,044 bytes); all 9,058 arrays empty; detail belongs to C028 topic. | observed | tool-receipt; M-148, M-143, M-149, M-150, M-119; build 25621760 | high; both copies, E-025 to E-036; No populated-element coverage; deeper child encoding/values open | U-048 | superseded-by F-488 |
| F-263 | Save producing build is user-reported as 25793767 (3.0.31.158478-rel-DangerousHorizons); preserved server executable is older 25621760 (3.0.30.158174). | inferred | review; M-118 | low; user report; Not an embedded build marker or matched reference load | U-040 | superseded-by F-277 |
| F-324 | Historical native helper binaries still carry the earlier nine-kind reader and remain preserved | observed | tool-receipt; M-112 | high; helper binaries; Not current library |  | retired: receipt |
| F-498 | All 38 supplied-save class identifiers are registered as U-prefixed UTF-16 literals (8 also bare) in read-only data of both server 25621760 and client 25793767; class identity cannot discriminate the producing build | observed | static-bytes; M-278..M-280 | high; whole-string presence only; names private | U-040 | superseded-by F-635 |
| F-277 | User reports the supplied save's producing release as build 25793767, version 3.0.31.158478-rel-DangerousHorizons; preserved server is build 25621760. | inferred | review | low; user statement; unverified pairing with any reference | U-040 | superseded-by F-685 |
| F-331 | ProspectDTKey 0xd8 and ProspectID are legacy strings read by pre-version-2 fixups; StateRecorderBlobs array at 0xe8 added by derived state type | observed | static-decompile; M-147 | high; build 25621760; object offsets only | U-059 | superseded-by F-715 |
| F-337 | Loading bit 0x01 invokes slot +8: state 0x1418e4250 and header 0x1418e44f0 branch on Version<2, pass legacy strings to +0x10/+0x38 incl. 0x14089a1c0 | observed | static-decompile; M-147 | high; build 25621760; 0x14089a1c0 semantics unresolved | U-059 | superseded-by F-715 |
| F-338 | State version-zero path calls 0x14170a4c0, sets byte +0x0c to 1, releases recorder allocations and clears the recorder count | observed | static-decompile; M-147 | high; build 25621760; migration and enum meaning unresolved | U-059 | superseded-by F-715 |
| F-339 | 16-byte key 0x145dc12c8: lookup 0x141f5b5c0 returns -1 if absent and is stored at +8 after fixup; version-use slot +0x1d8 reads no table when loading | observed | static-decompile; M-147 | high; build 25621760; not a wire layout; Version vs build number unproved | U-060 | superseded-by F-715 |
| F-325 | By-name candidate 0x14146e6c0: empty-name check, file lookup, preliminary load, load/name comparison, continuation to map load; builds names with `.json` suffix | observed | static-decompile; M-160, M-170 | medium; build 25621760; suffix does not prove plain-JSON content, encoding or companion files | U-057 | superseded-by F-717 |
| F-326 | From-file candidate 0x1414baf60: two readers share one path object and initial helper; both results gate state application, either failure enters reporting | observed | static-decompile; M-160, M-170 | medium; build 25621760; one reader checks the helper return, other converts; no proof of lost error | U-056 | superseded-by F-717 |
| F-333 | By-name call site 0x14146ef42 enters 0x1414831f0, which builds the state vtable and calls blob reader 0x1418e6240 at 0x141483466 | observed | static-decompile; M-147 | high; build 25621760; shared type does not make failure behavior equal | U-056 | superseded-by F-717 |
| F-334 | From-file chain is observed separately through 0x141823fb0 and 0x1418e6140 | observed | static-decompile; M-147 | medium; build 25621760; caller unknown | U-056 | superseded-by F-717 |
