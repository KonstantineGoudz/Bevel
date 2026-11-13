//! A minimal glTF viewer built with Rust + Bevy.
//!
//! Boot with `?viewer` (see `viewer-bevy.html`) to open this instead of the
//! level editor. It can:
//!
//! * preview `.glb` / `.gltf` files dropped onto the window or the page, or
//!   picked through the **Import .glb / .gltf** button (browser file picker on
//!   wasm),
//! * orbit / zoom the camera around the model,
//! * open a **Packaged models** panel on the left listing the `.glb` / `.gltf`
//!   files shipped in `assets/`, so a model bundled with the game can be viewed
//!   without picking it off disk (the web build writes `assets/models.json`,
//!   which `viewer-bevy.html` hands to the panel),
//! * report file name, load state, and scene stats in Bevy UI.
//!
//! Imports go through a `viewer://` in-memory asset source, so picked bytes
//! never touch disk and work the same on native and wasm. Packaged files load
//! through Bevy's normal asset server, relative to the asset root.
//!
//! JPEG textures only decode when Bevy's `jpeg` feature is enabled. glTF only
//! guarantees JPEG and PNG textures, so the crate turns that feature on in
//! `Cargo.toml`; without it, models like `PotOfCoals.glb` fail with
//! `invalid image mime type: image/jpeg`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::asset::LoadState;
use bevy::asset::io::memory::Dir;
use bevy::mesh::VertexAttributeValues;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

const VIEWER_TITLE: &str = "Model viewer";
const SUPPORTED_HINT: &str =
    "Import .glb / .gltf - drag & drop, use Import, or pick a packaged model on the left";
const PANEL_BG: Color = Color::srgb_u8(0x10, 0x16, 0x1e);
const BUTTON_BG: Color = Color::srgb_u8(0x14, 0x1c, 0x26);
const BUTTON_BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);
const BUTTON_TEXT: Color = Color::srgb_u8(0xc3, 0xd0, 0xdf);
const MUTED: Color = Color::srgb_u8(0x86, 0x97, 0xab);
const ROW_BG: Color = Color::srgb_u8(0x16, 0x1e, 0x29);
const ROW_BG_SELECTED: Color = Color::srgb_u8(0x3a, 0x2c, 0x12);
const ROW_BORDER_SELECTED: Color = Color::srgb_u8(0xf0, 0xa6, 0x3c);
const TREE_WIDTH: f32 = 300.0;
const PACKAGED_WIDTH: f32 = 240.0;
/// Top offset clearing the title + buttons + status bar on the left.
const TREE_TOP: f32 = 150.0;
/// Never build more rows than this; huge scenes get a "+ N more" cap note.
const MAX_TREE_ROWS: usize = 1500;
/// `WorldAssetRoot` spawns its children from the `SpawnScene` schedule, which
/// runs after `Update`. Wait this many frames for meshes before falling back to
/// node origins (for scenes that have no meshes at all).
const SPAWN_GRACE_FRAMES: u32 = 30;

/// In-memory filesystem backing the `viewer://` asset source. Registered in
/// `lib.rs` before `DefaultPlugins`; this resource just hands systems the
/// [`Dir`] to insert picked bytes into.
#[derive(Resource, Clone, Default)]
pub struct ViewerDir(pub Dir);

/// One queued import. Rejected files keep their name and a reason so the status
/// line can explain why nothing was loaded.
#[derive(Debug, Clone)]
pub enum ViewerUpload {
    Model { name: String, bytes: Vec<u8> },
    Rejected { name: String, reason: String },
}

/// Files picked outside Bevy (browser picker or page drag & drop on wasm, drag &
/// drop on native) waiting to be inserted into [`ViewerDir`] and loaded.
#[derive(Resource, Clone, Default)]
pub struct PendingUploads(pub Arc<Mutex<Vec<ViewerUpload>>>);

/// `.glb` / `.gltf` files shipped in the asset root (`assets/` on disk and in
/// the web build), in display order. Used by the Packaged models panel.
#[derive(Resource, Default)]
struct PackagedModels(Vec<String>);

#[derive(Resource)]
struct Viewer {
    status: String,
    file_name: Option<String>,
    root: Option<Entity>,
    handle: Option<Handle<bevy::world_serialization::WorldAsset>>,
    framed: bool,
    auto_rotate: bool,
    mesh_count: usize,
    /// Frames spent waiting for the scene spawner to produce meshes.
    polls: u32,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            status: SUPPORTED_HINT.to_owned(),
            file_name: None,
            root: None,
            handle: None,
            framed: true,
            auto_rotate: true,
            mesh_count: 0,
            polls: 0,
        }
    }
}

#[derive(Resource)]
struct ViewerOrbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for ViewerOrbit {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.85,
            pitch: 0.5,
            distance: 6.0,
        }
    }
}

#[derive(Component)]
struct ViewerButton(ViewerAction);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ViewerAction {
    Import,
    ToggleSpin,
    ResetView,
    Back,
    LoadPackaged(usize),
}

#[derive(Component)]
struct ViewerStatus;

/// The teal cube shown before a model loads; removed once one is visible.
#[derive(Component)]
struct ViewerPlaceholder;

#[derive(Component)]
struct PackagedPanel;

#[derive(Component)]
struct PackagedList;

#[derive(Component)]
struct PackagedCount;

/// UI state for the Scene tree. The hierarchy itself is read live from the
/// spawned entities, so this only tracks what is expanded/selected plus a
/// rebuild marker.
#[derive(Resource, Default)]
struct ViewerTree {
    expanded: HashSet<Entity>,
    selected: Option<Entity>,
    /// Entities with children seen on the last rebuild (for Expand all).
    expandable: Vec<Entity>,
    last_root: Option<Entity>,
    last_count: usize,
    dirty: bool,
}

impl ViewerTree {
    fn reset(&mut self) {
        self.expanded.clear();
        self.expandable.clear();
        self.selected = None;
        self.last_root = None;
        self.last_count = 0;
        self.dirty = true;
    }
}

#[derive(Component)]
struct TreePanel;

#[derive(Component)]
struct TreeList;

#[derive(Component)]
struct TreeCount;

#[derive(Component)]
struct TreeRow(Entity);

#[derive(Component)]
struct TreeToggle(Entity);

#[derive(Component)]
struct TreeButton(TreeAction);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TreeAction {
    Toggle(Entity),
    Select(Entity),
    ExpandAll,
    CollapseAll,
}

pub struct ViewerPlugin;

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Viewer>()
            .init_resource::<ViewerOrbit>()
            .init_resource::<PendingUploads>()
            .init_resource::<PackagedModels>()
            .init_resource::<ViewerTree>()
            .insert_resource(crate::controls::Controls::load())
            .add_systems(
                Startup,
                (setup_viewer, setup_packaged_panel, setup_wasm_picker),
            )
            .add_systems(
                Update,
                (
                    drain_file_drops,
                    drain_uploads,
                    frame_when_ready,
                    refresh_tree,
                    orbit_camera,
                    refresh_status,
                )
                    .chain(),
            )
            .add_observer(on_viewer_button)
            .add_observer(on_tree_button);
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup_viewer(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ClearColor(Color::srgb_u8(0x0a, 0x0d, 0x12)));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb_u8(0xb8, 0xc8, 0xe0),
        brightness: 400.0,
        ..default()
    });

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 45.0_f32.to_radians(),
            near: 0.05,
            far: 500.0,
            ..default()
        }),
        Transform::from_xyz(3.5, 2.6, 4.5).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(5.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Ground reference + a placeholder cube so the viewport is never empty.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(24.0, 24.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb_u8(0x14, 0x1a, 0x22),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.51, 0.0),
    ));
    commands.spawn((
        ViewerPlaceholder,
        Mesh3d(meshes.add(Cuboid::new(0.6, 0.6, 0.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb_u8(0x3f, 0xd0, 0xb0),
            perceptual_roughness: 0.4,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.3, 0.0),
    ));

    // Top bar: title + actions + status, all plain Bevy UI (no HTML needed).
    let root = commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            top: px(0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(10)),
            row_gap: px(6),
            ..default()
        })
        .id();

    let title = commands
        .spawn((
            Text::new(format!("{VIEWER_TITLE} - Bevy (Rust)")),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();

    let row = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            ..default()
        })
        .id();
    let buttons = [
        viewer_button(&mut commands, ViewerAction::Import, "Import .glb / .gltf"),
        viewer_button(&mut commands, ViewerAction::ToggleSpin, "Spin: on"),
        viewer_button(&mut commands, ViewerAction::ResetView, "Reset view"),
        viewer_button(&mut commands, ViewerAction::Back, "Back to maps"),
    ];
    commands.entity(row).add_children(&buttons);

    let status = commands
        .spawn((
            ViewerStatus,
            Text::new(SUPPORTED_HINT),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(MUTED),
            Node {
                max_width: px(720),
                ..default()
            },
            BackgroundColor(PANEL_BG),
        ))
        .id();

    commands.entity(root).add_children(&[title, row, status]);

    spawn_tree_panel(&mut commands);
}

fn spawn_tree_panel(commands: &mut Commands) {
    let panel = commands
        .spawn((
            TreePanel,
            Node {
                position_type: PositionType::Absolute,
                right: px(8),
                top: px(TREE_TOP),
                bottom: px(8),
                width: px(TREE_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10)),
                row_gap: px(6),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BUTTON_BORDER),
        ))
        .id();

    let header = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Baseline,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        })
        .id();
    let heading = commands
        .spawn((
            Text::new("Scene"),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    let count = commands
        .spawn((
            TreeCount,
            Text::new("no model"),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    commands.entity(header).add_children(&[heading, count]);

    let toolbar = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(6),
            ..default()
        })
        .id();
    let expand = tree_action_button(commands, TreeAction::ExpandAll, "Expand all");
    let collapse = tree_action_button(commands, TreeAction::CollapseAll, "Collapse");
    commands.entity(toolbar).add_children(&[expand, collapse]);

    let list = commands
        .spawn((
            TreeList,
            ScrollPosition::default(),
            Node {
                width: percent(100),
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();

    let hint = commands
        .spawn((
            Text::new("Every item in the .glb, top to bottom. Click a row to focus it."),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x5b, 0x6a, 0x7d)),
            Pickable::IGNORE,
        ))
        .id();

    commands
        .entity(panel)
        .add_children(&[header, toolbar, list, hint]);
}

// ---------------------------------------------------------------------------
// Packaged models
// ---------------------------------------------------------------------------

/// Every `.glb` / `.gltf` file shipped in the asset root. On wasm the list
/// comes from `assets/models.json` (written by `scripts/build-web.sh`) through
/// a global that `viewer-bevy.html` sets before booting Bevy; on native the
/// asset folder is read directly.
fn packaged_models() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_packaged_models()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native_packaged_models()
    }
}

/// Normalize a manifest/file-system path to a safe asset path: forward
/// slashes, relative to the asset root, and ending in `.glb` / `.gltf`.
fn normalize_packaged_model(raw: &str) -> Option<String> {
    let path = raw.trim().replace('\\', "/");
    if path.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            other => parts.push(other),
        }
    }
    let normalized = parts.join("/");
    if normalized.is_empty() {
        return None;
    }
    let lower = normalized.to_lowercase();
    (lower.ends_with(".glb") || lower.ends_with(".gltf")).then_some(normalized)
}

/// Sort for display and drop exact duplicates. Case-insensitive so
/// `Hero.glb` and `hero.glb` sit next to each other.
fn finish_model_list(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = models.into_iter().collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out.dedup();
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn native_packaged_models() -> Vec<String> {
    let root = std::path::PathBuf::from(crate::asset_root());
    let mut models = Vec::new();
    collect_packaged_models(&root, &root, 0, &mut models);
    finish_model_list(models)
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_packaged_models(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    // Guard against a symlink loop in a hand-made asset folder.
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_packaged_models(root, &path, depth + 1, out);
        } else if kind.is_file()
            && let Ok(relative) = path.strip_prefix(root)
            && let Some(name) = normalize_packaged_model(&relative.to_string_lossy())
        {
            out.push(name);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_packaged_models() -> Vec<String> {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return Vec::new();
    };
    let Ok(value) = js_sys::Reflect::get(
        window.unchecked_ref::<wasm_bindgen::JsValue>(),
        &wasm_bindgen::JsValue::from_str("__packagedModels"),
    ) else {
        return Vec::new();
    };
    let Ok(list) = value.dyn_into::<js_sys::Array>() else {
        return Vec::new();
    };
    finish_model_list(
        list.iter()
            .filter_map(|item| item.as_string())
            .filter_map(|name| normalize_packaged_model(&name)),
    )
}

fn packaged_count_label(count: usize) -> String {
    match count {
        0 => "none".to_owned(),
        1 => "1 model".to_owned(),
        total => format!("{total} models"),
    }
}

/// One clickable row in the Packaged models panel.
fn packaged_button(commands: &mut Commands, index: usize, path: &str) -> Entity {
    commands
        .spawn((
            Button,
            ViewerButton(ViewerAction::LoadPackaged(index)),
            Node {
                width: percent(100),
                padding: UiRect::axes(px(8), px(6)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BUTTON_BORDER),
            children![(
                Text::new(path),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(BUTTON_TEXT),
            )],
        ))
        .id()
}

/// Left-hand list of models that ship with the build. It mirrors the Scene
/// panel on the right: a heading, a scrollable list, and a one-line hint.
fn setup_packaged_panel(mut commands: Commands) {
    let models = packaged_models();
    let panel = commands
        .spawn((
            PackagedPanel,
            Node {
                position_type: PositionType::Absolute,
                left: px(8),
                top: px(TREE_TOP),
                bottom: px(8),
                width: px(PACKAGED_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(10)),
                row_gap: px(6),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            BorderColor::all(BUTTON_BORDER),
        ))
        .id();

    let header = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Baseline,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        })
        .id();
    let heading = commands
        .spawn((
            Text::new("Packaged"),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .id();
    let count = commands
        .spawn((
            PackagedCount,
            Text::new(packaged_count_label(models.len())),
            TextFont {
                font_size: FontSize::Px(11.0),
                ..default()
            },
            TextColor(MUTED),
        ))
        .id();
    commands.entity(header).add_children(&[heading, count]);

    let list = commands
        .spawn((
            PackagedList,
            ScrollPosition::default(),
            Node {
                width: percent(100),
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();

    if models.is_empty() {
        let empty = commands
            .spawn((
                Text::new(
                    "No .glb / .gltf files in assets/.\nAdd a model, then rebuild the web build.",
                ),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
            ))
            .id();
        commands.entity(list).add_child(empty);
    } else {
        for (index, path) in models.iter().enumerate() {
            let row = packaged_button(&mut commands, index, path);
            commands.entity(list).add_child(row);
        }
    }

    let hint = commands
        .spawn((
            Text::new("Models shipped in assets/. Click one to view it."),
            TextFont {
                font_size: FontSize::Px(10.0),
                ..default()
            },
            TextColor(Color::srgb_u8(0x5b, 0x6a, 0x7d)),
            Pickable::IGNORE,
        ))
        .id();

    commands.entity(panel).add_children(&[header, list, hint]);
    commands.insert_resource(PackagedModels(models));
}

fn tree_action_button(commands: &mut Commands, action: TreeAction, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            TreeButton(action),
            Node {
                flex_grow: 1.0,
                flex_basis: px(0),
                padding: UiRect::axes(px(8), px(4)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BUTTON_BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(BUTTON_TEXT),
            )],
        ))
        .id()
}

fn viewer_button(commands: &mut Commands, action: ViewerAction, text: &str) -> Entity {
    commands
        .spawn((
            Button,
            ViewerButton(action),
            Node {
                padding: UiRect::axes(px(14), px(8)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(8)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BUTTON_BG),
            BorderColor::all(BUTTON_BORDER),
            children![(
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(BUTTON_TEXT),
            )],
        ))
        .id()
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

fn on_viewer_button(
    event: On<Activate>,
    mut commands: Commands,
    buttons: Query<(&ViewerButton, &Children)>,
    mut labels: Query<&mut Text>,
    mut viewer: ResMut<Viewer>,
    mut orbit: ResMut<ViewerOrbit>,
    mut tree: ResMut<ViewerTree>,
    packaged: Res<PackagedModels>,
    asset_server: Res<AssetServer>,
    _uploads: Res<PendingUploads>,
) {
    let Ok((button, children)) = buttons.get(event.entity) else {
        return;
    };
    match button.0 {
        ViewerAction::Import => {
            #[cfg(target_arch = "wasm32")]
            trigger_browser_picker();
            #[cfg(not(target_arch = "wasm32"))]
            {
                viewer.status = "Pick a file: drag & drop a .glb/.gltf onto this window (native file dialogs need a display server; drop works everywhere)".to_owned();
            }
        }
        ViewerAction::ToggleSpin => {
            viewer.auto_rotate = !viewer.auto_rotate;
            for child in children.iter() {
                if let Ok(mut text) = labels.get_mut(child) {
                    text.0 = format!("Spin: {}", if viewer.auto_rotate { "on" } else { "off" });
                }
            }
        }
        ViewerAction::ResetView => {
            *orbit = ViewerOrbit::default();
        }
        ViewerAction::Back => {
            #[cfg(target_arch = "wasm32")]
            go_back_to_maps();
            #[cfg(not(target_arch = "wasm32"))]
            {
                viewer.status =
                    "Back: open the maps page in your browser (this is the desktop build)"
                        .to_owned();
            }
        }
        ViewerAction::LoadPackaged(index) => {
            let Some(path) = packaged.0.get(index).cloned() else {
                return;
            };
            show_scene(
                &mut commands,
                &asset_server,
                &mut viewer,
                &mut tree,
                &path,
                path.clone(),
                format!("Loading packaged \"{path}\"..."),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Import plumbing
// ---------------------------------------------------------------------------

/// Keep the original extension (the glTF loader keys off it) while making the
/// in-memory path safe and unique per upload.
fn safe_asset_name(original: &str, salt: usize) -> String {
    let ext = Path::new(original)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .filter(|e| e == ".glb" || e == ".gltf")
        .unwrap_or_else(|| ".glb".to_owned());
    format!("upload-{salt}{ext}")
}

fn is_model_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".glb") || lower.ends_with(".gltf")
}

/// Replace the current scene with one loaded from `asset_path` (an asset-root
/// path such as `character.glb`, or a `viewer://` import). Shared by imports
/// and packaged models so both get the same framing, status, and tree reset.
fn show_scene(
    commands: &mut Commands,
    asset_server: &AssetServer,
    viewer: &mut Viewer,
    tree: &mut ViewerTree,
    display_name: &str,
    asset_path: String,
    status: String,
) {
    if let Some(old) = viewer.root.take() {
        commands.entity(old).despawn();
    }
    let handle: Handle<bevy::world_serialization::WorldAsset> =
        asset_server.load(bevy::gltf::GltfAssetLabel::Scene(0).from_asset(asset_path));
    let root = commands
        .spawn((
            bevy::world_serialization::WorldAssetRoot(handle.clone()),
            Transform::IDENTITY,
        ))
        .id();
    viewer.root = Some(root);
    viewer.handle = Some(handle);
    viewer.framed = false;
    viewer.polls = 0;
    viewer.file_name = Some(display_name.to_owned());
    viewer.mesh_count = 0;
    viewer.status = status;
    // A new file means a new hierarchy: drop the old expansion/selection so
    // the tree rebuilds from the fresh scene graph.
    tree.reset();
}

/// Native drag & drop: `MessageReader<FileDragAndDrop>` fires for files
/// dropped onto the window. Read the bytes here and queue them like a picker
/// result so wasm and native share one load path.
fn drain_file_drops(mut drops: MessageReader<FileDragAndDrop>, uploads: Res<PendingUploads>) {
    for event in drops.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = event else {
            continue;
        };
        let name = path_buf
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "dropped.glb".to_owned());
        let upload = if is_model_file(&name) {
            match std::fs::read(path_buf) {
                Ok(bytes) => ViewerUpload::Model { name, bytes },
                Err(error) => ViewerUpload::Rejected {
                    name,
                    reason: format!("could not be read: {error}"),
                },
            }
        } else {
            ViewerUpload::Rejected {
                name,
                reason: "is not a .glb/.gltf file - drop a model file instead".to_owned(),
            }
        };
        if let Ok(mut queue) = uploads.0.lock() {
            queue.push(upload);
        }
    }
}

/// Move queued uploads into the `viewer://` memory source and spawn a
/// [`WorldAssetRoot`] for scene 0. Old models are despawned first.
fn drain_uploads(
    mut commands: Commands,
    dir: Res<ViewerDir>,
    uploads: Res<PendingUploads>,
    asset_server: Res<AssetServer>,
    mut viewer: ResMut<Viewer>,
    mut tree: ResMut<ViewerTree>,
    mut counter: Local<usize>,
) {
    let pending: Vec<ViewerUpload> = uploads
        .0
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default();
    for upload in pending {
        let (name, bytes) = match upload {
            ViewerUpload::Rejected { name, reason } => {
                viewer.status = format!("\"{name}\" {reason}");
                continue;
            }
            ViewerUpload::Model { name, bytes } => (name, bytes),
        };
        *counter += 1;
        let asset_name = safe_asset_name(&name, *counter);
        dir.0.insert_asset(Path::new(&asset_name), bytes.clone());
        show_scene(
            &mut commands,
            &asset_server,
            &mut viewer,
            &mut tree,
            &name,
            format!("viewer://{asset_name}"),
            format!("Loading \"{name}\" ({} bytes)...", bytes.len()),
        );
    }
}

/// Once the `WorldAsset` reports `Loaded`, centre the orbit target on the
/// model's geometry so both tiny and huge models land in view.
///
/// Node origins are not enough: glTF keeps nodes at the origin surprisingly
/// often and stores the size in the vertex data (`PotOfCoals.glb` does exactly
/// that), so the bounds come from transformed mesh vertices. Node origins are
/// only the fallback for scenes without meshes.
fn frame_when_ready(
    mut commands: Commands,
    mut viewer: ResMut<Viewer>,
    asset_server: Res<AssetServer>,
    mut orbit: ResMut<ViewerOrbit>,
    children: Query<&Children>,
    draws: Query<(&Mesh3d, &GlobalTransform)>,
    transforms: Query<&GlobalTransform>,
    meshes: Res<Assets<Mesh>>,
    skinned: Query<(), With<SkinnedMesh>>,
    placeholders: Query<Entity, With<ViewerPlaceholder>>,
) {
    if viewer.framed {
        return;
    }
    let Some(handle) = viewer.handle.clone() else {
        return;
    };
    match asset_server.load_state(&handle) {
        LoadState::Loaded => {
            // The model is visible now; the teal stand-in cube would only
            // sit inside it.
            for entity in &placeholders {
                commands.entity(entity).despawn();
            }
        }
        LoadState::Failed(error) => {
            let name = viewer.file_name.as_deref().unwrap_or("model").to_owned();
            // Surface the loader's actual reason: "no JPEG support" and "not a
            // glTF file" used to look identical to the user.
            bevy::log::error!("Could not load \"{name}\": {error}");
            viewer.status = format!("Could not load \"{name}\": {error}");
            viewer.framed = true;
            return;
        }
        LoadState::NotLoaded | LoadState::Loading => return,
    }
    let Some(root) = viewer.root else {
        viewer.framed = true;
        return;
    };
    let (from_meshes, from_nodes) =
        scene_bounds(root, &children, &draws, &transforms, &meshes, &skinned);
    if from_meshes.is_empty() {
        // The scene spawner runs after `Update`, so meshes show up a frame or
        // two later. Only give up on them once the grace window expires.
        viewer.polls += 1;
        if viewer.polls < SPAWN_GRACE_FRAMES {
            return;
        }
    }
    let nodes = from_nodes.nodes;
    let bounds = if from_meshes.is_empty() {
        from_nodes
    } else {
        from_meshes
    };
    if bounds.is_empty() {
        viewer.framed = true;
        return;
    }
    orbit.target = bounds.center();
    // `radius` is the bounding sphere; `1 / sin(fov/2)` would touch the
    // viewport edge, so add a little margin and keep the whole model visible.
    orbit.distance = (bounds.radius().max(0.05) * 2.6).clamp(0.05, 400.0);
    viewer.mesh_count = bounds.meshes;
    viewer.framed = true;
    viewer.status = format!(
        "Viewing \"{}\" - {} meshes, {} nodes | right-drag to orbit | wheel to zoom",
        viewer.file_name.as_deref().unwrap_or("model"),
        bounds.meshes,
        nodes,
    );
}

/// Accumulates world-space bounds and counts while walking a spawned scene.
#[derive(Debug, Clone, Copy)]
struct SceneBounds {
    min: Vec3,
    max: Vec3,
    meshes: usize,
    nodes: usize,
}

impl Default for SceneBounds {
    fn default() -> Self {
        Self {
            min: Vec3::splat(f32::MAX),
            max: Vec3::splat(f32::MIN),
            meshes: 0,
            nodes: 0,
        }
    }
}

impl SceneBounds {
    fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    fn add_point(&mut self, point: Vec3) {
        if !point.is_finite() {
            return;
        }
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// Add a mesh's local AABB, transformed into world space.
    fn add_mesh(&mut self, min: Vec3, max: Vec3, global: &GlobalTransform) {
        let transform = global.affine();
        for corner in corners(min, max) {
            self.add_point(transform.transform_point3(corner));
        }
        self.meshes += 1;
    }

    fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    fn radius(&self) -> f32 {
        ((self.max - self.min) * 0.5).length()
    }
}

fn corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

/// Local-space min/max of the mesh positions, or `None` when the mesh has no
/// usable position data.
fn mesh_extents(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for [x, y, z] in positions {
        let point = Vec3::new(*x, *y, *z);
        if !point.is_finite() {
            continue;
        }
        min = min.min(point);
        max = max.max(point);
    }
    (min.x <= max.x).then_some((min, max))
}

/// Bounds of everything under `root`, once by mesh vertices and once by node
/// origins, so the caller can prefer geometry and still handle empty scenes.
fn scene_bounds(
    root: Entity,
    children: &Query<&Children>,
    draws: &Query<(&Mesh3d, &GlobalTransform)>,
    transforms: &Query<&GlobalTransform>,
    meshes: &Assets<Mesh>,
    skinned: &Query<(), With<SkinnedMesh>>,
) -> (SceneBounds, SceneBounds) {
    let mut from_meshes = SceneBounds::default();
    let mut from_nodes = SceneBounds::default();
    let mut stack: Vec<Entity> = vec![root];
    // Guards against pathological scene graphs; real models are far smaller.
    let mut visited = 0usize;
    while let Some(entity) = stack.pop() {
        visited += 1;
        if visited > 20_000 {
            break;
        }
        if let Ok(global) = transforms.get(entity) {
            from_nodes.nodes += 1;
            from_nodes.add_point(global.translation());
        }
        if let Ok((draw, global)) = draws.get(entity)
            && let Some((min, max)) = meshes.get(&draw.0).and_then(mesh_extents)
        {
            // A skinned mesh is drawn from its joint palette, not from this
            // entity's transform. glTF exporters commonly put a -90° scene
            // rotation on the node above the skin, so transforming the
            // bind-pose AABB by it aims the camera at the character's feet.
            // The local bind-pose bounds are the honest ones.
            let transform = if skinned.contains(entity) {
                GlobalTransform::IDENTITY
            } else {
                *global
            };
            from_meshes.add_mesh(min, max, &transform);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    (from_meshes, from_nodes)
}

// ---------------------------------------------------------------------------
// Scene tree
// ---------------------------------------------------------------------------

/// Kind badge shown in front of every tree row.
fn node_kind(has_mesh: bool, has_light: bool, has_camera: bool) -> &'static str {
    if has_mesh {
        "Mesh"
    } else if has_light {
        "Light"
    } else if has_camera {
        "Camera"
    } else {
        "Node"
    }
}

/// Human label for a row. glTF nodes without names still get a stable,
/// kind-prefixed fallback so the tree never shows a blank row.
fn tree_label(raw: Option<&str>, kind: &str, fallback_id: impl std::fmt::Display) -> String {
    let name = raw.map(str::trim).filter(|n| !n.is_empty());
    match name {
        Some(name) => format!("{kind}  {name}"),
        None => format!("{kind}  #{fallback_id}"),
    }
}

/// Depth-first visible rows for a forest. `children` maps a node id to its
/// ordered children; only descendants of expanded nodes are emitted.
/// Returns `(id, depth, has_children)` in display order.
fn flatten_tree(
    roots: &[usize],
    children: &HashMap<usize, Vec<usize>>,
    expanded: &HashSet<usize>,
) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, usize)> = roots.iter().rev().map(|id| (*id, 0)).collect();
    while let Some((id, depth)) = stack.pop() {
        let kids = children.get(&id).cloned().unwrap_or_default();
        out.push((id, depth, !kids.is_empty()));
        if expanded.contains(&id) {
            for kid in kids.iter().rev() {
                stack.push((*kid, depth + 1));
            }
        }
    }
    out
}

fn on_tree_button(
    event: On<Activate>,
    buttons: Query<&TreeButton>,
    mut tree: ResMut<ViewerTree>,
    mut viewer: ResMut<Viewer>,
    mut orbit: ResMut<ViewerOrbit>,
    transforms: Query<&GlobalTransform>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    match button.0 {
        TreeAction::Toggle(entity) => {
            if !tree.expanded.remove(&entity) {
                tree.expanded.insert(entity);
            }
            tree.dirty = true;
        }
        TreeAction::Select(entity) => {
            tree.selected = Some(entity);
            tree.dirty = true;
            if let Ok(global) = transforms.get(entity) {
                orbit.target = global.translation();
            }
            viewer.status = format!(
                "Selected {} - right-drag to orbit around it",
                entity_label(entity, &transforms),
            );
        }
        TreeAction::ExpandAll => {
            let all = tree.expandable.clone();
            tree.expanded.extend(all);
            tree.dirty = true;
        }
        TreeAction::CollapseAll => {
            tree.expanded.clear();
            tree.dirty = true;
        }
    }
}

fn entity_label(entity: Entity, transforms: &Query<&GlobalTransform>) -> String {
    // Names are resolved at rebuild time; the status line only needs something
    // stable, so fall back to the entity index here.
    let _ = transforms;
    format!("item {}", entity.index())
}

/// Rebuild the tree rows whenever the scene graph changes (new model, spawner
/// progress) or the user expands/collapses/selects. The hierarchy is read live
/// from `Children`, so the panel always mirrors the loaded .glb.
#[allow(clippy::too_many_arguments)]
fn refresh_tree(
    mut commands: Commands,
    viewer: Res<Viewer>,
    mut tree: ResMut<ViewerTree>,
    children: Query<&Children>,
    names: Query<&Name>,
    mesh_marks: Query<(), With<Mesh3d>>,
    light_marks: Query<(), Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>>,
    camera_marks: Query<(), With<Camera>>,
    lists: Query<Entity, With<TreeList>>,
    rows: Query<Entity, Or<(With<TreeRow>, With<TreeToggle>)>>,
    mut counts: Query<&mut Text, With<TreeCount>>,
) {
    let Ok(list) = lists.single() else {
        return;
    };
    let Some(root) = viewer.root else {
        if tree.last_root.is_some() || tree.last_count != 0 {
            for row in &rows {
                commands.entity(row).despawn();
            }
            tree.reset();
            for mut text in &mut counts {
                text.0 = "no model".to_owned();
            }
        }
        return;
    };

    // Walk the live graph: entity -> ordered children, plus kind/name lookups.
    let mut child_map: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut order: Vec<Entity> = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<Entity> = children
        .get(root)
        .map(|kids| kids.iter().rev().collect())
        .unwrap_or_default();
    while let Some(entity) = stack.pop() {
        visited += 1;
        if visited > 20_000 {
            break;
        }
        order.push(entity);
        if let Ok(kids) = children.get(entity) {
            let kids: Vec<Entity> = kids.iter().collect();
            if !kids.is_empty() {
                child_map.insert(entity, kids.clone());
                for kid in kids.iter().rev() {
                    stack.push(*kid);
                }
            }
        }
    }

    let total = order.len();
    if !tree.dirty && tree.last_root == Some(root) && tree.last_count == total {
        return;
    }

    // First build after a load: auto-expand the top levels (or everything for
    // small scenes) so the tree is useful without any clicking.
    if tree.last_root != Some(root) {
        tree.expanded.clear();
        if total <= 300 {
            tree.expanded.extend(child_map.keys().copied());
        } else {
            // Depth-limited expansion for huge scenes.
            let mut depth_of: HashMap<Entity, usize> = HashMap::new();
            let mut depth_stack: Vec<(Entity, usize)> = children
                .get(root)
                .map(|kids| kids.iter().map(|e| (e, 0)).collect())
                .unwrap_or_default();
            while let Some((entity, depth)) = depth_stack.pop() {
                depth_of.insert(entity, depth);
                if depth < 2 {
                    if child_map.contains_key(&entity) {
                        tree.expanded.insert(entity);
                    }
                    if let Some(kids) = child_map.get(&entity) {
                        for kid in kids {
                            depth_stack.push((*kid, depth + 1));
                        }
                    }
                }
            }
        }
    }
    tree.expandable = child_map.keys().copied().collect();

    for row in &rows {
        commands.entity(row).despawn();
    }

    // Visible rows: depth-first, descending only into expanded nodes.
    let mut visible: Vec<(Entity, usize, bool)> = Vec::new();
    let mut dfs: Vec<(Entity, usize)> = children
        .get(root)
        .map(|kids| kids.iter().rev().map(|e| (e, 0)).collect())
        .unwrap_or_default();
    while let Some((entity, depth)) = dfs.pop() {
        let kids = child_map.get(&entity).cloned().unwrap_or_default();
        visible.push((entity, depth, !kids.is_empty()));
        if visible.len() >= MAX_TREE_ROWS {
            break;
        }
        if tree.expanded.contains(&entity) {
            for kid in kids.iter().rev() {
                dfs.push((*kid, depth + 1));
            }
        }
    }
    let capped = total > visible.len();

    let mut row_entities = Vec::with_capacity(visible.len());
    for (entity, depth, has_children) in visible {
        row_entities.push(spawn_tree_row(
            &mut commands,
            entity,
            depth,
            has_children,
            tree.expanded.contains(&entity),
            tree.selected == Some(entity),
            names.get(entity).ok().map(|n| n.as_str()),
            node_kind(
                mesh_marks.contains(entity),
                light_marks.contains(entity),
                camera_marks.contains(entity),
            ),
        ));
    }
    commands.entity(list).add_children(&row_entities);

    if capped {
        let more = commands
            .spawn((
                Text::new(format!(
                    "... {} more items (collapse branches to browse)",
                    total - row_entities.len()
                )),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
            ))
            .id();
        commands.entity(list).add_child(more);
    }

    if total == 0 {
        let empty = commands
            .spawn((
                Text::new("No items under the scene root."),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(MUTED),
            ))
            .id();
        commands.entity(list).add_child(empty);
    }

    for mut text in &mut counts {
        text.0 = if total == 1 {
            "1 item".to_owned()
        } else {
            format!("{total} items")
        };
    }

    tree.last_root = Some(root);
    tree.last_count = total;
    tree.dirty = false;
}

#[allow(clippy::too_many_arguments)]
fn spawn_tree_row(
    commands: &mut Commands,
    target: Entity,
    depth: usize,
    has_children: bool,
    is_expanded: bool,
    is_selected: bool,
    raw_name: Option<&str>,
    kind: &str,
) -> Entity {
    let label = tree_label(raw_name, kind, target.index());
    let row = commands
        .spawn((
            TreeRow(target),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(4),
                padding: UiRect {
                    left: px(6.0 + depth as f32 * 14.0),
                    right: px(4),
                    top: px(2),
                    bottom: px(2),
                },
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(if is_selected { ROW_BG_SELECTED } else { ROW_BG }),
            BorderColor::all(if is_selected {
                ROW_BORDER_SELECTED
            } else {
                BUTTON_BORDER
            }),
        ))
        .id();

    let mut parts = Vec::with_capacity(2);
    if has_children {
        let toggle = commands
            .spawn((
                Button,
                TreeButton(TreeAction::Toggle(target)),
                TreeToggle(target),
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
                BorderColor::all(BUTTON_BORDER),
                children![(
                    Text::new(if is_expanded { "-" } else { "+" }),
                    TextFont {
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(BUTTON_TEXT),
                )],
            ))
            .id();
        parts.push(toggle);
    } else {
        let spacer = commands
            .spawn(Node {
                width: px(20),
                height: px(20),
                ..default()
            })
            .id();
        parts.push(spacer);
    }

    let pick = commands
        .spawn((
            Button,
            TreeButton(TreeAction::Select(target)),
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
            children![(
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(if is_selected {
                    Color::WHITE
                } else {
                    BUTTON_TEXT
                }),
            )],
        ))
        .id();
    parts.push(pick);

    commands.entity(row).add_children(&parts);
    row
}

// ---------------------------------------------------------------------------
// Camera + HUD
// ---------------------------------------------------------------------------

fn orbit_camera(
    time: Res<Time>,
    input: crate::controls::PlayerInput,
    mut motions: MessageReader<bevy::input::mouse::MouseMotion>,
    mut wheels: MessageReader<bevy::input::mouse::MouseWheel>,
    mut viewer: ResMut<Viewer>,
    mut orbit: ResMut<ViewerOrbit>,
    cameras: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    if input.just_pressed(crate::controls::Action::ViewerReset) {
        *orbit = ViewerOrbit::default();
    }
    let mut delta = Vec2::ZERO;
    for motion in motions.read() {
        // Right-drag orbits (left stays free for UI clicks).
        if input.pressed(crate::controls::Action::OrbitDrag) {
            delta += motion.delta;
        }
    }
    if delta != Vec2::ZERO {
        orbit.yaw -= delta.x * 0.008;
        orbit.pitch = (orbit.pitch + delta.y * 0.008).clamp(-1.4, 1.4);
    }
    for wheel in wheels.read() {
        orbit.distance = (orbit.distance * (1.0 - wheel.y * 0.12)).clamp(0.4, 200.0);
    }
    if viewer.auto_rotate {
        orbit.yaw += time.delta_secs() * 0.35;
        // Keep the status honest when spinning with no model yet.
        if viewer.root.is_none() {
            viewer.mesh_count = 0;
        }
    }
    let eye = orbit.target
        + Vec3::new(
            orbit.pitch.cos() * orbit.yaw.sin(),
            orbit.pitch.sin(),
            orbit.pitch.cos() * orbit.yaw.cos(),
        ) * orbit.distance;
    for (mut transform, mut projection) in cameras {
        *transform = Transform::from_translation(eye).looking_at(orbit.target, Vec3::Y);
        // Keep clipping sane as the user zooms between millimetre and
        // kilometre-scale models.
        if let Projection::Perspective(perspective) = &mut *projection {
            perspective.near = (orbit.distance * 0.002).clamp(0.001, 1.0);
            perspective.far = (orbit.distance * 40.0).max(100.0);
        }
    }
}

fn refresh_status(viewer: Res<Viewer>, mut texts: Query<&mut Text, With<ViewerStatus>>) {
    if !viewer.is_changed() {
        return;
    }
    let mut extra = String::new();
    if let Some(name) = &viewer.file_name
        && !viewer.status.contains(name.as_str())
    {
        extra = format!("  |  {name}");
    }
    for mut text in &mut texts {
        text.0 = format!("{}{extra}", viewer.status);
    }
}

// ---------------------------------------------------------------------------
// Browser glue (wasm only)
// ---------------------------------------------------------------------------

/// Hidden `<input type=file>` created once from Rust so the Bevy **Import**
/// button opens the OS picker even though the canvas owns the page. Also
/// installs page-level drag & drop: winit has no browser drop events, so the
/// hint in the UI would otherwise be a lie on wasm.
#[cfg(target_arch = "wasm32")]
fn setup_wasm_picker(uploads: Res<PendingUploads>) {
    use wasm_bindgen::JsCast;
    use web_sys::HtmlInputElement;

    install_drop_target(&uploads.0);

    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if document.get_element_by_id("bevy-gltf-input").is_some() {
        return;
    }
    let Ok(element) = document.create_element("input") else {
        return;
    };
    let Ok(input) = element.dyn_into::<HtmlInputElement>() else {
        return;
    };
    input.set_id("bevy-gltf-input");
    input.set_type("file");
    input.set_accept(".glb,.gltf");
    input.set_attribute("style", "display:none").ok();
    let queue = uploads.0.clone();
    let callback = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        read_wasm_input_into_queue(&queue);
    }) as Box<dyn FnMut()>);
    input.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    document.body().map(|body| body.append_child(&input));
}

#[cfg(not(target_arch = "wasm32"))]
fn setup_wasm_picker(_uploads: Res<PendingUploads>) {}

#[cfg(target_arch = "wasm32")]
fn trigger_browser_picker() {
    use wasm_bindgen::JsCast;
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(element) = document.get_element_by_id("bevy-gltf-input") else {
        return;
    };
    let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.click();
}

#[cfg(target_arch = "wasm32")]
fn read_wasm_input_into_queue(queue: &Arc<Mutex<Vec<ViewerUpload>>>) {
    use wasm_bindgen::JsCast;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(element) = document.get_element_by_id("bevy-gltf-input") else {
        return;
    };
    let Ok(input) = element.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    let Some(files) = input.files() else {
        return;
    };
    for index in 0..files.length() {
        if let Some(file) = files.get(index) {
            read_wasm_file(queue, file);
        }
    }
    input.set_value("");
}

/// Reads one browser `File` into the shared queue. Shared by the file picker
/// and by drag & drop so both behave identically.
#[cfg(target_arch = "wasm32")]
fn read_wasm_file(queue: &Arc<Mutex<Vec<ViewerUpload>>>, file: web_sys::File) {
    use wasm_bindgen::JsCast;

    let name = file.name();
    if !is_model_file(&name) {
        if let Ok(mut pending) = queue.lock() {
            pending.push(ViewerUpload::Rejected {
                name,
                reason: "is not a .glb/.gltf file - pick a model file instead".to_owned(),
            });
        }
        return;
    }
    let Ok(reader) = web_sys::FileReader::new() else {
        return;
    };
    let reader_clone = reader.clone();
    let queue = queue.clone();
    let callback = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        use js_sys::Uint8Array;
        let bytes: Vec<u8> = reader_clone
            .result()
            .ok()
            .and_then(|value| {
                (!value.is_null() && !value.is_undefined())
                    .then(|| Uint8Array::new(&value).to_vec())
            })
            .unwrap_or_default();
        if let Ok(mut pending) = queue.lock() {
            if bytes.is_empty() {
                pending.push(ViewerUpload::Rejected {
                    name: name.clone(),
                    reason: "could not be read - the browser returned no bytes".to_owned(),
                });
            } else {
                pending.push(ViewerUpload::Model {
                    name: name.clone(),
                    bytes,
                });
            }
        }
    }) as Box<dyn FnMut()>);
    reader.set_onload(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    reader.read_as_array_buffer(&file).ok();
}

/// HTML5 drag & drop. winit does not translate browser drops into
/// `FileDragAndDrop`, so the viewer listens on the window itself and feeds the
/// same in-memory upload queue the picker uses.
#[cfg(target_arch = "wasm32")]
fn install_drop_target(queue: &Arc<Mutex<Vec<ViewerUpload>>>) {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return;
    };

    let on_dragover = wasm_bindgen::closure::Closure::wrap(Box::new(|event: web_sys::DragEvent| {
        // The browser refuses to fire `drop` unless `dragover` cancels.
        event.prevent_default();
        if let Some(data) = event.data_transfer() {
            data.set_drop_effect("copy");
        }
    })
        as Box<dyn FnMut(web_sys::DragEvent)>);
    window
        .add_event_listener_with_callback("dragover", on_dragover.as_ref().unchecked_ref())
        .ok();
    on_dragover.forget();

    let queue = queue.clone();
    let on_drop = wasm_bindgen::closure::Closure::wrap(Box::new(move |event: web_sys::DragEvent| {
        event.prevent_default();
        let Some(files) = event.data_transfer().and_then(|data| data.files()) else {
            return;
        };
        for index in 0..files.length() {
            if let Some(file) = files.get(index) {
                read_wasm_file(&queue, file);
            }
        }
    })
        as Box<dyn FnMut(web_sys::DragEvent)>);
    window
        .add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref())
        .ok();
    on_drop.forget();
}

#[cfg(target_arch = "wasm32")]
fn go_back_to_maps() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href("./");
    }
}

// ---------------------------------------------------------------------------
// Tests (pure logic, no window needed)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_extensions_are_detected_case_insensitively() {
        assert!(is_model_file("mech.GLB"));
        assert!(is_model_file("level.gltf"));
        assert!(!is_model_file("texture.png"));
        assert!(!is_model_file("model.bin"));
        assert!(!is_model_file("no-extension"));
    }

    #[test]
    fn packaged_model_paths_are_normalized_and_safe() {
        assert_eq!(
            normalize_packaged_model("character.glb").as_deref(),
            Some("character.glb")
        );
        assert_eq!(
            normalize_packaged_model("./art/hero.GLB").as_deref(),
            Some("art/hero.GLB")
        );
        assert_eq!(
            normalize_packaged_model("props\\crate.gltf").as_deref(),
            Some("props/crate.gltf")
        );
        assert_eq!(normalize_packaged_model("notes.txt"), None);
        assert_eq!(normalize_packaged_model("../escape.glb"), None);
        assert_eq!(normalize_packaged_model("/absolute/model.glb"), None);
        assert_eq!(normalize_packaged_model("   "), None);
    }

    #[test]
    fn packaged_model_list_is_sorted_and_keeps_one_copy() {
        let models = finish_model_list([
            "Hero.glb".to_owned(),
            "character.glb".to_owned(),
            "Hero.glb".to_owned(),
            "art/hero.glb".to_owned(),
        ]);
        assert_eq!(models, vec!["art/hero.glb", "character.glb", "Hero.glb"]);
    }

    #[test]
    fn packaged_count_reads_like_a_list_label() {
        assert_eq!(packaged_count_label(0), "none");
        assert_eq!(packaged_count_label(1), "1 model");
        assert_eq!(packaged_count_label(3), "3 models");
    }

    #[test]
    fn asset_names_keep_the_loader_extension_and_stay_unique() {
        assert_eq!(safe_asset_name("Mech.GLB", 2), "upload-2.glb");
        assert_eq!(safe_asset_name("level.gltf", 7), "upload-7.gltf");
        // Unknown extensions fall back to .glb so the loader still runs.
        assert_eq!(safe_asset_name("blob.bin", 1), "upload-1.glb");
    }

    #[test]
    fn viewer_boots_with_helpful_status() {
        let viewer = Viewer::default();
        assert!(viewer.status.contains(".glb"));
        assert!(viewer.auto_rotate);
        assert!(viewer.root.is_none());
    }

    #[test]
    fn mesh_extents_cover_the_vertex_positions() {
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0, 0.0, 0.0], [2.0, 1.0, -1.0], [1.0, 3.0, 0.5]],
        );

        let (min, max) = mesh_extents(&mesh).expect("the mesh has positions");
        assert_eq!(min, Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(max, Vec3::new(2.0, 3.0, 0.5));
    }

    #[test]
    fn scene_bounds_transform_meshes_into_world_space() {
        let mut bounds = SceneBounds::default();
        assert!(bounds.is_empty());

        let transform = GlobalTransform::from_translation(Vec3::new(10.0, 2.0, 0.0));
        bounds.add_mesh(Vec3::splat(-1.0), Vec3::splat(1.0), &transform);

        assert_eq!(bounds.meshes, 1);
        assert_eq!(bounds.center(), Vec3::new(10.0, 2.0, 0.0));
        // Half the diagonal of a 2x2x2 box.
        assert!((bounds.radius() - 3.0_f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn meshes_at_the_origin_still_get_real_bounds() {
        // PotOfCoals keeps every node at the origin and puts the size in the
        // vertices, so framing must not rely on node positions.
        let mut nodes = SceneBounds::default();
        nodes.add_point(Vec3::ZERO);
        nodes.add_point(Vec3::ZERO);
        assert!(nodes.radius() < 1e-6);

        let mut meshes = SceneBounds::default();
        meshes.add_mesh(
            Vec3::splat(-1.5),
            Vec3::splat(1.5),
            &GlobalTransform::IDENTITY,
        );
        assert!((meshes.radius() - 3.0 * 3.0_f32.sqrt() * 0.5).abs() < 1e-5);
    }

    #[test]
    fn tree_kinds_prefer_mesh_over_light_and_camera() {
        assert_eq!(node_kind(true, true, true), "Mesh");
        assert_eq!(node_kind(false, true, true), "Light");
        assert_eq!(node_kind(false, false, true), "Camera");
        assert_eq!(node_kind(false, false, false), "Node");
    }

    #[test]
    fn tree_labels_never_come_back_blank() {
        assert_eq!(tree_label(Some("Prop"), "Mesh", 3), "Mesh  Prop");
        assert_eq!(tree_label(Some("  "), "Node", 7), "Node  #7");
        assert_eq!(tree_label(None, "Light", 9), "Light  #9");
    }

    fn sample_forest() -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
        // 0 -> [1, 2], 1 -> [3]; 2 and 3 are leaves.
        let roots = vec![0];
        let children: HashMap<usize, Vec<usize>> =
            [(0, vec![1, 2]), (1, vec![3])].into_iter().collect();
        (roots, children)
    }

    #[test]
    fn collapsed_branches_hide_their_descendants() {
        let (roots, children) = sample_forest();
        let expanded: HashSet<usize> = [0].into_iter().collect();
        assert_eq!(
            flatten_tree(&roots, &children, &expanded),
            vec![(0, 0, true), (1, 1, true), (2, 1, false)]
        );
    }

    #[test]
    fn expanded_branches_show_depth_first() {
        let (roots, children) = sample_forest();
        let expanded: HashSet<usize> = [0, 1].into_iter().collect();
        assert_eq!(
            flatten_tree(&roots, &children, &expanded),
            vec![(0, 0, true), (1, 1, true), (3, 2, false), (2, 1, false)]
        );
    }

    #[test]
    fn a_fully_collapsed_tree_shows_only_roots() {
        let (roots, children) = sample_forest();
        assert_eq!(
            flatten_tree(&roots, &children, &HashSet::new()),
            vec![(0, 0, true)]
        );
    }
}
