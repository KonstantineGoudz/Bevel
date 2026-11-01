use super::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::math::Dir3;
use bevy::window::WindowResolution;

mod gizmo;
mod physics;
mod route;
mod save;
mod state;
mod touch;
mod transform;

/// The default camera eye, looking wherever `target` is.
fn ray_towards(target: Vec3) -> Ray3d {
    let eye = Vec3::new(3.5, 5.9, 3.1);
    Ray3d::new(eye, Dir3::new(target - eye).unwrap())
}

fn capture_input(mode: Mode, view: ViewMode) -> CaptureInput {
    CaptureInput {
        mode,
        view,
        first_person: view == ViewMode::First,
        over_viewport: true,
        tab: false,
        focus_lost: false,
    }
}

fn test_route(valid: bool) -> Route {
    Route {
        track: path::Track::new(vec![Vec2::ZERO, Vec2::X]),
        trail: Vec::new(),
        line: Vec::new(),
        distances: vec![1.0],
        controls: vec![path::AnchorControls {
            incoming: None,
            outgoing: None,
        }],
        valid,
        hexes: 2,
    }
}

/// A window plus the touch resources `touch_input` reads. `scale_factor`
/// mimics a high-DPR phone display.
fn touch_app(scale_factor: f32) -> (App, Entity) {
    let mut app = App::new();
    app.init_resource::<Touches>();
    app.insert_resource(TouchState {
        enabled: true,
        ui_scale: 1.0,
        ..default()
    });
    app.insert_resource(Time::<()>::default());
    app.add_message::<bevy::input::touch::TouchInput>();
    // A real schedule keeps the message reader's cursor between frames;
    // `run_system_once` would replay every earlier touch each call.
    app.add_systems(PreUpdate, bevy::input::touch::touch_screen_input_system);
    app.add_systems(Update, touch_input);
    let window = app
        .world_mut()
        .spawn((
            Window {
                resolution: WindowResolution::new(
                    (800.0 * scale_factor) as u32,
                    (600.0 * scale_factor) as u32,
                )
                .with_scale_factor_override(scale_factor),
                ..default()
            },
            PrimaryWindow,
        ))
        .id();
    (app, window)
}

fn send_touch(
    app: &mut App,
    window: Entity,
    phase: bevy::input::touch::TouchPhase,
    position: Vec2,
) {
    app.world_mut()
        .resource_mut::<Messages<bevy::input::touch::TouchInput>>()
        .write(bevy::input::touch::TouchInput {
            phase,
            position,
            window,
            force: None,
            id: 1,
        });
    app.update();
}

/// A world with the invisible floor and a character, without any of the
/// rendering or UI plugins.
fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        TransformPlugin,
        PhysicsPlugins::new(PostUpdate),
    ));
    app.init_asset::<Mesh>();
    app.init_resource::<SpatialQueryDiagnostics>();
    app.init_resource::<avian3d::collider_tree::ColliderTreeDiagnostics>();
    app.init_resource::<avian3d::collision::CollisionDiagnostics>();
    app.init_resource::<avian3d::dynamics::solver::SolverDiagnostics>();
    app.insert_resource(Kit {
        parts: Vec::new(),
        mats: Vec::new(),
        glass: Handle::default(),
        ghost: Handle::default(),
        pill: Handle::default(),
        pill_mat: Handle::default(),
        character_collider: Collider::capsule(PILL_RADIUS, PILL_CYLINDER),
    });
    app.init_resource::<Editor>();
    app.init_resource::<Orbit>();
    app.init_resource::<TouchState>();
    app.init_resource::<ButtonInput<KeyCode>>();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(20.0, 0.2, 20.0),
        Position::from_xyz(0.0, -0.1, 0.0),
    ));
    app.world_mut().spawn(CharacterVisual);
    // Let the physics pipeline build its broad phase for the floor.
    app.update();
    app.update();
    app
}

fn tick_character(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.world_mut()
        .run_system_once(character_move)
        .expect("character move");
}

fn test_piece(id: u32) -> Piece {
    Piece {
        id,
        shape: model::shape_index("block").unwrap(),
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
    }
}
