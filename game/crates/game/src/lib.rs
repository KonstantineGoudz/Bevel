//! Party Dungeon — top-down 3D multiplayer party crawler.

use bevy::prelude::*;

/// Entry point for every target. `bevy_main` generates the Android glue.
#[bevy_main]
pub fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Party Dungeon".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(GamePlugin)
        .run();
}

/// Everything the game adds to the default app.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, _app: &mut App) {}
}
