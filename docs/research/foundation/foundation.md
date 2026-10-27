# Native foundation and reference baseline

Native qualification; original baseline: server build 25621760/client 25793767.
Save-class, C001 and control-store digests are separate.

Before restructure: 96ddb0b:docs/research/foundation/2026-10-13-foundation-receipts.md

## Question cluster

- What native lifecycle, persistence, inspection and fault handling is qualified?
- What tooling and original baseline support it, within which limits?

## Current answer

No ICARUS compatibility (F-487). Provisional 30 Hz: F-868;
persistence/recovery/stale/failed workers: F-869, F-771; incomplete-state
rejection: F-482. [Operations](../../runbooks/native-server.md).

Authored persistence/lifecycle: schemas 4–10 (F-919, F-922, F-923, F-927),
exclusions (F-928), opt-in schema-11 modifier timers/cache/authority,
near-budget reload, replacement/retry and atomicity (F-931).
[Procedures/limits](../../runbooks/authored-worlds.md).

Bool/I32 limits/exclusions: F-929, runbook. Schema-7/8/9 preflight/schema-11
modifier costs: F-926; full preflight per step even empty; full ticks, uniform
latency/peak-memory/speedup unqualified.

Faults/redaction: F-239, F-671, F-870, F-880, F-423. Load/restarts (F-871),
interrupted soaks/compressed 72-hour schedules (F-872, F-873), POSIX recovery
(F-511): no ICARUS performance/physical power-loss qualification; earlier
soaks do not qualify later persistence (U-035).

Bounded opaque inspection (`world_imported` false): F-880 to F-883;
schema-3 lossless sidecars: F-567; readers: F-886 to F-888; randomized
preservation/codecs/readers/bounded peers: F-604 to F-607; corrupt stores:
F-611.

Schema-4 bridge: F-938 ([installation/limits](../../runbooks/native-server.md)).
Saved-pose/physics: U-025; stock client unqualified.

Loopback admission/cleanup/pins: F-875; session limits: F-833; metrics:
F-932; concurrency/partial-bunch sweeps: F-877; no ICARUS client. Missing reruns:
M-898. The 48 h attempt hit its disk guard; no final summary; copied reload
passes (M-1055). Anonymous byte-matched server data.pak fetch:
F-828, F-829, F-831.

Toolchain/headless/typing/CI: F-879, F-484, F-497; client identity:
F-878/F-731 (U-034 resolved). Original fixity/export/entry-map/exchange:
F-265/F-266/F-268/F-253; unrun launcher: F-252; expired runtime windows: F-218.

Debug idle (F-939): archetype checks (M-1060), lookup maps (M-1063),
relationship guards (M-1064) and occupancy slots (M-1070), each in 3 matched
pairs of 32 samples/mode without peers; host 1,024 medians fell from about
40 to 25–32 ms, with overlapping ranges for lookup-only and occupancy work and
one candidate p95 above 33.333 ms. M-1057 gives one-run debug medians at
1/64/1024 actors.

Release idle (F-941, M-1075): the authored bridge's 1,024-actor median cost
over a matched no-bridge app fell from 874–970 to 470–484 us across 3 pairs
for one candidate combining exact archetype scans, single-entity reads, an
ID-indexed placement fast path and unchanged-position write skips; 1/64
ranges overlap. Every tick still validates every binding with unchanged error
precedence. Whole binaries are compared: fixed mode order, binary layout and
host activity limit attribution, and no single change is measured. No
reference/client/ICARUS qualification.

## Open

- Authentication/stock client (U-038); world import (U-045).
- Original bug reproduction (U-047); ICARUS performance/stability (U-049).
- Paced multi-day effects/physical power loss (U-035).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-972, M-1012..M-1023, M-1040, M-1041, M-1055..M-1057, M-1060, M-1063, M-1064, M-1069, M-1070, M-1075.
