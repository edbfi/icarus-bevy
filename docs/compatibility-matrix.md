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

## Partial observations and experimental components

| Subsurface | Status | Evidence and remaining gate |
| --- | --- | --- |
| Inner control dispatch and version comparison | Observed | [Negotiation notes](specs/control-negotiation.md); outer framing, callback policy and runtime fixtures unknown |
| Prospect-loading control flow | Observed | [Loading notes](specs/prospect-loading.md) and [by-name rejection runs](research/2026-10-04-prospect-rejection.md); from-file runtime reachability and complete schema remain unresolved |
| Inner prospect state and serialization dispatch | Observed on older executable | [Loading notes](specs/prospect-loading.md); reflected header/recorder fields, version callbacks and tagged-property dispatch identified; recorder payloads and newer-build fidelity unresolved |
| Root property tags | Observed on two copies; experimental opaque reader implemented | [Tag framing](specs/tagged-properties.md); memory522 profile, bounded root spans, authored malformed-input fixtures and independent/native agreement on six root records per copy; nested values and complete schema validity unsupported |
| Recorder field framing | Observed; experimental bounded reader implemented | [Recorder framing](specs/tagged-properties.md); one shared struct header, exact element/field/byte-array boundaries, unknown fields preserved; component contents, class resolution and complete import unsupported |
| Selected base-component framing | Observed on 1,296 records from two selected classes; explicit library subset implemented | [Component framing](specs/tagged-properties.md#explicit-base-component-framing); independent/native agreement on opaque properties plus zero object trailer; concrete dispatch checked per class, but library class recognition, value semantics, GUID trailers and import unsupported |
| Selected tagged struct arrays | Experimental library reader; C034 own-array population qualified | [Struct-array framing](specs/tagged-properties.md#explicit-tagged-struct-array-framing); 28 selected C028 empty arrays, all 11,606 empty C034 inherited arrays, and all 1,658 C034 own arrays (15,342 elements/30,684 fields) have independent/native exact-span agreement. Automatic schema recognition and value semantics remain unqualified |
| Third selected component and own struct | All supplied top and own headers independently/native qualified | [C034 comparison](specs/prospect-loading.md#third-selected-class-bounded-top-header-comparison); All 1,658 supplied top and own bodies and 9,948 inherited Struct bodies have exact native/independent immediate spans; descriptors remain optional. Own-array shared/element headers now qualified; 15,342 Struct and 15,342 Float spans individually match older 12/four-byte transfer demands. Numeric values remain opaque |
| C009 top component framing | All 486 supplied bodies independently/native qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); exact 31-field optional schema and spans agree across 6,781,758 bytes under an explicit 71,956-byte cap. Inherited child values and import remain unqualified |
| C009 selected top scalars | Strict string structure and individual widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 3,402 Name/Str/Enum spans agree across 136,170 bytes; 1,458 Int4 and 486 Bool0 spans match. Text/numeric/enum meaning and restoration remain unqualified |
| C009 own children and leaves | Headers, array/string structure and scalar widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 972 child bodies, 486 Int arrays and 486 Str values match native/independent structure. All selected Int/Bool widths agree. Numerical/string meaning and import remain unqualified |
| C009 immediate-parent children | Headers, string structure and scalar widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 11,780 fields match across 1,944 arrays and 486 standalone streams. All 1,464 string-framed leaves and selected Int/Bool widths agree. Two array families are empty; value meanings remain unqualified |
| Short stateless handshake | Observed; independent loopback lab implemented | [Handshake notes](specs/stateless-handshake.md); original challenge/echo/ack observed with authentication inactive; stock-client admission pending |
| Default network checksum | Experimental primitive implemented; early value observed | [Checksum notes](specs/network-checksum.md); final active inputs/callback policy unresolved |
| Archive strings | Experimental strict subset implemented | [String codec](specs/archive-string.md); authored fixtures pass; actual network flags and original round trips pending |
| Prospect envelope | Observed on supplied saves; experimental read-only inspector | [Envelope notes](specs/prospect-envelope.md); twelve UTF-16LE envelopes pass independent integrity checks, with measured bounds; user reports build 25793767; matched build pairing, binary schema and complete import remain unvalidated |

Server observations apply to build 25621760. Separate build-25793767 client
static comparison supports selected generic tag/array branches only; it does not
establish a matched dedicated-server baseline. The supplied save's producing
build remains user-reported as 25793767, without a matched load. These observations
do not promote the complete transport, authentication or official-save-import
surfaces above.
