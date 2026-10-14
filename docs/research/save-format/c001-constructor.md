# C001 constructor decoder and exact-window listing

Covers older-server C001 callback/candidate-constructor location and decoding: metadata bounds, listing rejection/diagnostic, bounded standalone decoder/fixtures; native capture completeness; REA tooling/connection; 2026-10-13 and 2026-10-12 pauses/gates.
Full text: fabd2f9:docs/implementation-status.md lines 16-68, 117-143, 144-169, 195-231, 280-302, 435-444, 629-640, 885-940.

Full wording before condensation: 43757e7c13686591d66de968c3f28e1afc9c36ae:docs/research/save-format/2026-10-13-c001-decoder.md.

Full wording before condensation: 2c40a16:docs/research/save-format/2026-10-13-c001-decoder.md.

## Question cluster

- Where are the C001 construction callback and candidate constructor, and what are their exact byte windows (F-031, F-032, F-034)?
- Why did the retained listing query reject, and where (F-014, F-019, F-021, U-003)?
- Can a database-free decoder decode those exact windows faithfully (F-043 to F-050, U-001, U-002)?
- Does a construction/vtable bridge or registration flow exist (U-004, U-006)?
- Is native snapshot capture complete before publication (F-003, F-022 to F-030)?
- What is the REA tooling and connection state (F-051 to F-053)?
- What is paused and what gates resumption (F-001, F-055, F-056; gates.md)?

## Answered

- All older-server exception records validate; callback/candidate intervals are 305/296 bytes (F-031, F-032, F-033; M-011).
- Native getter 0x1419fe200: sole retained incoming reference is DEFAULT/non-call, no containing function (F-039, F-040, F-042; M-013).
- World fallthrough/Pending post-call locations are derived continuations, not identities (F-035 to F-038; M-012).
- Retained listing rejects after Ghidra script exception; exit zero not accepted, bindings unchanged (F-019, F-020, F-021; M-008).
- Diagnostic locates rejection at first listing-entry operation/check 0x1419fe870 (F-014, F-016, F-017, F-018; M-007).
- Bounded decoder: fixed x64 buffers, no Program/database, unsupported fallbacks rejected; nine recipes/five guards pass after checker correction (F-043 to F-049; M-014).
- Pinned relative-zero CALL gives jump/type-1/BRANCH p-code; original nine-mode checker fails (F-006, F-007).
- Frozen fixture v2: nonzero-call oracle, sixteen mutations reject, installed-API compile passes (F-009, F-010, F-012; M-003, M-004).
- Final independent fixture/outer review and fresh ten-mode engine pass; independent input reconstruction/strict replay agree, bindings unchanged, owned cleanup gone (F-406, F-407; M-176, M-177).
- Execution instruction resumes offline research; case gates retained, no connected-lab/original-runtime scope (F-404; M-174).
- Full physical windows decode as 51/49 instructions; exception replay, strict input/output joins, 324 bindings/cleanup pass (F-409; M-179, M-180).
- Both windows call one common direct callee and share 40 typed post-call instructions; callback writes candidate table inline, with no in-window candidate-constructor call (F-410; M-181).
- Callee starts measured 106-byte exception interval; full directory/independent tuple-result joins agree, no code/unwind decode (F-414; M-185, M-186).
- Separately reviewed single-window decoder: compile, ten fresh authored engine modes/independent physical replay pass; classes/runtime/source bound, cleanup passes (F-415; M-187, M-188).
- Separate original callee: all 106 physical bytes decode as 24 instructions; strict replay, 340 bindings/copied classes/cleanup pass (F-416; M-189, M-190).
- Retained callee output qualifies two typed direct CALL targets, numeric register COPY/local stack shape; fresh successful exit/immutable joins captured (F-419; M-193 to M-195).
- Targets start measured 196/217-byte intervals; full directory validation/independent physical mappings agree, no code/unwind decode (F-420; M-197 to M-199).
- Two-window source review, fresh API compile/ten actual authored modes pass; independent 413-byte recipe reconstruction,326pins/classes/cleanup qualify (F-422; M-201, M-202).
- Separately reviewed original nested case:45/48 instructions cover all413 physical bytes;353pins/classes, exact input/result replay/owned cleanup qualify (F-424; M-205, M-206).
- Fresh retained-output replay:93declared local edges, ten opaque calls, three return sites; both graphs open, no receiver/preservation/archive proof (F-426; M-209 to M-211).
- Callback/candidate replay:99declared in-window edges, two common-callee calls/two return sites;19pins/strict601-byte joins pass, both graphs open (F-430; M-221 to M-223).
- Source-only receiver review accepts conditional normal-return ABI inference with local stack/save-slot/home-area/argument joins; unwind intent alone cannot clear gate (F-431; M-224).
- Three-record unwind source passes136authored/74independent checks; fixed acquisition/publication guards pass78VM cases (F-433; M-226, M-227).
- Fresh metadata: three12-byte v1 records, register3 push endpoints offset2,48/32/48-byte allocation endpoints offset6; each flags2/opaque handler RVA.21pins, full parent-directory/independent record replay pass (F-434; M-228).
- Register tooling source/API compile,92source/schema/89wrapper guards qualify; one live query yields1480storage descriptors/six named GPR joins,304pins unchanged, owned cleanup gone (F-435, F-436; M-229 to M-231).
- Frame sources: flags0 parent81authored/24independent after three false-positive fixes, frozen; handler sibling121/65 after slot accounting, opaque RVAs and conditional ordinary-entry paths (F-437, F-438; M-232, M-233).
- Four byte-unit spaces:145authored/104independent/82wrapper/API compile,304pins/exit0/replay/cleanup. RAM/constant addresses64bits, register/unique32; value widths separate (F-439; M-234 to M-236).
- Lossless three-case adapter:176authored/49independent, full windows/unwind unchanged, descriptor-derived GPR/flag maps (F-440; M-237).
- Cases F-442/F-447/F-450/F-457/F-460 replay707code/36metadata bytes: actual/independentexit0, partial store/call/return claims cleared and gates false.
- Caller76VM/17source after eight fixes; independent replay eight checks. Case: 44pins, three prologue opcode27 stops (F-441, F-442; M-238, M-239).
- Bitvector219authored/29independent preserves parent guards; unknown carriers lose stack provenance. Normal-return/frame-integrity/nonalias assumptions and absent complete escape checks remain (F-444; M-242).
- Public constants AND27/POPCOUNT72/LZCOUNT73/SPULL74/MAX75:28source checks/external-childexit0, three pins/minimalenv/offline/owned cleanup (F-445; M-243, M-244).
- Bitvector caller84VM/23source; replay ten checks. Case: 48pins, three prolog joins/COPY-width stops, null sites; linear shapes are not reachability proof (F-446, F-447; M-245, M-246).
- Narrow COPY/nonstack STORE/diagnostics319authored/39independent, exact reversal; widths1/2/4/8 and mapped byte flags. Partial GPR/narrow stack writes unsupported; escape/frame/nonalias assumptions remain (F-448; M-247).
- Narrow caller85VM/24source; replay47checks. Case: 48pins, three prolog joins/opcode26 row-index2 stops; no reachability/receiver/ABI/frame/archive proof (F-449, F-450; M-248, M-249).
- Zeroing438authored/94independent/250integer vectors: volatile low32 self-XOR/explicit ZEXT, signed comparisons/cache havoc; cap-summary fix supersedes parent inconsistency (F-453; M-253).
- Low32 adapter259authored/53independent: seven named pairs/exact reversal. Nine-field parser79/28: preserved comment-envelope fix; actual storage/text joins remain caller work (F-454, F-455; M-254, M-255).
- Zeroing caller97VM/34source; replay128checks. Case: 52pins, seven low32 pairs/nine constants/three prolog joins, exact partial-COPY row-index1 stops; no new query/reachability proof (F-456, F-457; M-256, M-257).
- Read-copy513authored/59independent/150snapshot pairs, exact three-edit reversal. Live writes/CALL invalidate source knowledge, copied temporaries remain independent; partial writes/widening unsupported, unknown identity lost (F-458; M-258).
- Read-copy caller97VM/37source-delta; replay128checks. Case: 52pins, three prolog joins/exact mixed16-byte COPY/byte LOAD row-index stops, cleared arrays/gates; linear shapes do not prove reachability; original data unread (F-459, F-460; M-259, M-260).
- Opaque-data checker passes657authored/58independent checks; vector adapter350authored/58independent, with exact parent reversals. RAM contents remain unread, quarters geometrically derived, narrow stack accesses rejected; ABI/frame/receiver/escape conditions remain unproved (F-461, F-462; M-261, M-262).
- Data caller passes111VM/41source checks after redundant descriptor/quarter joins hardened; frozen adapter already enforced them. Reconstructed former-guard probe failure is preserved. Fresh retained case exits1 at pretty-JSON64KiB guard before publication; no result or model qualification retained (F-463, F-464; M-263, M-264).
- Source-only gate audit finds two inconsistent slot-address sums; no replacement selected. Hypothetical register continuity still needs object origin/path/final-vptr and slot-contract joins, without arbitrary transitive-memory proof or independent helper QA (F-465; M-265, M-266).
- Compact caller/serialization checks pass with unchanged64KiB cap; fresh40736-byte result and independent replay exit0,52pins and229source checks. Common conditional model reports24states/1return/9stores/2calls; constructors stop at exact XMM1-quarter COPY index1. Common semantics are not independently proved; all17proofs stay false (F-466, F-467; M-267, M-268).
- Native capture visits every domain-identified entity, rejects missing Position before publication; tests/manual suite/release build/smoke pass at af4270b (F-003, F-022 to F-028; M-010, M-002).
- Prior-bindings transition to af4270b qualifies (F-030; M-001, M-009).
- REA 6.4.0 exposes 170 tools with aligned identity and no target open; Ghidra available (F-051, F-053; M-015, M-006).
- Live MCP catalog aligns, no target open; Ghidra byte reader requires default analysis/loader-adjusted memory, preserving explicit physical-metadata capability gap (F-432; M-225).

## Partial

- Historical fixture-v2 unfinished checkpoint is superseded by qualified review and ten-mode execution (F-011, F-406, F-407; U-001 resolved).
- Listing diagnostic gives location only, not cause (F-015, U-003).
- Metadata intervals may be procedure fragments (F-034, U-005).
- Selected unwind records describe intent only; local code/stack/receiver joins remain pending and handler data is unread (F-434, U-041).
- Register descriptors and space units qualify storage geometry, not global/subbyte aliases or live values. Input/prolog/register-pair/opcode joins and exact controlled-stop locations qualify while frame replay remains partial; ordinary-entry, ABI, frame integrity, nonalias and complete escape conditions remain unproved (F-436, F-438 to F-460, U-041).
- Earlier retained-facts candidate lacked a captured external exit; it remains unqualified and preserved beside the successful fresh case (F-419; M-196).
- Frozen narrow parent's invented output-cap rejection retains a stale equality summary with qualified=false; zeroing sibling clears it, preserving the earlier observation (F-451, F-453; M-250, M-253).

## Unresolved

- Receiver continuity through opaque calls and concrete C001 archive dispatch remain unproved despite qualified constructor/nested control edges (F-419, F-424, F-426, F-430, U-004, U-041).
- Construction/vtable bridge and C001 bodies (U-004).
- Registration argument flow and caller absence for getter 0x1419fe200 (F-041, U-006).
- Older World/Pending registration seed (F-054; see topic World/Pending stores).
- C009 top-scalar retry and limit API status from the 2026-10-12 pause (F-057, F-059; see topic C009 audits).

## Limitations and exclusions

- Authored decoder tests contain no original code; the separate original-code case supplies only bounded instruction facts, with no callee preservation, ABI, archive-slot or runtime proof (F-050, F-409, F-410).
- Metadata bounds are not decoded functions or a construction bridge; no C001 save body parsed (F-034).
- Capture schema excludes entities without domain identity; no original runtime, private save parsing or ICARUS compatibility test (F-025, F-029).
- REA connection/metadata is not target analysis or target authority; cold-import/retained-provider limits remain (F-052, F-053).
- Foundation remains nonplayable; no original runtime/container or client ran (F-002, F-055).
- Historical 2026-10-12 pause is superseded; its revision and suite are historical (F-056, F-058, F-061).

## Receipts

- Fixture and decoder: M-003, M-004, M-014, M-176, M-177.
- Exact-window review, result and local post-call facts: M-179, M-180, M-181.
- Common callee metadata source/case review and result: M-185, M-186; single-window source/fixture qualification: M-187, M-188; original callee case: M-189, M-190.
- Typed callee facts/review/replay and preserved capture gap: M-193 to M-196; nested metadata/review/execution: M-197 to M-199.
- Nested two-window source/API/engine qualification: M-201, M-202; original case: M-205, M-206.
- Nested declared CFG checker/wrapper reviews and retained-output qualification: M-209 to M-211.
- Callback/candidate declared CFG reviews and retained-output qualification: M-221 to M-223.
- Conditional receiver assessment and live connected capability check: M-224, M-225.
- Unwind metadata source/wrapper review and scoped physical qualification: M-226 to M-228.
- Register metadata source/wrapper/query and conditional frame source reviews: M-229 to M-233.
- Address-unit source/wrapper/query and independent replay: M-234 to M-236.
- Lossless adapter source review: M-237.
- Retained-frame caller review, exact input joins and controlled partial replay: M-238, M-239.
- Bitvector sibling review and public provider opcode query: M-242 to M-244.
- Bitvector caller/source joins and controlled COPY-width partial replay: M-245, M-246.
- Narrow-width and diagnostic source review: M-247.
- Narrow caller and fresh controlled-partial replay: M-248, M-249.
- Supplementary flag/alias/epoch audit and cap-summary defect: M-250.
- Zeroing source, low32 adapter and nine-field public-opcode parser: M-253 to M-255.
- Zeroing caller, retained joins and controlled partial replay: M-256, M-257.
- Low32 read-copy source review: M-258.
- Read-copy caller and fresh mixed-width partial replay: M-259, M-260.
- Opaque-data/vector sources, caller review and rejected output-cap case: M-261 to M-264.
- Original registration preparation and hypothetical ownership-gate/arithmetic assessment: M-265, M-266.
- Compact caller and fresh mixed complete/partial replay: M-267, M-268.
- Resumption and fresh tooling check: M-174.
- Listing rejection and diagnostic: M-007, M-008.
- Metadata bounds and references: M-011, M-012, M-013.
- Native capture fix and transition: M-001, M-002, M-009, M-010.
- REA state: M-005, M-006, M-015.
- Historical checkpoint: M-016 to M-022.
