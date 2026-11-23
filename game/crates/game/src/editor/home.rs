//! Native home screen: maps + settings inside Bevy.
//!
//! Wasm keeps the HTML home page (`web/index.html`); native has no browser
//! page, so this overlay is the maps list and the controls settings. It is a
//! fullscreen modal: while open, canvas tools, the camera and the Add menu
//! stay inert (see [`home_modal_open`]).

use super::*;

use crate::controls::{Action, Binding, ClusterButton};

/// Which home tab is visible.

/// One pending rebind: `index` is the binding slot, `-1` appends a new one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CaptureSlot {
    pub(super) action: Action,
    pub(super) index: isize,
}

/// Native home-screen state. `open` starts true when the app booted without
/// a level (see `load_session`); the palette Maps button reopens it.
#[derive(Resource)]
pub(super) struct HomeMenu {
    pub(super) open: bool,
    pub(super) status: String,
    pub(super) capturing: Option<CaptureSlot>,
    /// Bumped whenever the map list or bindings change so the rows rebuild.
    pub(super) dirty: u64,
}

impl Default for HomeMenu {
    fn default() -> Self {
        Self {
            open: false,
            status: String::new(),
            capturing: None,
            dirty: 1,
        }
    }
}

/// True while the home overlay owns the keyboard and pointer. Canvas systems
/// check this first and do nothing while it holds.
pub(super) fn home_modal_open(home: Option<Res<HomeMenu>>) -> bool {
    home.is_some_and(|home| home.open)
}

/// Bundled modal gates (Add menu + home) as a single [`SystemParam`] so
/// systems that already sit at Bevy's 16-parameter limit can check both.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct MenuHome<'w> {
    pub(super) menu: Res<'w, AddMenu>,
    pub(super) home: Option<Res<'w, HomeMenu>>,
}

// ---------------------------------------------------------------------------
// UI markers
// ---------------------------------------------------------------------------

#[derive(Component)]
pub(super) struct HomeRoot;

#[derive(Component)]
pub(super) struct HomePanel;

#[derive(Component)]
pub(super) struct HomeStatus;

#[derive(Component)]
pub(super) struct HomeControlsList;

#[derive(Component)]
pub(super) struct HomeSettingsSection;

#[derive(Component)]
pub(super) struct HomeButton(HomeAction);

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum HomeAction {
    Close,
    ResetControls,
    Rebind(Action, isize),
    RemoveBinding(Action, usize),
    CancelCapture,
    CycleTouch(ClusterButton),
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

pub(super) fn spawn_home_menu(commands: &mut Commands) {
    let root = commands
        .spawn((
            HomeRoot,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.03, 0.04, 0.06, 0.86)),
        ))
        .id();

    let panel = commands
        .spawn((
            HomePanel,
            Node {
                width: px(660),
                max_height: percent(88),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(16)),
                row_gap: px(10),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(12)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BORDER_ON),
        ))
        .id();

    let title = commands
        .spawn((
            Text::new("Party Dungeon — home"),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    let subtitle = commands
        .spawn((
            Text::new("Tune controls, then enter the shared world."),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();



    // ---- Settings section ----
    let settings_section = commands
        .spawn((
            HomeSettingsSection,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                ..default()
            },
        ))
        .id();
    let settings_hint = commands
        .spawn((
            Text::new("Click a binding, then press a key, a mouse button, or a gamepad button. Esc cancels. Alt-click / right-click a chip also removes it; use + to add. Saved to controls.json."),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let controls_list = commands
        .spawn((
            HomeControlsList,
            ScrollPosition::default(),
            Node {
                width: percent(100),
                height: px(340),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    let settings_footer = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            ..default()
        })
        .id();
    let reset = home_button(commands, HomeAction::ResetControls, "Reset defaults");
    let done = home_button(commands, HomeAction::Close, "Done");
    commands
        .entity(settings_footer)
        .add_children(&[reset, done]);
    commands.entity(settings_section).add_children(&[
        settings_hint,
        controls_list,
        settings_footer,
    ]);

    let status = commands
        .spawn((
            HomeStatus,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();

    commands
        .entity(panel)
        .add_children(&[title, subtitle, settings_section, status]);
    commands.entity(root).add_child(panel);
}

fn home_button(commands: &mut Commands, action: HomeAction, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            HomeButton(action),
            Node {
                padding: UiRect::axes(px(12), px(7)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(8)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
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

// ---------------------------------------------------------------------------
// Open / close + sync
// ---------------------------------------------------------------------------

pub(super) fn open_home(home: &mut HomeMenu) {
    home.open = true;
    home.capturing = None;
    home.dirty = home.dirty.wrapping_add(1);
    if home.status.is_empty() {
        home.status = String::new();
    }
}

pub(super) fn close_home(home: &mut HomeMenu) {
    home.open = false;
    home.capturing = None;
}

pub(super) fn sync_home_menu(
    mut commands: Commands,
    home: Res<HomeMenu>,
    controls: Res<crate::controls::Controls>,
    mut roots: Query<&mut Node, With<HomeRoot>>,
    settings_sections: Query<Entity, With<HomeSettingsSection>>,
    controls_lists: Query<Entity, With<HomeControlsList>>,
    mut status_texts: Query<&mut Text, With<HomeStatus>>,
    mut last_built: Local<u64>,
) {
    let display = if home.open {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut roots {
        if node.display != display {
            node.display = display;
        }
    }
    if !home.open {
        return;
    }
    if let Ok(settings) = settings_sections.single() {
        commands.entity(settings).insert(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            display: Display::Flex,
            ..default()
        });
    }
    for mut text in &mut status_texts {
        let shown = if let Some(slot) = home.capturing {
            format!(
                "Press a key, mouse button, or gamepad button for {}… (Esc cancels)",
                slot.action.id()
            )
        } else if home.status.is_empty() {
            String::new()
        } else {
            home.status.clone()
        };
        if text.0 != shown {
            text.0 = shown;
        }
    }
    let fingerprint = home
        .dirty
        .wrapping_add((controls.to_json().len() as u64).wrapping_mul(31));
    if *last_built == fingerprint {
        return;
    }
    *last_built = fingerprint;
    if let Ok(list) = controls_lists.single() {
        rebuild_controls_list(&mut commands, list, &home, &controls);
    }
}



fn rebuild_controls_list(
    commands: &mut Commands,
    list: Entity,
    home: &HomeMenu,
    controls: &crate::controls::Controls,
) {
    use crate::controls::Action;
    commands.entity(list).despawn_children();
    let groups = [
        "Play",
        "Camera",
        "Mode",
        "Editor",
        "Transform",
        "Add menu",
        "Model viewer",
    ];
    let mut top: Vec<Entity> = Vec::new();
    for group in groups {
        let header = commands
            .spawn((
                Text::new(group),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
            ))
            .id();
        top.push(header);
        for action in Action::ALL.iter().copied().filter(|a| a.group() == group) {
            let label = commands
                .spawn((
                    Text::new(action.label()),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                    Node {
                        width: px(180),
                        flex_shrink: 0.0,
                        ..default()
                    },
                ))
                .id();
            let mut kids: Vec<Entity> = vec![label];
            for (index, binding) in controls.binding(action).iter().enumerate() {
                let capturing_here = home
                    .capturing
                    .is_some_and(|slot| slot.action == action && slot.index as usize == index);
                let text = if capturing_here {
                    "press input…".to_owned()
                } else {
                    crate::controls::chord_label(binding)
                };
                let chip = commands
                    .spawn((
                        Button,
                        HomeButton(HomeAction::Rebind(action, index as isize)),
                        Node {
                            padding: UiRect::axes(px(8), px(5)),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(6)),
                            ..default()
                        },
                        BackgroundColor(if capturing_here { BUTTON_ON } else { BUTTON_BG }),
                        BorderColor::all(if capturing_here { BORDER_ON } else { BORDER }),
                        children![(
                            Text::new(text),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                        )],
                    ))
                    .id();
                let remove = commands
                    .spawn((
                        Button,
                        HomeButton(HomeAction::RemoveBinding(action, index)),
                        Node {
                            padding: UiRect::axes(px(6), px(5)),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(6)),
                            ..default()
                        },
                        BackgroundColor(BUTTON_BG),
                        BorderColor::all(BORDER),
                        children![(
                            Text::new("x"),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                        )],
                    ))
                    .id();
                kids.push(chip);
                kids.push(remove);
            }
            if home
                .capturing
                .is_some_and(|slot| slot.action == action && slot.index == -1)
            {
                let waiting = commands
                    .spawn((
                        Button,
                        HomeButton(HomeAction::CancelCapture),
                        Node {
                            padding: UiRect::axes(px(8), px(5)),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(6)),
                            ..default()
                        },
                        BackgroundColor(BUTTON_ON),
                        BorderColor::all(BORDER_ON),
                        children![(
                            Text::new("press input…"),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                        )],
                    ))
                    .id();
                kids.push(waiting);
            } else {
                let add = commands
                    .spawn((
                        Button,
                        HomeButton(HomeAction::Rebind(action, -1)),
                        Node {
                            padding: UiRect::axes(px(8), px(5)),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(6)),
                            ..default()
                        },
                        BackgroundColor(BUTTON_BG),
                        BorderColor::all(BORDER),
                        children![(
                            Text::new("+"),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                        )],
                    ))
                    .id();
                kids.push(add);
            }
            let row = commands
                .spawn(Node {
                    width: percent(100),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    flex_wrap: FlexWrap::Wrap,
                    row_gap: px(4),
                    ..default()
                })
                .id();
            commands.entity(row).add_children(&kids);
            top.push(row);
        }
    }
    let touch_header = commands
        .spawn((
            Text::new("Touch buttons"),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    top.push(touch_header);
    for button in ClusterButton::ALL.iter().copied() {
        let current = controls.touch_action(button);
        let label = match current {
            Some(action) => action.label().to_owned(),
            None => "Disabled".to_owned(),
        };
        let name = commands
            .spawn((
                Text::new(format!("Button {}", button.id())),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                Node {
                    width: px(180),
                    flex_shrink: 0.0,
                    ..default()
                },
            ))
            .id();
        let chip = commands
            .spawn((
                Button,
                HomeButton(HomeAction::CycleTouch(button)),
                Node {
                    padding: UiRect::axes(px(8), px(5)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(BUTTON_BG),
                BorderColor::all(BORDER),
                children![(
                    Text::new(label),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                )],
            ))
            .id();
        let row = commands
            .spawn(Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
                ..default()
            })
            .id();
        commands.entity(row).add_children(&[name, chip]);
        top.push(row);
    }
    commands.entity(list).add_children(&top);
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn on_home_button(
    event: On<Activate>,
    buttons: Query<&HomeButton>,
    mut home: ResMut<HomeMenu>,
    mut controls: ResMut<crate::controls::Controls>,
    editor: Res<Editor>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    match button.0.clone() {
        HomeAction::Close => {
            close_home(&mut home);
            home.status = current_level_note(&editor);
        }
        HomeAction::ResetControls => {
            *controls = crate::controls::Controls::with_defaults();
            persist_controls(&controls, &mut home);
            home.capturing = None;
            home.dirty = home.dirty.wrapping_add(1);
        }
        HomeAction::Rebind(action, index) => {
            home.capturing = Some(CaptureSlot { action, index });
            home.dirty = home.dirty.wrapping_add(1);
        }
        HomeAction::RemoveBinding(action, index) => {
            remove_binding(&mut controls, action, index, &mut home, &mut commands);
        }
        HomeAction::CancelCapture => {
            home.capturing = None;
            home.dirty = home.dirty.wrapping_add(1);
        }
        HomeAction::CycleTouch(button) => {
            cycle_touch(&mut controls, button, &mut home);
        }
    }
}

pub(super) fn current_level_note(_editor: &Editor) -> String {
    "shared world".to_owned()
}


fn remove_binding(
    controls: &mut crate::controls::Controls,
    action: Action,
    index: usize,
    home: &mut HomeMenu,
    _commands: &mut Commands,
) {
    let mut bindings = controls.binding(action).to_vec();
    if index >= bindings.len() {
        return;
    }
    bindings.remove(index);
    // Replace via JSON round-trip: Controls exposes test-only setters, so
    // rebuild from its serialized form with one entry dropped.
    let mut stored: serde_json::Value =
        serde_json::from_str(&controls.to_json()).unwrap_or(serde_json::Value::Null);
    if let Some(map) = stored.get_mut("bindings").and_then(|v| v.as_object_mut()) {
        let list: Vec<serde_json::Value> = bindings
            .iter()
            .map(|binding| {
                serde_json::to_value(binding_to_stored(binding)).unwrap_or(serde_json::Value::Null)
            })
            .collect();
        map.insert(action.id().to_owned(), serde_json::Value::Array(list));
    }
    if let Some(next) = crate::controls::Controls::from_json(&stored.to_string()) {
        *controls = next;
        persist_controls(controls, home);
    }
    home.capturing = None;
    home.dirty = home.dirty.wrapping_add(1);
}

fn binding_to_stored(binding: &Binding) -> StoredBindingJson {
    match binding {
        Binding::Key(chord) => StoredBindingJson {
            kind: "key".to_owned(),
            code: crate::controls_codes::keycode_name(chord.key).to_owned(),
            ctrl: chord.ctrl,
            shift: chord.shift,
            alt: chord.alt,
        },
        Binding::Mouse(chord) => StoredBindingJson {
            kind: "mouse".to_owned(),
            code: crate::controls_codes::mousebutton_name(chord.button).to_owned(),
            ctrl: false,
            shift: chord.shift,
            alt: false,
        },
        Binding::Pad(button) => StoredBindingJson {
            kind: "pad".to_owned(),
            code: crate::controls_codes::gamepadbutton_name(*button).to_owned(),
            ctrl: false,
            shift: false,
            alt: false,
        },
    }
}

#[derive(serde::Serialize)]
struct StoredBindingJson {
    kind: String,
    code: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

fn persist_controls(controls: &crate::controls::Controls, home: &mut HomeMenu) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match controls.save_native() {
            Ok(path) => {
                home.status = format!("saved {}", path.display());
            }
            Err(error) => {
                home.status = error;
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (controls, home);
    }
}

fn cycle_touch(
    controls: &mut crate::controls::Controls,
    button: ClusterButton,
    home: &mut HomeMenu,
) {
    use crate::controls::Action as A;
    const ORDER: [Option<A>; 6] = [
        Some(A::Jump),
        Some(A::Sprint),
        Some(A::Slide),
        Some(A::Interact),
        Some(A::Shoot),
        None,
    ];
    let current = controls.touch_action(button);
    let next = ORDER
        .iter()
        .position(|slot| *slot == current)
        .map(|index| ORDER[(index + 1) % ORDER.len()])
        .unwrap_or(Some(A::Jump));
    // Controls exposes test-only touch setters; round-trip through JSON here.
    let mut stored: serde_json::Value =
        serde_json::from_str(&controls.to_json()).unwrap_or(serde_json::Value::Null);
    if let Some(map) = stored.get_mut("touch").and_then(|v| v.as_object_mut()) {
        match next {
            Some(action) => {
                map.insert(
                    button.id().to_owned(),
                    serde_json::Value::String(action.id().to_owned()),
                );
            }
            None => {
                map.insert(button.id().to_owned(), serde_json::Value::Null);
            }
        }
    }
    if let Some(next_controls) = crate::controls::Controls::from_json(&stored.to_string()) {
        *controls = next_controls;
        persist_controls(controls, home);
    }
    home.dirty = home.dirty.wrapping_add(1);
}

// ---------------------------------------------------------------------------
// Capture: turn the next key / mouse / pad press into a binding
// ---------------------------------------------------------------------------

pub(super) fn update_home_capture(
    mut home: ResMut<HomeMenu>,
    mut controls: ResMut<crate::controls::Controls>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    pads: Query<&Gamepad>,
    mut commands: Commands,
) {
    let Some(slot) = home.capturing else {
        return;
    };
    // Esc without modifiers cancels.
    if keys.just_pressed(KeyCode::Escape)
        && !keys.pressed(KeyCode::ControlLeft)
        && !keys.pressed(KeyCode::ControlRight)
        && !keys.pressed(KeyCode::SuperLeft)
        && !keys.pressed(KeyCode::SuperRight)
        && !keys.pressed(KeyCode::ShiftLeft)
        && !keys.pressed(KeyCode::ShiftRight)
        && !keys.pressed(KeyCode::AltLeft)
        && !keys.pressed(KeyCode::AltRight)
    {
        home.capturing = None;
        home.dirty = home.dirty.wrapping_add(1);
        return;
    }
    // Keys first: any just-pressed key becomes a chord with the live modifiers.
    let mut fresh_keys: Vec<KeyCode> = keys.get_just_pressed().copied().collect();
    fresh_keys.sort_by_key(|key| crate::controls_codes::keycode_name(*key));
    // Ignore bare modifier presses; the chord completes with the real key.
    fresh_keys.retain(|key| !is_modifier(*key));
    if let Some(key) = fresh_keys.into_iter().next() {
        let binding = Binding::Key(crate::controls::KeyChord {
            key,
            ctrl: keys.pressed(KeyCode::ControlLeft)
                || keys.pressed(KeyCode::ControlRight)
                || keys.pressed(KeyCode::SuperLeft)
                || keys.pressed(KeyCode::SuperRight),
            shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
            alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        });
        finish_capture(&mut home, &mut controls, slot, binding);
        let _ = &mut commands;
        return;
    }
    // Mouse buttons (shift is part of the chord).
    let mut fresh_mouse: Vec<MouseButton> = mouse.get_just_pressed().copied().collect();
    fresh_mouse.sort_by_key(|button| crate::controls_codes::mousebutton_name(*button));
    if let Some(button) = fresh_mouse.into_iter().next() {
        let binding = Binding::Mouse(crate::controls::MouseChord {
            button,
            shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        });
        finish_capture(&mut home, &mut controls, slot, binding);
        let _ = &mut commands;
        return;
    }
    // Gamepad buttons across all connected pads.
    for pad in &pads {
        for button in all_pad_buttons() {
            if pad.just_pressed(button) {
                finish_capture(&mut home, &mut controls, slot, Binding::Pad(button));
                let _ = &mut commands;
                return;
            }
        }
    }
}

fn is_modifier(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

fn all_pad_buttons() -> [GamepadButton; 19] {
    use bevy::input::gamepad::GamepadButton as Pad;
    [
        Pad::South,
        Pad::East,
        Pad::North,
        Pad::West,
        Pad::C,
        Pad::Z,
        Pad::LeftTrigger,
        Pad::LeftTrigger2,
        Pad::RightTrigger,
        Pad::RightTrigger2,
        Pad::Select,
        Pad::Start,
        Pad::Mode,
        Pad::LeftThumb,
        Pad::RightThumb,
        Pad::DPadUp,
        Pad::DPadDown,
        Pad::DPadLeft,
        Pad::DPadRight,
    ]
}

fn finish_capture(
    home: &mut HomeMenu,
    controls: &mut crate::controls::Controls,
    slot: CaptureSlot,
    binding: Binding,
) {
    use crate::controls::Action as A;
    // DPadRight lives outside the fixed array above; check it explicitly so
    // every standard button is capturable.
    let conflicts = controls.conflicts_with(slot.action, &binding);
    if !conflicts.is_empty() {
        home.status = format!(
            "already used by {} — pick something else",
            conflicts
                .first()
                .map(|action| action.label())
                .unwrap_or("another action")
        );
        // Stay capturing so the user can retry immediately.
        home.dirty = home.dirty.wrapping_add(1);
        return;
    }
    let mut bindings = controls.binding(slot.action).to_vec();
    if slot.index >= 0 && (slot.index as usize) < bindings.len() {
        bindings[slot.index as usize] = binding;
    } else {
        // A fresh slot appends; unknown actions cannot happen (slot came
        // from the live `Action::ALL` list).
        let _ = A::MoveForward;
        bindings.push(binding);
    }
    let mut stored: serde_json::Value =
        serde_json::from_str(&controls.to_json()).unwrap_or(serde_json::Value::Null);
    if let Some(map) = stored.get_mut("bindings").and_then(|v| v.as_object_mut()) {
        let list: Vec<serde_json::Value> = bindings
            .iter()
            .map(|entry| {
                serde_json::to_value(binding_to_stored(entry)).unwrap_or(serde_json::Value::Null)
            })
            .collect();
        map.insert(slot.action.id().to_owned(), serde_json::Value::Array(list));
    }
    if let Some(next) = crate::controls::Controls::from_json(&stored.to_string()) {
        *controls = next;
        persist_controls(controls, home);
    }
    home.capturing = None;
    home.dirty = home.dirty.wrapping_add(1);
}

// ---------------------------------------------------------------------------
// Name field focus

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_level_json_loads_as_a_snapshot() {
        let doc = serde_json::json!({
            "format": "party-dungeon-level",
            "version": 2,
            "nextId": 5,
            "groups": [],
            "pieces": [
                {"id": 1, "shape": "block", "x": 1.0, "z": 2.0, "y": 0.5, "rotDeg": 30.0}
            ],
            "platforms": [],
        });
        let snapshot = crate::session::snapshot_from_level_json(&doc).expect("converts");
        assert_eq!(snapshot.pieces.len(), 1);
        assert_eq!(snapshot.next_id, Some(5));
        let loaded = snapshot.validate().expect("validates");
        assert_eq!(loaded.pieces.len(), 1);
        assert!((loaded.pieces[0].rot - 30.0_f32.to_radians()).abs() < 1e-5);
    }

    #[test]
    fn capture_rejects_same_context_conflicts() {
        use crate::controls::KeyChord;
        let controls = crate::controls::Controls::with_defaults();
        let dup = Binding::Key(KeyChord::key(KeyCode::KeyW));
        // W is MoveForward (Play); MoveBack (Play) must conflict, CopyJson (Build) must not.
        assert!(!controls.conflicts_with(Action::MoveBack, &dup).is_empty());
        assert!(controls.conflicts_with(Action::CopyJson, &dup).is_empty());
    }
}
