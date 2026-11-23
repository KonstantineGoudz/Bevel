#!/usr/bin/env bash
set -euo pipefail

# Build and run party-dungeon for Android using xbuild.
#
# Usage:
#   ./game/scripts/build-android.sh [--debug|--release] [--device <device-id>] [--run] [--test] [--logs] [--screenshot [path]] [...]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CARGO_TOOLS="$REPO_ROOT/.tmp/cargo-tools/bin"
XDG_CACHE="$REPO_ROOT/.tmp/xdg-cache"

if [ ! -x "$CARGO_TOOLS/x" ]; then
    echo "Error: 'x' binary not found at $CARGO_TOOLS/x" >&2
    exit 1
fi

# Ensure llvm tools and lld are available in PATH
export PATH="$CARGO_TOOLS:$PATH"
export XDG_CACHE_HOME="$XDG_CACHE"

PROFILE="--release"
RUN_APP=0
RUN_TEST=0
SHOW_LOGS=0
DO_SCREENSHOT=0
SCREENSHOT_PATH=""
DEVICE_ARG=""
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            PROFILE="--debug"
            shift
            ;;
        --release)
            PROFILE="--release"
            shift
            ;;
        --run)
            RUN_APP=1
            shift
            ;;
        --test)
            RUN_TEST=1
            shift
            ;;
        --logs|--logcat)
            SHOW_LOGS=1
            shift
            ;;
        --screenshot)
            DO_SCREENSHOT=1
            if [[ $# -gt 1 && ! "$2" =~ ^-- ]]; then
                SCREENSHOT_PATH="$2"
                shift 2
            else
                shift
            fi
            ;;
        --device)
            DEVICE_ARG="$2"
            shift 2
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

echo "==> Building party-dungeon for Android (arm64, $PROFILE)..."

CMD=(
    "$CARGO_TOOLS/x" build
    $PROFILE
    --platform android
    --arch arm64
    --format apk
    --manifest-path "$REPO_ROOT/game/Cargo.toml"
    -p party-dungeon
)

"${CMD[@]}" "${EXTRA_ARGS[@]}"

PROFILE_DIR="release"
if [ "$PROFILE" = "--debug" ]; then
    PROFILE_DIR="debug"
fi

APK_PATH="$REPO_ROOT/game/target/x/$PROFILE_DIR/android/party-dungeon.apk"
echo "==> Built APK: $APK_PATH"

# Copy to web/downloads if directory exists
if [ -d "$REPO_ROOT/game/web/downloads" ]; then
    cp -f "$APK_PATH" "$REPO_ROOT/game/web/downloads/party-dungeon.apk"
    echo "==> Updated $REPO_ROOT/game/web/downloads/party-dungeon.apk"
fi

# If test is requested, delegate to test-android.sh
if [ "$RUN_TEST" -eq 1 ]; then
    TEST_CMD=("$SCRIPT_DIR/test-android.sh" "$PROFILE")
    if [ -n "$DEVICE_ARG" ]; then
        TEST_CMD+=(--device "$DEVICE_ARG")
    fi
    if [ -n "$SCREENSHOT_PATH" ]; then
        TEST_CMD+=(--screenshot "$SCREENSHOT_PATH")
    fi
    exec "${TEST_CMD[@]}"
fi

# Device resolution for --run, --logs, or --screenshot
get_device() {
    if [ -n "$DEVICE_ARG" ]; then
        echo "${DEVICE_ARG#adb:}"
        return 0
    fi
    local dev
    dev=$(adb devices | awk 'NR>1 && $2=="device" {print $1; exit}')
    if [ -z "$dev" ]; then
        echo "Error: No connected Android devices found." >&2
        exit 1
    fi
    echo "${dev#adb:}"
}

if [ "$RUN_APP" -eq 1 ] || [ "$DO_SCREENSHOT" -eq 1 ] || [ "$SHOW_LOGS" -eq 1 ]; then
    DEVICE_SERIAL=$(get_device)
    ADB=(adb -s "$DEVICE_SERIAL")

    if [ "$RUN_APP" -eq 1 ]; then
        echo "==> Installing to device $DEVICE_SERIAL..."
        "${ADB[@]}" install -r "$APK_PATH"
        echo "==> Launching party-dungeon..."
        "${ADB[@]}" shell am start -n biz.conman.partydungeon/android.app.NativeActivity
    fi

    if [ "$DO_SCREENSHOT" -eq 1 ]; then
        TARGET_SHOT="${SCREENSHOT_PATH:-$REPO_ROOT/game/target/android-screen.png}"
        mkdir -p "$(dirname "$TARGET_SHOT")"
        echo "==> Capturing screenshot to $TARGET_SHOT..."
        sleep 2
        "${ADB[@]}" exec-out screencap -p > "$TARGET_SHOT"
        echo "==> Saved $TARGET_SHOT"
    fi

    if [ "$SHOW_LOGS" -eq 1 ]; then
        echo "==> Streaming logs for party_dungeon on $DEVICE_SERIAL..."
        exec "${ADB[@]}" logcat -v time -s party_dungeon:V RustStdoutStderr:V ImeTracker:I InsetsController:D
    fi
fi
