//! The authoritative tick: inputs in, snapshots out.
//!
//! Players are kinematic state integrated by hand — exactly like the game,
//! which never puts the character body in the dynamics solver either. The
//! shared [`walking`] helpers (`ground_step`,
//! `air_step`, `slide_step`, `jump_velocity`) and tuning constants keep the
//! sim's motion math identical to the client's; the 60Hz snapshots correct
//! whatever small divergence remains. Collision tests run parry directly
//! against the game's own collider shapes.

use std::collections::{HashMap, HashSet};

use bevy_math::Vec2;
use party_dungeon::model;
use party_dungeon::sim_support::{
    air_step, ground_step, jump_velocity, slide_step, trigger_at,
    MAX_GROUNDED_RISE, MIN_GROUND_NORMAL, PUSH_FACTOR, SLIDE_END_SPEED, SLIDE_MIN_ENTRY_SPEED,
    STEP_DOWN, STEP_UP,
};
use party_dungeon::sim_support::{GRAVITY, PILL_HALF};
use party_dungeon_net::{Input, PlayerState, Snapshot, WorldPiece, INTERACT_RANGE};
use rapier3d::math::{Pose, Vector};
use rapier3d::prelude::RigidBodyHandle;

use crate::world::World;

/// Walk/sprint pace mirrors the free controller in `editor/walking.rs`.
const WALK_PACE: f32 = 2.8;
const SPRINT_PACE: f32 = 5.5;
/// Damage ticks while standing in a hazard (1s). Mirrors
/// `DAMAGE_COOLDOWN_SECS`, expressed in sim ticks.
const HURT_COOLDOWN_TICKS: u64 = 60;
/// Fell out of the world below this height.
const KILL_PLANE: f32 = -100.0;
/// Penetration tolerated before a contact counts as blocking, in metres.
#[allow(dead_code)]
const CONTACT_SKIN: f32 = 0.02;

pub struct SimPlayer {
    pub sub: String,
    pub name: String,
    pub pos: Vec2,
    pub feet: f32,
    pub vy: f32,
    pub hv: Vec2,
    pub grounded: bool,
    pub yaw: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub alive: bool,
    pub online: bool,
    pub sliding: bool,
    pub last_input: Input,
    pub last_input_tick: u64,
    pub last_hurt_tick: u64,
    pub dead_at_tick: Option<u64>,
    pub ground_platform: Option<RigidBodyHandle>,
    pub ride_vel: Vec2,
}

pub enum SimEvent {
    Kill { target: String, by: String },
    Respawn { sub: String },
    InteractToggled { piece_rid: String, paused: bool },
}

pub struct Sim {
    pub world: World,
    pub players: HashMap<String, SimPlayer>,
    pub pieces: Vec<(String, model::Piece)>,
    /// Mover tracks (platform paths) retained from the last [`Sim::rebuild`],
    /// so an in-place edit can re-derive the Rapier world with
    /// [`Sim::rebuild_current`] without losing the paths.
    pub mover_specs: HashMap<String, crate::world::SimMoverSpec>,
    pub tick: u64,
    pub dungeon_version: u64,
    pub spawn: Vec2,
    pub spawn_feet: f32,
    pub interactables: HashSet<String>,
    pub dirty: bool,
    next_id: u32,
    rid_to_id: HashMap<String, u32>,
}

impl Sim {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            players: HashMap::new(),
            pieces: Vec::new(),
            mover_specs: HashMap::new(),
            tick: 0,
            dungeon_version: 1,
            spawn: Vec2::ZERO,
            spawn_feet: 0.0,
            interactables: HashSet::new(),
            dirty: false,
            next_id: 1,
            rid_to_id: HashMap::new(),
        }
    }

    fn model_id(&mut self, rid: &str) -> u32 {
        if let Some(id) = self.rid_to_id.get(rid) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.rid_to_id.insert(rid.to_string(), id);
        id
    }

    /// Translate server rows into game pieces (same shapes the client sees).
    pub fn convert_pieces(
        &mut self,
        rows: &[(String, party_dungeon_server::Piece)],
    ) -> Vec<(String, model::Piece)> {
        let mut out = Vec::with_capacity(rows.len());
        for (rid, row) in rows {
            let Some(shape) = model::shape_index(&row.shape) else {
                continue;
            };
            let id = self.model_id(rid);
            out.push((
                rid.clone(),
                model::Piece {
                    id,
                    shape,
                    pos: Vec2::new(row.x, row.z),
                    rot: row.rot_deg.to_radians(),
                    pitch: row.pitch_deg.to_radians(),
                    roll: row.roll_deg.to_radians(),
                    elevation: row.elevation,
                    dynamic: row.dynamic,
                    trigger: model::Trigger { damage: row.damage },
                    trigger_size: bevy_math::Vec3::new(
                        row.trigger_w,
                        row.trigger_h,
                        row.trigger_l,
                    ),
                    trigger_shape_override: model::TriggerShape::Box,
                    trigger_alpha: row.trigger_alpha,
                    trigger_color: model::TRIGGER_DEFAULT_COLOR,
                    trigger_visible: true,
                    interact: model::InteractAction::parse(&row.interact)
                        .unwrap_or(model::InteractAction::None),
                },
            ));
        }
        out
    }

    /// Translate server paths into mover specs.
    pub fn convert_paths(
        rows: &[(String, party_dungeon_server::PlatformPath)],
    ) -> HashMap<String, crate::world::SimMoverSpec> {
        let mut out = HashMap::new();
        for (rid, path) in rows {
            let Some(mode) = party_dungeon::platforms::PlatformMode::parse(&path.mode) else {
                continue;
            };
            let waypoints: Vec<party_dungeon::platforms::PlatformWaypoint> = path
                .waypoints
                .iter()
                .map(|w| party_dungeon::platforms::PlatformWaypoint::new(Vec2::new(w.x, w.z)))
                .collect();
            if waypoints.len() < 2 {
                continue;
            }
            out.insert(
                rid.clone(),
                crate::world::SimMoverSpec {
                    waypoints,
                    closed: mode == party_dungeon::platforms::PlatformMode::Loop,
                    mode,
                    speed: path.speed,
                    paused: path.paused,
                    elevation: 0.0,
                },
            );
        }
        out
    }

    /// Rebuild from server rows (boot + hot reload).
    pub fn rebuild(
        &mut self,
        pieces: &[(String, model::Piece)],
        paths: &HashMap<String, crate::world::SimMoverSpec>,
    ) {
        let mut specs = paths.clone();
        for (rid, spec) in specs.iter_mut() {
            if let Some((_, piece)) = pieces.iter().find(|(r, _)| r == rid) {
                spec.elevation = piece.elevation;
            }
        }
        for (_, piece) in pieces.iter() {
            if model::shapes().get(piece.shape).map(|s| s.id) == Some("spawn") {
                self.spawn = piece.pos;
                self.spawn_feet = piece.elevation.max(0.0);
                break;
            }
        }
        self.interactables.clear();
        for (rid, piece) in pieces.iter() {
            if matches!(piece.interact, model::InteractAction::TogglePause) {
                self.interactables.insert(rid.clone());
            }
        }
        self.world.rebuild(pieces, &specs);
        self.pieces = pieces.to_vec();
        // Retain the mover tracks so in-place edits can rebuild the physics
        // world without a round trip to the store (`rebuild_current`).
        self.mover_specs = specs;
        self.dungeon_version += 1;
        self.dirty = false;
    }

    /// Rebuild the physics world from the current in-memory pieces and mover
    /// specs. Used after an accepted wire edit (place/update/delete) so the
    /// Rapier world, spawn point, and interactable set all follow the new
    /// geometry and `dungeon_version` bumps exactly once per edited batch.
    ///
    /// Mover tracks are kept because [`Sim::mover_specs`] carries them, and a
    /// runtime pause toggle (not persisted per edit) survives the rebuild.
    pub fn rebuild_current(&mut self) {
        let mut specs = self.mover_specs.clone();
        // Pause toggles are runtime state, not a persisted edit; keep them.
        for (rid, mover) in &self.world.movers {
            if let Some(spec) = specs.get_mut(rid) {
                spec.paused = mover.paused;
            }
        }
        // A deleted piece takes its track with it.
        specs.retain(|rid, _| self.pieces.iter().any(|(r, _)| r == rid));
        let pieces = self.pieces.clone();
        self.rebuild(&pieces, &specs);
    }

    /// Translate one wire piece (an authored edit) into the game's piece model.
    ///
    /// Mirrors [`Sim::convert_pieces`] exactly — same catalogue lookup, same
    /// rid-keyed stable model id — so a piece delivered by an edit is
    /// indistinguishable from one loaded out of the store. `None` means the
    /// wire names a shape this build does not know; callers reject the edit
    /// *before* persisting it.
    pub fn piece_from_wire(&mut self, wire: &WorldPiece) -> Option<model::Piece> {
        let shape = model::shape_index(&wire.shape)?;
        let id = self.model_id(&wire.rid);
        Some(model::Piece {
            id,
            shape,
            pos: Vec2::new(wire.x, wire.z),
            rot: wire.rot_deg.to_radians(),
            pitch: wire.pitch_deg.to_radians(),
            roll: wire.roll_deg.to_radians(),
            elevation: wire.elevation,
            dynamic: wire.dynamic,
            trigger: model::Trigger {
                damage: wire.damage,
            },
            trigger_size: bevy_math::Vec3::new(wire.trigger_w, wire.trigger_h, wire.trigger_l),
            trigger_shape_override: model::TriggerShape::Box,
            trigger_alpha: wire.trigger_alpha,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: true,
            interact: model::InteractAction::parse(&wire.interact)
                .unwrap_or(model::InteractAction::None),
        })
    }

    /// Upsert one piece from the wire (an accepted Place/Update). Returns
    /// `false` when the shape is unknown, so nothing changed.
    ///
    /// The Rapier world is not touched here: an edit batch is applied and then
    /// finalized once with [`Sim::rebuild_current`].
    pub fn apply_wire_piece(&mut self, wire: &WorldPiece) -> bool {
        let Some(piece) = self.piece_from_wire(wire) else {
            return false;
        };
        if let Some((_, existing)) = self.pieces.iter_mut().find(|(rid, _)| rid == &wire.rid) {
            *existing = piece;
        } else {
            self.pieces.push((wire.rid.clone(), piece));
        }
        true
    }

    /// Remove one piece from the in-memory world (an accepted Delete). Also
    /// drops its mover track. Returns whether the piece was present.
    ///
    /// Like [`Sim::apply_wire_piece`], the final Rapier rebuild is deferred to
    /// [`Sim::rebuild_current`] so a batch bumps `dungeon_version` once.
    pub fn delete_wire_piece(&mut self, rid: &str) -> bool {
        let before = self.pieces.len();
        self.pieces.retain(|(r, _)| r != rid);
        let removed = self.pieces.len() != before;
        self.mover_specs.remove(rid);
        self.world.movers.remove(rid);
        self.interactables.remove(rid);
        removed
    }

    /// Join (or resume) a player. Returns true on first join.
    pub fn join(&mut self, sub: &str, name: &str, hp: f32, max_hp: f32) -> bool {
        let first = !self.players.contains_key(sub);
        let max_hp = max_hp.clamp(1.0, 10_000.0);
        let hp = hp.clamp(0.0, max_hp);
        let now = self.tick;
        let player = self.players.entry(sub.to_string()).or_insert_with(|| SimPlayer {
            sub: sub.to_string(),
            name: name.to_string(),
            pos: self.spawn,
            feet: self.spawn_feet,
            vy: 0.0,
            hv: Vec2::ZERO,
            grounded: true,
            yaw: 0.0,
            hp,
            max_hp,
            alive: hp > 0.0,
            online: true,
            sliding: false,
            last_input: Input::idle(0),
            // The input-timeout check compares against `self.tick`, which is
            // large on a long-running server; starting at 0 would mark a
            // joining player stale on the very next step if their first input
            // has not arrived yet.
            last_input_tick: now,
            last_hurt_tick: 0,
            dead_at_tick: None,
            ground_platform: None,
            ride_vel: Vec2::ZERO,
        });
        player.online = true;
        player.name = name.to_string();
        // A (re)join counts as "heard from now" for the input-timeout check,
        // and the input ordering clock restarts too: a reconnecting client
        // begins a new session at tick 0 (or its last server tick), so keeping
        // the old high `last_input.tick` would reject every input until the
        // new session's tick caught up — the player would sit "online" but be
        // ignored (and time out to invisible) for minutes.
        player.last_input = Input::idle(0);
        player.last_input_tick = now;
        if !player.alive {
            player.pos = self.spawn;
            player.feet = self.spawn_feet;
            player.hp = player.max_hp.max(1.0);
            player.alive = true;
            player.dead_at_tick = None;
            player.vy = 0.0;
            player.hv = Vec2::ZERO;
            player.grounded = true;
        }
        first
    }

    pub fn leave(&mut self, sub: &str) {
        if let Some(p) = self.players.get_mut(sub) {
            p.online = false;
        }
    }

    /// One fixed step. Returns per-tick events (kills, respawns, toggles).
    pub fn step(&mut self, inputs: &HashMap<String, Input>) -> Vec<SimEvent> {
        self.tick += 1;
        let dt = party_dungeon_net::TICK_DT;
        let mut events = Vec::new();

        for (sub, input) in inputs {
            if let Some(p) = self.players.get_mut(sub) {
                if party_dungeon_net::tick_is_newer(input.tick, p.last_input.tick)
                    || p.last_input_tick == 0
                {
                    p.last_input = input.sanitize();
                    p.last_input_tick = self.tick;
                    // Do not strand a player permanently offline: once input
                    // flows again they are back in snapshots (and therefore in
                    // their own reconciliation) without a reconnect.
                    p.online = true;
                }
            }
        }
        let stale: Vec<String> = self
            .players
            .iter()
            .filter(|(_, p)| {
                p.online
                    && self
                        .tick
                        .saturating_sub(p.last_input_tick.max(1))
                        > party_dungeon_net::INPUT_TIMEOUT_TICKS as u64
            })
            .map(|(s, _)| s.clone())
            .collect();
        for sub in stale {
            self.leave(&sub);
        }

        // Advance movers; collect per-body XZ displacement for riders.
        let mut deltas: HashMap<RigidBodyHandle, Vec2> = HashMap::new();
        let rids: Vec<String> = self.world.movers.keys().cloned().collect();
        for rid in rids {
            let total = self
                .world
                .movers
                .get(&rid)
                .and_then(|m| m.track.as_ref().map(|t| t.total()))
                .unwrap_or(0.0);
            let (handle, tx, tz, elev) = {
                let Some(m) = self.world.movers.get_mut(&rid) else {
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
            deltas.insert(handle, Vec2::new(
                tx - self.world.bodies.get(handle).map(|b| b.translation().x).unwrap_or(tx),
                tz - self.world.bodies.get(handle).map(|b| b.translation().z).unwrap_or(tz),
            ));
            if let Some(body) = self.world.bodies.get_mut(handle) {
                body.set_next_kinematic_translation(Vector::new(tx, elev, tz));
            }
            for solid in &mut self.world.solids {
                if solid.mover == Some(handle) {
                    solid.pos.translation.x = tx;
                    solid.pos.translation.z = tz;
                }
            }
        }

        // Integrate players.
        let subs: Vec<String> = self.players.keys().cloned().collect();
        for sub in subs {
            let (online, alive, input) = match self.players.get(&sub) {
                Some(p) => (p.online, p.alive, p.last_input),
                None => continue,
            };
            if !online {
                continue;
            }
            if !alive {
                let dead_at = self.players.get(&sub).and_then(|p| p.dead_at_tick);
                if let Some(dead_at) = dead_at {
                    if self.tick - dead_at >= party_dungeon_net::RESPAWN_DELAY_TICKS as u64 {
                        if let Some(p) = self.players.get_mut(&sub) {
                            p.pos = self.spawn;
                            p.feet = self.spawn_feet;
                            p.hp = p.max_hp.max(1.0);
                            p.alive = true;
                            p.dead_at_tick = None;
                            p.vy = 0.0;
                            p.hv = Vec2::ZERO;
                            p.grounded = true;
                        }
                        events.push(SimEvent::Respawn { sub: sub.clone() });
                    }
                }
                continue;
            }
            self.step_player(&sub, &input, dt, &deltas, &mut events);
        }

        self.world.step_dynamics();
        events
    }

    #[allow(clippy::too_many_arguments)]
    fn step_player(
        &mut self,
        sub: &str,
        input: &Input,
        dt: f32,
        deltas: &HashMap<RigidBodyHandle, Vec2>,
        events: &mut Vec<SimEvent>,
    ) {
        // Wish direction mirrors free control: stick (mx right, mz forward)
        // rotated by the camera yaw; analogue magnitude sets pace.
        let wish = Vec2::new(input.mx, input.mz);
        let mut strength = wish.length().clamp(0.0, 1.0);
        if strength < 0.12 {
            strength = 0.0;
        }
        let wish = if strength > 0.0 {
            wish.normalize_or_zero()
        } else {
            Vec2::ZERO
        };
        let forward = Vec2::new(-input.yaw.sin(), -input.yaw.cos());
        let right = Vec2::new(input.yaw.cos(), -input.yaw.sin());
        let direction = (forward * wish.y + right * wish.x).normalize_or_zero();
        let wish_speed = (if input.sprint { SPRINT_PACE } else { WALK_PACE }) * strength;
        let target = direction * wish_speed;

        let was_grounded = self.players[sub].grounded;
        let speed = self.players[sub].hv.length();
        let was_sliding = self.players[sub].sliding;
        // Landing slides mirror the game's rules: enter on a fast landing
        // with slide held, leave when slow or airborne.
        let mut sliding = (was_sliding && was_grounded && speed >= SLIDE_END_SPEED)
            || (input.slide && was_grounded && speed > SLIDE_MIN_ENTRY_SPEED);
        if sliding && !was_grounded {
            sliding = false;
        }

        let taking_off = input.jump && was_grounded;
        let mut ride_vel = Vec2::ZERO;
        {
            let p = self.players.get_mut(sub).unwrap();
            p.sliding = sliding;
            if taking_off {
                if let Some(handle) = p.ground_platform {
                    if let Some(d) = deltas.get(&handle) {
                        ride_vel = *d / dt.max(1e-4);
                    }
                }
                p.vy = jump_velocity(p.vy, true, true);
                p.hv += ride_vel;
                p.grounded = false;
                p.sliding = false;
                sliding = false;
            }
            if p.grounded {
                if sliding {
                    p.hv = slide_step(p.hv, target.normalize_or_zero(), dt);
                } else {
                    p.hv = ground_step(p.hv, target, dt);
                }
                p.vy = 0.0;
            } else {
                p.hv = air_step(p.hv, direction, wish_speed, dt);
                p.vy -= GRAVITY * dt;
            }
            p.yaw = input.yaw;
            p.ride_vel = if taking_off { ride_vel } else { Vec2::ZERO };
        }

        // Ride movers: apply the deck displacement before walking.
        if was_grounded && !taking_off {
            let platform = self.players[sub].ground_platform;
            if let Some(handle) = platform {
                if let Some(d) = deltas.get(&handle) {
                    if let Some(p) = self.players.get_mut(sub) {
                        p.pos += *d;
                    }
                }
            }
        }

        self.integrate(sub, dt, taking_off);

        // Interact: toggle nearby pause-motion pieces on the edge.
        if input.interact {
            let (pos, feet) = {
                let p = &self.players[sub];
                (p.pos, p.feet)
            };
            let mut best: Option<(String, f32)> = None;
            for rid in self.interactables.clone() {
                let Some((_, piece)) = self.pieces.iter().find(|(r, _)| r == &rid) else {
                    continue;
                };
                let d = (piece.pos - pos).length();
                if d < INTERACT_RANGE
                    && (feet - piece.elevation).abs() < 2.0
                    && best.as_ref().is_none_or(|(_, bd)| d < *bd)
                {
                    best = Some((rid.clone(), d));
                }
            }
            if let Some((rid, _)) = best {
                if let Some(m) = self.world.movers.get_mut(&rid) {
                    m.paused = !m.paused;
                    // The shared world changed: bump so the streaming layer
                    // re-sends just this mover's cell with the new `paused`
                    // flag (`WorldChunk`), plus a `DungeonChanged` notice.
                    self.dungeon_version += 1;
                    events.push(SimEvent::InteractToggled { piece_rid: rid, paused: m.paused });
                }
            }
        }

        // Hazards use the game's own overlap test.
        let pieces: Vec<model::Piece> = self.pieces.iter().map(|(_, p)| p.clone()).collect();
        let hazard = {
            let p = &self.players[sub];
            trigger_at(p.pos, p.feet, &pieces)
        };
        if let Some(trigger) = hazard {
            let should_hit = self
                .players
                .get(sub)
                .map(|p| self.tick - p.last_hurt_tick >= HURT_COOLDOWN_TICKS)
                .unwrap_or(false);
            if should_hit {
                let p = self.players.get_mut(sub).unwrap();
                p.last_hurt_tick = self.tick;
                p.hp = (p.hp - trigger.damage).max(0.0);
                if p.hp <= 0.0 && p.alive {
                    p.alive = false;
                    p.dead_at_tick = Some(self.tick);
                    events.push(SimEvent::Kill {
                        target: sub.to_string(),
                        by: "hazard".to_string(),
                    });
                }
            }
        }
        let fell = self
            .players
            .get(sub)
            .map(|p| p.feet < KILL_PLANE && p.alive)
            .unwrap_or(false);
        if fell {
            let p = self.players.get_mut(sub).unwrap();
            p.hp = 0.0;
            p.alive = false;
            p.dead_at_tick = Some(self.tick);
            events.push(SimEvent::Kill {
                target: sub.to_string(),
                by: "void".to_string(),
            });
        }

        // Shove dynamic props the capsule overlaps.
        self.push_dynamics(sub);
    }

    /// Move one player with collision. Compact version of the game's
    /// sampled-terrain walk; shared accel helpers keep the feel identical.
    fn integrate(&mut self, sub: &str, dt: f32, taking_off: bool) {
        let (mut pos, mut feet, mut vy, mut hv, mut grounded) = {
            let p = &self.players[sub];
            (
                p.pos,
                p.feet,
                p.vy,
                p.hv,
                p.grounded && !taking_off,
            )
        };
        let mut ground_platform: Option<RigidBodyHandle> = None;

        if grounded {
            let disp = hv * dt;
            let attempts = [disp, Vec2::new(disp.x, 0.0), Vec2::new(0.0, disp.y)];
            let mut blocked = true;
            for (i, step) in attempts.iter().enumerate() {
                if step.length_squared() < 1e-12 {
                    continue;
                }
                if self.hits_solid(pos, feet, *step) {
                    if let Some((npos, nfeet)) = self.step_up(pos, feet, *step) {
                        pos = npos;
                        feet = nfeet;
                        blocked = false;
                        if i > 0 {
                            hv = *step / dt.max(1e-4);
                        }
                        break;
                    }
                    continue;
                }
                pos += *step;
                blocked = false;
                if i > 0 {
                    hv = *step / dt.max(1e-4);
                }
                break;
            }
            if blocked {
                hv = Vec2::ZERO;
            }
            match self.ground_height(pos, feet) {
                Some((stand, plat))
                    if (stand - feet).abs() <= STEP_DOWN + 0.02 =>
                {
                    feet = stand;
                    ground_platform = plat;
                }
                _ => {
                    grounded = false;
                }
            }
        } else {
            let (npos, nhv) = self.sweep_airborne(pos, feet, hv, vy, dt);
            pos = npos;
            hv = nhv;
            feet += vy * dt;
            if vy <= MAX_GROUNDED_RISE {
                if let Some((stand, plat)) = self.ground_height(pos, feet) {
                    let gap = feet - stand;
                    if gap >= 0.0 && gap <= STEP_DOWN + 0.1 {
                        feet = stand;
                        vy = 0.0;
                        grounded = true;
                        ground_platform = plat;
                    }
                }
            }
        }
        if let Some(p) = self.players.get_mut(sub) {
            p.pos = pos;
            p.feet = feet;
            p.vy = vy;
            p.hv = hv;
            p.grounded = grounded;
            p.ground_platform = ground_platform;
        }
    }

    /// Highest walkable top under `pos`, plus supporting platform if any.
    /// Mirrors the game's `ground_at` ray probe: a downward ray from just
    /// above the tallest step, looking for the first upward-facing hit.
    fn ground_height(
        &self,
        pos: Vec2,
        feet: f32,
    ) -> Option<(f32, Option<RigidBodyHandle>)> {
        use rapier3d::parry::query::Ray;
        let origin_y = feet + STEP_UP + 0.06;
        let ray = Ray::new(
            Vector::new(pos.x, origin_y, pos.y).into(),
            Vector::new(0.0, -1.0, 0.0),
        );
        let max_toi = STEP_UP + 1.6;
        let mut best: Option<(f32, Option<RigidBodyHandle>)> = None;
        for solid in self.world.each_solid() {
            let Some(hit) = solid.shape.cast_ray_and_get_normal(&solid.pos, &ray, max_toi, true)
            else {
                continue;
            };
            if hit.normal.y <= MIN_GROUND_NORMAL {
                continue;
            }
            let raw = origin_y - hit.time_of_impact;
            let top = if hit.normal.y > 0.01 {
                raw + party_dungeon::sim_support::PILL_RADIUS * (1.0 / hit.normal.y - 1.0)
            } else {
                raw
            };
            if top > feet + STEP_UP + 0.02 {
                continue;
            }
            match best {
                Some((stand, _)) if stand >= top => {}
                _ => {
                    let plat = solid.mover;
                    best = Some((top, plat));
                }
            }
        }
        best
    }

    /// Capsule sweep test: does moving along `step` hit anything solid?
    /// Floors (near-vertical normals) are ignored: they don't block walks.
    fn hits_solid(&self, pos: Vec2, feet: f32, step: Vec2) -> bool {
        use rapier3d::parry::query::{cast_shapes, ShapeCastOptions};
        if step.length_squared() < 1e-12 {
            return false;
        }
        let dist = step.length();
        let dir = step / dist;
        let capsule = crate::world::character_capsule();
        let from = Pose::from_translation(Vector::new(
            pos.x,
            feet + PILL_HALF,
            pos.y,
        ));
        let vel = Vector::new(dir.x, 0.0, dir.y) * dist;
        let zero = Vector::new(0.0, 0.0, 0.0);
        for solid in self.world.each_solid() {
            let Ok(Some(hit)) = cast_shapes(
                &from,
                vel,
                &*capsule,
                &solid.pos,
                zero,
                &*solid.shape,
                ShapeCastOptions::with_max_time_of_impact(1.0),
            ) else {
                continue;
            };
            // Debug near wall (removed after verification)
            // Ignore floors/ceilings: only walls (horizontal normals) block.
            if hit.normal1.y.abs() > 0.5 {
                continue;
            }
            if hit.time_of_impact < 1.0 - 1e-4 {
                return true;
            }
        }
        false
    }

    /// Step-up attempt: raise, move, require headroom and a landing.
    fn step_up(&self, pos: Vec2, feet: f32, step: Vec2) -> Option<(Vec2, f32)> {
        let raised = feet + STEP_UP;
        let cand = pos + step;
        // Headroom: a capsule at the raised height must not overlap anything.
        let probe = Pose::from_translation(Vector::new(
            cand.x,
            raised + PILL_HALF,
            cand.y,
        ));
        let capsule = crate::world::character_capsule();
        for solid in self.world.each_solid() {
            if let Ok(Some(_)) =
                rapier3d::parry::query::contact(&probe, &*capsule, &solid.pos, &*solid.shape, 0.01)
            {
                return None;
            }
        }
        match self.ground_height(cand, raised) {
            Some((stand, _)) if (stand - raised).abs() <= STEP_DOWN + 0.05 => {
                Some((cand, stand))
            }
            _ => None,
        }
    }

    /// Airborne horizontal sweep with a single slide along the contact.
    fn sweep_airborne(&self, pos: Vec2, feet: f32, hv: Vec2, vy: f32, dt: f32) -> (Vec2, Vec2) {
        use rapier3d::parry::query::{cast_shapes, ShapeCastOptions};
        let dist = hv.length() * dt;
        if dist < 1e-6 {
            return (pos, hv);
        }
        let dir = hv / hv.length();
        let capsule = crate::world::character_capsule();
        let from = Pose::from_translation(Vector::new(
            pos.x,
            feet + PILL_HALF,
            pos.y,
        ));
        let vel = Vector::new(dir.x, 0.0, dir.y) * dist;
        let zero = Vector::new(0.0, 0.0, 0.0);
        let mut best: Option<(f32, Vector)> = None;
        for solid in self.world.each_solid() {
            let Ok(Some(hit)) = cast_shapes(
                &from,
                vel,
                &*capsule,
                &solid.pos,
                zero,
                &*solid.shape,
                ShapeCastOptions::with_max_time_of_impact(1.0),
            ) else {
                continue;
            };
            if hit.time_of_impact < 1.0 {
                match best {
                    Some((d, _)) if d <= hit.time_of_impact => {}
                    _ => best = Some((hit.time_of_impact, hit.normal1)),
                }
            }
        }
        let Some((toi, n)) = best else {
            return (pos + hv * dt, hv);
        };
        let travel = (toi - 0.01).max(0.0);
        let mut pos = pos + Vec2::new(dir.x, dir.y) * travel;
        let mut hv = hv;
        let into = hv.dot(Vec2::new(n.x, n.z));
        if into < 0.0 {
            hv -= Vec2::new(n.x, n.z) * into;
        }
        let rest = hv * dt - Vec2::new(dir.x, dir.y) * travel;
        if rest.length_squared() > 1e-10 && !self.hits_solid(pos, feet, rest) {
            pos += rest;
        }
        let _ = vy;
        (pos, hv)
    }

    /// Shove dynamic props overlapping the player capsule.
    fn push_dynamics(&mut self, sub: &str) {
        let (pos, feet, hv) = match self.players.get(sub) {
            Some(p) if p.hv.length_squared() > 0.01 => (p.pos, p.feet, p.hv),
            _ => return,
        };
        let capsule = crate::world::character_capsule();
        let probe = Pose::from_translation(Vector::new(
            pos.x,
            feet + PILL_HALF,
            pos.y,
        ));
        let mut shoves: Vec<rapier3d::prelude::RigidBodyHandle> = Vec::new();
        for solid in self.world.each_solid() {
            if solid.dynamic.is_none() {
                continue;
            }
            let Ok(Some(_)) = rapier3d::parry::query::contact(
                &probe,
                &*capsule,
                &solid.pos,
                &*solid.shape,
                0.05,
            ) else {
                continue;
            };
            if let Some(h) = solid.dynamic {
                shoves.push(h);
            }
        }
        for handle in shoves {
            if let Some(body) = self.world.bodies.get_mut(handle) {
                let push =
                    hv.normalize_or_zero() * PUSH_FACTOR.max(0.2);
                body.apply_impulse(Vector::new(push.x, 0.0, push.y), true);
            }
        }
    }

    /// All online players as wire states, sorted by `sub` so the bytes are
    /// deterministic and buckets can be built in one pass. The net layer calls
    /// this **once per tick** and feeds the result to
    /// [`crate::stream::PlayerBuckets`] for per-receiver filtering.
    pub fn player_states(&self) -> Vec<PlayerState> {
        let mut players: Vec<PlayerState> = self
            .players
            .values()
            .filter(|p| p.online)
            .map(|p| PlayerState {
                sub: p.sub.clone(),
                name: p.name.clone(),
                x: p.pos.x,
                y: p.feet,
                z: p.pos.y,
                vx: p.hv.x,
                vy: p.vy,
                vz: p.hv.y,
                yaw: p.yaw,
                hp: p.hp,
                max_hp: p.max_hp,
                alive: p.alive,
                online: p.online,
            })
            .collect();
        players.sort_by(|a, b| a.sub.cmp(&b.sub));
        players
    }

    /// Build the authoritative snapshot for this tick (every online player).
    ///
    /// This is the unfiltered view — kept for tests and admin/debug callers.
    /// The authoritative net path uses [`Sim::snapshot_for`] so each peer only
    /// sees the players around it.
    pub fn snapshot(&self, server_ms: i64) -> Snapshot {
        Snapshot {
            tick: self.tick as u32,
            server_ms,
            players: self.player_states(),
            dungeon_version: self.dungeon_version,
        }
    }

    /// Per-receiver filtered snapshot: only the players within `radius` of the
    /// receiver (everyone when `radius` is `None`), always including the
    /// receiver itself, sorted by `sub`.
    ///
    /// `visible` is the receiver's previously visible set; it is updated in
    /// place so a player who leaves the entry radius lingers out to
    /// `PLAYER_UNLOAD_MARGIN` (see [`crate::stream::PlayerBuckets::filter`]).
    /// The per-tick [`PlayerBuckets`](crate::stream::PlayerBuckets) are built
    /// here; the net layer builds them once and reuses them for every peer.
    pub fn snapshot_for(
        &self,
        server_ms: i64,
        viewer: &str,
        radius: Option<f32>,
        visible: &mut HashSet<String>,
    ) -> Snapshot {
        let buckets = crate::stream::PlayerBuckets::build(self.player_states());
        let origin = self
            .players
            .get(viewer)
            .map(|p| [p.pos.x, p.pos.y])
            .unwrap_or([self.spawn.x, self.spawn.y]);
        buckets.snapshot(
            self.tick as u32,
            server_ms,
            self.dungeon_version,
            viewer,
            origin,
            radius,
            visible,
        )
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}
