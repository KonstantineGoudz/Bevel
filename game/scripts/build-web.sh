#!/usr/bin/env bash
# Build the Bevy client for the browser into web/pkg, then serve it on 8082.
# Usage:
#   ./build-web.sh            fast iteration build (wasm-dev: no LTO, links fast)
#   ./build-web.sh --release  final optimized build (wasm-release: LTO, slow link)
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--release" ]]; then
  PROFILE="wasm-release"
else
  PROFILE="wasm-dev"
fi

cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p party-dungeon

mkdir -p web/pkg
wasm-bindgen \
  --out-name party_dungeon \
  --out-dir web/pkg \
  --target web \
  "target/wasm32-unknown-unknown/$PROFILE/party_dungeon.wasm"

echo "Built web/pkg ($PROFILE) — run ./scripts/serve-web.sh and open http://localhost:8082/"
