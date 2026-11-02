//! The viewer loads imported bytes through a `viewer://` in-memory asset
//! source, then spawns scene 0 as a [`WorldAsset`]. This test drives exactly
//! that path with a tiny JPEG-textured glTF.
//!
//! glTF only guarantees JPEG and PNG textures, and Bevy gates JPEG behind the
//! `jpeg` feature. If that feature is dropped from `Cargo.toml`, this test
//! fails with `invalid image mime type: image/jpeg` — which is how a real
//! model (e.g. Khronos' `PotOfCoals.glb`) failed to load in the viewer.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::gltf::{GltfAssetLabel, GltfPlugin};
use bevy::image::ImagePlugin;
use bevy::mesh::MeshPlugin;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldSerializationPlugin};

const FIXTURE: &[u8] = include_bytes!("fixtures/jpeg_cube.glb");

fn viewer_app() -> (App, Dir) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let dir = Dir::default();
    let reader = dir.clone();
    app.register_asset_source(
        "viewer",
        AssetSourceBuilder::new(move || {
            Box::new(MemoryAssetReader {
                root: reader.clone(),
            })
        }),
    );
    app.add_plugins((
        AssetPlugin::default(),
        ImagePlugin::default(),
        MeshPlugin,
        GltfPlugin::default(),
        WorldSerializationPlugin,
    ));
    app.finish();
    app.cleanup();
    (app, dir)
}

#[test]
fn a_jpeg_textured_glb_loads_through_the_viewer_asset_path() {
    let (mut app, dir) = viewer_app();
    dir.insert_asset(Path::new("fixture.glb"), FIXTURE.to_vec());

    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<WorldAsset> =
        server.load(GltfAssetLabel::Scene(0).from_asset("viewer://fixture.glb"));

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(error) => panic!("glTF scene failed to load: {error}"),
            LoadState::NotLoaded | LoadState::Loading => {}
        }
        assert!(Instant::now() < deadline, "timed out loading the fixture");
        std::thread::sleep(Duration::from_millis(5));
    }

    let images = app.world().resource::<Assets<Image>>();
    assert!(
        images
            .iter()
            .any(|(_, image)| image.width() == 8 && image.height() == 8),
        "the embedded JPEG should decode into an 8x8 texture"
    );

    let worlds = app.world().resource::<Assets<WorldAsset>>();
    let world = &worlds.get(&handle).expect("scene asset exists").world;
    assert!(
        world.entities().len() >= 2,
        "scene should spawn the node and its mesh, got {} entities",
        world.entities().len()
    );
}
