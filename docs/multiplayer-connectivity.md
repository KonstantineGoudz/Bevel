# Multiplayer Connectivity: Signaling, WebRTC, and TURN

> **Path note.** The three legs below (signaling WebRTC/TURN) are identical on
> both replication paths: the live `bevy_replicon` client
> (`crates/repl_matchbox`) keeps the same matchbox room, the same NPM routes and
> the same `DUNGEON_ICE_SERVERS`/`window.DUNGEON_ICE_SERVERS` TURN config. What
> differs is the HUD wording in §1 and the browser-less probes in §4: those are
> written for the legacy `net_client` path. On the replicon path the HUD is
> driven by the backend's `BackendStatus` (`WaitingForHost` / `Handshaking` /
> `Failed` / `Connected`, `relayed=` logged on transitions) and
> `wire_probe`/`world_edit_probe` do not apply. For the replication-path
> operations, verification and cutover see
> [replicon-migration.md](replicon-migration.md).

Why two players who both see "signaling is reachable" can still fail to meet,
what each corner-HUD message actually means, and how to configure a TURN relay
so a phone on mobile data can reach the sim.

This is the operator + debugging reference for `game/crates/game/src/net_client.rs`
(transport lifecycle and HUD), `game/crates/sim/src/net.rs` (the host socket),
and `party_dungeon_net::{parse_ice_servers, flatten_ice_servers}` in
`game/crates/net/src/lib.rs` (ICE configuration).

---

## 1. The three legs

Failing to reach the world is not one problem — three independent legs have to
work, and each fails differently.

```
   browser / native client                     your infrastructure
   ──────────────────────                      ────────────────────
   1. signaling WebSocket  ──── wss://…/party-dungeon ──►  NPM (TCP 443) ──► sim :6536
        (join the matchbox room; exchange SDP offers/candidates)

   2. WebRTC data channel  ──── direct, or via STUN reflexive address ───►  sim host
        (the actual game traffic: reliable ClientMsg/ServerMsg + unreliable Snapshot)

   3. TURN relay (fallback) ──── client ↔ TURN server ↔ sim host
        (when NAT/firewall blocks 2; allocated by the ICE config)
```

**Leg 1 — signaling WebSocket.** Always required and always first. It carries
the matchbox room handshake (`Hello`/`Welcome` are *application* messages on the
WebRTC channel, not on this socket) and the SDP exchange. It is ordinary HTTPS:
`wss://play.conman.biz/party-dungeon` proxied by NPM to the sim's `:6536`. If
this leg fails you cannot even see the sim's WebRTC offer. Failure is almost
always proxy/DNS/TLS/UDP-unrelated (it is TCP 443), so it is the one leg that
"just works" in normal deployments — if it doesn't, the problem is the reverse
proxy or a cold sim container.

**Leg 2 — the WebRTC data channel.** Once signaling has exchanged the offers,
the two peers try to connect *directly* over UDP (or over the NAT mappings they
learned from a STUN server). This is the leg that fails on a phone using mobile
data: carrier-grade NAT (CGNAT) and strict corporate/guest firewalls drop or
rewrite inbound UDP, so neither peer can reach the other's address. The
symptom is brutal because signaling is healthy: the host's offer arrives, the
WebSocket is open, an id is assigned — and the peer connection just never
completes. Nothing is logged as an error by the transport; the client sits in
"waiting for the world server".

**Leg 3 — TURN relay.** A TURN server is a public relay: the client allocates an
address on it and sends its media through it; the TURN server forwards to the
sim. This is the standard fix for CGNAT/mobile-data players (and for
"both ends behind NAT"). It is a *fallback*: when ICE finds a direct path the
TURN server is never used, so adding it costs nothing on healthy networks.

### Which HUD message means which leg

The client's `SignalState` (`net_client.rs`) maps onto the legs like this. The
messages below quote the user-visible strings from `NetStatus::hud()` (`…`
abridges the endpoint/attempt details):

| HUD state (enum) | Visible when | Message (abridged) | Meaning / next step |
|---|---|---|---|
| `Failed` (red) | signaling socket errors/closes | "Can't reach the world server at … (is the reverse proxy routing it?)" | **Leg 1**. NPM route, TLS, WebSocket upgrade, sim container down. Not a NAT problem. |
| `Connecting` (grey) | no signaling id yet | "Connecting to the world server at …" | Leg 1 in progress. |
| `WaitingForHost` (grey) | signaling is up, **no host peer was ever reported** | "Signaling is reachable at … but the world server has not answered yet — retrying (attempt N)" | **The sim is not in the room.** Deploy/restart the sim; check its signaling URL/room id. |
| `WaitingForHost` + `saw_peer` (red) | signaling is up, a host peer **was** reported, but the WebRTC link never completed | "Found the world server at … but the direct WebRTC link could not be established — this network likely blocks peer-to-peer UDP (mobile data or a strict NAT/firewall). Try Wi-Fi, or configure a TURN relay (see docs/multiplayer-connectivity.md). Retrying (attempt N)" | **Leg 2/3.** ICE/NAT. Try Wi-Fi, or add TURN (this document). |
| `Denied` (red) | sim answered `Hello` with `Deny` | "The world server rejected this session: … Log out and log in again." | Auth/session, not transport. |
| `Connected` | a peer connection is up | no HUD text | All legs work. |

Important details behind that table:

- The NAT case is **not a separate enum variant**. It is
  `SignalState::WaitingForHost` with `NetStatus::saw_peer = true`. `saw_peer`
  becomes true as soon as signaling reports any non-disconnected host peer
  (even while the WebRTC handshake is still pending), and it is sticky for the
  attempt: once seen it stays true until the socket is rebuilt for the next
  attempt. So a host that *leaves* a room mid-session can briefly also surface
  the red NAT message until the retry rebuilds the status.
- The two "waiting" messages both aut-retry, but for different reasons. A
  half-dead WebRTC handshake keeps the socket open and healthy-looking, so a
  watchdog re-joins the room after `HANDSHAKE_TIMEOUT_SECS` (12 s), giving the
  host a fresh offer, up to `SIGNAL_MAX_ATTEMPTS` (12) attempts. On the NAT case
  retrying the same network will not help — it needs Wi-Fi or TURN.
- The ICE configuration is read when the WebRTC socket is built (per signaling
  attempt). Changing the network or the relay config therefore needs a **page
  reload** in the browser (the page injects the config at boot) or a client
  restart natively; a plain retry will not pick up a new `window.DUNGEON_ICE_SERVERS`.
- While signed out, the login overlay owns the screen: `Connecting` and
  `WaitingForHost` (both variants) stay hidden, but `Failed` and `Denied` are
  shown behind/above it because a broken route is exactly what makes logging in
  then "Connecting…" forever look like an auth bug.

---

## 2. Configuration

All three environments accept the **same JSON list**, in the browser
`RTCConfiguration` shape. In the browser it must be a **JSON string**, not a JS
array/object (the client reads it through `web_global()` → `as_string()`).

### Where the list lives

| Environment | Variable | Read at |
|---|---|---|
| sim host | `DUNGEON_ICE_SERVERS` (environment) | sim startup (`host_socket` → `configured_ice_server`) |
| native client | `DUNGEON_ICE_SERVERS` (environment) | each signaling attempt (`connect_signaling`) |
| browser/WASM | `window.DUNGEON_ICE_SERVERS` (string) | each signaling attempt |

The browser value is injected by `game/web/game.html` before boot; the sim and
native client read their process environment. Best practice: put the same JSON
string in all three (and in your `.env` for the sim, since `game/docker-compose.yml`
passes `env_file: crates/server/.env` to the sim container).

```sh
# crates/server/.env  (also copied to game/.env for compose interpolation)
# One line: keep it free of `#` (comment marker) and of `$` in the credential
# (compose interpolation); verify what the sim actually received with:
#   docker compose exec sim printenv DUNGEON_ICE_SERVERS
DUNGEON_ICE_SERVERS=[{"urls":["stun:stun.l.google.com:19302","stun:stun1.l.google.com:19302"]},{"urls":["turn:turn.example.com:3478?transport=udp","turn:turn.example.com:3478?transport=tcp"],"username":"dungeon","credential":"CHANGE_ME"}]
```

```js
// game/web/game.html — inside the <script type="module"> block that sets the
// other window.DUNGEON_* globals, either before the block or as the fallback:
window.DUNGEON_ICE_SERVERS =
  iceOverride || window.DUNGEON_ICE_SERVERS ||
  '[{"urls":["stun:stun.l.google.com:19302","stun:stun1.l.google.com:19302"]},{"urls":["turn:turn.example.com:3478?transport=udp","turn:turn.example.com:3478?transport=tcp"],"username":"dungeon","credential":"CHANGE_ME"}]';
```

For quick debugging without a web rebuild, pass the same JSON URL-encoded in the
page URL (it overrides the window global):

```
https://play.conman.biz/game.html?ice=%5B%7B%22urls%22%3A%5B%22stun%3Astun.l.google.com%3A19302%22%5D%7D%2C%7B%22urls%22%3A%5B%22turn%3Aturn.example.com%3A3478%3Ftransport%3Dudp%22%5D%2C%22username%22%3A%22dungeon%22%2C%22credential%22%3A%22CHANGE_ME%22%7D%5D
```

Native client equivalent for a one-off run:

```sh
DUNGEON_ICE_SERVERS='[{"urls":["stun:stun.l.google.com:19302"]},{"urls":["turn:turn.example.com:3478?transport=udp"],"username":"dungeon","credential":"CHANGE_ME"}]' \
  cargo run -p party-dungeon
```

### JSON shape and validation rules

```json
[
  { "urls": ["stun:stun.l.google.com:19302", "stun:stun1.l.google.com:19302"] },
  {
    "urls": [
      "turn:turn.example.com:3478?transport=udp",
      "turn:turn.example.com:3478?transport=tcp"
    ],
    "username": "dungeon",
    "credential": "CHANGE_ME"
  }
]
```

- Every entry has a **`urls` array** (arrays only — the browser's own
  `RTCIceServer` also permits a bare string, but this parser does not).
  `username`/`credential` are optional; `null` is accepted.
- Allowed URL schemes: `stun:`, `stuns:`, `turn:`, `turns:`.
  `turn:`/`turns:` **require both** `username` and `credential`.
- URLs are trimmed; comparison for dedup is exact.
- **Blank/unset** means "no configured ICE servers" — the transport keeps
  matchbox's built-in public STUN defaults.
  **Malformed** means background defaults **plus a loud log line**, never a
  silent half-working config:
  - sim: `sim: ignoring DUNGEON_ICE_SERVERS: <error>` (still starts, STUN-only)
  - client: `ignoring ICE server config: <error>` (WARN; STUN-only)
  - a successful parse logs the count: `sim: ICE: N configured url(s) (TURN credentials set)`
    / `ICE: N configured server url(s) (TURN credentials set)`.
- **Setting the variable replaces the built-in STUN defaults** (matchbox's
  `WebRtcSocketBuilder::ice_server` swaps the whole config —
  `matchbox_socket::RtcIceServerConfig`). Include the `stun:` URLs you still
  want. If you only list TURN URLs, drop the STUN entries at your own risk:
  STUN is what finds the direct path.
- **One credential pair per deployment.** `matchbox_socket` carries a single
  `(urls, username, credential)` triple, so the parser merges every `urls`
  array (in order, deduplicated) and takes the credentials from the first entry
  that has both a username and a credential (falling back to the first entry
  with a username). Do not mix relay accounts: one TURN server (or one account)
  per list.
- **Both ends.** Give the sim and the client the same list. In the common
  CGNAT-client case the client-side TURN allocation is what fixes connectivity,
  but configure the sim too: host-side ICE matters when the sim itself is behind
  NAT, and it keeps the deployment symmetric and debuggable.
- TURN credentials in `.env` end up passed by compose's `env_file`, and compose's
  `$` interpolation makes a `$` in the value ambiguous. Use a URL-safe random
  password (no `$`), and verify what the sim actually received with
  `docker compose exec sim printenv DUNGEON_ICE_SERVERS`.

---

## 3. Self-hosted TURN (coturn)

The relay is a separate public service; it is **not** proxied by NPM and its
traffic does not go through the signaling WebSocket. Do not edit
`game/docker-compose.yml` for this — add a small override file (or a separate
compose project) so the core stack stays clean:

```yaml
# game/docker-compose.turn.yml — used as an override:
#   docker compose -f docker-compose.yml -f docker-compose.turn.yml up -d
# (or run coturn as a standalone compose project on the relay host)
services:
  turn:
    image: coturn/coturn:4.6.2
    restart: unless-stopped
    # On Linux, host networking is by far the simplest: coturn needs 3478 and a
    # RANGE of UDP relay ports, which is painful (and flaky on Docker Desktop)
    # to publish through the default bridge network.
    network_mode: host
    command:
      - --listening-port=3478
      - --tls-listening-port=5349
      - --realm=party-dungeon
      - --fingerprint
      - --lt-cred-mech
      - --user=dungeon:CHANGE_ME
      # Narrow the relay range so the router rule is a single UDP range:
      - --min-port=49160
      - --max-port=49200
      - --no-cli
      - --no-multicast-peers
      # Only when the TURN host itself sits behind 1:1 NAT:
      # - --external-ip=203.0.113.10
      # TLS (needed only for turns://):
      # - --cert=/etc/coturn/fullchain.pem
      # - --pkey=/etc/coturn/privkey.pem
    volumes:
      - ./turn/certs:/etc/coturn:ro
```

`turnserver.conf` notes (if you prefer a config file over the long `command`):
the same settings have config names — `listening-port=3478`,
`tls-listening-port=5349`, `realm=…`, `fingerprint`, `lt-cred-mech`,
`user=dungeon:CHANGE_ME`, `min-port=…`, `max-port=…`, `no-cli`,
`no-multicast-peers`, `external-ip=…`, `cert=…`, `pkey=…` — then mount the file
at `/etc/coturn/turnserver.conf`.

**Static user vs `--use-auth-secret`.** This deployment configures static JSON
in environment variables, so the **static user** variant above is the right
default: the credentials never change.

The TURN REST / time-limited variant is available if you plan to mint
credentials dynamically:

```yaml
    command:
      - --listening-port=3478
      - --tls-listening-port=5349
      - --realm=party-dungeon
      - --fingerprint
      - --lt-cred-mech
      - --use-auth-secret
      - --static-auth-secret=CHANGE_ME
      - --min-port=49160
      - --max-port=49200
      - --no-cli
      - --no-multicast-peers
```

With `--use-auth-secret` the ICE `username` is an expiry timestamp and the
`credential` is an HMAC (TURN REST API):

```sh
username=$(date -d '+1 day' +%s)
credential=$(printf %s "$username" | openssl dgst -sha1 -hmac "$TURN_SECRET" -binary | base64)
# → {"urls": ["turn:turn.example.com:3478"], "username": "'$username'", "credential": "'$credential'"}
```

The credential **expires**, so it cannot live in a static `.env`/`window` value
forever — you must regenerate and redeploy (or mint it per client from an
endpoint). For this repo's static-env configuration, use the static-user
variant unless you are adding a credential-minting endpoint.

**Ports and the router caveat.**

- Forward to the TURN host: **3478/udp**, **3478/tcp** (TURN/TCP fallback),
  **5349/tcp** and optionally **5349/udp** (TLS/DTLS for `turns:`), and the UDP
  relay range **49160–49200/udp**.
- The relay range is the one people forget: without it, allocations are made
  but no media flows. That is why `--min-port`/`--max-port` is narrowed to a
  single range you can add as one port-forward rule.
- No router/admin access to forward UDP? Use a **hosted TURN** provider instead
  (below) — it removes this requirement entirely.
- `--external-ip=<public-ip>` is required only when the relay host is behind a
  1:1 NAT (cloud VMs with a floating IP usually need it; a host with the public
  IP on its interface does not). Docker bridge networking makes this worse —
  prefer `network_mode: host`.
- `turn:` over UDP/TCP works even when the game page is served over HTTPS
  (WebRTC is not subject to mixed-content restrictions for TCP/UDP TURN).
  `turns:` (TLS, port 5349) is only needed when a firewall allows outbound TLS
  but blocks other UDP/TCP; it needs `cert`/`pkey`.
- Keep `--no-multicast-peers` and `--no-cli`; optionally add
  `--denied-peer-ip=10.0.0.0-10.255.255.255` etc. to stop the relay being used
  to reach private ranges. Use a long random static password.

**Automated forwarding with NAT-PMP.** If the router speaks NAT-PMP
(RFC 6886 — most consumer routers do; some ship it disabled), you do not need
to add the forwards by hand: `game/scripts/natpmp-turn.py` asks the gateway for
exactly the mappings above and keeps them alive.

```sh
./scripts/natpmp-turn.py           # map once, print the granted table
./scripts/natpmp-turn.py --loop    # refresh forever at 2/3 of the lifetime
```

It maps `3478/udp`, `3478/tcp` and the `49160–49200/udp` relay range to the
machine running coturn (change them with `--relay-range` / `--lifetime`).
Because mappings are per-host and expire when the router reboots, run the
refresher in a `--network host` container so Docker restarts it with the host:

```sh
docker run -d --name party-dungeon-natpmp --network host --restart unless-stopped \
  -v "$PWD/scripts/natpmp-turn.py:/natpmp-turn.py:ro" \
  python:3-alpine python3 /natpmp-turn.py --loop
```

The current play deployment runs `coturn/coturn` with host networking, static
user `party`, `--external-ip=<public-ip>/<lan-ip>` and that refresher container;
the same credential pair is in the sim's `DUNGEON_ICE_SERVERS` and in
`game/web/game.html`. Rotate it in all three places (coturn command, sim env,
page) together. If NAT-PMP is disabled on the router, forward the same ranges
in its UI instead — the game config does not depend on how the ports are
opened.

### Hosted TURN (zero router changes)

If you have no control over the relay host's router/firewall — or do not want to
run one — use a hosted TURN provider (Metered, Twilio Network Traversal,
Cloudflare Calls TURN, …). They give you a `turn:` host plus a static or
time-limited credential; put those in the same JSON list, no port-forwarding
needed:

```json
[
  { "urls": ["stun:stun.l.google.com:19302"] },
  {
    "urls": ["turn:global.turn.example.net:3478?transport=udp",
             "turn:global.turn.example.net:3478?transport=tcp"],
    "username": "your-account-username",
    "credential": "your-account-credential"
  }
]
```

Pick a provider whose credentials are **static** (or mint them at page-load from
your own endpoint), because this repo puts the credential in a static env var /
`window` global. Big hosted relays are placed near users, so latency is usually
fine (§4).

---

## 4. Verification

Work through the legs in order; each test isolates one of them.

1. **Signaling (leg 1).**
   ```sh
   ./scripts/check-signal.sh https://play.conman.biz
   ```
   Expect `HTTP 101` on `/party-dungeon` and healthy auth routes. This proves
   the WebSocket upgrade through the reverse proxy — nothing about WebRTC.

2. **Signaling + host + data channel, without a browser.**
   ```sh
   cargo run -p party-dungeon-sim --example signal_probe -- ws://127.0.0.1:6536/party-dungeon
   cargo run -p party-dungeon-sim --example signal_probe -- wss://play.conman.biz/party-dungeon
   ```
   The probe joins the room and reports peer states, whether a signaling id was
   assigned, and whether a host peer connection completed:
   - `CONNECTED: the host peer is reachable` (with elapsed time) → legs 1+2 work
     from *this machine's* network.
   - `NO HOST PEER after 20s (id assigned: true) — signaling works, WebRTC does
     not` → leg 1 works, leg 2 fails from this network. Run it from the affected
     network to reproduce the NAT failure.

   The probe reads `DUNGEON_ICE_SERVERS` from its own environment, so it
   exercises the same STUN/TURN configuration as the sim and the native client
   — run it with the relay list exported to test a TURN relay from a given
   network. (`wss://` works too: the example installs an explicit rustls
   `CryptoProvider`, which the workspace needs because it links more than one.)
   With no relay list it uses the transport's built-in STUN defaults and is a
   scope check: which leg fails, from where.

3. **The streaming protocol itself, without a browser.**

   ```sh
   cargo run -p party-dungeon-sim --example wire_probe -- wss://play.conman.biz/party-dungeon wireprobe
   ```

   This joins as a player (unsigned JWT; the sim only decodes claims at the
   join gate), sends `Hello` + `SetInterest`, and prints the `Welcome`, every
   streamed `WorldChunk`, and snapshot counters. A healthy run ends with
   `OK: deployed sim streams Welcome + WorldChunk + a filtered Snapshot (self
   included)`. It creates a `player:<sub>` row — delete it after a production
   test (`DELETE player WHERE name = 'wireprobe';`). Use it after a sim deploy
   to prove the new streaming protocol is live even when no browser is
   available.

4. **Authoritative edits and remote players, without a browser.**

   ```sh
   # place a piece through the in-game edit path; prints "RID <rid>" once the
   # streamed echo contains it ("OK: place visible in the streamed world")
   cargo run -p party-dungeon-sim --example world_edit_probe -- wss://play.conman.biz/party-dungeon <sub> place
   # other modes need the persisted rid as the 4th argument:
   cargo run -p party-dungeon-sim --example world_edit_probe -- <url> <sub> observe dungeon_piece:xyz
   cargo run -p party-dungeon-sim --example world_edit_probe -- <url> <sub> update  dungeon_piece:xyz
   cargo run -p party-dungeon-sim --example world_edit_probe -- <url> <sub> delete  dungeon_piece:xyz
   ```

   The probe joins as a player, sends `ClientMsg::WorldEdit`, and waits for
   `ServerMsg::WorldEditApplied` plus the streamed `WorldChunk` echo — the
   end-to-end check for persist → apply → re-stream. Success prints
   `RID <rid>`; a rejected edit prints `WorldEditRejected key=… reason=…` and
   exits non-zero. It reads `DUNGEON_ICE_SERVERS` like the other probes, creates
   a `player:<sub>` row, and (until `delete`) leaves one piece in the store.
   To see another player from a real client, join with two accounts and watch
   the client's console: `remote players: N visible [subs]` lists the *remote*
   subs only (yourself excluded; shortened), so two accounts standing together
   log `remote players: 1 visible [<them>]`, and `remote players: 0 visible`
   when the other account leaves the interest radius. The line only changes when
   the set changes; before `Welcome` arrives the renderer holds off entirely and
   logs `remote players: identity not known yet (no Welcome) …`.

5. **Candidate pairs in the browser.** Open `chrome://webrtc-internals` in
   desktop Chrome/Edge while the game page is open, select its peer connection,
   and look at:
   - **ICE candidate pairs / selected candidate pair** — a pair with type
     `relay` on either end means TURN is in use; only `host`/`srflx` pairs plus a
     never-completing `checking`/`failed` state means TURN is missing/blocked.
   - **`iceConnectionState`** — `failed` with candidates gathered is the classic
     NAT signature; `checking` that never leaves means no reachable pair.
   - **Candidates added** — if no `typ relay` candidate appears, the client never
     got a usable relay allocation (bad TURN URL/credentials/unreachable relay).

6. **The client's own log line.** On the transition to connected the client logs
   how the link was made (native: terminal; browser: dev console — on a phone you
   need USB remote debugging via `chrome://inspect` to read it, so the HUD is the
   practical on-device signal):
   ```
   webrtc link up: local=ServerReflexive remote=Relay relayed=true
   ```
   (`remote` says how the peer is reached; `CandidateType::Relay` = TURN was
   required. `relayed=true` if either end is a relay.) Occasional fallback:
   `webrtc link up (candidate pair not reported yet)` — the transport reports
   the pair asynchronously; not an error.
   On the sim side, the host logs its ICE config at startup:
   `sim: ICE: N configured url(s) (TURN credentials set)`.

7. **Latency trade-off when relayed.** A direct path costs the network RTT
   between the two peers. A relayed path adds two hops (client↔TURN and
   TURN↔sim), typically **+20–100 ms RTT** depending on how far the TURN server
   is; TURN/TCP or `turns:` adds more (TCP head-of-line blocking). Put the relay
   geographically close to the players. The game is client-predicted, so local
   input feels the same, but remote player interpolation and server corrections
   lag by the extra hops — visible as slightly sloppier remote avatars, not as
   input lag.

---

## 5. Ops checklist — "signaling works but the world server never answers"

This is the exact incident: the browser HUD showed *"Signaling is reachable …
but the world server has not answered yet"*, the phone was on 5G, signaling and
the sim host were healthy through the public proxy, and a local probe connected
in 0.3 s. That combination means **the sim is in the room but the peer-to-peer
WebRTC data channel never completes — ICE/NAT, most likely CGNAT on mobile
data.** It is not a sim/deploy fault, though the currently deployed sim image
and web build both predate the streaming + connectivity work, so the deployed
client cannot currently tell the two "waiting" states apart — the new build
can (the red message and link log below exist only after redeploying both).

### What serves `play.conman.biz`

The public site is three independent routes through NPM on `192.168.0.172`; only
the first one is the static site, which is why "site down but login/signaling
still work" is a recognised failure shape:

| Public route | Forwards to | Serves |
|---|---|---|
| `/` (`index.html`, `game.html`, `viewer*.html`, `pkg/`, `assets/`) | `192.168.0.143:6082` | the static web build, from the durable nginx container `party-dungeon-web` (`nginx:alpine`, `-p 6082:80`, read-only mount of `game/web/`, `--restart unless-stopped`) |
| `/login`, `/signup`, `/refresh` | `192.168.0.143:6565` | the sim's auth API (`signup.rs`; proxies Rauthy) |
| `/party-dungeon` (WebSocket) | `192.168.0.143:6536` | matchbox signaling — the path is the room id, keep it |

- The site used to be `serve-web.py` on `:6082`, a dev process. When the host had
  an interruption the Python process died with it and NPM returned **502 for the
  site** while `/login` and `/party-dungeon` stayed healthy (they proxy straight
  to the sim) — that asymmetry is the tell. It is now the restart-policy nginx
  container, which survives host/session interruptions.
- NPM's admin API login is `{"identity": …, "secret": …}` (`POST /api/tokens`),
  handy for scripted checks of locations, certificates and forwarding rules.
- Deploying the web: `./scripts/build-web.sh` writes `game/web/pkg` +
  `game/web/assets`; the container's read-only mount serves them immediately
  (nginx reads from disk — no restart, restart only when nginx config/locations
  change). The checked-in HTML is served from the same mount. If you build on
  another machine, copy `game/web/{pkg,assets,*.html}` into the mounted
  directory.
- Build stamp: `BUILD` in `game/web/game.html` and `CLIENT_BUILD` in
  `crates/game/src/net_client.rs` must match (currently `2026-11-17.8`, the deployed replicon build).
  `BUILD` also cache-busts the wasm import (`?v=…`), and the HUD prints
  `CLIENT_BUILD` in each connection message — check it after a deploy to prove
  the new client is actually served (mobile caches are otherwise convincing).

1. **Confirm it is NAT, not the sim.** Load the same page on Wi-Fi. If it
   connects on Wi-Fi but not on mobile data, leg 2 is blocked on the mobile
   network. (If Wi-Fi also fails with the *grey* "has not answered yet"
   message, the sim host is not in the room — a different problem.)
2. **Prove legs 1 + host are healthy** with `./scripts/check-signal.sh
   https://play.conman.biz` and the public-URL `signal_probe` from a dev
   machine. That is exactly what was already done in this incident.
3. **Stand up a relay** — either self-host coturn (§3, needs UDP
   forwarding for 3478 and the relay range) or use a hosted TURN provider
   (zero router changes). For one stuck phone on 5G, the usual fix is a hosted
   relay or a coturn box with a public IP.
4. **Configure both ends** with the same JSON (STUN + TURN entries):
   - sim: `DUNGEON_ICE_SERVERS` in `crates/server/.env` (compose passes it via
     `env_file`), then restart the sim container so the host socket is rebuilt
     (`docker compose up -d --force-recreate sim`, or `./up.sh`).
   - browser: `window.DUNGEON_ICE_SERVERS` in `game/web/game.html`; for a quick
     test after the web build supports it, use `?ice=` instead (§2).
5. **Rebuild and redeploy BOTH sides.** Required, because the running sim image
   and the deployed web build both predate the connectivity (and streaming)
   work — the old builds ignore `DUNGEON_ICE_SERVERS`, have no `?ice=`, and
   cannot show the red NAT HUD message or the link log:
   ```sh
   cd game
   ./up.sh --build              # rebuild the sim image (sim container needs the env at startup)
   ./scripts/build-web.sh       # wasm + web/pkg + web/assets
   # on the play host the party-dungeon-web nginx mount serves these at once
   # (copy game/web/{pkg,assets,*.html} into the mount if built elsewhere);
   # confirm the HUD/build stamp matches the checked-in value (2026-11-17.8) after reloading the page
   ```
   Sanity-check the env reached the container:
   ```sh
   cd game && docker compose exec sim printenv DUNGEON_ICE_SERVERS
   docker compose logs sim | grep "ICE:"
   ```
6. **Verify end-to-end:** open the page on the failing phone/network. The HUD
   should stop at `Connected` (no message), and the browser console should show
   `webrtc link up: … relayed=true`. Repeat on Wi-Fi: the same build should
   connect with `relayed=false` (direct) or occasionally `true` (ICE chose the
   relay — also fine).
7. **If it still fails:** the relay is unreachable or misconfigured. Check, in
   order: TURN URL host/port reachable from the phone's network (a TURN test
   page or `turnutils_uclient` from an external network), credentials exactly
   matching (no stray `$` interpolation in `.env`), the UDP relay range forwarded
   to the TURN host, `external-ip` set when the relay host is behind 1:1 NAT, and
   `turns:`/5349 only if TLS certs are configured.
8. **Rollback:** unset `DUNGEON_ICE_SERVERS` on both ends (and remove
   `window.DUNGEON_ICE_SERVERS` / `?ice=`) to return to the built-in STUN
   defaults. A malformed value already falls back to STUN with a loud log line,
   so a typo cannot brick the standard path.

---

## 6. Where the code lives

| Concern | File |
|---|---|
| `SignalState`, `NetStatus::hud()`, `saw_peer`, retry watchdog, link-up log | `game/crates/game/src/net_client.rs` |
| ICE env/global read + `configured_ice_server()` (native + WASM) | `game/crates/game/src/net_client.rs` |
| host-side ICE config (`host_socket` → `configured_ice_server`) | `game/crates/sim/src/net.rs` |
| `IceServer`, `parse_ice_servers`, `flatten_ice_servers` | `game/crates/net/src/lib.rs` |
| browser globals injected before boot (`DUNGEON_SIGNAL_URL`, `…_ICE_SERVERS`, `?ice=`) | `game/web/game.html` |
| signaling/auth route smoke test | `game/scripts/check-signal.sh` |
| browser-less host reachability probe | `game/crates/sim/examples/signal_probe.rs` |
| browser-less streaming-protocol probe (`Welcome`/`WorldChunk`/`Snapshot`) | `game/crates/sim/examples/wire_probe.rs` |
| browser-less authoritative-edit probe (`WorldEdit` → `WorldEditApplied` → chunk echo) | `game/crates/sim/examples/world_edit_probe.rs` |
| remote-player presence log (`remote players: N visible [subs]`) | `game/crates/game/src/remote_players.rs` |
