//! touch systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Touch
// ---------------------------------------------------------------------------

/// Turn raw touches into a stick vector, look/pan/zoom deltas, and taps.
pub(super) fn touch_input(
    touches: Res<Touches>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut state: ResMut<TouchState>,
) {
    state.look = Vec2::ZERO;
    state.pan = Vec2::ZERO;
    state.zoom = 0.0;
    state.tap = None;
    let now = time.elapsed_secs_f64();

    // Phones start in touch mode before the first tap; a narrow window on a
    // desktop gets the compact layout too, which is the right trade.
    if !state.enabled
        && (touches.any_just_pressed()
            || touches.iter().next().is_some()
            || window.width() * window.scale_factor() < 700.0)
    {
        state.enabled = true;
    }
    if !state.enabled {
        return;
    }

    let size = window.size();
    let s = state.ui_scale;
    let centre = stick_centre(size, s);

    // Presses are handled separately: on a slow frame the press and release can
    // arrive together and never appear in `iter()`.
    for touch in touches.iter_just_pressed() {
        let id = touch.id();
        let pos = touch.position();
        // The stick sits inside the bottom-left control zone, so decide the
        // stick first: a press on the pad is a grip, not a camera gesture. The
        // grab radius is deliberately larger than the drawn pad so DPR and
        // UiScale rounding cannot put the thumb just outside it.
        let stick = grabs_stick(pos, centre, STICK_REACH * s * 1.7);
        let control = !stick && over_controls(pos, size, s);
        state.tracks.insert(
            id,
            TouchTrack {
                last: pos,
                moved: 0.0,
                time: now,
                stick,
                control,
            },
        );
    }

    // A second finger anywhere (canvas, stick or a control) makes this
    // gesture multi-touch, so its release can never be a placement tap.
    if state.tracks.len() > 1 {
        state.multi = true;
    }

    for touch in touches.iter() {
        if touches.just_pressed(touch.id()) {
            continue;
        }
        let id = touch.id();
        let pos = touch.position();
        if let Some(track) = state.tracks.get_mut(&id) {
            let delta = pos - track.last;
            track.last = pos;
            track.moved += delta.length();
            if track.stick {
                if state.stick_pointer.is_none() {
                    state.stick = stick_offset(pos, centre, s);
                }
            } else if !track.control {
                state.look += delta;
            }
        }
    }

    for touch in touches.iter_just_released() {
        let id = touch.id();
        let pos = touch.position();
        if let Some(track) = state.tracks.remove(&id) {
            if track.stick && state.stick_pointer.is_none() {
                state.stick = Vec2::ZERO;
            }
            // A short, still press away from every control is a canvas tap.
            // This comes from the raw touch stream, so it works over bare
            // ground where no pickable entity exists to generate a pointer
            // event.
            if !track.stick
                && !track.control
                && !state.multi
                && track.moved <= path_edit::TAP_MAX_MOVE
                && now - track.time <= TOUCH_TAP_MAX_TIME
                && tap_allowed(now, &state)
            {
                state.tap = Some(pos);
            }
        }
    }

    for touch in touches.iter_just_canceled() {
        if let Some(track) = state.tracks.remove(&touch.id()) {
            if track.stick && state.stick_pointer.is_none() {
                state.stick = Vec2::ZERO;
            }
        }
    }

    // A picked stick drives itself from pointer events in the UI space; raw
    // touch deltas must not also rotate the camera from the same finger.
    if state.stick_pointer.is_some() {
        state.look = Vec2::ZERO;
        state.pan = Vec2::ZERO;
        state.zoom = 0.0;
    }
    if touches.iter().next().is_none() && state.stick_pointer != Some(PointerId::Mouse) {
        // Release can be missed if the pointer leaves the canvas.
        state.stick_pointer = None;
        state.stick = Vec2::ZERO;
    }

    // CDP touch emulation (and some browsers) deliver taps as mouse clicks.
    // Route those through the same hit-test so one path serves both.
    let fingers: Vec<Vec2> = state
        .tracks
        .values()
        .filter(|track| !track.stick && !track.control)
        .map(|track| track.last)
        .collect();

    if fingers.len() >= 2 {
        let centroid = (fingers[0] + fingers[1]) * 0.5;
        let span = fingers[0].distance(fingers[1]);
        if let Some((last_centroid, last_span)) = state.two_finger {
            state.pan += centroid - last_centroid;
            if last_span > 1.0 {
                state.zoom += (span - last_span) / last_span;
            }
            // While pinching, do not also rotate the view.
            state.look = Vec2::ZERO;
        }
        state.two_finger = Some((centroid, span));
    } else {
        state.two_finger = None;
    }

    if state.tracks.is_empty() && touches.iter().next().is_none() {
        state.multi = false;
    }
}

/// A press inside the stick's reach grabs it, even though the pad also lies in
/// the bottom-left control zone.
pub(super) fn grabs_stick(pos: Vec2, centre: Vec2, reach: f32) -> bool {
    pos.distance(centre) <= reach
}

/// Stick deflection: screen right is x, screen up is +y, clamped to 1.
#[allow(dead_code)] // exercised by tests; the hot path inlines it
pub(super) fn stick_offset(pos: Vec2, centre: Vec2, scale: f32) -> Vec2 {
    let offset = (pos - centre) / (STICK_TRAVEL * scale);
    Vec2::new(offset.x, -offset.y).clamp_length_max(1.0)
}

/// Screen areas owned by touch controls (toolbar + carousel, stick, jump).
pub(super) fn over_controls(pos: Vec2, size: Vec2, scale: f32) -> bool {
    pos.y <= 128.0 * scale
        || (pos.y >= size.y - 170.0 * scale && pos.x <= 190.0 * scale)
        || (pos.y >= size.y - 170.0 * scale && pos.x >= size.x - 120.0 * scale)
}

pub(super) fn sync_input_mode(
    touch: Res<TouchState>,
    mut layers: Query<(&InputLayer, &mut Node), Without<ModeUi>>,
) {
    if !touch.is_changed() {
        return;
    }
    for (layer, mut node) in &mut layers {
        let visible = layer.desktop != touch.enabled;
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Show the build overlays in Build and the play overlays in Play, for the
/// input layer the player is actually using.
pub(super) fn sync_mode_ui(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    mut nodes: Query<(&ModeUi, &mut Node)>,
) {
    for (ui, mut node) in &mut nodes {
        let show = layer_matches(*ui, editor.mode, touch.enabled);
        node.display = if show { Display::Flex } else { Display::None };
    }
}

pub(super) fn sync_touch_path_row(
    editor: Res<Editor>,
    mut row: Single<&mut Node, With<TouchPathRow>>,
    // The row and its buttons both own `Node`; the filter makes the two
    // queries disjoint so Bevy's access checks accept them.
    mut buttons: Query<(&TouchButton, &mut Node), Without<TouchPathRow>>,
) {
    let plan = editor.actor_tool == Some(ActorTool::Plan);
    let path = editor.actor_tool == Some(ActorTool::Path);
    let show = plan || path;
    row.display = if show { Display::Flex } else { Display::None };
    for (button, mut node) in &mut buttons {
        let visible = match button.0 {
            TouchAction::Add => show,
            TouchAction::Walk | TouchAction::Smooth => plan,
            _ => continue,
        };
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

pub(super) fn sync_knob(touch: Res<TouchState>, mut knob: Single<&mut Node, With<TouchKnob>>) {
    if !touch.is_changed() {
        return;
    }
    let centre = (STICK_BASE - STICK_KNOB) * 0.5;
    knob.left = px(centre + touch.stick.x * STICK_TRAVEL);
    knob.bottom = px(centre + touch.stick.y * STICK_TRAVEL);
}

pub(super) fn sync_shape_label(
    editor: Res<Editor>,
    mut label: Single<&mut Text, With<ShapeLabel>>,
) {
    if !editor.is_changed() {
        return;
    }
    label.0 = shapes()[editor.mobile_shape].name.to_string();
}

pub(super) fn update_touch_buttons(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    mut buttons: Query<(&TouchButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = match button.0 {
            TouchAction::Play => false,
            TouchAction::Path => editor.actor_tool == Some(ActorTool::Path),
            TouchAction::Plan => editor.actor_tool == Some(ActorTool::Plan),
            TouchAction::Add => touch.append,
            TouchAction::Walk => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && walk_enabled(
                        editor.plan.as_ref(),
                        editor.route.as_ref(),
                        editor.walk.is_some(),
                    )
            }
            TouchAction::Smooth => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && editor.plan.as_ref().is_some_and(|plan| !plan.is_empty())
            }
            TouchAction::Place => editor.tool == Some(editor.mobile_shape),
            _ => false,
        };
        bg.0 = if on {
            BUTTON_ON
        } else {
            Color::srgba(0.06, 0.08, 0.11, 0.62)
        };
        *border = BorderColor::all(if on {
            BORDER_ON
        } else {
            Color::srgba(0.5, 0.7, 0.85, 0.25)
        });
    }
}

pub(super) fn update_touch_status(
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    mut text: Single<&mut Text, With<TouchStatus>>,
) {
    if !touch.enabled {
        return;
    }
    let height = editor.character.map(|c| c.feet).unwrap_or(0.0);
    let tool = match editor.actor_tool {
        Some(ActorTool::Path) => "path",
        Some(ActorTool::Plan) => "plan",
        Some(ActorTool::Place) => "char",
        None => editor
            .tool
            .map(|shape| shapes()[shape].name)
            .unwrap_or("select"),
    };
    let plan = editor
        .plan
        .as_ref()
        .map(|plan| format!("  {}wp", plan.waypoints.len()))
        .unwrap_or_default();
    text.0 = format!(
        "{}  {}  y{:.2}  {}p  {}{}{}",
        editor.mode.label(),
        orbit.mode.label(),
        height,
        editor.pieces.len(),
        tool,
        plan,
        if touch.append { "  add" } else { "" }
    );
}

/// Which touch button (if any) covers a point, in CSS pixels. The UI is drawn
/// with a 1/DPR UiScale on phones, so on screen these are the design sizes.
#[allow(dead_code)] // superseded by UI activation; kept for the hit-test test
pub(super) fn touch_action_at(pos: Vec2, css: Vec2) -> Option<TouchAction> {
    if (8.0..=42.0).contains(&pos.y) {
        let width = (css.x - 20.0) / 9.0;
        let index = ((pos.x - 8.0) / (width + 4.0)).floor().max(0.0) as usize;
        return [
            TouchAction::View,
            TouchAction::Rotate,
            TouchAction::Delete,
            TouchAction::Undo,
            TouchAction::Redo,
            TouchAction::Path,
            TouchAction::Plan,
            TouchAction::Course,
            TouchAction::Play,
        ]
        .get(index.min(8))
        .copied();
    }
    if (46.0..=84.0).contains(&pos.y) {
        let quarter = (css.x - 20.0) / 4.0;
        let index = ((pos.x - 8.0) / (quarter + 4.0)).floor().max(0.0) as usize;
        return match index.min(3) {
            0 => Some(TouchAction::PrevShape),
            1 => None,
            2 => Some(TouchAction::NextShape),
            _ => Some(TouchAction::Place),
        };
    }
    if (88.0..=122.0).contains(&pos.y) {
        let third = (css.x - 20.0) / 3.0;
        let index = ((pos.x - 8.0) / (third + 4.0)).floor().max(0.0) as usize;
        return [TouchAction::Add, TouchAction::Walk, TouchAction::Smooth]
            .get(index.min(2))
            .copied();
    }
    let jump = Vec2::new(css.x - 62.0, css.y - 66.0);
    if pos.distance(jump) < 52.0 {
        return Some(TouchAction::Jump);
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_touch_action(
    action: TouchAction,
    editor: &mut Editor,
    orbit: &mut Orbit,
    touch: &mut TouchState,
    clipboard: &mut Clipboard,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) {
    match action {
        TouchAction::Play => set_mode(editor, editor.mode.toggled()),
        TouchAction::View => cycle_view(orbit, editor),
        TouchAction::Rotate => rotate_piece(editor, model::ANCHOR_STEP),
        TouchAction::Delete => {
            let waypoint_selected = editor.actor_tool == Some(ActorTool::Plan)
                && editor
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.selected.is_some());
            if waypoint_selected {
                delete_waypoint(editor, nav, spatial_query, filter);
            } else {
                delete_piece(editor);
            }
        }
        TouchAction::Undo => editor.undo(),
        TouchAction::Redo => editor.redo(),
        TouchAction::Path => {
            editor.tool = None;
            // Toggle, like the palette button, so a second tap leaves path
            // mode. The active state is shown on the button and in the status.
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Path) {
                None
            } else {
                Some(ActorTool::Path)
            };
            editor.selected = None;
            editor.drag = None;
        }
        TouchAction::Plan => {
            let entering = editor.actor_tool != Some(ActorTool::Plan);
            editor.actor_tool = if entering {
                Some(ActorTool::Plan)
            } else {
                None
            };
            editor.tool = None;
            editor.selected = None;
            editor.drag = None;
            editor.ghost = None;
            if entering {
                orbit.mode = ViewMode::Top;
                orbit.pitch = 1.4;
                orbit.distance = 13.0;
                focus_character(orbit, editor.character);
            }
        }
        TouchAction::Add => touch.append = !touch.append,
        TouchAction::Walk => {
            if editor.actor_tool == Some(ActorTool::Plan) {
                walk_from_plan(editor, nav, spatial_query, filter);
            }
        }
        TouchAction::Smooth => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && smooth_plan(editor, nav, spatial_query, filter)
            {
                editor.note = "smoothed";
            }
        }
        TouchAction::Place => {
            editor.tool = if editor.tool == Some(editor.mobile_shape) {
                None
            } else {
                Some(editor.mobile_shape)
            };
            editor.actor_tool = None;
            editor.selected = None;
            editor.drag = None;
        }
        TouchAction::PrevShape | TouchAction::NextShape => {
            let count = shapes().len();
            let delta = if action == TouchAction::NextShape {
                1
            } else {
                count - 1
            };
            editor.mobile_shape = (editor.mobile_shape + delta) % count;
            if editor.tool.is_some() {
                editor.tool = Some(editor.mobile_shape);
            }
        }
        TouchAction::Jump => touch.jump = true,
        TouchAction::Course => {
            editor.load_course();
            focus_character(orbit, editor.character);
        }
        TouchAction::CopyJson => copy_level_json(editor, clipboard),
    }
}

/// Centre of the stick pad in the picking/UI coordinate space.
pub(super) fn stick_centre(size: Vec2, scale: f32) -> Vec2 {
    Vec2::new(
        (STICK_MARGIN + STICK_BASE * 0.5) * scale,
        size.y - (STICK_MARGIN + STICK_BASE * 0.5) * scale,
    )
}

pub(super) fn on_canvas_press(
    event: On<PointerPress>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut touch: ResMut<TouchState>,
) {
    if !touch.enabled || event.button != PointerButton::Primary {
        return;
    }
    let pos = event.pointer.position;
    let centre = stick_centre(window.size(), touch.ui_scale);
    if grabs_stick(pos, centre, STICK_REACH * touch.ui_scale * 1.7) {
        touch.stick_pointer = Some(event.pointer.id);
        touch.stick = stick_offset(pos, centre, touch.ui_scale);
        touch.tap = None;
    }
}

pub(super) fn on_canvas_move(
    event: On<PointerMove>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut touch: ResMut<TouchState>,
) {
    if touch.stick_pointer != Some(event.pointer.id) {
        return;
    }
    let centre = stick_centre(window.size(), touch.ui_scale);
    touch.stick = stick_offset(event.pointer.position, centre, touch.ui_scale);
}

pub(super) fn on_canvas_release(event: On<PointerRelease>, mut touch: ResMut<TouchState>) {
    if touch.stick_pointer == Some(event.pointer.id) {
        touch.stick_pointer = None;
        touch.stick = Vec2::ZERO;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_path_press(
    event: On<PointerPress>,
    window: Single<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    touches: Res<Touches>,
    camera: Single<(&Camera, &Transform)>,
    orbit: Res<Orbit>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    if event.button != PointerButton::Primary || capture.grabbed {
        return;
    }
    if editor.keyboard_transform.is_some() {
        return;
    }
    if touches.iter().count() > 1 {
        touch.multi = true;
    }
    let pos = event.pointer.position;
    if touch.enabled {
        let size = window.size();
        let centre = stick_centre(size, touch.ui_scale);
        if grabs_stick(pos, centre, STICK_REACH * touch.ui_scale * 1.7)
            || over_controls(pos, size, touch.ui_scale)
        {
            return;
        }
    }

    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    let handle = path_handle_at(&editor, camera, &global, pos, touch.enabled, &orbit);
    let now = time.elapsed_secs_f64();
    let drag = gesture.0.press(event.pointer.id, pos, now, handle);
    if let Some(drag) = drag {
        let waypoint = match drag.handle {
            PathHandle::Waypoint(id) => id,
            PathHandle::Tangent { waypoint, .. } => waypoint,
        };
        if let Some(plan) = editor.plan.as_mut() {
            plan.select(Some(waypoint));
        }
        touch.path_drag_active = true;
        touch.suppress_until = now + 0.08;
        touch.tap = None;
    }
}

pub(super) fn on_path_move(event: On<PointerMove>, mut gesture: ResMut<PathGesture>) {
    // Tap candidates track movement here; an active handle drag uses
    // PointerDrag so it keeps receiving updates over UI nodes.
    if !gesture.0.dragging() {
        gesture
            .0
            .drag(event.pointer.id, event.pointer.position, event.delta);
    }
}

pub(super) fn on_path_drag(event: On<PointerDrag>, mut gesture: ResMut<PathGesture>) {
    gesture
        .0
        .drag(event.pointer.id, event.pointer.position, event.delta);
}

pub(super) fn on_path_release(
    event: On<PointerRelease>,
    time: Res<Time>,
    touches: Res<Touches>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    if touches.iter().count() > 1 {
        touch.multi = true;
    }
    let now = time.elapsed_secs_f64();
    gesture.0.release(event.pointer.id, now);
    touch.path_drag_active = gesture.0.dragging();
}

pub(super) fn on_path_drag_end(
    event: On<PointerDragEnd>,
    time: Res<Time>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    gesture.0.release(event.pointer.id, time.elapsed_secs_f64());
    touch.path_drag_active = gesture.0.dragging();
}

pub(super) fn on_path_cancel(
    event: On<PointerCancel>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    gesture.0.cancel(event.pointer.id);
    touch.path_drag_active = gesture.0.dragging();
}

pub(super) fn path_drag(
    mut editor: ResMut<Editor>,
    gesture: Res<PathGesture>,
    capture: Res<PointerCapture>,
    camera: Single<(&Camera, &Transform)>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // Looking owns the mouse; a drag that was already in flight finishes when
    // the button is released.
    if capture.grabbed {
        return;
    }
    // Play never edits objects, even if a handle drag was in flight when the
    // mode switched. The gesture still releases normally.
    let Some(drag) = gesture.0.drag_target() else {
        return;
    };
    let Some(screen) = gesture.0.screen() else {
        return;
    };
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    apply_path_drag(
        &mut editor,
        drag,
        screen,
        camera,
        &global,
        &nav,
        &spatial_query,
        &filter,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_touch_button(
    event: On<Activate>,
    buttons: Query<&TouchButton>,
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    mut clipboard: ResMut<Clipboard>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    if editor.keyboard_transform.is_some() {
        return;
    }
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    let now = time.elapsed_secs_f64();
    // A touch often synthesizes a mouse click too, which would fire this
    // observer twice and toggle a tool straight back off.
    if let Some((last, at)) = touch.last_action {
        if last == button.0 && now - at < 0.25 {
            return;
        }
    }
    touch.last_action = Some((button.0, now));
    // A UI tap is not a grid tap: drop any tap from this gesture.
    touch.suppress_until = now + 0.08;
    touch.tap = None;
    if !object_editing_allowed(editor.mode) && touch_is_authoring(button.0) {
        return;
    }
    run_touch_action(
        button.0,
        &mut editor,
        &mut orbit,
        &mut touch,
        &mut clipboard,
        &nav,
        &spatial_query,
        &filter,
    );
}
