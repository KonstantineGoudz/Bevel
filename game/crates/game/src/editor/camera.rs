//! camera systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// How long a web pointer-lock request may take before it counts as refused.
pub(super) const LOCK_GRACE: f32 = 0.5;

/// Whether the pointer is over the 3D view rather than the desktop palette.
/// Play hides the palette, so the whole screen is viewport there.
pub(super) fn over_viewport(editor: &Editor, touch: &TouchState, cursor: Option<Vec2>) -> bool {
    if touch.enabled {
        return false;
    }
    let Some(cursor) = cursor else {
        return false;
    };
    !object_editing_allowed(editor.mode) || cursor.x > PANEL_WIDTH
}

#[cfg(target_arch = "wasm32")]
pub(super) fn pointer_is_locked() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|document| document.pointer_lock_element().is_some())
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn pointer_is_locked() -> bool {
    true
}

/// Grab the pointer on entering first person, let go on leaving it or when
/// the window loses focus, and let Tab toggle the grab without changing view.
#[allow(clippy::too_many_arguments)]
pub(super) fn manage_capture(
    mut capture: ResMut<PointerCapture>,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    input: PlayerInput,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut focus: MessageReader<WindowFocused>,
    input_focus: Res<InputFocus>,
    time: Res<Time>,
) {
    let was_grabbed = capture.grabbed;
    let focus_lost = focus.read().any(|event| !event.focused);
    let first_person =
        orbit.mode == ViewMode::First && editor.character.is_some() && !touch.enabled;
    let input = CaptureInput {
        mode: editor.mode,
        view: orbit.mode,
        first_person,
        over_viewport: over_viewport(&editor, &touch, window.cursor_position()),
        tab: input.just_pressed(Action::CaptureToggle)
            && editor.keyboard_transform.is_none()
            && input_focus.get().is_none(),
        focus_lost,
    };
    let mut grabbed = next_capture(capture.grabbed, capture.last_mode, capture.last_view, input);

    // On the web the browser can drop the lock by itself (Esc) and can refuse
    // a request without telling winit. Believe the document, after a short
    // grace period for a fresh request to settle.
    if capture.applied {
        let now = time.elapsed_secs();
        if !grabbed || pointer_is_locked() {
            capture.unlocked_since = None;
        } else {
            let since = *capture.unlocked_since.get_or_insert(now);
            if now - since > LOCK_GRACE {
                grabbed = false;
                capture.unlocked_since = None;
            }
        }
    }

    if grabbed && !was_grabbed {
        // Looking owns the mouse: drop any object drag that was in flight and
        // the placement ghost, which is not meaningful without a cursor.
        editor.drag = None;
        editor.gizmo_drag = None;
        editor.ghost = None;
    }
    capture.grabbed = grabbed;
    capture.last_mode = editor.mode;
    capture.last_view = orbit.mode;

    if capture.applied != grabbed {
        cursor_options.grab_mode = if grabbed {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        cursor_options.visible = !grabbed;
        capture.applied = grabbed;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn camera_control(
    mut orbit: ResMut<Orbit>,
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<MinimapCamera>)>,
    mut editor: ResMut<Editor>,
    gesture: Res<PathGesture>,
    mut touch: ResMut<TouchState>,
    input: PlayerInput,
    time: Res<Time>,
    capture: Res<PointerCapture>,
    mut minimap: ResMut<Minimap>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    window: Single<&Window, With<PrimaryWindow>>,
    input_focus: Res<InputFocus>,
    menu: Res<AddMenu>,
    outliner: Res<OutlinerState>,
    panel_state: Res<PropertiesPanelState>,
) {
    // The modal transform, a focused property field, or a pointer over the
    // property panel owns the mouse. Drain the deltas so the camera does not
    // jump when editing ends.
    let over_properties = !editor.selection.is_empty()
        && window.cursor_position().is_some_and(|p| {
            over_properties_panel(
                p,
                window.size(),
                !editor.selection.is_empty(),
                panel_state.collapsed,
                &touch,
            )
        });
    let over_outliner = !capture.grabbed
        && editor.mode == Mode::Build
        && window
            .cursor_position()
            .is_some_and(|p| over_outliner_panel(p, window.size(), outliner.collapsed, &touch));
    if editor.keyboard_transform.is_some()
        || input_focus.get().is_some()
        || over_properties
        || over_outliner
    {
        motion.read().for_each(drop);
        wheel.read().for_each(drop);
        return;
    }
    // Touch browsers also synthesize mouse motion for a finger drag. While a
    // finger is down, that synthesized motion must not also steer the camera:
    // dragging the stick should move the character only.
    let raw_delta: Vec2 = motion.read().map(|m| m.delta).sum();
    // A held gizmo handle owns the gesture just like a path-handle drag:
    // the orbit must not swing while a finger reshapes the selection.
    let gizmo_held = editor
        .gizmo_drag
        .as_ref()
        .is_some_and(|drag| drag.pointer != PointerId::Mouse);
    let touching = touch.enabled
        && (touch.stick_pointer.is_some()
            || !touch.tracks.is_empty()
            || gesture.0.dragging()
            || gizmo_held);
    let delta = if touching { Vec2::ZERO } else { raw_delta };
    // Browsers report wheel deltas in pixels (often +-100), while some
    // platforms report whole lines. Normalise the big values to lines.
    let raw_scroll: f32 = wheel.read().map(|w| w.y).sum();
    let scroll = if raw_scroll.abs() > 10.0 {
        raw_scroll / 120.0
    } else {
        raw_scroll
    };
    let over_panel = object_editing_allowed(editor.mode)
        && window.cursor_position().is_some_and(|p| p.x <= PANEL_WIDTH);

    // The Add menu owns single-letter shortcuts while open: typing "v" or "f"
    // in the search must not cycle the view or refocus it. (Chords are
    // modifier-exact, so remapped Ctrl-combos stay disjoint regardless.)
    if !menu.open {
        if input.just_pressed(Action::CycleView) {
            cycle_view(&mut orbit, &editor);
        }
        if input.just_pressed(Action::FocusCamera) {
            focus(&mut orbit, &editor);
        }
    }

    if orbit.mode == ViewMode::First && editor.character.is_some() {
        // First person looks only while the pointer is captured; Tab lets go
        // so the palette can be used without leaving the view.
        if capture.grabbed {
            orbit.yaw -= delta.x * 0.005;
            orbit.pitch = (orbit.pitch - delta.y * 0.005).clamp(-1.35, 1.35);
            if let Some(character) = editor.character.as_mut() {
                character.yaw = orbit.yaw;
            }
        }
    } else if !over_panel {
        if input.pressed(Action::OrbitDrag) {
            // In Build an orbit pivots around the selected piece, so
            // the thing being edited stays in the middle of the view.
            if object_editing_allowed(editor.mode)
                && let Some(center) = selection_center(&editor)
            {
                orbit.target = center;
            }
            orbit.yaw -= delta.x * 0.006;
            let (low, high) = if orbit.mode == ViewMode::Top {
                (0.7, 1.5)
            } else {
                (0.12, 1.5)
            };
            orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(low, high);
        }
        if input.pressed(Action::PanDrag) {
            let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
            let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
            let k = orbit.distance * 0.0018;
            orbit.target -= right * delta.x * k;
            orbit.target += forward * delta.y * k;
            orbit.target.y = 0.0;
        }
        if scroll != 0.0 {
            // The wheel over the Play minimap zooms the map, not the main
            // view; everywhere else it dollies the orbit camera.
            let over_minimap = editor.mode == Mode::Play
                && window
                    .cursor_position()
                    .is_some_and(|p| minimap_hit(p, window.size(), minimap_scale(&touch)));
            if over_minimap {
                minimap.half_extent = (minimap.half_extent * (1.0 + scroll * 0.12))
                    .clamp(MINIMAP_MIN_HALF, MINIMAP_MAX_HALF);
            } else {
                orbit.distance = (orbit.distance * (1.0 - scroll * 0.12)).clamp(1.5, 60.0);
            }
        }
    }

    // Touch: one finger looks/orbits, two fingers pan and pinch. A path
    // handle drag owns the gesture, so the camera keeps still until release.
    // A held gizmo handle owns it the same way (see `gizmo_held` above).
    if gesture.0.dragging() || gizmo_held {
        touch.look = Vec2::ZERO;
        touch.pan = Vec2::ZERO;
        touch.zoom = 0.0;
    } else if touch.enabled {
        match orbit.mode {
            ViewMode::First if editor.character.is_some() => {
                orbit.yaw -= touch.look.x * 0.005;
                orbit.pitch = (orbit.pitch - touch.look.y * 0.005).clamp(-1.35, 1.35);
            }
            _ => {
                if touch.look != Vec2::ZERO
                    && object_editing_allowed(editor.mode)
                    && let Some(center) = selection_center(&editor)
                {
                    orbit.target = center;
                }
                orbit.yaw -= touch.look.x * 0.006;
                let (low, high) = if orbit.mode == ViewMode::Top {
                    (0.7, 1.5)
                } else {
                    (0.12, 1.5)
                };
                orbit.pitch = (orbit.pitch + touch.look.y * 0.005).clamp(low, high);
                if touch.pan != Vec2::ZERO {
                    let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
                    let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
                    let k = orbit.distance * 0.0018;
                    orbit.target -= right * touch.pan.x * k;
                    orbit.target += forward * touch.pan.y * k;
                    orbit.target.y = 0.0;
                }
                if touch.zoom != 0.0 {
                    orbit.distance = (orbit.distance * (1.0 - touch.zoom * 0.8)).clamp(1.5, 60.0);
                }
            }
        }
    }

    // Gamepad right stick orbits (or looks, in first person). Standard fixed
    // layout; the stick itself is not remappable, only the buttons are.
    if !menu.open && input_focus.get().is_none() {
        let stick = input.camera_stick();
        if stick != Vec2::ZERO {
            let dt = time.delta_secs().clamp(0.0, 0.25);
            if orbit.mode == ViewMode::First && editor.character.is_some() {
                orbit.yaw -= stick.x * 2.5 * dt;
                orbit.pitch = (orbit.pitch - stick.y * 1.8 * dt).clamp(-1.35, 1.35);
                if let Some(character) = editor.character.as_mut() {
                    character.yaw = orbit.yaw;
                }
            } else {
                orbit.yaw -= stick.x * 2.5 * dt;
                let (low, high) = if orbit.mode == ViewMode::Top {
                    (0.7, 1.5)
                } else {
                    (0.12, 1.5)
                };
                orbit.pitch = (orbit.pitch - stick.y * 1.8 * dt).clamp(low, high);
            }
        }
    }

    if orbit.mode == ViewMode::First {
        if let Some(character) = editor.character {
            let eye = Vec3::new(
                character.pos.x,
                character.feet
                    + if character.sliding {
                        EYE_HEIGHT * SLIDE_SCALE_Y
                    } else {
                        EYE_HEIGHT
                    },
                character.pos.y,
            );
            let look = Vec3::new(
                -orbit.yaw.sin() * orbit.pitch.cos(),
                orbit.pitch.sin(),
                -orbit.yaw.cos() * orbit.pitch.cos(),
            );
            camera.translation = eye;
            camera.look_at(eye + look, Vec3::Y);
            return;
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

/// The centre of the selection, for orbiting in Build. With several pieces
/// selected this is their centroid, so the camera frames the whole group.
pub(super) fn selection_center(editor: &Editor) -> Option<Vec3> {
    let mut sum = Vec3::ZERO;
    let mut count = 0.0;
    for id in editor.selection.ids() {
        let Some(piece) = editor.pieces.iter().find(|piece| piece.id == *id) else {
            continue;
        };
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        let (low, high) = outline_bounds(&model::shape_points(shape));
        let local = Vec3::new(
            (low.x + high.x) * 0.5,
            (shape.base + shape.top()) * 0.5,
            (low.y + high.y) * 0.5,
        );
        let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
        sum += root + model::piece_quat(piece) * local;
        count += 1.0;
    }
    (count > 0.0).then(|| sum / count)
}

/// Put the orbit centre back on the selection, or on the character's feet when
/// nothing is selected.
pub(super) fn focus(orbit: &mut Orbit, editor: &Editor) {
    if let Some(center) = selection_center(editor) {
        orbit.target = center;
    } else {
        focus_character(orbit, editor.character);
    }
}

/// Put the orbit centre on the character's feet.
pub(super) fn focus_character(orbit: &mut Orbit, character: Option<Character>) {
    if let Some(character) = character {
        orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
    }
}

/// Third -> First -> Top. Shared by the V key and the touch View button.
pub(super) fn cycle_view(orbit: &mut Orbit, editor: &Editor) {
    orbit.mode = orbit.mode.next();
    match orbit.mode {
        ViewMode::Third => {
            orbit.pitch = 0.9;
            orbit.distance = 7.5;
        }
        ViewMode::Top => {
            orbit.pitch = 1.4;
            orbit.distance = 13.0;
        }
        ViewMode::First => {
            orbit.pitch = 0.0;
            if let (Some(character), Some(route)) = (editor.character, editor.route.as_ref()) {
                if let Some(target) = route.line.last().or_else(|| route.trail.last()) {
                    let towards = Vec2::new(target.x, target.z) - character.pos;
                    if towards.length_squared() > 0.01 {
                        orbit.yaw = (-towards.x).atan2(-towards.y);
                    }
                }
            } else if let Some(character) = editor.character {
                orbit.yaw = character.yaw;
            }
        }
    }
    if orbit.mode != ViewMode::First {
        focus(orbit, editor);
    }
}

pub(super) fn update_cursor(
    mut cursor: ResMut<Cursor>,
    touch: Res<TouchState>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform), Without<MinimapCamera>>,
    focus: Res<InputFocus>,
) {
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    cursor.screen = touch.tap.or_else(|| window.cursor_position());
    cursor.screen_size = window.size();
    cursor.text_focus = focus.get().is_some();
    cursor.tap = touch.tap.is_some();
    cursor.ray = cursor
        .screen
        .and_then(|p| camera.viewport_to_world(&global, p).ok());
    cursor.world = cursor.ray.and_then(|ray| {
        ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            .map(|p| Vec2::new(p.x, p.z))
    });
    if let Some(world) = cursor.world {
        cursor.last_world = Some(world);
    }
}
