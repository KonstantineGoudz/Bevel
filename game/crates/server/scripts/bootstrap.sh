#!/bin/sh
# Bootstrap a fresh SurrealDB for Party Dungeon (see scripts/bootstrap.py).
# Run from game/crates/server.
set -eu
cd "$(dirname "$0")/.."
exec python3 scripts/bootstrap.py
