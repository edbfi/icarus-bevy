# Animation, audio, and VFX

## Animation graphs and imported rigs

An `AnimationPlayer` plays graph node indices, not raw clip handles. Retain the
indices returned when building `AnimationGraph`, store the graph as an asset,
and attach its `AnimationGraphHandle` to the actual player. Do not guess that the
first clip will always occupy index 1. Imported glTF players commonly live on
descendants of the scene root.

This installs a graph on a loaded rig after its hierarchy is ready. It expects a
real `assets/models/actor.glb` containing scene 0 and animation 0, and a full 3D
runtime with glTF animation support. The verifier checks its Rust API surface.

<!-- verify: presentation -->
```rust
use bevy::{prelude::*, world_serialization::WorldInstanceReady};
use std::time::Duration;

#[derive(Component)]
struct Playback {
    graph: Handle<AnimationGraph>,
    clip: AnimationNodeIndex,
}

fn load_actor(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let (graph, clip) = AnimationGraph::from_clip(
        assets.load("models/actor.glb#Animation0")
    );
    commands.spawn((
        WorldAssetRoot(assets.load("models/actor.glb#Scene0")),
        Playback { graph: graphs.add(graph), clip },
    )).observe(start_actor);
}

fn start_actor(
    ready: On<WorldInstanceReady>,
    roots: Query<&Playback>,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
    mut commands: Commands,
) {
    let Ok(playback) = roots.get(ready.entity) else { return };
    for entity in children.iter_descendants(ready.entity) {
        if let Ok(mut player) = players.get_mut(entity) {
            let mut transitions = AnimationTransitions::new();
            transitions.play(&mut player, playback.clip, Duration::ZERO).repeat();
            commands.entity(entity).insert((
                AnimationGraphHandle(playback.graph.clone()), transitions,
            ));
        }
    }
}

fn main() {
    App::new().add_plugins(DefaultPlugins)
        .add_systems(Startup, load_actor).run();
}
```

The snippet configures animation; add a camera and lighting to view the model.
For changing states, call `AnimationTransitions::play` when the desired clip changes,
rather than restarting it every frame. Preserve a single authority for root motion
and physical pose. Procedural transform animation must be ordered relative to
animation evaluation and transform propagation.

Rig targets use `AnimationTargetId` and `AnimatedBy`. IDs derived under older Bevy
algorithms need regeneration on migration. Animation masks exclude the groups
whose bits are set; confirm this before applying an upper-body mask. Timeline
events use `AnimationEvent` and observers; their trigger exposes the target through
`on.trigger().target`, not an assumed entity-event accessor. For easing or small
procedural effects, use curves or direct component updates instead of constructing
a skeletal animation system unnecessarily.

## Audio lifecycle

With the `audio` feature, spawn an `AudioPlayer` holding a loaded audio handle and
`PlaybackSettings` selecting looping, once, despawn, or component removal. The
engine creates `AudioSink`/`SpatialAudioSink` after playback starts, so a sink query
can initially have no match. Change live volume, pause, mute, speed, or position
through the sink; initial `PlaybackSettings` are not a live mixer control.

Define ownership and replay explicitly. Removing a sink while its player remains
can start the source again; drained one-shot playback needs deliberate component
replacement or a fresh player. Keep music handles alive, wait for readiness before
latency-sensitive transitions, and implement crossfades with overlap rather than
abrupt despawn. Apply master/category gain to already-playing sinks when sliders
change.

Spatial playback uses a `SpatialListener` and appropriate transforms/scale. Built-in
spatial audio is stereo spatialization, not a complete acoustic simulation or HRTF
graph. If richer mixing requires a plugin such as Seedling, check its declared Bevy
dependency and platform support before replacing the audio backend. Browser sound
must handle user-gesture unlock and failed playback. Provide captions or equivalent
cues when sound conveys required information.

## Effects

Use sprite sheets, a small number of ECS-driven particles, or material parameters
for simple effects. Large GPU particle systems such as Hanabi introduce plugin,
shader, and backend requirements; verify the selected release. Compute-based
effects need a backend that supports compute (WebGPU rather than WebGL2 in the
browser). Initialize the particle attributes consumed by the effect and measure
overdraw, spawning rate, allocation, and GPU costs under representative load.

Gaussian splat rendering, capture encoders, inverse kinematics, and audio graphs
are separate integrations, not implied Bevy-core features. Do not install a suite
of plugins merely because this reference mentions them.

Sources: [animation](https://docs.rs/bevy/0.19.1/bevy/animation/index.html),
[animated mesh](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/animation/animated_mesh.rs),
[audio API](https://docs.rs/bevy/0.19.1/bevy/audio/index.html),
[Hanabi](https://github.com/djeedai/bevy_hanabi),
[Seedling](https://github.com/CorvusPrudens/bevy_seedling).
