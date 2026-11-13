//! draw systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Drawing (gizmo overlays only; the solids are entities)
// ---------------------------------------------------------------------------

#[allow(clippy::collapsible_if)]
pub(super) fn draw(
    editor: Res<Editor>,
    movers: Res<Movers>,
    cursor: Res<Cursor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    mut gizmos: Gizmos,
    mut xray_gizmos: Gizmos<XrayGizmos>,
) {
    let (cq, cr) = model::pixel_to_hex(Vec2::new(orbit.target.x, orbit.target.z));
    for dq in -8..=8 {
        for dr in -8..=8 {
            let center = model::hex_center(cq + dq, cr + dr);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|p| {
                    let h = crate::glade::world_terrain_height_at(*p, &editor.pieces);
                    Vec3::new(p.x, h + GRID_Y, p.y)
                })
                .collect();
            closed(&mut gizmos, &points, GRID);
        }
    }

    // In Play the cursor only means something when a click can route, so it
    // appears with a character in third/top view.
    let show_hover = orbit.mode != ViewMode::First
        && (object_editing_allowed(editor.mode) || editor.character.is_some());
    if show_hover {
        if let Some(p) = cursor.last_world {
            let center = model::nearest_hex(p);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|q| {
                    let h = crate::glade::world_terrain_height_at(*q, &editor.pieces);
                    Vec3::new(q.x, h + GRID_Y + 0.012, q.y)
                })
                .collect();
            closed(&mut gizmos, &points, GRID_HOVER);

            // World cursor: a sphere on the snapped spot, a ring around it and
            // a short vertical spike so it reads in both top and first person.
            let spot = placement_spot(&editor, p);
            let spot_h = crate::glade::world_terrain_height_at(spot.0, &editor.pieces);
            let at = Vec3::new(spot.0.x, spot_h + 0.04, spot.0.y);
            let ring: Vec<Vec3> = (0..24)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 24.0;
                    at + Vec3::new(a.cos() * 0.22, 0.0, a.sin() * 0.22)
                })
                .collect();
            closed(&mut gizmos, &ring, PATH);
            gizmos.line(at, at + Vec3::Y * 0.45, PATH);
            gizmos.sphere(Isometry3d::new(at, Quat::IDENTITY), 0.05, PATH);
        }
    }

    // Live 3D concentric terrain brush ring when `glade_terrain` is armed.
    if editor
        .tool
        .and_then(|t| shapes().get(t))
        .is_some_and(|s| s.id == "glade_terrain")
        && let Some(p) = cursor.last_world
    {
        let brush_color = Color::srgb_u8(0xf4, 0xd3, 0x5e);
        let inner_color = Color::srgb_u8(0x98, 0xd8, 0x62);
        for (radius, col) in [(3.1, inner_color), (5.4, brush_color)] {
            let ring_pts: Vec<Vec3> = (0..40)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 40.0;
                    let q = p + Vec2::new(a.cos(), a.sin()) * radius;
                    let h = crate::glade::world_terrain_height_at(q, &editor.pieces);
                    Vec3::new(q.x, h + 0.05, q.y)
                })
                .collect();
            closed(&mut gizmos, &ring_pts, col);
        }
    }

    if editor.actor_tool == Some(ActorTool::Grass) {
        // Brush footprint preview: the dabbed hex plus its neighbours, amber
        // while erasing.
        if let Some(p) = cursor.last_world {
            let colour = if editor.grass_erase {
                AMBER
            } else {
                Color::srgb_u8(0x3f, 0xa0, 0x4d)
            };
            for hex in brush_cells(model::pixel_to_hex(p)) {
                let points: Vec<Vec3> = model::hex_corners(model::hex_center(hex.0, hex.1))
                    .iter()
                    .map(|q| Vec3::new(q.x, GRID_Y + 0.003, q.y))
                    .collect();
                closed(&mut gizmos, &points, colour);
            }
        }
    }

    if object_editing_allowed(editor.mode) {
        // Every selected piece gets a selection box. Only the primary carries
        // the transform gizmo, so a multi-selection stays readable and a
        // handle can never be ambiguous about which piece it moves.
        for id in editor.selection.ids() {
            let Some(piece) = editor.pieces.iter().find(|p| p.id == *id) else {
                continue;
            };
            let Some(shape) = shapes().get(piece.shape) else {
                continue;
            };
            selection_box(&mut gizmos, piece, shape);
            anchors(&mut gizmos, piece, shape);
            if orbit.mode != ViewMode::First && editor.selection.is_primary(*id) {
                transform_gizmo(&mut gizmos, &mut xray_gizmos, piece, shape, touch.enabled);
                if shape.is_glade() {
                    let y = piece.elevation + 0.03;
                    for (a, b) in crate::glade::boolean_union_outline(&editor.pieces) {
                        gizmos.line(
                            Vec3::new(a.x, y, a.y),
                            Vec3::new(b.x, y, b.y),
                            Color::srgb_u8(0x52, 0xd6, 0xf4),
                        );
                    }
                }
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
            };
            anchors(&mut gizmos, &piece, shape);
        }
    }

    if let Some(route) = &editor.route {
        let colour = if route.valid { PATH } else { AMBER };
        gizmos.linestrip(route.trail.iter().chain(route.line.iter()).copied(), colour);

        // A little target ring so the end of the curve is unambiguous, laid on
        // whatever surface the curve ends on.
        let target = route
            .line
            .last()
            .or_else(|| route.trail.last())
            .copied()
            .unwrap_or(Vec3::new(0.0, 0.07, 0.0));
        gizmos.circle(
            Isometry3d::new(target, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.22,
            colour,
        );
    }

    // Plan handles stay live in Play's top view so the Bezier curve can be
    // reshaped without going back to edit.
    if orbit.mode == ViewMode::Top
        && (editor.actor_tool == Some(ActorTool::Plan) || editor.mode == Mode::Play)
    {
        if let Some(plan) = &editor.plan {
            let controls = editor
                .route
                .as_ref()
                .map(|route| route.controls.as_slice())
                .unwrap_or(&[]);
            for (index, waypoint) in plan.waypoints.iter().enumerate() {
                let at = Vec3::new(waypoint.pos.x, waypoint.height + 0.06, waypoint.pos.y);
                let colour = if plan.selected == Some(waypoint.id) {
                    AMBER
                } else {
                    PATH
                };
                gizmos.circle(
                    Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    0.16,
                    colour,
                );
                gizmos.sphere(Isometry3d::new(at, Quat::IDENTITY), 0.06, colour);

                if let Some(controls) = controls.get(index) {
                    for control in [controls.incoming, controls.outgoing].into_iter().flatten() {
                        let handle = Vec3::new(control.x, waypoint.height + 0.06, control.y);
                        gizmos.line(at, handle, colour);
                        gizmos.sphere(Isometry3d::new(handle, Quat::IDENTITY), 0.05, colour);
                    }
                }
            }
        }
    }

    // Moving-platform and spline paths: amber for the selected piece, muted otherwise.
    // Drawn in Build and in Play so the motion stays readable while riding.
    for piece in &editor.pieces {
        let is_glade_path = shapes()
            .get(piece.shape)
            .is_some_and(|s| s.id == "glade_path");
        let Some(mover) = movers.map.get(&piece.id) else {
            continue;
        };
        if !is_glade_path && mover.waypoints.len() < 2 && mover.track.is_none() {
            continue;
        }
        let colour = if editor.selection.is_primary(piece.id) {
            AMBER
        } else {
            MUTED
        };
        let y = piece.elevation + 0.06;
        if let Some(track) = mover.track.as_ref() {
            let line: Vec<Vec3> = track
                .points()
                .iter()
                .map(|p| Vec3::new(p.x, y, p.y))
                .collect();
            if line.len() >= 2 {
                gizmos.linestrip(line, colour);
            }
        }
        let controls = if editor.selection.is_primary(piece.id) {
            mover.controls()
        } else {
            Vec::new()
        };
        for (index, waypoint) in mover.waypoints.iter().enumerate() {
            let at = Vec3::new(waypoint.pos.x, y, waypoint.pos.y);
            let selected_waypoint =
                editor.selection.is_primary(piece.id) && mover.selected == Some(index);
            let waypoint_colour = if selected_waypoint { AMBER } else { colour };
            gizmos.circle(
                Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                0.12,
                waypoint_colour,
            );
            gizmos.sphere(Isometry3d::new(at, Quat::IDENTITY), 0.05, waypoint_colour);
            // The selected waypoint gets a ground-plane move gizmo, the
            // same X/Z arrows the piece gizmo draws. Its shafts are live
            // handles: `mover_handle_at` grabs them like the dot.
            if selected_waypoint {
                mover_waypoint_gizmo(&mut gizmos, at);
            }
            // Tangent handles for the selected piece's path, like the plan.
            if let Some(ctrl) = controls.get(index) {
                for control in [ctrl.incoming, ctrl.outgoing].into_iter().flatten() {
                    let handle = Vec3::new(control.x, y, control.y);
                    gizmos.line(at, handle, waypoint_colour);
                    gizmos.sphere(
                        Isometry3d::new(handle, Quat::IDENTITY),
                        0.05,
                        waypoint_colour,
                    );
                }
            }
        }
    }

    if let Some(character) = editor.character {
        let feet = Vec3::new(character.pos.x, character.feet + 0.05, character.pos.y);
        gizmos.circle(
            Isometry3d::new(feet, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.44,
            PILL_COLOR,
        );
        actor_arrow(&mut gizmos, character);
    }
}

/// Where a click would put the thing you are holding: the ghost's snapped
/// point when a tool is armed, otherwise the nearest hex centre.
pub(super) fn placement_spot(editor: &Editor, world: Vec2) -> (Vec2, f32) {
    if let Some(ghost) = editor.ghost {
        (ghost.pos, ghost.rot)
    } else {
        (model::nearest_hex(world), 0.0)
    }
}
