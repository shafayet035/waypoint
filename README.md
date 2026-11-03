# Waypoint

An agent-first engine for bus, truck and car driving games, written in Rust. An AI coding agent
can create, run, test and fix a game entirely through text files and the `wp` command line.
Games will ship on Android and iOS. See [`architecture-design.md`](architecture-design.md).

## Quick start

```sh
cargo build
./target/debug/wp new my_game
cd my_game
../target/debug/wp check
../target/debug/wp test
../target/debug/wp vehicle test hatchback
../target/debug/wp run --headless --throttle 1 --seconds 8 --trace
../target/debug/wp shot --scenario skidpad --at 10s --camera showcase --out skidpad.png
../target/debug/wp run        # drive in a window: W/S throttle-brake, A/D steer, C camera, R reset
```

## Use it from Claude Code (MCP)

```sh
cargo build
claude mcp add waypoint -- "$PWD/target/debug/wp" --project "$PWD/examples/sandbox" mcp
```

Claude then gets tools to check and test the project, take screenshots, and drive a live
simulation (`session_start`, `session_call` with control-protocol methods such as `sim.step`,
`input.set`, `world.find`, `render.screenshot`).

## Android (main target)

```sh
rustup target add aarch64-linux-android        # once (rust-toolchain.toml does this too)
tools/android/build.sh                         # → target/android/waypoint.apk (arm64, signed with a debug key)
tools/android/run.sh                           # installs on a phone over USB, or starts the "waypoint" emulator
```

Needs the Android SDK in `~/Library/Android/sdk` with `platforms;android-36`, `build-tools`, an NDK,
`platform-tools`, and for the emulator `emulator` plus `system-images;android-36;google_apis;arm64-v8a`.
No Gradle or Java code: the game is a native library loaded by Android's `NativeActivity`.
Agents can drive the game on the device: set `[dev] control_port = 7878` in `game.toml`, rebuild, then
`adb forward tcp:7878 tcp:7878` and send control-protocol requests to `127.0.0.1:7878`.

## iOS

```sh
rustup target add aarch64-apple-ios-sim aarch64-apple-ios   # once (rust-toolchain.toml does this too)
tools/ios/build.sh sim                                      # → target/ios/sim/Waypoint.app
xcrun simctl install booted target/ios/sim/Waypoint.app
xcrun simctl launch booted dev.waypoint.player
```

Touch controls: left thumb drags sideways to steer; right thumb high for throttle, low for brake;
tap the top edge to change camera. The simulator needs an iOS runtime
(`xcodebuild -downloadPlatform iOS`, about 7–8 GB).

## Status

Milestone M0 (Foundations): the simulation core, data formats, CLI and a first wgpu renderer
(windowed and headless) work on desktop, and the player app builds for Android and iOS. See [`AGENTS.md`](AGENTS.md) for the crate map and rules.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
