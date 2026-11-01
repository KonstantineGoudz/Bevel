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
    mut gizmos: Gizmos,
) {
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

    // In Play the cursor only means something when a click can route, so it
    // appears with a character in third/top view.
    let show_hover = orbit.mode != ViewMode::First
        && (object_editing_allowed(editor.mode) || editor.character.is_some());
    if show_hover {
        if let Some(p) = cursor.last_world {
            let center = model::nearest_hex(p);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|q| Vec3::new(q.x, GRID_Y + 0.002, q.y))
                .collect();
            closed(&mut gizmos, &points, GRID_HOVER);

            // World cursor: a sphere on the snapped spot, a ring around it and
            // a short vertical spike so it reads in both top and first person.
            let spot = placement_spot(&editor, p);
            let at = Vec3::new(spot.0.x, 0.04, spot.0.y);
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

    if object_editing_allowed(editor.mode) {
        if let Some(id) = editor.selected {
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == id) {
                if let Some(shape) = shapes().get(piece.shape) {
                    selection_box(&mut gizmos, piece, shape);
                    anchors(&mut gizmos, piece, shape);
                    if orbit.mode != ViewMode::First {
                        transform_gizmo(&mut gizmos, piece, shape);
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

    // Moving-platform paths: amber for the selected piece, muted otherwise.
    // Drawn in Build and in Play so the motion stays readable while riding.
    for piece in &editor.pieces {
        let Some(mover) = movers.map.get(&piece.id) else {
            continue;
        };
        if mover.waypoints.len() < 2 && mover.track.is_none() {
            continue;
        }
        let colour = if editor.selected == Some(piece.id) {
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
        for waypoint in &mover.waypoints {
            let at = Vec3::new(waypoint.pos.x, y, waypoint.pos.y);
            gizmos.circle(
                Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                0.12,
                colour,
            );
        }
    }

    if let Some(character) = editor.character {
        let feet = Vec3::new(character.pos.x, character.feet + 0.05, character.pos.y);
        gizmos.circle(
            Isometry3d::new(feet, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.44,
            PILL_COLOR,
        );
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
