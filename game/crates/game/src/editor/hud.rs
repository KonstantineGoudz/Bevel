//! Character status window and respawn-on-death.
//!
//! The window is plain Bevy UI spawned once in `setup`; `update_character_status`
//! keeps its health bar and velocity readout in sync with `Editor.character`.
//! When health hits zero, `respawn_on_death` moves the character to the first
//! spawn marker (if any) with full health.

use super::*;

#[derive(Component)]
pub(super) struct CharacterStatus;

#[derive(Component)]
pub(super) struct HealthFill;

#[derive(Component)]
pub(super) struct HealthText;

#[derive(Component)]
pub(super) struct VelocityText;

#[derive(Component)]
pub(super) struct VelocityAxes;

const STATUS_WIDTH: f32 = 220.0;
const STATUS_MARGIN: f32 = 10.0;
const HEALTH_BAR_HEIGHT: f32 = 8.0;
const STATUS_VALUE: Color = Color::srgb_u8(0xc3, 0xd0, 0xdf);

/// Width of the health fill as a percent of the bar, for tests and the updater.
pub(super) fn fill_percent(fraction: f32) -> f32 {
    (fraction.clamp(0.0, 1.0) * 100.0).clamp(0.0, 100.0)
}

fn fill_color(fraction: f32) -> Color {
    let t = fraction.clamp(0.0, 1.0);
    // Green -> amber -> red as health drops.
    if t > 0.5 {
        let k = (t - 0.5) * 2.0;
        Color::srgb_u8(
            (0xe0 as f32 * (1.0 - k) + 0x3f as f32 * k) as u8,
            (0x5a as f32 * (1.0 - k) + 0xd0 as f32 * k) as u8,
            0x52,
        )
    } else {
        let k = t * 2.0;
        Color::srgb_u8(
            0xe0,
            (0x5a as f32 * k + 0xa6 as f32 * (1.0 - k)) as u8,
            0x4a,
        )
    }
}

/// Health readout for the status window.
pub(super) fn format_health(health: Health) -> String {
    format!(
        "{} / {}",
        health.current.ceil() as i32,
        health.max.ceil() as i32
    )
}

/// Speed readout for the status window, in metres per second.
pub(super) fn format_speed(speed: f32) -> String {
    if speed.is_finite() {
        format!("{speed:.2} m/s")
    } else {
        "-".to_string()
    }
}

/// Velocity components for the status window: `x`/`z` horizontal, `y` vertical.
pub(super) fn format_velocity(velocity: Vec3) -> String {
    if velocity.is_finite() {
        format!(
            "x {:.2}   y {:.2}   z {:.2}",
            velocity.x, velocity.y, velocity.z
        )
    } else {
        "x -   y -   z -".to_string()
    }
}

pub(super) fn spawn_character_status(commands: &mut Commands) {
    let root = commands
        .spawn((
            CharacterStatus,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                // Clear the desktop hint bar in Build mode; in Play it just
                // leaves a small gap above the window.
                top: px(34),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .id();

    let window = commands
        .spawn((
            Pickable::IGNORE,
            Node {
                width: px(STATUS_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(STATUS_MARGIN)),
                row_gap: px(4),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BAR_BG),
            BorderColor::all(BORDER),
        ))
        .id();

    let title = commands
        .spawn((
            Pickable::IGNORE,
            Text::new("CHARACTER"),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                margin: UiRect::bottom(px(2)),
                ..default()
            },
        ))
        .id();
    commands.entity(window).add_child(title);

    let health_row = status_row(commands, "HEALTH", HealthText, STATUS_VALUE);
    commands.entity(window).add_child(health_row);

    let track = commands
        .spawn((
            Pickable::IGNORE,
            Node {
                width: percent(100),
                height: px(HEALTH_BAR_HEIGHT),
                border_radius: BorderRadius::all(px(4)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.08)),
        ))
        .id();
    let fill = commands
        .spawn((
            HealthFill,
            Pickable::IGNORE,
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            BackgroundColor(Color::srgb_u8(0x3f, 0xd0, 0x70)),
        ))
        .id();
    commands.entity(track).add_child(fill);
    commands.entity(window).add_child(track);

    let velocity_row = status_row(commands, "VELOCITY", VelocityText, STATUS_VALUE);
    commands.entity(window).add_child(velocity_row);

    let axes = commands
        .spawn((
            VelocityAxes,
            Pickable::IGNORE,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    commands.entity(window).add_child(axes);

    commands.entity(root).add_child(window);
}

/// A label/value row inside the status window. `marker` tags the value text so
/// the updater can find it.
fn status_row(
    commands: &mut Commands,
    label: &str,
    marker: impl Component,
    value_color: Color,
) -> Entity {
    let row = commands
        .spawn((
            Pickable::IGNORE,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .id();
    let name = commands
        .spawn((
            Pickable::IGNORE,
            Text::new(label),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let mut value = commands.spawn((
        Pickable::IGNORE,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(value_color),
    ));
    value.insert(marker);
    let value = value.id();
    commands.entity(row).add_children(&[name, value]);
    row
}

pub(super) fn update_character_status(
    editor: Res<Editor>,
    mut roots: Query<&mut Node, With<CharacterStatus>>,
    mut fills: Query<
        (&mut Node, &mut BackgroundColor),
        (With<HealthFill>, Without<CharacterStatus>),
    >,
    mut health_labels: Query<
        &mut Text,
        (
            With<HealthText>,
            Without<VelocityText>,
            Without<VelocityAxes>,
        ),
    >,
    mut velocity_labels: Query<
        &mut Text,
        (
            With<VelocityText>,
            Without<HealthText>,
            Without<VelocityAxes>,
        ),
    >,
    mut axes_labels: Query<
        &mut Text,
        (
            With<VelocityAxes>,
            Without<HealthText>,
            Without<VelocityText>,
        ),
    >,
) {
    let Some(character) = editor.character else {
        for mut node in &mut roots {
            node.display = Display::None;
        }
        return;
    };
    for mut node in &mut roots {
        node.display = Display::Flex;
    }
    let fraction = character.health.fraction();
    for (mut node, mut bg) in &mut fills {
        node.width = percent(fill_percent(fraction));
        bg.0 = fill_color(fraction);
    }
    for mut text in &mut health_labels {
        text.0 = format_health(character.health);
    }
    for mut text in &mut velocity_labels {
        text.0 = format_speed(character.speed());
    }
    for mut text in &mut axes_labels {
        text.0 = format_velocity(character.velocity());
    }
}

/// First spawn marker's ground position, if the level has one.
pub(crate) fn find_spawn(pieces: &[Piece]) -> Option<&Piece> {
    pieces.iter().find(|piece| {
        shapes()
            .get(piece.shape)
            .is_some_and(|shape| shape.id == "spawn")
    })
}

pub(crate) fn respawn_character(editor: &mut Editor, orbit: &mut Orbit) {
    let Some(character) = editor.character.as_mut() else {
        return;
    };
    if character.health.is_alive() {
        return;
    }
    if let Some(spawn) = find_spawn(&editor.pieces) {
        character.pos = spawn.pos;
        // A little above the marker so physics settles instead of starting
        // interpenetrating.
        character.feet = spawn.elevation + 0.1;
    }
    character.health = Health::default();
    character.vy = 0.0;
    character.hv = Vec2::ZERO;
    character.ride_vel = Vec2::ZERO;
    character.grounded = false;
    editor.walk = None;
    editor.note = "respawned";
    if orbit.mode == ViewMode::Third {
        orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
    }
}

pub(super) fn respawn_on_death(mut editor: ResMut<Editor>, mut orbit: ResMut<Orbit>) {
    respawn_character(&mut editor, &mut orbit);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_piece(id: u32, pos: Vec2) -> Piece {
        Piece {
            id,
            shape: model::shape_index("spawn").unwrap(),
            pos,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
        }
    }

    fn ready_character(health: Health, hv: Vec2, vy: f32) -> Character {
        Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy,
            hv,
            ride_vel: Vec2::ZERO,
            grounded: false,
            health,
        }
    }

    fn dead_character(pos: Vec2) -> Character {
        let mut character = ready_character(Health::default(), Vec2::ZERO, 0.0);
        character.pos = pos;
        character.health.damage(character.health.max + 10.0);
        character
    }

    #[test]
    fn health_fill_percent_matches_fraction() {
        assert_eq!(fill_percent(1.0), 100.0);
        assert_eq!(fill_percent(0.5), 50.0);
        assert_eq!(fill_percent(0.0), 0.0);
        assert_eq!(fill_percent(-1.0), 0.0);
        assert_eq!(fill_percent(2.0), 100.0);
    }

    #[test]
    fn health_and_velocity_are_formatted_for_the_window() {
        assert_eq!(format_health(Health::new(40.0, 100.0)), "40 / 100");
        assert_eq!(format_speed(2.5), "2.50 m/s");
        assert_eq!(
            format_velocity(Vec3::new(1.5, 2.0, 0.0)),
            "x 1.50   y 2.00   z 0.00"
        );
        assert_eq!(format_speed(f32::NAN), "-");
        assert_eq!(format_velocity(Vec3::splat(f32::NAN)), "x -   y -   z -");
    }

    #[test]
    fn character_velocity_folds_horizontal_and_vertical() {
        let character = ready_character(Health::default(), Vec2::new(1.5, 0.0), 2.0);
        assert_eq!(character.velocity(), Vec3::new(1.5, 2.0, 0.0));
        assert!((character.speed() - 2.5).abs() < 1e-6);

        let mut riding = character;
        riding.ride_vel = Vec2::new(0.5, -1.0);
        assert_eq!(riding.velocity(), Vec3::new(2.0, 2.0, -1.0));
    }

    #[test]
    fn the_status_window_tracks_the_character() {
        let mut app = App::new();
        app.init_resource::<Editor>();
        app.add_systems(Update, update_character_status);

        let root = app
            .world_mut()
            .spawn((CharacterStatus, Node::default()))
            .id();
        let fill = app
            .world_mut()
            .spawn((HealthFill, Node::default(), BackgroundColor::default()))
            .id();
        let health = app.world_mut().spawn((HealthText, Text::new(""))).id();
        let speed = app.world_mut().spawn((VelocityText, Text::new(""))).id();
        let axes = app.world_mut().spawn((VelocityAxes, Text::new(""))).id();

        app.world_mut().resource_mut::<Editor>().character = Some(ready_character(
            Health::new(40.0, 100.0),
            Vec2::new(1.5, 0.0),
            2.0,
        ));
        app.update();

        assert_eq!(app.world().get::<Text>(health).unwrap().0, "40 / 100");
        assert_eq!(app.world().get::<Text>(speed).unwrap().0, "2.50 m/s");
        assert_eq!(
            app.world().get::<Text>(axes).unwrap().0,
            "x 1.50   y 2.00   z 0.00"
        );
        assert_eq!(app.world().get::<Node>(fill).unwrap().width, percent(40));
        assert_eq!(
            app.world().get::<Node>(root).unwrap().display,
            Display::Flex
        );

        app.world_mut().resource_mut::<Editor>().character = None;
        app.update();
        assert_eq!(
            app.world().get::<Node>(root).unwrap().display,
            Display::None
        );
    }

    #[test]
    fn find_spawn_returns_the_first_spawn_marker() {
        let block = Piece {
            id: 1,
            shape: model::shape_index("block").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
        };
        let spawn = spawn_piece(2, Vec2::new(3.0, 4.0));
        assert!(find_spawn(&[]).is_none());
        assert!(find_spawn(&[block]).is_none());
        assert_eq!(find_spawn(&[spawn]).unwrap().pos, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn death_respawns_at_spawn_with_full_health() {
        let mut editor = Editor::default();
        editor.pieces.push(spawn_piece(1, Vec2::new(3.0, 4.0)));
        editor.character = Some(dead_character(Vec2::ZERO));
        let mut orbit = Orbit::default();
        respawn_character(&mut editor, &mut orbit);
        let character = editor.character.unwrap();
        assert_eq!(character.pos, Vec2::new(3.0, 4.0));
        assert_eq!(character.health, Health::default());
        assert_eq!(editor.note, "respawned");
        assert_eq!(orbit.target, Vec3::new(3.0, 0.0, 4.0));
    }

    #[test]
    fn death_without_a_spawn_revives_in_place() {
        let mut editor = Editor::default();
        editor.character = Some(dead_character(Vec2::new(1.0, 2.0)));
        let mut orbit = Orbit::default();
        respawn_character(&mut editor, &mut orbit);
        let character = editor.character.unwrap();
        assert_eq!(character.pos, Vec2::new(1.0, 2.0));
        assert!(character.health.is_alive());
    }

    #[test]
    fn a_living_character_is_untouched() {
        let mut editor = Editor::default();
        editor.pieces.push(spawn_piece(1, Vec2::new(9.0, 9.0)));
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.7,
            vy: 1.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            health: Health::new(40.0, 100.0),
        });
        let mut orbit = Orbit::default();
        respawn_character(&mut editor, &mut orbit);
        let character = editor.character.unwrap();
        assert_eq!(character.pos, Vec2::ZERO);
        assert_eq!(character.health.current, 40.0);
        assert_eq!(character.vy, 1.0);
    }
}
