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

/// A slope pushes the character upward while it climbs, so a small positive
/// vertical speed still counts as ground. A real jump rises much faster.
pub(super) const MAX_GROUNDED_RISE: f32 = JUMP_SPEED * 0.5;

/// How far below the feet a surface may sit and still count as the ground
/// while walking down a slope. A descending character leaves the surface
/// faster than gravity can pull it back, so contact-only support flickers and
/// jump presses are silently dropped on most frames.
pub(super) const STEP_DOWN: f32 = 0.35;
/// Highest ledge the character steps onto without jumping, in metres. Small
/// lips (tile seams, slightly raised ramp bases) are walked over instead of
/// blocking forever; full block faces stay walls. The probe lands a little
/// past the capsule's edge, where a ramp has already risen a touch, so this
/// clears lips a few centimetres shorter than itself.
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

/// Top height of a ledge the character may step onto, or `None` when the way
/// ahead is a real wall. Probes down just past the capsule's leading edge:
/// a walkable surface within `STEP_UP` above the feet counts as a step, while
/// taller faces and missing headroom for the capsule do not. Returns the
/// surface height and its normal's vertical component, which sizes the
/// overlap check in [`step_destination`].
pub(super) fn step_up_top(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    center: Vec3,
    feet: f32,
    dir: Vec2,
) -> Option<(f32, f32)> {
    if dir == Vec2::ZERO {
        return None;
    }
    let ahead = Vec2::new(center.x, center.z) + dir * (PILL_RADIUS + 0.15);
    let plain = Vec3::new(ahead.x, feet + STEP_UP + PILL_RADIUS, ahead.y);
    let hit = spatial_query.cast_ray(
        plain.real(),
        Dir3::NEG_Y,
        STEP_UP + PILL_RADIUS + 0.05,
        true,
        filter,
    )?;
    if hit.normal.y <= MIN_GROUND_NORMAL {
        return None;
    }
    let top = plain.y - hit.distance;
    if top <= feet + 0.02 || top > feet + STEP_UP {
        return None;
    }
    // The step is only usable when the capsule fits above it.
    let headroom = Vec3::new(ahead.x, top + 0.1, ahead.y).real();
    if spatial_query
        .cast_ray(headroom, Dir3::Y, PILL_HALF * 2.0 - 0.2, true, filter)
        .is_some()
    {
        return None;
    }
    Some((top, hit.normal.y))
}

/// Capsule centre for mounting the `top` ledge found by [`step_up_top`], or
/// `None` when the fat capsule would not fit there.
///
/// The probe ray is thin: at a crowded border — a lipped ramp backed by a
/// block, the corner where two pieces meet — the ray can thread past
/// neighbouring geometry that the 0.32 m capsule radius still overlaps.
/// Teleporting there anyway embeds the capsule in solid rock, and the next
/// move-and-slide depenetrates it: the capsule pops up into the air or
/// flickers between poses for as long as it keeps pushing into the wall.
/// Refusing the step keeps the character blocked against the obstacle, the
/// same as against any wall too tall to climb.
pub(super) fn step_destination(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    collider: &Collider,
    rotation: Quat,
    center: Vec3,
    top: f32,
    normal_y: f32,
    dir: Vec2,
) -> Option<Vec3> {
    let dest = Vec3::new(
        center.x + dir.x * (PILL_RADIUS + 0.15),
        top + PILL_HALF,
        center.z + dir.y * (PILL_RADIUS + 0.15),
    );
    // An overlap test reports bodies the capsule would embed in. Lift the
    // check pose so the ledge surface itself — which the capsule correctly
    // touches — does not read as an overlap. The lift grows on steeper
    // slopes: the uphill surface rises into the capsule's disc, by
    // `R * (1 - ny) / ny` at the disc edge for a plane with vertical
    // component `ny`. It stays small next to real walls, so a destination
    // inside neighbouring geometry still reads as an overlap.
    let lift = (PILL_RADIUS * (1.0 - normal_y) / normal_y.max(MIN_GROUND_NORMAL) + 0.04).min(0.15);
    let probe = Vec3::new(dest.x, dest.y + lift, dest.z);
    let overlaps = !spatial_query
        .shape_intersections(collider, probe.real(), rotation, filter)
        .is_empty();
    if overlaps { None } else { Some(dest) }
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
    keys: Res<ButtonInput<KeyCode>>,
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
    let jump = keys.pressed(KeyCode::Space) || touch.jump;
    touch.jump = false;

    // Steering, not teleporting: the same capsule and move-and-slide as
    // first-person control, so ramps, gaps and gravity are resolved by physics.
    // Horizontal speed is physical: on the ground it chases the route pace,
    // in the air inertia carries and the route can only strafe a little.
    let direction = to_target.normalize_or_zero();
    let frame = time.delta_secs().clamp(0.0, 0.25);
    let steps = ((frame / (1.0 / 60.0)).ceil() as usize).clamp(1, 10);
    let step_dt = frame / steps as f32;
    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);
    let mut center = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);
    let mut vy = jump_velocity(character.vy, character.grounded, jump);
    let mut hv = character.hv;
    // A jump is a vertical impulse: the takeoff frame keeps entry momentum
    // untouched instead of applying ground steering toward the wish.
    let taking_off = jump && character.grounded;
    if taking_off {
        // Leaving a moving platform carries its motion into the leap.
        hv += character.ride_vel;
    }
    let airborne = !was_grounded && vy <= MAX_GROUNDED_RISE;
    if !airborne && !taking_off {
        // Leaving the ground with a jump keeps the run-up: a standing start
        // still needs a first step so the route does not hop in place.
        if hv.length() < 0.05 && direction != Vec2::ZERO {
            hv = direction * WALK_SPEED.min(1.0);
        }
    }
    let mut travelled = 0.0;

    for _ in 0..steps {
        vy -= GRAVITY * step_dt;
        if !taking_off {
            if was_grounded {
                hv = ground_step(hv, direction * WALK_SPEED, step_dt);
            } else {
                hv = air_step(hv, direction, WALK_SPEED, step_dt);
            }
        }
        let velocity = Vec3::new(hv.x, vy, hv.y);
        let mut hits: Vec<PushHit> = Vec::new();
        let output = move_and_slide.move_and_slide(
            &kit.character_collider,
            center.real(),
            rotation,
            velocity,
            std::time::Duration::from_secs_f32(step_dt),
            &MoveAndSlideConfig::default(),
            &filter,
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
        vy = output.projected_velocity.y;
        // Walls scrub the into-wall component so route inertia slides along
        // them instead of storing up speed for after the collision.
        hv = Vec2::new(output.projected_velocity.x, output.projected_velocity.z);
        apply_pushes(&hits, &collider_of, &rigid, &masses, &mut bodies);
    }

    let mut feet = center.y - PILL_HALF;
    // Step-up assist, same as free control: a low lip at the route border
    // scrubs route momentum to nothing, so mount ledges within STEP_UP when
    // the frame barely moved. Runs before `moved` is read so the position
    // advances onto the ledge the same frame.
    let intended = WALK_SPEED * frame;
    if was_grounded && travelled < intended * 0.25 {
        let dir = if hv.length() > 0.2 {
            hv.normalize_or_zero()
        } else {
            direction
        };
        if let Some((top, normal_y)) =
            step_up_top(&move_and_slide.spatial_query, &filter, center, feet, dir)
            && let Some(dest) = step_destination(
                &move_and_slide.spatial_query,
                &filter,
                &kit.character_collider,
                rotation,
                center,
                top,
                normal_y,
                dir,
            )
        {
            center = dest;
            feet = top;
            hv = dir * WALK_SPEED;
        }
    }
    let moved = Vec2::new(center.x, center.z);
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

    // Probe down from the capsule centre so a descent can follow the slope for
    // a short step instead of free-falling between contact frames.
    feet = center.y - PILL_HALF;
    let surface = move_and_slide
        .spatial_query
        .cast_ray(
            Vec3::new(center.x, feet + PILL_RADIUS, center.z).real(),
            Dir3::NEG_Y,
            PILL_RADIUS / MIN_GROUND_NORMAL + STEP_DOWN,
            true,
            &filter,
        )
        .filter(|hit| hit.normal.y > MIN_GROUND_NORMAL)
        .map(|hit| {
            let uphill = Vec2::new(hit.normal.x, hit.normal.z).dot(direction) < -0.05;
            (contact_gap(hit.distance, hit.normal.y), uphill)
        });
    let (grounded, drop) = if vy > MAX_GROUNDED_RISE {
        (false, None)
    } else {
        surface.map_or((false, None), |(gap, uphill)| {
            ground_support(gap, was_grounded, uphill)
        })
    };
    if grounded {
        vy = 0.0;
    }
    if let Some(drop) = drop {
        feet -= drop;
    }

    if let Some(target) = editor.character.as_mut() {
        target.pos = moved;
        target.feet = feet;
        target.vy = if grounded { 0.0 } else { vy };
        // Keep the post-slide horizontal velocity so inertia survives the
        // frame and walls do not leave phantom speed behind.
        target.hv = hv;
        // Touching down on a moving platform converts back to platform-frame
        // velocity, so the takeoff fold cannot stack speed across hops.
        // Runs on the landing transition only, never while settled.
        if grounded && !was_grounded {
            target.hv -= character.ride_vel;
        }
        target.grounded = grounded;
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
        || keys.any_pressed([
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ])
    {
        editor.walk = None;
        editor.note = "manual control";
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn character_move(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
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
    let mut wish = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        wish.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        wish.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        wish.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        wish.x -= 1.0;
    }
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

    let sprint = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let wish_speed = (if sprint { 5.5 } else { 2.8 }) * strength;
    let target = direction * wish_speed;

    // Split a long frame into short substeps. Under software rendering a frame
    // can be 0.3 s; a single step that long lets gravity cancel a jump and
    // tunnels through colliders.
    let frame = time.delta_secs().clamp(0.0, 0.25);
    let steps = ((frame / (1.0 / 60.0)).ceil() as usize).clamp(1, 10);
    let step_dt = frame / steps as f32;

    // `pressed`, not `just_pressed`: a frame can span the whole key tap on a
    // slow frame, and a held space should still jump on landing.
    let jump = keys.pressed(KeyCode::Space) || touch.jump;
    touch.jump = false;
    // A jump is a vertical impulse: it sets `vy` and leaves `hv` alone so
    // run-up momentum carries into the leap.
    let mut vy = jump_velocity(character.vy, character.grounded, jump);
    let mut hv = character.hv;
    // The takeoff frame preserves entry momentum exactly: no ground steering
    // toward the wish, so releasing the stick mid-jump does not scrub speed.
    let taking_off = jump && character.grounded;
    if taking_off {
        // Leaving a moving platform carries its motion into the leap.
        hv += character.ride_vel;
    }

    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);
    let mut center = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);
    let frame_start = Vec2::new(center.x, center.z);

    // Standing still on a slope should not slide: without friction the
    // projected gravity would walk the character back down the ramp.
    let idle = wish == Vec2::ZERO && character.grounded && vy <= 0.01;

    for _ in 0..steps {
        if idle {
            vy = 0.0;
            hv = Vec2::ZERO;
        } else {
            vy -= GRAVITY * step_dt;
            if !taking_off {
                if was_grounded {
                    hv = ground_step(hv, target, step_dt);
                } else {
                    hv = air_step(hv, direction, wish_speed, step_dt);
                }
            }
        }
        let velocity = Vec3::new(hv.x, vy, hv.y);
        let mut hits: Vec<PushHit> = Vec::new();
        let output = move_and_slide.move_and_slide(
            &kit.character_collider,
            center.real(),
            rotation,
            velocity,
            std::time::Duration::from_secs_f32(step_dt),
            &MoveAndSlideConfig::default(),
            &filter,
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
        center = output.position.f32();
        vy = output.projected_velocity.y;
        // Keep the wall-slid horizontal velocity so inertia slides along
        // walls instead of pushing through them on the next substep.
        hv = Vec2::new(output.projected_velocity.x, output.projected_velocity.z);
        apply_pushes(&hits, &collider_of, &rigid, &masses, &mut bodies);
    }

    // Follow a descending slope for a short step instead of free-falling until
    // the next contact frame; a jump pressed between those frames was ignored.
    let mut feet = center.y - PILL_HALF;
    // Step-up assist: pushing into a low lip scrubs `hv` to nothing, so the
    // raw slide would pin the character at the border forever. When the frame
    // barely moved despite a clear wish, mount ledges within STEP_UP.
    let intended = target.length() * frame;
    let moved_len = Vec2::new(center.x, center.z).distance(frame_start);
    if was_grounded && target.length() > 0.3 && moved_len < intended * 0.25 {
        let dir = if hv.length() > 0.2 {
            hv.normalize_or_zero()
        } else {
            direction
        };
        if let Some((top, normal_y)) =
            step_up_top(&move_and_slide.spatial_query, &filter, center, feet, dir)
            && let Some(dest) = step_destination(
                &move_and_slide.spatial_query,
                &filter,
                &kit.character_collider,
                rotation,
                center,
                top,
                normal_y,
                dir,
            )
        {
            // Move onto the probed column as well as up: support below the
            // step is only found once the capsule is over the ledge.
            center = dest;
            feet = top;
            // Carry the intended pace onto the step instead of the scrubbed
            // slide remainder.
            hv = dir * target.length().max(0.5);
        }
    }
    let surface = move_and_slide
        .spatial_query
        .cast_ray(
            Vec3::new(center.x, feet + PILL_RADIUS, center.z).real(),
            Dir3::NEG_Y,
            PILL_RADIUS / MIN_GROUND_NORMAL + STEP_DOWN,
            true,
            &filter,
        )
        .filter(|hit| hit.normal.y > MIN_GROUND_NORMAL)
        .map(|hit| {
            let uphill = Vec2::new(hit.normal.x, hit.normal.z).dot(direction) < -0.05;
            (contact_gap(hit.distance, hit.normal.y), uphill)
        });
    let (grounded, drop) = if vy > MAX_GROUNDED_RISE {
        // A jump must not be cancelled by the ground it is leaving.
        (false, None)
    } else {
        surface.map_or((false, None), |(gap, uphill)| {
            ground_support(gap, was_grounded, uphill)
        })
    };
    if grounded {
        vy = 0.0;
    }
    if let Some(drop) = drop {
        feet -= drop;
    }

    character.pos = Vec2::new(center.x, center.z);
    character.feet = feet;
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

    character.grounded = grounded;
    character.vy = if grounded { 0.0 } else { vy };
    // Touching down on a moving platform converts back to platform-frame
    // velocity, so the takeoff fold cannot stack speed across hops.
    // Runs on the landing transition only, never while settled.
    if grounded && !was_grounded {
        hv -= character.ride_vel;
    }
    // On landing the stored momentum decays through `ground_step` over the
    // next frames, giving a short physical slide instead of a hard stop.
    character.hv = hv;
}
