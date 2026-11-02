use super::*;

fn keys_with(code: KeyCode) -> ButtonInput<KeyCode> {
    let mut keys = ButtonInput::default();
    keys.press(code);
    keys
}

fn cursor_looking_at(eye: Vec3, target: Vec3) -> Cursor {
    Cursor {
        ray: Some(Ray3d::new(eye, Dir3::new(target - eye).unwrap())),
        screen: Some(Vec2::ZERO),
        world: Some(Vec2::new(target.x, target.z)),
        ..Default::default()
    }
}

#[test]
fn selecting_a_piece_without_moving_keeps_its_position() {
    let wall = model::shape_index("wall").unwrap();
    let anchor = Piece {
        id: 1,
        shape: wall,
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
    // Within anchor range, but not yet snapped: a bare selection must not
    // pull it into place.
    let start_pos = Vec2::new(model::edge_len() - 0.05, 0.02);
    let piece = Piece {
        id: 2,
        shape: wall,
        pos: start_pos,
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
    let mut editor = Editor::default();
    editor.pieces.push(anchor);
    editor.pieces.push(piece);
    assert!(editor.snap, "snap is on by default");
    let drag = Drag {
        id: 2,
        offset: Vec2::ZERO,
        plane: 0.0,
        moved: false,
    };
    editor.drag = Some(drag);

    // No pointer travel: base + offset is the piece's current position.
    apply_body_drag(&mut editor, drag, start_pos);
    let kept = editor.pieces.iter().find(|p| p.id == 2).unwrap();
    assert!(
        (kept.pos - start_pos).length_squared() < 1e-10,
        "selecting snapped the piece to {:?}",
        kept.pos
    );
    assert!(
        !editor.drag.expect("drag").moved,
        "a pure selection must not count as a move"
    );

    // Actually dragging does move (and snap) the piece.
    apply_body_drag(&mut editor, drag, start_pos + Vec2::new(0.05, 0.0));
    let moved_piece = editor.pieces.iter().find(|p| p.id == 2).unwrap();
    assert!(
        (moved_piece.pos - start_pos).length_squared() > 1e-10,
        "a real drag should move the piece"
    );
    assert!(
        editor.drag.expect("drag").moved,
        "a real drag should flag the move"
    );
}

#[test]
fn g_then_x_locks_to_world_x_and_enter_confirms() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selected = Some(1);
    let mouse = ButtonInput::default();
    let mut cursor = cursor_looking_at(Vec3::new(1.0, 5.0, 0.0), Vec3::new(1.0, 0.0, 0.0));

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyG),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "move XZ");
    assert!(editor.keyboard_transform.is_some());

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyX),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "move X");
    assert_eq!(
        editor.keyboard_transform.unwrap().axis,
        Some(TransformAxis::X)
    );

    cursor = cursor_looking_at(Vec3::new(2.0, 5.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &ButtonInput::default(),
        &mouse,
        &cursor,
        true,
        false
    ));
    let piece = editor.pieces[0];
    assert!((piece.pos.x - 1.0).abs() < 1e-5, "x = {}", piece.pos.x);
    assert!(piece.pos.y.abs() < 1e-5, "z = {}", piece.pos.y);
    assert!(piece.elevation.abs() < 1e-5);

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::Enter),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert!(editor.keyboard_transform.is_none());
    assert_eq!(editor.note, "moved");
    assert!(editor.hard_dirty);
}

#[test]
fn escape_restores_the_piece_and_removes_the_undo_step() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selected = Some(1);
    let mouse = ButtonInput::default();
    let start = cursor_looking_at(Vec3::new(1.0, 5.0, 0.0), Vec3::new(1.0, 0.0, 0.0));

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyG),
        &mouse,
        &start,
        true,
        false
    ));
    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &ButtonInput::default(),
        &mouse,
        &cursor_looking_at(Vec3::new(2.0, 5.0, 0.0), Vec3::new(2.0, 0.0, 0.0)),
        true,
        false
    ));
    assert!(editor.pieces[0].pos.x.abs() > 0.5);

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::Escape),
        &mouse,
        &start,
        true,
        false
    ));
    assert_eq!(editor.pieces[0], test_piece(1));
    assert!(editor.undo.is_empty(), "cancel must remove its undo step");
    assert!(editor.keyboard_transform.is_none());
    assert_eq!(editor.note, "cancelled");
}

#[test]
fn r_opens_rotate_and_axis_keys_toggle_locks() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selected = Some(1);
    let mouse = ButtonInput::default();
    let cursor = cursor_looking_at(Vec3::new(1.0, 5.0, 1.0), Vec3::ZERO);

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyR),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "rotate Y");

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyX),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "rotate X");
    assert_eq!(
        editor.keyboard_transform.unwrap().axis,
        Some(TransformAxis::X)
    );

    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyZ),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "rotate Z");

    // Pressing the same axis again drops the constraint; rotations fall back
    // to yaw.
    assert!(keyboard_transform(
        &mut editor,
        &Controls::with_defaults(),
        &keys_with(KeyCode::KeyZ),
        &mouse,
        &cursor,
        true,
        false
    ));
    assert_eq!(editor.note, "rotate Y");
    assert_eq!(editor.keyboard_transform.unwrap().axis, None);
}

#[test]
fn keyboard_moves_and_rotations_respect_the_locked_axis() {
    let base = test_piece(1);
    let moved = moved_piece(
        base,
        TransformMode::Move,
        Some(TransformAxis::X),
        Vec3::ZERO,
        Vec3::new(2.0, 0.0, 3.0),
    );
    assert!((moved.pos.x - 2.0).abs() < 1e-5);
    assert!(moved.pos.y.abs() < 1e-5);
    assert!(moved.elevation.abs() < 1e-5);

    let moved = moved_piece(
        base,
        TransformMode::Move,
        Some(TransformAxis::Y),
        Vec3::ZERO,
        Vec3::new(2.0, 1.5, 3.0),
    );
    assert!((moved.elevation - 1.5).abs() < 1e-5);
    assert_eq!(moved.pos, base.pos);

    let shape = &shapes()[base.shape];
    let origin = gizmo_origin(&base, shape);
    let angle = 0.6_f32;
    let (sin, cos) = angle.sin_cos();
    let rotated = moved_piece(
        base,
        TransformMode::Rotate,
        Some(TransformAxis::X),
        origin + Vec3::Y,
        origin + Vec3::Y * cos + Vec3::Z * sin,
    );
    let expected = Quat::from_rotation_x(angle) * model::piece_quat(&base);
    assert!(model::piece_quat(&rotated).dot(expected).abs() > 0.99999);
    assert_eq!(rotated.pos, base.pos);
    assert_eq!(rotated.elevation, base.elevation);
}
