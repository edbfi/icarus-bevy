# Rendering, materials, and cameras

Start with the built-in renderer for a normal game. A physics plugin is independent
of renderer choice. `2d` and `3d` feature profiles include rendering and platform
support; `2d_api`/`3d_api` expose world-side types for a separately supplied renderer.

The following is an asset-free 3D starting point for a project with `bevy = "0.19.1"`.
The documentation verifier type-checks it; running it needs the actual renderer,
window backend, and GPU environment.

<!-- verify: presentation -->
```rust
use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.6, 0.8),
            perceptual_roughness: 0.7,
            ..default()
        })),
    ));
    commands.spawn((
        PointLight { intensity: 200_000.0, ..default() },
        Transform::from_xyz(3.0, 4.0, 5.0),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
```

For 2D, spawn `Camera2d` and a `Sprite` (for example `Sprite::from_image` or
`Sprite::from_color`); for 2D meshes use `Mesh2d` and `MeshMaterial2d`. Share mesh
and material handles where appropriate. Mutating a shared material changes every
user; allocate a separate asset when an object needs independent parameters.

## Cameras and visibility

Check camera activation, projection, clipping, render layers, target, transform,
and inherited visibility before rewriting shaders to fix an invisible entity.
Bevy uses right-handed 3D coordinates, Y up, and camera forward along local -Z.
Use camera conversion methods for cursor-to-world rays; handle their `Result` and
account for the target viewport and scale factor.

`RenderTarget` is a component in 0.19. Multiple cameras need explicit target,
order, layer, and clear policies. Attach UI to the intended camera when the default
selection is ambiguous. An orbit camera should keep yaw/pitch/distance state,
clamp pitch, and apply mouse deltas once per frame. Obstruction testing belongs to
physics/scene queries; smoothing should not alter the simulation's authoritative pose.

## Choose the smallest rendering extension

1. Change `StandardMaterial`, lights, and existing post-processing components.
2. Use `Material`, `Material2d`, or `MaterialExtension` for custom shading.
3. Add extraction and preparation of render-world data for specialized work.
4. Add a render system/pass only when material-level changes cannot express it.

Custom material types generally derive `Asset`, `TypePath`, `AsBindGroup`, and
`Clone`; register the matching material plugin. Keep Rust bind-group declarations
and WGSL bindings/layout aligned. Consult the versioned shader example for the
correct `ShaderRef` imports and group macros. A manual `AsBindGroup` implementation
in 0.19 requires `label`; the derive handles that detail.

Camera render nodes/`ViewNode` were replaced by systems in `Core2d`/`Core3d` in
0.19. Use current `ViewQuery`, `RenderContext`, and pass ordering rather than
copying an old graph-node tutorial. The top-level non-camera schedule named
`RenderGraph` still exists. Keep main-world and render-world entity identities
and asset preparation separate.

Forward rendering is the default; deferred rendering is an explicit opaque-path
tradeoff involving G-buffer bandwidth and MSAA constraints. Transparent content
still needs its appropriate path. Validate custom pipelines and features on the
minimum supported GPU and browser backend, not just with `cargo check`.

Sources: [camera API](https://docs.rs/bevy/0.19.1/bevy/camera/index.html),
[PBR](https://docs.rs/bevy/0.19.1/bevy/pbr/index.html),
[shader examples](https://github.com/bevyengine/bevy/tree/v0.19.1/examples/shader),
[render systems](https://github.com/bevyengine/bevy/tree/v0.19.1/examples/shader_advanced).
