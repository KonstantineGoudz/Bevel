# 06 — Live Bridge & MCP Server Architecture

## 1. Live HTTP Bridge (`editor/bridge.rs`)

[`editor/bridge.rs`](file:///home/kon/dev/bevy/game/crates/game/src/editor/bridge.rs) (745 lines) embeds a localhost HTTP endpoint (`127.0.0.1:6559` by default) on native builds so the MCP server can inspect and mutate a running game instance.

### Architectural Errors & Limitations

1. **Single-Threaded Blocking `TcpListener::incoming()` Loop**
   In [`serve`](file:///home/kon/dev/bevy/game/crates/game/src/editor/bridge.rs#L181-L235):
   - Connections are handled sequentially on one background thread (`mcp-bridge`).
   - For `POST /command`, the handler blocks on `rx.recv_timeout(Duration::from_secs(8))` waiting for the Bevy main thread to drain the inbox.
   - **Failure mode**: If the game is minimized, paused, or slow to frame, or if a client stalls mid-request (up to 5s read timeout), **all other endpoints (`GET /health`, `GET /state`) are completely blocked** behind that single connection.
   - **Improvement**: Either dispatch each accepted `TcpStream` to a small thread pool / non-blocking loop, or use a lightweight async/pooled HTTP server.

2. **Byte-by-Byte Unbuffered HTTP/1.0 Parsing**
   [`read_request`](file:///home/kon/dev/bevy/game/crates/game/src/editor/bridge.rs#L116-L165) reads HTTP headers **1 byte at a time** (`stream.read(&mut byte)`) with no `BufReader`. While local traffic is small, this issues hundreds of `read` syscalls per request and only supports `HTTP/1.0` without proper header/status-code handling.

3. **Frame Ordering Bug (`bridge_drain` vs `sync_visuals`)**
   As noted in [01-workspace-and-plugins.md](file:///home/kon/dev/bevy/docs/architecture-review/01-workspace-and-plugins.md), [`editor.rs:286-291`](file:///home/kon/dev/bevy/game/crates/game/src/editor.rs#L286-L291) schedules `(bridge_drain, bridge_publish).chain().after(autosave_session)`, which contradicts the comment on lines 284–285 (`drain commands before the minimap/visuals read the editor`).
   - **Fix**: Schedule `bridge_drain` *before* `rebuild_nav` / `sync_visuals` / `sync_minimap`, and keep `bridge_publish` *after* `sync_visuals` / `autosave_session`.

4. **Expensive `GET /state` Serialization**
   `bridge_publish` calls `editor.capture(orbit, movers).to_json()` at 2 Hz or whenever `editor.hard_dirty` is set, which serializes the full `Snapshot` including up to 60 `undo` and 60 `redo` `SavedLevel` histories into a JSON `String` on the main game thread.
   - **Improvement**: Strip `undo`/`redo` from the live bridge snapshot (or serialize on a background thread) so `bridge_publish` never stalls the render loop.

5. **Whole-world authority vs the streamed client world**
   `GET /state` serves whatever the live game has in its editor world. Since interest management (see [interest-management.md](file:///home/kon/dev/bevy/docs/interest-management.md)) streams only the cells around each character, a client that streamed normally holds a **partial** world — and `party-dungeon-mcp`'s `save_world` / `import_level_json` write that snapshot back through `import_snapshot`, which would replace the authoritative world with the local fragment. The bridge therefore requires the attached client to request full-world interest (`ClientMsg::SetInterest` with `None` radii): bridge default-on for native builds does this, and `DUNGEON_FULL_WORLD=1` forces it. Keep those in sync when touching either side; a bridge attached to a streaming client is a data-loss bug, not a display bug.
   - **Future hardening**: have `/health` report the client's effective interest radii so the MCP server can refuse mutating tools against a partial world, rather than relying on the client-side env defaults. Note that full-world interest still drains through the per-peer chunk budget (`CHUNK_SEND_BUDGET`): a 15 625-cell world takes ~977 ticks (~16 s) before `/state` is complete. For very large worlds, point the MCP tools at SurrealDB (`Source::Db`/`Source::Auto`) instead of the live bridge. See [interest-management.md](file:///home/kon/dev/bevy/docs/interest-management.md) §7.

---

## 2. MCP Server (`crates/mcp-server/`)

[`crates/mcp-server/src/main.rs`](file:///home/kon/dev/bevy/game/crates/mcp-server/src/main.rs) (1,549 lines) and [`store.rs`](file:///home/kon/dev/bevy/game/crates/mcp-server/src/store.rs) (854 lines) expose 23 MCP tools and 4 `level://` resources over stdio.

### Architectural Issues

1. **Pulling All of Bevy + Rapier into `mcp-server`**
   [`mcp-server/Cargo.toml:14`](file:///home/kon/dev/bevy/game/crates/mcp-server/Cargo.toml#L14) depends on `party-dungeon = { path = "../game" }`, which compiles and links the entire Bevy engine and `bevy_rapier3d` into the headless CLI binary.
   - **Fix**: Depend only on a new `party-dungeon-core` crate.

2. **Duplicated Logic Between `store.rs` and `session.rs`**
   `sanitize_level_name` (1.000 similarity) and `snapshot_from_level_json` (0.984 similarity) are copy-pasted between [`store.rs`](file:///home/kon/dev/bevy/game/crates/mcp-server/src/store.rs#L41) and [`session.rs`](file:///home/kon/dev/bevy/game/crates/game/src/session.rs). Moving `Snapshot` + validation into `party-dungeon-core` eliminates this drift risk.

3. **Monolithic `main.rs` (1,549 Lines)**
   All 23 tool parameter structs, tool handlers, and resource handlers live in one file. Splitting into `tools/query.rs`, `tools/edit.rs`, `tools/play.rs`, and `resources.rs` will make the MCP surface much easier to maintain.
