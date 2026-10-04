# Synthetic durability workload

`cargo run --release --locked --example synthetic_soak -- --data-dir
work/soak-new --duration-seconds 7200 --entities 100000 --save-seconds 10`
runs a bounded, authored population through the real Bevy fixed schedule and
asynchronous snapshot writer. Use a fresh data directory. The example mutates
positions deterministically, records tick and save observations, then reopens the
store and compares the complete final snapshot.

The workload emits JSON lines on stdout. Tick measurements cover the fixed
simulation systems; they exclude snapshot capture and the outer loop. Save latency
is submission-to-observation, including polling delay. Snapshot capture is measured
separately. Quantiles use the sorted sample at `floor((n - 1) * percentile / 100)`.
Periodic RSS samples describe the live process; final RSS also includes recovery
verification allocations. No warm-up samples are discarded. Record CPU affinity,
build commit, binary hash, host resources and concurrent workloads with results.

This tests native scheduling and persistence under synthetic load. It contains no
ICARUS gameplay, networking or original content, and cannot establish stock-client
compatibility or a performance improvement over the official server. The two-hour
limit is also shorter than the release plan's sustained qualification runs.

For repeated synchronous persistence measurements, use
`cargo run --release --locked --example snapshot_bench -- --data-dir
work/bench-new --entities 100000 --iterations 20`. It first performs two warm-up
commits, then records each save and load separately and compares all recovered
state. Use multiple fresh-directory runs per revision, the same CPU affinity and
population, and retain the executable hashes. `/usr/bin/time -v` can record the
whole process's peak RSS; that includes verification and must not be labeled
steady-state server memory. Keep baseline binaries under ignored `work/` when
comparing revisions.

`python3 tools/native_recovery_campaign.py --binary target/release/icarus-server
--output work/recovery-new --cycles 24` exercises repeated process termination and
recovery with 10,000 authored entities. It alternates SIGKILL and SIGTERM around a
later autosave window after observing an initial committed save. Every cycle checks
the published checksum, restored clock, acknowledged generation and complete
entity state. It does not claim deterministic interception of each filesystem
operation, real disk exhaustion or physical power-loss qualification. Use a fresh
output directory and retain the helper/binary provenance and per-cycle logs.
