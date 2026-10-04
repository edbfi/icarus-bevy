# Scheduling, states, and clocks

Use a plugin to own related registrations and expose system sets when other
features need ordering hooks. Avoid imposing a universal project directory tree
or turning a small prototype into dozens of one-system plugins.

`Startup` runs once. `PreUpdate` prepares frame data; fixed simulation runs before
`Update`; `PostUpdate` includes derived presentation work such as transform
propagation. A fixed loop may execute zero, one, or several times per frame.
`Res<Time>` follows the active schedule's clock; select `Time<Real>`,
`Time<Virtual>`, or `Time<Fixed>` explicitly when pause/scaling policy matters.
Bevy's fixed default is 64 Hz; configure a different rate deliberately.

`.chain()`, `.before()`, and `.after()` declare dependencies **within a schedule**.
Ordering two conflicting systems is different from repairing conflicting
parameters in one system. Chain a sequence that truly depends on previous
results; independent systems should retain scheduling freedom.

`Commands` queues structural changes. Normal ordered dependencies insert deferred
application when needed, so a chained consumer can see a producer's spawn. Do not
use `chain_ignore_deferred` when that visibility is required. Exclusive `&mut World`
systems allow direct structural access at the cost of parallelism.

This test proves the command visibility relied on by the chain:

<!-- verify: core -->
```rust
use bevy::prelude::*;

#[derive(Component)]
struct Ready;

#[derive(Resource, Default)]
struct Seen(usize);

fn enqueue(mut commands: Commands) {
    commands.spawn(Ready);
}

fn count(query: Query<(), With<Ready>>, mut seen: ResMut<Seen>) {
    seen.0 = query.iter().count();
}

#[test]
fn chained_consumer_sees_deferred_spawn() {
    let mut app = App::new();
    app.init_resource::<Seen>()
        .add_systems(Update, (enqueue, count).chain());
    app.update();
    assert_eq!(app.world().resource::<Seen>().0, 1);
}
```

For game modes, derive `States`, install `StatesPlugin` in a minimal app, and call
`init_state`. Set `NextState<T>` to request a transition; it is not an immediate
replacement of `State<T>`. Use `OnEnter`, `OnExit`, and `run_if(in_state(...))` for
lifecycle and execution gates. `DespawnOnExit(state)` expresses entity ownership.

<!-- verify: core -->
```rust
use bevy::{prelude::*, state::app::StatesPlugin};

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
enum Mode {
    #[default]
    Menu,
    Playing,
}

#[derive(Component)]
struct MenuRoot;

fn enter_menu(mut commands: Commands) {
    commands.spawn((MenuRoot, DespawnOnExit(Mode::Menu)));
}

#[test]
fn leaving_menu_cleans_up_owned_entities() {
    let mut app = App::new();
    app.add_plugins(StatesPlugin)
        .init_state::<Mode>()
        .add_systems(OnEnter(Mode::Menu), enter_menu);
    app.update();
    assert_eq!(app.world_mut().query::<&MenuRoot>().iter(app.world()).count(), 1);
    app.world_mut().resource_mut::<NextState<Mode>>().set(Mode::Playing);
    app.update();
    assert_eq!(app.world().resource::<State<Mode>>().get(), &Mode::Playing);
    assert_eq!(app.world_mut().query::<&MenuRoot>().iter(app.world()).count(), 0);
}
```

Do not confuse running a schedule with advancing an app's time, maintaining
messages, and applying state transitions. Headless tests should usually step
`App::update()`; see [testing](testing-and-performance.md).

Sources: [App](https://docs.rs/bevy/0.19.1/bevy/app/struct.App.html),
[schedule configuration](https://docs.rs/bevy/0.19.1/bevy/ecs/schedule/trait.IntoScheduleConfigs.html),
[state example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/state/states.rs).
