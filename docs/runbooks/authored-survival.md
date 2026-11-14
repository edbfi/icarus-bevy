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

Clock settings bind the authored-world
input; rates and actual frequency bind the declaration input
([exact contract](../architecture.md#persistence)). Enabling, disabling or changing
a bound rule refuses reopen; retain the recipe. No schema change, reset or
migration; release-only changes remain provenance.

## Activation and time

Only authenticated, activated attachments consume. Returning reservation is not
activation; pending, revoked and detached players incur nothing. Before debit,
a bounded drain of queued authentication outcomes detaches stale/ineligible
epochs: this is the eligibility cut. Later events apply at the next cut;
the cut neither activates nor publishes players.

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
