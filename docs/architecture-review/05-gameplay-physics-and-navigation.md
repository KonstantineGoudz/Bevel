# 05 — Gameplay, Physics & Navigation

## 1. Strengths

- **Clean Pure-Logic Separation**:
  - [`locomotion.rs`](file:///home/kon/dev/bevy/game/crates/game/src/locomotion.rs) (417 lines) is a textbook pure state machine (`LocomotionState`, `LocomotionInput`, `LocomotionMachine`) with clear priority rules (`Dead` > airborne > `Sliding` > grounded) and zero ECS coupling.
  - [`statuses.rs`](file:///home/kon/dev/bevy/game/crates/game/src/statuses.rs) isolates health and status math cleanly.
  - [`editor/walking.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/walking.rs) extracts pure kinematics helpers (`jump_velocity`, `ground_step`, `air_step`, `slide_step`) for unit testing.
- **glTF Avatar Decoupling**: [`editor/character.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/character.rs) keeps the physics capsule as the canonical root entity and attaches the skinned glTF avatar as a follower, matching clips by name (`idle_loop`, `walk_loop`, `sprint_loop`, etc.) so asset re-exports don't break indices.

---

## 2. Architectural Errors & Bottlenecks

### A. Physics & Gameplay Systems Run in `Update` Instead of `FixedUpdate`
In [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L147-L203), all character movement, platform movement, projectile advancement, and collision-push systems (`walk`, `move_movers`, `character_move`, `hazard_damage`, `resolve_platform_push`, `resolve_dynamic_push`, `advance_cones`) run in **`Update`** using variable `Time::delta_secs()`.
Meanwhile, `RapierPhysicsPlugin` steps physics on Rapier's simulation schedule.
- **Consequence**:
  - Variable frame rates alter jump arcs, slide friction decay, and platform-riding synchronization.
  - Kinematic/dynamic shove systems (`resolve_platform_push`, `resolve_dynamic_push`) can desync from Rapier's fixed substeps on frame spikes or low-FPS WebGL/mobile devices.
- **Improvement**: Move simulation systems (`walk`, `move_movers`, `character_move`, `resolve_platform_push`, `resolve_dynamic_push`, `hazard_damage`, `advance_cones`) into `FixedUpdate` (or Rapier's `PhysicsSet` / `FixedPostUpdate` ordering) with `Time<Fixed>`.

### B. $O(N^2)$ Navigation Grid Rebuild in `path::blocked`
[`path.rs:196`](file:///home/kon/dev/bevy/game/crates/game/src/path.rs#L196) (`blocked(pieces: &[Piece])`) has `transitive_loop_depth = 5` and `linear_scan_in_loop = 1`:
- For every piece in `pieces`, it calls `piece_cell(piece, shape, pieces)` and (for wedges/blocks) `block_at(..., pieces)` / `entries_onto(..., pieces)`, each of which linearly scans `pieces`.
- `find_path_cost` ([`path.rs:284`](file:///home/kon/dev/bevy/game/crates/game/src/path.rs#L284)) also shows `linear_scan_in_loop = 2` and `transitive_loop_depth = 3`.
- **Improvement**: Pre-index `pieces` by `Hex` cell (`HashMap<Hex, SmallVec<[usize; 4]>>`) once at the top of `blocked()` and `Nav` rebuilds, turning $O(N^2)$ into $O(N)$.

### C. Collider Rebuilding & `colliders.rs` Complexity
[`editor/colliders.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/colliders.rs) (`piece_collider_for_glade`, cognitive complexity 113) decomposes Glade walls into compound cuboids with archway cutouts, while [`editor/visuals.rs:673`](file:///home/kon/dev/bevy/game/crates/game/src/editor/visuals.rs#L673) rebuilds a full `Collider::trimesh` for the entire world terrain (`world_terrain_elevated_collider`) whenever terrain changes.
- **Risk**: Rebuilding a 280m × 280m world trimesh collider synchronously on the main thread during a continuous terrain brush stroke (`terrain_stroke`) can cause frame hitches, especially on WASM.
- **Improvement**: Chunk the world terrain into spatial tiles (e.g., 16m × 16m or 32m × 32m) so terrain sculpting only rebuilds the meshes and `Collider::heightfield` / `Collider::trimesh` for dirty chunks.

### D. `Editor` Mode State Leakage
`walk`, `character_move`, `hazard_damage`, `handle_interact`, and `respawn_on_death` run even in `Mode::Build` / `Mode::Building` unless they manually guard internally, and `Character` state is mutated in-place on `Editor` rather than cleanly resetting or snapshotting play-test state when toggling between `Build` and `Play`.
- **Improvement**: Use Bevy `States` (`EditorModeState::Build`, `EditorModeState::Building`, `EditorModeState::Play`) so gameplay systems only execute in `Play` (or when `walk.is_some()` for route preview).
