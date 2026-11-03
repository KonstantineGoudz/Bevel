//! Party Dungeon — top-down 3D multiplayer party crawler.

use bevy::asset::AssetApp;
use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use bevy::prelude::*;
use bevy_rapier3d::prelude::{NoUserData, RapierPhysicsPlugin};

pub mod controls;
mod controls_codes;
mod editor;
pub mod locomotion;
mod mesh;
mod model;
mod path;
mod path_edit;
mod platforms;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod session;
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
        .add_plugins(DefaultPlugins.set(WindowPlugin {
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
        }));
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

/// Everything the game adds to the default app.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(editor::EditorPlugin);
    }
}
