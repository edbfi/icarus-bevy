# Setup and migration

## New projects and features

After checking the current stable release, `cargo add bevy` adds the normal engine
dependency. For the reproducible baseline here, use `cargo add bevy@0.20.0`.
That Cargo requirement permits compatible patches; retain `Cargo.lock` to reproduce
an application build. The verification script uses `=0.20.0` for an exact check.

Bevy 0.20.0 requires Rust 1.97.1 or newer. Check the project's toolchain before
interpreting compiler errors as engine bugs. On native Linux, full default builds
also need platform development packages; use the official
[Linux dependencies guide](https://github.com/bevyengine/bevy/blob/v0.20.0/docs/linux_dependencies.md)
for the distribution instead of disabling required functionality to hide a failure.

| Application | Bevy feature selection with `default-features = false` |
|---|---|
| Normal 2D game | `2d`; add `ui` and `audio` when needed |
| Normal 3D game | `3d`; add `ui` and `audio` when needed |
| UI application | `ui`; add `audio` if needed |
| Headless gameplay tests | `std`, `async_executor`, `bevy_state` as required |
| Custom renderer | Relevant `2d_api`, `3d_api`, `ui_api`; supply renderer/platform separately |

Default features combine `2d`, `3d`, `ui`, and `audio`. Disabling defaults matters:
Cargo features are additive, so listing `2d` alone otherwise removes nothing.
The `*_api` collections expose scene data but do not provide a complete renderer.
`default_app` provides common app plugins; `default_platform` also brings native
window/input support. Check [the exact feature definitions](https://github.com/bevyengine/bevy/blob/v0.20.0/Cargo.toml)
when composing a reduced build. `MinimalPlugins` does not include every service a
test might use; add `StatesPlugin`, `AssetPlugin`, etc. as needed.

Keep build tuning optional and measurable. `dynamic_linking` can improve native
iteration but adds deployment constraints. Avoid unconditional `cargo clean` and
keep development-only features out of shipping builds. A release build and the
actual feature/target combinations remain separate checks.

## Migration procedure

Read every intervening [official migration guide](https://bevy.org/learn/migration-guides/).
Upgrade Bevy and incompatible ecosystem crates together, then fix the earliest
errors before cascading trait failures. Run `cargo tree -d` to locate mixed Bevy
versions, followed by `cargo tree -i bevy_ecs@VERSION` for the dependency that pulls
one in. An isolated tool may legitimately use another version; types crossing the
application boundary cannot be interchanged.

These are destinations for old idioms, not a claim that every change happened in
the same release:

| Old idiom | Baseline 0.20.0 form |
|---|---|
| `add_system`, `add_startup_system` | `add_systems(Update, ...)`, `add_systems(Startup, ...)` |
| `Input<KeyCode>` | `ButtonInput<KeyCode>` |
| `delta_seconds`, `elapsed_seconds` | `delta_secs`, `elapsed_secs` |
| `Camera2dBundle`, `SpriteBundle`, `NodeBundle` | Component tuples and required components |
| `get_single`, `get_single_mut` | `single`, `single_mut`, handling the `Result` |
| Buffered `EventReader`, `EventWriter`, `add_event` | `MessageReader`, `MessageWriter`, `add_message` |
| Observer `Trigger<E>` | `On<E>`; observer events still exist |
| `Parent`, manual child-list bookkeeping | `ChildOf`, maintained `Children` relationships |
| `SceneRoot`, `DynamicScene` | `WorldAssetRoot`, `DynamicWorld` |
| `TextFont` handle and floating-point size fields | `FontSource` (handle `.into()`) and `FontSize::Px(...)` |
| Lifecycle `Replace`, `on_replace` | `Discard`, `on_discard` |
| `LoadContext::loader()` | `LoadContext::load_builder()` |
| Light `shadows_enabled` | `shadow_maps_enabled`; check contact-shadow support separately |
| `Camera.target` | Separate `RenderTarget` component |

For 0.18 → 0.19, also review resource entities, generic query trait changes,
world serialization versus BSN scenes, render systems replacing camera render
nodes, and changed animation target identifiers. `InputFocus::set` since **0.19.1**
takes both an entity and a `FocusCause`; even patch-mismatched examples can fail.

Sources: [0.18 → 0.19 guide](https://bevy.org/learn/migration-guides/0-18-to-0-19/),
[versioned API](https://docs.rs/bevy/0.20.0/bevy/),
[official examples](https://github.com/bevyengine/bevy/tree/v0.20.0/examples).

## 0.19 → 0.20 migration

Bevy **0.20.0** was published as a non-yanked stable release on **2026-10-08**;
its MSRV is **Rust 1.97.1**. The earlier `0.20.0-rc.2` had Rust 1.96.0 as its
minimum, so prerelease metadata is not sufficient for the final release.
Preserve projects on 0.19 unless an upgrade is requested. For an upgrade, read the
[0.19 → 0.20 guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/)
alongside the final release's docs and examples.

| Area | 0.20 destination or behavior |
|---|---|
| Lifecycle observers | `On<Add, T>` becomes `On<Add<T>>`; dynamically filtered observers use `On<Add<()>>` |
| Pointer events | `Pointer<Press>` becomes `PointerPress`; pointer data lives under `event.pointer` |
| UI buttons | Use `ui_widgets::Button`, `Hovered`, and `Pressed` instead of the deprecated UI `Button` and `Interaction`; actions use `Activate` |
| Custom shaders | Translate naga_oil directives to WESL and use `.wesl`; plain WGSL remains supported, GLSL does not |
| Transmission | Add `ScreenSpaceTransmission` to cameras that need it; `Camera3d` no longer requires it |
| Querying entity lists | `iter_many` families yield `Result` items; handle errors or use `.matched()` where supported to retain skip-unmatched behavior |
| Render/UI ordering | Some built-in sets now use weak ordering; add strict dependencies for channel, atomic, or other side effects not tracked by ECS access |
| BSN templates | Scene references use `@`; component expressions no longer need `template_value`; entity lists use `--` separators |

Also audit custom extraction (`ExtractComponent` now specifies a target app),
font-family variants, text input, and shader module paths when those features are
used. Sprite rendering now uses the mesh/material backend, but normal `Sprite`
components remain supported; explicitly set Z levels for overlapping sprites.
These are migration checkpoints, not a complete replacement for the official guide.
