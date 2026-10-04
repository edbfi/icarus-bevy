# UI, accessibility, and localization

Build layout with `Node` and its flex/grid properties, style with components such
as `BackgroundColor` and `BorderColor`, and use `Text` for UI text (`Text2d` for
world-space text). In 0.19, `TextFont.font` is `FontSource` and `font_size` is
`FontSize`, not a bare handle and float. Font loading is still asynchronous.
The default embedded font is limited; ship fonts covering the required scripts.

This example expects `assets/fonts/ui.ttf` supplied by the application. With normal
default plugins it shows a labeled button and updates its background on pointer
interaction. It intentionally leaves the actual application action to the caller.

<!-- verify: presentation -->
```rust
use bevy::{input_focus::{FocusCause, InputFocus}, prelude::*};

#[derive(Component)]
struct StartButton;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, style_button)
        .run();
}

fn setup(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn(Camera2d);
    commands.spawn((
        Node {
            width: percent(100), height: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        children![(
            StartButton,
            Button,
            Node {
                padding: UiRect::all(px(16)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.1, 0.2, 0.3)),
            children![(
                Text::new("Start"),
                TextFont {
                    font: assets.load("fonts/ui.ttf").into(),
                    font_size: FontSize::Px(24.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        )],
    ));
}

fn style_button(
    mut focus: ResMut<InputFocus>,
    mut buttons: Query<(Entity, &Interaction, &mut BackgroundColor),
        (With<StartButton>, Changed<Interaction>)>,
) {
    for (entity, interaction, mut background) in &mut buttons {
        let color = match interaction {
            Interaction::Pressed => {
                focus.set(entity, FocusCause::Pressed);
                Color::srgb(0.15, 0.4, 0.3)
            }
            Interaction::Hovered => Color::srgb(0.2, 0.3, 0.4),
            Interaction::None => Color::srgb(0.1, 0.2, 0.3),
        };
        *background = BackgroundColor(color);
    }
}
```

`Changed<Interaction>` also matches insertion. Treat pointer hover, keyboard
focus, and activation as separate states. Do not clear keyboard focus merely
because the pointer leaves. `InputFocus::set(entity, FocusCause)` records why focus
moved. Setting focus alone does not implement navigation or keyboard activation:
wire focus/navigation support and route Enter/Space/gamepad confirmation to the
same application action as clicking. Higher-level `bevy_ui_widgets` can provide
behavior; inspect its current plugins and examples before composing widgets.

Use marker components or stored entity references to update the intended text or
bar; do not rely on a fixed child index after the hierarchy becomes dynamic.
Update existing entities and text instead of rebuilding the UI every frame.
`children![]` is useful for static composition, `with_children`/relationship
commands for dynamic content. A parent with no meaningful size can collapse a
percentage-based child; inspect computed layout before changing unrelated styles.

For an accessible flow, provide labels/semantics, visible focus, logical navigation,
contrast, scalable text, remapping, reduced motion, and alternatives to audio-only
cues. `AccessibleLabel` is useful when a button has no adequate visible text.
Test with keyboard-only use and the actual target platform's assistive tools;
adding components is not evidence of complete screen-reader support.

Localization requires message IDs, plural/select rules, parameter formatting,
font fallback, and layout expansion. Preserve the project's localization crate
and check its Bevy compatibility before adding a Fluent integration. Do not
concatenate translated sentence fragments or encode game state in rendered text.
Test long strings, right-to-left layouts where required, and locale changes.

Sources: [UI](https://docs.rs/bevy/0.19.1/bevy/ui/index.html),
[text](https://docs.rs/bevy/0.19.1/bevy/text/index.html),
[input focus](https://docs.rs/bevy/0.19.1/bevy/input_focus/index.html),
[UI examples](https://github.com/bevyengine/bevy/tree/v0.19.1/examples/ui).
