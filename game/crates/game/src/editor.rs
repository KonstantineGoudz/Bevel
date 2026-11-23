//! A 3D level editor: a component palette on the left, an orbit camera, and
//! solid meshes extruded from the same footprints the model tests use.
//!
//! Left-drag places and moves, right-drag orbits, middle-drag pans, the wheel
//! zooms. Everything is in metres.

use std::collections::{HashMap, HashSet};

#[cfg(not(target_os = "android"))]
use bevy::clipboard::Clipboard;
#[cfg(target_os = "android")]
use crate::android_clipboard::Clipboard;
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
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};
use bevy_rapier3d::prelude::*;

use crate::controls::{Action, ClusterButton, Controls, PlayerInput};
use crate::locomotion::{LocomotionInput, LocomotionState};
use crate::mesh::{extrude, wedge};
use crate::model::{
    self, AnchorKind, CATEGORIES, HIT_PAD, InteractAction, PANEL_WIDTH, Piece, SNAP_THRESHOLD,
    SnapKind, Style, shapes,
};
use crate::path;
use crate::path::Hex;
use crate::path_edit::{self, PathDrag, PathHandle, Plan, TangentSide};
use crate::session;
use crate::statuses::Health;

use party_dungeon_net::PlayerState;

use selection::Selection;

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
///
/// Public for the headless server simulation, which builds the same capsule.
const EYE_HEIGHT: f32 = 1.55;
pub const PILL_RADIUS: f32 = 0.32;
pub const PILL_CYLINDER: f32 = 1.1;
pub const PILL_HALF: f32 = PILL_CYLINDER * 0.5 + PILL_RADIUS;
/// Walking pace, in metres per second.
///
/// Public for the headless server simulation (shared tuning).
pub const WALK_SPEED: f32 = 2.2;
/// Gravity, in m/s^2. A little above earth so the character does not float.
///
/// Public for the headless server simulation (shared tuning).
pub const GRAVITY: f32 = 18.0;
/// Takeoff velocity, in m/s.
///
/// Public for the headless server simulation (shared tuning).
pub const JUMP_SPEED: f32 = 7.0;
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
        app            .init_resource::<Editor>()
            // Android builds have no system clipboard (arboard has no
            // Android backend), so the stub resource is provided here; on
            // other platforms Bevy already inserted the real one and this
            // is a no-op.
            .init_resource::<Clipboard>()
            .init_resource::<Cursor>()
            .init_resource::<Orbit>()
            .init_resource::<PointerCapture>()
            .init_resource::<Visuals>()
            // One UI unit per logical pixel unless a phone display scales it
            // down; a zero scale would put the stick centre off screen.
            .insert_resource(TouchState {
                ui_scale: 1.0,
                enabled: cfg!(target_os = "android"),
                ..Default::default()
            })
            .init_resource::<PathGesture>()
            .init_resource::<UiScaleApplied>()
            .init_resource::<Nav>()
            .init_resource::<PropertyEditState>()
            .init_resource::<DamageClock>()
            .init_resource::<Movers>()
            .init_resource::<world_sync::WorldSync>()
            // Authoritative world edits staged for the net layer (also
            // initialized there; headless editor tests have no net plugin).
            .init_resource::<crate::net_client::PendingWorldEdits>()
            .init_resource::<AddMenu>()
            .init_resource::<Minimap>()
            .init_resource::<VineVisuals>()
            .init_resource::<WorldTerrainVisuals>()
            .init_resource::<OutlinerState>()
            .init_resource::<OutlinerEdit>()
            .init_resource::<PropertiesPanelState>()
            .init_resource::<RightPanelState>()
            .init_resource::<LeftPanelState>()
            .init_resource::<HomeMenu>()
            // Ordering anchors for the net layer's authoritative-pose
            // correction (see `PlayController`/`CameraFollow`): it must run
            // after the play controller and before the camera follows.
            .configure_sets(Update, (PlayController, CameraFollow))
            // Remappable controls, loaded from browser storage on web.
            .insert_resource(crate::controls::Controls::load())
            .add_systems(
                Startup,
                (
                    character::load_character_model,
                    setup,
                    setup_wasm_text_input,
                    setup_bridge,
                )
                    .chain(),
            )
            .add_systems(Update, sync_selected_glade_wall_mover.after(prune_movers))
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
                    character_move.in_set(PlayController),
                    hazard_damage,
                    handle_interact,
                    respawn_on_death,
                    manage_capture,
                    camera_control.in_set(CameraFollow),
                    path_drag,
                    rebuild_nav,
                    refresh_route,
                )
                    .chain(),
            )
            .add_systems(Update, sync_selected_glade_wall_mover.after(prune_movers))
            // The second half is ordered after `refresh_route` rather than in
            // the same tuple: Bevy's tuple system sets top out before this many
            // systems.
            .add_systems(
                Update,
                (
                    resolve_platform_push,
                    resolve_dynamic_push,
                    shoot_cone,
                    advance_cones,
                    update_cursor,
                    sync_dynamic_pieces,
                    touch_gizmo_fallback,
                    sync_wasm_text_input,
                    update_add_menu,
                    interact,
                    apply_property_edits,
                    sync_visuals,
                    // Own chain slot: the main Update tuple is already at
                    // Bevy's chaining limit, and this only touches the
                    // minimap camera/UI after the main camera has moved.
                    sync_minimap,
                )
                    .chain()
                    .after(refresh_route),
            )
            .add_systems(Update, camera_occlusion.after(camera_control))
            // Streamed worlds (single unified MMO world): runs rarely, on its
            // own so tuple limits elsewhere are untouched. The edit diff runs
            // right after, so an apply reconciles net outcomes and stages the
            // editor's state before composing.
            .add_systems(
                Update,
                (
                    world_sync::apply_world_chunks,
                    world_sync::sync_world_edits,
                )
                    .chain(),
            )
            // Character model + animation. Its own chain so the editor's
            // tuples stay under Bevy's limit; it runs after `sync_visuals` has
            // spawned or despawned the capsule root.
            .add_systems(
                Update,
                (
                    character::inspect_character_model,
                    character::sync_character_avatar,
                    character::wire_character_animation,
                    character::mark_animation_root,
                    character::hide_collider_meshes,
                    character::drive_character_animation,
                )
                    .chain()
                    .after(sync_visuals),
            )
            .add_systems(
                PostUpdate,
                character::pin_animation_root
                    .after(bevy::app::AnimationSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
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
                    update_home_capture,
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
                    sync_right_panel,
                    sync_left_panel,
                    properties_scroll,
                    sync_transparency_slider,
                    sync_trigger_color_buttons,
                    sync_trigger_visible_label,
                    update_palette,
                    sync_palette_groups,
                    sync_path_actions,
                    sync_grass_actions,
                    update_touch_buttons,
                    update_status,
                    update_touch_status,
                    update_character_status,
                    draw,
                    draw_view_cone,
                )
                    .chain()
                    .after(sync_visuals),
            )
            .add_systems(Update, sync_bezier_control_meshes.after(draw))
            // Native home screen (controls). Its own chain so the
            // editor chains stay under Bevy's tuple limit.
            .add_systems(
                Update,
                (sync_world_terrain, sync_vine_actions, sync_vines, sync_home_menu)
                    .chain()
                    .after(sync_visuals),
            )
            // Live MCP bridge: drain commands before the minimap/visuals read
            // the editor, then publish the fresh snapshot.
            .add_systems(
                Update,
                (bridge_drain, bridge_publish).chain(),
            )
            .add_observer(on_palette_activate)
            .add_observer(on_touch_button)
            .add_observer(on_add_menu_item)
            .add_observer(on_home_button);

        // Replicon path: pull the local body toward the replicated player pose,
        // in the same slot as the legacy `net_pose_correction` (after the local
        // controller, before the camera follows it). Registered here because it
        // needs the editor's `Editor`/`Orbit` resources.
        #[cfg(feature = "replicon")]
        app.add_systems(
            Update,
            replicon_pose_correction
                .after(PlayController)
                .before(CameraFollow),
        );

        gizmo::configure_xray_gizmos(app);
        minimap::configure_minimap_gizmos(app);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub(crate) struct Editor {
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
    /// Everything selected. The primary (first) piece drives the single-object
    /// tools; the whole set moves, rotates, copies and deletes together.
    selection: Selection,
    /// The last copied pieces in their original layout, ready to paste.
    clipboard: Vec<Piece>,
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
    /// Painted grass cover: hex -> density 1..=GRASS_MAX. Renders as tuft
    /// clusters and optionally slows movement and routes.
    grass: HashMap<Hex, u8>,
    /// When true, grassy hexes cost extra in routes and slow the character.
    grass_slows: bool,
    /// The grass brush removes instead of paints while set.
    grass_erase: bool,
    /// Hex radius of the grass brush (`0` = 1 hex, `1` = 7 hexes, `2` = 19, `3` = 37).
    grass_brush_radius: u8,
    /// Density step added/removed per grass brush dab (`1..=GRASS_MAX`).
    grass_brush_strength: u8,
    /// Bumped on every grass change so the tuft visuals rebuild.
    grass_rev: u64,
    /// An open paint stroke: set while the brush is down. The stroke commits
    /// once on press; release pops the undo entry when nothing changed.
    grass_stroke: bool,
    /// Whether the open stroke changed anything yet.
    grass_stroke_dirty: bool,
    /// Last dabbed hex this stroke, so holding still does not repaint.
    grass_dab: Option<Hex>,
    /// Painted procedural 3D vines anchored to surfaces.
    vines: Vec<VineNode>,
    /// The vine brush removes instead of paints while set.
    vine_erase: bool,
    /// Bumped on every vine change so the batched vine meshes rebuild.
    vine_rev: u64,
    /// An open vine paint/erase stroke.
    vine_stroke: bool,
    /// Whether the open vine stroke changed anything yet.
    vine_stroke_dirty: bool,
    /// Last painted vine dab world position in the current stroke.
    vine_last_pos: Option<Vec3>,
    /// Active continuous Tiny Glade terrain brush stroke.
    terrain_stroke: bool,
    /// Whether the open terrain brush stroke modified any terrain yet.
    terrain_stroke_dirty: bool,
    /// Target plateau height locked at the start of the terrain stroke.
    terrain_stroke_target_h: f32,
    /// Last sculpted world XZ position during the current terrain stroke.
    terrain_stroke_last_pos: Option<Vec2>,
    /// One pending platform waypoint from the Motion tool, drained by movers.
    platform_request: Option<(u32, Vec2)>,
}

/// One painted procedural vine anchor on a 3D surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VineNode {
    pub pos: Vec3,
    pub normal: Vec3,
    pub seed: u32,
}

/// One undo step: the level geometry plus its authoring metadata, so grouping
/// and labels undo alongside placement, transforms and deletes. Grass and vines
/// ride along so one undo step removes one paint stroke.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LevelState {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) outline: SceneOutline,
    pub(crate) grass: HashMap<Hex, u8>,
    pub(crate) vines: Vec<VineNode>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            pieces: Vec::new(),
            outline: SceneOutline::default(),
            tool: None,
            actor_tool: None,
            mode: Mode::Build,
            character: None,
            plan: None,
            route: None,
            walk: None,
            selection: Selection::default(),
            clipboard: Vec::new(),
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
            grass: HashMap::new(),
            grass_slows: false,
            grass_erase: false,
            grass_brush_radius: 1,
            grass_brush_strength: 1,
            grass_rev: 0,
            grass_stroke: false,
            grass_stroke_dirty: false,
            grass_dab: None,
            vines: Vec::new(),
            vine_erase: false,
            vine_rev: 0,
            vine_stroke: false,
            vine_stroke_dirty: false,
            vine_last_pos: None,
            terrain_stroke: false,
            terrain_stroke_dirty: false,
            terrain_stroke_target_h: 2.5,
            terrain_stroke_last_pos: None,
            platform_request: None,
        }
    }
}

/// The palette can arm a building block, the character, or one of the path
/// tools (character route, plan, platform motion, or the grass brush).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActorTool {
    Place,
    Path,
    Plan,
    Platform,
    Grass,
    Vine,
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
    ///
    /// Horizontal speed is the character's *own* motion (`hv`), not the
    /// world motion of the platform underfoot: a rider standing still on a
    /// mover is idle, even though `velocity()` (and the status readout)
    /// reports the platform's speed.
    pub(crate) fn locomotion_input(self) -> LocomotionInput {
        LocomotionInput::new(
            self.grounded,
            self.sliding,
            self.hv.length(),
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
    elevation: f32,
    kind: SnapKind,
}

#[derive(Clone)]
struct Drag {
    id: u32,
    offset: Vec2,
    /// Height of the horizontal plane the drag moves in. Fixed at grab time
    /// so the piece tracks the pointer 1:1 instead of sliding with the
    /// ground-plane projection when the piece is elevated.
    plane: f32,
    moved: bool,
    /// The rest of the selection, as `(id, offset)` where the offset is the
    /// piece's position relative to the grabbed piece at grab time. The whole
    /// group keeps its layout while the grabbed piece follows the pointer.
    group: Vec<(u32, Vec2)>,
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
    /// Glade building width / diameter handle.
    GladeWidth,
    /// Glade building depth / wall length handle.
    GladeLength,
    /// Glade wall height handle (controls fence / wall / crenellated tier).
    GladeHeight,
    /// Glade roof pitch handle at the roof apex.
    GladeRoofPitch,
}

#[derive(Clone)]
struct GizmoDrag {
    id: u32,
    handle: GizmoHandle,
    /// World point grabbed when the drag started, so the piece moves by the
    /// pointer's travel rather than jumping to the pointer.
    grab: Vec3,
    /// The grabbed piece at the moment of the grab.
    start: Piece,
    /// The rest of the selection at the moment of the grab. They take the same
    /// delta as the grabbed piece, so one handle moves or rotates them all.
    others: Vec<Piece>,
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

#[derive(Clone, Debug)]
struct KeyboardTransform {
    id: u32,
    mode: TransformMode,
    /// `None` is the free plane for moves and yaw for rotations.
    axis: Option<TransformAxis>,
    /// Every selected piece before the modal began, primary first. Escape or a
    /// right click restores all of them.
    originals: Vec<Piece>,
    /// The primary piece at the last axis change; the pointer moves relative
    /// to this.
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
    /// Set by the shoot button, consumed by the cone-firing system.
    shoot: bool,
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

/// Authoring versus playing. Build owns general level editing; Building owns
/// procedural building authoring and contextual attachments; Play hands the
/// capsule to the player while route editing and view switching stay available.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    #[default]
    Build,
    Building,
    Play,
}

impl Mode {
    fn toggled(self) -> Self {
        match self {
            Mode::Build | Mode::Building => Mode::Play,
            Mode::Play => Mode::Build,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Building => "building",
            Mode::Play => "play",
        }
    }
}

impl Editor {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn capture(&self, orbit: &Orbit, movers: &Movers) -> session::Snapshot {
        let mut grass: Vec<session::SavedGrass> = self
            .grass
            .iter()
            .map(|(&(q, r), &density)| session::SavedGrass { q, r, density })
            .collect();
        grass.sort_by_key(|saved| (saved.q, saved.r));
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
            grass,
            grass_slows: self.grass_slows,
            vines: self.vines.iter().map(session::SavedVine::from).collect(),
        }
    }

    /// Replace the whole local world from a validated snapshot.
    ///
    /// The live world arrives as streamed cells (`editor/world_sync.rs`), which
    /// installs incrementally and preserves session state; this wholesale form
    /// is what level seeding and the editor tests use.
    #[cfg_attr(not(test), allow(dead_code))]
    fn apply_loaded(&mut self, orbit: &mut Orbit, movers: &mut Movers, loaded: session::Loaded) {
        self.pieces = loaded.pieces;
        self.outline = loaded.outline;
        self.outline.bump();
        self.character = loaded.character;
        self.undo = loaded.undo;
        self.redo = loaded.redo;
        self.next_id = loaded.next_id;
        self.grass = loaded.grass;
        self.grass_slows = loaded.grass_slows;
        self.grass_rev = self.grass_rev.wrapping_add(1);
        self.vines = loaded.vines;
        self.vine_rev = self.vine_rev.wrapping_add(1);
        if let Some(camera) = loaded.camera {
            *orbit = camera;
        }
        movers::apply_saved(movers, &self.pieces, &loaded.platforms);

        self.selection.clear();
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
        self.selection.clear();
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
        self.grass.clear();
        self.grass_rev = self.grass_rev.wrapping_add(1);
        self.vines.clear();
        self.vine_rev = self.vine_rev.wrapping_add(1);
        self.hard_dirty = true;
        self.note = "course: walk straight, up the ramp, over the block, drop";
    }

    fn snapshot(&self) -> LevelState {
        LevelState {
            pieces: self.pieces.clone(),
            outline: self.outline.clone(),
            grass: self.grass.clone(),
            vines: self.vines.clone(),
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
            self.grass = prev.grass;
            self.grass_rev = self.grass_rev.wrapping_add(1);
            self.vines = prev.vines;
            self.vine_rev = self.vine_rev.wrapping_add(1);
            // Keep the UI's rebuild counter monotonic across history moves.
            self.outline.revision = revision.wrapping_add(1);
            self.selection.clear();
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
            self.grass = next.grass;
            self.grass_rev = self.grass_rev.wrapping_add(1);
            self.vines = next.vines;
            self.vine_rev = self.vine_rev.wrapping_add(1);
            self.outline.revision = revision.wrapping_add(1);
            self.selection.clear();
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

/// Left-palette section whose visibility depends on the active editor `Mode`
/// and the currently selected object(s).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum PaletteGroup {
    /// General dungeon / level categories + actor tools (`Mode::Build`).
    BuildMode,
    /// Core building tools: Building + Walls & Paths (`Mode::Building`).
    BuildingBase,
    /// Contextual group: Doors & Windows, shown when a building or wall is selected.
    DoorsAndWindows,
    /// Contextual group: Fixtures (e.g. brazier, upcoming lamps), shown when a
    /// building, wall, or surface is selected.
    BuildingFixtures,
}

/// Hint shown in `Mode::Building` when no building/wall/surface is selected.
#[derive(Component)]
struct BuildingSelectionHint;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct SelectionPaletteContext {
    supports_doors_and_windows: bool,
    supports_fixtures: bool,
}

fn selection_palette_context(editor: &Editor) -> SelectionPaletteContext {
    let mut ctx = SelectionPaletteContext::default();
    for id in editor.selection.ids() {
        let Some(piece) = editor.pieces.iter().find(|p| p.id == *id) else {
            continue;
        };
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        if matches!(
            shape.id,
            "glade_room" | "glade_tower" | "glade_wall" | "wall" | "tall_wall" | "arch"
        ) {
            ctx.supports_doors_and_windows = true;
            ctx.supports_fixtures = true;
        } else if matches!(
            shape.id,
            "glade_terrain" | "floor" | "bridge" | "block" | "fence"
        ) {
            ctx.supports_fixtures = true;
        }
    }
    ctx
}

/// Desktop Walk/Smooth row, shown only while Plan mode is active.
#[derive(Component)]
struct PathActions;

/// Desktop grass row (erase + slow toggles), shown while Grass is armed.
#[derive(Component)]
struct GrassActions;

/// Desktop vine row (erase toggle), shown while Vine is armed.
#[derive(Component)]
struct VineActions;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteAction {
    Build,
    Building,
    Play,
    View,
    Shape(usize),
    Character,
    Path,
    Plan,
    Walk,
    Smooth,
    Grass,
    GrassErase,
    GrassSlow,
    GrassSize,
    GrassDensity,
    GrassClear,
    Vine,
    VineErase,
    Course,
    /// Copy the level JSON for debugging. This is not a level edit.
    CopyJson,
    /// Open the native home screen (maps + settings). Native-only UI; the
    /// button is hidden on wasm, where the browser provides the home page.
    Home,
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct PieceVisual;

#[derive(Component)]
struct CharacterVisual;

/// Marker for the main 3D camera (as opposed to the minimap camera).
#[derive(Component)]
pub(crate) struct MainCamera;

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
    Grass,
    GrassErase,
    Add,
    Walk,
    Smooth,
    Place,
    PrevShape,
    NextShape,
    Jump,
    /// Player interaction (E key equivalent). Live in Play, like Jump.
    Interact,
    /// Remappable A/B/C/X/Y/Z cluster button. Resolved through the Controls
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
    /// Cached procedural signature per Glade piece (and ghost 0) so meshes
    /// rebuild reactively when overlapping neighbours, paths, or dimensions change.
    glade_signatures: HashMap<u32, u64>,
    character: Option<Entity>,
    /// The glTF avatar that follows the capsule root.
    character_avatar: Option<Entity>,
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
    /// Projectile mesh, shared by every cone the actor fires.
    cone: Handle<Mesh>,
    cone_mat: Handle<StandardMaterial>,
    character_collider: Collider,
    /// Woody bark material for painted procedural vine stems.
    vine_stem_mat: Handle<StandardMaterial>,
    /// Ivy leaf material for painted procedural vines.
    vine_leaf_mat: Handle<StandardMaterial>,
}

// ---------------------------------------------------------------------------
// Networked local pose
// ---------------------------------------------------------------------------

/// The free play controller's Update system (`character_move`).
///
/// A system set rather than a bare system name so the net layer can order its
/// authoritative-pose correction after the local controller without naming an
/// editor-private system: the correction must run *after* the local collision
/// step it pulls back, and *before* the camera follows the result.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PlayController;

/// The camera-follow system (`camera_control`).
///
/// Anything that moves the local character must land before this so the camera
/// sees the frame's final pose (see [`PlayController`]).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CameraFollow;

/// Apply one authoritative self state (from a snapshot) to the local camera and
/// character.
///
/// Returns the correction that was applied, or `None` when the pose is not
/// network-driven right now:
/// * only [`Mode::Play`] — authoring modes keep their purely local body,
/// * never while walking an authored route ([`Editor::walk`]) — that controller
///   owns the capsule and its camera,
/// * nothing to correct without a placed character.
///
/// The local controller still runs first every frame and keeps its collision
/// feel; this only pulls its result back toward the state every other client
/// sees (small errors blended, teleports snapped — see
/// [`crate::net_client::correct_character_pose`]).
pub(crate) fn apply_authoritative_pose(
    editor: &mut Editor,
    orbit: &mut Orbit,
    state: &PlayerState,
    dt: f32,
) -> Option<crate::net_client::PoseCorrection> {
    if editor.mode != Mode::Play || editor.walk.is_some() {
        return None;
    }
    let character = editor.character.as_mut()?;
    let before = character.pos;
    let correction = crate::net_client::correct_character_pose(character, state, dt);
    let after = character.pos;
    // Follow the correction with the orbit centre so the third-person camera
    // moves with the body instead of the world sliding under it — the same
    // delta-follow `character_move` uses for movement it did not steer. First
    // person derives its eye from `character.pos` directly, and top-down keeps
    // the camera put (the pill visibly moves across the map), so only third
    // person takes the delta.
    if orbit.mode == ViewMode::Third {
        let delta = after - before;
        if delta != Vec2::ZERO {
            orbit.target.x += delta.x;
            orbit.target.z += delta.y;
        }
    }
    Some(correction)
}

// The two world-sync systems are registered by [`EditorPlugin`] but the
// replicon client adapter (a sibling module) must order itself around them:
// it mirrors replicated piece entities into `WorldChunks` before the apply and
// sends the staged edit queue right after the diff.
// Only the replicon adapter orders itself around the streamed-world systems and
// (in its tests) drives the compose/diff directly.
#[cfg(feature = "replicon")]
pub(crate) use world_sync::{
    apply_world_chunks as apply_streamed_world, sync_world_edits as sync_streamed_edits,
};
// The replicon adapter's tests build a `WorldSync` to check the entity-to-editor
// mapping; the plugin itself only orders around the systems above.
#[cfg(all(feature = "replicon", test))]
pub(crate) use world_sync::WorldSync;

/// Replicated authoritative local pose (cargo feature `replicon`).
///
/// The replicon path has no snapshot feed: the server replicates the local
/// player as an entity (`PlayerSub` + `PlayerPose`). `repl_client` decodes it
/// into a [`party_dungeon_net::PlayerState`] stored in
/// [`crate::repl_client::ReplicatedSelfPose`]; this system feeds that into the
/// shared [`apply_authoritative_pose`] path, so the pose maths, snap threshold
/// and third-person camera delta-follow are identical on both transports.
///
/// Registered by `repl_client::ReplClientPlugin` after [`PlayController`] (the
/// local controller) and before [`CameraFollow`], exactly like the legacy
/// `net_client::net_pose_correction`.
#[cfg(feature = "replicon")]
pub(crate) fn replicon_pose_correction(
    mut editor: ResMut<Editor>,
    mut orbit: ResMut<Orbit>,
    pose: Option<Res<crate::repl_client::ReplicatedSelfPose>>,
    client_state: Option<Res<State<bevy_replicon::prelude::ClientState>>>,
    auth_state: Option<Res<crate::auth::AuthState>>,
    time: Res<Time>,
    mut last_snap_log: Local<f64>,
) {
    let signed_in = !crate::auth::auth_required()
        || auth_state
            .as_deref()
            .is_some_and(|state| *state == crate::auth::AuthState::SignedIn);
    if !signed_in {
        return;
    }
    // Only a live replicated session has authoritative state; while
    // connecting/disconnected the local controller owns the pose.
    let connected = client_state
        .as_deref()
        .is_some_and(|state| *state.get() == bevy_replicon::prelude::ClientState::Connected);
    if !connected {
        return;
    }
    let Some(state) = pose.as_deref().and_then(|pose| pose.state.clone()) else {
        return;
    };
    let Some(correction) = apply_authoritative_pose(
        &mut editor,
        &mut orbit,
        &state,
        time.delta_secs(),
    ) else {
        return;
    };
    // One line per second for a visible pop (teleport, respawn, bad
    // prediction), mirroring the legacy path's rate-limited log.
    if correction.snapped {
        let now = time.elapsed_secs_f64();
        if now - *last_snap_log >= 1.0 {
            *last_snap_log = now;
            bevy::log::info!(
                "local pose corrected by {:.2} m (snapped; client build {})",
                correction.distance,
                crate::net_client::CLIENT_BUILD
            );
        }
    }
}

mod add_menu;
pub(crate) mod bridge;
mod camera;
mod character;
/// Public for the headless server simulation (shared collider builders).
pub mod colliders;
mod draw;
mod editing;
mod gizmo;
mod grass;
pub mod hazards;
mod home;
mod hud;
mod input;
mod left_panel;
mod interacts;
pub(crate) mod minimap;
mod movers;
pub(crate) mod outline;
mod outliner;
mod overlay;
mod picking;
mod properties;
mod right_panel;
mod route;
mod save;
mod selection;
mod setup;
mod shooting;
mod touch;
mod transform;
mod vines;
mod world_sync;
mod visuals;
/// Public for the headless server simulation (shared locomotion maths).
pub mod walking;

use add_menu::*;
use bridge::*;
use camera::*;
use colliders::*;
use draw::*;
use editing::*;
use gizmo::*;
use grass::*;
use hazards::*;
use home::*;
use hud::*;
use input::*;
use left_panel::*;
use interacts::*;
use minimap::*;
use movers::*;
use outline::*;
use outliner::*;
use overlay::*;
use picking::*;
use properties::*;
use right_panel::*;
use route::*;
use save::*;
use setup::*;
use shooting::*;
use touch::*;
use transform::*;
use vines::*;
use visuals::*;
use walking::*;

#[cfg(test)]
mod tests;

/// Unit tests for the authoritative local pose (`net_client`'s
/// `net_pose_correction` calling [`apply_authoritative_pose`]). Live in this
/// file because the gating reads `Editor`/`Mode`/`walk`, which are private to
/// the editor module; the pose maths itself is tested in `net_client`.
#[cfg(test)]
mod net_pose_tests {
    use super::*;

    fn character_at(pos: Vec2) -> Character {
        Character {
            pos,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: Health::default(),
        }
    }

    fn self_state(x: f32, y: f32, z: f32, yaw: f32) -> PlayerState {
        PlayerState {
            sub: "me".to_string(),
            name: "Me".to_string(),
            x,
            y,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw,
            hp: 100.0,
            max_hp: 100.0,
            alive: true,
            online: true,
        }
    }

    fn play_editor(pos: Vec2) -> Editor {
        let mut editor = Editor::default();
        editor.mode = Mode::Play;
        editor.character = Some(character_at(pos));
        editor
    }

    #[test]
    fn a_small_error_is_blended_and_the_camera_follows() {
        let mut editor = play_editor(Vec2::ZERO);
        let mut orbit = Orbit::default();
        orbit.mode = ViewMode::Third;
        orbit.target = Vec3::ZERO;
        let state = self_state(0.4, 0.0, -0.2, 0.5);

        let correction = apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0)
            .expect("play mode with a character is corrected");

        assert!(!correction.snapped);
        assert!((correction.distance - 0.4f32.hypot(0.2)).abs() < 1e-5);
        let character = editor.character.expect("character");
        // Blended toward the snapshot, not snapped onto it.
        assert!(
            character.pos.x > 0.0 && character.pos.x < 0.4,
            "pos {:?} should blend toward 0.4",
            character.pos
        );
        assert!(character.pos.y < 0.0 && character.pos.y > -0.2);
        assert_eq!(character.yaw, 0.5, "facing is applied exactly");
        // The camera follows the same delta, so the correction does not slide
        // the world under the camera.
        assert!((orbit.target.x - character.pos.x).abs() < 1e-6);
        assert!((orbit.target.z - character.pos.y).abs() < 1e-6);
    }

    #[test]
    fn a_teleport_snaps_pose_facing_health_and_alive() {
        let mut editor = play_editor(Vec2::new(1.0, 2.0));
        let mut orbit = Orbit::default();
        orbit.mode = ViewMode::Third;
        orbit.target = Vec3::new(1.0, 0.0, 2.0);
        let mut state = self_state(-8.0, 3.5, 4.0, -1.2);
        state.vx = 1.5;
        state.vz = -2.5;
        state.vy = -3.0;
        state.hp = 35.0;
        state.max_hp = 80.0;

        let correction = apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0)
            .expect("corrected");

        assert!(correction.snapped, "a 10 m jump must snap, not blend");
        let character = editor.character.expect("character");
        assert_eq!(character.pos, Vec2::new(-8.0, 4.0));
        assert_eq!(character.feet, 3.5);
        assert_eq!(character.hv, Vec2::new(1.5, -2.5));
        assert_eq!(character.vy, -3.0);
        assert_eq!(character.yaw, -1.2);
        assert_eq!(character.health.current, 35.0);
        assert_eq!(character.health.max, 80.0);
        assert!(character.health.is_alive());
        assert_eq!(
            orbit.target,
            Vec3::new(-8.0, 0.0, 4.0),
            "the camera follows the snapped pose"
        );
    }

    #[test]
    fn a_dead_authoritative_state_kills_the_local_body() {
        let mut editor = play_editor(Vec2::new(2.0, 2.0));
        let mut orbit = Orbit::default();
        let mut state = self_state(2.0, 0.0, 2.0, 0.0);
        state.hp = 0.0;
        state.alive = false;

        apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0).expect("corrected");

        let character = editor.character.expect("character");
        assert_eq!(character.health.current, 0.0);
        assert!(character.health.is_dead());
    }

    #[test]
    fn authoring_modes_and_route_walking_are_left_alone() {
        let state = self_state(9.0, 0.0, -3.0, 0.3);
        let mut orbit = Orbit::default();

        // Build mode: the capsule belongs to the editor, never to the net.
        let mut editor = Editor::default();
        editor.character = Some(character_at(Vec2::ZERO));
        assert!(apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0).is_none());
        assert_eq!(editor.character.expect("character").pos, Vec2::ZERO);

        // Play, but the authored route owns the capsule and camera.
        editor.mode = Mode::Play;
        editor.walk = Some(Walk {
            distance: 2.0,
            stuck: 0.0,
        });
        assert!(apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0).is_none());
        assert_eq!(editor.character.expect("character").pos, Vec2::ZERO);

        // Play without a placed character: nothing to correct.
        editor.walk = None;
        editor.character = None;
        assert!(apply_authoritative_pose(&mut editor, &mut orbit, &state, 1.0 / 60.0).is_none());
    }

    #[test]
    fn the_pose_system_is_inert_without_a_net_client() {
        let mut app = App::new();
        app.init_resource::<Editor>();
        app.init_resource::<Orbit>();
        app.init_resource::<Time>();
        app.init_resource::<crate::auth::AuthState>();
        app.add_systems(Update, crate::net_client::net_pose_correction);
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.mode = Mode::Play;
            editor.character = Some(character_at(Vec2::new(3.0, -1.0)));
        }

        app.update();

        // Without the net plugin there is no NetClient: the local pose is not
        // touched even in Play mode.
        let editor = app.world().resource::<Editor>();
        assert_eq!(
            editor.character.expect("character").pos,
            Vec2::new(3.0, -1.0)
        );
    }
}
