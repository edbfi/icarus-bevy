---
name: bevy
description: Build, debug, test, and migrate Rust games and applications using the Bevy engine (the bevy Cargo crate). Use for Bevy ECS, scheduling, messages and observers, input, assets, scenes, rendering, shaders, UI, animation, audio, physics integration, and native or browser builds.
license: AGPL-3.0
metadata:
  author: engels74
  version: "1.0.0"
  vendor: bevy
---

# Bevy

One entry point for the `cargo add bevy` engine. Read only the references relevant
to the task; they are part of this skill and need no sibling skill installations.

## Establish the version first

The verified baseline is **Bevy 0.19.1**, latest non-yanked stable release checked
on **2026-10-04**, with Rust **1.95.0** minimum. The available 0.20 release candidate
is not this baseline. This is dated evidence, not a permanent assertion of latest.

Read `Cargo.toml`, workspace dependencies, `Cargo.lock`, the Rust toolchain, enabled
features, and target before choosing APIs. Preserve an existing project's release
unless an upgrade is requested. For a new project or a request for “latest,” check
the [published version metadata](https://crates.io/api/v1/crates/bevy), select a stable
release, and confirm its MSRV and feature names with `cargo info bevy@VERSION`.

Use `https://docs.rs/bevy/VERSION/bevy/` and the matching release-tag examples at
`https://github.com/bevyengine/bevy/tree/vVERSION/examples`. `latest` and `main`
can move independently of a user's lockfile. For a git dependency, inspect its
resolved revision instead. If the network is unavailable, use the resolved local
crate sources and explicitly leave latest-release verification unconfirmed.

## Work from the required behavior

1. Identify the data, its owner, and when it changes. Use components for entity
   state, resources for singletons, assets for shared loaded data, and systems for
   scheduled work. Group a feature's registrations in a plugin when useful.
2. Pick the schedule and communication model deliberately. `FixedUpdate` is for
   fixed-step simulation; `Update` is for frame-driven work. Buffered `Message`s
   and triggered observer `Event`s have different lifetimes and ordering.
3. Consult a version-matched official example before using an unfamiliar API.
   Adapt the smallest useful pattern to the project's features and architecture.
4. Check the changed targets and test observable behavior. A successful compile
   cannot detect query-initialization panics, incorrect schedule order, missing
   assets, or rendering problems. Run the affected flow when the environment permits.
5. Report the tested Bevy version, features, target, and any runtime coverage gaps.

## Read by task

| Task | Reference |
|---|---|
| New project, Cargo features, build setup, upgrades, stale API names | [Setup and migration](references/setup-and-migration.md) |
| Components, resources, queries, borrowing, hierarchy, change detection | [ECS and queries](references/ecs-and-queries.md) |
| Plugins, schedules, deferred commands, states, clocks | [Scheduling and states](references/scheduling-and-states.md) |
| Buffered messages, observers, entity events, lifecycle hooks | [Messages and observers](references/messages-and-observers.md) |
| Keyboard/gamepad actions, fixed input, physics integration | [Input and physics](references/input-and-physics.md) |
| Loading, handles, custom loaders, glTF, scenes, durable saves | [Assets and persistence](references/assets-and-persistence.md) |
| 2D/3D, cameras, materials, WGSL, renderer extensions | [Rendering and cameras](references/rendering-and-cameras.md) |
| Layout, text, interaction, focus, accessibility, localization | [UI and accessibility](references/ui-and-accessibility.md) |
| Animation graphs, transitions, audio playback, effects | [Animation, audio, and VFX](references/animation-audio-and-vfx.md) |
| Headless tests, async work, diagnostics, performance, screenshots | [Testing and performance](references/testing-and-performance.md) |
| WASM, WebGPU/WebGL2, voxel pipelines, capture, engine ports | [Platforms and specialized workflows](references/platforms-and-specialized-workflows.md) |
| Source reconciliation, provenance, validation scope, rechecking this skill | [Sources and verification](references/sources-and-verification.md) |

## Cross-cutting traps

- A system tuple does not establish order. Access conflicts serialize systems but
  do not select which goes first. Add only the dependencies behavior requires.
- Conflicting parameters **inside one system** need disjoint query filters or
  `ParamSet`; ordering separate systems cannot repair that conflict.
- `Commands` are deferred. Do not immediately query a just-queued spawn from the
  same system and assume the component exists.
- In 0.19, resources are singleton components. Derive `Resource` alone and audit
  broad entity queries that now encounter resource entities.
- Use current components such as `Camera2d`, `Sprite`, `Mesh3d`, and `Node` instead
  of obsolete built-in bundles. Custom `Bundle` types still exist.
- Avoid silently adding engine plugins, replacing physics backends, upgrading
  dependencies, or installing tooling unrelated to the requested behavior.
- Third-party crate version numbers do not imply Bevy compatibility. Inspect their
  dependency requirements and features before recommending or adding them.
