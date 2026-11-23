#!/usr/bin/env bash
# (Re)create the production static web container with the cache policy in
# `web/nginx.conf`.
#
# The site is a durable nginx container serving `game/web/` from disk (not the
# dev server), so a new build in `web/` is live without a restart. Only a
# *config* change needs the container recreated — this script does that safely:
# it validates the config in a throwaway container, swaps the live container,
# and smoke-tests that the page and the build manifest answer.
#
# Usage:
#   ./scripts/deploy-web.sh
#   PARTY_DUNGEON_WEB_PORT=6083 ./scripts/deploy-web.sh   # staging instance
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="${PARTY_DUNGEON_WEB_CONTAINER:-party-dungeon-web}"
PORT="${PARTY_DUNGEON_WEB_PORT:-6082}"
IMAGE="${PARTY_DUNGEON_WEB_IMAGE:-nginx:alpine}"
ROOT="$PWD/web"
CONF="$ROOT/nginx.conf"

[[ -f "$CONF" ]] || { echo "missing $CONF" >&2; exit 1; }
[[ -f "$ROOT/game.html" ]] || { echo "missing $ROOT/game.html" >&2; exit 1; }

echo "== validating $CONF"
# Parse the config before touching the live container: a broken directive would
# otherwise take the site down on restart.
docker run --rm -v "$CONF:/etc/nginx/conf.d/default.conf:ro" "$IMAGE" nginx -t

echo "== (re)starting $NAME on port $PORT"
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
  -p "$PORT:80" \
  -v "$ROOT:/usr/share/nginx/html:ro" \
  -v "$CONF:/etc/nginx/conf.d/default.conf:ro" \
  --restart unless-stopped \
  "$IMAGE" >/dev/null

# Smoke test: wait for the page, then require the build manifest (the thing that
# makes a deploy visible and the pkg directory cacheable).
for _ in $(seq 1 50); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/game.html"; then
    break
  fi
  sleep 0.2
done
curl -fsS -o /dev/null "http://127.0.0.1:$PORT/game.html"
echo "== live build manifest:"
if ! curl -fsS "http://127.0.0.1:$PORT/pkg/build.json"; then
  echo
  echo "warning: $ROOT/pkg/build.json is missing — run ./scripts/build-web.sh" >&2
fi
echo
echo "Serving $ROOT on http://localhost:$PORT/ (container $NAME)"
