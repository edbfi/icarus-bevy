# Implementation status

This is an independent server foundation, not yet an ICARUS-compatible server.
No gameplay, protocol, authentication, world-data parity, or official-save import
has been demonstrated. Reliability and performance targets remain unproven.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | Offline inventory approved; case in ignored `work/icarus-server-baseline/` |
| M1: original bug reproduction | Pending | Requires separately approved dynamic lab execution |
| M2: interoperability/data feasibility | Pending | Requires protocol, platform and world-data evidence |
| M3: Bevy gameplay slice | In progress, foundation only | Native scaffold under development; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending | Official save format and failure scenarios unknown |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

The 30 Hz development rate is provisional, not a discovered ICARUS property.
Native snapshot version 1 stores authored foundation records; it is not the ICARUS
save format. No official server bugs are marked reproduced or fixed.

See the [full plan](linux-dedicated-server-plan.md) and
[approved offline batch](research-authorization.md).
