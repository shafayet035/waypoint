# wp_vehicle

The vehicle model, stepped in `vehicle_dynamics` each physics substep:
load transfer → steering → drivetrain and automatic gearbox (shifts on road speed) → per-wheel
relaxed slip → brush tyre force → wheel spin → body force and yaw moment → aero drag.

- Part models are pure functions in `parts.rs`; test them there.
- Low-speed handling: slip states are capped at the sliding limit, relaxation length shrinks
  below 3 m/s and a contact damper fades in. Without these a stopped car rocks on its tyres.
- `set_rolling_speed` starts a vehicle at speed; `telemetry` gives speed, g-forces, gear, rpm.

Validation codes WP-V001..V070 live in `def.rs`; keep messages pointing at the field.
