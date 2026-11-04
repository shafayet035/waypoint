# Waypoint project: working notes for agents

This folder is a Waypoint game. Everything is plain text; there is no editor.

## Layout

- `game.toml`: name, version, `start_scene`, and `[plugins] disabled`.
- `vehicles/*.vehicle.ron`: one vehicle per file. Units are in field names. Here: `hatchback`
  (front-drive car with ABS) and `city_bus` (12 m bus: air brakes, ABS, 80 km/h governor).
- `scenes/*.scene.ron`: ground settings and named spawn points.
- `tests/*.scenario.ron`: headless tests with a driver and assertions.
- `tests/*.shot.ron` + `tests/*.png`: screenshot tests: a frame of a scenario or scene, compared
  with the approved picture.

## The loop

1. Edit a file.
2. `wp check --json`: schema and cross-reference errors, each with a code, file, line and fix.
3. `wp test --json`: runs every scenario; `wp test <name>` runs one.
4. On failure, read the assertion `detail` and the metrics, fix, and repeat.
5. A screenshot test that fails leaves `target/shots/<name>.png` (the new picture) and
   `<name>.diff.png` (changes in magenta). Open both. If the change is intended,
   `wp test --approve <name>` saves the new picture.

## Useful commands

- `wp schema vehicle|scene|scenario|game`: exact fields, with descriptions.
- `wp docs scenario`: drivers and assertions. `wp docs errors/<CODE>`: how to fix an error.
- `wp vehicle test <name>`: 0–100 km/h time, 100–0 km/h stopping distance, skidpad lateral g.
- `wp run --headless --throttle 1 --seconds 10 --trace`: quick manual drive.

## Facts worth knowing

- Simulation: 60 Hz gameplay ticks, 480 Hz physics. Same build + same seed = identical results,
  so a test that passes once passes every time.
- Physics limits: with tyre friction 1.0, about 39 m to stop from 100 km/h and about 1 g of grip.
  If a test asks for better than physics allows, change the test, not the physics.
- The `bus_*` and `car_brake_100` tests check regulation limits (UN ECE R13 braking, EU turning
  circle). Keep them passing when you tune those vehicles.
- Vehicle events (GearShift, AbsEngaged, WheelLift, LowAirPressure, RolledOver, ...) can be asserted
  with `NoEvent("...")` / `ExpectEvent("...")`, and polled live with `events.poll`.
