//! Server-side spatial interest management: the level chunk index, per-peer
//! streaming views, and per-receiver player filtering.
//!
//! A vast dungeon cannot be pushed to a client wholesale. Every level piece is
//! assigned to the fixed [`CHUNK_SIZE`](party_dungeon_net::CHUNK_SIZE) XZ cell
//! containing its origin, each cell carries a version that only moves when its
//! content changes, and each peer is streamed only the cells around its
//! character — bounded by [`CHUNK_SEND_BUDGET`](party_dungeon_net::CHUNK_SEND_BUDGET)
//! per tick, with radius + unload-margin hysteresis so pacing across a cell
//! border does not thrash the stream. Snapshots are filtered the same way for
//! players (entry radius, exit radius + margin, self always visible).
//!
//! Everything here is pure data + functions: no sockets, no `matchbox`, no
//! async. [`crate::net::HostNet`] owns one [`ChunkIndex`] and one
//! [`PeerView`] per connected peer, calls [`ChunkIndex::sync`] and
//! [`PeerView::tick`] once per tick, and encodes the resulting
//! [`StreamUpdate`] into `ServerMsg::WorldChunk` / `WorldUnload`.

use std::collections::{HashMap, HashSet, VecDeque};

use party_dungeon::model;
use party_dungeon_net::{
    chunks_within, sanitize_radius, ChunkId, PlayerState, Snapshot, WorldChunk, WorldPath,
    WorldPiece, CHUNK_SIZE, DEFAULT_PLAYER_RADIUS, DEFAULT_WORLD_RADIUS, MAX_PLAYER_RADIUS,
    MAX_WORLD_RADIUS, PLAYER_UNLOAD_MARGIN, WORLD_UNLOAD_MARGIN,
};

use crate::sim::Sim;
use crate::world::SimMover;

/// Apply the wire order for cells: `(z, x)`, exactly like
/// [`chunks_within`](party_dungeon_net::chunks_within). Deterministic on both
/// ends and in tests.
fn sort_cells(cells: &mut [ChunkId]) {
    cells.sort_by_key(|c| (c.z, c.x));
}

// ---------------------------------------------------------------------------
// Cell content
// ---------------------------------------------------------------------------

/// The wire geometry of one cell: the pieces whose origin lies in it and the
/// mover tracks of those pieces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellContent {
    pub pieces: Vec<WorldPiece>,
    pub paths: Vec<WorldPath>,
}

impl CellContent {
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty() && self.paths.is_empty()
    }
}

/// Wire piece for one sim piece. Mirrors the DB row shape (`dungeon_piece`);
/// editor-only cosmetics (trigger color/visibility, outliner names) never
/// leave the editor.
fn world_piece(rid: &str, piece: &model::Piece, shape_id: &str) -> WorldPiece {
    WorldPiece {
        rid: rid.to_string(),
        shape: shape_id.to_string(),
        // `piece.pos` is the ground XZ position: x = world X, y = world Z.
        x: piece.pos.x,
        z: piece.pos.y,
        elevation: piece.elevation,
        rot_deg: piece.rot.to_degrees(),
        pitch_deg: piece.pitch.to_degrees(),
        roll_deg: piece.roll.to_degrees(),
        dynamic: piece.dynamic,
        damage: piece.trigger.damage,
        interact: piece.interact.id().to_string(),
        trigger_w: piece.trigger_size.x,
        trigger_h: piece.trigger_size.y,
        trigger_l: piece.trigger_size.z,
        trigger_alpha: piece.trigger_alpha,
    }
}

/// Wire mover track for one sim mover.
fn world_path(piece_rid: &str, mover: &SimMover) -> WorldPath {
    WorldPath {
        piece_rid: piece_rid.to_string(),
        mode: mover.mode.id().to_string(),
        speed: mover.speed,
        paused: mover.paused,
        waypoints: mover
            .waypoints
            .iter()
            .map(|w| [w.pos.x, w.pos.y])
            .collect(),
    }
}

/// Partition the sim's world into cells. A piece belongs to the cell
/// containing its origin (`floor`, not truncation, so negative coordinates
/// land correctly); a mover track belongs to its piece's cell.
fn cells_from_sim(sim: &Sim) -> HashMap<ChunkId, CellContent> {
    let catalogue = model::shapes();
    let mut cells: HashMap<ChunkId, CellContent> = HashMap::new();
    // Piece rid -> cell, so movers can follow their own piece.
    let mut piece_cell: HashMap<&str, ChunkId> = HashMap::new();
    for (rid, piece) in &sim.pieces {
        let Some(shape) = catalogue.get(piece.shape) else {
            continue;
        };
        let cell = ChunkId::containing(piece.pos.x, piece.pos.y);
        piece_cell.insert(rid.as_str(), cell);
        cells
            .entry(cell)
            .or_default()
            .pieces
            .push(world_piece(rid, piece, shape.id));
    }
    for (piece_rid, mover) in &sim.world.movers {
        let Some(cell) = piece_cell.get(piece_rid.as_str()).copied() else {
            continue; // a track without a piece is not streamed
        };
        cells
            .entry(cell)
            .or_default()
            .paths
            .push(world_path(piece_rid, mover));
    }
    // Content equality (and therefore the client's view) must not depend on
    // DB iteration order: canonicalize per cell.
    for content in cells.values_mut() {
        content.pieces.sort_by(|a, b| a.rid.cmp(&b.rid));
        content.paths.sort_by(|a, b| a.piece_rid.cmp(&b.piece_rid));
    }
    cells
}

// ---------------------------------------------------------------------------
// Chunk index
// ---------------------------------------------------------------------------

/// Spatial index of the streamed world with per-cell versions.
///
/// Rebuilt whenever the sim's `dungeon_version` moves. A rebuild content-diffs
/// every cell: unchanged cells keep their version (clients ignore them), changed
/// cells get a fresh one, and a cell that became empty gets a *tombstone*
/// version so peers still holding it are told to clear. Versions come from a
/// monotonic counter, so a version is never repeated for a cell — not even
/// after an editor clears and later repopulates it.
#[derive(Clone, Debug)]
pub struct ChunkIndex {
    /// Cells with geometry, keyed by cell.
    cells: HashMap<ChunkId, CellContent>,
    /// Last version issued for every cell ever seen (content or tombstone).
    versions: HashMap<ChunkId, u64>,
    /// Monotonic version counter. Never decreases, so versions never repeat.
    next_version: u64,
    /// `dungeon_version` this index reflects (`None` before the first sync).
    built_version: Option<u64>,
}

impl ChunkIndex {
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
            versions: HashMap::new(),
            next_version: 1,
            built_version: None,
        }
    }

    /// The dungeon version this index reflects, if it was ever synced.
    pub fn built_version(&self) -> Option<u64> {
        self.built_version
    }

    /// Rebuild from the sim when its `dungeon_version` moved. Returns `true`
    /// when the index was rebuilt (i.e. cell content may have changed).
    pub fn sync(&mut self, sim: &Sim) -> bool {
        if self.built_version == Some(sim.dungeon_version) {
            return false;
        }
        self.rebuild(cells_from_sim(sim));
        self.built_version = Some(sim.dungeon_version);
        true
    }

    /// Reindex `cells`, bumping the version of exactly the cells whose content
    /// changed and tombstoning cells that became empty.
    ///
    /// Versions are allocated in `(z, x)` order from the monotonic counter, so
    /// they are deterministic run to run.
    pub fn rebuild(&mut self, mut cells: HashMap<ChunkId, CellContent>) {
        cells.retain(|_, content| !content.is_empty());
        let mut next = self.next_version;

        // Tombstones first: a cell that disappeared gets a fresh version so
        // peers holding it are told to clear, and a later repopulate gets an
        // even newer one (never a repeat of the old version).
        let mut gone: Vec<ChunkId> = self
            .cells
            .keys()
            .filter(|id| !cells.contains_key(*id))
            .copied()
            .collect();
        sort_cells(&mut gone);
        for id in gone {
            self.versions.insert(id, next);
            next += 1;
        }

        // Then changed cells: unchanged content keeps its version.
        let mut changed: Vec<ChunkId> = cells
            .keys()
            .filter(|id| self.cells.get(*id) != cells.get(*id))
            .copied()
            .collect();
        sort_cells(&mut changed);
        for id in changed {
            self.versions.insert(id, next);
            next += 1;
        }

        self.next_version = next;
        self.cells = cells;
    }

    /// Cells with geometry, sorted by `(z, x)` (the wire order).
    pub fn cell_ids(&self) -> Vec<ChunkId> {
        let mut ids: Vec<ChunkId> = self.cells.keys().copied().collect();
        sort_cells(&mut ids);
        ids
    }

    /// True when the cell currently has geometry (not just a tombstone).
    pub fn has_content(&self, id: ChunkId) -> bool {
        self.cells.contains_key(&id)
    }

    /// Geometry of one cell, when it has any.
    pub fn content(&self, id: ChunkId) -> Option<&CellContent> {
        self.cells.get(&id)
    }

    /// Last version issued for `id` (content or tombstone).
    pub fn version(&self, id: ChunkId) -> Option<u64> {
        self.versions.get(&id).copied()
    }

    /// Wire message for `id`: its geometry, or an empty chunk for a tombstone
    /// (which tells a peer holding the cell to drop it).
    pub fn chunk(&self, id: ChunkId) -> Option<WorldChunk> {
        let version = self.version(id)?;
        Some(match self.cells.get(&id) {
            Some(content) => WorldChunk {
                id,
                version,
                pieces: content.pieces.clone(),
                paths: content.paths.clone(),
            },
            None => WorldChunk::empty(id, version),
        })
    }

    /// Number of cells with geometry.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// True when no cell has geometry.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

impl Default for ChunkIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Per-peer streaming view
// ---------------------------------------------------------------------------

/// One tick's worth of streaming work for one peer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamUpdate {
    /// Cell messages to send on the reliable channel, in `(z, x)` order.
    pub chunks: Vec<WorldChunk>,
    /// Cells that left the interest window entirely; drop their geometry.
    pub unload: Vec<ChunkId>,
}

impl StreamUpdate {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.unload.is_empty()
    }
}

/// FIFO of cells waiting for a send slot, deduplicated so a cell that keeps
/// changing is queued at most once (its version is read when it is sent).
#[derive(Clone, Debug, Default)]
struct PendingQueue {
    order: VecDeque<ChunkId>,
    queued: HashSet<ChunkId>,
}

impl PendingQueue {
    fn push(&mut self, id: ChunkId) {
        if self.queued.insert(id) {
            self.order.push_back(id);
        }
    }

    fn pop(&mut self) -> Option<ChunkId> {
        let id = self.order.pop_front()?;
        self.queued.remove(&id);
        Some(id)
    }

    /// Drop queued cells the closure rejects (they left the interest set).
    fn retain(&mut self, keep: impl Fn(&ChunkId) -> bool) {
        let mut dropped: Vec<ChunkId> = Vec::new();
        self.order.retain(|id| {
            let keep_it = keep(id);
            if !keep_it {
                dropped.push(*id);
            }
            keep_it
        });
        for id in dropped {
            self.queued.remove(&id);
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

/// Everything the streaming layer knows about one connected peer: how much of
/// the world it wants, which cells it currently holds (and at which version),
/// what is queued, and which players it currently sees.
#[derive(Clone, Debug)]
pub struct PeerView {
    /// Level-streaming radius in metres; `None` = the whole world.
    pub world_radius: Option<f32>,
    /// Other-player visibility radius in metres; `None` = everyone.
    pub player_radius: Option<f32>,
    /// Cell -> version last handed to this peer (what it holds).
    sent: HashMap<ChunkId, u64>,
    /// Changed cells waiting for a send slot (bounded per tick).
    pending: PendingQueue,
    /// Players visible in the peer's last snapshot (exit hysteresis).
    pub visible_players: HashSet<String>,
}

impl PeerView {
    /// A fresh view at the default radii. Used when a peer completes `Hello`;
    /// its cells then stream in over the next few ticks.
    pub fn new() -> Self {
        Self {
            world_radius: Some(DEFAULT_WORLD_RADIUS),
            player_radius: Some(DEFAULT_PLAYER_RADIUS),
            sent: HashMap::new(),
            pending: PendingQueue::default(),
            visible_players: HashSet::new(),
        }
    }

    /// Apply a client `SetInterest`. `None` means "no limit"; `Some` is
    /// clamped so a client can never ask for an unbounded stream.
    pub fn set_interest(&mut self, world_radius: Option<f32>, player_radius: Option<f32>) {
        self.world_radius = sanitize_radius(world_radius, MAX_WORLD_RADIUS);
        self.player_radius = sanitize_radius(player_radius, MAX_PLAYER_RADIUS);
    }

    /// Version of `id` this peer was last sent, when it still holds the cell.
    pub fn sent_version(&self, id: ChunkId) -> Option<u64> {
        self.sent.get(&id).copied()
    }

    /// Cells this peer currently holds, sorted by `(z, x)`.
    pub fn held_cells(&self) -> Vec<ChunkId> {
        let mut ids: Vec<ChunkId> = self.sent.keys().copied().collect();
        sort_cells(&mut ids);
        ids
    }

    /// Chunks waiting for a send slot (after this tick's budget was spent).
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// One streaming tick for this peer, standing at `origin` (world XZ).
    ///
    /// * desired = non-empty cells within `world_radius` (all cells when
    ///   `None`) plus cells already held within `radius + WORLD_UNLOAD_MARGIN`;
    /// * held cells outside the desired set are batched into `unload`;
    /// * desired cells whose version differs from what this peer holds are
    ///   queued, then at most `budget` of them are emitted (the rest waits for
    ///   the next tick);
    /// * a held cell that became empty is sent as an empty chunk and then
    ///   dropped from the held set.
    pub fn tick(&mut self, index: &ChunkIndex, origin: [f32; 2], budget: usize) -> StreamUpdate {
        let cells = interest_cells(index, origin, self.world_radius, &self.sent);
        let wanted: HashSet<ChunkId> = cells.iter().copied().collect();

        // Everything this peer holds that is no longer in its interest window
        // leaves now, as one batched unload.
        let mut unload: Vec<ChunkId> = self
            .sent
            .keys()
            .filter(|id| !wanted.contains(*id))
            .copied()
            .collect();
        sort_cells(&mut unload);
        for id in &unload {
            self.sent.remove(id);
        }
        self.pending.retain(|id| wanted.contains(id));

        // Queue cells whose version moved since this peer was last sent them.
        for id in &cells {
            let Some(version) = index.version(*id) else {
                continue;
            };
            if self.sent.get(id) != Some(&version) {
                self.pending.push(*id);
            }
        }

        // Spend the per-tick budget. A queued cell whose version moved again
        // while waiting is sent at its *current* version.
        let mut chunks = Vec::new();
        let mut spent = 0usize;
        while spent < budget {
            let Some(id) = self.pending.pop() else {
                break;
            };
            if !wanted.contains(&id) {
                continue; // left the interest set while queued
            }
            let Some(version) = index.version(id) else {
                continue;
            };
            match index.content(id) {
                Some(content) => {
                    chunks.push(WorldChunk {
                        id,
                        version,
                        pieces: content.pieces.clone(),
                        paths: content.paths.clone(),
                    });
                    self.sent.insert(id, version);
                }
                None => {
                    // Tombstone: clear the cell, then stop holding it so it is
                    // not re-sent (and so it cannot pin a large held set).
                    chunks.push(WorldChunk::empty(id, version));
                    self.sent.remove(&id);
                }
            }
            spent += 1;
        }

        StreamUpdate { chunks, unload }
    }
}

impl Default for PeerView {
    fn default() -> Self {
        Self::new()
    }
}

/// The cells a peer should hold: non-empty cells within `radius` (every
/// non-empty cell when `radius` is `None`) plus already-held cells still
/// within `radius + WORLD_UNLOAD_MARGIN` — the unload hysteresis.
///
/// Returned sorted by `(z, x)`, deduplicated.
pub fn interest_cells(
    index: &ChunkIndex,
    origin: [f32; 2],
    radius: Option<f32>,
    held: &HashMap<ChunkId, u64>,
) -> Vec<ChunkId> {
    let (x, z) = (origin[0], origin[1]);
    let mut cells: Vec<ChunkId> = match radius {
        Some(radius) => {
            // Enumerating the disc is cheap for normal radii but costs
            // O((radius / CHUNK_SIZE)^2) — 66k cells at MAX_WORLD_RADIUS.
            // Scanning the populated cells instead is bounded by world
            // content, so take whichever is cheaper. Both predicates are the
            // same footprint test, so the result is identical.
            let span = (radius.max(0.0) / CHUNK_SIZE).ceil() as usize;
            let disc_estimate = span
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_pow(2);
            if disc_estimate <= index.len() {
                chunks_within(x, z, radius)
                    .into_iter()
                    .filter(|id| index.has_content(*id))
                    .collect()
            } else {
                index
                    .cell_ids()
                    .into_iter()
                    .filter(|id| id.is_within(x, z, radius))
                    .collect()
            }
        }
        None => index.cell_ids(),
    };

    // Hysteresis: keep what the peer already holds a little longer, including
    // cells that just became empty (they still need a clearing message).
    let keep_radius = radius.map(|r| r + WORLD_UNLOAD_MARGIN);
    let mut seen: HashSet<ChunkId> = cells.iter().copied().collect();
    for id in held.keys() {
        let keep = match keep_radius {
            Some(keep) => id.is_within(x, z, keep),
            None => true,
        };
        if keep && seen.insert(*id) {
            cells.push(*id);
        }
    }

    sort_cells(&mut cells);
    cells
}

// ---------------------------------------------------------------------------
// Per-receiver player filtering
// ---------------------------------------------------------------------------

/// Online players bucketed by cell, built **once per tick** and reused for
/// every receiver. Filtering a receiver is then a handful of bucket lookups
/// plus exact distance tests, instead of a scan of every player per peer.
#[derive(Clone, Debug, Default)]
pub struct PlayerBuckets {
    /// Sorted by `sub`, so both the wire bytes and the output order are
    /// deterministic.
    players: Vec<PlayerState>,
    buckets: HashMap<ChunkId, Vec<usize>>,
}

impl PlayerBuckets {
    /// Build the per-tick index from the sim's online players.
    pub fn build(mut players: Vec<PlayerState>) -> Self {
        players.sort_by(|a, b| a.sub.cmp(&b.sub));
        let mut buckets: HashMap<ChunkId, Vec<usize>> = HashMap::new();
        for (i, p) in players.iter().enumerate() {
            buckets
                .entry(ChunkId::containing(p.x, p.z))
                .or_default()
                .push(i);
        }
        Self { players, buckets }
    }

    /// All players in the index, sorted by `sub`.
    pub fn players(&self) -> &[PlayerState] {
        &self.players
    }

    /// Players visible to one receiver, sorted by `sub`.
    ///
    /// Entry at `radius`; a player already visible stays until
    /// `radius + PLAYER_UNLOAD_MARGIN` (no avatar flicker at the edge); the
    /// receiver itself is always included; `None` means "everyone". Updates
    /// `visible` in place so the caller keeps the hysteresis across ticks.
    ///
    /// Per-receiver cost is `O(cells in the exit disc + k)` where `k` is the
    /// number of players actually inside it — not `O(P)` — so broadcasting to
    /// every peer is `O(peers * k)` instead of `O(peers * P)`.
    pub fn filter(
        &self,
        viewer: &str,
        origin: [f32; 2],
        radius: Option<f32>,
        visible: &mut HashSet<String>,
    ) -> Vec<PlayerState> {
        // `players` is sorted by `sub`: locate the receiver once instead of
        // scanning the list while visiting candidate cells.
        let viewer_idx = self
            .players
            .binary_search_by(|p| p.sub.as_str().cmp(viewer))
            .ok();

        let players: Vec<PlayerState> = match radius {
            // Full-world dev/bridge client: everyone, already sub-sorted.
            None => self.players.clone(),
            Some(radius) => {
                let radius = radius.max(0.0);
                let entry_sq = radius * radius;
                let exit_sq = (radius + PLAYER_UNLOAD_MARGIN) * (radius + PLAYER_UNLOAD_MARGIN);

                // Only the cells that can hold a player within the exit radius
                // are candidates (~9x9 at the default 96 m radius). Collect
                // indices, then sort/dedup so the clone below is sub-sorted
                // regardless of the cell visit order.
                let mut included: Vec<usize> = Vec::new();
                for cell in chunks_within(origin[0], origin[1], radius + PLAYER_UNLOAD_MARGIN) {
                    let Some(indices) = self.buckets.get(&cell) else {
                        continue;
                    };
                    for &i in indices {
                        if Some(i) == viewer_idx {
                            continue; // the receiver is added once, below
                        }
                        let p = &self.players[i];
                        let dx = p.x - origin[0];
                        let dz = p.z - origin[1];
                        let d2 = dx * dx + dz * dz;
                        if d2 <= entry_sq || (d2 <= exit_sq && visible.contains(&p.sub)) {
                            included.push(i);
                        }
                    }
                }
                // The receiver always sees itself, even when `origin` is not
                // its body (stale/teleported pose passed by a caller).
                if let Some(i) = viewer_idx {
                    included.push(i);
                }
                included.sort_unstable();
                included.dedup();

                included
                    .into_iter()
                    .map(|i| self.players[i].clone())
                    .collect()
            }
        };
        visible.clear();
        visible.extend(players.iter().map(|p| p.sub.clone()));
        players
    }

    /// One per-receiver [`Snapshot`]: the filtered player list plus the tick
    /// metadata. `visible` carries the receiver's previous visible set.
    #[allow(clippy::too_many_arguments)]
    pub fn snapshot(
        &self,
        tick: u32,
        server_ms: i64,
        dungeon_version: u64,
        viewer: &str,
        origin: [f32; 2],
        radius: Option<f32>,
        visible: &mut HashSet<String>,
    ) -> Snapshot {
        Snapshot {
            tick,
            server_ms,
            players: self.filter(viewer, origin, radius, visible),
            dungeon_version,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Sim;
    use crate::world::SimMoverSpec;
    use bevy_math::Vec2;
    use party_dungeon::platforms::{PlatformMode, PlatformWaypoint};
    use party_dungeon_net::{CHUNK_SEND_BUDGET, CHUNK_SIZE};

    fn row(rid: &str, shape: &str, x: f32, z: f32) -> (String, party_dungeon_server::Piece) {
        (
            rid.to_string(),
            party_dungeon_server::Piece {
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
                trigger_alpha: party_dungeon_server::TRIGGER_DEFAULT_ALPHA,
            },
        )
    }

    /// Hand-built sim (same pattern as `tests.rs`): convert DB-style rows and
    /// rebuild the world, optionally with mover paths.
    fn sim_with(
        rows: &[(String, party_dungeon_server::Piece)],
        paths: &[(&str, SimMoverSpec)],
    ) -> Sim {
        let mut sim = Sim::new();
        let pieces = sim.convert_pieces(rows);
        let specs: HashMap<String, SimMoverSpec> = paths
            .iter()
            .map(|(rid, spec)| (rid.to_string(), spec.clone()))
            .collect();
        sim.rebuild(&pieces, &specs);
        sim
    }

    fn sim_of(rows: &[(String, party_dungeon_server::Piece)]) -> Sim {
        sim_with(rows, &[])
    }

    fn cell(x: i32, z: i32) -> ChunkId {
        ChunkId { x, z }
    }

    /// World coordinate of a cell centre.
    fn center(axis: i32) -> f32 {
        axis as f32 * CHUNK_SIZE + CHUNK_SIZE * 0.5
    }

    fn mover_spec() -> SimMoverSpec {
        SimMoverSpec {
            waypoints: vec![
                PlatformWaypoint::new(Vec2::new(0.0, 0.0)),
                PlatformWaypoint::new(Vec2::new(4.0, 0.0)),
            ],
            closed: false,
            mode: PlatformMode::PingPong,
            speed: 1.0,
            paused: false,
            elevation: 0.0,
        }
    }

    fn piece_row(sub: &str, x: f32, z: f32) -> PlayerState {
        PlayerState {
            sub: sub.to_string(),
            name: sub.to_uppercase(),
            x,
            y: 0.0,
            z,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            yaw: 0.0,
            hp: 100.0,
            max_hp: 100.0,
            alive: true,
            online: true,
        }
    }

    fn subs(players: &[PlayerState]) -> Vec<&str> {
        players.iter().map(|p| p.sub.as_str()).collect()
    }

    // -- chunk index ------------------------------------------------------

    #[test]
    fn pieces_land_in_their_floor_cell_including_negatives() {
        let sim = sim_of(&[
            row("a", "floor", -0.5, -0.5),
            row("b", "floor", -33.0, 0.0),
            row("c", "floor", 0.0, 0.0),
            row("d", "floor", 31.9, 31.9),
            row("e", "floor", CHUNK_SIZE, CHUNK_SIZE),
        ]);
        let mut index = ChunkIndex::new();
        assert!(index.sync(&sim), "first sync builds");

        // floor(-0.5/32) = -1, floor(-33/32) = -2.
        assert_eq!(
            index.cell_ids(),
            vec![cell(-1, -1), cell(-2, 0), cell(0, 0), cell(1, 1)]
        );
        let rids = |c: ChunkId| -> Vec<String> {
            index
                .content(c)
                .unwrap()
                .pieces
                .iter()
                .map(|p| p.rid.clone())
                .collect()
        };
        assert_eq!(rids(cell(-1, -1)), vec!["a"]);
        assert_eq!(rids(cell(-2, 0)), vec!["b"]);
        // (0,0) and (31.9,31.9) share a cell; content is rid-sorted.
        assert_eq!(rids(cell(0, 0)), vec!["c", "d"]);
        assert_eq!(rids(cell(1, 1)), vec!["e"]);
        // The wire piece carries world Z in `z`, not `piece.pos.y` y-up.
        let p = &index.content(cell(1, 1)).unwrap().pieces[0];
        assert_eq!((p.x, p.z), (CHUNK_SIZE, CHUNK_SIZE));
    }

    #[test]
    fn a_glade_roof_pitch_is_streamed_from_the_sim_piece() {
        // A glade tower's roof pitch lives in `trigger_alpha` (alpha * 2) and
        // is gameplay: the streamed wire piece must carry the stored value, so
        // a freshly joined client renders the roof instead of the default.
        let mut tower = row("tower", "glade_tower", 16.0, 16.0);
        tower.1.trigger_alpha = 0.6;
        let sim = sim_of(&[tower]);
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        let piece = &index.content(cell(0, 0)).unwrap().pieces[0];
        assert_eq!(piece.shape, "glade_tower");
        assert!(
            (piece.trigger_alpha - 0.6).abs() < 1e-6,
            "streamed alpha = {}",
            piece.trigger_alpha
        );
    }

    #[test]
    fn mover_paths_follow_their_piece_cell() {
        let sim = sim_with(
            &[
                row("pad", "block", 40.0, 10.0),
                row("floor", "floor", -40.0, 10.0),
            ],
            &[("pad", mover_spec())],
        );
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        // floor(40/32) = 1, floor(10/32) = 0.
        let content = index.content(cell(1, 0)).expect("pad cell");
        assert_eq!(content.pieces.len(), 1);
        assert_eq!(content.pieces[0].rid, "pad");
        assert_eq!(content.paths.len(), 1);
        assert_eq!(content.paths[0].piece_rid, "pad");
        assert!(!content.paths[0].paused);
        // The other piece's cell has no path.
        assert!(index.content(cell(-2, 0)).unwrap().paths.is_empty());
    }

    #[test]
    fn versions_are_stable_and_only_changed_cells_bump() {
        let rows = [
            row("a", "floor", 16.0, 16.0),   // cell (0, 0)
            row("b", "floor", 96.0, 16.0),   // cell (3, 0)
            row("c", "floor", 208.0, 16.0),  // cell (6, 0)
        ];
        let mut sim = sim_of(&rows);
        let mut index = ChunkIndex::new();
        index.sync(&sim);

        let a = index.version(cell(0, 0)).unwrap();
        let b = index.version(cell(3, 0)).unwrap();
        let c = index.version(cell(6, 0)).unwrap();
        assert!(a != b && b != c, "one version per cell");

        // Same dungeon version: no rebuild at all.
        assert!(!index.sync(&sim));
        // Same content after a rebuild (the sim bumped its version): the
        // per-cell versions must stay put, so clients are not re-sent.
        let same = sim.pieces.clone();
        sim.rebuild(&same, &HashMap::new());
        assert!(index.sync(&sim));
        assert_eq!(index.version(cell(0, 0)), Some(a));
        assert_eq!(index.version(cell(3, 0)), Some(b));
        assert_eq!(index.version(cell(6, 0)), Some(c));

        // Move `b` inside its own cell (3, 0): only that cell bumps.
        let mut moved = sim.pieces.clone();
        for (rid, piece) in moved.iter_mut() {
            if rid == "b" {
                piece.pos = Vec2::new(100.0, 20.0);
            }
        }
        sim.rebuild(&moved, &HashMap::new());
        assert!(index.sync(&sim));
        assert_eq!(index.version(cell(0, 0)), Some(a), "untouched cell stable");
        assert_eq!(index.version(cell(6, 0)), Some(c), "untouched cell stable");
        assert!(
            index.version(cell(3, 0)).unwrap() > b,
            "changed cell gets a fresh version"
        );
    }

    #[test]
    fn versions_never_repeat_after_clear_and_repopulate() {
        let rows = [
            row("a", "floor", 16.0, 16.0),  // cell (0, 0)
            row("b", "floor", 96.0, 16.0),  // cell (3, 0)
        ];
        let mut sim = Sim::new();
        let original = sim.convert_pieces(&rows);
        sim.rebuild(&original, &HashMap::new());
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        let mut seen = vec![index.version(cell(3, 0)).unwrap()];

        // Delete the piece: tombstone version.
        let without: Vec<_> = original.iter().filter(|(r, _)| r != "b").cloned().collect();
        sim.rebuild(&without, &HashMap::new());
        index.sync(&sim);
        assert!(index.content(cell(3, 0)).is_none(), "cleared");
        seen.push(index.version(cell(3, 0)).expect("tombstone version"));

        // Repopulate it: a brand new version, strictly newer than the tombstone.
        sim.rebuild(&original, &HashMap::new());
        index.sync(&sim);
        assert!(index.content(cell(3, 0)).is_some(), "repopulated");
        seen.push(index.version(cell(3, 0)).unwrap());

        assert!(
            seen.windows(2).all(|w| w[1] > w[0]),
            "versions must never repeat, got {seen:?}"
        );
    }

    #[test]
    fn mover_pause_toggle_bumps_only_that_cell() {
        let mut sim = sim_with(
            &[
                row("pad", "block", 40.0, 10.0),    // cell (1, 0)
                row("flat", "floor", -40.0, 10.0),  // cell (-2, 0)
            ],
            &[("pad", mover_spec())],
        );
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        let pad = index.version(cell(1, 0)).unwrap();
        let flat = index.version(cell(-2, 0)).unwrap();
        let pad_pieces = index.content(cell(1, 0)).unwrap().pieces.clone();

        // An interact toggle flips `paused` and bumps the dungeon version
        // (exactly what `Sim::step` does); the pieces are unchanged.
        sim.world.movers.get_mut("pad").unwrap().paused = true;
        sim.dungeon_version += 1;
        assert!(index.sync(&sim));

        assert!(index.version(cell(1, 0)).unwrap() > pad, "mover cell bumped");
        assert_eq!(index.version(cell(-2, 0)), Some(flat), "other cell stable");
        let content = index.content(cell(1, 0)).unwrap();
        assert_eq!(content.pieces, pad_pieces, "geometry unchanged");
        assert!(content.paths[0].paused, "new paused flag is streamed");
    }

    // -- peer streaming ---------------------------------------------------

    /// Index with one floor piece at the centre of each cell in the grid.
    fn grid_index(axes: impl Iterator<Item = (i32, i32)>) -> ChunkIndex {
        let rows: Vec<_> = axes
            .map(|(x, z)| row(&format!("p-{x}-{z}"), "floor", center(x), center(z)))
            .collect();
        let sim = sim_of(&rows);
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        index
    }

    fn grid_25() -> ChunkIndex {
        grid_index((-2..=2).flat_map(|z| (-2..=2).map(move |x| (x, z))))
    }

    #[test]
    fn stream_sends_initial_cells_within_the_budget() {
        let index = grid_25();
        let mut view = PeerView::new(); // DEFAULT_WORLD_RADIUS = 128
        let first = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), CHUNK_SEND_BUDGET, "budget respected");
        assert!(first.unload.is_empty());
        assert_eq!(view.pending_len(), 25 - CHUNK_SEND_BUDGET);
        // The first batch is in (z, x) order.
        let order: Vec<(i32, i32)> = first.chunks.iter().map(|c| (c.id.z, c.id.x)).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "deterministic (z, x) order");

        let second = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert_eq!(second.chunks.len(), 25 - CHUNK_SEND_BUDGET);
        assert_eq!(view.pending_len(), 0);

        // Fully streamed: no work left.
        let third = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert!(third.is_empty(), "nothing to send once resident");
        assert_eq!(view.held_cells().len(), 25);
    }

    #[test]
    fn unload_keeps_held_cells_inside_the_margin() {
        // Content in four cells along +x, 32 m apart.
        let index = grid_index(
            [(0, 0), (1, 0), (2, 0), (6, 0)].into_iter(),
        );
        assert_eq!(index.cell_ids().len(), 4);

        let mut view = PeerView::new();
        view.set_interest(Some(0.0), Some(0.0)); // exactly the containing cell

        // Stand in cell (0, 0): only it is inside radius 0.
        let first = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), 1);
        assert_eq!(first.chunks[0].id, cell(0, 0));

        // One cell over (48, 16): cell (1, 0) enters, and the held cell
        // (0, 0) is still inside radius + margin, so it is NOT unloaded.
        let second = view.tick(&index, [center(1), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(second.chunks.len(), 1);
        assert_eq!(second.chunks[0].id, cell(1, 0));
        assert!(second.unload.is_empty(), "held cell inside the margin");

        // Two cells over: (0, 0) is now beyond radius + 32 m, so it unloads
        // while (1, 0) is kept.
        let third = view.tick(&index, [center(2), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(third.chunks.len(), 1);
        assert_eq!(third.chunks[0].id, cell(2, 0));
        assert_eq!(third.unload, vec![cell(0, 0)]);

        // Far away: every held cell is outside the margin, batched in (z, x).
        let fourth = view.tick(&index, [center(6), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(fourth.chunks.len(), 1);
        assert_eq!(fourth.chunks[0].id, cell(6, 0));
        assert_eq!(fourth.unload, vec![cell(1, 0), cell(2, 0)]);
        assert_eq!(view.held_cells(), vec![cell(6, 0)]);
    }

    #[test]
    fn held_cell_that_became_empty_is_cleared_then_forgotten() {
        let rows = [
            row("a", "floor", center(0), center(0)),
            row("b", "floor", center(3), center(0)),
        ];
        let mut sim = sim_of(&rows);
        let mut index = ChunkIndex::new();
        index.sync(&sim);

        let mut view = PeerView::new();
        view.set_interest(Some(0.0), Some(0.0));
        let first = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), 1);
        let before = index.version(cell(0, 0)).unwrap();
        assert_eq!(view.sent_version(cell(0, 0)), Some(before));

        // Delete everything in the peer's cell; the sim bumps its version.
        let without: Vec<_> = sim.pieces.iter().filter(|(r, _)| r != "a").cloned().collect();
        sim.rebuild(&without, &HashMap::new());
        assert!(index.sync(&sim));

        let clearing = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(clearing.chunks.len(), 1, "one clearing message");
        let chunk = &clearing.chunks[0];
        assert_eq!(chunk.id, cell(0, 0));
        assert!(chunk.is_empty());
        assert!(
            chunk.version > before,
            "clearing message carries the tombstone version"
        );
        assert!(clearing.unload.is_empty());
        assert_eq!(
            view.sent_version(cell(0, 0)),
            None,
            "empty cell is no longer held"
        );

        // Nothing left to say about that cell.
        let after = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert!(after.is_empty());
    }

    #[test]
    fn empty_dungeon_streams_nothing() {
        let sim = sim_of(&[]);
        let mut index = ChunkIndex::new();
        assert!(index.sync(&sim));
        assert!(index.is_empty());
        assert!(index.cell_ids().is_empty());
        let mut view = PeerView::new();
        view.set_interest(None, None);
        let update = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert!(update.is_empty(), "nothing to stream in an empty world");
        // A wild radius must not panic or enumerate a huge disc.
        view.set_interest(Some(f32::MAX), Some(f32::MAX));
        let update = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert!(update.is_empty());
    }

    #[test]
    fn none_radius_streams_every_cell() {
        let index = grid_index([(-9, -4), (0, 0), (5, 7)].into_iter());
        let mut view = PeerView::new();
        view.set_interest(None, None);
        let first = view.tick(&index, [0.0, 0.0], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), 3, "None = the whole world");
        assert!(first.unload.is_empty());
        // Even standing far away from everything, all cells stay resident.
        let second = view.tick(&index, [10_000.0, -10_000.0], CHUNK_SEND_BUDGET);
        assert!(second.is_empty());
        assert_eq!(view.held_cells().len(), 3);
    }

    #[test]
    fn client_interest_is_clamped_and_defaults_to_the_contract_radii() {
        let mut view = PeerView::new();
        assert_eq!(view.world_radius, Some(DEFAULT_WORLD_RADIUS));
        assert_eq!(view.player_radius, Some(DEFAULT_PLAYER_RADIUS));

        view.set_interest(Some(MAX_WORLD_RADIUS * 10.0), Some(-5.0));
        assert_eq!(view.world_radius, Some(MAX_WORLD_RADIUS));
        assert_eq!(view.player_radius, Some(0.0));

        view.set_interest(Some(f32::NAN), Some(f32::INFINITY));
        assert_eq!(view.world_radius, Some(MAX_WORLD_RADIUS));
        assert_eq!(view.player_radius, Some(MAX_PLAYER_RADIUS));

        view.set_interest(None, None);
        assert_eq!(view.world_radius, None);
        assert_eq!(view.player_radius, None);
    }

    #[test]
    fn restreams_only_the_cells_that_changed() {
        let rows = [
            row("a", "floor", center(0), center(0)),
            row("b", "floor", center(2), center(0)),
        ];
        let mut sim = sim_of(&rows);
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        let mut view = PeerView::new();
        view.set_interest(Some(64.0), Some(0.0));
        // Cells (0,0) and (2,0) are both inside 64 m of the origin.
        let first = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), 2);

        // Move `b` inside its own cell: only that cell is re-sent.
        let mut moved = sim.pieces.clone();
        for (rid, piece) in moved.iter_mut() {
            if rid == "b" {
                piece.pos = Vec2::new(center(2) + 3.0, center(0) - 2.0);
            }
        }
        sim.rebuild(&moved, &HashMap::new());
        assert!(index.sync(&sim));
        let update = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(update.chunks.len(), 1);
        assert_eq!(update.chunks[0].id, cell(2, 0));
        assert!(update.unload.is_empty());
    }

    #[test]
    fn interact_toggle_restreams_the_mover_cell() {
        let mut sim = sim_with(
            &[row("pad", "block", center(0), center(0))],
            &[("pad", mover_spec())],
        );
        let mut index = ChunkIndex::new();
        index.sync(&sim);
        let mut view = PeerView::new();
        let first = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(first.chunks.len(), 1);
        assert!(!first.chunks[0].paths[0].paused);

        sim.world.movers.get_mut("pad").unwrap().paused = true;
        sim.dungeon_version += 1;
        assert!(index.sync(&sim));
        let update = view.tick(&index, [center(0), center(0)], CHUNK_SEND_BUDGET);
        assert_eq!(update.chunks.len(), 1, "the paused mover is re-sent");
        assert!(update.chunks[0].paths[0].paused);
    }

    #[test]
    fn interest_cells_sorted_and_hysteretic() {
        let index = grid_index([(0, 0), (1, 0), (2, 0)].into_iter());
        let held: HashMap<ChunkId, u64> = [(cell(2, 0), 3u64)].into_iter().collect();
        // Radius 0 at the first cell: only (0,0) is eager; the held (2,0) is
        // 64 m away (beyond radius + margin = 32) so it is dropped.
        let cells = interest_cells(&index, [center(0), center(0)], Some(0.0), &held);
        assert_eq!(cells, vec![cell(0, 0)]);

        // Standing in (1,0): held (2,0) is one cell away (within 32 m).
        let cells = interest_cells(&index, [center(1), center(0)], Some(0.0), &held);
        assert_eq!(cells, vec![cell(1, 0), cell(2, 0)]);
    }

    // -- player filtering -------------------------------------------------

    #[test]
    fn player_filter_uses_entry_radius_and_exit_hysteresis() {
        let buckets = PlayerBuckets::build(vec![
            piece_row("viewer", 0.0, 0.0),
            piece_row("near", 5.0, 0.0),
            piece_row("edge", 20.0, 0.0),
            piece_row("far", 300.0, 0.0),
        ]);
        let mut visible = HashSet::new();

        // Entry radius 10 m: near is in, edge/far are out.
        let seen = buckets.filter("viewer", [0.0, 0.0], Some(10.0), &mut visible);
        assert_eq!(subs(&seen), vec!["near", "viewer"]);
        assert_eq!(
            visible,
            ["near".to_string(), "viewer".to_string()]
                .into_iter()
                .collect::<HashSet<String>>()
        );

        // Bring `edge` into view with a bigger radius first…
        let wide = buckets.filter("viewer", [0.0, 0.0], Some(30.0), &mut visible);
        assert_eq!(subs(&wide), vec!["edge", "near", "viewer"]);

        // …then shrink: edge (20 m) is past the 10 m entry radius but inside
        // the 10 + 32 m exit radius and was visible, so it lingers.
        let shrink = buckets.filter("viewer", [0.0, 0.0], Some(10.0), &mut visible);
        assert_eq!(subs(&shrink), vec!["edge", "near", "viewer"]);

        // A fresh receiver has no hysteresis: only the entry radius applies.
        let mut fresh = HashSet::new();
        let strict = buckets.filter("viewer", [0.0, 0.0], Some(10.0), &mut fresh);
        assert_eq!(subs(&strict), vec!["near", "viewer"]);

        // Far never appears, and the visible set is pruned to the snapshot.
        assert!(!strict.iter().any(|p| p.sub == "far"));
        assert_eq!(
            fresh,
            ["near".to_string(), "viewer".to_string()]
                .into_iter()
                .collect::<HashSet<String>>()
        );
        assert!(PLAYER_UNLOAD_MARGIN > 0.0);
    }

    #[test]
    fn player_filter_output_is_sub_sorted_across_cells() {
        // "zoe" sits in a cell visited before "amy"'s cell in `(z, x)` order,
        // so candidates are collected as zoe, amy — the output must still be
        // sub-sorted, and the far player must not be cloned in.
        let buckets = PlayerBuckets::build(vec![
            piece_row("viewer", 16.0, 16.0), // cell (0, 0)
            piece_row("zoe", -16.0, 16.0),   // cell (-1, 0), visited first
            piece_row("amy", 48.0, 16.0),    // cell (1, 0), visited last
            piece_row("far", 900.0, 900.0),
        ]);
        let mut visible = HashSet::new();
        let seen = buckets.filter("viewer", [16.0, 16.0], Some(64.0), &mut visible);
        assert_eq!(
            subs(&seen),
            vec!["amy", "viewer", "zoe"],
            "near players in, far player out, order still sub-sorted"
        );
        assert_eq!(visible.len(), 3);
        assert!(!visible.contains("far"));
    }

    #[test]
    fn player_filter_always_includes_the_receiver_itself() {
        let buckets = PlayerBuckets::build(vec![
            piece_row("viewer", 0.0, 0.0),
            piece_row("other", 500.0, 0.0),
        ]);
        let mut visible = HashSet::new();
        // Even with a zero radius and an origin far from the viewer's body.
        let seen = buckets.filter("viewer", [10_000.0, 10_000.0], Some(0.0), &mut visible);
        assert_eq!(subs(&seen), vec!["viewer"]);
    }

    #[test]
    fn player_filter_none_radius_includes_everyone() {
        let buckets = PlayerBuckets::build(vec![
            piece_row("a", 0.0, 0.0),
            piece_row("b", 5_000.0, -1_000.0),
            piece_row("c", -300.0, 42.0),
        ]);
        let mut visible = HashSet::new();
        let seen = buckets.filter("a", [0.0, 0.0], None, &mut visible);
        assert_eq!(subs(&seen), vec!["a", "b", "c"], "None = everyone, sorted");
        assert_eq!(visible.len(), 3);
    }

    #[test]
    fn filtered_snapshot_keeps_metadata_and_updates_visibility() {
        let buckets = PlayerBuckets::build(vec![
            piece_row("a", 0.0, 0.0),
            piece_row("b", 4.0, 0.0),
            piece_row("c", 400.0, 0.0),
        ]);
        let mut visible = HashSet::new();
        let snap = buckets.snapshot(7, 12_345, 9, "a", [0.0, 0.0], Some(8.0), &mut visible);
        assert_eq!(snap.tick, 7);
        assert_eq!(snap.server_ms, 12_345);
        assert_eq!(snap.dungeon_version, 9);
        assert_eq!(subs(&snap.players), vec!["a", "b"]);
        assert!(snap.players.iter().all(|p| p.online));
    }

    #[test]
    fn sim_snapshot_for_filters_like_the_buckets() {
        let mut sim = Sim::new();
        sim.join("a", "A", 100.0, 100.0);
        sim.join("b", "B", 100.0, 100.0);
        sim.join("c", "C", 100.0, 100.0);
        sim.players.get_mut("b").unwrap().pos = Vec2::new(5.0, 0.0);
        sim.players.get_mut("c").unwrap().pos = Vec2::new(300.0, 0.0);

        let mut visible = HashSet::new();
        let snap = sim.snapshot_for(1, "a", Some(16.0), &mut visible);
        assert_eq!(subs(&snap.players), vec!["a", "b"]);
        assert_eq!(snap.dungeon_version, sim.dungeon_version);

        // Shrinking past b's distance keeps it (previously visible).
        sim.players.get_mut("b").unwrap().pos = Vec2::new(20.0, 0.0);
        let snap = sim.snapshot_for(2, "a", Some(1.0), &mut visible);
        assert_eq!(subs(&snap.players), vec!["a", "b"]);

        // Fresh receiver: strict radius, self always there.
        let mut fresh = HashSet::new();
        let snap = sim.snapshot_for(3, "a", Some(1.0), &mut fresh);
        assert_eq!(subs(&snap.players), vec!["a"]);

        // The unfiltered snapshot is unchanged (all online players, sorted).
        let all = sim.snapshot(4);
        assert_eq!(subs(&all.players), vec!["a", "b", "c"]);
    }
}
