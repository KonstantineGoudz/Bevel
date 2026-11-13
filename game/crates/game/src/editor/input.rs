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
    mut movers: ResMut<Movers>,
    mut home: ResMut<HomeMenu>,
) {
    // While the pointer is captured the mouse only looks; Tab must release it
    // before a button can be used.
    if capture.grabbed || editor.keyboard_transform.is_some() {
        return;
    }
    // The home overlay owns every click while open.
    if home.open {
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
        PaletteAction::Building => set_mode(&mut editor, Mode::Building),
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
            if editor.actor_tool == Some(ActorTool::Platform)
                && let Some(id) = editor.selection.primary()
                && let Some(mover) = movers.map.get_mut(&id)
            {
                for wp in &mut mover.waypoints {
                    wp.incoming = None;
                    wp.outgoing = None;
                }
                mover.rebuild();
                editor.hard_dirty = true;
                editor.note = "smoothed";
            } else if editor.actor_tool == Some(ActorTool::Plan)
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
        PaletteAction::Home => {
            open_home(&mut home);
            home.status = current_level_note(&editor);
            home.dirty = home.dirty.wrapping_add(1);
            #[cfg(target_arch = "wasm32")]
            {
                editor.note = "maps live on the browser home page";
            }
            return;
        }
    }
    // Motion-path editing pins the piece selection; a camera-only action
    // must not drop it mid-path. Every other palette action changes tools,
    // modes or levels, so the clear below still applies to those.
    if editor.actor_tool != Some(ActorTool::Platform) || !matches!(button.0, PaletteAction::View) {
        editor.selection.clear();
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
/// Build/Building-only. Route editing (waypoints and curve handles) stays
/// available in Play so the authored path can be test-driven and reshaped.
pub(super) fn object_editing_allowed(mode: Mode) -> bool {
    matches!(mode, Mode::Build | Mode::Building)
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
            editor.selection.clear();
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
        Mode::Building => {
            editor.note = "building mode";
        }
    }
}

/// Every palette action except the mode switch and the view control changes
/// the level.
pub(super) fn palette_is_authoring(action: PaletteAction) -> bool {
    !matches!(
        action,
        PaletteAction::Build
            | PaletteAction::Building
            | PaletteAction::Play
            | PaletteAction::View
            | PaletteAction::CopyJson
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
    gates: MenuHome,
    right_panel: Res<RightPanelState>,
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
    if gates.menu.open {
        return;
    }

    // The native home screen owns the canvas while open.
    if home_modal_open(gates.home) {
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
        editor.selection.clear();
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
    if !playing && over_right_panel(screen, cursor.screen_size, &right_panel, &touch) {
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
            others: dragged_group(&editor, start.id),
            last_raw: start.elevation,
            last: start.elevation,
            pointer: PointerId::Mouse,
            screen,
        });
        editor.ghost = None;
    }

    if let Some(drag) = editor.gizmo_drag.clone() {
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
        let default_size = if shape.is_glade() {
            crate::glade::default_size(shape)
        } else {
            crate::model::TRIGGER_DEFAULT_SIZE
        };
        let default_alpha = if shape.is_glade() {
            crate::glade::GLADE_DEFAULT_ROOF_PITCH
        } else {
            crate::model::TRIGGER_DEFAULT_ALPHA
        };
        let default_color = if shape.id == "glade_terrain" {
            crate::glade::DEFAULT_TERRAIN_COLOR
        } else if shape.is_glade() {
            crate::glade::GLADE_DEFAULT_ROOF_COLOR
        } else {
            crate::model::TRIGGER_DEFAULT_COLOR
        };
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
            trigger_size: default_size,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: default_alpha,
            trigger_color: default_color,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        };
        let (snap, snap_elevation) =
            if matches!(shape.id, "glade_door" | "glade_window" | "door" | "gate") {
            let wall_spines: Vec<(u32, Vec<Vec2>)> = editor
                .pieces
                .iter()
                .filter_map(|p| movers::glade_wall_spine(p, &movers).map(|pts| (p.id, pts)))
                .collect();
            if let Some((pos, rot, wall_elev)) =
                crate::glade::snap_door_to_glade_wall_with_elevation(
                    &raw,
                    &editor.pieces,
                    cursor.ray,
                    &wall_spines,
                )
            {
                (
                    model::Snap {
                        pos,
                        rot,
                        kind: SnapKind::Anchor,
                    },
                    wall_elev,
                )
            } else if matches!(shape.id, "glade_door" | "glade_window") {
                // Building doors and windows must attach to a building wall;
                // do not show a ground ghost or allow placing them in open space.
                editor.ghost = None;
                if clicked {
                    editor.note = "click on a building wall to place a door or window";
                }
                return;
            } else {
                let s = model::snap_piece(
                    &raw,
                    shape,
                    &editor.pieces,
                    editor.snap,
                    editor.snap,
                    SNAP_THRESHOLD,
                );
                let h = crate::glade::world_terrain_height_at(s.pos, &editor.pieces);
                (s, h)
            }
        } else {
            let s = model::snap_piece(
                &raw,
                shape,
                &editor.pieces,
                editor.snap,
                editor.snap,
                SNAP_THRESHOLD,
            );
            let h = if shape.id == "glade_terrain" {
                0.0
            } else {
                crate::glade::world_terrain_height_at(s.pos, &editor.pieces)
            };
            (s, h)
        };
        editor.ghost = Some(Ghost {
            pos: snap.pos,
            rot: snap.rot,
            elevation: snap_elevation,
            kind: snap.kind,
        });

        // Continuous Tiny Glade terrain brush: when `glade_terrain` is armed,
        // clicking and dragging continuously raises/extends a plateau ridge
        // across the landscape (or Shift+drag lowers/carves terrain), keeping
        // the brush armed for multi-stroke sculpting.
        if shape.id == "glade_terrain" {
            let held = input.pressed(Action::PrimaryClick);
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
            let lower = shift || editor.grass_erase;
            if clicked || held {
                if apply_terrain_brush_dab(&mut editor, world, lower) {
                    editor.note = if lower {
                        "lowered / carved terrain (drag to sculpt)"
                    } else {
                        "sculpted terrain plateau (drag to extend, Shift+drag to lower)"
                    };
                }
                if !held {
                    finish_terrain_stroke(&mut editor);
                }
            }
            if input.just_released(Action::PrimaryClick) && editor.terrain_stroke {
                finish_terrain_stroke(&mut editor);
            }
            return;
        }

        // Dedicated dirt/stone terrain path tool: clicking starts a `glade_path`
        // piece and subsequent clicks extend its polyline/spline until Esc or
        // another tool is chosen.
        if shape.id == "glade_path" {
            if clicked {
                if let Some(sel_id) = editor.selection.primary()
                    && let Some(sel_piece) = editor.pieces.iter().find(|p| p.id == sel_id).copied()
                    && shapes()
                        .get(sel_piece.shape)
                        .is_some_and(|s| s.id == "glade_path")
                {
                    let last_pos = movers
                        .map
                        .get(&sel_id)
                        .and_then(|m| m.waypoints.last())
                        .map(|w| w.pos)
                        .unwrap_or(sel_piece.pos);
                    if snap.pos.distance(last_pos) > 0.15 {
                        editor.commit();
                        let mover = movers.mover_mut(sel_id, sel_piece.pos);
                        mover
                            .waypoints
                            .push(crate::platforms::PlatformWaypoint::new(snap.pos));
                        mover.selected = Some(mover.waypoints.len() - 1);
                        mover.rebuild();
                        editor.hard_dirty = true;
                        editor.note = "path extended (Esc to finish)";
                    }
                } else {
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
                        elevation: snap_elevation,
                        dynamic: false,
                        trigger: default_trigger,
                        trigger_size: default_size,
                        trigger_shape_override: crate::model::TriggerShape::Box,
                        trigger_alpha: default_alpha,
                        trigger_color: default_color,
                        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                        interact: crate::model::InteractAction::None,
                    });
                    editor.selection.set(id);
                    movers.mover_mut(id, snap.pos);
                    editor.hard_dirty = true;
                    editor.note = "path started — click to add points (Esc to finish)";
                }
            }
            return;
        }

        if clicked {
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
            let keep_stamping = keeps_stamping(shift, touch.append);

            // Chained path-wall drawing: when stamping a `glade_wall` with Shift or
            // mobile `Add` while an existing `glade_wall` is selected, append the clicked
            // point to that wall's curved path spine instead of spawning a separate piece!
            if shape.id == "glade_wall"
                && keep_stamping
                && let Some(sel_id) = editor.selection.primary()
                && let Some(sel_piece) = editor.pieces.iter().find(|p| p.id == sel_id).copied()
                && shapes()
                    .get(sel_piece.shape)
                    .is_some_and(|s| s.id == "glade_wall")
                && snap.pos.distance(sel_piece.pos) > 0.15
            {
                editor.commit();
                let mover = movers.mover_mut(sel_id, sel_piece.pos);
                mover
                    .waypoints
                    .push(crate::platforms::PlatformWaypoint::new(snap.pos));
                mover.selected = Some(mover.waypoints.len() - 1);
                mover.rebuild();
                editor.hard_dirty = true;
                editor.note = "wall path extended (Shift/Add to keep drawing)";
                return;
            }

            editor.commit();
            let id = editor.next_id;
            editor.next_id += 1;
            let initial_elevation = snap_elevation;
            let placed = Piece {
                id,
                shape: tool,
                pos: snap.pos,
                rot: snap.rot,
                pitch: 0.0,
                roll: 0.0,
                elevation: initial_elevation,
                dynamic: false,
                trigger: default_trigger,
                trigger_size: default_size,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: default_alpha,
                trigger_color: default_color,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                interact: crate::model::InteractAction::None,
            };
            editor.pieces.push(placed);
            editor.selection.set(id);
            if let Some(host_id) = crate::glade::find_host_building(&placed, &editor.pieces) {
                editor.auto_group_with_host(id, host_id);
            }

            // If a new `glade_wall` is placed onto an active `Plan` curve, adopt
            // the plan's waypoints (including Bezier tangents) so the wall follows the path!
            let mut followed_plan = false;
            if shape.id == "glade_wall"
                && let Some(plan) = &editor.plan
                && plan.waypoints.len() >= 2
                && plan
                    .waypoints
                    .iter()
                    .any(|w| w.pos.distance(snap.pos) <= 2.0)
            {
                let plan_wps: Vec<crate::platforms::PlatformWaypoint> = plan
                    .waypoints
                    .iter()
                    .map(|w| crate::platforms::PlatformWaypoint {
                        pos: w.pos,
                        incoming: w.incoming,
                        outgoing: w.outgoing,
                    })
                    .collect();
                let mover = movers.mover_mut(id, snap.pos);
                mover.waypoints = plan_wps;
                mover.rebuild();
                followed_plan = true;
            }

            editor.note = if followed_plan {
                "placed: wall following plan path"
            } else {
                match snap.kind {
                    SnapKind::Anchor => "placed: anchor snap",
                    SnapKind::Grid => "placed: grid snap",
                    SnapKind::Free => "placed: free",
                }
            };
            // One drop per palette click. Holding Shift (or the sticky touch
            // Add) keeps the stamp armed, so a row of blocks is a Shift-click
            // each, while a plain click hands the pointer back to selection so
            // the piece can be moved straight away.
            if !keep_stamping {
                editor.tool = None;
                editor.ghost = None;
                if !followed_plan {
                    editor.note = match snap.kind {
                        SnapKind::Anchor => "placed: anchor snap (Shift keeps stamping)",
                        SnapKind::Grid => "placed: grid snap (Shift keeps stamping)",
                        SnapKind::Free => "placed: free (Shift keeps stamping)",
                    };
                }
            }
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
            editor.selection.clear();
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
            editor.selection.clear();
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
            let id = match editor.selection.primary() {
                Some(id) if editor.pieces.iter().any(|p| p.id == id) => id,
                _ => {
                    // No valid selection: a click on a piece selects it so
                    // the following clicks can lay down its path.
                    if let Some(hit) = cursor
                        .ray
                        .and_then(|ray| pick_unlocked(&editor.pieces, ray, &editor.outline.locked))
                    {
                        editor.selection.set(hit);
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
            editor.selection.clear();
            editor.drag = None;
        } else if playing {
            // Play routes the capsule; it never selects or drags objects.
        } else if let Some(hit) = hit {
            // Shift (or the sticky touch Add) grows the selection instead of
            // replacing it. A plain click on a piece that is already part of a
            // multi-selection keeps the group, so the whole set can be
            // dragged from any of its pieces.
            let shift =
                input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            if click_selection(&mut editor, hit, append) {
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
                        group: group_offsets(&editor, hit, piece.pos),
                    });
                }
                editor.commit();
            } else {
                editor.drag = None;
                editor.note = if editor.selection.is_empty() {
                    "nothing selected"
                } else {
                    "selection"
                };
            }
        } else {
            editor.selection.clear();
            editor.drag = None;
        }
    }

    if !playing
        && input.pressed(Action::PrimaryClick)
        && let Some(drag) = editor.drag.clone()
    {
        let base = cursor
            .ray
            .and_then(|ray| drag_plane_point(ray, drag.plane))
            .unwrap_or(world);
        apply_body_drag(&mut editor, drag, base);
    }

    if !playing
        && input.just_released(Action::PrimaryClick)
        && let Some(drag) = editor.drag.clone()
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

/// Whether a placement keeps the stamp armed: holding Shift (or the sticky
/// touch Add) asks for another block, while a plain click hands the pointer
/// back to selection so the piece can be moved straight away.
pub(super) fn keeps_stamping(shift: bool, touch_add: bool) -> bool {
    append_for(shift, touch_add)
}

/// Apply a click on `hit` to the selection. Shift-clicking (or the sticky
/// touch Add) toggles membership; a plain click on a piece outside the
/// selection replaces it, and a plain click on a piece that is already
/// selected keeps the group so the whole set can be dragged from any member.
///
/// Returns whether the click should start a body drag from `hit`.
pub(super) fn click_selection(editor: &mut Editor, hit: u32, append: bool) -> bool {
    if append {
        editor.selection.toggle(hit);
        return false;
    }
    if !editor.selection.contains(hit) {
        editor.selection.set(hit);
    }
    true
}

/// The other selected pieces as `(id, offset)` measured from the grabbed
/// piece, so one drag carries the whole selection rigidly.
pub(super) fn group_offsets(editor: &Editor, anchor: u32, anchor_pos: Vec2) -> Vec<(u32, Vec2)> {
    let mut ids: Vec<u32> = editor
        .selection
        .ids()
        .iter()
        .copied()
        .filter(|id| *id != anchor)
        .collect();
    for mate_id in editor.attached_building_mates(anchor) {
        if !ids.contains(&mate_id) {
            ids.push(mate_id);
        }
    }
    ids.into_iter()
        .filter_map(|id| {
            editor
                .pieces
                .iter()
                .find(|piece| piece.id == id)
                .map(|piece| (id, piece.pos - anchor_pos))
        })
        .collect()
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
    let mut moving: Vec<u32> = drag.group.iter().map(|(id, _)| *id).collect();
    moving.push(drag.id);
    let candidates = snap_against(editor, &moving);
    let (snap, door_elev) = if matches!(shape.id, "glade_door" | "glade_window" | "door" | "gate")
        && let Some((pos, rot, wall_elev)) =
            crate::glade::snap_door_to_glade_wall_with_elevation(&raw, &candidates, None, &[])
    {
        (
            model::Snap {
                pos,
                rot,
                kind: SnapKind::Anchor,
            },
            Some(wall_elev),
        )
    } else {
        (
            model::snap_piece(
                &raw,
                shape,
                &candidates,
                editor.snap,
                false,
                SNAP_THRESHOLD,
            ),
            None,
        )
    };
    let (changed, delta_elev) = {
        let target = &mut editor.pieces[index];
        let new_elev = door_elev.unwrap_or(target.elevation);
        let delta_elev = new_elev - target.elevation;
        let changed = (target.pos - snap.pos).length_squared() > 1e-10
            || (target.rot - snap.rot).abs() > 1e-6
            || delta_elev.abs() > 1e-6;
        target.pos = snap.pos;
        target.rot = snap.rot;
        target.elevation = new_elev;
        (changed, delta_elev)
    };
    // The rest of the selection (and attached building items) follows the grabbed
    // piece rigidly, keeping the layout captured at grab time.
    for (id, offset) in &drag.group {
        if let Some(other) = editor.pieces.iter_mut().find(|p| p.id == *id) {
            other.pos = snap.pos + *offset;
            other.elevation += delta_elev;
        }
    }
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

/// Apply one continuous dab of the Tiny Glade terrain brush at `pos`.
/// When `lower` is false, raises or extends a wide plateau ridge along the
/// stroke; when `lower` is true, lowers or carves terrain (including carving
/// into the built-in hillside). Also lifts/lowers non-terrain pieces sitting
/// on the sculpted ground so buildings ride cleanly with the terrain.
pub(super) fn apply_terrain_brush_dab(editor: &mut Editor, pos: Vec2, lower: bool) -> bool {
    let Some(terrain_idx) = model::shape_index("glade_terrain") else {
        return false;
    };
    let terrain_shape = &shapes()[terrain_idx];
    let default_ext = crate::glade::default_size(terrain_shape);

    if !editor.terrain_stroke {
        editor.commit();
        editor.terrain_stroke = true;
        editor.terrain_stroke_dirty = false;
        editor.terrain_stroke_last_pos = None;

        let current_h = crate::glade::world_terrain_height_at(pos, &editor.pieces);
        let near_chunk_center = editor.pieces.iter().find_map(|p| {
            (p.shape == terrain_idx && p.elevation >= -0.05 && p.pos.distance(pos) <= 2.4)
                .then_some(p.trigger_size.y)
        });
        editor.terrain_stroke_target_h = if let Some(chunk_h) = near_chunk_center {
            (chunk_h + 1.45).clamp(
                crate::glade::TERRAIN_MIN_HEIGHT,
                crate::glade::TERRAIN_MAX_HEIGHT,
            )
        } else if current_h > 0.45 {
            current_h.clamp(default_ext.y, crate::glade::TERRAIN_MAX_HEIGHT)
        } else {
            default_ext.y
        };
    }

    if let Some(last_pos) = editor.terrain_stroke_last_pos
        && last_pos.distance(pos) < 1.35
        && editor.terrain_stroke_dirty
    {
        return false;
    }
    let prev_pos = editor.terrain_stroke_last_pos;
    editor.terrain_stroke_last_pos = Some(pos);

    let mut changed = false;
    if lower {
        let mut to_remove = Vec::new();
        for piece in &mut editor.pieces {
            if piece.shape != terrain_idx || piece.elevation < -0.05 {
                continue;
            }
            let d = piece.pos.distance(pos);
            if d <= 3.6 {
                piece.trigger_size.y -= 1.45;
                changed = true;
                if piece.trigger_size.y < crate::glade::TERRAIN_MIN_HEIGHT + 0.08 {
                    to_remove.push(piece.id);
                }
            } else if d <= 6.2 {
                piece.trigger_size.y =
                    (piece.trigger_size.y - 0.65).max(crate::glade::TERRAIN_MIN_HEIGHT);
                changed = true;
            }
        }
        if !to_remove.is_empty() {
            editor.pieces.retain(|p| !to_remove.contains(&p.id));
        }

        // If carving into the built-in hillside or perimeter hills, spawn or
        // deepen a negative-elevation carve node at `pos`.
        if crate::glade::base_meadow_height_at(pos) > 0.15 {
            if let Some(carve) = editor
                .pieces
                .iter_mut()
                .find(|p| p.shape == terrain_idx && p.elevation < -0.05 && p.pos.distance(pos) <= 3.0)
            {
                carve.trigger_size.y =
                    (carve.trigger_size.y + 1.2).min(crate::glade::TERRAIN_MAX_HEIGHT);
                changed = true;
            } else {
                let id = editor.next_id;
                editor.next_id += 1;
                editor.pieces.push(Piece {
                    id,
                    shape: terrain_idx,
                    pos,
                    rot: 0.0,
                    pitch: 0.0,
                    roll: 0.0,
                    elevation: -1.0,
                    dynamic: false,
                    trigger: crate::model::Trigger::harmless(),
                    trigger_size: Vec3::new(9.5, 2.6, 9.5),
                    trigger_shape_override: crate::model::TriggerShape::Box,
                    trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
                    trigger_color: crate::glade::DEFAULT_TERRAIN_COLOR,
                    trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                    interact: crate::model::InteractAction::None,
                });
                changed = true;
            }
        }
    } else {
        // Remove any negative carve node under the raising brush
        editor.pieces.retain(|p| {
            !(p.shape == terrain_idx && p.elevation < -0.05 && p.pos.distance(pos) <= 3.5)
        });

        let target_h = editor.terrain_stroke_target_h;
        let stroke_rot = prev_pos
            .filter(|p| p.distance(pos) > 0.25)
            .map(|p| {
                let d = pos - p;
                d.x.atan2(d.y)
            })
            .unwrap_or(editor.ghost_rot);

        if let Some(existing) = editor
            .pieces
            .iter_mut()
            .filter(|p| p.shape == terrain_idx && p.elevation >= -0.05)
            .min_by(|a, b| {
                a.pos
                    .distance(pos)
                    .partial_cmp(&b.pos.distance(pos))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .filter(|p| p.pos.distance(pos) <= 2.8)
        {
            let new_h = existing.trigger_size.y.max(target_h);
            let new_w = (existing.trigger_size.x + 1.2).min(16.0);
            let new_l = (existing.trigger_size.z + 1.2).min(16.0);
            existing.trigger_size = Vec3::new(new_w, new_h, new_l);
            let id = existing.id;
            editor.selection.set(id);
            changed = true;
        } else {
            let id = editor.next_id;
            editor.next_id += 1;
            editor.pieces.push(Piece {
                id,
                shape: terrain_idx,
                pos,
                rot: stroke_rot,
                pitch: 0.0,
                roll: 0.0,
                elevation: 0.0,
                dynamic: false,
                trigger: crate::model::Trigger::harmless(),
                trigger_size: Vec3::new(default_ext.x, target_h, default_ext.z),
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
                trigger_color: crate::glade::DEFAULT_TERRAIN_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
                interact: crate::model::InteractAction::None,
            });
            editor.selection.set(id);
            changed = true;
        }
    }

    if changed {
        // Lift or lower non-terrain pieces within the brush's influence radius
        // so buildings, towers, walls, and trees ride smoothly on the terrain.
        let snapshot = editor.pieces.clone();
        for piece in &mut editor.pieces {
            if piece.shape == terrain_idx {
                continue;
            }
            if piece.pos.distance(pos) <= 9.5 {
                let ground_h = crate::glade::world_terrain_height_at(piece.pos, &snapshot);
                if (piece.elevation - ground_h).abs() <= 4.5 {
                    piece.elevation = ground_h;
                }
            }
        }
        editor.terrain_stroke_dirty = true;
        editor.hard_dirty = true;
    }

    changed
}

pub(super) fn finish_terrain_stroke(editor: &mut Editor) {
    if !editor.terrain_stroke_dirty {
        editor.undo.pop();
    }
    editor.terrain_stroke = false;
    editor.terrain_stroke_dirty = false;
    editor.terrain_stroke_last_pos = None;
}
