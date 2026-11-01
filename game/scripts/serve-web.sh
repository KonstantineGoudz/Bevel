#!/usr/bin/env bash
# Serve the web build (game/web) on http://localhost:8082 with caching off.
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/serve-web.py
