use super::*;
use bevy::ecs::system::RunSystemOnce;

fn play_app(feet: f32) -> App {
    let mut app = physics_app();
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.mode = Mode::Play;
        editor.character = Some(Character {
            pos: Vec2::new(1.0, 2.0),
            feet,
            yaw: -std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    app
}

fn cone_count(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<ConeShot>>()
        .iter(app.world())
        .count()
}

fn cone_x(app: &mut App) -> Option<f32> {
    let mut query = app.world_mut().query::<(&Transform, &ConeShot)>();
    query
        .iter(app.world())
        .next()
        .map(|(transform, _)| transform.translation.x)
}

/// Tick the projectile flight the way the live schedule does: time passes,
/// then the cones advance against the freshly simulated colliders.
fn tick_cones(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.world_mut()
        .run_system_once(advance_cones)
        .expect("advance cones");
}

/// A dynamic crate floating at muzzle height straight ahead. The fired cone
/// leaves x = 1.45 at y = 1.25 heading +x; the crate face sits at x = 2.4.
fn crate_ahead(app: &mut App) -> Entity {
    let prop = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.425, 0.425, 0.425),
            Velocity::zero(),
            ExternalImpulse::default(),
            Transform::from_xyz(2.825, 1.25, 2.0),
        ))
        .id();
    // Let the physics pipeline register the new collider before firing.
    app.update();
    app.update();
    prop
}

#[test]
fn a_cone_shoves_a_dynamic_crate_and_still_despawns() {
    let mut app = play_app(0.0);
    let prop = crate_ahead(&mut app);
    app.world_mut().resource_mut::<TouchState>().shoot = true;
    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");
    assert_eq!(cone_count(&mut app), 1);

    let mut gone = false;
    for _ in 0..60 {
        tick_cones(&mut app);
        if cone_count(&mut app) == 0 {
            gone = true;
            break;
        }
    }
    assert!(
        gone,
        "the cone must despawn on the crate, not fly through it"
    );

    // The crate takes the hit as motion along the shot, like a capsule
    // shove: roughly the cone speed times the shared push factor. Flush the
    // physics schedule first so the impulse shows up in the velocity.
    app.update();
    let velocity = app
        .world()
        .get::<Velocity>(prop)
        .expect("dynamic prop keeps its velocity")
        .linear;
    assert!(
        velocity.x > 0.3,
        "the crate should gain forward speed, got {velocity:?}"
    );
}

/// A wall straight ahead of the muzzle at cone height. The fired cone leaves
/// x = 1.45 at y = 1.25 heading +x; the wall face sits at x = 3.9.
fn wall_ahead(app: &mut App) {
    app.world_mut().spawn((
        RigidBody::Fixed,
        Collider::cuboid(0.1, 1.4, 0.6),
        Transform::from_xyz(4.0, 1.4, 2.0),
    ));
    // Let the physics pipeline register the new collider before firing.
    app.update();
    app.update();
}

#[test]
fn a_cone_disappears_when_it_hits_a_wall() {
    let mut app = play_app(0.0);
    wall_ahead(&mut app);
    app.world_mut().resource_mut::<TouchState>().shoot = true;
    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");
    assert_eq!(cone_count(&mut app), 1);

    let mut furthest: f32 = 1.45;
    let mut gone_at = None;
    for tick in 0..60 {
        tick_cones(&mut app);
        match cone_x(&mut app) {
            Some(x) => furthest = furthest.max(x),
            None => {
                gone_at = Some(tick);
                break;
            }
        }
    }
    assert!(
        gone_at.is_some_and(|tick| tick < 30),
        "the cone must die on the wall, not fly through it"
    );
    assert!(
        furthest < 3.9,
        "the cone must never cross the wall face at x = 3.9, reached {furthest}"
    );
}

#[test]
fn a_cone_flies_its_full_range_when_nothing_is_in_the_way() {
    let mut app = play_app(0.0);
    app.world_mut().resource_mut::<TouchState>().shoot = true;
    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");

    // 30 ticks fly ~8.6 m: inside the 14 m range, so still alive.
    for _ in 0..30 {
        tick_cones(&mut app);
    }
    assert_eq!(cone_count(&mut app), 1, "a clear shot keeps flying");
    let travelled = cone_x(&mut app).expect("a cone") - 1.45;
    assert!(
        (travelled - 8.64).abs() < 0.5,
        "the cone flies at full speed while clear, travelled {travelled}"
    );

    // 60 ticks fly ~17 m: past the range, so it retires mid-air.
    for _ in 0..30 {
        tick_cones(&mut app);
    }
    assert_eq!(cone_count(&mut app), 0, "range expiry retires a clear shot");
}

#[test]
fn the_x_pad_button_fires_a_cone_along_the_facing() {
    let mut app = play_app(0.0);
    // What the X cluster button sets when its action is Shoot.
    app.world_mut().resource_mut::<TouchState>().shoot = true;

    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");

    assert_eq!(cone_count(&mut app), 1, "one press spawns one cone");
    let (origin, rotation, velocity) = {
        let mut query = app.world_mut().query::<(&Transform, &ConeShot)>();
        let (transform, cone) = query.single(app.world()).expect("a cone");
        (transform.translation, transform.rotation, cone.velocity)
    };
    // Facing -90 degrees is +x; the shot must leave the capsule that way.
    assert!(
        (velocity - Vec3::X * CONE_SPEED).length() < 1e-4,
        "{velocity:?}"
    );
    assert!(
        origin.x > 1.0,
        "the cone leads the capsule along its facing: {origin:?}"
    );
    assert!((origin.z - 2.0).abs() < 1e-5, "{origin:?}");
    assert!(
        (rotation * Vec3::Y - Vec3::X).length() < 1e-5,
        "the cone mesh points along its flight"
    );
    // The press is consumed, so holding does not fire every frame.
    assert!(!app.world().resource::<TouchState>().shoot);
}

#[test]
fn a_desktop_shoot_key_fires_a_cone() {
    let mut app = play_app(0.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyQ);

    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");
    assert_eq!(cone_count(&mut app), 1, "the desktop binding also fires");
}

#[test]
fn building_never_fires_and_never_queues_a_shot() {
    let mut app = play_app(0.0);
    app.world_mut().resource_mut::<Editor>().mode = Mode::Build;
    app.world_mut().resource_mut::<TouchState>().shoot = true;

    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");

    assert_eq!(cone_count(&mut app), 0, "Build mode is not gameplay");
    assert!(
        !app.world().resource::<TouchState>().shoot,
        "a Build-mode press must not leak into Play"
    );
}

#[test]
fn a_dead_actor_cannot_shoot() {
    let mut app = play_app(0.0);
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let mut character = editor.character.unwrap();
        character.health.current = 0.0;
        editor.character = Some(character);
    }
    app.world_mut().resource_mut::<TouchState>().shoot = true;

    app.world_mut()
        .run_system_once(shoot_cone)
        .expect("shoot cone");
    assert_eq!(cone_count(&mut app), 0);
}
