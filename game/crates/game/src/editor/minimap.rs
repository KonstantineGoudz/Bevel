//! Play-mode minimap: a top-down viewport camera in the top-right corner.
//!
//! Visible only in Play. Drag the map to pan, roll the wheel over it to
//! zoom, click it to route / add a waypoint, drag a waypoint dot to move it.
//! The main canvas ignores presses inside the minimap rect; the observers
//! below own that corner instead.

use bevy::camera::{OrthographicProjection, ScalingMode, Viewport};

use super::*;

// Design-space geometry: multiplied by the UI scale (1 on desktop,
// `touch.ui_scale` on phones) to get logical pixels.
pub(super) const MINIMAP_SIZE: f32 = 220.0;
pub(super) const MINIMAP_RIGHT: f32 = 8.0;
pub(super) const MINIMAP_TOP: f32 = 50.0;
pub(super) const MINIMAP_MIN_HALF: f32 = 4.0;
pub(super) const MINIMAP_MAX_HALF: f32 = 40.0;
/// Camera altitude in metres; the projection is orthographic so this only
/// needs to clear the tallest piece.
const MINIMAP_HEIGHT: f32 = 60.0;

/// Marker for the top-down minimap camera. Main-camera queries exclude it
/// with `Without<MinimapCamera>` so they keep working unchanged.
#[derive(Component)]
pub(super) struct MinimapCamera;

/// Root of the minimap UI (frame + recenter). It carries no `InputLayer` so
/// one instance serves desktop and touch; `sync_minimap` toggles it.
#[derive(Component)]
pub(super) struct MinimapUi;

/// The pickable overlay rect exactly covering the minimap viewport. The 3D
/// view shows through its transparent background; its observers own presses
/// in that corner.
#[derive(Component)]
pub(super) struct MinimapFrame;

/// Recenter button below the frame. Kept outside the frame rect so its
/// clicks never double as map clicks.
#[derive(Component)]
pub(super) struct MinimapRecenter;

/// One in-flight minimap press.
#[derive(Clone, Copy)]
struct MinimapPress {
    pointer: PointerId,
}

/// Play-only top-down view state.
#[derive(Resource)]
pub(super) struct Minimap {
    pub(super) center: Vec2,
    pub(super) half_extent: f32,
    press: Option<MinimapPress>,
    handle: Option<PathHandle>,
    moved: f32,
    ready: bool,
}

impl Default for Minimap {
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            half_extent: 12.0,
            press: None,
            handle: None,
            moved: 0.0,
            ready: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure math (tested below; the live raycasts go through the real camera)
// ---------------------------------------------------------------------------

/// Design px to logical px for minimap geometry.
pub(super) fn minimap_scale(touch: &TouchState) -> f32 {
    if touch.enabled { touch.ui_scale } else { 1.0 }
}

/// Logical rect of the minimap square: top-left origin plus edge length.
pub(super) fn minimap_rect(size: Vec2, s: f32) -> (Vec2, f32) {
    let edge = MINIMAP_SIZE * s;
    let origin = Vec2::new(size.x - (MINIMAP_RIGHT + MINIMAP_SIZE) * s, MINIMAP_TOP * s);
    (origin, edge)
}

/// Whether a screen point (same space as `window.size()`) is on the minimap.
pub(super) fn minimap_hit(pos: Vec2, size: Vec2, s: f32) -> bool {
    let (origin, edge) = minimap_rect(size, s);
    pos.x >= origin.x && pos.x <= origin.x + edge && pos.y >= origin.y && pos.y <= origin.y + edge
}

/// World (x, z) to map px relative to the window. Screen up is world -Z.
#[allow(dead_code)] // exercised by tests; the live view goes through the camera
pub(super) fn world_to_map(world: Vec2, center: Vec2, half: f32, origin: Vec2, edge: f32) -> Vec2 {
    Vec2::new(
        origin.x + (world.x - center.x + half) / (2.0 * half) * edge,
        origin.y + (world.y - center.y + half) / (2.0 * half) * edge,
    )
}

/// Inverse of [`world_to_map`].
#[allow(dead_code)] // exercised by tests; the live view goes through the camera
pub(super) fn map_to_world(map: Vec2, center: Vec2, half: f32, origin: Vec2, edge: f32) -> Vec2 {
    Vec2::new(
        center.x - half + (map.x - origin.x) / edge * 2.0 * half,
        center.y - half + (map.y - origin.y) / edge * 2.0 * half,
    )
}

/// Drag delta (logical px, +x right, +y down) moves the view center the
/// opposite way so the content follows the pointer.
pub(super) fn pan_center(center: Vec2, delta: Vec2, edge: f32, half: f32) -> Vec2 {
    center - delta * (2.0 * half / edge)
}

/// Where the minimap looks when (re)opened: the character, else the level
/// centroid, else the origin.
pub(super) fn default_center(editor: &Editor) -> Vec2 {
    if let Some(character) = editor.character {
        return character.pos;
    }
    if editor.pieces.is_empty() {
        return Vec2::ZERO;
    }
    let sum = editor
        .pieces
        .iter()
        .fold(Vec2::ZERO, |acc, piece| acc + piece.pos);
    sum / editor.pieces.len() as f32
}

/// Half-extent that fits every piece and the character, with a margin.
pub(super) fn fit_half_extent(editor: &Editor) -> f32 {
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    let mut any = false;
    for piece in &editor.pieces {
        low = low.min(piece.pos);
        high = high.max(piece.pos);
        any = true;
    }
    if let Some(character) = editor.character {
        low = low.min(character.pos);
        high = high.max(character.pos);
        any = true;
    }
    if !any {
        return 12.0;
    }
    let span = (high - low).max_element().max(4.0);
    (span * 0.5 + 3.0).clamp(MINIMAP_MIN_HALF, MINIMAP_MAX_HALF)
}

/// Raycast the minimap ray against the level, like `surface_target` but from
/// an explicit ray instead of the main cursor.
fn minimap_surface(
    ray: Ray3d,
    rapier: &RapierContext,
    filter: QueryFilter,
    fallback: Vec2,
) -> (Vec2, f32) {
    rapier
        .cast_ray_and_get_normal(ray.origin, ray.direction.into(), 80.0, true, filter)
        .map(|(_, hit)| hit.point)
        .map(|point| (Vec2::new(point.x, point.z), point.y))
        .unwrap_or((fallback, 0.0))
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

pub(super) fn spawn_minimap(commands: &mut Commands) {
    commands.spawn((
        MinimapCamera,
        Camera3d::default(),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 2.0,
            },
            scale: 12.0,
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_translation(Vec3::new(0.0, MINIMAP_HEIGHT, 0.0))
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    ));

    let root = commands
        .spawn((
            MinimapUi,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                display: Display::None,
                ..default()
            },
        ))
        .id();

    let frame = commands
        .spawn((
            MinimapFrame,
            Node {
                position_type: PositionType::Absolute,
                right: px(MINIMAP_RIGHT),
                top: px(MINIMAP_TOP),
                width: px(MINIMAP_SIZE),
                height: px(MINIMAP_SIZE),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::srgba(0.5, 0.7, 0.85, 0.35)),
        ))
        .id();
    let tag = commands
        .spawn((
            Pickable::IGNORE,
            Text::new("MAP"),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                position_type: PositionType::Absolute,
                left: px(6),
                top: px(4),
                ..default()
            },
        ))
        .id();
    commands.entity(frame).add_child(tag);
    commands
        .entity(frame)
        .observe(on_minimap_press)
        .observe(on_minimap_drag)
        .observe(on_minimap_release)
        .observe(on_minimap_drag_end)
        .observe(on_minimap_cancel);

    let recenter = commands
        .spawn((
            MinimapRecenter,
            Button,
            Node {
                position_type: PositionType::Absolute,
                right: px(MINIMAP_RIGHT),
                top: px(MINIMAP_TOP + MINIMAP_SIZE + 6.0),
                height: px(24),
                padding: UiRect::horizontal(px(8)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.08, 0.11, 0.75)),
            BorderColor::all(Color::srgba(0.5, 0.7, 0.85, 0.25)),
            children![(
                Text::new("⌖ center"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();
    commands.entity(recenter).observe(on_minimap_recenter);

    commands.entity(root).add_children(&[frame, recenter]);
}

// ---------------------------------------------------------------------------
// Per-frame sync: visibility, viewport rect, projection, watchdog
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_minimap(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut minimap: ResMut<Minimap>,
    mut cameras: Query<(&mut Camera, &mut Transform, &mut Projection), With<MinimapCamera>>,
    mut roots: Query<&mut Node, With<MinimapUi>>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
) {
    let active = editor.mode == Mode::Play;
    if active && !minimap.ready {
        minimap.center = default_center(&editor);
        minimap.half_extent = fit_half_extent(&editor);
        minimap.ready = true;
    } else if !active {
        minimap.ready = false;
        minimap.press = None;
        minimap.handle = None;
        minimap.moved = 0.0;
    }
    // A release that misses the frame (pointer lifted off-rect) would wedge
    // the press forever; drop presses whose pointer is already gone.
    if let Some(press) = minimap.press {
        let gone = match press.pointer {
            PointerId::Mouse => !mouse.pressed(MouseButton::Left),
            PointerId::Touch(id) => !touches.iter().any(|t| t.id() == id),
            _ => false,
        };
        if gone {
            minimap.press = None;
            minimap.handle = None;
            minimap.moved = 0.0;
        }
    }

    let s = minimap_scale(&touch);
    let size = window.size();
    let (_origin, edge) = minimap_rect(size, s);
    let sf = window.scale_factor();
    let phys_edge = ((edge * sf).round().max(1.0)) as u32;
    let phys_left = (((size.x - (MINIMAP_RIGHT + MINIMAP_SIZE) * s) * sf)
        .round()
        .max(0.0)) as u32;
    let phys_top = (((MINIMAP_TOP * s) * sf).round().max(0.0)) as u32;
    for (mut camera, mut transform, mut projection) in &mut cameras {
        camera.is_active = active;
        camera.order = 1;
        camera.viewport = Some(Viewport {
            physical_position: UVec2::new(phys_left, phys_top),
            physical_size: UVec2::new(phys_edge, phys_edge),
            depth: 0.0..1.0,
        });
        if let Projection::Orthographic(ortho) = &mut *projection {
            ortho.scale = minimap.half_extent;
        }
        transform.translation = Vec3::new(minimap.center.x, MINIMAP_HEIGHT, minimap.center.y);
        transform.rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    }
    for mut node in &mut roots {
        node.display = if active { Display::Flex } else { Display::None };
    }
}

// ---------------------------------------------------------------------------
// Pointer observers: pan, zoom is in `camera_control`, click routes
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn on_minimap_press(
    event: On<PointerPress>,
    time: Res<Time>,
    cameras: Query<(&Camera, &GlobalTransform), With<MinimapCamera>>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut minimap: ResMut<Minimap>,
    mut touch: ResMut<TouchState>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
) {
    if event.button != PointerButton::Primary || editor.mode != Mode::Play {
        return;
    }
    if capture.grabbed || menu.open || focus.get().is_some() {
        return;
    }
    if minimap.press.is_some() {
        return;
    }
    let Ok((camera, global)) = cameras.single() else {
        return;
    };
    let pos = event.pointer.position;
    let Ok(ray) = camera.viewport_to_world(global, pos) else {
        return;
    };
    let _ = ray;
    let now = time.elapsed_secs_f64();
    let radius = if touch.enabled { 26.0 } else { 14.0 };
    // Waypoint and tangent dots reshape the route straight from the map.
    if let Some(id) = waypoint_hit(&editor, camera, global, pos, radius) {
        if let Some(plan) = editor.plan.as_mut() {
            plan.select(Some(id));
        }
        minimap.press = Some(MinimapPress {
            pointer: event.pointer.id,
        });
        minimap.handle = Some(PathHandle::Waypoint(id));
        minimap.moved = 0.0;
        touch.tap = None;
        touch.suppress_until = touch.suppress_until.max(now + 0.08);
        return;
    }
    if let Some(route) = editor.route.as_ref()
        && let Some((waypoint, side)) = tangent_hit(&editor, route, camera, global, pos, radius)
    {
        if let Some(plan) = editor.plan.as_mut() {
            plan.select(Some(waypoint));
        }
        minimap.press = Some(MinimapPress {
            pointer: event.pointer.id,
        });
        minimap.handle = Some(PathHandle::Tangent { waypoint, side });
        minimap.moved = 0.0;
        touch.tap = None;
        touch.suppress_until = touch.suppress_until.max(now + 0.08);
        return;
    }
    minimap.press = Some(MinimapPress {
        pointer: event.pointer.id,
    });
    minimap.handle = None;
    minimap.moved = 0.0;
    touch.tap = None;
    touch.suppress_until = touch.suppress_until.max(now + 0.08);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_minimap_drag(
    event: On<PointerDrag>,
    window: Single<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MinimapCamera>>,
    touch: Res<TouchState>,
    nav: Res<Nav>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    mut editor: ResMut<Editor>,
    mut minimap: ResMut<Minimap>,
) {
    let owned = minimap
        .press
        .is_some_and(|press| press.pointer == event.pointer.id);
    if !owned || editor.mode != Mode::Play {
        return;
    }
    minimap.moved += event.delta.length();
    let Ok((camera, global)) = cameras.single() else {
        return;
    };
    if let Some(handle) = minimap.handle {
        let Ok(rapier_ctx) = rapier.single() else {
            return;
        };
        let filter = player
            .iter()
            .next()
            .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
            .unwrap_or_default();
        apply_path_drag(
            &mut editor,
            PathDrag {
                pointer: event.pointer.id,
                handle,
            },
            event.pointer.position,
            camera,
            global,
            &nav,
            &rapier_ctx,
            filter,
        );
    } else {
        let s = minimap_scale(&touch);
        let edge = minimap_rect(window.size(), s).1;
        if edge > 0.0 {
            minimap.center = pan_center(minimap.center, event.delta, edge, minimap.half_extent);
        }
    }
}

/// Shared release path for `PointerRelease` and `PointerDragEnd`: a still
/// press with no handle routes exactly like a main-canvas Play click.
#[allow(clippy::too_many_arguments)]
fn finish_minimap_press(
    pointer: PointerId,
    screen: Vec2,
    cameras: &Query<(&Camera, &GlobalTransform), With<MinimapCamera>>,
    orbit: &Orbit,
    nav: &Nav,
    rapier: &RapierContext,
    filter: QueryFilter,
    keys: &ButtonInput<KeyCode>,
    touch: &TouchState,
    editor: &mut Editor,
    minimap: &mut Minimap,
) {
    let owned = minimap.press.is_some_and(|press| press.pointer == pointer);
    if !owned {
        return;
    }
    let handle = minimap.handle.take();
    let moved = minimap.moved;
    minimap.press = None;
    minimap.moved = 0.0;
    if handle.is_some() {
        // Handle drags commit live in `on_minimap_drag`.
        return;
    }
    if moved > path_edit::TAP_MAX_MOVE || editor.mode != Mode::Play {
        // It was a pan, not a tap.
        return;
    }
    if !click_routes(true, orbit.mode, editor.character.is_some()) {
        return;
    }
    let Ok((camera, global)) = cameras.single() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(global, screen) else {
        return;
    };
    let fallback = ray
        .plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
        .map(|point| Vec2::new(point.x, point.z))
        .unwrap_or(minimap.center);
    let (target, height) = minimap_surface(ray, rapier, filter, fallback);
    let cell = resolve_cell(&editor.pieces, target, height);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let append = append_for(shift, touch.append);
    add_waypoint(
        editor, target, cell, height, append, true, nav, rapier, filter,
    );
    editor.selection.clear();
    editor.drag = None;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_minimap_release(
    event: On<PointerRelease>,
    cameras: Query<(&Camera, &GlobalTransform), With<MinimapCamera>>,
    orbit: Res<Orbit>,
    nav: Res<Nav>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    keys: Res<ButtonInput<KeyCode>>,
    touch: Res<TouchState>,
    mut editor: ResMut<Editor>,
    mut minimap: ResMut<Minimap>,
) {
    let Ok(rapier_ctx) = rapier.single() else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    finish_minimap_press(
        event.pointer.id,
        event.pointer.position,
        &cameras,
        &orbit,
        &nav,
        &rapier_ctx,
        filter,
        &keys,
        &touch,
        &mut editor,
        &mut minimap,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_minimap_drag_end(
    event: On<PointerDragEnd>,
    cameras: Query<(&Camera, &GlobalTransform), With<MinimapCamera>>,
    orbit: Res<Orbit>,
    nav: Res<Nav>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    keys: Res<ButtonInput<KeyCode>>,
    touch: Res<TouchState>,
    mut editor: ResMut<Editor>,
    mut minimap: ResMut<Minimap>,
) {
    let Ok(rapier_ctx) = rapier.single() else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    finish_minimap_press(
        event.pointer.id,
        event.pointer.position,
        &cameras,
        &orbit,
        &nav,
        &rapier_ctx,
        filter,
        &keys,
        &touch,
        &mut editor,
        &mut minimap,
    );
}

pub(super) fn on_minimap_cancel(event: On<PointerCancel>, mut minimap: ResMut<Minimap>) {
    if minimap
        .press
        .is_some_and(|press| press.pointer == event.pointer.id)
    {
        minimap.press = None;
        minimap.handle = None;
        minimap.moved = 0.0;
    }
}

pub(super) fn on_minimap_recenter(
    event: On<Activate>,
    buttons: Query<&MinimapRecenter>,
    time: Res<Time>,
    editor: Res<Editor>,
    mut minimap: ResMut<Minimap>,
    mut touch: ResMut<TouchState>,
) {
    if buttons.get(event.entity).is_err() || editor.mode != Mode::Play {
        return;
    }
    let now = time.elapsed_secs_f64();
    // A UI tap is not a grid tap: drop any tap from this gesture.
    touch.suppress_until = touch.suppress_until.max(now + 0.08);
    touch.tap = None;
    minimap.center = default_center(&editor);
    minimap.half_extent = fit_half_extent(&editor);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with_level() -> Editor {
        let mut editor = Editor::default();
        for pos in [Vec2::ZERO, Vec2::new(6.0, 0.0), Vec2::new(0.0, 8.0)] {
            editor.pieces.push(Piece {
                id: editor.next_id,
                shape: 0,
                pos,
                rot: 0.0,
                pitch: 0.0,
                roll: 0.0,
                elevation: 0.0,
                dynamic: false,
                trigger: crate::model::Trigger::harmless(),
                trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
                trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                interact: crate::model::InteractAction::None,
            });
            editor.next_id += 1;
        }
        editor
    }

    #[test]
    fn the_minimap_rect_sits_in_the_top_right() {
        let size = Vec2::new(800.0, 600.0);
        let (origin, edge) = minimap_rect(size, 1.0);
        assert_eq!(edge, MINIMAP_SIZE);
        assert_eq!(origin, Vec2::new(800.0 - 228.0, 50.0));
        // Touch UI scale shrinks the rect into logical space.
        let (small_origin, small_edge) = minimap_rect(size, 0.5);
        assert_eq!(small_edge, MINIMAP_SIZE * 0.5);
        assert_eq!(small_origin, Vec2::new(800.0 - 228.0 * 0.5, 25.0));
    }

    #[test]
    fn minimap_hit_covers_the_frame_only() {
        let size = Vec2::new(800.0, 600.0);
        assert!(minimap_hit(Vec2::new(700.0, 100.0), size, 1.0));
        assert!(minimap_hit(Vec2::new(572.0, 50.0), size, 1.0), "edge");
        assert!(!minimap_hit(Vec2::new(571.0, 100.0), size, 1.0), "left");
        assert!(!minimap_hit(Vec2::new(400.0, 300.0), size, 1.0), "canvas");
    }

    #[test]
    fn world_map_round_trips_with_north_up() {
        let center = Vec2::new(3.0, 4.0);
        let origin = Vec2::new(572.0, 50.0);
        let edge = 220.0;
        let middle = world_to_map(center, center, 10.0, origin, edge);
        assert_eq!(middle, origin + Vec2::splat(110.0));
        // Smaller world z (north) maps higher on screen (smaller y).
        let north = world_to_map(center + Vec2::new(0.0, -10.0), center, 10.0, origin, edge);
        assert_eq!(north, origin + Vec2::new(110.0, 0.0));
        let back = map_to_world(north, center, 10.0, origin, edge);
        assert!((back - (center + Vec2::new(0.0, -10.0))).length() < 1e-4);
    }

    #[test]
    fn dragging_moves_the_view_with_the_pointer() {
        let center = Vec2::ZERO;
        // Drag right-down: the center moves left-up so content follows.
        let moved = pan_center(center, Vec2::new(22.0, 11.0), 220.0, 10.0);
        assert!((moved - Vec2::new(-2.0, -1.0)).length() < 1e-4);
    }

    #[test]
    fn the_minimap_opens_on_the_character_and_fits_the_level() {
        let mut editor = editor_with_level();
        // No character: centroid of (0,0), (6,0), (0,8).
        assert_eq!(default_center(&editor), Vec2::new(2.0, 8.0 / 3.0));
        // Span 8 + margin, clamped into range.
        assert_eq!(fit_half_extent(&editor), 7.0);
        editor.character = Some(Character {
            pos: Vec2::new(20.0, 20.0),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        assert_eq!(default_center(&editor), Vec2::new(20.0, 20.0));
        // Span 20 + margin, still under the max.
        assert_eq!(fit_half_extent(&editor), 13.0);
    }
}
