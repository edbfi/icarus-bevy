# Supplied-save class audits

Structural qualification of selected supplied-save classes (C009 with nested
D01, C013, C028, C034) against both copies, and the enabling reader changes;
values stay opaque. Preconditions: [gates](../../gates.md#before-new-save-class-work).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-save-class-audits.md

## Question cluster

- Which selected classes have exact, individually qualified header, width and string-frame structure in both supplied copies?
- Which descriptor families lack populated coverage?
- What lies outside the selection, and what remains unknown about meanings, restoration and original behavior?

## Current answer

Every body of the save's 38 classes round-trips byte-identically through a
lossless value-opaque tree in both copies (F-556, F-559, F-900). Census: F-496,
F-901, F-722 (U-023 resolved), F-668.
Server 25621760 matches client 25793767 on audited serializers and load paths
(gates); recorder registrations, defaults, dispatch and overrides:
[restore rules](../../specs/save-format/restore-rules.md) (F-645..F-680,
F-741, F-746, F-747, F-750, F-902..F-904). Selected coverage: F-728, F-737,
F-738. Reference-load recorder changes: F-894; load timings: F-731; opaque
math Structs: F-900, F-602; construction grids: F-643.

All 15 prospects type recorder values losslessly within tree limits (F-766).
Reference plans: F-897, F-898, F-916; candidate retention, not restoration:
F-920.

Strict authored C001 adapters: F-927; ordinary import stays closed.
Reload/lifecycle: F-930; removal and comparator models and their open limits:
F-937, with the server helper F-940
([boundary](../../specs/save-format/restore-rules.md#confirmed)). Declaration-only
row-handle replay preserves layout refusals (F-1187); the selected row member
has a forward type, without width, component-owner or accessor association.

Saved identifiers do not discriminate the producing build (F-635, F-896).
Row references: F-660; reader allocation/comparator limits: F-142, F-198.

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Further recorder adapters and qualified coordinates precede an all-or-nothing official-save writer; authored consumers and reference coverage do not qualify original behavior (U-025, U-038, U-045).
- Reuse the heavy-world baseline (F-894, F-736) and connected S1 (F-701); producing-build pairing remains U-040.

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766..M-769, M-772..M-777, M-780, M-782, M-784, M-786, M-788, M-790..M-792, M-794..M-808, M-811, M-850, M-851, M-854..M-859, M-873, M-878, M-880, M-900, M-919..M-929, M-937..M-943, M-958, M-959; M-983, M-984, M-986, M-987, M-989..M-993; M-990/ev_693319f721fe, M-992/ev_5ad56afa9170, M-995..M-1010, M-1024/ev_053a26045f8e, M-1025, M-1027/ev_c40e7a02c708, M-1028, M-1035..M-1037, M-1044..M-1048, M-1050, M-1052..M-1054, M-1058, M-1059, M-1061..M-1062, M-1065..M-1068, M-1071..M-1074; M-1425 to M-1428.
