use super::*;

#[test]
fn build_and_play_modes_gate_object_editing() {
    assert_eq!(Mode::default(), Mode::Build);
    assert_eq!(Mode::Build.toggled(), Mode::Play);
    assert_eq!(Mode::Play.toggled(), Mode::Build);
    assert!(object_editing_allowed(Mode::Build));
    assert!(!object_editing_allowed(Mode::Play));
}

#[test]
fn first_person_grabs_the_pointer_and_tab_releases_it() {
    let build = Mode::Build;
    let third = ViewMode::Third;
    let first = ViewMode::First;

    // Switching into first person over the view grabs the pointer.
    assert!(next_capture(
        false,
        build,
        third,
        capture_input(build, first)
    ));
    // Doing it with the pointer over the palette does not.
    assert!(!next_capture(
        false,
        build,
        third,
        CaptureInput {
            over_viewport: false,
            ..capture_input(build, first)
        }
    ));

    // Tab lets go without leaving first person.
    assert!(!next_capture(
        true,
        build,
        first,
        CaptureInput {
            tab: true,
            ..capture_input(build, first)
        }
    ));
    // Tab over the view takes it back; over the palette it does not.
    assert!(next_capture(
        false,
        build,
        first,
        CaptureInput {
            tab: true,
            ..capture_input(build, first)
        }
    ));
    assert!(!next_capture(
        false,
        build,
        first,
        CaptureInput {
            tab: true,
            over_viewport: false,
            ..capture_input(build, first)
        }
    ));

    // Leaving first person and losing focus both release it.
    assert!(!next_capture(
        true,
        build,
        first,
        CaptureInput {
            first_person: false,
            ..capture_input(build, third)
        }
    ));
    assert!(!next_capture(
        true,
        build,
        first,
        CaptureInput {
            focus_lost: true,
            ..capture_input(build, first)
        }
    ));
    // Regaining focus does not grab by itself.
    assert!(!next_capture(
        false,
        build,
        first,
        capture_input(build, first)
    ));
}

#[test]
fn play_keeps_first_person_capture_and_build_releases_it() {
    let first = ViewMode::First;
    let third = ViewMode::Third;

    assert!(next_capture(
        true,
        Mode::Build,
        first,
        capture_input(Mode::Play, first)
    ));
    assert!(!next_capture(
        true,
        Mode::Play,
        first,
        capture_input(Mode::Build, first)
    ));
    // Play in third person, or on touch, never grabs.
    assert!(!next_capture(
        false,
        Mode::Build,
        third,
        capture_input(Mode::Play, third)
    ));
    assert!(!next_capture(
        true,
        Mode::Build,
        first,
        CaptureInput {
            first_person: false,
            ..capture_input(Mode::Build, first)
        }
    ));
}

#[test]
fn the_palette_is_not_the_viewport_but_all_of_play_is() {
    let mut editor = Editor::default();
    let mut touch = TouchState::default();
    let on_palette = Some(Vec2::new(10.0, 10.0));
    let on_view = Some(Vec2::new(PANEL_WIDTH + 1.0, 10.0));

    assert!(!over_viewport(&editor, &touch, on_palette));
    assert!(over_viewport(&editor, &touch, on_view));
    assert!(!over_viewport(&editor, &touch, None));

    // Play hides the palette, so the whole screen can be looked through.
    editor.mode = Mode::Play;
    assert!(over_viewport(&editor, &touch, on_palette));

    // Touch has no captured pointer.
    editor.mode = Mode::Build;
    touch.enabled = true;
    assert!(!over_viewport(&editor, &touch, on_view));
}

#[test]
fn the_mode_overlays_follow_the_mode_and_input_layer() {
    let build_desktop = ModeUi {
        desktop: true,
        play: false,
    };
    assert!(layer_matches(build_desktop, Mode::Build, false));
    assert!(!layer_matches(build_desktop, Mode::Play, false));
    assert!(!layer_matches(build_desktop, Mode::Build, true));

    let play_touch = ModeUi {
        desktop: false,
        play: true,
    };
    assert!(layer_matches(play_touch, Mode::Play, true));
    assert!(!layer_matches(play_touch, Mode::Build, true));
    assert!(!layer_matches(play_touch, Mode::Play, false));
}

#[test]
fn play_mode_only_allows_non_authoring_ui_actions() {
    assert!(!palette_is_authoring(PaletteAction::Build));
    assert!(!palette_is_authoring(PaletteAction::Play));
    assert!(!palette_is_authoring(PaletteAction::View));
    assert!(!palette_is_authoring(PaletteAction::CopyJson));
    assert!(palette_is_authoring(PaletteAction::Character));
    assert!(palette_is_authoring(PaletteAction::Shape(0)));
    assert!(palette_is_authoring(PaletteAction::Course));

    assert!(!touch_is_authoring(TouchAction::Play));
    assert!(!touch_is_authoring(TouchAction::View));
    assert!(!touch_is_authoring(TouchAction::Jump));
    assert!(!touch_is_authoring(TouchAction::Interact));
    assert!(!touch_is_authoring(TouchAction::Pad(ClusterButton::A)));
    assert!(!touch_is_authoring(TouchAction::CopyJson));
    assert!(touch_is_authoring(TouchAction::Path));
    assert!(touch_is_authoring(TouchAction::Place));
    assert!(touch_is_authoring(TouchAction::Grass));
    assert!(touch_is_authoring(TouchAction::GrassErase));
    assert!(palette_is_authoring(PaletteAction::Grass));
    assert!(palette_is_authoring(PaletteAction::GrassErase));
    assert!(palette_is_authoring(PaletteAction::GrassSlow));
}

#[test]
fn every_catalogue_shape_gets_a_collider() {
    for shape in shapes() {
        if shape.is_trigger {
            assert!(
                piece_collider(shape).is_none(),
                "{} trigger should have no collider",
                shape.id
            );
        } else {
            assert!(
                piece_collider(shape).is_some(),
                "{} has no collider",
                shape.id
            );
        }
    }
}
