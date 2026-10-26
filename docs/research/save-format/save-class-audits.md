# Supplied-save class audits

Structural qualification of selected supplied-save classes (C009 with its D01
nested arrays, C013, C028 and C034) against both supplied copies, and the
reader changes that enabled it. Values stay opaque throughout. Preconditions for
new class work are in [gates](../../gates.md#before-new-save-class-work).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-save-class-audits.md

## Question cluster

- Which selected classes have exact, individually qualified header, width and string-frame structure in both supplied copies?
- Which descriptor families lack populated coverage?
- What lies outside the selection, and what remains unknown about meanings, restoration and original behavior?

## Current answer

Every body of every one of the save's 38 classes frames and round-trips
byte-identically through a lossless value-opaque tree in both copies, under
statically qualified Map, Set and Int64 headers and 1 MiB component and
struct-array bounds (F-556, F-900); all 163 Maps per copy frame into
items and no Set occurs (F-559, F-900). The census began with C009, C013, C028
and C034 (F-496), then C001 (F-901, F-722; U-023 resolved), the
player-state candidates and the remaining classes (F-668; F-213). Server
25621760 matches client 25793767 across the audited string/name, scalar/math,
array/container and tagged serializers, migration/restore and load-entry failure
paths (F-715..F-720, F-725..F-728, F-732..F-738, F-742..F-744,
F-893, F-899). All 65 recorder registrations, constructor
defaults, dispatch, subsystem operations, owner matching and nested virtual
overrides are qualified at the [restore-rules spec](../../specs/save-format/restore-rules.md)
and its source rows (F-645..F-680, F-741, F-746, F-747, F-750,
F-902..F-904). Selected coverage includes 6 of 7 populated C001 arrays,
`CA06`/`DA04`, C028 spans, `SL-004` blob contracts and accolade/prospect
structures. Reference-load recorder changes remain F-894; load timings: F-731.
Opaque Structs use four fixed-width engine math types (F-900, F-602).
Construction grids nest per-type row names over pieces in the grid's frame
(F-643).

All 15 prospects type every recorder value losslessly within tree limits
(F-766). Reference planning separates instance identity/content and classifies
recorder fields by restorer (F-897). Scoped catalog declarations bind roots,
map-owned paths/tiles and class-owned variables (F-916), extending F-898. After
completed scratch release, candidates retain roots, identities, typing and
bytes under unchanged limits (F-920): retention, not restoration or RSS.

Strict authored C001 adapters bind source state to placements, owner lifecycle,
Bool/I32 use gates and stacked inventories with durable actions (F-927).
Coordinates, layouts and semantics are authored; ordinary import stays closed.
Reload and primary lifecycle: F-930. Paired bounded traces and authored
models cover local removal selection and comparator order (F-937). The
zero-tail and exact counted-u32 queries refuse `ERR_SYMBOL_UNSUPPORTED`;
no profile or extent is accepted (M-1062, M-1066). Guard kinds are `0x1168`
then `0x1102`, not helper classifications (M-1065, M-1067). Prior metadata boundaries and remaining
unknowns: [spec](../../specs/save-format/restore-rules.md#confirmed)
(M-1050, M-1052..M-1054, M-1058, M-1059, M-1061).

Saved identifiers do not discriminate the producing build (F-635, F-896).
Row references: F-660; reader allocation/comparator limits: F-142, F-198.

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Complete further recorder adapters and qualify coordinates before an all-or-nothing official-save writer. Authored consumers (F-927) and reference coverage (F-916) do not qualify original behavior (U-025, U-038, U-045).
- Reuse the preserved heavy-world baseline (F-894, F-736) and connected S1 (F-701). Producing-build pairing remains U-040; native import keeps its gates (U-038, U-045).

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766..M-769, M-772..M-777, M-780, M-782, M-784, M-786, M-788, M-790..M-792, M-794..M-808, M-811, M-850, M-851, M-854..M-859, M-873, M-878, M-880, M-900, M-919..M-925, M-926..M-929, M-937..M-943, M-958, M-959; M-983, M-984, M-986, M-987, M-989..M-993; M-990/ev_693319f721fe, M-992/ev_5ad56afa9170, M-995..M-1010, M-1024/ev_053a26045f8e, M-1025, M-1027/ev_c40e7a02c708, M-1028, M-1035..M-1037, M-1044..M-1048, M-1050, M-1052..M-1054, M-1058, M-1059, M-1061..M-1062, M-1065..M-1067.
