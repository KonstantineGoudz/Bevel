use super::*;

#[test]
fn the_redo_touch_button_authors_the_level() {
    assert!(touch_is_authoring(TouchAction::Redo));
}

#[test]
fn touch_suppress_blocks_a_tap_after_a_button() {
    let touch = TouchState {
        suppress_until: 2.0,
        ..Default::default()
    };
    assert!(!tap_allowed(1.9, &touch));
    assert!(tap_allowed(2.1, &touch));
    let multi = TouchState {
        multi: true,
        ..Default::default()
    };
    assert!(!tap_allowed(3.0, &multi));
}

/// Touch positions arrive from Bevy in logical pixels, the same space as
/// picking and the camera viewport. On a DPR-3 phone they must not be
/// divided again, or every tap lands a third of the way to the origin.
#[test]
fn a_tap_keeps_its_logical_position_on_a_high_dpr_screen() {
    let (mut app, window) = touch_app(3.0);
    let tap = Vec2::new(420.0, 380.0);
    send_touch(
        &mut app,
        window,
        bevy::input::touch::TouchPhase::Started,
        tap,
    );
    send_touch(&mut app, window, bevy::input::touch::TouchPhase::Ended, tap);

    assert_eq!(app.world().resource::<TouchState>().tap, Some(tap));
}

/// The thumbstick hit-test and deflection use the same logical touch
/// positions, so a DPR-3 press on the drawn pad must still grab it.
#[test]
fn the_stick_grabs_a_logical_touch_on_a_high_dpr_screen() {
    let (mut app, window) = touch_app(3.0);
    let centre = Vec2::new(86.0, 600.0 - 86.0);
    send_touch(
        &mut app,
        window,
        bevy::input::touch::TouchPhase::Started,
        centre,
    );
    send_touch(
        &mut app,
        window,
        bevy::input::touch::TouchPhase::Moved,
        centre + Vec2::new(STICK_TRAVEL, 0.0),
    );

    assert_eq!(
        app.world().resource::<TouchState>().stick,
        Vec2::new(1.0, 0.0)
    );
}

#[test]
fn the_touch_layout_owns_its_control_zones() {
    let size = Vec2::new(390.0, 844.0);
    assert!(over_controls(Vec2::new(100.0, 20.0), size, 1.0), "toolbar");
    assert!(over_controls(Vec2::new(60.0, 800.0), size, 1.0), "stick");
    assert!(over_controls(Vec2::new(340.0, 800.0), size, 1.0), "jump");
    assert!(!over_controls(Vec2::new(195.0, 420.0), size, 1.0), "canvas");
}

#[test]
fn grabbing_the_stick_wins_over_the_control_zone() {
    let size = Vec2::new(390.0, 844.0);
    let s = 0.5;
    let centre = Vec2::new(
        (STICK_MARGIN + STICK_BASE * 0.5) * s,
        size.y - (STICK_MARGIN + STICK_BASE * 0.5) * s,
    );
    // The pad deliberately sits inside the bottom-left control zone; a
    // press on it must still grab the stick rather than be ignored.
    assert!(over_controls(centre, size, s), "the pad is a control");
    assert!(grabs_stick(centre, centre, STICK_REACH * s), "pad centre");
    assert!(grabs_stick(
        centre + Vec2::new(STICK_REACH * s * 0.9, 0.0),
        centre,
        STICK_REACH * s
    ));
    assert!(!grabs_stick(
        centre + Vec2::new(STICK_REACH * s * 1.5, 0.0),
        centre,
        STICK_REACH * s
    ));
}

#[test]
fn touch_buttons_include_plan_and_add() {
    let css = Vec2::new(390.0, 844.0);
    assert_eq!(
        touch_action_at(Vec2::new(310.0, 25.0), css),
        Some(TouchAction::Course)
    );
    assert_eq!(
        touch_action_at(Vec2::new(270.0, 25.0), css),
        Some(TouchAction::Plan)
    );
    assert_eq!(
        touch_action_at(Vec2::new(350.0, 25.0), css),
        Some(TouchAction::Play)
    );
    assert_eq!(
        touch_action_at(Vec2::new(383.0, 25.0), css),
        Some(TouchAction::Search)
    );
    assert_eq!(
        touch_action_at(Vec2::new(40.0, 100.0), css),
        Some(TouchAction::Add)
    );
}

#[test]
fn touch_buttons_are_hit_by_css_position() {
    let css = Vec2::new(390.0, 844.0);
    assert_eq!(
        touch_action_at(Vec2::new(37.0, 25.0), css),
        Some(TouchAction::View)
    );
    assert_eq!(
        touch_action_at(Vec2::new(310.0, 25.0), css),
        Some(TouchAction::Course)
    );
    assert_eq!(
        touch_action_at(Vec2::new(336.0, 63.0), css),
        Some(TouchAction::Place)
    );
    assert_eq!(
        touch_action_at(Vec2::new(53.0, 63.0), css),
        Some(TouchAction::PrevShape)
    );
    assert_eq!(
        touch_action_at(Vec2::new(328.0, 778.0), css),
        Some(TouchAction::Jump)
    );
    assert_eq!(touch_action_at(Vec2::new(200.0, 400.0), css), None);
}

#[test]
fn cluster_buttons_map_jump_once_and_toggle_run_slide() {
    let mut touch = TouchState::default();
    apply_touch_button(&mut touch, Some(Action::Jump));
    assert!(touch.jump);
    apply_touch_button(&mut touch, Some(Action::Sprint));
    assert!(touch.sprint);
    apply_touch_button(&mut touch, Some(Action::Sprint));
    assert!(!touch.sprint, "run toggles back off");
    apply_touch_button(&mut touch, Some(Action::Slide));
    assert!(touch.slide);
    apply_touch_button(&mut touch, Some(Action::Interact));
    assert!(touch.interact);
    apply_touch_button(&mut touch, None);
    assert!(touch.jump && touch.interact && touch.slide && !touch.sprint);
}

#[test]
fn the_stick_maps_screen_to_walk_direction() {
    let centre = Vec2::new(86.0, 758.0);
    assert_eq!(stick_offset(centre, centre, 1.0), Vec2::ZERO);

    let right = stick_offset(centre + Vec2::new(44.0, 0.0), centre, 1.0);
    assert!((right - Vec2::X).length() < 1e-4, "{right}");

    // Screen up is +y in walk space.
    let up = stick_offset(centre - Vec2::new(0.0, 44.0), centre, 1.0);
    assert!((up - Vec2::Y).length() < 1e-4, "{up}");

    let far = stick_offset(centre + Vec2::new(300.0, 0.0), centre, 1.0);
    assert!((far.length() - 1.0).abs() < 1e-4);
}

/// A finger can grab a gizmo handle and drag the piece, then release it.
/// Regression test: gizmo grab and drive used to require mouse buttons, so
/// touch could select pieces but never move them with the handles.
#[test]
fn a_finger_can_grab_and_drag_a_gizmo_handle() {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::camera::{RenderTargetInfo, Viewport};
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{Pointer, PointerDrag, PointerPress, PointerRelease};
    use bevy::picking::pointer::Location;

    let mut app = physics_app();
    app.init_resource::<Touches>();
    app.init_resource::<PathGesture>();
    app.init_resource::<Movers>();
    app.init_resource::<Nav>();
    app.init_resource::<PointerCapture>();
    app.world_mut().resource_mut::<TouchState>().enabled = true;
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        },
        PrimaryWindow,
    ));
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(test_piece(1));
        editor.selected = Some(1);
    }
    // A headless camera with explicit viewport math (no renderer needed):
    // projection and viewport are consistent, which is all the hit test
    // and the drag ray need.
    let eye = Vec3::new(6.0, 7.0, 6.0);
    let cam_transform = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
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
    app.world_mut()
        .spawn((camera, Camera3d::default(), cam_transform));
    let canvas = app
        .world_mut()
        .spawn(Node::default())
        .observe(on_path_press)
        .observe(on_gizmo_move)
        .observe(on_gizmo_drag)
        .observe(on_gizmo_release)
        .observe(on_gizmo_drag_end)
        .observe(on_gizmo_cancel)
        .id();

    // Press exactly on the Y handle tip.
    let tip = {
        let piece = test_piece(1);
        let shape = &shapes()[piece.shape];
        gizmo_origin(&piece, shape) + Vec3::Y * gizmo_frame(&piece, shape).0
    };
    let press_pos = {
        let world = app.world_mut();
        let (camera, transform) = world
            .query::<(&Camera, &Transform)>()
            .single(world)
            .expect("camera");
        camera
            .world_to_viewport(&GlobalTransform::from(*transform), tip)
            .expect("handle on screen")
    };
    let finger = PointerId::Touch(7);
    let pointing = |position| {
        Pointer::new(
            finger,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 800,
                    height: 600,
                },
                position,
            },
        )
    };
    app.world_mut().trigger(PointerPress {
        entity: canvas,
        pointer: pointing(press_pos),
        button: PointerButton::Primary,
        hit: HitData::new(canvas, 0.0, None, None),
        count: 1,
    });
    let drag = app
        .world()
        .resource::<Editor>()
        .gizmo_drag
        .expect("finger grabs the handle");
    assert_eq!(drag.pointer, finger);
    assert_eq!(drag.handle, GizmoHandle::Y);

    // Drag straight up, then drive one frame like the live schedule.
    let moved_pos = press_pos + Vec2::new(0.0, -80.0);
    app.world_mut().trigger(PointerDrag {
        entity: canvas,
        pointer: pointing(moved_pos),
        button: PointerButton::Primary,
        distance: Vec2::new(0.0, -80.0),
        delta: Vec2::new(0.0, -80.0),
    });
    app.world_mut()
        .run_system_once(path_drag)
        .expect("drive gizmo");
    assert!(
        app.world().resource::<Editor>().pieces[0].elevation > 0.05,
        "elevation = {}",
        app.world().resource::<Editor>().pieces[0].elevation
    );

    // Release ends the drag and keeps the pose.
    app.world_mut().trigger(PointerRelease {
        entity: canvas,
        pointer: pointing(moved_pos),
        button: PointerButton::Primary,
        hit: HitData::new(canvas, 0.0, None, None),
    });
    let editor = app.world().resource::<Editor>();
    assert!(editor.gizmo_drag.is_none());
    assert!(editor.hard_dirty);
    assert!(editor.pieces[0].elevation > 0.05);
}

/// Pressing the negative end of an axis grabs the same handle, so a piece
/// can be shoved from either side on touch just as with the mouse.
#[test]
fn a_finger_can_grab_the_negative_end_of_an_axis() {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::camera::{RenderTargetInfo, Viewport};
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{PointerPress, PointerRelease};
    use bevy::picking::pointer::Location;

    let mut app = physics_app();
    app.init_resource::<Touches>();
    app.init_resource::<PathGesture>();
    app.init_resource::<Movers>();
    app.init_resource::<Nav>();
    app.init_resource::<PointerCapture>();
    app.world_mut().resource_mut::<TouchState>().enabled = true;
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        },
        PrimaryWindow,
    ));
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(test_piece(1));
        editor.selected = Some(1);
    }
    let eye = Vec3::new(6.0, 7.0, 6.0);
    let cam_transform = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
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
    app.world_mut()
        .spawn((camera, Camera3d::default(), cam_transform));
    let canvas = app
        .world_mut()
        .spawn(Node::default())
        .observe(on_path_press)
        .observe(on_gizmo_release)
        .id();

    // Press exactly on the negative X tip.
    let tip = {
        let piece = test_piece(1);
        let shape = &shapes()[piece.shape];
        gizmo_origin(&piece, shape) - Vec3::X * gizmo_frame(&piece, shape).0
    };
    let press_pos = {
        let world = app.world_mut();
        let (camera, transform) = world
            .query::<(&Camera, &Transform)>()
            .single(world)
            .expect("camera");
        camera
            .world_to_viewport(&GlobalTransform::from(*transform), tip)
            .expect("handle on screen")
    };
    let finger = PointerId::Touch(11);
    let pointing = |position| {
        Pointer::new(
            finger,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 800,
                    height: 600,
                },
                position,
            },
        )
    };
    app.world_mut().trigger(PointerPress {
        entity: canvas,
        pointer: pointing(press_pos),
        button: PointerButton::Primary,
        hit: HitData::new(canvas, 0.0, None, None),
        count: 1,
    });
    let drag = app
        .world()
        .resource::<Editor>()
        .gizmo_drag
        .expect("finger grabs the negative handle");
    assert_eq!(drag.pointer, finger);
    assert_eq!(drag.handle, GizmoHandle::X);

    app.world_mut().trigger(PointerRelease {
        entity: canvas,
        pointer: pointing(press_pos),
        button: PointerButton::Primary,
        hit: HitData::new(canvas, 0.0, None, None),
    });
    assert!(app.world().resource::<Editor>().gizmo_drag.is_none());
}

#[test]
fn the_pad_cluster_is_excluded_from_the_gizmo_fallback() {
    // Bottom-right pads at right 16..178 / bottom 20..182 (layout px): the
    // raw-touch gizmo fallback must never steal their taps.
    let size = Vec2::new(390.0, 844.0);
    let s = 0.5;
    assert!(over_pad_cluster(Vec2::new(390.0 - 20.0, 844.0 - 30.0), size, s));
    assert!(over_pad_cluster(Vec2::new(390.0 - 60.0, 844.0 - 80.0), size, s));
    assert!(!over_pad_cluster(Vec2::new(195.0, 420.0), size, s));
    assert!(!over_pad_cluster(Vec2::new(390.0 - 200.0, 844.0 - 200.0), size, s));
}

/// The raw-touch fallback grabs a gizmo handle even when UI picking never
/// delivers the press to the canvas (no canvas observers are registered
/// here at all): this is the path real mobile browsers take when the press
/// lands on overlapping UI.
#[test]
fn raw_touch_grabs_a_gizmo_without_pointer_events() {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::camera::{RenderTargetInfo, Viewport};
    use bevy::input::touch::TouchPhase;

    let (mut app, window) = touch_app(1.0);
    app.init_resource::<Editor>();
    app.init_resource::<PathGesture>();
    app.init_resource::<PointerCapture>();
    app.init_resource::<AddMenu>();
    app.init_resource::<InputFocus>();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(test_piece(1));
        editor.selected = Some(1);
    }
    let eye = Vec3::new(6.0, 7.0, 6.0);
    let cam_transform = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
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
    app.world_mut()
        .spawn((camera, Camera3d::default(), cam_transform));
    // Deliberately no canvas and no observers: the press below only exists
    // as a raw touch, like a press UI picking swallows on device.
    app.add_systems(Update, touch_gizmo_fallback.after(touch_input));

    let tip = {
        let piece = test_piece(1);
        let shape = &shapes()[piece.shape];
        gizmo_origin(&piece, shape) + Vec3::X * gizmo_frame(&piece, shape).0
    };
    let press_pos = {
        let world = app.world_mut();
        let (camera, transform) = world
            .query::<(&Camera, &Transform)>()
            .single(world)
            .expect("camera");
        camera
            .world_to_viewport(&GlobalTransform::from(*transform), tip)
            .expect("handle on screen")
    };
    send_touch(&mut app, window, TouchPhase::Started, press_pos);

    let editor = app.world().resource::<Editor>();
    let drag = editor.gizmo_drag.expect("raw touch grabs the handle");
    assert_eq!(drag.handle, GizmoHandle::X);
    assert_eq!(drag.pointer, PointerId::Touch(1));
    let touch = app.world().resource::<TouchState>();
    assert!(
        touch.tracks.get(&1).is_some_and(|track| track.control),
        "the grabbing finger must not also orbit"
    );

    // Drag the finger with still no pointer events: the drag follows the
    // raw track, so it never depends on canvas-targeted move events.
    let moved_pos = press_pos + Vec2::new(40.0, 0.0);
    send_touch(&mut app, window, TouchPhase::Moved, moved_pos);
    let screen = app
        .world()
        .resource::<Editor>()
        .gizmo_drag
        .expect("drag continues")
        .screen;
    assert!(
        (screen - moved_pos).length() < 1e-4,
        "drag follows the finger, screen = {screen}"
    );

    // Lift the finger with still no release event: the drag must end here,
    // or it would stick and block selection and the camera.
    send_touch(&mut app, window, TouchPhase::Ended, moved_pos);
    let editor = app.world().resource::<Editor>();
    assert!(editor.gizmo_drag.is_none());
    assert!(editor.hard_dirty);
}
