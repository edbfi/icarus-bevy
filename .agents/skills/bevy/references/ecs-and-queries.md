# ECS and queries

Choose component boundaries by access patterns and independent ownership. Small
components improve composition, but splitting every scalar is not a universal
optimization. Ordinary helper methods on data types are fine. Add `Reflect` only
when reflection, tooling, or serialization needs it.

In 0.19, `Resource: Component` and resources occupy singleton entities. Deriving
both traits conflicts. Keep singleton and ordinary per-entity roles in distinct
types; inserting a resource component on another entity can move its ownership.
Broad queries can now see resource entities and conflict with `ResMut<T>`. Use
`Without<IsResource>` (import `bevy::ecs::resource::IsResource`) when a query really
means ordinary entities, rather than blindly adding that filter everywhere.

Use `With<T>`/`Without<T>` for presence constraints, `Option<&T>` for optional
data, `Or<...>` for alternatives, and narrow read/write access to what the system
uses. Two mutable queries for different marker types can still overlap: an entity
can have both markers. Prove disjointness with a complementary exclusion or use
`ParamSet` and finish each borrow before taking the next.

The following executes both a required-component spawn and disjoint queries.

<!-- verify: core -->
```rust
use bevy::prelude::*;

#[derive(Component, Default)]
struct HitPoints(u32);

#[derive(Component)]
#[require(HitPoints)]
struct Hero;

fn resolve_round(
    mut heroes: Query<&mut HitPoints, With<Hero>>,
    mut others: Query<&mut HitPoints, Without<Hero>>,
) {
    for mut hp in &mut heroes {
        hp.0 += 2;
    }
    for mut hp in &mut others {
        hp.0 = hp.0.saturating_sub(1);
    }
}

#[test]
fn required_components_and_disjoint_access_work() {
    let mut app = App::new();
    let hero = app.world_mut().spawn(Hero).id();
    let other = app.world_mut().spawn(HitPoints(4)).id();
    app.add_systems(Update, resolve_round);
    app.update();
    assert_eq!(app.world().get::<HitPoints>(hero).unwrap().0, 2);
    assert_eq!(app.world().get::<HitPoints>(other).unwrap().0, 3);
}
```

Required components are inserted when absent; explicitly supplied values win.
They are an insertion aid, not continuous validation after later removal. Use a
spawn function or custom `Bundle` for reusable aggregates when that is clearer.

`Changed<T>` includes insertion and mutable dereference, even if the resulting
value is equal. It scans matching entities' change ticks; it is not an indexed
queue of changed entities. Avoid unnecessary writes, use `set_if_neq` when
appropriate, and use `Ref<T>` when comparing added versus changed status.
`RemovedComponents<T>` reports removals; it cannot supply the old component value.

Use `query.get(entity)` for an optional target and `single()` when cardinality
errors deserve explicit handling. `Single<...>` has system-parameter validation
semantics; choose it only when those semantics fit the system. Missing a selected
entity after despawn is normally a recoverable case.

`ChildOf` is the source of truth for the standard hierarchy; let relationship
hooks maintain `Children`. Standard parent despawning also removes linked children.
Store local poses in `Transform`; `GlobalTransform` is propagated later. Order a
consumer after `TransformSystems::Propagate` in `PostUpdate` when it needs the
current frame's world pose.

For generic query helpers, check 0.19's `IterQueryData` versus
`SingleEntityQueryData` bounds. Reach for query lenses, custom `QueryData`, sparse
storage, or parallel iteration only when access or measured workload calls for it.

Sources: [queries](https://docs.rs/bevy/0.19.1/bevy/ecs/system/struct.Query.html),
[resources](https://docs.rs/bevy/0.19.1/bevy/ecs/resource/index.html),
[hierarchy example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/ecs/hierarchy.rs).
