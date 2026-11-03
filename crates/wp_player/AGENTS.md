# wp_player

The game player for every platform. `play(root, scenario, exit_after, run)` loads a project and
runs it with the platform's window loop.

- Desktop/iOS: the `waypoint-player` binary (`main.rs`). Project lookup: a folder argument,
  `WP_PROJECT`, `<exe dir>/game` (iOS bundle), then the current folder. `--scenario <name>` plays a
  scenario's driver; `WP_EXIT_AFTER=<s>` closes it (smoke tests).
- Android: this crate is also a cdylib, `libwp_player.so`, loaded by `NativeActivity`.
  `android_main` unpacks `assets/game` (listed in `assets/game/files.txt`) into app storage, then plays.
  Logs go to logcat with tag `waypoint`; panics are logged too.
- `[dev] control_port` in game.toml opens the control protocol on 127.0.0.1 (Android: `adb forward`).
