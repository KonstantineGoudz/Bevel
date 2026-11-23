# Interest Management & World Streaming

> **Legacy-path note.** This document describes the hand-written matchbox
> snapshot/chunk path (`crates/net` + `HostNet`/`NetClientPlugin`), which is the
> **default unbuilt** path and the rollback. The live deployment now uses
> `bevy_replicon` visibility-filtered replication instead — see
> [replicon-migration.md](replicon-migration.md). Everything here still matches
> the legacy build; the chunk/`SetInterest`/`WorldUnload` mechanics below do not
> run on the live replicon path (interest there is server-env
> `DUNGEON_WORLD_RADIUS`/`DUNGEON_PLAYER_RADIUS` with no per-client
> `SetInterest`).

How Party Dungeon sends a large shared world to each client without pushing
the whole thing to everybody. This is the design and operator reference for the
spatial streaming model in `game/crates/net/src/lib.rs`, the sim host
(`game/crates/sim/src/net.rs`) and the client chunk store
(`game/crates/game/src/editor/world_sync.rs`).

Status: **implemented** — the wire contract (`game/crates/net`) is fixed and
both the sim host (`game/crates/sim/src/{stream,net}.rs`) and the client
(`game/crates/game/src/{net_client.rs,editor/world_sync.rs}`) implement it. The
workspace is green: net 10, sim 38 and game 505 lib tests, `cargo check
--workspace` clean and the new streaming modules clippy-clean. Anything in this
document that names a constant, a message or a default is taken from the wire
crate and must match it; see §9 for where each piece lives.

---

## 1. The problem

The world used to be one message. `ServerMsg::World { dungeon_version, pieces,
paths }` carried **every** dungeon piece and mover track, and the sim sent it:

- once to every client on join (right after `Welcome`), and
- to **every** connected client on every `dungeon_version` bump —
  `maybe_broadcast_world` in `sim/src/net.rs` re-encoded the full world after any
  edit, interact-mover toggle or DB reload.

That is fine for a 200-piece demo level and hopeless for the world the MMO is
supposed to have:

- **Payload is O(total pieces).** A 100 000-piece world is roughly an 8 MB
  message. Every join floods it; every edit broadcasts it to every player. The
  egress of a single edit is `8 MB × online players`, and the client-side apply
  rebuilds its entire local world (clearing selection and resetting editor
  state) for a one-piece change.
- **Snapshots were global.** `Sim::snapshot()` serialized *every* online player
  into every receiver's `Snapshot`, so snapshot size was O(online players): 10 000
  players is ~800 KB per tick, i.e. tens of MB/s per client — a bandwidth wall at
  a few hundred players, long before the geometry wall.
- **Reliable channel pressure.** The whole world rode the reliable SCTP channel:
  a join into a vast world delayed every later reliable message (`Kill`,
  `DungeonChanged`, …) behind megabytes of geometry.

Nothing in the physics or the DB had to change for streaming: the sim already
owns one authoritative world and clients already rebuild geometry from a
serialized piece list. What changed is **which** pieces and players a given
client is told about, and the shape of the messages used to say it.

---

## 2. The model

Three independent relevance mechanisms, all receiver-relative:

1. **Spatial cells (chunks).** The XZ plane is partitioned into fixed
   **32 m × 32 m** cells. A piece belongs to the cell containing its **origin**
   (`x`, `z`); a mover track belongs to the cell of its piece. Each cell has an
   opaque version, bumped only when that cell's contents change and never reused
   for that cell (a cell that empties is tombstoned with a new version so a
   client holding it clears it). The sim streams a client only the cells around
   its player, and unloads cells that fall out of range.
2. **Per-tick chunk budget.** Each peer may receive at most `CHUNK_SEND_BUDGET`
   chunk messages per tick; the remainder **queues** for later ticks so a join
   into a vast world streams in over several ticks instead of one burst.
3. **Per-receiver player filtering.** `Snapshot.players` contains only the
   **online** players within the receiver's player radius (plus the receiver
   itself). There is no global roster broadcast. Entry/exit hysteresis prevents
   avatar flicker at the edge.

Radii use **hysteresis**: loading is eager (a cell/piece is admitted as soon as
it is inside the radius), unloading is lazy (it is kept until it is past
`radius + margin`). `WORLD_UNLOAD_MARGIN` and `PLAYER_UNLOAD_MARGIN` are both
32 m today — a full cell for chunks and a third of the player radius for
avatars. The effect is that a player pacing across a cell border does not
thrash the stream, and an avatar at the edge of visibility does not pop in and
out.

`None` radii mean **no limit**: stream every cell / include every player. That
exists only for dev clients that need the complete world locally — primarily a
native game with the MCP live bridge, whose `GET /state` must expose the whole
world to the `party-dungeon-mcp` server. WASM clients always stream; a native
client streams only when the bridge is disabled (see [§7](#7-operations--tuning)).

```
                       ┌────────────── sim (authoritative world) ──────────────┐
                       │  pieces/paths (SurrealDB) ──► per-cell index+version │
                       └───────┬───────────────────────────┬─────────────────┘
                               │                           │
             per peer: interest radii,         per tick: per peer
             resident cells, sent versions,   filter players by XZ radius,
             queue of pending chunks          self always included
             (<= CHUNK_SEND_BUDGET / tick)
                               │                           │
                               ▼                           ▼
                    ServerMsg::WorldChunk          Snapshot (unreliable,
                    ServerMsg::WorldUnload          60 Hz, per receiver)
                               │                           │
                               ▼                           ▼
              client chunk store (rid → stable        remote avatars
              local id) + incremental apply           (RemoteFeed)
```

---

## 3. Wire contract

All of this lives in `game/crates/net/src/lib.rs` (`party-dungeon-net`), which is
transport-free (postcard + serde) so the sim, the native client and the WASM
client agree exactly. Channel 0 is reliable/ordered and carries `ClientMsg` (to
the sim) and `ServerMsg` (from the sim); channel 1 is unreliable and carries
`Snapshot`.

### Messages

| Direction | Message | Payload | Purpose |
|---|---|---|---|
| client → sim | `ClientMsg::Hello { token, name }` | auth JWT + optional display name | join / authenticate (unchanged) |
| client → sim | `ClientMsg::Input(Input)` | one tick of intent (unchanged) | movement / action |
| client → sim | `ClientMsg::SetInterest { world_radius: Option<f32>, player_radius: Option<f32> }` | metres, `None` = no limit | how much of the world this client wants; sent after `Welcome` (and on every reconnect) and **retried each frame until the reliable channel accepts it**, so a full send buffer at join cannot silently leave the client on defaults. Absent = sim defaults. |
| client → sim | `ClientMsg::WorldEdit { ops: Vec<WorldOp> }` | ≤ `MAX_WORLD_OPS` ops: `Place { tag, piece }`, `Update { piece }`, `Delete { rid }` | an authored level edit. The sim validates + persists it (system of record), applies it and bumps `dungeon_version`, so only the changed cells re-stream; see “Authoring / edits”. |
| sim → client | `ServerMsg::Welcome { sub, tick, dungeon_version }` | identity, start tick, global version | answer to `Hello` (unchanged) |
| sim → client | `ServerMsg::WorldChunk(WorldChunk)` | `{ id: ChunkId, version: u64, pieces: Vec<WorldPiece>, paths: Vec<WorldPath> }` | one cell: initial stream, re-stream after an edit, or empty to clear a now-empty cell. **Replaces** whatever the client had for `chunk.id`. |
| sim → client | `ServerMsg::WorldUnload { cells: Vec<ChunkId> }` | cell ids | cells that left the streaming radius; drop their geometry (a fresh `WorldChunk` arrives if they come back). |
| sim → client | `ServerMsg::Kill { target, by }` | subs | kill feed (unchanged) |
| sim → client | `ServerMsg::DungeonChanged { dungeon_version }` | global version | coarse “geometry changed” notification (HUD/logging). Sent once per version bump, **before** that tick's changed cells on the same ordered channel; the cells arrive as `WorldChunk` messages. |
| sim → client | `ServerMsg::WorldEditApplied { tag, rid }` | client `Place` tag + store-generated rid | answer to a `WorldOp::Place`, sent **before** the streamed echo so the client can re-key its optimistic piece instead of duplicating it. |
| sim → client | `ServerMsg::WorldEditRejected { key, reason }` | op tag/rid + reason | an authored op was rejected (invalid values, unknown rid, store unavailable); the client rolls its optimistic copy back. |
| sim → client | `ServerMsg::Deny { reason }` | text | auth/full-server rejection (unchanged) |
| sim → client | `Snapshot` (channel 1) | `{ tick, server_ms, players: Vec<PlayerState>, dungeon_version }` | **per-receiver filtered**: only relevant players, always including the receiver, sorted by `sub`. |

`ServerMsg::World` and the client's `PendingWorld` staging are gone. A client
that never receives a `WorldChunk` for a cell renders an empty world there —
that is the intended state, not a failed sync. `WorldPiece` / `WorldPath` keep
mirroring the DB row fields the editor needs; only their envelope is new.
(`WorldPiece.trigger_alpha` was added when glade roof pitch became shared
gameplay — see §5 "Glade roof pitch".)

### Constants and defaults

| Constant | Value | Meaning |
|---|---|---|
| `CHUNK_SIZE` | `32.0` m | edge length of one cell |
| `DEFAULT_WORLD_RADIUS` | `128.0` m | default cell-streaming radius around a player (≈69 cells at a cell centre) |
| `WORLD_UNLOAD_MARGIN` | `32.0` m (= one cell) | extra distance a loaded cell is kept before `WorldUnload` (hysteresis) |
| `DEFAULT_PLAYER_RADIUS` | `96.0` m | default radius within which other players appear in a snapshot |
| `PLAYER_UNLOAD_MARGIN` | `32.0` m | extra distance an already-visible player is kept in the snapshot |
| `MAX_WORLD_RADIUS` | `4096.0` m | server-side clamp on a requested world radius |
| `MAX_PLAYER_RADIUS` | `1024.0` m | server-side clamp on a requested player radius |
| `CHUNK_SEND_BUDGET` | `16` messages / peer / tick | chunks sent to one peer per tick before the rest queue |
| `MAX_WORLD_OPS` | `64` ops / `ClientMsg::WorldEdit` | authored-edit batch bound; the sim rejects (not applies) an oversize batch |
| `chunk_axis(v)` | `floor(v / CHUNK_SIZE)` as `i32` | cell coordinate; **floor**, so negative coordinates land correctly; non-finite → cell 0 |
| `ChunkId` | `{ x: i32, z: i32 }` | one cell; `containing`, `min`, `center`, `distance_sq`, `is_within` helpers |
| `chunks_within(x, z, r)` | sorted by `(z, x)` | every cell whose **footprint intersects** the circle; both ends must agree on order |
| `sanitize_radius(Some(r), max)` | `r.clamp(0.0, max)`, NaN/∞ → `max` | client radius clamping; `None` stays `None` |

Rate context: `TICK_RATE = 60`, `TICK_DT = 1/60`. The chunk budget is per tick,
so a full radius-128 load (69 cells) takes `ceil(69 / 16) ≈ 5` ticks ≈ 83 ms.
`INTERP_DELAY_TICKS`, `SNAP_THRESHOLD`, `CORRECT_RATE`, `INTERACT_RANGE` and the
rest of the movement contract are unchanged by streaming.

---

## 4. Server flow (sim host)

Per peer, the host keeps: the verified `sub`, the requested interest (or the
defaults until `SetInterest` arrives), the set of **resident** cells, the
per-cell versions last sent to it, and a queue of chunks waiting for budget.
Every tick:

1. **Identity.** `Hello` → token verification → `Welcome` → create the peer's
   interest state at the default radii. The client's `SetInterest` normally
   arrives in the next reliable batch; until then the sim uses
   `DEFAULT_WORLD_RADIUS` / `DEFAULT_PLAYER_RADIUS`. A `SetInterest` from a peer
   that has not completed `Hello` is ignored.
2. **Refresh the target cell set** from the player's current position:
   `chunks_within(x, z, world_radius)` when limited (footprint-intersects, so a
   cell is admitted as soon as the player can approach it), or every non-empty
   cell when `world_radius` is `None`, **plus** the cells the peer already holds
   while they are still inside the `world_radius + WORLD_UNLOAD_MARGIN` circle
   (the hysteresis). Only cells that currently contain geometry are ever queued
   for streaming; an empty cell costs nothing.
3. **Exits.** A held cell that falls out of the desired set — i.e. it is fully
   outside the `world_radius + WORLD_UNLOAD_MARGIN` circle — is dropped from the
   peer's resident set and batched into a single `WorldUnload { cells }` for
   that tick. `None` interest never unloads. Cells that were queued but never
   sent are simply dropped from the queue (no unload needed).
4. **Budgeted sends.** The peer's queue is a FIFO of cell ids, deduplicated:
   pushing a cell that is already queued is a no-op, and the **version is read
   at send time**, so a cell that changes again while waiting goes out at its
   current revision rather than an outdated one. Every tick the sim pushes the
   desired cells whose version differs from what the peer holds, then drains at
   most `CHUNK_SEND_BUDGET` from the queue. Queued cells that left the interest
   set in the meantime are discarded when the set is refreshed. Because the
   reliable channel is ordered, a cell's unload can never overtake the chunk
   that replaced it; the batched `WorldUnload` is sent after that tick's chunks.
5. **Dirty cells.** Edits reach the sim by one of two paths: **MCP/DB edits**
   (the bridge reloads the world on its poll) and **in-game edits**
   (`ClientMsg::WorldEdit` → service-role reducers → apply to the sim, see
   "Authoring / edits" below). Either way the sim bumps the global
   `dungeon_version`, and the streaming index re-diffs cell content:
   **unchanged cells keep their version**, changed cells get a fresh one, and a
   cell whose contents became empty gets a *tombstone* version and is
   streamed as `WorldChunk::empty(id, version)` so peers holding it drop the
   visuals (the peer then stops holding it). Versions come from a monotonic
   counter and are never reused for a cell — not even after an editor clears and
   later repopulates it — which is what lets the client safely ignore
   `version <= current`. Each peer holding a changed cell is queued its new
   `WorldChunk`. The coarse `DungeonChanged` is broadcast once per version bump —
   before that tick's changed chunks, on the same ordered channel — for
   HUD/logging, and a mover pause toggle re-streams the piece's cell.
6. **Snapshots.** Online players are bucketed by cell **once per tick** and
   reused for every receiver (the same filter is exposed as `Sim::snapshot_for`
   for single-receiver/debug callers; `Sim::snapshot` remains the unfiltered
   test/admin view). For each connected peer, filter to those within its player
   radius (plus hysteresis: a player already visible is kept until they pass
   `player_radius + PLAYER_UNLOAD_MARGIN`), **always include the receiver**,
   sort by `sub`, and encode/send on the unreliable channel. With a limited
   radius the filter only enumerates the cells within `radius + margin` (a 9 × 9
   grid at the default 96 m) and clones just the players that pass the distance
   test; `None` clones the whole roster. Each peer's visible set is carried
   across ticks in its own streaming state, and the per-tick buckets are not
   built at all when no peer is connected. The filtering is per receiver, so
   snapshots are no longer a single global buffer; per-receiver work is
   O(cells in range + nearby players), not O(all online players).

Unload traffic is not subject to `CHUNK_SEND_BUDGET`: it is a list of cell ids,
not geometry, and holding geometry longer than necessary is worse than sending
the unload immediately. A peer's streaming state (radii, resident cells, queue,
visible players) is torn down with the peer connection.

### Authoring / edits

Edits use the same streaming path — they are never broadcast wholesale. The
client detects a user edit (a new local piece, changed gameplay fields on a
persisted piece, or a user deletion) and sends it as `ClientMsg::WorldEdit`
(`WorldOp::{Place, Update, Delete}`, at most `MAX_WORLD_OPS` = 64 ops per
message). The sim validates the ops against the store's value rules and the
shape catalogue, **persists them through the service-role reducers**
(`create_piece`/`update_piece`/`delete_piece` — the DB stays the system of
record), applies them to the authoritative world, and bumps `dungeon_version`
once per batch. Consequently only the **changed cells** re-stream (`WorldChunk`)
— an emptied cell is tombstoned and sent as an empty chunk so peers clear it —
and untouched cells cost nothing. `ServerMsg::WorldEditApplied { tag, rid }`
maps the client's optimistic `Place` tag to the store-generated rid, and
`WorldEditRejected { key, reason }` rolls the optimistic copy back. Until the
echo arrives the client keeps its **optimistic local copy** visible; pending
overrides win over streamed values until the store echoes a canonical change, so
an in-flight edit never flickers back to the old state. Deletes are optimistic
too: the piece is hidden locally until the echo no longer contains it, and
`WorldEditRejected` reinstalls it.

The MCP/DB edit path is different only in *how the edit reaches the sim*
(the sim polls the DB and rebuilds): it converges on the same world, and the
same cell-diffing stream carries the result to clients.

---

## 5. Client flow (Bevy)

`crates/game` keeps a **chunk store** (`WorldChunks` in `net_client.rs`) and
`editor/world_sync.rs` composes it into the editor's local world. The store is
updated per message; the composition runs only when the store is `dirty` (a
`Welcome`, a chunk or an unload) and installs the **union of the resident
cells** with stable ids. The important invariants:

- **Store.** `WorldChunks { chunks: HashMap<ChunkId, WorldChunk>, dirty, dungeon_version }`.
  `insert` keeps the newest revision (a duplicate or stale `version` is ignored:
  reliable delivery can still reorder a re-send around a newer edit) and sets
  `dirty`; `unload` drops cells (an unload for an unknown cell is a no-op); a
  `Welcome` starts a fresh session and `reset`s the store so the new stream is
  composed from scratch.
- **Composition.** `world_sync.rs` walks the resident cells in `(z, x)` order,
  pieces inside a cell in `rid` order, and assigns ids/validates exactly like a
  loaded level (`session::Snapshot::validate`). Deterministic order keeps ids
  stable across applies and makes streamed edits (and MCP comparisons) behave
  the same on every client that started from the same ids. On a validation
  error the batch is dropped (and logged) rather than installed partially. The
  composed list is installed into `Editor.pieces`; the *renderer's* work stays
  incremental because ids are stable.
- **Stable local ids.** The wire only carries DB row ids (`WorldPiece.rid`). The
  editor keys pieces by its own `u32` (`Piece.id`, `editor.next_id`), and the
  undo stack, selection, outliner and mover map all reference those ids. The
  client therefore keeps a `rid → local id` map across applies and reuses the
  local id when the same piece arrives again in a later `WorldChunk`. A re-sent
  cell must **not** renumber its pieces: otherwise a one-piece edit would clear
  the user's selection, orphan undos and re-key movers. Ids are only ever handed
  out from a monotonically growing counter, and never below the editor's own
  next id, so streamed and locally-created pieces cannot collide.
- **Cell replacement semantics.** A `WorldChunk` replaces the cell's contents
  wholesale: pieces present in the new chunk are updated/added, resident pieces
  of that cell that are absent from it disappear from the composition, and that
  cell's mover tracks are replaced. Mover tracks are keyed by `piece_rid`; a path
  whose piece is not resident has nothing to drive and is skipped.
- **Unload.** `WorldUnload` drops the cell from the store; the next composition
  no longer contains its pieces, so their visuals despawn and their
  `rid → id` entries are purged. `next_id` stays monotonic; a selected piece that
  unloads is removed from the selection (order and primary of the remaining ids
  are kept). If the player walks back, the cell arrives again as a fresh chunk
  and its pieces get fresh ids.
- **State preservation.** Installing a composition only rewrites the piece list
  and movers; it must not touch editor state the wire format does not carry:
  orbit camera, grass/vines, character, undo/redo history. Selection entries for
  pieces that unloaded are dropped (order and primary of the survivors kept);
  outliner labels/groups/locks are kept for ids that remain resident and pruned
  for ids that unloaded (an unloaded id never comes back). Note what is **not**
  preserved: `trigger_color` and `trigger_visible` are editor-only cosmetics on
  the `Piece` and are absent from `WorldPiece`, so every streamed apply rebuilds
  them from defaults — exactly like the old wholesale apply. (`trigger_alpha`
  *does* stream: it is gameplay for glade roof pitch and stays editor-local only
  for non-glade trigger opacity — see "Glade roof pitch" below.) The *gameplay*
  trigger data (damage, extents, interact action) and the glade roof pitch come
  from the wire. These are session-local cosmetics/editor history; the DB carries
  gameplay data only. (Contrast with the old wholesale `PendingWorld` apply,
  which rebuilt the world through `Editor::apply_loaded` and reset
  selection/plan/route; `apply_loaded` now survives only for tests/level
  seeding, not for the stream.)
- **Local edits.** A client's edit is detected locally (a diff against the last
  streamed store state), shown optimistically, and sent as
  `ClientMsg::WorldEdit` to the sim (see "Authoring / edits" in §4) — the sim
  persists it, bumps the version and streams the changed cells back. Because ids
  are stable and the echo is idempotent, the local editor does not see a
  self-inflicted rebuild; an in-flight optimistic override wins over the
  streamed value until the store echoes a canonical change. Remote
  edits to resident cells apply in place; edits to unloaded cells simply are not
  local — nothing fails, the change is visible when the cell streams in.
  The install marks the editor `hard_dirty`, so the MCP bridge republishes
  `/state` and a full-world dev client stays in sync after a streamed apply.
- **Remote players** are unchanged mechanically: `RemoteFeed` publishes the
  newest snapshot and `remote_players.rs` turns other players into avatars,
  interpolating `INTERP_DELAY_TICKS` behind. The difference is that the snapshot
  now only ever contains players near us, and **entry/exit is a radius
  transition**, not a join/leave.

### Glade roof pitch

Glade pieces have no separate roof field in the wire: their **roof pitch** is
derived from `Piece.trigger_alpha` (`glade::roof_pitch` =
`sanitize_trigger_alpha(alpha) * 2.0`), so the gameplay value travels in
`WorldPiece.trigger_alpha` (`0.0..=1.0`, validated by the store's
`Transform::SanitizeAlpha`, the generated client and the sim's `world_edit`
validation). The pitch applies to roof-bearing kinds (rooms/towers —
`glade::roof_pitch` returns `0.0` for wall, stairs, door, window, terrain and
tree). For non-glade shapes the same slot is only an editor-local trigger
opacity cosmetic and is not treated as a shared gameplay value.

The editor keeps a stable round-trip: `SavedPiece.trigger_alpha` stores the
**roof pitch** for glade shapes (matching `capture`/`SavedPiece::from_piece`),
and `session.rs`'s loader divides it by two into `Piece.trigger_alpha`
(sanitize range `0.08..=2.0` for the pitch). `editor/world_sync.rs` stores
`Some(roof_pitch)` for glade pieces and includes glade alpha in its gameplay
equality comparison, so moving the roof gizmo sends exactly one `Update` and the
streamed echo clears the optimistic override — no edit loop.

---

## 6. Worked bandwidth examples

Assumptions (all approximate, chosen so the arithmetic is checkable):

- World: **4 km × 4 km** = 125 × 125 = **15 625 cells** of 32 m.
- **100 000 pieces** → ~6.4 pieces per cell on average.
- **10 000 concurrent players** → 625 players/km².
- ~**80 bytes** per `WorldPiece` on the wire (`rid` string dominates: a
  `dungeon_piece:…` DB id, shape id, 8 floats, flags, trigger extents).
- ~**80 bytes** per `PlayerState` (`sub` + `name` strings dominate; 10 floats,
  2 bools).
- Player at a cell centre: `chunks_within(…, 128)` = **69 cells**; resident set
  can reach **101 cells** with the 32 m unload margin.

### Before (full-world push + global snapshots)

| Event | Bytes per client | Notes |
|---|---|---|
| Join | ~8 MB (`100 000 × 80 B`) | one reliable message, applied wholesale |
| Any edit / mover toggle | ~8 MB re-sent | × 10 000 clients ≈ **80 GB egress per edit** |
| Snapshot | 10 000 players × 80 B ≈ **800 KB/tick** | × 60 Hz ≈ **48 MB/s per client** |

The snapshot line is the fatal one: even one client cannot sustain 48 MB/s, and
the sim would have to encode and fan out ~800 KB × clients every 16 ms.

### After (cell streaming + per-receiver snapshot filter)

| Event | Bytes | Notes |
|---|---|---|
| Join: initial radius | 69 cells × 6.4 pieces × 80 B ≈ **35 KB** | streamed over `ceil(69/16) = 5` ticks (~83 ms) under `CHUNK_SEND_BUDGET` |
| Worst-case resident geometry | 101 cells ≈ **52 KB** | the steady-state local world of one player |
| One edited cell | 6.4 × 80 B ≈ **0.5 KB** | only to clients that have that cell; a dense cell (120 pieces) is ~10 KB — the wire test caps one chunk under 32 KB |
| Snapshot | (1 self + ~18 in radius) × 80 B + ~14 B ≈ **1.5 KB/tick** | ≈ **90 KB/s per client** at 60 Hz |

Geometry on a join went from ~8 MB to ~35 KB (**~230× less**), and per-edit
egress from 8 MB × players to ~0.5 KB × local players. Snapshot bandwidth per
client went from ~48 MB/s to ~90 KB/s at this density.

**But snapshot fan-out is still the dominant cost at scale.** Filtering shrinks
each snapshot; it does not merge them. Every peer still gets its own encoded
`Snapshot` every tick, so the sim's encoded volume and egress scale as
`players × visible players`, not `players`. The per-receiver *filter* is cheap
by comparison (only the cells within `player_radius` are enumerated, so it is
O(nearby), not O(all online players)); the bytes are the problem. At 1 000
players × ~18 visible that is ~90 MB/s aggregate; at 10 000 players ~900 MB/s,
with ~10.8 million player entries encoded per second across the server. That is
the next bottleneck, and why player-id interning, delta snapshots and snapshot
relaying are on the future-work list (§8). The player radius is the main tuning
knob: local player density, not world size, sets this cost.

---

## 7. Operations & tuning

The simulator and the game both need to know how much world a client wants.

| Env var | Where | Default | Effect |
|---|---|---|---|
| `DUNGEON_WORLD_RADIUS` | game client (native; WASM has no env and uses the default) | `DEFAULT_WORLD_RADIUS` (128 m) | world/cell radius the client puts in `SetInterest`; clamped by the sim to `MAX_WORLD_RADIUS`. The player radius has no env knob and stays at `DEFAULT_PLAYER_RADIUS` |
| `DUNGEON_FULL_WORLD=1` | game client | unset | request `None`/`None` interest (the whole world, every player); for local dev and tooling |
| `PARTY_DUNGEON_BRIDGE` | game client (native only) | **on** at `127.0.0.1:6559` | MCP live bridge; when it is on, the client requests full-world interest so `GET /state` exposes the complete world. Set `PARTY_DUNGEON_BRIDGE=0` to make a native client stream like a WASM one |

**Why the MCP bridge needs the whole world.** `party-dungeon-mcp` prefers the
live bridge (`GET /state` → `party_dungeon::session::Snapshot`) over SurrealDB,
and its mutating tools (`save_world`, `import_level_json`, piece/path/grass
edits) work on that snapshot and then `import_snapshot` it back. If the game is
only streaming a 128 m radius, `/state` would contain a *fraction* of the world
and a save would replace the real world with that fraction — a catastrophic
partial-world overwrite. Hence: bridge enabled → the native client sends
`SetInterest { world_radius: None, player_radius: None }` after `Welcome`;
`DUNGEON_FULL_WORLD=1` does the same for a native client without the bridge.
**Because the bridge defaults on for native, so does full-world interest** — a
native build only streams when `PARTY_DUNGEON_BRIDGE=0` (or the bridge address is
otherwise disabled). WASM has no bridge and always streams.

**A full-world client still streams.** `None` does not bypass the transport: it
means "every non-empty cell is desired", and those cells queue and drain at
`CHUNK_SEND_BUDGET` per tick. A truly vast world therefore takes a while to
become complete: the 15 625-cell example from §6 needs
`ceil(15 625 / 16) = 977` ticks ≈ **16 s**, and `GET /state` is a *partial* world
for that whole window — precisely when an MCP save would destructively write the
fragment. Two mitigations: point the MCP tools at the DB (`Source::Auto` falls
back to SurrealDB; `Source::Db` forces it) for very large worlds, or keep the
live-bridge dev world small. For comparison, a normal 69-cell gameplay radius
drains in five ticks.

**Why normal clients must not ask for `None`.** Nothing in the contract stops a
client from sending `None`; the sim obeys it for dev clients. In production that
is a bandwidth amplification vector (one peer can ask for the entire world at up
to 16 chunks/tick forever). Production deployments should either leave
`DUNGEON_FULL_WORLD` off, gate `None` in the sim behind a server-side
dev flag / loopback check, or clamp it to `MAX_WORLD_RADIUS` for untrusted
peers. See §8.

Other knobs and their effects:

- `DEFAULT_WORLD_RADIUS` (compile-time) sets the out-of-the-box stream radius and
  the sim's fallback before `SetInterest` arrives. Larger = more resident cells
  (memory, initial load ticks, per-edit fan-out) and fewer visible pop-ins.
- `DEFAULT_PLAYER_RADIUS` / `PLAYER_UNLOAD_MARGIN` trade avatar reach for
  snapshot cost. The cost is proportional to **local player density ×
  π·r²**, so the radius is the single most effective snapshot-bandwidth knob.
- `CHUNK_SEND_BUDGET` trades join latency against burst size: 69 cells at 16/tick
  is ~83 ms; lowering it smooths bursts but delays far geometry.
- `WORLD_UNLOAD_MARGIN` above one cell increases resident memory without
  reducing thrash further; below one cell a player pacing a border re-streams a
  cell every crossing.

Diagnostics: the sim logs each rebuild (`sim: dungeon changed -> version N`),
each interact toggle and each kill, so a chunk-version bump is observable from
the log. On the client, `RemoteFeed`/`remote_players` presence logs and the
network HUD are unchanged; there is no per-chunk log (chunks are ordinary
reliable traffic).

---

## 8. Limitations & future work

1. **Per-piece LOD / impostors.** Streaming decides *whether* a piece exists
   locally, not how much detail it costs. A cell at the edge of the radius is
   sent at full fidelity. Per-piece LOD (or billboard impostors beyond a shorter
   visual radius) would cut the resident geometry and allow a larger stream
   radius for the same bytes.
2. **Snapshot player-id interning.** Every snapshot repeats each visible
   player's full `sub` and `name` string. A 28-character Firebase `sub` plus a
   name is ~40% of an 80-byte `PlayerState`. Interning (session-scoped `u16`/`u32`
   ids with a reliable id↔sub mapping, or a roster add/remove message and
   snapshots keyed by id) would shrink snapshots proportionally and also make
   per-receiver encoding cheaper. This is the highest-leverage snapshot fix.
3. **Chunk delta compression.** One moved piece re-sends the whole cell (~0.5 KB
   typical, ~10 KB dense). Per-cell add/update/remove deltas keyed by cell
   version would make edits cheap in dense cells. Related: send the full cell
   only on first load, then operations.
4. **Pieces straddling cell borders.** Ownership is by **origin** only. A 20 m
   wall whose origin is in cell A but which extends deep into cell B:
   - loads with A, so its far end can appear before B is streamed (fine), but
   - is **not** included when B is loaded without A, and its extent is not
     considered when deciding whether A is still needed — a player standing in
     B near the border can see A's piece vanish when A unloads at
     `radius + margin`, and
   - the radius test ignores geometry extent: a large piece can be visible
     outside its cell's admitted footprint.
   Fix options: include the piece in every cell its AABB overlaps, or pad the
   load/unload radius by the largest piece half-extent. Today the model assumes
   pieces are much smaller than a cell.
5. **Huge-piece cells.** One enormous piece makes its cell an arbitrarily large
   message; the 32 m cell size assumes per-piece data stays smallish. A
   size-based split (or rejecting oversized single pieces) would protect the
   budget.
6. **f32 origin precision for enormous worlds.** `WorldPiece` positions are
   `f32`, and `chunk_axis` casts to `i32`: at ±10⁶ m the float resolution is
   already ~6 cm, and beyond ~±10⁷ m cell coordinates overflow `i32`. Truly vast
   worlds need origin rebasing (render/physics near the player at the origin)
   and/or `f64` world coordinates in the protocol. Not an issue at the current
   4 km scale.
7. **DB edit detection is coarse.** `sim/src/bridge.rs::poll_edits` polls every
   ~2 s and only rebuilds when the **piece count changes**; path changes are
   ignored (`let _ = paths`). Moves/rotations/damage/interact edits written
   directly to the DB (e.g. through the MCP server) keep the count and do **not**
   bump `dungeon_version`, so other clients do not receive the changed cell until
   some count-changing DB edit or a restart forces a rebuild. This limits the
   **MCP/DB** path only — in-game edits bypass it (`ClientMsg::WorldEdit` persists
   and applies in one step, §4). Fix for the DB path: a DB revision/`updated_at`
   query (or a SurrealDB live query/change feed) that detects any row change,
   then bumps the cells that changed.
8. **`None` interest is trusted.** As above: a client can request the whole world
   and the sim serves it. Needs a server-side dev gate for production.
9. **Pop-in at the radius edge.** Hysteresis removes flicker but not the
   transition: a cell/avatar appears at the boundary at full detail, and at the
   current margins an avatar can become visible at 96 m then persist to 128 m.
   Fades/LOD would hide the seam.
10. **Per-receiver snapshot encode cost.** Filtering means the sim builds and
    encodes one `Snapshot` per peer per tick. With a limited radius the work is
    O(cells in `player_radius` + nearby players) per receiver — not O(all online
    players) — but the encoded output is still `players × visible players` per
    tick in aggregate, which is the scaling wall alongside egress. Batch
    encoding, interning (§8.2) or a snapshot relay are the mitigations. (`None`
    interest is the exception: it clones the whole roster for that peer, which
    is why it is dev-only.)
11. **Visibility is 2D distance only.** Players within the radius are visible
    through walls (no occlusion/LOS), same as the old global roster within the
    radius. Acceptable for a party crawler; worth revisiting if PvP stealth ever
    matters.
12. **Editor free-cam and remote tooling.** Interest follows the **character**,
    not the camera. An editor flying the free camera around sees cells stream
    relative to the character, not the view — which is exactly why a native
    dev/bridge client takes the full-world path instead of trying to game the
    camera.
13. **Undo/redo across stream boundaries.** Undo entries capture local piece
    ids. A cell that unloads takes its pieces (and their ids) out of the editor's
    resident world, and a reloaded cell may allocate fresh ids for the same DB
    rows. Undoing an edit that touched a since-unloaded piece therefore cannot
    restore it locally — the sim/DB remains the truth and the cell re-streams on
    return. Another player's edit is never in your undo history at all. This is
    accepted: interest management makes the local editor a *view*, not the
    authority.
14. **Full-world dev clients converge over time.** A `None`-interest client is
    not sent the world inline; its cells still drain through
    `CHUNK_SEND_BUDGET`, so a vast world leaves `/state` partial for seconds
    (see §7) and a live-bridge MCP save in that window would write a fragment.
    For very large worlds, prefer the DB source for MCP edits, or keep the dev
    world small enough to drain almost immediately. A larger budget shortens the
    window at the cost of bigger join bursts.
15. **Only some trigger fields are editor-local.** `trigger_color` and
    `trigger_visible` are editor-only `Piece` fields with no `WorldPiece`
    equivalent, so they reset on every streamed apply (see §5).
    `trigger_alpha` **is** streamed — it is gameplay for glade pieces (roof
    pitch = `alpha * 2`) and an editor-local opacity cosmetic for every other
    shape (see §5 "Glade roof pitch"), so a non-glade opacity tweak simply does
    not reach other clients. The other gameplay fields — trigger damage,
    extent, alpha-independent shape, interact action, `paused` mover state, and
    the glade roof pitch — do stream. If the remaining cosmetics ever need to
    be shared, they belong in the DB schema, not the wire alone.
16. **Every version bump re-indexes the whole world.** `ChunkIndex::rebuild`
    re-partitions the *entire* piece list and content-diffs every cell on each
    `dungeon_version` bump (edit, hot reload, interact/mover toggle) — O(total
    pieces) per bump even when one cell changed. Fine at today's scale (edits are
    infrequent), but a vast world with a mover toggled every frame would pay for
    it every time. Future optimization: carry a per-cell dirty set out of the
    sim/DB rebuild and re-index only those cells. The wire model already allows
    it — cell versions are independent, so peers only hear about the cells that
    actually changed.
17. **No protocol-version handshake.** `Hello`/`Welcome`/`Deny` carry no
    protocol/version field. An in-flight deploy (sim updated ahead of a cached
    WASM client, or vice versa) can leave the old client unable to decode the
    streamed messages — `ServerMsg::World` no longer exists, so a stale client
    sees no geometry at all — until it refreshes, with no diagnostic beyond
    decode failures. A version in `Hello`/`Welcome` (and a `Deny` on mismatch)
    is the obvious future fix; today, bump the client cache/refresh on deploy.

---

## 9. Where the code lives

| Concern | File |
|---|---|
| wire contract: cells, constants, `WorldChunk`, `SetInterest`, filtered `Snapshot` | `game/crates/net/src/lib.rs` |
| sim interest semantics: `ChunkIndex` (cell content + versions), `PeerView` (queue/budget/unload), `PlayerBuckets` (filtered snapshots) | `game/crates/sim/src/stream.rs` |
| sim host: peer interest state, budgeted chunk sends, unloads, per-receiver snapshots | `game/crates/sim/src/net.rs` |
| sim loop and edit polling | `game/crates/sim/src/main.rs`, `game/crates/sim/src/bridge.rs`, `game/crates/sim/src/sim.rs` |
| client chunk store + incremental apply + authoritative edit diff/optimistic merge | `game/crates/game/src/editor/world_sync.rs` |
| client net: `SetInterest` on `Welcome`, chunk/unload receive, `WorldEdit` send queue, `RemoteFeed` | `game/crates/game/src/net_client.rs` |
| authoritative edit validation + reducer translation | `game/crates/sim/src/world_edit.rs`, `game/crates/sim/src/net.rs` |
| remote avatars (snapshot-driven; UI name labels, `Welcome` identity gate) | `game/crates/game/src/remote_players.rs` |
| MCP full-world consumer (`GET /state`) | `game/crates/game/src/editor/bridge.rs`, `game/crates/mcp-server/src/store.rs` |
