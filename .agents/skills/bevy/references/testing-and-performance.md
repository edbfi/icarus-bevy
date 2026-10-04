# Testing and performance

## Small apps with explicit dependencies

Use `App::new()` plus the required plugins/resources, call `app.update()`, and
assert world state. `DefaultPlugins` introduces devices, assets, and rendering
that ordinary gameplay tests do not need. `MinimalPlugins` is useful but does not
provide state or asset services automatically. Avoid `App::run()` in unit tests.

Control time with `TimeUpdateStrategy`. `FixedTimesteps(n)` provides fixed-sized
advancement; warm up the initial zero-delta update before counting. Use
`ManualDuration` to exercise frames with zero or multiple simulation ticks.

<!-- verify: core -->
```rust
use bevy::{prelude::*, time::{TimePlugin, TimeUpdateStrategy}};

#[derive(Resource, Default)]
struct Steps(u32);

fn step(mut steps: ResMut<Steps>) {
    steps.0 += 1;
}

#[test]
fn fixed_time_advances_predictably() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::FixedTimesteps(1))
        .init_resource::<Steps>()
        .add_systems(FixedUpdate, step);
    app.update();
    assert_eq!(app.world().resource::<Steps>().0, 0);
    for _ in 0..4 { app.update(); }
    assert_eq!(app.world().resource::<Steps>().0, 4);
}
```

Run systems at least once to catch parameter initialization conflicts. For
messages, inspect effects or use a test-owned `MessageCursor`; for observers,
trigger and inspect the result after the relevant command boundary. Assert entity
membership and values without depending on query iteration order.

## Async and assets

Move expensive pure work to `AsyncComputeTaskPool` and I/O to `IoTaskPool` when
appropriate. Return owned results to the ECS; do not borrow the live `World` into
a background task. Retain `Task<T>` handles while work matters: dropping a task
cancels it unless detached. Poll with the version's nonblocking helper (0.19:
`bevy::tasks::futures::check_ready`) or drain a channel without blocking a system.

Prefer an injected deterministic executor/result in tests. When using real tasks,
step toward an observable completion condition with a bounded deadline and useful
failure output. Test cancellation and results arriving after entity removal or a
newer request. Arbitrary sleeps cannot establish readiness.

## Validation levels

1. `cargo check --all-targets` with the project's features catches Rust API errors.
2. Focused `cargo test` exercises ECS initialization and behavior without a window.
3. Run the game flow to verify assets, shaders, input, focus, sound, and cleanup.
4. Build and exercise other supported profiles/targets where changed behavior
   depends on them. A native compile is not browser validation.

The bundled `scripts/verify-examples.py` checks this skill's marked Rust blocks.
See [verification scope](sources-and-verification.md) before interpreting its result.

## Diagnose before optimizing

Reproduce the workload and measure frame-time distributions, allocation/asset
growth, entity counts, draw calls, and GPU time where relevant. Distinguish CPU
systems, render extraction/preparation, GPU work, shader compilation, and asset
loading stalls. A fixed entity-count threshold is not a performance diagnosis.

Use `FrameTimeDiagnosticsPlugin`, `DiagnosticsStore`, tracing spans, and supported
GPU tools as appropriate. Optimizing only average FPS can hide visible spikes.
Narrow queries and read access before trying `par_iter_mut`; overhead can outweigh
parallelism on small workloads. Batch structural churn and reuse assets where
measurement supports it. `Changed<T>` still performs tick checks and is not free.

For visual regression, capture through `Screenshot`/`ScreenshotCaptured`, await
completion, and retain expected/actual/diff artifacts. Fix camera, assets, seed,
time, viewport, backend, and warm-up conditions. Document a tolerance for driver
variation and review baseline changes. Successful Rust compilation does not
compile every WGSL shader or exercise GPU pipeline specialization.

Sources: [time strategy](https://docs.rs/bevy/0.19.1/bevy/time/enum.TimeUpdateStrategy.html),
[diagnostics](https://docs.rs/bevy/0.19.1/bevy/diagnostic/index.html),
[screenshot example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/window/screenshot.rs).
