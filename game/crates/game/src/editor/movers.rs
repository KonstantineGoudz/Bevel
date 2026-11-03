//! Moving platforms: per-piece Bezier paths with ride carry.
//!
//! Each mover belongs to one piece id. Its waypoints build a smoothed cubic
//! Bezier [`path::Track`] (same curve family as the character route); the
//! per-frame system advances along arc length so speed is constant, then
//! adds the platform delta to a standing rider so they move with it.

use std::collections::HashMap;

use avian3d::collision::collider::contact_query::contact;

use super::*;
use crate::platforms::{self, PlatformMode, PlatformWaypoint};

/// Default travel speed, m/s.
pub const DEFAULT_PLATFORM_SPEED: f32 = 1.5;

#[derive(Clone, Debug)]
pub struct Mover {
    pub waypoints: Vec<PlatformWaypoint>,
    /// Selected waypoint index for handle dragging and Delete.
    pub selected: Option<usize>,
    pub track: Option<path::Track>,
    pub mode: PlatformMode,
    pub speed: f32,
    pub distance: f32,
    pub dir: f32,
    pub paused: bool,
}

impl Mover {
    pub fn new(start: Vec2) -> Self {
        Self {
            waypoints: vec![PlatformWaypoint::new(start)],
            selected: None,
            track: None,
            mode: PlatformMode::default(),
            speed: DEFAULT_PLATFORM_SPEED,
            distance: 0.0,
            dir: 1.0,
            paused: false,
        }
    }

    pub fn rebuild(&mut self) {
        if self.waypoints.len() >= 2 {
            // Loop mode is a true closed circuit: the end connects back to
            // the start so the platform never jumps.
            let closed = self.mode == PlatformMode::Loop;
            self.track = Some(platforms::build_platform_track(&self.waypoints, closed));
            // Keep progress in range after edits.
            let total = self.track.as_ref().map(|t| t.total()).unwrap_or(0.0);
            self.distance = self.distance.clamp(0.0, total);
        } else {
            self.track = None;
            self.distance = 0.0;
        }
    }

    /// Advance one step; returns the target ground position, if moving.
    pub fn step(&mut self, dt: f32) -> Option<Vec2> {
        if self.paused {
            return None;
        }
        let track = self.track.as_ref()?;
        let total = track.total();
        if total <= 0.0 {
            return None;
        }
        let (distance, dir) =
            platforms::advance_platform(self.distance, self.dir, total, self.speed, dt, self.mode);
        self.distance = distance;
        self.dir = dir;
        Some(track.at(distance).0)
    }
}

#[derive(Resource, Default)]
pub struct Movers {
    pub map: HashMap<u32, Mover>,
}

impl Movers {
    pub fn mover_mut(&mut self, piece_id: u32, start: Vec2) -> &mut Mover {
        self.map
            .entry(piece_id)
            .or_insert_with(|| Mover::new(start))
    }

    pub fn remove_for(&mut self, piece_id: u32) {
        self.map.remove(&piece_id);
    }

    pub fn retain_existing(&mut self, pieces: &[Piece]) {
        let ids: std::collections::HashSet<u32> = pieces.iter().map(|p| p.id).collect();
        self.map.retain(|id, _| ids.contains(id));
    }
}

/// Drop paths whose piece is gone (delete/undo/load).
pub(super) fn prune_movers(editor: Res<Editor>, mut movers: ResMut<Movers>) {
    movers.retain_existing(&editor.pieces);
}

pub(crate) fn to_saved(piece_id: u32, mover: &Mover) -> session::SavedPlatform {
    session::SavedPlatform {
        piece: piece_id,
        mode: mover.mode.id().to_owned(),
        speed: Some(mover.speed),
        paused: mover.paused,
        waypoints: mover
            .waypoints
            .iter()
            .map(|w| session::SavedPlatformPoint {
                x: w.pos.x,
                z: w.pos.y,
                incoming: w.incoming.map(|v| [v.x, v.y]),
                outgoing: w.outgoing.map(|v| [v.x, v.y]),
            })
            .collect(),
    }
}

pub(crate) fn apply_saved(movers: &mut Movers, pieces: &[Piece], saved: &[session::SavedPlatform]) {
    movers.map.clear();
    for entry in saved {
        let Some(piece) = pieces.iter().find(|p| p.id == entry.piece) else {
            continue;
        };
        let waypoints: Vec<PlatformWaypoint> = entry
            .waypoints
            .iter()
            .map(|w| PlatformWaypoint {
                pos: Vec2::new(w.x, w.z),
                incoming: w.incoming.map(|[x, y]| Vec2::new(x, y)),
                outgoing: w.outgoing.map(|[x, y]| Vec2::new(x, y)),
            })
            .collect();
        if waypoints.is_empty() {
            continue;
        }
        let mut mover = Mover::new(piece.pos);
        mover.waypoints = waypoints;
        mover.mode = PlatformMode::parse(&entry.mode).unwrap_or_default();
        mover.speed = entry
            .speed
            .unwrap_or(DEFAULT_PLATFORM_SPEED)
            .clamp(0.5, 8.0);
        mover.paused = entry.paused;
        mover.distance = 0.0;
        mover.dir = 1.0;
        mover.rebuild();
        movers.map.insert(entry.piece, mover);
    }
}

/// Drain the one pending waypoint parked by the Motion tool.
pub(super) fn flush_platform_requests(mut editor: ResMut<Editor>, mut movers: ResMut<Movers>) {
    let Some((id, pos)) = editor.platform_request.take() else {
        return;
    };
    let Some(piece) = editor.pieces.iter().find(|p| p.id == id).copied() else {
        editor.note = "select a piece first, then add motion";
        return;
    };
    let mover = movers.mover_mut(id, piece.pos);
    mover.waypoints.push(PlatformWaypoint::new(pos));
    mover.selected = Some(mover.waypoints.len() - 1);
    mover.rebuild();
    editor.hard_dirty = true;
    editor.note = "motion waypoint added";
}

/// Delete the selected waypoint of the selected piece's path, if any.
/// Returns true when something was removed.
pub(super) fn delete_selected_mover_waypoint(editor: &mut Editor, movers: &mut Movers) -> bool {
    let Some(id) = editor.selected else {
        return false;
    };
    let Some(mover) = movers.map.get_mut(&id) else {
        return false;
    };
    let Some(index) = mover.selected else {
        return false;
    };
    if index >= mover.waypoints.len() {
        mover.selected = None;
        return false;
    }
    mover.waypoints.remove(index);
    mover.selected = None;
    mover.rebuild();
    editor.hard_dirty = true;
    editor.note = "motion waypoint deleted";
    true
}

/// Apply one pointer update to a mover handle: drag the waypoint itself, or
/// reshape the curve through a tangent handle. Ground-plane raycast, like
/// the click that appends waypoints, so handles track the pointer exactly.
pub(super) fn apply_mover_drag(
    editor: &mut Editor,
    movers: &mut Movers,
    drag: PathDrag,
    screen: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> bool {
    let Some(id) = editor.selected else {
        return false;
    };
    let Some(mover) = movers.map.get_mut(&id) else {
        return false;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, screen) else {
        return false;
    };
    let Some(point) = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
    else {
        return false;
    };
    let pos = Vec2::new(point.x, point.z);
    match drag.handle {
        PathHandle::Waypoint(raw) => {
            let index = raw as usize;
            if index >= mover.waypoints.len() {
                return false;
            }
            mover.waypoints[index].pos = pos;
            mover.selected = Some(index);
        }
        PathHandle::Tangent { waypoint, side } => {
            let index = waypoint as usize;
            let Some(entry) = mover.waypoints.get_mut(index) else {
                return false;
            };
            match side {
                TangentSide::Incoming => entry.incoming = Some(pos),
                TangentSide::Outgoing => entry.outgoing = Some(pos),
            }
            mover.selected = Some(index);
        }
    }
    mover.rebuild();
    editor.hard_dirty = true;
    true
}

/// Contact skin for ride detection, in metres. A settled rider rests within
/// this gap of the deck, so the pre-move support check uses the same window
/// the old footprint ride check did.
const RIDE_SKIN: f32 = 0.35;
/// Penetration below this is resting contact, not a shove. Keeps a capsule
/// leaning on a platform from being pushed a skin-step every frame.
const PUSH_EPSILON: f32 = 0.001;
/// Extra clearance added to a depenetration push so float error does not
/// re-trigger contact on the next frame.
const PUSH_SKIN: f32 = 0.01;
/// A contact whose platform normal points this far up is deck support (a
/// flat top or a ramp slope), not a wall. Flat decks report 1.0 and ramp
/// slopes ~0.91; edge and corner clips fall well below, so they shove
/// instead of carrying — that is what keeps a capsule brushed by the side
/// from getting dragged along as a "rider".
const SUPPORT_NORMAL_Y: f32 = 0.7;
/// Minimum alignment between the push direction and the contact normal for
/// a deflected shove. Below this the sideways escape would barely separate,
/// so the push falls back to the bare contact normal.
const PUSH_MIN_ALIGNMENT: f32 = 0.3;

/// World-space capsule pose for a character state.
fn capsule_pose(pos: Vec2, feet: f32, yaw: f32) -> (Vec3, Quat) {
    (
        Vec3::new(pos.x, feet + PILL_HALF, pos.y),
        Quat::from_rotation_y(yaw),
    )
}

/// World-space body pose for a platform piece.
fn platform_pose(piece: &Piece) -> (Vec3, Quat) {
    (
        Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        model::piece_quat(piece),
    )
}

/// Deck height under `at` for ride checks. Flat decks report their top;
/// wedges interpolate along the rise so ramp riders count as standing.
/// Shapes with no standable surface (bars, pillars, …) report `None` and
/// can never carry.
fn deck_height_at(shape: &model::Shape, piece: &Piece, at: Vec2) -> Option<f32> {
    match &shape.geom {
        model::Geometry::Wedge { d, h, .. } => {
            let local = model::to_local(piece, at);
            let t = ((local.y + d * 0.5) / d).clamp(0.0, 1.0);
            Some(piece.elevation + shape.base + t * h)
        }
        model::Geometry::Rect { .. }
        | model::Geometry::Hex { .. }
        | model::Geometry::Diamond { .. } => Some(piece.elevation + shape.top()),
        _ => None,
    }
}

/// True rider: deck-like contact normal, capsule over the deck footprint,
/// feet on the surface. A rim clip or side brush fails the footprint or
/// height test and shoves instead of carrying, so a platform can never
/// drag the capsule along from beside it — the thin-floor rim in particular
/// touches bottom-on-deck with an upward normal while hanging off the edge.
fn is_rider(pos: Vec2, feet: f32, piece: &Piece, shape: &model::Shape, normal_y: f32) -> bool {
    if normal_y <= SUPPORT_NORMAL_Y {
        return false;
    }
    if !model::hit_test(shape, piece, pos, 0.05) {
        return false;
    }
    let Some(deck) = deck_height_at(shape, piece, pos) else {
        return false;
    };
    (feet - deck).abs() <= RIDE_SKIN
}

/// True when the capsule rests on the platform deck: a contact within the
/// ride skin that passes the full rider test (deck-like normal, over the
/// footprint, feet on the surface). Ramps count via their sloped face
/// normal and interpolated surface height.
fn is_supported(
    capsule: &Collider,
    cap_pos: Vec3,
    cap_rot: Quat,
    platform: &Collider,
    plat_pos: Vec3,
    plat_rot: Quat,
    pos: Vec2,
    feet: f32,
    piece: &Piece,
    shape: &model::Shape,
) -> bool {
    let Ok(Some(hit)) = contact(
        capsule,
        cap_pos.real(),
        cap_rot,
        platform,
        plat_pos.real(),
        plat_rot,
        RIDE_SKIN,
    ) else {
        return false;
    };
    if hit.penetration < -RIDE_SKIN {
        return false;
    }
    let normal = plat_rot * hit.local_normal2;
    is_rider(pos, feet, piece, shape, normal.y)
}

/// Push needed to separate an overlapping capsule from a platform, in world
/// space. Returns `None` for triggers (no collider), resting contact, and
/// true riders — the carry owns that case. Pure Parry contact on the real
/// collider shapes, so blocks, walls, and ramp wedges all separate exactly
/// with no 2D footprint approximation.
///
/// The shove runs along the horizontal offset from the platform centre,
/// scaled so its along-normal component clears the penetration. An
/// off-centre hit therefore deflects sideways off the face instead of
/// surfing it forever; a dead-centre hit pushes straight ahead. A deck-edge
/// clip the offset cannot separate falls back to the bare contact normal.
fn contact_push(
    capsule: &Collider,
    cap_pos: Vec3,
    cap_rot: Quat,
    platform: &Collider,
    plat_pos: Vec3,
    plat_rot: Quat,
    pos: Vec2,
    feet: f32,
    piece: &Piece,
    shape: &model::Shape,
) -> Option<Vec3> {
    let Ok(Some(hit)) = contact(
        capsule,
        cap_pos.real(),
        cap_rot,
        platform,
        plat_pos.real(),
        plat_rot,
        0.0,
    ) else {
        return None;
    };
    if hit.penetration <= PUSH_EPSILON {
        return None;
    }
    let normal = plat_rot * hit.local_normal2;
    if !normal.is_finite() || normal.length_squared() < 1e-6 {
        return None;
    }
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO {
        return None;
    }
    if is_rider(pos, feet, piece, shape, normal.y) {
        // True rider on the deck (or a ramp slope): carried, never shoved.
        return None;
    }
    let distance = hit.penetration + PUSH_SKIN;
    let offset = Vec2::new(cap_pos.x - plat_pos.x, cap_pos.z - plat_pos.z);
    let sideways = if offset.length_squared() > 1e-8 {
        offset.normalize_or_zero()
    } else {
        // Dead centre: shove along the contact's own horizontal facing.
        Vec2::new(normal.x, normal.z).normalize_or_zero()
    };
    if sideways == Vec2::ZERO {
        return Some(normal * distance);
    }
    let alignment = normal.x * sideways.x + normal.z * sideways.y;
    if alignment > PUSH_MIN_ALIGNMENT {
        let step = distance / alignment;
        Some(Vec3::new(sideways.x * step, 0.0, sideways.y * step))
    } else {
        Some(normal * distance)
    }
}

/// Move every platform along its track, carrying a standing rider by the
/// same delta and shoving the capsule out of any side overlap via Parry
/// contact on the real collider shapes — the same push whether the capsule
/// is standing still, walking, or airborne.
/// Runs before `character_move` so physics sees the carried position.
/// Also stamps the platform velocity underfoot onto `ride_vel` (zero when
/// not riding) so jumps can inherit the ride's inertia.
pub(super) fn move_movers(
    time: Res<Time>,
    mut movers: ResMut<Movers>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    kit: Res<Kit>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.25);
    if dt <= 0.0 {
        return;
    }
    // Snapshot rider pose before any platform moves.
    let rider = editor.character.as_ref().map(|c| (c.pos, c.feet, c.yaw));
    let capsule = kit.character_collider.clone();
    let mut carry = Vec2::ZERO;
    let carry_feet = 0.0;
    let mut deltas: HashMap<u32, Vec2> = HashMap::new();

    // Advance pieces first, remembering deltas for ridden ones.
    for piece in &mut editor.pieces {
        let Some(mover) = movers.map.get_mut(&piece.id) else {
            continue;
        };
        let Some(target) = mover.step(dt) else {
            continue;
        };
        let delta = target - piece.pos;
        if delta.length_squared() < 1e-12 {
            continue;
        }
        // Was the rider supported by this platform before it moved?
        if let Some((pos, feet, yaw)) = rider {
            let shape = &shapes()[piece.shape];
            if let Some(platform) = piece_collider(shape) {
                let (cap_pos, cap_rot) = capsule_pose(pos, feet, yaw);
                let (plat_pos, plat_rot) = platform_pose(piece);
                if is_supported(
                    &capsule, cap_pos, cap_rot, &platform, plat_pos, plat_rot, pos, feet, piece,
                    shape,
                ) {
                    carry += delta;
                    // Platforms are level for now; keep the hook for ramps.
                    let _ = carry_feet;
                }
            }
        }
        piece.pos = target;
        deltas.insert(piece.id, delta);
    }

    if carry != Vec2::ZERO {
        if let Some(character) = editor.character.as_mut() {
            character.pos += carry;
            character.feet += carry_feet;
        }
        // Third person rides along: shift the orbit centre by the same delta
        // so the camera follows a standing rider. A delta (not a snap) keeps
        // a manual pan offset intact.
        if orbit.mode == ViewMode::Third {
            orbit.target.x += carry.x;
            orbit.target.z += carry.y;
        }
        editor.soft_dirty = true;
    }
    // A platform that ends up overlapping the capsule shoves it clear,
    // deflecting off-centre hits sideways so the capsule slides off the
    // face instead of surfing it. Runs on the post-carry pose against the
    // platforms' new positions, so a still, idle capsule is pushed exactly
    // like an airborne one.
    //
    // The pose is copied out first so the scan can borrow `editor.pieces`
    // immutably while resolving; it is written back once clear.
    let mut push = Vec2::ZERO;
    let mut push_up = 0.0;
    let mut flow = carry;
    if let Some(snapshot) = editor.character.as_ref().map(|c| (c.pos, c.feet, c.yaw)) {
        let (mut pos, mut feet, yaw) = snapshot;
        // Settle stacked platforms: a shove out of one can land inside the
        // next, so re-scan until clear (or the bound below trips).
        for _ in 0..3 {
            let mut moved = false;
            let (cap_pos, cap_rot) = capsule_pose(pos, feet, yaw);
            for piece in &editor.pieces {
                let Some(mover) = movers.map.get(&piece.id) else {
                    continue;
                };
                if mover.track.is_none() {
                    continue;
                }
                let shape = &shapes()[piece.shape];
                let Some(platform) = piece_collider(shape) else {
                    continue;
                };
                let (plat_pos, plat_rot) = platform_pose(piece);
                let Some(shove) = contact_push(
                    &capsule, cap_pos, cap_rot, &platform, plat_pos, plat_rot, pos, feet, piece,
                    shape,
                ) else {
                    continue;
                };
                pos += Vec2::new(shove.x, shove.z);
                feet += shove.y;
                push += Vec2::new(shove.x, shove.z);
                push_up += shove.y;
                if let Some(delta) = deltas.get(&piece.id) {
                    flow += *delta;
                }
                moved = true;
                break;
            }
            if !moved {
                break;
            }
        }
        if push != Vec2::ZERO || push_up != 0.0 {
            if let Some(character) = editor.character.as_mut() {
                character.pos = pos;
                character.feet = feet;
            }
        }
    }
    if push != Vec2::ZERO {
        if orbit.mode == ViewMode::Third {
            orbit.target.x += push.x;
            orbit.target.z += push.y;
        }
        editor.soft_dirty = true;
    }
    // Stamp the ride velocity every frame: the platform's world velocity
    // while carried or shoved, zero otherwise. `character_move` folds this
    // into `hv` on takeoff so jumps inherit the ride, and back out on
    // landing so repeated hops cannot stack speed. Only the platforms'
    // travel counts — the extra depenetration distance is not velocity.
    if let Some(character) = editor.character.as_mut() {
        character.ride_vel = if flow != Vec2::ZERO {
            flow / dt
        } else {
            Vec2::ZERO
        };
    }
}

/// Final depenetration after `character_move` (and route-following `walk`):
/// with the platforms already at their new poses, the grounded controller
/// may still have stepped into one on stale colliders — or idled inside one
/// it was pushed into. Sweep the capsule out along the Parry contact normals
/// so the frame always ends clear, whatever the controller did.
pub(super) fn resolve_platform_push(
    mut editor: ResMut<Editor>,
    movers: Res<Movers>,
    mut orbit: ResMut<Orbit>,
    kit: Res<Kit>,
) {
    let Some((mut pos, mut feet, yaw)) = editor.character.as_ref().map(|c| (c.pos, c.feet, c.yaw))
    else {
        return;
    };
    let capsule = kit.character_collider.clone();
    let mut push = Vec2::ZERO;
    let mut touched = false;
    for _ in 0..3 {
        let mut moved = false;
        let (cap_pos, cap_rot) = capsule_pose(pos, feet, yaw);
        for piece in &editor.pieces {
            let Some(mover) = movers.map.get(&piece.id) else {
                continue;
            };
            if mover.track.is_none() {
                continue;
            }
            let shape = &shapes()[piece.shape];
            let Some(platform) = piece_collider(shape) else {
                continue;
            };
            let (plat_pos, plat_rot) = platform_pose(piece);
            let Some(shove) = contact_push(
                &capsule, cap_pos, cap_rot, &platform, plat_pos, plat_rot, pos, feet, piece, shape,
            ) else {
                continue;
            };
            pos += Vec2::new(shove.x, shove.z);
            feet += shove.y;
            push += Vec2::new(shove.x, shove.z);
            touched = true;
            moved = true;
            break;
        }
        if !moved {
            break;
        }
    }
    if touched {
        if let Some(character) = editor.character.as_mut() {
            character.pos = pos;
            character.feet = feet;
        }
        if push != Vec2::ZERO {
            if orbit.mode == ViewMode::Third {
                orbit.target.x += push.x;
                orbit.target.z += push.y;
            }
        }
        editor.soft_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_piece(id: u32, pos: Vec2) -> Piece {
        Piece {
            id,
            shape: model::shape_index("block").unwrap(),
            pos,
            rot: 0.0,
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
        }
    }

    #[test]
    fn a_mover_needs_two_waypoints_to_move() {
        let mut mover = Mover::new(Vec2::ZERO);
        assert!(mover.step(1.0).is_none());
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 4.0));
        mover.rebuild();
        let target = mover.step(1.0);
        assert!(target.is_some());
        assert!(target.unwrap().x > 0.0);
    }

    #[test]
    fn a_paused_mover_holds_still() {
        let mut mover = Mover::new(Vec2::ZERO);
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 4.0));
        mover.rebuild();
        mover.paused = true;
        assert!(mover.step(1.0).is_none());
    }

    #[test]
    fn platform_path_survives_a_save_round_trip() {
        let mut mover = Mover::new(Vec2::ZERO);
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 4.0));
        mover.mode = PlatformMode::Loop;
        mover.speed = 2.5;
        mover.paused = true;
        mover.rebuild();
        let saved = to_saved(7, &mover);
        assert_eq!(saved.piece, 7);
        assert_eq!(saved.mode, "loop");

        let pieces = vec![block_piece(7, Vec2::ZERO)];
        let mut movers = Movers::default();
        apply_saved(&mut movers, &pieces, std::slice::from_ref(&saved));
        let restored = movers.map.get(&7).expect("restored mover");
        assert_eq!(restored.waypoints.len(), 2);
        assert_eq!(restored.mode, PlatformMode::Loop);
        assert_eq!(restored.speed, 2.5);
        assert!(restored.paused);
        assert!(restored.track.is_some());
    }

    #[test]
    fn platforms_for_missing_pieces_are_dropped_on_load() {
        let mut mover = Mover::new(Vec2::ZERO);
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 2.0));
        mover.rebuild();
        let saved = to_saved(99, &mover);
        let mut movers = Movers::default();
        apply_saved(&mut movers, &[], std::slice::from_ref(&saved));
        assert!(movers.map.is_empty());
    }

    #[test]
    fn one_shot_stops_at_the_end() {
        let mut mover = Mover::new(Vec2::ZERO);
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 2.0));
        mover.rebuild();
        mover.mode = PlatformMode::OneShot;
        mover.speed = 10.0;
        let target = mover.step(1.0).unwrap();
        assert!((target.x - 2.0).abs() < 0.1, "{target:?}");
        let again = mover.step(1.0).unwrap();
        assert!((again.x - 2.0).abs() < 0.1, "{again:?}");
    }
}
