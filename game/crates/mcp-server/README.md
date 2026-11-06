# Party Dungeon MCP server

An MCP (Model Context Protocol) server for the Party Dungeon level editor.
AI agents connect over stdio and can **query all level data, add / edit /
delete objects, set platform paths, plan routes, and control the player** —
against level files, and live against a running game.

## Layout

- `game/crates/mcp-server/` — this server (`party-dungeon-mcp` binary).
- `game/crates/game/src/editor/bridge.rs` — the in-game live bridge
  (localhost HTTP, native builds only).
- `game/levels/` — default directory for level files (created on demand).

## Level files

The canonical format is the session **Snapshot JSON** (`{"version":1,…}` —
the same shape the game keeps in browser storage). The server reads and
writes `<levels>/<name>.json`. It can also **import** the debug `level.json`
the game writes with `S` / CopyJson (`format: party-dungeon-level`), and
**export** back to it.

Resolution, in order:

| Setting | Meaning |
|---|---|
| `PARTY_DUNGEON_LEVEL=/path/to/file.json` | Use this exact snapshot file |
| `PARTY_DUNGEON_LEVEL=<name>` | Default level name (default: `level`) |
| `PARTY_DUNGEON_LEVELS_DIR=<dir>` | Levels directory (default: `./game/levels`, else `./levels`) |
| `PARTY_DUNGEON_BRIDGE=<addr>` | Live bridge address (default `127.0.0.1:3559`; empty disables) |

The native game boots from the same file: `party-dungeon --level
/path/to/file.json` (or the env var). Whatever the MCP server saves, the
game loads on (re)start.

## Live bridge

Run the game with the bridge on (default on native) and the server forwards
edits instantly instead of waiting for a reload:

```sh
PARTY_DUNGEON_BRIDGE=1 ./party-dungeon --level ./game/levels/level.json
```

Probe it: `GET http://127.0.0.1:3559/health`, `GET /state`,
`POST /command {"op":"status"}`. The bridge only binds loopback addresses.

## Tools (23)

**Queries** — `list_shapes`, `get_level`, `list_pieces`, `get_piece`,
`get_character`, `get_platforms`, `find_path`, `plan_route`,
`validate_level`, `list_levels`, `bridge_status`

**Editing** — `add_piece`, `update_piece`, `delete_pieces`, `clear_level`,
`set_character`, `remove_character`, `set_platform_path`,
`clear_platform_path`, `paint_grass`, `import_level_json`,
`export_level_json`

**Player** — `player_control` with
`teleport | damage | heal | set_hp | respawn | walk_to | stop`.
`walk_to` needs the live game (it arms the real route follower);
everything else also persists to the file.

Each mutating tool takes `live: true` by default (apply to the file *and*
forward to the running game when reachable). Queries take
`source: "auto" | "file" | "live"`.

## Resources

- `level://active` — current level snapshot
- `level://shapes` — shape catalogue
- `level://platforms` — platform paths
- `level://character` — player character

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
        "PARTY_DUNGEON_LEVELS_DIR": "/path/to/game/levels",
        "PARTY_DUNGEON_BRIDGE": "127.0.0.1:3559"
      }
    }
  }
}
```

Then: start the game with `--level <same file>`, and ask the agent to
build. Example prompts:

- “List the shapes, then build a 3×3 floor of `floor` hexes around the origin.”
- “Add a wall across the middle with a door gap, then find a path from (-6,0) to (6,0).”
- “Put the character at the entrance, walk them to the chest, and report arrival.”
- “Give platform on piece #4 a loop through these waypoints and pause it.”

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
