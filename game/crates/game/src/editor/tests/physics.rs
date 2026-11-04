use super::*;

#[test]
fn camera_and_character_changes_are_noticed() {
    let seen = Seen {
        orbit: Orbit::default(),
        character: None,
    };
    assert!(!soft_changed(&seen, &Orbit::default(), None));
    let moved = Orbit {
        yaw: 1.0,
        ..Orbit::default()
    };
    assert!(soft_changed(&seen, &moved, None));
    let character = Character {
        pos: Vec2::ZERO,
        feet: 0.0,
        yaw: 0.0,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    };
    assert!(soft_changed(&seen, &Orbit::default(), Some(character)));
}

#[test]
fn the_route_line_climbs_a_ramp_and_ignores_wall_tops() {
    // A ramp rising from 0 to 0.9 between x = 1 and x = 3, then a block.
    let ramp = |p: Vec2| -> Option<f32> {
        Some(if p.x < 1.0 {
            0.0
        } else if p.x < 3.0 {
            (p.x - 1.0) * 0.45
        } else {
            0.9
        })
    };
    let points: Vec<Vec2> = (0..11).map(|i| Vec2::new(i as f32 * 0.3, 0.0)).collect();
    let heights = lift_heights(points.iter().copied(), ramp, 0.0);
    assert_eq!(heights.len(), 11);
    assert!((heights[0] - 0.0).abs() < 1e-4);
    assert!((heights[5] - 0.225).abs() < 1e-4, "mid-ramp {heights:?}");
    assert!((heights[10] - 0.9).abs() < 1e-4, "block top {heights:?}");
    assert!(
        heights.windows(2).all(|pair| pair[0] <= pair[1] + 1e-4),
        "the line should only climb: {heights:?}"
    );

    // A wall top (2.8) beside the curve is not treated as a step.
    let wall = |p: Vec2| Some(if (p.x - 1.0).abs() < 0.05 { 2.8 } else { 0.0 });
    let heights = lift_heights(
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
        ]
        .into_iter(),
        wall,
        0.0,
    );
    assert_eq!(heights, vec![0.0, 0.0, 0.0]);

    // A missed ray between two surfaces keeps the last height (no crack).
    let missing = lift_heights(
        [Vec2::ZERO, Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0)].into_iter(),
        |_| None,
        0.9,
    );
    assert_eq!(missing, vec![0.9, 0.9, 0.9]);
}

#[test]
fn a_jump_launches_only_from_the_ground() {
    assert_eq!(jump_velocity(0.0, true, true), JUMP_SPEED);
    assert_eq!(jump_velocity(0.0, true, false), 0.0);
    assert_eq!(jump_velocity(-3.0, false, true), -3.0);
}

#[test]
fn ground_velocity_chases_the_wish() {
    let dt = 1.0 / 60.0;
    // A standing start moves toward the target but does not teleport.
    let stepped = ground_step(Vec2::ZERO, Vec2::new(2.8, 0.0), dt);
    assert!(
        stepped.x > 0.0 && stepped.x < 2.8,
        "one step should make progress without snapping: {stepped:?}"
    );
    // Close enough snaps to the target.
    assert_eq!(
        ground_step(Vec2::new(2.79, 0.0), Vec2::new(2.8, 0.0), dt),
        Vec2::new(2.8, 0.0)
    );
    // A reversal takes more than one frame: momentum matters on the ground.
    let turned = ground_step(Vec2::new(2.8, 0.0), Vec2::new(-2.8, 0.0), dt);
    assert!(turned.x > 0.0, "should still move forward: {turned:?}");
    // Stopping decays instead of halting instantly.
    let stopping = ground_step(Vec2::new(2.8, 0.0), Vec2::ZERO, dt);
    assert!(
        stopping.x > 2.0 && stopping.x < 2.8,
        "landing should slide a little: {stopping:?}"
    );
}

#[test]
fn air_keeps_inertia_without_steering() {
    let dt = 1.0 / 60.0;
    let cruising = Vec2::new(2.8, 0.0);
    // No input: velocity is untouched.
    assert_eq!(air_step(cruising, Vec2::ZERO, 0.0, dt), cruising);
    // A jump-length coast with no steering loses nothing.
    let mut coast = cruising;
    for _ in 0..48 {
        coast = air_step(coast, Vec2::ZERO, 0.0, dt);
    }
    assert_eq!(coast, cruising);
}

#[test]
fn air_strafing_is_limited() {
    let dt = 1.0 / 60.0;
    // Shoving back against forward momentum only nibbles at it.
    let braked = air_step(Vec2::new(2.8, 0.0), Vec2::new(-1.0, 0.0), 2.8, dt);
    assert!(
        braked.x > 2.5,
        "one frame must not reverse momentum: {braked:?}"
    );
    // Strafing sideways adds a little lateral speed while keeping most of
    // the forward run-up.
    let strafed = air_step(Vec2::new(2.8, 0.0), Vec2::Y, 2.8, dt);
    assert!(strafed.y > 0.0 && strafed.y < 0.3, "{strafed:?}");
    assert!(strafed.x > 2.5, "forward momentum survives: {strafed:?}");
    // Speed never grows past the faster of entry speed and wish speed.
    let mut fast = Vec2::new(5.5, 0.0);
    for _ in 0..120 {
        fast = air_step(fast, Vec2::X, 2.8, dt);
    }
    assert!(
        fast.length() <= 5.5 + 1e-4,
        "sprint momentum caps air speed: {fast:?}"
    );
    // From standstill the wish still builds up over the jump.
    let mut still = Vec2::ZERO;
    for _ in 0..120 {
        still = air_step(still, Vec2::X, 2.8, dt);
    }
    assert!(
        (still.x - 2.8).abs() < 0.05,
        "a standing jump steers up to the wish: {still:?}"
    );
}

#[test]
fn slide_keeps_momentum_and_only_bleeds_speed_slowly() {
    let dt = 1.0 / 60.0;
    // No steering: direction is untouched and one frame of slide friction
    // costs far less than one frame of normal ground braking.
    let sliding = slide_step(Vec2::new(5.5, 0.0), Vec2::ZERO, dt);
    assert!(
        sliding.x > 5.4 && sliding.y == 0.0,
        "a slide should coast on momentum: {sliding:?}"
    );
    let braking = ground_step(Vec2::new(5.5, 0.0), Vec2::ZERO, dt);
    assert!(
        sliding.x > braking.x,
        "slide friction must be lower than ground braking: slide {sliding:?} vs ground {braking:?}"
    );
    // A sprint slide is still moving fast a second later, unlike a normal
    // landing which ground control would have stopped in a few frames.
    let mut coast = Vec2::new(5.5, 0.0);
    for _ in 0..60 {
        coast = slide_step(coast, Vec2::ZERO, dt);
    }
    assert!(
        coast.x > 2.5,
        "a second of sliding should retain momentum: {coast:?}"
    );
    // Steering bends the slide but never speeds it up or snaps it around.
    let turned = slide_step(Vec2::new(5.5, 0.0), Vec2::new(-1.0, 0.0), dt);
    assert!(
        turned.x > 5.0 && turned.length() <= 5.5 + 1e-4,
        "one frame of opposite steering must not reverse or boost a slide: {turned:?}"
    );
    let mut steered = Vec2::new(5.5, 0.0);
    for _ in 0..40 {
        steered = slide_step(steered, Vec2::Y, dt);
    }
    assert!(
        steered.y > 1.0,
        "held steering should eventually bend the slide: {steered:?}"
    );
    assert!(
        steered.length() < 5.5,
        "steering a slide never adds speed: {steered:?}"
    );
}

#[test]
fn a_slide_only_starts_on_a_landing_with_ctrl_and_momentum() {
    // The requested case: airborne, then touchdown with Ctrl held.
    assert!(next_sliding(false, true, true, true, 5.5));
    // Ctrl pressed while already running on the ground: not a landing.
    assert!(!next_sliding(false, false, true, true, 5.5));
    // Landing without Ctrl, or releasing it mid-slide, ends/never starts.
    assert!(!next_sliding(false, true, false, true, 5.5));
    assert!(!next_sliding(true, false, false, true, 5.5));
    // A standing hop has no momentum to slide on.
    assert!(!next_sliding(false, true, true, true, 0.5));
    // Leaving the ground ends a slide even with Ctrl still held.
    assert!(!next_sliding(true, false, true, false, 5.5));
    // An ongoing slide persists while fast, and ends once it slows.
    assert!(next_sliding(true, false, true, true, 2.0));
    assert!(!next_sliding(true, false, true, true, 0.5));
}

#[test]
fn a_sliding_capsule_squashes_and_leans_along_its_momentum() {
    let mut character = Character {
        pos: Vec2::new(1.0, 2.0),
        feet: 0.0,
        yaw: 0.0,
        vy: 0.0,
        hv: Vec2::new(5.5, 0.0),
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: true,
        health: crate::statuses::Health::default(),
    };
    let (pos, rot, scale) = character_pose(character);
    assert_eq!(pos.x, 1.0);
    assert_eq!(pos.z, 2.0);
    assert!(
        pos.y < PILL_HALF && pos.y > 0.0,
        "a sliding capsule sits lower without sinking through the floor: {pos:?}"
    );
    assert!(scale.y < 1.0 && scale.x > 1.0, "slide squash: {scale:?}");
    // The capsule's up axis leans toward +x, the slide direction.
    let up = rot * Vec3::Y;
    assert!(
        up.x > 0.3 && up.y > 0.0 && up.y < 1.0,
        "the capsule should lean forward along its momentum: {up:?}"
    );

    character.sliding = false;
    let (pos, rot, scale) = character_pose(character);
    assert_eq!(pos.y, PILL_HALF);
    assert_eq!(rot, Quat::from_rotation_y(character.yaw));
    assert_eq!(scale, Vec3::ONE);
}

#[test]
fn a_slope_counts_as_ground_on_the_way_down() {
    assert_eq!(ground_support(0.0, false, false), (true, None), "standing");
    assert_eq!(
        ground_support(0.01, false, false),
        (true, Some(0.01)),
        "contact"
    );
    assert_eq!(
        ground_support(0.2, true, false),
        (true, Some(0.2)),
        "walking down a ramp"
    );
    assert_eq!(
        ground_support(0.2, false, false),
        (false, None),
        "falling past a ledge"
    );
    assert_eq!(
        ground_support(0.4, true, false),
        (false, None),
        "too far below to step down"
    );
    assert_eq!(
        ground_support(-0.02, false, false),
        (true, None),
        "slight penetration is ground, but must not be lifted"
    );
    assert_eq!(
        ground_support(0.05, true, true),
        (true, None),
        "an ascent is ground but is never re-seated"
    );
}

#[test]
fn a_rounded_capsule_on_a_slope_has_no_gap() {
    assert!((contact_gap(PILL_RADIUS, 1.0)).abs() < 1e-6, "flat");
    assert!((contact_gap(PILL_RADIUS / 0.9, 0.9)).abs() < 1e-5, "slope");
    assert!(
        contact_gap(PILL_RADIUS + 0.06, 1.0) > 0.0,
        "above the ground"
    );
}

#[test]
fn third_person_can_jump_without_a_route() {
    let mut app = physics_app();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);

    tick_character(&mut app);

    let feet = app.world().resource::<Editor>().character.unwrap().feet;
    assert!(
        feet > 0.05,
        "character should leave the ground, feet={feet}"
    );
}

#[test]
fn an_airborne_character_lands_without_a_route() {
    let mut app = physics_app();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 1.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: false,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;

    for _ in 0..120 {
        tick_character(&mut app);
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.feet < 0.2,
        "should fall back to the floor, feet={}",
        character.feet
    );
    assert!(character.grounded, "should be grounded after landing");
}

#[test]
fn a_jump_carries_forward_momentum() {
    let mut app = physics_app();
    // Forward along +x with this yaw convention.
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            // Sprinting along +x when the jump fires.
            hv: Vec2::new(5.5, 0.0),
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    // No steering held: only the jump button.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);

    tick_character(&mut app);

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.grounded, "should be airborne after jumping");
    assert!(
        (character.hv.x - 5.5).abs() < 0.3,
        "takeoff keeps sprint momentum, hv={:?}",
        character.hv
    );

    // Release everything and coast: inertia alone should keep carrying the
    // character forward while airborne.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::Space);
    let before = character.pos.x;
    for _ in 0..20 {
        tick_character(&mut app);
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        !character.grounded,
        "should still be airborne mid-jump, feet={}",
        character.feet
    );
    assert!(
        character.pos.x - before > 0.8,
        "momentum should carry the jump with no input, moved {}",
        character.pos.x - before
    );
    assert!(
        character.hv.x > 3.5,
        "airborne speed decays only a little, hv={:?}",
        character.hv
    );
}

/// A sprinting character at the origin, ready to jump along +x.
fn sprint_jumper(app: &mut App, hv: Vec2) {
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    let mut editor = app.world_mut().resource_mut::<Editor>();
    editor.character = Some(Character {
        pos: Vec2::ZERO,
        feet: 0.0,
        yaw: -std::f32::consts::FRAC_PI_2,
        vy: 0.0,
        hv,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    });
    editor.walk = None;
}

fn tick_until_grounded(app: &mut App) -> Character {
    for _ in 0..180 {
        tick_character(app);
        let character = app.world().resource::<Editor>().character.unwrap();
        if character.grounded {
            return character;
        }
    }
    panic!("character never landed");
}

#[test]
fn holding_ctrl_through_a_jump_lands_in_a_momentum_slide() {
    let mut app = physics_app();
    sprint_jumper(&mut app, Vec2::new(5.5, 0.0));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ControlLeft);

    tick_character(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.grounded, "should be airborne after jumping");
    assert!(!character.sliding, "no slide while still in the air");

    // Keep Ctrl held, let go of Space so the landing does not re-jump.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::Space);
    let landed = tick_until_grounded(&mut app);
    assert!(
        landed.sliding,
        "landing with Left Ctrl held after a jump should start a slide, hv={:?}",
        landed.hv
    );
    assert!(
        landed.hv.x > 3.5,
        "the slide should start on jump momentum, hv={:?}",
        landed.hv
    );

    // No steering at all: momentum alone carries the slide, bleeding speed
    // far slower than normal ground braking would.
    let before = landed.pos.x;
    for _ in 0..30 {
        tick_character(&mut app);
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x - before > 1.5,
        "the slide should coast forward with no input, moved {}",
        character.pos.x - before
    );
    assert!(
        character.sliding && character.hv.x > 3.0,
        "the slide should still be alive half a second in, hv={:?} sliding={}",
        character.hv,
        character.sliding
    );

    // Releasing Ctrl ends the slide; normal ground control takes over.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::ControlLeft);
    tick_character(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.sliding, "releasing Ctrl should end the slide");
}

#[test]
fn landing_a_jump_without_ctrl_does_not_slide() {
    let mut app = physics_app();
    sprint_jumper(&mut app, Vec2::new(5.5, 0.0));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    tick_character(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::Space);

    let landed = tick_until_grounded(&mut app);
    assert!(!landed.sliding, "a plain landing must not slide");

    // Normal landing braking scrubs the sprint within a few frames.
    for _ in 0..30 {
        tick_character(&mut app);
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        !character.sliding && character.hv.x < 1.0,
        "without a slide, ground control should stop the landing run, hv={:?}",
        character.hv
    );
}

#[test]
fn ctrl_on_the_ground_without_a_jump_does_not_slide() {
    let mut app = physics_app();
    sprint_jumper(&mut app, Vec2::new(5.5, 0.0));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ControlLeft);
    for _ in 0..10 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        assert!(
            !character.sliding,
            "Ctrl alone on the ground must never start a slide"
        );
    }
}

#[test]
fn a_standing_hop_landing_with_ctrl_does_not_slide() {
    let mut app = physics_app();
    sprint_jumper(&mut app, Vec2::ZERO);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ControlLeft);
    tick_character(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::Space);

    let landed = tick_until_grounded(&mut app);
    assert!(
        !landed.sliding,
        "a hop with no horizontal momentum has nothing to slide on, hv={:?}",
        landed.hv
    );
}

#[test]
fn jumping_out_of_a_slide_cancels_it() {
    let mut app = physics_app();
    sprint_jumper(&mut app, Vec2::new(5.5, 0.0));
    app.world_mut()
        .resource_mut::<Editor>()
        .character
        .as_mut()
        .unwrap()
        .sliding = true;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ControlLeft);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);

    tick_character(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.grounded, "should be airborne after slide-jump");
    assert!(!character.sliding, "the jump should cancel the slide");
    assert!(
        character.hv.x > 4.5,
        "the slide-jump should keep slide momentum, hv={:?}",
        character.hv
    );
}

#[test]
fn air_steering_cannot_snap_momentum_around() {
    let mut app = physics_app();
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 1.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            // Already flying along +x.
            hv: Vec2::new(2.8, 0.0),
            ride_vel: Vec2::ZERO,
            grounded: false,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    // Hold backwards (-x) for one frame: momentum must survive.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyS);
    tick_character(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.hv.x > 2.0,
        "one frame of opposite steering must not kill momentum, hv={:?}",
        character.hv
    );
}

/// A block piece shuttling along +x with a rider standing on it. The world
/// collider stays put (visual sync is not running) while the editor piece —
/// the source of carry truth — travels; the rider starts centred so a short
/// ride stays over both.
fn riding_setup(app: &mut App) {
    let piece = Piece {
        id: 7,
        shape: model::shape_index("block").unwrap(),
        pos: Vec2::ZERO,
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
    };
    app.world_mut().resource_mut::<Editor>().pieces.push(piece);
    let mut movers = Movers::default();
    let mover = movers.mover_mut(7, Vec2::ZERO);
    mover
        .waypoints
        .push(crate::platforms::PlatformWaypoint::new(Vec2::X * 4.0));
    mover.rebuild();
    mover.speed = 1.5;
    app.insert_resource(movers);
    // Static world geometry matching the platform's start pose.
    let block = &shapes()[model::shape_index("block").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(block).unwrap(),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    app.update();
    app.update();
}

fn riding_character() -> Character {
    Character {
        pos: Vec2::ZERO,
        feet: 0.9,
        yaw: -std::f32::consts::FRAC_PI_2,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    }
}

#[test]
fn jumping_from_a_moving_platform_carries_its_motion() {
    let mut app = physics_app();
    riding_setup(&mut app);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(riding_character());
        editor.walk = None;
    }

    // Ride a few frames with no input: positional carry plus the stamped
    // ride velocity, while `hv` stays put.
    for _ in 0..10 {
        tick_rider(&mut app);
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        (character.ride_vel.x - 1.5).abs() < 0.05,
        "ride velocity tracks the platform, ride_vel={:?}",
        character.ride_vel
    );
    assert!(
        (character.pos.x - 0.24).abs() < 0.05,
        "carry moves the rider, pos={:?}",
        character.pos
    );
    assert!(
        character.hv.length() < 0.3,
        "standing adds no self motion, hv={:?}",
        character.hv
    );

    // Jump with no steering: the leap inherits the full ride.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    tick_rider(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.grounded, "should be airborne after jumping");
    assert!(
        (character.hv.x - 1.5).abs() < 0.3,
        "takeoff inherits platform motion, hv={:?}",
        character.hv
    );

    // Coast with no input: inertia keeps carrying the jump along.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::Space);
    let before = character.pos.x;
    for _ in 0..20 {
        tick_rider(&mut app);
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        !character.grounded,
        "should still be airborne mid-jump, feet={}",
        character.feet
    );
    assert!(
        character.pos.x - before > 0.35,
        "the jump drifts with the ride, moved {}",
        character.pos.x - before
    );
}

#[test]
fn landing_on_a_moving_platform_does_not_stack_speed() {
    let mut app = physics_app();
    riding_setup(&mut app);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        // Dropping in fast from just above the platform top.
        let mut falling = riding_character();
        falling.feet = 1.2;
        falling.hv = Vec2::new(3.0, 0.0);
        falling.grounded = false;
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(falling);
        editor.walk = None;
    }

    let mut landed = false;
    for _ in 0..120 {
        tick_rider(&mut app);
        if app.world().resource::<Editor>().character.unwrap().grounded {
            landed = true;
            break;
        }
    }
    assert!(landed, "should land on the platform");
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        (character.hv.x - 1.5).abs() < 0.35,
        "landing converts back to ride-frame velocity, hv={:?}",
        character.hv
    );

    // Hopping again inherits the ride exactly once: no stacking.
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    tick_rider(&mut app);
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(!character.grounded, "should be airborne after jumping");
    assert!(
        (character.hv.x - 3.0).abs() < 0.4,
        "one fold per jump, hv={:?}",
        character.hv
    );
}

/// True Parry contact check: no tracked platform may penetrate the capsule
/// beyond a hair. Uses the real collider shapes, so this fails exactly when
/// the player would see overlap. Same Parry contact the movers use, so the
/// helper and the game can never disagree about what "overlap" means.
fn assert_platforms_clear(app: &App, context: &str) {
    use bevy_rapier3d::parry::query::contact;

    let world = app.world();
    let editor = world.resource::<Editor>();
    let movers = world.resource::<Movers>();
    let kit = world.resource::<Kit>();
    let character = editor.character.expect("character");
    let cap_iso = super::super::movers::iso(
        Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y),
        Quat::from_rotation_y(character.yaw),
    );
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
        let plat_iso = super::super::movers::iso(
            Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
            model::piece_quat(piece),
        );
        let hit = contact(
            &cap_iso,
            kit.character_collider.as_shape(),
            &plat_iso,
            platform.as_shape(),
            0.0,
        )
        .expect("contact query");
        if let Some(hit) = hit {
            assert!(
                -hit.dist <= 0.002,
                "{context}: penetration {:.3}: char={:?} piece={:?}",
                -hit.dist,
                character.pos,
                piece.pos,
            );
        }
    }
}

/// A platform driving sideways into a standing capsule must shove it ahead
/// instead of passing through. The capsule is idle (no input, grounded), the
/// case the grounded controller would otherwise never resolve.
#[test]
fn a_still_capsule_is_pushed_by_a_moving_platform() {
    let mut app = physics_app();
    riding_setup(&mut app);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            // Parked just ahead of the block's +x face (half width 0.9 plus
            // the 0.32 capsule radius clears at ~1.22).
            pos: Vec2::new(1.5, 0.0),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }

    // No input at all: the capsule idles while the platform drives into it.
    let start = app.world().resource::<Editor>().character.unwrap().pos.x;
    for _ in 0..120 {
        tick_rider(&mut app);
        // The frame must always end clear of every tracked platform.
        assert_platforms_clear(&app, "idle capsule");
    }
    let end = app.world().resource::<Editor>().character.unwrap().pos.x;
    assert!(
        end - start > 0.5,
        "the idle capsule must be shoved ahead of the platform, start={start} end={end}"
    );
}

/// An idle capsule standing on a block that gets shoved off must start
/// falling immediately, even with no input. It used to keep `grounded = true`
/// because the idle branch never checked whether anything was still underfoot,
/// so it hovered until the first movement input re-entered the normal
/// controller.
#[test]
fn a_capsule_pushed_off_a_block_falls_while_idle() {
    let mut app = physics_app();
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;

    // A fixed block for the capsule to stand on.
    let block = model::shape_index("block").unwrap();
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(&shapes()[block]).unwrap(),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // A tall pillar sweeping through the capsule's body: a pure side push, not
    // a deck it could ride.
    let pillar = model::shape_index("pillar").unwrap();
    app.world_mut().resource_mut::<Editor>().pieces.push(Piece {
        id: 7,
        shape: pillar,
        pos: Vec2::new(-2.0, 0.0),
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
    });
    let mut movers = Movers::default();
    let mover = movers.mover_mut(7, Vec2::new(-2.0, 0.0));
    mover
        .waypoints
        .push(crate::platforms::PlatformWaypoint::new(Vec2::new(2.0, 0.0)));
    mover.rebuild();
    mover.speed = 1.5;
    app.insert_resource(movers);

    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.9,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }

    let mut airborne = false;
    let mut minimum_feet = f32::MAX;
    for _ in 0..240 {
        tick_rider(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        minimum_feet = minimum_feet.min(character.feet);
        if !character.grounded {
            airborne = true;
        }
        if airborne && character.grounded && character.feet.abs() < 0.05 {
            break;
        }
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        airborne,
        "the idle capsule must leave the block after the push, pos={:?} feet={} grounded={}",
        character.pos, character.feet, character.grounded
    );
    assert!(
        minimum_feet < 0.5,
        "the capsule must actually descend, minimum feet={minimum_feet}"
    );
    assert!(
        character.grounded && character.feet.abs() < 0.05,
        "the capsule must land on the floor, pos={:?} feet={} grounded={}",
        character.pos,
        character.feet,
        character.grounded
    );
}

/// Same side-swipe while airborne: the platform must still shove, and the
/// capsule must never end a frame intersecting it.
#[test]
fn an_airborne_capsule_is_pushed_by_a_moving_platform() {
    let mut app = physics_app();
    riding_setup(&mut app);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let mut hovering = riding_character();
        hovering.pos = Vec2::new(1.5, 0.0);
        hovering.feet = 0.2;
        hovering.grounded = false;
        editor.character = Some(hovering);
        editor.walk = None;
    }

    let start = app.world().resource::<Editor>().character.unwrap().pos.x;
    for _ in 0..120 {
        tick_rider(&mut app);
        assert_platforms_clear(&app, "airborne capsule");
    }
    let end = app.world().resource::<Editor>().character.unwrap().pos.x;
    assert!(
        end - start > 0.3,
        "the airborne capsule must be shoved, start={start} end={end}"
    );
}

/// An off-centre hit deflects sideways off the face instead of surfing it:
/// a capsule clipped by the platform's corner slides clear to the side and
/// the platform travels on past it, rather than dragging the capsule along.
#[test]
fn a_corner_hit_deflects_aside_instead_of_surfing() {
    let mut app = physics_app();
    riding_setup(&mut app);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            // Ahead of the block but off to the side: the face would shove
            // it straight forward forever if the push never escaped.
            pos: Vec2::new(1.5, 0.8),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }

    for _ in 0..120 {
        tick_rider(&mut app);
        assert_platforms_clear(&app, "corner hit");
    }
    let editor = app.world().resource::<Editor>();
    let character = editor.character.unwrap();
    let platform = editor.pieces.iter().find(|p| p.id == 7).unwrap().pos;
    assert!(
        character.pos.y > 1.15,
        "the capsule must slide clear of the face (half depth 0.9 + radius 0.32), pos={:?}",
        character.pos,
    );
    assert!(
        platform.x - character.pos.x > 0.5,
        "the platform must travel on past the escaped capsule, char={:?} platform={:?}",
        character.pos,
        platform,
    );
    assert_eq!(
        character.ride_vel,
        Vec2::ZERO,
        "nothing touches the capsule anymore, so no ride velocity may linger"
    );
}

/// Walk from a floor tile into an adjacent ramp's low edge, the way a player
/// crosses the tile border in the game. The ramp must be mountable without
/// jumping.
#[test]
fn walking_off_a_tile_mounts_the_next_ramp() {
    let mut app = physics_app();
    let floor = &shapes()[model::shape_index("floor").unwrap()];
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    // Tile centred on the origin; its vertical edge sits near x = 1.04.
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(floor).unwrap(),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    // Ramp low edge flush with the tile edge, rising along +x.
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.04, 0.0, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(-0.5, 0.0),
            feet: 0.0,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    // The ramp is short: the character mounts it, rides up and walks off the
    // far side, so track the peak height along the way.
    let mut peak = 0.0f32;
    for _ in 0..240 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        peak = peak.max(character.feet);
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x > 3.4,
        "should cross the tile border, climb the ramp and walk off, pos={:?} feet={}",
        character.pos,
        character.feet
    );
    assert!(
        peak > 0.5,
        "should ride up the ramp slope on the way, peak={peak}"
    );
}

/// Repro: a character parked right before a ramp's low edge, then pressing W.
/// The first step from a standing start barely moves, which once tripped the
/// step-up assist: it read the ramp slope a half metre ahead as a ledge and
/// teleported the capsule forward onto it. A flush ramp has no vertical face to
/// step over, so the first frames must accelerate in place and only climb once
/// the capsule actually reaches and slides up the slope.
#[test]
fn a_standing_start_does_not_teleport_onto_a_ramp() {
    let mut app = physics_app();
    let (pieces, _) = crate::editor::editing::course();
    for piece in &pieces {
        let shape = &shapes()[piece.shape];
        let rot = model::piece_quat(piece);
        app.world_mut().spawn((
            RigidBody::Fixed,
            piece_collider(shape).unwrap(),
            Transform {
                translation: Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
                rotation: rot,
                ..default()
            },
        ));
    }
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        // Stand just short of the ramp's low edge (x ~ 1.078) with no run-up.
        let mut character = crate::editor::editing::course().1;
        character.pos = Vec2::new(0.72, 0.0);
        character.grounded = true;
        editor.character = Some(character);
        editor.walk = None;
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let mut max_step = 0.0f32;
    let mut prev: Option<Vec2> = None;
    let mut climbed = false;
    for _ in 0..180 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        if let Some(p) = prev {
            max_step = max_step.max(character.pos.distance(p));
        }
        prev = Some(character.pos);
        if character.feet > 0.5 {
            climbed = true;
        }
    }

    assert!(
        max_step < 0.2,
        "a standing start must accelerate, not teleport onto the ramp, max single-frame step={max_step}"
    );
    assert!(
        climbed,
        "the capsule should still ride up the ramp once it reaches it"
    );
}

/// One ramp, no neighbours: spawn it alone so the climb itself is isolated.
fn ramp_only_app() -> App {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();
    app.update();
    app
}

fn place_character(app: &mut App, pos: Vec2, feet: f32, yaw: f32) {
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = yaw;
    let mut editor = app.world_mut().resource_mut::<Editor>();
    editor.character = Some(Character {
        pos,
        feet,
        yaw,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    });
    editor.walk = None;
}

/// Largest single-frame 3D displacement over frames that *start*
/// grounded. A grounded frame may never reposition the capsule by more
/// than the wish travel plus the terrain rise over that travel; anything
/// larger is a teleport or a depenetration fling, whatever its source.
fn max_grounded_frame_step(app: &mut App, frames: usize) -> (f32, f32) {
    let mut worst = 0.0f32;
    let mut peak = 0.0f32;
    let mut prev: Option<(Vec3, bool)> = None;
    for _ in 0..frames {
        tick_character(app);
        let c = app.world().resource::<Editor>().character.unwrap();
        let at = Vec3::new(c.pos.x, c.feet, c.pos.y);
        peak = peak.max(c.feet);
        if let Some((p, was_grounded)) = prev
            && was_grounded
        {
            worst = worst.max(at.distance(p));
        }
        prev = Some((at, c.grounded));
    }
    (worst, peak)
}

/// Sprint straight up a ramp: the capsule must stay grounded the whole
/// climb and every grounded frame must move by no more than the sprint
/// travel plus the slope rise over it — no entry pop, no mid-slope fling.
#[test]
fn sprinting_up_a_ramp_stays_grounded_and_never_pops() {
    let mut app = ramp_only_app();
    place_character(
        &mut app,
        Vec2::new(0.5, 0.0),
        0.0,
        -std::f32::consts::FRAC_PI_2,
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);

    // Stop counting at the ridge (x ~ 3.0); walking off it is a fall.
    let mut worst = 0.0f32;
    let mut peak = 0.0f32;
    let mut prev: Option<(Vec3, bool)> = None;
    for _ in 0..90 {
        tick_character(&mut app);
        let c = app.world().resource::<Editor>().character.unwrap();
        if c.pos.x > 3.0 {
            break;
        }
        assert!(
            c.grounded,
            "sprinting up a ramp must stay grounded, pos={:?} feet={} vy={}",
            c.pos, c.feet, c.vy
        );
        let at = Vec3::new(c.pos.x, c.feet, c.pos.y);
        peak = peak.max(c.feet);
        if let Some((p, was_grounded)) = prev
            && was_grounded
        {
            worst = worst.max(at.distance(p));
        }
        prev = Some((at, c.grounded));
    }
    assert!(peak > 0.8, "should sprint to the ramp top, peak={peak}");
    assert!(
        worst < 0.12,
        "grounded sprint frames must follow the slope, worst frame={worst}"
    );
}

/// Enter the same ramp at 30 degrees to its rise: the climb crosses the
/// slope diagonally, which used to catch the wedge's side features and
/// pop the capsule. Grounded frames still may not jump.
#[test]
fn entering_a_ramp_at_an_angle_does_not_pop() {
    let mut app = ramp_only_app();
    // Yaw giving wish direction (0.866, 0.5): 30 degrees off the rise.
    place_character(&mut app, Vec2::new(0.5, -0.4), 0.0, -2.094);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let (worst, peak) = max_grounded_frame_step(&mut app, 110);
    assert!(
        peak > 0.4,
        "the diagonal entry should still climb the slope, peak={peak}"
    );
    assert!(
        worst < 0.08,
        "grounded frames must follow the slope diagonally, worst frame={worst}"
    );
}

/// Stop halfway up a ramp, stand there, then walk on: standing must not
/// drift or sink, and the restart must not pop.
#[test]
fn stopping_and_restarting_on_a_ramp_does_not_drift_or_pop() {
    let mut app = ramp_only_app();
    place_character(
        &mut app,
        Vec2::new(0.5, 0.0),
        0.0,
        -std::f32::consts::FRAC_PI_2,
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    for _ in 0..35 {
        tick_character(&mut app);
    }
    let settled = app.world().resource::<Editor>().character.unwrap();
    assert!(
        (0.2..0.75).contains(&settled.feet),
        "should be parked mid-ramp, feet={}",
        settled.feet
    );

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyW);
    for _ in 0..30 {
        tick_character(&mut app);
    }
    let rested = app.world().resource::<Editor>().character.unwrap();
    assert!(
        (rested.pos - settled.pos).length() < 0.01 && (rested.feet - settled.feet).abs() < 0.01,
        "standing on a ramp must not drift, {:?} -> {:?}",
        settled.pos,
        rested.pos
    );
    assert!(rested.grounded, "standing on a ramp stays grounded");

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    let (worst, peak) = max_grounded_frame_step(&mut app, 90);
    assert!(peak > 0.8, "should climb on to the top, peak={peak}");
    assert!(
        worst < 0.08,
        "restarting on a ramp must not pop, worst frame={worst}"
    );
}

/// Repro: ramp top into the neighbouring block (the course map layout).
/// The capsule must walk off the ramp onto the block without launching into
/// the air or teleporting forward in a single frame.
#[test]
fn walking_up_a_ramp_steps_onto_the_next_block() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    let block = &shapes()[model::shape_index("block").unwrap()];
    // Course layout: ramp at hex (1,0) rising along +x, block at hex (2,0).
    let ramp_pos = model::hex_center(1, 0);
    let block_pos = model::hex_center(2, 0);
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(ramp_pos.x, 0.0, ramp_pos.y),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(block).unwrap(),
        Transform::from_xyz(block_pos.x, 0.0, block_pos.y),
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(ramp_pos.x - 1.5, ramp_pos.y),
            feet: 0.0,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let mut peak = 0.0f32;
    let mut max_step = 0.0f32;
    let mut prev: Option<Vec2> = None;
    for _ in 0..600 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        peak = peak.max(character.feet);
        if let Some(p) = prev {
            max_step = max_step.max(character.pos.distance(p));
        }
        prev = Some(character.pos);
        // Launch guard: the capsule should never leave the ground here.
        assert!(
            character.feet < 1.6,
            "capsule launched into the air, pos={:?} feet={} vy={}",
            character.pos,
            character.feet,
            character.vy,
        );
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        max_step < 0.2,
        "capsule teleported at the ramp/block border, max single-frame step={max_step}"
    );
    assert!(
        character.pos.x > block_pos.x + 1.0,
        "should cross the ramp top onto the block and walk off, pos={:?} feet={}",
        character.pos,
        character.feet
    );
    assert!(
        peak > 0.8 && peak < 1.3,
        "should ride up to the 0.9 block top without launching, peak={peak}"
    );
}

/// Full course walk: floor, ramp, block, floor (the default map layout) in
/// route-following mode. No launches, no single-frame pops at any seam.
#[test]
fn route_walk_crosses_the_full_course_without_pops() {
    let mut app = physics_app();
    let floor = &shapes()[model::shape_index("floor").unwrap()];
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    let block = &shapes()[model::shape_index("block").unwrap()];
    for (i, shape) in [floor, ramp, block, floor].into_iter().enumerate() {
        let pos = model::hex_center(i as i32, 0);
        let (y, rot) = if i == 1 {
            (0.0, Quat::from_rotation_y(-std::f32::consts::PI * 1.5))
        } else {
            (0.0, Quat::default())
        };
        app.world_mut().spawn((
            RigidBody::Fixed,
            piece_collider(shape).unwrap(),
            Transform {
                translation: Vec3::new(pos.x, y, pos.y),
                rotation: rot,
                ..default()
            },
        ));
    }
    app.update();
    app.update();

    let start = model::hex_center(0, 0);
    let end = model::hex_center(3, 0);
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: start,
            feet: 0.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.route = Some(Route {
            track: path::Track::new(vec![start, end]),
            trail: Vec::new(),
            line: Vec::new(),
            distances: vec![start.distance(end)],
            controls: vec![path::AnchorControls {
                incoming: None,
                outgoing: None,
            }],
            valid: true,
            hexes: 2,
        });
        editor.walk = Some(Walk {
            distance: 0.0,
            stuck: 0.0,
        });
    }

    let mut peak = 0.0f32;
    let mut max_step = 0.0f32;
    let mut prev: Option<Vec2> = None;
    for _ in 0..900 {
        tick_walk(&mut app);
        let editor = app.world().resource::<Editor>();
        if let Some(character) = editor.character {
            peak = peak.max(character.feet);
            if let Some(p) = prev {
                max_step = max_step.max(character.pos.distance(p));
            }
            prev = Some(character.pos);
            assert!(
                character.feet < 1.6,
                "route walker launched, pos={:?} feet={} vy={}",
                character.pos,
                character.feet,
                character.vy,
            );
        }
        if editor.walk.is_none() {
            break;
        }
    }
    let editor = app.world().resource::<Editor>();
    assert!(
        max_step < 0.2,
        "route walker teleported at a seam, max single-tick step={max_step}"
    );
    assert!(
        peak > 0.8 && peak < 1.3,
        "walker should ride up to the 0.9 block top, peak={peak}"
    );
}

/// Same border crossing one level up: block top into a ramp resting on it.
#[test]
fn walking_off_a_block_mounts_the_ramp_on_it() {
    let mut app = physics_app();
    let block = &shapes()[model::shape_index("block").unwrap()];
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    // Block top at y = 0.9, spanning x in [-0.9, 0.9].
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(block).unwrap(),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    // Ramp resting on the block, low edge flush with the block edge.
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(1.9, 0.9, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(-0.5, 0.0),
            feet: 0.9,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let mut peak = 0.0f32;
    for _ in 0..240 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        peak = peak.max(character.feet);
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x > 3.2,
        "should cross the block border, climb the ramp and walk off, pos={:?} feet={}",
        character.pos,
        character.feet
    );
    assert!(
        peak > 1.4,
        "should ride up the ramp slope on the way, peak={peak}"
    );
}

/// A ramp base sitting slightly above the floor (a small placement lip) must
/// still be mountable: the step-up assist walks over ledges within STEP_UP
/// instead of pinning the character at the border. Taller faces stay walls.
#[test]
fn a_small_lip_at_the_ramp_base_is_stepped_over() {
    for lift in [0.05, 0.15, 0.3] {
        let mut app = physics_app();
        let ramp = &shapes()[model::shape_index("ramp").unwrap()];
        app.world_mut().spawn((
            RigidBody::Fixed,
            piece_collider(ramp).unwrap(),
            Transform {
                translation: Vec3::new(2.0, lift, 0.0),
                rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
                ..default()
            },
        ));
        app.update();
        app.update();

        app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
        app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.character = Some(Character {
                pos: Vec2::new(0.5, 0.0),
                feet: 0.0,
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
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);

        let mut peak = 0.0f32;
        for _ in 0..240 {
            tick_character(&mut app);
            let character = app.world().resource::<Editor>().character.unwrap();
            peak = peak.max(character.feet);
        }

        let character = app.world().resource::<Editor>().character.unwrap();
        assert!(
            character.pos.x > 3.0,
            "lift={lift}: should step over the lip and cross the ramp, pos={:?} feet={}",
            character.pos,
            character.feet
        );
        assert!(
            peak > lift + 0.4,
            "lift={lift}: should ride up the slope after the step, peak={peak}"
        );
    }
}

/// A step onto a ledge is refused when the capsule would not fit there: a
/// lipped ramp backed directly by a block. The probe ray threads onto the
/// ramp slope, but the fat capsule at the destination overlaps the block
/// behind it. Stepping anyway would embed the capsule in solid rock and the
/// next move-and-slide would depenetrate it — a launch into the air followed
/// by a flicker between poses. Staying blocked is correct here.
#[test]
fn step_up_refuses_a_destination_inside_the_next_block() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    let block = &shapes()[model::shape_index("block").unwrap()];
    // Ramp lip at x = 1.0, floating 0.3 above the floor; block face at x = 1.2
    // so the step destination overlaps the block.
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.3, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(block).unwrap(),
        Transform::from_xyz(2.1, 0.0, 0.0),
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(0.5, 0.0),
            feet: 0.0,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let mut max_step = 0.0f32;
    let mut prev: Option<Vec2> = None;
    for _ in 0..240 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        if let Some(p) = prev {
            max_step = max_step.max(character.pos.distance(p));
        }
        prev = Some(character.pos);
        assert!(
            character.feet < 0.5,
            "capsule launched out of the crowded border, pos={:?} feet={} vy={}",
            character.pos,
            character.feet,
            character.vy,
        );
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        max_step < 0.2,
        "capsule teleported into the block behind the lip, max single-frame step={max_step}"
    );
    assert!(
        character.pos.x < 1.0,
        "the occupied step must stay blocked, pos={:?} feet={}",
        character.pos,
        character.feet
    );
}

/// Taller faces stay walls: a lip above STEP_UP cannot be stepped over.
#[test]
fn a_tall_lip_still_blocks() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.6, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(0.5, 0.0),
            feet: 0.0,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    for _ in 0..180 {
        tick_character(&mut app);
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x < 1.0,
        "a 0.6 lip must stay a wall, pos={:?} feet={}",
        character.pos,
        character.feet
    );
    assert!(
        character.feet < 0.05,
        "should not gain height off the wall, feet={}",
        character.feet
    );
}

/// Route following mounts a lipped ramp too: the walker shares the step-up.
#[test]
fn route_following_mounts_a_lipped_ramp() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.15, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(-0.5, 0.0),
            feet: 0.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.route = Some(Route {
            track: path::Track::new(vec![Vec2::new(-0.5, 0.0), Vec2::new(4.5, 0.0)]),
            trail: Vec::new(),
            line: Vec::new(),
            distances: vec![5.0],
            controls: vec![path::AnchorControls {
                incoming: None,
                outgoing: None,
            }],
            valid: true,
            hexes: 2,
        });
        editor.walk = Some(Walk {
            distance: 0.0,
            stuck: 0.0,
        });
    }

    let mut peak = 0.0f32;
    for _ in 0..300 {
        tick_walk(&mut app);
        let editor = app.world().resource::<Editor>();
        if let Some(character) = editor.character {
            peak = peak.max(character.feet);
        }
        if editor.walk.is_none() {
            break;
        }
    }

    let editor = app.world().resource::<Editor>();
    let character = editor.character.unwrap();
    assert!(
        editor.walk.is_none(),
        "the route should finish, note={}",
        editor.note
    );
    assert!(
        character.pos.x > 3.0,
        "should cross the lipped ramp on the route, pos={:?} feet={}",
        character.pos,
        character.feet
    );
    assert!(
        peak > 0.5,
        "should ride up the slope on the way, peak={peak}"
    );
}

#[test]
fn can_jump_while_climbing_a_ramp() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::First;
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(0.5, 0.0),
            feet: 0.0,
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
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    let mut on_ramp = false;
    for _ in 0..180 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        if (0.25..0.7).contains(&character.feet) {
            on_ramp = true;
            break;
        }
    }
    assert!(on_ramp, "character should climb the ramp");

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    tick_character(&mut app);

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.vy > 4.0,
        "jump should launch from the ramp, vy={}",
        character.vy
    );
}

#[test]
fn can_jump_while_descending_a_ramp() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Fixed,
        piece_collider(ramp).unwrap(),
        Transform {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(-std::f32::consts::PI * 1.5),
            ..default()
        },
    ));
    app.update();

    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::First;
    // Forward along -x, i.e. down the ramp.
    app.world_mut().resource_mut::<Orbit>().yaw = std::f32::consts::FRAC_PI_2;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::new(2.9, 0.0),
            feet: 0.85,
            yaw: std::f32::consts::FRAC_PI_2,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);

    let mut descending = false;
    for _ in 0..180 {
        tick_character(&mut app);
        let character = app.world().resource::<Editor>().character.unwrap();
        if (0.25..0.7).contains(&character.feet) {
            descending = true;
            break;
        }
    }
    assert!(descending, "character should walk down the ramp");

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    tick_character(&mut app);

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.vy > 4.0,
        "jump should launch while descending the ramp, vy={}",
        character.vy
    );
}

#[test]
fn clicking_empty_ground_selects_nothing() {
    let floor = model::shape_index("floor").unwrap();
    let pieces = vec![Piece {
        id: 1,
        shape: floor,
        pos: Vec2::ZERO,
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
    }];
    assert_eq!(pick(&pieces, ray_towards(Vec3::new(5.0, 0.0, 5.0))), None);
}

#[test]
fn a_session_captures_and_restores_pieces_camera_character_and_history() {
    let mut editor = Editor::default();
    editor.session = Some("map-1".into());
    editor.pieces = vec![test_piece(2), test_piece(4)];
    editor.character = Some(Character {
        pos: Vec2::new(1.5, -2.5),
        feet: 0.25,
        yaw: 0.5,
        vy: 3.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: false,
        sliding: false,
        health: crate::statuses::Health::default(),
    });
    editor.undo = vec![LevelState {
        pieces: vec![test_piece(1)],
        ..default()
    }];
    editor.redo = vec![LevelState {
        pieces: vec![test_piece(5)],
        ..default()
    }];
    editor.next_id = 6;
    let orbit = Orbit {
        target: Vec3::new(1.0, 2.0, 3.0),
        yaw: 0.4,
        pitch: 0.6,
        distance: 11.0,
        mode: ViewMode::Top,
    };

    let movers = Movers::default();
    let snapshot = editor.capture(&orbit, &movers);
    assert_eq!(snapshot.next_id, Some(6));
    assert_eq!(snapshot.pieces.len(), 2);

    let mut restored = Editor::default();
    let mut restored_orbit = Orbit::default();
    let mut restored_movers = Movers::default();
    restored.apply_loaded(
        &mut restored_orbit,
        &mut restored_movers,
        snapshot.validate().unwrap(),
    );

    assert_eq!(restored.pieces, vec![test_piece(2), test_piece(4)]);
    assert_eq!(restored.next_id, 6);
    assert_eq!(restored.undo[0].pieces, vec![test_piece(1)]);
    assert_eq!(restored.redo[0].pieces, vec![test_piece(5)]);
    let character = restored.character.unwrap();
    assert_eq!(character.pos, Vec2::new(1.5, -2.5));
    assert_eq!(character.feet, 0.25);
    assert!((character.yaw - 0.5).abs() < 1e-5);
    assert_eq!(character.vy, 0.0);
    assert!(!character.grounded);
    assert_eq!(restored_orbit.target, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(restored_orbit.mode, ViewMode::Top);
    assert_eq!(restored_orbit.distance, 11.0);
}

#[test]
fn push_impulse_matches_body_to_inward_speed() {
    // Head-on shove: body at rest, character walking into it at 2.8 m/s.
    let impulse =
        push_impulse(Vec3::X, Vec3::new(-2.8, 0.0, 0.0), Vec3::ZERO, 2.0).expect("should push");
    // touch_dir = -normal = -X; inward = 2.8; impulse = -X * 2.8 * 0.85 * 2.
    assert!(impulse.x < -4.0 && impulse.x > -5.5, "{impulse:?}");
    assert!(impulse.y.abs() < 1e-5 && impulse.z.abs() < 1e-5);

    // Separating or barely touching: no impulse, so resting contact jitters.
    assert!(push_impulse(Vec3::X, Vec3::ZERO, Vec3::ZERO, 2.0).is_none());
    assert!(push_impulse(Vec3::X, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, 2.0).is_none());
    assert!(push_impulse(Vec3::X, Vec3::new(-0.05, 0.0, 0.0), Vec3::ZERO, 2.0).is_none());

    // Degenerate inputs never push.
    assert!(push_impulse(Vec3::ZERO, Vec3::new(-2.0, 0.0, 0.0), Vec3::ZERO, 2.0).is_none());
    assert!(push_impulse(Vec3::X, Vec3::new(-2.0, 0.0, 0.0), Vec3::ZERO, 0.0).is_none());

    // A body already moving with the character needs no shove.
    assert!(
        push_impulse(
            Vec3::X,
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::new(-2.0, 0.0, 0.0),
            2.0
        )
        .is_none()
    );
}

#[test]
fn walking_into_a_dynamic_crate_shoves_it() {
    let mut app = physics_app();
    let prop = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.425, 0.425, 0.425),
            Velocity::zero(),
            ExternalImpulse::default(),
            Transform::from_xyz(1.2, 0.425, 0.0),
        ))
        .id();
    // Build the collider tree and computed mass before the first shove.
    app.update();
    app.update();

    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    // Forward along +x.
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    for _ in 0..40 {
        tick_character(&mut app);
    }

    // Impulses land in the solver on the next physics step; flush the
    // schedule so the shove shows up in the body's velocity.
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

#[test]
fn walking_into_a_static_wall_shoves_nothing() {
    let mut app = physics_app();
    let wall = app
        .world_mut()
        .spawn((
            RigidBody::Fixed,
            Collider::cuboid(0.1, 1.4, 0.6),
            Transform::from_xyz(1.2, 1.4, 0.0),
        ))
        .id();
    app.update();
    app.update();

    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);

    for _ in 0..40 {
        tick_character(&mut app);
    }

    // A static wall must not pick up speed; it still blocks the capsule.
    if let Some(velocity) = app.world().get::<Velocity>(wall) {
        assert!(
            velocity.linear.length() < 1e-4,
            "static wall should not move, got {:?}",
            velocity.linear
        );
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x < 0.9,
        "the wall should still block, pos={:?}",
        character.pos
    );
}

/// The reporter's level, verbatim: one floor, pill parked at its rim.
/// A floor piece with a motion path driving toward the pill.
fn user_level_app() -> (App, Vec2) {
    let floor_pos = Vec2::new(-1.273, -2.268);
    let char_pos = Vec2::new(-0.969, -0.973);
    let mut app = physics_app();
    app.world_mut().resource_mut::<Editor>().pieces.push(Piece {
        id: 1,
        shape: model::shape_index("floor").unwrap(),
        pos: floor_pos,
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
    });
    let dir = (char_pos - floor_pos).normalize_or_zero();
    let mut movers = Movers::default();
    let mover = movers.mover_mut(1, floor_pos);
    mover
        .waypoints
        .push(crate::platforms::PlatformWaypoint::new(
            floor_pos + dir * 4.0,
        ));
    mover.rebuild();
    app.insert_resource(movers);
    app.update();
    app.update();
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: char_pos,
            feet: 0.0,
            yaw: 91f32.to_radians(),
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    (app, dir)
}

/// Rim touch is not riding: while the floor slides toward the pill but has
/// not reached it, the pill must stand perfectly still — no carry, no shove,
/// no ride velocity. (Before the footprint check, the rim's upward contact
/// normal read as "supported" and dragged the pill from frame zero.)
#[test]
fn user_level_rim_touch_drags_nothing() {
    let (mut app, _) = user_level_app();
    let start = app.world().resource::<Editor>().character.unwrap().pos;
    for _ in 0..10 {
        tick_rider(&mut app);
        assert_platforms_clear(&app, "rim approach");
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.distance(start) < 1e-3,
        "the pill must stand still until the floor is actually under it, moved {:?} -> {:?}",
        start,
        character.pos,
    );
    assert_eq!(
        character.ride_vel,
        Vec2::ZERO,
        "no contact yet, so no ride velocity"
    );
}

/// Once the sliding floor is truly under the pill it picks it up and carries
/// it — legitimate riding (over the deck, feet on the surface), never a rim
/// drag. The floor travels 2.88 m in 120 ticks; the pill must end up riding
/// the deck, having travelled with it.
#[test]
fn user_level_floor_picks_up_only_when_aboard() {
    let (mut app, dir) = user_level_app();
    let start = app.world().resource::<Editor>().character.unwrap().pos;
    for _ in 0..120 {
        tick_rider(&mut app);
        assert_platforms_clear(&app, "slide-under");
    }
    let editor = app.world().resource::<Editor>();
    let character = editor.character.unwrap();
    let platform = editor.pieces.iter().find(|p| p.id == 1).unwrap();
    let travelled = (character.pos - start).dot(dir);
    assert!(
        travelled > 1.0,
        "the pill must ride once the floor is under it, travelled {travelled} along {dir:?}",
    );
    let shape = &shapes()[platform.shape];
    assert!(
        model::hit_test(shape, platform, character.pos, 0.05),
        "riding means over the deck footprint, not hanging off it: char={:?} floor={:?}",
        character.pos,
        platform.pos,
    );
    assert!(
        (character.feet - (platform.elevation + shape.top())).abs() < 0.05,
        "standing on the deck top, feet={} deck={}",
        character.feet,
        platform.elevation + shape.top(),
    );
}

/// A pill parked clear beside the track is never touched: the floor slides
/// past, the pill stays put, no ride velocity lingers.
#[test]
fn user_level_pill_beside_the_track_is_untouched() {
    let (mut app, dir) = user_level_app();
    let perp = Vec2::new(-dir.y, dir.x);
    let start = Vec2::new(-1.273, -2.268) + dir * 2.0 + perp * 1.6;
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let mut character = editor.character.unwrap();
        character.pos = start;
        editor.character = Some(character);
    }
    for _ in 0..120 {
        tick_rider(&mut app);
        assert_platforms_clear(&app, "beside track");
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.distance(start) < 0.05,
        "the floor must pass without dragging the pill, moved {:?} -> {:?}",
        start,
        character.pos,
    );
    assert_eq!(character.ride_vel, Vec2::ZERO);
}

/// A dynamic crate overlapping the idle capsule must shove the capsule out
/// of the way instead of treating it as an immovable wall. No physics step
/// needed: the sweep works on editor state through parry contacts, like the
/// platform push.
#[test]
fn dynamic_crate_overlapping_idle_character_shoves_it() {
    let mut app = physics_app();
    app.init_resource::<Movers>();
    app.init_resource::<Visuals>();
    let block = model::shape_index("block").unwrap();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
        // Block half-width 0.9, capsule radius 0.32: centred at x = -1.0
        // the block face intrudes 0.22 into the capsule's -x side, so the
        // shortest exit shoves the capsule +x, away from the crate.
        editor.pieces.push(Piece {
            id: 9,
            shape: block,
            pos: Vec2::new(-1.0, 0.0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: true,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        });
    }
    let prop = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            piece_collider(&shapes()[block]).unwrap(),
            Velocity::zero(),
            ExternalImpulse::default(),
            Transform::from_xyz(-1.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(9, prop);
    app.update();
    app.update();

    tick_rider(&mut app);

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x > 0.05,
        "the capsule must be shoved out of the crate, pos={:?}",
        character.pos
    );
    assert!(
        character.hv.x.abs() < 0.05,
        "a stationary crate imparts no knockback, hv={:?}",
        character.hv
    );
    // The frame must end clear: no residual penetration at rest.
    use bevy_rapier3d::parry::query::contact;
    let world = app.world();
    let editor = world.resource::<Editor>();
    let shape = &shapes()[block];
    let cap_iso = super::super::movers::iso(
        Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y),
        Quat::from_rotation_y(character.yaw),
    );
    let piece = &editor.pieces[0];
    let prop_iso = super::super::movers::iso(
        Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        model::piece_quat(piece),
    );
    let hit = contact(
        &cap_iso,
        world.resource::<Kit>().character_collider.as_shape(),
        &prop_iso,
        piece_collider(shape).unwrap().as_shape(),
        0.0,
    )
    .expect("contact query");
    assert!(
        hit.map_or(true, |hit| hit.dist >= -0.002),
        "capsule and crate must end clear of each other"
    );
}

/// A moving crate imparts part of its approach speed as knockback, mirroring
/// `push_impulse` with the roles swapped: the faster the crate, the harder
/// the shove, but it never overshoots the body's own speed.
#[test]
fn dynamic_crate_knockback_transfers_approach_speed() {
    let mut app = physics_app();
    app.init_resource::<Movers>();
    app.init_resource::<Visuals>();
    let block = model::shape_index("block").unwrap();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
        editor.pieces.push(Piece {
            id: 9,
            shape: block,
            pos: Vec2::new(-1.0, 0.0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: true,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        });
    }
    let prop = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            piece_collider(&shapes()[block]).unwrap(),
            Velocity {
                linear: Vec3::new(2.0, 0.0, 0.0),
                ..Velocity::zero()
            },
            ExternalImpulse::default(),
            Transform::from_xyz(-1.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(9, prop);

    tick_rider(&mut app);

    let character = app.world().resource::<Editor>().character.unwrap();
    // Approach speed 2.0 along +x, PUSH_FACTOR 0.85: hv lands at ~1.7.
    assert!(
        (character.hv.x - 1.7).abs() < 0.15,
        "knockback should carry most of the approach speed, hv={:?}",
        character.hv
    );
    assert!(character.pos.x > 0.05, "pos={:?}", character.pos);
}

/// The frame must end with the capsule clear of the given piece, like
/// `assert_platforms_clear` but for dynamic bodies shoved by anything.
fn assert_no_penetration(app: &App, piece_id: u32, context: &str) {
    use bevy_rapier3d::parry::query::contact;

    let world = app.world();
    let editor = world.resource::<Editor>();
    let character = editor.character.expect("character");
    let piece = editor
        .pieces
        .iter()
        .find(|p| p.id == piece_id)
        .expect("piece");
    let shape = &shapes()[piece.shape];
    let prop = piece_collider(shape).expect("collider");
    let cap_iso = super::super::movers::iso(
        Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y),
        Quat::from_rotation_y(character.yaw),
    );
    let prop_iso = super::super::movers::iso(
        Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        model::piece_quat(piece),
    );
    let hit = contact(
        &cap_iso,
        world.resource::<Kit>().character_collider.as_shape(),
        &prop_iso,
        prop.as_shape(),
        0.0,
    )
    .expect("contact query");
    if let Some(hit) = hit {
        assert!(
            -hit.dist <= 0.002,
            "{context}: penetration {:.3}: char={:?} piece={:?}",
            -hit.dist,
            character.pos,
            piece.pos,
        );
    }
}

/// Full chain from the report: a platform shuttles on its path while a
/// dynamic crate with real momentum barrels into the idle capsule, and the
/// capsule gets bulldozed instead of acting as an immovable wall.
/// (The platform teleports each frame, so Rapier only creeps a crate it
/// touches; the crate's velocity here stands in for a shove that already
/// happened — a faster platform, a fall, or a thrown prop. The platform
/// itself stays clear to prove tracked pieces are never double-shoved.)
#[test]
fn platform_shoves_crate_into_idle_character_shoves_character() {
    let mut app = physics_app();
    app.init_resource::<Movers>();
    app.init_resource::<Visuals>();
    app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
    let block = model::shape_index("block").unwrap();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        // Tracked platform driving -x, kept clear of the action so any
        // shove of the capsule can only arrive via the crate.
        editor.pieces.push(Piece {
            id: 7,
            shape: block,
            pos: Vec2::new(-6.0, 0.0),
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
        });
        // Dynamic crate launched at the idle capsule.
        editor.pieces.push(Piece {
            id: 8,
            shape: block,
            pos: Vec2::new(1.0, 0.0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: true,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        });
        editor.character = Some(Character {
            pos: Vec2::new(3.1, 0.0),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.walk = None;
    }
    {
        let mut movers = Movers::default();
        let mover = movers.mover_mut(7, Vec2::new(-6.0, 0.0));
        mover
            .waypoints
            .push(crate::platforms::PlatformWaypoint::new(Vec2::new(
                -2.0, 0.0,
            )));
        mover.rebuild();
        mover.speed = 1.5;
        app.insert_resource(movers);
    }
    let platform_entity = app
        .world_mut()
        .spawn((
            // Kinematic, like the live schedule now spawns tracked movers:
            // teleports carry velocity so the body can shove dynamics.
            RigidBody::KinematicPositionBased,
            piece_collider(&shapes()[block]).unwrap(),
            Transform::from_xyz(-6.0, 0.0, 0.0),
        ))
        .id();
    let crate_entity = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            piece_collider(&shapes()[block]).unwrap(),
            Friction::coefficient(0.0),
            Velocity {
                linear: Vec3::new(4.0, 0.0, 0.0),
                ..Velocity::zero()
            },
            ExternalImpulse::default(),
            Transform::from_xyz(1.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(7, platform_entity);
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(8, crate_entity);
    app.update();
    app.update();

    // Step real time per update so Rapier actually integrates motion
    // (the default harness clock barely advances between updates).
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(16),
    ));

    let start = app.world().resource::<Editor>().character.unwrap().pos.x;
    let mut max_hv: f32 = 0.0;
    for _ in 0..120 {
        tick_rider(&mut app);
        // The frame must always end clear of the crate.
        assert_no_penetration(&app, 8, "rolling crate");
        // Keep physics in step with the editor: the platform teleports by
        // editing its piece, so copy the pose to its body before stepping.
        let piece_pos = app
            .world()
            .resource::<Editor>()
            .pieces
            .iter()
            .find(|p| p.id == 7)
            .unwrap()
            .pos;
        app.world_mut()
            .get_mut::<Transform>(platform_entity)
            .unwrap()
            .translation = Vec3::new(piece_pos.x, 0.0, piece_pos.y);
        app.update();
        // Pull the simulated crate back into the editor, like
        // `sync_dynamic_pieces` does in the live schedule.
        let crate_pos = app
            .world()
            .get::<Transform>(crate_entity)
            .unwrap()
            .translation;
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            let piece = editor.pieces.iter_mut().find(|p| p.id == 8).unwrap();
            piece.pos = Vec2::new(crate_pos.x, crate_pos.z);
            piece.elevation = crate_pos.y;
        }
        max_hv = max_hv.max(app.world().resource::<Editor>().character.unwrap().hv.x);
    }

    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x - start > 1.0,
        "the crate must bulldoze the idle capsule down the track, start={start} end={:?}",
        character.pos
    );
    assert!(
        (1.0..=4.5).contains(&max_hv),
        "knockback should carry most of the 4 m/s approach without flinging, max_hv={max_hv}"
    );
}

/// A path follower ramming a stationary dynamic prop must shove it with
/// real momentum, not creep through it. Tracked movers are kinematic, so
/// Rapier derives velocity from the editor-authored teleports and the crate
/// picks up speed instead of just depenetrating in place.
#[test]
fn path_platform_rams_stationary_crate_with_momentum() {
    let mut app = physics_app();
    app.init_resource::<Movers>();
    app.init_resource::<Visuals>();
    let block = model::shape_index("block").unwrap();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(Piece {
            id: 7,
            shape: block,
            pos: Vec2::ZERO,
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
        });
        editor.pieces.push(Piece {
            id: 8,
            shape: block,
            pos: Vec2::new(3.0, 0.0),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: true,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            interact: crate::model::InteractAction::None,
        });
        editor.walk = None;
    }
    {
        let mut movers = Movers::default();
        let mover = movers.mover_mut(7, Vec2::ZERO);
        mover
            .waypoints
            .push(crate::platforms::PlatformWaypoint::new(Vec2::X * 8.0));
        mover.rebuild();
        mover.speed = 3.0;
        app.insert_resource(movers);
    }
    let platform_entity = app
        .world_mut()
        .spawn((
            RigidBody::KinematicPositionBased,
            piece_collider(&shapes()[block]).unwrap(),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    let crate_entity = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            piece_collider(&shapes()[block]).unwrap(),
            Velocity::zero(),
            ExternalImpulse::default(),
            Transform::from_xyz(3.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(7, platform_entity);
    app.world_mut()
        .resource_mut::<Visuals>()
        .map
        .insert(8, crate_entity);
    app.update();
    app.update();
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(16),
    ));

    let mut max_crate_vx: f32 = 0.0;
    for _ in 0..150 {
        tick_rider(&mut app);
        // The platform must never sit deep inside the crate: contact each
        // frame, never a tunnel-through overlap.
        assert_pieces_clear(&app, 7, 8, "platform rams crate");
        let piece_pos = app
            .world()
            .resource::<Editor>()
            .pieces
            .iter()
            .find(|p| p.id == 7)
            .unwrap()
            .pos;
        app.world_mut()
            .get_mut::<Transform>(platform_entity)
            .unwrap()
            .translation = Vec3::new(piece_pos.x, 0.0, piece_pos.y);
        app.update();
        let (crate_pos, crate_vel) = {
            let crate_transform = app
                .world()
                .get::<Transform>(crate_entity)
                .unwrap()
                .translation;
            let crate_vel = app
                .world()
                .get::<Velocity>(crate_entity)
                .map(|v| v.linear.x)
                .unwrap_or(0.0);
            (crate_transform, crate_vel)
        };
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            let piece = editor.pieces.iter_mut().find(|p| p.id == 8).unwrap();
            piece.pos = Vec2::new(crate_pos.x, crate_pos.z);
            piece.elevation = crate_pos.y;
        }
        max_crate_vx = max_crate_vx.max(crate_vel);
    }

    let editor = app.world().resource::<Editor>();
    let platform_x = editor.pieces.iter().find(|p| p.id == 7).unwrap().pos.x;
    let crate_x = editor.pieces.iter().find(|p| p.id == 8).unwrap().pos.x;
    assert!(
        max_crate_vx > 1.5,
        "the crate must pick up real speed from the ram, max_vx={max_crate_vx}"
    );
    assert!(
        crate_x - 3.0 > 1.0,
        "the crate must travel with the shove, crate_x={crate_x}"
    );
    assert!(
        crate_x > platform_x,
        "the crate must end up ahead of the platform, not tunnelled through: crate={crate_x} platform={platform_x}"
    );
}

/// Two editor pieces must end the frame clear of each other: contact at
/// most, never a deep overlap. Piece-vs-piece version of
/// `assert_no_penetration`.
fn assert_pieces_clear(app: &App, a_id: u32, b_id: u32, context: &str) {
    use bevy_rapier3d::parry::query::contact;

    let world = app.world();
    let editor = world.resource::<Editor>();
    let find = |id| editor.pieces.iter().find(|p| p.id == id).expect("piece");
    let a = find(a_id);
    let b = find(b_id);
    let shape_a = &shapes()[a.shape];
    let shape_b = &shapes()[b.shape];
    let Some(prop_a) = piece_collider(shape_a) else {
        return;
    };
    let Some(prop_b) = piece_collider(shape_b) else {
        return;
    };
    let iso_a = super::super::movers::iso(
        Vec3::new(a.pos.x, a.elevation, a.pos.y),
        model::piece_quat(a),
    );
    let iso_b = super::super::movers::iso(
        Vec3::new(b.pos.x, b.elevation, b.pos.y),
        model::piece_quat(b),
    );
    let hit =
        contact(&iso_a, prop_a.as_shape(), &iso_b, prop_b.as_shape(), 0.0).expect("contact query");
    if let Some(hit) = hit {
        assert!(
            -hit.dist <= 0.05,
            "{context}: deep overlap {:.3}: a={:?} b={:?}",
            -hit.dist,
            a.pos,
            b.pos,
        );
    }
}
