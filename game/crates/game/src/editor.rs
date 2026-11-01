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
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::input::touch::Touches;
use bevy::picking::Pickable;
use bevy::picking::events::{
    PointerCancel, PointerDrag, PointerDragEnd, PointerMove, PointerPress, PointerRelease,
};
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::prelude::*;
use bevy::shape::InfinitePlane3d;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};

use crate::mesh::{extrude, wedge};
use crate::model::{
    self, AnchorKind, CATEGORIES, HIT_PAD, PANEL_WIDTH, Piece, SNAP_THRESHOLD, SnapKind, Style,
    shapes,
};
use crate::path;
use crate::path_edit::{self, PathDrag, PathHandle, Plan, TangentSide};
use crate::session;

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
            .add_systems(Startup, setup)
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
                    character_move,
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
                    update_cursor,
                    interact,
                    sync_visuals,
                    update_palette,
                    sync_path_actions,
                    update_touch_buttons,
                    update_status,
                    update_touch_status,
                    draw,
                )
                    .chain()
                    .after(refresh_route),
            )
            .add_observer(on_palette_activate)
            .add_observer(on_touch_button);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct Editor {
    pieces: Vec<Piece>,
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
    undo: Vec<Vec<Piece>>,
    redo: Vec<Vec<Piece>>,
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
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            pieces: Vec::new(),
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
            undo: Vec::new(),
            redo: Vec::new(),
            hard_dirty: false,
            soft_dirty: false,
            note: "Click a component, then the grid",
            mobile_shape: 0,
            last_palette: None,
            seen_nav: 0,
        }
    }
}

/// The palette can arm a building block, the character, or the path tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActorTool {
    Place,
    Path,
    Plan,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Character {
    pub(crate) pos: Vec2,
    /// Height of the capsule's feet above the world floor.
    pub(crate) feet: f32,
    pub(crate) yaw: f32,
    /// Vertical velocity from the physics step.
    pub(crate) vy: f32,
    pub(crate) grounded: bool,
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
    moved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GizmoHandle {
    X,
    Y,
    Z,
    /// Yaw around the world's vertical axis.
    Yaw,
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
    world: Option<Vec2>,
    ray: Option<Ray3d>,
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
        self.character = Some(character);
        self.selected = None;
        self.ghost = None;
        self.drag = None;
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

    fn commit(&mut self) {
        self.undo.push(self.pieces.clone());
        if self.undo.len() > session::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.hard_dirty = true;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.pieces, prev));
            if self.redo.len() > session::MAX_HISTORY {
                self.redo.remove(0);
            }
            self.selected = None;
            self.hard_dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.pieces, next));
            if self.undo.len() > session::MAX_HISTORY {
                self.undo.remove(0);
            }
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
    Path,
    Plan,
    Add,
    Walk,
    Smooth,
    Place,
    PrevShape,
    NextShape,
    Jump,
    Course,
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

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// On a DPR-2 phone the logical viewport is half the CSS size; shrink the UI
/// by the same factor so it keeps its CSS proportions, and remember it so
/// touch coordinates can be mapped into logical space.
fn setup_ui_scale(
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

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let orbit = Orbit::default();
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
        if let model::Geometry::Wedge { w, d, h } = &shape.geom {
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
                "LMB place / select / drag  |  Y lifts, corner squares slide a plane  |  Ctrl+C / Ctrl+V copy & paste  |  Path: click route, Shift append  |  Enter walk  |  drag waypoints / curve handles  |  RMB orbit (selection in Build)  |  MMB pan  |  wheel zoom  |  V view  |  Tab mouse  |  P play / build  |  C character  |  R rotate  |  S save",
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
                width: px(170),
                flex_direction: FlexDirection::Row,
                column_gap: px(4),
                ..default()
            },
        ))
        .id();
    let play_buttons = [
        palette_compact_button(&mut commands, PaletteAction::View, "View"),
        palette_compact_button(&mut commands, PaletteAction::Build, "Exit Play"),
    ];
    commands.entity(play_bar).add_children(&play_buttons);

    spawn_touch_ui(&mut commands);
}

/// The phone layout: a toolbar, a shape carousel, a thumbstick and a jump
/// button. Play swaps the toolbar and carousel for an exit/view row. Hidden
/// until the first touch arrives.
fn spawn_touch_ui(commands: &mut Commands) {
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

fn touch_button(commands: &mut Commands, action: TouchAction, text: &str) -> Entity {
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

fn palette_label(commands: &mut Commands, text: &str) -> Entity {
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

fn palette_compact_button(commands: &mut Commands, action: PaletteAction, text: &str) -> Entity {
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

fn palette_button(commands: &mut Commands, action: PaletteAction, text: &str) -> Entity {
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

/// A solid collider matching the piece's footprint and vertical extent.
/// The body sits at the piece's ground point, so the collider is offset up by
/// half its height inside a compound.
fn piece_collider(shape: &model::Shape) -> Option<Collider> {
    let height = shape.height.max(0.02);
    let mid = shape.base + height * 0.5;

    // A wedge is convex already, so its six corners are the whole collider.
    if let model::Geometry::Wedge { w, d, h } = &shape.geom {
        let (hw, hd) = (w / 2.0, d / 2.0);
        let base = shape.base;
        let top = shape.base + *h;
        let points = [
            Vec3::new(-hw, base, -hd),
            Vec3::new(hw, base, -hd),
            Vec3::new(hw, base, hd),
            Vec3::new(-hw, base, hd),
            Vec3::new(hw, top, hd),
            Vec3::new(-hw, top, hd),
        ];
        return Collider::convex_hull(&points);
    }

    let body = match &shape.geom {
        model::Geometry::Bar { len, thick } => Collider::cuboid(*len, height, *thick),
        model::Geometry::Rect { w, h } => Collider::cuboid(*w, height, *h),
        model::Geometry::Circle { r } => Collider::cylinder(*r, height),
        model::Geometry::Hex { .. } | model::Geometry::Diamond { .. } => {
            let outline = model::shape_points(shape);
            let mut hull = Vec::with_capacity(outline.len() * 2);
            for point in &outline {
                hull.push(Vec3::new(point.x, shape.base, point.y));
                hull.push(Vec3::new(point.x, shape.top(), point.y));
            }
            Collider::convex_hull(&hull)?
        }
        _ => {
            // Cross and flag outlines are concave; their footprint box is a
            // fair collider and much cheaper.
            let outline = model::shape_points(shape);
            let (low, high) = outline_bounds(&outline);
            Collider::cuboid(
                (high.x - low.x).max(0.05),
                height,
                (high.y - low.y).max(0.05),
            )
        }
    };

    Some(Collider::compound(vec![(
        Position(Vec3::new(0.0, mid, 0.0).real()),
        Rotation::default(),
        body,
    )]))
}

fn outline_bounds(points: &[Vec2]) -> (Vec2, Vec2) {
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for point in points {
        low = low.min(*point);
        high = high.max(*point);
    }
    (low, high)
}

/// The meshes that make up one catalogue entry: outline, base, height, slot.
/// Slot 0 is the shape's own colour, slot 1 is glass.
fn solid_parts(shape: &model::Shape) -> Vec<(Vec<Vec2>, f32, f32, u8)> {
    let outline = model::shape_points(shape);
    if shape.style == Style::Window {
        let sill = 0.95;
        let head = 2.2;
        vec![
            (outline.clone(), shape.base, sill, 0),
            (outline.clone(), head, (shape.top() - head).max(0.05), 0),
            (outline, sill, (head - sill).max(0.05), 1),
        ]
    } else {
        vec![(outline, shape.base, shape.height.max(0.02), 0)]
    }
}

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

/// Vertical velocity for a physics step. A jump request only launches when the
/// character is standing on something; in the air it is ignored.
fn jump_velocity(vy: f32, grounded: bool, jump: bool) -> f32 {
    if jump && grounded { JUMP_SPEED } else { vy }
}

/// A slope pushes the character upward while it climbs, so a small positive
/// vertical speed still counts as ground. A real jump rises much faster.
const MAX_GROUNDED_RISE: f32 = JUMP_SPEED * 0.5;

/// How far below the feet a surface may sit and still count as the ground
/// while walking down a slope. A descending character leaves the surface
/// faster than gravity can pull it back, so contact-only support flickers and
/// jump presses are silently dropped on most frames.
const STEP_DOWN: f32 = 0.35;
/// Surfaces steeper than this are walls, not ground.
const MIN_GROUND_NORMAL: f32 = 0.4;

/// Ground support from a contact gap, as `(grounded, drop_to_contact)`. A
/// negative gap means the capsule is touching or slightly penetrating; it is
/// never moved up, the physics resolves that. Positive gaps up to `STEP_DOWN`
/// are followed when already walking, so a descent does not free-fall.
fn ground_support(gap: f32, can_snap: bool, uphill: bool) -> (bool, Option<f32>) {
    if gap <= 0.0 {
        (gap >= -0.2, None)
    } else if gap <= 0.13 {
        (true, if uphill { None } else { Some(gap) })
    } else if can_snap && !uphill && gap <= STEP_DOWN {
        (true, Some(gap))
    } else {
        (false, None)
    }
}

/// How much to lower the capsule for a downward ray from the bottom sphere's
/// centre. A capsule touches a slope along the surface normal, not at its
/// lowest point, so a plain vertical chord overstates the gap on an ascent and
/// snapping by it would fight the climb.
fn contact_gap(ray_distance: f32, normal_y: f32) -> f32 {
    ray_distance - PILL_RADIUS / normal_y.max(0.01)
}

/// Advance the character along the drawn route at a constant walking pace.
/// The camera's orbit centre follows in third/top view; in first person the
/// look direction follows the tangent.
fn walk(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut touch: ResMut<TouchState>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    kit: Res<Kit>,
    player: Query<Entity, With<CharacterVisual>>,
    move_and_slide: MoveAndSlide,
) {
    let Some(mut current) = editor.walk else {
        return;
    };
    let Some(character) = editor.character else {
        editor.walk = None;
        return;
    };
    let was_grounded = character.grounded;
    let Some(entity) = player.iter().next() else {
        return;
    };

    let Some((total, look)) = editor.route.as_ref().map(|route| {
        let total = route.track.total();
        (total, route.track.at((current.distance + 0.7).min(total)).0)
    }) else {
        editor.walk = None;
        return;
    };

    let to_target = look - character.pos;
    if current.distance >= total - 0.12 && to_target.length() < 0.2 {
        prune_plan(&mut editor, current.distance);
        editor.walk = None;
        editor.note = "arrived";
        return;
    }

    // The jump button works while the route keeps steering, so a jump carries
    // forward along the path instead of cancelling it. Read it only after the
    // arrival check: a jump pressed as the route ends is left for the free
    // controller so the button still lands.
    let jump = keys.pressed(KeyCode::Space) || touch.jump;
    touch.jump = false;

    // Steering, not teleporting: the same capsule and move-and-slide as
    // first-person control, so ramps, gaps and gravity are resolved by physics.
    let direction = to_target.normalize_or_zero();
    let frame = time.delta_secs().clamp(0.0, 0.25);
    let steps = ((frame / (1.0 / 60.0)).ceil() as usize).clamp(1, 10);
    let step_dt = frame / steps as f32;
    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);
    let mut center = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);
    let mut vy = jump_velocity(character.vy, character.grounded, jump);
    let mut travelled = 0.0;

    for _ in 0..steps {
        vy -= GRAVITY * step_dt;
        let velocity = Vec3::new(direction.x * WALK_SPEED, vy, direction.y * WALK_SPEED);
        let output = move_and_slide.move_and_slide(
            &kit.character_collider,
            center.real(),
            rotation,
            velocity,
            std::time::Duration::from_secs_f32(step_dt),
            &MoveAndSlideConfig::default(),
            &filter,
            |_| MoveAndSlideHitResponse::Accept,
        );
        let next = output.position.f32();
        travelled += Vec2::new(next.x - center.x, next.z - center.z).length();
        center = next;
        vy = output.projected_velocity.y;
    }

    let moved = Vec2::new(center.x, center.z);
    let yaw = if direction.length_squared() > 0.01 {
        (-direction.x).atan2(-direction.y)
    } else {
        character.yaw
    };

    let nearest = editor
        .route
        .as_ref()
        .map(|route| route.track.nearest_distance(moved))
        .unwrap_or(current.distance);

    // Probe down from the capsule centre so a descent can follow the slope for
    // a short step instead of free-falling between contact frames.
    let mut feet = center.y - PILL_HALF;
    let surface = move_and_slide
        .spatial_query
        .cast_ray(
            Vec3::new(center.x, feet + PILL_RADIUS, center.z).real(),
            Dir3::NEG_Y,
            PILL_RADIUS / MIN_GROUND_NORMAL + STEP_DOWN,
            true,
            &filter,
        )
        .filter(|hit| hit.normal.y > MIN_GROUND_NORMAL)
        .map(|hit| {
            let uphill = Vec2::new(hit.normal.x, hit.normal.z).dot(direction) < -0.05;
            (contact_gap(hit.distance, hit.normal.y), uphill)
        });
    let (grounded, drop) = if vy > MAX_GROUNDED_RISE {
        (false, None)
    } else {
        surface.map_or((false, None), |(gap, uphill)| {
            ground_support(gap, was_grounded, uphill)
        })
    };
    if grounded {
        vy = 0.0;
    }
    if let Some(drop) = drop {
        feet -= drop;
    }

    if let Some(target) = editor.character.as_mut() {
        target.pos = moved;
        target.feet = feet;
        target.vy = if grounded { 0.0 } else { vy };
        target.grounded = grounded;
        target.yaw = yaw;
    }

    if orbit.mode == ViewMode::First {
        orbit.yaw = yaw;
    } else {
        orbit.target = Vec3::new(moved.x, 0.0, moved.y);
    }

    current.distance = nearest.clamp(current.distance - 0.2, total);

    // Reached waypoints stop being part of the live plan; the drawn line
    // stays behind as a record.
    prune_plan(&mut editor, current.distance);

    if travelled < 0.005 {
        current.stuck += frame;
    } else {
        current.stuck = 0.0;
    }

    if current.stuck > 0.8 {
        editor.walk = None;
        editor.note = "blocked";
    } else if current.distance >= total - 0.12 {
        editor.walk = None;
        editor.note = "arrived";
    } else {
        editor.walk = Some(current);
    }

    // Any manual steering hands control back to the player.
    if touch.stick.length_squared() > 0.02
        || keys.any_pressed([
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ])
    {
        editor.walk = None;
        editor.note = "manual control";
    }
}

fn rotate_piece(editor: &mut Editor, step: f32) {
    if let Some(id) = editor.selected {
        if editor.pieces.iter().any(|p| p.id == id) {
            editor.commit();
            if let Some(piece) = editor.pieces.iter_mut().find(|p| p.id == id) {
                piece.rot = model::normalize_angle(piece.rot + step);
            }
        }
    } else if editor.tool.is_some() {
        editor.ghost_rot = model::normalize_angle(editor.ghost_rot + step);
    }
}

fn delete_piece(editor: &mut Editor) {
    if let Some(id) = editor.selected {
        editor.commit();
        editor.pieces.retain(|p| p.id != id);
        editor.selected = None;
    }
}

/// Copy the selected piece, keeping its shape, rotation and height.
fn copy_piece(editor: &mut Editor) {
    let Some(piece) = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|piece| piece.id == id).copied())
    else {
        editor.note = "nothing to copy";
        return;
    };
    editor.clipboard = Some(piece);
    editor.note = "copied";
}

/// Paste the clipboard as a new piece at `at`, or one hex beside the source
/// when the pointer is not over the world. Rotation and height are preserved.
fn paste_piece(editor: &mut Editor, at: Option<Vec2>) {
    let Some(source) = editor.clipboard else {
        editor.note = "nothing to paste";
        return;
    };
    let Some(shape) = shapes().get(source.shape) else {
        return;
    };
    let origin = at.unwrap_or_else(|| {
        let (q, r) = model::pixel_to_hex(source.pos);
        model::hex_center(q + 1, r)
    });
    let raw = Piece {
        id: editor.next_id,
        pos: origin,
        ..source
    };
    let snap = model::snap_piece(
        &raw,
        shape,
        &editor.pieces,
        editor.snap,
        editor.snap,
        SNAP_THRESHOLD,
    );

    editor.commit();
    let id = editor.next_id;
    editor.next_id += 1;
    editor.pieces.push(Piece {
        id,
        pos: snap.pos,
        rot: snap.rot,
        ..raw
    });
    editor.selected = Some(id);
    editor.tool = None;
    editor.drag = None;
    editor.ghost = None;
    editor.note = "pasted";
}

/// Free movement resolved by Avian's move-and-slide against the real colliders:
/// gravity, wall sliding, step-up and jumping. WASD and sprint only apply in
/// first person; jumping and gravity apply in every view, including while no
/// route is steering.
fn character_move(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    kit: Res<Kit>,
    player: Query<Entity, With<CharacterVisual>>,
    move_and_slide: MoveAndSlide,
) {
    if editor.walk.is_some() {
        return;
    }
    let Some(character) = editor.character.as_mut() else {
        return;
    };
    let was_grounded = character.grounded;
    let Some(entity) = player.iter().next() else {
        return;
    };

    // The stick and WASD both steer, in every view. Third person is the
    // default on a phone, and the pad is the only movement control there.
    let mut wish = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        wish.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        wish.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        wish.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        wish.x -= 1.0;
    }
    if touch.enabled && touch.stick.length_squared() > 0.01 {
        wish = touch.stick;
    }
    // An analogue pad moves slower near the middle; a small dead zone stops a
    // resting thumb from drifting.
    let mut strength = wish.length().clamp(0.0, 1.0);
    if strength < 0.12 {
        strength = 0.0;
    }
    let wish = if strength > 0.0 {
        wish.normalize_or_zero()
    } else {
        Vec2::ZERO
    };

    let forward = Vec2::new(-orbit.yaw.sin(), -orbit.yaw.cos());
    let right = Vec2::new(orbit.yaw.cos(), -orbit.yaw.sin());
    let direction = (forward * wish.y + right * wish.x).normalize_or_zero();

    let sprint = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed = (if sprint { 5.5 } else { 2.8 }) * strength;

    // Split a long frame into short substeps. Under software rendering a frame
    // can be 0.3 s; a single step that long lets gravity cancel a jump and
    // tunnels through colliders.
    let frame = time.delta_secs().clamp(0.0, 0.25);
    let steps = ((frame / (1.0 / 60.0)).ceil() as usize).clamp(1, 10);
    let step_dt = frame / steps as f32;

    // `pressed`, not `just_pressed`: a frame can span the whole key tap on a
    // slow frame, and a held space should still jump on landing.
    let jump = keys.pressed(KeyCode::Space) || touch.jump;
    touch.jump = false;
    let mut vy = jump_velocity(character.vy, character.grounded, jump);

    let filter = SpatialQueryFilter::from_excluded_entities([entity]);
    let rotation = Quat::from_rotation_y(character.yaw);
    let mut center = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);

    // Standing still on a slope should not slide: without friction the
    // projected gravity would walk the character back down the ramp.
    let idle = wish == Vec2::ZERO && character.grounded && vy <= 0.01;

    for _ in 0..steps {
        if idle {
            vy = 0.0;
        } else {
            vy -= GRAVITY * step_dt;
        }
        let velocity = Vec3::new(direction.x * speed, vy, direction.y * speed);
        let output = move_and_slide.move_and_slide(
            &kit.character_collider,
            center.real(),
            rotation,
            velocity,
            std::time::Duration::from_secs_f32(step_dt),
            &MoveAndSlideConfig::default(),
            &filter,
            |_| MoveAndSlideHitResponse::Accept,
        );
        center = output.position.f32();
        vy = output.projected_velocity.y;
    }

    // Follow a descending slope for a short step instead of free-falling until
    // the next contact frame; a jump pressed between those frames was ignored.
    let mut feet = center.y - PILL_HALF;
    let surface = move_and_slide
        .spatial_query
        .cast_ray(
            Vec3::new(center.x, feet + PILL_RADIUS, center.z).real(),
            Dir3::NEG_Y,
            PILL_RADIUS / MIN_GROUND_NORMAL + STEP_DOWN,
            true,
            &filter,
        )
        .filter(|hit| hit.normal.y > MIN_GROUND_NORMAL)
        .map(|hit| {
            let uphill = Vec2::new(hit.normal.x, hit.normal.z).dot(direction) < -0.05;
            (contact_gap(hit.distance, hit.normal.y), uphill)
        });
    let (grounded, drop) = if vy > MAX_GROUNDED_RISE {
        // A jump must not be cancelled by the ground it is leaving.
        (false, None)
    } else {
        surface.map_or((false, None), |(gap, uphill)| {
            ground_support(gap, was_grounded, uphill)
        })
    };
    if grounded {
        vy = 0.0;
    }
    if let Some(drop) = drop {
        feet -= drop;
    }

    character.pos = Vec2::new(center.x, center.z);
    character.feet = feet;
    if orbit.mode == ViewMode::First {
        character.yaw = orbit.yaw;
    } else if direction != Vec2::ZERO {
        // Face the way the pad points. Third person follows the pill so it
        // stays in frame; top down keeps the camera put, so the pill visibly
        // moves across the map instead of the grid appearing to stand still.
        character.yaw = (-direction.x).atan2(-direction.y);
        if orbit.mode == ViewMode::Third {
            orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
        }
    }

    character.grounded = grounded;
    character.vy = if grounded { 0.0 } else { vy };
}

/// The test course: a starting pad, a ramp rising along +x, a 0.9 m block at
/// the same height, and a landing pad after the drop. The character starts on
/// the pad facing the ramp, so pressing V then W runs it end to end.
fn course() -> (Vec<Piece>, Character) {
    let floor = model::shape_index("floor").unwrap();
    let ramp = model::shape_index("ramp").unwrap();
    let block = model::shape_index("block").unwrap();

    let pieces = vec![
        Piece {
            id: 1,
            shape: floor,
            pos: model::hex_center(0, 0),
            rot: 0.0,
            elevation: 0.0,
        },
        // 270 deg makes the ramp rise along +x.
        Piece {
            id: 2,
            shape: ramp,
            pos: model::hex_center(1, 0),
            rot: std::f32::consts::PI * 1.5,
            elevation: 0.0,
        },
        Piece {
            id: 3,
            shape: block,
            pos: model::hex_center(2, 0),
            rot: 0.0,
            elevation: 0.0,
        },
        Piece {
            id: 4,
            shape: floor,
            pos: model::hex_center(3, 0),
            rot: 0.0,
            elevation: 0.0,
        },
    ];

    let character = Character {
        pos: model::hex_center(0, 0),
        feet: 0.0,
        // Forward along +x with this yaw convention.
        yaw: -std::f32::consts::FRAC_PI_2,
        vy: 0.0,
        grounded: true,
    };

    (pieces, character)
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// How long a web pointer-lock request may take before it counts as refused.
const LOCK_GRACE: f32 = 0.5;

/// Whether the pointer is over the 3D view rather than the desktop palette.
/// Play hides the palette, so the whole screen is viewport there.
fn over_viewport(editor: &Editor, touch: &TouchState, cursor: Option<Vec2>) -> bool {
    if touch.enabled {
        return false;
    }
    let Some(cursor) = cursor else {
        return false;
    };
    !object_editing_allowed(editor.mode) || cursor.x > PANEL_WIDTH
}

#[cfg(target_arch = "wasm32")]
fn pointer_is_locked() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|document| document.pointer_lock_element().is_some())
}

#[cfg(not(target_arch = "wasm32"))]
fn pointer_is_locked() -> bool {
    true
}

/// Grab the pointer on entering first person, let go on leaving it or when
/// the window loses focus, and let Tab toggle the grab without changing view.
#[allow(clippy::too_many_arguments)]
fn manage_capture(
    mut capture: ResMut<PointerCapture>,
    mut editor: ResMut<Editor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut focus: MessageReader<WindowFocused>,
    time: Res<Time>,
) {
    let was_grabbed = capture.grabbed;
    let focus_lost = focus.read().any(|event| !event.focused);
    let first_person =
        orbit.mode == ViewMode::First && editor.character.is_some() && !touch.enabled;
    let input = CaptureInput {
        mode: editor.mode,
        view: orbit.mode,
        first_person,
        over_viewport: over_viewport(&editor, &touch, window.cursor_position()),
        tab: keys.just_pressed(KeyCode::Tab),
        focus_lost,
    };
    let mut grabbed = next_capture(capture.grabbed, capture.last_mode, capture.last_view, input);

    // On the web the browser can drop the lock by itself (Esc) and can refuse
    // a request without telling winit. Believe the document, after a short
    // grace period for a fresh request to settle.
    if capture.applied {
        let now = time.elapsed_secs();
        if !grabbed || pointer_is_locked() {
            capture.unlocked_since = None;
        } else {
            let since = *capture.unlocked_since.get_or_insert(now);
            if now - since > LOCK_GRACE {
                grabbed = false;
                capture.unlocked_since = None;
            }
        }
    }

    if grabbed && !was_grabbed {
        // Looking owns the mouse: drop any object drag that was in flight and
        // the placement ghost, which is not meaningful without a cursor.
        editor.drag = None;
        editor.gizmo_drag = None;
        editor.ghost = None;
    }
    capture.grabbed = grabbed;
    capture.last_mode = editor.mode;
    capture.last_view = orbit.mode;

    if capture.applied != grabbed {
        cursor_options.grab_mode = if grabbed {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        cursor_options.visible = !grabbed;
        capture.applied = grabbed;
    }
}

#[allow(clippy::too_many_arguments)]
fn camera_control(
    mut orbit: ResMut<Orbit>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
    mut editor: ResMut<Editor>,
    gesture: Res<PathGesture>,
    mut touch: ResMut<TouchState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    capture: Res<PointerCapture>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    // Touch browsers also synthesize mouse motion for a finger drag. While a
    // finger is down, that synthesized motion must not also steer the camera:
    // dragging the stick should move the character only.
    let raw_delta: Vec2 = motion.read().map(|m| m.delta).sum();
    let touching = touch.enabled
        && (touch.stick_pointer.is_some() || !touch.tracks.is_empty() || gesture.0.dragging());
    let delta = if touching { Vec2::ZERO } else { raw_delta };
    // Browsers report wheel deltas in pixels (often +-100), while some
    // platforms report whole lines. Normalise the big values to lines.
    let raw_scroll: f32 = wheel.read().map(|w| w.y).sum();
    let scroll = if raw_scroll.abs() > 10.0 {
        raw_scroll / 120.0
    } else {
        raw_scroll
    };
    let over_panel = object_editing_allowed(editor.mode)
        && window.cursor_position().is_some_and(|p| p.x <= PANEL_WIDTH);

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let command = command_down(&keys);

    if keys.just_pressed(KeyCode::KeyV) && !command {
        cycle_view(&mut orbit, &editor);
    }
    if keys.just_pressed(KeyCode::KeyF) {
        focus(&mut orbit, &editor);
    }

    if orbit.mode == ViewMode::First && editor.character.is_some() {
        // First person looks only while the pointer is captured; Tab lets go
        // so the palette can be used without leaving the view.
        if capture.grabbed {
            orbit.yaw -= delta.x * 0.005;
            orbit.pitch = (orbit.pitch - delta.y * 0.005).clamp(-1.35, 1.35);
            if let Some(character) = editor.character.as_mut() {
                character.yaw = orbit.yaw;
            }
        }
    } else if !over_panel {
        if mouse.pressed(MouseButton::Right) && !shift {
            // In Build an orbit pivots around the selected piece, so
            // the thing being edited stays in the middle of the view.
            if object_editing_allowed(editor.mode)
                && let Some(center) = selection_center(&editor)
            {
                orbit.target = center;
            }
            orbit.yaw -= delta.x * 0.006;
            let (low, high) = if orbit.mode == ViewMode::Top {
                (0.7, 1.5)
            } else {
                (0.12, 1.5)
            };
            orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(low, high);
        }
        if mouse.pressed(MouseButton::Middle) || (mouse.pressed(MouseButton::Right) && shift) {
            let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
            let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
            let k = orbit.distance * 0.0018;
            orbit.target -= right * delta.x * k;
            orbit.target += forward * delta.y * k;
            orbit.target.y = 0.0;
        }
        if scroll != 0.0 {
            orbit.distance = (orbit.distance * (1.0 - scroll * 0.12)).clamp(1.5, 60.0);
        }
    }

    // Touch: one finger looks/orbits, two fingers pan and pinch. A path
    // handle drag owns the gesture, so the camera keeps still until release.
    if gesture.0.dragging() {
        touch.look = Vec2::ZERO;
        touch.pan = Vec2::ZERO;
        touch.zoom = 0.0;
    } else if touch.enabled {
        match orbit.mode {
            ViewMode::First if editor.character.is_some() => {
                orbit.yaw -= touch.look.x * 0.005;
                orbit.pitch = (orbit.pitch - touch.look.y * 0.005).clamp(-1.35, 1.35);
            }
            _ => {
                if touch.look != Vec2::ZERO
                    && object_editing_allowed(editor.mode)
                    && let Some(center) = selection_center(&editor)
                {
                    orbit.target = center;
                }
                orbit.yaw -= touch.look.x * 0.006;
                let (low, high) = if orbit.mode == ViewMode::Top {
                    (0.7, 1.5)
                } else {
                    (0.12, 1.5)
                };
                orbit.pitch = (orbit.pitch + touch.look.y * 0.005).clamp(low, high);
                if touch.pan != Vec2::ZERO {
                    let right = Vec3::new(orbit.yaw.cos(), 0.0, -orbit.yaw.sin());
                    let forward = Vec3::new(-orbit.yaw.sin(), 0.0, -orbit.yaw.cos());
                    let k = orbit.distance * 0.0018;
                    orbit.target -= right * touch.pan.x * k;
                    orbit.target += forward * touch.pan.y * k;
                    orbit.target.y = 0.0;
                }
                if touch.zoom != 0.0 {
                    orbit.distance = (orbit.distance * (1.0 - touch.zoom * 0.8)).clamp(1.5, 60.0);
                }
            }
        }
    }

    if orbit.mode == ViewMode::First {
        if let Some(character) = editor.character {
            let eye = Vec3::new(
                character.pos.x,
                character.feet + EYE_HEIGHT,
                character.pos.y,
            );
            let look = Vec3::new(
                -orbit.yaw.sin() * orbit.pitch.cos(),
                orbit.pitch.sin(),
                -orbit.yaw.cos() * orbit.pitch.cos(),
            );
            camera.translation = eye;
            camera.look_at(eye + look, Vec3::Y);
            return;
        }
    }

    let offset = Vec3::new(
        orbit.pitch.cos() * orbit.yaw.sin(),
        orbit.pitch.sin(),
        orbit.pitch.cos() * orbit.yaw.cos(),
    ) * orbit.distance;
    camera.translation = orbit.target + offset;
    camera.look_at(orbit.target, Vec3::Y);
}

/// The centre of the selected piece, for orbiting in Build.
fn selection_center(editor: &Editor) -> Option<Vec3> {
    let piece = editor
        .pieces
        .iter()
        .find(|piece| Some(piece.id) == editor.selected)?;
    let shape = shapes().get(piece.shape)?;
    Some(Vec3::new(
        piece.pos.x,
        piece.elevation + (shape.base + shape.top()) * 0.5,
        piece.pos.y,
    ))
}

/// Put the orbit centre back on the selection, or on the character's feet when
/// nothing is selected.
fn focus(orbit: &mut Orbit, editor: &Editor) {
    if let Some(center) = selection_center(editor) {
        orbit.target = center;
    } else {
        focus_character(orbit, editor.character);
    }
}

/// Put the orbit centre on the character's feet.
fn focus_character(orbit: &mut Orbit, character: Option<Character>) {
    if let Some(character) = character {
        orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
    }
}

/// Third -> First -> Top. Shared by the V key and the touch View button.
fn cycle_view(orbit: &mut Orbit, editor: &Editor) {
    orbit.mode = orbit.mode.next();
    match orbit.mode {
        ViewMode::Third => {
            orbit.pitch = 0.9;
            orbit.distance = 7.5;
        }
        ViewMode::Top => {
            orbit.pitch = 1.4;
            orbit.distance = 13.0;
        }
        ViewMode::First => {
            orbit.pitch = 0.0;
            if let (Some(character), Some(route)) = (editor.character, editor.route.as_ref()) {
                if let Some(target) = route.line.last().or_else(|| route.trail.last()) {
                    let towards = Vec2::new(target.x, target.z) - character.pos;
                    if towards.length_squared() > 0.01 {
                        orbit.yaw = (-towards.x).atan2(-towards.y);
                    }
                }
            } else if let Some(character) = editor.character {
                orbit.yaw = character.yaw;
            }
        }
    }
    if orbit.mode != ViewMode::First {
        focus(orbit, editor);
    }
}

fn update_cursor(
    mut cursor: ResMut<Cursor>,
    touch: Res<TouchState>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform)>,
) {
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    cursor.screen = touch.tap.or_else(|| window.cursor_position());
    cursor.tap = touch.tap.is_some();
    cursor.ray = cursor
        .screen
        .and_then(|p| camera.viewport_to_world(&global, p).ok());
    cursor.world = cursor.ray.and_then(|ray| {
        ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            .map(|p| Vec2::new(p.x, p.z))
    });
    if let Some(world) = cursor.world {
        cursor.last_world = Some(world);
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn on_palette_activate(
    event: On<Activate>,
    buttons: Query<&PaletteButton>,
    time: Res<Time>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // While the pointer is captured the mouse only looks; Tab must release it
    // before a button can be used.
    if capture.grabbed {
        return;
    }
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    let now = time.elapsed_secs_f64();
    if let Some((last, at)) = editor.last_palette {
        if last == button.0 && now - at < 0.25 {
            return;
        }
    }
    editor.last_palette = Some((button.0, now));
    // A click on an on-screen control is not a canvas click: the play bar
    // floats over the world in play mode.
    touch.suppress_until = touch.suppress_until.max(now + 0.08);
    // While playing, only the mode switch responds; every other palette button
    // is an authoring action and must not change the level.
    if !object_editing_allowed(editor.mode) && palette_is_authoring(button.0) {
        return;
    }
    match button.0 {
        PaletteAction::Build => set_mode(&mut editor, Mode::Build),
        PaletteAction::Play => set_mode(&mut editor, Mode::Play),
        PaletteAction::View => cycle_view(&mut orbit, &editor),
        PaletteAction::Shape(index) => {
            editor.tool = if editor.tool == Some(index) {
                None
            } else {
                Some(index)
            };
            editor.actor_tool = None;
        }
        PaletteAction::Character => {
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Place) {
                None
            } else {
                Some(ActorTool::Place)
            };
            editor.tool = None;
        }
        PaletteAction::Path => {
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Path) {
                None
            } else {
                Some(ActorTool::Path)
            };
            editor.tool = None;
        }
        PaletteAction::Plan => {
            let entering = editor.actor_tool != Some(ActorTool::Plan);
            editor.actor_tool = if entering {
                Some(ActorTool::Plan)
            } else {
                None
            };
            editor.tool = None;
            if entering {
                orbit.mode = ViewMode::Top;
                orbit.pitch = 1.4;
                orbit.distance = 13.0;
                focus_character(&mut orbit, editor.character);
            }
        }
        PaletteAction::Walk => {
            if editor.actor_tool == Some(ActorTool::Plan) {
                walk_from_plan(&mut editor, &nav, &spatial_query, &filter);
            }
        }
        PaletteAction::Smooth => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && smooth_plan(&mut editor, &nav, &spatial_query, &filter)
            {
                editor.note = "smoothed";
            }
        }
        PaletteAction::Course => {
            editor.load_course();
            focus_character(&mut orbit, editor.character);
        }
    }
    editor.selected = None;
    editor.ghost = None;
    editor.ghost_rot = 0.0;
}

/// Keep the walkable hex graph in step with the placed pieces. Blocks and ramps
/// therefore take effect as soon as they land, not on the next reload.
fn rebuild_nav(editor: Res<Editor>, mut nav: ResMut<Nav>) {
    if nav.pieces == editor.pieces {
        return;
    }
    let (edges, hexes) = path::blocked(&editor.pieces);
    nav.edges = edges;
    nav.hexes = hexes;
    nav.pieces = editor.pieces.clone();
    nav.revision += 1;
}

/// If a route is up and the navmesh changed, plan it again against the new
/// graph. Placing a block bends an existing path around it; placing a ramp can
/// open the route up onto a block.
fn refresh_route(
    mut editor: ResMut<Editor>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // Mid-drag the pieces have not settled yet; wait for the release.
    if editor.drag.is_some() || editor.gizmo_drag.is_some() {
        return;
    }
    if editor.seen_nav == nav.revision {
        return;
    }
    editor.seen_nav = nav.revision;
    if editor.plan.as_ref().is_none_or(|plan| plan.is_empty()) {
        return;
    }
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    let was_walking = editor.walk.is_some();
    let ok = rebuild_route(&mut editor, &nav, &spatial_query, &filter, was_walking);
    let (_, valid) = rebuild_walk_state(ok, was_walking);
    if let Some(route) = editor.route.as_mut() {
        route.valid = valid;
    }
    if !ok {
        editor.note = if was_walking {
            "layout broke the plan, still walking"
        } else {
            "plan no longer reachable"
        };
    }
}

/// Object editing — placing, selecting, gizmos, rotate/delete/undo — is
/// Build-only. Route editing (waypoints and curve handles) stays available in
/// Play so the authored path can be test-driven and reshaped.
fn object_editing_allowed(mode: Mode) -> bool {
    mode == Mode::Build
}

/// Switch modes. Play holsters the object tools so a stray click cannot place,
/// select or delete; the level, plan and any live walk survive the switch.
fn set_mode(editor: &mut Editor, mode: Mode) {
    if editor.mode == mode {
        return;
    }
    editor.mode = mode;
    match mode {
        Mode::Play => {
            editor.tool = None;
            editor.actor_tool = None;
            editor.selected = None;
            editor.ghost = None;
            editor.drag = None;
            editor.gizmo_drag = None;
            editor.note = if editor.character.is_some() {
                "playing: WASD / stick moves the capsule"
            } else {
                "playing: place a character to control one"
            };
        }
        Mode::Build => {
            editor.note = "build mode";
        }
    }
}

/// Every palette action except the mode switch and the view control changes
/// the level.
fn palette_is_authoring(action: PaletteAction) -> bool {
    !matches!(
        action,
        PaletteAction::Build | PaletteAction::Play | PaletteAction::View
    )
}

/// Camera, jump and the mode switch stay live in Play; the rest author.
fn touch_is_authoring(action: TouchAction) -> bool {
    !matches!(
        action,
        TouchAction::Play | TouchAction::View | TouchAction::Jump
    )
}

/// Ctrl on Windows/Linux, Command on macOS.
fn command_down(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
}

#[allow(clippy::too_many_arguments)]
fn interact(
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    cursor: Res<Cursor>,
    capture: Res<PointerCapture>,
    touch: Res<TouchState>,
    gesture: Res<PathGesture>,
    nav: Res<Nav>,
    camera: Single<(&Camera, &Transform)>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
) {
    let (camera, camera_transform) = *camera;
    let camera_global = GlobalTransform::from(*camera_transform);
    let surface_filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    // P flips between authoring and play. Toggle before the cursor check so it
    // works even when the pointer leaves the window.
    if keys.just_pressed(KeyCode::KeyP) {
        let mode = editor.mode.toggled();
        set_mode(&mut editor, mode);
        return;
    }
    let playing = !object_editing_allowed(editor.mode);
    if playing && keys.just_pressed(KeyCode::Escape) {
        set_mode(&mut editor, Mode::Build);
        return;
    }

    // Object copy/paste works from the keyboard even when the pointer is off
    // the grid. Paste falls back to one hex beside the source in that case.
    let command = command_down(&keys);
    if !playing && command && keys.just_pressed(KeyCode::KeyC) {
        copy_piece(&mut editor);
    }
    if !playing && command && keys.just_pressed(KeyCode::KeyV) && editor.actor_tool.is_none() {
        paste_piece(&mut editor, cursor.world);
    }

    let (Some(screen), Some(world)) = (cursor.screen, cursor.world) else {
        return;
    };

    if !playing && keys.just_pressed(KeyCode::Escape) {
        editor.tool = None;
        editor.actor_tool = None;
        editor.selected = None;
        editor.ghost = None;
        editor.drag = None;
        editor.plan = None;
        editor.route = None;
        editor.walk = None;
        editor.note = "Click a component, then the grid";
    }

    if keys.just_pressed(KeyCode::Enter) && editor.actor_tool == Some(ActorTool::Plan) {
        walk_from_plan(&mut editor, &nav, &spatial_query, &surface_filter);
    }

    if !playing && !command && keys.just_pressed(KeyCode::KeyC) {
        editor.tool = None;
        editor.actor_tool = Some(ActorTool::Place);
        editor.ghost = None;
        editor.note = "click the grid to place the character";
    }

    if !playing && keys.just_pressed(KeyCode::KeyR) {
        let step = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            -model::ANCHOR_STEP
        } else {
            model::ANCHOR_STEP
        };
        rotate_piece(&mut editor, step);
    }

    if !playing && (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace)) {
        let waypoint_selected = editor.actor_tool == Some(ActorTool::Plan)
            && editor
                .plan
                .as_ref()
                .is_some_and(|plan| plan.selected.is_some());
        if waypoint_selected {
            delete_waypoint(&mut editor, &nav, &spatial_query, &surface_filter);
        } else {
            delete_piece(&mut editor);
        }
    }

    if !playing
        && keys.just_pressed(KeyCode::KeyZ)
        && (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        editor.undo();
    }

    if !playing
        && keys.just_pressed(KeyCode::KeyS)
        && !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        save(&editor);
    }

    // Captured first-person look owns the mouse: clicks must not place,
    // select, drag or route until Tab releases the pointer.
    if capture.grabbed {
        return;
    }

    // Clicks on the palette are handled by the widget observers; ignore them
    // here. The palette is hidden in touch mode and in Play, so the whole
    // screen is canvas there.
    if !playing && !touch.enabled && screen.x <= PANEL_WIDTH {
        return;
    }

    // Mouse clicks work in both layers; raw touch taps arrive in `cursor.tap`.
    // A tap that armed a UI button is filtered by `tap_allowed`.
    let clicked = (cursor.tap || mouse.just_pressed(MouseButton::Left))
        && tap_allowed(time.elapsed_secs_f64(), &touch);

    // A handle drag owns the pointer; do not also place or select.
    if gesture.0.dragging() {
        return;
    }

    editor.ghost = None;

    // Gizmo handles take priority over selecting whatever is under them.
    if gizmo_takes_pointer(&editor, touch.tap.is_some())
        && let (Some(screen), Some(ray)) = (cursor.screen, cursor.ray)
        && mouse.just_pressed(MouseButton::Left)
        && let Some(handle) = gizmo_hit(&editor, camera, &camera_global, screen, ray)
        && let Some(start) = editor
            .selected
            .and_then(|id| editor.pieces.iter().find(|p| p.id == id).copied())
        && let Some(shape) = shapes().get(start.shape)
        && let Some(grab) = gizmo_drag_point(ray, handle, gizmo_origin(&start, shape))
    {
        editor.commit();
        editor.gizmo_drag = Some(GizmoDrag {
            id: start.id,
            handle,
            grab,
            start,
            last_raw: start.elevation,
            last: start.elevation,
        });
        editor.ghost = None;
    }

    if let Some(drag) = editor.gizmo_drag {
        if mouse.pressed(MouseButton::Left) {
            apply_gizmo_drag(&mut editor, drag, cursor.ray);
        }
        if mouse.just_released(MouseButton::Left) {
            editor.gizmo_drag = None;
        }
        return;
    }

    if let Some(tool) = editor.tool {
        let shape = &shapes()[tool];
        let raw = Piece {
            id: 0,
            shape: tool,
            pos: world,
            rot: editor.ghost_rot,
            elevation: 0.0,
        };
        let snap = model::snap_piece(
            &raw,
            shape,
            &editor.pieces,
            editor.snap,
            editor.snap,
            SNAP_THRESHOLD,
        );
        editor.ghost = Some(Ghost {
            pos: snap.pos,
            rot: snap.rot,
            kind: snap.kind,
        });

        if clicked {
            editor.commit();
            let id = editor.next_id;
            editor.next_id += 1;
            editor.pieces.push(Piece {
                id,
                shape: tool,
                pos: snap.pos,
                rot: snap.rot,
                elevation: 0.0,
            });
            editor.selected = Some(id);
            editor.note = match snap.kind {
                SnapKind::Anchor => "placed: anchor snap",
                SnapKind::Grid => "placed: grid snap",
                SnapKind::Free => "placed: free",
            };
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Place) {
        if clicked {
            let pos = model::nearest_hex(world);
            let character = Character {
                pos,
                feet: 0.0,
                yaw: orbit.yaw,
                vy: 0.0,
                grounded: true,
            };
            editor.character = Some(character);
            editor.route = None;
            editor.walk = None;
            focus_character(&mut orbit, Some(character));
            editor.note = "character placed";
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Path) {
        if clicked && orbit.mode != ViewMode::First {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            // A tap here is a waypoint, never a selection: drop any stale
            // highlight left over from placing the block.
            editor.selected = None;
            editor.drag = None;
        }
        return;
    }

    if editor.actor_tool == Some(ActorTool::Plan) {
        if orbit.mode != ViewMode::Top {
            if clicked {
                editor.note = "plan editing is top-down";
            }
            return;
        }
        if clicked {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = append_for(shift, touch.append);
            let start_walk = click_starts_walk(true, editor.walk.is_some());
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                start_walk,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        }
        return;
    }

    if clicked {
        let hit = cursor.ray.and_then(|ray| pick(&editor.pieces, ray));

        // Play routes the capsule with plain clicks, including clicks on a
        // block or ramp (their tops are the surface the user means). Build
        // only authorises a route through the Path/Plan tools above; a bare
        // click there is selection, so loading a course cannot start walking.
        if click_routes(playing, orbit.mode, editor.character.is_some()) {
            let (target, height) = surface_target(&cursor, &spatial_query, &surface_filter, world);
            let cell = resolve_cell(&editor.pieces, target, height);
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            let append = orbit.mode == ViewMode::Top && append_for(shift, touch.append);
            add_waypoint(
                &mut editor,
                target,
                cell,
                height,
                append,
                true,
                &nav,
                &spatial_query,
                &surface_filter,
            );
            editor.selected = None;
            editor.drag = None;
        } else if playing {
            // Play routes the capsule; it never selects or drags objects.
        } else if let Some(hit) = hit {
            editor.selected = Some(hit);
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == hit) {
                editor.drag = Some(Drag {
                    id: hit,
                    offset: piece.pos - world,
                    moved: false,
                });
            }
            editor.commit();
        } else {
            editor.selected = None;
            editor.drag = None;
        }
    }

    if !playing && mouse.pressed(MouseButton::Left) {
        if let Some(drag) = editor.drag {
            if let Some((index, piece)) = editor
                .pieces
                .iter()
                .enumerate()
                .find(|(_, p)| p.id == drag.id)
                .map(|(i, p)| (i, *p))
            {
                let shape = &shapes()[piece.shape];
                let raw = Piece {
                    pos: world + drag.offset,
                    ..piece
                };
                let snap = model::snap_piece(
                    &raw,
                    shape,
                    &editor.pieces,
                    editor.snap,
                    editor.snap,
                    SNAP_THRESHOLD,
                );
                let target = &mut editor.pieces[index];
                target.pos = snap.pos;
                target.rot = snap.rot;
                if let Some(d) = editor.drag.as_mut() {
                    d.moved = true;
                }
            }
        }
    }

    if !playing && mouse.just_released(MouseButton::Left) {
        if let Some(drag) = editor.drag {
            if !drag.moved {
                editor.undo.pop();
            }
            editor.drag = None;
        }
    }
}

/// Where the click actually landed on the world, not just on the y = 0 plane.
/// Without this, clicking the top of a block maps to the ground behind it.
fn surface_target(
    cursor: &Cursor,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    fallback: Vec2,
) -> (Vec2, f32) {
    cursor
        .ray
        .and_then(|ray| {
            spatial_query
                .cast_ray(ray.origin.real(), ray.direction, 80.0, true, filter)
                .map(|hit| ray.get_point(hit.distance))
        })
        .map(|point| (Vec2::new(point.x, point.z), point.y))
        .unwrap_or((fallback, 0.0))
}

/// Height of the walkable surface under `at`, ignoring walls' vertical faces.
fn surface_height(
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    at: Vec2,
) -> Option<f32> {
    let origin = Vec3::new(at.x, 8.0, at.y);
    spatial_query
        .cast_ray(origin.real(), Dir3::NEG_Y, 16.0, true, filter)
        .filter(|hit| hit.normal.y > 0.3)
        .map(|hit| origin.y - hit.distance)
}

/// Lift the flat track onto the geometry it crosses, so the drawn curve climbs
/// a ramp and sits on a block top instead of running along the floor below.
fn route_line(
    track: &path::Track,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    start_height: f32,
) -> Vec<Vec3> {
    let points = track.points();
    if points.is_empty() {
        return Vec::new();
    }

    let heights = lift_heights(
        points.iter().copied(),
        |point| surface_height(spatial_query, filter, point),
        start_height,
    );
    let smoothed = smooth_heights(&heights);

    points
        .iter()
        .zip(smoothed)
        .map(|(point, height)| Vec3::new(point.x, height + 0.07, point.y))
        .collect()
}

/// Turn surface samples along the curve into a height profile.
///
/// * A sample far above the previous one is ignored (a wall top beside the
///   curve, not a step the walker could climb).
/// * A missing sample keeps the previous height.
/// * A large drop is only accepted if the footprint around the point has no
///   support; otherwise it is the seam between a ramp and a block, which the
///   capsule bridges, so the drawn curve should too.
fn lift_heights(
    points: impl Iterator<Item = Vec2>,
    sample: impl Fn(Vec2) -> Option<f32>,
    start: f32,
) -> Vec<f32> {
    /// Half the capsule's width: any support this close counts as a seam.
    const FOOTPRINT: f32 = 0.25;
    const STEP_UP: f32 = 0.5;
    const DROP: f32 = 0.45;

    let mut heights = Vec::new();
    let mut previous = start;
    for point in points {
        let mut height = sample(point).unwrap_or(previous);
        if height > previous + STEP_UP {
            height = previous;
        } else if height < previous - DROP {
            let supported = [
                Vec2::new(FOOTPRINT, 0.0),
                Vec2::new(-FOOTPRINT, 0.0),
                Vec2::new(0.0, FOOTPRINT),
                Vec2::new(0.0, -FOOTPRINT),
            ]
            .into_iter()
            .any(|offset| {
                sample(point + offset).is_some_and(|near| (near - previous).abs() < 0.15)
            });
            if supported {
                height = previous;
            }
        }
        heights.push(height);
        previous = height;
    }
    heights
}

/// Three-tap smoothing so the curve ramps up instead of kinking at seams.
fn smooth_heights(heights: &[f32]) -> Vec<f32> {
    (0..heights.len())
        .map(|i| {
            let before = heights[i.saturating_sub(1)];
            let here = heights[i];
            let after = heights[(i + 1).min(heights.len().saturating_sub(1))];
            (before + here * 2.0 + after) * 0.25
        })
        .collect()
}

/// A plan-mode click while stopped only lays out the route; Path mode and a
/// moving cylinder start walking.
fn click_starts_walk(plan_mode: bool, walking: bool) -> bool {
    !plan_mode || walking
}

/// A plain click routes the capsule only in Play. In Build it selects, so
/// loading a course (or any stray click) never starts a walk. What the cursor
/// is over does not matter: Play never selects, and a click on a block or ramp
/// is a request to route onto it.
fn click_routes(playing: bool, view: ViewMode, has_character: bool) -> bool {
    playing && view != ViewMode::First && has_character
}

/// Shift on desktop and the sticky Add toggle on touch both append.
fn append_for(shift: bool, touch_add: bool) -> bool {
    shift || touch_add
}

/// The cell of the block/ramp under a point, or the rounded ground cell.
fn resolve_cell(pieces: &[Piece], at: Vec2, height: f32) -> path::Hex {
    path::cell_under(pieces, at, height).unwrap_or_else(|| model::pixel_to_hex(at))
}

/// The drawn line up to the sample nearest `point`.
fn line_prefix(track: &path::Track, line: &[Vec3], point: Vec2) -> Vec<Vec3> {
    if line.is_empty() {
        return Vec::new();
    }
    let index = track.nearest_index(point).min(line.len() - 1);
    line[..=index].to_vec()
}

/// Remove waypoints the walker has reached and trim the matching route data.
fn prune_plan(editor: &mut Editor, walked: f32) {
    let Editor { plan, route, .. } = editor;
    if let (Some(plan), Some(route)) = (plan, route) {
        let removed = plan.prune(&route.distances, walked);
        if removed > 0 {
            route.distances.drain(..removed);
            route.controls.drain(..removed);
        }
    }
}

/// Extend the frozen walked trail with the walked part of the old suffix.
/// The suffix line starts where the existing trail ends, so its first point is
/// skipped once the trail is non-empty.
fn walked_trail(
    trail: &[Vec3],
    old_track: &path::Track,
    old_line: &[Vec3],
    start: Vec2,
) -> Vec<Vec3> {
    let mut out = trail.to_vec();
    let walked = line_prefix(old_track, old_line, start);
    let skip = usize::from(!out.is_empty());
    out.extend(walked.into_iter().skip(skip));
    out
}

/// Where a live rebuild starts: the current track point, not the old start.
fn rebuild_start(live: bool, route: Option<&Route>, walk: Option<&Walk>, character: Vec2) -> Vec2 {
    if live {
        if let (Some(route), Some(walk)) = (route, walk) {
            return route.track.at(walk.distance).0;
        }
    }
    character
}

/// Keep the old plan unless the new one built successfully.
fn choose_plan(old: Option<Plan>, new: Option<Plan>, ok: bool) -> Option<Plan> {
    if ok { new } else { old }
}

/// A UI button press must not leak through as a canvas tap.
fn tap_allowed(now: f64, touch: &TouchState) -> bool {
    now >= touch.suppress_until && !touch.multi
}

fn within_radius(screen: Vec2, target: Vec2, radius: f32) -> bool {
    screen.distance(target) <= radius
}

/// Which waypoint is under a screen point, if any.
fn waypoint_hit(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    radius: f32,
) -> Option<u32> {
    let plan = editor.plan.as_ref()?;
    let mut best: Option<(f32, u32)> = None;
    for waypoint in &plan.waypoints {
        let at = Vec3::new(waypoint.pos.x, waypoint.height + 0.06, waypoint.pos.y);
        let Ok(target) = camera.world_to_viewport(camera_transform, at) else {
            continue;
        };
        if !within_radius(screen, target, radius) {
            continue;
        }
        let distance = screen.distance(target);
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, waypoint.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Which tangent control is under a screen point, if any.
fn tangent_hit(
    editor: &Editor,
    route: &Route,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    radius: f32,
) -> Option<(u32, TangentSide)> {
    let plan = editor.plan.as_ref()?;
    let mut best: Option<(f32, u32, TangentSide)> = None;
    for (waypoint, controls) in plan.waypoints.iter().zip(route.controls.iter()) {
        let height = waypoint.height + 0.06;
        let handles = [
            (controls.incoming, TangentSide::Incoming),
            (controls.outgoing, TangentSide::Outgoing),
        ];
        for (control, side) in handles {
            let Some(control) = control else {
                continue;
            };
            let at = Vec3::new(control.x, height, control.y);
            let Ok(target) = camera.world_to_viewport(camera_transform, at) else {
                continue;
            };
            if !within_radius(screen, target, radius) {
                continue;
            }
            let distance = screen.distance(target);
            if best.is_none_or(|(best_distance, _, _)| distance < best_distance) {
                best = Some((distance, waypoint.id, side));
            }
        }
    }
    best.map(|(_, waypoint, side)| (waypoint, side))
}

/// Which waypoint/tangent handle is under a screen point.
fn path_handle_at(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    touch_enabled: bool,
    orbit: &Orbit,
) -> Option<PathHandle> {
    // Play can still reshape the route handles; object editing is what Play
    // leaves behind.
    let editing = editor.actor_tool == Some(ActorTool::Plan) || editor.mode == Mode::Play;
    if !editing || orbit.mode != ViewMode::Top {
        return None;
    }
    let radius = if touch_enabled { 24.0 } else { 10.0 };
    if let Some(id) = waypoint_hit(editor, camera, camera_transform, screen, radius) {
        return Some(PathHandle::Waypoint(id));
    }
    let route = editor.route.as_ref()?;
    tangent_hit(editor, route, camera, camera_transform, screen, radius)
        .map(|(waypoint, side)| PathHandle::Tangent { waypoint, side })
}

/// Apply one pointer update to a handle. The tangent branch is added by the
/// curve task.
#[allow(clippy::too_many_arguments)]
fn apply_path_drag(
    editor: &mut Editor,
    drag: PathDrag,
    screen: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let Some(ray) = camera.viewport_to_world(camera_transform, screen).ok() else {
        return false;
    };

    match drag.handle {
        PathHandle::Waypoint(id) => {
            let hit = spatial_query
                .cast_ray(ray.origin.real(), ray.direction, 80.0, true, filter)
                .map(|hit| ray.get_point(hit.distance))
                .or_else(|| {
                    ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
                });
            let Some(point) = hit else {
                return false;
            };
            let pos = Vec2::new(point.x, point.z);
            let height = point.y;
            let cell = resolve_cell(&editor.pieces, pos, height);
            set_waypoint(editor, id, pos, cell, height, nav, spatial_query, filter)
        }
        PathHandle::Tangent { waypoint, side } => {
            let Some(point) =
                ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            else {
                return false;
            };
            set_tangent(
                editor,
                waypoint,
                side,
                Vec2::new(point.x, point.z),
                nav,
                spatial_query,
                filter,
            )
        }
    }
}

/// Move one tangent control (clamped against its adjacent anchor) and rebuild.
fn set_tangent(
    editor: &mut Editor,
    waypoint_id: u32,
    side: TangentSide,
    control: Vec2,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let character = editor.character;
    let live = old_walk.is_some();

    {
        let Some(plan) = editor.plan.as_mut() else {
            return false;
        };
        let Some(waypoint) = plan.get(waypoint_id).copied() else {
            return false;
        };
        let Some(index) = plan
            .waypoints
            .iter()
            .position(|candidate| candidate.id == waypoint_id)
        else {
            return false;
        };
        let adjacent = match side {
            TangentSide::Outgoing => plan
                .waypoints
                .get(index + 1)
                .map(|next| next.pos)
                .unwrap_or(waypoint.pos),
            TangentSide::Incoming => plan
                .waypoints
                .get(index.wrapping_sub(1))
                .map(|previous| previous.pos)
                .or_else(|| character.map(|character| character.pos))
                .unwrap_or(waypoint.pos),
        };
        let limit = waypoint.pos.distance(adjacent) * 2.0;
        let control = waypoint.pos + (control - waypoint.pos).clamp_length_max(limit);
        match side {
            TangentSide::Incoming => plan.waypoints[index].incoming = Some(control),
            TangentSide::Outgoing => plan.waypoints[index].outgoing = Some(control),
        }
    }

    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
        return false;
    }

    // Store the control exactly as the curve builder clamped it, so the
    // handle and the drawn curve cannot drift apart on the next rebuild.
    let Editor { plan, route, .. } = editor;
    if let (Some(plan), Some(route)) = (plan.as_mut(), route.as_ref()) {
        if let Some(index) = plan
            .waypoints
            .iter()
            .position(|waypoint| waypoint.id == waypoint_id)
        {
            if let Some(controls) = route.controls.get(index) {
                match side {
                    TangentSide::Incoming => {
                        plan.waypoints[index].incoming = controls.incoming;
                    }
                    TangentSide::Outgoing => {
                        plan.waypoints[index].outgoing = controls.outgoing;
                    }
                }
            }
        }
    }
    true
}

/// Move one waypoint and rebuild, restoring the old plan on failure.
#[allow(clippy::too_many_arguments)]
fn set_waypoint(
    editor: &mut Editor,
    id: u32,
    pos: Vec2,
    cell: path::Hex,
    height: f32,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();

    {
        let Some(plan) = editor.plan.as_mut() else {
            return false;
        };
        let Some(waypoint) = plan.get_mut(id) else {
            return false;
        };
        waypoint.pos = pos;
        waypoint.cell = cell;
        waypoint.height = height;
    }

    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Delete the selected waypoint. The last waypoint clears the plan entirely.
fn delete_waypoint(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;

    let Some(plan) = editor.plan.as_mut() else {
        return false;
    };
    if !plan.delete_selected() {
        return false;
    }
    if plan.is_empty() {
        editor.plan = None;
        editor.route = None;
        editor.walk = None;
        editor.note = "plan cleared";
        return true;
    }

    let live = old_walk.is_some();
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Walk is available only for a stopped, non-empty, reachable plan.
fn walk_enabled(plan: Option<&Plan>, route: Option<&Route>, walking: bool) -> bool {
    plan.is_some_and(|plan| !plan.is_empty()) && route.is_some_and(|route| route.valid) && !walking
}

/// A nav rebuild may keep or clear validity but may never start a walk.
fn rebuild_walk_state(valid: bool, was_walking: bool) -> (bool, bool) {
    (was_walking, valid)
}

/// Rebuild from the cylinder's current position, then start walking.
fn walk_from_plan(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    if !walk_enabled(
        editor.plan.as_ref(),
        editor.route.as_ref(),
        editor.walk.is_some(),
    ) {
        return false;
    }
    if !rebuild_route(editor, nav, spatial_query, filter, false) {
        return false;
    }
    editor.walk = Some(Walk {
        distance: 0.0,
        stuck: 0.0,
    });
    editor.note = "walking";
    true
}

/// Drop every manual tangent. Returns how many handles were cleared.
fn clear_tangents(plan: &mut Plan) -> usize {
    let mut cleared = 0;
    for waypoint in &mut plan.waypoints {
        if waypoint.incoming.take().is_some() {
            cleared += 1;
        }
        if waypoint.outgoing.take().is_some() {
            cleared += 1;
        }
    }
    cleared
}

/// Return the plan's curve handles to the automatic tangents and rebuild.
fn smooth_plan(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();
    let Some(plan) = editor.plan.as_mut() else {
        return false;
    };
    clear_tangents(plan);
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    if !ok {
        editor.plan = old_plan;
        editor.route = old_route;
        editor.walk = old_walk;
    }
    ok
}

/// Rebuild the route geometry from the current plan. `live` keeps the walked
/// line prefix and restarts the suffix at the current track point.
fn rebuild_route(
    editor: &mut Editor,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
    live: bool,
) -> bool {
    let Some(character) = editor.character else {
        editor.note = "place a character first (C)";
        return false;
    };
    let Some(plan) = editor.plan.as_ref() else {
        return false;
    };
    let waypoints = plan.waypoints.clone();
    let live = live && editor.route.is_some() && editor.walk.is_some();
    let start = rebuild_start(
        live,
        editor.route.as_ref(),
        editor.walk.as_ref(),
        character.pos,
    );
    let height = character.feet;
    let start_cell = resolve_cell(&editor.pieces, start, height);
    let Some(route_plan) = path::plan_route(start, start_cell, &waypoints, &nav.edges, &nav.hexes)
    else {
        editor.note = "no route to that point";
        return false;
    };

    let new_line = route_line(&route_plan.track, spatial_query, filter, height);
    let (trail, line) = if live {
        if let Some(old) = editor.route.as_ref() {
            (
                walked_trail(&old.trail, &old.track, &old.line, start),
                new_line,
            )
        } else {
            (Vec::new(), new_line)
        }
    } else {
        (Vec::new(), new_line)
    };

    editor.route = Some(Route {
        track: route_plan.track,
        trail,
        line,
        distances: route_plan.distances,
        controls: route_plan.controls,
        valid: true,
        hexes: route_plan.hexes,
    });
    if live {
        if let Some(walk) = editor.walk.as_mut() {
            walk.distance = 0.0;
        }
    }
    true
}

/// Add or replace a waypoint and rebuild. On failure the previous plan, route
/// and walk are restored unchanged.
#[allow(clippy::too_many_arguments)]
fn add_waypoint(
    editor: &mut Editor,
    target: Vec2,
    cell: path::Hex,
    height: f32,
    append: bool,
    start_walk: bool,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) -> bool {
    if editor.character.is_none() {
        editor.note = "place a character first (C)";
        return false;
    }

    let old_plan = editor.plan.clone();
    let old_route = editor.route.clone();
    let old_walk = editor.walk;
    let live = old_walk.is_some();

    let mut plan = old_plan.clone().unwrap_or_default();
    if append && !plan.is_empty() {
        plan.append(target, cell, height);
    } else {
        plan.replace(target, cell, height);
    }

    editor.plan = Some(plan.clone());
    let ok = rebuild_route(editor, nav, spatial_query, filter, live);
    editor.plan = choose_plan(old_plan, Some(plan), ok);
    if !ok {
        editor.route = old_route;
        editor.walk = old_walk;
        return false;
    }

    editor.walk = start_walk.then_some(Walk {
        distance: 0.0,
        stuck: 0.0,
    });
    editor.seen_nav = nav.revision;
    editor.note = if start_walk { "walking" } else { "route drawn" };
    true
}

/// Ray against each piece's local bounding box, expanded by the click pad.
/// A box test (rather than a footprint at one height) means a wall's whole
/// visible face is clickable, and tall props win over the floor under them.
fn pick(pieces: &[Piece], ray: Ray3d) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for piece in pieces {
        let shape = &shapes()[piece.shape];
        if let Some(distance) = ray_piece_distance(piece, shape, ray) {
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, piece.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

fn ray_piece_distance(piece: &Piece, shape: &model::Shape, ray: Ray3d) -> Option<f32> {
    // Into the piece's local frame: model `y` is world `z`, yaw is `-rot`.
    let (sin, cos) = piece.rot.sin_cos();
    let ox = ray.origin.x - piece.pos.x;
    let oz = ray.origin.z - piece.pos.y;
    let origin = Vec3::new(ox * cos + oz * sin, ray.origin.y, -ox * sin + oz * cos);
    let direction = *ray.direction;
    let dir = Vec3::new(
        direction.x * cos + direction.z * sin,
        direction.y,
        -direction.x * sin + direction.z * cos,
    );

    let points = model::shape_points(shape);
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for p in &points {
        low = low.min(*p);
        high = high.max(*p);
    }

    let pad = HIT_PAD;
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    slab(
        origin.x,
        dir.x,
        low.x - pad,
        high.x + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.z,
        dir.z,
        low.y - pad,
        high.y + pad,
        &mut near,
        &mut far,
    )?;
    slab(
        origin.y,
        dir.y,
        piece.elevation + shape.base - pad,
        piece.elevation + shape.top() + pad,
        &mut near,
        &mut far,
    )?;

    (far >= 0.0).then(|| near.max(0.0))
}

fn slab(origin: f32, dir: f32, low: f32, high: f32, near: &mut f32, far: &mut f32) -> Option<()> {
    if dir.abs() < 1e-6 {
        return (origin >= low && origin <= high).then_some(());
    }
    let mut a = (low - origin) / dir;
    let mut b = (high - origin) / dir;
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    *near = near.max(a);
    *far = far.min(b);
    (*near <= *far).then_some(())
}

// ---------------------------------------------------------------------------
// Visuals
// ---------------------------------------------------------------------------

fn spawn_piece(commands: &mut Commands, kit: &Kit, piece: &Piece, ghost: bool) -> Entity {
    let transform = Transform {
        translation: Vec3::new(piece.pos.x, piece.elevation, piece.pos.y),
        rotation: model::yaw_quat(piece.rot),
        ..default()
    };
    let mut root = commands.spawn((PieceVisual, transform));

    if !ghost {
        let shape = &shapes()[piece.shape];
        if let Some(collider) = piece_collider(shape) {
            root.insert((
                RigidBody::Static,
                collider,
                Position(transform.translation.real()),
                Rotation(transform.rotation),
            ));
        }
    }
    let root = root.id();

    for (mesh, slot) in &kit.parts[piece.shape] {
        let material = if ghost {
            kit.ghost.clone()
        } else if *slot == 1 {
            kit.glass.clone()
        } else {
            kit.mats[piece.shape].clone()
        };
        let child = commands
            .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material)))
            .id();
        commands.entity(root).add_child(child);
    }
    root
}

fn sync_visuals(
    mut commands: Commands,
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    kit: Res<Kit>,
    mut visuals: ResMut<Visuals>,
    mut transforms: Query<
        (&mut Transform, Option<&mut Position>, Option<&mut Rotation>),
        Or<(With<PieceVisual>, With<CharacterVisual>)>,
    >,
    mut visibilities: Query<&mut Visibility, With<CharacterVisual>>,
) {
    let ghost_wanted = editor.tool.is_some() && editor.ghost.is_some();

    visuals.map.retain(|id, entity| {
        let keep = if *id == 0 {
            ghost_wanted
        } else {
            editor.pieces.iter().any(|p| p.id == *id)
        };
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for piece in &editor.pieces {
        match visuals.map.get(&piece.id) {
            Some(&entity) => {
                let pos = Vec3::new(piece.pos.x, piece.elevation, piece.pos.y);
                let rot = model::yaw_quat(piece.rot);
                if let Ok((mut transform, position, rotation)) = transforms.get_mut(entity) {
                    transform.translation = pos;
                    transform.rotation = rot;
                    if let Some(mut position) = position {
                        position.0 = pos.real();
                    }
                    if let Some(mut rotation) = rotation {
                        rotation.0 = rot;
                    }
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, piece, false);
                visuals.map.insert(piece.id, entity);
            }
        }
    }

    if let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost) {
        let piece = Piece {
            id: 0,
            shape: tool,
            pos: ghost.pos,
            rot: ghost.rot,
            elevation: 0.0,
        };
        match visuals.map.get(&0) {
            Some(&entity) => {
                if let Ok((mut transform, _, _)) = transforms.get_mut(entity) {
                    transform.translation = Vec3::new(piece.pos.x, 0.0, piece.pos.y);
                    transform.rotation = model::yaw_quat(piece.rot);
                }
            }
            None => {
                let entity = spawn_piece(&mut commands, &kit, &piece, true);
                visuals.map.insert(0, entity);
            }
        }
    }

    // The character: one capsule, hidden while you are inside it.
    if let Some(character) = editor.character {
        let entity = match visuals.character {
            Some(entity) => entity,
            None => {
                let transform = Transform::from_xyz(
                    character.pos.x,
                    character.feet + PILL_HALF,
                    character.pos.y,
                );
                let entity = commands
                    .spawn((
                        CharacterVisual,
                        RigidBody::Kinematic,
                        kit.character_collider.clone(),
                        Mesh3d(kit.pill.clone()),
                        MeshMaterial3d(kit.pill_mat.clone()),
                        transform,
                        Position(transform.translation.real()),
                        Rotation(transform.rotation),
                    ))
                    .id();
                visuals.character = Some(entity);
                entity
            }
        };
        let pos = Vec3::new(character.pos.x, character.feet + PILL_HALF, character.pos.y);
        let rot = Quat::from_rotation_y(character.yaw);
        if let Ok((mut transform, position, rotation)) = transforms.get_mut(entity) {
            transform.translation = pos;
            transform.rotation = rot;
            if let Some(mut position) = position {
                position.0 = pos.real();
            }
            if let Some(mut rotation) = rotation {
                rotation.0 = rot;
            }
        }
        if let Ok(mut visibility) = visibilities.get_mut(entity) {
            *visibility = if orbit.mode == ViewMode::First {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
    } else if let Some(entity) = visuals.character.take() {
        commands.entity(entity).despawn();
    }
}

// ---------------------------------------------------------------------------
// Touch
// ---------------------------------------------------------------------------

/// Turn raw touches into a stick vector, look/pan/zoom deltas, and taps.
fn touch_input(
    touches: Res<Touches>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut state: ResMut<TouchState>,
) {
    state.look = Vec2::ZERO;
    state.pan = Vec2::ZERO;
    state.zoom = 0.0;
    state.tap = None;
    let now = time.elapsed_secs_f64();

    // Phones start in touch mode before the first tap; a narrow window on a
    // desktop gets the compact layout too, which is the right trade.
    if !state.enabled
        && (touches.any_just_pressed()
            || touches.iter().next().is_some()
            || window.width() * window.scale_factor() < 700.0)
    {
        state.enabled = true;
    }
    if !state.enabled {
        return;
    }

    let size = window.size();
    let s = state.ui_scale;
    let centre = stick_centre(size, s);

    // Presses are handled separately: on a slow frame the press and release can
    // arrive together and never appear in `iter()`.
    for touch in touches.iter_just_pressed() {
        let id = touch.id();
        let pos = touch.position() / window.scale_factor();
        // The stick sits inside the bottom-left control zone, so decide the
        // stick first: a press on the pad is a grip, not a camera gesture. The
        // grab radius is deliberately larger than the drawn pad so DPR and
        // UiScale rounding cannot put the thumb just outside it.
        let stick = grabs_stick(pos, centre, STICK_REACH * s * 1.7);
        let control = !stick && over_controls(pos, size, s);
        state.tracks.insert(
            id,
            TouchTrack {
                last: pos,
                moved: 0.0,
                time: now,
                stick,
                control,
            },
        );
    }

    // A second finger anywhere (canvas, stick or a control) makes this
    // gesture multi-touch, so its release can never be a placement tap.
    if state.tracks.len() > 1 {
        state.multi = true;
    }

    for touch in touches.iter() {
        if touches.just_pressed(touch.id()) {
            continue;
        }
        let id = touch.id();
        let pos = touch.position() / window.scale_factor();
        if let Some(track) = state.tracks.get_mut(&id) {
            let delta = pos - track.last;
            track.last = pos;
            track.moved += delta.length();
            if track.stick {
                if state.stick_pointer.is_none() {
                    state.stick = stick_offset(pos, centre, s);
                }
            } else if !track.control {
                state.look += delta;
            }
        }
    }

    for touch in touches.iter_just_released() {
        let id = touch.id();
        let pos = touch.position() / window.scale_factor();
        if let Some(track) = state.tracks.remove(&id) {
            if track.stick && state.stick_pointer.is_none() {
                state.stick = Vec2::ZERO;
            }
            // A short, still press away from every control is a canvas tap.
            // This comes from the raw touch stream, so it works over bare
            // ground where no pickable entity exists to generate a pointer
            // event.
            if !track.stick
                && !track.control
                && !state.multi
                && track.moved <= path_edit::TAP_MAX_MOVE
                && now - track.time <= TOUCH_TAP_MAX_TIME
                && tap_allowed(now, &state)
            {
                state.tap = Some(pos);
            }
        }
    }

    for touch in touches.iter_just_canceled() {
        if let Some(track) = state.tracks.remove(&touch.id()) {
            if track.stick && state.stick_pointer.is_none() {
                state.stick = Vec2::ZERO;
            }
        }
    }

    // A picked stick drives itself from pointer events in the UI space; raw
    // touch deltas must not also rotate the camera from the same finger.
    if state.stick_pointer.is_some() {
        state.look = Vec2::ZERO;
        state.pan = Vec2::ZERO;
        state.zoom = 0.0;
    }
    if touches.iter().next().is_none() && state.stick_pointer != Some(PointerId::Mouse) {
        // Release can be missed if the pointer leaves the canvas.
        state.stick_pointer = None;
        state.stick = Vec2::ZERO;
    }

    // CDP touch emulation (and some browsers) deliver taps as mouse clicks.
    // Route those through the same hit-test so one path serves both.
    let fingers: Vec<Vec2> = state
        .tracks
        .values()
        .filter(|track| !track.stick && !track.control)
        .map(|track| track.last)
        .collect();

    if fingers.len() >= 2 {
        let centroid = (fingers[0] + fingers[1]) * 0.5;
        let span = fingers[0].distance(fingers[1]);
        if let Some((last_centroid, last_span)) = state.two_finger {
            state.pan += centroid - last_centroid;
            if last_span > 1.0 {
                state.zoom += (span - last_span) / last_span;
            }
            // While pinching, do not also rotate the view.
            state.look = Vec2::ZERO;
        }
        state.two_finger = Some((centroid, span));
    } else {
        state.two_finger = None;
    }

    if state.tracks.is_empty() && touches.iter().next().is_none() {
        state.multi = false;
    }
}

/// A press inside the stick's reach grabs it, even though the pad also lies in
/// the bottom-left control zone.
fn grabs_stick(pos: Vec2, centre: Vec2, reach: f32) -> bool {
    pos.distance(centre) <= reach
}

/// Stick deflection: screen right is x, screen up is +y, clamped to 1.
#[allow(dead_code)] // exercised by tests; the hot path inlines it
fn stick_offset(pos: Vec2, centre: Vec2, scale: f32) -> Vec2 {
    let offset = (pos - centre) / (STICK_TRAVEL * scale);
    Vec2::new(offset.x, -offset.y).clamp_length_max(1.0)
}

/// Screen areas owned by touch controls (toolbar + carousel, stick, jump).
fn over_controls(pos: Vec2, size: Vec2, scale: f32) -> bool {
    pos.y <= 128.0 * scale
        || (pos.y >= size.y - 170.0 * scale && pos.x <= 190.0 * scale)
        || (pos.y >= size.y - 170.0 * scale && pos.x >= size.x - 120.0 * scale)
}

fn sync_input_mode(
    touch: Res<TouchState>,
    mut layers: Query<(&InputLayer, &mut Node), Without<ModeUi>>,
) {
    if !touch.is_changed() {
        return;
    }
    for (layer, mut node) in &mut layers {
        let visible = layer.desktop != touch.enabled;
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Show the build overlays in Build and the play overlays in Play, for the
/// input layer the player is actually using.
fn sync_mode_ui(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    mut nodes: Query<(&ModeUi, &mut Node)>,
) {
    for (ui, mut node) in &mut nodes {
        let show = layer_matches(*ui, editor.mode, touch.enabled);
        node.display = if show { Display::Flex } else { Display::None };
    }
}

fn sync_touch_path_row(
    editor: Res<Editor>,
    mut row: Single<&mut Node, With<TouchPathRow>>,
    // The row and its buttons both own `Node`; the filter makes the two
    // queries disjoint so Bevy's access checks accept them.
    mut buttons: Query<(&TouchButton, &mut Node), Without<TouchPathRow>>,
) {
    let plan = editor.actor_tool == Some(ActorTool::Plan);
    let path = editor.actor_tool == Some(ActorTool::Path);
    let show = plan || path;
    row.display = if show { Display::Flex } else { Display::None };
    for (button, mut node) in &mut buttons {
        let visible = match button.0 {
            TouchAction::Add => show,
            TouchAction::Walk | TouchAction::Smooth => plan,
            _ => continue,
        };
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn sync_knob(touch: Res<TouchState>, mut knob: Single<&mut Node, With<TouchKnob>>) {
    if !touch.is_changed() {
        return;
    }
    let centre = (STICK_BASE - STICK_KNOB) * 0.5;
    knob.left = px(centre + touch.stick.x * STICK_TRAVEL);
    knob.bottom = px(centre + touch.stick.y * STICK_TRAVEL);
}

fn sync_shape_label(editor: Res<Editor>, mut label: Single<&mut Text, With<ShapeLabel>>) {
    if !editor.is_changed() {
        return;
    }
    label.0 = shapes()[editor.mobile_shape].name.to_string();
}

fn update_touch_buttons(
    editor: Res<Editor>,
    touch: Res<TouchState>,
    mut buttons: Query<(&TouchButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = match button.0 {
            TouchAction::Play => false,
            TouchAction::Path => editor.actor_tool == Some(ActorTool::Path),
            TouchAction::Plan => editor.actor_tool == Some(ActorTool::Plan),
            TouchAction::Add => touch.append,
            TouchAction::Walk => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && walk_enabled(
                        editor.plan.as_ref(),
                        editor.route.as_ref(),
                        editor.walk.is_some(),
                    )
            }
            TouchAction::Smooth => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && editor.plan.as_ref().is_some_and(|plan| !plan.is_empty())
            }
            TouchAction::Place => editor.tool == Some(editor.mobile_shape),
            _ => false,
        };
        bg.0 = if on {
            BUTTON_ON
        } else {
            Color::srgba(0.06, 0.08, 0.11, 0.62)
        };
        *border = BorderColor::all(if on {
            BORDER_ON
        } else {
            Color::srgba(0.5, 0.7, 0.85, 0.25)
        });
    }
}

fn update_touch_status(
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    touch: Res<TouchState>,
    mut text: Single<&mut Text, With<TouchStatus>>,
) {
    if !touch.enabled {
        return;
    }
    let height = editor.character.map(|c| c.feet).unwrap_or(0.0);
    let tool = match editor.actor_tool {
        Some(ActorTool::Path) => "path",
        Some(ActorTool::Plan) => "plan",
        Some(ActorTool::Place) => "char",
        None => editor
            .tool
            .map(|shape| shapes()[shape].name)
            .unwrap_or("select"),
    };
    let plan = editor
        .plan
        .as_ref()
        .map(|plan| format!("  {}wp", plan.waypoints.len()))
        .unwrap_or_default();
    text.0 = format!(
        "{}  {}  y{:.2}  {}p  {}{}{}",
        editor.mode.label(),
        orbit.mode.label(),
        height,
        editor.pieces.len(),
        tool,
        plan,
        if touch.append { "  add" } else { "" }
    );
}

/// Which touch button (if any) covers a point, in CSS pixels. The UI is drawn
/// with a 1/DPR UiScale on phones, so on screen these are the design sizes.
#[allow(dead_code)] // superseded by UI activation; kept for the hit-test test
fn touch_action_at(pos: Vec2, css: Vec2) -> Option<TouchAction> {
    if (8.0..=42.0).contains(&pos.y) {
        let width = (css.x - 20.0) / 8.0;
        let index = ((pos.x - 8.0) / (width + 4.0)).floor().max(0.0) as usize;
        return [
            TouchAction::View,
            TouchAction::Rotate,
            TouchAction::Delete,
            TouchAction::Undo,
            TouchAction::Path,
            TouchAction::Plan,
            TouchAction::Course,
            TouchAction::Play,
        ]
        .get(index.min(7))
        .copied();
    }
    if (46.0..=84.0).contains(&pos.y) {
        let quarter = (css.x - 20.0) / 4.0;
        let index = ((pos.x - 8.0) / (quarter + 4.0)).floor().max(0.0) as usize;
        return match index.min(3) {
            0 => Some(TouchAction::PrevShape),
            1 => None,
            2 => Some(TouchAction::NextShape),
            _ => Some(TouchAction::Place),
        };
    }
    if (88.0..=122.0).contains(&pos.y) {
        let third = (css.x - 20.0) / 3.0;
        let index = ((pos.x - 8.0) / (third + 4.0)).floor().max(0.0) as usize;
        return [TouchAction::Add, TouchAction::Walk, TouchAction::Smooth]
            .get(index.min(2))
            .copied();
    }
    let jump = Vec2::new(css.x - 62.0, css.y - 66.0);
    if pos.distance(jump) < 52.0 {
        return Some(TouchAction::Jump);
    }
    None
}

fn run_touch_action(
    action: TouchAction,
    editor: &mut Editor,
    orbit: &mut Orbit,
    touch: &mut TouchState,
    nav: &Nav,
    spatial_query: &SpatialQuery,
    filter: &SpatialQueryFilter,
) {
    match action {
        TouchAction::Play => set_mode(editor, editor.mode.toggled()),
        TouchAction::View => cycle_view(orbit, editor),
        TouchAction::Rotate => rotate_piece(editor, model::ANCHOR_STEP),
        TouchAction::Delete => {
            let waypoint_selected = editor.actor_tool == Some(ActorTool::Plan)
                && editor
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.selected.is_some());
            if waypoint_selected {
                delete_waypoint(editor, nav, spatial_query, filter);
            } else {
                delete_piece(editor);
            }
        }
        TouchAction::Undo => editor.undo(),
        TouchAction::Path => {
            editor.tool = None;
            // Toggle, like the palette button, so a second tap leaves path
            // mode. The active state is shown on the button and in the status.
            editor.actor_tool = if editor.actor_tool == Some(ActorTool::Path) {
                None
            } else {
                Some(ActorTool::Path)
            };
            editor.selected = None;
            editor.drag = None;
        }
        TouchAction::Plan => {
            let entering = editor.actor_tool != Some(ActorTool::Plan);
            editor.actor_tool = if entering {
                Some(ActorTool::Plan)
            } else {
                None
            };
            editor.tool = None;
            editor.selected = None;
            editor.drag = None;
            editor.ghost = None;
            if entering {
                orbit.mode = ViewMode::Top;
                orbit.pitch = 1.4;
                orbit.distance = 13.0;
                focus_character(orbit, editor.character);
            }
        }
        TouchAction::Add => touch.append = !touch.append,
        TouchAction::Walk => {
            if editor.actor_tool == Some(ActorTool::Plan) {
                walk_from_plan(editor, nav, spatial_query, filter);
            }
        }
        TouchAction::Smooth => {
            if editor.actor_tool == Some(ActorTool::Plan)
                && smooth_plan(editor, nav, spatial_query, filter)
            {
                editor.note = "smoothed";
            }
        }
        TouchAction::Place => {
            editor.tool = if editor.tool == Some(editor.mobile_shape) {
                None
            } else {
                Some(editor.mobile_shape)
            };
            editor.actor_tool = None;
            editor.selected = None;
            editor.drag = None;
        }
        TouchAction::PrevShape | TouchAction::NextShape => {
            let count = shapes().len();
            let delta = if action == TouchAction::NextShape {
                1
            } else {
                count - 1
            };
            editor.mobile_shape = (editor.mobile_shape + delta) % count;
            if editor.tool.is_some() {
                editor.tool = Some(editor.mobile_shape);
            }
        }
        TouchAction::Jump => touch.jump = true,
        TouchAction::Course => {
            editor.load_course();
            focus_character(orbit, editor.character);
        }
    }
}

/// Centre of the stick pad in the picking/UI coordinate space.
fn stick_centre(size: Vec2, scale: f32) -> Vec2 {
    Vec2::new(
        (STICK_MARGIN + STICK_BASE * 0.5) * scale,
        size.y - (STICK_MARGIN + STICK_BASE * 0.5) * scale,
    )
}

fn on_canvas_press(
    event: On<PointerPress>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut touch: ResMut<TouchState>,
) {
    if !touch.enabled || event.button != PointerButton::Primary {
        return;
    }
    let pos = event.pointer.position;
    let centre = stick_centre(window.size(), touch.ui_scale);
    if grabs_stick(pos, centre, STICK_REACH * touch.ui_scale * 1.7) {
        touch.stick_pointer = Some(event.pointer.id);
        touch.stick = stick_offset(pos, centre, touch.ui_scale);
        touch.tap = None;
    }
}

fn on_canvas_move(
    event: On<PointerMove>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut touch: ResMut<TouchState>,
) {
    if touch.stick_pointer != Some(event.pointer.id) {
        return;
    }
    let centre = stick_centre(window.size(), touch.ui_scale);
    touch.stick = stick_offset(event.pointer.position, centre, touch.ui_scale);
}

fn on_canvas_release(event: On<PointerRelease>, mut touch: ResMut<TouchState>) {
    if touch.stick_pointer == Some(event.pointer.id) {
        touch.stick_pointer = None;
        touch.stick = Vec2::ZERO;
    }
}

#[allow(clippy::too_many_arguments)]
fn on_path_press(
    event: On<PointerPress>,
    window: Single<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    touches: Res<Touches>,
    camera: Single<(&Camera, &Transform)>,
    orbit: Res<Orbit>,
    capture: Res<PointerCapture>,
    mut editor: ResMut<Editor>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    if event.button != PointerButton::Primary || capture.grabbed {
        return;
    }
    if touches.iter().count() > 1 {
        touch.multi = true;
    }
    let pos = event.pointer.position;
    if touch.enabled {
        let size = window.size();
        let centre = stick_centre(size, touch.ui_scale);
        if grabs_stick(pos, centre, STICK_REACH * touch.ui_scale * 1.7)
            || over_controls(pos, size, touch.ui_scale)
        {
            return;
        }
    }

    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    let handle = path_handle_at(&editor, camera, &global, pos, touch.enabled, &orbit);
    let now = time.elapsed_secs_f64();
    let drag = gesture.0.press(event.pointer.id, pos, now, handle);
    if let Some(drag) = drag {
        let waypoint = match drag.handle {
            PathHandle::Waypoint(id) => id,
            PathHandle::Tangent { waypoint, .. } => waypoint,
        };
        if let Some(plan) = editor.plan.as_mut() {
            plan.select(Some(waypoint));
        }
        touch.path_drag_active = true;
        touch.suppress_until = now + 0.08;
        touch.tap = None;
    }
}

fn on_path_move(event: On<PointerMove>, mut gesture: ResMut<PathGesture>) {
    // Tap candidates track movement here; an active handle drag uses
    // PointerDrag so it keeps receiving updates over UI nodes.
    if !gesture.0.dragging() {
        gesture
            .0
            .drag(event.pointer.id, event.pointer.position, event.delta);
    }
}

fn on_path_drag(event: On<PointerDrag>, mut gesture: ResMut<PathGesture>) {
    gesture
        .0
        .drag(event.pointer.id, event.pointer.position, event.delta);
}

fn on_path_release(
    event: On<PointerRelease>,
    time: Res<Time>,
    touches: Res<Touches>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    if touches.iter().count() > 1 {
        touch.multi = true;
    }
    let now = time.elapsed_secs_f64();
    gesture.0.release(event.pointer.id, now);
    touch.path_drag_active = gesture.0.dragging();
}

fn on_path_drag_end(
    event: On<PointerDragEnd>,
    time: Res<Time>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    gesture.0.release(event.pointer.id, time.elapsed_secs_f64());
    touch.path_drag_active = gesture.0.dragging();
}

fn on_path_cancel(
    event: On<PointerCancel>,
    mut gesture: ResMut<PathGesture>,
    mut touch: ResMut<TouchState>,
) {
    gesture.0.cancel(event.pointer.id);
    touch.path_drag_active = gesture.0.dragging();
}

fn path_drag(
    mut editor: ResMut<Editor>,
    gesture: Res<PathGesture>,
    capture: Res<PointerCapture>,
    camera: Single<(&Camera, &Transform)>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    // Looking owns the mouse; a drag that was already in flight finishes when
    // the button is released.
    if capture.grabbed {
        return;
    }
    // Play never edits objects, even if a handle drag was in flight when the
    // mode switched. The gesture still releases normally.
    let Some(drag) = gesture.0.drag_target() else {
        return;
    };
    let Some(screen) = gesture.0.screen() else {
        return;
    };
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    apply_path_drag(
        &mut editor,
        drag,
        screen,
        camera,
        &global,
        &nav,
        &spatial_query,
        &filter,
    );
}

#[allow(clippy::too_many_arguments)]
fn on_touch_button(
    event: On<Activate>,
    buttons: Query<&TouchButton>,
    time: Res<Time>,
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    mut touch: ResMut<TouchState>,
    nav: Res<Nav>,
    spatial_query: SpatialQuery,
    player: Query<Entity, With<CharacterVisual>>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    let filter = player
        .iter()
        .next()
        .map(|entity| SpatialQueryFilter::from_excluded_entities([entity]))
        .unwrap_or_default();
    let now = time.elapsed_secs_f64();
    // A touch often synthesizes a mouse click too, which would fire this
    // observer twice and toggle a tool straight back off.
    if let Some((last, at)) = touch.last_action {
        if last == button.0 && now - at < 0.25 {
            return;
        }
    }
    touch.last_action = Some((button.0, now));
    // A UI tap is not a grid tap: drop any tap from this gesture.
    touch.suppress_until = now + 0.08;
    touch.tap = None;
    if !object_editing_allowed(editor.mode) && touch_is_authoring(button.0) {
        return;
    }
    run_touch_action(
        button.0,
        &mut editor,
        &mut orbit,
        &mut touch,
        &nav,
        &spatial_query,
        &filter,
    );
}

// ---------------------------------------------------------------------------
// Palette and status
// ---------------------------------------------------------------------------

fn update_palette(
    editor: Res<Editor>,
    mut buttons: Query<(&PaletteButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = match button.0 {
            PaletteAction::Build | PaletteAction::Play | PaletteAction::View => false,
            PaletteAction::Shape(index) => editor.tool == Some(index),
            PaletteAction::Character => editor.actor_tool == Some(ActorTool::Place),
            PaletteAction::Path => editor.actor_tool == Some(ActorTool::Path),
            PaletteAction::Plan => editor.actor_tool == Some(ActorTool::Plan),
            PaletteAction::Walk => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && walk_enabled(
                        editor.plan.as_ref(),
                        editor.route.as_ref(),
                        editor.walk.is_some(),
                    )
            }
            PaletteAction::Smooth => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && editor.plan.as_ref().is_some_and(|plan| !plan.is_empty())
            }
            PaletteAction::Course => false,
        };
        bg.0 = if on { BUTTON_ON } else { BUTTON_BG };
        *border = BorderColor::all(if on { BORDER_ON } else { BORDER });
    }
}

fn sync_path_actions(editor: Res<Editor>, mut row: Single<&mut Node, With<PathActions>>) {
    let display = if editor.actor_tool == Some(ActorTool::Plan) {
        Display::Flex
    } else {
        Display::None
    };
    if row.display != display {
        row.display = display;
    }
}

fn update_status(
    editor: Res<Editor>,
    cursor: Res<Cursor>,
    orbit: Res<Orbit>,
    mut text: Single<&mut Text, With<StatusText>>,
) {
    let hex = if orbit.mode == ViewMode::First {
        editor.character.map(|c| {
            let (q, r) = model::pixel_to_hex(c.pos);
            format!(
                "hex {q},{r}  y {:.2} {}",
                c.feet,
                if c.grounded { "grounded" } else { "air" }
            )
        })
    } else {
        cursor.world.map(|p| {
            let (q, r) = model::pixel_to_hex(p);
            format!("hex {q},{r}")
        })
    }
    .unwrap_or_else(|| "-".to_string());

    let tool = editor.tool.map(|t| shapes()[t].name).unwrap_or("select");

    let tool = match editor.actor_tool {
        Some(ActorTool::Place) => "character",
        Some(ActorTool::Path) => "path",
        Some(ActorTool::Plan) => "plan",
        None => tool,
    };

    let plan = editor
        .plan
        .as_ref()
        .map(|plan| {
            format!(
                "   |   plan {}wp sel {} {}",
                plan.waypoints.len(),
                plan.selected
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                if editor.walk.is_some() {
                    "walking"
                } else {
                    "stopped"
                }
            )
        })
        .unwrap_or_default();

    let selection = editor
        .selected
        .and_then(|id| editor.pieces.iter().find(|p| p.id == id))
        .map(|p| shapes()[p.shape].name)
        .unwrap_or("none");

    let snap = editor
        .ghost
        .map(|g| match g.kind {
            SnapKind::Anchor => "anchor",
            SnapKind::Grid => "grid",
            SnapKind::Free => "free",
        })
        .unwrap_or("none");

    text.0 = format!(
        "{}   |   {} placed   |   mode: {}   |   tool: {}   |   view: {}{}{}   |   selected: {}   |   snap {}   |   {:.0} m   |   {}",
        hex,
        editor.pieces.len(),
        editor.mode.label(),
        tool,
        orbit.mode.label(),
        match editor.route.as_ref() {
            Some(route) if route.valid => format!(" (route {} hexes)", route.hexes),
            Some(_) => " (route invalid)".to_string(),
            None => String::new(),
        },
        plan,
        selection,
        snap,
        orbit.distance,
        editor.note,
    );
}

// ---------------------------------------------------------------------------
// Drawing (gizmo overlays only; the solids are entities)
// ---------------------------------------------------------------------------

#[allow(clippy::collapsible_if)]
fn draw(editor: Res<Editor>, cursor: Res<Cursor>, orbit: Res<Orbit>, mut gizmos: Gizmos) {
    let (cq, cr) = model::pixel_to_hex(Vec2::new(orbit.target.x, orbit.target.z));
    for dq in -8..=8 {
        for dr in -8..=8 {
            let center = model::hex_center(cq + dq, cr + dr);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|p| Vec3::new(p.x, GRID_Y, p.y))
                .collect();
            closed(&mut gizmos, &points, GRID);
        }
    }

    // In Play the cursor only means something when a click can route, so it
    // appears with a character in third/top view.
    let show_hover = orbit.mode != ViewMode::First
        && (object_editing_allowed(editor.mode) || editor.character.is_some());
    if show_hover {
        if let Some(p) = cursor.last_world {
            let center = model::nearest_hex(p);
            let points: Vec<Vec3> = model::hex_corners(center)
                .iter()
                .map(|q| Vec3::new(q.x, GRID_Y + 0.002, q.y))
                .collect();
            closed(&mut gizmos, &points, GRID_HOVER);

            // World cursor: a sphere on the snapped spot, a ring around it and
            // a short vertical spike so it reads in both top and first person.
            let spot = placement_spot(&editor, p);
            let at = Vec3::new(spot.0.x, 0.04, spot.0.y);
            let ring: Vec<Vec3> = (0..24)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / 24.0;
                    at + Vec3::new(a.cos() * 0.22, 0.0, a.sin() * 0.22)
                })
                .collect();
            closed(&mut gizmos, &ring, PATH);
            gizmos.line(at, at + Vec3::Y * 0.45, PATH);
            gizmos.sphere(Isometry3d::new(at, Quat::IDENTITY), 0.05, PATH);
        }
    }

    if object_editing_allowed(editor.mode) {
        if let Some(id) = editor.selected {
            if let Some(piece) = editor.pieces.iter().find(|p| p.id == id) {
                if let Some(shape) = shapes().get(piece.shape) {
                    selection_box(&mut gizmos, piece, shape);
                    anchors(&mut gizmos, piece, shape);
                    if orbit.mode != ViewMode::First {
                        transform_gizmo(&mut gizmos, piece, shape);
                    }
                }
            }
        }
    }

    if let (Some(tool), Some(ghost)) = (editor.tool, editor.ghost) {
        if let Some(shape) = shapes().get(tool) {
            let piece = Piece {
                id: 0,
                shape: tool,
                pos: ghost.pos,
                rot: ghost.rot,
                elevation: 0.0,
            };
            anchors(&mut gizmos, &piece, shape);
        }
    }

    if let Some(route) = &editor.route {
        let colour = if route.valid { PATH } else { AMBER };
        gizmos.linestrip(route.trail.iter().chain(route.line.iter()).copied(), colour);

        // A little target ring so the end of the curve is unambiguous, laid on
        // whatever surface the curve ends on.
        let target = route
            .line
            .last()
            .or_else(|| route.trail.last())
            .copied()
            .unwrap_or(Vec3::new(0.0, 0.07, 0.0));
        gizmos.circle(
            Isometry3d::new(target, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.22,
            colour,
        );
    }

    // Plan handles stay live in Play's top view so the Bezier curve can be
    // reshaped without going back to edit.
    if orbit.mode == ViewMode::Top
        && (editor.actor_tool == Some(ActorTool::Plan) || editor.mode == Mode::Play)
    {
        if let Some(plan) = &editor.plan {
            let controls = editor
                .route
                .as_ref()
                .map(|route| route.controls.as_slice())
                .unwrap_or(&[]);
            for (index, waypoint) in plan.waypoints.iter().enumerate() {
                let at = Vec3::new(waypoint.pos.x, waypoint.height + 0.06, waypoint.pos.y);
                let colour = if plan.selected == Some(waypoint.id) {
                    AMBER
                } else {
                    PATH
                };
                gizmos.circle(
                    Isometry3d::new(at, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    0.16,
                    colour,
                );
                gizmos.sphere(Isometry3d::new(at, Quat::IDENTITY), 0.06, colour);

                if let Some(controls) = controls.get(index) {
                    for control in [controls.incoming, controls.outgoing].into_iter().flatten() {
                        let handle = Vec3::new(control.x, waypoint.height + 0.06, control.y);
                        gizmos.line(at, handle, colour);
                        gizmos.sphere(Isometry3d::new(handle, Quat::IDENTITY), 0.05, colour);
                    }
                }
            }
        }
    }

    if let Some(character) = editor.character {
        let feet = Vec3::new(character.pos.x, character.feet + 0.05, character.pos.y);
        gizmos.circle(
            Isometry3d::new(feet, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            0.44,
            PILL_COLOR,
        );
    }
}

/// Where a click would put the thing you are holding: the ghost's snapped
/// point when a tool is armed, otherwise the nearest hex centre.
fn placement_spot(editor: &Editor, world: Vec2) -> (Vec2, f32) {
    if let Some(ghost) = editor.ghost {
        (ghost.pos, ghost.rot)
    } else {
        (model::nearest_hex(world), 0.0)
    }
}

/// The gizmo's handles: X, Y and Z arrows, a yaw ring, and one plane handle
/// per pair of axes on the selected piece.
fn transform_gizmo(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    for (dir, colour) in [(Vec3::X, GIZMO_X), (Vec3::Y, GIZMO_Y), (Vec3::Z, GIZMO_Z)] {
        let end = origin + dir * reach;
        gizmos.line(origin, end, colour);
        // Arrow heads as diamonds so they still read edge-on, scaled to the
        // handle. A vertical arrow uses X as its visible width.
        let back = end - dir * (reach * GIZMO_ARROW_BACK);
        let side = if dir == Vec3::Y {
            Vec3::X * (reach * GIZMO_ARROW_SIDE)
        } else {
            Vec3::new(dir.z, 0.0, dir.x) * (reach * GIZMO_ARROW_SIDE)
        };
        closed(gizmos, &[back + side, end, back - side], colour);
    }

    for (a, b, colour) in [
        (Vec3::X, Vec3::Y, GIZMO_Y),
        (Vec3::Z, Vec3::Y, GIZMO_Y),
        (Vec3::X, Vec3::Z, AMBER),
    ] {
        closed(gizmos, &gizmo_plane_corners(origin, a, b, reach), colour);
    }

    let ring_points: Vec<Vec3> = (0..32)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 32.0;
            origin + Vec3::new(a.cos() * ring, 0.0, a.sin() * ring)
        })
        .collect();
    closed(gizmos, &ring_points, AMBER);
}

/// Where the gizmo's axes meet: just above the piece's top, or at ground level
/// for sunken pieces such as a pit.
fn gizmo_origin(piece: &Piece, shape: &model::Shape) -> Vec3 {
    let base = shape.base.max(0.0) + 0.04;
    Vec3::new(piece.pos.x, piece.elevation + base, piece.pos.y)
}

/// The centre of one two-axis plane handle.
fn gizmo_plane_center(origin: Vec3, a: Vec3, b: Vec3, reach: f32) -> Vec3 {
    origin + (a + b) * (reach * GIZMO_PLANE_CENTER)
}

/// The four corners of a plane handle, in loop order.
fn gizmo_plane_corners(origin: Vec3, a: Vec3, b: Vec3, reach: f32) -> [Vec3; 4] {
    let centre = gizmo_plane_center(origin, a, b, reach);
    let half = reach * GIZMO_PLANE_SIZE;
    [
        centre - a * half - b * half,
        centre + a * half - b * half,
        centre + a * half + b * half,
        centre - a * half + b * half,
    ]
}

/// World points that identify each draggable handle. Axis handles use their
/// tip; plane handles use their centre.
fn gizmo_handle_points(piece: &Piece, shape: &model::Shape) -> Vec<(GizmoHandle, Vec3)> {
    let origin = gizmo_origin(piece, shape);
    let (reach, _) = gizmo_frame(piece, shape);
    [
        (GizmoHandle::X, Vec3::X),
        (GizmoHandle::Y, Vec3::Y),
        (GizmoHandle::Z, Vec3::Z),
    ]
    .into_iter()
    .map(|(handle, dir)| (handle, origin + dir * reach))
    .chain(
        [
            (GizmoHandle::Xy, Vec3::X, Vec3::Y),
            (GizmoHandle::Zy, Vec3::Z, Vec3::Y),
            (GizmoHandle::Xz, Vec3::X, Vec3::Z),
        ]
        .into_iter()
        .map(|(handle, a, b)| (handle, gizmo_plane_center(origin, a, b, reach))),
    )
    .collect()
}

/// Gizmo geometry for a piece: how far an X or Z handle reaches from the piece's
/// origin and how wide the yaw ring is. Both are sized against the piece's
/// bounding box, so the gizmo stays bigger than the selection it belongs to.
fn gizmo_frame(piece: &Piece, shape: &model::Shape) -> (f32, f32) {
    let size = bounding_size(piece, shape);
    (size * GIZMO_GROWTH, size * GIZMO_RING_SCALE)
}

/// The piece's bounding box as the largest of its ground-plane extent and its
/// height, in metres.
fn bounding_size(piece: &Piece, shape: &model::Shape) -> f32 {
    let (low, high) = piece_bounds(piece, shape);
    let ground = Vec2::new(high.x - low.x, high.y - low.y);
    ground.x.max(ground.y).max(shape.top() - shape.base)
}

/// World-space bounding box of a piece: the low and high corners of its
/// rotated ground outline, in `(x, z)`.
fn piece_bounds(piece: &Piece, shape: &model::Shape) -> (Vec2, Vec2) {
    let points = model::shape_points(shape);
    let (sin, cos) = piece.rot.sin_cos();
    let at = |p: Vec2| {
        Vec2::new(
            piece.pos.x + p.x * cos - p.y * sin,
            piece.pos.y + p.x * sin + p.y * cos,
        )
    };
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for point in &points {
        let p = at(*point);
        low.x = low.x.min(p.x);
        low.y = low.y.min(p.y);
        high.x = high.x.max(p.x);
        high.y = high.y.max(p.y);
    }
    (low, high)
}

/// Closest point on the line `origin + s * axis` to the ray, as `s`.
fn ray_axis_param(ray: Ray3d, origin: Vec3, axis: Vec3) -> Option<f32> {
    let dir = *ray.direction;
    let w0 = origin - ray.origin;
    let b = axis.dot(dir);
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-5 {
        return None;
    }
    Some((b * dir.dot(w0) - axis.dot(w0)) / denom)
}

/// Distance from a point to a screen-space segment.
fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len = ab.length_squared();
    if len < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Select/shape-place modes let a gizmo handle take the pointer. Character,
/// path and plan modes own the pointer for their own actions; touch taps are
/// handled by the path gesture layer.
fn gizmo_takes_pointer(editor: &Editor, touch_tap: bool) -> bool {
    object_editing_allowed(editor.mode) && editor.actor_tool.is_none() && !touch_tap
}

/// Which gizmo handle is under the pointer, if any.
fn gizmo_hit(
    editor: &Editor,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    screen: Vec2,
    ray: Ray3d,
) -> Option<GizmoHandle> {
    let id = editor.selected?;
    let piece = editor.pieces.iter().find(|p| p.id == id)?;
    let shape = shapes().get(piece.shape)?;
    let origin = gizmo_origin(piece, shape);
    let (reach, ring) = gizmo_frame(piece, shape);

    let project = |point: Vec3| camera.world_to_viewport(camera_transform, point).ok();

    // Plane handles first: their squares are small and unambiguous, and a
    // click inside one must not be stolen by an axis line crossing behind it.
    for (handle, a, b) in [
        (GizmoHandle::Xy, Vec3::X, Vec3::Y),
        (GizmoHandle::Zy, Vec3::Z, Vec3::Y),
        (GizmoHandle::Xz, Vec3::X, Vec3::Z),
    ] {
        let corners = gizmo_plane_corners(origin, a, b, reach);
        let Some(quad) = corners
            .iter()
            .map(|point| project(*point))
            .collect::<Option<Vec<Vec2>>>()
            .and_then(|points| <[Vec2; 4]>::try_from(points).ok())
        else {
            continue;
        };
        if plane_handle_hit(screen, &quad) {
            return Some(handle);
        }
    }

    let mut best: Option<(f32, GizmoHandle)> = None;
    if let Ok(origin_screen) = camera.world_to_viewport(camera_transform, origin) {
        for (handle, point) in gizmo_handle_points(piece, shape) {
            if !matches!(handle, GizmoHandle::X | GizmoHandle::Y | GizmoHandle::Z) {
                continue;
            }
            if let Some(end_screen) = project(point) {
                let distance = point_segment_distance(screen, origin_screen, end_screen);
                if distance < GIZMO_HANDLE_HIT && best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, handle));
                }
            }
        }
    }

    if best.is_none() {
        if let Some(hit) = ray.plane_intersection_point(origin, InfinitePlane3d::new(Vec3::Y)) {
            let radius = Vec2::new(hit.x - origin.x, hit.z - origin.z).length();
            if (radius - ring).abs() < ring * GIZMO_YAW_HIT {
                best = Some((0.0, GizmoHandle::Yaw));
            }
        }
    }

    best.map(|(_, handle)| handle)
}

/// The point on the drag line or plane that the pointer ray meets. Quantities
/// are measured from the grab position, so the piece follows the pointer's
/// travel rather than jumping to where it points.
fn gizmo_drag_point(ray: Ray3d, handle: GizmoHandle, origin: Vec3) -> Option<Vec3> {
    match handle {
        GizmoHandle::X => ray_axis_point(ray, origin, Vec3::X),
        GizmoHandle::Z => ray_axis_point(ray, origin, Vec3::Z),
        GizmoHandle::Y => vertical_drag_point(ray, origin),
        GizmoHandle::Xy => plane_ray_point(ray, origin, Vec3::Z),
        GizmoHandle::Zy => plane_ray_point(ray, origin, Vec3::X),
        GizmoHandle::Xz | GizmoHandle::Yaw => plane_ray_point(ray, origin, Vec3::Y),
    }
}

fn plane_ray_point(ray: Ray3d, origin: Vec3, normal: Vec3) -> Option<Vec3> {
    ray.plane_intersection_point(origin, InfinitePlane3d::new(normal))
}

fn ray_axis_point(ray: Ray3d, origin: Vec3, axis: Vec3) -> Option<Vec3> {
    ray_axis_param(ray, origin, axis).map(|s| origin + axis * s)
}

/// Vertical drags use a camera-facing plane through the axis, which stays
/// stable in a three-quarter view. Straight overhead there is no such plane,
/// so fall back to the closest point on the axis.
fn vertical_drag_point(ray: Ray3d, origin: Vec3) -> Option<Vec3> {
    let flat = Vec3::new(ray.origin.x - origin.x, 0.0, ray.origin.z - origin.z);
    if flat.length_squared() > 1e-4 {
        let normal = flat.normalize();
        if let Some(hit) = plane_ray_point(ray, origin, normal) {
            return Some(Vec3::new(origin.x, hit.y, origin.z));
        }
    }
    ray_axis_point(ray, origin, Vec3::Y)
}

/// The piece state after dragging from `grab` to `current`. Only the axes the
/// handle owns change; every other component is kept from the grab snapshot.
fn dragged_piece(start: Piece, handle: GizmoHandle, grab: Vec3, current: Vec3) -> Piece {
    let delta = current - grab;
    let mut piece = start;
    match handle {
        GizmoHandle::X => piece.pos.x += delta.x,
        GizmoHandle::Z => piece.pos.y += delta.z,
        GizmoHandle::Y => piece.elevation += delta.y,
        GizmoHandle::Xy => {
            piece.pos.x += delta.x;
            piece.elevation += delta.y;
        }
        GizmoHandle::Zy => {
            piece.pos.y += delta.z;
            piece.elevation += delta.y;
        }
        GizmoHandle::Xz => {
            piece.pos.x += delta.x;
            piece.pos.y += delta.z;
        }
        GizmoHandle::Yaw => {
            let grabbed = angle_around(start.pos, grab);
            let now = angle_around(start.pos, current);
            if let (Some(grabbed), Some(now)) = (grabbed, now) {
                // Local +x points along `rot` in the ground plane.
                piece.rot = model::normalize_angle(start.rot + now - grabbed);
            }
        }
    }
    piece
}

/// Yaw of `point` around `centre` on the ground plane, or `None` when the two
/// are too close to give a stable direction.
fn angle_around(centre: Vec2, point: Vec3) -> Option<f32> {
    let direction = Vec2::new(point.x - centre.x, point.z - centre.y);
    (direction.length() > 0.1).then(|| direction.y.atan2(direction.x))
}

/// Snap a dragged elevation onto the top of a piece below it, but only when
/// the drag is moving toward that surface and it is within reach. Away from a
/// surface the raw height wins, so the piece follows the pointer instead of
/// stepping over a grid. The moving piece itself is ignored.
fn snap_elevation(
    piece: Piece,
    shape: &model::Shape,
    others: &[Piece],
    raw: f32,
    direction: f32,
    snapping: bool,
    threshold: f32,
) -> f32 {
    if !snapping || direction == 0.0 {
        return raw;
    }

    let moving_up = direction > 0.0;
    let mut best: Option<(f32, f32)> = None;
    let mut consider = |elevation: f32| {
        // A surface behind the pointer must not capture the piece: that would
        // make it snap backwards while the user drags away from it.
        if (moving_up && elevation < raw) || (!moving_up && elevation > raw) {
            return;
        }
        let distance = (elevation - raw).abs();
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, elevation));
        }
    };

    // The ground is the natural zero, not `-base`; every piece starts there.
    consider(0.0);
    for other in others {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        let (low, high) = piece_bounds(other, other_shape);
        if piece.pos.x < low.x - HIT_PAD
            || piece.pos.x > high.x + HIT_PAD
            || piece.pos.y < low.y - HIT_PAD
            || piece.pos.y > high.y + HIT_PAD
        {
            continue;
        }
        consider(other.elevation + other_shape.top() - shape.base);
    }

    if let Some((distance, elevation)) = best
        && distance <= threshold
    {
        return elevation;
    }
    raw
}

fn apply_gizmo_drag(editor: &mut Editor, drag: GizmoDrag, ray: Option<Ray3d>) {
    let Some(ray) = ray else {
        return;
    };
    let Some(index) = editor.pieces.iter().position(|piece| piece.id == drag.id) else {
        return;
    };
    let shape = &shapes()[drag.start.shape];
    let origin = gizmo_origin(&drag.start, shape);
    let Some(current) = gizmo_drag_point(ray, drag.handle, origin) else {
        return;
    };
    let mut piece = dragged_piece(drag.start, drag.handle, drag.grab, current);
    let raw = piece.elevation;

    if matches!(
        drag.handle,
        GizmoHandle::Y | GizmoHandle::Xy | GizmoHandle::Zy
    ) {
        if (raw - drag.last_raw).abs() > 1e-6 {
            piece.elevation = snap_elevation(
                piece,
                shape,
                &editor.pieces,
                raw,
                raw - drag.last_raw,
                editor.snap,
                ELEVATION_SNAP,
            );
        } else {
            // Clicking the handle without moving must not shift the piece,
            // even if it happens to sit near a surface.
            piece.elevation = drag.last;
        }
    }
    editor.pieces[index] = piece;
    editor.gizmo_drag = Some(GizmoDrag {
        last_raw: raw,
        last: piece.elevation,
        ..drag
    });
}

/// A plane handle is only grabbable while it has a visible face. Seen edge-on,
/// its projection collapses to a sliver that must not steal an axis click.
fn plane_handle_hit(p: Vec2, quad: &[Vec2; 4]) -> bool {
    quad_area(quad) >= 24.0 && point_in_quad(p, quad)
}

fn quad_area(quad: &[Vec2; 4]) -> f32 {
    let mut twice = 0.0;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        twice += a.x * b.y - b.x * a.y;
    }
    twice.abs() * 0.5
}

/// Point-in-quad for the projected plane handles. The quad must be convex and
/// in loop order.
fn point_in_quad(p: Vec2, quad: &[Vec2; 4]) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[j];
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn closed(gizmos: &mut Gizmos, points: &[Vec3], colour: Color) {
    if points.len() < 2 {
        return;
    }
    gizmos.linestrip(
        points.iter().copied().chain(std::iter::once(points[0])),
        colour,
    );
}

fn selection_box(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let points = model::shape_points(shape);
    let (sin, cos) = piece.rot.sin_cos();
    let at = |p: Vec2, y: f32| {
        Vec3::new(
            piece.pos.x + p.x * cos - p.y * sin,
            y,
            piece.pos.y + p.x * sin + p.y * cos,
        )
    };
    let low = piece.elevation + shape.base - 0.01;
    let high = piece.elevation + shape.top() + 0.01;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        gizmos.line(at(a, low), at(b, low), AMBER);
        gizmos.line(at(a, high), at(b, high), AMBER);
        gizmos.line(at(a, low), at(a, high), AMBER);
    }
}

fn anchors(gizmos: &mut Gizmos, piece: &Piece, shape: &model::Shape) {
    let y = piece.elevation + shape.top() + 0.03;
    let size = 0.07;
    for anchor in model::world_anchors(piece, shape) {
        let c = Vec3::new(anchor.pos.x, y, anchor.pos.y);
        let corners = [
            c + Vec3::new(0.0, 0.0, size),
            c + Vec3::new(size, 0.0, 0.0),
            c + Vec3::new(0.0, 0.0, -size),
            c + Vec3::new(-size, 0.0, 0.0),
        ];
        closed(gizmos, &corners, anchor.kind.color());
        if anchor.kind == AnchorKind::End {
            let normal = Vec3::new(anchor.normal.x, 0.0, anchor.normal.y);
            gizmos.line(c, c + normal * 0.15, anchor.kind.color());
        }
    }
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

fn level_json(editor: &Editor) -> String {
    let pieces: Vec<String> = editor
        .pieces
        .iter()
        .map(|p| {
            format!(
                "{{\"shape\":\"{}\",\"x\":{:.3},\"z\":{:.3},\"y\":{:.3},\"rotDeg\":{:.1}}}",
                shapes()[p.shape].id,
                p.pos.x,
                p.pos.y,
                p.elevation,
                p.rot.to_degrees()
            )
        })
        .collect();
    let character = editor
        .character
        .map(|c| {
            format!(
                ",\"character\":{{\"x\":{:.3},\"z\":{:.3},\"yawDeg\":{:.1}}}",
                c.pos.x,
                c.pos.y,
                c.yaw.to_degrees()
            )
        })
        .unwrap_or_default();
    format!(
        "{{\"format\":\"party-dungeon-level\",\"version\":1,\"units\":\"metres\",\"hexSize\":{},\"pieces\":[{}]{character}}}",
        model::HEX_SIZE,
        pieces.join(",")
    )
}

fn save(editor: &Editor) {
    let json = level_json(editor);

    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = std::path::Path::new("level.json");
        match std::fs::write(path, &json) {
            Ok(()) => info!("saved {} pieces to level.json", editor.pieces.len()),
            Err(err) => warn!("could not save level.json: {err}"),
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        // No filesystem in the browser; the console is the sink for now.
        bevy::log::info!("{}", json);
        let _ = &json;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::Dir3;

    /// The default camera eye, looking wherever `target` is.
    fn ray_towards(target: Vec3) -> Ray3d {
        let eye = Vec3::new(3.5, 5.9, 3.1);
        Ray3d::new(eye, Dir3::new(target - eye).unwrap())
    }

    #[test]
    fn plan_and_path_modes_choose_walk_and_append_correctly() {
        assert!(!click_starts_walk(true, false));
        assert!(click_starts_walk(true, true));
        assert!(click_starts_walk(false, false));
        assert!(click_starts_walk(false, true));
        assert!(append_for(true, false));
        assert!(append_for(false, true));
        assert!(!append_for(false, false));
    }

    #[test]
    fn only_play_routes_on_a_click() {
        // Play in third/top view with a character: any click becomes a
        // waypoint, including one that lands on a block or ramp (Build would
        // select that prop instead).
        assert!(click_routes(true, ViewMode::Third, true));
        assert!(click_routes(true, ViewMode::Top, true));

        // Build must never start a walk from a click: that is selection.
        assert!(!click_routes(false, ViewMode::Third, true));
        assert!(!click_routes(false, ViewMode::Top, true));

        // First person or no character: no route.
        assert!(!click_routes(true, ViewMode::First, true));
        assert!(!click_routes(true, ViewMode::Third, false));
    }

    #[test]
    fn build_and_play_modes_gate_object_editing() {
        assert_eq!(Mode::default(), Mode::Build);
        assert_eq!(Mode::Build.toggled(), Mode::Play);
        assert_eq!(Mode::Play.toggled(), Mode::Build);
        assert!(object_editing_allowed(Mode::Build));
        assert!(!object_editing_allowed(Mode::Play));
    }

    fn capture_input(mode: Mode, view: ViewMode) -> CaptureInput {
        CaptureInput {
            mode,
            view,
            first_person: view == ViewMode::First,
            over_viewport: true,
            tab: false,
            focus_lost: false,
        }
    }

    #[test]
    fn first_person_grabs_the_pointer_and_tab_releases_it() {
        let build = Mode::Build;
        let third = ViewMode::Third;
        let first = ViewMode::First;

        // Switching into first person over the view grabs the pointer.
        assert!(next_capture(
            false,
            build,
            third,
            capture_input(build, first)
        ));
        // Doing it with the pointer over the palette does not.
        assert!(!next_capture(
            false,
            build,
            third,
            CaptureInput {
                over_viewport: false,
                ..capture_input(build, first)
            }
        ));

        // Tab lets go without leaving first person.
        assert!(!next_capture(
            true,
            build,
            first,
            CaptureInput {
                tab: true,
                ..capture_input(build, first)
            }
        ));
        // Tab over the view takes it back; over the palette it does not.
        assert!(next_capture(
            false,
            build,
            first,
            CaptureInput {
                tab: true,
                ..capture_input(build, first)
            }
        ));
        assert!(!next_capture(
            false,
            build,
            first,
            CaptureInput {
                tab: true,
                over_viewport: false,
                ..capture_input(build, first)
            }
        ));

        // Leaving first person and losing focus both release it.
        assert!(!next_capture(
            true,
            build,
            first,
            CaptureInput {
                first_person: false,
                ..capture_input(build, third)
            }
        ));
        assert!(!next_capture(
            true,
            build,
            first,
            CaptureInput {
                focus_lost: true,
                ..capture_input(build, first)
            }
        ));
        // Regaining focus does not grab by itself.
        assert!(!next_capture(
            false,
            build,
            first,
            capture_input(build, first)
        ));
    }

    #[test]
    fn play_keeps_first_person_capture_and_build_releases_it() {
        let first = ViewMode::First;
        let third = ViewMode::Third;

        assert!(next_capture(
            true,
            Mode::Build,
            first,
            capture_input(Mode::Play, first)
        ));
        assert!(!next_capture(
            true,
            Mode::Play,
            first,
            capture_input(Mode::Build, first)
        ));
        // Play in third person, or on touch, never grabs.
        assert!(!next_capture(
            false,
            Mode::Build,
            third,
            capture_input(Mode::Play, third)
        ));
        assert!(!next_capture(
            true,
            Mode::Build,
            first,
            CaptureInput {
                first_person: false,
                ..capture_input(Mode::Build, first)
            }
        ));
    }

    #[test]
    fn the_palette_is_not_the_viewport_but_all_of_play_is() {
        let mut editor = Editor::default();
        let mut touch = TouchState::default();
        let on_palette = Some(Vec2::new(10.0, 10.0));
        let on_view = Some(Vec2::new(PANEL_WIDTH + 1.0, 10.0));

        assert!(!over_viewport(&editor, &touch, on_palette));
        assert!(over_viewport(&editor, &touch, on_view));
        assert!(!over_viewport(&editor, &touch, None));

        // Play hides the palette, so the whole screen can be looked through.
        editor.mode = Mode::Play;
        assert!(over_viewport(&editor, &touch, on_palette));

        // Touch has no captured pointer.
        editor.mode = Mode::Build;
        touch.enabled = true;
        assert!(!over_viewport(&editor, &touch, on_view));
    }

    #[test]
    fn the_mode_overlays_follow_the_mode_and_input_layer() {
        let build_desktop = ModeUi {
            desktop: true,
            play: false,
        };
        assert!(layer_matches(build_desktop, Mode::Build, false));
        assert!(!layer_matches(build_desktop, Mode::Play, false));
        assert!(!layer_matches(build_desktop, Mode::Build, true));

        let play_touch = ModeUi {
            desktop: false,
            play: true,
        };
        assert!(layer_matches(play_touch, Mode::Play, true));
        assert!(!layer_matches(play_touch, Mode::Build, true));
        assert!(!layer_matches(play_touch, Mode::Play, false));
    }

    #[test]
    fn entering_play_holsters_tools_but_keeps_the_level_and_walk() {
        let mut editor = Editor {
            tool: Some(0),
            actor_tool: Some(ActorTool::Path),
            selected: Some(3),
            walk: Some(Walk {
                distance: 0.4,
                stuck: 0.0,
            }),
            plan: Some(Plan::default()),
            ..Default::default()
        };

        set_mode(&mut editor, Mode::Play);

        assert_eq!(editor.mode, Mode::Play);
        assert!(editor.tool.is_none());
        assert!(editor.actor_tool.is_none());
        assert!(editor.selected.is_none());
        assert!(editor.walk.is_some(), "a live walk keeps going in play");
        assert!(editor.plan.is_some(), "the plan survives a mode switch");
    }

    #[test]
    fn play_mode_only_allows_non_authoring_ui_actions() {
        assert!(!palette_is_authoring(PaletteAction::Build));
        assert!(!palette_is_authoring(PaletteAction::Play));
        assert!(!palette_is_authoring(PaletteAction::View));
        assert!(palette_is_authoring(PaletteAction::Character));
        assert!(palette_is_authoring(PaletteAction::Shape(0)));
        assert!(palette_is_authoring(PaletteAction::Course));

        assert!(!touch_is_authoring(TouchAction::Play));
        assert!(!touch_is_authoring(TouchAction::View));
        assert!(!touch_is_authoring(TouchAction::Jump));
        assert!(touch_is_authoring(TouchAction::Path));
        assert!(touch_is_authoring(TouchAction::Place));
    }

    #[test]
    fn the_line_prefix_stops_at_the_current_sample() {
        let track = path::Track::new(vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
        let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
        let prefix = line_prefix(&track, &line, Vec2::new(1.1, 0.0));
        assert_eq!(prefix.len(), 2);
    }

    #[test]
    fn successive_live_edits_keep_the_walked_prefix() {
        let a = Vec3::ZERO;
        let b = Vec3::X;
        let c = Vec3::X * 2.0;
        let d = Vec3::X * 3.0;
        let first_track =
            path::Track::new(vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
        let trail = walked_trail(&[], &first_track, &[a, b, c], Vec2::new(1.0, 0.0));
        assert_eq!(trail, vec![a, b]);

        let second_track = path::Track::new(vec![
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(3.0, 0.0),
        ]);
        let trail = walked_trail(&trail, &second_track, &[b, c, d], Vec2::new(2.0, 0.0));
        assert_eq!(trail, vec![a, b, c], "the first walked prefix is immutable");
    }

    #[test]
    fn arrival_prunes_the_final_waypoint() {
        let mut editor = Editor::default();
        let mut plan = Plan::default();
        plan.replace(Vec2::ZERO, (0, 0), 0.0);
        editor.plan = Some(plan);
        editor.route = Some(test_route(true));
        prune_plan(&mut editor, 0.95);
        assert!(
            editor.plan.as_ref().is_none_or(|plan| plan.is_empty()),
            "the final waypoint must be pruned on arrival"
        );
        assert!(editor.route.as_ref().unwrap().distances.is_empty());
    }

    #[test]
    fn a_live_rebuild_starts_at_the_current_track_point() {
        let route = Route {
            track: path::Track::new(vec![Vec2::ZERO, Vec2::new(4.0, 0.0)]),
            trail: Vec::new(),
            line: Vec::new(),
            distances: vec![4.0],
            controls: Vec::new(),
            valid: true,
            hexes: 2,
        };
        let walk = Walk {
            distance: 2.0,
            stuck: 0.0,
        };
        assert_eq!(
            rebuild_start(true, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
            Vec2::new(2.0, 0.0)
        );
        assert_eq!(
            rebuild_start(false, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
            Vec2::new(9.0, 9.0)
        );
    }

    #[test]
    fn a_refused_edit_keeps_the_old_plan() {
        let mut old = Plan::default();
        old.replace(Vec2::ZERO, (0, 0), 0.0);
        let mut new = old.clone();
        new.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
        assert_eq!(
            choose_plan(Some(old.clone()), Some(new.clone()), false)
                .expect("old plan")
                .waypoints
                .len(),
            1
        );
        assert_eq!(
            choose_plan(Some(old), Some(new), true)
                .expect("new plan")
                .waypoints
                .len(),
            2
        );
    }

    fn test_route(valid: bool) -> Route {
        Route {
            track: path::Track::new(vec![Vec2::ZERO, Vec2::X]),
            trail: Vec::new(),
            line: Vec::new(),
            distances: vec![1.0],
            controls: vec![path::AnchorControls {
                incoming: None,
                outgoing: None,
            }],
            valid,
            hexes: 2,
        }
    }

    #[test]
    fn walk_is_enabled_only_for_a_stopped_valid_plan() {
        let mut plan = Plan::default();
        plan.replace(Vec2::ZERO, (0, 0), 0.0);
        let empty = Plan::default();
        assert!(walk_enabled(Some(&plan), Some(&test_route(true)), false));
        assert!(!walk_enabled(Some(&plan), Some(&test_route(true)), true));
        assert!(!walk_enabled(Some(&plan), Some(&test_route(false)), false));
        assert!(!walk_enabled(Some(&empty), Some(&test_route(true)), false));
        assert!(!walk_enabled(None, None, false));
    }

    #[test]
    fn a_nav_rebuild_never_starts_or_resurrects_a_walk() {
        assert_eq!(rebuild_walk_state(false, true), (true, false));
        assert_eq!(rebuild_walk_state(true, true), (true, true));
        assert_eq!(rebuild_walk_state(false, false), (false, false));
        assert_eq!(rebuild_walk_state(true, false), (false, true));
    }

    #[test]
    fn smoothing_clears_manual_tangents() {
        let mut plan = Plan::default();
        plan.replace(Vec2::ZERO, (0, 0), 0.0);
        plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
        plan.waypoints[0].outgoing = Some(Vec2::new(1.0, 1.0));
        plan.waypoints[1].incoming = Some(Vec2::new(1.0, 1.0));
        clear_tangents(&mut plan);
        assert!(
            plan.waypoints
                .iter()
                .all(|w| w.incoming.is_none() && w.outgoing.is_none())
        );
    }

    #[test]
    fn a_gizmo_handle_wins_over_an_armed_shape_tool() {
        let mut editor = Editor {
            tool: Some(0),
            ..Default::default()
        };
        assert!(
            gizmo_takes_pointer(&editor, false),
            "a gizmo handle must be grabbable while placing shapes"
        );
        editor.actor_tool = Some(ActorTool::Place);
        assert!(!gizmo_takes_pointer(&editor, false));
        editor.actor_tool = Some(ActorTool::Path);
        assert!(!gizmo_takes_pointer(&editor, false));
    }

    #[test]
    fn a_waypoint_handle_has_a_fingertip_target() {
        assert!(within_radius(Vec2::ZERO, Vec2::new(20.0, 0.0), 24.0));
        assert!(!within_radius(Vec2::ZERO, Vec2::new(25.0, 0.0), 24.0));
    }

    #[test]
    fn touch_suppress_blocks_a_tap_after_a_button() {
        let touch = TouchState {
            suppress_until: 2.0,
            ..Default::default()
        };
        assert!(!tap_allowed(1.9, &touch));
        assert!(tap_allowed(2.1, &touch));
        let multi = TouchState {
            multi: true,
            ..Default::default()
        };
        assert!(!tap_allowed(3.0, &multi));
    }

    #[test]
    fn leaving_plan_keeps_the_plan() {
        let mut editor = Editor::default();
        let mut plan = Plan::default();
        plan.replace(Vec2::ZERO, (0, 0), 0.0);
        editor.plan = Some(plan);
        editor.actor_tool = Some(ActorTool::Plan);
        editor.actor_tool = Some(ActorTool::Path);
        assert_eq!(editor.plan.as_ref().map(|p| p.waypoints.len()), Some(1));
    }

    #[test]
    fn a_wall_face_is_clickable_at_mid_height() {
        let wall = model::shape_index("wall").unwrap();
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![
            Piece {
                id: 1,
                shape: floor,
                pos: Vec2::ZERO,
                rot: 0.0,
                elevation: 0.0,
            },
            Piece {
                id: 2,
                shape: wall,
                pos: Vec2::new(0.519, -0.9),
                rot: std::f32::consts::FRAC_PI_2,
                elevation: 0.0,
            },
        ];
        let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 1.4, -0.9)));
        assert_eq!(
            hit,
            Some(2),
            "clicking the wall face should pick the wall, not the floor under it"
        );
    }

    #[test]
    fn a_wall_is_clickable_near_its_top_too() {
        let wall = model::shape_index("wall").unwrap();
        let pieces = vec![Piece {
            id: 2,
            shape: wall,
            pos: Vec2::new(0.519, -0.9),
            rot: std::f32::consts::FRAC_PI_2,
            elevation: 0.0,
        }];
        let hit = pick(&pieces, ray_towards(Vec3::new(0.519, 2.6, -0.9)));
        assert_eq!(hit, Some(2), "the upper face of a wall should be clickable");
    }

    #[test]
    fn the_floor_is_clickable() {
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![Piece {
            id: 1,
            shape: floor,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        }];
        assert_eq!(
            pick(&pieces, ray_towards(Vec3::new(0.2, 0.0, 0.2))),
            Some(1)
        );
    }

    #[test]
    fn the_course_lines_up_for_a_walk() {
        let (pieces, character) = course();
        assert_eq!(pieces.len(), 4);

        let ramp = pieces
            .iter()
            .find(|p| shapes()[p.shape].id == "ramp")
            .unwrap();
        let block = pieces
            .iter()
            .find(|p| shapes()[p.shape].id == "block")
            .unwrap();

        // Ramp rotated 270 degrees rises along +x; its top edge and the block's
        // near face should meet at the same height with a small gap.
        let ramp_top_x = ramp.pos.x + 1.0;
        let block_near_x = block.pos.x - 0.9;
        assert!(
            (block_near_x - ramp_top_x).abs() < 0.25,
            "ramp top {ramp_top_x} vs block face {block_near_x}"
        );
        assert!((shapes()[ramp.shape].top() - shapes()[block.shape].top()).abs() < 1e-4);

        // Start before the ramp, facing +x.
        assert!(character.pos.x < ramp.pos.x);
        let forward = Vec2::new(-character.yaw.sin(), -character.yaw.cos());
        assert!(
            forward.x > 0.99,
            "course should start facing +x, got {forward}"
        );
    }

    #[test]
    fn the_touch_layout_owns_its_control_zones() {
        let size = Vec2::new(390.0, 844.0);
        assert!(over_controls(Vec2::new(100.0, 20.0), size, 1.0), "toolbar");
        assert!(over_controls(Vec2::new(60.0, 800.0), size, 1.0), "stick");
        assert!(over_controls(Vec2::new(340.0, 800.0), size, 1.0), "jump");
        assert!(!over_controls(Vec2::new(195.0, 420.0), size, 1.0), "canvas");
    }

    #[test]
    fn grabbing_the_stick_wins_over_the_control_zone() {
        let size = Vec2::new(390.0, 844.0);
        let s = 0.5;
        let centre = Vec2::new(
            (STICK_MARGIN + STICK_BASE * 0.5) * s,
            size.y - (STICK_MARGIN + STICK_BASE * 0.5) * s,
        );
        // The pad deliberately sits inside the bottom-left control zone; a
        // press on it must still grab the stick rather than be ignored.
        assert!(over_controls(centre, size, s), "the pad is a control");
        assert!(grabs_stick(centre, centre, STICK_REACH * s), "pad centre");
        assert!(grabs_stick(
            centre + Vec2::new(STICK_REACH * s * 0.9, 0.0),
            centre,
            STICK_REACH * s
        ));
        assert!(!grabs_stick(
            centre + Vec2::new(STICK_REACH * s * 1.5, 0.0),
            centre,
            STICK_REACH * s
        ));
    }

    #[test]
    fn the_route_line_climbs_a_ramp_and_ignores_wall_tops() {
        // A ramp rising from 0 to 0.9 between x = 1 and x = 3, then a block.
        let ramp = |p: Vec2| -> Option<f32> {
            Some(if p.x < 1.0 {
                0.0
            } else if p.x < 3.0 {
                (p.x - 1.0) * 0.45
            } else {
                0.9
            })
        };
        let points: Vec<Vec2> = (0..11).map(|i| Vec2::new(i as f32 * 0.3, 0.0)).collect();
        let heights = lift_heights(points.iter().copied(), ramp, 0.0);
        assert_eq!(heights.len(), 11);
        assert!((heights[0] - 0.0).abs() < 1e-4);
        assert!((heights[5] - 0.225).abs() < 1e-4, "mid-ramp {heights:?}");
        assert!((heights[10] - 0.9).abs() < 1e-4, "block top {heights:?}");
        assert!(
            heights.windows(2).all(|pair| pair[0] <= pair[1] + 1e-4),
            "the line should only climb: {heights:?}"
        );

        // A wall top (2.8) beside the curve is not treated as a step.
        let wall = |p: Vec2| Some(if (p.x - 1.0).abs() < 0.05 { 2.8 } else { 0.0 });
        let heights = lift_heights(
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(2.0, 0.0),
            ]
            .into_iter(),
            wall,
            0.0,
        );
        assert_eq!(heights, vec![0.0, 0.0, 0.0]);

        // A missed ray between two surfaces keeps the last height (no crack).
        let missing = lift_heights(
            [Vec2::ZERO, Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0)].into_iter(),
            |_| None,
            0.9,
        );
        assert_eq!(missing, vec![0.9, 0.9, 0.9]);
    }

    #[test]
    fn the_route_line_bridges_a_seam_but_falls_off_a_real_edge() {
        // Ramp/block tops at 0.9 with a narrow crack at x = 2.0.
        let cracked = |p: Vec2| Some(if (1.9..=2.1).contains(&p.x) { 0.0 } else { 0.9 });
        let seam = lift_heights(
            [
                Vec2::new(1.7, 0.0),
                Vec2::new(2.0, 0.0),
                Vec2::new(2.3, 0.0),
            ]
            .into_iter(),
            cracked,
            0.9,
        );
        assert_eq!(seam, vec![0.9, 0.9, 0.9], "the seam is bridged");

        // Past the end of the block there is no support within the footprint.
        let edge = |p: Vec2| Some(if p.x < 2.0 { 0.9 } else { 0.0 });
        let dropped = lift_heights(
            [
                Vec2::new(2.3, 0.0),
                Vec2::new(2.5, 0.0),
                Vec2::new(2.7, 0.0),
            ]
            .into_iter(),
            edge,
            0.9,
        );
        assert_eq!(dropped, vec![0.0, 0.0, 0.0], "a real edge still drops");
    }

    #[test]
    fn the_route_line_smooths_the_step_onto_a_block() {
        let smoothed = smooth_heights(&[0.0, 0.0, 0.9, 0.9]);
        assert!((smoothed[0] - 0.0).abs() < 1e-4);
        assert!((smoothed[1] - 0.225).abs() < 1e-4);
        assert!((smoothed[2] - 0.675).abs() < 1e-4);
        assert!((smoothed[3] - 0.9).abs() < 1e-4);
    }

    #[test]
    fn the_gizmo_is_bigger_than_the_selected_pieces_bounding_box() {
        for (shape_index, shape) in shapes().iter().enumerate() {
            let piece = Piece {
                id: 0,
                shape: shape_index,
                pos: Vec2::ZERO,
                rot: 0.0,
                elevation: 0.0,
            };
            let (reach, ring) = gizmo_frame(&piece, shape);
            let size = bounding_size(&piece, shape);
            assert!(
                reach > size,
                "{}: gizmo handle reach {reach} should be bigger than its bounding box {size}",
                shape.id
            );
            assert!(
                ring > size * 0.5,
                "{}: gizmo yaw ring {ring} should clear half its bounding box {size}",
                shape.id
            );
        }
    }

    #[test]
    fn gizmo_axis_projection_and_screen_distance() {
        // A ray straight down at x = 2 hits the X axis at s = 2.
        let ray = Ray3d::new(Vec3::new(2.0, 5.0, 0.0), Dir3::NEG_Y);
        let s = ray_axis_param(ray, Vec3::ZERO, Vec3::X).unwrap();
        assert!((s - 2.0).abs() < 1e-4, "s = {s}");

        // And at z = -1.5 for the Z axis.
        let ray = Ray3d::new(Vec3::new(0.0, 5.0, -1.5), Dir3::NEG_Y);
        let s = ray_axis_param(ray, Vec3::ZERO, Vec3::Z).unwrap();
        assert!((s + 1.5).abs() < 1e-4, "s = {s}");

        let d = point_segment_distance(Vec2::new(5.0, 5.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-4);
        let d = point_segment_distance(Vec2::new(-3.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0));
        assert!((d - 3.0).abs() < 1e-4);
    }

    #[test]
    fn touch_buttons_include_plan_and_add() {
        let css = Vec2::new(390.0, 844.0);
        assert_eq!(
            touch_action_at(Vec2::new(352.0, 25.0), css),
            Some(TouchAction::Course)
        );
        assert_eq!(
            touch_action_at(Vec2::new(300.0, 25.0), css),
            Some(TouchAction::Plan)
        );
        assert_eq!(
            touch_action_at(Vec2::new(370.0, 25.0), css),
            Some(TouchAction::Play)
        );
        assert_eq!(
            touch_action_at(Vec2::new(40.0, 100.0), css),
            Some(TouchAction::Add)
        );
    }

    #[test]
    fn touch_buttons_are_hit_by_css_position() {
        let css = Vec2::new(390.0, 844.0);
        assert_eq!(
            touch_action_at(Vec2::new(37.0, 25.0), css),
            Some(TouchAction::View)
        );
        assert_eq!(
            touch_action_at(Vec2::new(352.0, 25.0), css),
            Some(TouchAction::Course)
        );
        assert_eq!(
            touch_action_at(Vec2::new(336.0, 63.0), css),
            Some(TouchAction::Place)
        );
        assert_eq!(
            touch_action_at(Vec2::new(53.0, 63.0), css),
            Some(TouchAction::PrevShape)
        );
        assert_eq!(
            touch_action_at(Vec2::new(328.0, 778.0), css),
            Some(TouchAction::Jump)
        );
        assert_eq!(touch_action_at(Vec2::new(200.0, 400.0), css), None);
    }

    #[test]
    fn the_stick_maps_screen_to_walk_direction() {
        let centre = Vec2::new(86.0, 758.0);
        assert_eq!(stick_offset(centre, centre, 1.0), Vec2::ZERO);

        let right = stick_offset(centre + Vec2::new(44.0, 0.0), centre, 1.0);
        assert!((right - Vec2::X).length() < 1e-4, "{right}");

        // Screen up is +y in walk space.
        let up = stick_offset(centre - Vec2::new(0.0, 44.0), centre, 1.0);
        assert!((up - Vec2::Y).length() < 1e-4, "{up}");

        let far = stick_offset(centre + Vec2::new(300.0, 0.0), centre, 1.0);
        assert!((far.length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn every_catalogue_shape_gets_a_collider() {
        for shape in shapes() {
            assert!(
                piece_collider(shape).is_some(),
                "{} has no collider",
                shape.id
            );
        }
    }

    #[test]
    fn a_jump_launches_only_from_the_ground() {
        assert_eq!(jump_velocity(0.0, true, true), JUMP_SPEED);
        assert_eq!(jump_velocity(0.0, true, false), 0.0);
        assert_eq!(jump_velocity(-3.0, false, true), -3.0);
    }

    #[test]
    fn a_slope_counts_as_ground_on_the_way_down() {
        assert_eq!(ground_support(0.0, false, false), (true, None), "standing");
        assert_eq!(
            ground_support(0.01, false, false),
            (true, Some(0.01)),
            "contact"
        );
        assert_eq!(
            ground_support(0.2, true, false),
            (true, Some(0.2)),
            "walking down a ramp"
        );
        assert_eq!(
            ground_support(0.2, false, false),
            (false, None),
            "falling past a ledge"
        );
        assert_eq!(
            ground_support(0.4, true, false),
            (false, None),
            "too far below to step down"
        );
        assert_eq!(
            ground_support(-0.02, false, false),
            (true, None),
            "slight penetration is ground, but must not be lifted"
        );
        assert_eq!(
            ground_support(0.05, true, true),
            (true, None),
            "an ascent is ground but is never re-seated"
        );
    }

    #[test]
    fn a_rounded_capsule_on_a_slope_has_no_gap() {
        assert!((contact_gap(PILL_RADIUS, 1.0)).abs() < 1e-6, "flat");
        assert!((contact_gap(PILL_RADIUS / 0.9, 0.9)).abs() < 1e-5, "slope");
        assert!(
            contact_gap(PILL_RADIUS + 0.06, 1.0) > 0.0,
            "above the ground"
        );
    }

    /// A world with the invisible floor and a character, without any of the
    /// rendering or UI plugins.
    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            PhysicsPlugins::new(PostUpdate),
        ));
        app.init_asset::<Mesh>();
        app.init_resource::<SpatialQueryDiagnostics>();
        app.init_resource::<avian3d::collider_tree::ColliderTreeDiagnostics>();
        app.init_resource::<avian3d::collision::CollisionDiagnostics>();
        app.init_resource::<avian3d::dynamics::solver::SolverDiagnostics>();
        app.insert_resource(Kit {
            parts: Vec::new(),
            mats: Vec::new(),
            glass: Handle::default(),
            ghost: Handle::default(),
            pill: Handle::default(),
            pill_mat: Handle::default(),
            character_collider: Collider::capsule(PILL_RADIUS, PILL_CYLINDER),
        });
        app.init_resource::<Editor>();
        app.init_resource::<Orbit>();
        app.init_resource::<TouchState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(20.0, 0.2, 20.0),
            Position::from_xyz(0.0, -0.1, 0.0),
        ));
        app.world_mut().spawn(CharacterVisual);
        // Let the physics pipeline build its broad phase for the floor.
        app.update();
        app.update();
        app
    }

    fn tick_character(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut()
            .run_system_once(character_move)
            .expect("character move");
    }

    #[test]
    fn third_person_can_jump_without_a_route() {
        let mut app = physics_app();
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.character = Some(Character {
                pos: Vec2::ZERO,
                feet: 0.0,
                yaw: 0.0,
                vy: 0.0,
                grounded: true,
            });
            editor.walk = None;
        }
        app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);

        tick_character(&mut app);

        let feet = app.world().resource::<Editor>().character.unwrap().feet;
        assert!(
            feet > 0.05,
            "character should leave the ground, feet={feet}"
        );
    }

    #[test]
    fn an_airborne_character_lands_without_a_route() {
        let mut app = physics_app();
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.character = Some(Character {
                pos: Vec2::ZERO,
                feet: 1.0,
                yaw: 0.0,
                vy: 0.0,
                grounded: false,
            });
            editor.walk = None;
        }
        app.world_mut().resource_mut::<Orbit>().mode = ViewMode::Third;

        for _ in 0..120 {
            tick_character(&mut app);
        }

        let character = app.world().resource::<Editor>().character.unwrap();
        assert!(
            character.feet < 0.2,
            "should fall back to the floor, feet={}",
            character.feet
        );
        assert!(character.grounded, "should be grounded after landing");
    }

    #[test]
    fn can_jump_while_climbing_a_ramp() {
        let mut app = physics_app();
        let ramp = &shapes()[model::shape_index("ramp").unwrap()];
        app.world_mut().spawn((
            RigidBody::Static,
            piece_collider(ramp).unwrap(),
            Position::from_xyz(2.0, 0.0, 0.0),
            Rotation(model::yaw_quat(std::f32::consts::PI * 1.5)),
        ));
        app.update();

        app.world_mut().resource_mut::<Orbit>().mode = ViewMode::First;
        app.world_mut().resource_mut::<Orbit>().yaw = -std::f32::consts::FRAC_PI_2;
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.character = Some(Character {
                pos: Vec2::new(0.5, 0.0),
                feet: 0.0,
                yaw: -std::f32::consts::FRAC_PI_2,
                vy: 0.0,
                grounded: true,
            });
            editor.walk = None;
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);

        let mut on_ramp = false;
        for _ in 0..180 {
            tick_character(&mut app);
            let character = app.world().resource::<Editor>().character.unwrap();
            if (0.25..0.7).contains(&character.feet) {
                on_ramp = true;
                break;
            }
        }
        assert!(on_ramp, "character should climb the ramp");

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        tick_character(&mut app);

        let character = app.world().resource::<Editor>().character.unwrap();
        assert!(
            character.vy > 4.0,
            "jump should launch from the ramp, vy={}",
            character.vy
        );
    }

    #[test]
    fn can_jump_while_descending_a_ramp() {
        let mut app = physics_app();
        let ramp = &shapes()[model::shape_index("ramp").unwrap()];
        app.world_mut().spawn((
            RigidBody::Static,
            piece_collider(ramp).unwrap(),
            Position::from_xyz(2.0, 0.0, 0.0),
            Rotation(model::yaw_quat(std::f32::consts::PI * 1.5)),
        ));
        app.update();

        app.world_mut().resource_mut::<Orbit>().mode = ViewMode::First;
        // Forward along -x, i.e. down the ramp.
        app.world_mut().resource_mut::<Orbit>().yaw = std::f32::consts::FRAC_PI_2;
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.character = Some(Character {
                pos: Vec2::new(2.9, 0.0),
                feet: 0.85,
                yaw: std::f32::consts::FRAC_PI_2,
                vy: 0.0,
                grounded: true,
            });
            editor.walk = None;
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);

        let mut descending = false;
        for _ in 0..180 {
            tick_character(&mut app);
            let character = app.world().resource::<Editor>().character.unwrap();
            if (0.25..0.7).contains(&character.feet) {
                descending = true;
                break;
            }
        }
        assert!(descending, "character should walk down the ramp");

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        tick_character(&mut app);

        let character = app.world().resource::<Editor>().character.unwrap();
        assert!(
            character.vy > 4.0,
            "jump should launch while descending the ramp, vy={}",
            character.vy
        );
    }

    #[test]
    fn clicking_empty_ground_selects_nothing() {
        let floor = model::shape_index("floor").unwrap();
        let pieces = vec![Piece {
            id: 1,
            shape: floor,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        }];
        assert_eq!(pick(&pieces, ray_towards(Vec3::new(5.0, 0.0, 5.0))), None);
    }

    #[test]
    fn an_axis_drag_moves_by_the_pointer_delta_not_to_the_pointer() {
        let start = Piece {
            id: 1,
            shape: 0,
            pos: Vec2::new(1.0, 2.0),
            rot: 0.0,
            elevation: 0.0,
        };
        // Grabbed far out on the handle, then moved a little. The piece must
        // follow the little movement, not leap out to the pointer.
        let moved = dragged_piece(
            start,
            GizmoHandle::X,
            Vec3::new(3.0, 0.5, 2.0),
            Vec3::new(3.4, 0.5, 2.0),
        );
        assert!((moved.pos.x - 1.4).abs() < 1e-5, "pos = {:?}", moved.pos);
        assert!((moved.pos.y - 2.0).abs() < 1e-5, "pos = {:?}", moved.pos);
        assert!((moved.elevation - 0.0).abs() < 1e-5);
    }

    #[test]
    fn the_vertical_handle_lifts_and_lowers_a_piece() {
        let start = Piece {
            id: 1,
            shape: 0,
            pos: Vec2::new(1.0, 2.0),
            rot: 0.0,
            elevation: 0.5,
        };
        // Grabbed at the top of the vertical handle, not at the piece origin.
        let moved = dragged_piece(
            start,
            GizmoHandle::Y,
            Vec3::new(1.0, 2.0, 2.0),
            Vec3::new(1.0, 2.75, 2.0),
        );
        assert!((moved.elevation - 1.25).abs() < 1e-5, "{}", moved.elevation);
        assert_eq!(moved.pos, start.pos);
    }

    #[test]
    fn grabbing_the_vertical_handle_without_dragging_keeps_the_height() {
        let block = model::shape_index("block").unwrap();
        let start = Piece {
            id: 1,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            // Free-placed just above the ground, where a snap would move it.
            elevation: 0.1,
        };
        let mut editor = Editor::default();
        editor.pieces.push(start);
        let origin = gizmo_origin(&start, &shapes()[block]);
        let grab = |target: Vec3| {
            let eye = target + Vec3::new(3.0, 4.0, 0.0);
            Ray3d::new(eye, Dir3::new(target - eye).unwrap())
        };
        let drag = GizmoDrag {
            id: start.id,
            handle: GizmoHandle::Y,
            grab: origin,
            start,
            last_raw: start.elevation,
            last: start.elevation,
        };

        // A click with no pointer travel must not change the height.
        apply_gizmo_drag(&mut editor, drag, Some(grab(origin)));
        assert!(
            (editor.pieces[0].elevation - start.elevation).abs() < 1e-5,
            "elevation = {}",
            editor.pieces[0].elevation
        );

        // Dragging down onto the ground does snap.
        let drag = editor.gizmo_drag.expect("drag");
        apply_gizmo_drag(&mut editor, drag, Some(grab(origin - Vec3::Y * 0.1)));
        assert!(editor.pieces[0].elevation.abs() < 1e-5);
    }

    #[test]
    fn a_yaw_drag_keeps_the_grab_offset() {
        let start = Piece {
            id: 1,
            shape: 0,
            pos: Vec2::ZERO,
            rot: 0.3,
            elevation: 0.0,
        };
        // Grabbed on the +X side of the ring and dragged a quarter turn to +Z.
        let moved = dragged_piece(
            start,
            GizmoHandle::Yaw,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
        );
        let expected = model::normalize_angle(0.3 + std::f32::consts::FRAC_PI_2);
        assert!((moved.rot - expected).abs() < 1e-5, "rot = {}", moved.rot);
    }

    #[test]
    fn a_plane_handle_moves_in_two_axes_at_once() {
        let start = Piece {
            id: 1,
            shape: 0,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        };
        let moved = dragged_piece(
            start,
            GizmoHandle::Xy,
            Vec3::new(1.0, 0.5, 0.0),
            Vec3::new(1.4, 1.1, 0.0),
        );
        assert!((moved.pos.x - 0.4).abs() < 1e-5);
        assert!((moved.pos.y - 0.0).abs() < 1e-5);
        assert!((moved.elevation - 0.6).abs() < 1e-5);
    }

    #[test]
    fn height_snapping_stacks_a_block_on_the_one_below() {
        let block = model::shape_index("block").unwrap();
        let below = Piece {
            id: 1,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        };
        let moving = Piece {
            id: 2,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        };
        let top = shapes()[block].top();
        let snapped = snap_elevation(
            moving,
            &shapes()[block],
            &[below],
            top - 0.05,
            1.0,
            true,
            ELEVATION_SNAP,
        );
        assert!((snapped - top).abs() < 1e-5, "snapped = {snapped}");
    }

    #[test]
    fn height_snapping_leaves_free_heights_alone() {
        let crate_index = model::shape_index("crate").unwrap();
        let piece = Piece {
            id: 1,
            shape: crate_index,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        };
        let shape = &shapes()[crate_index];
        let free = snap_elevation(piece, shape, &[], 0.62, 1.0, true, ELEVATION_SNAP);
        assert!((free - 0.62).abs() < 1e-5, "free = {free}");
        let unsnapped = snap_elevation(piece, shape, &[], 0.62, 1.0, false, ELEVATION_SNAP);
        assert!((unsnapped - 0.62).abs() < 1e-5, "unsnapped = {unsnapped}");
    }

    #[test]
    fn height_snapping_releases_a_stacked_piece_immediately() {
        let block = model::shape_index("block").unwrap();
        let below = Piece {
            id: 1,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        };
        let moving = Piece {
            id: 2,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: shapes()[block].top(),
        };
        let top = shapes()[block].top();
        let lifted = snap_elevation(
            moving,
            &shapes()[block],
            &[below],
            top + 0.01,
            1.0,
            true,
            ELEVATION_SNAP,
        );
        assert!((lifted - (top + 0.01)).abs() < 1e-5, "lifted = {lifted}");
    }

    #[test]
    fn height_snapping_only_captures_surfaces_ahead_of_the_drag() {
        let crate_index = model::shape_index("crate").unwrap();
        let piece = Piece {
            id: 1,
            shape: crate_index,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.05,
        };
        let shape = &shapes()[crate_index];
        // Lifting away from the ground is free: it must not be pulled back down.
        let lifted = snap_elevation(piece, shape, &[], 0.05, 1.0, true, ELEVATION_SNAP);
        assert!((lifted - 0.05).abs() < 1e-5, "lifted = {lifted}");
        // Lowering toward the same ground still snaps.
        let lowered = snap_elevation(piece, shape, &[], 0.05, -1.0, true, ELEVATION_SNAP);
        assert!(lowered.abs() < 1e-5, "lowered = {lowered}");
    }

    #[test]
    fn the_gizmo_offers_a_vertical_and_three_plane_handles() {
        let block = model::shape_index("block").unwrap();
        let piece = Piece {
            id: 1,
            shape: block,
            pos: Vec2::new(1.0, 2.0),
            rot: 0.0,
            elevation: 0.75,
        };
        let shape = &shapes()[block];
        let origin = gizmo_origin(&piece, shape);
        let (reach, _) = gizmo_frame(&piece, shape);
        let points = gizmo_handle_points(&piece, shape);

        let find = |handle| {
            points
                .iter()
                .find(|(h, _)| *h == handle)
                .map(|(_, point)| *point)
        };
        let y = find(GizmoHandle::Y).expect("vertical handle");
        assert!((y - (origin + Vec3::Y * reach)).length() < 1e-5);

        let plane = |a: Vec3, b: Vec3| origin + (a + b) * reach * GIZMO_PLANE_CENTER;
        assert!((find(GizmoHandle::Xy).expect("xy") - plane(Vec3::X, Vec3::Y)).length() < 1e-5);
        assert!((find(GizmoHandle::Zy).expect("zy") - plane(Vec3::Z, Vec3::Y)).length() < 1e-5);
        assert!((find(GizmoHandle::Xz).expect("xz") - plane(Vec3::X, Vec3::Z)).length() < 1e-5);
    }

    #[test]
    fn a_plane_handle_needs_the_pointer_inside_its_square() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        assert!(point_in_quad(Vec2::new(5.0, 5.0), &square));
        assert!(!point_in_quad(Vec2::new(15.0, 5.0), &square));
    }

    #[test]
    fn an_edge_on_plane_handle_does_not_steal_the_click() {
        // A vertical plane seen from straight above collapses to a sliver.
        let sliver = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        assert!(!plane_handle_hit(Vec2::new(5.0, 0.5), &sliver));

        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        assert!(plane_handle_hit(Vec2::new(5.0, 5.0), &square));
    }

    #[test]
    fn the_build_orbit_target_is_the_selected_piece_centre() {
        let block = model::shape_index("block").unwrap();
        let mut editor = Editor::default();
        editor.pieces.push(Piece {
            id: 7,
            shape: block,
            pos: Vec2::new(2.0, 3.0),
            rot: 0.0,
            elevation: 1.0,
        });
        editor.selected = Some(7);
        let target = selection_center(&editor).expect("selected centre");
        let shape = &shapes()[block];
        assert!((target.x - 2.0).abs() < 1e-5);
        assert!((target.z - 3.0).abs() < 1e-5);
        assert!((target.y - (1.0 + (shape.base + shape.top()) * 0.5)).abs() < 1e-5);

        editor.selected = None;
        assert_eq!(selection_center(&editor), None);
    }

    #[test]
    fn saving_writes_each_pieces_height() {
        let block = model::shape_index("block").unwrap();
        let mut editor = Editor::default();
        editor.pieces.push(Piece {
            id: 1,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 1.25,
        });
        let json = level_json(&editor);
        assert!(json.contains("\"y\":1.250"), "{json}");
    }

    #[test]
    fn copying_keeps_the_selected_piece() {
        let block = model::shape_index("block").unwrap();
        let piece = Piece {
            id: 7,
            shape: block,
            pos: Vec2::new(1.0, 2.0),
            rot: 0.4,
            elevation: 1.5,
        };
        let mut editor = Editor::default();
        editor.pieces.push(piece);
        editor.selected = Some(7);

        copy_piece(&mut editor);

        assert_eq!(editor.clipboard, Some(piece));
    }

    #[test]
    fn pasting_makes_a_new_selected_copy_at_the_cursor() {
        let block = model::shape_index("block").unwrap();
        let source = Piece {
            id: 3,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.4,
            elevation: 1.5,
        };
        let mut editor = Editor::default();
        editor.snap = false;
        editor.next_id = 4;
        editor.pieces.push(source);
        editor.selected = Some(3);
        copy_piece(&mut editor);

        paste_piece(&mut editor, Some(Vec2::new(5.0, 6.0)));

        assert_eq!(editor.pieces.len(), 2);
        let pasted = *editor.pieces.last().unwrap();
        assert_eq!(pasted.id, 4);
        assert_eq!(editor.selected, Some(4));
        assert_eq!(pasted.shape, block);
        assert!((pasted.pos - Vec2::new(5.0, 6.0)).length() < 1e-5);
        assert!((pasted.rot - 0.4).abs() < 1e-5);
        assert!((pasted.elevation - 1.5).abs() < 1e-5);
        assert_eq!(editor.undo.len(), 1, "a paste must be undoable");
    }

    #[test]
    fn pasting_without_a_cursor_lands_beside_the_source() {
        let block = model::shape_index("block").unwrap();
        let mut editor = Editor::default();
        editor.snap = false;
        editor.pieces.push(Piece {
            id: 1,
            shape: block,
            pos: model::hex_center(0, 0),
            rot: 0.0,
            elevation: 0.0,
        });
        editor.selected = Some(1);
        copy_piece(&mut editor);

        paste_piece(&mut editor, None);

        let pasted = *editor.pieces.last().unwrap();
        let expected = model::hex_center(1, 0);
        assert!(
            (pasted.pos - expected).length() < 1e-5,
            "pos = {:?}",
            pasted.pos
        );
    }

    #[test]
    fn copying_and_pasting_nothing_are_harmless() {
        let mut editor = Editor::default();
        copy_piece(&mut editor);
        assert!(editor.clipboard.is_none());
        paste_piece(&mut editor, Some(Vec2::ZERO));
        assert!(editor.pieces.is_empty());
        assert_eq!(editor.selected, None);
    }

    fn test_piece(id: u32) -> Piece {
        Piece {
            id,
            shape: model::shape_index("block").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            elevation: 0.0,
        }
    }

    #[test]
    fn undo_then_redo_restores_the_newer_state() {
        let mut editor = Editor::default();
        editor.commit();
        editor.pieces.push(test_piece(1));
        editor.commit();
        editor.pieces.push(test_piece(2));
        editor.undo();
        assert_eq!(editor.pieces.len(), 1);
        assert_eq!(editor.redo.len(), 1);
        editor.redo();
        assert_eq!(editor.pieces.len(), 2);
        assert!(editor.redo.is_empty());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut editor = Editor::default();
        editor.commit();
        editor.pieces.push(test_piece(1));
        editor.commit();
        editor.pieces.push(test_piece(2));
        editor.undo();
        assert_eq!(editor.redo.len(), 1);
        editor.commit();
        editor.pieces.push(test_piece(3));
        assert!(editor.redo.is_empty());
    }

    #[test]
    fn both_history_stacks_are_capped_at_sixty() {
        let mut editor = Editor::default();
        for id in 1..=70 {
            editor.commit();
            editor.pieces.push(test_piece(id));
        }
        assert_eq!(editor.undo.len(), 60);
        assert_eq!(editor.undo[0].len(), 10);
        assert_eq!(editor.undo[59].len(), 69);
        for _ in 0..70 {
            editor.undo();
        }
        assert_eq!(editor.redo.len(), 60);
    }

    #[test]
    fn edits_and_history_moves_mark_the_session_dirty() {
        let mut editor = Editor::default();
        editor.commit();
        editor.pieces.push(test_piece(1));
        assert!(editor.hard_dirty);
        editor.hard_dirty = false;
        editor.undo();
        assert!(editor.hard_dirty);
        editor.hard_dirty = false;
        editor.redo();
        assert!(editor.hard_dirty);
    }

    #[test]
    fn the_course_reset_clears_redo() {
        let mut editor = Editor::default();
        editor.redo.push(vec![test_piece(1)]);
        editor.load_course();
        assert!(editor.redo.is_empty());
    }
}
