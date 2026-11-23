# Party Dungeon MCP server

An MCP (Model Context Protocol) server for Party Dungeon. AI agents connect
over stdio and can **query the world, add / edit / delete objects, set
platform paths, plan routes, and control the player**.

The game is an MMO with **one shared world** — there are no level files. The
system of record is SurrealDB (`dungeon_piece` / `platform_path`); the sim
loads it, is authoritative at runtime, and pushes geometry to clients on
join and on every change. Edits made here land in the database, and the sim
hot-reloads them within ~2s (then broadcasts the new world to players).

## Layout

- `game/crates/mcp-server/` — this server (`party-dungeon-mcp` binary).
- `game/crates/game/src/editor/bridge.rs` — the in-game live bridge
  (localhost HTTP, native builds only) for instant edits to a running game.
- `game/crates/server/` — the SurrealDB schema + service-role `Backend`.

## Backends

| `source` | Behaviour |
|---|---|
| `auto` (default) | Prefer the live game bridge; fall back to SurrealDB |
| `db` | SurrealDB only (the single shared world) |
| `live` | Live game only; error when the bridge is unreachable |

### Environment

| Variable | Meaning |
|---|---|
| `DUNGEON_SURREAL_URL` | DB endpoint, `host:port` (default `127.0.0.1:6000`) |
| `DUNGEON_SURREAL_NS` / `DUNGEON_SURREAL_DB` | Namespace / database (default `party_dungeon` / `main`) |
| `DUNGEON_SERVICE_USER` / `DUNGEON_SERVICE_PASS` | Service-role DB user (from `crates/server/.env`) |
| `PARTY_DUNGEON_BRIDGE=<addr>` | Live bridge address (default `127.0.0.1:6559`; empty disables) |

Seeding a fresh world: import a snapshot once with the `import_level_json`
tool (e.g. `game/levels/demo.json`), or `Backend::import_snapshot`. After
that the world lives in the database and is edited in place.

## Live bridge

Run the game with the bridge on (default on native) and the server forwards
edits to the running instance instantly instead of waiting for the DB poll:

```sh
PARTY_DUNGEON_BRIDGE=1 ./party-dungeon
```

Probe it: `GET http://127.0.0.1:6559/health`, `GET /state`,
`POST /command {"op":"status"}`. The bridge only binds loopback addresses.

### Screenshots

`GET /screenshot` returns one frame as raw `image/png` — but only when the
game runs **headless**: a windowed client has no offscreen target to read
back and answers `503` with a JSON error.

```sh
./party-dungeon --headless                     # 1280x720 @ 60 fps, bridge on
./party-dungeon --headless --headless-size 1600x900 --headless-fps 30
```

The `screenshot` tool wraps that endpoint: it fetches a PNG from the live
bridge, optionally writes it to `path` (parent dirs are created), and returns
it inline as an MCP image content block plus a JSON summary (`bytes`,
`contentType`, saved `path`). The first frame right after startup can take
several seconds — captures are only armed once the renderer has drawn a
frame, and a **software renderer** spends ~5 s compiling pipelines first. The
bridge keeps the request open for up to 12 s and the MCP server waits up to
20 s, so a cold game still returns a frame instead of an error.

```jsonc
{ "path": "/tmp/dungeon.png" }   // path is optional; omit for inline only
```

Without `--headless` (or with the game not running) the tool fails with a
`tool_error` that says so; `bridge_status` reports the render info from
`/health` (`headless`, `render`, `screenshot`) in its `health` field.

## Tools (24)

**Queries** — `list_shapes`, `get_world`, `describe_world`, `list_pieces`,
`get_piece`, `get_character`, `get_platforms`, `find_path`, `plan_route`,
`validate_world`, `bridge_status`, `screenshot` (headless game only)

**Editing** — `add_piece`, `update_piece`, `delete_pieces`, `clear_world`,
`set_character`, `remove_character`, `set_platform_path`,
`clear_platform_path`, `import_level_json`, `export_level_json`
(`paint_grass` is editor-session-local; the shared world does not persist
grass, so the tool reports that instead of silently dropping data)

**Player** — `player_control` with
`teleport | damage | heal | set_hp | respawn | walk_to | stop`.
`walk_to` needs the live game (it arms the real route follower);
everything else persists to SurrealDB.

Each mutating tool takes `live: true` by default (write the database *and*
forward to the running game when reachable). Queries take
`source: "auto" | "db" | "live"`.

## Resources

- `world://active` — current world snapshot
- `world://shapes` — shape catalogue
- `world://platforms` — platform paths
- `world://character` — player character

## Run it

```sh
cargo run -p party-dungeon-mcp
```

Release (much smaller binary): `cargo build --release -p party-dungeon-mcp`.

## Client configuration

Claude Desktop / Cursor / VS Code (`mcp.json` / `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "party-dungeon": {
      "command": "/path/to/game/target/release/party-dungeon-mcp",
      "env": {
        "DUNGEON_SURREAL_URL": "127.0.0.1:6000",
        "DUNGEON_SURREAL_NS": "party_dungeon",
        "DUNGEON_SURREAL_DB": "main",
        "DUNGEON_SERVICE_USER": "service",
        "DUNGEON_SERVICE_PASS": "<from crates/server/.env>",
        "PARTY_DUNGEON_BRIDGE": "127.0.0.1:6559"
      }
    }
  }
}
```

Then: start the game (add `--headless` there too if you want screenshots) and
ask the agent to build. Example prompts:

- “List the shapes, then build a 3×3 floor of `floor` hexes around the origin.”
- “Add a wall across the middle with a door gap, then find a path from (-6,0) to (6,0).”
- “Put the character at the entrance, walk them to the chest, and report arrival.”
- “Give platform on piece #4 a loop through these waypoints and pause it.”
- “Take a screenshot of the dungeon and save it to /tmp/dungeon.png.”

## Shape ids (quick reference)

`floor pit water bridge | wall fence window pillar | block ramp |
door gate secret rubble | crate chest altar brazier | herb ore |
spawn entry | spikes | trigger trigger_box trigger_sphere trigger_cylinder`

World units are metres on the ground plane (`x`, `z`); `elevation` lifts the
piece; rotations are degrees (`rotDeg` yaw, `pitchDeg`, `rollDeg`).

## Tests

```sh
cargo test -p party-dungeon-mcp   # file store, validation, conversions
cargo test -p party-dungeon        # game incl. bridge-adjacent systems
```
