#!/usr/bin/env bash
# Self-contained smoke test for Party Dungeon's headless GPU mode.
#
# It builds `target/debug/party-dungeon` if needed, then runs a bounded headless
# session (no window, full wgpu renderer into an offscreen image), waits for the
# live MCP bridge, asserts `GET /health` reports headless, pulls a frame from
# `GET /screenshot`, checks the PNG magic bytes and frame dimensions, and
# requires the bounded run to exit 0. Logs land in a temp dir removed on exit.
#
#   ./scripts/check-headless.sh                 # 180 frames at 640x360, max 45 s
#   ./scripts/check-headless.sh 600             # more frames (slower)
#   ./scripts/check-headless.sh 600 1280x720    # FRAMES and SIZE
#   FRAMES=600 SIZE=1280x720 MAX_SECONDS=120 ./scripts/check-headless.sh
#
# The bounded run ends at whichever comes first: the frame budget (a real GPU
# reaches it quickly) or the wall-clock cap `MAX_SECONDS` (default 45 s), so a
# software renderer that only manages a few fps still finishes in bounded time.
#
# PARTY_DUNGEON_BRIDGE selects the bridge address (unset or `1` means the
# default 127.0.0.1:6559; `0` or empty disables it, which this test needs).
# Headless mode itself needs no display server.
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
  cat <<'EOF'
usage: ./scripts/check-headless.sh [FRAMES] [SIZE]

  FRAMES      rendered frames before the bounded run exits (default 180, env FRAMES)
  SIZE        offscreen viewport WxH (default 640x360, env SIZE)
  MAX_SECONDS wall-clock cap in seconds (default 45, env MAX_SECONDS)

examples:
  ./scripts/check-headless.sh
  ./scripts/check-headless.sh 600 1280x720
  FRAMES=600 SIZE=1280x720 MAX_SECONDS=120 ./scripts/check-headless.sh
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi
if [ $# -gt 2 ]; then
  usage >&2
  echo "error: at most two arguments (FRAMES, SIZE)" >&2
  exit 2
fi

FRAMES="${1:-${FRAMES:-180}}"
SIZE="${2:-${SIZE:-640x360}}"
MAX_SECONDS="${MAX_SECONDS:-45}"

case "$FRAMES" in
  ''|*[!0-9]*)
    echo "error: FRAMES must be a positive integer, got '$FRAMES'" >&2
    exit 2
    ;;
esac
if [ "$FRAMES" -le 0 ]; then
  echo "error: FRAMES must be greater than zero, got '$FRAMES'" >&2
  exit 2
fi
case "$MAX_SECONDS" in
  ''|*[!0-9]*)
    echo "error: MAX_SECONDS must be a positive integer, got '$MAX_SECONDS'" >&2
    exit 2
    ;;
esac
if [ "$MAX_SECONDS" -le 0 ]; then
  echo "error: MAX_SECONDS must be greater than zero, got '$MAX_SECONDS'" >&2
  exit 2
fi
if ! [[ "$SIZE" =~ ^[0-9]+[xX][0-9]+$ ]]; then
  echo "error: SIZE must look like 640x360, got '$SIZE'" >&2
  exit 2
fi

# Resolve the bridge address the same way the game does: unset or `1` -> the
# default loopback address, `0`/empty -> disabled, anything else -> that address.
BRIDGE="${PARTY_DUNGEON_BRIDGE:-1}"
case "$BRIDGE" in
  0|"")
    echo "error: PARTY_DUNGEON_BRIDGE='$BRIDGE' disables the MCP bridge;" >&2
    echo "       this smoke test needs it to reach /health and /screenshot." >&2
    exit 1
    ;;
  1) ADDR="127.0.0.1:6559" ;;
  *) ADDR="$BRIDGE" ;;
esac

BIN="target/debug/party-dungeon"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/party-dungeon-headless.XXXXXX")
pid=""

cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
  echo "FAIL: $*" >&2
  if [ -s "$tmp/game.log" ]; then
    echo "--- last 40 lines of the game log ---" >&2
    tail -n 40 "$tmp/game.log" >&2
  fi
  exit 1
}

# A background child is "running" until it is reaped; treat a zombie as done so
# the wait loop below cannot spin on an exited process.
is_running() {
  local stat
  stat=$(ps -o stat= -p "$1" 2>/dev/null | tr -d '[:space:]') || return 1
  case "$stat" in
    '') return 1 ;;
    Z*) return 1 ;;
  esac
  return 0
}

if [ ! -x "$BIN" ]; then
  echo "== building $BIN (CARGO_HOME=.cargo-home cargo build -p party-dungeon)"
  if ! CARGO_HOME=.cargo-home cargo build -p party-dungeon >"$tmp/build.log" 2>&1; then
    echo "--- last 40 lines of the build log ---" >&2
    tail -n 40 "$tmp/build.log" >&2
    fail "cargo build failed"
  fi
fi
if [ ! -x "$BIN" ]; then
  fail "build finished but $BIN is still missing"
fi

if [ "$FRAMES" -lt 60 ]; then
  echo "note: FRAMES=$FRAMES is under a second at 60 Hz; the run may exit before" >&2
  echo "      /screenshot can be served, so keep it >= 60 if this fails." >&2
fi

echo "== launching headless: $SIZE @ 60 Hz, $FRAMES frames / ${MAX_SECONDS}s cap, bridge $ADDR"
DUNGEON_AUTH_REQUIRED="${DUNGEON_AUTH_REQUIRED:-0}" \
  "$BIN" --headless --headless-size "$SIZE" --headless-fps 60 \
  --headless-frames "$FRAMES" --headless-seconds "$MAX_SECONDS" >"$tmp/game.log" 2>&1 &
pid=$!

# --- 1. wait for the live MCP bridge -----------------------------------------
echo "== waiting for GET http://$ADDR/health (up to 20 s)"
health=""
for _ in $(seq 1 40); do
  if health=$(curl -fsS --max-time 2 "http://$ADDR/health" 2>/dev/null); then
    break
  fi
  health=""
  if ! is_running "$pid"; then
    wait "$pid" || true
    pid=""
    fail "the game exited before the bridge answered on $ADDR"
  fi
  sleep 0.5
done
if [ -z "$health" ]; then
  fail "no /health response from $ADDR within 20 s"
fi

# --- 2. health must report headless mode -------------------------------------
case "$health" in
  *'"headless":true'*|*'"headless": true'*) ;;
  *) fail "health JSON does not report headless: $health" ;;
esac
echo "   OK: $health"

# --- 3. the bridge must serve a PNG frame ------------------------------------
echo "== GET http://$ADDR/screenshot"
if ! curl -fsS --max-time 20 "http://$ADDR/screenshot" -o "$tmp/frame.png"; then
  fail "GET /screenshot failed"
fi
if [ ! -s "$tmp/frame.png" ]; then
  fail "screenshot is empty"
fi

sig=$(od -An -tx1 -N8 "$tmp/frame.png" | tr -d ' \n')
if [ "$sig" != "89504e470d0a1a0a" ]; then
  fail "screenshot is not a PNG (first 8 bytes: ${sig:-none})"
fi

bytes=$(wc -c <"$tmp/frame.png" | tr -d '[:space:]')
dims=$(od -An -tu1 -j16 -N8 "$tmp/frame.png" |
  awk '{printf "%dx%d", $1 * 16777216 + $2 * 65536 + $3 * 256 + $4, $5 * 16777216 + $6 * 65536 + $7 * 256 + $8}')

echo "   OK: PNG frame ${dims}, ${bytes} bytes"

# --- 4. the bounded run must finish cleanly ----------------------------------
# The game stops at the first bound (frames or wall clock); give it startup
# slack on top of the wall-clock cap.
wait_secs=$((MAX_SECONDS + 30))
echo "== waiting for the bounded run to exit 0 (up to ${wait_secs}s)"
deadline=$((SECONDS + wait_secs))
while is_running "$pid" && [ "$SECONDS" -lt "$deadline" ]; do
  sleep 0.2
done
if is_running "$pid"; then
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pid=""
  fail "the game was still running after ${wait_secs}s (expected to stop after $FRAMES frames or ${MAX_SECONDS}s)"
fi

status=0
wait "$pid" || status=$?
pid=""
if [ "$status" -ne 0 ]; then
  fail "the game exited with status $status"
fi

if [ "$dims" != "$(printf '%s' "$SIZE" | tr '[:upper:]' '[:lower:]')" ]; then
  echo "note: frame is ${dims} but SIZE=$SIZE was requested" >&2
fi

echo
echo "PASS: headless GPU mode works"
echo "  mode:     --headless $SIZE @ 60 Hz, bounded to $FRAMES frames / ${MAX_SECONDS}s"
echo "  bridge:   http://$ADDR (health: headless=true)"
echo "  frame:    ${dims} PNG, ${bytes} bytes"
echo "  exit:     0"
