use super::*;

#[test]
fn hard_dirty_saves_immediately_and_soft_dirty_waits() {
    assert!(save_due(true, false, 10.0, 10.0, 0.0));
    assert!(!save_due(false, true, 10.0, 10.3, 0.0));
    assert!(save_due(false, true, 10.0, 11.0, 0.0));
    assert!(!save_due(false, true, 10.0, 20.0, 25.0));
}

#[test]
fn a_successful_save_clears_only_save_failure_notes() {
    let mut editor = Editor::default();
    let orbit = Orbit::default();
    let mut last = LastSaved::default();
    let mut clock = SaveClock::default();
    for note in [
        "save failed: storage is full",
        "save failed: storage unavailable",
        "save failed: saved map list is invalid",
    ] {
        editor.note = note;
        record_save_success(&mut editor, &orbit, &mut last, &mut clock, 12.0);
        assert_eq!(editor.note, "saved");
    }

    editor.note = "map loaded";
    record_save_success(&mut editor, &orbit, &mut last, &mut clock, 13.0);
    assert_eq!(editor.note, "map loaded");
}

#[test]
fn a_successful_save_refreshes_the_saved_pose_and_clock() {
    let character = Character {
        pos: Vec2::new(1.0, 2.0),
        feet: 0.5,
        yaw: 0.75,
        vy: 3.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health::default(),
    };
    let mut editor = Editor {
        character: Some(character),
        hard_dirty: true,
        soft_dirty: true,
        ..Default::default()
    };
    let orbit = Orbit {
        yaw: 1.0,
        ..Orbit::default()
    };
    let mut last = LastSaved::default();
    let mut clock = SaveClock {
        last_save: 2.0,
        retry_at: 99.0,
    };

    record_save_success(&mut editor, &orbit, &mut last, &mut clock, 12.0);

    assert!(!editor.hard_dirty);
    assert!(!editor.soft_dirty);
    assert_eq!(clock.last_save, 12.0);
    assert_eq!(clock.retry_at, 0.0);
    let seen = last.0.unwrap();
    assert_eq!(seen.orbit, orbit);
    assert_eq!(seen.character, Some(character));
}

#[test]
fn redo_shortcuts_need_command_shift_z_or_command_y() {
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::KeyY);
    assert!(redo_requested(&keys));

    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::ShiftLeft);
    keys.press(KeyCode::KeyZ);
    assert!(redo_requested(&keys));

    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::KeyZ);
    assert!(!redo_requested(&keys));
}

#[test]
fn saving_writes_each_pieces_height() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.pieces.push(Piece {
        id: 1,
        shape: block,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 1.25,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    });
    let json = level_json(&editor, &Orbit::default(), &Movers::default());
    assert!(json.contains("\"y\":1.250"), "{json}");
}

#[test]
fn saving_writes_each_pieces_full_rotation() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.pieces.push(Piece {
        id: 1,
        shape: block,
        pos: Vec2::ZERO,
        rot: 30.0_f32.to_radians(),
        pitch: (-15.0_f32).to_radians(),
        roll: 10.0_f32.to_radians(),
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
    let json = level_json(&editor, &Orbit::default(), &Movers::default());
    assert!(json.contains("\"rotDeg\":30.0"), "{json}");
    assert!(json.contains("\"pitchDeg\":-15.0"), "{json}");
    assert!(json.contains("\"rollDeg\":10.0"), "{json}");
    assert!(json.contains("\"dynamic\":true"), "{json}");
    assert!(!json.contains("\"interact\""), "{json}");
}

#[test]
fn saving_writes_an_interaction_only_when_set() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.pieces.push(Piece {
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
        interact: crate::model::InteractAction::TogglePause,
    });
    let json = level_json(&editor, &Orbit::default(), &Movers::default());
    assert!(json.contains("\"interact\":\"togglePause\""), "{json}");
}

#[test]
fn copying_keeps_the_selected_piece() {
    let block = model::shape_index("block").unwrap();
    let piece = Piece {
        id: 7,
        shape: block,
        pos: Vec2::new(1.0, 2.0),
        rot: 0.4,
        pitch: 0.0,
        roll: 0.0,
        elevation: 1.5,
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
    editor.pieces.push(piece);
    editor.selection.set(7);

    copy_piece(&mut editor);

    assert_eq!(editor.clipboard, vec![piece]);
}

#[test]
fn pasting_makes_a_new_selected_copy_at_the_cursor() {
    let block = model::shape_index("block").unwrap();
    let source = Piece {
        id: 3,
        shape: block,
        pos: Vec2::ZERO,
        rot: 0.4,
        pitch: 0.0,
        roll: 0.0,
        elevation: 1.5,
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
    editor.snap = false;
    editor.next_id = 4;
    editor.pieces.push(source);
    editor.selection.set(3);
    copy_piece(&mut editor);

    paste_piece(&mut editor, Some(Vec2::new(5.0, 6.0)));

    assert_eq!(editor.pieces.len(), 2);
    let pasted = *editor.pieces.last().unwrap();
    assert_eq!(pasted.id, 4);
    assert_eq!(editor.selection.primary(), Some(4));
    assert_eq!(pasted.shape, block);
    assert!((pasted.pos - Vec2::new(5.0, 6.0)).length() < 1e-5);
    assert!((pasted.rot - 0.4).abs() < 1e-5);
    assert!((pasted.elevation - 1.5).abs() < 1e-5);
    assert_eq!(editor.undo.len(), 1, "a paste must be undoable");
}

#[test]
fn pasting_without_a_cursor_lands_beside_the_source() {
    let block = model::shape_index("block").unwrap();
    let mut editor = Editor::default();
    editor.snap = false;
    editor.pieces.push(Piece {
        id: 1,
        shape: block,
        pos: model::hex_center(0, 0),
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
    editor.selection.set(1);
    copy_piece(&mut editor);

    paste_piece(&mut editor, None);

    let pasted = *editor.pieces.last().unwrap();
    let expected = model::hex_center(1, 0);
    assert!(
        (pasted.pos - expected).length() < 1e-5,
        "pos = {:?}",
        pasted.pos
    );
}

#[test]
fn copying_and_pasting_nothing_are_harmless() {
    let mut editor = Editor::default();
    copy_piece(&mut editor);
    assert!(editor.clipboard.is_empty());
    paste_piece(&mut editor, Some(Vec2::ZERO));
    assert!(editor.pieces.is_empty());
    assert_eq!(editor.selection.primary(), None);
}

#[test]
fn applying_a_snapshot_without_a_camera_keeps_the_default_camera() {
    let mut editor = Editor::default();
    let mut orbit = Orbit::default();
    let mut movers = Movers::default();
    let loaded = session::Snapshot {
        version: 1,
        ..Default::default()
    }
    .validate()
    .unwrap();
    editor.apply_loaded(&mut orbit, &mut movers, loaded);
    assert_eq!(orbit.yaw, Orbit::default().yaw);
    assert_eq!(orbit.mode, ViewMode::Third);
}

#[test]
fn undo_then_redo_restores_the_newer_state() {
    let mut editor = Editor::default();
    editor.commit();
    editor.pieces.push(test_piece(1));
    editor.commit();
    editor.pieces.push(test_piece(2));
    editor.undo();
    assert_eq!(editor.pieces.len(), 1);
    assert_eq!(editor.redo.len(), 1);
    editor.redo();
    assert_eq!(editor.pieces.len(), 2);
    assert!(editor.redo.is_empty());
}

#[test]
fn a_new_edit_clears_redo() {
    let mut editor = Editor::default();
    editor.commit();
    editor.pieces.push(test_piece(1));
    editor.commit();
    editor.pieces.push(test_piece(2));
    editor.undo();
    assert_eq!(editor.redo.len(), 1);
    editor.commit();
    editor.pieces.push(test_piece(3));
    assert!(editor.redo.is_empty());
}

#[test]
fn both_history_stacks_are_capped_at_sixty() {
    let mut editor = Editor::default();
    for id in 1..=70 {
        editor.commit();
        editor.pieces.push(test_piece(id));
    }
    assert_eq!(editor.undo.len(), 60);
    assert_eq!(editor.undo[0].pieces.len(), 10);
    assert_eq!(editor.undo[59].pieces.len(), 69);
    for _ in 0..70 {
        editor.undo();
    }
    assert_eq!(editor.redo.len(), 60);
}

#[test]
fn edits_and_history_moves_mark_the_session_dirty() {
    let mut editor = Editor::default();
    editor.commit();
    editor.pieces.push(test_piece(1));
    assert!(editor.hard_dirty);
    editor.hard_dirty = false;
    editor.undo();
    assert!(editor.hard_dirty);
    editor.hard_dirty = false;
    editor.redo();
    assert!(editor.hard_dirty);
}

#[test]
fn the_course_reset_clears_redo() {
    let mut editor = Editor::default();
    editor.redo.push(LevelState {
        pieces: vec![test_piece(1)],
        ..default()
    });
    editor.load_course();
    assert!(editor.redo.is_empty());
}

#[test]
fn a_session_capture_restores_labels_and_groups() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    let group = editor.group_node(GraphNode::Piece(1)).unwrap();
    editor.commit();
    assert!(editor.rename_node(GraphNode::Piece(1), "North crate"));
    assert!(editor.outline.toggle_lock(1));

    let orbit = Orbit::default();
    let movers = Movers::default();
    let snapshot = editor.capture(&orbit, &movers);
    assert_eq!(snapshot.pieces[0].name.as_deref(), Some("North crate"));
    assert_eq!(snapshot.pieces[0].group, Some(group));
    assert!(snapshot.pieces[0].locked);
    assert_eq!(snapshot.groups.len(), 1);
    assert_eq!(snapshot.undo.len(), 2);
    assert!(snapshot.undo[0].groups.is_empty());
    assert_eq!(snapshot.undo[1].groups.len(), 1);
    assert_eq!(snapshot.undo[1].pieces[0].group, Some(group));

    let mut restored = Editor::default();
    let mut restored_orbit = Orbit::default();
    let mut restored_movers = Movers::default();
    restored.apply_loaded(
        &mut restored_orbit,
        &mut restored_movers,
        snapshot.validate().unwrap(),
    );
    assert_eq!(restored.node_label(GraphNode::Piece(1)), "North crate");
    assert_eq!(restored.outline.piece_parent(1), Some(group));
    assert!(restored.outline.is_locked(1));
    assert_eq!(
        restored.outline.group(group).unwrap().name,
        format!("Group {group}")
    );
    assert_eq!(restored.undo.len(), editor.undo.len());
    assert!(restored.undo[0].outline.groups.is_empty());
    assert_eq!(restored.undo[1].outline.groups.len(), 1);
    assert_eq!(restored.undo[1].outline.piece_parent(1), Some(group));
}

#[test]
fn the_debug_level_json_includes_labels_and_groups() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    let group = editor.group_node(GraphNode::Piece(1)).unwrap();
    editor.rename_node(GraphNode::Piece(1), "North \"wall\"");
    editor.outline.toggle_lock(1);
    let json = level_json(&editor, &Orbit::default(), &Movers::default());
    assert!(json.contains("\"name\":\"North \\\"wall\\\"\""), "{json}");
    assert!(json.contains("\"locked\":true"), "{json}");
    assert!(json.contains(&format!("\"group\":{group}")), "{json}");
    assert!(json.contains("\"groups\":[{\"id\":"), "{json}");
    assert!(json.contains("\"collapsed\":false"), "{json}");
}

#[test]
fn copying_copies_platform_paths() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(7));
    let mut movers = Movers::default();
    let mover = movers.mover_mut(7, Vec2::ZERO);
    mover.waypoints.push(crate::platforms::PlatformWaypoint {
        pos: Vec2::new(4.0, 0.0),
        incoming: Some(Vec2::new(3.0, -1.0)),
        outgoing: None,
    });
    mover.mode = crate::platforms::PlatformMode::Loop;
    mover.speed = 2.5;
    mover.paused = true;
    mover.rebuild();

    let json = level_json(&editor, &Orbit::default(), &movers);
    let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let platforms = doc
        .get("platforms")
        .expect("platforms key")
        .as_array()
        .unwrap();
    assert_eq!(platforms.len(), 1, "{json}");
    let saved = &platforms[0];
    assert_eq!(saved.get("piece").unwrap(), 7);
    assert_eq!(saved.get("mode").unwrap(), "loop");
    assert_eq!(saved.get("speed").unwrap(), 2.5);
    assert_eq!(saved.get("paused").unwrap(), true);
    let waypoints = saved.get("waypoints").unwrap().as_array().unwrap();
    assert_eq!(waypoints.len(), 2);
    assert_eq!(waypoints[0].get("x").unwrap(), 0.0);
    assert_eq!(waypoints[1].get("x").unwrap(), 4.0);
    assert_eq!(
        waypoints[1].get("incoming").unwrap(),
        &serde_json::json!([3.0, -1.0])
    );
}

#[test]
fn copying_copies_trigger_volumes_only_when_custom() {
    let mut editor = Editor::default();
    let shape = model::shape_index("trigger_box").unwrap();
    editor.pieces.push(Piece {
        id: 1,
        shape,
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: Vec3::new(3.0, 4.0, 5.0),
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: 0.8,
        trigger_color: 0xff0000,
        trigger_visible: false,
        interact: crate::model::InteractAction::None,
    });
    editor.pieces.push(Piece {
        id: 2,
        shape,
        pos: Vec2::X,
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

    let json = level_json(&editor, &Orbit::default(), &Movers::default());
    let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let pieces = doc.get("pieces").unwrap().as_array().unwrap();
    let custom = &pieces[0];
    assert_eq!(custom.get("triggerWidth").unwrap(), 3.0);
    assert_eq!(custom.get("triggerLength").unwrap(), 5.0);
    assert_eq!(custom.get("triggerHeight").unwrap(), 4.0);
    assert_eq!(custom.get("triggerAlpha").unwrap(), 0.8);
    assert_eq!(custom.get("triggerColor").unwrap(), "#ff0000");
    assert_eq!(custom.get("triggerVisible").unwrap(), false);
    let plain = &pieces[1];
    assert!(plain.get("triggerWidth").is_none(), "{plain}");
    assert!(plain.get("triggerShape").is_none(), "{plain}");
}

#[test]
fn copying_copies_character_camera_and_next_id() {
    let mut editor = Editor::default();
    editor.next_id = 42;
    editor.character = Some(Character {
        pos: Vec2::new(1.5, -2.5),
        feet: 0.75,
        yaw: 1.0,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: true,
        sliding: false,
        health: crate::statuses::Health {
            current: 70.0,
            max: 100.0,
        },
    });
    let orbit = Orbit {
        target: Vec3::new(1.0, 2.0, 3.0),
        yaw: 0.5,
        pitch: 0.9,
        distance: 12.0,
        mode: ViewMode::Top,
    };
    let json = level_json(&editor, &orbit, &Movers::default());
    let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(doc.get("version").unwrap(), 2);
    assert_eq!(doc.get("nextId").unwrap(), 42);
    let character = doc.get("character").expect("character key");
    assert_eq!(character.get("feet").unwrap(), 0.75);
    assert_eq!(character.get("hp").unwrap(), 70.0);
    assert_eq!(character.get("maxHp").unwrap(), 100.0);
    let camera = doc.get("camera").expect("camera key");
    assert_eq!(camera.get("targetX").unwrap(), 1.0);
    assert_eq!(camera.get("mode").unwrap(), "top");
    assert_eq!(camera.get("distance").unwrap(), 12.0);
}
