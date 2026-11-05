use super::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::math::Dir3;
use bevy::window::WindowResolution;

mod gizmo;
mod panel;
mod physics;
mod route;
mod save;
mod selection;
mod shooting;
mod boot;
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
    app.init_resource::<Editor>();
    app.init_resource::<OutlinerState>();
    app.init_resource::<PropertiesPanelState>();
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
        RapierPhysicsPlugin::<NoUserData>::default(),
    ));
    app.init_asset::<Mesh>();
    app.init_resource::<ButtonInput<MouseButton>>();
    app.init_resource::<crate::controls::Controls>();
    app.insert_resource(Kit {
        parts: Vec::new(),
        mats: Vec::new(),
        glass: Handle::default(),
        ghost: Handle::default(),
        pill: Handle::default(),
        pill_mat: Handle::default(),
        cone: Handle::default(),
        cone_mat: Handle::default(),
        grass: Handle::default(),
        grass_mats: [Handle::default(), Handle::default()],
        // Rapier takes half height first: capsule_y(half_height, radius).
        character_collider: Collider::capsule_y(PILL_CYLINDER * 0.5, PILL_RADIUS),
    });
    app.init_resource::<Editor>();
    app.init_resource::<Orbit>();
    // resolve_dynamic_push reads both every tick; live app always has them.
    app.init_resource::<Movers>();
    app.init_resource::<Visuals>();
    app.init_resource::<TouchState>();
    app.init_resource::<AddMenu>();
    app.init_resource::<ButtonInput<KeyCode>>();
    app.world_mut().spawn((
        RigidBody::Fixed,
        Collider::cuboid(10.0, 0.1, 10.0),
        Transform::from_xyz(0.0, -0.1, 0.0),
    ));
    app.init_resource::<InputFocus>();
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

fn tick_walk(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.world_mut().run_system_once(walk).expect("walk");
}

/// Movers run before the character, like the live schedule: platforms carry
/// the rider, then physics jumps or lands on the carried pose. The final
/// depenetration runs last, also like the live schedule.
fn tick_rider(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.world_mut()
        .run_system_once(move_movers)
        .expect("move movers");
    app.world_mut()
        .run_system_once(character_move)
        .expect("character move");
    app.world_mut()
        .run_system_once(resolve_platform_push)
        .expect("resolve platform push");
    app.world_mut()
        .run_system_once(resolve_dynamic_push)
        .expect("resolve dynamic push");
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
        dynamic: false,
        trigger: crate::model::Trigger::harmless(),
        trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
        trigger_shape_override: crate::model::TriggerShape::Box,
        trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
        trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
        trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        interact: crate::model::InteractAction::None,
    }
}
