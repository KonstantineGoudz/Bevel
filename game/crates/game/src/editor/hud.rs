//! Health-bar HUD and respawn-on-death.
//!
//! The bar is plain Bevy UI spawned once in `setup`; `update_health_hud`
//! keeps its width/colour/text in sync with `Editor.character`. When health
//! hits zero, `respawn_on_death` moves the character to the first spawn
//! marker (if any) with full health.

use super::*;

#[derive(Component)]
pub(super) struct HealthHud;

#[derive(Component)]
pub(super) struct HealthFill;

#[derive(Component)]
pub(super) struct HealthText;

const HUD_WIDTH: f32 = 240.0;
const HUD_HEIGHT: f32 = 22.0;
const HUD_BG: Color = Color::srgba(0.04, 0.05, 0.07, 0.82);
const HUD_BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);

/// Width of the fill as a percent of the bar, for tests and the updater.
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

pub(super) fn spawn_health_hud(commands: &mut Commands) {
    let root = commands
        .spawn((
            HealthHud,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: px(8),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                column_gap: px(8),
                ..default()
            },
        ))
        .id();

    let bar = commands
        .spawn((
            Pickable::IGNORE,
            Node {
                width: px(HUD_WIDTH),
                height: px(HUD_HEIGHT),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(HUD_BG),
            BorderColor::all(HUD_BORDER),
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
    commands.entity(bar).add_child(fill);

    let label = commands
        .spawn((
            HealthText,
            Pickable::IGNORE,
            Text::new("100 / 100"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
        ))
        .id();

    commands.entity(root).add_children(&[bar, label]);
}

pub(super) fn update_health_hud(
    editor: Res<Editor>,
    mut roots: Query<&mut Node, (With<HealthHud>, Without<HealthFill>)>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), (With<HealthFill>, Without<HealthHud>)>,
    mut labels: Query<&mut Text, With<HealthText>>,
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
    for mut text in &mut labels {
        text.0 = format!(
            "{} / {}",
            character.health.current.ceil() as i32,
            character.health.max.ceil() as i32
        );
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
        }
    }

    fn dead_character(pos: Vec2) -> Character {
        let mut health = Health::default();
        health.damage(health.max + 10.0);
        Character {
            pos,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            grounded: false,
            health,
        }
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
