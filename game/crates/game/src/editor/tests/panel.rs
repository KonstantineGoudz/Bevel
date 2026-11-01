use super::*;
use bevy::text::EditableText;

#[test]
fn property_fields_map_to_piece_values() {
    let mut piece = test_piece(1);

    PropertyField::PosX.set(&mut piece, 2.5);
    PropertyField::PosY.set(&mut piece, 1.25);
    PropertyField::PosZ.set(&mut piece, -3.0);
    assert_eq!(piece.pos, Vec2::new(2.5, -3.0));
    assert!((piece.elevation - 1.25).abs() < 1e-5);

    PropertyField::RotX.set(&mut piece, 10.0);
    PropertyField::RotY.set(&mut piece, 20.0);
    PropertyField::RotZ.set(&mut piece, -30.0);
    assert!((piece.pitch.to_degrees() - 10.0).abs() < 1e-4);
    assert!((piece.rot.to_degrees() - 20.0).abs() < 1e-4);
    assert!((piece.roll.to_degrees() + 30.0).abs() < 1e-4);
    assert_eq!(PropertyField::PosX.format(&piece), "2.500");
    assert_eq!(PropertyField::RotY.format(&piece), "20.0");
    assert_eq!(PropertyField::RotY.parse(" 20 "), Some(20.0));
    assert_eq!(PropertyField::RotY.parse("not a number"), None);
}

#[test]
fn the_property_panel_hit_box_matches_its_floating_window() {
    let size = Vec2::new(1000.0, 800.0);
    let inside = Vec2::new(1000.0 - PROPERTIES_WIDTH - 5.0, 40.0);
    assert!(over_properties_panel(inside, size, true));
    assert!(!over_properties_panel(inside, size, false));
    assert!(!over_properties_panel(Vec2::new(100.0, 40.0), size, true));
    assert!(!over_properties_panel(Vec2::new(760.0, 700.0), size, true));
}

#[test]
fn the_physics_toggle_is_undoable_and_flips_both_ways() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selected = Some(1);

    toggle_selected_physics(&mut editor);
    assert!(editor.pieces[0].dynamic);
    assert_eq!(editor.undo.len(), 1);
    assert_eq!(editor.note, "physics: dynamic");

    toggle_selected_physics(&mut editor);
    assert!(!editor.pieces[0].dynamic);
    assert_eq!(editor.undo.len(), 2);
    assert_eq!(editor.note, "physics: static");
}

#[test]
fn hud_and_panel_systems_have_disjoint_queries() {
    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<Movers>();
    app.init_resource::<TouchState>();
    app.init_resource::<InputFocus>();
    app.init_resource::<PropertyEditState>();
    app.init_resource::<Visuals>();
    app.add_systems(
        Update,
        (
            update_health_hud,
            sync_properties_panel,
            apply_property_edits,
        ),
    );

    app.world_mut().spawn((HealthHud, Node::default()));
    app.world_mut()
        .spawn((HealthFill, Node::default(), BackgroundColor::default()));
    app.world_mut().spawn((HealthText, Text::new("")));
    app.world_mut().spawn((PropertiesPanel, Node::default()));
    app.world_mut()
        .spawn((PropertyShapeLabel, Text::new("Block")));
    app.world_mut()
        .spawn((PhysicsToggleLabel, Text::new("Physics: Static")));
    app.world_mut()
        .spawn((PlatformModeLabel, Text::new("Mode: none")));
    app.world_mut()
        .spawn((PlatformPauseLabel, Text::new("Paused: no")));
    app.world_mut()
        .spawn((PlatformSpeedLabel, Text::new("1.5 m/s")));
    app.world_mut()
        .spawn((PropertyField::PosX, EditableText::new("0"), Node::default()));

    app.update();
}

#[test]
fn visuals_system_has_disjoint_queries() {
    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<Orbit>();
    app.init_resource::<Visuals>();
    app.insert_resource(Kit {
        parts: Vec::new(),
        mats: Vec::new(),
        glass: Handle::default(),
        ghost: Handle::default(),
        pill: Handle::default(),
        pill_mat: Handle::default(),
        character_collider: Collider::cuboid(1.0, 1.0, 1.0),
    });
    app.add_systems(Update, sync_visuals);
    app.update();
}
