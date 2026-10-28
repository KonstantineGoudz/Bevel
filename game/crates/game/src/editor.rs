//! A simple level editor: a component palette on the left, a hex grid to build
//! on, and gizmo rendering. No backend, no assets — the pieces are drawn from
//! the same geometry the model tests use.

use bevy::color::Alpha;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::PrimaryWindow;

use crate::model::{
    self, AnchorKind, CATEGORIES, HEX_SIZE, PANEL_WIDTH, Piece, SnapKind, shapes,
};

const PANEL_BG: Color = Color::srgb_u8(0x10, 0x16, 0x1e);
const BUTTON_BG: Color = Color::srgb_u8(0x14, 0x1c, 0x26);
const BUTTON_ON: Color = Color::srgb_u8(0x33, 0x27, 0x12);
const BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);
const BORDER_ON: Color = Color::srgb_u8(0xb0, 0x77, 0x28);
const MUTED: Color = Color::srgb_u8(0x86, 0x97, 0xab);
const AMBER: Color = Color::srgb_u8(0xf0, 0xa6, 0x3c);
const GRID: Color = Color::srgba(0.47, 0.59, 0.71, 0.10);
const GRID_HOVER: Color = Color::srgba(0.47, 0.66, 0.86, 0.30);

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<Cursor>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (update_cursor, interact, update_palette, update_status, draw).chain(),
            )
            .add_observer(on_palette_activate);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct Editor {
    pieces: Vec<Piece>,
    tool: Option<usize>,
    selected: Option<u32>,
    next_id: u32,
    snap: bool,
    ghost_rot: f32,
    ghost: Option<Ghost>,
    drag: Option<Drag>,
    undo: Vec<Vec<Piece>>,
    note: &'static str,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            pieces: Vec::new(),
            tool: None,
            selected: None,
            next_id: 1,
            snap: true,
            ghost_rot: 0.0,
            ghost: None,
            drag: None,
            undo: Vec::new(),
            note: "Click a component, then the grid",
        }
    }
}

#[derive(Clone, Copy)]
struct Ghost {
    pos: Vec2,
    rot: f32,
    kind: SnapKind,
}

#[derive(Clone, Copy)]
struct Drag {
    id: u32,
    offset: Vec2,
    moved: bool,
}

#[derive(Resource, Default)]
struct Cursor {
    screen: Option<Vec2>,
    world: Option<Vec2>,
}

impl Editor {
    fn commit(&mut self) {
        self.undo.push(self.pieces.clone());
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.pieces = prev;
            self.selected = None;
        }
    }
}

#[derive(Component)]
struct PaletteButton(usize);

#[derive(Component)]
struct StatusText;

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);

    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: px(PANEL_WIDTH),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(12)),
                row_gap: px(3),
                border: UiRect::right(px(1)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(Color::srgb_u8(0x24, 0x30, 0x3f)),
        ))
        .id();

    let mut children = vec![
        commands
            .spawn((
                Text::new("COMPONENTS"),
                TextFont { font_size: FontSize::Px(12.0), ..default() },
                TextColor(MUTED),
                Node { margin: UiRect::bottom(px(8)), ..default() },
            ))
            .id(),
    ];

    for category in CATEGORIES {
        children.push(
            commands
                .spawn((
                    Text::new(category.to_uppercase()),
                    TextFont { font_size: FontSize::Px(11.0), ..default() },
                    TextColor(Color::srgb_u8(0x6c, 0x7f, 0x95)),
                    Node { margin: UiRect::top(px(10)), ..default() },
                ))
                .id(),
        );

        for (index, shape) in shapes().iter().enumerate() {
            if shape.category != category {
                continue;
            }
            children.push(
                commands
                    .spawn((
                        Button,
                        PaletteButton(index),
                        Node {
                            width: percent(100),
                            height: px(24),
                            align_items: AlignItems::Center,
                            padding: UiRect::horizontal(px(8)),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(6)),
                            ..default()
                        },
                        BackgroundColor(BUTTON_BG),
                        BorderColor::all(BORDER),
                        children![(
                            Text::new(shape.name),
                            TextFont { font_size: FontSize::Px(13.0), ..default() },
                            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                        )],
                    ))
                    .id(),
            );
        }
    }

    commands.entity(panel).add_children(&children);

    commands.spawn((
        StatusText,
        Text::new(""),
        TextFont { font_size: FontSize::Px(12.0), ..default() },
        TextColor(MUTED),
        Node {
            position_type: PositionType::Absolute,
            left: px(PANEL_WIDTH + 14.0),
            bottom: px(10.0),
            ..default()
        },
    ));

    commands.spawn((
        Text::new("R rotate  |  Del remove  |  Ctrl+Z undo  |  S save  |  Esc cancel"),
        TextFont { font_size: FontSize::Px(11.0), ..default() },
        TextColor(Color::srgb_u8(0x5f, 0x70, 0x85)),
        Node {
            position_type: PositionType::Absolute,
            left: px(PANEL_WIDTH + 14.0),
            top: px(10.0),
            ..default()
        },
    ));
}

// ---------------------------------------------------------------------------
// Cursor and input
// ---------------------------------------------------------------------------

fn update_cursor(
    mut cursor: ResMut<Cursor>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
) {
    let (camera, transform) = *camera;
    cursor.screen = window.cursor_position();
    cursor.world = cursor
        .screen
        .and_then(|p| camera.viewport_to_world_2d(transform, p).ok());
}

fn on_palette_activate(
    event: On<Activate>,
    buttons: Query<&PaletteButton>,
    mut editor: ResMut<Editor>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    editor.tool = if editor.tool == Some(button.0) { None } else { Some(button.0) };
    editor.selected = None;
    editor.ghost = None;
    editor.ghost_rot = 0.0;
}

fn interact(
    mut editor: ResMut<Editor>,
    cursor: Res<Cursor>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let (Some(screen), Some(world)) = (cursor.screen, cursor.world) else {
        return;
    };

    if keys.just_pressed(KeyCode::Escape) {
        editor.tool = None;
        editor.selected = None;
        editor.ghost = None;
        editor.drag = None;
        editor.note = "Click a component, then the grid";
    }

    if keys.just_pressed(KeyCode::KeyR) {
        let step = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            -model::ANCHOR_STEP
        } else {
            model::ANCHOR_STEP
        };
        if let Some(id) = editor.selected {
            if editor.pieces.iter().any(|p| p.id == id) {
                editor.commit();
                if let Some(piece) = editor.pieces.iter_mut().find(|p| p.id == id) {
                    piece.rot = model::normalize_angle(piece.rot + step);
                }
            }
        } else if editor.tool.is_some() {
            editor.ghost_rot = model::normalize_angle(editor.ghost_rot + step);
        }
    }

    if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace) {
        if let Some(id) = editor.selected {
            editor.commit();
            editor.pieces.retain(|p| p.id != id);
            editor.selected = None;
        }
    }

    if keys.just_pressed(KeyCode::KeyZ)
        && (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        editor.undo();
    }

    if keys.just_pressed(KeyCode::KeyS)
        && !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        save(&editor);
    }

    // Clicks on the palette are handled by the widget observers; ignore them here.
    if screen.x <= PANEL_WIDTH {
        return;
    }

    if let Some(tool) = editor.tool {
        let shape = &shapes()[tool];
        let raw = Piece { id: 0, shape: tool, pos: world, rot: editor.ghost_rot };
        let snap = model::snap_piece(&raw, shape, &editor.pieces, editor.snap, editor.snap, 22.0);
        editor.ghost = Some(Ghost { pos: snap.pos, rot: snap.rot, kind: snap.kind });

        if mouse.just_pressed(MouseButton::Left) {
            editor.commit();
            let id = editor.next_id;
            editor.next_id += 1;
            editor.pieces.push(Piece { id, shape: tool, pos: snap.pos, rot: snap.rot });
            editor.selected = Some(id);
            editor.note = match snap.kind {
                SnapKind::Anchor => "placed: anchor snap",
                SnapKind::Grid => "placed: grid snap",
                SnapKind::Free => "placed: free",
            };
        }
        return;
    }

    editor.ghost = None;

    if mouse.just_pressed(MouseButton::Left) {
        if let Some(hit) = pick(&editor.pieces, world) {
            editor.selected = Some(hit);
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == hit) {
                editor.drag = Some(Drag { id: hit, offset: piece.pos - world, moved: false });
            }
            editor.commit();
        } else {
            editor.selected = None;
            editor.drag = None;
        }
    }

    if mouse.pressed(MouseButton::Left) {
        if let Some(drag) = editor.drag {
            if let Some((index, piece)) = editor
                .pieces
                .iter()
                .enumerate()
                .find(|(_, p)| p.id == drag.id)
                .map(|(i, p)| (i, *p))
            {
                let shape = &shapes()[piece.shape];
                let raw = Piece { pos: world + drag.offset, ..piece };
                let snap =
                    model::snap_piece(&raw, shape, &editor.pieces, editor.snap, editor.snap, 22.0);
                let target = &mut editor.pieces[index];
                target.pos = snap.pos;
                target.rot = snap.rot;
                if let Some(d) = editor.drag.as_mut() {
                    d.moved = true;
                }
            }
        }
    }

    if mouse.just_released(MouseButton::Left) {
        if let Some(drag) = editor.drag {
            if !drag.moved {
                editor.undo.pop();
            }
            editor.drag = None;
        }
    }
}

fn pick(pieces: &[Piece], p: Vec2) -> Option<u32> {
    pieces
        .iter()
        .rev()
        .find(|piece| {
            shapes()
                .get(piece.shape)
                .is_some_and(|shape| model::hit_test(shape, piece, p, 6.0))
        })
        .map(|p| p.id)
}

// ---------------------------------------------------------------------------
// Palette and status
// ---------------------------------------------------------------------------

fn update_palette(
    editor: Res<Editor>,
    mut buttons: Query<(&PaletteButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = editor.tool == Some(button.0);
        bg.0 = if on { BUTTON_ON } else { BUTTON_BG };
        *border = BorderColor::all(if on { BORDER_ON } else { BORDER });
    }
}

fn update_status(editor: Res<Editor>, cursor: Res<Cursor>, mut text: Single<&mut Text, With<StatusText>>) {
    let hex = cursor
        .world
        .map(|p| {
            let (q, r) = model::pixel_to_hex(p);
            format!("hex {q},{r}")
        })
        .unwrap_or_else(|| "-".to_string());

    let tool = editor
        .tool
        .map(|t| shapes()[t].name)
        .unwrap_or("select");

    let selection = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id))
        .map(|p| shapes()[p.shape].name)
        .unwrap_or("none");

    let snap = editor
        .ghost
        .map(|g| match g.kind {
            SnapKind::Anchor => "anchor",
            SnapKind::Grid => "grid",
            SnapKind::Free => "free",
        })
        .unwrap_or("none");

    text.0 = format!(
        "{}   |   {} placed   |   tool: {}   |   selected: {}   |   snap {}   |   {}",
        hex,
        editor.pieces.len(),
        tool,
        selection,
        snap,
        editor.note,
    );
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(editor: Res<Editor>, cursor: Res<Cursor>, window: Single<&Window>, mut gizmos: Gizmos) {
    let size = window.resolution.size();
    let half = size * 0.5;

    let (min_q, max_q, min_r, max_r) = grid_range(half);
    for q in min_q..=max_q {
        for r in min_r..=max_r {
            let center = model::hex_center(q, r);
            if center.x.abs() > half.x + HEX_SIZE || center.y.abs() > half.y + HEX_SIZE {
                continue;
            }
            closed(&mut gizmos, &model::hex_corners(center), GRID);
        }
    }

    if let Some(p) = cursor.world {
        closed(&mut gizmos, &model::hex_corners(model::nearest_hex(p)), GRID_HOVER);
    }

    for piece in &editor.pieces {
        if let Some(shape) = shapes().get(piece.shape) {
            let colour = if editor.selected == Some(piece.id) { AMBER } else { shape.color };
            closed(&mut gizmos, &world_points(piece, shape, 1.0), colour);
            surface_inner(&mut gizmos, piece, shape);
            detail(&mut gizmos, piece, shape);
        }
    }

    if let Some(id) = editor.selected {
        if let Some(piece) = editor.pieces.iter().find(|p| p.id == id) {
            if let Some(shape) = shapes().get(piece.shape) {
                anchors(&mut gizmos, piece, shape);
            }
        }
    }

    if let Some(ghost) = editor.ghost {
        if let Some(tool) = editor.tool {
            let piece = Piece { id: 0, shape: tool, pos: ghost.pos, rot: ghost.rot };
            if let Some(shape) = shapes().get(tool) {
                closed(&mut gizmos, &world_points(&piece, shape, 1.0), shape.color.with_alpha(0.55));
                surface_inner(&mut gizmos, &piece, shape);
                anchors(&mut gizmos, &piece, shape);
            }
        }
    }
}

fn grid_range(half: Vec2) -> (i32, i32, i32, i32) {
    let q = (half.x / (HEX_SIZE * 1.732_050_8)).ceil() as i32 + 2;
    let r = (half.y / (HEX_SIZE * 1.5)).ceil() as i32 + 2;
    (-q, q, -r - 2, r + 2)
}

fn world_points(piece: &Piece, shape: &model::Shape, scale: f32) -> Vec<Vec2> {
    let (sin, cos) = piece.rot.sin_cos();
    model::shape_points(shape)
        .into_iter()
        .map(|p| {
            let p = p * scale;
            Vec2::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos) + piece.pos
        })
        .collect()
}

/// Surfaces get a second, inset outline so a floor reads as a tile instead of
/// dissolving into the grid.
fn surface_inner(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    if matches!(shape.geom, model::Geometry::Hex { .. }) {
        closed(gizmos, &world_points(piece, shape, 0.72), shape.color.with_alpha(0.5));
    }
}

fn detail(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    use model::Style;
    let bar = Vec2::from_angle(piece.rot + std::f32::consts::FRAC_PI_2) * 9.0;
    match shape.style {
        Style::Door | Style::Gate => {
            gizmos.line_2d(piece.pos - bar, piece.pos + bar, shape.color.with_alpha(0.65));
        }
        Style::Secret => {
            gizmos.line_2d(piece.pos - bar * 0.6, piece.pos + bar * 0.6, shape.color.with_alpha(0.35));
        }
        _ => {}
    }
}

fn closed(gizmos: &mut Gizmos, points: &[Vec2], colour: Color) {
    if points.len() < 2 {
        return;
    }
    let first = points[0];
    gizmos.linestrip_2d(points.iter().copied().chain(std::iter::once(first)), colour);
}

fn anchors(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    for anchor in model::world_anchors(piece, shape) {
        let size = 4.5;
        let c = anchor.pos;
        let points = [
            c + Vec2::new(0.0, size),
            c + Vec2::new(size, 0.0),
            c + Vec2::new(0.0, -size),
            c + Vec2::new(-size, 0.0),
        ];
        closed(gizmos, &points, anchor.kind.color());
        if anchor.kind == AnchorKind::End {
            gizmos.line_2d(c, c + anchor.normal * 9.0, anchor.kind.color());
        }
    }
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

fn level_json(editor: &Editor) -> String {
    let pieces: Vec<String> = editor
        .pieces
        .iter()
        .map(|p| {
            format!(
                "{{\"shape\":\"{}\",\"x\":{:.2},\"y\":{:.2},\"rotDeg\":{:.1}}}",
                shapes()[p.shape].id,
                p.pos.x,
                p.pos.y,
                p.rot.to_degrees()
            )
        })
        .collect();
    format!(
        "{{\"format\":\"party-dungeon-level\",\"version\":1,\"hexSize\":{HEX_SIZE},\"pieces\":[{}]}}",
        pieces.join(",")
    )
}

fn save(editor: &Editor) {
    let json = level_json(editor);

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::path::Path::new("level.json");
        match std::fs::write(path, &json) {
            Ok(()) => info!("saved {} pieces to level.json", editor.pieces.len()),
            Err(err) => warn!("could not save level.json: {err}"),
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        // No filesystem in the browser; the console is the sink for now.
        bevy::log::info!("{}", json);
        let _ = &json;
    }
}
