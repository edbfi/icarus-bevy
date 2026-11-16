# Synthetic durability workload

`synthetic_soak` mutates authored entity positions through Bevy's fixed schedule
and asynchronous writer, then reopens the fresh store to compare the full snapshot.

```bash
cargo run --release --locked --example synthetic_soak -- \
  --data-dir work/soak-new --duration-seconds 7200 --entities 100000 --save-seconds 10
```

JSON output covers up to 259,200 seconds (72 hours). Tick timings exclude
snapshot capture and the outer loop. Save latency spans submission to observation,
including polling delay. Fixed-memory histograms count every sample: ticks at 1 µs through 131,072 µs,
saves/captures at 100 µs through 10 s. Quantiles use rank
`floor((n - 1) * percentile / 100)`, rounded down to resolution; out-of-range
ranks yield `null`, while maxima are exact.
`--warmup-seconds` (default 600) selects the first RSS baseline sample.
`rss_growth_after_warmup_percent` ends before recovery verification; only final
`rss_kib` includes its allocations. `rss_slope_kib_per_hour` fits all post-warm-up
status samples by least squares. Record affinity, commit, binary hash, host
resources and concurrent workloads.

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

These authored workloads prove neither compatibility nor official-server speedup.
The [plan](../linux-dedicated-server-plan.md) requires heavy ICARUS worlds.

For repeated synchronous persistence measurements, use:

```bash
cargo run --release --locked --example snapshot_bench -- \
  --data-dir work/bench-new --entities 100000 --iterations 20
```

The benchmark makes two warm-up commits, times each save/load and checks all
recovered state. Repeat fresh runs at matched population, affinity and build
settings; retain baseline binaries and hashes under ignored `work/`.
`/usr/bin/time -v` includes verification allocations: its peak RSS must not be
labeled steady-state server memory.

For schemas 7–9, `native_lifecycle_bench` measures capture, preflight, storage
and owner actions, verifying full state outside each timer. Create the case
directory first; its store child must be absent:

```bash
cargo run --release --locked --example native_lifecycle_bench -- \
  --data-dir work/CASE/new-store --operations preflight --slots 3,64,256,1024
```

Run operations separately to avoid starving later ones within the shared
20-second limit. Defaults: three repetitions, five warm-ups, 50 samples. JSON
retains raw times and incomplete markers. Enforce external CPU, memory and
hard-time limits. Action timings include capture and preflight; load reuses the
open store. Match fixtures, builds and affinity; exclude incomplete cases from
comparisons.

Recovery alternates SIGKILL/SIGTERM around autosaves after one commit.
Default mode checks 10,000 authored entities, checksum, clock and acknowledgements.
Use fresh output; retain provenance/logs. No mode qualifies
filesystem fault points, disk exhaustion or physical power loss.

```bash
python3 tools/native_recovery_campaign.py --binary target/release/icarus-server \
  --output work/recovery-new --cycles 24
```

Build and run the schema14 fixture:

```bash
python3 tools/cargo_guard.py build --locked --example authored_player_recovery
python3 tools/native_recovery_campaign.py --mode authored-player \
  --binary target/debug/examples/authored_player_recovery \
  --output work/CASE/new-campaign --cycles 4
```

The 7 Hz schema14 fixture covers active/detached players (F-1183). Fresh Rust
verification checks typed state and eight resumed steps; no downtime debit.
Identity/collection/exit failures, 20s child deadlines or the 30 GiB disk floor
deny success; retain all receipts. No production-loop/client qualification.

Use `--mode authored-player-inventory` for schema15 (1–4 cycles). Its independent
oracle checks sparse bags, conservation, revisions and allocation; graceful stop
checks the exact writer's leave cutoff. Bounds: 240s workload, 64 MiB output,
3 GiB available-RAM start floor. Apply external CPU/memory limits.
F-1200/M-1516 records four authored recovery cycles; no production/client or
physical-power-loss qualification.
