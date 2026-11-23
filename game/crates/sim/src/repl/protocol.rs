//! Cross-thread protocol between the Tokio authority loop and the embedded
//! `bevy_replicon` server thread.
//!
//! The Tokio side owns the authoritative [`Sim`](crate::sim::Sim), the 60 Hz
//! step and SurrealDB persistence; the Bevy side owns the replicated ECS
//! entities and the matchbox transport. They share nothing but these plain
//! `std::sync::mpsc` messages — no locks, no Bevy types crossing the thread
//! boundary except the replication-crate components (which are plain data).
//!
//! Direction summary:
//!
//! * Tokio → Bevy ([`ServerCommand`]): one [`ServerFrame`] per tick (player
//!   poses every tick; piece deltas only when the world changed), session
//!   joins with their persisted player row, editor replies, and leaves.
//! * Bevy → Tokio ([`ClientIntent`]): replicated input ticks, authoritative
//!   world-edit batches, joins after backend authorization, and disconnects.

use std::sync::mpsc::{Receiver, Sender};

use party_dungeon_net::{Input, WorldOp};
use party_dungeon_replication::{PieceData, PiecePath, PlayerCell, PlayerPose};

use crate::authority::EditReply;

/// Tokio (authority) → Bevy (transport/ECS) side of the bridge.
pub type CommandSender = Sender<ServerCommand>;
pub type CommandReceiver = Receiver<ServerCommand>;
/// Bevy (transport/ECS) → Tokio (authority) side of the bridge.
pub type IntentSender = Sender<ClientIntent>;
pub type IntentReceiver = Receiver<ClientIntent>;

/// One changed-world delta. Built only when `dungeon_version` moved, so an
/// idle tick carries no piece data at all (player poses are the hot path).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PieceDelta {
    /// New or changed pieces, keyed by store rid (`dungeon_piece.rid`).
    pub upsert: Vec<(String, PieceData)>,
    /// New or changed mover tracks (only pieces with a platform path).
    pub paths: Vec<(String, PiecePath)>,
    /// Rids whose mover track disappeared.
    pub path_remove: Vec<String>,
    /// Deleted rids.
    pub remove: Vec<String>,
}

impl PieceDelta {
    pub fn is_empty(&self) -> bool {
        self.upsert.is_empty()
            && self.paths.is_empty()
            && self.path_remove.is_empty()
            && self.remove.is_empty()
    }
}

/// One replicated player. The cell is precomputed so the Bevy side never has
/// to re-derive the interest grid from a quantized pose.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub sub: String,
    pub name: String,
    pub pose: PlayerPose,
    /// Player interest cell ([`PlayerCell`] filter; the world has its own
    /// [`WorldCell`](party_dungeon_replication::WorldCell) filter).
    pub cell: PlayerCell,
}

/// The authoritative state of one 60 Hz tick, as handed to the Bevy thread.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerFrame {
    pub tick: u32,
    pub dungeon_version: u32,
    /// Non-empty only when the world changed since the previous frame.
    pub pieces: PieceDelta,
    pub players: Vec<PlayerView>,
}

/// One command for the embedded server app.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerCommand {
    /// Advance the replicated world/players.
    Frame(ServerFrame),
    /// A session was authorized and the authority persisted/joined its player.
    Joined {
        sub: String,
        name: String,
        tick: u32,
        dungeon_version: u32,
        world_radius: f32,
        /// Spawn position used to initialize the client's `VisibleCells`.
        x: f32,
        z: f32,
    },
    /// Answer one authored edit op to its author.
    EditReply { sub: String, reply: EditReply },
    /// The player left; despawn its replicated entity.
    PlayerLeft { sub: String },
    /// A short notice line (kills today) for every client.
    Notice(String),
}

/// One client intent for the authority loop.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientIntent {
    /// A replicated input tick for a connected client.
    Input { sub: String, input: Input },
    /// A batched authoritative world edit from a connected client.
    WorldEdit { sub: String, ops: Vec<WorldOp> },
    /// The transport authorized the client; ask the authority to join it.
    Join { sub: String, name: String },
    /// The client disconnected; persist the offline flag.
    Leave { sub: String },
}

/// Radius (metres) around a player within which world cells replicate.
pub const DEFAULT_WORLD_RADIUS: f32 = party_dungeon_net::DEFAULT_WORLD_RADIUS;
/// Radius (metres) around a player within which other players replicate.
pub const DEFAULT_PLAYER_RADIUS: f32 = party_dungeon_net::DEFAULT_PLAYER_RADIUS;

/// World radius from `DUNGEON_WORLD_RADIUS`, clamped to the protocol bounds.
pub fn world_radius_from_env() -> f32 {
    std::env::var("DUNGEON_WORLD_RADIUS")
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, party_dungeon_net::MAX_WORLD_RADIUS))
        .unwrap_or(DEFAULT_WORLD_RADIUS)
}

/// Player radius from `DUNGEON_PLAYER_RADIUS`, clamped to the protocol bounds.
pub fn player_radius_from_env() -> f32 {
    std::env::var("DUNGEON_PLAYER_RADIUS")
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, party_dungeon_net::MAX_PLAYER_RADIUS))
        .unwrap_or(DEFAULT_PLAYER_RADIUS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_delta_emptiness() {
        let mut delta = PieceDelta::default();
        assert!(delta.is_empty());
        delta.remove.push("dungeon_piece:abc".to_string());
        assert!(!delta.is_empty());
    }

    #[test]
    fn radii_are_positive_and_bounded() {
        assert!(world_radius_from_env() > 0.0);
        assert!(world_radius_from_env() <= party_dungeon_net::MAX_WORLD_RADIUS);
        assert!(player_radius_from_env() > 0.0);
        assert!(player_radius_from_env() <= party_dungeon_net::MAX_PLAYER_RADIUS);
    }
}
