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

# The viewer lists packaged models from this manifest (viewer-bevy.html fetches
# it before the wasm boots). Generate it from the copied assets so a new model
# appears in the list after a build, with no hand-kept second catalogue.
python3 - <<'PY'
import json
import os

root = "assets"
models = []
for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in sorted(filenames):
        if os.path.splitext(name)[1].lower() in {".glb", ".gltf"}:
            path = os.path.relpath(os.path.join(dirpath, name), root)
            models.append(path.replace(os.sep, "/"))
models.sort(key=str.lower)
with open(os.path.join("web", "assets", "models.json"), "w", encoding="utf-8") as handle:
    json.dump({"models": models}, handle, indent=2)
    handle.write("\n")
print(f"Packaged models: {len(models)}")
PY

echo "Built web/pkg ($PROFILE) and web/assets — run ./scripts/serve-web.sh and open http://localhost:8082/"
