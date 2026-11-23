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
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
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
        // Brush footprint preview: all hexes in the current brush radius plus
        // an outer circular brush ring draped over the terrain, amber while erasing.
        if let Some(p) = cursor.last_world {
            let colour = if editor.grass_erase {
                AMBER
            } else {
                Color::srgb_u8(0x3f, 0xa0, 0x4d)
            };
            let center_hex = model::pixel_to_hex(p);
            for hex in brush_cells_with_radius(center_hex, editor.grass_brush_radius) {
                let points: Vec<Vec3> = model::hex_corners(model::hex_center(hex.0, hex.1))
                    .iter()
                    .map(|q| {
                        let h = crate::glade::world_terrain_height_at(*q, &editor.pieces);
                        Vec3::new(q.x, h + GRID_Y + 0.018, q.y)
                    })
                    .collect();
                closed(&mut gizmos, &points, colour);
            }
            let center_pt = model::hex_center(center_hex.0, center_hex.1);
            let ring_r = (editor.grass_brush_radius as f32 + 0.65) * model::HEX_SIZE * 1.5;
            let ring_segs = 32;
            let ring_pts: Vec<Vec3> = (0..ring_segs)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / ring_segs as f32;
                    let pt = center_pt + Vec2::new(a.cos(), a.sin()) * ring_r;
                    let h = crate::glade::world_terrain_height_at(pt, &editor.pieces);
                    Vec3::new(pt.x, h + GRID_Y + 0.025, pt.y)
                })
                .collect();
            closed(&mut gizmos, &ring_pts, colour.with_alpha(0.85));
        }
    }

    if editor.actor_tool == Some(ActorTool::Vine) {
        let filter = player
            .iter()
            .next()
            .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
            .unwrap_or_default();
        let hit = cursor.ray.and_then(|ray| {
            rapier.single().ok().and_then(|ctx| {
                ctx.cast_ray_and_get_normal(ray.origin, ray.direction.into(), 80.0, true, filter)
                    .map(|(_, h)| (h.point, h.normal))
            })
        });
        let fallback = cursor
            .last_world
            .map(|p| (Vec3::new(p.x, 0.0, p.y), Vec3::Y));
        if let Some((point, normal)) = hit.or(fallback) {
            let n = normal.normalize_or_zero();
            let n = if n.length_squared() < 0.5 { Vec3::Y } else { n };
            let (u, v) = n.any_orthonormal_pair();
            let radius = if editor.vine_erase {
                VINE_ERASE_RADIUS
            } else {
                VINE_BRUSH_RADIUS
            };
            let colour = if editor.vine_erase {
                AMBER
            } else {
                Color::srgb_u8(0x3f, 0xa0, 0x4d)
            };
            let segs = 24;
            let ring_pts: Vec<Vec3> = (0..segs)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / segs as f32;
                    point + n * 0.03 + (u * a.cos() + v * a.sin()) * radius
                })
                .collect();
            closed(&mut gizmos, &ring_pts, colour);
            gizmos.line(point, point + n * 0.18, colour);
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
            if !movers::is_glade_curve_shape(shape) {
                selection_box(&mut gizmos, piece, shape);
            }
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

    // Moving-platform and spline paths: Tiny Glade–style white curve controls
    // for selected `glade_wall` / `glade_path` pieces, and amber/muted paths
    // for moving platforms.
    for piece in &editor.pieces {
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        let is_glade_wall = shape.id == "glade_wall";
        let is_glade_stairs = shape.id == "glade_stairs";
        let is_glade_path = shape.id == "glade_path";
        let is_glade_curve = is_glade_wall || is_glade_stairs || is_glade_path;
        let Some(mover) = movers.map.get(&piece.id) else {
            continue;
        };
        if !is_glade_path && mover.waypoints.len() < 2 && mover.track.is_none() {
            continue;
        }
        let is_selected = editor.selection.is_primary(piece.id);
        if is_glade_curve && (!is_selected || !object_editing_allowed(editor.mode)) {
            if editor.actor_tool == Some(ActorTool::Platform)
                && let Some(track) = mover.track.as_ref()
            {
                let y = piece.elevation + 0.05;
                let line: Vec<Vec3> = track
                    .points()
                    .iter()
                    .map(|p| Vec3::new(p.x, y, p.y))
                    .collect();
                if line.len() >= 2 {
                    gizmos.linestrip(line, MUTED);
                }
            }
            continue;
        }

        if is_glade_curve {
            let white = Color::srgba(1.0, 1.0, 1.0, 0.95);
            let wire = Color::srgba(1.0, 1.0, 1.0, 0.72);
            let wp_count = mover.waypoints.len().max(2);
            let track_pts = mover.track.as_ref().map(|t| t.points().to_vec());
            let base_y_at = |pt: Vec2| -> f32 {
                if is_glade_wall {
                    crate::glade::wall_surface_y_at(piece, pt, &editor.pieces, track_pts.as_deref())
                        + 0.05
                } else {
                    piece.elevation + 0.05
                }
            };
            let handle_y_at = |pt: Vec2, frac: f32| -> f32 {
                glade_curve_handle_y(
                    piece,
                    Some(shape),
                    &editor.pieces,
                    track_pts.as_deref(),
                    pt,
                    frac,
                )
            };
            if let Some(pts) = track_pts.as_ref()
                && pts.len() >= 2
            {
                for pair in pts.windows(2) {
                    dashed_line(
                        &mut xray_gizmos,
                        Vec3::new(pair[0].x, base_y_at(pair[0]), pair[0].y),
                        Vec3::new(pair[1].x, base_y_at(pair[1]), pair[1].y),
                        wire,
                    );
                }
                if is_glade_wall {
                    let top_line: Vec<Vec3> = pts
                        .iter()
                        .map(|p| Vec3::new(p.x, handle_y_at(*p, 0.0), p.y))
                        .collect();
                    xray_gizmos.linestrip(top_line, white);
                    for wp in &mover.waypoints {
                        dashed_line(
                            &mut xray_gizmos,
                            Vec3::new(wp.pos.x, base_y_at(wp.pos), wp.pos.y),
                            Vec3::new(wp.pos.x, handle_y_at(wp.pos, 0.0), wp.pos.y),
                            wire,
                        );
                    }
                } else if is_glade_stairs {
                    let last_i = (pts.len() - 1).max(1) as f32;
                    let slope_line: Vec<Vec3> = pts
                        .iter()
                        .enumerate()
                        .map(|(i, p)| Vec3::new(p.x, handle_y_at(*p, i as f32 / last_i), p.y))
                        .collect();
                    xray_gizmos.linestrip(slope_line, white);
                    for (index, wp) in mover.waypoints.iter().enumerate() {
                        let frac = index as f32 / (wp_count - 1) as f32;
                        dashed_line(
                            &mut xray_gizmos,
                            Vec3::new(wp.pos.x, base_y_at(wp.pos), wp.pos.y),
                            Vec3::new(wp.pos.x, handle_y_at(wp.pos, frac), wp.pos.y),
                            wire,
                        );
                    }
                } else {
                    let path_line: Vec<Vec3> = pts
                        .iter()
                        .map(|p| Vec3::new(p.x, handle_y_at(*p, 0.0), p.y))
                        .collect();
                    xray_gizmos.linestrip(path_line, wire);
                }
            }

            let controls = mover.controls();
            for (index, waypoint) in mover.waypoints.iter().enumerate() {
                let frac = index as f32 / (wp_count - 1) as f32;
                let hy = handle_y_at(waypoint.pos, frac);
                let at = Vec3::new(waypoint.pos.x, hy, waypoint.pos.y);
                let selected_waypoint = mover.selected == Some(index);
                if selected_waypoint
                    && (is_glade_wall
                        || is_glade_stairs
                        || editor.actor_tool == Some(ActorTool::Platform))
                {
                    mover_waypoint_gizmo(&mut gizmos, at);
                }
                if let Some(ctrl) = controls.get(index) {
                    let tan_entries = [
                        ctrl.incoming
                            .map(|c| (c, ((index as f32 - 0.33).max(0.0)) / (wp_count - 1) as f32)),
                        ctrl.outgoing.map(|c| {
                            (
                                c,
                                ((index as f32 + 0.33).min((wp_count - 1) as f32))
                                    / (wp_count - 1) as f32,
                            )
                        }),
                    ];
                    for (control, tan_frac) in tan_entries.into_iter().flatten() {
                        let handle =
                            Vec3::new(control.x, handle_y_at(control, tan_frac), control.y);
                        dashed_line(
                            &mut xray_gizmos,
                            at,
                            handle,
                            if selected_waypoint {
                                AMBER
                            } else {
                                Color::srgba(0.35, 0.85, 1.0, 0.75)
                            },
                        );
                    }
                }
            }

            // Vertical drop lines from midpoint insert handles to ground.
            for (seg, midpoint) in mover.midpoints().into_iter().enumerate() {
                let hy = handle_y_at(midpoint, (seg as f32 + 0.5) / (wp_count - 1) as f32);
                let mid = Vec3::new(midpoint.x, hy, midpoint.y);
                if is_glade_wall || is_glade_stairs {
                    dashed_line(
                        &mut xray_gizmos,
                        Vec3::new(midpoint.x, base_y_at(midpoint), midpoint.y),
                        mid,
                        wire,
                    );
                }
            }
            continue;
        }

        let colour = if is_selected { AMBER } else { MUTED };
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
        let controls = if is_selected {
            mover.controls()
        } else {
            Vec::new()
        };
        for (index, waypoint) in mover.waypoints.iter().enumerate() {
            let at = Vec3::new(waypoint.pos.x, y, waypoint.pos.y);
            let selected_waypoint = is_selected && mover.selected == Some(index);
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

#[derive(Component)]
pub(super) struct BezierControlMesh;

struct BezierControlAssets {
    puck: Handle<Mesh>,
    sphere: Handle<Mesh>,
    rod: Handle<Mesh>,
    box_mesh: Handle<Mesh>,
    white_mat: Handle<StandardMaterial>,
    amber_mat: Handle<StandardMaterial>,
    cyan_mat: Handle<StandardMaterial>,
    stem_mat: Handle<StandardMaterial>,
    dark_rim_mat: Handle<StandardMaterial>,
}

#[derive(Default)]
pub(super) struct BezierControlPool {
    assets: Option<BezierControlAssets>,
    entities: Vec<Entity>,
}

fn unlit_solid_mat(
    materials: &mut Assets<StandardMaterial>,
    color: Color,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: color,
        unlit: true,
        ..default()
    })
}

/// Sync solid 3D Bezier control objects (waypoint knobs, tangent spheres + solid
/// stems, and midpoint `+` insert buttons) for the selected `glade_wall`,
/// `glade_stairs`, or `glade_path` piece. Uses a pooled set of entities so
/// handles are crisp solid 3D objects that are easy to see and grab on mobile.
#[allow(clippy::too_many_arguments)]
pub(super) fn sync_bezier_control_meshes(
    mut commands: Commands,
    editor: Res<Editor>,
    movers: Res<Movers>,
    touch: Res<TouchState>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut pool: Local<BezierControlPool>,
    mut existing: Query<
        (
            &mut Mesh3d,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Transform,
            &mut Visibility,
        ),
        With<BezierControlMesh>,
    >,
) {
    let (Some(meshes), Some(materials)) = (meshes.as_deref_mut(), materials.as_deref_mut()) else {
        return;
    };
    let assets = pool.assets.get_or_insert_with(|| BezierControlAssets {
        puck: meshes.add(Cylinder::new(1.0, 1.0)),
        sphere: meshes.add(Sphere::new(1.0)),
        rod: meshes.add(Cylinder::new(1.0, 1.0)),
        box_mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        white_mat: unlit_solid_mat(materials, Color::srgb(0.98, 0.98, 0.99)),
        amber_mat: unlit_solid_mat(materials, AMBER),
        cyan_mat: unlit_solid_mat(materials, Color::srgb(0.18, 0.84, 1.0)),
        stem_mat: unlit_solid_mat(materials, Color::srgb(0.55, 0.90, 1.0)),
        dark_rim_mat: unlit_solid_mat(materials, Color::srgb(0.12, 0.15, 0.20)),
    });

    let mut specs: Vec<(Handle<Mesh>, Handle<StandardMaterial>, Transform)> = Vec::new();
    if object_editing_allowed(editor.mode)
        && let Some(id) = editor.selection.primary()
        && let Some(piece) = editor.pieces.iter().find(|p| p.id == id)
        && let Some(shape) = shapes().get(piece.shape)
        && matches!(shape.id, "glade_wall" | "glade_stairs" | "glade_path")
        && let Some(mover) = movers.map.get(&id)
        && mover.waypoints.len() >= 2
    {
        let wp_count = mover.waypoints.len().max(2);
        let track_pts = mover.track.as_ref().map(|t| t.points());
        let handle_y_at = |pt: Vec2, frac: f32| -> f32 {
            glade_curve_handle_y(piece, Some(shape), &editor.pieces, track_pts, pt, frac)
        };

        let node_r = if touch.enabled { 0.23 } else { 0.15 };
        let puck_h = if touch.enabled { 0.13 } else { 0.09 };
        let tan_r = if touch.enabled { 0.17 } else { 0.115 };
        let stem_r = if touch.enabled { 0.032 } else { 0.022 };
        let mid_r = if touch.enabled { 0.20 } else { 0.14 };
        let mid_h = if touch.enabled { 0.09 } else { 0.065 };
        let bar_len = mid_r * 1.35;
        let bar_t = mid_r * 0.28;

        let controls = mover.controls();
        for (index, waypoint) in mover.waypoints.iter().enumerate() {
            let frac = index as f32 / (wp_count - 1) as f32;
            let at = Vec3::new(
                waypoint.pos.x,
                handle_y_at(waypoint.pos, frac),
                waypoint.pos.y,
            );
            let selected = mover.selected == Some(index);
            let knob_mat = if selected {
                assets.amber_mat.clone()
            } else {
                assets.white_mat.clone()
            };

            // Dark contrast collar + solid waypoint puck + dome cap.
            specs.push((
                assets.puck.clone(),
                assets.dark_rim_mat.clone(),
                Transform::from_translation(at - Vec3::Y * (puck_h * 0.22)).with_scale(Vec3::new(
                    node_r * 1.24,
                    puck_h * 0.7,
                    node_r * 1.24,
                )),
            ));
            specs.push((
                assets.puck.clone(),
                knob_mat.clone(),
                Transform::from_translation(at).with_scale(Vec3::new(node_r, puck_h, node_r)),
            ));
            specs.push((
                assets.sphere.clone(),
                knob_mat.clone(),
                Transform::from_translation(at + Vec3::Y * (puck_h * 0.45))
                    .with_scale(Vec3::splat(node_r * 0.62)),
            ));

            if let Some(ctrl) = controls.get(index) {
                let tan_mat = if selected {
                    assets.amber_mat.clone()
                } else {
                    assets.cyan_mat.clone()
                };
                let tan_entries = [
                    ctrl.incoming
                        .map(|c| (c, ((index as f32 - 0.33).max(0.0)) / (wp_count - 1) as f32)),
                    ctrl.outgoing.map(|c| {
                        (
                            c,
                            ((index as f32 + 0.33).min((wp_count - 1) as f32))
                                / (wp_count - 1) as f32,
                        )
                    }),
                ];
                for (control, tan_frac) in tan_entries.into_iter().flatten() {
                    let handle = Vec3::new(control.x, handle_y_at(control, tan_frac), control.y);
                    let delta = handle - at;
                    let dist = delta.length();
                    if dist > 0.02 {
                        let rot = Quat::from_rotation_arc(Vec3::Y, delta / dist);
                        specs.push((
                            assets.rod.clone(),
                            if selected {
                                assets.amber_mat.clone()
                            } else {
                                assets.stem_mat.clone()
                            },
                            Transform::from_translation((at + handle) * 0.5)
                                .with_rotation(rot)
                                .with_scale(Vec3::new(stem_r, dist, stem_r)),
                        ));
                    }
                    // Dark collar + solid Bezier tangent sphere.
                    specs.push((
                        assets.puck.clone(),
                        assets.dark_rim_mat.clone(),
                        Transform::from_translation(handle - Vec3::Y * (tan_r * 0.35))
                            .with_scale(Vec3::new(tan_r * 1.18, tan_r * 0.35, tan_r * 1.18)),
                    ));
                    specs.push((
                        assets.sphere.clone(),
                        tan_mat.clone(),
                        Transform::from_translation(handle).with_scale(Vec3::splat(tan_r)),
                    ));
                }
            }
        }

        // Solid `+` insert/split buttons at segment midpoints.
        for (seg, midpoint) in mover.midpoints().into_iter().enumerate() {
            let hy = handle_y_at(midpoint, (seg as f32 + 0.5) / (wp_count - 1) as f32);
            let mid = Vec3::new(midpoint.x, hy, midpoint.y);
            specs.push((
                assets.puck.clone(),
                assets.dark_rim_mat.clone(),
                Transform::from_translation(mid).with_scale(Vec3::new(mid_r, mid_h, mid_r)),
            ));
            let plus_y = mid + Vec3::Y * (mid_h * 0.52);
            specs.push((
                assets.box_mesh.clone(),
                assets.white_mat.clone(),
                Transform::from_translation(plus_y).with_scale(Vec3::new(
                    bar_len,
                    bar_t * 0.65,
                    bar_t,
                )),
            ));
            specs.push((
                assets.box_mesh.clone(),
                assets.white_mat.clone(),
                Transform::from_translation(plus_y).with_scale(Vec3::new(
                    bar_t,
                    bar_t * 0.65,
                    bar_len,
                )),
            ));
        }
    }

    for (index, (mesh, mat, transform)) in specs.iter().enumerate() {
        if let Some(&entity) = pool.entities.get(index)
            && let Ok((mut m, mut mm, mut t, mut vis)) = existing.get_mut(entity)
        {
            m.0 = mesh.clone();
            mm.0 = mat.clone();
            *t = *transform;
            *vis = Visibility::Inherited;
            continue;
        }
        let entity = commands
            .spawn((
                BezierControlMesh,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                *transform,
            ))
            .id();
        if index < pool.entities.len() {
            pool.entities[index] = entity;
        } else {
            pool.entities.push(entity);
        }
    }

    for &entity in pool.entities.iter().skip(specs.len()) {
        if let Ok((_, _, _, mut vis)) = existing.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
    }
}
