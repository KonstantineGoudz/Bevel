#!/bin/sh
# Export a timestamped SurrealQL backup of the live database.
# Runs `surreal export` *inside* the compose container, so no host binary
# or credential plumbing is needed. Run from game/crates/server.
#
# Nightly example (host cron):
#   0 3 * * * cd /srv/party-dungeon/game/crates/server && ./scripts/backup.sh
set -eu
cd "$(dirname "$0")/.."

if [ ! -f .env ]; then
  echo "Missing .env (copy .env.example first)." >&2
  exit 1
fi
# shellcheck disable=SC1091
. ./.env

: "${SURREAL_ROOT_USER:=root}"
: "${SURREAL_ROOT_PASS:?set SURREAL_ROOT_PASS in .env}"
: "${DUNGEON_SURREAL_NS:=party_dungeon}"
: "${DUNGEON_SURREAL_DB:=main}"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
FILE="/backups/${DUNGEON_SURREAL_DB}-${STAMP}.surql"

docker compose exec -T surrealdb surreal export \
  -e http://localhost:6000 \
  -u "$SURREAL_ROOT_USER" -p "$SURREAL_ROOT_PASS" \
  --auth-level root \
  --namespace "$DUNGEON_SURREAL_NS" \
  --database "$DUNGEON_SURREAL_DB" \
  "$FILE"

echo "Wrote backups/${DUNGEON_SURREAL_DB}-${STAMP}.surql"
echo "Restore with: surreal import -e <endpoint> --ns $DUNGEON_SURREAL_NS --db <target> backups/<file>"
