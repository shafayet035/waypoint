# ADR 0001: Architecture patterns

Status: accepted, 2026-11-04

## Context

Waypoint is a driving engine that AI agents operate through text, a CLI and a
protocol, and that ships to Android, iOS and desktop. It must stay
deterministic, testable headless, easy for agents to navigate, and open to
plugins and mods. A single pattern does not cover all of that, so we use a
small set, each where it fits, and enforce the important rules with tests.

## Decision

### 1. Layered architecture with a strict dependency rule

Every crate belongs to one layer and may depend only on its own layer or
lower ones. `crates/waypoint/tests/architecture.rs` fails the build if a crate
depends on anything not listed for it.

| Layer | Crates | May use |
| --- | --- | --- |
| 0 Foundation | `wp_core`, `wp_ecs`, `wp_app`, `wp_data` | layer 0 |
| 1 Simulation | `wp_physics`, `wp_world`, `wp_vehicle` | layers 0–1 |
| 2 Verification | `wp_test` | layers 0–2 |
| 3 Presentation | `wp_rhi`, `wp_render` | layers 0–1, 3 |
| 4 Agent interface | `wp_control` | layers 0–1, 3–4 |
| 5 Platform | `wp_platform` | layers 0–1, 3–5 |
| 6 Applications | `waypoint`, `wp_cli`, `wp_player` | anything |

The simulation never depends on rendering, windows or GPUs. That is what makes
headless runs, determinism and scenario tests possible (design principle 3).

### 2. Entity-Component-System with data-oriented design (runtime)

State lives in components (plain data, serde-serializable, with stable names);
behaviour lives in systems that run in explicit stages. No logic hides in
component methods that mutate other entities. This gives determinism (ordered
storage, ordered systems), reflection (JSON for every component) and exact
snapshots.

### 3. Plugin architecture (microkernel)

The core (`wp_app` + `wp_ecs`) knows nothing about vehicles or graphics.
Every feature is a `Plugin` that registers components, resources and systems,
declares dependencies, and can be disabled in `game.toml`. Engine features use
the same API as games and mods. One feature per plugin (physics, world,
vehicle, trailer, ...).

### 4. Ports and adapters (hexagonal) at the edges

The core exposes ports; the outside world plugs in adapters:

- Platform: desktop window, Android `NativeActivity`, iOS app (all `wp_platform`
  / `wp_player`) around the same `Session`.
- GPU: `wp_rhi` wraps wgpu (Vulkan, Metal, DX12); a native backend would slot in
  behind the same types.
- Agent transports: stdin/stdout (`wp serve`), TCP (`wp run --control`), MCP
  (`wp mcp`) all drive the same `wp_control::Session`.
- Data: RON/TOML files through `wp_data`; importers (OSM, OpenDRIVE) produce the
  same data types.

### 5. Event-driven messaging and command objects

Systems talk through typed events (`Events<T>`, cleared each tick) rather than
calling each other. Inputs are plain data (`VehicleInput`, control-protocol
requests): commands that can be logged and replayed.

### 6. Functional core, imperative shell (inside crates)

Physics and control laws are pure functions with unit tests (`parts.rs`,
`drivers.rs` control laws, tyre and brake models). Systems are thin shells that
read components, call those functions and write results back.

### 7. Data-driven content with schemas

Vehicles, trailers, scenes and scenarios are text files with JSON Schemas and
coded diagnostics. New content needs a file, not code.

## Code rules that follow

- One responsibility per module; split files that grow past about 500 lines.
- Public API at the crate root; internals in modules.
- Errors are `Diagnostic`s with stable codes listed in `wp_cli/src/catalog.rs`
  (a test enforces this).
- Front ends (CLI, MCP, player) share setup code (`waypoint::drive`) instead
  of repeating it.
- Refactors must keep every scenario's state hash identical unless behaviour is
  meant to change.

## Consequences

Adding a feature means: data types (with schema) → a plugin with systems →
pure functions for the maths → scenario tests. Cross-layer shortcuts fail the
architecture test, so they are discussed rather than slipped in.
