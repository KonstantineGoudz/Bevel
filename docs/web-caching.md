# Web build caching and build stamps

The browser client is one large wasm module (`pkg/party_dungeon_bg.wasm`):
**49.8 MB** in a `wasm-release` build and ~197 MB in `wasm-dev`. The static host
used to send neither `Cache-Control` nor compression, so browsers re-downloaded
the whole module on almost every load (the stock nginx heuristic cache lifetime
is 10 % of the file's age — minutes after a deploy). Phones on mobile data paid
200 MB per visit.

This page documents the scheme that makes a reload transfer ~nothing and only a
new build fetch a new module.

## What a load transfers

| request | before | first load now | reload now |
|---|---|---|---|
| `game.html` | 6.9 KB, heuristic revalidation | 3.6 KB gzip | revalidated (304) |
| `pkg/build.json` | — | 194 B, never cached | 194 B, never cached |
| `pkg/<stamp>/party_dungeon_bg.wasm` | 49.8 MB raw (197 MB dev) | 11.7 MB gzip | not requested at all |
| `pkg/<stamp>/party_dungeon.js` | 152 KB | 21 KB gzip | not requested at all |
| `assets/character.glb` | 7.3 MB raw | 6.1 MB gzip | revalidated (304) |

## The build stamp: one value, three consumers

`scripts/build-web.sh` generates a UTC stamp for every build
(`2026-11-17.223045Z`, overridable with `PARTY_DUNGEON_BUILD`) and exports it as
`PARTY_DUNGEON_BUILD`. That one value becomes:

1. `web/pkg/build.json` → `{"build": "<stamp>", …}` — the manifest the pages
   fetch (`cache: "no-store"`) to learn which build is live;
2. the directory name: `web/pkg/<stamp>/party_dungeon.js` + `.wasm`;
3. `net_client::CLIENT_BUILD` inside the wasm, via
   `game/crates/game/build.rs` (`cargo:rustc-env`). The HUD prints it in every
   connection message, so a screenshot names the build the browser is running.

Consequences:

- A rebuild always produces a new stamp → new URLs → the browser fetches the new
  module once. Nobody has to remember to bump a constant (the old scheme had a
  hand-kept pair: `CLIENT_BUILD` in Rust and `BUILD` in `game.html`; a missed
  bump under immutable caching would have served a stale wasm forever).
- `build.json` is written *after* the new directory is complete, via
  write-then-rename. It is the atomic pointer: a page that reads it mid-deploy
  keeps the previous build (still on disk), and no page ever sees a half-written
  build.
- JS glue and wasm are always from the same stamp directory, so an old
  `party_dungeon.js` can never be paired with a new `party_dungeon_bg.wasm`
  (the failure mode the old "no-store everywhere" dev server was guarding
  against).
- Native/dev builds have no stamp and report `CLIENT_BUILD == "dev"`.

## Layout and cache policy (`web/nginx.conf`)

| path | policy | why |
|---|---|---|
| `pkg/build.json` | `no-store` | it is the live pointer; caching it would pin a build |
| `pkg/<stamp>/…` | `public, max-age=31536000, immutable` | content of one build under its own path |
| `pkg/<missing>` | `no-store` 404 | never let a browser pin a 404 for a year (`error_page` in the config) |
| `*.html` | `no-cache` | revalidate on navigation → a deploy is visible immediately (304, cheap) |
| `assets/…` | `no-cache` | fixed paths compiled into the wasm; ETag revalidation, 304 is ~200 B |
| `nginx.conf` | `deny all` | it lives in the served tree but is not part of the site |

Compression is precomputed by the build (`gzip -9 -n` next to the wasm, the JS
and the big `.glb`s) and served by nginx `gzip_static`. The wasm is deliberately
**not** in `gzip_types`: 200 MB compressed on the fly would occupy a worker per
request, and the build always writes the `.gz`. First load is ~11.7 MB instead
of ~49.8 MB (release build) — the browser decompresses it locally.

## Deploying

```sh
cd game
./scripts/build-web.sh --release --features replicon   # writes pkg/<stamp>/ + build.json, all in web/
# the durable nginx container serves web/ from disk: the build is live at once
```

Only nginx *config* changes need the container recreated:

```sh
./scripts/deploy-web.sh            # validates nginx.conf, swaps the container, smoke-tests
```

`deploy-web.sh` mounts `web/nginx.conf` at `/etc/nginx/conf.d/default.conf` and
`web/` at `/usr/share/nginx/html` (read-only), exactly like the documented
`docker run` in `AGENTS.md`.

Build elsewhere with `PARTY_DUNGEON_WEB_OUT=/tmp/web-stage ./scripts/build-web.sh …`,
then copy `pkg/` and `assets/` into the mounted tree.

## Verifying a deployment

```sh
# Which build is live, and is the manifest uncached?
curl -sS https://play.conman.biz/pkg/build.json

# The module: 200, gzip, immutable, one-year lifetime.
curl -sSI -H 'Accept-Encoding: gzip' \
  "https://play.conman.biz/pkg/$(curl -sS https://play.conman.biz/pkg/build.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["build"])')/party_dungeon_bg.wasm"

# Integrity: what the browser gets is what is on disk.
curl -sS --compressed "https://play.conman.biz/pkg/<stamp>/party_dungeon_bg.wasm" \
  | sha256sum; sha256sum game/web/pkg/<stamp>/party_dungeon_bg.wasm

# Repeat-load: an asset revalidates for free.
ETAG=$(curl -sSI -H 'Accept-Encoding: gzip' https://play.conman.biz/assets/character.glb | grep -i etag | cut -d' ' -f2 | tr -d '\r')
curl -sS -o /dev/null -w '%{http_code} %{size_download}\n' -H "If-None-Match: $ETAG" \
  https://play.conman.biz/assets/character.glb    # 304 0
```

In the browser: reload with devtools open — the wasm and JS requests show as
`(memory cache)`/`(disk cache)` (they are not re-requested at all), and the only
traffic is `game.html` revalidating (304) plus the 194-byte `build.json`, with
the assets revalidating as 304s. A new deploy changes `build.json`, and the next
reload fetches the new stamp directory once. The HUD's `client build <stamp>`
line should equal `build.json`'s `build`.

## Rollback

Old builds are kept (`PARTY_DUNGEON_KEEP_BUILDS`, default **2**: the live build
plus the previous one, pruned by **directory mtime** — not by name, because a
pinned/legacy stamp such as `2026-11-17.8` does not sort chronologically against
auto stamps like `2026-11-17.173925Z`). Normal builds never rewrite an existing
build directory; only a deliberate re-deploy of a pinned, *non-live* stamp
replaces one. Each build directory carries its own `build.json` (profile,
features, sizes), so rolling back is copying the pointer:

```sh
cp web/pkg/2026-11-17.173925Z/build.json web/pkg/build.json.tmp \
  && mv web/pkg/build.json.tmp web/pkg/build.json
```

Browsers holding the rolled-back-to build already have it cached, so a rollback
is instant for them too. Do not re-deploy a *live* stamp: its URLs are cached as
immutable, so the bytes behind them must never change.

## Knobs (`scripts/build-web.sh`)

| variable | default | meaning |
|---|---|---|
| `PARTY_DUNGEON_FEATURES` | — | features, e.g. `replicon` (same as `--features`) |
| `PARTY_DUNGEON_BUILD` | UTC timestamp | pin the stamp (reproducible / re-deploy an existing build) |
| `PARTY_DUNGEON_WEB_OUT` | `web` | served tree to write (stage a build elsewhere) |
| `PARTY_DUNGEON_KEEP_BUILDS` | `2` | how many stamp directories to keep |

## Dev server

`serve-web.py` still sends `no-store` everywhere. With the versioned layout that
is no longer required for correctness, but it keeps day-to-day iteration from
ever being blocked by a cached artifact; the stamp and manifest work the same,
so `http://localhost:6082/game.html` exercises the same code path.

## Gotchas

- **File modes matter.** The nginx workers run as an unprivileged user; a page
  or asset with mode `0600` returns **403** (this happened to
  `viewer-bevy.html`). Files that are meant to be served must be world-readable:
  `chmod 644 game/web/*.html game/web/nginx.conf`.
- Do not hand-edit `web/pkg/` or `web/assets/`: they are build outputs.
- Keep the `nginx.conf` mount when recreating the container, or the cache policy
  silently disappears (the site still works, just re-downloads the wasm).
- `pkg/<stamp>` directories are pruned newest-first; a pinned older
  `PARTY_DUNGEON_BUILD` is protected from pruning, but anything beyond
  `KEEP_BUILDS` is deleted.
