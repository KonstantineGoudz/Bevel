//! The shipped character model must load through Bevy's regular asset path and
//! contain the clips the locomotion wiring looks up by name.
//!
//! The test points Bevy's asset root at `game/assets/`, the same folder the web
//! build copies to `web/assets/` and native runs read by default. A missing or
//! renamed file therefore fails here instead of silently leaving the player as
//! a capsule placeholder.

use std::time::{Duration, Instant};

use bevy::animation::AnimationPlugin;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::gltf::{Gltf, GltfAssetLabel, GltfNode, GltfPlugin};
use bevy::image::ImagePlugin;
use bevy::mesh::MeshPlugin;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldSerializationPlugin};

/// `game/assets`, relative to this crate (`game/crates/game`).
const ASSET_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

fn asset_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins((
        AssetPlugin {
            file_path: ASSET_ROOT.to_string(),
            ..default()
        },
        ImagePlugin::default(),
        MeshPlugin,
        AnimationPlugin,
        GltfPlugin::default(),
        WorldSerializationPlugin,
    ));
    app.finish();
    app.cleanup();
    app
}

#[test]
fn the_shipped_character_exposes_every_locomotion_clip() {
    let mut app = asset_app();
    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<Gltf> = server.load("character.glb");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        match server.load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(error) => {
                panic!("character.glb failed to load from {ASSET_ROOT}: {error:?}")
            }
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "timed out loading character.glb from {ASSET_ROOT}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let gltfs = app.world().resource::<Assets<Gltf>>();
    let gltf = gltfs.get(&handle).expect("loaded character.glb");
    for clip in [
        "idle_loop_fit",
        "walk_loop_fit",
        "sprint_loop_fit",
        "jump_start_fit",
        "jump_loop_fit",
        "slide_loop_fit",
    ] {
        assert!(
            gltf.named_animations.contains_key(clip),
            "character.glb is missing {clip}; it has {:?}",
            gltf.named_animations.keys().collect::<Vec<_>>()
        );
    }
}

/// The scene spawner adds an `AnimationPlayer` to the clip's animation root.
/// The locomotion wiring only works if that player exists, so pin the
/// expectation with the shipped file.
#[test]
fn the_shipped_scene_spawns_an_animation_player() {
    let mut app = asset_app();
    let server = app.world().resource::<AssetServer>().clone();
    let scene: Handle<WorldAsset> =
        server.load(GltfAssetLabel::Scene(0).from_asset("character.glb"));
    app.world_mut().spawn(WorldAssetRoot(scene));

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        let mut players = app
            .world_mut()
            .query_filtered::<Entity, With<AnimationPlayer>>();
        if players.iter(app.world()).next().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "character.glb scene never spawned an AnimationPlayer"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The shipping asset must contain a skinned mesh now that the character
/// artwork has been merged into the animation rig.
#[test]
fn the_shipped_character_has_a_renderable_skinned_mesh() {
    let mut app = asset_app();
    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<Gltf> = server.load("character.glb");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        match server.load_state(&handle) {
            LoadState::Loaded => break,
            LoadState::Failed(error) => panic!("character.glb failed to load: {error:?}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "timed out loading character.glb");
        std::thread::sleep(Duration::from_millis(10));
    }

    let gltfs = app.world().resource::<Assets<Gltf>>();
    let gltf = gltfs.get(&handle).expect("loaded character.glb");
    assert!(!gltf.meshes.is_empty(), "character.glb has no mesh");
    assert!(!gltf.skins.is_empty(), "character.glb has no skin");

    let nodes = app.world().resource::<Assets<GltfNode>>();
    let render_node = gltf.nodes.iter().any(|node_handle| {
        nodes
            .get(node_handle)
            .is_some_and(|node| node.mesh.is_some() && node.skin.is_some())
    });
    assert!(render_node, "no skinned mesh node in character.glb");
}
