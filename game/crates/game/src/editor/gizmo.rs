//! gizmo systems for the level editor.

use super::*;

/// Gizmo group for overlays that should render X-ray (on top of buildings and
/// terrain rather than being occluded by 3D meshes).
#[derive(Default, Reflect, GizmoConfigGroup)]
#[reflect(Default)]
pub(super) struct XrayGizmos;

/// Register the X-ray gizmo group before `draw` runs.
pub(super) fn configure_xray_gizmos(app: &mut App) {
    app.insert_gizmo_config(
        XrayGizmos,
        GizmoConfig {
            depth_bias: -1.0,
            ..default()
        },
    );
}

/// Draw a dashed line segment in 3D space.
fn dashed_line<Config: GizmoConfigGroup>(
    gizmos: &mut Gizmos<Config>,
    a: Vec3,
    b: Vec3,
    colour: Color,
) {
    let dist = a.distance(b);
    if dist < 1e-4 {
        return;
    }
    let step = 0.22;
    let dash = 0.11;
    let dir = (b - a) / dist;
    let mut t = 0.0;
    while t < dist {
        let end = (t + dash).min(dist);
        gizmos.line(a + dir * t, a + dir * end, colour);
        t += step;
    }
}

/// Draw a filled-outline arrowhead at `tip` pointing along unit `dir`.
fn glade_arrowhead(
    gizmos: &mut Gizmos,
    tip: Vec3,
    dir: Vec3,
    head_len: f32,
    head_w: f32,
    colour: Color,
) {
    let side = if dir.y.abs() > 0.85 {
        Vec3::X
    } else {
        Vec3::new(-dir.z, 0.0, dir.x).normalize_or_zero()
    };
    let neck = tip - dir * head_len;
    closed(
        gizmos,
        &[neck + side * head_w, tip, neck - side * head_w],
        colour,
    );
}

/// Draw a clean white 3D arrow from `base` to `tip` with a filled-outline head.
fn glade_arrow(gizmos: &mut Gizmos, base: Vec3, tip: Vec3, head_len: f32, head_w: f32, colour: Color) {
    let dir = (tip - base).normalize_or_zero();
    if dir == Vec3::ZERO {
        return;
    }
    let side = if dir.y.abs() > 0.85 {
        Vec3::X
    } else {
        Vec3::new(-dir.z, 0.0, dir.x).normalize_or_zero()
    };
    let neck = tip - dir * head_len;
    gizmos.line(base, neck, colour);
    gizmos.line(base + side * 0.02, neck + side * 0.02, colour);
    glade_arrowhead(gizmos, tip, dir, head_len, head_w, colour);
}

/// Tiny Glade–style white building controls: dashed bounding/roof wireframe,
/// 4 outward face resize arrows, vertical height bar with end nodes, upward
/// roof-pitch apex arrow, 4 ground-plane move arrows, and 2 corner rotation arcs.
fn glade_transform_gizmo(
    gizmos: &mut Gizmos,
    xray_gizmos: &mut Gizmos<XrayGizmos>,
    piece: &Piece,
    shape: &model::Shape,
    touch: bool,
) {
    let kind = crate::glade::GladeKind::from_shape(shape);
    if kind == Some(crate::glade::GladeKind::Path) {
        return;
    }
    let white = Color::srgba(1.0, 1.0, 1.0, 0.95);
    let wire = Color::srgba(1.0, 1.0, 1.0, 0.65);
    let ext = crate::glade::piece_extents(piece, shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rot = model::piece_quat(piece);
    let hx = ext.x * 0.5;
    let hz = ext.z * 0.5;
    let pt = |x: f32, y: f32, z: f32| root + rot * Vec3::new(x, y, z);

    // 1. Dashed wireframe bounding box + roof ridge
    if !matches!(kind, Some(crate::glade::GladeKind::Terrain)) {
        let b = [
            pt(-hx, 0.0, -hz),
            pt(hx, 0.0, -hz),
            pt(hx, 0.0, hz),
            pt(-hx, 0.0, hz),
        ];
        let t = [
            pt(-hx, ext.y, -hz),
            pt(hx, ext.y, -hz),
            pt(hx, ext.y, hz),
            pt(-hx, ext.y, hz),
        ];
        for i in 0..4 {
            let j = (i + 1) % 4;
            dashed_line(xray_gizmos, b[i], b[j], wire);
            dashed_line(xray_gizmos, t[i], t[j], wire);
            dashed_line(xray_gizmos, b[i], t[i], wire);
        }
        let rise = crate::glade::roof_rise(piece, shape);
        if rise > 0.05 {
            let top_h = ext.y + rise;
            if kind == Some(crate::glade::GladeKind::Tower) {
                let apex = pt(0.0, top_h, 0.0);
                for corner in t {
                    dashed_line(xray_gizmos, corner, apex, wire);
                }
            } else if kind == Some(crate::glade::GladeKind::Room) {
                if ext.x >= ext.z {
                    let r0 = pt(-hx, top_h, 0.0);
                    let r1 = pt(hx, top_h, 0.0);
                    dashed_line(xray_gizmos, r0, r1, wire);
                    dashed_line(xray_gizmos, t[0], r0, wire);
                    dashed_line(xray_gizmos, t[3], r0, wire);
                    dashed_line(xray_gizmos, t[1], r1, wire);
                    dashed_line(xray_gizmos, t[2], r1, wire);
                } else {
                    let r0 = pt(0.0, top_h, -hz);
                    let r1 = pt(0.0, top_h, hz);
                    dashed_line(xray_gizmos, r0, r1, wire);
                    dashed_line(xray_gizmos, t[0], r0, wire);
                    dashed_line(xray_gizmos, t[1], r0, wire);
                    dashed_line(xray_gizmos, t[3], r1, wire);
                    dashed_line(xray_gizmos, t[2], r1, wire);
                }
            }
        }
    }

    // 2. Tactile handles (4 outward face arrows, vertical height bar, roof apex arrow)
    let head_len = if touch { 0.28 } else { 0.18 };
    let head_w = if touch { 0.16 } else { 0.10 };
    let node_r = if touch { 0.14 } else { 0.09 };
    for (handle, tip, stem_base) in glade_handle_visuals_for(piece, shape, touch) {
        match handle {
            GizmoHandle::GladeWidth | GizmoHandle::GladeLength | GizmoHandle::GladeRoofPitch => {
                glade_arrow(gizmos, stem_base, tip, head_len, head_w, white);
            }
            GizmoHandle::GladeHeight => {
                let bottom = Vec3::new(stem_base.x, root.y, stem_base.z);
                gizmos.line(bottom, tip, white);
                for end in [bottom, tip] {
                    gizmos.sphere(Isometry3d::new(end, Quat::IDENTITY), node_r, white);
                }
            }
            _ => {}
        }
    }

    // 3. Ground-plane 4 arrow move controls + 2 corner rotation arcs
    let move_head_len = if touch { 0.22 } else { 0.15 };
    let move_head_w = if touch { 0.13 } else { 0.09 };
    for (base, tip) in glade_move_arrows(piece, shape, touch) {
        glade_arrow(gizmos, base, tip, move_head_len, move_head_w, white);
    }
    let arc_head_len = if touch { 0.18 } else { 0.12 };
    let arc_head_w = if touch { 0.11 } else { 0.075 };
    for arc in glade_rotation_arcs(piece, shape, touch) {
        for pair in arc.windows(2) {
            gizmos.line(pair[0], pair[1], white);
        }
        if arc.len() >= 2 {
            let start_dir = (arc[0] - arc[1]).normalize_or_zero();
            if start_dir != Vec3::ZERO {
                glade_arrowhead(gizmos, arc[0], start_dir, arc_head_len, arc_head_w, white);
            }
            let last = arc.len() - 1;
            let end_dir = (arc[last] - arc[last - 1]).normalize_or_zero();
            if end_dir != Vec3::ZERO {
                glade_arrowhead(gizmos, arc[last], end_dir, arc_head_len, arc_head_w, white);
            }
        }
    }
}

pub(super) fn transform_gizmo(
    gizmos: &mut Gizmos,
    xray_gizmos: &mut Gizmos<XrayGizmos>,
    piece: &Piece,
    shape: &model::Shape,
    touch: bool,
) {
    if shape.is_glade() {
        glade_transform_gizmo(gizmos, xray_gizmos, piece, shape, touch);
        return;
    }

    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    // Each axis runs both ways from the origin so a piece can be shoved
    // from either side; both ends get an arrowhead pointing outward.
    // Arrow heads are diamonds so they still read edge-on, scaled to the
    // handle. A vertical arrow uses X for its visible width.
    for (dir, colour) in [(Vec3::X, GIZMO_X), (Vec3::Y, GIZMO_Y), (Vec3::Z, GIZMO_Z)] {
        gizmos.line(origin - dir * reach, origin + dir * reach, colour);
        for (end, outward) in [(origin + dir * reach, dir), (origin - dir * reach, -dir)] {
            let back = end - outward * (reach * GIZMO_ARROW_BACK);
            let side = if dir == Vec3::Y {
                Vec3::X * (reach * GIZMO_ARROW_SIDE)
            } else {
                Vec3::new(dir.z, 0.0, dir.x) * (reach * GIZMO_ARROW_SIDE)
            };
            closed(gizmos, &[back + side, end, back - side], colour);
        }
    }

    for (a, b, colour) in [
        (Vec3::X, Vec3::Y, GIZMO_Y),
        (Vec3::Z, Vec3::Y, GIZMO_Y),
        (Vec3::X, Vec3::Z, AMBER),
    ] {
        closed(gizmos, &gizmo_plane_corners(origin, a, b, reach), colour);
    }

    let rotation_rings: &[(Vec3, Vec3, Color)] = &[
        (Vec3::Y, Vec3::Z, GIZMO_X),
        (Vec3::X, Vec3::Z, GIZMO_Y),
        (Vec3::X, Vec3::Y, GIZMO_Z),
    ];
    for &(u, v, colour) in rotation_rings {
        let points: Vec<Vec3> = (0..32)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 32.0;
                origin + (u * a.cos() + v * a.sin()) * ring
            })
            .collect();
        closed(gizmos, &points, colour);
    }
}

/// Tactile 3D handles for Glade buildings: `(handle, handle_pos, stem_anchor_pos)`.
pub(super) fn glade_handle_visuals(
    piece: &Piece,
    shape: &model::Shape,
) -> Vec<(GizmoHandle, Vec3, Vec3)> {
    glade_handle_visuals_for(piece, shape, false)
}

/// Tactile 3D handles for Glade buildings, with enlarged spacing on touch screens.
pub(super) fn glade_handle_visuals_for(
    piece: &Piece,
    shape: &model::Shape,
    touch: bool,
) -> Vec<(GizmoHandle, Vec3, Vec3)> {
    if !shape.is_glade() {
        return Vec::new();
    }
    let kind = crate::glade::GladeKind::from_shape(shape);
    if kind == Some(crate::glade::GladeKind::Path) {
        return Vec::new();
    }
    let ext = crate::glade::piece_extents(piece, shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rot = model::piece_quat(piece);
    let mid_y = (ext.y * 0.5).max(0.35);
    let hx = ext.x * 0.5;
    let hz = ext.z * 0.5;
    let pad = if touch { 0.56 } else { 0.34 };
    let v_pad = if touch { 0.52 } else { 0.32 };

    let mut out = vec![
        (
            GizmoHandle::GladeWidth,
            root + rot * Vec3::new(hx + pad, mid_y, 0.0),
            root + rot * Vec3::new(hx, mid_y, 0.0),
        ),
        (
            GizmoHandle::GladeWidth,
            root + rot * Vec3::new(-hx - pad, mid_y, 0.0),
            root + rot * Vec3::new(-hx, mid_y, 0.0),
        ),
    ];

    if !matches!(
        kind,
        Some(crate::glade::GladeKind::Door | crate::glade::GladeKind::Window)
    ) {
        out.push((
            GizmoHandle::GladeLength,
            root + rot * Vec3::new(0.0, mid_y, hz + pad),
            root + rot * Vec3::new(0.0, mid_y, hz),
        ));
        out.push((
            GizmoHandle::GladeLength,
            root + rot * Vec3::new(0.0, mid_y, -hz - pad),
            root + rot * Vec3::new(0.0, mid_y, -hz),
        ));
    }

    let height_anchor_xz = if matches!(
        kind,
        Some(
            crate::glade::GladeKind::Wall
                | crate::glade::GladeKind::Door
                | crate::glade::GladeKind::Window
        )
    ) {
        Vec2::ZERO
    } else {
        Vec2::new(hx + pad * 0.55, hz + pad * 0.55)
    };
    out.push((
        GizmoHandle::GladeHeight,
        root + rot * Vec3::new(height_anchor_xz.x, ext.y, height_anchor_xz.y),
        root + rot * Vec3::new(height_anchor_xz.x, 0.0, height_anchor_xz.y),
    ));

    if matches!(
        kind,
        Some(
            crate::glade::GladeKind::Room
                | crate::glade::GladeKind::Tower
                | crate::glade::GladeKind::Terrain
        )
    ) {
        let top_h = crate::glade::total_height(piece, shape);
        out.push((
            GizmoHandle::GladeRoofPitch,
            root + rot * Vec3::new(0.0, top_h + v_pad * 1.15, 0.0),
            root + rot * Vec3::new(0.0, top_h, 0.0),
        ));
    }
    out
}

/// 4 ground-plane move arrows (`(base, tip)` in world space) outside the piece's
/// base footprint, one per cardinal face, for dragging on the XZ plane.
pub(super) fn glade_move_arrows(
    piece: &Piece,
    shape: &model::Shape,
    touch: bool,
) -> Vec<(Vec3, Vec3)> {
    if !shape.is_glade()
        || crate::glade::GladeKind::from_shape(shape) == Some(crate::glade::GladeKind::Path)
    {
        return Vec::new();
    }
    let ext = crate::glade::piece_extents(piece, shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rot = model::piece_quat(piece);
    let hx = ext.x * 0.5;
    let hz = ext.z * 0.5;
    let y = 0.05;
    let inner = if touch { 0.24 } else { 0.16 };
    let outer = if touch { 0.88 } else { 0.62 };
    let pt = |x: f32, z: f32| root + rot * Vec3::new(x, y, z);

    vec![
        (pt(hx + inner, 0.0), pt(hx + outer, 0.0)),
        (pt(-hx - inner, 0.0), pt(-hx - outer, 0.0)),
        (pt(0.0, hz + inner), pt(0.0, hz + outer)),
        (pt(0.0, -hz - inner), pt(0.0, -hz - outer)),
    ]
}

/// 2 corner rotation arcs on the ground plane at opposite corners outside the
/// piece's base footprint (avoiding the `(+hx, +hz)` height-bar corner).
pub(super) fn glade_rotation_arcs(
    piece: &Piece,
    shape: &model::Shape,
    touch: bool,
) -> Vec<Vec<Vec3>> {
    if !shape.is_glade()
        || crate::glade::GladeKind::from_shape(shape) == Some(crate::glade::GladeKind::Path)
    {
        return Vec::new();
    }
    let ext = crate::glade::piece_extents(piece, shape);
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rot = model::piece_quat(piece);
    let hx = ext.x * 0.5;
    let hz = ext.z * 0.5;
    let y = 0.05;
    let arc_pad = if touch { 0.48 } else { 0.32 };
    let radius = (hx * hx + hz * hz).sqrt() + arc_pad;
    let half_span = 0.36_f32;
    const ARC_STEPS: usize = 10;

    [(-hz).atan2(hx), hz.atan2(-hx)]
        .into_iter()
        .map(|center_angle| {
            (0..=ARC_STEPS)
                .map(|i| {
                    let t = i as f32 / ARC_STEPS as f32;
                    let a = center_angle + (t * 2.0 - 1.0) * half_span;
                    root + rot * Vec3::new(a.cos() * radius, y, a.sin() * radius)
                })
                .collect()
        })
        .collect()
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

/// World points that identify each draggable handle. Axis handles list their
/// positive tip; the hit test mirrors it through the origin so the negative
/// end grabs the same handle. Plane handles use their centre.
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
pub(super) fn bounding_size(piece: &Piece, shape: &model::Shape) -> f32 {
    if shape.is_glade() {
        let ext = crate::glade::piece_extents(piece, shape);
        let top = crate::glade::total_height(piece, shape);
        return ext.x.max(ext.z).max(top);
    }
    let points = model::shape_points(shape);
    let (low, high) = outline_bounds(&points);
    let ground = Vec2::new(high.x - low.x, high.y - low.y);
    ground.x.max(ground.y).max(shape.top() - shape.base)
}

/// World-space bounding box of a piece: the low and high corners of its fully
/// oriented box, projected onto `(x, z)`.
pub(super) fn piece_bounds(piece: &Piece, shape: &model::Shape) -> (Vec2, Vec2) {
    let points = if shape.is_glade() {
        crate::glade::local_footprint(piece, shape)
    } else {
        model::shape_points(shape)
    };
    let top = if shape.is_glade() {
        crate::glade::total_height(piece, shape)
    } else {
        shape.top()
    };
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rotation = model::piece_quat(piece);
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for point in &points {
        for y in [shape.base, top] {
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

/// Whether a press may start a gizmo drag. This is the shared gate for the
/// desktop mouse path (`interact`) and the touch path (`on_path_press`): a
/// selected piece, no armed tool, and no other gesture in flight.
pub(super) fn gizmo_grab_allowed(editor: &Editor, gesture_dragging: bool) -> bool {
    object_editing_allowed(editor.mode)
        && editor.actor_tool.is_none()
        && !editor.selection.is_empty()
        && !gesture_dragging
        && editor.gizmo_drag.is_none()
}

/// Which gizmo handle is under the pointer, if any. Touch gets a fingertip
/// tolerance (matching the path-handle hit test); the mouse keeps the exact
/// one so dense handles stay distinguishable.
pub(super) fn gizmo_hit(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    ray: Ray3d,
    touch: bool,
) -> Option<GizmoHandle> {
    let id = editor.selection.primary()?;
    let piece = editor.pieces.iter().find(|p| p.id == id)?;
    let shape = shapes().get(piece.shape)?;
    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    let project = |point: Vec3| camera.world_to_viewport(camera_transform, point).ok();

    if shape.is_glade() {
        let orb_radius = if touch { 64.0 } else { 28.0 };
        let mut best: Option<(f32, GizmoHandle)> = None;
        for (handle, tip, stem_base) in glade_handle_visuals_for(piece, shape, touch) {
            if let Some(tip_screen) = project(tip) {
                let d_tip = screen.distance(tip_screen);
                let d_stem = project(stem_base)
                    .map(|stem_screen| point_segment_distance(screen, stem_screen, tip_screen))
                    .unwrap_or(d_tip);
                let d = d_tip.min(d_stem);
                if d <= orb_radius && best.is_none_or(|(best_d, _)| d < best_d) {
                    best = Some((d, handle));
                }
            }
        }

        let ground_hit = if touch { 40.0 } else { 22.0 };
        for (base, tip) in glade_move_arrows(piece, shape, touch) {
            if let (Some(base_screen), Some(tip_screen)) = (project(base), project(tip)) {
                let d = point_segment_distance(screen, base_screen, tip_screen);
                if d <= ground_hit && best.is_none_or(|(best_d, _)| d < best_d) {
                    best = Some((d, GizmoHandle::Xz));
                }
            }
        }
        for arc in glade_rotation_arcs(piece, shape, touch) {
            for pair in arc.windows(2) {
                if let (Some(a_screen), Some(b_screen)) = (project(pair[0]), project(pair[1])) {
                    let d = point_segment_distance(screen, a_screen, b_screen);
                    if d <= ground_hit && best.is_none_or(|(best_d, _)| d < best_d) {
                        best = Some((d, GizmoHandle::Yaw));
                    }
                }
            }
        }
        return best.map(|(_, handle)| handle);
    }

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
    // Fingertips need a bigger target than a mouse cursor, especially on Glade buildings.
    let hit_radius = if touch {
        if shape.is_glade() { 38.0 } else { 24.0 }
    } else {
        GIZMO_HANDLE_HIT
    };
    if let Ok(origin_screen) = camera.world_to_viewport(camera_transform, origin) {
        for (handle, point) in gizmo_handle_points(piece, shape) {
            if !matches!(handle, GizmoHandle::X | GizmoHandle::Y | GizmoHandle::Z) {
                continue;
            }
            // Both ends of the axis grab the same handle; the negative tip
            // mirrors the positive one through the origin.
            for tip in [point, 2.0 * origin - point] {
                if let Some(end_screen) = project(tip) {
                    let distance = point_segment_distance(screen, origin_screen, end_screen);
                    if distance < hit_radius && best.is_none_or(|(d, _)| distance < d) {
                        best = Some((distance, handle));
                    }
                }
            }
        }
    }

    if best.is_none() {
        let mut ring_best: Option<(f32, GizmoHandle)> = None;
        let ring_tol = if touch && shape.is_glade() {
            GIZMO_YAW_HIT * 2.0
        } else {
            GIZMO_YAW_HIT
        };
        let rotation_axes: &[(GizmoHandle, Vec3)] = if shape.is_glade() {
            &[(GizmoHandle::Yaw, Vec3::Y)]
        } else {
            &[
                (GizmoHandle::Pitch, Vec3::X),
                (GizmoHandle::Yaw, Vec3::Y),
                (GizmoHandle::Roll, Vec3::Z),
            ]
        };
        for &(handle, axis) in rotation_axes {
            if let Some(hit) = ray.plane_intersection_point(origin, InfinitePlane3d::new(axis)) {
                let radius = (hit - origin).length();
                let error = (radius - ring).abs();
                if error < ring * ring_tol && ring_best.is_none_or(|(d, _)| error < d) {
                    ring_best = Some((error, handle));
                }
            }
        }
        best = ring_best;
    }

    best.map(|(_, handle)| handle)
}

/// Start a gizmo drag from a press at `screen`: hit-test the selected
/// piece's handles and resolve the world grab point. Shared by the desktop
/// mouse path and the touch path so both grab the same handles. Returns the
/// handle, the grabbed piece snapshot, and the grab point.
pub(super) fn gizmo_grab(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    ray: Ray3d,
    touch: bool,
) -> Option<(GizmoHandle, Piece, Vec3)> {
    let handle = gizmo_hit(editor, camera, camera_transform, screen, ray, touch)?;
    let start = editor
        .selection
        .primary()
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id).copied())?;
    let shape = shapes().get(start.shape)?;
    let grab = gizmo_drag_point(ray, handle, gizmo_origin(&start, shape))?;
    Some((handle, start, grab))
}

/// The drag state for a grab that has just started: the grabbed piece plus
/// the rest of the selection, which follows it. Both pointer paths (desktop
/// mouse and touch) build the drag here so they can never disagree about who
/// moves.
pub(super) fn dragged_group(editor: &Editor, id: u32) -> Vec<Piece> {
    let mut ids: Vec<u32> = editor
        .selection
        .ids()
        .iter()
        .copied()
        .filter(|selected| *selected != id)
        .collect();
    for mate_id in editor.attached_building_mates(id) {
        if !ids.contains(&mate_id) {
            ids.push(mate_id);
        }
    }
    ids.into_iter()
        .filter_map(|selected| editor.pieces.iter().find(|piece| piece.id == selected).copied())
        .collect()
}

/// The pieces a drag may snap against: everything except the pieces travelling
/// with it. A group must not snap onto itself, or one member would land on
/// another's top and then be dragged off the surface it snapped to.
pub(super) fn snap_against(editor: &Editor, moving: &[u32]) -> Vec<Piece> {
    editor
        .pieces
        .iter()
        .filter(|piece| !moving.contains(&piece.id))
        .copied()
        .collect()
}

/// Finish the gizmo drag owned by `pointer`, if any. Mirrors the mouse
/// release in `interact`: the commit ran at grab time, so mark the finished
/// pose dirty. Also kills the release tap and suppresses canvas taps briefly,
/// so lifting the finger does not reselect the piece it just moved.
pub(super) fn release_gizmo_drag(
    editor: &mut Editor,
    touch: &mut TouchState,
    pointer: PointerId,
    now: f64,
) {
    if editor
        .gizmo_drag
        .as_ref()
        .is_some_and(|drag| drag.pointer == pointer)
    {
        editor.hard_dirty = true;
        editor.gizmo_drag = None;
        touch.tap = None;
        touch.suppress_until = touch.suppress_until.max(now + 0.25);
    }
}

/// The point on the drag line or plane that the pointer ray meets. Quantities
/// are measured from the grab position, so the piece follows the pointer's
/// travel rather than jumping to where it points.
pub(super) fn gizmo_drag_point(ray: Ray3d, handle: GizmoHandle, origin: Vec3) -> Option<Vec3> {
    match handle {
        GizmoHandle::X => ray_axis_point(ray, origin, Vec3::X),
        GizmoHandle::Z => ray_axis_point(ray, origin, Vec3::Z),
        GizmoHandle::Y | GizmoHandle::GladeHeight | GizmoHandle::GladeRoofPitch => {
            vertical_drag_point(ray, origin)
        }
        GizmoHandle::Xy => plane_ray_point(ray, origin, Vec3::Z),
        GizmoHandle::Zy => plane_ray_point(ray, origin, Vec3::X),
        GizmoHandle::Xz
        | GizmoHandle::Yaw
        | GizmoHandle::GladeWidth
        | GizmoHandle::GladeLength => plane_ray_point(ray, origin, Vec3::Y),
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
            let shape = &shapes()[start.shape];
            if shape.is_glade() && matches!(handle, GizmoHandle::Pitch | GizmoHandle::Roll) {
                return piece;
            }
            let axis = match handle {
                GizmoHandle::Pitch => Vec3::X,
                GizmoHandle::Yaw => Vec3::Y,
                GizmoHandle::Roll => Vec3::Z,
                _ => unreachable!(),
            };
            let origin = gizmo_origin(&start, shape);
            if let Some(angle) = signed_angle_around(axis, grab - origin, current - origin) {
                let rotation = Quat::from_axis_angle(axis, angle) * model::piece_quat(&start);
                model::set_piece_quat(&mut piece, rotation);
            }
        }
        GizmoHandle::GladeWidth
        | GizmoHandle::GladeLength
        | GizmoHandle::GladeHeight
        | GizmoHandle::GladeRoofPitch => {
            let shape = &shapes()[start.shape];
            let mut ext = crate::glade::piece_extents(&start, shape);
            let root = Vec3::new(start.pos.x, start.elevation, start.pos.y);
            let inv_rot = model::piece_quat(&start).inverse();
            let local_grab = inv_rot * (grab - root);
            let local_curr = inv_rot * (current - root);
            let kind = crate::glade::GladeKind::from_shape(shape);
            match handle {
                GizmoHandle::GladeWidth => {
                    let sign = if local_grab.x >= 0.0 { 1.0 } else { -1.0 };
                    let dw = (local_curr.x - local_grab.x) * sign * 2.0;
                    let (min_w, max_w) = match kind {
                        Some(crate::glade::GladeKind::Wall) => (0.8, crate::glade::GLADE_MAX_SPAN),
                        Some(crate::glade::GladeKind::Terrain) => (
                            crate::glade::TERRAIN_MIN_SPAN,
                            crate::glade::TERRAIN_MAX_SPAN,
                        ),
                        _ => (1.0, crate::glade::GLADE_MAX_SPAN),
                    };
                    ext.x = (ext.x + dw).clamp(min_w, max_w);
                    if kind == Some(crate::glade::GladeKind::Tower) {
                        ext.z = ext.x;
                    }
                    piece.trigger_size = ext;
                }
                GizmoHandle::GladeLength => {
                    let sign = if local_grab.z >= 0.0 { 1.0 } else { -1.0 };
                    let dz = (local_curr.z - local_grab.z) * sign * 2.0;
                    let (min_z, max_z) = match kind {
                        Some(crate::glade::GladeKind::Wall) => (0.22, 1.4),
                        Some(crate::glade::GladeKind::Terrain) => (
                            crate::glade::TERRAIN_MIN_SPAN,
                            crate::glade::TERRAIN_MAX_SPAN,
                        ),
                        _ => (1.0, crate::glade::GLADE_MAX_SPAN),
                    };
                    ext.z = (ext.z + dz).clamp(min_z, max_z);
                    if kind == Some(crate::glade::GladeKind::Tower) {
                        ext.x = ext.z;
                    }
                    piece.trigger_size = ext;
                }
                GizmoHandle::GladeHeight => {
                    let (min_h, max_h) = match kind {
                        Some(crate::glade::GladeKind::Terrain) => (
                            crate::glade::TERRAIN_MIN_HEIGHT,
                            crate::glade::TERRAIN_MAX_HEIGHT,
                        ),
                        _ => (
                            crate::glade::GLADE_MIN_HEIGHT,
                            crate::glade::GLADE_MAX_HEIGHT,
                        ),
                    };
                    ext.y = (ext.y + delta.y).clamp(min_h, max_h);
                    piece.trigger_size = ext;
                }
                GizmoHandle::GladeRoofPitch => {
                    let start_alpha = start.trigger_alpha.clamp(0.0, 1.0);
                    piece.trigger_alpha = (start_alpha + delta.y * 0.25).clamp(0.0, 1.0);
                }
                _ => {}
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
    let terrain_h = crate::glade::terrain_height_at(piece.pos, others)
        .max(crate::glade::world_terrain_height_at(piece.pos, others));
    if terrain_h > 0.01 {
        consider(terrain_h - shape.base);
    }
    for other in others {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if let Some(chunk_h) = crate::glade::terrain_chunk_height_at(other, other_shape, piece.pos)
        {
            consider(chunk_h - shape.base);
            continue;
        }
        let (low, high) = piece_bounds(other, other_shape);
        if piece.pos.x < low.x - HIT_PAD
            || piece.pos.x > high.x + HIT_PAD
            || piece.pos.y < low.y - HIT_PAD
            || piece.pos.y > high.y + HIT_PAD
        {
            continue;
        }
        let other_top = if other_shape.is_glade() {
            crate::glade::piece_extents(other, other_shape).y
        } else {
            other_shape.top()
        };
        consider(other.elevation + other_top - shape.base);
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
        GizmoHandle::X | GizmoHandle::Z | GizmoHandle::Xz
    ) && matches!(shape.id, "glade_door" | "glade_window" | "door" | "gate")
        && (piece.pos - drag.start.pos).length_squared() > 1e-8
    {
        let mut moving: Vec<u32> = drag.others.iter().map(|other| other.id).collect();
        moving.push(drag.id);
        let candidates = snap_against(editor, &moving);
        if let Some((snapped_pos, snapped_rot, wall_elev)) =
            crate::glade::snap_door_to_glade_wall_with_elevation(&piece, &candidates, Some(ray), &[])
        {
            piece.pos = snapped_pos;
            piece.rot = snapped_rot;
            piece.elevation = wall_elev;
        }
    }

    if matches!(
        drag.handle,
        GizmoHandle::Y | GizmoHandle::Xy | GizmoHandle::Zy
    ) {
        if (raw - drag.last_raw).abs() > 1e-6 {
            let mut moving: Vec<u32> = drag.others.iter().map(|other| other.id).collect();
            moving.push(drag.id);
            let candidates = snap_against(editor, &moving);
            piece.elevation = snap_elevation(
                piece,
                shape,
                &candidates,
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
    // The rest of the selection (and any building attachments) takes the same
    // delta as the grabbed piece, orbiting around `drag.start` when rotated.
    let delta_rot = model::piece_quat(&piece) * model::piece_quat(&drag.start).inverse();
    for other in &drag.others {
        let mut moved = *other;
        let rel = Vec3::new(
            other.pos.x - drag.start.pos.x,
            other.elevation - drag.start.elevation,
            other.pos.y - drag.start.pos.y,
        );
        let rotated_rel = delta_rot * rel;
        moved.pos = piece.pos + Vec2::new(rotated_rel.x, rotated_rel.z);
        moved.elevation = piece.elevation + rotated_rel.y;
        model::set_piece_quat(&mut moved, delta_rot * model::piece_quat(other));
        if let Some(target) = editor.pieces.iter_mut().find(|target| target.id == other.id) {
            *target = moved;
        }
    }

    let old_piece = editor.pieces[index];
    editor.pieces[index] = piece;
    if matches!(
        drag.handle,
        GizmoHandle::GladeWidth | GizmoHandle::GladeLength
    ) {
        crate::glade::pin_doors_on_glade_resize(&old_piece, &piece, &mut editor.pieces);
    }
    if shape.id == "glade_terrain" {
        let snapshot = editor.pieces.clone();
        let terrain_idx = drag.start.shape;
        let reach = piece.trigger_size.x.max(piece.trigger_size.z) * 0.65 + 2.0;
        for other in &mut editor.pieces {
            if other.shape == terrain_idx {
                continue;
            }
            if other.pos.distance(piece.pos) <= reach {
                let ground_h = crate::glade::world_terrain_height_at(other.pos, &snapshot);
                if (other.elevation - ground_h).abs() <= 5.0 {
                    other.elevation = ground_h;
                }
            }
        }
    }
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

/// Reach of a motion-path waypoint's move gizmo, in metres. Fixed (a
/// waypoint has no bounding box to size against), big enough to grab.
/// Axes and diamond heads use the same fractions as the piece gizmo.
pub(super) const MOVER_GIZMO_REACH: f32 = 1.1;

/// Move gizmo for the selected motion-path waypoint: ground-plane X/Z
/// shafts running both ways from the point with outward diamond heads, the
/// same method as the piece transform gizmo. No Y shaft: waypoints live on
/// the ground plane and only ever move in it.
pub(super) fn mover_waypoint_gizmo(gizmos: &mut Gizmos, at: Vec3) {
    for (dir, colour) in [(Vec3::X, GIZMO_X), (Vec3::Z, GIZMO_Z)] {
        gizmos.line(
            at - dir * MOVER_GIZMO_REACH,
            at + dir * MOVER_GIZMO_REACH,
            colour,
        );
        for (end, outward) in [
            (at + dir * MOVER_GIZMO_REACH, dir),
            (at - dir * MOVER_GIZMO_REACH, -dir),
        ] {
            let back = end - outward * (MOVER_GIZMO_REACH * GIZMO_ARROW_BACK);
            let side = Vec3::new(dir.z, 0.0, dir.x) * (MOVER_GIZMO_REACH * GIZMO_ARROW_SIDE);
            closed(gizmos, &[back + side, end, back - side], colour);
        }
    }
}

/// Actor facing arrow: a ground-level shaft running outward from the
/// character ring edge, tipped with the same diamond head as the transform
/// gizmo arrows. All sizes in metres.
pub(super) const ACTOR_ARROW_INNER: f32 = 0.44;
pub(super) const ACTOR_ARROW_TIP: f32 = 1.24;
pub(super) const ACTOR_ARROW_HEAD_BACK: f32 = 0.2;
pub(super) const ACTOR_ARROW_HEAD_SIDE: f32 = 0.1;

/// Unit facing direction for an actor yaw, on the ground plane. Matches the
/// movement code's convention (`yaw = atan2(-dx, -dy)`), so the arrow agrees
/// with where the capsule actually walks.
pub(super) fn actor_facing(yaw: f32) -> Vec2 {
    Vec2::new(-yaw.sin(), -yaw.cos())
}

/// Shaft endpoints and diamond-head triangle of the actor facing arrow, in
/// world space on the ground plane under the capsule. Pure so tests can pin
/// the facing math without a window.
pub(super) fn actor_arrow_geometry(pos: Vec2, feet: f32, yaw: f32) -> (Vec3, Vec3, [Vec3; 3]) {
    let facing = actor_facing(yaw);
    let base = Vec3::new(pos.x, feet + 0.05, pos.y);
    let dir = Vec3::new(facing.x, 0.0, facing.y);
    let start = base + dir * ACTOR_ARROW_INNER;
    let tip = base + dir * ACTOR_ARROW_TIP;
    let back = tip - dir * ACTOR_ARROW_HEAD_BACK;
    let side = Vec3::new(-facing.y, 0.0, facing.x) * ACTOR_ARROW_HEAD_SIDE;
    (start, tip, [back + side, tip, back - side])
}

/// Draw the actor facing arrow the same way the transform gizmo draws its
/// arrows: a shaft line plus a closed diamond head, in the actor's colour.
pub(super) fn actor_arrow(gizmos: &mut Gizmos, character: Character) {
    let (start, tip, head) = actor_arrow_geometry(character.pos, character.feet, character.yaw);
    gizmos.line(start, tip, PILL_COLOR);
    closed(gizmos, &head, PILL_COLOR);
}

pub(super) fn selection_box(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let points = if shape.is_glade() {
        crate::glade::local_footprint(piece, shape)
    } else {
        model::shape_points(shape)
    };
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rotation = model::piece_quat(piece);
    let at = |p: Vec2, y: f32| root + rotation * Vec3::new(p.x, y, p.y);
    let low = shape.base - 0.01;
    let high = if shape.is_glade() {
        crate::glade::piece_extents(piece, shape).y + 0.01
    } else {
        shape.top() + 0.01
    };
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
    let scale = if shape.is_glade() {
        let ext = crate::glade::piece_extents(piece, shape);
        let def = crate::glade::default_size(shape);
        Vec2::new(
            (ext.x / def.x.max(0.1)).max(0.1),
            (ext.z / def.z.max(0.1)).max(0.1),
        )
    } else {
        Vec2::ONE
    };
    let top_y = if shape.is_glade() {
        crate::glade::piece_extents(piece, shape).y
    } else {
        shape.top()
    };
    for anchor in &shape.anchors {
        let c = root
            + rotation
                * Vec3::new(
                    anchor.local.x * scale.x,
                    top_y + 0.03,
                    anchor.local.y * scale.y,
                );
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
