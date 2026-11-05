# wp_player

The game player for every platform. `play(root, data_dir, scenario, exit_after, run)` loads a project and
runs it with the platform's window loop. With game.toml `[quality] auto = true` (the default) it first
reads the device's tier from `<data_dir>/<game>/settings.toml`, or measures it (`wp_render::bench`, about a
second on first launch) and saves it there; the log says which (`quality flagship (benchmark 3.0 ms ...)`).
The window runs the render governor (lower resolution, effects, then 30 fps when frames run late or the
OS reports heat); step changes are logged (`render step 3 (Serious heat ...)`).
Data folders: Android app storage; iOS/macOS `~/Library/Application Support/Waypoint`; Linux XDG data home.

- Desktop/iOS: the `waypoint-player` binary (`main.rs`). Project lookup: a folder argument,
  `WP_PROJECT`, `<exe dir>/game` (iOS bundle), then the current folder. `--scenario <name>` plays a
  scenario's driver; `WP_EXIT_AFTER=<s>` closes it (smoke tests).
- Android: this crate is also a cdylib, `libwp_player.so`, loaded by `NativeActivity`.
  `android_main` unpacks `assets/game` (listed in `assets/game/files.txt`) into app storage, then plays.
  Logs go to logcat with tag `waypoint`; panics are logged too.
- `[dev] control_port` in game.toml opens the control protocol on 127.0.0.1 (Android: `adb forward`).
