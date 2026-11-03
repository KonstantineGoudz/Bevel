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
    editor: Res<Editor>,
    panel_state: Res<PropertiesPanelState>,
    outliner: Res<OutlinerState>,
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
        let mut control = !stick && over_controls(pos, size, s);
        // A press on the floating property panel is panel business, never a
        // camera gesture or a grid tap.
        if !control
            && editor.mode == Mode::Build
            && editor.selected.is_some()
            && over_properties_panel(pos, size, true, panel_state.collapsed, &state)
        {
            control = true;
        }
        // Same for the object graph: its rows, toggles and label field own
        // the press.
        if !control
            && editor.mode == Mode::Build
            && over_outliner_panel(pos, size, outliner.collapsed, &state)
        {
            control = true;
        }
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

/// The A/B/X/Y pad cluster (bottom-right). Its buttons are real widgets, so
/// a press on them never reaches the canvas observer — but the raw-touch
/// fallback sees every press and must not steal those taps for gizmos.
/// Geometry mirrors `spawn_touch_ui`: 76px pads at right/bottom 16/102 and
/// 20/106, in the same scaled space as `over_controls`.
pub(super) fn over_pad_cluster(pos: Vec2, size: Vec2, scale: f32) -> bool {
    pos.x >= size.x - 186.0 * scale && pos.y >= size.y - 190.0 * scale
}

/// Grab, drive and release gizmo drags straight from the raw touch stream,
/// without waiting for UI picking to deliver events to the canvas.
///
/// On some mobile browsers a press that lands on overlapping UI never
/// reaches the canvas observer (`on_path_press`), so finger taps on a
/// handle silently did nothing while mouse and synthetic tests worked.
/// This runs right after `touch_input` and tries the same grab the observer
/// would; whichever path runs first wins, because both go through
/// `gizmo_grab_allowed` (which refuses while a drag exists).
///
/// The drive and release halves are load-bearing, not just the grab: a
/// press that missed the canvas will also miss it for move/release events
/// (implicit pointer capture retargets them to the press target), so a
/// raw-grabbed drag must follow the touch track and end when the track
/// disappears — otherwise it sticks forever and blocks selection and the
/// camera.
#[allow(clippy::too_many_arguments)]
pub(super) fn touch_gizmo_fallback(
    touches: Res<Touches>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform)>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
    panel_state: Res<PropertiesPanelState>,
    outliner: Res<OutlinerState>,
) {
    if !touch.enabled || capture.grabbed || menu.open {
        return;
    }
    if editor.keyboard_transform.is_some() || focus.get().is_some() {
        return;
    }
    // A second finger down is a pinch, not a handle grab.
    if touch.multi {
        return;
    }
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    let size = window.size();
    let s = touch.ui_scale;
    let centre = stick_centre(size, s);
    let reach = STICK_REACH * s * 1.7;
    let now = time.elapsed_secs_f64();
    for pressed in touches.iter_just_pressed() {
        // The observer path may already own this press.
        if editor.gizmo_drag.is_some() {
            return;
        }
        let id = pressed.id();
        let pos = pressed.position();
        if grabs_stick(pos, centre, reach)
            || over_controls(pos, size, s)
            || over_pad_cluster(pos, size, s)
            || (editor.selected.is_some()
                && over_properties_panel(pos, size, true, panel_state.collapsed, &touch))
            || (editor.mode == Mode::Build
                && over_outliner_panel(pos, size, outliner.collapsed, &touch))
        {
            continue;
        }
        if !gizmo_grab_allowed(&editor, gesture.0.dragging()) {
            continue;
        }
        let Ok(ray) = camera.viewport_to_world(&global, pos) else {
            continue;
        };
        if let Some((handle, start, grab)) = gizmo_grab(&editor, camera, &global, pos, ray, true) {
            editor.commit();
            editor.gizmo_drag = Some(GizmoDrag {
                id: start.id,
                handle,
                grab,
                start,
                last_raw: start.elevation,
                last: start.elevation,
                pointer: PointerId::Touch(id),
                screen: pos,
            });
            // The track already exists (this runs after `touch_input`): mark
            // it owned so the finger neither orbits nor taps while dragging.
            if let Some(track) = touch.tracks.get_mut(&id) {
                track.control = true;
            }
            touch.tap = None;
            touch.suppress_until = now + 0.08;
        }
    }
    // Drive and release raw-grabbed drags from the track: their move and
    // release events miss the canvas observers for the same reason the
    // press did. Mouse drags never match a touch track and are untouched.
    if let Some(drag) = editor.gizmo_drag.as_mut()
        && let PointerId::Touch(id) = drag.pointer
    {
        if let Some(track) = touch.tracks.get(&id) {
            drag.screen = track.last;
        } else {
            // Finger lifted (or canceled) with no release event delivered:
            // end the drag here so it can never stick and block selection
            // and the camera.
            release_gizmo_drag(&mut editor, &mut touch, PointerId::Touch(id), now);
        }
    }
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
    menu: Res<AddMenu>,
    controls: Res<Controls>,
    mut buttons: Query<(&TouchButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = match button.0 {
            TouchAction::Play => false,
            TouchAction::Path => editor.actor_tool == Some(ActorTool::Path),
            TouchAction::Plan => editor.actor_tool == Some(ActorTool::Plan),
            TouchAction::Add => touch.append,
            TouchAction::Search => menu.open,
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
            // The remappable cluster highlights while its toggled action is
            // armed; momentary actions (jump / interact) never highlight.
            TouchAction::Pad(button) => match controls.touch_action(button) {
                Some(Action::Sprint) => touch.sprint,
                Some(Action::Slide) => touch.slide,
                _ => false,
            },
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
        Some(ActorTool::Platform) => "motion",
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
        let width = (css.x - 20.0) / 10.0;
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
            TouchAction::Search,
        ]
        .get(index.min(9))
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

/// Apply one press of a remappable A/B/X/Y cluster button. Jump and
/// interact fire once; sprint and slide toggle until pressed again (there is
/// no release event on the touch buttons, so holds are sticky).
pub(super) fn apply_touch_button(touch: &mut TouchState, action: Option<Action>) {
    match action {
        Some(Action::Jump) => touch.jump = true,
        Some(Action::Interact) => touch.interact = true,
        Some(Action::Sprint) => touch.sprint = !touch.sprint,
        Some(Action::Slide) => touch.slide = !touch.slide,
        _ => {}
    }
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
    controls: &Controls,
    movers: &Movers,
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
        TouchAction::Interact => touch.interact = true,
        TouchAction::Pad(button) => {
            apply_touch_button(touch, controls.touch_action(button));
        }
        TouchAction::Course => {
            editor.load_course();
            focus_character(orbit, editor.character);
        }
        TouchAction::CopyJson => copy_level_json(editor, orbit, movers, clipboard),
        // The search button toggles the Add menu in `on_touch_button`, which
        // owns the menu focus and list state. Nothing to do on the level.
        TouchAction::Search => {}
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
    mut movers: ResMut<Movers>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
    panel_state: Res<PropertiesPanelState>,
    outliner: Res<OutlinerState>,
) {
    if event.button != PointerButton::Primary || capture.grabbed {
        return;
    }
    if menu.open {
        return;
    }
    if editor.keyboard_transform.is_some() || focus.get().is_some() {
        return;
    }
    if touches.iter().count() > 1 {
        touch.multi = true;
    }
    let pos = event.pointer.position;
    if editor.selected.is_some()
        && over_properties_panel(pos, window.size(), true, panel_state.collapsed, &touch)
    {
        return;
    }
    if editor.mode == Mode::Build
        && over_outliner_panel(pos, window.size(), outliner.collapsed, &touch)
    {
        return;
    }
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
    let now = time.elapsed_secs_f64();
    // Gizmo handles take priority over path handles and selection, on
    // touch just as on desktop. The desktop mouse path lives in
    // `interact`; this branch only serves non-mouse pointers (fingers,
    // pens) so the two can never double-grab the same press. Touch uses
    // the fingertip hit tolerance.
    if event.pointer.id != PointerId::Mouse
        && gizmo_grab_allowed(&editor, gesture.0.dragging())
        && let Ok(ray) = camera.viewport_to_world(&global, pos)
        && let Some((handle, start, grab)) = gizmo_grab(&editor, camera, &global, pos, ray, true)
    {
        editor.commit();
        editor.gizmo_drag = Some(GizmoDrag {
            id: start.id,
            handle,
            grab,
            start,
            last_raw: start.elevation,
            last: start.elevation,
            pointer: event.pointer.id,
            screen: pos,
        });
        touch.tap = None;
        touch.suppress_until = now + 0.08;
        return;
    }
    // A gizmo drag owns the gesture: a second finger must not start a
    // path drag while a handle is held.
    if editor.gizmo_drag.is_some() {
        return;
    }
    let handle = path_handle_at(
        &editor,
        &movers,
        camera,
        &global,
        pos,
        touch.enabled,
        &orbit,
    );
    let drag = gesture.0.press(event.pointer.id, pos, now, handle);
    if let Some(drag) = drag {
        if editor.actor_tool == Some(ActorTool::Platform) {
            // A motion handle belongs to the selected piece's path; record
            // the waypoint so Delete and the panel know what is active.
            let index = match drag.handle {
                PathHandle::Waypoint(id) => id as usize,
                PathHandle::Tangent { waypoint, .. } => waypoint as usize,
            };
            if let Some(id) = editor.selected {
                if let Some(mover) = movers.map.get_mut(&id) {
                    mover.selected = Some(index);
                }
            }
        } else {
            let waypoint = match drag.handle {
                PathHandle::Waypoint(id) => id,
                PathHandle::Tangent { waypoint, .. } => waypoint,
            };
            if let Some(plan) = editor.plan.as_mut() {
                plan.select(Some(waypoint));
            }
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

/// Track the owning finger of a touch gizmo drag. PointerDrag keeps
/// arriving over UI nodes; PointerMove alone would stall over the toolbar.
fn track_gizmo_pointer(editor: &mut Editor, pointer: PointerId, screen: Vec2) {
    if let Some(drag) = editor.gizmo_drag.as_mut()
        && drag.pointer == pointer
    {
        drag.screen = screen;
    }
}

pub(super) fn on_gizmo_move(event: On<PointerMove>, mut editor: ResMut<Editor>) {
    track_gizmo_pointer(&mut editor, event.pointer.id, event.pointer.position);
}

pub(super) fn on_gizmo_drag(event: On<PointerDrag>, mut editor: ResMut<Editor>) {
    track_gizmo_pointer(&mut editor, event.pointer.id, event.pointer.position);
}

/// Finish the gizmo drag owned by `pointer`, if any. Touch has no
/// mouse-release signal, so pointer release ends the drag here instead of
/// in `interact`.
pub(super) fn on_gizmo_release(
    event: On<PointerRelease>,
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    mut touch: ResMut<TouchState>,
) {
    release_gizmo_drag(
        &mut editor,
        &mut touch,
        event.pointer.id,
        time.elapsed_secs_f64(),
    );
}

pub(super) fn on_gizmo_drag_end(
    event: On<PointerDragEnd>,
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    mut touch: ResMut<TouchState>,
) {
    release_gizmo_drag(
        &mut editor,
        &mut touch,
        event.pointer.id,
        time.elapsed_secs_f64(),
    );
}

pub(super) fn on_gizmo_cancel(
    event: On<PointerCancel>,
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    mut touch: ResMut<TouchState>,
) {
    release_gizmo_drag(
        &mut editor,
        &mut touch,
        event.pointer.id,
        time.elapsed_secs_f64(),
    );
}

pub(super) fn path_drag(
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
    gesture: Res<PathGesture>,
    capture: Res<PointerCapture>,
    camera: Single<(&Camera, &Transform)>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
    menu: Res<AddMenu>,
) {
    // Looking owns the mouse; a drag that was already in flight finishes when
    // the button is released.
    if capture.grabbed || menu.open {
        return;
    }
    // Touch gizmo drags are driven from pointer events: a finger has no
    // cursor, so `cursor.ray` is stale while it is down. The desktop mouse
    // path keeps living in `interact`.
    if let Some(drag) = editor.gizmo_drag
        && drag.pointer != PointerId::Mouse
    {
        let (camera, transform) = *camera;
        let global = GlobalTransform::from(*transform);
        let ray = camera.viewport_to_world(&global, drag.screen).ok();
        apply_gizmo_drag(&mut editor, drag, ray);
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
    // Motion handles edit the selected piece's path, not the character plan.
    if editor.actor_tool == Some(ActorTool::Platform) {
        apply_mover_drag(&mut editor, &mut movers, drag, screen, camera, &global);
        return;
    }
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
    mut menu: ResMut<AddMenu>,
    mut focus: ResMut<InputFocus>,
    mut search_query: Query<(Entity, &mut bevy::text::EditableText), With<AddMenuSearch>>,
    mut root_query: Query<&mut Node, With<AddMenuRoot>>,
    controls: Res<Controls>,
    movers: Res<Movers>,
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
    if button.0 == TouchAction::Search {
        // Mobile-only toggle for the Add (entity search) menu.
        if !object_editing_allowed(editor.mode) || orbit.mode == ViewMode::First {
            return;
        }
        // Another text field owns the keyboard; don't steal it.
        if focus.get().is_some() && !menu.open {
            return;
        }
        if menu.open {
            if let Ok((search_entity, mut text)) = search_query.single_mut() {
                close_menu(&mut menu, &mut text, &mut focus, search_entity);
            } else {
                menu.open = false;
                menu.query.clear();
                menu.selected = 0;
                focus.clear();
            }
            sync_root_visibility(&mut root_query, false);
        } else if let Ok((search_entity, mut text)) = search_query.single_mut() {
            open_menu(&mut menu, search_entity, &mut text, &mut focus);
            sync_root_visibility(&mut root_query, true);
        }
        return;
    }
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
        &controls,
        &movers,
    );
}
