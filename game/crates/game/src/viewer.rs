//! A minimal glTF viewer built with Rust + Bevy.
//!
//! Boot with `?viewer` (see `viewer-bevy.html`) to open this instead of the
//! level editor. It can:
//!
//! * preview `.glb` / `.gltf` files dropped onto the window or the page, or
//!   picked through the **Import .glb / .gltf** button (browser file picker on
//!   wasm),
//! * orbit / zoom the camera around the model,
//! * report file name, load state, and scene stats in Bevy UI.
//!
//! Imports go through a `viewer://` in-memory asset source, so picked bytes
//! never touch disk and work the same on native and wasm.
//!
//! JPEG textures only decode when Bevy's `jpeg` feature is enabled. glTF only
//! guarantees JPEG and PNG textures, so the crate turns that feature on in
//! `Cargo.toml`; without it, models like `PotOfCoals.glb` fail with
//! `invalid image mime type: image/jpeg`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::asset::LoadState;
use bevy::asset::io::memory::Dir;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, Button};

const VIEWER_TITLE: &str = "Model viewer";
const SUPPORTED_HINT: &str =
    "Import .glb / .gltf - drag & drop a file onto this window, or use the Import button";
const PANEL_BG: Color = Color::srgb_u8(0x10, 0x16, 0x1e);
const BUTTON_BG: Color = Color::srgb_u8(0x14, 0x1c, 0x26);
const BUTTON_BORDER: Color = Color::srgb_u8(0x24, 0x30, 0x3f);
const BUTTON_TEXT: Color = Color::srgb_u8(0xc3, 0xd0, 0xdf);
const MUTED: Color = Color::srgb_u8(0x86, 0x97, 0xab);
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
}

#[derive(Component)]
struct ViewerStatus;

pub struct ViewerPlugin;

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Viewer>()
            .init_resource::<ViewerOrbit>()
            .init_resource::<PendingUploads>()
            .add_systems(Startup, (setup_viewer, setup_wasm_picker))
            .add_systems(
                Update,
                (
                    drain_file_drops,
                    drain_uploads,
                    frame_when_ready,
                    orbit_camera,
                    refresh_status,
                )
                    .chain(),
            )
            .add_observer(on_viewer_button);
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
    buttons: Query<(&ViewerButton, &Children)>,
    mut labels: Query<&mut Text>,
    mut viewer: ResMut<Viewer>,
    mut orbit: ResMut<ViewerOrbit>,
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
        let handle: Handle<bevy::world_serialization::WorldAsset> = asset_server.load(
            bevy::gltf::GltfAssetLabel::Scene(0).from_asset(format!("viewer://{asset_name}")),
        );
        if let Some(old) = viewer.root.take() {
            commands.entity(old).despawn();
        }
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
        viewer.file_name = Some(name.clone());
        viewer.mesh_count = 0;
        viewer.status = format!("Loading \"{name}\" ({} bytes)...", bytes.len());
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
    mut viewer: ResMut<Viewer>,
    asset_server: Res<AssetServer>,
    mut orbit: ResMut<ViewerOrbit>,
    children: Query<&Children>,
    draws: Query<(&Mesh3d, &GlobalTransform)>,
    transforms: Query<&GlobalTransform>,
    meshes: Res<Assets<Mesh>>,
) {
    if viewer.framed {
        return;
    }
    let Some(handle) = viewer.handle.clone() else {
        return;
    };
    match asset_server.load_state(&handle) {
        LoadState::Loaded => {}
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
    let (from_meshes, from_nodes) = scene_bounds(root, &children, &draws, &transforms, &meshes);
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
            from_meshes.add_mesh(min, max, global);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    (from_meshes, from_nodes)
}

// ---------------------------------------------------------------------------
// Camera + HUD
// ---------------------------------------------------------------------------

fn orbit_camera(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut motions: MessageReader<bevy::input::mouse::MouseMotion>,
    mut wheels: MessageReader<bevy::input::mouse::MouseWheel>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut viewer: ResMut<Viewer>,
    mut orbit: ResMut<ViewerOrbit>,
    cameras: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    if keyboard.just_pressed(KeyCode::KeyR) {
        *orbit = ViewerOrbit::default();
    }
    let mut delta = Vec2::ZERO;
    for motion in motions.read() {
        // Right-drag orbits (left stays free for UI clicks).
        if mouse.pressed(MouseButton::Right) {
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
}
