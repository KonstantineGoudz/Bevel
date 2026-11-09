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

# Ship the runtime assets next to the page. Bevy's default asset root is
# `assets/`, so the browser fetches `/assets/character.glb` while native runs
# read `game/assets/character.glb` from the crate's working directory.
rm -rf web/assets
cp -R assets web/assets

echo "Built web/pkg ($PROFILE) and web/assets — run ./scripts/serve-web.sh and open http://localhost:8082/"
