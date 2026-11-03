#!/bin/sh
# Install and launch target/android/waypoint.apk on a connected phone, or on the
# "waypoint" emulator (created and started if needed). Then follow the game's log.
#   tools/android/run.sh [--no-log]
set -eu
ANDROID_HOME=${ANDROID_HOME:-$HOME/Library/Android/sdk}
PACKAGE=${PACKAGE:-dev.waypoint.player}
AVD=${AVD:-waypoint}
IMAGE=${IMAGE:-"system-images;android-36;google_apis;arm64-v8a"}
ADB="$ANDROID_HOME/platform-tools/adb"
APK=target/android/waypoint.apk
[ -f "$APK" ] || { echo "error: build first: tools/android/build.sh" >&2; exit 1; }

if ! "$ADB" devices | grep -qE "device$"; then
  if ! "$ANDROID_HOME/emulator/emulator" -list-avds | grep -qx "$AVD"; then
    echo no | "$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager" create avd -n "$AVD" -k "$IMAGE" -d pixel_7 >/dev/null
  fi
  echo "starting emulator ${AVD}..."
  "$ANDROID_HOME/emulator/emulator" -avd "$AVD" -no-snapshot-save -no-audio -no-boot-anim -gpu host >/tmp/waypoint-emulator.log 2>&1 &
  "$ADB" wait-for-device
  until [ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; do sleep 2; done
fi

"$ADB" install -r "$APK" >/dev/null
"$ADB" shell am force-stop "$PACKAGE"
"$ADB" logcat -c
"$ADB" shell am start -n "$PACKAGE/android.app.NativeActivity" >/dev/null
echo "launched $PACKAGE"
[ "${1:-}" = "--no-log" ] || exec "$ADB" logcat -s waypoint:V RustStdoutStderr:V AndroidRuntime:E DEBUG:V
