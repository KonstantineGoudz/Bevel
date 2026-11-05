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
    // Exactly one grass options row and one path row.
    assert_eq!(
        world.query_filtered::<&GrassActions, ()>().iter(world).count(),
        1,
        "one grass actions row"
    );
    assert_eq!(
        world.query_filtered::<&PathActions, ()>().iter(world).count(),
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
        let mut cameras = world
            .query_filtered::<(&mut Camera, &mut Projection), With<MinimapCamera>>();
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
