//! Headless boot test: stand up the real EditorPlugin and run frames, to
//! catch startup/schedule panics (e.g. bad Single queries) and missing UI.

use super::*;
use bevy::clipboard::Clipboard;
use bevy::gizmos::prelude::{
    AppGizmoBuilder, DefaultGizmoConfigGroup, GizmoAsset, GizmoConfigStore,
};
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::window::WindowResolution;

fn boot_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        RapierPhysicsPlugin::<NoUserData>::default(),
    ));
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<GizmoAsset>();
    // Resources/messages DefaultPlugins would normally provide.
    app.init_resource::<ButtonInput<KeyCode>>();
    app.init_resource::<ButtonInput<MouseButton>>();
    app.init_resource::<Touches>();
    app.init_resource::<InputFocus>();
    app.init_resource::<UiScale>();
    app.init_resource::<Time>();
    app.init_resource::<Clipboard>();
    app.init_resource::<GizmoConfigStore>();
    app.init_gizmo_group::<DefaultGizmoConfigGroup>();
    app.add_message::<WindowFocused>();
    app.add_message::<MouseMotion>();
    app.add_message::<MouseWheel>();
    app.add_plugins(EditorPlugin);
    // A headless stand-in for the winit window (CursorOptions is normally
    // added by the windowing backend).
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(1280, 720),
            ..default()
        },
        PrimaryWindow,
        CursorOptions::default(),
    ));
    app
}

#[test]
fn full_editor_schedule_runs_without_panicking() {
    let mut app = boot_app();
    app.update();
    app.update();
    app.update();
}

#[test]
fn boot_app_with_glade_pieces_runs_without_panicking() {
    let mut app = boot_app();
    let json = serde_json::json!({
        "version": 1,
        "pieces": [
            {
                "id": 1,
                "shape": "glade_tower",
                "x": 0.0,
                "z": 0.0,
                "elevation": 0.0,
                "rotDeg": 0.0,
                "pitchDeg": 0.0,
                "rollDeg": 0.0,
                "dynamic": false,
                "triggerWidth": 3.0,
                "triggerLength": 3.0,
                "triggerHeight": 4.0,
                "triggerAlpha": 0.8
            },
            {
                "id": 2,
                "shape": "glade_room",
                "x": 4.0,
                "z": 0.0,
                "elevation": 0.0,
                "rotDeg": 0.0,
                "pitchDeg": 0.0,
                "rollDeg": 0.0,
                "dynamic": false,
                "triggerWidth": 4.0,
                "triggerLength": 3.0,
                "triggerHeight": 3.0,
                "triggerAlpha": 0.8
            },
            {
                "id": 3,
                "shape": "glade_wall",
                "x": 0.0,
                "z": 4.0,
                "elevation": 0.0,
                "rotDeg": 0.0,
                "pitchDeg": 0.0,
                "rollDeg": 0.0,
                "dynamic": false,
                "triggerWidth": 1.0,
                "triggerLength": 3.0,
                "triggerHeight": 2.0,
                "triggerAlpha": 0.8
            },
            {
                "id": 4,
                "shape": "glade_door",
                "x": 0.0,
                "z": 4.0,
                "elevation": 0.0,
                "rotDeg": 0.0,
                "pitchDeg": 0.0,
                "rollDeg": 0.0,
                "dynamic": false
            }
        ],
        "character": {
            "x": 0.0,
            "z": 0.0,
            "feet": 0.0,
            "yawDeg": 0.0
        },
        "camera": {
            "targetX": 0.0,
            "targetY": 1.0,
            "targetZ": 0.0,
            "yaw": 0.0,
            "pitch": 0.3,
            "distance": 6.0,
            "mode": "third"
        }
    });
    let snapshot: crate::session::Snapshot = serde_json::from_value(json).expect("parses");
    let loaded = snapshot.validate().expect("validates");
    let world = app.world_mut();
    world.resource_scope(|world, mut editor: Mut<Editor>| {
        world.resource_scope(|_world, mut orbit: Mut<Orbit>| {
            let mut movers = Movers::default();
            editor.apply_loaded(&mut orbit, &mut movers, loaded);
        });
    });
    app.update();
    app.update();
    app.update();
}

#[test]
fn setup_spawns_the_palette_and_toolbars() {
    let mut app = boot_app();
    app.update();
    let world = app.world_mut();
    // Desktop palette buttons, including the grass brush and its row.
    let palette: Vec<_> = world
        .query::<&PaletteButton>()
        .iter(world)
        .map(|button| button.0)
        .collect();
    assert!(palette.contains(&PaletteAction::Grass));
    assert!(palette.contains(&PaletteAction::GrassErase));
    assert!(palette.contains(&PaletteAction::GrassSlow));
    assert!(palette.contains(&PaletteAction::Path));
    assert!(
        palette.contains(&PaletteAction::Home),
        "palette has a Maps button for the native home screen"
    );
    assert_eq!(
        world.query_filtered::<&HomeRoot, ()>().iter(world).count(),
        1,
        "native home overlay is spawned"
    );
    // Exactly one grass options row and one path row.
    assert_eq!(
        world
            .query_filtered::<&GrassActions, ()>()
            .iter(world)
            .count(),
        1,
        "one grass actions row"
    );
    assert_eq!(
        world
            .query_filtered::<&PathActions, ()>()
            .iter(world)
            .count(),
        1,
        "one path actions row"
    );
    // Touch toolbar carries the grass brush; the path row its eraser.
    let touch: Vec<_> = world
        .query::<&TouchButton>()
        .iter(world)
        .map(|button| button.0)
        .collect();
    assert!(touch.contains(&TouchAction::Grass));
    assert!(touch.contains(&TouchAction::GrassErase));
    // Exactly one default-UI camera (the main view): Bevy assigns UI to
    // the highest-order camera, so without the marker every button would
    // render into the minimap viewport instead.
    assert_eq!(
        world
            .query_filtered::<Entity, With<bevy::ui::IsDefaultUiCamera>>()
            .iter(world)
            .count(),
        1,
        "one default UI camera"
    );
}

#[test]
fn minimap_click_sets_a_route_like_a_play_click() {
    use bevy::camera::{CameraProjection, NormalizedRenderTarget, RenderTargetInfo};
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{PointerPress, PointerRelease};
    use bevy::picking::pointer::Location;

    let mut app = boot_app();
    app.update();
    {
        let world = app.world_mut();
        let mut editor = world.resource_mut::<Editor>();
        editor.mode = Mode::Play;
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
    }
    app.update();
    app.update();
    // Prime the computed camera values; the camera plugin derives these
    // from the window and viewport every frame in the real app.
    let (origin, edge) = minimap_rect(Vec2::new(1280.0, 720.0), 1.0);
    let centre = origin + Vec2::splat(edge * 0.5);
    {
        let world = app.world_mut();
        let mut cameras =
            world.query_filtered::<(&mut Camera, &mut Projection), With<MinimapCamera>>();
        let (mut camera, mut projection) = cameras.single_mut(world).expect("minimap");
        if let Projection::Orthographic(ortho) = &mut *projection {
            ortho.update(edge, edge);
        }
        camera.computed.target_info = Some(RenderTargetInfo {
            physical_size: UVec2::new(1280, 720),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view = projection.get_clip_from_view();
    }
    let frame = app
        .world_mut()
        .query_filtered::<Entity, With<MinimapFrame>>()
        .single(app.world_mut())
        .expect("minimap frame");
    let pointing = |position| {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 1280,
                    height: 720,
                },
                position,
            },
        )
    };
    app.world_mut().trigger(PointerPress {
        entity: frame,
        pointer: pointing(centre),
        button: PointerButton::Primary,
        hit: HitData::new(frame, 0.0, None, None),
        count: 1,
    });
    app.world_mut().trigger(PointerRelease {
        entity: frame,
        pointer: pointing(centre),
        button: PointerButton::Primary,
        hit: HitData::new(frame, 0.0, None, None),
    });
    let editor = app.world().resource::<Editor>();
    assert!(
        editor.plan.as_ref().is_some_and(|plan| !plan.is_empty()),
        "a minimap tap adds a waypoint"
    );
    assert!(editor.walk.is_some(), "routing starts walking");
}

#[test]
fn follow_button_centers_the_map_on_the_character() {
    let mut app = boot_app();
    app.update();
    {
        let world = app.world_mut();
        let mut editor = world.resource_mut::<Editor>();
        editor.mode = Mode::Play;
        editor.character = Some(Character {
            pos: Vec2::new(1.0, 2.0),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
    }
    app.update();
    let follow = app
        .world_mut()
        .query_filtered::<Entity, With<MinimapFollow>>()
        .single(app.world_mut())
        .expect("follow button");
    app.world_mut().trigger(Activate { entity: follow });
    app.update();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character.as_mut().expect("character").pos = Vec2::new(7.0, -4.0);
    }
    app.update();
    assert_eq!(
        app.world().resource::<Minimap>().center,
        Vec2::new(7.0, -4.0),
        "follow mode keeps the character centered"
    );
}

#[test]
fn first_person_view_cone_is_minimap_only() {
    use bevy::camera::visibility::RenderLayers;

    let mut app = boot_app();
    app.update();
    {
        let world = app.world_mut();
        let mut editor = world.resource_mut::<Editor>();
        editor.mode = Mode::Play;
        editor.character = Some(Character {
            pos: Vec2::new(2.0, 1.0),
            feet: 0.0,
            yaw: 0.4,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        world.resource_mut::<Orbit>().mode = ViewMode::First;
    }
    app.update();
    let world = app.world_mut();
    // The minimap camera renders the overlay layer; the main view must not.
    let minimap_layers = world
        .query_filtered::<&RenderLayers, With<MinimapCamera>>()
        .single(world)
        .expect("minimap render layers");
    assert!(minimap_layers.intersects(&RenderLayers::layer(1)));
    let main_layers = world
        .query_filtered::<Option<&RenderLayers>, (With<Camera3d>, Without<MinimapCamera>)>()
        .single(world)
        .expect("main camera")
        .cloned()
        .unwrap_or_default();
    assert!(!main_layers.intersects(&RenderLayers::layer(1)));
    let (config, _) = world
        .resource::<GizmoConfigStore>()
        .config::<MinimapGizmos>();
    assert_eq!(config.render_layers, RenderLayers::layer(1));
}

#[test]
fn xray_gizmos_render_in_front_of_geometry() {
    let mut app = boot_app();
    app.update();
    let (config, _) = app
        .world()
        .resource::<GizmoConfigStore>()
        .config::<XrayGizmos>();
    assert_eq!(config.depth_bias, -1.0);
}

#[test]
fn play_mode_with_grass_runs_without_panicking() {
    let mut app = boot_app();
    app.update();
    {
        let world = app.world_mut();
        let mut editor = world.resource_mut::<Editor>();
        let mut piece = super::test_piece(1);
        piece.pos = Vec2::ZERO;
        editor.pieces.push(piece);
        editor.character = Some(Character {
            pos: Vec2::new(3.0, 0.0),
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        editor.mode = Mode::Play;
        editor.grass.insert((0, 0), 3);
        editor.grass.insert((5, 5), 1);
        editor.grass_rev = editor.grass_rev.wrapping_add(1);
    }
    app.update();
    app.update();
    let world = app.world_mut();
    // The minimap camera woke up with a viewport; grass grew tufts.
    let (minimap_active, has_viewport) = {
        let minimap = world
            .query_filtered::<&Camera, With<MinimapCamera>>()
            .single(world)
            .expect("minimap camera");
        (minimap.is_active, minimap.viewport.is_some())
    };
    assert!(minimap_active, "minimap is active in Play");
    assert!(has_viewport, "minimap owns its corner viewport");
    assert!(
        world.query::<&GrassTuft>().iter(world).count() > 0,
        "tufts render for painted hexes"
    );
}

#[test]
fn full_schedule_advances_a_bridge_style_walk() {
    let mut app = boot_app();
    app.update();
    let start = Vec2::new(6.0, 0.0);
    let end = Vec2::new(-3.0, 0.0);
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.character = Some(Character {
            pos: start,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: crate::statuses::Health::default(),
        });
        let mut plan = Plan::default();
        plan.append(end, model::pixel_to_hex(end), 0.0);
        editor.plan = Some(plan);
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
            hexes: 9,
        });
        editor.walk = Some(Walk {
            distance: 0.0,
            stuck: 0.0,
        });
    }
    for _ in 0..60 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
    }
    let editor = app.world().resource::<Editor>();
    let pos = editor.character.expect("character").pos;
    // Headless frames advance almost no real time, so each step is tiny —
    // but the schedule must still move the walker forward monotonically.
    assert!(
        pos.distance(start) > 0.02,
        "the full schedule should walk the character, pos={pos:?}"
    );
    assert!(
        editor.walk.is_some(),
        "a 9 m route is still in progress after a few centimetres"
    );
}

#[test]
fn building_mode_palette_groups_follow_mode_and_selection() {
    let mut app = boot_app();
    app.update();

    let group_display = |world: &mut World, target: PaletteGroup| -> Display {
        let mut q = world.query::<(&PaletteGroup, &Node)>();
        q.iter(world)
            .find(|(g, _)| **g == target)
            .map(|(_, n)| n.display)
            .expect("palette group")
    };

    // 1. Default Build mode shows BuildMode and hides Building groups.
    {
        let world = app.world_mut();
        assert_eq!(group_display(world, PaletteGroup::BuildMode), Display::Flex);
        assert_eq!(group_display(world, PaletteGroup::BuildingBase), Display::None);
        assert_eq!(group_display(world, PaletteGroup::DoorsAndWindows), Display::None);
        assert_eq!(group_display(world, PaletteGroup::BuildingFixtures), Display::None);
    }

    // 2. Switching to Building mode with no selection shows BuildingBase only.
    app.world_mut().resource_mut::<Editor>().mode = Mode::Building;
    app.update();
    {
        let world = app.world_mut();
        assert_eq!(group_display(world, PaletteGroup::BuildMode), Display::None);
        assert_eq!(group_display(world, PaletteGroup::BuildingBase), Display::Flex);
        assert_eq!(group_display(world, PaletteGroup::DoorsAndWindows), Display::None);
        assert_eq!(group_display(world, PaletteGroup::BuildingFixtures), Display::None);
    }

    // 3. Selecting a Building Room reveals Doors & Windows and Fixtures.
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let mut room = super::test_piece(1);
        room.shape = model::shape_index("glade_room").unwrap();
        editor.pieces.push(room);
        editor.selection.set(1);
    }
    app.update();
    {
        let world = app.world_mut();
        assert_eq!(group_display(world, PaletteGroup::DoorsAndWindows), Display::Flex);
        assert_eq!(group_display(world, PaletteGroup::BuildingFixtures), Display::Flex);
    }

    // 4. Selecting a Floor reveals Fixtures, but keeps Doors & Windows hidden.
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let mut floor = super::test_piece(2);
        floor.shape = model::shape_index("floor").unwrap();
        editor.pieces.push(floor);
        editor.selection.set(2);
    }
    app.update();
    {
        let world = app.world_mut();
        assert_eq!(group_display(world, PaletteGroup::DoorsAndWindows), Display::None);
        assert_eq!(group_display(world, PaletteGroup::BuildingFixtures), Display::Flex);
    }
}

#[test]
fn building_door_and_window_require_clicking_a_building_wall() {
    use bevy::ecs::system::RunSystemOnce;

    let mut app = boot_app();
    app.update();

    let door_idx = model::shape_index("glade_door").unwrap();
    let room_idx = model::shape_index("glade_room").unwrap();

    // Place a room at the origin (extents ~3.2 x 2.4).
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.mode = Mode::Building;
        let mut room = super::test_piece(1);
        room.shape = room_idx;
        room.pos = Vec2::ZERO;
        editor.pieces.push(room);
        editor.tool = Some(door_idx);
    }

    // Clicking far away on empty ground must NOT place the door.
    {
        let world = app.world_mut();
        {
            let mut cursor = world.resource_mut::<Cursor>();
            cursor.world = Some(Vec2::new(20.0, 20.0));
            cursor.last_world = cursor.world;
            cursor.screen = Some(Vec2::new(640.0, 360.0));
            cursor.ray = None;
        }
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        world.run_system_once(interact).unwrap();
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear_just_pressed(MouseButton::Left);
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
    }

    assert_eq!(
        app.world().resource::<Editor>().pieces.len(),
        1,
        "door must not be placed on empty ground"
    );
    assert!(
        app.world().resource::<Editor>().ghost.is_none(),
        "ghost must be hidden when not hovering a building wall"
    );

    // Clicking on the room's wall snaps and places the door.
    {
        let world = app.world_mut();
        {
            let mut cursor = world.resource_mut::<Cursor>();
            cursor.world = Some(Vec2::new(0.0, 1.2));
            cursor.last_world = cursor.world;
            cursor.screen = Some(Vec2::new(640.0, 360.0));
            cursor.ray = None;
        }
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        world.run_system_once(interact).unwrap();
    }

    assert_eq!(
        app.world().resource::<Editor>().pieces.len(),
        2,
        "door should be placed when clicking on a building wall"
    );
}
