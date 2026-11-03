# wp_vehicle

Two plugins: `VehiclePlugin` and `TrailerPlugin` (trailer.rs, needs `vehicle`). The vehicle step
(`dynamics.rs`) is an orchestrator, `step_vehicle`, over named phases: `update_steering`,
`measure_acceleration`, `update_body_motion`, `wheel_loads`, `update_drivetrain`, `update_brakes`,
`tyre_forces`, `add_body_forces`. Keep their order: it is part of determinism.

Each physics substep (480 Hz):
steering → body roll/pitch (damped springs) → wheel loads (longitudinal + lateral transfer) →
tipping about the outer wheels → drivetrain and automatic gearbox (shifts on road speed, speed
governor) → brakes (hydraulic or air: reservoir, compressor, chamber lag) → per wheel: sliding
friction, relaxed slip, brush tyre force, ABS valve, wheel spin → body force and yaw moment → drag.

- Part models are pure functions in `parts.rs` (tyre, sliding friction, ABS, air brakes); test there.
- Notable moments are sent as `VehicleEvent`s (Events resource, cleared each tick).
- Low-speed handling: slip states are capped at the sliding limit, relaxation length shrinks
  below 3 m/s and a contact damper fades in. Without these a stopped vehicle rocks on its tyres.
- Rollover: when |ay| > g·track/2h the vehicle pivots about its outer wheels (rigid-body equation
  about that edge); past the balance angle it is `rolled_over` and slides on its side.
- New definition fields must be optional (`#[serde(default)]`) so existing files keep loading.

Validation codes WP-V001..V090 live in `def.rs`; keep messages pointing at the field.
