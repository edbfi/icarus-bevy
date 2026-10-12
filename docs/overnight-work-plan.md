# Overnight research and implementation batch

Historical record: the reverse-skill package was removed on 2026-10-12. Its
commands and package requirements below are preserved as approval history, not
current instructions. AGENTS.md links identify the former upstream revision.

Status: **historical; execution window expired**. Approved by the user
("Go ahead") on 2026-10-04 for the window below.
Recorded window: 2026-10-04 21:13:19 UTC to 2026-10-05 05:13:19 UTC
(23:13–07:13 Europe/Berlin). Scope and timestamps are in
`work/icarus-overnight/scope.md` and `session.json`.

For current work, follow the [canonical plan](linux-dedicated-server-plan.md#autonomous-execution-and-maintenance)
and [implementation status](implementation-status.md). The 2026-10-12 user
instruction permits frequent feature-branch pushes and supersedes this batch's
no-push rule. Its eight-hour clock and runtime authorization remain historical;
do not reuse its session as a new approval. Maintain results in the existing status
and research documents instead of creating the once-planned overnight report.
The remaining text preserves the original batch contract and is not an active queue.

## Objective and duration

Use an **eight-hour window from approval** to advance the independent native Linux
Rust/Bevy server: recover connection and persistence behavior, implement justified
pieces, investigate failure handling, and test the native foundation. Preserve
Rust 1.99.0 and Bevy 0.19.1. The existing comprehensive
[project plan](linux-dedicated-server-plan.md) and its release gates remain in force.

Deliver a morning report with findings, code changes, test and measurement results,
commits, unresolved questions and the next useful experiment. A playable replacement
is not promised overnight. Stock-client compatibility, official-save fidelity and
improvement over the original remain outcomes to demonstrate.

Stop earlier if all useful in-scope work is exhausted. A blocked research branch
does not stop the batch: switch to the fallback queue below. At the deadline,
stop owned experiments, preserve evidence and finish the handoff. Do not leave
unattended reference servers or schedule future work beyond this window.

## Work sequence

Times are budgets, not a requirement to spend time on a stalled approach. A native
soak may overlap lightweight analysis, subject to the resource limits below.

| Approximate window | Work | Concrete deliverable and decision gate |
| --- | --- | --- |
| 0–30 minutes | Establish overnight scope, verify baseline hashes and available tools, prepare reusable bounded helpers | Case records, command journal and checked helper source; no repeated inventory of unchanged 10 GB content unless needed |
| 30 minutes–2.5 hours | Trace network-version construction/callback registration, string/archive decoding, prospect readers and failure output | Address-backed behavior notes, call paths, field constraints and unresolved semantics; targeted Ghidra exports where useful |
| 2.5–4 hours | Run discriminating isolated reference experiments when static evidence provides a usable input/entry point | Repeated observations for startup, local negotiation or disposable save failures; distinguish environment failures from original defects |
| 4–6 hours | Implement bounded independent Rust components supported by evidence; otherwise strengthen native persistence and lifecycle | Reviewable source, authored fixtures, meaningful regression tests; unknown compatibility behavior remains unavailable |
| 6–7.5 hours | Native fault/recovery campaigns, sustained fixed-workload run and profiles; fix demonstrated problems | Raw results, reproducible workload description, before/after measurements for any optimization |
| 7.5–8 hours | Full checks, evidence review, cleanup and final logical commits | Morning report and clean handoff with precise limitations |

### Research priorities

1. Follow the version generator's cache, checksum/transformation helpers and
   callback registration. Recover exact inputs and byte/string handling; distinguish
   defaults from the active ICARUS policy. Do not force acceptance or spoof identity.
2. Trace control-message serialization and admission checks outward toward framing.
   Recover lengths, bounds, byte/bit order and state transitions before implementing
   codecs. Inner control messages are not complete UDP packets.
3. Follow the by-name and from-file prospect paths into readers/converters. Identify
   actual fields, format/version markers, conversion failures and application order.
   Establish which dedicated-server commands reach those paths.
4. Resolve the failure-output virtual target before calling it fatal. Investigate
   missing/truncated data, name mismatches and partial-load behavior with disposable
   fixtures only. A candidate becomes a bug only with reproducible evidence.

Use the existing evidence as the starting point, including the split runtime ranges
in the version generator. Do not repeat whole-program analysis without a specific
question. Timebox an unproductive hypothesis to roughly 45 minutes before pivoting.

### Rust implementation and fallback queue

Implement only independently authored code. Prefer one small vertical piece with
tests over scaffolding for many unknown systems. Eligible pieces include bounded
byte/bit readers, established string/checksum primitives, configuration mappings,
and a read-only format inspector if sufficient evidence exists. Keep experimental
codecs disconnected from a production listener until their framing and admission
behavior are established. Synthetic tests are not stock-client validation.

If compatibility research is blocked, proceed through these useful independent tasks:

1. Audit and test save-worker errors, backpressure, stale results and shutdown
   deadlines; fix demonstrated native failures with regression coverage.
2. Test interrupted publication, corrupt generations, oversized input, denied writes
   and disk-full behavior using case-local files, injected I/O failures or a small
   container tmpfs. Never fill the host filesystem or simulate a host power failure.
3. Exercise repeated start/save/stop/abrupt-exit/recovery cycles with authored state;
   check acknowledged generations and state hashes rather than process exit alone.
4. Run up to two hours of a fixed synthetic population with periodic saves and
   bounded mutation. Record memory, tick-time distribution, missed deadlines and
   save latency; keep warm-up and workload dimensions explicit.
5. Profile the authored workload. Optimize only a measured bottleneck, retaining
   correctness and comparing repeated before/after runs. Do not compare this empty
   or synthetic workload against the original startup as an ICARUS speedup claim.

## Single-approval execution contract

**Approval of this document authorizes the whole bounded batch, including writing,
reviewing, correcting, compiling and executing new or revised project-local helpers
for the tasks and command families below. Individual helper revisions, function
addresses, fixture bytes and test cases within these limits need no further approval.**
This is explicit advance authorization for those scripts and their stated effects,
not an inference that reading the toolkit authorizes execution.

Helpers belong under `tools/`, `tools/ghidra/`, `tests/` or the native project's
source/examples. Generated helpers, fixtures, logs, databases and raw recovered
material stay in `work/icarus-overnight/`. Helpers may invoke only tools and actions
within this contract. Review each helper before execution and record its source
hash plus the actual argument vector, inputs, output paths and timeout in the case.
No downloaded script execution, opaque shell installers or expansion into a new
side-effect category is authorized.

Repository changes and local Conventional Commits are included. Continue on
`feat/native-server-foundation`; commit coherent `feat`, `fix`, `test`, `perf`,
`docs` or `chore` changes regularly, after their relevant checks. Do not push, merge,
publish a release, rewrite history or commit directly to `main`.

## Target scope and allowed effects

- Read the existing original installation in `icarus-orig/`, the approved prior
  case artifacts and this repository. Preserve original bytes and prior evidence.
- Read/disassemble/decompile the shipping executable and related bundled libraries
  when a concrete dependency/call path requires them. Inspect imports/exports first
  for newly selected libraries. No unpacking game asset archives or exporting game
  data tables, terrain, collision, navigation, models, audio or other assets.
- Create a new case and append evidence, findings, paths, work items and timelines.
  Update reusable source and sanitized documentation outside ignored directories.
- Run original code only in the isolated container contract below. Permit ordinary
  logs and file-output observation, not debugger injection, binary patching or
  instrumentation. Keep any automatically generated original data private; do not
  reuse it as replacement content.
- Create and mutate disposable authored native state and small synthetic reference
  inputs; preserve the inputs and outcomes needed for reproducibility. No production
  saves, accounts, passwords, credentials or real players are involved.
- Run and profile the native replacement and authored test/probe binaries. Send
  local test traffic only as described below; all sent/received bytes may be recorded
  by the probe itself. No host packet capture or promiscuous interface access.
- Read public technical documentation over HTTPS, preferring primary sources.
  Cargo may fetch needed open-source dependencies from its normal registry and
  download storage, with provenance/license review and a committed lockfile. No
  game/client/SDK downloads, Docker image pulls, new system packages, credentials,
  paid services or uploads of originals/private evidence. Use installed tools;
  if another tool is required, use an available alternative or defer that branch.
- Ordinary Cargo/Ghidra/pre-commit caches are allowed. No global hooks, shell
  profiles, daemon settings, firewall rules, mounts or client-global configuration
  changes. Existing repository hooks remain enabled.

## Case initialization and command families

These are parameterized command templates: approval covers substituting verified
addresses, bounded lengths and case-local paths, and implementing the described
helpers. Record exact expanded commands before each batch. Execute from the project
root. Use canonical vendor script paths.

```bash
bash .agents/vendor/reverse-skill/skills/scripts/master-route.sh \
  --hint "game reverse ICARUS offline server compatibility"
bash .agents/vendor/reverse-skill/skills/scripts/case-init.sh \
  --hint "game reverse ICARUS overnight offline lab" \
  --case-name icarus-overnight --project-root "$PWD" \
  --auth-granted --auth-basis own_system \
  --evidence-of-auth "User approval of docs/overnight-work-plan.md" \
  --sample "$PWD/icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe" \
  --in-scope-asset "$PWD/icarus-orig" --network-profile offline
bash .agents/vendor/reverse-skill/skills/scripts/case-guard.sh \
  --case-root work/icarus-overnight
```

Complete scope before ACT: roles `lead`, `cre`, `doc`; local binary, case files and
isolated container surfaces; static analysis/decompilation, isolated original
execution, local-only probes, authored fault tests, native implementation and
reporting. `offline` means original code has no external network route; namespace
loopback traffic is explicitly in scope. Public documentation and dependency
retrieval run separately from target execution. Do not overwrite an existing case:
resume it if it represents this approval, otherwise use a timestamped suffix.

| Command family | Permitted parameters and bounds |
| --- | --- |
| `rabin2 -I/-i/-E/-S/-z` with JSON variants | Authorized executables/libraries; artifact output under the new case |
| `timeout 120s r2 -N -q … -c '…;q' SAMPLE` | Read-only metadata, bytes, strings, xrefs, bounded function analysis/disassembly; no write/debug/HTTP-server commands |
| `python3 tools/…py …`, `bash tools/…sh …`, equivalent inline helpers | PE metadata/reference discovery, artifact hashing, annotations, bounded orchestration, fixture generation, native test/measurement harnesses and isolated-container operations only |
| Headless Ghidra, template below | Targeted disassembly/function discovery and decompilation; no target execution or symbol downloads |
| `cargo fmt`, `cargo clippy`, `cargo test`, `cargo build`, `cargo run` | This native project and authored helpers/examples; use `--locked` after intentional dependency resolution |
| `/usr/bin/time`, process/resource inspection, installed profilers | Only owned test/analysis processes; no unrelated process attachment or collection |
| `docker run/exec/logs/inspect/stats/stop/rm/cp` | Only the labeled containers created by this batch, pinned local image, mounts and limits below |
| `python3 .agents/vendor/reverse-skill/skills/case-review/scripts/review_case.py work/icarus-overnight --verify-hashes --strict` | Local evidence review; fix case records or report a review failure honestly |
| `prek run --all-files --hook-stage manual`, `git diff/status/add/commit` | Required checks and categorized local commits; no bypassing hooks |

Ghidra template, with argument values selected and journaled before execution:

```bash
JAVA_HOME="$(brew --prefix openjdk@21)/libexec" \
JAVA_TOOL_OPTIONS="-Xmx4g" \
timeout 900s "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" \
  "$PWD/work/icarus-overnight/ghidra" Overnight \
  -import "$PWD/icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe" \
  -analysisTimeoutPerFile 300 -max-cpu 2 \
  -scriptPath "$PWD/tools/ghidra" \
  -preScript OfflineAnalysis.java \
  -postScript OVERNIGHT_HELPER.java CASE_OUTPUT SELECTED_ADDRESSES
```

`OVERNIGHT_HELPER.java` means the locally authored, reviewed helper covered by this
approval. It may create function definitions, follow references and export bounded
analysis inside the Ghidra database; it may not patch the original. Processing the
case database with `-process` and `-noanalysis` instead of a fresh import is included.
Disable symbol-server analyzers before analysis; never enable PDB downloads.
Limit each batch to 24 selected functions, 30 seconds per decompilation and 15 minutes
overall; stop an unproductive sequence after two batches and change method.

## Isolated reference experiment contract

Adapt `tools/run_reference_offline.sh` into a reusable launcher with unique run
directories and reliable timeout cleanup. This approval includes executing that
revised launcher within the following fixed constraints:

- Image: `ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237`.
- Mandatory flags: `--pull never --network none --read-only --cap-drop ALL
  --security-opt no-new-privileges --memory 6g --cpus 2 --pids-limit 512
  --shm-size 256m`. Run as the existing unprivileged image user. No published ports,
  host networking, host PID namespace, privileged mode, added capabilities or Docker
  socket inside the container.
- Mount `icarus-orig/` at `/reference` read-only and the specific run directory at
  `/home/container` writable. Case-local native helpers may be mounted read-only.
  Temporary writable `/tmp` is at most 1 GiB; `/run` at most 16 MiB.
- Use the existing Proton/xvfb launcher, Steam app IDs and private UserDir. Variants
  may change offline diagnostic/startup/settings arguments or an authored fixture
  path. Arguments may not enable external services, alter identity validation,
  redirect writes outside the case, or point to real saves. If the original needs
  a writable installation or connected Steam service, record that limitation and
  switch tasks rather than broadening the mounts/network.
- At most **12 reference runs**, **15 minutes each**, **one at a time**. Limit
  repetition of an unchanged environment failure to two attempts.
- `docker exec` may run an authored native probe inside that same container network
  namespace against **127.0.0.1 ports 17777 and 27015 only**. Targeted protocol probes
  require evidence-backed framing; at most 10 requests/second, 1,000 requests/run,
  and 64 KiB/request. No blind high-volume fuzzing of the original. Record both sides
  of probe I/O without granting raw-packet capabilities. No external client is needed.
- Save experiments use fresh case-local inputs, up to 1 MiB each initially. Generate
  only independently authored fields justified by recovered format observations.
  Preserve an unmodified fixture alongside each mutated copy. No archive extraction.
- Capture pre-stop and post-stop logs, exit status, resource samples and produced
  file metadata; stop and remove only the exact container ID created by this run.
  Verify cleanup on success, failure and timeout. Never use global Docker prune.

If startup or platform initialization prevents meaningful protocol/save experiments,
do not interpret that as a game defect and do not spend the night repeating it.

## Resource limits and supervision

Planning inspection found 8 logical CPUs, about 15 GiB RAM and 106 GiB free disk.
Use conservative limits on this shared host:

- At most four CPU-intensive workers/threads across heavy analysis/build/test jobs;
  use two Cargo build jobs and two Ghidra analysis CPUs. Run the reference container
  separately from heavy Ghidra/build work. Lightweight log review may overlap.
- Monitor aggregate owned-job memory; target at most 10 GiB and stop/defer heavy
  work if host available memory falls below 3 GiB. Ghidra heap is capped at 4 GiB.
- Cap new research/runtime artifacts at 12 GiB and total batch disk growth including
  build/cache outputs at 20 GiB. Stop growth if free space falls below 30 GiB.
  Retain evidence; remove only reproducible scratch created by this batch when needed.
- Keep each generated fixture, queue and probe allocation bounded. Native stress
  tests may deliberately kill their own processes; never kill unrelated processes.
- Give every child experiment a timeout and cleanup handler. Check progress in
  short intervals and write checkpoints after each meaningful milestone. A two-hour
  soak does not require a two-hour blocking tool call.

## No-interruption policy and handoff

After approval, **do not ask the user for routine permissions or preferences during
the window**. Choose conservative reversible options within this contract. If a
tool/platform approval cannot be avoided, a new side-effect category is required,
or a task needs the Windows client, leave that branch pending and continue other
in-scope work. Do not treat the user's absence as expanded authorization.

Before handoff:

1. Run relevant tests after changes and the full manual check suite at the end.
   Retain failed-test evidence and fix failures where possible; do not disable tests.
2. Review evidence hashes, scope and finding/path references. Separate static
   observations, synthetic tests, original reproductions and validated compatibility.
3. Verify all owned containers/processes stopped, originals unchanged, private
   artifacts ignored and staged changes free of copied assets, raw original code
   and secrets. Preserve user changes and report any unfinished local edits.
4. Commit coherent completed work and write `docs/overnight-report.md` with achieved
   results, measurements, failures, deferred tasks, commit IDs and next actions.
   Report actual soak duration and workload; never promote it to the three 72-hour
   qualification runs or a production performance claim.

The vendored [AGENTS.md](https://github.com/zhaoxuya520/reverse-skill/blob/cab634bd855fc287f6e420c1f36fd1a6b9245960/AGENTS.md) says:
"Explicit user approval is required before running any repository script."
It also requires renewed consent for new side-effect categories. The user's approval
of this enumerated batch supplies advance script/target authorization for these
categories, including the specified helper revisions and experiments, so repeated
approvals are unnecessary. Anything outside the contract is deferred until morning.
