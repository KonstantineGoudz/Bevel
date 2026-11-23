//! Tokio-side replicon path: the authoritative loop plus the cross-thread
//! mirror into the embedded `bevy_replicon` server app.
//!
//! This is the *same* authority as the legacy path — [`Sim`], the 60 Hz step,
//! [`Authority`] persistence and the DB hot-reload poll — with the transport
//! swapped from `HostNet` to `bevy_replicon` running on a dedicated thread.
//! Nothing about gameplay changes; only how state reaches clients.

use std::collections::HashMap;
use std::time::Duration;

use party_dungeon_net::{Input, TICK_DT};
use party_dungeon_replication::{PieceData, PiecePath, PlayerCell};

use crate::authority::Authority;
use crate::repl::app::{self, ReplServerChannels, ReplServerConfig};
use crate::repl::convert;
use crate::repl::protocol::{
    ClientIntent, PieceDelta, PlayerView, ServerCommand, ServerFrame,
};
use crate::sim::{Sim, SimEvent};

/// Run the authoritative loop with the embedded `bevy_replicon` server.
///
/// Blocks until the replication thread disappears (shutdown / fatal transport
/// error). The signaling server and auth API are already running — `main`
/// starts them for both paths so `/party-dungeon` stays the same room.
pub async fn run(
    mut sim: Sim,
    mut authority: Authority,
    config: ReplServerConfig,
) -> anyhow::Result<()> {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (intent_tx, intent_rx) = std::sync::mpsc::channel();
    let _server_thread = app::spawn_server_thread(
        config.clone(),
        ReplServerChannels {
            commands: command_rx,
            intents: intent_tx,
        },
        Some(crate::repl::backend_glue::jwt_authenticator()),
    )?;

    let mut inputs: HashMap<String, Input> = HashMap::new();
    // Mirror of the last replicated piece/path set, for the version-gated delta.
    let mut replicated: HashMap<String, PieceData> = HashMap::new();
    let mut replicated_paths: HashMap<String, PiecePath> = HashMap::new();
    // 0 forces the first frame to carry the whole world.
    let mut last_version = 0u32;

    let mut tick_interval = tokio::time::interval(Duration::from_secs_f32(TICK_DT));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    loop {
        tick_interval.tick().await;

        // 1) Drain the replication thread's client intents. Inputs are just
        //    latest-per-sub (the same map `Sim::step` consumed before); edits
        //    go through the shared authority (persist → apply → bump), joins
        //    persist the character row and answer with the session player.
        while let Ok(intent) = intent_rx.try_recv() {
            match intent {
                ClientIntent::Join { sub, name } => {
                    let claimed = if name.trim().is_empty() {
                        None
                    } else {
                        Some(name)
                    };
                    let display = authority.join(&sub, claimed).await;
                    let name = display.unwrap_or_else(|| sub.clone());
                    sim.join(&sub, &name, 100.0, 100.0);
                    let (x, z) = sim
                        .players
                        .get(&sub)
                        .map(|player| (player.pos.x, player.pos.y))
                        .unwrap_or((sim.spawn.x, sim.spawn.y));
                    let joined = ServerCommand::Joined {
                        sub: sub.clone(),
                        name,
                        tick: sim.tick as u32,
                        dungeon_version: u32::try_from(sim.dungeon_version).unwrap_or(u32::MAX),
                        world_radius: config.world_radius,
                        x,
                        z,
                    };
                    if command_tx.send(joined).is_err() {
                        eprintln!("sim: replicon thread gone (join); stopping");
                        return Ok(());
                    }
                }
                ClientIntent::Input { sub, input } => {
                    inputs.insert(sub, input);
                }
                ClientIntent::WorldEdit { sub, ops } => {
                    let replies = authority.apply_world_edit(&sub, &ops, &mut sim).await;
                    for reply in replies {
                        if command_tx
                            .send(ServerCommand::EditReply {
                                sub: sub.clone(),
                                reply,
                            })
                            .is_err()
                        {
                            eprintln!("sim: replicon thread gone (edit reply); stopping");
                            return Ok(());
                        }
                    }
                }
                ClientIntent::Leave { sub } => {
                    inputs.remove(&sub);
                    sim.leave(&sub);
                    authority.leave(&sub).await;
                    let _ = command_tx.send(ServerCommand::PlayerLeft { sub });
                }
            }
        }

        // 2) Authoritative step — identical to the legacy loop.
        for event in sim.step(&inputs) {
            match event {
                SimEvent::Kill { target, by } => {
                    eprintln!("sim: kill {target} by {by} at tick {}", sim.tick);
                    authority.log_death(&by, &target, sim.tick).await;
                    let _ = command_tx.send(ServerCommand::Notice(format!(
                        "{target} was slain by {by}"
                    )));
                }
                SimEvent::Respawn { sub } => {
                    eprintln!("sim: respawn {sub}");
                }
                SimEvent::InteractToggled { piece_rid, paused } => {
                    eprintln!("sim: toggle {piece_rid} paused={paused}");
                    // `Sim::step` already bumped `dungeon_version`; the frame
                    // below picks the changed mover cell up in the piece delta.
                }
            }
        }

        // 3) Out-of-band (MCP/DB) edits: unchanged 2 s cadence.
        if sim.tick % 120 == 0 && authority.poll_edits(&mut sim).await {
            eprintln!("sim: dungeon changed -> version {}", sim.dungeon_version);
        }

        // 4) Mirror the authoritative state into the replication thread.
        let version = u32::try_from(sim.dungeon_version).unwrap_or(u32::MAX);
        let pieces = if version != last_version {
            last_version = version;
            piece_delta(&sim, &mut replicated, &mut replicated_paths)
        } else {
            PieceDelta::default()
        };
        let frame = ServerFrame {
            tick: sim.tick as u32,
            dungeon_version: version,
            pieces,
            players: player_views(&sim),
        };
        if command_tx.send(ServerCommand::Frame(frame)).is_err() {
            eprintln!("sim: replicon thread gone (frame); stopping");
            return Ok(());
        }
    }
}

/// Diff the authoritative piece set against what has been replicated.
///
/// Only runs when `dungeon_version` moved (edits, mover pause toggles, hot
/// reloads), so the per-tick cost on an idle server is one version compare.
fn piece_delta(
    sim: &Sim,
    replicated: &mut HashMap<String, PieceData>,
    replicated_paths: &mut HashMap<String, PiecePath>,
) -> PieceDelta {
    let mut current: HashMap<String, PieceData> = HashMap::with_capacity(sim.pieces.len());
    for (rid, piece) in &sim.pieces {
        current.insert(rid.clone(), convert::piece_data(piece));
    }
    // Mover tracks are runtime state (`sim.world.movers`), so they follow the
    // sim — including an interact pause toggle — and disappear with the piece.
    let mut current_paths: HashMap<String, PiecePath> =
        HashMap::with_capacity(sim.world.movers.len());
    for (rid, mover) in &sim.world.movers {
        current_paths.insert(rid.clone(), convert::piece_path(mover));
    }
    let mut delta = PieceDelta::default();
    for (rid, data) in &current {
        if replicated.get(rid) != Some(data) {
            delta.upsert.push((rid.clone(), *data));
        }
    }
    for (rid, path) in &current_paths {
        if replicated_paths.get(rid) != Some(path) {
            delta.paths.push((rid.clone(), path.clone()));
        }
    }
    for rid in replicated_paths.keys() {
        if !current_paths.contains_key(rid) {
            delta.path_remove.push(rid.clone());
        }
    }
    for rid in replicated.keys() {
        if !current.contains_key(rid) {
            delta.remove.push(rid.clone());
        }
    }
    // Deterministic order keeps the cross-thread stream stable (and the tests
    // comparable run to run); replicon itself does not care about the order.
    delta.upsert.sort_by(|a, b| a.0.cmp(&b.0));
    delta.paths.sort_by(|a, b| a.0.cmp(&b.0));
    delta.path_remove.sort();
    delta.remove.sort();
    *replicated = current;
    *replicated_paths = current_paths;
    delta
}

/// One replicated view per player (online flag rides in the pose).
fn player_views(sim: &Sim) -> Vec<PlayerView> {
    let mut views: Vec<PlayerView> = sim
        .players
        .values()
        .map(|player| {
            let pose = convert::player_pose(player);
            PlayerView {
                sub: player.sub.clone(),
                name: player.name.clone(),
                cell: PlayerCell::containing(pose.x(), convert::pose_z(&pose)),
                pose,
            }
        })
        .collect();
    views.sort_by(|a, b| a.sub.cmp(&b.sub));
    views
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec2;
    use party_dungeon::model;
    use party_dungeon_net::WorldPiece;

    fn wire(rid: &str, shape: &str, x: f32, z: f32) -> WorldPiece {
        WorldPiece {
            rid: rid.to_string(),
            shape: shape.to_string(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: 0.0,
            interact: "none".to_string(),
            trigger_w: 2.0,
            trigger_h: 2.0,
            trigger_l: 2.0,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
        }
    }

    #[test]
    fn piece_delta_is_empty_when_the_world_did_not_change() {
        let mut sim = Sim::new();
        sim.rebuild(&[], &HashMap::new());
        let mut replicated = HashMap::new();
        let mut paths = HashMap::new();
        let first = piece_delta(&sim, &mut replicated, &mut paths);
        // The first sync of an empty world is empty (nothing to replicate).
        assert!(first.is_empty());

        assert!(sim.apply_wire_piece(&wire("dungeon_piece:a", "floor", 10.0, 10.0)));
        sim.rebuild_current();
        let added = piece_delta(&sim, &mut replicated, &mut paths);
        assert_eq!(added.upsert.len(), 1);
        assert_eq!(added.upsert[0].0, "dungeon_piece:a");
        assert!(added.remove.is_empty());

        // No version move means the caller never asks for a delta; asking again
        // with the same mirror is a no-op too.
        let unchanged = piece_delta(&sim, &mut replicated, &mut paths);
        assert!(unchanged.is_empty());
    }

    #[test]
    fn piece_delta_tracks_moves_and_deletes() {
        let mut sim = Sim::new();
        sim.rebuild(&[], &HashMap::new());
        let mut replicated = HashMap::new();
        let mut paths = HashMap::new();
        assert!(sim.apply_wire_piece(&wire("dungeon_piece:a", "floor", 0.0, 0.0)));
        assert!(sim.apply_wire_piece(&wire("dungeon_piece:b", "block", 4.0, 0.0)));
        sim.rebuild_current();
        assert_eq!(piece_delta(&sim, &mut replicated, &mut paths).upsert.len(), 2);

        // Move one in place: only that piece is an upsert.
        assert!(sim.apply_wire_piece(&wire("dungeon_piece:a", "floor", 40.0, 0.0)));
        sim.rebuild_current();
        let moved = piece_delta(&sim, &mut replicated, &mut paths);
        assert_eq!(moved.upsert.len(), 1);
        assert_eq!(moved.upsert[0].0, "dungeon_piece:a");
        assert!(moved.remove.is_empty());

        // Delete: only a removal, no bogus upsert.
        assert!(sim.delete_wire_piece("dungeon_piece:b"));
        sim.rebuild_current();
        let deleted = piece_delta(&sim, &mut replicated, &mut paths);
        assert!(deleted.upsert.is_empty());
        assert_eq!(deleted.remove, vec!["dungeon_piece:b".to_string()]);
    }

    #[test]
    fn piece_delta_carries_and_removes_mover_paths() {
        use party_dungeon::platforms::{PlatformMode, PlatformWaypoint};
        use crate::world::SimMoverSpec;

        let mut sim = Sim::new();
        sim.rebuild(&[], &HashMap::new());
        let mut paths = HashMap::new();
        paths.insert(
            "dungeon_piece:pad".to_string(),
            SimMoverSpec {
                waypoints: vec![
                    PlatformWaypoint::new(Vec2::new(0.0, 0.0)),
                    PlatformWaypoint::new(Vec2::new(4.0, 0.0)),
                ],
                closed: false,
                mode: PlatformMode::PingPong,
                speed: 1.5,
                paused: false,
                elevation: 0.0,
            },
        );
        let pieces = sim.convert_pieces(&[(String::from("dungeon_piece:pad"), party_dungeon_server::Piece {
            rid: "dungeon_piece:pad".into(),
            shape: "block".into(),
            x: 0.0,
            z: 0.0,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: 0.0,
            interact: "none".into(),
            trigger_w: 2.0,
            trigger_h: 2.0,
            trigger_l: 2.0,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
        })]);
        sim.rebuild(&pieces, &paths);
        let mut replicated = HashMap::new();
        let mut mirrored_paths = HashMap::new();
        let delta = piece_delta(&sim, &mut replicated, &mut mirrored_paths);
        assert_eq!(delta.paths.len(), 1);
        let (rid, path) = &delta.paths[0];
        assert_eq!(rid, "dungeon_piece:pad");
        assert_eq!(path.waypoints, vec![[0, 0], [400, 0]]);
        assert_eq!(path.speed_cm_s, 150);
        assert_eq!(path.mode, convert::path_mode_id(PlatformMode::PingPong));
        assert!(!path.paused);

        // Pausing the mover (interact) changes only the replicated path.
        sim.world.movers.get_mut("dungeon_piece:pad").unwrap().paused = true;
        let paused = piece_delta(&sim, &mut replicated, &mut mirrored_paths);
        assert!(paused.upsert.is_empty(), "piece geometry did not change");
        assert_eq!(paused.paths.len(), 1);
        assert!(paused.paths[0].1.paused);

        // Deleting the piece removes its track from the delta too.
        assert!(sim.delete_wire_piece("dungeon_piece:pad"));
        sim.rebuild_current();
        let removed = piece_delta(&sim, &mut replicated, &mut mirrored_paths);
        assert_eq!(removed.path_remove, vec!["dungeon_piece:pad".to_string()]);
        assert_eq!(removed.remove, vec!["dungeon_piece:pad".to_string()]);
    }

    #[test]
    fn player_views_carry_pose_cell_and_name() {
        let mut sim = Sim::new();
        sim.rebuild(&[], &HashMap::new());
        sim.join("zed", "Zed", 100.0, 100.0);
        sim.players.get_mut("zed").unwrap().pos = Vec2::new(40.0, -8.0);
        let views = player_views(&sim);
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert_eq!(view.sub, "zed");
        assert_eq!(view.name, "Zed");
        assert!((view.pose.x() - 40.0).abs() <= 0.005);
        assert_eq!(view.cell, PlayerCell::containing(40.0, -8.0));
    }
}
