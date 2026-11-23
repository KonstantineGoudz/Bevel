#!/usr/bin/env bash
# Build the Bevy client for the browser and lay it out for cacheable static
# hosting.
#
# Usage:
#   ./build-web.sh                       fast iteration build (wasm-dev: no LTO, links fast)
#   ./build-web.sh --release             final optimized build (wasm-release: LTO, slow link)
#   ./build-web.sh --features replicon   enable the bevy_replicon networking path
#
# Features can also come from $PARTY_DUNGEON_FEATURES (space/comma separated).
#
# Output layout (served by web/nginx.conf, see docs/web-caching.md):
#
#   web/pkg/build.json            the live build stamp — always served fresh
#   web/pkg/<stamp>/…             one immutable directory per build
#   web/assets/                   fixed paths (Bevy hardcodes `assets/`), ETag revalidation
#
# The stamp is generated here and exported as PARTY_DUNGEON_BUILD, so one value
# is both written into build.json (which the pages use to pick the pkg
# directory) and compiled into the wasm as `net_client::CLIENT_BUILD` (printed
# in the in-game HUD). A rebuild therefore always gets new URLs, and a reload of
# an unchanged build downloads nothing but the revalidation of the HTML.
set -euo pipefail
cd "$(dirname "$0")/.."

# Use the workspace-local Cargo home when the caller did not pick one: CI/dev
# runs here keep `~/.cargo` read-only, and every other command in this repo is
# documented with CARGO_HOME=.cargo-home.
if [[ -z "${CARGO_HOME:-}" && -d "$PWD/.cargo-home" ]]; then
  export CARGO_HOME="$PWD/.cargo-home"
fi

# Serialize builds. Two concurrent runs (a terminal build overlapping an agent
# build, say) would fight over the staging directory and the live manifest.
if command -v flock >/dev/null 2>&1; then
  exec 9>"${TMPDIR:-/tmp}/party-dungeon-build-web.lock"
  if ! flock -n 9; then
    echo "another build-web.sh is running — waiting for it to finish…" >&2
    flock 9
  fi
fi

PROFILE="wasm-dev"
FEATURES="${PARTY_DUNGEON_FEATURES:-}"
# Where the served tree lives. Override to stage a build elsewhere and copy it
# in later (the mount is read-only, so `cp -a` of just the changed paths).
WEB_OUT="${PARTY_DUNGEON_WEB_OUT:-web}"
# How many builds to keep on disk. >1 keeps the previous build as the instant
# rollback target (copy its `pkg/<stamp>/build.json` over `pkg/build.json`) and
# covers a browser that is mid-load while a deploy happens.
KEEP_BUILDS="${PARTY_DUNGEON_KEEP_BUILDS:-2}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE="wasm-release"; shift ;;
    --features) FEATURES="${2:?--features needs a value}"; shift 2 ;;
    --features=*) FEATURES="${1#--features=}"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if ! [[ "$KEEP_BUILDS" =~ ^[0-9]+$ ]] || [[ "$KEEP_BUILDS" -lt 1 ]]; then
  echo "PARTY_DUNGEON_KEEP_BUILDS must be a positive integer (got '$KEEP_BUILDS')" >&2
  exit 2
fi

BUILD_FEATURE_ARGS=()
if [[ -n "$FEATURES" ]]; then
  BUILD_FEATURE_ARGS=(--features "$FEATURES")
fi

# Build stamp: UTC wall-clock so a human reading `ls web/pkg` sees build order.
# The prune step does NOT rely on the name — it sorts by directory mtime, because
# a pinned/legacy stamp (`2026-11-17.8`) does not sort chronologically against
# auto stamps. `PARTY_DUNGEON_BUILD` pins it for a reproducible build or a
# re-deploy of an existing one; the default cannot repeat a previous stamp unless
# two builds start in the same second.
BUILD_STAMP="${PARTY_DUNGEON_BUILD:-$(date -u +%Y-%m-%d.%H%M%SZ)}"
export PARTY_DUNGEON_BUILD="$BUILD_STAMP"
echo "web build stamp: $BUILD_STAMP (profile $PROFILE${FEATURES:+, features: $FEATURES})"

cargo build --profile "$PROFILE" --target wasm32-unknown-unknown -p party-dungeon "${BUILD_FEATURE_ARGS[@]}"

# Stage everything under a scratch directory and publish it only when complete:
# a failed or half-finished build must never become visible to a browser.
BUILD_TMP="$WEB_OUT/.build"
STAGE="$BUILD_TMP/pkg-$BUILD_STAMP"
ASSETS_STAGE="$BUILD_TMP/assets-$BUILD_STAMP"
rm -rf "$BUILD_TMP"
trap 'rm -rf "$BUILD_TMP"' EXIT
mkdir -p "$STAGE"

wasm-bindgen \
  --out-name party_dungeon \
  --out-dir "$STAGE" \
  --target web \
  "target/wasm32-unknown-unknown/$PROFILE/party_dungeon.wasm"

# Precompress the artifacts that would otherwise dominate a first load. nginx
# serves `<file>.gz` for `<file>` (`gzip_static`), so a release wasm goes over the
# wire as ~11.7 MB instead of ~49.8 MB (wasm-dev: ~28 MB instead of ~197 MB) and
# the browser decompresses it locally. `-n` omits the filename/mtime from the
# gzip header, keeping the artifact byte-identical for an identical input.
gzip -9 -n -c "$STAGE/party_dungeon_bg.wasm" > "$STAGE/party_dungeon_bg.wasm.gz"
gzip -9 -n -c "$STAGE/party_dungeon.js" > "$STAGE/party_dungeon.js.gz"

# Ship the runtime assets next to the page. Bevy's default asset root is
# `assets/`, so the browser fetches `/assets/character.glb` while native runs
# read `game/assets/character.glb` from the crate's working directory.
#
# `cp -a` keeps the source mtimes on purpose: nginx derives an asset's ETag from
# the served file's (mtime, size), so a fresh copy timestamp would make every
# deploy look like a changed asset and force a full re-download of the ~6 MB
# character on the next visit.
cp -a assets "$ASSETS_STAGE"

# The viewer lists packaged models from this manifest (viewer-bevy.html fetches
# it before the wasm boots). Generate it from the copied assets so a new model
# appears in the list after a build, with no hand-kept second catalogue.
python3 - "$ASSETS_STAGE" <<'PY'
import json
import os
import sys

root = sys.argv[1]
models = []
for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in sorted(filenames):
        if os.path.splitext(name)[1].lower() in {".glb", ".gltf"}:
            path = os.path.relpath(os.path.join(dirpath, name), root)
            models.append(path.replace(os.sep, "/"))
models.sort(key=str.lower)
with open(os.path.join(root, "models.json"), "w", encoding="utf-8") as handle:
    json.dump({"models": models}, handle, indent=2)
    handle.write("\n")
print(f"Packaged models: {len(models)}")
PY

# glTF textures dominate and barely compress, but the geometry/JSON in the
# character and kit files still saves ~1.3 MB on a first load; skip the small
# files where the gzip header costs as much as it saves.
while IFS= read -r -d '' asset; do
  gzip -9 -n -c "$asset" > "$asset.gz"
  # Same timestamp as the source, so the ETag of the served `.gz` is stable
  # across deploys that did not change the asset.
  touch -r "$asset" "$asset.gz"
done < <(find "$ASSETS_STAGE" -type f \( -name '*.glb' -o -name '*.gltf' \) -size +64k -print0)

# Swap the asset tree with renames (no copied files ever visible in place).
if [[ -d "$WEB_OUT/assets" ]]; then
  mv "$WEB_OUT/assets" "$BUILD_TMP/assets-old-$BUILD_STAMP"
fi
mv "$ASSETS_STAGE" "$WEB_OUT/assets"

# Publish the build: move the completed package into its stamp directory first,
# then atomically swap the manifest that points at it. A page loading during a
# deploy keeps the previous (still present) build; nothing ever sees a partial
# one, and JS glue and wasm are always from the same stamp directory.
mkdir -p "$WEB_OUT/pkg"
# Re-deploying a pinned stamp (PARTY_DUNGEON_BUILD=…) replaces that directory.
# Only pin a stamp that is NOT live: a live stamp's URLs are cached as immutable,
# so the bytes behind them must never change.
rm -rf "${WEB_OUT:?}/pkg/${BUILD_STAMP:?}"
mv "$STAGE" "$WEB_OUT/pkg/$BUILD_STAMP"

python3 - "$WEB_OUT/pkg" "$BUILD_STAMP" "$PROFILE" "$FEATURES" <<'PY'
import json
import os
import sys
import time

pkg, stamp, profile, features = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
build_dir = os.path.join(pkg, stamp)
wasm = os.path.join(build_dir, "party_dungeon_bg.wasm")
manifest = {
    "build": stamp,
    "profile": profile,
    "features": [part for part in features.replace(",", " ").split() if part],
    "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "wasm_bytes": os.path.getsize(wasm),
    "wasm_gz_bytes": os.path.getsize(wasm + ".gz"),
}


def write_atomic(path, payload):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2)
        handle.write("\n")
    os.replace(tmp, path)


# Each build carries its own manifest, so a rollback is a copy of that file and
# never has to reconstruct the build's profile/features from memory.
write_atomic(os.path.join(build_dir, "build.json"), manifest)
# Then the live pointer — the last write of the whole script, so a page reading
# it mid-deploy still gets the previous (still present) build.
write_atomic(os.path.join(pkg, "build.json"), manifest)
print(
    "Build manifest: "
    f"{os.path.join(pkg, 'build.json')} → {stamp} "
    f"({manifest['wasm_bytes'] / 1e6:.1f} MB wasm, "
    f"{manifest['wasm_gz_bytes'] / 1e6:.1f} MB gzipped)"
)
PY

# Prune old builds, newest first by directory mtime — deliberately NOT by name.
# A pinned or legacy stamp (`2026-11-17.8`) does not sort chronologically against
# auto stamps (`2026-11-17.173925Z`), and name-sorting once made the prune delete
# the live build's actual rollback target. The live stamp is skipped anyway.
mapfile -t old_builds < <(find "$WEB_OUT/pkg" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %f\n' | sort -rn | cut -d' ' -f2-)
for ((i = KEEP_BUILDS; i < ${#old_builds[@]}; i++)); do
  if [[ "${old_builds[$i]}" == "$BUILD_STAMP" ]]; then
    continue
  fi
  rm -rf "${WEB_OUT:?}/pkg/${old_builds[$i]}"
done

echo "Built $WEB_OUT/pkg/$BUILD_STAMP"
echo "  serve locally:  ./scripts/serve-web.sh          (dev server, no caching)"
echo "  deploy caching: ./scripts/deploy-web.sh         (nginx container, immutable pkg)"
echo "  open:           http://localhost:6082/game.html"
