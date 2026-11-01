//! colliders systems for the level editor.

use super::*;

pub(super) fn piece_collider(shape: &model::Shape) -> Option<Collider> {
    let height = shape.height.max(0.02);
    let mid = shape.base + height * 0.5;

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

    let body = match &shape.geom {
        model::Geometry::Bar { len, thick } => Collider::cuboid(*len, height, *thick),
        model::Geometry::Rect { w, h } => Collider::cuboid(*w, height, *h),
        model::Geometry::Circle { r } => Collider::cylinder(*r, height),
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
                (high.x - low.x).max(0.05),
                height,
                (high.y - low.y).max(0.05),
            )
        }
    };

    Some(Collider::compound(vec![(
        Position(Vec3::new(0.0, mid, 0.0).real()),
        Rotation::default(),
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
