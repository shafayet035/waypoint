# wp_core

Foundation types with no engine dependencies.

- `Diagnostic`: code + message + location + expected + help. Render with `.render()` (text) or serde (JSON).
- `SimTime`: 60 Hz ticks, 8 physics substeps. Use `dt()` for gameplay and `physics_dt()` in Physics systems.
- `SimRng`: seeded xoshiro256**. Use `SimRng::fork(seed, "subsystem")` for independent streams.
- `StableHasher`: FNV-1a. Never use `std::hash::DefaultHasher` for anything persisted or compared across runs.

Pitfall: `SimTime::seconds()` comes from the tick count; do not accumulate `dt` yourself.
