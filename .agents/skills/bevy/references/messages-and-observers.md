# Messages and observers

| Need | Mechanism |
|---|---|
| Read buffered notifications in scheduled systems | `#[derive(Message)]`, `add_message`, `MessageWriter::write`, `MessageReader::read` |
| React to an explicit trigger | `#[derive(Event)]`, `On<E>`, `add_observer`, `trigger` |
| Target an entity, optionally propagate through relationships | `#[derive(EntityEvent)]`, entity observers, configured propagation |
| Enforce component lifecycle invariants | Component hooks; use lifecycle observers for composable reactions |

Messages have a cursor per reader; one reader does not consume another's messages.
They have bounded retention governed by message maintenance, normally two update
cycles. A reader gated off by a state or run condition can miss messages. If work
must survive a pause or await acknowledgement, use an owned queue or explicit
durable state instead. Order a producer before its consumer when same-frame
delivery matters.

Observers run in response to triggers; multiple observers have no implicit logical
order. `world.trigger` executes directly, whereas `commands.trigger` waits until
the command queue is applied. Commands created by an observer still follow command
application rules. Do not use an observer cascade as an implicit ordered workflow.

This checks both communication paths in the same app:

<!-- verify: core -->
```rust
use bevy::prelude::*;

#[derive(Message)]
struct Award(u32);

#[derive(Event)]
struct Reset;

#[derive(Resource, Default)]
struct Points(u32);

fn award(mut writer: MessageWriter<Award>) {
    writer.write(Award(7));
}

fn collect(mut reader: MessageReader<Award>, mut points: ResMut<Points>) {
    for award in reader.read() {
        points.0 += award.0;
    }
}

fn reset(_: On<Reset>, mut points: ResMut<Points>) {
    points.0 = 0;
}

#[test]
fn messages_and_observers_have_distinct_entry_points() {
    let mut app = App::new();
    app.init_resource::<Points>()
        .add_message::<Award>()
        .add_observer(reset)
        .add_systems(Update, (award, collect).chain());
    app.update();
    assert_eq!(app.world().resource::<Points>().0, 7);
    app.world_mut().trigger(Reset);
    assert_eq!(app.world().resource::<Points>().0, 0);
}
```

An event type's name does not determine the trait it implements. Some engine and
plugin types still end in `Event` but implement `Message`. Inspect the definition
before selecting a reader or observer. `AnimationEvent` has its own trigger data.

For lifecycle code, distinguish `Add`, `Insert`, `Discard`, `Remove`, and `Despawn`.
In 0.19 the former replacement lifecycle is named `Discard`/`on_discard`. Hooks
receive a `DeferredWorld`; they are not ordinary systems with arbitrary parameters.
Read the current lifecycle ordering before maintaining a reverse index or cleaning
up external state.

Sources: [messages example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/ecs/message.rs),
[observers](https://docs.rs/bevy/0.19.1/bevy/ecs/observer/index.html),
[lifecycle](https://docs.rs/bevy/0.19.1/bevy/ecs/lifecycle/index.html).
