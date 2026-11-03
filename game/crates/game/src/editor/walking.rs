//! walking systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

/// Vertical velocity for a physics step. A jump request only launches when the
/// character is standing on something; in the air it is ignored. The jump
/// itself is a vertical impulse: horizontal momentum (`hv`) is untouched, so
/// a sprint carries into the leap.
pub(super) fn jump_velocity(vy: f32, grounded: bool, jump: bool) -> f32 {
    if jump && grounded { JUMP_SPEED } else { vy }
}

/// Ground acceleration toward the wish velocity, in m/s^2. High enough to
/// feel responsive, low enough to leave a short physical slide on
/// landing/turns instead of teleporting the velocity.
pub(super) const GROUND_ACCEL: f32 = 30.0;
/// Airborne steering acceleration, in m/s^2. Deliberately small: inertia
/// carries the jump and the stick can only strafe a little.
pub(super) const AIR_ACCEL: f32 = 8.0;

/// Move `current` toward `target` by at most `GROUND_ACCEL * dt`.
/// Pure helper so the ground feel stays unit-testable without a window.
pub(super) fn ground_step(current: Vec2, target: Vec2, dt: f32) -> Vec2 {
    if dt <= 0.0 {
        return current;
    }
    let to_target = target - current;
    let max_step = GROUND_ACCEL * dt;
    let dist = to_target.length();
    if dist <= max_step || dist == 0.0 {
        target
    } else {
        current + to_target * (max_step / dist)
    }
}

/// One air-control step. With no steering the velocity is untouched
/// (inertia). Steering only adds speed along the wish direction, up to the
/// wish speed, then the total is capped at the faster of the entry speed and
/// the wish speed — so you can nudge a jump sideways but never instantly
/// reverse or gain speed in mid-air.
pub(super) fn air_step(current: Vec2, wish_dir: Vec2, wish_speed: f32, dt: f32) -> Vec2 {
    if dt <= 0.0 || wish_speed <= 0.0 || wish_dir == Vec2::ZERO {
        return current;
    }
    let dir = wish_dir.normalize_or_zero();
    if dir == Vec2::ZERO {
        return current;
    }
    let along = current.dot(dir);
    let add = (wish_speed - along).clamp(0.0, AIR_ACCEL * dt);
    if add <= 0.0 {
        return current;
    }
    let stepped = current + dir * add;
    let cap = current.length().max(wish_speed);
    if cap <= 0.0 {
        return stepped;
    }
    let len = stepped.length();
    if len > cap {
        stepped * (cap / len)
    } else {
        stepped
    }
}

/// Slide friction, in m/s^2. Far below `GROUND_ACCEL`, so a landing slide
/// keeps most of its entry momentum and coasts for a second or more instead
/// of scrubbing to the wish speed in a few frames.
pub(super) const SLIDE_FRICTION: f32 = 2.4;
/// How hard steering can bend a slide's direction, in m/s^2. Steering may
/// turn the slide but never adds speed: the desired velocity is always the
/// (already decayed) slide speed pointed along the wish direction.
pub(super) const SLIDE_STEER: f32 = 8.0;
/// Slowest horizontal landing speed that can enter a slide, in m/s. A
/// standing hop lands below this and simply stands up again.
pub(super) const SLIDE_MIN_ENTRY_SPEED: f32 = 1.5;
/// Speed at which a slide ends and normal ground control resumes, in m/s.
pub(super) const SLIDE_END_SPEED: f32 = 0.85;

/// One slide step: decay the speed by `SLIDE_FRICTION`, then bend the
/// remaining velocity toward `wish_dir` by at most `SLIDE_STEER * dt`
/// without ever increasing its magnitude. With no steering the slide is
/// pure momentum: direction untouched, speed bleeding off slowly.
/// Pure helper so the slide feel stays unit-testable without a window.
pub(super) fn slide_step(current: Vec2, wish_dir: Vec2, dt: f32) -> Vec2 {
    if dt <= 0.0 {
        return current;
    }
    let speed = current.length();
    if speed <= 0.0 {
        return Vec2::ZERO;
    }
    let decayed_speed = (speed - SLIDE_FRICTION * dt).max(0.0);
    if decayed_speed <= 0.0 {
        return Vec2::ZERO;
    }
    let mut next = current / speed * decayed_speed;
    let dir = wish_dir.normalize_or_zero();
    if dir != Vec2::ZERO {
        let desired = dir * decayed_speed;
        let to_desired = desired - next;
        let max_step = SLIDE_STEER * dt;
        let dist = to_desired.length();
        if dist <= max_step || dist == 0.0 {
            next = desired;
        } else {
            next += to_desired * (max_step / dist);
            // Bending is a change of direction, not a second brake: restore
            // the friction-only speed so a steered slide costs no more
            // momentum than a straight one.
            next = next.normalize_or_zero() * decayed_speed;
        }
    }
    next
}

/// Whether the capsule is sliding after a frame. A slide can only be
/// *entered* on the landing frame itself (airborne last frame, grounded
/// now) with Left Ctrl held and enough horizontal momentum; pressing Ctrl
/// while already running on the ground never starts one. An ongoing slide
/// survives while Ctrl stays held, the capsule stays grounded, and it is
/// still moving faster than `SLIDE_END_SPEED`.
pub(super) fn next_sliding(
    was_sliding: bool,
    landed: bool,
    slide_held: bool,
    grounded: bool,
    speed: f32,
) -> bool {
    if !slide_held || !grounded {
        return false;
    }
    (was_sliding && speed >= SLIDE_END_SPEED) || (landed && speed >= SLIDE_MIN_ENTRY_SPEED)
}

/// A slope pushes the character upward while it climbs, so a small positive
/// vertical speed still counts as ground. A real jump rises much faster.
pub(super) const MAX_GROUNDED_RISE: f32 = JUMP_SPEED * 0.5;

/// How far below the feet a surface may sit and still count as the ground
/// while walking down a slope. A descending character leaves the surface
/// faster than gravity can pull it back, so contact-only support flickers and
/// jump presses are silently dropped on most frames.
pub(super) const STEP_DOWN: f32 = 0.35;
/// Highest terrain rise the grounded walk mounts in one move, in metres.
/// The surface sampled at the destination may sit at most this far above
/// the feet: tile seams and slightly raised ramp bases are walked over,
/// while anything taller is a wall (the wall rays above this height say
/// so first). A ramp's slope never presents a single rise anywhere near
/// this — it is climbed continuously, a few centimetres per frame.
pub(super) const STEP_UP: f32 = 0.4;
/// Surfaces steeper than this are walls, not ground.
pub(super) const MIN_GROUND_NORMAL: f32 = 0.4;

/// Ground support from a contact gap, as `(grounded, drop_to_contact)`. A
/// negative gap means the capsule is touching or slightly penetrating; it is
/// never moved up, the physics resolves that. Positive gaps up to `STEP_DOWN`
/// are followed when already walking, so a descent does not free-fall.
pub(super) fn ground_support(gap: f32, can_snap: bool, uphill: bool) -> (bool, Option<f32>) {
    if gap <= 0.0 {
        (gap >= -0.2, None)
    } else if gap <= 0.13 {
        (true, if uphill { None } else { Some(gap) })
    } else if can_snap && !uphill && gap <= STEP_DOWN {
        (true, Some(gap))
    } else {
        (false, None)
    }
}

/// How much to lower the capsule for a downward ray from the bottom sphere's
/// centre. A capsule touches a slope along the surface normal, not at its
/// lowest point, so a plain vertical chord overstates the gap on an ascent and
/// snapping by it would fight the climb.
pub(super) fn contact_gap(ray_distance: f32, normal_y: f32) -> f32 {
    ray_distance - PILL_RADIUS / normal_y.max(0.01)
}

/// The grounded controller never asks physics to push the capsule out of
/// terrain. While grounded it only ever moves by the frame's wish
/// displacement, and the feet follow the surface sampled straight down at
/// the destination — so a ramp is climbed metre by metre at walking pace,
/// with no depenetration pop at its sharp toe or ridge and no step-up
/// repositioning. Move-and-slide (and its depenetration) is kept for
/// airborne motion only, where the capsule is not resting on terrain.
///
/// Sample the walkable surface under `at`, returning the foot height the
/// capsule stands at there plus the surface normal's vertical component.
/// The ray starts just above the tallest acceptable step, so a wall top
/// beside the path is never sampled as ground. The standing height is
/// lifted by `R * (1/ny - 1)`: a capsule resting on a slope touches along
/// the normal, so its lowest point sits that far above the surface point
/// directly beneath its centre.
fn ground_at(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    at: Vec2,
    feet: f32,
) -> Option<(f32, f32)> {
    let origin_y = feet + STEP_UP + 0.06;
    let hit = spatial_query.cast_ray(
        Vec3::new(at.x, origin_y, at.y).real(),
        Dir3::NEG_Y,
        STEP_UP + 1.6,
        true,
        filter,
    )?;
    if hit.normal.y <= MIN_GROUND_NORMAL {
        return None;
    }
    let raw = origin_y - hit.distance;
    let stand = raw + PILL_RADIUS * (1.0 / hit.normal.y - 1.0);
    Some((stand, hit.normal.y))
}

/// A wall blocking movement from `pos` along `dir`, if one reaches the
/// capsule within `reach`. Three rays fan across the capsule's width at
/// step height: anything taller than a step that stops a ray head-on is a
/// wall, while a step-able lip (below the rays) and a ramp slope (whose
/// normal leans up, never head-on) are not. The hit doubles as the push
/// record for dynamic bodies in the way.
fn wall_hit(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    pos: Vec2,
    feet: f32,
    dir: Vec2,
    reach: f32,
    velocity: Vec3,
) -> Option<PushHit> {
    let len = dir.length();
    if len <= 0.0 {
        return None;
    }
    let dir = dir / len;
    let Ok(dir3) = Dir3::new(Vec3::new(dir.x, 0.0, dir.y)) else {
        return None;
    };
    let perp = Vec2::new(-dir.y, dir.x);
    for offset in [-PILL_RADIUS * 0.75, 0.0, PILL_RADIUS * 0.75] {
        let base = pos + perp * offset;
        let origin = Vec3::new(base.x, feet + STEP_UP + 0.05, base.y);
        let Some(hit) = spatial_query.cast_ray(origin.real(), dir3, reach, true, filter) else {
            continue;
        };
        let facing = Vec2::new(hit.normal.x, hit.normal.z).dot(dir);
        if hit.normal.y < MIN_GROUND_NORMAL && facing < -0.05 {
            return Some(PushHit {
                collider: hit.entity,
                point: origin + Vec3::new(dir.x, 0.0, dir.y) * hit.distance,
                normal: hit.normal,
                character_velocity: velocity,
            });
        }
    }
    None
}

/// Whether there is headroom for the capsule standing at `at` on a
/// surface whose foot height is `stand`. Only a ceiling matters here:
/// side geometry is already handled by the wall rays, and the capsule
/// body legitimately grazes the terrain it stands on, so a full overlap
/// test would refuse every slope approach.
fn headroom_at(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    at: Vec2,
    stand: f32,
) -> bool {
    let origin = Vec3::new(at.x, stand + 0.05, at.y).real();
    spatial_query
        .cast_ray(origin, Dir3::Y, PILL_HALF * 2.0 - 0.1, true, filter)
        .is_none()
}

/// Character motion state carried through one frame of [`step_motion`].
#[derive(Clone, Copy)]
struct Motion {
    pos: Vec2,
    feet: f32,
    vy: f32,
    hv: Vec2,
    grounded: bool,
}

/// One grounded substep: move by at most `hv * dt`, following the sampled
/// terrain. Returns the horizontal distance actually travelled. Nothing
/// here can move the capsule further than the wish displacement, nor lift
/// it faster than the terrain under the destination rises.
#[allow(clippy::too_many_arguments)]
fn grounded_substep(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    motion: &mut Motion,
    target: Vec2,
    dt: f32,
    sliding: bool,
    pushes: &mut Vec<PushHit>,
) -> f32 {
    motion.hv = if sliding {
        slide_step(motion.hv, target.normalize_or_zero(), dt)
    } else {
        ground_step(motion.hv, target, dt)
    };
    motion.vy = 0.0;
    let disp = motion.hv * dt;
    if disp.length_squared() <= 1e-12 {
        // No wish travel: settle onto whatever is underfoot (a platform
        // may have carried the capsule) without drifting.
        let ground = ground_at(spatial_query, filter, motion.pos, motion.feet)
            .filter(|(stand, _)| (stand - motion.feet).abs() <= STEP_DOWN.max(STEP_UP));
        match ground {
            Some((stand, _)) => motion.feet = stand,
            None => motion.grounded = false,
        }
        return 0.0;
    }
    // Full displacement first, then the axis-separated slides, so a
    // diagonal push into a wall still travels along it.
    let attempts = [disp, Vec2::new(disp.x, 0.0), Vec2::new(0.0, disp.y)];
    let mut pushed = false;
    for step in attempts {
        if step.length_squared() <= 1e-12 {
            continue;
        }
        let cand = motion.pos + step;
        let velocity = Vec3::new(motion.hv.x, 0.0, motion.hv.y);
        if let Some(hit) = wall_hit(
            spatial_query,
            filter,
            motion.pos,
            motion.feet,
            step,
            step.length() + PILL_RADIUS + 0.02,
            velocity,
        ) {
            if !pushed {
                pushes.push(hit);
                pushed = true;
            }
            continue;
        }
        let Some((stand, normal_y)) = ground_at(spatial_query, filter, cand, motion.feet) else {
            // Nothing walkable below within reach: walked off the world.
            motion.pos = cand;
            motion.grounded = false;
            return step.length();
        };
        let rise = stand - motion.feet;
        if rise > STEP_UP {
            continue;
        }
        if rise < -STEP_DOWN {
            // A ledge drops away — unless walkable ground continues just
            // past the capsule's centre, which bridges seams and cracks
            // (ramp ridge onto the next block) instead of dipping in.
            let ahead = cand + step.normalize_or_zero() * PILL_RADIUS;
            let bridges = ground_at(spatial_query, filter, ahead, motion.feet)
                .is_some_and(|(ahead_stand, _)| (ahead_stand - motion.feet).abs() <= STEP_UP);
            if bridges {
                motion.pos = cand;
                return step.length();
            }
            motion.pos = cand;
            motion.grounded = false;
            return step.length();
        }
        if !headroom_at(spatial_query, filter, cand, stand) {
            continue;
        }
        motion.pos = cand;
        // On a slope the feet may only gain or lose height as fast as the
        // terrain rises over the distance travelled, so the flat-to-slope
        // normal change at a ramp's toe cannot snap the capsule up in one
        // frame. Flat ground (a genuine step) still seats instantly.
        motion.feet = if normal_y < 0.99 {
            let grade = ((1.0 - normal_y * normal_y).max(0.0)).sqrt() / normal_y;
            let max_delta = step.length() * grade + 0.005;
            motion.feet + (stand - motion.feet).clamp(-max_delta, max_delta)
        } else {
            stand
        };
        if step != disp {
            // A reduced slide: keep only the velocity that was used.
            motion.hv = step / dt;
        }
        return step.length();
    }
    // Blocked on every attempt: the wall scrubs the wish velocity.
    motion.hv = Vec2::ZERO;
    0.0
}

/// Advance the character one frame. Grounded motion is the sampled-terrain
/// walk of [`grounded_substep`]; airborne motion keeps move-and-slide so
/// jumps, falls and wall slides stay physical. Landing is resolved by the
/// ground probe after the airborne steps, never by depenetration.
#[allow(clippy::too_many_arguments)]
fn step_motion(
    move_and_slide: &MoveAndSlide,
    collider: &Collider,
    rotation: Quat,
    filter: &SpatialQueryFilter,
    motion: &mut Motion,
    target: Vec2,
    wish_dir: Vec2,
    wish_speed: f32,
    was_grounded: bool,
    taking_off: bool,
    idle: bool,
    sliding: bool,
    frame: f32,
) -> (f32, Vec<PushHit>) {
    let spatial_query = &move_and_slide.spatial_query;
    let steps = ((frame / (1.0 / 60.0)).ceil() as usize).clamp(1, 10);
    let step_dt = frame / steps as f32;
    let mut travelled = 0.0;
    let mut pushes: Vec<PushHit> = Vec::new();
    if taking_off {
        motion.grounded = false;
    }
    if idle {
        motion.hv = Vec2::ZERO;
        motion.vy = 0.0;
        let ground = ground_at(spatial_query, filter, motion.pos, motion.feet)
            .filter(|(stand, _)| (stand - motion.feet).abs() <= STEP_DOWN.max(STEP_UP));
        match ground {
            Some((stand, _)) => {
                motion.feet = stand;
                motion.grounded = true;
            }
            None => {
                // A mover shoved the capsule off its perch (or the platform
                // slid out from under it). Idle frames still have to notice
                // that and start falling instead of hovering until input.
                motion.grounded = false;
                motion.vy = -GRAVITY * frame;
            }
        }
        return (0.0, pushes);
    }
    let mut center = Vec3::new(motion.pos.x, motion.feet + PILL_HALF, motion.pos.y);
    for _ in 0..steps {
        if motion.grounded {
            travelled += grounded_substep(
                spatial_query,
                filter,
                motion,
                target,
                step_dt,
                sliding,
                &mut pushes,
            );
            center = Vec3::new(motion.pos.x, motion.feet + PILL_HALF, motion.pos.y);
        } else {
            motion.vy -= GRAVITY * step_dt;
            if !taking_off {
                if was_grounded {
                    motion.hv = ground_step(motion.hv, target, step_dt);
                } else {
                    motion.hv = air_step(motion.hv, wish_dir, wish_speed, step_dt);
                }
            }
            let velocity = Vec3::new(motion.hv.x, motion.vy, motion.hv.y);
            let mut hits: Vec<PushHit> = Vec::new();
            let output = move_and_slide.move_and_slide(
                collider,
                center.real(),
                rotation,
                velocity,
                std::time::Duration::from_secs_f32(step_dt),
                &MoveAndSlideConfig::default(),
                filter,
                |hit| {
                    hits.push(PushHit {
                        collider: hit.entity,
                        point: hit.point.f32(),
                        normal: **hit.normal,
                        character_velocity: *hit.velocity,
                    });
                    MoveAndSlideHitResponse::Accept
                },
            );
            let next = output.position.f32();
            travelled += Vec2::new(next.x - center.x, next.z - center.z).length();
            center = next;
            motion.vy = output.projected_velocity.y;
            motion.hv = Vec2::new(output.projected_velocity.x, output.projected_velocity.z);
            pushes.append(&mut hits);
        }
    }
    motion.pos = Vec2::new(center.x, center.z);
    motion.feet = center.y - PILL_HALF;
    if !motion.grounded {
        // Probe down from the capsule centre so a descent can follow the
        // slope for a short step instead of free-falling between frames.
        let surface = spatial_query
            .cast_ray(
                Vec3::new(center.x, motion.feet + PILL_RADIUS, center.z).real(),
                Dir3::NEG_Y,
                PILL_RADIUS / MIN_GROUND_NORMAL + STEP_DOWN,
                true,
                filter,
            )
            .filter(|hit| hit.normal.y > MIN_GROUND_NORMAL)
            .map(|hit| {
                let uphill = Vec2::new(hit.normal.x, hit.normal.z).dot(wish_dir) < -0.05;
                (contact_gap(hit.distance, hit.normal.y), uphill)
            });
        let (grounded, drop) = if motion.vy > MAX_GROUNDED_RISE {
            (false, None)
        } else {
            surface.map_or((false, None), |(gap, uphill)| {
                ground_support(gap, was_grounded, uphill)
            })
        };
        if grounded {
            motion.vy = 0.0;
            motion.grounded = true;
            if let Some(drop) = drop {
                motion.feet -= drop;
            }
        }
    } else {
        motion.vy = 0.0;
    }
    (travelled, pushes)
}

/// How much of the character's inward speed a shoved body picks up. Below 1.0
/// so a pushed crate trails the player instead of sticking to the capsule and
/// jittering.
pub(super) const PUSH_FACTOR: f32 = 0.85;
/// Minimum inward speed before a shove counts, in m/s. Filters contact jitter
/// when the capsule rests against a prop.
pub(super) const PUSH_MIN_SPEED: f32 = 0.15;

/// One recorded character-vs-prop contact. Collected inside the move-and-slide
/// callback (which can only return a response, not touch other queries) and
/// applied afterwards as an impulse.
#[derive(Clone, Copy, Debug)]
struct PushHit {
    collider: Entity,
    point: Vec3,
    normal: Vec3,
    character_velocity: Vec3,
}

/// Velocity-matching impulse that brings `body_vel` up to `PUSH_FACTOR` of the
/// character's inward speed, without overshooting. Returns `None` when the
/// character is separating or barely touching.
pub(super) fn push_impulse(
    normal: Vec3,
    character_velocity: Vec3,
    body_velocity: Vec3,
    body_mass: f32,
) -> Option<Vec3> {
    if body_mass <= 0.0 || !body_mass.is_finite() {
        return None;
    }
    // `normal` is the obstacle surface normal; the push direction is into the
    // body. The double-negative form below matches Avian's kinematic example
    // and pushes into the body whichever way the normal is oriented.
    let touch_dir = -normal.normalize_or_zero();
    if touch_dir == Vec3::ZERO {
        return None;
    }
    let relative = character_velocity - body_velocity;
    let inward = touch_dir.dot(relative);
    if inward < PUSH_MIN_SPEED {
        return None;
    }
    Some(touch_dir * (inward * PUSH_FACTOR * body_mass))
}

/// Resolve a hit collider to the rigid body that owns it. Piece colliders live
/// on the body entity itself; child colliders (if any) point at it through
/// `ColliderOf`.
fn push_target(collider: Entity, collider_of: &Query<&ColliderOf>) -> Entity {
    collider_of
        .get(collider)
        .map(|of| of.body)
        .unwrap_or(collider)
}

/// Apply recorded shoves to dynamic bodies. Static geometry and the player
/// are skipped; each hit only accelerates its body up to the character's
/// inward speed so light props do not launch.
fn apply_pushes(
    hits: &[PushHit],
    collider_of: &Query<&ColliderOf>,
    rigid: &Query<&RigidBody>,
    masses: &Query<&ComputedMass>,
    bodies: &mut Query<Forces>,
) {
    for hit in hits {
        let target = push_target(hit.collider, collider_of);
        let Ok(body) = rigid.get(target) else {
            continue;
        };
        if !body.is_dynamic() {
            continue;
        }
        let Ok(mass) = masses.get(target) else {
            continue;
        };
        let Ok(mut forces) = bodies.get_mut(target) else {
            continue;
        };
        let body_vel = forces.linear_velocity();
        let Some(impulse) =
            push_impulse(hit.normal, hit.character_velocity, body_vel, mass.value())
        else {
            continue;
        };
        forces.apply_linear_impulse_at_point(impulse, hit.point.real());
    }
}

/// Advance the character along the drawn route at a constant walking pace.
/// The camera's orbit centre follows in third/top view; in first person the
/// look direction follows the tangent.
#[allow(clippy::too_many_arguments)]
pub(super) fn walk(
    time: Res<Time>,
    input: PlayerInput,
    mut touch: ResMut<TouchState>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    kit: Res<Kit>,
    player: Query<Entity, With<CharacterVisual>>,
    move_and_slide: MoveAndSlide,
    focus: Res<InputFocus>,
    collider_of: Query<&ColliderOf>,
    rigid: Query<&RigidBody>,
    masses: Query<&ComputedMass>,
    mut bodies: Query<Forces>,
    menu: Res<AddMenu>,
) {
    if focus.get().is_some() || menu.open {
        return;
    }
    let Some(mut current) = editor.walk else {
        return;
    };
    let Some(character) = editor.character else {
        editor.walk = None;
        return;
    };
    let was_grounded = character.grounded;
    let Some(entity) = player.iter().next() else {
        return;
    };

    let Some((total, look)) = editor.route.as_ref().map(|route| {
        let total = route.track.total();
        (total, route.track.at((current.distance + 0.7).min(total)).0)
    }) else {
        editor.walk = None;
        return;
    };

    let to_target = look - character.pos;
    if current.distance >= total - 0.12 && to_target.length() < 0.2 {
        prune_plan(&mut editor, current.distance);
        editor.walk = None;
        editor.note = "arrived";
        return;
    }

    // The jump button works while the route keeps steering, so a jump carries
    // forward along the path instead of cancelling it. Read it only after the
    // arrival check: a jump pressed as the route ends is left for the free
    // controller so the button still lands.
    let jump = input.pressed(Action::Jump) || touch.jump;
    touch.jump = false;
    // Left Ctrl held on landing after the jump starts a momentum slide,
    // exactly as in free control; releasing it (or jumping) ends the slide.
    let slide_held = input.pressed(Action::Slide) || touch.slide;

    // Steering, not teleporting: grounded route following uses the same
    // sampled-terrain walk as first-person control, so ramps, gaps and
    // drops are resolved by the ground under the destination, never by
    // depenetration. Horizontal speed is physical: on the ground it chases
    // the route pace, in the air inertia carries and the route can only
    // strafe a little.
    let direction = to_target.normalize_or_zero();
    let frame = time.delta_secs().clamp(0.0, 0.25);
    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);
    let vy = jump_velocity(character.vy, character.grounded, jump);
    let mut hv = character.hv;
    // A jump is a vertical impulse: the takeoff frame keeps entry momentum
    // untouched instead of applying ground steering toward the wish.
    let taking_off = jump && character.grounded;
    if taking_off {
        // Leaving a moving platform carries its motion into the leap.
        hv += character.ride_vel;
    }
    let sliding = character.sliding && slide_held && !taking_off;
    let airborne = !was_grounded && vy <= MAX_GROUNDED_RISE;
    if !airborne && !taking_off {
        // Leaving the ground with a jump keeps the run-up: a standing start
        // still needs a first step so the route does not hop in place.
        if hv.length() < 0.05 && direction != Vec2::ZERO {
            hv = direction * WALK_SPEED.min(1.0);
        }
    }
    let mut motion = Motion {
        pos: character.pos,
        feet: character.feet,
        vy,
        hv,
        grounded: character.grounded,
    };
    let (travelled, pushes) = step_motion(
        &move_and_slide,
        &kit.character_collider,
        rotation,
        &filter,
        &mut motion,
        direction * WALK_SPEED,
        direction,
        WALK_SPEED,
        was_grounded,
        taking_off,
        false,
        sliding,
        frame,
    );
    apply_pushes(&pushes, &collider_of, &rigid, &masses, &mut bodies);
    let moved = motion.pos;
    let yaw = if direction.length_squared() > 0.01 {
        (-direction.x).atan2(-direction.y)
    } else {
        character.yaw
    };

    let nearest = editor
        .route
        .as_ref()
        .map(|route| route.track.nearest_distance(moved))
        .unwrap_or(current.distance);

    if let Some(target) = editor.character.as_mut() {
        target.pos = moved;
        target.feet = motion.feet;
        target.vy = if motion.grounded { 0.0 } else { motion.vy };
        // Keep the post-move horizontal velocity so inertia survives the
        // frame and walls do not leave phantom speed behind.
        target.hv = motion.hv;
        // Touching down on a moving platform converts back to platform-frame
        // velocity, so the takeoff fold cannot stack speed across hops.
        // Runs on the landing transition only, never while settled.
        if motion.grounded && !was_grounded {
            target.hv -= character.ride_vel;
        }
        target.grounded = motion.grounded;
        target.sliding = next_sliding(
            sliding,
            motion.grounded && !was_grounded,
            slide_held,
            motion.grounded,
            target.hv.length(),
        );
        target.yaw = yaw;
    }

    if orbit.mode == ViewMode::First {
        orbit.yaw = yaw;
    } else {
        orbit.target = Vec3::new(moved.x, 0.0, moved.y);
    }

    current.distance = nearest.clamp(current.distance - 0.2, total);

    // Reached waypoints stop being part of the live plan; the drawn line
    // stays behind as a record.
    prune_plan(&mut editor, current.distance);

    if travelled < 0.005 {
        current.stuck += frame;
    } else {
        current.stuck = 0.0;
    }

    if current.stuck > 0.8 {
        editor.walk = None;
        editor.note = "blocked";
    } else if current.distance >= total - 0.12 {
        editor.walk = None;
        editor.note = "arrived";
    } else {
        editor.walk = Some(current);
    }

    // Any manual steering hands control back to the player.
    if touch.stick.length_squared() > 0.02
        || input.pressed(Action::MoveForward)
        || input.pressed(Action::MoveBack)
        || input.pressed(Action::MoveLeft)
        || input.pressed(Action::MoveRight)
    {
        editor.walk = None;
        editor.note = "manual control";
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn character_move(
    time: Res<Time>,
    input: PlayerInput,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    kit: Res<Kit>,
    player: Query<Entity, With<CharacterVisual>>,
    move_and_slide: MoveAndSlide,
    focus: Res<InputFocus>,
    collider_of: Query<&ColliderOf>,
    rigid: Query<&RigidBody>,
    masses: Query<&ComputedMass>,
    mut bodies: Query<Forces>,
    menu: Res<AddMenu>,
) {
    if focus.get().is_some() || menu.open {
        return;
    }
    if editor.walk.is_some() {
        return;
    }
    let Some(character) = editor.character.as_mut() else {
        return;
    };
    let was_grounded = character.grounded;
    let old_pos = character.pos;
    let Some(entity) = player.iter().next() else {
        return;
    };

    // The stick and WASD both steer, in every view. Third person is the
    // default on a phone, and the pad is the only movement control there.
    // Keyboard, D-pad and the left stick merge in `move_vector`; touch
    // overrides while the thumb is down.
    let mut wish = input.move_vector();
    if touch.enabled && touch.stick.length_squared() > 0.01 {
        wish = touch.stick;
    }
    // An analogue pad moves slower near the middle; a small dead zone stops a
    // resting thumb from drifting.
    let mut strength = wish.length().clamp(0.0, 1.0);
    if strength < 0.12 {
        strength = 0.0;
    }
    let wish = if strength > 0.0 {
        wish.normalize_or_zero()
    } else {
        Vec2::ZERO
    };

    let forward = Vec2::new(-orbit.yaw.sin(), -orbit.yaw.cos());
    let right = Vec2::new(orbit.yaw.cos(), -orbit.yaw.sin());
    let direction = (forward * wish.y + right * wish.x).normalize_or_zero();

    let sprint = input.pressed(Action::Sprint) || touch.sprint;
    let wish_speed = (if sprint { 5.5 } else { 2.8 }) * strength;
    let target = direction * wish_speed;

    // Split a long frame into short substeps. Under software rendering a frame
    // can be 0.3 s; a single step that long lets gravity cancel a jump and
    // tunnels through colliders. `step_motion` does the splitting.
    let frame = time.delta_secs().clamp(0.0, 0.25);

    // `pressed`, not `just_pressed`: a frame can span the whole key tap on a
    // slow frame, and a held space should still jump on landing.
    let jump = input.pressed(Action::Jump) || touch.jump;
    touch.jump = false;
    // A jump is a vertical impulse: it sets `vy` and leaves `hv` alone so
    // run-up momentum carries into the leap.
    let vy = jump_velocity(character.vy, character.grounded, jump);
    let mut hv = character.hv;
    // The takeoff frame preserves entry momentum exactly: no ground steering
    // toward the wish, so releasing the stick mid-jump does not scrub speed.
    let taking_off = jump && character.grounded;
    if taking_off {
        // Leaving a moving platform carries its motion into the leap.
        hv += character.ride_vel;
    }
    // Slide: Left Ctrl is only read as a landing input. Holding it while
    // airborne queues a slide that starts on touchdown (see `next_sliding`);
    // holding it while already grounded never starts one, and a jump
    // takeoff cancels any slide in progress. The touch slide button toggles
    // the same held state.
    let slide_held = input.pressed(Action::Slide) || touch.slide;
    let sliding = character.sliding && slide_held && !taking_off;

    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);

    // Standing still on a slope should not slide: without friction the
    // projected gravity would walk the character back down the ramp. A
    // sliding capsule is never idle, even with no steering held: its
    // momentum is the whole point.
    let idle = wish == Vec2::ZERO && character.grounded && vy <= 0.01 && !sliding;

    let grounded_entry = character.grounded;
    let mut motion = Motion {
        pos: character.pos,
        feet: character.feet,
        vy,
        hv,
        grounded: grounded_entry,
    };
    let (_travelled, pushes) = step_motion(
        &move_and_slide,
        &kit.character_collider,
        rotation,
        &filter,
        &mut motion,
        target,
        direction,
        wish_speed,
        was_grounded,
        taking_off,
        idle,
        sliding,
        frame,
    );
    apply_pushes(&pushes, &collider_of, &rigid, &masses, &mut bodies);

    character.pos = motion.pos;
    character.feet = motion.feet;
    if orbit.mode == ViewMode::First {
        character.yaw = orbit.yaw;
    } else if direction != Vec2::ZERO {
        // Face the way the pad points. Third person follows the pill so it
        // stays in frame; top down keeps the camera put, so the pill visibly
        // moves across the map instead of the grid appearing to stand still.
        character.yaw = (-direction.x).atan2(-direction.y);
        if orbit.mode == ViewMode::Third {
            orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
        }
    } else if orbit.mode == ViewMode::Third {
        // No steering this frame, but physics may still have moved us
        // (inertia, slides, gravity). Platform carry itself is applied in
        // `move_movers`, which already shifts the orbit centre by the same
        // delta; this only covers movement from this system's own step.
        // Follow by delta so a manual pan offset survives standing still.
        let delta = character.pos - old_pos;
        if delta != Vec2::ZERO {
            orbit.target.x += delta.x;
            orbit.target.z += delta.y;
        }
    }

    character.grounded = motion.grounded;
    character.vy = if motion.grounded { 0.0 } else { motion.vy };
    // Touching down on a moving platform converts back to platform-frame
    // velocity, so the takeoff fold cannot stack speed across hops.
    // Runs on the landing transition only, never while settled.
    if motion.grounded && !was_grounded {
        motion.hv -= character.ride_vel;
    }
    // On landing the stored momentum decays through `ground_step` over the
    // next frames, giving a short physical slide instead of a hard stop.
    // With Left Ctrl held through the landing that decay is replaced by a
    // real slide: `next_sliding` keeps it alive on momentum alone.
    character.hv = motion.hv;
    character.sliding = next_sliding(
        sliding,
        motion.grounded && !was_grounded,
        slide_held,
        motion.grounded,
        character.hv.length(),
    );
}
