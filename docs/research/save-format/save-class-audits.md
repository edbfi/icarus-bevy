# Supplied-save class audits

Header, width and string-frame coverage for C009 (nested D01), C013, C028
and C034 in both supplied copies. [Gates](../../gates.md#before-new-save-class-work).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-save-class-audits.md

## Question cluster

Which selected structures match both copies? Which descriptor families lack
populated examples? What remains unknown about restoration and behavior?

## Current answer

All 38 classes round-trip byte-identically through a value-opaque tree in both
copies (F-556, F-559, F-900). Census: F-496, F-901, F-722 (U-023 resolved), F-668.
Server 25621760 matches client 25793767 on audited serializers and load paths
(gates); recorder registrations, defaults, dispatch and overrides:
[restore rules](../../specs/save-format/restore-rules.md) (F-645..F-680,
F-741, F-746, F-747, F-750, F-902..F-904). Selected coverage: F-728, F-737,
F-738. Recorder and clocks: F-894/F-895/F-1271; saved slot keys and C026 UIDs:
F-1275; load timings: F-731; opaque math Structs: F-900, F-602; construction
grids: F-643.

All 15 prospects type recorder values losslessly within tree limits (F-766).
Import planning: row names (F-897, F-898), world context (F-916),
synthetic quest coverage (F-1263) and candidate retention (F-920).
C016 records saved positions (F-651). With normal Win64 register preservation,
a modeled skipped iteration may advance the stitching counter (F-1266).

Authored adapters F-927 leave import closed. Lifecycle F-930; operands and
append F-937/F-940. Reuse F-1195/M-1426's row-handle list/base join; M-1517
qualifies the repeated reader. Removal declaration F-1237/F-1238. With modeled
`AL=1`, all 24 bytes reach removal. Final `AL=0` bytes, actual ABI and receiver,
owner, row and native import remain open (F-1260).

Saved identifiers do not discriminate the producing build (F-635, F-896).
Row references: F-660; reader allocation/comparator limits: F-142, F-198.

## Open

- [C016 binding questions](../../specs/save-format/restore-rules.md#open-questions).
- Packaged data-table row meanings remain out of scope by policy (F-660).
- An all-or-nothing official-save writer needs recorder adapters, coordinate binding and original behavior evidence (U-025, U-038, U-045).
- Reuse the heavy-world baseline (F-894, F-736) and connected S1 (F-701); producing-build pairing remains U-040.

## Artifacts

Cited findings; M-1425 to M-1428, M-1431 to M-1458, M-1471 to M-1476; M-1508, M-1517, M-1601, M-1602.
