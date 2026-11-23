//! Party Dungeon — top-down 3D multiplayer party crawler.

use bevy::asset::{AssetApp, AssetMetaCheck};
use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowPlugin};
use bevy_rapier3d::prelude::{NoUserData, RapierPhysicsPlugin};

#[cfg(target_os = "android")]
mod android_clipboard;
#[cfg(target_os = "android")]
mod android_assets;
#[cfg(target_os = "android")]
mod android_ime;
#[cfg(target_os = "android")]
mod android_link;
#[cfg(target_os = "android")]
mod android_input;
pub mod auth;
pub mod controls;
mod controls_codes;
mod editor;// Re-exported for the headless server simulation, which shares the exact
// collider builder, locomotion maths, and tuning constants. The `editor`
// module itself stays private; only these items gain a public path.
pub mod sim_support {    pub use crate::editor::colliders::piece_collider_for_glade;
    pub use crate::editor::hazards::{trigger_at, DAMAGE_COOLDOWN_SECS};
    pub use crate::editor::walking::{
        air_step, ground_step, jump_velocity, slide_step, MAX_GROUNDED_RISE, MIN_GROUND_NORMAL,
        PUSH_FACTOR, SLIDE_END_SPEED, SLIDE_MIN_ENTRY_SPEED, STEP_DOWN, STEP_UP,
    };
    pub use crate::editor::{
        GRAVITY, JUMP_SPEED, PILL_CYLINDER, PILL_HALF, PILL_RADIUS, WALK_SPEED,
    };
}
pub mod glade;
pub mod gltf_export;
pub mod headless;
pub mod locomotion;
pub mod net_client;
pub mod remote_players;
/// `bevy_replicon` client path (cargo feature `replicon`), replacing the
/// hand-written snapshot/chunk transport while keeping prediction, the
/// authoritative pose correction, the editor world sync and the avatar visuals.
#[cfg(feature = "replicon")]
pub mod repl_client;
mod mesh;
pub mod model;
pub mod path;
mod path_edit;
pub mod platforms;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub mod session;
pub mod statuses;
mod viewer;

/// Window setup for the normal client. Headless mode has no primary window at
/// all (so nothing to focus, resize, or close) and must not treat the missing
/// window as "all windows closed" — that would exit on the first frame.
fn window_plugin(headless: bool, viewer: bool) -> WindowPlugin {
    let primary_window = (!headless).then(|| Window {
        title: if viewer {
            "Party Dungeon — model viewer".into()
        } else {
            "Party Dungeon — editor".into()
        },
        canvas: Some("#bevy".into()),
        fit_canvas_to_parent: true,
        ..default()
    });
    WindowPlugin {
        primary_window,
        exit_condition: if headless {
            ExitCondition::DontExit
        } else {
            ExitCondition::OnAllClosed
        },
        ..default()
    }
}

/// Entry point for every target. `bevy_main` generates the Android glue.
#[bevy_main]
pub fn main() {
    let boot = session::boot();
    let viewer_mode = boot.viewer;
    // Headless GPU mode: no window/event loop, but the full renderer (see
    // `headless.rs`). The world still arrives from the sim over the net and
    // the live MCP bridge keeps working, so the game can be driven without a
    // browser.
    let headless = boot.headless.clone();

    // In-memory `viewer://` source for imported .glb/.gltf bytes. Asset
    // sources must be registered before `AssetPlugin` (inside
    // `DefaultPlugins`), so this lives here rather than in the viewer plugin.
    let viewer_dir = Dir::default();
    let reader_dir = viewer_dir.clone();

    let mut app = App::new();
    app.register_asset_source(
        "viewer",
        AssetSourceBuilder::new(move || {
            Box::new(MemoryAssetReader {
                root: reader_dir.clone(),
            })
        }),
    );
    let mut plugins = DefaultPlugins
        .set(AssetPlugin {
            file_path: asset_root(),
            // Nothing in this project ships `.meta` files, and Bevy
            // checks for one beside *every* asset by default. On web
            // that is an extra round trip per asset and a console full
            // of `/assets/character.glb.meta 404`, which reads like a
            // broken deploy. Loading defaults are what we want anyway.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(window_plugin(headless.is_some(), viewer_mode));
    if headless.is_some() {
        // Winit panics outright in environments without a display server, and
        // headless mode never creates a window or runs an event loop. Dropping
        // the plugin also removes its app runner; `HeadlessPlugin` installs
        // `ScheduleRunnerPlugin::run_loop` in its place.
        plugins = plugins.disable::<bevy::winit::WinitPlugin>();
    }
    app.insert_resource(boot)
        .insert_resource(viewer::ViewerDir(viewer_dir))
        .add_plugins(plugins);
    if let Some(config) = headless {
        app.add_plugins(headless::HeadlessPlugin::new(config));
    }
    if viewer_mode {
        // The glTF viewer needs no physics and no editor state.
        app.add_plugins(viewer::ViewerPlugin);
    } else {
        app.add_plugins((RapierPhysicsPlugin::<NoUserData>::default(), GamePlugin));
    }
    app.run();
}

/// wasm entry point. `#[bevy_main]` only wires Android; a cdylib has no
/// `main` for wasm-bindgen to call, so export one explicitly.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn wasm_start() {
    main();
}

/// Read a string global the host page put on `window`.
///
/// Browsers have no env vars, so build-time configuration that differs per
/// deployment (signaling endpoint, auth API origin) is injected by `game.html`
/// and read here. Native builds keep using `std::env`, so these return `None`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn web_global(name: &str) -> Option<String> {
    let window = web_sys::window()?;
    js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str(name))
        .ok()?
        .as_string()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn web_global(_name: &str) -> Option<String> {
    None
}

/// Bevy's asset root. Bundles (web, Android, iOS) use the default `assets/`.
///
/// On native, Bevy resolves `AssetPlugin::file_path` against the executable's
/// directory (`target/release/assets`), not the process working directory. So
/// look for assets next to the executable first, then for the development
/// checkout's `game/assets/` a few levels up from `target/<profile>/` or
/// `target/<triple>/<profile>/`. Shipped apps that put `assets/` beside the
/// executable keep the default.
pub(crate) fn asset_root() -> String {
    let desktop = cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ));
    if !desktop {
        return "assets".to_owned();
    }
    if let Some(root) = exe_relative_asset_root() {
        return root.to_owned();
    }
    // Last development fallback: the source checkout, independent of where
    // Cargo placed the executable or what the current directory is.
    let source_assets = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    if source_assets.is_dir() {
        return source_assets.to_string_lossy().into_owned();
    }
    "assets".to_owned()
}

/// Pick a path relative to the executable, if one of the usual layouts exists.
fn exe_relative_asset_root() -> Option<&'static str> {
    let exe = std::env::current_exe().ok()?;
    exe_relative_asset_root_from(exe.parent()?)
}

fn exe_relative_asset_root_from(dir: &std::path::Path) -> Option<&'static str> {
    let near = dir.join("assets").is_dir();
    let up_two = dir.join("../../assets").is_dir();
    let up_three = dir.join("../../../assets").is_dir();
    (near || up_two || up_three).then(|| pick_asset_root(near, up_two, up_three))
}

/// Best executable-relative asset path. `assets` is a shipped layout; the
/// `../..` variants are Cargo's `target/<profile>` and
/// `target/<triple>/<profile>` folders, where `game/assets` sits two or three
/// levels above the executable.
fn pick_asset_root(near: bool, up_two: bool, up_three: bool) -> &'static str {
    if near {
        "assets"
    } else if up_two {
        "../../assets"
    } else if up_three {
        "../../../assets"
    } else {
        "assets"
    }
}

/// Everything the game adds to the default app.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // GPU grass needs the renderer (compute shaders). Headless/unit-test
        // apps built on `MinimalPlugins` skip it; the grass components below
        // are inert data without the plugin.
        if app.is_plugin_added::<bevy::render::RenderPlugin>() {
            app.add_plugins(
                bevy_grass::prelude::GrassPlugins
                    .build()
                    .disable::<bevy_grass::prelude::FarGrassPlugin>()
                    .set(bevy_grass::prelude::GrassPlugin {
                        settings: bevy_grass::prelude::GrassSettings::from(
                            bevy_grass::prelude::GrassQuality::Medium,
                        ),
                        ..default()
                    }),
            );
        }
        // Android has no deploy-time assets/ directory (xbuild does not bundle
        // one), so the shipped models are compiled in and served from the
        // in-memory `embedded` source instead. No-op everywhere else.
        #[cfg(target_os = "android")]
        crate::android_assets::register(app);
        // TEMPORARY grass diagnostics for the Adreno-840 "no grass" report
        // (remove once the Samsung issue is understood).
        #[cfg(target_os = "android")]
        app.add_systems(Update, android_grass_diagnostics);
        // TEMPORARY Android input diagnostics (remove once login touch is fixed).
        // TEMPORARY Android input diagnostics (remove with android_input::log_android_input).
        #[cfg(target_os = "android")]
        app.add_systems(Update, crate::android_input::log_android_input);
        // TEMPORARY Android input diagnostics (remove with android_input).
        #[cfg(target_os = "android")]
        {
            app.add_observer(crate::android_input::log_pointer_down);
            app.add_observer(crate::android_input::log_pointer_up);
            app.add_observer(crate::android_input::log_pointer_click);
        }
        app.add_plugins(editor::EditorPlugin);
        app.add_plugins(auth::AuthPlugin);
        // Transport: the hand-written matchbox snapshot/chunk protocol is the
        // default; the `replicon` feature swaps in bevy_replicon replication
        // (same matchbox/WebRTC transport) plus its own avatar system.
        #[cfg(not(feature = "replicon"))]
        {
            app.add_plugins(net_client::NetClientPlugin);
            // Other players' avatars: reads the snapshots `NetClientPlugin`
            // stores.
            app.add_plugins(remote_players::RemotePlayersPlugin);
        }
        #[cfg(feature = "replicon")]
        {
            app.add_plugins(repl_client::ReplClientPlugin);
        }
    }
}

/// TEMPORARY: say, once every two seconds, what bevy_grass thinks it is doing.
///
/// The device report is "terrain draws, grass does not", with no grass warning
/// at all — so this prints the support verdict, the CPU tables, the GPU
/// readback counters and the field entities themselves. Remove with the
/// `Update` registration in [`GamePlugin`] once the Samsung case is fixed.
#[cfg(target_os = "android")]
fn android_grass_diagnostics(
    support: Option<Res<bevy_grass::prelude::GrassSupport>>,
    stats: Option<Res<bevy_grass::prelude::GrassStats>>,
    fields: Query<(
        Entity,
        &GlobalTransform,
        &bevy_grass::prelude::GrassField,
        Option<&ViewVisibility>,
    )>,
    mut lift_fields: Query<&mut Transform, With<bevy_grass::prelude::GrassField>>,
    mut hide_meshes: Query<&mut Visibility, With<Mesh3d>>,
    cameras: Query<(&GlobalTransform, Option<&bevy_grass::prelude::GrassCamera>), With<Camera3d>>,
    time: Res<Time>,
    mut clear: ResMut<ClearColor>,
    mut next_log: Local<f64>,
    mut last_stage: Local<u32>,
) {
    let now = time.elapsed_secs_f64();
    if now < *next_log {
        return;
    }
    *next_log = now + 2.0;
    let support = match support.as_deref() {
        Some(support) => support.to_string(),
        None => "absent".to_string(),
    };
    // Staged experiments (temporary, with the diagnostics themselves):
    //   t>20 s: lift the grass field 4 m — hidden blades would float above the
    //           terrain, which tells a placement/heightmap bug from a draw bug.
    //   t>40 s: hide every `Mesh3d` (terrain, character) — grass alone on the
    //           sky shows whether it rasterizes at all.
    let stage = (now / 20.0) as u32;
    if stage > *last_stage {
        *last_stage = stage;
        if stage == 1 {
            for mut transform in &mut lift_fields {
                transform.translation.y += 4.0;
                bevy::log::info!(
                    "grass diag: lifted the grass field to y={:.1}",
                    transform.translation.y
                );
            }
        } else if stage == 2 {
            for mut visibility in &mut hide_meshes {
                *visibility = Visibility::Hidden;
            }
            bevy::log::info!("grass diag: hid all Mesh3d entities");
        } else if stage == 3 {
            // Grass drawn but shaded black would hide against the dark clear
            // colour; magenta makes any fragment obvious.
            *clear = ClearColor(Color::srgb(1.0, 0.0, 1.0));
            bevy::log::info!("grass diag: clear colour set to magenta");
        }
    }
    let stats = stats.as_deref();
    let tables = stats.map(|s| s.tables).unwrap_or_default();
    let gpu = stats.map(|s| s.gpu).unwrap_or_default();
    let fields: Vec<String> = fields
        .iter()
        .map(|(entity, transform, field, visible)| {
            let pos = transform.translation();
            format!(
                "{entity}@({:.0},{:.0},{:.0}) size={:?} visible={:?}",
                pos.x,
                pos.y,
                pos.z,
                field.size,
                visible.copied()
            )
        })
        .collect();
    let cameras: Vec<String> = cameras
        .iter()
        .map(|(transform, grass_camera)| {
            let pos = transform.translation();
            format!(
                "({:.0},{:.0},{:.0}) grass_camera={}",
                pos.x,
                pos.y,
                pos.z,
                grass_camera.is_some()
            )
        })
        .collect();
    bevy::log::info!(
        "grass diag: support=[{support}] tables(fields={} chunks={} rebuilds={} layout_rebuilds={} last={:?}) gpu(readings={} blades={:?} slices={} overflows={}) fields={:?} cameras={:?}",
        tables.fields,
        tables.chunks,
        tables.rebuilds,
        tables.layout_rebuilds,
        tables.last_rebuild,
        gpu.readings,
        gpu.blades,
        gpu.slices,
        gpu.overflows,
        fields,
        cameras
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_asset_root_prefers_near_assets_then_the_checkout() {
        assert_eq!(pick_asset_root(true, true, true), "assets");
        // `game/target/release/party-dungeon`: `../../assets` is `game/assets`.
        assert_eq!(pick_asset_root(false, true, true), "../../assets");
        // `game/target/x86_64-.../release`: three levels up.
        assert_eq!(pick_asset_root(false, false, true), "../../../assets");
        assert_eq!(pick_asset_root(false, false, false), "assets");
    }

    #[test]
    fn executable_relative_roots_match_cargo_layouts() {
        let root = std::env::temp_dir().join(format!(
            "party-dungeon-assets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let release = root.join("game/target/release");
        let triple = root.join("game/target/x86_64-pc-windows-msvc/release");
        let near = root.join("shipped");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(&triple).unwrap();
        std::fs::create_dir_all(&near.join("assets")).unwrap();
        std::fs::create_dir_all(root.join("game/assets")).unwrap();

        // The exact failure from the release log: target/release -> game/assets.
        assert_eq!(exe_relative_asset_root_from(&release), Some("../../assets"));
        assert_eq!(
            exe_relative_asset_root_from(&triple),
            Some("../../../assets")
        );
        assert_eq!(exe_relative_asset_root_from(&near), Some("assets"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
