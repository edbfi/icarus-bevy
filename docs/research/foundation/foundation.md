# Native foundation and reference baseline

Native behavior and qualification; original server build 25621760/client
25793767 baseline. Save-class, C001 and control-store work have separate digests.

Before restructure: 96ddb0b:docs/research/foundation/2026-10-13-foundation-receipts.md

## Question cluster

- What native lifecycle, persistence, inspection and fault handling is qualified?
- What tooling and original baseline support it, within which limits?

## Current answer

The independent foundation is not ICARUS-compatible (F-487). Configuration and
provisional 30 Hz scheduling: F-868. Native snapshots, atomic publication,
two-generation recovery, stale/failed-worker handling and bounded final save:
F-869, F-771; incomplete-state rejection: F-482.

Authored schemas 4–10: spatial state (F-919), foundation links (F-922), lifecycle
(F-923), C001/scalar/inventory persistence (F-927), runtime integrity/callback
exclusions (F-928). Schema 11 preserves opt-in modifier timers/cache and owner authority through
reload, replacement and retry; failed steps are atomic (F-931). Procedures and limits:
[authored worlds](../../runbooks/authored-worlds.md).

Bool/I32 assessment retains provenance and unqualified restore/RepNotify obligations
even when empty (F-929); fixture exclusions remain in the runbook.

Schema-7/8/9 preflight and schema-11 modifier-call costs: F-926. Full modifier
preflight remains per step, even empty; full ticks and uniform latency,
peak-memory or speedup gains remain unqualified.

Filesystem/input faults and diagnostic redaction: F-239, F-671, F-870, F-880,
F-423. Synthetic load/restarts (F-871), interrupted soaks and compressed 72-hour
schedules (F-872, F-873), modeled POSIX crash recovery (F-511) measure neither
ICARUS performance nor physical power loss. Earlier soaks do not qualify later
persistence changes (U-035).

Bounded opaque inspection with `world_imported` false: F-880 to F-883;
lossless schema-3 sidecars: F-567; structured readers: F-886 to F-888.
Randomized preservation/codecs/readers and bounded peer tables: F-604 to F-607;
fail-closed corrupted stores: F-611.

Loopback authored-probe admission, cleanup and version pinning: F-875;
held-session updates/recovery and controller limits: F-833; bounded concurrency/partial-bunch
sweeps: F-877. No ICARUS client has used this layer; the interrupted rerun
has no final result/exit receipt (M-898).
Input parity (F-828) permits the anonymous, byte-matched dedicated-server
data.pak fetch (F-829, F-831).

Toolchain/headless linkage: F-879; typing: F-484; CI: F-497; client identity:
F-878/F-731 (U-034 resolved). Original fixity/export/entry-map/exchange limits:
F-265/F-266/F-268/F-253. Launcher revision unrun (F-252); runtime windows
expired (F-218).

## Open

- Authentication/stock client (U-038); world import (U-045).
- Original bug reproduction (U-047); ICARUS performance/stability (U-049).
- Paced multi-day effects/physical power loss (U-035).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-972, M-1012..M-1019.
