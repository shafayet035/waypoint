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
```

## Status

Milestone M0 (Foundations) is in progress: the simulation core, the data formats and the CLI work
headless. The renderer comes next. See [`AGENTS.md`](AGENTS.md) for the crate map and rules.

Licence: MIT OR Apache-2.0 (open question in the design).
