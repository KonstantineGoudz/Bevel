#!/usr/bin/env bash
# Check the endpoints the game page needs, in the order it needs them.
#
# The page derives everything from its own origin, so in production it talks to
#   wss://<play host>/party-dungeon     (WebRTC signaling → sim :6536)
#   https://<play host>/login|/signup   (auth API → sim :6565)
# A plain HTTP GET can look healthy while the WebSocket upgrade is still broken,
# which is exactly the failure this script is for.
#
#   ./scripts/check-signal.sh                              # local stack
#   ./scripts/check-signal.sh https://play.conman.biz      # through the proxy
set -uo pipefail

if [ $# -ge 1 ]; then
  # Same-origin deployment: everything hangs off this origin.
  SIGNAL_URL="${1%/}/party-dungeon"
  AUTH_BASE="${1%/}"
else
  SIGNAL_URL="http://127.0.0.1:6536/party-dungeon"
  AUTH_BASE="http://127.0.0.1:6565"
fi

status=0

echo "== signaling WebSocket: $SIGNAL_URL"
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 \
  -H 'Connection: Upgrade' \
  -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  "$SIGNAL_URL" 2>/dev/null)
if [ "$code" = "101" ]; then
  echo "   OK: 101 Switching Protocols — the route reaches the sim's signaling server"
else
  echo "   FAIL: got HTTP ${code:-none}, expected 101"
  echo "         - 404/502 from the proxy: the /party-dungeon location is missing, or"
  echo "           it points at the web server (:6082) instead of the sim (:6536)"
  echo "         - 400 from nginx-proxy-manager: turn on 'Websockets Support' for it"
  echo "         - nothing locally: is the sim container up? (docker compose ps)"
  status=1
fi

# A 404 here means the proxy is wrong. The sim answers 400/415/422 for a bad
# body, which proves the request reached the sim's auth API.
for path in /login /signup; do
  echo "== auth api: ${AUTH_BASE}${path}"
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 \
    -X POST -H 'Content-Type: application/json' -d '{}' \
    "${AUTH_BASE}${path}" 2>/dev/null)
  case "$code" in
    404|502|503|405|501)
      echo "   FAIL: got HTTP ${code:-none} — the auth API is not reachable on this"
      echo "         route (expected the sim's :6565 behind the proxy; 501/404 is"
      echo "         the static web server answering instead, so the proxy location"
      echo "         is missing)"
      status=1
      ;;
    *)
      echo "   OK: HTTP ${code:-none} from the sim auth API"
      ;;
  esac
done

exit "$status"
