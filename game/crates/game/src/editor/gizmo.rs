//! gizmo systems for the level editor.

use super::*;

pub(super) fn transform_gizmo(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    for (dir, colour) in [(Vec3::X, GIZMO_X), (Vec3::Y, GIZMO_Y), (Vec3::Z, GIZMO_Z)] {
        let end = origin + dir * reach;
        gizmos.line(origin, end, colour);
        // Arrow heads as diamonds so they still read edge-on, scaled to the
        // handle. A vertical arrow uses X as its visible width.
        let back = end - dir * (reach * GIZMO_ARROW_BACK);
        let side = if dir == Vec3::Y {
            Vec3::X * (reach * GIZMO_ARROW_SIDE)
        } else {
            Vec3::new(dir.z, 0.0, dir.x) * (reach * GIZMO_ARROW_SIDE)
        };
        closed(gizmos, &[back + side, end, back - side], colour);
    }

    for (a, b, colour) in [
        (Vec3::X, Vec3::Y, GIZMO_Y),
        (Vec3::Z, Vec3::Y, GIZMO_Y),
        (Vec3::X, Vec3::Z, AMBER),
    ] {
        closed(gizmos, &gizmo_plane_corners(origin, a, b, reach), colour);
    }

    for (u, v, colour) in [
        (Vec3::Y, Vec3::Z, GIZMO_X),
        (Vec3::X, Vec3::Z, GIZMO_Y),
        (Vec3::X, Vec3::Y, GIZMO_Z),
    ] {
        let points: Vec<Vec3> = (0..32)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 32.0;
                origin + (u * a.cos() + v * a.sin()) * ring
            })
            .collect();
        closed(gizmos, &points, colour);
    }
}

/// Where the gizmo's axes meet: just above the piece's base, or at ground level
/// for sunken pieces such as a pit.
pub(super) fn gizmo_origin(piece: &Piece, shape: &model::Shape) -> Vec3 {
    let base = shape.base.max(0.0) + 0.04;
    Vec3::new(piece.pos.x, piece.elevation + base, piece.pos.y)
}

/// The centre of one two-axis plane handle.
pub(super) fn gizmo_plane_center(origin: Vec3, a: Vec3, b: Vec3, reach: f32) -> Vec3 {
    origin + (a + b) * (reach * GIZMO_PLANE_CENTER)
}

/// The four corners of a plane handle, in loop order.
pub(super) fn gizmo_plane_corners(origin: Vec3, a: Vec3, b: Vec3, reach: f32) -> [Vec3; 4] {
    let centre = gizmo_plane_center(origin, a, b, reach);
    let half = reach * GIZMO_PLANE_SIZE;
    [
        centre - a * half - b * half,
        centre + a * half - b * half,
        centre + a * half + b * half,
        centre - a * half + b * half,
    ]
}

/// World points that identify each draggable handle. Axis handles use their
/// tip; plane handles use their centre.
pub(super) fn gizmo_handle_points(piece: &Piece, shape: &model::Shape) -> Vec<(GizmoHandle, Vec3)> {
    let origin = gizmo_origin(piece, shape);
    let (reach, _) = gizmo_frame(piece, shape);
    [
        (GizmoHandle::X, Vec3::X),
        (GizmoHandle::Y, Vec3::Y),
        (GizmoHandle::Z, Vec3::Z),
    ]
    .into_iter()
    .map(|(handle, dir)| (handle, origin + dir * reach))
    .chain(
        [
            (GizmoHandle::Xy, Vec3::X, Vec3::Y),
            (GizmoHandle::Zy, Vec3::Z, Vec3::Y),
            (GizmoHandle::Xz, Vec3::X, Vec3::Z),
        ]
        .into_iter()
        .map(|(handle, a, b)| (handle, gizmo_plane_center(origin, a, b, reach))),
    )
    .collect()
}

/// Gizmo geometry for a piece: how far an X or Z handle reaches from the piece's
/// origin and how wide the yaw ring is. Both are sized against the piece's
/// bounding box, so the gizmo stays bigger than the selection it belongs to.
pub(super) fn gizmo_frame(piece: &Piece, shape: &model::Shape) -> (f32, f32) {
    let size = bounding_size(piece, shape);
    (size * GIZMO_GROWTH, size * GIZMO_RING_SCALE)
}

/// The piece's bounding box as the largest of its local ground-plane extent and
/// its height, in metres. Tilt does not change the gizmo size.
pub(super) fn bounding_size(_piece: &Piece, shape: &model::Shape) -> f32 {
    let points = model::shape_points(shape);
    let (low, high) = outline_bounds(&points);
    let ground = Vec2::new(high.x - low.x, high.y - low.y);
    ground.x.max(ground.y).max(shape.top() - shape.base)
}

/// World-space bounding box of a piece: the low and high corners of its fully
/// oriented box, projected onto `(x, z)`.
pub(super) fn piece_bounds(piece: &Piece, shape: &model::Shape) -> (Vec2, Vec2) {
    let points = model::shape_points(shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rotation = model::piece_quat(piece);
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for point in &points {
        for y in [shape.base, shape.top()] {
            let world = root + rotation * Vec3::new(point.x, y, point.y);
            low.x = low.x.min(world.x);
            low.y = low.y.min(world.z);
            high.x = high.x.max(world.x);
            high.y = high.y.max(world.z);
        }
    }
    (low, high)
}

/// Closest point on the line `origin + s * axis` to the ray, as `s`.
pub(super) fn ray_axis_param(ray: Ray3d, origin: Vec3, axis: Vec3) -> Option<f32> {
    let dir = *ray.direction;
    let w0 = origin - ray.origin;
    let b = axis.dot(dir);
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-5 {
        return None;
    }
    Some((b * dir.dot(w0) - axis.dot(w0)) / denom)
}

/// Distance from a point to a screen-space segment.
pub(super) fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len = ab.length_squared();
    if len < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Select/shape-place modes let a gizmo handle take the pointer. Character,
/// path and plan modes own the pointer for their own actions; touch taps are
/// handled by the path gesture layer.
pub(super) fn gizmo_takes_pointer(editor: &Editor, touch_tap: bool) -> bool {
    object_editing_allowed(editor.mode) && editor.actor_tool.is_none() && !touch_tap
}

/// Which gizmo handle is under the pointer, if any.
pub(super) fn gizmo_hit(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    ray: Ray3d,
) -> Option<GizmoHandle> {
    let id = editor.selected?;
    let piece = editor.pieces.iter().find(|p| p.id == id)?;
    let shape = shapes().get(piece.shape)?;
    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    let project = |point: Vec3| camera.world_to_viewport(camera_transform, point).ok();

    // Plane handles first: their squares are small and unambiguous, and a
    // click inside one must not be stolen by an axis line crossing behind it.
    for (handle, a, b) in [
        (GizmoHandle::Xy, Vec3::X, Vec3::Y),
        (GizmoHandle::Zy, Vec3::Z, Vec3::Y),
        (GizmoHandle::Xz, Vec3::X, Vec3::Z),
    ] {
        let corners = gizmo_plane_corners(origin, a, b, reach);
        let Some(quad) = corners
            .iter()
            .map(|point| project(*point))
            .collect::<Option<Vec<Vec2>>>()
            .and_then(|points| <[Vec2; 4]>::try_from(points).ok())
        else {
            continue;
        };
        if plane_handle_hit(screen, &quad) {
            return Some(handle);
        }
    }

    let mut best: Option<(f32, GizmoHandle)> = None;
    if let Ok(origin_screen) = camera.world_to_viewport(camera_transform, origin) {
        for (handle, point) in gizmo_handle_points(piece, shape) {
            if !matches!(handle, GizmoHandle::X | GizmoHandle::Y | GizmoHandle::Z) {
                continue;
            }
            if let Some(end_screen) = project(point) {
                let distance = point_segment_distance(screen, origin_screen, end_screen);
                if distance < GIZMO_HANDLE_HIT && best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, handle));
                }
            }
        }
    }

    if best.is_none() {
        let mut ring_best: Option<(f32, GizmoHandle)> = None;
        for (handle, axis) in [
            (GizmoHandle::Pitch, Vec3::X),
            (GizmoHandle::Yaw, Vec3::Y),
            (GizmoHandle::Roll, Vec3::Z),
        ] {
            if let Some(hit) = ray.plane_intersection_point(origin, InfinitePlane3d::new(axis)) {
                let radius = (hit - origin).length();
                let error = (radius - ring).abs();
                if error < ring * GIZMO_YAW_HIT && ring_best.is_none_or(|(d, _)| error < d) {
                    ring_best = Some((error, handle));
                }
            }
        }
        best = ring_best;
    }

    best.map(|(_, handle)| handle)
}

/// The point on the drag line or plane that the pointer ray meets. Quantities
/// are measured from the grab position, so the piece follows the pointer's
/// travel rather than jumping to where it points.
pub(super) fn gizmo_drag_point(ray: Ray3d, handle: GizmoHandle, origin: Vec3) -> Option<Vec3> {
    match handle {
        GizmoHandle::X => ray_axis_point(ray, origin, Vec3::X),
        GizmoHandle::Z => ray_axis_point(ray, origin, Vec3::Z),
        GizmoHandle::Y => vertical_drag_point(ray, origin),
        GizmoHandle::Xy => plane_ray_point(ray, origin, Vec3::Z),
        GizmoHandle::Zy => plane_ray_point(ray, origin, Vec3::X),
        GizmoHandle::Xz | GizmoHandle::Yaw => plane_ray_point(ray, origin, Vec3::Y),
        GizmoHandle::Pitch => plane_ray_point(ray, origin, Vec3::X),
        GizmoHandle::Roll => plane_ray_point(ray, origin, Vec3::Z),
    }
}

pub(super) fn plane_ray_point(ray: Ray3d, origin: Vec3, normal: Vec3) -> Option<Vec3> {
    ray.plane_intersection_point(origin, InfinitePlane3d::new(normal))
}

pub(super) fn ray_axis_point(ray: Ray3d, origin: Vec3, axis: Vec3) -> Option<Vec3> {
    ray_axis_param(ray, origin, axis).map(|s| origin + axis * s)
}

/// Vertical drags use a camera-facing plane through the axis, which stays
/// stable in a three-quarter view. Straight overhead there is no such plane,
/// so fall back to the closest point on the axis.
pub(super) fn vertical_drag_point(ray: Ray3d, origin: Vec3) -> Option<Vec3> {
    let flat = Vec3::new(ray.origin.x - origin.x, 0.0, ray.origin.z - origin.z);
    if flat.length_squared() > 1e-4 {
        let normal = flat.normalize();
        if let Some(hit) = plane_ray_point(ray, origin, normal) {
            return Some(Vec3::new(origin.x, hit.y, origin.z));
        }
    }
    ray_axis_point(ray, origin, Vec3::Y)
}

/// The piece state after dragging from `grab` to `current`. Only the axes the
/// handle owns change; every other component is kept from the grab snapshot.
pub(super) fn dragged_piece(start: Piece, handle: GizmoHandle, grab: Vec3, current: Vec3) -> Piece {
    let delta = current - grab;
    let mut piece = start;
    match handle {
        GizmoHandle::X => piece.pos.x += delta.x,
        GizmoHandle::Z => piece.pos.y += delta.z,
        GizmoHandle::Y => piece.elevation += delta.y,
        GizmoHandle::Xy => {
            piece.pos.x += delta.x;
            piece.elevation += delta.y;
        }
        GizmoHandle::Zy => {
            piece.pos.y += delta.z;
            piece.elevation += delta.y;
        }
        GizmoHandle::Xz => {
            piece.pos.x += delta.x;
            piece.pos.y += delta.z;
        }
        GizmoHandle::Pitch | GizmoHandle::Yaw | GizmoHandle::Roll => {
            let axis = match handle {
                GizmoHandle::Pitch => Vec3::X,
                GizmoHandle::Yaw => Vec3::Y,
                GizmoHandle::Roll => Vec3::Z,
                _ => unreachable!(),
            };
            let shape = &shapes()[start.shape];
            let origin = gizmo_origin(&start, shape);
            if let Some(angle) = signed_angle_around(axis, grab - origin, current - origin) {
                let rotation = Quat::from_axis_angle(axis, angle) * model::piece_quat(&start);
                model::set_piece_quat(&mut piece, rotation);
            }
        }
    }
    piece
}

/// Signed angle from `from` to `to` around `axis`, both measured from the
/// rotation origin. Vectors are projected onto the plane normal to the axis.
pub(super) fn signed_angle_around(axis: Vec3, from: Vec3, to: Vec3) -> Option<f32> {
    let from = from - axis * from.dot(axis);
    let to = to - axis * to.dot(axis);
    (from.length_squared() > 0.01 && to.length_squared() > 0.01)
        .then(|| from.cross(to).dot(axis).atan2(from.dot(to)))
}

/// Snap a dragged elevation onto the top of a piece below it, but only when
/// the drag is moving toward that surface and it is within reach. Away from a
/// surface the raw height wins, so the piece follows the pointer instead of
/// stepping over a grid. The moving piece itself is ignored.
pub(super) fn snap_elevation(
    piece: Piece,
    shape: &model::Shape,
    others: &[Piece],
    raw: f32,
    direction: f32,
    snapping: bool,
    threshold: f32,
) -> f32 {
    if !snapping || direction == 0.0 {
        return raw;
    }

    let moving_up = direction > 0.0;
    let mut best: Option<(f32, f32)> = None;
    let mut consider = |elevation: f32| {
        // A surface behind the pointer must not capture the piece: that would
        // make it snap backwards while the user drags away from it.
        if (moving_up && elevation < raw) || (!moving_up && elevation > raw) {
            return;
        }
        let distance = (elevation - raw).abs();
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, elevation));
        }
    };

    // The ground is the natural zero, not `-base`; every piece starts there.
    consider(0.0);
    for other in others {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        let (low, high) = piece_bounds(other, other_shape);
        if piece.pos.x < low.x - HIT_PAD
            || piece.pos.x > high.x + HIT_PAD
            || piece.pos.y < low.y - HIT_PAD
            || piece.pos.y > high.y + HIT_PAD
        {
            continue;
        }
        consider(other.elevation + other_shape.top() - shape.base);
    }

    if let Some((distance, elevation)) = best
        && distance <= threshold
    {
        return elevation;
    }
    raw
}

pub(super) fn apply_gizmo_drag(editor: &mut Editor, drag: GizmoDrag, ray: Option<Ray3d>) {
    let Some(ray) = ray else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|piece| piece.id == drag.id) else {
        return;
    };
    let shape = &shapes()[drag.start.shape];
    let origin = gizmo_origin(&drag.start, shape);
    let Some(current) = gizmo_drag_point(ray, drag.handle, origin) else {
        return;
    };
    let mut piece = dragged_piece(drag.start, drag.handle, drag.grab, current);
    let raw = piece.elevation;

    if matches!(
        drag.handle,
        GizmoHandle::Y | GizmoHandle::Xy | GizmoHandle::Zy
    ) {
        if (raw - drag.last_raw).abs() > 1e-6 {
            piece.elevation = snap_elevation(
                piece,
                shape,
                &editor.pieces,
                raw,
                raw - drag.last_raw,
                editor.snap,
                ELEVATION_SNAP,
            );
        } else {
            // Clicking the handle without moving must not shift the piece,
            // even if it happens to sit near a surface.
            piece.elevation = drag.last;
        }
    }
    editor.pieces[index] = piece;
    editor.gizmo_drag = Some(GizmoDrag {
        last_raw: raw,
        last: piece.elevation,
        ..drag
    });
}

/// A plane handle is only grabbable while it has a visible face. Seen edge-on,
/// its projection collapses to a sliver that must not steal an axis click.
pub(super) fn plane_handle_hit(p: Vec2, quad: &[Vec2; 4]) -> bool {
    quad_area(quad) >= 24.0 && point_in_quad(p, quad)
}

pub(super) fn quad_area(quad: &[Vec2; 4]) -> f32 {
    let mut twice = 0.0;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        twice += a.x * b.y - b.x * a.y;
    }
    twice.abs() * 0.5
}

/// Point-in-quad for the projected plane handles. The quad must be convex and
/// in loop order.
pub(super) fn point_in_quad(p: Vec2, quad: &[Vec2; 4]) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[j];
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(super) fn closed(gizmos: &mut Gizmos, points: &[Vec3], colour: Color) {
    if points.len() < 2 {
        return;
    }
    gizmos.linestrip(
        points.iter().copied().chain(std::iter::once(points[0])),
        colour,
    );
}

pub(super) fn selection_box(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let points = model::shape_points(shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rotation = model::piece_quat(piece);
    let at = |p: Vec2, y: f32| root + rotation * Vec3::new(p.x, y, p.y);
    let low = shape.base - 0.01;
    let high = shape.top() + 0.01;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        gizmos.line(at(a, low), at(b, low), AMBER);
        gizmos.line(at(a, high), at(b, high), AMBER);
        gizmos.line(at(a, low), at(a, high), AMBER);
    }
}

pub(super) fn anchors(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let size = 0.07;
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rotation = model::piece_quat(piece);
    for anchor in &shape.anchors {
        let c = root + rotation * Vec3::new(anchor.local.x, shape.top() + 0.03, anchor.local.y);
        let corners = [
            c + rotation * Vec3::new(0.0, 0.0, size),
            c + rotation * Vec3::new(size, 0.0, 0.0),
            c + rotation * Vec3::new(0.0, 0.0, -size),
            c + rotation * Vec3::new(-size, 0.0, 0.0),
        ];
        closed(gizmos, &corners, anchor.kind.color());
        if anchor.kind == AnchorKind::End {
            let normal = rotation * Vec3::new(anchor.normal.x, 0.0, anchor.normal.y);
            gizmos.line(c, c + normal * 0.15, anchor.kind.color());
        }
    }
}
