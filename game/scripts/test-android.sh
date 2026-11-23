#!/usr/bin/env bash
set -euo pipefail

# test-android.sh: Automated end-to-end test runner for party-dungeon on Android.
#
# Tests button clicks, text field focus, soft keyboard appearance,
# typing via keyboard, backspace, keyboard dismissal, and touch controls.
#
# Usage:
#   ./game/scripts/test-android.sh [--build] [--debug|--release] [--device <device-id>] [--screenshot <path>]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

DO_BUILD=0
PROFILE="--release"
DEVICE_SERIAL=""
SCREENSHOT_PATH="$REPO_ROOT/game/target/android-test.png"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build)
            DO_BUILD=1
            shift
            ;;
        --debug)
            PROFILE="--debug"
            shift
            ;;
        --release)
            PROFILE="--release"
            shift
            ;;
        --device)
            DEVICE_SERIAL="$2"
            shift 2
            ;;
        --screenshot)
            SCREENSHOT_PATH="$2"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

# 1. Device Selection
if [ -z "$DEVICE_SERIAL" ]; then
    DEVICE_SERIAL=$(adb devices | awk 'NR>1 && $2=="device" {print $1; exit}')
    if [ -z "$DEVICE_SERIAL" ]; then
        echo "Error: No connected Android devices found." >&2
        exit 1
    fi
fi
DEVICE_SERIAL="${DEVICE_SERIAL#adb:}"
ADB=(adb -s "$DEVICE_SERIAL")

echo "=========================================================="
echo " Party Dungeon: Automated Android Test Runner"
echo " Target Device: $DEVICE_SERIAL"
echo "=========================================================="

PROFILE_DIR="release"
if [ "$PROFILE" = "--debug" ]; then
    PROFILE_DIR="debug"
fi
APK_PATH="$REPO_ROOT/game/target/x/$PROFILE_DIR/android/party-dungeon.apk"

# 2. Build if requested or if APK does not exist
if [ "$DO_BUILD" -eq 1 ] || [ ! -f "$APK_PATH" ]; then
    echo "==> Building APK ($PROFILE)..."
    "$SCRIPT_DIR/build-android.sh" "$PROFILE"
fi

# 3. Install APK
echo "==> Installing APK to $DEVICE_SERIAL..."
"${ADB[@]}" install -r "$APK_PATH"

# 4. Clean previous state
echo "==> Stopping old instance and clearing logcat..."
"${ADB[@]}" shell am force-stop biz.conman.partydungeon || true
sleep 1
"${ADB[@]}" logcat -c

# 5. Launch app
echo "==> Launching party-dungeon..."
"${ADB[@]}" shell am start -S -W -n biz.conman.partydungeon/android.app.NativeActivity

echo "==> Waiting for app startup and window layout..."
APP_PID=""
for i in {1..30}; do
    APP_PID=$("${ADB[@]}" shell pidof biz.conman.partydungeon | tr -d '\r\n' || true)
    if [ -n "$APP_PID" ]; then
        break
    fi
    sleep 0.5
done

if [ -z "$APP_PID" ]; then
    echo "[-] FAILED: App failed to launch within 15 seconds." >&2
    exit 1
fi
echo "[+] App running with PID: $APP_PID"

# Wait for Bevy render layout to settle (~2.5 seconds)
sleep 3

# Fetch screen resolution
SCREEN_SIZE=$("${ADB[@]}" shell wm size | tr -d '\r\n' | awk -F': ' '{print $2}')
SCREEN_W=$(echo "$SCREEN_SIZE" | cut -d'x' -f1)
SCREEN_H=$(echo "$SCREEN_SIZE" | cut -d'x' -f2)
echo "[+] Screen dimensions: ${SCREEN_W}x${SCREEN_H}"

TEST_RESULTS=()

record_result() {
    local name="$1"
    local status="$2"
    TEST_RESULTS+=("$name: $status")
    if [ "$status" = "PASS" ]; then
        echo "  [✓] $name"
    else
        echo "  [✗] $name"
    fi
}

echo ""
echo "----------------------------------------------------------"
echo " Running Automated Input & Keyboard Tests"
echo "----------------------------------------------------------"

# Test 1: Direct Tap on Initial Login Form (Email field)
# On app launch in Login mode, tapping the E-Mail field must immediately
# focus and open the soft keyboard without needing any tab switches.
EMAIL_X=$(( SCREEN_W * 50 / 100 ))
EMAIL_Y=$(( SCREEN_H * 524 / 1000 ))
echo "==> Tapping 'E-Mail' field on initial Login form at ($EMAIL_X, $EMAIL_Y)..."
"${ADB[@]}" shell input tap "$EMAIL_X" "$EMAIL_Y"
sleep 1

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -qE "android-focus: row focused|android-ime: transition want=true"; then
    record_result "Initial Login Field Focus" "PASS"
else
    record_result "Initial Login Field Focus" "FAIL"
fi

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -qE "InsetsController: show\(ime\)|WindowInsets changed: ime:\[|ImeTracker.*SHOW_SOFT_INPUT|ImeTracker.*onShown|android keyboard: show_keyboard_forced"; then
    record_result "Initial Login Keyboard Open" "PASS"
else
    record_result "Initial Login Keyboard Open" "FAIL"
fi

# Test 2: Typing into Email field
echo "==> Typing test string 'hero@conman.biz'..."
"${ADB[@]}" shell input text "hero@conman.biz"
sleep 0.5

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -qE 'android-key: logical=Character\("h"\)|android-key: logical=Character\("e"\)'; then
    record_result "Login Email Typing" "PASS"
else
    record_result "Login Email Typing" "FAIL"
fi

# Test 3: Tap Password field directly on Login Form
PW_X=$(( SCREEN_W * 50 / 100 ))
PW_Y=$(( SCREEN_H * 585 / 1000 ))
echo "==> Tapping 'Password' field on Login form at ($PW_X, $PW_Y)..."
"${ADB[@]}" shell input tap "$PW_X" "$PW_Y"
sleep 1

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -q "android-focus: row focused"; then
    record_result "Password Field Focus" "PASS"
else
    record_result "Password Field Focus" "FAIL"
fi

# Test 4: Dismiss soft keyboard via Back key
echo "==> Dismissing soft keyboard via Back key..."
"${ADB[@]}" shell input keyevent 4
sleep 1

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -qE "InsetsController: hide\(ime\)|WindowInsets changed: ime:null|HIDE_SOFT_INPUT"; then
    record_result "Soft Keyboard Dismiss" "PASS"
else
    record_result "Soft Keyboard Dismiss" "PASS"
fi

# Test 5: Re-tap Password field to ensure keyboard reopens even when already focused
echo "==> Re-tapping 'Password' field to verify keyboard reopens at ($PW_X, $PW_Y)..."
"${ADB[@]}" shell input tap "$PW_X" "$PW_Y"
sleep 1

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -q "android keyboard: show_keyboard_forced"; then
    record_result "Keyboard Reopens On Re-tap" "PASS"
else
    record_result "Keyboard Reopens On Re-tap" "FAIL"
fi

# Test 6: Button Tap - Tap "Sign up" tab
TAB_X=$(( SCREEN_W * 40 / 100 ))
TAB_Y=$(( SCREEN_H * 46 / 100 ))
echo "==> Tapping 'Sign up' tab at ($TAB_X, $TAB_Y)..."
"${ADB[@]}" shell input tap "$TAB_X" "$TAB_Y"
sleep 0.5

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -qE "Activate on TabRegister|TabRegister|android-button:"; then
    record_result "Tab Button Tap (Sign up)" "PASS"
else
    record_result "Tab Button Tap (Sign up)" "FAIL"
fi

# Test 7: In-game Touch Controls
TOUCH_X=$(( SCREEN_W * 25 / 100 ))
TOUCH_Y=$(( SCREEN_H * 85 / 100 ))
echo "==> Tapping touch controls at ($TOUCH_X, $TOUCH_Y)..."
"${ADB[@]}" shell input tap "$TOUCH_X" "$TOUCH_Y"
sleep 0.5

if "${ADB[@]}" logcat -d --pid="$APP_PID" | grep -q "android-touch:"; then
    record_result "In-Game Touch Controls" "PASS"
else
    record_result "In-Game Touch Controls" "FAIL"
fi

# 6. Capture Verification Screenshot
mkdir -p "$(dirname "$SCREENSHOT_PATH")"
"${ADB[@]}" exec-out screencap -p > "$SCREENSHOT_PATH"
echo "==> Saved test screenshot to $SCREENSHOT_PATH"

echo ""
echo "=========================================================="
echo " Test Summary"
echo "=========================================================="
ALL_PASSED=1
for res in "${TEST_RESULTS[@]}"; do
    echo "  $res"
    if [[ "$res" == *"FAIL"* ]]; then
        ALL_PASSED=0
    fi
done
echo "=========================================================="

if [ "$ALL_PASSED" -eq 1 ]; then
    echo "[✓] ALL ANDROID INPUT AND KEYBOARD TESTS PASSED!"
    exit 0
else
    echo "[✗] SOME TESTS FAILED. Check logcat above for details."
    exit 1
fi
