//! Android-only embedded game assets.
//!
//! xbuild's direct-APK path does not bundle an `assets/` directory, so the
//! Android asset manager would have nothing to read. The shipped models are
//! small (character.glb is ~6 MB), so they are compiled into the binary and
//! served from Bevy's in-memory `embedded` source instead — identical bytes to
//! what desktop and web builds read off disk.
use std::path::{Path, PathBuf};

use bevy::asset::io::embedded::EmbeddedAssetRegistry;
use bevy::asset::AssetPath;
use bevy::prelude::App;

/// Asset keys served from the embedded source. These must match the plain
/// relative paths the desktop/web builds load through the file source.
pub const CHARACTER_MODEL: &str = "character.glb";

/// `character.glb` as an `embedded://` path for Android loads.
pub fn character_model_path() -> AssetPath<'static> {
    AssetPath::from_path_buf(PathBuf::from(CHARACTER_MODEL)).with_source("embedded")
}

/// Compile the shipped models into the binary and expose them through the
/// `embedded` asset source. Runs once at app build, after `AssetPlugin`
/// (which owns the registry) is in place.
pub fn register(app: &mut App) {
    app.init_resource::<EmbeddedAssetRegistry>();
    let mut registry = app
        .world_mut()
        .resource_mut::<EmbeddedAssetRegistry>();
    registry.insert_asset(
        PathBuf::from(CHARACTER_MODEL),
        Path::new(CHARACTER_MODEL),
        &include_bytes!("../../../assets/character.glb")[..],
    );
}
