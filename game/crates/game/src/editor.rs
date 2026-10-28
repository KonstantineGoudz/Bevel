//! A 3D level editor: a component palette on the left, an orbit camera, and
//! solid meshes extruded from the same footprints the model tests use.
//!
//! Left-drag places and moves, right-drag orbits, middle-drag pans, the wheel
//! zooms. Everything is in metres.

use std::collections::HashMap;

use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::shape::InfinitePlane3d;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::PrimaryWindow;

use crate::mesh::extrude;
use crate::model::{
    self, AnchorKind, CATEGORIES, HIT_PAD, PANEL_WIDTH, Piece, SNAP_THRESHOLD, SnapKind, Style,
    shapes,
};

const PANEL_BG: Color = Color::srgb_u8(0x10, 0x16, 0x1e);
const BUTTON_BG: Color = Color::srgb_u8(0x14, 0x1c, 0x26);
const BUTTON_ON: Color = Color::srgb_u8(0x33, 0x27, 0x12);
const BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);
const BORDER_ON: Color = Color::srgb_u8(0xb0, 0x77, 0x28);
const MUTED: Color = Color::srgb_u8(0x86, 0x97, 0xab);
const AMBER: Color = Color::srgb_u8(0xf0, 0xa6, 0x3c);
const GRID: Color = Color::srgba(0.47, 0.59, 0.71, 0.10);
const GRID_HOVER: Color = Color::srgba(0.47, 0.66, 0.86, 0.45);
const GRID_Y: f32 = 0.004;

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<Cursor>()
            .init_resource::<Orbit>()
            .init_resource::<Visuals>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    camera_control,
                    update_cursor,
                    interact,
                    sync_visuals,
                    update_palette,
                    update_status,
                    draw,
                )
                    .chain(),
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
    ray: Option<Ray3d>,
}

#[derive(Resource)]
struct Orbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.85,
            pitch: 0.9,
            distance: 7.5,
        }
    }
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

#[derive(Component)]
struct PieceVisual;

/// One entity per piece (and one for the ghost, keyed 0).
#[derive(Resource, Default)]
struct Visuals {
    map: HashMap<u32, Entity>,
}

/// Meshes and materials, built once at startup.
#[derive(Resource)]
struct Kit {
    /// Per shape, the meshes that make it up and which material slot each uses.
    parts: Vec<Vec<(Handle<Mesh>, u8)>>,
    mats: Vec<Handle<StandardMaterial>>,
    glass: Handle<StandardMaterial>,
    ghost: Handle<StandardMaterial>,
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let orbit = Orbit::default();
    let eye = orbit.target
        + Vec3::new(
            orbit.pitch.cos() * orbit.yaw.sin(),
            orbit.pitch.sin(),
            orbit.pitch.cos() * orbit.yaw.cos(),
        ) * orbit.distance;
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60.0_f32.to_radians(),
            near: 0.05,
            far: 400.0,
            ..default()
        }),
        Transform::from_translation(eye).looking_at(orbit.target, Vec3::Y),
    ));

    commands.insert_resource(ClearColor(Color::srgb_u8(0x0a, 0x0d, 0x12)));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb_u8(0xb8, 0xc8, 0xe0),
        brightness: 280.0,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            illuminance: 7_500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(7.0, 12.0, 6.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    // Solid geometry: one mesh set per catalogue entry.
    let glass = materials.add(StandardMaterial {
        base_color: Color::srgba(0.60, 0.80, 0.90, 0.26),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.15,
        ..default()
    });
    let ghost = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.78, 0.38, 0.34),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });

    let mut parts = Vec::with_capacity(shapes().len());
    let mut mats = Vec::with_capacity(shapes().len());
    for shape in shapes() {
        mats.push(materials.add(StandardMaterial {
            base_color: shape.color,
            perceptual_roughness: 0.85,
            metallic: 0.0,
            ..default()
        }));
        let mut made = Vec::new();
        for (outline, base, height, slot) in solid_parts(shape) {
            made.push((meshes.add(extrude(&outline, base, height)), slot));
        }
        parts.push(made);
    }
    commands.insert_resource(Kit {
        parts,
        mats,
        glass,
        ghost,
    });

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
            BorderColor::all(BORDER),
        ))
        .id();

    let mut children = vec![
        commands
            .spawn((
                Text::new("COMPONENTS"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(MUTED),
                Node {
                    margin: UiRect::bottom(px(8)),
                    ..default()
                },
            ))
            .id(),
    ];

    for category in CATEGORIES {
        children.push(
            commands
                .spawn((
                    Text::new(category.to_uppercase()),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0x6c, 0x7f, 0x95)),
                    Node {
                        margin: UiRect::top(px(10)),
                        ..default()
                    },
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
                            TextFont {
                                font_size: FontSize::Px(13.0),
                                ..default()
                            },
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
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(MUTED),
        Node {
            position_type: PositionType::Absolute,
            left: px(PANEL_WIDTH + 14.0),
            bottom: px(10.0),
            ..default()
        },
    ));

    commands.spawn((
        Text::new(
            "LMB place / drag   |   RMB orbit   |   MMB / Shift+RMB pan   |   wheel zoom   |   R rotate   |   Del remove   |   Ctrl+Z undo   |   S save",
        ),
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

/// The meshes that make up one catalogue entry: outline, base, height, slot.
/// Slot 0 is the shape's own colour, slot 1 is glass.
fn solid_parts(shape: &model::Shape) -> Vec<(Vec<Vec2>, f32, f32, u8)> {
    let outline = model::shape_points(shape);
    if shape.style == Style::Window {
        let sill = 0.95;
        let head = 2.2;
        vec![
            (outline.clone(), shape.base, sill, 0),
            (outline.clone(), head, (shape.top() - head).max(0.05), 0),
            (outline, sill, (head - sill).max(0.05), 1),
        ]
    } else {
        vec![(outline, shape.base, shape.height.max(0.02), 0)]
    }
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

fn camera_control(
    mut orbit: ResMut<Orbit>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let delta: Vec2 = motion.read().map(|m| m.delta).sum();
    // Browsers report wheel deltas in pixels (often +-100), while some
    // platforms report whole lines. Normalise the big values to lines.
    let raw_scroll: f32 = wheel.read().map(|w| w.y).sum();
    let scroll = if raw_scroll.abs() > 10.0 {
        raw_scroll / 120.0
    } else {
        raw_scroll
    };
    let over_panel = window.cursor_position().is_some_and(|p| p.x <= PANEL_WIDTH);

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    if !over_panel {
        if mouse.pressed(MouseButton::Right) && !shift {
            orbit.yaw -= delta.x * 0.006;
            orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(0.12, 1.5);
        }
        if mouse.pressed(MouseButton::Middle) || (mouse.pressed(MouseButton::Right) && shift) {
            let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
            let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
            let k = orbit.distance * 0.0018;
            orbit.target -= right * delta.x * k;
            orbit.target += forward * delta.y * k;
            orbit.target.y = 0.0;
        }
        if scroll != 0.0 {
            orbit.distance = (orbit.distance * (1.0 - scroll * 0.12)).clamp(1.5, 60.0);
        }
    }

    let offset = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.sin(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.cos(),
    ) * orbit.distance;
    camera.translation = orbit.target + offset;
    camera.look_at(orbit.target, Vec3::Y);
}

fn update_cursor(
    mut cursor: ResMut<Cursor>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform)>,
) {
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    cursor.screen = window.cursor_position();
    cursor.ray = cursor
        .screen
        .and_then(|p| camera.viewport_to_world(&global, p).ok());
    cursor.world = cursor.ray.and_then(|ray| {
        ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            .map(|p| Vec2::new(p.x, p.z))
    });
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

fn on_palette_activate(
    event: On<Activate>,
    buttons: Query<&PaletteButton>,
    mut editor: ResMut<Editor>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    editor.tool = if editor.tool == Some(button.0) {
        None
    } else {
        Some(button.0)
    };
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
        let raw = Piece {
            id: 0,
            shape: tool,
            pos: world,
            rot: editor.ghost_rot,
        };
        let snap = model::snap_piece(
            &raw,
            shape,
            &editor.pieces,
            editor.snap,
            editor.snap,
            SNAP_THRESHOLD,
        );
        editor.ghost = Some(Ghost {
            pos: snap.pos,
            rot: snap.rot,
            kind: snap.kind,
        });

        if mouse.just_pressed(MouseButton::Left) {
            editor.commit();
            let id = editor.next_id;
            editor.next_id += 1;
            editor.pieces.push(Piece {
                id,
                shape: tool,
                pos: snap.pos,
                rot: snap.rot,
            });
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
        if let Some(hit) = cursor.ray.and_then(|ray| pick(&editor.pieces, ray)) {
            editor.selected = Some(hit);
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == hit) {
                editor.drag = Some(Drag {
                    id: hit,
                    offset: piece.pos - world,
                    moved: false,
                });
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
                let raw = Piece {
                    pos: world + drag.offset,
                    ..piece
                };
                let snap = model::snap_piece(
                    &raw,
                    shape,
                    &editor.pieces,
                    editor.snap,
                    editor.snap,
                    SNAP_THRESHOLD,
                );
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

/// Ray against each piece's local bounding box, expanded by the click pad.
/// A box test (rather than a footprint at one height) means a wall's whole
/// visible face is clickable, and tall props win over the floor under them.
fn pick(pieces: &[Piece], ray: Ray3d) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for piece in pieces {
        let shape = &shapes()[piece.shape];
        if let Some(distance) = ray_piece_distance(piece, shape, ray) {
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, piece.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

fn ray_piece_distance(piece: &Piece, shape: &model::Shape, ray: Ray3d) -> Option<f32> {
    // Into the piece's local frame: model `y` is world `z`, yaw is `-rot`.
    let (sin, cos) = piece.rot.sin_cos();
    let ox = ray.origin.x - piece.pos.x;
    let oz = ray.origin.z - piece.pos.y;
    let origin = Vec3::new(ox * cos + oz * sin, ray.origin.y, -ox * sin + oz * cos);
    let direction = *ray.direction;
    let dir = Vec3::new(
        direction.x * cos + direction.z * sin,
        direction.y,
        -direction.x * sin + direction.z * cos,
    );

    let points = model::shape_points(shape);
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for p in &points {
        low = low.min(*p);
        high = high.max(*p);
    }

    let pad = HIT_PAD;
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    slab(
        origin.x,
        dir.x,
        low.x - pad,
        high.x + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.z,
        dir.z,
        low.y - pad,
        high.y + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.y,
        dir.y,
        shape.base - pad,
        shape.top() + pad,
        &mut near,
        &mut far,
    )?;

    (far >= 0.0).then(|| near.max(0.0))
}

fn slab(origin: f32, dir: f32, low: f32, high: f32, near: &mut f32, far: &mut f32) -> Option<()> {
    if dir.abs() < 1e-6 {
        return (origin >= low && origin <= high).then_some(());
    }
    let mut a = (low - origin) / dir;
    let mut b = (high - origin) / dir;
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    *near = near.max(a);
    *far = far.min(b);
    (*near <= *far).then_some(())
}

// ---------------------------------------------------------------------------
// Visuals
// ---------------------------------------------------------------------------

fn spawn_piece(commands: &mut Commands, kit: &Kit, piece: &Piece, ghost: bool) -> Entity {
    let root = commands
        .spawn((
            PieceVisual,
            Transform {
                translation: Vec3::new(piece.pos.x, 0.0, piece.pos.y),
                rotation: model::yaw_quat(piece.rot),
                ..default()
            },
        ))
        .id();

    for (mesh, slot) in &kit.parts[piece.shape] {
        let material = if ghost {
            kit.ghost.clone()
        } else if *slot == 1 {
            kit.glass.clone()
        } else {
            kit.mats[piece.shape].clone()
        };
        let child = commands
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material)))
            .id();
        commands.entity(root).add_child(child);
    }
    root
}

fn sync_visuals(
    mut commands: Commands,
    editor: Res<Editor>,
    kit: Res<Kit>,
    mut visuals: ResMut<Visuals>,
    mut transforms: Query<&mut Transform, With<PieceVisual>>,
) {
    let ghost_wanted = editor.tool.is_some() && editor.ghost.is_some();

    visuals.map.retain(|id, entity| {
        let keep = if *id == 0 {
            ghost_wanted
        } else {
            editor.pieces.iter().any(|p| p.id == *id)
        };
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for piece in &editor.pieces {
        match visuals.map.get(&piece.id) {
            Some(&entity) => {
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, 0.0, piece.pos.y);
                    transform.rotation = model::yaw_quat(piece.rot);
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, piece, false);
                visuals.map.insert(piece.id, entity);
            }
        }
    }

    if let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost) {
        let piece = Piece {
            id: 0,
            shape: tool,
            pos: ghost.pos,
            rot: ghost.rot,
        };
        match visuals.map.get(&0) {
            Some(&entity) => {
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, 0.0, piece.pos.y);
                    transform.rotation = model::yaw_quat(piece.rot);
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, &piece, true);
                visuals.map.insert(0, entity);
            }
        }
    }
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

fn update_status(
    editor: Res<Editor>,
    cursor: Res<Cursor>,
    orbit: Res<Orbit>,
    mut text: Single<&mut Text, With<StatusText>>,
) {
    let hex = cursor
        .world
        .map(|p| {
            let (q, r) = model::pixel_to_hex(p);
            format!("hex {q},{r}")
        })
        .unwrap_or_else(|| "-".to_string());

    let tool = editor.tool.map(|t| shapes()[t].name).unwrap_or("select");

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
        "{}   |   {} placed   |   tool: {}   |   selected: {}   |   snap {}   |   zoom {:.1} m   |   {}",
        hex,
        editor.pieces.len(),
        tool,
        selection,
        snap,
        orbit.distance,
        editor.note,
    );
}

// ---------------------------------------------------------------------------
// Drawing (gizmo overlays only; the solids are entities)
// ---------------------------------------------------------------------------

fn draw(editor: Res<Editor>, cursor: Res<Cursor>, orbit: Res<Orbit>, mut gizmos: Gizmos) {
    let (cq, cr) = model::pixel_to_hex(Vec2::new(orbit.target.x, orbit.target.z));
    for dq in -8..=8 {
        for dr in -8..=8 {
            let center = model::hex_center(cq + dq, cr + dr);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|p| Vec3::new(p.x, GRID_Y, p.y))
                .collect();
            closed(&mut gizmos, &points, GRID);
        }
    }

    if let Some(p) = cursor.world {
        let center = model::nearest_hex(p);
        let points: Vec<Vec3> = model::hex_corners(center)
            .iter()
            .map(|q| Vec3::new(q.x, GRID_Y + 0.002, q.y))
            .collect();
        closed(&mut gizmos, &points, GRID_HOVER);
    }

    if let Some(id) = editor.selected {
        if let Some(piece) = editor.pieces.iter().find(|p| p.id == id) {
            if let Some(shape) = shapes().get(piece.shape) {
                selection_box(&mut gizmos, piece, shape);
                anchors(&mut gizmos, piece, shape);
            }
        }
    }

    if let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost) {
        if let Some(shape) = shapes().get(tool) {
            let piece = Piece {
                id: 0,
                shape: tool,
                pos: ghost.pos,
                rot: ghost.rot,
            };
            anchors(&mut gizmos, &piece, shape);
        }
    }
}

fn closed(gizmos: &mut Gizmos, points: &[Vec3], colour: Color) {
    if points.len() < 2 {
        return;
    }
    gizmos.linestrip(
        points.iter().copied().chain(std::iter::once(points[0])),
        colour,
    );
}

fn selection_box(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let points = model::shape_points(shape);
    let (sin, cos) = piece.rot.sin_cos();
    let at = |p: Vec2, y: f32| {
        Vec3::new(
            piece.pos.x + p.x * cos - p.y * sin,
            y,
            piece.pos.y + p.x * sin + p.y * cos,
        )
    };
    let low = shape.base - 0.01;
    let high = shape.top() + 0.01;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        gizmos.line(at(a, low), at(b, low), AMBER);
        gizmos.line(at(a, high), at(b, high), AMBER);
        gizmos.line(at(a, low), at(a, high), AMBER);
    }
}

fn anchors(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let y = shape.top() + 0.03;
    let size = 0.07;
    for anchor in model::world_anchors(piece, shape) {
        let c = Vec3::new(anchor.pos.x, y, anchor.pos.y);
        let corners = [
            c + Vec3::new(0.0, 0.0, size),
            c + Vec3::new(size, 0.0, 0.0),
            c + Vec3::new(0.0, 0.0, -size),
            c + Vec3::new(-size, 0.0, 0.0),
        ];
        closed(gizmos, &corners, anchor.kind.color());
        if anchor.kind == AnchorKind::End {
            let normal = Vec3::new(anchor.normal.x, 0.0, anchor.normal.y);
            gizmos.line(c, c + normal * 0.15, anchor.kind.color());
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
                "{{\"shape\":\"{}\",\"x\":{:.3},\"z\":{:.3},\"rotDeg\":{:.1}}}",
                shapes()[p.shape].id,
                p.pos.x,
                p.pos.y,
                p.rot.to_degrees()
            )
        })
        .collect();
    format!(
        "{{\"format\":\"party-dungeon-level\",\"version\":1,\"units\":\"metres\",\"hexSize\":{},\"pieces\":[{}]}}",
        model::HEX_SIZE,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Dir3;

    /// The default camera eye, looking wherever `target` is.
    fn ray_towards(target: Vec3) -> Ray3d {
        let eye = Vec3::new(3.5, 5.9, 3.1);
        Ray3d::new(eye, Dir3::new(target - eye).unwrap())
    }

    #[test]
    fn a_wall_face_is_clickable_at_mid_height() {
        let wall = model::shape_index("wall").unwrap();
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![
            Piece {
                id: 1,
                shape: floor,
                pos: Vec2::ZERO,
                rot: 0.0,
            },
            Piece {
                id: 2,
                shape: wall,
                pos: Vec2::new(0.519, -0.9),
                rot: std::f32::consts::FRAC_PI_2,
            },
        ];
        let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 1.4, -0.9)));
        assert_eq!(
            hit,
            Some(2),
            "clicking the wall face should pick the wall, not the floor under it"
        );
    }

    #[test]
    fn a_wall_is_clickable_near_its_top_too() {
        let wall = model::shape_index("wall").unwrap();
        let pieces = vec![Piece {
            id: 2,
            shape: wall,
            pos: Vec2::new(0.519, -0.9),
            rot: std::f32::consts::FRAC_PI_2,
        }];
        let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 2.6, -0.9)));
        assert_eq!(hit, Some(2), "the upper face of a wall should be clickable");
    }

    #[test]
    fn the_floor_is_clickable() {
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![Piece {
            id: 1,
            shape: floor,
            pos: Vec2::ZERO,
            rot: 0.0,
        }];
        assert_eq!(
            pick(&pieces, ray_towards(Vec3::new(0.2, 0.0, 0.2))),
            Some(1)
        );
    }

    #[test]
    fn clicking_empty_ground_selects_nothing() {
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![Piece {
            id: 1,
            shape: floor,
            pos: Vec2::ZERO,
            rot: 0.0,
        }];
        assert_eq!(pick(&pieces, ray_towards(Vec3::new(5.0, 0.0, 5.0))), None);
    }
}
