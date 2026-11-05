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
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    movers: Res<Movers>,
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
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
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
        PaletteAction::Grass => {
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Grass) {
                None
            } else {
                Some(ActorTool::Grass)
            };
            editor.tool = None;
            if editor.actor_tool == Some(ActorTool::Grass) {
                editor.note = if editor.grass_erase {
                    "grass brush: drag to erase, Erase toggles back to paint"
                } else {
                    "grass brush: drag to paint, Shift is not needed"
                };
            }
        }
        // Brush option toggles: flip the flag without disturbing the
        // selection or the armed tool.
        PaletteAction::GrassErase => {
            editor.grass_erase = !editor.grass_erase;
            editor.note = if editor.grass_erase {
                "grass eraser on"
            } else {
                "grass brush on"
            };
            return;
        }
        PaletteAction::GrassSlow => {
            editor.grass_slows = !editor.grass_slows;
            editor.note = if editor.grass_slows {
                "grass slows movement and routes"
            } else {
                "grass is decoration only"
            };
            // Route costs changed: rebuild the live route at once.
            if editor.plan.is_some() {
                let filter = player
                    .iter()
                    .next()
                    .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
                    .unwrap_or_default();
                if let Ok(rapier_ctx) = rapier.single() {
                    let live = editor.walk.is_some();
                    rebuild_route(&mut editor, &nav, &rapier_ctx, filter, live);
                }
            }
            return;
        }
        PaletteAction::Walk => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && let Ok(rapier_ctx) = rapier.single()
            {
                walk_from_plan(&mut editor, &nav, &rapier_ctx, filter);
            }
        }
        PaletteAction::Smooth => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && let Ok(rapier_ctx) = rapier.single()
                && smooth_plan(&mut editor, &nav, &rapier_ctx, filter)
            {
                editor.note = "smoothed";
            }
        }
        PaletteAction::Course => {
            editor.load_course();
            focus_character(&mut orbit, editor.character);
        }
        PaletteAction::CopyJson => {
            copy_level_json(&mut editor, &orbit, &movers, &mut clipboard);
            // Copying is not an authoring action: keep the current selection
            // and tool so the debug copy does not disturb what was on screen.
            return;
        }
    }
    // Motion-path editing pins the piece selection; a camera-only action
    // must not drop it mid-path. Every other palette action changes tools,
    // modes or levels, so the clear below still applies to those.
    if editor.actor_tool != Some(ActorTool::Platform) || !matches!(button.0, PaletteAction::View) {
        editor.selected = None;
    }
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
    rapier: ReadRapierContext,
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
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    let Ok(rapier_ctx) = rapier.single() else {
        return;
    };
    let was_walking = editor.walk.is_some();
    let ok = rebuild_route(&mut editor, &nav, &rapier_ctx, filter, was_walking);
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
                // Key names live in the Controls settings, not here: the
                // defaults are WASD / arrows / stick to move, Space to jump,
                // E to interact, hold Left Ctrl in the air to slide on landing.
                "playing: move / jump / interact (see Controls on the home screen)"
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

/// Camera, jump, interact and the mode switch stay live in Play; the rest author.
pub(super) fn touch_is_authoring(action: TouchAction) -> bool {
    !matches!(
        action,
        TouchAction::Play
            | TouchAction::View
            | TouchAction::Jump
            | TouchAction::Interact
            | TouchAction::Pad(_)
            | TouchAction::CopyJson
    )
}
/// Ctrl on Windows/Linux, Command on macOS.
pub(super) fn command_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
}

/// Cmd/Ctrl+Shift+Z or Cmd/Ctrl+Y. Covered by the Redo action in live
/// code; kept for the shortcut test below.
#[allow(dead_code)]
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
    outliner: Res<OutlinerState>,
    panel_state: Res<PropertiesPanelState>,
    camera: Single<(&Camera, &Transform), Without<MinimapCamera>>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    input: PlayerInput,
    time: Res<Time>,
    mut movers: ResMut<Movers>,
) {
    let (camera, camera_transform) = *camera;
    let camera_global = GlobalTransform::from(*camera_transform);
    let surface_filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    let Ok(rapier_ctx) = rapier.single() else {
        return;
    };

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
        &input.controls,
        &input.keys,
        &input.mouse,
        &cursor,
        allow_transform,
        over_palette,
    ) {
        return;
    }

    // P flips between authoring and play. Toggle before the cursor check so it
    // works even when the pointer leaves the window.
    if input.just_pressed(Action::TogglePlay) {
        let mode = editor.mode.toggled();
        set_mode(&mut editor, mode);
        return;
    }
    let playing = !object_editing_allowed(editor.mode);
    if playing && input.just_pressed(Action::Back) {
        set_mode(&mut editor, Mode::Build);
        return;
    }

    // Object copy/paste works from the keyboard even when the pointer is off
    // the grid. Paste falls back to one hex beside the source in that case.
    if !playing && input.just_pressed(Action::CopySel) {
        copy_piece(&mut editor);
    }
    if !playing && input.just_pressed(Action::Paste) && editor.actor_tool.is_none() {
        paste_piece(&mut editor, cursor.world);
    }

    let (Some(screen), Some(world)) = (cursor.screen, cursor.world) else {
        return;
    };

    if !playing && input.just_pressed(Action::Back) {
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

    if input.just_pressed(Action::WalkPlan) && editor.actor_tool == Some(ActorTool::Plan) {
        walk_from_plan(&mut editor, &nav, &rapier_ctx, surface_filter);
    }

    if !playing && input.just_pressed(Action::PlaceCharacter) {
        editor.tool = None;
        editor.actor_tool = Some(ActorTool::Place);
        editor.ghost = None;
        editor.note = "click the grid to place the character";
    }

    if !playing && input.just_pressed(Action::RotatePiece) {
        let step =
            if input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight) {
                -model::ANCHOR_STEP
            } else {
                model::ANCHOR_STEP
            };
        rotate_piece(&mut editor, step);
    }

    if !playing && input.just_pressed(Action::DeletePiece) {
        // Motion waypoints win while the Motion tool is armed; otherwise the
        // whole piece goes.
        if editor.actor_tool == Some(ActorTool::Platform)
            && delete_selected_mover_waypoint(&mut editor, &mut movers)
        {
            // Waypoint removed; the piece survives.
        } else {
            let waypoint_selected = editor.actor_tool == Some(ActorTool::Plan)
                && editor
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.selected.is_some());
            if waypoint_selected {
                delete_waypoint(&mut editor, &nav, &rapier_ctx, surface_filter);
            } else {
                delete_piece(&mut editor);
            }
        }
    }

    if !playing && input.just_pressed(Action::Undo) {
        editor.undo();
    } else if !playing && input.just_pressed(Action::Redo) {
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
        && over_properties_panel(
            screen,
            cursor.screen_size,
            editor.selected.is_some(),
            panel_state.collapsed,
            &touch,
        )
    {
        return;
    }
    if !playing && over_outliner_panel(screen, cursor.screen_size, outliner.collapsed, &touch) {
        return;
    }

    // Mouse clicks work in both layers; raw touch taps arrive in `cursor.tap`.
    // A tap that armed a UI button is filtered by `tap_allowed`.
    let clicked = (cursor.tap || input.just_pressed(Action::PrimaryClick))
        && tap_allowed(time.elapsed_secs_f64(), &touch);

    // A handle drag owns the pointer; do not also place or select.
    if gesture.0.dragging() {
        return;
    }

    editor.ghost = None;

    // Gizmo handles take priority over selecting whatever is under them.
    if gizmo_takes_pointer(&editor, touch.tap.is_some())
        && let (Some(screen), Some(ray)) = (cursor.screen, cursor.ray)
        && input.just_pressed(Action::PrimaryClick)
        && let Some((handle, start, grab)) =
            gizmo_grab(&editor, camera, &camera_global, screen, ray, false)
    {
        editor.commit();
        editor.gizmo_drag = Some(GizmoDrag {
            id: start.id,
            handle,
            grab,
            start,
            last_raw: start.elevation,
            last: start.elevation,
            pointer: PointerId::Mouse,
            screen,
        });
        editor.ghost = None;
    }

    if let Some(drag) = editor.gizmo_drag {
        if input.pressed(Action::PrimaryClick) {
            apply_gizmo_drag(&mut editor, drag, cursor.ray);
        }
        if input.just_released(Action::PrimaryClick) {
            // `commit` ran when the drag started, before the pointer moved. Mark
            // the finished pose dirty so autosave captures the actual rotation,
            // not the snapshot from the first frame of the drag.
            editor.hard_dirty = true;
            editor.gizmo_drag = None;
        }
        return;
    }

    // The Play minimap owns its corner: clicks there pan or route through the
    // minimap observers, never through the main canvas.
    if editor.mode == Mode::Play && minimap_hit(screen, cursor.screen_size, minimap_scale(&touch)) {
        return;
    }

    if let Some(tool) = editor.tool {
        let shape = &shapes()[tool];
        let default_trigger = crate::model::Trigger::damaging(shape.damage);
        let raw = Piece {
            id: 0,
            shape: tool,
            pos: world,
            rot: editor.ghost_rot,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: default_trigger,
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
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
                trigger: default_trigger,
                trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
                trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                interact: crate::model::InteractAction::None,
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
                hv: Vec2::ZERO,
                ride_vel: Vec2::ZERO,
                grounded: true,
                sliding: false,
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
            let (target, height) = surface_target(&cursor, &rapier_ctx, surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &rapier_ctx,
                surface_filter,
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
            let (target, height) = surface_target(&cursor, &rapier_ctx, surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
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
                &rapier_ctx,
                surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Grass) {
        // Click or hold-drag paints one stroke; the stroke commits once on
        // press and pops the undo entry on release when nothing changed.
        let held = input.pressed(Action::PrimaryClick);
        if clicked || held {
            if !editor.grass_stroke {
                editor.commit();
                editor.grass_stroke = true;
                editor.grass_stroke_dirty = false;
                editor.grass_dab = None;
            }
            let hex = model::pixel_to_hex(world);
            if editor.grass_dab != Some(hex) {
                editor.grass_dab = Some(hex);
                let erase = editor.grass_erase;
                if apply_dab(&mut editor.grass, hex, erase) {
                    editor.grass_stroke_dirty = true;
                    editor.grass_rev = editor.grass_rev.wrapping_add(1);
                    editor.hard_dirty = true;
                    editor.note = if editor.grass_erase {
                        "erased grass"
                    } else {
                        "painted grass"
                    };
                }
            }
            // A tap is a one-dab stroke: finish at once so separate taps
            // undo separately. A held button keeps stroking through the drag.
            if !held {
                finish_grass_stroke(&mut editor, &nav, &rapier_ctx, surface_filter);
            }
        }
        if input.just_released(Action::PrimaryClick) && editor.grass_stroke {
            finish_grass_stroke(&mut editor, &nav, &rapier_ctx, surface_filter);
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Platform) {
        if clicked {
            // A click on a waypoint or tangent handle selects it; it must
            // not also lay down a new waypoint. The press observer usually
            // records this first, but the click must stay a pure select
            // even if the two ever disagree.
            if select_mover_handle(
                &mut editor,
                &mut movers,
                camera,
                &camera_global,
                screen,
                touch.enabled,
            ) {
                return;
            }
            let id = match editor.selected {
                Some(id) if editor.pieces.iter().any(|p| p.id == id) => id,
                _ => {
                    // No valid selection: a click on a piece selects it so
                    // the following clicks can lay down its path.
                    if let Some(hit) = cursor
                        .ray
                        .and_then(|ray| pick_unlocked(&editor.pieces, ray, &editor.outline.locked))
                    {
                        editor.selected = Some(hit);
                        editor.note = "motion: piece selected, click to add path points";
                    } else {
                        editor.note = "select a piece first, then add motion";
                    }
                    return;
                }
            };
            editor.platform_request = Some((id, world));
        }
        return;
    }

    if clicked {
        let hit = cursor
            .ray
            .and_then(|ray| pick_unlocked(&editor.pieces, ray, &editor.outline.locked));

        // Play routes the capsule with plain clicks, including clicks on a
        // block or ramp (their tops are the surface the user means). Build
        // only authorises a route through the Path/Plan tools above; a bare
        // click there is selection, so loading a course cannot start walking.
        if click_routes(playing, orbit.mode, editor.character.is_some()) {
            let (target, height) = surface_target(&cursor, &rapier_ctx, surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
            let append = orbit.mode == ViewMode::Top && append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &rapier_ctx,
                surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        } else if playing {
            // Play routes the capsule; it never selects or drags objects.
        } else if let Some(hit) = hit {
            editor.selected = Some(hit);
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == hit) {
                // Grab in the piece's own horizontal plane, not on the
                // ground: clicking the face of a tall or elevated piece
                // projects far behind its base on y = 0, which made the
                // piece feel offset from the cursor as soon as it moved.
                let plane = piece.elevation;
                let grab = cursor
                    .ray
                    .and_then(|ray| drag_plane_point(ray, plane))
                    .unwrap_or(world);
                editor.drag = Some(Drag {
                    id: hit,
                    offset: piece.pos - grab,
                    plane,
                    moved: false,
                });
            }
            editor.commit();
        } else {
            editor.selected = None;
            editor.drag = None;
        }
    }

    if !playing && input.pressed(Action::PrimaryClick) {
        if let Some(drag) = editor.drag {
            let base = cursor
                .ray
                .and_then(|ray| drag_plane_point(ray, drag.plane))
                .unwrap_or(world);
            apply_body_drag(&mut editor, drag, base);
        }
    }

    if !playing
        && input.just_released(Action::PrimaryClick)
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

/// Advance a body drag to the pointer's current ground-plane projection.
/// A click that only selects the piece (no pointer travel yet) must not move
/// it, even when it sits within anchor range of a neighbour. This mirrors
/// `apply_gizmo_drag`, which keeps the last elevation until the pointer moves.
pub(super) fn apply_body_drag(editor: &mut Editor, drag: Drag, base: Vec2) {
    let Some((index, piece)) = editor
        .pieces
        .iter()
        .enumerate()
        .find(|(_, p)| p.id == drag.id)
        .map(|(i, p)| (i, *p))
    else {
        return;
    };
    // The pointer has not travelled yet: selecting must not snap.
    let raw_pos = base + drag.offset;
    if (raw_pos - piece.pos).length_squared() < 1e-10 {
        return;
    }
    let shape = &shapes()[piece.shape];
    // Follow the pointer in the grab plane. Anchor-snapping stays magnetic,
    // but there is no grid snap mid-drag: re-snapping to the nearest
    // hex/edge every frame is what made pieces jump between cells as soon as
    // the drag started.
    let raw = Piece {
        pos: raw_pos,
        ..piece
    };
    let snap = model::snap_piece(
        &raw,
        shape,
        &editor.pieces,
        editor.snap,
        false,
        SNAP_THRESHOLD,
    );
    let target = &mut editor.pieces[index];
    let changed =
        (target.pos - snap.pos).length_squared() > 1e-10 || (target.rot - snap.rot).abs() > 1e-6;
    target.pos = snap.pos;
    target.rot = snap.rot;
    if changed {
        if let Some(d) = editor.drag.as_mut() {
            d.moved = true;
        }
    }
}

/// Pointer position on the horizontal plane at `height`. Dragging in the
/// piece's own plane (rather than always on y = 0) keeps the grab point under
/// the cursor for tall and elevated pieces.
fn drag_plane_point(ray: Ray3d, height: f32) -> Option<Vec2> {
    let dir = *ray.direction;
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (height - ray.origin.y) / dir.y;
    if t < 0.0 {
        return None;
    }
    let hit = ray.origin + dir * t;
    Some(Vec2::new(hit.x, hit.z))
}
