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

Independent schema-12 inventories: F-1029/F-1030; source-free authority, CLI and
transaction/reload checks; no restoration or replication.

Authored persistence/lifecycle: schemas 4–12 (F-919, F-922, F-923, F-927),
exclusions (F-928), schema-11 timers/cache/authority (F-931).
[Procedures/limits](../../runbooks/authored-worlds.md).

Bool/I32 limits/exclusions: F-929, runbook. Schema-7/8/9 preflight/schema-11
modifier costs: F-926; owned advance avoids restaging, full preflight remains.
Armed 1,024-owner median-of-run p95: 32.558 to 23.136 ms; three paired
ratios 0.678–0.876 (M-1259). Other cells mixed.
No full-frame, throughput, uniform or RSS proof.

Faults/redaction: F-239, F-671, F-870, F-880, F-423. Load/restarts (F-871),
interrupted soaks/compressed 72-hour schedules (F-872, F-873), POSIX recovery
(F-511): no ICARUS performance/physical power-loss qualification; earlier
soaks do not qualify later persistence (U-035).

Bounded opaque inspection (`world_imported` false): F-880 to F-883;
schema-3 lossless sidecars: F-567; readers: F-886 to F-888; randomized
preservation/codecs/readers/bounded peers: F-604 to F-607; corrupt stores:
F-611.

Spatial bridge/lifecycle: F-938 ([configuration](../../runbooks/native-server.md)).
`frame.orientation: right_handed_zyx_degrees_v1` temporarily
normalizes a validated `[x,y,z,w]` without changing native state, applies
signed-axis `A R A^T`, then right-handed active
`Rz(yaw) Ry(pitch) Rx(roll)`; output is `[pitch,yaw,roll]` degrees.
At `|cos(pitch)| <= 1e-10`, roll is zero and yaw `atan2(-M01,M11)`.
Signed zero is canonicalized; units affect translation only.
This is an authored convention; qualification is pending. No official rotation or saved-pose mapping
(U-025), physics or stock-client qualification.

Loopback admission/cleanup/pins: F-875; holds/limits: F-833; metrics: F-932;
concurrency/partial-bunch sweeps: F-877. Missing reruns: M-898.
Anonymous byte-matched server data.pak fetch: F-828, F-829, F-831;
authored gameplay input: F-993.

Toolchain/headless/CI: F-879, F-497; client identity: F-878/F-731 (U-034
resolved). Original fixity/entry-map/exchange: F-265/F-268/F-253; unrun
launcher: F-252; expired runtime windows: F-218.

Authored bridge debug/release idle and joined-peer costs, overlaps and limits:
F-952 (debug medians and frame-exceeding p95: M-1060). Frozen-frame decisions
replay in M-1088. Binary layout, fixed mode order and host activity limit
attribution.

Authored inventory last-key validation costs: F-1053.

## Open

- Authentication/stock client (U-038); world import (U-045).
- Original bug reproduction (U-047); ICARUS performance/stability (U-049).
- Paced multi-day effects/physical power loss (U-035).

## Artifacts

M-1274 to M-1278; M-1130, M-1131; M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-972, M-1012..M-1023, M-1040, M-1041, M-1055..M-1057, M-1060, M-1063, M-1064, M-1069, M-1070, M-1075, M-1076, M-1088, M-1256 to M-1260.
