#!/usr/bin/env bash
# Drive the mobile app on an emulator or simulator against a host daemon seeded with fixture webapps.
set -euo pipefail

PLATFORM="${1:-}"
shift || true
case "$PLATFORM" in
android | ios) ;;
*)
    echo "usage: e2e-mobile.sh <android|ios> [flow...]" >&2
    exit 2
    ;;
esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
E2E_DIR="${ROOT}/.e2e"
FIXTURE_PORT="${E2E_FIXTURE_PORT:-8899}"
PACKAGE=com.bridgething.gateway
HA_DIR="${ROOT}/packages/webapps/catalog/home-assistant"
FLOWS=("$@")
[ "${#FLOWS[@]}" -gt 0 ] || FLOWS=("${ROOT}/mobile/e2e/flows")

AVD="${BRIDGETHING_E2E_AVD:-Pixel_10_Pro}"
ABI="${BRIDGETHING_E2E_ABI:-arm64-v8a}"
APK="${ROOT}/mobile/android/app/build/outputs/apk/debug/app-debug.apk"
EMULATOR="${ANDROID_HOME:-$HOME/Library/Android/sdk}/emulator/emulator"

SIM="${BRIDGETHING_E2E_SIM:-iPhone 17 Pro}"
DERIVED="${E2E_DIR}/ios-derived"
APP="${DERIVED}/Build/Products/Debug-iphonesimulator/bridgething.app"

started_emulator=""
started_sim=""
fixture_pid=""

cleanup() {
    local status=$?
    BRIDGETHING_DEV_DIR="$E2E_DIR" "${ROOT}/scripts/dev-daemon.sh" stop >/dev/null 2>&1 || true
    [ -n "$fixture_pid" ] && kill "$fixture_pid" 2>/dev/null || true
    if [ "${BRIDGETHING_E2E_KEEP_DEVICE:-0}" != "1" ]; then
        [ -n "$started_emulator" ] && adb emu kill >/dev/null 2>&1 || true
        [ -n "$started_sim" ] && xcrun simctl shutdown "$started_sim" >/dev/null 2>&1 || true
    fi
    exit "$status"
}
trap cleanup EXIT

for tool in maestro bun cargo zip; do
    command -v "$tool" >/dev/null || { echo "[e2e] $tool not found" >&2; exit 1; }
done

mkdir -p "${E2E_DIR}/examples"

echo "== fixture bundle: home assistant =="
( cd "$HA_DIR" && bun run build >/dev/null )
rm -f "${E2E_DIR}/examples/home-assistant.zip"
( cd "${HA_DIR}/dist" && zip -qr "${E2E_DIR}/examples/home-assistant.zip" . )

android_device() {
    command -v adb >/dev/null || { echo "[e2e] adb not found" >&2; exit 1; }
    echo "== emulator =="
    if ! adb get-state >/dev/null 2>&1; then
        [ -x "$EMULATOR" ] || { echo "[e2e] no emulator binary at $EMULATOR" >&2; exit 1; }
        nohup "$EMULATOR" -avd "$AVD" -no-snapshot-load -no-boot-anim </dev/null >"${E2E_DIR}/emulator.log" 2>&1 &
        started_emulator=1
        adb wait-for-device
    fi
    until [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; do sleep 2; done
    adb shell wm dismiss-keyguard >/dev/null 2>&1 || true

    echo "== apk ($ABI, js bundled) =="
    source "${ROOT}/scripts/gradle-jdk.sh"
    gradle_jdk_env
    ( cd "${ROOT}/mobile/android" && JAVA_HOME="$GRADLE_JAVA" ./gradlew :app:assembleDebug \
        --console=plain -q \
        -PbridgethingBundleJs=true \
        -PreactNativeArchitectures="$ABI" \
        -PcargoNdkAbis="$ABI" \
        -Porg.gradle.java.installations.paths="$GRADLE_INSTALLS" \
        -Porg.gradle.java.installations.auto-download=false </dev/null )
    adb uninstall "$PACKAGE" >/dev/null 2>&1 || true
    adb install -t "$APK" >/dev/null
    FIXTURE_HOST=10.0.2.2
    EXCLUDE_TAGS=ios-only
}

ios_device() {
    command -v xcrun >/dev/null || { echo "[e2e] xcrun not found" >&2; exit 1; }
    echo "== simulator ($SIM) =="
    local udid
    udid="$(xcrun simctl list devices available -j | python3 -c '
import json, sys
name = sys.argv[1]
for devices in json.load(sys.stdin)["devices"].values():
    for device in devices:
        if device["name"] == name:
            print(device["udid"]); sys.exit(0)
sys.exit(1)' "$SIM")" || { echo "[e2e] no available simulator named '$SIM'" >&2; exit 1; }
    if [ "$(xcrun simctl list devices -j | python3 -c 'import json,sys; u=sys.argv[1]; print(next(d["state"] for ds in json.load(sys.stdin)["devices"].values() for d in ds if d["udid"]==u))' "$udid")" != "Booted" ]; then
        xcrun simctl boot "$udid"
        started_sim="$udid"
    fi
    xcrun simctl bootstatus "$udid" -b >/dev/null

    echo "== app (simulator, js bundled) =="
    ( cd "${ROOT}/mobile/ios" && pod install --silent )
    FORCE_BUNDLING=1 xcodebuild build \
        -workspace "${ROOT}/mobile/ios/bridgething.xcworkspace" \
        -scheme bridgething \
        -configuration Debug \
        -sdk iphonesimulator \
        -destination "id=${udid}" \
        -derivedDataPath "$DERIVED" \
        CODE_SIGNING_ALLOWED=NO \
        -quiet
    xcrun simctl uninstall "$udid" "$PACKAGE" >/dev/null 2>&1 || true
    xcrun simctl install "$udid" "$APP"
    FIXTURE_HOST=127.0.0.1
    EXCLUDE_TAGS=android-only
}

"${PLATFORM}_device"

echo "== host daemon (seeded from ${E2E_DIR}/examples) =="
rm -rf "${E2E_DIR}/state" "${E2E_DIR}/webapps" "${E2E_DIR}/.seeded"
BRIDGETHING_DEV_DIR="$E2E_DIR" "${ROOT}/scripts/dev-daemon.sh" start

echo "== fixture server :${FIXTURE_PORT} =="
E2E_FIXTURE_PORT="$FIXTURE_PORT" BRIDGETHING_DEV_DIR="$E2E_DIR" bun "${ROOT}/scripts/e2e/fixture-server.ts" &
fixture_pid=$!
until curl -fsS "http://127.0.0.1:${FIXTURE_PORT}/companion.json" >/dev/null 2>&1; do sleep 0.5; done

echo "== maestro ($PLATFORM) =="
maestro test --exclude-tags "$EXCLUDE_TAGS" -e FIXTURE_HOST="$FIXTURE_HOST" -e FIXTURE_PORT="$FIXTURE_PORT" "${FLOWS[@]}"
