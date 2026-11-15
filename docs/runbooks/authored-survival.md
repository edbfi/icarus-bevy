# Authored survival

Authored-test contract (F-1183); library and focused progression tests pass.
Rules and daylight are independently authored; stock-client behavior is unqualified.
Requires schema-14 [player setup](../architecture.md#persistence).
Both default absent; enable either or both.

## Configuration and reopening

```toml
[network.player_consumption]
oxygen = 1
water = 1
food = 1

[network.authored_world.clock]
initial_minute = 480
seconds_per_day = 3600
tick_hz = 30
```

Rates are integer vital units per simulated second, each 0..=10000000, with at
least one positive. Only current oxygen/water/food decrease, clamped at zero;
health, stamina, maxima, placement, biome and growth stay unchanged.
The clock takes minute 0..=1439 and positive i32 seconds/day. Its `tick_hz`
must equal server `tick_hz` (1..=240).

Clock settings bind the authored-world input; rates/frequency bind the declaration
([contract](../architecture.md#persistence)). Changing, enabling or disabling a
bound rule refuses reopen; retain the recipe. No schema change, reset or migration;
release-only changes remain provenance.

## Activation and time

Only authenticated, activated attachments consume. Pending reservations, revoked
and detached players incur nothing. Before debit, a bounded authentication-outcome
drain detaches stale/ineligible epochs: the eligibility cut. Later events wait for
the next cut; it neither activates nor publishes players.

For completed tick `t`, rate `r` debits
`floor((t+1)*r/hz) - floor(t*r/hz)`. The batch is atomic after Gameplay/before
Finish; projections follow successful Finish. Polling, pressure retries and NAKs
never repeat debit. Unchanged currents use no revision.

Saved `SimulationClock.tick` alone owns phase. Day sampling and fractional debit
resume from the selected generation, without wall-time catch-up or a second
saved accumulator. Offline ticks advance day phase; downtime charges nothing.
The restored sample precedes Join/open/update. Sampled authored and legacy
monotonic clocks are mutually exclusive.

## Failed health

Owner/Fixed-timestep checks precede Bevy time/polling and every debit, including
offline consumption without a day clock. Failure latches: stop admission, refuse
Finish/publication/capture and fail the runner. Resource reinsertion cannot
clear failure; preserve the store and diagnose before restarting.

Authentication backend loss closes peers and detaches their epochs before debit;
committed player state remains capturable for shutdown.

## Checkpoint leave

F-1188: `request_leave` freezes active key/epoch/revision and authored location/vitals,
blocking consumption, edits and reattachment. The 64-operation cap includes
detached/failed/durable barriers; attempts last ≤60 monotonic seconds.
Capture/storage failure or timeout needs explicit `retry_leave`, rejecting stale
attempt receipts; worker loss is terminal. Timeout never cancels or rolls back I/O.
Poll `leave_status`; `settle_leave` requires durability and original detachment.

API `PeerLab::enable_authored_controller_leave(plan)` requires checkpoints and a
matching controller identity/layout/checksum before admission. Explicit
`network.authored_controller_leave` enables it after qualified preflight; default
false. Codec declarations alone never enable dispatch. Types: Quit (1), CharacterSelect
(2), Titlescreen (4). Missing/None, dropship, disconnected and no-save do nothing.

The plugin cuts off before fixed ticks, including zero-tick/consuming updates.
Missing projection or refused cutoff silently drops the intent: no retry,
completion or registry change. A fresh normal request is required, avoiding
an automatic delayed freeze of a later revision.
Only the exact writer receipt permits one reliable completion property update;
pressure/loss retains that operation. ACK of the reliable prefix through completion
permits detach; later traffic is excluded. Address/connection/epoch checks prevent
redirected completion after revocation/reuse. Detached durable barriers settle
automatically; pending/failed ones remain.

Cancel before reliable admission suppresses completion but retains the frozen
save obligation; after admission it cannot retract traffic. Duplicate normals
preserve the first type/cutoff/deadline. Failed or suppressed leaves stay connected
and frozen until disconnect, revocation or transport expiry: no automatic retry,
failure-close or gameplay resume. A client continuing ACKs can remain frozen.
Authored policy and transport ACK grant no full player-save or client menu proof.
