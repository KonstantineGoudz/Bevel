//! Cone projectiles: the actor fires a cone along its facing.
//!
//! Firing is gameplay, so it only runs in Play and only with a live actor.
//! The cone is a plain mesh entity with no collider of its own: every frame
//! it casts its cone shape ahead along its flight, so it disappears the
//! moment it would touch anything instead of passing through, and it
//! retires once it has flown its range. The spawn direction comes from the
//! same [`actor_facing`] helper the facing arrow draws with, so what you
//! see pointing is what you shoot.

use super::*;
use bevy_rapier3d::parry::query::ShapeCastOptions;

/// Flight speed of a fired cone, in m/s.
pub(super) const CONE_SPEED: f32 = 18.0;
/// How far one cone flies before retiring, in metres. The one knob for
/// projectile reach: raise it for a longer shot, lower it for a stubby one.
/// Lifetime stays as a backstop so a cone can never fly forever.
pub(super) const CONE_RANGE: f32 = 14.0;
/// How long one cone stays in the world, in seconds. Backstop only: at full
/// speed the range above runs out first.
pub(super) const CONE_LIFE: f32 = 1.6;
/// Cone mesh radius, in metres.
pub(super) const CONE_RADIUS: f32 = 0.16;
/// Cone mesh length (base to tip), in metres.
pub(super) const CONE_LENGTH: f32 = 0.55;
/// Height above the feet where the cone leaves the capsule.
pub(super) const CONE_MUZZLE: f32 = 1.25;
/// How far ahead of the capsule centre the cone spawns, so it never starts
/// inside the player's own body.
pub(super) const CONE_LEAD: f32 = 0.45;
/// Colour of the projectile.
pub(super) const CONE_COLOR: Color = Color::srgb_u8(0xff, 0xc4, 0x5c);

/// A cone in flight.
#[derive(Component)]
pub(super) struct ConeShot {
    pub(super) velocity: Vec3,
    pub(super) life: f32,
    /// Metres flown so far. Retires at [`range`](ConeShot::range).
    pub(super) travelled: f32,
    /// How far this cone may fly, in metres. Set from [`CONE_RANGE`] on
    /// spawn; a field (not just the const) so future work can vary shots.
    pub(super) range: f32,
}

/// Spawn pose for one shot: muzzle position and the rotation that points the
/// mesh's +Y axis along the flight direction. Pure so tests can pin it.
pub(super) fn cone_shot_pose(pos: Vec2, feet: f32, yaw: f32) -> (Vec3, Quat, Vec3) {
    let facing = actor_facing(yaw);
    let dir = Vec3::new(facing.x, 0.0, facing.y);
    let origin = Vec3::new(pos.x, feet + CONE_MUZZLE, pos.y) + dir * CONE_LEAD;
    (
        origin,
        Quat::from_rotation_arc(Vec3::Y, dir),
        dir * CONE_SPEED,
    )
}

/// Fire a cone when the Shoot action (desktop) or the cone touch button
/// (phones) is pressed. Runs once per press.
pub(super) fn shoot_cone(
    mut commands: Commands,
    input: PlayerInput,
    mut touch: ResMut<TouchState>,
    editor: Res<Editor>,
    kit: Res<Kit>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
) {
    // Firing is gameplay; in Build the action is inert and must not leak into
    // the next Play session.
    if editor.mode != Mode::Play {
        touch.shoot = false;
        return;
    }
    if focus.get().is_some() || menu.open {
        touch.shoot = false;
        return;
    }
    let pressed = input.just_pressed(Action::Shoot) || touch.shoot;
    touch.shoot = false;
    if !pressed {
        return;
    }
    let Some(character) = editor.character else {
        return;
    };
    if character.health.is_dead() {
        return;
    }
    let (origin, rotation, velocity) = cone_shot_pose(character.pos, character.feet, character.yaw);
    commands.spawn((
        ConeShot {
            velocity,
            life: CONE_LIFE,
            travelled: 0.0,
            range: CONE_RANGE,
        },
        Mesh3d(kit.cone.clone()),
        MeshMaterial3d(kit.cone_mat.clone()),
        Transform::from_translation(origin).with_rotation(rotation),
    ));
}

/// Hand one cone impact to the dynamic body behind `collider`, if there is
/// one. Static geometry is untouched; the impulse only accelerates the body
/// up to the cone's inward speed so light props take the hit without
/// launching into orbit. Same velocity-matching shove the capsule uses.
#[allow(clippy::too_many_arguments)]
fn shove_body(
    collider: Entity,
    normal: Vec3,
    cone_velocity: Vec3,
    point: Vec3,
    parents: &Query<&ChildOf>,
    rigid: &Query<&RigidBody>,
    bodies: &mut Query<
        (
            &mut ExternalImpulse,
            &Velocity,
            &ReadMassProperties,
            &Transform,
        ),
        With<RigidBody>,
    >,
) {
    let target = push_target(collider, parents);
    let Ok(body) = rigid.get(target) else {
        return;
    };
    if *body != RigidBody::Dynamic {
        return;
    }
    let Ok((mut impulse, velocity, props, transform)) = bodies.get_mut(target) else {
        return;
    };
    let mass = props.get().mass;
    let com_world = transform.translation + transform.rotation * props.get().local_center_of_mass;
    let Some(push) = push_impulse(normal, cone_velocity, velocity.linear, mass) else {
        return;
    };
    impulse.impulse += push;
    impulse.torque_impulse += (point - com_world).cross(push);
}

/// Fly every cone forward. A cone disappears the moment its shape cast
/// touches anything, and retires once it has flown its range (lifetime is
/// only the backstop). The cast runs against the live colliders, so walls,
/// floors, platforms and props all stop it — no pass-through, whatever the
/// controller did. A dynamic body in the way picks up the cone's motion
/// through the same velocity-matching impulse the capsule shoves with, then
/// the cone still despawns on the spot.
pub(super) fn advance_cones(
    time: Res<Time>,
    mut commands: Commands,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
    parents: Query<&ChildOf>,
    rigid: Query<&RigidBody>,
    // Cones never carry a RigidBody, so the With/Without pair keeps the
    // two Transform accesses disjoint.
    mut bodies: Query<
        (
            &mut ExternalImpulse,
            &Velocity,
            &ReadMassProperties,
            &Transform,
        ),
        With<RigidBody>,
    >,
    mut cones: Query<(Entity, &mut ConeShot, &mut Transform), Without<RigidBody>>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.25);
    if dt <= 0.0 {
        return;
    }
    let Ok(ctx) = rapier.single() else {
        return;
    };
    // The muzzle leads the capsule, but never cast against the player anyway.
    let filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    // Rapier's cone takes half height first, radius second.
    let shape = Collider::cone(CONE_LENGTH * 0.5, CONE_RADIUS);
    for (entity, mut cone, mut transform) in &mut cones {
        let speed = cone.velocity.length();
        if !speed.is_finite() || speed <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let dir = cone.velocity / speed;
        let remaining = cone.range - cone.travelled;
        if remaining <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let step = (speed * dt).min(remaining);
        let origin = transform.translation;
        let options = ShapeCastOptions::with_max_time_of_impact(step);
        if let Some((hit_entity, hit)) =
            ctx.cast_shape(origin, transform.rotation, dir, &shape, options, filter)
        {
            // Contact with anything ends the flight on the spot. A dynamic
            // body also takes the hit as motion, like a capsule shove.
            // Scene queries report the hit collider in `details.normal1`
            // (world space); without impact geometry there is no shove.
            if let Some(details) = hit.details {
                shove_body(
                    hit_entity,
                    details.normal1,
                    cone.velocity,
                    details.witness1,
                    &parents,
                    &rigid,
                    &mut bodies,
                );
            }
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation = origin + dir * step;
        cone.travelled += step;
        cone.life -= dt;
        if cone.travelled >= cone.range - 1e-4 || cone.life <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn a_shot_leaves_the_capsule_facing_forward() {
        // Yaw -90 degrees faces +x (the sprint direction used by the
        // movement tests), so the cone must lead the capsule along +x.
        let (origin, rotation, velocity) =
            cone_shot_pose(Vec2::new(2.0, 3.0), 0.5, -std::f32::consts::FRAC_PI_2);
        assert!((origin.x - (2.0 + CONE_LEAD)).abs() < 1e-5, "{origin:?}");
        assert!((origin.y - (0.5 + CONE_MUZZLE)).abs() < 1e-6);
        assert!((origin.z - 3.0).abs() < 1e-5);
        assert!(
            (velocity - Vec3::X * CONE_SPEED).length() < 1e-4,
            "{velocity:?}"
        );
        // The mesh's +Y must end up along the flight direction.
        let tip = rotation * Vec3::Y;
        assert!((tip - Vec3::X).length() < 1e-5, "{tip:?}");
    }

    #[test]
    fn yaw_zero_shoots_towards_minus_z() {
        let (_, rotation, velocity) = cone_shot_pose(Vec2::ZERO, 0.0, 0.0);
        assert!(
            (velocity - Vec3::NEG_Z * CONE_SPEED).length() < 1e-4,
            "{velocity:?}"
        );
        assert!((rotation * Vec3::Y - Vec3::NEG_Z).length() < 1e-5);
    }

    #[test]
    fn a_cone_retires_after_its_life_runs_out() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            RapierPhysicsPlugin::<NoUserData>::default(),
        ));
        // Let the plugin spawn its Rapier context before the first query.
        app.update();
        let entity = app
            .world_mut()
            .spawn((
                ConeShot {
                    velocity: Vec3::X * 10.0,
                    life: 0.05,
                    travelled: 0.0,
                    range: CONE_RANGE,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut()
            .run_system_once(advance_cones)
            .expect("advance cones");
        let moved = app.world().get::<Transform>(entity).unwrap().translation;
        assert!(moved.x > 0.0, "an alive cone flies: {moved:?}");
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        app.world_mut()
            .run_system_once(advance_cones)
            .expect("advance cones");
        assert!(
            app.world().get::<Transform>(entity).is_none(),
            "an expired cone despawns"
        );
    }

    #[test]
    fn a_cone_retires_once_it_has_flown_its_range() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            RapierPhysicsPlugin::<NoUserData>::default(),
        ));
        // Let the plugin spawn its Rapier context before the first query.
        app.update();
        let entity = app
            .world_mut()
            .spawn((
                ConeShot {
                    velocity: Vec3::X * 10.0,
                    life: CONE_LIFE,
                    travelled: 0.0,
                    range: 0.5,
                },
                Transform::default(),
            ))
            .id();
        // 16 ms at 10 m/s flies 0.16 m: still short of the 0.5 m range.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut()
            .run_system_once(advance_cones)
            .expect("advance cones");
        assert!(
            app.world().get::<Transform>(entity).is_some(),
            "a cone inside its range keeps flying"
        );
        // Four more ticks fly 0.8 m total: past the range, so it retires
        // with lifetime to spare.
        for _ in 0..4 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.world_mut()
                .run_system_once(advance_cones)
                .expect("advance cones");
        }
        assert!(
            app.world().get::<Transform>(entity).is_none(),
            "a cone past its range despawns"
        );
    }
}
