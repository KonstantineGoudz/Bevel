# Party Dungeon — Architecture Review

This folder contains a full architectural review of **Party Dungeon** (`/home/kon/dev/bevy`), covering the Rust workspace (`game/crates/game`, `game/crates/mcp-server`), Bevy `0.20.0` / `bevy_rapier3d` `0.37` integration, procedural Glade geometry, editor UI, gameplay/navigation, and the live MCP bridge.

## Executive Summary

Party Dungeon has several strong architectural foundations:
- **Pure, headless-testable domain modules**: [`locomotion.rs`](file:///home/kon/dev/bevy/game/crates/game/src/locomotion.rs), [`statuses.rs`](file:///home/kon/dev/bevy/game/crates/game/src/statuses.rs), [`model.rs`](file:///home/kon/dev/bevy/game/crates/game/src/model.rs), and [`path.rs`](file:///home/kon/dev/bevy/game/crates/game/src/path.rs) isolate math, state machines, and navigation logic from Bevy ECS systems so they can be unit-tested without a window.
- **Deterministic procedural chemistry & custom shaders**: [`glade.rs`](file:///home/kon/dev/bevy/game/crates/game/src/glade.rs) and [`grass_material.rs`](file:///home/kon/dev/bevy/game/crates/game/src/grass_material.rs) implement rich procedural building/terrain generation and custom `ExtendedMaterial` grass shading.
- **Dual-mode boot**: [`lib.rs`](file:///home/kon/dev/bevy/game/crates/game/src/lib.rs) cleanly separates `viewer::ViewerPlugin` (no physics, no editor state) from `GamePlugin` + `RapierPhysicsPlugin`.

However, rapid feature growth has introduced **significant architectural errors, structural coupling, and performance bottlenecks**:
1. **Broken workspace boundary**: [`mcp-server/Cargo.toml`](file:///home/kon/dev/bevy/game/crates/mcp-server/Cargo.toml) depends on the entire `party-dungeon` crate (pulling in all of Bevy and Rapier) while still duplicating persistence helpers from [`session.rs`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs).
2. **Scheduling & correctness bugs in `EditorPlugin`**: [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs) registers `sync_selected_glade_wall_mover` twice, runs `bridge_drain` *after* `sync_visuals` / `sync_minimap` / `autosave_session` (contradicting its own doc comment), and chains ~80 systems in `Update` without `States` gating or `FixedUpdate` for physics.
3. **Monolithic god-files and god-functions**: [`glade.rs`](file:///home/kon/dev/bevy/game/crates/game/src/glade.rs) is 7,485 lines (6,519 function lines), [`editor/input.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/input.rs#L383) has an 825-line `interact` function (cognitive complexity 344, 15 parameters), and [`Editor`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L307) is a single mega-resource mixing level data, undo history, tool state, grass strokes, and terrain strokes.
4. **Per-frame $O(N^2)$ loops & GPU asset leaks**: [`sync_visuals`](file:///home/kon/dev/bevy/game/crates/game/src/editor/visuals.rs#L136) performs $O(N^2)$ ID scans and allocates new `Mesh`/`StandardMaterial` handles on every Glade rebuild without freeing old ones; [`draw`](file:///home/kon/dev/bevy/game/crates/game/src/editor/draw.rs#L10) runs 1,734 terrain-height queries and 289 `Vec` allocations per frame for the hex grid.
5. **Blocking single-threaded HTTP bridge**: [`editor/bridge.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/bridge.rs#L181) parses HTTP/1.0 one byte at a time and blocks the listener loop for up to 8 seconds per `POST /command`.
6. **Current build state**: `cargo check --workspace` currently fails with 10 compile errors in `party-dungeon` due to a partially wired `vines` feature (`SavedVine`, `sync_vine_actions`, `PaletteAction::Vine`, `ActorTool::Vine`, `Kit` vine materials).

## Review Index

| Document | Scope |
| --- | --- |
| [01-workspace-and-plugins.md](file:///home/kon/dev/bevy/docs/architecture-review/01-workspace-and-plugins.md) | Crate topology, Bevy plugin/schedule architecture, system ordering bugs |
| [02-data-model-and-persistence.md](file:///home/kon/dev/bevy/docs/architecture-review/02-data-model-and-persistence.md) | `model.rs`, `session.rs`, `Snapshot` schema, undo/redo, native vs WASM split |
| [03-editor-and-ui-architecture.md](file:///home/kon/dev/bevy/docs/architecture-review/03-editor-and-ui-architecture.md) | `Editor` mega-resource, `use super::*` coupling, `interact()`, UI duplication |
| [04-procedural-geometry-and-rendering.md](file:///home/kon/dev/bevy/docs/architecture-review/04-procedural-geometry-and-rendering.md) | `glade.rs` monolith, `sync_visuals` leaks, `draw` overhead, `viewer.rs` |
| [05-gameplay-physics-and-navigation.md](file:///home/kon/dev/bevy/docs/architecture-review/05-gameplay-physics-and-navigation.md) | `Update` vs `FixedUpdate`, `path::blocked` $O(N^2)$ scans, locomotion & colliders |
| [06-bridge-and-mcp-server.md](file:///home/kon/dev/bevy/docs/architecture-review/06-bridge-and-mcp-server.md) | `editor/bridge.rs` HTTP server, `mcp-server` dependency & duplication |
| [07-improvement-roadmap.md](file:///home/kon/dev/bevy/docs/architecture-review/07-improvement-roadmap.md) | Prioritized refactoring roadmap (P0–P3) |
