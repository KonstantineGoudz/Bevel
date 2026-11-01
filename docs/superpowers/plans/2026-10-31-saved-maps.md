# Saved Maps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the web editor named maps in local storage: an index page that creates, renames, deletes, and opens maps, and a game page that restores pieces, camera, character, and undo/redo history and autosaves as you edit.

**Architecture:** A new `session.rs` owns the JSON snapshot schema, validation, index metadata, and a small `Store` abstraction with a wasm `localStorage` implementation and an in-memory test double. `editor.rs` gains a redo stack and a capture/apply conversion between live state and `Snapshot`, plus an autosave system. `lib.rs` reads the session id and snapshot before starting Bevy. The two static pages get the picker UI and a preflight check.

**Tech Stack:** Rust 2024, Bevy 0.20, wasm-bindgen 0.2, web-sys, serde/serde_json, plain HTML/CSS/JS.

**Spec:** `docs/superpowers/specs/2026-10-31-saved-maps-design.md`

## Global Constraints

- Snapshot version is `1`.
- Keys are `party-dungeon.index.v1` and `party-dungeon.session.<id>.v1`.
- Snapshot JSON uses camelCase: `nextId`, `rotDeg`, `yawDeg`, `targetX/Y/Z`, `pieceCount`, `createdAt`, `updatedAt`.
- Undo and redo are capped at 60 snapshots each; drop the oldest first.
- Map names are trimmed, non-empty, and at most 60 characters; duplicate names are allowed.
- Autosave throttles camera/character writes to at most once per second; committed edits save immediately.
- Planned routes, waypoints, curve handles, selected tool, and build/play mode are never saved.
- Native desktop keeps `level.json`; no local-storage code runs there.
- The web build targets `wasm32-unknown-unknown` and is served from `game/web` on port 8082.
- Run Rust tests from `game/` with `cargo test -p party-dungeon`.

## Review Focus

1. Refreshing mid-walk or mid-camera-drag: the last pose and camera should return, not the state from before the movement. Pinned by Task 7's timing/pose tests and Task 10's walk-reload check.
2. Reloading and then undoing: ids that exist only inside restored history must not be reused by new placements. Pinned by Task 2's `next_id` test.
3. Storage full during autosave: editing must continue and the previous map must survive. Pinned by Task 3's quota test and Task 7's retry test.
4. A missing, corrupt, or unknown-version map: the game page must show an error with a way back, never an empty editor. Pinned by Task 2's validation tests and Task 10's missing-session check.
5. Unicode, 60-character, and duplicate names: the picker must render them literally and keep them distinct. Pinned by Task 9's picker checks.

---

### Task 1: Snapshot schema and JSON round-trip

**Files:**
- Create: `game/crates/game/src/session.rs`
- Modify: `game/crates/game/Cargo.toml`
- Modify: `game/crates/game/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `session.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Snapshot`, `SavedPiece`, `SavedCharacter`, `SavedCamera`, `LoadError`; `Snapshot::from_json(&str) -> Result<Snapshot, LoadError>`; `Snapshot::to_json(&self) -> String`.

- [ ] **Step 1: Add the module and dependencies**

Add `serde = { version = "1", features = ["derive"] }` and `serde_json = "1"` to `[dependencies]` in `game/crates/game/Cargo.toml`. Add `js-sys = "0.3"` and replace the wasm `web-sys` features with `["Document", "Element", "Window", "Storage", "Location", "DomException"]`. Add `mod session;` in `lib.rs`.

- [ ] **Step 2: Write the failing round-trip tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn piece() -> SavedPiece {
        SavedPiece { id: 3, shape: "block".into(), x: 1.5, z: -2.0,
            elevation: 0.5, rot_deg: 30.0 }
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let snapshot = Snapshot {
            version: 1,
            next_id: Some(9),
            pieces: vec![piece()],
            character: Some(SavedCharacter { x: 1.0, z: 2.0, feet: 0.25, yaw_deg: 90.0 }),
            camera: Some(SavedCamera { target_x: 0.0, target_y: 1.0, target_z: 2.0,
                yaw: 0.85, pitch: 0.9, distance: 7.5, mode: "third".into() }),
            undo: vec![vec![]],
            redo: vec![vec![piece()]],
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"rotDeg\":30.0"), "{json}");
        assert_eq!(Snapshot::from_json(&json).unwrap(), snapshot);
    }

    #[test]
    fn a_minimal_snapshot_reads_with_defaults() {
        let snapshot = Snapshot::from_json(r#"{"version":1}"#).unwrap();
        assert_eq!(snapshot.version, 1);
        assert!(snapshot.pieces.is_empty());
        assert!(snapshot.character.is_none());
        assert!(snapshot.camera.is_none());
        assert!(snapshot.undo.is_empty());
        assert!(snapshot.redo.is_empty());
        assert!(snapshot.next_id.is_none());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon session::tests`
Expected: FAIL — unresolved types and methods.

- [ ] **Step 4: Implement the schema**

Define these with `serde` derives. Every `Option`/collection field is `#[serde(default)]`; all structs use `#[serde(rename_all = "camelCase")]`.

- `Snapshot`: `version: u32`, `next_id: Option<u32>`, `pieces: Vec<SavedPiece>`, `character: Option<SavedCharacter>`, `camera: Option<SavedCamera>`, `undo: Vec<Vec<SavedPiece>>`, `redo: Vec<Vec<SavedPiece>>`. Derive `Default`.
- `SavedPiece`: `id: u32`, `shape: String`, `x: f32`, `z: f32`, `elevation: f32`, `rot_deg: f32`.
- `SavedCharacter`: `x`, `z`, `feet`, `yaw_deg` — all `f32`, `feet` defaulted.
- `SavedCamera`: `target_x/y/z`, `yaw`, `pitch`, `distance`, `mode: String`.
- `LoadError`: `MissingVersion`, `UnsupportedVersion(u32)`, `UnknownShape(String)`, `NonFinite(&'static str)`, `Parse(String)`, `NotFound`. Derive `Debug, Clone, PartialEq, Eq`.
- Constants: `SNAPSHOT_VERSION: u32 = 1`, `MAX_HISTORY: usize = 60`.

`from_json` maps `serde_json::Error` to `LoadError::Parse(error.to_string())`. `to_json` returns the serialized string.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p party-dungeon session::tests` — 2 passed.

- [ ] **Step 6: Commit**

```bash
git add game/Cargo.toml game/Cargo.lock game/crates/game/Cargo.toml game/crates/game/src/lib.rs game/crates/game/src/session.rs
git commit -m "feat: add session snapshot schema"
```

---

### Task 2: Validate snapshots and recover ids

**Files:**
- Modify: `game/crates/game/src/session.rs`
- Modify: `game/crates/game/src/editor.rs` (visibility and derives only)
- Test: `session.rs` tests

**Interfaces:**
- Consumes: Task 1.
- Produces: `Loaded`; `Snapshot::validate(self) -> Result<Loaded, LoadError>`; `ViewMode::parse(&str) -> Option<ViewMode>`; `ViewMode::id(self) -> &'static str`.

- [ ] **Step 1: Expose the live state types**

In `editor.rs`, make `Character` (`Clone, Copy, PartialEq, Debug`), `Orbit` (`Resource, Clone, PartialEq`), and `ViewMode` `pub(crate)`, with `pub(crate)` fields. Add `ViewMode::parse` accepting `"third" | "first" | "top"` and `ViewMode::id` returning the same strings.

- [ ] **Step 2: Write the failing validation tests**

```rust
fn saved(id: u32, shape: &str, x: f32, z: f32) -> SavedPiece {
    SavedPiece { id, shape: shape.into(), x, z, elevation: 0.0, rot_deg: 0.0 }
}

fn camera() -> SavedCamera {
    SavedCamera { target_x: 0.0, target_y: 0.0, target_z: 0.0,
        yaw: 0.85, pitch: 0.9, distance: 7.5, mode: "third".into() }
}

#[test]
fn a_minimal_snapshot_validates_to_an_empty_map() {
    let loaded = Snapshot { version: 1, ..Default::default() }.validate().unwrap();
    assert!(loaded.pieces.is_empty());
    assert!(loaded.character.is_none());
    assert!(loaded.camera.is_none());
    assert_eq!(loaded.next_id, 1);
}

#[test]
fn an_unknown_shape_refuses_the_map() {
    let snapshot = Snapshot { version: 1,
        pieces: vec![saved(1, "cloud", 0.0, 0.0)], ..Default::default() };
    assert_eq!(snapshot.validate().unwrap_err(), LoadError::UnknownShape("cloud".into()));
}

#[test]
fn a_non_finite_piece_refuses_the_map() {
    let mut piece = saved(1, "block", 0.0, 0.0);
    piece.x = f32::NAN;
    let snapshot = Snapshot { version: 1, pieces: vec![piece], ..Default::default() };
    assert_eq!(snapshot.validate().unwrap_err(), LoadError::NonFinite("piece x"));
}

#[test]
fn a_wrong_version_refuses_the_map() {
    let zero = Snapshot { version: 0, ..Default::default() };
    assert_eq!(zero.validate().unwrap_err(), LoadError::MissingVersion);
    let two = Snapshot { version: 2, ..Default::default() };
    assert_eq!(two.validate().unwrap_err(), LoadError::UnsupportedVersion(2));
}

#[test]
fn bad_camera_and_character_fall_back_to_defaults() {
    let snapshot = Snapshot { version: 1,
        camera: Some(SavedCamera { target_x: f32::NAN, ..camera() }),
        character: Some(SavedCharacter { x: f32::INFINITY, z: 0.0, feet: 0.0, yaw_deg: 0.0 }),
        ..Default::default() };
    let loaded = snapshot.validate().unwrap();
    assert!(loaded.camera.is_none());
    assert!(loaded.character.is_none());
}

#[test]
fn invalid_history_entries_are_dropped_individually() {
    let mut bad = saved(9, "block", 0.0, 0.0);
    bad.x = f32::NAN;
    let snapshot = Snapshot { version: 1,
        pieces: vec![saved(1, "block", 0.0, 0.0)],
        undo: vec![vec![saved(2, "block", 0.0, 0.0)], vec![bad]],
        redo: vec![vec![saved(3, "block", 0.0, 0.0)]],
        ..Default::default() };
    let loaded = snapshot.validate().unwrap();
    assert_eq!(loaded.undo.len(), 1);
    assert_eq!(loaded.undo[0][0].id, 2);
    assert_eq!(loaded.redo.len(), 1);
}

#[test]
fn next_id_covers_ids_that_only_exist_in_history() {
    let snapshot = Snapshot { version: 1, next_id: Some(5),
        pieces: vec![saved(2, "block", 0.0, 0.0)],
        undo: vec![vec![saved(7, "block", 0.0, 0.0)]],
        redo: vec![vec![saved(9, "block", 0.0, 0.0)]],
        ..Default::default() };
    assert_eq!(snapshot.validate().unwrap().next_id, 10);
}

#[test]
fn history_is_trimmed_to_the_newest_sixty() {
    let undo = (0..70).map(|i| vec![saved(i + 1, "block", 0.0, 0.0)]).collect();
    let loaded = Snapshot { version: 1, undo, ..Default::default() }.validate().unwrap();
    assert_eq!(loaded.undo.len(), 60);
    assert_eq!(loaded.undo[0][0].id, 11);
    assert_eq!(loaded.undo[59][0].id, 70);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon session::tests`
Expected: FAIL — missing `validate`, `Loaded`, `ViewMode::parse`.

- [ ] **Step 4: Implement validation**

Define `Loaded { pieces: Vec<Piece>, character: Option<Character>, camera: Option<Orbit>, undo: Vec<Vec<Piece>>, redo: Vec<Vec<Piece>>, next_id: u32 }` deriving `Debug, Clone, PartialEq`. `validate` consumes the snapshot and follows these rules:

1. Version `0` → `MissingVersion`; any version other than `SNAPSHOT_VERSION` → `UnsupportedVersion`.
2. Current pieces: resolve each shape with `model::shape_index`; unknown → `UnknownShape`. Reject non-finite `x`, `z`, `elevation`, or rotation with the field names used in the tests. Build `Piece { rot: rot_deg.to_radians(), .. }`.
3. Undo/redo: apply the same piece validation to each entry, drop entries that fail, then keep only the newest `MAX_HISTORY` entries of each.
4. Character: keep only if all four values are finite; reset `vy: 0.0`, `grounded: false`.
5. Camera: keep only if all six floats are finite, `distance > 0.0`, and `ViewMode::parse(mode)` succeeds.
6. `next_id = max(saved next_id or 1, highest piece id across current/undo/redo + 1, 1)`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p party-dungeon session::tests` — 10 passed.

- [ ] **Step 6: Commit**

```bash
git add game/crates/game/src/session.rs game/crates/game/src/editor.rs
git commit -m "feat: validate session snapshots"
```

---

### Task 3: Index metadata and quota-safe storage

**Files:**
- Modify: `game/crates/game/src/session.rs`
- Test: `session.rs` tests

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces: `Meta`, `Store`, `StorageError`, `LocalStore` (wasm only), `session_key`, `read_index`, `write_index`, `read_session`, `write_session`, `save_session`.

- [ ] **Step 1: Write the failing storage tests**

```rust
#[test]
fn a_session_key_is_prefixed_and_versioned() {
    assert_eq!(session_key("abc"), "party-dungeon.session.abc.v1");
}

#[test]
fn index_json_round_trips_and_skips_broken_entries() {
    let metas = vec![Meta { id: "a".into(), name: "Alpha".into(),
        created_at: 1.0, updated_at: 2.0, piece_count: 3 }];
    assert_eq!(parse_index(&encode_index(&metas)), metas);
    let mixed = r#"[
        {"id":"a","name":"Alpha","createdAt":1,"updatedAt":2,"pieceCount":3},
        {"name":"no id"}
    ]"#;
    assert_eq!(parse_index(mixed).len(), 1);
}

#[test]
fn saving_a_session_updates_its_index_entry() {
    let mut store = MemoryStore::new();
    let meta = Meta { id: "a".into(), name: "Alpha".into(),
        created_at: 1.0, updated_at: 2.0, piece_count: 0 };
    write_index(&mut store, &[meta]).unwrap();
    let snapshot = Snapshot { version: 1,
        pieces: vec![saved(1, "block", 0.0, 0.0), saved(2, "block", 1.0, 0.0)],
        ..Default::default() };
    save_session(&mut store, "a", &snapshot, 42.0).unwrap();
    let metas = read_index(&store);
    assert_eq!(metas[0].piece_count, 2);
    assert_eq!(metas[0].updated_at, 42.0);
    assert_eq!(read_session(&store, "a").unwrap(), snapshot);
}

#[test]
fn a_full_store_keeps_the_previous_snapshot() {
    let mut store = MemoryStore::with_quota(400);
    let old = Snapshot { version: 1,
        pieces: vec![saved(1, "block", 0.0, 0.0)], ..Default::default() };
    write_session(&mut store, "a", &old).unwrap();
    let big = Snapshot { version: 1,
        pieces: (0..20).map(|i| saved(i, "block", i as f32, 0.0)).collect(),
        ..Default::default() };
    assert_eq!(write_session(&mut store, "a", &big).unwrap_err(),
        StorageError::QuotaExceeded);
    assert_eq!(read_session(&store, "a").unwrap(), old);
}

#[test]
fn a_missing_session_is_not_found() {
    let store = MemoryStore::new();
    assert_eq!(read_session(&store, "nope").unwrap_err(), LoadError::NotFound);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon session::tests`
Expected: FAIL — missing `Meta`, `MemoryStore`, storage functions.

- [ ] **Step 3: Implement the store and index**

Add `INDEX_KEY = "party-dungeon.index.v1"`, `SESSION_PREFIX = "party-dungeon.session."`, and `SOFT_SAVE_INTERVAL: f64 = 1.0`. Define:

```rust
pub(crate) enum StorageError { QuotaExceeded, Unavailable }

pub(crate) trait Store {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), StorageError>;
    fn remove(&mut self, key: &str);
}

pub(crate) struct Meta { id: String, name: String, created_at: f64, updated_at: f64, piece_count: usize }

pub(crate) fn session_key(id: &str) -> String
pub(crate) fn encode_index(metas: &[Meta]) -> String
pub(crate) fn parse_index(text: &str) -> Vec<Meta>
pub(crate) fn read_index(store: &dyn Store) -> Vec<Meta>
pub(crate) fn write_index(store: &mut dyn Store, metas: &[Meta]) -> Result<(), StorageError>
pub(crate) fn read_session(store: &dyn Store, id: &str) -> Result<Snapshot, LoadError>
pub(crate) fn write_session(store: &mut dyn Store, id: &str, snapshot: &Snapshot) -> Result<(), StorageError>
pub(crate) fn save_session(store: &mut dyn Store, id: &str, snapshot: &Snapshot, now_ms: f64) -> Result<(), StorageError>
```

Rules: `StorageError` derives `Debug, Clone, Copy, PartialEq, Eq`; `Meta` derives `Debug, Clone, PartialEq, Serialize, Deserialize` with camelCase names; `parse_index` deserializes into `Vec<serde_json::Value>` and keeps entries that deserialize into `Meta`, so one bad entry does not discard the rest. `read_index` treats missing/invalid JSON as empty. `read_session` returns `NotFound` when the key is absent and parses the value otherwise. `save_session` writes the session first, then updates only the matching index entry's `updated_at` and `piece_count` and writes the index.

- [ ] **Step 4: Implement `LocalStore` and `MemoryStore`**

`LocalStore` (wasm only) wraps `window.local_storage()`. Missing storage → `Unavailable`. On `set_item` error, read the `DomException.name`: `QuotaExceededError` or `NS_ERROR_DOM_QUOTA_REACHED` → `QuotaExceeded`, anything else → `Unavailable`. `remove` ignores errors.

`MemoryStore` (tests only) holds a `HashMap<String, String>` and optional byte quota. `set` computes the total size with the key replaced in place (do not double-count an existing key) and returns `QuotaExceeded` without touching the map when the new total is over quota. This keeps the old value intact, which the quota test checks.

- [ ] **Step 5: Run the tests and check wasm**

Run: `cargo test -p party-dungeon session::tests` — 15 passed.
Run: `cargo check -p party-dungeon --target wasm32-unknown-unknown --profile wasm-release` — PASS.

- [ ] **Step 6: Commit**

```bash
git add game/crates/game/src/session.rs
git commit -m "feat: add quota-safe session storage"
```

---

### Task 4: Redo stack and dirty flags

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `editor.rs` tests

**Interfaces:**
- Consumes: `session::MAX_HISTORY`.
- Produces: `Editor.redo`, `Editor.hard_dirty`, `Editor.soft_dirty`, `Editor::redo()`.

- [ ] **Step 1: Write the failing tests**

```rust
fn test_piece(id: u32) -> Piece {
    Piece { id, shape: model::shape_index("block").unwrap(),
        pos: Vec2::ZERO, rot: 0.0, elevation: 0.0 }
}

#[test]
fn undo_then_redo_restores_the_newer_state() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.commit();
    editor.pieces.push(test_piece(2));
    editor.commit();
    editor.undo();
    assert_eq!(editor.pieces.len(), 1);
    assert_eq!(editor.redo.len(), 1);
    editor.redo();
    assert_eq!(editor.pieces.len(), 2);
    assert!(editor.redo.is_empty());
}

#[test]
fn a_new_edit_clears_redo() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.commit();
    editor.pieces.push(test_piece(2));
    editor.commit();
    editor.undo();
    assert_eq!(editor.redo.len(), 1);
    editor.pieces.push(test_piece(3));
    editor.commit();
    assert!(editor.redo.is_empty());
}

#[test]
fn both_history_stacks_are_capped_at_sixty() {
    let mut editor = Editor::default();
    for id in 1..=70 {
        editor.pieces.push(test_piece(id));
        editor.commit();
    }
    assert_eq!(editor.undo.len(), 60);
    assert_eq!(editor.undo[0].len(), 10);
    assert_eq!(editor.undo[59].len(), 69);
    for _ in 0..70 { editor.undo(); }
    assert_eq!(editor.redo.len(), 60);
}

#[test]
fn edits_and_history_moves_mark_the_session_dirty() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.commit();
    assert!(editor.hard_dirty);
    editor.hard_dirty = false;
    editor.undo();
    assert!(editor.hard_dirty);
    editor.hard_dirty = false;
    editor.redo();
    assert!(editor.hard_dirty);
}

#[test]
fn the_course_reset_clears_redo() {
    let mut editor = Editor::default();
    editor.redo.push(vec![test_piece(1)]);
    editor.load_course();
    assert!(editor.redo.is_empty());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon editor::tests`
Expected: FAIL — missing `redo`, `hard_dirty`, `soft_dirty`, `redo()`.

- [ ] **Step 3: Implement the stacks**

Add `use crate::session;` and the three fields to `Editor`/`Default`. Then:

- `commit`: push current pieces to undo, trim undo to `session::MAX_HISTORY`, clear redo, set `hard_dirty = true`.
- `undo`: pop undo, push current pieces to redo, trim redo, restore, clear selection, set `hard_dirty = true`.
- `redo`: pop redo, push current pieces to undo, trim undo, restore, clear selection, set `hard_dirty = true`.
- `load_course`: also clear redo and set `hard_dirty = true`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p party-dungeon editor::tests` — all editor tests pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: add redo stack and dirty tracking"
```

---

### Task 5: Capture and apply a session

**Files:**
- Modify: `game/crates/game/src/session.rs`
- Modify: `game/crates/game/src/editor.rs`
- Test: `editor.rs` tests

**Interfaces:**
- Consumes: Tasks 1–2, 4.
- Produces: `Editor.session: Option<String>`, `Editor::capture(&Orbit) -> Snapshot`, `Editor::apply_loaded(&mut self, &mut Orbit, Loaded)`; `From<&Piece> for SavedPiece`, `From<&Character> for SavedCharacter`, `From<&Orbit> for SavedCamera`.

- [ ] **Step 1: Write the failing capture/apply tests**

```rust
#[test]
fn a_session_captures_and_restores_pieces_camera_character_and_history() {
    let mut editor = Editor::default();
    editor.session = Some("map-1".into());
    editor.pieces = vec![test_piece(2), test_piece(4)];
    editor.character = Some(Character { pos: Vec2::new(1.5, -2.5), feet: 0.25,
        yaw: 0.5, vy: 3.0, grounded: false });
    editor.undo = vec![vec![test_piece(1)]];
    editor.redo = vec![vec![test_piece(5)]];
    editor.next_id = 6;
    let mut orbit = Orbit { target: Vec3::new(1.0, 2.0, 3.0), yaw: 0.4,
        pitch: 0.6, distance: 11.0, mode: ViewMode::Top };

    let snapshot = editor.capture(&orbit);
    assert_eq!(snapshot.next_id, Some(6));
    assert_eq!(snapshot.pieces.len(), 2);

    let mut restored = Editor::default();
    let mut restored_orbit = Orbit::default();
    restored.apply_loaded(&mut restored_orbit, snapshot.validate().unwrap());

    assert_eq!(restored.pieces, vec![test_piece(2), test_piece(4)]);
    assert_eq!(restored.next_id, 6);
    assert_eq!(restored.undo, vec![vec![test_piece(1)]]);
    assert_eq!(restored.redo, vec![vec![test_piece(5)]]);
    let character = restored.character.unwrap();
    assert_eq!(character.pos, Vec2::new(1.5, -2.5));
    assert_eq!(character.feet, 0.25);
    assert!((character.yaw - 0.5).abs() < 1e-5);
    assert_eq!(character.vy, 0.0);
    assert!(!character.grounded);
    assert_eq!(restored_orbit.target, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(restored_orbit.mode, ViewMode::Top);
    assert_eq!(restored_orbit.distance, 11.0);
}

#[test]
fn applying_a_snapshot_without_a_camera_keeps_the_default_camera() {
    let mut editor = Editor::default();
    let mut orbit = Orbit::default();
    let loaded = session::Snapshot { version: 1, ..Default::default() }.validate().unwrap();
    editor.apply_loaded(&mut orbit, loaded);
    assert_eq!(orbit.yaw, Orbit::default().yaw);
    assert_eq!(orbit.mode, ViewMode::Third);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon editor::tests`
Expected: FAIL — missing `capture` and `apply_loaded`.

- [ ] **Step 3: Implement conversions and state transfer**

In `session.rs`, implement `From<&Piece>`, `From<&Character>`, and `From<&Orbit>` using the saved fields; shape uses `shapes()[index].id`, rotation/yaw convert to degrees, camera mode uses `ViewMode::id`.

In `editor.rs`, `capture` fills every snapshot field from live state: pieces via `SavedPiece::from`, character via `SavedCharacter::from` (absent stays `None`), camera always present, undo/redo mapped snapshot-by-snapshot, and `next_id: Some(self.next_id)`.

`apply_loaded` assigns pieces, character, undo, redo, and `next_id`; replaces the orbit only when the loaded camera is `Some`; then clears `selected`, `ghost`, `drag`, `gizmo_drag`, `plan`, `route`, `walk`, `tool`, and `actor_tool`, and sets `note = "map loaded"`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p party-dungeon` — all tests pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/session.rs game/crates/game/src/editor.rs
git commit -m "feat: capture and restore editor state"
```

---

### Task 6: Boot from the URL and local storage

**Files:**
- Modify: `game/crates/game/src/session.rs`
- Modify: `game/crates/game/src/lib.rs`
- Modify: `game/crates/game/src/editor.rs`
- Modify: `game/web/game.html`
- Test: `session.rs` tests

**Interfaces:**
- Consumes: Tasks 2–3, 5.
- Produces: `Boot { id: Option<String>, loaded: Option<Loaded> }`, `boot() -> Result<Boot, String>`, `parse_session_id(&str) -> Option<String>`, `show_error(&str)` (wasm), `load_session` startup system.

- [ ] **Step 1: Write the failing query-string test**

```rust
#[test]
fn the_session_id_is_read_from_the_query_string() {
    assert_eq!(parse_session_id("?session=abc-123"), Some("abc-123".into()));
    assert_eq!(parse_session_id("?x=1&session=abc-123&y=2"), Some("abc-123".into()));
    assert_eq!(parse_session_id("?session="), None);
    assert_eq!(parse_session_id(""), None);
    assert_eq!(parse_session_id("?Session=abc"), None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p party-dungeon session::tests::the_session_id_is_read_from_the_query_string`
Expected: FAIL — missing `parse_session_id`.

- [ ] **Step 3: Implement boot**

`parse_session_id` strips a leading `?`, splits on `&`, finds the case-sensitive `session=` pair, and rejects empty values.

`Boot` derives `Debug, Default, Resource` and `Clone` if needed for the startup system.

`boot()` on wasm: read `window.location().search()`; missing id → error asking the user to choose a map. Read the index and require the id to be present; otherwise error that the map is not in the list. Read the snapshot and `validate()` it, mapping each `LoadError` to a readable sentence (parse, missing state, unsupported version, unknown shape, invalid geometry, missing version). Return `Boot { id, loaded }`. On native, return `Boot::default()`.

`show_error` (wasm): find `#error`, set its text content, set `style="display:block"`, and remove the `hidden` attribute from `#back`.

- [ ] **Step 4: Wire boot and startup restore**

In `lib.rs`, call `session::boot()` before `App::new()`. On `Err`, call `show_error` on wasm (log on native) and return without starting Bevy. Insert the successful `Boot` as a resource.

In `editor.rs`, register `.add_systems(Startup, (load_session, setup).chain())`. `load_session` takes `Option<Res<session::Boot>>`, `ResMut<Editor>`, `ResMut<Orbit>`; when both id and loaded are present it sets `editor.session = Some(id)` and calls `apply_loaded`.

- [ ] **Step 5: Add the page preflight**

In `game/web/game.html`, add `<a id="back" href="./" hidden>Back to maps</a>` after `#error`. Before `init()`, read the `session` query parameter. If it is absent, or `localStorage.getItem("party-dungeon.session." + id + ".v1")` is null, show `#error` with a readable message, unhide `#back`, and do not call `init()`.

- [ ] **Step 6: Run the tests and check wasm**

Run: `cargo test -p party-dungeon` — all pass.
Run: `cargo check -p party-dungeon --target wasm32-unknown-unknown --profile wasm-release` — PASS.

- [ ] **Step 7: Commit**

```bash
git add game/crates/game/src/session.rs game/crates/game/src/lib.rs game/crates/game/src/editor.rs game/web/game.html
git commit -m "feat: boot the editor from a saved session"
```

---

### Task 7: Autosave and manual save

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `editor.rs` tests

**Interfaces:**
- Consumes: Tasks 3, 5, 6.
- Produces: `Seen`, `LastSaved`, `SaveClock`, `autosave_session`, `save_due`, `soft_changed`, `persist`.

- [ ] **Step 1: Write the failing timing tests**

```rust
#[test]
fn hard_dirty_saves_immediately_and_soft_dirty_waits() {
    assert!(save_due(true, false, 10.0, 10.0, 0.0));
    assert!(!save_due(false, true, 10.0, 10.3, 0.0));
    assert!(save_due(false, true, 10.0, 11.0, 0.0));
    assert!(!save_due(false, true, 10.0, 20.0, 25.0));
}

#[test]
fn camera_and_character_changes_are_noticed() {
    let seen = Seen { orbit: Orbit::default(), character: None };
    assert!(!soft_changed(&seen, &Orbit::default(), None));
    let moved = Orbit { yaw: 1.0, ..Orbit::default() };
    assert!(soft_changed(&seen, &moved, None));
    let character = Character { pos: Vec2::ZERO, feet: 0.0, yaw: 0.0,
        vy: 0.0, grounded: true };
    assert!(soft_changed(&seen, &Orbit::default(), Some(character)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon editor::tests`
Expected: FAIL — missing `save_due`, `soft_changed`, `Seen`.

- [ ] **Step 3: Implement the state and timing helpers**

Add resources `LastSaved(Option<Seen>)` and `SaveClock { last_save: f64, retry_at: f64 }`, both `Resource, Default`. `Seen { orbit: Orbit, character: Option<Character> }` derives `Clone, PartialEq`.

- `save_due(hard, soft, last_save, now, retry_at) -> bool`: false before `retry_at`; otherwise `hard || (soft && now - last_save >= session::SOFT_SAVE_INTERVAL)`.
- `soft_changed(seen, orbit, character) -> bool`: compare all orbit fields and only the character's `pos`, `feet`, and `yaw`. Ignore `vy` and `grounded`, which change every physics frame and are not saved.

- [ ] **Step 4: Implement autosave**

`autosave_session(time, editor, orbit, last, clock, focus)`:

1. Return when `editor.session` is `None` (native).
2. Any unfocused `WindowFocused` message sets `hard_dirty`.
3. On the first run, store the current orbit/character in `LastSaved` and return.
4. If the orbit or saved character pose changed, set `soft_dirty`.
5. Return unless `save_due`.
6. Call `persist(&editor, &orbit)`. On success clear both dirty flags, set `clock.last_save = now`, and refresh `LastSaved`. On failure set `note` to `"save failed: storage is full"` or `"save failed: storage unavailable"` and set `clock.retry_at = now + 2.0`, keeping the dirty flags so the write retries.

Register both resources and add `autosave_session` last in the second chained `Update` tuple.

- [ ] **Step 5: Implement persistence and manual save**

`persist(&Editor, &Orbit) -> Result<(), session::StorageError>`: on wasm, require `editor.session`, build `editor.capture(orbit)`, and call `session::save_session(&mut LocalStore::new(), id, &snapshot, session::now_ms())`. On native, return `Ok(())`.

Change `save` to `fn save(editor: &mut Editor, orbit: &Orbit)`. Native keeps writing `level.json` and sets `"saved level.json"`. Wasm calls `persist` and sets `"saved"` or the same failure note. Clear both dirty flags on success. Update the `interact` call site to `save(&mut editor, &orbit)` and gate it with `!command`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p party-dungeon` — all pass.

- [ ] **Step 7: Check autosave manually**

Build and serve, then create a session in the browser console:

```js
const id = "manual-test";
localStorage.setItem("party-dungeon.index.v1", JSON.stringify([{id, name:"Manual", createdAt:Date.now(), updatedAt:Date.now(), pieceCount:0}]));
localStorage.setItem(`party-dungeon.session.${id}.v1`, JSON.stringify({version:1}));
```

Open `http://localhost:8082/game.html?session=manual-test`, place a block, move the camera, wait two seconds, and refresh. Expected: the block and camera return.

- [ ] **Step 8: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: autosave edits, camera, and character"
```

---

### Task 8: Redo controls

**Files:**
- Modify: `game/crates/game/src/editor.rs`
- Test: `editor.rs` tests

**Interfaces:**
- Consumes: Task 4.
- Produces: `TouchAction::Redo`, `redo_requested(&ButtonInput<KeyCode>) -> bool`.

- [ ] **Step 1: Write the failing shortcut tests**

```rust
#[test]
fn redo_shortcuts_need_command_shift_z_or_command_y() {
    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::KeyY);
    assert!(redo_requested(&keys));

    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::ShiftLeft);
    keys.press(KeyCode::KeyZ);
    assert!(redo_requested(&keys));

    let mut keys = ButtonInput::default();
    keys.press(KeyCode::ControlLeft);
    keys.press(KeyCode::KeyZ);
    assert!(!redo_requested(&keys));
}

#[test]
fn the_redo_touch_button_authors_the_level() {
    assert!(touch_is_authoring(TouchAction::Redo));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p party-dungeon editor::tests`
Expected: FAIL — missing `Redo` and `redo_requested`.

- [ ] **Step 3: Implement the shortcuts and button**

`redo_requested`: require `command_down`; return true for Cmd/Ctrl+Shift+Z or Cmd/Ctrl+Y.

In `interact`, when `!playing && command && just_pressed(KeyZ)`, undo unless Shift is held, in which case redo. Separately, redo on `redo_requested(&keys)` when KeyZ was not the trigger, so Ctrl+Shift+Z cannot fire twice.

Add `Redo` after `Undo` in `TouchAction`, add a Redo button after Undo in `spawn_touch_ui`, map it to `editor.redo()` in `run_touch_action`, and mention `Ctrl+Shift+Z` in the desktop hint. `touch_is_authoring` already treats it as authoring.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p party-dungeon` — all pass.

- [ ] **Step 5: Commit**

```bash
git add game/crates/game/src/editor.rs
git commit -m "feat: add redo controls"
```

---

### Task 9: Map picker on the index page

**Files:**
- Modify: `game/web/index.html`

**Interfaces:**
- Consumes: the storage schema from Tasks 1–3.
- Produces: a picker that reads and writes `party-dungeon.index.v1` and `party-dungeon.session.<id>.v1`.

- [ ] **Step 1: Replace the placeholder actions and add the list markup**

Keep the existing visual style. Replace the two placeholder buttons with one primary **New map** button. Add a **Your maps** section with `#map-list`, an empty state, a `<dialog id="name-dialog">` (input, inline error, Cancel/Confirm), and a `<dialog id="delete-dialog">` naming the map (Cancel/Delete).

- [ ] **Step 2: Add the picker script**

Implement in a module script:

- `readIndex()`: parse `party-dungeon.index.v1`; keep entries with a non-empty `id` and `name` and numeric timestamps; drop entries whose `party-dungeon.session.<id>.v1` key is missing (warn in the console); sort by `updatedAt` descending.
- `writeIndex(metas)`, `newId()` (`crypto.randomUUID()` with a timestamp-plus-random fallback).
- `createMap(name)`: write minimal `{version:1}` state, append metadata with `Date.now()` and `pieceCount: 0`, write the index, navigate to `game.html?session=<id>`. If the index write fails, remove the state key before reporting the error.
- `renameMap(id, name)`: replace only that entry's name; keep `updatedAt` so the row does not jump.
- `deleteMap(id)`: write the index without the entry first, then remove the state key.
- `render()`: build rows with DOM APIs and `textContent`; show name, formatted `updatedAt`, piece count, rename, and delete. Clicking the main row opens `game.html?session=<id>`. Use a link for the opening target and sibling buttons for the actions.
- `openNameDialog(mode, id)`: prefill `"Untitled map"` for create or the current name; trim; reject empty or over-60-character names inline.

Storage errors in create/rename/delete appear inline in the open dialog; the list must stay unchanged. Names are never inserted as HTML.

- [ ] **Step 3: Verify the picker manually**

Serve with `./scripts/serve-web.sh` and open `http://localhost:8082/`. Check: empty state; create "First map" and open it; the row returns with a piece count and last-edited time; rename keeps row order; delete confirms by name and removes only that map; a 60-character unicode name renders literally; two maps share a name and stay separate.

- [ ] **Step 4: Commit**

```bash
git add game/web/index.html
git commit -m "feat: add map picker to the index page"
```

---

### Task 10: Build and end-to-end verification

**Files:**
- Modify only if verification finds a defect.

- [ ] **Step 1: Run the full suite and build**

From `game/`: `cargo test -p party-dungeon` — all pass.
From `game/`: `./scripts/build-web.sh` — `Built web/pkg`, no errors.

- [ ] **Step 2: Serve and run the checklist**

Run `./scripts/serve-web.sh` and open `http://localhost:8082/`.

1. Create "Reload test" and place at least two blocks and the character.
2. Move the camera and walk for a couple of seconds, wait one second, refresh — pieces, camera, and character return.
3. Ctrl+Z after refresh undoes the last pre-refresh edit; Ctrl+Shift+Z redoes it.
4. Place a block after undoing — redo no longer restores the old edit.
5. Rename, return to the index, and confirm the new name and current piece count.
6. Give two maps the same name; both rows remain.
7. Delete one; the other still opens.
8. Open `game.html?session=missing` — error text and a **Back to maps** link, no blank editor.
9. Set the current session key to `not json` and reload — error page with a way back.
10. Fill local storage with junk, edit, and confirm the status reports a save failure while the editor keeps working; free space and confirm the next save succeeds.
11. On a narrow phone viewport, confirm the touch toolbar still fits and Undo and Redo are both tappable.

- [ ] **Step 3: Check native behaviour**

From `game/`: `cargo run -p party-dungeon` — the editor starts empty; `S` still writes `level.json`; no local-storage error appears.

- [ ] **Step 4: Commit any fixes**

```bash
git add game/crates/game/src game/web game/Cargo.toml game/Cargo.lock
git commit -m "fix: polish saved map restore"
```
