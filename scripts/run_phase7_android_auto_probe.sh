#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
client_root=${MELODEE_ANDROID_CLIENT_ROOT:-/home/steven/src/github/melodee-player}
client_src="$client_root/src"
expected_commit=7008eed187f42940b6614fe3438cf02fa5a07d94
probe_rel=app/src/androidTest/java/com/melodee/autoplayer/Phase7RealServerAndroidAutoTest.kt
probe_target="$client_src/$probe_rel"

: "${MELODEE_PHASE7_CLIENT_BASE_URL:?set the disposable Rust fixture URL}"
: "${MELODEE_PHASE7_CLIENT_EMAIL:=phase7-client@example.test}"
: "${MELODEE_PHASE7_CLIENT_PASSWORD:=phase7-client-password}"

actual_commit=$(git -C "$client_root" rev-parse HEAD)
if [[ "$actual_commit" != "$expected_commit" ]]; then
  echo "Android client commit mismatch: expected $expected_commit, found $actual_commit" >&2
  exit 1
fi
if [[ -n "$(git -C "$client_root" status --short)" ]]; then
  echo "Android client checkout must be clean" >&2
  exit 1
fi
if [[ -e "$probe_target" ]]; then
  echo "Probe target already exists: $probe_target" >&2
  exit 1
fi

mkdir -p "$(dirname "$probe_target")"
cp "$repo_root/compatibility/clients/android/Phase7RealServerAndroidAutoTest.kt" "$probe_target"
cleanup() {
  rm -f "$probe_target"
}
trap cleanup EXIT INT TERM

(
  cd "$client_src"
  ./gradlew assembleDebug assembleDebugAndroidTest
)
adb install -r "$client_src/app/build/outputs/apk/debug/app-debug.apk" >/dev/null
adb install -r "$client_src/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk" >/dev/null
adb shell am instrument -w -r \
  -e class com.melodee.autoplayer.Phase7RealServerAndroidAutoTest \
  -e baseUrl "$MELODEE_PHASE7_CLIENT_BASE_URL" \
  -e email "$MELODEE_PHASE7_CLIENT_EMAIL" \
  -e password "$MELODEE_PHASE7_CLIENT_PASSWORD" \
  com.melodee.autoplayer.test/androidx.test.runner.AndroidJUnitRunner
