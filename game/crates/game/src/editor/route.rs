//! route systems for the level editor.

use super::*;

pub(super) fn surface_target(
    cursor: &Cursor,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    fallback: Vec2,
) -> (Vec2, f32) {
    cursor
        .ray
        .and_then(|ray| {
            spatial_query
                .cast_ray(ray.origin.real(), ray.direction, 80.0, true, filter)
                .map(|hit| ray.get_point(hit.distance))
        })
        .map(|point| (Vec2::new(point.x, point.z), point.y))
        .unwrap_or((fallback, 0.0))
}

/// Height of the walkable surface under `at`, ignoring walls' vertical faces.
pub(super) fn surface_height(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    at: Vec2,
) -> Option<f32> {
    let origin = Vec3::new(at.x, 8.0, at.y);
    spatial_query
        .cast_ray(origin.real(), Dir3::NEG_Y, 16.0, true, filter)
        .filter(|hit| hit.normal.y > 0.3)
        .map(|hit| origin.y - hit.distance)
}

/// Lift the flat track onto the geometry it crosses, so the drawn curve climbs
/// a ramp and sits on a block top instead of running along the floor below.
pub(super) fn route_line(
    track: &path::Track,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    start_height: f32,
) -> Vec<Vec3> {
    let points = track.points();
    if points.is_empty() {
        return Vec::new();
    }

    let heights = lift_heights(
        points.iter().copied(),
        |point| surface_height(spatial_query, filter, point),
        start_height,
    );
    let smoothed = smooth_heights(&heights);

    points
        .iter()
        .zip(smoothed)
        .map(|(point, height)| Vec3::new(point.x, height + 0.07, point.y))
        .collect()
}

/// Turn surface samples along the curve into a height profile.
///
/// * A sample far above the previous one is ignored (a wall top beside the
///   curve, not a step the walker could climb).
/// * A missing sample keeps the previous height.
/// * A large drop is only accepted if the footprint around the point has no
///   support; otherwise it is the seam between a ramp and a block, which the
///   capsule bridges, so the drawn curve should too.
pub(super) fn lift_heights(
    points: impl Iterator<Item = Vec2>,
    sample: impl Fn(Vec2) -> Option<f32>,
    start: f32,
) -> Vec<f32> {
    /// Half the capsule's width: any support this close counts as a seam.
    const FOOTPRINT: f32 = 0.25;
    const STEP_UP: f32 = 0.5;
    const DROP: f32 = 0.45;

    let mut heights = Vec::new();
    let mut previous = start;
    for point in points {
        let mut height = sample(point).unwrap_or(previous);
        if height > previous + STEP_UP {
            height = previous;
        } else if height < previous - DROP {
            let supported = [
                Vec2::new(FOOTPRINT, 0.0),
                Vec2::new(-FOOTPRINT, 0.0),
                Vec2::new(0.0, FOOTPRINT),
                Vec2::new(0.0, -FOOTPRINT),
            ]
            .into_iter()
            .any(|offset| {
                sample(point + offset).is_some_and(|near| (near - previous).abs() < 0.15)
            });
            if supported {
                height = previous;
            }
        }
        heights.push(height);
        previous = height;
    }
    heights
}

/// Three-tap smoothing so the curve ramps up instead of kinking at seams.
pub(super) fn smooth_heights(heights: &[f32]) -> Vec<f32> {
    (0..heights.len())
        .map(|i| {
            let before = heights[i.saturating_sub(1)];
            let here = heights[i];
            let after = heights[(i + 1).min(heights.len().saturating_sub(1))];
            (before + here * 2.0 + after) * 0.25
        })
        .collect()
}

/// A plan-mode click while stopped only lays out the route; Path mode and a
/// moving cylinder start walking.
pub(super) fn click_starts_walk(plan_mode: bool, walking: bool) -> bool {
    !plan_mode || walking
}

/// A plain click routes the capsule only in Play. In Build it selects, so
/// loading a course (or any stray click) never starts a walk. What the cursor
/// is over does not matter: Play never selects, and a click on a block or ramp
/// is a request to route onto it.
pub(super) fn click_routes(playing: bool, view: ViewMode, has_character: bool) -> bool {
    playing && view != ViewMode::First && has_character
}

/// Shift on desktop and the sticky Add toggle on touch both append.
pub(super) fn append_for(shift: bool, touch_add: bool) -> bool {
    shift || touch_add
}

/// The cell of the block/ramp under a point, or the rounded ground cell.
pub(super) fn resolve_cell(pieces: &[Piece], at: Vec2, height: f32) -> path::Hex {
    path::cell_under(pieces, at, height).unwrap_or_else(|| model::pixel_to_hex(at))
}

/// The drawn line up to the sample nearest `point`.
pub(super) fn line_prefix(track: &path::Track, line: &[Vec3], point: Vec2) -> Vec<Vec3> {
    if line.is_empty() {
        return Vec::new();
    }
    let index = track.nearest_index(point).min(line.len() - 1);
    line[..=index].to_vec()
}

/// Remove waypoints the walker has reached and trim the matching route data.
pub(super) fn prune_plan(editor: &mut Editor, walked: f32) {
    let Editor { plan, route, .. } = editor;
    if let (Some(plan), Some(route)) = (plan, route) {
        let removed = plan.prune(&route.distances, walked);
        if removed > 0 {
            route.distances.drain(..removed);
            route.controls.drain(..removed);
        }
    }
}

/// Extend the frozen walked trail with the walked part of the old suffix.
/// The suffix line starts where the existing trail ends, so its first point is
/// skipped once the trail is non-empty.
pub(super) fn walked_trail(
    trail: &[Vec3],
    old_track: &path::Track,
    old_line: &[Vec3],
    start: Vec2,
) -> Vec<Vec3> {
    let mut out = trail.to_vec();
    let walked = line_prefix(old_track, old_line, start);
    let skip = usize::from(!out.is_empty());
    out.extend(walked.into_iter().skip(skip));
    out
}

/// Where a live rebuild starts: the current track point, not the old start.
pub(super) fn rebuild_start(
    live: bool,
    route: Option<&Route>,
    walk: Option<&Walk>,
    character: Vec2,
) -> Vec2 {
    if live {
        if let (Some(route), Some(walk)) = (route, walk) {
            return route.track.at(walk.distance).0;
        }
    }
    character
}

/// Keep the old plan unless the new one built successfully.
pub(super) fn choose_plan(old: Option<Plan>, new: Option<Plan>, ok: bool) -> Option<Plan> {
    if ok { new } else { old }
}

/// A UI button press must not leak through as a canvas tap.
pub(super) fn tap_allowed(now: f64, touch: &TouchState) -> bool {
    now >= touch.suppress_until && !touch.multi
}

pub(super) fn within_radius(screen: Vec2, target: Vec2, radius: f32) -> bool {
    screen.distance(target) <= radius
}

/// Which waypoint is under a screen point, if any.
pub(super) fn waypoint_hit(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    radius: f32,
) -> Option<u32> {
    let plan = editor.plan.as_ref()?;
    let mut best: Option<(f32, u32)> = None;
    for waypoint in &plan.waypoints {
        let at = Vec3::new(waypoint.pos.x, waypoint.height + 0.06, waypoint.pos.y);
        let Ok(target) = camera.world_to_viewport(camera_transform, at) else {
            continue;
        };
        if !within_radius(screen, target, radius) {
            continue;
        }
        let distance = screen.distance(target);
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, waypoint.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Which tangent control is under a screen point, if any.
pub(super) fn tangent_hit(
    editor: &Editor,
    route: &Route,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    radius: f32,
) -> Option<(u32, TangentSide)> {
    let plan = editor.plan.as_ref()?;
    let mut best: Option<(f32, u32, TangentSide)> = None;
    for (waypoint, controls) in plan.waypoints.iter().zip(route.controls.iter()) {
        let height = waypoint.height + 0.06;
        let handles = [
            (controls.incoming, TangentSide::Incoming),
            (controls.outgoing, TangentSide::Outgoing),
        ];
        for (control, side) in handles {
            let Some(control) = control else {
                continue;
            };
            let at = Vec3::new(control.x, height, control.y);
            let Ok(target) = camera.world_to_viewport(camera_transform, at) else {
                continue;
            };
            if !within_radius(screen, target, radius) {
                continue;
            }
            let distance = screen.distance(target);
            if best.is_none_or(|(best_distance, _, _)| distance < best_distance) {
                best = Some((distance, waypoint.id, side));
            }
        }
    }
    best.map(|(_, waypoint, side)| (waypoint, side))
}

/// Which waypoint/tangent handle is under a screen point.
pub(super) fn path_handle_at(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    touch_enabled: bool,
    orbit: &Orbit,
) -> Option<PathHandle> {
    // Play can still reshape the route handles; object editing is what Play
    // leaves behind.
    let editing = editor.actor_tool == Some(ActorTool::Plan) || editor.mode == Mode::Play;
    if !editing || orbit.mode != ViewMode::Top {
        return None;
    }
    let radius = if touch_enabled { 24.0 } else { 10.0 };
    if let Some(id) = waypoint_hit(editor, camera, camera_transform, screen, radius) {
        return Some(PathHandle::Waypoint(id));
    }
    let route = editor.route.as_ref()?;
    tangent_hit(editor, route, camera, camera_transform, screen, radius)
        .map(|(waypoint, side)| PathHandle::Tangent { waypoint, side })
}

/// Apply one pointer update to a handle. The tangent branch is added by the
/// curve task.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_path_drag(
    editor: &mut Editor,
    drag: PathDrag,
    screen: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let Some(ray) = camera.viewport_to_world(camera_transform, screen).ok() else {
        return false;
    };

    match drag.handle {
        PathHandle::Waypoint(id) => {
            let hit = spatial_query
                .cast_ray(ray.origin.real(), ray.direction, 80.0, true, filter)
                .map(|hit| ray.get_point(hit.distance))
                .or_else(|| {
                    ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
                });
            let Some(point) = hit else {
                return false;
            };
            let pos = Vec2::new(point.x, point.z);
            let height = point.y;
            let cell = resolve_cell(&editor.pieces, pos, height);
            set_waypoint(editor, id, pos, cell, height, nav, spatial_query, filter)
        }
        PathHandle::Tangent { waypoint, side } => {
            let Some(point) =
                ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            else {
                return false;
            };
            set_tangent(
                editor,
                waypoint,
                side,
                Vec2::new(point.x, point.z),
                nav,
                spatial_query,
                filter,
            )
        }
    }
}

/// Move one tangent control (clamped against its adjacent anchor) and rebuild.
pub(super) fn set_tangent(
    editor: &mut Editor,
    waypoint_id: u32,
    side: TangentSide,
    control: Vec2,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let character = editor.character;
    let live = old_walk.is_some();

    {
        let Some(plan) = editor.plan.as_mut() else {
            return false;
        };
        let Some(waypoint) = plan.get(waypoint_id).copied() else {
            return false;
        };
        let Some(index) = plan
            .waypoints
            .iter()
            .position(|candidate| candidate.id == waypoint_id)
        else {
            return false;
        };
        let adjacent = match side {
            TangentSide::Outgoing => plan
                .waypoints
                .get(index + 1)
                .map(|next| next.pos)
                .unwrap_or(waypoint.pos),
            TangentSide::Incoming => plan
                .waypoints
                .get(index.wrapping_sub(1))
                .map(|previous| previous.pos)
                .or_else(|| character.map(|character| character.pos))
                .unwrap_or(waypoint.pos),
        };
        let limit = waypoint.pos.distance(adjacent) * 2.0;
        let control = waypoint.pos + (control - waypoint.pos).clamp_length_max(limit);
        match side {
            TangentSide::Incoming => plan.waypoints[index].incoming = Some(control),
            TangentSide::Outgoing => plan.waypoints[index].outgoing = Some(control),
        }
    }

    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
        return false;
    }

    // Store the control exactly as the curve builder clamped it, so the
    // handle and the drawn curve cannot drift apart on the next rebuild.
    let Editor { plan, route, .. } = editor;
    if let (Some(plan), Some(route)) = (plan.as_mut(), route.as_ref()) {
        if let Some(index) = plan
            .waypoints
            .iter()
            .position(|waypoint| waypoint.id == waypoint_id)
        {
            if let Some(controls) = route.controls.get(index) {
                match side {
                    TangentSide::Incoming => {
                        plan.waypoints[index].incoming = controls.incoming;
                    }
                    TangentSide::Outgoing => {
                        plan.waypoints[index].outgoing = controls.outgoing;
                    }
                }
            }
        }
    }
    true
}

/// Move one waypoint and rebuild, restoring the old plan on failure.
#[allow(clippy::too_many_arguments)]
pub(super) fn set_waypoint(
    editor: &mut Editor,
    id: u32,
    pos: Vec2,
    cell: path::Hex,
    height: f32,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();

    {
        let Some(plan) = editor.plan.as_mut() else {
            return false;
        };
        let Some(waypoint) = plan.get_mut(id) else {
            return false;
        };
        waypoint.pos = pos;
        waypoint.cell = cell;
        waypoint.height = height;
    }

    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Delete the selected waypoint. The last waypoint clears the plan entirely.
pub(super) fn delete_waypoint(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;

    let Some(plan) = editor.plan.as_mut() else {
        return false;
    };
    if !plan.delete_selected() {
        return false;
    }
    if plan.is_empty() {
        editor.plan = None;
        editor.route = None;
        editor.walk = None;
        editor.note = "plan cleared";
        return true;
    }

    let live = old_walk.is_some();
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Walk is available only for a stopped, non-empty, reachable plan.
pub(super) fn walk_enabled(plan: Option<&Plan>, route: Option<&Route>, walking: bool) -> bool {
    plan.is_some_and(|plan| !plan.is_empty()) && route.is_some_and(|route| route.valid) && !walking
}

/// A nav rebuild may keep or clear validity but may never start a walk.
pub(super) fn rebuild_walk_state(valid: bool, was_walking: bool) -> (bool, bool) {
    (was_walking, valid)
}

/// Rebuild from the cylinder's current position, then start walking.
pub(super) fn walk_from_plan(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    if !walk_enabled(
        editor.plan.as_ref(),
        editor.route.as_ref(),
        editor.walk.is_some(),
    ) {
        return false;
    }
    if !rebuild_route(editor, nav, spatial_query, filter, false) {
        return false;
    }
    editor.walk = Some(Walk {
        distance: 0.0,
        stuck: 0.0,
    });
    editor.note = "walking";
    true
}

/// Drop every manual tangent. Returns how many handles were cleared.
pub(super) fn clear_tangents(plan: &mut Plan) -> usize {
    let mut cleared = 0;
    for waypoint in &mut plan.waypoints {
        if waypoint.incoming.take().is_some() {
            cleared += 1;
        }
        if waypoint.outgoing.take().is_some() {
            cleared += 1;
        }
    }
    cleared
}

/// Return the plan's curve handles to the automatic tangents and rebuild.
pub(super) fn smooth_plan(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();
    let Some(plan) = editor.plan.as_mut() else {
        return false;
    };
    clear_tangents(plan);
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Rebuild the route geometry from the current plan. `live` keeps the walked
/// line prefix and restarts the suffix at the current track point.
pub(super) fn rebuild_route(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    live: bool,
) -> bool {
    let Some(character) = editor.character else {
        editor.note = "place a character first (C)";
        return false;
    };
    let Some(plan) = editor.plan.as_ref() else {
        return false;
    };
    let waypoints = plan.waypoints.clone();
    let live = live && editor.route.is_some() && editor.walk.is_some();
    let start = rebuild_start(
        live,
        editor.route.as_ref(),
        editor.walk.as_ref(),
        character.pos,
    );
    let height = character.feet;
    let start_cell = resolve_cell(&editor.pieces, start, height);
    let Some(route_plan) = path::plan_route(start, start_cell, &waypoints, &nav.edges, &nav.hexes)
    else {
        editor.note = "no route to that point";
        return false;
    };

    let new_line = route_line(&route_plan.track, spatial_query, filter, height);
    let (trail, line) = if live {
        if let Some(old) = editor.route.as_ref() {
            (
                walked_trail(&old.trail, &old.track, &old.line, start),
                new_line,
            )
        } else {
            (Vec::new(), new_line)
        }
    } else {
        (Vec::new(), new_line)
    };

    editor.route = Some(Route {
        track: route_plan.track,
        trail,
        line,
        distances: route_plan.distances,
        controls: route_plan.controls,
        valid: true,
        hexes: route_plan.hexes,
    });
    if live {
        if let Some(walk) = editor.walk.as_mut() {
            walk.distance = 0.0;
        }
    }
    true
}

/// Add or replace a waypoint and rebuild. On failure the previous plan, route
/// and walk are restored unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) fn add_waypoint(
    editor: &mut Editor,
    target: Vec2,
    cell: path::Hex,
    height: f32,
    append: bool,
    start_walk: bool,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    if editor.character.is_none() {
        editor.note = "place a character first (C)";
        return false;
    }

    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();

    let mut plan = old_plan.clone().unwrap_or_default();
    if append && !plan.is_empty() {
        plan.append(target, cell, height);
    } else {
        plan.replace(target, cell, height);
    }

    editor.plan = Some(plan.clone());
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    editor.plan = choose_plan(old_plan, Some(plan), ok);
    if !ok {
        editor.route = old_route;
        editor.walk = old_walk;
        return false;
    }

    editor.walk = start_walk.then_some(Walk {
        distance: 0.0,
        stuck: 0.0,
    });
    editor.seen_nav = nav.revision;
    editor.note = if start_walk { "walking" } else { "route drawn" };
    true
}
