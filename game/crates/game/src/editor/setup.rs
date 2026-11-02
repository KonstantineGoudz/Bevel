//! setup systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// On a DPR-2 phone the logical viewport is half the CSS size; shrink the UI
/// by the same factor so it keeps its CSS proportions, and remember it so
/// touch coordinates can be mapped into logical space.
pub(super) fn setup_ui_scale(
    window: Single<&Window, With<PrimaryWindow>>,
    mut ui_scale: ResMut<UiScale>,
    mut touch: ResMut<TouchState>,
    mut applied: ResMut<UiScaleApplied>,
) {
    if applied.0 {
        return;
    }
    let scale = window.scale_factor();
    let css_width = window.width() * scale;
    if scale > 1.0 && css_width < 700.0 {
        applied.0 = true;
        ui_scale.0 = 1.0 / scale;
        touch.ui_scale = 1.0 / scale;
    }
}

pub(super) fn load_session(
    boot: Option<Res<session::Boot>>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut movers: ResMut<Movers>,
) {
    let Some(boot) = boot else {
        return;
    };
    if let (Some(id), Some(loaded)) = (&boot.id, boot.loaded.clone()) {
        let had_camera = loaded.camera.is_some();
        editor.session = Some(id.clone());
        editor.apply_loaded(&mut orbit, &mut movers, loaded);
        // If the saved camera was missing or looks at empty space, frame the
        // level so a newly opened map doesn't start on a black void. The user
        // described a black screen that only cleared after a right-drag, which
        // matches an orbit target far from any piece.
        if !had_camera || editor.pieces.is_empty() {
            // No correction needed; default orbit already looks at origin.
        } else {
            let target2 = Vec2::new(orbit.target.x, orbit.target.z);
            let mut closest = f32::MAX;
            let mut sum = Vec2::ZERO;
            for piece in &editor.pieces {
                let d = piece.pos.distance(target2);
                if d < closest {
                    closest = d;
                }
                sum += piece.pos;
            }
            // If the saved target is more than ~2× the viewing distance away
            // from every piece, it likely points at void (e.g. an older save
            // or a map moved in the index). Frame the content instead.
            if closest > orbit.distance * 2.0 + 10.0 {
                if let Some(character) = editor.character {
                    orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
                    orbit.yaw = character.yaw;
                } else {
                    let center = sum / editor.pieces.len() as f32;
                    orbit.target = Vec3::new(center.x, 0.0, center.y);
                }
                // Pull back enough to see a scattered map.
                orbit.distance = orbit.distance.max(12.0).min(30.0);
            }
        }
    }
}

pub(super) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    orbit: Res<Orbit>,
) {
    let eye = orbit.target
        + Vec3::new(
            orbit.pitch.cos() * orbit.yaw.sin(),
            orbit.pitch.sin(),
            orbit.pitch.cos() * orbit.yaw.cos(),
        ) * orbit.distance;
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60.0_f32.to_radians(),
            near: 0.05,
            far: 400.0,
            ..default()
        }),
        Transform::from_translation(eye).looking_at(orbit.target, Vec3::Y),
    ));

    commands.insert_resource(ClearColor(Color::srgb_u8(0x0a, 0x0d, 0x12)));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb_u8(0xb8, 0xc8, 0xe0),
        brightness: 280.0,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            illuminance: 7_500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(7.0, 12.0, 6.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));

    // Invisible floor under the whole grid: the character always has something
    // to stand on, so walking off a tile does not drop it into the void.
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(160.0, 0.2, 160.0),
        Transform::from_xyz(0.0, -0.1, 0.0),
    ));
    // Solid geometry: one mesh set per catalogue entry.
    let glass = materials.add(StandardMaterial {
        base_color: Color::srgba(0.60, 0.80, 0.90, 0.26),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.15,
        ..default()
    });
    let ghost = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.78, 0.38, 0.34),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });

    let mut parts = Vec::with_capacity(shapes().len());
    let mut mats = Vec::with_capacity(shapes().len());
    for shape in shapes() {
        mats.push(materials.add(StandardMaterial {
            base_color: shape.color,
            perceptual_roughness: 0.85,
            metallic: 0.0,
            ..default()
        }));
        let mut made = Vec::new();
        if shape.is_trigger {
            let mesh: Mesh = match shape.trigger_shape {
                model::TriggerShape::Box => Cuboid::new(1.0, 1.0, 1.0).into(),
                model::TriggerShape::Sphere => Sphere::new(0.5).mesh().ico(4).unwrap().into(),
                model::TriggerShape::Cylinder => Cylinder::new(0.5, 1.0).into(),
            };
            made.push((meshes.add(mesh), 0));
        } else if let model::Geometry::Wedge { w, d, h } = &shape.geom {
            made.push((meshes.add(wedge(*w, *d, *h)), 0));
        } else {
            for (outline, base, height, slot) in solid_parts(shape) {
                made.push((meshes.add(extrude(&outline, base, height)), slot));
            }
        }
        parts.push(made);
    }
    let pill = meshes.add(Capsule3d::new(0.32, 1.1));
    let pill_mat = materials.add(StandardMaterial {
        base_color: PILL_COLOR,
        perceptual_roughness: 0.35,
        metallic: 0.0,
        ..default()
    });
    commands.insert_resource(Kit {
        parts,
        mats,
        glass,
        ghost,
        pill,
        pill_mat,
        character_collider: Collider::capsule(PILL_RADIUS, PILL_CYLINDER),
    });

    let panel = commands
        .spawn((
            InputLayer { desktop: true },
            ModeUi {
                desktop: true,
                play: false,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: px(PANEL_WIDTH),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10)),
                row_gap: px(1),
                border: UiRect::right(px(1)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BORDER),
        ))
        .id();

    // A single Play button shares the COMPONENTS heading row so starting play
    // costs no extra vertical space in the narrow panel.
    let mode_row = commands
        .spawn(Node {
            width: px(64),
            flex_direction: FlexDirection::Row,
            ..default()
        })
        .id();
    let play = palette_compact_button(&mut commands, PaletteAction::Play, "Play");
    commands.entity(mode_row).add_child(play);

    let header = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            column_gap: px(6),
            margin: UiRect::bottom(px(6)),
            ..default()
        })
        .id();
    let title = commands
        .spawn((
            Text::new("COMPONENTS"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    commands.entity(header).add_children(&[title, mode_row]);

    let mut children = vec![header];

    for category in CATEGORIES {
        children.push(palette_label(&mut commands, &category.to_uppercase()));

        for (index, shape) in shapes().iter().enumerate() {
            if shape.category != category {
                continue;
            }
            children.push(palette_button(
                &mut commands,
                PaletteAction::Shape(index),
                shape.name,
            ));
        }
    }

    children.push(palette_label(&mut commands, "ACTOR"));
    let actor_row = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(4),
            ..default()
        })
        .id();
    let actor_buttons = [
        palette_compact_button(&mut commands, PaletteAction::Character, "Char"),
        palette_compact_button(&mut commands, PaletteAction::Path, "Path"),
        palette_compact_button(&mut commands, PaletteAction::Plan, "Plan"),
        palette_compact_button(&mut commands, PaletteAction::Course, "Course"),
        palette_compact_button(&mut commands, PaletteAction::CopyJson, "JSON"),
    ];
    commands.entity(actor_row).add_children(&actor_buttons);
    children.push(actor_row);

    let path_actions = commands
        .spawn((
            PathActions,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                display: Display::None,
                ..default()
            },
        ))
        .id();
    let path_buttons = [
        palette_compact_button(&mut commands, PaletteAction::Walk, "Walk"),
        palette_compact_button(&mut commands, PaletteAction::Smooth, "Smooth"),
    ];
    commands.entity(path_actions).add_children(&path_buttons);
    children.push(path_actions);

    commands.entity(panel).add_children(&children);

    let status_bar = commands
        .spawn((
            InputLayer { desktop: true },
            Node {
                position_type: PositionType::Absolute,
                left: px(PANEL_WIDTH + 4.0),
                bottom: px(4.0),
                padding: UiRect::axes(px(10), px(5)),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            },
            BackgroundColor(BAR_BG),
        ))
        .id();
    let status_text = commands
        .spawn((
            StatusText,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    commands.entity(status_bar).add_child(status_text);

    let hint_bar = commands
        .spawn((
            InputLayer { desktop: true },
            ModeUi {
                desktop: true,
                play: false,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(PANEL_WIDTH + 4.0),
                top: px(4.0),
                padding: UiRect::axes(px(10), px(4)),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            },
            BackgroundColor(BAR_BG),
        ))
        .id();
    let hint_text = commands
        .spawn((
            Text::new(
                "LMB place / select / drag  |  Y lifts, plane squares slide  |  rings pitch / yaw / roll  |  Ctrl+C / Ctrl+V copy & paste  |  Ctrl+Z undo, Ctrl+Shift+Z / Ctrl+Y redo  |  Path: click route, Shift append  |  Enter walk  |  drag waypoints / curve handles  |  RMB orbit (selection in Build)  |  MMB pan  |  wheel zoom  |  V view  |  Tab mouse  |  P play / build  |  C character  |  F3 add menu (not first person)  |  G move / R rotate  |  X Y Z lock axis  |  Enter / LMB confirm  |  Esc / RMB cancel  |  Shift+R 30 deg  |  S save  |  Shift+S copy JSON",
            ),
            TextFont { font_size: FontSize::Px(11.0), ..default() },
            TextColor(Color::srgb_u8(0x6b, 0x7d, 0x93)),
        ))
        .id();
    commands.entity(hint_bar).add_child(hint_text);

    // Play replaces the editor palette with a single exit control; a View
    // button keeps camera cycling reachable without the keyboard.
    let play_bar = commands
        .spawn((
            InputLayer { desktop: true },
            ModeUi {
                desktop: true,
                play: true,
            },
            Node {
                position_type: PositionType::Absolute,
                right: px(8),
                top: px(8),
                width: px(230),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                ..default()
            },
        ))
        .id();
    let play_buttons = [
        palette_compact_button(&mut commands, PaletteAction::View, "View"),
        palette_compact_button(&mut commands, PaletteAction::CopyJson, "JSON"),
        palette_compact_button(&mut commands, PaletteAction::Build, "Exit Play"),
    ];
    commands.entity(play_bar).add_children(&play_buttons);

    spawn_touch_ui(&mut commands);
    spawn_properties_panel(&mut commands);
    spawn_object_graph(&mut commands);
    spawn_character_status(&mut commands);
    spawn_add_menu(&mut commands);
}

/// The phone layout: a toolbar, a shape carousel, a thumbstick and a jump
/// button. Play swaps the toolbar and carousel for an exit/view row. Hidden
/// until the first touch arrives.
pub(super) fn spawn_touch_ui(commands: &mut Commands) {
    let root = commands
        .spawn((
            InputLayer { desktop: false },
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                display: Display::None,
                ..default()
            },
        ))
        .id();

    // Top toolbar.
    let tools = commands
        .spawn((
            ModeUi {
                desktop: false,
                play: false,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(8),
                right: px(8),
                top: px(8),
                height: px(34),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                ..default()
            },
        ))
        .id();
    let tool_buttons = [
        touch_button(commands, TouchAction::View, "View"),
        touch_button(commands, TouchAction::Rotate, "Rot"),
        touch_button(commands, TouchAction::Delete, "Del"),
        touch_button(commands, TouchAction::Undo, "Undo"),
        touch_button(commands, TouchAction::Redo, "Redo"),
        touch_button(commands, TouchAction::Path, "Path"),
        touch_button(commands, TouchAction::Plan, "Plan"),
        touch_button(commands, TouchAction::Course, "Course"),
        touch_button(commands, TouchAction::Play, "Play"),
    ];
    commands.entity(tools).add_children(&tool_buttons);

    // Play toolbar: just the camera cycle and the way back to edit.
    let play_tools = commands
        .spawn((
            ModeUi {
                desktop: false,
                play: true,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(8),
                right: px(8),
                top: px(8),
                height: px(34),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                ..default()
            },
        ))
        .id();
    let play_buttons = [
        touch_button(commands, TouchAction::View, "View"),
        touch_button(commands, TouchAction::CopyJson, "JSON"),
        touch_button(commands, TouchAction::Play, "Exit"),
    ];
    commands.entity(play_tools).add_children(&play_buttons);

    // Shape carousel + place toggle.
    let carousel = commands
        .spawn((
            ModeUi {
                desktop: false,
                play: false,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(8),
                right: px(8),
                top: px(46),
                height: px(34),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .id();
    let prev = touch_button(commands, TouchAction::PrevShape, "<");
    let label = commands
        .spawn((
            ShapeLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            Node {
                flex_grow: 1.0,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .id();
    let next = touch_button(commands, TouchAction::NextShape, ">");
    let place = touch_button(commands, TouchAction::Place, "Place");
    commands
        .entity(carousel)
        .add_children(&[prev, label, next, place]);

    // Path editing row: Add in Path mode; Add/Walk/Smooth in Plan mode.
    let path_row = commands
        .spawn((
            TouchPathRow,
            Node {
                position_type: PositionType::Absolute,
                left: px(8),
                right: px(8),
                top: px(88),
                height: px(34),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                display: Display::None,
                ..default()
            },
        ))
        .id();
    let path_buttons = [
        touch_button(commands, TouchAction::Add, "Add"),
        touch_button(commands, TouchAction::Walk, "Walk"),
        touch_button(commands, TouchAction::Smooth, "Smooth"),
    ];
    commands.entity(path_row).add_children(&path_buttons);

    // Compact status under the toolbar.
    let status = commands
        .spawn((
            TouchStatus,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                position_type: PositionType::Absolute,
                left: px(10),
                top: px(128),
                ..default()
            },
        ))
        .id();

    // Thumbstick.
    let base = commands
        .spawn((
            // Let the canvas behind receive the drag; the visual pad is a
            // drawing, not an input target.
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(STICK_MARGIN),
                bottom: px(STICK_MARGIN),
                width: px(STICK_BASE),
                height: px(STICK_BASE),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(percent(50)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.08, 0.11, 0.55)),
            BorderColor::all(Color::srgba(0.5, 0.7, 0.85, 0.25)),
        ))
        .id();
    let knob = commands
        .spawn((
            TouchKnob,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px((STICK_BASE - STICK_KNOB) * 0.5),
                bottom: px((STICK_BASE - STICK_KNOB) * 0.5),
                width: px(STICK_KNOB),
                height: px(STICK_KNOB),
                border_radius: BorderRadius::all(percent(50)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.25, 0.82, 0.70, 0.55)),
        ))
        .id();
    commands.entity(base).add_child(knob);

    // Jump.
    let jump = commands
        .spawn((
            Button,
            TouchButton(TouchAction::Jump),
            Node {
                position_type: PositionType::Absolute,
                right: px(20),
                bottom: px(24),
                width: px(84),
                height: px(84),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(percent(50)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.08, 0.11, 0.62)),
            BorderColor::all(Color::srgba(0.5, 0.7, 0.85, 0.25)),
            children![(
                Text::new("Jump"),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();

    // A full-screen picking layer behind the controls. Taps that reach it are
    // grid taps; taps on buttons are consumed by the button first. Using the
    // picking position keeps this in the same coordinate space as the UI.
    let canvas = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                ..default()
            },
            // Behind every control, in picking as well as in draw order, so a
            // toolbar tap never also lands on the grid behind it.
            ZIndex(-1),
            BackgroundColor(Color::NONE),
        ))
        .id();
    commands
        .entity(canvas)
        .observe(on_canvas_press)
        .observe(on_canvas_move)
        .observe(on_canvas_release)
        .observe(on_path_press)
        .observe(on_path_move)
        .observe(on_path_drag)
        .observe(on_path_release)
        .observe(on_path_drag_end)
        .observe(on_path_cancel);

    // The canvas is top-level and always pickable: desktop mouse presses need
    // the same pointer stream as touch taps.
    commands
        .entity(root)
        .add_children(&[tools, carousel, path_row, status, base, jump]);
}

pub(super) fn touch_button(commands: &mut Commands, action: TouchAction, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            TouchButton(action),
            Node {
                flex_grow: 1.0,
                flex_basis: px(0),
                height: px(34),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.08, 0.11, 0.62)),
            BorderColor::all(Color::srgba(0.5, 0.7, 0.85, 0.25)),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id()
}

pub(super) fn palette_label(commands: &mut Commands, text: &str) -> Entity {
    commands
        .spawn((
            Text::new(text),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x6c, 0x7f, 0x95)),
            Node {
                margin: UiRect::top(px(8)),
                ..default()
            },
        ))
        .id()
}

pub(super) fn palette_compact_button(
    commands: &mut Commands,
    action: PaletteAction,
    text: &str,
) -> Entity {
    commands
        .spawn((
            Button,
            PaletteButton(action),
            Node {
                flex_grow: 1.0,
                flex_basis: px(0),
                height: px(20),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id()
}

pub(super) fn palette_button(commands: &mut Commands, action: PaletteAction, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            PaletteButton(action),
            Node {
                width: percent(100),
                height: px(20),
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(px(8)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id()
}
