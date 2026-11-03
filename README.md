# Waypoint

An agent-first engine for bus, truck and car driving games, written in Rust. An AI coding agent
can create, run, test and fix a game entirely through text files and the `wp` command line.
Games will ship on Android and iOS. See [`architecture-design.md`](architecture-design.md).

## Quick start

```sh
cargo build
./target/debug/wp new my_game
cd my_game
../target/debug/wp check
../target/debug/wp test
../target/debug/wp vehicle test hatchback
../target/debug/wp run --headless --throttle 1 --seconds 8 --trace
../target/debug/wp shot --scenario skidpad --at 10s --camera showcase --out skidpad.png
../target/debug/wp run        # drive in a window: W/S throttle-brake, A/D steer, C camera, R reset
```

## Use it from Claude Code (MCP)

```sh
cargo build
claude mcp add waypoint -- "$PWD/target/debug/wp" --project "$PWD/examples/sandbox" mcp
```

Claude then gets tools to check and test the project, take screenshots, and drive a live
simulation (`session_start`, `session_call` with control-protocol methods such as `sim.step`,
`input.set`, `world.find`, `render.screenshot`).

## Status

Milestone M0 (Foundations): the simulation core, data formats, CLI and a first wgpu renderer
(windowed and headless) work on desktop. Android and iOS builds come next. See [`AGENTS.md`](AGENTS.md) for the crate map and rules.

Licence: MIT OR Apache-2.0 (open question in the design).
