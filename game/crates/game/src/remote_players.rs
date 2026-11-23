//! Visuals for the other players in the shared world.
//!
//! The sim includes the other online players near us in each filtered
//! `Snapshot` (interest management keeps the list to the receiver's player
//! radius), but a snapshot is a packet, not a visual: two people could stand in
//! the same room and see nothing of each other. This module gives each remote a
//! capsule avatar (matching the physics pill exactly), a facing wedge, and a
//! floating name label, and interpolates between the last two snapshots so
//! movement is smooth at any frame rate.
//!
//! ## Identity comes from `Welcome`, not from the snapshot
//!
//! A snapshot is filtered *per receiver* and always includes the receiver, so
//! the first entry is just as likely to be someone else. Before `Welcome`
//! arrives we do not know which `sub` is ours; spawning "everyone in the
//! snapshot" would briefly draw an avatar of ourselves (and, worse, make the
//! real players indistinguishable from a duplicate). Avatars therefore wait for
//! `RemoteFeed::self_sub`, which the net client learns from `Welcome` on the
//! reliable channel.
//!
//! ## Nameplates are UI, not `Text2d`
//!
//! `Text2d` is extracted into the sprite pipeline's `Transparent2d` render
//! phase, and those phases are created **only for `Camera2d` cameras**
//! (`bevy_core_pipeline::core_2d::extract_core_2d_camera_phases`). This game
//! has third-person `Camera3d` cameras only, so a `Text2d` nameplate is never
//! drawn: the avatar capsule appears but its name silently does not. The label
//! is therefore a screen-space UI node whose absolute position is projected
//! from the avatar's head every frame — it renders with the same UI text stack
//! as the HUD, which works on every target (including WebGL2).
//!
//! ## Snapshot gating
//!
//! Snapshots arrive on an unreliable channel. The renderer consumes each
//! *distinct* snapshot once: re-processing the same packet would reset
//! interpolation. The gate keys on `(tick, self_sub, seen subs)` — not on tick
//! alone — because the feed can legitimately publish a new snapshot for the
//! same tick (a duplicate packet that carries another player, or a session
//! whose tick restarted at 0 after a sim restart), and dropping that would
//! leave an avatar missing until the next tick.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::ui::UiScale;
use party_dungeon_net::{tick_diff, PlayerState, Snapshot, TICK_RATE};

use crate::editor::MainCamera;
use crate::net_client::RemoteFeed;
use crate::sim_support::{PILL_CYLINDER, PILL_HALF, PILL_RADIUS};

/// Ticks of snapshots a player may be absent from before their avatar is
/// despawned (~1.5 s at 60 Hz). Losing a snapshot to an unreliable channel must
/// not pop the avatar. Measured in *ticks* rather than processed frames so a
/// slow renderer (or a stalled feed) cannot stretch the grace to seconds.
const MISSED_TICKS_MAX: u32 = 90;

/// Nameplate text size, in UI pixels.
const NAMEPLATE_FONT_PX: f32 = 14.0;
/// Rough advance width of the built-in font at `NAMEPLATE_FONT_PX`, used to
/// size the label node so it can be centred on the avatar without reading the
/// measured text width back from the layout (which lags a frame and is in
/// physical pixels on some targets).
const NAMEPLATE_GLYPH_ADVANCE: f32 = 0.62;
/// Extra world-space height above the avatar centre for the label anchor.
const NAMEPLATE_LIFT: f32 = 0.2;

pub struct RemotePlayersPlugin;

impl Plugin for RemotePlayersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RemoteAssets>().add_systems(
            Update,
            (
                sync_remote_players,
                interpolate_remote_avatars,
                place_remote_nameplates,
                despawn_orphan_nameplates,
            )
                .chain(),
        );
    }
}

/// One other player, rendered locally. Rebuilt from snapshots; the net client
/// owns no ECS entities.
#[derive(Component, Debug)]
pub struct RemotePlayer {
    pub sub: String,
    pub name: String,
    /// Pose at the previous snapshot (interpolated from).
    pub interp_from: Option<PlayerState>,
    /// Pose at the newest snapshot (interpolated to).
    pub interp_to: Option<PlayerState>,
    /// 0..1 progress from `interp_from` to `interp_to`.
    pub interp_alpha: f32,
    /// Server tick of the last snapshot this player appeared in.
    pub last_seen_tick: u32,
}

/// A floating name label belonging to one [`RemotePlayer`]. It lives in the UI
/// tree (see the module docs for why it is not a `Text2d`).
#[derive(Component)]
pub struct RemoteNameplate {
    /// The avatar this label follows.
    pub owner: Entity,
    /// Width of the label node in UI pixels, so the projected position can be
    /// centred on the avatar.
    pub width: f32,
}

/// What the last processed snapshot looked like, so the same packet is not
/// applied twice (which would restart interpolation) while a *changed* packet
/// with an unchanged tick still is.
#[derive(PartialEq, Eq)]
struct SeenSnapshot {
    tick: u32,
    self_sub: Option<String>,
    /// The remote (non-self) subjects in the snapshot, sorted.
    subs: Vec<String>,
}

/// Shared avatar meshes, built on first use (needs `Assets<Mesh>`).
#[derive(Resource, Default)]
struct RemoteAssets {
    body: Option<Handle<Mesh>>,
    nose: Option<Handle<Mesh>>,
    /// Last snapshot the renderer acted on.
    seen: Option<SeenSnapshot>,
    /// The remote-subject set at the last INFO presence line, so the log fires
    /// once per change instead of once per snapshot (~60 times a second).
    logged_subs: Option<Vec<String>>,
    /// Whether the "identity unknown" state was already logged.
    logged_no_identity: bool,
}

impl RemotePlayer {
    /// World pose for the current interpolation state.
    pub(crate) fn pose(&self) -> (Vec3, Quat) {
        let Some(to) = self.interp_to.as_ref() else {
            return (Vec3::Y * PILL_HALF, Quat::IDENTITY);
        };
        let translation = match self.interp_from.as_ref() {
            Some(from) => lerp_pose(from, to, self.interp_alpha),
            None => pose_of(to),
        };
        let yaw = match self.interp_from.as_ref() {
            Some(from) => angle_lerp(from.yaw, to.yaw, self.interp_alpha),
            None => to.yaw,
        };
        (translation, Quat::from_rotation_y(yaw))
    }
}

/// Feet position + capsule height offset -> centre of the avatar.
fn pose_of(state: &PlayerState) -> Vec3 {
    Vec3::new(state.x, state.y + PILL_HALF, state.z)
}

fn lerp_pose(from: &PlayerState, to: &PlayerState, alpha: f32) -> Vec3 {
    let t = alpha.clamp(0.0, 1.0);
    let a = pose_of(from);
    let b = pose_of(to);
    a.lerp(b, t)
}

/// Shortest-arc angle interpolation, so a remote turning across ±180° doesn't
/// spin the long way round.
fn angle_lerp(from: f32, to: f32, alpha: f32) -> f32 {
    let mut delta = (to - from) % std::f32::consts::TAU;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    from + delta * alpha.clamp(0.0, 1.0)
}

/// A few characters of the subject for logs (subjects are long opaque ids).
pub(crate) fn short_sub(sub: &str) -> &str {
    sub.get(..8).unwrap_or(sub)
}

/// Stable, well-spread colour per auth subject, so the same player keeps the
/// same avatar colour across sessions and machines.
pub(crate) fn color_for_sub(sub: &str) -> Color {
    // FNV-1a: cheap, stable across runs (unlike `DefaultHasher`).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sub.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hue = (hash % 360) as f32;
    Color::hsl(hue, 0.65, 0.55)
}

/// Display name for a remote (empty names fall back to "Player").
pub(crate) fn display_name(player: &PlayerState) -> &str {
    if player.name.trim().is_empty() {
        "Player"
    } else {
        player.name.as_str()
    }
}

/// Spawn the capsule + facing wedge for one remote and return its entity.
fn spawn_avatar(
    commands: &mut Commands,
    assets: &mut RemoteAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    player: &PlayerState,
    tick: u32,
) -> Entity {
    let body = assets
        .body
        .get_or_insert_with(|| meshes.add(Capsule3d::new(PILL_RADIUS, PILL_CYLINDER)))
        .clone();
    let nose = assets
        .nose
        .get_or_insert_with(|| meshes.add(Cone {
            radius: PILL_RADIUS * 0.45,
            height: PILL_RADIUS * 1.1,
        }))
        .clone();
    let color = color_for_sub(&player.sub);
    let material = materials.add(StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.7,
        ..default()
    });
    let translation = pose_of(player);
    let rotation = Quat::from_rotation_y(player.yaw);
    let avatar = commands
        .spawn((
            RemotePlayer {
                sub: player.sub.clone(),
                name: player.name.clone(),
                interp_from: None,
                interp_to: Some(player.clone()),
                interp_alpha: 1.0,
                last_seen_tick: tick,
            },
            Mesh3d(body),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(translation).with_rotation(rotation),
            Visibility::default(),
            children![(
                Mesh3d(nose),
                MeshMaterial3d(material),
                // Points along -Z, the direction `Quat::from_rotation_y` faces.
                Transform::from_xyz(0.0, PILL_CYLINDER * 0.25, -PILL_RADIUS),
            )],
        ))
        .id();
    spawn_nameplate(commands, avatar, display_name(player));
    avatar
}

/// Spawn the screen-space name label for an avatar. It starts hidden; the
/// placement system projects it onto the screen every frame.
pub(crate) fn spawn_nameplate(commands: &mut Commands, owner: Entity, label: &str) {
    commands.spawn((
        RemoteNameplate {
            owner,
            width: nameplate_width(label),
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(nameplate_width(label)),
            ..default()
        },
        Text::new(label.to_string()),
        TextFont {
            font_size: bevy::text::FontSize::Px(NAMEPLATE_FONT_PX),
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::new(Justify::Left, LineBreak::NoWrap),
        Visibility::Hidden,
    ));
}

/// Node width that fits `label` on one line, from the font's average advance.
pub(crate) fn nameplate_width(label: &str) -> f32 {
    let glyphs = label.chars().count().max(1) as f32;
    glyphs * NAMEPLATE_FONT_PX * NAMEPLATE_GLYPH_ADVANCE + 8.0
}

/// Update a label's text (and the node width used to centre it).
pub(crate) fn retitle_nameplate(
    nameplates: &mut Query<(Entity, &mut RemoteNameplate)>,
    texts: &mut Query<&mut Text>,
    plate_entity: Entity,
    label: &str,
) {
    if let Ok((_, mut plate)) = nameplates.get_mut(plate_entity) {
        plate.width = nameplate_width(label);
    }
    if let Ok(mut text) = texts.get_mut(plate_entity) {
        if text.0 != label {
            text.0 = label.to_string();
        }
    }
}

/// Turn the newest snapshot into avatar spawns, pose updates and despawns.
fn sync_remote_players(
    mut commands: Commands,
    feed: Option<Res<RemoteFeed>>,
    mut assets: ResMut<RemoteAssets>,
    mut avatars: Query<(Entity, &mut RemotePlayer)>,
    mut nameplates: Query<(Entity, &mut RemoteNameplate)>,
    mut texts: Query<&mut Text>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let Some(feed) = feed else {
        return;
    };
    let Some(snapshot) = feed.snapshot.as_ref() else {
        return;
    };
    let self_sub = feed.self_sub.as_deref();

    // The remote subjects in this snapshot, sorted for stable comparison.
    let subs: Vec<String> = sorted_remote_subs(snapshot, self_sub);

    // Apply each distinct snapshot exactly once. A repeated packet would reset
    // interpolation; a *changed* packet with an unchanged tick must still be
    // applied, hence the full key rather than the tick alone.
    let seen = SeenSnapshot {
        tick: snapshot.tick,
        self_sub: feed.self_sub.clone(),
        subs: subs.clone(),
    };
    if assets.seen.as_ref() == Some(&seen) {
        return;
    }
    assets.seen = Some(seen);

    // Presence is worth a log line, and the *set* is the part that changes: one
    // line per change makes "do I receive the other player?" answerable from
    // the browser console without a debugger.
    if assets.logged_subs.as_ref() != Some(&subs) {
        if subs.is_empty() {
            info!("remote players: 0 visible");
        } else {
            let subjects: Vec<&str> = subs.iter().map(|sub| short_sub(sub)).collect();
            info!(
                "remote players: {} visible [{}]",
                subs.len(),
                subjects.join(", ")
            );
        }
        assets.logged_subs = Some(subs.clone());
    }
    if self_sub.is_none() && !assets.logged_no_identity {
        assets.logged_no_identity = true;
        info!(
            "remote players: identity not known yet (no Welcome); holding {} player(s) from the snapshot",
            snapshot.players.len()
        );
    }

    // Which avatars already exist, by subject.
    let mut existing: HashMap<String, Entity> = HashMap::new();
    for (entity, remote) in &mut avatars {
        existing.insert(remote.sub.clone(), entity);
    }
    // Nameplate entity by avatar entity, so a despawn takes its label with it.
    let mut plates: HashMap<Entity, Entity> = HashMap::new();
    for (plate_entity, plate) in &mut nameplates {
        plates.insert(plate.owner, plate_entity);
    }

    let Some(self_sub) = self_sub else {
        // Do not guess our identity from a per-receiver snapshot: wait for
        // `Welcome`. Any avatars here are stale anyway, and the grace below
        // still expires them.
        expire_missing_avatars(
            &mut commands,
            &mut avatars,
            &mut plates,
            existing,
            self_sub,
            snapshot.tick,
        );
        return;
    };

    for player in &snapshot.players {
        if player.sub == self_sub {
            continue;
        }
        match existing.remove(&player.sub) {
            Some(entity) => {
                let renamed = if let Ok((_, mut remote)) = avatars.get_mut(entity) {
                    remote.last_seen_tick = snapshot.tick;
                    let renamed = remote.name != player.name;
                    remote.name = player.name.clone();
                    // The previous target becomes the interpolation origin.
                    remote.interp_from = remote.interp_to.replace(player.clone());
                    remote.interp_alpha = 0.0;
                    renamed
                } else {
                    false
                };
                if renamed && let Some(&plate_entity) = plates.get(&entity) {
                    retitle_nameplate(
                        &mut nameplates,
                        &mut texts,
                        plate_entity,
                        display_name(player),
                    );
                }
            }
            None => {
                let (Some(mut meshes), Some(mut materials)) = (meshes.as_mut(), materials.as_mut())
                else {
                    // Headless (no renderer): nothing to draw, nothing to do.
                    continue;
                };
                // Presence is worth a log line: "other player visible" is the
                // difference between an empty world and a broken one.
                info!(
                    "remote player visible: {} ({})",
                    display_name(player),
                    short_sub(&player.sub)
                );
                spawn_avatar(
                    &mut commands,
                    &mut assets,
                    &mut meshes,
                    &mut materials,
                    player,
                    snapshot.tick,
                );
            }
        }
    }

    // Anything left was absent from this snapshot; also drop an avatar whose
    // sub turned out to be us (only possible if identity arrived late).
    expire_missing_avatars(
        &mut commands,
        &mut avatars,
        &mut plates,
        existing,
        Some(self_sub),
        snapshot.tick,
    );
}

/// Remote (non-self) subjects in a snapshot, sorted and deduplicated.
fn sorted_remote_subs(snapshot: &Snapshot, self_sub: Option<&str>) -> Vec<String> {
    let mut subs: Vec<String> = snapshot
        .players
        .iter()
        .filter(|player| Some(player.sub.as_str()) != self_sub)
        .map(|player| player.sub.clone())
        .collect();
    subs.sort_unstable();
    subs.dedup();
    subs
}

/// Despawn an avatar together with its nameplate.
fn despawn_avatar(
    commands: &mut Commands,
    plates: &mut HashMap<Entity, Entity>,
    avatar: Entity,
) {
    if let Some(plate) = plates.remove(&avatar) {
        commands.entity(plate).despawn();
    }
    commands.entity(avatar).despawn();
}

/// Expire avatars absent from the snapshot for longer than the grace, and
/// despawn one whose subject turned out to be us (late `Welcome`).
fn expire_missing_avatars(
    commands: &mut Commands,
    avatars: &mut Query<(Entity, &mut RemotePlayer)>,
    plates: &mut HashMap<Entity, Entity>,
    existing: HashMap<String, Entity>,
    self_sub: Option<&str>,
    tick: u32,
) {
    for (sub, entity) in existing {
        let Ok((_, remote)) = avatars.get(entity) else {
            continue;
        };
        let gone = sub == self_sub.unwrap_or("")
            || tick_diff(tick, remote.last_seen_tick) > MISSED_TICKS_MAX as i32;
        if gone {
            info!(
                "remote player gone: {} ({})",
                remote.name,
                short_sub(&remote.sub)
            );
            despawn_avatar(commands, plates, entity);
        }
    }
}

/// Advance the interpolation clock and write the avatar transform.
pub(crate) fn interpolate_remote_avatars(time: Res<Time>, mut q: Query<(&mut RemotePlayer, &mut Transform)>) {
    // One snapshot is covered per tick, so `alpha` reaches 1 as the next
    // snapshot arrives.
    let step = time.delta_secs() * TICK_RATE as f32;
    for (mut remote, mut transform) in &mut q {
        remote.interp_alpha = (remote.interp_alpha + step).min(1.0);
        let (translation, rotation) = remote.pose();
        if transform.translation != translation {
            transform.translation = translation;
        }
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// Project each nameplate from its avatar's head to the screen.
pub(crate) fn place_remote_nameplates(
    ui_scale: Option<Res<UiScale>>,
    cameras: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<MainCamera>)>,
    avatars: Query<&GlobalTransform, With<RemotePlayer>>,
    mut plates: Query<(&RemoteNameplate, &mut Node, &mut Visibility)>,
) {
    let Some((camera, camera_transform)) = cameras.iter().next() else {
        return;
    };
    // UI values are multiplied by `UiScale`, so undo it to land on the logical
    // viewport coordinate `world_to_viewport` returns.
    let inv_ui_scale = ui_scale
        .as_ref()
        .map(|scale| 1.0 / scale.0.max(f32::EPSILON))
        .unwrap_or(1.0);
    for (plate, mut node, mut visibility) in &mut plates {
        let Ok(avatar) = avatars.get(plate.owner) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let head = avatar.translation() + Vec3::Y * (PILL_HALF + NAMEPLATE_LIFT);
        match camera.world_to_viewport(camera_transform, head) {
            Ok(screen) => {
                let left = (screen.x - plate.width * 0.5) * inv_ui_scale;
                let top = (screen.y - NAMEPLATE_FONT_PX * 1.6) * inv_ui_scale;
                let left = Val::Px(left);
                let top = Val::Px(top);
                if node.left != left {
                    node.left = left;
                }
                if node.top != top {
                    node.top = top;
                }
                if *visibility != Visibility::Visible {
                    *visibility = Visibility::Visible;
                }
            }
            // Behind the camera or outside its depth range.
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

/// Remove a label whose avatar is gone (e.g. despawned by another system).
pub(crate) fn despawn_orphan_nameplates(
    mut commands: Commands,
    avatars: Query<Entity, With<RemotePlayer>>,
    plates: Query<(Entity, &RemoteNameplate), Without<RemotePlayer>>,
) {
    let live: HashSet<Entity> = avatars.iter().collect();
    for (plate_entity, plate) in &plates {
        if !live.contains(&plate.owner) {
            // The owner was despawned (normally `despawn_avatar` already took
            // the label with it, but a system outside this module may despawn
            // an avatar), so the label must not outlive it.
            commands.entity(plate_entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::MinimalPlugins;

    fn avatar_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        // No renderer here, but the plugin only needs the asset collections to
        // build its meshes/materials (and the timeline for interpolation).
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.init_resource::<RemoteFeed>();
        app.add_plugins(RemotePlayersPlugin);
        app
    }

    fn snapshot(tick: u32, players: Vec<PlayerState>) -> Snapshot {
        Snapshot {
            tick,
            server_ms: i64::from(tick) * 16,
            players,
            dungeon_version: 0,
        }
    }

    fn set_feed(app: &mut App, self_sub: &str, snap: Snapshot) {
        let mut feed = app.world_mut().resource_mut::<RemoteFeed>();
        feed.self_sub = Some(self_sub.to_string());
        feed.snapshot = Some(snap);
    }

    fn set_feed_without_identity(app: &mut App, snap: Snapshot) {
        let mut feed = app.world_mut().resource_mut::<RemoteFeed>();
        feed.self_sub = None;
        feed.snapshot = Some(snap);
    }

    fn avatars(app: &mut App) -> Vec<(String, Vec3)> {
        let mut query = app.world_mut().query::<(&RemotePlayer, &Transform)>();
        query
            .iter(app.world())
            .map(|(remote, transform)| (remote.sub.clone(), transform.translation))
            .collect()
    }

    fn avatar_position(app: &mut App, sub: &str) -> Option<Vec3> {
        avatars(app)
            .into_iter()
            .find(|(candidate, _)| candidate == sub)
            .map(|(_, position)| position)
    }

    fn nameplate_count(app: &mut App) -> usize {
        let mut query = app.world_mut().query_filtered::<Entity, With<RemoteNameplate>>();
        query.iter(app.world()).count()
    }

    fn nameplate_owner(app: &mut App, sub: &str) -> Option<Entity> {
        let avatar = {
            let mut query = app.world_mut().query::<(Entity, &RemotePlayer)>();
            query
                .iter(app.world())
                .find(|(_, remote)| remote.sub == sub)
                .map(|(entity, _)| entity)
        }?;
        let mut query = app.world_mut().query::<&RemoteNameplate>();
        query
            .iter(app.world())
            .find(|plate| plate.owner == avatar)
            .map(|plate| plate.owner)
    }

    /// Set the interpolation progress explicitly, so tests never depend on
    /// wall-clock frame deltas.
    fn set_alpha(app: &mut App, sub: &str, alpha: f32) {
        let mut query = app.world_mut().query::<&mut RemotePlayer>();
        let mut remote = query
            .iter_mut(app.world_mut())
            .find(|remote| remote.sub == sub)
            .expect("avatar exists");
        remote.interp_alpha = alpha;
    }

    fn nameplate_label(app: &mut App, sub: &str) -> Option<String> {
        let avatar = {
            let mut query = app.world_mut().query::<(Entity, &RemotePlayer)>();
            query
                .iter(app.world())
                .find(|(_, remote)| remote.sub == sub)
                .map(|(entity, _)| entity)
        }?;
        let plate = {
            let mut query = app.world_mut().query::<(Entity, &RemoteNameplate)>();
            query
                .iter(app.world())
                .find(|(_, plate)| plate.owner == avatar)
                .map(|(entity, _)| entity)
        }?;
        app.world().entity(plate).get::<Text>().map(|text| text.0.clone())
    }

    fn avatar_state(app: &mut App, sub: &str) -> (Option<PlayerState>, Option<PlayerState>) {
        let mut query = app.world_mut().query::<&RemotePlayer>();
        let remote = query
            .iter(app.world())
            .find(|remote| remote.sub == sub)
            .expect("avatar exists");
        (remote.interp_from.clone(), remote.interp_to.clone())
    }

    /// The bug this module fixes: two players in one world, neither rendering
    /// the other.
    #[test]
    fn snapshot_spawns_an_avatar_per_other_player() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 4.0, 0.0, 6.0, std::f32::consts::FRAC_PI_2);
        set_feed(&mut app, "me", snapshot(10, vec![me, bob]));
        app.update();
        app.update();

        let spawned = avatars(&mut app);
        assert_eq!(spawned.len(), 1, "expected exactly one avatar: {spawned:?}");
        assert_eq!(spawned[0].0, "bob");
        // Positioned at the remote's feet + capsule half-height.
        assert!((spawned[0].1.x - 4.0).abs() < 1e-5);
        assert!((spawned[0].1.z - 6.0).abs() < 1e-5);
        assert!((spawned[0].1.y - PILL_HALF).abs() < 1e-5);
        // Every avatar gets exactly one nameplate, owned by that avatar.
        assert_eq!(nameplate_count(&mut app), 1);
        assert!(nameplate_owner(&mut app, "bob").is_some());
    }

    /// The acceptance case: two other players, one of them moving. The moving
    /// avatar's interpolation target and rendered pose advance with snapshots,
    /// and both avatars (plus labels) disappear after the grace period.
    #[test]
    fn two_players_one_moving_track_and_expire() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let mut bob = player("bob", "Bob", 3.0, 0.0, 0.0, 0.0);
        let cara = player("cara", "Cara", -2.0, 0.0, 1.0, 0.0);
        set_feed(
            &mut app,
            "me",
            snapshot(1, vec![me.clone(), bob.clone(), cara.clone()]),
        );
        app.update();

        let spawned = avatars(&mut app);
        assert_eq!(spawned.len(), 2, "both other players need an avatar: {spawned:?}");
        assert_eq!(nameplate_count(&mut app), 2);
        assert!((avatar_position(&mut app, "bob").unwrap().x - 3.0).abs() < 1e-5);
        assert!((avatar_position(&mut app, "cara").unwrap().x + 2.0).abs() < 1e-5);

        // Bob moves: the new snapshot becomes the interpolation target and the
        // old pose the origin. Driving alpha to 1 puts the avatar at the new
        // pose; the idle player stays put.
        bob.x = 9.0;
        set_feed(
            &mut app,
            "me",
            snapshot(2, vec![me.clone(), bob.clone(), cara.clone()]),
        );
        app.update();
        let (from, to) = avatar_state(&mut app, "bob");
        assert_eq!(from.as_ref().map(|state| state.x), Some(3.0));
        assert_eq!(to.as_ref().map(|state| state.x), Some(9.0));
        set_alpha(&mut app, "bob", 1.0);
        app.update();
        assert!(
            (avatar_position(&mut app, "bob").unwrap().x - 9.0).abs() < 1e-4,
            "bob should have moved to x=9"
        );
        assert!((avatar_position(&mut app, "cara").unwrap().x + 2.0).abs() < 1e-4);

        // Both leave. One missed snapshot is tolerated; past the grace both the
        // avatars and their nameplates are gone.
        set_feed(&mut app, "me", snapshot(3, vec![me.clone()]));
        app.update();
        assert_eq!(avatars(&mut app).len(), 2, "one missed snapshot must not pop");

        for tick in 4..(4 + MISSED_TICKS_MAX + 5) {
            set_feed(&mut app, "me", snapshot(tick, vec![me.clone()]));
            app.update();
        }
        assert!(avatars(&mut app).is_empty(), "stale avatars were not despawned");
        assert_eq!(nameplate_count(&mut app), 0, "stale nameplates were not despawned");
    }

    /// A single dropped snapshot must not make the other player flicker out.
    #[test]
    fn one_missed_snapshot_keeps_the_avatar() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 1.0, 0.0, 1.0, 0.0);
        set_feed(&mut app, "me", snapshot(1, vec![me.clone(), bob.clone()]));
        app.update();
        set_feed(&mut app, "me", snapshot(2, vec![me.clone()]));
        app.update();
        assert_eq!(avatars(&mut app).len(), 1);
        assert_eq!(nameplate_count(&mut app), 1);
    }

    /// The identity arrives from `Welcome`, which can land after the first
    /// snapshots. Guessing "the first entry is me" would spawn an avatar of
    /// ourselves, so avatars are held until the subject is known.
    #[test]
    fn avatars_wait_for_identity_and_never_include_self() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 1.0, 0.0, 1.0, 0.0);

        set_feed_without_identity(&mut app, snapshot(1, vec![me.clone(), bob.clone()]));
        app.update();
        assert!(
            avatars(&mut app).is_empty(),
            "must not spawn an avatar before we know which subject is ours"
        );

        // `Welcome` lands on the next frame.
        set_feed(&mut app, "me", snapshot(2, vec![me, bob]));
        app.update();
        let spawned = avatars(&mut app);
        assert_eq!(spawned.len(), 1, "only bob: {spawned:?}");
        assert_eq!(spawned[0].0, "bob");
    }

    /// The feed is allowed to publish another snapshot for the same tick (a
    /// duplicate packet carrying a new subject, or a restarted session). The
    /// gate must not swallow it: that was the "avatar never appears while the
    /// world keeps ticking" failure mode.
    #[test]
    fn a_changed_snapshot_with_the_same_tick_is_still_applied() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 1.0, 0.0, 1.0, 0.0);
        let cara = player("cara", "Cara", -1.0, 0.0, 2.0, 0.0);

        set_feed(&mut app, "me", snapshot(7, vec![me.clone(), bob.clone()]));
        app.update();
        assert_eq!(avatars(&mut app).len(), 1);

        // Same tick, but the roster changed: cara must appear.
        set_feed(&mut app, "me", snapshot(7, vec![me, bob, cara]));
        app.update();
        let spawned = avatars(&mut app);
        assert_eq!(spawned.len(), 2, "same-tick roster change was dropped: {spawned:?}");
    }

    /// A controlled session restart (sim tick back to 0) must not freeze the
    /// renderer until the tick numbers catch up with the old session.
    #[test]
    fn a_tick_rewind_starts_a_fresh_snapshot_stream() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 1.0, 0.0, 1.0, 0.0);
        set_feed(&mut app, "me", snapshot(50_000, vec![me.clone(), bob.clone()]));
        app.update();
        assert_eq!(avatars(&mut app).len(), 1);

        // New sim session: tick 1, bob is now 5 m away.
        let mut moved = bob.clone();
        moved.x = 5.0;
        set_feed(&mut app, "me", snapshot(1, vec![me, moved]));
        app.update();
        assert_eq!(avatar_state(&mut app, "bob").1.map(|state| state.x), Some(5.0));
    }

    /// A remote's display name can change after the join (the sim persists the
    /// claimed name), and the label must follow instead of freezing.
    #[test]
    fn a_renamed_player_retitles_its_nameplate() {
        let mut app = avatar_app();
        let me = player("me", "Me", 0.0, 0.0, 0.0, 0.0);
        let bob = player("bob", "Bob", 1.0, 0.0, 1.0, 0.0);
        set_feed(&mut app, "me", snapshot(1, vec![me.clone(), bob.clone()]));
        app.update();
        assert_eq!(nameplate_label(&mut app, "bob").as_deref(), Some("Bob"));

        let renamed = player("bob", "Bobby", 1.0, 0.0, 1.0, 0.0);
        set_feed(&mut app, "me", snapshot(2, vec![me, renamed]));
        app.update();
        assert_eq!(nameplate_label(&mut app, "bob").as_deref(), Some("Bobby"));
    }

    fn player(sub: &str, name: &str, x: f32, y: f32, z: f32, yaw: f32) -> PlayerState {
        PlayerState {
            sub: sub.to_string(),
            name: name.to_string(),
            x,
            y,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw,
            hp: 100.0,
            max_hp: 100.0,
            alive: true,
            online: true,
        }
    }

    #[test]
    fn avatar_centre_sits_on_the_feet() {
        let state = player("a", "Ann", 3.0, 7.0, -2.0, 0.0);
        let pose = pose_of(&state);
        assert_eq!(pose.x, 3.0);
        assert_eq!(pose.z, -2.0);
        assert!((pose.y - (7.0 + PILL_HALF)).abs() < 1e-6);
    }

    #[test]
    fn interpolation_hits_both_snapshot_poses() {
        let from = player("a", "Ann", 0.0, 0.0, 0.0, 0.0);
        let to = player("a", "Ann", 10.0, 0.0, 0.0, 0.0);
        assert_eq!(lerp_pose(&from, &to, 0.0), pose_of(&from));
        assert_eq!(lerp_pose(&from, &to, 1.0), pose_of(&to));
        let mid = lerp_pose(&from, &to, 0.5);
        assert!((mid.x - 5.0).abs() < 1e-5);
    }

    /// Turning across the ±180° seam must take the short way round.
    #[test]
    fn yaw_interpolation_takes_the_short_arc() {
        let from = 170f32.to_radians();
        let to = -170f32.to_radians();
        let mid = angle_lerp(from, to, 0.5).to_degrees();
        // Halfway is 180°, not 0°.
        assert!(
            (mid.abs() - 180.0).abs() < 1e-3,
            "expected ±180 at the midpoint, got {mid}"
        );
    }

    #[test]
    fn colours_are_stable_and_distinct() {
        assert_eq!(color_for_sub("alice"), color_for_sub("alice"));
        assert_ne!(color_for_sub("alice"), color_for_sub("bob"));
    }

    #[test]
    fn remote_subject_set_is_sorted_and_excludes_self() {
        let snap = snapshot(
            1,
            vec![
                player("zed", "Zed", 0.0, 0.0, 0.0, 0.0),
                player("me", "Me", 0.0, 0.0, 0.0, 0.0),
                player("amy", "Amy", 0.0, 0.0, 0.0, 0.0),
            ],
        );
        assert_eq!(
            sorted_remote_subs(&snap, Some("me")),
            vec!["amy".to_string(), "zed".to_string()]
        );
        assert_eq!(sorted_remote_subs(&snap, None).len(), 3);
    }

    /// A remote that misses snapshots is kept for a grace period, then dropped.
    #[test]
    fn missing_remotes_are_tolerated_then_despawned() {
        // Grace is measured in server ticks: a stale avatar is dropped once the
        // snapshot tick has advanced past the last tick it was seen in.
        assert!(tick_diff(MISSED_TICKS_MAX + 1, 0) > MISSED_TICKS_MAX as i32);
        assert!(tick_diff(MISSED_TICKS_MAX, 0) <= MISSED_TICKS_MAX as i32);
        // Tick wrap is handled like every other tick comparison.
        assert!(tick_diff(5, u32::MAX - 80) > 0);
    }
}
