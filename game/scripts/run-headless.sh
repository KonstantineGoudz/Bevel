#!/usr/bin/env bash
# Run Party Dungeon headless: no window, no display server, but the full wgpu
# renderer still draws into an offscreen image. The live MCP bridge stays up, so
# the MCP server can drive the world and pull PNG frames from `GET /screenshot`.
#
#   ./scripts/run-headless.sh
#   ./scripts/run-headless.sh --headless-size 1920x1080 --headless-fps 30
#   ./scripts/run-headless.sh --headless-frames 300     # bounded run (CI/smoke)
#   ./scripts/run-headless.sh --headless-seconds 45     # wall-clock cap (slow GPUs)
#
# Extra arguments pass straight through to the game, and a flag wins over its
# env var. The same values can be exported instead:
#
#   PARTY_DUNGEON_HEADLESS_SIZE=1920x1080 PARTY_DUNGEON_HEADLESS_FPS=30 \
#     ./scripts/run-headless.sh
#
# Defaults: 1280x720 at 60 Hz, run until quit. `--headless=1920x1080` is
# shorthand for `--headless --headless-size 1920x1080`. `--headless-frames` and
# `--headless-seconds` are independent bounds; whichever is reached first ends
# the run.
#
# This script also defaults PARTY_DUNGEON_BRIDGE=1 (the MCP server needs it) and
# DUNGEON_AUTH_REQUIRED=0 (there is no keyboard to log in with in headless
# mode) -- but only when they are unset, so an explicit value in your
# environment still wins.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -z "${PARTY_DUNGEON_BRIDGE+x}" ]; then
  export PARTY_DUNGEON_BRIDGE=1
fi
if [ -z "${DUNGEON_AUTH_REQUIRED+x}" ]; then
  export DUNGEON_AUTH_REQUIRED=0
fi

exec cargo run -p party-dungeon -- --headless "$@"
