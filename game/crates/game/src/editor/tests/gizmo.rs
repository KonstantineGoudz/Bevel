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

#[test]
fn glade_gizmo_handles_resize_width_length_height_and_roof_pitch() {
    let room_idx = model::shape_index("glade_room").unwrap();
    let room_shape = &shapes()[room_idx];
    let start = Piece {
        id: 1,
        shape: room_idx,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::glade::default_size(room_shape),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
        trigger_color: crate::glade::GLADE_DEFAULT_ROOF_COLOR,
        trigger_visible: true,
        interact: crate::model::InteractAction::None,
    };

    let handles = glade_handle_visuals(&start, room_shape);
    assert_eq!(handles.len(), 6, "room has +X/-X width, +Z/-Z length, height, and roof pitch handles");

    // Drag +X width handle outward by 0.5m -> width grows by 1.0m (from 3.2 to 4.2)
    let widened = dragged_piece(
        start,
        GizmoHandle::GladeWidth,
        Vec3::new(1.6, 1.3, 0.0),
        Vec3::new(2.1, 1.3, 0.0),
    );
    assert!((widened.trigger_size.x - 4.2).abs() < 1e-4);

    // Drag +Z length handle outward by 0.4m -> length grows by 0.8m (from 2.4 to 3.2)
    let lengthened = dragged_piece(
        start,
        GizmoHandle::GladeLength,
        Vec3::new(0.0, 1.3, 1.2),
        Vec3::new(0.0, 1.3, 1.6),
    );
    assert!((lengthened.trigger_size.z - 3.2).abs() < 1e-4);

    // Drag height handle down by 1.8m -> height drops from 2.6m to 0.8m (low fence tier!)
    let lowered = dragged_piece(
        start,
        GizmoHandle::GladeHeight,
        Vec3::new(1.0, 2.6, 1.0),
        Vec3::new(1.0, 0.8, 1.0),
    );
    assert!((lowered.trigger_size.y - 0.8).abs() < 1e-4);
    assert_eq!(
        crate::glade::wall_tier(lowered.trigger_size.y),
        crate::glade::WallTier::LowFence
    );

    // Drag roof pitch handle down by 2.0m -> pitch drops to 0 (crenellated battlements!)
    let flattened = dragged_piece(
        start,
        GizmoHandle::GladeRoofPitch,
        Vec3::new(0.0, 3.8, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    assert_eq!(
        crate::glade::roof_style(&flattened, room_shape),
        crate::glade::GladeRoofStyle::Battlements
    );

    // Touch mode places Glade handles further outward and gives Glade doors a simplified 3-handle set.
    let touch_handles = glade_handle_visuals_for(&start, room_shape, true);
    assert!(
        touch_handles[0].1.x > handles[0].1.x + 0.15,
        "mobile touch handles sit further out from the wall for easy finger grabs"
    );

    let door_idx = model::shape_index("glade_door").unwrap();
    let door_shape = &shapes()[door_idx];
    let door_piece = Piece {
        shape: door_idx,
        trigger_size: crate::glade::default_size(door_shape),
        ..start
    };
    let door_handles = glade_handle_visuals_for(&door_piece, door_shape, true);
    assert_eq!(
        door_handles.len(),
        3,
        "glade_door shows only +X/-X width and top height handles"
    );
}

#[test]
fn glade_pieces_can_only_rotate_around_the_y_axis() {
    let room_idx = model::shape_index("glade_room").unwrap();
    let room_shape = &shapes()[room_idx];
    let start = Piece {
        id: 1,
        shape: room_idx,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::glade::default_size(room_shape),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
        trigger_color: crate::glade::GLADE_DEFAULT_ROOF_COLOR,
        trigger_visible: true,
        interact: crate::model::InteractAction::None,
    };
    let origin = gizmo_origin(&start, room_shape);

    // Pitch handle drag must NOT tilt Glade objects
    let pitch_drag = dragged_piece(
        start,
        GizmoHandle::Pitch,
        origin + Vec3::Y,
        origin + Vec3::Z,
    );
    assert_eq!(pitch_drag.pitch, 0.0, "Glade piece pitch must remain 0");
    assert_eq!(pitch_drag.roll, 0.0, "Glade piece roll must remain 0");
    assert_eq!(pitch_drag.rot, start.rot, "Glade piece rot must remain untouched on pitch drag");

    // Roll handle drag must NOT tilt Glade objects
    let roll_drag = dragged_piece(
        start,
        GizmoHandle::Roll,
        origin + Vec3::X,
        origin + Vec3::Y,
    );
    assert_eq!(roll_drag.pitch, 0.0, "Glade piece pitch must remain 0");
    assert_eq!(roll_drag.roll, 0.0, "Glade piece roll must remain 0");
    assert_eq!(roll_drag.rot, start.rot, "Glade piece rot must remain untouched on roll drag");

    // Yaw handle drag DOES rotate Glade objects around the Y axis
    let angle = 0.5_f32;
    let (sin, cos) = angle.sin_cos();
    let yaw_drag = dragged_piece(
        start,
        GizmoHandle::Yaw,
        origin + Vec3::X,
        origin + Vec3::X * cos + Vec3::Z * sin,
    );
    assert_eq!(yaw_drag.pitch, 0.0);
    assert_eq!(yaw_drag.roll, 0.0);
    assert!((yaw_drag.rot - angle).abs() < 1e-4, "Glade piece rotates freely on Y axis (yaw)");

    // set_piece_quat forces pitch and roll to 0 for Glade pieces
    let mut modified = start;
    model::set_piece_quat(&mut modified, Quat::from_euler(bevy::math::EulerRot::XYZ, 0.4, 0.8, -0.3));
    assert_eq!(modified.pitch, 0.0, "pitch clamped to 0");
    assert_eq!(modified.roll, 0.0, "roll clamped to 0");
    assert!(modified.rot.abs() > 0.01, "yaw preserved");
}

#[test]
fn glade_terrain_and_tent_houses_support_gizmo_reshaping_and_elevation_snapping() {
    let terrain_idx = model::shape_index("glade_terrain").unwrap();
    let terrain_shape = &shapes()[terrain_idx];
    let terrain = Piece {
        id: 1,
        shape: terrain_idx,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::glade::default_size(terrain_shape),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
        trigger_color: crate::glade::DEFAULT_TERRAIN_COLOR,
        trigger_visible: true,
        interact: crate::model::InteractAction::None,
    };

    // Terrain chunk exposes all 6 tactile handles (+X/-X width, +Z/-Z length, height, and falloff/cliff steepness)
    let handles = glade_handle_visuals(&terrain, terrain_shape);
    assert_eq!(handles.len(), 6);

    // Drag terrain height up to 15.0m (doubled height range!)
    let tall_terrain = dragged_piece(
        terrain,
        GizmoHandle::GladeHeight,
        Vec3::new(1.0, terrain.trigger_size.y, 1.0),
        Vec3::new(1.0, 15.0, 1.0),
    );
    assert!((tall_terrain.trigger_size.y - 15.0).abs() < 1e-4);

    // Drag room height down to 0.20m -> transforms into a tent-like house!
    let room_idx = model::shape_index("glade_room").unwrap();
    let room_shape = &shapes()[room_idx];
    let room = Piece {
        id: 2,
        shape: room_idx,
        trigger_size: crate::glade::default_size(room_shape),
        ..terrain
    };
    let tent_room = dragged_piece(
        room,
        GizmoHandle::GladeHeight,
        Vec3::new(1.0, 2.6, 1.0),
        Vec3::new(1.0, 0.20, 1.0),
    );
    assert!((tent_room.trigger_size.y - 0.20).abs() < 1e-4);
    assert!(crate::glade::is_tent_house(&tent_room, room_shape));

    // Snapping elevation of a piece over `glade_terrain` snaps cleanly onto the terrain plateau height
    let expected_h = crate::glade::terrain_height_at(Vec2::ZERO, &[terrain]);
    let snapped_y = snap_elevation(
        room,
        room_shape,
        &[terrain],
        expected_h - 0.08,
        1.0,
        true,
        ELEVATION_SNAP,
    );
    assert!(
        (snapped_y - expected_h).abs() < 1e-4,
        "piece snaps onto terrain plateau height {expected_h}, got {snapped_y}"
    );

    // Verify Rapier trimesh collider builds cleanly for `glade_terrain`
    let collider = piece_collider_for_glade(&terrain, terrain_shape, &[terrain], &[], None);
    assert!(collider.is_some(), "glade_terrain generates a walkable 3D collider");

    // Verify continuous terrain brush stroke (`apply_terrain_brush_dab` + `finish_terrain_stroke`)
    // extends a large-scale plateau across a drag and lifts buildings sitting on it!
    let mut editor = Editor::default();
    editor.pieces.push(Piece {
        id: 1,
        shape: room_idx,
        pos: Vec2::new(3.0, 0.0),
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::glade::default_size(room_shape),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
        trigger_color: crate::glade::GLADE_DEFAULT_ROOF_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    });
    editor.next_id = 2;

    assert!(apply_terrain_brush_dab(&mut editor, Vec2::new(0.0, 0.0), false));
    assert!(apply_terrain_brush_dab(&mut editor, Vec2::new(3.2, 0.0), false));
    finish_terrain_stroke(&mut editor);

    let room_after = editor.pieces.iter().find(|p| p.id == 1).unwrap();
    assert!(
        room_after.elevation > 1.8,
        "room sitting on sculpted terrain stroke is lifted onto the plateau (elevation = {})",
        room_after.elevation
    );
    assert_eq!(
        editor.undo.len(),
        1,
        "entire continuous terrain brush drag commits as a single undo step"
    );

    // Shift+drag (`lower = true`) carves into the built-in hillside
    let hill_pos = Vec2::new(-7.6, -6.8);
    let before_hill = crate::glade::world_terrain_height_at(hill_pos, &editor.pieces);
    assert!(apply_terrain_brush_dab(&mut editor, hill_pos, true));
    finish_terrain_stroke(&mut editor);
    let after_hill = crate::glade::world_terrain_height_at(hill_pos, &editor.pieces);
    assert!(
        after_hill < before_hill - 0.5,
        "lowering terrain brush carves into the built-in hillside ({before_hill} -> {after_hill})"
    );
}

#[test]
fn glade_gizmo_provides_4_move_arrows_and_2_corner_rotation_arcs() {
    use bevy::camera::{RenderTargetInfo, Viewport};

    let room_idx = model::shape_index("glade_room").unwrap();
    let room_shape = &shapes()[room_idx];
    let room = Piece {
        id: 1,
        shape: room_idx,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::glade::default_size(room_shape),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::glade::GLADE_DEFAULT_ROOF_PITCH,
        trigger_color: crate::glade::GLADE_DEFAULT_ROOF_COLOR,
        trigger_visible: true,
        interact: crate::model::InteractAction::None,
    };

    let arrows = glade_move_arrows(&room, room_shape, false);
    assert_eq!(arrows.len(), 4, "Glade building has 4 ground-plane move arrows");
    let arcs = glade_rotation_arcs(&room, room_shape, false);
    assert_eq!(arcs.len(), 2, "Glade building has 2 corner rotation arcs");

    let mut editor = Editor::default();
    editor.pieces.push(room);
    editor.selection.set(1);

    let mut camera = Camera::default();
    camera.viewport = Some(Viewport {
        physical_position: UVec2::ZERO,
        physical_size: UVec2::new(800, 600),
        depth: 0.0..1.0,
    });
    camera.computed.clip_from_view =
        Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 800.0 / 600.0, 0.1, 100.0);
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(800, 600),
        scale_factor: 1.0,
    });
    let cam_global = GlobalTransform::from(
        Transform::from_xyz(0.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    );

    for (base, tip) in &arrows {
        let mid = (*base + *tip) * 0.5;
        let screen = camera.world_to_viewport(&cam_global, mid).unwrap();
        let ray = camera.viewport_to_world(&cam_global, screen).unwrap();
        assert_eq!(
            gizmo_hit(&editor, &camera, &cam_global, screen, ray, false),
            Some(GizmoHandle::Xz),
            "clicking move arrow at {mid} hits GizmoHandle::Xz"
        );
    }

    for arc in &arcs {
        let mid = arc[arc.len() / 2];
        let screen = camera.world_to_viewport(&cam_global, mid).unwrap();
        let ray = camera.viewport_to_world(&cam_global, screen).unwrap();
        assert_eq!(
            gizmo_hit(&editor, &camera, &cam_global, screen, ray, false),
            Some(GizmoHandle::Yaw),
            "clicking corner arc at {mid} hits GizmoHandle::Yaw"
        );
    }
}
