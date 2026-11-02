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
fn damage_property_maps_to_trigger_and_clamps() {
    let mut piece = test_piece(1);
    assert_eq!(PropertyField::Damage.get(&piece), 0.0);

    PropertyField::Damage.set(&mut piece, 12.5);
    assert!((piece.trigger.damage - 12.5).abs() < 1e-5);
    assert_eq!(PropertyField::Damage.format(&piece), "12.5");

    // Negative edits clamp to harmless instead of healing on touch.
    PropertyField::Damage.set(&mut piece, -4.0);
    assert_eq!(piece.trigger.damage, 0.0);

    assert_eq!(PropertyField::Damage.parse(" 7.5 "), Some(7.5));
    assert_eq!(PropertyField::Damage.parse("not a number"), None);
}

#[test]
fn the_property_panel_hit_box_matches_its_floating_window() {
    let size = Vec2::new(1000.0, 800.0);
    let inside = Vec2::new(1000.0 - PROPERTIES_WIDTH - 5.0, 40.0);
    assert!(over_properties_panel(inside, size, true));
    assert!(!over_properties_panel(inside, size, false));
    assert!(!over_properties_panel(Vec2::new(100.0, 40.0), size, true));
    // Panel height is 850, so y=750 is still inside (12..862); check beyond.
    assert!(!over_properties_panel(Vec2::new(760.0, 900.0), size, true));
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
fn status_and_panel_systems_have_disjoint_queries() {
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
            update_character_status,
            sync_properties_panel,
            apply_property_edits,
        ),
    );

    app.world_mut().spawn((CharacterStatus, Node::default()));
    app.world_mut()
        .spawn((HealthFill, Node::default(), BackgroundColor::default()));
    app.world_mut().spawn((HealthText, Text::new("")));
    app.world_mut().spawn((VelocityText, Text::new("")));
    app.world_mut().spawn((VelocityAxes, Text::new("")));
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
        .spawn((PlatformEditLabel, Text::new("Edit path: off")));
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

#[test]
fn the_object_graph_hit_box_matches_its_floating_window() {
    let size = Vec2::new(1000.0, 800.0);
    let inside = Vec2::new(PANEL_WIDTH + OUTLINER_MARGIN + 20.0, OUTLINER_TOP + 20.0);
    assert!(over_outliner_panel(inside, size, false));
    assert!(!over_outliner_panel(
        Vec2::new(PANEL_WIDTH + 2.0, OUTLINER_TOP),
        size,
        false
    ));
    assert!(!over_outliner_panel(
        Vec2::new(
            PANEL_WIDTH + OUTLINER_MARGIN,
            OUTLINER_TOP + OUTLINER_HEIGHT + 1.0
        ),
        size,
        false
    ));
    // Collapsed: only the title bar occupies space.
    assert!(!over_outliner_panel(
        Vec2::new(PANEL_WIDTH + OUTLINER_MARGIN + 20.0, OUTLINER_TOP + 40.0),
        size,
        true
    ));
    assert!(over_outliner_panel(
        Vec2::new(PANEL_WIDTH + OUTLINER_MARGIN + 4.0, OUTLINER_TOP + 4.0),
        size,
        true
    ));
}

#[test]
fn the_object_graph_panel_spawns_and_lists_pieces_and_groups() {
    fn spawn_panel(mut commands: Commands) {
        spawn_object_graph(&mut commands);
    }

    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<OutlinerState>();
    app.init_resource::<OutlinerEdit>();
    app.init_resource::<InputFocus>();
    app.add_systems(Startup, spawn_panel);
    app.add_systems(
        Update,
        (
            sync_outliner_selection,
            apply_outliner_edit,
            sync_outliner_rows,
            sync_outliner_panel,
            sync_outliner_fields,
            update_outliner_rows,
        ),
    );
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(test_piece(1));
        editor.pieces.push(test_piece(2));
        let group = editor.new_group();
        assert!(editor.move_node(GraphNode::Piece(1), Some(group)));
    }

    app.update();

    let list = app
        .world_mut()
        .query_filtered::<Entity, With<OutlinerList>>()
        .single(app.world())
        .expect("outliner list");
    let rows: Vec<GraphNode> = {
        let world = app.world_mut();
        let mut query = world.query::<&OutlinerRow>();
        query.iter(world).map(|row| row.0).collect()
    };
    assert!(rows.contains(&GraphNode::Piece(1)));
    assert!(rows.contains(&GraphNode::Piece(2)));
    assert!(rows.iter().any(|node| matches!(node, GraphNode::Group(_))));
    assert!(app.world().get_entity(list).is_ok());

    // Collapse the panel from its header button.
    let button = {
        let world = app.world_mut();
        let mut query = world.query_filtered::<Entity, With<OutlinerCollapseButton>>();
        query.single(world).expect("collapse button")
    };
    let panel = {
        let world = app.world_mut();
        let mut query = world.query_filtered::<Entity, With<OutlinerPanel>>();
        query.single(world).expect("outliner panel")
    };
    let body = {
        let world = app.world_mut();
        let mut query = world.query_filtered::<Entity, With<OutlinerBody>>();
        query.single(world).expect("outliner body")
    };
    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(app.world().resource::<OutlinerState>().collapsed);
    assert_eq!(
        app.world().entity(panel).get::<Node>().unwrap().height,
        px(OUTLINER_COLLAPSED_HEIGHT)
    );
    assert_eq!(
        app.world().entity(body).get::<Node>().unwrap().display,
        Display::None
    );

    app.world_mut().trigger(Activate { entity: button });
    app.update();
    assert!(!app.world().resource::<OutlinerState>().collapsed);
    assert_eq!(
        app.world().entity(panel).get::<Node>().unwrap().height,
        px(OUTLINER_HEIGHT)
    );
    assert_eq!(
        app.world().entity(body).get::<Node>().unwrap().display,
        Display::Flex
    );
}

#[test]
fn a_press_on_row_text_resolves_to_the_owning_row() {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::Pointer;
    use bevy::picking::pointer::Location;

    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<OutlinerState>();
    app.init_resource::<OutlinerEdit>();
    app.init_resource::<InputFocus>();
    app.init_resource::<Orbit>();
    app.world_mut()
        .resource_mut::<Editor>()
        .pieces
        .push(test_piece(1));

    let list = app.world_mut().spawn((OutlinerList, Node::default())).id();
    let row = app
        .world_mut()
        .spawn((OutlinerRow(GraphNode::Piece(1)), ChildOf(list)))
        .id();
    // Bevy's UI picking targets the text node (or a text span), not the row,
    // so the press must bubble up through the row and resolve there.
    let text = app.world_mut().spawn(ChildOf(row)).id();
    assert_eq!(
        app.world()
            .entity(text)
            .get::<ChildOf>()
            .map(|c| c.parent()),
        Some(row)
    );
    assert_eq!(
        app.world().entity(row).get::<ChildOf>().map(|c| c.parent()),
        Some(list)
    );
    // In the app the observer lives on the list and receives the event after
    // bubbling; attaching it to the text here exercises the same hit
    // resolution without needing the full picking pipeline.
    app.world_mut().entity_mut(text).observe(on_outliner_press);

    app.world_mut().trigger(PointerPress {
        entity: text,
        pointer: Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 100,
                    height: 100,
                },
                position: Vec2::ZERO,
            },
        ),
        button: PointerButton::Primary,
        hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
        count: 1,
    });

    assert_eq!(
        app.world().resource::<OutlinerState>().selection,
        GraphNode::Piece(1)
    );
    assert_eq!(app.world().resource::<Editor>().selected, Some(1));
}

#[test]
fn a_new_group_button_activation_creates_exactly_one_group() {
    fn spawn_panel(mut commands: Commands) {
        spawn_object_graph(&mut commands);
    }

    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<OutlinerState>();
    app.init_resource::<OutlinerEdit>();
    app.init_resource::<InputFocus>();
    app.add_systems(Startup, spawn_panel);
    app.update();

    let button = {
        let world = app.world_mut();
        let mut query = world.query::<(&OutlinerActionButton, Entity)>();
        query
            .iter(world)
            .find(|(action, _)| action.0 == OutlinerAction::NewGroup)
            .map(|(_, entity)| entity)
            .expect("New Group button")
    };
    app.world_mut().trigger(Activate { entity: button });

    let editor = app.world().resource::<Editor>();
    assert_eq!(editor.outline.groups.len(), 1);
    assert_eq!(editor.note, "group added");
    assert!(matches!(
        app.world().resource::<OutlinerState>().selection,
        GraphNode::Group(_)
    ));
}
