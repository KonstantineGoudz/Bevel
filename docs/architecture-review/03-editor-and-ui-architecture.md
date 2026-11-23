# 03 — Editor & UI Architecture

## 1. Flat `use super::*;` Coupling Across 32 Submodules

[`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L1283-L1309) declares 32 submodules (`add_menu`, `bridge`, `camera`, `character`, `colliders`, `draw`, `editing`, `gizmo`, `grass`, `hazards`, `home`, `hud`, `input`, `interacts`, `minimap`, `movers`, `outline`, `outliner`, `overlay`, `picking`, `properties`, `right_panel`, `route`, `save`, `selection`, `setup`, `shooting`, `touch`, `transform`, `vines`, `visuals`, `walking`).

Nearly all of them start with `use super::*;` and share `pub(super)` visibility into `editor.rs`.
- **Consequence**: There are no enforced module boundaries. Any UI file (`properties.rs`, `home.rs`, `touch.rs`) can directly mutate any field of `Editor`, `Movers`, `Orbit`, or `Visuals`.
- **Improvement**: Group submodules into clear domains (`editor/ui/`, `editor/tools/`, `editor/sim/`, `editor/scene/`) with explicit imports instead of `use super::*;`.

---

## 2. The `Editor` Mega-Resource

[`Editor`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L307-L380) is a single Bevy `Resource` holding 35+ unrelated fields:
- **Document data**: `pieces`, `outline`, `grass`, `next_id`, `undo`, `redo`, `hard_dirty`, `soft_dirty`
- **File/session metadata**: `session`, `level_name`, `level_path`
- **Tool & interaction state**: `tool`, `actor_tool`, `mode`, `selection`, `clipboard`, `snap`, `ghost_rot`, `ghost`, `drag`, `gizmo_drag`, `keyboard_transform`, `last_palette`, `note`
- **Brush stroke state**: `grass_slows`, `grass_erase`, `grass_brush_radius`, `grass_brush_strength`, `grass_rev`, `grass_stroke`, `grass_stroke_dirty`, `grass_dab`, `terrain_stroke`, `terrain_stroke_dirty`, `terrain_stroke_target_h`, `terrain_stroke_last_pos`
- **Runtime play/navigation state**: `character`, `plan`, `route`, `walk`, `seen_nav`, `platform_request`

### Why This Hurts
Because almost every system takes `Res<Editor>` or `ResMut<Editor>`, Bevy cannot run editor systems in parallel, and change detection (`editor.is_changed()`) triggers falsely whenever *any* unrelated field (like `editor.note` or `editor.ghost`) is touched.
- **Improvement**: Split `Editor` into focused resources:
  - `LevelDocument` (`pieces`, `outline`, `grass`, `next_id`, `dirty` flags)
  - `UndoHistory` (`undo`, `redo`)
  - `ToolState` (`mode`, `tool`, `actor_tool`, `selection`, `clipboard`, `snap`, `ghost`, `drag`, `gizmo_drag`)
  - `BrushState` (grass & terrain stroke fields)
  - `PlaySession` (`character`, `plan`, `route`, `walk`, `seen_nav`)

---

## 3. God Functions & High-Arity Systems

Graph analysis from `codebase-memory-mcp` highlights severe complexity concentration in the editor:

| Function | File | Lines | Params | Cognitive Complexity |
| --- | --- | --- | --- | --- |
| `interact` | [`editor/input.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/input.rs#L383) | 825 | 15 | 344 |
| `sync_visuals` | [`editor/visuals.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/visuals.rs#L136) | 322 | 15 | 180 |
| `dragged_piece` | [`editor/gizmo.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/gizmo.rs) | — | 4 | 172 |
| `on_palette_activate` | [`editor/input.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/input.rs#L10) | 230 | 13 | 150 |
| `camera_control` | [`editor/camera.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/camera.rs) | 237 | 15 | 92 |
| `sync_properties_panel` | [`editor/properties.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/properties.rs) | 211 | 15 | — |

### Specific Issues in `editor/input.rs`
- `interact()` handles keyboard transforms, mode toggling, copy/paste, deletion, tool placement, terrain sculpting, grass painting, gizmo dragging, piece dragging, and route clicking all in one 825-line function.
- `#[allow(clippy::too_many_arguments)]` is used repeatedly across `input.rs`, `bridge.rs`, `properties.rs`, and `viewer.rs` instead of grouping parameters with Bevy `SystemParam` structs (though `MenuHome` and `PlayerInput` show good use of `SystemParam` in a few places).
- **Improvement**: Decompose `interact()` into mode/tool-specific handlers or Bevy `Observer`/state-gated systems (e.g., `handle_shortcuts`, `handle_terrain_brush`, `handle_grass_brush`, `handle_gizmo_drag`, `handle_piece_drag`).

---

## 4. UI & Input Code Duplication

1. **Duplicated UI Widget Builders**
   `codebase-memory-mcp` `SIMILAR_TO` analysis shows 1.000 similarity between:
   - `editor/minimap.rs::spawn_minimap_button` and `editor/setup.rs::{touch_button, palette_compact_button, palette_button}`
   - `editor/properties.rs::section_label` and `editor/setup.rs::palette_label`
   - **Improvement**: Extract a small `editor/ui_widgets.rs` module for styled buttons, section headers, and toggle pills.

2. **Duplicated Piece-Construction Helpers**
   `SIMILAR_TO` also flags 1.000 similarity across `interacts.rs::block_piece`, `hud.rs::spawn_piece`, `movers.rs::block_piece`, `hazards.rs::floor_piece`, `picking.rs::piece`, and `tests/mod.rs::test_piece`. These should share a single `Piece::new(...)` builder in `model.rs` (or `test_piece` in a test utility).

3. **Generated Input Tables (`controls_codes.rs`)**
   [`controls_codes.rs`](file:///home/kon/dev/bevy/game/crates/game/src/controls_codes.rs) uses two 190-arm `match` functions (`keycode_from_name` and `keycode_name`, cognitive complexity 391 each). A static `&[(&str, KeyCode)]` lookup table or macro would cut the code in half and make both directions impossible to drift out of sync.
