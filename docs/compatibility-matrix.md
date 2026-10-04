# Compatibility matrix

No official build pair has been validated. Every complete ICARUS-facing surface
below remains unknown; limited static observations are listed separately.
Native foundation tests do not promote these rows. Each eventual row must identify
the exact build/content, reference scenario, evidence IDs, specification, test and
intentional differences. Status progression: unknown → observed → specified →
implemented → validated. Expand rows for every supported content/game-mode variant.

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

Native snapshot version 1 and a provisional 30 Hz loop are authored infrastructure,
not reverse-engineered ICARUS formats or timing.

The [isolated startup](research/2026-10-04-offline-startup-report.md) reported an
entry-world maximum tick rate of 30 and an IpNetDriver listener after Steam failed.
Those observations do not promote gameplay timing or authenticated transport rows:
no client connected and no playable prospect was loaded.

## Partial static observations

| Subsurface | Status | Evidence and remaining gate |
| --- | --- | --- |
| Inner control dispatch and version comparison | Observed | [Negotiation notes](specs/control-negotiation.md); outer framing, callback policy and runtime fixtures unknown |
| Prospect-loading control flow | Observed | [Loading notes](specs/prospect-loading.md); schema, command reachability and runtime failures unknown |

These observations apply only to tool build 25621760. They do not promote the
complete transport, authentication or official-save-import surfaces above.
