# 01 — Workspace Topology & Bevy Plugin Architecture

## 1. Current Workspace Layout

The Cargo workspace in [`game/Cargo.toml`](file:///home/kon/dev/bevy/game/Cargo.toml) has two members:
- `crates/game` (`party-dungeon`, `lib` + `cdylib`) — depends on `bevy = "=0.20.0"`, `bevy_rapier3d = "0.37"`, `serde`, `serde_json`, and WASM bindings.
- `crates/mcp-server` (`party-dungeon-mcp`, `bin`) — depends on `rmcp`, `tokio`, `serde`, `schemars`, `anyhow`, `bevy_math = "=0.20.0"`, **and `party-dungeon = { path = "../game" }`**.

### Architecture Error: Missing `party-dungeon-core` Crate
[`mcp-server/Cargo.toml`](file:///home/kon/dev/bevy/game/crates/mcp-server/Cargo.toml#L21-L23) contains a comment explaining why it depends on `bevy_math`:
> `# Same math types the game uses (bevy re-exports this exact crate), without pulling the whole engine into the MCP server.`

Yet line 14 directly depends on `party-dungeon = { path = "../game" }`, which **pulls the entire Bevy engine and `bevy_rapier3d` into `mcp-server`** anyway!
- **Impact**: `mcp-server` suffers from full Bevy + Rapier compile times and binary bloat, even though it only uses `model`, `path`, `platforms`, and `session` data types.
- **Fix**: Extract a headless `crates/core` (`party-dungeon-core`) crate containing `model`, `path`, `platforms`, `statuses`, `locomotion`, and the `Snapshot` serde types, depending only on `bevy_math`, `bevy_color`, `serde`, and `serde_json`. Both `party-dungeon` and `party-dungeon-mcp` should depend on `party-dungeon-core`.

---

## 2. Bevy Plugin & Schedule Architecture

### Strengths
- [`lib.rs`](file:///home/kon/dev/bevy/game/crates/game/src/lib.rs#L78-L83) branches cleanly between `viewer::ViewerPlugin` and `(RapierPhysicsPlugin::<NoUserData>::default(), GamePlugin)`.
- [`GamePlugin`](file:///home/kon/dev/bevy/game/crates/game/src/lib.rs#L157-L163) delegates to `editor::EditorPlugin`.

### Architecture Errors & Bugs in `EditorPlugin`

1. **Duplicate System Registration**
   In [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L143-L176), `sync_selected_glade_wall_mover.after(prune_movers)` is registered **twice** in `Update` (once at lines 143–146, and again at lines 173–176). This runs the system twice every frame.

2. **Bridge Drain Ordering Bug**
   [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L284-L291) states:
   > `// Live MCP bridge: drain commands before the minimap/visuals read the editor, then publish the fresh snapshot.`
   
   However, the actual schedule is:
   ```rust
   .add_systems(
       Update,
       (bridge_drain, bridge_publish)
           .chain()
           .after(autosave_session),
   )
   ```
   Because `autosave_session` runs `.after(sync_visuals)`, `bridge_drain` actually runs **after** `sync_visuals`, `sync_minimap`, `draw`, and `autosave_session`. Live MCP mutations wait a full frame to appear visually and are not included in that frame's autosave.

3. **Rigid `.chain()` Tuples & Lack of `SystemSet`s / `States`**
   [`EditorPlugin::build`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L99-L299) registers ~80+ systems across ~7 giant `.chain()` tuples split only because Bevy's tuple trait limit was hit (noted explicitly in comments at lines 177–179, 196–198, 205–207, and 271–272).
   - **No Bevy `States` for `Mode` (`Build`, `Building`, `Play`) or `HomeMenu.open`**: Instead of `.run_if(in_state(...))`, dozens of systems run every frame and manually early-return by checking `editor.mode` or `home.open`.
   - **No `SystemSet` phases**: Ordering is expressed with ad-hoc `.after(refresh_route)`, `.after(sync_visuals)`, `.after(autosave_session)` anchors instead of semantic sets (`EditorSet::Input`, `EditorSet::Simulation`, `EditorSet::SyncVisuals`, `EditorSet::Ui`, `EditorSet::Bridge`).
   - **No `FixedUpdate` for physics/gameplay**: `walk`, `character_move`, `hazard_damage`, `move_movers`, `resolve_platform_push`, `resolve_dynamic_push`, and `advance_cones` all run in `Update` rather than `FixedUpdate`.

4. **Current Compilation Breakage (WIP State)**
   Running `cargo check --workspace` currently fails with 10 errors in `crates/game` due to an incomplete `vines` integration:
   - Missing `SavedVine` and `Snapshot.vines` / `Loaded.vines` in [`session.rs`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs)
   - Missing `sync_vine_actions` in [`editor.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L259)
   - Missing `vine_leaf_mat` / `vine_stem_mat` in `Kit` initialization in [`editor/setup.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/setup.rs#L252)
   - Non-exhaustive matches on `PaletteAction::Vine` / `PaletteAction::VineErase` and `ActorTool::Vine` in [`editor/input.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/input.rs#L57), [`editor/overlay.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/overlay.rs#L14), and [`editor/touch.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/touch.rs#L452).
