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
```

To see what a scenario looks like at a moment, use `wp shot` and open the PNG.
Render tests skip themselves when no GPU adapter exists.

The `wp` binary is `target/debug/wp` after a build. Every command takes `--json`.

## Map

| Crate | What it owns |
| --- | --- |
| `wp_core` | Diagnostics with codes, `SimTime`, `SimRng`, stable hashing |
| `wp_ecs` | Entities, components, resources, queries, events, scheduler, reflection |
| `wp_app` | `App`, `Plugin`, plugin dependency resolution |
| `wp_data` | RON/TOML loading with line-accurate errors, JSON Schema export |
| `wp_physics` | Planar rigid bodies, `Pose`, `Velocity`, `RigidBody`, `Ground` |
| `wp_vehicle` | `.vehicle.ron`, validation, brush tyres, engine, gearbox, brakes, aero |
| `wp_world` | `.scene.ron`, spawn points, `Name`, `Player` |
| `wp_test` | `.scenario.ron`, drivers, assertions, the scenario runner |
| `wp_rhi` | wgpu device setup (headless or window), offscreen targets, PNG capture |
| `wp_render` | Frame extraction from the ECS, cameras, sky/ground/vehicle pipeline |
| `wp_platform` | Window, keyboard driving, fixed-timestep interactive loop (winit) |
| `waypoint` | `DefaultPlugins`, project loader, prelude |
| `wp_cli` | The `wp` command and the error-code catalogue |

`examples/sandbox` is a complete project and also the template for `wp new`.

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
- **Physics changes** must keep `examples/sandbox` scenarios and `wp vehicle test hatchback`
  plausible: about 8–10 s to 100 km/h, about 38–40 m to stop from 100 km/h, about 0.74 g on the skidpad.

## M0 status

Done: ECS, scheduler, plugins, data + schemas, planar vehicle physics at 480 Hz, scenario tests,
the wgpu renderer (procedural sky, gridded ground, blob shadows, lit instanced vehicles, 4× MSAA),
`wp shot`, windowed `wp run` with keyboard driving, and
`wp new/check/schema/docs/run/test/systems/plugins/vehicle test`.
Not yet: Android and iOS builds; vehicle meshes (glTF) instead of procedural boxes; reverse gear,
ABS and traction control; the control protocol and MCP server (M1).
