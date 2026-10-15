# Synthetic durability workload

The synthetic soak runs a bounded, authored population through the real Bevy fixed
schedule and asynchronous snapshot writer. Use a fresh data directory. The example
mutates positions deterministically, records tick and save observations, then
reopens the store and compares the complete final snapshot.

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

Long runs need not take their wall time. `--compressed` treats the duration as
simulated time: each update runs exactly one fixed step with no pacing, saves
fall due every `--save-seconds` of simulated time and a save still in flight is
awaited (`save_waits`), so a run executes the duration's exact tick and save
counts as fast as the host allows. `--start-tick` starts the clock high to
exercise long-uptime counters. Warm-up, status and growth use simulated time.

```bash
cargo run --release --locked --example synthetic_soak -- --compressed \
  --data-dir work/soak-new --duration-seconds 259200 --entities 100000 --save-seconds 10
```

Compression covers operation counts, retained growth per operation and
counter ranges. It does not cover behavior paced by the wall clock (the
production loop's autosave timer, peer idle timeouts), real-time deadline
pressure between ticks, or storage, page-cache and host effects that accrue
over days; pair it with a short paced run for those.

This tests native scheduling and persistence under synthetic load. It contains no
ICARUS gameplay, networking or original content, and cannot establish stock-client
compatibility or a performance improvement over the official server. A 72-hour
synthetic run is not the [canonical plan's](../linux-dedicated-server-plan.md)
stability target, which needs heavy ICARUS worlds.

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

The recovery campaign exercises repeated process termination and
recovery with 10,000 authored entities. It alternates SIGKILL and SIGTERM around a
later autosave window after observing an initial committed save. Every cycle checks
the published checksum, restored clock, acknowledged generation and complete
entity state. It does not claim deterministic interception of each filesystem
operation, real disk exhaustion or physical power-loss qualification. Use a fresh
output directory and retain the helper/binary provenance and per-cycle logs.

```bash
python3 tools/native_recovery_campaign.py --binary target/release/icarus-server \
  --output work/recovery-new --cycles 24
```
