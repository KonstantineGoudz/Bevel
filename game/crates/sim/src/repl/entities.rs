//! Replicated ECS entities maintained by the embedded `bevy_replicon` server.
//!
//! The Tokio authority thread sends [`ServerFrame`]s and session/reply commands;
//! this module owns the server-side ECS: one replicated entity per world piece
//! (`PieceId` + `PieceData` + `WorldCell`), one per player (`PlayerSub` +
//! `PlayerName` + `PlayerPose` + `WorldCell`), and each authorized client's
//! `VisibleCells` interest set. Replicon itself handles delta encoding, packet
//! splitting, acking and the visibility filtering declared by
//! `party_dungeon_replication::WorldCell`.
//!
//! Change detection is the replication trigger, so every mutation here compares
//! first: a pose that did not move must not be re-sent, and a piece that did not
//! change must not be re-serialized.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use party_dungeon_replication::{
    EditAppliedMsg, EditRejectedMsg, NoticeMsg, PieceData, PieceId, PiecePath, PlayerCell,
    PlayerName, PlayerPose, PlayerSub, VisibleCells, VisiblePlayerCells, WelcomeMsg, WorldCell,
};

use crate::authority::EditReply;
use crate::repl::protocol::{
    ClientIntent, PlayerView, ServerCommand, ServerFrame,
};

/// Rid → replicated piece entity.
#[derive(Resource, Default)]
pub struct WorldEntities {
    pub pieces: HashMap<String, Entity>,
    pub players: HashMap<String, Entity>,
}

/// The bridge resources shared with the Tokio thread.
///
/// The command receiver sits in a `Mutex` because `std::sync::mpsc::Receiver`
/// is `Send` but not `Sync`, and Bevy resources must be both.
#[derive(Resource)]
pub struct ReplBridge {
    pub commands: std::sync::Mutex<Receiver<ServerCommand>>,
    pub intents: Sender<ClientIntent>,
}

/// Interest radii, mirroring the legacy `DUNGEON_WORLD_RADIUS` /
/// `DUNGEON_PLAYER_RADIUS` knobs (player radius is informational for replicon:
/// visibility is cell-based, but it is logged and reported in `WelcomeMsg`).
#[derive(Resource, Clone, Debug)]
pub struct ReplConfig {
    pub world_radius: f32,
    pub player_radius: f32,
}

/// Sub → `ConnectedClient` entity (and the reverse), plus the last cell we
/// pushed into that client's `VisibleCells` (avoids re-insert churn).
#[derive(Resource, Default)]
pub struct SessionEntities {
    by_sub: HashMap<String, Entity>,
    by_entity: HashMap<Entity, String>,
    last_cell: HashMap<String, u16>,
}

impl SessionEntities {
    /// Register a freshly authorized client. Returns the entity previously
    /// mapped to this sub (a reconnect of the same account).
    pub fn insert_session(&mut self, sub: String, entity: Entity) -> Option<Entity> {
        let previous = self.by_sub.insert(sub.clone(), entity);
        if let Some(old) = previous.filter(|old| *old != entity) {
            self.by_entity.remove(&old);
        }
        self.by_entity.insert(entity, sub);
        previous
    }

    /// Drop a client by entity; returns its sub when it was mapped.
    pub fn remove_entity(&mut self, entity: Entity) -> Option<String> {
        let sub = self.by_entity.remove(&entity)?;
        if self.by_sub.get(&sub) == Some(&entity) {
            self.by_sub.remove(&sub);
            self.last_cell.remove(&sub);
        }
        Some(sub)
    }

    pub fn client_for(&self, sub: &str) -> Option<Entity> {
        self.by_sub.get(sub).copied()
    }

    pub fn sub_for(&self, entity: Entity) -> Option<&str> {
        self.by_entity.get(&entity).map(String::as_str)
    }

    pub fn visible_cell(&self, sub: &str) -> Option<u16> {
        self.last_cell.get(sub).copied()
    }

    pub fn set_visible_cell(&mut self, sub: &str, cell: u16) {
        self.last_cell.insert(sub.to_string(), cell);
    }

    pub fn len(&self) -> usize {
        self.by_sub.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_sub.is_empty()
    }
}

/// Tracks the authoritative tick/version currently represented in the ECS
/// (used for logs and for the `WelcomeMsg` the authority fills in).
#[derive(Resource, Default)]
pub struct DungeonMeta {
    pub tick: u32,
    pub dungeon_version: u32,
}

/// Apply every command the Tokio authority produced this frame.
#[allow(clippy::too_many_arguments)]
pub fn apply_server_commands(
    mut commands: Commands,
    bridge: Res<ReplBridge>,
    mut entities: ResMut<WorldEntities>,
    mut sessions: ResMut<SessionEntities>,
    config: Res<ReplConfig>,
    mut meta: ResMut<DungeonMeta>,
    mut pieces: Query<(&mut PieceData, &WorldCell)>,
    mut players: Query<(&mut PlayerName, &mut PlayerPose, &PlayerCell)>,
    mut welcomes: MessageWriter<ToClients<WelcomeMsg>>,
    mut applied: MessageWriter<ToClients<EditAppliedMsg>>,
    mut rejected: MessageWriter<ToClients<EditRejectedMsg>>,
    mut notices: MessageWriter<ToClients<NoticeMsg>>,
) {
    let Ok(commands_rx) = bridge.commands.lock() else {
        return;
    };
    while let Ok(command) = commands_rx.try_recv() {
        match command {
            ServerCommand::Frame(frame) => {
                apply_frame(&mut commands, &mut entities, &mut pieces, &mut players, frame);
            }
            ServerCommand::Joined {
                sub,
                name,
                tick,
                dungeon_version,
                world_radius,
                x,
                z,
            } => {
                let Some(client) = sessions.client_for(&sub) else {
                    warn!("repl: joined sub={sub} has no connected client; ignoring");
                    continue;
                };
                let cell = WorldCell::containing(x, z);
                // Two interest filters: world geometry at `world_radius`,
                // other players at `player_radius` (the legacy radii).
                commands.entity(client).insert((
                    VisibleCells::around(x, z, world_radius),
                    VisiblePlayerCells(
                        VisibleCells::around(x, z, config.player_radius).0,
                    ),
                ));
                sessions.set_visible_cell(&sub, cell.0);
                meta.tick = tick;
                meta.dungeon_version = dungeon_version;
                welcomes.write(ToClients {
                    targets: SendTargets::Single(ClientId::Client(client)),
                    message: WelcomeMsg {
                        sub: sub.clone(),
                        name: name.clone(),
                        tick,
                        dungeon_version,
                        world_radius,
                    },
                });
                info!(
                    "repl: welcome sub={sub} name={name} tick={tick} version={dungeon_version} world_radius={world_radius}",
                );
            }
            ServerCommand::EditReply { sub, reply } => {
                let Some(client) = sessions.client_for(&sub) else {
                    continue;
                };
                match reply {
                    EditReply::Applied { tag, rid } => {
                        applied.write(ToClients {
                            targets: SendTargets::Single(ClientId::Client(client)),
                            message: EditAppliedMsg { tag: tag.clone(), rid: rid.clone() },
                        });
                        debug!("repl: edit applied sub={sub} tag={tag} rid={rid}");
                    }
                    EditReply::Rejected { key, reason } => {
                        rejected.write(ToClients {
                            targets: SendTargets::Single(ClientId::Client(client)),
                            message: EditRejectedMsg {
                                key: key.clone(),
                                reason: reason.clone(),
                            },
                        });
                        warn!("repl: edit rejected sub={sub} key={key} reason={reason}");
                    }
                }
            }
            ServerCommand::PlayerLeft { sub } => {
                if let Some(entity) = entities.players.remove(&sub) {
                    commands.entity(entity).despawn();
                }
            }
            ServerCommand::Notice(text) => {
                debug!("repl: notice {text}");
                notices.write(ToClients {
                    targets: SendTargets::All,
                    message: NoticeMsg(text),
                });
            }
        }
    }
}

fn apply_frame(
    commands: &mut Commands,
    entities: &mut WorldEntities,
    pieces: &mut Query<(&mut PieceData, &WorldCell)>,
    players: &mut Query<(&mut PlayerName, &mut PlayerPose, &PlayerCell)>,
    frame: ServerFrame,
) {
    for (rid, data) in frame.pieces.upsert {
        let cell = WorldCell::containing(data.x(), data.z());
        match entities.pieces.get(&rid).copied() {
            Some(entity) => {
                if let Ok((mut current, current_cell)) = pieces.get_mut(entity) {
                    if *current != data {
                        *current = data;
                    }
                    if *current_cell != cell {
                        commands.entity(entity).insert(cell);
                    }
                }
            }
            None => {
                let entity = commands
                    .spawn((Replicated, PieceId(rid.clone()), data, cell))
                    .id();
                entities.pieces.insert(rid, entity);
            }
        }
    }
    // Mover tracks are their own component on the piece entity; upsert after
    // the pieces so a freshly spawned entity already exists.
    for (rid, path) in frame.pieces.paths {
        if let Some(entity) = entities.pieces.get(&rid).copied() {
            commands.entity(entity).insert(path);
        }
    }
    for rid in frame.pieces.path_remove {
        if let Some(entity) = entities.pieces.get(&rid).copied() {
            commands.entity(entity).remove::<PiecePath>();
        }
    }
    for rid in frame.pieces.remove {
        if let Some(entity) = entities.pieces.remove(&rid) {
            commands.entity(entity).despawn();
        }
    }
    for view in frame.players {
        upsert_player(commands, entities, players, view);
    }
}

fn upsert_player(
    commands: &mut Commands,
    entities: &mut WorldEntities,
    players: &mut Query<(&mut PlayerName, &mut PlayerPose, &PlayerCell)>,
    view: PlayerView,
) {
    match entities.players.get(&view.sub).copied() {
        Some(entity) => {
            if let Ok((mut name, mut pose, cell)) = players.get_mut(entity) {
                if name.0 != view.name {
                    name.0 = view.name;
                }
                if *pose != view.pose {
                    *pose = view.pose;
                }
                if *cell != view.cell {
                    commands.entity(entity).insert(view.cell);
                }
            }
        }
        None => {
            let entity = commands
                .spawn((
                    Replicated,
                    PlayerSub(view.sub.clone()),
                    PlayerName(view.name),
                    view.pose,
                    view.cell,
                ))
                .id();
            entities.players.insert(view.sub, entity);
        }
    }
}

/// Re-insert each authorized client's `VisibleCells` when its own player crosses
/// a cell boundary (`VisibleCells` is an immutable replicon client component, so
/// a new interest set is an insert, not a mutation).
pub fn sync_visibility(
    mut commands: Commands,
    config: Res<ReplConfig>,
    mut sessions: ResMut<SessionEntities>,
    moved: Query<(&PlayerSub, &PlayerPose, &PlayerCell), Changed<PlayerCell>>,
) {
    for (sub, pose, cell) in &moved {
        let Some(client) = sessions.client_for(&sub.0) else {
            continue;
        };
        if sessions.visible_cell(&sub.0) == Some(cell.0) {
            continue;
        }
        let z = crate::repl::convert::pose_z(pose);
        commands.entity(client).insert((
            VisibleCells::around(pose.x(), z, config.world_radius),
            VisiblePlayerCells(VisibleCells::around(pose.x(), z, config.player_radius).0),
        ));
        sessions.set_visible_cell(&sub.0, cell.0);
        debug!(
            "repl: visibility sub={} cell={} world_radius={} player_radius={}",
            sub.0, cell.0, config.world_radius, config.player_radius
        );
    }
}

/// Forward replicated input ticks to the authority thread.
pub fn forward_inputs(
    mut messages: MessageReader<FromClient<party_dungeon_replication::InputMsg>>,
    sessions: Res<SessionEntities>,
    bridge: Res<ReplBridge>,
) {
    for message in messages.read() {
        let Some(client) = message.client_id.entity() else {
            continue; // listen-server sender: no transport client
        };
        let Some(sub) = sessions.sub_for(client) else {
            continue; // authorized but our join bookkeeping has not run yet
        };
        let intent = ClientIntent::Input {
            sub: sub.to_string(),
            input: message.message.0,
        };
        if bridge.intents.send(intent).is_err() {
            break; // authority thread gone (shutdown)
        }
    }
}

/// Forward replicated world-edit batches to the authority thread.
pub fn forward_world_edits(
    mut messages: MessageReader<FromClient<party_dungeon_replication::WorldEditMsg>>,
    sessions: Res<SessionEntities>,
    bridge: Res<ReplBridge>,
) {
    for message in messages.read() {
        let Some(client) = message.client_id.entity() else {
            continue;
        };
        let Some(sub) = sessions.sub_for(client) else {
            continue;
        };
        let intent = ClientIntent::WorldEdit {
            sub: sub.to_string(),
            ops: message.message.ops.clone(),
        };
        debug!(
            "repl: world edit sub={sub} ops={}",
            message.message.ops.len()
        );
        if bridge.intents.send(intent).is_err() {
            break;
        }
    }
}

/// Detect disconnects (the backend removes `ClientSession` when it tears a peer
/// down) and tell the authority so it can persist the offline flag.
pub fn detect_disconnects(
    mut removed: RemovedComponents<crate::repl::backend_glue::ClientSession>,
    mut sessions: ResMut<SessionEntities>,
    bridge: Res<ReplBridge>,
) {
    for entity in removed.read() {
        if let Some(sub) = sessions.remove_entity(entity) {
            eprintln!("sim: replicon client disconnected sub={sub}");
            let _ = bridge.intents.send(ClientIntent::Leave { sub });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_map_tracks_both_directions() {
        let mut sessions = SessionEntities::default();
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        assert_eq!(sessions.insert_session("alice".into(), a), None);
        assert_eq!(sessions.client_for("alice"), Some(a));
        assert_eq!(sessions.sub_for(a), Some("alice"));
        assert_eq!(sessions.len(), 1);

        // Reconnect: the same sub maps to the new entity, the old one is gone.
        assert_eq!(sessions.insert_session("alice".into(), b), Some(a));
        assert_eq!(sessions.client_for("alice"), Some(b));
        assert_eq!(sessions.sub_for(a), None);
        assert_eq!(sessions.remove_entity(b), Some("alice".into()));
        assert_eq!(sessions.sub_for(b), None);
        assert!(sessions.is_empty());
        assert_eq!(sessions.remove_entity(b), None);
    }

    #[test]
    fn visible_cell_bookkeeping() {
        let mut sessions = SessionEntities::default();
        let a = Entity::from_raw_u32(3).unwrap();
        sessions.insert_session("bob".into(), a);
        assert_eq!(sessions.visible_cell("bob"), None);
        sessions.set_visible_cell("bob", 42);
        assert_eq!(sessions.visible_cell("bob"), Some(42));
        sessions.remove_entity(a);
        assert_eq!(sessions.visible_cell("bob"), None);
    }
}
