# Storage lifecycle

Keep full game-client and server installations locally available for research.
Do not replace them with partial downloads or depend on hosted archives.
Preserve original inputs, private state, evidence and unfinished work under
the existing project and scratch rules. Missing references, file extensions,
case age and closed state alone never establish disposability.

Before substantial downloads, builds or experiments, run the
[storage report](../../docs/runbooks/storage.md). Account for expected peak
growth across active cases, copied binaries, official installations and all
worktree targets. Existing per-case limits and the 30 GiB floor still apply;
reporting does not reserve space or authorize an experiment.

At session close, record which outputs must remain, which are reproducible
scratch, their owner and reproduction inputs. Run the report; reconcile stale
case states with their owners rather than closing them by age. At milestones,
review aggregate retained growth. Preserve required binaries as well as their
hashes: a digest alone cannot restore an artifact.

Cleanup requires a reviewed exact-path proposal with estimated allocated
bytes, reproduction justification, evidence checks, ownership and liveness.
Obtain user approval before deletion; recheck all guards at execution time.
Never run a broad prune when approval covers only a subset of its candidates.
Do not narrow evidence manifests merely to make cleanup possible.

Keep host log retention separate from research retention. Log rotation must
name exact logs and retention limits; never treat application databases,
conversation histories or other agents' worktrees as disposable caches.

## Reporting procedure

Run `bash tools/storage_report.sh` from the repository root.

The report performs no cleanup, builds or downloads. Missing tools or unreadable
paths fail the report. Sizes are snapshots, not reservations. `du` counts shared
hardlinks once per invocation, attributing blocks to the first encountered path.
Other worktree targets and the T3 boot log are measured separately; do not add
their totals to overlapping root measurements. The scratch scan has a 15-minute
timeout; a timeout is an incomplete report, never evidence of no candidates.

For an exact-path proposal, create an owned case with `case.json`, then save
`python3 -B tools/work_gc.py --json` output inside it. Review eligible paths
against reproduction instructions and ownership. Exclude held and deferred
paths. Include allocated size and pending checks in the approval manifest;
re-scan before execution. The snapshot is never deletion authority.

If capacity is insufficient, defer new work and report the shortfall rather
than delete protected inputs or lower the floor. Identical evidence bytes may
share an immutable store only after a separately reviewed preservation design.
