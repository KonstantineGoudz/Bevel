//! Docked right-side inspector panel holding the Object Graph (outliner) and
//! Properties tool windows.
//!
//! Visible in `Mode::Build` (hidden in `Mode::Play` so the top-right play bar
//! and minimap remain unobstructed). The panel itself can be collapsed to a slim
//! edge tab on the right, resized horizontally from its left edge, and its two
//! child tool windows can each be collapsed or resized via the splitter between
//! them.

use super::*;

pub(super) const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 260.0;
pub(super) const RIGHT_PANEL_MIN_WIDTH: f32 = 200.0;
pub(super) const RIGHT_PANEL_MAX_WIDTH: f32 = 500.0;
pub(super) const RIGHT_PANEL_DEFAULT_OUTLINER_HEIGHT: f32 = 340.0;
pub(super) const RIGHT_PANEL_MIN_SECTION_HEIGHT: f32 = 110.0;
pub(super) const RIGHT_PANEL_HEADER_HEIGHT: f32 = 30.0;
pub(super) const RIGHT_PANEL_SPLITTER_HEIGHT: f32 = 6.0;
pub(super) const RIGHT_PANEL_HANDLE_WIDTH: f32 = 5.0;
pub(super) const RIGHT_PANEL_TAB_WIDTH: f32 = 28.0;
pub(super) const RIGHT_PANEL_TAB_HEIGHT: f32 = 32.0;
pub(super) const RIGHT_PANEL_TAB_TOP_DESKTOP: f32 = 8.0;
pub(super) const RIGHT_PANEL_TOUCH_TOP: f32 = 140.0;

#[derive(Resource)]
pub(super) struct RightPanelState {
    pub(super) open: bool,
    pub(super) width: f32,
    pub(super) outliner_height: f32,
    pub(super) touch_seen: bool,
    pub(super) dragging_width: bool,
    pub(super) dragging_split: bool,
}

impl Default for RightPanelState {
    fn default() -> Self {
        Self {
            open: true,
            width: RIGHT_PANEL_DEFAULT_WIDTH,
            outliner_height: RIGHT_PANEL_DEFAULT_OUTLINER_HEIGHT,
            touch_seen: false,
            dragging_width: false,
            dragging_split: false,
        }
    }
}

impl RightPanelState {
    pub(super) fn clamp_width(&self, width: f32, window_width: f32, touch: &TouchState) -> f32 {
        let s = if touch.enabled { touch.ui_scale } else { 1.0 };
        let reserved_left = if touch.enabled { 40.0 } else { PANEL_WIDTH + 120.0 };
        let max_for_window = ((window_width / s) - reserved_left).max(RIGHT_PANEL_MIN_WIDTH);
        width.clamp(
            RIGHT_PANEL_MIN_WIDTH,
            RIGHT_PANEL_MAX_WIDTH.min(max_for_window),
        )
    }

    pub(super) fn clamp_outliner_height(
        &self,
        height: f32,
        window_height: f32,
        touch: &TouchState,
    ) -> f32 {
        let s = if touch.enabled { touch.ui_scale } else { 1.0 };
        let top = if touch.enabled {
            RIGHT_PANEL_TOUCH_TOP
        } else {
            0.0
        };
        let available = (window_height / s) - top - RIGHT_PANEL_HEADER_HEIGHT - RIGHT_PANEL_SPLITTER_HEIGHT - 16.0;
        let max_h = (available - RIGHT_PANEL_MIN_SECTION_HEIGHT).max(RIGHT_PANEL_MIN_SECTION_HEIGHT);
        height.clamp(RIGHT_PANEL_MIN_SECTION_HEIGHT, max_h)
    }
}

#[derive(Component)]
pub(super) struct RightPanelRoot;

#[derive(Component)]
pub(super) struct RightPanelEdgeTab;

#[derive(Component)]
pub(super) struct RightPanelToggleButton;

#[derive(Component)]
pub(super) struct RightPanelWidthHandle;

#[derive(Component)]
pub(super) struct RightPanelSplitter;

/// Whether a logical screen point is over the right side panel (when open) or
/// its edge re-open tab (when closed).
pub(super) fn over_right_panel(
    screen: Vec2,
    size: Vec2,
    state: &RightPanelState,
    touch: &TouchState,
) -> bool {
    let s = if touch.enabled { touch.ui_scale } else { 1.0 };
    if state.open {
        let top = if touch.enabled {
            RIGHT_PANEL_TOUCH_TOP
        } else {
            0.0
        };
        screen.x >= size.x - state.width * s && screen.y >= top * s
    } else {
        let tab_top = if touch.enabled {
            RIGHT_PANEL_TOUCH_TOP
        } else {
            RIGHT_PANEL_TAB_TOP_DESKTOP
        };
        screen.x >= size.x - RIGHT_PANEL_TAB_WIDTH * s
            && screen.y >= tab_top * s
            && screen.y <= (tab_top + RIGHT_PANEL_TAB_HEIGHT) * s
    }
}

pub(super) fn spawn_right_panel(commands: &mut Commands) {
    let root = commands
        .spawn((
            RightPanelRoot,
            Node {
                position_type: PositionType::Absolute,
                right: px(0),
                top: px(0),
                bottom: px(0),
                width: px(RIGHT_PANEL_DEFAULT_WIDTH),
                flex_direction: FlexDirection::Row,
                border: UiRect::left(px(1)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BORDER),
        ))
        .id();

    // Left edge drag handle for horizontal panel resizing.
    let width_handle = commands
        .spawn((
            RightPanelWidthHandle,
            Node {
                width: px(RIGHT_PANEL_HANDLE_WIDTH),
                height: percent(100),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    commands
        .entity(width_handle)
        .observe(on_width_press)
        .observe(on_width_drag)
        .observe(on_width_release)
        .observe(on_width_cancel);

    let column = commands
        .spawn(Node {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: px(0),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            padding: UiRect::new(px(4), px(8), px(8), px(8)),
            row_gap: px(4),
            ..default()
        })
        .id();

    // Outer panel header with title and close/collapse ('>') button.
    let header = commands
        .spawn(Node {
            width: percent(100),
            height: px(RIGHT_PANEL_HEADER_HEIGHT - 8.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::bottom(px(4)),
            border: UiRect::bottom(px(1)),
            ..default()
        })
        .insert(BorderColor::all(BORDER))
        .id();
    let title = commands
        .spawn((
            Text::new("INSPECTOR"),
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let close_btn = commands
        .spawn((
            Button,
            RightPanelToggleButton,
            Node {
                width: px(20),
                height: px(20),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(4)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(">"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
                TextLayout::no_wrap(),
            )],
        ))
        .id();
    commands.entity(close_btn).observe(on_right_panel_toggle);
    commands.entity(header).add_children(&[title, close_btn]);

    let outliner = spawn_object_graph(commands);

    // Horizontal splitter between Outliner and Properties.
    let splitter = commands
        .spawn((
            RightPanelSplitter,
            Node {
                width: percent(100),
                height: px(RIGHT_PANEL_SPLITTER_HEIGHT),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
            children![(
                Pickable::IGNORE,
                Node {
                    width: px(42),
                    height: px(2),
                    border_radius: BorderRadius::all(px(1)),
                    ..default()
                },
                BackgroundColor(BORDER),
            )],
        ))
        .id();
    commands
        .entity(splitter)
        .observe(on_split_press)
        .observe(on_split_drag)
        .observe(on_split_release)
        .observe(on_split_cancel);

    let properties = spawn_properties_panel(commands);

    commands
        .entity(column)
        .add_children(&[header, outliner, splitter, properties]);
    commands.entity(root).add_children(&[width_handle, column]);

    // Slim edge tab shown on the right when the panel is closed.
    let edge_tab = commands
        .spawn((
            Button,
            RightPanelEdgeTab,
            Node {
                position_type: PositionType::Absolute,
                right: px(0),
                top: px(RIGHT_PANEL_TAB_TOP_DESKTOP),
                width: px(RIGHT_PANEL_TAB_WIDTH),
                height: px(RIGHT_PANEL_TAB_HEIGHT),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::new(px(1), px(0), px(1), px(1)),
                border_radius: BorderRadius::new(px(6), px(0), px(0), px(6)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(BAR_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new("<"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(MUTED),
                TextLayout::no_wrap(),
            )],
        ))
        .id();
    commands.entity(edge_tab).observe(on_right_panel_toggle);
}

fn on_right_panel_toggle(
    event: On<Activate>,
    toggles: Query<(), Or<(With<RightPanelToggleButton>, With<RightPanelEdgeTab>)>>,
    mut state: ResMut<RightPanelState>,
    mut focus: ResMut<InputFocus>,
) {
    if toggles.get(event.entity).is_ok() {
        state.open = !state.open;
        state.dragging_width = false;
        state.dragging_split = false;
        if focus.get().is_some() {
            focus.clear();
        }
    }
}

fn on_width_press(
    event: On<PointerPress>,
    handles: Query<(), With<RightPanelWidthHandle>>,
    mut state: ResMut<RightPanelState>,
) {
    if handles.get(event.entity).is_ok() && event.button == PointerButton::Primary {
        state.dragging_width = true;
    }
}

fn on_width_drag(
    event: On<PointerDrag>,
    window: Single<&Window, With<PrimaryWindow>>,
    touch: Res<TouchState>,
    mut state: ResMut<RightPanelState>,
) {
    if !state.dragging_width {
        return;
    }
    let s = if touch.enabled { touch.ui_scale } else { 1.0 };
    let next = state.width - event.delta.x / s;
    state.width = state.clamp_width(next, window.width(), &touch);
}

fn on_width_release(_event: On<PointerRelease>, mut state: ResMut<RightPanelState>) {
    state.dragging_width = false;
}

fn on_width_cancel(_event: On<PointerCancel>, mut state: ResMut<RightPanelState>) {
    state.dragging_width = false;
}

fn on_split_press(
    event: On<PointerPress>,
    splitters: Query<(), With<RightPanelSplitter>>,
    mut state: ResMut<RightPanelState>,
) {
    if splitters.get(event.entity).is_ok() && event.button == PointerButton::Primary {
        state.dragging_split = true;
    }
}

fn on_split_drag(
    event: On<PointerDrag>,
    window: Single<&Window, With<PrimaryWindow>>,
    touch: Res<TouchState>,
    mut state: ResMut<RightPanelState>,
) {
    if !state.dragging_split {
        return;
    }
    let s = if touch.enabled { touch.ui_scale } else { 1.0 };
    let next = state.outliner_height + event.delta.y / s;
    state.outliner_height = state.clamp_outliner_height(next, window.height(), &touch);
}

fn on_split_release(_event: On<PointerRelease>, mut state: ResMut<RightPanelState>) {
    state.dragging_split = false;
}

fn on_split_cancel(_event: On<PointerCancel>, mut state: ResMut<RightPanelState>) {
    state.dragging_split = false;
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn sync_right_panel(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    window: Single<&Window, With<PrimaryWindow>>,
    outliner_state: Res<OutlinerState>,
    props_state: Res<PropertiesPanelState>,
    mut state: ResMut<RightPanelState>,
    mut roots: Query<
        &mut Node,
        (
            With<RightPanelRoot>,
            Without<RightPanelEdgeTab>,
            Without<OutlinerPanel>,
            Without<PropertiesPanel>,
            Without<RightPanelSplitter>,
        ),
    >,
    mut tabs: Query<
        &mut Node,
        (
            With<RightPanelEdgeTab>,
            Without<RightPanelRoot>,
            Without<OutlinerPanel>,
            Without<PropertiesPanel>,
            Without<RightPanelSplitter>,
        ),
    >,
    mut outliners: Query<
        &mut Node,
        (
            With<OutlinerPanel>,
            Without<RightPanelRoot>,
            Without<RightPanelEdgeTab>,
            Without<PropertiesPanel>,
            Without<RightPanelSplitter>,
        ),
    >,
    mut splitters: Query<
        (&mut Node, &mut BackgroundColor),
        (
            With<RightPanelSplitter>,
            Without<RightPanelRoot>,
            Without<RightPanelEdgeTab>,
            Without<OutlinerPanel>,
            Without<PropertiesPanel>,
        ),
    >,
    mut properties: Query<
        &mut Node,
        (
            With<PropertiesPanel>,
            Without<RightPanelRoot>,
            Without<RightPanelEdgeTab>,
            Without<OutlinerPanel>,
            Without<RightPanelSplitter>,
        ),
    >,
    mut width_handles: Query<
        &mut BackgroundColor,
        (With<RightPanelWidthHandle>, Without<RightPanelSplitter>),
    >,
) {
    // On first touch input, close the panel so it never covers the mobile viewport.
    if touch.enabled && !state.touch_seen {
        state.touch_seen = true;
        state.open = false;
    }

    state.width = state.clamp_width(state.width, window.width(), &touch);
    state.outliner_height =
        state.clamp_outliner_height(state.outliner_height, window.height(), &touch);

    let in_build = object_editing_allowed(editor.mode);
    let root_top = if touch.enabled {
        RIGHT_PANEL_TOUCH_TOP
    } else {
        0.0
    };
    let tab_top = if touch.enabled {
        RIGHT_PANEL_TOUCH_TOP
    } else {
        RIGHT_PANEL_TAB_TOP_DESKTOP
    };

    for mut root in &mut roots {
        let display = if in_build && state.open {
            Display::Flex
        } else {
            Display::None
        };
        if root.display != display {
            root.display = display;
        }
        if root.top != px(root_top) {
            root.top = px(root_top);
        }
        if root.width != px(state.width) {
            root.width = px(state.width);
        }
    }

    for mut tab in &mut tabs {
        let display = if in_build && !state.open {
            Display::Flex
        } else {
            Display::None
        };
        if tab.display != display {
            tab.display = display;
        }
        if tab.top != px(tab_top) {
            tab.top = px(tab_top);
        }
    }

    for mut bg in &mut width_handles {
        bg.0 = if state.dragging_width {
            BORDER_ON
        } else {
            Color::NONE
        };
    }

    let out_collapsed = outliner_state.collapsed;
    let prop_collapsed = props_state.collapsed;

    for (mut splitter, mut bg) in &mut splitters {
        let show_splitter = !out_collapsed && !prop_collapsed;
        let display = if show_splitter {
            Display::Flex
        } else {
            Display::None
        };
        if splitter.display != display {
            splitter.display = display;
        }
        bg.0 = if state.dragging_split {
            BORDER_ON.with_alpha(0.35)
        } else {
            Color::NONE
        };
    }

    for mut outliner in &mut outliners {
        if out_collapsed {
            outliner.height = px(OUTLINER_COLLAPSED_HEIGHT);
            outliner.flex_grow = 0.0;
            outliner.flex_shrink = 0.0;
        } else if prop_collapsed {
            outliner.height = Val::Auto;
            outliner.flex_grow = 1.0;
            outliner.flex_shrink = 1.0;
        } else {
            outliner.height = px(state.outliner_height);
            outliner.flex_grow = 0.0;
            outliner.flex_shrink = 0.0;
        }
    }

    for mut prop in &mut properties {
        if prop_collapsed {
            prop.height = px(PROPERTIES_COLLAPSED_HEIGHT);
            prop.flex_grow = 0.0;
            prop.flex_shrink = 0.0;
        } else {
            prop.height = Val::Auto;
            prop.flex_grow = 1.0;
            prop.flex_shrink = 1.0;
        }
    }
}
