#!/bin/sh
# Build Waypoint.app for iOS.
#   tools/ios/build.sh [sim|device] [project-dir] [low|mid|flagship]
# The game is packed for one quality tier by `wp asset build` (default: game.toml's
# tier); WITH_TESTS=1 keeps scenario files for --scenario.
# sim    → target/ios/sim/Waypoint.app    (iPhone/iPad simulator on Apple Silicon)
# device → target/ios/device/Waypoint.app (needs CODESIGN_IDENTITY and a provisioning profile to install)
# Install on a booted simulator:
#   xcrun simctl install booted target/ios/sim/Waypoint.app
#   xcrun simctl launch booted dev.waypoint.player [--scenario skidpad]
set -eu

KIND=${1:-sim}
PROJECT=${2:-examples/sandbox}
TIER=${3:-}
BUNDLE_ID=${BUNDLE_ID:-dev.waypoint.player}
export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-16.0}
# Prefer rustup's toolchain (it has the iOS targets) over a system Rust.
[ -d "$HOME/.cargo/bin" ] && export PATH="$HOME/.cargo/bin:$PATH"

case "$KIND" in
  sim)    TARGET=aarch64-apple-ios-sim; PLATFORM=iPhoneSimulator; SDK=iphonesimulator ;;
  device) TARGET=aarch64-apple-ios;     PLATFORM=iPhoneOS;        SDK=iphoneos ;;
  *) echo "usage: $0 [sim|device] [project-dir]" >&2; exit 2 ;;
esac

if [ ! -f "$PROJECT/game.toml" ]; then
  echo "error[WP-G001]: no game.toml in $PROJECT" >&2; exit 1
fi
rustup target list --installed 2>/dev/null | grep -qx "$TARGET" || {
  echo "error: Rust target $TARGET is missing; run: rustup target add $TARGET" >&2; exit 1; }

cargo build --release -p wp_player --target "$TARGET"

APP="target/ios/$KIND/Waypoint.app"
rm -rf "$APP"
mkdir -p "$APP"
cp "target/$TARGET/release/waypoint-player" "$APP/waypoint-player"
# Ship the game packed for the tier: runtime files only, textures sized for it.
cargo build -p wp_cli
target/debug/wp --project "$PROJECT" asset build ${TIER:+--tier "$TIER"} ${WITH_TESTS:+--with-tests} --out "$APP/game"

SDK_VERSION=$(xcrun --sdk "$SDK" --show-sdk-version)
cat > "$APP/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>waypoint-player</string>
  <key>CFBundleName</key><string>Waypoint</string>
  <key>CFBundleDisplayName</key><string>Waypoint</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleSupportedPlatforms</key><array><string>$PLATFORM</string></array>
  <key>DTPlatformName</key><string>$SDK</string>
  <key>DTSDKName</key><string>$SDK$SDK_VERSION</string>
  <key>MinimumOSVersion</key><string>$IPHONEOS_DEPLOYMENT_TARGET</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string><string>metal</string></array>
  <key>UILaunchScreen</key><dict/>
  <key>UIRequiresFullScreen</key><true/>
  <key>UIStatusBarHidden</key><true/>
  <key>UISupportedInterfaceOrientations</key>
  <array><string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>
  <key>UISupportedInterfaceOrientations~ipad</key>
  <array><string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>
</dict>
</plist>
PLIST
plutil -lint "$APP/Info.plist" >/dev/null

if [ "$KIND" = device ] && [ -z "${CODESIGN_IDENTITY:-}" ]; then
  echo "warning: built $APP unsigned; set CODESIGN_IDENTITY and add embedded.mobileprovision to install on a phone" >&2
else
  codesign --force --sign "${CODESIGN_IDENTITY:--}" --timestamp=none "$APP"
fi
echo "$APP"
