# Findings: Network

Active rows: stateless handshake, checksum, control dispatcher and current-client World/Pending control stores.

F- IDs form one space across `docs/ledgers/findings/`; retired rows keep their
full text under `retired/`. Admission and columns follow `.agents/rules/docs.md`.

Before restructure: 96ddb0b:docs/ledgers/findings.md

Last reconciled: 2026-10-14

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-096 | Disconnected experimental reader composes observed byte/u32/FString initial-body sequence with caller-selected bounds/byte order. | observed | authored-test; M-037 | medium; borrowed opaque token spans; privacy-safe aggregates; negotiation/identity/game-connection flags all false; no live message | U-014 | active |
| F-097 | Initial-body reader: strict string policy, derived 131,081-byte ceiling; no listener, version admission or token state machine. | observed | authored-test; M-037 | high; reader scope; not a protocol implementation |  | active |
| F-253 | Original-server stateless challenge, cookie echo and acknowledgement were observed in isolation with Steam authentication inactive. | observed | runtime-capture; M-152; build 25621760 | high; isolated exchange; No stock-client or authenticated session | U-038 | active |
| F-254 | Independent checksum arithmetic matches one early startup value that is later invalidated; active admission policy is unresolved. | observed | runtime-capture; M-153 | medium; one startup value; Detail in control topic | U-037 | active |
| F-255 | Checksum follow-up resolves settings-derived setter and cache invalidation; candidate initializer references register teardown, not compat callbacks. | observed | static-decompile; M-146 | medium; direct-reference scan; Indirect/runtime registration unresolved | U-037 | active |
| F-267 | Targeted radare2 follow-up inspected control negotiation, version comparison and prospect-loading branches. | observed | static-decompile; M-160, M-161, M-156; build 25621760 | medium; selected references; Negotiation/loading notes incomplete and unvalidated | U-037 | active |
| F-280 | Stateless short frame: marker bit, restart bit, selector bit, 64-bit LE binary64 timestamp, 160 cookie bits, termination bit, 4 padding bits; 29 bytes. | observed | static-decompile; M-169 | high; writers/parser at the listed addresses; optional prefix and extended restart not observed | U-051 | active |
| F-281 | Default version checksum: UTF-16 units as LE u32, ASCII A-Z lowercased, seed and result complemented, reflected CRC-32 table 0xedb88320 at 0x145eed270. | observed | static-decompile; M-169; M-146 | high; default construction path only; callback route and active inputs unverified | U-037 | active |
| F-284 | Control dispatcher validates the message ID, then dispatches IDs 0,4,5,9,10,15,17; ID 0 reads one byte, a u32 remote version and a string, then compares versions via 0x141f2e6e0. | observed | static-decompile; M-170 | high; one function, one operating-mode branch; meanings inferred from strings; other branch and outer framing unknown | U-014; U-037 | active |
| F-478 | Current-client World and Pending store primary and FNetworkNotify table constants at +0/+0x28 of one unchanged unknown base after their constructor calls (World CALL 0x143ca5a55, Pending 0x143a58679); a prefix keeps that base through a +0x620 load | observed | static-decompile; M-023..M-028, M-034..M-036 | high; fixed windows; no owner identity | U-007, U-009, U-011, U-013 | active |
| F-479 | Current-client UWorld/UPendingNetGame constructor symbols start 8/71-byte metadata intervals (334,054-record directory validates); fixed windows export only prologues and pre-call prefixes; derived continuations are not symbols or receiver identities | observed | static-bytes; M-012, M-029..M-032 | high; procedure-fragment bounds; complete constructors absent | U-008 | active |
| F-480 | Build-25793767 PDB identity validates; 17 public records / 15 addresses name the control tables; each UWorld/UPendingNetGame FNetworkNotify table's +0x18 slot equals its NotifyControlMessage; the 0x290 table distance is image placement only | observed | static-bytes; M-034; build 25793767 | high; static; not receivers, runtime dispatch or older owner | U-007, U-010, U-011 | active |
| F-481 | Dispatcher caller scans: one absolute pointer at 0x145468198, no E8/E9 rel32 call; two 7-byte LEAs materialize slot 0x145468180 and store it at base +0x28; a Ghidra fallback finds one non-call reference with no function; the archive input path is unresolved | observed | static-bytes; M-039..M-045 | medium; candidates and fragments only | U-014, U-015, U-016, U-017, U-018 | active |
| F-499 | Older server 25621760 .rdata holds one whole UTF-16LE registration literal each, UWorld at 0x145631fb0 and UPendingNetGame at 0x1455af520, plus bare World (UTF-16 0x1446f4960, ASCII 0x14474d078) and ASCII PendingNetGame (0x1454dc9f0) | observed | static-bytes; M-281..M-283 | high; anchors only; no xrefs, registration flow or owner | U-043 | active |
| F-500 | In server 25621760 each literal has three LEA-shaped rel32 references and no absolute pointer: UWorld via rdx 0x140864e16, r8 0x140865ad2 and +2 via rdx 0x143b86add; UPendingNetGame via rdx 0x140854b96, r8 0x140855532 and +2 via rdx 0x143b21d5d | observed | static-bytes; M-288 | medium; byte patterns, boundaries unconfirmed; +2 fits TEXT("UName")+1 getters, unproved | U-043 | active |
