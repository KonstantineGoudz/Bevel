# 07 — Prioritized Improvement Roadmap

## P0 — Immediate Bug & Correctness Fixes

1. **Resolve Current WIP `vines` Compile Errors**
   Complete or gate the incomplete `vines` wiring across `session.rs`, `editor.rs`, `editor/setup.rs`, `editor/input.rs`, `editor/overlay.rs`, and `editor/touch.rs` so `cargo check --workspace` and `cargo test --workspace` pass again.
2. **Remove Duplicate System Registration**
   Delete the duplicate `sync_selected_glade_wall_mover.after(prune_movers)` registration in [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L143-L176).
3. **Fix `bridge_drain` / `bridge_publish` Schedule Order**
   Move `bridge_drain` before `rebuild_nav` and `sync_visuals`, while keeping `bridge_publish` after `sync_visuals` and `autosave_session` (matching the comment at [`editor.rs:284-285`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L284-L285)).
4. **Fix `bridge.rs` HTTP Reader & Concurrency**
   Wrap `TcpStream` in `BufReader` instead of 1-byte `read()` loops, and handle connections on short-lived worker threads or a bounded pool so a stalled `POST /command` cannot block `GET /health` or `GET /state`.

---

## P1 — Structural & Workspace Refactoring

1. **Extract `crates/core` (`party-dungeon-core`)**
   Move `model`, `path`, `platforms`, `statuses`, `locomotion`, and `Snapshot` serde types into `crates/core` with minimal dependencies (`bevy_math`, `bevy_color`, `serde`, `serde_json`). Point both `party-dungeon` and `party-dungeon-mcp` at `party-dungeon-core` to remove Bevy + Rapier from `mcp-server` and eliminate duplicated `sanitize_level_name` / `snapshot_from_level_json`.
2. **Split `Editor` Mega-Resource & Adopt Bevy `States` + `SystemSet`s**
   Break `Editor` into `LevelDocument`, `UndoHistory`, `ToolState`, `BrushState`, and `PlaySession`. Introduce `States` for `Mode` (`Build`, `Building`, `Play`) and `HomeMenu`, and organize `EditorPlugin` into semantic `SystemSet`s instead of 80-system `.chain()` tuples.
3. **Move Simulation Systems to `FixedUpdate`**
   Run `walk`, `move_movers`, `character_move`, `resolve_platform_push`, `resolve_dynamic_push`, `hazard_damage`, and `advance_cones` on `FixedUpdate` for frame-rate-independent physics and platform riding.
4. **Decompose `glade.rs` (7,485 lines) & `editor/input.rs::interact` (825 lines)**
   Split `glade.rs` into `glade/{mod,footprint,openings,building,terrain,trees,kit}.rs`. Split `interact()` into focused tool/input handlers and replace `use super::*;` in `editor/*` with explicit imports.

---

## P2 — Performance & Memory Improvements

1. **Eliminate $O(N^2)$ Loops in `sync_visuals`, `path::blocked`, and `session::validate_groups`**
   - In `sync_visuals`, build a `HashSet<u32>` of live piece IDs once and gate Glade signature checks on a `pieces_rev` / `Changed<Editor>` dirty flag.
   - In `path::blocked`, pre-index `pieces` by `Hex` cell in a `HashMap<Hex, ...>` before iterating.
   - In `validate_groups`, use a `HashSet<u32>` of valid piece IDs.
2. **Cache Roof `StandardMaterial`s & Reuse Meshes During Drag**
   Cache `StandardMaterial` handles by `roof_color` in `Kit`/`Visuals` instead of calling `materials.add(...)` on every Glade rebuild.
3. **Cache Hex Grid Gizmo Geometry in `editor/draw.rs`**
   Stop calling `world_terrain_height_at` 1,734 times and allocating 289 `Vec<Vec3>`s every frame; cache grid lines per `(cq, cr, terrain_rev)`.
4. **Chunked World Terrain Meshes & Colliders**
   Split the 280m × 280m world terrain into spatial chunks so `terrain_stroke` only rebuilds dirty chunks instead of regenerating the entire world trimesh.
5. **Diff-Based Undo/Redo & Lean Live Snapshots**
   Omit `undo`/`redo` history from `bridge_publish` JSON snapshots, and consider delta-based undo operations instead of cloning the full `pieces` + `grass` state on every stroke.

---

## P3 — Code Hygiene & Maintainability

1. **Consolidate Duplicated UI & Test Builders**
   Extract shared UI widget constructors from `setup.rs`, `minimap.rs`, and `properties.rs` into `editor/ui_widgets.rs`, and centralize piece-construction helpers (`block_piece`, `floor_piece`, `test_piece`).
2. **Replace `controls_codes.rs` Giant `match` Functions with a Table**
   Use a single `&[(&str, KeyCode)]` / macro table instead of two 190-arm `match` blocks (`cognitive = 391`).
3. **Split `session.rs` by Target**
   Separate pure snapshot serde from `storage_native.rs` and `storage_web.rs`, removing `#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]` from [`lib.rs`](file:///home/kon/dev/bevy/game/crates/game/src/lib.rs#L21).
