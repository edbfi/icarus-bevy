# Compatibility matrix

No official build pair is validated. Every complete ICARUS-facing surface
remains unknown; bounded static observations are listed separately and do not
promote a surface. Status progression: unknown, observed, specified,
implemented, validated. Each eventual row names the exact build and content,
reference scenario, evidence IDs, specification, test and intentional
differences. Server observations apply to build 25621760 (M-168); client static
comparisons apply to build 25793767 (M-167) and do not establish a matched
dedicated-server baseline. Full text before condensation:
16d1a9d:docs/ledgers/compatibility.md.

Last reconciled: 2026-10-18

## Surfaces

| Surface | Status | Required proof |
| --- | --- | --- |
| Discovery and queries | Unknown | Stock-client browser/query behavior |
| Negotiation and authentication | Unknown | Valid identity and legitimate service integration |
| Transport and wire encoding | Unknown | Framing, ordering, fragmentation, compression/protection |
| Initial synchronization and replication | Unknown | Consistent objects, properties, relevancy, RPCs |
| Join, disconnect and reconnect | Unknown | Correct ownership, cleanup, resynchronization |
| World data and collision | Unknown | Representative stock region with independently authored server data |
| Movement and player lifecycle | Unknown | Authoritative movement, correction, death/respawn |
| Inventory, crafting and production | Unknown | State/invariant parity and concurrent actions |
| Construction and structural systems | Unknown | Placement, support, damage and connected systems |
| Wildlife, combat and tames | Unknown | AI/navigation, ownership and persistent populations |
| Weather, resources and environment | Unknown | World evolution and player effects |
| Missions, prospects and progression | Unknown | All required modes, transitions and rewards |
| Administration and configuration | Unknown | Observed official settings and commands |
| Official save import | Unknown | Complete field preservation and stock-client validation |
| Large-world capacity | Unknown | Matched representative workloads within verified player limit |

Native snapshot version 1 and the provisional 30 Hz loop are authored
infrastructure (F-215, F-216). The isolated startup reported an entry-world
maximum tick rate of 30 and an IpNetDriver listener after Steam failed; no
client connected and no prospect loaded (F-268).

## Bounded observations and experimental components

| Subsurface | Status | Evidence | Remaining gate |
| --- | --- | --- | --- |
| Control dispatch and version comparison | Observed | F-284, F-712, F-713, F-714; [spec](../specs/network/control-negotiation.md) | Stock-client session (U-038) |
| Initial control archive fields | Experimental reader | F-096, F-097 | Stock-client session (U-038) |
| Control-dispatch pointer stores | Static vftables, destructor and S1 dispatch qualified | F-091, F-095, F-110, F-113, F-724, F-729 | Native stock-client session (U-038) |
| Current-client notify tables | Slot/name equalities, `/GR-` layout and S1 dispatch qualified | F-088, F-724, F-729 | Native stock-client session (U-038) |
| Current-client constructor intervals and prefixes | Complete constructors and ABI qualified | F-082, F-083, F-093, F-723 | Native stock-client session (U-038) |
| Current-client World/Pending table stores | Receiver origin, layout and `InitBase` qualified | F-062, F-063, F-071, F-076, F-274, F-723, F-724 | Native stock-client session (U-038) |
| Prospect-loading control flow | Observed; entry rules mirrored | [spec](../specs/save-format/prospect-loading.md); F-256, F-717 | Runtime failure reachability and complete schema (U-040, U-058) |
| Inner prospect state and dispatch | Observed; version migration mirrored | F-257, F-715 | Recorder payloads and newer-build fidelity (U-025) |
| Root property tags and recorder framing | Observed on two copies; experimental readers | F-244, F-245, F-727; [spec](../specs/save-format/tagged-properties.md) | Packaged row meanings and native import (U-020, U-045) |
| Selected base-component framing | Observed on 1,296 records; 38-class overrides verified | F-259, F-260, F-727 | Packaged row meanings and native import (U-020, U-045) |
| Selected tagged struct arrays | Library reader; C034 and C001 arrays qualified | F-316, F-196, F-728 | Packaged row meanings and native import (U-020, U-045) |
| C028 top, child headers and leaves | Complete supplied population and restore hooks qualified | F-115, F-128, F-135, F-137, F-123, F-124, F-125, F-716, F-728 | Value meanings and native import (U-020, U-045) |
| C034 top, own and inherited scalars | Exact structures, widths and `FName` parsing qualified | F-146, F-147, F-150, F-152, F-196, F-719, F-720 | Value meanings and native import (U-020, U-045) |
| C009 top, children and D01 arrays | Headers, string structure, widths and restore hooks qualified | F-182, F-183, F-174, F-158, F-162, F-728 | Packaged row meanings and native import (U-020, U-045) |
| C001 construction seeds | Complete functions, registration and bridge qualified | F-270, F-034, F-534, F-721, F-722 | Value meanings and native import (U-020, U-045) |
| Stateless handshake | Observed; prefix, restart and tolerance rules verified | F-253, F-280, F-729; [spec](../specs/network/stateless-handshake.md) | Stock-client admission (U-052) |
| Default network checksum | Experimental primitive; wire-verified | F-254, F-255, F-281, F-689, F-713; [spec](../specs/network/network-checksum.md) | Stock-client session (U-038) |
| Archive strings | Experimental strict subset; original normalization and timings verified | F-142, F-282, F-285, F-719, F-731; [spec](../specs/save-format/archive-string.md) | Stock-client session and import (U-025, U-038) |
| Prospect envelope | Observed on supplied saves; read-only inspector | F-261, F-283, F-241; [spec](../specs/save-format/prospect-envelope.md) | Build pairing, binary schema, import (U-025, U-040) |
