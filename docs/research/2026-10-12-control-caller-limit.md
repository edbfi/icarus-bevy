# Bounded control-caller investigation

The concrete archive, bit-position and byte-order path into dispatcher
`0x1439ee7f0` remains unresolved. This investigation recovered no caller. It does
not qualify the disconnected control-body reader, admission or client compatibility.

## Inputs and scope

The user invoked the native-server continuation on 2026-10-12, authorizing bounded
offline static inspection. Each attempt has a fresh scope, fifteen-minute session,
exclusive command/output receipts and complete bound-file checks. The original
server executable is build 25621760, 108,067,840 bytes, SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
It was never launched or modified. No private player data, packaged assets,
terrain, gameplay tables or connected-client session was used.

## REA provider limit

The harness did not expose REA tools despite correct project registration. An
owned stdio MCP client connected directly to `.agents/rea/rea.sh`; the server
advertised 170 tools and `binary_session` succeeded. Commands followed the actual
advertised schemas. `open_binary` bound the correct artifact metadata promptly;
the provider started lazily on `procedure_callers`.

The first thirty-second request expired during import. A separately reviewed
retry allowed the configured 330-second provider startup plus a thirty-second
operation allowance. It returned `provider_timeout`, startup limit 330,000 ms,
after 331,103 ms. Neither attempt returned callers or references. The ephemeral
provider database offered no advertised retained-project reuse option. Repeating
the same import was stopped. Evidence and raw errors:
`work/rea-control-caller-20261013/` and its `execution-v2/`.

All 1,352 / 1,377 bound files respectively remained unchanged. Owned REA server,
Ghidra JVM and helper identities were verified gone. Metadata binding, tool
availability and import progress are tooling observations, not protocol evidence.

## Retained-database fallback

The diagnosed provider limitation justified direct headless Ghidra under the REA
skill and project rule. `tools/ghidra/ExportIncomingReferences.java` validates
stored executable SHA, one exact existing function entry and initialized executable
memory. It exports at most 256 incoming references and 64 KiB total into exclusive
TSV/manifest files. No analysis, disassembly, decompilation or database mutation
is requested. The manifest records exact output bytes/hash, coverage and effects.
Independent review and compilation against Ghidra 12.1.4/JDK 21 passed.

The launcher uses a new copy of the retained `Overnight` project, `-readOnly
-noanalysis`, a 180-second timeout, two CPUs, 2 GiB Java heap and sampled 6 GiB
owned RSS / 2 GiB case limits. Fresh Java home, configuration and temporary paths
are case-local. The inherited seccomp launcher denies new non-Unix sockets and
io_uring. Network namespaces are unavailable; no namespace, full filesystem
sandbox or hard cgroup memory guarantee is claimed. Installed launchers, launch
properties and JARs are hash-bound; unenumerated host native libraries remain a
provenance limitation.

The first run produced script output but its lifecycle wrapper rejected, with
no accepted result. All 249 bindings remained unchanged and cleanup was verified.
Source review found a plausible exiting-zombie race: the ownership loop could
read an empty environment after observing a live same-group process. The sanitized
receipt does not prove this was the historical trigger. A separate retry skips
zombies and rechecks PID/start-time/group/live state after environment reads,
including errors. Stable token mismatches still reject. Twenty-four actual-wrapper
mocked boundary checks pass, including unconditional cleanup after tracking or
resource failure. Original failure artifacts remain immutable.

The fresh retry exited zero at 2026-10-12 23:11:44 UTC, with all 279 bindings
unchanged and owned cleanup verified. Evidence:
`work/rea-control-fallback-20261013/execution-v2/`.

| Retained reference | Observation |
| --- | --- |
| Target | `0x1439ee7f0` |
| Source | `0x14671d164` |
| Reference type | Ghidra unsigned type ID 100; `is_call=false` |
| Containing source function | None recorded |
| Coverage | Existing retained program ReferenceManager only |

Reference TSV SHA-256:
`855f249ee6b0e9477b5e3cde3dc7cfe3a4360e06919857b0f328cc3aaad4e8a4`.
Manifest SHA-256:
`00bfb6a62ca00fe33cf9851db125d4c7746799a8d900f05e94d01e6f654487e6`.
This is direct fallback evidence, not returned REA Evidence. It does not discover
unknown references, indirect targets or missing instructions. No recorded caller
in this partial database is not proof of no callers. Further work must locate a
concrete dispatcher owner/call path before assigning archive or transport meaning.

## Bounded file-pattern candidates

A fresh independently reviewed fallback then scanned the pinned PE64 input using
an authored bounded parser, without executing, disassembling or importing it.
The installed REA portable layout profile supports ELF64, while its Ghidra path
builds a fresh `-import` / `-deleteProject` invocation and autodetects `.hop`, not
a retained `.gpr` database. Hash-matched installed source confirms this provider
limitation; no third unchanged cold import was attempted.

The parser requires standard PE64/x64 executable mappings, at most 96 sections
and 128 MiB input before hashing or reading. It excludes resources, overlays,
raw padding, virtual zero fill, writable data and executable sections without the
CODE flag. It checks complete five-byte E8/E9 rel32 patterns in eligible code and
eight-byte absolute pointers in eligible read-only initialized data. Combined
candidates are capped at sixteen, with no partial successful result above that
cap. Output contains neutral section IDs/offsets/addresses only, bounded to 64 KiB.
These patterns do not establish instructions, callers or vtable ownership.

Fifteen authored and thirteen independent synthetic PE checks pass. Review fixed
alignment, resource mapping and code-section eligibility gaps before execution.
Twenty-three actual-wrapper mocked checks and two independent pre-entry checks
pass, including authorization before target reads, oversize before allocation,
bounded result reads, complete schema validation and unconditional owned cleanup.
The pure parser and synthetic checks are retained as `tools/pe_candidates.cjs`
and `tools/tests/pe_candidates*.test.cjs`; fixtures read no original artifact.

The fresh offline sixty-second run passed with all 317 bindings unchanged and
verified cleanup. It examined 104,117,677 eligible starting offsets in eight
sections and returned one candidate: an absolute pointer at `0x145468198`, file
offset 88,501,656 / RVA 88,506,776, in read-only initialized section SEC02.
No E8/E9 pattern matched this exact target. Indirect calls, RIP-relative addressing,
RVA pointers, relocations and unscanned instruction forms remain unqualified.
The next discriminator is bounded context/owner provenance for that exact pointer.

Evidence: `work/control-caller-candidates-20261013/`; result SHA-256
`e7a65b3efc7f0cc5210f526530bc0955b4b47024363b919f1d1289647ba56461`.
This authored fallback result is not returned REA Evidence and establishes no
control archive, byte order, admission rule or client compatibility.

## Exact pointer context

A separately reviewed fresh case inspected a 128-byte window at
`0x145468158`–`0x1454681d8`, including the exact candidate at `0x145468198`.
It emits mapped addresses/flags only; unmapped values become `null`, without raw
bytes or names. A structural locator discriminator uses a pinned public compiler
profile: six u32 fields, image-relative signature/self relationships and bounded
descriptor mappings. It never claims RTTI, class identity or table ownership.
The primary ABI source is [LLVM MicrosoftCXXABI.cpp at e0416baf](https://github.com/llvm/llvm-project/blob/e0416baf412b44d09be9b300b50a15c57cdf148b/clang/lib/CodeGen/MicrosoftCXXABI.cpp).
Source/license/retrieval provenance are retained separately from the strictly
offline target run. The original compiler/profile is not established by this
reference. The potential secondary LEA scan was explicitly disabled.

Eighteen authored and eleven independent structural fixtures, twenty-eight
wrapper checks and two independent pre-entry checks pass. The actual run passed
with all 354 bindings unchanged and verified owned cleanup. The first nine
window entries map executable addresses, ending at the dispatcher; seven following
words are unmapped. No preceding slot points to a qualifying locator, so zero
locator bodies were read and no table hypothesis or LEA result was produced.
This does not prove RTTI is disabled or that the sequence is a vtable.

Evidence: `work/control-dispatch-owner-20261013/`; result SHA-256
`23a603994a721697446b88cbf66693b07fd95423f1dd68f549c8a3e7c5737df1`.
The next bounded discriminator is reference-pattern candidates to these nine
exact observed slots. Instruction boundaries, constructor/owner identity and
the actual archive path still require separate confirmation.

## Observed-slot address-reference candidates

A new reviewed case checks only REX.W LEA RIP-relative seven-byte patterns
targeting the nine exact observed slot addresses, selected from the hash-bound
accepted context output. These slots are not assumed table bases. It scans complete
eligible code offsets only, with the same sixteen-candidate/64 KiB fail-closed
limits and offline resource/ownership contract. Exception records and instructions
are not interpreted by this scan.

Twelve authored and nine independent PE checks, twenty-seven wrapper checks,
three actual-probe composition checks and two independent pre-entry checks pass.
Composition tests corrected a copied import/call mismatch before target work.
The fresh run passed with all 387 bindings unchanged and verified cleanup,
examining 73,913,697 eligible offsets. It returns two byte-pattern candidates:

| Candidate source | Target observed slot |
| --- | --- |
| `0x1439da30e` | `0x145468180` |
| `0x1439dae70` | `0x145468180` |

Evidence: `work/control-dispatch-table-references-20261013/`; result SHA-256
`eb10c15cb2aec7982956e4a37c2b22214407385b808ff3548b0869d27434f9e7`.
Neither is a confirmed instruction, constructor or caller. The next bounded
operation locates metadata containment in PE exception records, then checks
selected code with explicit fragment/whole-function limits before owner or
archive conclusions. No connection, admission or stock-client result is implied.

## Complete bounded exception metadata

The next reviewed case validates the complete AMD64 exception directory before
selecting the two exact candidates and known dispatcher. Its additional profile
requires sorted nonoverlapping intervals, complete executable file mappings and
aligned opaque unwind pointers; it rejects unusual variants rather than treating
this profile as universal. No code or unwind bodies are decoded.

All 323,249 records / 3,878,988 bytes pass. The recorded ranges are:

| Selected target | Recorded begin | Recorded end, exclusive | Bytes |
| --- | --- | --- | --- |
| `0x1439da30e` | `0x1439da2e8` | `0x1439da75a` | 1,138 |
| `0x1439dae70` | `0x1439dae50` | `0x1439db145` | 757 |
| `0x1439ee7f0` | `0x1439ee7f0` | `0x1439f0578` | 7,560 |

Twelve authored PE fixtures, nine independent PE fixtures, twenty-nine wrapper
checks, three actual-probe mocks and six independent boundary/schema checks pass.
The review added rejection of contradictory shared-record tuples. All 428 bound
files remain exact and owned cleanup passes. Evidence:
`work/control-caller-function-ranges-20261013/`; result SHA-256
`71f74cc712d962b570f63361dc6cd81d512625e5e72cee69fe4dccf92b717eff`.

These are recorded containment intervals, possibly procedure fragments in chained
or noncontiguous code. They do not prove instruction boundaries, primary entries,
complete functions, constructors, callers or owner/archive identity. Targeted
instruction evidence must preserve that distinction, including any analyst-defined
function used solely as bounded decompiler input.

## Targeted instruction context and preserved failed attempt

The first retained-project run failed before exporting results: Ghidra recursively
compiled the script directory's authored fixture API stubs, and the resulting
helper referenced `ProgramStub` instead of the installed API. All 476 bindings
remained exact and owned cleanup passed. The failed session, bundle, receipts and
diagnosis remain under `work/control-caller-instructions-20261013/`.

The reviewed retry uses the same Java source in an exclusive source-only leaf and
a fresh project copy/home/cache. Forty-four fixture groups pass, including classes,
fixture subtrees and symlinks rejected before target reads or launch. At 00:34 UTC
on 2026-10-13 the run passed with all 523 bindings unchanged and verified cleanup.
Both candidate sites decode as seven-byte LEA instructions. With analyzers disabled,
neither has a generated reference record; the helper reports zero confirmed slot
references. Independently joining those decoded boundaries with the frozen PE
scan's exact RIP-relative target calculation supports static materialization of
`0x145468180` at both sites. This does not prove reachability, an object store,
table base, constructor, owner or caller. The second metadata start has an existing database function but no
function containing the probe, so no decompilation input was selected for it.

The first site's decompiler input was explicitly forced from metadata; the known
dispatcher used its retained definition. Two decompilations completed. This does
not establish complete or correctly identified functions. The first output warns
about unreachable blocks beyond the approved metadata end: configuring a bounded
function body does not prove that native decompiler flow stayed within that body.
That output is retained as a limited local lead, not bounded-flow or owner proof.
Direct operand/pcode checks at the two sites are the next discriminator; the
inferred object-pointer store still needs instruction confirmation. The additive
independent post-run review retains these distinctions without changing bound
artifacts: `execution-v2/notes/independent-postrun-analysis.md`, SHA-256
`93e34312b12ebca7dd4cab8f2cae971da24e1b15a3e55f66adcbd8bdb6156ca5`.

Evidence: `work/control-caller-instructions-20261013/execution-v2/`; manifest
SHA-256 `0d92507405781fafb5341e00b8cd552c33653aaaacfceff3e4ef21a560b3f77e`,
context SHA-256 `fdfac273340bb962c9c71bd509def0c9d7a5eb37cca814801a0f50ab76b1d004`.
Caller/owner/archive identity and authenticated interoperability remain open.

## Instruction-only operand and local store proof

The next separately reviewed case uses no decompiler or function creation. It
exports at most eight sequential whole instructions from each fixed 32-byte
window, with at most 14 decoder lookahead bytes inside the qualified metadata
range. It stops at gaps, crossing instructions or non-fallthrough control; it
follows no outgoing flow. Numeric operands and raw p-code origins/widths are
bounded and validated. Every exported instruction hash matches the corresponding
fixed file span in the fully hash-checked original PE. Internal engine/cache reads
are not instrumented; the admitted decoder span and exported span are distinct.

Seven Java main fixtures plus authored and independent schema, source-isolation
and lifecycle checks pass, 58 checks/modes total. The target run passes with all
592 bindings unchanged, verified owned cleanup, eleven instructions and 31 p-code
operations. Both LEAs copy the eight-byte constant `0x145468180` into register
space 548, offset zero. At `0x1439da319`, an eight-byte store writes that value to
the address formed from register `548:0x38 + 0x28`; at `0x1439dae77`, the same
store uses `548:0x8 + 0x28`. The intervening instruction in the first window does
not overwrite the value or base register. Operand records independently agree
with the p-code COPY/INT_ADD/STORE chain. Unique temporaries are local to each
instruction and are not carried across instruction boundaries.

This proves local static stores, without identifying the base as an entry
parameter or an object of a particular class. Table identity, constructor status,
runtime reachability, receiver ownership and indirect dispatch remain unproved.
The next useful check traces the bounded prefixes before those LEAs, followed by
an actual receiver-load/indirect-call join and same-build type registration.

Evidence: `work/control-caller-operands-20261013/`; payload SHA-256
`2f142c4296638943dd668f6ae99577737a27125dd9a078fc33ba5acb7a87e35c`,
manifest SHA-256
`c0562a56611dc7328b1827963502ff694d60c06d8c73f05f6e759ad7128c24b5`.
The tested source is copied exactly to
`tools/ghidra/ExportBoundedInstructionOperands.java` (SHA-256
`1e0efbdcd66963dd888b70152b10c50fb9eb41f386dd29e4b214a5d9209b7831`)
and separately compiles against the installed Ghidra 12.1.4/JDK 21 APIs. The
tracked path is a source copy; the accepted target run used the isolated case path.

## Fixed prefixes and base-register limits

A separate instruction-only case covers fixed 38-/32-byte prefixes before the
two known LEAs, with fourteen admitted lookahead bytes and twelve instruction
attempts per window. Calls and other control operations stop export. The target
run passes at 2026-10-13 01:21 UTC: eleven instructions, 34 p-code operations,
all 646 bindings unchanged, original-byte hash joins and owned cleanup verified.
Seven Java main fixtures and the remaining independent/authored schema, source
isolation and lifecycle checks pass, 61 checks/modes total.

TARGET01 exports four instructions through the call at `0x1439da2f5`. Its earlier
eight-byte COPY at `0x1439da2ed` moves register `548:0x8` into `548:0x38`.
The call is a provenance barrier, and twenty bytes before the later LEA remain
unexported. No base-register connection across that call is asserted. A separately
scoped post-call window can close the local instruction gap without attributing
post-call register values to entry arguments or assuming ABI preservation.

TARGET02 exports seven consecutive fallthrough instructions to the earlier LEA.
Joining the two previously qualified LEA/store instructions gives the continuous
static interval `[0x1439dae50,0x1439dae7b)`. Register `548:0x8` is unchanged
throughout. An eight-byte LOAD at `0x1439dae5f` reads program memory at base
`+0x620` into register `548:0x38`. The sequence then stores `0x145467ef0` at
the base and `0x145468180` at base `+0x28`, both eight bytes. These are decoded
static semantics; no runtime load or instruction was executed. Instruction-local
temporaries, origins, widths and memory-space selectors are checked explicitly.

Independent interpretation and nine rejecting mutations corroborate these joins.
Metadata association at the selected prefix boundary still does not establish a
primary entry, parameter, constructor, class, vtable or actual receiver/caller.
Same-build type registration and an actual receiver-load/indirect-call join remain
required. No decompiler/function creation or outgoing flow is used in this case.

Evidence: `work/control-caller-prefixes-20261013/`; payload SHA-256
`1b19d44ba6fe627bccd15e99fd7124ea98b7b8a75aebbb74306296258ef8a61d`,
manifest SHA-256
`d86b32d71102b66b8d19b3ab14583e000711632d3dd536719ff88666b4f2f62c`.
The additive post-run interpretation is separate from the immutable 646-binding
set; its SHA-256 is
`5562bd5665f8fc1d1b404ba56a000f9d2e6f064bf749001c30b6e2875197b3b0`.
