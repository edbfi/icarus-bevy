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
