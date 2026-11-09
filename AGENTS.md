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
| `wp_world` | Scenery (buildings and trees: `scenery.rs`), `.scene.ron`, spawn points, scene traffic settings, `Name`, `Player` |
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
minimal template with data only (a night minibus route, toon look, and an electric-shuttle mod). `examples/red_canyon` shows the stylised look: a
desert loop road between layered mesas, boulders, scrub and an oasis, driven in a boxy 4x4; its models are our own CC0 work
from `tools/models/blender/` (nature.py: rocks, mesas, autumn trees, pines, palms, bushes, grass; offroader.py), generated
headless (`Blender -b -P <script> -- <out dir>`), and its scenery from `tools/scenes/red_canyon.py`.

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
  behaviour did not change. Screenshot tests (`tests/*.shot.ron`) guard the look the same way. Scenarios with a golden replay (`tests/<name>.replay.ron`) check this on
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
Bends at nodes: where a road continues at a slight angle (a two-road node within 20° of straight), the lanes get room to bend
through (`BEND_RADIUS_M` 15 m); before, their connector had no length and was rejected as an impossibly tight turn.
Long vehicles and people: a bus's outward hold in a bend (front axle off the path so the rear wheels don't cut in) is capped to
the room left in the lane when it points at the kerb, or its front rides over the pavement. Drivers watch the full width their
front sweeps in the sharpest bend of the next 20 m (κ (a² + 2aL) / 2) for anyone walking on a crossing (people standing still
don't count, so nobody waits on someone waiting for them). People waiting at a kerb step back 0.9 m, and people on a pavement
step aside 0.9 m, from a moving body within 1.2 m. Gate `town_people` (20 vehicles incl. buses, 16 people, 5 min: nobody hit);
10 seeds × 10 min × 20 vehicles: 0 hits (was 16 in 50 min), 1 vehicle collision.
Missions (`wp_mission`, gameplay layer): road files have bus `stops`; `missions/<name>.mission.ron` lists
stops with a timetable, passengers board/alight while the doors are open, and a score (punctuality,
comfort ≤ 0.3 g, safety). `wp run --mission route_1` to play; the `Autopilot` driver lets the traffic AI
drive a scenario's mission (`wp_traffic::autopilot` waypoints, held until released). M3 gate `bus_route`:
two laps of route 1 (14 stops, ~15 min) in traffic with lights: complete, score 80, no collisions.
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
cap the internal render height (540/720/900) and upscale (`Renderer::set_max_height`; Catmull-Rom plus sharpening clamped to
the nearest pixels: 0.0299 RMS against a supersampled reference where bilinear scored 0.0351, `wp_render/tests/upscale.rs`);
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
inside each .glb reduced to the tier's size and GPU-compressed (`wp_asset::pack_glb`): Basis Universal `.basis`
images (`image/x-basis`; ETC1S for colour, UASTC for normal/data maps, with mipmaps) that the renderer transcodes on
upload to ASTC 4×4, else ETC2, else BC7 (`wp_asset::basis`; wp_rhi requests whichever the GPU has), keeping textures
compressed in memory; plain RGBA if it has none. `--no-compress` packs JPEG (colour) / PNG (data) instead. `tools/android/build.sh <project> [tier]` and `tools/ios/build.sh sim|device <project> [tier]` bundle it.
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
Stylised look (`style = "stylised"`): wrapped light (`look.wrap`), sky light in shade tinted cool (`shade`), aerial haze through the
fog path (`haze`: ground fog density ≈ 1200 m visibility, 160 m height scale, sun glow), then post-processing (`wp_render::post`,
only when `Look::post()`): the scene renders linear with distance in alpha into Rgba16Float, a 5-level blur pyramid gives bloom and
the distance blur (sharp to ~2.2× the focus distance), then exposure, ACES, saturation, a split-tone grade (`highlights`, `shade`),
an S-curve and a vignette. Overrides: haze, bloom, depth_blur, vignette, shade, highlights. Other styles never take the post path.
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
Buildings (`wp_world::scenery`): a scene's `scenery: Some("scenery/<name>.scenery.ron")` holds hand-placed
`buildings` (outline + height + style) and `streets` (rows generated along roads behind the sidewalk, seeded; never on a
road, junction or each other). Styles (built in: plaster, brick, concrete, office, shophouse; games add their own):
wall colours, roof, storey_m, bay_m, window share, lit share, shop fronts. Meshes are plain data (`scenery::build_tiles`,
per-vertex sRGB colour + facade pattern); the renderer draws windows procedurally (box-filtered, so distant facades do
not shimmer), lights a share at night, casts shadows from buildings near the shadow map, and streams building tiles
like road tiles (`TileCache`). WP-B001–B007; `wp docs scenery`, `wp schema scenery`. Sandbox town and harbour have scenery.
`wp import osm` also writes `scenery/<name>.scenery.ron` from `building=*` outlines (`osm::import_with_buildings`: height
from `height`, else `building:levels` × 3 m + 0.5, else typical for the kind; style from the kind), leaving out buildings that
stand on the imported roads (a 0.8 × 0.66 km Gulshan export: 290 kept, 92 left out).
Buildings are solid: `apply_scene` turns their outlines into `wp_physics::Walls` (a 0.5 m thin box behind each face, in a
16 m grid); the contact solver stops bodies against them (immovable, same restitution and friction), records
`Impacts.walls`, and `detect_collisions` sends `Collision { other: "building" }` with the whole dent on the vehicle.
Gate `crash_into_building`: 75 km/h into a warehouse wall, stops at the wall, never through. Chase, side and
showcase cameras pull in front of the first wall between them and the car (`Scenery::first_hit`).
Props: scenery `props` (a model at a position, heading, scale, optional solid square: `Walls` labelled "prop") and `prop_rows`
(models along kerbs facing the road, skipping junctions, crossings, lamps, stops, trees, buildings; seeded); trees and tree rows
can use `model`/`models` (scaled to the tree's height); `furniture` gives the engine's street lamps (head position for the light),
bus stops and people models. Drawn by `extract::props` (one item per model part; `head…` materials glow when lit). WP-B010 values,
WP-B011 unknown model.
Trees: scenery `trees` (hand-placed) and `tree_rows` (along kerbs or verges; skip junctions, lamps, bus stops, buildings;
seeded). Kinds Round (lumpy icosphere crown), Cone, Palm (drooping fronds); built-in meshes `mesh::crown/cone/fronds`
with far versions the renderer picks by size over distance × lod_bias; drawn within 450 m of the player; trunks are
solid (`Walls` labelled "tree": `Collision { other: "tree" }`). `wp import osm` also imports `natural=tree` points.
Screenshot tests: `tests/<name>.shot.ron` (`wp_test::ShotDef`: scenario or scene/spawn/vehicle/controls, at_s, camera,
size, optional quality tier, tolerance) + the approved `tests/<name>.png`. `wp test` renders each (`Renderer::set_quality` for
a tier) and compares with `wp_render::compare` (Oklab distance, 3×3 neighbourhood both ways, % of pixels over a threshold);
failures write `target/shots/<name>.png` and `.diff.png`; `wp test --approve <name>` saves new pictures; no GPU: skipped.
WP-O001–O003. Sandbox: town_day, town_night, town_cab, town_cab_rain, town_cab_night_rain, town_wet_night, town_fog, town_night_fog, town_low, crash_building; harbour: harbour_quay. Shipping builds
leave tests/ out (only scenarios with `--with-tests`).
Windscreen rain (`extract::windscreen`): in the cab view in rain, a glass quad is blended over the scene (`fs_glass`; hidden
behind the cab, writes no depth). Drops are not stored: each sits in a cell of four grids on the glass and shows once its
seeded arrival time has passed since a blade last crossed it; `Wipers::since_wiped` (mirrored in the shader) works that out
from simulated time, so pictures stay deterministic. Tandem wipers (arms drawn as boxes) run intermittent/normal/fast by
rain; mirrored for right-hand drive. Drops show the scene upside down (sky below, road above), glint with nearby lights, and
wet glass glows round them. Rain at night now darkens the overcast sky (it used to turn day-grey). Sandbox:
`scenes/town_night_rain.scene.ron`; shots `town_cab_rain`, `town_cab_night_rain`.
Wet roads: road layers mark themselves wet (`InstanceRaw.material.w` = how readily puddles form: asphalt and markings 1,
sidewalks 0.3); with `weather.wetness` > 0, `fs_mesh` adds noise puddles (darker, mirror-smooth, dimmed sky reflection) and
`wet_streaks`: every light in view reflected through a lobe that is narrow sideways and long towards the viewer at low angles
(σ·cos θ by 2.5σ), so headlights, tail lamps and street lamps streak down the road; brightness goes with how bright a light
looks (√ intensity), not its range. Dry frames take no wet branch (pictures unchanged). Shot `town_wet_night`.
Street reflections: on wet frames the renderer first draws the scene mirrored in the road plane (sky, buildings, every mesh
instance; not roads or ground, which are the mirror) at half the internal size into a linear Rgba16Float texture (`fs_sky_mirror`,
`fs_mesh_mirror`: no tone mapping, front faces culled, nothing within 2.5 m of the eye so a cab never shows), and wet surfaces
replace the sky term with `street_reflection` (same screen position, five taps smeared down the screen by roughness). Mid and
Flagship have them (`QualityTier::reflections`), Low not; the governor drops them with the shadow map; `wp shot` has them.
`Renderer::set_reflections`. Dry frames are byte-identical with them on or off (`wp_render/tests/reflections.rs`).
Fog: scene `weather: Some((fog: 0.85))` (0–1; `Weather::visibility_m`: 1500 m × 0.04^fog, so 1.0 ≈ 60 m, 0.85 ≈ 100 m). Renderer:
`Environment::with_fog` greys the sky, dims the sun and sets `ground_fog` = 3 / visibility (extinction per m at ground level,
`Globals.weather.w`); the shader's `in_fog` integrates fog that thins with height (40 m scale, closed form along the ray), lit by the
sky and by the sun through a forward-scattering phase function, plus `fog_lights`: each local light's in-scattering along the view
ray in 8 pieces (falloff integrated exactly per piece; beam shape and phase at the middle), so headlamp beams, lamp cones and halos
show. The sky is fogged too. Clear weather takes the old path (pictures unchanged). Headlights come on in fog > 0.3. AI traffic
caps its speed to stop within sight (`wp_traffic::sight_speed_ms`: 1 s reaction, comfortable braking) and leaves longer gaps; the
world hash is unchanged without fog. Sandbox: `scenes/town_fog`, `town_night_fog`; shots `town_fog`, `town_night_fog`; gate
`town_fog_traffic` (3 min, no collisions or red lights). Cost: ~15 % frame time at Flagship (64 lights) on the M4.
Automatic quality: game.toml `[quality] auto = true` (default): the player measures the device on first launch and keeps the
tier in `settings.toml` in its data folder (`wp_platform::settings`; edit `tier` to override, delete to re-measure). The measure is
`wp_render::bench`: an engine-owned rainy night grid town (generated buildings, trees, lamps, 40 cars with headlights) drawn
offscreen at Mid settings, 1280×720, median of 40 waited frames; plus RAM (`wp_rhi::system_memory_gb`). `QualityTier::for_device`:
≤ 6 ms → Flagship, ≤ 12 ms → Mid, else Low; under 7 GB at most Mid, under 5 GB Low; a CPU rasteriser Low (thresholds are
provisional until checked on reference phones). `wp bench [--json]`; `wp run --quality auto`. Apple M4: 2.7–3.2 ms → Flagship.
Render governor (`wp_render::governor`, pure; windows and the player run it when they have a tier): watches frame intervals
(target 30 fps on Low, 60 otherwise) and the OS heat state (`wp_platform::thermal`: NSProcessInfo.thermalState on iOS/macOS,
AThermal via dlsym on Android 11+), and moves through 6 steps: render scale 1 → 0.85 → 0.7, then shadow map ½ and lights ½,
then no shadow map and ¼ lights at 0.6 scale, then a 30 fps cap (`ControlFlow::WaitUntil`). Down after 1.5 s of late frames;
up by probing after 20 s calm (a probe that slips within 5 s is undone and the wait doubles, up to 160 s); 1 s settle after each
change. Heat floors: Fair 1, Serious 3, Critical 5. The simulation is never touched (same ticks at any step). `Renderer::
set_render_scale`/`apply_step`; `wp run --thermal serious|critical` simulates heat, `--no-governor` turns it off; the window
summary reports the final step. Critical on the M4: ~29 fps drawn, ticks unchanged.
Tilt steering (`wp_vehicle::tilt`, pure): the phone is the wheel. "Up" from the motion sensor (device frame) → the angle
of turn about the screen's normal, so how far the phone leans back does not matter; the screen's top edge is the device
axis nearest up (kept until 70° past it), so either landscape works without asking the OS; straight ahead is how the phone
was held when tilt started or after a reset; 1.5° dead zone, full steering at 25°, curve ^1.5 (calm on motorways), fades to
centre as the phone nears flat. It feeds `DriverAids` as an analogue stick. `wp_platform::motion` reads the sensor (Android
NDK gravity/accelerometer on a looper thread; iOS CoreMotion gravity); touch layout with tilt: left half brakes, right half
accelerates; top right corner switches touch/tilt and saves it in settings.toml (`[controls] steering`). `wp run --steering
tilt`, `--tilt-deg <deg>` (fake sensor). Scenario driver `Tilt([(at_s, turn_deg, throttle, brake)])`; gates
`tilt_lane_change` (7° at 80 km/h: one lane, 0.25 g, no ESC) and `tilt_slalom` (±40° at 100 km/h: no spin).
Temporal anti-aliasing was tried (branch `taa-experiment`: jitter, per-object motion, clipped history) and not merged: with
4× MSAA already, it gained 7 % on still frames but was twice as far from the reference in motion. See that commit for numbers.
Terrain (`wp_physics::terrain`, `wp_world::terrain`): a scene's `terrain: Some((size_m, cell_m, noise, hills, ...))` shapes
hills and valleys on a grid (default 4 m; heights bilinear between points, never below 0, flattening to 0 over `edge_m` at the
edge). Roads are levelled into it: the land along each road is smoothed over `road_smoothing_m` into the road's height profile,
pinned to shared node heights (the land averaged round each node), levelled across the road plus `shoulder_m`, and blended back
to the land's own shape over `blend_m` (cuttings and embankments). One `HeightField` (resource `Terrain`) is what physics drives
on and the renderer draws. Physics: `Footing::on` measures the grade along and across a body between its wheels; gravity along
the ground pulls the body (`m·g·sinθ`), only `g·cosθ` presses into it, and the springs feel acceleration plus that pull (load
moves to the rear axle facing uphill: +h/L × grade). Trailers too. Climbing, the automatic gearbox only shifts up if the next
gear still out-pulls the hill (no hunting). Route drivers and AI traffic brake against downhill slopes (feed-forward), and plan
stops more gently downhill. Renderer: land tiles (smooth, or `faceted` low-poly), coloured from `colour` (else the look's
ground) to `rock` on slopes past `rock_from_grade`, drawn 6 cm under the true heights so roads (draped vertex by vertex,
normals tilted) never show it through; vehicles and trailers tilt with the ground, everything else stands on the ground under
it, buildings on the lowest ground under their outline, cameras follow up and down and stay above the land; the flat plane
sits 12 cm lower. Telemetry: `altitude_m`, `grade_pct`. WP-W009 (values), WP-W010 (grid too big). Flat scenes are unchanged
(every hash, replay and picture). Sandbox: `scenes/hills.scene.ron` (a 10 % pass); gates `hill_road`, `hill_start`,
`hill_rollback`, `bus_hill`, shot `hills`; Red Canyon has faceted red hills. Not yet: tunnels and driving under bridges (one
ground height per point), wet-road reflections on slopes (mirrored in z = 0).
Static friction on slopes: the tyre model makes grip from sliding, so a braked vehicle used to creep downhill (9 cm in 30 s on
10 %). A body standing still (under 0.2 m/s and 0.05 rad/s) now has gravity's pull held by its tyres up to what they can:
along, brake torque / radius plus rolling resistance per wheel (each capped at μ·load); across, μ·total load. Vehicles and
trailers; flat scenes unchanged.
Grass (`wp_world::grass`, `wp_render::grass` + `grass.wgsl`): a scene's `grass: Some((density_per_m2, height_m, colour,
tip_colour, draw_m, sway, verge_m, max_grade, area))`. `grass::build` makes a mask on a 1 m grid (coarser past 4M cells) over the
area (else the terrain's square, else 150 m round the roads): bare on roads, sidewalks, junctions and lane connectors plus
`verge_m` (0.7 m soft edge) and inside buildings, thinning on ground steeper than 70–100 % of `max_grade`. The GPU places the
tufts: one instanced draw, instance k = a cell of a square grid round the camera snapped to world cells (stable as the camera
moves; PCG hash per cell, not sin, which goes coarse far from the origin), kept by mask × clump noise × distance fade, rooted on
the land heights (R32Float, bilinear by hand), five triangle blades whose tips sway with simulated time (shots repeat). Lit like
meshes (wrapped light, sun shadows, local lights, light through blades towards the sun, fog, post). Reach × tier: Low 0.45,
Mid 0.75, Flagship 1 (`QualityTier::grass_reach`); at most 160k tufts. No cost or change in scenes without grass. WP-W011.
Sandbox hills: meadow (16 per m²); Red Canyon: sparse dry grass.
Water (`terrain::WaterDef`, `wp_physics::Water`): a scene's `water: [(level_m, at, colour, deep_colour, clarity_m, waves)]`
(needs terrain; WP-W012 values, WP-W013 warning without terrain). Terrain may now go below 0 (hills with negative height dig
hollows; the flat plane is not drawn inside the land's square, `Globals.land_rect`), and `flats: [(position, radius_m,
height_m, blend_m)]` level an area before the hills (an oasis, a town). `terrain::flood` assigns each grid point to a body: one
with `at` fills the hollow joined to that point below its level (lakes up in the hills), one without fills all low land and
keeps roads 0.6 m above it. Physics: depth = surface − ground under the centre; tyre grip × (1 − 0.4·min(depth / wheel
radius, 1)); drag ½ρ·0.6·|v|·v over the tyres (0.8 m² per m of depth up to 0.25 m) then the body (width ahead, length
sideways); past the vehicle's `wading_depth_m` (default 0.35; the off-roader's snorkel 0.7) the engine floods for good
(`EngineFlooded`, `Vehicle.flooded` serialised only when true, so other hashes are unchanged). Telemetry `water_depth_m`,
`engine_flooded`. Rendering: water tiles per body over cells with a wet corner (depth per vertex, colours in the vertex),
`fs_water` after the opaque pass: depth-tinted body, sky gradient reflected by Fresnel through four moving ripple trains (calmer
with distance), a tight sun glint through fine noise, broken foam where it laps the shore, premultiplied with alpha kept for
post. Cameras stay 0.8 m above water. Road meshes on terrain are refined first (no edge over 2 m) so the land never shows
through large junction triangles. Sandbox `scenes/lake.scene.ron`: `ford_shallow`, `pond_flood`, shot `ford`; Red Canyon:
oasis pond (`pond` spawn, shot `canyon_oasis`).
Jumps (`Vehicle.vertical`, only on terrain, so flat-scene hashes are unchanged): the body rides on the mean ground height under
its four contact points (`Footing::contacts_m`, smoothing the grid's kinks over the wheelbase); the vertical acceleration the
ground imposes (filtered over 0.1 s, like a suspension) scales the weight pressing down (× 0–3: heavier in dips, lighter over
crests). Past −1 g above 5 m/s the vehicle leaves the ground: a projectile with no tyre forces or slope pull, tilted as it left;
it lands when it meets the ground (load spike from the closing speed over 0.12 s). `Airborne`/`Landed { air_s, impact_ms }`
only for flights of 0.1 s or more (the crest flickers do not count); telemetry `airborne`. Sandbox `scenes/jump.scene.ron`
(a 2.5 m bump, take-off from about 43 km/h): `jump_fast` (90 km/h: one 0.6 s jump, no spin), `jump_slow` (30 km/h: stays
down), shot `jump`.
Bridges: a road with `bridge: true` (on terrain) runs straight between its end nodes' heights (clamped above water) instead of
following the land; only its first and last 12 m (`ABUTMENT_M`) level the land. Each becomes a `wp_physics::Deck` (centreline
every 2 m, heights, half width = carriageway + sidewalk + 0.5 m) in `HeightField::decks` (16 m lookup grid): `height(p)` is the
deck where there is one (else the land; `land_height`/`land_gradient` for the land itself), so physics, road draping, telemetry
and water depth all see the deck, and a car that left the deck would fall to the land. The land under a deck is cut 0.9 m below
it (not at the ends); no grass under decks. Side walls (0.3 × 0.9 m, `terrain::parapets`) are solid `Walls` labelled "bridge".
Drawn as cubes (`extract::bridge_items`): a 0.8 m concrete slab in ~8 m pieces following the deck, walls, and pillars every
24 m down to the land. WP-N012 now also accepts a road whose lanes all lead on and are led into (a line between dead ends has no
loop). Sandbox `scenes/gorge.scene.ron` (plateaus 18 m up, a 140 m bridge over a pond): `bridge_cross` (60 km/h, 0.01 m off the
lane centre), `bridge_wall` (steered into the wall: collision, stays on the deck), shot `bridge`.
Not yet: testing on physical phones; running on an iOS simulator.
