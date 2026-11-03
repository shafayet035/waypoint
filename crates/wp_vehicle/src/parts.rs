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
