//! Authoritative headless simulation: the dungeon, the bodies, the tick.
//!
//! The sim owns one [`rapier3d`] world built from the same catalogue the
//! editor uses. Static solids come straight from the game's own collider
//! builder (`party_dungeon::editor::colliders`), so art changes can never
//! desync collision. Players are kinematic state (exactly like the game:
//! the character capsule is integrated by hand, never simulated), movers are
//! kinematic bodies driven along the game's Bezier tracks, and dynamic props
//! are real Rapier dynamic bodies the players can shove.
//!
//! One tick, 60 times a second: drain inputs, advance movers, integrate
//! players, step dynamics, apply hazards, deaths, and interactions, then
//! emit a [`Snapshot`](party_dungeon_net::Snapshot).
//!
//! The world is not pushed wholesale: [`stream`] partitions it into 32 m XZ
//! cells with per-cell versions and streams each peer only the cells around
//! its character (and only the players around it) under a per-tick budget —
//! see [`stream::ChunkIndex`], [`stream::PeerView`] and
//! [`stream::PlayerBuckets`].

pub mod authority;
pub mod bridge;
pub mod net;
/// Embedded `bevy_replicon` server path (feature `replicon`).
#[cfg(feature = "replicon")]
pub mod repl;
pub mod signup;
pub mod sim;
pub mod stream;
pub mod world;
pub mod world_edit;

#[cfg(test)]
mod tests;

pub use sim::Sim;
