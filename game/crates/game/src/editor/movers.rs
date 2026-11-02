//! Moving platforms: per-piece Bezier paths with ride carry.
//!
//! Each mover belongs to one piece id. Its waypoints build a smoothed cubic
//! Bezier [`path::Track`] (same curve family as the character route); the
//! per-frame system advances along arc length so speed is constant, then
//! adds the platform delta to a standing rider so they move with it.

use std::collections::HashMap;

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

/// Move every platform along its track, carrying a standing rider by the
/// same delta (full carry, including vertical when we add height later).
/// Runs before `character_move` so physics sees the carried position.
/// Also stamps the platform velocity underfoot onto `ride_vel` (zero when
/// not riding) so jumps can inherit the ride's inertia.
pub(super) fn move_movers(
    time: Res<Time>,
    mut movers: ResMut<Movers>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.25);
    if dt <= 0.0 {
        return;
    }
    // Snapshot rider pose before any platform moves.
    let rider = editor.character.map(|c| (c.pos, c.feet));
    let mut carry = Vec2::ZERO;
    let carry_feet = 0.0;

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
        // Was the rider standing on this piece before it moved?
        if let Some((pos, feet)) = rider {
            let shape = &shapes()[piece.shape];
            if platforms::is_riding(pos, feet, piece, shape) {
                carry += delta;
                // Platforms are level for now; keep the hook for ramps.
                let _ = carry_feet;
            }
        }
        piece.pos = target;
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
    // Stamp the ride velocity every frame: the platform's world velocity
    // while carried, zero otherwise. `character_move` folds this into `hv`
    // on takeoff so jumps inherit the ride, and back out on landing so
    // repeated hops cannot stack speed.
    if let Some(character) = editor.character.as_mut() {
        character.ride_vel = if carry != Vec2::ZERO {
            carry / dt
        } else {
            Vec2::ZERO
        };
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
