# Input and physics

## Frame input and fixed simulation

`ButtonInput<KeyCode>` is appropriate for simple frame-driven controls. For
rebindable gameplay, map physical controls to game actions and separate menu,
gameplay, and text-entry contexts. Handle focus loss, device removal, deadzones,
and synthesized releases so an action cannot remain stuck.

Read raw input after `bevy::input::InputSystems` in `PreUpdate` when fixed
simulation needs it in the same frame. Carry held/absolute values forward and
queue edge transitions; accumulate relative motion once per input frame. The
first fixed tick drains pending edges and deltas. If no tick runs, retain them;
if several run, do not replay them. Do not multiply a mouse displacement by delta
time again; a velocity or rate does need a time factor.

This tests the buffering contract independently of physical device events:

<!-- verify: core -->
```rust
use bevy::prelude::*;
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq)]
enum JumpEdge { Press, Release }

#[derive(Resource, Default)]
struct PendingInput {
    edges: VecDeque<JumpEdge>,
    motion: Vec2,
    movement: Vec2,
}

#[derive(Resource, Default)]
struct TickInput {
    edges: Vec<JumpEdge>,
    motion: Vec2,
    movement: Vec2,
}

fn sample_tick(mut pending: ResMut<PendingInput>, mut tick: ResMut<TickInput>) {
    tick.edges.clear();
    tick.edges.extend(pending.edges.drain(..));
    tick.motion = std::mem::take(&mut pending.motion);
    tick.movement = pending.movement;
}

#[test]
fn input_survives_a_gap_and_is_consumed_once() {
    let mut app = App::new();
    app.init_resource::<PendingInput>()
        .init_resource::<TickInput>()
        .add_systems(FixedUpdate, sample_tick);
    {
        let mut pending = app.world_mut().resource_mut::<PendingInput>();
        pending.edges.extend([JumpEdge::Press, JumpEdge::Release]);
        pending.motion += Vec2::new(3.0, 1.0);
        pending.movement = Vec2::X;
    }
    // No fixed schedule ran: the pending press and release remain available.
    assert_eq!(app.world().resource::<PendingInput>().edges.len(), 2);
    app.world_mut().run_schedule(FixedUpdate);
    let tick = app.world().resource::<TickInput>();
    assert_eq!(tick.edges, [JumpEdge::Press, JumpEdge::Release]);
    assert_eq!(tick.motion, Vec2::new(3.0, 1.0));
    for _ in 0..3 {
        app.world_mut().run_schedule(FixedUpdate);
        let tick = app.world().resource::<TickInput>();
        assert!(tick.edges.is_empty());
        assert_eq!(tick.motion, Vec2::ZERO);
        assert_eq!(tick.movement, Vec2::X);
    }
}
```

This deliberately steps a single system's fixed schedule; it does not test the
main-loop clock or device ingestion. For ordered rapid taps, ingest keyboard or
other raw messages instead of reconstructing order from `just_pressed` booleans.
Separate device streams do not establish a cross-device total order. Replays and
rollback need tick-indexed commands, stable ordering, and controlled randomness;
fixed time alone is insufficient for determinism.

## Physics is an integration choice

Bevy core does not supply rigid-body physics. Preserve an existing Rapier or Avian
integration. Before adding either, inspect its current manifest and compatibility
table against the project's exact Bevy line; do not infer compatibility from
similar version numbers. Use the plugin's own docs for collision message types,
query APIs, shape units, and schedule sets.

- Decide which system owns position: dynamic bodies generally receive forces,
  impulses, or velocities. Writing their `Transform` each frame is a teleport.
- Place drive systems before the plugin's synchronization/step and readback users
  after its writeback, in the schedule where the plugin actually runs.
- Match the physics timestep and Bevy fixed clock if moving a plugin to
  `FixedUpdate`; do not merely move your gameplay systems and assume it follows.
- Treat interpolation as presentation. Query authoritative physics poses for
  simulation; do not feed interpolated display poses back into the solver.
- Test collision layers, event opt-in flags, tunneling/CCD, scaling, and kinematic
  motion. A kinematic target by itself does not guarantee obstacle avoidance.

Sources: [input](https://docs.rs/bevy/0.19.1/bevy/input/index.html),
[fixed timestep example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/movement/physics_in_fixed_timestep.rs),
[Rapier integration](https://github.com/dimforge/bevy_rapier),
[Avian](https://github.com/avianphysics/avian).
