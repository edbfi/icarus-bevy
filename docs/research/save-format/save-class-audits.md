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
F-902..F-904). Completed selected coverage
also includes 6 of 7 populated C001 arrays, `CA06`/`DA04`, all 29,762 C028
spans, `SL-004` blob contracts, accolade/prospect structures and `15 / 15`
`inspect-save --stage-check`; reference-load recorder changes remain F-894.
A bounded comparison of load times is F-731.
Opaque Structs use four fixed-width engine math types (F-900, F-602).
Construction grids nest per-type row names over pieces in the grid's frame
(F-643).

Within the tree limits, all 15 prospects type every recorder value losslessly
(F-766). Reference planning distinguishes instance identity from content and
classifies recorder fields by their restorers (F-897). Scoped catalog declarations bind roots,
map-owned paths/tiles and class-owned variables (F-916), extending the earlier
coverage in F-898. Retained candidates preserve roots, identities, typing and
bytes within unchanged limits after completed scratch is released (F-920).
These allowances measure retention, not restoration or RSS.

Strict authored C001 adapters bind source state to placements, owner lifecycle,
Bool/I32 use gates and stacked inventories with durable actions (F-927).
Coordinates, layouts and semantics are authored; ordinary import stays closed.
Reload uses matching `u16` tenths operands but unordered guards differ;
primary lifecycle, scaling, init and callback qualification: F-930. The
local removal selector and comparator order have paired bounded traces and authored
models (F-937). The helper targets have public-symbol matches but no covering
exception range (M-1050); procedure extents and callee effects remain open, as do
owner/row meanings, subclasses and live behavior.

No saved class identifier, field or type name, or recorder version tells the
older server from the newer client, and the prospect carries no version string
(F-635, F-896). Recorder code gives every class's fields their meanings (F-645 to
F-666, F-676, F-677, F-734 to F-736, F-741 to F-744, F-746, F-747, F-750, F-902 to
F-904); item
stats, alterations and slots are row references (F-660). Readers use borrowed
strict string frames without success-path allocation (F-142); comparator limits
stay at 128 KiB (F-198).

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Complete further recorder adapters and qualify coordinates before an all-or-nothing official-save writer. Authored consumers (F-927) and reference coverage (F-916) do not qualify original behavior (U-025, U-038, U-045).
- Reuse the preserved heavy-world baseline (F-894, F-736) and connected S1 (F-701). Producing-build pairing remains U-040; native import keeps its gates (U-038, U-045).

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766..M-769, M-772..M-777, M-780, M-782, M-784, M-786, M-788, M-790..M-792, M-794..M-808, M-811, M-850, M-851, M-854..M-859, M-873, M-878, M-880, M-900, M-919..M-925, M-926..M-929, M-937..M-943, M-958, M-959; M-983, M-984, M-986, M-987, M-989..M-993; M-990/ev_693319f721fe, M-992/ev_5ad56afa9170, M-995..M-1010, M-1024/ev_053a26045f8e, M-1025, M-1027/ev_c40e7a02c708, M-1028, M-1035..M-1037, M-1044..M-1048.
