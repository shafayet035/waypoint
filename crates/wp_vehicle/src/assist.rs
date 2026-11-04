//! Driver aids: turn what a player presses (keys, thumbs) into vehicle
//! inputs that feel right. Keys are all-or-nothing, so without help a tap on
//! the brake is an emergency stop and a tap on steer is full lock at any
//! speed. Pure and deterministic, so scenarios drive through exactly what a
//! player gets.

use crate::{Telemetry, VehicleDef, VehicleInput};

/// What the player is pressing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    /// Accelerate, 0 to 1 (brakes while rolling backwards).
    pub accelerate: f32,
    /// Brake, 0 to 1 (reverses when held at a standstill).
    pub brake: f32,
    /// Steering, −1 (right) to 1 (left).
    pub steer: f32,
    /// `steer` is an analogue position (a thumb or stick), not a key.
    pub analog_steer: bool,
}

/// First bite of the brakes when a key is pressed, 0 to 1.
const BRAKE_BITE: f32 = 0.12;
/// How fast the brakes build while the key is held, per second (enough for
/// the ABS limit in about 0.9 s).
const BRAKE_BUILD_PER_S: f32 = 0.9;
/// How fast the pedals follow their targets, per second.
const THROTTLE_RATE: f32 = 3.0;
const BRAKE_RATE: f32 = 6.0;
/// Key steering winds on this fast at a standstill and at speed (full stick
/// per second), and returns to centre faster still.
const STEER_ON_SLOW: f32 = 4.0;
const STEER_ON_FAST: f32 = 1.2;
const STEER_RETURN_SLOW: f32 = 6.0;
const STEER_RETURN_FAST: f32 = 3.0;
const STEER_ANALOG: f32 = 8.0;
/// Counter-steer: when the car slides, steer this much towards the direction
/// of travel per unit of sideslip (front wheels follow the slide, as a good
/// driver's would). Sideslip up to `SLIDE_START_DEG` is normal at the grip
/// limit and left alone.
const COUNTER_STEER: f32 = 1.5;
const SLIDE_START_DEG: f32 = 4.0;

/// The aids' state between ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriverAids {
    /// Steering position before the speed limit, −1 to 1.
    stick: f32,
    brake_held_s: f32,
    input: VehicleInput,
}

impl DriverAids {
    /// One step: the vehicle input for these controls, `dt` seconds after the last.
    pub fn update(
        &mut self,
        c: Controls,
        motion: &Telemetry,
        def: &VehicleDef,
        dt: f32,
    ) -> VehicleInput {
        let forward_kmh = motion.forward_kmh;
        let approach =
            |v: f32, target: f32, rate: f32| v + (target - v).clamp(-rate * dt, rate * dt);
        // Driving-game convention: holding brake at a standstill selects
        // reverse and backs up; accelerate then brakes until stopped, after
        // which it selects drive again.
        let input = &mut self.input;
        if c.brake > 0.5 && c.accelerate < 0.5 && forward_kmh < 1.0 {
            input.reverse = true;
        } else if c.accelerate > 0.5 && c.brake < 0.5 && forward_kmh > -1.0 {
            input.reverse = false;
        }
        let (go, slow) = if input.reverse {
            (c.brake, c.accelerate)
        } else {
            (c.accelerate, c.brake)
        };

        // Brakes: a tap bites gently; holding builds to full.
        self.brake_held_s = if slow > 0.0 {
            self.brake_held_s + dt
        } else {
            0.0
        };
        let brake = slow * (BRAKE_BITE + BRAKE_BUILD_PER_S * self.brake_held_s).min(1.0);
        input.throttle = approach(input.throttle, go, THROTTLE_RATE);
        input.brake = approach(input.brake, brake, BRAKE_RATE);

        // Steering: quick to respond, quicker to straighten, and never more
        // lock than the front tyres can use at this speed.
        let speed = forward_kmh.abs() / 3.6;
        let target = c.steer.clamp(-1.0, 1.0);
        let returning = target == 0.0 || target * self.stick < 0.0;
        let t = (speed / 25.0).min(1.0);
        let lerp = |slow: f32, fast: f32| slow + (fast - slow) * t;
        let rate = if c.analog_steer {
            STEER_ANALOG
        } else if returning {
            lerp(STEER_RETURN_SLOW, STEER_RETURN_FAST)
        } else {
            lerp(STEER_ON_SLOW, STEER_ON_FAST)
        };
        self.stick = approach(self.stick, target, rate);
        let max = def.steering.max_angle_deg.to_radians().max(1e-3);
        let slide =
            motion.sideslip_deg.signum() * (motion.sideslip_deg.abs() - SLIDE_START_DEG).max(0.0);
        let counter = COUNTER_STEER * slide.to_radians() / max;
        input.steer = (self.stick * steer_limit(def, speed) + counter).clamp(-1.0, 1.0);
        *input
    }
}

/// Fraction of full steering lock (0 to 1) that keeps the front tyres near
/// their peak grip at `speed_ms`: the angle for the tightest circle the tyres
/// can hold (wheelbase × μg / v²), plus a little slip angle for the tyres to
/// work with. Full lock at parking speeds; about 10° of 35° at 60 km/h in a hatchback.
pub fn steer_limit(def: &VehicleDef, speed_ms: f32) -> f32 {
    let max = def.steering.max_angle_deg.to_radians().max(1e-3);
    let mu = def.tyre.friction;
    let path = (def.wheelbase_m * mu * 9.81 / speed_ms.max(0.1).powi(2)).atan();
    let slip = (1.2 * mu / def.tyre.stiffness_per_load.max(1e-3)).atan();
    ((path + slip) / max).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hatchback() -> VehicleDef {
        let src = include_str!("../../../examples/sandbox/vehicles/hatchback.vehicle.ron");
        wp_data::parse_ron("hatchback.vehicle.ron", src).unwrap()
    }

    fn run(aids: &mut DriverAids, c: Controls, kmh: f32, seconds: f32) -> VehicleInput {
        let def = hatchback();
        let motion = Telemetry {
            forward_kmh: kmh,
            speed_kmh: kmh.abs(),
            ..Default::default()
        };
        let mut out = VehicleInput::default();
        for _ in 0..(seconds * 60.0).round() as usize {
            out = aids.update(c, &motion, &def, 1.0 / 60.0);
        }
        out
    }

    #[test]
    fn counter_steers_into_a_slide() {
        let def = hatchback();
        // Sliding to the right of where the car points: steer right.
        let motion = Telemetry {
            forward_kmh: 60.0,
            speed_kmh: 60.0,
            sideslip_deg: -8.0,
            ..Default::default()
        };
        let out = DriverAids::default().update(Controls::default(), &motion, &def, 1.0 / 60.0);
        assert!(out.steer < -0.1, "{}", out.steer);
    }

    #[test]
    fn steering_lock_shrinks_with_speed() {
        let def = hatchback();
        assert_eq!(steer_limit(&def, 2.0), 1.0, "full lock when parking");
        let at_60 = steer_limit(&def, 60.0 / 3.6) * def.steering.max_angle_deg;
        assert!((8.0..12.0).contains(&at_60), "{at_60}° at 60 km/h");
        assert!(steer_limit(&def, 120.0 / 3.6) < steer_limit(&def, 60.0 / 3.6));
    }

    #[test]
    fn a_brake_tap_bites_gently_and_holding_builds_to_full() {
        let brake = Controls {
            brake: 1.0,
            ..Default::default()
        };
        let tap = run(&mut DriverAids::default(), brake, 50.0, 0.15);
        assert!(tap.brake < 0.6, "tap gives {}", tap.brake);
        let held = run(&mut DriverAids::default(), brake, 50.0, 1.1);
        assert_eq!(held.brake, 1.0);
        assert!(!held.reverse, "no reverse while rolling forward");
    }

    #[test]
    fn steering_responds_quickly_and_recentres() {
        let mut aids = DriverAids::default();
        let left = Controls {
            steer: 1.0,
            ..Default::default()
        };
        let on = run(&mut aids, left, 10.0, 0.3);
        assert!(
            on.steer > 0.9 * steer_limit(&hatchback(), 10.0 / 3.6),
            "{}",
            on.steer
        );
        let off = run(&mut aids, Controls::default(), 10.0, 0.2);
        assert_eq!(off.steer, 0.0);
    }

    #[test]
    fn holding_brake_at_a_standstill_reverses() {
        let brake = Controls {
            brake: 1.0,
            ..Default::default()
        };
        let out = run(&mut DriverAids::default(), brake, 0.0, 0.5);
        assert!(out.reverse && out.throttle > 0.9 && out.brake == 0.0);
    }
}
