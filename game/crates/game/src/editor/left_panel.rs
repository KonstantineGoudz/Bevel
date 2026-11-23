//! Collapsible COMPONENTS palette docked to the left edge.
//!
//! Mirrors `right_panel.rs`: the palette collapses to a slim edge tab so the
//! viewport gets the full width back, and the hint/status bars docked beside it
//! slide left with it. The palette is desktop-only (the phone layout has its own
//! toolbar/carousel), so touch builds keep their full-screen canvas and never
//! see the tab.
//!
//! The palette root itself is spawned by `setup`; this module owns its collapse
//! state, the two toggle widgets, and the hit box. The palette root carries no
//! `ModeUi`/`InputLayer` because `sync_left_panel` folds the mode, input-layer
//! and collapse conditions into one `display` write.

use super::*;

pub(super) const LEFT_PANEL_TAB_WIDTH: f32 = 28.0;
pub(super) const LEFT_PANEL_TAB_HEIGHT: f32 = 32.0;
pub(super) const LEFT_PANEL_TAB_TOP: f32 = 8.0;
/// Distance between the palette edge and the bars docked beside it.
pub(super) const LEFT_PANEL_BAR_GAP: f32 = 4.0;

/// Open/closed state of the left palette.
#[derive(Resource)]
pub(super) struct LeftPanelState {
    pub(super) open: bool,
}

impl Default for LeftPanelState {
    fn default() -> Self {
        Self { open: true }
    }
}

/// The COMPONENTS palette root (spawned by `setup`).
#[derive(Component)]
pub(super) struct LeftPanelRoot;

/// In-panel collapse chevron, in the mode-tabs row.
#[derive(Component)]
pub(super) struct LeftPanelToggleButton;

/// Slim tab on the left edge that re-opens the collapsed palette.
#[derive(Component)]
pub(super) struct LeftPanelEdgeTab;

/// A bar docked beside the palette that slides when the palette collapses.
#[derive(Component)]
pub(super) struct LeftPanelOffsetBar {
    /// `left` while the palette is open.
    pub(super) open: f32,
    /// `left` while the palette is collapsed.
    pub(super) closed: f32,
}

/// Whether a logical screen point is over the open palette or, while collapsed,
/// over its re-open tab. Touch builds have no palette at all.
pub(super) fn over_left_panel(screen: Vec2, state: &LeftPanelState, touch: &TouchState) -> bool {
    if touch.enabled {
        return false;
    }
    if state.open {
        screen.x <= PANEL_WIDTH
    } else {
        screen.x <= LEFT_PANEL_TAB_WIDTH
            && screen.y >= LEFT_PANEL_TAB_TOP
            && screen.y <= LEFT_PANEL_TAB_TOP + LEFT_PANEL_TAB_HEIGHT
    }
}

/// The collapse chevron that sits at the end of the palette's mode-tabs row.
/// The observer is attached here, so `setup` only has to parent the button.
pub(super) fn spawn_left_panel_toggle(commands: &mut Commands) -> Entity {
    let button = commands
        .spawn((
            Button,
            LeftPanelToggleButton,
            Node {
                width: px(20),
                height: px(20),
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new("<"),
                TextLayout::no_wrap(),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
            )],
        ))
        .id();
    commands.entity(button).observe(on_left_panel_toggle);
    button
}

/// The slim tab on the left edge that re-opens the collapsed palette.
pub(super) fn spawn_left_panel_edge_tab(commands: &mut Commands) -> Entity {
    let tab = commands
        .spawn((
            Button,
            LeftPanelEdgeTab,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(LEFT_PANEL_TAB_TOP),
                width: px(LEFT_PANEL_TAB_WIDTH),
                height: px(LEFT_PANEL_TAB_HEIGHT),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::new(px(0), px(1), px(1), px(1)),
                border_radius: BorderRadius::new(px(0), px(6), px(6), px(0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(BAR_BG),
            BorderColor::all(BORDER),
            children![(
                Text::new(">"),
                TextLayout::no_wrap(),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(MUTED),
            )],
        ))
        .id();
    commands.entity(tab).observe(on_left_panel_toggle);
    tab
}

fn on_left_panel_toggle(
    event: On<Activate>,
    toggles: Query<(), Or<(With<LeftPanelToggleButton>, With<LeftPanelEdgeTab>)>>,
    mut state: ResMut<LeftPanelState>,
    mut focus: ResMut<InputFocus>,
) {
    if toggles.get(event.entity).is_ok() {
        state.open = !state.open;
        // A focused property/outliner field must not keep eating keys while the
        // palette it belongs to has just folded away.
        if focus.get().is_some() {
            focus.clear();
        }
    }
}

pub(super) fn sync_left_panel(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    state: Res<LeftPanelState>,
    mut roots: Query<&mut Node, With<LeftPanelRoot>>,
    mut tabs: Query<&mut Node, (With<LeftPanelEdgeTab>, Without<LeftPanelRoot>)>,
    mut bars: Query<
        (&LeftPanelOffsetBar, &mut Node),
        (Without<LeftPanelRoot>, Without<LeftPanelEdgeTab>),
    >,
) {
    // The palette is authoring-only on the desktop layer, exactly like its old
    // `ModeUi { desktop: true, play: false }`, plus the collapse state.
    let visible = !touch.enabled && object_editing_allowed(editor.mode);

    for mut root in &mut roots {
        let display = if visible && state.open {
            Display::Flex
        } else {
            Display::None
        };
        if root.display != display {
            root.display = display;
        }
    }

    for mut tab in &mut tabs {
        let display = if visible && !state.open {
            Display::Flex
        } else {
            Display::None
        };
        if tab.display != display {
            tab.display = display;
        }
    }

    // Status and hint bars sit in the strip the palette used to occupy; slide
    // them left so a collapsed palette leaves nothing behind.
    for (bar, mut node) in &mut bars {
        let left = if state.open || touch.enabled {
            bar.open
        } else {
            bar.closed
        };
        if node.left != px(left) {
            node.left = px(left);
        }
    }
}
