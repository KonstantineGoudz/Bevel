use super::*;

#[test]
fn gizmo_grab_allowed_needs_build_select_and_no_tool() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selection.set(1);
    assert!(gizmo_grab_allowed(&editor, false));
    editor.actor_tool = Some(ActorTool::Place);
    assert!(!gizmo_grab_allowed(&editor, false));
    editor.actor_tool = None;
    assert!(
        !gizmo_grab_allowed(&editor, true),
        "an in-flight handle drag owns the pointer"
    );
    editor.selection.clear();
    assert!(!gizmo_grab_allowed(&editor, false));
    editor.selection.set(1);
    editor.mode = Mode::Play;
    assert!(!gizmo_grab_allowed(&editor, false));
}

#[test]
fn release_gizmo_drag_clears_and_suppresses_the_tap() {
    let mut editor = Editor::default();
    let mut touch = TouchState::default();
    // A foreign pointer must not disturb the drag.
    release_gizmo_drag(&mut editor, &mut touch, PointerId::Touch(9), 1.0);
    assert!(editor.gizmo_drag.is_none());

    let start = test_piece(1);
    editor.gizmo_drag = Some(GizmoDrag {
        id: 1,
        handle: GizmoHandle::Y,
        grab: Vec3::ZERO,
        start,
        others: Vec::new(),
        last_raw: 0.0,
        last: 0.0,
        pointer: PointerId::Touch(3),
        screen: Vec2::ZERO,
    });
    release_gizmo_drag(&mut editor, &mut touch, PointerId::Touch(3), 2.0);
    assert!(editor.gizmo_drag.is_none());
    assert!(editor.hard_dirty);
    assert!(touch.tap.is_none());
    assert!(touch.suppress_until >= 2.0);
}

#[test]
fn a_gizmo_handle_wins_over_an_armed_shape_tool() {
    let mut editor = Editor {
        tool: Some(0),
        ..Default::default()
    };
    assert!(
        gizmo_takes_pointer(&editor, false),
        "a gizmo handle must be grabbable while placing shapes"
    );
    editor.actor_tool = Some(ActorTool::Place);
    assert!(!gizmo_takes_pointer(&editor, false));
    editor.actor_tool = Some(ActorTool::Path);
    assert!(!gizmo_takes_pointer(&editor, false));
}

#[test]
fn a_waypoint_handle_has_a_fingertip_target() {
    assert!(within_radius(Vec2::ZERO, Vec2::new(20.0, 0.0), 24.0));
    assert!(!within_radius(Vec2::ZERO, Vec2::new(25.0, 0.0), 24.0));
}

#[test]
fn a_wall_face_is_clickable_at_mid_height() {
    let wall = model::shape_index("wall").unwrap();
    let floor = model::shape_index("floor").unwrap();
    let pieces = vec![
        Piece {
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
        },
        Piece {
            id: 2,
            shape: wall,
            pos: Vec2::new(0.519, -0.9),
            rot: std::f32::consts::FRAC_PI_2,
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
        },
    ];
    let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 1.4, -0.9)));
    assert_eq!(
        hit,
        Some(2),
        "clicking the wall face should pick the wall, not the floor under it"
    );
}

#[test]
fn a_wall_is_clickable_near_its_top_too() {
    let wall = model::shape_index("wall").unwrap();
    let pieces = vec![Piece {
        id: 2,
        shape: wall,
        pos: Vec2::new(0.519, -0.9),
        rot: std::f32::consts::FRAC_PI_2,
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
    let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 2.6, -0.9)));
    assert_eq!(hit, Some(2), "the upper face of a wall should be clickable");
}

#[test]
fn the_floor_is_clickable() {
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
    assert_eq!(
        pick(&pieces, ray_towards(Vec3::new(0.2, 0.0, 0.2))),
        Some(1)
    );
}

#[test]
fn the_gizmo_is_bigger_than_the_selected_pieces_bounding_box() {
    for (shape_index, shape) in shapes().iter().enumerate() {
        let piece = Piece {
            id: 0,
            shape: shape_index,
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
        let (reach, ring) = gizmo_frame(&piece, shape);
        let size = bounding_size(&piece, shape);
        assert!(
            reach > size,
            "{}: gizmo handle reach {reach} should be bigger than its bounding box {size}",
            shape.id
        );
        assert!(
            ring > size * 0.5,
            "{}: gizmo yaw ring {ring} should clear half its bounding box {size}",
            shape.id
        );
    }
}

#[test]
fn gizmo_axis_projection_and_screen_distance() {
    // A ray straight down at x = 2 hits the X axis at s = 2.
    let ray = Ray3d::new(Vec3::new(2.0, 5.0, 0.0), Dir3::NEG_Y);
    let s = ray_axis_param(ray, Vec3::ZERO, Vec3::X).unwrap();
    assert!((s - 2.0).abs() < 1e-4, "s = {s}");

    // And at z = -1.5 for the Z axis.
    let ray = Ray3d::new(Vec3::new(0.0, 5.0, -1.5), Dir3::NEG_Y);
    let s = ray_axis_param(ray, Vec3::ZERO, Vec3::Z).unwrap();
    assert!((s + 1.5).abs() < 1e-4, "s = {s}");

    let d = point_segment_distance(Vec2::new(5.0, 5.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
    assert!((d - 5.0).abs() < 1e-4);
    let d = point_segment_distance(Vec2::new(-3.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
    assert!((d - 3.0).abs() < 1e-4);
}

#[test]
fn an_axis_drag_moves_by_the_pointer_delta_not_to_the_pointer() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::new(1.0, 2.0),
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
    // Grabbed far out on the handle, then moved a little. The piece must
    // follow the little movement, not leap out to the pointer.
    let moved = dragged_piece(
        start,
        GizmoHandle::X,
        Vec3::new(3.0, 0.5, 2.0),
        Vec3::new(3.4, 0.5, 2.0),
    );
    assert!((moved.pos.x - 1.4).abs() < 1e-5, "pos = {:?}", moved.pos);
    assert!((moved.pos.y - 2.0).abs() < 1e-5, "pos = {:?}", moved.pos);
    assert!((moved.elevation - 0.0).abs() < 1e-5);
}

#[test]
fn a_negative_axis_drag_moves_by_the_pointer_delta() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::new(1.0, 2.0),
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
    // Grabbed on the negative X end and shoved further negative: the piece
    // follows the pointer travel, not the grab position.
    let moved = dragged_piece(
        start,
        GizmoHandle::X,
        Vec3::new(-3.0, 0.5, 2.0),
        Vec3::new(-3.4, 0.5, 2.0),
    );
    assert!((moved.pos.x - 0.6).abs() < 1e-5, "pos = {:?}", moved.pos);
    assert!((moved.pos.y - 2.0).abs() < 1e-5, "pos = {:?}", moved.pos);
    assert!((moved.elevation - 0.0).abs() < 1e-5);
}

#[test]
fn the_vertical_handle_lifts_and_lowers_a_piece() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::new(1.0, 2.0),
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.5,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    };
    // Grabbed at the top of the vertical handle, not at the piece origin.
    let moved = dragged_piece(
        start,
        GizmoHandle::Y,
        Vec3::new(1.0, 2.0, 2.0),
        Vec3::new(1.0, 2.75, 2.0),
    );
    assert!((moved.elevation - 1.25).abs() < 1e-5, "{}", moved.elevation);
    assert_eq!(moved.pos, start.pos);
}

#[test]
fn grabbing_the_vertical_handle_without_dragging_keeps_the_height() {
    let block = model::shape_index("block").unwrap();
    let start = Piece {
        id: 1,
        shape: block,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        // Free-placed just above the ground, where a snap would move it.
        elevation: 0.1,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    };
    let mut editor = Editor::default();
    editor.pieces.push(start);
    let origin = gizmo_origin(&start, &shapes()[block]);
    let grab = |target: Vec3| {
        let eye = target + Vec3::new(3.0, 4.0, 0.0);
        Ray3d::new(eye, Dir3::new(target - eye).unwrap())
    };
    let drag = GizmoDrag {
        id: start.id,
        handle: GizmoHandle::Y,
        grab: origin,
        start,
        others: Vec::new(),
        last_raw: start.elevation,
        last: start.elevation,
        pointer: PointerId::Mouse,
        screen: Vec2::ZERO,
    };

    // A click with no pointer travel must not change the height.
    apply_gizmo_drag(&mut editor, drag, Some(grab(origin)));
    assert!(
        (editor.pieces[0].elevation - start.elevation).abs() < 1e-5,
        "elevation = {}",
        editor.pieces[0].elevation
    );

    // Dragging down onto the ground does snap.
    let drag = editor.gizmo_drag.clone().expect("drag");
    apply_gizmo_drag(&mut editor, drag, Some(grab(origin - Vec3::Y * 0.1)));
    assert!(editor.pieces[0].elevation.abs() < 1e-5);
}

#[test]
fn a_yaw_drag_keeps_the_grab_offset() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::ZERO,
        rot: 0.3,
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
    // Grabbed on the +X side of the ring and dragged a quarter turn to +Z.
    let moved = dragged_piece(
        start,
        GizmoHandle::Yaw,
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
    );
    let expected = model::normalize_angle(0.3 + std::f32::consts::FRAC_PI_2);
    assert!((moved.rot - expected).abs() < 1e-5, "rot = {}", moved.rot);
}

#[test]
fn a_pitch_drag_tilts_around_the_world_x_axis() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::ZERO,
        rot: 0.3,
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
    let origin = gizmo_origin(&start, &shapes()[start.shape]);
    let angle = 0.6_f32;
    let (sin, cos) = angle.sin_cos();
    let moved = dragged_piece(
        start,
        GizmoHandle::Pitch,
        origin + Vec3::Y,
        origin + Vec3::Y * cos + Vec3::Z * sin,
    );
    let expected = Quat::from_rotation_x(angle) * model::piece_quat(&start);
    assert!(
        model::piece_quat(&moved).dot(expected).abs() > 0.99999,
        "orientation = {:?}",
        model::piece_quat(&moved)
    );
    assert_eq!(moved.pos, start.pos);
    assert_eq!(moved.elevation, start.elevation);
}

#[test]
fn a_roll_drag_tilts_around_the_world_z_axis() {
    let start = Piece {
        id: 1,
        shape: 0,
        pos: Vec2::ZERO,
        rot: 0.3,
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
    let origin = gizmo_origin(&start, &shapes()[start.shape]);
    let angle = -0.45_f32;
    let (sin, cos) = angle.sin_cos();
    let moved = dragged_piece(
        start,
        GizmoHandle::Roll,
        origin + Vec3::X,
        origin + Vec3::X * cos + Vec3::Y * sin,
    );
    let expected = Quat::from_rotation_z(angle) * model::piece_quat(&start);
    assert!(
        model::piece_quat(&moved).dot(expected).abs() > 0.99999,
        "orientation = {:?}",
        model::piece_quat(&moved)
    );
    assert_eq!(moved.pos, start.pos);
    assert_eq!(moved.elevation, start.elevation);
}

#[test]
fn a_plane_handle_moves_in_two_axes_at_once() {
    let start = Piece {
        id: 1,
        shape: 0,
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
    let moved = dragged_piece(
        start,
        GizmoHandle::Xy,
        Vec3::new(1.0, 0.5, 0.0),
        Vec3::new(1.4, 1.1, 0.0),
    );
    assert!((moved.pos.x - 0.4).abs() < 1e-5);
    assert!((moved.pos.y - 0.0).abs() < 1e-5);
    assert!((moved.elevation - 0.6).abs() < 1e-5);
}

#[test]
fn height_snapping_stacks_a_block_on_the_one_below() {
    let block = model::shape_index("block").unwrap();
    let below = Piece {
        id: 1,
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
    };
    let moving = Piece {
        id: 2,
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
    };
    let top = shapes()[block].top();
    let snapped = snap_elevation(
        moving,
        &shapes()[block],
        &[below],
        top - 0.05,
        1.0,
        true,
        ELEVATION_SNAP,
    );
    assert!((snapped - top).abs() < 1e-5, "snapped = {snapped}");
}

#[test]
fn height_snapping_leaves_free_heights_alone() {
    let crate_index = model::shape_index("crate").unwrap();
    let piece = Piece {
        id: 1,
        shape: crate_index,
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
    let shape = &shapes()[crate_index];
    let free = snap_elevation(piece, shape, &[], 0.62, 1.0, true, ELEVATION_SNAP);
    assert!((free - 0.62).abs() < 1e-5, "free = {free}");
    let unsnapped = snap_elevation(piece, shape, &[], 0.62, 1.0, false, ELEVATION_SNAP);
    assert!((unsnapped - 0.62).abs() < 1e-5, "unsnapped = {unsnapped}");
}

#[test]
fn height_snapping_releases_a_stacked_piece_immediately() {
    let block = model::shape_index("block").unwrap();
    let below = Piece {
        id: 1,
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
    };
    let moving = Piece {
        id: 2,
        shape: block,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: shapes()[block].top(),
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    };
    let top = shapes()[block].top();
    let lifted = snap_elevation(
        moving,
        &shapes()[block],
        &[below],
        top + 0.01,
        1.0,
        true,
        ELEVATION_SNAP,
    );
    assert!((lifted - (top + 0.01)).abs() < 1e-5, "lifted = {lifted}");
}

#[test]
fn height_snapping_only_captures_surfaces_ahead_of_the_drag() {
    let crate_index = model::shape_index("crate").unwrap();
    let piece = Piece {
        id: 1,
        shape: crate_index,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.05,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    };
    let shape = &shapes()[crate_index];
    // Lifting away from the ground is free: it must not be pulled back down.
    let lifted = snap_elevation(piece, shape, &[], 0.05, 1.0, true, ELEVATION_SNAP);
    assert!((lifted - 0.05).abs() < 1e-5, "lifted = {lifted}");
    // Lowering toward the same ground still snaps.
    let lowered = snap_elevation(piece, shape, &[], 0.05, -1.0, true, ELEVATION_SNAP);
    assert!(lowered.abs() < 1e-5, "lowered = {lowered}");
}

#[test]
fn the_gizmo_offers_a_vertical_and_three_plane_handles() {
    let block = model::shape_index("block").unwrap();
    let piece = Piece {
        id: 1,
        shape: block,
        pos: Vec2::new(1.0, 2.0),
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.75,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    };
    let shape = &shapes()[block];
    let origin = gizmo_origin(&piece, shape);
    let (reach, _) = gizmo_frame(&piece, shape);
    let points = gizmo_handle_points(&piece, shape);

    let find = |handle| {
        points
            .iter()
            .find(|(h, _)| *h == handle)
            .map(|(_, point)| *point)
    };
    let y = find(GizmoHandle::Y).expect("vertical handle");
    assert!((y - (origin + Vec3::Y * reach)).length() < 1e-5);

    let plane = |a: Vec3, b: Vec3| origin + (a + b) * reach * GIZMO_PLANE_CENTER;
    assert!((find(GizmoHandle::Xy).expect("xy") - plane(Vec3::X, Vec3::Y)).length() < 1e-5);
    assert!((find(GizmoHandle::Zy).expect("zy") - plane(Vec3::Z, Vec3::Y)).length() < 1e-5);
    assert!((find(GizmoHandle::Xz).expect("xz") - plane(Vec3::X, Vec3::Z)).length() < 1e-5);
}

#[test]
fn a_plane_handle_needs_the_pointer_inside_its_square() {
    let square = [
        Vec2::new(0.0, 0.0),
        Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 10.0),
        Vec2::new(0.0, 10.0),
    ];
    assert!(point_in_quad(Vec2::new(5.0, 5.0), &square));
    assert!(!point_in_quad(Vec2::new(15.0, 5.0), &square));
}

#[test]
fn an_edge_on_plane_handle_does_not_steal_the_click() {
    // A vertical plane seen from straight above collapses to a sliver.
    let sliver = [
        Vec2::new(0.0, 0.0),
        Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 1.0),
        Vec2::new(0.0, 1.0),
    ];
    assert!(!plane_handle_hit(Vec2::new(5.0, 0.5), &sliver));

    let square = [
        Vec2::new(0.0, 0.0),
        Vec2::new(10.0, 0.0),
        Vec2::new(10.0, 10.0),
        Vec2::new(0.0, 10.0),
    ];
    assert!(plane_handle_hit(Vec2::new(5.0, 5.0), &square));
}

#[test]
fn the_build_orbit_target_is_the_selected_piece_centre() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.pieces.push(Piece {
        id: 7,
        shape: block,
        pos: Vec2::new(2.0, 3.0),
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 1.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    });
    editor.selection.set(7);
    let target = selection_center(&editor).expect("selected centre");
    let shape = &shapes()[block];
    assert!((target.x - 2.0).abs() < 1e-5);
    assert!((target.z - 3.0).abs() < 1e-5);
    assert!((target.y - (1.0 + (shape.base + shape.top()) * 0.5)).abs() < 1e-5);

    editor.selection.clear();
    assert_eq!(selection_center(&editor), None);
}

#[test]
fn actor_facing_matches_the_movement_convention() {
    // Yaw zero faces -z, matching `forward` in the movement code; yaw -90
    // degrees faces +x, the sprint-jump direction in the physics tests.
    assert!((actor_facing(0.0) - Vec2::new(0.0, -1.0)).length() < 1e-6);
    assert!((actor_facing(-std::f32::consts::FRAC_PI_2) - Vec2::X).length() < 1e-6);
    assert!((actor_facing(std::f32::consts::PI) - Vec2::Y).length() < 1e-6);
    assert!((actor_facing(1.0).length() - 1.0).abs() < 1e-6);
}

#[test]
fn actor_arrow_runs_outward_from_the_ring_on_the_ground() {
    let pos = Vec2::new(2.0, 3.0);
    let (start, tip, head) = actor_arrow_geometry(pos, 0.5, -std::f32::consts::FRAC_PI_2);
    let base = Vec3::new(2.0, 0.55, 3.0);
    let dir = Vec3::X;
    // Shaft starts at the ring edge and ends at the tip, both on the ground.
    assert!((start - (base + dir * ACTOR_ARROW_INNER)).length() < 1e-5);
    assert!((tip - (base + dir * ACTOR_ARROW_TIP)).length() < 1e-5);
    assert!((start.y - 0.55).abs() < 1e-6 && (tip.y - 0.55).abs() < 1e-6);
    // Diamond head: the tip is one vertex, the other two sit symmetric
    // about the shaft, one head-length back and a half-width apart.
    assert!((head[1] - tip).length() < 1e-6);
    let back = (head[0] + head[2]) * 0.5;
    assert!((back - (tip - dir * ACTOR_ARROW_HEAD_BACK)).length() < 1e-5);
    assert!(((head[0] - head[2]).length() - 2.0 * ACTOR_ARROW_HEAD_SIDE).abs() < 1e-5);
    let across = (head[0] - back).normalize_or_zero();
    assert!(across.dot(dir).abs() < 1e-5, "head spans across the shaft");
}

#[test]
fn a_gizmo_drag_carries_the_rest_of_the_selection() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.pieces.push(Piece {
        pos: Vec2::new(1.0, 0.0),
        ..test_piece(2)
    });
    editor.selection.set_all([1, 2]);

    let piece = editor.pieces[0];
    let origin = gizmo_origin(&piece, &shapes()[block]);
    let aim = |target: Vec3| {
        let eye = target + Vec3::new(3.0, 4.0, 0.0);
        Ray3d::new(eye, Dir3::new(target - eye).unwrap())
    };
    let grab = gizmo_drag_point(aim(origin), GizmoHandle::X, origin).unwrap();
    let drag = GizmoDrag {
        id: 1,
        handle: GizmoHandle::X,
        grab,
        start: piece,
        others: dragged_group(&editor, 1),
        last_raw: piece.elevation,
        last: piece.elevation,
        pointer: PointerId::Mouse,
        screen: Vec2::ZERO,
    };

    let target = origin + Vec3::X * 0.75;
    apply_gizmo_drag(&mut editor, drag, Some(aim(target)));

    let primary = editor.pieces[0];
    let other = editor.pieces[1];
    assert!(
        (primary.pos.x - 0.75).abs() < 1e-3,
        "the grabbed piece followed the handle: x = {}",
        primary.pos.x
    );
    assert!(
        (other.pos.x - 1.75).abs() < 1e-3,
        "the other selected piece took the same delta: x = {}",
        other.pos.x
    );
    assert!(other.elevation.abs() < 1e-6, "a ground drag does not lift");
}
