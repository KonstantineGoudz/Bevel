# AGENTS.md

Shared context and conventions for all AI agents working in **Party Dungeon** (`/home/kon/dev/bevy`).

## Project Overview

**Party Dungeon** is a top-down 3D multiplayer party crawler and level editor built with **Bevy `0.20.0`** and **`bevy_rapier3d` `0.37`** (Rust edition `2024`, Cargo resolver `3`).

## Repository Layout

- `game/` — Rust Cargo workspace (`members = ["crates/game", "crates/mcp-server"]`):
  - `game/crates/game/` (`party-dungeon`, `lib` + `cdylib` + bin):
    - `src/lib.rs` — Entry point (`#[bevy_main] pub fn main()`). Boots either the full editor/game (with `RapierPhysicsPlugin`) or the lightweight glTF `viewer::ViewerPlugin` (no physics or editor state).
    - `src/editor.rs` & `src/editor/` — In-game level editor (camera, selection, gizmos, transform, picking, outliner, properties, grass, hazards, movers, route, save, touch, and `bridge.rs` loopback HTTP bridge).
    - `src/model.rs`, `src/platforms.rs`, `src/statuses.rs` — Core level data model, platform paths, and status effects.
    - `src/locomotion.rs`, `src/path.rs`, `src/path_edit.rs` — Character locomotion state machine and route/pathfinding.
    - `src/glade.rs`, `src/mesh.rs`, `src/gltf_export.rs` — Procedural glade/mesh generation and glTF export.
    - `src/controls.rs`, `src/controls_codes.rs`, `src/session.rs`, `src/viewer.rs` — Input bindings, boot/session config, and standalone model viewer.
    - `tests/` — Integration tests (`character_model.rs`, `gltf_viewer.rs`).
  - `game/crates/mcp-server/` (`party-dungeon-mcp` bin):
    - Stdio MCP server exposing 23 level-editing/player-control tools and 4 `level://` resources, backed by snapshot JSON files and/or the live game bridge.
  - `game/assets/` — Runtime assets (`character.glb`, `glade_pieces_kit.glb`).
  - `game/levels/` — Level snapshot JSON files (e.g., `demo.json`).
  - `game/web/` — Browser/WASM pages (`index.html`, `game.html`, `viewer.html`, `viewer-bevy.html`). Note: `game/web/pkg/` and `game/web/assets/` are generated build outputs (gitignored).
  - `game/art/` — Blender 5.2+ character pipeline (`export_combined.py` merges `teal_vanguard.glb` + `quinn_anim.glb` into `assets/character.glb`).
  - `game/scripts/` — `build-web.sh`, `serve-web.sh`, `serve-web.py`.
- `notes/` — Astro (`^7.4.2`) design-notes site (`notes/src/`, `notes/schema/0001_dungeon.sql`).
- `docs/` — Design specs (`docs/superpowers/specs/`), implementation plans (`docs/superpowers/plans/`), and animation notes (`docs/animations/unreal.md`).
- `dsh-party-dungeon-mcp/` — `@local/party-dungeon-mcp` bundle metadata (`cordis.patch.yml`, `locale/en.json`).
- `.cargo-home/` — Workspace-local Cargo home (gitignored; registry mirror for sandboxed builds).

## Common Commands

### Rust / Game / MCP Server (run from `game/`)

```sh
cargo check --workspace
cargo test --workspace
cargo run -p party-dungeon -- --level ./levels/demo.json
cargo run -p party-dungeon-mcp
```

### Web / WASM (run from `game/`)

```sh
./scripts/build-web.sh            # fast iteration (wasm-dev profile, no LTO)
./scripts/build-web.sh --release  # optimized release (wasm-release profile, LTO)
./scripts/serve-web.sh            # serves game/web on http://localhost:8082 (no-cache)
```

### Design Notes Site (run from `notes/`)

```sh
npm run dev
npm run build
```

## Key Conventions & Gotchas

- **Do not hand-edit `game/web/assets/` or `game/web/pkg/`**: `./scripts/build-web.sh` replaces `web/assets/` from `game/assets/` and regenerates `web/assets/models.json` for `viewer-bevy.html`.
- **Cargo profiles**: `dev` and `wasm-dev` use `opt-level = 1` so Bevy runs at interactive framerates without slow full-LTO builds; use `release` / `wasm-release` only for final builds.
- **glTF textures**: `bevy` in `game/crates/game/Cargo.toml` explicitly enables the `jpeg` feature because Bevy 0.20 omits it by default and glTF allows JPEG textures.
- **Level files & live bridge**: The canonical level format is Snapshot JSON (`{"version":1,...}`). On native builds, `editor/bridge.rs` binds a loopback HTTP bridge (default `127.0.0.1:3559`). Relevant env vars: `PARTY_DUNGEON_LEVEL`, `PARTY_DUNGEON_LEVELS_DIR`, `PARTY_DUNGEON_BRIDGE`.
- **Character animation names**: `export_combined.py` preserves clip names (`idle_loop_fit`, `walk_loop_fit`, `sprint_loop_fit`, `jump_start_fit`, etc.) required by the game's name-based animation lookup.
- **Building (Glade) pieces, modes & transform gizmos**:
  - `Shape::is_glade()` checks `self.id.starts_with("glade_")` so display names/categories (`"Building"`, `"Walls & Paths"`, `"Doors & Windows"` in `model.rs`) stay decoupled from procedural chemistry (`glade.rs`) and saved `glade_*` IDs.
  - Editor `Mode` (`editor.rs`) has `Build`, `Building`, and `Play`; use `object_editing_allowed(editor.mode)` rather than `editor.mode == Mode::Build` for authoring checks. `editor/setup.rs` + `editor/overlay.rs` (`PaletteGroup` & `selection_palette_context`) switch between `BuildMode` and `BuildingBase`, showing `DoorsAndWindows` only when a building/wall is selected and `BuildingFixtures` when a building/wall/surface is selected.
  - `glade_door` and `glade_window` (`editor/input.rs`) only show a ghost and place when `snap_door_to_glade_wall_with_elevation` hits a building wall.
  - `Shape::is_glade()` pieces (`glade_room`, `glade_tower`, `glade_wall`, `glade_door`, `glade_window`, `glade_terrain`, etc.) use Tiny Glade–style white gizmos in `editor/gizmo.rs` (`glade_transform_gizmo`): dashed wireframe + roof ridge rendered via `XrayGizmos` (`depth_bias: -1.0`, `configure_xray_gizmos`), mid-height face resize arrows (`GladeWidth` / `GladeLength`), vertical height bar (`GladeHeight` at `(+hx, +hz)`), roof-pitch apex arrow (`GladeRoofPitch`), **4 outward ground-plane move arrows** (`glade_move_arrows` -> `GizmoHandle::Xz`), and **2 opposite-corner ground-plane rotation arcs** (`glade_rotation_arcs` -> `GizmoHandle::Yaw`).
  - Glade pieces only rotate around the Y axis (`model::set_piece_quat` clamps `pitch` and `roll` to `0.0`); `gizmo_hit` scores all visible Glade handles together by screen-space distance so nearby handles do not shadow each other.
  - `DefaultGizmoConfigGroup` is depth-tested against 3D meshes, so ground-plane Glade controls are placed outside the piece's footprint (`y = 0.05`) rather than inside the mesh; `GladeKind::Path` uses the route/waypoint gizmo instead.
- **Codebase knowledge graph**: If `codebase-memory-mcp` is available, project `home-kon-dev-bevy` is indexed for graph/symbol queries.
