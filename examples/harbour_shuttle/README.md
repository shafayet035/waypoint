# Harbour Shuttle

A night minibus service round a small harbour town, in the rain, drawn in a
toon style. The second Waypoint game: built from `wp new --template minimal`
with data files only (no engine code), checked with `wp check` and `wp test`.

```sh
wp --project examples/harbour_shuttle run --mission night_run   # play (W/S/A/D, C camera)
wp --project examples/harbour_shuttle test                      # 5 scenarios, 2 golden replays
```

| File | What it is |
| --- | --- |
| `game.toml` | start scene, Mid tier, toon `[look]`, the mod switched off |
| `vehicles/minibus.vehicle.ron` | a 4.8 t, 16-seat diesel minibus (class M2) with ABS, TC and ESC; diesel `sound` |
| `assets/vehicles/minibus.glb` | its model, from `tools/models/vehicles.py` (CC0, licence file beside it) |
| `roads/harbour.road.ron` | six nodes, seven roads: quay, market street, cliff road, lighthouse lane, ferry hill; lights at the quay, priority at the square; six bus stops |
| `scenes/harbour.scene.ron` | traffic, people, light rain, a running clock from 8 pm |
| `missions/night_run.mission.ron` + `.lua` | two timetabled laps; the script adds the late ferry's passengers |
| `tests/` | ECE R13 braking for the minibus, the night run (autopilot: complete, score ≥ 85, nobody hurt), harbour traffic |
| `mods/electric_shuttle/` | a mod replacing the minibus with an electric one (one gear, instant torque, motor whine); enable it in `game.toml` |

Building it found engine issues that were fixed in the engine, not worked
around in the game: AI drivers braking hard at bends and stops and flicking
between pedals, the AI treating vehicles over 3.5 t as cars, ESC acting in
calm driving, pedestrians stepping out at lights while a car was still
crossing, and `wp check` not saying when a road could not be driven through
(now WP-N012).
