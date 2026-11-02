//! picking systems for the level editor.

use std::collections::BTreeSet;

use super::*;

pub(super) fn pick(pieces: &[Piece], ray: Ray3d) -> Option<u32> {
    pick_where(pieces, ray, |_| true)
}

/// Viewport selection ignores locked pieces; the object graph remains the
/// place to select one so its lock can be released.
pub(super) fn pick_unlocked(pieces: &[Piece], ray: Ray3d, locked: &BTreeSet<u32>) -> Option<u32> {
    pick_where(pieces, ray, |piece| !locked.contains(&piece.id))
}

fn pick_where(pieces: &[Piece], ray: Ray3d, allow: impl Fn(&Piece) -> bool) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for piece in pieces {
        if !allow(piece) {
            continue;
        }
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

    if shape.is_trigger {
        let size = model::sanitize_trigger_size(piece.trigger_size);
        let form = model::piece_trigger_shape(piece, shape);
        // Use axis-aligned bounds for picking, even for sphere/cylinder: the
        // box hull is a generous click target and keeps the math simple.
        // For sphere the hull is the box; for cylinder it's also the box.
        let hw = size.x * 0.5;
        let hl = size.z * 0.5;
        let _ = form; // shape only matters for overlap, not picking
        let pad = HIT_PAD;
        let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
        slab(origin.x, dir.x, -hw - pad, hw + pad, &mut near, &mut far)?;
        slab(origin.z, dir.z, -hl - pad, hl + pad, &mut near, &mut far)?;
        slab(origin.y, dir.y, -pad, size.y + pad, &mut near, &mut far)?;
        return (far >= 0.0).then(|| near.max(0.0));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Dir3;

    fn piece(id: u32) -> Piece {
        Piece {
            id,
            shape: model::shape_index("crate").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: model::Trigger::harmless(),
            trigger_size: model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: model::TriggerShape::default(),
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: model::TRIGGER_DEFAULT_VISIBLE,
        }
    }

    fn down_ray() -> Ray3d {
        Ray3d::new(Vec3::new(0.0, 4.0, 0.0), Dir3::NEG_Y)
    }

    #[test]
    fn locked_pieces_are_skipped_by_viewport_picks() {
        let pieces = vec![piece(1)];
        assert_eq!(pick(&pieces, down_ray()), Some(1));

        let mut locked = BTreeSet::new();
        locked.insert(1);
        assert_eq!(pick_unlocked(&pieces, down_ray(), &locked), None);
    }

    #[test]
    fn a_locked_piece_lets_the_pick_through_to_the_next_one() {
        // Two crates at the same spot: locking the first exposes the second.
        let pieces = vec![piece(1), piece(2)];
        let mut locked = BTreeSet::new();
        locked.insert(1);
        assert_eq!(pick_unlocked(&pieces, down_ray(), &locked), Some(2));
    }
}
