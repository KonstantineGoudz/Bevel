//! Selected-piece property panel and its physics toggle.

use super::*;
use bevy::input_focus::InputFocus;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::RelativeCursorPosition;
use bevy::ui_widgets::TextInput;

pub(super) const PROPERTIES_WIDTH: f32 = 230.0;
pub(super) const PROPERTIES_MARGIN: f32 = 12.0;
pub(super) const PROPERTIES_HEIGHT: f32 = 850.0;

#[derive(Component)]
pub(super) struct PlatformSection;

#[derive(Component)]
pub(super) struct PlatformModeButton;

#[derive(Component)]
pub(super) struct PlatformModeLabel;

#[derive(Component)]
pub(super) struct PlatformPauseButton;

#[derive(Component)]
pub(super) struct PlatformPauseLabel;

#[derive(Component)]
pub(super) struct PlatformClearButton;

#[derive(Component)]
pub(super) struct PlatformSlowerButton;

#[derive(Component)]
pub(super) struct PlatformFasterButton;

#[derive(Component)]
pub(super) struct PlatformSpeedLabel;

#[derive(Component)]
pub(super) struct PlatformStaticLabel;

#[derive(Component)]
pub(super) struct PlatformEditButton;

#[derive(Component)]
pub(super) struct PlatformEditLabel;

/// Whether a logical screen point is over the floating property panel.
pub(super) fn over_properties_panel(screen: Vec2, size: Vec2, selected: bool) -> bool {
    selected
        && screen.x >= size.x - PROPERTIES_WIDTH - PROPERTIES_MARGIN
        && screen.y >= PROPERTIES_MARGIN
        && screen.y <= PROPERTIES_MARGIN + PROPERTIES_HEIGHT
}

#[derive(Component)]
pub(super) struct PropertiesPanel;

#[derive(Component)]
pub(super) struct PropertyShapeLabel;

#[derive(Component)]
pub(super) struct PhysicsToggle;

#[derive(Component)]
pub(super) struct PhysicsToggleLabel;

#[derive(Component)]
pub(super) struct TriggerShapeButton;

#[derive(Component)]
pub(super) struct TriggerShapeLabel;

#[derive(Component)]
pub(super) struct TransparencySlider;

#[derive(Component)]
pub(super) struct TransparencySliderFill;

#[derive(Component)]
pub(super) struct TransparencyValueLabel;

#[derive(Component)]
pub(super) struct TriggerColorButton(pub u32);

#[derive(Component)]
pub(super) struct TriggerVisibleButton;

#[derive(Component)]
pub(super) struct TriggerVisibleLabel;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum PropertyField {
    PosX,
    PosY,
    PosZ,
    RotX,
    RotY,
    RotZ,
    /// Per-piece collision-trigger damage. Any piece can hurt; 0 is harmless.
    /// Adding more trigger effects later means adding more variants here.
    Damage,
    TriggerWidth,
    TriggerHeight,
    TriggerLength,
    TriggerAlpha,
}

impl PropertyField {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::PosX | Self::RotX => "X",
            Self::PosY | Self::RotY => "Y",
            Self::PosZ | Self::RotZ => "Z",
            Self::Damage => "D",
            Self::TriggerWidth => "W",
            Self::TriggerHeight => "H",
            Self::TriggerLength => "L",
            Self::TriggerAlpha => "A",
        }
    }

    pub(super) fn rotation(self) -> bool {
        matches!(self, Self::RotX | Self::RotY | Self::RotZ)
    }

    pub(super) fn get(self, piece: &Piece) -> f32 {
        match self {
            Self::PosX => piece.pos.x,
            Self::PosY => piece.elevation,
            Self::PosZ => piece.pos.y,
            Self::RotX => piece.pitch.to_degrees(),
            Self::RotY => piece.rot.to_degrees(),
            Self::RotZ => piece.roll.to_degrees(),
            Self::Damage => piece.trigger.damage,
            Self::TriggerWidth => piece.trigger_size.x,
            Self::TriggerHeight => piece.trigger_size.y,
            Self::TriggerLength => piece.trigger_size.z,
            Self::TriggerAlpha => piece.trigger_alpha,
        }
    }

    pub(super) fn set(self, piece: &mut Piece, value: f32) {
        match self {
            Self::PosX => piece.pos.x = value,
            Self::PosY => piece.elevation = value,
            Self::PosZ => piece.pos.y = value,
            Self::RotX => piece.pitch = value.to_radians(),
            Self::RotY => piece.rot = value.to_radians(),
            Self::RotZ => piece.roll = value.to_radians(),
            // Damage is clamped to >= 0; non-finite input never reaches here
            // (`parse` filters it).
            Self::Damage => piece.trigger.set_damage(value),
            Self::TriggerWidth => piece.trigger_size.x = value.max(crate::model::TRIGGER_MIN_SIZE),
            Self::TriggerHeight => piece.trigger_size.y = value.max(crate::model::TRIGGER_MIN_SIZE),
            Self::TriggerLength => piece.trigger_size.z = value.max(crate::model::TRIGGER_MIN_SIZE),
            Self::TriggerAlpha => {
                piece.trigger_alpha = value.clamp(
                    crate::model::TRIGGER_MIN_ALPHA,
                    crate::model::TRIGGER_MAX_ALPHA,
                )
            }
        }
    }

    pub(super) fn format(self, piece: &Piece) -> String {
        let value = self.get(piece);
        let text = if self.rotation() {
            format!("{:.1}", value)
        } else if matches!(self, Self::Damage) {
            format!("{:.1}", value)
        } else {
            format!("{:.3}", value)
        };
        // Avoid displaying -0.0 and a full 360 after physics or Euler snapping.
        if text == "-0.000" || text == "-0.0" || text == "360.0" {
            text.trim_start_matches('-')
                .to_owned()
                .replace("360.0", "0.0")
        } else {
            text
        }
    }

    pub(super) fn parse(self, text: &str) -> Option<f32> {
        text.trim().parse::<f32>().ok().filter(|v| v.is_finite())
    }
}

#[derive(Resource, Default)]
pub(super) struct PropertyEditState {
    selected: Option<u32>,
    last_focus: Option<Entity>,
    /// Last text the user actually edited, so physics motion or an initial
    /// focus does not re-apply a stale value every frame.
    last_text: String,
    /// One undo step per focused field session.
    committed: bool,
}

pub(super) fn spawn_properties_panel(commands: &mut Commands) {
    let panel = commands
        .spawn((
            PropertiesPanel,
            Node {
                position_type: PositionType::Absolute,
                right: px(PROPERTIES_MARGIN),
                top: px(PROPERTIES_MARGIN),
                width: px(PROPERTIES_WIDTH),
                height: px(PROPERTIES_HEIGHT),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10)),
                row_gap: px(5),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                display: Display::None,
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
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let title = commands
        .spawn((
            Text::new("PROPERTIES"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    let shape = commands
        .spawn((
            PropertyShapeLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
        ))
        .id();
    commands.entity(header).add_children(&[title, shape]);
    commands.entity(panel).add_child(header);

    let position_label = section_label(commands, "POSITION");
    commands.entity(panel).add_child(position_label);
    for (index, field) in [
        PropertyField::PosX,
        PropertyField::PosY,
        PropertyField::PosZ,
    ]
    .into_iter()
    .enumerate()
    {
        let row = property_row(commands, field, index);
        commands.entity(panel).add_child(row);
    }
    let rotation_label = section_label(commands, "ROTATION (DEGREES)");
    commands.entity(panel).add_child(rotation_label);
    for (index, field) in [
        PropertyField::RotX,
        PropertyField::RotY,
        PropertyField::RotZ,
    ]
    .into_iter()
    .enumerate()
    {
        let row = property_row(commands, field, index + 3);
        commands.entity(panel).add_child(row);
    }
    // Collision trigger: per-piece damage. Any object can hurt; 0 is
    // harmless. Future trigger effects get their own rows here.
    let damage_label = section_label(commands, "TRIGGER (DAMAGE PER HIT)");
    commands.entity(panel).add_child(damage_label);
    let damage_row = property_row(commands, PropertyField::Damage, 6);
    commands.entity(panel).add_child(damage_row);

    // Parametric trigger volume (only meaningful for trigger shapes, but shown
    // for any selection to keep the panel layout stable; values are ignored
    // for non-triggers).
    let trigger_vol_label = section_label(commands, "TRIGGER VOLUME (W / H / L)");
    commands.entity(panel).add_child(trigger_vol_label);
    for (offset, field) in [
        PropertyField::TriggerWidth,
        PropertyField::TriggerHeight,
        PropertyField::TriggerLength,
    ]
    .into_iter()
    .enumerate()
    {
        let row = property_row(commands, field, 7 + offset);
        commands.entity(panel).add_child(row);
    }
    let shape_toggle = commands
        .spawn((
            Button,
            TriggerShapeButton,
            Node {
                width: percent(100),
                height: px(24),
                margin: UiRect::top(px(4)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                TriggerShapeLabel,
                Text::new("Shape: Box"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();
    commands.entity(shape_toggle).observe(on_trigger_shape);
    commands.entity(panel).add_child(shape_toggle);

    // Transparency slider for trigger zones.
    let transp_label = section_label(commands, "TRANSPARENCY");
    commands.entity(panel).add_child(transp_label);
    let alpha_row = property_row(commands, PropertyField::TriggerAlpha, 10);
    commands.entity(panel).add_child(alpha_row);
    // Visual slider: track + fill + value label.
    let slider_row = commands
        .spawn(Node {
            width: percent(100),
            height: px(22),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(6),
            ..default()
        })
        .id();
    let slider_caption = commands
        .spawn((
            Text::new("T"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                width: px(16),
                ..default()
            },
        ))
        .id();
    let slider_track = commands
        .spawn((
            TransparencySlider,
            Button,
            RelativeCursorPosition::default(),
            Node {
                width: px(120),
                height: px(12),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
        ))
        .id();
    let slider_fill = commands
        .spawn((
            TransparencySliderFill,
            Node {
                width: percent(35.0),
                height: percent(100),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb_u8(0x3f, 0xd0, 0xb0)),
        ))
        .id();
    commands.entity(slider_track).add_child(slider_fill);
    let slider_value = commands
        .spawn((
            TransparencyValueLabel,
            Text::new("35%"),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            Node {
                width: px(40),
                ..default()
            },
        ))
        .id();
    commands
        .entity(slider_row)
        .add_children(&[slider_caption, slider_track, slider_value]);
    commands.entity(panel).add_child(slider_row);

    // Trigger color: preset swatches for quick assignment.
    let color_label = section_label(commands, "TRIGGER COLOR");
    commands.entity(panel).add_child(color_label);
    let color_row = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(4),
            ..default()
        })
        .id();
    for &preset in &crate::model::TRIGGER_COLOR_PRESETS {
        let btn = commands
            .spawn((
                Button,
                TriggerColorButton(preset),
                Node {
                    width: px(22),
                    height: px(22),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(4)),
                    ..default()
                },
                BackgroundColor(crate::model::trigger_color_to_bevy(preset)),
                BorderColor::all(BORDER),
            ))
            .id();
        commands.entity(btn).observe(on_trigger_color);
        commands.entity(color_row).add_child(btn);
    }
    commands.entity(panel).add_child(color_row);

    // Visibility toggle for edit mode (always selectable via scene graph).
    let visible_toggle = commands
        .spawn((
            Button,
            TriggerVisibleButton,
            Node {
                width: percent(100),
                height: px(24),
                margin: UiRect::top(px(4)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                TriggerVisibleLabel,
                Text::new("Show in Edit: Yes"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();
    commands.entity(visible_toggle).observe(on_trigger_visible);
    commands.entity(panel).add_child(visible_toggle);

    let toggle = commands
        .spawn((
            Button,
            PhysicsToggle,
            Node {
                width: percent(100),
                height: px(24),
                margin: UiRect::top(px(4)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                PhysicsToggleLabel,
                Text::new("Physics: Static"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(Color::srgb_u8(0xc3, 0xd0, 0xdf)),
            )],
        ))
        .id();
    commands.entity(toggle).observe(on_physics_toggle);
    commands.entity(panel).add_child(toggle);

    // Motion section: mode / pause / speed / clear for the selected piece.
    let motion = commands
        .spawn((
            PlatformSection,
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(5),
                ..default()
            },
        ))
        .id();
    let motion_label = section_label(commands, "MOTION (PLATFORM PATH)");
    commands.entity(motion).add_child(motion_label);
    let edit = platform_button(
        commands,
        PlatformEditButton,
        PlatformEditLabel,
        "Edit path: off",
    );
    commands.entity(edit).observe(on_platform_edit);
    commands.entity(motion).add_child(edit);
    let mode = platform_button(
        commands,
        PlatformModeButton,
        PlatformModeLabel,
        "Mode: none",
    );
    commands.entity(mode).observe(on_platform_mode);
    commands.entity(motion).add_child(mode);
    let pause = platform_button(
        commands,
        PlatformPauseButton,
        PlatformPauseLabel,
        "Paused: no",
    );
    commands.entity(pause).observe(on_platform_pause);
    commands.entity(motion).add_child(pause);
    let speed_row = commands
        .spawn(Node {
            width: percent(100),
            flex_direction: FlexDirection::Row,
            column_gap: px(4),
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let slower = platform_button(commands, PlatformSlowerButton, PlatformStaticLabel, "-");
    commands.entity(slower).observe(on_platform_slower);
    let speed_label = commands
        .spawn((
            PlatformSpeedLabel,
            Text::new("1.5 m/s"),
            TextFont {
                font_size: FontSize::Px(12.0),
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
    let faster = platform_button(commands, PlatformFasterButton, PlatformStaticLabel, "+");
    commands.entity(faster).observe(on_platform_faster);
    commands
        .entity(speed_row)
        .add_children(&[slower, speed_label, faster]);
    commands.entity(motion).add_child(speed_row);
    let clear = platform_button(
        commands,
        PlatformClearButton,
        PlatformStaticLabel,
        "Clear path",
    );
    commands.entity(clear).observe(on_platform_clear);
    commands.entity(motion).add_child(clear);
    commands.entity(panel).add_child(motion);
}

fn platform_button<C: Component, L: Component>(
    commands: &mut Commands,
    button: C,
    label: L,
    text: &str,
) -> Entity {
    commands
        .spawn((
            Button,
            button,
            Node {
                width: percent(100),
                height: px(24),
                margin: UiRect::top(px(4)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BORDER),
            children![(
                label,
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

fn section_label(commands: &mut Commands, text: &str) -> Entity {
    commands
        .spawn((
            Text::new(text),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x6c, 0x7f, 0x95)),
            Node {
                margin: UiRect::top(px(4)),
                ..default()
            },
        ))
        .id()
}

fn property_row(commands: &mut Commands, field: PropertyField, index: usize) -> Entity {
    let row = commands
        .spawn(Node {
            width: percent(100),
            height: px(22),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(6),
            ..default()
        })
        .id();
    let label = commands
        .spawn((
            Text::new(field.label()),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                width: px(16),
                ..default()
            },
        ))
        .id();

    let mut editable = EditableText::new("");
    editable.visible_width = Some(7.0);
    editable.allow_newlines = false;
    let input = commands
        .spawn((
            field,
            TabIndex(index as i32),
            TextInput,
            editable,
            TextLayout::no_wrap(),
            TextFont {
                font_size: FontSize::Px(12.0),
                ..default()
            },
            TextCursorStyle::default(),
            Node {
                width: px(78),
                height: px(22),
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
    commands.entity(row).add_children(&[label, input]);
    row
}

pub(super) fn on_physics_toggle(
    event: On<Activate>,
    buttons: Query<&PhysicsToggle>,
    mut editor: ResMut<Editor>,
) {
    if buttons.get(event.entity).is_ok() {
        toggle_selected_physics(&mut editor);
    }
}

pub(super) fn toggle_selected_physics(editor: &mut Editor) {
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|piece| piece.id == id) else {
        return;
    };
    editor.commit();
    editor.pieces[index].dynamic = !editor.pieces[index].dynamic;
    editor.hard_dirty = true;
    editor.note = if editor.pieces[index].dynamic {
        "physics: dynamic"
    } else {
        "physics: static"
    };
}

pub(super) fn on_trigger_shape(
    event: On<Activate>,
    buttons: Query<&TriggerShapeButton>,
    mut editor: ResMut<Editor>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|p| p.id == id) else {
        return;
    };
    let shape_opt = shapes().get(editor.pieces[index].shape);
    let is_trigger = shape_opt.is_some_and(|s| s.is_trigger);
    if !is_trigger {
        editor.note = "only triggers have a shape";
        return;
    }
    editor.commit();
    let piece = &mut editor.pieces[index];
    let current = if shape_opt.unwrap().id == "trigger" {
        piece.trigger_shape_override
    } else {
        shape_opt.unwrap().trigger_shape
    };
    let next = match current {
        crate::model::TriggerShape::Box => crate::model::TriggerShape::Sphere,
        crate::model::TriggerShape::Sphere => crate::model::TriggerShape::Cylinder,
        crate::model::TriggerShape::Cylinder => crate::model::TriggerShape::Box,
    };
    // For generic trigger, store override; for specific triggers, switch the piece's shape to the matching specific shape.
    if shape_opt.unwrap().id == "trigger" {
        piece.trigger_shape_override = next;
        editor.note = "trigger shape updated";
    } else {
        let target_id = match next {
            crate::model::TriggerShape::Box => "trigger_box",
            crate::model::TriggerShape::Sphere => "trigger_sphere",
            crate::model::TriggerShape::Cylinder => "trigger_cylinder",
        };
        if let Some(new_idx) = model::shape_index(target_id) {
            piece.shape = new_idx;
            // Keep override in sync for consistency.
            piece.trigger_shape_override = next;
            editor.note = "trigger shape updated";
        }
    }
    editor.hard_dirty = true;
}

pub(super) fn on_trigger_color(
    event: On<Activate>,
    buttons: Query<&TriggerColorButton>,
    mut editor: ResMut<Editor>,
) {
    let Ok(btn) = buttons.get(event.entity) else {
        return;
    };
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|p| p.id == id) else {
        return;
    };
    if !shapes()
        .get(editor.pieces[index].shape)
        .is_some_and(|s| s.is_trigger)
    {
        editor.note = "only triggers have a color";
        return;
    }
    editor.commit();
    editor.pieces[index].trigger_color = btn.0;
    editor.hard_dirty = true;
    editor.note = "trigger color updated";
}

pub(super) fn on_trigger_visible(
    event: On<Activate>,
    buttons: Query<&TriggerVisibleButton>,
    mut editor: ResMut<Editor>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|p| p.id == id) else {
        return;
    };
    if !shapes()
        .get(editor.pieces[index].shape)
        .is_some_and(|s| s.is_trigger)
    {
        editor.note = "only triggers have visibility";
        return;
    }
    editor.commit();
    editor.pieces[index].trigger_visible = !editor.pieces[index].trigger_visible;
    editor.hard_dirty = true;
    editor.note = if editor.pieces[index].trigger_visible {
        "trigger visible in edit"
    } else {
        "trigger hidden in edit"
    };
}

fn selected_mover_id(editor: &Editor) -> Option<u32> {
    let id = editor.selected?;
    editor.pieces.iter().any(|p| p.id == id).then_some(id)
}

pub(super) fn on_platform_mode(
    event: On<Activate>,
    buttons: Query<&PlatformModeButton>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    let Some(id) = selected_mover_id(&editor) else {
        return;
    };
    let start = editor.pieces.iter().find(|p| p.id == id).map(|p| p.pos);
    let Some(start) = start else {
        return;
    };
    let mover = movers.mover_mut(id, start);
    mover.mode = match mover.mode {
        crate::platforms::PlatformMode::PingPong => crate::platforms::PlatformMode::Loop,
        crate::platforms::PlatformMode::Loop => crate::platforms::PlatformMode::OneShot,
        crate::platforms::PlatformMode::OneShot => crate::platforms::PlatformMode::PingPong,
    };
    // Loop opens/closes the circuit, so the track must be rebuilt.
    mover.rebuild();
    editor.hard_dirty = true;
    editor.note = "motion mode updated";
}

pub(super) fn on_platform_pause(
    event: On<Activate>,
    buttons: Query<&PlatformPauseButton>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    let Some(id) = selected_mover_id(&editor) else {
        return;
    };
    let start = editor.pieces.iter().find(|p| p.id == id).map(|p| p.pos);
    let Some(start) = start else {
        return;
    };
    let mover = movers.mover_mut(id, start);
    mover.paused = !mover.paused;
    editor.hard_dirty = true;
    editor.note = if mover.paused {
        "motion paused"
    } else {
        "motion resumed"
    };
}

pub(super) fn on_platform_clear(
    event: On<Activate>,
    buttons: Query<&PlatformClearButton>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    let Some(id) = selected_mover_id(&editor) else {
        return;
    };
    movers.remove_for(id);
    editor.hard_dirty = true;
    editor.note = "motion path cleared";
}

fn bump_platform_speed(editor: &mut Editor, movers: &mut Movers, delta: f32) {
    let Some(id) = selected_mover_id(editor) else {
        return;
    };
    let start = editor.pieces.iter().find(|p| p.id == id).map(|p| p.pos);
    let Some(start) = start else {
        return;
    };
    let mover = movers.mover_mut(id, start);
    mover.speed = (mover.speed + delta).clamp(0.5, 8.0);
    editor.hard_dirty = true;
}

pub(super) fn on_platform_edit(
    event: On<Activate>,
    buttons: Query<&PlatformEditButton>,
    mut editor: ResMut<Editor>,
) {
    if buttons.get(event.entity).is_err() {
        return;
    }
    if editor.selected.is_none() {
        editor.note = "select a piece first, then edit motion";
        return;
    }
    let entering = editor.actor_tool != Some(ActorTool::Platform);
    editor.actor_tool = if entering {
        Some(ActorTool::Platform)
    } else {
        None
    };
    editor.tool = None;
    editor.ghost = None;
    editor.note = if entering {
        "motion: click to add points, drag handles to reshape"
    } else {
        "motion editing off"
    };
}

pub(super) fn on_platform_slower(
    event: On<Activate>,
    buttons: Query<&PlatformSlowerButton>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
) {
    if buttons.get(event.entity).is_ok() {
        bump_platform_speed(&mut editor, &mut movers, -0.5);
    }
}

pub(super) fn on_platform_faster(
    event: On<Activate>,
    buttons: Query<&PlatformFasterButton>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
) {
    if buttons.get(event.entity).is_ok() {
        bump_platform_speed(&mut editor, &mut movers, 0.5);
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn apply_property_edits(
    mut editor: ResMut<Editor>,
    focus: Res<InputFocus>,
    mut state: ResMut<PropertyEditState>,
    fields: Query<(&PropertyField, &EditableText)>,
    visuals: Res<Visuals>,
    mut bodies: Query<
        (
            &mut Transform,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
            Has<Sleeping>,
        ),
        With<PieceVisual>,
    >,
    mut commands: Commands,
) {
    if state.selected != editor.selected {
        // Let `sync_properties_panel` update the selection, clear focus and
        // refresh all fields before accepting an edit for the new piece.
        state.committed = false;
        state.last_text.clear();
        return;
    }
    let Some(focused) = focus.get() else {
        state.last_focus = None;
        state.committed = false;
        return;
    };
    let Ok((field, text)) = fields.get(focused) else {
        return;
    };
    let current_text = text.value().to_string();
    if state.last_focus != Some(focused) {
        state.last_focus = Some(focused);
        state.committed = false;
        state.last_text = current_text;
        return;
    }
    if current_text == state.last_text {
        return;
    }
    state.last_text = current_text.clone();
    let Some(value) = field.parse(&current_text) else {
        return;
    };
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|piece| piece.id == id) else {
        return;
    };
    if (field.get(&editor.pieces[index]) - value).abs() < 1e-4 {
        return;
    }

    if !state.committed {
        editor.commit();
        state.committed = true;
    }
    field.set(&mut editor.pieces[index], value);
    editor.hard_dirty = true;

    // A dynamic body ignores the piece transform in `sync_visuals`; teleport
    // the active body so editing its numbers is not immediately overwritten by
    // the next physics step.
    let piece = editor.pieces[index];
    if !piece.dynamic {
        return;
    }
    let Some(&entity) = visuals.map.get(&piece.id) else {
        return;
    };
    let Ok((mut transform, mut position, mut rotation, mut linear, mut angular, sleeping)) =
        bodies.get_mut(entity)
    else {
        return;
    };
    let pos = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
    let rot = model::piece_quat(&piece);
    transform.translation = pos;
    transform.rotation = rot;
    position.0 = pos.real();
    rotation.0 = rot;
    *linear = LinearVelocity::default();
    *angular = AngularVelocity::default();
    if sleeping {
        commands.entity(entity).remove::<Sleeping>();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_properties_panel(
    editor: Res<Editor>,
    movers: Res<Movers>,
    touch: Res<TouchState>,
    mut focus: ResMut<InputFocus>,
    mut state: ResMut<PropertyEditState>,
    mut panel: Single<&mut Node, With<PropertiesPanel>>,
    mut fields: Query<(Entity, &PropertyField, &mut EditableText)>,
    mut shape_label: Single<
        &mut Text,
        (
            With<PropertyShapeLabel>,
            Without<PhysicsToggleLabel>,
            Without<PlatformModeLabel>,
            Without<PlatformPauseLabel>,
            Without<PlatformSpeedLabel>,
            Without<PlatformEditLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut toggle_label: Single<
        &mut Text,
        (
            With<PhysicsToggleLabel>,
            Without<PropertyShapeLabel>,
            Without<PlatformModeLabel>,
            Without<PlatformPauseLabel>,
            Without<PlatformSpeedLabel>,
            Without<PlatformEditLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut mode_label: Single<
        &mut Text,
        (
            With<PlatformModeLabel>,
            Without<PropertyShapeLabel>,
            Without<PhysicsToggleLabel>,
            Without<PlatformPauseLabel>,
            Without<PlatformSpeedLabel>,
            Without<PlatformEditLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut pause_label: Single<
        &mut Text,
        (
            With<PlatformPauseLabel>,
            Without<PropertyShapeLabel>,
            Without<PhysicsToggleLabel>,
            Without<PlatformModeLabel>,
            Without<PlatformSpeedLabel>,
            Without<PlatformEditLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut speed_label: Single<
        &mut Text,
        (
            With<PlatformSpeedLabel>,
            Without<PropertyShapeLabel>,
            Without<PhysicsToggleLabel>,
            Without<PlatformModeLabel>,
            Without<PlatformPauseLabel>,
            Without<PlatformEditLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut edit_label: Single<
        &mut Text,
        (
            With<PlatformEditLabel>,
            Without<PropertyShapeLabel>,
            Without<PhysicsToggleLabel>,
            Without<PlatformModeLabel>,
            Without<PlatformPauseLabel>,
            Without<PlatformSpeedLabel>,
            Without<TriggerShapeLabel>,
        ),
    >,
    mut trigger_shape_label: Single<&mut Text, With<TriggerShapeLabel>>,
) {
    let selected = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|piece| piece.id == id).copied());
    let visible = editor.mode == Mode::Build && !touch.enabled && selected.is_some();
    panel.display = if visible {
        Display::Flex
    } else {
        Display::None
    };

    if !visible {
        state.selected = None;
        state.committed = false;
        state.last_text.clear();
        if let Some(focused) = focus.get()
            && fields.get(focused).is_ok()
        {
            focus.clear();
        }
        return;
    }
    let Some(piece) = selected else {
        return;
    };

    let forced = state.selected != Some(piece.id);
    state.selected = Some(piece.id);
    if forced {
        state.committed = false;
        if let Some(focused) = focus.get()
            && fields.get(focused).is_ok()
        {
            focus.clear();
        }
    }

    for (entity, field, mut text) in &mut fields {
        if forced || focus.get() != Some(entity) {
            let wanted = field.format(&piece);
            if text.value().to_string() != wanted {
                text.editor_mut().set_text(&wanted);
            }
        }
    }
    shape_label.0 = match editor.outline.label(piece.id) {
        Some(label) => format!("{} ({})", label, shapes()[piece.shape].name),
        None => shapes()[piece.shape].name.to_string(),
    };
    toggle_label.0 = if piece.dynamic {
        "Physics: Dynamic".to_string()
    } else {
        "Physics: Static".to_string()
    };
    edit_label.0 = if editor.actor_tool == Some(ActorTool::Platform) {
        "Edit path: on".to_string()
    } else {
        "Edit path: off".to_string()
    };
    if let Some(mover) = editor.selected.and_then(|id| movers.map.get(&id)) {
        mode_label.0 = format!(
            "Mode: {} ({} wp)",
            mover.mode.label(),
            mover.waypoints.len()
        );
        pause_label.0 = if mover.paused {
            "Paused: yes".to_string()
        } else {
            "Paused: no".to_string()
        };
        speed_label.0 = format!("{:.1} m/s", mover.speed);
    } else {
        mode_label.0 = "Mode: none".to_string();
        pause_label.0 = "Paused: no".to_string();
        speed_label.0 = format!("{:.1} m/s", super::movers::DEFAULT_PLATFORM_SPEED);
    }
    // Trigger shape label: show current effective shape for triggers, otherwise Box.
    if let Some(shape) = shapes().get(piece.shape)
        && shape.is_trigger
    {
        let current = model::piece_trigger_shape(&piece, shape);
        trigger_shape_label.0 = format!("Shape: {}", current.label());
    } else {
        trigger_shape_label.0 = "Shape: Box".to_string();
    }
}

pub(super) fn sync_transparency_slider(
    editor: Res<Editor>,
    mut fill_query: Query<&mut Node, With<TransparencySliderFill>>,
    mut label_query: Query<&mut Text, With<TransparencyValueLabel>>,
) {
    let Some(id) = editor.selected else {
        return;
    };
    let Some(piece) = editor.pieces.iter().find(|p| p.id == id) else {
        return;
    };
    let alpha = model::sanitize_trigger_alpha(piece.trigger_alpha);
    for mut node in &mut fill_query {
        node.width = percent(alpha * 100.0);
    }
    for mut text in &mut label_query {
        text.0 = format!("{:.0}%", alpha * 100.0);
    }
}

pub(super) fn handle_transparency_slider(
    mouse: Res<ButtonInput<MouseButton>>,
    slider_query: Query<(&RelativeCursorPosition, &Interaction), With<TransparencySlider>>,
    mut editor: ResMut<Editor>,
    mut committed: Local<bool>,
) {
    // Reset commit flag when mouse is released.
    if !mouse.pressed(MouseButton::Left) {
        *committed = false;
        return;
    }
    let mut target_alpha: Option<f32> = None;
    for (pos, interaction) in &slider_query {
        if *interaction == Interaction::Pressed {
            if let Some(normalized) = pos.normalized {
                target_alpha = Some(normalized.x.clamp(0.0, 1.0));
                break;
            }
        }
    }
    let Some(new_alpha) = target_alpha else {
        *committed = false;
        return;
    };
    let Some(id) = editor.selected else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|p| p.id == id) else {
        return;
    };
    let current = model::sanitize_trigger_alpha(editor.pieces[index].trigger_alpha);
    if (current - new_alpha).abs() < 1e-3 {
        return;
    }
    if !*committed {
        editor.commit();
        *committed = true;
    }
    editor.pieces[index].trigger_alpha = new_alpha;
    editor.hard_dirty = true;
}

pub(super) fn sync_trigger_color_buttons(
    editor: Res<Editor>,
    mut buttons: Query<(&TriggerColorButton, &mut BorderColor)>,
) {
    let selected_color = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id))
        .map(|p| p.trigger_color)
        .unwrap_or(crate::model::TRIGGER_DEFAULT_COLOR);
    let is_trigger = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id))
        .and_then(|p| shapes().get(p.shape))
        .is_some_and(|s| s.is_trigger);
    for (btn, mut border) in &mut buttons {
        // Highlight only when selected piece is a trigger and color matches.
        let is_selected = is_trigger && btn.0 == selected_color;
        *border = BorderColor::all(if is_selected { BORDER_ON } else { BORDER });
    }
}

pub(super) fn sync_trigger_visible_label(
    editor: Res<Editor>,
    mut labels: Query<&mut Text, With<TriggerVisibleLabel>>,
) {
    let visible = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id))
        .map(|p| p.trigger_visible)
        .unwrap_or(true);
    for mut text in &mut labels {
        text.0 = if visible {
            "Show in Edit: Yes".to_string()
        } else {
            "Show in Edit: No".to_string()
        };
    }
}
