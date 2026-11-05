//! Remappable input controls: a generic action registry.
//!
//! Every keyboard / mouse / gamepad trigger in the game goes through an
//! [`Action`]. Components and systems never match on raw [`KeyCode`]s for
//! gameplay or editor shortcuts; they ask [`Controls`] whether an action is
//! `pressed` / `just_pressed` / `just_released`.
//!
//! Bindings persist in browser storage ([`CONTROLS_STORAGE_KEY`]) so the
//! Controls page on the home screen and the game stay in sync. The settings
//! UI lives entirely on the home screen (static HTML + JS); the game only
//! reads. Adding a new rebindable control is:
//!
//! 1. a variant on [`Action`] with an `id`, `label`, `group` and [`context`],
//! 2. its default bindings in [`Controls::with_defaults`],
//! 3. a row in the home-screen settings table (same `id` strings),
//! 4. call-site usage via [`Controls::pressed`] / `just_pressed`.
//!
//! Gamepad sticks keep standard fixed roles (left stick moves, right stick
//! orbits) and touch controls are fixed on-screen buttons; both are
//! documented in the settings UI rather than remapped.

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::controls_codes::{
    gamepadbutton_from_name, gamepadbutton_name, keycode_from_name, keycode_name,
    mousebutton_from_name, mousebutton_name,
};

/// Browser-storage key shared with the home-screen settings page.
pub const CONTROLS_STORAGE_KEY: &str = "party-dungeon.controls.v1";
/// Bumped when the stored schema changes; older payloads fall back to defaults.
pub const CONTROLS_VERSION: u32 = 1;
/// Gamepad stick deflection below this is ignored.
pub const STICK_DEADZONE: f32 = 0.2;

/// When an action may fire. Bindings only conflict within one context; the
/// modal transform and the open Add menu own the keyboard while active, so a
/// shared chord across contexts (e.g. `R` to rotate vs. transform-rotate) is
/// resolved by that existing precedence instead of the conflict check.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ActionContext {
    Play,
    Build,
    Modal,
    Menu,
    Viewer,
    Global,
}

/// On-screen touch buttons (the A/B/C/X/Y/Z cluster). What each button does
/// is itself remappable (see [`Controls::touch_action`]); only Jump, Sprint,
/// Slide ("run"/"crouch" on touch) and Interact are valid targets.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClusterButton {
    A,
    B,
    C,
    X,
    Y,
    Z,
}

impl ClusterButton {
    pub const ALL: &'static [ClusterButton] = &[
        ClusterButton::A,
        ClusterButton::B,
        ClusterButton::C,
        ClusterButton::X,
        ClusterButton::Y,
        ClusterButton::Z,
    ];

    /// Stable id shared with the home-screen settings page.
    pub fn id(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            "X" => Some(Self::X),
            "Y" => Some(Self::Y),
            "Z" => Some(Self::Z),
            _ => None,
        }
    }

    /// Factory default action. A jumps, B slides (the future crouch), C
    /// interacts, X fires a cone, Y runs (sprints) and Z starts disabled so
    /// there is a spare remappable button. Interact also keeps its
    /// Play-toolbar "Use" button, so remapping C costs no functionality.
    pub fn default_action(self) -> Option<Action> {
        match self {
            Self::A => Some(Action::Jump),
            Self::B => Some(Action::Slide),
            Self::C => Some(Action::Interact),
            Self::X => Some(Action::Shoot),
            Self::Y => Some(Action::Sprint),
            Self::Z => None,
        }
    }
}

/// Touch-button targets: the actions a cluster button may trigger. `None`
/// disables the button.
pub fn parse_touch_target(id: &str) -> Option<Action> {
    match Action::parse(id) {
        Some(
            action @ (Action::Jump
            | Action::Sprint
            | Action::Slide
            | Action::Interact
            | Action::Shoot),
        ) => Some(action),
        _ => None,
    }
}

/// Every rebindable control in the game. Serialized by [`Action::id`], never
/// by variant order, so reordering this enum never breaks saved bindings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    // -- Play: character ----------------------------------------------------
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    Jump,
    Sprint,
    Slide,
    Interact,
    /// Fire a cone along the actor's facing.
    Shoot,
    // -- Camera (both modes) -------------------------------------------------
    OrbitDrag,
    PanDrag,
    CycleView,
    FocusCamera,
    CaptureToggle,
    // -- Mode / shared -------------------------------------------------------
    TogglePlay,
    Back,
    WalkPlan,
    MenuToggle,
    /// Primary mouse click: select, place, confirm, drag. The touch tap is
    /// a separate fixed input ORed in by callers.
    PrimaryClick,
    // -- Build: editor -------------------------------------------------------
    PlaceCharacter,
    CopySel,
    Paste,
    RotatePiece,
    DeletePiece,
    Undo,
    Redo,
    SaveLevel,
    CopyJson,
    NewGroup,
    UngroupNodes,
    RenameItem,
    // -- Modal transform -----------------------------------------------------
    TransMove,
    TransRotate,
    AxisX,
    AxisY,
    AxisZ,
    ConfirmModal,
    // -- Add menu (open) -----------------------------------------------------
    MenuUp,
    MenuDown,
    MenuConfirm,
    // -- glTF viewer ---------------------------------------------------------
    ViewerReset,
}

impl Action {
    /// All actions, for the settings UI and conflict checks.
    pub const ALL: &'static [Action] = &[
        Action::MoveForward,
        Action::MoveBack,
        Action::MoveLeft,
        Action::MoveRight,
        Action::Jump,
        Action::Sprint,
        Action::Slide,
        Action::Interact,
        Action::Shoot,
        Action::OrbitDrag,
        Action::PanDrag,
        Action::CycleView,
        Action::FocusCamera,
        Action::CaptureToggle,
        Action::TogglePlay,
        Action::Back,
        Action::WalkPlan,
        Action::MenuToggle,
        Action::PrimaryClick,
        Action::PlaceCharacter,
        Action::CopySel,
        Action::Paste,
        Action::RotatePiece,
        Action::DeletePiece,
        Action::Undo,
        Action::Redo,
        Action::SaveLevel,
        Action::CopyJson,
        Action::NewGroup,
        Action::UngroupNodes,
        Action::RenameItem,
        Action::TransMove,
        Action::TransRotate,
        Action::AxisX,
        Action::AxisY,
        Action::AxisZ,
        Action::ConfirmModal,
        Action::MenuUp,
        Action::MenuDown,
        Action::MenuConfirm,
        Action::ViewerReset,
    ];

    /// Stable id used in save files and shared with the settings page JS.
    pub fn id(self) -> &'static str {
        match self {
            Self::MoveForward => "moveForward",
            Self::MoveBack => "moveBack",
            Self::MoveLeft => "moveLeft",
            Self::MoveRight => "moveRight",
            Self::Jump => "jump",
            Self::Sprint => "sprint",
            Self::Slide => "slide",
            Self::Interact => "interact",
            Self::Shoot => "shoot",
            Self::OrbitDrag => "orbitDrag",
            Self::PanDrag => "panDrag",
            Self::CycleView => "cycleView",
            Self::FocusCamera => "focusCamera",
            Self::CaptureToggle => "captureToggle",
            Self::TogglePlay => "togglePlay",
            Self::Back => "back",
            Self::WalkPlan => "walkPlan",
            Self::MenuToggle => "menuToggle",
            Self::PrimaryClick => "primaryClick",
            Self::PlaceCharacter => "placeCharacter",
            Self::CopySel => "copySel",
            Self::Paste => "paste",
            Self::RotatePiece => "rotatePiece",
            Self::DeletePiece => "deletePiece",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::SaveLevel => "saveLevel",
            Self::CopyJson => "copyJson",
            Self::NewGroup => "newGroup",
            Self::UngroupNodes => "ungroupNodes",
            Self::RenameItem => "renameItem",
            Self::TransMove => "transMove",
            Self::TransRotate => "transRotate",
            Self::AxisX => "axisX",
            Self::AxisY => "axisY",
            Self::AxisZ => "axisZ",
            Self::ConfirmModal => "confirmModal",
            Self::MenuUp => "menuUp",
            Self::MenuDown => "menuDown",
            Self::MenuConfirm => "menuConfirm",
            Self::ViewerReset => "viewerReset",
        }
    }

    /// Inverse of [`Action::id`]; unknown ids return `None` so newer saves
    /// keep loading (the binding is ignored).
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.iter().find(|action| action.id() == id).copied()
    }

    /// Short human name for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::MoveForward => "Move forward",
            Self::MoveBack => "Move back",
            Self::MoveLeft => "Move left",
            Self::MoveRight => "Move right",
            Self::Jump => "Jump",
            Self::Sprint => "Sprint (hold)",
            Self::Slide => "Slide (hold)",
            Self::Interact => "Interact",
            Self::Shoot => "Shoot cone",
            Self::OrbitDrag => "Orbit camera (drag)",
            Self::PanDrag => "Pan camera (drag)",
            Self::CycleView => "Cycle camera view",
            Self::FocusCamera => "Focus selection",
            Self::CaptureToggle => "Pointer capture on/off",
            Self::TogglePlay => "Play / build mode",
            Self::Back => "Back / cancel",
            Self::WalkPlan => "Walk route",
            Self::MenuToggle => "Add menu",
            Self::PrimaryClick => "Select / place (click)",
            Self::PlaceCharacter => "Place character",
            Self::CopySel => "Copy",
            Self::Paste => "Paste",
            Self::RotatePiece => "Rotate piece",
            Self::DeletePiece => "Delete",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::SaveLevel => "Save level",
            Self::CopyJson => "Copy level JSON",
            Self::NewGroup => "New group",
            Self::UngroupNodes => "Ungroup nodes",
            Self::RenameItem => "Rename",
            Self::TransMove => "Transform: move",
            Self::TransRotate => "Transform: rotate",
            Self::AxisX => "Transform axis: X",
            Self::AxisY => "Transform axis: Y",
            Self::AxisZ => "Transform axis: Z",
            Self::ConfirmModal => "Confirm",
            Self::MenuUp => "Menu: previous",
            Self::MenuDown => "Menu: next",
            Self::MenuConfirm => "Menu: confirm",
            Self::ViewerReset => "Viewer: reset view",
        }
    }

    /// Settings-page group heading.
    pub fn group(self) -> &'static str {
        match self {
            Self::MoveForward
            | Self::MoveBack
            | Self::MoveLeft
            | Self::MoveRight
            | Self::Jump
            | Self::Sprint
            | Self::Slide
            | Self::Interact
            | Self::Shoot
            | Self::WalkPlan => "Play",
            Self::OrbitDrag
            | Self::PanDrag
            | Self::CycleView
            | Self::FocusCamera
            | Self::CaptureToggle => "Camera",
            Self::TogglePlay | Self::Back | Self::MenuToggle | Self::PrimaryClick => "Mode",
            Self::PlaceCharacter
            | Self::CopySel
            | Self::Paste
            | Self::RotatePiece
            | Self::DeletePiece
            | Self::Undo
            | Self::Redo
            | Self::SaveLevel
            | Self::CopyJson
            | Self::NewGroup
            | Self::UngroupNodes
            | Self::RenameItem => "Editor",
            Self::TransMove
            | Self::TransRotate
            | Self::AxisX
            | Self::AxisY
            | Self::AxisZ
            | Self::ConfirmModal => "Transform",
            Self::MenuUp | Self::MenuDown | Self::MenuConfirm => "Add menu",
            Self::ViewerReset => "Model viewer",
        }
    }

    pub fn context(self) -> ActionContext {
        match self {
            Self::MoveForward
            | Self::MoveBack
            | Self::MoveLeft
            | Self::MoveRight
            | Self::Jump
            | Self::Sprint
            | Self::Slide
            | Self::Interact
            | Self::Shoot => ActionContext::Play,
            Self::OrbitDrag
            | Self::PanDrag
            | Self::CycleView
            | Self::FocusCamera
            | Self::CaptureToggle
            | Self::TogglePlay
            | Self::Back
            | Self::WalkPlan
            | Self::MenuToggle
            | Self::PrimaryClick => ActionContext::Global,
            Self::PlaceCharacter
            | Self::CopySel
            | Self::Paste
            | Self::RotatePiece
            | Self::DeletePiece
            | Self::Undo
            | Self::Redo
            | Self::SaveLevel
            | Self::CopyJson
            | Self::NewGroup
            | Self::UngroupNodes
            | Self::RenameItem => ActionContext::Build,
            Self::TransMove
            | Self::TransRotate
            | Self::AxisX
            | Self::AxisY
            | Self::AxisZ
            | Self::ConfirmModal => ActionContext::Modal,
            Self::MenuUp | Self::MenuDown | Self::MenuConfirm => ActionContext::Menu,
            Self::ViewerReset => ActionContext::Viewer,
        }
    }

    /// True when incidental extra modifiers must not block the action (plain
    /// locomotion keeps working while sprint/slide modifiers are held).
    pub fn mod_insensitive(self) -> bool {
        matches!(
            self,
            Self::MoveForward
                | Self::MoveBack
                | Self::MoveLeft
                | Self::MoveRight
                | Self::Jump
                | Self::Interact
        )
    }
}

/// One keyboard chord: a key plus the modifiers it requires. `Super` (macOS
/// Command) counts as `ctrl`, matching the editor's `command_down` helper.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord {
    pub key: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyChord {
    pub fn key(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    pub fn ctrl(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    pub fn ctrl_shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            shift: true,
            alt: false,
        }
    }

    pub fn shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: true,
            alt: false,
        }
    }
}

/// One mouse trigger: a button, optionally with Shift held (pan vs. orbit).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MouseChord {
    pub button: MouseButton,
    pub shift: bool,
}

impl MouseChord {
    pub fn button(button: MouseButton) -> Self {
        Self {
            button,
            shift: false,
        }
    }
}

/// One rebindable trigger: a key chord, a mouse chord, or a gamepad button.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Binding {
    Key(KeyChord),
    Mouse(MouseChord),
    Pad(GamepadButton),
}

/// The live binding table. Stateless at runtime: queries read the current
/// [`ButtonInput`]s (plus connected [`Gamepad`]s) through these helpers.
#[derive(Resource, Clone, Debug)]
pub struct Controls {
    bindings: HashMap<Action, Vec<Binding>>,
    /// Remappable touch-cluster mapping: `None` disables the button.
    touch: HashMap<ClusterButton, Option<Action>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredControls {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    bindings: HashMap<String, Vec<StoredBinding>>,
    /// Touch-cluster mapping: button id -> action id (or null = disabled).
    #[serde(default)]
    touch: HashMap<String, Option<String>>,
}

/// Wire format for one binding, shared with the home-screen settings page:
/// `kind` is `key` | `mouse` | `pad`, `code` is the canonical variant name
/// (`KeyboardEvent.code` for keys, with `Meta*` normalized to `Super*`).
#[derive(Serialize, Deserialize, Clone)]
struct StoredBinding {
    kind: String,
    code: String,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    alt: bool,
}

impl StoredBinding {
    fn from_binding(binding: &Binding) -> Self {
        match binding {
            Binding::Key(chord) => Self {
                kind: "key".to_string(),
                code: keycode_name(chord.key).to_string(),
                ctrl: chord.ctrl,
                shift: chord.shift,
                alt: chord.alt,
            },
            Binding::Mouse(chord) => Self {
                kind: "mouse".to_string(),
                code: mousebutton_name(chord.button).to_string(),
                ctrl: false,
                shift: chord.shift,
                alt: false,
            },
            Binding::Pad(button) => Self {
                kind: "pad".to_string(),
                code: gamepadbutton_name(*button).to_string(),
                ctrl: false,
                shift: false,
                alt: false,
            },
        }
    }

    fn to_binding(&self) -> Option<Binding> {
        match self.kind.as_str() {
            "key" => keycode_from_name(&self.code).map(|key| {
                Binding::Key(KeyChord {
                    key,
                    ctrl: self.ctrl,
                    shift: self.shift,
                    alt: self.alt,
                })
            }),
            "mouse" => mousebutton_from_name(&self.code).map(|button| {
                Binding::Mouse(MouseChord {
                    button,
                    shift: self.shift,
                })
            }),
            "pad" => gamepadbutton_from_name(&self.code).map(Binding::Pad),
            _ => None,
        }
    }
}

impl Default for Controls {
    fn default() -> Self {
        Self::with_defaults()
    }
}

fn key(key: KeyCode) -> Binding {
    Binding::Key(KeyChord::key(key))
}

fn mouse(button: MouseButton) -> Binding {
    Binding::Mouse(MouseChord::button(button))
}

fn pad(button: GamepadButton) -> Binding {
    Binding::Pad(button)
}

impl Controls {
    /// Factory defaults: exactly the historical controls.
    pub fn with_defaults() -> Self {
        use Action as A;
        let raw: &[(A, &[Binding])] = &[
            (
                A::MoveForward,
                &[
                    key(KeyCode::KeyW),
                    key(KeyCode::ArrowUp),
                    pad(GamepadButton::DPadUp),
                ],
            ),
            (
                A::MoveBack,
                &[
                    key(KeyCode::KeyS),
                    key(KeyCode::ArrowDown),
                    pad(GamepadButton::DPadDown),
                ],
            ),
            (
                A::MoveLeft,
                &[
                    key(KeyCode::KeyA),
                    key(KeyCode::ArrowLeft),
                    pad(GamepadButton::DPadLeft),
                ],
            ),
            (
                A::MoveRight,
                &[
                    key(KeyCode::KeyD),
                    key(KeyCode::ArrowRight),
                    pad(GamepadButton::DPadRight),
                ],
            ),
            (A::Jump, &[key(KeyCode::Space), pad(GamepadButton::South)]),
            (
                A::Sprint,
                &[
                    Binding::Key(KeyChord {
                        key: KeyCode::ShiftLeft,
                        ctrl: false,
                        shift: true,
                        alt: false,
                    }),
                    Binding::Key(KeyChord {
                        key: KeyCode::ShiftRight,
                        ctrl: false,
                        shift: true,
                        alt: false,
                    }),
                ],
            ),
            (
                A::Slide,
                &[
                    Binding::Key(KeyChord {
                        key: KeyCode::ControlLeft,
                        ctrl: true,
                        shift: false,
                        alt: false,
                    }),
                    Binding::Key(KeyChord {
                        key: KeyCode::ControlRight,
                        ctrl: true,
                        shift: false,
                        alt: false,
                    }),
                ],
            ),
            (A::Interact, &[key(KeyCode::KeyE), pad(GamepadButton::East)]),
            (
                A::Shoot,
                &[key(KeyCode::KeyQ), pad(GamepadButton::RightTrigger)],
            ),
            (A::OrbitDrag, &[mouse(MouseButton::Right)]),
            (
                A::PanDrag,
                &[
                    mouse(MouseButton::Middle),
                    Binding::Mouse(MouseChord {
                        button: MouseButton::Right,
                        shift: true,
                    }),
                ],
            ),
            (
                A::CycleView,
                &[key(KeyCode::KeyV), pad(GamepadButton::West)],
            ),
            (
                A::FocusCamera,
                &[key(KeyCode::KeyF), pad(GamepadButton::North)],
            ),
            (A::CaptureToggle, &[key(KeyCode::Tab)]),
            (
                A::TogglePlay,
                &[key(KeyCode::KeyP), pad(GamepadButton::Start)],
            ),
            (A::Back, &[key(KeyCode::Escape), pad(GamepadButton::Select)]),
            (A::WalkPlan, &[key(KeyCode::Enter)]),
            (A::MenuToggle, &[key(KeyCode::F3)]),
            (A::PrimaryClick, &[mouse(MouseButton::Left)]),
            (A::PlaceCharacter, &[key(KeyCode::KeyC)]),
            (A::CopySel, &[Binding::Key(KeyChord::ctrl(KeyCode::KeyC))]),
            (A::Paste, &[Binding::Key(KeyChord::ctrl(KeyCode::KeyV))]),
            (A::RotatePiece, &[key(KeyCode::KeyR)]),
            (
                A::DeletePiece,
                &[key(KeyCode::Delete), key(KeyCode::Backspace)],
            ),
            (A::Undo, &[Binding::Key(KeyChord::ctrl(KeyCode::KeyZ))]),
            (
                A::Redo,
                &[
                    Binding::Key(KeyChord::ctrl_shift(KeyCode::KeyZ)),
                    Binding::Key(KeyChord::ctrl(KeyCode::KeyY)),
                ],
            ),
            (A::SaveLevel, &[key(KeyCode::KeyS)]),
            (A::CopyJson, &[Binding::Key(KeyChord::shift(KeyCode::KeyS))]),
            (A::NewGroup, &[Binding::Key(KeyChord::ctrl(KeyCode::KeyG))]),
            (
                A::UngroupNodes,
                &[Binding::Key(KeyChord::ctrl_shift(KeyCode::KeyG))],
            ),
            (A::RenameItem, &[key(KeyCode::F2)]),
            (A::TransMove, &[key(KeyCode::KeyG)]),
            (A::TransRotate, &[key(KeyCode::KeyR)]),
            (A::AxisX, &[key(KeyCode::KeyX)]),
            (A::AxisY, &[key(KeyCode::KeyY)]),
            (A::AxisZ, &[key(KeyCode::KeyZ)]),
            (
                A::ConfirmModal,
                &[key(KeyCode::Enter), pad(GamepadButton::South)],
            ),
            (A::MenuUp, &[key(KeyCode::ArrowUp)]),
            (A::MenuDown, &[key(KeyCode::ArrowDown)]),
            (
                A::MenuConfirm,
                &[key(KeyCode::Enter), pad(GamepadButton::South)],
            ),
            (A::ViewerReset, &[key(KeyCode::KeyR)]),
        ];
        Self {
            bindings: raw
                .iter()
                .map(|(action, bindings)| (*action, bindings.to_vec()))
                .collect(),
            touch: ClusterButton::ALL
                .iter()
                .map(|button| (*button, button.default_action()))
                .collect(),
        }
    }

    /// What a touch-cluster button triggers (`None` = disabled). Missing
    /// entries fall back to the factory default.
    pub fn touch_action(&self, button: ClusterButton) -> Option<Action> {
        self.touch
            .get(&button)
            .copied()
            .unwrap_or_else(|| button.default_action())
    }

    #[cfg(test)]
    pub fn set_test_touch(&mut self, button: ClusterButton, action: Option<Action>) {
        self.touch.insert(button, action);
    }

    /// Bindings for one action (empty means unbound).
    pub fn binding(&self, action: Action) -> &[Binding] {
        self.bindings.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replace one action's bindings (used by tests; the settings page edits
    /// storage directly and the game reloads it at boot).
    #[cfg(test)]
    pub fn set_test_bindings(&mut self, action: Action, bindings: Vec<Binding>) {
        self.bindings.insert(action, bindings);
    }

    fn chord_down(chord: &KeyChord, keys: &ButtonInput<KeyCode>, insensitive: bool) -> bool {
        if !keys.pressed(chord.key) {
            return false;
        }
        let ctrl = keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::ControlRight)
            || keys.pressed(KeyCode::SuperLeft)
            || keys.pressed(KeyCode::SuperRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        if insensitive {
            (!chord.ctrl || ctrl) && (!chord.shift || shift) && (!chord.alt || alt)
        } else {
            chord.ctrl == ctrl && chord.shift == shift && chord.alt == alt
        }
    }

    fn chord_just_pressed(
        chord: &KeyChord,
        keys: &ButtonInput<KeyCode>,
        insensitive: bool,
    ) -> bool {
        if !keys.just_pressed(chord.key) {
            return false;
        }
        let ctrl = keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::ControlRight)
            || keys.pressed(KeyCode::SuperLeft)
            || keys.pressed(KeyCode::SuperRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        if insensitive {
            (!chord.ctrl || ctrl) && (!chord.shift || shift) && (!chord.alt || alt)
        } else {
            chord.ctrl == ctrl && chord.shift == shift && chord.alt == alt
        }
    }

    fn chord_just_released(chord: &KeyChord, keys: &ButtonInput<KeyCode>) -> bool {
        keys.just_released(chord.key)
    }

    fn binding_down(
        binding: &Binding,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
        insensitive: bool,
    ) -> bool {
        match binding {
            Binding::Key(chord) => Self::chord_down(chord, keys, insensitive),
            Binding::Mouse(chord) => {
                mouse.pressed(chord.button)
                    && (!chord.shift
                        || keys.pressed(KeyCode::ShiftLeft)
                        || keys.pressed(KeyCode::ShiftRight))
                    && (chord.shift
                        || (!keys.pressed(KeyCode::ShiftLeft)
                            && !keys.pressed(KeyCode::ShiftRight)))
            }
            Binding::Pad(button) => pads.iter().any(|pad| pad.pressed(*button)),
        }
    }

    fn binding_just_pressed(
        binding: &Binding,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
        insensitive: bool,
    ) -> bool {
        match binding {
            Binding::Key(chord) => Self::chord_just_pressed(chord, keys, insensitive),
            Binding::Mouse(chord) => {
                mouse.just_pressed(chord.button)
                    && (!chord.shift
                        || keys.pressed(KeyCode::ShiftLeft)
                        || keys.pressed(KeyCode::ShiftRight))
                    && (chord.shift
                        || (!keys.pressed(KeyCode::ShiftLeft)
                            && !keys.pressed(KeyCode::ShiftRight)))
            }
            Binding::Pad(button) => pads.iter().any(|pad| pad.just_pressed(*button)),
        }
    }

    fn binding_just_released(
        binding: &Binding,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
    ) -> bool {
        match binding {
            Binding::Key(chord) => Self::chord_just_released(chord, keys),
            Binding::Mouse(_) => mouse.just_released(match binding {
                Binding::Mouse(chord) => chord.button,
                _ => unreachable!(),
            }),
            Binding::Pad(button) => pads.iter().any(|pad| pad.just_released(*button)),
        }
    }

    /// Held this frame.
    pub fn pressed(
        &self,
        action: Action,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
    ) -> bool {
        let insensitive = action.mod_insensitive();
        self.binding(action)
            .iter()
            .any(|binding| Self::binding_down(binding, keys, mouse, pads, insensitive))
    }

    /// Begun this frame.
    pub fn just_pressed(
        &self,
        action: Action,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
    ) -> bool {
        let insensitive = action.mod_insensitive();
        self.binding(action)
            .iter()
            .any(|binding| Self::binding_just_pressed(binding, keys, mouse, pads, insensitive))
    }

    /// Released this frame.
    pub fn just_released(
        &self,
        action: Action,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
    ) -> bool {
        self.binding(action)
            .iter()
            .any(|binding| Self::binding_just_released(binding, keys, mouse, pads))
    }

    /// Keyboard + D-pad + left-stick move vector: x right, y forward (screen
    /// space, matching the touch stick). Touch input is merged by the caller.
    pub fn move_vector(
        &self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
        pads: &[&Gamepad],
    ) -> Vec2 {
        let mut wish = Vec2::ZERO;
        if self.pressed(Action::MoveForward, keys, mouse, pads) {
            wish.y += 1.0;
        }
        if self.pressed(Action::MoveBack, keys, mouse, pads) {
            wish.y -= 1.0;
        }
        if self.pressed(Action::MoveRight, keys, mouse, pads) {
            wish.x += 1.0;
        }
        if self.pressed(Action::MoveLeft, keys, mouse, pads) {
            wish.x -= 1.0;
        }
        // Standard layout: the left stick always moves, on top of buttons.
        for pad in pads {
            let stick = pad.left_stick();
            if stick.length() > STICK_DEADZONE {
                wish += Vec2::new(stick.x, stick.y);
            }
        }
        if wish.length() > 1.0 {
            wish = wish.normalize_or_zero();
        }
        wish
    }

    /// Right-stick orbit deflection for the camera (standard fixed layout).
    pub fn camera_stick(pads: &[&Gamepad]) -> Vec2 {
        let mut out = Vec2::ZERO;
        for pad in pads {
            let stick = pad.right_stick();
            if stick.length() > STICK_DEADZONE {
                out += Vec2::new(stick.x, stick.y);
            }
        }
        if out.length() > 1.0 {
            out = out.normalize_or_zero();
        }
        out
    }

    /// Short label for the first binding of an action ("W", "LMB" ...),
    /// for hints and the settings page.
    pub fn primary_label(&self, action: Action) -> String {
        self.binding(action)
            .first()
            .map(binding_label)
            .unwrap_or_else(|| "—".to_string())
    }

    fn to_stored(&self) -> StoredControls {
        StoredControls {
            version: CONTROLS_VERSION,
            bindings: self
                .bindings
                .iter()
                .map(|(action, bindings)| {
                    (
                        action.id().to_owned(),
                        bindings.iter().map(StoredBinding::from_binding).collect(),
                    )
                })
                .collect(),
            touch: self
                .touch
                .iter()
                .map(|(button, action)| {
                    (
                        button.id().to_owned(),
                        action.map(|action| action.id().to_owned()),
                    )
                })
                .collect(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.to_stored()).expect("controls serialize to JSON")
    }

    /// Parse stored JSON over the defaults: unknown actions and bad entries
    /// are ignored; missing actions keep their defaults.
    pub fn from_json(json: &str) -> Option<Self> {
        let stored: StoredControls = serde_json::from_str(json).ok()?;
        if stored.version != CONTROLS_VERSION {
            return None;
        }
        let mut controls = Self::with_defaults();
        for (id, stored_bindings) in stored.bindings {
            let Some(action) = Action::parse(&id) else {
                continue;
            };
            let bindings: Vec<Binding> = stored_bindings
                .iter()
                .filter_map(StoredBinding::to_binding)
                .collect();
            // An explicit empty list unbinds the action; anything that parses
            // replaces the defaults.
            if stored_bindings.is_empty() || !bindings.is_empty() {
                controls.bindings.insert(action, bindings);
            }
        }
        for (button_id, action_id) in stored.touch {
            let Some(button) = ClusterButton::parse(&button_id) else {
                continue;
            };
            match action_id {
                // Explicit null disables the button.
                None => {
                    controls.touch.insert(button, None);
                }
                // Only game actions make sense on a momentary touch button;
                // anything else keeps the default.
                Some(id) => {
                    if let Some(action) = parse_touch_target(&id) {
                        controls.touch.insert(button, Some(action));
                    }
                }
            }
        }
        Some(controls)
    }

    /// Load persisted bindings (browser storage on web), else defaults.
    pub fn load() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self::load_from_browser().unwrap_or_else(Self::with_defaults)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::with_defaults()
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn load_from_browser() -> Option<Self> {
        let window = web_sys::window()?;
        let storage = window.local_storage().ok()??;
        let json = storage.get_item(CONTROLS_STORAGE_KEY).ok()??;
        Self::from_json(&json)
    }

    /// Same-chord bindings on two actions in one context. Defaults must be
    /// empty; the settings page blocks anything that would add one.
    pub fn default_conflicts() -> Vec<(Action, Action, String)> {
        let controls = Self::with_defaults();
        let mut out = Vec::new();
        for (index, left) in Action::ALL.iter().enumerate() {
            for right in &Action::ALL[index + 1..] {
                if left.context() != right.context() {
                    continue;
                }
                for binding in controls.binding(*left) {
                    if controls.binding(*right).contains(binding)
                        && !out.iter().any(|(a, b, _): &(Action, Action, String)| {
                            (*a == *left && *b == *right) || (*a == *right && *b == *left)
                        })
                    {
                        out.push((*left, *right, chord_label(binding)));
                    }
                }
            }
        }
        out
    }

    /// Actions sharing one context that would also fire on `binding`.
    pub fn conflicts_with(&self, action: Action, binding: &Binding) -> Vec<Action> {
        Action::ALL
            .iter()
            .filter(|other| {
                **other != action
                    && other.context() == action.context()
                    && self.binding(**other).contains(binding)
            })
            .copied()
            .collect()
    }
}

/// One bundled input read for gameplay and editor systems. Counts as a
/// single system parameter while exposing keys, mouse, gamepads and the
/// remappable [`Controls`] table together.
#[derive(SystemParam)]
pub struct PlayerInput<'w, 's> {
    pub keys: Res<'w, ButtonInput<KeyCode>>,
    pub mouse: Res<'w, ButtonInput<MouseButton>>,
    pub pads: Query<'w, 's, &'static Gamepad>,
    pub controls: Res<'w, Controls>,
}

impl PlayerInput<'_, '_> {
    fn gamepads(&self) -> Vec<&Gamepad> {
        self.pads.iter().collect()
    }

    /// Held this frame.
    pub fn pressed(&self, action: Action) -> bool {
        let pads = self.gamepads();
        self.controls
            .pressed(action, &self.keys, &self.mouse, &pads)
    }

    /// Begun this frame.
    pub fn just_pressed(&self, action: Action) -> bool {
        let pads = self.gamepads();
        self.controls
            .just_pressed(action, &self.keys, &self.mouse, &pads)
    }

    /// Released this frame.
    pub fn just_released(&self, action: Action) -> bool {
        let pads = self.gamepads();
        self.controls
            .just_released(action, &self.keys, &self.mouse, &pads)
    }

    /// Keyboard + D-pad + left-stick move vector. Touch input stays merged
    /// by the caller.
    pub fn move_vector(&self) -> Vec2 {
        let pads = self.gamepads();
        self.controls.move_vector(&self.keys, &self.mouse, &pads)
    }

    /// Right-stick orbit deflection (standard fixed layout).
    pub fn camera_stick(&self) -> Vec2 {
        let pads = self.gamepads();
        Controls::camera_stick(&pads)
    }
}

/// Canonical trigger text used for conflict messages and the settings UI.
pub fn chord_label(binding: &Binding) -> String {
    match binding {
        Binding::Key(chord) => {
            let mut parts = Vec::new();
            if chord.ctrl {
                parts.push("Ctrl".to_string());
            }
            if chord.shift {
                parts.push("Shift".to_string());
            }
            if chord.alt {
                parts.push("Alt".to_string());
            }
            parts.push(key_label(&chord.key));
            parts.join("+")
        }
        Binding::Mouse(chord) => {
            let mut label = mouse_label(&chord.button);
            if chord.shift {
                label = format!("Shift+{label}");
            }
            label
        }
        Binding::Pad(button) => pad_label(button),
    }
}

fn binding_label(binding: &Binding) -> String {
    chord_label(binding)
}

fn key_label(key: &KeyCode) -> String {
    let name = keycode_name(*key);
    if let Some(rest) = name.strip_prefix("Key") {
        return rest.to_string();
    }
    if let Some(rest) = name.strip_prefix("Digit") {
        return rest.to_string();
    }
    if let Some(rest) = name.strip_prefix("Numpad") {
        return format!("Num{rest}");
    }
    match name {
        "ArrowUp" => "↑".to_string(),
        "ArrowDown" => "↓".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        "Space" => "Space".to_string(),
        "Escape" => "Esc".to_string(),
        "Backspace" => "⌫".to_string(),
        "Delete" => "Del".to_string(),
        "SuperLeft" | "SuperRight" => "Cmd".to_string(),
        "Unknown" => "?".to_string(),
        other => other.to_string(),
    }
}

fn mouse_label(button: &MouseButton) -> String {
    match mousebutton_name(*button) {
        "Left" => "LMB".to_string(),
        "Right" => "RMB".to_string(),
        "Middle" => "MMB".to_string(),
        "Back" => "Mouse4".to_string(),
        "Forward" => "Mouse5".to_string(),
        other => other.to_string(),
    }
}

fn pad_label(button: &GamepadButton) -> String {
    match button {
        GamepadButton::South => "A".to_string(),
        GamepadButton::East => "B".to_string(),
        GamepadButton::North => "Y".to_string(),
        GamepadButton::West => "X".to_string(),
        GamepadButton::C => "C".to_string(),
        GamepadButton::Z => "Z".to_string(),
        GamepadButton::LeftTrigger => "LB".to_string(),
        GamepadButton::LeftTrigger2 => "LT".to_string(),
        GamepadButton::RightTrigger => "RB".to_string(),
        GamepadButton::RightTrigger2 => "RT".to_string(),
        GamepadButton::Select => "Back".to_string(),
        GamepadButton::Start => "Start".to_string(),
        GamepadButton::Mode => "Home".to_string(),
        GamepadButton::LeftThumb => "L3".to_string(),
        GamepadButton::RightThumb => "R3".to_string(),
        GamepadButton::DPadUp => "D-Up".to_string(),
        GamepadButton::DPadDown => "D-Down".to_string(),
        GamepadButton::DPadLeft => "D-Left".to_string(),
        GamepadButton::DPadRight => "D-Right".to_string(),
        GamepadButton::Other(index) => format!("Pad{index}"),
    }
}

/// Short pretty names for every action, used by the settings page contract
/// test below and kept in sync with the home-screen table.
#[cfg(test)]
pub fn action_catalog() -> Vec<(&'static str, &'static str, &'static str)> {
    Action::ALL
        .iter()
        .map(|action| (action.id(), action.label(), action.group()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn pressed_keys(keys: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut input = ButtonInput::<KeyCode>::default();
        for key in keys {
            input.press(*key);
        }
        input
    }

    fn released_keys() -> ButtonInput<KeyCode> {
        ButtonInput::<KeyCode>::default()
    }

    fn mouse() -> ButtonInput<MouseButton> {
        ButtonInput::<MouseButton>::default()
    }

    #[test]
    fn defaults_have_no_same_context_conflicts() {
        assert_eq!(Controls::default_conflicts(), Vec::new());
    }

    #[test]
    fn every_action_has_an_id_label_and_group() {
        let mut ids = HashSet::new();
        for action in Action::ALL {
            assert!(ids.insert(action.id()), "duplicate id");
            assert!(!action.label().is_empty());
            assert!(!action.group().is_empty());
            assert_eq!(Action::parse(action.id()), Some(*action));
        }
        assert_eq!(ids.len(), Action::ALL.len());
    }

    #[test]
    fn chords_match_modifiers_exactly_unless_insensitive() {
        let controls = Controls::with_defaults();
        let pads: [&Gamepad; 0] = [];
        // Plain S saves; Shift+S must not.
        let s = pressed_keys(&[KeyCode::KeyS]);
        assert!(controls.pressed(Action::SaveLevel, &s, &mouse(), &pads));
        assert!(!controls.pressed(Action::CopyJson, &s, &mouse(), &pads));
        let shift_s = pressed_keys(&[KeyCode::KeyS, KeyCode::ShiftLeft]);
        assert!(!controls.pressed(Action::SaveLevel, &shift_s, &mouse(), &pads));
        assert!(controls.pressed(Action::CopyJson, &shift_s, &mouse(), &pads));
        // Movement ignores extra modifiers so sprint-walking works.
        let shift_w = pressed_keys(&[KeyCode::KeyW, KeyCode::ShiftLeft]);
        assert!(controls.pressed(Action::MoveForward, &shift_w, &mouse(), &pads));
        assert!(controls.pressed(Action::Sprint, &shift_w, &mouse(), &pads));
    }

    #[test]
    fn ctrl_or_command_both_count_as_ctrl() {
        let controls = Controls::with_defaults();
        let pads: [&Gamepad; 0] = [];
        let cmd_z = pressed_keys(&[KeyCode::KeyZ, KeyCode::SuperLeft]);
        assert!(controls.pressed(Action::Undo, &cmd_z, &mouse(), &pads));
        let alt_z = pressed_keys(&[KeyCode::KeyZ, KeyCode::AltLeft]);
        assert!(!controls.pressed(Action::Undo, &alt_z, &mouse(), &pads));
    }

    #[test]
    fn bindings_round_trip_through_json() {
        let controls = Controls::with_defaults();
        let json = controls.to_json();
        assert!(json.contains("\"moveForward\""), "{json}");
        let loaded = Controls::from_json(&json).expect("reloads");
        assert_eq!(loaded.binding(Action::Jump), controls.binding(Action::Jump));
        // Unknown actions and bad versions fall back gracefully.
        assert!(Controls::from_json(r#"{"version":1,"bindings":{"nope":[]}}"#).is_some());
        assert!(Controls::from_json(r#"{"version":999,"bindings":{}}"#).is_none());
        assert!(Controls::from_json("not json").is_none());
    }

    #[test]
    fn unbound_actions_report_nothing() {
        let mut controls = Controls::with_defaults();
        controls.set_test_bindings(Action::Jump, Vec::new());
        let pads: [&Gamepad; 0] = [];
        assert!(!controls.pressed(
            Action::Jump,
            &pressed_keys(&[KeyCode::Space]),
            &mouse(),
            &pads
        ));
        assert_eq!(controls.primary_label(Action::Jump), "—");
    }

    #[test]
    fn touch_buttons_default_to_abxy() {
        let controls = Controls::with_defaults();
        assert_eq!(controls.touch_action(ClusterButton::A), Some(Action::Jump));
        assert_eq!(controls.touch_action(ClusterButton::B), Some(Action::Slide));
        assert_eq!(
            controls.touch_action(ClusterButton::C),
            Some(Action::Interact),
            "the C pad button interacts on phones"
        );
        assert_eq!(
            controls.touch_action(ClusterButton::X),
            Some(Action::Shoot),
            "the X pad button fires a cone on phones"
        );
        assert_eq!(
            controls.touch_action(ClusterButton::Y),
            Some(Action::Sprint)
        );
        assert_eq!(
            controls.touch_action(ClusterButton::Z),
            None,
            "Z starts disabled as the spare remappable button"
        );
        assert_eq!(ClusterButton::parse("A"), Some(ClusterButton::A));
        assert_eq!(ClusterButton::parse("C"), Some(ClusterButton::C));
        assert_eq!(ClusterButton::parse("Z"), Some(ClusterButton::Z));
        assert_eq!(ClusterButton::parse("nope"), None);
    }

    #[test]
    fn touch_map_round_trips_and_validates() {
        let mut controls = Controls::with_defaults();
        controls.set_test_touch(ClusterButton::B, Some(Action::Jump));
        controls.set_test_touch(ClusterButton::Y, None);
        let json = controls.to_json();
        assert!(json.contains("\"touch\""), "{json}");
        let loaded = Controls::from_json(&json).expect("reloads");
        assert_eq!(loaded.touch_action(ClusterButton::A), Some(Action::Jump));
        assert_eq!(loaded.touch_action(ClusterButton::B), Some(Action::Jump));
        assert_eq!(loaded.touch_action(ClusterButton::Y), None);

        // Unknown buttons are ignored; non-game actions and bad ids fall
        // back to the default instead of disabling the button.
        let loaded = Controls::from_json(
            r#"{"version":1,"bindings":{},"touch":{"A":"saveLevel","B":"bogus","Q":"jump","X":null}}"#,
        )
        .expect("reloads");
        assert_eq!(loaded.touch_action(ClusterButton::A), Some(Action::Jump));
        assert_eq!(loaded.touch_action(ClusterButton::B), Some(Action::Slide));
        assert_eq!(loaded.touch_action(ClusterButton::X), None);
    }

    #[test]
    fn release_edges_fire_once() {
        let controls = Controls::with_defaults();
        let pads: [&Gamepad; 0] = [];
        let mut held = pressed_keys(&[KeyCode::Space]);
        held.release(KeyCode::Space);
        assert!(controls.just_released(Action::Jump, &held, &mouse(), &pads));
        assert!(!controls.just_released(Action::Jump, &released_keys(), &mouse(), &pads));
        assert!(!controls.pressed(Action::Jump, &released_keys(), &mouse(), &pads));
    }

    #[test]
    fn conflicts_with_names_same_context_overlaps() {
        let controls = Controls::with_defaults();
        let dup = Binding::Key(KeyChord::key(KeyCode::KeyW));
        assert_eq!(
            controls.conflicts_with(Action::MoveBack, &dup),
            vec![Action::MoveForward]
        );
        // Cross-context overlaps are allowed by design: Enter confirms in
        // the menu and the modal and walks a route globally.
        let enter = Binding::Key(KeyChord::key(KeyCode::Enter));
        assert_eq!(
            controls.conflicts_with(Action::ConfirmModal, &enter),
            Vec::new()
        );
        assert_eq!(
            controls.conflicts_with(
                Action::MenuUp,
                &Binding::Key(KeyChord::key(KeyCode::ArrowUp))
            ),
            Vec::new()
        );
    }
}
