//! Party Dungeon — top-down 3D multiplayer party crawler.

use avian3d::prelude::PhysicsPlugins;
use bevy::prelude::*;

mod editor;
mod mesh;
mod model;
mod path;
mod path_edit;
mod platforms;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod session;
pub mod statuses;

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

    App::new()
        .insert_resource(boot)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Party Dungeon — editor".into(),
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((PhysicsPlugins::default(), GamePlugin))
        .run();
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
