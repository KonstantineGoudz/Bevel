//! Transport-agnostic authority: sessions and authoritative world edits.
//!
//! Both replication paths share this module — the legacy matchbox
//! [`HostNet`](crate::net::HostNet) and the embedded `bevy_replicon` server
//! thread ([`crate::repl`]) — so a player row and an authored edit go through
//! exactly the same service-role reducers, the same validation, and the same
//! **persist → apply → one version bump per batch** ordering no matter which
//! transport carried the message.
//!
//! Nothing here touches a socket: callers hand in the author's auth `sub` and
//! the authoritative [`Sim`], and get back transport-neutral [`EditReply`]s to
//! deliver themselves. That keeps the old path's wire behaviour byte-identical
//! while the replicon path reuses the same logic.

use std::time::{Duration, Instant};

use party_dungeon_net::{WorldOp, MAX_WORLD_OPS};
use party_dungeon_server::{Backend, BackendError, SurrealConfig};

use crate::sim::Sim;
use crate::world_edit::{self, PreparedOp};

/// How long a failed world-edit service connection is left alone before a new
/// connect is attempted, so a down store cannot stall the 60 Hz loop.
const EDIT_SERVICE_RETRY: Duration = Duration::from_secs(5);

/// One reply the transport must deliver back to the author of an edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditReply {
    /// A `Place` was persisted; `rid` is the store-generated record id.
    Applied { tag: String, rid: String },
    /// An op was rejected (invalid, missing, or the store failed); `key` is
    /// the op's tag (Place) or rid (Update/Delete).
    Rejected { key: String, reason: String },
}

impl EditReply {
    /// Legacy protocol form: `HostNet` sends this on its reliable channel.
    pub fn into_server_msg(self) -> party_dungeon_net::ServerMsg {
        match self {
            EditReply::Applied { tag, rid } => {
                party_dungeon_net::ServerMsg::WorldEditApplied { tag, rid }
            }
            EditReply::Rejected { key, reason } => {
                party_dungeon_net::ServerMsg::WorldEditRejected { key, reason }
            }
        }
    }
}

/// Verify an auth token (JWT) via the configured provider.
///
/// Decodes `sub` and gates on its presence; full `iss`/`aud` enforcement
/// happens in SurrealDB's row rules (derived from `AuthConfig`) on every write,
/// so dev mode (no DB) still works. `decode_claims_unverified` is display /
/// routing only — the DB re-verifies the signature via the provider's JWKS.
pub fn verify_token(token: &str) -> Result<String, String> {
    let claims = party_dungeon_server::decode_claims_unverified(token)
        .map_err(|e| format!("bad token: {e}"))?;
    if claims.sub.is_empty() {
        return Err("token missing sub".to_string());
    }
    Ok(claims.sub)
}

/// Claimed display name, canonicalized like the row rule (`register_player`
/// trims; the rule requires 1..=24 chars). `None` keeps whatever the row
/// already has.
pub fn sanitize_name(name: Option<String>) -> Option<String> {
    let trimmed = name?.trim().to_string();
    if trimmed.is_empty() || trimmed.chars().count() > 24 {
        return None;
    }
    Some(trimmed)
}

/// Cached service-role access plus the authoritative world-edit state machine.
///
/// The `Backend` cache exists because a `connect_service` costs a WebSocket
/// handshake (tens of ms) and edits arrive in batches continuously while
/// someone is building.
pub struct Authority {
    surreal: Option<SurrealConfig>,
    service_user: String,
    service_pass: String,
    edit_service: Option<Backend>,
    edit_retry_at: Option<Instant>,
    /// Monotonic counter for dev-mode (no DB) local piece rids.
    local_edits: u64,
}

impl Authority {
    pub fn new(
        surreal: Option<SurrealConfig>,
        service_user: String,
        service_pass: String,
    ) -> Self {
        Self {
            surreal,
            service_user,
            service_pass,
            edit_service: None,
            edit_retry_at: None,
            local_edits: 0,
        }
    }

    /// True when a store is configured (edits persist; otherwise dev mode).
    pub fn has_db(&self) -> bool {
        self.surreal.is_some()
    }

    /// The configured store, if any (the bridge's hot-reload poll needs it).
    pub fn surreal(&self) -> Option<&SurrealConfig> {
        self.surreal.as_ref()
    }

    /// Persist one character per auth subject: create-or-fetch the player row,
    /// then claim the signup display name when the client sent one.
    ///
    /// Returns the display name to join with, or `None` when the DB is
    /// unreachable (dev mode) so the caller falls back to `sub`.
    pub async fn join(&self, sub: &str, name: Option<String>) -> Option<String> {
        let cfg = self.surreal.clone()?;
        let backend = Backend::connect_service(&cfg, &self.service_user, &self.service_pass)
            .await
            .ok()?;
        let now_ms = party_dungeon_server::now_ms();
        let player = backend.connect_player(sub, now_ms).await.ok()?;
        match sanitize_name(name) {
            Some(name) => {
                // Only claim when it differs — the row already carries the
                // name after the first join.
                if player.name != name {
                    backend.register_player(sub, &name, now_ms).await.ok()?;
                }
                Some(name)
            }
            // No claimed name: resume with the row's display name (the store
            // defaults a fresh character to "Adventurer").
            None => Some(player.name),
        }
    }

    /// Best-effort offline flag for the persistent row.
    pub async fn leave(&self, sub: &str) {
        let Some(cfg) = self.surreal.clone() else {
            return;
        };
        let Ok(backend) = Backend::connect_service(&cfg, &self.service_user, &self.service_pass).await
        else {
            return;
        };
        let _ = backend
            .disconnect_player(sub, party_dungeon_server::now_ms())
            .await;
    }

    /// Poll the store for out-of-band dungeon edits (the MCP/DB authoring
    /// path) and hot-reload the sim when the row count changed. Shared by both
    /// transports; unchanged 2 s cadence lives in the callers.
    pub async fn poll_edits(&self, sim: &mut Sim) -> bool {
        let Some(cfg) = self.surreal.clone() else {
            return false;
        };
        let mut known_version = sim.dungeon_version;
        crate::bridge::poll_edits(
            sim,
            &cfg,
            &self.service_user,
            &self.service_pass,
            &mut known_version,
        )
        .await
    }

    /// Persist a kill into the shared feed (best effort).
    pub async fn log_death(&self, by: &str, target: &str, tick: u64) {
        let Some(cfg) = self.surreal.clone() else {
            return;
        };
        let Ok(backend) = Backend::connect_service(&cfg, &self.service_user, &self.service_pass).await
        else {
            return;
        };
        let _ = backend.log_death(by, target, tick as i64).await;
    }

    /// Handle one authored edit batch.
    ///
    /// Order per op is **persist → apply**, and the whole batch is finalized
    /// with a single [`Sim::rebuild_current`] so `dungeon_version` bumps exactly
    /// once per message (one coordinated re-stream, not one per op). Replies
    /// are returned in op order; the transport must deliver them before the
    /// changed state, which both paths do by sending them in the same poll /
    /// frame as the apply (before their streaming pass).
    pub async fn apply_world_edit(
        &mut self,
        sub: &str,
        ops: &[WorldOp],
        sim: &mut Sim,
    ) -> Vec<EditReply> {
        if world_edit::is_oversize(ops) {
            eprintln!(
                "sim: world edit rejected sub={sub}: {} ops exceeds MAX_WORLD_OPS={MAX_WORLD_OPS}",
                ops.len()
            );
            // Reply for the ops we can key, but bound the reply count: the
            // batch length is attacker-controlled, so never send one message
            // per op of a huge batch.
            let reason = format!("too many ops ({} > {MAX_WORLD_OPS})", ops.len());
            return ops
                .iter()
                .take(MAX_WORLD_OPS)
                .map(|op| EditReply::Rejected {
                    key: world_edit::op_key(op).to_string(),
                    reason: reason.clone(),
                })
                .collect();
        }

        let mut replies = Vec::new();
        let mut changed = false;
        for op in ops {
            match world_edit::prepare_op(op) {
                Ok(prepared) => match self.persist_and_apply(sim, sub, prepared).await {
                    Ok(outcome) => {
                        changed |= outcome.changed;
                        if let Some(reply) = outcome.reply {
                            replies.push(reply);
                        }
                    }
                    Err(reply) => replies.push(reply),
                },
                Err(rejected) => {
                    eprintln!(
                        "sim: world edit rejected sub={sub} key={} reason={}",
                        rejected.key, rejected.reason
                    );
                    replies.push(EditReply::Rejected {
                        key: rejected.key,
                        reason: rejected.reason,
                    });
                }
            }
        }
        if changed {
            // One rebuild for the batch: recompute spawn/interactables and bump
            // `dungeon_version` once so the streaming layer re-sends only the
            // cells whose content actually changed.
            sim.rebuild_current();
            eprintln!(
                "sim: world edit applied sub={sub} -> dungeon_version={}",
                sim.dungeon_version
            );
        }
        replies
    }

    /// Persist one validated op (system of record), then apply it to the sim.
    ///
    /// `Ok(Outcome { changed })` means the op was accepted (a Place also
    /// carries an [`EditReply::Applied`] to deliver); `Err(reply)` means the
    /// sim was left untouched for that op and the client must roll back.
    async fn persist_and_apply(
        &mut self,
        sim: &mut Sim,
        sub: &str,
        op: PreparedOp,
    ) -> Result<Outcome, EditReply> {
        let has_db = self.has_db();
        match op {
            PreparedOp::Place { tag, piece } => {
                let (rid, wire) = if has_db {
                    match self.write_edit(EditWrite::Place(piece, sub.to_string())).await {
                        Ok(EditWriteOutcome::Piece(row)) => {
                            let rid = row.rid.clone();
                            (rid.clone(), world_edit::wire_from_row(&row))
                        }
                        Ok(EditWriteOutcome::Deleted) => {
                            return Err(self.reject(sub, &tag, "store returned no piece".into()));
                        }
                        Err(reason) => return Err(self.reject(sub, &tag, reason)),
                    }
                } else {
                    self.local_edits += 1;
                    let rid = format!("dungeon_piece:local-{}", self.local_edits);
                    eprintln!(
                        "sim: world edit place sub={sub} tag={tag} rid={rid} (dev: not persisted)"
                    );
                    (rid.clone(), world_edit::wire_from_new_piece(&rid, &piece))
                };
                let applied = sim.apply_wire_piece(&wire);
                if applied {
                    eprintln!("sim: world edit place accepted sub={sub} tag={tag} rid={rid}");
                } else {
                    eprintln!(
                        "sim: world edit place sub={sub} rid={rid} persisted but not applicable (unknown shape)"
                    );
                }
                Ok(Outcome {
                    reply: Some(EditReply::Applied { tag, rid }),
                    changed: applied,
                })
            }
            PreparedOp::Update { rid, piece } => {
                let wire = if has_db {
                    match self.write_edit(EditWrite::Update(rid.clone(), piece)).await {
                        Ok(EditWriteOutcome::Piece(row)) => world_edit::wire_from_row(&row),
                        Ok(EditWriteOutcome::Deleted) => {
                            return Err(self.reject(sub, &rid, "store returned no piece".into()));
                        }
                        Err(reason) => return Err(self.reject(sub, &rid, reason)),
                    }
                } else {
                    if !sim.pieces.iter().any(|(r, _)| r == &rid) {
                        return Err(self.reject(sub, &rid, format!("no piece with rid {rid}")));
                    }
                    eprintln!("sim: world edit update sub={sub} rid={rid} (dev: not persisted)");
                    world_edit::wire_from_new_piece(&rid, &piece)
                };
                let applied = sim.apply_wire_piece(&wire);
                if applied {
                    eprintln!("sim: world edit update accepted sub={sub} rid={rid}");
                }
                Ok(Outcome {
                    reply: None,
                    changed: applied,
                })
            }
            PreparedOp::Delete { rid } => {
                if has_db {
                    match self.write_edit(EditWrite::Delete(rid.clone(), sub.to_string())).await {
                        Ok(_) => {}
                        Err(reason) => return Err(self.reject(sub, &rid, reason)),
                    }
                }
                let existed = sim.delete_wire_piece(&rid);
                if has_db {
                    eprintln!(
                        "sim: world edit delete accepted sub={sub} rid={rid} sim_present={existed}"
                    );
                    Ok(Outcome {
                        reply: None,
                        changed: existed,
                    })
                } else if existed {
                    eprintln!("sim: world edit delete sub={sub} rid={rid} (dev: not persisted)");
                    Ok(Outcome {
                        reply: None,
                        changed: true,
                    })
                } else {
                    Err(self.reject(sub, &rid, format!("no piece with rid {rid}")))
                }
            }
        }
    }

    /// Build + log one rejected-edit reply.
    fn reject(&self, sub: &str, key: &str, reason: String) -> EditReply {
        eprintln!("sim: world edit rejected sub={sub} key={key} reason={reason}");
        EditReply::Rejected {
            key: key.to_string(),
            reason,
        }
    }

    /// The cached service-role backend for world edits, connected lazily and
    /// reused. Returns `None` when no DB is configured (dev mode) or the store
    /// is unreachable; a failed connect is not retried for
    /// [`EDIT_SERVICE_RETRY`] so a down store cannot stall the tick loop.
    async fn edit_backend(&mut self) -> Option<&Backend> {
        if self.edit_service.is_none() {
            if let Some(at) = self.edit_retry_at {
                if Instant::now() < at {
                    return None;
                }
            }
            let cfg = self.surreal.clone()?;
            match Backend::connect_service(&cfg, &self.service_user, &self.service_pass).await {
                Ok(backend) => {
                    eprintln!("sim: world-edit service backend connected (cached)");
                    self.edit_service = Some(backend);
                    self.edit_retry_at = None;
                }
                Err(err) => {
                    eprintln!("sim: world-edit service backend unavailable: {err}");
                    self.edit_retry_at = Some(Instant::now() + EDIT_SERVICE_RETRY);
                    return None;
                }
            }
        }
        self.edit_service.as_ref()
    }

    /// One service-role write for an accepted edit.
    ///
    /// On a [`BackendError::Store`] error (dropped WS, store restart) the
    /// cached connection is dropped, a fresh one is opened once, and the write
    /// is retried. Validation/not-found errors are final and become the
    /// rejection reason.
    async fn write_edit(&mut self, write: EditWrite) -> Result<EditWriteOutcome, String> {
        let now_ms = party_dungeon_server::now_ms();
        let mut retried = false;
        loop {
            let attempt = {
                let Some(backend) = self.edit_backend().await else {
                    return Err("piece store unavailable (no service connection)".to_string());
                };
                match &write {
                    EditWrite::Place(piece, actor) => backend
                        .create_piece(piece.clone(), actor, now_ms)
                        .await
                        .map(EditWriteOutcome::Piece),
                    EditWrite::Update(rid, piece) => backend
                        .update_piece(rid, piece.clone())
                        .await
                        .map(EditWriteOutcome::Piece),
                    EditWrite::Delete(rid, actor) => backend
                        .delete_piece(rid, actor, now_ms)
                        .await
                        .map(|()| EditWriteOutcome::Deleted),
                }
            };
            match attempt {
                Ok(outcome) => return Ok(outcome),
                Err(err @ BackendError::Store(_)) => {
                    eprintln!("sim: world edit store error ({err}); reconnecting once");
                    self.edit_service = None;
                    if retried {
                        return Err(err.to_string());
                    }
                    retried = true;
                }
                Err(err) => return Err(err.to_string()),
            }
        }
    }
}

/// What accepting one op did to the sim.
struct Outcome {
    /// Reply to deliver for this op (Place always answers; Update/Delete don't).
    reply: Option<EditReply>,
    /// Whether the authoritative world changed (drives the batch version bump).
    changed: bool,
}

/// One world-edit write against the store (args already validated).
enum EditWrite {
    Place(party_dungeon_server::NewPiece, String),
    Update(String, party_dungeon_server::NewPiece),
    Delete(String, String),
}

/// What a successful world-edit write produced.
enum EditWriteOutcome {
    /// A persisted row (create/update); the sim applies its canonical values.
    Piece(party_dungeon_server::Piece),
    Deleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_gate_requires_a_subject() {
        // Structurally invalid token: rejected before touching a store.
        assert!(verify_token("not-a-jwt").is_err());
        assert!(verify_token("a.b.c").is_err());
    }

    #[test]
    fn claimed_names_are_canonicalized_like_the_row_rule() {
        assert_eq!(sanitize_name(Some("  Quinn ".into())).as_deref(), Some("Quinn"));
        assert_eq!(sanitize_name(Some("   ".into())), None);
        assert_eq!(sanitize_name(None), None);
        assert_eq!(sanitize_name(Some("x".repeat(25))), None);
    }

    #[test]
    fn edit_replies_map_to_the_legacy_protocol() {
        let applied = EditReply::Applied {
            tag: "local-1".into(),
            rid: "dungeon_piece:abc".into(),
        };
        assert_eq!(
            applied.into_server_msg(),
            party_dungeon_net::ServerMsg::WorldEditApplied {
                tag: "local-1".into(),
                rid: "dungeon_piece:abc".into(),
            }
        );
        let rejected = EditReply::Rejected {
            key: "local-2".into(),
            reason: "bad".into(),
        };
        assert_eq!(
            rejected.into_server_msg(),
            party_dungeon_net::ServerMsg::WorldEditRejected {
                key: "local-2".into(),
                reason: "bad".into(),
            }
        );
    }
}
