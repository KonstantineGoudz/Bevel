//! The Object Graph panel: an outliner for every object in the scene.
//!
//! Rows are rebuilt from [`SceneOutline::rows`] whenever the graph changes, so the
//! panel always mirrors the data model rather than trying to keep a second
//! copy of the hierarchy in Bevy entities. Clicking a row selects its piece or
//! group; dragging a row onto a group moves it inside; dragging it onto the
//! SCENE row (or another loose object) moves it back to the root.
//!
//! The label field renames the current selection. Group / Ungroup wrap the
//! selection in a new parent group or promote a group's children, both as
//! single undo steps.

use super::*;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input_focus::FocusCause;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::TextInput;

pub(super) const OUTLINER_WIDTH: f32 = 250.0;
pub(super) const OUTLINER_MARGIN: f32 = 8.0;
pub(super) const OUTLINER_TOP: f32 = 38.0;
pub(super) const OUTLINER_HEIGHT: f32 = 350.0;
/// Height of the title bar when the graph is collapsed to just its header.
pub(super) const OUTLINER_COLLAPSED_HEIGHT: f32 = 34.0;

/// Whether a logical screen point is over the floating object graph.
pub(super) fn over_outliner_panel(screen: Vec2, _size: Vec2, collapsed: bool) -> bool {
    let height = if collapsed {
        OUTLINER_COLLAPSED_HEIGHT
    } else {
        OUTLINER_HEIGHT
    };
    let left = PANEL_WIDTH + OUTLINER_MARGIN;
    screen.x >= left
        && screen.x <= left + OUTLINER_WIDTH
        && screen.y >= OUTLINER_TOP
        && screen.y <= OUTLINER_TOP + height
}

#[derive(Component)]
pub(super) struct OutlinerPanel;

#[derive(Component)]
pub(super) struct OutlinerList;

/// The collapsible half of the panel (toolbar, label field, tree, hint).
#[derive(Component)]
pub(super) struct OutlinerBody;

#[derive(Component)]
pub(super) struct OutlinerCollapseButton;

#[derive(Component)]
pub(super) struct OutlinerCollapseLabel;

#[derive(Component)]
pub(super) struct OutlinerCount;

#[derive(Component)]
pub(super) struct OutlinerLabelRow;

#[derive(Component)]
pub(super) struct OutlinerNameInput;

#[derive(Component)]
pub(super) struct OutlinerRow(pub(super) GraphNode);

#[derive(Component)]
pub(super) struct OutlinerToggle(pub(super) u32);

/// Per-piece padlock badge. Clicking it flips viewport-selectability without
/// selecting the row.
#[derive(Component)]
pub(super) struct OutlinerLockButton(pub(super) u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum OutlinerAction {
    NewGroup,
    Group,
    Ungroup,
}

#[derive(Component)]
pub(super) struct OutlinerActionButton(pub(super) OutlinerAction);

/// UI state for the panel. The graph itself lives in `Editor::outline`, next to
/// the pieces, so undo and saving see it.
#[derive(Resource, Default)]
pub(super) struct OutlinerState {
    pub(super) selection: GraphNode,
    /// True when the panel is rolled up to its title bar.
    pub(super) collapsed: bool,
    drag: Option<GraphNode>,
    /// Pointer position at the start of a row drag. A release without enough
    /// travel is a click, never a reparent.
    drag_start: Option<Vec2>,
    last_revision: u64,
    last_piece_ids: Vec<u32>,
    last_has_character: bool,
    built: bool,
}

#[derive(Resource, Default)]
pub(super) struct OutlinerEdit {
    selected: GraphNode,
    last_focus: Option<Entity>,
    last_text: String,
    committed: bool,
}

pub(super) fn spawn_object_graph(commands: &mut Commands) {
    let panel = commands
        .spawn((
            OutlinerPanel,
            InputLayer { desktop: true },
            ModeUi {
                desktop: true,
                play: false,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(PANEL_WIDTH + OUTLINER_MARGIN),
                top: px(OUTLINER_TOP),
                width: px(OUTLINER_WIDTH),
                height: px(OUTLINER_HEIGHT),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(8)),
                row_gap: px(5),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BAR_BG),
            BorderColor::all(BORDER),
        ))
        .id();

    let header = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(6),
            ..default()
        })
        .id();
    let collapse = commands
        .spawn((
            Button,
            OutlinerCollapseButton,
            Node {
                width: px(16),
                height: px(16),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(4)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                OutlinerCollapseLabel,
                Text::new("v"),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..default()
                },
                TextColor(MUTED),
                TextLayout::no_wrap(),
            )],
        ))
        .id();
    commands.entity(collapse).observe(on_outliner_collapse);
    let title = commands
        .spawn((
            Text::new("OBJECT GRAPH"),
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let count = commands
        .spawn((
            OutlinerCount,
            Text::new("0 items"),
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x6b, 0x7d, 0x93)),
        ))
        .id();
    let header_spacer = commands
        .spawn(Node {
            flex_grow: 1.0,
            ..default()
        })
        .id();
    commands
        .entity(header)
        .add_children(&[collapse, title, header_spacer, count]);

    let toolbar = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(4),
            ..default()
        })
        .id();
    let buttons = [
        outliner_action_button(commands, OutlinerAction::NewGroup, "New Group"),
        outliner_action_button(commands, OutlinerAction::Group, "Group"),
        outliner_action_button(commands, OutlinerAction::Ungroup, "Ungroup"),
    ];
    commands.entity(toolbar).add_children(&buttons);

    // Inline label editor for the selected piece or group.
    let label_row = commands
        .spawn((
            OutlinerLabelRow,
            Node {
                width: percent(100),
                height: px(22),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(4),
                ..default()
            },
        ))
        .id();
    let label_title = commands
        .spawn((
            Text::new("LABEL"),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x6c, 0x7f, 0x95)),
            Node {
                width: px(38),
                ..default()
            },
        ))
        .id();
    let mut editable = EditableText::new("");
    editable.visible_width = Some(20.0);
    editable.allow_newlines = false;
    let input = commands
        .spawn((
            OutlinerNameInput,
            TabIndex(100),
            TextInput,
            editable,
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextCursorStyle::default(),
            Node {
                flex_grow: 1.0,
                height: px(20),
                padding: UiRect::axes(px(5), px(2)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(4)),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
        ))
        .id();
    commands
        .entity(label_row)
        .add_children(&[label_title, input]);

    let list = commands
        .spawn((
            OutlinerList,
            ScrollPosition::default(),
            Node {
                width: percent(100),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    // One observer on the list handles every row and collapse toggle through
    // pointer bubbling, so rebuilt rows need no observers of their own.
    commands
        .entity(list)
        .observe(on_outliner_press)
        .observe(on_outliner_release)
        .observe(on_outliner_cancel);

    let hint = commands
        .spawn((
            Text::new("Select - drag to group - padlock locks"),
            TextFont {
                font_size: FontSize::Px(9.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x5b, 0x6a, 0x7d)),
            Pickable::IGNORE,
            Node {
                margin: UiRect::top(px(2)),
                ..default()
            },
        ))
        .id();

    let body = commands
        .spawn((
            OutlinerBody,
            Node {
                width: percent(100),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: px(5),
                ..default()
            },
        ))
        .id();
    commands
        .entity(body)
        .add_children(&[toolbar, label_row, list, hint]);
    commands.entity(panel).add_children(&[header, body]);
}

pub(super) fn sync_outliner_panel(
    state: Res<OutlinerState>,
    mut panels: Query<&mut Node, (With<OutlinerPanel>, Without<OutlinerBody>)>,
    mut bodies: Query<&mut Node, (With<OutlinerBody>, Without<OutlinerPanel>)>,
    mut labels: Query<&mut Text, With<OutlinerCollapseLabel>>,
) {
    let height = if state.collapsed {
        OUTLINER_COLLAPSED_HEIGHT
    } else {
        OUTLINER_HEIGHT
    };
    for mut panel in &mut panels {
        if panel.height != px(height) {
            panel.height = px(height);
        }
    }
    for mut body in &mut bodies {
        let display = if state.collapsed {
            Display::None
        } else {
            Display::Flex
        };
        if body.display != display {
            body.display = display;
        }
    }
    let glyph = if state.collapsed { ">" } else { "v" };
    for mut label in &mut labels {
        if label.0.as_str() != glyph {
            label.0 = glyph.to_owned();
        }
    }
}

pub(super) fn on_outliner_collapse(
    event: On<Activate>,
    buttons: Query<&OutlinerCollapseButton>,
    mut state: ResMut<OutlinerState>,
    mut focus: ResMut<InputFocus>,
) {
    if buttons.get(event.entity).is_ok() {
        state.collapsed = !state.collapsed;
        state.drag = None;
        state.drag_start = None;
        if focus.get().is_some() {
            focus.clear();
        }
    }
}

fn outliner_action_button(commands: &mut Commands, action: OutlinerAction, text: &str) -> Entity {
    let entity = commands
        .spawn((
            Button,
            OutlinerActionButton(action),
            Node {
                flex_grow: 1.0,
                flex_basis: px(0),
                height: px(20),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(5)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();
    commands.entity(entity).observe(on_outliner_action);
    entity
}

pub(super) fn sync_outliner_selection(editor: Res<Editor>, mut state: ResMut<OutlinerState>) {
    let mut selection = state.selection;

    // The viewport owns piece selection. A group or the character can stay
    // highlighted only while no piece is selected.
    if let Some(id) = editor.selected {
        selection = GraphNode::Piece(id);
    } else if matches!(selection, GraphNode::Piece(_)) {
        selection = GraphNode::Root;
    }
    match selection {
        GraphNode::Piece(id) if editor.piece(id).is_none() => selection = GraphNode::Root,
        GraphNode::Group(id) if editor.outline.group(id).is_none() => selection = GraphNode::Root,
        GraphNode::Character if editor.character.is_none() => selection = GraphNode::Root,
        _ => {}
    }
    if state.selection != selection {
        state.selection = selection;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_outliner_rows(
    mut commands: Commands,
    mut editor: ResMut<Editor>,
    mut state: ResMut<OutlinerState>,
    list: Single<Entity, With<OutlinerList>>,
) {
    {
        let editor = &mut *editor;
        editor.outline.prune(&editor.pieces);
    }
    let ids: Vec<u32> = editor.pieces.iter().map(|piece| piece.id).collect();
    let has_character = editor.character.is_some();
    if state.built
        && state.last_revision == editor.outline.revision
        && state.last_piece_ids == ids
        && state.last_has_character == has_character
    {
        return;
    }
    state.last_revision = editor.outline.revision;
    state.last_piece_ids = ids;
    state.last_has_character = has_character;
    state.built = true;

    commands.entity(*list).despawn_children();
    let root = spawn_row(&mut commands, &editor, GraphNode::Root, 0, 0, false);
    commands.entity(*list).add_child(root);

    let rows = editor.outline.rows(&editor.pieces, has_character);
    if rows.is_empty() {
        let empty = commands
            .spawn((
                Text::new("No objects yet - add a component"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
                Pickable::IGNORE,
                Node {
                    margin: UiRect::all(px(6)),
                    ..default()
                },
            ))
            .id();
        commands.entity(*list).add_child(empty);
        return;
    }
    for row in rows {
        let entity = spawn_row(
            &mut commands,
            &editor,
            row.node,
            row.depth,
            row.child_count,
            row.collapsed,
        );
        commands.entity(*list).add_child(entity);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_row(
    commands: &mut Commands,
    editor: &Editor,
    node: GraphNode,
    depth: usize,
    child_count: usize,
    collapsed: bool,
) -> Entity {
    let row = commands
        .spawn((
            OutlinerRow(node),
            Node {
                width: percent(100),
                height: px(22),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::left(px(4.0 + depth as f32 * 12.0)),
                column_gap: px(3),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(4)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
        ))
        .id();

    if let GraphNode::Group(id) = node {
        let glyph = if collapsed { ">" } else { "v" };
        let toggle = commands
            .spawn((
                OutlinerToggle(id),
                Node {
                    width: px(14),
                    height: px(18),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                children![(
                    Text::new(glyph),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(MUTED),
                    Pickable::IGNORE,
                )],
            ))
            .id();
        commands.entity(row).add_child(toggle);
    } else {
        let spacer = commands
            .spawn((
                Pickable::IGNORE,
                Node {
                    width: px(14),
                    ..default()
                },
            ))
            .id();
        commands.entity(row).add_child(spacer);
    }

    let locked = editor.node_locked(node);
    let colour = match node {
        GraphNode::Root => MUTED,
        GraphNode::Group(_) => AMBER,
        GraphNode::Character => PILL_COLOR,
        GraphNode::Piece(_) if locked => MUTED,
        GraphNode::Piece(_) => Color::srgb_u8(0xc3, 0xd0, 0xdf),
    };
    let label = commands
        .spawn((
            Text::new(editor.node_label(node)),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(colour),
            TextLayout::no_wrap(),
            Pickable::IGNORE,
            Node {
                flex_grow: 1.0,
                overflow: Overflow::clip(),
                ..default()
            },
        ))
        .id();
    commands.entity(row).add_child(label);

    let suffix = match node {
        GraphNode::Group(_) => Some(format!("({child_count})")),
        _ => editor.node_suffix(node),
    };
    if let Some(suffix) = suffix {
        let text = commands
            .spawn((
                Text::new(suffix),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0x62, 0x74, 0x89)),
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(row).add_child(text);
    }

    if let GraphNode::Piece(id) = node {
        let locked = editor.outline.is_locked(id);
        let icon = if locked {
            AMBER
        } else {
            Color::srgb_u8(0x62, 0x74, 0x89)
        };
        let lock = commands
            .spawn((
                OutlinerLockButton(id),
                Node {
                    width: px(16),
                    height: px(18),
                    position_type: PositionType::Relative,
                    ..default()
                },
                BackgroundColor(if locked { BUTTON_ON } else { Color::NONE }),
            ))
            .id();
        // Two tiny nodes make a padlock without depending on a glyph in the
        // default font: a rounded shackle above a solid body.
        let shackle = commands
            .spawn((
                Pickable::IGNORE,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(5),
                    top: px(3),
                    width: px(6),
                    height: px(6),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(3)),
                    ..default()
                },
                BorderColor::all(icon),
            ))
            .id();
        let body = commands
            .spawn((
                Pickable::IGNORE,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(3),
                    bottom: px(2),
                    width: px(10),
                    height: px(7),
                    border_radius: BorderRadius::all(px(1)),
                    ..default()
                },
                BackgroundColor(icon),
            ))
            .id();
        commands.entity(lock).add_children(&[shackle, body]);
        commands.entity(row).add_child(lock);
    }
    row
}

pub(super) fn sync_outliner_fields(
    mut commands: Commands,
    editor: Res<Editor>,
    state: Res<OutlinerState>,
    mut edit: ResMut<OutlinerEdit>,
    mut focus: ResMut<InputFocus>,
    mut counts: Query<&mut Text, With<OutlinerCount>>,
    mut inputs: Query<
        (Entity, &mut EditableText, Has<InteractionDisabled>),
        With<OutlinerNameInput>,
    >,
    mut buttons: Query<(
        &OutlinerActionButton,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
) {
    let count = format!(
        "{} items / {} groups",
        editor.pieces.len(),
        editor.outline.groups.len()
    );
    for mut text in &mut counts {
        if text.0 != count {
            text.0 = count.clone();
        }
    }

    let renamable = editor.node_renamable(state.selection);
    let Ok((entity, mut text, disabled)) = inputs.single_mut() else {
        return;
    };
    if disabled == renamable {
        if renamable {
            commands.entity(entity).remove::<InteractionDisabled>();
        } else {
            commands.entity(entity).insert(InteractionDisabled);
        }
    }
    if !renamable {
        edit.selected = state.selection;
        edit.last_text.clear();
        edit.committed = false;
        if focus.get() == Some(entity) {
            focus.clear();
        } else if !text.value().to_string().is_empty() {
            text.editor_mut().set_text("");
        }
    } else {
        let forced = edit.selected != state.selection;
        edit.selected = state.selection;
        if forced {
            edit.committed = false;
            edit.last_text.clear();
            if focus.get() == Some(entity) {
                focus.clear();
            }
        }
        if focus.get() != Some(entity) {
            let wanted = editor.node_label(state.selection);
            if text.value() != wanted.as_str() {
                text.editor_mut().set_text(&wanted);
            }
        }
    }

    for (button, mut background, mut border) in &mut buttons {
        let enabled = match button.0 {
            OutlinerAction::NewGroup => true,
            OutlinerAction::Group => state.selection.is_object(),
            OutlinerAction::Ungroup => matches!(state.selection, GraphNode::Group(_)),
        };
        background.0 = if enabled {
            BUTTON_BG
        } else {
            Color::srgba(0.05, 0.06, 0.08, 0.35)
        };
        *border = BorderColor::all(if enabled {
            BORDER
        } else {
            Color::srgba(0.14, 0.18, 0.24, 0.5)
        });
    }
}

pub(super) fn apply_outliner_edit(
    mut editor: ResMut<Editor>,
    state: Res<OutlinerState>,
    mut edit: ResMut<OutlinerEdit>,
    focus: Res<InputFocus>,
    inputs: Query<(Entity, &EditableText), With<OutlinerNameInput>>,
) {
    let Ok((entity, text)) = inputs.single() else {
        return;
    };
    if focus.get() != Some(entity) {
        edit.last_focus = None;
        edit.committed = false;
        return;
    }
    if !editor.node_renamable(state.selection) {
        return;
    }
    let current = text.value().to_string();
    if edit.last_focus != Some(entity) {
        edit.last_focus = Some(entity);
        edit.committed = false;
        edit.last_text = current;
        return;
    }
    if current == edit.last_text {
        return;
    }
    edit.last_text = current.clone();
    let cleaned = clean_label(&current);
    let before = match state.selection {
        GraphNode::Piece(id) => editor.outline.label(id).unwrap_or_default().to_owned(),
        GraphNode::Group(id) => editor
            .outline
            .group(id)
            .map(|group| group.name.clone())
            .unwrap_or_default(),
        GraphNode::Root | GraphNode::Character => return,
    };
    if before == cleaned {
        return;
    }
    if !edit.committed {
        editor.commit();
        edit.committed = true;
    }
    editor.rename_node(state.selection, &cleaned);
}

pub(super) fn update_outliner_rows(
    state: Res<OutlinerState>,
    mut rows: Query<(&OutlinerRow, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (row, mut background, mut border) in &mut rows {
        // The scene root is the drop target and the default state; it should
        // not look like a selected object.
        let on = row.0 != GraphNode::Root && row.0 == state.selection;
        background.0 = if on { BUTTON_ON } else { Color::NONE };
        *border = BorderColor::all(if on { BORDER_ON } else { Color::NONE });
    }
}

/// What a pointer press landed on inside the graph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutlinerHit {
    Lock(u32),
    Toggle(u32),
    Row(GraphNode),
    Empty,
}

/// Resolve the row or collapse toggle from the entity Bevy actually picked.
/// UI text picking targets the `Text` node (or one of its spans), not the row
/// that contains it, so every hit is walked up the `ChildOf` chain until a
/// marker component is found. Reaching the list itself means empty space.
fn outliner_hit(
    start: Entity,
    list: Entity,
    rows: &Query<&OutlinerRow>,
    toggles: &Query<&OutlinerToggle>,
    locks: &Query<&OutlinerLockButton>,
    parents: &Query<&ChildOf>,
) -> Option<OutlinerHit> {
    let mut current = start;
    for _ in 0..64 {
        if let Ok(lock) = locks.get(current) {
            return Some(OutlinerHit::Lock(lock.0));
        }
        if let Ok(toggle) = toggles.get(current) {
            return Some(OutlinerHit::Toggle(toggle.0));
        }
        if let Ok(row) = rows.get(current) {
            return Some(OutlinerHit::Row(row.0));
        }
        if current == list {
            return Some(OutlinerHit::Empty);
        }
        let Ok(parent) = parents.get(current) else {
            return None;
        };
        current = parent.parent();
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) fn on_outliner_press(
    event: On<PointerPress>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut state: ResMut<OutlinerState>,
    mut focus: ResMut<InputFocus>,
    toggles: Query<&OutlinerToggle>,
    locks: Query<&OutlinerLockButton>,
    rows: Query<&OutlinerRow>,
    parents: Query<&ChildOf>,
    list: Single<Entity, With<OutlinerList>>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    if editor.keyboard_transform.is_some() || editor.mode != Mode::Build {
        return;
    }
    state.drag = None;
    state.drag_start = None;
    // Pointer events rewrite `event.entity` as they bubble, so by the time this
    // observer on the list runs the target is the list itself. The original
    // target is the text node / toggle / row that was actually clicked.
    let Some(hit) = outliner_hit(
        event.original_event_target(),
        *list,
        &rows,
        &toggles,
        &locks,
        &parents,
    ) else {
        return;
    };
    match hit {
        OutlinerHit::Lock(id) => {
            editor.toggle_lock(id);
            if focus.get().is_some() {
                focus.clear();
            }
        }
        OutlinerHit::Toggle(id) => {
            if let Some(group) = editor.outline.group_mut(id) {
                group.collapsed = !group.collapsed;
            }
            editor.outline.bump();
            // Collapse state is view state, not a level edit, but persisting it
            // means reopening a map keeps the same branches closed.
            editor.soft_dirty = true;
            if focus.get().is_some() {
                focus.clear();
            }
        }
        OutlinerHit::Empty => {
            state.selection = GraphNode::Root;
            editor.selected = None;
            if focus.get().is_some() {
                focus.clear();
            }
        }
        OutlinerHit::Row(node) => {
            if focus.get().is_some() {
                focus.clear();
            }
            state.selection = node;
            // Selecting in the graph is an object action: holster a shape
            // placer so the next canvas click selects or drags instead of
            // dropping a new block.
            editor.tool = None;
            editor.ghost = None;
            match node {
                GraphNode::Piece(id) => {
                    editor.selected = Some(id);
                }
                GraphNode::Group(_) => {
                    editor.selected = None;
                }
                GraphNode::Character => {
                    editor.selected = None;
                    focus_character(&mut orbit, editor.character);
                }
                GraphNode::Root => {
                    editor.selected = None;
                }
            }
            if node.is_object() {
                state.drag = Some(node);
                state.drag_start = Some(event.pointer.position);
            }
        }
    }
}

pub(super) fn on_outliner_release(
    event: On<PointerRelease>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<OutlinerState>,
    rows: Query<&OutlinerRow>,
    toggles: Query<&OutlinerToggle>,
    locks: Query<&OutlinerLockButton>,
    parents: Query<&ChildOf>,
    list: Single<Entity, With<OutlinerList>>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Some(start) = state.drag_start.take() else {
        state.drag = None;
        return;
    };
    let Some(drag) = state.drag.take() else {
        return;
    };
    // Selecting the row can change the panel layout (for example the label
    // field becomes active), which can move a different row under a still
    // pointer. Only a deliberate drag may reparent anything.
    if event.pointer.position.distance(start) < 4.0 {
        return;
    }
    let target = match outliner_hit(
        event.original_event_target(),
        *list,
        &rows,
        &toggles,
        &locks,
        &parents,
    ) {
        Some(OutlinerHit::Toggle(id)) => GraphNode::Group(id),
        Some(OutlinerHit::Row(node)) => node,
        Some(OutlinerHit::Lock(_)) | Some(OutlinerHit::Empty) | None => return,
    };
    if target == drag {
        return;
    }
    let parent = match target {
        GraphNode::Group(id) => Some(id),
        GraphNode::Piece(id) => editor.outline.piece_parent(id),
        GraphNode::Root | GraphNode::Character => None,
    };
    if editor.move_node(drag, parent) {
        state.selection = match target {
            GraphNode::Group(_) => target,
            _ => drag,
        };
        editor.selected = match state.selection {
            GraphNode::Piece(id) => Some(id),
            _ => None,
        };
    }
}

pub(super) fn on_outliner_cancel(_event: On<PointerCancel>, mut state: ResMut<OutlinerState>) {
    state.drag = None;
    state.drag_start = None;
}

pub(super) fn on_outliner_action(
    event: On<Activate>,
    buttons: Query<&OutlinerActionButton>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<OutlinerState>,
    mut focus: ResMut<InputFocus>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    state.drag = None;
    state.drag_start = None;
    if focus.get().is_some() {
        focus.clear();
    }
    match button.0 {
        OutlinerAction::NewGroup => {
            let id = editor.new_group();
            state.selection = GraphNode::Group(id);
            editor.selected = None;
        }
        OutlinerAction::Group => {
            if let Some(id) = editor.group_node(state.selection) {
                state.selection = GraphNode::Group(id);
                editor.selected = None;
            } else {
                editor.note = "select a piece or group first";
            }
        }
        OutlinerAction::Ungroup => {
            if let Some(next) = editor.ungroup_node(state.selection) {
                state.selection = next;
                editor.selected = None;
            } else {
                editor.note = "select a group to ungroup";
            }
        }
    }
}

pub(super) fn outliner_shortcuts(
    input: PlayerInput,
    menu: Res<AddMenu>,
    mut focus: ResMut<InputFocus>,
    mut editor: ResMut<Editor>,
    mut state: ResMut<OutlinerState>,
    inputs: Query<Entity, With<OutlinerNameInput>>,
) {
    if menu.open || focus.get().is_some() || editor.mode != Mode::Build {
        return;
    }
    if input.just_pressed(Action::Back) {
        state.selection = GraphNode::Root;
        state.drag = None;
        state.drag_start = None;
        return;
    }
    if input.just_pressed(Action::UngroupNodes) {
        if let Some(next) = editor.ungroup_node(state.selection) {
            state.selection = next;
            editor.selected = None;
        }
        return;
    }
    if input.just_pressed(Action::NewGroup) {
        if let Some(id) = editor.group_node(state.selection) {
            state.selection = GraphNode::Group(id);
            editor.selected = None;
        }
        return;
    }
    if input.just_pressed(Action::RenameItem)
        && !state.collapsed
        && editor.node_renamable(state.selection)
        && let Ok(input) = inputs.single()
    {
        focus.set(input, FocusCause::Navigated);
    }
}

pub(super) fn outliner_scroll(
    mut wheel: MessageReader<MouseWheel>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut list: Query<(&mut ScrollPosition, &ComputedNode), With<OutlinerList>>,
    touch: Res<TouchState>,
    editor: Res<Editor>,
    state: Res<OutlinerState>,
) {
    let raw: f32 = wheel
        .read()
        .map(|wheel| match wheel.unit {
            MouseScrollUnit::Line => wheel.y * 21.0,
            MouseScrollUnit::Pixel => wheel.y,
        })
        .sum();
    if touch.enabled || state.collapsed || editor.mode != Mode::Build || raw == 0.0 {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    if !over_outliner_panel(cursor, window.size(), state.collapsed) {
        return;
    }
    let Ok((mut position, computed)) = list.single_mut() else {
        return;
    };
    let max = (computed.content_size() - computed.size()).y * computed.inverse_scale_factor();
    position.y = (position.y - raw).clamp(0.0, max.max(0.0));
}
