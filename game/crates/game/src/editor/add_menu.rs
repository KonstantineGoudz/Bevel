//! Add menu (F3): a searchable popup that arms any
//! placeable component without scrolling the palette.
//!
//! The menu is modal while open: `interact` ignores canvas clicks so choosing
//! an entry (or pressing Esc) never also places a piece. Typing filters the
//! 23 catalogue shapes plus the character; Up/Down moves the highlight, Enter
//! arms it, Esc or F3 closes.
//!
//! The menu is unavailable in first person.

use super::*;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui_widgets::TextInput;

/// How many matches stay visible. The list rebuilds as the query narrows, so
/// this only caps the empty-query height.
pub(super) const ADD_MENU_MAX_RESULTS: usize = 9;

#[derive(Resource, Default)]
pub(super) struct AddMenu {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) selected: usize,
    last_built: Option<(String, usize, usize)>,
}

/// Everything the menu can arm: a catalogue shape or the character placer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AddKind {
    Shape(usize),
    Character,
}

#[derive(Component)]
pub(super) struct AddMenuRoot;

#[derive(Component)]
pub(super) struct AddMenuSearch;

#[derive(Component)]
pub(super) struct AddMenuList;

#[derive(Component, Clone, Copy)]
pub(super) struct AddMenuItem(pub(super) AddKind);

/// All armable entries: the character placer first, then the catalogue order
/// (grouped by category in `model.rs`).
fn all_entries() -> Vec<(AddKind, &'static str, &'static str, &'static str)> {
    let mut out = Vec::with_capacity(shapes().len() + 1);
    out.push((AddKind::Character, "Character", "Actor", "character"));
    for (index, shape) in shapes().iter().enumerate() {
        out.push((AddKind::Shape(index), shape.name, shape.category, shape.id));
    }
    out
}

/// Case-insensitive multi-token filter over name, category and id. Every
/// whitespace-separated token must appear somewhere in the entry; exact name
/// prefixes sort first so "wall" beats "tall wall" style partials.
pub(super) fn filter_add_menu(query: &str) -> Vec<(AddKind, &'static str, &'static str)> {
    let all = all_entries();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all
            .into_iter()
            .map(|(kind, name, category, _)| (kind, name, category))
            .collect();
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let first = tokens.first().copied().unwrap_or("");
    let mut scored: Vec<(u8, (AddKind, &'static str, &'static str))> = Vec::new();
    for (kind, name, category, id) in all {
        let hay = format!("{name} {category} {id}").to_lowercase();
        if !tokens.iter().all(|t| hay.contains(t)) {
            continue;
        }
        let name_lower = name.to_lowercase();
        let score = if name_lower.starts_with(first) {
            0
        } else if name_lower.contains(first) {
            1
        } else {
            2
        };
        scored.push((score, (kind, name, category)));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.1.cmp(b.1.1)));
    scored.into_iter().map(|(_, entry)| entry).collect()
}

pub(super) fn activate_add_kind(editor: &mut Editor, kind: AddKind) {
    match kind {
        AddKind::Shape(index) => {
            editor.tool = Some(index);
            editor.actor_tool = None;
            editor.selected = None;
            editor.ghost = None;
            editor.ghost_rot = 0.0;
            editor.note = "click the grid to place";
        }
        AddKind::Character => {
            editor.tool = None;
            editor.actor_tool = Some(ActorTool::Place);
            editor.selected = None;
            editor.ghost = None;
            editor.ghost_rot = 0.0;
            editor.note = "click the grid to place the character";
        }
    }
}

fn open_menu(menu: &mut AddMenu, search: Entity, text: &mut EditableText, focus: &mut InputFocus) {
    menu.open = true;
    menu.query.clear();
    menu.selected = 0;
    menu.last_built = None;
    text.editor_mut().set_text("");
    focus.set(search, FocusCause::Navigated);
}

fn close_menu(menu: &mut AddMenu, text: &mut EditableText, focus: &mut InputFocus, search: Entity) {
    menu.open = false;
    menu.query.clear();
    menu.selected = 0;
    menu.last_built = None;
    text.editor_mut().set_text("");
    if focus.get() == Some(search) {
        focus.clear();
    }
}

pub(super) fn spawn_add_menu(commands: &mut Commands) {
    let wrapper = commands
        .spawn((
            AddMenuRoot,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexStart,
                padding: UiRect::top(px(120)),
                display: Display::None,
                ..default()
            },
        ))
        .id();

    let panel = commands
        .spawn((
            Node {
                width: px(380),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10)),
                row_gap: px(6),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BORDER_ON),
        ))
        .id();

    let header = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let title = commands
        .spawn((
            Text::new("ADD"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let hint = commands
        .spawn((
            Text::new("F3 / Enter add - Esc close"),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x6b, 0x7d, 0x93)),
        ))
        .id();
    commands.entity(header).add_children(&[title, hint]);

    let mut editable = EditableText::new("");
    editable.allow_newlines = false;
    let search = commands
        .spawn((
            AddMenuSearch,
            TextInput,
            editable,
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextCursorStyle::default(),
            Node {
                width: percent(100),
                height: px(26),
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

    let list = commands
        .spawn((
            AddMenuList,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                ..default()
            },
        ))
        .id();

    commands.entity(panel).add_children(&[header, search, list]);
    commands.entity(wrapper).add_child(panel);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_add_menu(
    mut commands: Commands,
    mut menu: ResMut<AddMenu>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut focus: ResMut<InputFocus>,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    gesture: Res<PathGesture>,
    mut search_query: Query<(Entity, &mut EditableText), With<AddMenuSearch>>,
    list_query: Query<Entity, With<AddMenuList>>,
    mut root_query: Query<&mut Node, With<AddMenuRoot>>,
) {
    // The compact touch layout has its own carousel; the menu is desktop-only.
    if touch.enabled {
        if menu.open {
            menu.open = false;
            menu.query.clear();
            menu.selected = 0;
            menu.last_built = None;
            focus.clear();
        }
        return;
    }
    if editor.keyboard_transform.is_some() {
        return;
    }
    // Another text field (the object-graph label editor) owns the keyboard.
    // F3 there must not open the Add popup.
    if focus.get().is_some() && !menu.open {
        return;
    }
    // Placing is Build-only; leaving Build dismisses a stale menu.
    if !object_editing_allowed(editor.mode) {
        if menu.open {
            menu.open = false;
            menu.query.clear();
            menu.selected = 0;
            menu.last_built = None;
            focus.clear();
        }
        sync_root_visibility(&mut root_query, false);
        return;
    }
    // Add menu is not available in first person. Dismiss a stale menu the same
    // way leaving Build does.
    if orbit.mode == ViewMode::First {
        if menu.open {
            menu.open = false;
            menu.query.clear();
            menu.selected = 0;
            menu.last_built = None;
            focus.clear();
        }
        sync_root_visibility(&mut root_query, false);
        return;
    }

    let Ok((search_entity, mut search_text)) = search_query.single_mut() else {
        return;
    };
    let command = command_down(&keys);

    // F3 toggles.
    if !command && keys.just_pressed(KeyCode::F3) && !gesture.0.dragging() {
        if !menu.open {
            open_menu(&mut menu, search_entity, &mut search_text, &mut focus);
        } else {
            close_menu(&mut menu, &mut search_text, &mut focus, search_entity);
            sync_root_visibility(&mut root_query, false);
            return;
        }
    }

    if !menu.open {
        sync_root_visibility(&mut root_query, false);
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        close_menu(&mut menu, &mut search_text, &mut focus, search_entity);
        sync_root_visibility(&mut root_query, false);
        return;
    }
    // Right click cancels like the modal transforms; left clicks on the panel
    // are handled by the item observers, and canvas clicks are ignored while
    // open (see `interact`).
    if mouse.just_pressed(MouseButton::Right) {
        close_menu(&mut menu, &mut search_text, &mut focus, search_entity);
        sync_root_visibility(&mut root_query, false);
        return;
    }

    let current = search_text.value().to_string();
    if current != menu.query {
        menu.query = current;
        menu.selected = 0;
    }

    let filtered = filter_add_menu(&menu.query);
    let shown: Vec<(AddKind, &'static str, &'static str)> =
        filtered.into_iter().take(ADD_MENU_MAX_RESULTS).collect();
    if shown.is_empty() {
        menu.selected = 0;
    } else {
        menu.selected = menu.selected.min(shown.len() - 1);
    }

    if keys.just_pressed(KeyCode::ArrowUp) && !shown.is_empty() {
        menu.selected = (menu.selected + shown.len() - 1) % shown.len();
    }
    if keys.just_pressed(KeyCode::ArrowDown) && !shown.is_empty() {
        menu.selected = (menu.selected + 1) % shown.len();
    }
    if keys.just_pressed(KeyCode::Enter) && !shown.is_empty() {
        let (kind, _, _) = shown[menu.selected];
        activate_add_kind(&mut editor, kind);
        close_menu(&mut menu, &mut search_text, &mut focus, search_entity);
        sync_root_visibility(&mut root_query, false);
        rebuild_list(&mut commands, &list_query, &[], 0);
        menu.last_built = Some((String::new(), 0, 0));
        return;
    }

    sync_root_visibility(&mut root_query, true);
    let fingerprint = (menu.query.clone(), menu.selected, shown.len());
    if menu.last_built.as_ref() != Some(&fingerprint) {
        rebuild_list(&mut commands, &list_query, &shown, menu.selected);
        menu.last_built = Some(fingerprint);
    }
}

fn sync_root_visibility(root_query: &mut Query<&mut Node, With<AddMenuRoot>>, open: bool) {
    let display = if open { Display::Flex } else { Display::None };
    for mut node in root_query {
        if node.display != display {
            node.display = display;
        }
    }
}

fn rebuild_list(
    commands: &mut Commands,
    list_query: &Query<Entity, With<AddMenuList>>,
    shown: &[(AddKind, &'static str, &'static str)],
    selected: usize,
) {
    let Ok(list) = list_query.single() else {
        return;
    };
    commands.entity(list).despawn_children();
    commands.entity(list).with_children(|parent| {
        if shown.is_empty() {
            parent.spawn((
                Text::new("No matches - type to filter"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(MUTED),
                Node {
                    width: percent(100),
                    height: px(24),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(px(8)),
                    ..default()
                },
            ));
            return;
        }
        for (index, (kind, name, category)) in shown.iter().enumerate() {
            let on = index == selected;
            parent.spawn((
                Button,
                AddMenuItem(*kind),
                Node {
                    width: percent(100),
                    height: px(24),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(px(8)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    ..default()
                },
                BackgroundColor(if on { BUTTON_ON } else { BUTTON_BG }),
                BorderColor::all(if on { BORDER_ON } else { BORDER }),
                children![(
                    Text::new(format!("{name}  ({category})")),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
                )],
            ));
        }
    });
}

pub(super) fn on_add_menu_item(
    event: On<Activate>,
    items: Query<&AddMenuItem>,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    mut menu: ResMut<AddMenu>,
    mut focus: ResMut<InputFocus>,
    mut search_query: Query<(Entity, &mut EditableText), With<AddMenuSearch>>,
    list_query: Query<Entity, With<AddMenuList>>,
    mut root_query: Query<&mut Node, With<AddMenuRoot>>,
    mut commands: Commands,
) {
    if !menu.open || editor.keyboard_transform.is_some() {
        return;
    }
    let Ok(item) = items.get(event.entity) else {
        return;
    };
    if !object_editing_allowed(editor.mode) || orbit.mode == ViewMode::First {
        return;
    }
    activate_add_kind(&mut editor, item.0);
    if let Ok((search_entity, mut text)) = search_query.single_mut() {
        close_menu(&mut menu, &mut text, &mut focus, search_entity);
    } else {
        menu.open = false;
        menu.query.clear();
        menu.selected = 0;
        menu.last_built = None;
        focus.clear();
    }
    sync_root_visibility(&mut root_query, false);
    rebuild_list(&mut commands, &list_query, &[], 0);
    menu.last_built = Some((String::new(), 0, 0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_lists_character_and_every_shape() {
        let all = filter_add_menu("");
        assert_eq!(all.len(), shapes().len() + 1);
        assert_eq!(all[0].0, AddKind::Character);
        assert!(all.iter().any(|(_, name, _)| *name == "Wall"));
    }

    #[test]
    fn name_search_is_case_insensitive() {
        let hits = filter_add_menu("WALL");
        assert!(hits.iter().any(|(_, name, _)| *name == "Wall"));
        let hits = filter_add_menu("wall");
        assert!(hits.iter().any(|(_, name, _)| *name == "Wall"));
    }

    #[test]
    fn category_search_finds_every_barrier() {
        let hits = filter_add_menu("barrier");
        let names: Vec<_> = hits.iter().map(|(_, name, _)| *name).collect();
        for expected in ["Wall", "Fence", "Window", "Pillar"] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }

    #[test]
    fn character_search_finds_the_actor_placer() {
        let hits = filter_add_menu("char");
        assert!(hits.iter().any(|(kind, _, _)| *kind == AddKind::Character));
    }

    #[test]
    fn multi_token_search_narrows_to_one_entry() {
        let hits = filter_add_menu("wall barrier");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "Wall");
    }

    #[test]
    fn no_match_returns_empty() {
        assert!(filter_add_menu("zzz-no-such-piece").is_empty());
    }

    #[test]
    fn arming_a_shape_holsters_the_actor_tool() {
        let mut editor = Editor::default();
        editor.actor_tool = Some(ActorTool::Place);
        editor.selected = Some(7);
        let wall = model::shape_index("wall").unwrap();
        activate_add_kind(&mut editor, AddKind::Shape(wall));
        assert_eq!(editor.tool, Some(wall));
        assert_eq!(editor.actor_tool, None);
        assert_eq!(editor.selected, None);
    }

    #[test]
    fn arming_character_holsters_the_shape_tool() {
        let mut editor = Editor::default();
        editor.tool = Some(0);
        activate_add_kind(&mut editor, AddKind::Character);
        assert_eq!(editor.actor_tool, Some(ActorTool::Place));
        assert_eq!(editor.tool, None);
    }
}
