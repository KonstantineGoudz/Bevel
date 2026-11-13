//! Party Dungeon — top-down 3D multiplayer party crawler.

use bevy::asset::AssetApp;
use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use bevy::prelude::*;
use bevy_rapier3d::prelude::{NoUserData, RapierPhysicsPlugin};

pub mod controls;
mod controls_codes;
mod editor;
pub mod glade;
pub mod gltf_export;
pub mod grass_material;
pub mod locomotion;
mod mesh;
pub mod model;
pub mod path;
mod path_edit;
pub mod platforms;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub mod session;
pub mod statuses;
mod viewer;

/// Entry point for every target. `bevy_main` generates the Android glue.
#[bevy_main]
pub fn main() {
    let boot = match session::boot() {
        Ok(boot) => boot,
        Err(error) => {
            #[cfg(target_arch = "wasm32")]
            session::show_error(&error);
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("{error}");
            return;
        }
    };
    let viewer_mode = boot.viewer;

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
    app.insert_resource(boot)
        .insert_resource(viewer::ViewerDir(viewer_dir))
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: asset_root(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: if viewer_mode {
                            "Party Dungeon — model viewer".into()
                        } else {
                            "Party Dungeon — editor".into()
                        },
                        canvas: Some("#bevy".into()),
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                }),
        );
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
    let source_assets = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets"
    ));
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
        app.add_plugins(editor::EditorPlugin);
    }
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
        assert_eq!(
            exe_relative_asset_root_from(&release),
            Some("../../assets")
        );
        assert_eq!(
            exe_relative_asset_root_from(&triple),
            Some("../../../assets")
        );
        assert_eq!(exe_relative_asset_root_from(&near), Some("assets"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
