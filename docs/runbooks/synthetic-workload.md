# Synthetic durability workload

The synthetic soak runs authored entities through Bevy's fixed schedule and
asynchronous writer. In a fresh data directory, it mutates positions, measures
ticks and saves, then reopens the store and compares the complete snapshot.

```bash
cargo run --release --locked --example synthetic_soak -- \
  --data-dir work/soak-new --duration-seconds 7200 --entities 100000 --save-seconds 10
```

The workload emits JSON lines on stdout and accepts up to 259,200 seconds, the
plan's 72-hour stability run. Tick measurements cover the fixed simulation
systems; they exclude snapshot capture and the outer loop. Save latency is
submission-to-observation, including polling delay. Snapshot capture is measured
separately. Every sample goes into a fixed-memory histogram (ticks at 1 µs up to
131,072 µs; saves and captures at 100 µs up to 10 s), so memory does not grow
with duration. Quantiles use the sorted-sample rank `floor((n - 1) * percentile /
100)`, rounded down to the resolution, and are `null` when the rank falls past the
histogram range; maxima are exact. Periodic RSS samples describe the live
process. `--warmup-seconds` (default 600) selects the first status sample used as
the growth baseline; `rss_growth_after_warmup_percent` compares it with the RSS
just before recovery verification, whose allocations only the final `rss_kib`
includes. No tick samples are discarded. Record CPU affinity, build commit,
binary hash, host resources and concurrent workloads with results.
`rss_slope_kib_per_hour` fits a least-squares line to every status sample
after warm-up, a steadier leak signal than the two-point growth.

`--compressed` uses simulated time: one fixed step per unpaced update, saves
every `--save-seconds`, awaiting in-flight saves (`save_waits`). It executes
exact tick/save counts as fast as the host allows. Warm-up, status and growth
use simulated time. `--start-tick` exercises long-uptime counters.

```bash
cargo run --release --locked --example synthetic_soak -- --compressed \
  --data-dir work/soak-new --duration-seconds 259200 --entities 100000 --save-seconds 10
```

Compression covers operation counts, retained growth per operation and
counter ranges. It does not cover behavior paced by the wall clock (the
production loop's autosave timer, peer idle timeouts), real-time deadline
pressure between ticks, or storage, page-cache and host effects that accrue
over days; pair it with a short paced run for those.

These authored workloads establish neither stock-client compatibility nor an
official-server speedup. The [plan's](../linux-dedicated-server-plan.md) stability
target requires heavy ICARUS worlds, not 72 hours of synthetic scheduling.

For repeated synchronous persistence measurements, use:

```bash
cargo run --release --locked --example snapshot_bench -- \
  --data-dir work/bench-new --entities 100000 --iterations 20
```

The benchmark first performs two warm-up commits, then records each save and load
separately and compares all recovered state. Use multiple fresh-directory runs per
revision, the same CPU affinity and population, and retain the executable hashes.
`/usr/bin/time -v` can record the whole process's peak RSS; that includes verification
and must not be labeled steady-state server memory. Keep baseline binaries under
ignored `work/` when comparing revisions.

For schemas 7–9, `native_lifecycle_bench` measures capture, preflight, storage
and owner actions, verifying full state outside each timer. Create the case
directory first; its store child must be absent:

```bash
cargo run --release --locked --example native_lifecycle_bench -- \
  --data-dir work/CASE/new-store --operations preflight --slots 3,64,256,1024
```

Select operations separately to avoid starving later ones under the shared
20-second case limit. Defaults are three repetitions, five warm-ups and 50
samples; JSON retains raw times and marks incomplete cases. Enforce external
CPU, memory and hard-time limits. Action timings include internal capture and
preflight; load uses the same open store. Match fixtures, build settings and
affinity across revisions; exclude incomplete cases from comparisons.

The recovery campaign alternates SIGKILL/SIGTERM near autosaves after an initial
committed save, using 10,000 authored entities. Each cycle checks the checksum,
clock, acknowledged generation and all entities. Use a fresh directory; retain
helper/binary provenance and cycle logs. It does not qualify exact filesystem
interception, disk exhaustion or physical power loss.

```bash
python3 tools/native_recovery_campaign.py --binary target/release/icarus-server \
  --output work/recovery-new --cycles 24
```
