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
| Control-dispatch pointer stores | Local static stores and bounded register sequences qualified | [Caller investigation](research/2026-10-12-control-caller-limit.md#local-post-call-gap-closed); original-byte joins prove eight-byte stores at offsets zero and `+0x28` in both selected sequences. A post-call suffix closes the local gap with unknown register state; no provenance crosses its earlier call. Class, table identity, actual receiver/caller and runtime reachability remain open |
| Initial control archive fields | Experimental caller-selected reader implemented | [Initial archive-body fields](specs/control-negotiation.md#experimental-initial-archive-body-fields); authored explicit-endian byte/u32/strict-string fixtures pass. Message boundaries, active byte order, token handling and admission remain unknown |
| Current-client named notify tables | Two static slot/name equalities qualified | [Current-client symbols](research/2026-10-12-control-caller-limit.md#current-client-public-symbols-and-named-notify-slots); paired build-25793767 public symbols identify UWorld and UPendingNetGame FNetworkNotify secondary tables; both `+0x18` pointers equal named NotifyControlMessage addresses. No code dereference, object-offset, older-owner or runtime proof |
| Prospect-loading control flow | Observed | [Loading notes](specs/prospect-loading.md) and [by-name rejection runs](research/2026-10-04-prospect-rejection.md); from-file runtime reachability and complete schema remain unresolved |
| Inner prospect state and serialization dispatch | Observed on older executable | [Loading notes](specs/prospect-loading.md); reflected header/recorder fields, version callbacks and tagged-property dispatch identified; recorder payloads and newer-build fidelity unresolved |
| Root property tags | Observed on two copies; experimental opaque reader implemented | [Tag framing](specs/tagged-properties.md); memory522 profile, bounded root spans, authored malformed-input fixtures and independent/native agreement on six root records per copy; nested values and complete schema validity unsupported |
| Recorder field framing | Observed; experimental bounded reader implemented | [Recorder framing](specs/tagged-properties.md); one shared struct header, exact element/field/byte-array boundaries, unknown fields preserved; component contents, class resolution and complete import unsupported |
| Selected base-component framing | Observed on 1,296 records from two selected classes; explicit library subset implemented | [Component framing](specs/tagged-properties.md#explicit-base-component-framing); independent/native agreement on opaque properties plus zero object trailer; concrete dispatch checked per class, but library class recognition, value semantics, GUID trailers and import unsupported |
| C028 top scalar selection | Exact neutral ranges, Int/Bool widths and strict string structure qualified | [C028 comparison](specs/prospect-loading.md#selected-repeated-component-c028); 12,940 ranges across ten descriptors reproduce prior headers/native aggregates; 3,882 Int4 and 2,588 Bool0 spans match individually. Ten new native batches qualify 6,470 strict string frames / 328,878 bytes. Old native individual header spans remain unavailable; meanings and restoration remain open |
| Selected tagged struct arrays | Experimental library reader; C034 own-array population qualified | [Struct-array framing](specs/tagged-properties.md#explicit-tagged-struct-array-framing); 28 selected C028 empty arrays, all 11,606 empty C034 inherited arrays, and all 1,658 C034 own arrays (15,342 elements/30,684 fields) have independent/native exact-span agreement. Automatic schema recognition and value semantics remain unqualified |
| C028 direct Struct-child headers | Complete supplied population qualified | [C028 comparison](specs/prospect-loading.md#selected-repeated-component-c028); twelve fresh native batches match all 7,764 payloads / 1,174,952 bytes and 20,704 individual headers. All optional descriptors appear in these copies; values, grandchildren and restoration remain unqualified |
| Third selected component and own struct | All supplied top and own headers independently/native qualified | [C034 comparison](specs/prospect-loading.md#third-selected-class-bounded-top-header-comparison); All 1,658 supplied top and own bodies and 9,948 inherited Struct bodies have exact native/independent immediate spans; descriptors remain optional. Own-array shared/element headers now qualified; 15,342 Struct and 15,342 Float spans individually match older 12/four-byte transfer demands. Numeric values remain opaque |
| C034 inherited scalars | Thirteen exact descriptor structures/widths qualified | [C034 comparison](specs/prospect-loading.md#third-selected-class-bounded-top-header-comparison); 1,658 strict Name frames, 6,632 Int4, 1,658 UInt32 four-byte and 11,606 Bool0 spans agree. Historical native headers are reused, with two fresh native Name batches. Value meanings and import remain open |
| C034 own selected Int/Bool spans | Four exact individual-width contracts qualified | [C034 comparison](specs/prospect-loading.md#third-selected-class-bounded-top-header-comparison); 4,974 Int4 and 1,658 Bool0 spans match after full inherited-result replay and both-copy bounds. No new native own-scalar calls; historical header spans remain separate from numerical meaning/import |
| C034 top selected scalars | Seven exact descriptor structures/widths qualified | [C034 comparison](specs/prospect-loading.md#third-selected-class-bounded-top-header-comparison); 3,316 Name, 1,658 Str and 1,658 conditional Enum transfer frames match strict structure; 3,316 Int4 and 1,658 Bool0 spans match individually. Eight new top batches follow complete prior replay and both-copy budgets. Meanings, enum lookup and restoration remain open |
| C009 top component framing | All 486 supplied bodies independently/native qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); exact 31-field optional schema and spans agree across 6,781,758 bytes under an explicit 71,956-byte cap. Inherited child values and import remain unqualified |
| C009 selected top scalars | Strict string structure and individual widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 3,402 Name/Str/Enum spans agree across 136,170 bytes; 1,458 Int4 and 486 Bool0 spans match. Text/numeric/enum meaning and restoration remain unqualified |
| C009 own children and leaves | Headers, array/string structure and scalar widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 972 child bodies, 486 Int arrays and 486 Str values match native/independent structure. All selected Int/Bool widths agree. Numerical/string meaning and import remain unqualified |
| C009 immediate-parent children | Headers, string structure and scalar widths qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); 11,780 fields match across 1,944 arrays and 486 standalone streams. All 1,464 string-framed leaves and selected Int/Bool widths agree. Two array families are empty; value meanings remain unqualified |
| C009 common-ancestor children | Immediate and selected deeper headers, binary widths and populated scalar structures qualified | [C009 comparison](specs/prospect-loading.md#c009-top-header-comparison); common 9,018 headers, 1,468 binary and 7,272 scalar spans agree. ND01's 278 arrays contain 3,206 elements/25,648 qualified headers; selected deeper coverage appears below. CA06 is empty; meanings and restoration remain unqualified |
| C009 ND01 leaves | Four exact descriptor structures/widths qualified | [ND01 comparison](specs/prospect-loading.md#c009-top-header-comparison); 6,412 strict Name/Str frames and 6,412 Int4 spans agree. Meanings and import remain open |
| C009 D01 nested array children | Four exact schemas, supplied headers and populated leaf structures/widths qualified | [D01 comparison](specs/prospect-loading.md#c009-top-header-comparison); 12,824 arrays contain 12,868 elements/27,111 immediate fields with exact spans. All 25,680 Int4 and 1,431 strict Str frames match. DA04 is empty throughout; meanings and restoration remain open |
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
