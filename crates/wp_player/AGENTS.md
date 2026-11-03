# wp_player

`waypoint-player` runs a project with keyboard or touch. It is the binary inside `Waypoint.app`
(see `tools/ios/build.sh`). Project lookup: a folder argument, `WP_PROJECT`, `<exe dir>/game`
(mobile bundles), then the current folder. `--scenario <name>` plays a scenario's driver (demo).
`WP_EXIT_AFTER=<s>` closes it after that many seconds (smoke tests).
Setup goes through `waypoint::start`, shared with other front ends.
