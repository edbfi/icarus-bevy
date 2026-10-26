# Native foundation and reference baseline

Native qualification; original baseline: server build 25621760/client 25793767.
Save-class, C001 and control-store digests are separate.

Before restructure: 96ddb0b:docs/research/foundation/2026-10-13-foundation-receipts.md

## Question cluster

- What native lifecycle, persistence, inspection and fault handling is qualified?
- What tooling and original baseline support it, within which limits?

## Current answer

No ICARUS compatibility (F-487). Configuration/provisional 30 Hz: F-868;
persistence/recovery/stale/failed-worker handling: F-869, F-771;
incomplete-state rejection: F-482. See the [operating guide](../../runbooks/native-server.md).

Authored schemas 4–10: F-919, F-922, F-923, F-927; runtime integrity/callback
exclusions: F-928. Schema 11 retains opt-in modifier timers/cache and owner
authority through reload (including near-budget stores), replacement/retry;
failed steps are atomic (F-931). See [authored worlds](../../runbooks/authored-worlds.md)
for procedures/limits.

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

Bounded opaque inspection (`world_imported` false): F-880 to F-883;
lossless schema-3 sidecars: F-567; structured readers: F-886 to F-888.
Randomized preservation/codecs/readers and bounded peer tables: F-604 to F-607;
corrupted stores fail closed: F-611.

Opt-in schema-4 bridge (F-938): [installation and limits](../../runbooks/native-server.md).
Saved-pose/physics mappings remain U-025; stock-client behavior is unqualified.

Loopback authored-probe admission/cleanup/version pinning: F-875;
held-session updates/recovery/controller limits: F-833; historical metrics:
F-932; bounded concurrency/partial-bunch sweeps: F-877. No ICARUS client used
this layer. Missing rerun receipts: M-898. The 48 h attempt hit the disk guard
without a final probe summary; copied reload passes (M-1055).
Input parity (F-828) permits the anonymous, byte-matched dedicated-server
data.pak fetch (F-829, F-831).

Toolchain/headless linkage: F-879; typing: F-484; CI: F-497; client identity:
F-878/F-731 (U-034 resolved). Original fixity/export/entry-map/exchange limits:
F-265/F-266/F-268/F-253. Launcher revision unrun (F-252); runtime windows
expired (F-218).

With fresh archetype absence checks, three matched debug runs per version
give host medians of 29.08–32.08 ms at 1,024 actors versus 38.74–40.25 ms;
p95 still exceeds a 30 Hz interval (F-939). Ranges overlap at 1/64 actors.
Earlier one-run host medians at 1/64/1024 actors were 1.17/3.94/41.59 ms; lab
baselines were 0.50/0.51/1.03 ms (M-1057). Release and ICARUS performance
remain unmeasured.

## Open

- Authentication/stock client (U-038); world import (U-045).
- Original bug reproduction (U-047); ICARUS performance/stability (U-049).
- Paced multi-day effects/physical power loss (U-035).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-972, M-1012..M-1023, M-1040, M-1041, M-1055..M-1057, M-1060.
