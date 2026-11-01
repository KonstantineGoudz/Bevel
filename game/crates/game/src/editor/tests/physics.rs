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
        grounded: true,
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
            grounded: true,
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
            grounded: false,
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
fn can_jump_while_climbing_a_ramp() {
    let mut app = physics_app();
    let ramp = &shapes()[model::shape_index("ramp").unwrap()];
    app.world_mut().spawn((
        RigidBody::Static,
        piece_collider(ramp).unwrap(),
        Position::from_xyz(2.0, 0.0, 0.0),
        Rotation(Quat::from_rotation_y(-std::f32::consts::PI * 1.5)),
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
            grounded: true,
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
        RigidBody::Static,
        piece_collider(ramp).unwrap(),
        Position::from_xyz(2.0, 0.0, 0.0),
        Rotation(Quat::from_rotation_y(-std::f32::consts::PI * 1.5)),
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
            grounded: true,
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
        grounded: false,
        health: crate::statuses::Health::default(),
    });
    editor.undo = vec![vec![test_piece(1)]];
    editor.redo = vec![vec![test_piece(5)]];
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
    assert_eq!(restored.undo, vec![vec![test_piece(1)]]);
    assert_eq!(restored.redo, vec![vec![test_piece(5)]]);
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
            Collider::cuboid(0.85, 0.85, 0.85),
            Position::from_xyz(1.2, 0.425, 0.0),
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
            grounded: true,
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

    let velocity = app
        .world()
        .get::<LinearVelocity>(prop)
        .expect("dynamic prop keeps its velocity")
        .0;
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
            RigidBody::Static,
            Collider::cuboid(0.2, 2.8, 1.2),
            Position::from_xyz(1.2, 1.4, 0.0),
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
            grounded: true,
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
    if let Some(velocity) = app.world().get::<LinearVelocity>(wall) {
        assert!(
            velocity.0.length() < 1e-4,
            "static wall should not move, got {:?}",
            velocity.0
        );
    }
    let character = app.world().resource::<Editor>().character.unwrap();
    assert!(
        character.pos.x < 0.9,
        "the wall should still block, pos={:?}",
        character.pos
    );
}
