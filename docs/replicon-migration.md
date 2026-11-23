# bevy_replicon replication path (landed)

**Status: landed and deployed.** The live client (`play.conman.biz`) and the
sim image now run the `bevy_replicon 0.45` path (`crates/replication` +
`crates/repl_matchbox`) over the existing matchbox/WebRTC room. The build stamp
verified live is **`2026-11-17.8`**; the rule is: `CLIENT_BUILD`
in `game/crates/game/src/net_client.rs` must equal `BUILD` in
`game/web/game.html`, and the HUD prints that value — read it to know which
client a deployment is actually serving.

The hand-written snapshot/chunk protocol (`crates/net` + `HostNet` /
`NetClientPlugin`) is still in-tree and is the **default unbuilt** path: both
crates select it unless the `replicon` cargo feature is enabled. It is the
rollback. The legacy reference docs remain valid for that build:
[docs/interest-management.md](interest-management.md) (chunk streaming /
per-receiver snapshots) and [docs/multiplayer-connectivity.md](multiplayer-connectivity.md)
(signaling, ICE/TURN and the legacy HUD states).

---

## 1. Why replicon (and why not renet)

`bevy_replicon_renet 0.21` only ships raw-UDP netcode and Steam transports. Raw
UDP is impossible from a browser, so renet cannot serve the WASM client on
`play.conman.biz`. `bevy_replicon` itself is transport-agnostic: a messaging
backend just moves raw bytes between replicon's `ServerMessages`/
`ClientMessages` and a transport. We implement that backend over the **same
matchbox signaling room and TURN relays** the legacy path used
(`crates/repl_matchbox`), so the deployment (NPM locations, coturn, NAT story,
`/party-dungeon` room id) is unchanged.

## 2. Crates and runtime topology

| Crate / module | Role |
|---|---|
| `game/crates/replication` (`party-dungeon-replication`) | Shared replicon schema: replicated components, messages, visibility filters, quantized postcard encoding for both ends. Registration order is part of the protocol hash. |
| `game/crates/repl_matchbox` (`party-dungeon-repl-matchbox`) | Replicon messaging backend over matchbox/WebRTC: host/client sockets, postcard handshake, channel pump, `ConnectedClient` lifecycle, `ClientStats`/`ConnectedClientStats`, ICE passthrough, in-memory `testkit` transport for tests. |
| `game/crates/sim/src/repl/` | Server role. The authoritative Tokio loop (`authority.rs`) stays untouched; a tiny embedded Bevy app (`MinimalPlugins` + `StatesPlugin` + `RepliconPlugins` + `MatchboxServerPlugin`) runs on its own thread at 62.5 Hz, owns the replicated entities, and talks to Tokio over `std::sync::mpsc` (`ServerFrame`/`ClientIntent`). |
| `game/crates/game/src/repl_client.rs` | Client role (feature `replicon`). Replaces the socket/recv path of `net_client`; prediction, authoritative pose correction, the editor world sync and the avatar visuals are shared, not duplicated. |

Both paths share `crates/sim/src/authority.rs`: a player row and an authored
edit go through the same service-role reducers, the same validation and the same
**persist → apply → one `dungeon_version` bump per batch** ordering whichever
transport carried them.

## 3. Wire contract (`crates/replication`)

All traffic is **binary postcard** — no JSON, no protobuf. The hot player pose
is fixed-point quantized; geometry is quantized on the same scales (the encoder
rounds, the decoder divides).

| Field family | Encoding | Step / max rounding error |
|---|---|---|
| position / elevation | cm (`i32` / `i16`) | 1 cm / ≤0.005 m |
| extents (`trigger_w/h/l`) | cm (`u16`) | 1 cm / ≤0.005 m |
| yaw / rot | `u16` over one full turn | ~0.0055° / ≤0.0028° |
| pitch / roll | centi-degrees (`i16`) | 0.01° / ≤0.005° |
| glade roof pitch (`trigger_alpha`) | `u16` | 1/65535 / ~7.6e-6 |
| `piece.damage` | whole store points (`u16`, 0..=10000) | **1 point** / ≤0.5 |
| hp | centi-hp (`u16`) | 0.01 hp / ≤0.005 hp |
| velocity | cm/s (`i16`) | 0.01 m/s / ≤0.005 m/s |

Replicated components:

- `PieceId(rid)`, `PieceData` (shape index, quantized geometry, damage, flags,
  interact id, trigger extents/alpha), `PiecePath` (waypoints in cm, speed cm/s,
  mode byte, paused) — one entity per world piece; movers are the same entity's
  path component.
- `PlayerSub`, `PlayerName`, `PlayerPose` (quantized pose + hp + alive/online
  flags) — one entity per player.
- Visibility filters: `WorldCell` (32 m grid) with client component
  `VisibleCells`, and `PlayerCell` with client component `VisiblePlayerCells`.
  Geometry and players use separate interest sets, so players can be filtered at
  a shorter radius than world geometry. Both are immutable: re-insert on change.

Messages:

| Direction | Message | Replicon channel | Notes |
|---|---|---|---|
| client → server | `InputMsg(party_dungeon_net::Input)` | unreliable | one 60 Hz input tick |
| client → server | `WorldEditMsg { ops: Vec<WorldOp> }` | ordered | ≤ `MAX_WORLD_OPS` (64); the same `WorldOp::{Place,Update,Delete}` the legacy path used |
| server → client | `WelcomeMsg { sub, name, tick, dungeon_version, world_radius }` | ordered | identity + session start; published resource/message by the backend |
| server → client | `EditAppliedMsg { tag, rid }` | ordered | optimistic `Place` tag → store rid |
| server → client | `EditRejectedMsg { key, reason }` | ordered | optimistic copy rolls back |
| server → client | `NoticeMsg(String)` | unreliable | cheap chat/notice lines (kills today) |

`party_dungeon_replication::register(app)` installs components, filters and
messages in a fixed order on both ends; the order is part of the protocol hash.
The four server messages are `make_message_independent`, i.e. they do not depend
on replicated entity state.

## 4. Backend contract (`crates/repl_matchbox`)

### Channels

matchbox channel **0** is a reliable control channel carrying the postcard
`Handshake`. Replicon channel `i` maps to matchbox channel `i + 1`; when the
two directions registered different reliabilities at one replicon index,
matchbox gets the **stronger** of the two.

| matchbox | server side | client side | reliability |
|---|---|---|---|
| 0 | handshake (control) | handshake (control) | reliable |
| 1 | `ServerChannel::Updates` | `ClientChannel::MutationAcks` | reliable |
| 2 | `ServerChannel::Mutations` | `InputMsg` | unreliable |
| 3.. | server messages/events | client messages/events | merged |

`ConnectedClient::max_size` defaults to 16 KiB (the matchbox message cap) and is
used by replicon's packet splitting.

### Handshake and auth

```text
client                                        server host (sim)
  │ ── Handshake::Hello { token, protocol } ──► │  (reliable control ch 0)
  │                                            │  protocol hash equal?
  │                                            │  JWT verified (same claim
  │                                            │  decoder as legacy Hello:
  │                                            │  decode `sub`; DB row rules
  │                                            │  enforce iss/aud)
  │ ◄── Handshake::Welcome { sub, name, tick } ┘  or Handshake::Deny { reason }
  │      (then replicon replication starts)
  │ ── Handshake::Ping { nonce } ──►  ◄── Handshake::Pong { nonce }   (RTT probe)
```

- The **protocol hash** (all component/message registrations) is compared in the
  handshake: a client and server built from different schemas cannot talk —
  `Deny { reason: "protocol mismatch" }`. This is why both ends must be rebuilt
  together.
- The JWT is the live `AuthSession` token (or the dev env-token fallback); the
  sim's `JwtAuthenticator` reuses the exact same decoder as the legacy path. On
  success the backend spawns a `ConnectedClient` entity, inserts
  `AuthorizedClient` with `AuthMethod::Custom`, and the sim persists/joins the
  player row (`ClientSession { sub, name }` → authority `Join`).
- The client publishes `SessionInfo` / `SessionStarted` on success and
  `SessionDenied` on a rejected handshake (`BackendStatus.state = Failed`).
- Token refresh replaces `ClientBackendConfig` and re-dials/re-handshakes with
  the fresh token; an unchanged token never reconnects.

### Assets, ICE and lifecycle

- ICE/TURN is unchanged: the same `DUNGEON_ICE_SERVERS` JSON (browser:
  `window.DUNGEON_ICE_SERVERS` / `?ice=`) is parsed by the existing helper and
  inserted as `BackendIceServer` on both ends. Unset keeps matchbox's built-in
  STUN defaults. The room is still `ws(s)://…/party-dungeon`.
- Socket creation is lazy: the client creates the socket only when
  `ClientBackendConfig { signaling_url, token }` exists (i.e. after login).
- `ClientState` (`Disconnected → Connecting → Connected`), `BackendStatus {
  state: Idle|WaitingForHost|Handshaking|Connected|Failed, detail, relayed }`,
  `ReconnectRequest`, replicon's `DisconnectRequest` (server sends a control
  `Deny`, despawns the client entity and tears the peer down), and
  `BackendRuntime` (native: drive the socket loop on the host Tokio runtime
  instead of the backend's dedicated thread).

### Stats

- Client: replicon `ClientStats` resource — `rtt` (seconds) from the control
  Ping/Pong, `sent_bps`/`received_bps` over a ~250 ms window. The game logs it
  once a second while connected:
  `replicon stats: rtt 42.0 ms, loss 0.00%, sent 12.3 KiB/s, recv 34.5 KiB/s`.
  `packet_loss` stays 0: matchbox/SCTP exposes no loss metric from the client
  side either.
- Server: each `ConnectedClient` entity has replicon `ConnectedClientStats`
  (`sent_bps`/`received_bps`; `rtt` and `packet_loss` stay 0 — matchbox/SCTP
  exposes neither). Today the sim fills them but does not log them.

## 5. Server flow (sim)

1. `main` starts signaling (`:6536`) and the auth API (`:6565`) for both paths,
   then `repl::enabled()` (`DUNGEON_REPLICON=1` + feature compiled in) selects
   the replicon loop; otherwise the legacy `HostNet` loop runs.
2. `repl::app` spawns the embedded Bevy app thread (62.5 Hz). It hosts the
   matchbox server socket, the postcard handshake and replicon.
3. The Tokio authority loop keeps the `Sim`, 60 Hz tick, SurrealDB persistence
   and DB hot-reload poll. Each tick it sends one `ServerFrame` (player poses
   every tick; a `PieceDelta` only when `dungeon_version` moved — upserts,
   path add/remove and deletes) and receives `ClientIntent`s (inputs, edit
   batches, joins, leaves).
4. On an authorized client the app re-inserts `VisibleCells` around the player
   (`DUNGEON_WORLD_RADIUS`) and `VisiblePlayerCells`
   (`DUNGEON_PLAYER_RADIUS`) whenever the player's cell changes; replicon then
   filters replication per client.
5. Edits go through the shared authority: persist via the service-role
   reducers → apply to `Sim` → one version bump → next `PieceDelta` carries the
   changed pieces. The author gets `EditAppliedMsg`/`EditRejectedMsg`; a
   `NoticeMsg` (kills today) goes to all clients.

## 6. Client flow (game, feature `replicon`)

- **Connect:** once `AuthState::SignedIn` (or immediately when
  `DUNGEON_AUTH_REQUIRED=0`), `connect_when_signed_in` inserts the backend
  config with the live token and the existing signaling URL helper (same
  `DUNGEON_SIGNAL_URL` / `window.DUNGEON_SIGNAL_URL` and `?signal=` override).
- **Session:** `WelcomeMsg` → `ReplSession` (`sub`, `name`, `tick`,
  `dungeon_version`, `world_radius`); the HUD is driven by replicon
  `ClientState` + `BackendStatus` (WaitingForHost / Handshaking / Failed /
  Connected, with `relayed=` in the transition log).
- **Input/prediction:** `sample_input_repl` samples the same camera-relative
  intent as the legacy path (`PlayerInput::move_vector()` + `Orbit.yaw` +
  Actions) and sends `InputMsg` every fixed tick (60 Hz, unreliable);
  `predict_repl` advances the shared `Predicted`. `refresh_local_pose` decodes
  the replicated local player entity (`PlayerSub == WelcomeMsg.sub`) and
  `editor::replicon_pose_correction` runs it through the same
  `apply_authoritative_pose` (blend/snap, exact yaw, hp, jump protection,
  camera delta-follow) used by the legacy pose correction.
- **World:** `track_replicated_pieces` mirrors `PieceId`/`PieceData`/
  `PiecePath` entities into the **same synthetic `WorldChunks` store** the
  legacy path uses (one 32 m cell per grid square, deterministic order,
  version bump per changed cell, an empty revision when a cell's last piece
  disappears). `editor/world_sync.rs` therefore keeps its stable
  `rid → local id` mapping, its optimistic `overrides`/`locals` and its
  `WorldOp` diff unchanged. A replicated piece disappearing (server delete or
  leaving visibility) is removed from the editor world; the editor never turns
  that into a user `Delete`.
- **Edits:** the staged ops go out as `WorldEditMsg` (≤ 64/message);
  `EditAppliedMsg`/`EditRejectedMsg` feed the same `PendingWorldEdits` queue,
  so the tag→rid mapping and optimistic rollback behave exactly as on the
  legacy path.
- **Avatars:** `sync_replicated_avatars` turns replicated
  `PlayerSub`/`PlayerName`/`PlayerPose` entities into the same capsule +
  screen-projected nameplate avatars `remote_players.rs` builds, reusing its
  interpolation and nameplate systems. The presence log is unchanged:
  `remote players: N visible [subs]`.
- **Stats/HUD:** the corner box uses `BackendStatus`; stats are logged as
  described in §4.

## 7. Interest management

Replicon visibility filters replace `ChunkIndex`/`PeerView`/per-receiver
snapshots:

- geometry: `WorldCell` on pieces vs `VisibleCells` on the client, radius
  `DUNGEON_WORLD_RADIUS` (default **128 m**);
- players: `PlayerCell` on players vs `VisiblePlayerCells` on the client,
  radius `DUNGEON_PLAYER_RADIUS` (default **96 m**);
- both are sim environment variables, clamped to `MAX_*_RADIUS`. The server
  re-inserts the sets when the player's cell changes; replicon handles delta
  encoding, packet splitting, priority and acking.

**`ClientMsg::SetInterest` is not honoured on this path.** There is no
per-client interest request: radii are server-env only. Consequences:

- `DUNGEON_FULL_WORLD` / the native MCP bridge's full-world request do **not**
  widen a replicon client beyond `DUNGEON_WORLD_RADIUS`. A native full-world
  dev client on the replicon path still sees only ~128 m of geometry, so the
  live MCP bridge's `GET /state` is a partial world — prefer the MCP DB source
  (`Source::Db`) for edits, or raise `DUNGEON_WORLD_RADIUS` for the dev
  deployment. (The legacy path keeps the `None`-radii full-world behaviour.)
- Visibility is removal-semantics: losing interest and a server delete both
  look like "the entity is gone" to the editor (the migration's accepted
  trade-off; the server knows the difference, the client does not need to).

## 8. Build selection, cutover, rollback

The feature is **off by default** — every crate still builds the legacy path
unless asked otherwise.

| Where | Knob | Legacy default | Replicon |
|---|---|---|---|
| client crate | cargo feature | none | `--features replicon` on `party-dungeon` |
| sim crate | cargo feature | none | `--features replicon` on `party-dungeon-sim` |
| sim runtime | env | unset / `DUNGEON_REPLICON=0` | `DUNGEON_REPLICON=1` |
| compose build | `DUNGEON_SIM_FEATURES` → Docker `CARGO_FEATURES` build arg | empty | `replicon` |
| compose runtime | `DUNGEON_REPLICON`, `DUNGEON_WORLD_RADIUS`, `DUNGEON_PLAYER_RADIUS` | `0` / `128` / `96` | `1` (+ optional radii) |
| web build script | `--features replicon` or `PARTY_DUNGEON_FEATURES` | none | `replicon` |

### Cutover (both ends together)

The protocol hash and handshake are checked, so a replicon client cannot join a
legacy server (and vice versa). Rebuild **both** sides in the same change
window:

```sh
# 1. Sim image: build with the feature and run the new path.
#    These are compose interpolation values, so they live in game/.env
#    (crates/server/.env stays the env_file for runtime secrets/ICE).
cd game
# game/.env:
#   DUNGEON_SIM_FEATURES=replicon
#   DUNGEON_REPLICON=1
#   DUNGEON_WORLD_RADIUS=128      # optional, defaults to 128
#   DUNGEON_PLAYER_RADIUS=96      # optional, defaults to 96
./up.sh --build                   # CARGO_FEATURES=replicon enters the sim image
docker compose logs sim | grep -E "replicon replication path|replicon server \("

# 2. Web client: same feature, then bump and verify the stamp.
./scripts/build-web.sh --features replicon --release
# BUILD (web/game.html) and CLIENT_BUILD (crates/game/src/net_client.rs)
# must match (deployed: 2026-11-17.8)
# on — keep the pair equal and read the HUD to see what a deploy serves)
# The party-dungeon-web nginx container serves the new web/pkg from its mount.

# Native client (optional, e.g. with the MCP bridge):
cargo run -p party-dungeon --features replicon
```

`PARTY_DUNGEON_FEATURES=replicon ./scripts/build-web.sh --release` is the env
equivalent; the script defaults `CARGO_HOME=.cargo-home` when it exists, so
`./scripts/build-web.sh` works without exporting it.

### Rollback

```sh
# Sim: rebuild without the feature and force the legacy path.
cd game
# game/.env: DUNGEON_SIM_FEATURES=  (empty)  and  DUNGEON_REPLICON=0
./up.sh --build

# Web: rebuild without the feature and restore the legacy build stamp.
./scripts/build-web.sh --release
```

Both ends move together again. The legacy path is untouched by replicon code
(shared authority aside), so a rollback returns to the previously verified
snapshot/chunk behaviour.

## 9. Operational delta vs the legacy docs

- **Unchanged:** the matchbox room `/party-dungeon`, the NPM locations
  (`/login|/signup|/refresh` → sim `:6565`, `/party-dungeon` → sim `:6536`), the
  signaling server, the auth API, and TURN/ICE (`DUNGEON_ICE_SERVERS`). The
  coturn/STUN incident playbook in
  [multiplayer-connectivity.md](multiplayer-connectivity.md) still applies.
- **Replaced:** chunk streaming, `WorldUnload`, per-receiver `Snapshot`s and
  `ClientMsg::SetInterest` interest management → replicon visibility filters.
  The bandwidth/queue examples in
  [interest-management.md](interest-management.md) describe the legacy path.
- **HUD:** the replicon HUD is driven by `BackendStatus` (`WaitingForHost`,
  `Handshaking`, `Failed`, `Connected`) and logs `relayed=` on state changes.
  The legacy HUD's separate "host peer seen but link not established" NAT
  message (legacy `NetClient::saw_peer`) does not exist on this path — a NAT
  failure surfaces as a long `WaitingForHost`/`Failed` with the backend detail.
- **Probes:** `signal_probe` still proves signaling + host reachability (it
  joins the room and completes WebRTC; it does not speak the replicon
  handshake). `wire_probe` and `world_edit_probe` speak the **legacy** `ClientMsg`
  protocol and will not work against a replicon server — use the client logs /
  `replicon stats:` and the DB for verification instead.

## 10. Known gaps

1. **Mover-path authoring is not in `WorldEditMsg`.** Tracks replicate and
   animate via `PiecePath`, but dragging waypoints in the editor is not
   persisted (same gap as the legacy path; use the `set_platform_path` reducer
   / MCP for authoring).
2. **Touch steering is not mirrored into `InputMsg`.** Touch stick/buttons feed
   only the local controller (editor-private `TouchState`), so a phone's
   authoritative body is not driven by the sim (same gap as the legacy path).
3. **Visibility removal vs delete is unified client-side.** Both despawn the
   replicated entity and the editor treats it as "removed from the world".
4. **`piece.damage` is quantized to whole store points** (legacy sent `f32`), so
   fractional damage values round to the nearest integer point.
5. **Interest is env-only** (see §7): no per-client skinnier/looser interest,
   and the MCP full-world native bridge does not get the whole world.
6. **Server-side RTT/loss are unavailable** (matchbox/SCTP), so
   `ConnectedClientStats.rtt`/`packet_loss` stay 0 and the stats log's `loss`
   field always reads 0.

## 11. Verification

```sh
# Backend: both roles + the in-memory two-app transport.
CARGO_HOME=.cargo-home cargo check -p party-dungeon-repl-matchbox --features server,client
CARGO_HOME=.cargo-home cargo test  -p party-dungeon-repl-matchbox --features server,client,testkit

# Schema / sim server role / client role.
CARGO_HOME=.cargo-home cargo check -p party-dungeon-replication --features server,client
CARGO_HOME=.cargo-home cargo check -p party-dungeon-sim --features replicon
CARGO_HOME=.cargo-home cargo test  -p party-dungeon-sim --lib --features replicon
CARGO_HOME=.cargo-home cargo check -p party-dungeon --features replicon
CARGO_HOME=.cargo-home cargo check -p party-dungeon --features replicon --target wasm32-unknown-unknown
```

Live checks after a deploy:

- sim log: `sim: replicon replication path (DUNGEON_REPLICON=1)` and
  `sim: replicon server (signaling …, world_radius …, player_radius …)`;
- client log: `replicon: connecting to …`, `replicon: session …` /
  `replicon: welcome sub=… world_radius=…`, and the once-a-second
  `replicon stats: rtt … sent … recv …`;
- HUD shows no message once `ClientState::Connected` + `BackendState::Connected`;
- two accounts standing together log `remote players: 1 visible [<them>]` and
  see capsules + nameplates; a world edit round-trips (piece appears for both,
  persists after a sim restart);
- `cargo check`/clippy with the features above, and a browser built with
  `--features replicon` (WASM client check) — not the default build.

## 12. File map

| Concern | File |
|---|---|
| schema: components, messages, filters, quantization | `game/crates/replication/src/lib.rs` |
| backend: handshake, channels, stats, ICE, lifecycle | `game/crates/repl_matchbox/src/{lib,client,server,transport}.rs` |
| sim server role: embedded app, mirroring, auth glue, authority loop | `game/crates/sim/src/repl/*.rs` |
| shared authority (both paths): sessions + edits | `game/crates/sim/src/authority.rs` |
| edittable conversion (quantized) | `game/crates/sim/src/repl/convert.rs` |
| client role: connect, input, world mirror, edits, avatars, stats | `game/crates/game/src/repl_client.rs` |
| editor composition (shared): stable ids, optimistic edits | `game/crates/game/src/editor/world_sync.rs` |
| avatar/nameplate visuals (shared) | `game/crates/game/src/remote_players.rs` |
| build selection: features, compose args/envs, web script | `game/crates/*/Cargo.toml`, `game/docker-compose.yml`, `game/crates/sim/Dockerfile`, `game/scripts/build-web.sh` |
