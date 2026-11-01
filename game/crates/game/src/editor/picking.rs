//! picking systems for the level editor.

use super::*;

pub(super) fn pick(pieces: &[Piece], ray: Ray3d) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for piece in pieces {
        let shape = &shapes()[piece.shape];
        if let Some(distance) = ray_piece_distance(piece, shape, ray) {
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, piece.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

pub(super) fn ray_piece_distance(piece: &Piece, shape: &model::Shape, ray: Ray3d) -> Option<f32> {
    // Into the piece's local frame, including pitch and roll. The model's `y`
    // is vertical and its `(x, z)` is the ground plane.
    let root = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let inverse = model::piece_quat(piece).inverse();
    let origin = inverse * (ray.origin - root);
    let dir = inverse * *ray.direction;

    let points = model::shape_points(shape);
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for p in &points {
        low = low.min(*p);
        high = high.max(*p);
    }

    let pad = HIT_PAD;
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    slab(
        origin.x,
        dir.x,
        low.x - pad,
        high.x + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.z,
        dir.z,
        low.y - pad,
        high.y + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.y,
        dir.y,
        shape.base - pad,
        shape.top() + pad,
        &mut near,
        &mut far,
    )?;

    (far >= 0.0).then(|| near.max(0.0))
}

pub(super) fn slab(
    origin: f32,
    dir: f32,
    low: f32,
    high: f32,
    near: &mut f32,
    far: &mut f32,
) -> Option<()> {
    if dir.abs() < 1e-6 {
        return (origin >= low && origin <= high).then_some(());
    }
    let mut a = (low - origin) / dir;
    let mut b = (high - origin) / dir;
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    *near = near.max(a);
    *far = far.min(b);
    (*near <= *far).then_some(())
}
