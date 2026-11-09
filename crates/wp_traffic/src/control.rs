//! Pure controllers for driving along a path: Stanley steering, curve- and
//! stop-limited target speed, and PI speed control. Shared by AI traffic and
//! the scenario drivers.

use glam::Vec2;
use wp_road::Polyline;

/// How far behind and ahead of the last known progress a route follower
/// looks for itself on the path, m.
pub const TRACK_BACK_M: f32 = 5.0;
pub const TRACK_AHEAD_M: f32 = 25.0;

/// Stanley path tracking (Thrun et al., 2006): steer by the heading error at
/// the front axle plus a cross-track term that fades with speed. Returns the
/// front-wheel angle, rad, + left.
/// `near_s` is the vehicle's progress along the path; the front axle is
/// projected onto the path just around it.
pub fn stanley_steering(
    path: &Polyline,
    front_axle: Vec2,
    heading: f32,
    speed: f32,
    near_s: f32,
) -> f32 {
    const GAIN: f32 = 1.2;
    const SOFTENING: f32 = 1.0; // m/s, keeps the term finite at standstill
    let (s, lateral, _) =
        path.project_between(front_axle, near_s - TRACK_BACK_M, near_s + TRACK_AHEAD_M);
    let (_, dir) = path.sample(s + 0.5);
    let heading_error = wrap(dir.y.atan2(dir.x) - heading);
    // `lateral` is + when the axle is left of the path: steer right to return.
    heading_error + (-GAIN * lateral / (speed + SOFTENING)).atan()
}

/// Target speed: the requested speed, limited by bends ahead (2.5 m/s²
/// lateral, comfortable for passengers) and by the distance left to stop.
pub fn route_speed_kmh(path: &Polyline, s: f32, kmh: f32, remaining: f32) -> f32 {
    route_speed_kmh_with(path, s, kmh, remaining, 2.5, 5.0, 5.0, 2.0)
}

/// [`route_speed_kmh`] with its settings: the sideways acceleration allowed
/// in bends and the braking allowed before them (m/s²), and how curvature is
/// sampled (every `step` metres over `window` metres; short, tight junction
/// turns need small values).
#[allow(clippy::too_many_arguments)]
pub fn route_speed_kmh_with(
    path: &Polyline,
    s: f32,
    kmh: f32,
    remaining: f32,
    lateral: f32,
    step: f32,
    window: f32,
    braking: f32,
) -> f32 {
    let mut v = kmh / 3.6;
    let mut d = 0.0;
    while d < 40.0 {
        let (_, t0) = path.sample(s + d);
        let (_, t1) = path.sample(s + d + window);
        let curvature = t0.angle_to(t1).abs() / window;
        if curvature > 1e-4 {
            // Speed allowed at that bend, and what we can brake down to it from here.
            let bend = (lateral / curvature).sqrt();
            v = v.min((bend * bend + 2.0 * braking * d).sqrt());
        }
        d += step;
    }
    v = v.min((2.0 * braking * (remaining - 1.0).max(0.0)).sqrt() + 0.3);
    v * 3.6
}

/// PI speed control on km/h error. Returns (throttle, brake).
pub fn speed_control(integral: &mut f32, target_kmh: f32, speed_kmh: f32, dt: f32) -> (f32, f32) {
    let err = target_kmh - speed_kmh;
    *integral = (*integral + 0.02 * err * dt).clamp(0.0, 0.6);
    let throttle = (0.08 * err + *integral).clamp(0.0, 1.0);
    let brake = if err < -3.0 {
        (-0.05 * err).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (throttle, brake)
}

/// [`speed_control`] on a slope. `pull` is gravity along the heading, m/s²
/// (+ uphill, − downhill; 0 on the flat gives exactly [`speed_control`]);
/// `brake_mps2` is the deceleration at full pedal. Downhill, once the
/// vehicle is at its target speed, brake pressure that cancels the slope is
/// added, so it neither runs away nor overshoots a stop.
pub fn speed_control_on_slope(
    integral: &mut f32,
    target_kmh: f32,
    speed_kmh: f32,
    dt: f32,
    pull: f32,
    brake_mps2: f32,
) -> (f32, f32) {
    let (throttle, brake) = speed_control(integral, target_kmh, speed_kmh, dt);
    if pull >= 0.0 {
        return (throttle, brake);
    }
    // Fades in from 2 km/h under the target to fully at 1 km/h over it.
    let share = ((2.0 - (target_kmh - speed_kmh)) / 3.0).clamp(0.0, 1.0);
    let hold = (-pull / brake_mps2.max(1.0)).min(1.0) * share;
    (throttle * (1.0 - share), (brake + hold).min(1.0))
}

/// Wrap an angle to (−π, π].
pub fn wrap(a: f32) -> f32 {
    let pi = std::f32::consts::PI;
    (a + pi).rem_euclid(2.0 * pi) - pi
}
