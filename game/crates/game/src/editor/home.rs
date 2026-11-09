//! Native home screen: maps + settings inside Bevy.
//!
//! Wasm keeps the HTML home page (`web/index.html`); native has no browser
//! page, so this overlay is the maps list and the controls settings. It is a
//! fullscreen modal: while open, canvas tools, the camera and the Add menu
//! stay inert (see [`home_modal_open`]).

use super::*;
use bevy::input_focus::InputFocus;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui_widgets::TextInput;

use crate::controls::{Action, Binding, ClusterButton};

/// Which home tab is visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum HomeTab {
    #[default]
    Maps,
    Settings,
}

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
    pub(super) tab: HomeTab,
    pub(super) status: String,
    pub(super) capturing: Option<CaptureSlot>,
    /// Bumped whenever the map list or bindings change so the rows rebuild.
    pub(super) dirty: u64,
    /// Last name typed in the New/Save-as field (mirrored from the widget).
    pub(super) pending_name: String,
}

impl Default for HomeMenu {
    fn default() -> Self {
        Self {
            open: false,
            tab: HomeTab::Maps,
            status: String::new(),
            capturing: None,
            dirty: 1,
            pending_name: String::new(),
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
pub(super) struct HomeMapList;

#[derive(Component)]
pub(super) struct HomeControlsList;

#[derive(Component)]
pub(super) struct HomeNameInput;

#[derive(Component)]
pub(super) struct HomeMapsSection;

#[derive(Component)]
pub(super) struct HomeSettingsSection;

#[derive(Component)]
pub(super) struct HomeButton(HomeAction);

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum HomeAction {
    TabMaps,
    TabSettings,
    Refresh,
    Create,
    Open(String),
    Delete(String),
    Save,
    SaveAs,
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
            Text::new("Native build: pick a map from the levels dir, or tune controls. Browser builds use the maps page instead."),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();

    let tabs = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            ..default()
        })
        .id();
    let maps_tab = home_button(commands, HomeAction::TabMaps, "Maps");
    let settings_tab = home_button(commands, HomeAction::TabSettings, "Controls");
    commands.entity(tabs).add_children(&[maps_tab, settings_tab]);

    // ---- Maps section ----
    let maps_section = commands
        .spawn((
            HomeMapsSection,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                ..default()
            },
        ))
        .id();

    let name_row = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let name_label = commands
        .spawn((
            Text::new("Map name"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let mut editable = EditableText::new("");
    editable.allow_newlines = false;
    let name_input = commands
        .spawn((
            HomeNameInput,
            TextInput,
            editable,
            TextCursorStyle::default(),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            Node {
                flex_grow: 1.0,
                height: px(28),
                padding: UiRect::axes(px(8), px(4)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
        ))
        .id();
    let create = home_button(commands, HomeAction::Create, "New map");
    commands
        .entity(name_row)
        .add_children(&[name_label, name_input, create]);

    let map_list = commands
        .spawn((
            HomeMapList,
            ScrollPosition::default(),
            Node {
                width: percent(100),
                height: px(260),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();

    let maps_footer = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            ..default()
        })
        .id();
    let refresh = home_button(commands, HomeAction::Refresh, "Refresh");
    let save = home_button(commands, HomeAction::Save, "Save");
    let save_as = home_button(commands, HomeAction::SaveAs, "Save as");
    let close = home_button(commands, HomeAction::Close, "Enter editor");
    commands
        .entity(maps_footer)
        .add_children(&[refresh, save, save_as, close]);

    commands
        .entity(maps_section)
        .add_children(&[name_row, map_list, maps_footer]);

    // ---- Settings section ----
    let settings_section = commands
        .spawn((
            HomeSettingsSection,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                display: Display::None,
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

    commands.entity(panel).add_children(&[
        title,
        subtitle,
        tabs,
        maps_section,
        settings_section,
        status,
    ]);
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
    maps_sections: Query<Entity, With<HomeMapsSection>>,
    settings_sections: Query<Entity, With<HomeSettingsSection>>,
    map_lists: Query<Entity, With<HomeMapList>>,
    controls_lists: Query<Entity, With<HomeControlsList>>,
    mut status_texts: Query<&mut Text, With<HomeStatus>>,
    mut tab_buttons: Query<(&HomeButton, &mut BackgroundColor, &mut BorderColor)>,
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
    // Tab highlight.
    for (button, mut bg, mut border) in &mut tab_buttons {
        let on = match button.0 {
            HomeAction::TabMaps => home.tab == HomeTab::Maps,
            HomeAction::TabSettings => home.tab == HomeTab::Settings,
            _ => continue,
        };
        bg.0 = if on { BUTTON_ON } else { BUTTON_BG };
        *border = BorderColor::all(if on { BORDER_ON } else { BORDER });
    }
    // Section visibility.
    if let Ok(maps) = maps_sections.single() {
        commands.entity(maps).insert(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            display: if home.tab == HomeTab::Maps {
                Display::Flex
            } else {
                Display::None
            },
            ..default()
        });
    }
    if let Ok(settings) = settings_sections.single() {
        commands.entity(settings).insert(Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            display: if home.tab == HomeTab::Settings {
                Display::Flex
            } else {
                Display::None
            },
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
    if let Ok(list) = map_lists.single() {
        rebuild_map_list(&mut commands, list, &home);
    }
    if let Ok(list) = controls_lists.single() {
        rebuild_controls_list(&mut commands, list, &home, &controls);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn rebuild_map_list(commands: &mut Commands, list: Entity, _home: &HomeMenu) {
    use crate::session::list_native_levels;
    commands.entity(list).despawn_children();
    let levels = list_native_levels();
    if levels.is_empty() {
        let empty = commands
            .spawn((
                Text::new("No maps yet. Type a name above and press New map."),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(MUTED),
            ))
            .id();
        commands.entity(list).add_child(empty);
        return;
    }
    let mut rows = Vec::new();
    for level in levels {
        let label = if level.unreadable {
            format!("{} (unreadable)", level.name)
        } else {
            format!(
                "{} — {} {} {}",
                level.name,
                level.pieces,
                if level.pieces == 1 { "piece" } else { "pieces" },
                if level.has_character {
                    "+ character"
                } else {
                    ""
                }
            )
        };
        let text = commands
            .spawn((
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            ))
            .id();
        let open = commands
            .spawn((
                Button,
                HomeButton(HomeAction::Open(level.name.clone())),
                Node {
                    padding: UiRect::axes(px(10), px(6)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(BUTTON_BG),
                BorderColor::all(BORDER),
                children![(
                    Text::new("Open"),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                )],
            ))
            .id();
        let delete = commands
            .spawn((
                Button,
                HomeButton(HomeAction::Delete(level.name.clone())),
                Node {
                    padding: UiRect::axes(px(10), px(6)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(BUTTON_BG),
                BorderColor::all(BORDER),
                children![(
                    Text::new("Delete"),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                )],
            ))
            .id();
        let row = commands
            .spawn((
                Node {
                    width: percent(100),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    padding: UiRect::all(px(8)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(8)),
                    ..default()
                },
                BorderColor::all(BORDER),
            ))
            .id();
        commands.entity(row).add_children(&[text, open, delete]);
        rows.push(row);
    }
    commands.entity(list).add_children(&rows);
}

#[cfg(target_arch = "wasm32")]
fn rebuild_map_list(commands: &mut Commands, list: Entity, _home: &HomeMenu) {
    commands.entity(list).despawn_children();
    commands.entity(list).with_children(|parent| {
        parent.spawn((
            Text::new("Maps live on the browser home page on web builds."),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ));
    });
}

fn rebuild_controls_list(
    commands: &mut Commands,
    list: Entity,
    home: &HomeMenu,
    controls: &crate::controls::Controls,
) {
    use crate::controls::Action;
    commands.entity(list).despawn_children();
    let groups = ["Play", "Camera", "Mode", "Editor", "Transform", "Add menu", "Model viewer"];
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
                let capturing_here = home.capturing.is_some_and(|slot| {
                    slot.action == action && slot.index as usize == index
                });
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
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut movers: ResMut<Movers>,
    mut focus: ResMut<InputFocus>,
    name_fields: Query<(Entity, &EditableText), With<HomeNameInput>>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    match button.0.clone() {
        HomeAction::TabMaps => {
            home.tab = HomeTab::Maps;
            home.capturing = None;
            home.dirty = home.dirty.wrapping_add(1);
        }
        HomeAction::TabSettings => {
            home.tab = HomeTab::Settings;
            home.dirty = home.dirty.wrapping_add(1);
        }
        HomeAction::Refresh => {
            home.dirty = home.dirty.wrapping_add(1);
            home.status = "refreshed".to_owned();
        }
        HomeAction::Create => {
            let typed = name_fields
                .single()
                .map(|(_, text)| text.value().to_string())
                .unwrap_or_default();
            create_map(&mut editor, &mut orbit, &mut movers, typed, &mut home);
        }
        HomeAction::Open(name) => {
            open_map(&mut editor, &mut orbit, &mut movers, &name, &mut home);
        }
        HomeAction::Delete(name) => {
            delete_map(&name, &mut home);
        }
        HomeAction::Save => {
            save_current(&editor, &orbit, &movers, &mut home);
        }
        HomeAction::SaveAs => {
            let typed = name_fields
                .single()
                .map(|(_, text)| text.value().to_string())
                .unwrap_or_default();
            save_as(&mut editor, &orbit, &movers, typed, &mut home);
        }
        HomeAction::Close => {
            if let Ok((entity, _)) = name_fields.single() {
                if focus.get() == Some(entity) {
                    focus.clear();
                }
            }
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
            if let Ok((entity, _)) = name_fields.single() {
                if focus.get() == Some(entity) {
                    focus.clear();
                }
            }
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

pub(super) fn current_level_note(editor: &Editor) -> String {
    match &editor.level_name {
        Some(name) => format!("editing {name}"),
        None => "unsaved map — open Maps to save".to_owned(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn create_map(
    editor: &mut Editor,
    orbit: &mut Orbit,
    movers: &mut Movers,
    typed: String,
    home: &mut HomeMenu,
) {
    use crate::session::sanitize_level_name;
    let Some(name) = sanitize_level_name(&typed).or_else(|| {
        // Empty field creates a numbered untitled map.
        let mut counter = 1;
        loop {
            let candidate = format!("untitled-{counter}");
            let path = crate::session::level_path_for(Some(&candidate)).ok()?;
            if !path.exists() {
                return Some(candidate);
            }
            counter += 1;
        }
    }) else {
        home.status = "invalid name (use a-z, 0-9, '-', '_')".to_owned();
        return;
    };
    if crate::session::level_path_for(Some(&name))
        .is_ok_and(|path| path.exists())
    {
        home.status = format!("{name} already exists — pick Open, or another name");
        return;
    }
    clear_editor(editor, orbit, movers);
    editor.level_name = Some(name.clone());
    editor.level_path = crate::session::level_path_for(Some(&name)).ok();
    editor.session = None;
    home.pending_name = name.clone();
    home.status = format!("created {name} — Save to write it");
    home.dirty = home.dirty.wrapping_add(1);
    close_home(home);
}

#[cfg(target_arch = "wasm32")]
fn create_map(
    _editor: &mut Editor,
    _orbit: &mut Orbit,
    _movers: &mut Movers,
    _typed: String,
    home: &mut HomeMenu,
) {
    home.status = "maps live on the browser home page on web builds".to_owned();
}

#[cfg(not(target_arch = "wasm32"))]
fn open_map(
    editor: &mut Editor,
    orbit: &mut Orbit,
    movers: &mut Movers,
    name: &str,
    home: &mut HomeMenu,
) {
    match crate::session::load_native_level(name) {
        Ok(loaded) => {
            editor.apply_loaded(orbit, movers, loaded);
            editor.level_name = Some(name.to_owned());
            editor.level_path = crate::session::level_path_for(Some(name)).ok();
            editor.session = None;
            home.pending_name = name.to_owned();
            home.status = format!("opened {name}");
            home.dirty = home.dirty.wrapping_add(1);
            close_home(home);
        }
        Err(error) => {
            home.status = error;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn open_map(
    _editor: &mut Editor,
    _orbit: &mut Orbit,
    _movers: &mut Movers,
    _name: &str,
    home: &mut HomeMenu,
) {
    home.status = "maps live on the browser home page on web builds".to_owned();
}

#[cfg(not(target_arch = "wasm32"))]
fn delete_map(name: &str, home: &mut HomeMenu) {
    match crate::session::delete_native_level(name) {
        Ok(()) => {
            home.status = format!("deleted {name}");
            home.dirty = home.dirty.wrapping_add(1);
        }
        Err(error) => {
            home.status = error;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn delete_map(_name: &str, home: &mut HomeMenu) {
    home.status = "maps live on the browser home page on web builds".to_owned();
}

#[cfg(not(target_arch = "wasm32"))]
fn save_current(editor: &Editor, orbit: &Orbit, movers: &Movers, home: &mut HomeMenu) {
    let Some(name) = editor.level_name.clone() else {
        home.status = "no map open — type a name and press Save as".to_owned();
        home.tab = HomeTab::Maps;
        return;
    };
    let snapshot = editor.capture(orbit, movers);
    match crate::session::save_native_level(&name, &snapshot) {
        Ok(path) => {
            home.status = format!("saved {} ({} pieces)", path.display(), snapshot.pieces.len());
            home.dirty = home.dirty.wrapping_add(1);
        }
        Err(error) => {
            home.status = error;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn save_current(_editor: &Editor, _orbit: &Orbit, _movers: &Movers, home: &mut HomeMenu) {
    home.status = "saving happens in the browser on web builds".to_owned();
}

#[cfg(not(target_arch = "wasm32"))]
fn save_as(
    editor: &mut Editor,
    orbit: &Orbit,
    movers: &Movers,
    typed: String,
    home: &mut HomeMenu,
) {
    let Some(name) = crate::session::sanitize_level_name(&typed) else {
        home.status = "type a valid name first (use a-z, 0-9, '-', '_')".to_owned();
        return;
    };
    let snapshot = editor.capture(orbit, movers);
    match crate::session::save_native_level(&name, &snapshot) {
        Ok(path) => {
            editor.level_name = Some(name.clone());
            editor.level_path = Some(path.clone());
            home.pending_name = name.clone();
            home.status = format!("saved {} ({} pieces)", path.display(), snapshot.pieces.len());
            home.dirty = home.dirty.wrapping_add(1);
        }
        Err(error) => {
            home.status = error;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn save_as(
    _editor: &mut Editor,
    _orbit: &Orbit,
    _movers: &Movers,
    _typed: String,
    home: &mut HomeMenu,
) {
    home.status = "saving happens in the browser on web builds".to_owned();
}

fn clear_editor(editor: &mut Editor, orbit: &mut Orbit, movers: &mut Movers) {
    editor.pieces.clear();
    editor.outline = SceneOutline::default();
    editor.character = None;
    editor.undo.clear();
    editor.redo.clear();
    editor.next_id = 1;
    editor.grass.clear();
    editor.grass_slows = false;
    editor.grass_rev = editor.grass_rev.wrapping_add(1);
    editor.selection.clear();
    editor.ghost = None;
    editor.drag = None;
    editor.gizmo_drag = None;
    editor.keyboard_transform = None;
    editor.plan = None;
    editor.route = None;
    editor.walk = None;
    editor.tool = None;
    editor.actor_tool = None;
    editor.platform_request = None;
    editor.hard_dirty = false;
    editor.soft_dirty = false;
    editor.note = "new map";
    *orbit = Orbit::default();
    movers.map.clear();
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
        map.insert(
            slot.action.id().to_owned(),
            serde_json::Value::Array(list),
        );
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
// ---------------------------------------------------------------------------

pub(super) fn sync_home_name_field(
    home: Res<HomeMenu>,
    mut names: Query<&mut EditableText, With<HomeNameInput>>,
) {
    if !home.open {
        return;
    }
    // Keep the widget text as the source of truth; only mirror the level name
    // into it when the field is empty at open time. Create/Save-as read the
    // widget directly.
    for mut text in &mut names {
        let current = text.value().to_string();
        if current != home.pending_name && current.is_empty() && !home.pending_name.is_empty() {
            let pending = home.pending_name.clone();
            text.editor_mut().set_text(&pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_plain_names_and_rejects_paths() {
        assert_eq!(
            crate::session::sanitize_level_name("my-map_1"),
            Some("my-map_1".to_owned())
        );
        assert_eq!(crate::session::sanitize_level_name("  spaced  "), Some("spaced".to_owned()));
        assert_eq!(crate::session::sanitize_level_name("a/b"), None);
        assert_eq!(crate::session::sanitize_level_name(".."), None);
        assert_eq!(crate::session::sanitize_level_name(""), None);
    }

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
        // W is MoveForward (Play); MoveBack (Play) must conflict, Save (Build) must not.
        assert!(!controls.conflicts_with(Action::MoveBack, &dup).is_empty());
        assert!(controls.conflicts_with(Action::SaveLevel, &dup).is_empty());
    }
}
