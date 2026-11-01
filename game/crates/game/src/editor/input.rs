//! input systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn on_palette_activate(
    event: On<Activate>,
    buttons: Query<&PaletteButton>,
    time: Res<Time>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    mut clipboard: ResMut<Clipboard>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // While the pointer is captured the mouse only looks; Tab must release it
    // before a button can be used.
    if capture.grabbed || editor.keyboard_transform.is_some() {
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
    if let Some((last, at)) = editor.last_palette {
        if last == button.0 && now - at < 0.25 {
            return;
        }
    }
    editor.last_palette = Some((button.0, now));
    // A click on an on-screen control is not a canvas click: the play bar
    // floats over the world in play mode.
    touch.suppress_until = touch.suppress_until.max(now + 0.08);
    // While playing, only the mode switch responds; every other palette button
    // is an authoring action and must not change the level.
    if !object_editing_allowed(editor.mode) && palette_is_authoring(button.0) {
        return;
    }
    match button.0 {
        PaletteAction::Build => set_mode(&mut editor, Mode::Build),
        PaletteAction::Play => set_mode(&mut editor, Mode::Play),
        PaletteAction::View => cycle_view(&mut orbit, &editor),
        PaletteAction::Shape(index) => {
            editor.tool = if editor.tool == Some(index) {
                None
            } else {
                Some(index)
            };
            editor.actor_tool = None;
        }
        PaletteAction::Character => {
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Place) {
                None
            } else {
                Some(ActorTool::Place)
            };
            editor.tool = None;
        }
        PaletteAction::Path => {
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Path) {
                None
            } else {
                Some(ActorTool::Path)
            };
            editor.tool = None;
        }
        PaletteAction::Plan => {
            let entering = editor.actor_tool != Some(ActorTool::Plan);
            editor.actor_tool = if entering {
                Some(ActorTool::Plan)
            } else {
                None
            };
            editor.tool = None;
            if entering {
                orbit.mode = ViewMode::Top;
                orbit.pitch = 1.4;
                orbit.distance = 13.0;
                focus_character(&mut orbit, editor.character);
            }
        }
        PaletteAction::Platform => {
            let entering = editor.actor_tool != Some(ActorTool::Platform);
            editor.actor_tool = if entering {
                Some(ActorTool::Platform)
            } else {
                None
            };
            editor.tool = None;
            if entering {
                editor.note = if editor.selected.is_some() {
                    "motion: click to add path points"
                } else {
                    "select a piece first, then add motion"
                };
            }
        }
        PaletteAction::Walk => {
            if editor.actor_tool == Some(ActorTool::Plan) {
                walk_from_plan(&mut editor, &nav, &spatial_query, &filter);
            }
        }
        PaletteAction::Smooth => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && smooth_plan(&mut editor, &nav, &spatial_query, &filter)
            {
                editor.note = "smoothed";
            }
        }
        PaletteAction::Course => {
            editor.load_course();
            focus_character(&mut orbit, editor.character);
        }
        PaletteAction::CopyJson => {
            copy_level_json(&mut editor, &mut clipboard);
            // Copying is not an authoring action: keep the current selection
            // and tool so the debug copy does not disturb what was on screen.
            return;
        }
    }
    editor.selected = None;
    editor.ghost = None;
    editor.ghost_rot = 0.0;
}

/// Keep the walkable hex graph in step with the placed pieces. Blocks and ramps
/// therefore take effect as soon as they land, not on the next reload.
pub(super) fn rebuild_nav(editor: Res<Editor>, mut nav: ResMut<Nav>) {
    if nav.pieces == editor.pieces {
        return;
    }
    let (edges, hexes) = path::blocked(&editor.pieces);
    nav.edges = edges;
    nav.hexes = hexes;
    nav.pieces = editor.pieces.clone();
    nav.revision += 1;
}

/// If a route is up and the navmesh changed, plan it again against the new
/// graph. Placing a block bends an existing path around it; placing a ramp can
/// open the route up onto a block.
pub(super) fn refresh_route(
    mut editor: ResMut<Editor>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // Mid-drag the pieces have not settled yet; wait for the release.
    if editor.drag.is_some() || editor.gizmo_drag.is_some() {
        return;
    }
    if editor.seen_nav == nav.revision {
        return;
    }
    editor.seen_nav = nav.revision;
    if editor.plan.as_ref().is_none_or(|plan| plan.is_empty()) {
        return;
    }
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    let was_walking = editor.walk.is_some();
    let ok = rebuild_route(&mut editor, &nav, &spatial_query, &filter, was_walking);
    let (_, valid) = rebuild_walk_state(ok, was_walking);
    if let Some(route) = editor.route.as_mut() {
        route.valid = valid;
    }
    if !ok {
        editor.note = if was_walking {
            "layout broke the plan, still walking"
        } else {
            "plan no longer reachable"
        };
    }
}

/// Object editing — placing, selecting, gizmos, rotate/delete/undo — is
/// Build-only. Route editing (waypoints and curve handles) stays available in
/// Play so the authored path can be test-driven and reshaped.
pub(super) fn object_editing_allowed(mode: Mode) -> bool {
    mode == Mode::Build
}

/// Switch modes. Play holsters the object tools so a stray click cannot place,
/// select or delete; the level, plan and any live walk survive the switch.
pub(super) fn set_mode(editor: &mut Editor, mode: Mode) {
    if editor.mode == mode {
        return;
    }
    editor.mode = mode;
    editor.keyboard_transform = None;
    match mode {
        Mode::Play => {
            editor.tool = None;
            editor.actor_tool = None;
            editor.selected = None;
            editor.ghost = None;
            editor.drag = None;
            editor.gizmo_drag = None;
            editor.note = if editor.character.is_some() {
                "playing: WASD / stick moves the capsule"
            } else {
                "playing: place a character to control one"
            };
        }
        Mode::Build => {
            editor.note = "build mode";
        }
    }
}

/// Every palette action except the mode switch and the view control changes
/// the level.
pub(super) fn palette_is_authoring(action: PaletteAction) -> bool {
    !matches!(
        action,
        PaletteAction::Build | PaletteAction::Play | PaletteAction::View | PaletteAction::CopyJson
    )
}

/// Camera, jump and the mode switch stay live in Play; the rest author.
pub(super) fn touch_is_authoring(action: TouchAction) -> bool {
    !matches!(
        action,
        TouchAction::Play | TouchAction::View | TouchAction::Jump | TouchAction::CopyJson
    )
}

/// Ctrl on Windows/Linux, Command on macOS.
pub(super) fn command_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
}

/// Cmd/Ctrl+Shift+Z or Cmd/Ctrl+Y.
pub(super) fn redo_requested(keys: &ButtonInput<KeyCode>) -> bool {
    if !command_down(keys) {
        return false;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    (shift && keys.just_pressed(KeyCode::KeyZ)) || keys.just_pressed(KeyCode::KeyY)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn interact(
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    cursor: Res<Cursor>,
    capture: Res<PointerCapture>,
    touch: Res<TouchState>,
    gesture: Res<PathGesture>,
    nav: Res<Nav>,
    menu: Res<AddMenu>,
    camera: Single<(&Camera, &Transform)>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut clipboard: ResMut<Clipboard>,
) {
    let (camera, camera_transform) = *camera;
    let camera_global = GlobalTransform::from(*camera_transform);
    let surface_filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();

    // A focused property field owns the keyboard. The text widget handles its
    // own edits and shortcuts.
    if cursor.text_focus {
        return;
    }

    // The Add menu is modal: choosing (or dismissing) it must never also
    // place, select or drag a piece on the same click.
    if menu.open {
        return;
    }

    // Blender-style G/R owns the keyboard and pointer until confirm or cancel.
    let allow_transform =
        !capture.grabbed && orbit.mode != ViewMode::First && !gesture.0.dragging();
    let over_palette = !touch.enabled && cursor.screen.is_some_and(|p| p.x <= PANEL_WIDTH);
    if keyboard_transform(
        &mut editor,
        &keys,
        &mouse,
        &cursor,
        allow_transform,
        over_palette,
    ) {
        return;
    }

    // P flips between authoring and play. Toggle before the cursor check so it
    // works even when the pointer leaves the window.
    if keys.just_pressed(KeyCode::KeyP) {
        let mode = editor.mode.toggled();
        set_mode(&mut editor, mode);
        return;
    }
    let playing = !object_editing_allowed(editor.mode);
    if playing && keys.just_pressed(KeyCode::Escape) {
        set_mode(&mut editor, Mode::Build);
        return;
    }

    // Object copy/paste works from the keyboard even when the pointer is off
    // the grid. Paste falls back to one hex beside the source in that case.
    let command = command_down(&keys);
    if !playing && command && keys.just_pressed(KeyCode::KeyC) {
        copy_piece(&mut editor);
    }
    if !playing && command && keys.just_pressed(KeyCode::KeyV) && editor.actor_tool.is_none() {
        paste_piece(&mut editor, cursor.world);
    }

    let (Some(screen), Some(world)) = (cursor.screen, cursor.world) else {
        return;
    };

    if !playing && keys.just_pressed(KeyCode::Escape) {
        editor.tool = None;
        editor.actor_tool = None;
        editor.selected = None;
        editor.ghost = None;
        editor.drag = None;
        editor.plan = None;
        editor.route = None;
        editor.walk = None;
        editor.note = "Click a component, then the grid";
    }

    if keys.just_pressed(KeyCode::Enter) && editor.actor_tool == Some(ActorTool::Plan) {
        walk_from_plan(&mut editor, &nav, &spatial_query, &surface_filter);
    }

    if !playing && !command && keys.just_pressed(KeyCode::KeyC) {
        editor.tool = None;
        editor.actor_tool = Some(ActorTool::Place);
        editor.ghost = None;
        editor.note = "click the grid to place the character";
    }

    if !playing && !command && keys.just_pressed(KeyCode::KeyR) {
        let step = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            -model::ANCHOR_STEP
        } else {
            model::ANCHOR_STEP
        };
        rotate_piece(&mut editor, step);
    }

    if !playing && (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace)) {
        let waypoint_selected = editor.actor_tool == Some(ActorTool::Plan)
            && editor
                .plan
                .as_ref()
                .is_some_and(|plan| plan.selected.is_some());
        if waypoint_selected {
            delete_waypoint(&mut editor, &nav, &spatial_query, &surface_filter);
        } else {
            delete_piece(&mut editor);
        }
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if !playing && command && keys.just_pressed(KeyCode::KeyZ) {
        if shift {
            editor.redo();
        } else {
            editor.undo();
        }
    } else if !playing && redo_requested(&keys) {
        editor.redo();
    }

    // Manual S-key saving lives in `save_shortcut` (same chain) so `interact`
    // stays under Bevy's system-parameter limit while saves still see movers.

    // Captured first-person look owns the mouse: clicks must not place,
    // select, drag or route until Tab releases the pointer.
    if capture.grabbed {
        return;
    }

    // Clicks on the palette are handled by the widget observers; ignore them
    // here. The palette is hidden in touch mode and in Play, so the whole
    // screen is canvas there.
    if !playing && !touch.enabled && screen.x <= PANEL_WIDTH {
        return;
    }
    if !playing
        && !touch.enabled
        && over_properties_panel(screen, cursor.screen_size, editor.selected.is_some())
    {
        return;
    }

    // Mouse clicks work in both layers; raw touch taps arrive in `cursor.tap`.
    // A tap that armed a UI button is filtered by `tap_allowed`.
    let clicked = (cursor.tap || mouse.just_pressed(MouseButton::Left))
        && tap_allowed(time.elapsed_secs_f64(), &touch);

    // A handle drag owns the pointer; do not also place or select.
    if gesture.0.dragging() {
        return;
    }

    editor.ghost = None;

    // Gizmo handles take priority over selecting whatever is under them.
    if gizmo_takes_pointer(&editor, touch.tap.is_some())
        && let (Some(screen), Some(ray)) = (cursor.screen, cursor.ray)
        && mouse.just_pressed(MouseButton::Left)
        && let Some(handle) = gizmo_hit(&editor, camera, &camera_global, screen, ray)
        && let Some(start) = editor
            .selected
            .and_then(|id| editor.pieces.iter().find(|p| p.id == id).copied())
        && let Some(shape) = shapes().get(start.shape)
        && let Some(grab) = gizmo_drag_point(ray, handle, gizmo_origin(&start, shape))
    {
        editor.commit();
        editor.gizmo_drag = Some(GizmoDrag {
            id: start.id,
            handle,
            grab,
            start,
            last_raw: start.elevation,
            last: start.elevation,
        });
        editor.ghost = None;
    }

    if let Some(drag) = editor.gizmo_drag {
        if mouse.pressed(MouseButton::Left) {
            apply_gizmo_drag(&mut editor, drag, cursor.ray);
        }
        if mouse.just_released(MouseButton::Left) {
            // `commit` ran when the drag started, before the pointer moved. Mark
            // the finished pose dirty so autosave captures the actual rotation,
            // not the snapshot from the first frame of the drag.
            editor.hard_dirty = true;
            editor.gizmo_drag = None;
        }
        return;
    }

    if let Some(tool) = editor.tool {
        let shape = &shapes()[tool];
        let raw = Piece {
            id: 0,
            shape: tool,
            pos: world,
            rot: editor.ghost_rot,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
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

        if clicked {
            editor.commit();
            let id = editor.next_id;
            editor.next_id += 1;
            editor.pieces.push(Piece {
                id,
                shape: tool,
                pos: snap.pos,
                rot: snap.rot,
                pitch: 0.0,
                roll: 0.0,
                elevation: 0.0,
                dynamic: false,
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

    if editor.actor_tool == Some(ActorTool::Place) {
        if clicked {
            let pos = model::nearest_hex(world);
            let character = Character {
                pos,
                feet: 0.0,
                yaw: orbit.yaw,
                vy: 0.0,
                grounded: true,
                health: crate::statuses::Health::default(),
            };
            editor.character = Some(character);
            editor.route = None;
            editor.walk = None;
            focus_character(&mut orbit, Some(character));
            editor.note = "character placed";
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Path) {
        if clicked && orbit.mode != ViewMode::First {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            // A tap here is a waypoint, never a selection: drop any stale
            // highlight left over from placing the block.
            editor.selected = None;
            editor.drag = None;
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Plan) {
        if orbit.mode != ViewMode::Top {
            if clicked {
                editor.note = "plan editing is top-down";
            }
            return;
        }
        if clicked {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            let start_walk = click_starts_walk(true, editor.walk.is_some());
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                start_walk,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Platform) {
        if clicked {
            let Some(id) = editor.selected else {
                editor.note = "select a piece first, then add motion";
                return;
            };
            if editor.pieces.iter().any(|p| p.id == id) {
                editor.platform_request = Some((id, world));
            } else {
                editor.note = "select a piece first, then add motion";
            }
        }
        return;
    }

    if clicked {
        let hit = cursor.ray.and_then(|ray| pick(&editor.pieces, ray));

        // Play routes the capsule with plain clicks, including clicks on a
        // block or ramp (their tops are the surface the user means). Build
        // only authorises a route through the Path/Plan tools above; a bare
        // click there is selection, so loading a course cannot start walking.
        if click_routes(playing, orbit.mode, editor.character.is_some()) {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = orbit.mode == ViewMode::Top && append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        } else if playing {
            // Play routes the capsule; it never selects or drags objects.
        } else if let Some(hit) = hit {
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

    if !playing && mouse.pressed(MouseButton::Left) {
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

    if !playing
        && mouse.just_released(MouseButton::Left)
        && let Some(drag) = editor.drag
    {
        if drag.moved {
            // Same as a gizmo drag: the commit happened on press, so mark
            // the final position dirty when the pointer lifts.
            editor.hard_dirty = true;
        } else {
            editor.undo.pop();
        }
        editor.drag = None;
    }
}
