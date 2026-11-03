//! A 3D level editor: a component palette on the left, an orbit camera, and
//! solid meshes extruded from the same footprints the model tests use.
//!
//! Left-drag places and moves, right-drag orbits, middle-drag pans, the wheel
//! zooms. Everything is in metres.

use std::collections::{HashMap, HashSet};

use avian3d::character_controller::prelude::{
    MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitResponse,
};
use avian3d::math::{ToF32Precision, ToRealPrecision};
use avian3d::prelude::*;
use bevy::clipboard::Clipboard;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::input::touch::Touches;
use bevy::input_focus::InputFocus;
use bevy::picking::Pickable;
use bevy::picking::events::{
    PointerCancel, PointerDrag, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::prelude::*;
use bevy::shape::InfinitePlane3d;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

use crate::controls::{Action, ClusterButton, Controls, PlayerInput};
use crate::locomotion::{LocomotionInput, LocomotionState};
use crate::mesh::{extrude, wedge};
use crate::model::{
    self, AnchorKind, CATEGORIES, HIT_PAD, InteractAction, PANEL_WIDTH, Piece, SNAP_THRESHOLD,
    SnapKind, Style, shapes,
};
use crate::path;
use crate::path_edit::{self, PathDrag, PathHandle, Plan, TangentSide};
use crate::session;
use crate::statuses::Health;

const PANEL_BG: Color = Color::srgb_u8(0x10, 0x16, 0x1e);
const BUTTON_BG: Color = Color::srgb_u8(0x14, 0x1c, 0x26);
const BUTTON_ON: Color = Color::srgb_u8(0x33, 0x27, 0x12);
const BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);
const BORDER_ON: Color = Color::srgb_u8(0xb0, 0x77, 0x28);
const MUTED: Color = Color::srgb_u8(0x86, 0x97, 0xab);
const AMBER: Color = Color::srgb_u8(0xf0, 0xa6, 0x3c);
const GRID: Color = Color::srgba(0.47, 0.59, 0.71, 0.10);
const GRID_HOVER: Color = Color::srgba(0.47, 0.66, 0.86, 0.45);
const GRID_Y: f32 = 0.004;
const PATH: Color = Color::srgb_u8(0x5f, 0xe0, 0xc0);
const GIZMO_X: Color = Color::srgb_u8(0xe0, 0x5a, 0x52);
const GIZMO_Y: Color = Color::srgb_u8(0x62, 0xc9, 0x6b);
const GIZMO_Z: Color = Color::srgb_u8(0x5a, 0x8c, 0xe0);
/// Gizmo handles are sized against the selected piece's bounding box and stay
/// bigger than it, so big selections keep visible handles and small ones keep a
/// grabbable one.
const GIZMO_GROWTH: f32 = 1.25;
/// Yaw ring radius as a fraction of the piece's bounding box.
const GIZMO_RING_SCALE: f32 = 0.8;
/// Screen-space grab tolerance for an axis handle, in logical pixels.
const GIZMO_HANDLE_HIT: f32 = 10.0;
/// What fraction of the yaw ring radius still counts as the handle.
const GIZMO_YAW_HIT: f32 = 0.1;
/// Arrow head back offset, as a fraction of the handle length.
const GIZMO_ARROW_BACK: f32 = 0.16;
/// Arrow head side offset, as a fraction of the handle length.
const GIZMO_ARROW_SIDE: f32 = 0.07;
/// How far along its two axes a plane handle sits, as a fraction of the reach.
const GIZMO_PLANE_CENTER: f32 = 0.42;
/// Half-width of a plane handle, as a fraction of the reach.
const GIZMO_PLANE_SIZE: f32 = 0.09;
/// How close a dragged piece must come to a surface top before it snaps onto
/// it, in metres. Away from a surface the drag is free: there is no height
/// grid competing with the surface snap.
const ELEVATION_SNAP: f32 = 0.2;
const PILL_COLOR: Color = Color::srgb_u8(0x3f, 0xd0, 0xb0);
const BAR_BG: Color = Color::srgba(0.04, 0.05, 0.07, 0.82);
/// Eye height and capsule dimensions, in metres.
const EYE_HEIGHT: f32 = 1.55;
const PILL_RADIUS: f32 = 0.32;
const PILL_CYLINDER: f32 = 1.1;
const PILL_HALF: f32 = PILL_CYLINDER * 0.5 + PILL_RADIUS;
/// Walking pace, in metres per second.
const WALK_SPEED: f32 = 2.2;
/// Gravity, in m/s^2. A little above earth so the character does not float.
const GRAVITY: f32 = 18.0;
const JUMP_SPEED: f32 = 7.0;
/// Touch stick geometry, in logical pixels.
const STICK_BASE: f32 = 132.0;
const STICK_MARGIN: f32 = 20.0;
const STICK_KNOB: f32 = 52.0;
const STICK_REACH: f32 = 110.0;
const STICK_TRAVEL: f32 = 44.0;
/// Longest press that still counts as a canvas tap. Generous because a slow
/// frame can separate the press and release; movement is the real drag test.
const TOUCH_TAP_MAX_TIME: f64 = 1.0;

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Editor>()
            .init_resource::<Cursor>()
            .init_resource::<Orbit>()
            .init_resource::<LastSaved>()
            .init_resource::<SaveClock>()
            .init_resource::<PointerCapture>()
            .init_resource::<Visuals>()
            // One UI unit per logical pixel unless a phone display scales it
            // down; a zero scale would put the stick centre off screen.
            .insert_resource(TouchState {
                ui_scale: 1.0,
                ..Default::default()
            })
            .init_resource::<PathGesture>()
            .init_resource::<UiScaleApplied>()
            .init_resource::<Nav>()
            .init_resource::<PropertyEditState>()
            .init_resource::<DamageClock>()
            .init_resource::<Movers>()
            .init_resource::<AddMenu>()
            .init_resource::<OutlinerState>()
            .init_resource::<OutlinerEdit>()
            .init_resource::<PropertiesPanelState>()
            // Remappable controls, loaded from browser storage on web.
            .insert_resource(crate::controls::Controls::load())
            .add_systems(
                Startup,
                (load_session, setup, setup_wasm_text_input).chain(),
            )
            .add_systems(
                Update,
                (
                    setup_ui_scale,
                    touch_input,
                    sync_input_mode,
                    sync_mode_ui,
                    sync_touch_path_row,
                    sync_knob,
                    sync_shape_label,
                    walk,
                    flush_platform_requests,
                    prune_movers,
                    move_movers,
                    character_move,
                    hazard_damage,
                    handle_interact,
                    respawn_on_death,
                    manage_capture,
                    camera_control,
                    path_drag,
                    rebuild_nav,
                    refresh_route,
                )
                    .chain(),
            )
            // The second half is ordered after `refresh_route` rather than in
            // the same tuple: Bevy's tuple system sets top out before this many
            // systems.
            .add_systems(
                Update,
                (
                    resolve_platform_push,
                    update_cursor,
                    sync_dynamic_pieces,
                    touch_gizmo_fallback,
                    sync_wasm_text_input,
                    update_add_menu,
                    save_shortcut,
                    interact,
                    apply_property_edits,
                    sync_visuals,
                )
                    .chain()
                    .after(refresh_route),
            )
            .add_systems(
                Update,
                (
                    outliner_shortcuts,
                    sync_outliner_selection,
                    apply_outliner_edit,
                    sync_outliner_rows,
                    sync_outliner_panel,
                    sync_outliner_fields,
                    update_outliner_rows,
                    outliner_scroll,
                )
                    .chain()
                    .after(sync_visuals),
            )
            .add_systems(
                Update,
                (
                    handle_transparency_slider,
                    sync_properties_chrome,
                    sync_properties_panel,
                    sync_transparency_slider,
                    sync_trigger_color_buttons,
                    sync_trigger_visible_label,
                    update_palette,
                    sync_path_actions,
                    update_touch_buttons,
                    update_status,
                    update_touch_status,
                    update_character_status,
                    draw,
                    autosave_session,
                )
                    .chain()
                    .after(sync_visuals),
            )
            .add_observer(on_palette_activate)
            .add_observer(on_touch_button)
            .add_observer(on_add_menu_item);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct Editor {
    session: Option<String>,
    pieces: Vec<Piece>,
    /// User labels, groups, and group membership layered over the pieces.
    outline: SceneOutline,
    tool: Option<usize>,
    actor_tool: Option<ActorTool>,
    /// Build authors the level; Play hands the capsule to the player.
    mode: Mode,
    character: Option<Character>,
    plan: Option<Plan>,
    route: Option<Route>,
    walk: Option<Walk>,
    selected: Option<u32>,
    /// The last copied piece, ready to paste as a new object.
    clipboard: Option<Piece>,
    next_id: u32,
    snap: bool,
    ghost_rot: f32,
    ghost: Option<Ghost>,
    drag: Option<Drag>,
    gizmo_drag: Option<GizmoDrag>,
    keyboard_transform: Option<KeyboardTransform>,
    undo: Vec<LevelState>,
    redo: Vec<LevelState>,
    /// Set by any change to the level itself, including undo and redo.
    hard_dirty: bool,
    /// Set by changes that are not level edits.
    soft_dirty: bool,
    note: &'static str,
    /// Shape picked by the mobile carousel.
    mobile_shape: usize,
    /// Last palette action and when it fired, so a touch plus its synthesized
    /// mouse click cannot toggle a tool twice.
    last_palette: Option<(PaletteAction, f64)>,
    /// Navigation revision the current route was planned against.
    seen_nav: u64,
    /// One pending platform waypoint from the Motion tool, drained by movers.
    platform_request: Option<(u32, Vec2)>,
}

/// One undo step: the level geometry plus its authoring metadata, so grouping
/// and labels undo alongside placement, transforms and deletes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LevelState {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) outline: SceneOutline,
}

#[derive(Clone, PartialEq)]
struct Seen {
    orbit: Orbit,
    character: Option<Character>,
}

#[derive(Resource, Default)]
struct LastSaved(Option<Seen>);

#[derive(Resource, Default)]
struct SaveClock {
    last_save: f64,
    retry_at: f64,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            session: None,
            pieces: Vec::new(),
            outline: SceneOutline::default(),
            tool: None,
            actor_tool: None,
            mode: Mode::Build,
            character: None,
            plan: None,
            route: None,
            walk: None,
            selected: None,
            clipboard: None,
            next_id: 1,
            snap: true,
            ghost_rot: 0.0,
            ghost: None,
            drag: None,
            gizmo_drag: None,
            keyboard_transform: None,
            undo: Vec::new(),
            redo: Vec::new(),
            hard_dirty: false,
            soft_dirty: false,
            note: "Click a component, then the grid",
            mobile_shape: 0,
            last_palette: None,
            seen_nav: 0,
            platform_request: None,
        }
    }
}

/// The palette can arm a building block, the character, or one of the path
/// tools (character route, plan, or platform motion).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActorTool {
    Place,
    Path,
    Plan,
    Platform,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Character {
    pub(crate) pos: Vec2,
    /// Height of the capsule's feet above the world floor.
    pub(crate) feet: f32,
    pub(crate) yaw: f32,
    /// Vertical velocity from the physics step.
    pub(crate) vy: f32,
    /// Horizontal velocity in world XZ (`x` = world x, `y` = world z).
    /// Ground control drives this toward the wish velocity; in the air it
    /// carries inertia and only gains limited strafing acceleration.
    pub(crate) hv: Vec2,
    /// Platform velocity underfoot, stamped by `move_movers` each frame and
    /// zero when not riding. A jump adds this to `hv` so leaps inherit the
    /// ride; landing subtracts it back so repeated hops cannot stack speed.
    pub(crate) ride_vel: Vec2,
    pub(crate) grounded: bool,
    /// True while the capsule is in a landing slide: it entered on touchdown
    /// with Left Ctrl held after being airborne, keeps its horizontal
    /// momentum with only low friction, and ends when Ctrl is released, a
    /// jump takes off, it leaves the ground, or it slows to a walk.
    pub(crate) sliding: bool,
    pub(crate) health: Health,
}

impl Character {
    /// Velocity in world metres per second: `x`/`z` horizontal, `y` vertical.
    /// The platform underfoot is folded in, so a rider shows its world motion.
    pub(crate) fn velocity(self) -> Vec3 {
        let horizontal = self.hv + self.ride_vel;
        Vec3::new(horizontal.x, self.vy, horizontal.y)
    }

    /// Speed in metres per second, ignoring direction.
    pub(crate) fn speed(self) -> f32 {
        self.velocity().length()
    }

    /// Snapshot of the physics fields the locomotion state machine reads.
    pub(crate) fn locomotion_input(self) -> LocomotionInput {
        LocomotionInput::new(
            self.grounded,
            self.sliding,
            (self.hv + self.ride_vel).length(),
            self.vy,
            self.health.is_dead(),
        )
    }

    /// What the character is doing right now: stationary, walking, running,
    /// sliding, jumping, falling, or dead. See [`crate::locomotion`].
    pub(crate) fn locomotion_state(self) -> LocomotionState {
        crate::locomotion::classify(self.locomotion_input())
    }
}

/// A routed path with arc length, so the walker moves at a constant speed.
#[derive(Clone)]
struct Route {
    track: path::Track,
    /// The walked part of the line, frozen across live edits.
    trail: Vec<Vec3>,
    /// Lifted samples of the current (future) track, ready to draw.
    line: Vec<Vec3>,
    /// Arc length of each waypoint along `track`.
    distances: Vec<f32>,
    /// Curve handles aligned with the waypoints.
    controls: Vec<path::AnchorControls>,
    /// False when a level change made the plan unreachable.
    valid: bool,
    hexes: usize,
}

#[derive(Clone, Copy)]
struct Walk {
    distance: f32,
    stuck: f32,
}

#[derive(Clone, Copy)]
struct Ghost {
    pos: Vec2,
    rot: f32,
    kind: SnapKind,
}

#[derive(Clone, Copy)]
struct Drag {
    id: u32,
    offset: Vec2,
    /// Height of the horizontal plane the drag moves in. Fixed at grab time
    /// so the piece tracks the pointer 1:1 instead of sliding with the
    /// ground-plane projection when the piece is elevated.
    plane: f32,
    moved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GizmoHandle {
    X,
    Y,
    Z,
    /// Pitch around the world's X axis.
    Pitch,
    /// Yaw around the world's vertical axis.
    Yaw,
    /// Roll around the world's Z axis.
    Roll,
    /// Vertical plane spanned by X and Y.
    Xy,
    /// Vertical plane spanned by Z and Y.
    Zy,
    /// Ground plane spanned by X and Z.
    Xz,
}

#[derive(Clone, Copy)]
struct GizmoDrag {
    id: u32,
    handle: GizmoHandle,
    /// World point grabbed when the drag started, so the piece moves by the
    /// pointer's travel rather than jumping to the pointer.
    grab: Vec3,
    /// The selected piece at the moment of the grab.
    start: Piece,
    /// Raw elevation at the previous frame, so a snap can tell which way the
    /// piece is travelling.
    last_raw: f32,
    /// Elevation applied last frame, kept while the pointer is still.
    last: f32,
    /// Which pointer owns the drag. Desktop uses the mouse (driven from the
    /// live cursor in `interact`); a finger drag is driven from pointer
    /// events in `touch`, where there is no cursor while the finger is down.
    pointer: PointerId,
    /// Latest screen position of the owning pointer. Updated by the move
    /// observers; only the touch path reads it.
    screen: Vec2,
}

/// Blender-style modal transform: `G` for move, `R` for rotate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TransformMode {
    Move,
    Rotate,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TransformAxis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug)]
struct KeyboardTransform {
    id: u32,
    mode: TransformMode,
    /// `None` is the free plane for moves and yaw for rotations.
    axis: Option<TransformAxis>,
    /// The piece before the modal began, restored by Escape or right click.
    original: Piece,
    /// The piece at the last axis change; the pointer moves relative to this.
    base: Piece,
    /// Pointer-derived point where the current axis constraint was grabbed.
    grab: Vec3,
    /// True once the first movement has pushed an undo snapshot.
    committed: bool,
}

/// Everything the touch layer reads, rebuilt each frame from `Touches`.
#[derive(Resource, Default)]
struct UiScaleApplied(bool);

#[derive(Resource, Default)]
struct TouchState {
    /// Set the first time any touch arrives; switches the whole UI over.
    enabled: bool,
    /// Stick direction: x right, y forward, length <= 1.
    stick: Vec2,
    /// One-finger drag in logical pixels, for looking and orbiting.
    look: Vec2,
    /// Two-finger centroid drag.
    pan: Vec2,
    /// Fractional pinch change; positive means fingers moved apart.
    zoom: f32,
    /// Screen position of a tap that finished this frame.
    tap: Option<Vec2>,
    /// Set by the jump button, consumed by the movement system.
    jump: bool,
    /// Set by the interact button, consumed by the interaction system.
    interact: bool,
    /// Toggled by the sprint cluster button; ORed with the Sprint action.
    sprint: bool,
    /// Toggled by the slide cluster button; ORed with the Slide action and
    /// read as "crouch held" once slide becomes crouch.
    slide: bool,
    /// Ignore canvas taps until this time; a UI button tap must not also place.
    suppress_until: f64,
    /// Last touch action and when it fired, for the same double-fire guard.
    last_action: Option<(TouchAction, f64)>,
    /// Sticky touch equivalent of Shift: append rather than replace.
    append: bool,
    /// A path handle owns the current touch gesture; suppress camera look.
    path_drag_active: bool,
    /// Sticky for the current gesture: more than one finger has been down.
    multi: bool,
    /// Pointer holding the stick, in the picking/UI coordinate space. Using the
    /// pointer position instead of raw touch coordinates keeps the stick lined
    /// up with the drawn pad at any DPR or UiScale.
    stick_pointer: Option<PointerId>,
    tracks: HashMap<u64, TouchTrack>,
    two_finger: Option<(Vec2, f32)>,
    /// UI scale on this display: 1 / window scale factor.
    ui_scale: f32,
}

#[derive(Clone, Copy)]
struct TouchTrack {
    last: Vec2,
    /// Total distance travelled since the press, for tap detection.
    moved: f32,
    /// Press time, so a long press is a drag, not a tap.
    time: f64,
    stick: bool,
    control: bool,
}

/// Mouse/touch gesture ownership for path handles and taps.
#[derive(Resource, Default)]
struct PathGesture(path_edit::Gesture);

#[derive(Resource, Default)]
struct Cursor {
    screen: Option<Vec2>,
    /// Logical size of the window, for UI hit tests.
    screen_size: Vec2,
    world: Option<Vec2>,
    ray: Option<Ray3d>,
    /// True while a text input owns keyboard focus.
    text_focus: bool,
    /// A tap completed this frame (touch builds/clicks).
    tap: bool,
    /// Last ground point the pointer was over; stays put between taps so the
    /// placement cursor is visible on touch.
    last_world: Option<Vec2>,
}

#[derive(Resource, Clone, PartialEq, Debug)]
pub(crate) struct Orbit {
    pub(crate) target: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) distance: f32,
    pub(crate) mode: ViewMode,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.85,
            pitch: 0.9,
            distance: 7.5,
            mode: ViewMode::Third,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewMode {
    Third,
    First,
    Top,
}

impl ViewMode {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "third" => Some(ViewMode::Third),
            "first" => Some(ViewMode::First),
            "top" => Some(ViewMode::Top),
            _ => None,
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            ViewMode::Third => "third",
            ViewMode::First => "first",
            ViewMode::Top => "top",
        }
    }

    fn next(self) -> Self {
        match self {
            ViewMode::Third => ViewMode::First,
            ViewMode::First => ViewMode::Top,
            ViewMode::Top => ViewMode::Third,
        }
    }

    fn label(self) -> &'static str {
        self.id()
    }
}

/// Pointer grab for first-person looking. Capture is separate from the view:
/// Tab lets go so the palette can be used without leaving first person, and
/// Tab over the viewport takes the pointer back.
#[derive(Resource)]
struct PointerCapture {
    grabbed: bool,
    /// The value last pushed to the window, so a refused grab is not retried
    /// every frame.
    applied: bool,
    last_mode: Mode,
    last_view: ViewMode,
    /// When the web pointer lock first looked missing. A fresh request takes a
    /// frame or two to settle, so wait before believing the lock is gone.
    unlocked_since: Option<f32>,
}

impl Default for PointerCapture {
    fn default() -> Self {
        Self {
            grabbed: false,
            applied: false,
            last_mode: Mode::Build,
            last_view: ViewMode::Third,
            unlocked_since: None,
        }
    }
}

/// One frame of capture inputs, split out so the rules stay testable without
/// a window or a real pointer.
#[derive(Clone, Copy)]
struct CaptureInput {
    mode: Mode,
    view: ViewMode,
    first_person: bool,
    over_viewport: bool,
    tab: bool,
    focus_lost: bool,
}

/// Whether the pointer should be grabbed after this frame.
fn next_capture(grabbed: bool, last_mode: Mode, last_view: ViewMode, input: CaptureInput) -> bool {
    if input.focus_lost || !input.first_person {
        return false;
    }
    if input.tab {
        return !grabbed && input.over_viewport;
    }
    if input.mode != last_mode {
        return input.mode == Mode::Play;
    }
    if input.view != last_view {
        return input.over_viewport;
    }
    grabbed
}

/// Authoring versus playing. Build owns object editing (place, select, gizmos,
/// rotate, delete); Play hands the capsule to the player while route editing
/// (waypoints and curve handles) and view switching stay available.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    #[default]
    Build,
    Play,
}

impl Mode {
    fn toggled(self) -> Self {
        match self {
            Mode::Build => Mode::Play,
            Mode::Play => Mode::Build,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Play => "play",
        }
    }
}

impl Editor {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn capture(&self, orbit: &Orbit, movers: &Movers) -> session::Snapshot {
        session::Snapshot {
            version: session::SNAPSHOT_VERSION,
            next_id: Some(self.next_id),
            pieces: self
                .pieces
                .iter()
                .map(|piece| session::SavedPiece::from_piece(piece, &self.outline))
                .collect(),
            groups: self
                .outline
                .groups
                .iter()
                .map(session::SavedGroup::from)
                .collect(),
            character: self.character.as_ref().map(session::SavedCharacter::from),
            camera: Some(session::SavedCamera::from(orbit)),
            undo: self.undo.iter().map(session::SavedLevel::from).collect(),
            redo: self.redo.iter().map(session::SavedLevel::from).collect(),
            platforms: movers
                .map
                .iter()
                .map(|(id, mover)| movers::to_saved(*id, mover))
                .collect(),
        }
    }

    fn apply_loaded(&mut self, orbit: &mut Orbit, movers: &mut Movers, loaded: session::Loaded) {
        self.pieces = loaded.pieces;
        self.outline = loaded.outline;
        self.outline.bump();
        self.character = loaded.character;
        self.undo = loaded.undo;
        self.redo = loaded.redo;
        self.next_id = loaded.next_id;
        if let Some(camera) = loaded.camera {
            *orbit = camera;
        }
        movers::apply_saved(movers, &self.pieces, &loaded.platforms);

        self.selected = None;
        self.ghost = None;
        self.drag = None;
        self.gizmo_drag = None;
        self.keyboard_transform = None;
        self.plan = None;
        self.route = None;
        self.walk = None;
        self.tool = None;
        self.actor_tool = None;
        self.platform_request = None;
        self.note = "map loaded";
    }

    /// Replace the level with the little traversal course.
    fn load_course(&mut self) {
        let (mut pieces, character) = course();

        // Fresh ids so visuals and undo keys never collide with what was here.
        let base = self.next_id.max(1);
        for (i, piece) in pieces.iter_mut().enumerate() {
            piece.id = base + i as u32;
        }
        self.next_id = base + pieces.len() as u32;

        self.pieces = pieces;
        self.outline = SceneOutline::default();
        self.outline.bump();
        self.character = Some(character);
        self.selected = None;
        self.ghost = None;
        self.drag = None;
        self.gizmo_drag = None;
        self.keyboard_transform = None;
        self.route = None;
        self.walk = None;
        self.tool = None;
        self.actor_tool = None;
        self.plan = None;
        self.undo.clear();
        self.redo.clear();
        self.hard_dirty = true;
        self.note = "course: walk straight, up the ramp, over the block, drop";
    }

    fn snapshot(&self) -> LevelState {
        LevelState {
            pieces: self.pieces.clone(),
            outline: self.outline.clone(),
        }
    }

    fn commit(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > session::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.hard_dirty = true;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            let revision = self.outline.revision;
            self.redo.push(self.snapshot());
            if self.redo.len() > session::MAX_HISTORY {
                self.redo.remove(0);
            }
            self.pieces = prev.pieces;
            self.outline = prev.outline;
            // Keep the UI's rebuild counter monotonic across history moves.
            self.outline.revision = revision.wrapping_add(1);
            self.selected = None;
            self.hard_dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            let revision = self.outline.revision;
            self.undo.push(self.snapshot());
            if self.undo.len() > session::MAX_HISTORY {
                self.undo.remove(0);
            }
            self.pieces = next.pieces;
            self.outline = next.outline;
            self.outline.revision = revision.wrapping_add(1);
            self.selected = None;
            self.hard_dirty = true;
        }
    }
}

/// The walkable hex graph. Rebuilt from the pieces whenever they change, so a
/// block or ramp is in the graph for the very next route query.
#[derive(Resource, Default)]
struct Nav {
    edges: HashSet<path::Edge>,
    hexes: HashSet<path::Hex>,
    /// Snapshot of the pieces the graph was built from.
    pieces: Vec<Piece>,
    revision: u64,
}

#[derive(Component)]
struct PaletteButton(PaletteAction);

/// Desktop Walk/Smooth row, shown only while Plan mode is active.
#[derive(Component)]
struct PathActions;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteAction {
    Build,
    Play,
    View,
    Shape(usize),
    Character,
    Path,
    Plan,
    Walk,
    Smooth,
    Course,
    /// Copy the level JSON for debugging. This is not a level edit.
    CopyJson,
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct PieceVisual;

#[derive(Component)]
struct CharacterVisual;

/// Marks a UI root as belonging to the desktop or the touch layer.
#[derive(Component)]
struct InputLayer {
    desktop: bool,
}

/// Marks an on-screen overlay as belonging to one input layer and one mode.
/// `sync_mode_ui` shows it only when both match.
#[derive(Component, Clone, Copy)]
struct ModeUi {
    desktop: bool,
    play: bool,
}

fn layer_matches(ui: ModeUi, mode: Mode, touch_enabled: bool) -> bool {
    ui.desktop != touch_enabled && ui.play == (mode == Mode::Play)
}

#[derive(Component)]
#[allow(dead_code)] // visual grouping; actions are dispatched by hit-testing
struct TouchButton(TouchAction);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TouchAction {
    Play,
    View,
    Rotate,
    Delete,
    Undo,
    Redo,
    Path,
    Plan,
    Add,
    Walk,
    Smooth,
    Place,
    PrevShape,
    NextShape,
    Jump,
    /// Player interaction (E key equivalent). Live in Play, like Jump.
    Interact,
    /// Remappable A/B/X/Y cluster button. Resolved through the Controls
    /// touch map at press time.
    Pad(ClusterButton),
    Course,
    /// Open the Add (entity search) menu. Mobile-only; desktop uses F3.
    Search,
    /// Copy the level JSON for debugging. This is not a level edit.
    CopyJson,
}

#[derive(Component)]
struct TouchKnob;

#[derive(Component)]
struct TouchPathRow;

#[derive(Component)]
struct ShapeLabel;

#[derive(Component)]
struct TouchStatus;

/// One entity per piece (and one for the ghost, keyed 0).
#[derive(Resource, Default)]
struct Visuals {
    map: HashMap<u32, Entity>,
    character: Option<Entity>,
}

/// Meshes and materials, built once at startup.
#[derive(Resource)]
struct Kit {
    /// Per shape, the meshes that make it up and which material slot each uses.
    parts: Vec<Vec<(Handle<Mesh>, u8)>>,
    mats: Vec<Handle<StandardMaterial>>,
    glass: Handle<StandardMaterial>,
    ghost: Handle<StandardMaterial>,
    pill: Handle<Mesh>,
    pill_mat: Handle<StandardMaterial>,
    character_collider: Collider,
}

mod add_menu;
mod camera;
mod colliders;
mod draw;
mod editing;
mod gizmo;
mod hazards;
mod hud;
mod input;
mod interacts;
mod movers;
pub(crate) mod outline;
mod outliner;
mod overlay;
mod picking;
mod properties;
mod route;
mod save;
mod setup;
mod touch;
mod transform;
mod visuals;
mod walking;

use add_menu::*;
use camera::*;
use colliders::*;
use draw::*;
use editing::*;
use gizmo::*;
use hazards::*;
use hud::*;
use input::*;
use interacts::*;
use movers::*;
use outline::*;
use outliner::*;
use overlay::*;
use picking::*;
use properties::*;
use route::*;
use save::*;
use setup::*;
use touch::*;
use transform::*;
use visuals::*;
use walking::*;

#[cfg(test)]
mod tests;
