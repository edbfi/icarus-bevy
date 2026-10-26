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

Authored schemas 4–10: F-919, F-922, F-923, F-927; integrity/callback exclusions:
F-928. Schema 11 opt-in modifier timers/cache and owner authority survive
reload (including near-budget stores), replacement and retry; failed steps
are atomic (F-931).
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

Debug idle costs (F-939): M-1060 fresh archetype absence checks, 3 matched pairs,
32 samples/mode, no peers; host 1,024 medians 38.74–40.25 to 29.08–32.08 ms,
candidate p95 35.14–36.30 ms; 1/64 ranges overlap. M-1057 one-run medians
at 1/64/1024: host 1.17/3.94/41.59 ms; lab 0.50/0.51/1.03 ms.

M-1063 lookup-map-only: 6 runs/3 pairs/32 raw samples per row; median
ranges overlap at all counts/both modes. Host 1,024 parent 28.67–31.76 versus
candidate 30.12–30.55 ms; no demonstrated latency gain; allocations/RSS unmeasured.

Relationship-absence guards (M-1064): debug whole-App idle,
no peers; P,C,C,P,P,C, three pairs/32 samples per mode. Host 1,024 parent/candidate
median ranges 28.158393–34.245773/25.264452–28.268425 ms; p95
35.918821–38.149755/32.095349–34.547397 ms. All three paired medians/p95 lower;
median ranges overlap, p95 ranges do not; one candidate p95 >33.333 ms.
Attribution limited by fixed inner baseline/host order, extra untimed tests,
binary layout and uncontrolled host activity. Both resource checks and all four
component checks retain error precedence; no skip/cache acceptance.
No uniform/release/reference/client/ICARUS performance qualification.

## Open

- Authentication/stock client (U-038); world import (U-045).
- Original bug reproduction (U-047); ICARUS performance/stability (U-049).
- Paced multi-day effects/physical power loss (U-035).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-972, M-1012..M-1023, M-1040, M-1041, M-1055..M-1057, M-1060, M-1063, M-1064.
