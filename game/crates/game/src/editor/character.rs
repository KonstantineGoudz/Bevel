//! The playable character's glTF model and locomotion animations.
//!
//! `assets/character.glb` ships with the game (the web build copies
//! `game/assets/` to `web/assets/`). It contains the Teal Vanguard mesh
//! skinned to the UE5 Quinn rig; see `game/art/README.md` for the Blender
//! pipeline. The file's scene is spawned under an avatar entity that follows
//! the physics capsule, while the capsule itself stays the character root so
//! every existing physics, camera, minimap, and line-of-sight query keeps
//! working.
//!
//! Animation clips are matched by name rather than index, so a re-export that
//! reorders clips keeps working:
//!
//! | [`LocomotionState`] | clip slot | preferred clip |
//! | --- | --- | --- |
//! | `Stationary` / `Dead` | idle | `idle_loop` |
//! | `Walking` | walk | `walk_loop` |
//! | `Running` | run | `sprint_loop`, then `jog_fwd_loop` |
//! | `Jumping` | jump-start | `jump_start` |
//! | `Falling` | jump | `jump_loop` |
//! | `Sliding` | slide | `slide_loop` |
//!
//! A glTF that authors a collision mesh (a node or mesh named `collider`,
//! `collision`, `col_`, `UCX_*`, `UBX_*`, `USP_*`, or `UCP_*`) gets a convex
//! hull collider built from that mesh. Files without one keep the capsule
//! collider. Files without any renderable mesh keep the capsule placeholder
//! visible too, so the player never becomes an invisible collider.

use std::time::Duration;

use bevy::gltf::{Gltf, GltfAssetLabel, GltfMesh, GltfNode};
use bevy::mesh::VertexAttributeValues;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::*;

/// Path inside Bevy's asset root. The web build copies `game/assets/` to
/// `web/assets/`, so the browser fetches this from `/assets/character.glb`.
pub(super) const CHARACTER_MODEL_PATH: &str = "character.glb";

/// The Quinn rig walks toward +Z in glTF space; the game treats yaw 0 as -Z.
const MODEL_YAW: f32 = std::f32::consts::PI;

/// Cross-fade between grounded locomotion clips, in seconds.
const ANIMATION_FADE: f32 = 0.18;
/// Faster fade for short airborne and slide transitions.
const SNAP_FADE: f32 = 0.08;

/// Names that mark a glTF mesh as collision geometry rather than artwork.
const COLLIDER_MARKERS: &[&str] = &[
    "collider",
    "collision",
    "col_",
    "ucx_",
    "ubx_",
    "usp_",
    "ucp_",
];

/// The shipped character asset and everything discovered in it.
///
/// Inserted at startup. When glTF support is unavailable (headless test apps
/// that only add `AssetPlugin`), both handles stay `None` and the capsule
/// placeholder is used.
#[derive(Resource, Default)]
pub(super) struct CharacterModel {
    /// The whole glTF, used to inspect clips, nodes, and meshes.
    gltf: Option<Handle<Gltf>>,
    /// Scene 0, spawned as the avatar's child.
    scene: Option<Handle<WorldAsset>>,
    /// Facts discovered once the glTF finishes loading.
    pub(super) spec: CharacterSpec,
}

/// One-time inspection result for [`CharacterModel`].
#[derive(Default)]
pub(super) struct CharacterSpec {
    /// Set once loading succeeded or failed, so inspection runs once.
    pub(super) inspected: bool,
    /// Whether the file has artwork to draw (anything that is not a collider).
    pub(super) render_mesh: bool,
    /// Collider authored in the file, already centred on the capsule root.
    pub(super) collider: Option<Collider>,
    /// Animation graph shared by every avatar instance.
    pub(super) graph: Option<Handle<AnimationGraph>>,
    /// Resolved clip index per locomotion animation slot.
    pub(super) clips: CharacterClips,
}

/// The avatar that follows the physics capsule and carries the glTF scene.
#[derive(Component)]
pub(super) struct CharacterAvatar;

/// The fallback capsule mesh, shown only when the model has no artwork.
#[derive(Component)]
pub(super) struct CharacterPill;

/// The entity holding the spawned glTF scene.
#[derive(Component)]
struct CharacterSceneRoot;

/// Marks the skeleton root bone, whose baked root motion the game must cancel.
#[derive(Component)]
pub(super) struct CharacterRootBone;

/// Tracks the locomotion state currently playing on an `AnimationPlayer`.
#[derive(Component, Clone, Copy)]
pub(super) struct CharacterAnimation {
    state: LocomotionState,
    clips: CharacterClips,
}

/// One role a locomotion animation can fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AnimationSlot {
    Idle,
    Walk,
    Run,
    JumpStart,
    Jump,
    Slide,
}

impl AnimationSlot {
    /// Candidate clip names, best first. Exact matches win, then prefix
    /// matches (`idle_loop` -> `idle_loop_fit`), then substrings.
    fn candidates(self) -> &'static [&'static str] {
        match self {
            Self::Idle => &["idle_loop", "idle"],
            Self::Walk => &["walk_loop", "walk"],
            Self::Run => &["sprint_loop", "jog_fwd_loop", "run_loop", "jog", "run"],
            Self::JumpStart => &["jump_start", "ninjajump_start"],
            Self::Jump => &["jump_loop", "jump"],
            Self::Slide => &["slide_loop", "slide"],
        }
    }
}

/// The clip index chosen for each locomotion slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CharacterClips {
    pub(super) idle: Option<AnimationNodeIndex>,
    pub(super) walk: Option<AnimationNodeIndex>,
    pub(super) run: Option<AnimationNodeIndex>,
    pub(super) jump_start: Option<AnimationNodeIndex>,
    pub(super) jump: Option<AnimationNodeIndex>,
    pub(super) slide: Option<AnimationNodeIndex>,
}

impl CharacterClips {
    fn get(self, slot: AnimationSlot) -> Option<AnimationNodeIndex> {
        match slot {
            AnimationSlot::Idle => self.idle,
            AnimationSlot::Walk => self.walk,
            AnimationSlot::Run => self.run,
            AnimationSlot::JumpStart => self.jump_start,
            AnimationSlot::Jump => self.jump,
            AnimationSlot::Slide => self.slide,
        }
    }

    /// Clip for a locomotion state, falling back to idle when a slot is
    /// missing (for example a file with no slide clip).
    pub(super) fn for_state(self, state: LocomotionState) -> Option<AnimationNodeIndex> {
        self.get(slot_for_state(state)).or(self.idle)
    }

    fn is_empty(self) -> bool {
        self.idle.is_none()
            && self.walk.is_none()
            && self.run.is_none()
            && self.jump_start.is_none()
            && self.jump.is_none()
            && self.slide.is_none()
    }
}

/// Which animation role a locomotion state wants.
pub(super) fn slot_for_state(state: LocomotionState) -> AnimationSlot {
    match state {
        // There is no death clip in the shipping asset; idle is the least
        // misleading fallback.
        LocomotionState::Stationary | LocomotionState::Dead => AnimationSlot::Idle,
        LocomotionState::Walking => AnimationSlot::Walk,
        LocomotionState::Running => AnimationSlot::Run,
        LocomotionState::Jumping => AnimationSlot::JumpStart,
        LocomotionState::Falling => AnimationSlot::Jump,
        LocomotionState::Sliding => AnimationSlot::Slide,
    }
}

/// How long to fade into a state.
fn fade_for(state: LocomotionState) -> Duration {
    match state {
        LocomotionState::Jumping | LocomotionState::Falling | LocomotionState::Sliding => {
            Duration::from_secs_f32(SNAP_FADE)
        }
        _ => Duration::from_secs_f32(ANIMATION_FADE),
    }
}

/// True when a node or mesh name marks collision geometry.
pub(super) fn is_collider_mesh_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    COLLIDER_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Best match quality for one candidate: exact beats prefix beats substring.
fn clip_match_rank(name: &str, candidate: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    let candidate = candidate.to_ascii_lowercase();
    if name == candidate {
        Some(0)
    } else if name.starts_with(&candidate) {
        Some(1)
    } else if name.contains(&candidate) {
        Some(2)
    } else {
        None
    }
}

/// Resolve one slot from the file's named clips. Candidate order decides
/// between equally good name matches, so `sprint_loop` wins over `jog_fwd_loop`
/// for running.
fn find_clip(
    named: &[(String, AnimationNodeIndex)],
    candidates: &[&str],
) -> Option<AnimationNodeIndex> {
    let mut best: Option<((usize, u8, usize), AnimationNodeIndex)> = None;
    for (candidate_index, candidate) in candidates.iter().enumerate() {
        for (name_index, (name, index)) in named.iter().enumerate() {
            let Some(rank) = clip_match_rank(name, candidate) else {
                continue;
            };
            let key = (candidate_index, rank, name_index);
            if best.as_ref().is_none_or(|(best_key, _)| key < *best_key) {
                best = Some((key, *index));
            }
        }
    }
    best.map(|(_, index)| index)
}

fn clips_from_named(named: &[(String, AnimationNodeIndex)]) -> CharacterClips {
    CharacterClips {
        idle: find_clip(named, AnimationSlot::Idle.candidates()),
        walk: find_clip(named, AnimationSlot::Walk.candidates()),
        run: find_clip(named, AnimationSlot::Run.candidates()),
        jump_start: find_clip(named, AnimationSlot::JumpStart.candidates()),
        jump: find_clip(named, AnimationSlot::Jump.candidates()),
        slide: find_clip(named, AnimationSlot::Slide.candidates()),
    }
}

/// Build a graph containing every named clip, so future clips can be wired
/// without touching the loader again.
fn build_animation_graph(
    gltf: &Gltf,
    graphs: &mut Assets<AnimationGraph>,
) -> Option<(Handle<AnimationGraph>, CharacterClips)> {
    if gltf.named_animations.is_empty() {
        return None;
    }
    let mut entries: Vec<(String, Handle<AnimationClip>)> = gltf
        .named_animations
        .iter()
        .map(|(name, clip)| (name.to_string(), clip.clone()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let (graph, indices) = AnimationGraph::from_clips(entries.iter().map(|(_, clip)| clip.clone()));
    let named: Vec<(String, AnimationNodeIndex)> = entries
        .into_iter()
        .zip(indices)
        .map(|((name, _), index)| (name, index))
        .collect();
    let handle = graphs.add(graph);
    Some((handle, clips_from_named(&named)))
}

/// True when the file has a mesh that is not collision geometry.
fn has_render_mesh(gltf: &Gltf, nodes: &Assets<GltfNode>) -> bool {
    let mut saw_node = false;
    for handle in &gltf.nodes {
        let Some(node) = nodes.get(handle) else {
            continue;
        };
        saw_node = true;
        if node.mesh.is_some() && !is_collider_mesh_name(&node.name) {
            return true;
        }
    }
    if saw_node {
        false
    } else {
        !gltf.meshes.is_empty()
    }
}

/// Build a convex hull from every collision mesh in the file, in the capsule
/// root's local space.
fn collider_from_gltf(
    gltf: &Gltf,
    nodes: &Assets<GltfNode>,
    gltf_meshes: &Assets<GltfMesh>,
    meshes: &Assets<Mesh>,
) -> Option<Collider> {
    let mut points: Vec<Vec3> = Vec::new();
    for handle in &gltf.nodes {
        let Some(node) = nodes.get(handle) else {
            continue;
        };
        if !is_collider_mesh_name(&node.name) {
            continue;
        }
        let Some(mesh_handle) = &node.mesh else {
            continue;
        };
        let Some(gltf_mesh) = gltf_meshes.get(mesh_handle) else {
            continue;
        };
        for primitive in &gltf_mesh.primitives {
            let Some(mesh) = meshes.get(&primitive.mesh) else {
                continue;
            };
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                continue;
            };
            points.extend(
                positions
                    .iter()
                    .map(|point| node.transform.transform_point(Vec3::from_array(*point))),
            );
        }
    }
    if points.len() < 4 {
        return None;
    }
    // Character collision meshes are authored with the feet at the origin;
    // the capsule root sits at the capsule centre.
    let points: Vec<Vec3> = points
        .into_iter()
        .map(|point| point - Vec3::Y * PILL_HALF)
        .collect();
    Collider::convex_hull(&points)
}

/// World pose for the avatar. The feet sit on the ground plane, and a sliding
/// character faces its momentum like the capsule does.
pub(super) fn avatar_pose(character: Character) -> (Vec3, Quat) {
    let yaw = if character.sliding && character.hv.length_squared() > 0.01 {
        let dir = character.hv.normalize_or_zero();
        (-dir.x).atan2(-dir.y)
    } else {
        character.yaw
    };
    (
        Vec3::new(character.pos.x, character.feet, character.pos.y),
        Quat::from_rotation_y(yaw),
    )
}

/// Startup: begin loading the shipped character asset. Headless test apps
/// without `GltfPlugin` skip the load (and keep the capsule placeholder).
pub(super) fn load_character_model(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    gltf_assets: Option<Res<Assets<Gltf>>>,
) {
    let model = if gltf_assets.is_some() {
        CharacterModel {
            gltf: Some(asset_server.load(CHARACTER_MODEL_PATH)),
            scene: Some(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(CHARACTER_MODEL_PATH)),
            ),
            ..default()
        }
    } else {
        CharacterModel::default()
    };
    commands.insert_resource(model);
}

/// Inspect the loaded glTF once: choose the collider, resolve clip names, and
/// build the shared animation graph.
pub(super) fn inspect_character_model(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    gltfs: Option<Res<Assets<Gltf>>>,
    nodes: Option<Res<Assets<GltfNode>>>,
    gltf_meshes: Option<Res<Assets<GltfMesh>>>,
    meshes: Option<Res<Assets<Mesh>>>,
    mut graphs: Option<ResMut<Assets<AnimationGraph>>>,
    mut model: ResMut<CharacterModel>,
    roots: Query<Entity, With<CharacterVisual>>,
) {
    if model.spec.inspected {
        return;
    }
    let Some(gltf_handle) = model.gltf.clone() else {
        return;
    };
    let Some(gltfs) = gltfs.as_ref() else {
        return;
    };
    if matches!(
        asset_server.load_state(&gltf_handle),
        bevy::asset::LoadState::Failed(_)
    ) {
        warn!("{CHARACTER_MODEL_PATH} failed to load; keeping the capsule placeholder");
        model.spec.inspected = true;
        return;
    }
    // Wait for clips, nodes, meshes, and textures so the render-mesh and
    // collider checks see every subasset, not a half-loaded file.
    if !asset_server.is_loaded_with_dependencies(&gltf_handle) {
        return;
    }
    let Some(gltf) = gltfs.get(&gltf_handle) else {
        return;
    };

    let render_mesh = nodes
        .as_ref()
        .is_some_and(|nodes| has_render_mesh(gltf, nodes));
    let collider = match (nodes.as_ref(), gltf_meshes.as_ref(), meshes.as_ref()) {
        (Some(nodes), Some(gltf_meshes), Some(meshes)) => {
            collider_from_gltf(gltf, nodes, gltf_meshes, meshes)
        }
        _ => None,
    };
    if let Some(graphs) = graphs.as_mut()
        && let Some((graph, clips)) = build_animation_graph(gltf, graphs)
    {
        model.spec.graph = Some(graph);
        model.spec.clips = clips;
    }
    model.spec.render_mesh = render_mesh;
    model.spec.collider = collider.clone();
    model.spec.inspected = true;

    info!(
        "{CHARACTER_MODEL_PATH}: {} mesh(es), {} clip(s), render_mesh={render_mesh}, \
         collider_mesh={}",
        gltf.meshes.len(),
        gltf.named_animations.len(),
        collider.is_some()
    );
    if !render_mesh {
        info!(
            "{CHARACTER_MODEL_PATH} has no renderable mesh; keeping the capsule \
             placeholder visible (the skeleton and clips are still wired)"
        );
    }
    if let Some(collider) = collider {
        for root in &roots {
            commands.entity(root).insert(collider.clone());
        }
    }
}

/// Keep the avatar under the physics capsule. The avatar is a sibling of the
/// capsule root, so it follows position and yaw without inheriting the slide
/// squash or tilt applied to the collider.
pub(super) fn sync_character_avatar(
    mut commands: Commands,
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    kit: Res<Kit>,
    model: Res<CharacterModel>,
    mut visuals: ResMut<Visuals>,
    mut avatars: Query<(&mut Transform, &mut Visibility), With<CharacterAvatar>>,
    mut pills: Query<&mut Visibility, (With<CharacterPill>, Without<CharacterAvatar>)>,
) {
    let Some(character) = editor.character else {
        if let Some(avatar) = visuals.character_avatar.take() {
            commands.entity(avatar).despawn();
        }
        return;
    };

    let avatar = match visuals.character_avatar {
        Some(avatar) => avatar,
        None => {
            let (translation, rotation) = avatar_pose(character);
            let avatar = commands
                .spawn((
                    CharacterAvatar,
                    Name::new("CharacterAvatar"),
                    Transform {
                        translation,
                        rotation,
                        ..default()
                    },
                    Visibility::Visible,
                ))
                .id();
            let pill = commands
                .spawn((
                    CharacterPill,
                    Mesh3d(kit.pill.clone()),
                    MeshMaterial3d(kit.pill_mat.clone()),
                    Transform::from_xyz(0.0, PILL_HALF, 0.0),
                    Visibility::Visible,
                ))
                .id();
            commands.entity(avatar).add_child(pill);
            if let Some(scene) = model.scene.clone() {
                let scene_root = commands
                    .spawn((
                        CharacterSceneRoot,
                        WorldAssetRoot(scene),
                        Transform::from_rotation(Quat::from_rotation_y(MODEL_YAW)),
                    ))
                    .id();
                commands.entity(avatar).add_child(scene_root);
            }
            visuals.character_avatar = Some(avatar);
            avatar
        }
    };

    let (translation, rotation) = avatar_pose(character);
    if let Ok((mut transform, mut visibility)) = avatars.get_mut(avatar) {
        transform.translation = translation;
        transform.rotation = rotation;
        transform.scale = Vec3::ONE;
        *visibility = if orbit.mode == ViewMode::First {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    }

    // Loading, a meshless rig, or a failed load all keep the capsule visible.
    let show_pill = !model.spec.render_mesh;
    for mut visibility in &mut pills {
        *visibility = if show_pill {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Wire the spawned scene's `AnimationPlayer` to the shared graph once.
pub(super) fn wire_character_animation(
    mut commands: Commands,
    editor: Res<Editor>,
    model: Res<CharacterModel>,
    avatars: Query<Entity, With<CharacterAvatar>>,
    children: Query<&Children>,
    mut players: Query<(Entity, &mut AnimationPlayer), Without<CharacterAnimation>>,
) {
    let Some(graph) = model.spec.graph.clone() else {
        return;
    };
    let clips = model.spec.clips;
    if clips.is_empty() {
        return;
    }
    let state = editor
        .character
        .map_or(LocomotionState::Stationary, Character::locomotion_state);

    for avatar in &avatars {
        for entity in children.iter_descendants(avatar) {
            let Ok((player_entity, mut player)) = players.get_mut(entity) else {
                continue;
            };
            let mut transitions = AnimationTransitions::new();
            if let Some(index) = clips.for_state(state) {
                transitions
                    .play(&mut player, index, Duration::ZERO)
                    .repeat();
            }
            commands.entity(player_entity).insert((
                AnimationGraphHandle(graph.clone()),
                transitions,
                CharacterAnimation { state, clips },
            ));
        }
    }
}

/// Mark the skeleton's `root` bone. Its clip translation is baked root motion;
/// [`pin_animation_root`] cancels it so the capsule remains the only source of
/// travel. Runs every frame because the scene spawner, not this module, adds
/// the bone entities.
pub(super) fn mark_animation_root(
    mut commands: Commands,
    avatars: Query<Entity, With<CharacterAvatar>>,
    children: Query<&Children>,
    names: Query<&Name, Without<CharacterRootBone>>,
) {
    for avatar in &avatars {
        for entity in children.iter_descendants(avatar) {
            if names
                .get(entity)
                .is_ok_and(|name| name.as_str().eq_ignore_ascii_case("root"))
            {
                commands.entity(entity).insert(CharacterRootBone);
            }
        }
    }
}

/// Pick a clip when the locomotion state changes.
pub(super) fn drive_character_animation(
    editor: Res<Editor>,
    mut players: Query<(
        &mut AnimationPlayer,
        &mut AnimationTransitions,
        &mut CharacterAnimation,
    )>,
) {
    let Some(character) = editor.character else {
        return;
    };
    let next = character.locomotion_state();
    for (mut player, mut transitions, mut animation) in &mut players {
        if animation.state == next {
            continue;
        }
        animation.state = next;
        let Some(index) = animation.clips.for_state(next) else {
            continue;
        };
        transitions
            .play(&mut player, index, fade_for(next))
            .repeat();
    }
}

/// Cancel the clips' baked root motion. The physics capsule owns travel, so
/// leaving the skeleton root translation in place would slide the model out of
/// its collider. Runs after animation writes and before transform propagation.
pub(super) fn pin_animation_root(mut roots: Query<&mut Transform, With<CharacterRootBone>>) {
    for mut transform in &mut roots {
        if transform.translation != Vec3::ZERO {
            transform.translation = Vec3::ZERO;
        }
    }
}

/// Hide collision meshes that a glTF scene spawned into the world.
pub(super) fn hide_collider_meshes(
    avatars: Query<Entity, With<CharacterAvatar>>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut visibilities: Query<&mut Visibility>,
) {
    for avatar in &avatars {
        for entity in children.iter_descendants(avatar) {
            if names
                .get(entity)
                .is_ok_and(|name| is_collider_mesh_name(name.as_str()))
                && let Ok(mut visibility) = visibilities.get_mut(entity)
            {
                *visibility = Visibility::Hidden;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<(String, AnimationNodeIndex)> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.to_string(), AnimationNodeIndex::new(index)))
            .collect()
    }

    /// Every clip in `character.glb`, in file order.
    const SHIPPED_CLIPS: &[&str] = &[
        "ninjajump_start_fit",
        "slide_start_fit",
        "ninjajump_land_fit",
        "ninjajump_idle_loop_fit",
        "crouch_idle_loop_fit",
        "idle_loop_fit",
        "jog_fwd_loop_fit",
        "jump_land_fit",
        "jump_loop_fit",
        "jump_start_fit",
        "laytoidle_fit",
        "yes_fit",
        "walk_loop_fit",
        "slide_loop_fit",
        "roll_fit",
        "sprint_loop_fit",
        "swim_idle_loop_fit",
        "swim_fwd_loop_fit",
        "walk_carry_loop_fit",
        "walk_formal_loop_fit",
        "zombie_walk_fwd_loop_fit",
        "slide_exit_fit",
    ];

    #[test]
    fn shipped_clips_fill_every_locomotion_slot() {
        let clips = clips_from_named(&named(SHIPPED_CLIPS));
        assert_eq!(clips.idle, Some(AnimationNodeIndex::new(5)));
        assert_eq!(clips.walk, Some(AnimationNodeIndex::new(12)));
        assert_eq!(clips.run, Some(AnimationNodeIndex::new(15)));
        assert_eq!(clips.jump_start, Some(AnimationNodeIndex::new(9)));
        assert_eq!(clips.jump, Some(AnimationNodeIndex::new(8)));
        assert_eq!(clips.slide, Some(AnimationNodeIndex::new(13)));
    }

    #[test]
    fn states_map_to_the_expected_animation_slots() {
        assert_eq!(
            slot_for_state(LocomotionState::Stationary),
            AnimationSlot::Idle
        );
        assert_eq!(slot_for_state(LocomotionState::Dead), AnimationSlot::Idle);
        assert_eq!(
            slot_for_state(LocomotionState::Walking),
            AnimationSlot::Walk
        );
        assert_eq!(slot_for_state(LocomotionState::Running), AnimationSlot::Run);
        assert_eq!(
            slot_for_state(LocomotionState::Jumping),
            AnimationSlot::JumpStart
        );
        assert_eq!(
            slot_for_state(LocomotionState::Falling),
            AnimationSlot::Jump
        );
        assert_eq!(
            slot_for_state(LocomotionState::Sliding),
            AnimationSlot::Slide
        );
    }

    #[test]
    fn a_missing_state_clip_falls_back_to_idle() {
        let clips = CharacterClips {
            idle: Some(AnimationNodeIndex::new(0)),
            ..default()
        };
        assert_eq!(
            clips.for_state(LocomotionState::Sliding),
            Some(AnimationNodeIndex::new(0))
        );
    }

    #[test]
    fn crouch_idle_does_not_steal_the_idle_slot() {
        // `crouch_idle_loop_fit` contains `idle_loop`, but a name that starts
        // with the candidate must win.
        let clips = clips_from_named(&named(&["crouch_idle_loop_fit", "idle_loop_fit"]));
        assert_eq!(clips.idle, Some(AnimationNodeIndex::new(1)));
    }

    #[test]
    fn collider_mesh_names_are_recognized() {
        for name in [
            "Collider",
            "player_collision",
            "col_body",
            "UCX_Quinn",
            "UBX_Box",
            "USP_Sphere",
            "UCP_Capsule",
        ] {
            assert!(is_collider_mesh_name(name), "{name} should be a collider");
        }
        for name in ["Quinn", "root", "walk_loop_fit", "clavicle_l"] {
            assert!(!is_collider_mesh_name(name), "{name} should be artwork");
        }
    }

    fn avatar_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(Editor {
            character: Some(Character {
                pos: Vec2::new(1.0, -2.0),
                feet: 0.5,
                yaw: 0.0,
                vy: 0.0,
                hv: Vec2::ZERO,
                ride_vel: Vec2::ZERO,
                grounded: true,
                sliding: false,
                health: Health::default(),
            }),
            ..default()
        });
        app.init_resource::<Orbit>();
        app.init_resource::<Visuals>();
        app.insert_resource(Kit {
            parts: Vec::new(),
            mats: Vec::new(),
            glass: Handle::default(),
            ghost: Handle::default(),
            pill: Handle::default(),
            pill_mat: Handle::default(),
            cone: Handle::default(),
            cone_mat: Handle::default(),
            grass: Handle::default(),
            grass_mats: [Handle::default(), Handle::default()],
            character_collider: Collider::capsule_y(PILL_CYLINDER * 0.5, PILL_RADIUS),
        });
        app.insert_resource(CharacterModel::default());
        app.add_systems(Update, sync_character_avatar);
        app
    }

    #[test]
    fn a_meshless_model_keeps_a_visible_capsule_placeholder() {
        let mut app = avatar_app();
        app.update();

        let mut avatars = app
            .world_mut()
            .query_filtered::<Entity, With<CharacterAvatar>>();
        let avatar = avatars
            .iter(app.world())
            .next()
            .expect("a character spawns an avatar");
        let transform = *app
            .world()
            .entity(avatar)
            .get::<Transform>()
            .expect("avatar transform");
        assert_eq!(transform.translation, Vec3::new(1.0, 0.5, -2.0));

        let mut pills = app
            .world_mut()
            .query_filtered::<Entity, With<CharacterPill>>();
        let pill = pills
            .iter(app.world())
            .next()
            .expect("the placeholder capsule is a child of the avatar");
        assert_eq!(
            *app.world().entity(pill).get::<Visibility>().unwrap(),
            Visibility::Visible
        );

        // A later inspection that finds artwork hides the placeholder.
        app.world_mut()
            .resource_mut::<CharacterModel>()
            .spec
            .render_mesh = true;
        app.update();
        assert_eq!(
            *app.world().entity(pill).get::<Visibility>().unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn the_avatar_stands_on_its_feet_and_faces_its_yaw() {
        let character = Character {
            pos: Vec2::new(1.0, -2.0),
            feet: 0.5,
            yaw: 0.25,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: false,
            health: Health::default(),
        };
        let (position, rotation) = avatar_pose(character);
        assert_eq!(position, Vec3::new(1.0, 0.5, -2.0));
        assert!((rotation.to_euler(EulerRot::YXZ).0 - 0.25).abs() < 1e-5);
    }

    #[test]
    fn a_sliding_avatar_faces_its_momentum() {
        let character = Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 1.0,
            vy: 0.0,
            hv: Vec2::new(-1.0, 0.0),
            ride_vel: Vec2::ZERO,
            grounded: true,
            sliding: true,
            health: Health::default(),
        };
        let (_, rotation) = avatar_pose(character);
        // Momentum toward -X means facing -X: yaw = atan2(1, 0) = pi/2.
        assert!((rotation.to_euler(EulerRot::YXZ).0 - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }
}
