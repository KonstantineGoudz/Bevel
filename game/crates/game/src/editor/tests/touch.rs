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
        touch_action_at(Vec2::new(352.0, 25.0), css),
        Some(TouchAction::Course)
    );
    assert_eq!(
        touch_action_at(Vec2::new(300.0, 25.0), css),
        Some(TouchAction::Plan)
    );
    assert_eq!(
        touch_action_at(Vec2::new(370.0, 25.0), css),
        Some(TouchAction::Play)
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
        touch_action_at(Vec2::new(352.0, 25.0), css),
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
