//! Streamed world apply + authoritative world-edit sync.
//!
//! The MMO has exactly one world, owned by the sim (backed by SurrealDB). There
//! are no level files and the world is not pushed wholesale: the sim streams
//! the 32 m XZ cells around the character (`ServerMsg::WorldChunk`) and
//! unloads them again (`ServerMsg::WorldUnload`). The local world is therefore
//! the union of the cells resident in [`WorldChunks`], applied incrementally so
//! ordinary editor state survives:
//!
//! * **Stable local ids.** The wire keys pieces by DB `rid`; the editor keys
//!   them by `u32`. [`WorldSync`] keeps the `rid -> id` mapping across applies,
//!   so a re-sent cell reuses its ids instead of renumbering (which would clear
//!   the selection, orphan undo steps and re-key movers). Only rids that leave
//!   the stream lose their mapping, and `next_id` stays monotonic.
//! * **Incremental visuals.** Pieces are written straight to `editor.pieces`
//!   and `editor.hard_dirty` is set: `sync_visuals` then despawns only the ids
//!   that vanished and spawns only the new ones, moving the rest in place.
//! * **Preserved state.** Orbit camera, grass, vines, character, undo/redo and
//!   outliner labels/groups/locks are session-local and are not touched; only
//!   a selection entry whose piece just unloaded is dropped.
//!
//! Editor-only cosmetics (trigger colors, outliner names) are not part of the
//! shared world and stay session-local.
//!
//! # Authoritative editing
//!
//! Every user edit must reach the sim exactly once — the sim persists it,
//! applies it to the authoritative world and bumps `dungeon_version`, so the
//! changed cells stream back to every player. [`WorldSync::sync_edits`] diffs
//! the editor's pieces against what the store last showed and stages
//! [`WorldOp`]s in [`PendingWorldEdits`] (sent by `net_client`):
//!
//! * a piece the editor created locally (no rid) becomes `Place { tag, piece }`
//!   and stays visible as a *local*;
//! * a persisted piece whose gameplay fields changed becomes `Update { piece }`,
//!   throttled per rid so a gizmo drag cannot flood the reliable channel;
//! * a persisted piece the user deleted (its cell is still resident) becomes
//!   `Delete { rid }`;
//! * pieces whose cell simply unloaded are forgotten silently — an unload is
//!   not a user deletion.
//!
//! Optimistic changes never fight the streamed echo:
//!
//! * pending edits of persisted pieces live in `overrides` and win over the
//!   streamed value until the store echoes a change (then its canonical values
//!   are adopted and the override ends);
//! * local placements are merged by [`WorldSync::compose`] with the same local
//!   id, and `WorldEditApplied { tag, rid }` re-keys that id to the persisted
//!   rid, so the echoed chunk updates the piece in place instead of duplicating
//!   it;
//! * `WorldEditRejected { key }` drops the optimistic copy (and, for a
//!   placement, the piece itself at the next apply) and logs a warning.
//!
//! After every compose the baselines are rebased to exactly what was installed,
//! so a streamed change is never mistaken for a user edit (which would echo an
//! `Update` back at the sim forever).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use party_dungeon_net::{ChunkId, WorldChunk, WorldOp, WorldPath, WorldPiece};

use super::{Editor, Movers};
use crate::model::{self, Piece};
use crate::net_client::{PendingWorldEdits, WorldChunks};
use crate::session;

/// Throttle between queued `Update`s for one persisted piece. A gizmo drag
/// changes the piece every frame; ~7 messages/s keeps other players watching it
/// move without flooding the reliable channel.
const UPDATE_THROTTLE_SECS: f64 = 0.15;

/// Float tolerance when comparing streamed (store-canonicalized) values with
/// the client's own state. Degree normalization, DB f32 round-trips and the
/// store's extent sanitizing all introduce sub-tolerance noise.
const EPS: f32 = 1e-4;
/// Angular tolerance in degrees (about 3.5 arc-seconds).
const ANGLE_EPS_DEG: f32 = 1e-3;

/// A piece the editor created that has no persisted rid yet.
#[derive(Clone, Copy, Debug)]
struct LocalPiece {
    /// The optimistic piece as the editor currently shows it.
    piece: Piece,
    /// A `WorldOp::Place` for this tag has been accepted by the reliable
    /// channel (so later edits must wait for the rid instead of rewriting the
    /// message already on the wire).
    sent: bool,
    /// The user deleted the local piece while its Place was in flight; the row
    /// is removed as soon as `WorldEditApplied` reveals its rid.
    cancelled: bool,
}

/// Stable local piece ids for the streamed world plus the authoritative-edit
/// diff state. See the module docs.
#[derive(Resource, Default)]
pub(crate) struct WorldSync {
    /// Persisted rid (or, until `WorldEditApplied`, the local tag) to editor id.
    rid_to_id: HashMap<String, u32>,
    /// Reverse of `rid_to_id`.
    id_to_rid: HashMap<u32, String>,
    /// Cell the sim last streamed each persisted piece in (from the last
    /// compose). Used to tell an unload from a user deletion.
    rid_cell: HashMap<String, ChunkId>,
    /// Cells resident in the last compose.
    resident_cells: HashSet<ChunkId>,
    /// Pending user edits of persisted pieces, keyed by rid. Wins over the
    /// streamed value until the store echoes a change.
    overrides: HashMap<String, Piece>,
    /// The streamed wire state an override was staged against (or the state a
    /// `Place` was sent with). A streamed change to it means the store has
    /// answered, so the override ceases and its canonical values are adopted.
    override_baseline: HashMap<String, WorldPiece>,
    /// Not-yet-persisted placements, keyed by client tag.
    locals: HashMap<String, LocalPiece>,
    /// Accepted `Place` tags to their persisted rid.
    tag_to_rid: HashMap<String, String>,
    /// Placements the store has accepted (`WorldEditApplied`) but whose echoed
    /// chunk has not arrived yet, keyed by rid. Keeping them visible under
    /// their stable id means a compose in that window cannot drop the piece.
    pending_echo: HashMap<String, Piece>,
    /// What the sim last showed or acknowledged for a persisted rid; the diff
    /// baseline (rebased after every compose).
    last_synced: HashMap<String, Piece>,
    /// Rids that have been part of a streamed chunk at least once. A mapped rid
    /// that never streamed (just placed) is not a remote deletion when a
    /// compose happens before its echo.
    streamed: HashSet<String>,
    /// User deletions awaiting confirmation from the stream, keyed by rid.
    deleting: HashSet<String>,
    /// Rids whose rejected deletion must be reinstalled by the next compose.
    restoring: HashSet<String>,
    /// `Place` payloads accepted by the reliable channel, keyed by tag, so an
    /// edit made after the send can be folded into a follow-up `Update`.
    sent_places: HashMap<String, WorldPiece>,
    /// Ids whose placement was rejected: skipped by the diff until the next
    /// compose has removed them from the editor.
    blocked_ids: HashSet<u32>,
    /// `Time::elapsed_secs_f64` of the last queued `Update`, per rid.
    update_at: HashMap<String, f64>,
    /// Rids whose latest edit is waiting for the throttle window to elapse.
    pending_update: HashSet<String>,
    /// Next local id to hand out. Only ever grows.
    next_id: u32,
    /// A rejection needs the caller to recompose from the stream.
    rolled_back: bool,
}

impl WorldSync {
    /// Compose the union of `chunks` into a validated world with stable ids.
    ///
    /// Order is deterministic — cells sorted by `(z, x)`, pieces and paths
    /// sorted by `rid`, locals appended by tag — so every client and every
    /// apply agrees on ids and piece order. Known rids reuse their local id,
    /// new rids get the next one, pending overrides replace their streamed
    /// value in place and not-yet-persisted placements are appended. Rids that
    /// left the stream are purged; a re-added piece is a fresh one.
    ///
    /// Runs [`session::Snapshot::validate`] on the streamed pieces/paths, the
    /// same validation a loaded level gets, so the editor never receives
    /// non-finite transforms, unknown shapes or malformed trigger extents. On
    /// error the bookkeeping is left untouched (a later chunk message retries).
    pub(crate) fn compose(
        &mut self,
        chunks: &HashMap<ChunkId, WorldChunk>,
        queue: &mut PendingWorldEdits,
    ) -> Result<session::Loaded, session::LoadError> {
        let mut cells: Vec<&WorldChunk> = chunks.values().collect();
        cells.sort_by_key(|chunk| (chunk.id.z, chunk.id.x));
        let resident_cells: HashSet<ChunkId> = cells.iter().map(|chunk| chunk.id).collect();

        // Work on clones so a failed validation cannot corrupt the bookkeeping.
        let mut rid_to_id = self.rid_to_id.clone();
        let mut next_id = self.next_id.max(1);
        let overrides = self.overrides.clone();
        let mut seen: HashSet<String> = HashSet::new();
        let mut rid_cell: HashMap<String, ChunkId> = HashMap::new();
        let mut visible_ids: HashSet<u32> = HashSet::new();
        let mut clear_overrides: Vec<String> = Vec::new();
        let mut streamed_now: HashSet<String> = HashSet::new();
        let mut saved_pieces = Vec::new();

        for chunk in &cells {
            let mut pieces: Vec<&WorldPiece> = chunk.pieces.iter().collect();
            pieces.sort_by(|a, b| a.rid.cmp(&b.rid));
            for piece in pieces {
                // A piece belongs to exactly one cell (by origin); a buggy peer
                // sending it twice must not create duplicate editor ids.
                if !seen.insert(piece.rid.clone()) {
                    continue;
                }
                rid_cell.insert(piece.rid.clone(), chunk.id);
                streamed_now.insert(piece.rid.clone());
                if self.deleting.contains(&piece.rid) {
                    // Tombstoned: keep the id mapping but hide the piece until
                    // the store echoes the cell without it.
                    rid_to_id.entry(piece.rid.clone()).or_insert_with(|| {
                        let id = next_id;
                        next_id += 1;
                        id
                    });
                    continue;
                }
                let id = match rid_to_id.get(&piece.rid) {
                    Some(&id) => id,
                    None => {
                        let id = next_id;
                        next_id += 1;
                        rid_to_id.insert(piece.rid.clone(), id);
                        id
                    }
                };
                // A pending local edit wins over the streamed value only while
                // the stream still shows the state the edit was staged against.
                // Once the store echoes a change it has answered (our op or a
                // competing one): adopt its canonical values.
                if let Some(baseline) = self.override_baseline.get(&piece.rid)
                    && !wire_matches(piece, baseline)
                {
                    clear_overrides.push(piece.rid.clone());
                }
                visible_ids.insert(id);
                saved_pieces.push(saved_piece(id, piece));
            }
        }

        let mut saved_platforms = Vec::new();
        for chunk in &cells {
            let mut paths: Vec<&WorldPath> = chunk.paths.iter().collect();
            paths.sort_by(|a, b| a.piece_rid.cmp(&b.piece_rid));
            for path in paths {
                // Paths live in their piece's cell and are keyed by `piece_rid`;
                // a path whose piece is not resident has nothing to drive.
                let Some(&piece) = rid_to_id.get(path.piece_rid.as_str()) else {
                    continue;
                };
                if !visible_ids.contains(&piece) {
                    continue;
                }
                saved_platforms.push(saved_platform(piece, path));
            }
        }

        // Not-yet-persisted placements are appended after the streamed pieces,
        // always in tag order, each with the id it was first given.
        let mut tags: Vec<String> = self.locals.keys().cloned().collect();
        tags.sort();
        let mut local_pieces = Vec::new();
        for tag in &tags {
            // Keep the mapping alive even for a cancelled placement: the piece
            // stays hidden until its rid is known (or the op is rejected).
            seen.insert(tag.clone());
            let Some(local) = self.locals.get(tag) else {
                continue;
            };
            if local.cancelled {
                continue;
            }
            let id = match rid_to_id.get(tag) {
                Some(&id) => id,
                None => {
                    let id = next_id;
                    next_id += 1;
                    rid_to_id.insert(tag.clone(), id);
                    id
                }
            };
            let mut piece = local.piece;
            piece.id = id;
            local_pieces.push(piece);
        }

        let snapshot = session::Snapshot {
            version: session::SNAPSHOT_VERSION,
            next_id: Some(next_id),
            pieces: saved_pieces,
            platforms: saved_platforms,
            ..Default::default()
        };
        let mut loaded = snapshot.validate()?;

        // Optimistic overrides replace the validated streamed piece in place.
        for (rid, piece) in overrides.iter() {
            if clear_overrides.contains(rid) {
                continue;
            }
            let Some(&id) = rid_to_id.get(rid) else {
                continue;
            };
            if let Some(slot) = loaded.pieces.iter_mut().find(|p| p.id == id) {
                let mut optimistic = *piece;
                optimistic.id = id;
                *slot = optimistic;
            }
        }
        loaded.pieces.extend(local_pieces);
        // Accepted placements whose echo has not arrived yet stay visible with
        // their stable id (a compose between Applied and the echo must not make
        // the optimistic piece vanish and lose its outliner row/selection).
        let mut pending_rids: Vec<String> = self
            .pending_echo
            .keys()
            .filter(|rid| !streamed_now.contains(*rid) && !self.deleting.contains(*rid))
            .cloned()
            .collect();
        pending_rids.sort();
        for rid in pending_rids {
            let Some(&id) = rid_to_id.get(&rid) else {
                continue;
            };
            let piece = overrides
                .get(&rid)
                .copied()
                .unwrap_or(self.pending_echo[&rid]);
            let mut optimistic = piece;
            optimistic.id = id;
            loaded.pieces.push(optimistic);
        }
        loaded.next_id = loaded.next_id.max(next_id);

        // Which tracked keys left the stream, and why. A key absent from the
        // stream while its cell is still resident means the stored cell was
        // replaced (a delete, ours or another player's); while its cell is gone
        // it simply unloaded.
        let mut forget_remote: Vec<String> = Vec::new();
        let mut forget_unload: Vec<String> = Vec::new();
        for key in rid_to_id.keys() {
            if seen.contains(key) {
                continue;
            }
            let cell = self.rid_cell.get(key).copied();
            let still_resident = cell.is_some_and(|cell| resident_cells.contains(&cell));
            if still_resident && self.streamed.contains(key) {
                // Streamed before, gone from a cell that is still resident:
                // the store really removed it (a delete, ours or another
                // player's).
                forget_remote.push(key.clone());
            } else if cell.is_some() && !still_resident {
                forget_unload.push(key.clone());
            }
            // Anything else — a fresh placement whose Applied answer arrived
            // before its echo, or a cancelled placement never streamed — stays
            // tracked (hidden if tombstoned) until the stream or a rejection
            // resolves it.
        }

        let mut id_to_rid: HashMap<u32, String> = HashMap::new();
        for (key, id) in rid_to_id.iter() {
            id_to_rid.insert(*id, key.clone());
        }

        // Commit.
        self.rid_to_id = rid_to_id;
        self.id_to_rid = id_to_rid;
        self.rid_cell = rid_cell;
        self.resident_cells = resident_cells;
        self.next_id = next_id;
        self.overrides = overrides;
        self.streamed.extend(streamed_now.iter().cloned());
        for rid in clear_overrides {
            self.overrides.remove(&rid);
            self.override_baseline.remove(&rid);
            self.pending_update.remove(&rid);
            queue.remove_key(&rid);
        }
        for key in forget_remote {
            self.rid_to_id.remove(&key);
            self.id_to_rid.retain(|_, rid| *rid != key);
            self.streamed.remove(&key);
            self.pending_echo.remove(&key);
            self.deleting.remove(&key);
            self.restoring.remove(&key);
            self.overrides.remove(&key);
            self.override_baseline.remove(&key);
            self.last_synced.remove(&key);
            self.update_at.remove(&key);
            self.pending_update.remove(&key);
            self.tag_to_rid.remove(&key);
            queue.remove_key(&key);
        }
        for key in forget_unload {
            // The geometry left the interest radius, not the world. Drop the
            // local mirror; a pending *edit* still reaches the store (its rid
            // is valid), but an unload is not a deletion, so a queued Delete is
            // forgotten silently.
            self.rid_to_id.remove(&key);
            self.id_to_rid.retain(|_, rid| *rid != key);
            self.streamed.remove(&key);
            self.pending_echo.remove(&key);
            self.rid_cell.remove(&key);
            self.last_synced.remove(&key);
            if self.deleting.remove(&key) {
                queue.remove_key(&key);
            }
        }
        self.streamed.retain(|rid| self.rid_to_id.contains_key(rid));
        // Bookkeeping that can no longer refer to a tracked piece.
        self.pending_echo.retain(|rid, _| {
            !streamed_now.contains(rid)
                && self.rid_to_id.contains_key(rid)
                && !self.deleting.contains(rid)
        });
        self.overrides
            .retain(|rid, _| self.rid_to_id.contains_key(rid));
        self.override_baseline
            .retain(|rid, _| self.overrides.contains_key(rid));
        self.last_synced
            .retain(|rid, _| self.rid_to_id.contains_key(rid) && !self.locals.contains_key(rid));
        Ok(loaded)
    }

    /// Rebase the diff baselines to exactly what a compose just installed.
    ///
    /// Without this the next diff would see the streamed change as a local user
    /// edit and echo it back to the sim — an infinite loop between clients.
    pub(crate) fn rebase(&mut self, installed: &[Piece]) {
        let keys: Vec<Option<String>> = installed
            .iter()
            .map(|piece| self.id_to_rid.get(&piece.id).cloned())
            .collect();
        for (piece, key) in installed.iter().zip(keys) {
            let Some(key) = key else { continue };
            if let Some(local) = self.locals.get_mut(&key) {
                local.piece = *piece;
            } else {
                self.last_synced.insert(key, *piece);
            }
        }
        let live: HashSet<u32> = installed.iter().map(|piece| piece.id).collect();
        self.last_synced
            .retain(|rid, _| self.rid_to_id.get(rid).is_some_and(|id| live.contains(id)));
    }

    /// Drop rejected-placement ids that the latest compose removed from the
    /// editor, so the id can be re-used by a later placement.
    pub(super) fn clear_blocked(&mut self, installed: &[Piece]) {
        let live: HashSet<u32> = installed.iter().map(|piece| piece.id).collect();
        self.blocked_ids.retain(|id| live.contains(id));
    }

    /// A rejected deletion has been reinstalled (or is gone): the diff may
    /// manage the piece again.
    pub(super) fn clear_restoring(&mut self) {
        self.restoring.clear();
    }

    /// True once when the last batch contained an op the store rejected (the
    /// caller should tell the user).
    pub(super) fn take_rolled_back(&mut self) -> bool {
        std::mem::take(&mut self.rolled_back)
    }

    /// Reconcile net outcomes and diff the editor against the baselines,
    /// queueing world ops. Returns `true` when the caller must recompose (a
    /// rejection rolled back an optimistic change and the streamed state must
    /// be reinstalled).
    pub(crate) fn sync_edits(
        &mut self,
        pieces: &[Piece],
        movers: Option<&Movers>,
        queue: &mut PendingWorldEdits,
        now: f64,
    ) -> bool {
        let recompose = self.reconcile_events(queue, now);
        self.capture(pieces, movers, queue, now);
        recompose
    }

    /// Apply `WorldEditApplied` / `WorldEditRejected` answers and remember
    /// which `Place`s reached the wire.
    fn reconcile_events(&mut self, queue: &mut PendingWorldEdits, now: f64) -> bool {
        for op in queue.take_sent() {
            if let WorldOp::Place { tag, piece } = op {
                self.sent_places.insert(tag, piece);
            }
        }

        let mut recompose = false;
        for (tag, rid) in queue.take_applied() {
            let Some(id) = self.rid_to_id.remove(&tag) else {
                bevy::log::warn!("world edit applied for unknown tag {tag} -> {rid}");
                continue;
            };
            self.rid_to_id.insert(rid.clone(), id);
            self.id_to_rid.insert(id, rid.clone());
            self.tag_to_rid.insert(tag.clone(), rid.clone());
            let sent = self.sent_places.remove(&tag);
            let Some(local) = self.locals.remove(&tag) else {
                continue;
            };
            if local.cancelled {
                // The user deleted it while the Place was in flight: remove the
                // freshly persisted row again.
                self.overrides.remove(&rid);
                self.override_baseline.remove(&rid);
                self.last_synced.remove(&rid);
                self.deleting.insert(rid.clone());
                queue.remove_key(&rid);
                queue.upsert(WorldOp::Delete { rid: rid.clone() });
                recompose = true;
                continue;
            }
            self.rid_cell.insert(
                rid.clone(),
                ChunkId::containing(local.piece.pos.x, local.piece.pos.y),
            );
            // Visible until the echoed chunk arrives, so a compose in between
            // keeps the piece (and its selection/outliner row) stable.
            self.pending_echo.insert(rid.clone(), local.piece);
            self.last_synced.insert(rid.clone(), local.piece);
            // Edits made after the Place went out are folded into one Update.
            let edited_after_send = sent
                .as_ref()
                .is_some_and(|wire| !wire_matches_piece(wire, &local.piece));
            if edited_after_send {
                self.overrides.insert(rid.clone(), local.piece);
                if let Some(wire) = sent {
                    self.override_baseline.insert(rid.clone(), wire);
                } else {
                    self.override_baseline
                        .insert(rid.clone(), to_world_piece("", &local.piece));
                }
                queue.upsert(WorldOp::Update {
                    piece: to_world_piece(&rid, &local.piece),
                });
                self.update_at.insert(rid.clone(), now);
                self.pending_update.remove(&rid);
            }
        }

        for (key, reason) in queue.take_rejected() {
            bevy::log::warn!("world edit rejected ({key}): {reason}");
            self.rolled_back = true;
            recompose = true;
            queue.remove_key(&key);
            if self.locals.remove(&key).is_some() {
                // An unsaved placement: forget the local piece. Its editor id
                // is blocked until the next compose drops the piece itself so
                // the diff cannot immediately re-register (and re-send) it.
                if let Some(id) = self.rid_to_id.remove(&key) {
                    self.id_to_rid.remove(&id);
                    self.blocked_ids.insert(id);
                }
                self.sent_places.remove(&key);
                self.pending_echo.remove(&key);
                self.last_synced.remove(&key);
            } else if self.overrides.remove(&key).is_some() {
                // A persisted piece: drop the optimistic copy; the recompose
                // reinstalls the streamed state.
                self.override_baseline.remove(&key);
                self.last_synced.remove(&key);
                self.update_at.remove(&key);
                self.pending_update.remove(&key);
            } else {
                // A rejected Delete: the piece must come back, and the diff
                // must not re-delete it before the recompose reinstalls it.
                if self.deleting.remove(&key) {
                    if self.streamed.contains(&key) {
                        // The store still has it: the recompose reinstalls it.
                        self.restoring.insert(key.clone());
                    } else {
                        // It never streamed back (the placement never reached
                        // the world): nothing to restore, forget it.
                        self.forget_rid(&key);
                    }
                }
                self.update_at.remove(&key);
                self.pending_update.remove(&key);
            }
        }
        recompose
    }

    /// Diff `pieces` (the editor's current world) against the baselines and
    /// stage the resulting ops.
    fn capture(
        &mut self,
        pieces: &[Piece],
        movers: Option<&Movers>,
        queue: &mut PendingWorldEdits,
        now: f64,
    ) {
        let current: HashMap<u32, Piece> = pieces.iter().map(|piece| (piece.id, *piece)).collect();

        // (a) Local placements: fold edits into their pending Place, or notice
        // the user deleted one (possibly with a Place already on the wire).
        let tags: Vec<String> = self.locals.keys().cloned().collect();
        for tag in tags {
            let Some(&id) = self.rid_to_id.get(&tag) else {
                continue;
            };
            let Some(mut local) = self.locals.get(&tag).copied() else {
                continue;
            };
            match current.get(&id).copied() {
                Some(piece) => {
                    if local.cancelled {
                        continue;
                    }
                    let runtime_owned =
                        piece.dynamic || movers.is_some_and(|movers| movers.map.contains_key(&id));
                    let changed = !same_persisted(&local.piece, &piece, runtime_owned);
                    local.piece = piece;
                    if changed && !local.sent {
                        // Nothing on the wire yet: replace the queued Place with
                        // the latest state rather than sending an Update before
                        // the rid even exists.
                        queue.upsert(WorldOp::Place {
                            tag: tag.clone(),
                            piece: to_world_piece("", &piece),
                        });
                    }
                    self.locals.insert(tag, local);
                }
                None => {
                    if local.cancelled {
                        continue;
                    }
                    if local.sent {
                        // The row is being created; delete it once its rid is
                        // known (WorldEditApplied).
                        local.cancelled = true;
                        self.locals.insert(tag, local);
                    } else {
                        // Never reached the wire: forget it entirely.
                        queue.remove_key(&tag);
                        self.forget_local(&tag);
                    }
                }
            }
        }

        // (b) Persisted pieces: user edits and deletions.
        let known: Vec<(u32, String)> = self
            .id_to_rid
            .iter()
            .filter(|(_, key)| !self.locals.contains_key(*key))
            .map(|(id, key)| (*id, key.clone()))
            .collect();
        for (id, rid) in known {
            if self.restoring.contains(&rid) {
                // A rejected delete: wait for the recompose to reinstall it
                // before diffing again, or it would be deleted in a loop.
                continue;
            }
            if self.deleting.contains(&rid) {
                if let Some(piece) = current.get(&id).copied() {
                    // The user undid the delete before it was confirmed: keep
                    // the piece and retract the queued Delete.
                    self.deleting.remove(&rid);
                    queue.remove_key(&rid);
                    self.last_synced.insert(rid, piece);
                }
                continue;
            }
            let Some(piece) = current.get(&id).copied() else {
                let resident = self
                    .rid_cell
                    .get(&rid)
                    .is_some_and(|cell| self.resident_cells.contains(cell));
                if resident {
                    self.begin_delete(&rid, queue);
                } else {
                    // The cell unloaded: the piece leaving the editor is not a
                    // user deletion.
                    self.forget_rid(&rid);
                }
                continue;
            };
            let runtime_owned =
                piece.dynamic || movers.is_some_and(|movers| movers.map.contains_key(&id));
            let Some(baseline) = self.last_synced.get(&rid).copied() else {
                self.last_synced.insert(rid, piece);
                continue;
            };
            if same_persisted(&baseline, &piece, runtime_owned) {
                // Only editor-local fields or runtime motion (physics, mover
                // tracks) changed: never send those upstream. Keep an existing
                // override's cosmetics current though.
                if let Some(over) = self.overrides.get_mut(&rid) {
                    *over = piece;
                }
                continue;
            }
            self.overrides.insert(rid.clone(), piece);
            self.override_baseline
                .entry(rid.clone())
                .or_insert_with(|| to_world_piece("", &baseline));
            self.last_synced.insert(rid.clone(), piece);
            self.queue_update(&rid, piece, queue, now);
        }

        // (c) Pieces the editor created locally (no rid yet).
        for piece in pieces {
            if self.id_to_rid.contains_key(&piece.id) || self.blocked_ids.contains(&piece.id) {
                continue;
            }
            let tag = format!("local-{}", piece.id);
            // Undo can resurrect an old id: drop any stale tag first.
            self.forget_local(&tag);
            self.rid_to_id.insert(tag.clone(), piece.id);
            self.id_to_rid.insert(piece.id, tag.clone());
            self.locals.insert(
                tag.clone(),
                LocalPiece {
                    piece: *piece,
                    sent: false,
                    cancelled: false,
                },
            );
            queue.upsert(WorldOp::Place {
                tag,
                piece: to_world_piece("", piece),
            });
        }

        // (d) Release throttled updates with their latest state.
        let pending: Vec<String> = self.pending_update.iter().cloned().collect();
        for rid in pending {
            let Some(piece) = self
                .overrides
                .get(&rid)
                .copied()
                .or_else(|| self.last_synced.get(&rid).copied())
            else {
                self.pending_update.remove(&rid);
                continue;
            };
            let last = self
                .update_at
                .get(&rid)
                .copied()
                .unwrap_or(f64::NEG_INFINITY);
            if now - last >= UPDATE_THROTTLE_SECS {
                queue.upsert(WorldOp::Update {
                    piece: to_world_piece(&rid, &piece),
                });
                self.update_at.insert(rid.clone(), now);
                self.pending_update.remove(&rid);
            }
        }
    }

    /// Stage one `Update` for `rid`, throttled to one per
    /// [`UPDATE_THROTTLE_SECS`]; a blocked edit is released later with the
    /// latest state.
    fn queue_update(&mut self, rid: &str, piece: Piece, queue: &mut PendingWorldEdits, now: f64) {
        let last = self
            .update_at
            .get(rid)
            .copied()
            .unwrap_or(f64::NEG_INFINITY);
        if now - last >= UPDATE_THROTTLE_SECS {
            queue.upsert(WorldOp::Update {
                piece: to_world_piece(rid, &piece),
            });
            self.update_at.insert(rid.to_string(), now);
            self.pending_update.remove(rid);
        } else {
            self.pending_update.insert(rid.to_string());
        }
    }

    /// The user removed a persisted piece from the editor. Its cell is still
    /// resident, so this is a real delete, not an unload.
    fn begin_delete(&mut self, rid: &str, queue: &mut PendingWorldEdits) {
        self.deleting.insert(rid.to_string());
        self.pending_echo.remove(rid);
        self.overrides.remove(rid);
        self.override_baseline.remove(rid);
        self.last_synced.remove(rid);
        self.update_at.remove(rid);
        self.pending_update.remove(rid);
        queue.remove_key(rid);
        queue.upsert(WorldOp::Delete {
            rid: rid.to_string(),
        });
    }

    /// Forget a local placement (never persisted) and its mapping.
    fn forget_local(&mut self, tag: &str) {
        self.locals.remove(tag);
        if let Some(id) = self.rid_to_id.remove(tag)
            && self.id_to_rid.get(&id).is_some_and(|key| key == tag)
        {
            self.id_to_rid.remove(&id);
        }
        self.tag_to_rid.remove(tag);
        self.last_synced.remove(tag);
    }

    /// The piece's cell unloaded: drop the local mirror but keep any pending
    /// edit (its rid is still valid in the store).
    fn forget_rid(&mut self, rid: &str) {
        if let Some(id) = self.rid_to_id.remove(rid)
            && self.id_to_rid.get(&id).is_some_and(|key| key == rid)
        {
            self.id_to_rid.remove(&id);
        }
        self.rid_cell.remove(rid);
        self.pending_echo.remove(rid);
        self.last_synced.remove(rid);
    }
}

/// Mirror of `From<&Piece> for SavedPiece`: omit damage when it matches the
/// catalogue default so default-damage shapes (e.g. spikes) stay dangerous.
fn default_or_some(shape_id: &str, damage: f32) -> Option<f32> {
    let default = crate::model::shape_index(shape_id)
        .and_then(|index| crate::model::shapes().get(index))
        .map(|shape| shape.damage)
        .unwrap_or(0.0);
    if (damage - default).abs() < 1e-6 {
        None
    } else {
        Some(damage)
    }
}

/// True when a shape id names a glade (procedural building) piece, whose
/// `trigger_alpha` is the shared roof pitch (`roof_pitch = alpha * 2.0`) and
/// therefore gameplay rather than an editor-local opacity cosmetic.
fn is_glade_shape(shape_id: &str) -> bool {
    model::shape_index(shape_id)
        .and_then(|index| model::shapes().get(index))
        .is_some_and(|shape| shape.is_glade())
}

/// True when an editor piece's shape is a glade piece.
fn is_glade_piece(piece: &Piece) -> bool {
    model::shapes()
        .get(piece.shape)
        .is_some_and(|shape| shape.is_glade())
}

/// One wire piece as the loader's `SavedPiece`, with its local id assigned.
fn saved_piece(id: u32, piece: &WorldPiece) -> session::SavedPiece {
    session::SavedPiece {
        id,
        shape: piece.shape.clone(),
        x: piece.x,
        z: piece.z,
        elevation: piece.elevation,
        rot_deg: piece.rot_deg,
        pitch_deg: piece.pitch_deg,
        roll_deg: piece.roll_deg,
        dynamic: piece.dynamic,
        damage: default_or_some(&piece.shape, piece.damage),
        interact: if piece.interact == "none" {
            None
        } else {
            Some(piece.interact.clone())
        },
        trigger_width: Some(piece.trigger_w),
        trigger_length: Some(piece.trigger_l),
        trigger_height: Some(piece.trigger_h),
        trigger_shape: None,
        // Glade alpha is the shared roof pitch: stream it in and keep it.
        // `SavedPiece.trigger_alpha` is the pitch (alpha * 2) for glade shapes
        // — the same unit `capture`/`SavedPiece::from_piece` writes — and the
        // loader divides it back into `Piece.trigger_alpha`. For every other
        // shape trigger opacity is only an editor-local cosmetic, so the
        // streamed value is ignored (the loader restores the default).
        trigger_alpha: if is_glade_shape(&piece.shape) {
            Some(model::sanitize_trigger_alpha(piece.trigger_alpha) * 2.0)
        } else {
            None
        },
        trigger_color: None,
        trigger_visible: None,
        name: None,
        group: None,
        locked: false,
    }
}

/// One editor piece as a wire piece, using the same defaults as the streamed
/// conversion (`session::SavedPiece::from`). `rid` is empty for a `Place` (the
/// store generates it) and the persisted rid for an `Update`.
fn to_world_piece(rid: &str, piece: &Piece) -> WorldPiece {
    let saved = session::SavedPiece::from(piece);
    let size = model::sanitize_trigger_size(piece.trigger_size);
    WorldPiece {
        rid: rid.to_string(),
        shape: saved.shape,
        x: saved.x,
        z: saved.z,
        elevation: saved.elevation,
        rot_deg: saved.rot_deg,
        pitch_deg: saved.pitch_deg,
        roll_deg: saved.roll_deg,
        dynamic: saved.dynamic,
        damage: saved.damage.unwrap_or_else(|| {
            model::shapes()
                .get(piece.shape)
                .map(|shape| shape.damage)
                .unwrap_or(0.0)
        }),
        interact: saved.interact.unwrap_or_else(|| "none".to_string()),
        trigger_w: size.x.clamp(0.1, 100.0),
        trigger_h: size.y.clamp(0.1, 100.0),
        trigger_l: size.z.clamp(0.1, 100.0),
        // The editor piece's alpha (glade roof pitch = alpha * 2), clamped to
        // the wire/store domain. Cosmetic for non-glade shapes; gameplay for
        // glade ones.
        trigger_alpha: model::sanitize_trigger_alpha(piece.trigger_alpha),
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS
}

fn angle_near(a: f32, b: f32) -> bool {
    let delta = (a - b).rem_euclid(360.0);
    delta.min(360.0 - delta) <= ANGLE_EPS_DEG
}

/// True when two wire pieces carry the same gameplay values (rids ignored).
/// Store canonicalization (degree wrapping, extent sanitizing) makes exact
/// equality the wrong test between a client value and an echoed one.
fn wire_matches(a: &WorldPiece, b: &WorldPiece) -> bool {
    a.shape == b.shape
        && a.dynamic == b.dynamic
        && near(a.x, b.x)
        && near(a.z, b.z)
        && near(a.elevation, b.elevation)
        && angle_near(a.rot_deg, b.rot_deg)
        && angle_near(a.pitch_deg, b.pitch_deg)
        && angle_near(a.roll_deg, b.roll_deg)
        && near(a.damage, b.damage)
        && a.interact == b.interact
        && near(a.trigger_w, b.trigger_w)
        && near(a.trigger_h, b.trigger_h)
        && near(a.trigger_l, b.trigger_l)
        // Glade roof pitch is gameplay; for every other shape the alpha is an
        // editor-local opacity cosmetic that never round-trips.
        && (!is_glade_shape(&a.shape) || near(a.trigger_alpha, b.trigger_alpha))
}

/// True when a streamed wire piece equals an editor piece's gameplay values.
fn wire_matches_piece(wire: &WorldPiece, piece: &Piece) -> bool {
    wire_matches(wire, &to_world_piece("", piece))
}

/// True when two editor pieces carry the same persisted gameplay values.
/// `ignore_transform` skips position/rotation/elevation for pieces whose pose is
/// driven at runtime (physics bodies and mover-track platforms), so their
/// motion is never mistaken for a user edit.
fn same_persisted(a: &Piece, b: &Piece, ignore_transform: bool) -> bool {
    if a.shape != b.shape
        || a.dynamic != b.dynamic
        || a.interact != b.interact
        || (a.trigger.damage - b.trigger.damage).abs() > EPS
    {
        return false;
    }
    // A glade piece's alpha drives its roof shape: a roof-pitch edit must be
    // uploaded. For other shapes the alpha is an editor-local cosmetic.
    if is_glade_piece(a) && (a.trigger_alpha - b.trigger_alpha).abs() > EPS {
        return false;
    }
    let size_a = model::sanitize_trigger_size(a.trigger_size);
    let size_b = model::sanitize_trigger_size(b.trigger_size);
    if (size_a - size_b).length_squared() > EPS * EPS {
        return false;
    }
    if ignore_transform {
        return true;
    }
    a.pos == b.pos
        && a.elevation == b.elevation
        && a.rot == b.rot
        && a.pitch == b.pitch
        && a.roll == b.roll
}

/// One wire mover track as the loader's `SavedPlatform`, keyed by local id.
fn saved_platform(piece: u32, path: &WorldPath) -> session::SavedPlatform {
    session::SavedPlatform {
        piece,
        mode: path.mode.clone(),
        speed: Some(path.speed),
        paused: path.paused,
        waypoints: path
            .waypoints
            .iter()
            .map(|[x, z]| session::SavedPlatformPoint {
                x: *x,
                z: *z,
                incoming: None,
                outgoing: None,
            })
            .collect(),
    }
}

/// Write a composed world into the editor: pieces, movers and the dirty flag
/// only. Never touches camera, character, grass, vines or undo/redo, and keeps
/// outliner metadata for ids that survived.
fn install(editor: &mut Editor, movers: &mut Movers, loaded: session::Loaded) {
    let pieces = loaded.pieces;
    let live: HashSet<u32> = pieces.iter().map(|piece| piece.id).collect();

    // Keep labels/groups/locks for pieces that are still resident; drop rows
    // for pieces that unloaded (their ids will never come back).
    editor.outline.labels.retain(|id, _| live.contains(id));
    editor.outline.membership.retain(|id, _| live.contains(id));
    editor.outline.locked.retain(|id| live.contains(id));
    // The outliner only rebuilds when the revision moves.
    editor.outline.bump();

    // A selected piece that unloaded is no longer resident: drop just that
    // entry, keeping the order (and so the primary) of the ones that remain.
    let kept: Vec<u32> = editor
        .selection
        .ids()
        .iter()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    if kept.len() != editor.selection.len() {
        editor.selection.set_all(kept);
    }

    editor.pieces = pieces;
    // Movers are derived from the resident paths; replacing them wholesale is
    // how a streamed cell's tracks are updated/removed.
    super::movers::apply_saved(movers, &editor.pieces, &loaded.platforms);
    editor.next_id = editor.next_id.max(loaded.next_id);
    editor.hard_dirty = true;
    editor.note = "world synced";
}

/// Reconcile streamed cells *and* authoritative edits when the store changed.
///
/// Cheap in the common case (`dirty` is only set by Welcome, a chunk message or
/// an unload); the whole resident union is recomposed each time, with stable
/// ids keeping the renderer's work incremental. Net outcomes are reconciled and
/// the editor's current state is staged *before* composing, so an edit made in
/// the same frame as a streamed apply is never reverted by it.
pub(crate) fn apply_world_chunks(
    chunks: Option<ResMut<WorldChunks>>,
    mut sync: ResMut<WorldSync>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
    mut edits: ResMut<PendingWorldEdits>,
    time: Res<Time>,
    auth: Option<Res<crate::auth::AuthState>>,
) {
    let Some(mut chunks) = chunks else {
        // Net layer not present (e.g. headless editor tests): nothing to apply.
        return;
    };
    // Never install a world before login: Hello (and our identity) hasn't gone
    // out yet, so park the dirty flag until the session exists.
    if crate::auth::auth_required() {
        let signed_in = auth.is_some_and(|a| *a == crate::auth::AuthState::SignedIn);
        if !signed_in {
            return;
        }
    }

    let now = time.elapsed_secs_f64();
    let mut recompose = chunks.dirty;
    if sync.sync_edits(&editor.pieces, Some(&movers), &mut edits, now) {
        recompose = true;
    }
    let rolled_back = sync.take_rolled_back();
    if rolled_back {
        recompose = true;
    }
    if !recompose {
        return;
    }
    chunks.dirty = false;

    // Never allocate below ids the editor may already be using locally.
    sync.next_id = sync.next_id.max(editor.next_id.max(1));
    match sync.compose(&chunks.chunks, &mut edits) {
        Ok(loaded) => {
            install(&mut editor, &mut movers, loaded);
            sync.rebase(&editor.pieces);
            sync.clear_blocked(&editor.pieces);
            sync.clear_restoring();
            if rolled_back {
                editor.note = "edit rejected by the world server";
            }
        }
        Err(error) => {
            // Clearing `dirty` avoids retrying the same bad batch every frame;
            // the next chunk message retries with fresh data.
            bevy::log::warn!("dropping invalid streamed world: {error:?}");
        }
    }
}

/// Diff the editor every frame (the streamed store is only applied when it
/// changes) so edits made to pieces already in the world are queued even
/// without a chunk message. Runs after [`apply_world_chunks`].
pub(crate) fn sync_world_edits(
    mut sync: ResMut<WorldSync>,
    mut edits: ResMut<PendingWorldEdits>,
    mut editor: ResMut<Editor>,
    movers: Option<Res<Movers>>,
    chunks: Option<ResMut<WorldChunks>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    let recompose = sync.sync_edits(&editor.pieces, movers.as_deref(), &mut edits, now);
    if sync.take_rolled_back() {
        editor.note = "edit rejected by the world server";
    }
    if recompose && let Some(mut chunks) = chunks {
        // A rejected op dropped an optimistic change: install the streamed
        // state again on the next apply.
        chunks.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use party_dungeon_net::MAX_WORLD_OPS;

    fn piece(rid: &str, x: f32, z: f32) -> WorldPiece {
        WorldPiece {
            rid: rid.to_string(),
            shape: "block".to_string(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: 0.0,
            interact: "none".to_string(),
            trigger_w: 1.0,
            trigger_h: 1.0,
            trigger_l: 1.0,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
        }
    }

    fn chunk(cell: (i32, i32), version: u64, pieces: Vec<WorldPiece>) -> WorldChunk {
        WorldChunk {
            id: ChunkId {
                x: cell.0,
                z: cell.1,
            },
            version,
            pieces,
            paths: Vec::new(),
        }
    }

    fn store(chunks: impl IntoIterator<Item = WorldChunk>) -> HashMap<ChunkId, WorldChunk> {
        chunks.into_iter().map(|chunk| (chunk.id, chunk)).collect()
    }

    fn editor_piece(id: u32, x: f32, z: f32) -> Piece {
        Piece {
            id,
            shape: model::shape_index("block").expect("block shape"),
            pos: Vec2::new(x, z),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: model::Trigger::harmless(),
            trigger_size: model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: model::TriggerShape::Box,
            trigger_alpha: model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: model::TRIGGER_DEFAULT_VISIBLE,
            interact: model::InteractAction::None,
        }
    }

    /// Local ids keyed by rid, sorted for comparison.
    fn mapping(sync: &WorldSync) -> Vec<(String, u32)> {
        let mut mapped: Vec<(String, u32)> = sync
            .rid_to_id
            .iter()
            .map(|(rid, id)| (rid.clone(), *id))
            .collect();
        mapped.sort();
        mapped
    }

    /// Stage the editor's visible pieces and return the queued ops.
    fn stage(sync: &mut WorldSync, queue: &mut PendingWorldEdits, pieces: &[Piece], now: f64) {
        sync.sync_edits(pieces, None, queue, now);
    }

    /// Pretend the reliable channel accepted every queued op.
    fn flush_send(queue: &mut PendingWorldEdits) -> Vec<WorldOp> {
        let batch = queue.peek_batch();
        queue.commit_batch(batch.len());
        batch
    }

    fn op_keys(queue: &PendingWorldEdits) -> Vec<String> {
        queue
            .ops()
            .iter()
            .map(|op| match op {
                WorldOp::Place { tag, .. } => format!("place:{tag}"),
                WorldOp::Update { piece } => format!("update:{}", piece.rid),
                WorldOp::Delete { rid } => format!("delete:{rid}"),
            })
            .collect()
    }

    #[test]
    fn compose_is_deterministic_and_incremental() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([
            chunk(
                (1, 0),
                1,
                vec![piece("b", 34.0, 0.0), piece("a", 33.0, 0.0)],
            ),
            chunk((0, 0), 1, vec![piece("c", 0.0, 0.0)]),
        ]);

        // Cells by (z, x), pieces by rid: c first, then a, then b.
        let first = sync.compose(&chunks, &mut queue).expect("first compose");
        assert_eq!(
            first.pieces.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            mapping(&sync),
            vec![
                ("a".to_string(), 2),
                ("b".to_string(), 3),
                ("c".to_string(), 1),
            ]
        );
        sync.rebase(&first.pieces);

        // A re-sent world reuses every id and keeps the order.
        let before = mapping(&sync);
        let second = sync.compose(&chunks, &mut queue).expect("second compose");
        assert_eq!(
            second.pieces.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(mapping(&sync), before);
        assert_eq!(sync.next_id, 4, "a re-apply allocates nothing");
        assert!(queue.ops().is_empty(), "a re-apply stages no edit");
    }

    #[test]
    fn an_edit_keeps_ids_and_moves_the_piece() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let before = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let first = sync.compose(&before, &mut queue).expect("first compose");
        let id = first.pieces[0].id;
        sync.rebase(&first.pieces);

        let edited = store([chunk((0, 0), 2, vec![piece("a", 5.0, -2.0)])]);
        let second = sync.compose(&edited, &mut queue).expect("edited compose");
        assert_eq!(second.pieces.len(), 1);
        assert_eq!(second.pieces[0].id, id, "an edit never renumbers");
        assert_eq!(second.pieces[0].pos, Vec2::new(5.0, -2.0));
    }

    #[test]
    fn unload_purges_ids_and_a_readded_piece_is_new() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let mut chunks = store([
            chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)]),
            chunk((1, 0), 1, vec![piece("b", 32.0, 0.0)]),
        ]);
        let loaded = sync
            .compose(&chunks, &mut queue)
            .expect("compose both cells");
        sync.rebase(&loaded.pieces);
        assert_eq!(mapping(&sync).len(), 2);
        let id_a = sync.rid_to_id["a"];

        // Cell (1, 0) leaves the stream: b's geometry and mapping go with it.
        chunks.remove(&ChunkId { x: 1, z: 0 });
        let remaining = sync.compose(&chunks, &mut queue).expect("compose one cell");
        assert_eq!(
            remaining.pieces.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![id_a],
            "the surviving piece keeps its id"
        );
        assert_eq!(mapping(&sync), vec![("a".to_string(), id_a)]);
        assert!(queue.ops().is_empty(), "an unload is not a user deletion");

        // Walking back streams b again — as a fresh piece, never the old id.
        chunks.insert(
            ChunkId { x: 1, z: 0 },
            chunk((1, 0), 2, vec![piece("b", 32.0, 0.0)]),
        );
        let readded = sync.compose(&chunks, &mut queue).expect("re-add cell");
        let id_b = sync.rid_to_id["b"];
        assert_ne!(id_b, id_a);
        assert!(readded.pieces.iter().any(|p| p.id == id_b));
        assert_eq!(sync.next_id, id_b + 1, "next_id stays monotonic");
    }

    #[test]
    fn an_empty_store_composes_an_empty_world() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let mut chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose one cell");
        sync.rebase(&loaded.pieces);
        chunks.clear();

        let empty = sync.compose(&chunks, &mut queue).expect("compose empty");
        assert!(empty.pieces.is_empty());
        assert!(empty.platforms.is_empty());
        assert!(mapping(&sync).is_empty(), "unloaded ids are purged");
    }

    #[test]
    fn paths_are_keyed_to_their_pieces_stable_id() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let mut path_chunk = chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)]);
        path_chunk.paths.push(WorldPath {
            piece_rid: "a".to_string(),
            mode: "pingpong".to_string(),
            speed: 2.0,
            paused: true,
            waypoints: vec![[0.0, 0.0], [4.0, 0.0]],
        });
        let chunks = store([path_chunk]);
        let loaded = sync
            .compose(&chunks, &mut queue)
            .expect("compose with a mover");
        assert_eq!(loaded.platforms.len(), 1);
        assert_eq!(loaded.platforms[0].piece, loaded.pieces[0].id);
        assert_eq!(loaded.platforms[0].mode, "pingpong");
        assert!(loaded.platforms[0].paused);
    }

    #[test]
    fn a_local_placement_is_merged_once_with_a_stable_id() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let first = sync.compose(&chunks, &mut queue).expect("first compose");
        sync.rebase(&first.pieces);
        let id_a = first.pieces[0].id;

        // The user places a new piece; the editor already gave it id 2.
        let mut pieces = first.pieces.clone();
        pieces.push(editor_piece(2, 4.0, 4.0));
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["place:local-2".to_string()]);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        assert_eq!(queue.ops().len(), 1, "one Place per placement");
        flush_send(&mut queue);

        // Compose merges the local in, after the streamed piece, once.
        let second = sync
            .compose(&chunks, &mut queue)
            .expect("compose with a local");
        assert_eq!(
            second.pieces.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![id_a, 2]
        );
        sync.rebase(&second.pieces);
        let third = sync.compose(&chunks, &mut queue).expect("recompose");
        assert_eq!(
            third.pieces.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![id_a, 2],
            "a local never duplicates or renumbers across composes"
        );
        assert!(queue.ops().is_empty(), "staged ops are not re-queued");
    }

    #[test]
    fn a_placement_is_sent_once_and_the_echo_reuses_its_local_id() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        // An empty resident cell, like a fresh editor in a streamed world.
        let chunks = store([chunk((0, 0), 1, Vec::new())]);
        let empty = sync.compose(&chunks, &mut queue).expect("empty compose");
        sync.rebase(&empty.pieces);

        let pieces = vec![editor_piece(1, 3.0, 4.0)];
        stage(&mut sync, &mut queue, &pieces, 0.0);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        assert_eq!(op_keys(&queue), vec!["place:local-1".to_string()]);

        // The reliable channel accepts it; the sim answers Applied with a rid
        // and then echoes the changed cell.
        flush_send(&mut queue);
        stage(&mut sync, &mut queue, &pieces, 0.2);
        queue.note_applied("local-1".to_string(), "dungeon_piece:x".to_string());
        stage(&mut sync, &mut queue, &pieces, 0.3);
        assert!(
            queue.ops().is_empty(),
            "no Update for an unedited placement"
        );
        assert_eq!(sync.rid_to_id["dungeon_piece:x"], 1);
        assert!(sync.locals.is_empty());

        let echoed = store([chunk((0, 0), 2, vec![piece("dungeon_piece:x", 3.0, 4.0)])]);
        let installed = sync.compose(&echoed, &mut queue).expect("echo compose");
        sync.rebase(&installed.pieces);
        assert_eq!(installed.pieces.len(), 1, "the echo does not duplicate");
        assert_eq!(installed.pieces[0].id, 1, "the echo reuses the local id");

        // The echo must not be mistaken for a user edit (Lead: echo loop).
        stage(&mut sync, &mut queue, &installed.pieces, 0.4);
        assert!(
            queue.ops().is_empty(),
            "the echoed state is the baseline now"
        );
    }

    #[test]
    fn an_applied_placement_stays_visible_until_its_echo_arrives() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, Vec::new())]);
        let empty = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&empty.pieces);

        let pieces = vec![editor_piece(1, 2.0, 3.0)];
        stage(&mut sync, &mut queue, &pieces, 0.0);
        flush_send(&mut queue);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        queue.note_applied("local-1".to_string(), "dungeon_piece:x".to_string());
        stage(&mut sync, &mut queue, &pieces, 0.2);

        // A chunk for another cell makes the store apply before the echo for
        // our placement arrives: the optimistic piece must keep its id and stay
        // visible (no vanish, no duplicate).
        let mut interim_store = chunks.clone();
        interim_store.insert(ChunkId { x: 1, z: 0 }, chunk((1, 0), 1, Vec::new()));
        let interim = sync
            .compose(&interim_store, &mut queue)
            .expect("interim compose");
        assert_eq!(interim.pieces.len(), 1);
        assert_eq!(interim.pieces[0].id, 1);
        assert_eq!(interim.pieces[0].pos, Vec2::new(2.0, 3.0));

        // The echo finally streams it: one piece, still the same id.
        let mut echoed = interim_store.clone();
        echoed.insert(
            ChunkId { x: 0, z: 0 },
            chunk((0, 0), 2, vec![piece("dungeon_piece:x", 2.0, 3.0)]),
        );
        let installed = sync.compose(&echoed, &mut queue).expect("echo compose");
        assert_eq!(installed.pieces.len(), 1);
        assert_eq!(installed.pieces[0].id, 1);

        // And an edit made in that window is not lost: it is still queued as an
        // Update once the rid is known.
        let mut edited = installed.pieces.clone();
        edited[0].pos = Vec2::new(4.0, 3.0);
        stage(&mut sync, &mut queue, &edited, 0.4);
        assert_eq!(op_keys(&queue), vec!["update:dungeon_piece:x".to_string()]);
    }

    #[test]
    fn a_rejected_delete_reinstalls_the_piece_without_re_deleting_it() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);

        // The user deletes the piece and the store refuses the write.
        stage(&mut sync, &mut queue, &[], 0.0);
        assert_eq!(op_keys(&queue), vec!["delete:a".to_string()]);
        flush_send(&mut queue);
        queue.note_rejected("a".to_string(), "write failed".to_string());
        assert!(
            stage_returns_recompose(&mut sync, &mut queue, &[], 0.1),
            "recompose requested"
        );

        let restored = sync.compose(&chunks, &mut queue).expect("rollback compose");
        sync.rebase(&restored.pieces);
        sync.clear_restoring();
        assert_eq!(
            restored.pieces.len(),
            1,
            "the rejected delete brings it back"
        );

        // The diff must not immediately delete it again.
        stage(&mut sync, &mut queue, &restored.pieces, 0.2);
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn updates_are_throttled_per_piece_and_send_the_latest_state() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let first = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&first.pieces);

        let mut pieces = first.pieces.clone();
        pieces[0].pos = Vec2::new(1.0, 0.0);
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["update:a".to_string()]);
        let sent = flush_send(&mut queue);
        match &sent[0] {
            WorldOp::Update { piece } => assert_eq!(piece.x, 1.0),
            other => panic!("expected Update, got {other:?}"),
        }

        // A second edit inside the window queues nothing yet.
        pieces[0].pos = Vec2::new(2.0, 0.0);
        stage(&mut sync, &mut queue, &pieces, 0.05);
        assert!(queue.ops().is_empty(), "throttled while the window is open");

        // After the window, the latest state goes out once.
        stage(&mut sync, &mut queue, &pieces, 0.30);
        assert_eq!(op_keys(&queue), vec!["update:a".to_string()]);
        let sent = flush_send(&mut queue);
        match &sent[0] {
            WorldOp::Update { piece } => assert_eq!(piece.x, 2.0),
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn a_delete_is_sent_only_while_the_cell_is_resident() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([
            chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)]),
            chunk((1, 0), 1, vec![piece("b", 32.0, 0.0)]),
        ]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);
        assert_eq!(sync.rid_cell["b"], ChunkId { x: 1, z: 0 });

        // The user deletes b while its cell is resident: that is a real delete.
        let pieces = vec![loaded.pieces[0]];
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["delete:b".to_string()]);

        // Its cell unloads before the op went out: the deletion is forgotten
        // silently (the geometry left the stream; the user did not delete it
        // there) and no further op is staged.
        let mut unloaded = chunks.clone();
        unloaded.remove(&ChunkId { x: 1, z: 0 });
        sync.compose(&unloaded, &mut queue)
            .expect("compose unloaded");
        assert!(
            queue.ops().is_empty(),
            "an unload cancels a not-yet-sent delete"
        );
        stage(&mut sync, &mut queue, &pieces, 0.1);
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn a_confirmed_delete_is_forgotten_after_the_cell_echoes() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let mut chunks = store([chunk(
            (0, 0),
            1,
            vec![piece("a", 0.0, 0.0), piece("b", 2.0, 0.0)],
        )]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);
        let pieces: Vec<Piece> = loaded
            .pieces
            .iter()
            .copied()
            .filter(|p| p.id == loaded.pieces[0].id)
            .collect();

        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["delete:b".to_string()]);
        flush_send(&mut queue);

        // The store echoes the cell without b: the tombstone ends and its id
        // mapping is purged.
        chunks = store([chunk((0, 0), 2, vec![piece("a", 0.0, 0.0)])]);
        let confirmed = sync
            .compose(&chunks, &mut queue)
            .expect("confirmed compose");
        assert_eq!(confirmed.pieces.len(), 1);
        assert!(!sync.deleting.contains("b"));
        assert!(!sync.rid_to_id.contains_key("b"));
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn a_rejected_edit_rolls_back_to_the_streamed_state() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);
        let id = loaded.pieces[0].id;

        let mut pieces = loaded.pieces.clone();
        pieces[0].pos = Vec2::new(9.0, 9.0);
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["update:a".to_string()]);
        flush_send(&mut queue);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        assert!(!sync.overrides.is_empty(), "the optimistic edit is staged");

        queue.note_rejected("a".to_string(), "write failed".to_string());
        assert!(stage_returns_recompose(&mut sync, &mut queue, &pieces, 0.2));
        let restored = sync.compose(&chunks, &mut queue).expect("rollback compose");
        sync.rebase(&restored.pieces);
        assert_eq!(restored.pieces[0].id, id);
        assert_eq!(
            restored.pieces[0].pos,
            Vec2::ZERO,
            "the streamed state returns"
        );
        assert!(sync.overrides.is_empty());
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn a_rejected_placement_drops_its_optimistic_piece() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, Vec::new())]);
        let empty = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&empty.pieces);
        let pieces = vec![editor_piece(1, 3.0, 4.0)];

        stage(&mut sync, &mut queue, &pieces, 0.0);
        flush_send(&mut queue);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        queue.note_rejected("local-1".to_string(), "bad shape".to_string());
        stage(&mut sync, &mut queue, &pieces, 0.2);

        let installed = sync.compose(&chunks, &mut queue).expect("dropping compose");
        assert!(installed.pieces.is_empty(), "the rejected piece is gone");
        sync.clear_blocked(&installed.pieces);
        assert!(sync.locals.is_empty());
        assert!(sync.blocked_ids.is_empty());
        // A later diff must not immediately re-register the same piece.
        stage(&mut sync, &mut queue, &installed.pieces, 0.3);
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn a_pending_override_survives_the_streamed_echo_and_wins_over_it() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("a", 0.0, 0.0)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);

        let mut pieces = loaded.pieces.clone();
        pieces[0].pos = Vec2::new(5.0, 0.0);
        stage(&mut sync, &mut queue, &pieces, 0.0);
        flush_send(&mut queue);

        // A stale re-send of the old cell must not revert the optimistic edit.
        let loaded = sync.compose(&chunks, &mut queue).expect("stale compose");
        assert_eq!(
            loaded.pieces[0].pos,
            Vec2::new(5.0, 0.0),
            "the override wins until the store echoes a change"
        );

        // The store echoes it: the canonical value is adopted and the override
        // ends (no further Update is queued for it).
        let echoed = store([chunk((0, 0), 2, vec![piece("a", 5.0, 0.0)])]);
        let adopted = sync.compose(&echoed, &mut queue).expect("echo compose");
        sync.rebase(&adopted.pieces);
        assert_eq!(adopted.pieces[0].pos, Vec2::new(5.0, 0.0));
        assert!(
            sync.overrides.is_empty(),
            "the store answered: override ends"
        );
        stage(&mut sync, &mut queue, &adopted.pieces, 0.2);
        assert!(queue.ops().is_empty());
    }

    #[test]
    fn runtime_motion_is_not_uploaded_while_edits_still_are() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let mut wire = piece("a", 0.0, 0.0);
        wire.dynamic = true;
        let chunks = store([chunk((0, 0), 1, vec![wire])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);

        // A dynamic piece moves because physics drives it: no Update.
        let mut pieces = loaded.pieces.clone();
        pieces[0].pos = Vec2::new(0.5, 0.0);
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert!(queue.ops().is_empty(), "physics motion is not a user edit");

        // A real property edit on the same piece is still sent (with its
        // current pose, which the store then owns).
        pieces[0].trigger.damage = 3.0;
        stage(&mut sync, &mut queue, &pieces, 0.02);
        assert_eq!(op_keys(&queue), vec!["update:a".to_string()]);
        match &queue.ops()[0] {
            WorldOp::Update { piece } => {
                assert_eq!(piece.damage, 3.0);
                assert_eq!(piece.x, 0.5);
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn batches_are_capped_at_max_world_ops() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, Vec::new())]);
        let empty = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&empty.pieces);

        let count = MAX_WORLD_OPS + 5;
        let pieces: Vec<Piece> = (1..=count as u32)
            .map(|id| editor_piece(id, id as f32, 0.0))
            .collect();
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(queue.queued(), count, "every placement is queued");
        assert_eq!(
            queue.peek_batch().len(),
            MAX_WORLD_OPS,
            "one message never exceeds the protocol bound"
        );
    }

    fn stage_returns_recompose(
        sync: &mut WorldSync,
        queue: &mut PendingWorldEdits,
        pieces: &[Piece],
        now: f64,
    ) -> bool {
        sync.sync_edits(pieces, None, queue, now)
    }

    fn apply_app() -> App {
        let mut app = App::new();
        app.init_resource::<Editor>();
        app.init_resource::<Movers>();
        app.init_resource::<WorldSync>();
        app.init_resource::<WorldChunks>();
        app.init_resource::<PendingWorldEdits>();
        app.init_resource::<Time>();
        // The apply parks until the session is signed in.
        app.insert_resource(crate::auth::AuthState::SignedIn);
        app
    }

    fn stream(app: &mut App) {
        app.world_mut()
            .run_system_once(apply_world_chunks)
            .expect("apply world chunks");
    }

    #[test]
    fn an_apply_preserves_editor_state_and_purges_unloaded_pieces() {
        let mut app = apply_app();
        app.world_mut().resource_mut::<WorldChunks>().insert(chunk(
            (0, 0),
            1,
            vec![piece("a", 0.0, 0.0), piece("b", 2.0, 0.0)],
        ));
        stream(&mut app);

        let (id_a, id_b) = {
            let editor = app.world().resource::<Editor>();
            assert_eq!(editor.pieces.len(), 2);
            (editor.pieces[0].id, editor.pieces[1].id)
        };
        {
            // Session-local editor state that a streamed update must not reset.
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.selection.set(id_a);
            let undo = editor.snapshot();
            editor.undo.push(undo);
            editor.outline.labels.insert(id_a, "keep me".to_string());
            editor.outline.labels.insert(id_b, "gone soon".to_string());
            editor.outline.locked.insert(id_a);
        }

        // The cell is re-sent version 2: "a" moved, "c" is new, "b" is gone.
        app.world_mut().resource_mut::<WorldChunks>().insert(chunk(
            (0, 0),
            2,
            vec![piece("a", 5.0, -2.0), piece("c", 0.0, 4.0)],
        ));
        stream(&mut app);

        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.pieces.len(), 2);
        let moved = editor
            .pieces
            .iter()
            .find(|piece| piece.id == id_a)
            .expect("the surviving piece keeps its id");
        assert_eq!(
            moved.pos,
            Vec2::new(5.0, -2.0),
            "remote edit applied in place"
        );
        assert!(
            !editor.pieces.iter().any(|piece| piece.id == id_b),
            "a removed piece disappears"
        );
        assert!(
            editor.pieces.iter().any(|piece| piece.id > id_b),
            "a new piece gets a fresh id"
        );
        // Editor state survives the streamed update.
        assert_eq!(editor.selection.primary(), Some(id_a));
        assert_eq!(editor.undo.len(), 1);
        assert_eq!(editor.outline.label(id_a), Some("keep me"));
        assert!(editor.outline.is_locked(id_a));
        // The unloaded piece's outliner row is gone.
        assert!(editor.outline.label(id_b).is_none());
        assert!(editor.hard_dirty);
        // The remote edit is not echoed back as a local user edit.
        let queue = app.world().resource::<PendingWorldEdits>();
        assert!(queue.ops().is_empty(), "streamed changes are the baseline");
    }

    #[test]
    fn an_edit_is_sent_after_a_streamed_apply_without_being_reverted() {
        let mut app = apply_app();
        app.world_mut().resource_mut::<WorldChunks>().insert(chunk(
            (0, 0),
            1,
            vec![piece("a", 0.0, 0.0)],
        ));
        stream(&mut app);
        let id = app.world().resource::<Editor>().pieces[0].id;

        // The user moves the piece: the diff queues one Update.
        app.world_mut().resource_mut::<Editor>().pieces[0].pos = Vec2::new(6.0, 0.0);
        app.world_mut()
            .run_system_once(sync_world_edits)
            .expect("diff");
        {
            let queue = app.world().resource::<PendingWorldEdits>();
            assert_eq!(queue.queued(), 1, "the user edit is queued");
        }

        // A chunk message for the *stale* cell arrives in the same frame as the
        // edit: the apply must keep the optimistic move, not revert it.
        app.world_mut().resource_mut::<WorldChunks>().insert(chunk(
            (0, 0),
            1,
            vec![piece("a", 0.0, 0.0)],
        ));
        stream(&mut app);
        let editor = app.world().resource::<Editor>();
        assert_eq!(
            editor.pieces[0].pos,
            Vec2::new(6.0, 0.0),
            "an edit staged this frame survives a streamed apply"
        );
        assert_eq!(editor.pieces[0].id, id);
    }

    #[test]
    fn applying_an_empty_store_clears_the_world_but_keeps_the_camera_and_undo() {
        let mut app = apply_app();
        app.world_mut().resource_mut::<WorldChunks>().insert(chunk(
            (0, 0),
            1,
            vec![piece("a", 0.0, 0.0)],
        ));
        stream(&mut app);
        let id = app.world().resource::<Editor>().pieces[0].id;
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            editor.selection.set(id);
            let undo = editor.snapshot();
            editor.undo.push(undo);
        }

        {
            let mut chunks = app.world_mut().resource_mut::<WorldChunks>();
            chunks.unload(&[ChunkId { x: 0, z: 0 }]);
        }
        stream(&mut app);

        let editor = app.world().resource::<Editor>();
        assert!(editor.pieces.is_empty(), "an empty store is an empty world");
        assert!(
            app.world().resource::<Movers>().map.is_empty(),
            "no mover survives its piece"
        );
        assert!(
            editor.selection.is_empty(),
            "the unloaded selection is dropped"
        );
        assert_eq!(editor.undo.len(), 1, "undo history is preserved");
        assert!(editor.hard_dirty);
        assert!(
            app.world().resource::<PendingWorldEdits>().ops().is_empty(),
            "an unload is not a user deletion"
        );
    }

    /// A streamed glade piece whose alpha is a roof pitch, not the default.
    fn glade_tower_wire(rid: &str, alpha: f32) -> WorldPiece {
        let mut wire = piece(rid, 0.0, 0.0);
        wire.shape = "glade_tower".to_string();
        wire.trigger_alpha = alpha;
        wire
    }

    #[test]
    fn a_streamed_glade_roof_pitch_reaches_the_editor() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![glade_tower_wire("tower", 0.6)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");

        let shape = model::shape_index("glade_tower").expect("glade_tower shape");
        assert_eq!(loaded.pieces[0].shape, shape);
        assert!(
            (loaded.pieces[0].trigger_alpha - 0.6).abs() < 1e-5,
            "the streamed roof-pitch alpha is installed, got {}",
            loaded.pieces[0].trigger_alpha
        );
        // The roof is built from the streamed alpha (pitch = alpha * 2), so a
        // freshly joined client no longer renders the default roof.
        let pitch = crate::glade::roof_pitch(&loaded.pieces[0], &model::shapes()[shape]);
        assert!((pitch - 1.2).abs() < 1e-5, "roof pitch is {pitch}");

        // A streamed pitch is acknowledged gameplay, never echoed as an edit.
        sync.rebase(&loaded.pieces);
        stage(&mut sync, &mut queue, &loaded.pieces, 0.1);
        assert!(
            queue.ops().is_empty(),
            "a streamed roof pitch is the baseline, not a user edit"
        );
    }

    #[test]
    fn a_glade_roof_pitch_edit_is_uploaded_but_other_opacity_is_cosmetic() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![glade_tower_wire("tower", 0.35)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);

        // Raising the roof pitch is a gameplay edit: exactly one Update.
        let mut pieces = loaded.pieces.clone();
        pieces[0].trigger_alpha = 0.6;
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["update:tower".to_string()]);
        match &queue.ops()[0] {
            WorldOp::Update { piece } => {
                assert_eq!(piece.shape, "glade_tower");
                assert!((piece.trigger_alpha - 0.6).abs() < 1e-6);
            }
            other => panic!("expected Update, got {other:?}"),
        }
        flush_send(&mut queue);

        // A non-glade trigger's opacity stays an editor-local cosmetic.
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![piece("block", 0.0, 0.0)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);
        let mut pieces = loaded.pieces.clone();
        pieces[0].trigger_alpha = 0.9;
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert!(
            queue.ops().is_empty(),
            "non-glade trigger opacity is editor-local"
        );
    }

    #[test]
    fn a_streamed_roof_pitch_answer_ends_the_override() {
        let mut sync = WorldSync::default();
        let mut queue = PendingWorldEdits::default();
        let chunks = store([chunk((0, 0), 1, vec![glade_tower_wire("tower", 0.35)])]);
        let loaded = sync.compose(&chunks, &mut queue).expect("compose");
        sync.rebase(&loaded.pieces);

        let mut pieces = loaded.pieces.clone();
        pieces[0].trigger_alpha = 0.6;
        stage(&mut sync, &mut queue, &pieces, 0.0);
        assert_eq!(op_keys(&queue), vec!["update:tower".to_string()]);
        flush_send(&mut queue);
        stage(&mut sync, &mut queue, &pieces, 0.1);
        assert!(!sync.overrides.is_empty(), "the roof edit is optimistic");

        // The store echoes the canonical pitch: the override is acknowledged
        // and ends (a cosmetic-only field would keep it alive forever).
        let echoed = store([chunk((0, 0), 2, vec![glade_tower_wire("tower", 0.6)])]);
        let adopted = sync.compose(&echoed, &mut queue).expect("echo compose");
        sync.rebase(&adopted.pieces);
        assert!((adopted.pieces[0].trigger_alpha - 0.6).abs() < 1e-5);
        assert!(
            sync.overrides.is_empty(),
            "the store answered the roof edit: override ends"
        );
        stage(&mut sync, &mut queue, &adopted.pieces, 0.2);
        assert!(queue.ops().is_empty());
    }
}
