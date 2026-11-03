//! Part models, kept as pure functions so they are easy to test and reuse.

/// Brush tyre model with combined slip.
///
/// `slip_ratio` is κ = (ωr − vx)/|vx| and `slip_tan` is tan α = vy/|vx|, both
/// as relaxed state variables. Returns `(fx, fy)` in the tyre frame
/// (x along the wheel heading, y to the left), in newtons.
///
/// The force rises with initial slope `stiffness_per_load × load`, then
/// saturates smoothly at `friction × load`, so a single parameter set covers
/// grip, the limit and full sliding. The force direction follows the combined
/// slip vector, which gives the friction circle for free.
pub fn brush_tyre(
    load: f32,
    friction: f32,
    stiffness_per_load: f32,
    slip_ratio: f32,
    slip_tan: f32,
) -> (f32, f32) {
    if load <= 0.0 || friction <= 0.0 {
        return (0.0, 0.0);
    }
    let kappa = slip_ratio.max(-0.98);
    let denom = 1.0 + kappa;
    let sx = kappa / denom;
    let sy = slip_tan / denom;
    let s = (sx * sx + sy * sy).sqrt();
    if s < 1e-9 {
        return (0.0, 0.0);
    }
    let theta = stiffness_per_load / (3.0 * friction);
    let ts = theta * s;
    let f = if ts < 1.0 {
        friction * load * (3.0 * ts - 3.0 * ts * ts + ts * ts * ts)
    } else {
        friction * load
    };
    (f * sx / s, -f * sy / s)
}

/// Cap relaxed slip states at the point where the tyre fully slides.
///
/// The relaxed slip represents carcass deflection. Past the sliding limit the
/// extra slip is sliding, not stored deflection, so it must not be kept: if it
/// were, a car braking to a stop would be pushed backwards by its own tyres.
/// Returns the capped `(slip_ratio, slip_tan)`.
pub fn cap_slip(
    friction: f32,
    stiffness_per_load: f32,
    slip_ratio: f32,
    slip_tan: f32,
) -> (f32, f32) {
    if friction <= 0.0 || stiffness_per_load <= 0.0 {
        return (slip_ratio, slip_tan);
    }
    let kappa = slip_ratio.max(-0.98);
    let sx = kappa / (1.0 + kappa);
    let sy = slip_tan / (1.0 + kappa);
    let s = (sx * sx + sy * sy).sqrt();
    let limit = 3.0 * friction / stiffness_per_load;
    if s <= limit {
        return (kappa, slip_tan);
    }
    let scale = limit / s;
    let (sx, sy) = (sx * scale, sy * scale);
    let kappa = sx / (1.0 - sx);
    (kappa, sy * (1.0 + kappa))
}

/// Friction available when the contact patch slides.
///
/// `slide` is the kinematic combined slip |V_slip| / max(|v|, 1 m/s). Up to the
/// point where the brush model saturates the tyre keeps its peak friction;
/// beyond it friction falls towards `peak × sliding_ratio` (a locked wheel).
pub fn sliding_friction(peak: f32, sliding_ratio: f32, stiffness_per_load: f32, slide: f32) -> f32 {
    let saturation = 3.0 * peak / stiffness_per_load.max(1e-3);
    let excess = (slide - saturation).max(0.0);
    peak * (sliding_ratio + (1.0 - sliding_ratio) * (-excess / 0.25).exp())
}

/// Thresholds of the ABS controller: release a wheel when its slip passes
/// `RELEASE`, reapply when it recovers above `REAPPLY` (both negative = braking).
pub const ABS_RELEASE_SLIP: f32 = -0.18;
pub const ABS_REAPPLY_SLIP: f32 = -0.10;

/// One step of a per-wheel ABS valve. `factor` scales brake torque (0 = fully
/// released, 1 = full pressure). Returns the new factor.
pub fn abs_step(factor: f32, slip: f32, braking: bool, speed: f32, dt: f32) -> f32 {
    if !braking || speed < 1.5 {
        return 1.0;
    }
    if slip < ABS_RELEASE_SLIP {
        (factor - 30.0 * dt).max(0.0) // dump pressure in about 1/30 s
    } else if slip > ABS_REAPPLY_SLIP {
        (factor + 6.0 * dt).min(1.0) // rebuild over about 1/6 s
    } else {
        factor
    }
}

/// Air brake pressures, bar.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AirState {
    pub reservoir_bar: f32,
    pub chamber_bar: f32,
}

/// Advance an air brake system by `dt` for a pedal `demand` in 0..1. Returns
/// the brake fraction (chamber pressure / full-application pressure).
pub fn air_brake_step(
    state: &mut AirState,
    air: &crate::def::AirBrakeDef,
    demand: f32,
    dt: f32,
) -> f32 {
    let target = (demand.clamp(0.0, 1.0) * air.chamber_full_bar).min(state.reservoir_bar);
    let before = state.chamber_bar;
    state.chamber_bar += (target - state.chamber_bar) * (dt / (air.response_time_s + dt));
    if state.chamber_bar > before {
        state.reservoir_bar -= (state.chamber_bar - before) * air.consumption_ratio;
    }
    state.reservoir_bar =
        (state.reservoir_bar + air.compressor_bar_per_s * dt).min(air.reservoir_max_bar);
    (state.chamber_bar / air.chamber_full_bar).clamp(0.0, 1.0)
}

/// Linear interpolation on a (rpm, torque) curve, clamped at both ends.
pub fn curve_torque(curve: &[(f32, f32)], rpm: f32) -> f32 {
    match curve {
        [] => 0.0,
        [only] => only.1,
        _ => {
            if rpm <= curve[0].0 {
                return curve[0].1;
            }
            for w in curve.windows(2) {
                let ((r0, t0), (r1, t1)) = (w[0], w[1]);
                if rpm <= r1 {
                    let u = (rpm - r0) / (r1 - r0);
                    return t0 + (t1 - t0) * u;
                }
            }
            curve[curve.len() - 1].1
        }
    }
}

/// Integrate wheel spin with brake-like torques that can stop the wheel but
/// never reverse it (static friction behaviour at standstill).
pub fn step_wheel(
    omega: f32,
    drive_torque: f32,
    tyre_torque: f32,
    resist_torque: f32,
    inertia: f32,
    dt: f32,
) -> f32 {
    let free = omega + (drive_torque - tyre_torque) / inertia * dt;
    let stop = resist_torque.max(0.0) / inertia * dt;
    if free.abs() <= stop {
        0.0
    } else {
        free - stop * free.signum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tyre_is_linear_at_small_slip_and_saturates() {
        let (fx, fy) = brush_tyre(4000.0, 1.0, 16.0, 0.001, 0.0);
        assert!((fx - 16.0 * 4000.0 * 0.001).abs() / fx < 0.01, "fx = {fx}");
        assert_eq!(fy, 0.0);
        let (fx, _) = brush_tyre(4000.0, 1.0, 16.0, 0.5, 0.0);
        assert!((fx - 4000.0).abs() < 1.0, "saturated fx = {fx}");
    }

    #[test]
    fn tyre_force_stays_inside_friction_circle() {
        for &(k, a) in &[(0.05, 0.05), (0.2, 0.3), (-0.5, 0.1), (-0.98, 0.8)] {
            let (fx, fy) = brush_tyre(3000.0, 0.9, 15.0, k, a);
            assert!((fx * fx + fy * fy).sqrt() <= 0.9 * 3000.0 + 1e-3);
        }
    }

    #[test]
    fn lateral_force_opposes_slip() {
        let (_, fy) = brush_tyre(3000.0, 1.0, 15.0, 0.0, 0.05);
        assert!(fy < 0.0);
    }

    #[test]
    fn capped_slip_still_gives_full_sliding_force() {
        let (k, a) = cap_slip(1.0, 16.0, -0.9, 0.3);
        let (fx, fy) = brush_tyre(3000.0, 1.0, 16.0, k, a);
        assert!(((fx * fx + fy * fy).sqrt() - 3000.0).abs() < 1.0);
        assert!(fx < 0.0 && fy < 0.0);
        // Small slips are untouched.
        assert_eq!(cap_slip(1.0, 16.0, 0.01, 0.02), (0.01, 0.02));
    }

    #[test]
    fn sliding_loses_friction_only_past_the_peak() {
        assert_eq!(sliding_friction(1.0, 0.75, 16.0, 0.1), 1.0);
        let locked = sliding_friction(1.0, 0.75, 16.0, 1.0);
        assert!((locked - 0.75).abs() < 0.02, "locked μ = {locked}");
    }

    #[test]
    fn abs_releases_and_reapplies() {
        let dt = 1.0 / 480.0;
        let mut f = 1.0;
        for _ in 0..20 {
            f = abs_step(f, -0.4, true, 20.0, dt);
        }
        assert!(f < 0.01, "should have released, f = {f}");
        for _ in 0..100 {
            f = abs_step(f, -0.02, true, 20.0, dt);
        }
        assert!(f > 0.99);
        assert_eq!(abs_step(0.2, -0.5, false, 20.0, dt), 1.0);
    }

    #[test]
    fn air_brakes_lag_and_consume_air() {
        let air = crate::def::AirBrakeDef {
            reservoir_max_bar: 10.0,
            chamber_full_bar: 8.0,
            response_time_s: 0.3,
            compressor_bar_per_s: 0.0,
            consumption_ratio: 0.1,
            warning_bar: 5.5,
        };
        let mut st = AirState {
            reservoir_bar: 10.0,
            chamber_bar: 0.0,
        };
        let dt = 1.0 / 480.0;
        let mut frac = 0.0;
        for _ in 0..(0.3 / dt) as usize {
            frac = air_brake_step(&mut st, &air, 1.0, dt);
        }
        assert!(
            (0.55..0.7).contains(&frac),
            "63% after one time constant, got {frac}"
        );
        for _ in 0..2000 {
            frac = air_brake_step(&mut st, &air, 1.0, dt);
        }
        assert!(frac > 0.99);
        assert!(
            (st.reservoir_bar - 9.2).abs() < 0.01,
            "8 bar × 0.1 consumed, left {}",
            st.reservoir_bar
        );
    }

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = [(1000.0, 100.0), (3000.0, 200.0)];
        assert_eq!(curve_torque(&c, 500.0), 100.0);
        assert_eq!(curve_torque(&c, 2000.0), 150.0);
        assert_eq!(curve_torque(&c, 9000.0), 200.0);
    }

    #[test]
    fn brakes_stop_but_do_not_reverse() {
        assert_eq!(step_wheel(1.0, 0.0, 0.0, 1000.0, 1.0, 0.01), 0.0);
        let w = step_wheel(10.0, 0.0, 0.0, 100.0, 1.0, 0.01);
        assert!((w - 9.0).abs() < 1e-6);
    }
}
