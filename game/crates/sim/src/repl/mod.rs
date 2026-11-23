//! Embedded `bevy_replicon` server path (feature `replicon`).
//!
//! The authoritative simulation stays exactly where it was: a Tokio task owns
//! [`Sim`](crate::sim::Sim), the 60 Hz tick and SurrealDB persistence
//! ([`crate::authority::Authority`]). What changes is the transport: instead of
//! `HostNet`'s hand-written chunk streaming, a small Bevy app runs on its own
//! thread, owns one replicated entity per world piece/player, and lets
//! `bevy_replicon` do delta encoding, reliability, interest management
//! (visibility) and message channels over the *same* matchbox/WebRTC room
//! (`/party-dungeon`).
//!
//! Layout:
//!
//! * [`convert`] — quantized `model::Piece` ↔ `PieceData`, `SimPlayer` ↔
//!   `PlayerPose`. Pure and unit-tested.
//! * [`protocol`] — the cross-thread message enums/channels (no Bevy types
//!   cross other than the plain replication components).
//! * [`entities`] — the Bevy systems that spawn/mutate/despawn replicated
//!   entities (pieces with `PieceData`/`WorldCell`, players with
//!   `PlayerPose`/`PlayerCell`, movers with `PiecePath`), re-insert the two
//!   interest sets (`VisibleCells` for geometry `VisiblePlayerCells` for
//!   players) on cell changes, and forward
//!   `FromClient<InputMsg>`/`FromClient<WorldEditMsg>` to Tokio.
//! * [`app`] — the embedded app/thread (`MinimalPlugins` + `StatesPlugin` +
//!   `RepliconPlugins(Custom)` + `replication::register` + `MatchboxServerPlugin`).
//! * [`run`] — the Tokio authority loop that mirrors `Sim` into the app and
//!   feeds client intents back through the shared `Authority`.
//! * [`backend_glue`] — the only code that names the matchbox-replicon backend
//!   API (JWT authenticator + session lifecycle).
//!
//! The legacy path remains the default; [`app::enabled`] gates on
//! `DUNGEON_REPLICON=1`.

pub mod app;
pub mod backend_glue;
pub mod convert;
pub mod entities;
pub mod protocol;
pub mod run;

#[cfg(test)]
mod tests;

pub use app::{enabled, spawn_server_thread, ReplServerChannels, ReplServerConfig};
pub use run::run as run_authority_loop;
