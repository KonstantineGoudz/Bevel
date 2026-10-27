#!/usr/bin/env bash
# Serve the web build (game/web) on http://localhost:8082.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "Serving web/ on http://localhost:8082/"
exec python3 -m http.server 8082 -d web
