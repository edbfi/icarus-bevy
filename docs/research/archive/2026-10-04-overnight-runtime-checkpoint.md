> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings.md). Moved from `docs/research/` on 2026-10-13.

# Offline reference resume experiment

Historical report — 2026-10-04. The runtime window below has ended. Later
[by-name rejection tests](2026-10-04-prospect-rejection.md) used a separate entry
path and do not resolve this command probe. See the
[current checkpoint](../../implementation-status.md) for pending work and pause status.

The overnight reference run observed the original for **120 seconds**, then stopped
and removed its container. It remained running until the harness stopped it; the
resulting exit code was 130 and Docker reported no OOM kill. This is not a
reproduction of a spontaneous server crash.

The launcher supplied `-ExecCmds=ResumeProspect`, using a command-name observation
as a reachability probe. The original log includes that argument, but contains no
evidence that the command executed or reached prospect loading. The server again
loaded the entry map and reported an IP driver listening on port 17777, while
Steam dedicated-server initialization failed with AppId 0. No request traffic was
sent in this run. Neither playable startup nor save handling was validated.

Do not repeat this unchanged startup environment to infer save behavior. Further
original save tests need a demonstrated entry point. The failed command probe is
retained as a negative result, not silently converted into a missing-file test.

## Isolation and provenance

The approved [overnight contract](../../history/overnight-work-plan.md) authorized this run.
`tools/run_reference_offline.py` creates a fresh run directory, journals arguments
and its source hash, invokes the case guard, and captures logs before and after
stopping the exact labeled container. Docker inspection verified:

- Pinned pre-existing image; no pull and `network=none`.
- Read-only root filesystem and original installation mount.
- Unprivileged `container` user, all capabilities dropped, no-new-privileges.
- 6 GiB memory, two CPUs, 512 PID limit and bounded temporary filesystems.
- Writable home limited to the run's private case directory; no published ports.

Original executable build and hash remain the same as the
[static checkpoint](2026-10-04-overnight-static-checkpoint.md). Private overnight
evidence E-035/E-036 records the Docker state/cleanup summary and final game log.
Full invocation, resource samples and both console captures are under
`work/icarus-overnight/runtime/run-01-resume/`. Original generated files stay private
and are not replacement content.
