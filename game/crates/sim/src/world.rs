//! Headless physics world: statics, movers, and dynamic props.
//!
//! Character collision does NOT go through the Rapier solver — exactly like
//! the game, players are kinematic state resolved against exact collider
//! geometry. The colliders themselves come from the game's own builder
//! (`party_dungeon::sim_support::colliders`), so art can never desync physics.
//! Rapier integrates dynamic props and carries kinematic movers; manual
//! parry queries resolve characters.

use std::collections::{HashMap, HashSet};

use bevy_math::Vec2;
use party_dungeon::model;
use rapier3d::math::{Pose, Rotation, Vector};
use rapier3d::prelude::*;

/// A solid in world space.
#[derive(Clone)]
pub struct Solid {
    pub rid: String,
    pub shape: SharedShape,
    pub pos: Pose,
    pub mover: Option<RigidBodyHandle>,
    pub dynamic: Option<RigidBodyHandle>,
}

/// Mover runtime state.
pub struct SimMover {
    pub rid: String,
    pub waypoints: Vec<party_dungeon::platforms::PlatformWaypoint>,
    pub mode: party_dungeon::platforms::PlatformMode,
    pub speed: f32,
    pub distance: f32,
    pub dir: f32,
    pub paused: bool,
    pub track: Option<party_dungeon::path::Track>,
    pub body: RigidBodyHandle,
    pub last_pos: Vec2,
    pub elevation: f32,
}

/// Spec translated from a SurrealDB platform path.
#[derive(Clone, Debug)]
pub struct SimMoverSpec {
    pub waypoints: Vec<party_dungeon::platforms::PlatformWaypoint>,
    pub closed: bool,
    pub mode: party_dungeon::platforms::PlatformMode,
    pub speed: f32,
    pub paused: bool,
    pub elevation: f32,
}

pub struct World {
    pub bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub integration: IntegrationParameters,
    pub gravity: Vector,
    pub solids: Vec<Solid>,
    pub movers: HashMap<String, SimMover>,
    pub platform_bodies: HashMap<String, RigidBodyHandle>,
    pub dynamic_bodies: HashSet<RigidBodyHandle>,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd: CCDSolver,
    pipeline: PhysicsPipeline,
}

impl World {
    pub fn new() -> Self {
        let mut integration = IntegrationParameters::default();
        integration.dt = party_dungeon_net::TICK_DT;
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            integration,
            gravity: Vector::new(0.0, -party_dungeon::sim_support::GRAVITY, 0.0),
            solids: Vec::new(),
            movers: HashMap::new(),
            platform_bodies: HashMap::new(),
            dynamic_bodies: HashSet::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            ccd: CCDSolver::new(),
            pipeline: PhysicsPipeline::new(),
        }
    }

    /// Rebuild everything from the authoritative piece + path lists.
    pub fn rebuild(
        &mut self,
        pieces: &[(String, model::Piece)],
        paths: &std::collections::HashMap<String, SimMoverSpec>,
    ) {
        *self = Self::new();
        let shapes = model::shapes();
        let all: Vec<model::Piece> = pieces.iter().map(|(_, p)| p.clone()).collect();
        for (rid, piece) in pieces {
            let Some(shape) = shapes.get(piece.shape) else {
                continue;
            };
            let Some(collider) =
                party_dungeon::sim_support::piece_collider_for_glade(piece, shape, &all, &[], None)
            else {
                continue;
            };
            let is_mover_track = paths.contains_key(rid)
                && !matches!(shape.id, "glade_wall" | "glade_stairs" | "glade_path");
            let iso = piece_isometry(piece);
            if piece.dynamic {
                let body = RigidBodyBuilder::dynamic().pose(iso).build();
                let handle = self.bodies.insert(body);
                let co = ColliderBuilder::new(collider.raw.clone()).build();
                let _ = self.colliders.insert_with_parent(co, handle, &mut self.bodies);
                self.dynamic_bodies.insert(handle);
                self.solids.push(Solid {
                    rid: rid.clone(),
                    shape: collider.raw.clone(),
                    pos: iso,
                    mover: None,
                    dynamic: Some(handle),
                });
            } else if is_mover_track {
                let body = RigidBodyBuilder::kinematic_position_based().pose(iso).build();
                let handle = self.bodies.insert(body);
                let co = ColliderBuilder::new(collider.raw.clone()).build();
                let _ = self.colliders.insert_with_parent(co, handle, &mut self.bodies);
                self.platform_bodies.insert(rid.clone(), handle);
                self.solids.push(Solid {
                    rid: rid.clone(),
                    shape: collider.raw.clone(),
                    pos: iso,
                    mover: Some(handle),
                    dynamic: None,
                });
            } else {
                let body = RigidBodyBuilder::fixed().pose(iso).build();
                let handle = self.bodies.insert(body);
                let co = ColliderBuilder::new(collider.raw.clone()).build();
                let _ = self.colliders.insert_with_parent(co, handle, &mut self.bodies);
                self.solids.push(Solid {
                    rid: rid.clone(),
                    shape: collider.raw.clone(),
                    pos: iso,
                    mover: None,
                    dynamic: None,
                });
            }
        }
        for (rid, spec) in paths {
            let Some(&handle) = self.platform_bodies.get(rid) else {
                continue;
            };
            let track = if spec.waypoints.len() >= 2 {
                Some(party_dungeon::platforms::build_platform_track(
                    &spec.waypoints,
                    spec.closed,
                ))
            } else {
                None
            };
            let start = spec
                .waypoints
                .first()
                .map(|w| w.pos)
                .unwrap_or(Vec2::ZERO);
            self.movers.insert(
                rid.clone(),
                SimMover {
                    rid: rid.clone(),
                    waypoints: spec.waypoints.clone(),
                    mode: spec.mode,
                    speed: spec.speed,
                    distance: 0.0,
                    dir: 1.0,
                    paused: spec.paused,
                    track,
                    body: handle,
                    last_pos: start,
                    elevation: spec.elevation,
                },
            );
        }
        self.step_dynamics();
    }

    /// Advance movers along their tracks. Returns per-body XZ displacement
    /// for rider carry.
    pub fn step_movers(&mut self, dt: f32) -> HashMap<RigidBodyHandle, Vec2> {
        let mut deltas = HashMap::new();
        let rids: Vec<String> = self.movers.keys().cloned().collect();
        for rid in rids {
            let total = self
                .movers
                .get(&rid)
                .and_then(|m| m.track.as_ref().map(|t| t.total()))
                .unwrap_or(0.0);
            let (handle, tx, tz, elev) = {
                let Some(m) = self.movers.get_mut(&rid) else {
                    continue;
                };
                if !m.paused && total > 0.0 && m.speed > 0.0 {
                    (m.distance, m.dir) = party_dungeon::platforms::advance_platform(
                        m.distance, m.dir, total, m.speed, dt, m.mode,
                    );
                }
                let (tx, tz) = m
                    .track
                    .as_ref()
                    .map(|t| t.at(m.distance.clamp(0.0, total.max(0.0))).0)
                    .map(|p| (p.x, p.y))
                    .unwrap_or((m.last_pos.x, m.last_pos.y));
                m.last_pos = Vec2::new(tx, tz);
                (m.body, tx, tz, m.elevation)
            };
            let prev = self
                .bodies
                .get(handle)
                .map(|b| (b.translation().x, b.translation().z));
            if let Some(body) = self.bodies.get_mut(handle) {
                body.set_next_kinematic_translation(Vector::new(tx, elev, tz));
            }
            if let Some((px, pz)) = prev {
                deltas.insert(handle, Vec2::new(tx - px, tz - pz));
            }
            for solid in &mut self.solids {
                if solid.mover == Some(handle) {
                    solid.pos.translation.x = tx;
                    solid.pos.translation.z = tz;
                }
            }
        }
        deltas
    }

    /// Step dynamic props (resting, shoved). Players and movers are driven
    /// kinematically by the sim, not the solver.
    pub fn step_dynamics(&mut self) {
        self.pipeline.step(
            self.gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd,
            &(),
            &(),
        );
        // Keep the manual-collision copies in lockstep with moved dynamics.
        for solid in &mut self.solids {
            if let Some(h) = solid.dynamic {
                if let Some(body) = self.bodies.get(h) {
                    solid.pos = *body.position();
                }
            }
        }
    }

    /// All solids for manual character queries (statics, movers, dynamics).
    pub fn each_solid(&self) -> impl Iterator<Item = &Solid> {
        self.solids.iter()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

/// World isometry for a piece: ground position + elevation, full orientation
/// (the game clamps glade pieces to yaw when it stores them).
pub fn piece_isometry(piece: &model::Piece) -> Pose {
    let q = model::piece_quat(piece);
    Pose::from_parts(
        Vector::new(piece.pos.x, piece.elevation, piece.pos.y),
        Rotation::from_xyzw(q.x, q.y, q.z, q.w),
    )
}

/// Character capsule matching the game's `character_collider`:
/// `capsule_y(PILL_CYLINDER / 2, PILL_RADIUS)`.
pub fn character_capsule() -> SharedShape {
    let hh = party_dungeon::sim_support::PILL_CYLINDER * 0.5;
    SharedShape::capsule(
        Vector::new(0.0, -hh, 0.0),
        Vector::new(0.0, hh, 0.0),
        party_dungeon::sim_support::PILL_RADIUS,
    )
}

/// Capsule centre for a character standing with `feet` at the given height.
pub fn capsule_center(pos: Vec2, feet: f32) -> Vector {
    Vector::new(pos.x, feet + party_dungeon::sim_support::PILL_HALF, pos.y)
}
