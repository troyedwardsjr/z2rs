#!/usr/bin/env bash
# Talk to a connected Android device or emulator through adb. See README.md.
#
#   tools/android/device.sh install [debug|release]   adb install -r the built APK
#   tools/android/device.sh run [debug|release]       install, then start the app
#   tools/android/device.sh log                       logcat for the app and the z2rs tag
#
# adb comes from PATH, else $ANDROID_HOME/platform-tools (or $ANDROID_SDK_ROOT).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACKAGE="com.z2rs.game"
ACTIVITY="$PACKAGE/.LauncherActivity"
LOG_TAG="z2rs"

die() {
  echo "android device: $1" >&2
  shift
  for line in "$@"; do echo "  $line" >&2; done
  exit 2
}

if command -v adb >/dev/null 2>&1; then
  ADB=adb
else
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  if [ -n "$SDK" ] && [ -x "$SDK/platform-tools/adb" ]; then
    ADB="$SDK/platform-tools/adb"
  else
    die "adb not found." \
      "Put the SDK's platform-tools on PATH, or set ANDROID_HOME. To install them:" \
      "  sdkmanager \"platform-tools\""
  fi
fi

need_device() {
  local n
  n="$("$ADB" devices | awk 'NR > 1 && $2 == "device"' | wc -l | tr -d ' ')"
  [ "$n" -gt 0 ] || die "no device or emulator is connected." \
    "Enable USB debugging on the phone and accept the prompt, or start an emulator," \
    "then check with:  $ADB devices"
}

apk_path() {
  local variant="$1" dir f
  dir="$ROOT/android/app/build/outputs/apk/$variant"
  for f in "$dir/app-$variant.apk" "$dir"/*.apk; do
    if [ -f "$f" ]; then
      echo "$f"
      return 0
    fi
  done
  die "no $variant APK in $dir." "Build it first:  make android$([ "$variant" = release ] && echo -release)"
}

install() {
  local variant="${1:-debug}" apk
  case "$variant" in debug | release) ;; *) die "variant must be debug or release, not '$variant'" ;; esac
  apk="$(apk_path "$variant")"
  case "$apk" in
    *-unsigned.apk) die "$apk is unsigned; sign it with apksigner first (README.md)." ;;
  esac
  need_device
  echo "android device: installing $apk"
  "$ADB" install -r "$apk"
}

cmd="${1:-}"
[ $# -gt 0 ] && shift
case "$cmd" in
  install) install "$@" ;;
  run)
    install "$@"
    "$ADB" shell am start -n "$ACTIVITY"
    ;;
  log)
    need_device
    pid="$("$ADB" shell pidof -s "$PACKAGE" 2>/dev/null | tr -d '\r' || true)"
    if [ -n "$pid" ]; then
      echo "android device: logcat for $PACKAGE (pid $pid); Ctrl-C to stop"
      exec "$ADB" logcat --pid="$pid"
    fi
    echo "android device: $PACKAGE is not running; showing the $LOG_TAG tag only (Ctrl-C to stop)"
    exec "$ADB" logcat -s "$LOG_TAG:V" "RustStdoutStderr:V" "AndroidRuntime:E"
    ;;
  *)
    sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
