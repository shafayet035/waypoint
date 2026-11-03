# Agentic Driving Engine — Architecture Design

Nov 3, 2026 · @Shafayet Hossain

Source: https://claude.ai/artifact/SaZtW1iVHMa9BsJmQZTFSX

## Summary and goals

We are building a from-scratch, cross-platform Rust engine for bus, truck and car driving games. Its first user is an AI coding agent: a game is something an agent can create, run, test and fix entirely through text, a CLI and a machine-readable API. A human then plays it on an Android or iOS phone or tablet. Working name: **Waypoint** (open question below).

**What "agentic" means here.** An AI agent (Claude Code, Cursor, a custom LLM loop) can do every step of game development without a GUI editor or human eyes: scaffold a project, write vehicles, maps and missions as text files, run the game headless, query the live world, take screenshots, replay a session and read structured errors that say how to fix them.

**Goals for v1.0**

1. **Agent-operable.** Any feature reachable in the editor is also reachable from files, the `wp` CLI and the control protocol. Nothing requires a mouse.
2. **Realistic simulation.** Physics validated against real vehicle data: tyres, suspension, drivetrain, air brakes, articulated trailers, weather-dependent grip and damage.
3. **High-end mobile graphics.** Physically based lighting, sky, weather and night lighting built for phone GPUs; ray-traced shadows and reflections as an optional extra on flagships that support them.
4. **Scales from older phones to flagships.** One set of game content runs on three quality tiers, from older budget phones to current flagships. Driving physics (tyres, suspension, drivetrain, brakes) stays equally realistic on every tier; only crash-deformation detail and background simulation scale down.
5. **Living roads.** A lane-level road network with AI traffic, signals, junctions and bus stops with passengers.
6. **Cross-platform.** Games ship on Android and iOS. Development tools (the wp CLI, headless runs, tests) run on Windows, macOS and Linux.
7. **Deterministic and headless.** The same inputs and seed give the same result, so agents can write tests that pass or fail reliably.
8. **Modular and moddable.** The engine is a framework of plugins. Developers extend it in Rust; players mod games with data, Lua and sandboxed WASM, without the game's source code.

**Non-goals for v1.0:** multiplayer netcode, web and desktop game builds, console ports, a general-purpose engine for non-driving genres, and a full visual editor (a debug inspector, yes; a Unity-style editor, no).

## Design principles for AI agents

Every design choice is judged by one question: can an agent with only a terminal do this, see the result and know whether it worked? Eight rules follow from that.

| # | Principle | What it means in practice |
| --- | --- | --- |
| 1 | Text is the source of truth | Scenes, vehicles, roads, missions and settings are plain-text files (RON or TOML) that diff cleanly. No binary scene files; binary assets (meshes, audio) are referenced by path. |
| 2 | Everything has a schema | Every data type exports a JSON Schema via `wp schema`. Agents validate files before running, and editors get autocomplete. |
| 3 | Headless is first-class | The full simulation runs with no window and no GPU. Rendering is a consumer of sim state, never a dependency of it. |
| 4 | Deterministic by default | Fixed timestep, seeded RNG, ordered systems. A recorded input stream replays to the same result on the same build. |
| 5 | Introspectable at runtime | Any entity, component, resource or system can be listed, read and changed over the control protocol while the game runs. |
| 6 | Errors teach | Every error has a stable code, the file and line, what was expected, and a suggested fix, in both human and JSON form. |
| 7 | Fast feedback | `wp check` validates a project in under 2 s; hot reload applies data changes without restarting; a headless 60 s scenario runs in seconds. |
| 8 | Small, documented surface | One obvious way to do each thing. Each crate ships an `AGENTS.md` with its concepts, examples and pitfalls, kept in sync by tests. |

What agents struggle with most, and how these rules answer it: guessing APIs (schemas, docs in-repo), not seeing the screen (screenshots, world queries), flaky tests (determinism), and slow loops (headless and hot reload).

## Architecture overview

The engine is six layers. The agent layer on top can reach every layer below it, while the simulation never depends on rendering, which is what makes headless runs and deterministic tests possible.

*Diagram: Agents reach every layer of the engine through one agent layer (top to bottom).*

| Layer | Role | Parts |
| --- | --- | --- |
| Agent layer | AI coding agents | wp CLI, MCP server, Control protocol, Scenario tests |
| Game | games and mods | Data files (RON/TOML), Lua missions, Rust game systems |
| Simulation | deterministic, headless | Vehicle, Physics, Road network, Traffic AI, World |
| Presentation | reads sim only | Renderer (3 tiers), Audio (cpal), Inspector (egui) |
| Core | built in-house | ECS + scheduler, Reflection, Data + schemas, Maths, time |
| Platform | Android, iOS | Window (winit), Touch + sensors, File system, Threads |

*Lower layers never depend on higher ones; rendering reads a copy of sim state each frame.*

Data flows one way each tick: input and agent commands go in, the scheduler runs simulation systems, then presentation reads a snapshot. Agents see the result through queries, events and screenshots.

## Modularity, plugins and modding

The engine is a framework of plugins. Every feature we ship, from the tyre model to the traffic AI and the lighting pass, is a plugin built on the same public API that game developers and modders use. A game can remove, replace or extend any of them.

**Three ways to extend**

| Who | Extends with | Can do | Sandboxed |
| --- | --- | --- | --- |
| Engine and plugin developers | Rust crates implementing `Plugin` | Anything: new components, systems, render passes, physics parts, CLI and RPC commands | No (compiled in) |
| Game developers | Rust game crates, Lua scripts, data files | Gameplay, vehicles, maps, UI, missions | No |
| Modders and players | Mod packages: data, assets, Lua, WASM modules | Add or override vehicles, liveries, maps, missions, traffic, sounds, materials, new systems | Yes: capability permissions per mod |

**Plugin system (`wp_app`).** A game is an `App` plus a list of plugins. Each plugin registers what it adds, declares dependencies with version ranges and can be switched on or off in `game.toml`. `DefaultPlugins` is just a bundle, so a game can replace one part (say, its own traffic AI) and keep the rest. `wp plugins` lists what is loaded and in what order.

**Stable extension points.** Each is a Rust trait with a matching data and scripting form, versioned and documented:

- Vehicle parts: engines, gearboxes, tyre models, brake systems, suspension types, electronics (ABS, ESC, retarder logic).
- Traffic and pedestrian behaviours, route planners, mission and timetable logic.
- Render passes, post effects, material models, weather effects.
- Importers for assets and maps (glTF, OSM, OpenDRIVE, or a modder's own format).
- Input devices, UI screens and HUD widgets, audio sources.
- CLI commands, control-protocol methods and scenario-test assertions.

**Mod packages.** A mod is a folder or `.wpmod` archive with a `mod.toml`: id, version, engine API version, dependencies, load order and requested permissions. The virtual file system layers mods over the base game, so a mod adds files or overrides them by path. Data patches change single fields without copying whole files, which keeps mods from conflicting. Tools: `wp mod new`, `wp mod check` (schema validation and a conflict report), `wp mod pack`, with hot reload in dev mode.

**Code in mods.** Lua covers most mod logic. For heavier logic, mods ship WASM components (run with `wasmtime`) against an API defined in WIT. Rust has no stable binary interface, so WASM is the stable, cross-platform and sandboxed boundary. iOS forbids JIT compilation, so WASM runs in an interpreter there. Loading native libraries is off by default and only allowed for trusted local development.

**Compatibility.** The mod API uses semantic versioning. Deprecated APIs keep working with warnings for at least one minor version, and `wp migrate` updates old data files. The package format is ready for mod.io or an in-game mod browser, but a distribution service is out of scope for v1.0.

## Core runtime

The core is a custom archetype ECS driven by a fixed-timestep scheduler, with a thin platform layer underneath. "From scratch" means we own the engine's architecture and every gameplay system; we still use a few foundational crates where writing our own adds no value.

**ECS (`wp_ecs`).** Archetype storage, typed queries, resources and events. Every component derives `Reflect` (name, fields, types, docs) and `Serialize`. Reflection lets the control protocol and save files work for any component, including ones a game defines. We write our own instead of using `hecs` or `bevy_ecs` so that reflection, determinism and stable entity IDs are built in rather than bolted on.

**Scheduler.** Systems declare their reads and writes. The scheduler builds a deterministic order: explicit stages (`Input → Agents → Physics → Post → Render-extract`), stable ordering inside a stage, and parallelism only between systems with no conflicting access. `wp systems` prints the resolved graph.

**Time.** Physics runs at a fixed 480 Hz for the player and nearby vehicles (tyres need small steps) and 120 Hz for mid-distance vehicles; gameplay and AI at 60 Hz, so every physics rate divides evenly into a gameplay tick; rendering at display rate with interpolation. Headless mode can run faster than real time or step exactly N ticks.

**Determinism rules.** No `HashMap` iteration in sim code (use ordered maps or sorted IDs); one seeded RNG per subsystem; no wall-clock reads in the sim; `f32` maths without fast-math flags. Guarantee: bit-identical replays on the same build and platform. Cross-platform bit-identity is a stretch goal (see Risks).

**Platform layer (`wp_platform`).**

| Concern | Choice | Why |
| --- | --- | --- |
| Windows and events | `winit` | Covers Android and iOS, plus desktop for development. |
| GPU | `wgpu` | Portable base for every tier, wrapped by our own RHI (see Rendering engine). |
| Maths | `glam` | Fast, SIMD, widely known to agents. |
| Touch, tilt and controllers | `winit` touch events, platform motion sensors and controller APIs | Touch steering and tilt are the main controls on phones. |
| Audio output | `cpal` (own mixer on top) | Engine and tyre sounds need custom synthesis. |
| Files and assets | Own virtual file system | Same paths for loose files, packed archives and mod layers. |

## Rendering engine

One renderer serves three mobile quality tiers. Flagship phones get the richest lighting and optional ray-traced effects; older phones get the same content with simpler lighting and baked probes. Games and mods never ship separate assets per tier.

**Render hardware interface (`wp_rhi`).** Our own thin layer over `wgpu`, which runs on Vulkan (Android) and Metal (iOS), and on desktop APIs for development machines. wgpu exposes ray queries as experimental features and its ray-tracing pipelines are still in development ([wgpu ray tracing docs](https://wgpu.rs/doc/wgpu/documentation/extensions/ray_tracing/index.html)), so ray tracing stays optional. A native Vulkan or Metal path is added only if wgpu blocks a mobile feature we need.

**Frame pipeline (all tiers)**

- **Bandwidth first:** phone GPUs render in tiles, and memory bandwidth costs both frame time and battery. Passes are grouped so lighting runs in on-chip tile memory, with no full-screen round trips where avoidable.
- **Geometry:** GPU culling, classic LODs, impostors and instancing; city blocks are built from instanced modular pieces so they render in few draw calls.
- **Lighting:** clustered forward shading for many street lights, headlights and signals at night.
- **Global illumination:** baked lightmaps and probes for the static world, blended across times of day; probes update dynamically on Flagship.
- **Reflections and shadows:** reflection probes on every tier; screen-space reflections on wet roads (Mid and Flagship); cascaded shadows with a tier-sized shadow map. Ray-traced shadows and reflections are an optional extra on flagships with hardware ray tracing.
- **Materials:** layered PBR, including car paint with clearcoat, wet and dry blending driven by the weather sim, puddles, rain-covered glass, retroreflective signs and road markings; cheaper variants on lower tiers.
- **Sky and weather:** physically based atmosphere, rain, windscreen rain with working wipers and road spray on every tier; volumetric clouds and fog on Flagship only.
- **Camera:** physical exposure; our own temporal anti-aliasing and upscaling on every tier, plus MetalFX upscaling on iOS where available.
- **Streaming:** textures and meshes streamed per world tile, sized to each tier's memory budget, since phones share one memory pool between CPU and GPU.
- **Power:** the engine reads the OS thermal state and lowers render cost (resolution, then effects, then frame cap) before the phone throttles. Simulation is never touched.

**Quality tiers.** The engine benchmarks the device on first run and picks a tier. Every setting also lives in `settings.toml`, so agents can test each tier headless. Targets below are design goals, to be checked on reference phones, and must hold through a 20-minute drive.

| Tier | Example device class | Target | Lighting | Geometry | Traffic radius |
| --- | --- | --- | --- | --- | --- |
| Low | Older or budget phones, 4 GB RAM | 30 fps, upscaled from about 540p | Baked probes and lightmaps, one shadow cascade | Aggressive LODs and impostors | Small |
| Mid | Mainstream phones from the last three years, 6–8 GB RAM | 60 fps, upscaled from about 720p | Baked GI, screen-space reflections, cascaded shadows | LODs with GPU culling | Medium |
| Flagship | Current flagships with hardware ray tracing, 8 GB+ RAM | 60 fps, upscaled from about 900p | Dynamic probes, volumetrics, optional ray-traced shadows and reflections | Full detail with GPU culling | Large |

## Physics engine

Realism lives here. The player's vehicle runs at 480 Hz with detailed tyre, suspension and drivetrain models, and that fidelity is the same on every tier. Only crash-deformation detail and background work (traffic count, distant vehicles, debris) scale down on older phones.

- **Solver (`wp_physics`):** our own rigid-body and articulated-body solver with sub-stepping. 480 Hz (8 steps per 60 Hz gameplay tick) for the player and nearby vehicles, 120 Hz for mid-distance vehicles, kinematic or mesoscopic beyond that. Deterministic, multithreaded per simulation island, SIMD-friendly data layout.
- **Tyres:** a physical brush model with temperature, wear, pressure, camber and relaxation length, not just a fitted curve. Grip comes from the road surface (asphalt, concrete, cobbles, gravel, snow, ice) and water depth from the weather sim, so aquaplaning emerges naturally.
- **Suspension:** real kinematics for multi-link, double wishbone and solid axles; leaf springs for trucks; air suspension with ride-height levelling and bus kneeling; anti-roll bars and bump stops.
- **Chassis:** optional torsional flex for long frames such as articulated buses and heavy trucks.
- **Powertrain:** engine torque maps by RPM and load, turbo spool, exhaust brake, retarder, clutch heat, automatic and AMT shift logic, fuel use; battery-electric drivetrains with regenerative braking.
- **Brakes:** a full air system (compressor, tanks, valve delays, ABS and EBS) and brake fade with temperature.
- **Aerodynamics:** drag and crosswind sensitivity, which matters for high-sided buses and trucks.
- **Articulation and cargo:** fifth wheels, drawbars, multiple trailers and articulated buses; cargo mass that moves the centre of gravity; liquid slosh in tankers.
- **Damage:** node-beam soft bodies for crash deformation, plus mechanical damage (bent steering, burst tyres, broken air lines). The player's soft-body resolution scales with the tier (full on Flagship; on Low, mesh deformation without a full node-beam body); AI vehicles use a simplified model.
- **Validation:** every vehicle type has a test suite that compares stopping distance, acceleration, turning circle and rollover threshold against published data. Out-of-band results fail CI.

## Driving modules

On top of the physics engine, these modules make it a driving engine rather than a generic one. Each is a crate with its own data format, so an agent can make a new vehicle or map by writing a file, not code.

**Vehicles (`wp_vehicle`).** The vehicle part models described under Physics engine (tyres, suspension, powertrain, brakes, aero, damage) live in this crate and run on the generic solver in `wp_physics`. A vehicle is a `.vehicle.ron` file listing its parts (wheels, axles, suspension, tyres, powertrain, brakes), masses and mesh, so modders can build one from existing parts or plug in new ones. `wp vehicle test city_bus --manoeuvre brake_80_0` runs standard tests headless and prints stopping distance, peak lateral g and so on.

**Road network (`wp_road`).** A lane graph is the core data: roads as clothoid or spline centrelines with lane profiles, junctions with lane connections, signals, signs, speed limits, bus stops and depots. The graph drives three things: mesh generation (road surface, markings, kerbs), navigation, and traffic rules. Import from OpenStreetMap and OpenDRIVE is planned so agents can build real places.

**Traffic AI (`wp_traffic`).** Each NPC vehicle is an agent with three layers: route planner (A\* on the lane graph), behaviour (lane following with IDM car-following and MOBIL lane changes, junction negotiation, signal obedience, yielding to buses leaving stops) and control (steering and pedal controllers that drive the real vehicle model). Far-away traffic switches to a cheap mesoscopic model, and the traffic radius scales with the quality tier. Pedestrians and passengers use the same pattern on a sidewalk graph.

**World streaming (`wp_world`).** The map is split into tiles that load and unload around the player on background threads. Floating origin keeps precision on maps tens of kilometres wide.

**Audio and input.** Engine sound is synthesised from RPM and load (granular blending of recorded samples), with tyre, wind and air-brake layers. Input maps touch controls (on-screen wheel, pedals and buttons), tilt steering and Bluetooth controllers to actions, with haptic feedback from tyre aligning torque and the road surface.

## The agent interface

Agents reach the engine through four layers, from static to live: project files, the `wp` CLI, the control protocol and an MCP server. All four expose the same concepts with the same names.

**1. Project files.** A game is a folder an agent can read top to bottom:

```
my_bus_game/
  game.toml            # name, version, start scene, enabled modules
  AGENTS.md            # generated: what this project contains, how to test it
  scenes/city.scene.ron
  vehicles/city_bus.vehicle.ron
  roads/downtown.road.ron      # or imported .osm / .xodr
  missions/route_42.mission.lua
  tests/route_42.scenario.ron
  assets/              # meshes (glTF), textures, audio
  src/                 # optional Rust game code (custom systems)
```

Gameplay logic lives in two places: Rust crates for systems that need speed or new components, and **Lua (Luau via `mlua`)** for missions, timetables and dialogue, which agents write well and which hot-reload instantly. Both call the same documented API.

**2. The `wp` CLI.** Every command takes `--json` and returns a stable structure.

| Command | What it does |
| --- | --- |
| `wp new <name> --template bus` | Scaffold a project from a template (bus, truck, sandbox). |
| `wp check` | Validate every file against schemas and cross-references; no run needed. |
| `wp schema <type>` | Print the JSON Schema for a data type (vehicle, scene, mission). |
| `wp docs <topic>` | Print API docs and examples offline, for the installed version. |
| `wp run [--headless] [--ticks N] [--seed S]` | Run the game headless on the development machine, or on a connected phone or emulator. |
| `wp test` | Run all scenario tests and report pass/fail with metrics. |
| `wp shot --scene city --camera chase --at 12.5s` | Render a frame to PNG, headless (software or GPU). |
| `wp replay <file>` | Replay a recorded session, optionally to a video. |
| `wp build --target android` | Package for Android or iOS. |

**3. Control protocol.** A running game opens a JSON-RPC 2.0 endpoint (WebSocket or stdio). Methods are grouped by purpose:

- **Query:** `world.entities`, `world.get`, `world.find` (e.g. all vehicles within 50 m of the bus), `road.lane_at`, `sim.metrics`.
- **Act:** `input.set` (throttle, brake, steer for any vehicle), `world.spawn`, `world.set`, `mission.trigger`.
- **Control time:** `sim.pause`, `sim.step` (N ticks), `sim.speed`, `sim.snapshot` and `sim.restore`.
- **Observe:** `render.screenshot` (PNG or base64), `render.camera`, `log.subscribe`, `events.subscribe` (collisions, red-light violations, passenger boarded).

This same API lets AI or RL agents *drive* vehicles, CARLA-style, at no extra cost.

**4. MCP server.** `wp mcp` wraps the CLI and control protocol as Model Context Protocol tools and resources, so Claude and other agents can use the engine without any glue code. Docs and schemas are served as MCP resources.

**Error format.** Every diagnostic has the same shape, in text and JSON:

```
error[WP-V012]: gearbox ratio count does not match `gears`
  --> vehicles/city_bus.vehicle.ron:41:5
  expected 6 ratios, found 5
  help: add a ratio for gear 6, e.g. 0.74, or set gears: 5
  docs: wp docs errors/WP-V012
```

## Agent feedback loop

An agent knows its change worked only when a test says so, so scenario tests are the engine's main verification tool. The intended loop for every change:

1. **Edit** a data file, Lua script or Rust system.
2. **Check** with `wp check --json`: schema, references and type errors in under 2 s.
3. **Run** the relevant scenarios headless with `wp test route_42`.
4. **Inspect** failures: metrics, the event log, a screenshot at the failure tick, and a replay file.
5. **Fix** and repeat. If it passes, record a golden replay so future changes are caught.

**Scenario files** describe setup, scripted or AI inputs, and assertions:

```
// tests/route_42.scenario.ron
(
  scene: "scenes/city.scene.ron",
  seed: 7,
  player: (vehicle: "city_bus", spawn: "depot_north", driver: Ai(route: "route_42")),
  duration: 600s,
  assert: [
    MissionComplete("route_42"),
    NoCollisions,
    MaxLateralG(0.35),          // passenger comfort
    StopsServed(min: 12),
    FrameBudget(sim_ms: 4.0),   // performance guard
  ],
)
```

**Other verification tools**

- **Golden replays:** a recorded input stream plus a hash of world state every N ticks. A changed hash shows the first tick that diverged.
- **Visual snapshots:** `wp shot` frames compared against stored images with a perceptual diff threshold, so agents can catch broken rendering without looking. Each reference scene is checked at every quality tier, on reference phones in a device lab.
- **Vehicle benchmarks:** standard manoeuvres (0–80 km/h, brake test, lane change, tight turn with trailer) with pass bands, so tuning stays plausible.
- **Performance budgets:** per-system timings in every test report; a regression fails the test. Each tier has its own frame-time budget, measured nightly on reference Low and Flagship phones, including a 20-minute thermal soak.

**Engine self-tests.** The engine repo uses the same tools on itself: every example game in `examples/` is a scenario test in CI, headless on Linux and on Android and iOS emulators.

## Repository layout

One Cargo workspace, one crate per module, with a strict rule: lower layers never depend on higher ones, and nothing in `sim` depends on `render`. CI enforces this with a dependency check.

| Crate | Layer | Responsibility | Key external deps |
| --- | --- | --- | --- |
| `wp_core` | Foundation | Maths, IDs, time, logging, errors with codes | `glam`, `tracing` |
| `wp_ecs` | Foundation | ECS, scheduler, reflection, events | none |
| `wp_app` | Foundation | `App`, `Plugin` trait, plugin dependencies and load order | none |
| `wp_data` | Foundation | RON/TOML loading, JSON Schema export, data patches, hot reload | `serde`, `ron`, `schemars` |
| `wp_platform` | Platform | Window, touch and sensor input, thermal state, virtual file system with mod layers, threads | `winit` |
| `wp_rhi` | Platform | GPU abstraction over wgpu (Vulkan, Metal) | `wgpu` |
| `wp_physics` | Simulation | Rigid, articulated and soft-body solver, collision | none (own solver) |
| `wp_vehicle` | Simulation | Tyres, suspension, powertrain, brakes, aero, damage parts | none |
| `wp_road` | Simulation | Lane graph, mesh generation, OSM/OpenDRIVE import | none |
| `wp_traffic` | Simulation | NPC vehicles, pedestrians, passengers | none |
| `wp_world` | Simulation | Scenes, tile streaming, weather, time of day, save/load | none |
| `wp_script` | Gameplay | Lua runtime and bindings | `mlua` (Luau) |
| `wp_mod` | Gameplay | Mod loading, permissions, WASM host | `wasmtime` |
| `wp_render` | Presentation | Frame pipeline, quality tiers, headless capture | `wp_rhi` |
| `wp_audio` | Presentation | Mixer, engine sound synthesis | `cpal` |
| `wp_control` | Agent | JSON-RPC server, snapshots, replay | `tokio`, `serde_json` |
| `wp_test` | Agent | Scenario runner, assertions, golden replays, image and perf checks | none |
| `wp_cli` | Agent | The `wp` binary, including `wp mcp` and `wp mod` | `clap` |
| `wp_inspector` | Tools | Debug overlay and entity inspector for humans | `egui` |
| `waypoint` | Facade | `DefaultPlugins` and a prelude for game and mod code | none |

Alongside the crates: `examples/` (bus route, truck delivery, sandbox), `templates/` for `wp new`, `docs/` (the source for `wp docs`) and a root `AGENTS.md`.

## Roadmap

Seven milestones take the engine to v1.0 in about 27 months, assuming 6–8 engineers (including at least 2 for graphics and 2 for physics) working alongside AI agents. Low and Mid tiers ship before the Flagship tier, so the game runs well on ordinary phones first. Agent tooling ships from M0, not at the end: the CLI and schemas come first, and the control protocol and MCP server arrive with the first drivable vehicle.

*Diagram: Seven milestones to v1.0, each ending in a test that proves it.*

| Milestone | Duration | Scope | Exit gate |
| --- | --- | --- | --- |
| M0 Foundations | est. 3 months | ECS, plugins, scheduler, data + schemas; RHI on wgpu, wp check, wp schema | Car drives on a flat plane; headless test passes |
| M1 Realistic drive | est. 4 months | 480 Hz solver, tyres, suspension, powertrain; bus and car, control protocol, MCP server | Bus passes brake and acceleration validation |
| M2 Roads and trucks | est. 4 months | Lane graph, road meshes, OSM / OpenDRIVE import; trucks, trailers, full air-brake system | Truck backs a trailer into a dock in a test |
| M3 Living city | est. 4 months | Traffic AI, pedestrians, Lua missions, streaming; Low and Mid tier renderer, weather | 10-minute bus route at 30 fps on a Low-tier phone |
| M4 Flagship visuals | est. 5 months | Flagship tier: dynamic probes, volumetrics; optional ray tracing, MetalFX upscaling | Flagship scenes pass image and 20-minute thermal tests |
| **M5 Agents and mods** | est. 3 months | Mod SDK, WASM host, templates, wp docs; golden replays, second game built by an agent | **An agent builds a new game and a mod, unaided** |
| M6 Ship 1.0 | est. 4 months | Soft-body damage, engine audio, touch and tilt; 1.0 docs, asset pack | v1.0 released for Android and iOS |

M5 is the real test of the design: if an agent cannot build a second game and a mod using only `wp new`, the docs and the tests, the agent interface is fixed before shipping. Durations are estimates to revisit after M0.

## Risks and open questions

The biggest risk is scope: a from-scratch engine plus credible heavy-vehicle physics is years of work, so every milestone must ship a playable, testable game.

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Scope: realistic physics, high-end mobile graphics and a from-scratch engine at once | Nothing playable for a long time | Every milestone ends in a working game; Low and Mid tiers ship before the Flagship tier; build in-house only the ECS, physics solver and renderer, where determinism, reflection and vehicle realism need it; use proven crates for everything else (windowing, GPU access, maths, audio output, scripting, WASM). |
| Specialist skills needed for rendering and vehicle physics | Slips and poor quality | Plan for at least 2 graphics and 2 physics engineers; AI agents handle tooling, tests, content and glue code. |
| wgpu ray tracing is experimental, and few phones support it | Flagship ray-traced effects blocked | Ray tracing is optional; a native Vulkan or Metal path only if needed. |
| Looking great on flagships while running on older phones | Two renderers to maintain and test | Shared content and frame graph; per-tier image and frame-time tests on reference phones every night. |
| 480 Hz physics and soft-body damage too heavy for older phone CPUs | Frame drops, heat and battery drain on Low | Player fidelity fixed; scale traffic count, soft-body detail and distant-vehicle rate per tier; per-system budgets in tests. |
| Realism without real vehicle data | Vehicles feel wrong | Validation suites against published specs; partner with bus and truck operators for data. |
| Cross-platform float determinism | Replays and golden tests differ between OSes | Guarantee same-platform determinism only; hash tolerances in cross-platform CI. |
| Mod API churn, or unsafe mods | Broken mods, security issues | Semantic versioning, deprecation windows, `wp migrate`; mods sandboxed in Lua and WASM with permissions; no native DLLs by default. |
| Agents cannot judge visual realism | The look regresses unnoticed | Reference-image tests with perceptual metrics; human art review at each milestone. |
| Phones throttle when hot | Frame rate falls during long drives | Lower render cost first using the OS thermal state; 20-minute soak tests per tier. |
| App store rules on downloaded code | Lua and WASM mods rejected on iOS | Check Apple and Google policies before M5; fall back to reviewed content packs on iOS. |

**Open questions**

- [ ] Final engine name (Waypoint is a placeholder).
- [ ] Exact reference phones for the Low and Flagship tiers.
- [ ] Shader language: WGSL only, or Slang compiled to each backend?
- [x] Scripting: decided on Luau via mlua, since agents and modders know Lua well. Rhai (pure Rust) was the alternative.
- [ ] Licence: MIT/Apache-2.0 dual licence (Rust norm), or something else?
- [ ] Which first game should drive the engine: a city bus route or a long-haul truck delivery?
