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
