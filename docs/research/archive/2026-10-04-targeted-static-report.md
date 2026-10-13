> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings.md). Moved from `docs/research/` on 2026-10-13.

# Targeted connection and prospect-load analysis

Historical report — 2026-10-04. Findings and proposed next steps retain their
original scope. Later reports cover the
[default checksum](../../specs/network-checksum.md) and
[failure-output target](2026-10-04-save-failure-output.md); they do not rewrite
the earlier unknowns below. See the [current checkpoint](../../implementation-status.md)
for pending work and pause status. Reproduction commands are historical, not
authorization for a new batch; current tooling follows the project REA rules.

The targeted pass located three diagnostic references that the earlier bounded
Ghidra pass did not recover. Inspection identifies a control-message dispatcher,
a version comparison with an optional callback, and two prospect-loading paths.
These are static observations from one executable, not stock-client validation or
reproductions of the reported crashes. No original code or assets entered the
replacement; its Rust implementation is unchanged by this research.

## Scope, method and limits

This continues the user's instruction to investigate the existing server files
offline, within `work/icarus-server-baseline/scope.md`. The original remains
read-only; the case guard passed. There was no original execution, network
experiment, download, packet capture or asset extraction in this pass. Existing
import-table evidence E-003 precedes function analysis. Tools: Python standard
library and the installed radare2 6.2.2; no additional toolkit scripts were run
apart from the already approved case guard.

Reference: build **25621760**, executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`,
preferred image base `0x140000000`. All addresses below are preferred virtual
addresses for this exact build, not portable symbols or runtime addresses.

The initial read-only scan searched executable sections for RIP-relative LEA/MOV
candidates pointing at the three selected diagnostics, then looked up enclosing
ranges in the PE exception directory. Each candidate landed on an instruction
boundary in disassembly beginning at the corresponding runtime-function entry.
This is stronger than a raw byte match; it still does not prove dynamic reachability.
Linear disassembly can include embedded tables. The control dispatcher ends with
such a table, which was interpreted as data rather than executable instructions.
Runtime-function entries can also split a single routine: the version generator
fell through its first range, so a follow-up covered five adjacent ranges and
checked their branch targets. A range endpoint alone is not a function boundary.

The subsequent pass inspected only selected direct callees, bounded to 16 KiB
each and 60 seconds per radare2 command. One failure helper had no exact runtime
entry, so inspection used 12 instructions and did not invent a function size.
Ghidra was not rerun and no new decompilation is claimed.

## Evidence and address map

IDs are local to `work/icarus-server-baseline/`. Each `evidence/E-*.md` records its
artifact path, SHA-256, observation time and reproduction command. Raw assembly
and diagnostic annotations remain in ignored `work/`.

| Evidence | Address / range size | Observed role; names are analyst descriptions |
| --- | --- | --- |
| E-008 | Three anchors below | Candidate references and enclosing runtime ranges |
| E-009 | `0x14146e6c0`, 3,162 bytes | By-name prospect loader; reference to `0x1449f1550` at `0x14146f195` |
| E-010 | `0x1414baf60`, 1,239 bytes | From-file prospect loader; reference to `0x1449f6660` at `0x1414bb176` |
| E-011 | `0x1439ee7f0`, 7,560 bytes | Control dispatcher; version diagnostic reference to `0x14546d780` at `0x1439eecd4` |
| E-012 | `0x141f2e6e0`, 99 bytes | Version equality fallback or callback dispatch |
| E-013 | `0x1439f0530`, 18 entries | Message-ID dispatch table |
| E-014 | `0x141824760`, 959 bytes | First reader called by E-010 |
| E-015 | `0x141823fb0`, 259 bytes | Second reader called by E-010 |
| E-016 | `0x141ee8930`, 12 instructions inspected | Failure-output forwarding helper; trailing padding is not behavior |
| E-017 | `0x141817bd0`, 996 bytes | Preliminary loader called by E-009 |
| E-018 | `0x14181e530`, 577 bytes | Data loader called by E-009; `.json` suffix and name comparison |
| E-019 | `0x1418e6140`, 251 bytes | Shared archive conversion used by E-014/E-015 |
| E-020 | `0x141f31140`, 330 bytes | Output formatting and virtual output call |
| E-021 | `0x141e15db0`, 1,311 bytes | Signed-length string serialization used in negotiation |
| E-022 | `0x141f2b220`, initial 172 bytes | Incomplete first runtime range of local version generation |
| E-023 | `0x141804d30`, 339 bytes | Shared by-name loader forwards to unresolved conversion |
| E-024 | `0x141f2b220`, extended 778 bytes | Version cache, callback and default checksum construction across five runtime ranges |

For a bounded range in the table, reproduce with the installed tool, substituting
the numeric address and byte count. The example is the version comparison:

```bash
timeout 60s /home/linuxbrew/.linuxbrew/bin/r2 -N -q \
  -e scr.color=0 -e bin.relocs.apply=false \
  -c 'pDj 99 @ 0x141f2e6e0;q' \
  icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe
```

Preserve output under a fresh case artifact name. For E-016 the command was
`pd 12 @ 0x141ee8930;q`. E-013 decodes 18 little-endian uint32 values at the
table address and adds the image base; the dispatcher explicitly indexes this
table with the incoming message ID after a range check. The follow-up argument
vectors are also retained in `static/targeted-followup-commands.json`.

## Findings

All findings below have severity `n/a_re`, category `reverse_algo` and status
`candidate`. Confidence describes the static interpretation only. The observations
share one binary and lack independent runtime corroboration.

**F-002 — Version compatibility is conditional (high confidence; E-011/E-012).**
The negotiation branch obtains a local 32-bit value and reads a remote 32-bit value,
then calls `0x141f2e6e0`. That helper returns equality when no usable callback is
available. Otherwise it dispatches the two values through the callback. The caller's
failure branch references an incompatible-version diagnostic and connection-close
accounting; success proceeds to token handling. The follow-up partially identifies
local value generation (F-007); whether ICARUS registers a custom callback remains
unresolved. Hard-coding equality as the entire ICARUS compatibility policy would
therefore be premature.

**F-003 — Control negotiation has several distinct branches (high confidence;
E-011/E-013/E-021).** The server-side dispatcher checks message validity before
dispatch. IDs 0, 4, 5, 9, 10, 15 and 17 have non-default entries in this table.
Nearby diagnostics identify version negotiation, netspeed, login, join, split join,
connection swapping and debug text respectively. ID 0 reads one byte, a 32-bit
version and a string-shaped value before checking archive error state and version
compatibility. This establishes an inner message-processing observation, not a
complete packet layout: framing, bit alignment, reliability and connection state
are still unknown. See the [negotiation notes](../../specs/control-negotiation.md).

**F-004 — A from-file load stages two reads before state application (medium
confidence; E-010/E-014/E-015/E-019).** The outer function checks both reader
results before calling the apparent state-application routines. Both reader
candidates receive the same path object, call `0x141f30550`, then use the shared
archive conversion. The first does not immediately branch on the initial call's
return; the second does. The first still checks its later conversion result.
The initial callee's exact contract and both readers' schemas are unknown. This
asymmetry is a test lead, not proof of incorrect error handling or a double-read
race. The full save format cannot be inferred from a `.json` suffix elsewhere.

**F-005 — Load failure enters reporting whose termination behavior is unresolved
(medium confidence; E-010/E-016/E-020).** Reader failure reaches diagnostic output,
then `0x141ee8930`, which updates a global flag and forwards into a formatter and
virtual output call. The caller contains cleanup and a false-return path afterward.
Neither a fatal-looking log call nor trailing `int3` padding establishes a crash;
the dynamic output target and its return behavior must be resolved first.

**F-006 — The by-name path distinguishes lookup, load and name validation (medium
confidence; E-009/E-017/E-018).** The outer function has separate empty-name,
missing-file, preliminary-load failure and subsequent-load branches. One helper
constructs a `.json`-suffixed path and, after successful loading, compares a
string-shaped field in the result against the requested name. Exact field names,
case rules, schema and all error outcomes remain unconfirmed. Preserve these
stages separately when designing import diagnostics; do not collapse every failure
into an empty new world.

**F-007 — Local version construction has its own cache and callback (high
confidence; E-011/E-022/E-024).** The initial negotiation calls the generator with
its callback option enabled. The generator first returns a cached value when
available; otherwise it can obtain a checksum from a callback or enter default
construction. That default path formats a string containing the project name,
another string, NetCL, EngineNetVer and GameNetVer, transforms it through a helper,
then calls a checksum-producing helper with a zero second argument and caches the
result. It also inspects a network-version override. Exact checksum algorithm,
string normalization, runtime inputs and active callback registration remain
unknown. The visible game release number alone is not a discovered wire version.

## Paths and bug investigation

P-002 (`callflow`): control-message validity check → dispatch ID 0 → read fields
and check archive error → compare versions through fallback/callback → reject or
continue token handling (E-011/E-012/E-013/E-021, F-002/F-003).

P-003 (`callflow`): from-file loader → reader A → reader B → apply state on success;
either reader failure → reporting helper → formatter/virtual output, with actual
termination unresolved (E-010/E-014/E-015/E-016/E-019/E-020, F-004/F-005).

P-004 (`callflow`): by-name loader → preliminary prospect information → second
load and name comparison → proceed toward map loading or report failure
(E-009/E-017/E-018, F-006).

Useful later experiments are a missing file, truncated file, malformed contents,
mismatched embedded prospect name and interruption between staged reads. Use only
disposable authored/player-created state in a separately approved runtime case.
First establish which entry path dedicated-server commands actually call. Capture
exit status, logs and resulting state; do not label the current hypotheses bugs.
For the replacement, parse and validate into staging state, publish only after
validation, and provide distinct recoverable errors. These are design requirements,
not claims that an original defect has been fixed.

## Timeline and next gate

The case timeline records the transition from unsuccessful whole-program analysis
to targeted references, then bounded direct-callee inspection and synthesis. The
three original anchors are now located; the earlier timeout report remains valid
as the result of that earlier pass.

Next static targets are active version callbacks and checksum construction, the
archive converter's schema and the shared file-loading callee's contract. A
working stock-client connection and controlled save fixtures are still required
before promoting these observations to validated specifications. No additional
game installation is needed for this offline work.
