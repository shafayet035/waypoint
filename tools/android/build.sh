#!/bin/sh
# Build target/android/waypoint.apk (arm64) with the game folder bundled as assets.
#   tools/android/build.sh [project-dir]
# Needs ANDROID_HOME (default ~/Library/Android/sdk) with platforms;android-36,
# build-tools, an NDK, and rustup's aarch64-linux-android target.
# Install and start:  tools/android/run.sh
set -eu

PROJECT=${1:-examples/sandbox}
PACKAGE=${PACKAGE:-dev.waypoint.player}
LABEL=${LABEL:-Waypoint}
MIN_SDK=26
TARGET_SDK=36
TARGET=aarch64-linux-android
ANDROID_HOME=${ANDROID_HOME:-$HOME/Library/Android/sdk}
[ -d "$HOME/.cargo/bin" ] && export PATH="$HOME/.cargo/bin:$PATH"

fail() { echo "error: $*" >&2; exit 1; }
[ -f "$PROJECT/game.toml" ] || fail "[WP-G001] no game.toml in $PROJECT"
NDK=${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)}
[ -d "$NDK" ] || fail "no Android NDK under $ANDROID_HOME/ndk (sdkmanager \"ndk;<version>\")"
BT=$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1)
[ -d "$BT" ] || fail "no build-tools under $ANDROID_HOME (sdkmanager \"build-tools;<version>\")"
PLATFORM_JAR="$ANDROID_HOME/platforms/android-$TARGET_SDK/android.jar"
[ -f "$PLATFORM_JAR" ] || fail "missing $PLATFORM_JAR (sdkmanager \"platforms;android-$TARGET_SDK\")"
rustup target list --installed | grep -qx "$TARGET" || fail "run: rustup target add $TARGET"

# The NDK's host tools are universal binaries in darwin-x86_64 on every Mac.
TC="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/aarch64-linux-android$MIN_SDK-clang"
export CC_aarch64_linux_android="$TC/aarch64-linux-android$MIN_SDK-clang"
export AR_aarch64_linux_android="$TC/llvm-ar"
# 16 KB pages: required for Android 15+ devices that use them.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
cargo build --release -p wp_player --lib --target "$TARGET"

OUT=target/android
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/lib/arm64-v8a" "$STAGE/assets/game"
cp "target/$TARGET/release/libwp_player.so" "$STAGE/lib/arm64-v8a/"

# Game data as assets, plus the file list the player uses to unpack them.
(cd "$PROJECT" && find . -type f \( -name '*.toml' -o -name '*.ron' -o -name '*.md' \) -not -path './target/*' | sed 's|^\./||' | sort) > "$STAGE/files.txt"
while read -r f; do
  mkdir -p "$STAGE/assets/game/$(dirname "$f")"
  cp "$PROJECT/$f" "$STAGE/assets/game/$f"
done < "$STAGE/files.txt"
mv "$STAGE/files.txt" "$STAGE/assets/game/files.txt"

# The control port (dev builds only) needs the INTERNET permission, even for localhost.
PERMISSIONS=""
if grep -Eq '^[[:space:]]*control_port[[:space:]]*=' "$PROJECT/game.toml"; then
  PERMISSIONS='<uses-permission android:name="android.permission.INTERNET"/>'
fi

cat > "$STAGE/AndroidManifest.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="$PACKAGE" android:versionCode="1" android:versionName="0.1.0">
  <uses-feature android:glEsVersion="0x00030000" android:required="true"/>
  $PERMISSIONS
  <application android:label="$LABEL" android:hasCode="false" android:extractNativeLibs="true"
      android:theme="@android:style/Theme.NoTitleBar.Fullscreen">
    <activity android:name="android.app.NativeActivity" android:exported="true"
        android:screenOrientation="sensorLandscape" android:launchMode="singleTask"
        android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density">
      <meta-data android:name="android.app.lib_name" android:value="wp_player"/>
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
XML

"$BT/aapt2" link -o "$OUT/unaligned.apk" -I "$PLATFORM_JAR" --manifest "$STAGE/AndroidManifest.xml" \
  -A "$STAGE/assets" --min-sdk-version $MIN_SDK --target-sdk-version $TARGET_SDK
(cd "$STAGE" && zip -q -r ../unaligned.apk lib)
"$BT/zipalign" -P 16 -f 4 "$OUT/unaligned.apk" "$OUT/waypoint.apk"

KEYSTORE=${KEYSTORE:-$HOME/.android/debug.keystore}
if [ ! -f "$KEYSTORE" ]; then
  mkdir -p "$(dirname "$KEYSTORE")"
  keytool -genkeypair -keystore "$KEYSTORE" -storepass android -alias androiddebugkey -keypass android \
    -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null 2>&1
fi
# apksigner is Java; newer JDKs print harmless native-access warnings, so drop those lines.
quiet() { "$@" 2>&1 | grep -v -e '^WARNING' -e '^$' || true; }
quiet "$BT/apksigner" sign --ks "$KEYSTORE" --ks-pass pass:android --key-pass pass:android \
  --ks-key-alias androiddebugkey "$OUT/waypoint.apk"
"$BT/apksigner" verify "$OUT/waypoint.apk" 2>/dev/null || fail "APK signature does not verify"
rm -f "$OUT/unaligned.apk"
echo "$OUT/waypoint.apk"
