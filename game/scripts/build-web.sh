#!/usr/bin/env bash
# Build the Bevy client for the browser into web/pkg, then serve it on 8082.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --profile wasm-release --target wasm32-unknown-unknown -p party-dungeon

mkdir -p web/pkg
wasm-bindgen \
  --out-name party_dungeon \
  --out-dir web/pkg \
  --target web \
  target/wasm32-unknown-unknown/wasm-release/party_dungeon.wasm

echo "Built web/pkg — run ./scripts/serve-web.sh and open http://localhost:8082/"
