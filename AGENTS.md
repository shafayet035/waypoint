# Waypoint engine: notes for agents

Waypoint is an agent-first driving engine in Rust. The design is in
`architecture-design.md`. This repo is at **milestone M0 (Foundations)**.

## Build, test, run

```sh
cargo test --workspace                 # all unit, integration and scenario tests
cargo clippy --workspace --all-targets # must stay warning-free
cargo run -p wp_cli -- --project examples/sandbox test
cargo run -p wp_cli -- --project examples/sandbox vehicle test hatchback
cargo run -p wp_cli -- --project examples/sandbox shot --scenario skidpad --at 10s --camera showcase
cargo run -p wp_cli -- --project examples/sandbox run      # opens a window; W/S/A/D to drive
cargo run -p wp_cli -- --project <game> import osm map.osm --name town --drive-side left
cargo run -p wp_cli -- --project <game> import opendrive map.xodr --name town
```

To see what a scenario looks like at a moment, use `wp shot` and open the PNG.

Live control: `wp serve` (JSON-RPC on stdin/stdout), `wp run --control 0` (a window plus a
localhost port), or `wp mcp` (MCP server). Call `rpc.methods` first. Register the MCP server
with Claude Code: `claude mcp add waypoint -- <repo>/target/debug/wp --project <game dir> mcp`.
Render tests skip themselves when no GPU adapter exists.

The `wp` binary is `target/debug/wp` after a build. Every command takes `--json`.
Use rustup's toolchain (`~/.cargo/bin` first on PATH); `rust-toolchain.toml` pins stable plus the
iOS targets. A Homebrew Rust earlier on PATH lacks those targets.

Android (main target): `tools/android/build.sh` → `target/android/waypoint.apk`; `tools/android/run.sh`
installs it on a phone or the `waypoint` emulator and follows `adb logcat -s waypoint`.

iOS: `tools/ios/build.sh sim` builds `target/ios/sim/Waypoint.app` (game folder bundled inside);
`tools/ios/build.sh device` builds for iPhone (set `CODESIGN_IDENTITY` to sign).

## Map

| Crate | What it owns |
| --- | --- |
| `wp_core` | Diagnostics with codes, `SimTime`, `SimRng`, stable hashing |
| `wp_ecs` | Entities, components, resources, queries, events, scheduler, reflection |
| `wp_app` | `App`, `Plugin`, plugin dependency resolution |
| `wp_data` | RON/TOML loading with line-accurate errors, JSON Schema export |
| `wp_physics` | Planar rigid bodies, `Pose`, `Velocity`, `RigidBody`, `Ground` |
| `wp_vehicle` | `.vehicle.ron`/`.trailer.ron`, validation, `VehiclePlugin` + `TrailerPlugin`, tyres, drivetrain, brakes, suspension |
| `wp_road` | `.road.ron` road networks: geometry, lanes, junction connectors, A* routing, road meshes |
| `wp_world` | `.scene.ron`, spawn points, scene traffic settings, `Name`, `Player` |
| `wp_traffic` | AI traffic: routes, lane following, IDM car-following, junction claims; shared path controllers |
| `wp_script` | Luau mission scripts: sandbox, hooks, `mission` API, deterministic random |
| `wp_mission` | Missions: bus stops on routes, timetable, passengers, score; `MissionPlugin` |
| `wp_test` | `.scenario.ron`, drivers (`drivers.rs`), assertions, the scenario runner |
| `wp_asset` | glTF models in the engine frame (one part per material, roles from names), `.asset.ron` licences, triangle budgets |
| `wp_rhi` | wgpu device setup (headless or window), offscreen targets, PNG capture |
| `wp_render` | Frame extraction from the ECS, cameras, sky/ground/vehicle pipeline |
| `wp_platform` | Window, keyboard driving, fixed-timestep interactive loop (winit) |
| `wp_player` | `waypoint-player`: plays a project in a window; the app shipped to iOS/Android |
| `wp_control` | Control protocol: JSON-RPC 2.0 sessions (query, drive, step, snapshot, screenshot), TCP transport |
| `waypoint` | `DefaultPlugins`, project loader, `drive` (shared session setup), prelude |
| `wp_cli` | The `wp` command and the error-code catalogue |

`examples/sandbox` is a complete project and also the template for `wp new` (`--template minimal` takes
one car, a flat ground and two tests from it). `examples/harbour_shuttle` is a second game built from the
minimal template with data only (a night minibus route, toon look, and an electric-shuttle mod).

## Architecture

Read `docs/adr/0001-architecture-patterns.md`. In short: layered crates (enforced by
`crates/waypoint/tests/architecture.rs`), ECS with data-oriented design, one feature per plugin,
ports and adapters at the edges (platforms, GPU, agent transports), typed events, pure functions
for the maths with thin systems around them, and data files with schemas for content.

## Rules that keep the engine trustworthy

- **Determinism.** No `HashMap`/`HashSet` iteration in simulation code (lookups are fine), no
  wall-clock reads in the sim, randomness only from `SimRng`. Same build + seed = same hash.
- **Errors teach.** New diagnostics need a stable code (`WP-<letter><3 digits>`), a location when
  there is a file, and a `help`. Add every new code to `crates/wp_cli/src/catalog.rs`; a test
  fails otherwise. Never reuse a code for a different meaning.
- **Layers.** Lower crates never depend on higher ones; simulation never depends on rendering.
  Rendering only reads the world (`wp_render::extract`); it must never change simulation state.
- **Data first.** A new vehicle, scene or test should need a file, not code. New data fields get
  doc comments (they become schema descriptions) and units in their names.
- **Refactors** must keep every scenario's `final_hash` identical (`wp test --json`): that proves
  behaviour did not change. Scenarios with a golden replay (`tests/<name>.replay.ron`) check this on
  every `wp test`; after an intended change, re-record with `wp replay record --scenario <name>`. Rendering refactors: compare `wp shot` PNGs byte for byte.
- **Physics changes** must keep `examples/sandbox` scenarios passing and `wp vehicle test` plausible.
  Hatchback: about 8.5 s to 100 km/h, about 41 m to stop from 100 km/h (ABS), 0.74 g on the skidpad,
  11 m turning circle. City bus: about 11 s to 50 km/h, 22 m from 60 km/h, 21 m turning circle.
  The bus and car braking tests encode UN ECE R13 limits; they are the M1 gate.

## M0 status

Done: ECS, scheduler, plugins, data + schemas, planar vehicle physics at 480 Hz, scenario tests,
the wgpu renderer (procedural sky, gridded ground, blob shadows, lit instanced vehicles, 4× MSAA),
`wp shot`, windowed `wp run` with keyboard driving, world snapshots, the control protocol
(`wp serve`, `wp run --control`), the MCP server (`wp mcp`), and
`wp new/check/schema/docs/run/test/systems/plugins/vehicle test`.
Android: the APK builds, installs and runs on the Android 16 emulator (arm64) at a steady 60 ticks/s;
touch controls and the control protocol over `adb forward` are verified there.
iOS: the player app builds, links and signs for the simulator and builds for devices; it has not
been launched yet (no simulator runtime installed here).
M1 (realistic drive): suspension roll/pitch, tipping and rollover, sliding friction, ABS, air
brakes, speed governor, vehicle events (`events.poll`, `NoEvent`/`ExpectEvent`), a 12 m city bus,
and regulation-based validation tests (ECE R13 braking, EU turning circle).
M2 (done): pin joints, semi-trailers, reverse gear, tractor unit, docks and a reversing
driver; the docking gate test passes (`truck_docking`). Road networks with lanes, junctions, routing
and road meshes; a Stanley route follower passes the town route test (`town_route`, ≤ 0.2 m off
lane centre). Routes change lanes (smooth blends on roads ≥ 30 m); junction corners are trimmed
geometrically with 6 m kerb returns. `wp import osm` turns OpenStreetMap data into a road network
and scene (clipped to the export bounds, one-way carriageways centred, left/right traffic); on a
real Dhaka export, routes to 127 of 145 roads drive within 0.8 m of the lane centre (the rest are dead
ends at the map edge). `wp import opendrive` reads ASAM OpenDRIVE: exact line/arc/spiral/poly3/
paramPoly3 reference lines, lane offsets, one road per lane section (lane counts can change), sidewalks,
speed limits and LHT/RHT; junctions become nodes whose paths the engine rebuilds (straight-on lanes
match by sideways position, so slip roads merge into the outer lane). On esmini's sample maps every
reachable road drives within 0.8 m of the lane centre. Where a road continues into one with more or
fewer lanes, the wider road gets a MUTCD-length taper (`RoadGeometry::tapers`): lanes shift to meet
their partners, extra lanes close (or open), and edges, markings and sidewalks follow; the
`lane_drop` gate merges at 70 km/h within 0.04 m of the lane centre at 0.07 g.
Driver aids (`wp_vehicle::assist`) sit between keys or thumbs and the car: brakes bite at 12 % and
build to full in about 0.9 s, steering lock shrinks with speed (about 10° at 60 km/h), and counter-steer
catches slides past 4° of sideslip. The window's input and the `Keyboard` scenario driver both use them;
`keyboard_slalom` (no spin at 100 km/h) and `keyboard_brake_tap` guard the feel. The chase camera trails
the car's heading (0.25 s) in windows only; `wp shot` stays rigid.
M3 (in progress): AI traffic (`wp_traffic`). A scene's `traffic: Some((count, vehicles))` spawns AI
vehicles that drive the full vehicle model: random destinations routed on the lane graph, Stanley
steering (long vehicles aim the middle of their wheelbase and keep to the kerb), IDM car-following of
anything ahead (the player too), merges that yield, and junction claims sized to each vehicle's swept
path (wait at a stop line while a conflicting path is claimed or the exit is blocked). Bodies collide
physically (`wp_physics::contact`: box colliders, sequential impulses with restitution 0.15 and body
friction, every substep); `Collision` events and `NoCollisions` report contacts. Scenes can place
`parked` vehicles (brakes on); `crash_into_bus` checks a car stops against a parked bus.
Junction control on road nodes: `Signals` (phases from opposite approaches, drawn as traffic lights)
and `Priority(major)` (minor roads give way to major-road traffic within 6 s). AI traffic obeys both;
`RanRedLight` events and `NoRedLightViolations` catch anyone entering on red. Gates: `town_traffic`
(lights at the centre) and `village_traffic` (priority crossroads, signalled T): no collisions or red
lights. Pedestrians (`wp_traffic::pedestrian`) walk a sidewalk graph built from the roads
(`wp_road::sidewalk`: sidewalk centre lines, zebra crossings at junction arms and dead ends, corner
links), cross with the lights or when no vehicle is within 5 s, and are obstacles to AI drivers, who
stop short of zebras; `HitPedestrian` and `NoPedestrianHits` (village: 30 people, none hit).
Missions (`wp_mission`, gameplay layer): road files have bus `stops`; `missions/<name>.mission.ron` lists
stops with a timetable, passengers board/alight while the doors are open, and a score (punctuality,
comfort ≤ 0.3 g, safety). `wp run --mission route_1` to play; the `Autopilot` driver lets the traffic AI
drive a scenario's mission (`wp_traffic::autopilot` waypoints, held until released). M3 gate `bus_route`:
two laps of route 1 (14 stops, ~15 min) in traffic with lights: complete, score 85, no collisions.
Mission scripts (`wp_script`): Luau via mlua (vendored C++; the Android build sets
CXX_aarch64_linux_android), sandboxed, no `os`/clock, `math.random` seeded. Hooks on_start/on_tick/
on_arrive/on_depart/on_complete/on_fail; `mission.*` API queues commands. `wp check` compiles scripts
(WP-L001 with the line); `wp docs scripting`. route_1.lua adds a school group on lap two.
Scale: `RoadNetwork::lane_at` uses a 25 m lane grid (results identical to a full scan); scene traffic
can set `radius_m` so AI vehicles that fall out of range of the player are recycled near them (big OSM
maps: Gulshan, 25 cars within 250 m of a moving player).
Weather: scene `weather: Some((rain, wetness))` → reflected `Weather` resource (live-editable); grip ×
(1 − 0.3·wetness) via Ground; AI slows (speed × √grip, longer gaps, gentler); renderer: overcast sky,
dark glossy wet asphalt, rain streaks. `car_brake_100_wet`: 57 m (dry 41 m).
Quality tiers: game.toml `[quality] tier` (low/mid/flagship; `wp run --quality`). Windows and the player
cap the internal render height (540/720/900) and upscale (`Renderer::set_max_height`, bilinear pass);
traffic ×0.6/1/1.3, pedestrians ×0.5/1/1.5, rain ×0.5/1/1. Tests and `wp shot` run without a tier. `town_traffic` gate: 12
vehicles, 3 min, no collisions. Stress (20 vehicles × 10 min × 10 seeds): cars only 0 collisions;
with 12 m buses about 1 per 10 min on the town's tight bends. Gulshan (25 cars): 0 collisions.
Assets (`wp_asset`): `.glb` under `assets/`, each with `<stem>.asset.ron` (source, author, licence
Own/Cc0/CcBy3/CcBy4/Purchased, credit). A vehicle's `model: Some("assets/vehicles/x.glb")` replaces its boxes
(origin on the ground midway between the axles; wheels stay procedural; materials named paint…/glass…/
brake…/head… get roles). `wp check`: WP-F001–F010 (licence, size, origin, tier triangle budgets).
`wp asset info|list`, `wp credits`, `wp docs assets`. The sandbox's hatchback and city bus models come from
`tools/models/vehicles.py` (our own, CC0). Levels of detail: models load with two simpler copies (vertex
clustering to ~50 % and ~25 % of the triangles, `wp_asset::simplify`); the renderer picks one per item from
size ÷ distance × the tier's `lod_bias` (Low 0.6, Mid 1, Flagship 1.6). `wp asset build` checks every model
and reports its levels against the tier budgets. Materials: glTF base colour, normal (tangents from screen-space
derivatives, no stored tangents), metallic-roughness, emissive and occlusion maps (PNG/JPEG, TEXCOORD_0), CPU-built
mipmaps, one bind group per material (group 1; neutral maps for untextured meshes), each image uploaded once and
capped to the tier's size (Low 1024, Mid 2048, Flagship 4096 px).
Shipping: `wp asset build --tier low --out <dir>` writes the game for one tier: runtime files only (no tests,
replays or docs; `--with-tests` keeps scenarios), switched-off mods left out, game.toml set to the tier, and images
inside each .glb reduced to the tier's size and re-encoded (`wp_asset::pack_glb`: colour maps → JPEG, data maps
stay PNG). `tools/android/build.sh <project> [tier]` and `tools/ios/build.sh sim|device <project> [tier]` bundle it.
Streaming: road meshes are cut into 400 m tiles (`wp_road::build_tiles`, positions relative to each tile).
Windows build them on a worker thread and keep only tiles within the tier's view distance (700/1000/1500 m)
on the GPU, uploading 4 per frame nearest first; tiles outside the view are culled. Everything is drawn
camera-relative (floating origin: `Camera::view_proj_relative`). `wp shot` stays synchronous and complete;
`wp shot --json` and window summaries report `render` stats (draw calls, triangles, tiles).
Sun shadows: one shadow-map cascade centred on the camera's target, texel-snapped, 3×3 filtered and faded
at its edge (Low 1024 px over ±30 m, Mid 2048 over ±45 m, Flagship 4096 over ±60 m; `wp shot` uses 2048 over
±45 m). Blob shadows remain as soft contact shading. `WindowOptions.quality` sets resolution, streaming and shadows.
Replays (`wp_vehicle::replay`): `.replay.ron` = setup + input changes + resets + a world hash every N ticks.
`wp run --record`, `replay.start`/`replay.stop` (control protocol), `wp replay <file>` (exit 1 + WP-Y002 at the
first differing checkpoint), `wp replay record --scenario` (goldens; 8 in the sandbox).
Event subscriptions: `events.subscribe {kinds?, entity?}` pushes `events.event` notifications as they happen
over `wp serve` (written before the step's response) and `wp run --control` (a writer thread per connection);
MCP keeps `events.poll` (WP-K009 if subscribing there).
Time of day and night: scene `time_of_day: Some((hour: 21.5, hours_per_hour: 60.0))` → reflected
`TimeOfDay` resource (live-editable; the `clock` system advances it). `Environment::at_hour` moves the sun
(east at 6, 75° south at noon, west at 18), warms it near the horizon and opens exposure at night.
Headlights come on at dusk or in heavy rain (two spot beams + tail lamps per vehicle); roads with sidewalks
get street lamps every 30 m (`RoadNetwork::lamps`, lit at dusk). The shader adds the nearest local lights
(Low 16, Mid 32, Flagship 64). Sandbox: `scenes/town_night.scene.ron`.
Mods (`waypoint::mods`): `mods/<id>/mod.toml` (name, version, priority, enabled); files mirror the game's
layout and are read through a layered `Vfs` (base, then mods by priority/id): same path replaces, new path
adds. Diagnostics point at the real file in the mod; game.toml `[mods] disabled`. `wp mod new|list`,
`wp docs mods`, WP-X001/X002.
Crash damage: the contact solver records the closing speed at first contact (`contact::Impacts`); each new
collision adds a `Dent` (body frame, depth from speed and mass share: 0 below 5 km/h, about 18 cm at 30 km/h
between equal cars, at most 70 cm) to a `Damage` component (added on first hit, so crash-free runs hash as
before) and lowers `health_pct`. `Collision` events carry `impact_kmh`; telemetry has `health_pct`. The renderer
bends the vehicle's model around its dents (`extract::vehicle::deform`, uploaded once per change).
Sound (`wp_audio`): a synthesized voice per player vehicle: firing-frequency harmonics (`engine.cylinders`),
half-order rumble, intake hiss with throttle, road roar with speed, squeal when sliding. Tuned per vehicle with
`sound: (volume, pitch, roughness, rumble, intake, road, skid)` (multipliers). Windows play it via cpal (device
opened on a thread; `wp run --mute`); `wp audio --scenario X --out f.wav` renders it headless for analysis.
Art styles: game.toml `[look]` → reflected `Look` resource (live-editable): `style = "realistic" | "toon" |
"flat"` (toon: banded light, one crisp highlight, bold colour, no grid; flat: matte pastel, wrapped light, no
reflections) plus overrides: sky_top, sky_horizon, ground, grid, asphalt, markings, sidewalk (sRGB 0–255),
saturation, fog, exposure, toon_bands. Every tier keeps the game's style; only detail drops.
Cab camera (`CameraMode::Cab`, `--camera cab`, C in windows): the driver's eye from the vehicle's optional
`cab: Some((eye_m: (x, y, z), fov_deg))` or placed from the body style, on the left for right-hand traffic and
the right for left-hand traffic, moving with roll and pitch; the cab shows a dashboard with gauges (lit at
night), a steering wheel that turns with the wheels, pillars and the roof edge. Look around: Q/E glance
left/right, mouse drag to look anywhere; the view swings back when released (`ViewOptions.look`).
WASM mods (`wp_wasm`, gameplay layer): mod.toml `code = [".wat"/".wasm"]` + `capabilities`; wasmi interpreter
(portable dispatch, no JIT: iOS-safe), 2M fuel per tick, host imports in module "waypoint" checked per capability;
`WasmPlugin` runs `on_tick` in the Agents stage after traffic (scenario drivers run later and win). Status in
the reflected `WasmMods` resource. Example: harbour_shuttle `mods/old_town_limit` (25 km/h in the old town).
Traffic sound: `wp_audio::Mixer` mixes a voice per source to stereo (constant-power pan, fade in/out);
`wp_platform::audio::sound_sources` gives the player's vehicle plus the 5 nearest within 60 m, quieter with
distance and panned by direction (windows play it; `wp audio` mixes it to mono).
Damage drives worse (`dynamics::Harm`): full power above 70 % health falling to 35 % when wrecked; a dent on a
front corner pulls the steering towards it (about 1° per 30 cm).
Traction control and ESC (`brakes.traction_control`, `brakes.esc`; on for the hatchback): TC cuts engine torque
above 12 % driven-wheel slip; ESC compares yaw rate (and sideslip) with a capped bicycle-model target and brakes
the outer front (slide) or inner rear (plough). Sine-with-dwell at 80 km/h: 90° spin without, 11° and yaw back
to 0 within 1 s with (FMVSS 126). Events `EscEngaged`/`EscReleased`; telemetry `tc_active`, `esc_active`.
Not yet: testing on physical phones; running on an iOS simulator; GPU-compressed textures (ASTC/ETC2, KTX2).
