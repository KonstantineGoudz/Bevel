//! colliders systems for the level editor.

use super::*;

#[allow(dead_code)]
pub fn piece_collider_for(piece: &Piece, shape: &model::Shape) -> Option<Collider> {
    piece_collider_for_glade(piece, shape, &[], &[], None)
}

#[allow(dead_code)]
pub fn piece_collider_for_with_spine(
    piece: &Piece,
    shape: &model::Shape,
    wall_spine: Option<&[Vec2]>,
) -> Option<Collider> {
    piece_collider_for_glade(piece, shape, &[], &[], wall_spine)
}

/// Helper that decomposes an exposed wall segment into Rapier compound boxes,
/// carving out any archway door openings so players can walk through.
fn wall_segment_collider_boxes(
    seg: &crate::glade::ExposedWallSegment,
    wall_depth: f32,
    inward_offset: bool,
    openings: &[crate::glade::ArchOpening],
    parts: &mut Vec<(Vec3, Quat, Collider)>,
) {
    let seg_len = seg.length();
    if seg_len < 0.05 || seg.height() < 0.05 {
        return;
    }
    let d = seg.p1 - seg.p0;
    let tangent = d / seg_len;
    let yaw = -d.y.atan2(d.x);
    let rot = Quat::from_rotation_y(yaw);

    let offset_2d = if inward_offset {
        -seg.normal * (wall_depth * 0.5)
    } else {
        Vec2::ZERO
    };

    let push_sub_box = |parts: &mut Vec<(Vec3, Quat, Collider)>,
                        u0: f32,
                        u1: f32,
                        y0: f32,
                        y1: f32| {
        let sub_len = u1 - u0;
        let sub_h = y1 - y0;
        if sub_len < 0.04 || sub_h < 0.04 {
            return;
        }
        let mid_u = (u0 + u1) * 0.5;
        let center_2d = seg.p0 + tangent * mid_u + offset_2d;
        let mid_y = (y0 + y1) * 0.5;
        parts.push((
            Vec3::new(center_2d.x, mid_y, center_2d.y),
            rot,
            Collider::cuboid(sub_len * 0.5, sub_h * 0.5, wall_depth * 0.5),
        ));
    };

    if openings.is_empty() {
        push_sub_box(parts, 0.0, seg_len, seg.base_y, seg.top_y);
        return;
    }

    let mut cursor_u = 0.0_f32;
    for arch in openings {
        let arch_start = (arch.center_dist - arch.width * 0.5).max(cursor_u);
        let arch_end = (arch.center_dist + arch.width * 0.5).min(seg_len);

        // Wall portion before the archway
        if arch_start > cursor_u + 0.04 {
            push_sub_box(parts, cursor_u, arch_start, seg.base_y, seg.top_y);
        }

        // Lintel / header portion above the archway
        let crown = arch.crown_height.min(seg.top_y - 0.06);
        if crown > seg.base_y && crown < seg.top_y {
            push_sub_box(parts, arch_start, arch_end, crown, seg.top_y);
        }

        cursor_u = arch_end;
    }

    // Wall portion after the last archway
    if cursor_u + 0.04 < seg_len {
        push_sub_box(parts, cursor_u, seg_len, seg.base_y, seg.top_y);
    }
}

pub fn piece_collider_for_glade(
    piece: &Piece,
    shape: &model::Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    wall_spine: Option<&[Vec2]>,
) -> Option<Collider> {
    if shape.is_glade() {
        let ext = crate::glade::piece_extents(piece, shape);
        let height = ext.y.max(0.05);
        let mid = height * 0.5;

        let solo_piece;
        let effective_pieces = if all_pieces.is_empty() {
            solo_piece = [piece.clone()];
            &solo_piece[..]
        } else {
            all_pieces
        };

        match crate::glade::GladeKind::from_shape(shape) {
            Some(crate::glade::GladeKind::Path) => return None,
            Some(crate::glade::GladeKind::Tower) => {
                let mut exposed =
                    crate::glade::compute_exposed_wall_segments(piece, shape, effective_pieces);
                crate::glade::compute_tower_door_cutouts(
                    piece,
                    &mut exposed,
                    effective_pieces,
                    path_polylines,
                );
                let wall_depth = 0.26;
                let mut parts = Vec::new();

                // Floor plate
                let floor_thick = 0.08;
                parts.push((
                    Vec3::new(0.0, -floor_thick * 0.5, 0.0),
                    Quat::default(),
                    Collider::cylinder(floor_thick * 0.5, ext.x * 0.5),
                ));

                // Ceiling plate
                if height >= 1.8 {
                    let ceil_thick = 0.08;
                    parts.push((
                        Vec3::new(0.0, height + ceil_thick * 0.5, 0.0),
                        Quat::default(),
                        Collider::cylinder(ceil_thick * 0.5, ext.x * 0.5),
                    ));
                }

                for seg in &exposed {
                    wall_segment_collider_boxes(seg, wall_depth, true, &[], &mut parts);
                }

                if parts.is_empty() {
                    return Some(Collider::compound(vec![(
                        Vec3::new(0.0, mid, 0.0),
                        Quat::default(),
                        Collider::cylinder(height * 0.5, ext.x * 0.5),
                    )]));
                }
                return Some(Collider::compound(parts));
            }
            Some(crate::glade::GladeKind::Door) => {
                // Two narrow side jambs so the arched doorway is open to walk through!
                let jamb_w = 0.08_f32.min(ext.x * 0.12);
                let clear_w = (ext.x - jamb_w * 2.0).max(0.78);
                let offset_x = clear_w * 0.5 + jamb_w * 0.5;
                let jamb = Collider::cuboid(jamb_w * 0.5, height * 0.5, ext.z * 0.5);
                let mut parts = vec![
                    (Vec3::new(-offset_x, mid, 0.0), Quat::default(), jamb.clone()),
                    (Vec3::new(offset_x, mid, 0.0), Quat::default(), jamb),
                ];
                if height > 1.95 {
                    let header_h = (height - 1.85).min(height * 0.25);
                    let header = Collider::cuboid(ext.x * 0.5, header_h * 0.5, ext.z * 0.5);
                    parts.push((
                        Vec3::new(0.0, height - header_h * 0.5, 0.0),
                        Quat::default(),
                        header,
                    ));
                }
                return Some(Collider::compound(parts));
            }
            Some(crate::glade::GladeKind::Window) => {
                return Some(Collider::compound(vec![(
                    Vec3::new(0.0, mid, 0.0),
                    Quat::default(),
                    Collider::cuboid(ext.x * 0.5, height * 0.5, ext.z * 0.5),
                )]));
            }
            Some(crate::glade::GladeKind::Wall) => {
                let mut parts = Vec::new();
                if let Some(spine) = wall_spine.filter(|pts| pts.len() >= 2) {
                    for pair in spine.windows(2) {
                        let p0 = crate::glade::world_to_local_2d(piece, pair[0]);
                        let p1 = crate::glade::world_to_local_2d(piece, pair[1]);
                        let d = p1 - p0;
                        let len = d.length();
                        if len < 0.05 {
                            continue;
                        }
                        let tangent = d / len;
                        let normal = Vec2::new(-tangent.y, tangent.x);
                        let seg = crate::glade::ExposedWallSegment {
                            p0,
                            p1,
                            normal,
                            base_y: 0.0,
                            top_y: height,
                            corner_start: false,
                            corner_end: false,
                            seam_start: false,
                            seam_end: false,
                        };
                        let openings = crate::glade::compute_arch_openings(
                            piece,
                            &seg,
                            effective_pieces,
                            path_polylines,
                        );
                        wall_segment_collider_boxes(&seg, ext.z, false, &openings, &mut parts);
                    }
                } else {
                    let hl = ext.x * 0.5;
                    let seg = crate::glade::ExposedWallSegment {
                        p0: Vec2::new(-hl, 0.0),
                        p1: Vec2::new(hl, 0.0),
                        normal: Vec2::new(0.0, 1.0),
                        base_y: 0.0,
                        top_y: height,
                        corner_start: true,
                        corner_end: true,
                        seam_start: false,
                        seam_end: false,
                    };
                    let openings = crate::glade::compute_arch_openings(
                        piece,
                        &seg,
                        effective_pieces,
                        path_polylines,
                    );
                    wall_segment_collider_boxes(&seg, ext.z, false, &openings, &mut parts);
                }
                if parts.is_empty() {
                    return Some(Collider::compound(vec![(
                        Vec3::new(0.0, mid, 0.0),
                        Quat::default(),
                        Collider::cuboid(ext.x * 0.5, height * 0.5, ext.z * 0.5),
                    )]));
                }
                return Some(Collider::compound(parts));
            }
            Some(crate::glade::GladeKind::Terrain) => {
                let (verts, indices) = crate::glade::terrain_surface_vertices_and_indices(
                    piece,
                    shape,
                    effective_pieces,
                );
                if let Ok(trimesh) = Collider::trimesh(verts, indices) {
                    return Some(trimesh);
                }
                return Some(Collider::cylinder(height * 0.5, ext.x.max(ext.z) * 0.5));
            }
            _ => {
                // GladeKind::Room ("house")
                // Hollow building perimeter wall colliders with archway cutouts for doors
                let exposed =
                    crate::glade::compute_exposed_wall_segments(piece, shape, effective_pieces);
                let wall_depth = 0.26;
                let mut parts = Vec::new();

                // Floor plate
                let floor_thick = 0.08;
                parts.push((
                    Vec3::new(0.0, -floor_thick * 0.5, 0.0),
                    Quat::default(),
                    Collider::cuboid(ext.x * 0.5, floor_thick * 0.5, ext.z * 0.5),
                ));

                // Ceiling plate
                if height >= 1.8 {
                    let ceil_thick = 0.08;
                    parts.push((
                        Vec3::new(0.0, height + ceil_thick * 0.5, 0.0),
                        Quat::default(),
                        Collider::cuboid(ext.x * 0.5, ceil_thick * 0.5, ext.z * 0.5),
                    ));
                }

                for seg in &exposed {
                    let openings = crate::glade::compute_arch_openings(
                        piece,
                        seg,
                        effective_pieces,
                        path_polylines,
                    );
                    wall_segment_collider_boxes(seg, wall_depth, true, &openings, &mut parts);
                }

                if parts.is_empty() {
                    return Some(Collider::compound(vec![(
                        Vec3::new(0.0, mid, 0.0),
                        Quat::default(),
                        Collider::cuboid(ext.x * 0.5, height * 0.5, ext.z * 0.5),
                    )]));
                }
                return Some(Collider::compound(parts));
            }
        }
    }
    piece_collider(shape)
}

pub fn piece_collider(shape: &model::Shape) -> Option<Collider> {
    if shape.is_trigger {
        return None;
    }
    let height = shape.height.max(0.02);
    let mid = shape.base + height * 0.5;

    // Threshold pieces (doors and gates) have open side jambs so characters can walk through
    if matches!(shape.style, Style::Door | Style::Gate) {
        let (len, thick) = match shape.geom {
            model::Geometry::Bar { len, thick } => (len, thick),
            _ => (1.2, 0.2),
        };
        let jamb_w = 0.14_f32.min(len * 0.15);
        let clear_w = (len - jamb_w * 2.0).max(0.72);
        let offset_x = clear_w * 0.5 + jamb_w * 0.5;
        let jamb = Collider::cuboid(jamb_w * 0.5, height * 0.5, thick * 0.5);
        let mut parts = vec![
            (Vec3::new(-offset_x, mid, 0.0), Quat::default(), jamb.clone()),
            (Vec3::new(offset_x, mid, 0.0), Quat::default(), jamb),
        ];
        if height > 1.95 {
            let header_h = (height - 1.85).min(height * 0.25);
            let header = Collider::cuboid(len * 0.5, header_h * 0.5, thick * 0.5);
            parts.push((
                Vec3::new(0.0, shape.base + height - header_h * 0.5, 0.0),
                Quat::default(),
                header,
            ));
        }
        return Some(Collider::compound(parts));
    }

    // A wedge is convex already, so its six corners are the whole collider.
    if let model::Geometry::Wedge { w, d, h } = &shape.geom {
        let (hw, hd) = (w / 2.0, d / 2.0);
        let base = shape.base;
        let top = shape.base + *h;
        let points = [
            Vec3::new(-hw, base, -hd),
            Vec3::new(hw, base, -hd),
            Vec3::new(hw, base, hd),
            Vec3::new(-hw, base, hd),
            Vec3::new(hw, top, hd),
            Vec3::new(-hw, top, hd),
        ];
        return Collider::convex_hull(&points);
    }

    // Rapier takes half extents with some argument orders flipped
    // (cylinder/cone/capsule put the height first).
    let body = match &shape.geom {
        model::Geometry::Bar { len, thick } => {
            Collider::cuboid(len * 0.5, height * 0.5, thick * 0.5)
        }
        model::Geometry::Rect { w, h } => Collider::cuboid(w * 0.5, height * 0.5, h * 0.5),
        model::Geometry::Circle { r } => Collider::cylinder(height * 0.5, *r),
        model::Geometry::Hex { .. } | model::Geometry::Diamond { .. } => {
            let outline = model::shape_points(shape);
            let mut hull = Vec::with_capacity(outline.len() * 2);
            for point in &outline {
                hull.push(Vec3::new(point.x, shape.base, point.y));
                hull.push(Vec3::new(point.x, shape.top(), point.y));
            }
            Collider::convex_hull(&hull)?
        }
        _ => {
            // Cross and flag outlines are concave; their footprint box is a
            // fair collider and much cheaper.
            let outline = model::shape_points(shape);
            let (low, high) = outline_bounds(&outline);
            Collider::cuboid(
                (high.x - low.x).max(0.05) * 0.5,
                height * 0.5,
                (high.y - low.y).max(0.05) * 0.5,
            )
        }
    };

    Some(Collider::compound(vec![(
        Vec3::new(0.0, mid, 0.0),
        Quat::default(),
        body,
    )]))
}

pub(super) fn outline_bounds(points: &[Vec2]) -> (Vec2, Vec2) {
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for point in points {
        low = low.min(*point);
        high = high.max(*point);
    }
    (low, high)
}

/// The meshes that make up one catalogue entry: outline, base, height, slot.
/// Slot 0 is the shape's own colour, slot 1 is glass.
pub(super) fn solid_parts(shape: &model::Shape) -> Vec<(Vec<Vec2>, f32, f32, u8)> {
    let outline = model::shape_points(shape);
    if shape.style == Style::Window {
        let sill = 0.95;
        let head = 2.2;
        vec![
            (outline.clone(), shape.base, sill, 0),
            (outline.clone(), head, (shape.top() - head).max(0.05), 0),
            (outline, sill, (head - sill).max(0.05), 1),
        ]
    } else {
        vec![(outline, shape.base, shape.height.max(0.02), 0)]
    }
}
