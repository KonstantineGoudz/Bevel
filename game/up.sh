#!/usr/bin/env bash
# Full stack bring-up (run from bevy/game):
#   ./up.sh            # fast: reuse the existing sim image
#   ./up.sh --build    # rebuild the sim image (after sim/game/server source changes)
#
# Does: infra up -> fix rauthy-data ownership (Rauthy runs as 10001:10001) ->
# full stack -> wait for SurrealDB -> probe SurrealDB, Rauthy, and the sim's
# auth API. Safe to re-run.
set -u
cd "$(dirname "$0")"

COMPOSE="docker compose"
if ! docker info >/dev/null 2>&1; then
  # The daemon needs root here; fall back to sudo and keep using it throughout.
  COMPOSE="sudo docker compose"
fi

say() { printf '\n--- %s ---\n' "$1"; }

say "1. start infra (surrealdb + rauthy; creates volumes)"
$COMPOSE up -d surrealdb rauthy

say "2. fix rauthy-data ownership + restart rauthy"
VOL=/var/lib/docker/volumes/party-dungeon_rauthy-data/_data
if [ -d "$VOL" ]; then
  # Rauthy runs as uid 10001; a root-owned volume makes it crash with
  # "WAL: IOError: Permission denied".
  if ! chown -R 10001:10001 "$VOL" 2>/dev/null; then
    echo "  (need root for chown — run: sudo chown -R 10001:10001 $VOL)"
  fi
  stat -c '  %U:%G %a %n' "$VOL" 2>/dev/null || true
fi
$COMPOSE restart rauthy

say "3. full stack ($([ "${1:-}" = "--build" ] && echo "rebuilding sim" || echo "no rebuild"))"
if [ "${1:-}" = "--build" ]; then
  $COMPOSE up --build -d
else
  $COMPOSE up -d --no-build
fi

say "4. wait for SurrealDB (max 90s)"
S=unknown
for _ in $(seq 1 18); do
  S=$(docker inspect -f '{{.State.Health.Status}}' party-dungeon-surrealdb-1 2>/dev/null || echo unknown)
  [ "$S" = healthy ] && break
  sleep 5
done
echo "  surreal health: $S"

say "5. probes"
curl -s --max-time 10 http://localhost:6080/auth/v1/health || echo "  rauthy: no response"
echo
curl -s --max-time 10 -o /dev/null -w '  auth api /login: HTTP %{http_code}\n' \
  -X POST http://localhost:6565/login -H 'Content-Type: application/json' -d '{}'
$COMPOSE exec -T surrealdb /surreal is-ready -e http://localhost:6000

say "6. status"
$COMPOSE ps

cat <<'EOF'

Notes
- Rauthy "WAL: Locked" after an unclean shutdown: one-off recovery, then remove it
    HQL_WAL_IGNORE_LOCK=true $COMPOSE up -d rauthy
- NPM must route, on play.conman.biz -> this host:
    /signup  /login  /refresh  -> :6565   (sim auth API)
    /party-dungeon             -> :6536   (WebRTC signaling; keep the path)
EOF
