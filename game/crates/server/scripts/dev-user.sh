#!/bin/sh
# Create a local dev login (RECORD access) for manual testing without Firebase.
# Run from game/crates/server. Needs the stack up + schema bootstrapped.
#
#   ./scripts/dev-user.sh alice s3cret
set -eu
cd "$(dirname "$0")/.."

if [ ! -f .env ]; then
  echo "Missing .env (copy .env.example first)." >&2
  exit 1
fi
# shellcheck disable=SC1091
. ./.env

USER="${1:?usage: dev-user.sh <name> <pass>}"
PASS="${2:?usage: dev-user.sh <name> <pass>}"
: "${SURREAL_PORT:=6000}"
: "${DUNGEON_SURREAL_NS:=party_dungeon}"
: "${DUNGEON_SURREAL_DB:=main}"

curl -s -X POST "http://127.0.0.1:${SURREAL_PORT}/signup" \
  -H 'Content-Type: application/json' \
  -d "{\"ns\":\"$DUNGEON_SURREAL_NS\",\"db\":\"$DUNGEON_SURREAL_DB\",\"ac\":\"dev_login\",\"name\":\"$USER\",\"pass\":\"$PASS\"}"
echo
