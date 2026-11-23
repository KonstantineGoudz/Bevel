# 02 — Data Model & Persistence Architecture

## 1. Core Data Model (`model.rs`)

[`model.rs`](file:///home/kon/dev/bevy/game/crates/game/src/model.rs) (1,937 lines) defines the hex grid math, shape catalogue (`shapes()`), `Piece`, `Geometry`, `Style`, `Trigger`, and `InteractAction`.

### Strengths
- **Unit-testable geometry**: Hex math (`hex_center`, `pixel_to_hex`, `nearest_edge`) and snapping are pure functions with zero Bevy ECS dependencies.
- **Stable shape IDs**: `Shape::is_glade()` checks `self.id.starts_with("glade_")`, decoupling display names/categories from procedural IDs.

### Architectural Issues
1. **Monolithic `Piece` Struct ("Kitchen Sink" Data Model)**
   `Piece` is `Copy` and carries every optional feature field directly on every placed object: trigger damage/shape/size/color/alpha/visible, interaction action, dynamic flag, Glade dimensions (`width`, `length`, `height`, `roof_pitch`, `roof_style`, `roof_color`), etc.
   - As new features are added (e.g., vines, new triggers, new Glade types), `Piece`, `SavedPiece`, `properties.rs`, and `bridge.rs` all grow together.
   - **Improvement**: Split optional feature payloads or use typed component/extension enums (e.g., `PieceKind::Block`, `PieceKind::Glade(GladeParams)`, `PieceKind::Trigger(TriggerParams)`) rather than flattening every domain's fields onto a single struct.

2. **Index-Based `piece.shape: usize` Coupling**
   Pieces reference shapes by their index in `shapes()` (`Vec<Shape>` / slice) at runtime, while `SavedPiece` serializes `shape: String` (the shape `id`). Any mismatch or out-of-bounds index requires `.get(piece.shape)` checks scattered across `visuals.rs`, `colliders.rs`, `path.rs`, and `draw.rs`.
   - **Improvement**: Use a typed `ShapeId(&'static str)` or newtype index `ShapeIndex(u16)` with a central lookup helper so shape resolution cannot silently drift.

---

## 2. Persistence & Session Snapshots (`session.rs` & `save.rs`)

[`session.rs`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs) is 2,330 lines (78 KB) and mixes three distinct responsibilities:
1. **Canonical `Snapshot` serde schema** (`Snapshot`, `SavedPiece`, `SavedCharacter`, `SavedCamera`, `SavedGroup`, `SavedPlatform`, `SavedGrass`).
2. **Native filesystem I/O & CLI arg parsing** (`list_native_levels`, `level_file_from_env_or_args`).
3. **WASM `localStorage` / browser session persistence** (using `web-sys`).

### Architectural Issues
1. **Cross-Target Dead Code Suppression**
   In [`lib.rs`](file:///home/kon/dev/bevy/game/crates/game/src/lib.rs#L21-L22):
   ```rust
   #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
   pub mod session;
   ```
   Suppressing `dead_code` across the entire 2,330-line `session` module on native builds hides genuinely dead functions and mixes browser-specific storage code with the canonical level format.
   - **Improvement**: Split `session.rs` into `snapshot.rs` (pure serde schema + validation, shared with `mcp-server`), `storage_native.rs`, and `storage_web.rs` behind a small `LevelStorage` trait or `#[cfg]` module boundary.

2. **Full-Level Deep Cloning for Undo/Redo**
   [`LevelState`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L386-L390) clones the entire `pieces: Vec<Piece>`, `outline: SceneOutline`, and `grass: HashMap<Hex, u8>` on every edit, storing up to `MAX_HISTORY` (60) steps in `Editor.undo` / `Editor.redo` **and** serializing full `undo` and `redo` arrays inside [`Snapshot`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs#L60-L63).
   - **Impact**: Large levels with painted grass or terrain strokes multiply memory and JSON snapshot size by up to 120× (60 undo + 60 redo), slowing down `autosave_session` and `bridge_publish` (`GET /state`).
   - **Improvement**:
     - Exclude `undo`/`redo` from `bridge_publish` live snapshots (or make history an optional editor-only sidecar, not part of the canonical level file).
     - Move from full-state snapshots to command/diff-based undo entries (e.g., `UndoOp::MovePieces`, `UndoOp::PaintGrassDelta`, `UndoOp::EditPiece`).

3. **Duplicated Snapshot & Validation Logic in `mcp-server`**
   `codebase-memory-mcp` similarity analysis flagged exact/near-exact duplication between `session.rs` and `mcp-server/src/store.rs`:
   - `sanitize_level_name`: 1.000 Jaccard similarity
   - `snapshot_from_level_json`: 0.984 Jaccard similarity
   Both should live once in the shared core crate.

4. **Linear Scans in `validate_groups`**
   [`session.rs:1284`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs#L1284) (`validate_groups`) performs linear scans inside a loop (`linear_scan_in_loop = 2`). Indexing valid piece IDs into a `HashSet<u32>` once before validating group memberships eliminates the quadratic scan.
